use instance::{
    authlib::default_authlib_injector_library,
    instance_metadata::{
        IncludeAction, IncludeEntry, InstanceMetadata, ModEntry, Object, ResourceSyncMode,
    },
    manifest::VanillaVersionManifest,
    mod_sync::ModSyncSettings,
    overrides::with_overrides,
    version_metadata::{Library, OsArch, VersionMetadata},
};
use launcher_auth::providers::AuthProviderConfig;
use log::{debug, info, warn};
use relative_path::{RelativePath, RelativePathBuf};
use reqwest::Client;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use url::Url;
use utils::{
    files::{self, CheckTask, CopyTask},
    paths::{
        AssetsDir, BaseUrl, DataDir, InstanceDir, InstanceDirFS, LibrariesDir, ModsDir,
        ResourcesUrlBase,
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

#[derive(PartialEq, Eq, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    Fabric,
    Forge,
    Neoforge,
}

async fn get_file_object(
    base_path: &Path,
    path: &RelativePath,
    base_url: &BaseUrl,
    instance_dir: &InstanceDir,
) -> Result<Object, GetObjectsError> {
    let full_path = path.to_path(base_path);
    if !full_path.exists() {
        return Err(GetObjectsError::IncludeFileNotFound(path.to_string()));
    }
    if full_path.is_dir() {
        return Err(GetObjectsError::IncludeFileIsDirectory(path.to_string()));
    }
    // TODO: other non-file cases?

    let metadata = full_path.metadata()?;
    let hash = files::hash_file(&full_path).await?;
    let object_url = instance_dir
        .minecraft_dir()
        .instance_object_path(path)
        .to_url(base_url);
    Ok(Object {
        path: path.into(),
        sha1: hash,
        size: metadata.len(),
        url: object_url,
    })
}

async fn get_directory_objects(
    base_path: &Path,
    path: &RelativePathBuf,
    base_url: &BaseUrl,
    instance_dir: &InstanceDir,
    ignore_paths: &HashSet<PathBuf>,
) -> Result<Vec<Object>, GetObjectsError> {
    let full_path = path.to_path(base_path);
    let files = files::get_files_ignore_paths(&full_path, ignore_paths);
    let hashes = files::hash_files(&files, progress::no_progress_bar()).await?;

    let mut objects = Vec::with_capacity(files.len());
    for (path, hash) in files.iter().zip(hashes.iter()) {
        let object_path = RelativePathBuf::from_path(path.strip_prefix(base_path)?)?;
        let object_url = instance_dir
            .minecraft_dir()
            .instance_object_path(&object_path)
            .to_url(base_url);
        objects.push(Object {
            path: object_path.clone(),
            sha1: hash.clone(),
            size: path.metadata()?.len(),
            url: object_url,
        });
    }

    Ok(objects)
}

fn validate_mod_sync_overrides(
    mod_entries: &[ModEntry],
    mod_sync: &ModSyncSettings,
) -> Result<(), GenerateError> {
    let mod_ids = mod_entries
        .iter()
        .map(|entry| entry.mod_id.as_str())
        .collect::<HashSet<_>>();
    let mut policy_ids = HashSet::new();

    for mod_id in &mod_sync.blocked {
        if mod_ids.contains(mod_id.as_str()) {
            return Err(GenerateError::BlockedModInPack(mod_id.clone()));
        }
        if !policy_ids.insert(mod_id.as_str()) {
            return Err(GenerateError::ConflictingModSyncPolicy(mod_id.clone()));
        }
    }

    for mod_id in &mod_sync.required {
        if !mod_ids.contains(mod_id.as_str()) {
            return Err(GenerateError::OverrideModNotInPack(mod_id.clone()));
        }
        if !policy_ids.insert(mod_id.as_str()) {
            return Err(GenerateError::ConflictingModSyncPolicy(mod_id.clone()));
        }
    }

    let mut set_ids = HashSet::new();
    for set in &mod_sync.optional_sets {
        if !set_ids.insert(set.id.as_str()) {
            return Err(GenerateError::DuplicateOptionalSet(set.id.clone()));
        }
        for mod_id in &set.mod_ids {
            if !mod_ids.contains(mod_id.as_str()) {
                return Err(GenerateError::OverrideModNotInPack(mod_id.clone()));
            }
            if !policy_ids.insert(mod_id.as_str()) {
                return Err(GenerateError::ConflictingModSyncPolicy(mod_id.clone()));
            }
        }
    }
    Ok(())
}

async fn collect_mod_entries(
    source_dir: &Path,
    base_url: &BaseUrl,
    instance_dir: &InstanceDir,
    mod_sync: &ModSyncSettings,
) -> Result<Vec<ModEntry>, GenerateError> {
    let mods_dir_rel = RelativePathBuf::from(ModsDir::name());
    let mods_dir = mods_dir_rel.to_path(source_dir);
    if !mods_dir.is_dir() {
        return Ok(Vec::new());
    }

    let blocked = mod_sync
        .blocked
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&mods_dir).map_err(GetObjectsError::Io)? {
        let path = entry.map_err(GetObjectsError::Io)?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jar") {
            continue;
        }
        let mod_id = match utils::mod_id::extract_mod_id(&path) {
            Ok(Some(mod_id)) => mod_id,
            Ok(None) => {
                warn!("Skipping mod jar without mod id: {}", path.display());
                continue;
            }
            Err(err) => {
                return Err(GenerateError::ModIdExtract(
                    path.display().to_string(),
                    err.to_string(),
                ));
            }
        };
        if blocked.contains(mod_id.as_str()) {
            return Err(GenerateError::BlockedModInPack(mod_id));
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                GenerateError::ModIdExtract(
                    path.display().to_string(),
                    "missing file name".to_string(),
                )
            })?;
        let rel_path = RelativePathBuf::from(format!("{}/{file_name}", ModsDir::name()));
        let object = get_file_object(source_dir, &rel_path, base_url, instance_dir).await?;
        entries.push(ModEntry { mod_id, object });
    }

    entries.sort_by(|left, right| left.mod_id.cmp(&right.mod_id));
    Ok(entries)
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
    #[error("include file not found: {0}")]
    IncludeFileNotFound(String),
    #[error("file include points to a directory: {0}")]
    IncludeFileIsDirectory(String),
    #[error("directory include points to a file: {0}")]
    IncludeDirectoryIsFile(String),
}

