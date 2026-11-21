use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::{
    build::{mod_id_of, read_mod_id},
    config::{Config, INDEX_DIRS, LOCAL_DIR, set_versions},
    ctx::Ctx,
    git::{Files, Git, SNAPSHOT_PREFIX, as_blob_ref, blob_ref, is_committed_binary},
    index::{
        DISABLED_SUFFIX, INDEX_SUFFIX, IndexFile, Side, current_os, index_dir_of, index_name,
        index_path, is_indexed_binary,
    },
    modrinth,
    pack::{PackFile, Source, pack_files},
    target::Target,
};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    Added(String),
    Modified(String),
    Deleted(String),
}

impl Change {
    pub fn path(&self) -> &str {
        match self {
            Change::Added(path) | Change::Modified(path) | Change::Deleted(path) => path,
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Added(path) => write!(f, "A {path}"),
            Change::Modified(path) => write!(f, "M {path}"),
            Change::Deleted(path) => write!(f, "D {path}"),
        }
    }
}

pub fn diff(old: &Files, new: &Files) -> Vec<Change> {
    let mut changes = vec![];
    for (path, content) in new {
        match old.get(path) {
            None => changes.push(Change::Added(path.clone())),
            Some(previous) if previous != content => changes.push(Change::Modified(path.clone())),
            _ => {}
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            changes.push(Change::Deleted(path.clone()));
        }
    }
    changes.sort();
    changes
}

/// The part of `tree` a target holds. Servers' jars go through `apply_jars` instead.
pub fn domain(tree: &Files, config: &Config, target: &Target) -> anyhow::Result<Files> {
    let os = current_os();
    let mut files = Files::new();
    for (path, content) in tree {
        if config.is_managed_file(path) && !target.is_excluded(path) {
            files.insert(path.clone(), content.clone());
        }
    }
    if target.is_prism() {
        for file in pack_files(tree, config)? {
            if !(file.side()?.includes(Side::Client) && file.applies_to_os(os)?) {
                continue;
            }
            if let Some((path, _)) = &file.index {
                files.insert(path.clone(), tree[path].clone());
            }
            if let Source::Committed { path, .. } = &file.source {
                files.insert(path.clone(), tree[path].clone());
            }
        }
    }
    Ok(files)
}

fn jar_entries(tree: &Files, config: &Config, target: &Target) -> anyhow::Result<Vec<PackFile>> {
    let os = current_os();
    let mut entries = vec![];
    for file in pack_files(tree, config)? {
        if (target.is_prism() || file.dir == "mods")
            && file.side()?.includes(target.side())
            && file.applies_to_os(os)?
        {
            entries.push(file);
        }
    }
    Ok(entries)
}

