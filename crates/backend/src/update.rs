use std::{env, fs, process::Command, sync::Arc};

use futures::StreamExt as _;
use launcher_bridge::{FrontendSender, MessageToFrontend, UpdateStatusView};
use reqwest::Client;

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

fn version_url() -> Option<String> {
    update_url().map(|url| format!("{url}/version"))
}

pub fn should_check_updates() -> bool {
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

async fn fetch_new_version(client: &Client) -> anyhow::Result<String> {
    let version_url = version_url().ok_or_else(|| anyhow::anyhow!("No update URL configured"))?;
    let text = client
        .get(version_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(text.trim().to_string())
}

async fn need_update(client: &Client) -> anyhow::Result<bool> {
    let remote = fetch_new_version(client).await?;
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

#[cfg(not(target_os = "macos"))]
fn replace_and_restart(new_binary: &[u8]) -> anyhow::Result<()> {
    let current_exe = env::current_exe()?;
    let temp_exe = env::temp_dir().join("new_launcher_update");
    fs::write(&temp_exe, new_binary)?;
    self_replace::self_replace(&temp_exe)?;
    fs::remove_file(&temp_exe).ok();
    let args: Vec<String> = env::args().collect();
    Command::new(&current_exe).args(&args[1..]).spawn()?;
    std::process::exit(0);
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

    let args: Vec<String> = env::args().collect();
    Command::new(&current_exe).args(&args[1..]).spawn()?;
    std::process::exit(0);
}

pub async fn run(client: Client, frontend: FrontendSender) {
    match need_update(&client).await {
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