#[derive(thiserror::Error, Debug)]
pub enum GetExtraForgeLibsError {
    #[error("failed to parse extra forge library paths: {0}")]
    ExtraForgeLibs(#[from] ExtraForgeLibsError),
    #[error("failed to read extra forge library metadata: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to hash extra forge libraries: {0}")]
    HashFiles(#[from] files::HashFilesError),
}

struct ParsedExtraLib {
    source_path: PathBuf,
    rel_path: String,
    gav: String,
}

fn parse_extra_forge_lib(
    path: &Path,
    libraries_dir: &Path,
) -> Result<Option<ParsedExtraLib>, ExtraForgeLibsError> {
    if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jar") {
        return Ok(None);
    }

    let rel_path = path
        .strip_prefix(libraries_dir)
        .map_err(|_| ExtraForgeLibsError::OutsideLibrariesDir(path.to_path_buf()))?;
    let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");

    let parts = RelativePath::new(&rel_path_str)
        .components()
        .map(|x| x.as_str())
        .collect::<Vec<_>>();
    if parts.len() < 3 {
        return Err(ExtraForgeLibsError::InvalidLayout(path.to_path_buf()));
    }
    let version = parts[parts.len() - 2].to_string();
    let artifact = parts[parts.len() - 3].to_string();
    let group = parts[..parts.len() - 3].join(".");
    if group.is_empty() {
        return Err(ExtraForgeLibsError::InvalidLayout(path.to_path_buf()));
    }

    let file_stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ExtraForgeLibsError::MissingFileName(path.to_path_buf()))?
        .to_string();
    let stem_prefix = format!("{artifact}-{version}");
    let suffix = file_stem
        .strip_prefix(&stem_prefix)
        .ok_or_else(|| ExtraForgeLibsError::BadLibraryName(file_stem.clone()))?
        .replace("-", ":");

    Ok(Some(ParsedExtraLib {
        source_path: path.to_path_buf(),
        rel_path: rel_path_str,
        gav: format!("{group}:{artifact}:{version}{suffix}"),
    }))
}

fn should_include_extra_forge_lib(gav: &str, version_library_names: &HashSet<String>) -> bool {
    if version_library_names.contains(gav) {
        return false;
    }
    if gav.starts_with("net.neoforged.installertools:installertools:")
        || gav.starts_with("net.minecraftforge:installertools:")
    {
        return false;
    }
    true
}