fn rel_path(root: &Path, path: &Path) -> anyhow::Result<String> {
    Ok(path
        .strip_prefix(root)?
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn walk(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Pull,
    /// No network: unknown jars get a placeholder that never matches.
    Check,
}

fn file_stem(filename: &str) -> &str {
    let name = filename.strip_suffix(DISABLED_SUFFIX).unwrap_or(filename);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn read_prism_indexes(
    game_dir: &Path,
    dir: &str,
) -> anyhow::Result<HashMap<String, (String, IndexFile)>> {
    let mut indexes = HashMap::new();
    let Ok(entries) = std::fs::read_dir(game_dir.join(dir).join(".index")) else {
        return Ok(indexes);
    };
    for entry in entries {
        let path = entry?.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = file_name.strip_suffix(INDEX_SUFFIX) else {
            continue;
        };
        let index = IndexFile::parse(&std::fs::read_to_string(&path)?)
            .with_context(|| path.display().to_string())?;
        if let Some(filename) = index.filename() {
            indexes.insert(filename.to_string(), (name.to_string(), index));
        }
    }
    Ok(indexes)
}

pub async fn read_target(
    ctx: &mut Ctx,
    config: &Config,
    target: &Target,
    base_tree: &Files,
    base_domain: &Files,
    mode: Mode,
) -> anyhow::Result<Files> {
    let mut snapshot = Files::new();
    if target.is_prism() {
        Box::pin(read_prism(
            ctx,
            config,
            target,
            base_tree,
            base_domain,
            mode,
            &mut snapshot,
        ))
        .await?;
    } else {
        for path in base_domain.keys() {
            if let Ok(content) = std::fs::read(target.game_dir.join(path)) {
                snapshot.insert(path.clone(), content);
            }
        }
    }
    Ok(snapshot)
}

fn prism_managed_files(config: &Config, game_dir: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut found = vec![];
    for root in config.managed_roots() {
        let full = game_dir.join(&root);
        let files = if full.is_dir() {
            walk(&full)?
        } else if full.is_file() {
            vec![full]
        } else {
            vec![]
        };
        for file in files {
            let rel = rel_path(game_dir, &file)?;
            if rel
                .split('/')
                .any(|part| part == ".index" || part == LOCAL_DIR)
            {
                continue;
            }
            if let Some((dir, name)) = rel.split_once('/')
                && INDEX_DIRS.contains(&dir)
                && !name.contains('/')
                && is_indexed_binary(dir, name)
            {
                continue;
            }
            found.push((rel, file));
        }
    }
    Ok(found)
}

async fn read_prism(
    ctx: &mut Ctx,
    config: &Config,
    target: &Target,
    base_tree: &Files,
    base_domain: &Files,
    mode: Mode,
    snapshot: &mut Files,
) -> anyhow::Result<()> {
    let game_dir = &target.game_dir;
    for (rel, file) in prism_managed_files(config, game_dir)? {
        snapshot.insert(rel, std::fs::read(&file)?);
    }

    for dir in INDEX_DIRS {
        if !config.handles_index_dir(dir) {
            continue;
        }
        let prism_indexes = read_prism_indexes(game_dir, dir)?;
        let mut base_by_filename = HashMap::new();
        let mut base_committed = HashMap::new();
        for (path, content) in base_domain {
            if index_dir_of(path) == Some(dir) {
                let entry = IndexFile::parse_bytes(content)?;
                if let Some(filename) = entry.filename() {
                    base_by_filename
                        .insert(filename.to_string(), (path.clone(), entry, content.clone()));
                }
            } else if is_committed_binary(path)
                && let Some((path_dir, filename)) = path.split_once('/')
                && path_dir == dir
                && let Some(blob) = as_blob_ref(content)
            {
                base_committed.insert(
                    filename.to_string(),
                    (path.clone(), blob.to_string(), content.clone()),
                );
            }
        }
        let Ok(dir_entries) = std::fs::read_dir(game_dir.join(dir)) else {
            continue;
        };
        let mut binaries = vec![];
        for entry in dir_entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_file() && is_indexed_binary(dir, &name) {
                binaries.push((name, entry.path()));
            }
        }
        binaries.sort();
        let mut seen = HashSet::new();
        for (name, _) in &binaries {
            let filename = name.strip_suffix(DISABLED_SUFFIX).unwrap_or(name);
            if !seen.insert(filename) {
                bail!(
                    "{dir}/{filename} is both enabled and disabled in {}, remove one",
                    target.name
                );
            }
        }
        if dir == "mods" {
            let mut by_mod_id = HashMap::new();
            for (name, path) in &binaries {
                if name.ends_with(DISABLED_SUFFIX) {
                    continue;
                }
                let Ok(Some(mod_id)) = read_mod_id(path).await else {
                    continue;
                };
                if let Some(other) = by_mod_id.insert(mod_id.clone(), name) {
                    bail!(
                        "mods/{other} and mods/{name} are both mod \"{mod_id}\" in {}, remove one",
                        target.name
                    );
                }
            }
        }
        let present = binaries
            .iter()
            .map(|(name, _)| {
                name.strip_suffix(DISABLED_SUFFIX)
                    .unwrap_or(name)
                    .to_string()
            })
            .collect::<HashSet<_>>();
        // A renamed jar may be an update of one of these.
        let mut vanished = base_by_filename
            .iter()
            .filter(|(filename, _)| !present.contains(filename.as_str()))
            .map(|(_, (path, entry, _))| (path.clone(), entry.clone()))
            .collect::<Vec<_>>();
        for (name, path) in binaries {
            let filename = name
                .strip_suffix(DISABLED_SUFFIX)
                .unwrap_or(&name)
                .to_string();
            let rel = format!("{dir}/{filename}");
            let sha1 = ctx.hashes.sha1(&path).await?;

            // A committed file stays committed.
            if let Some((base_path, blob, content)) = base_committed.get(&filename) {
                let (base_sha1, _) = ctx.committed(blob, base_path).await?;
                let content = if base_sha1 == sha1 {
                    content.clone()
                } else {
                    store_blob(ctx, mode, &rel, &path, &sha1)?
                };
                snapshot.insert(base_path.clone(), content);
                if let Some((index_path, _, index_content)) = base_by_filename.get(&filename) {
                    snapshot.insert(index_path.clone(), index_content.clone());
                }
                continue;
            }

            if let Some((base_path, base_entry, base_content)) = base_by_filename.get(&filename)
                && base_entry.sha1() == Some(sha1.as_str())
            {
                snapshot.insert(base_path.clone(), base_content.clone());
                continue;
            }

            let found = Box::pin(make_entry(
                ctx,
                prism_indexes.get(&filename),
                &filename,
                &sha1,
                mode,
            ))
            .await?;
            if let Some((base_path, base_entry, _)) = base_by_filename.get(&filename) {
                let mut entry = match found {
                    Found::Index(_, entry) => entry,
                    Found::Committed(index) => {
                        snapshot.insert(rel.clone(), store_blob(ctx, mode, &rel, &path, &sha1)?);
                        index.map_or_else(
                            || IndexFile::metadata(file_stem(&filename), &filename),
                            |(_, index)| index,
                        )
                    }
                };
                entry.carry_overrides(base_entry);
                snapshot.insert(base_path.clone(), entry.to_bytes());
                continue;
            }

            let (name, entry, write_new) = match found {
                Found::Index(name, entry) => (name, entry, true),
                Found::Committed(index) => {
                    snapshot.insert(rel.clone(), store_blob(ctx, mode, &rel, &path, &sha1)?);
                    match index {
                        Some((name, index)) => (name, index, true),
                        None => {
                            let stem = file_stem(&filename);
                            (
                                stem.to_string(),
                                IndexFile::metadata(stem, &filename),
                                false,
                            )
                        }
                    }
                }
            };
            let mut position = find_update(&vanished, &entry, &name);
            if position.is_none() && dir == "mods" && mode == Mode::Pull {
                position = Box::pin(find_update_by_mod_id(
                    ctx,
                    &vanished,
                    &base_committed,
                    &path,
                ))
                .await;
            }
            if let Some(position) = position {
                let (base_path, base_entry) = vanished.remove(position);
                let mut entry = entry;
                entry.carry_overrides(&base_entry);
                snapshot.insert(base_path, entry.to_bytes());
                continue;
            }
            if !write_new {
                continue;
            }
            let mut index = index_path(dir, &name);
            if base_tree.contains_key(&index) || snapshot.contains_key(&index) {
                index = index_path(dir, &format!("{name}-{}", current_os()));
            }
            snapshot.insert(index, entry.to_bytes());
        }
    }
    Ok(())
}

/// The only vanished entry with the same project, else the same index name.
fn find_update(vanished: &[(String, IndexFile)], entry: &IndexFile, name: &str) -> Option<usize> {
    let only = |matches: Vec<usize>| (matches.len() == 1).then(|| matches[0]);
    if let Some(project) = entry.project() {
        let same = vanished
            .iter()
            .enumerate()
            .filter(|(_, (_, base))| base.project().as_deref() == Some(project.as_str()))
            .map(|(position, _)| position)
            .collect::<Vec<_>>();
        if !same.is_empty() {
            return only(same);
        }
    }
    only(
        vanished
            .iter()
            .enumerate()
            .filter(|(_, (path, _))| index_name(path) == name)
            .map(|(position, _)| position)
            .collect(),
    )
}

enum Found {
    Index(String, IndexFile),
    /// With Prism's index file, minus its download.
    Committed(Option<(String, IndexFile)>),
}

/// Fallback for `find_update`: the only vanished entry with the same mod id.
async fn find_update_by_mod_id(
    ctx: &mut Ctx,
    vanished: &[(String, IndexFile)],
    base_committed: &HashMap<String, (String, String, Vec<u8>)>,
    jar: &Path,
) -> Option<usize> {
    let new_id = mod_id_of(jar).await.ok()?;
    let mut matches = vec![];
    for (position, (_, base)) in vanished.iter().enumerate() {
        let old_jar = match (base.url(), base.sha1()) {
            (Some(url), Some(sha1)) => {
                let Ok(url) = url.parse() else {
                    continue;
                };
                match ctx.files.ensure(&[(url, sha1.to_string())]).await {
                    Ok(mut cached) => cached.remove(sha1),
                    Err(_) => None,
                }
            }
            _ => match base
                .filename()
                .and_then(|filename| base_committed.get(filename))
            {
                Some((path, blob, _)) => ctx
                    .committed(blob, path)
                    .await
                    .ok()
                    .map(|(_, cached)| cached),
                None => None,
            },
        };
        if let Some(old_jar) = old_jar
            && mod_id_of(&old_jar).await.ok().as_deref() == Some(new_id.as_str())
        {
            matches.push(position);
        }
    }
    (matches.len() == 1).then(|| matches[0])
}

async fn make_entry(
    ctx: &Ctx,
    prism_index: Option<&(String, IndexFile)>,
    filename: &str,
    sha1: &str,
    mode: Mode,
) -> anyhow::Result<Found> {
    if let Some((name, index)) = prism_index {
        let mut entry = index.clone();
        if entry.url().is_none() {
            entry.strip_download();
            return Ok(Found::Committed(Some((name.clone(), entry))));
        }
        entry.convert(sha1);
        return Ok(Found::Index(name.clone(), entry));
    }
    let name = file_stem(filename).to_string();
    if mode == Mode::Check {
        return Ok(Found::Index(
            name,
            IndexFile::new(file_stem(filename), filename, "unindexed", sha1),
        ));
    }
    if let Some(found) = modrinth::file_by_sha1(&ctx.client, sha1).await? {
        let mut entry = IndexFile::new(&name, filename, &found.url, sha1);
        entry.set_modrinth(&found.project_id, &found.version_id);
        return Ok(Found::Index(name, entry));
    }
    Ok(Found::Committed(None))
}

fn store_blob(
    ctx: &mut Ctx,
    mode: Mode,
    rel: &str,
    path: &Path,
    sha1: &str,
) -> anyhow::Result<Vec<u8>> {
    if mode == Mode::Check {
        return Ok(blob_ref("unsynced"));
    }
    let blob = ctx.git.store_file(rel, path)?;
    ctx.hashes.set_blob_sha1(&blob, sha1);
    Ok(blob_ref(&blob))
}

pub fn ensure_committed(git: &Git, relevant: impl Fn(&str) -> bool) -> anyhow::Result<()> {
    let paths = git
        .changed_paths()?
        .into_iter()
        .filter(|path| relevant(path))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(());
    }
    let mut list = paths[..paths.len().min(5)].join(", ");
    if paths.len() > 5 {
        list.push_str(&format!(" and {} more", paths.len() - 5));
    }
    let verb = if paths.len() == 1 { "has" } else { "have" };
    bail!("{list} {verb} uncommitted changes, commit them first");
}

/// The same update on both sides would otherwise conflict over index file bytes.
/// Index files that describe the same jar are the same version.
fn same_jar(path: &str, a: &[u8], b: &[u8]) -> bool {
    if index_dir_of(path).is_none() {
        return false;
    }
    let (Ok(a), Ok(b)) = (IndexFile::parse_bytes(a), IndexFile::parse_bytes(b)) else {
        return false;
    };
    a.sha1().is_some() && a.sha1() == b.sha1() && a.filename() == b.filename()
}

fn align_with_head(files: &mut Files, head_tree: &Files) {
    for (path, content) in files.iter_mut() {
        if let Some(head) = head_tree.get(path)
            && same_jar(path, head, content)
        {
            *content = head.clone();
        }
    }
}

struct Agreed {
    files: Files,
    /// Why a path the target still has as agreed isn't a change there.
    notes: HashMap<String, &'static str>,
}

/// Each path as Prism and the repo last agreed on it: its version in the newest
/// pull that was then committed, else the record's. Without a record, a version
/// anywhere in the history counts, since an instance never synced is an old copy.
fn agreed(
    git: &Git,
    base: Option<&str>,
    base_domain: &Files,
    pulled: &[(String, String)],
    target_files: &Files,
) -> anyhow::Result<Agreed> {
    let mut agreed = Agreed {
        files: base_domain.clone(),
        notes: HashMap::new(),
    };
    let paths = diff(base_domain, target_files)
        .iter()
        .map(|change| change.path().to_string())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(agreed);
    }
    let history = match base {
        Some(_) => HashMap::new(),
        None => git.path_versions("HEAD", &paths)?,
    };
    let mut snapshots = vec![];
    for (snapshot, head) in pulled.iter().rev() {
        if git.commit_exists(snapshot) && git.commit_exists(head) {
            snapshots.push((
                git.path_blobs(snapshot, &paths)?,
                git.path_versions(&format!("{head}..HEAD"), &paths)?,
            ));
        }
    }
    let present = paths
        .iter()
        .filter_map(|path| target_files.get(path).map(Vec::as_slice))
        .collect::<Vec<_>>();
    let mut ids = git.blob_ids(&present)?.into_iter();
    for path in paths {
        let id = target_files
            .contains_key(&path)
            .then(|| ids.next())
            .flatten();
        let in_history = history.get(&path).map_or(&[][..], Vec::as_slice);
        let mut version = None;
        let mut note = "already pulled";
        for (blobs, since_pull) in &snapshots {
            let pulled_id = blobs.get(&path).cloned();
            let since_pull = since_pull.get(&path).map_or(&[][..], Vec::as_slice);
            let content = match &pulled_id {
                Some(id) if is_committed_binary(&path) => Some(blob_ref(id)),
                Some(id) => git.read_blobs(std::slice::from_ref(id))?.pop(),
                None => None,
            };
            if has_version(git, &path, content.as_deref(), &pulled_id, since_pull)? {
                version = Some(content);
                break;
            }
        }
        let target = target_files.get(&path);
        if version.is_none()
            && base.is_none()
            && has_version(git, &path, target.map(Vec::as_slice), &id, in_history)?
        {
            version = Some(target.cloned());
            note = "older version";
        }
        let Some(version) = version else {
            continue;
        };
        if version.as_ref() == target {
            agreed.notes.insert(path.clone(), note);
        }
        match version {
            Some(content) => agreed.files.insert(path, content),
            None => agreed.files.remove(&path),
        };
    }
    Ok(agreed)
}

