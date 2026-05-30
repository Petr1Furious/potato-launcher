use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use instance::{
    instance_metadata::InstanceMetadata, manifest::InstanceManifestEntry, storage::LocalInstance,
    storage::RemoteSource,
};
use launcher_bridge::{FrontendSender, MessageToFrontend};
use tokio::sync::mpsc;
use url::Url;
use utils::{
    adaptive_download, files, java,
    paths::{DataDir, InstanceDirFS, InstancesDir, NativesDir},
    progress::{
        ProgressEvent, ProgressHandle, ProgressReporter, ProgressStage, ProgressTracker, Unit,
    },
};
use uuid::Uuid;

use crate::{BackendEvent, catalog::BackendCatalogState, instances::remote_entry_id};

#[derive(Clone)]
pub(crate) struct InstallRequest {
    pub(crate) id: Uuid,
    pub(crate) force_overwrite: bool,
    pub(crate) launcher_dir: PathBuf,
    pub(crate) client: reqwest::Client,
    pub(crate) local_instances: Vec<LocalInstance>,
    pub(crate) catalogs: HashMap<Url, BackendCatalogState>,
    pub(crate) frontend: FrontendSender,
    pub(crate) internal: mpsc::UnboundedSender<BackendEvent>,
}

#[derive(Clone, Debug)]
pub(crate) struct InstallOutput {
    pub(crate) requested_id: Uuid,
    pub(crate) instance: LocalInstance,
}

#[derive(Clone)]
struct InstallPlan {
    view_id: Uuid,
    dir_name: String,
    source: RemoteSource,
    entry: InstanceManifestEntry,
    existing: Option<LocalInstance>,
}

#[derive(Clone)]
pub(crate) struct BackendProgressReporter {
    id: Uuid,
    frontend: FrontendSender,
    internal: mpsc::UnboundedSender<BackendEvent>,
}

impl BackendProgressReporter {
    pub fn new(
        id: Uuid,
        frontend: FrontendSender,
        internal: mpsc::UnboundedSender<BackendEvent>,
    ) -> Self {
        Self {
            id,
            frontend,
            internal,
        }
    }

    pub fn handle(&self, stage: ProgressStage, message: impl Into<String>) -> ProgressHandle<Self> {
        ProgressHandle::new(self.clone(), stage)
            .with_message(message)
            .with_unit(Unit {
                name: "items".to_string(),
                size: 1,
            })
    }
}

impl ProgressReporter for BackendProgressReporter {
    fn event(&self, event: ProgressEvent) {
        let stage = bridge_stage(&event.stage);
        let message = event
            .message
            .clone()
            .unwrap_or_else(|| stage_message(&event.stage).to_string());

        self.frontend.send(MessageToFrontend::InstanceProgress {
            id: self.id,
            stage,
            current: event.current,
            total: event.total,
            message: Arc::<str>::from(message.clone()),
        });
        let _ = self.internal.send(BackendEvent::InstallProgress {
            id: self.id,
            stage,
            current: event.current,
            total: event.total,
            message: Arc::<str>::from(message),
            show_bar: event.total > 1,
        });
    }
}

pub(crate) fn bridge_stage(stage: &ProgressStage) -> launcher_bridge::ProgressStage {
    match stage {
        ProgressStage::Metadata => launcher_bridge::ProgressStage::Metadata,
        ProgressStage::Java => launcher_bridge::ProgressStage::Java,
        ProgressStage::Checking
        | ProgressStage::Downloading
        | ProgressStage::Copying
        | ProgressStage::Extracting
        | ProgressStage::Other(_) => launcher_bridge::ProgressStage::Files,
    }
}

fn stage_message(stage: &ProgressStage) -> String {
    match stage {
        ProgressStage::Checking => launcher_i18n::progress::checking_files().to_string(),
        ProgressStage::Downloading => launcher_i18n::progress::downloading_files().to_string(),
        ProgressStage::Copying => launcher_i18n::progress::copying_files().to_string(),
        ProgressStage::Extracting => launcher_i18n::progress::extracting_files().to_string(),
        ProgressStage::Metadata => launcher_i18n::progress::downloading_metadata().to_string(),
        ProgressStage::Java => launcher_i18n::progress::installing_java().to_string(),
        ProgressStage::Other(_) => launcher_i18n::progress::installing().to_string(),
    }
}

