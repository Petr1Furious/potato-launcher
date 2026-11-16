use anyhow::Context;
use reqwest::{Client, StatusCode};
use serde::Deserialize;

const API: &str = "https://api.modrinth.com/v2";

#[derive(Deserialize)]
struct Hashes {
    sha1: String,
}

#[derive(Deserialize)]
struct VersionFileJson {
    url: String,
    hashes: Hashes,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct VersionJson {
    id: String,
    project_id: String,
    files: Vec<VersionFileJson>,
}

#[derive(Clone, Debug)]
pub struct ModrinthFile {
    pub project_id: String,
    pub version_id: String,
    pub url: String,
    pub sha1: String,
}

fn to_file(version: VersionJson, sha1: Option<&str>) -> Option<ModrinthFile> {
    let file = match sha1 {
        Some(sha1) => version.files.into_iter().find(|f| f.hashes.sha1 == sha1)?,
        None => {
            let mut files = version.files;
            let index = files.iter().position(|f| f.primary).unwrap_or(0);
            if files.is_empty() {
                return None;
            }
            files.swap_remove(index)
        }
    };
    Some(ModrinthFile {
        project_id: version.project_id,
        version_id: version.id,
        url: file.url,
        sha1: file.hashes.sha1,
    })
}

pub async fn file_by_sha1(client: &Client, sha1: &str) -> anyhow::Result<Option<ModrinthFile>> {
    let response = client
        .get(format!("{API}/version_file/{sha1}?algorithm=sha1"))
        .send()
        .await
        .context("can't reach Modrinth")?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let version: VersionJson = response.error_for_status()?.json().await?;
    Ok(to_file(version, Some(sha1)))
}

pub async fn latest_file(client: &Client, slug: &str) -> anyhow::Result<ModrinthFile> {
    let response = client
        .get(format!("{API}/project/{slug}/version"))
        .send()
        .await
        .context("can't reach Modrinth")?;
    if response.status() == StatusCode::NOT_FOUND {
        anyhow::bail!("no Modrinth project '{slug}'");
    }
    let versions: Vec<VersionJson> = response.error_for_status()?.json().await?;
    versions
        .into_iter()
        .next()
        .and_then(|version| to_file(version, None))
        .with_context(|| format!("Modrinth project '{slug}' has no files"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "uses the network"]
    async fn looks_up_sodium() {
        let client = crate::http_client();
        let file = latest_file(&client, "sodium").await.unwrap();
        let by_hash = file_by_sha1(&client, &file.sha1).await.unwrap().unwrap();
        assert_eq!(by_hash.project_id, file.project_id);
        assert!(
            file_by_sha1(&client, &"0".repeat(40))
                .await
                .unwrap()
                .is_none()
        );
    }
}
