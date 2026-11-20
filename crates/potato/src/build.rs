use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use generate::instance::{InstanceGenerator, InstanceSpec, RemoteConfig};
use instance::{
    authlib::mirror_authlib_injector_library,
    instance_metadata::{InstanceMetadata, ModEntry, Object},
    mod_sync::{ModSyncSettings, OptionalModSet},
    version_metadata::OsArch,
};
use log::info;
use relative_path::RelativePathBuf;
use url::Url;
use utils::{
    adaptive_download,
    files::{self},
    paths::{BaseUrl, DataDir, InstancesDir},
};

use crate::{
    config::{CONFIG_FILE, Config, LoaderName, object_key},
    ctx::Ctx,
    index::Side,
    java_mirror::mirror_java_runtimes,
    modrinth,
    pack::{PackFile, Source, pack_files},
    progress::TerminalProgress,
    upload::Upload,
};

pub struct BuildOutput {
    pub config: Config,
    pub metadata: InstanceMetadata,
    pub uploads: BTreeMap<String, Upload>,
    /// Object keys the metadata points at.
    pub referenced: BTreeSet<String>,
}

pub async fn read_mod_id(path: &Path) -> anyhow::Result<Option<String>> {
    utils::mod_id::extract_mod_id(path)
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))
}

pub async fn mod_id_of(path: &Path) -> anyhow::Result<String> {
    let context = || format!("can't read the mod id of {}", path.display());
    read_mod_id(path)
        .await
        .with_context(context)?
        .with_context(context)
}

pub async fn modrinth_mod_id(ctx: &Ctx, loader: LoaderName, slug: &str) -> anyhow::Result<String> {
    let cache_path = ctx.cache_dir.join("modrinth-mod-ids.json");
    let mut cache: HashMap<String, String> = std::fs::read(&cache_path)
        .ok()
        .and_then(|content| serde_json::from_slice(&content).ok())
        .unwrap_or_default();
    let key = format!("{slug}/{}", loader.as_str());
    if let Some(id) = cache.get(&key) {
        return Ok(id.clone());
    }
    let filter = modrinth::Filter {
        game_version: None,
        loader: modrinth::Filter::loader_for("mods", loader),
    };
    let file = modrinth::pick_file(&ctx.client, slug, &filter, None).await?;
    let path = ctx
        .files
        .ensure(&[(file.url.parse()?, file.sha1.clone())])
        .await?
        .remove(&file.sha1)
        .expect("ensured");
    let id = mod_id_of(&path).await?;
    cache.insert(key, id.clone());
    std::fs::create_dir_all(&ctx.cache_dir)?;
    std::fs::write(&cache_path, serde_json::to_vec(&cache)?)?;
    Ok(id)
}

struct Entry {
    file: PackFile,
    sha1: String,
    url: Url,
    cached: PathBuf,
}

