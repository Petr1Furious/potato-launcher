use flate2::read::GzDecoder;
use futures::StreamExt;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use tar::Archive;
use tokio::process::Command;
use url::Url;

use serde_json::Value;
#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::*;

use crate::paths::{DataDir, JavaDir};
use crate::progress::ProgressTracker;

#[derive(Debug, Clone, Deserialize)]
pub struct JavaInstallation {
    pub version: String,
    pub path: PathBuf,
}

/// Launcher-facing platform identity for Java runtime selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaPlatform {
    pub os: String,
    pub arch: String,
}

/// Azul API query target for a specific platform archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JavaPlatformTarget {
    /// Azul API `os` query value (e.g. `linux-glibc`).
    pub azul_os: &'static str,
    /// Launcher/metadata `os` value (e.g. `linux`).
    pub launcher_os: &'static str,
    pub arch: &'static str,
    pub archive_type: &'static str,
}

pub const MIRROR_PLATFORM_TARGETS: &[JavaPlatformTarget] = &[
    JavaPlatformTarget {
        azul_os: "windows",
        launcher_os: "windows",
        arch: "x64",
        archive_type: "zip",
    },
    JavaPlatformTarget {
        azul_os: "linux-glibc",
        launcher_os: "linux",
        arch: "x64",
        archive_type: "tar.gz",
    },
    JavaPlatformTarget {
        azul_os: "macos",
        launcher_os: "macos",
        arch: "x64",
        archive_type: "tar.gz",
    },
    JavaPlatformTarget {
        azul_os: "macos",
        launcher_os: "macos",
        arch: "aarch64",
        archive_type: "tar.gz",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZuluPackage {
    pub name: String,
    pub download_url: Url,
}

lazy_static::lazy_static! {
    static ref JAVA_VERSION_RGX: Regex = Regex::new(r#""(.*)?""#).unwrap();
}

#[cfg(target_os = "windows")]
const JAVA_BINARY_NAME: &str = "java.exe";

#[cfg(not(target_os = "windows"))]
const JAVA_BINARY_NAME: &str = "java";

pub fn current_platform() -> JavaPlatform {
    let arch = match std::env::consts::ARCH {
        "x86_64" | "amd64" => "x64".to_string(),
        "aarch64" => "aarch64".to_string(),
        arch => arch.to_string(),
    };

    let os = std::env::consts::OS.to_string();

    JavaPlatform { os, arch }
}

pub async fn get_installation_pub(path: &Path) -> Option<JavaInstallation> {
    get_installation(path).await
}

async fn get_installation(path: &Path) -> Option<JavaInstallation> {
    let path = if path.is_file() {
        path.to_path_buf()
    } else {
        which::which(path).ok()?
    };

    let mut cmd = Command::new(&path);
    #[cfg(target_os = "windows")]
    {
        use winapi::um::winbase::CREATE_NO_WINDOW;

        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.arg("-version").output().await.ok()?;

    let version_result = String::from_utf8_lossy(&output.stderr);
    let captures = JAVA_VERSION_RGX.captures(&version_result)?;

    let version = captures.get(1)?.as_str().to_string();
    Some(JavaInstallation { version, path })
}

#[cfg(not(target_os = "windows"))]
fn check_arch(java_version_output: &str) -> bool {
    let arch = std::env::consts::ARCH;
    match arch {
        "x86_64" | "amd64" => java_version_output.contains("x86-64"),
        "aarch64" => {
            java_version_output.contains("aarch64") || java_version_output.contains("arm64")
        }
        _ => false,
    }
}

#[cfg(target_os = "windows")]
fn check_arch(_: &str) -> bool {
    true
}

async fn does_match(java: &JavaInstallation, required_version: &str) -> bool {
    if !(java.version.starts_with(&required_version.to_string())
        || java.version.starts_with(&format!("1.{required_version}")))
    {
        return false;
    }

    if std::env::consts::ARCH != "aarch64" {
        return true;
    }
    let output = Command::new("file").arg(&java.path).output().await;
    if let Ok(output) = output {
        let output = String::from_utf8_lossy(&output.stdout);
        check_arch(&output)
    } else {
        false
    }
}

pub async fn check_java(required_version: &str, path: &Path) -> bool {
    if let Some(installation) = get_installation(path).await {
        does_match(&installation, required_version).await
    } else {
        false
    }
}

#[cfg(target_os = "windows")]
fn find_java_in_registry(
    key_name: &str,
    subkey_suffix: &str,
    java_dir_key: &str,
) -> Vec<JavaInstallation> {
    let hk_local_machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = match hk_local_machine
        .open_subkey_with_flags(key_name, KEY_READ | KEY_ENUMERATE_SUB_KEYS)
    {
        Ok(key) => key,
        Err(_) => return Vec::new(),
    };

    let subkeys: Vec<String> = key.enum_keys().filter_map(Result::ok).collect();
    let mut res = Vec::new();

    for subkey in subkeys {
        let key_path = format!("{key_name}\\{subkey}{subkey_suffix}");
        if let Ok(subkey) = hk_local_machine.open_subkey(&key_path)
            && let Ok(java_dir_value) = subkey.get_value::<String, _>(java_dir_key)
        {
            let exe_path = Path::new(&java_dir_value).join("bin").join("java.exe");
            if let Ok(version) = subkey.get_value::<String, _>("Version") {
                res.push(JavaInstallation {
                    version,
                    path: exe_path,
                });
            }
        }
    }

    res
}

#[cfg(target_os = "windows")]
async fn find_java_installations() -> Vec<JavaInstallation> {
    let mut res = Vec::new();

    let registry_paths = vec![
        (r"SOFTWARE\Eclipse Adoptium\JDK", r"\hotspot\MSI", "Path"),
        (r"SOFTWARE\Eclipse Adoptium\JRE", r"\hotspot\MSI", "Path"),
        (r"SOFTWARE\AdoptOpenJDK\JDK", r"\hotspot\MSI", "Path"),
        (r"SOFTWARE\AdoptOpenJDK\JRE", r"\hotspot\MSI", "Path"),
        (r"SOFTWARE\Eclipse Foundation\JDK", r"\hotspot\MSI", "Path"),
        (r"SOFTWARE\Eclipse Foundation\JRE", r"\hotspot\MSI", "Path"),
        (r"SOFTWARE\JavaSoft\JDK", "", "JavaHome"),
        (r"SOFTWARE\JavaSoft\JRE", "", "JavaHome"),
        (r"SOFTWARE\Microsoft\JDK", r"\hotspot\MSI", "Path"),
        (r"SOFTWARE\Azul Systems\Zulu", "", "InstallationPath"),
        (r"SOFTWARE\BellSoft\Liberica", "", "InstallationPath"),
    ];

    for (key, subkey_suffix, java_dir_key) in registry_paths {
        res.extend(find_java_in_registry(key, subkey_suffix, java_dir_key));
    }

    res
}

#[cfg(not(target_os = "windows"))]
async fn find_java_in_dir(dir: &Path, suffix: &str, startswith: &str) -> Vec<JavaInstallation> {
    let mut res = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let subdir = entry.path();
            if subdir.is_file() {
                continue;
            }
            if !startswith.is_empty()
                && !subdir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with(startswith)
            {
                continue;
            }
            if let Some(java) =
                get_installation(&subdir.join(suffix).join("bin").join("java")).await
            {
                res.push(java);
            }
        }
    }

    res
}

#[cfg(target_os = "linux")]
async fn find_java_installations() -> Vec<JavaInstallation> {
    let dirs = [
        "/usr/java",
        "/usr/lib/jvm",
        "/usr/lib64/jvm",
        "/usr/lib32/jvm",
        "/opt/jdk",
    ];
    let mut res = Vec::new();
    for dir in dirs.iter() {
        res.extend(find_java_in_dir(Path::new(dir), "", "").await);
    }
    res
}

#[cfg(target_os = "macos")]
async fn find_java_installations() -> Vec<JavaInstallation> {
    let args = [
        ("/Library/Java/JavaVirtualMachines", "Contents/Home", ""),
        (
            "/System/Library/Java/JavaVirtualMachines",
            "Contents/Home",
            "",
        ),
        ("/usr/local/opt", "", "openjdk"),
        ("/opt/homebrew/opt", "", "openjdk"),
    ];
    let mut res = Vec::new();
    for (dir, suffix, startswith) in args.iter() {
        res.extend(find_java_in_dir(Path::new(dir), suffix, startswith).await);
    }
    res
}

#[derive(thiserror::Error, Debug)]
pub enum JavaDownloadError {
    #[error("no Java versions available")]
    NoJavaVersionsAvailable,
    #[error("downloaded Java installation did not pass validation")]
    InvalidDownloadedJava,
    #[error("Java metadata response does not contain a versions array")]
    NoVersionsArray,
    #[error("Java metadata response is missing package name")]
    NoPackageName,
    #[error("Java metadata response is missing download URL")]
    NoDownloadURL,
    #[error("download URL does not contain a file name")]
    NoFileNameInURL,
    #[error("archive name does not have the expected file extension")]
    NoFileExtensionInName,
    #[error("network request failed while downloading Java: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to parse Java metadata JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to parse Java download URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("file I/O failed while downloading/installing Java: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to extract Java archive: {0}")]
    Extract(#[from] crate::files::ExtractZipError),
}

fn get_java_download_params_for_target(
    required_version: &str,
    target: &JavaPlatformTarget,
) -> String {
    format!(
        "java_version={required_version}&os={}&arch={}&archive_type={}&java_package_type=jre&javafx_bundled=false&latest=true&release_status=ga",
        target.azul_os, target.arch, target.archive_type
    )
}

pub async fn query_zulu_package(
    client: &Client,
    required_version: &str,
    target: &JavaPlatformTarget,
) -> Result<Option<ZuluPackage>, JavaDownloadError> {
    let query_str = get_java_download_params_for_target(required_version, target);
    let versions_url = format!("https://api.azul.com/metadata/v1/zulu/packages/?{query_str}");

    let response = client.get(&versions_url).send().await?;
    let body = response.text().await?;
    let versions: Value = serde_json::from_str(&body)?;

    let Some(entries) = versions.as_array() else {
        return Err(JavaDownloadError::NoVersionsArray);
    };
    if entries.is_empty() {
        return Ok(None);
    }

    let entry = &entries[0];
    let name = entry["name"]
        .as_str()
        .ok_or(JavaDownloadError::NoPackageName)?
        .to_string();
    let download_url = entry["download_url"]
        .as_str()
        .ok_or(JavaDownloadError::NoDownloadURL)?;
    Ok(Some(ZuluPackage {
        name,
        download_url: Url::parse(download_url)?,
    }))
}

async fn java_archive_download_path(
    data_dir: &DataDir,
    archive_type: &str,
) -> Result<PathBuf, JavaDownloadError> {
    let tmp_dir = data_dir.tmp_dir();
    tokio::fs::create_dir_all(&tmp_dir).await?;
    Ok(tmp_dir.join(format!(
        "java_download.{}.{archive_type}",
        std::process::id()
    )))
}

/// Zulu macOS archives ship as .app bundles where the JRE lives under
/// {archive_root}/Contents/Home, while Linux/Windows archives use a flat layout.
async fn resolve_java_home_dir(extracted_root: &Path) -> PathBuf {
    let bundle_home = extracted_root.join("Contents").join("Home");
    if tokio::fs::metadata(&bundle_home)
        .await
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        bundle_home
    } else {
        extracted_root.to_path_buf()
    }
}

async fn install_extracted_java(
    java_dir: &Path,
    filename: &str,
    target_dir: &Path,
) -> Result<(), JavaDownloadError> {
    let extracted_root = java_dir.join(filename);
    let java_home_src = resolve_java_home_dir(&extracted_root).await;

    if tokio::fs::try_exists(target_dir).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(target_dir).await?;
    }

    tokio::fs::rename(&java_home_src, target_dir).await?;

    if java_home_src != extracted_root
        && tokio::fs::try_exists(&extracted_root)
            .await
            .unwrap_or(false)
    {
        tokio::fs::remove_dir_all(&extracted_root).await?;
    }

    Ok(())
}

fn archive_root_name<'a>(name: &'a str, archive_type: &str) -> Result<&'a str, JavaDownloadError> {
    name.strip_suffix(&format!(".{archive_type}"))
        .ok_or(JavaDownloadError::NoFileExtensionInName)
}