/// Whether `versions` has this version.
fn has_version(
    git: &Git,
    path: &str,
    content: Option<&[u8]>,
    id: &Option<String>,
    versions: &[Option<String>],
) -> anyhow::Result<bool> {
    if versions.contains(id) {
        return Ok(true);
    }
    let Some(content) = content.filter(|_| index_dir_of(path).is_some()) else {
        return Ok(false);
    };
    let old = git.read_blobs(&versions.iter().flatten().cloned().collect::<Vec<_>>())?;
    Ok(old.iter().any(|old| same_jar(path, old, content)))
}

/// The record, moved first to the newest commit that has exactly the files of
/// a pull since then (that pull was committed).
fn current_base(git: &Git, config: &Config, target: &Target) -> anyhow::Result<Option<String>> {
    let base = target
        .recorded_base()
        .filter(|commit| git.commit_exists(commit));
    let mut domains: HashMap<String, Files> = HashMap::new();
    for (snapshot, head) in target.pulled().iter().rev() {
        if !git.commit_exists(snapshot) || !git.commit_exists(head) {
            continue;
        }
        let pulled = domain(&git.read_tree(snapshot)?, config, target)?;
        for commit in git.commits_since(head)? {
            if !domains.contains_key(&commit) {
                let files = domain(&git.read_tree(&commit)?, config, target)?;
                domains.insert(commit.clone(), files);
            }
            if domains[&commit] == pulled {
                set_record(git, target, &commit)?;
                return Ok(Some(commit));
            }
        }
    }
    Ok(base)
}

