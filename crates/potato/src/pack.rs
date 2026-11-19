use std::collections::BTreeMap;

use anyhow::{Context, bail};

use crate::{
    config::{Config, INDEX_DIRS},
    git::{Files, as_blob_ref, is_committed_binary},
    index::{IndexFile, Side, index_dir_of, index_name},
};

pub enum Source {
    Url { url: String, sha1: String },
    Committed { path: String, blob: String },
}

pub struct PackFile {
    pub dir: &'static str,
    pub filename: String,
    /// Without a URL, it only sets side, os and project ids for a committed file.
    pub index: Option<(String, IndexFile)>,
    pub source: Source,
}

impl PackFile {
    pub fn side(&self) -> anyhow::Result<Side> {
        self.index
            .as_ref()
            .map_or(Ok(Side::Both), |(_, index)| index.side())
    }

    pub fn applies_to_os(&self, os: &str) -> anyhow::Result<bool> {
        self.index
            .as_ref()
            .map_or(Ok(true), |(_, index)| index.applies_to_os(os))
    }

    /// The name `required` and optional sets use.
    pub fn name(&self) -> String {
        match &self.index {
            Some((path, _)) => index_name(path).to_string(),
            None => self
                .filename
                .rsplit_once('.')
                .map_or(self.filename.as_str(), |(stem, _)| stem)
                .to_string(),
        }
    }

    pub fn os(&self) -> anyhow::Result<Vec<&'static str>> {
        self.index
            .as_ref()
            .map_or(Ok(vec![]), |(_, index)| index.os())
    }

    /// Only when it has a download URL.
    pub fn download_index(&self) -> Option<&(String, IndexFile)> {
        matches!(self.source, Source::Url { .. })
            .then_some(self.index.as_ref())
            .flatten()
    }
}

pub fn pack_files(tree: &Files, config: &Config) -> anyhow::Result<Vec<PackFile>> {
    let mut committed = BTreeMap::new();
    for (path, content) in tree {
        if is_committed_binary(path) {
            let (dir, filename) = path.split_once('/').expect("binary is in a pack folder");
            if !config.handles_index_dir(dir) {
                continue;
            }
            let blob = as_blob_ref(content)
                .with_context(|| format!("{path} wasn't read as a committed file"))?;
            committed.insert(
                (dir.to_string(), filename.to_string()),
                (path.clone(), blob.to_string()),
            );
        }
    }

    let mut files = vec![];
    for (path, content) in tree {
        let Some(dir) = index_dir_of(path) else {
            continue;
        };
        if !config.handles_index_dir(dir) {
            continue;
        }
        let index = IndexFile::parse_bytes(content).with_context(|| path.clone())?;
        let filename = index
            .filename()
            .with_context(|| format!("{path} has no filename"))?
            .to_string();
        let key = (dir.to_string(), filename.clone());
        let source = match (index.url(), committed.remove(&key)) {
            (Some(_), Some((file, _))) => {
                bail!("{file} is committed and {path} has a download URL, remove one")
            }
            (Some(url), None) => Source::Url {
                url: url.to_string(),
                sha1: index
                    .sha1()
                    .with_context(|| format!("{path} has no sha1, pull from prism again"))?
                    .to_string(),
            },
            (None, Some((file, blob))) => Source::Committed { path: file, blob },
            (None, None) => {
                bail!("{path} has no download URL, commit {dir}/{filename} or remove it")
            }
        };
        files.push(PackFile {
            dir: INDEX_DIRS
                .into_iter()
                .find(|known| *known == dir)
                .expect("index dir"),
            filename,
            index: Some((path.clone(), index)),
            source,
        });
    }
    for ((dir, filename), (path, blob)) in committed {
        files.push(PackFile {
            dir: INDEX_DIRS
                .into_iter()
                .find(|known| *known == dir)
                .expect("pack dir"),
            filename,
            index: None,
            source: Source::Committed { path, blob },
        });
    }
    Ok(files)
}