async fn write_response_to_file(
    response: reqwest::Response,
    path: &Path,
    progress_tracker: &impl ProgressTracker,
) -> Result<(), JavaDownloadError> {
    use tokio::io::AsyncWriteExt as _;

    let mut file = tokio::fs::File::create(path).await?;
    let total_size = response.content_length().unwrap_or(0);
    progress_tracker.set_length(total_size);

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        progress_tracker.inc(chunk.len() as u64);
    }
    file.flush().await?;
    Ok(())
}

async fn install_java_from_archive(
    archive_path: &Path,
    archive_type: &str,
    archive_name: &str,
    required_version: &str,
    data_dir: &DataDir,
) -> Result<JavaInstallation, JavaDownloadError> {
    let java_dir = JavaDir::root().to_fs(data_dir);
    let filename = archive_root_name(archive_name, archive_type)?.to_string();

    let target_dir = JavaDir::root()
        .java_version_dir(required_version)
        .to_fs(data_dir);

    if archive_type == "tar.gz" {
        let archive_path = archive_path.to_path_buf();
        let java_dir = java_dir.clone();
        tokio::task::spawn_blocking(move || -> Result<(), JavaDownloadError> {
            let archive = fs::File::open(&archive_path)?;
            let tar = GzDecoder::new(archive);
            let mut archive = Archive::new(tar);
            archive.unpack(&java_dir)?;
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)??;
    } else {
        crate::files::extract_zip(archive_path, &java_dir, false).await?;
    }

    install_extracted_java(&java_dir, &filename, &target_dir).await?;

    let java_path = JavaDir::root()
        .java_version_dir(required_version)
        .bin_path(JAVA_BINARY_NAME)
        .to_fs(data_dir);
    if !check_java(required_version, &java_path).await {
        return Err(JavaDownloadError::InvalidDownloadedJava);
    }
    get_installation(&java_path)
        .await
        .ok_or(JavaDownloadError::InvalidDownloadedJava)
}

#[allow(clippy::too_many_arguments)]
pub async fn download_java_from_runtime(
    client: &Client,
    url: &Url,
    archive_type: &str,
    archive_name: &str,
    required_version: &str,
    data_dir: &DataDir,
    progress_tracker: impl ProgressTracker,
    unpacking_message: &str,
) -> Result<JavaInstallation, JavaDownloadError> {
    let response = client.get(url.clone()).send().await?;
    let java_download_path = java_archive_download_path(data_dir, archive_type).await?;
    let _archive_guard = crate::files::TempFileGuard::new(java_download_path.clone());
    write_response_to_file(response, &java_download_path, &progress_tracker).await?;
    progress_tracker.set_length(0);
    progress_tracker.set_message(unpacking_message);
    install_java_from_archive(
        &java_download_path,
        archive_type,
        archive_name,
        required_version,
        data_dir,
    )
    .await
}

pub async fn download_java(
    client: &Client,
    required_version: &str,
    data_dir: &DataDir,
    progress_tracker: impl ProgressTracker,
    unpacking_message: Option<&str>,
) -> Result<JavaInstallation, JavaDownloadError> {
    let platform = current_platform();
    let archive_types = ["tar.gz", "zip"];

    for archive_type in archive_types {
        let Some(target) = MIRROR_PLATFORM_TARGETS.iter().find(|target| {
            target.launcher_os == platform.os
                && target.arch == platform.arch
                && target.archive_type == archive_type
        }) else {
            continue;
        };

        let Some(package) = query_zulu_package(client, required_version, target).await? else {
            continue;
        };

        let response = client.get(package.download_url.clone()).send().await?;
        let java_download_path = java_archive_download_path(data_dir, archive_type).await?;
        let _archive_guard = crate::files::TempFileGuard::new(java_download_path.clone());
        write_response_to_file(response, &java_download_path, &progress_tracker).await?;
        if let Some(message) = unpacking_message {
            progress_tracker.set_length(0);
            progress_tracker.set_message(message);
        }

        if let Ok(installation) = install_java_from_archive(
            &java_download_path,
            archive_type,
            &package.name,
            required_version,
            data_dir,
        )
        .await
        {
            return Ok(installation);
        }
    }

    Err(JavaDownloadError::NoJavaVersionsAvailable)
}

pub async fn get_java(required_version: &str, data_dir: &DataDir) -> Option<JavaInstallation> {
    let java_dir = JavaDir::root()
        .java_version_dir(required_version)
        .bin_path(JAVA_BINARY_NAME)
        .to_fs(data_dir);
    let mut installations = find_java_installations().await;

    if let Some(default_installation) = get_installation(Path::new(JAVA_BINARY_NAME)).await {
        installations.push(default_installation);
    }

    if let Some(installation) = get_installation(&java_dir).await {
        installations.push(installation);
    }

    for installation in installations {
        if does_match(&installation, required_version).await {
            return Some(installation);
        }
    }

    None
}
