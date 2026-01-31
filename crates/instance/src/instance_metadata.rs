use std::collections::HashMap;

use relative_path::RelativePathBuf;
use serde::{Deserialize, Serialize};

use launcher_auth::providers::AuthProviderConfig;
use utils::{
    files::{self, CheckTask},
    paths::{DataDir, InstanceDirFS, VersionsDir},
    progress,
};

use crate::{
    os::{get_os_name, get_system_arch},
    overrides::with_overrides,
};

use super::{
    manifest::InstanceManifestEntry,
    version_metadata::{Arguments, AssetIndex, Library, VersionMetadata},
};

#[derive(Deserialize, Serialize, Debug)]
pub struct Object {
    pub path: RelativePathBuf,
    pub sha1: String,
    pub url: String,
}

fn yes() -> bool {
    true
}

/// A single include rule for a file or a directory
/// Has rules for what to do when file/directory contents differ from the remote
#[derive(Deserialize, Serialize)]
pub struct Include {
    pub path: RelativePathBuf,

    // TODO rewrite
    #[serde(default = "yes")]
    pub overwrite: bool,

    #[serde(default = "yes")]
    pub delete_extra: bool,

    #[serde(default)]
    pub recursive: bool,

    /// contails either the file or all files in the directory
    #[serde(default)]
    pub objects: Vec<Object>,
}

/// Complete metadata for a single instance.
/// Contains all used minecraft versions (also known as client.json)
/// e.g. "fabric-loader-0.18.4-1.21.11" and "1.21.11" for a fabric 1.21.11 instance
/// and other fields
#[derive(Deserialize, Serialize)]
pub struct InstanceMetadata {
    /// instance name
    #[serde(default)]
    name: String,

    /// auth backend to use for this instance
    #[serde(default)]
    auth_backend: Option<AuthProviderConfig>,

    /// additional files to include with the instance
    /// (e.g. mods, configs, server.dat, etc.)
    /// and rules for what to do when file/directory contents differ from the remote
    #[serde(default)]
    include: Vec<Include>,

    /// base URL for assets
    /// if not set, the launcher will download assets from Mojang servers
    #[serde(default)]
    resources_url_base: Option<String>,

    /// extra (neo)forge libraries to include with the instance
    #[serde(default)]
    extra_forge_libs: Vec<Library>,

    /// default JVM RAM limit (`-Xmx`) for this version
    /// e.g. "8192M"
    default_xmx: Option<String>,

    // used minecraft versions (client.json) ordered from parent to child
    // e.g. ["1.21.11", "fabric-loader-0.18.4-1.21.11"] since fabric-loader-0.18.4-1.21.11 inherits from 1.21.11
    #[serde(default)]
    versions: Vec<VersionMetadata>,
}

const DEFAULT_RESOURCES_URL_BASE: &str = "https://resources.download.minecraft.net";

#[derive(thiserror::Error, Debug)]
pub enum InstanceMetadataError {
    #[error("Missing asset index")]
    MissingAssetIndex,
    #[error("Missing client download")]
    MissingClientDownload,
    #[error("Missing version metadata")]
    MissingVersionMetadata,
}

impl InstanceMetadata {
    pub async fn read_local(
        entry: &InstanceManifestEntry,
        instance_dir: &InstanceDirFS,
    ) -> anyhow::Result<Option<Self>> {
        let meta_path = instance_dir.meta_path();
        if !meta_path.exists() {
            return Ok(None);
        }

        let meta_file = tokio::fs::read(meta_path).await?;
        let mut metadata: InstanceMetadata = serde_json::from_slice(&meta_file)?;

        if metadata.name.is_empty() {
            metadata.name = entry.name.clone();
        }

        if metadata.versions.is_empty() {
            let mut versions = Vec::with_capacity(entry.versions.len());
            for info in &entry.versions {
                versions
                    .push(VersionMetadata::read_local(instance_dir.data_dir(), &info.id).await?);
            }
            metadata.versions = versions;
        }

        Ok(Some(metadata))
    }

