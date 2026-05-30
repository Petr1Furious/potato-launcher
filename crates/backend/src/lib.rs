mod catalog;
mod install;
pub mod instances;
mod launch;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use catalog::{BackendCatalogState, backend_status, fetch_backend_catalog};
use instance::{instance_metadata::InstanceMetadata, storage::InstanceStorage};
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
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use url::Url;
use utils::paths::{DataDir, InstancesDir};
use uuid::Uuid;

const SETTINGS_FILE: &str = "settings.json";
const INSTANCE_SETTINGS_FILE: &str = "instance_settings.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub backend_urls: Vec<Url>,
    #[serde(default)]
    pub hide_window_after_launch: bool,
    #[serde(default)]
    pub hide_usernames_in_cards: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InstanceUserSettings {
    #[serde(default)]
    pub instances: HashMap<Uuid, InstanceUserSettingsEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InstanceUserSettingsEntry {
    #[serde(default)]
    pub selected_account: Option<AccountKey>,
    #[serde(default)]
    pub account_override: Option<AccountKey>,
    #[serde(default)]
    pub xmx_mb: Option<u64>,
    #[serde(default)]
    pub jvm_flags: Option<String>,
}

impl InstanceUserSettings {
    async fn load(launcher_dir: &Path) -> anyhow::Result<Self> {
        let path = launcher_dir.join(INSTANCE_SETTINGS_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }

        let bytes = tokio::fs::read(path).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn save(&self, launcher_dir: &Path) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(launcher_dir).await?;
        let bytes = serde_json::to_vec_pretty(self)?;
        tokio::fs::write(launcher_dir.join(INSTANCE_SETTINGS_FILE), bytes).await?;
        Ok(())
    }

    fn entry_mut(&mut self, instance: Uuid) -> &mut InstanceUserSettingsEntry {
        self.instances.entry(instance).or_default()
    }
}

impl Settings {
    async fn load(launcher_dir: &Path) -> anyhow::Result<Self> {
        let path = launcher_dir.join(SETTINGS_FILE);
        if !path.exists() {
            let settings = Self {
                backend_urls: default_instance_manifest_urls(),
                hide_window_after_launch: false,
                hide_usernames_in_cards: false,
            };
            settings.save(launcher_dir).await?;
            return Ok(settings);
        }

        let bytes = tokio::fs::read(path).await?;
        Ok(serde_json::from_slice(&bytes)?)
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
    instance_settings: InstanceUserSettings,
    instance_storage: InstanceStorage,
    auth_storage: AuthStorage,
    catalogs: HashMap<Url, BackendCatalogState>,
    client: reqwest::Client,
    installing: HashMap<Uuid, instances::InstallProgressView>,
    install_tasks: HashMap<Uuid, JoinHandle<()>>,
    install_errors: HashMap<Uuid, Arc<str>>,
    installed_overrides: HashSet<Uuid>,
    launching: HashSet<Uuid>,
    running: HashSet<Uuid>,
    launch_tasks: HashMap<Uuid, LaunchHandle>,
    launch_errors: HashMap<Uuid, Arc<str>>,
}

struct LaunchHandle {
    kill: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

enum BackendEvent {
    FetchFinished {
        url: Url,
        state: BackendCatalogState,
    },
    InstallProgress {
        id: Uuid,
        stage: ProgressStage,
        current: u64,
        total: u64,
        message: Arc<str>,
        show_bar: bool,
    },
    InstallFinished {
        id: Uuid,
        result: Result<install::InstallOutput, Arc<str>>,
    },
    LaunchStarted {
        id: Uuid,
    },
    LaunchAccountUpdated {
        provider: AuthProviderConfig,
        account: AccountData,
    },
    LaunchFinished {
        id: Uuid,
        exit: launcher_bridge::ExitOutcome,
    },
    AddAccountFinished {
        result: Result<(AuthProviderConfig, AccountData), Arc<str>>,
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
        let instance_settings = InstanceUserSettings::load(&launcher_dir)
            .await
            .unwrap_or_else(|err| {
                log::warn!("Failed to load instance user settings: {err:?}");
                InstanceUserSettings::default()
            });
        let catalogs = settings
            .backend_urls
            .iter()
            .cloned()
            .map(|url| (url, BackendCatalogState::NotFetched))
            .collect();
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
            instance_settings,
            instance_storage,
            auth_storage,
            catalogs,
            client: reqwest::Client::new(),
            installing: HashMap::new(),
            install_tasks: HashMap::new(),
            install_errors: HashMap::new(),
            installed_overrides: HashSet::new(),
            launching: HashSet::new(),
            running: HashSet::new(),
            launch_tasks: HashMap::new(),
            launch_errors: HashMap::new(),
        })
    }

    fn backend_statuses(&self) -> Arc<[BackendStatus]> {
        self.visible_backend_urls()
            .into_iter()
            .map(|(url, configured, referenced_by_instances)| {
                let state = self
                    .catalogs
                    .get(&url)
                    .unwrap_or(&BackendCatalogState::NotFetched);
                backend_status(&url, state, configured, referenced_by_instances)
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
                install_errors: &self.install_errors,
                installed_overrides: &self.installed_overrides,
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

    fn instance_settings_views(&self) -> HashMap<Uuid, instances::InstanceUserSettingsView> {
        self.instance_settings
            .instances
            .iter()
            .map(|(id, settings)| {
                (
                    *id,
                    instances::InstanceUserSettingsView {
                        selected_account: settings.selected_account.clone(),
                        account_override: settings.account_override.clone(),
                        xmx_mb: settings.xmx_mb,
                        jvm_flags: settings
                            .jvm_flags
                            .as_ref()
                            .map(|flags| Arc::<str>::from(flags.clone())),
                    },
                )
            })
            .collect()
    }

    fn launcher_settings_view(&self) -> LauncherSettingsView {
        LauncherSettingsView {
            hide_window_after_launch: self.settings.hide_window_after_launch,
            hide_usernames_in_cards: self.settings.hide_usernames_in_cards,
        }
    }

    fn local_metadata_views(&self) -> HashMap<Uuid, instances::LocalMetadataView> {
        let data_dir = DataDir::new(self.launcher_dir.clone());
        self.instance_storage
            .iter()
            .filter_map(|local| {
                let path = InstancesDir::root()
                    .instance_dir(&local.dir_name)
                    .meta_path()
                    .to_fs(&data_dir);
                let bytes = std::fs::read(path).ok()?;
                let metadata = serde_json::from_slice::<InstanceMetadata>(&bytes).ok()?;
                Some((
                    local.id,
                    instances::LocalMetadataView {
                        auth_provider: metadata.auth_backend.clone(),
                        default_xmx_mb: parse_xmx_mb(metadata.default_xmx.as_deref()),
                    },
                ))
            })
            .collect()
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
            self.catalogs.insert(url, BackendCatalogState::NotFetched);
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
            .insert(url.clone(), BackendCatalogState::Fetching);
        let client = self.client.clone();
        let internal = internal.clone();
        tokio::spawn(async move {
            let state = fetch_backend_catalog(client, url.clone()).await;
            let _ = internal.send(BackendEvent::FetchFinished { url, state });
        });
    }

    fn handle_fetch_finished(&mut self, url: Url, state: BackendCatalogState, tx: &FrontendSender) {
        self.catalogs.insert(url, state);
        self.emit_snapshot(tx);
    }

    fn start_install(
        &mut self,
        id: Uuid,
        force_overwrite: bool,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.install_tasks.contains_key(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Info,
                message: Arc::<str>::from("Install is already running"),
            });
            return;
        }

        self.install_errors.remove(&id);
        self.installing.insert(
            id,
            instances::InstallProgressView {
                stage: ProgressStage::Metadata,
                current: 0,
                total: 0,
                message: Arc::<str>::from("Preparing install"),
                show_bar: false,
            },
        );

        let request = install::InstallRequest {
            id,
            force_overwrite,
            launcher_dir: self.launcher_dir.clone(),
            client: self.client.clone(),
            local_instances: self.instance_storage.all().to_vec(),
            catalogs: self.catalogs.clone(),
            frontend: tx,
            internal: internal.clone(),
        };

        let handle = tokio::spawn(async move {
            let result = install::install_instance(request)
                .await
                .map_err(|err| Arc::<str>::from(format!("{err:#}")));
            let _ = internal.send(BackendEvent::InstallFinished { id, result });
        });
        self.install_tasks.insert(id, handle);
    }

    async fn handle_install_finished(
        &mut self,
        id: Uuid,
        result: Result<install::InstallOutput, Arc<str>>,
        tx: &FrontendSender,
    ) {
        self.install_tasks.remove(&id);
        self.installing.remove(&id);

        match result {
            Ok(output) => {
                let data_dir = DataDir::new(self.launcher_dir.clone());
                let save_result = if self.instance_storage.get(output.instance.id).is_some() {
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
                        if output.requested_id != output.instance.id
                            && let Some(settings) = self
                                .instance_settings
                                .instances
                                .remove(&output.requested_id)
                        {
                            self.instance_settings
                                .instances
                                .insert(output.instance.id, settings);
                            if let Err(err) = self.instance_settings.save(&self.launcher_dir).await
                            {
                                log::error!(
                                    "Failed to transfer settings from remote view {} to installed instance {}: {err:#}",
                                    output.requested_id,
                                    output.instance.id
                                );
                            }
                        }
                        self.install_errors.remove(&output.requested_id);
                        self.install_errors.remove(&output.instance.id);
                        tx.send(MessageToFrontend::Notification {
                            level: NotificationLevel::Success,
                            message: Arc::<str>::from("Instance install completed"),
                        });
                    }
                    Err(err) => {
                        log::error!(
                            "Failed to save installed instance {}: {err:#}",
                            output.instance.id
                        );
                        let error = Arc::<str>::from(err.to_string());
                        self.install_errors
                            .insert(output.requested_id, error.clone());
                        tx.send(MessageToFrontend::Notification {
                            level: NotificationLevel::Error,
                            message: Arc::<str>::from(format!(
                                "Failed to save installed instance: {error}"
                            )),
                        });
                    }
                }
            }
            Err(error) => {
                log::error!("Install task for instance {id} failed: {error}");
                self.install_errors.insert(id, error.clone());
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::<str>::from(format!("Install failed: {error}")),
                });
            }
        }

        self.emit_snapshot(tx);
    }

    fn cancel_install(&mut self, id: Uuid, tx: &FrontendSender) {
        if let Some(handle) = self.install_tasks.remove(&id) {
            handle.abort();
        }
        self.installing.remove(&id);
        self.emit_snapshot(tx);
    }

    async fn delete_instance(&mut self, id: Uuid, tx: &FrontendSender) {
        if self.running.contains(&id) || self.launching.contains(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::<str>::from("Stop the instance before deleting it"),
            });
            return;
        }
        if self.install_tasks.contains_key(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::<str>::from("Cancel the install before deleting this instance"),
            });
            return;
        }
        let data_dir = DataDir::new(self.launcher_dir.clone());
        match self.instance_storage.remove_from_disk(&data_dir, id).await {
            Ok(Some(_)) => {
                self.install_errors.remove(&id);
                self.launch_errors.remove(&id);
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Success,
                    message: Arc::<str>::from("Instance deleted"),
                });
            }
            Ok(None) => {
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Warning,
                    message: Arc::<str>::from("Instance is not installed locally"),
                });
            }
            Err(err) => {
                log::error!("Failed to delete instance {id}: {err:#}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::<str>::from(format!("Failed to delete instance: {err}")),
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
                message: Arc::<str>::from("Enter an offline nickname in the instance details"),
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
                message: Arc::<str>::from("Offline nickname cannot be empty"),
            });
            return;
        }

        let (key, provider, account) = launch::offline_account(nickname);
        match self.auth_storage.insert_account(&provider, account) {
            Ok(_) => {
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Success,
                    message: Arc::<str>::from(format!("Added offline account {}", key.1)),
                });
            }
            Err(err) => {
                log::error!("Failed to save offline account {key:?}: {err:#}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::<str>::from(format!("Failed to save offline account: {err}")),
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
                    message: Arc::<str>::from("Account removed"),
                });
            }
            Err(err) => {
                log::error!("Failed to remove account {key:?}: {err:#}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::<str>::from(format!("Failed to remove account: {err}")),
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

    fn required_provider_for_instance(&self, instance: Uuid) -> Option<AuthProviderConfig> {
        self.build_instance_views()
            .iter()
            .find(|view| view.id == instance)
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
                        message: Arc::<str>::from(format!("Added account {username}")),
                    });
                }
                Err(err) => {
                    log::error!("Failed to save authenticated account: {err:#}");
                    tx.send(MessageToFrontend::Notification {
                        level: NotificationLevel::Error,
                        message: Arc::<str>::from(format!("Failed to save account: {err}")),
                    });
                }
            },
            Err(error) => {
                log::error!("Authentication failed: {error}");
                tx.send(MessageToFrontend::Notification {
                    level: NotificationLevel::Error,
                    message: Arc::<str>::from(format!("Authentication failed: {error}")),
                });
            }
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_account_override(
        &mut self,
        instance: Uuid,
        account: Option<AccountKey>,
        tx: &FrontendSender,
    ) {
        if let Some(account) = &account
            && let Some(required) = self.required_provider_for_instance(instance)
            && self.account_provider(account).as_ref() == Some(&required)
        {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::<str>::from(
                    "Use Account selection for accounts from the required provider",
                ),
            });
            return;
        }
        self.instance_settings.entry_mut(instance).account_override = account;
        if let Err(err) = self.instance_settings.save(&self.launcher_dir).await {
            log::error!("Failed to save account override for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::<str>::from(format!("Failed to save account override: {err}")),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_selected_account(
        &mut self,
        instance: Uuid,
        account: Option<AccountKey>,
        tx: &FrontendSender,
    ) {
        if let Some(account) = &account
            && let Some(required) = self.required_provider_for_instance(instance)
            && self.account_provider(account).as_ref() != Some(&required)
        {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::<str>::from("Selected account must match the instance auth provider"),
            });
            return;
        }
        let clear_override = account.is_some();
        let entry = self.instance_settings.entry_mut(instance);
        entry.selected_account = account;
        if clear_override {
            entry.account_override = None;
        }
        if let Err(err) = self.instance_settings.save(&self.launcher_dir).await {
            log::error!("Failed to save selected account for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::<str>::from(format!("Failed to save selected account: {err}")),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_launcher_settings(&mut self, settings: LauncherSettingsView, tx: &FrontendSender) {
        self.settings.hide_window_after_launch = settings.hide_window_after_launch;
        self.settings.hide_usernames_in_cards = settings.hide_usernames_in_cards;
        if let Err(err) = self.settings.save(&self.launcher_dir).await {
            log::error!("Failed to save launcher settings: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::<str>::from(format!("Failed to save launcher settings: {err}")),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_memory(
        &mut self,
        instance: Uuid,
        xmx_mb: Option<u64>,
        tx: &FrontendSender,
    ) {
        self.instance_settings.entry_mut(instance).xmx_mb = xmx_mb;
        if let Err(err) = self.instance_settings.save(&self.launcher_dir).await {
            log::error!("Failed to save memory override for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::<str>::from(format!("Failed to save memory override: {err}")),
            });
        }
        self.emit_snapshot(tx);
    }

    async fn set_instance_jvm_flags(
        &mut self,
        instance: Uuid,
        flags: Option<String>,
        tx: &FrontendSender,
    ) {
        self.instance_settings.entry_mut(instance).jvm_flags =
            flags.and_then(|flags| (!flags.trim().is_empty()).then(|| flags.trim().to_string()));
        if let Err(err) = self.instance_settings.save(&self.launcher_dir).await {
            log::error!("Failed to save JVM flags for instance {instance}: {err:#}");
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Error,
                message: Arc::<str>::from(format!("Failed to save JVM flags: {err}")),
            });
        }
        self.emit_snapshot(tx);
    }

    fn start_launch(
        &mut self,
        id: Uuid,
        account: Option<AccountKey>,
        tx: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) {
        if self.launching.contains(&id) || self.running.contains(&id) {
            tx.send(MessageToFrontend::Notification {
                level: NotificationLevel::Info,
                message: Arc::<str>::from("Instance is already launching or running"),
            });
            return;
        }

        let settings = self.instance_settings.instances.get(&id);
        let configured_override = settings.and_then(|settings| settings.account_override.clone());
        let selected_account = settings.and_then(|settings| settings.selected_account.clone());
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
        self.launching.insert(id);
        let (kill_tx, mut kill_rx) = oneshot::channel();
        let request = launch::LaunchRequest {
            id,
            account,
            bypass_required_provider,
            xmx_mb: settings.and_then(|settings| settings.xmx_mb),
            jvm_flags: settings.and_then(|settings| settings.jvm_flags.clone()),
            launcher_dir: self.launcher_dir.clone(),
            local_instances: self.instance_storage.all().to_vec(),
            account_entries: self.launch_accounts(),
            frontend: tx,
        };
        let internal_for_task = internal.clone();
        let task = tokio::spawn(async move {
            match launch::launch_instance(request).await {
                Ok(start) => {
                    if let Some((provider, account)) = start.refreshed_account {
                        let _ = internal_for_task
                            .send(BackendEvent::LaunchAccountUpdated { provider, account });
                    }
                    let _ = internal_for_task.send(BackendEvent::LaunchStarted { id });
                    let mut child = start.child;
                    let exit = tokio::select! {
                        status = child.wait() => exit_outcome(status),
                        _ = &mut kill_rx => {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            launcher_bridge::ExitOutcome::Terminated
                        }
                    };
                    let _ = internal_for_task.send(BackendEvent::LaunchFinished { id, exit });
                }
                Err(err) => {
                    log::error!("Failed to launch instance {id}: {err:#}");
                    let _ = internal_for_task.send(BackendEvent::LaunchFinished {
                        id,
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

    fn handle_launch_started(&mut self, id: Uuid, tx: &FrontendSender) {
        self.launching.remove(&id);
        self.running.insert(id);
        tx.send(MessageToFrontend::InstanceProgress {
            id,
            stage: ProgressStage::Launch,
            current: 1,
            total: 1,
            message: Arc::<str>::from("Minecraft is running"),
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
                message: Arc::<str>::from(format!("Failed to save refreshed account: {err}")),
            });
        }
        self.emit_snapshot(tx);
    }

    fn handle_launch_finished(
        &mut self,
        id: Uuid,
        exit: launcher_bridge::ExitOutcome,
        tx: &FrontendSender,
    ) {
        if let Some(mut handle) = self.launch_tasks.remove(&id) {
            handle.kill.take();
        }
        self.launching.remove(&id);
        self.running.remove(&id);
        match &exit {
            launcher_bridge::ExitOutcome::Success | launcher_bridge::ExitOutcome::Terminated => {
                self.launch_errors.remove(&id);
            }
            launcher_bridge::ExitOutcome::ExitCode(code) => {
                self.launch_errors.insert(
                    id,
                    Arc::<str>::from(format!("Minecraft exited with code {code}")),
                );
            }
            launcher_bridge::ExitOutcome::Error(error) => {
                self.launch_errors.insert(id, error.clone());
            }
        }
        tx.send(MessageToFrontend::LaunchFinished {
            instance: id,
            exit: exit.clone(),
        });
        self.emit_snapshot(tx);
    }

    fn kill_launch(&mut self, id: Uuid, tx: &FrontendSender) {
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
                instance: id,
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
                                    message: Arc::<str>::from(format!("Failed to add backend URL: {err}")),
                                });
                            }
                        }
                    }
                    MessageToBackend::RemoveBackendUrl(url) => {
                        if let Err(err) = state.remove_backend_url(&url, &frontend).await {
                            log::error!("Failed to remove backend URL {url}: {err:#}");
                            frontend.send(MessageToFrontend::Notification {
                                level: NotificationLevel::Error,
                                message: Arc::<str>::from(format!("Failed to remove backend URL: {err}")),
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
                    MessageToBackend::Quit => break,
                    other => {
                        log::info!("Unhandled backend message: {other:?}");
                    }
                }
            }
            event = internal_receiver.recv() => {
                match event {
                    Some(BackendEvent::FetchFinished { url, state: catalog_state }) => {
                        state.handle_fetch_finished(url, catalog_state, &frontend);
                    }
                    Some(BackendEvent::InstallProgress { id, stage, current, total, message, show_bar }) => {
                        if state.install_tasks.contains_key(&id) {
                            state.installing.insert(id, instances::InstallProgressView {
                                stage,
                                current,
                                total,
                                message,
                                show_bar,
                            });
                        }
                        state.emit_snapshot(&frontend);
                    }
                    Some(BackendEvent::InstallFinished { id, result }) => {
                        state.handle_install_finished(id, result, &frontend).await;
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
                    None => break,
                }
            }
        }
    }

    frontend.send(MessageToFrontend::Quit);
    Ok(())
}
