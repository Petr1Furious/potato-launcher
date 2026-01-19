use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::providers::AuthProviderConfig;

use super::user_info::AccountData;

#[derive(PartialEq, Default, Clone, Debug)]
pub enum AccountDataSource {
    #[default]
    Persistent,
    Runtime,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StorageEntry {
    #[serde(flatten)]
    pub auth_data: AccountData,
    #[serde(skip)]
    pub source: AccountDataSource,
}

pub struct AuthStorage {
    disk_path: PathBuf,
    storage: HashMap<String, HashMap<String, StorageEntry>>, // backend id -> username -> auth data
}

impl AuthStorage {
    pub fn load(auth_data_path: PathBuf) -> Self {
        // let launcher_dir = config.get_launcher_dir();
        // let auth_data_path = get_auth_data_path(&launcher_dir);
        Self {
            storage: std::fs::read_to_string(&auth_data_path)
                .map(|data| serde_json::from_str(&data).unwrap_or_default())
                .unwrap_or_default(),
            disk_path: auth_data_path,
        }
    }

    pub fn get_by_id(&self, id: &str, username: &str) -> Option<StorageEntry> {
        self.storage
            .get(id)
            .and_then(|user_map| user_map.get(username))
            .cloned()
    }

    pub fn get_id_nicknames(&self, id: &str) -> Vec<String> {
        self.storage
            .get(id)
            .unwrap_or(&HashMap::new())
            .keys()
            .cloned()
            .collect()
    }

    fn save(&self) {
        if let Ok(auth_data_str) = serde_json::to_string(&self.storage) {
            let _ = std::fs::write(&self.disk_path, auth_data_str);
        }
    }

    pub fn insert(&mut self, provider_spec: &AuthProviderConfig, auth_data: AccountData) {
        let id = provider_spec.get_id();
        let username = auth_data.user_info.username.clone();

        self.storage.entry(id.clone()).or_default().insert(
            username.clone(),
            StorageEntry {
                auth_data: auth_data.clone(),
                source: AccountDataSource::Runtime,
            },
        );

        self.save();
    }

    pub fn delete_by_id(&mut self, id: &str, username: &str) {
        if let Some(user_map) = self.storage.get_mut(id) {
            user_map.remove(username);
            if user_map.is_empty() {
                self.storage.remove(id);
            }
        }
        self.save();
    }

    pub fn get_all_entries(&self) -> Vec<(String, String)> {
        let mut entries = HashMap::new();
        for (id, user_map) in &self.storage {
            for username in user_map.keys() {
                entries.insert(id.clone(), username.clone());
            }
        }

        let mut result: Vec<_> = entries.into_iter().collect();
        result.sort();
        result
    }
}
