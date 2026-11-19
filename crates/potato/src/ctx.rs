use std::path::{Path, PathBuf};

use crate::{
    config::{Config, LocalConfig, cache_dir},
    fetch::FileCache,
    git::Git,
    hashcache::HashCache,
};

pub struct Ctx {
    pub repo: PathBuf,
    pub git: Git,
    pub client: reqwest::Client,
    pub cache_dir: PathBuf,
    pub hashes: HashCache,
    pub files: FileCache,
    pub verbose: bool,
}

impl Ctx {
    pub fn new(repo: &Path) -> anyhow::Result<Self> {
        Ok(Self::with_cache(repo, cache_dir()?))
    }

    pub fn with_cache(repo: &Path, cache_dir: PathBuf) -> Self {
        Self {
            repo: repo.to_path_buf(),
            git: Git::new(repo),
            client: crate::http_client(),
            hashes: HashCache::load(&cache_dir),
            files: FileCache::new(&cache_dir),
            verbose: false,
            cache_dir,
        }
    }

    pub fn config(&self) -> anyhow::Result<Config> {
        Config::load(&self.repo)
    }

    pub fn local(&self) -> anyhow::Result<LocalConfig> {
        LocalConfig::load(&self.repo)
    }

    /// A committed jar's sha1 and cached copy.
    pub async fn committed(&mut self, blob: &str, path: &str) -> anyhow::Result<(String, PathBuf)> {
        if let Some(sha1) = self.hashes.blob_sha1(blob) {
            let cached = self.files.path(sha1);
            if cached.exists() {
                return Ok((sha1.to_string(), cached));
            }
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        let temp = tempfile::NamedTempFile::new_in(&self.cache_dir)?;
        self.git.blob_to_file(blob, path, temp.path())?;
        let sha1 = utils::files::hash_file(temp.path()).await?;
        let cached = self.files.path(&sha1);
        if let Some(parent) = cached.parent() {
            std::fs::create_dir_all(parent)?;
        }
        temp.persist(&cached)?;
        self.hashes.set_blob_sha1(blob, &sha1);
        Ok((sha1, cached))
    }
}
