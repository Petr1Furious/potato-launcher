use futures::stream::{self, StreamExt};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::{fs, io};
use url::Url;
use walkdir::WalkDir;

use crate::progress::ProgressTracker;

#[derive(thiserror::Error, Debug)]
pub enum GetFilesInDirError {
    #[error("failed to get files in dir: {0}")]
    WalkDir(#[from] walkdir::Error),
}

pub fn get_files_in_dir(path: &Path) -> Result<Vec<PathBuf>, GetFilesInDirError> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.is_dir() {
        return Ok(Vec::new());
    }

    Ok(WalkDir::new(path)
        .into_iter()
        .map(|entry| entry.map(|entry| entry.into_path()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| entry.is_file())
        .collect())
}

pub fn get_files_ignore_paths(
    path: &Path,
    ignore_paths: &HashSet<PathBuf>,
) -> io::Result<Vec<PathBuf>> {
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

pub async fn hash_file(path: &Path) -> io::Result<String> {
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
    progress_bar: impl ProgressTracker,
) -> Result<Vec<String>, HashFilesError>
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
                return Err(e.into());
            }
        }
    }

    progress_bar.finish();

    Ok(results
        .into_iter()
        .map(|value| value.expect("hash result missing"))
        .collect())
}

#[derive(thiserror::Error, Debug)]
pub enum HashFilesError {
    #[error("failed while hashing files: {0}")]
    Io(#[from] io::Error),
}

pub async fn remove_file_or_dir(path: &Path) -> io::Result<()> {
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
    /// If not set, the file will never be redownloaded if it already exists
    pub remote_sha1: Option<String>,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct DownloadTask {
    pub url: Url,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct CopyTask {
    pub source: PathBuf,
    pub target: PathBuf,
}

/// Deduplicate check tasks by destination path while preserving order.
pub fn dedup_check_tasks(tasks: Vec<CheckTask>) -> Vec<CheckTask> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(tasks.len());
    for task in tasks {
        if seen.insert(task.path.clone()) {
            deduped.push(task);
        }
    }
    deduped
}

/// Deduplicate copy tasks by destination path while preserving order.
pub fn dedup_copy_tasks(tasks: Vec<CopyTask>) -> Vec<CopyTask> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(tasks.len());
    for task in tasks {
        if seen.insert(task.target.clone()) {
            deduped.push(task);
        }
    }
    deduped
}

fn temp_path_for(target_path: &Path) -> PathBuf {
    let mut tmp_path = target_path.as_os_str().to_owned();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    tmp_path.push(format!(".tmp.{}.{}", std::process::id(), nonce));
    PathBuf::from(tmp_path)
}

async fn atomic_replace_file(tmp_path: &Path, target_path: &Path) -> io::Result<()> {
    match fs::remove_file(target_path).await {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    fs::rename(tmp_path, target_path).await
}

pub async fn get_download_task(check_task: &CheckTask) -> io::Result<Option<DownloadTask>> {
    match fs::metadata(&check_task.path).await {
        Ok(metadata) if !metadata.is_file() => {
            return Ok(Some(DownloadTask {
                url: check_task.url.clone(),
                path: check_task.path.clone(),
            }));
        }
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(Some(DownloadTask {
                url: check_task.url.clone(),
                path: check_task.path.clone(),
            }));
        }
        Err(err) => return Err(err),
    }

    if let Some(remote_sha1) = &check_task.remote_sha1 {
        let local_sha1 = hash_file(&check_task.path).await?;
        if &local_sha1 != remote_sha1 {
            return Ok(Some(DownloadTask {
                url: check_task.url.clone(),
                path: check_task.path.clone(),
            }));
        }
    }

    Ok(None)
}

#[derive(thiserror::Error, Debug)]
pub enum CheckTasksError {
    #[error("Hash of file {0} is missing")]
    HashMissing(PathBuf),
}

#[derive(thiserror::Error, Debug)]
pub enum GetDownloadTasksError {
    #[error("failed while reading local files for download checks: {0}")]
    Io(#[from] io::Error),
    #[error("failed while hashing local files for download checks: {0}")]
    HashFiles(#[from] HashFilesError),
    #[error("download check state is invalid: {0}")]
    CheckTasks(#[from] CheckTasksError),
}

pub async fn get_download_tasks(
    check_tasks: Vec<CheckTask>,
    progress_bar: impl ProgressTracker,
) -> Result<Vec<DownloadTask>, GetDownloadTasksError> {
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

    let hashes = hash_files(&to_hash, progress_bar).await?;
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
                } else if let Some(remote_sha1) = &task.remote_sha1
                    && remote_sha1
                        != hashes
                            .get(&path)
                            .ok_or(CheckTasksError::HashMissing(path.clone()))?
                {
                    need_download = true;
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

#[derive(thiserror::Error, Debug)]
pub enum DownloadFileError {
    #[error("network request failed while downloading file: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("file I/O failed while downloading file: {0}")]
    Io(#[from] io::Error),
}

pub async fn download_file(
    client: &reqwest::Client,
    task: &DownloadTask,
) -> Result<(), DownloadFileError> {
    if let Some(parent) = task.path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let tmp_path = temp_path_for(&task.path);
    let response = client
        .get(task.url.as_str())
        .send()
        .await?
        .error_for_status()?;
    let mut file = fs::File::create(&tmp_path).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => file.write_all(&chunk).await?,
            Err(err) => {
                let _ = fs::remove_file(&tmp_path).await;
                return Err(err.into());
            }
        }
    }
    file.flush().await?;
    drop(file);
    atomic_replace_file(&tmp_path, &task.path).await?;
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum DownloadFileParsedError {
    #[error("network request failed while fetching JSON file: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to parse downloaded JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub async fn download_file_parsed<T>(
    client: &reqwest::Client,
    task: &DownloadTask,
) -> Result<T, DownloadFileParsedError>
where
    T: serde::de::DeserializeOwned,
{
    let response = client.get(task.url.as_str()).send().await?;
    let bytes = response.bytes().await?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn download_files(
    client: &reqwest::Client,
    download_tasks: Vec<DownloadTask>,
    progress_bar: impl ProgressTracker,
) -> Result<(), DownloadFilesError> {
    progress_bar.set_length(download_tasks.len() as u64);

    const MAX_CONCURRENT_TASKS: usize = 8;

    let mut tasks = stream::iter(download_tasks.into_iter().map(|task| {
        let client = client.clone();
        async move { download_file(&client, &task).await }
    }))
    .buffer_unordered(MAX_CONCURRENT_TASKS);

    while let Some(result) = tasks.next().await {
        match result {
            Ok(()) => {
                progress_bar.inc(1);
            }
            Err(e) => {
                progress_bar.finish();
                return Err(e.into());
            }
        }
    }

    progress_bar.finish();
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum DownloadFilesError {
    #[error("one or more file downloads failed: {0}")]
    DownloadFile(#[from] DownloadFileError),
}

pub async fn copy_files_if_different(
    copy_tasks: Vec<CopyTask>,
    progress_bar: impl ProgressTracker,
) -> Result<(), CopyFilesError> {
    progress_bar.set_length(copy_tasks.len() as u64);

    const MAX_CONCURRENT_TASKS: usize = 8;

    let mut tasks = stream::iter(
        copy_tasks
            .into_iter()
            .map(|task| async move { copy_file_if_different(&task.source, &task.target).await }),
    )
    .buffer_unordered(MAX_CONCURRENT_TASKS);

    while let Some(result) = tasks.next().await {
        match result {
            Ok(_) => {
                progress_bar.inc(1);
            }
            Err(e) => {
                progress_bar.finish();
                return Err(e.into());
            }
        }
    }

    progress_bar.finish();
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum CopyFilesError {
    #[error("one or more file copy operations failed: {0}")]
    CopyFile(#[from] CopyFileError),
}

#[derive(thiserror::Error, Debug)]
pub enum ReadFileParsedError {
    #[error("failed to read JSON file from disk: {0}")]
    Io(#[from] io::Error),
    #[error("failed to parse JSON file from disk: {0}")]
    Json(#[from] serde_json::Error),
}

pub async fn read_file_parsed<T>(path: &Path) -> Result<T, ReadFileParsedError>
where
    T: serde::de::DeserializeOwned,
{
    let bytes = fs::read(path).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(thiserror::Error, Debug)]
pub enum WriteFileJsonError {
    #[error("failed to write JSON file to disk: {0}")]
    Io(#[from] io::Error),
    #[error("failed to serialize JSON payload: {0}")]
    Json(#[from] serde_json::Error),
}

pub async fn write_file_json<T>(path: &Path, value: &T) -> Result<(), WriteFileJsonError>
where
    T: serde::Serialize,
{
    let content = serde_json::to_string(value)?;
    fs::write(path, content).await?;
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum CopyFileError {
    #[error("Source path is not a file: {0}")]
    SourceNotFile(PathBuf),
    #[error("file I/O failed while copying file: {0}")]
    Io(#[from] io::Error),
}

/// Compare two files by size+sha1 and atomically replace target from source when different.
/// Returns true when replacement happened.
/// This function will return an error if target is a directory.
pub async fn copy_file_if_different(source: &Path, target: &Path) -> Result<bool, CopyFileError> {
    let source_meta = fs::metadata(source).await?;
    if !source_meta.is_file() {
        return Err(CopyFileError::SourceNotFile(source.to_path_buf()));
    }

    let same = match fs::metadata(target).await {
        Ok(target_meta) => {
            if !target_meta.is_file() || source_meta.len() != target_meta.len() {
                false
            } else {
                let mut source_file = fs::File::open(source).await?;
                let mut target_file = fs::File::open(target).await?;
                let mut source_buf = [0u8; 64 * 1024];
                let mut target_buf = [0u8; 64 * 1024];

                loop {
                    let source_n = source_file.read(&mut source_buf).await?;
                    let target_n = target_file.read(&mut target_buf).await?;
                    if source_n != target_n {
                        break false;
                    }
                    if source_n == 0 {
                        break true;
                    }
                    if source_buf[..source_n] != target_buf[..target_n] {
                        break false;
                    }
                }
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };
    if same {
        return Ok(false);
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).await?;
    }
    let tmp_path = temp_path_for(target);
    fs::copy(source, &tmp_path).await?;
    atomic_replace_file(&tmp_path, target).await?;
    Ok(true)
}

#[derive(thiserror::Error, Debug)]
pub enum RetainPathsError {
    #[error("Target path is not a directory: {0}")]
    TargetNotDirectory(PathBuf),
    #[error("Path is outside target dir: {0}")]
    PathOutsideTargetDir(PathBuf),
}

#[derive(thiserror::Error, Debug)]
pub enum RetainOnlyFilesError {
    #[error("invalid retain paths: {0}")]
    RetainPaths(#[from] RetainPathsError),
    #[error("failed while traversing target directory: {0}")]
    WalkDir(#[from] walkdir::Error),
    #[error("failed to resolve relative path under target directory: {0}")]
    StripPrefix(#[from] std::path::StripPrefixError),
    #[error("file I/O failed while retaining files: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy)]
pub struct RetainStats {
    pub removed_files: usize,
    pub removed_dirs: usize,
    pub keep_files: usize,
}

/// Remove every file under target_dir not present in `keep_files`.
/// Then remove directories that are not parents of any kept file.
pub async fn retain_only_files_and_parents(
    target_dir: &Path,
    keep_files: &HashSet<PathBuf>,
) -> Result<RetainStats, RetainOnlyFilesError> {
    if !target_dir.exists() {
        return Ok(RetainStats {
            removed_files: 0,
            removed_dirs: 0,
            keep_files: keep_files.len(),
        });
    }
    if !target_dir.is_dir() {
        return Err(RetainPathsError::TargetNotDirectory(target_dir.to_path_buf()).into());
    }

    let mut keep_rel_files = HashSet::with_capacity(keep_files.len());
    let mut keep_rel_dirs = HashSet::new();
    keep_rel_dirs.insert(PathBuf::new());
    let mut removed_files = 0usize;
    let mut removed_dirs = 0usize;

    for keep_file in keep_files {
        let rel = keep_file
            .strip_prefix(target_dir)
            .map_err(|_| RetainPathsError::PathOutsideTargetDir(keep_file.clone()))?
            .to_path_buf();
        keep_rel_files.insert(rel.clone());

        if let Some(parent) = rel.parent() {
            let mut cur = parent;
            loop {
                keep_rel_dirs.insert(cur.to_path_buf());
                if cur.as_os_str().is_empty() {
                    break;
                }
                match cur.parent() {
                    Some(next) => cur = next,
                    None => break,
                }
            }
        }
    }

    for entry in WalkDir::new(target_dir)
        .contents_first(true)
        .into_iter()
    {
        let entry = entry?;
        let path = entry.path();
        if path == target_dir {
            continue;
        }

        let rel = path.strip_prefix(target_dir)?.to_path_buf();
        if entry.file_type().is_dir() {
            if !keep_rel_dirs.contains(&rel) {
                match fs::remove_dir(path).await {
                    Ok(()) => {
                        removed_dirs += 1;
                    }
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => return Err(err.into()),
                }
            }
        } else if !keep_rel_files.contains(&rel) {
            match fs::remove_file(path).await {
                Ok(()) => {
                    removed_files += 1;
                }
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    Ok(RetainStats {
        removed_files,
        removed_dirs,
        keep_files: keep_files.len(),
    })
}