    pub async fn read_or_download(
        client: &reqwest::Client,
        entry: &InstanceManifestEntry,
        instance_dir: &InstanceDirFS,
    ) -> anyhow::Result<Self> {
        let check_tasks = entry.get_check_tasks(instance_dir);
        let download_tasks =
            files::get_download_tasks(check_tasks, progress::no_progress_bar()).await?;
        files::download_files(client, download_tasks, progress::no_progress_bar()).await?;

        Self::read_local(entry, instance_dir)
            .await?
            .ok_or_else(|| InstanceMetadataError::MissingVersionMetadata.into())
    }

    pub async fn save(&self, instance_dir: &InstanceDirFS) -> anyhow::Result<()> {
        let path = instance_dir.meta_path();
        let serialized = serde_json::to_string(self)?;
        tokio::fs::write(path, serialized).await?;

        Ok(())
    }

    pub fn get_resources_url_base(&self) -> &str {
        self.resources_url_base
            .as_deref()
            .unwrap_or(DEFAULT_RESOURCES_URL_BASE)
    }

    pub fn get_java_version(&self) -> String {
        self.versions
            .first()
            .and_then(|metadata| metadata.java_version.as_ref())
            .map(|x| x.major_version.to_string())
            .unwrap_or_else(|| "8".to_string())
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_client_check_task(&self, data_dir: &DataDir) -> anyhow::Result<CheckTask> {
        let version = self
            .versions
            .first()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?;

        if let Some(downloads) = version.downloads.as_ref()
            && let Some(client) = downloads.client.as_ref()
        {
            Ok(client.get_check_task(
                &VersionsDir::root()
                    .client_jar_path(self.get_id())
                    .to_fs(data_dir),
            ))
        } else {
            Err(InstanceMetadataError::MissingClientDownload.into())
        }
    }

    pub fn get_auth_provider(&self) -> Option<&AuthProviderConfig> {
        self.auth_backend.as_ref()
    }

    pub fn get_libraries_with_overrides(&self) -> Vec<Library> {
        let os_name = get_os_name();
        let arch = get_system_arch();

        let all_libraries = self
            .versions
            .iter()
            .rev() // prioritize child libraries
            .flat_map(|metadata| with_overrides(&metadata.libraries, &metadata.id));

        let mut existing_names = HashMap::new();
        all_libraries
            .filter(|library| library.applies_to_os(&os_name, &arch))
            .filter(|library| {
                // Newer NeoForge versions add duplicate asm library
                let (name, version) = library.get_name_and_version();
                if let Some(prev_version) = existing_names.get(&name) {
                    version == *prev_version
                } else {
                    existing_names.insert(name, version);
                    true
                }
            })
            .collect()
    }

    pub fn get_id(&self) -> &str {
        &self.versions.last().unwrap().id
    }

    pub fn get_parent_id(&self) -> &str {
        &self.versions.first().unwrap().id
    }

    pub fn get_asset_index(&self) -> anyhow::Result<&AssetIndex> {
        let version = self
            .versions
            .first()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?;
        Ok(version
            .asset_index
            .as_ref()
            .ok_or(InstanceMetadataError::MissingAssetIndex)?)
    }

    pub fn get_arguments(&self) -> anyhow::Result<Arguments> {
        let first = self
            .versions
            .first()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?;
        let mut merged_arguments = first.get_arguments()?;

        for metadata in &self.versions[1..] {
            if let Some(arguments) = metadata.arguments.clone() {
                merged_arguments.game.extend(arguments.game);
                merged_arguments.jvm.extend(arguments.jvm);
            } else if metadata.minecraft_arguments.is_some() {
                merged_arguments = metadata.get_arguments()?;
            }
        }

        Ok(merged_arguments)
    }

    pub fn get_main_class(&self) -> anyhow::Result<&str> {
        Ok(self
            .versions
            .last()
            .ok_or(InstanceMetadataError::MissingVersionMetadata)?
            .main_class
            .as_str())
    }

    pub fn get_extra_forge_libs(&self) -> Vec<Library> {
        self.extra_forge_libs.clone()
    }

    pub fn get_default_xmx(&self) -> Option<&str> {
        self.default_xmx.as_deref()
    }
}
