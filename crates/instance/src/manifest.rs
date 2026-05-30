use std::path::Path;

use launcher_auth::providers::AuthProviderConfig;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;
use utils::{
    files::{self, CheckTask},
    paths::{DataDir, InstanceDirFS, VersionsDir},
};

#[derive(thiserror::Error, Debug)]
pub enum ManifestError {
    #[error("network request failed while fetching manifest: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to write manifest JSON file: {0}")]
    WriteFileJson(#[from] files::WriteFileJsonError),
}

/// A single entry in the vanilla version manifest
/// https://piston-meta.mojang.com/mc/game/version_manifest_v2.json
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VanillaManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub url: Url,
    pub time: String,
    pub release_time: String,
    pub sha1: String,
    // will anyone ever need this?
    pub compliance_level: Option<u8>,
}

impl VanillaManifestEntry {
    pub fn get_check_task(&self, data_dir: &DataDir) -> CheckTask {
        let path = VersionsDir::root().metadata_path(&self.id).to_fs(data_dir);
        CheckTask {
            url: self.url.clone(),
            remote_sha1: Some(self.sha1.clone()),
            path,
        }
    }

    pub fn to_metadata_info(&self) -> VersionMetadataInfo {
        VersionMetadataInfo {
            id: self.id.clone(),
            url: self.url.clone(),
            sha1: self.sha1.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

/// The vanilla version manifest
/// https://piston-meta.mojang.com/mc/game/version_manifest_v2.json
#[derive(Serialize, Deserialize, Clone)]
pub struct VanillaVersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VanillaManifestEntry>,
}

impl VanillaVersionManifest {
    pub async fn fetch(client: &Client, url: &Url) -> Result<Self, ManifestError> {
        let res = client
            .get(url.clone())
            .send()
            .await?
            .error_for_status()?
            .json::<Self>()
            .await?;
        Ok(res)
    }

    pub fn get_entry(&self, minecraft_version: &str) -> Option<&VanillaManifestEntry> {
        self.versions.iter().find(|v| v.id == minecraft_version)
    }

    pub async fn save_to_file(&self, manifest_path: &Path) -> Result<(), ManifestError> {
        Ok(files::write_file_json(manifest_path, self).await?)
    }
}

/// Used minecraft versions to allow avoiding extra round trip.
/// (manifest -> versions + instance) instead of (manifest -> instance -> versions)
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct VersionMetadataInfo {
    pub id: String,
    pub url: Url,
    pub sha1: String,
}

impl VersionMetadataInfo {
    pub fn to_check_task(&self, data_dir: &DataDir) -> CheckTask {
        CheckTask {
            url: self.url.clone(),
            remote_sha1: Some(self.sha1.clone()),
            path: VersionsDir::root().metadata_path(&self.id).to_fs(data_dir),
        }
    }
}

/// A single entry in the instance manifest.
/// This is used to get instance metadata.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct InstanceManifestEntry {
    pub name: String,
    pub url: Url,
    pub sha1: String,
    /// Must always match `InstanceMetadata.auth_backend` for the entry's metadata.
    #[serde(default)]
    pub auth_backend: Option<AuthProviderConfig>,
}

impl InstanceManifestEntry {
    pub fn to_check_task(&self, instance_dir: &InstanceDirFS) -> CheckTask {
        CheckTask {
            url: self.url.clone(),
            remote_sha1: Some(self.sha1.clone()),
            path: instance_dir.meta_path(),
        }
    }
}

/// Replaces the vanilla version manifest.
/// This is the first metadata the launcher will fetch.
#[derive(Serialize, Deserialize, Clone)]
pub struct InstanceManifest {
    pub instances: Vec<InstanceManifestEntry>,
}

impl InstanceManifest {
    pub async fn fetch(client: &Client, url: &Url) -> Result<Self, ManifestError> {
        let res = client
            .get(url.clone())
            .send()
            .await?
            .error_for_status()?
            .json::<Self>()
            .await?;
        Ok(res)
    }

    pub async fn save_to_file(&self, manifest_path: &Path) -> Result<(), ManifestError> {
        Ok(files::write_file_json(manifest_path, self).await?)
    }
}
