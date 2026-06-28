use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use futures::StreamExt;
use instance::java_runtime::JavaRuntime;
use log::info;
use reqwest::Client;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use utils::{
    java::{MIRROR_PLATFORM_TARGETS, query_zulu_package},
    paths::{BaseUrl, DataDir, JavaMirrorDir},
};

pub struct MirrorJavaRuntimesResult {
    pub runtimes_by_version: HashMap<String, Vec<JavaRuntime>>,
    pub archive_paths: Vec<PathBuf>,
}

pub async fn mirror_java_runtimes(
    client: &Client,
    output_dir: &DataDir,
    base_url: &BaseUrl,
    java_versions: &HashSet<String>,
) -> anyhow::Result<MirrorJavaRuntimesResult> {
    let mut runtimes_by_version = HashMap::new();
    let mut archive_paths = Vec::new();
    let mut downloaded = 0u32;
    let mut skipped = 0u32;

    for version in java_versions {
        let mut runtimes = Vec::new();
        for target in MIRROR_PLATFORM_TARGETS {
            let Some(package) = query_zulu_package(client, version, target).await? else {
                log::warn!(
                    "No Zulu JRE package found for Java {version} on {} {}",
                    target.launcher_os,
                    target.arch
                );
                continue;
            };

            let archive_path = JavaMirrorDir::root()
                .version_dir(version)
                .archive_path(&package.name)
                .to_fs(output_dir);
            archive_paths.push(archive_path.clone());

            if archive_path.exists() {
                info!(
                    "Skipping Java {version} {} {} (already have {})",
                    target.launcher_os, target.arch, package.name
                );
                skipped += 1;
            } else {
                if let Some(parent) = archive_path.parent() {
                    fs::create_dir_all(parent).await?;
                }
                info!(
                    "Downloading Java {version} {} {} ({})",
                    target.launcher_os, target.arch, package.name
                );
                let response = client.get(package.download_url.clone()).send().await?;
                let mut file = fs::File::create(&archive_path).await?;
                let mut stream = response.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    file.write_all(&chunk?).await?;
                }
                downloaded += 1;
            }

            let url = JavaMirrorDir::root()
                .version_dir(version)
                .archive_path(&package.name)
                .to_url(base_url);

            runtimes.push(JavaRuntime {
                name: package.name,
                version: version.clone(),
                os: target.launcher_os.to_string(),
                arch: target.arch.to_string(),
                archive_type: target.archive_type.to_string(),
                url,
            });
        }
        runtimes_by_version.insert(version.clone(), runtimes);
    }

    info!(
        "Java runtime mirroring complete: {} downloaded, {} skipped",
        downloaded, skipped
    );

    Ok(MirrorJavaRuntimesResult {
        runtimes_by_version,
        archive_paths,
    })
}