async fn collect_entries(
    ctx: &mut Ctx,
    tree: &crate::git::Files,
    config: &Config,
) -> anyhow::Result<Vec<Entry>> {
    let files = pack_files(tree, config)?
        .into_iter()
        .filter_map(|file| match file.side() {
            Ok(side) => side.includes(Side::Client).then_some(Ok(file)),
            Err(err) => Some(Err(err)),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut downloads = vec![];
    for file in &files {
        if let Source::Url { url, sha1 } = &file.source {
            let url: Url = url
                .parse()
                .with_context(|| format!("{}/{}: bad url", file.dir, file.filename))?;
            downloads.push((url, sha1.clone()));
        }
    }
    let cached = ctx.files.ensure(&downloads).await?;
    let mut entries = vec![];
    for file in files {
        let (sha1, url, cached_path) = match &file.source {
            Source::Url { url, sha1 } => (sha1.clone(), url.parse()?, cached[sha1].clone()),
            Source::Committed { path, blob } => {
                let (sha1, cached_path) = ctx.committed(blob, path).await?;
                let url = config.publish.object_url(&sha1);
                (sha1, url, cached_path)
            }
        };
        entries.push(Entry {
            file,
            sha1,
            url,
            cached: cached_path,
        });
    }
    Ok(entries)
}

fn map_names(
    names: &[String],
    ids: &HashMap<String, String>,
    what: &str,
) -> anyhow::Result<Vec<String>> {
    let mut mapped = vec![];
    for name in names {
        let id = ids
            .get(name)
            .with_context(|| format!("unknown mod '{name}' in {what}"))?;
        if !mapped.contains(id) {
            mapped.push(id.clone());
        }
    }
    Ok(mapped)
}

fn visit_strings(value: &serde_json::Value, visit: &mut impl FnMut(&str)) {
    match value {
        serde_json::Value::String(s) => visit(s),
        serde_json::Value::Array(items) => items.iter().for_each(|v| visit_strings(v, visit)),
        serde_json::Value::Object(map) => map.values().for_each(|v| visit_strings(v, visit)),
        _ => {}
    }
}

fn replace_strings(value: &mut serde_json::Value, mapping: &HashMap<String, String>) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(new) = mapping.get(s.as_str()) {
                *s = new.clone();
            }
        }
        serde_json::Value::Array(items) => {
            items.iter_mut().for_each(|v| replace_strings(v, mapping))
        }
        serde_json::Value::Object(map) => {
            map.values_mut().for_each(|v| replace_strings(v, mapping))
        }
        _ => {}
    }
}

fn staged_upload(key: String, path: PathBuf, rel: &str, id: &str) -> Upload {
    let pack_prefix = format!("instances/{id}/minecraft/");
    if let Some(pack_path) = rel.strip_prefix(&pack_prefix) {
        return Upload::new(key, path, pack_path.to_string());
    }
    let group = match rel.split('/').next().unwrap_or(rel) {
        "java_mirror" => "java",
        "versions" | "versions_replaced" => "game jars",
        other => other,
    };
    Upload {
        group: Some(group.to_string()),
        ..Upload::new(key, path, rel.to_string())
    }
}

fn decode(rel: &str) -> String {
    percent_encoding::percent_decode_str(rel)
        .decode_utf8_lossy()
        .into_owned()
}

