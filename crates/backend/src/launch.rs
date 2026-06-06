use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use instance::{instance_metadata::InstanceMetadata, os, storage::LocalInstance};
use launcher_auth::{
    AccountData,
    flow::{AuthMessage, AuthMessageProvider, perform_auth},
    providers::{AuthProviderConfig, OfflineAuthProvider},
    storage::{AccountKey, StorageAccountEntry},
};
use launcher_bridge::{FrontendSender, MessageToFrontend};
use launcher_build_config::{launcher_name, use_native_glfw_default, version};
use tokio::process::{Child, Command};
use utils::{
    java,
    paths::{AssetsDir, DataDir, InstanceDirFS, InstancesDir, LibrariesDir, LogsDir, NativesDir},
};
use uuid::Uuid;

const DEFAULT_OFFLINE_USERNAME: &str = "Player";
const DEFAULT_XMX: &str = "4096M";

#[cfg(target_os = "windows")]
const PATH_SEPARATOR: &str = ";";
#[cfg(not(target_os = "windows"))]
const PATH_SEPARATOR: &str = ":";

const LEGACY_GC_OPTIONS: &[&str] = &[
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:+UseG1GC",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=50",
    "-XX:G1HeapRegionSize=32M",
    "-XX:+DisableExplicitGC",
    "-XX:+AlwaysPreTouch",
    "-XX:+ParallelRefProcEnabled",
];
const MODERN_GC_OPTIONS: &[&str] = &["-XX:+UseZGC", "-XX:+UseStringDeduplication"];
const JAVA_21_GC_OPTIONS: &[&str] = &[
    "-XX:+UseZGC",
    "-XX:+ZGenerational",
    "-XX:+UseStringDeduplication",
];

#[derive(Clone)]
pub(crate) struct LaunchRequest {
    pub(crate) id: Uuid,
    pub(crate) account: Option<AccountKey>,
    pub(crate) bypass_required_provider: bool,
    pub(crate) xmx_mb: Option<u64>,
    pub(crate) jvm_flags: Option<String>,
    pub(crate) java_path: Option<String>,
    pub(crate) resolved_java: Option<java::JavaInstallation>,
    pub(crate) use_native_glfw: Option<bool>,
    pub(crate) launcher_dir: PathBuf,
    pub(crate) local_instances: Vec<LocalInstance>,
    pub(crate) account_entries: Vec<(AccountKey, AuthProviderConfig, AccountData)>,
    pub(crate) frontend: FrontendSender,
}