struct State {
    base: Option<String>,
    base_tree: Files,
    base_domain: Files,
    head_tree: Files,
    head_domain: Files,
    snapshot: Files,
    agreed: Agreed,
}

impl State {
    /// Changes in the target since it last agreed with the repo, that the commit
    /// doesn't already have.
    fn changed_there(&self) -> Vec<Change> {
        let mut changes = diff(&self.agreed.files, &self.snapshot);
        changes.retain(|change| {
            self.snapshot.get(change.path()) != self.head_domain.get(change.path())
        });
        changes
    }

    /// Paths the target has that differ from the commit, but aren't changes there.
    fn skipped(&self) -> Vec<(&str, &'static str)> {
        self.agreed
            .notes
            .iter()
            .filter(|(path, _)| self.snapshot.get(*path) != self.head_domain.get(*path))
            .map(|(path, note)| (path.as_str(), *note))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect()
    }
}

/// Reads the target and works out what it agrees on with the repo. With
/// `against_head`, the last commit counts as agreed on.
async fn sync_state(
    ctx: &mut Ctx,
    config: &Config,
    target: &Target,
    mode: Mode,
    against_head: bool,
) -> anyhow::Result<State> {
    let base = current_base(&ctx.git, config, target)?;
    let base_tree = match &base {
        Some(base) => ctx.git.read_tree(base)?,
        None => Files::new(),
    };
    let base_domain = domain(&base_tree, config, target)?;
    let head_tree = ctx.git.read_tree(&ctx.git.head()?)?;
    let head_domain = domain(&head_tree, config, target)?;
    let (known_tree, known_domain) = match &base {
        Some(_) if !against_head => (&base_tree, &base_domain),
        _ => (&head_tree, &head_domain),
    };
    let known_domain = read_paths(target, known_domain, &head_domain);
    let mut snapshot = read_target(ctx, config, target, known_tree, &known_domain, mode).await?;
    ctx.hashes.save()?;
    align_with_head(&mut snapshot, &head_tree);
    let agreed = if against_head {
        Agreed {
            files: head_domain.clone(),
            notes: HashMap::new(),
        }
    } else {
        let pulled = target.pulled();
        agreed(&ctx.git, base.as_deref(), &base_domain, &pulled, &snapshot)?
    };
    Ok(State {
        base,
        base_tree,
        base_domain,
        head_tree,
        head_domain,
        snapshot,
        agreed,
    })
}

