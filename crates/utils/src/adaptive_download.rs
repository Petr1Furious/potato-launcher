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
use crate::utils::is_connect_error;

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

async fn download_file(client: &Client, task: &DownloadTask) -> anyhow::Result<u128> {
    let start = Instant::now();

    let response = client
        .get(task.url.as_str())
        .send()
        .await?
        .error_for_status()?;
    let mut stream = response.bytes_stream();

    if let Some(parent_dir) = task.path.parent() {
        tokio::fs::create_dir_all(parent_dir).await?;
    }

    // write to a temporary file first
    let mut tmp_path = task.path.as_os_str().to_owned();
    tmp_path.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_path);

    {
        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create temp file {:?}: {}", tmp_path, e))?;

        let per_chunk_timeout = REQUEST_TIMEOUT;
        while let Some(chunk_result) =
            tokio::time::timeout(per_chunk_timeout, stream.next()).await?
        {
            let chunk = chunk_result?;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
    }

    if !tmp_path.exists() {
        return Err(anyhow::anyhow!(
            "Temporary file {:?} does not exist after creation",
            tmp_path
        ));
    }

    // then atomically rename it to the target path
    if task.path.exists() {
        files::remove_file_or_dir(&task.path).await?;
    }

    tokio::fs::rename(&tmp_path, &task.path)
        .await
        .map_err(|e| {
            anyhow::anyhow!("Failed to rename {:?} to {:?}: {}", tmp_path, task.path, e)
        })?;

    let latency_ms = start.elapsed().as_millis();

    Ok(latency_ms)
}

fn is_timeout_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .is_some_and(|e| e.is_timeout())
        || e.downcast_ref::<tokio::time::error::Elapsed>().is_some()
        || format!("{e:?}").contains("connection closed before message completed")
    // reqwest doesn't let us check for this error directly
}

/// Download a single file, returning (success, latency_ms).
/// On success, we return Ok(Some(latency_ms)).
/// If it's a timeout, we return Ok(None). If it's another error, we return Err(e).
async fn do_download(client: &Client, task: &DownloadTask) -> anyhow::Result<Option<u128>> {
    let latency_ms = match download_file(client, task).await {
        Ok(r) => r,
        Err(e) => {
            // If it's a timeout, we return Ok(None), else Err
            if is_timeout_error(&e) || is_connect_error(&e) {
                debug!("Timeout downloading {}", task.url);
                return Ok(None);
            } else {
                debug!("Error downloading {}: {:?}", task.url, e);
                return Err(e);
            }
        }
    };

    Ok(Some(latency_ms))
}

#[derive(thiserror::Error, Debug)]
pub enum AdaptiveDownloadError {
    #[error("Connection timed out")]
    ConnectionTimeout,
}

pub async fn download_files(
    download_tasks: Vec<DownloadTask>,
    progress_bar: impl ProgressTracker,
) -> anyhow::Result<()> {
    progress_bar.set_length(download_tasks.len() as u64);

    let client = Client::builder().connect_timeout(REQUEST_TIMEOUT).build()?;

    let desired_concurrency = Arc::new(AtomicUsize::new(4));

    let sliding_window = Arc::new(Mutex::new(SlidingWindow::new()));

    let mut cur_tasks = download_tasks;
    let mut active = FuturesUnordered::new();

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
            Ok(Some(latency_ms)) => {
                progress_bar.inc(1);
                (true, latency_ms)
            }
            Ok(None) => {
                cur_tasks.push(task);
                (false, 0)
            }
            Err(e) => {
                return Err(e);
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
                        return Err(AdaptiveDownloadError::ConnectionTimeout.into());
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