pub(crate) struct LaunchStart {
    pub(crate) child: Child,
    pub(crate) refreshed_account: Option<(AuthProviderConfig, AccountData)>,
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum LaunchError {
    #[error("installed instance {0} was not found")]
    InstanceNotFound(Uuid),
    #[error("account {0:?} was not found")]
    AccountNotFound(AccountKey),
    #[error("account {account:?} is not compatible with required auth provider {required:?}")]
    IncompatibleAccount {
        account: AccountKey,
        required: AuthProviderConfig,
    },
    #[error("no account is available for auth provider {0:?}")]
    NoCompatibleAccount(AuthProviderConfig),
    #[error("no account is available for launch")]
    NoAccount,
    #[error("Java {0} was not found; install or repair the instance first")]
    JavaNotFound(String),
    #[error(
        "invalid custom Java path: the configured executable is missing or incompatible with Java {0}"
    )]
    InvalidCustomJavaPath(String),
    #[cfg(target_os = "linux")]
    #[error("failed to find native GLFW library: {0}")]
    NativeGlfwNotFound(#[from] utils::compat::NativeGlfwError),
    #[error("classpath entry does not exist: {0}")]
    MissingClasspathEntry(String),
    #[error("authlib-injector is missing at {0}")]
    MissingAuthlibInjector(String),
    #[error("failed while processing instance metadata: {0}")]
    InstanceMetadata(#[from] instance::instance_metadata::InstanceMetadataError),
    #[error("authentication failed: {0}")]
    Auth(#[from] launcher_auth::flow::PerformAuthError),
    #[error("file I/O failed while launching: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to prepare Java for launch: {0}")]
    JavaPrepare(#[from] anyhow::Error),
}

pub(crate) fn default_offline_account() -> (AccountKey, AuthProviderConfig, AccountData) {
    offline_account(DEFAULT_OFFLINE_USERNAME)
}

pub(crate) fn offline_account(username: &str) -> (AccountKey, AuthProviderConfig, AccountData) {
    let provider = AuthProviderConfig::Offline(OfflineAuthProvider {});
    let data = offline_account_data(username);
    let key = (offline_provider_id(), data.user_info.username.clone());
    (key, provider, data)
}

pub(crate) fn stored_accounts(
    entries: impl Iterator<Item = (StorageAccountEntry, AuthProviderConfig)>,
) -> Vec<(AccountKey, AuthProviderConfig, AccountData)> {
    entries
        .map(|(entry, provider)| {
            let key = (
                entry.provider_id,
                entry.auth_data.user_info.username.clone(),
            );
            (key, provider, entry.auth_data)
        })
        .collect()
}

pub(crate) async fn launch_instance(request: LaunchRequest) -> Result<LaunchStart, LaunchError> {
    let data_dir = DataDir::new(request.launcher_dir.clone());
    let local = request
        .local_instances
        .iter()
        .find(|instance| instance.id == request.id)
        .ok_or(LaunchError::InstanceNotFound(request.id))?;
    let instance_dir = InstancesDir::root()
        .instance_dir(&local.dir_name)
        .with_data_dir(data_dir.clone());
    let metadata = read_metadata(&instance_dir).await?;
    let required_provider = (!request.bypass_required_provider)
        .then(|| metadata.get_auth_provider().cloned())
        .flatten();
    let (provider, account_data) = resolve_account(
        request.account,
        required_provider.as_ref(),
        &request.account_entries,
    )?;
    let online = !matches!(provider, AuthProviderConfig::Offline(_));
    let original_account_data = account_data.clone();
    let account_data = if online {
        perform_auth(
            Some(account_data),
            provider.clone(),
            Arc::new(LaunchAuthMessages::new(request.frontend.clone())),
        )
        .await?
    } else {
        account_data
    };
    let refreshed_account =
        (account_data != original_account_data).then(|| (provider.clone(), account_data.clone()));
    let java = if let Some(installation) = request.resolved_java {
        installation
    } else {
        let java_version = metadata.get_java_version();
        if let Some(path) = request.java_path.as_deref() {
            let java_path = std::path::Path::new(path);
            if !java::check_java(&java_version, java_path).await {
                return Err(LaunchError::InvalidCustomJavaPath(java_version));
            }
            java::get_installation_pub(java_path)
                .await
                .ok_or_else(|| LaunchError::InvalidCustomJavaPath(java_version))?
        } else {
            java::get_java(&java_version, &data_dir)
                .await
                .ok_or_else(|| LaunchError::JavaNotFound(java_version.clone()))?
        }
    };
    let minecraft_dir_short = instance_dir.minecraft_dir();
    let minecraft_dir_game = game_directory_for_launch(&minecraft_dir_short)?;
    let args = build_launch_arguments(&LaunchBuildContext {
        metadata: &metadata,
        provider: &provider,
        account: &account_data,
        online,
        xmx_mb: request.xmx_mb,
        jvm_flags: request.jvm_flags.as_deref(),
        use_native_glfw: request
            .use_native_glfw
            .unwrap_or_else(use_native_glfw_default),
        data_dir: &data_dir,
        game_directory: &minecraft_dir_game,
    })?;

    tokio::fs::create_dir_all(&minecraft_dir_short).await?;

    let logs_dir = LogsDir::root().to_fs(&data_dir);
    tokio::fs::create_dir_all(&logs_dir).await?;
    let log_file = std::fs::File::create(logs_dir.join("latest_minecraft_launch.log"))?;

    log::info!(
        "Launching {} with Java at {}",
        metadata.get_name(),
        java.path.display()
    );
    log::debug!("Java arguments: {:?}", args.java);
    log::debug!("Main class: {}", args.main_class);
    log::debug!("Game arguments: {:?}", args.game);

    let mut command = Command::new(java.path);
    command
        .args(args.java)
        .arg(args.main_class)
        .args(args.game)
        .current_dir(&minecraft_dir_short)
        .stdout(log_file.try_clone()?)
        .stderr(log_file);

    #[cfg(target_os = "windows")]
    {
        use winapi::um::winbase::CREATE_NO_WINDOW;

        command.creation_flags(CREATE_NO_WINDOW);
    }

    Ok(LaunchStart {
        child: command.spawn()?,
        refreshed_account,
    })
}

struct LaunchArguments {
    java: Vec<String>,
    main_class: String,
    game: Vec<String>,
}

struct LaunchBuildContext<'a> {
    metadata: &'a InstanceMetadata,
    provider: &'a AuthProviderConfig,
    account: &'a AccountData,
    online: bool,
    xmx_mb: Option<u64>,
    jvm_flags: Option<&'a str>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    use_native_glfw: bool,
    data_dir: &'a DataDir,
    game_directory: &'a Path,
}

fn build_launch_arguments(ctx: &LaunchBuildContext<'_>) -> Result<LaunchArguments, LaunchError> {
    let LaunchBuildContext {
        metadata,
        provider,
        account,
        online,
        xmx_mb,
        jvm_flags,
        data_dir,
        game_directory,
        ..
    } = ctx;
    let classpath = classpath(metadata, data_dir)?;
    let libraries_dir = LibrariesDir::root().to_fs(data_dir);
    let natives_dir = NativesDir::for_id(metadata.get_parent_id()?).to_fs(data_dir);
    let assets_dir = AssetsDir::root().to_fs(data_dir);
    let asset_index = metadata.get_asset_index()?;
    let variables = HashMap::from([
        ("natives_directory".to_string(), path_string(&natives_dir)),
        ("launcher_name".to_string(), launcher_name().to_string()),
        (
            "launcher_version".to_string(),
            version().unwrap_or("dev").to_string(),
        ),
        ("classpath".to_string(), classpath),
        (
            "classpath_separator".to_string(),
            PATH_SEPARATOR.to_string(),
        ),
        ("library_directory".to_string(), path_string(&libraries_dir)),
        (
            "auth_player_name".to_string(),
            account.user_info.username.clone(),
        ),
        ("version_name".to_string(), metadata.get_id()?.to_string()),
        ("game_directory".to_string(), path_string(game_directory)),
        ("assets_root".to_string(), path_string(&assets_dir)),
        ("assets_index_name".to_string(), asset_index.id.clone()),
        (
            "auth_uuid".to_string(),
            account.user_info.uuid.simple().to_string(),
        ),
        (
            "auth_access_token".to_string(),
            account.access_token.clone(),
        ),
        ("clientid".to_string(), String::new()),
        ("auth_xuid".to_string(), String::new()),
        (
            "user_type".to_string(),
            if *online { "mojang" } else { "offline" }.to_string(),
        ),
        ("version_type".to_string(), "release".to_string()),
        ("resolution_width".to_string(), "925".to_string()),
        ("resolution_height".to_string(), "530".to_string()),
        ("user_properties".to_string(), "{}".to_string()),
    ]);

    let mut java_args = gc_options(&metadata.get_java_version())
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    java_args.extend([
        "-Xms512M".to_string(),
        format!(
            "-Xmx{}",
            xmx_mb
                .map(|mb| format!("{mb}M"))
                .unwrap_or_else(|| normalize_xmx(metadata.get_default_xmx()))
        ),
        "-Duser.language=en".to_string(),
        "-Dfile.encoding=UTF-8".to_string(),
    ]);
    if let Some(flags) = jvm_flags {
        java_args.extend(flags.split_whitespace().map(ToString::to_string));
    }

    #[cfg(target_os = "linux")]
    if ctx.use_native_glfw {
        let glfw_path = utils::compat::linux_find_native_glfw()?;
        log::info!("Using native GLFW at {glfw_path}");
        java_args.push(format!("-Dorg.lwjgl.glfw.libname={glfw_path}"));
    }

    if *online && let Some(auth_url) = provider.get_injector_url() {
        let authlib_path = metadata
            .authlib_injector_path(data_dir, provider)?
            .ok_or_else(|| {
                LaunchError::MissingAuthlibInjector(
                    "authlib-injector library has no artifact path".to_string(),
                )
            })?;
        if !authlib_path.exists() {
            return Err(LaunchError::MissingAuthlibInjector(path_string(
                &authlib_path,
            )));
        }
        java_args.insert(
            0,
            format!("-javaagent:{}={auth_url}", path_string(&authlib_path)),
        );
    }

    let arguments = metadata.get_arguments()?;
    java_args.extend(process_args(&arguments.jvm, &variables));
    let game_args = process_args(&arguments.game, &variables);

    Ok(LaunchArguments {
        java: java_args,
        main_class: metadata.get_main_class()?.to_string(),
        game: game_args,
    })
}

fn classpath(metadata: &InstanceMetadata, data_dir: &DataDir) -> Result<String, LaunchError> {
    let mut used = HashSet::new();
    let mut paths = Vec::new();
    for path in metadata.get_classpath_paths(data_dir)? {
        if !path.exists() {
            return Err(LaunchError::MissingClasspathEntry(path_string(&path)));
        }
        let path = path_string(&path);
        if used.insert(path.clone()) {
            paths.push(path);
        }
    }
    let joined = paths.join(PATH_SEPARATOR);
    Ok({
        #[cfg(target_os = "windows")]
        {
            joined.replace('/', "\\")
        }
        #[cfg(not(target_os = "windows"))]
        {
            joined
        }
    })
}

#[cfg(target_os = "windows")]
fn game_directory_for_launch(path: &Path) -> Result<PathBuf, LaunchError> {
    Ok(PathBuf::from(utils::compat::win_get_long_path_name(
        &path.to_string_lossy(),
    )?))
}

#[cfg(not(target_os = "windows"))]
fn game_directory_for_launch(path: &Path) -> Result<PathBuf, LaunchError> {
    Ok(path.to_path_buf())
}

fn process_args(
    args: &[instance::version_metadata::VariableArgument],
    variables: &HashMap<String, String>,
) -> Vec<String> {
    args.iter()
        .flat_map(|arg| arg.get_matching_values(&os::get_os_name(), &os::get_system_arch()))
        .map(|arg| replace_launch_variables(arg, variables))
        .collect()
}

fn replace_launch_variables(argument: &str, variables: &HashMap<String, String>) -> String {
    variables
        .iter()
        .fold(argument.to_string(), |acc, (key, value)| {
            acc.replace(&format!("${{{key}}}"), value)
        })
}

fn gc_options(java_version: &str) -> &'static [&'static str] {
    let java_major_version = java_version.parse::<u64>().unwrap_or(8);
    if java_major_version >= 23 {
        MODERN_GC_OPTIONS
    } else if java_major_version >= 21 {
        JAVA_21_GC_OPTIONS
    } else {
        LEGACY_GC_OPTIONS
    }
}

fn normalize_xmx(value: Option<&str>) -> String {
    let value = value.unwrap_or(DEFAULT_XMX).trim();
    if value.chars().all(|character| character.is_ascii_digit()) {
        format!("{value}M")
    } else {
        value.to_string()
    }
}

pub(crate) async fn read_metadata(
    instance_dir: &InstanceDirFS,
) -> Result<InstanceMetadata, LaunchError> {
    Ok(InstanceMetadata::read_local(instance_dir).await?)
}

fn resolve_account(
    requested: Option<AccountKey>,
    required_provider: Option<&AuthProviderConfig>,
    accounts: &[(AccountKey, AuthProviderConfig, AccountData)],
) -> Result<(AuthProviderConfig, AccountData), LaunchError> {
    if let Some(key) = requested {
        let (provider, data) = accounts
            .iter()
            .find(|(account_key, _, _)| account_key == &key)
            .map(|(_, provider, data)| (provider.clone(), data.clone()))
            .ok_or_else(|| LaunchError::AccountNotFound(key.clone()))?;
        if let Some(required) = required_provider
            && &provider != required
        {
            return Err(LaunchError::IncompatibleAccount {
                account: key,
                required: required.clone(),
            });
        }
        return Ok((provider, data));
    }

    if let Some(required) = required_provider {
        return accounts
            .iter()
            .find(|(_, provider, _)| provider == required)
            .map(|(_, provider, data)| (provider.clone(), data.clone()))
            .ok_or_else(|| LaunchError::NoCompatibleAccount(required.clone()));
    }

    accounts
        .first()
        .map(|(_, provider, data)| (provider.clone(), data.clone()))
        .ok_or(LaunchError::NoAccount)
}

fn offline_provider_id() -> Uuid {
    Uuid::new_v3(&Uuid::NAMESPACE_URL, b"potato-launcher:offline-provider")
}

fn offline_account_data(username: &str) -> AccountData {
    AccountData {
        access_token: username.to_string(),
        refresh_token: None,
        user_info: launcher_auth::UserInfo {
            uuid: Uuid::new_v3(&Uuid::NAMESPACE_DNS, username.as_bytes()),
            username: username.to_string(),
        },
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

struct LaunchAuthMessages {
    frontend: FrontendSender,
    offline_nickname: Mutex<String>,
    message: Mutex<Option<AuthMessage>>,
}

impl LaunchAuthMessages {
    fn new(frontend: FrontendSender) -> Self {
        Self {
            frontend,
            offline_nickname: Mutex::new(DEFAULT_OFFLINE_USERNAME.to_string()),
            message: Mutex::new(None),
        }
    }
}

#[async_trait]
impl AuthMessageProvider for LaunchAuthMessages {
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
            .unwrap_or_else(|_| DEFAULT_OFFLINE_USERNAME.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_auth::providers::{MicrosoftAuthProvider, OfflineAuthProvider};

    #[test]
    fn requested_account_must_match_required_provider() {
        let required = AuthProviderConfig::Microsoft(MicrosoftAuthProvider {});
        let (key, provider, account) = offline_account("Tester");
        let accounts = vec![(key.clone(), provider, account)];

        let err = resolve_account(Some(key.clone()), Some(&required), &accounts).unwrap_err();

        assert!(matches!(
            err,
            LaunchError::IncompatibleAccount { account, required: actual_required }
                if account == key && actual_required == required
        ));
    }

    #[test]
    fn unresolved_required_provider_reports_no_compatible_account() {
        let required = AuthProviderConfig::Microsoft(MicrosoftAuthProvider {});
        let (key, provider, account) = offline_account("Tester");
        let accounts = vec![(key, provider, account)];

        let err = resolve_account(None, Some(&required), &accounts).unwrap_err();

        assert!(matches!(
            err,
            LaunchError::NoCompatibleAccount(actual_required) if actual_required == required
        ));
    }

    #[test]
    fn compatible_required_provider_is_selected() {
        let required = AuthProviderConfig::Offline(OfflineAuthProvider {});
        let (key, provider, account) = offline_account("Tester");
        let accounts = vec![(key, provider, account.clone())];

        let (actual_provider, actual_account) =
            resolve_account(None, Some(&required), &accounts).unwrap();

        assert_eq!(actual_provider, required);
        assert_eq!(actual_account, account);
    }
}
