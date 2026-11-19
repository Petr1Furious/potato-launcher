use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::{
    build::mod_id_of,
    config::SelfHosted,
    config::{Config, INDEX_DIRS, LOCAL_DIR, object_key, set_versions},
    ctx::Ctx,
    git::{Files, Git, SNAPSHOT_PREFIX, as_blob_ref, blob_ref, is_committed_binary},
    index::{
        DISABLED_SUFFIX, INDEX_SUFFIX, IndexFile, Side, current_os, index_dir_of, index_name,
        index_path, is_indexed_binary,
    },
    modrinth,
    pack::{PackFile, Source, pack_files},
    storage,
    target::Target,
    upload::{self, Upload},
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

/// Files of `tree` that `target` holds: managed files plus, for Prism, the
/// index files and committed jars that apply to it. Servers get jars, but
/// they're handled separately.
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

/// Jars and zips `target` should have.
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
    /// Reads everything and prepares uploads; may ask Modrinth.
    Pull,
    /// Only compares; no network.
    Check,
}

pub struct Snapshot {
    pub files: Files,
    pub uploads: Vec<Upload>,
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

/// What the repo would contain for this target's domain, given its current files.
pub async fn read_target(
    ctx: &mut Ctx,
    config: &Config,
    target: &Target,
    base_tree: &Files,
    base_domain: &Files,
    mode: Mode,
) -> anyhow::Result<Snapshot> {
    let mut snapshot = Snapshot {
        files: Files::new(),
        uploads: vec![],
    };
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
                snapshot.files.insert(path.clone(), content);
            }
        }
    }
    Ok(snapshot)
}

/// Ordinary files under the managed paths of a Prism game folder: no index
/// files, and no jars or zips directly in a pack folder.
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
    snapshot: &mut Snapshot,
) -> anyhow::Result<()> {
    let game_dir = &target.game_dir;
    for (rel, file) in prism_managed_files(config, game_dir)? {
        snapshot.files.insert(rel, std::fs::read(&file)?);
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
        let present = binaries
            .iter()
            .map(|(name, _)| {
                name.strip_suffix(DISABLED_SUFFIX)
                    .unwrap_or(name)
                    .to_string()
            })
            .collect::<HashSet<_>>();
        // Entries whose file is gone: a renamed jar may be an update of one of them.
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

            // A committed file stays committed: the same bytes keep the repo's blob.
            if let Some((base_path, blob, content)) = base_committed.get(&filename) {
                let (base_sha1, _) = ctx.committed(blob, base_path).await?;
                let content = if base_sha1 == sha1 {
                    content.clone()
                } else {
                    store_blob(ctx, mode, &rel, &path, &sha1)?
                };
                snapshot.files.insert(base_path.clone(), content);
                if let Some((index_path, _, index_content)) = base_by_filename.get(&filename) {
                    snapshot
                        .files
                        .insert(index_path.clone(), index_content.clone());
                }
                continue;
            }

            if let Some((base_path, base_entry, base_content)) = base_by_filename.get(&filename)
                && base_entry.sha1() == Some(sha1.as_str())
            {
                snapshot
                    .files
                    .insert(base_path.clone(), base_content.clone());
                continue;
            }

            let found = Box::pin(make_entry(
                ctx,
                config,
                prism_indexes.get(&filename),
                &filename,
                &sha1,
                &path,
                mode,
                snapshot,
            ))
            .await?;
            if let Some((base_path, base_entry, _)) = base_by_filename.get(&filename) {
                let mut entry = match found {
                    Found::Index(_, entry) => entry,
                    Found::Committed(index) => {
                        snapshot
                            .files
                            .insert(rel.clone(), store_blob(ctx, mode, &rel, &path, &sha1)?);
                        index.map_or_else(
                            || IndexFile::metadata(file_stem(&filename), &filename),
                            |(_, index)| index,
                        )
                    }
                };
                entry.carry_overrides(base_entry);
                snapshot.files.insert(base_path.clone(), entry.to_bytes());
                continue;
            }

            let (name, entry, write_new) = match found {
                Found::Index(name, entry) => (name, entry, true),
                Found::Committed(index) => {
                    snapshot
                        .files
                        .insert(rel.clone(), store_blob(ctx, mode, &rel, &path, &sha1)?);
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
                snapshot.files.insert(base_path, entry.to_bytes());
                continue;
            }
            if !write_new {
                continue;
            }
            let mut index = index_path(dir, &name);
            if base_tree.contains_key(&index) || snapshot.files.contains_key(&index) {
                index = index_path(dir, &format!("{name}-{}", current_os()));
            }
            snapshot.files.insert(index, entry.to_bytes());
        }
    }
    Ok(())
}

/// Which vanished entry a renamed jar replaces: the only one from the same
/// Modrinth or CurseForge project, or else the only one with the same index name.
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

