pub mod adaptive_download;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod compat;
pub mod files;
pub mod instance_id;
pub mod java;
pub mod java_proxy;
pub mod logging;
pub mod mod_id;
pub mod paths;
pub mod progress;
pub mod vec_either_untagged;

use std::collections::HashSet;

use serde::Serialize;
use sha1::Digest as _;
use sha1::Sha1;

pub fn get_unique_name(existing_names: &HashSet<String>, name_base: &str) -> String {
    let taken = existing_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    instance_id::allocate_unique_name(&taken, name_base)
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