async fn get_extra_forge_libs(
    extra_forge_libs_paths: &[PathBuf],
    data_dir: &DataDir,
    download_server_base: &BaseUrl,
    version_library_names: &HashSet<String>,
) -> Result<Vec<Library>, GetExtraForgeLibsError> {
    let libraries_dir = LibrariesDir::root().to_fs(data_dir);
    let mut parsed_libs = Vec::with_capacity(extra_forge_libs_paths.len());

    for path in extra_forge_libs_paths {
        let Some(parsed) = parse_extra_forge_lib(path, &libraries_dir)? else {
            debug!("Skipping non-jar file: {}", path.display());
            continue;
        };
        if !should_include_extra_forge_lib(&parsed.gav, version_library_names) {
            debug!(
                "Skipping extra forge lib already covered elsewhere: {}",
                parsed.gav
            );
            continue;
        }
        parsed_libs.push(parsed);
    }

    if parsed_libs.is_empty() {
        return Ok(vec![]);
    }

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
            let size = lib.source_path.metadata()?.len();
            Ok(Library::from_download(lib.gav, url, sha1, size))
        })
        .collect::<Result<Vec<_>, GetExtraForgeLibsError>>()?)
}

fn vanilla() -> Loader {
    Loader::Vanilla
}

#[derive(Deserialize, Clone)]
pub struct InstanceSpec {
    pub name: String,
    pub minecraft_version: String,
    #[serde(default = "vanilla")]
    pub loader: Loader,
    /// latest/recommended will be used if not set
    pub loader_version: Option<String>,

    pub source_dir: Option<PathBuf>,
    #[serde(default)]
    pub include_rules: Vec<IncludeEntry>,
    #[serde(default)]
    pub mod_sync: ModSyncSettings,
    #[serde(default)]
    pub resource_sync: ResourceSyncMode,

    pub auth_backend: Option<AuthProviderConfig>,
    pub default_xmx: Option<String>,
}

pub struct RemoteConfig {
    pub download_server_base: BaseUrl,
    /// Whether to download all files and replace
    /// artifact URLs with the download server base
    pub replace_download_urls: bool,
}

pub struct InstanceGenerator {
    pub client: Client,
    /// `object`/`objects` fields must be unset for include_rules
    pub spec: InstanceSpec,
    /// If absent, includes won't be processed.
    /// Always present in instance-builder, never present on local instance generation
    pub remote_config: Option<RemoteConfig>,
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
    #[error("objects must be unset in include rules")]
    IncludeObjectsSet,
    #[error("blocked mod id appears in mod pack: {0}")]
    BlockedModInPack(String),
    #[error("optional/required override not present in mod pack: {0}")]
    OverrideModNotInPack(String),
    #[error("mod id appears in more than one mod sync policy: {0}")]
    ConflictingModSyncPolicy(String),
    #[error("duplicate optional mod set id: {0}")]
    DuplicateOptionalSet(String),
    #[error("failed to extract mod id from {0}: {1}")]
    ModIdExtract(String, String),
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
            .get_entry(&self.spec.minecraft_version)
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

        match &self.spec.loader {
            Loader::Vanilla => {
                if self.spec.loader_version.is_some() {
                    warn!("Ignoring loader version for vanilla version");
                }
            }
            Loader::Forge | Loader::Neoforge => {
                let result = ForgeGenerator::new(
                    vanilla_metadata,
                    if self.spec.loader == Loader::Forge {
                        forge::Loader::Forge
                    } else {
                        forge::Loader::Neoforge
                    },
                    self.spec.loader_version.clone(),
                )
                .generate(&self.client, output_dir, work_dir)
                .await?;
                metadata.push(result.metadata);

                let extra_forge_lib_paths = result
                    .extra_libs_copy_tasks
                    .iter()
                    .map(|task| task.source.clone())
                    .collect::<Vec<_>>();
                copy_tasks.extend(result.extra_libs_copy_tasks);

                // TODO: is it okay to silently skip this?
                if let Some(include_config) = self.remote_config.as_ref() {
                    let version_library_names = metadata
                        .iter()
                        .flat_map(|version| version.libraries.iter())
                        .map(|library| library.get_full_name())
                        .collect();
                    extra_forge_libs = get_extra_forge_libs(
                        &extra_forge_lib_paths,
                        &DataDir::new(result.installer_work_dir),
                        &include_config.download_server_base,
                        &version_library_names,
                    )
                    .await?;
                }
            }
            Loader::Fabric => {
                let result = FabricGenerator::new(
                    &self.spec.minecraft_version,
                    self.spec.loader_version.clone(),
                )
                .generate(&self.client)
                .await?;
                metadata.push(result);
            }
        };

        for metadata in metadata.iter_mut() {
            metadata.libraries = with_overrides(&metadata.libraries, &metadata.id);
        }

