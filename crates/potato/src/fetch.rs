use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use url::Url;
use utils::files::DownloadTask;

use crate::progress::TerminalProgress;

pub struct FileCache {
    root: PathBuf,
}

impl FileCache {
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            root: cache_dir.join("files"),
        }
    }

    pub fn path(&self, sha1: &str) -> PathBuf {
        self.root.join(&sha1[..2]).join(sha1)
    }

    pub async fn ensure(
        &self,
        wanted: &[(Url, String)],
    ) -> anyhow::Result<HashMap<String, PathBuf>> {
        let mut paths = HashMap::new();
        let mut tasks = vec![];
        let mut seen = std::collections::HashSet::new();
        for (url, sha1) in wanted {
            let path = self.path(sha1);
            if !path.exists() && seen.insert(sha1.clone()) {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                tasks.push(DownloadTask {
                    url: url.clone(),
                    path: path.clone(),
                    size: None,
                });
            }
            paths.insert(sha1.clone(), path);
        }
        if !tasks.is_empty() {
            let downloaded = tasks
                .iter()
                .map(|task| (task.path.clone(), task.url.clone()))
                .collect::<Vec<_>>();
            let progress = TerminalProgress::new().handle("Downloading");
            utils::adaptive_download::download_files(tasks, &self.root.join(".tmp"), progress)
                .await
                .context("download failed")?;
            for (path, url) in downloaded {
                let expected = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
                let actual = utils::files::hash_file(&path).await?;
                if actual != expected {
                    let _ = std::fs::remove_file(&path);
                    bail!("{url} doesn't match its sha1 (expected {expected}, got {actual})");
                }
            }
        }
        Ok(paths)
    }

    pub async fn fetch_unknown(
        &self,
        client: &reqwest::Client,
        url: &Url,
    ) -> anyhow::Result<(String, PathBuf)> {
        let bytes = client
            .get(url.clone())
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("can't download {url}"))?
            .bytes()
            .await?;
        let temp = tempfile::NamedTempFile::new_in({
            std::fs::create_dir_all(&self.root)?;
            &self.root
        })?;
        std::fs::write(temp.path(), &bytes)?;
        let sha1 = utils::files::hash_file(temp.path()).await?;
        let path = self.path(&sha1);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        temp.persist(&path)?;
        Ok((sha1, path))
    }

    pub async fn insert(&self, source: &Path, sha1: &str) -> anyhow::Result<PathBuf> {
        let path = self.path(sha1);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            tokio::fs::copy(source, &path).await?;
        }
        Ok(path)
    }
}
