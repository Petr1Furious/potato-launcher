use std::{collections::HashMap, path::PathBuf};

use utils::{
    files::{self, CheckTask},
    paths::{AssetsDir, DataDir},
    progress,
};

use super::version_metadata::AssetIndex;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize)]
pub struct ObjectData {
    pub hash: String,
}

#[derive(Serialize, Deserialize)]
pub struct AssetsMetadata {
    pub objects: HashMap<String, ObjectData>,
}

impl AssetsMetadata {
    pub async fn fetch(url: &str) -> anyhow::Result<Self> {
        let client = Client::new();
        let response = client.get(url).send().await?.json().await?;
        Ok(response)
    }

    pub fn get_path(data_dir: &DataDir, asset_id: &str) -> anyhow::Result<PathBuf> {
        Ok(AssetsDir::root().asset_index_path(asset_id).to_fs(data_dir))
    }

    pub async fn read_local(asset_id: &str, data_dir: &DataDir) -> anyhow::Result<Self> {
        let data = tokio::fs::read(Self::get_path(data_dir, asset_id)?).await?;
        let data: Self = serde_json::from_slice(&data)?;
        Ok(data)
    }

    pub async fn read_or_download(
        client: &reqwest::Client,
        asset_index: &AssetIndex,
        data_dir: &DataDir,
    ) -> anyhow::Result<Self> {
        let asset_index_path = Self::get_path(data_dir, &asset_index.id)?;
        let check_task = CheckTask {
            url: asset_index.url.clone(),
            remote_sha1: Some(asset_index.sha1.clone()),
            path: asset_index_path.clone(),
        };
        let check_tasks = vec![check_task];
        let download_tasks =
            files::get_download_tasks(check_tasks, progress::no_progress_bar()).await?;
        files::download_files(client, download_tasks, progress::no_progress_bar()).await?;
        Self::read_local(&asset_index.id, data_dir).await
    }

    pub fn get_check_tasks(
        &self,
        data_dir: &DataDir,
        resources_url_base: &Url,
        check_hashes: bool,
    ) -> anyhow::Result<Vec<CheckTask>> {
        let mut check_tasks = vec![];

        check_tasks.extend(
            self.objects
                .values()
                .map(|object| {
                    let rel_path = format!("{}/{}", &object.hash[..2], object.hash);
                    let url = resources_url_base.join(&rel_path)?;
                    Ok(CheckTask {
                        url,
                        path: AssetsDir::root()
                            .assets_object_dir()
                            .to_fs(data_dir)
                            .join(&object.hash[..2])
                            .join(&object.hash),
                        remote_sha1: if check_hashes {
                            Some(object.hash.clone())
                        } else {
                            None
                        },
                    })
                })
                .collect::<Result<Vec<_>, url::ParseError>>()?,
        );

        Ok(check_tasks)
    }

    pub async fn save_to_file(&self, asset_id: &str, data_dir: &DataDir) -> anyhow::Result<()> {
        let data = serde_json::to_vec(self)?;
        tokio::fs::write(Self::get_path(data_dir, asset_id)?, data).await?;
        Ok(())
    }
}
