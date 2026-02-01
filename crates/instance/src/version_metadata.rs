use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncReadExt as _};
use url::Url;

use utils::{
    files::{self, CheckTask},
    paths::{DataDir, LibrariesDir, VersionsDir},
    progress,
};

use crate::instance_metadata::InstanceMetadata;

use super::manifest::VersionMetadataInfo;

fn get_arch_os_name(os_name: &str, arch: &str) -> String {
    os_name.to_string()
        + match arch {
            "arm32" => "-arm32",
            "arm64" => "-arm64",
            _ => "",
        }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Os {
    name: Option<String>,
    arch: Option<String>,
}

impl Os {
    fn matches_os(&self, os_name: &str, arch: &str) -> bool {
        if let Some(expected_arch) = &self.arch {
            if expected_arch != arch {
                return false;
            }
        }
        if let Some(expected_name) = &self.name {
            if expected_name != os_name && expected_name != &format!("{os_name}-{arch}") {
                return false;
            }
        }

        true
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Rule {
    /// "allow" or "disallow"
    action: String,
    /// Optional OS constraints
    os: Option<Os>,
    /// Optional feature flags (e.g. {"has_custom_resolution": true})
    features: Option<HashMap<String, bool>>,
}

impl Rule {
    fn allowed_on_os(&self, os_name: &str, arch: &str) -> Option<bool> {
        let is_allowed = self.action == "allow";
        let matching_features = ["has_custom_resolution"];

        if let Some(os) = &self.os {
            if !os.matches_os(os_name, arch) {
                return None;
            }
        }

        if let Some(features) = &self.features {
            for (feature, value) in features {
                let contains = matching_features.contains(&feature.as_str());
                if contains != *value {
                    return None;
                }
            }
        }

        Some(is_allowed)
    }
}

fn rules_apply(rules: &[Rule], os_name: &str, arch: &str) -> bool {
    let mut some_allowed = false;
    for rule in rules {
        if let Some(is_allowed) = rule.allowed_on_os(os_name, arch) {
            if !is_allowed {
                return false;
            }
            some_allowed = true;
        }
    }
    some_allowed
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum ArgumentValue {
    String(String),
    Array(Vec<String>),
}

impl ArgumentValue {
    pub fn get_values(&self) -> Vec<&str> {
        match self {
            ArgumentValue::String(s) => vec![s.as_str()],
            ArgumentValue::Array(a) => a.iter().map(|x| x.as_str()).collect(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ComplexArgument {
    value: ArgumentValue,
    rules: Vec<Rule>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum VariableArgument {
    Simple(String),
    Complex(ComplexArgument),
}

impl VariableArgument {
    pub fn get_values(&self) -> Vec<&str> {
        match self {
            VariableArgument::Simple(s) => vec![s.as_str()],
            VariableArgument::Complex(c) => c.value.get_values(),
        }
    }

    pub fn get_matching_values(&self, os_name: &str, arch: &str) -> Vec<&str> {
        match self {
            VariableArgument::Simple(s) => vec![s.as_str()],
            VariableArgument::Complex(complex) => {
                if rules_apply(&complex.rules, os_name, arch) {
                    complex.value.get_values()
                } else {
                    vec![]
                }
            }
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Arguments {
    pub game: Vec<VariableArgument>,
    pub jvm: Vec<VariableArgument>,
}

#[derive(Deserialize, Serialize)]
pub struct AssetIndex {
    pub id: String,
    pub sha1: String,
    pub url: Url,
}

#[derive(Deserialize, Serialize)]
pub struct JavaVersion {
    #[serde(rename = "majorVersion")]
    pub major_version: u64,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Download {
    pub sha1: String,
    pub url: Url,
}

impl Download {
    pub fn get_check_task(&self, path: &Path) -> CheckTask {
        CheckTask {
            url: self.url.clone(),
            remote_sha1: Some(self.sha1.clone()),
            path: path.to_path_buf(),
        }
    }

    pub fn get_filename(&self) -> &str {
        self.url
            .path()
            .rsplit('/')
            .next()
            .unwrap_or(self.url.path())
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct LibraryDownloads {
    pub artifact: Option<Download>,
    pub classifiers: Option<HashMap<String, Download>>,
}

lazy_static::lazy_static! {
    static ref MOJANG_LIBRARIES_URL: Url = Url::parse("https://libraries.minecraft.net/").expect("valid url");
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Library {
    name: String,
    pub(crate) downloads: Option<LibraryDownloads>,
    pub(crate) rules: Option<Vec<Rule>>,
    url: Option<Url>,
    sha1: Option<String>,
    pub(crate) natives: Option<HashMap<String, String>>,
}

impl Library {
    pub fn from_download(name: String, url: Url, sha1: String) -> Self {
        Library {
            name,
            downloads: Some(LibraryDownloads {
                artifact: Some(Download { url, sha1 }),
                classifiers: None,
            }),
            rules: None,
            url: None,
            sha1: None,
            natives: None,
        }
    }

    fn get_rel_path_from_name(&self) -> String {
        let full_name = self.name.clone();
        let mut parts: Vec<&str> = full_name.split(':').collect();
        if parts.len() != 4 {
            parts.push("");
        }
        let (pkg, name, version, suffix) = (parts[0], parts[1], parts[2], parts[3]);
        // neoforge adds "@jar" to the version, so we need to remove it
        let version = version.split("@jar").next().unwrap();
        let pkg_path = pkg.replace('.', "/");
        let suffix = if suffix.is_empty() {
            "".to_string()
        } else {
            format!("-{suffix}")
        };
        format!("{pkg_path}/{name}/{version}/{name}-{version}{suffix}.jar")
    }

    fn get_path_from_name(&self, data_dir: &DataDir) -> PathBuf {
        return LibrariesDir::root()
            .to_fs(data_dir)
            .join(self.get_rel_path_from_name());
    }

    pub fn get_library_path(&self, data_dir: &DataDir) -> Option<PathBuf> {
        if let Some(downloads) = &self.downloads {
            if downloads.artifact.is_some() {
                Some(self.get_path_from_name(data_dir))
            } else {
                None
            }
        } else {
            Some(self.get_path_from_name(data_dir))
        }
    }

    pub fn get_url(&self) -> Url {
        self.url.clone().unwrap_or(MOJANG_LIBRARIES_URL.clone())
    }

    fn get_library_dir(&self, data_dir: &DataDir) -> PathBuf {
        self.get_path_from_name(data_dir)
            .parent()
            .expect("valid path")
            .to_path_buf()
    }

    fn get_native_name(&self, os_arch: &str) -> Option<&str> {
        self.natives.as_ref()?.get(os_arch).map(|x| x.as_str())
    }

    pub fn get_native_download(&self, native_name: &str) -> Option<&Download> {
        let downloads = self.downloads.as_ref()?;
        let classifiers = downloads.classifiers.as_ref()?;
        let download = classifiers.get(native_name)?;
        Some(download)
    }

    pub fn get_native_path(
        &self,
        data_dir: &DataDir,
        native_name: &str,
        native_download: &Download,
    ) -> PathBuf {
        self.get_library_dir(data_dir)
            .join(native_name)
            .join(native_download.get_filename())
    }

    pub fn get_os_native_path(
        &self,
        data_dir: &DataDir,
        os_name: &str,
        arch: &str,
    ) -> Option<PathBuf> {
        if let Some(native_name) = self.get_native_name(&get_arch_os_name(os_name, arch)) {
            if let Some(download) = self.get_native_download(native_name) {
                return Some(self.get_native_path(data_dir, native_name, download));
            }
        }
        None
    }

    fn get_library_check_task(&self, data_dir: &DataDir) -> anyhow::Result<Option<CheckTask>> {
        if let Some(downloads) = &self.downloads {
            if let Some(artifact) = &downloads.artifact
                && let Some(path) = self.get_library_path(data_dir)
            {
                // if the library has an artifact, we return its check task
                Ok(Some(artifact.get_check_task(&path)))
            } else {
                // if it has "downloads" but not an artifact, we don't return anything
                // since "downloads" also includes natives (classifiers)
                Ok(None)
            }
        } else {
            // else infer it from the library name
            Ok(Some(CheckTask {
                url: self.get_url().join(&self.get_rel_path_from_name())?,
                remote_sha1: self.sha1.clone(),
                path: self.get_path_from_name(data_dir),
            }))
        }
    }

    /// Get all check_tasks, including both the library
    /// and its natives matching the given OS and arch
    /// [os_with_arch = None] means all natives
    pub fn get_check_tasks(
        &self,
        data_dir: &DataDir,
        os_with_arch: Option<(&str, &str)>,
    ) -> anyhow::Result<Vec<CheckTask>> {
        let mut tasks = vec![];
        if let Some(task) = self.get_library_check_task(data_dir)? {
            tasks.push(task);
        }
        if let Some((os_name, arch)) = os_with_arch {
            if let Some(native_name) = self.get_native_name(&get_arch_os_name(os_name, arch)) {
                if let Some(download) = self.get_native_download(native_name) {
                    let path = self.get_native_path(data_dir, native_name, download);
                    tasks.push(download.get_check_task(&path));
                }
            }
        } else if let Some(natives) = &self.natives {
            for native_name in natives.values() {
                if let Some(download) = self.get_native_download(native_name) {
                    let path = self.get_native_path(data_dir, native_name, download);
                    tasks.push(download.get_check_task(&path));
                }
            }
        }

        Ok(tasks)
    }

    pub fn applies_to_os(&self, os_name: &str, arch: &str) -> bool {
        if let Some(rules) = &self.rules {
            rules_apply(rules, os_name, arch)
        } else {
            true
        }
    }

    pub fn get_sha1_url(&self) -> anyhow::Result<Url> {
        let mut path = self.get_rel_path_from_name();
        path.push_str(".sha1");
        Ok(self.get_url().join(&path)?)
    }

    pub fn get_group_id(&self) -> String {
        let parts: Vec<&str> = self.name.split(':').collect();
        parts[0].to_string()
    }

    pub fn get_full_name(&self) -> String {
        self.name.clone()
    }

    pub fn get_name_and_version(&self) -> (String, String) {
        let mut parts: Vec<&str> = self.name.split(':').collect();
        if parts.len() != 4 {
            parts.push("");
        }
        let version = parts[2].to_string();
        parts.remove(2);
        (parts.join(":"), version)
    }
}

#[derive(Deserialize, Serialize)]
pub struct Downloads {
    pub client: Option<Download>,
}

/// VersionMetadata is the metadata for a Minecraft version.
/// Note that version != instance, since an instance may contain multiple versions.
/// For example, a 1.21.11 Fabric instance may contain versions such as
/// "fabric-loader-0.18.4-1.21.11" and "1.21.11"
#[derive(Deserialize, Serialize)]
pub struct VersionMetadata {
    pub arguments: Option<Arguments>,

    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndex>,

    pub downloads: Option<Downloads>,
    pub id: String,

    #[serde(rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
    pub libraries: Vec<Library>,

    #[serde(rename = "mainClass")]
    pub main_class: String,

    #[serde(rename = "inheritsFrom")]
    pub inherits_from: Option<String>,

    // legacy field used by old Minecraft versions
    #[serde(rename = "minecraftArguments")]
    pub minecraft_arguments: Option<String>,
}

lazy_static::lazy_static! {
    static ref LEGACY_JVM_ARGS: Vec<VariableArgument> = vec![
        VariableArgument::Complex(ComplexArgument {
            value: ArgumentValue::String("-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump".to_string()),
            rules: vec![Rule{
                action: "allow".to_string(),
                os: Some(Os {
                    name: Some("windows".to_string()),
                    arch: None,
                }),
                features: None,
            }],
        }),
        VariableArgument::Complex(ComplexArgument {
            value: ArgumentValue::Array(vec!["-Dos.name=Windows 10".to_string(), "-Dos.version=10.0".to_string()]),
            rules: vec![Rule{
                action: "allow".to_string(),
                os: Some(Os {
                    name: Some("windows".to_string()),
                    arch: None,
                }),
                features: None,
            }],
        }),
        VariableArgument::Simple("-Djava.library.path=${natives_directory}".to_string()),
        VariableArgument::Simple("-Dminecraft.launcher.brand=${launcher_name}".to_string()),
        VariableArgument::Simple("-Dminecraft.launcher.version=${launcher_version}".to_string()),
        VariableArgument::Simple("-cp".to_string()),
        VariableArgument::Simple("${classpath}".to_string()),
    ];
}

impl VersionMetadata {
    pub async fn read_local(data_dir: &DataDir, version_id: &str) -> anyhow::Result<Self> {
        let version_path = VersionsDir::root()
            .metadata_path(version_id)
            .to_fs(data_dir);
        let mut file = fs::File::open(version_path).await?;
        let mut content = String::new();
        file.read_to_string(&mut content).await?;
        let metadata = serde_json::from_str(&content)?;
        Ok(metadata)
    }

    pub async fn fetch(client: &reqwest::Client, url: &str) -> anyhow::Result<Self> {
        let response = client.get(url).send().await?.error_for_status()?;
        let metadata = response.json().await?;
        Ok(metadata)
    }

    pub fn get_check_task(metadata_info: &VersionMetadataInfo, data_dir: &DataDir) -> CheckTask {
        let url = metadata_info.url.clone();
        let sha1 = metadata_info.sha1.clone();
        let path = VersionsDir::root()
            .metadata_path(&metadata_info.id)
            .to_fs(data_dir);
        CheckTask {
            url,
            remote_sha1: Some(sha1),
            path,
        }
    }

    pub async fn read_or_download(
        client: &reqwest::Client,
        metadata_info: &VersionMetadataInfo,
        data_dir: &DataDir,
    ) -> anyhow::Result<Self> {
        let check_task = Self::get_check_task(metadata_info, data_dir);
        let check_tasks = vec![check_task];
        let download_entries =
            files::get_download_tasks(check_tasks, progress::no_progress_bar()).await?;
        files::download_files(client, download_entries, progress::no_progress_bar()).await?;
        Self::read_local(data_dir, &metadata_info.id).await
    }

    pub fn get_arguments(&self) -> anyhow::Result<Arguments> {
        match &self.arguments {
            Some(arguments) => Ok(arguments.clone()),
            None => {
                let minecraft_arguments = self.minecraft_arguments.clone().unwrap();
                Ok(Arguments {
                    game: minecraft_arguments
                        .split_whitespace()
                        .map(|x| VariableArgument::Simple(x.to_string()))
                        .collect(),
                    jvm: LEGACY_JVM_ARGS.clone(),
                })
            }
        }
    }

    pub async fn save(&self, data_dir: &DataDir) -> anyhow::Result<()> {
        let version_path = VersionsDir::root()
            .metadata_path(&self.id)
            .to_fs_safe(data_dir);
        let content = serde_json::to_string(self)?;
        fs::write(version_path, content).await?;
        Ok(())
    }

    /// Convert vanilla version metadata into instance metadata
    pub fn to_instance_metadata(self) -> InstanceMetadata {
        InstanceMetadata {
            name: self.id.clone(),
            auth_backend: None,
            include: vec![],
            resources_url_base: None,
            extra_forge_libs: vec![],
            default_xmx: None,
            versions: vec![self],
            overrides_applied: false,
        }
    }
}
