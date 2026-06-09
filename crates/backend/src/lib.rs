mod catalog;
mod install;
pub mod instances;
mod launch;
mod local;
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
        InstanceId, InstanceStorage, InstanceUserSettings, LocalInstance, RemoteSource,
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
    AccountView, BackendReceiver, BackendStatus, FrontendSender, LauncherSettingsView,
    MessageToBackend, MessageToFrontend, NotificationLevel, ProgressStage,
};
use launcher_build_config::default_instance_manifest_urls;
use launcher_i18n::{detect_system_language_code, resolve_language_code, set_lang};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use url::Url;
use utils::paths::{DataDir, InstanceDirFS, InstancesDir};

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
    async fn load(launcher_dir: &Path) -> anyhow::Result<Self> {
        let path = launcher_dir.join(SETTINGS_FILE);
        if !path.exists() {
            let mut settings = Self {
                backend_urls: default_instance_manifest_urls(),
                hide_window_after_launch: false,
                hide_usernames_in_cards: false,
                language: None,
            };
            settings.ensure_language_resolved().await?;
            settings.save(launcher_dir).await?;
            return Ok(settings);
        }

        let bytes = tokio::fs::read(path).await?;
        let mut settings: Self = serde_json::from_slice(&bytes)?;
        if settings.ensure_language_resolved().await? {
            settings.save(launcher_dir).await?;
        }
        Ok(settings)
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
        tokio::fs::create_dir_all(launcher_dir).await?;
        let bytes = serde_json::to_vec_pretty(self)?;
        tokio::fs::write(launcher_dir.join(SETTINGS_FILE), bytes).await?;
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
    installing: HashMap<InstanceId, instances::InstallProgressView>,
    creating_local: HashMap<InstanceId, Arc<str>>,
    creating_local_params: HashMap<InstanceId, local::CreateLocalParams>,
    install_tasks: HashMap<InstanceId, JoinHandle<()>>,
    install_errors: HashMap<InstanceId, Arc<str>>,
    launching: HashSet<InstanceId>,
    java_prep_tasks: HashSet<InstanceId>,
    running: HashSet<InstanceId>,
    launch_tasks: HashMap<InstanceId, LaunchHandle>,
    launch_errors: HashMap<InstanceId, Arc<str>>,
}

struct LaunchHandle {
    kill: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

enum BackendEvent {
    FetchFinished {
        url: Url,
        result: CatalogFetchResult,
    },
    InstallProgress {
        id: InstanceId,
        stage: ProgressStage,
        current: u64,
        total: u64,
        message: Arc<str>,
        show_bar: bool,
    },
    InstallFinished {
        id: InstanceId,
        is_run: bool,
        result: Result<install::InstallOutput, Arc<str>>,
    },
    ModSyncFinished {
        id: InstanceId,
        result: Result<(), Arc<str>>,
    },
    LaunchPrepFinished {
        id: InstanceId,
    },
    LaunchStarted {
        id: InstanceId,
    },
    LaunchAccountUpdated {
        provider: AuthProviderConfig,
        account: AccountData,
    },
    LaunchFinished {
        id: InstanceId,
        exit: launcher_bridge::ExitOutcome,
    },
    AddAccountFinished {
        result: Result<(AuthProviderConfig, AccountData), Arc<str>>,
    },
    JavaResolved {
        instance: InstanceId,
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
        self.frontend.send(MessageToFrontend::AuthPrompt(message));
    }

    async fn get_message(&self) -> Option<AuthMessage> {
        self.message.lock().ok().and_then(|message| message.clone())
    }

