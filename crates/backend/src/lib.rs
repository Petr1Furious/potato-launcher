mod catalog;
mod install;
pub mod instances;
mod launch;
mod local;
mod tasks;
mod update;
mod versions;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use catalog::{
    BackendCatalogEntry, CatalogFetchResult, backend_status, delete_cached_manifest,
    fetch_backend_catalog, load_cached_manifest, save_cached_manifest,
};
use instance::{
    install_params::InstallCause,
    instance_metadata::InstanceMetadata,
    storage::{
        InstanceHandle, InstanceStorage, InstanceUserSettings, LocalInstance, RemoteSource,
        load_instance_settings, save_instance_settings,
    },
};
use launcher_auth::{
    AccountData,
    flow::{AuthMessage, AuthMessageProvider, perform_auth},
    providers::AuthProviderConfig,
    storage::{AccountKey, AuthStorage},
};
use launcher_bridge::{
    AccountView, AuthPromptContext, BackendReceiver, BackendStatus, FrontendSender,
    InstanceTaskView, LauncherSettingsView, MessageToBackend, MessageToFrontend, NotificationLevel,
};
use launcher_build_config::default_instance_manifest_urls;
use launcher_i18n::{detect_system_language_code, resolve_language_code, set_lang};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use utils::{
    files,
    paths::{DataDir, InstanceDirFS, InstancesDir},
};

const SETTINGS_FILE: &str = "settings.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub backend_urls: Vec<Url>,
    #[serde(default)]
    pub hide_window_after_launch: bool,
    #[serde(default)]
    pub hide_usernames_in_cards: bool,
    #[serde(default)]
    pub language: Option<String>,
}

impl Settings {
    fn defaults() -> Self {
        Self {
            backend_urls: default_instance_manifest_urls(),
            hide_window_after_launch: false,
            hide_usernames_in_cards: false,
            language: None,
        }
    }

    async fn load(launcher_dir: &Path) -> anyhow::Result<(Self, bool)> {
        let path = launcher_dir.join(SETTINGS_FILE);
        let (mut settings, reset_from_corruption) =
            match files::read_file_parsed::<Self>(&path).await {
                Ok(settings) => (settings, false),
                Err(files::ReadFileParsedError::Io(err))
                    if err.kind() == std::io::ErrorKind::NotFound =>
                {
                    (Self::defaults(), false)
                }
                Err(err) => {
                    log::warn!(
                        "Corrupt launcher settings {}: {err}; resetting",
                        path.display()
                    );
                    if let Err(backup_err) = files::backup_corrupt_file(&path).await {
                        log::warn!("Failed to back up corrupt settings file: {backup_err}");
                    }
                    (Self::defaults(), true)
                }
            };

        let language_resolved = settings.ensure_language_resolved().await?;
        if language_resolved || !path.exists() {
            settings.save(launcher_dir).await?;
        }

        Ok((settings, reset_from_corruption))
    }

    async fn ensure_language_resolved(&mut self) -> anyhow::Result<bool> {
        if self.language.is_some() {
            set_lang(self.resolved_language_code());
            return Ok(false);
        }
        let resolved = detect_system_language_code().to_string();
        self.language = Some(resolved);
        set_lang(self.resolved_language_code());
        Ok(true)
    }

    fn resolved_language_code(&self) -> &str {
        resolve_language_code(self.language.as_deref(), None)
    }

    async fn save(&self, launcher_dir: &Path) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        files::write_file_atomic(&launcher_dir.join(SETTINGS_FILE), &bytes).await?;
        Ok(())
    }
}

pub struct BackendState {
    launcher_dir: PathBuf,
    settings: Settings,
    instance_storage: InstanceStorage,
    auth_storage: AuthStorage,
    catalogs: HashMap<Url, BackendCatalogEntry>,
    client: reqwest::Client,
    activities: HashMap<InstanceHandle, Activity>,
    creating_local: HashMap<InstanceHandle, Arc<str>>,
    creating_local_params: HashMap<InstanceHandle, local::CreateLocalParams>,
    install_errors: HashMap<InstanceHandle, Arc<str>>,
    launch_errors: HashMap<InstanceHandle, Arc<str>>,
    launch_after_install: HashMap<InstanceHandle, bool>,
    add_account_cancel: Option<CancellationToken>,
    add_account_task: Option<JoinHandle<()>>,
    auth_instance: Option<InstanceHandle>,
    startup_notices: Vec<(NotificationLevel, Arc<str>)>,
}

struct Activity {
    kind: instances::ActivityKind,
    tasks: Arc<[InstanceTaskView]>,
    join: JoinHandle<()>,
    kill: Option<oneshot::Sender<()>>,
}

impl Activity {
    fn new(
        kind: instances::ActivityKind,
        tasks: Arc<[InstanceTaskView]>,
        join: JoinHandle<()>,
    ) -> Self {
        Self {
            kind,
            tasks,
            join,
            kill: None,
        }
    }
}

enum BackendEvent {
    FetchFinished {
        url: Url,
        result: CatalogFetchResult,
    },
    InstanceTasks {
        handle: InstanceHandle,
        tasks: Arc<[InstanceTaskView]>,
    },
    InstallFinished {
        handle: InstanceHandle,
        is_run: bool,
        result: Result<install::InstallOutput, Arc<str>>,
    },
    LaunchStarted {
        handle: InstanceHandle,
    },
    AccountUpdated {
        provider: AuthProviderConfig,
        account: AccountData,
    },
    LaunchFinished {
        handle: InstanceHandle,
        exit: launcher_bridge::ExitOutcome,
    },
    AddAccountFinished {
        result: Result<(AuthProviderConfig, AccountData), Arc<str>>,
    },
    AddAccountCancelled,
    AuthLaunchPrompt {
        instance: Option<InstanceHandle>,
    },
    JavaResolved {
        instance: InstanceHandle,
        path: Option<Arc<str>>,
    },
}

struct AuthPromptReporter {
    frontend: FrontendSender,
    offline_nickname: Mutex<String>,
    message: Mutex<Option<AuthMessage>>,
}

impl AuthPromptReporter {
    fn new(frontend: FrontendSender) -> Self {
        Self {
            frontend,
            offline_nickname: Mutex::new("Player".to_string()),
            message: Mutex::new(None),
        }
    }
}

#[async_trait]
impl AuthMessageProvider for AuthPromptReporter {
    async fn set_message(&self, message: AuthMessage) {
        if let Ok(mut stored) = self.message.lock() {
            *stored = Some(message.clone());
        }
        self.frontend.send(MessageToFrontend::AuthPrompt {
            context: AuthPromptContext::AddAccount,
            message,
        });
    }

    async fn get_message(&self) -> Option<AuthMessage> {
        self.message.lock().ok().and_then(|message| message.clone())
    }

    async fn clear(&self) {
        if let Ok(mut message) = self.message.lock() {
            *message = None;
        }
        self.frontend.send(MessageToFrontend::AuthPromptCleared);
    }

    async fn request_offline_nickname(&self) -> String {
        self.offline_nickname
            .lock()
            .map(|nickname| nickname.clone())
            .unwrap_or_else(|_| "Player".to_string())
    }

    async fn need_offline_nickname(&self) -> bool {
        false
    }

    async fn set_offline_nickname(&self, nickname: String) {
        if let Ok(mut stored) = self.offline_nickname.lock() {
            *stored = nickname;
        }
    }
}

