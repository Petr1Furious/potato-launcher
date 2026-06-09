use std::{
    collections::{HashMap, HashSet},
    ops,
    path::{Path, PathBuf},
};

use relative_path::RelativePathBuf;
use serde::{Deserialize, Serialize};

use launcher_auth::providers::AuthProviderConfig;
use url::Url;
use utils::{
    files::{self, CheckTask, ConfigOption, ConfigOptionTask, ConfigType, DeleteTask},
    hash_struct,
    paths::{
        AssetsDir, BaseUrl, DataDir, InstanceDirFS, InstancesDir, ResourcesUrlBase, VersionsDir,
    },
};

use crate::{
    assets::{AssetIndex, AssetsMetadata},
    mod_sync::{ModSyncError, build_mod_sync_plan},
    os::{get_os_name, get_system_arch},
    overrides::with_overrides,
};

use super::{
    manifest::InstanceManifestEntry,
    manifest::ManifestError,
    version_metadata::{Arguments, Library, OsArch, VersionMetadata, VersionMetadataError},
};

const COMPLETION_MARKER_FILE: &str = ".download_complete";

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Object {
    pub path: RelativePathBuf,
    pub sha1: String,
    pub size: u64,
    pub url: Url,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApplyOn {
    Update,
    Always,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IncludeActionFile {
    pub object: Option<Object>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IncludeActionConfigOptions {
    pub config_type: ConfigType,
    pub options: Vec<ConfigOption>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IncludeActionDirectory {
    pub objects: Vec<Object>,
    /// If true, files in this directory that are not present in the manifest will be deleted
    pub delete_extra: bool,
    /// If true, this action will be skipped if the directory already exists and has the completion marker file
    pub skip_if_dir_exists: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IncludeAction {
    File(IncludeActionFile),
    ConfigOptions(IncludeActionConfigOptions),
    Directory(IncludeActionDirectory),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IncludeEntry {
    pub path: RelativePathBuf,
    pub apply_on: ApplyOn,
    /// For directories, passed through to all files in the directory
    pub overwrite: bool,
    #[serde(flatten)]
    pub action: IncludeAction,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ModEntry {
    pub mod_id: String,
    pub object: Object,
}

pub use crate::install_params::{InstallCause, InstallParams};
pub use crate::mod_sync::{ModSyncSettings, ModSyncWarning};

pub struct InstallTasks {
    pub tasks: TaskSet,
    /// Warnings from mod sync related to illegal user actions (e.g. deleting a required mod)
    pub mod_warnings: Vec<ModSyncWarning>,
}

#[derive(Debug)]
pub struct EnableOptionalModTask {
    pub source: PathBuf,
    pub target: PathBuf,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSyncMode {
    #[default]
    OnUpdate,
    Always,
    AlwaysFast,
}

/// Complete metadata for a single instance.
/// Contains all used minecraft versions (also known as client.json)
/// e.g. "fabric-loader-0.18.4-1.21.11" and "1.21.11" for a fabric 1.21.11 instance
/// and other fields
#[derive(Deserialize, Serialize)]
pub struct InstanceMetadata {
    /// instance name
    #[serde(default)]
    pub name: String,

    /// auth backend to use for this instance
    #[serde(default)]
    pub auth_backend: Option<AuthProviderConfig>,

    /// additional files to include with the instance
    /// (e.g. configs, server.dat, etc.)
    /// and rules for what to do when file/directory contents differ from the remote
    // TODO: docs about specificity rules
    #[serde(default)]
    pub include: Vec<IncludeEntry>,

    /// Mod jars declared by the instance manifest
    #[serde(default)]
    pub mod_entries: Vec<ModEntry>,
    /// How local mod jars are reconciled with `mod_entries` on install/update/launch
    pub mod_sync: ModSyncSettings,
    /// When to verify client jar, libraries, and assets (separate from mod sync)
    pub resource_sync: ResourceSyncMode,

    /// base URL for assets
    /// if not set, the launcher will download assets from Mojang servers
    #[serde(default)]
    pub resources_url_base: ResourcesUrlBase,

    /// Forge/NeoForge installer libraries not listed in version metadata.
    /// Populated for server-built instances so clients can download them;
    /// empty for local instances (jars are copied during generation instead).
    #[serde(default)]
    pub extra_forge_libs: Vec<Library>,

    /// authlib-injector jar for custom auth providers (javaagent only, not classpath)
    #[serde(default = "crate::authlib::default_authlib_injector_library")]
    pub authlib_injector: Library,

    /// default JVM RAM limit (`-Xmx`) for this version
    /// e.g. "8192M"
    pub default_xmx: Option<String>,

    // used minecraft versions (client.json) ordered from parent to child
    // e.g. ["1.21.11", "fabric-loader-0.18.4-1.21.11"] since fabric-loader-0.18.4-1.21.11 inherits from 1.21.11
    #[serde(default)]
    pub versions: Vec<VersionMetadata>,

    // whether the overrides were already applied to the libraries (e.g. on instance_builder build)
    // this should be false for mojang's vanilla versions
    pub overrides_applied: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum InstanceMetadataError {
    #[error("Missing asset index")]
    MissingAssetIndex,
    #[error("Missing client download")]
    MissingClientDownload,
    #[error("Missing version metadata")]
    MissingVersionMetadata,
    #[error("io error: {0}")]
    ReadFileIo(#[from] std::io::Error),
    #[error("failed to parse instance metadata JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to download instance metadata JSON: {0}")]
    DownloadFileParsed(#[from] files::DownloadFileParsedError),
    #[error("failed while processing version metadata: {0}")]
    VersionMetadata(#[from] VersionMetadataError),
    #[error("failed while processing version library metadata: {0}")]
    Library(#[from] super::version_metadata::LibraryError),
    #[error("failed while processing asset metadata: {0}")]
    AssetsMetadata(#[from] crate::assets::AssetsMetadataError),
    #[error("failed to download asset index file: {0}")]
    DownloadFile(#[from] files::DownloadFileError),
    #[error("failed to read local JSON file: {0}")]
    ReadFileParsed(#[from] files::ReadFileParsedError),
    #[error("failed to build asset object URL: {0}")]
    ParseUrl(#[from] url::ParseError),
    #[error("failed to hash instance metadata for manifest: {0}")]
    HashStruct(#[from] utils::HashStructError),
    #[error("failed to write instance metadata JSON file: {0}")]
    WriteFileJson(#[from] files::WriteFileJsonError),
    #[error("failed while building manifest metadata: {0}")]
    Manifest(#[from] ManifestError),
    #[error("missing object in include entry: {0}")]
    MissingObject(String),
    #[error("include entry and object paths do not match: {0}")]
    PathMismatch(String),
    #[error("mod sync failed: {0}")]
    ModSync(#[from] ModSyncError),
    #[error("failed while checking local files for download: {0}")]
    GetDownloadTasks(#[from] files::GetDownloadTasksError),
}

#[derive(Default)]
pub struct TaskSet {
    pub check_tasks: Vec<CheckTask>,
    pub config_option_tasks: Vec<ConfigOptionTask>,
    pub delete_tasks: Vec<DeleteTask>,
    pub enable_optional_mod_tasks: Vec<EnableOptionalModTask>,
}

impl ops::AddAssign<TaskSet> for TaskSet {
    fn add_assign(&mut self, rhs: TaskSet) {
        self.check_tasks.extend(rhs.check_tasks);
        self.config_option_tasks.extend(rhs.config_option_tasks);
        self.delete_tasks.extend(rhs.delete_tasks);
        self.enable_optional_mod_tasks
            .extend(rhs.enable_optional_mod_tasks);
    }
}

impl InstanceMetadata {
    pub async fn read_local(instance_dir: &InstanceDirFS) -> Result<Self, InstanceMetadataError> {
        let meta_path = instance_dir.meta_path();
        Ok(files::read_file_parsed(&meta_path).await?)
    }

    pub async fn read_or_download(
        client: &reqwest::Client,
        entry: &InstanceManifestEntry,
        instance_dir: &InstanceDirFS,
    ) -> Result<Self, InstanceMetadataError> {
        let check_task = entry.to_check_task(instance_dir);
        if let Some(download_task) = files::get_download_task(&check_task).await? {
            Ok(files::download_file_parsed(client, &download_task).await?)
        } else {
            Self::read_local(instance_dir).await
        }
    }

    pub fn to_manifest_entry(
        &self,
        unique_name: &str,
        base_url: &BaseUrl,
    ) -> Result<InstanceManifestEntry, InstanceMetadataError> {
        Ok(InstanceManifestEntry {
            name: self.name.clone(),
            url: InstancesDir::root()
                .instance_dir(unique_name)
                .meta_path()
                .to_url(base_url),
            sha1: hash_struct(&self)?,
            auth_backend: self.auth_backend.clone(),
            required_java_version: self.get_java_version(),
        })
    }

    pub async fn save(&self, instance_dir: &InstanceDirFS) -> Result<(), InstanceMetadataError> {
        let path = instance_dir.meta_path();
        Ok(files::write_file_json(&path, self).await?)
    }

    pub fn get_resources_url_base(&self) -> &ResourcesUrlBase {
        &self.resources_url_base
    }

    pub fn get_java_version(&self) -> String {
        self.versions
            .first()
            .and_then(|metadata| metadata.java_version.as_ref())
            .map(|x| x.major_version.to_string())
            .unwrap_or_else(|| "8".to_string())
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_client_check_task(
        &self,
        data_dir: &DataDir,
    ) -> Result<CheckTask, InstanceMetadataError> {
        let metadata = self
            .versions
            .first()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?;

        if let Some(downloads) = metadata.downloads.as_ref()
            && let Some(client) = downloads.client.as_ref()
        {
            Ok(client.get_check_task(
                &VersionsDir::root()
                    .client_jar_path(self.get_id()?)
                    .to_fs(data_dir),
            ))
        } else {
            Err(InstanceMetadataError::MissingClientDownload)
        }
    }

    pub fn get_auth_provider(&self) -> Option<&AuthProviderConfig> {
        self.auth_backend.as_ref()
    }

    pub async fn get_all_install_tasks(
        &self,
        client: &reqwest::Client,
        params: &InstallParams,
    ) -> Result<InstallTasks, InstanceMetadataError> {
        let mut tasks = TaskSet::default();
        if params.cause == InstallCause::Update
            || matches!(
                self.resource_sync,
                ResourceSyncMode::Always | ResourceSyncMode::AlwaysFast
            )
        {
            tasks
                .check_tasks
                .push(self.get_client_check_task(params.instance_dir.data_dir())?);
            tasks
                .check_tasks
                .extend(self.get_library_check_tasks(params.instance_dir.data_dir())?);
            tasks.check_tasks.extend(
                self.get_asset_check_tasks(client, params.instance_dir.data_dir())
                    .await?,
            );
        }

        let mod_plan = build_mod_sync_plan(&self.mod_entries, &self.mod_sync, params)?;
        tasks += mod_plan.tasks;
        tasks += self.get_include_tasks(params)?;

        Ok(InstallTasks {
            tasks,
            mod_warnings: mod_plan.warnings,
        })
    }

    pub async fn mark_include_downloads_complete(
        &self,
        minecraft_dir: &Path,
    ) -> Result<(), InstanceMetadataError> {
        for rule in &self.include {
            if !matches!(rule.action, IncludeAction::Directory(_)) {
                continue;
            }
            let rule_path = rule.path.to_path(minecraft_dir);
            if rule_path.is_dir() {
                tokio::fs::write(rule_path.join(COMPLETION_MARKER_FILE), b"").await?;
            }
        }
        Ok(())
    }

    pub fn get_native_library_paths(
        &self,
        data_dir: &DataDir,
    ) -> Result<Vec<PathBuf>, InstanceMetadataError> {
        let target = current_os_arch();
        let mut paths = Vec::new();
        for library in self.get_libraries_with_overrides() {
            if let Some(native_path) = library.get_os_native_path(&target)? {
                paths.push(native_path.to_fs(data_dir));
            }
        }
        Ok(paths)
    }

    pub fn get_classpath_paths(
        &self,
        data_dir: &DataDir,
    ) -> Result<Vec<PathBuf>, InstanceMetadataError> {
        let mut paths = Vec::new();
        for library in self.get_libraries_with_overrides() {
            if let Some(path) = library.get_artifact_path(data_dir)? {
                paths.push(path);
            }
        }
        let effective_client_path = VersionsDir::root()
            .client_jar_path(self.get_id()?)
            .to_fs(data_dir);
        paths.push(effective_client_path);
        Ok(paths)
    }

    fn get_library_check_tasks(
        &self,
        data_dir: &DataDir,
    ) -> Result<Vec<CheckTask>, InstanceMetadataError> {
        let target = current_os_arch();
        let mut tasks = Vec::new();
        for library in self
            .get_libraries_with_overrides()
            .into_iter()
            .chain(self.get_extra_forge_libs())
        {
            tasks.extend(library.get_check_tasks(data_dir, &target)?);
        }
        tasks.extend(
            self.authlib_injector
                .get_check_tasks(data_dir, &OsArch::All)?,
        );
        Ok(tasks)
    }

    async fn get_asset_check_tasks(
        &self,
        client: &reqwest::Client,
        data_dir: &DataDir,
    ) -> Result<Vec<CheckTask>, InstanceMetadataError> {
        let mut tasks = Vec::new();

        for version in &self.versions {
            let Some(asset_index) = &version.asset_index else {
                continue;
            };

            let index_task = asset_index.get_check_task(data_dir);
            if let Some(download_task) = files::get_download_task(&index_task).await? {
                files::download_file(client, &download_task).await?;
            }
            let path = AssetsDir::root()
                .asset_index_path(&asset_index.id)
                .to_fs(data_dir);
            let asset_metadata: AssetsMetadata = files::read_file_parsed(&path).await?;
            tasks.extend(asset_metadata.get_check_tasks(
                data_dir,
                &self.resources_url_base,
                false,
            )?);
        }

        Ok(tasks)
    }

    fn get_include_tasks(&self, params: &InstallParams) -> Result<TaskSet, InstanceMetadataError> {
        let mut tasks = TaskSet::default();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();

        let mut include_file = |object: &Object, path: PathBuf, overwrite: bool| {
            if overwrite || !path.exists() {
                tasks.check_tasks.push(CheckTask {
                    url: object.url.clone(),
                    remote_size: Some(object.size),
                    remote_sha1: Some(object.sha1.clone()),
                    path,
                });
            }
        };

        // includes are sorted from the most specific to the less specific by the generator.
        // seen_paths is only used to avoid deleting files included by more specific rules,
        // preventing object intersections is done by the generator
        for entry in &self.include {
            let entry_path = entry.path.to_path(params.instance_dir.minecraft_dir());
            if entry.apply_on == ApplyOn::Update && params.cause != InstallCause::Update {
                continue;
            }
            match &entry.action {
                IncludeAction::File(action) => {
                    let object = action.object.as_ref().ok_or_else(|| {
                        InstanceMetadataError::MissingObject(entry.path.to_string())
                    })?;
                    if entry.path != object.path {
                        return Err(InstanceMetadataError::PathMismatch(entry.path.to_string()));
                    }
                    include_file(object, entry_path.clone(), entry.overwrite);
                    seen_paths.insert(entry_path);
                }
                IncludeAction::ConfigOptions(action) => {
                    tasks.config_option_tasks.push(ConfigOptionTask {
                        path: entry_path,
                        config_type: action.config_type,
                        options: action.options.clone(),
                    });
                }
                IncludeAction::Directory(action) => {
                    if action.skip_if_dir_exists
                        && entry_path.exists()
                        && entry_path.join(COMPLETION_MARKER_FILE).exists()
                    {
                        continue;
                    }
                    for object in action.objects.iter() {
                        let path = object.path.to_path(params.instance_dir.minecraft_dir());
                        include_file(object, path.clone(), entry.overwrite);
                        seen_paths.insert(path);
                    }
                    if action.delete_extra || params.force_overwrite {
                        let dir_path = entry.path.to_path(params.instance_dir.minecraft_dir());
                        for file in files::get_files_ignore_paths(&dir_path, &seen_paths) {
                            tasks.delete_tasks.push(DeleteTask { path: file.clone() });
                        }
                    }
                }
            }
        }

        // TODO: can duplicates be added?
        tasks.check_tasks = files::dedup_check_tasks(tasks.check_tasks);
        Ok(tasks)
    }

    pub fn get_libraries_with_overrides(&self) -> Vec<Library> {
        let os_name = get_os_name();
        let arch = get_system_arch();

        let all_libraries = self
            .versions
            .iter()
            .rev() // prioritize child libraries
            .flat_map(|metadata| {
                if self.overrides_applied {
                    metadata.libraries.clone()
                } else {
                    with_overrides(&metadata.libraries, &metadata.id)
                }
            });

        let mut existing_names = HashMap::new();
        all_libraries
            .filter(|library| library.applies_to_os(&os_name, &arch))
            .filter(|library| {
                // Newer NeoForge versions add duplicate asm library
                let (name, version) = library.get_name_and_version();
                if let Some(prev_version) = existing_names.get(&name) {
                    version == *prev_version
                } else {
                    existing_names.insert(name, version);
                    true
                }
            })
            .collect()
    }

    pub fn get_id(&self) -> Result<&str, InstanceMetadataError> {
        Ok(&self
            .versions
            .last()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?
            .id)
    }

    pub fn get_parent_id(&self) -> Result<&str, InstanceMetadataError> {
        Ok(&self
            .versions
            .first()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?
            .id)
    }

    pub fn get_asset_index(&self) -> Result<&AssetIndex, InstanceMetadataError> {
        let version = self
            .versions
            .first()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?;
        version
            .asset_index
            .as_ref()
            .ok_or(InstanceMetadataError::MissingAssetIndex)
    }

    pub fn get_arguments(&self) -> Result<Arguments, InstanceMetadataError> {
        let first = self
            .versions
            .first()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?;
        let mut merged_arguments = first.get_arguments()?;

        for metadata in &self.versions[1..] {
            if let Some(arguments) = metadata.arguments.clone() {
                merged_arguments.game.extend(arguments.game);
                merged_arguments.jvm.extend(arguments.jvm);
            } else if metadata.minecraft_arguments.is_some() {
                merged_arguments = metadata.get_arguments()?;
            }
        }

        Ok(merged_arguments)
    }

    pub fn get_main_class(&self) -> Result<&str, InstanceMetadataError> {
        Ok(self
            .versions
            .last()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?
            .main_class
            .as_str())
    }

    pub fn get_extra_forge_libs(&self) -> Vec<Library> {
        self.extra_forge_libs.clone()
    }

    pub fn get_default_xmx(&self) -> Option<&str> {
        self.default_xmx.as_deref()
    }
}

fn current_os_arch() -> OsArch {
    OsArch::Specific {
        os: get_os_name(),
        arch: get_system_arch(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_metadata(include: Vec<IncludeEntry>, mod_entries: Vec<ModEntry>) -> InstanceMetadata {
        InstanceMetadata {
            name: "Test".to_string(),
            auth_backend: None,
            include,
            mod_entries,
            mod_sync: ModSyncSettings::default(),
            resource_sync: ResourceSyncMode::OnUpdate,
            resources_url_base: ResourcesUrlBase::default(),
            extra_forge_libs: Vec::new(),
            authlib_injector: crate::authlib::default_authlib_injector_library(),
            default_xmx: None,
            versions: Vec::new(),
            overrides_applied: true,
        }
    }

    #[test]
    fn client_download_uses_effective_child_version_id() {
        let data_dir = DataDir::new(std::env::temp_dir());
        let mut metadata = empty_metadata(vec![], vec![]);
        metadata.versions = vec![
            VersionMetadata {
                arguments: None,
                asset_index: None,
                downloads: Some(crate::version_metadata::Downloads {
                    client: Some(crate::version_metadata::Download {
                        sha1: "abc".to_string(),
                        url: Url::parse("https://example.invalid/client.jar").unwrap(),
                        size: 100,
                    }),
                }),
                id: "1.21.11".to_string(),
                java_version: None,
                libraries: Vec::new(),
                main_class: "net.minecraft.client.main.Main".to_string(),
                inherits_from: None,
                minecraft_arguments: None,
            },
            VersionMetadata {
                arguments: None,
                asset_index: None,
                downloads: None,
                id: "fabric-loader-0.19.2-1.21.11".to_string(),
                java_version: None,
                libraries: Vec::new(),
                main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".to_string(),
                inherits_from: Some("1.21.11".to_string()),
                minecraft_arguments: None,
            },
        ];

        let task = metadata.get_client_check_task(&data_dir).unwrap();

        assert!(
            task.path.ends_with(
                "versions/fabric-loader-0.19.2-1.21.11/fabric-loader-0.19.2-1.21.11.jar"
            )
        );
    }
}
