use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use futures::stream::{self, StreamExt};
use instance::java_runtime::JavaRuntime;
use log::info;
use reqwest::Client;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use utils::{
    java::{JavaPlatformTarget, MIRROR_PLATFORM_TARGETS, query_zulu_package},
    paths::{BaseUrl, DataDir, JavaMirrorDir},
};

const MAX_CONCURRENT_JAVA_TASKS: usize = 8;

pub struct MirrorJavaRuntimesResult {
    pub runtimes_by_version: HashMap<String, Vec<JavaRuntime>>,
    pub archive_paths: Vec<PathBuf>,
}

struct MirrorJavaTaskOutcome {
    version: String,
    runtime: Option<JavaRuntime>,
    archive_path: Option<PathBuf>,
    downloaded: bool,
    skipped: bool,
}

async fn mirror_java_task(
    client: Client,
    output_dir: DataDir,
    base_url: BaseUrl,
    version: String,
    target: JavaPlatformTarget,
) -> anyhow::Result<MirrorJavaTaskOutcome> {
    let Some(package) = query_zulu_package(&client, &version, &target).await? else {
        log::warn!(
            "No Zulu JRE package found for Java {version} on {} {}",
            target.launcher_os,
            target.arch
        );
        return Ok(MirrorJavaTaskOutcome {
            version,
            runtime: None,
            archive_path: None,
            downloaded: false,
            skipped: false,
        });
    };

    let archive_path = JavaMirrorDir::root()
        .version_dir(&version)
        .archive_path(&package.name)
        .to_fs(&output_dir);

    let (downloaded, skipped) = if archive_path.exists() {
        info!(
            "Skipping Java {version} {} {} (already have {})",
            target.launcher_os, target.arch, package.name
        );
        (false, true)
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
        let mut response_stream = response.bytes_stream();
        while let Some(chunk) = response_stream.next().await {
            file.write_all(&chunk?).await?;
        }
        (true, false)
    };

    let url = JavaMirrorDir::root()
        .version_dir(&version)
        .archive_path(&package.name)
        .to_url(&base_url);

    Ok(MirrorJavaTaskOutcome {
        version: version.clone(),
        runtime: Some(JavaRuntime {
            name: package.name,
            version,
            os: target.launcher_os.to_string(),
            arch: target.arch.to_string(),
            archive_type: target.archive_type.to_string(),
            url,
        }),
        archive_path: Some(archive_path),
        downloaded,
        skipped,
    })
}

pub async fn mirror_java_runtimes(
    client: &Client,
    output_dir: &DataDir,
    base_url: &BaseUrl,
    java_versions: &HashSet<String>,
) -> anyhow::Result<MirrorJavaRuntimesResult> {
    let tasks = java_versions
        .iter()
        .flat_map(|version| {
            MIRROR_PLATFORM_TARGETS
                .iter()
                .copied()
                .map(move |target| (version.clone(), target))
        })
        .collect::<Vec<_>>();

    let output_dir = output_dir.clone();
    let base_url = base_url.clone();

    let mut downloaded = 0u32;
    let mut skipped = 0u32;
    let mut runtimes_by_version: HashMap<String, Vec<JavaRuntime>> = HashMap::new();
    let mut archive_paths = Vec::new();

    let mut task_stream = stream::iter(tasks.into_iter().map(|(version, target)| {
        let client = client.clone();
        let output_dir = output_dir.clone();
        let base_url = base_url.clone();
        async move { mirror_java_task(client, output_dir, base_url, version, target).await }
    }))
    .buffer_unordered(MAX_CONCURRENT_JAVA_TASKS);

    while let Some(outcome) = task_stream.next().await {
        let outcome = outcome?;
        if outcome.downloaded {
            downloaded += 1;
        }
        if outcome.skipped {
            skipped += 1;
        }
        if let Some(path) = outcome.archive_path {
            archive_paths.push(path);
        }
        if let Some(runtime) = outcome.runtime {
            runtimes_by_version
                .entry(outcome.version)
                .or_default()
                .push(runtime);
        }
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
