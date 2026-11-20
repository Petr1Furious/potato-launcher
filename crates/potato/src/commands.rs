use std::{
    io::Read as _,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::{
    build::{mod_id_of, modrinth_mod_id, read_mod_id},
    config::{CONFIG_FILE, INDEX_DIRS, LocalConfig, TargetType, set_local_target, set_versions},
    ctx::Ctx,
    git::Git,
    index::{INDEX_SUFFIX, IndexFile, Side, index_dir_of, index_name, index_path, normalize_os},
    modrinth,
    target::{PRISM, check_folder, parse_mmc_pack, pulled_file, synced_file},
};

const TEMPLATE: &str = r#"id = "{id}"
minecraft = "1.21.1"
loader = "vanilla"

[publish]
base_url = "https://example.com/data"
target = "rsync://user@example.com/srv/potato/site/data"

[client]
mod_sync = "delta"

[[paths]]
path = "config"
type = "directory"
skip_if_dir_exists = true
"#;

pub fn init(repo: &Path, id: Option<String>, prism: Option<PathBuf>) -> anyhow::Result<()> {
    std::fs::create_dir_all(repo)?;
    let config_path = repo.join(CONFIG_FILE);
    if config_path.exists() {
        bail!("{CONFIG_FILE} already exists");
    }
    let prism = prism
        .map(|prism| check_folder(TargetType::Prism, &prism))
        .transpose()?;
    let repo = repo.canonicalize()?;
    let id = match id {
        Some(id) => id,
        None => repo
            .file_name()
            .and_then(|n| n.to_str())
            .context("can't guess an id, pass --id")?
            .to_lowercase(),
    };
    utils::instance_id::validate_instance_id(&id)
        .map_err(|err| anyhow::anyhow!("invalid id '{id}': {err}, pass --id"))?;
    let git = Git::new(&repo);
    if !git.is_repo() {
        git.init()?;
    }
    std::fs::write(&config_path, TEMPLATE.replace("{id}", &id))?;
    let gitignore = repo.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    if !existing.lines().any(|line| line.trim() == ".potato/") {
        std::fs::write(&gitignore, format!("{existing}.potato/\n"))?;
    }
    if let Some(prism) = prism {
        set_local_target(&repo, PRISM, Some((TargetType::Prism, &prism)))?;
        if let Ok(content) = std::fs::read(prism.join("mmc-pack.json")) {
            let (minecraft, loader, loader_version) = parse_mmc_pack(&content)?;
            set_versions(&repo, &minecraft, loader, loader_version.as_deref())?;
        }
    }
    println!("created {CONFIG_FILE}, set [publish] and run `potato pull prism`");
    Ok(())
}

pub fn target_add(
    repo: &Path,
    name: &str,
    kind: Option<TargetType>,
    path: &Path,
) -> anyhow::Result<()> {
    if !repo.join(CONFIG_FILE).exists() {
        bail!("no {CONFIG_FILE} here, run `potato init` first");
    }
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("target names use letters, digits, - and _");
    }
    let kind = kind.unwrap_or(if path.join("mmc-pack.json").exists() {
        TargetType::Prism
    } else {
        TargetType::Server
    });
    let path = check_folder(kind, path)?;
    let unchanged = LocalConfig::load(repo)?
        .targets
        .get(name)
        .is_some_and(|old| old.kind == kind && old.path == path);
    if !unchanged {
        forget_sync(repo, name)?;
    }
    set_local_target(repo, name, Some((kind, &path)))?;
    println!("{name} ({}): {}", kind.as_str(), path.display());
    Ok(())
}

pub fn target_list(repo: &Path) -> anyhow::Result<()> {
    let local = LocalConfig::load(repo)?;
    if local.targets.is_empty() {
        println!("no targets, add one with `potato target add <name> <folder>`");
        return Ok(());
    }
    let rows = local
        .targets
        .iter()
        .map(|(name, target)| {
            let state = if synced_file(repo, name).exists() {
                "synced"
            } else {
                "never synced"
            };
            (name, target.kind.as_str(), state, target.path.display())
        })
        .collect::<Vec<_>>();
    let name_width = rows.iter().map(|row| row.0.len()).max().unwrap_or(0);
    let state_width = rows.iter().map(|row| row.2.len()).max().unwrap_or(0);
    for (name, kind, state, path) in rows {
        println!("{name:name_width$}  {kind:6}  {state:state_width$}  {path}");
    }
    Ok(())
}

