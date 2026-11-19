use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::Context;
use async_trait::async_trait;

use super::{LOCK_KEY, Storage, locked_error};

pub struct FileStorage {
    root: PathBuf,
    lock: Mutex<Option<std::fs::File>>,
}

impl FileStorage {
    pub fn new(url: url::Url) -> anyhow::Result<Self> {
        let root = url
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("invalid file target {url}"))?;
        Ok(Self {
            root,
            lock: Mutex::new(None),
        })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }
}

fn set_executable(path: &Path, executable: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, executable);
    Ok(())
}

#[async_trait]
impl Storage for FileStorage {
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match tokio::fs::read(self.path(key)).await {
            Ok(content) => Ok(Some(content)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn put(&self, key: &str, data: Vec<u8>) -> anyhow::Result<()> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        utils::files::write_file_atomic(&path, data).await?;
        Ok(())
    }

    async fn put_path(&self, key: &str, source: &Path, executable: bool) -> anyhow::Result<()> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temp = path.with_extension("potato-tmp");
        tokio::fs::copy(source, &temp).await?;
        set_executable(&temp, executable)?;
        tokio::fs::rename(&temp, &path).await?;
        Ok(())
    }

    async fn put_objects(&self, files: &[(String, PathBuf)]) -> anyhow::Result<()> {
        for (key, source) in files {
            let path = self.path(key);
            if path.exists() {
                continue;
            }
            self.put_path(key, source, false).await?;
        }
        Ok(())
    }

    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let mut listed = vec![];
        let mut stack = vec![self.root.join(prefix.trim_end_matches('/'))];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file() {
                    let key = path
                        .strip_prefix(&self.root)?
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    if key.starts_with(prefix) {
                        listed.push(key);
                    }
                }
            }
        }
        listed.sort();
        Ok(listed)
    }

    async fn delete(&self, keys: &[String]) -> anyhow::Result<()> {
        for key in keys {
            match tokio::fs::remove_file(self.path(key)).await {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    async fn lock(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("can't create {}", self.root.display()))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path(LOCK_KEY))?;
        if !try_flock(&file)? {
            return Err(locked_error());
        }
        *self.lock.lock().expect("lock mutex") = Some(file);
        Ok(())
    }

    async fn unlock(&self) -> anyhow::Result<()> {
        self.lock.lock().expect("lock mutex").take();
        Ok(())
    }
}

#[cfg(unix)]
fn try_flock(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(err)
    }
}

#[cfg(not(unix))]
fn try_flock(file: &std::fs::File) -> std::io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(err)) => Err(err),
    }
}
