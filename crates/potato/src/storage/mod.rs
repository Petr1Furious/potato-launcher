mod file;
mod rsync;
mod s3;

use std::path::{Path, PathBuf};

use anyhow::bail;
use async_trait::async_trait;

pub const LOCK_KEY: &str = ".potato-lock";

#[async_trait]
pub trait Storage: Send + Sync {
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    async fn put(&self, key: &str, data: Vec<u8>) -> anyhow::Result<()>;
    async fn put_path(&self, key: &str, path: &Path, executable: bool) -> anyhow::Result<()>;
    /// Content-addressed, so an existing key can be skipped or rewritten.
    async fn put_objects(&self, files: &[(String, PathBuf)]) -> anyhow::Result<()>;
    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<String>>;
    async fn delete(&self, keys: &[String]) -> anyhow::Result<()>;
    async fn lock(&self) -> anyhow::Result<()>;
    async fn unlock(&self) -> anyhow::Result<()>;
}

pub fn open(target: &str) -> anyhow::Result<Box<dyn Storage>> {
    let url = url::Url::parse(target)
        .map_err(|err| anyhow::anyhow!("invalid storage target '{target}': {err}"))?;
    match url.scheme() {
        "file" => Ok(Box::new(file::FileStorage::new(url)?)),
        "rsync" => Ok(Box::new(rsync::RsyncStorage::new(&url)?)),
        "s3" => Ok(Box::new(s3::S3Storage::new(&url)?)),
        other => bail!("unsupported storage '{other}', use file://, rsync:// or s3://"),
    }
}

pub async fn locked<T>(
    storage: &dyn Storage,
    job: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    storage.lock().await?;
    // Ctrl-C would otherwise leave an S3 lock behind.
    let result = tokio::select! {
        result = job => result,
        _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("interrupted")),
    };
    let unlocked = storage.unlock().await;
    let value = result?;
    unlocked?;
    Ok(value)
}

fn locked_error() -> anyhow::Error {
    anyhow::anyhow!("storage is locked by another potato command")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_storage_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let url = url::Url::from_directory_path(dir.path()).unwrap();
        let storage = open(url.as_str()).unwrap();
        assert!(storage.get("a/b.json").await.unwrap().is_none());
        storage.put("a/b.json", b"1".to_vec()).await.unwrap();
        assert_eq!(storage.get("a/b.json").await.unwrap().unwrap(), b"1");

        let source = dir.path().join("src.bin");
        std::fs::write(&source, b"x").unwrap();
        storage
            .put_objects(&[("objects/ab/abc".into(), source.clone())])
            .await
            .unwrap();
        let listed = storage.list("objects/").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], "objects/ab/abc");

        storage.lock().await.unwrap();
        let second = open(url.as_str()).unwrap();
        assert!(second.lock().await.is_err());
        storage.unlock().await.unwrap();
        // A child process forked by a parallel test can hold the lock until it execs.
        let mut locked = second.lock().await;
        for _ in 0..50 {
            if locked.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            locked = second.lock().await;
        }
        locked.unwrap();
        second.unlock().await.unwrap();

        storage.delete(&["objects/ab/abc".into()]).await.unwrap();
        assert!(storage.list("objects/").await.unwrap().is_empty());
    }
}