    async fn clear(&self) {
        if let Ok(mut message) = self.message.lock() {
            *message = None;
        }
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
        let settings = Settings::load(&launcher_dir).await?;
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
        let instance_storage = InstanceStorage::load(&data_dir)
            .await
            .unwrap_or_else(|err| {
                log::warn!("Failed to load local instance storage: {err:?}");
                InstanceStorage::empty()
            });
        let auth_storage = AuthStorage::load(launcher_dir.join("auth_data.json"))
            .unwrap_or_else(|_| AuthStorage::empty(launcher_dir.join("auth_data.json")));

        Ok(Self {
            launcher_dir,
            settings,
            instance_storage,
            auth_storage,
            catalogs,
            client: reqwest::Client::new(),
            installing: HashMap::new(),
            creating_local: HashMap::new(),
            creating_local_params: HashMap::new(),
            install_tasks: HashMap::new(),
            install_errors: HashMap::new(),
            launching: HashSet::new(),
            java_prep_tasks: HashSet::new(),
            running: HashSet::new(),
            launch_tasks: HashMap::new(),
            launch_errors: HashMap::new(),
        })
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
        instances::build_instance_views(&instances::InstanceViewBuildInput {
            local_instances: self.instance_storage.all(),
            catalogs: &self.catalogs,
            live_state: instances::InstanceLiveState {
                installing: &self.installing,
                creating_local: &self.creating_local,
                install_errors: &self.install_errors,
                launching: &self.launching,
                running: &self.running,
                launch_errors: &self.launch_errors,
            },
            local_metadata: &local_metadata,
            user_settings: &instance_settings,
            accounts: &account_views,
        })
        .into()
    }

