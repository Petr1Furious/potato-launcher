use instance::version_metadata::VersionMetadata;
use log::info;
use reqwest::Client;
use serde::Deserialize;
use utils::paths::DataDir;

const FABRIC_META_BASE_URL: &str = "https://meta.fabricmc.net/v2/versions/loader/";

#[derive(Deserialize)]
struct FabricVersionLoader {
    version: String,
}

#[derive(Deserialize)]
struct FabricVersionMeta {
    loader: FabricVersionLoader,
}

pub struct FabricVersionsMeta {
    versions: Vec<FabricVersionMeta>,
}

#[derive(thiserror::Error, Debug)]
pub enum FabricGeneratorError {
    #[error("no Fabric versions found for game version {0}")]
    NoVersionsFound(String),
    #[error("network request failed while fetching Fabric metadata: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed while processing Fabric version metadata: {0}")]
    VersionMetadata(#[from] instance::version_metadata::VersionMetadataError),
}

impl FabricVersionsMeta {
    pub async fn fetch(game_version: &str) -> Result<Self, FabricGeneratorError> {
        let fabric_manifest_url = format!("{FABRIC_META_BASE_URL}{game_version}");
        let client = Client::new();
        let response = client
            .get(&fabric_manifest_url)
            .send()
            .await?
            .error_for_status()?;
        let fabric_versions: Vec<FabricVersionMeta> = response.json().await?;
        Ok(Self {
            versions: fabric_versions,
        })
    }

    pub fn get_versions(&self) -> Vec<&str> {
        self.versions
            .iter()
            .map(|version| version.loader.version.as_str())
            .collect()
    }

    pub fn get_latest_version(&self) -> Option<&str> {
        self.get_versions().first().copied()
    }
}

async fn download_fabric_metadata(
    client: &Client,
    minecraft_version: &str,
    loader_version: &str,
    data_dir: &DataDir,
) -> Result<VersionMetadata, FabricGeneratorError> {
    let fabric_metadata_url =
        format!("{FABRIC_META_BASE_URL}{minecraft_version}/{loader_version}/profile/json");
    let version_metadata = VersionMetadata::fetch(client, &fabric_metadata_url).await?;
    version_metadata.save(data_dir).await?;
    Ok(version_metadata)
}

pub struct FabricGenerator {
    minecraft_version: String,
    loader_version: Option<String>,
}

impl FabricGenerator {
    pub fn new(minecraft_version: &str, loader_version: Option<String>) -> Self {
        Self {
            minecraft_version: minecraft_version.to_string(),
            loader_version,
        }
    }
}

impl FabricGenerator {
    pub async fn generate(
        &self,
        client: &Client,
        output_dir: &DataDir,
    ) -> Result<VersionMetadata, FabricGeneratorError> {
        info!(
            "Generating Fabric {}, minecraft version {}",
            self.loader_version.as_deref().unwrap_or("<auto>"),
            self.minecraft_version
        );

        let fabric_version = match &self.loader_version {
            Some(loader_version) => loader_version.clone(),
            None => {
                let meta = FabricVersionsMeta::fetch(&self.minecraft_version).await?;
                let version =
                    meta.get_latest_version()
                        .ok_or(FabricGeneratorError::NoVersionsFound(
                            self.minecraft_version.clone(),
                        ))?;
                info!("Loader version not specified, using latest version: {version}");
                version.to_string()
            }
        };

        info!("Downloading Fabric version metadata");
        let fabric_metadata =
            download_fabric_metadata(client, &self.minecraft_version, &fabric_version, output_dir)
                .await?;

        info!("Fabric \"{}\" generated", &fabric_version);

        Ok(fabric_metadata)
    }
}
