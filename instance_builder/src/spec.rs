use launcher_auth::providers::AuthProviderConfig;
use log::{debug, error, info, warn};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::fs;

use shared::{
    files::sync_mapping,
    generate::{
        extra::{ExtraMetadataGenerator, IncludeConfig, IncludeRule},
        manifest::get_version_info,
    },
    loader_generator::{
        fabric::FabricGenerator,
        forge::{ForgeGenerator, Loader},
        generator::VersionGenerator,
        vanilla::VanillaGenerator,
    },
    paths::{
        get_extra_metadata_path, get_metadata_path, get_minecraft_dir, get_versions_dir,
        get_versions_extra_dir,
    },
    utils::{VANILLA_MANIFEST_URL, get_vanilla_version_info},
    version::{asset_metadata::AssetsMetadata, version_manifest::VersionManifest},
};

use crate::{
    generate::{mapping::get_mapping, patch::replace_download_urls, sync::sync_version},
    progress::TerminalProgressBar,
    utils::{exec_string_command, get_assets_dir, get_replaced_metadata_dir},
};

fn vanilla() -> String {
    "vanilla".to_string()
}

#[derive(Deserialize)]
pub struct Instances {
    pub name: String,
    pub minecraft_version: String,

    #[serde(default = "vanilla")]
    pub loader_name: String,

    pub loader_version: Option<String>,

    #[serde(default)]
    pub include: Vec<IncludeRule>,

    pub include_from: Option<String>,

    pub auth_backend: Option<AuthProviderConfig>,

    pub recommended_xmx: Option<String>,

    pub exec_before: Option<String>,
    pub exec_after: Option<String>,
}

#[derive(Deserialize)]
pub struct Spec {
    pub download_server_base: String,
    pub resources_url_base: Option<String>,

    #[serde(default)]
    pub replace_download_urls: bool,

    pub version_manifest_url: Option<String>,

    pub instances: Vec<Instances>,
    pub exec_before_all: Option<String>,
    pub exec_after_all: Option<String>,
}

pub fn get_manifest_path(data_dir: &Path) -> PathBuf {
    data_dir.join("version_manifest.json")
}

impl Spec {
    pub async fn from_file(path: &Path) -> anyhow::Result<Spec> {
        let content = fs::read_to_string(path).await?;
        let spec = serde_json::from_str(&content)?;
        Ok(spec)
    }

    pub async fn generate(
        self,
        output_dir: &Path,
        work_dir: &Path,
        delete_remote_instances: Option<&HashSet<String>>,
    ) -> anyhow::Result<()> {
        if let Some(command) = &self.exec_before_all {
            exec_string_command(command).await?;
        }

        info!("Fetching version manifest");
        let vanilla_manifest = VersionManifest::fetch(VANILLA_MANIFEST_URL).await?;

        if delete_remote_instances.is_some() && self.version_manifest_url.is_none() {
            warn!(
                "--delete-remote flag is set but version_manifest_url spec option is not; ignoring the flag"
            );
        }

        let mut version_manifest = if let Some(version_manifest_url) = &self.version_manifest_url {
            info!("Fetching remote version manifest from: {version_manifest_url}");
            match VersionManifest::fetch(version_manifest_url).await {
                Ok(mut manifest) => {
                    info!(
                        "Successfully fetched remote manifest with {} versions",
                        manifest.versions.len()
                    );
                    if let Some(to_delete) = delete_remote_instances
                        && !to_delete.is_empty()
                    {
                        let before = manifest.versions.len();
                        manifest.versions.retain(|v| {
                            let name = v.get_name();
                            !to_delete.contains(name.as_str())
                        });
                        let removed = before - manifest.versions.len();
                        if removed > 0 {
                            info!("Removed {removed} remote instance(s) from fetched manifest");
                        } else {
                            warn!("No remote instances matched the provided delete list");
                        }
                    }
                    manifest
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch remote version manifest: {e}. Starting with empty manifest."
                    );
                    VersionManifest { versions: vec![] }
                }
            }
        } else {
            VersionManifest { versions: vec![] }
        };
        let mut synced_metadata = HashSet::new();
        let mut mapping = HashMap::new();