impl BackendState {
    async fn load(launcher_dir: PathBuf) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&launcher_dir).await?;
        let data_dir = DataDir::new(launcher_dir.clone());

        let tmp_dir = data_dir.tmp_dir();
        tokio::spawn(async move {
            match tokio::fs::remove_dir_all(&tmp_dir).await {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    log::warn!("Failed to clear temp dir {}: {err:#}", tmp_dir.display());
                }
            }
        });
        let mut startup_notices: Vec<(NotificationLevel, Arc<str>)> = Vec::new();
        let (settings, settings_reset) = Settings::load(&launcher_dir).await?;
        if settings_reset {
            startup_notices.push((
                NotificationLevel::Warning,
                Arc::from(launcher_i18n::notifications::settings_reset_from_corruption()),
            ));
        }

        let mut catalogs = HashMap::new();
        for url in &settings.backend_urls {
            let entry = match load_cached_manifest(&launcher_dir, url).await {
                Ok(manifest) => {
                    log::info!(
                        "Loaded cached backend manifest from {url}: {} published instances",
                        manifest.instances.len()
                    );
                    BackendCatalogEntry::from_cache(Arc::new(manifest))
                }
                Err(err) => {
                    log::warn!("Failed to load cached backend manifest for {url}: {err:#}");
                    BackendCatalogEntry::new_not_fetched()
                }
            };
            catalogs.insert(url.clone(), entry);
        }

        let instance_storage = match InstanceStorage::load(&data_dir).await {
            Ok(loaded) => {
                if !loaded.recovered.is_empty() {
                    startup_notices.push((
                        NotificationLevel::Warning,
                        Arc::from(launcher_i18n::notifications::instances_recovered(
                            loaded.recovered.len() as i64,
                        )),
                    ));
                }
                loaded.storage
            }
            Err(err) => {
                log::warn!("Failed to load local instance storage: {err:?}");
                startup_notices.push((
                    NotificationLevel::Warning,
                    Arc::from(launcher_i18n::notifications::instances_load_failed()),
                ));
                InstanceStorage::empty()
            }
        };

        let auth_path = launcher_dir.join("auth_data.json");
        let auth_storage = match AuthStorage::load(auth_path.clone()).await {
            Ok(storage) => storage,
            Err(err) => {
                log::warn!("Failed to load saved accounts: {err}");

                if let Err(backup_err) = files::backup_corrupt_file(&auth_path).await {
                    log::warn!("Failed to back up corrupt auth storage: {backup_err}");
                }
                startup_notices.push((
                    NotificationLevel::Warning,
                    Arc::from(launcher_i18n::notifications::accounts_reset_from_corruption()),
                ));
                AuthStorage::empty(auth_path)
            }
        };

        Ok(Self {
            launcher_dir,
            settings,
            instance_storage,
            auth_storage,
            catalogs,
            client: reqwest::Client::new(),
            activities: HashMap::new(),
            creating_local: HashMap::new(),
            creating_local_params: HashMap::new(),
            install_errors: HashMap::new(),
            launch_errors: HashMap::new(),
            launch_after_install: HashMap::new(),
            add_account_cancel: None,
            add_account_task: None,
            auth_instance: None,
            startup_notices,
        })
    }

    fn activity_snapshots(&self) -> instances::ActivityMap {
        self.activities
            .iter()
            .map(|(handle, activity)| {
                (
                    handle.clone(),
                    instances::ActivitySnapshot {
                        kind: activity.kind,
                        tasks: activity.tasks.clone(),
                    },
                )
            })
            .collect()
    }

    fn activity_kind(&self, handle: &InstanceHandle) -> Option<instances::ActivityKind> {
        self.activities.get(handle).map(|activity| activity.kind)
    }

    fn is_busy(&self, handle: &InstanceHandle) -> bool {
        self.activities.contains_key(handle)
    }

    fn backend_statuses(&self) -> Arc<[BackendStatus]> {
        self.visible_backend_urls()
            .into_iter()
            .map(|(url, configured, referenced_by_instances)| {
                let entry = self
                    .catalogs
                    .get(&url)
                    .cloned()
                    .unwrap_or_else(BackendCatalogEntry::new_not_fetched);
                backend_status(&url, &entry, configured, referenced_by_instances)
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn visible_backend_urls(&self) -> Vec<(Url, bool, bool)> {
        let configured = self
            .settings
            .backend_urls
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let referenced = self
            .instance_storage
            .iter()
            .filter_map(|instance| instance.source.as_ref())
            .map(|source| source.manifest_url.clone())
            .collect::<HashSet<_>>();

        let mut urls = self.settings.backend_urls.clone();
        for url in &referenced {
            if !urls.iter().any(|existing| existing == url) {
                urls.push(url.clone());
            }
        }

        urls.into_iter()
            .map(|url| {
                let is_configured = configured.contains(&url);
                let is_referenced = referenced.contains(&url);
                (url, is_configured, is_referenced)
            })
            .collect()
    }

    fn account_views(&self) -> Arc<[AccountView]> {
        self.auth_storage
            .accounts()
            .filter_map(|entry| {
                let provider = self.auth_storage.get_provider(entry.provider_id)?.clone();
                Some((
                    (
                        entry.provider_id,
                        entry.auth_data.user_info.username.clone(),
                    ),
                    provider,
                    entry.auth_data.clone(),
                ))
            })
            .enumerate()
            .map(|(index, (key, provider, data))| AccountView {
                key,
                provider,
                data,
                selected: index == 0,
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn launch_accounts(&self) -> Vec<(AccountKey, AuthProviderConfig, AccountData)> {
        let mut accounts =
            launch::stored_accounts(self.auth_storage.accounts().filter_map(|entry| {
                let provider = self.auth_storage.get_provider(entry.provider_id)?.clone();
                Some((entry.clone(), provider))
            }));
        if accounts.is_empty() {
            accounts.push(launch::default_offline_account());
        }
        accounts
    }

    fn build_instance_views(&self) -> Arc<[launcher_bridge::InstanceView]> {
        let local_metadata = self.local_metadata_views();
        let account_views = self.account_views();
        let instance_settings = self.instance_settings_views();
        let activities = self.activity_snapshots();
        instances::build_instance_views(&instances::InstanceViewBuildInput {
            language: self.settings.resolved_language_code(),
            local_instances: self.instance_storage.all(),
            catalogs: &self.catalogs,
            live_state: instances::InstanceLiveState {
                activities: &activities,
                creating_local: &self.creating_local,
                install_errors: &self.install_errors,
                launch_errors: &self.launch_errors,
            },
            local_metadata: &local_metadata,
            user_settings: &instance_settings,
            accounts: &account_views,
            launch_after_install: &self.launch_after_install,
        })
        .into()
    }

    fn instance_settings_views(
        &self,
    ) -> HashMap<InstanceHandle, instances::InstanceUserSettingsView> {
        let data_dir = DataDir::new(self.launcher_dir.clone());
        self.instance_storage
            .iter()
            .map(|local| {
                let instance_dir = InstancesDir::root()
                    .instance_dir(&local.dir_name)
                    .with_data_dir(data_dir.clone());
                let settings_path = instance_dir.settings_path();
                let settings = match std::fs::read(&settings_path) {
                    Ok(bytes) => serde_json::from_slice::<InstanceUserSettings>(&bytes)
                        .unwrap_or_else(|err| {
                            log::warn!(
                                "Failed to parse instance settings {}: {err:#}",
                                settings_path.display()
                            );
                            InstanceUserSettings::default()
                        }),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        InstanceUserSettings::default()
                    }
                    Err(err) => {
                        log::warn!(
                            "Failed to read instance settings {}: {err:#}",
                            settings_path.display()
                        );
                        InstanceUserSettings::default()
                    }
                };
                (
                    local.handle.clone(),
                    instances::InstanceUserSettingsView {
                        selected_account: settings.selected_account.clone(),
                        account_override: settings.account_override.clone(),
                        xmx_mb: settings.xmx_mb,
                        jvm_flags: settings
                            .jvm_flags
                            .as_ref()
                            .map(|flags| Arc::<str>::from(flags.clone())),
                        java_path: settings
                            .java_path
                            .as_ref()
                            .map(|p| Arc::<str>::from(p.clone())),
                        use_native_glfw: settings.use_native_glfw,
                        optional_mod_sets: settings.optional_mod_sets.clone(),
                    },
                )
            })
            .collect()
    }

    fn local_metadata_views(&self) -> HashMap<InstanceHandle, instances::LocalMetadataView> {
        let data_dir = DataDir::new(self.launcher_dir.clone());
        self.instance_storage
            .iter()
            .filter_map(|local| {
                if !local.is_installed() {
                    return None;
                }
                let path = InstancesDir::root()
                    .instance_dir(&local.dir_name)
                    .meta_path()
                    .to_fs(&data_dir);
                let bytes = std::fs::read(path).ok()?;
                let metadata = serde_json::from_slice::<InstanceMetadata>(&bytes).ok()?;
                Some((
                    local.handle.clone(),
                    instances::LocalMetadataView {
                        display_name: metadata.display_name.clone(),
                        auth_provider: metadata.auth_backend.clone(),
                        default_xmx_mb: parse_xmx_mb(metadata.default_xmx.as_deref()),
                        required_java_version: Some(Arc::from(metadata.get_java_version())),
                        mod_sync: metadata.mod_sync.clone(),
                    },
                ))
            })
            .collect()
    }

    fn launcher_settings_view(&self) -> LauncherSettingsView {
        LauncherSettingsView {
            hide_window_after_launch: self.settings.hide_window_after_launch,
            hide_usernames_in_cards: self.settings.hide_usernames_in_cards,
            language: self.settings.resolved_language_code().to_string(),
        }
    }

    fn instance_dir_fs(&self, instance: &LocalInstance) -> InstanceDirFS {
        let data_dir = DataDir::new(self.launcher_dir.clone());
        InstancesDir::root()
            .instance_dir(&instance.dir_name)
            .with_data_dir(data_dir)
    }

    fn load_settings_for_id(&self, handle: &InstanceHandle) -> InstanceUserSettings {
        let Some(instance) = self.instance_storage.get(handle) else {
            return InstanceUserSettings::default();
        };
        let instance_dir = self.instance_dir_fs(instance);
        match std::fs::read(instance_dir.settings_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => InstanceUserSettings::default(),
        }
    }

    fn handle_instance_tasks(
        &mut self,
        handle: InstanceHandle,
        tasks: Arc<[InstanceTaskView]>,
        frontend: &FrontendSender,
    ) {
        let Some(activity) = self.activities.get_mut(&handle) else {
            return;
        };
        activity.tasks = tasks.clone();
        frontend.send(MessageToFrontend::InstanceProgress { handle, tasks });
    }

    fn emit_snapshot(&self, tx: &FrontendSender) {
        tx.send(MessageToFrontend::BackendsUpdated {
            backends: self.backend_statuses(),
        });
        tx.send(MessageToFrontend::SettingsUpdated(
            self.launcher_settings_view(),
        ));
        tx.send(MessageToFrontend::AccountsUpdated(self.account_views()));
        tx.send(MessageToFrontend::InstancesUpdated(
            self.build_instance_views(),
        ));
    }

    async fn add_backend_url(&mut self, url: Url, tx: &FrontendSender) -> anyhow::Result<bool> {
        let inserted = !self
            .settings
            .backend_urls
            .iter()
            .any(|existing| existing == &url);
        if inserted {
            self.settings.backend_urls.push(url.clone());
            self.catalogs
                .insert(url, BackendCatalogEntry::new_not_fetched());
            self.settings.save(&self.launcher_dir).await?;
        }
        self.emit_snapshot(tx);
        Ok(inserted)
    }

    async fn remove_backend_url(&mut self, url: &Url, tx: &FrontendSender) -> anyhow::Result<()> {
        self.settings
            .backend_urls
            .retain(|existing| existing != url);
        if !self
            .instance_storage
            .iter()
            .filter_map(|instance| instance.source.as_ref())
            .any(|source| &source.manifest_url == url)
        {
            self.catalogs.remove(url);
            if let Err(err) = delete_cached_manifest(&self.launcher_dir, url).await {
                log::warn!("Failed to delete cached manifest for {url}: {err:#}");
            }
        }
        self.settings.save(&self.launcher_dir).await?;
        self.emit_snapshot(tx);
        Ok(())
    }

    async fn refresh_all(
        &mut self,
        internal: &mpsc::UnboundedSender<BackendEvent>,
        tx: &FrontendSender,
    ) {
        self.reload_instance_storage(tx).await;
        for (url, _, _) in self.visible_backend_urls() {
            self.start_fetch(url, internal);
        }
        self.emit_snapshot(tx);
    }

    async fn reload_instance_storage(&mut self, tx: &FrontendSender) {
        let data_dir = DataDir::new(self.launcher_dir.clone());
        match InstanceStorage::load(&data_dir).await {
            Ok(loaded) => {
                self.instance_storage = loaded.storage;
                if !loaded.recovered.is_empty() {
                    tx.send(MessageToFrontend::Notification {
                        level: NotificationLevel::Warning,
                        message: Arc::from(launcher_i18n::notifications::instances_recovered(
                            loaded.recovered.len() as i64,
                        )),
                    });
                }
            }
            Err(err) => {
                log::warn!("Failed to reload local instance storage: {err:?}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Warning,
                    message: Arc::from(launcher_i18n::notifications::instances_load_failed()),
                });
            }
        }
    }

    fn start_fetch(&mut self, url: Url, internal: &mpsc::UnboundedSender<BackendEvent>) {
        self.catalogs
            .entry(url.clone())
            .and_modify(BackendCatalogEntry::set_fetching)
            .or_insert_with(|| {
                let mut entry = BackendCatalogEntry::new_not_fetched();
                entry.set_fetching();
                entry
            });
        let client = self.client.clone();
        let internal = internal.clone();
        tokio::spawn(async move {
            let result = fetch_backend_catalog(client, url.clone()).await;
            let _ = internal.send(BackendEvent::FetchFinished { url, result });
        });
    }

    fn handle_fetch_finished(&mut self, url: Url, result: CatalogFetchResult, tx: &FrontendSender) {
        let entry = self
            .catalogs
            .entry(url.clone())
            .or_insert_with(BackendCatalogEntry::new_not_fetched);
        match result {
            CatalogFetchResult::Success(manifest) => {
                let manifest = Arc::new(manifest);
                entry.apply_fetch_success(manifest.clone());
                let launcher_dir = self.launcher_dir.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        save_cached_manifest(&launcher_dir, &url, manifest.as_ref()).await
                    {
                        log::warn!("Failed to save cached backend manifest for {url}: {err:#}");
                    }
                });
            }
            CatalogFetchResult::Failed(failure) => entry.apply_fetch_failure(failure),
        }
        self.emit_snapshot(tx);
    }

    fn start_create_local(
        &mut self,
        display_name: String,
        minecraft_version: String,
        loader: launcher_bridge::LocalLoader,
        loader_version: Option<String>,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        let dir_name = match local::validate_create_local(
            &display_name,
            loader,
            &loader_version,
            &self.instance_storage,
            &self.catalogs,
        ) {
            Ok(dir_name) => dir_name,
            Err(message) => {
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message,
                });
                return;
            }
        };

        let handle = InstanceHandle::local_new();
        self.install_errors.remove(&handle);
        self.launch_after_install.insert(handle.clone(), true);
        self.creating_local
            .insert(handle.clone(), Arc::from(dir_name.clone()));
        self.creating_local_params.insert(
            handle.clone(),
            local::CreateLocalParams {
                dir_name: dir_name.clone(),
                minecraft_version: minecraft_version.clone(),
                loader,
                loader_version: loader_version.clone(),
            },
        );

        let tasks = tasks::InstanceTaskList::new(handle.clone(), internal.clone());
        let request = local::CreateLocalRequest {
            handle: handle.clone(),
            dir_name,
            minecraft_version,
            loader,
            loader_version,
            launcher_dir: self.launcher_dir.clone(),
            client: self.client.clone(),
            frontend: tx.clone(),
            internal: internal.clone(),
            tasks: tasks.clone(),
        };

        let task_handle = handle.clone();
        let task = tokio::spawn(async move {
            let result = local::create_local_instance(request).await;
            let _ = internal.send(BackendEvent::InstallFinished {
                handle: task_handle,
                is_run: false,
                result,
            });
        });
        self.activities.insert(
            handle,
            Activity::new(instances::ActivityKind::Install, tasks.snapshot(), task),
        );
    }

    #[expect(clippy::too_many_arguments)]
    fn prepare_install(
        &self,
        handle: InstanceHandle,
        is_run: bool,
        force_overwrite: bool,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
        tasks: tasks::InstanceTaskList,
        auth: Option<(AuthProviderConfig, AccountData)>,
    ) -> install::InstallRequest {
        let settings = self.load_settings_for_id(&handle);
        install::InstallRequest {
            handle,
            cause: if is_run {
                InstallCause::Run
            } else {
                InstallCause::Update
            },
            force_overwrite,
            optional_mod_preferences: settings.optional_mod_sets,
            launcher_dir: DataDir::new(self.launcher_dir.clone()),
            client: self.client.clone(),
            local_instances: self.instance_storage.all().to_vec(),
            catalogs: self.catalogs.clone(),
            frontend: tx,
            internal,
            java_path: settings.java_path,
            tasks,
            auth,
        }
    }

    fn notify_busy(&self, handle: &InstanceHandle, tx: &FrontendSender) {
        let message = match self.activity_kind(handle) {
            Some(instances::ActivityKind::Install) => {
                launcher_i18n::notifications::install_already_running()
            }
            _ => launcher_i18n::notifications::already_launching_or_running(),
        };
        tx.send(MessageToFrontend::Notification {
            level: NotificationLevel::Info,
            message: Arc::from(message),
        });
    }

    fn start_install(
        &mut self,
        handle: InstanceHandle,
        force_overwrite: bool,
        launch_after_install: Option<bool>,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.is_busy(&handle) {
            self.notify_busy(&handle, &tx);
            return;
        }

        let auth = if self.required_provider_for_instance(&handle).is_some() {
            match self.resolve_instance_account(&handle, None) {
                Ok(auth) => Some(auth),
                Err(err) => {
                    self.notify_account_unresolved(&handle, err, &tx);
                    return;
                }
            }
        } else {
            None
        };

        self.install_errors.remove(&handle);
        if let Some(enabled) = launch_after_install {
            self.launch_after_install.insert(handle.clone(), enabled);
        }

        let tasks = tasks::InstanceTaskList::new(handle.clone(), internal.clone());
        let request = self.prepare_install(
            handle.clone(),
            false,
            force_overwrite,
            tx,
            internal.clone(),
            tasks.clone(),
            auth,
        );
        let task_handle = handle.clone();
        let task = tokio::spawn(async move {
            let result = install::install_instance(request)
                .await
                .map_err(|err| Arc::<str>::from(format!("{err:#}")));
            let _ = internal.send(BackendEvent::InstallFinished {
                handle: task_handle,
                is_run: false,
                result,
            });
        });
        self.activities.insert(
            handle,
            Activity::new(instances::ActivityKind::Install, tasks.snapshot(), task),
        );
    }

    async fn handle_install_finished(
        &mut self,
        handle: InstanceHandle,
        is_run: bool,
        result: Result<install::InstallOutput, Arc<str>>,
        tx: &FrontendSender,
    ) {
        if !is_run && self.activity_kind(&handle) == Some(instances::ActivityKind::Install) {
            self.activities.remove(&handle);
        }

        match result {
            Ok(output) => {
                self.creating_local.remove(&handle);
                self.creating_local_params.remove(&handle);
                let data_dir = DataDir::new(self.launcher_dir.clone());
                let save_result = if self.instance_storage.get(&output.instance.handle).is_some() {
                    self.instance_storage
                        .update(&data_dir, output.instance.clone())
                        .await
                } else {
                    self.instance_storage
                        .add(&data_dir, output.instance.clone())
                        .await
                };

                match save_result {
                    Ok(()) => {
                        self.install_errors.remove(&output.instance.handle);
                        if !is_run {
                            log::info!("Instance install completed for {}", output.instance.handle);
                        }
                    }
                    Err(err) => {
                        log::error!(
                            "Failed to save installed instance {}: {err:#}",
                            output.instance.handle
                        );
                        let error = Arc::<str>::from(err.to_string());
                        self.install_errors
                            .insert(output.instance.handle.clone(), error.clone());
                        tx.send(MessageToFrontend::Notification {
                            level: NotificationLevel::Error,
                            message: Arc::from(
                                launcher_i18n::notifications::failed_save_installed(
                                    error.to_string(),
                                ),
                            ),
                        });
                    }
                }
            }
            Err(error) => {
                log::error!("Install task for instance {handle} failed: {error}");
                self.install_errors.insert(handle, error.clone());
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::from(launcher_i18n::notifications::install_failed(
                        error.to_string(),
                    )),
                });
            }
        }

        self.emit_snapshot(tx);
    }

    fn cancel_install(&mut self, handle: InstanceHandle, tx: &FrontendSender) {
        match self.activity_kind(&handle) {
            Some(instances::ActivityKind::LaunchPrep) => {
                if let Some(activity) = self.activities.remove(&handle) {
                    activity.join.abort();
                }
                self.emit_snapshot(tx);
                return;
            }
            Some(instances::ActivityKind::Running) => return,
            Some(instances::ActivityKind::Install) => {
                if let Some(activity) = self.activities.remove(&handle) {
                    activity.join.abort();
                }
            }
            None => {}
        }
        self.launch_after_install.remove(&handle);
        let params = self.creating_local_params.remove(&handle);
        let dir_name = self
            .creating_local
            .remove(&handle)
            .map(|name| name.to_string())
            .or_else(|| params.as_ref().map(|params| params.dir_name.clone()));
        self.install_errors.remove(&handle);

        if let Some(dir_name) = dir_name
            && self.instance_storage.get(&handle).is_none()
        {
            let launcher_dir = self.launcher_dir.clone();
            tokio::spawn(async move {
                let data_dir = DataDir::new(launcher_dir);
                let instance_path = InstancesDir::root()
                    .instance_dir(&dir_name)
                    .with_data_dir(data_dir)
                    .to_fs();
                if instance_path.exists()
                    && let Err(err) = tokio::fs::remove_dir_all(&instance_path).await
                {
                    log::warn!(
                        "Failed to remove partial local instance directory {}: {err:#}",
                        instance_path.display()
                    );
                }
            });
        }

        self.emit_snapshot(tx);
    }

    fn retry_create_local(
        &mut self,
        handle: InstanceHandle,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.is_busy(&handle) {
            self.notify_busy(&handle, &tx);
            return;
        }

        let Some(params) = self.creating_local_params.get(&handle).cloned() else {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::install_failed(
                    "no stored create parameters for retry".to_string(),
                )),
            });
            return;
        };

        self.install_errors.remove(&handle);
        self.launch_after_install.insert(handle.clone(), true);
        self.creating_local
            .insert(handle.clone(), Arc::from(params.dir_name.clone()));

        let tasks = tasks::InstanceTaskList::new(handle.clone(), internal.clone());
        let request = local::CreateLocalRequest {
            handle: handle.clone(),
            dir_name: params.dir_name,
            minecraft_version: params.minecraft_version,
            loader: params.loader,
            loader_version: params.loader_version,
            launcher_dir: self.launcher_dir.clone(),
            client: self.client.clone(),
            frontend: tx.clone(),
            internal: internal.clone(),
            tasks: tasks.clone(),
        };

        let task_handle = handle.clone();
        let task = tokio::spawn(async move {
            let result = local::create_local_instance(request).await;
            let _ = internal.send(BackendEvent::InstallFinished {
                handle: task_handle,
                is_run: false,
                result,
            });
        });
        self.activities.insert(
            handle,
            Activity::new(instances::ActivityKind::Install, tasks.snapshot(), task),
        );
    }

    async fn delete_instance(&mut self, handle: InstanceHandle, tx: &FrontendSender) {
        match self.activity_kind(&handle) {
            Some(instances::ActivityKind::LaunchPrep | instances::ActivityKind::Running) => {
                log::warn!("Ignoring delete for {handle}: instance is running or launching");
                return;
            }
            Some(instances::ActivityKind::Install) => {
                log::warn!("Ignoring delete for {handle}: install is in progress");
                return;
            }
            None => {}
        }
        let data_dir = DataDir::new(self.launcher_dir.clone());
        match self
            .instance_storage
            .remove_from_disk(&data_dir, &handle)
            .await
        {
            Ok(Some(_)) => {
                self.install_errors.remove(&handle);
                self.launch_errors.remove(&handle);
                log::info!("Instance deleted: {handle}");
            }
            Ok(None) => {
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Warning,
                    message: Arc::from(
                        launcher_i18n::notifications::instance_not_installed_locally(),
                    ),
                });
            }
            Err(err) => {
                log::error!("Failed to delete instance {handle}: {err:#}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::from(launcher_i18n::notifications::failed_delete_instance(
                        err.to_string(),
                    )),
                });
            }
        }
        self.emit_snapshot(tx);
    }

    fn start_add_account(
        &mut self,
        provider: AuthProviderConfig,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if matches!(provider, AuthProviderConfig::Offline(_)) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Info,
                message: Arc::from(launcher_i18n::notifications::enter_offline_nickname()),
            });
            return;
        }

        if self.add_account_task.is_some() {
            log::warn!("Ignoring add account request: authentication already in progress");
            return;
        }

        let cancel = CancellationToken::new();
        let cancel_token = cancel.clone();
        self.add_account_cancel = Some(cancel);
        let auth_prompt = Arc::new(AuthPromptReporter::new(tx));
        let client = self.client.clone();
        let task = tokio::spawn(async move {
            let result = tokio::select! {
                result = perform_auth(&client, None, provider.clone(), auth_prompt) => {
                    Some(
                        result
                            .map(|account| (provider, account))
                            .map_err(|err| Arc::<str>::from(format!("{err:#}"))),
                    )
                }
                () = cancel_token.cancelled() => None,
            };
            let event = match result {
                Some(result) => BackendEvent::AddAccountFinished { result },
                None => BackendEvent::AddAccountCancelled,
            };
            let _ = internal.send(event);
        });
        self.add_account_task = Some(task);
    }

    fn cancel_auth(&mut self, tx: &FrontendSender) {
        if let Some(cancel) = self.add_account_cancel.take() {
            cancel.cancel();
            return;
        }
        if let Some(instance) = self.auth_instance.take() {
            self.cancel_instance_auth(instance, tx);
        }
    }

    fn cancel_instance_auth(&mut self, instance: InstanceHandle, tx: &FrontendSender) {
        tx.send(MessageToFrontend::AuthPromptCleared);
        let error = Arc::<str>::from(launcher_i18n::notifications::authentication_cancelled());
        match self.activity_kind(&instance) {
            Some(instances::ActivityKind::Running) | None => return,
            Some(instances::ActivityKind::LaunchPrep) => {
                if let Some(mut activity) = self.activities.remove(&instance) {
                    activity.kill.take();
                    activity.join.abort();
                }
                self.launch_errors.insert(instance.clone(), error.clone());
                tx.send(MessageToFrontend::LaunchFinished {
                    instance: instance.clone(),
                    exit: launcher_bridge::ExitOutcome::Error(error),
                });
            }
            Some(instances::ActivityKind::Install) => {
                if let Some(activity) = self.activities.remove(&instance) {
                    activity.join.abort();
                }
                self.install_errors.insert(instance.clone(), error);
            }
        }
        self.emit_snapshot(tx);
    }

    fn clear_add_account_task(&mut self) {
        self.add_account_task.take();
        self.add_account_cancel.take();
    }

    async fn submit_offline_nickname(&mut self, nickname: String, tx: &FrontendSender) {
        let nickname = nickname.trim();
        if nickname.is_empty() {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::offline_nickname_empty()),
            });
            return;
        }

        let (key, provider, account) = launch::offline_account(nickname);
        match self.auth_storage.insert_account(&provider, account).await {
            Ok(_) => {
                log::info!("Added offline account {}", key.1);
            }
            Err(err) => {
                log::error!("Failed to save offline account {key:?}: {err:#}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::from(launcher_i18n::notifications::failed_save_offline_account(
                        err.to_string(),
                    )),
                });
            }
        }
        self.emit_snapshot(tx);
    }

    async fn clear_account_references(&self, key: &AccountKey) -> anyhow::Result<()> {
        let data_dir = DataDir::new(self.launcher_dir.clone());
        for local in self.instance_storage.all() {
            let instance_dir = InstancesDir::root()
                .instance_dir(&local.dir_name)
                .with_data_dir(data_dir.clone());
            let mut settings = load_instance_settings(&instance_dir).await?;
            let mut changed = false;
            if settings.selected_account.as_ref() == Some(key) {
                settings.selected_account = None;
                changed = true;
            }
            if settings.account_override.as_ref() == Some(key) {
                settings.account_override = None;
                changed = true;
            }
            if changed {
                save_instance_settings(&instance_dir, &settings).await?;
            }
        }
        Ok(())
    }

    async fn remove_account(&mut self, key: AccountKey, tx: &FrontendSender) {
        match self.auth_storage.delete_account(key.0, &key.1).await {
            Ok(()) => {
                if let Err(err) = self.clear_account_references(&key).await {
                    log::warn!("Failed to clear instance account references for {key:?}: {err:#}");
                }
                log::info!("Account removed: {key:?}");
            }
            Err(err) => {
                log::error!("Failed to remove account {key:?}: {err:#}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::from(launcher_i18n::notifications::failed_remove_account(
                        err.to_string(),
                    )),
                });
            }
        }
        self.emit_snapshot(tx);
    }

    fn account_provider(&self, key: &AccountKey) -> Option<AuthProviderConfig> {
        self.account_views()
            .iter()
            .find(|account| &account.key == key)
            .map(|account| account.provider.clone())
    }

    async fn ensure_instance_for_settings(
        &mut self,
        handle: &InstanceHandle,
    ) -> anyhow::Result<LocalInstance> {
        if let Some(local) = self.instance_storage.get(handle) {
            return Ok(local.clone());
        }

        for (url, state) in &self.catalogs {
            let Some(manifest) = state.manifest() else {
                continue;
            };
            for entry in &manifest.instances {
                if instances::remote_entry_handle(url, &entry.id) == *handle {
                    let source = RemoteSource {
                        manifest_url: url.clone(),
                        id_in_manifest: entry.id.clone(),
                    };
                    let local = LocalInstance::new_pending_remote(
                        handle.clone(),
                        self.instance_storage
                            .allocate_dir_name(&entry.id)
                            .map_err(|err| {
                                anyhow::anyhow!(
                                    "invalid instance id '{}' in catalog {}: {err}",
                                    entry.id,
                                    url
                                )
                            })?,
                        source,
                    );
                    let data_dir = DataDir::new(self.launcher_dir.clone());
                    self.instance_storage.add(&data_dir, local.clone()).await?;
                    return Ok(local);
                }
            }
        }

        Err(anyhow::anyhow!(
            "instance {handle} was not found in local storage or fetched catalogs"
        ))
    }

    async fn update_instance_settings(
        &mut self,
        handle: &InstanceHandle,
        update: impl FnOnce(&mut InstanceUserSettings),
    ) -> anyhow::Result<InstanceUserSettings> {
        let local = self.ensure_instance_for_settings(handle).await?;
        let instance_dir = self.instance_dir_fs(&local);
        let mut settings = load_instance_settings(&instance_dir).await?;
        update(&mut settings);
        save_instance_settings(&instance_dir, &settings).await?;
        Ok(settings)
    }

    fn required_provider_for_instance(
        &self,
        instance: &InstanceHandle,
    ) -> Option<AuthProviderConfig> {
        self.build_instance_views()
            .iter()
            .find(|view| &view.handle == instance)
            .and_then(|view| view.auth_provider.clone())
    }

    fn resolve_instance_account(
        &self,
        handle: &InstanceHandle,
        requested: Option<AccountKey>,
    ) -> Result<(AuthProviderConfig, AccountData), launch::LaunchError> {
        let settings = self.load_settings_for_id(handle);
        let launch_accounts = self.launch_accounts();
        let valid_override =
            launch::stored_account_if_valid(&settings.account_override, &launch_accounts);
        let bypass_required_provider = requested.is_none() && valid_override.is_some();
        let account = requested.or(valid_override).or_else(|| {
            launch::stored_account_if_valid(&settings.selected_account, &launch_accounts)
        });
        let required = (!bypass_required_provider)
            .then(|| self.required_provider_for_instance(handle))
            .flatten();
        launch::resolve_account(account, required.as_ref(), &launch_accounts)
    }

    fn notify_account_unresolved(
        &self,
        handle: &InstanceHandle,
        err: launch::LaunchError,
        tx: &FrontendSender,
    ) {
        let reason = self
            .build_instance_views()
            .iter()
            .find(|view| &view.handle == handle)
            .and_then(|view| view.launch_blocked_reason.clone());
        tx.send(MessageToFrontend::Notification {
            level: NotificationLevel::Warning,
            message: reason.unwrap_or_else(|| Arc::from(format!("{err:#}"))),
        });
    }

    async fn handle_add_account_finished(
        &mut self,
        result: Result<(AuthProviderConfig, AccountData), Arc<str>>,
        tx: &FrontendSender,
    ) {
        self.clear_add_account_task();
        tx.send(MessageToFrontend::AuthPromptCleared);
        match result {
            Ok((provider, account)) => {
                match self.auth_storage.insert_account(&provider, account).await {
                    Ok((_, username)) => {
                        log::info!("Added account {username}");
                    }
                    Err(err) => {
                        log::error!("Failed to save authenticated account: {err:#}");
                        tx.send(MessageToFrontend::Notification {
                            level: NotificationLevel::Error,
                            message: Arc::from(launcher_i18n::notifications::failed_save_account(
                                err.to_string(),
                            )),
                        });
                    }
                }
            }
            Err(error) => {
                log::error!("Authentication failed: {error}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::from(launcher_i18n::notifications::authentication_failed(
                        error.to_string(),
                    )),
                });
            }
        }
        self.emit_snapshot(tx);
    }

    fn handle_add_account_cancelled(&mut self, tx: &FrontendSender) {
        self.clear_add_account_task();
        tx.send(MessageToFrontend::AuthPromptCleared);
    }

    fn handle_auth_launch_prompt(&mut self, instance: Option<InstanceHandle>) {
        self.auth_instance = instance;
    }

    async fn set_instance_account_override(
        &mut self,
        instance: InstanceHandle,
        account: Option<AccountKey>,
        tx: &FrontendSender,
    ) {
        if let Some(account) = &account
            && let Some(required) = self.required_provider_for_instance(&instance)
            && self.account_provider(account).as_ref() == Some(&required)
        {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(
                    launcher_i18n::notifications::use_account_selection_for_required(),
                ),
            });
            return;
        }
        if let Err(err) = self
            .update_instance_settings(&instance, |settings| settings.account_override = account)
            .await
        {
            log::error!("Failed to save account override for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_account_override(
                    err.to_string(),
                )),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_selected_account(
        &mut self,
        instance: InstanceHandle,
        account: Option<AccountKey>,
        tx: &FrontendSender,
    ) {
        if let Some(account) = &account
            && let Some(required) = self.required_provider_for_instance(&instance)
            && self.account_provider(account).as_ref() != Some(&required)
        {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::selected_account_must_match()),
            });
            return;
        }
        let clear_override = account.is_some();
        if let Err(err) = self
            .update_instance_settings(&instance, |settings| {
                settings.selected_account = account;
                if clear_override {
                    settings.account_override = None;
                }
            })
            .await
        {
            log::error!("Failed to save selected account for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_selected_account(
                    err.to_string(),
                )),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_launcher_settings(&mut self, settings: LauncherSettingsView, tx: &FrontendSender) {
        self.settings.hide_window_after_launch = settings.hide_window_after_launch;
        self.settings.hide_usernames_in_cards = settings.hide_usernames_in_cards;
        self.settings.language =
            Some(resolve_language_code(Some(settings.language.as_str()), None).to_string());
        set_lang(self.settings.resolved_language_code());
        if let Err(err) = self.settings.save(&self.launcher_dir).await {
            log::error!("Failed to save launcher settings: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_launcher_settings(
                    err.to_string(),
                )),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_memory(
        &mut self,
        instance: InstanceHandle,
        xmx_mb: Option<u64>,
        tx: &FrontendSender,
    ) {
        if let Err(err) = self
            .update_instance_settings(&instance, |settings| settings.xmx_mb = xmx_mb)
            .await
        {
            log::error!("Failed to save memory override for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_memory_override(
                    err.to_string(),
                )),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_jvm_flags(
        &mut self,
        instance: InstanceHandle,
        flags: Option<String>,
        tx: &FrontendSender,
    ) {
        let normalized =
            flags.and_then(|flags| (!flags.trim().is_empty()).then(|| flags.trim().to_string()));
        if let Err(err) = self
            .update_instance_settings(&instance, |settings| settings.jvm_flags = normalized)
            .await
        {
            log::error!("Failed to save JVM flags for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_jvm_flags(
                    err.to_string(),
                )),
            });
        }
        self.emit_snapshot(tx);
    }

    fn is_local_install_in_progress(&self, instance: &InstanceHandle) -> bool {
        self.creating_local.contains_key(instance)
    }

    fn set_launch_after_install(
        &mut self,
        instance: InstanceHandle,
        enabled: bool,
        tx: &FrontendSender,
    ) {
        if let std::collections::hash_map::Entry::Occupied(mut e) =
            self.launch_after_install.entry(instance)
        {
            e.insert(enabled);
            self.emit_snapshot(tx);
        }
    }

    async fn set_optional_mod_set_enabled(
        &mut self,
        instance: InstanceHandle,
        set_id: String,
        enabled: bool,
        tx: &FrontendSender,
    ) {
        if self.activity_kind(&instance) == Some(instances::ActivityKind::Install) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Info,
                message: Arc::from(launcher_i18n::notifications::install_already_running()),
            });
            return;
        }
        if self.is_local_install_in_progress(&instance) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(
                    launcher_i18n::notifications::optional_mod_install_in_progress(),
                ),
            });
            return;
        }
        if matches!(
            self.activity_kind(&instance),
            Some(instances::ActivityKind::LaunchPrep | instances::ActivityKind::Running)
        ) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::optional_mod_instance_running()),
            });
            return;
        }
        let Some(local) = self
            .instance_storage
            .all()
            .iter()
            .find(|entry| entry.handle == instance && entry.is_installed())
            .cloned()
        else {
            return;
        };
        let dir_name = local.dir_name.clone();
        let data_dir = DataDir::new(self.launcher_dir.clone());
        let instance_dir = InstancesDir::root()
            .instance_dir(&dir_name)
            .with_data_dir(data_dir.clone());
        let Ok(metadata) = InstanceMetadata::read_local(&instance_dir).await else {
            return;
        };
        let is_optional = metadata
            .mod_sync
            .optional_sets
            .iter()
            .any(|entry| entry.id == set_id);
        if !is_optional {
            return;
        }

        if let Err(err) = self
            .update_instance_settings(&instance, |settings| {
                settings.optional_mod_sets.insert(set_id.clone(), enabled);
            })
            .await
        {
            log::error!("Failed to save optional mod set setting for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_optional_mod(
                    err.to_string(),
                )),
            });
            return;
        }
        self.emit_snapshot(tx);

        if let Err(err) =
            install::apply_optional_mod_set(&instance_dir, &metadata, &set_id, enabled).await
        {
            log::error!(
                "Failed to apply optional mod set {set_id} for instance {instance}: {err:#}"
            );
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::optional_mod_sync_failed(
                    err.to_string(),
                )),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_use_native_glfw(
        &mut self,
        instance: InstanceHandle,
        enabled: bool,
        tx: &FrontendSender,
    ) {
        if self.is_local_install_in_progress(&instance) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::java_path_install_in_progress()),
            });
            return;
        }
        if let Err(err) = self
            .update_instance_settings(&instance, |settings| {
                settings.use_native_glfw = Some(enabled)
            })
            .await
        {
            log::error!("Failed to save native GLFW setting for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_native_glfw(
                    err.to_string(),
                )),
            });
            return;
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_java_path(
        &mut self,
        instance: InstanceHandle,
        path: Option<String>,
        tx: &FrontendSender,
    ) {
        if self.is_local_install_in_progress(&instance) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::java_path_install_in_progress()),
            });
            return;
        }
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            log::warn!("Ignoring request to clear Java path for instance {instance}");
            return;
        };
        let Some(required_version) = self.required_java_version_for(&instance) else {
            log::error!("Missing required Java version for instance {instance}");
            return;
        };
        let java_path = std::path::Path::new(&path);
        if !utils::java::check_java(&required_version, java_path).await {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::invalid_java_path()),
            });
            return;
        }
        if let Err(err) = self
            .update_instance_settings(&instance, |settings| settings.java_path = Some(path))
            .await
        {
            log::error!("Failed to save Java path for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::failed_save_java_path(
                    err.to_string(),
                )),
            });
            return;
        }
        log::info!("Java path set for instance {instance}");
        self.emit_snapshot(tx);
    }

    fn required_java_version_for(&self, instance: &InstanceHandle) -> Option<String> {
        if self.is_local_install_in_progress(instance) {
            return None;
        }
        self.build_instance_views()
            .iter()
            .find(|v| &v.handle == instance)
            .and_then(|v| v.required_java_version.as_deref().map(str::to_owned))
    }

    fn resolve_java_path(
        &self,
        instance: InstanceHandle,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.is_local_install_in_progress(&instance) {
            return;
        }
        let Some(local) = self.instance_storage.get(&instance).cloned() else {
            return;
        };
        let stored_java_path = self.load_settings_for_id(&instance).java_path.clone();
        let launcher_dir = self.launcher_dir.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            let data_dir = utils::paths::DataDir::new(launcher_dir);
            let instance_dir = InstancesDir::root()
                .instance_dir(&local.dir_name)
                .with_data_dir(data_dir.clone());
            let metadata = match launch::read_metadata(&instance_dir).await {
                Ok(metadata) => metadata,
                Err(err) => {
                    log::error!(
                        "Failed to read instance metadata for Java resolve on {instance}: {err:#}"
                    );
                    let _ = internal.send(BackendEvent::JavaResolved {
                        instance,
                        path: None,
                    });
                    return;
                }
            };
            // progress from this standalone resolve has no visible activity
            // its task snapshots are dropped by the event handler
            let tasks = tasks::InstanceTaskList::new(instance.clone(), internal.clone());
            match install::resolve_java(
                &client,
                &metadata,
                &data_dir,
                stored_java_path.as_deref(),
                &tasks,
            )
            .await
            {
                Ok(installation) => {
                    install::persist_java_installation(instance, &installation, &internal);
                }
                Err(err) => {
                    log::error!("Failed to resolve Java for instance {instance}: {err:#}");
                    let _ = internal.send(BackendEvent::JavaResolved {
                        instance,
                        path: None,
                    });
                }
            }
        });
    }

    fn start_launch(
        &mut self,
        handle: InstanceHandle,
        account: Option<AccountKey>,
        skip_sync: bool,
        prepared_java: Option<utils::java::JavaInstallation>,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.is_busy(&handle) {
            self.notify_busy(&handle, &tx);
            return;
        }

        let Some(local) = self.instance_storage.get(&handle) else {
            return;
        };
        if !local.is_installed() {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::instance_not_installed_locally()),
            });
            return;
        }

        let (provider, account_data) = match self.resolve_instance_account(&handle, account) {
            Ok(resolved) => resolved,
            Err(err) => {
                self.notify_account_unresolved(&handle, err, &tx);
                return;
            }
        };

        self.launch_errors.remove(&handle);

        let settings = self.load_settings_for_id(&handle);
        let tasks = tasks::InstanceTaskList::new(handle.clone(), internal.clone());
        let java_path = settings.java_path.clone();
        // auth happens first in the spawned task; the sync itself never
        // needs the account again
        let install_request = self.prepare_install(
            handle.clone(),
            true,
            false,
            tx.clone(),
            internal.clone(),
            tasks.clone(),
            None,
        );
        let xmx_mb = settings.xmx_mb;
        let jvm_flags = settings.jvm_flags.clone();
        let use_native_glfw = settings.use_native_glfw;
        let launcher_dir = self.launcher_dir.clone();
        let client = self.client.clone();
        let local_instances = self.instance_storage.all().to_vec();
        let frontend = tx.clone();
        let (kill_tx, mut kill_rx) = oneshot::channel();
        let task_handle = handle.clone();
        let initial_tasks = tasks.snapshot();
        let task = tokio::spawn(async move {
            // 1. authenticate before any sync starts
            let auth_messages = Arc::new(launch::InstanceAuthMessages::new(
                frontend.clone(),
                task_handle.clone(),
                internal.clone(),
            ));
            let authenticated = match launch::authenticate_account(
                &client,
                provider,
                account_data,
                auth_messages,
                &frontend,
            )
            .await
            {
                Ok(authenticated) => authenticated,
                Err(err) => {
                    log::error!("Failed to authenticate for instance {task_handle}: {err:#}");
                    let _ = internal.send(BackendEvent::LaunchFinished {
                        handle: task_handle,
                        exit: launcher_bridge::ExitOutcome::Error(Arc::<str>::from(format!(
                            "{err:#}"
                        ))),
                    });
                    return;
                }
            };
            if let Some(refreshed) = &authenticated.refreshed {
                let _ = internal.send(BackendEvent::AccountUpdated {
                    provider: authenticated.provider.clone(),
                    account: refreshed.clone(),
                });
            }

            // 2. sync files and resolve Java (parallel inside install_instance)
            // or reuse the result of an install that just finished
            let java = if skip_sync {
                match prepared_java {
                    Some(java) => java,
                    None => {
                        let resolve_result = async {
                            let local = local_instances
                                .iter()
                                .find(|instance| instance.handle == task_handle)
                                .ok_or_else(|| {
                                    anyhow::anyhow!("installed instance {task_handle} not found")
                                })?;
                            let data_dir = DataDir::new(launcher_dir.clone());
                            let instance_dir = InstancesDir::root()
                                .instance_dir(&local.dir_name)
                                .with_data_dir(data_dir.clone());
                            let metadata = launch::read_metadata(&instance_dir).await?;
                            install::resolve_java(
                                &client,
                                &metadata,
                                &data_dir,
                                java_path.as_deref(),
                                &tasks,
                            )
                            .await
                        }
                        .await;
                        match resolve_result {
                            Ok(java) => java,
                            Err(err) => {
                                let _ = internal.send(BackendEvent::LaunchFinished {
                                    handle: task_handle,
                                    exit: launcher_bridge::ExitOutcome::Error(Arc::<str>::from(
                                        format!("{err:#}"),
                                    )),
                                });
                                return;
                            }
                        }
                    }
                }
            } else {
                let install_result = install::install_instance(install_request)
                    .await
                    .map_err(|err| Arc::<str>::from(format!("{err:#}")));
                let _ = internal.send(BackendEvent::InstallFinished {
                    handle: task_handle.clone(),
                    is_run: true,
                    result: install_result.clone(),
                });
                match install_result {
                    Ok(output) => output.java,
                    Err(err) => {
                        log::error!("Failed to update instance {task_handle} on launch: {err}");
                        let _ = internal.send(BackendEvent::LaunchFinished {
                            handle: task_handle,
                            exit: launcher_bridge::ExitOutcome::Error(err),
                        });
                        return;
                    }
                }
            };

            // 3. start the game
            let launch_handle = task_handle.clone();
            let launch_result = launch::launch_instance(launch::LaunchRequest {
                handle: launch_handle.clone(),
                provider: authenticated.provider,
                account_data: authenticated.data,
                online: authenticated.online,
                xmx_mb,
                jvm_flags,
                java,
                use_native_glfw,
                launcher_dir,
                local_instances,
            })
            .await;

            match launch_result {
                Ok(start) => {
                    let _ = internal.send(BackendEvent::LaunchStarted {
                        handle: launch_handle.clone(),
                    });
                    let mut child = start.child;
                    let exit = tokio::select! {
                        status = child.wait() => exit_outcome(status),
                        _ = &mut kill_rx => {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            launcher_bridge::ExitOutcome::Terminated
                        }
                    };
                    let _ = internal.send(BackendEvent::LaunchFinished {
                        handle: launch_handle.clone(),
                        exit,
                    });
                }
                Err(err) => {
                    log::error!("Failed to launch instance {launch_handle}: {err:#}");
                    let _ = internal.send(BackendEvent::LaunchFinished {
                        handle: launch_handle.clone(),
                        exit: launcher_bridge::ExitOutcome::Error(Arc::<str>::from(format!(
                            "{err:#}"
                        ))),
                    });
                }
            }
        });
        let mut activity = Activity::new(instances::ActivityKind::LaunchPrep, initial_tasks, task);
        activity.kill = Some(kill_tx);
        self.activities.insert(handle, activity);
    }

    async fn handle_java_resolved(
        &mut self,
        instance: InstanceHandle,
        path: Option<Arc<str>>,
        tx: &FrontendSender,
    ) {
        if let Some(ref path) = path {
            if let Err(err) = self
                .update_instance_settings(&instance, |settings| {
                    settings.java_path = Some(path.to_string());
                })
                .await
            {
                log::error!("Failed to save Java path for instance {instance}: {err:#}");
            } else {
                self.emit_snapshot(tx);
            }
        }
        tx.send(MessageToFrontend::JavaPathResolved { instance, path });
    }

    fn handle_launch_started(&mut self, handle: InstanceHandle, tx: &FrontendSender) {
        if let Some(activity) = self.activities.get_mut(&handle) {
            activity.kind = instances::ActivityKind::Running;
            activity.tasks = Arc::from([]);
        }
        self.emit_snapshot(tx);
    }

    async fn handle_launch_account_updated(
        &mut self,
        provider: AuthProviderConfig,
        account: AccountData,
        tx: &FrontendSender,
    ) {
        if let Err(err) = self.auth_storage.insert_account(&provider, account).await {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::failed_save_refreshed_account(
                    err.to_string(),
                )),
            });
        }
        self.emit_snapshot(tx);
    }

    fn handle_launch_finished(
        &mut self,
        handle: InstanceHandle,
        exit: launcher_bridge::ExitOutcome,
        tx: &FrontendSender,
    ) {
        self.auth_instance = None;
        if let Some(mut activity) = self.activities.remove(&handle) {
            activity.kill.take();
        }
        match &exit {
            launcher_bridge::ExitOutcome::Success | launcher_bridge::ExitOutcome::Terminated => {
                self.launch_errors.remove(&handle);
            }
            launcher_bridge::ExitOutcome::ExitCode(code) => {
                self.launch_errors.insert(
                    handle.clone(),
                    Arc::from(launcher_i18n::notifications::minecraft_exited_with_code(
                        *code,
                    )),
                );
            }
            launcher_bridge::ExitOutcome::Error(error) => {
                self.launch_errors.insert(
                    handle.clone(),
                    Arc::from(launcher_i18n::notifications::launch_failed(
                        error.to_string(),
                    )),
                );
            }
        }
        tx.send(MessageToFrontend::LaunchFinished {
            instance: handle,
            exit: exit.clone(),
        });
        self.emit_snapshot(tx);
    }

    fn kill_launch(&mut self, handle: InstanceHandle, tx: &FrontendSender) {
        match self.activity_kind(&handle) {
            Some(instances::ActivityKind::Running) => {
                if let Some(activity) = self.activities.get_mut(&handle)
                    && let Some(kill) = activity.kill.take()
                {
                    let _ = kill.send(());
                }
            }
            Some(instances::ActivityKind::LaunchPrep) => {
                if let Some(activity) = self.activities.remove(&handle) {
                    activity.join.abort();
                }
                tx.send(MessageToFrontend::LaunchFinished {
                    instance: handle.clone(),
                    exit: launcher_bridge::ExitOutcome::Terminated,
                });
            }
            Some(instances::ActivityKind::Install) | None => {}
        }
        self.emit_snapshot(tx);
    }
}

