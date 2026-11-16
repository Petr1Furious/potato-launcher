use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Entry {
    size: u64,
    mtime_ns: u128,
    sha1: String,
}

/// sha1 of files, cached by path, size and modification time.
pub struct HashCache {
    path: PathBuf,
    entries: HashMap<String, Entry>,
    dirty: bool,
}

impl HashCache {
    pub fn load(cache_dir: &Path) -> Self {
        let path = cache_dir.join("hashes.json");
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|content| serde_json::from_slice(&content).ok())
            .unwrap_or_default();
        Self {
            path,
            entries,
            dirty: false,
        }
    }

    pub async fn sha1(&mut self, path: &Path) -> anyhow::Result<String> {
        let metadata = std::fs::metadata(path)?;
        let mtime_ns = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let key = path.to_string_lossy().into_owned();
        if let Some(entry) = self.entries.get(&key)
            && entry.size == metadata.len()
            && entry.mtime_ns == mtime_ns
        {
            return Ok(entry.sha1.clone());
        }
        let sha1 = utils::files::hash_file(path).await?;
        self.entries.insert(
            key,
            Entry {
                size: metadata.len(),
                mtime_ns,
                sha1: sha1.clone(),
            },
        );
        self.dirty = true;
        Ok(sha1)
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_vec(&self.entries)?)?;
        self.dirty = false;
        Ok(())
    }
}

impl Drop for HashCache {
    fn drop(&mut self) {
        let _ = self.save();
    }
}
