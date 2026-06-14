use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LocalizedString {
    Plain(String),
    Localized(BTreeMap<String, String>),
}

impl LocalizedString {
    pub fn resolve(&self, language: &str) -> Option<&str> {
        match self {
            LocalizedString::Plain(value) => non_empty(value),
            LocalizedString::Localized(values) => {
                let primary = language
                    .split(['-', '_'])
                    .next()
                    .filter(|part| !part.is_empty());

                values
                    .get(language)
                    .and_then(|value| non_empty(value))
                    .or_else(|| {
                        primary.and_then(|language| {
                            values.get(language).and_then(|value| non_empty(value))
                        })
                    })
                    .or_else(|| values.get("en").and_then(|value| non_empty(value)))
                    .or_else(|| values.values().find_map(|value| non_empty(value)))
            }
        }
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_localized_string_roundtrips() {
        let value: LocalizedString = serde_json::from_str(r#""Potato""#).unwrap();
        assert_eq!(value.resolve("ru"), Some("Potato"));
        assert_eq!(serde_json::to_string(&value).unwrap(), r#""Potato""#);
    }

    #[test]
    fn map_localized_string_resolves_language() {
        let value: LocalizedString =
            serde_json::from_str(r#"{"en":"Potato","ru":"Картошка"}"#).unwrap();

        assert_eq!(value.resolve("ru"), Some("Картошка"));
        assert_eq!(value.resolve("ru-RU"), Some("Картошка"));
        assert_eq!(value.resolve("de"), Some("Potato"));
    }
}
