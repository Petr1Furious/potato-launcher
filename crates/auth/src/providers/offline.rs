use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    UserInfo,
    flow::{AuthMessageProvider, AuthResultData, AuthState},
    providers::{AuthProvider, base::AuthProviderError},
};

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct OfflineAuthProvider {}

#[async_trait]
impl AuthProvider for OfflineAuthProvider {
    async fn authenticate(
        &self,
        message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
    ) -> Result<AuthState, AuthProviderError> {
        Ok(AuthState::UserInfo(AuthResultData {
            access_token: message_provider.request_offline_nickname().await,
            refresh_token: None,
        }))
    }

    async fn refresh(&self, _: String) -> Result<AuthState, AuthProviderError> {
        Ok(AuthState::Auth)
    }

    async fn get_user_info(&self, token: &str) -> Result<AuthState, AuthProviderError> {
        let nickname = token;
        let namespace = Uuid::NAMESPACE_DNS;
        let generated_uuid = Uuid::new_v3(&namespace, nickname.as_bytes());

        Ok(AuthState::Success(UserInfo {
            uuid: generated_uuid,
            username: nickname.to_string(),
        }))
    }

    fn get_injector_url(&self) -> Option<String> {
        None
    }
}
