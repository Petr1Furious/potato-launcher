use instance::{
    instance_metadata::{Include, InstanceMetadata, Object},
    manifest::VanillaVersionManifest,
    overrides::with_overrides,
    version_metadata::{Library, OsArch, VersionMetadata},
};
use launcher_auth::providers::AuthProviderConfig;
use log::{debug, info, warn};
use relative_path::{RelativePath, RelativePathBuf};
use reqwest::Client;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use utils::{
    files::{self, CheckTask, CopyTask},
    paths::{AssetsDir, BaseUrl, DataDir, InstanceDir, InstanceDirFS, LibrariesDir, VersionsDir},
    progress,
    utils::VANILLA_MANIFEST_URL,
};

use crate::loader::{
    fabric::FabricGenerator,
    forge::{self, ForgeGenerator},
};

#[derive(PartialEq, Eq)]
pub enum Loader {
    Vanilla,
    Fabric,
    Forge,
    Neoforge,
}

async fn get_objects(
    from_base_path: &Path,
    path: &Path,
    base_url: &BaseUrl,
    instance_dir: &InstanceDir,
    existing_paths: &HashSet<PathBuf>,
) -> anyhow::Result<Vec<Object>> {
    let files = files::get_files_ignore_paths(path, existing_paths)?;
    let hashes = files::hash_files(&files, progress::no_progress_bar()).await?;

    let mut objects = Vec::with_capacity(files.len());
    for (path, hash) in files.iter().zip(hashes.iter()) {
        objects.push(Object {
            path: RelativePathBuf::from_path(path.strip_prefix(from_base_path)?)?,
            sha1: hash.clone(),
            url: instance_dir.to_url(base_url),
        });
    }

    Ok(objects)
}

#[derive(thiserror::Error, Debug)]
pub enum ExtraForgeLibsError {
    #[error("Bad library name: {0}")]
    BadLibraryName(String),
    #[error("Extra forge library path is outside libraries dir: {0}")]
    OutsideLibrariesDir(PathBuf),
    #[error("Invalid forge library layout under libraries dir: {0}")]
    InvalidLayout(PathBuf),
    #[error("Missing file name for path: {0}")]
    MissingFileName(PathBuf),
}

async fn get_extra_forge_libs(
    extra_forge_libs_paths: &[PathBuf],
    data_dir: &DataDir,
    download_server_base: Option<&BaseUrl>,
) -> anyhow::Result<Vec<Library>> {
    struct ParsedExtraLib {
        source_path: PathBuf,
        rel_path: String,
        gav: String,
    }

    let libraries_dir = LibrariesDir::root().to_fs(data_dir);
    let mut parsed_libs = Vec::with_capacity(extra_forge_libs_paths.len());

    for path in extra_forge_libs_paths {
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jar") {
            debug!("Skipping non-jar file: {}", path.display());
            continue;
        }

        let rel_path = path
            .strip_prefix(&libraries_dir)
            .map_err(|_| ExtraForgeLibsError::OutsideLibrariesDir(path.clone()))?;
        let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");
        let rel_path = RelativePath::new(&rel_path_str);

        let parts = rel_path
            .components()
            .map(|x| x.as_str())
            .collect::<Vec<_>>();
        if parts.len() < 3 {
            return Err(ExtraForgeLibsError::InvalidLayout(path.clone()).into());
        }
        let version = parts[parts.len() - 2].to_string();
        let artifact = parts[parts.len() - 3].to_string();
        let group = parts[..parts.len() - 3].join(".");
        if group.is_empty() {
            return Err(ExtraForgeLibsError::InvalidLayout(path.clone()).into());
        }

        let file_stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ExtraForgeLibsError::MissingFileName(path.clone()))?
            .to_string();
        let stem_prefix = format!("{artifact}-{version}");
        let suffix = file_stem
            .strip_prefix(&stem_prefix)
            .ok_or_else(|| ExtraForgeLibsError::BadLibraryName(file_stem.clone()))?
            .replace("-", ":");

        parsed_libs.push(ParsedExtraLib {
            source_path: path.clone(),
            rel_path: rel_path_str,
            gav: format!("{group}:{artifact}:{version}{suffix}"),
        });
    }

    if let Some(download_server_base) = download_server_base {
        let paths_to_hash = parsed_libs
            .iter()
            .map(|lib| lib.source_path.clone())
            .collect::<Vec<_>>();
        let hashes = files::hash_files(&paths_to_hash, progress::no_progress_bar()).await?;

        Ok(parsed_libs
            .into_iter()
            .zip(hashes)
            .map(|(lib, sha1)| {
                let url = LibrariesDir::root()
                    .library_path(RelativePath::new(&lib.rel_path))
                    .to_url(download_server_base);
                Library::from_download(lib.gav, url, sha1)
            })
            .collect())
    } else {
        Ok(parsed_libs
            .into_iter()
            .map(|lib| Library::empty(lib.gav))
            .collect())
    }
}

