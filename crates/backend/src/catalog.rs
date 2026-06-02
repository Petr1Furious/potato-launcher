use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use instance::manifest::{InstanceManifest, ManifestError};
use launcher_bridge::{BackendFetchState, BackendStatus};
use sha1::{Digest, Sha1};
use url::Url;
use utils::files;

pub const CATALOG_CACHE_DIR: &str = "catalog_cache";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendFetchStatus {
    NotFetched,
    Fetching,
    Ok,
    Offline,
    Error,
}

#[derive(Clone)]
pub struct BackendCatalogEntry {
    pub manifest: Option<Arc<InstanceManifest>>,
    pub status: BackendFetchStatus,
    pub error: Option<Arc<str>>,
}

impl BackendCatalogEntry {
    pub fn new_not_fetched() -> Self {
        Self {
            manifest: None,
            status: BackendFetchStatus::NotFetched,
            error: None,
        }
    }

    pub fn from_cache(manifest: Arc<InstanceManifest>) -> Self {
        Self {
            manifest: Some(manifest),
            status: BackendFetchStatus::NotFetched,
            error: None,
        }
    }

    pub fn with_manifest(manifest: InstanceManifest, status: BackendFetchStatus) -> Self {
        Self {
            manifest: Some(Arc::new(manifest)),
            status,
            error: None,
        }
    }

    pub fn manifest(&self) -> Option<&Arc<InstanceManifest>> {
        self.manifest.as_ref()
    }

    pub fn to_fetch_state(&self) -> BackendFetchState {
        match self.status {
            BackendFetchStatus::NotFetched => BackendFetchState::NotFetched,
            BackendFetchStatus::Fetching => BackendFetchState::Fetching,
            BackendFetchStatus::Ok => BackendFetchState::Fetched {
                instance_count: self
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.instances.len())
                    .unwrap_or(0),
            },
            BackendFetchStatus::Offline => BackendFetchState::Offline,
            BackendFetchStatus::Error => BackendFetchState::Error(
                self.error
                    .clone()
                    .unwrap_or_else(|| Arc::from("unknown catalog error")),
            ),
        }
    }

    pub fn set_fetching(&mut self) {
        self.status = BackendFetchStatus::Fetching;
    }

    pub fn apply_fetch_success(&mut self, manifest: Arc<InstanceManifest>) {
        self.manifest = Some(manifest);
        self.status = BackendFetchStatus::Ok;
        self.error = None;
    }

    pub fn apply_fetch_failure(&mut self, failure: FetchFailure) {
        match failure {
            FetchFailure::Offline => {
                self.status = BackendFetchStatus::Offline;
                self.error = None;
            }
            FetchFailure::Error(message) => {
                self.status = BackendFetchStatus::Error;
                self.error = Some(message);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchFailure {
    Offline,
    Error(Arc<str>),
}

pub enum CatalogFetchResult {
    Success(InstanceManifest),
    Failed(FetchFailure),
}

fn catalog_cache_dir(launcher_dir: &Path) -> PathBuf {
    launcher_dir.join(CATALOG_CACHE_DIR)
}

fn catalog_cache_path(launcher_dir: &Path, url: &Url) -> PathBuf {
    let mut hasher = Sha1::new();
    hasher.update(url.as_str().as_bytes());
    let hash = hasher.finalize();
    let mut filename = String::with_capacity(40);
    for byte in hash {
        use std::fmt::Write as _;
        let _ = write!(filename, "{byte:02x}");
    }
    catalog_cache_dir(launcher_dir).join(format!("{filename}.json"))
}

pub async fn load_cached_manifest(
    launcher_dir: &Path,
    url: &Url,
) -> Result<InstanceManifest, ManifestError> {
    InstanceManifest::load_from_file(&catalog_cache_path(launcher_dir, url)).await
}

pub async fn save_cached_manifest(
    launcher_dir: &Path,
    url: &Url,
    manifest: &InstanceManifest,
) -> Result<(), ManifestError> {
    let cache_dir = catalog_cache_dir(launcher_dir);
    if let Err(err) = tokio::fs::create_dir_all(&cache_dir).await {
        return Err(ManifestError::WriteFileJson(files::WriteFileJsonError::Io(
            err,
        )));
    }
    manifest
        .save_to_file(&catalog_cache_path(launcher_dir, url))
        .await
}

pub async fn delete_cached_manifest(launcher_dir: &Path, url: &Url) -> std::io::Result<()> {
    let path = catalog_cache_path(launcher_dir, url);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub async fn fetch_backend_catalog(client: reqwest::Client, url: Url) -> CatalogFetchResult {
    log::info!("Fetching backend manifest from {url}");
    match InstanceManifest::fetch(&client, &url).await {
        Ok(manifest) => {
            log::info!(
                "Fetched backend manifest from {url}: {} published instances",
                manifest.instances.len()
            );
            CatalogFetchResult::Success(manifest)
        }
        Err(error) => {
            let detailed_error = format!("{error:?}");
            let failure = classify_manifest_error(error);
            match &failure {
                FetchFailure::Offline => {
                    log::warn!(
                        "Backend manifest at {url} is offline or timed out: {detailed_error}"
                    );
                }
                FetchFailure::Error(message) => {
                    log::error!(
                        "Failed to fetch backend manifest at {url}: {message}; details: {detailed_error}"
                    );
                }
            }
            CatalogFetchResult::Failed(failure)
        }
    }
}

pub fn classify_manifest_error(error: ManifestError) -> FetchFailure {
    match &error {
        ManifestError::Reqwest(err) if err.is_connect() || err.is_timeout() => {
            FetchFailure::Offline
        }
        _ => FetchFailure::Error(Arc::<str>::from(error.to_string())),
    }
}

pub fn backend_status(
    url: &Url,
    entry: &BackendCatalogEntry,
    configured: bool,
    referenced_by_instances: bool,
) -> BackendStatus {
    BackendStatus {
        url: url.clone(),
        fetch_state: entry.to_fetch_state(),
        configured,
        referenced_by_instances,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn fetch_backend_catalog_preserves_success_and_failure_states() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0_u8; 1024];
                    let read = stream.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..read]);
                    let (status, body) = if request.starts_with("GET /ok ") {
                        (
                            "HTTP/1.1 200 OK",
                            r#"{"instances":[{"name":"Vanilla","url":"http://127.0.0.1/meta.json","sha1":"abc","required_java_version":"21"}]}"#,
                        )
                    } else {
                        ("HTTP/1.1 500 Internal Server Error", r#"{"error":"boom"}"#)
                    };
                    let response = format!(
                        "{status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });

        let client = reqwest::Client::new();
        let ok_url = Url::parse(&format!("http://{addr}/ok")).unwrap();
        let fail_url = Url::parse(&format!("http://{addr}/fail")).unwrap();

        let ok = fetch_backend_catalog(client.clone(), ok_url).await;
        let fail = fetch_backend_catalog(client, fail_url).await;

        assert!(matches!(
            ok,
            CatalogFetchResult::Success(manifest) if manifest.instances.len() == 1
        ));
        assert!(matches!(
            fail,
            CatalogFetchResult::Failed(FetchFailure::Error(_))
        ));
    }
}
