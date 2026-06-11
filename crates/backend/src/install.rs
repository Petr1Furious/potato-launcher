use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs, io,
    path::Path,
    sync::Arc,
};

use anyhow::anyhow;
use either::Either;
use instance::{
    instance_metadata::{InstallCause, InstallParams, InstanceMetadata, ModSyncWarning},
    manifest::InstanceManifestEntry,
    mod_sync,
    storage::{self, InstanceId, InstanceState, LocalInstance, RemoteSource},
};
use launcher_bridge::{FrontendSender, MessageToFrontend, NotificationLevel};
use tokio::sync::mpsc;
use url::Url;
use utils::{
    adaptive_download,
    files::{self, ConfigOptionTask, ConfigType},
    java,
    paths::{DataDir, InstanceDirFS, InstancesDir, NativesDir},
    progress::{
        ProgressEvent, ProgressHandle, ProgressReporter, ProgressStage, ProgressTracker, Unit,
    },
};

use crate::{BackendEvent, catalog::BackendCatalogEntry, instances::remote_entry_id};

#[derive(Clone)]
pub(crate) struct InstallRequest {
    pub(crate) id: InstanceId,

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
    view_id: InstanceId,
    dir_name: String,
    source: RemoteSource,
    entry: InstanceManifestEntry,
    existing: Option<LocalInstance>,
}

#[derive(Clone)]
pub(crate) struct BackendProgressReporter {
    id: InstanceId,
    frontend: FrontendSender,
    internal: mpsc::UnboundedSender<BackendEvent>,
}

