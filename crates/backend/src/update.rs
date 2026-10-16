use std::{env, fs, path::Path, process::Command, sync::Arc};

use futures::StreamExt as _;
use launcher_bridge::{FrontendSender, MessageToFrontend, NotificationLevel, UpdateStatusView};
use reqwest::Client;

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

fn update_url() -> Option<String> {
    launcher_build_config::backend_api_base().map(|base| {
        #[cfg(target_os = "windows")]
        return format!("{base}/launchers/windows/exe");
        #[cfg(target_os = "linux")]
        return format!("{base}/launchers/linux/bin");
        #[cfg(target_os = "macos")]
        return format!("{base}/launchers/macos/archive");
    })
}

fn version_url(package: Option<PackagedInstall>) -> Option<String> {
    if package == Some(PackagedInstall::Flatpak) {
        return launcher_build_config::backend_api_base()
            .map(|base| format!("{base}/launchers/linux/flatpak/version"));
    }
    update_url().map(|url| format!("{url}/version"))
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
    if launcher_build_config::backend_api_base().is_none() {
        log::info!("Backend API base not set, skipping auto-update");
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

async fn fetch_new_version(
    client: &Client,
    package: Option<PackagedInstall>,
) -> anyhow::Result<String> {
    let version_url =
        version_url(package).ok_or_else(|| anyhow::anyhow!("No update URL configured"))?;
    let text = client
        .get(version_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(text.trim().to_string())
}

async fn need_update(client: &Client, package: Option<PackagedInstall>) -> anyhow::Result<bool> {
    let remote = fetch_new_version(client, package).await?;
    let local = launcher_build_config::version().expect("version checked in should_check_updates");
    Ok(remote != local)
}

async fn download(client: &Client, frontend: &FrontendSender) -> anyhow::Result<Vec<u8>> {
    let url = update_url().ok_or_else(|| anyhow::anyhow!("No update URL configured"))?;
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
    match need_update(&client, Some(package)).await {
        Ok(false) => {}
        Ok(true) => {
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
    match need_update(&client, None).await {
        Ok(false) => {
            frontend.send(MessageToFrontend::UpdateStatus(UpdateStatusView::UpToDate));
        }
        Ok(true) => match download(&client, &frontend).await {
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
