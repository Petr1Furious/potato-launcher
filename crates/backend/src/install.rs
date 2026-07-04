use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::Arc,
};

use anyhow::anyhow;
use either::Either;
use instance::{
    instance_metadata::{InstallCause, InstallParams, InstanceMetadata, ModSyncWarning, TaskSet},
    manifest::InstanceManifestEntry,
    mod_sync,
    storage::{self, InstanceHandle, InstanceState, LocalInstance, RemoteSource},
};
use launcher_bridge::{
    FrontendSender, MessageToFrontend, NotificationLevel, ProgressUnit, TaskKind,
};
use tokio::sync::mpsc;
use url::Url;
use utils::{
    adaptive_download,
    files::{self, ConfigOptionTask, ConfigType},
    java,
    paths::{DataDir, InstanceDirFS, InstancesDir, NativesDir},
    progress::ProgressTracker,
};

use launcher_auth::{AccountData, providers::AuthProviderConfig};

use crate::{
    BackendEvent,
    catalog::BackendCatalogEntry,
    instances::remote_entry_handle,
    launch::{InstanceAuthMessages, authenticate_account},
    tasks::{InstanceTaskList, TaskHandle},
};

#[derive(Clone)]
pub(crate) struct InstallRequest {
    pub(crate) handle: InstanceHandle,

    pub(crate) cause: InstallCause,
    pub(crate) force_overwrite: bool,
    pub(crate) optional_mod_preferences: HashMap<String, bool>,

    pub(crate) launcher_dir: DataDir,
    pub(crate) client: reqwest::Client,
    pub(crate) local_instances: Vec<LocalInstance>,
    pub(crate) catalogs: HashMap<Url, BackendCatalogEntry>,
    pub(crate) frontend: FrontendSender,
    pub(crate) internal: mpsc::UnboundedSender<BackendEvent>,
    pub(crate) java_path: Option<String>,
    pub(crate) tasks: InstanceTaskList,
    /// When set, the account is validated/refreshed before any file sync
    /// starts (instances that declare a required auth provider).
    pub(crate) auth: Option<(AuthProviderConfig, AccountData)>,
}

#[derive(Clone, Debug)]
pub(crate) struct InstallOutput {
    pub(crate) instance: LocalInstance,
    pub(crate) java: java::JavaInstallation,
}

#[derive(Clone)]
struct InstallPlan {
    view_handle: InstanceHandle,
    dir_name: String,
    source: RemoteSource,
    entry: InstanceManifestEntry,
    existing: Option<LocalInstance>,
}

fn install_action_label(cause: InstallCause, force_overwrite: bool) -> &'static str {
    match (cause, force_overwrite) {
        (InstallCause::Run, _) => "launch",
        (InstallCause::Update, true) => "force sync",
        (InstallCause::Update, false) => "sync",
    }
}

fn instance_dir_name(instance_dir: &InstanceDirFS) -> String {
    instance_dir
        .to_fs()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

fn log_install_task_set(action: &str, instance: &str, tasks: &TaskSet) {
    log::info!(
        "{action} for instance '{instance}': {} check, {} delete, {} config, {} optional-mod task(s)",
        tasks.check_tasks.len(),
        tasks.delete_tasks.len(),
        tasks.config_option_tasks.len(),
        tasks.enable_optional_mod_tasks.len()
    );
    files::log_check_tasks(action, instance, &tasks.check_tasks);
}

pub(crate) async fn install_instance(request: InstallRequest) -> anyhow::Result<InstallOutput> {
    let local_only = request
        .local_instances
        .iter()
        .find(|instance| instance.handle == request.handle && instance.source.is_none())
        .cloned();
    if let Some(local) = local_only {
        return install_local_only_instance(request, local).await;
    }

    let plan = resolve_install_plan(&request.handle, &request.local_instances, &request.catalogs)?;
    let action = install_action_label(request.cause, request.force_overwrite);
    log::info!(
        "Starting {action} for instance '{}' (handle {})",
        plan.dir_name,
        request.handle
    );
    let instance_dir = InstancesDir::root()
        .instance_dir(&plan.dir_name)
        .with_data_dir(request.launcher_dir.clone());
    instance_dir.ensure_dir();

    let internal = request.internal.clone();
    let tasks = request.tasks.clone();

    authenticate_install_account(&request).await?;

    let metadata = install_metadata(
        &request.client,
        &plan.entry,
        &instance_dir,
        tasks.task(
            TaskKind::Metadata,
            ProgressUnit::Items,
            launcher_i18n::progress::downloading_metadata(),
        ),
    )
    .await?;

    let previous_metadata = InstanceMetadata::read_local(&instance_dir).await.ok();
    let previous_mod_entries = previous_metadata
        .as_ref()
        .map(|metadata| metadata.mod_entries.clone())
        .unwrap_or_default();
    let previous_content_rules = previous_metadata
        .as_ref()
        .map(|metadata| metadata.content_rules.clone())
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
        previous_content_rules,
        optional_sets_enabled,
    };
    // file sync and Java resolution are independent once metadata is known
    let (files_result, java_result) = tokio::join!(
        install_game_files(
            &request.client,
            &metadata,
            &install_params,
            &tasks,
            &request.frontend,
        ),
        resolve_java(
            &request.client,
            &metadata,
            &request.launcher_dir,
            request.java_path.as_deref(),
            &tasks,
        ),
    );
    files_result?;
    let java = java_result?;
    persist_java_installation(request.handle.clone(), &java, &internal);

    // it is important to save metadata after install_game_files
    // because mod_sync looks at the delta between old and new metadata
    metadata.save(&instance_dir).await?;

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
            plan.view_handle,
            plan.dir_name,
            plan.source,
            Some(plan.entry.sha1),
        )
    };

    Ok(InstallOutput { instance, java })
}

