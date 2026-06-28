use std::{collections::HashMap, path::PathBuf};

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
    providers: HashMap<AuthProviderId, AuthProviderConfig>,
    accounts: Vec<StorageAccountEntry>,
}

pub struct AuthStorage {
    disk_path: PathBuf,
    storage: AuthStorageData,
}

#[derive(thiserror::Error, Debug)]
pub enum AuthStorageError {
    #[error("failed to read auth storage from disk: {0}")]
    ReadIo(#[source] std::io::Error),
    #[error("failed to parse auth storage JSON: {0}")]
    ParseJson(#[from] serde_json::Error),
    #[error("auth storage JSON root must be an object")]
    InvalidRootType,
    #[error("failed to write auth storage JSON to disk: {0}")]
    WriteIo(#[source] std::io::Error),
}

impl AuthStorage {
    pub fn empty(auth_data_path: PathBuf) -> Self {
        Self {
            disk_path: auth_data_path,
            storage: AuthStorageData {
                providers: HashMap::new(),
                accounts: Vec::new(),
            },
        }
    }

    pub async fn load(auth_data_path: PathBuf) -> Result<Self, AuthStorageError> {
        let str_data = match tokio::fs::read_to_string(&auth_data_path).await {
            Ok(data) => Some(data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(AuthStorageError::ReadIo(e)),
        };
        let value = if let Some(str_data) = str_data {
            serde_json::from_str(&str_data)?
        } else {
            json!({})
        };
        let value_object = value.as_object().ok_or(AuthStorageError::InvalidRootType)?;
        let storage = if value_object.is_empty() {
            AuthStorageData {
                providers: HashMap::new(),
                accounts: Vec::new(),
            }
        } else {
            serde_json::from_value(value)?
        };

        Ok(Self {
            storage,
            disk_path: auth_data_path,
        })
    }

    async fn save(&self) -> Result<(), AuthStorageError> {
        let auth_data_str = serde_json::to_string(&self.storage)?;
        utils::files::write_file_atomic(&self.disk_path, auth_data_str.as_bytes())
            .await
            .map_err(AuthStorageError::WriteIo)?;
        Ok(())
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

    pub async fn insert_account(
        &mut self,
        provider_spec: &AuthProviderConfig,
        auth_data: AccountData,
    ) -> Result<AccountKey, AuthStorageError> {
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
                self.save().await?;
                return Ok((provider_id, username));
            }
        }
        self.storage.accounts.push(new_entry);
        self.save().await?;
        Ok((provider_id, username))
    }

    pub async fn delete_account(
        &mut self,
        auth_provider_id: AuthProviderId,
        username: &Username,
    ) -> Result<(), AuthStorageError> {
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
        self.save().await
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

    pub fn accounts(&self) -> impl Iterator<Item = &StorageAccountEntry> {
        self.storage.accounts.iter()
    }
}
