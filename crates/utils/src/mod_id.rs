use std::{
    collections::HashSet,
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

use log::warn;
use serde_json::Value;
use zip::ZipArchive;

const FABRIC_MOD_JSON: &str = "fabric.mod.json";
const NEOFORGE_MODS_TOML: &str = "META-INF/neoforge.mods.toml";
const FORGE_MODS_TOML: &str = "META-INF/mods.toml";

const LEGACY_INFO_FILES: &[&str] = &[
    "mcmod.info",
    "META-INF/mcmod.info",
    "cccmod.info",
    "neimod.info",
];

const JARJAR_PREFIX: &str = "META-INF/jarjar/";

const PACKAGING_SUFFIXES: &[&str] = &[
    "-all",
    "-full",
    "-universal",
    "-client",
    "-server",
    "-slim",
    "-NeoForge",
    "-neoforge",
    "-NEOFORGE",
    "-Forge",
    "-forge",
    "-Fabric",
    "-fabric",
];

#[derive(thiserror::Error, Debug)]
pub enum ExtractModIdError {
    #[error("file I/O failed while reading mod jar: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to read mod jar as zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("failed to parse mod metadata JSON: {0}")]
    ParseJson(#[from] serde_json::Error),
    #[error("failed to parse mod metadata TOML: {0}")]
    ParseToml(#[from] toml::de::Error),
}

/// Returns the primary mod ID declared in a mod JAR's metadata files.
///
/// Falls back to deriving an id from the jar filename when metadata is missing.
pub fn extract_mod_id(path: &Path) -> Result<Option<String>, ExtractModIdError> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    if let Some(id) = extract_mod_id_from_archive(&mut archive)? {
        return Ok(Some(id));
    }

    if let Some(id) = mod_id_from_filename(path) {
        warn!(
            "Using filename fallback for mod id '{id}': {}",
            path.display()
        );
        return Ok(Some(id));
    }

    Ok(None)
}

fn extract_mod_id_from_archive<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Option<String>, ExtractModIdError> {
    let entry_names = archive
        .file_names()
        .map(str::to_owned)
        .collect::<HashSet<_>>();

    if entry_names.contains(FABRIC_MOD_JSON)
        && let Some(id) = read_fabric_mod_id(archive)?
    {
        return Ok(Some(id));
    }

    if entry_names.contains(NEOFORGE_MODS_TOML)
        && let Some(id) = read_mods_toml_mod_id(archive, NEOFORGE_MODS_TOML)?
    {
        return Ok(Some(id));
    }

    if entry_names.contains(FORGE_MODS_TOML)
        && let Some(id) = read_mods_toml_mod_id(archive, FORGE_MODS_TOML)?
    {
        return Ok(Some(id));
    }

    for info_file in LEGACY_INFO_FILES {
        if entry_names.contains(*info_file)
            && let Some(id) = read_mcmod_info_mod_id(archive, info_file)?
        {
            return Ok(Some(id));
        }
    }

    extract_mod_id_from_jarjar(archive, &entry_names)
}

fn extract_mod_id_from_jarjar<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    entry_names: &HashSet<String>,
) -> Result<Option<String>, ExtractModIdError> {
    let mut nested_jars = entry_names
        .iter()
        .filter(|name| name.starts_with(JARJAR_PREFIX) && name.ends_with(".jar"))
        .cloned()
        .collect::<Vec<_>>();
    nested_jars.sort();

    for nested_path in nested_jars {
        let Some(bytes) = read_zip_entry_bytes(archive, &nested_path)? else {
            continue;
        };
        let mut nested = ZipArchive::new(Cursor::new(bytes))?;
        if let Some(id) = extract_mod_id_from_archive(&mut nested)? {
            return Ok(Some(id));
        }
    }

    Ok(None)
}

fn mod_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem.is_empty() {
        return None;
    }

    let mut name = stem.to_string();
    strip_packaging_suffixes(&mut name);

    let parts = name.split('-').collect::<Vec<_>>();
    let mut mod_parts = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 && looks_like_version_part(part) {
            break;
        }
        if index == 0 && looks_like_version_part(part) {
            return None;
        }
        mod_parts.push(*part);
    }

    if mod_parts.is_empty() {
        mod_parts.push(parts[0]);
    }

    let id = normalize_filename_mod_id(&mod_parts.join("-"));
    is_valid_mod_id(&id).then_some(id)
}

fn strip_packaging_suffixes(name: &mut String) {
    loop {
        let before = name.clone();
        for suffix in PACKAGING_SUFFIXES {
            if name.ends_with(suffix) {
                name.truncate(name.len() - suffix.len());
            }
        }
        if *name == before {
            break;
        }
    }
}

