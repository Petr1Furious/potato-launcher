use instance::{
    authlib::default_authlib_injector_library,
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
use url::Url;
use utils::{
    files::{self, CheckTask, CopyTask},
    paths::{
        AssetsDir, BaseUrl, DataDir, InstanceDir, InstanceDirFS, LibrariesDir, ResourcesUrlBase,
    },
    progress,
};

use crate::loader::{
    fabric::FabricGenerator,
    forge::{self, ForgeGenerator},
};

lazy_static::lazy_static! {
    pub static ref VANILLA_MANIFEST_URL: Url = Url::parse("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json").unwrap();
}

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
) -> Result<Vec<Object>, GetObjectsError> {
    let files = files::get_files_ignore_paths(path, existing_paths)?;
    let hashes = files::hash_files(&files, progress::no_progress_bar()).await?;

    let mut objects = Vec::with_capacity(files.len());
    for (path, hash) in files.iter().zip(hashes.iter()) {
        let object_path = RelativePathBuf::from_path(path.strip_prefix(from_base_path)?)?;
        let object_url = instance_dir
            .minecraft_dir()
            .instance_object_path(&object_path)
            .to_url(base_url);
        objects.push(Object {
            path: object_path,
            sha1: hash.clone(),
            url: object_url,
        });
    }

    Ok(objects)
}

#[derive(thiserror::Error, Debug)]
pub enum ExtraForgeLibsError {
    #[error("bad library name: {0}")]
    BadLibraryName(String),
    #[error("extra forge library path is outside libraries dir: {0}")]
    OutsideLibrariesDir(PathBuf),
    #[error("invalid forge library layout under libraries dir: {0}")]
    InvalidLayout(PathBuf),
    #[error("missing file name for path: {0}")]
    MissingFileName(PathBuf),
}

#[derive(thiserror::Error, Debug)]
pub enum GetObjectsError {
    #[error("failed to enumerate include files: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to hash include files: {0}")]
    HashFiles(#[from] files::HashFilesError),
    #[error("include path is outside include_from root: {0}")]
    StripPrefix(#[from] std::path::StripPrefixError),
    #[error("failed to convert include path to relative path: {0}")]
    RelativePath(#[from] relative_path::FromPathError),
    #[error("failed to build include object URL: {0}")]
    Url(#[from] url::ParseError),
}

#[derive(thiserror::Error, Debug)]
pub enum GetExtraForgeLibsError {
    #[error("failed to parse extra forge library paths: {0}")]
    ExtraForgeLibs(#[from] ExtraForgeLibsError),
    #[error("failed to hash extra forge libraries: {0}")]
    HashFiles(#[from] files::HashFilesError),
}

async fn get_extra_forge_libs(
    extra_forge_libs_paths: &[PathBuf],
    data_dir: &DataDir,
    download_server_base: Option<&BaseUrl>,
) -> Result<Vec<Library>, GetExtraForgeLibsError> {
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
pub enum GenerateError {
    #[error("failed to fetch vanilla version manifest: {0}")]
    Manifest(#[from] instance::manifest::ManifestError),
    #[error("failed while reading/downloading version metadata: {0}")]
    VersionMetadata(#[from] instance::version_metadata::VersionMetadataError),
    #[error("requested minecraft version does not exist in vanilla manifest")]
    VanillaVersionNotFound,
    #[error("failed while generating Fabric metadata: {0}")]
    Fabric(#[from] crate::loader::fabric::FabricGeneratorError),
    #[error("failed while generating Forge metadata: {0}")]
    Forge(#[from] crate::loader::forge::ForgeGenerateError),
    #[error("failed while parsing extra forge libraries: {0}")]
    ExtraForgeLibs(#[from] GetExtraForgeLibsError),
    #[error("failed while collecting include objects: {0}")]
    GetObjects(#[from] GetObjectsError),
    #[error("failed while building authlib-injector check tasks: {0}")]
    AuthlibLibrary(#[from] instance::version_metadata::LibraryError),
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
    ) -> Result<GeneratorResult, GenerateError> {
        let output_dir = instance_dir.data_dir();
        let work_data_dir = DataDir::new(work_dir.to_path_buf());

        info!("Fetching version manifest");
        let vanilla_manifest =
            VanillaVersionManifest::fetch(&self.client, &VANILLA_MANIFEST_URL).await?;
        let metadata_info = vanilla_manifest
            .get_entry(&self.minecraft_version)
            .ok_or(GenerateError::VanillaVersionNotFound)?
            .to_metadata_info();

        let vanilla_metadata =
            VersionMetadata::read_or_download(&self.client, &metadata_info, &work_data_dir).await?;

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
                    vanilla_metadata,
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
                    .map(|task| task.source.clone())
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
                        .generate(&self.client)
                        .await?;
                metadata.push(result);
            }
        };

        for metadata in metadata.iter_mut() {
            metadata.libraries = with_overrides(&metadata.libraries, &metadata.id);
        }

        let mut include = vec![];
        if let Some(include_config) = &self.include_config {
            if include_config.replace_download_urls {
                for metadata in metadata.iter() {
                    check_tasks.extend(
                        metadata
                            .get_check_tasks(
                                &self.client,
                                output_dir,
                                &ResourcesUrlBase::default(),
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
                    if objects.is_empty() {
                        warn!("No objects found for rule: {}", rule.path);
                    }
                    copy_tasks.extend(objects.iter().map(|object| CopyTask {
                        source: object.path.to_path(from),
                        target: object.path.to_path(instance_dir.minecraft_dir()),
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
                    "Replacing download URLs in metadata for {}",
                    &vanilla_metadata.id
                );
                *vanilla_metadata = vanilla_metadata
                    .with_replaced_download_urls(&include_config.download_server_base, output_dir)
                    .await?;
            }
        }

        let mut resources_url_base = ResourcesUrlBase::default();
        if let Some(include_config) = &self.include_config
            && include_config.replace_download_urls {
                resources_url_base = AssetsDir::root()
                    .assets_object_dir()
                    .to_resources_url_base(&include_config.download_server_base);
            }

        let authlib_injector = default_authlib_injector_library();
        check_tasks.extend(authlib_injector.get_check_tasks(output_dir, os_arch)?);

        Ok(GeneratorResult {
            metadata: InstanceMetadata {
                name: self.instance_name,
                auth_backend: self.auth_backend,
                include,
                resources_url_base,
                extra_forge_libs,
                authlib_injector,
                default_xmx: self.default_xmx,
                versions: metadata,
                overrides_applied: true,
            },
            check_tasks,
            copy_tasks,
            other_generated_files,
        })
    }
}
