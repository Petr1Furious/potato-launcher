use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use log::{debug, info};
use url::Url;
use rand::{SeedableRng as _, rngs::StdRng, seq::SliceRandom as _};
use shared::{
    adaptive_download::download_files,
    files::{CheckEntry, get_download_entries},
    paths::{get_client_jar_path, get_libraries_dir},
    progress::ProgressBar,
    version::{asset_metadata::AssetsMetadata, version_metadata::VersionMetadata},
};

use crate::{progress::TerminalProgressBar, utils::get_assets_dir};

pub fn get_libraries_check_downloads(
    version_metadata: &VersionMetadata,
    libraries_dir: &Path,
) -> Vec<CheckEntry> {
    let mut entries = vec![];
    for library in &version_metadata.libraries {
        entries.extend(library.get_check_entries(libraries_dir, None));
    }
    debug!("Library check entries: {entries:?}");
    entries
}

fn get_client_download_entry(
    version_metadata: &VersionMetadata,
    data_dir: &Path,
) -> Option<CheckEntry> {
    let client_download = version_metadata.downloads.as_ref()?.client.as_ref()?;

    Some(client_download.get_check_entry(&get_client_jar_path(data_dir, &version_metadata.id)))
}

const RESOURCES_URL_BASE: &str = "https://resources.download.minecraft.net";

pub struct SyncResult {
    pub paths_to_copy: Vec<PathBuf>,
}

pub async fn sync_version(
    version_metadata: &VersionMetadata,
    output_dir: &Path,
) -> anyhow::Result<SyncResult> {
    let libraries_dir = get_libraries_dir(output_dir);
    let mut check_entries = get_libraries_check_downloads(version_metadata, &libraries_dir);
    info!("Got {} libraries to check", check_entries.len());

    if let Some(asset_index) = &version_metadata.asset_index {
        let assets_dir = get_assets_dir(output_dir);
        let assets_metadata = AssetsMetadata::read_or_download(asset_index, &assets_dir).await?;
        let resources_url_base = Url::parse(RESOURCES_URL_BASE)?;
        let asset_check_entries =
            assets_metadata.get_check_entries(&assets_dir, &resources_url_base, true)?;

        let mut already_have = 0;
        for entry in &asset_check_entries {
            if entry.path.exists() {
                already_have += 1;
            }
        }
        info!(
            "Already have {}/{} assets",
            already_have,
            asset_check_entries.len()
        );

        check_entries.extend(asset_check_entries);
    }

    if let Some(client_entry) = get_client_download_entry(version_metadata, output_dir) {
        info!("Got client.jar to check");
        check_entries.push(client_entry);
    }

    let progress_bar = Arc::new(TerminalProgressBar::new());

    let all_paths = check_entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();

    progress_bar.set_message("Checking files...");
    let mut download_entries = get_download_entries(check_entries, progress_bar.clone()).await?;

    let mut rng = StdRng::from_os_rng();
    download_entries.shuffle(&mut rng);

    progress_bar.reset();
    progress_bar.set_message("Downloading files...");
    download_files(download_entries, progress_bar).await?;

    Ok(SyncResult {
        paths_to_copy: all_paths,
    })
}
