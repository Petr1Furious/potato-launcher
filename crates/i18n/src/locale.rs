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
    locale.split(['-', '_']).next().filter(|part| !part.is_empty())
}
