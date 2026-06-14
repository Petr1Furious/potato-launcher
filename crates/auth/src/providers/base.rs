use std::sync::Arc;

use async_trait::async_trait;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::{
    flow::{AuthMessageProvider, AuthState},
    providers::elyby::elyby_default_launcher_name,
};

use super::{
    elyby::{ElyByAuthError, ElyByAuthProvider},
    microsoft::{MicrosoftAuthError, MicrosoftAuthProvider},
    offline::OfflineAuthProvider,
    telegram::{TGAuthProvider, TelegramAuthError},
};

#[derive(thiserror::Error, Debug)]
pub enum AuthProviderError {
    #[error("microsoft authentication flow failed: {0}")]
    Microsoft(#[from] MicrosoftAuthError),
    #[error("telegram authentication flow failed: {0}")]
    Telegram(#[from] TelegramAuthError),
    #[error("ely.by authentication flow failed: {0}")]
    ElyBy(#[from] ElyByAuthError),
}

impl AuthProviderError {
    pub fn is_client_error(&self) -> bool {
        match self {
            AuthProviderError::Microsoft(err) => err.is_client_error(),
            AuthProviderError::Telegram(err) => err.is_client_error(),
            AuthProviderError::ElyBy(err) => err.is_client_error(),
        }
    }

    pub fn is_connect_error(&self) -> bool {
        match self {
            AuthProviderError::Microsoft(err) => err.is_connect_error(),
            AuthProviderError::Telegram(err) => err.is_connect_error(),
            AuthProviderError::ElyBy(err) => err.is_connect_error(),
        }
    }

    pub fn is_timeout(&self) -> bool {
        match self {
            AuthProviderError::Microsoft(err) => err.is_timeout(),
            AuthProviderError::Telegram(err) => err.is_timeout(),
            AuthProviderError::ElyBy(err) => err.is_timeout(),
        }
    }
}

/// All methods here should be stateless
#[async_trait]
pub trait AuthProvider {
    async fn authenticate(
        &self,
        message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
    ) -> Result<AuthState, AuthProviderError>;

    async fn refresh(&self, refresh_token: String) -> Result<AuthState, AuthProviderError>;

    async fn get_user_info(&self, token: &str) -> Result<AuthState, AuthProviderError>;

    fn get_injector_url(&self) -> Option<String>;
}

/// A list of auth providers, excluding additional provider-specific
/// config. Use for UI rendering
#[derive(Clone, Copy, PartialEq)]
pub enum AuthProviderType {
    Microsoft,
    Telegram,
    ElyBy,
    Offline,
}

impl AuthProviderType {
    pub fn iter() -> impl Iterator<Item = AuthProviderType> {
        [
            AuthProviderType::Microsoft,
            AuthProviderType::Telegram,
            AuthProviderType::ElyBy,
            AuthProviderType::Offline,
        ]
        .iter()
        .copied()
    }
}

/// A full provider description, including all the urls and keys.
/// Use for serialization
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthProviderConfig {
    Microsoft(MicrosoftAuthProvider),
    Telegram(TGAuthProvider),
    #[serde(rename = "ely.by")]
    ElyBy(ElyByAuthProvider),
    Offline(OfflineAuthProvider),
}

impl Default for AuthProviderConfig {
    fn default() -> Self {
        AuthProviderConfig::Microsoft(MicrosoftAuthProvider {})
    }
}

impl AuthProviderConfig {
    pub fn legacy_from_id(id: &str) -> Self {
        let mut iter = id.split('_');
        let provider_name = iter.next().unwrap_or("");
        let args: Vec<&str> = iter.collect();
        match provider_name {
            "telegram" => {
                if args.len() == 1 {
                    AuthProviderConfig::Telegram(TGAuthProvider {
                        auth_base_url: args[0].to_string(),
                    })
                } else {
                    warn!("Invalid arguments for telegram provider: {args:?}");
                    AuthProviderConfig::default()
                }
            }
            "elyby" => {
                if args.len() == 3 {
                    AuthProviderConfig::ElyBy(ElyByAuthProvider {
                        client_id: args[0].to_string(),
                        client_secret: args[1].to_string(),
                        launcher_name: args[2].to_string(),
                    })
                } else if args.len() == 2 {
                    AuthProviderConfig::ElyBy(ElyByAuthProvider {
                        client_id: args[0].to_string(),
                        client_secret: args[1].to_string(),
                        launcher_name: elyby_default_launcher_name(),
                    })
                } else {
                    warn!("Invalid arguments for elyby provider: {args:?}");
                    AuthProviderConfig::default()
                }
            }
            "microsoft" => AuthProviderConfig::Microsoft(MicrosoftAuthProvider {}),
            "offline" => AuthProviderConfig::Offline(OfflineAuthProvider {}),
            _ => {
                warn!("Unknown auth backend id: {id}");
                AuthProviderConfig::default()
            }
        }
    }

    pub fn get_type(&self) -> AuthProviderType {
        match self {
            AuthProviderConfig::Microsoft(_) => AuthProviderType::Microsoft,
            AuthProviderConfig::Telegram(_) => AuthProviderType::Telegram,
            AuthProviderConfig::ElyBy(_) => AuthProviderType::ElyBy,
            AuthProviderConfig::Offline(_) => AuthProviderType::Offline,
        }
    }

    pub fn get_injector_url(&self) -> Option<String> {
        self.get_provider().get_injector_url()
    }

    pub(crate) fn get_provider(&self) -> Box<dyn AuthProvider + Send> {
        match self {
            AuthProviderConfig::Microsoft(provider) => Box::new(provider.clone()),
            AuthProviderConfig::Telegram(provider) => Box::new(provider.clone()),
            AuthProviderConfig::ElyBy(provider) => Box::new(provider.clone()),
            AuthProviderConfig::Offline(provider) => Box::new(provider.clone()),
        }
    }
}
