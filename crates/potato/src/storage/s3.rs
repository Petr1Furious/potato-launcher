use std::path::{Path, PathBuf};

use anyhow::Context;
use async_trait::async_trait;
use futures::TryStreamExt;
use object_store::{
    ObjectStore, PutMode, PutOptions, PutPayload, aws::AmazonS3Builder, path::Path as StorePath,
};

use super::{LOCK_KEY, Listed, Storage, locked_error};

/// `s3://bucket/prefix`. Credentials, region and endpoint come from the usual
/// `AWS_*` environment variables.
pub struct S3Storage {
    store: Box<dyn ObjectStore>,
    prefix: String,
}

impl S3Storage {
    pub fn new(url: &url::Url) -> anyhow::Result<Self> {
        let bucket = url.host_str().context("s3 target needs a bucket")?;
        let store = AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .with_conditional_put(object_store::aws::S3ConditionalPut::ETagMatch)
            .build()
            .context("can't set up s3")?;
        Ok(Self {
            store: Box::new(store),
            prefix: url.path().trim_matches('/').to_string(),
        })
    }

    fn path(&self, key: &str) -> StorePath {
        if self.prefix.is_empty() {
            StorePath::from(key)
        } else {
            StorePath::from(format!("{}/{key}", self.prefix))
        }
    }

    fn key(&self, path: &StorePath) -> String {
        let full = path.as_ref();
        if self.prefix.is_empty() {
            full.to_string()
        } else {
            full.strip_prefix(&format!("{}/", self.prefix))
                .unwrap_or(full)
                .to_string()
        }
    }
}

#[async_trait]
impl Storage for S3Storage {
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match self.store.get(&self.path(key)).await {
            Ok(result) => Ok(Some(result.bytes().await?.to_vec())),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn put(&self, key: &str, data: Vec<u8>) -> anyhow::Result<()> {
        self.store
            .put(&self.path(key), PutPayload::from(data))
            .await?;
        Ok(())
    }

    async fn put_path(&self, key: &str, path: &Path, _executable: bool) -> anyhow::Result<()> {
        self.put(key, tokio::fs::read(path).await?).await
    }

    async fn put_objects(&self, files: &[(String, PathBuf)]) -> anyhow::Result<()> {
        for (key, source) in files {
            match self.store.head(&self.path(key)).await {
                Ok(_) => continue,
                Err(object_store::Error::NotFound { .. }) => {}
                Err(err) => return Err(err.into()),
            }
            self.put(key, tokio::fs::read(source).await?).await?;
        }
        Ok(())
    }

    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<Listed>> {
        let start = self.path(prefix.trim_end_matches('/'));
        let objects: Vec<_> = self.store.list(Some(&start)).try_collect().await?;
        let mut listed = objects
            .into_iter()
            .map(|meta| Listed {
                key: self.key(&meta.location),
                modified: meta.last_modified.timestamp(),
            })
            .filter(|listed| listed.key.starts_with(prefix))
            .collect::<Vec<_>>();
        listed.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(listed)
    }

    async fn delete(&self, keys: &[String]) -> anyhow::Result<()> {
        for key in keys {
            match self.store.delete(&self.path(key)).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    async fn lock(&self) -> anyhow::Result<()> {
        let options = PutOptions {
            mode: PutMode::Create,
            ..Default::default()
        };
        let payload = PutPayload::from(utils_now().to_string().into_bytes());
        match self
            .store
            .put_opts(&self.path(LOCK_KEY), payload, options)
            .await
        {
            Ok(_) => Ok(()),
            Err(object_store::Error::AlreadyExists { .. }) => Err(locked_error().context(format!(
                "if nothing is running, delete {LOCK_KEY} from the bucket"
            ))),
            Err(err) => Err(err.into()),
        }
    }

    async fn unlock(&self) -> anyhow::Result<()> {
        self.delete(&[LOCK_KEY.to_string()]).await
    }
}

fn utils_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
