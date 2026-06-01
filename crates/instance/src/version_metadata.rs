use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use relative_path::RelativePathBuf;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use utils::{
    files::{self, CheckTask},
    hash_struct,
    paths::{AssetsDir, BaseUrl, DataDir, LibrariesDir, NativePath, ResourcesUrlBase, VersionsDir},
};

use crate::{
    assets::{AssetIndex, AssetsMetadata, AssetsMetadataError},
    authlib::default_authlib_injector_library,
    instance_metadata::{InstanceMetadata, ModsUpdateBehavior},
};

use super::manifest::VersionMetadataInfo;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Os {
    name: Option<String>,
    arch: Option<String>,
}

impl Os {
    fn matches_os(&self, os_name: &str, arch: &str) -> bool {
        if let Some(expected_arch) = &self.arch
            && expected_arch != arch
        {
            return false;
        }
        if let Some(expected_name) = &self.name
            && expected_name != os_name
            && expected_name != &format!("{os_name}-{arch}")
        {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replacing_download_urls_preserves_libraries() {
        let metadata = VersionMetadata {
            arguments: None,
            asset_index: None,
            downloads: None,
            id: "1.21.11".to_string(),
            java_version: None,
            libraries: vec![Library {
                name: "org.apache.logging.log4j:log4j-api:2.25.2".to_string(),
                downloads: Some(LibraryDownloads {
                    artifact: Some(Download {
                        sha1: "abc".to_string(),
                        url: Url::parse("https://example.invalid/log4j-api.jar").unwrap(),
                    }),
                    classifiers: None,
                }),
                rules: None,
                url: None,
                sha1: None,
                natives: None,
            }],
            main_class: "net.minecraft.client.main.Main".to_string(),
            inherits_from: None,
            minecraft_arguments: None,
        };
        let data_dir = DataDir::new(std::env::temp_dir());
        let base_url = BaseUrl::new(Url::parse("http://localhost:8080/").unwrap());

        let replaced = metadata
            .with_replaced_download_urls(&base_url, &data_dir)
            .await
            .unwrap();

        assert_eq!(replaced.libraries.len(), 1);
        let artifact = replaced.libraries[0]
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.artifact.as_ref())
            .unwrap();
        assert_eq!(artifact.sha1, "abc");
        assert_eq!(
            artifact.url.as_str(),
            "http://localhost:8080/libraries/org/apache/logging/log4j/log4j-api/2.25.2/log4j-api-2.25.2.jar"
        );
    }
}

/// Target OS/arch selection for native libraries
#[derive(Clone, Debug)]
pub enum OsArch {
    All,
    Specific { os: String, arch: String },
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

        if let Some(os) = &self.os
            && !os.matches_os(os_name, arch)
        {
            return None;
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

#[derive(Deserialize, Serialize, Clone)]
pub struct JavaVersion {
    #[serde(rename = "majorVersion")]
    pub major_version: u64,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Download {
    pub sha1: String,
    #[serde(
        deserialize_with = "deserialize_download_url",
        serialize_with = "serialize_download_url"
    )]
    pub url: Url,
}

/// Workaround for metadata that has `"url": ""` on locally-built artifacts (e.g. Forge).
const EMPTY_DOWNLOAD_URL: &str = "launcher-empty://download";

fn deserialize_download_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    if raw.is_empty() {
        Url::parse(EMPTY_DOWNLOAD_URL).map_err(serde::de::Error::custom)
    } else {
        Url::parse(&raw).map_err(serde::de::Error::custom)
    }
}

fn serialize_download_url<S>(url: &Url, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if url.as_str() == EMPTY_DOWNLOAD_URL {
        serializer.serialize_str("")
    } else {
        serializer.serialize_str(url.as_str())
    }
}

impl Download {
    pub fn get_check_task(&self, path: &Path) -> CheckTask {
        CheckTask {
            url: self.url.clone(),
            remote_sha1: (!self.sha1.is_empty()).then(|| self.sha1.clone()),
            remote_size: None,
            path: path.to_path_buf(),
        }
    }

