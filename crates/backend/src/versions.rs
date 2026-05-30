use std::sync::Arc;

use generate::{
    instance::VANILLA_MANIFEST_URL,
    loader::{
        fabric::FabricVersionsMeta,
        forge::{ForgeMavenMetadata, NeoforgeMavenMetadata},
    },
};
use instance::manifest::VanillaVersionManifest;
use launcher_bridge::{FrontendSender, LocalLoader, MessageToFrontend};

pub fn start_fetch_local_create_versions(client: reqwest::Client, frontend: FrontendSender) {
    tokio::spawn(async move {
        let message = match VanillaVersionManifest::fetch(&client, &VANILLA_MANIFEST_URL).await {
            Ok(manifest) => {
                let versions: Arc<[(String, String)]> = manifest
                    .versions
                    .iter()
                    .map(|entry| (entry.id.clone(), entry.type_.clone()))
                    .collect();
                MessageToFrontend::LocalCreateVersionsUpdated {
                    versions,
                    latest_release: manifest.latest.release,
                    error: None,
                }
            }
            Err(error) => MessageToFrontend::LocalCreateVersionsUpdated {
                versions: Arc::new([]),
                latest_release: String::new(),
                error: Some(Arc::from(error.to_string())),
            },
        };
        frontend.send(message);
    });
}

pub fn start_fetch_loader_versions(
    client: reqwest::Client,
    frontend: FrontendSender,
    minecraft_version: String,
    loader: LocalLoader,
) {
    tokio::spawn(async move {
        let message = match fetch_loader_versions(&client, &minecraft_version, loader).await {
            Ok(versions) => MessageToFrontend::LoaderVersionsUpdated {
                minecraft_version,
                loader,
                versions: versions.into(),
                error: None,
            },
            Err(error) => MessageToFrontend::LoaderVersionsUpdated {
                minecraft_version,
                loader,
                versions: Arc::new([]),
                error: Some(Arc::from(error.to_string())),
            },
        };
        frontend.send(message);
    });
}

async fn fetch_loader_versions(
    client: &reqwest::Client,
    minecraft_version: &str,
    loader: LocalLoader,
) -> Result<Vec<String>, String> {
    match loader {
        LocalLoader::Vanilla => Ok(vec![]),
        LocalLoader::Fabric => FabricVersionsMeta::fetch(minecraft_version)
            .await
            .map(|meta| {
                meta.get_versions()
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            })
            .map_err(|error| error.to_string()),
        LocalLoader::Forge => ForgeMavenMetadata::fetch(client)
            .await
            .map(|meta| meta.get_matching_versions(minecraft_version))
            .map_err(|error| error.to_string()),
        LocalLoader::Neoforge => NeoforgeMavenMetadata::fetch(client)
            .await
            .map(|meta| meta.get_matching_versions(minecraft_version))
            .map_err(|error| error.to_string()),
    }
}