fn forget_sync(repo: &Path, name: &str) -> anyhow::Result<()> {
    for file in [synced_file(repo, name), pulled_file(repo, name)] {
        match std::fs::remove_file(file) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
            _ => {}
        }
    }
    let git = Git::new(repo);
    if git.is_repo() {
        git.delete_refs(&format!("refs/potato/pulled/{name}/"))?;
    }
    Ok(())
}

pub fn target_remove(repo: &Path, name: &str) -> anyhow::Result<()> {
    if !LocalConfig::load(repo)?.targets.contains_key(name) {
        bail!("unknown target '{name}'");
    }
    forget_sync(repo, name)?;
    set_local_target(repo, name, None)
}

pub struct AddOptions {
    pub os: Vec<String>,
    pub side: Option<String>,
    pub dir: String,
    pub name: Option<String>,
}

pub async fn add(ctx: &mut Ctx, source: &str, options: AddOptions) -> anyhow::Result<()> {
    let config = ctx.config()?;
    if !INDEX_DIRS.contains(&options.dir.as_str()) {
        bail!("--dir must be one of {}", INDEX_DIRS.join(", "));
    }
    if !config.handles_index_dir(&options.dir) {
        bail!(
            "{} isn't synced, add a [[paths]] entry for it first",
            options.dir
        );
    }
    let os = options
        .os
        .iter()
        .map(|os| normalize_os(os))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut side = options.side.as_deref().map(Side::parse).transpose()?;

    let local = Path::new(source);
    let (sha1, path, source_url, filename, mut found, default_name) =
        if let Some(spec) = source.strip_prefix("modrinth:") {
            let (slug, pin) = match spec.split_once('@') {
                Some((slug, pin)) => (slug, Some(pin)),
                None => (spec, None),
            };
            let filter = modrinth::Filter {
                game_version: Some(&config.minecraft),
                loader: modrinth::Filter::loader_for(&options.dir, config.loader),
            };
            let file = modrinth::pick_file(&ctx.client, slug, &filter, pin).await?;
            let path = ctx
                .files
                .ensure(&[(file.url.parse()?, file.sha1.clone())])
                .await?
                .remove(&file.sha1)
                .expect("ensured");
            let filename = file.filename.clone();
            (
                file.sha1.clone(),
                path,
                None,
                filename,
                Some(file),
                slug.to_string(),
            )
        } else if local.is_file() {
            let sha1 = utils::files::hash_file(local).await?;
            let path = ctx.files.insert(local, &sha1).await?;
            let filename = local
                .file_name()
                .and_then(|n| n.to_str())
                .context("bad file name")?;
            (
                sha1,
                path,
                None,
                filename.to_string(),
                None,
                stem_of(filename),
            )
        } else {
            let url: url::Url = source.parse().with_context(|| {
                format!("{source} is neither a file, a URL nor modrinth:<slug>")
            })?;
            if let Some(suggestion) = modrinth_page(&url) {
                bail!("{source} is a web page, use modrinth:{suggestion}");
            }
            let (sha1, path) = ctx.files.fetch_unknown(&ctx.client, &url).await?;
            let filename = url
                .path_segments()
                .and_then(|mut s| s.next_back().map(str::to_string))
                .filter(|n| !n.is_empty())
                .context("can't tell the file name from the URL")?;
            let filename = percent_encoding::percent_decode_str(&filename)
                .decode_utf8_lossy()
                .into_owned();
            let name = stem_of(&filename);
            (sha1, path, Some(url), filename, None, name)
        };
    check_archive(&options.dir, &path)
        .await
        .with_context(|| format!("can't add {source}"))?;
    if found.is_none() {
        found = modrinth::file_by_sha1(&ctx.client, &sha1).await?;
    }
    if side.is_none()
        && let Some(found) = &found
    {
        side = Some(
            modrinth::project(&ctx.client, &found.project_id)
                .await?
                .side(),
        );
    }

    let mut entry = match &found {
        Some(found) => {
            let mut entry = IndexFile::new(&default_name, &filename, &found.url, &sha1);
            entry.set_modrinth(&found.project_id, &found.version_id);
            entry
        }
        None => match source_url {
            Some(url) => IndexFile::new(&default_name, &filename, url.as_str(), &sha1),
            None => {
                let rel = format!("{}/{filename}", options.dir);
                let dest = ctx.repo.join(&rel);
                if dest.exists() {
                    bail!("{rel} already exists");
                }
                std::fs::create_dir_all(dest.parent().expect("has parent"))?;
                std::fs::copy(&path, &dest)?;
                println!("added {rel}");
                if side.is_none() && os.is_empty() {
                    return Ok(());
                }
                IndexFile::metadata(&default_name, &filename)
            }
        },
    };
    if let Some(side) = side {
        entry.set_side(side);
    }
    entry.set_os(&os);

    let mut name = options.name.unwrap_or(default_name);
    let mut rel = index_path(&options.dir, &name);
    if ctx.repo.join(&rel).exists() && !os.is_empty() {
        name = format!("{name}-{}", os.join("-"));
        rel = index_path(&options.dir, &name);
    }
    if ctx.repo.join(&rel).exists() {
        bail!("{rel} already exists, pass --name");
    }
    let full = ctx.repo.join(&rel);
    std::fs::create_dir_all(full.parent().expect("has parent"))?;
    std::fs::write(&full, entry.to_bytes())?;
    println!("added {rel}");

    if options.dir == "mods"
        && let Some(found) = &found
    {
        let loader = modrinth::Filter::loader_for("mods", config.loader);
        match missing_dependencies(ctx, &found.required, loader).await {
            Ok(missing) => {
                for slug in missing {
                    println!("missing dependency {slug}: potato mod add modrinth:{slug}");
                }
            }
            Err(err) => eprintln!("can't check dependencies: {err:#}"),
        }
    }
    Ok(())
}

