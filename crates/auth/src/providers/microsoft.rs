use crate::UserInfo;
use crate::flow::{AuthMessage, AuthMessageProvider, AuthResultData, AuthState};
use crate::providers::{AuthProvider, base::AuthProviderError};
use crate::vendor::minecraft_msa_auth::MinecraftAuthorizationFlow;
use async_trait::async_trait;
use oauth2::basic::BasicClient;
use oauth2::reqwest as oauth2_reqwest;
use oauth2::{
    AuthUrl, ClientId, DeviceAuthorizationUrl, DeviceCodeErrorResponseType, EndpointNotSet,
    EndpointSet, RefreshToken, RequestTokenError, Scope, StandardDeviceAuthorizationResponse,
    TokenResponse, TokenUrl,
};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const MSA_DEVICE_CODE_URL: &str = "https://login.live.com/oauth20_connect.srf";
const MSA_TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";
const MSA_CLIENT_ID: &str = "00000000441cc96b";
const MSA_SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";

#[derive(thiserror::Error, Debug)]
pub enum MicrosoftAuthError {
    #[error("authentication timed out")]
    AuthTimeout,
    #[error("OAuth setup failed: {0}")]
    OAuthUrlParse(#[from] oauth2::url::ParseError),
    #[error("OAuth HTTP client setup failed: {0}")]
    OAuthHttpClient(#[from] oauth2_reqwest::Error),
    #[error("OAuth device code request failed: {0}")]
    DeviceCodeRequest(String),
    #[error("OAuth token exchange failed: {0}")]
    TokenExchange(String),
    #[error("failed to build verification URL: {0}")]
    VerificationUrl(String),
    #[error("microsoft token response did not include refresh token")]
    MissingRefreshToken,
    #[error("minecraft token exchange failed: {0}")]
    MinecraftAuth(#[from] crate::vendor::minecraft_msa_auth::MinecraftAuthorizationError),
    #[error("network request failed while fetching Minecraft profile: {0}")]
    Reqwest(#[from] reqwest::Error),
}

impl MicrosoftAuthError {
    pub fn is_client_error(&self) -> bool {
        matches!(self, Self::Reqwest(err) if err.status().is_some_and(|status| status.is_client_error()))
    }

    pub fn is_connect_error(&self) -> bool {
        matches!(self, Self::Reqwest(err) if err.is_connect())
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::AuthTimeout)
            || matches!(self, Self::Reqwest(err) if err.is_timeout() || err.status().map(|status| status.as_u16()) == Some(524))
    }
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct MicrosoftAuthProvider {}

#[derive(Deserialize)]
struct MinecraftProfileResponse {
    id: Uuid,
    name: String,
}

fn async_http_client() -> Result<oauth2_reqwest::Client, oauth2_reqwest::Error> {
    oauth2_reqwest::ClientBuilder::new()
        .redirect(oauth2_reqwest::redirect::Policy::none())
        .build()
}

fn get_oauth_client() -> Result<
    BasicClient<EndpointSet, EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet>,
    MicrosoftAuthError,
> {
    Ok(BasicClient::new(ClientId::new(MSA_CLIENT_ID.to_string()))
        .set_auth_uri(AuthUrl::new(MSA_DEVICE_CODE_URL.to_string())?)
        .set_token_uri(TokenUrl::new(MSA_TOKEN_URL.to_string())?)
        .set_device_authorization_url(DeviceAuthorizationUrl::new(
            MSA_DEVICE_CODE_URL.to_string(),
        )?))
}

async fn get_ms_token(
    message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
) -> Result<AuthResultData, MicrosoftAuthError> {
    let client = get_oauth_client()?;

    let details: StandardDeviceAuthorizationResponse = client
        .exchange_device_code()
        .add_scope(Scope::new(MSA_SCOPE.to_string()))
        .add_extra_param("response_type", "device_code")
        .request_async(&async_http_client()?)
        .await
        .map_err(|e| MicrosoftAuthError::DeviceCodeRequest(e.to_string()))?;

    let code = details.user_code().secret().to_string();
    let url = Url::parse_with_params(details.verification_uri(), &[("otc", code.clone())])
        .map_err(|e| MicrosoftAuthError::VerificationUrl(e.to_string()))?
        .to_string();

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
        .map_err(|e| match &e {
            RequestTokenError::ServerResponse(resp)
                if *resp.error() == DeviceCodeErrorResponseType::ExpiredToken =>
            {
                MicrosoftAuthError::AuthTimeout
            }
            _ => MicrosoftAuthError::TokenExchange(e.to_string()),
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
    ) -> Result<AuthState, AuthProviderError> {
        let ms_token = get_ms_token(message_provider.clone()).await?;
        message_provider.clear().await;
        let mc_flow = MinecraftAuthorizationFlow::new(Client::new());
        let mc_token = mc_flow
            .exchange_microsoft_token(ms_token.access_token)
            .await
            .map_err(MicrosoftAuthError::MinecraftAuth)?
            .access_token()
            .clone()
            .0;

        Ok(AuthState::UserInfo(AuthResultData {
            access_token: mc_token,
            refresh_token: Some(
                ms_token
                    .refresh_token
                    .ok_or(MicrosoftAuthError::MissingRefreshToken)?,
            ),
        }))
    }

    async fn refresh(&self, refresh_token: String) -> Result<AuthState, AuthProviderError> {
        let oauth_client = get_oauth_client()?;
        let token_response = oauth_client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .add_scope(Scope::new(MSA_SCOPE.to_string()))
            .request_async(&async_http_client().map_err(MicrosoftAuthError::OAuthHttpClient)?)
            .await
            .map_err(|e| MicrosoftAuthError::TokenExchange(e.to_string()))?;

        let mc_flow = MinecraftAuthorizationFlow::new(Client::new());
        let mc_token = mc_flow
            .exchange_microsoft_token(token_response.access_token().secret().to_string())
            .await
            .map_err(MicrosoftAuthError::MinecraftAuth)?
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

    async fn get_user_info(&self, token: &str) -> Result<AuthState, AuthProviderError> {
        let client = Client::new();
        let resp: MinecraftProfileResponse = client
            .get("https://api.minecraftservices.com/minecraft/profile")
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(MicrosoftAuthError::Reqwest)?
            .error_for_status()
            .map_err(MicrosoftAuthError::Reqwest)?
            .json()
            .await
            .map_err(MicrosoftAuthError::Reqwest)?;

        Ok(AuthState::Success(UserInfo {
            uuid: resp.id,
            username: resp.name,
        }))
    }

    fn get_injector_url(&self) -> Option<String> {
        None
    }
}
