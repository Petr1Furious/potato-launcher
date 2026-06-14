use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::PathBuf,
};

const LAUNCHER_NAME_DEFAULT: &str = "Potato Launcher";
const LAUNCHER_APP_ID_DEFAULT: &str = "com.petr1furious.potato_launcher";

const BUILD_ENV_PATH: &str = "../../build.env";

fn load_build_env() -> HashMap<String, String> {
    let build_env_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BUILD_ENV_PATH);
    println!("cargo:rerun-if-changed={}", build_env_path.display());

    let Ok(contents) = fs::read_to_string(&build_env_path) else {
        return HashMap::new();
    };

    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn get_env(key: &str, build_env: &HashMap<String, String>) -> Option<String> {
    env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| build_env.get(key).cloned())
        .filter(|value| !value.is_empty())
}

fn main() {
    let build_env = load_build_env();

    for name in [
        "LAUNCHER_NAME",
        "LAUNCHER_APP_ID",
        "LAUNCHER_ICON",
        "INSTANCE_MANIFEST_URLS",
        "BACKEND_API_BASE",
        "VERSION",
        "USE_NATIVE_GLFW_DEFAULT",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let launcher_name =
        get_env("LAUNCHER_NAME", &build_env).unwrap_or_else(|| LAUNCHER_NAME_DEFAULT.into());
    let launcher_app_id =
        get_env("LAUNCHER_APP_ID", &build_env).unwrap_or_else(|| LAUNCHER_APP_ID_DEFAULT.into());
    let launcher_icon = get_env("LAUNCHER_ICON", &build_env);
    let backend_api_base = get_env("BACKEND_API_BASE", &build_env);
    let version = get_env("VERSION", &build_env);
    let use_native_glfw_default = get_env("USE_NATIVE_GLFW_DEFAULT", &build_env)
        .unwrap_or_else(|| "false".into())
        .parse::<bool>()
        .expect("USE_NATIVE_GLFW_DEFAULT must be a boolean");

    let raw_urls = get_env("INSTANCE_MANIFEST_URLS", &build_env).unwrap_or_default();
    let instance_manifest_urls = parse_url_list(&raw_urls);
    let urls_literal = instance_manifest_urls
        .iter()
        .map(|u| format!("{u:?}"))
        .collect::<Vec<_>>()
        .join(", ");

    let generated = format!(
        r#"pub const LAUNCHER_NAME: &str = {launcher_name:?};
pub const LAUNCHER_APP_ID: &str = {launcher_app_id:?};
pub const LAUNCHER_ICON: Option<&str> = {launcher_icon:?};
pub const INSTANCE_MANIFEST_URLS: &[&str] = &[{urls_literal}];
pub const BACKEND_API_BASE: Option<&str> = {backend_api_base:?};
pub const VERSION: Option<&str> = {version:?};
pub const USE_NATIVE_GLFW_DEFAULT: bool = {use_native_glfw_default};
"#
    );

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("generated.rs");
    fs::write(out_path, generated).unwrap();
}

fn parse_url_list(raw: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    raw.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert(s.to_string()))
        .map(str::to_owned)
        .collect()
}