fn stem_of(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map_or(filename, |(stem, _)| stem)
        .to_string()
}

/// `modrinth.com/mod/voxy/version/0.2.20` -> `voxy@0.2.20`
fn modrinth_page(url: &url::Url) -> Option<String> {
    if url.host_str() != Some("modrinth.com") {
        return None;
    }
    let mut segments = url.path_segments()?.skip(1);
    let slug = segments.next().filter(|slug| !slug.is_empty())?;
    match (segments.next(), segments.next()) {
        (Some("version"), Some(version)) => Some(format!("{slug}@{version}")),
        _ => Some(slug.to_string()),
    }
}

async fn check_archive(dir: &str, path: &Path) -> anyhow::Result<()> {
    if dir == "mods" {
        read_mod_id(path).await?;
        return Ok(());
    }
    let mut magic = [0; 4];
    let read = std::fs::File::open(path)?.read_exact(&mut magic);
    if read.is_err() || magic != *b"PK\x03\x04" {
        bail!("not a zip archive");
    }
    Ok(())
}

/// Slugs of required projects no mod in the repo belongs to.
async fn missing_dependencies(
    ctx: &Ctx,
    required: &[String],
    loader: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    if required.is_empty() {
        return Ok(vec![]);
    }
    let mut sha1s = vec![];
    if let Ok(items) = std::fs::read_dir(ctx.repo.join("mods/.index")) {
        for item in items {
            let path = item?.path();
            if !path.to_string_lossy().ends_with(INDEX_SUFFIX) {
                continue;
            }
            let entry = IndexFile::parse(&std::fs::read_to_string(&path)?)
                .with_context(|| path.display().to_string())?;
            sha1s.extend(entry.sha1().map(str::to_string));
        }
    }
    if let Ok(items) = std::fs::read_dir(ctx.repo.join("mods")) {
        for item in items {
            let path = item?.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "jar") {
                sha1s.push(utils::files::hash_file(&path).await?);
            }
        }
    }
    let present = modrinth::projects_of_files(&ctx.client, &sha1s).await?;
    let missing = required
        .iter()
        .filter(|id| !present.contains(*id))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(vec![]);
    }
    Ok(modrinth::projects(&ctx.client, &missing)
        .await?
        .into_iter()
        // A version shared by several loaders lists the dependencies of all of them.
        .filter(|project| loader.is_none_or(|loader| project.loaders.iter().any(|l| l == loader)))
        .map(|project| project.slug)
        .collect())
}

