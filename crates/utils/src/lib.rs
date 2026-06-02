pub mod adaptive_download;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod compat;
pub mod files;
pub mod java;
pub mod logging;
pub mod mod_id;
pub mod paths;
pub mod progress;

use std::collections::HashSet;

use serde::Serialize;
use sha1::Digest as _;
use sha1::Sha1;

pub fn get_unique_name(existing_names: &HashSet<String>, name_base: &str) -> String {
    if !existing_names.contains(name_base) {
        return name_base.to_string();
    }
    let mut num = 1;
    loop {
        let candidate = format!("{name_base} ({num})");
        if !existing_names.contains(&candidate) {
            return candidate;
        }
        num += 1;
    }
}

#[derive(thiserror::Error, Debug)]
pub enum HashStructError {
    #[error("failed to serialize value for hashing: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub fn hash_struct(s: &impl Serialize) -> Result<String, HashStructError> {
    let mut hasher = Sha1::new();
    hasher.update(serde_json::to_string(s)?);
    Ok(hex::encode(hasher.finalize()))
}
