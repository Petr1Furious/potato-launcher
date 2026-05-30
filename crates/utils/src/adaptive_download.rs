use futures::stream::{FuturesUnordered, StreamExt};
use log::{debug, warn};
use reqwest::Client;
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::files::{self, DownloadTask};
use crate::progress::ProgressTracker;

const MAX_CONCURRENCY: usize = 50;
const MIN_CONCURRENCY: usize = 1;
const WINDOW_DURATION: Duration = Duration::from_secs(2);
const UPDATE_CONCURRENCY_EVERY: usize = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_TIMEOUTS_AT_MIN_CONCURRENCY: usize = 2;

struct DownloadRecord {
    timestamp: Instant,
    success: bool,
    latency_ms: u128,
}

struct SlidingWindow {
    window: VecDeque<DownloadRecord>,
    total: usize,
    successes: usize,
    sum_latency_successes: u128,
}

impl SlidingWindow {
    fn new() -> Self {
        Self {
            window: VecDeque::new(),
            total: 0,
            successes: 0,
            sum_latency_successes: 0,
        }
    }

    fn push(&mut self, success: bool, latency_ms: u128) {
        self.window.push_back(DownloadRecord {
            timestamp: Instant::now(),
            success,
            latency_ms,
        });

        self.total += 1;
        if success {
            self.successes += 1;
            self.sum_latency_successes += latency_ms;
        }
    }

    fn pop_expired(&mut self) {
        let now = Instant::now();
        while let Some(front) = self.window.front() {
            if now.duration_since(front.timestamp) > WINDOW_DURATION {
                let rec = self.window.pop_front().unwrap();
                self.total -= 1;
                if rec.success {
                    self.successes -= 1;
                    self.sum_latency_successes -= rec.latency_ms;
                }
            } else {
                break;
            }
        }
    }