fn set_record(git: &Git, target: &Target, commit: &str) -> anyhow::Result<()> {
    target.record_base(commit)?;
    git.update_ref(&target.ref_name(), commit)?;
    target.clear_pulled()?;
    git.delete_refs(&target.pulled_ref_prefix())
}

fn remember_pull(git: &Git, target: &Target, snapshot: &str, head: &str) -> anyhow::Result<()> {
    target.add_pulled(snapshot, head)?;
    git.update_ref(
        &format!("{}{snapshot}", target.pulled_ref_prefix()),
        snapshot,
    )
}

fn list_changes(changes: &[Change], notes: &HashMap<String, &str>, place: &str) -> String {
    changes
        .iter()
        .map(|change| match notes.get(change.path()) {
            Some(note) => format!("  {change} ({note} in {place})\n"),
            None => format!("  {change}\n"),
        })
        .collect()
}

pub async fn pull(ctx: &mut Ctx, name: &str, force: bool) -> anyhow::Result<()> {
    let config = ctx.config()?;
    let target = Target::resolve(&ctx.repo, name, &config, &ctx.local()?)?;
    if !target.is_prism() {
        bail!("can't pull from a server");
    }
    let head = ctx.git.head()?;
    let state = sync_state(ctx, &config, &target, Mode::Pull, force).await?;
    let changes = state.changed_there();
    if changes.is_empty() {
        println!("{name}: nothing to pull");
    } else {
        let agreed_commit = ctx.git.write_commit(
            None,
            &state.agreed.files,
            &format!("{SNAPSHOT_PREFIX} base: {name}"),
        )?;
        let commit = ctx.git.write_commit(
            Some(&agreed_commit),
            &state.snapshot,
            &format!("{SNAPSHOT_PREFIX}: {name}"),
        )?;
        let merged = ctx.git.cherry_pick_no_commit(&commit)?;
        remember_pull(&ctx.git, &target, &commit, &head)?;
        println!("{name}:");
        print!("{}", list_changes(&changes, &HashMap::new(), name));
        if !merged {
            println!("conflicts, resolve them and commit");
        }
    }
    for (path, note) in state.skipped() {
        println!("skipped {path}: {note} in {name}");
    }

    if let Some((minecraft, loader, loader_version)) = target.mmc_pack()?
        && set_versions(&ctx.repo, &minecraft, loader, loader_version.as_deref())?
    {
        println!("updated versions in potato.toml");
    }
    Ok(())
}