        let mut include = vec![];
        if let Some(remote_config) = &self.remote_config {
            if remote_config.replace_download_urls {
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

            if let Some(source_dir) = &self.spec.source_dir {
                // Handled separately, because we may want to put default contents for a config file,
                // but overwrite some config keys. This is not done in builder, where seen_paths is only
                // used to determine which files have to be deleted
                let mut existing_paths =
                    HashMap::from([(false, HashSet::new()), (true, HashSet::new())]);
                let mut sorted_rules = self.spec.include_rules.to_vec();
                sorted_rules.sort_by(|a, b| a.path.cmp(&b.path).reverse());
                for mut rule in sorted_rules {
                    if rule.path.components().count() == 1 && rule.path.as_str() == ModsDir::name()
                    {
                        warn!(
                            "Skipping mods directory include rule, mods are now managed separately"
                        );
                        continue;
                    }
                    let overwrite = rule.overwrite;
                    let rule_path = rule.path.clone();
                    match &mut rule.action {
                        IncludeAction::File(action) => {
                            if action.object.is_some() {
                                return Err(GenerateError::IncludeObjectsSet);
                            }
                            let object = get_file_object(
                                source_dir,
                                &rule.path,
                                &remote_config.download_server_base,
                                instance_dir.rel(),
                            )
                            .await?;
                            copy_tasks.push(CopyTask {
                                source: object.path.to_path(source_dir),
                                target: object.path.to_path(instance_dir.minecraft_dir()),
                            });
                            action.object = Some(object);
                        }
                        IncludeAction::Directory(action) => {
                            if !action.objects.is_empty() {
                                return Err(GenerateError::IncludeObjectsSet);
                            }
                            let objects = get_directory_objects(
                                source_dir,
                                &rule.path,
                                &remote_config.download_server_base,
                                instance_dir.rel(),
                                &existing_paths[&rule.overwrite],
                            )
                            .await?;
                            if objects.is_empty() {
                                warn!("No objects found for rule: {}", rule.path);
                            }
                            copy_tasks.extend(objects.iter().map(|object| CopyTask {
                                source: object.path.to_path(source_dir),
                                target: object.path.to_path(instance_dir.minecraft_dir()),
                            }));
                            existing_paths
                                .get_mut(&overwrite)
                                .expect("overwrite key initialized")
                                .extend(
                                    objects.iter().map(|object| object.path.to_path(source_dir)),
                                );
                            action.objects = objects;
                        }
                        IncludeAction::ConfigOptions(..) => {}
                    }
                    existing_paths
                        .get_mut(&overwrite)
                        .expect("overwrite key initialized")
                        .insert(rule_path.to_path(source_dir));
                    include.push(rule);
                }
            } else {
                warn!("Ignoring include rules, source_dir is not set");
            }

            if remote_config.replace_download_urls {
                let vanilla_metadata = metadata.first_mut().expect("Vanilla metadata present");
                info!(
                    "Replacing download URLs in metadata for {}",
                    &vanilla_metadata.id
                );
                *vanilla_metadata = vanilla_metadata
                    .with_replaced_download_urls(&remote_config.download_server_base, output_dir)
                    .await?;
            }
        }

        let mut resources_url_base = ResourcesUrlBase::default();
        if let Some(include_config) = &self.remote_config
            && include_config.replace_download_urls
        {
            resources_url_base = AssetsDir::root()
                .assets_object_dir()
                .to_resources_url_base(&include_config.download_server_base);
        }

        let mod_entries = if let (Some(source_dir), Some(remote_config)) =
            (&self.spec.source_dir, &self.remote_config)
        {
            collect_mod_entries(
                source_dir,
                &remote_config.download_server_base,
                instance_dir.rel(),
                &self.spec.mod_sync,
            )
            .await?
        } else {
            Vec::new()
        };
        validate_mod_sync_overrides(&mod_entries, &self.spec.mod_sync)?;

        let authlib_injector = default_authlib_injector_library();
        check_tasks.extend(authlib_injector.get_check_tasks(output_dir, os_arch)?);

        Ok(GeneratorResult {
            metadata: InstanceMetadata {
                name: self.spec.name,
                auth_backend: self.spec.auth_backend,
                include,
                mod_entries,
                mod_sync: self.spec.mod_sync.clone(),
                resource_sync: self.spec.resource_sync,
                resources_url_base,
                extra_forge_libs,
                authlib_injector,
                default_xmx: self.spec.default_xmx,
                versions: metadata,
                overrides_applied: true,
            },
            check_tasks,
            copy_tasks,
            other_generated_files,
        })
    }
}
