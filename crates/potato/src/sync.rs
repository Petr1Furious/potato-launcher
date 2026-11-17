use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::{
    config::{Config, INDEX_DIRS, LOCAL_DIR, object_key, set_versions},
    ctx::Ctx,
    git::{Files, Git, SNAPSHOT_PREFIX},
    index::{
        DISABLED_SUFFIX, INDEX_SUFFIX, IndexFile, Side, current_os, index_dir_of, index_name,
        index_path, is_indexed_binary,
    },
    modrinth, storage,
    target::Target,
    upload::{self, Upload},
};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    Added(String),
    Modified(String),
    Deleted(String),
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
/// index files that apply to it. Servers hold jars but no index files.
pub fn domain(tree: &Files, config: &Config, target: &Target) -> anyhow::Result<Files> {
    let os = current_os();
    let mut files = Files::new();
    for (path, content) in tree {
        if index_dir_of(path).is_some() {
            if !target.is_prism() {
                continue;
            }
            let entry = IndexFile::parse_bytes(content).with_context(|| path.clone())?;
            if entry.side()?.includes(Side::Client) && entry.applies_to_os(os)? {
                files.insert(path.clone(), content.clone());
            }
        } else if config.is_managed_file(path) && !target.is_excluded(path) {
            files.insert(path.clone(), content.clone());
        }
    }
    Ok(files)
}

