use instance::{manifest::VanillaVersionManifest, version_metadata::VersionMetadata};
use launcher_auth::providers::AuthProviderConfig;
use log::{debug, error, info, warn};
use reqwest::Client;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use utils::{
    files,
    paths::{BaseUrl, DataDir},
    utils::VANILLA_MANIFEST_URL,
};

pub enum Loader {
    Vanilla,
    Fabric,
    Forge,
    Neoforge,
}

pub struct IncludeRule {
    pub path: String,

    pub overwrite: bool,
    pub delete_extra: bool,
    pub recursive: bool,
}

pub struct InstanceGenerator {
    pub client: Client,

    pub name: String,
    pub minecraft_version: String,

    pub loader: Loader,
    // latest/recommended will be used if not set
    pub loader_version: Option<String>,

    pub include: Vec<IncludeRule>,
    pub include_from: Option<String>,

    pub auth_backend: Option<AuthProviderConfig>,

    pub recommended_xmx: Option<String>,

    pub download_server_base: Option<BaseUrl>,
    pub replace_download_urls: bool,
}

#[derive(thiserror::Error, Debug)]
enum InstanceGeneratorError {
    #[error("Vanilla version not found")]
    VanillaVersionNotFound,
}

impl InstanceGenerator {
    pub async fn generate(
        self,
        output_dir: &DataDir,
        work_dir: &DataDir,
        delete_remote_instances: Option<&HashSet<String>>,
    ) -> anyhow::Result<()> {
        info!("Fetching version manifest");
        let vanilla_manifest =
            VanillaVersionManifest::fetch(&self.client, &VANILLA_MANIFEST_URL).await?;
        let metadata_info = vanilla_manifest
            .get_entry(&self.minecraft_version)
            .ok_or(InstanceGeneratorError::VanillaVersionNotFound)?
            .to_metadata_info();

        let vanilla_metadata =
            VersionMetadata::read_or_download(&self.client, &metadata_info, output_dir).await?;

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
