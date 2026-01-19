use crate::UserInfo;
use crate::flow::{AuthMessage, AuthMessageProvider, AuthResultData, AuthState};
use crate::providers::AuthProvider;
use crate::vendor::minecraft_msa_auth::MinecraftAuthorizationFlow;
use async_trait::async_trait;
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, ClientId, DeviceAuthorizationUrl, DeviceCodeErrorResponseType, EndpointNotSet,
    EndpointSet, RefreshToken, RequestTokenError, Scope, StandardDeviceAuthorizationResponse,
    TokenResponse, TokenUrl,
};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

const MSA_DEVICE_CODE_URL: &str = "https://login.live.com/oauth20_connect.srf";
const MSA_TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";
const MSA_CLIENT_ID: &str = "00000000441cc96b";
const MSA_SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";

#[derive(thiserror::Error, Debug)]
pub enum AuthError {
    #[error("Timeout during authentication")]
    AuthTimeout,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct MicrosoftAuthProvider {}

#[derive(Deserialize)]
struct MinecraftProfileResponse {
    id: String,
    name: String,
}

fn async_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

fn get_oauth_client()
-> BasicClient<EndpointSet, EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet> {
    BasicClient::new(ClientId::new(MSA_CLIENT_ID.to_string()))
        .set_auth_uri(AuthUrl::new(MSA_DEVICE_CODE_URL.to_string()).unwrap())
        .set_token_uri(TokenUrl::new(MSA_TOKEN_URL.to_string()).unwrap())
        .set_device_authorization_url(
            DeviceAuthorizationUrl::new(MSA_DEVICE_CODE_URL.to_string()).unwrap(),
        )
}

async fn get_ms_token(
    message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
) -> anyhow::Result<AuthResultData> {
    let client = get_oauth_client();

    let details: StandardDeviceAuthorizationResponse = client
        .exchange_device_code()
        .add_scope(Scope::new(MSA_SCOPE.to_string()))
        .add_extra_param("response_type", "device_code")
        .request_async(&async_http_client()?)
        .await?;

    let code = details.user_code().secret().to_string();
    let url =
        Url::parse_with_params(details.verification_uri(), &[("otc", code.clone())])?.to_string();

    let _ = open::that(&url);
    message_provider
        .set_message(AuthMessage::LinkCode { url, code })
        .await;

    let token = client
        .exchange_device_access_token(&details)
        .request_async(
            &async_http_client()?,
            tokio::time::sleep,
            Some(Duration::from_secs(60 * 5)),
        )
        .await
        .map_err(|e| -> anyhow::Error {
            match &e {
                RequestTokenError::ServerResponse(resp)
                    if *resp.error() == DeviceCodeErrorResponseType::ExpiredToken =>
                {
                    AuthError::AuthTimeout.into()
                }
                _ => e.into(),
            }
        })?;

    Ok(AuthResultData {
        access_token: token.access_token().secret().to_string(),
        refresh_token: token.refresh_token().map(|t| t.secret().to_string()),
    })
}

#[async_trait]
impl AuthProvider for MicrosoftAuthProvider {
    async fn authenticate(
        &self,
        message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
    ) -> anyhow::Result<AuthState> {
        let ms_token = get_ms_token(message_provider.clone()).await?;
        message_provider.clear().await;
        let mc_flow = MinecraftAuthorizationFlow::new(Client::new());
        let mc_token = mc_flow
            .exchange_microsoft_token(ms_token.access_token)
            .await?
            .access_token()
            .clone()
            .0;

        Ok(AuthState::UserInfo(AuthResultData {
            access_token: mc_token,
            refresh_token: Some(ms_token.refresh_token.unwrap()),
        }))
    }

    async fn refresh(&self, refresh_token: String) -> anyhow::Result<AuthState> {
        let oauth_client = get_oauth_client();
        let token_response = oauth_client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .add_scope(Scope::new(MSA_SCOPE.to_string()))
            .request_async(&async_http_client()?)
            .await?;

        let mc_flow = MinecraftAuthorizationFlow::new(Client::new());
        let mc_token = mc_flow
            .exchange_microsoft_token(token_response.access_token().secret().to_string())
            .await?
            .access_token()
            .clone()
            .0;

        Ok(AuthState::UserInfo(AuthResultData {
            access_token: mc_token,
            refresh_token: token_response
                .refresh_token()
                .map(|t| t.secret().to_string()),
        }))
    }

    async fn get_user_info(&self, token: &str) -> anyhow::Result<AuthState> {
        let client = Client::new();
        let resp: MinecraftProfileResponse = client
            .get("https://api.minecraftservices.com/minecraft/profile")
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(AuthState::Success(UserInfo {
            uuid: resp.id,
            username: resp.name,
        }))
    }

    fn get_injector_url(&self) -> Option<String> {
        None
    }
}
