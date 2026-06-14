use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    io::Write as _,
    path::{Path, PathBuf},
};

use instance::version_metadata::VersionMetadata;
use log::{debug, error, info, warn};
use reqwest::Client;
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;
use utils::{
    files::{self, CopyTask},
    java::{download_java, get_java},
    paths::{DataDir, LibrariesDir, VersionsDir},
    progress,
};

const FORGE_MAVEN_METADATA_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json";

const FORGE_PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";

const NEOFORGE_MAVEN_METADATA_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum VersionFragment {
    Alpha,
    Beta,
    Snapshot,
    String(String),
    Number(usize),
}

impl VersionFragment {
    fn string_to_parts(version: &str) -> Vec<Self> {
        version
            .split(&['.', '-', '+'])
            .map(|part| {
                if let Ok(number) = part.parse::<usize>() {
                    VersionFragment::Number(number)
                } else if part.eq_ignore_ascii_case("alpha") {
                    VersionFragment::Alpha
                } else if part.eq_ignore_ascii_case("beta") {
                    VersionFragment::Beta
                } else if part.eq_ignore_ascii_case("snapshot") {
                    VersionFragment::Snapshot
                } else {
                    VersionFragment::String(part.to_string())
                }
            })
            .collect()
    }
}

fn neoforge_minecraft_version_prefix(minecraft_version: &str) -> Vec<VersionFragment> {
    let mut parts = VersionFragment::string_to_parts(minecraft_version);
    if parts.is_empty() {
        return parts;
    }

    // 25w14craftmine -> 0.25w14craftmine
    if parts[0] == VersionFragment::String("25w14craftmine".into()) {
        parts.insert(0, VersionFragment::Number(0));
    } else {
        // 26.1 -> 26.1.0, 1.21 -> 1.21.0
        if parts.len() < 3 {
            parts.push(VersionFragment::Number(0));
        }
        // 1.21.4 -> 21.4 (legacy NeoForge versioning)
        if parts[0] == VersionFragment::Number(1) {
            parts.remove(0);
        }
    }

    parts
}

fn version_fragments_start_with(version: &str, prefix: &[VersionFragment]) -> bool {
    VersionFragment::string_to_parts(version).starts_with(prefix)
}

#[derive(Debug, Deserialize)]
pub struct ForgeMavenMetadata {
    versions: HashMap<String, Vec<String>>,
}

#[derive(thiserror::Error, Debug)]
pub enum ForgeMavenMetadataError {
    #[error("network request failed while fetching Forge maven metadata: {0}")]
    Reqwest(#[from] reqwest::Error),
}

impl ForgeMavenMetadata {
    pub async fn fetch(client: &Client) -> Result<Self, ForgeMavenMetadataError> {
        let response = client
            .get(FORGE_MAVEN_METADATA_URL)
            .send()
            .await?
            .error_for_status()?;
        Ok(ForgeMavenMetadata {
            versions: response.json().await?,
        })
    }

    pub fn get_matching_versions(&self, minecraft_version: &str) -> Vec<String> {
        self.versions
            .get(minecraft_version)
            .cloned()
            .unwrap_or(vec![])
            .into_iter()
            .rev()
            .filter_map(|version| {
                version
                    .strip_prefix(&format!("{minecraft_version}-"))
                    .map(|forge_version| forge_version.to_string())
            })
            .collect()
    }

