use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use anyhow::{Context, bail};

pub type Files = BTreeMap<String, Vec<u8>>;

/// Prefix of commits potato creates to record a target's state.
pub const SNAPSHOT_PREFIX: &str = "potato snapshot";

pub struct Git {
    dir: PathBuf,
}

impl Git {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.dir).args(args);
        command
    }

    fn run(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> anyhow::Result<Output> {
        let mut command = self.command(args);
        command.envs(env.iter().copied());
        command.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().context("can't run git")?;
        if let Some(input) = stdin {
            let mut pipe = child.stdin.take().expect("stdin is piped");
            let input = input.to_vec();
            std::thread::spawn(move || pipe.write_all(&input));
        }
        Ok(child.wait_with_output()?)
    }

    fn checked(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> anyhow::Result<Vec<u8>> {
        let output = self.run(args, env, stdin)?;
        if !output.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    fn out(&self, args: &[&str]) -> anyhow::Result<String> {
        let stdout = self.checked(args, &[], None)?;
        Ok(String::from_utf8_lossy(&stdout).trim().to_string())
    }

    pub fn init(&self) -> anyhow::Result<()> {
        self.checked(&["init", "-q"], &[], None).map(drop)
    }

    pub fn is_repo(&self) -> bool {
        self.run(&["rev-parse", "--git-dir"], &[], None)
            .is_ok_and(|output| output.status.success())
    }

    pub fn is_clean(&self) -> anyhow::Result<bool> {
        Ok(self.out(&["status", "--porcelain"])?.is_empty())
    }

    pub fn ensure_clean(&self) -> anyhow::Result<()> {
        if !self.is_clean()? {
            bail!("repo has uncommitted changes, commit them first");
        }
        Ok(())
    }

    pub fn head(&self) -> anyhow::Result<String> {
        self.out(&["rev-parse", "HEAD"])
            .context("repo has no commits yet")
    }

    pub fn branch(&self) -> anyhow::Result<Option<String>> {
        let output = self.run(&["symbolic-ref", "--short", "-q", "HEAD"], &[], None)?;
        Ok(output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()))
    }

    pub fn root_commit(&self) -> anyhow::Result<String> {
        let out = self.out(&["rev-list", "--max-parents=0", "HEAD"])?;
        out.lines()
            .last()
            .map(str::to_string)
            .context("repo has no commits yet")
    }

    pub fn commit_exists(&self, commit: &str) -> bool {
        self.run(
            &["cat-file", "-e", &format!("{commit}^{{commit}}")],
            &[],
            None,
        )
        .is_ok_and(|output| output.status.success())
    }

    pub fn is_ancestor(&self, ancestor: &str, commit: &str) -> anyhow::Result<bool> {
        if !self.commit_exists(ancestor) {
            return Ok(false);
        }
        let output = self.run(
            &["merge-base", "--is-ancestor", ancestor, commit],
            &[],
            None,
        )?;
        Ok(output.status.success())
    }

    pub fn message(&self, commit: &str) -> anyhow::Result<String> {
        self.out(&["log", "-1", "--format=%s", commit])
    }

    pub fn parent(&self, commit: &str) -> anyhow::Result<Option<String>> {
        let out = self.out(&["rev-list", "--parents", "-n", "1", commit])?;
        Ok(out.split_whitespace().nth(1).map(str::to_string))
    }

    pub fn read_tree(&self, commit: &str) -> anyhow::Result<Files> {
        let listing = self.checked(&["ls-tree", "-r", "-z", "--full-tree", commit], &[], None)?;
        let mut entries = vec![];
        for record in listing.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
            let record = std::str::from_utf8(record)?;
            let (meta, path) = record.split_once('\t').context("bad ls-tree output")?;
            let mut parts = meta.split_whitespace();
            let (_, kind, sha) = (parts.next(), parts.next(), parts.next());
            if kind == Some("blob") {
                entries.push((
                    path.to_string(),
                    sha.context("bad ls-tree output")?.to_string(),
                ));
            }
        }
        let input = entries
            .iter()
            .map(|(_, sha)| format!("{sha}\n"))
            .collect::<String>();
        let output = self.checked(&["cat-file", "--batch"], &[], Some(input.as_bytes()))?;
        let mut files = Files::new();
        let mut rest = output.as_slice();
        for (path, _) in entries {
            let header_end = rest
                .iter()
                .position(|b| *b == b'\n')
                .context("bad cat-file output")?;
            let header = std::str::from_utf8(&rest[..header_end])?;
            let size: usize = header
                .rsplit(' ')
                .next()
                .and_then(|size| size.parse().ok())
                .context("bad cat-file output")?;
            let start = header_end + 1;
            files.insert(path, rest[start..start + size].to_vec());
            rest = &rest[start + size + 1..];
        }
        Ok(files)
    }

    /// Creates a commit with exactly `files` as its tree.
    pub fn write_commit(
        &self,
        parent: Option<&str>,
        files: &Files,
        message: &str,
    ) -> anyhow::Result<String> {
        let temp = tempfile::tempdir()?;
        let index_file = temp.path().join("index");
        let index_env = index_file.to_string_lossy().into_owned();
        let env = [
            ("GIT_INDEX_FILE", index_env.as_str()),
            ("GIT_AUTHOR_NAME", "potato"),
            ("GIT_AUTHOR_EMAIL", "potato@localhost"),
            ("GIT_COMMITTER_NAME", "potato"),
            ("GIT_COMMITTER_EMAIL", "potato@localhost"),
        ];
        let mut paths = String::new();
        for (index, content) in files.values().enumerate() {
            let path = temp.path().join(index.to_string());
            std::fs::write(&path, content)?;
            paths.push_str(&path.to_string_lossy());
            paths.push('\n');
        }
        let hashes = self.checked(
            &["hash-object", "-w", "--no-filters", "--stdin-paths"],
            &[],
            Some(paths.as_bytes()),
        )?;
        let hashes = String::from_utf8(hashes)?;
        let mut index_info = String::new();
        for (path, sha) in files.keys().zip(hashes.lines()) {
            index_info.push_str(&format!("100644 {sha}\t{path}\n"));
        }
        self.checked(&["read-tree", "--empty"], &env, None)?;
        self.checked(
            &["update-index", "--index-info"],
            &env,
            Some(index_info.as_bytes()),
        )?;
        let tree = String::from_utf8(self.checked(&["write-tree"], &env, None)?)?;
        let mut args = vec!["commit-tree", tree.trim(), "-m", message];
        if let Some(parent) = parent {
            args.extend(["-p", parent]);
        }
        let commit = self.checked(&args, &env, None)?;
        Ok(String::from_utf8(commit)?.trim().to_string())
    }

    /// Merges without committing. Returns false when there are conflicts.
    pub fn merge_no_commit(&self, commit: &str, message: &str) -> anyhow::Result<bool> {
        let output = self.run(
            &[
                "merge",
                "--no-ff",
                "--no-commit",
                "--allow-unrelated-histories",
                "-m",
                message,
                commit,
            ],
            &[],
            None,
        )?;
        if output.status.success() {
            return Ok(true);
        }
        let conflicts = self.out(&["diff", "--name-only", "--diff-filter=U"])?;
        if conflicts.is_empty() {
            bail!(
                "git merge failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(false)
    }

    pub fn update_ref(&self, name: &str, commit: &str) -> anyhow::Result<()> {
        self.checked(&["update-ref", name, commit], &[], None)
            .map(drop)
    }
}
