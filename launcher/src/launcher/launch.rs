use launcher_auth::AccountData;
use launcher_auth::providers::AuthProviderConfig;
use log::{debug, warn};
use maplit::hashmap;
use shared::paths::{
    get_authlib_injector_path, get_client_jar_path, get_libraries_dir, get_logs_dir,
    get_minecraft_dir, get_natives_dir,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::process::{Child, Command as TokioCommand};

use super::compat;
use crate::config::runtime_config::Config;
use crate::constants;
use crate::version::complete_version_metadata::CompleteVersionMetadata;
use crate::version::os;
use shared::version::version_metadata;

const GC_OPTIONS: &[&str] = &[
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

#[cfg(target_os = "windows")]
const PATHSEP: &str = ";";
#[cfg(not(target_os = "windows"))]
const PATHSEP: &str = ":";

fn replace_launch_config_variables(
    argument: String,
    variables: &HashMap<String, String>,
) -> String {
    variables
        .iter()
        .fold(argument, |acc, (k, v)| acc.replace(&format!("${{{k}}}"), v))
}

fn process_args(
    args: &Vec<version_metadata::VariableArgument>,
    variables: &HashMap<String, String>,
) -> Vec<String> {
    let mut options = vec![];
    for arg in args {
        options.extend(
            arg.get_matching_values(&os::get_os_name(), &os::get_system_arch())
                .iter()
                .map(|v| replace_launch_config_variables(v.to_string(), variables)),
        );
    }
    options
}

#[derive(thiserror::Error, Debug)]
pub enum LaunchError {
    #[error("Missing authlib injector")]
    MissingAuthlibInjector,
    #[error("Missing library {0}")]
    MissingLibrary(PathBuf),
    #[error("Java path for version {0} not found")]
    JavaPathNotFound(String),
}

pub async fn launch(
    version_metadata: &CompleteVersionMetadata,
    config: &Config,
    account_data: &AccountData,
    online: bool,
) -> anyhow::Result<Child> {
    let launcher_dir = config.get_launcher_dir();
    let mut minecraft_dir = get_minecraft_dir(&launcher_dir, version_metadata.get_name());
    let libraries_dir = get_libraries_dir(&launcher_dir);
    let natives_dir = get_natives_dir(&launcher_dir, version_metadata.get_parent_id());

    let minecraft_dir_short = minecraft_dir.clone();
    if cfg!(windows) {
        minecraft_dir = PathBuf::from(compat::win_get_long_path_name(
            &minecraft_dir_short.to_string_lossy(),
        )?);
    }

    let mut used_library_paths = HashSet::new();
    let mut classpath = vec![];
    for library in version_metadata.get_libraries_with_overrides() {
        if let Some(path) = library.get_library_path(&libraries_dir) {
            if !path.is_file() {
                return Err(LaunchError::MissingLibrary(path.clone()).into());
            }

            let path_string = path.to_string_lossy().to_string();
            if !used_library_paths.contains(&path_string) {
                // vanilla mojang manifests have duplicates for some reason
                used_library_paths.insert(path_string.clone());
                classpath.push(path_string);
            }
        }
    }

    let client_jar_path = get_client_jar_path(&launcher_dir, version_metadata.get_id());
    if !client_jar_path.exists() {
        return Err(LaunchError::MissingLibrary(client_jar_path).into());
    }

    classpath.push(client_jar_path.to_string_lossy().to_string());

    let mut classpath_str = classpath.join(PATHSEP);
    if cfg!(windows) {
        classpath_str = classpath_str.replace("/", "\\");
    }

    let variables: HashMap<String, String> = hashmap! {
        "natives_directory".to_string() => natives_dir.to_str().unwrap().to_string(),
        "launcher_name".to_string() => "java-minecraft-launcher".to_string(),
        "launcher_version".to_string() => "1.6.84-j".to_string(),
        "classpath".to_string() => classpath_str,
        "classpath_separator".to_string() => PATHSEP.to_string(),
        "library_directory".to_string() => libraries_dir.to_str().unwrap().to_string(),
        "auth_player_name".to_string() => account_data.user_info.username.clone(),
        "version_name".to_string() => version_metadata.get_id().to_string(),
        "game_directory".to_string() => minecraft_dir.to_str().unwrap().to_string(),
        "assets_root".to_string() => config.get_assets_dir().to_str().unwrap().to_string(),
        "assets_index_name".to_string() => version_metadata.get_asset_index()?.id.to_string(),
        "auth_uuid".to_string() => account_data.user_info.uuid.replace("-", ""),
        "auth_access_token".to_string() => account_data.access_token.clone(),
        "clientid".to_string() => "".to_string(),
        "auth_xuid".to_string() => "".to_string(),
        "user_type".to_string() => if online { "mojang" } else { "offline" }.to_string(),
        "version_type".to_string() => "release".to_string(),
        "resolution_width".to_string() => "925".to_string(),
        "resolution_height".to_string() => "530".to_string(),
        "user_properties".to_string() => "{}".to_string(),
    };

    let xmx = config.xmx.get(version_metadata.get_name()).map_or_else(
        || {
            warn!(
                "No Xmx value found for version {}",
                version_metadata.get_name()
            );
            format!("{}M", constants::XMX_DEFAULT)
        },
        |s| s.clone(),
    );

    let mut java_options = [
        GC_OPTIONS
            .iter()
            .map(|&s| s.to_string())
            .collect::<Vec<_>>(),
        vec![
            "-Xms512M".to_string(),
            format!("-Xmx{}", xmx),
            "-Duser.language=en".to_string(),
            "-Dfile.encoding=UTF-8".to_string(),
        ],
    ]
    .concat();

    let auth_provider = &config
        .get_selected_auth_profile()
        .map(|p| AuthProviderConfig::from_id(&p.auth_backend_id));
    if online && let Some(injector_url) = auth_provider.as_ref().and_then(|x| x.get_injector_url())
    {
        let authlib_injector_path = get_authlib_injector_path(&launcher_dir);
        if !authlib_injector_path.exists() {
            return Err(LaunchError::MissingAuthlibInjector.into());
        }
        java_options.insert(
            0,
            format!(
                "-javaagent:{}={}",
                authlib_injector_path.to_str().unwrap(),
                injector_url,
            ),
        );
    }

    #[cfg(target_os = "linux")]
    if *config
        .use_native_glfw
        .get(version_metadata.get_name())
        .unwrap_or(&crate::config::build_config::USE_NATIVE_GLFW_DEFAULT)
    {
        use crate::launcher::compat::linux_find_native_glfw;
        let glfw_path = linux_find_native_glfw()?;
        log::info!("Using GLFW at {glfw_path}");
        java_options.push("-Dorg.lwjgl.glfw.libname=".to_string() + &glfw_path);
    }

    let arguments = version_metadata.get_arguments()?;

    java_options.extend(process_args(&arguments.jvm, &variables));
    let minecraft_options = process_args(&arguments.game, &variables);

    let java_path = config
        .java_paths
        .get(version_metadata.get_name())
        .ok_or_else(|| LaunchError::JavaPathNotFound(version_metadata.get_name().to_string()))?;

    debug!("Launching java {java_path} with arguments {java_options:?}");
    debug!("Main class: {}", version_metadata.get_main_class());
    debug!("Game arguments: {minecraft_options:?}");

    let mut cmd = TokioCommand::new(java_path);
    cmd.args(&java_options)
        .arg(version_metadata.get_main_class())
        .args(&minecraft_options)
        .current_dir(minecraft_dir_short);

    let file =
        std::fs::File::create(get_logs_dir(&launcher_dir).join("latest_minecraft_launch.log"))?;
    cmd.stdout(file.try_clone()?);
    cmd.stderr(file);

    #[cfg(target_os = "windows")]
    {
        use winapi::um::winbase::CREATE_NO_WINDOW;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    Ok(cmd.spawn()?)
}