        for version in self.instances {
            if let Some(command) = &version.exec_before {
                exec_string_command(command).await?;
            }

            let vanilla_version_info =
                get_vanilla_version_info(&vanilla_manifest, &version.minecraft_version)?;

            let progress_bar = Arc::new(TerminalProgressBar::new());

            let generator: Box<dyn VersionGenerator> = match version.loader_name.as_str() {
                "vanilla" => {
                    if version.loader_version.is_some() {
                        warn!("Ignoring loader version for vanilla version");
                    }

                    Box::new(VanillaGenerator::new(
                        version.name.clone(),
                        vanilla_version_info,
                    ))
                }

                "fabric" => Box::new(FabricGenerator::new(
                    version.name.clone(),
                    vanilla_version_info,
                    version.loader_version.clone(),
                )),

                "forge" => Box::new(ForgeGenerator::new(
                    version.name.clone(),
                    vanilla_version_info,
                    Loader::Forge,
                    version.loader_version.clone(),
                    progress_bar.clone(),
                )),

                "neoforge" => Box::new(ForgeGenerator::new(
                    version.name.clone(),
                    vanilla_version_info,
                    Loader::Neoforge,
                    version.loader_version.clone(),
                    progress_bar.clone(),
                )),

                _ => {
                    error!("Unsupported loader name: {}", version.loader_name);
                    continue;
                }
            };

            let mut workdir_paths_to_copy = vec![];

            let mut result = generator.generate(work_dir).await?;
            let mut replaced_metadata = HashMap::new();
            if self.replace_download_urls {
                let versions_dir = get_versions_dir(output_dir);
                let replaced_metadata_dir = get_replaced_metadata_dir(work_dir);

                for metadata in result.metadata.iter_mut() {
                    if synced_metadata.contains(&metadata.id) {
                        info!("Skipping {}, it is already synced", &metadata.id);
                        continue;
                    }
                    info!("Syncing {}", &metadata.id);

                    let sync_result = sync_version(metadata, work_dir).await?;
                    if let Some(asset_index) = &metadata.asset_index {
                        let assets_dir = get_assets_dir(work_dir);
                        let asset_index_path =
                            AssetsMetadata::get_path(&assets_dir, &asset_index.id).await?;
                        workdir_paths_to_copy.push(asset_index_path);
                    }
                    workdir_paths_to_copy.extend(sync_result.paths_to_copy);

                    replace_download_urls(metadata, &self.download_server_base, work_dir).await?;
                    metadata.save(&replaced_metadata_dir).await?;

                    synced_metadata.insert(metadata.id.clone());

                    let replaced_metadata_path =
                        get_metadata_path(&replaced_metadata_dir, &metadata.id);
                    replaced_metadata.insert(metadata.id.clone(), replaced_metadata_path.clone());
                    mapping.insert(
                        get_metadata_path(&versions_dir, &metadata.id),
                        replaced_metadata_path,
                    );
                }
            } else {
                let versions_dir = get_versions_dir(work_dir);
                for metadata in result.metadata.iter_mut() {
                    workdir_paths_to_copy.push(get_metadata_path(&versions_dir, &metadata.id));
                }
            }
            workdir_paths_to_copy.extend(result.extra_libs_paths.clone());

            let resources_url_base = if self.replace_download_urls {
                self.resources_url_base.clone()
            } else {
                None
            };

            let include_config = if let Some(include_from) = version.include_from {
                Some(IncludeConfig {
                    include: version.include,
                    include_from,
                    download_server_base: self.download_server_base.clone(),
                    resources_url_base,
                })
            } else {
                if !version.include.is_empty() {
                    warn!("Ignoring include, include_from is not set");
                }
                None
            };

            let extra_generator = ExtraMetadataGenerator::new(
                version.name.clone(),
                include_config,
                result.extra_libs_paths,
                version.auth_backend,
                version.recommended_xmx,
            );
            let extra_generator_result = extra_generator.generate(work_dir).await?;
            mapping.extend(extra_generator_result.include_mapping.into_iter().map(
                |(include_entry, source_path)| {
                    let minecraft_dir = get_minecraft_dir(output_dir, &version.name);
                    (minecraft_dir.join(include_entry), source_path)
                },
            ));

            let versions_extra_dir = get_versions_extra_dir(work_dir);
            workdir_paths_to_copy.push(get_extra_metadata_path(&versions_extra_dir, &version.name));

            info!("Getting version info for {}", &version.name);
            let version_info = get_version_info(
                work_dir,
                &result.metadata,
                &version.name,
                Some(self.download_server_base.as_str()),
                &replaced_metadata,
            )
            .await?;

            version_manifest
                .versions
                .retain(|v| v.get_name() != version_info.get_name());
            version_manifest.versions.push(version_info);

            mapping.extend(get_mapping(output_dir, work_dir, &workdir_paths_to_copy)?);

            if let Some(command) = &version.exec_after {
                exec_string_command(command).await?;
            }

            info!("Finished generating version {}", &version.name);
        }

        info!("Syncing {} entries", mapping.len());
        debug!("Sync mapping (target->source): {mapping:?}");
        let stats = sync_mapping(output_dir, &mapping).await?;
        info!(
            "Synced {} files (copied {}, deleted {})",
            stats.total_files, stats.copied_files, stats.deleted_files
        );

        let manifest_path = get_manifest_path(output_dir);
        version_manifest.save_to_file(&manifest_path).await?;

        if let Some(command) = &self.exec_after_all {
            exec_string_command(command).await?;
        }
        Ok(())
    }
}