    fn has_version(&self, minecraft_version: &str, forge_version: &str) -> bool {
        self.versions
            .get(minecraft_version)
            .is_some_and(|versions| {
                versions.contains(&format!("{minecraft_version}-{forge_version}"))
            })
    }
}

#[derive(Debug, Deserialize)]
struct Versions {
    version: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Versioning {
    versions: Versions,
}

#[derive(Debug, Deserialize)]
pub struct NeoforgeMavenMetadata {
    versioning: Versioning,
}

#[derive(thiserror::Error, Debug)]
pub enum NeoforgeMavenMetadataError {
    #[error("network request failed while fetching Neoforge maven metadata: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to parse Neoforge maven metadata XML: {0}")]
    Xml(#[from] serde_xml_rs::Error),
}

impl NeoforgeMavenMetadata {
    pub async fn fetch(client: &Client) -> Result<Self, NeoforgeMavenMetadataError> {
        let response = client
            .get(NEOFORGE_MAVEN_METADATA_URL)
            .send()
            .await?
            .error_for_status()?;
        let metadata: NeoforgeMavenMetadata = serde_xml_rs::from_str(&response.text().await?)?;
        Ok(metadata)
    }

    pub fn get_matching_versions(&self, minecraft_version: &str) -> Vec<String> {
        let prefix = neoforge_minecraft_version_prefix(minecraft_version);
        if prefix.is_empty() {
            return vec![];
        }

        let mut matched: Vec<String> = self
            .versioning
            .versions
            .version
            .iter()
            .filter(|version| version_fragments_start_with(version, &prefix))
            .cloned()
            .collect();

        matched.sort_by_cached_key(|version| VersionFragment::string_to_parts(version));
        matched.reverse();
        matched
    }

    pub fn get_latest_matching_version(&self, minecraft_version: &str) -> Option<String> {
        self.get_matching_versions(minecraft_version)
            .into_iter()
            .next()
    }

    pub fn has_version(&self, version: &str) -> bool {
        self.versioning
            .versions
            .version
            .contains(&version.to_string())
    }
}

#[derive(Deserialize)]
pub struct ForgePromotions {
    promos: HashMap<String, String>,
}

#[derive(thiserror::Error, Debug)]
pub enum ForgePromotionsError {
    #[error("network request failed while fetching Forge promotions metadata: {0}")]
    Reqwest(#[from] reqwest::Error),
}

impl ForgePromotions {
    pub async fn fetch(client: &Client) -> Result<Self, ForgePromotionsError> {
        let response = client
            .get(FORGE_PROMOTIONS_URL)
            .send()
            .await?
            .error_for_status()?;
        let promotions: ForgePromotions = response.json().await?;
        Ok(promotions)
    }