impl BackendProgressReporter {
    pub fn new(
        id: InstanceId,
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
            id: self.id.clone(),
            stage,
            current,
            total: event.total,
            message: Arc::<str>::from(message.clone()),
        });
        let _ = self.internal.send(BackendEvent::InstallProgress {
            id: self.id.clone(),
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
    let plan = resolve_install_plan(&request.id, &request.local_instances, &request.catalogs)?;
    let instance_dir = InstancesDir::root()
        .instance_dir(&plan.dir_name)
        .with_data_dir(request.launcher_dir.clone());
    instance_dir.ensure_dir();

    let progress = BackendProgressReporter::new(
        plan.view_id.clone(),
        request.frontend.clone(),
        request.internal,
    );

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

    let previous_mod_entries = InstanceMetadata::read_local(&instance_dir)
        .await
        .ok()
        .map(|metadata| metadata.mod_entries)
        .unwrap_or_default();
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

    // it is important to save metadata after install_game_files
    // because mod_sync looks at the delta between old and new metadata
    metadata.save(&instance_dir).await?;

    if request.cause == InstallCause::Update {
        resolve_java(&metadata, &request.launcher_dir, None, &progress).await?;
    }

    let instance = if let Some(mut existing) = plan.existing {
        if request.cause == InstallCause::Run && !existing.is_installed() {
            return Err(anyhow!(
                "attempting to run an instance that is not installed"
            ));
        }
        existing.source = Some(plan.source);
        existing.state = InstanceState::Installed;
        existing.last_synced_sha1 = Some(plan.entry.sha1);
        existing
    } else {
        if request.cause == InstallCause::Run {
            return Err(anyhow!(
                "attempting to run an instance that is not installed"
            ));
        }
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
    id: &InstanceId,
    local_instances: &[LocalInstance],
    catalogs: &HashMap<Url, BackendCatalogEntry>,
) -> anyhow::Result<InstallPlan> {
    if let Some(local) = local_instances.iter().find(|instance| &instance.id == id) {
        return resolve_local_install_plan(local.clone(), catalogs);
    }

    for (url, state) in catalogs {
        let Some(manifest) = state.manifest() else {
            continue;
        };
        for entry in &manifest.instances {
            if remote_entry_id(url, &entry.name) == *id {
                let dir_name = allocate_dir_name(local_instances, &entry.name);
                return Ok(InstallPlan {
                    view_id: id.clone(),
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
        view_id: local.id.clone(),
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
    // do not save metadata to disk yet
    // save only after install_game_files
    let metadata = InstanceMetadata::read_or_fetch(client, entry, instance_dir).await?;
    progress.inc(1);
    Ok(metadata)
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigTaskError {
    #[error("failed to read config file: {0}")]
    ReadConfig(std::io::Error),
    #[error("failed to write config file: {0}")]
    WriteConfig(std::io::Error),
    #[error("failed to parse/serialize config file: {0}")]
    ParseJsonConfig(#[from] serde_json::Error),
    #[error("failed to parse/serialize config file: {0}")]
    ParseYamlConfig(#[from] serde_saphyr::Error),
    #[error("failed to parse config file: {0}")]
    SerializeYamlConfig(#[from] serde_saphyr::ser::Error),
    #[error("failed to serialize config file: {0}")]
    ParseTomlConfig(#[from] toml_edit::TomlError),
    #[error("invalid config file structure: {0}")]
    ConfigStructure(String),
}

fn config_key_path(key: &[Either<String, usize>]) -> String {
    let parts = key
        .iter()
        .map(|part| match part {
            Either::Left(key) => serde_json::Value::String(key.clone()),
            Either::Right(index) => serde_json::Value::Number((*index).into()),
        })
        .collect::<Vec<_>>();
    serde_json::Value::Array(parts).to_string()
}

fn with_config_option_context(
    option: &utils::files::ConfigOption,
    error: ConfigTaskError,
) -> ConfigTaskError {
    match error {
        ConfigTaskError::ConfigStructure(reason) => ConfigTaskError::ConfigStructure(format!(
            "{reason} while applying option {}",
            config_key_path(&option.key)
        )),
        error => error,
    }
}

fn with_config_task_context(task: &ConfigOptionTask, error: ConfigTaskError) -> ConfigTaskError {
    match error {
        ConfigTaskError::ConfigStructure(reason) => ConfigTaskError::ConfigStructure(format!(
            "{:?} config {}: {reason}",
            task.config_type,
            task.path.display()
        )),
        error => error,
    }
}

fn apply_json_config_option(
    value: &mut serde_json::Value,
    option: &utils::files::ConfigOption,
) -> Result<(), ConfigTaskError> {
    let mut current = value;
    for key in option.key.clone() {
        match key {
            Either::Left(key) => {
                current = current
                    .as_object_mut()
                    .ok_or_else(|| ConfigTaskError::ConfigStructure("expected object".into()))?
                    .entry(key)
                    .or_insert(serde_json::Value::Null);
            }
            Either::Right(index) => {
                let array = current
                    .as_array_mut()
                    .ok_or_else(|| ConfigTaskError::ConfigStructure("expected array".into()))?;
                let len = array.len();
                if index == len {
                    array.push(serde_json::Value::Null);
                } else if index > len {
                    return Err(ConfigTaskError::ConfigStructure(
                        "cannot access index".into(),
                    ));
                }
                current = array.get_mut(index).expect("array element should exist");
            }
        }
    }
    *current = option.value.clone();
    Ok(())
}

fn apply_json_config_options(
    value: &mut serde_json::Value,
    options: &[utils::files::ConfigOption],
) -> Result<(), ConfigTaskError> {
    for option in options {
        apply_json_config_option(value, option)
            .map_err(|error| with_config_option_context(option, error))?;
    }
    Ok(())
}

fn json_to_toml_value(value: &serde_json::Value) -> Result<toml_edit::Value, ConfigTaskError> {
    match value {
        serde_json::Value::Null => Err(ConfigTaskError::ConfigStructure(
            "TOML config values cannot be null".into(),
        )),
        serde_json::Value::Bool(value) => Ok(toml_edit::Value::from(*value)),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(toml_edit::Value::from(value))
            } else if let Some(value) = value.as_u64() {
                let value = i64::try_from(value).map_err(|_| {
                    ConfigTaskError::ConfigStructure("TOML integer is too large".into())
                })?;
                Ok(toml_edit::Value::from(value))
            } else if let Some(value) = value.as_f64() {
                Ok(toml_edit::Value::from(value))
            } else {
                Err(ConfigTaskError::ConfigStructure(
                    "unsupported TOML number".into(),
                ))
            }
        }
        serde_json::Value::String(value) => Ok(toml_edit::Value::from(value.clone())),
        serde_json::Value::Array(values) => {
            let mut array = toml_edit::Array::new();
            for value in values {
                array.push_formatted(json_to_toml_value(value)?);
            }
            Ok(toml_edit::Value::from(array))
        }
        serde_json::Value::Object(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values {
                table.insert(key, json_to_toml_value(value)?);
            }
            Ok(toml_edit::Value::from(table))
        }
    }
}

fn toml_placeholder_for_key(key: &Either<String, usize>) -> toml_edit::Value {
    match key {
        Either::Left(_) => toml_edit::Value::from(toml_edit::InlineTable::new()),
        Either::Right(_) => toml_edit::Value::from(toml_edit::Array::new()),
    }
}

fn set_toml_item_path(
    item: &mut toml_edit::Item,
    key: &[Either<String, usize>],
    value: toml_edit::Value,
) -> Result<(), ConfigTaskError> {
    let Some((head, tail)) = key.split_first() else {
        *item = toml_edit::Item::Value(value);
        return Ok(());
    };

    match head {
        Either::Left(key) => {
            let table = item
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_like_mut()
                .ok_or_else(|| ConfigTaskError::ConfigStructure("expected table".into()))?;
            let child = table.entry(key).or_insert(toml_edit::Item::None);
            set_toml_item_path(child, tail, value)
        }
        Either::Right(index) => {
            let array = item
                .as_array_mut()
                .ok_or_else(|| ConfigTaskError::ConfigStructure("expected array".into()))?;
            set_toml_array_path(array, *index, tail, value)
        }
    }
}

fn set_toml_value_path(
    current: &mut toml_edit::Value,
    key: &[Either<String, usize>],
    value: toml_edit::Value,
) -> Result<(), ConfigTaskError> {
    let Some((head, tail)) = key.split_first() else {
        *current = value;
        return Ok(());
    };

    match head {
        Either::Left(key) => {
            let table = current
                .as_inline_table_mut()
                .ok_or_else(|| ConfigTaskError::ConfigStructure("expected inline table".into()))?;
            if tail.is_empty() {
                table.insert(key, value);
                Ok(())
            } else {
                let child = table.get_or_insert(key, toml_placeholder_for_key(&tail[0]));
                set_toml_value_path(child, tail, value)
            }
        }
        Either::Right(index) => {
            let array = current
                .as_array_mut()
                .ok_or_else(|| ConfigTaskError::ConfigStructure("expected array".into()))?;
            set_toml_array_path(array, *index, tail, value)
        }
    }
}

fn set_toml_array_path(
    array: &mut toml_edit::Array,
    index: usize,
    tail: &[Either<String, usize>],
    value: toml_edit::Value,
) -> Result<(), ConfigTaskError> {
    let len = array.len();
    if index > len {
        return Err(ConfigTaskError::ConfigStructure(
            "cannot access index".into(),
        ));
    }

    if tail.is_empty() {
        if index == len {
            array.push_formatted(value);
        } else {
            *array.get_mut(index).expect("array element should exist") = value;
        }
        return Ok(());
    }

    if index == len {
        array.push_formatted(toml_placeholder_for_key(&tail[0]));
    }
    let child = array.get_mut(index).expect("array element should exist");
    set_toml_value_path(child, tail, value)
}

fn apply_toml_config_options(
    document: &mut toml_edit::DocumentMut,
    options: &[utils::files::ConfigOption],
) -> Result<(), ConfigTaskError> {
    for option in options {
        set_toml_item_path(
            document.as_item_mut(),
            &option.key,
            json_to_toml_value(&option.value)?,
        )
        .map_err(|error| with_config_option_context(option, error))?;
    }
    Ok(())
}

fn apply_properties_config_options(
    contents: &str,
    options: &[utils::files::ConfigOption],
) -> Result<String, ConfigTaskError> {
    let mut lines = contents
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    for option in options {
        let [Either::Left(key)] = option.key.as_slice() else {
            return Err(with_config_option_context(
                option,
                ConfigTaskError::ConfigStructure(
                    "properties config keys must be a single string".into(),
                ),
            ));
        };
        let value = match &option.value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::Null
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => {
                return Err(with_config_option_context(
                    option,
                    ConfigTaskError::ConfigStructure(
                        "properties config values must be strings, booleans, or numbers".into(),
                    ),
                ));
            }
        };

        let mut replaced = false;
        for line in &mut lines {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                continue;
            }
            let Some(separator) = trimmed.find(['=', ':']) else {
                continue;
            };
            if trimmed[..separator].trim_end() == key {
                *line = format!("{key}={value}");
                replaced = true;
                break;
            }
        }
        if !replaced {
            lines.push(format!("{key}={value}"));
        }
    }

    let mut output = lines.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

async fn run_config_option_task(task: &ConfigOptionTask) -> Result<(), ConfigTaskError> {
    let contents = if task.path.exists() {
        tokio::fs::read_to_string(&task.path)
            .await
            .map_err(ConfigTaskError::ReadConfig)?
    } else {
        match task.config_type {
            ConfigType::Json => "{}".to_string(),
            ConfigType::Yaml => "---\n".to_string(),
            ConfigType::Toml => "".to_string(),
            ConfigType::Properties => "".to_string(),
        }
    };

    let new_contents = match task.config_type {
        ConfigType::Json | ConfigType::Yaml => {
            let mut value: serde_json::Value = match task.config_type {
                ConfigType::Json => serde_json::from_str(&contents)?,
                ConfigType::Yaml => serde_saphyr::from_str(&contents)?,
                _ => unreachable!(),
            };
            apply_json_config_options(&mut value, &task.options)
                .map_err(|error| with_config_task_context(task, error))?;
            match task.config_type {
                ConfigType::Json => serde_json::to_string_pretty(&value)?,
                ConfigType::Yaml => serde_saphyr::to_string(&value)?,
                _ => unreachable!(),
            }
        }
        ConfigType::Toml => {
            let mut document = contents.parse::<toml_edit::DocumentMut>()?;
            apply_toml_config_options(&mut document, &task.options)
                .map_err(|error| with_config_task_context(task, error))?;
            document.to_string()
        }
        ConfigType::Properties => apply_properties_config_options(&contents, &task.options)
            .map_err(|error| with_config_task_context(task, error))?,
    };
    tokio::fs::write(&task.path, new_contents)
        .await
        .map_err(ConfigTaskError::WriteConfig)
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

    for config_task in &install_tasks.tasks.config_option_tasks {
        run_config_option_task(config_task).await?;
    }

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
    view_id: InstanceId,
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

pub(crate) async fn resolve_java(
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
    storage::allocate_dir_name(&taken, base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use instance::manifest::InstanceManifest;
    use launcher_bridge::ProgressStage as BridgeProgressStage;
    use serde_json::json;
    use utils::files::ConfigOption;

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

        let plan = resolve_install_plan(&id, &[], &catalogs).unwrap();

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

    fn config_option(key: Vec<Either<String, usize>>, value: serde_json::Value) -> ConfigOption {
        ConfigOption { key, value }
    }

    fn key(name: &str) -> Either<String, usize> {
        Either::Left(name.to_string())
    }

    fn index(index: usize) -> Either<String, usize> {
        Either::Right(index)
    }

    #[tokio::test]
    async fn config_option_task_updates_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        tokio::fs::write(&path, r#"{"mods":[{}]}"#).await.unwrap();

        run_config_option_task(&ConfigOptionTask {
            path: path.clone(),
            config_type: ConfigType::Json,
            options: vec![
                config_option(vec![key("enabled")], json!(true)),
                config_option(vec![key("mods"), index(0), key("name")], json!("example")),
            ],
        })
        .await
        .unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(path).await.unwrap()).unwrap();
        assert_eq!(value["enabled"], json!(true));
        assert_eq!(value["mods"][0]["name"], json!("example"));
    }

    #[tokio::test]
    async fn config_option_task_updates_yaml() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        tokio::fs::write(&path, "mods:\n  - {}\n").await.unwrap();

        run_config_option_task(&ConfigOptionTask {
            path: path.clone(),
            config_type: ConfigType::Yaml,
            options: vec![config_option(
                vec![key("mods"), index(0), key("enabled")],
                json!(true),
            )],
        })
        .await
        .unwrap();

        let value: serde_json::Value =
            serde_saphyr::from_str(&tokio::fs::read_to_string(path).await.unwrap()).unwrap();
        assert_eq!(value["mods"][0]["enabled"], json!(true));
    }

    #[tokio::test]
    async fn config_option_task_updates_toml() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, "mods = [{}]\n").await.unwrap();

        run_config_option_task(&ConfigOptionTask {
            path: path.clone(),
            config_type: ConfigType::Toml,
            options: vec![
                config_option(vec![key("graphics"), key("enabled")], json!(true)),
                config_option(vec![key("mods"), index(0), key("name")], json!("example")),
            ],
        })
        .await
        .unwrap();

        let document = tokio::fs::read_to_string(path)
            .await
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(document["graphics"]["enabled"].as_bool(), Some(true));
        assert_eq!(document["mods"][0]["name"].as_str(), Some("example"));
    }

    #[tokio::test]
    async fn config_option_task_updates_properties() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.properties");
        tokio::fs::write(&path, "enabled=false\n# keep comment\n")
            .await
            .unwrap();

        run_config_option_task(&ConfigOptionTask {
            path: path.clone(),
            config_type: ConfigType::Properties,
            options: vec![
                config_option(vec![key("enabled")], json!(true)),
                config_option(vec![key("max-count")], json!(5)),
            ],
        })
        .await
        .unwrap();

        let contents = tokio::fs::read_to_string(path).await.unwrap();
        assert!(contents.contains("enabled=true\n"));
        assert!(contents.contains("# keep comment\n"));
        assert!(contents.contains("max-count=5\n"));
    }

    #[test]
    fn resolves_existing_remote_instance_for_update() {
        let url = Url::parse("https://example.com/manifest.json").unwrap();
        let local = LocalInstance::new_remote(
            remote_entry_id(&url, "Vanilla"),
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

        let plan =
            resolve_install_plan(&local.id, std::slice::from_ref(&local), &catalogs).unwrap();

        assert_eq!(plan.view_id, local.id);
        assert_eq!(plan.dir_name, "Vanilla");
        assert!(plan.existing.is_some());
    }
}
