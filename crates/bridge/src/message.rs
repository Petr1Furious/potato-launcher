use std::sync::Arc;

use instance::storage::InstanceHandle;
use launcher_auth::{
    AccountData, flow::AuthMessage, providers::AuthProviderConfig, storage::AccountKey,
};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatusView {
    Checking,
    Downloading { current: u64, total: u64 },
    NotApplicable,
    UpToDate,
    Replacing,
    Error { message: Arc<str>, offline: bool },
    ReadOnly,
}

#[derive(Clone, Debug)]
pub enum MessageToBackend {
    Refresh,
    InstallInstance {
        handle: InstanceHandle,
        force_overwrite: bool,
        /// When `Some`, shows launch-after checkbox defaulting to that value.
        launch_after_install: Option<bool>,
    },
    CancelInstall(InstanceHandle),
    RetryCreateLocal(InstanceHandle),
    DeleteInstance(InstanceHandle),
    Launch {
        instance: InstanceHandle,
        account: Option<AccountKey>,
    },
    KillInstance(InstanceHandle),
    AddBackendUrl(Url),
    RemoveBackendUrl(Url),
    StartAddAccount(AuthProviderConfig),
    SubmitOfflineNickname(String),
    RemoveAccount(AccountKey),
    SetInstanceSelectedAccount {
        instance: InstanceHandle,
        account: Option<AccountKey>,
    },
    SetInstanceAccountOverride {
        instance: InstanceHandle,
        account: Option<AccountKey>,
    },
    SetLauncherSettings(LauncherSettingsView),
    SetInstanceMemory {
        instance: InstanceHandle,
        xmx_mb: Option<u64>,
    },
    SetInstanceJvmFlags {
        instance: InstanceHandle,
        flags: Option<String>,
    },
    SetInstanceJavaPath {
        instance: InstanceHandle,
        path: Option<String>,
    },
    SetInstanceUseNativeGlfw {
        instance: InstanceHandle,
        enabled: bool,
    },
    SetOptionalModSetEnabled {
        instance: InstanceHandle,
        set_id: String,
        enabled: bool,
    },
    SetLaunchAfterInstall {
        instance: InstanceHandle,
        enabled: bool,
    },
    ResolveJavaPath(InstanceHandle),
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
    ProceedAfterUpdateFailure,
    CancelAuth,
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
        handle: InstanceHandle,
        tasks: Arc<[InstanceTaskView]>,
    },
    AccountsUpdated(Arc<[AccountView]>),
    BackendsUpdated {
        backends: Arc<[BackendStatus]>,
    },
    SettingsUpdated(LauncherSettingsView),
    AuthPrompt {
        context: AuthPromptContext,
        message: AuthMessage,
    },
    AuthPromptCleared,
    Notification {
        level: NotificationLevel,
        message: Arc<str>,
    },
    LaunchFinished {
        instance: InstanceHandle,
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
    UpdateStatus(UpdateStatusView),
    JavaPathResolved {
        instance: InstanceHandle,
        path: Option<Arc<str>>,
    },
    Quit,
}

#[derive(Clone, Debug)]
pub struct InstanceView {
    pub handle: InstanceHandle,
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
    pub java_path: Option<Arc<str>>,
    pub required_java_version: Option<Arc<str>>,
    /// `None` uses the launcher build default.
    pub use_native_glfw: Option<bool>,
    pub optional_mod_sets: Arc<[OptionalModSetView]>,
    /// `Some` while install/update is in progress and launch-after is offered.
    pub launch_after_install: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct OptionalModSetView {
    pub set_id: Arc<str>,
    pub display_name: Arc<str>,
    pub enabled: bool,
    pub enabled_by_default: bool,
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
        tasks: Arc<[InstanceTaskView]>,
    },
    InstallFailed(Arc<str>),
    /// Auth/sync/Java preparation before the game process spawns
    LaunchPreparing {
        tasks: Arc<[InstanceTaskView]>,
    },
    Running,
    LaunchFailed(Arc<str>),
    OrphanedFromBackend,
}

impl InstanceLiveStatus {
    pub fn is_orphaned(&self) -> bool {
        matches!(self, Self::OrphanedFromBackend)
    }

    pub fn active_tasks(&self) -> Option<&Arc<[InstanceTaskView]>> {
        match self {
            Self::Installing { tasks } | Self::LaunchPreparing { tasks } => Some(tasks),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceTaskView {
    pub id: u64,
    pub kind: TaskKind,
    pub message: Arc<str>,
    pub current: u64,
    /// `0` means indeterminate
    pub total: u64,
    pub unit: ProgressUnit,
    pub finished: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Auth,
    Metadata,
    CheckFiles,
    Download,
    Java,
    Extract,
    Copy,
    Generate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressUnit {
    Items,
    Bytes,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthPromptContext {
    AddAccount,
    /// Interactive auth needed by an instance install or launch.
    Instance {
        handle: InstanceHandle,
    },
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
