use std::collections::HashSet;

use url::Url;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub fn launcher_name() -> &'static str {
    LAUNCHER_NAME
}

pub fn lower_launcher_name() -> String {
    normalize_launcher_name(LAUNCHER_NAME)
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
    parse_default_instance_manifest_urls(VERSION_MANIFEST_URL, VERSION_MANIFEST_URLS)
}

pub fn backend_api_base() -> Option<Url> {
    BACKEND_API_BASE.and_then(|url| Url::parse(url).ok())
}

pub fn version() -> Option<&'static str> {
    VERSION
}

pub fn parse_default_instance_manifest_urls(
    single_url: Option<&str>,
    url_list: Option<&str>,
) -> Vec<Url> {
    let mut urls = Vec::new();
    let mut seen = HashSet::new();

    for raw_url in single_url
        .into_iter()
        .chain(url_list.into_iter().flat_map(split_url_list))
    {
        let raw_url = raw_url.trim();
        if raw_url.is_empty() {
            continue;
        }
        let Ok(url) = Url::parse(raw_url) else {
            continue;
        };
        if seen.insert(url.as_str().to_string()) {
            urls.push(url);
        }
    }

    urls
}

fn split_url_list(value: &str) -> impl Iterator<Item = &str> {
    value.split([',', ';', '\n'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url_lists_with_supported_separators() {
        let urls = parse_default_instance_manifest_urls(
            None,
            Some(
                " https://one.example/manifest.json,https://two.example/a; \nhttps://three.example/b ",
            ),
        );

        assert_eq!(
            urls.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://one.example/manifest.json",
                "https://two.example/a",
                "https://three.example/b"
            ]
        );
    }

    #[test]
    fn merges_single_and_list_urls_in_stable_deduped_order() {
        let urls = parse_default_instance_manifest_urls(
            Some("https://one.example/manifest.json"),
            Some("https://two.example/a,https://one.example/manifest.json,not a url"),
        );

        assert_eq!(
            urls.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec!["https://one.example/manifest.json", "https://two.example/a"]
        );
    }

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