pub struct IncludeRule {
    pub path: RelativePathBuf,

    pub overwrite: bool,
    pub delete_extra: bool,
    pub recursive: bool,
}

pub struct IncludeConfig {
    pub include_rules: Vec<IncludeRule>,
    pub include_from: Option<PathBuf>,
    pub download_server_base: BaseUrl,
    // Whether to download all files and replace
    // artifact URLs with the download server base
    pub replace_download_urls: bool,
}

pub struct InstanceGenerator {
    pub client: Client,
    pub instance_name: String,
    pub minecraft_version: String,
    pub loader: Loader,

    // latest/recommended will be used if not set
    pub loader_version: Option<String>,

    pub include_config: Option<IncludeConfig>,
    pub auth_backend: Option<AuthProviderConfig>,
    pub default_xmx: Option<String>,
}

#[derive(thiserror::Error, Debug)]
enum InstanceGeneratorError {
    #[error("Vanilla version not found")]
    VanillaVersionNotFound,
}

pub struct GeneratorResult {
    pub metadata: InstanceMetadata,
    pub check_tasks: Vec<CheckTask>,
    pub copy_tasks: Vec<CopyTask>,
    /// These are needed since the instance builder needs to know
    /// which files to keep. These paths do not include paths
    /// From check_tasks and copy_tasks.
    pub other_generated_files: Vec<PathBuf>,
}

