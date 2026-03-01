use generate::instance::{IncludeConfig, IncludeRule, InstanceGenerator, Loader};
use instance::manifest::{AuthlibInjectorDownload, DEFAULT_AUTHLIB_INJECTOR, InstanceManifest};
use instance::version_metadata::OsArch;
use launcher_auth::providers::AuthProviderConfig;
use log::{info, warn};
use relative_path::RelativePathBuf;
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tokio::fs;
use url::Url;
use utils::adaptive_download;
use utils::{
    files::{self, CheckTask, CopyTask},
    paths::{BaseUrl, DataDir, InstancesDir, LibrariesDir},
    utils::get_unique_name,
};

use crate::progress::TerminalProgressBar;

fn vanilla() -> String {
    "vanilla".to_string()
}

#[derive(Deserialize)]
pub struct IncludeRuleSpec {
    pub path: String,
    #[serde(default = "yes")]
    pub overwrite: bool,
    #[serde(default = "yes")]
    pub delete_extra: bool,
    #[serde(default)]
    pub recursive: bool,
}

fn yes() -> bool {
    true
}

const INSTANCE_MANIFEST_FILENAME: &str = "instance_manifest.json";

#[derive(Deserialize)]
pub struct InstanceSpec {
    pub name: String,
    pub minecraft_version: String,

    #[serde(default = "vanilla")]
    pub loader_name: String,

    pub loader_version: Option<String>,

    #[serde(default)]
    pub include: Vec<IncludeRuleSpec>,

    pub include_from: Option<String>,

    pub auth_backend: Option<AuthProviderConfig>,

    pub recommended_xmx: Option<String>,
}

#[derive(Deserialize)]
pub struct Spec {
    pub download_server_base: Url,

    #[serde(default)]
    pub replace_download_urls: bool,
    pub instances: Vec<InstanceSpec>,
}

impl Spec {
    pub async fn from_file(path: &Path) -> anyhow::Result<Spec> {
        let content = fs::read_to_string(path).await?;
        let spec = serde_json::from_str(&content)?;
        Ok(spec)
    }

