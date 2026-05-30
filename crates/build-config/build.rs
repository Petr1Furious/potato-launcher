use std::{env, fs, path::PathBuf};

const LAUNCHER_NAME_DEFAULT: &str = "Potato Launcher";
const LAUNCHER_APP_ID_DEFAULT: &str = "com.petr1furious.potato_launcher";

fn main() {
    for name in [
        "LAUNCHER_NAME",
        "LAUNCHER_APP_ID",
        "LAUNCHER_ICON",
        "VERSION_MANIFEST_URL",
        "VERSION_MANIFEST_URLS",
        "BACKEND_API_BASE",
        "VERSION",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let launcher_name = env::var("LAUNCHER_NAME").unwrap_or_else(|_| LAUNCHER_NAME_DEFAULT.into());
    let launcher_app_id =
        env::var("LAUNCHER_APP_ID").unwrap_or_else(|_| LAUNCHER_APP_ID_DEFAULT.into());
    let launcher_icon = env::var("LAUNCHER_ICON").ok();
    let version_manifest_url = env::var("VERSION_MANIFEST_URL").ok();
    let version_manifest_urls = env::var("VERSION_MANIFEST_URLS").ok();
    let backend_api_base = env::var("BACKEND_API_BASE").ok();
    let version = env::var("VERSION").ok();

    let generated = format!(
        r#"pub const LAUNCHER_NAME: &str = {launcher_name:?};
pub const LAUNCHER_APP_ID: &str = {launcher_app_id:?};
pub const LAUNCHER_ICON: Option<&str> = {launcher_icon:?};
pub const VERSION_MANIFEST_URL: Option<&str> = {version_manifest_url:?};
pub const VERSION_MANIFEST_URLS: Option<&str> = {version_manifest_urls:?};
pub const BACKEND_API_BASE: Option<&str> = {backend_api_base:?};
pub const VERSION: Option<&str> = {version:?};
"#
    );

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("generated.rs");
    fs::write(out_path, generated).unwrap();
}
