use std::collections::{BTreeSet, HashSet};

use anyhow::{Context, bail};
use instance::manifest::InstanceManifest;
use serde::{Deserialize, Serialize};
use utils::paths::BaseUrl;

use crate::{
    build::build,
    ctx::Ctx,
    storage::{self, Storage},
    sync, upload,
};

pub const MANIFEST_KEY: &str = "instance_manifest.json";
pub const DEFAULT_GRACE_HOURS: u64 = 2;

#[derive(Serialize, Deserialize, Clone)]
pub struct PublishRecord {
    pub source_commit: String,
    pub repo_root: String,
    #[serde(default)]
    pub order: Option<i64>,
    pub published_at: i64,
    #[serde(default)]
    pub objects: Vec<String>,
}

fn meta_key(id: &str) -> String {
    format!("instances/{id}/meta.json")
}

fn record_key(id: &str) -> String {
    format!("instances/{id}/publish.json")
}

fn history_key(id: &str, time: i64) -> String {
    format!("history/{id}/{time}.json")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

async fn read_record(storage: &dyn Storage, id: &str) -> anyhow::Result<Option<PublishRecord>> {
    match storage.get(&record_key(id)).await? {
        Some(content) => Ok(Some(
            serde_json::from_slice(&content).with_context(|| format!("bad {}", record_key(id)))?,
        )),
        None => Ok(None),
    }
}

async fn read_manifest(storage: &dyn Storage) -> anyhow::Result<InstanceManifest> {
    match storage.get(MANIFEST_KEY).await? {
        Some(content) => serde_json::from_slice(&content).context("bad instance_manifest.json"),
        None => Ok(InstanceManifest { instances: vec![] }),
    }
}

async fn sort_manifest(
    storage: &dyn Storage,
    manifest: &mut InstanceManifest,
    known: (&str, Option<i64>),
) -> anyhow::Result<()> {
    let mut keyed = vec![];
    for entry in manifest.instances.drain(..) {
        let order = if entry.id == known.0 {
            known.1
        } else {
            read_record(storage, &entry.id).await?.and_then(|r| r.order)
        };
        keyed.push((order.unwrap_or(i64::MAX), entry));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    manifest.instances = keyed.into_iter().map(|(_, entry)| entry).collect();
    Ok(())
}

async fn put_json(storage: &dyn Storage, key: &str, value: &impl Serialize) -> anyhow::Result<()> {
    storage.put(key, serde_json::to_vec(value)?).await
}

fn check_previous(
    ctx: &Ctx,
    previous: Option<&PublishRecord>,
    id: &str,
    repo_root: &str,
    head: &str,
) -> anyhow::Result<()> {
    let Some(previous) = previous else {
        return Ok(());
    };
    if previous.repo_root != repo_root {
        bail!("'{id}' is published from another repo, use --force to take it over");
    }
    if !ctx.git.is_ancestor(&previous.source_commit, head)? {
        bail!(
            "the last publish ({}) isn't in your history, pull first",
            &previous.source_commit[..previous.source_commit.len().min(8)]
        );
    }
    Ok(())
}

pub async fn publish(ctx: &mut Ctx, force: bool) -> anyhow::Result<()> {
    let head = ctx.git.head()?;
    let repo_root = ctx.git.root_commit()?;
    if !force {
        let config = ctx.config()?;
        sync::ensure_committed(&ctx.git, |path| config.is_pack_file(path))?;
        let storage = storage::open(&config.publish.target)?;
        let previous = read_record(storage.as_ref(), &config.id).await?;
        check_previous(ctx, previous.as_ref(), &config.id, &repo_root, &head)?;
    }
    let output = Box::pin(build(ctx, &head)).await?;
    let config = &output.config;
    let id = config.id.clone();
    let storage = storage::open(&config.publish.target)?;
    let storage = storage.as_ref();

    let job = async {
        let previous = read_record(storage, &id).await?;
        if !force {
            check_previous(ctx, previous.as_ref(), &id, &repo_root, &head)?;
        }

        let existing = storage
            .list("objects/")
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let uploads = output
            .uploads
            .values()
            .filter(|upload| !existing.contains(&upload.key))
            .collect::<Vec<_>>();
        upload::print(uploads.iter().copied(), ctx.verbose);
        storage
            .put_objects(&upload::pairs(uploads.iter().copied()))
            .await?;
        let missing = output
            .referenced
            .iter()
            .filter(|key| !existing.contains(*key) && !output.uploads.contains_key(*key))
            .collect::<Vec<_>>();
        if let Some(key) = missing.first() {
            bail!("{key} is missing from storage and from your cache");
        }

        let time = now();
        storage
            .put(
                &meta_key(&id),
                serde_json::to_string(&output.metadata)?.into_bytes(),
            )
            .await?;
        if let Some(previous) = &previous {
            put_json(storage, &history_key(&id, time), previous).await?;
        }
        let record = PublishRecord {
            source_commit: head.clone(),
            repo_root: repo_root.clone(),
            order: config.order,
            published_at: time,
            objects: output.referenced.iter().cloned().collect(),
        };
        put_json(storage, &record_key(&id), &record).await?;

        let mut manifest = read_manifest(storage).await?;
        let entry = output
            .metadata
            .to_manifest_entry(&id, &BaseUrl::new(config.publish.base()))?;
        manifest.instances.retain(|existing| existing.id != id);
        manifest.instances.push(entry);
        sort_manifest(storage, &mut manifest, (&id, config.order)).await?;
        put_json(storage, MANIFEST_KEY, &manifest).await?;
        Ok(())
    };
    storage::locked(storage, job).await?;
    println!("published {id} ({})", &head[..8]);
    Ok(())
}

pub async fn unpublish(ctx: &mut Ctx) -> anyhow::Result<()> {
    let config = ctx.config()?;
    let id = config.id.clone();
    let storage = storage::open(&config.publish.target)?;
    let storage = storage.as_ref();
    let job = async {
        let mut manifest = read_manifest(storage).await?;
        let before = manifest.instances.len();
        manifest.instances.retain(|entry| entry.id != id);
        if manifest.instances.len() == before {
            bail!("'{id}' isn't published");
        }
        put_json(storage, MANIFEST_KEY, &manifest).await?;
        if let Some(record) = read_record(storage, &id).await? {
            put_json(storage, &history_key(&id, now()), &record).await?;
        }
        storage.delete(&[record_key(&id), meta_key(&id)]).await?;
        Ok(())
    };
    storage::locked(storage, job).await?;
    println!("unpublished {id}");
    Ok(())
}

pub async fn gc(ctx: &mut Ctx, grace_hours: u64) -> anyhow::Result<()> {
    let config = ctx.config()?;
    let storage = storage::open(&config.publish.target)?;
    let storage = storage.as_ref();
    let (objects, history) =
        storage::locked(storage, collect_garbage(storage, grace_hours)).await?;
    println!("removed {objects} objects and {history} old versions");
    Ok(())
}

pub async fn collect_garbage(
    storage: &dyn Storage,
    grace_hours: u64,
) -> anyhow::Result<(usize, usize)> {
    let cutoff = now() - (grace_hours * 60 * 60) as i64;
    let mut keep = BTreeSet::new();
    for entry in read_manifest(storage).await?.instances {
        if let Some(record) = read_record(storage, &entry.id).await? {
            keep.extend(record.objects);
        }
    }
    let mut old_history = vec![];
    for key in storage.list("history/").await? {
        let replaced_at = key
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|time| time.parse::<i64>().ok())
            .unwrap_or(0);
        if replaced_at < cutoff {
            old_history.push(key);
        } else if let Some(content) = storage.get(&key).await?
            && let Ok(record) = serde_json::from_slice::<PublishRecord>(&content)
        {
            keep.extend(record.objects);
        }
    }
    let garbage = storage
        .list("objects/")
        .await?
        .into_iter()
        .filter(|key| !keep.contains(key))
        .collect::<Vec<_>>();
    storage.delete(&garbage).await?;
    storage.delete(&old_history).await?;
    Ok((garbage.len(), old_history.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manifest_sorted_by_order_then_id() {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            storage::open(url::Url::from_directory_path(dir.path()).unwrap().as_str()).unwrap();
        let storage = storage.as_ref();
        let entry = |id: &str| -> instance::manifest::InstanceManifestEntry {
            serde_json::from_value(serde_json::json!({
                "id": id, "url": "https://x/meta.json", "sha1": "0", "required_java_version": "21"
            }))
            .unwrap()
        };
        for (id, order) in [("b", Some(5)), ("c", None)] {
            let record = PublishRecord {
                source_commit: "c".into(),
                repo_root: "r".into(),
                order,
                published_at: 0,
                objects: vec![],
            };
            put_json(storage, &record_key(id), &record).await.unwrap();
        }
        let mut manifest = InstanceManifest {
            instances: vec![entry("c"), entry("b"), entry("a"), entry("z")],
        };
        sort_manifest(storage, &mut manifest, ("z", Some(1)))
            .await
            .unwrap();
        let ids = manifest
            .instances
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["z", "b", "a", "c"]);
    }

    #[tokio::test]
    async fn gc_keeps_referenced_and_recent() {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            storage::open(url::Url::from_directory_path(dir.path()).unwrap().as_str()).unwrap();
        let storage = storage.as_ref();
        for key in [
            "objects/aa/current",
            "objects/bb/recent-history",
            "objects/cc/old-history",
            "objects/dd/unreferenced",
            "objects/ff/replaced-hours-ago",
        ] {
            storage.put(key, b"x".to_vec()).await.unwrap();
        }

        let manifest = serde_json::json!({"instances": [{
            "id": "a", "url": "https://x/instances/a/meta.json", "sha1": "0", "required_java_version": "21"
        }]});
        storage
            .put(MANIFEST_KEY, serde_json::to_vec(&manifest).unwrap())
            .await
            .unwrap();
        let record = |objects: &[&str]| PublishRecord {
            source_commit: "c".into(),
            repo_root: "r".into(),
            order: None,
            published_at: 0,
            objects: objects.iter().map(|s| s.to_string()).collect(),
        };
        put_json(storage, &record_key("a"), &record(&["objects/aa/current"]))
            .await
            .unwrap();
        put_json(
            storage,
            &history_key("a", now() - 3600),
            &record(&["objects/bb/recent-history"]),
        )
        .await
        .unwrap();
        put_json(
            storage,
            &history_key("a", now() - 30 * 86400),
            &record(&["objects/cc/old-history"]),
        )
        .await
        .unwrap();
        put_json(
            storage,
            &history_key("a", now() - 3 * 3600),
            &record(&["objects/ff/replaced-hours-ago"]),
        )
        .await
        .unwrap();

        let (objects, history) = collect_garbage(storage, DEFAULT_GRACE_HOURS).await.unwrap();
        assert_eq!((objects, history), (3, 2));
        let left = storage.list("objects/").await.unwrap();
        assert_eq!(
            left,
            vec!["objects/aa/current", "objects/bb/recent-history"]
        );
    }
}
