use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use instance::{
    instance_metadata::{ContentRule, ResourceSyncMode},
    localized::LocalizedString,
    mod_sync::ModSyncMode,
};
use launcher_auth::providers::AuthProviderConfig;
use serde::Deserialize;
use url::Url;

pub const CONFIG_FILE: &str = "potato.toml";
pub const LOCAL_DIR: &str = ".potato";
pub const LOCAL_FILE: &str = ".potato/local.toml";
pub const INDEX_DIRS: [&str; 4] = ["mods", "resourcepacks", "shaderpacks", "datapacks"];

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoaderName {
    #[default]
    Vanilla,
    Fabric,
    Forge,
    Neoforge,
}

impl LoaderName {
    pub fn as_str(self) -> &'static str {
        match self {
            LoaderName::Vanilla => "vanilla",
            LoaderName::Fabric => "fabric",
            LoaderName::Forge => "forge",
            LoaderName::Neoforge => "neoforge",
        }
    }

    pub fn to_generate(self) -> generate::instance::Loader {
        match self {
            LoaderName::Vanilla => generate::instance::Loader::Vanilla,
            LoaderName::Fabric => generate::instance::Loader::Fabric,
            LoaderName::Forge => generate::instance::Loader::Forge,
            LoaderName::Neoforge => generate::instance::Loader::Neoforge,
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct PublishConfig {
    pub base_url: Url,
    pub target: String,
    #[serde(default)]
    pub mirror: bool,
}

impl PublishConfig {
    pub fn base(&self) -> Url {
        let mut url = self.base_url.clone();
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        url
    }

    pub fn object_url(&self, sha1: &str) -> Url {
        self.base()
            .join(&object_key(sha1))
            .expect("object key is a valid relative URL")
    }
}

pub fn object_key(sha1: &str) -> String {
    format!("objects/{}/{sha1}", &sha1[..2])
}

#[derive(Deserialize, Clone)]
pub struct OptionalSetConfig {
    pub id: String,
    pub display_name: LocalizedString,
    #[serde(default)]
    pub enabled_by_default: bool,
    pub mods: Vec<String>,
}

#[derive(Deserialize, Clone, Default)]
pub struct ClientConfig {
    #[serde(default)]
    pub resource_sync: ResourceSyncMode,
    #[serde(default)]
    pub mod_sync: ModSyncMode,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub blocked: Vec<String>,
    #[serde(default)]
    pub optional_sets: Vec<OptionalSetConfig>,
}

#[derive(Deserialize, Clone, Default)]
pub struct ServerConfig {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Deserialize, Clone)]
pub struct Config {
    pub id: String,
    pub display_name: Option<LocalizedString>,
    pub minecraft: String,
    #[serde(default)]
    pub loader: LoaderName,
    pub loader_version: Option<String>,
    pub default_xmx: Option<String>,
    pub auth_backend: Option<AuthProviderConfig>,
    pub order: Option<i64>,
    pub publish: PublishConfig,
    #[serde(default)]
    pub client: ClientConfig,
    #[serde(default)]
    pub paths: Vec<ContentRule>,
    #[serde(default)]
    pub servers: BTreeMap<String, ServerConfig>,
}

impl Config {
    pub fn parse(content: &str) -> anyhow::Result<Self> {
        let config: Config = toml::from_str(content).context("invalid potato.toml")?;
        utils::instance_id::validate_instance_id(&config.id)
            .map_err(|err| anyhow::anyhow!("invalid id '{}': {err}", config.id))?;
        for set in &config.client.optional_sets {
            utils::instance_id::validate_instance_id(&set.id)
                .map_err(|err| anyhow::anyhow!("invalid optional set id '{}': {err}", set.id))?;
        }
        Ok(config)
    }

    pub fn load(repo: &Path) -> anyhow::Result<Self> {
        let path = repo.join(CONFIG_FILE);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("can't read {}", path.display()))?;
        Self::parse(&content)
    }

    pub fn managed_roots(&self) -> Vec<String> {
        self.paths
            .iter()
            .map(|rule| rule.path.normalize().into_string())
            .filter(|path| !path.is_empty() && path != "mods")
            .collect()
    }

    /// `mods` always; other pack folders only when `[[paths]]` covers them.
    pub fn handles_index_dir(&self, dir: &str) -> bool {
        dir == "mods" || self.managed_roots().iter().any(|root| under(dir, root))
    }

    pub fn is_pack_file(&self, path: &str) -> bool {
        path == CONFIG_FILE
            || crate::index::index_dir_of(path).is_some_and(|dir| self.handles_index_dir(dir))
            || (crate::git::is_committed_binary(path)
                && path
                    .split_once('/')
                    .is_some_and(|(dir, _)| self.handles_index_dir(dir)))
            || self.is_managed_file(path)
    }

    pub fn is_managed_file(&self, path: &str) -> bool {
        if crate::index::index_dir_of(path).is_some() || crate::git::is_committed_binary(path) {
            return false;
        }
        self.managed_roots().iter().any(|root| under(path, root))
    }
}

