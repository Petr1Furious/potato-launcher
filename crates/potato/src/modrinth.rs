use std::collections::{HashMap, HashSet};

use anyhow::{Context, bail};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use url::Url;

use crate::{config::LoaderName, index::Side};

const API: &str = "https://api.modrinth.com/v2/";

#[derive(Deserialize)]
struct Hashes {
    sha1: String,
}

#[derive(Deserialize)]
struct VersionFileJson {
    url: String,
    filename: String,
    hashes: Hashes,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct DependencyJson {
    project_id: Option<String>,
    dependency_type: String,
}

#[derive(Deserialize)]
struct VersionJson {
    id: String,
    project_id: String,
    version_number: String,
    version_type: String,
    files: Vec<VersionFileJson>,
    #[serde(default)]
    dependencies: Vec<DependencyJson>,
}

#[derive(Clone, Debug)]
pub struct ModrinthFile {
    pub project_id: String,
    pub version_id: String,
    pub url: String,
    pub sha1: String,
    pub filename: String,
    /// Project ids of required dependencies.
    pub required: Vec<String>,
}

#[derive(Deserialize)]
pub struct Project {
    pub slug: String,
    #[serde(default)]
    pub loaders: Vec<String>,
    client_side: String,
    server_side: String,
}

impl Project {
    pub fn side(&self) -> Side {
        match (self.client_side.as_str(), self.server_side.as_str()) {
            (_, "unsupported") => Side::Client,
            ("unsupported", _) => Side::Server,
            _ => Side::Both,
        }
    }
}

/// Which versions of a project fit the pack.
pub struct Filter<'a> {
    pub game_version: Option<&'a str>,
    pub loader: Option<&'static str>,
}

impl Filter<'_> {
    /// The loader Modrinth lists files of `dir` under.
    pub fn loader_for(dir: &str, loader: LoaderName) -> Option<&'static str> {
        match dir {
            "mods" => (loader != LoaderName::Vanilla).then(|| loader.as_str()),
            "resourcepacks" => Some("minecraft"),
            "datapacks" => Some("datapack"),
            _ => None,
        }
    }

    fn describe(&self) -> String {
        [self.game_version, self.loader]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn to_file(version: VersionJson, sha1: Option<&str>) -> Option<ModrinthFile> {
    let mut files = version.files;
    let index = match sha1 {
        Some(sha1) => files.iter().position(|f| f.hashes.sha1 == sha1)?,
        None if files.is_empty() => return None,
        None => files.iter().position(|f| f.primary).unwrap_or(0),
    };
    let file = files.swap_remove(index);
    Some(ModrinthFile {
        project_id: version.project_id,
        version_id: version.id,
        url: file.url,
        sha1: file.hashes.sha1,
        filename: file.filename,
        required: version
            .dependencies
            .into_iter()
            .filter(|dep| dep.dependency_type == "required")
            .filter_map(|dep| dep.project_id)
            .collect(),
    })
}

fn endpoint(segments: &[&str]) -> Url {
    let mut url = Url::parse(API).expect("valid API url");
    url.path_segments_mut()
        .expect("API url has a path")
        .pop_if_empty()
        .extend(segments);
    url
}

pub async fn file_by_sha1(client: &Client, sha1: &str) -> anyhow::Result<Option<ModrinthFile>> {
    let mut url = endpoint(&["version_file", sha1]);
    url.query_pairs_mut().append_pair("algorithm", "sha1");
    let response = client
        .get(url)
        .send()
        .await
        .context("can't reach Modrinth")?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let version: VersionJson = response.error_for_status()?.json().await?;
    Ok(to_file(version, Some(sha1)))
}

/// Project ids of the files Modrinth knows.
pub async fn projects_of_files(
    client: &Client,
    sha1s: &[String],
) -> anyhow::Result<HashSet<String>> {
    if sha1s.is_empty() {
        return Ok(HashSet::new());
    }
    let versions: HashMap<String, VersionJson> = client
        .post(endpoint(&["version_files"]))
        .json(&serde_json::json!({ "hashes": sha1s, "algorithm": "sha1" }))
        .send()
        .await
        .context("can't reach Modrinth")?
        .error_for_status()?
        .json()
        .await?;
    Ok(versions.into_values().map(|v| v.project_id).collect())
}

pub async fn project(client: &Client, id: &str) -> anyhow::Result<Project> {
    let response = client
        .get(endpoint(&["project", id]))
        .send()
        .await
        .context("can't reach Modrinth")?;
    if response.status() == StatusCode::NOT_FOUND {
        bail!("no Modrinth project '{id}'");
    }
    Ok(response.error_for_status()?.json().await?)
}

pub async fn projects(client: &Client, ids: &[&str]) -> anyhow::Result<Vec<Project>> {
    let mut url = endpoint(&["projects"]);
    url.query_pairs_mut()
        .append_pair("ids", &serde_json::to_string(ids)?);
    Ok(client
        .get(url)
        .send()
        .await
        .context("can't reach Modrinth")?
        .error_for_status()?
        .json()
        .await?)
}

/// Versions come newest first.
fn pick(mut versions: Vec<VersionJson>, pin: Option<&str>) -> Option<VersionJson> {
    let index = match pin {
        Some(pin) => versions
            .iter()
            .position(|v| v.id == pin || v.version_number == pin)?,
        None => ["release", "beta", "alpha"]
            .into_iter()
            .find_map(|kind| versions.iter().position(|v| v.version_type == kind))?,
    };
    Some(versions.swap_remove(index))
}

/// The most stable, then newest, version that fits, or the pinned one.
pub async fn pick_file(
    client: &Client,
    slug: &str,
    filter: &Filter<'_>,
    pin: Option<&str>,
) -> anyhow::Result<ModrinthFile> {
    let mut url = endpoint(&["project", slug, "version"]);
    if let Some(game_version) = filter.game_version {
        url.query_pairs_mut()
            .append_pair("game_versions", &serde_json::to_string(&[game_version])?);
    }
    if let Some(loader) = filter.loader {
        url.query_pairs_mut()
            .append_pair("loaders", &serde_json::to_string(&[loader])?);
    }
    let response = client
        .get(url)
        .send()
        .await
        .context("can't reach Modrinth")?;
    if response.status() == StatusCode::NOT_FOUND {
        bail!("no Modrinth project '{slug}'");
    }
    let versions: Vec<VersionJson> = response.error_for_status()?.json().await?;
    let wanted = match pin {
        Some(pin) => format!("version {pin} of {slug}"),
        None => slug.to_string(),
    };
    let version =
        pick(versions, pin).with_context(|| format!("no {wanted} for {}", filter.describe()))?;
    let number = version.version_number.clone();
    to_file(version, None).with_context(|| format!("{slug} {number} has no files"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "uses the network"]
    async fn looks_up_sodium() {
        let client = crate::http_client();
        let filter = Filter {
            game_version: Some("1.21.1"),
            loader: Some("fabric"),
        };
        let file = pick_file(&client, "sodium", &filter, None).await.unwrap();
        assert!(file.filename.ends_with(".jar"));
        let by_hash = file_by_sha1(&client, &file.sha1).await.unwrap().unwrap();
        assert_eq!(by_hash.project_id, file.project_id);
        assert!(
            projects_of_files(&client, std::slice::from_ref(&file.sha1))
                .await
                .unwrap()
                .contains(&file.project_id)
        );
        assert_eq!(
            project(&client, "sodium").await.unwrap().side(),
            Side::Client
        );
        assert!(
            file_by_sha1(&client, &"0".repeat(40))
                .await
                .unwrap()
                .is_none()
        );
    }
}
