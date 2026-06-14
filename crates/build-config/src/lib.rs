use url::Url;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub fn launcher_name() -> &'static str {
    LAUNCHER_NAME
}

pub fn lower_launcher_name() -> String {
    normalize_launcher_name(LAUNCHER_NAME)
}

pub fn data_dir_name() -> &'static str {
    "potato-launcher"
}

fn normalize_launcher_name(value: &str) -> String {
    let normalized = value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if normalized.is_empty() {
        "launcher".to_string()
    } else {
        normalized
    }
}

pub fn launcher_app_id() -> &'static str {
    LAUNCHER_APP_ID
}

pub fn launcher_icon() -> Option<&'static str> {
    LAUNCHER_ICON
}

pub fn default_instance_manifest_urls() -> Vec<Url> {
    INSTANCE_MANIFEST_URLS
        .iter()
        .filter_map(|url| Url::parse(url).ok())
        .collect()
}

pub fn backend_api_base() -> Option<Url> {
    BACKEND_API_BASE.and_then(|url| Url::parse(url).ok())
}

pub fn version() -> Option<&'static str> {
    VERSION
}

pub fn use_native_glfw_default() -> bool {
    USE_NATIVE_GLFW_DEFAULT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_launcher_name_is_data_dir_friendly() {
        assert_eq!(
            normalize_launcher_name("Potato Launcher Dev"),
            "potato-launcher-dev"
        );
        assert_eq!(
            normalize_launcher_name("  Potato__Launcher  "),
            "potato-launcher"
        );
        assert_eq!(normalize_launcher_name("___"), "launcher");
    }
}
