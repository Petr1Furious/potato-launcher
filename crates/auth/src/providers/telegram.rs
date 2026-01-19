use crate::{
    UserInfo,
    flow::{AuthMessage, AuthMessageProvider, AuthResultData, AuthState},
    providers::AuthProvider,
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

impl TGAuthProvider {
    async fn get_bot_name(&self) -> anyhow::Result<String> {
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
    ) -> anyhow::Result<AuthState> {
        let client = Client::new();
        let bot_name = self.get_bot_name().await?;
        let start_resp: LoginStartResponse = client
            .post(format!("{}/login/start", self.auth_base_url))
            .send()
            .await?
            .json()
            .await?;

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
                    resp.error_for_status_ref()?;
                    let poll_resp: LoginPollResponse = resp.json().await?;
                    access_token = poll_resp.user.access_token;
                    break;
                }
                Err(e) => {
                    if !e.is_timeout() {
                        return Err(e.into());
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

    async fn refresh(&self, _: String) -> anyhow::Result<AuthState> {
        Ok(AuthState::Auth)
    }

    async fn get_user_info(&self, token: &str) -> anyhow::Result<AuthState> {
        let resp: UserInfo = Client::new()
            .get(format!("{}/login/profile", self.auth_base_url))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(AuthState::Success(resp))
    }

    fn get_injector_url(&self) -> Option<String> {
        Some(self.auth_base_url.clone())
    }
}
