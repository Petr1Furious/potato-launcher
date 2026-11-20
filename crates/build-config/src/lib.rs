use url::Url;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub fn launcher_name() -> &'static str {
    LAUNCHER_NAME
}

pub fn lower_launcher_name() -> String {
    normalize_launcher_name(LAUNCHER_NAME)
}

pub fn data_dir_name() -> String {
    LAUNCHER_DATA_DIR.map_or_else(lower_launcher_name, str::to_string)
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

pub enum UpdateSource {
    Static(String),
    GitHub(&'static str),
}

pub fn update_source() -> Option<UpdateSource> {
    if let Some(url) = LAUNCHER_UPDATE_URL {
        return Some(UpdateSource::Static(url.trim_end_matches('/').to_string()));
    }
    LAUNCHER_UPDATE_GITHUB.map(UpdateSource::GitHub)
}

/// Same as `LOWER_LAUNCHER_NAME` in scripts/ci/set-build-env.sh.
pub fn release_lower_name() -> String {
    release_lower(LAUNCHER_NAME)
}

fn release_lower(value: &str) -> String {
    value.to_lowercase().replace('\'', "").replace(' ', "_")
}

pub const fn version() -> Option<&'static str> {
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

    #[test]
    fn release_lower_matches_ci() {
        assert_eq!(release_lower("Potato Launcher"), "potato_launcher");
        assert_eq!(release_lower("Petr's Launcher"), "petrs_launcher");
    }
}
