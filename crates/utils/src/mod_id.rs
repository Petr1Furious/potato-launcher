use std::{collections::HashSet, fs::File, io::Read, path::Path};

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
pub fn extract_mod_id(path: &Path) -> Result<Option<String>, ExtractModIdError> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    extract_mod_id_from_archive(&mut archive)
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

    Ok(None)
}

fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, ExtractModIdError> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    let mut content = String::new();
    entry.read_to_string(&mut content)?;
    Ok(Some(strip_bom(&content).to_owned()))
}

fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

fn read_fabric_mod_id<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Option<String>, ExtractModIdError> {
    let Some(content) = read_zip_entry(archive, FABRIC_MOD_JSON)? else {
        return Ok(None);
    };
    let json: Value = serde_json::from_str(&content)?;
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

    let json: Value = serde_json::from_str(&normalized)?;
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
