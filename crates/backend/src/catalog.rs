use std::sync::Arc;

use instance::manifest::{InstanceManifest, ManifestError};
use launcher_bridge::{BackendFetchState, BackendStatus};
use url::Url;

#[derive(Clone)]
pub enum BackendCatalogState {
    NotFetched,
    Fetching,
    Fetched(Arc<InstanceManifest>),
    Offline,
    Error(Arc<str>),
}

impl BackendCatalogState {
    pub fn to_fetch_state(&self) -> BackendFetchState {
        match self {
            Self::NotFetched => BackendFetchState::NotFetched,
            Self::Fetching => BackendFetchState::Fetching,
            Self::Fetched(manifest) => BackendFetchState::Fetched {
                instance_count: manifest.instances.len(),
            },
            Self::Offline => BackendFetchState::Offline,
            Self::Error(error) => BackendFetchState::Error(error.clone()),
        }
    }
}

pub async fn fetch_backend_catalog(client: reqwest::Client, url: Url) -> BackendCatalogState {
    log::info!("Fetching backend manifest from {url}");
    match InstanceManifest::fetch(&client, &url).await {
        Ok(manifest) => {
            log::info!(
                "Fetched backend manifest from {url}: {} published instances",
                manifest.instances.len()
            );
            BackendCatalogState::Fetched(Arc::new(manifest))
        }
        Err(error) => {
            let detailed_error = format!("{error:?}");
            let state = classify_manifest_error(error);
            match &state {
                BackendCatalogState::Offline => {
                    log::warn!(
                        "Backend manifest at {url} is offline or timed out: {detailed_error}"
                    );
                }
                BackendCatalogState::Error(message) => {
                    log::error!(
                        "Failed to fetch backend manifest at {url}: {message}; details: {detailed_error}"
                    );
                }
                _ => {}
            }
            state
        }
    }
}

pub fn classify_manifest_error(error: ManifestError) -> BackendCatalogState {
    match &error {
        ManifestError::Reqwest(err) if err.is_connect() || err.is_timeout() => {
            BackendCatalogState::Offline
        }
        _ => BackendCatalogState::Error(Arc::<str>::from(error.to_string())),
    }
}

pub fn backend_status(
    url: &Url,
    state: &BackendCatalogState,
    configured: bool,
    referenced_by_instances: bool,
) -> BackendStatus {
    BackendStatus {
        url: url.clone(),
        fetch_state: state.to_fetch_state(),
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
                            r#"{"instances":[{"name":"Vanilla","url":"http://127.0.0.1/meta.json","sha1":"abc","versions":[]}]}"#,
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

        assert!(
            matches!(ok, BackendCatalogState::Fetched(manifest) if manifest.instances.len() == 1)
        );
        assert!(matches!(fail, BackendCatalogState::Error(_)));
    }
}
