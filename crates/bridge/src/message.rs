use std::sync::Arc;

use launcher_auth::{
    AccountData, flow::AuthMessage, providers::AuthProviderConfig, storage::AccountKey,
};
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum MessageToBackend {
    Refresh,
    SelectInstance(Option<Uuid>),
    InstallInstance {
        id: Uuid,
        force_overwrite: bool,
    },
    CancelInstall(Uuid),
    RetryCreateLocal(Uuid),
    DeleteInstance(Uuid),
    Launch {
        instance: Uuid,
        account: Option<AccountKey>,
    },
    KillInstance(Uuid),
    AddBackendUrl(Url),
    RemoveBackendUrl(Url),
    StartAddAccount(AuthProviderConfig),
    SubmitOfflineNickname(String),
    RemoveAccount(AccountKey),
    SetInstanceSelectedAccount {
        instance: Uuid,
        account: Option<AccountKey>,
    },
    SetInstanceAccountOverride {
        instance: Uuid,
        account: Option<AccountKey>,
    },
    SetLauncherSettings(LauncherSettingsView),
    SetInstanceMemory {
        instance: Uuid,
        xmx_mb: Option<u64>,
    },
    SetInstanceJvmFlags {
        instance: Uuid,
        flags: Option<String>,
    },
    CreateLocalInstance {
        display_name: String,
        minecraft_version: String,
        loader: LocalLoader,
        loader_version: Option<String>,
    },
    FetchLocalCreateVersions,
    FetchLoaderVersions {
        minecraft_version: String,
        loader: LocalLoader,
    },
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalLoader {
    Vanilla,
    Fabric,
    Forge,
    Neoforge,
}

#[derive(Clone, Debug)]
pub enum MessageToFrontend {
    InstancesUpdated(Arc<[InstanceView]>),
    InstanceProgress {
        id: Uuid,
        stage: ProgressStage,
        current: u64,
        total: u64,
        message: Arc<str>,
    },
    AccountsUpdated(Arc<[AccountView]>),
    BackendsUpdated {
        backends: Arc<[BackendStatus]>,
    },
    SettingsUpdated(LauncherSettingsView),
    AuthPrompt(AuthMessage),
    Notification {
        level: NotificationLevel,
        message: Arc<str>,
    },
    LaunchFinished {
        instance: Uuid,
        exit: ExitOutcome,
    },
    LocalCreateVersionsUpdated {
        versions: Arc<[(String, String)]>,
        latest_release: String,
        error: Option<Arc<str>>,
    },
    LoaderVersionsUpdated {
        minecraft_version: String,
        loader: LocalLoader,
        versions: Arc<[String]>,
        error: Option<Arc<str>>,
    },
    Quit,
}

#[derive(Clone, Debug)]
pub struct InstanceView {
    pub id: Uuid,
    pub display_name: Arc<str>,
    pub dir_name: Arc<str>,
    pub origin: InstanceOrigin,
    pub status: InstanceLiveStatus,
    pub locally_installed: bool,
    pub orphaned: bool,
    pub auth_provider: Option<AuthProviderConfig>,
    pub default_xmx_mb: Option<u64>,
    pub selected_account: Option<AccountKey>,
    pub account_override: Option<AccountKey>,
    pub has_required_account: bool,
    pub launch_blocked_reason: Option<Arc<str>>,
    pub effective_account_username: Option<Arc<str>>,
    pub effective_auth_provider: Option<AuthProviderConfig>,
    pub effective_xmx_mb: Option<u64>,
    pub jvm_flags: Option<Arc<str>>,
}

impl InstanceView {
    pub fn is_orphaned(&self) -> bool {
        self.orphaned
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceOrigin {
    Local,
    Backend { url: Url },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceLiveStatus {
    NotInstalled,
    Installed,
    Outdated,
    Installing {
        stage: ProgressStage,
        current: u64,
        total: u64,
        message: Arc<str>,
        show_bar: bool,
    },
    InstallFailed(Arc<str>),
    Launching,
    Running,
    LaunchFailed(Arc<str>),
    OrphanedFromBackend,
}

impl InstanceLiveStatus {
    pub fn is_orphaned(&self) -> bool {
        matches!(self, Self::OrphanedFromBackend)
    }

    pub fn is_installing(&self) -> bool {
        matches!(self, Self::Installing { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressStage {
    Metadata,
    Files,
    Java,
    Launch,
}

#[derive(Clone, Debug)]
pub struct AccountView {
    pub key: AccountKey,
    pub provider: AuthProviderConfig,
    pub data: AccountData,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherSettingsView {
    pub hide_window_after_launch: bool,
    pub hide_usernames_in_cards: bool,
    pub language: String,
}

impl Default for LauncherSettingsView {
    fn default() -> Self {
        Self {
            hide_window_after_launch: false,
            hide_usernames_in_cards: false,
            language: "en".to_string(),
        }
    }
}

impl LauncherSettingsView {
    pub fn new(
        hide_window_after_launch: bool,
        hide_usernames_in_cards: bool,
        language: impl Into<String>,
    ) -> Self {
        Self {
            hide_window_after_launch,
            hide_usernames_in_cards,
            language: language.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackendStatus {
    pub url: Url,
    pub fetch_state: BackendFetchState,
    pub configured: bool,
    pub referenced_by_instances: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendFetchState {
    NotFetched,
    Fetching,
    Fetched { instance_count: usize },
    Offline,
    Error(Arc<str>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitOutcome {
    Success,
    ExitCode(i32),
    Terminated,
    Error(Arc<str>),
}