pub(crate) async fn install_instance(request: InstallRequest) -> anyhow::Result<InstallOutput> {
    let plan = resolve_install_plan(request.id, &request.local_instances, &request.catalogs)?;
    let data_dir = DataDir::new(request.launcher_dir.clone());
    let instance_dir = InstancesDir::root()
        .instance_dir(&plan.dir_name)
        .with_data_dir(data_dir.clone());
    instance_dir.ensure_dir();

    let progress =
        BackendProgressReporter::new(plan.view_id, request.frontend.clone(), request.internal);

    let metadata = install_metadata(
        &request.client,
        &plan.entry,
        &instance_dir,
        progress.handle(
            ProgressStage::Metadata,
            launcher_i18n::progress::downloading_metadata(),
        ),
    )
    .await?;

    install_game_files(
        &request.client,
        &metadata,
        &data_dir,
        &instance_dir,
        request.force_overwrite,
        &progress,
    )
    .await?;

    ensure_java(&metadata, &data_dir, &progress).await?;

    let instance = if let Some(mut existing) = plan.existing {
        existing.source = Some(plan.source);
        existing.last_synced_sha1 = Some(plan.entry.sha1);
        existing
    } else {
        LocalInstance::new_remote(plan.dir_name, plan.source, Some(plan.entry.sha1))
    };

    Ok(InstallOutput {
        requested_id: request.id,
        instance,
    })
}

fn resolve_install_plan(
    id: Uuid,
    local_instances: &[LocalInstance],
    catalogs: &HashMap<Url, BackendCatalogState>,
) -> anyhow::Result<InstallPlan> {
    if let Some(local) = local_instances.iter().find(|instance| instance.id == id) {
        return resolve_local_install_plan(local.clone(), catalogs);
    }

    for (url, state) in catalogs {
        let BackendCatalogState::Fetched(manifest) = state else {
            continue;
        };
        for entry in &manifest.instances {
            if remote_entry_id(url, &entry.name) == id {
                let dir_name = allocate_dir_name(local_instances, &entry.name);
                return Ok(InstallPlan {
                    view_id: id,
                    dir_name,
                    source: RemoteSource {
                        manifest_url: url.clone(),
                        name_in_manifest: entry.name.clone(),
                    },
                    entry: entry.clone(),
                    existing: None,
                });
            }
        }
    }

    Err(anyhow::anyhow!(
        "instance {id} was not found in local storage or fetched catalogs"
    ))
}

fn resolve_local_install_plan(
    local: LocalInstance,
    catalogs: &HashMap<Url, BackendCatalogState>,
) -> anyhow::Result<InstallPlan> {
    let source = local
        .source
        .clone()
        .ok_or_else(|| anyhow::anyhow!("local-only instance cannot be updated from a backend"))?;
    let manifest = match catalogs.get(&source.manifest_url) {
        Some(BackendCatalogState::Fetched(manifest)) => manifest,
        Some(BackendCatalogState::Fetching) => {
            return Err(anyhow::anyhow!("backend is still fetching"));
        }
        Some(BackendCatalogState::Offline) => return Err(anyhow::anyhow!("backend is offline")),
        Some(BackendCatalogState::Error(error)) => return Err(anyhow::anyhow!("{error}")),
        Some(BackendCatalogState::NotFetched) | None => {
            return Err(anyhow::anyhow!("backend has not been fetched"));
        }
    };
    let entry = manifest
        .instances
        .iter()
        .find(|entry| entry.name == source.name_in_manifest)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("instance is no longer published by its backend"))?;

    Ok(InstallPlan {
        view_id: local.id,
        dir_name: local.dir_name.clone(),
        source,
        entry,
        existing: Some(local),
    })
}

async fn install_metadata(
    client: &reqwest::Client,
    entry: &InstanceManifestEntry,
    instance_dir: &InstanceDirFS,
    progress: ProgressHandle<BackendProgressReporter>,
) -> anyhow::Result<InstanceMetadata> {
    progress.set_length(1);
    let metadata = InstanceMetadata::read_or_download(client, entry, instance_dir).await?;
    progress.inc(1);
    Ok(metadata)
}

async fn install_game_files(
    client: &reqwest::Client,
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    instance_dir: &InstanceDirFS,
    force_overwrite: bool,
    progress: &BackendProgressReporter,
) -> anyhow::Result<()> {
    let check_tasks = metadata
        .get_install_check_tasks(
            client,
            data_dir,
            &instance_dir.minecraft_dir(),
            force_overwrite,
        )
        .await?;

    let check_progress = progress.handle(
        ProgressStage::Checking,
        launcher_i18n::progress::checking_install_files(),
    );
    let download_tasks = files::get_download_tasks(check_tasks, check_progress).await?;

    let download_progress = progress.handle(
        ProgressStage::Downloading,
        launcher_i18n::progress::downloading_install_files(),
    );
    adaptive_download::download_files(download_tasks, download_progress).await?;

    metadata
        .mark_include_downloads_complete(&instance_dir.minecraft_dir())
        .await?;

    let extract_progress = progress.handle(
        ProgressStage::Extracting,
        launcher_i18n::progress::extracting_native_libraries(),
    );
    extract_natives(metadata, data_dir, extract_progress).await?;

    Ok(())
}

async fn ensure_java(
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    progress: &BackendProgressReporter,
) -> anyhow::Result<()> {
    let java_version = metadata.get_java_version();
    if java::get_java(&java_version, data_dir).await.is_some() {
        progress
            .handle(
                ProgressStage::Java,
                launcher_i18n::progress::java_already_installed(),
            )
            .finish();
        return Ok(());
    }

    java::download_java(
        &java_version,
        data_dir,
        progress.handle(
            ProgressStage::Java,
            launcher_i18n::progress::installing_java_version(java_version.clone()),
        ),
    )
    .await?;
    Ok(())
}

