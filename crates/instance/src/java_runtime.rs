use serde::{Deserialize, Serialize};
use url::Url;

/// A mirrored Zulu JRE archive hosted on the instance download server.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JavaRuntime {
    /// Azul package name, e.g. "zulu21.50.19-ca-crac-jre21.0.11-linux_x64.tar.gz"
    pub name: String,
    /// Java major version, e.g. "21"
    pub version: String,
    /// Launcher-facing OS: "windows" | "linux" | "macos"
    pub os: String,
    /// "x64" | "aarch64"
    pub arch: String,
    pub archive_type: String,
    pub url: Url,
}

impl JavaRuntime {
    pub fn matches_platform(&self, version: &str, os: &str, arch: &str) -> bool {
        self.version == version && self.os == os && self.arch == arch
    }
}
