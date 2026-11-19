use std::{collections::BTreeMap, path::PathBuf};

pub struct Upload {
    pub key: String,
    pub path: PathBuf,
    /// For example `mods/jei.jar`.
    pub label: String,
    /// Libraries, assets and Java are summarized by group unless verbose.
    pub group: Option<String>,
}

impl Upload {
    pub fn new(key: String, path: PathBuf, label: String) -> Self {
        Self {
            key,
            path,
            label,
            group: None,
        }
    }

    fn size(&self) -> u64 {
        std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0)
    }
}

pub fn pairs<'a>(uploads: impl IntoIterator<Item = &'a Upload>) -> Vec<(String, PathBuf)> {
    uploads
        .into_iter()
        .map(|upload| (upload.key.clone(), upload.path.clone()))
        .collect()
}

pub fn print<'a>(uploads: impl IntoIterator<Item = &'a Upload>, verbose: bool) {
    let uploads = uploads.into_iter().collect::<Vec<_>>();
    if uploads.is_empty() {
        return;
    }
    let total = uploads.iter().map(|upload| upload.size()).sum();
    println!("uploading {} ({})", files(uploads.len()), human_size(total));
    let mut groups: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    for upload in uploads {
        match &upload.group {
            Some(group) if !verbose => {
                let entry = groups.entry(group).or_default();
                entry.0 += 1;
                entry.1 += upload.size();
            }
            _ => {
                println!("  {} ({})", upload.label, human_size(upload.size()));
            }
        }
    }
    for (group, (count, size)) in groups {
        println!("  {group}: {}, {}", files(count), human_size(size));
    }
}

fn files(count: usize) -> String {
    if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn human_sizes() {
        assert_eq!(super::human_size(512), "512 B");
        assert_eq!(super::human_size(1536), "1.5 KB");
        assert_eq!(super::human_size(3 << 20), "3.0 MB");
    }
}
