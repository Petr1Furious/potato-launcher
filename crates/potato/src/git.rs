use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use anyhow::{Context, bail};

use crate::{
    config::INDEX_DIRS,
    index::{DISABLED_SUFFIX, is_indexed_binary},
};

pub type Files = BTreeMap<String, Vec<u8>>;

/// A committed jar in `Files`: its blob id instead of its bytes.
const BLOB_REF: &[u8] = b"\0potato-blob ";

pub fn blob_ref(id: &str) -> Vec<u8> {
    [BLOB_REF, id.as_bytes()].concat()
}

pub fn as_blob_ref(content: &[u8]) -> Option<&str> {
    content
        .strip_prefix(BLOB_REF)
        .and_then(|id| std::str::from_utf8(id).ok())
}

pub fn is_committed_binary(path: &str) -> bool {
    path.split_once('/').is_some_and(|(dir, name)| {
        INDEX_DIRS.contains(&dir)
            && !name.contains('/')
            && !name.ends_with(DISABLED_SUFFIX)
            && is_indexed_binary(dir, name)
    })
}

const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

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

    #[cfg(test)]
    pub fn is_clean(&self) -> anyhow::Result<bool> {
        Ok(self.out(&["status", "--porcelain"])?.is_empty())
    }

    pub fn changed_paths(&self) -> anyhow::Result<Vec<String>> {
        let out = self.checked(
            &["status", "--porcelain", "-z", "--untracked-files=all"],
            &[],
            None,
        )?;
        let mut paths = vec![];
        let mut records = out.split(|byte| *byte == 0).filter(|r| !r.is_empty());
        while let Some(record) = records.next() {
            let record = std::str::from_utf8(record)?;
            let path = record.get(3..).context("bad git status output")?;
            paths.push(path.to_string());
            if (record.starts_with('R') || record.starts_with('C'))
                && let Some(old) = records.next()
            {
                paths.push(std::str::from_utf8(old)?.to_string());
            }
        }
        Ok(paths)
    }

    pub fn head(&self) -> anyhow::Result<String> {
        self.out(&["rev-parse", "HEAD"])
            .context("repo has no commits yet")
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

    pub fn read_tree(&self, commit: &str) -> anyhow::Result<Files> {
        let listing = self.checked(&["ls-tree", "-r", "-z", "--full-tree", commit], &[], None)?;
        let mut entries = vec![];
        let mut binaries = vec![];
        for record in listing.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
            let record = std::str::from_utf8(record)?;
            let (meta, path) = record.split_once('\t').context("bad ls-tree output")?;
            let mut parts = meta.split_whitespace();
            let (_, kind, sha) = (parts.next(), parts.next(), parts.next());
            let Some(sha) = sha.filter(|_| kind == Some("blob")) else {
                continue;
            };
            if is_committed_binary(path) {
                binaries.push((path.to_string(), sha.to_string()));
            } else {
                entries.push((path.to_string(), sha.to_string()));
            }
        }
        let input = entries
            .iter()
            .map(|(_, sha)| format!("{sha}\n"))
            .collect::<String>();
        let output = self.checked(&["cat-file", "--batch"], &[], Some(input.as_bytes()))?;
        let mut files = binaries
            .into_iter()
            .map(|(path, sha)| (path, blob_ref(&sha)))
            .collect::<Files>();
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
        let mut text = vec![];
        let mut index_info = String::new();
        for (path, content) in files {
            if let Some(id) = as_blob_ref(content) {
                index_info.push_str(&format!("100644 {id}\t{path}\n"));
                continue;
            }
            let file = temp.path().join(text.len().to_string());
            std::fs::write(&file, content)?;
            paths.push_str(&file.to_string_lossy());
            paths.push('\n');
            text.push(path);
        }
        let hashes = self.checked(
            &["hash-object", "-w", "--no-filters", "--stdin-paths"],
            &[],
            Some(paths.as_bytes()),
        )?;
        let hashes = String::from_utf8(hashes)?;
        for (path, sha) in text.iter().zip(hashes.lines()) {
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

    /// Returns false on conflicts.
    pub fn cherry_pick_no_commit(&self, commit: &str) -> anyhow::Result<bool> {
        let output = self.run(&["cherry-pick", "--no-commit", commit], &[], None)?;
        // Otherwise `git commit` reuses the snapshot's author and message.
        let _ = self.run(&["cherry-pick", "--quit"], &[], None);
        if output.status.success() {
            return Ok(true);
        }
        let conflicts = self.out(&["diff", "--name-only", "--diff-filter=U"])?;
        if conflicts.is_empty() {
            bail!(
                "git cherry-pick failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(false)
    }

    /// Goes through the repo's filters, so with Git LFS the blob is a pointer.
    pub fn store_file(&self, path: &str, file: &Path) -> anyhow::Result<String> {
        let file = file.to_string_lossy();
        self.out(&["hash-object", "-w", "--path", path, &file])
    }

    pub fn blob_to_file(&self, id: &str, path: &str, dest: &Path) -> anyhow::Result<()> {
        let status = self
            .command(&["cat-file", "blob", id])
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(dest)?)
            .status()
            .context("can't run git")?;
        if !status.success() {
            bail!("can't read {path} from git");
        }
        let is_pointer =
            std::fs::metadata(dest)?.len() < 1024 && std::fs::read(dest)?.starts_with(LFS_POINTER);
        if !is_pointer {
            return Ok(());
        }
        let fetched = dest.with_extension("lfs-tmp");
        let status = self
            .command(&["lfs", "smudge", "--", path])
            .stdin(std::fs::File::open(dest)?)
            .stdout(std::fs::File::create(&fetched)?)
            .status()
            .context("can't run git")?;
        if !status.success() {
            let _ = std::fs::remove_file(&fetched);
            bail!("can't fetch {path} from Git LFS, check that git-lfs is installed");
        }
        std::fs::rename(&fetched, dest)?;
        Ok(())
    }

    /// Blob ids git would give `contents`, without storing them.
    pub fn blob_ids(&self, contents: &[&[u8]]) -> anyhow::Result<Vec<String>> {
        let temp = tempfile::tempdir()?;
        let mut paths = String::new();
        let mut text = vec![];
        for (index, content) in contents.iter().enumerate() {
            if as_blob_ref(content).is_none() {
                let file = temp.path().join(index.to_string());
                std::fs::write(&file, content)?;
                paths.push_str(&file.to_string_lossy());
                paths.push('\n');
                text.push(index);
            }
        }
        let hashed = if text.is_empty() {
            vec![]
        } else {
            let out = self.checked(
                &["hash-object", "--no-filters", "--stdin-paths"],
                &[],
                Some(paths.as_bytes()),
            )?;
            String::from_utf8(out)?
                .lines()
                .map(str::to_string)
                .collect()
        };
        let mut hashed = hashed.into_iter();
        Ok(contents
            .iter()
            .map(|content| match as_blob_ref(content) {
                Some(id) => id.to_string(),
                None => hashed.next().unwrap_or_default(),
            })
            .collect())
    }

    pub fn read_blobs(&self, ids: &[String]) -> anyhow::Result<Vec<Vec<u8>>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let input = ids.iter().map(|id| format!("{id}\n")).collect::<String>();
        let output = self.checked(&["cat-file", "--batch"], &[], Some(input.as_bytes()))?;
        let mut blobs = vec![];
        let mut rest = output.as_slice();
        for _ in ids {
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
            blobs.push(rest[start..start + size].to_vec());
            rest = &rest[start + size + 1..];
        }
        Ok(blobs)
    }

    /// Every version `paths` had in the commits of `range` (`None` for a deletion).
    pub fn path_versions(
        &self,
        range: &str,
        paths: &[String],
    ) -> anyhow::Result<HashMap<String, Vec<Option<String>>>> {
        let mut args = vec![
            "log",
            "--format=",
            "--raw",
            "--no-abbrev",
            "--no-renames",
            "-z",
            range,
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        let out = self.checked(&args, &[], None)?;
        let mut versions: HashMap<String, Vec<Option<String>>> = HashMap::new();
        let mut records = out.split(|byte| *byte == 0).filter(|r| !r.is_empty());
        while let Some(meta) = records.next() {
            let meta = std::str::from_utf8(meta)?;
            let Some(path) = records.next() else {
                break;
            };
            let mut fields = meta.trim_start_matches(':').split_whitespace();
            let new = fields.nth(3).context("bad git log output")?;
            let deleted = fields.next() == Some("D");
            versions
                .entry(std::str::from_utf8(path)?.to_string())
                .or_default()
                .push((!deleted).then(|| new.to_string()));
        }
        Ok(versions)
    }

    /// Commits in `HEAD`'s history that `base` doesn't have, newest first.
    pub fn commits_since(&self, base: &str) -> anyhow::Result<Vec<String>> {
        Ok(self
            .out(&["rev-list", &format!("{base}..HEAD")])?
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// Blob ids of `paths` in `commit`; paths it lacks are missing.
    pub fn path_blobs(
        &self,
        commit: &str,
        paths: &[String],
    ) -> anyhow::Result<HashMap<String, String>> {
        let mut args = vec!["ls-tree", "-z", "--full-tree", commit, "--"];
        args.extend(paths.iter().map(String::as_str));
        let listing = self.checked(&args, &[], None)?;
        let mut blobs = HashMap::new();
        for record in listing.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
            let record = std::str::from_utf8(record)?;
            let (meta, path) = record.split_once('\t').context("bad ls-tree output")?;
            if let Some(id) = meta.split_whitespace().nth(2) {
                blobs.insert(path.to_string(), id.to_string());
            }
        }
        Ok(blobs)
    }

    pub fn delete_refs(&self, prefix: &str) -> anyhow::Result<()> {
        let refs = self.out(&["for-each-ref", "--format=%(refname)", prefix])?;
        for name in refs.lines().filter(|line| !line.is_empty()) {
            self.checked(&["update-ref", "-d", name], &[], None)?;
        }
        Ok(())
    }

    pub fn update_ref(&self, name: &str, commit: &str) -> anyhow::Result<()> {
        self.checked(&["update-ref", name, commit], &[], None)
            .map(drop)
    }
}