    pub fn get_filename(&self) -> Option<&str> {
        self.url.path().rsplit('/').next()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct LibraryDownloads {
    pub artifact: Option<Download>,
    pub classifiers: Option<HashMap<String, Download>>,
}

#[derive(thiserror::Error, Debug)]
pub enum LibraryError {
    #[error(
        "library '{library}': invalid library name (expected group:artifact:version[:classifier], got {library:?})"
    )]
    InvalidLibraryName { library: String },
    #[error(
        "library '{library}': missing download URL (no `url` base URL and no `downloads.artifact`)"
    )]
    MissingLibraryUrl { library: String },
    #[error("library '{library}': invalid native URL in classifier download")]
    InvalidNativeUrl { library: String },
    #[error("library '{library}': invalid library path '{path}'")]
    InvalidLibraryPath { library: String, path: String },
    #[error("library '{library}': failed to construct download URL: {source}")]
    Url {
        library: String,
        #[source]
        source: url::ParseError,
    },
    #[error("library '{library}': failed to construct library path: {source}")]
    PathsLibrary {
        library: String,
        #[source]
        source: utils::paths::LibraryError,
    },
}

#[derive(thiserror::Error, Debug)]
pub enum VersionMetadataError {
    #[error("version '{version_id}': failed while processing library metadata: {source}")]
    LibraryInVersion {
        version_id: String,
        #[source]
        source: LibraryError,
    },
    #[error("failed while processing version library metadata: {0}")]
    Library(#[from] LibraryError),
    #[error("failed to read local version metadata JSON: {0}")]
    ReadFileParsed(#[from] files::ReadFileParsedError),
    #[error("network request failed while fetching version metadata: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to check local version metadata state: {0}")]
    DownloadCheckIo(#[from] std::io::Error),
    #[error("failed to parse downloaded version metadata JSON: {0}")]
    DownloadFileParsed(#[from] files::DownloadFileParsedError),
    #[error("failed to hash version metadata for manifest: {0}")]
    HashStruct(#[from] utils::HashStructError),
    #[error("missing minecraft arguments for legacy version metadata")]
    MissingMinecraftArguments,
    #[error("failed to write version metadata JSON file: {0}")]
    WriteFileJson(#[from] files::WriteFileJsonError),
    #[error("failed to hash local library file: {0}")]
    HashFileIo(std::io::Error),
    #[error("failed to gather assets metadata check tasks: {0}")]
    AssetsMetadata(#[from] AssetsMetadataError),
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Library {
    name: String,
    pub(crate) downloads: Option<LibraryDownloads>,
    pub(crate) rules: Option<Vec<Rule>>,

    // This is probably supposed to be a base URL for the libraries
    // e.g. it is equal to https://maven.fabricmc.net/
    // here: https://meta.fabricmc.net/v2/versions/loader/1.16.5/0.18.4/profile/json
    url: Option<Url>,

    // fabric doesn't have sha1 for some libraries (why)
    sha1: Option<String>,

    pub(crate) natives: Option<HashMap<String, String>>,
}

impl Library {
    pub fn empty(name: String) -> Self {
        Library {
            name,
            downloads: None,
            rules: None,
            url: None,
            sha1: None,
            natives: None,
        }
    }

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

    fn get_rel_path(&self) -> Result<RelativePathBuf, LibraryError> {
        let full_name = self.name.clone();
        let parts: Vec<&str> = full_name.split(':').collect();
        if parts.len() != 3 && parts.len() != 4 {
            return Err(LibraryError::InvalidLibraryName {
                library: self.name.clone(),
            });
        }
        let (pkg, name, version) = (parts[0], parts[1], parts[2]);
        let suffix = *parts.get(3).unwrap_or(&"");
        // neoforge adds "@jar" to the version, so we need to remove it
        let version = version.split("@jar").next().unwrap();
        let pkg_path = pkg.replace('.', "/");
        let suffix = if suffix.is_empty() {
            "".to_string()
        } else {
            format!("-{suffix}")
        };
        Ok(RelativePathBuf::from(format!(
            "{pkg_path}/{name}/{version}/{name}-{version}{suffix}.jar"
        )))
    }

    fn get_path(&self, data_dir: &DataDir) -> Result<PathBuf, LibraryError> {
        Ok(LibrariesDir::root()
            .library_path(&self.get_rel_path()?)
            .to_fs(data_dir))
    }

    pub fn get_artifact_path(&self, data_dir: &DataDir) -> Result<Option<PathBuf>, LibraryError> {
        if self.has_artifact_to_download() {
            Ok(Some(self.get_path(data_dir)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_url(&self) -> Result<Url, LibraryError> {
        self.url.clone().ok_or(LibraryError::MissingLibraryUrl {
            library: self.name.clone(),
        })
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

    pub fn get_native_rel_path(
        &self,
        native_name: &str,
        native_download: &Download,
    ) -> Result<NativePath, LibraryError> {
        LibrariesDir::root()
            .library_path(&self.get_rel_path()?)
            .native_path(
                native_name,
                native_download
                    .get_filename()
                    .ok_or(LibraryError::InvalidNativeUrl {
                        library: self.name.clone(),
                    })?,
            )
            .map_err(|source| LibraryError::PathsLibrary {
                library: self.name.clone(),
                source,
            })
    }

    fn get_arch_os_name(os: &str, arch: &str) -> String {
        os.to_string()
            + match arch {
                "arm32" => "-arm32",
                "arm64" => "-arm64",
                _ => "",
            }
    }

    /// Get the native path for the library, matching the given target OS/arch.
    /// This function expects OsArch::Specific
    pub fn get_os_native_path(&self, target: &OsArch) -> Result<Option<NativePath>, LibraryError> {
        if let OsArch::Specific { os, arch } = target
            && let Some(native_name) = self.get_native_name(&Self::get_arch_os_name(os, arch))
            && let Some(download) = self.get_native_download(native_name)
        {
            return Ok(Some(self.get_native_rel_path(native_name, download)?));
        }
        Ok(None)
    }

    /// Check if the library has a library to download.
    /// This may not be the case for libraries with only natives (classifiers).
    pub fn has_artifact_to_download(&self) -> bool {
        if let Some(downloads) = &self.downloads {
            // if the library has an artifact, we return true
            // otherwise we assume it's only for classifiers
            downloads.artifact.is_some()
        } else {
            // if "downloads" is not set at all, the library can't have classifiers
            // in this case we should always assume the library can be downloaded
            // (by inferring from the library name)
            true
        }
    }

    fn get_library_check_task(
        &self,
        data_dir: &DataDir,
    ) -> Result<Option<CheckTask>, LibraryError> {
        if let Some(downloads) = &self.downloads {
            if let Some(artifact) = &downloads.artifact {
                // if the library has an artifact, we return its check task
                Ok(Some(artifact.get_check_task(&self.get_path(data_dir)?)))
            } else {
                // if it has "downloads" but not an artifact, we don't return anything
                // since "downloads" also includes natives (classifiers)
                Ok(None)
            }
        } else {
            // else infer it from the library name
            Ok(Some(CheckTask {
                url: self
                    .get_url()?
                    .join(self.get_rel_path()?.as_str())
                    .map_err(|source| self.map_url_error(source))?,
                remote_sha1: self.sha1.clone(),
                remote_size: None,
                path: self.get_path(data_dir)?,
            }))
        }
    }

    /// Get all check_tasks, including both the library
    /// and its natives matching the given OS and arch
    /// [target = OsArch::All] means all natives
    pub fn get_check_tasks(
        &self,
        data_dir: &DataDir,
        target: &OsArch,
    ) -> Result<Vec<CheckTask>, LibraryError> {
        let mut tasks = vec![];
        if let Some(task) = self.get_library_check_task(data_dir)? {
            tasks.push(task);
        }
        if let OsArch::Specific { os, arch } = target {
            if let Some(native_name) = self.get_native_name(&Self::get_arch_os_name(os, arch))
                && let Some(download) = self.get_native_download(native_name)
            {
                let path = self
                    .get_native_rel_path(native_name, download)?
                    .to_fs(data_dir);
                tasks.push(download.get_check_task(&path));
            }
        } else if let Some(natives) = &self.natives {
            for native_name in natives.values() {
                if let Some(download) = self.get_native_download(native_name) {
                    let path = self
                        .get_native_rel_path(native_name, download)?
                        .to_fs(data_dir);
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

    /// Get the inferred sha1 url from the library name.
    /// This should only be used for libraries that don't have a proper sha1 in the metadata.
    pub fn get_inferred_sha1_url(&self) -> Result<Url, LibraryError> {
        let mut path = self.get_rel_path()?;
        path.set_file_name(
            path.file_name()
                .ok_or(LibraryError::InvalidLibraryPath {
                    library: self.name.clone(),
                    path: path.to_string(),
                })?
                .to_string()
                + ".sha1",
        );
        self.get_url()?
            .join(path.as_str())
            .map_err(|source| self.map_url_error(source))
    }

    pub fn get_group_id(&self) -> String {
        let parts: Vec<&str> = self.name.split(':').collect();
        parts[0].to_string()
    }

    pub fn get_full_name(&self) -> String {
        self.name.clone()
    }

    fn map_url_error(&self, source: url::ParseError) -> LibraryError {
        LibraryError::Url {
            library: self.name.clone(),
            source,
        }
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

#[derive(Deserialize, Serialize, Clone)]
pub struct Downloads {
    pub client: Option<Download>,
}

/// VersionMetadata is the metadata for a Minecraft version.
/// Note that version != instance, since an instance may contain multiple versions.
/// For example, a 1.21.11 Fabric instance may contain versions such as
/// "fabric-loader-0.18.4-1.21.11" and "1.21.11"
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VersionMetadata {
    pub arguments: Option<Arguments>,
    pub asset_index: Option<AssetIndex>,
    pub downloads: Option<Downloads>,
    pub id: String,
    pub java_version: Option<JavaVersion>,
    pub libraries: Vec<Library>,
    pub main_class: String,
    pub inherits_from: Option<String>,

    // legacy field used by old Minecraft versions
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
    pub async fn read_local(
        data_dir: &DataDir,
        version_id: &str,
    ) -> Result<Self, VersionMetadataError> {
        let version_path = VersionsDir::root()
            .metadata_path(version_id)
            .to_fs(data_dir);
        Ok(files::read_file_parsed(&version_path).await?)
    }

    pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<Self, VersionMetadataError> {
        let response = client.get(url).send().await?.error_for_status()?;
        let metadata = response.json().await?;
        Ok(metadata)
    }

    pub async fn read_or_download(
        client: &reqwest::Client,
        metadata_info: &VersionMetadataInfo,
        data_dir: &DataDir,
    ) -> Result<Self, VersionMetadataError> {
        let check_task = metadata_info.to_check_task(data_dir);
        if let Some(download_task) = files::get_download_task(&check_task)
            .await
            .map_err(VersionMetadataError::DownloadCheckIo)?
        {
            Ok(files::download_file_parsed(client, &download_task).await?)
        } else {
            Self::read_local(data_dir, &metadata_info.id).await
        }
    }

    pub fn get_metadata_info(
        &self,
        base_url: &BaseUrl,
    ) -> Result<VersionMetadataInfo, VersionMetadataError> {
        Ok(VersionMetadataInfo {
            id: self.id.clone(),
            url: VersionsDir::root().metadata_path(&self.id).to_url(base_url),
            sha1: hash_struct(&self)?,
        })
    }

    pub fn get_arguments(&self) -> Result<Arguments, VersionMetadataError> {
        match &self.arguments {
            Some(arguments) => Ok(arguments.clone()),
            None => {
                let minecraft_arguments = self
                    .minecraft_arguments
                    .clone()
                    .ok_or(VersionMetadataError::MissingMinecraftArguments)?;
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

    pub async fn save(&self, data_dir: &DataDir) -> Result<(), VersionMetadataError> {
        let version_path = VersionsDir::root()
            .metadata_path(&self.id)
            .to_fs_safe(data_dir);
        Ok(files::write_file_json(&version_path, self).await?)
    }

    /// Convert vanilla version metadata into instance metadata
    pub fn to_instance_metadata(self) -> InstanceMetadata {
        InstanceMetadata {
            name: self.id.clone(),
            auth_backend: None,
            include: vec![],
            mod_entries: vec![],
            mods_update_behavior: ModsUpdateBehavior::default(),
            resources_url_base: ResourcesUrlBase::default(),
            extra_forge_libs: vec![],
            authlib_injector: default_authlib_injector_library(),
            default_xmx: None,
            versions: vec![self],
            overrides_applied: false,
        }
    }

    pub async fn with_replaced_download_urls(
        &self,
        download_server_base: &BaseUrl,
        data_dir: &DataDir,
    ) -> Result<VersionMetadata, VersionMetadataError> {
        let mut replaced_metadata = VersionMetadata {
            arguments: self.arguments.clone(),
            asset_index: self.asset_index.clone(),
            downloads: self.downloads.clone(),
            id: self.id.clone(),
            java_version: self.java_version.clone(),
            libraries: vec![],
            main_class: self.main_class.clone(),
            inherits_from: self.inherits_from.clone(),
            minecraft_arguments: self.minecraft_arguments.clone(),
        };
        if let Some(downloads) = &mut replaced_metadata.downloads
            && let Some(download) = &mut downloads.client
        {
            download.url = VersionsDir::root()
                .client_jar_path(&self.id)
                .to_url(download_server_base);
        }
        if let Some(asset_index) = &mut replaced_metadata.asset_index {
            asset_index.url = AssetsDir::root()
                .asset_index_path(&asset_index.id)
                .to_url(download_server_base);
        }

        let mut replaced_libraries = Vec::with_capacity(self.libraries.len());

        for library in &self.libraries {
            let mut artifact_sha1 = None;
            if let Some(downloads) = &library.downloads {
                if let Some(artifact) = &downloads.artifact {
                    artifact_sha1 = Some(artifact.sha1.clone());
                }
            } else if library.url.is_some() {
                artifact_sha1 = Some(if let Some(sha1) = &library.sha1 {
                    sha1.clone()
                } else {
                    files::hash_file(&library.get_path(data_dir)?)
                        .await
                        .map_err(VersionMetadataError::HashFileIo)?
                });
            }
            let library_artifact = artifact_sha1
                .map(|sha1| {
                    Ok::<Download, VersionMetadataError>(Download {
                        url: LibrariesDir::root()
                            .library_path(&library.get_rel_path()?)
                            .to_url(download_server_base),
                        sha1,
                    })
                })
                .transpose()?;

            let mut maybe_classifiers = None;
            if let Some(downloads) = &library.downloads
                && let Some(classifiers) = &downloads.classifiers
            {
                let mut replaced_classifiers = HashMap::with_capacity(classifiers.len());
                for (native_name, download) in classifiers.clone() {
                    let native_path = library.get_native_rel_path(&native_name, &download)?;
                    replaced_classifiers.insert(
                        native_name,
                        Download {
                            url: native_path.to_url(download_server_base),
                            sha1: download.sha1.clone(),
                        },
                    );
                }
                maybe_classifiers = Some(replaced_classifiers);
            }

            replaced_libraries.push(Library {
                name: library.name.clone(),
                downloads: Some(LibraryDownloads {
                    artifact: library_artifact,
                    classifiers: maybe_classifiers,
                }),
                rules: library.rules.clone(),
                url: None,
                sha1: None,
                natives: library.natives.clone(),
            });
        }
        replaced_metadata.libraries = replaced_libraries;

        Ok(replaced_metadata)
    }

    /// Get all check tasks for the version, including assets.
    /// This function will also read or download the asset metadata
    /// if the version has an asset index.
    pub async fn get_check_tasks(
        &self,
        client: &reqwest::Client,
        data_dir: &DataDir,
        resources_url_base: &ResourcesUrlBase,
        target: &OsArch,
    ) -> Result<Vec<CheckTask>, VersionMetadataError> {
        let asset_metadata = if let Some(asset_index) = &self.asset_index {
            Some(AssetsMetadata::read_or_download(client, asset_index, data_dir).await?)
        } else {
            None
        };
        self.get_check_tasks_with_assets(
            data_dir,
            asset_metadata.as_ref(),
            resources_url_base,
            target,
        )
    }

    fn get_check_tasks_with_assets(
        &self,
        data_dir: &DataDir,
        asset_metadata: Option<&AssetsMetadata>,
        resources_url_base: &ResourcesUrlBase,
        target: &OsArch,
    ) -> Result<Vec<CheckTask>, VersionMetadataError> {
        let mut tasks = if let Some(asset_metadata) = asset_metadata {
            asset_metadata.get_check_tasks(data_dir, resources_url_base, false)?
        } else {
            vec![]
        };

        tasks.reserve(1 + self.libraries.len());
        if let Some(downloads) = &self.downloads
            && let Some(download) = &downloads.client
        {
            tasks.push(
                download.get_check_task(
                    &VersionsDir::root()
                        .client_jar_path(&self.id)
                        .to_fs(data_dir),
                ),
            );
        }
        for library in &self.libraries {
            let check_tasks = library
                .get_check_tasks(data_dir, target)
                .map_err(|source| VersionMetadataError::LibraryInVersion {
                    version_id: self.id.clone(),
                    source,
                })?;
            tasks.extend(check_tasks);
        }

        Ok(tasks)
    }
}
