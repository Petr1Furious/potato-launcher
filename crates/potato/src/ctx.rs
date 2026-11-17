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
}
