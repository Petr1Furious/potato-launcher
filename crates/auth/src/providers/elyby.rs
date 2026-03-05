use async_trait::async_trait;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::UserInfo;
use crate::flow::{AuthMessage, AuthMessageProvider, AuthResultData, AuthState};

use super::base::{AuthProvider, AuthProviderError};

const ELY_BY_BASE: &str = "https://ely.by/";

#[derive(thiserror::Error, Debug)]
pub enum ExchangeCodeError {
    #[error("invalid code")]
    InvalidCode,
    #[error("invalid token type")]
    InvalidTokenType,
    #[error("missing access token")]
    MissingAccessToken,
    #[error("network request failed while exchanging Ely.by code: {0}")]
    Reqwest(#[from] reqwest::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum HandleRequestError {
    #[error("missing query string")]
    MissingQueryString,
    #[error("failed to parse auth callback query: {0}")]
    SerdeUrlEncoded(#[from] serde_urlencoded::de::Error),
    #[error("failed to exchange Ely.by auth code: {0}")]
    ExchangeCode(#[from] ExchangeCodeError),
    #[error("failed to build callback response: {0}")]
    Http(#[from] hyper::http::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum ElyByAuthError {
    #[error("request channel closed unexpectedly")]
    RequestChannelClosed,
    #[error("authentication timed out")]
    AuthTimeout,
    #[error("failed to bind local callback server: {0}")]
    BindListener(#[source] std::io::Error),
    #[error("failed to read local callback listener address: {0}")]
    LocalAddr(#[source] std::io::Error),
    #[error("failed to accept callback connection: {0}")]
    AcceptConnection(#[source] std::io::Error),
    #[error("callback server failed: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("failed while handling callback request: {0}")]
    HandleRequest(#[from] HandleRequestError),
    #[error("failed to exchange Ely.by auth code: {0}")]
    ExchangeCode(#[from] ExchangeCodeError),
    #[error("network request failed while fetching Ely.by profile: {0}")]
    Reqwest(#[from] reqwest::Error),
}

impl ElyByAuthError {
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

pub(crate) fn elyby_default_launcher_name() -> String {
    "Potato Launcher".to_string()
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct ElyByAuthProvider {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "elyby_default_launcher_name")]
    pub launcher_name: String,
}

#[derive(Deserialize)]
struct AuthQuery {
    code: String,
}

async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<String, ExchangeCodeError> {
    let client = Client::new();
    let resp = client
        .post("https://account.ely.by/api/oauth2/v1/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
            ("code", code),
        ])
        .send()
        .await?;

    let status = resp.status();
    let data: serde_json::Value = resp.json().await?;
    if status != 200 && data.get("error") == Some(&"invalid_request".into()) {
        return Err(ExchangeCodeError::InvalidCode);
    }

    if data.get("token_type") != Some(&"Bearer".into()) {
        return Err(ExchangeCodeError::InvalidTokenType);
    }

    if let Some(access_token) = data.get("access_token")
        && let Some(access_token) = access_token.as_str()
    {
        Ok(access_token.to_string())
    } else {
        Err(ExchangeCodeError::MissingAccessToken)
    }
}

enum TokenResult {
    Token(String),
    InvalidCode,
    Error(ElyByAuthError),
}

impl ElyByAuthProvider {
    pub fn new(client_id: String, client_secret: String, launcher_name: String) -> Self {
        ElyByAuthProvider {
            client_id,
            client_secret,
            launcher_name,
        }
    }

    async fn print_auth_url(
        &self,
        redirect_uri: &str,
        message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
    ) {
        let url = format!(
            "https://account.ely.by/oauth2/v1?client_id={}&redirect_uri={}&response_type=code&scope=account_info%20minecraft_server_session&prompt=select_account",
            &self.client_id, redirect_uri
        );
        let _ = open::that(&url);
        message_provider
            .set_message(AuthMessage::Link { url })
            .await;
    }

    async fn handle_request(
        &self,
        redirect_uri: String,
        req: Request<hyper::body::Incoming>,
        token_tx: Box<mpsc::UnboundedSender<TokenResult>>,
    ) -> Result<Response<Full<Bytes>>, HandleRequestError> {
        let query = req
            .uri()
            .query()
            .ok_or(HandleRequestError::MissingQueryString)?;
        let auth_query: AuthQuery = serde_urlencoded::from_str(query)?;

        let token_result = match exchange_code(
            &self.client_id,
            &self.client_secret,
            &auth_query.code,
            &redirect_uri,
        )
        .await
        {
            Ok(token) => TokenResult::Token(token),
            Err(ExchangeCodeError::InvalidCode) => TokenResult::InvalidCode,
            Err(e) => TokenResult::Error(e.into()),
        };

        let response = match &token_result {
            TokenResult::Token(_) => Response::builder()
                .status(302)
                .header(
                    "Location",
                    format!(
                        "https://account.ely.by/oauth2/code/success?appName={}",
                        &self.launcher_name,
                    ),
                )
                .body(Full::new(Bytes::from("")))?,

            TokenResult::InvalidCode => Response::builder()
                .status(400)
                .body(Full::new(Bytes::from("Invalid code")))?,

            TokenResult::Error(_) => Response::builder()
                .status(500)
                .body(Full::new(Bytes::from("Internal server error")))?,
        };

        let _ = token_tx.send(token_result);

        Ok(response)
    }
}

#[async_trait]
impl AuthProvider for ElyByAuthProvider {
    async fn authenticate(
        &self,
        message_provider: Arc<dyn AuthMessageProvider + Send + Sync>,
    ) -> Result<AuthState, AuthProviderError> {
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = TcpListener::bind(addr)
            .await
            .map_err(ElyByAuthError::BindListener)?;

        let redirect_uri = format!(
            "http://localhost:{}/",
            listener
                .local_addr()
                .map_err(ElyByAuthError::LocalAddr)?
                .port()
        );
        self.print_auth_url(&redirect_uri, message_provider).await;

        let mut http = http1::Builder::new();
        http.keep_alive(false);

        loop {
            let stream;
            tokio::select! {
                _ = sleep(Duration::from_secs(120)) => {
                    return Err(ElyByAuthError::AuthTimeout.into());
                }

                st = listener.accept() => {
                    stream = st.map_err(ElyByAuthError::AcceptConnection)?.0;
                }
            }

            let io = TokioIo::new(stream);

            let (token_tx, mut token_rx) = mpsc::unbounded_channel();
            let token_tx = Box::new(token_tx);

            http.serve_connection(
                io,
                service_fn(|req: Request<hyper::body::Incoming>| {
                    let token_tx = token_tx.clone();
                    self.handle_request(redirect_uri.clone(), req, token_tx)
                }),
            )
            .await
            .map_err(ElyByAuthError::Hyper)?;

            if let Some(token) = token_rx.recv().await {
                match token {
                    TokenResult::Token(token) => {
                        return Ok(AuthState::UserInfo(AuthResultData {
                            access_token: token,
                            refresh_token: None,
                        }));
                    }
                    TokenResult::InvalidCode => continue,
                    TokenResult::Error(e) => return Err(e.into()),
                }
            } else {
                return Err(ElyByAuthError::RequestChannelClosed.into());
            }
        }
    }

    async fn refresh(&self, _: String) -> Result<AuthState, AuthProviderError> {
        Ok(AuthState::Auth)
    }

    async fn get_user_info(&self, token: &str) -> Result<AuthState, AuthProviderError> {
        let client = Client::new();
        let resp: UserInfo = client
            .get("https://account.ely.by/api/account/v1/info")
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(ElyByAuthError::Reqwest)?
            .error_for_status()
            .map_err(ElyByAuthError::Reqwest)?
            .json()
            .await
            .map_err(ElyByAuthError::Reqwest)?;
        Ok(AuthState::Success(resp))
    }

    fn get_injector_url(&self) -> Option<String> {
        Some(ELY_BY_BASE.to_string())
    }
}