/// What a jar or zip found in Prism becomes in the repo.
enum Found {
    /// An index file that downloads it.
    Index(String, IndexFile),
    /// The file itself, committed, with Prism's index file (minus its download)
    /// for side, os and project ids.
    Committed(Option<(String, IndexFile)>),
}

/// Like `find_update`, for jars without a shared project or name: the only
/// vanished entry whose jar has the same mod id. Jars it can't read don't match.
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

#[allow(clippy::too_many_arguments)]
async fn make_entry(
    ctx: &Ctx,
    config: &Config,
    prism_index: Option<&(String, IndexFile)>,
    filename: &str,
    sha1: &str,
    path: &Path,
    mode: Mode,
    snapshot: &mut Snapshot,
) -> anyhow::Result<Found> {
    let to_storage = config.self_hosted == SelfHosted::Storage;
    let object_url = config.publish.object_url(sha1).to_string();
    if let Some((name, index)) = prism_index {
        let mut entry = index.clone();
        if entry.url().is_none() {
            if !to_storage {
                entry.strip_download();
                return Ok(Found::Committed(Some((name.clone(), entry))));
            }
            snapshot
                .uploads
                .push(jar_upload(sha1, path, filename, "no download URL"));
        }
        entry.convert(sha1, &object_url);
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
    if !to_storage {
        return Ok(Found::Committed(None));
    }
    snapshot
        .uploads
        .push(jar_upload(sha1, path, filename, "not on Modrinth"));
    Ok(Found::Index(
        name.clone(),
        IndexFile::new(&name, filename, &object_url, sha1),
    ))
}

/// The blob for a Prism file that goes into the repo as a committed file.
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

fn jar_upload(sha1: &str, path: &Path, filename: &str, note: &'static str) -> Upload {
    let dir = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("mods");
    Upload {
        note: Some(note),
        ..Upload::new(
            object_key(sha1),
            path.to_path_buf(),
            format!("{dir}/{filename}"),
        )
    }
}

/// Refuses when uncommitted changes touch files that `action` would leave out.
pub fn ensure_committed(
    git: &Git,
    action: &str,
    relevant: impl Fn(&str) -> bool,
) -> anyhow::Result<()> {
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
    bail!("uncommitted changes to {list} won't be {action}; commit them first or use --force");
}

/// Index files describing the jar the repo already has at the same path keep the
/// repo's bytes, so a mod updated the same way on both sides can't conflict.
fn align_with_head(files: &mut Files, head_tree: &Files) {
    for (path, content) in files.iter_mut() {
        if index_dir_of(path).is_none() {
            continue;
        }
        let Some(head) = head_tree.get(path).filter(|head| *head != content) else {
            continue;
        };
        let (Ok(ours), Ok(theirs)) = (
            IndexFile::parse_bytes(head),
            IndexFile::parse_bytes(content),
        ) else {
            continue;
        };
        if ours.sha1().is_some()
            && ours.sha1() == theirs.sha1()
            && ours.filename() == theirs.filename()
        {
            *content = head.clone();
        }
    }
}

/// The commit the target last matched, if this repo has it.
pub fn effective_base(git: &Git, target: &Target) -> anyhow::Result<Option<String>> {
    Ok(target
        .recorded_base()
        .filter(|commit| git.commit_exists(commit)))
}

fn print_changes(changes: &[Change]) {
    for change in changes {
        println!("  {change}");
    }
}

pub async fn pull(ctx: &mut Ctx, name: &str, force: bool) -> anyhow::Result<()> {
    let config = ctx.config()?;
    let target = Target::resolve(&ctx.repo, name, &config, &ctx.local()?)?;
    if !target.is_prism() {
        bail!(
            "{name} is a server; servers only get pushes, so copy changes made there into the repo"
        );
    }
    let base = if force {
        None
    } else {
        effective_base(&ctx.git, &target)?
    };
    // Without a base, compare against HEAD so unchanged files merge cleanly.
    let base_tree = match &base {
        Some(base) => ctx.git.read_tree(base)?,
        None => Files::new(),
    };
    let reference_tree = match &base {
        Some(_) => base_tree.clone(),
        None => ctx.git.read_tree(&ctx.git.head()?)?,
    };
    let base_domain = domain(&reference_tree, &config, &target)?;
    let mut snapshot = read_target(
        ctx,
        &config,
        &target,
        &reference_tree,
        &base_domain,
        Mode::Pull,
    )
    .await?;
    ctx.hashes.save()?;
    let head_tree = ctx.git.read_tree(&ctx.git.head()?)?;
    align_with_head(&mut snapshot.files, &head_tree);

    if !snapshot.uploads.is_empty() {
        let storage = storage::open(&config.publish.target)?;
        upload::print(&snapshot.uploads, ctx.verbose);
        let files = upload::pairs(&snapshot.uploads);
        storage::locked(storage.as_ref(), storage.put_objects(&files)).await?;
    }

    let mut changes = diff(&base_domain, &snapshot.files);
    if base.is_none() {
        // The snapshot merges as unrelated history, so deletions don't apply.
        changes.retain(|change| !matches!(change, Change::Deleted(_)));
    }
    if changes.is_empty() {
        println!("{name}: nothing to pull");
    } else {
        let mut tree = base_tree.clone();
        if base.is_some() {
            for path in base_domain.keys() {
                tree.remove(path);
            }
        }
        tree.extend(snapshot.files);
        let commit = ctx.git.write_commit(
            base.as_deref(),
            &tree,
            &format!("{SNAPSHOT_PREFIX}: {name}"),
        )?;
        let merged = ctx.git.cherry_pick_no_commit(&commit)?;
        ctx.git.update_ref(&target.ref_name(), &commit)?;
        target.record_base(&commit)?;
        println!("{name}:");
        print_changes(&changes);
        if merged {
            println!("review with `git diff --cached`, then commit");
        } else {
            println!("conflicts, resolve them and commit");
        }
    }

    if let Some((minecraft, loader, loader_version)) = target.mmc_pack()?
        && set_versions(&ctx.repo, &minecraft, loader, loader_version.as_deref())?
    {
        println!("updated versions in potato.toml");
    }
    Ok(())
}

pub async fn push(ctx: &mut Ctx, name: &str, force: bool) -> anyhow::Result<()> {
    let head = ctx.git.head()?;
    let config = ctx.config()?;
    let target = Target::resolve(&ctx.repo, name, &config, &ctx.local()?)?;
    if !force {
        ensure_committed(&ctx.git, "pushed", |path| {
            config.is_pack_file(path) && !target.is_excluded(path)
        })?;
    }
    if !force && target.is_running() {
        bail!("{name} is running, stop it first");
    }
    let base = effective_base(&ctx.git, &target)?;
    if base.is_none() && !force {
        bail!("{name} hasn't been synced with this repo yet, use --force to overwrite it");
    }
    let base_tree = match &base {
        Some(base) => ctx.git.read_tree(base)?,
        None => Files::new(),
    };
    let base_domain = domain(&base_tree, &config, &target)?;
    let head_tree = ctx.git.read_tree(&head)?;
    let head_domain = domain(&head_tree, &config, &target)?;
    if !force && base.is_some() {
        let snapshot =
            read_target(ctx, &config, &target, &base_tree, &base_domain, Mode::Check).await?;
        // Only changes the commit doesn't already have would be lost.
        let mut changes = diff(&base_domain, &snapshot.files);
        changes
            .retain(|change| snapshot.files.get(change.path()) != head_domain.get(change.path()));
        if !changes.is_empty() {
            if target.is_prism() {
                bail!("{name} changed since the last sync, run `potato pull {name}` first");
            }
            let mut list = changes
                .iter()
                .take(5)
                .map(Change::path)
                .collect::<Vec<_>>()
                .join(", ");
            if changes.len() > 5 {
                list.push_str(&format!(" and {} more", changes.len() - 5));
            }
            bail!(
                "{list} changed on {name} since the last push; copy the changes into the repo or use --force"
            );
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
    // A forced push makes a Prism instance match the commit exactly; servers keep
    // their own files, which never enter the repo.
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

    target.record_base(&head)?;
    ctx.git.update_ref(&target.ref_name(), &head)?;
    println!("pushed to {name}");
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

    // Index files a renamed update keeps: its old jar's file name, by index path.
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
        // Prism gets index files only when they can download the file.
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
            // A renamed update just wrote its index file under the same name.
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
        println!("no targets; add one with `potato target add <name> <folder>`");
        return Ok(());
    }
    let head = ctx.git.head()?;
    let head_tree = ctx.git.read_tree(&head)?;
    for name in names {
        let target = match Target::resolve(&ctx.repo, &name, &config, &local) {
            Ok(target) => target,
            Err(err) => {
                println!("{name}: {err}");
                continue;
            }
        };
        let base = effective_base(&ctx.git, &target)?;
        let Some(base) = base else {
            println!("{name}: never synced");
            continue;
        };
        let base_tree = ctx.git.read_tree(&base)?;
        let base_domain = domain(&base_tree, &config, &target)?;
        let snapshot =
            read_target(ctx, &config, &target, &base_tree, &base_domain, Mode::Check).await?;
        let to_pull = diff(&base_domain, &snapshot.files);
        let to_push = diff(&base_domain, &domain(&head_tree, &config, &target)?);
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
            print_changes(&to_pull);
        }
        if !to_push.is_empty() {
            println!(" to push:");
            print_changes(&to_push);
        }
    }
    ctx.hashes.save()?;
    Ok(())
}
