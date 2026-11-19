use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, bail};
use async_trait::async_trait;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::Mutex,
};

use super::{LOCK_KEY, Storage, locked_error};

const MISSING_EXIT: i32 = 44;

/// `rsync://[user@]host[:port]/abs/path`, reached over ssh.
pub struct RsyncStorage {
    host: String,
    port: Option<u16>,
    root: String,
    lock: Mutex<Option<Child>>,
    /// Socket of the ssh connection all calls share.
    control: tempfile::TempDir,
}

fn quote(value: &str) -> String {
    shell_words::quote(value).into_owned()
}

impl RsyncStorage {
    pub fn new(url: &url::Url) -> anyhow::Result<Self> {
        let host = url.host_str().context("rsync target needs a host")?;
        let host = if url.username().is_empty() {
            host.to_string()
        } else {
            format!("{}@{host}", url.username())
        };
        let root = url.path().trim_end_matches('/').to_string();
        if root.is_empty() {
            bail!("rsync target needs a path");
        }
        Ok(Self {
            host,
            port: url.port(),
            root,
            lock: Mutex::new(None),
            control: tempfile::tempdir()?,
        })
    }

    fn remote(&self, key: &str) -> String {
        format!("{}/{key}", self.root)
    }

    fn control_path(&self) -> String {
        format!("ControlPath={}", self.control.path().join("ssh").display())
    }

    fn ssh_options(&self) -> Vec<String> {
        let mut options = vec![
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            self.control_path(),
            "-o".to_string(),
            "ControlPersist=60".to_string(),
        ];
        if let Some(port) = self.port {
            options.extend(["-p".to_string(), port.to_string()]);
        }
        options
    }

    fn ssh(&self, script: &str) -> Command {
        let mut command = Command::new("ssh");
        command.args(self.ssh_options());
        command.arg(&self.host).arg(script);
        command.kill_on_drop(true);
        command
    }

    async fn run(
        &self,
        script: &str,
        stdin: Option<Vec<u8>>,
    ) -> anyhow::Result<std::process::Output> {
        let mut command = self.ssh(script);
        command
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("can't run ssh")?;
        if let Some(input) = stdin {
            let mut pipe = child.stdin.take().expect("stdin is piped");
            pipe.write_all(&input).await?;
            drop(pipe);
        }
        Ok(child.wait_with_output().await?)
    }

    async fn checked(&self, script: &str, stdin: Option<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
        let output = self.run(script, stdin).await?;
        if !output.status.success() {
            bail!(
                "ssh {} failed: {}",
                self.host,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    fn write_script(&self, key: &str, executable: bool) -> String {
        let path = self.remote(key);
        let dir = path.rsplit_once('/').map_or("/", |(dir, _)| dir);
        let temp = format!("{path}.potato-tmp");
        let mode = if executable { "755" } else { "644" };
        format!(
            "mkdir -p {dir} && cat > {temp} && chmod {mode} {temp} && mv {temp} {path}",
            dir = quote(dir),
            temp = quote(&temp),
            path = quote(&path),
        )
    }
}

impl Drop for RsyncStorage {
    fn drop(&mut self) {
        let _ = std::process::Command::new("ssh")
            .args(["-o", &self.control_path(), "-O", "exit", &self.host])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// rsync 3.1+ has a single overall progress line; openrsync (macOS) doesn't.
async fn supports_progress2() -> bool {
    let Ok(output) = Command::new("rsync").arg("--version").output().await else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(version) = text
        .split_whitespace()
        .skip_while(|word| *word != "version")
        .nth(1)
    else {
        return false;
    };
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    (major, minor) >= (3, 1)
}

#[async_trait]
impl Storage for RsyncStorage {
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = quote(&self.remote(key));
        let script = format!("if [ -f {path} ]; then cat {path}; else exit {MISSING_EXIT}; fi");
        let output = self.run(&script, None).await?;
        match output.status.code() {
            Some(0) => Ok(Some(output.stdout)),
            Some(MISSING_EXIT) => Ok(None),
            _ => bail!(
                "ssh {} failed: {}",
                self.host,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }

    async fn put(&self, key: &str, data: Vec<u8>) -> anyhow::Result<()> {
        self.checked(&self.write_script(key, false), Some(data))
            .await
            .map(drop)
    }

    async fn put_path(&self, key: &str, path: &Path, executable: bool) -> anyhow::Result<()> {
        let data = tokio::fs::read(path).await?;
        self.checked(&self.write_script(key, executable), Some(data))
            .await
            .map(drop)
    }

    async fn put_objects(&self, files: &[(String, PathBuf)]) -> anyhow::Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let staging = tempfile::tempdir()?;
        for (key, source) in files {
            let link = staging.path().join(key);
            if let Some(parent) = link.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !link.exists() {
                std::fs::hard_link(source, &link)
                    .or_else(|_| std::fs::copy(source, &link).map(drop))?;
            }
        }
        self.checked(&format!("mkdir -p {}", quote(&self.root)), None)
            .await?;
        let mut command = Command::new("rsync");
        command
            .arg("-r")
            .arg("--ignore-existing")
            .arg("--chmod=F644,D755");
        command
            .arg("-e")
            .arg(format!("ssh {}", shell_words::join(self.ssh_options())));
        if supports_progress2().await {
            command.arg("--info=progress2");
        }
        command
            .arg(format!("{}/", staging.path().display()))
            .arg(format!("{}:{}/", self.host, self.root));
        let status = command.status().await.context("can't run rsync")?;
        if !status.success() {
            bail!("rsync to {} failed", self.host);
        }
        Ok(())
    }

    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let start = prefix.trim_end_matches('/');
        let script = format!(
            "cd {} 2>/dev/null || exit 0; [ -e {start} ] || exit 0; find {start} -type f",
            quote(&self.root),
            start = quote(if start.is_empty() { "." } else { start }),
        );
        let output = String::from_utf8(self.checked(&script, None).await?)?;
        let mut listed = output
            .lines()
            .map(|line| line.strip_prefix("./").unwrap_or(line).to_string())
            .filter(|key| key.starts_with(prefix))
            .collect::<Vec<_>>();
        listed.sort();
        Ok(listed)
    }

    async fn delete(&self, keys: &[String]) -> anyhow::Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let mut input = vec![];
        for key in keys {
            input.extend_from_slice(key.as_bytes());
            input.push(0);
        }
        let script = format!("cd {} && xargs -0 rm -f --", quote(&self.root));
        self.checked(&script, Some(input)).await.map(drop)
    }

    async fn lock(&self) -> anyhow::Result<()> {
        let lock = quote(&self.remote(LOCK_KEY));
        let script = format!(
            "mkdir -p {} && exec flock -n {lock} sh -c 'echo locked; exec cat'",
            quote(&self.root)
        );
        let mut command = self.ssh(&script);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().context("can't run ssh")?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).await?;
        if line.trim() != "locked" {
            let _ = child.wait().await;
            return Err(locked_error());
        }
        *self.lock.lock().await = Some(child);
        Ok(())
    }

    async fn unlock(&self) -> anyhow::Result<()> {
        if let Some(mut child) = self.lock.lock().await.take() {
            drop(child.stdin.take());
            let _ = child.wait().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target() {
        let url = url::Url::parse("rsync://mc@example.com:2222/srv/site/data/").unwrap();
        let storage = RsyncStorage::new(&url).unwrap();
        assert_eq!(storage.host, "mc@example.com");
        assert_eq!(storage.port, Some(2222));
        assert_eq!(storage.root, "/srv/site/data");
        assert!(storage.write_script("a b/c", true).contains("chmod 755"));
    }
}