async fn authenticate_install_account(request: &InstallRequest) -> anyhow::Result<()> {
    let Some((provider, account_data)) = request.auth.clone() else {
        return Ok(());
    };
    let auth_task = request.tasks.task(
        TaskKind::Auth,
        ProgressUnit::Items,
        launcher_i18n::progress::authenticating(),
    );
    let messages = Arc::new(InstanceAuthMessages::new(
        request.frontend.clone(),
        request.handle.clone(),
        request.internal.clone(),
    ));
    let authenticated = authenticate_account(
        &request.client,
        provider,
        account_data,
        messages,
        &request.frontend,
    )
    .await?;
    if let Some(refreshed) = authenticated.refreshed {
        let _ = request.internal.send(BackendEvent::AccountUpdated {
            provider: authenticated.provider,
            account: refreshed,
        });
    }
    auth_task.finish();
    Ok(())
}

async fn install_local_only_instance(
    request: InstallRequest,
    local: LocalInstance,
) -> anyhow::Result<InstallOutput> {
    if request.cause != InstallCause::Run {
        return Err(anyhow::anyhow!(
            "local-only instance cannot be updated from a backend"
        ));
    }
    if !local.is_installed() {
        return Err(anyhow!(
            "attempting to run an instance that is not installed"
        ));
    }

    let action = install_action_label(request.cause, request.force_overwrite);
    log::info!(
        "Starting {action} for instance '{}' (handle {})",
        local.dir_name,
        local.handle
    );

    let instance_dir = InstancesDir::root()
        .instance_dir(&local.dir_name)
        .with_data_dir(request.launcher_dir.clone());

    let internal = request.internal.clone();
    let tasks = request.tasks.clone();

    authenticate_install_account(&request).await?;

    let metadata = InstanceMetadata::read_local(&instance_dir)
        .await
        .map_err(|err| anyhow!("failed to read local instance metadata: {err}"))?;
    let previous_mod_entries = metadata.mod_entries.clone();
    let previous_content_rules = metadata.content_rules.clone();
    let optional_sets_enabled = mod_sync::resolve_optional_set_enabled(
        &metadata.mod_sync,
        &request.optional_mod_preferences,
    );
    let install_params = InstallParams {
        instance_dir: instance_dir.clone(),
        cause: InstallCause::Run,
        force_overwrite: request.force_overwrite,
        previous_mod_entries,
        previous_content_rules,
        optional_sets_enabled,
    };

    let (files_result, java_result) = tokio::join!(
        install_game_files(
            &request.client,
            &metadata,
            &install_params,
            &tasks,
            &request.frontend,
        ),
        resolve_java(
            &request.client,
            &metadata,
            &request.launcher_dir,
            request.java_path.as_deref(),
            &tasks,
        ),
    );
    files_result?;
    let java = java_result?;
    persist_java_installation(local.handle.clone(), &java, &internal);
    metadata.save(&instance_dir).await?;

    Ok(InstallOutput {
        instance: local,
        java,
    })
}

