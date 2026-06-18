use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
};

use launcher_auth::storage::AccountKey;
use serde::{Deserialize, Serialize};
use url::Url;
use utils::{
    files,
    instance_id::{allocate_unique_name, slugify_local_dir_name, validate_instance_id},
    paths::{DataDir, InstanceDirFS, InstancesDir},
};

pub use utils::instance_id::InstanceIdError;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct InstanceHandle(String);

impl InstanceHandle {
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

    pub fn recovered_new() -> Self {
        Self(format!("recovered:{}", Uuid::new_v4()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for InstanceHandle {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for InstanceHandle {
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
    pub id_in_manifest: String,
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
    pub handle: InstanceHandle,
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
    #[error("local instance handle not found: {0}")]
    MissingInstanceHandle(InstanceHandle),
    #[error("failed to delete instance directory {path}: {source}")]
    DeleteInstanceDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl LocalInstance {
    pub fn new_remote(
        handle: InstanceHandle,
        dir_name: String,
        source: RemoteSource,
        last_synced_sha1: Option<String>,
    ) -> Self {
        Self {
            handle,
            dir_name,
            state: InstanceState::Installed,
            source: Some(source),
            last_synced_sha1,
        }
    }

    pub fn new_pending_remote(
        handle: InstanceHandle,
        dir_name: String,
        source: RemoteSource,
    ) -> Self {
        Self {
            handle,
            dir_name,
            state: InstanceState::PendingRemote,
            source: Some(source),
            last_synced_sha1: None,
        }
    }

    pub fn new_local(dir_name: String) -> Self {
        Self::new_local_with_handle(InstanceHandle::local_new(), dir_name)
    }

    pub fn new_local_with_handle(handle: InstanceHandle, dir_name: String) -> Self {
        Self {
            handle,
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

/// Result of loading instance storage from disk, including any instances with
/// corrupted descriptors that had to be repaired.
#[derive(Debug, Default)]
pub struct LoadedStorage {
    pub storage: InstanceStorage,
    /// `dir_name`s of instances that had to be recovered
    pub recovered: Vec<String>,
}

impl InstanceStorage {
    pub async fn load(data_dir: &DataDir) -> Result<LoadedStorage, InstanceStorageError> {
        let instances_dir = instances_dir(data_dir);
        if let Err(source) = tokio::fs::create_dir_all(&instances_dir).await {
            return Err(InstanceStorageError::CreateInstancesDir(source));
        }

        let mut read_dir = tokio::fs::read_dir(&instances_dir)
            .await
            .map_err(InstanceStorageError::ReadInstancesDir)?;
        let mut instances = Vec::new();
        let mut recovered = Vec::new();
        let mut seen_handles = HashMap::<InstanceHandle, String>::new();

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

            let parsed = files::read_file_parsed::<LocalInstance>(&descriptor)
                .await
                .map_err(|err| {
                    log::warn!(
                        "Failed to load instance descriptor {}: {err}",
                        descriptor.display()
                    );
                });

            let instance = match parsed {
                Err(()) => {
                    recovered.push(dir_name.clone());
                    recover_descriptor(&descriptor, &dir_name).await?
                }
                Ok(mut instance) => {
                    instance.dir_name = dir_name.clone();
                    if let Some(first_dir) = seen_handles.get(&instance.handle) {
                        log::warn!(
                            "Instance {dir_name} reuses handle {} already loaded from {first_dir}; assigning a new handle",
                            instance.handle
                        );
                        instance.handle = InstanceHandle::recovered_new();
                        write_descriptor(&descriptor, &instance).await?;
                        recovered.push(dir_name.clone());
                    }
                    instance
                }
            };
            seen_handles.insert(instance.handle.clone(), dir_name);
            instances.push(instance);
        }

        instances.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
        Ok(LoadedStorage {
            storage: Self { instances },
            recovered,
        })
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

    pub fn get(&self, handle: &InstanceHandle) -> Option<&LocalInstance> {
        self.instances
            .iter()
            .find(|instance| &instance.handle == handle)
    }

    pub fn get_mut(&mut self, handle: &InstanceHandle) -> Option<&mut LocalInstance> {
        self.instances
            .iter_mut()
            .find(|instance| &instance.handle == handle)
    }

    pub fn allocate_dir_name(&self, base: &str) -> Result<String, InstanceIdError> {
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
            .get_mut(&instance.handle)
            .ok_or_else(|| InstanceStorageError::MissingInstanceHandle(instance.handle.clone()))?;
        *existing = instance;
        Ok(())
    }

    pub fn remove(&mut self, handle: &InstanceHandle) -> Option<LocalInstance> {
        let index = self
            .instances
            .iter()
            .position(|instance| &instance.handle == handle)?;
        Some(self.instances.remove(index))
    }

    pub async fn remove_from_disk(
        &mut self,
        data_dir: &DataDir,
        handle: &InstanceHandle,
    ) -> Result<Option<LocalInstance>, InstanceStorageError> {
        let Some(instance) = self.remove(handle) else {
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
        files::write_file_atomic(&descriptor, &bytes)
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
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    files::write_file_atomic(&instance_dir.settings_path(), &bytes).await
}

pub fn allocate_dir_name(taken: &HashSet<&str>, id: &str) -> Result<String, InstanceIdError> {
    validate_instance_id(id)?;
    Ok(allocate_unique_name(taken, id))
}

pub fn allocate_local_dir_name(taken: &HashSet<&str>, display_name: &str) -> String {
    let base = slugify_local_dir_name(display_name);
    allocate_unique_name(taken, &base)
}

/// Backup the corrupted descriptor and write a minimal one in its place
async fn recover_descriptor(
    descriptor: &Path,
    dir_name: &str,
) -> Result<LocalInstance, InstanceStorageError> {
    if let Err(err) = files::backup_corrupt_file(descriptor).await {
        log::warn!(
            "Failed to back up corrupt descriptor {}: {err}",
            descriptor.display()
        );
    }
    let instance =
        LocalInstance::new_local_with_handle(InstanceHandle::recovered_new(), dir_name.to_string());
    write_descriptor(descriptor, &instance).await?;
    Ok(instance)
}

async fn write_descriptor(
    descriptor: &Path,
    instance: &LocalInstance,
) -> Result<(), InstanceStorageError> {
    let bytes =
        serde_json::to_vec_pretty(instance).map_err(InstanceStorageError::SerializeDescriptor)?;
    files::write_file_atomic(descriptor, &bytes)
        .await
        .map_err(|source| InstanceStorageError::WriteDescriptor {
            path: descriptor.to_path_buf(),
            source,
        })
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
    fn allocate_dir_name_resolves_conflicts_with_numeric_suffixes() {
        let taken = HashSet::from(["vanilla", "vanilla-2"]);

        assert_eq!(allocate_dir_name(&taken, "vanilla").unwrap(), "vanilla-3");
        assert_eq!(
            allocate_dir_name(&HashSet::new(), "minigames").unwrap(),
            "minigames"
        );
    }

    #[test]
    fn allocate_local_dir_name_slugifies_display_names() {
        let taken = HashSet::from(["my-pack", "my-pack-2"]);
        assert_eq!(allocate_local_dir_name(&taken, "My Pack"), "my-pack-3");
        assert_eq!(
            allocate_local_dir_name(&HashSet::new(), "Мой Пак"),
            "moi-pak"
        );
    }

    #[tokio::test]
    async fn load_save_roundtrip_keeps_display_source_separate_from_dir_name() {
        let data_dir = temp_data_dir();
        let mut storage = InstanceStorage::empty();
        let source = RemoteSource {
            manifest_url: Url::parse("https://backend.example/manifest.json").unwrap(),
            id_in_manifest: "vanilla".to_string(),
        };

        let first = LocalInstance::new_remote(
            InstanceHandle::remote(&source.manifest_url, "vanilla"),
            "vanilla".to_string(),
            source.clone(),
            Some("first-sha1".to_string()),
        );
        let second = LocalInstance::new_remote(
            InstanceHandle::remote(&source.manifest_url, "vanilla-copy"),
            "vanilla-copy".to_string(),
            source.clone(),
            Some("second-sha1".to_string()),
        );
        let first_handle = first.handle.clone();
        let second_handle = second.handle.clone();

        storage.add(&data_dir, first).await.unwrap();
        storage.add(&data_dir, second).await.unwrap();

        let loaded = InstanceStorage::load(&data_dir).await.unwrap().storage;
        let first = loaded.get(&first_handle).unwrap();
        let second = loaded.get(&second_handle).unwrap();

        assert_eq!(first.dir_name, "vanilla");
        assert_eq!(second.dir_name, "vanilla-copy");
        assert_eq!(first.source.as_ref().unwrap().id_in_manifest, "vanilla");
        assert_eq!(second.source.as_ref().unwrap().id_in_manifest, "vanilla");
    }

    #[tokio::test]
    async fn duplicate_handle_descriptors_are_recovered() {
        let data_dir = temp_data_dir();
        let instances = instances_dir(&data_dir);
        tokio::fs::create_dir_all(instances.join("One"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(instances.join("Two"))
            .await
            .unwrap();

        let handle = InstanceHandle::from("local:duplicate");
        let one = LocalInstance {
            handle: handle.clone(),
            dir_name: "One".to_string(),
            state: InstanceState::Installed,
            source: None,
            last_synced_sha1: None,
        };
        let two = LocalInstance {
            handle: handle.clone(),
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

        let loaded = InstanceStorage::load(&data_dir).await.unwrap();
        // the collision is broken by giving one of them a fresh local handle instead of dropping it
        assert_eq!(loaded.storage.all().len(), 2);
        assert_eq!(loaded.recovered.len(), 1);
        let handles: HashSet<_> = loaded
            .storage
            .iter()
            .map(|instance| instance.handle.clone())
            .collect();
        assert_eq!(handles.len(), 2);

        // Reloading is stable: the rewritten descriptors are no longer in conflict
        let reloaded = InstanceStorage::load(&data_dir).await.unwrap();
        assert_eq!(reloaded.storage.all().len(), 2);
        assert!(reloaded.recovered.is_empty());
    }

    #[tokio::test]
    async fn corrupt_descriptor_is_recovered_as_local_instance() {
        let data_dir = temp_data_dir();
        let instances = instances_dir(&data_dir);
        tokio::fs::create_dir_all(instances.join("Broken"))
            .await
            .unwrap();
        let broken_dir = InstancesDir::root()
            .instance_dir("Broken")
            .with_data_dir(data_dir.clone());
        let descriptor = broken_dir.local_instance_descriptor_path();
        tokio::fs::write(&descriptor, b"{ this is not valid json")
            .await
            .unwrap();

        let loaded = InstanceStorage::load(&data_dir).await.unwrap();
        assert_eq!(loaded.recovered, vec!["Broken".to_string()]);
        let recovered = loaded
            .storage
            .iter()
            .find(|instance| instance.dir_name == "Broken")
            .unwrap();
        assert!(recovered.is_installed());
        assert!(recovered.source.is_none());

        // The corrupted file is preserved nearby
        let mut entries = tokio::fs::read_dir(broken_dir.to_fs()).await.unwrap();
        let mut has_backup = false;
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry
                .file_name()
                .to_string_lossy()
                .contains("local_instance.json.corrupt-")
            {
                has_backup = true;
            }
        }
        assert!(has_backup);

        // Subsequent loads are clean because the descriptor was rewritten
        let reloaded = InstanceStorage::load(&data_dir).await.unwrap();
        assert!(reloaded.recovered.is_empty());
    }

    #[tokio::test]
    async fn pending_remote_settings_roundtrip() {
        let data_dir = temp_data_dir();
        let mut storage = InstanceStorage::empty();
        let source = RemoteSource {
            manifest_url: Url::parse("https://backend.example/manifest.json").unwrap(),
            id_in_manifest: "Configured".to_string(),
        };
        let handle = InstanceHandle::remote(&source.manifest_url, &source.id_in_manifest);
        let instance = LocalInstance::new_pending_remote(
            handle.clone(),
            "Configured".to_string(),
            source.clone(),
        );
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

        let loaded = InstanceStorage::load(&data_dir).await.unwrap().storage;
        let pending = loaded.get(&handle).unwrap();
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
        let handle = instance.handle.clone();

        storage.add(&data_dir, instance).await.unwrap();
        let dir = instances_dir(&data_dir).join("Local");
        assert!(dir.exists());

        let removed = storage.remove_from_disk(&data_dir, &handle).await.unwrap();
        assert!(removed.is_some());
        assert!(!dir.exists());
    }

    fn temp_data_dir() -> DataDir {
        let path = std::env::temp_dir().join(format!("potato-storage-test-{}", Uuid::new_v4()));
        DataDir::new(path)
    }
}
