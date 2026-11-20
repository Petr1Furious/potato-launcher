use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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

    fn title(self) -> &'static str {
        match self {
            Kind::WindowsExe => "Windows",
            Kind::MacosDmg | Kind::MacosArchive => "macOS",
            Kind::LinuxBin => "Linux",
            Kind::LinuxFlatpak => "Linux (Flatpak)",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Kind::WindowsExe => "Executable",
            Kind::MacosDmg => "Disk image",
            Kind::MacosArchive => "Archive",
            Kind::LinuxBin => "Binary",
            Kind::LinuxFlatpak => "Flatpak bundle",
        }
    }

    /// Symbol id in downloads.html.
    fn icon(self) -> &'static str {
        match self {
            Kind::WindowsExe => "windows",
            Kind::MacosDmg | Kind::MacosArchive => "macos",
            Kind::LinuxBin => "linux",
            Kind::LinuxFlatpak => "flatpak",
        }
    }
}

impl Kind {
    const ALL: [Kind; 5] = [
        Kind::WindowsExe,
        Kind::MacosDmg,
        Kind::MacosArchive,
        Kind::LinuxBin,
        Kind::LinuxFlatpak,
    ];

    /// The file name CI builds, from the launcher name.
    fn release_name(self, name: &str) -> String {
        let lower = lower_name(name);
        match self {
            Kind::WindowsExe => format!("{name}.exe"),
            Kind::MacosDmg => format!("{name}.dmg"),
            Kind::MacosArchive => format!("{lower}_macos.tar.gz"),
            Kind::LinuxBin => lower,
            Kind::LinuxFlatpak => format!("{lower}.flatpak"),
        }
    }

    /// Fixed, so the site's old-launcher rules don't need the launcher name.
    fn site_name(self) -> &'static str {
        match self {
            Kind::WindowsExe => "launcher.exe",
            Kind::MacosDmg => "launcher.dmg",
            Kind::MacosArchive => "launcher_macos.tar.gz",
            Kind::LinuxBin => "launcher",
            Kind::LinuxFlatpak => "launcher.flatpak",
        }
    }
}

fn classify(file_name: &str, name: &str) -> Option<Kind> {
    Kind::ALL
        .into_iter()
        .find(|kind| kind.release_name(name) == file_name)
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

struct Page<'a> {
    name: &'a str,
    files: &'a [Kind],
    versions: &'a BTreeMap<String, String>,
    icon: Option<&'a str>,
    source: Option<&'a str>,
}

fn render_index(page: &Page) -> String {
    let shown = page
        .files
        .iter()
        .filter_map(|kind| page.versions.get(kind.os()))
        .collect::<std::collections::BTreeSet<_>>();
    let shared = (shown.len() == 1).then(|| shown.into_iter().next().expect("one version"));
    let mut downloads = String::new();
    for kind in page.files {
        let mut detail = kind.detail().to_string();
        if shared.is_none()
            && let Some(version) = page.versions.get(kind.os())
        {
            detail.push_str(&format!(" · {}", escape(version)));
        }
        downloads.push_str(&format!(
            r##"<a class="download" data-os="{os}" href="launcher/{os}/{href}" download="{file}">
<svg><use href="#{icon}"/></svg>
<span class="tag">Your system</span>
<span class="title">{title}</span>
<span class="detail">{detail}</span>
<span class="button">Download</span>
</a>
"##,
            os = kind.os(),
            href = kind.site_name(),
            file = escape(&kind.release_name(page.name)),
            icon = kind.icon(),
            title = kind.title(),
        ));
    }
    if downloads.is_empty() {
        downloads.push_str("<p class=\"empty\">No downloads yet</p>\n");
    }
    let name = escape(page.name);
    let version = shared.map_or_else(String::new, |version| {
        format!(r#"<p class="version">{}</p>"#, escape(version))
    });
    let (favicon, logo) = match page.icon {
        Some(icon) => {
            let icon = percent_encoding::utf8_percent_encode(icon, HREF);
            (
                format!(r#"<link rel="icon" href="{icon}">"#),
                format!(r#"<img class="logo" src="{icon}" alt="">"#),
            )
        }
        None => (String::new(), String::new()),
    };
    let source = page.source.map_or_else(String::new, |url| {
        format!(
            r##"<footer><a href="{}" target="_blank" rel="noopener noreferrer"><svg><use href="#github"/></svg>View source on GitHub</a></footer>"##,
            escape(url)
        )
    });
    include_str!("downloads.html")
        .replace("{{name}}", &name)
        .replace("{{favicon}}", &favicon)
        .replace("{{logo}}", &logo)
        .replace("{{version}}", &version)
        .replace("{{downloads}}", downloads.trim_end())
        .replace("{{source}}", &source)
}

pub struct LauncherRelease<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub files: &'a [PathBuf],
    pub icon: Option<&'a Path>,
    pub source: Option<&'a str>,
}

pub async fn publish_launcher(target: &str, release: LauncherRelease<'_>) -> anyhow::Result<()> {
    let LauncherRelease {
        name,
        version,
        files,
        icon,
        source,
    } = release;
    let storage = storage::open(target)?;
    if let Some(icon) = icon {
        let extension = icon
            .extension()
            .and_then(|e| e.to_str())
            .context("the icon needs a file extension")?;
        storage
            .put_path(&format!("launcher/icon.{extension}"), icon, false)
            .await?;
    }
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
                &format!("launcher/{}/{}", kind.os(), kind.site_name()),
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
    let mut stored_icon = None;
    for key in storage.list("launcher/").await? {
        if key.starts_with("launcher/icon.") {
            stored_icon = Some(key);
            continue;
        }
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
        } else if let Some(kind) = Kind::ALL
            .into_iter()
            .find(|kind| kind.os() == os && kind.site_name() == file_name)
            && kind != Kind::MacosArchive
        {
            present.push(kind);
        }
    }
    present.sort();
    storage
        .put(
            "index.html",
            render_index(&Page {
                name,
                files: &present,
                versions: &versions,
                icon: stored_icon.as_deref(),
                source,
            })
            .into_bytes(),
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

    #[test]
    fn shows_a_shared_version_once() {
        let files = [Kind::WindowsExe, Kind::LinuxBin];
        let page = |windows: &str| {
            let versions = BTreeMap::from([
                ("windows".to_string(), windows.to_string()),
                ("linux".to_string(), "v2".to_string()),
            ]);
            render_index(&Page {
                name: "L",
                files: &files,
                versions: &versions,
                icon: None,
                source: None,
            })
        };
        let same = page("v2");
        assert_eq!(same.matches("v2").count(), 1, "{same}");
        let mixed = page("v1");
        assert!(
            mixed.contains("Executable · v1") && mixed.contains("Binary · v2"),
            "{mixed}"
        );
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
        let files = [bin, exe];
        let release = LauncherRelease {
            name: "Potato Launcher",
            version: "v1.2.3",
            files: &files,
            icon: None,
            source: Some("https://github.com/example/launcher"),
        };
        publish_launcher(target.as_str(), release).await.unwrap();
        let root = site.path();
        assert_eq!(
            std::fs::read_to_string(root.join("launcher/linux/version.txt")).unwrap(),
            "v1.2.3"
        );
        assert!(root.join("launcher/windows/launcher.exe").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("launcher/linux/launcher"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let index = std::fs::read_to_string(root.join("index.html")).unwrap();
        assert!(index.contains(r#"href="launcher/linux/launcher" download="potato_launcher""#));
        assert!(index.contains("v1.2.3"));
    }
}
