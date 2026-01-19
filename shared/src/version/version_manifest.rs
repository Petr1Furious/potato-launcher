use std::path::Path;

use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct MetadataInfo {
    pub id: String,
    pub url: String,
    pub sha1: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct VersionInfo {
    pub id: String,
    pub url: String,
    pub sha1: String,

    pub name: Option<String>,

    #[serde(default)]
    pub inherits_from: Vec<MetadataInfo>,

    pub extra_metadata_url: Option<String>,
    pub extra_metadata_sha1: Option<String>,
}

impl VersionInfo {
    pub fn get_name(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => self.id.clone(),
        }
    }

    pub fn get_parent_metadata_info(&self) -> MetadataInfo {
        match self.inherits_from.first() {
            Some(parent_info) => parent_info.clone(),
            None => MetadataInfo {
                id: self.id.clone(),
                url: self.url.clone(),
                sha1: self.sha1.clone(),
            },
        }
    }

    pub fn get_metadata_info(&self) -> Vec<MetadataInfo> {
        let mut versions_info = vec![self.get_parent_metadata_info()];
        for version_info in &self.inherits_from {
            versions_info.push(version_info.clone());
        }
        versions_info.push(MetadataInfo {
            id: self.id.clone(),
            url: self.url.clone(),
            sha1: self.sha1.clone(),
        });
        versions_info
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct VersionManifest {
    pub versions: Vec<VersionInfo>,
}

impl VersionManifest {
    pub async fn fetch(url: &str) -> anyhow::Result<Self> {
        let client = Client::new();
        let res = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Self>()
            .await?;
        Ok(res)
    }

    pub async fn save_to_file(&self, manifest_path: &Path) -> anyhow::Result<()> {
        let manifest_str = serde_json::to_string(self)?;
        tokio::fs::write(manifest_path, manifest_str).await?;
        Ok(())
    }
}
