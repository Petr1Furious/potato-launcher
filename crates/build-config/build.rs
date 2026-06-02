use std::{collections::HashSet, env, fs, path::PathBuf};

const LAUNCHER_NAME_DEFAULT: &str = "Potato Launcher";
const LAUNCHER_APP_ID_DEFAULT: &str = "com.petr1furious.potato_launcher";

fn main() {
    for name in [
        "LAUNCHER_NAME",
        "LAUNCHER_APP_ID",
        "LAUNCHER_ICON",
        "VERSION_MANIFEST_URL",
        "INSTANCE_MANIFEST_URLS",
        "BACKEND_API_BASE",
        "VERSION",
        "USE_NATIVE_GLFW_DEFAULT",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let launcher_name = env::var("LAUNCHER_NAME").unwrap_or_else(|_| LAUNCHER_NAME_DEFAULT.into());
    let launcher_app_id =
        env::var("LAUNCHER_APP_ID").unwrap_or_else(|_| LAUNCHER_APP_ID_DEFAULT.into());
    let launcher_icon = env::var("LAUNCHER_ICON").ok();
    let backend_api_base = env::var("BACKEND_API_BASE").ok();
    let version = env::var("VERSION").ok();
    let use_native_glfw_default = env::var("USE_NATIVE_GLFW_DEFAULT")
        .unwrap_or_else(|_| "false".into())
        .parse::<bool>()
        .expect("USE_NATIVE_GLFW_DEFAULT must be a boolean");

    // INSTANCE_MANIFEST_URLS takes priority; VERSION_MANIFEST_URL is only used
    // as a fallback when INSTANCE_MANIFEST_URLS is unset.
    let url_list = env::var("INSTANCE_MANIFEST_URLS").unwrap_or_default();
    let raw_urls = if url_list.is_empty() {
        env::var("VERSION_MANIFEST_URL").unwrap_or_default()
    } else {
        url_list
    };

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
