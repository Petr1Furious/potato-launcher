use std::{env, fs, path::Path, process::Command, sync::Arc};

use futures::StreamExt as _;
use launcher_bridge::{FrontendSender, MessageToFrontend, NotificationLevel, UpdateStatusView};
use launcher_build_config::UpdateSource;
use reqwest::{
    Client,
    header::{ACCEPT, USER_AGENT},
};
use serde::Deserialize;

/// A package manager owns the installed binary, so the launcher must not
/// replace it and can only tell the user that an update is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackagedInstall {
    Flatpak,
    Nix,
}

fn packaged_install() -> Option<PackagedInstall> {
    if env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists() {
        return Some(PackagedInstall::Flatpak);
    }
    if env::current_exe().is_ok_and(|exe| exe.starts_with("/nix/store")) {
        return Some(PackagedInstall::Nix);
    }
    None
}

fn os_dir() -> &'static str {
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(target_os = "macos")]
    return "macos";
}

fn release_file(package: Option<PackagedInstall>) -> String {
    let lower = launcher_build_config::release_lower_name();
    if package == Some(PackagedInstall::Flatpak) {
        return format!("{lower}.flatpak");
    }
    #[cfg(target_os = "windows")]
    return format!("{}.exe", launcher_build_config::launcher_name());
    #[cfg(target_os = "linux")]
    return lower;
    #[cfg(target_os = "macos")]
    return format!("{lower}_macos.tar.gz");
}

/// The name `potato publish-launcher` stores the file under on the site.
fn site_file(package: Option<PackagedInstall>) -> &'static str {
    if package == Some(PackagedInstall::Flatpak) {
        return "launcher.flatpak";
    }
    #[cfg(target_os = "windows")]
    return "launcher.exe";
    #[cfg(target_os = "linux")]
    return "launcher";
    #[cfg(target_os = "macos")]
    return "launcher_macos.tar.gz";
}

struct Release {
    version: String,
    download_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

async fn fetch_release(
    client: &Client,
    package: Option<PackagedInstall>,
) -> anyhow::Result<Release> {
    match launcher_build_config::update_source() {
        None => Err(anyhow::anyhow!("No update source configured")),
        Some(UpdateSource::Static(base)) => {
            let dir = format!("{base}/{}", os_dir());
            let version = client
                .get(format!("{dir}/version.txt"))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            Ok(Release {
                version: version.trim().to_string(),
                download_url: Some(format!("{dir}/{}", site_file(package))),
            })
        }
        Some(UpdateSource::GitHub(repo)) => {
            let file = release_file(package);
            let release: GitHubRelease = client
                .get(format!(
                    "https://api.github.com/repos/{repo}/releases/latest"
                ))
                .header(USER_AGENT, "potato-launcher")
                .header(ACCEPT, "application/vnd.github+json")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let download_url = release
                .assets
                .into_iter()
                .find(|asset| asset.name == file)
                .map(|asset| asset.browser_download_url);
            Ok(Release {
                version: release.tag_name,
                download_url,
            })
        }
    }
}

fn should_check_updates() -> bool {
    if env::var("CARGO").is_ok() {
        log::info!("Running from cargo, skipping auto-update");
        return false;
    }
    if launcher_build_config::version().is_none() {
        log::info!("Version not set, skipping auto-update");
        return false;
    }
    if launcher_build_config::update_source().is_none() {
        log::info!("Update source not set, skipping auto-update");
        return false;
    }
    true
}

fn is_connect_error(e: &anyhow::Error) -> bool {
    if let Some(e) = e.downcast_ref::<reqwest::Error>() {
        return e.is_connect() || e.status().is_some_and(|s| s.as_u16() == 523);
    }
    false
}

fn is_read_only_error(e: &anyhow::Error) -> bool {
    if let Some(e) = e.downcast_ref::<std::io::Error>() {
        return e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(18);
    }
    false
}

async fn newer_release(
    client: &Client,
    package: Option<PackagedInstall>,
) -> anyhow::Result<Option<Release>> {
    let release = fetch_release(client, package).await?;
    let local = launcher_build_config::version().expect("version checked in should_check_updates");
    Ok(is_newer(&release.version, local).then_some(release))
}

/// Semver when both parse (`v` prefix allowed), otherwise any difference.
fn is_newer(remote: &str, local: &str) -> bool {
    let parse =
        |version: &str| semver::Version::parse(version.strip_prefix('v').unwrap_or(version)).ok();
    match (parse(remote), parse(local)) {
        (Some(remote), Some(local)) => remote > local,
        _ => remote != local,
    }
}

async fn download(
    client: &Client,
    frontend: &FrontendSender,
    release: &Release,
) -> anyhow::Result<Vec<u8>> {
    let url = release
        .download_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Release {} has no file for this OS", release.version))?;
    let response = client.get(url).send().await?.error_for_status()?;
    let total = response.content_length().unwrap_or(0);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
        frontend.send(MessageToFrontend::UpdateStatus(
            UpdateStatusView::Downloading {
                current: bytes.len() as u64,
                total,
            },
        ));
    }
    Ok(bytes)
}

