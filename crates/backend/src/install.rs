use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs, io,
    path::Path,
    sync::Arc,
};

use either::Either;
use instance::{
    instance_metadata::{
        IncludeActionConfigOptions, InstallCause, InstallParams, InstanceMetadata, ModSyncWarning,
    },
    manifest::InstanceManifestEntry,
    mod_sync,
    storage::{LocalInstance, RemoteSource},
};
use launcher_bridge::{FrontendSender, MessageToFrontend, NotificationLevel};
use tokio::sync::mpsc;
use url::Url;
use utils::{
    adaptive_download,
    files::{self, ConfigType},
    java,
    paths::{DataDir, InstanceDirFS, InstancesDir, NativesDir},
    progress::{
        ProgressEvent, ProgressHandle, ProgressReporter, ProgressStage, ProgressTracker, Unit,
    },
};
use uuid::Uuid;

use crate::{BackendEvent, catalog::BackendCatalogEntry, instances::remote_entry_id};

#[derive(Clone)]
pub(crate) struct InstallRequest {
    pub(crate) id: Uuid,

    // TODO: pass the whole InstallParams here?
    pub(crate) cause: InstallCause,
    pub(crate) force_overwrite: bool,
    pub(crate) optional_mod_preferences: HashMap<String, bool>,

    pub(crate) launcher_dir: DataDir,
    pub(crate) client: reqwest::Client,
    pub(crate) local_instances: Vec<LocalInstance>,
    pub(crate) catalogs: HashMap<Url, BackendCatalogEntry>,
    pub(crate) frontend: FrontendSender,
    pub(crate) internal: mpsc::UnboundedSender<BackendEvent>,
}

#[derive(Clone, Debug)]
pub(crate) struct InstallOutput {
    pub(crate) instance: LocalInstance,
}

#[derive(Clone)]
struct InstallPlan {
    view_id: Uuid,
    dir_name: String,
    source: RemoteSource,
    entry: InstanceManifestEntry,
    existing: Option<LocalInstance>,
}

#[derive(Clone)]
pub(crate) struct BackendProgressReporter {
    id: Uuid,
    frontend: FrontendSender,
    internal: mpsc::UnboundedSender<BackendEvent>,
}

impl BackendProgressReporter {
    pub fn new(
        id: Uuid,
        frontend: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) -> Self {
        Self {
            id,
            frontend,
            internal,
        }
    }

    pub fn handle(&self, stage: ProgressStage, message: impl Into<String>) -> ProgressHandle<Self> {
        ProgressHandle::new(self.clone(), stage)
            .with_message(message)
            .with_unit(Unit {
                name: "items".to_string(),
                size: 1,
            })
    }
}

impl ProgressReporter for BackendProgressReporter {
    fn event(&self, event: ProgressEvent) {
        let stage = bridge_stage(&event.stage);
        let message = event
            .message
            .clone()
            .unwrap_or_else(|| stage_message(&event.stage).to_string());
        let current = if event.total > 0 {
            event.current.min(event.total)
        } else {
            event.current
        };

        self.frontend.send(MessageToFrontend::InstanceProgress {
            id: self.id,
            stage,
            current,
            total: event.total,
            message: Arc::<str>::from(message.clone()),
        });
        let _ = self.internal.send(BackendEvent::InstallProgress {
            id: self.id,
            stage,
            current,
            total: event.total,
            message: Arc::<str>::from(message),
            show_bar: event.total > 1,
        });
    }
}

pub(crate) fn bridge_stage(stage: &ProgressStage) -> launcher_bridge::ProgressStage {
    match stage {
        ProgressStage::Metadata => launcher_bridge::ProgressStage::Metadata,
        ProgressStage::Java => launcher_bridge::ProgressStage::Java,
        ProgressStage::Checking
        | ProgressStage::Downloading
        | ProgressStage::Copying
        | ProgressStage::Extracting
        | ProgressStage::Other(_) => launcher_bridge::ProgressStage::Files,
    }
}

