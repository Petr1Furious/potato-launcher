use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use instance::{
    manifest::{InstanceManifestEntry, VersionMetadataInfo},
    version_metadata::VersionMetadata,
};
use utils::{
    files::hash_file,
    paths::{BaseUrl, DataDir, InstancesDir, VersionsDir},
};

pub async fn get_instance_manifest_entry(
    work_dir: &Path,
    version_metadata: &[VersionMetadata],
    version_name: &str,
    download_server_base: Option<&BaseUrl>,
    replaced_metadata: &HashMap<String, PathBuf>,
) -> anyhow::Result<InstanceManifestEntry> {
    // is used in local instances to be compliant with the manifest format
    let download_server_base = download_server_base.unwrap_or("empty-url");
    let data_dir = DataDir::new(work_dir.to_path_buf());

    let mut metadata_info = vec![];
    for metadata in version_metadata {
        let rel_metadata_path = VersionsDir::root().metadata_path(&metadata.id);
        let metadata_path = replaced_metadata
            .get(&metadata.id)
            .cloned()
            .unwrap_or_else(|| rel_metadata_path.to_fs(&data_dir));
        metadata_info.push(VersionMetadataInfo {
            id: metadata.id.clone(),
            url,
            sha1: hash_file(&metadata_path).await?,
        });
    }

    let instance_dir = InstancesDir::root().instance_dir(version_name);
    let rel_meta_path = instance_dir.meta_path();
    let meta_path = rel_meta_path.to_fs(&data_dir);
    let meta_url = url_from_rel_path(
        Path::new(rel_meta_path.rel().as_str()),
        download_server_base,
    )?;
    let meta_sha1 = hash_file(&meta_path).await?;

    Ok(InstanceManifestEntry {
        name: version_name.to_string(),
        url: meta_url,
        sha1: meta_sha1,
        versions: metadata_info,
    })
}