fn resolve_install_plan(
    handle: &InstanceHandle,
    local_instances: &[LocalInstance],
    catalogs: &HashMap<Url, BackendCatalogEntry>,
) -> anyhow::Result<InstallPlan> {
    if let Some(local) = local_instances
        .iter()
        .find(|instance| &instance.handle == handle)
    {
        return resolve_local_install_plan(local.clone(), catalogs);
    }

    for (url, state) in catalogs {
        let Some(manifest) = state.manifest() else {
            continue;
        };
        for entry in &manifest.instances {
            if remote_entry_handle(url, &entry.id) == *handle {
                let dir_name = allocate_dir_name(local_instances, &entry.id).map_err(|err| {
                    anyhow::anyhow!(
                        "invalid instance id '{}' in catalog {}: {err}",
                        entry.id,
                        url
                    )
                })?;
                return Ok(InstallPlan {
                    view_handle: handle.clone(),
                    dir_name,
                    source: RemoteSource {
                        manifest_url: url.clone(),
                        id_in_manifest: entry.id.clone(),
                    },
                    entry: entry.clone(),
                    existing: None,
                });
            }
        }
    }

    Err(anyhow::anyhow!(
        "instance {handle} was not found in local storage or fetched catalogs"
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
        .find(|entry| entry.id == source.id_in_manifest)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("instance is no longer published by its backend"))?;

    Ok(InstallPlan {
        view_handle: local.handle.clone(),
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
    progress: TaskHandle,
) -> anyhow::Result<InstanceMetadata> {
    progress.set_length(1);
    // do not save metadata to disk yet
    // save only after install_game_files
    let metadata = InstanceMetadata::read_or_fetch(client, entry, instance_dir).await?;
    progress.inc(1);
    progress.finish();
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
    tasks: &InstanceTaskList,
    frontend: &FrontendSender,
) -> anyhow::Result<()> {
    let action = install_action_label(params.cause, params.force_overwrite);
    let instance = instance_dir_name(&params.instance_dir);
    let check_progress = tasks.task(
        TaskKind::CheckFiles,
        ProgressUnit::Items,
        launcher_i18n::progress::collecting_install_tasks(),
    );
    let install_tasks = metadata.get_all_install_tasks(client, params).await?;

    if !install_tasks.mod_warnings.is_empty() {
        log::info!(
            "{action} for instance '{instance}': {} mod sync warning(s)",
            install_tasks.mod_warnings.len()
        );
    }
    log_install_task_set(action, &instance, &install_tasks.tasks);

    for warning in &install_tasks.mod_warnings {
        notify_mod_sync_warning(frontend, warning);
    }

    for delete_task in &install_tasks.tasks.delete_tasks {
        files::remove_file_or_dir(&delete_task.path).await?;
    }

    let check_count = install_tasks.tasks.check_tasks.len();
    check_progress.set_message(launcher_i18n::progress::checking_install_files());
    let download_tasks =
        files::get_download_tasks(install_tasks.tasks.check_tasks, check_progress.clone()).await?;
    files::log_download_tasks(action, &instance, check_count, &download_tasks);
    check_progress.finish();

    let downloaded_paths = download_tasks
        .iter()
        .map(|task| task.path.clone())
        .collect::<HashSet<_>>();
    if !download_tasks.is_empty() {
        let unit = match files::total_download_size(&download_tasks) {
            Some(_) => ProgressUnit::Bytes,
            None => ProgressUnit::Items,
        };
        let download_progress = tasks.task(
            TaskKind::Download,
            unit,
            launcher_i18n::progress::downloading_install_files(),
        );
        adaptive_download::download_files(
            download_tasks,
            &params.instance_dir.data_dir().tmp_dir(),
            download_progress.clone(),
        )
        .await?;
        download_progress.finish();
    }

    enable_optional_mods(install_tasks.tasks.enable_optional_mod_tasks).await?;

    for config_task in &install_tasks.tasks.config_option_tasks {
        run_config_option_task(config_task).await?;
    }

    metadata
        .mark_include_downloads_complete(&params.instance_dir.minecraft_dir())
        .await?;

    extract_natives_if_needed(
        metadata,
        params.instance_dir.data_dir(),
        &downloaded_paths,
        params.force_overwrite,
        tasks,
    )
    .await?;

    Ok(())
}

async fn link_optional_mod(source: &Path, target: &Path) -> io::Result<()> {
    if !source.is_file() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    files::remove_file_or_dir(target).await?;
    if let Err(err) = tokio::fs::hard_link(source, target).await {
        log::error!(
            "Failed to hardlink optional mod from {} to {}; falling back to copy: {err}",
            source.display(),
            target.display()
        );
        tokio::fs::copy(source, target).await?;
    }
    Ok(())
}

async fn unlink_optional_mod(target: &Path) -> io::Result<()> {
    if tokio::fs::try_exists(target).await.unwrap_or(false) {
        files::remove_file_or_dir(target).await?;
    }
    Ok(())
}

async fn enable_optional_mods(
    tasks: Vec<instance::instance_metadata::EnableOptionalModTask>,
) -> io::Result<()> {
    for task in tasks {
        link_optional_mod(&task.source, &task.target).await?;
    }
    Ok(())
}

pub(crate) async fn apply_optional_mod_set(
    instance_dir: &InstanceDirFS,
    metadata: &InstanceMetadata,
    set_id: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    let Some(set) = metadata
        .mod_sync
        .optional_sets
        .iter()
        .find(|entry| entry.id == set_id)
    else {
        return Ok(());
    };
    let mod_ids = set.mod_ids.iter().cloned().collect::<HashSet<_>>();

    for entry in &metadata.mod_entries {
        if !mod_ids.contains(&entry.mod_id) {
            continue;
        }
        let filename =
            entry.object.path.file_name().ok_or_else(|| {
                anyhow!("optional mod entry has no jar filename: {}", entry.mod_id)
            })?;
        let source = instance_dir.optional_mods_dir().join(filename);
        let target = entry.object.path.to_path(instance_dir.minecraft_dir());
        if enabled {
            link_optional_mod(&source, &target).await?;
        } else {
            unlink_optional_mod(&target).await?;
        }
    }

    Ok(())
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

pub(crate) fn persist_java_installation(
    instance: InstanceHandle,
    installation: &java::JavaInstallation,
    internal: &mpsc::UnboundedSender<BackendEvent>,
) {
    let _ = internal.send(BackendEvent::JavaResolved {
        instance,
        path: Some(Arc::from(installation.path.to_string_lossy().as_ref())),
    });
}

pub(crate) async fn resolve_java(
    client: &reqwest::Client,
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    stored_path: Option<&str>,
    tasks: &InstanceTaskList,
) -> anyhow::Result<java::JavaInstallation> {
    let java_version = metadata.get_java_version();
    let java_progress = tasks.task(
        TaskKind::Java,
        ProgressUnit::Bytes,
        launcher_i18n::progress::checking_java(),
    );

    if let Some(path) = stored_path.filter(|path| !path.is_empty()) {
        let java_path = Path::new(path);
        if java::check_java(&java_version, java_path).await
            && let Some(installation) = java::get_installation_pub(java_path).await
        {
            java_progress.set_message(launcher_i18n::progress::java_already_installed());
            java_progress.finish();
            return Ok(installation);
        }
        log::warn!(
            "Stored Java path {path} is invalid for Java {java_version}; searching for a replacement"
        );
    }
    if let Some(installation) = java::get_java(&java_version, data_dir).await {
        java_progress.set_message(launcher_i18n::progress::java_already_installed());
        java_progress.finish();
        return Ok(installation);
    }

    java_progress.set_message(launcher_i18n::progress::installing_java_version(
        java_version.clone(),
    ));
    let platform = utils::java::current_platform();
    let unpacking_message = launcher_i18n::progress::unpacking_java();
    if let Some(runtime) = metadata.find_java_runtime(&java_version, &platform.os, &platform.arch) {
        java::download_java_from_runtime(
            client,
            &runtime.url,
            &runtime.archive_type,
            &runtime.name,
            &java_version,
            data_dir,
            java_progress.clone(),
            unpacking_message,
        )
        .await?;
    } else {
        java::download_java(
            client,
            &java_version,
            data_dir,
            java_progress.clone(),
            Some(unpacking_message),
        )
        .await?;
    }
    java_progress.finish();
    java::get_java(&java_version, data_dir)
        .await
        .ok_or_else(|| anyhow::anyhow!("Java {java_version} is still missing after download"))
}

const NATIVES_EXTRACTED_MARKER: &str = ".extracted";

fn natives_fingerprint(native_paths: &[std::path::PathBuf]) -> String {
    let mut lines = native_paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    lines.sort();
    lines.join("\n")
}

async fn extract_natives_if_needed(
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    downloaded_paths: &HashSet<std::path::PathBuf>,
    force_overwrite: bool,
    tasks: &InstanceTaskList,
) -> anyhow::Result<()> {
    let native_paths = metadata.get_native_library_paths(data_dir)?;
    let natives_dir = NativesDir::for_id(metadata.get_parent_version_id()?).to_fs(data_dir);
    let marker_path = natives_dir.join(NATIVES_EXTRACTED_MARKER);
    let fingerprint = natives_fingerprint(&native_paths);

    let jars_changed = native_paths
        .iter()
        .any(|path| downloaded_paths.contains(path));
    let marker_matches = matches!(
        tokio::fs::read_to_string(&marker_path).await,
        Ok(existing) if existing == fingerprint
    );
    if !force_overwrite && !jars_changed && marker_matches {
        log::debug!(
            "Skipping natives extraction for {}: up to date",
            natives_dir.display()
        );
        return Ok(());
    }

    let progress = tasks.task(
        TaskKind::Extract,
        ProgressUnit::Items,
        launcher_i18n::progress::extracting_native_libraries(),
    );
    progress.set_length(native_paths.len() as u64);
    if natives_dir.exists() {
        tokio::fs::remove_dir_all(&natives_dir).await?;
    }
    tokio::fs::create_dir_all(&natives_dir).await?;

    for native_path in &native_paths {
        files::extract_zip(native_path, &natives_dir, true).await?;
        progress.inc(1);
    }

    // marker last: an interrupted extraction must not look complete
    tokio::fs::write(&marker_path, fingerprint).await?;
    progress.finish();
    Ok(())
}

fn allocate_dir_name(
    local_instances: &[LocalInstance],
    base: &str,
) -> Result<String, storage::InstanceIdError> {
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
    use serde_json::json;
    use utils::files::ConfigOption;

    use crate::catalog::{BackendCatalogEntry, BackendFetchStatus};

    fn ok_catalog(manifest: InstanceManifest) -> BackendCatalogEntry {
        BackendCatalogEntry::with_manifest(manifest, BackendFetchStatus::Ok)
    }

    #[test]
    fn resolves_remote_install_plan_from_fetched_catalog() {
        let url = Url::parse("https://example.com/data/instance_manifest.json").unwrap();
        let entry = InstanceManifestEntry {
            id: "vanilla".to_string(),
            display_name: None,
            url: Url::parse("https://example.com/data/instances/vanilla/meta.json").unwrap(),
            sha1: "abc".to_string(),
            auth_backend: None,
            required_java_version: "8".to_string(),
        };
        let id = remote_entry_handle(&url, &entry.id);
        let catalogs = HashMap::from([(
            url.clone(),
            ok_catalog(InstanceManifest {
                instances: vec![entry],
            }),
        )]);

        let plan = resolve_install_plan(&id, &[], &catalogs).unwrap();

        assert_eq!(plan.view_handle, id);
        assert_eq!(plan.dir_name, "vanilla");
        assert_eq!(plan.source.manifest_url, url);
        assert_eq!(plan.source.id_in_manifest, "vanilla");
    }

    #[test]
    fn allocates_distinct_directory_names_for_duplicate_display_names() {
        let local_instances = vec![
            LocalInstance::new_local("vanilla".to_string()),
            LocalInstance::new_local("vanilla-2".to_string()),
        ];

        assert_eq!(
            allocate_dir_name(&local_instances, "vanilla").unwrap(),
            "vanilla-3"
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
        let url = Url::parse("https://example.com/data/instance_manifest.json").unwrap();
        let local = LocalInstance::new_remote(
            remote_entry_handle(&url, "vanilla"),
            "vanilla".to_string(),
            RemoteSource {
                manifest_url: url.clone(),
                id_in_manifest: "vanilla".to_string(),
            },
            Some("old".to_string()),
        );
        let entry = InstanceManifestEntry {
            id: "vanilla".to_string(),
            display_name: None,
            url: Url::parse("https://example.com/data/instances/vanilla/meta.json").unwrap(),
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
            resolve_install_plan(&local.handle, std::slice::from_ref(&local), &catalogs).unwrap();

        assert_eq!(plan.view_handle, local.handle);
        assert_eq!(plan.dir_name, "vanilla");
        assert!(plan.existing.is_some());
    }
}