pub async fn mods(ctx: &mut Ctx) -> anyhow::Result<()> {
    let config = ctx.config()?;
    let mut rows = vec![(
        "NAME".to_string(),
        "MOD ID".to_string(),
        "SIDE".to_string(),
        "OS".to_string(),
    )];
    let mut wanted = vec![];
    let mut entries = vec![];
    for dir in INDEX_DIRS
        .into_iter()
        .filter(|dir| config.handles_index_dir(dir))
    {
        let Ok(dir_entries) = std::fs::read_dir(ctx.repo.join(dir).join(".index")) else {
            continue;
        };
        for item in dir_entries {
            let path = item?.path();
            let rel = format!(
                "{dir}/.index/{}",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
            );
            if index_dir_of(&rel).is_none() {
                continue;
            }
            let entry =
                IndexFile::parse(&std::fs::read_to_string(&path)?).with_context(|| rel.clone())?;
            if dir == "mods"
                && let (Some(url), Some(sha1)) = (entry.url(), entry.sha1())
            {
                wanted.push((url.parse()?, sha1.to_string()));
            }
            entries.push((rel, entry));
        }
    }
    // Committed jars without an index file.
    for dir in INDEX_DIRS
        .into_iter()
        .filter(|dir| config.handles_index_dir(dir))
    {
        let Ok(dir_entries) = std::fs::read_dir(ctx.repo.join(dir)) else {
            continue;
        };
        for item in dir_entries {
            let filename = item?.file_name().to_string_lossy().into_owned();
            let described = entries.iter().any(|(rel, entry): &(String, IndexFile)| {
                rel.starts_with(&format!("{dir}/")) && entry.filename() == Some(filename.as_str())
            });
            if described || !crate::git::is_committed_binary(&format!("{dir}/{filename}")) {
                continue;
            }
            let stem = filename
                .rsplit_once('.')
                .map_or(filename.as_str(), |(stem, _)| stem);
            entries.push((index_path(dir, stem), IndexFile::metadata(stem, &filename)));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let cached = ctx.files.ensure(&wanted).await?;
    for (rel, entry) in entries {
        let dir = rel.split('/').next().unwrap_or_default();
        let jar = match entry.sha1().and_then(|sha1| cached.get(sha1)) {
            Some(path) => Some(path.clone()),
            None => entry
                .filename()
                .map(|filename| ctx.repo.join(dir).join(filename))
                .filter(|path| path.is_file()),
        };
        let mod_id = match jar {
            Some(path) if dir == "mods" => mod_id_of(&path).await.unwrap_or_else(|_| "?".into()),
            _ => "-".into(),
        };
        let os = entry.os()?.join(",");
        let name = if rel.starts_with("mods/") {
            index_name(&rel).to_string()
        } else {
            format!(
                "{}/{}",
                rel.split('/').next().unwrap_or_default(),
                index_name(&rel)
            )
        };
        rows.push((
            name,
            mod_id,
            entry.side()?.as_str().to_string(),
            if os.is_empty() { "all".into() } else { os },
        ));
    }
    let widths = rows.iter().fold([0; 3], |w, r| {
        [
            w[0].max(r.0.len()),
            w[1].max(r.1.len()),
            w[2].max(r.2.len()),
        ]
    });
    for (name, id, side, os) in rows {
        println!(
            "{name:w0$}  {id:w1$}  {side:w2$}  {os}",
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2]
        );
    }
    Ok(())
}

pub async fn mod_id(ctx: &mut Ctx, source: &str) -> anyhow::Result<()> {
    let id = match source.strip_prefix("modrinth:") {
        Some(slug) => modrinth_mod_id(ctx, ctx.config()?.loader, slug).await?,
        None => mod_id_of(Path::new(source)).await?,
    };
    println!("{id}");
    Ok(())
}

pub fn mod_set(ctx: &Ctx, name: &str, os: &[String], side: Option<&str>) -> anyhow::Result<()> {
    if os.is_empty() && side.is_none() {
        bail!("pass --os or --side");
    }
    let (dir, name) = name.split_once('/').unwrap_or(("mods", name));
    let mut rel = index_path(dir, name);
    let mut full = ctx.repo.join(&rel);
    let mut entry = match std::fs::read_to_string(&full) {
        Ok(content) => IndexFile::parse(&content).with_context(|| rel.clone())?,
        Err(_) => {
            let filename = committed_file(ctx, dir, name, &rel)?;
            let stem = stem_of(&filename);
            rel = index_path(dir, &stem);
            full = ctx.repo.join(&rel);
            IndexFile::metadata(&stem, &filename)
        }
    };
    if !os.is_empty() {
        if dir != "mods" {
            bail!("os is only supported for mods");
        }
        if os == ["all"] {
            entry.set_os(&[]);
        } else {
            let os = os
                .iter()
                .map(|os| normalize_os(os))
                .collect::<anyhow::Result<Vec<_>>>()?;
            entry.set_os(&os);
        }
    }
    if let Some(side) = side {
        entry.set_side(Side::parse(side)?);
    }
    std::fs::create_dir_all(full.parent().expect("has parent"))?;
    std::fs::write(&full, entry.to_bytes())?;
    let os = entry.os()?;
    println!(
        "{rel}: side {}, os {}",
        entry.side()?.as_str(),
        if os.is_empty() {
            "all".to_string()
        } else {
            os.join(", ")
        }
    );
    Ok(())
}

fn committed_file(ctx: &Ctx, dir: &str, name: &str, rel: &str) -> anyhow::Result<String> {
    [
        name.to_string(),
        format!("{name}.jar"),
        format!("{name}.zip"),
    ]
    .into_iter()
    .find(|filename| ctx.repo.join(dir).join(filename).is_file())
    .with_context(|| format!("no {rel} or committed {dir}/{name}, see `potato mod list`"))
}

pub fn mod_remove(ctx: &Ctx, name: &str) -> anyhow::Result<()> {
    let (dir, name) = name.split_once('/').unwrap_or(("mods", name));
    let rel = index_path(dir, name);
    let full = ctx.repo.join(&rel);
    let committed = match std::fs::read_to_string(&full) {
        Ok(content) => {
            let entry = IndexFile::parse(&content).with_context(|| rel.clone())?;
            std::fs::remove_file(&full)?;
            println!("removed {rel}");
            entry
                .url()
                .is_none()
                .then(|| entry.filename().map(str::to_string))
                .flatten()
        }
        Err(_) => Some(committed_file(ctx, dir, name, &rel)?),
    };
    if let Some(filename) = committed {
        let path = ctx.repo.join(dir).join(&filename);
        if path.is_file() {
            std::fs::remove_file(path)?;
            println!("removed {dir}/{filename}");
        }
    }
    if dir == "mods" {
        let client = ctx.config()?.client;
        let mut lists = vec![("required", &client.required)];
        lists.extend(
            client
                .optional_sets
                .iter()
                .map(|set| (set.id.as_str(), &set.mods)),
        );
        for (list, mods) in lists {
            if mods.iter().any(|m| m == name) {
                println!("{CONFIG_FILE} still lists {name} in {list}");
            }
        }
    }
    Ok(())
}

pub fn cache_clean(ctx: &Ctx) -> anyhow::Result<()> {
    if ctx.cache_dir.exists() {
        std::fs::remove_dir_all(&ctx.cache_dir)?;
    }
    println!("removed {}", ctx.cache_dir.display());
    Ok(())
}
