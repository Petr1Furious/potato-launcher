use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use url::Url;
use utils::paths::{DataDir, InstancesDir};
use uuid::Uuid;

const LOCAL_INSTANCE_FILE: &str = "local_instance.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSource {
    pub manifest_url: Url,
    pub name_in_manifest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalInstance {
    pub id: Uuid,
    pub dir_name: String,
    pub source: Option<RemoteSource>,
    pub last_synced_sha1: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct InstanceStorage {
    instances: Vec<LocalInstance>,
}

#[derive(thiserror::Error, Debug)]
pub enum InstanceStorageError {
    #[error("failed to create instances directory: {0}")]
    CreateInstancesDir(#[source] std::io::Error),
    #[error("failed to read instances directory: {0}")]
    ReadInstancesDir(#[source] std::io::Error),
    #[error("failed to read local instance descriptor {path}: {source}")]
    ReadDescriptor {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse local instance descriptor {path}: {source}")]
    ParseDescriptor {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize local instance descriptor: {0}")]
    SerializeDescriptor(#[source] serde_json::Error),
    #[error("failed to create instance directory {path}: {source}")]
    CreateInstanceDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write local instance descriptor {path}: {source}")]
    WriteDescriptor {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("local instance id not found: {0}")]
    MissingInstance(Uuid),
    #[error("duplicate local instance id {id} in {first_path:?} and {duplicate_path:?}")]
    DuplicateInstanceId {
        id: Uuid,
        first_path: PathBuf,
        duplicate_path: PathBuf,
    },
    #[error("failed to delete instance directory {path}: {source}")]
    DeleteInstanceDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl LocalInstance {
    pub fn new_remote(
        dir_name: String,
        source: RemoteSource,
        last_synced_sha1: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            dir_name,
            source: Some(source),
            last_synced_sha1,
        }
    }

    pub fn new_local(dir_name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            dir_name,
            source: None,
            last_synced_sha1: None,
        }
    }
}

impl InstanceStorage {
    pub async fn load(data_dir: &DataDir) -> Result<Self, InstanceStorageError> {
        let instances_dir = instances_dir(data_dir);
        if let Err(source) = tokio::fs::create_dir_all(&instances_dir).await {
            return Err(InstanceStorageError::CreateInstancesDir(source));
        }

        let mut read_dir = tokio::fs::read_dir(&instances_dir)
            .await
            .map_err(InstanceStorageError::ReadInstancesDir)?;
        let mut instances = Vec::new();
        let mut seen_ids = HashMap::<Uuid, PathBuf>::new();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(InstanceStorageError::ReadInstancesDir)?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let descriptor = path.join(LOCAL_INSTANCE_FILE);
            if !descriptor.exists() {
                continue;
            }

            let bytes = tokio::fs::read(&descriptor).await.map_err(|source| {
                InstanceStorageError::ReadDescriptor {
                    path: descriptor.clone(),
                    source,
                }
            })?;
            let mut instance =
                serde_json::from_slice::<LocalInstance>(&bytes).map_err(|source| {
                    InstanceStorageError::ParseDescriptor {
                        path: descriptor.clone(),
                        source,
                    }
                })?;
            instance.dir_name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or(instance.dir_name);
            if let Some(first_path) = seen_ids.insert(instance.id, descriptor.clone()) {
                return Err(InstanceStorageError::DuplicateInstanceId {
                    id: instance.id,
                    first_path,
                    duplicate_path: descriptor,
                });
            }
            instances.push(instance);
        }

        instances.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
        Ok(Self { instances })
    }

    pub fn empty() -> Self {
        Self {
            instances: Vec::new(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &LocalInstance> {
        self.instances.iter()
    }

    pub fn all(&self) -> &[LocalInstance] {
        &self.instances
    }

    pub fn get(&self, id: Uuid) -> Option<&LocalInstance> {
        self.instances.iter().find(|instance| instance.id == id)
    }

    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut LocalInstance> {
        self.instances.iter_mut().find(|instance| instance.id == id)
    }

    pub fn allocate_dir_name(&self, base: &str) -> String {
        let taken = self
            .instances
            .iter()
            .map(|instance| instance.dir_name.as_str())
            .collect::<HashSet<_>>();
        allocate_dir_name(&taken, base)
    }

    pub async fn add(
        &mut self,
        data_dir: &DataDir,
        instance: LocalInstance,
    ) -> Result<(), InstanceStorageError> {
        self.save_instance(data_dir, &instance).await?;
        self.instances.push(instance);
        self.instances.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
        Ok(())
    }

    pub async fn update(
        &mut self,
        data_dir: &DataDir,
        instance: LocalInstance,
    ) -> Result<(), InstanceStorageError> {
        self.save_instance(data_dir, &instance).await?;
        let existing = self
            .get_mut(instance.id)
            .ok_or(InstanceStorageError::MissingInstance(instance.id))?;
        *existing = instance;
        Ok(())
    }

    pub fn remove(&mut self, id: Uuid) -> Option<LocalInstance> {
        let index = self
            .instances
            .iter()
            .position(|instance| instance.id == id)?;
        Some(self.instances.remove(index))
    }

    pub async fn remove_from_disk(
        &mut self,
        data_dir: &DataDir,
        id: Uuid,
    ) -> Result<Option<LocalInstance>, InstanceStorageError> {
        let Some(instance) = self.remove(id) else {
            return Ok(None);
        };
        let dir = instances_dir(data_dir).join(&instance.dir_name);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(Some(instance)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Some(instance)),
            Err(source) => Err(InstanceStorageError::DeleteInstanceDir { path: dir, source }),
        }
    }

    async fn save_instance(
        &self,
        data_dir: &DataDir,
        instance: &LocalInstance,
    ) -> Result<(), InstanceStorageError> {
        let dir = instances_dir(data_dir).join(&instance.dir_name);
        tokio::fs::create_dir_all(&dir).await.map_err(|source| {
            InstanceStorageError::CreateInstanceDir {
                path: dir.clone(),
                source,
            }
        })?;

        let descriptor = dir.join(LOCAL_INSTANCE_FILE);
        let bytes = serde_json::to_vec_pretty(instance)
            .map_err(InstanceStorageError::SerializeDescriptor)?;
        tokio::fs::write(&descriptor, bytes)
            .await
            .map_err(|source| InstanceStorageError::WriteDescriptor {
                path: descriptor,
                source,
            })
    }
}

pub fn allocate_dir_name<'a>(taken: &HashSet<&'a str>, base: &str) -> String {
    let base = sanitize_dir_name(base);
    if !taken.contains(base.as_str()) {
        return base;
    }

    for i in 1.. {
        let candidate = format!("{base} ({i})");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("usize counter should not overflow while allocating an instance directory")
}

fn sanitize_dir_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();

    if sanitized.is_empty() {
        "Instance".to_string()
    } else {
        sanitized
    }
}

fn instances_dir(data_dir: &DataDir) -> PathBuf {
    InstancesDir::root().to_fs(data_dir)
}

pub fn local_instance_descriptor_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(LOCAL_INSTANCE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_dir_name_sanitizes_and_resolves_conflicts() {
        let taken = HashSet::from(["Vanilla", "Vanilla (1)", "Bad_Name"]);

        assert_eq!(allocate_dir_name(&taken, "Vanilla"), "Vanilla (2)");
        assert_eq!(allocate_dir_name(&taken, "Bad/Name"), "Bad_Name (1)");
        assert_eq!(allocate_dir_name(&HashSet::new(), "..."), "Instance");
        assert_eq!(allocate_dir_name(&HashSet::new(), "  "), "Instance");
    }

    #[tokio::test]
    async fn load_save_roundtrip_keeps_display_source_separate_from_dir_name() {
        let data_dir = temp_data_dir();
        let mut storage = InstanceStorage::empty();
        let source = RemoteSource {
            manifest_url: Url::parse("https://backend.example/manifest.json").unwrap(),
            name_in_manifest: "Vanilla".to_string(),
        };

        let first = LocalInstance::new_remote(
            "Vanilla".to_string(),
            source.clone(),
            Some("first-sha1".to_string()),
        );
        let second = LocalInstance::new_remote(
            "Vanilla (1)".to_string(),
            source.clone(),
            Some("second-sha1".to_string()),
        );
        let first_id = first.id;
        let second_id = second.id;

        storage.add(&data_dir, first).await.unwrap();
        storage.add(&data_dir, second).await.unwrap();

        let loaded = InstanceStorage::load(&data_dir).await.unwrap();
        let first = loaded.get(first_id).unwrap();
        let second = loaded.get(second_id).unwrap();

        assert_eq!(first.dir_name, "Vanilla");
        assert_eq!(second.dir_name, "Vanilla (1)");
        assert_eq!(first.source.as_ref().unwrap().name_in_manifest, "Vanilla");
        assert_eq!(second.source.as_ref().unwrap().name_in_manifest, "Vanilla");
    }

    #[tokio::test]
    async fn duplicate_uuid_descriptors_fail_explicitly() {
        let data_dir = temp_data_dir();
        let instances = instances_dir(&data_dir);
        tokio::fs::create_dir_all(instances.join("One"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(instances.join("Two"))
            .await
            .unwrap();

        let id = Uuid::new_v4();
        let one = LocalInstance {
            id,
            dir_name: "One".to_string(),
            source: None,
            last_synced_sha1: None,
        };
        let two = LocalInstance {
            id,
            dir_name: "Two".to_string(),
            source: None,
            last_synced_sha1: None,
        };

        tokio::fs::write(
            local_instance_descriptor_path(&instances.join("One")),
            serde_json::to_vec_pretty(&one).unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            local_instance_descriptor_path(&instances.join("Two")),
            serde_json::to_vec_pretty(&two).unwrap(),
        )
        .await
        .unwrap();

        let err = InstanceStorage::load(&data_dir).await.unwrap_err();
        assert!(matches!(
            err,
            InstanceStorageError::DuplicateInstanceId {
                id: duplicate_id,
                ..
            } if duplicate_id == id
        ));
    }

    #[tokio::test]
    async fn remove_from_disk_removes_descriptor_and_directory() {
        let data_dir = temp_data_dir();
        let mut storage = InstanceStorage::empty();
        let instance = LocalInstance::new_local("Local".to_string());
        let id = instance.id;

        storage.add(&data_dir, instance).await.unwrap();
        let dir = instances_dir(&data_dir).join("Local");
        assert!(dir.exists());

        let removed = storage.remove_from_disk(&data_dir, id).await.unwrap();
        assert!(removed.is_some());
        assert!(!dir.exists());
    }

    fn temp_data_dir() -> DataDir {
        let path = std::env::temp_dir().join(format!("potato-storage-test-{}", Uuid::new_v4()));
        DataDir::new(path)
    }
}