    pub fn get_latest_version(
        &self,
        minecraft_version: &str,
        version_type: &str,
    ) -> Option<String> {
        self.promos
            .get(&format!("{minecraft_version}-{version_type}"))
            .cloned()
    }
}

const FORGE_INSTALLER_BASE_URL: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge/";

const NEOFORGE_INSTALLER_BASE_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/";

async fn download_forge_installer(
    full_version: &str,
    work_dir: &Path,
    loader: &Loader,
) -> Result<PathBuf, DownloadForgeInstallerError> {
    let filename = format!("{loader:?}-{full_version}-installer.jar");
    let forge_installer_url = match loader {
        Loader::Forge => format!("{FORGE_INSTALLER_BASE_URL}{full_version}/{filename}"),
        Loader::Neoforge => format!("{NEOFORGE_INSTALLER_BASE_URL}{full_version}/{filename}"),
    };
    let forge_installer_path = work_dir.join(filename);

    let client = Client::new();
    let response = client
        .get(&forge_installer_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let mut file = tokio::fs::File::create(&forge_installer_path).await?;
    file.write_all(&response).await?;

    Ok(forge_installer_path)
}

#[derive(Deserialize)]
struct ProfileInfo {
    #[serde(rename = "lastVersionId")]
    last_version_id: String,
}

#[derive(Deserialize)]
pub struct LauncherProfiles {
    profiles: HashMap<String, ProfileInfo>,
}

pub enum Loader {
    Forge,
    Neoforge,
}

impl Display for Loader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Loader::Forge => write!(f, "Forge"),
            Loader::Neoforge => write!(f, "Neoforge"),
        }
    }
}

impl Debug for Loader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Loader::Forge => write!(f, "forge"),
            Loader::Neoforge => write!(f, "neoforge"),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ForgeError {
    #[error("Forge version {0} not found for minecraft {1}")]
    ForgeVersionNotFound(String, String),
    #[error("no forge profiles found")]
    NoForgeProfiles,
}

#[derive(thiserror::Error, Debug)]
pub enum GetForgeVersionError {
    #[error("failed to fetch Forge promotions metadata: {0}")]
    ForgePromotions(#[from] ForgePromotionsError),
    #[error("failed to fetch Forge maven metadata: {0}")]
    ForgeMavenMetadata(#[from] ForgeMavenMetadataError),
    #[error("failed to fetch Neoforge maven metadata: {0}")]
    NeoforgeMavenMetadata(#[from] NeoforgeMavenMetadataError),
    #[error("failed to resolve loader version: {0}")]
    Forge(#[from] ForgeError),
}

pub async fn get_forge_version(
    client: &Client,
    minecraft_version: &str,
    loader_version: &Option<String>,
    loader: &Loader,
) -> Result<String, GetForgeVersionError> {
    match loader {
        Loader::Forge => {
            let forge_promotions = ForgePromotions::fetch(client).await?;

            let forge_version = match loader_version {
                Some(version) => version.to_string(),
                None => {
                    const FORGE_DEFAULT: &str = "recommended";
                    info!("Version not set, using \"{FORGE_DEFAULT}\"");
                    forge_promotions
                        .get_latest_version(minecraft_version, FORGE_DEFAULT)
                        .ok_or(ForgeError::ForgeVersionNotFound(
                            FORGE_DEFAULT.to_string(),
                            minecraft_version.to_string(),
                        ))?
                }
            };

            let forge_maven_metadata = ForgeMavenMetadata::fetch(client).await?;
            if forge_maven_metadata.has_version(minecraft_version, &forge_version) {
                return Ok(forge_version);
            }
            let version_with_suffix = format!("{forge_version}-{minecraft_version}");
            if forge_maven_metadata.has_version(minecraft_version, &version_with_suffix) {
                return Ok(version_with_suffix);
            }
        }
        Loader::Neoforge => {
            let neoforge_maven_metadata = NeoforgeMavenMetadata::fetch(client).await?;

            let neoforge_version = match loader_version {
                Some(version) => version.to_string(),
                None => {
                    info!("Version not set, using latest");
                    neoforge_maven_metadata
                        .get_latest_matching_version(minecraft_version)
                        .ok_or(ForgeError::ForgeVersionNotFound(
                            "neoforge:latest".to_string(),
                            minecraft_version.to_string(),
                        ))?
                }
            };

            if neoforge_maven_metadata.has_version(&neoforge_version) {
                return Ok(neoforge_version);
            }
        }
    };

    let forge_version = loader_version.as_deref().unwrap_or("default");
    error!("{loader} version {forge_version} not found for minecraft {minecraft_version}");
    Err(
        ForgeError::ForgeVersionNotFound(forge_version.to_string(), minecraft_version.to_string())
            .into(),
    )
}

// trick forge installer into thinking that the folder is actually a minecraft instance folder
pub fn trick_forge(forge_work_dir: &Path, minecraft_version: &str) -> Result<(), TrickForgeError> {
    let data_dir = DataDir::new(forge_work_dir.to_path_buf());
    let versions_dir = VersionsDir::root().to_fs(&data_dir);
    std::fs::create_dir_all(versions_dir.join(minecraft_version))?;
    let mut file = std::fs::File::create(forge_work_dir.join("launcher_profiles.json"))?;
    let _ = file.write(b"{\"profiles\":{}}")?;
    Ok(())
}

pub fn get_full_version(minecraft_version: &str, forge_version: &str) -> String {
    format!("{minecraft_version}-{forge_version}")
}

// workaround for windows weirdness
fn to_abs_path_str(path: &Path) -> Result<String, ToAbsPathError> {
    let canonical = path.canonicalize()?;
    let path_str = canonical.to_string_lossy();

    #[cfg(windows)]
    {
        const VERBATIM_PREFIX: &str = r"\\?\";
        if let Some(stripped) = path_str.strip_prefix(VERBATIM_PREFIX) {
            Ok(stripped.to_string())
        } else {
            Ok(path_str.to_string())
        }
    }

    #[cfg(not(windows))]
    {
        Ok(path_str.to_string())
    }
}

fn log_forge_installer_output(label: &str, stdout: &[u8], stderr: &[u8]) {
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);
    if !stdout_str.trim().is_empty() {
        error!("{label} stdout:\n{stdout_str}");
    }
    if !stderr_str.trim().is_empty() {
        error!("{label} stderr:\n{stderr_str}");
    }
}

async fn run_forge_command(
    java_path: &Path,
    forge_installer_path: &Path,
    forge_work_dir: &Path,
) -> Result<(), RunForgeCommandError> {
    let mut cmd = tokio::process::Command::new(&to_abs_path_str(java_path)?);
    cmd.current_dir(&to_abs_path_str(forge_work_dir)?)
        .arg("-jar")
        .arg(&to_abs_path_str(forge_installer_path)?)
        .arg("--installClient")
        .arg(".");
    info!("Running forge installer: {cmd:?}");

    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if stderr_str.contains("'installClient' is not a recognized option") {
            info!("Retrying without '--installClient' argument.");
            let mut retry_cmd = tokio::process::Command::new(&to_abs_path_str(java_path)?);
            retry_cmd
                .current_dir(&to_abs_path_str(forge_work_dir)?)
                .arg("-jar")
                .arg(&to_abs_path_str(forge_installer_path)?);
            let retry_output = retry_cmd.output().await?;
            if !retry_output.status.success() {
                log_forge_installer_output(
                    "Forge installer retry",
                    &retry_output.stdout,
                    &retry_output.stderr,
                );
                return Err(RunForgeCommandError::RetryFailed(
                    String::from_utf8_lossy(&retry_output.stderr).to_string(),
                ));
            }
        } else {
            log_forge_installer_output("Forge installer", &output.stdout, &output.stderr);
            return Err(RunForgeCommandError::CommandFailed(stderr_str.to_string()));
        }
    }

    Ok(())
}

pub async fn install_forge(
    forge_work_dir: &Path,
    launcher_data_dir: &DataDir,
    forge_version: &str,
    vanilla_metadata: &VersionMetadata,
    loader: &Loader,
) -> Result<String, InstallForgeError> {
    std::fs::create_dir_all(forge_work_dir)?;

    let minecraft_version = &vanilla_metadata.id;

    let lock_file = forge_work_dir.join("forge.lock");

    if !lock_file.exists() {
        let java_version = vanilla_metadata
            .java_version
            .as_ref()
            .map(|v| v.major_version.to_string())
            .unwrap_or_else(|| {
                warn!("Java version not found, using default (8)");
                "8".to_string()
            });

        info!("Getting java {}", &java_version);
        let java_installation;
        if let Some(existing_java_installation) = get_java(&java_version, launcher_data_dir).await {
            java_installation = existing_java_installation;
        } else {
            info!("Java installation not found, downloading");

            java_installation = download_java(
                &java_version,
                launcher_data_dir,
                progress::no_progress_bar(),
            )
            .await?;
        }

        info!("Downloading forge installer");
        let full_version = match loader {
            Loader::Forge => get_full_version(minecraft_version, forge_version),
            Loader::Neoforge => forge_version.to_string(),
        };
        let forge_installer_path =
            download_forge_installer(&full_version, forge_work_dir, loader).await?;

        trick_forge(forge_work_dir, minecraft_version)?;

        run_forge_command(
            &java_installation.path,
            &forge_installer_path,
            forge_work_dir,
        )
        .await?;
    } else {
        info!("Forge {forge_version} already present, skipping installation");
    }

    let launcher_profiles_path = forge_work_dir.join("launcher_profiles.json");
    let launcher_profiles_content = std::fs::read_to_string(&launcher_profiles_path)?;
    let launcher_profiles: LauncherProfiles = serde_json::from_str(&launcher_profiles_content)?;

    let id = launcher_profiles
        .profiles
        .values()
        .next()
        .ok_or(ForgeError::NoForgeProfiles)?
        .last_version_id
        .clone();

    if !lock_file.exists() {
        std::fs::File::create(lock_file)?;
    }

    Ok(id)
}

#[derive(thiserror::Error, Debug)]
pub enum DownloadForgeInstallerError {
    #[error("network request failed while downloading installer: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("file I/O failed while writing installer: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum TrickForgeError {
    #[error("file I/O failed while preparing Forge work dir: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum ToAbsPathError {
    #[error("failed to canonicalize path for Forge command: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum RunForgeCommandError {
    #[error("failed to resolve absolute path for Forge command: {0}")]
    ToAbsPath(#[from] ToAbsPathError),
    #[error("failed to execute Forge installer command: {0}")]
    Io(#[from] std::io::Error),
    #[error("Forge installer command failed: {0}")]
    CommandFailed(String),
    #[error("Forge installer retry command failed: {0}")]
    RetryFailed(String),
}

#[derive(thiserror::Error, Debug)]
pub enum InstallForgeError {
    #[error("file I/O failed while installing Forge: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to download Java runtime for Forge installer: {0}")]
    JavaDownload(#[from] utils::java::JavaDownloadError),
    #[error("failed to download Forge installer: {0}")]
    DownloadInstaller(#[from] DownloadForgeInstallerError),
    #[error("failed to prepare Forge work directory layout: {0}")]
    TrickForge(#[from] TrickForgeError),
    #[error("failed while running Forge installer command: {0}")]
    RunForgeCommand(#[from] RunForgeCommandError),
    #[error("failed to parse launcher profiles JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to resolve Forge profile metadata: {0}")]
    Forge(#[from] ForgeError),
}

#[derive(thiserror::Error, Debug)]
pub enum ForgeGenerateError {
    #[error("failed to resolve Forge/Neoforge version: {0}")]
    GetForgeVersion(#[from] GetForgeVersionError),
    #[error("failed to install Forge/Neoforge: {0}")]
    InstallForge(#[from] InstallForgeError),
    #[error("failed to read generated version metadata: {0}")]
    VersionMetadata(#[from] instance::version_metadata::VersionMetadataError),
    #[error("failed to enumerate generated Forge libraries: {0}")]
    GetFilesInDir(#[from] files::GetFilesInDirError),
    #[error("generated library path is outside installer libraries directory: {0}")]
    StripPrefix(#[from] std::path::StripPrefixError),
}

pub struct ForgeGenerator<'a> {
    vanilla_metadata: &'a VersionMetadata,
    loader: Loader,
    loader_version: Option<String>,
}

pub struct GeneratorResult {
    pub metadata: VersionMetadata,
    pub extra_libs_copy_tasks: Vec<CopyTask>,
    pub installer_work_dir: PathBuf,
}

impl<'a> ForgeGenerator<'a> {
    pub fn new(
        vanilla_metadata: &'a VersionMetadata,
        loader: Loader,
        loader_version: Option<String>,
    ) -> Self {
        Self {
            vanilla_metadata,
            loader,
            loader_version,
        }
    }

    pub async fn generate(
        &self,
        client: &Client,
        output_dir: &DataDir,
        work_dir: &Path,
    ) -> Result<GeneratorResult, ForgeGenerateError> {
        let minecraft_version = self.vanilla_metadata.id.clone();

        info!(
            "Generating {} {}, minecraft version {}",
            self.loader,
            self.loader_version.as_deref().unwrap_or("<auto>"),
            minecraft_version
        );

        let forge_version = get_forge_version(
            client,
            &minecraft_version,
            &self.loader_version,
            &self.loader,
        )
        .await?;

        info!("Using {} version {}", self.loader, &forge_version);

        let installer_work_dir = work_dir
            .join(format!(".{:?}", self.loader))
            .join(get_full_version(&minecraft_version, &forge_version));
        let launcher_data_dir = DataDir::new(work_dir.to_path_buf());
        let id = install_forge(
            &installer_work_dir,
            &launcher_data_dir,
            &forge_version,
            self.vanilla_metadata,
            &self.loader,
        )
        .await?;

        let installer_data_dir = DataDir::new(installer_work_dir.to_path_buf());

        info!("Reading forge version metadata");
        let forge_metadata = VersionMetadata::read_local(&installer_data_dir, &id).await?;

        let installer_libraries_dir = LibrariesDir::root().to_fs(&installer_data_dir);
        let installer_extra_libs_paths = files::get_files_in_dir(&installer_libraries_dir)?
            .into_iter()
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jar"))
            .map(|path| {
                Ok::<PathBuf, ForgeGenerateError>(
                    path.strip_prefix(&installer_libraries_dir)?.to_path_buf(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        info!(
            "Found {} extra {} libs",
            installer_extra_libs_paths.len(),
            self.loader
        );
        debug!(
            "Extra {} libs: {:?}",
            self.loader, installer_extra_libs_paths
        );

        // collect extra forge libs copy tasks
        let libraries_dir = LibrariesDir::root().to_fs(output_dir);
        let mut extra_libs_copy_tasks = Vec::with_capacity(installer_extra_libs_paths.len());
        for lib_path in installer_extra_libs_paths {
            extra_libs_copy_tasks.push(CopyTask {
                source: installer_libraries_dir.join(&lib_path),
                target: libraries_dir.join(&lib_path),
            });
        }

        info!(
            "{} {} for {} generated",
            self.loader, &forge_version, &minecraft_version
        );

        Ok(GeneratorResult {
            metadata: forge_metadata,
            extra_libs_copy_tasks,
            installer_work_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neoforge_metadata(versions: &[&str]) -> NeoforgeMavenMetadata {
        NeoforgeMavenMetadata {
            versioning: Versioning {
                versions: Versions {
                    version: versions
                        .iter()
                        .map(|version| (*version).to_string())
                        .collect(),
                },
            },
        }
    }

    #[test]
    fn neoforge_matching_supports_26x_versions() {
        let metadata = neoforge_metadata(&[
            "21.4.123",
            "26.1.0.1-beta",
            "26.1.0.10-beta",
            "26.1.2.1",
            "26.2.0.1-beta",
        ]);

        let matched = metadata.get_matching_versions("26.1");
        assert_eq!(
            matched,
            vec!["26.1.0.10-beta".to_string(), "26.1.0.1-beta".to_string(),]
        );
        assert_eq!(
            metadata.get_latest_matching_version("26.1"),
            Some("26.1.0.10-beta".to_string())
        );
    }

    #[test]
    fn neoforge_matching_supports_legacy_versions() {
        let metadata = neoforge_metadata(&["21.4.123", "21.4.130", "21.5.1"]);

        let matched = metadata.get_matching_versions("1.21.4");
        assert_eq!(
            matched,
            vec!["21.4.130".to_string(), "21.4.123".to_string()]
        );
    }

    #[test]
    fn neoforge_matching_supports_three_part_minecraft_versions() {
        let metadata = neoforge_metadata(&["26.1.2.1", "26.1.2.2", "26.1.0.10-beta"]);

        let matched = metadata.get_matching_versions("26.1.2");
        assert_eq!(
            matched,
            vec!["26.1.2.2".to_string(), "26.1.2.1".to_string()]
        );
    }
}
