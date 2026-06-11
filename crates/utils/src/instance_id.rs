use std::collections::HashSet;

use lazy_static::lazy_static;
use regex::Regex;
use thiserror::Error;

pub const MAX_INSTANCE_ID_LEN: usize = 64;
pub const FALLBACK_LOCAL_DIR_NAME: &str = "instance";

lazy_static! {
    static ref INSTANCE_ID_RE: Regex =
        Regex::new(r"^[a-z][a-z0-9_-]*$").expect("valid instance id regex");
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum InstanceIdError {
    #[error("instance id is empty")]
    Empty,
    #[error("instance id is too long (max {MAX_INSTANCE_ID_LEN} characters)")]
    TooLong,
    #[error(
        "instance id must start with a lowercase letter and contain only lowercase letters, digits, hyphens, and underscores"
    )]
    InvalidFormat,
}

pub fn validate_instance_id(id: &str) -> Result<(), InstanceIdError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(InstanceIdError::Empty);
    }
    if id.len() > MAX_INSTANCE_ID_LEN {
        return Err(InstanceIdError::TooLong);
    }
    if !INSTANCE_ID_RE.is_match(id) {
        return Err(InstanceIdError::InvalidFormat);
    }
    Ok(())
}

pub fn slugify_local_dir_name(name: &str) -> String {
    let transliterated = unidecode::unidecode(name.trim());
    let mut slug = String::new();
    let mut last_was_sep = false;

    for ch in transliterated.chars() {
        match ch {
            'a'..='z' | '0'..='9' => {
                last_was_sep = false;
                slug.push(ch);
            }
            'A'..='Z' => {
                last_was_sep = false;
                slug.push(ch.to_ascii_lowercase());
            }
            ' ' | '_' | '-' | '.' if !slug.is_empty() && !last_was_sep => {
                slug.push('-');
                last_was_sep = true;
            }
            _ => {}
        }
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        FALLBACK_LOCAL_DIR_NAME.to_string()
    } else {
        slug
    }
}

pub fn allocate_unique_name(taken: &HashSet<&str>, base: &str) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }

    for num in 2.. {
        let candidate = format!("{base}-{num}");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("usize counter should not overflow while allocating a unique name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_instance_id_accepts_slug_like_values() {
        validate_instance_id("minigames").unwrap();
        validate_instance_id("sky-block_2").unwrap();
    }

    #[test]
    fn validate_instance_id_rejects_invalid_values() {
        assert_eq!(validate_instance_id(""), Err(InstanceIdError::Empty));
        assert_eq!(
            validate_instance_id("Minigames"),
            Err(InstanceIdError::InvalidFormat)
        );
        assert_eq!(
            validate_instance_id("1bad"),
            Err(InstanceIdError::InvalidFormat)
        );
        assert_eq!(
            validate_instance_id(&"a".repeat(MAX_INSTANCE_ID_LEN + 1)),
            Err(InstanceIdError::TooLong)
        );
    }

    #[test]
    fn slugify_local_dir_name_transliterates_and_normalizes() {
        assert_eq!(slugify_local_dir_name("My Cool Pack"), "my-cool-pack");
        assert_eq!(slugify_local_dir_name("Мой Пак"), "moi-pak");
        assert_eq!(slugify_local_dir_name("..."), FALLBACK_LOCAL_DIR_NAME);
    }

    #[test]
    fn allocate_unique_name_uses_numeric_suffixes() {
        let taken = HashSet::from(["minigames", "minigames-2"]);
        assert_eq!(allocate_unique_name(&taken, "minigames"), "minigames-3");
        assert_eq!(
            allocate_unique_name(&HashSet::new(), "minigames"),
            "minigames"
        );
    }
}
