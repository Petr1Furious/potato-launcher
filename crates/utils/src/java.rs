use flate2::read::GzDecoder;
use futures::StreamExt;
use regex::Regex;
use reqwest::{Client, Url};
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tar::Archive;
use tokio::process::Command;

use serde_json::Value;
#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::*;

use crate::paths::{DataDir, JavaDir};
use crate::progress::ProgressTracker;

#[derive(Debug, Deserialize)]
pub struct JavaInstallation {
    pub version: String,
    pub path: PathBuf,
}

lazy_static::lazy_static! {
    static ref JAVA_VERSION_RGX: Regex = Regex::new(r#""(.*)?""#).unwrap();
}

#[cfg(target_os = "windows")]
const JAVA_BINARY_NAME: &str = "java.exe";

#[cfg(not(target_os = "windows"))]
const JAVA_BINARY_NAME: &str = "java";

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
        if let Ok(subkey) = hk_local_machine.open_subkey(&key_path) {
            if let Ok(java_dir_value) = subkey.get_value::<String, _>(java_dir_key) {
                let exe_path = Path::new(&java_dir_value).join("bin").join("java.exe");
                if let Ok(version) = subkey.get_value::<String, _>("Version") {
                    res.push(JavaInstallation {
                        version,
                        path: exe_path,
                    });
                }
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
pub enum JavaParamsError {
    #[error("unsupported architecture")]
    UnsupportedArchitecture,
    #[error("unsupported operating system")]
    UnsupportedOS,
}

#[derive(thiserror::Error, Debug)]
pub enum JavaDownloadError {
    #[error("failed to build Java download query params: {0}")]
    Params(#[from] JavaParamsError),
    #[error("no Java versions available")]
    NoJavaVersionsAvailable,
    #[error("downloaded Java installation did not pass validation")]
    InvalidDownloadedJava,
    #[error("Java metadata response does not contain a versions array")]
    NoVersionsArray,
    #[error("Java metadata response is missing download URL")]
    NoDownloadURL,
    #[error("download URL does not contain a file name")]
    NoFileNameInURL,
    #[error("download URL does not have the expected file extension")]
    NoFileExtensionInURL,
    #[error("network request failed while downloading Java: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to parse Java metadata JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to parse Java download URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("file I/O failed while downloading/installing Java: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to extract Java ZIP archive: {0}")]
    Zip(#[from] zip::result::ZipError),
}

fn get_java_download_params(
    required_version: &str,
    archive_type: &str,
) -> Result<String, JavaParamsError> {
    let arch = match std::env::consts::ARCH {
        "x86_64" | "amd64" => "x64",
        "aarch64" => "aarch64",
        _ => return Err(JavaParamsError::UnsupportedArchitecture),
    };

    let os = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux-glibc",
        "macos" => "macos",
        _ => return Err(JavaParamsError::UnsupportedOS),
    };

    let params = format!(
        "java_version={required_version}&os={os}&arch={arch}&archive_type={archive_type}&java_package_type=jre&javafx_bundled=false&latest=true&release_status=ga"
    );

    Ok(params)
}

pub fn get_temp_dir() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    let temp_dir = temp_dir.join("temp_java_download");
    if !temp_dir.exists() {
        fs::create_dir_all(&temp_dir).unwrap();
    }
    temp_dir
}

pub async fn download_java(
    required_version: &str,
    data_dir: &DataDir,
    progress_tracker: impl ProgressTracker,
) -> Result<JavaInstallation, JavaDownloadError> {
    let java_dir = JavaDir::root().to_fs(data_dir);
    let client = Client::new();

    for archive_type in ["tar.gz", "zip"] {
        let query_str = get_java_download_params(required_version, archive_type)?;

        let versions_url = format!("https://api.azul.com/metadata/v1/zulu/packages/?{query_str}");

        let response = client.get(&versions_url).send().await?;
        let body = response.text().await?;
        let versions: Value = serde_json::from_str(&body)?;

        if versions
            .as_array()
            .ok_or(JavaDownloadError::NoVersionsArray)?
            .is_empty()
        {
            continue;
        }

        let version_url = versions[0]["download_url"]
            .as_str()
            .ok_or(JavaDownloadError::NoDownloadURL)?;
        let response = client.get(version_url).send().await?;

        let java_download_path = get_temp_dir().join(format!("java_download.{archive_type}"));
        let mut file = fs::File::create(&java_download_path)?;

        let total_size = response.content_length().unwrap_or(0);
        progress_tracker.set_length(total_size);

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            progress_tracker.inc(chunk.len() as u64);
        }
        progress_tracker.finish();

        let target_dir = JavaDir::root()
            .java_version_dir(required_version)
            .to_fs(data_dir);
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }

        let archive = fs::File::open(&java_download_path)?;
        if archive_type == "tar.gz" {
            let tar = GzDecoder::new(archive);
            let mut archive = Archive::new(tar);
            archive.unpack(&java_dir)?;
        } else {
            let mut archive = zip::ZipArchive::new(archive)?;
            archive.extract(&java_dir)?;
        }

        let url = Url::parse(version_url)?;
        let filename = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .ok_or(JavaDownloadError::NoFileNameInURL)?
            .strip_suffix(&format!(".{archive_type}"))
            .ok_or(JavaDownloadError::NoFileExtensionInURL)?;
        fs::rename(java_dir.join(filename), &target_dir)?;

        let java_path = JavaDir::root()
            .java_version_dir(required_version)
            .bin_path(JAVA_BINARY_NAME)
            .to_fs(data_dir);
        if !check_java(required_version, &java_path).await {
            return Err(JavaDownloadError::InvalidDownloadedJava);
        }
        if let Some(installation) = get_installation(&java_path).await {
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