pub fn under(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum TargetType {
    Prism,
    Server,
}

impl TargetType {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetType::Prism => "prism",
            TargetType::Server => "server",
        }
    }
}

#[derive(Deserialize)]
pub struct LocalTarget {
    pub kind: TargetType,
    pub path: PathBuf,
}

#[derive(Deserialize, Default)]
pub struct LocalConfig {
    #[serde(default)]
    pub targets: BTreeMap<String, LocalTarget>,
}

impl LocalConfig {
    pub fn load(repo: &Path) -> anyhow::Result<Self> {
        let path = repo.join(LOCAL_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let mut config: LocalConfig =
            toml::from_str(&content).context("invalid .potato/local.toml")?;
        for target in config.targets.values_mut() {
            target.path = expand_home(&target.path);
        }
        Ok(config)
    }
}

pub fn set_local_target(
    repo: &Path,
    name: &str,
    target: Option<(TargetType, &Path)>,
) -> anyhow::Result<()> {
    let file = repo.join(LOCAL_FILE);
    let content = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = content.parse().context("invalid .potato/local.toml")?;
    let targets = doc
        .entry("targets")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .context("[targets] in .potato/local.toml must be a table")?;
    targets.set_implicit(true);
    match target {
        Some((kind, path)) => {
            let mut table = toml_edit::Table::new();
            table["kind"] = toml_edit::value(kind.as_str());
            table["path"] = toml_edit::value(path.to_string_lossy().as_ref());
            targets.insert(name, toml_edit::Item::Table(table));
        }
        None => {
            targets.remove(name);
        }
    }
    std::fs::create_dir_all(repo.join(LOCAL_DIR))?;
    std::fs::write(&file, doc.to_string())?;
    Ok(())
}

fn expand_home(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    path.to_path_buf()
}

pub fn set_versions(
    repo: &Path,
    minecraft: &str,
    loader: LoaderName,
    loader_version: Option<&str>,
) -> anyhow::Result<bool> {
    let path = repo.join(CONFIG_FILE);
    let content = std::fs::read_to_string(&path)?;
    let mut doc: toml_edit::DocumentMut = content.parse()?;
    doc["minecraft"] = toml_edit::value(minecraft);
    doc["loader"] = toml_edit::value(loader.as_str());
    match loader_version {
        Some(version) => doc["loader_version"] = toml_edit::value(version),
        None => {
            doc.remove("loader_version");
        }
    }
    let updated = doc.to_string();
    if updated == content {
        return Ok(false);
    }
    std::fs::write(&path, updated)?;
    Ok(true)
}

pub fn cache_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("POTATO_CACHE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    match dirs::cache_dir() {
        Some(dir) => Ok(dir.join("potato")),
        None => bail!("can't find a cache dir, set POTATO_CACHE_DIR"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
id = "minigames"
display_name = { en = "Minigames", ru = "Миниигры" }
minecraft = "1.21.1"
loader = "neoforge"
loader_version = "21.1.77"
order = 10

[publish]
base_url = "https://mc.example.com/data"
target = "file:///srv/site/data"

[client]
mod_sync = "mirror"
blocked = ["optifine", "modrinth:xaeros-minimap"]

[[client.optional_sets]]
id = "shaders"
display_name = { en = "Shaders" }
mods = ["iris"]

[[paths]]
path = "config"
type = "directory"
skip_if_dir_exists = true

[[paths]]
path = "options.txt"
type = "file"
overwrite = false

[servers.main]
exclude = ["config/foo.toml"]
"#;

    #[test]
    fn parses_example() {
        let config = Config::parse(EXAMPLE).unwrap();
        assert_eq!(config.id, "minigames");
        assert_eq!(config.loader, LoaderName::Neoforge);
        assert_eq!(config.paths.len(), 2);
        assert_eq!(config.client.mod_sync, ModSyncMode::Mirror);
        assert_eq!(config.servers["main"].exclude, vec!["config/foo.toml"]);
        assert!(config.is_managed_file("config/a/b.toml"));
        assert!(config.is_managed_file("options.txt"));
        assert!(!config.is_managed_file("configs/x"));
        assert!(!config.is_managed_file("mods/.index/a.pw.toml"));
        assert_eq!(
            config.publish.object_url("abcdef").as_str(),
            "https://mc.example.com/data/objects/ab/abcdef"
        );
    }

    #[test]
    fn rejects_bad_id() {
        let bad = EXAMPLE.replace("id = \"minigames\"", "id = \"Bad Id\"");
        assert!(Config::parse(&bad).is_err());
    }
}