    fn instance_settings_views(&self) -> HashMap<InstanceId, instances::InstanceUserSettingsView> {
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
                    local.id.clone(),
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

    fn local_metadata_views(&self) -> HashMap<InstanceId, instances::LocalMetadataView> {
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
                    local.id.clone(),
                    instances::LocalMetadataView {
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

    fn load_settings_for_id(&self, id: &InstanceId) -> InstanceUserSettings {
        let Some(instance) = self.instance_storage.get(id) else {
            return InstanceUserSettings::default();
        };
        let instance_dir = self.instance_dir_fs(instance);
        match std::fs::read(instance_dir.settings_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => InstanceUserSettings::default(),
        }
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

    fn refresh_all(&mut self, internal: &mpsc::UnboundedSender<BackendEvent>, tx: &FrontendSender) {
        for (url, _, _) in self.visible_backend_urls() {
            self.start_fetch(url, internal);
        }
        self.emit_snapshot(tx);
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

        let id = InstanceId::local_new();
        self.install_errors.remove(&id);
        self.creating_local
            .insert(id.clone(), Arc::from(dir_name.clone()));
        self.creating_local_params.insert(
            id.clone(),
            local::CreateLocalParams {
                dir_name: dir_name.clone(),
                minecraft_version: minecraft_version.clone(),
                loader,
                loader_version: loader_version.clone(),
            },
        );
        self.installing.insert(
            id.clone(),
            instances::InstallProgressView {
                stage: ProgressStage::Metadata,
                current: 0,
                total: 0,
                message: Arc::from(launcher_i18n::notifications::preparing_local_instance()),
                show_bar: false,
            },
        );

        let request = local::CreateLocalRequest {
            id: id.clone(),
            dir_name,
            minecraft_version,
            loader,
            loader_version,
            launcher_dir: self.launcher_dir.clone(),
            client: self.client.clone(),
            frontend: tx.clone(),
            internal: internal.clone(),
        };

        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            let result = local::create_local_instance(request).await;
            let _ = internal.send(BackendEvent::InstallFinished {
                id: task_id,
                is_run: false,
                result,
            });
        });
        self.install_tasks.insert(id, handle);
    }

    fn prepare_install(
        &self,
        id: InstanceId,
        is_run: bool,
        force_overwrite: bool,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) -> install::InstallRequest {
        let optional_mod_preferences = self.load_settings_for_id(&id).optional_mod_sets;
        install::InstallRequest {
            id,
            cause: if is_run {
                InstallCause::Run
            } else {
                InstallCause::Update
            },
            force_overwrite,
            optional_mod_preferences,
            launcher_dir: DataDir::new(self.launcher_dir.clone()),
            client: self.client.clone(),
            local_instances: self.instance_storage.all().to_vec(),
            catalogs: self.catalogs.clone(),
            frontend: tx,
            internal,
        }
    }

    fn start_install(
        &mut self,
        id: InstanceId,
        force_overwrite: bool,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.install_tasks.contains_key(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Info,
                message: Arc::from(launcher_i18n::notifications::install_already_running()),
            });
            return;
        }

        self.install_errors.remove(&id);
        self.installing.insert(
            id.clone(),
            instances::InstallProgressView {
                stage: ProgressStage::Metadata,
                current: 0,
                total: 0,
                message: Arc::from(launcher_i18n::notifications::preparing_install()),
                show_bar: false,
            },
        );

        let request =
            self.prepare_install(id.clone(), false, force_overwrite, tx, internal.clone());
        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            let result = install::install_instance(request)
                .await
                .map_err(|err| Arc::<str>::from(format!("{err:#}")));
            let _ = internal.send(BackendEvent::InstallFinished {
                id: task_id,
                is_run: false,
                result,
            });
        });
        self.install_tasks.insert(id, handle);
    }

    async fn handle_install_finished(
        &mut self,
        id: InstanceId,
        is_run: bool,
        result: Result<install::InstallOutput, Arc<str>>,
        tx: &FrontendSender,
    ) {
        if !is_run {
            self.install_tasks.remove(&id);
            self.installing.remove(&id);
        }

        match result {
            Ok(output) => {
                self.creating_local.remove(&id);
                self.creating_local_params.remove(&id);
                let data_dir = DataDir::new(self.launcher_dir.clone());
                let save_result = if self.instance_storage.get(&output.instance.id).is_some() {
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
                        self.install_errors.remove(&output.instance.id);
                        if !is_run {
                            tx.send(MessageToFrontend::Notification {
                                level: NotificationLevel::Success,
                                message: Arc::from(
                                    launcher_i18n::notifications::install_completed(),
                                ),
                            });
                        }
                    }
                    Err(err) => {
                        log::error!(
                            "Failed to save installed instance {}: {err:#}",
                            output.instance.id
                        );
                        let error = Arc::<str>::from(err.to_string());
                        self.install_errors
                            .insert(output.instance.id.clone(), error.clone());
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
                log::error!("Install task for instance {id} failed: {error}");
                self.install_errors.insert(id, error.clone());
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

    fn cancel_install(&mut self, id: InstanceId, tx: &FrontendSender) {
        if self.java_prep_tasks.remove(&id) {
            if let Some(handle) = self.launch_tasks.remove(&id) {
                handle.task.abort();
            }
            self.installing.remove(&id);
            self.emit_snapshot(tx);
            return;
        }

        if let Some(handle) = self.install_tasks.remove(&id) {
            handle.abort();
        }
        self.installing.remove(&id);
        let params = self.creating_local_params.remove(&id);
        let dir_name = self
            .creating_local
            .remove(&id)
            .map(|name| name.to_string())
            .or_else(|| params.as_ref().map(|params| params.dir_name.clone()));
        self.install_errors.remove(&id);

        if let Some(dir_name) = dir_name
            && self.instance_storage.get(&id).is_none()
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
        id: InstanceId,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.install_tasks.contains_key(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Info,
                message: Arc::from(launcher_i18n::notifications::install_already_running()),
            });
            return;
        }

        let Some(params) = self.creating_local_params.get(&id).cloned() else {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::from(launcher_i18n::notifications::install_failed(
                    "no stored create parameters for retry".to_string(),
                )),
            });
            return;
        };

        self.install_errors.remove(&id);
        self.creating_local
            .insert(id.clone(), Arc::from(params.dir_name.clone()));
        self.installing.insert(
            id.clone(),
            instances::InstallProgressView {
                stage: ProgressStage::Metadata,
                current: 0,
                total: 0,
                message: Arc::from(launcher_i18n::notifications::preparing_local_instance()),
                show_bar: false,
            },
        );

        let request = local::CreateLocalRequest {
            id: id.clone(),
            dir_name: params.dir_name,
            minecraft_version: params.minecraft_version,
            loader: params.loader,
            loader_version: params.loader_version,
            launcher_dir: self.launcher_dir.clone(),
            client: self.client.clone(),
            frontend: tx.clone(),
            internal: internal.clone(),
        };

        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            let result = local::create_local_instance(request).await;
            let _ = internal.send(BackendEvent::InstallFinished {
                id: task_id,
                is_run: false,
                result,
            });
        });
        self.install_tasks.insert(id, handle);
    }

    async fn delete_instance(&mut self, id: InstanceId, tx: &FrontendSender) {
        if self.running.contains(&id) || self.launching.contains(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::stop_before_delete()),
            });
            return;
        }
        if self.install_tasks.contains_key(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::cancel_install_before_delete()),
            });
            return;
        }
        let data_dir = DataDir::new(self.launcher_dir.clone());
        match self.instance_storage.remove_from_disk(&data_dir, &id).await {
            Ok(Some(_)) => {
                self.install_errors.remove(&id);
                self.launch_errors.remove(&id);
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Success,
                    message: Arc::from(launcher_i18n::notifications::instance_deleted()),
                });
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
                log::error!("Failed to delete instance {id}: {err:#}");
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

        let auth_prompt = Arc::new(AuthPromptReporter::new(tx));
        tokio::spawn(async move {
            let result = perform_auth(None, provider.clone(), auth_prompt)
                .await
                .map(|account| (provider, account))
                .map_err(|err| Arc::<str>::from(format!("{err:#}")));
            let _ = internal.send(BackendEvent::AddAccountFinished { result });
        });
    }

    fn submit_offline_nickname(&mut self, nickname: String, tx: &FrontendSender) {
        let nickname = nickname.trim();
        if nickname.is_empty() {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::offline_nickname_empty()),
            });
            return;
        }

        let (key, provider, account) = launch::offline_account(nickname);
        match self.auth_storage.insert_account(&provider, account) {
            Ok(_) => {
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Success,
                    message: Arc::from(launcher_i18n::notifications::added_offline_account(
                        key.1.clone(),
                    )),
                });
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

    fn remove_account(&mut self, key: AccountKey, tx: &FrontendSender) {
        match self.auth_storage.delete_account(key.0, &key.1) {
            Ok(()) => {
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Success,
                    message: Arc::from(launcher_i18n::notifications::account_removed()),
                });
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
        id: &InstanceId,
    ) -> anyhow::Result<LocalInstance> {
        if let Some(local) = self.instance_storage.get(id) {
            return Ok(local.clone());
        }

        for (url, state) in &self.catalogs {
            let Some(manifest) = state.manifest() else {
                continue;
            };
            for entry in &manifest.instances {
                if instances::remote_entry_id(url, &entry.name) == *id {
                    let source = RemoteSource {
                        manifest_url: url.clone(),
                        name_in_manifest: entry.name.clone(),
                    };
                    let local = LocalInstance::new_pending_remote(
                        id.clone(),
                        self.instance_storage.allocate_dir_name(&entry.name),
                        source,
                    );
                    let data_dir = DataDir::new(self.launcher_dir.clone());
                    self.instance_storage.add(&data_dir, local.clone()).await?;
                    return Ok(local);
                }
            }
        }

        Err(anyhow::anyhow!(
            "instance {id} was not found in local storage or fetched catalogs"
        ))
    }

    async fn update_instance_settings(
        &mut self,
        id: &InstanceId,
        update: impl FnOnce(&mut InstanceUserSettings),
    ) -> anyhow::Result<InstanceUserSettings> {
        let local = self.ensure_instance_for_settings(id).await?;
        let instance_dir = self.instance_dir_fs(&local);
        let mut settings = load_instance_settings(&instance_dir).await?;
        update(&mut settings);
        save_instance_settings(&instance_dir, &settings).await?;
        Ok(settings)
    }

    fn required_provider_for_instance(&self, instance: &InstanceId) -> Option<AuthProviderConfig> {
        self.build_instance_views()
            .iter()
            .find(|view| &view.id == instance)
            .and_then(|view| view.auth_provider.clone())
    }

    fn handle_add_account_finished(
        &mut self,
        result: Result<(AuthProviderConfig, AccountData), Arc<str>>,
        tx: &FrontendSender,
    ) {
        match result {
            Ok((provider, account)) => match self.auth_storage.insert_account(&provider, account) {
                Ok((_, username)) => {
                    tx.send(MessageToFrontend::Notification {
                        level: NotificationLevel::Success,
                        message: Arc::from(launcher_i18n::notifications::added_account(username)),
                    });
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
            },
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

    async fn set_instance_account_override(
        &mut self,
        instance: InstanceId,
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
        instance: InstanceId,
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
        instance: InstanceId,
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
        instance: InstanceId,
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

    fn is_local_install_in_progress(&self, instance: &InstanceId) -> bool {
        self.creating_local.contains_key(instance)
    }

    async fn set_optional_mod_set_enabled(
        &mut self,
        instance: InstanceId,
        set_id: String,
        enabled: bool,
        tx: &FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.install_tasks.contains_key(&instance) {
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
        let Some(local) = self
            .instance_storage
            .all()
            .iter()
            .find(|entry| entry.id == instance && entry.is_installed())
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

        let settings = match self
            .update_instance_settings(&instance, |settings| {
                settings.optional_mod_sets.insert(set_id, enabled);
            })
            .await
        {
            Ok(settings) => settings,
            Err(err) => {
                log::error!(
                    "Failed to save optional mod set setting for instance {instance}: {err:#}"
                );
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::from(launcher_i18n::notifications::failed_save_optional_mod(
                        err.to_string(),
                    )),
                });
                return;
            }
        };
        self.emit_snapshot(tx);

        let optional_mod_preferences = settings.optional_mod_sets.clone();
        self.install_errors.remove(&instance);
        self.installing.insert(
            instance.clone(),
            instances::InstallProgressView {
                stage: ProgressStage::Files,
                current: 0,
                total: 0,
                message: Arc::from(launcher_i18n::progress::syncing_optional_mods()),
                show_bar: false,
            },
        );

        let client = self.client.clone();
        let task_instance = instance.clone();
        let frontend = tx.clone();
        let handle = tokio::spawn(async move {
            let result = install::sync_instance_mods(
                &client,
                data_dir,
                &dir_name,
                task_instance.clone(),
                optional_mod_preferences,
                frontend,
                internal.clone(),
            )
            .await
            .map_err(|err| Arc::<str>::from(format!("{err:#}")));
            let _ = internal.send(BackendEvent::ModSyncFinished {
                id: task_instance,
                result,
            });
        });
        self.install_tasks.insert(instance, handle);
    }

    async fn handle_mod_sync_finished(
        &mut self,
        id: InstanceId,
        result: Result<(), Arc<str>>,
        tx: &FrontendSender,
    ) {
        self.install_tasks.remove(&id);
        self.installing.remove(&id);
        if let Err(err) = result {
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
        instance: InstanceId,
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
        instance: InstanceId,
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
        let Some(required_version) = self.required_java_version_for(&instance) else {
            log::error!("Missing required Java version for instance {instance}");
            return;
        };
        if let Some(ref path_str) = path {
            let java_path = std::path::Path::new(path_str);
            if !utils::java::check_java(&required_version, java_path).await {
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::from(launcher_i18n::notifications::invalid_java_path()),
                });
                return;
            }
        }
        let is_set = path.is_some();
        if let Err(err) = self
            .update_instance_settings(&instance, |settings| settings.java_path = path)
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
        let message = if is_set {
            launcher_i18n::notifications::java_path_set().to_owned()
        } else {
            launcher_i18n::notifications::java_path_cleared().to_owned()
        };
        tx.send(MessageToFrontend::Notification {
            level: NotificationLevel::Success,
            message: Arc::from(message),
        });
        self.emit_snapshot(tx);
    }

    fn required_java_version_for(&self, instance: &InstanceId) -> Option<String> {
        if self.is_local_install_in_progress(instance) {
            return None;
        }
        self.build_instance_views()
            .iter()
            .find(|v| &v.id == instance)
            .and_then(|v| v.required_java_version.as_deref().map(str::to_owned))
    }

    fn resolve_java_path(
        &self,
        instance: InstanceId,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.is_local_install_in_progress(&instance) {
            return;
        }
        let Some(required_version) = self.required_java_version_for(&instance) else {
            log::error!("Missing required Java version for instance {instance}");
            return;
        };
        let data_dir = utils::paths::DataDir::new(self.launcher_dir.clone());
        tokio::spawn(async move {
            let path = utils::java::get_java(&required_version, &data_dir)
                .await
                .map(|installation| Arc::<str>::from(installation.path.to_string_lossy().as_ref()));
            let _ = internal.send(BackendEvent::JavaResolved { instance, path });
        });
    }

    fn start_launch(
        &mut self,
        id: InstanceId,
        account: Option<AccountKey>,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.launching.contains(&id)
            || self.running.contains(&id)
            || self.java_prep_tasks.contains(&id)
        {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Info,
                message: Arc::from(launcher_i18n::notifications::already_launching_or_running()),
            });
            return;
        }

        let Some(local) = self.instance_storage.get(&id) else {
            return;
        };
        if !local.is_installed() {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(launcher_i18n::notifications::instance_not_installed_locally()),
            });
            return;
        }

        let settings = self.load_settings_for_id(&id);
        let configured_override = settings.account_override.clone();
        let selected_account = settings.selected_account.clone();
        let bypass_required_provider = account.is_none() && configured_override.is_some();
        let account = account.or(configured_override).or(selected_account);
        if account.is_none()
            && let Some(view) = self
                .build_instance_views()
                .iter()
                .find(|view| view.id == id)
            && let Some(reason) = &view.launch_blocked_reason
        {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: reason.clone(),
            });
            return;
        }

        self.launch_errors.remove(&id);
        self.java_prep_tasks.insert(id.clone());
        self.installing.insert(
            id.clone(),
            instances::InstallProgressView {
                stage: launcher_bridge::ProgressStage::Java,
                current: 0,
                total: 0,
                message: Arc::from(launcher_i18n::progress::installing_java()),
                show_bar: false,
            },
        );
        self.emit_snapshot(&tx);

        let java_path = settings.java_path.clone();
        let install_request =
            self.prepare_install(id.clone(), true, false, tx.clone(), internal.clone());
        let xmx_mb = settings.xmx_mb;
        let jvm_flags = settings.jvm_flags.clone();
        let use_native_glfw = settings.use_native_glfw;
        let launcher_dir = self.launcher_dir.clone();
        let local_instances = self.instance_storage.all().to_vec();
        let account_entries = self.launch_accounts();
        let frontend = tx.clone();
        let (kill_tx, mut kill_rx) = oneshot::channel();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            let install_result = install::install_instance(install_request)
                .await
                .map_err(|err| Arc::<str>::from(format!("{err:#}")));
            let _ = internal.send(BackendEvent::InstallFinished {
                id: task_id.clone(),
                is_run: true,
                result: install_result.clone(),
            });
            if let Err(err) = install_result {
                log::error!("Failed to update instance {task_id} on launch: {err}");
                let _ = internal.send(BackendEvent::LaunchPrepFinished {
                    id: task_id.clone(),
                });
                let _ = internal.send(BackendEvent::LaunchFinished {
                    id: task_id,
                    exit: launcher_bridge::ExitOutcome::Error(err),
                });
                return;
            }
            let launch_id = task_id.clone();
            let launch_result = async {
                let local = local_instances
                    .iter()
                    .find(|instance| instance.id == launch_id)
                    .ok_or_else(|| launch::LaunchError::InstanceNotFound(launch_id.clone()))?;
                let data_dir = DataDir::new(launcher_dir.clone());
                let instance_dir = InstancesDir::root()
                    .instance_dir(&local.dir_name)
                    .with_data_dir(data_dir.clone());
                let metadata = launch::read_metadata(&instance_dir).await?;
                let progress = install::BackendProgressReporter::new(
                    launch_id.clone(),
                    frontend.clone(),
                    internal.clone(),
                );
                let java =
                    install::resolve_java(&metadata, &data_dir, java_path.as_deref(), &progress)
                        .await?;
                let _ = internal.send(BackendEvent::LaunchPrepFinished {
                    id: launch_id.clone(),
                });
                launch::launch_instance(launch::LaunchRequest {
                    id: launch_id.clone(),
                    account,
                    bypass_required_provider,
                    xmx_mb,
                    jvm_flags,
                    java,
                    use_native_glfw,
                    launcher_dir,
                    local_instances,
                    account_entries,
                    frontend,
                })
                .await
            }
            .await;

            match launch_result {
                Ok(start) => {
                    if let Some((provider, account)) = start.refreshed_account {
                        let _ =
                            internal.send(BackendEvent::LaunchAccountUpdated { provider, account });
                    }
                    let _ = internal.send(BackendEvent::LaunchStarted {
                        id: launch_id.clone(),
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
                        id: launch_id.clone(),
                        exit,
                    });
                }
                Err(err) => {
                    log::error!("Failed to launch instance {launch_id}: {err:#}");
                    let _ = internal.send(BackendEvent::LaunchPrepFinished {
                        id: launch_id.clone(),
                    });
                    let _ = internal.send(BackendEvent::LaunchFinished {
                        id: launch_id.clone(),
                        exit: launcher_bridge::ExitOutcome::Error(Arc::<str>::from(format!(
                            "{err:#}"
                        ))),
                    });
                }
            }
        });
        self.launch_tasks.insert(
            id,
            LaunchHandle {
                kill: Some(kill_tx),
                task,
            },
        );
    }

    fn handle_launch_prep_finished(&mut self, id: InstanceId, tx: &FrontendSender) {
        self.java_prep_tasks.remove(&id);
        self.installing.remove(&id);
        self.launching.insert(id);
        self.emit_snapshot(tx);
    }

    fn handle_launch_started(&mut self, id: InstanceId, tx: &FrontendSender) {
        self.launching.remove(&id);
        self.running.insert(id.clone());
        tx.send(MessageToFrontend::InstanceProgress {
            id: id.clone(),
            stage: ProgressStage::Launch,
            current: 1,
            total: 1,
            message: Arc::from(launcher_i18n::notifications::minecraft_running()),
        });
        self.emit_snapshot(tx);
    }

    fn handle_launch_account_updated(
        &mut self,
        provider: AuthProviderConfig,
        account: AccountData,
        tx: &FrontendSender,
    ) {
        if let Err(err) = self.auth_storage.insert_account(&provider, account) {
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
        id: InstanceId,
        exit: launcher_bridge::ExitOutcome,
        tx: &FrontendSender,
    ) {
        if let Some(mut handle) = self.launch_tasks.remove(&id) {
            handle.kill.take();
        }
        self.java_prep_tasks.remove(&id);
        self.installing.remove(&id);
        self.launching.remove(&id);
        self.running.remove(&id);
        match &exit {
            launcher_bridge::ExitOutcome::Success | launcher_bridge::ExitOutcome::Terminated => {
                self.launch_errors.remove(&id);
            }
            launcher_bridge::ExitOutcome::ExitCode(code) => {
                self.launch_errors.insert(
                    id.clone(),
                    Arc::from(launcher_i18n::notifications::minecraft_exited_with_code(
                        *code,
                    )),
                );
            }
            launcher_bridge::ExitOutcome::Error(error) => {
                self.launch_errors.insert(id.clone(), error.clone());
            }
        }
        tx.send(MessageToFrontend::LaunchFinished {
            instance: id,
            exit: exit.clone(),
        });
        self.emit_snapshot(tx);
    }

    fn kill_launch(&mut self, id: InstanceId, tx: &FrontendSender) {
        if let Some(handle) = self.launch_tasks.get_mut(&id)
            && let Some(kill) = handle.kill.take()
        {
            let _ = kill.send(());
        }
        if self.launching.contains(&id) {
            if let Some(handle) = self.launch_tasks.remove(&id) {
                handle.task.abort();
            }
            self.launching.remove(&id);
            tx.send(MessageToFrontend::LaunchFinished {
                instance: id.clone(),
                exit: launcher_bridge::ExitOutcome::Terminated,
            });
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
    state.refresh_all(&internal_sender, &frontend);

    loop {
        tokio::select! {
            message = receiver.recv() => {
                let Some(message) = message else {
                    break;
                };
                match message {
                    MessageToBackend::Refresh => {
                        state.refresh_all(&internal_sender, &frontend);
                    }
                    MessageToBackend::InstallInstance { id, force_overwrite } => {
                        state.start_install(id, force_overwrite, frontend.clone(), internal_sender.clone());
                        state.emit_snapshot(&frontend);
                    }
                    MessageToBackend::CancelInstall(id) => {
                        state.cancel_install(id, &frontend);
                    }
                    MessageToBackend::RetryCreateLocal(id) => {
                        state.retry_create_local(id, frontend.clone(), internal_sender.clone());
                        state.emit_snapshot(&frontend);
                    }
                    MessageToBackend::DeleteInstance(id) => {
                        state.delete_instance(id, &frontend).await;
                    }
                    MessageToBackend::Launch { instance, account } => {
                        state.start_launch(instance, account, frontend.clone(), internal_sender.clone());
                        state.emit_snapshot(&frontend);
                    }
                    MessageToBackend::KillInstance(id) => {
                        state.kill_launch(id, &frontend);
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
                    MessageToBackend::SubmitOfflineNickname(nickname) => {
                        state.submit_offline_nickname(nickname, &frontend);
                    }
                    MessageToBackend::RemoveAccount(account) => {
                        state.remove_account(account, &frontend);
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
                                internal_sender.clone(),
                            )
                            .await;
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
                    Some(BackendEvent::InstallProgress { id, stage, current, total, message, show_bar }) => {
                        if state.install_tasks.contains_key(&id)
                            || state.java_prep_tasks.contains(&id)
                        {
                            state.installing.insert(id, instances::InstallProgressView {
                                stage,
                                current,
                                total,
                                message,
                                show_bar,
                            });
                        }
                    }
                    Some(BackendEvent::LaunchPrepFinished { id }) => {
                        state.handle_launch_prep_finished(id, &frontend);
                    }
                    Some(BackendEvent::InstallFinished { id, is_run, result }) => {
                        state.handle_install_finished(id, is_run, result, &frontend).await;
                    }
                    Some(BackendEvent::ModSyncFinished { id, result }) => {
                        state.handle_mod_sync_finished(id, result, &frontend).await;
                    }
                    Some(BackendEvent::LaunchStarted { id }) => {
                        state.handle_launch_started(id, &frontend);
                    }
                    Some(BackendEvent::LaunchAccountUpdated { provider, account }) => {
                        state.handle_launch_account_updated(provider, account, &frontend);
                    }
                    Some(BackendEvent::LaunchFinished { id, exit }) => {
                        state.handle_launch_finished(id, exit, &frontend);
                    }
                    Some(BackendEvent::AddAccountFinished { result }) => {
                        state.handle_add_account_finished(result, &frontend);
                    }
                    Some(BackendEvent::JavaResolved { instance, path }) => {
                        frontend.send(MessageToFrontend::JavaPathResolved { instance, path });
                    }
                    None => break,
                }
            }
        }
    }

    frontend.send(MessageToFrontend::Quit);
    Ok(())
}
