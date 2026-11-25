use std::sync::Arc;

use serde::Deserialize;

#[derive(Deserialize)]
struct Response {
    success: bool,
    url: Option<String>,
    error: Option<String>,
}

pub async fn upload(client: &reqwest::Client, content: &str) -> anyhow::Result<Arc<str>> {
    let response: Response = client
        .post("https://api.mclo.gs/1/log")
        .form(&[("content", content)])
        .send()
        .await?
        .json()
        .await?;
    match response.url {
        Some(url) if response.success => Ok(url.into()),
        _ => anyhow::bail!(
            response
                .error
                .unwrap_or_else(|| "unknown error".to_string())
        ),
    }
}
