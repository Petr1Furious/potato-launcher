use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, bail};

use crate::storage;

/// Same rule as `release_lower_name()` in build-config and scripts/ci/set-build-env.sh.
pub fn lower_name(name: &str) -> String {
    name.to_lowercase().replace(' ', "_").replace('\'', "")
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    WindowsExe,
    MacosDmg,
    MacosArchive,
    LinuxBin,
    LinuxFlatpak,
}

impl Kind {
    fn os(self) -> &'static str {
        match self {
            Kind::WindowsExe => "windows",
            Kind::MacosDmg | Kind::MacosArchive => "macos",
            Kind::LinuxBin | Kind::LinuxFlatpak => "linux",
        }
    }

    fn executable(self) -> bool {
        matches!(self, Kind::WindowsExe | Kind::LinuxBin)
    }

    fn label(self) -> &'static str {
        match self {
            Kind::WindowsExe => "Windows",
            Kind::MacosDmg => "macOS",
            Kind::MacosArchive => "macOS (archive)",
            Kind::LinuxBin => "Linux",
            Kind::LinuxFlatpak => "Linux (Flatpak)",
        }
    }
}

fn classify(file_name: &str, name: &str) -> Option<Kind> {
    let lower = lower_name(name);
    if file_name == format!("{name}.exe") {
        Some(Kind::WindowsExe)
    } else if file_name == format!("{name}.dmg") {
        Some(Kind::MacosDmg)
    } else if file_name == format!("{lower}_macos.tar.gz") {
        Some(Kind::MacosArchive)
    } else if file_name == lower {
        Some(Kind::LinuxBin)
    } else if file_name == format!("{lower}.flatpak") {
        Some(Kind::LinuxFlatpak)
    } else {
        None
    }
}

const HREF: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'\'')
    .add(b'&');

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render_index(
    name: &str,
    files: &[(Kind, String)],
    versions: &BTreeMap<String, String>,
) -> String {
    let mut links = String::new();
    for (kind, file) in files {
        let version = versions
            .get(kind.os())
            .map(|v| format!(" <span>{}</span>", escape(v)))
            .unwrap_or_default();
        links.push_str(&format!(
            "<li><a href=\"launcher/{}/{}\">{}</a>{version}</li>\n",
            kind.os(),
            percent_encoding::utf8_percent_encode(file, HREF),
            kind.label()
        ));
    }
    let name = escape(name);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name}</title>
<style>
:root {{ color-scheme: light dark; --bg: #fff; --fg: #1a1a1a; --muted: #6b6b6b; --accent: #b5651d; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg: #161616; --fg: #ececec; --muted: #9a9a9a; --accent: #e0a060; }} }}
body {{ margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.5 system-ui, sans-serif; }}
main {{ max-width: 32rem; margin: 4rem auto; padding: 0 16px; }}
h1 {{ font-size: 1.75rem; margin: 0 0 1.5rem; }}
ul {{ list-style: none; padding: 0; margin: 0; }}
li {{ margin: 0.5rem 0; }}
a {{ color: var(--accent); font-weight: 600; }}
span {{ color: var(--muted); font-size: 0.875rem; margin-left: 0.5rem; }}
</style>
</head>
<body>
<main>
<h1>{name}</h1>
<ul>
{links}</ul>
</main>
</body>
</html>
"#
    )
}

pub async fn publish_launcher(
    target: &str,
    name: &str,
    version: &str,
    files: &[PathBuf],
) -> anyhow::Result<()> {
    let storage = storage::open(target)?;
    let mut touched = vec![];
    for file in files {
        let file_name = file
            .file_name()
            .and_then(|n| n.to_str())
            .with_context(|| format!("bad file name {}", file.display()))?;
        let Some(kind) = classify(file_name, name) else {
            bail!("don't know which OS {file_name} is for");
        };
        storage
            .put_path(
                &format!("launcher/{}/{file_name}", kind.os()),
                file,
                kind.executable(),
            )
            .await?;
        println!("uploaded {file_name}");
        if !touched.contains(&kind.os()) {
            touched.push(kind.os());
        }
    }
    for os in &touched {
        storage
            .put(
                &format!("launcher/{os}/version.txt"),
                version.as_bytes().to_vec(),
            )
            .await?;
    }

    let mut present = vec![];
    let mut versions = BTreeMap::new();
    for key in storage.list("launcher/").await? {
        let mut parts = key.splitn(3, '/').skip(1);
        let (Some(os), Some(file_name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if file_name == "version.txt" {
            if let Some(content) = storage.get(&key).await? {
                versions.insert(
                    os.to_string(),
                    String::from_utf8_lossy(&content).trim().to_string(),
                );
            }
        } else if let Some(kind) = classify(file_name, name)
            && kind.os() == os
            && kind != Kind::MacosArchive
        {
            present.push((kind, file_name.to_string()));
        }
    }
    present.sort();
    storage
        .put(
            "index.html",
            render_index(name, &present, &versions).into_bytes(),
        )
        .await?;
    println!("published {name} {version}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_release_files() {
        let name = "Potato's Launcher";
        assert_eq!(lower_name(name), "potatos_launcher");
        assert_eq!(
            classify("Potato's Launcher.exe", name),
            Some(Kind::WindowsExe)
        );
        assert_eq!(
            classify("Potato's Launcher.dmg", name),
            Some(Kind::MacosDmg)
        );
        assert_eq!(
            classify("potatos_launcher_macos.tar.gz", name),
            Some(Kind::MacosArchive)
        );
        assert_eq!(classify("potatos_launcher", name), Some(Kind::LinuxBin));
        assert_eq!(
            classify("potatos_launcher.flatpak", name),
            Some(Kind::LinuxFlatpak)
        );
        assert_eq!(classify("other.zip", name), None);
    }

    #[tokio::test]
    async fn publishes_to_file_target() {
        let site = tempfile::tempdir().unwrap();
        let release = tempfile::tempdir().unwrap();
        let bin = release.path().join("potato_launcher");
        let exe = release.path().join("Potato Launcher.exe");
        std::fs::write(&bin, b"bin").unwrap();
        std::fs::write(&exe, b"exe").unwrap();
        let target = url::Url::from_directory_path(site.path()).unwrap();
        publish_launcher(target.as_str(), "Potato Launcher", "v1.2.3", &[bin, exe])
            .await
            .unwrap();
        let root = site.path();
        assert_eq!(
            std::fs::read_to_string(root.join("launcher/linux/version.txt")).unwrap(),
            "v1.2.3"
        );
        assert!(root.join("launcher/windows/Potato Launcher.exe").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("launcher/linux/potato_launcher"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let index = std::fs::read_to_string(root.join("index.html")).unwrap();
        assert!(index.contains("launcher/linux/potato_launcher"));
        assert!(index.contains("v1.2.3"));
    }
}