fn exit_outcome(status: std::io::Result<std::process::ExitStatus>) -> launcher_bridge::ExitOutcome {
    match status {
        Ok(status) if status.success() => launcher_bridge::ExitOutcome::Success,
        Ok(status) => status
            .code()
            .map(launcher_bridge::ExitOutcome::ExitCode)
            .unwrap_or(launcher_bridge::ExitOutcome::Terminated),
        Err(err) => launcher_bridge::ExitOutcome::Error(Arc::<str>::from(err.to_string())),
    }
}

fn parse_xmx_mb(value: Option<&str>) -> Option<u64> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(raw) = value.strip_suffix(['m', 'M']) {
        raw.trim().parse().ok()
    } else if let Some(raw) = value.strip_suffix(['g', 'G']) {
        raw.trim().parse::<u64>().ok().map(|gb| gb * 1024)
    } else {
        value.parse().ok()
    }
}

pub async fn run(
    launcher_dir: PathBuf,
    mut receiver: BackendReceiver,
    frontend: FrontendSender,
) -> anyhow::Result<()> {
    let mut state = BackendState::load(launcher_dir).await?;
    let (internal_sender, mut internal_receiver) = mpsc::unbounded_channel();

    if update::should_check_updates() {
        frontend.send(MessageToFrontend::UpdateStatus(
            launcher_bridge::UpdateStatusView::Checking,
        ));
        let update_client = state.client.clone();
        let update_frontend = frontend.clone();
        tokio::spawn(async move {
            update::run(update_client, update_frontend).await;
        });
    } else {
        frontend.send(MessageToFrontend::UpdateStatus(
            launcher_bridge::UpdateStatusView::NotApplicable,
        ));
    }

    state.emit_snapshot(&frontend);
    for (level, message) in state.startup_notices.drain(..) {
        frontend.send(MessageToFrontend::Notification { level, message });
    }
    state.refresh_all(&internal_sender, &frontend).await;

    loop {
        tokio::select! {
            message = receiver.recv() => {
                let Some(message) = message else {
                    break;
                };
                match message {
                    MessageToBackend::Refresh => {
                        state.refresh_all(&internal_sender, &frontend).await;
                    }
                    MessageToBackend::InstallInstance {
                        handle,
                        force_overwrite,
                        launch_after_install,
                    } => {
                        state.start_install(
                            handle,
                            force_overwrite,
                            launch_after_install,
                            frontend.clone(),
                            internal_sender.clone(),
                        );
                        state.emit_snapshot(&frontend);
                    }
                    MessageToBackend::CancelInstall(handle) => {
                        state.cancel_install(handle, &frontend);
                    }
                    MessageToBackend::RetryCreateLocal(handle) => {
                        state.retry_create_local(handle, frontend.clone(), internal_sender.clone());
                        state.emit_snapshot(&frontend);
                    }
                    MessageToBackend::DeleteInstance(handle) => {
                        state.delete_instance(handle, &frontend).await;
                    }
                    MessageToBackend::Launch { instance, account } => {
                        state.start_launch(
                            instance,
                            account,
                            false,
                            None,
                            frontend.clone(),
                            internal_sender.clone(),
                        );
                        state.emit_snapshot(&frontend);
                    }
                    MessageToBackend::KillInstance(handle) => {
                        state.kill_launch(handle, &frontend);
                    }
                    MessageToBackend::AddBackendUrl(url) => {
                        match state.add_backend_url(url.clone(), &frontend).await {
                            Ok(true) => {
                                state.start_fetch(url, &internal_sender);
                                state.emit_snapshot(&frontend);
                            }
                            Ok(false) => {}
                            Err(err) => {
                                log::error!("Failed to add backend URL {url}: {err:#}");
                                frontend.send(MessageToFrontend::Notification {
                                    level: NotificationLevel::Error,
                                    message: Arc::from(launcher_i18n::notifications::failed_add_backend_url(err.to_string())),
                                });
                            }
                        }
                    }
                    MessageToBackend::RemoveBackendUrl(url) => {
                        if let Err(err) = state.remove_backend_url(&url, &frontend).await {
                            log::error!("Failed to remove backend URL {url}: {err:#}");
                            frontend.send(MessageToFrontend::Notification {
                                level: NotificationLevel::Error,
                                message: Arc::from(launcher_i18n::notifications::failed_remove_backend_url(err.to_string())),
                            });
                        }
                    }
                    MessageToBackend::StartAddAccount(provider) => {
                        state.start_add_account(provider, frontend.clone(), internal_sender.clone());
                    }
                    MessageToBackend::CancelAuth => {
                        state.cancel_auth(&frontend);
                    }
                    MessageToBackend::SubmitOfflineNickname(nickname) => {
                        state.submit_offline_nickname(nickname, &frontend).await;
                    }
                    MessageToBackend::RemoveAccount(account) => {
                        state.remove_account(account, &frontend).await;
                    }
                    MessageToBackend::SetInstanceSelectedAccount { instance, account } => {
                        state.set_instance_selected_account(instance, account, &frontend).await;
                    }
                    MessageToBackend::SetInstanceAccountOverride { instance, account } => {
                        state.set_instance_account_override(instance, account, &frontend).await;
                    }
                    MessageToBackend::SetLauncherSettings(settings) => {
                        state.set_launcher_settings(settings, &frontend).await;
                    }
                    MessageToBackend::SetInstanceMemory { instance, xmx_mb } => {
                        state.set_instance_memory(instance, xmx_mb, &frontend).await;
                    }
                    MessageToBackend::SetInstanceJvmFlags { instance, flags } => {
                        state.set_instance_jvm_flags(instance, flags, &frontend).await;
                    }
                    MessageToBackend::SetInstanceJavaPath { instance, path } => {
                        state.set_instance_java_path(instance, path, &frontend).await;
                    }
                    MessageToBackend::SetInstanceUseNativeGlfw { instance, enabled } => {
                        state
                            .set_instance_use_native_glfw(instance, enabled, &frontend)
                            .await;
                    }
                    MessageToBackend::SetOptionalModSetEnabled {
                        instance,
                        set_id,
                        enabled,
                    } => {
                        state
                            .set_optional_mod_set_enabled(
                                instance,
                                set_id,
                                enabled,
                                &frontend,
                            )
                            .await;
                    }
                    MessageToBackend::SetLaunchAfterInstall { instance, enabled } => {
                        state.set_launch_after_install(instance, enabled, &frontend);
                    }
                    MessageToBackend::ResolveJavaPath(instance) => {
                        state.resolve_java_path(instance, internal_sender.clone());
                    }
                    MessageToBackend::CreateLocalInstance {
                        display_name,
                        minecraft_version,
                        loader,
                        loader_version,
                    } => {
                        state.start_create_local(
                            display_name,
                            minecraft_version,
                            loader,
                            loader_version,
                            frontend.clone(),
                            internal_sender.clone(),
                        );
                        state.emit_snapshot(&frontend);
                    }
                    MessageToBackend::FetchLocalCreateVersions => {
                        versions::start_fetch_local_create_versions(
                            state.client.clone(),
                            frontend.clone(),
                        );
                    }
                    MessageToBackend::FetchLoaderVersions {
                        minecraft_version,
                        loader,
                    } => {
                        versions::start_fetch_loader_versions(
                            state.client.clone(),
                            frontend.clone(),
                            minecraft_version,
                            loader,
                        );
                    }
                    MessageToBackend::ProceedAfterUpdateFailure => {
                        frontend.send(MessageToFrontend::UpdateStatus(
                            launcher_bridge::UpdateStatusView::NotApplicable,
                        ));
                    }
                    MessageToBackend::Quit => break,
                }
            }
            event = internal_receiver.recv() => {
                match event {
                    Some(BackendEvent::FetchFinished { url, result }) => {
                        state.handle_fetch_finished(url, result, &frontend);
                    }
                    Some(BackendEvent::InstanceTasks { handle, tasks }) => {
                        state.handle_instance_tasks(handle, tasks, &frontend);
                    }
                    Some(BackendEvent::InstallFinished { handle, is_run, result }) => {
                        let launch_after = if is_run {
                            state.launch_after_install.remove(&handle);
                            None
                        } else {
                            state.launch_after_install.remove(&handle)
                        };
                        let should_launch = launch_after == Some(true) && result.is_ok();
                        let prepared_java = result.as_ref().ok().map(|output| output.java.clone());
                        state
                            .handle_install_finished(handle.clone(), is_run, result, &frontend)
                            .await;
                        if should_launch {
                            state.start_launch(
                                handle,
                                None,
                                true,
                                prepared_java,
                                frontend.clone(),
                                internal_sender.clone(),
                            );
                            state.emit_snapshot(&frontend);
                        }
                    }
                    Some(BackendEvent::LaunchStarted { handle }) => {
                        state.handle_launch_started(handle, &frontend);
                    }
                    Some(BackendEvent::AccountUpdated { provider, account }) => {
                        state.handle_launch_account_updated(provider, account, &frontend).await;
                    }
                    Some(BackendEvent::LaunchFinished { handle, exit }) => {
                        state.handle_launch_finished(handle, exit, &frontend);
                    }
                    Some(BackendEvent::AddAccountFinished { result }) => {
                        state.handle_add_account_finished(result, &frontend).await;
                    }
                    Some(BackendEvent::AddAccountCancelled) => {
                        state.handle_add_account_cancelled(&frontend);
                    }
                    Some(BackendEvent::AuthLaunchPrompt { instance }) => {
                        state.handle_auth_launch_prompt(instance);
                    }
                    Some(BackendEvent::JavaResolved { instance, path }) => {
                        state.handle_java_resolved(instance, path, &frontend).await;
                    }
                    None => break,
                }
            }
        }
    }

    frontend.send(MessageToFrontend::Quit);
    Ok(())
}