impl InstanceGenerator {
    /// Generate metadata and tasks for an instance. This function is allowed to do
    /// some work (e.g. fetch the vanilla manifest) or hash included files, but it does
    /// not download or copy most of the files. `other_generated_files` contains paths
    /// to generated files not included in check/copy tasks.
    pub async fn generate(
        self,
        instance_dir: &InstanceDirFS,
        work_dir: &Path,
        os_arch: &OsArch,
    ) -> anyhow::Result<GeneratorResult> {
        let output_dir = instance_dir.data_dir();

        info!("Fetching version manifest");
        let vanilla_manifest =
            VanillaVersionManifest::fetch(&self.client, &VANILLA_MANIFEST_URL).await?;
        let metadata_info = vanilla_manifest
            .get_entry(&self.minecraft_version)
            .ok_or(InstanceGeneratorError::VanillaVersionNotFound)?
            .to_metadata_info();

        let vanilla_metadata =
            VersionMetadata::read_or_download(&self.client, &metadata_info, output_dir).await?;

        let mut metadata = vec![vanilla_metadata];
        let vanilla_metadata = metadata.first().expect("Vanilla metadata present");
        let mut check_tasks = vec![];
        let mut copy_tasks = vec![];
        let mut other_generated_files = vec![];

        let mut extra_forge_libs = vec![];
        match &self.loader {
            Loader::Vanilla => {
                if self.loader_version.is_some() {
                    warn!("Ignoring loader version for vanilla version");
                }
            }
            Loader::Forge | Loader::Neoforge => {
                let result = ForgeGenerator::new(
                    &vanilla_metadata,
                    if self.loader == Loader::Forge {
                        forge::Loader::Forge
                    } else {
                        forge::Loader::Neoforge
                    },
                    self.loader_version.clone(),
                )
                .generate(&self.client, output_dir, work_dir)
                .await?;
                metadata.push(result.metadata);

                let extra_forge_libs_paths = result
                    .extra_libs_copy_tasks
                    .iter()
                    .map(|task| task.target.clone())
                    .collect::<Vec<_>>();
                extra_forge_libs = get_extra_forge_libs(
                    &extra_forge_libs_paths,
                    &DataDir::new(result.installer_work_dir),
                    self.include_config
                        .as_ref()
                        .map(|config| &config.download_server_base),
                )
                .await?;
                copy_tasks.extend(result.extra_libs_copy_tasks);
            }
            Loader::Fabric => {
                let result =
                    FabricGenerator::new(&self.minecraft_version, self.loader_version.clone())
                        .generate(&self.client, output_dir)
                        .await?;
                metadata.push(result);
            }
        };

        for metadata in metadata.iter_mut() {
            metadata.libraries = with_overrides(&metadata.libraries, &metadata.id);
        }

        // add the version metadata files to the other generated files
        other_generated_files.extend(metadata.iter().map(|metadata| {
            VersionsDir::root()
                .metadata_path(&metadata.id)
                .to_fs(output_dir)
        }));

        let mut include = vec![];
        if let Some(include_config) = &self.include_config {
            if include_config.replace_download_urls {
                for metadata in metadata.iter() {
                    check_tasks.extend(
                        metadata
                            .get_check_tasks(
                                &self.client,
                                output_dir,
                                &include_config.download_server_base,
                                os_arch,
                            )
                            .await?,
                    );
                    if let Some(asset_index) = &metadata.asset_index {
                        // asset index is not included in the check tasks since it is downloaded
                        // by metadata.get_check_tasks to know other check tasks
                        other_generated_files.push(
                            AssetsDir::root()
                                .asset_index_path(&asset_index.id)
                                .to_fs(output_dir),
                        );
                    }
                }
            }

            if let Some(from) = &include_config.include_from {
                let mut existing_paths = HashSet::new();
                for rule in include_config.include_rules.iter() {
                    let objects = get_objects(
                        from,
                        &rule.path.to_path(from),
                        &include_config.download_server_base,
                        instance_dir.rel(),
                        &existing_paths,
                    )
                    .await?;
                    copy_tasks.extend(objects.iter().map(|object| CopyTask {
                        source: object.path.to_path(from),
                        target: object.path.to_path(instance_dir.to_fs()),
                    }));
                    include.push(Include {
                        path: rule.path.clone(),
                        overwrite: rule.overwrite,
                        delete_extra: rule.delete_extra,
                        recursive: rule.recursive,
                        objects,
                    });
                    existing_paths.insert(rule.path.to_path(from));
                }
            } else {
                warn!("Ignoring include rules, include_from is not set");
            }

            if include_config.replace_download_urls {
                let vanilla_metadata = metadata.first_mut().expect("Vanilla metadata present");
                info!(
                    "Adding check tasks for {} metadata's files",
                    &vanilla_metadata.id
                );
                check_tasks.extend(
                    vanilla_metadata
                        .get_check_tasks(
                            &self.client,
                            output_dir,
                            &include_config.download_server_base,
                            &OsArch::All,
                        )
                        .await?,
                );
                info!(
                    "Replacing download URLs in metadata for {}",
                    &vanilla_metadata.id
                );
                *vanilla_metadata = vanilla_metadata
                    .with_replaced_download_urls(&include_config.download_server_base, output_dir)
                    .await?;
                vanilla_metadata.save(output_dir).await?;
            }
        }

        let resources_url_base = if let Some(include_config) = &self.include_config {
            if include_config.replace_download_urls {
                Some(AssetsDir::root().to_url(&include_config.download_server_base))
            } else {
                None
            }
        } else {
            None
        };

        Ok(GeneratorResult {
            metadata: InstanceMetadata::new(
                self.instance_name,
                self.auth_backend,
                include,
                resources_url_base,
                extra_forge_libs,
                self.default_xmx,
                metadata,
                true,
            ),
            check_tasks,
            copy_tasks,
            other_generated_files,
        })
    }
}