    pub async fn generate(
        self,
        output_dir: &Path,
        work_dir: &Path,
        _delete_remote_instances: Option<&HashSet<String>>,
    ) -> anyhow::Result<()> {
        let data_dir = DataDir::new(output_dir.to_path_buf());
        let download_server_base = BaseUrl::new(self.download_server_base.clone());
        let client = reqwest::Client::new();
        let mut existing_instance_names = HashSet::new();
        let mut all_check_tasks: Vec<CheckTask> = vec![];
        let mut all_copy_tasks: Vec<CopyTask> = vec![];
        let mut all_other_generated_files: Vec<PathBuf> = vec![];
        let manifest_path = output_dir.join(INSTANCE_MANIFEST_FILENAME);

        let mut all_metadata = vec![];
        let authlib_injector_custom_url = LibrariesDir::root()
            .authlib_injector_path()
            .to_url(&download_server_base);
        all_check_tasks.push(CheckTask {
            url: DEFAULT_AUTHLIB_INJECTOR.url.clone(),
            remote_sha1: None,
            path: LibrariesDir::root()
                .authlib_injector_path()
                .to_fs(&data_dir),
        });

        for instance in self.instances {
            let unique_name = get_unique_name(&existing_instance_names, &instance.name);
            if unique_name != instance.name {
                warn!(
                    "Duplicate instance name \"{}\"; using \"{}\"",
                    instance.name, unique_name
                );
            }
            existing_instance_names.insert(unique_name.clone());

            let loader = match instance.loader_name.as_str() {
                "vanilla" => Loader::Vanilla,
                "fabric" => Loader::Fabric,
                "forge" => Loader::Forge,
                "neoforge" => Loader::Neoforge,
                other => {
                    return Err(anyhow::anyhow!("Unsupported loader name: {other}"));
                }
            };

            let include_rules = instance
                .include
                .iter()
                .map(|rule| IncludeRule {
                    path: RelativePathBuf::from(rule.path.as_str()),
                    overwrite: rule.overwrite,
                    delete_extra: rule.delete_extra,
                    recursive: rule.recursive,
                })
                .collect::<Vec<_>>();

            let include_config = if self.replace_download_urls
                || instance.include_from.is_some()
                || !include_rules.is_empty()
            {
                Some(IncludeConfig {
                    include_rules,
                    include_from: instance.include_from.as_ref().map(PathBuf::from),
                    download_server_base: download_server_base.clone(),
                    replace_download_urls: self.replace_download_urls,
                })
            } else {
                None
            };

            if !self.replace_download_urls
                && include_config.is_none()
                && instance.include_from.is_some()
            {
                warn!("include_from set but include rules are empty");
            }

            let instance_rel = InstancesDir::root().instance_dir(&unique_name);
            let instance_dir = instance_rel.with_data_dir(data_dir.clone());
            instance_dir.ensure_dir();

            let result = InstanceGenerator {
                client: client.clone(),
                instance_name: unique_name.clone(),
                minecraft_version: instance.minecraft_version.clone(),
                loader,
                loader_version: instance.loader_version.clone(),
                include_config,
                auth_backend: instance.auth_backend.clone(),
                default_xmx: instance.recommended_xmx.clone(),
            }
            .generate(&instance_dir, work_dir, &OsArch::All)
            .await?;

            info!(
                "Instance \"{}\": {} check tasks, {} copy tasks, {} generated files",
                unique_name,
                result.check_tasks.len(),
                result.copy_tasks.len(),
                result.other_generated_files.len()
            );

            result.metadata.save(&instance_dir).await?;
            all_metadata.push(result.metadata);
            all_other_generated_files.extend(result.other_generated_files);
            all_other_generated_files.push(instance_dir.meta_path());
            all_check_tasks.extend(result.check_tasks);
            all_copy_tasks.extend(result.copy_tasks);

            info!("Finished generating instance {}", &unique_name);
        }

        let deduped_check_tasks = files::dedup_check_tasks(all_check_tasks);
        let deduped_copy_tasks = files::dedup_copy_tasks(all_copy_tasks);
        info!(
            "Running {} deduplicated check tasks and {} deduplicated copy tasks",
            deduped_check_tasks.len(),
            deduped_copy_tasks.len()
        );

        let mut keep_files: HashSet<PathBuf> = all_other_generated_files
            .into_iter()
            .collect::<HashSet<_>>();
        keep_files.extend(deduped_check_tasks.iter().map(|task| task.path.clone()));
        keep_files.extend(deduped_copy_tasks.iter().map(|task| task.target.clone()));
        keep_files.insert(manifest_path.clone());

        let check_progress = TerminalProgressBar::new("Checking files");
        let download_tasks = files::get_download_tasks(deduped_check_tasks, check_progress).await?;

        info!("Got {} download tasks", download_tasks.len());

        let download_progress = TerminalProgressBar::new("Downloading files");
        adaptive_download::download_files(download_tasks, download_progress).await?;

        let copy_progress = TerminalProgressBar::new("Copying files");
        files::copy_files_if_different(deduped_copy_tasks, copy_progress).await?;

        let authlib_injector_sha1 = files::hash_file(
            &LibrariesDir::root()
                .authlib_injector_path()
                .to_fs(&data_dir),
        )
        .await?;
        let manifest = InstanceManifest {
            instances: all_metadata
                .iter()
                .map(|metadata| {
                    metadata.get_manifest_entry(metadata.get_name(), &download_server_base)
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            authlib_injector: Some(AuthlibInjectorDownload {
                url: authlib_injector_custom_url.clone(),
                sha1: Some(authlib_injector_sha1),
            }),
        };
        manifest.save_to_file(&manifest_path).await?;

        let mut public_manifest_base = self.download_server_base.clone();
        if !public_manifest_base.path().ends_with('/') {
            public_manifest_base.set_path(&format!("{}/", public_manifest_base.path()));
        }
        let manifest_url = public_manifest_base.join(INSTANCE_MANIFEST_FILENAME)?;
        info!("Instance manifest now should be available at '{manifest_url}'");

        let retain_stats =
            files::retain_only_files_and_parents(data_dir.as_path(), &keep_files).await?;
        info!(
            "Cleanup done: removed {} files, {} dirs; kept {} files",
            retain_stats.removed_files, retain_stats.removed_dirs, retain_stats.keep_files
        );

        Ok(())
    }
}