/// Index entries whose files `target` should have.
fn jar_entries(tree: &Files, target: &Target) -> anyhow::Result<Vec<(String, IndexFile)>> {
    let os = current_os();
    let mut entries = vec![];
    for (path, content) in tree {
        let Some(dir) = index_dir_of(path) else {
            continue;
        };
        if !target.is_prism() && dir != "mods" {
            continue;
        }
        let entry = IndexFile::parse_bytes(content).with_context(|| path.clone())?;
        if entry.side()?.includes(target.side()) && entry.applies_to_os(os)? {
            entries.push((path.clone(), entry));
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
    add: &[String],
) -> anyhow::Result<Snapshot> {
    let mut snapshot = Snapshot {
        files: Files::new(),
        uploads: vec![],
    };
    if target.is_prism() {
        read_prism(
            ctx,
            config,
            target,
            base_tree,
            base_domain,
            mode,
            &mut snapshot,
        )
        .await?;
    } else {
        for path in base_domain.keys() {
            if let Ok(content) = std::fs::read(target.game_dir.join(path)) {
                snapshot.files.insert(path.clone(), content);
            }
        }
        for path in add {
            let path = path.trim_end_matches('/');
            let full = target.game_dir.join(path);
            let files = if full.is_dir() {
                walk(&full)?
            } else {
                vec![full]
            };
            for file in files {
                let rel = rel_path(&target.game_dir, &file)?;
                if !config.is_managed_file(&rel) {
                    bail!("{rel} isn't under a managed path");
                }
                let content = std::fs::read(&file).with_context(|| format!("can't read {rel}"))?;
                snapshot.files.insert(rel, content);
            }
        }
    }
    Ok(snapshot)
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
            snapshot.files.insert(rel, std::fs::read(&file)?);
        }
    }

    for dir in INDEX_DIRS {
        let prism_indexes = read_prism_indexes(game_dir, dir)?;
        let mut base_by_filename = HashMap::new();
        for (path, content) in base_domain {
            if index_dir_of(path) == Some(dir) {
                let entry = IndexFile::parse_bytes(content)?;
                if let Some(filename) = entry.filename() {
                    base_by_filename
                        .insert(filename.to_string(), (path.clone(), entry, content.clone()));
                }
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
        for (name, path) in binaries {
            let filename = name
                .strip_suffix(DISABLED_SUFFIX)
                .unwrap_or(&name)
                .to_string();
            let sha1 = ctx.hashes.sha1(&path).await?;
            if let Some((base_path, base_entry, base_content)) = base_by_filename.get(&filename) {
                if base_entry.sha1() == Some(sha1.as_str()) {
                    snapshot
                        .files
                        .insert(base_path.clone(), base_content.clone());
                    continue;
                }
                let (_, mut entry) = make_entry(
                    ctx,
                    config,
                    prism_indexes.get(&filename),
                    &filename,
                    &sha1,
                    &path,
                    mode,
                    snapshot,
                )
                .await?;
                entry.carry_overrides(base_entry);
                snapshot.files.insert(base_path.clone(), entry.to_bytes());
                continue;
            }
            let (name, entry) = make_entry(
                ctx,
                config,
                prism_indexes.get(&filename),
                &filename,
                &sha1,
                &path,
                mode,
                snapshot,
            )
            .await?;
            let mut index = index_path(dir, &name);
            if base_tree.contains_key(&index) || snapshot.files.contains_key(&index) {
                index = index_path(dir, &format!("{name}-{}", current_os()));
            }
            snapshot.files.insert(index, entry.to_bytes());
        }
    }
    Ok(())
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
) -> anyhow::Result<(String, IndexFile)> {
    let object_url = config.publish.object_url(sha1).to_string();
    if let Some((name, index)) = prism_index {
        let mut entry = index.clone();
        if entry.url().is_none() {
            snapshot
                .uploads
                .push(jar_upload(sha1, path, filename, "no download URL"));
        }
        entry.convert(sha1, &object_url);
        return Ok((name.clone(), entry));
    }
    let name = file_stem(filename).to_string();
    if mode == Mode::Check {
        return Ok((
            name,
            IndexFile::new(file_stem(filename), filename, "unindexed", sha1),
        ));
    }
    if let Some(found) = modrinth::file_by_sha1(&ctx.client, sha1).await? {
        let mut entry = IndexFile::new(&name, filename, &found.url, sha1);
        entry.set_modrinth(&found.project_id, &found.version_id);
        return Ok((name, entry));
    }
    snapshot
        .uploads
        .push(jar_upload(sha1, path, filename, "not on Modrinth"));
    Ok((
        name.clone(),
        IndexFile::new(&name, filename, &object_url, sha1),
    ))
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

pub async fn pull(ctx: &mut Ctx, name: &str, add: &[String]) -> anyhow::Result<()> {
    ctx.git.ensure_clean()?;
    let config = ctx.config()?;
    let target = Target::resolve(name, &config, &ctx.local()?)?;
    let base = effective_base(&ctx.git, &target)?;
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
    let snapshot = read_target(
        ctx,
        &config,
        &target,
        &reference_tree,
        &base_domain,
        Mode::Pull,
        add,
    )
    .await?;
    ctx.hashes.save()?;

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
    ctx.git.ensure_clean()?;
    let head = ctx.git.head()?;
    let config = ctx.config()?;
    let target = Target::resolve(name, &config, &ctx.local()?)?;
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
    if !force && base.is_some() {
        let snapshot = read_target(
            ctx,
            &config,
            &target,
            &base_tree,
            &base_domain,
            Mode::Check,
            &[],
        )
        .await?;
        if !diff(&base_domain, &snapshot.files).is_empty() {
            bail!("{name} changed since the last sync, run `potato pull {name}` first");
        }
    }

    let head_tree = ctx.git.read_tree(&head)?;
    let head_domain = domain(&head_tree, &config, &target)?;
    for (path, content) in &head_domain {
        if index_dir_of(path).is_some() {
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
        if index_dir_of(path).is_none() && !head_domain.contains_key(path) {
            remove_if_exists(&target.game_dir.join(path))?;
        }
    }
    apply_jars(ctx, &config, &target, &base_tree, &head_tree).await?;
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
) -> anyhow::Result<()> {
    let wanted = jar_entries(head_tree, target)?;
    let previous = jar_entries(base_tree, target)?;

    let mut downloads = vec![];
    for (path, entry) in &wanted {
        let sha1 = entry
            .sha1()
            .with_context(|| format!("{path} has no sha1, pull from prism again"))?;
        let url = entry.url().with_context(|| format!("{path} has no url"))?;
        downloads.push((
            url.parse().with_context(|| format!("{path}: bad url"))?,
            sha1.to_string(),
        ));
    }
    let cached = ctx.files.ensure(&downloads).await?;

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

    let mut kept = HashSet::new();
    for (path, entry) in &wanted {
        let dir = index_dir_of(path).expect("jar entries are index files");
        let filename = entry
            .filename()
            .with_context(|| format!("{path} has no filename"))?;
        kept.insert((dir, filename.to_string()));
        let sha1 = entry.sha1().expect("checked above");
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
            Some(existing) => ctx.hashes.sha1(existing).await? == sha1,
            None => false,
        };
        if !up_to_date {
            let new_disabled = match &existing {
                Some(existing) => existing == &disabled,
                None => {
                    target.is_prism() && dir == "mods" && off_by_default.contains(index_name(path))
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
        if target.is_prism() {
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
        }
    }

    for (path, entry) in &previous {
        let dir = index_dir_of(path).expect("jar entries are index files");
        let Some(filename) = entry.filename() else {
            continue;
        };
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
            if let Some((prism_name, _)) = prism_indexes[dir].get(filename) {
                remove_if_exists(&target.game_dir.join(index_path(dir, prism_name)))?;
            }
            remove_if_exists(&target.game_dir.join(path))?;
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
        let target = match Target::resolve(&name, &config, &local) {
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
        let snapshot = read_target(
            ctx,
            &config,
            &target,
            &base_tree,
            &base_domain,
            Mode::Check,
            &[],
        )
        .await?;
        let to_pull = diff(&base_domain, &snapshot.files);
        let to_push = diff(&base_domain, &domain(&head_tree, &config, &target)?);
        if to_pull.is_empty() && to_push.is_empty() {
            println!("{name}: in sync");
            continue;
        }
        println!("{name}:");
        if !to_pull.is_empty() {
            println!(" to pull:");
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
