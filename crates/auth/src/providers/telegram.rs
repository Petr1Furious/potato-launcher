use crate::{
    UserInfo,
    flow::{AuthMessage, AuthMessageProvider, AuthResultData, AuthState},
    providers::{AuthProvider, base::AuthProviderError},
};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

#[derive(Deserialize)]
struct LoginStartResponse {
    code: String,
    intermediate_token: String,
}

#[derive(Deserialize)]
struct BotInfo {
    bot_username: String,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct TGAuthProvider {
    pub auth_base_url: String,
}

#[derive(thiserror::Error, Debug)]
pub enum TelegramAuthError {
    #[error("network request failed during Telegram auth: {0}")]
    Reqwest(#[from] reqwest::Error),
}

impl TelegramAuthError {
    pub fn is_client_error(&self) -> bool {
        matches!(self, Self::Reqwest(err) if err.status().is_some_and(|status| status.is_client_error()))
    }

    pub fn is_connect_error(&self) -> bool {
        matches!(self, Self::Reqwest(err) if err.is_connect())
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Reqwest(err) if err.is_timeout() || err.status().map(|status| status.as_u16()) == Some(524))
    }
}

impl TGAuthProvider {
    async fn get_bot_name(&self) -> Result<String, TelegramAuthError> {
        let bot_info: BotInfo = Client::new()
            .get(format!("{}/info", self.auth_base_url))
            .send()
            .await?
            .json()
            .await?;
        Ok(bot_info.bot_username)
    }
}

#[derive(Serialize, Debug)]
struct LoginPollRequest {
    intermediate_token: String,
}

#[derive(Deserialize, Debug)]
struct LoginPollResponseUser {
    access_token: String,
}

#[derive(Deserialize, Debug)]
struct LoginPollResponse {
    user: LoginPollResponseUser,
}

#[async_trait]
impl AuthProvider for TGAuthProvider {
    async fn authenticate(
        &self,
        message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
    ) -> Result<AuthState, AuthProviderError> {
        let client = Client::new();
        let bot_name = self.get_bot_name().await?;
        let start_resp: LoginStartResponse = client
            .post(format!("{}/login/start", self.auth_base_url))
            .send()
            .await
            .map_err(TelegramAuthError::Reqwest)?
            .json()
            .await
            .map_err(TelegramAuthError::Reqwest)?;

        let tg_deeplink = format!("https://t.me/{}?start={}", bot_name, start_resp.code);
        let _ = open::that(&tg_deeplink);
        message_provider
            .set_message(AuthMessage::Link { url: tg_deeplink })
            .await;

        let access_token;
        loop {
            let response = client
                .post(format!("{}/login/poll", self.auth_base_url))
                .json(&LoginPollRequest {
                    intermediate_token: start_resp.intermediate_token.clone(),
                })
                .send()
                .await;

            match response {
                Ok(resp) => {
                    resp.error_for_status_ref()
                        .map_err(TelegramAuthError::Reqwest)?;
                    let poll_resp: LoginPollResponse =
                        resp.json().await.map_err(TelegramAuthError::Reqwest)?;
                    access_token = poll_resp.user.access_token;
                    break;
                }
                Err(e) => {
                    if !e.is_timeout() {
                        return Err(TelegramAuthError::Reqwest(e).into());
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        Ok(AuthState::UserInfo(AuthResultData {
            access_token,
            refresh_token: None,
        }))
    }

    async fn refresh(&self, _: String) -> Result<AuthState, AuthProviderError> {
        Ok(AuthState::Auth)
    }

    async fn get_user_info(&self, token: &str) -> Result<AuthState, AuthProviderError> {
        let resp: UserInfo = Client::new()
            .get(format!("{}/login/profile", self.auth_base_url))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(TelegramAuthError::Reqwest)?
            .error_for_status()
            .map_err(TelegramAuthError::Reqwest)?
            .json()
            .await
            .map_err(TelegramAuthError::Reqwest)?;
        Ok(AuthState::Success(resp))
    }

    fn get_injector_url(&self) -> Option<String> {
        Some(self.auth_base_url.clone())
    }
}