async fn extract_natives(
    metadata: &InstanceMetadata,
    data_dir: &DataDir,
    progress: ProgressHandle<BackendProgressReporter>,
) -> anyhow::Result<()> {
    let native_paths = metadata.get_native_library_paths(data_dir)?;
    progress.set_length(native_paths.len() as u64);
    let natives_dir = NativesDir::for_id(metadata.get_parent_id()?).to_fs(data_dir);
    if natives_dir.exists() {
        tokio::fs::remove_dir_all(&natives_dir).await?;
    }
    tokio::fs::create_dir_all(&natives_dir).await?;

    for native_path in native_paths {
        extract_zip(&native_path, &natives_dir)?;
        progress.inc(1);
    }
    progress.finish();
    Ok(())
}

fn extract_zip(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let file = fs::File::open(src)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(enclosed_name) = entry.enclosed_name() else {
            continue;
        };
        if enclosed_name
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == OsStr::new("META-INF"))
        {
            continue;
        }
        let output_path = dest.join(enclosed_name);
        if entry.is_dir() {
            fs::create_dir_all(&output_path)?;
        } else {
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = fs::File::create(output_path)?;
            io::copy(&mut entry, &mut output)?;
        }
    }
    Ok(())
}

fn allocate_dir_name(local_instances: &[LocalInstance], base: &str) -> String {
    let taken = local_instances
        .iter()
        .map(|instance| instance.dir_name.as_str())
        .collect::<HashSet<_>>();
    let normalized = base.trim();
    let base = if normalized.is_empty() {
        "Instance"
    } else {
        normalized
    };
    if !taken.contains(base) {
        return base.to_string();
    }
    for suffix in 1.. {
        let candidate = format!("{base} ({suffix})");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search should always return")
}

#[cfg(test)]
mod tests {
    use super::*;
    use instance::manifest::InstanceManifest;
    use launcher_bridge::ProgressStage as BridgeProgressStage;

    #[test]
    fn maps_worker_progress_to_bridge_stage() {
        assert_eq!(
            bridge_stage(&ProgressStage::Metadata),
            BridgeProgressStage::Metadata
        );
        assert_eq!(
            bridge_stage(&ProgressStage::Java),
            BridgeProgressStage::Java
        );
        assert_eq!(
            bridge_stage(&ProgressStage::Checking),
            BridgeProgressStage::Files
        );
        assert_eq!(
            bridge_stage(&ProgressStage::Extracting),
            BridgeProgressStage::Files
        );
    }

    #[test]
    fn resolves_remote_install_plan_from_fetched_catalog() {
        let url = Url::parse("https://example.com/manifest.json").unwrap();
        let entry = InstanceManifestEntry {
            name: "Vanilla".to_string(),
            url: Url::parse("https://example.com/vanilla/meta.json").unwrap(),
            sha1: "abc".to_string(),
            auth_backend: None,
        };
        let id = remote_entry_id(&url, &entry.name);
        let catalogs = HashMap::from([(
            url.clone(),
            BackendCatalogState::Fetched(Arc::new(InstanceManifest {
                instances: vec![entry],
            })),
        )]);

        let plan = resolve_install_plan(id, &[], &catalogs).unwrap();

        assert_eq!(plan.view_id, id);
        assert_eq!(plan.dir_name, "Vanilla");
        assert_eq!(plan.source.manifest_url, url);
        assert_eq!(plan.source.name_in_manifest, "Vanilla");
    }

    #[test]
    fn allocates_distinct_directory_names_for_duplicate_display_names() {
        let local_instances = vec![
            LocalInstance::new_local("Vanilla".to_string()),
            LocalInstance::new_local("Vanilla (1)".to_string()),
        ];

        assert_eq!(
            allocate_dir_name(&local_instances, "Vanilla"),
            "Vanilla (2)"
        );
    }

    #[test]
    fn resolves_existing_remote_instance_for_update() {
        let url = Url::parse("https://example.com/manifest.json").unwrap();
        let local = LocalInstance::new_remote(
            "Vanilla".to_string(),
            RemoteSource {
                manifest_url: url.clone(),
                name_in_manifest: "Vanilla".to_string(),
            },
            Some("old".to_string()),
        );
        let entry = InstanceManifestEntry {
            name: "Vanilla".to_string(),
            url: Url::parse("https://example.com/vanilla/meta.json").unwrap(),
            sha1: "new".to_string(),
            auth_backend: None,
        };
        let catalogs = HashMap::from([(
            url.clone(),
            BackendCatalogState::Fetched(Arc::new(InstanceManifest {
                instances: vec![entry],
            })),
        )]);

        let plan = resolve_install_plan(local.id, &[local.clone()], &catalogs).unwrap();

        assert_eq!(plan.view_id, local.id);
        assert_eq!(plan.dir_name, "Vanilla");
        assert!(plan.existing.is_some());
    }
}
