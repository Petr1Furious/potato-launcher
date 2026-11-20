use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::{
    config::{Config, LOCAL_DIR, LoaderName, LocalConfig, TargetType, under},
    index::Side,
};

pub const PRISM: &str = "prism";

pub enum TargetKind {
    Prism,
    Server { exclude: Vec<String> },
}

pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    /// The Prism instance folder or the server folder.
    pub root: PathBuf,
    pub game_dir: PathBuf,
    record: PathBuf,
    pulled: PathBuf,
}

impl Target {
    pub fn resolve(
        repo: &Path,
        name: &str,
        config: &Config,
        local: &LocalConfig,
    ) -> anyhow::Result<Self> {
        let record = synced_file(repo, name);
        let pulled = pulled_file(repo, name);
        let local_target = local.targets.get(name).with_context(|| {
            format!("unknown target '{name}', add it with `potato target add {name} <folder>`")
        })?;
        let root = local_target.path.clone();
        match local_target.kind {
            TargetType::Prism => {
                let game_dir = prism_game_dir(&root)?;
                Ok(Self {
                    name: name.to_string(),
                    kind: TargetKind::Prism,
                    root,
                    game_dir,
                    record,
                    pulled,
                })
            }
            TargetType::Server => {
                if !root.is_dir() {
                    bail!("{} doesn't exist", root.display());
                }
                let exclude = config
                    .servers
                    .get(name)
                    .map(|server| server.exclude.clone())
                    .unwrap_or_default();
                Ok(Self {
                    name: name.to_string(),
                    kind: TargetKind::Server { exclude },
                    game_dir: root.clone(),
                    root,
                    record,
                    pulled,
                })
            }
        }
    }

    pub fn names(local: &LocalConfig) -> Vec<String> {
        local.targets.keys().cloned().collect()
    }

    pub fn is_prism(&self) -> bool {
        matches!(self.kind, TargetKind::Prism)
    }

    pub fn side(&self) -> Side {
        if self.is_prism() {
            Side::Client
        } else {
            Side::Server
        }
    }

    pub fn is_excluded(&self, path: &str) -> bool {
        match &self.kind {
            TargetKind::Prism => false,
            TargetKind::Server { exclude } => exclude
                .iter()
                .any(|excluded| under(path, excluded.trim_end_matches('/'))),
        }
    }

    pub fn recorded_base(&self) -> Option<String> {
        std::fs::read_to_string(&self.record)
            .ok()
            .map(|content| content.trim().to_string())
            .filter(|content| !content.is_empty())
    }

    pub fn record_base(&self, commit: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.record.parent().expect("record has a parent"))?;
        std::fs::write(&self.record, format!("{commit}\n"))?;
        Ok(())
    }

    /// Each pull since the record moved, oldest first: its snapshot and the
    /// commit it was pulled onto.
    pub fn pulled(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(&self.pulled)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(snapshot, head)| (snapshot.to_string(), head.to_string()))
            .collect()
    }

    pub fn add_pulled(&self, snapshot: &str, head: &str) -> anyhow::Result<()> {
        let mut content = std::fs::read_to_string(&self.pulled).unwrap_or_default();
        content.push_str(&format!("{snapshot} {head}\n"));
        std::fs::create_dir_all(self.pulled.parent().expect("has a parent"))?;
        std::fs::write(&self.pulled, content)?;
        Ok(())
    }

    pub fn clear_pulled(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(&self.pulled) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    pub fn pulled_ref_prefix(&self) -> String {
        format!("refs/potato/pulled/{}/", self.name)
    }

    pub fn ref_name(&self) -> String {
        format!("refs/potato/{}", self.name)
    }

    pub fn mmc_pack(&self) -> anyhow::Result<Option<(String, LoaderName, Option<String>)>> {
        if !self.is_prism() {
            return Ok(None);
        }
        let path = self.root.join("mmc-pack.json");
        let Ok(content) = std::fs::read(&path) else {
            return Ok(None);
        };
        parse_mmc_pack(&content).map(Some)
    }

    pub fn is_running(&self) -> bool {
        if self.is_prism() {
            return false;
        }
        let level = std::fs::read_to_string(self.root.join("server.properties"))
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    line.strip_prefix("level-name=")
                        .map(|name| name.trim().to_string())
                })
            })
            .unwrap_or_else(|| "world".to_string());
        session_locked(&self.root.join(level).join("session.lock"))
    }
}

