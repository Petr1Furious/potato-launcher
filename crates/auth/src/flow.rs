use std::sync::Arc;

use async_trait::async_trait;

use crate::{providers::AuthProviderConfig, user_info::UserInfo};

use super::user_info::AccountData;

#[derive(Clone)]
pub struct AuthResultData {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

pub enum AuthState {
    Auth,
    Refresh,
    UserInfo(AuthResultData),
    Success(UserInfo),
}

#[derive(Clone, PartialEq, Debug)]
pub enum AuthMessage {
    Link { url: String },
    LinkCode { url: String, code: String },
}

#[derive(thiserror::Error, Debug)]
pub enum AuthError {
    #[error("Auth loop exceeded max iterations")]
    InfiniteAuthLoop,
}

#[async_trait]
pub trait AuthMessageProvider {
    async fn set_message(&self, message: AuthMessage);
    async fn get_message(&self) -> Option<AuthMessage>;
    async fn clear(&self);
    async fn request_offline_nickname(&self) -> String;
    async fn need_offline_nickname(&self) -> bool;
    async fn set_offline_nickname(&self, nickname: String);
}

pub async fn perform_auth(
    account_data: Option<AccountData>,
    auth_provider: AuthProviderConfig,
    auth_message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
) -> anyhow::Result<AccountData> {
    let mut auth_result_data = account_data.map(|data| AuthResultData {
        access_token: data.access_token,
        refresh_token: data.refresh_token,
    });
    let mut auth_state = auth_result_data
        .clone()
        .map_or(AuthState::Auth, AuthState::UserInfo);
    let auth_provider = auth_provider.get_provider();

    for _ in 0..10 {
        match auth_state {
            AuthState::Auth => {
                auth_state = auth_provider
                    .authenticate(auth_message_provider.clone())
                    .await?;
            }

            AuthState::Refresh => {
                let refresh_token = auth_result_data
                    .as_ref()
                    .and_then(|data| data.refresh_token.clone());
                auth_state = match refresh_token {
                    Some(refresh_token) => auth_provider.refresh(refresh_token).await?,
                    None => AuthState::Auth,
                };
            }

            AuthState::UserInfo(data) => {
                auth_result_data = Some(data.clone());
                auth_state = auth_provider
                    .get_user_info(&data.access_token)
                    .await
                    .or_else(|e| {
                        let is_client_error = e
                            .downcast_ref::<reqwest::Error>()
                            .and_then(|re| re.status())
                            .map(|status| status.is_client_error())
                            .unwrap_or(false);
                        if is_client_error {
                            Ok(AuthState::Refresh)
                        } else {
                            Err(e)
                        }
                    })?;
            }

            AuthState::Success(info) => {
                let auth_result_data = auth_result_data.unwrap();
                return Ok(AccountData {
                    access_token: auth_result_data.access_token,
                    refresh_token: auth_result_data.refresh_token,
                    user_info: info,
                });
            }
        }
    }

    Err(AuthError::InfiniteAuthLoop.into())
}