/// Servers are read path by path, so also read the paths only the commit has.
fn read_paths(target: &Target, known_domain: &Files, head_domain: &Files) -> Files {
    let mut paths = known_domain.clone();
    if !target.is_prism() {
        for (path, content) in head_domain {
            paths.entry(path.clone()).or_insert_with(|| content.clone());
        }
    }
    paths
}

pub async fn push(ctx: &mut Ctx, name: &str, force: bool) -> anyhow::Result<()> {
    let head = ctx.git.head()?;
    let config = ctx.config()?;
    let target = Target::resolve(&ctx.repo, name, &config, &ctx.local()?)?;
    if !force {
        ensure_committed(&ctx.git, |path| {
            config.is_pack_file(path) && !target.is_excluded(path)
        })?;
    }
    if !force && target.is_running() {
        bail!("{name} is running, stop it first");
    }
    let state = sync_state(ctx, &config, &target, Mode::Check, false).await?;
    let changes = state.changed_there();
    let State {
        base,
        mut base_tree,
        mut base_domain,
        head_tree,
        head_domain,
        snapshot,
        agreed,
    } = state;
    let pushed = diff(&snapshot, &head_domain);
    if !force {
        if !changes.is_empty() {
            let list = list_changes(&changes, &HashMap::new(), name);
            if target.is_prism() {
                bail!(
                    "{name} has changes you haven't committed, run `potato pull {name}` first or use --force:\n{}",
                    list.trim_end()
                );
            }
            bail!(
                "{name} has changes since the last push, use --force to overwrite them:\n{}",
                list.trim_end()
            );
        }
        // Nothing would be lost, so what the target has now can be replaced. A
        // server's snapshot only holds the record's files, so it keeps the record.
        if target.is_prism() || base.is_none() {
            base_tree = snapshot.clone();
            base_domain = snapshot;
        }
    }

    for (path, content) in &head_domain {
        if index_dir_of(path).is_some() || is_committed_binary(path) {
            continue;
        }
        let full = target.game_dir.join(path);
        if std::fs::read(&full).ok().as_ref() != Some(content) {
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, content)?;
        }
    }
    for path in base_domain.keys() {
        if index_dir_of(path).is_none()
            && !is_committed_binary(path)
            && !head_domain.contains_key(path)
        {
            remove_if_exists(&target.game_dir.join(path))?;
        }
    }
    // Servers keep their own files.
    let exact = force && target.is_prism();
    if exact {
        for (rel, file) in prism_managed_files(&config, &target.game_dir)? {
            if !head_domain.contains_key(&rel) {
                remove_if_exists(&file)?;
            }
        }
    }
    apply_jars(ctx, &config, &target, &base_tree, &head_tree, exact).await?;
    ctx.hashes.save()?;

    set_record(&ctx.git, &target, &head)?;
    if pushed.is_empty() {
        println!("{name}: nothing to push");
    } else {
        println!("{name}:");
        print!("{}", list_changes(&pushed, &agreed.notes, name));
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

async fn apply_jars(
    ctx: &mut Ctx,
    config: &Config,
    target: &Target,
    base_tree: &Files,
    head_tree: &Files,
    exact: bool,
) -> anyhow::Result<()> {
    let wanted = jar_entries(head_tree, config, target)?;
    let previous = jar_entries(base_tree, config, target)?;

    let mut downloads = vec![];
    for file in &wanted {
        if let Source::Url { url, sha1 } = &file.source {
            downloads.push((
                url.parse()
                    .with_context(|| format!("{}/{}: bad url", file.dir, file.filename))?,
                sha1.clone(),
            ));
        }
    }
    let mut cached = ctx.files.ensure(&downloads).await?;
    let mut wanted_sha1 = vec![];
    for file in &wanted {
        let sha1 = match &file.source {
            Source::Url { sha1, .. } => sha1.clone(),
            Source::Committed { path, blob } => {
                let (sha1, cached_path) = ctx.committed(blob, path).await?;
                cached.insert(sha1.clone(), cached_path);
                sha1
            }
        };
        wanted_sha1.push(sha1);
    }

    let off_by_default = config
        .client
        .optional_sets
        .iter()
        .filter(|set| !set.enabled_by_default)
        .flat_map(|set| set.mods.iter().cloned())
        .collect::<HashSet<_>>();

    let mut prism_indexes = BTreeMap::new();
    if target.is_prism() {
        for dir in INDEX_DIRS {
            prism_indexes.insert(dir, read_prism_indexes(&target.game_dir, dir)?);
        }
    }

    // To keep a renamed update disabled.
    let previous_by_index = previous
        .iter()
        .filter_map(|file| {
            file.index
                .as_ref()
                .map(|(path, _)| (path.clone(), file.filename.clone()))
        })
        .collect::<HashMap<_, _>>();
    let mut written = HashSet::new();
    let mut kept = HashSet::new();
    for (file, sha1) in wanted.iter().zip(&wanted_sha1) {
        let dir = file.dir;
        let filename = file.filename.as_str();
        kept.insert((dir, filename.to_string()));
        let enabled = target.game_dir.join(dir).join(filename);
        let disabled = target
            .game_dir
            .join(dir)
            .join(format!("{filename}{DISABLED_SUFFIX}"));
        let existing = [&enabled, &disabled]
            .into_iter()
            .find(|p| p.exists())
            .cloned();
        let up_to_date = match &existing {
            Some(existing) => &ctx.hashes.sha1(existing).await? == sha1,
            None => false,
        };
        if !up_to_date {
            let renamed_from_disabled = file
                .index
                .as_ref()
                .and_then(|(path, _)| previous_by_index.get(path))
                .is_some_and(|old| {
                    target
                        .game_dir
                        .join(dir)
                        .join(format!("{old}{DISABLED_SUFFIX}"))
                        .exists()
                });
            let new_disabled = match &existing {
                Some(existing) => existing == &disabled,
                None => {
                    renamed_from_disabled
                        || (target.is_prism()
                            && dir == "mods"
                            && off_by_default.contains(&file.name()))
                }
            };
            let (dest, other) = if new_disabled {
                (&disabled, &enabled)
            } else {
                (&enabled, &disabled)
            };
            std::fs::create_dir_all(dest.parent().expect("has parent"))?;
            std::fs::copy(&cached[sha1], dest)?;
            remove_if_exists(other)?;
        }
        if target.is_prism()
            && let Some((path, entry)) = file.download_index()
        {
            let ours = target.game_dir.join(path);
            if let Some((prism_name, _)) = prism_indexes[dir].get(filename) {
                let prism_path = target.game_dir.join(index_path(dir, prism_name));
                if up_to_date {
                    continue;
                }
                if prism_path != ours {
                    remove_if_exists(&prism_path)?;
                }
            }
            std::fs::create_dir_all(ours.parent().expect("has parent"))?;
            std::fs::write(&ours, entry.to_bytes())?;
            written.insert(ours);
        }
    }

    for file in &previous {
        let dir = file.dir;
        let filename = file.filename.as_str();
        if kept.contains(&(dir, filename.to_string())) {
            continue;
        }
        remove_if_exists(&target.game_dir.join(dir).join(filename))?;
        remove_if_exists(
            &target
                .game_dir
                .join(dir)
                .join(format!("{filename}{DISABLED_SUFFIX}")),
        )?;
        if target.is_prism() {
            let mut stale = vec![];
            if let Some((prism_name, _)) = prism_indexes[dir].get(filename) {
                stale.push(target.game_dir.join(index_path(dir, prism_name)));
            }
            if let Some((path, _)) = &file.index {
                stale.push(target.game_dir.join(path));
            }
            // A renamed update may have just written this file.
            for path in stale.into_iter().filter(|path| !written.contains(path)) {
                remove_if_exists(&path)?;
            }
        }
    }

    if exact {
        for dir in INDEX_DIRS
            .into_iter()
            .filter(|dir| config.handles_index_dir(dir))
        {
            if let Ok(entries) = std::fs::read_dir(target.game_dir.join(dir)) {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let filename = name.strip_suffix(DISABLED_SUFFIX).unwrap_or(&name);
                    if entry.file_type()?.is_file()
                        && is_indexed_binary(dir, &name)
                        && !kept.contains(&(dir, filename.to_string()))
                    {
                        remove_if_exists(&entry.path())?;
                    }
                }
            }
            for (filename, (prism_name, _)) in &prism_indexes[dir] {
                let path = target.game_dir.join(index_path(dir, prism_name));
                if !kept.contains(&(dir, filename.clone())) && !written.contains(&path) {
                    remove_if_exists(&path)?;
                }
            }
        }
    }
    Ok(())
}

