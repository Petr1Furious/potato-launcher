use std::{
    collections::HashMap,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{providers::AuthProviderConfig, user_info::Username};

use super::user_info::AccountData;

pub type AuthProviderId = Uuid;
pub type AccountKey = (AuthProviderId, Username);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StorageAccountEntry {
    pub provider_id: AuthProviderId,
    #[serde(flatten)]
    pub auth_data: AccountData,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AuthStorageData {
    version: u32,
    providers: HashMap<AuthProviderId, AuthProviderConfig>,
    accounts: Vec<StorageAccountEntry>,
}

pub struct AuthStorage {
    disk_path: PathBuf,
    storage: AuthStorageData,
}

const LATEST_STORAGE_VERSION: u32 = 1;

impl AuthStorage {
    pub fn load(auth_data_path: PathBuf) -> Self {
        let str_data = match std::fs::read_to_string(&auth_data_path) {
            Ok(data) => Some(data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => panic!("Failed to read auth data from disk: {e:?}"),
        };
        let value = str_data
            .map(|str_data| serde_json::from_str(&str_data).expect("Failed to parse auth data"))
            .unwrap_or(json!({}));
        let value_object = value.as_object().expect("Failed to parse auth data");
        let storage = if !value_object.is_empty()
            && value_object
                .get("version")
                .and_then(|v| v.as_number())
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                < LATEST_STORAGE_VERSION.into()
        {
            let legacy_storage: HashMap<String, HashMap<String, AccountData>> =
                serde_json::from_value(value).expect("Failed to parse legacy auth data");
            let mut res = AuthStorageData {
                version: LATEST_STORAGE_VERSION,
                providers: HashMap::new(),
                accounts: Vec::new(),
            };
            let mut provider_map = HashMap::new();
            for (provider_id, users) in legacy_storage {
                let provider_uuid = provider_map
                    .entry(provider_id.clone())
                    .or_insert(Uuid::new_v4());
                res.providers
                    .entry(*provider_uuid)
                    .or_insert(AuthProviderConfig::legacy_from_id(&provider_id));
                for (_, auth_data) in users {
                    res.accounts.push(StorageAccountEntry {
                        auth_data,
                        provider_id: *provider_uuid,
                    });
                }
            }
            res
        } else {
            serde_json::from_value(value).expect("Failed to parse auth data")
        };

        Self {
            storage,
            disk_path: auth_data_path,
        }
    }

    fn save(&self) {
        let auth_data_str =
            serde_json::to_string(&self.storage).expect("Failed to serialize auth data");
        std::fs::write(&self.disk_path, auth_data_str).expect("Failed to write auth data to disk");
    }

    pub fn get_account(
        &self,
        auth_provider_id: AuthProviderId,
        username: &Username,
    ) -> Option<&StorageAccountEntry> {
        self.storage.accounts.iter().find(|x| {
            x.provider_id == auth_provider_id && x.auth_data.user_info.username == *username
        })
    }

    pub fn get_provider(&self, auth_provider_id: AuthProviderId) -> Option<&AuthProviderConfig> {
        self.storage.providers.get(&auth_provider_id)
    }

    pub fn get_provider_usernames(&self, auth_provider_id: AuthProviderId) -> Vec<String> {
        self.storage
            .accounts
            .iter()
            .filter(|x| x.provider_id == auth_provider_id)
            .map(|x| x.auth_data.user_info.username.clone())
            .collect()
    }

    pub fn insert_account(
        &mut self,
        provider_spec: &AuthProviderConfig,
        auth_data: AccountData,
    ) -> AccountKey {
        let provider_id = self
            .storage
            .providers
            .iter()
            .find(|(_, config)| *config == provider_spec)
            .map(|(&id, _)| id)
            .unwrap_or_else(|| {
                let new_id = Uuid::new_v4();
                self.storage.providers.insert(new_id, provider_spec.clone());
                new_id
            });

        let username = auth_data.user_info.username.clone();
        let new_entry = StorageAccountEntry {
            provider_id,
            auth_data,
        };
        for entry in self.storage.accounts.iter_mut() {
            if entry.provider_id == provider_id && entry.auth_data.user_info.username == username {
                *entry = new_entry;
                self.save();
                return (provider_id, username);
            }
        }
        self.storage.accounts.push(new_entry);
        self.save();
        (provider_id, username)
    }

    pub fn delete_account(&mut self, auth_provider_id: AuthProviderId, username: &Username) {
        self.storage.accounts.retain(|x| {
            !(x.provider_id == auth_provider_id && x.auth_data.user_info.username == *username)
        });
        let used_providers = self
            .storage
            .accounts
            .iter()
            .map(|x| x.provider_id)
            .collect::<Vec<_>>();
        self.storage
            .providers
            .retain(|id, _| used_providers.contains(id));
        self.save();
    }

    pub fn account_keys(&self) -> Vec<AccountKey> {
        let mut result = Vec::new();
        for account in &self.storage.accounts {
            result.push((
                account.provider_id,
                account.auth_data.user_info.username.clone(),
            ));
        }
        result.sort();
        result
    }
}
