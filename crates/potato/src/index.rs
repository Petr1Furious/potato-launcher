use anyhow::Context;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::config::INDEX_DIRS;

pub const INDEX_SUFFIX: &str = ".pw.toml";
pub const DISABLED_SUFFIX: &str = ".disabled";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Client,
    Server,
    Both,
}

impl Side {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "client" => Ok(Side::Client),
            "server" => Ok(Side::Server),
            "both" => Ok(Side::Both),
            other => anyhow::bail!("unknown side '{other}'"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Side::Client => "client",
            Side::Server => "server",
            Side::Both => "both",
        }
    }

    pub fn includes(self, side: Side) -> bool {
        self == Side::Both || self == side
    }
}

pub fn current_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

pub fn normalize_os(os: &str) -> anyhow::Result<&'static str> {
    match os {
        "windows" => Ok("windows"),
        "macos" | "osx" => Ok("macos"),
        "linux" => Ok("linux"),
        other => anyhow::bail!("unknown os '{other}', expected windows, macos or linux"),
    }
}

/// `mods/.index/x.pw.toml` -> `mods`
pub fn index_dir_of(path: &str) -> Option<&'static str> {
    INDEX_DIRS.into_iter().find(|dir| {
        path.strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix("/.index/"))
            .is_some_and(|name| !name.contains('/') && name.ends_with(INDEX_SUFFIX))
    })
}

pub fn index_path(dir: &str, name: &str) -> String {
    format!("{dir}/.index/{name}{INDEX_SUFFIX}")
}

pub fn index_name(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(INDEX_SUFFIX).unwrap_or(file)
}

pub fn is_indexed_binary(dir: &str, file_name: &str) -> bool {
    let name = file_name.strip_suffix(DISABLED_SUFFIX).unwrap_or(file_name);
    if dir == "mods" {
        name.ends_with(".jar")
    } else {
        name.ends_with(".zip") || name.ends_with(".jar")
    }
}

#[derive(Clone)]
pub struct IndexFile {
    pub doc: DocumentMut,
}

impl IndexFile {
    pub fn parse(content: &str) -> anyhow::Result<Self> {
        let doc = content
            .parse::<DocumentMut>()
            .context("invalid index file")?;
        Ok(Self { doc })
    }

    pub fn parse_bytes(content: &[u8]) -> anyhow::Result<Self> {
        Self::parse(std::str::from_utf8(content).context("index file isn't utf-8")?)
    }

    /// For a committed jar: no download.
    pub fn metadata(name: &str, filename: &str) -> Self {
        let mut doc = DocumentMut::new();
        doc["name"] = value(name);
        doc["filename"] = value(filename);
        Self { doc }
    }

    pub fn strip_download(&mut self) {
        self.doc.remove("download");
    }

    pub fn new(name: &str, filename: &str, url: &str, sha1: &str) -> Self {
        let mut doc = DocumentMut::new();
        doc["name"] = value(name);
        doc["filename"] = value(filename);
        doc["side"] = value("both");
        let mut download = Table::new();
        download["url"] = value(url);
        download["hash-format"] = value("sha1");
        download["hash"] = value(sha1);
        doc["download"] = Item::Table(download);
        Self { doc }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.doc.to_string().into_bytes()
    }

    fn str_field(&self, key: &str) -> Option<&str> {
        self.doc.get(key).and_then(Item::as_str)
    }

    fn download_field(&self, key: &str) -> Option<&str> {
        self.doc
            .get("download")
            .and_then(|item| item.get(key))
            .and_then(Item::as_str)
    }

    pub fn filename(&self) -> Option<&str> {
        self.str_field("filename")
    }

    pub fn side(&self) -> anyhow::Result<Side> {
        self.str_field("side").map_or(Ok(Side::Both), Side::parse)
    }

