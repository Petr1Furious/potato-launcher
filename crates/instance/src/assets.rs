use std::{collections::HashMap, path::PathBuf};

use utils::{
    files::{self, CheckTask},
    paths::{AssetsDir, DataDir, ResourcesUrlBase},
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

#[derive(thiserror::Error, Debug)]
pub enum AssetsMetadataError {
    #[error("network request failed while fetching assets metadata: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to determine whether asset index needs download: {0}")]
    DownloadCheckIo(std::io::Error),
    #[error("failed to parse downloaded assets metadata JSON: {0}")]
    DownloadFileParsed(#[from] files::DownloadFileParsedError),
    #[error("failed to read local assets metadata JSON: {0}")]
    ReadFileParsed(#[from] files::ReadFileParsedError),
    #[error("failed to build asset object URL: {0}")]
    ParseUrl(#[from] url::ParseError),
    #[error("failed to write assets metadata JSON file: {0}")]
    WriteFileJson(#[from] files::WriteFileJsonError),
}

impl AssetsMetadata {
    pub async fn fetch(url: &str) -> Result<Self, AssetsMetadataError> {
        let client = Client::new();
        let response = client.get(url).send().await?.json().await?;
        Ok(response)
    }

    pub async fn read_or_download(
        client: &reqwest::Client,
        asset_index: &AssetIndex,
        data_dir: &DataDir,
    ) -> Result<Self, AssetsMetadataError> {
        let check_task = asset_index.get_check_task(data_dir);
        if let Some(download_task) = files::get_download_task(&check_task)
            .await
            .map_err(AssetsMetadataError::DownloadCheckIo)?
        {
            Ok(files::download_file_parsed(client, &download_task).await?)
        } else {
            Ok(files::read_file_parsed(&get_asset_index_path(data_dir, &asset_index.id)).await?)
        }
    }

    pub fn get_check_tasks(
        &self,
        data_dir: &DataDir,
        resources_url_base: &ResourcesUrlBase,
        check_hashes: bool,
    ) -> Result<Vec<CheckTask>, AssetsMetadataError> {
        let mut check_tasks = vec![];

        check_tasks.extend(
            self.objects
                .values()
                .map(|object| {
                    let rel_path = AssetsDir::root()
                        .assets_object_dir()
                        .object_path(&object.hash);
                    resources_url_base
                        .object_url(&object.hash)
                        .map(|url| CheckTask {
                            url,
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

    pub async fn save_to_file(
        &self,
        asset_id: &str,
        data_dir: &DataDir,
    ) -> Result<(), AssetsMetadataError> {
        Ok(files::write_file_json(&get_asset_index_path(data_dir, asset_id), self).await?)
    }
}
