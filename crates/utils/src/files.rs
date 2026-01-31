use futures::stream::{self, StreamExt};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt as _;
use tokio::{fs, io};
use url::Url;
use walkdir::WalkDir;

use crate::progress::ProgressTracker;

pub fn get_files_in_dir(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.is_dir() {
        return Ok(Vec::new());
    }

    Ok(WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().to_path_buf())
        .collect())
}

pub fn get_files_ignore_paths(
    path: &Path,
    ignore_paths: &HashSet<PathBuf>,
) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.is_dir() {
        return Ok(Vec::new());
    }

    Ok(WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().to_path_buf())
        .filter(|entry_path| !ignore_paths.contains(entry_path))
        .collect())
}

pub async fn hash_file(path: &Path) -> anyhow::Result<String> {
    let file = fs::File::open(path).await?;
    let mut reader = io::BufReader::new(file);
    let mut hasher = Sha1::new();
    let mut buffer = [0; 64 * 1024];

    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub async fn hash_files<P>(
    files: &[P],
    progress_bar: Arc<dyn ProgressTracker + Send + Sync>,
) -> anyhow::Result<Vec<String>>
where
    P: AsRef<Path>,
{
    let total_files = files.len() as u64;
    progress_bar.set_length(total_files);

    let max_concurrent_tasks = num_cpus::get();
    let mut results = vec![None; files.len()];
    let mut tasks = stream::iter(
        files
            .iter()
            .enumerate()
            .map(|(index, path)| async move { (index, hash_file(path.as_ref()).await) }),
    )
    .buffer_unordered(max_concurrent_tasks);

    while let Some((index, result)) = tasks.next().await {
        match result {
            Ok(value) => {
                progress_bar.inc(1);
                results[index] = Some(value);
            }
            Err(e) => {
                progress_bar.finish();
                return Err(e);
            }
        }
    }

    progress_bar.finish();

    Ok(results
        .into_iter()
        .map(|value| value.expect("hash result missing"))
        .collect())
}

pub async fn remove_file_or_dir(path: &Path) -> anyhow::Result<()> {
    if path.is_file() {
        fs::remove_file(path).await?;
    } else if path.is_dir() {
        fs::remove_dir_all(path).await?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct CheckTask {
    pub url: Url,
    pub remote_sha1: Option<String>,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct DownloadTask {
    pub url: Url,
    pub path: PathBuf,
}

#[derive(thiserror::Error, Debug)]
pub enum CheckTasksError {
    #[error("Hash of file {0} is missing")]
    HashMissing(PathBuf),
}

pub async fn get_download_tasks(
    check_tasks: Vec<CheckTask>,
    progress_bar: Arc<dyn ProgressTracker + Send + Sync>,
) -> anyhow::Result<Vec<DownloadTask>> {
    let mut to_hash = Vec::new();
    for task in &check_tasks {
        if task.remote_sha1.is_some() {
            match fs::metadata(&task.path).await {
                Ok(metadata) => {
                    if metadata.is_file() {
                        to_hash.push(task.path.clone());
                    }
                }
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    let hashes = hash_files(&to_hash, progress_bar.clone()).await?;
    let hashes = to_hash
        .into_iter()
        .zip(hashes.into_iter())
        .collect::<HashMap<_, _>>();

    let mut download_tasks = HashMap::new();
    for task in check_tasks {
        let path = task.path;
        let mut need_download = false;
        match fs::metadata(&path).await {
            Ok(metadata) => {
                if !metadata.is_file() {
                    need_download = true;
                } else if let Some(remote_sha1) = &task.remote_sha1 {
                    if remote_sha1
                        != hashes
                            .get(&path)
                            .ok_or(CheckTasksError::HashMissing(path.clone()))?
                    {
                        need_download = true;
                    }
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                need_download = true;
            }
            Err(err) => return Err(err.into()),
        }

        if need_download {
            download_tasks.insert(path, task.url);
        }
    }

    Ok(download_tasks
        .into_iter()
        .map(|(path, url)| DownloadTask { url, path })
        .collect())
}

pub async fn download_files(
    client: &reqwest::Client,
    download_tasks: Vec<DownloadTask>,
    progress_bar: Arc<dyn ProgressTracker + Send + Sync>,
) -> anyhow::Result<()> {
    progress_bar.set_length(download_tasks.len() as u64);

    const MAX_CONCURRENT_TASKS: usize = 8;

    let mut tasks = stream::iter(download_tasks.into_iter().map(|task| {
        let client = client.clone();
        async move {
            if let Some(parent) = task.path.parent() {
                fs::create_dir_all(parent).await?;
            }
            let bytes = client
                .get(task.url.clone())
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            fs::write(&task.path, &bytes).await?;
            Ok::<_, anyhow::Error>(())
        }
    }))
    .buffer_unordered(MAX_CONCURRENT_TASKS);

    while let Some(result) = tasks.next().await {
        match result {
            Ok(()) => {
                progress_bar.inc(1);
            }
            Err(e) => {
                progress_bar.finish();
                return Err(e);
            }
        }
    }

    progress_bar.finish();
    Ok(())
}
