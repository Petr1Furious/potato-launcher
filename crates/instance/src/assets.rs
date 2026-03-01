use std::{collections::HashMap, path::PathBuf};

use utils::{
    files::{self, CheckTask},
    paths::{AssetsDir, BaseUrl, DataDir},
};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Deserialize, Serialize, Clone)]
pub struct AssetIndex {
    pub id: String,
    pub sha1: String,
    pub url: Url,
}

fn get_asset_index_path(data_dir: &DataDir, asset_id: &str) -> PathBuf {
    AssetsDir::root().asset_index_path(asset_id).to_fs(data_dir)
}

impl AssetIndex {
    pub fn get_check_task(&self, data_dir: &DataDir) -> CheckTask {
        CheckTask {
            url: self.url.clone(),
            remote_sha1: Some(self.sha1.clone()),
            path: get_asset_index_path(data_dir, &self.id),
        }
    }
}

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

    pub async fn read_or_download(
        client: &reqwest::Client,
        asset_index: &AssetIndex,
        data_dir: &DataDir,
    ) -> anyhow::Result<Self> {
        let check_task = asset_index.get_check_task(data_dir);
        if let Some(download_task) = files::get_download_task(&check_task).await? {
            files::download_file_parsed(client, &download_task).await
        } else {
            files::read_file_parsed(&get_asset_index_path(data_dir, &asset_index.id)).await
        }
    }

    pub fn get_check_tasks(
        &self,
        data_dir: &DataDir,
        download_server_base: &BaseUrl,
        check_hashes: bool,
    ) -> anyhow::Result<Vec<CheckTask>> {
        let mut check_tasks = vec![];

        check_tasks.extend(
            self.objects
                .values()
                .map(|object| {
                    let rel_path = AssetsDir::root()
                        .assets_object_dir()
                        .object_path(&object.hash);
                    Ok(CheckTask {
                        url: rel_path.to_url(download_server_base),
                        path: rel_path.to_fs(data_dir),
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
        tokio::fs::write(get_asset_index_path(data_dir, asset_id), data).await?;
        Ok(())
    }
}
