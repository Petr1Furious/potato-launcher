use std::path::Path;

use launcher_auth::providers::AuthProviderConfig;
use serde::{Deserialize, Serialize};

use crate::{files::CheckEntry, paths::get_extra_metadata_path};

use super::{version_manifest::VersionInfo, version_metadata::Library};

#[derive(Deserialize, Serialize, Debug)]
pub struct Object {
    pub path: String,
    pub sha1: String,
    pub url: String,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Serialize)]
pub struct Include {
    pub path: String,

    #[serde(default = "yes")]
    pub overwrite: bool,

    #[serde(default = "yes")]
    pub delete_extra: bool,

    #[serde(default)]
    pub recursive: bool,

    #[serde(default)]
    pub objects: Vec<Object>,
}

#[derive(Deserialize, Serialize)]
pub struct ExtraVersionMetadata {
    #[serde(default)]
    pub auth_backend: Option<AuthProviderConfig>,

    #[serde(default)]
    pub include: Vec<Include>,

    #[serde(default)]
    pub resources_url_base: Option<String>,

    #[serde(default)]
    pub extra_forge_libs: Vec<Library>,

    pub recommended_xmx: Option<String>,
}

impl ExtraVersionMetadata {
    pub async fn read_local(
        version_info: &VersionInfo,
        versions_extra_dir: &Path,
    ) -> anyhow::Result<Option<Self>> {
        if version_info.extra_metadata_url.is_none() || version_info.extra_metadata_sha1.is_none() {
            return Ok(None);
        }

        let extra_version_metadata_path =
            get_extra_metadata_path(versions_extra_dir, &version_info.get_name());
        let extra_version_metadata_file = tokio::fs::read(extra_version_metadata_path).await?;

        Ok(Some(serde_json::from_slice(&extra_version_metadata_file)?))
    }

    pub fn get_check_entry(
        version_info: &VersionInfo,
        versions_extra_dir: &Path,
    ) -> Option<CheckEntry> {
        if version_info.extra_metadata_url.is_none() || version_info.extra_metadata_sha1.is_none() {
            return None;
        }

        let url = version_info.extra_metadata_url.as_ref().unwrap();
        let sha1 = version_info.extra_metadata_sha1.as_ref().unwrap();

        Some(CheckEntry {
            url: url.clone(),
            remote_sha1: Some(sha1.clone()),
            path: get_extra_metadata_path(versions_extra_dir, &version_info.get_name()),
        })
    }

    pub async fn save(&self, version_name: &str, versions_extra_dir: &Path) -> anyhow::Result<()> {
        let path = get_extra_metadata_path(versions_extra_dir, version_name);
        let serialized = serde_json::to_string(self)?;
        tokio::fs::write(path, serialized).await?;

        Ok(())
    }
}
