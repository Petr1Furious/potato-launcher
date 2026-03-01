use launcher_auth::{Username, storage::AuthProviderId};
use log::warn;
use serde::{Deserialize, Serialize};
use shared::paths::get_logs_dir;
use std::collections::HashMap;
use std::path::PathBuf;

use super::build_config;
use crate::{constants, lang::Lang, utils::get_data_dir};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct AuthProfile {
    pub auth_backend_id: AuthProviderId,
    pub username: Username,
}

fn provide_default_version_manifest_url() -> String {
    build_config::get_default_version_manifest_url()
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub java_paths: HashMap<String, String>,
    pub assets_dir: Option<String>,
    pub data_dir: Option<String>,
    pub xmx: HashMap<String, String>,
    pub use_native_glfw: HashMap<String, bool>,
    pub selected_instance_name: Option<String>,
    pub lang: Lang,
    pub hide_launcher_after_launch: bool,
    pub auth_profiles: HashMap<String, AuthProfile>,
    #[serde(default)]
    pub extra_version_manifest_urls: Vec<String>,
    #[serde(default = "provide_default_version_manifest_url")]
    pub selected_version_manifest_url: String,
}

const CONFIG_FILENAME: &str = "config.json";

fn get_config_path() -> PathBuf {
    get_data_dir().join(CONFIG_FILENAME)
}

impl Config {
    pub fn load() -> Config {
        let config_path = get_config_path();
        if config_path.exists() {
            let config_str =
                std::fs::read_to_string(&config_path).expect("Failed to read config file");
            if let Ok(config) = serde_json::from_str(&config_str) {
                return config;
            }
        }

        Config {
            java_paths: HashMap::new(),
            assets_dir: None,
            data_dir: None,
            xmx: HashMap::new(),
            use_native_glfw: HashMap::new(),
            selected_instance_name: None,
            lang: constants::DEFAULT_LANG,
            hide_launcher_after_launch: true,
            auth_profiles: HashMap::new(),
            extra_version_manifest_urls: Vec::new(),
            selected_version_manifest_url: build_config::get_default_version_manifest_url(),
        }
    }

    pub fn get_launcher_dir(&self) -> PathBuf {
        let data_dir = match &self.data_dir {
            None => dirs::data_dir()
                .expect("Failed to get data directory")
                .join(build_config::get_lower_launcher_name()),

            Some(dir) => PathBuf::from(dir),
        };
        if !data_dir.exists() {
            std::fs::create_dir_all(&data_dir).expect("Failed to create launcher directory");
        }
        data_dir
    }

    pub fn get_assets_dir(&self) -> PathBuf {
        let assets_dir = match &self.assets_dir {
            Some(dir) => PathBuf::from(dir),
            None => self.get_launcher_dir().join("assets"),
        };
        if !assets_dir.exists() {
            std::fs::create_dir_all(&assets_dir).expect("Failed to create assets directory");
        }
        assets_dir
    }

    pub fn get_effective_version_manifest_url(&self) -> &str {
        &self.selected_version_manifest_url
    }

    pub fn add_version_manifest_url(&mut self, url: String) {
        let url_trimmed = url.trim().to_string();
        if url_trimmed.is_empty() {
            return;
        }
        if url_trimmed == build_config::get_default_version_manifest_url() {
            return;
        }
        if !self
            .extra_version_manifest_urls
            .iter()
            .any(|u| u == &url_trimmed)
        {
            self.extra_version_manifest_urls.push(url_trimmed);
            self.save();
        }
    }

    pub fn remove_version_manifest_url(&mut self, url: &str) {
        self.extra_version_manifest_urls.retain(|u| u != url);
        if self.selected_version_manifest_url == url {
            self.selected_version_manifest_url = build_config::get_default_version_manifest_url();
        }
        self.save();
    }

    pub fn get_selected_auth_profile(&self) -> Option<&AuthProfile> {
        self.auth_profiles
            .get(self.selected_instance_name.as_ref()?)
    }

    pub fn set_selected_auth_profile(&mut self, auth_profile: AuthProfile) {
        if let Some(selected_instance_name) = &self.selected_instance_name {
            self.auth_profiles
                .insert(selected_instance_name.clone(), auth_profile);
            self.save();
        } else {
            warn!("Failed to set selected auth profile: no selected instance name");
        }
    }

    pub fn clear_selected_auth_profile(&mut self) {
        if let Some(selected_instance_name) = &self.selected_instance_name {
            self.auth_profiles.remove(selected_instance_name);
            self.save();
        } else {
            warn!("Failed to clear selected auth profile: no selected instance name");
        }
    }

    pub fn save(&self) {
        let config_str = serde_json::to_string_pretty(self).expect("Failed to serialize config");
        let config_path = get_config_path();
        std::fs::write(&config_path, config_str).expect("Failed to write config file");
    }
}

const LOGS_FILENAME: &str = "launcher.log";

pub fn get_logs_path() -> PathBuf {
    let logs_dir = get_logs_dir(&get_data_dir());
    if !logs_dir.exists() {
        std::fs::create_dir_all(&logs_dir).expect("Failed to create logs directory");
    }
    logs_dir.join(LOGS_FILENAME)
}
