use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    files,
    paths::{get_libraries_dir, get_rel_minecraft_dir, get_versions_extra_dir},
    progress::{self, NoProgressBar, ProgressBar as _},
    utils::{url_from_path, url_from_rel_path},
    version::{
        extra_version_metadata::{ExtraVersionMetadata, Include, Object},
        version_metadata::Library,
    },
};
use launcher_auth::providers::AuthProviderConfig;
use log::info;
use serde::Deserialize;

async fn get_objects(
    copy_from: &Path,
    from: &Path,
    download_server_base: &str,
    version_name: &str,
    existing_paths: &HashSet<PathBuf>,
) -> anyhow::Result<Vec<Object>> {
    let files = files::get_files_ignore_paths(from, existing_paths)?;

    let rel_paths = files
        .iter()
        .map(|p| p.strip_prefix(copy_from))
        .collect::<Result<Vec<_>, _>>()?;
    let hashes = files::hash_files(files.clone(), progress::no_progress_bar()).await?;

    let mut objects = vec![];
    for (rel_path, hash) in rel_paths.iter().zip(hashes.iter()) {
        let url = url_from_rel_path(
            &get_rel_minecraft_dir(version_name).join(rel_path),
            download_server_base,
        )?;
        objects.push(Object {
            path: rel_path.to_string_lossy().to_string().replace('\\', "/"),
            sha1: hash.clone(),
            url,
        });
    }

    Ok(objects)
}

#[derive(thiserror::Error, Debug)]
pub enum ExtraForgeLibsError {
    #[error("Bad library name: {0}")]
    BadLibraryName(String),
}

async fn get_extra_forge_libs(
    extra_forge_libs_paths: &[PathBuf],
    data_dir: &Path,
    download_server_base: &str,
) -> anyhow::Result<Vec<Library>> {
    let libraries_dir = get_libraries_dir(data_dir);

    let progress_bar = Arc::new(NoProgressBar);
    progress_bar.set_message("Hashing extra forge libraries");
    let hashes = files::hash_files::<&str>(extra_forge_libs_paths.to_vec(), progress_bar).await?;

    let libraries = extra_forge_libs_paths
        .iter()
        .zip(hashes.iter())
        .filter(|(path, _)| path.is_file() && path.extension().is_some_and(|ext| ext == "jar"))
        .map(|(path, hash)| {
            let url = url_from_path(path, data_dir, download_server_base)?;

            let parts = path
                .strip_prefix(&libraries_dir)?
                .components()
                .map(|x| x.as_os_str().to_string_lossy().replace('\\', "/"))
                .collect::<Vec<_>>();
            let version = parts[parts.len() - 2].to_string();
            let name = parts[parts.len() - 3].to_string();
            let group = parts
                .iter()
                .take(parts.len() - 3)
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(".");

            let filename = path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .strip_suffix(".jar")
                .unwrap()
                .to_string();
            let filename_without_suffix = format!("{name}-{version}");
            let suffix = filename
                .strip_prefix(&filename_without_suffix)
                .ok_or(ExtraForgeLibsError::BadLibraryName(filename.clone()))?;
            let suffix = suffix.replace("-", ":");

            let name = format!("{group}:{name}:{version}{suffix}");

            Ok(Library::from_download(name, url, hash.clone()))
        })
        .collect::<anyhow::Result<_>>()?;

    Ok(libraries)
}

pub struct GeneratorResult {
    // relative include path -> absolute source path
    pub include_mapping: HashMap<String, PathBuf>,

    pub extra_metadata: ExtraVersionMetadata,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
pub struct IncludeRule {
    pub path: String,

    #[serde(default = "yes")]
    pub overwrite: bool,

    #[serde(default = "yes")]
    pub delete_extra: bool,

    #[serde(default)]
    pub recursive: bool,
}

pub struct IncludeConfig {
    pub include: Vec<IncludeRule>,
    pub include_from: String,
    pub download_server_base: String,
    pub resources_url_base: Option<String>,
}

pub struct ExtraMetadataGenerator {
    version_name: String,
    include_config: Option<IncludeConfig>,
    extra_forge_libs_paths: Vec<PathBuf>,
    auth_backend: Option<AuthProviderConfig>,
    recommended_xmx: Option<String>,
}

impl ExtraMetadataGenerator {
    pub fn new(
        version_name: String,
        include_config: Option<IncludeConfig>,
        extra_forge_libs_paths: Vec<PathBuf>,
        auth_backend: Option<AuthProviderConfig>,
        recommended_xmx: Option<String>,
    ) -> Self {
        Self {
            version_name,
            include_config,
            extra_forge_libs_paths,
            auth_backend,
            recommended_xmx,
        }
    }

    pub async fn generate(self, work_dir: &Path) -> anyhow::Result<GeneratorResult> {
        info!(
            "Generating extra metadata for instance {}",
            self.version_name
        );

        let mut extra_metadata = ExtraVersionMetadata {
            include: vec![],
            resources_url_base: None,
            auth_backend: self.auth_backend,
            extra_forge_libs: vec![],
            recommended_xmx: self.recommended_xmx,
        };

        let mut include_mapping = HashMap::new();

        if let Some(include_config) = self.include_config {
            let extra_forge_libs = get_extra_forge_libs(
                &self.extra_forge_libs_paths,
                work_dir,
                &include_config.download_server_base,
            )
            .await?;

            let copy_from = PathBuf::from(&include_config.include_from);

            let mut include = vec![];
            let mut existing_paths = HashSet::new();
            for rule in include_config.include.iter() {
                let from = copy_from.join(Path::new(&rule.path));

                let objects = get_objects(
                    &copy_from,
                    &from,
                    &include_config.download_server_base,
                    &self.version_name,
                    &existing_paths,
                )
                .await?;
                include_mapping.insert(rule.path.clone(), from.clone());

                include.push(Include {
                    path: rule.path.clone(),
                    overwrite: rule.overwrite,
                    delete_extra: rule.delete_extra,
                    recursive: rule.recursive,
                    objects,
                });
                existing_paths.insert(from);
            }

            extra_metadata.include = include;
            extra_metadata.resources_url_base = include_config.resources_url_base;
            extra_metadata.extra_forge_libs = extra_forge_libs;
        }

        let versions_extra_dir = get_versions_extra_dir(work_dir);
        extra_metadata
            .save(&self.version_name, &versions_extra_dir)
            .await?;

        info!(
            "Extra metadata for instance {} generated",
            self.version_name
        );

        Ok(GeneratorResult {
            include_mapping,
            extra_metadata,
        })
    }
}
