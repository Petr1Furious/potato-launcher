use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::PathBuf,
};

use launcher_auth::storage::AccountKey;
use serde::{Deserialize, Serialize};
use url::Url;
use utils::paths::{DataDir, InstanceDirFS, InstancesDir};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn local_new() -> Self {
        Self(format!("local:{}", Uuid::new_v4()))
    }

    pub fn remote(manifest_url: &Url, name: &str) -> Self {
        Self(format!(
            "remote:{}#{}",
            canonical_manifest_url(manifest_url),
            encode_component(name)
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for InstanceId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for InstanceId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

fn canonical_manifest_url(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_string()
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSource {
    pub manifest_url: Url,
    pub name_in_manifest: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    PendingRemote,
    #[default]
    Installed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalInstance {
    pub id: InstanceId,
    #[serde(skip)]
    pub dir_name: String,
    #[serde(default)]
    pub state: InstanceState,
    pub source: Option<RemoteSource>,
    pub last_synced_sha1: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceUserSettings {
    #[serde(default)]
    pub selected_account: Option<AccountKey>,
    #[serde(default)]
    pub account_override: Option<AccountKey>,
    #[serde(default)]
    pub xmx_mb: Option<u64>,
    #[serde(default)]
    pub jvm_flags: Option<String>,
    #[serde(default)]
    pub java_path: Option<String>,
    #[serde(default)]
    pub use_native_glfw: Option<bool>,
    #[serde(default)]
    pub optional_mod_sets: HashMap<String, bool>,
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
    MissingInstance(InstanceId),
    #[error("duplicate local instance id {id} in {first_path:?} and {duplicate_path:?}")]
    DuplicateInstanceId {
        id: InstanceId,
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
        id: InstanceId,
        dir_name: String,
        source: RemoteSource,
        last_synced_sha1: Option<String>,
    ) -> Self {
        Self {
            id,
            dir_name,
            state: InstanceState::Installed,
            source: Some(source),
            last_synced_sha1,
        }
    }

    pub fn new_pending_remote(id: InstanceId, dir_name: String, source: RemoteSource) -> Self {
        Self {
            id,
            dir_name,
            state: InstanceState::PendingRemote,
            source: Some(source),
            last_synced_sha1: None,
        }
    }

    pub fn new_local(dir_name: String) -> Self {
        Self::new_local_with_id(InstanceId::local_new(), dir_name)
    }

    pub fn new_local_with_id(id: InstanceId, dir_name: String) -> Self {
        Self {
            id,
            dir_name,
            state: InstanceState::Installed,
            source: None,
            last_synced_sha1: None,
        }
    }

    pub fn is_installed(&self) -> bool {
        self.state == InstanceState::Installed
    }

    pub fn is_pending_remote(&self) -> bool {
        self.state == InstanceState::PendingRemote
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
        let mut seen_ids = HashMap::<InstanceId, PathBuf>::new();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(InstanceStorageError::ReadInstancesDir)?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(dir_name) = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
            else {
                continue;
            };
            let instance_dir = instance_dir(data_dir, &dir_name);

            let descriptor = instance_dir.local_instance_descriptor_path();
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
            instance.dir_name = dir_name;
            if let Some(first_path) = seen_ids.insert(instance.id.clone(), descriptor.clone()) {
                return Err(InstanceStorageError::DuplicateInstanceId {
                    id: instance.id.clone(),
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

    pub fn get(&self, id: &InstanceId) -> Option<&LocalInstance> {
        self.instances.iter().find(|instance| &instance.id == id)
    }

    pub fn get_mut(&mut self, id: &InstanceId) -> Option<&mut LocalInstance> {
        self.instances
            .iter_mut()
            .find(|instance| &instance.id == id)
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
            .get_mut(&instance.id)
            .ok_or_else(|| InstanceStorageError::MissingInstance(instance.id.clone()))?;
        *existing = instance;
        Ok(())
    }

    pub fn remove(&mut self, id: &InstanceId) -> Option<LocalInstance> {
        let index = self
            .instances
            .iter()
            .position(|instance| &instance.id == id)?;
        Some(self.instances.remove(index))
    }

    pub async fn remove_from_disk(
        &mut self,
        data_dir: &DataDir,
        id: &InstanceId,
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
        let instance_dir = instance_dir(data_dir, &instance.dir_name);
        let dir = instance_dir.to_fs();
        tokio::fs::create_dir_all(&dir).await.map_err(|source| {
            InstanceStorageError::CreateInstanceDir {
                path: dir.clone(),
                source,
            }
        })?;

        let descriptor = instance_dir.local_instance_descriptor_path();
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

pub async fn load_instance_settings(
    instance_dir: &InstanceDirFS,
) -> Result<InstanceUserSettings, std::io::Error> {
    let path = instance_dir.settings_path();
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(InstanceUserSettings::default())
        }
        Err(err) => Err(err),
    }
}

pub async fn save_instance_settings(
    instance_dir: &InstanceDirFS,
    settings: &InstanceUserSettings,
) -> Result<(), std::io::Error> {
    tokio::fs::create_dir_all(instance_dir.to_fs()).await?;
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    tokio::fs::write(instance_dir.settings_path(), bytes).await
}

pub fn allocate_dir_name(taken: &HashSet<&str>, base: &str) -> String {
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

pub fn sanitize_dir_name(name: &str) -> String {
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

fn instance_dir(data_dir: &DataDir, dir_name: &str) -> InstanceDirFS {
    InstancesDir::root()
        .instance_dir(dir_name)
        .with_data_dir(data_dir.clone())
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
            InstanceId::remote(&source.manifest_url, "Vanilla"),
            "Vanilla".to_string(),
            source.clone(),
            Some("first-sha1".to_string()),
        );
        let second = LocalInstance::new_remote(
            InstanceId::remote(&source.manifest_url, "Vanilla Copy"),
            "Vanilla (1)".to_string(),
            source.clone(),
            Some("second-sha1".to_string()),
        );
        let first_id = first.id.clone();
        let second_id = second.id.clone();

        storage.add(&data_dir, first).await.unwrap();
        storage.add(&data_dir, second).await.unwrap();

        let loaded = InstanceStorage::load(&data_dir).await.unwrap();
        let first = loaded.get(&first_id).unwrap();
        let second = loaded.get(&second_id).unwrap();

        assert_eq!(first.dir_name, "Vanilla");
        assert_eq!(second.dir_name, "Vanilla (1)");
        assert_eq!(first.source.as_ref().unwrap().name_in_manifest, "Vanilla");
        assert_eq!(second.source.as_ref().unwrap().name_in_manifest, "Vanilla");
    }

    #[tokio::test]
    async fn duplicate_id_descriptors_fail_explicitly() {
        let data_dir = temp_data_dir();
        let instances = instances_dir(&data_dir);
        tokio::fs::create_dir_all(instances.join("One"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(instances.join("Two"))
            .await
            .unwrap();

        let id = InstanceId::from("local:duplicate");
        let one = LocalInstance {
            id: id.clone(),
            dir_name: "One".to_string(),
            state: InstanceState::Installed,
            source: None,
            last_synced_sha1: None,
        };
        let two = LocalInstance {
            id: id.clone(),
            dir_name: "Two".to_string(),
            state: InstanceState::Installed,
            source: None,
            last_synced_sha1: None,
        };

        let one_dir = InstancesDir::root()
            .instance_dir("One")
            .with_data_dir(data_dir.clone());
        let two_dir = InstancesDir::root()
            .instance_dir("Two")
            .with_data_dir(data_dir.clone());

        tokio::fs::write(
            one_dir.local_instance_descriptor_path(),
            serde_json::to_vec_pretty(&one).unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            two_dir.local_instance_descriptor_path(),
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
    async fn pending_remote_settings_roundtrip() {
        let data_dir = temp_data_dir();
        let mut storage = InstanceStorage::empty();
        let source = RemoteSource {
            manifest_url: Url::parse("https://backend.example/manifest.json").unwrap(),
            name_in_manifest: "Configured".to_string(),
        };
        let id = InstanceId::remote(&source.manifest_url, &source.name_in_manifest);
        let instance =
            LocalInstance::new_pending_remote(id.clone(), "Configured".to_string(), source.clone());
        storage.add(&data_dir, instance).await.unwrap();

        let settings = InstanceUserSettings {
            xmx_mb: Some(4096),
            ..InstanceUserSettings::default()
        };
        let configured_dir = InstancesDir::root()
            .instance_dir("Configured")
            .with_data_dir(data_dir.clone());
        save_instance_settings(&configured_dir, &settings)
            .await
            .unwrap();

        let loaded = InstanceStorage::load(&data_dir).await.unwrap();
        let pending = loaded.get(&id).unwrap();
        assert!(pending.is_pending_remote());
        assert_eq!(pending.source.as_ref(), Some(&source));
        let loaded_settings = load_instance_settings(&configured_dir).await.unwrap();
        assert_eq!(loaded_settings.xmx_mb, Some(4096));
    }

    #[tokio::test]
    async fn remove_from_disk_removes_descriptor_and_directory() {
        let data_dir = temp_data_dir();
        let mut storage = InstanceStorage::empty();
        let instance = LocalInstance::new_local("Local".to_string());
        let id = instance.id.clone();

        storage.add(&data_dir, instance).await.unwrap();
        let dir = instances_dir(&data_dir).join("Local");
        assert!(dir.exists());

        let removed = storage.remove_from_disk(&data_dir, &id).await.unwrap();
        assert!(removed.is_some());
        assert!(!dir.exists());
    }

    fn temp_data_dir() -> DataDir {
        let path = std::env::temp_dir().join(format!("potato-storage-test-{}", Uuid::new_v4()));
        DataDir::new(path)
    }
}
