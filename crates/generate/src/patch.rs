use std::path::Path;

use shared::{
    files::hash_file,
    paths::{get_asset_index_path, get_client_jar_path, get_libraries_dir},
    utils::url_from_path,
    version::version_metadata::{Download, LibraryDownloads, VersionMetadata},
};

use crate::utils::get_assets_dir;

pub async fn replace_download_urls(
    version_metadata: &mut VersionMetadata,
    download_server_base: &str,
    data_dir: &Path,
) -> anyhow::Result<()> {
    let libraries_dir = get_libraries_dir(data_dir);

    if let Some(downloads) = &mut version_metadata.downloads
        && let Some(download) = &mut downloads.client
    {
        let client_path = get_client_jar_path(data_dir, &version_metadata.id);
        download.url = url_from_path(&client_path, data_dir, download_server_base)?;
    }

    if let Some(asset_index) = &mut version_metadata.asset_index {
        let asset_index_path = get_asset_index_path(&get_assets_dir(data_dir), &asset_index.id);
        asset_index.url = url_from_path(&asset_index_path, data_dir, download_server_base)?;
    }

    for library in &mut version_metadata.libraries {
        if let Some(library_path) = library.get_library_path(&libraries_dir) {
            if let Some(downloads) = &mut library.downloads {
                if let Some(artifact) = &mut downloads.artifact {
                    artifact.url = url_from_path(&library_path, data_dir, download_server_base)?;
                }
            } else if library.url.is_some() {
                let sha1 = if let Some(sha1) = &library.sha1 {
                    sha1.clone()
                } else {
                    hash_file(&library_path).await?
                };
                library.url = None;
                library.sha1 = None;
                library.downloads = Some(LibraryDownloads {
                    artifact: Some(Download {
                        url: url_from_path(&library_path, data_dir, download_server_base)?,
                        sha1,
                    }),
                    classifiers: None,
                });
            }
        }
    }

    for library in &mut version_metadata.libraries {
        if let Some(downloads) = &library.downloads
            && let Some(natives) = &downloads.classifiers
        {
            let mut new_natives_urls = vec![];

            for (native_name, download) in natives.clone() {
                let natives_path = library.get_native_path(&libraries_dir, &native_name, &download);
                new_natives_urls.push(url_from_path(
                    &natives_path,
                    data_dir,
                    download_server_base,
                )?);
            }

            let natives = library
                .downloads
                .as_mut()
                .unwrap()
                .classifiers
                .as_mut()
                .unwrap();
            for (download, new_url) in natives.values_mut().zip(new_natives_urls) {
                download.url = new_url;
            }
        }
    }

    Ok(())
}