/// Starts a fresh launcher process with the same arguments and exits this one.
pub fn restart_launcher() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    Command::new(env::current_exe()?).args(&args[1..]).spawn()?;
    std::process::exit(0);
}

#[cfg(not(target_os = "macos"))]
fn replace_and_restart(new_binary: &[u8]) -> anyhow::Result<()> {
    let temp_exe = env::temp_dir().join("new_launcher_update");
    fs::write(&temp_exe, new_binary)?;
    self_replace::self_replace(&temp_exe)?;
    fs::remove_file(&temp_exe).ok();
    restart_launcher()
}

#[cfg(target_os = "macos")]
fn replace_and_restart(new_archive: &[u8]) -> anyhow::Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let current_exe = env::current_exe()?;
    let contents_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No parent dir for executable"))?;
    let bundle_dir = contents_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No Contents dir"))?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No bundle dir"))?;

    let app_name = bundle_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Bundle dir has no name"))?;

    if !app_name.ends_with(".app") {
        return Err(anyhow::anyhow!(
            "Not running inside a .app bundle: {bundle_dir:?}"
        ));
    }

    let temp_dir = env::temp_dir().join("launcher_update_extract");
    let backup_dir = env::temp_dir().join("launcher_update_backup");

    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    let gz = GzDecoder::new(new_archive);
    let mut archive = Archive::new(gz);
    archive.unpack(&temp_dir)?;

    if backup_dir.exists() {
        fs::remove_dir_all(&backup_dir)?;
    }
    fs::rename(bundle_dir, &backup_dir)?;
    fs::rename(temp_dir.join("update.app"), bundle_dir)?;
    fs::remove_dir_all(&backup_dir).ok();

    restart_launcher()
}

/// Starts the update check. Self-updating installs block on the update screen
/// until it finishes; package-managed installs only get a notification.
pub fn start(client: Client, frontend: FrontendSender) {
    if !should_check_updates() {
        frontend.send(MessageToFrontend::UpdateStatus(
            UpdateStatusView::NotApplicable,
        ));
        return;
    }
    if let Some(package) = packaged_install() {
        log::info!("Installed via {package:?}, only checking for update notifications");
        frontend.send(MessageToFrontend::UpdateStatus(
            UpdateStatusView::NotApplicable,
        ));
        tokio::spawn(notify_if_outdated(client, frontend, package));
    } else {
        frontend.send(MessageToFrontend::UpdateStatus(UpdateStatusView::Checking));
        tokio::spawn(run(client, frontend));
    }
}

async fn notify_if_outdated(client: Client, frontend: FrontendSender, package: PackagedInstall) {
    match newer_release(&client, Some(package)).await {
        Ok(None) => {}
        Ok(Some(_)) => {
            let message = match package {
                PackagedInstall::Flatpak => launcher_i18n::update::available_flatpak(),
                PackagedInstall::Nix => launcher_i18n::update::available_nix(),
            };
            frontend.send(MessageToFrontend::Notification {
                level: NotificationLevel::Warning,
                message: Arc::from(message),
            });
        }
        Err(e) => log::warn!("Failed to check for updates: {e:#}"),
    }
}

async fn run(client: Client, frontend: FrontendSender) {
    match newer_release(&client, None).await {
        Ok(None) => {
            frontend.send(MessageToFrontend::UpdateStatus(UpdateStatusView::UpToDate));
        }
        Ok(Some(release)) => match download(&client, &frontend, &release).await {
            Ok(bytes) => {
                frontend.send(MessageToFrontend::UpdateStatus(UpdateStatusView::Replacing));
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if let Err(e) = replace_and_restart(&bytes) {
                    log::error!("Failed to apply update: {e:#}");
                    let status = if is_read_only_error(&e) {
                        UpdateStatusView::ReadOnly
                    } else {
                        UpdateStatusView::Error {
                            message: Arc::from(e.to_string()),
                            offline: false,
                        }
                    };
                    frontend.send(MessageToFrontend::UpdateStatus(status));
                }
            }
            Err(e) => {
                log::error!("Failed to download update: {e:#}");
                let status = if is_read_only_error(&e) {
                    UpdateStatusView::ReadOnly
                } else {
                    UpdateStatusView::Error {
                        message: Arc::from(e.to_string()),
                        offline: is_connect_error(&e),
                    }
                };
                frontend.send(MessageToFrontend::UpdateStatus(status));
            }
        },
        Err(e) => {
            log::error!("Failed to check for updates: {e:#}");
            frontend.send(MessageToFrontend::UpdateStatus(UpdateStatusView::Error {
                message: Arc::from(e.to_string()),
                offline: is_connect_error(&e),
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_release_tags() {
        assert!(is_newer("v1.2.0", "v1.1.9"));
        assert!(!is_newer("v1.1.0", "v1.2.0"));
        assert!(!is_newer("v1.2.0", "v1.2.0"));
        assert!(is_newer("v1.2.0", "v1.2.0-rc1"));
        assert!(is_newer("v0.0.1-dev2", "v0.0.1-dev1"));
        assert!(is_newer("v1.0.0", "3f2a9c1"));
    }
}