fn stage_message(stage: &ProgressStage) -> String {
    match stage {
        ProgressStage::Checking => launcher_i18n::progress::checking_files().to_string(),
        ProgressStage::Downloading => launcher_i18n::progress::downloading_files().to_string(),
        ProgressStage::Copying => launcher_i18n::progress::copying_files().to_string(),
        ProgressStage::Extracting => launcher_i18n::progress::extracting_files().to_string(),
        ProgressStage::Metadata => launcher_i18n::progress::downloading_metadata().to_string(),
        ProgressStage::Java => launcher_i18n::progress::installing_java().to_string(),
        ProgressStage::Other(_) => launcher_i18n::progress::installing().to_string(),
    }
}

pub(crate) async fn install_instance(request: InstallRequest) -> anyhow::Result<InstallOutput> {
    let plan = resolve_install_plan(request.id, &request.local_instances, &request.catalogs)?;
    let instance_dir = InstancesDir::root()
        .instance_dir(&plan.dir_name)
        .with_data_dir(request.launcher_dir.clone());
    instance_dir.ensure_dir();

    let progress =
        BackendProgressReporter::new(plan.view_id, request.frontend.clone(), request.internal);

    let previous_mod_entries = InstanceMetadata::read_local(&instance_dir)
        .await
        .ok()
        .map(|metadata| metadata.mod_entries)
        .unwrap_or_default();

    let metadata = install_metadata(
        &request.client,
        &plan.entry,
        &instance_dir,
        progress.handle(
            ProgressStage::Metadata,
            launcher_i18n::progress::downloading_metadata(),
        ),
    )
    .await?;

    let optional_sets_enabled = mod_sync::resolve_optional_set_enabled(
        &metadata.mod_sync,
        &request.optional_mod_preferences,
    );

    let install_params = InstallParams {
        instance_dir: instance_dir.clone(),
        cause: request.cause,
        force_overwrite: request.force_overwrite,
        previous_mod_entries,
        optional_sets_enabled,
    };
    install_game_files(
        &request.client,
        &metadata,
        &install_params,
        &progress,
        &request.frontend,
    )
    .await?;

    ensure_java(&metadata, &request.launcher_dir, &progress).await?;

    let instance = if let Some(mut existing) = plan.existing {
        existing.source = Some(plan.source);
        existing.last_synced_sha1 = Some(plan.entry.sha1);
        existing
    } else {
        LocalInstance::new_remote(
            plan.view_id,
            plan.dir_name,
            plan.source,
            Some(plan.entry.sha1),
        )
    };

    Ok(InstallOutput { instance })
}

fn resolve_install_plan(
    id: Uuid,
    local_instances: &[LocalInstance],
    catalogs: &HashMap<Url, BackendCatalogEntry>,
) -> anyhow::Result<InstallPlan> {
    if let Some(local) = local_instances.iter().find(|instance| instance.id == id) {
        return resolve_local_install_plan(local.clone(), catalogs);
    }

    for (url, state) in catalogs {
        let Some(manifest) = state.manifest() else {
            continue;
        };
        for entry in &manifest.instances {
            if remote_entry_id(url, &entry.name) == id {
                let dir_name = allocate_dir_name(local_instances, &entry.name);
                return Ok(InstallPlan {
                    view_id: id,
                    dir_name,
                    source: RemoteSource {
                        manifest_url: url.clone(),
                        name_in_manifest: entry.name.clone(),
                    },
                    entry: entry.clone(),
                    existing: None,
                });
            }
        }
    }

    Err(anyhow::anyhow!(
        "instance {id} was not found in local storage or fetched catalogs"
    ))
}

fn resolve_local_install_plan(
    local: LocalInstance,
    catalogs: &HashMap<Url, BackendCatalogEntry>,
) -> anyhow::Result<InstallPlan> {
    let source = local
        .source
        .clone()
        .ok_or_else(|| anyhow::anyhow!("local-only instance cannot be updated from a backend"))?;
    let manifest = match catalogs.get(&source.manifest_url) {
        Some(state) => state
            .manifest()
            .ok_or_else(|| anyhow::anyhow!("backend catalog is not available"))?,
        None => return Err(anyhow::anyhow!("backend has not been fetched")),
    };
    let entry = manifest
        .instances
        .iter()
        .find(|entry| entry.name == source.name_in_manifest)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("instance is no longer published by its backend"))?;

    Ok(InstallPlan {
        view_id: local.id,
        dir_name: local.dir_name.clone(),
        source,
        entry,
        existing: Some(local),
    })
}

