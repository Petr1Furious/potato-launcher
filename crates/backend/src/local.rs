use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use generate::instance::{InstanceGenerator, Loader};
use instance::{
    storage::{InstanceStorage, LocalInstance, sanitize_dir_name},
    version_metadata::OsArch,
};
use launcher_bridge::LocalLoader;
use tokio::sync::mpsc;
use url::Url;
use utils::{
    adaptive_download, files,
    paths::{DataDir, InstancesDir},
    progress::{ProgressStage, ProgressTracker},
};
use uuid::Uuid;

use crate::{
    BackendEvent,
    catalog::BackendCatalogEntry,
    install::{BackendProgressReporter, InstallOutput, ensure_java, install_game_files},
};

#[derive(Clone)]
pub(crate) struct CreateLocalParams {
    pub dir_name: String,
    pub minecraft_version: String,
    pub loader: LocalLoader,
    pub loader_version: Option<String>,
}

pub(crate) struct CreateLocalRequest {
    pub id: Uuid,
    pub dir_name: String,
    pub minecraft_version: String,
    pub loader: LocalLoader,
    pub loader_version: Option<String>,
    pub launcher_dir: PathBuf,
    pub client: reqwest::Client,
    pub frontend: launcher_bridge::FrontendSender,
    pub internal: mpsc::UnboundedSender<BackendEvent>,
}

pub(crate) fn validate_create_local(
    display_name: &str,
    loader: LocalLoader,
    loader_version: &Option<String>,
    storage: &InstanceStorage,
    catalogs: &HashMap<Url, BackendCatalogEntry>,
) -> Result<String, Arc<str>> {
    let name = display_name.trim();
    if name.is_empty() {
        return Err(Arc::from(
            launcher_i18n::notifications::local_instance_name_empty(),
        ));
    }

    let sanitized = sanitize_dir_name(name);
    let taken = storage
        .iter()
        .map(|instance| instance.dir_name.as_str())
        .collect::<HashSet<_>>();
    if taken.contains(sanitized.as_str()) {
        return Err(Arc::from(
            launcher_i18n::notifications::local_instance_name_exists(name.to_string()),
        ));
    }

    for state in catalogs.values() {
        let Some(manifest) = state.manifest() else {
            continue;
        };
        for entry in &manifest.instances {
            if entry.name == name || entry.name == sanitized {
                return Err(Arc::from(
                    launcher_i18n::notifications::local_instance_name_exists(name.to_string()),
                ));
            }
        }
    }

    match loader {
        LocalLoader::Vanilla | LocalLoader::Fabric => {}
        LocalLoader::Forge | LocalLoader::Neoforge => {
            if loader_version
                .as_ref()
                .is_none_or(|version| version.trim().is_empty())
            {
                return Err(Arc::from(
                    launcher_i18n::notifications::local_instance_loader_version_required(),
                ));
            }
        }
    }

    Ok(sanitized)
}

fn map_loader(loader: LocalLoader) -> Loader {
    match loader {
        LocalLoader::Vanilla => Loader::Vanilla,
        LocalLoader::Fabric => Loader::Fabric,
        LocalLoader::Forge => Loader::Forge,
        LocalLoader::Neoforge => Loader::Neoforge,
    }
}

pub(crate) async fn create_local_instance(
    request: CreateLocalRequest,
) -> Result<InstallOutput, Arc<str>> {
    create_local_instance_inner(request)
        .await
        .map_err(|err| Arc::<str>::from(format!("{err:#}")))
}

async fn create_local_instance_inner(request: CreateLocalRequest) -> anyhow::Result<InstallOutput> {
    let data_dir = DataDir::new(request.launcher_dir.clone());
    let instance_dir = InstancesDir::root()
        .instance_dir(&request.dir_name)
        .with_data_dir(data_dir.clone());
    instance_dir.ensure_dir();

    let progress =
        BackendProgressReporter::new(request.id, request.frontend.clone(), request.internal);

    let generate_progress = progress.handle(
        ProgressStage::Metadata,
        launcher_i18n::progress::generating_local_instance(),
    );

    let work_dir = data_dir.as_path().to_path_buf();

    let result = InstanceGenerator {
        client: request.client.clone(),
        instance_name: request.dir_name.clone(),
        minecraft_version: request.minecraft_version.trim().to_string(),
        loader: map_loader(request.loader),
        loader_version: request
            .loader_version
            .as_ref()
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty()),
        include_config: None,
        auth_backend: None,
        default_xmx: None,
    }
    .generate(&instance_dir, &work_dir, &OsArch::All)
    .await?;

    generate_progress.finish();

    let check_progress = progress.handle(
        ProgressStage::Checking,
        launcher_i18n::progress::checking_install_files(),
    );
    let download_tasks =
        files::get_download_tasks(result.check_tasks, check_progress.clone()).await?;
    check_progress.finish();

    let download_progress = progress.handle(
        ProgressStage::Downloading,
        launcher_i18n::progress::downloading_install_files(),
    );
    adaptive_download::download_files(download_tasks, download_progress).await?;

    if !result.copy_tasks.is_empty() {
        let copy_progress = progress.handle(
            ProgressStage::Copying,
            launcher_i18n::progress::copying_files(),
        );
        files::copy_files_if_different(result.copy_tasks, copy_progress).await?;
    }

    let metadata = result.metadata;
    metadata.save(&instance_dir).await?;

    install_game_files(
        &request.client,
        &metadata,
        &data_dir,
        &instance_dir,
        false,
        &progress,
    )
    .await?;

    ensure_java(&metadata, &data_dir, &progress).await?;

    let mut instance = LocalInstance::new_local(request.dir_name);
    instance.id = request.id;

    Ok(InstallOutput { instance })
}
