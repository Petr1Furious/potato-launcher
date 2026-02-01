use super::version_metadata::{Library, LibraryDownloads, Rule};
use log::info;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

pub const LIBRARY_OVERRIDES_JSON: &str = include_str!("../meta/library-overrides.json");

pub const MOJANG_LIBRARY_PATCHES_JSON: &str = include_str!("../meta/mojang-library-patches.json");

pub const LWJGL_VERSION_MATCHES_JSON: &str = include_str!("../meta/lwjgl-version-matches.json");

#[derive(Deserialize)]
pub struct Replacement {
    pub libraries: Vec<Library>,
    pub version: String,
}

#[derive(Deserialize)]
pub struct LibraryOverrides {
    lwjgl_group_ids: HashSet<String>,
    overrides: Vec<Replacement>,
}

lazy_static::lazy_static! {
    static ref LIBRARY_OVERRIDES: LibraryOverrides = {
        let overrides = LIBRARY_OVERRIDES_JSON;
        serde_json::from_str(overrides).expect("Failed to parse library patches")
    };
}

#[derive(Deserialize)]
pub struct LibraryPatch {
    downloads: Option<LibraryDownloads>,
    natives: Option<HashMap<String, String>>,
    rules: Option<Vec<Rule>>,
}

#[derive(Deserialize)]
pub struct LibraryPatches {
    #[serde(rename = "match")]
    match_: HashSet<String>,

    #[serde(rename = "override")]
    override_: Option<LibraryPatch>,

    #[serde(rename = "additionalLibraries")]
    additional_libraries: Option<Vec<Library>>,
}

lazy_static::lazy_static! {
    static ref LIBRARY_PATCHES: Vec<LibraryPatches> = {
        let overrides = MOJANG_LIBRARY_PATCHES_JSON;
        serde_json::from_str(overrides).expect("Failed to parse library overrides")
    };
}

lazy_static::lazy_static! {
    static ref LWJGL_VERSION_MATCHES: HashMap<String, String> = {
        let matches = LWJGL_VERSION_MATCHES_JSON;
        serde_json::from_str(matches).expect("Failed to parse lwjgl version matches")
    };
}

fn with_mojang_patches(libraries: Vec<Library>) -> Vec<Library> {
    let mut result = vec![];
    for mut library in libraries {
        for patches in &*LIBRARY_PATCHES {
            if patches.match_.contains(&library.get_full_name()) {
                if let Some(patch) = &patches.override_ {
                    info!("Modifying library: {}", library.get_full_name());
                    if let Some(downloads) = &patch.downloads {
                        library.downloads = Some(downloads.clone());
                    }
                    if let Some(natives) = &patch.natives {
                        library.natives = Some(natives.clone());
                    }
                    if let Some(rules) = &patch.rules {
                        library.rules = Some(rules.clone());
                    }
                }
                if let Some(additional_libraries) = &patches.additional_libraries {
                    info!(
                        "Adding additional libraries for {}",
                        library.get_full_name()
                    );
                    result.extend(additional_libraries.clone());
                }
            }
        }
        result.push(library.clone());
    }

    info!("Processed {} libraries with mojang overrides", result.len());

    result
}

/// Apply overrides for the libraries.
/// This is used to add compatibility for some systems (e.g. older minecraft versions on arm macos).
pub fn with_overrides(libraries: Vec<Library>, version_id: &str) -> Vec<Library> {
    let main_version = LWJGL_VERSION_MATCHES.get(version_id);
    if let Some(main_version) = main_version {
        info!("Found main lwjgl version: {main_version}");
    } else {
        info!("No main lwjgl version found");
    }

    let libraries = with_mojang_patches(libraries);

    let mut result = vec![];
    if let Some(main_version) = main_version {
        for library in libraries {
            if !LIBRARY_OVERRIDES
                .lwjgl_group_ids
                .contains(&library.get_group_id())
            {
                result.push(library.clone());
            }
        }

        for override_ in &LIBRARY_OVERRIDES.overrides {
            if &override_.version == main_version {
                info!("Adding override libraries for version {main_version}");
                result.extend(override_.libraries.clone());
            }
        }
    } else {
        result = libraries;
    }

    info!("Processed {} libraries with overrides", result.len());

    result
}