async fn install_metadata(
    client: &reqwest::Client,
    entry: &InstanceManifestEntry,
    instance_dir: &InstanceDirFS,
    progress: ProgressHandle<BackendProgressReporter>,
) -> anyhow::Result<InstanceMetadata> {
    progress.set_length(1);
    let metadata = InstanceMetadata::read_or_download(client, entry, instance_dir).await?;
    progress.inc(1);
    Ok(metadata)
}

#[derive(thiserror::Error, Debug)]
pub enum ApplyIncludesError {
    #[error("failed to delete file: {0}")]
    DeleteFileError(std::io::Error),
    #[error("failed to read config file: {0}")]
    ReadConfigError(std::io::Error),
    #[error("failed to write config file: {0}")]
    WriteConfigError(std::io::Error),
    #[error("failed to parse/serialize config file: {0}")]
    ParseJsonConfigError(#[from] serde_json::Error),
    #[error("failed to parse/serialize config file: {0}")]
    ParseYamlConfigError(#[from] serde_saphyr::Error),
    #[error("failed to parse config file: {0}")]
    SerializeYamlConfigError(#[from] serde_saphyr::ser::Error),
    #[error("failed to serialize config file: {0}")]
    ParseTomlConfigError(#[from] toml_edit::TomlError),
    #[error("invalid config file structure: {0}")]
    ConfigStructureError(String),
}

async fn apply_config_options(
    path: &Path,
    action: &IncludeActionConfigOptions,
) -> Result<(), ApplyIncludesError> {
    let contents = if path.exists() {
        tokio::fs::read_to_string(path)
            .await
            .map_err(ApplyIncludesError::ReadConfigError)?
    } else {
        match action.config_type {
            ConfigType::Json => "{}".to_string(),
            ConfigType::Yaml => "---\n".to_string(),
            ConfigType::Toml => "".to_string(),
            ConfigType::Properties => "".to_string(),
        }
    };
    // TODO: better errors
    let new_contents = match action.config_type {
        ConfigType::Json | ConfigType::Yaml => {
            let mut value: serde_json::Value = match action.config_type {
                ConfigType::Json => serde_json::from_str(&contents)?,
                ConfigType::Yaml => serde_saphyr::from_str(&contents)?,
                _ => unreachable!(),
            };
            for option in &action.options {
                let mut current = &mut value;
                for key in option.key.clone() {
                    match key {
                        Either::Left(key) => {
                            current = current
                                .as_object_mut()
                                .ok_or_else(|| {
                                    ApplyIncludesError::ConfigStructureError(
                                        "expected object".into(),
                                    )
                                })?
                                .entry(key)
                                .or_insert(serde_json::Value::Null);
                        }
                        Either::Right(index) => {
                            let array = current.as_array_mut().ok_or_else(|| {
                                ApplyIncludesError::ConfigStructureError("expected array".into())
                            })?;
                            let len = array.len();
                            if index == len {
                                array.push(serde_json::Value::Null);
                            } else if index > len {
                                return Err(ApplyIncludesError::ConfigStructureError(
                                    "cannot access index".into(),
                                ));
                            }
                            current = array.get_mut(index).expect("array element should exist");
                        }
                    }
                }
                *current = option.value.clone();
            }
            match action.config_type {
                ConfigType::Json => serde_json::to_string_pretty(&value)?,
                ConfigType::Yaml => serde_saphyr::to_string(&value)?,
                _ => unreachable!(),
            }
        }
        ConfigType::Toml => {
            todo!()
            // let mut document = contents.parse::<toml_edit::DocumentMut>()?;
            // for option in &action.options {
            //     let mut curr = document.as_item_mut();
            //     for &key in &option.key {
            //         match key {
            //             Either::Left(key) => {
            //                 curr = curr
            //                     .as_table_like_mut()
            //                     .ok_or_else(|| ApplyIncludesError::ConfigStructureError("expected table".into()))?
            //                     .entry(key)
            //                     .or_insert(toml_edit::Item::None);
            //             }
            //             Either::Right(index) => {
            //                 // If the length is n - 1, insert a new value
            //                 let array = curr
            //                     .as_array_mut()
            //                     .ok_or_else(|| ApplyIncludesError::ConfigStructureError("expected array".into()))?;
            //                 let len = array.len();
            //                 curr = array
            //                     .get_mut(*index)
            //                     .or_else(|| {
            //                         if *index == len {
            //                             array.push(toml_edit::Item::None);
            //                             array.last_mut()
            //                         } else {
            //                             None
            //                         }
            //                     })
            //                     .ok_or_else(|| ApplyIncludesError::ConfigStructureError("cannot access index".into()))?;
            //             }
            //         }
            //     }
            // }
        }
        ConfigType::Properties => todo!(),
    };
    tokio::fs::write(path, new_contents)
        .await
        .map_err(ApplyIncludesError::WriteConfigError)
}

pub(crate) async fn install_game_files(
    client: &reqwest::Client,
    metadata: &InstanceMetadata,
    params: &InstallParams,
    progress: &BackendProgressReporter,
    frontend: &FrontendSender,
) -> anyhow::Result<()> {
    let install_tasks = metadata.get_all_install_tasks(client, params).await?;

    for warning in &install_tasks.mod_warnings {
        notify_mod_sync_warning(frontend, warning);
    }

    for delete_task in &install_tasks.tasks.delete_tasks {
        files::remove_file_or_dir(&delete_task.path).await?;
    }

    let check_progress = progress.handle(
        ProgressStage::Checking,
        launcher_i18n::progress::checking_install_files(),
    );
    let download_tasks =
        files::get_download_tasks(install_tasks.tasks.check_tasks, check_progress).await?;

    let download_progress = progress.handle(
        ProgressStage::Downloading,
        launcher_i18n::progress::downloading_install_files(),
    );
    adaptive_download::download_files(download_tasks, download_progress).await?;

    enable_optional_mods(install_tasks.tasks.enable_optional_mod_tasks).await?;

    metadata
        .mark_include_downloads_complete(&params.instance_dir.minecraft_dir())
        .await?;

    if params.cause == InstallCause::Update {
        let extract_progress = progress.handle(
            ProgressStage::Extracting,
            launcher_i18n::progress::extracting_native_libraries(),
        );
        extract_natives(metadata, params.instance_dir.data_dir(), extract_progress).await?;
    }

    Ok(())
}

async fn enable_optional_mods(
    tasks: Vec<instance::instance_metadata::EnableOptionalModTask>,
) -> io::Result<()> {
    for task in tasks {
        if let Some(parent) = task.target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        files::remove_file_or_dir(&task.target).await?;
        if let Err(err) = tokio::fs::hard_link(&task.source, &task.target).await {
            log::error!(
                "Failed to hardlink optional mod from {} to {}; falling back to copy: {err}",
                task.source.display(),
                task.target.display()
            );
            tokio::fs::copy(&task.source, &task.target).await?;
        }
    }
    Ok(())
}

pub(crate) async fn sync_instance_mods(
    client: &reqwest::Client,
    launcher_dir: DataDir,
    dir_name: &str,
    view_id: Uuid,
    optional_mod_preferences: HashMap<String, bool>,
    frontend: FrontendSender,
    internal: mpsc::UnboundedSender<BackendEvent>,
) -> anyhow::Result<()> {
    let instance_dir = InstancesDir::root()
        .instance_dir(dir_name)
        .with_data_dir(launcher_dir.clone());
    let metadata = InstanceMetadata::read_local(&instance_dir).await?;
    let optional_sets_enabled =
        mod_sync::resolve_optional_set_enabled(&metadata.mod_sync, &optional_mod_preferences);
    let progress = BackendProgressReporter::new(view_id, frontend.clone(), internal);
    let install_params = InstallParams {
        instance_dir,
        cause: InstallCause::Update,
        force_overwrite: false,
        previous_mod_entries: metadata.mod_entries.clone(),
        optional_sets_enabled,
    };
    install_game_files(client, &metadata, &install_params, &progress, &frontend).await
}

fn notify_mod_sync_warning(frontend: &FrontendSender, warning: &ModSyncWarning) {
    let message = match warning {
        ModSyncWarning::ModRemoved { mod_id, path } => {
            log::warn!("Removed mod {mod_id} at {}", path.display());
            launcher_i18n::notifications::mod_removed(mod_id.clone(), path.display().to_string())
        }
        ModSyncWarning::ModAdded { mod_id, path } => {
            log::warn!("Restored mod {mod_id} to {}", path.display());
            launcher_i18n::notifications::mod_added(mod_id.clone(), path.display().to_string())
        }
    };
    frontend.send(MessageToFrontend::Notification {
        level: NotificationLevel::Warning,
        message: Arc::from(message),
    });
}

pub(crate) async fn resolve_java_for_launch(
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    configured_path: Option<&str>,
    progress: &BackendProgressReporter,
) -> anyhow::Result<java::JavaInstallation> {
    let java_version = metadata.get_java_version();
    if let Some(path) = configured_path {
        let java_path = Path::new(path);
        if java::check_java(&java_version, java_path).await
            && let Some(installation) = java::get_installation_pub(java_path).await
        {
            progress
                .handle(
                    ProgressStage::Java,
                    launcher_i18n::progress::java_already_installed(),
                )
                .finish();
            return Ok(installation);
        }
    }
    if let Some(installation) = java::get_java(&java_version, data_dir).await {
        progress
            .handle(
                ProgressStage::Java,
                launcher_i18n::progress::java_already_installed(),
            )
            .finish();
        return Ok(installation);
    }

    java::download_java(
        &java_version,
        data_dir,
        progress.handle(
            ProgressStage::Java,
            launcher_i18n::progress::installing_java_version(java_version.clone()),
        ),
    )
    .await?;
    java::get_java(&java_version, data_dir)
        .await
        .ok_or_else(|| anyhow::anyhow!("Java {java_version} is still missing after download"))
}

pub(crate) async fn ensure_java(
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    progress: &BackendProgressReporter,
) -> anyhow::Result<()> {
    let java_version = metadata.get_java_version();
    if java::get_java(&java_version, data_dir).await.is_some() {
        progress
            .handle(
                ProgressStage::Java,
                launcher_i18n::progress::java_already_installed(),
            )
            .finish();
        return Ok(());
    }

    java::download_java(
        &java_version,
        data_dir,
        progress.handle(
            ProgressStage::Java,
            launcher_i18n::progress::installing_java_version(java_version.clone()),
        ),
    )
    .await?;
    Ok(())
}

async fn extract_natives(
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    progress: ProgressHandle<BackendProgressReporter>,
) -> anyhow::Result<()> {
    let native_paths = metadata.get_native_library_paths(data_dir)?;
    progress.set_length(native_paths.len() as u64);
    let natives_dir = NativesDir::for_id(metadata.get_parent_id()?).to_fs(data_dir);
    if natives_dir.exists() {
        tokio::fs::remove_dir_all(&natives_dir).await?;
    }
    tokio::fs::create_dir_all(&natives_dir).await?;

    for native_path in native_paths {
        extract_zip(&native_path, &natives_dir)?;
        progress.inc(1);
    }
    progress.finish();
    Ok(())
}

fn extract_zip(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let file = fs::File::open(src)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(enclosed_name) = entry.enclosed_name() else {
            continue;
        };
        if enclosed_name
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == OsStr::new("META-INF"))
        {
            continue;
        }
        let output_path = dest.join(enclosed_name);
        if entry.is_dir() {
            fs::create_dir_all(&output_path)?;
        } else {
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = fs::File::create(output_path)?;
            io::copy(&mut entry, &mut output)?;
        }
    }
    Ok(())
}

fn allocate_dir_name(local_instances: &[LocalInstance], base: &str) -> String {
    let taken = local_instances
        .iter()
        .map(|instance| instance.dir_name.as_str())
        .collect::<HashSet<_>>();
    let normalized = base.trim();
    let base = if normalized.is_empty() {
        "Instance"
    } else {
        normalized
    };
    if !taken.contains(base) {
        return base.to_string();
    }
    for suffix in 1.. {
        let candidate = format!("{base} ({suffix})");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search should always return")
}

#[cfg(test)]
mod tests {
    use super::*;
    use instance::manifest::InstanceManifest;
    use launcher_bridge::ProgressStage as BridgeProgressStage;

    #[test]
    fn maps_worker_progress_to_bridge_stage() {
        assert_eq!(
            bridge_stage(&ProgressStage::Metadata),
            BridgeProgressStage::Metadata
        );
        assert_eq!(
            bridge_stage(&ProgressStage::Java),
            BridgeProgressStage::Java
        );
        assert_eq!(
            bridge_stage(&ProgressStage::Checking),
            BridgeProgressStage::Files
        );
        assert_eq!(
            bridge_stage(&ProgressStage::Extracting),
            BridgeProgressStage::Files
        );
    }

    use crate::catalog::{BackendCatalogEntry, BackendFetchStatus};

    fn ok_catalog(manifest: InstanceManifest) -> BackendCatalogEntry {
        BackendCatalogEntry::with_manifest(manifest, BackendFetchStatus::Ok)
    }

    #[test]
    fn resolves_remote_install_plan_from_fetched_catalog() {
        let url = Url::parse("https://example.com/manifest.json").unwrap();
        let entry = InstanceManifestEntry {
            name: "Vanilla".to_string(),
            url: Url::parse("https://example.com/vanilla/meta.json").unwrap(),
            sha1: "abc".to_string(),
            auth_backend: None,
            required_java_version: "8".to_string(),
        };
        let id = remote_entry_id(&url, &entry.name);
        let catalogs = HashMap::from([(
            url.clone(),
            ok_catalog(InstanceManifest {
                instances: vec![entry],
            }),
        )]);

        let plan = resolve_install_plan(id, &[], &catalogs).unwrap();

        assert_eq!(plan.view_id, id);
        assert_eq!(plan.dir_name, "Vanilla");
        assert_eq!(plan.source.manifest_url, url);
        assert_eq!(plan.source.name_in_manifest, "Vanilla");
    }

    #[test]
    fn allocates_distinct_directory_names_for_duplicate_display_names() {
        let local_instances = vec![
            LocalInstance::new_local("Vanilla".to_string()),
            LocalInstance::new_local("Vanilla (1)".to_string()),
        ];

        assert_eq!(
            allocate_dir_name(&local_instances, "Vanilla"),
            "Vanilla (2)"
        );
    }

    #[test]
    fn resolves_existing_remote_instance_for_update() {
        let url = Url::parse("https://example.com/manifest.json").unwrap();
        let local = LocalInstance::new_remote(
            Uuid::new_v4(),
            "Vanilla".to_string(),
            RemoteSource {
                manifest_url: url.clone(),
                name_in_manifest: "Vanilla".to_string(),
            },
            Some("old".to_string()),
        );
        let entry = InstanceManifestEntry {
            name: "Vanilla".to_string(),
            url: Url::parse("https://example.com/vanilla/meta.json").unwrap(),
            sha1: "new".to_string(),
            auth_backend: None,
            required_java_version: "8".to_string(),
        };
        let catalogs = HashMap::from([(
            url.clone(),
            ok_catalog(InstanceManifest {
                instances: vec![entry],
            }),
        )]);

        let plan = resolve_install_plan(local.id, std::slice::from_ref(&local), &catalogs).unwrap();

        assert_eq!(plan.view_id, local.id);
        assert_eq!(plan.dir_name, "Vanilla");
        assert!(plan.existing.is_some());
    }
}