    pub fn os(&self) -> anyhow::Result<Vec<&'static str>> {
        let Some(item) = self.doc.get("os") else {
            return Ok(vec![]);
        };
        let array = item.as_array().context("os must be a list")?;
        array
            .iter()
            .map(|os| normalize_os(os.as_str().context("os must be a list of strings")?))
            .collect()
    }

    pub fn applies_to_os(&self, os: &str) -> anyhow::Result<bool> {
        let list = self.os()?;
        Ok(list.is_empty() || list.contains(&os))
    }

    pub fn url(&self) -> Option<&str> {
        self.download_field("url")
    }

    pub fn sha1(&self) -> Option<&str> {
        (self.download_field("hash-format") == Some("sha1"))
            .then(|| self.download_field("hash"))
            .flatten()
    }

    pub fn set_side(&mut self, side: Side) {
        self.doc["side"] = value(side.as_str());
    }

    pub fn set_os(&mut self, os: &[&str]) {
        if os.is_empty() {
            self.doc.remove("os");
        } else {
            let mut array = toml_edit::Array::new();
            for os in os {
                array.push(*os);
            }
            self.doc["os"] = value(array);
        }
    }

    /// `modrinth:<id>` or `curseforge:<id>`.
    pub fn project(&self) -> Option<String> {
        let update = self.doc.get("update")?;
        if let Some(id) = update
            .get("modrinth")
            .and_then(|table| table.get("mod-id"))
            .and_then(Item::as_str)
        {
            return Some(format!("modrinth:{id}"));
        }
        let id = update.get("curseforge")?.get("project-id")?;
        let id = id
            .as_integer()
            .map(|id| id.to_string())
            .or_else(|| id.as_str().map(str::to_string))?;
        Some(format!("curseforge:{id}"))
    }

    pub fn set_modrinth(&mut self, project_id: &str, version_id: &str) {
        let mut table = Table::new();
        table["mod-id"] = value(project_id);
        table["version"] = value(version_id);
        let mut update = Table::new();
        update.set_implicit(true);
        update["modrinth"] = Item::Table(table);
        self.doc["update"] = Item::Table(update);
    }

    pub fn convert(&mut self, sha1: &str) {
        let download = &mut self.doc["download"];
        if let Some(table) = download.as_table_mut() {
            table.remove("mode");
        }
        download["hash-format"] = value("sha1");
        download["hash"] = value(sha1);
    }

    pub fn carry_overrides(&mut self, previous: &IndexFile) {
        for key in ["side", "os"] {
            match previous.doc.get(key) {
                Some(item) => self.doc[key] = item.clone(),
                None => {
                    self.doc.remove(key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRISM_CF: &str = r#"filename = "jei.jar"
name = "JEI"
side = "both"

[download]
hash = "d41d8cd98f00b204e9800998ecf8427e"
hash-format = "md5"
mode = "metadata:curseforge"

[update.curseforge]
file-id = 123
project-id = 456
"#;

    const PRISM_MR: &str = r#"filename = "sodium.jar"
name = "Sodium"
side = "client"

[download]
hash = "aaaa"
hash-format = "sha512"
url = "https://cdn.modrinth.com/data/x/sodium.jar"

[update.modrinth]
mod-id = "AANobbMI"
version = "abc"
"#;

    #[test]
    fn index_paths() {
        assert_eq!(index_dir_of("mods/.index/a.pw.toml"), Some("mods"));
        assert_eq!(
            index_dir_of("resourcepacks/.index/a.pw.toml"),
            Some("resourcepacks")
        );
        assert_eq!(index_dir_of("mods/.index/x/a.pw.toml"), None);
        assert_eq!(index_dir_of("config/a.pw.toml"), None);
        assert_eq!(index_name("mods/.index/voxy-linux.pw.toml"), "voxy-linux");
        assert!(is_indexed_binary("mods", "a.jar.disabled"));
        assert!(!is_indexed_binary("mods", "a.zip"));
        assert!(is_indexed_binary("shaderpacks", "a.zip"));
        assert!(!is_indexed_binary("shaderpacks", "a.zip.txt"));
    }

    #[test]
    fn curseforge_entry_becomes_metadata() {
        let mut file = IndexFile::parse(PRISM_CF).unwrap();
        assert_eq!(file.url(), None);
        file.strip_download();
        assert_eq!(file.sha1(), None);
        assert!(!file.to_bytes().windows(4).any(|w| w == b"mode"));
        assert!(file.doc["update"]["curseforge"].is_table());
    }

    #[test]
    fn converts_modrinth_entry_keeping_url() {
        let mut file = IndexFile::parse(PRISM_MR).unwrap();
        file.convert("ffff");
        assert_eq!(
            file.url(),
            Some("https://cdn.modrinth.com/data/x/sodium.jar")
        );
        assert_eq!(file.sha1(), Some("ffff"));
        assert_eq!(file.side().unwrap(), Side::Client);
    }

    #[test]
    fn os_and_overrides() {
        let mut repo = IndexFile::new("voxy", "voxy.jar", "https://u", "11");
        repo.set_os(&["osx"]);
        repo.set_side(Side::Client);
        assert_eq!(repo.os().unwrap(), vec!["macos"]);
        assert!(repo.applies_to_os("macos").unwrap());
        assert!(!repo.applies_to_os("linux").unwrap());
        let mut fresh = IndexFile::parse(PRISM_MR).unwrap();
        fresh.carry_overrides(&repo);
        assert_eq!(fresh.side().unwrap(), Side::Client);
        assert_eq!(fresh.os().unwrap(), vec!["macos"]);
    }
}