pub async fn build(ctx: &mut Ctx, commit: &str) -> anyhow::Result<BuildOutput> {
    let tree = ctx.git.read_tree(commit)?;
    let config_content = tree
        .get(CONFIG_FILE)
        .context("potato.toml isn't committed")?;
    let config = Config::parse(std::str::from_utf8(config_content)?)?;
    let base = config.publish.base();
    let mirror = config.publish.mirror;

    let entries = collect_entries(ctx, &tree, &config).await?;

    let staging = ctx.cache_dir.join("staging").join(&config.id);
    let source = staging.join("source");
    if source.exists() {
        std::fs::remove_dir_all(&source)?;
    }
    std::fs::create_dir_all(&source)?;
    for (path, content) in &tree {
        if config.is_managed_file(path) {
            let full = source.join(path);
            std::fs::create_dir_all(full.parent().expect("has parent"))?;
            std::fs::write(full, content)?;
        }
    }

    let mut uploads = BTreeMap::new();
    let mut upstream: HashMap<String, Url> = HashMap::new();
    let mut mod_entries = vec![];
    let mut mod_ids = HashMap::new();
    for entry in &entries {
        let dir = entry.file.dir;
        let filename = entry.file.filename.as_str();
        let cached_path = &entry.cached;
        if let Source::Committed { .. } = entry.file.source {
            let key = object_key(&entry.sha1);
            let label = format!("{dir}/{filename}");
            uploads.insert(key.clone(), Upload::new(key, cached_path.clone(), label));
        }
        if dir == "mods" {
            let mod_id = mod_id_of(cached_path).await.with_context(|| {
                entry
                    .file
                    .index
                    .as_ref()
                    .map_or_else(|| format!("{dir}/{filename}"), |(path, _)| path.clone())
            })?;
            mod_ids.insert(entry.file.name(), mod_id.clone());
            let url = if mirror {
                let key = object_key(&entry.sha1);
                let label = format!("{dir}/{filename}");
                uploads.insert(key.clone(), Upload::new(key, cached_path.clone(), label));
                config.publish.object_url(&entry.sha1)
            } else {
                entry.url.clone()
            };
            let os = entry
                .file
                .os()?
                .into_iter()
                .map(|os| if os == "macos" { "osx" } else { os }.to_string())
                .collect();
            mod_entries.push(ModEntry {
                mod_id,
                object: Object {
                    path: RelativePathBuf::from(format!("mods/{filename}")),
                    sha1: entry.sha1.clone(),
                    size: std::fs::metadata(cached_path)?.len(),
                    url,
                },
                os,
            });
        } else {
            if !entry.file.os()?.is_empty() {
                bail!("{dir}/{filename}: os is only supported for mods");
            }
            let rel = format!("{dir}/{filename}");
            let full = source.join(&rel);
            std::fs::create_dir_all(full.parent().expect("has parent"))?;
            std::fs::copy(cached_path, full)?;
            upstream.insert(entry.sha1.clone(), entry.url.clone());
        }
    }

    let mut blocked = vec![];
    for name in &config.client.blocked {
        let id = match name.strip_prefix("modrinth:") {
            Some(slug) => modrinth_mod_id(ctx, config.loader, slug).await?,
            None => name.clone(),
        };
        if !blocked.contains(&id) {
            blocked.push(id);
        }
    }
    let mod_sync = ModSyncSettings {
        mode: config.client.mod_sync,
        required: map_names(&config.client.required, &mod_ids, "required")?,
        blocked,
        optional_sets: config
            .client
            .optional_sets
            .iter()
            .map(|set| {
                Ok(OptionalModSet {
                    id: set.id.clone(),
                    display_name: set.display_name.clone(),
                    enabled_by_default: set.enabled_by_default,
                    mod_ids: map_names(&set.mods, &mod_ids, &format!("optional set '{}'", set.id))?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
    };

    let spec = InstanceSpec {
        id: config.id.clone(),
        display_name: config.display_name.clone(),
        minecraft_version: config.minecraft.clone(),
        mod_loader: config.loader.to_generate(),
        loader_version: config.loader_version.clone(),
        source_root: Some(source.clone()),
        content_rules: config.paths.clone(),
        mod_sync,
        os_specific_mods: Default::default(),
        resource_sync: config.client.resource_sync,
        auth_backend: config.auth_backend.clone(),
        default_xmx: config.default_xmx.clone(),
    };

    let out = staging.join("out");
    let data_dir = DataDir::new(out.clone());
    let base_url = BaseUrl::new(base.clone());
    let instance_dir = InstancesDir::root()
        .instance_dir(&config.id)
        .with_data_dir(data_dir.clone());
    instance_dir.ensure_dir();
    let work_dir = ctx.cache_dir.join("work");
    std::fs::create_dir_all(&work_dir)?;

    info!("Generating {}", config.id);
    let result = Box::pin(
        InstanceGenerator {
            client: ctx.client.clone(),
            spec,
            remote_config: Some(RemoteConfig {
                download_server_base: base_url.clone(),
                replace_download_urls: mirror,
            }),
        }
        .generate_with_mod_entries(
            &instance_dir,
            &work_dir,
            &OsArch::All,
            Some(mod_entries),
        ),
    )
    .await?;

    let mut metadata = result.metadata;
    let check_tasks = files::dedup_check_tasks(result.check_tasks);
    let copy_tasks = files::dedup_copy_tasks(result.copy_tasks);
    let mut keep = result
        .other_generated_files
        .into_iter()
        .collect::<HashSet<_>>();
    keep.extend(check_tasks.iter().map(|task| task.path.clone()));
    keep.extend(copy_tasks.iter().map(|task| task.target.clone()));

    let download_tasks = files::get_download_tasks(
        check_tasks,
        TerminalProgress::new().handle("Checking files"),
    )
    .await?;
    adaptive_download::download_files(
        download_tasks,
        &data_dir.tmp_dir(),
        TerminalProgress::new().handle("Downloading files"),
    )
    .await?;
    files::copy_files_if_different(copy_tasks, TerminalProgress::new().handle("Copying files"))
        .await?;

    if mirror {
        metadata.authlib_injector = mirror_authlib_injector_library(&data_dir, &base_url).await?;
        let java_versions = HashSet::from([metadata.get_java_version()]);
        let mirrored =
            mirror_java_runtimes(&ctx.client, &data_dir, &base_url, &java_versions).await?;
        keep.extend(mirrored.archive_paths);
        metadata.java_runtimes = mirrored
            .runtimes_by_version
            .get(&metadata.get_java_version())
            .cloned()
            .unwrap_or_default();
    }
    files::retain_only_files_and_parents(data_dir.as_path(), &keep).await?;

    let base_str = base.as_str().to_string();
    let objects_prefix = format!("{base_str}objects/");
    let mut value = serde_json::to_value(&metadata)?;
    let mut candidates = BTreeSet::new();
    visit_strings(&value, &mut |s| {
        if let Some(rest) = s.strip_prefix(&base_str)
            && !rest.is_empty()
            && !s.starts_with(&objects_prefix)
        {
            candidates.insert(s.to_string());
        }
    });
    let mut mapping = HashMap::new();
    for candidate in candidates {
        let rel = decode(&candidate[base_str.len()..]);
        let path = out.join(&rel);
        if !path.is_file() {
            continue;
        }
        let sha1 = ctx.hashes.sha1(&path).await?;
        let url = match upstream.get(&sha1) {
            Some(url) if !mirror => url.to_string(),
            _ => {
                let key = object_key(&sha1);
                uploads.insert(key.clone(), staged_upload(key, path, &rel, &config.id));
                config.publish.object_url(&sha1).to_string()
            }
        };
        mapping.insert(candidate, url);
    }
    replace_strings(&mut value, &mapping);

    let mut referenced = BTreeSet::new();
    if mirror {
        value["resources_url_base"] = serde_json::Value::String(objects_prefix.clone());
        let assets = out.join("assets").join("objects");
        for file in walk_files(&assets)? {
            let (Some(name), Some(parent)) = (
                file.file_name().and_then(|n| n.to_str()),
                file.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str()),
            ) else {
                continue;
            };
            let key = format!("objects/{parent}/{name}");
            referenced.insert(key.clone());
            let label = format!("assets/{parent}/{name}");
            let upload = Upload {
                group: Some("assets".to_string()),
                ..Upload::new(key.clone(), file, label)
            };
            uploads.insert(key, upload);
        }
    }
    visit_strings(&value, &mut |s| {
        if let Some(rest) = s.strip_prefix(&objects_prefix)
            && !rest.is_empty()
        {
            referenced.insert(format!("objects/{}", decode(rest)));
        }
    });
    let metadata: InstanceMetadata = serde_json::from_value(value)?;
    ctx.hashes.save()?;

    let build_dir = ctx.repo.join(".potato").join("build");
    std::fs::create_dir_all(&build_dir)?;
    std::fs::write(
        build_dir.join("meta.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    std::fs::write(
        build_dir.join("objects.json"),
        serde_json::to_vec_pretty(&referenced)?,
    )?;

    Ok(BuildOutput {
        config,
        metadata,
        uploads,
        referenced,
    })
}

fn walk_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_url_paths() {
        assert_eq!(
            decode("instances/a/minecraft/config/my%20file+x.toml"),
            "instances/a/minecraft/config/my file+x.toml"
        );
    }

    #[test]
    fn rewrites_nested_strings() {
        let mut value = serde_json::json!({"a": ["x", {"b": "y"}], "c": "x"});
        let mapping = HashMap::from([("x".to_string(), "z".to_string())]);
        replace_strings(&mut value, &mapping);
        assert_eq!(value, serde_json::json!({"a": ["z", {"b": "y"}], "c": "z"}));
    }
}
