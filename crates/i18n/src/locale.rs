const SUPPORTED: &[&str] = &["en", "ru"];

pub fn resolve_language_code(preferred: Option<&str>, system_locale: Option<&str>) -> &'static str {
    if let Some(preferred) = preferred.filter(|code| is_supported(code)) {
        return normalize_code(preferred);
    }

    if let Some(system_locale) = system_locale
        && let Some(code) = primary_subtag(system_locale).filter(|code| is_supported(code))
    {
        return normalize_code(code);
    }

    "en"
}

pub fn detect_system_language_code() -> &'static str {
    resolve_language_code(None, sys_locale::get_locale().as_deref())
}

fn is_supported(code: &str) -> bool {
    SUPPORTED.contains(&normalize_code(code))
}

fn normalize_code(code: &str) -> &'static str {
    match primary_subtag(code).unwrap_or(code) {
        "ru" => "ru",
        _ => "en",
    }
}

fn primary_subtag(locale: &str) -> Option<&str> {
    locale
        .split(['-', '_'])
        .next()
        .filter(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_russian_is_selected() {
        assert_eq!(resolve_language_code(Some("ru"), None), "ru");
        assert_eq!(resolve_language_code(Some("ru-RU"), None), "ru");
    }

    #[test]
    fn system_russian_maps_to_ru() {
        assert_eq!(resolve_language_code(None, Some("ru-RU")), "ru");
    }

    #[test]
    fn unsupported_system_defaults_to_english() {
        assert_eq!(resolve_language_code(None, Some("de-DE")), "en");
    }

    #[test]
    fn unknown_explicit_defaults_to_english() {
        assert_eq!(resolve_language_code(Some("de"), None), "en");
    }

    #[test]
    fn preferred_overrides_system() {
        assert_eq!(resolve_language_code(Some("en"), Some("ru-RU")), "en");
    }
}