pub fn synced_file(repo: &Path, name: &str) -> PathBuf {
    repo.join(LOCAL_DIR).join("synced").join(name)
}

pub fn pulled_file(repo: &Path, name: &str) -> PathBuf {
    repo.join(LOCAL_DIR).join("pulled").join(name)
}

fn prism_game_dir(root: &Path) -> anyhow::Result<PathBuf> {
    ["minecraft", ".minecraft"]
        .into_iter()
        .map(|dir| root.join(dir))
        .find(|dir| dir.is_dir())
        .with_context(|| format!("no minecraft folder in {}", root.display()))
}

pub fn check_folder(kind: TargetType, path: &Path) -> anyhow::Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("{} doesn't exist", path.display()))?;
    match kind {
        TargetType::Prism => {
            prism_game_dir(&path)?;
        }
        TargetType::Server => {
            if !path.is_dir() {
                bail!("{} isn't a folder", path.display());
            }
            if path.join("mmc-pack.json").exists() {
                bail!("{} is a Prism instance, use --prism", path.display());
            }
        }
    }
    Ok(path)
}

#[derive(Deserialize)]
struct MmcComponent {
    uid: String,
    version: Option<String>,
}

#[derive(Deserialize)]
struct MmcPack {
    components: Vec<MmcComponent>,
}

pub fn parse_mmc_pack(content: &[u8]) -> anyhow::Result<(String, LoaderName, Option<String>)> {
    let pack: MmcPack = serde_json::from_slice(content).context("invalid mmc-pack.json")?;
    let mut minecraft = None;
    let mut loader = (LoaderName::Vanilla, None);
    for component in pack.components {
        let kind = match component.uid.as_str() {
            "net.minecraft" => {
                minecraft = component.version;
                continue;
            }
            "net.fabricmc.fabric-loader" => LoaderName::Fabric,
            "net.minecraftforge" => LoaderName::Forge,
            "net.neoforged" => LoaderName::Neoforge,
            _ => continue,
        };
        loader = (kind, component.version);
    }
    let minecraft = minecraft.context("mmc-pack.json has no Minecraft version")?;
    Ok((minecraft, loader.0, loader.1))
}

/// Minecraft holds a POSIX (fcntl) lock on session.lock while the world is open.
#[cfg(unix)]
fn session_locked(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        return false;
    };
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as libc::c_short;
    lock.l_whence = libc::SEEK_SET as libc::c_short;
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) };
    result == 0 && lock.l_type != libc::F_UNLCK as libc::c_short
}

#[cfg(not(unix))]
fn session_locked(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_mmc_pack() {
        let content = br#"{"components":[{"uid":"org.lwjgl3","version":"3.3.3"},{"uid":"net.minecraft","version":"1.21.1"},{"uid":"net.neoforged","version":"21.1.77"}],"formatVersion":1}"#;
        let (mc, loader, version) = parse_mmc_pack(content).unwrap();
        assert_eq!(mc, "1.21.1");
        assert_eq!(loader, LoaderName::Neoforge);
        assert_eq!(version.as_deref(), Some("21.1.77"));
    }

    #[cfg(unix)]
    #[test]
    fn lock_holder_helper() {
        use std::os::fd::AsRawFd;
        let Ok(path) = std::env::var("POTATO_TEST_LOCK") else {
            return;
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as libc::c_short;
        lock.l_whence = libc::SEEK_SET as libc::c_short;
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) },
            0
        );
        println!("locked");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[cfg(unix)]
    #[test]
    fn detects_held_session_lock() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.lock");
        std::fs::write(&path, b"").unwrap();
        assert!(!session_locked(&path));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "target::tests::lock_holder_helper",
                "--exact",
                "--nocapture",
            ])
            .env("POTATO_TEST_LOCK", &path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let reader = std::io::BufReader::new(child.stdout.take().unwrap());
        for line in reader.lines() {
            if line.unwrap().contains("locked") {
                break;
            }
        }
        assert!(session_locked(&path));
        child.kill().unwrap();
        let _ = child.wait();
    }
}