pub async fn status(ctx: &mut Ctx) -> anyhow::Result<()> {
    let config = ctx.config()?;
    let local = ctx.local()?;
    let names = Target::names(&local);
    if names.is_empty() {
        println!("no targets, add one with `potato target add <name> <folder>`");
        return Ok(());
    }
    for name in names {
        let target = match Target::resolve(&ctx.repo, &name, &config, &local) {
            Ok(target) => target,
            Err(err) => {
                println!("{name}: {err}");
                continue;
            }
        };
        let state = sync_state(ctx, &config, &target, Mode::Check, false).await?;
        let to_pull = state.changed_there();
        let to_push = diff(&state.snapshot, &state.head_domain);
        if to_pull.is_empty() && to_push.is_empty() {
            println!("{name}: in sync");
            continue;
        }
        println!("{name}:");
        if !to_pull.is_empty() {
            println!(
                "{}",
                if target.is_prism() {
                    " to pull:"
                } else {
                    " changed there:"
                }
            );
            print!("{}", list_changes(&to_pull, &HashMap::new(), &name));
        }
        if !to_push.is_empty() {
            println!(" to push:");
            print!("{}", list_changes(&to_push, &state.agreed.notes, &name));
        }
    }
    ctx.hashes.save()?;
    Ok(())
}