    /// Insert the latest result, remove old ones, then compute success rate & average success latency.
    /// Success rate = successes / total
    /// Average latency = sum of successful latencies / successes (only for success).
    fn add_and_calculate(&mut self, success: bool, latency_ms: u128) -> (f64, f64) {
        self.push(success, latency_ms);
        self.pop_expired();

        if self.total == 0 {
            return (1.0, 0.0);
        }

        let success_rate = self.successes as f64 / self.total as f64;
        let avg_latency = if self.successes > 0 {
            self.sum_latency_successes as f64 / self.successes as f64
        } else {
            0.0
        };

        (success_rate, avg_latency)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DownloadFileError {
    #[error("network request failed while downloading {url}: {source}")]
    Reqwest { url: String, source: reqwest::Error },
    #[error("file I/O failed while downloading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("timed out while waiting for download chunk from {url}: {source}")]
    ChunkTimeout {
        url: String,
        source: tokio::time::error::Elapsed,
    },
    #[error("temporary file {path} does not exist after download")]
    MissingTempFile { path: String },
}

async fn download_file(client: &Client, task: &DownloadTask) -> Result<u128, DownloadFileError> {
    let start = Instant::now();

    let response = client
        .get(task.url.as_str())
        .send()
        .await
        .map_err(|source| DownloadFileError::Reqwest {
            url: task.url.to_string(),
            source,
        })?
        .error_for_status()
        .map_err(|source| DownloadFileError::Reqwest {
            url: task.url.to_string(),
            source,
        })?;
    let mut stream = response.bytes_stream();

    if let Some(parent_dir) = task.path.parent() {
        tokio::fs::create_dir_all(parent_dir)
            .await
            .map_err(|source| DownloadFileError::Io {
                path: parent_dir.display().to_string(),
                source,
            })?;
    }

    // write to a temporary file first
    let mut tmp_path = task.path.as_os_str().to_owned();
    tmp_path.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_path);

    {
        let mut file =
            tokio::fs::File::create(&tmp_path)
                .await
                .map_err(|source| DownloadFileError::Io {
                    path: tmp_path.display().to_string(),
                    source,
                })?;

        let per_chunk_timeout = REQUEST_TIMEOUT;
        while let Some(chunk_result) = tokio::time::timeout(per_chunk_timeout, stream.next())
            .await
            .map_err(|source| DownloadFileError::ChunkTimeout {
                url: task.url.to_string(),
                source,
            })?
        {
            let chunk = chunk_result.map_err(|source| DownloadFileError::Reqwest {
                url: task.url.to_string(),
                source,
            })?;
            file.write_all(&chunk)
                .await
                .map_err(|source| DownloadFileError::Io {
                    path: tmp_path.display().to_string(),
                    source,
                })?;
        }
        file.flush().await.map_err(|source| DownloadFileError::Io {
            path: tmp_path.display().to_string(),
            source,
        })?;
    }

    if !tmp_path.exists() {
        return Err(DownloadFileError::MissingTempFile {
            path: tmp_path.display().to_string(),
        });
    }

    // then atomically rename it to the target path
    if task.path.exists() {
        files::remove_file_or_dir(&task.path)
            .await
            .map_err(|source| DownloadFileError::Io {
                path: task.path.display().to_string(),
                source,
            })?;
    }

    tokio::fs::rename(&tmp_path, &task.path)
        .await
        .map_err(|source| DownloadFileError::Io {
            path: format!("{} -> {}", tmp_path.display(), task.path.display()),
            source,
        })?;

    let latency_ms = start.elapsed().as_millis();

    Ok(latency_ms)
}

fn is_transient_network_error(e: &DownloadFileError) -> bool {
    match e {
        DownloadFileError::ChunkTimeout { .. } => true,
        DownloadFileError::Reqwest { source, .. } => {
            let source_dbg = format!("{source:?}");
            source.is_timeout()
                || source.is_connect()
                || source.status().is_some_and(|s| s.as_u16() == 523)
                || source_dbg.contains("connection closed before message completed")
                || source_dbg.contains("peer closed connection without sending TLS close_notify")
                || source_dbg.contains("peer closed connection")
                || source_dbg.contains("connection closed")
                || source_dbg.contains("connection reset")
                || source_dbg.contains("connection aborted")
                || source_dbg.contains("broken pipe")
                || source_dbg.contains("SendRequest")
                || source_dbg.contains("connection error")
                || source_dbg.contains("Connection refused")
                || source_dbg.contains("Network is unreachable")
                || source_dbg.contains("Connection timed out")
        }
        _ => false,
    }
}

enum DownloadOutcome {
    Success(u128),
    TransientFailure(DownloadFileError),
}

async fn do_download(
    client: &Client,
    task: &DownloadTask,
) -> Result<DownloadOutcome, DownloadFileError> {
    let latency_ms = match download_file(client, task).await {
        Ok(r) => r,
        Err(e) => {
            if is_transient_network_error(&e) {
                debug!("Transient error downloading {}: {:?}", task.url, e);
                return Ok(DownloadOutcome::TransientFailure(e));
            } else {
                debug!("Error downloading {}: {:?}", task.url, e);
                return Err(e);
            }
        }
    };

    Ok(DownloadOutcome::Success(latency_ms))
}

#[derive(thiserror::Error, Debug)]
pub enum AdaptiveDownloadError {
    #[error("connection timed out after retries; latest error: {source}")]
    ConnectionTimeout { source: DownloadFileError },
    #[error("connection timed out after retries")]
    ConnectionTimeoutWithoutSource,
    #[error("failed to build HTTP client for adaptive downloader: {0}")]
    ClientBuild(reqwest::Error),
    #[error("download failed: {0}")]
    Download(DownloadFileError),
}

pub async fn download_files(
    download_tasks: Vec<DownloadTask>,
    progress_bar: impl ProgressTracker,
) -> Result<(), AdaptiveDownloadError> {
    progress_bar.set_length(download_tasks.len() as u64);

    let client = Client::builder()
        .connect_timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(AdaptiveDownloadError::ClientBuild)?;

    let desired_concurrency = Arc::new(AtomicUsize::new(4));

    let sliding_window = Arc::new(Mutex::new(SlidingWindow::new()));

    let mut cur_tasks = download_tasks;
    let mut active = FuturesUnordered::new();
    let mut last_transient_error = None;

    fn can_spawn_more(active_count: usize, concurrency: &Arc<AtomicUsize>) -> bool {
        active_count < concurrency.load(Ordering::SeqCst)
    }

    let spawn_if_possible = |active: &mut FuturesUnordered<_>, cur_tasks: &mut Vec<_>| {
        while can_spawn_more(active.len(), &desired_concurrency) {
            if let Some(task) = cur_tasks.pop() {
                let fut = async {
                    let result = do_download(&client, &task).await;
                    (result, task)
                };
                active.push(fut);
            } else {
                break;
            }
        }
    };

    spawn_if_possible(&mut active, &mut cur_tasks);

    let mut timeouts_at_min_concurrency = 0;

    let mut next_concurrency_update = UPDATE_CONCURRENCY_EVERY;
    loop {
        let Some((result, task)) = active.next().await else {
            break;
        };

        let (success, latency_ms) = match result {
            Ok(DownloadOutcome::Success(latency_ms)) => {
                progress_bar.inc(1);
                (true, latency_ms)
            }
            Ok(DownloadOutcome::TransientFailure(error)) => {
                last_transient_error = Some(error);
                cur_tasks.push(task);
                (false, 0)
            }
            Err(e) => {
                return Err(AdaptiveDownloadError::Download(e));
            }
        };

        let (success_rate, avg_latency) = {
            let mut guard = sliding_window.lock().await;
            guard.add_and_calculate(success, latency_ms)
        };

        let current = desired_concurrency.load(Ordering::SeqCst);
        next_concurrency_update -= 1;
        if next_concurrency_update == 0 {
            next_concurrency_update = UPDATE_CONCURRENCY_EVERY;
            let mut new_value = current;
            if success {
                if success_rate > 0.9 && avg_latency < 2000.0 {
                    new_value = (current + 1).min(MAX_CONCURRENCY);
                }
            } else {
                if current == MIN_CONCURRENCY {
                    timeouts_at_min_concurrency += 1;
                    if timeouts_at_min_concurrency >= MAX_TIMEOUTS_AT_MIN_CONCURRENCY {
                        if let Some(source) = last_transient_error {
                            return Err(AdaptiveDownloadError::ConnectionTimeout { source });
                        }
                        return Err(AdaptiveDownloadError::ConnectionTimeoutWithoutSource);
                    }
                    warn!("Timeouts at min concurrency: {timeouts_at_min_concurrency}");
                }
                new_value = (current - current.div_ceil(4)).max(MIN_CONCURRENCY);
            }

            if new_value != current {
                if new_value == MIN_CONCURRENCY {
                    timeouts_at_min_concurrency = 0;
                }
                desired_concurrency.store(new_value, Ordering::SeqCst);
                debug!("New concurrency: {new_value}");
            }
        }

        spawn_if_possible(&mut active, &mut cur_tasks);
    }

    Ok(())
}