fn looks_like_version_part(part: &str) -> bool {
    let part = part.trim();
    if part.is_empty() {
        return false;
    }

    let mut chars = part.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if first.is_ascii_digit() {
        return true;
    }

    if (first == 'v' || first == 'V') && chars.next().is_some_and(|c| c.is_ascii_digit()) {
        return true;
    }

    matches!(
        part.to_ascii_lowercase().as_str(),
        "forge" | "neoforge" | "fabric"
    )
}

fn normalize_filename_mod_id(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut last_was_sep = false;

    for ch in raw.chars() {
        match ch {
            'A'..='Z' => {
                last_was_sep = false;
                normalized.push(ch.to_ascii_lowercase());
            }
            'a'..='z' | '0'..='9' => {
                last_was_sep = false;
                normalized.push(ch);
            }
            ' ' | '.' | '_' | '-' if !normalized.is_empty() && !last_was_sep => {
                normalized.push('_');
                last_was_sep = true;
            }
            _ => {}
        }
    }

    normalized.trim_matches('_').to_string()
}

fn is_valid_mod_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|ch| matches!(ch, 'a'..='z' | '0'..='9' | '_' | '-'))
}

fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, ExtractModIdError> {
    let Some(bytes) = read_zip_entry_bytes(archive, name)? else {
        return Ok(None);
    };
    Ok(Some(strip_bom(&String::from_utf8_lossy(&bytes)).to_owned()))
}

fn read_zip_entry_bytes<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<Vec<u8>>, ExtractModIdError> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    let mut content = Vec::new();
    entry.read_to_end(&mut content)?;
    Ok(Some(content))
}

fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

fn sanitize_json_control_chars(content: &str) -> String {
    content
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn parse_json_lenient(content: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(content)
        .or_else(|_| serde_json::from_str(&sanitize_json_control_chars(content)))
}

fn read_fabric_mod_id<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Option<String>, ExtractModIdError> {
    let Some(content) = read_zip_entry(archive, FABRIC_MOD_JSON)? else {
        return Ok(None);
    };
    let json: Value = parse_json_lenient(&content)?;
    Ok(json
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned))
}

fn read_mods_toml_mod_id<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    entry_name: &str,
) -> Result<Option<String>, ExtractModIdError> {
    let Some(content) = read_zip_entry(archive, entry_name)? else {
        return Ok(None);
    };
    let root: Value = toml::from_str(&content)?;
    Ok(root
        .get("mods")
        .and_then(Value::as_array)
        .and_then(|mods| mods.first())
        .and_then(|entry| entry.get("modId"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned))
}

fn normalize_legacy_info_json(content: &str) -> String {
    content.replace("\n\n", "\\n").replace('\n', "")
}

fn read_mcmod_info_mod_id<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    entry_name: &str,
) -> Result<Option<String>, ExtractModIdError> {
    let Some(content) = read_zip_entry(archive, entry_name)? else {
        return Ok(None);
    };

    let normalized = if matches!(entry_name, "cccmod.info" | "neimod.info") {
        normalize_legacy_info_json(&content)
    } else {
        content
    };

    let json: Value = parse_json_lenient(&normalized)?;
    parse_mcmod_info_first_modid(&json)
}

fn parse_mcmod_info_first_modid(json: &Value) -> Result<Option<String>, ExtractModIdError> {
    let entries = if let Some(array) = json.as_array() {
        array
    } else if let Some(mod_list) = json.get("modList").and_then(Value::as_array) {
        mod_list
    } else if json.get("modid").is_some() {
        return Ok(json
            .get("modid")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned));
    } else {
        return Ok(None);
    };

    Ok(entries
        .first()
        .and_then(|entry| entry.get("modid"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_id_from_filename_strips_version_and_packaging_suffixes() {
        assert_eq!(
            mod_id_from_filename(Path::new("connector-2.0.0-beta.14+1.21.1-full.jar")),
            Some("connector".to_string())
        );
        assert_eq!(
            mod_id_from_filename(Path::new("kotlinforforge-5.11.0-all.jar")),
            Some("kotlinforforge".to_string())
        );
        assert_eq!(
            mod_id_from_filename(Path::new("BetterF3-11.0.3-NeoForge-1.21.1.jar")),
            Some("betterf3".to_string())
        );
        assert_eq!(
            mod_id_from_filename(Path::new("Applied-Mekanistics-1.6.3.jar")),
            Some("applied_mekanistics".to_string())
        );
        assert_eq!(mod_id_from_filename(Path::new("1.2.3.jar")), None);
    }
}
