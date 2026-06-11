use generate::instance::{InstanceGenerator, InstanceSpec, RemoteConfig};
use instance::{
    authlib::mirror_authlib_injector_library, manifest::InstanceManifest, version_metadata::OsArch,
};
use log::{info, warn};
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
    get_unique_name,
    paths::{BaseUrl, DataDir, InstancesDir},
    progress::ProgressStage,
};

use crate::progress::TerminalProgress;

const INSTANCE_MANIFEST_FILENAME: &str = "instance_manifest.json";

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

    pub async fn generate(self, output_dir: &Path, work_dir: &Path) -> anyhow::Result<()> {
        let data_dir = DataDir::new(output_dir.to_path_buf());
        let download_server_base = BaseUrl::new(self.download_server_base.clone());
        let client = reqwest::Client::new();
        let mut existing_instance_names = HashSet::new();
        let mut all_check_tasks: Vec<CheckTask> = vec![];
        let mut all_copy_tasks: Vec<CopyTask> = vec![];
        let mut all_other_generated_files: Vec<PathBuf> = vec![];
        let manifest_path = output_dir.join(INSTANCE_MANIFEST_FILENAME);

        let mut all_metadata = vec![];

        for instance in self.instances {
            let unique_name = get_unique_name(&existing_instance_names, &instance.name);
            if unique_name != instance.name {
                warn!(
                    "Duplicate instance name \"{}\"; using \"{}\"",
                    instance.name, unique_name
                );
            }
            existing_instance_names.insert(unique_name.clone());

            let remote_config = RemoteConfig {
                download_server_base: download_server_base.clone(),
                replace_download_urls: self.replace_download_urls,
            };

            if !self.replace_download_urls
                && instance.source_root.is_some()
                && instance.content_rules.is_empty()
            {
                warn!("source_root set but content rules are empty");
            }

            let instance_rel = InstancesDir::root().instance_dir(&unique_name);
            let instance_dir = instance_rel.with_data_dir(data_dir.clone());
            instance_dir.ensure_dir();

            let result = InstanceGenerator {
                client: client.clone(),
                spec: instance.clone(),
                remote_config: Some(remote_config),
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

            all_metadata.push(result.metadata);
            all_other_generated_files.extend(result.other_generated_files);
            // add instance metadata path to the list of other generated files
            // even though we save it later, after downloads
            // this is needed because we will know authlib
            all_other_generated_files.push(instance_dir.meta_path());
            all_check_tasks.extend(result.check_tasks);
            all_copy_tasks.extend(result.copy_tasks);

            info!("Finished generating instance {}", &unique_name);
        }

        let deduped_check_tasks = files::dedup_check_tasks(all_check_tasks);
        let deduped_copy_tasks = files::dedup_copy_tasks(all_copy_tasks);
        info!(
            "Running {} check tasks and {} copy tasks",
            deduped_check_tasks.len(),
            deduped_copy_tasks.len()
        );

        let mut keep_files: HashSet<PathBuf> = all_other_generated_files
            .into_iter()
            .collect::<HashSet<_>>();
        keep_files.extend(deduped_check_tasks.iter().map(|task| task.path.clone()));
        keep_files.extend(deduped_copy_tasks.iter().map(|task| task.target.clone()));
        keep_files.insert(manifest_path.clone());

        let check_progress =
            TerminalProgress::new().handle(ProgressStage::Checking, "Checking files");
        let download_tasks = files::get_download_tasks(deduped_check_tasks, check_progress).await?;

        info!("Got {} download tasks", download_tasks.len());

        let download_progress =
            TerminalProgress::new().handle(ProgressStage::Downloading, "Downloading files");
        adaptive_download::download_files(download_tasks, download_progress).await?;

        let copy_progress = TerminalProgress::new().handle(ProgressStage::Copying, "Copying files");
        files::copy_files_if_different(deduped_copy_tasks, copy_progress).await?;

        let authlib_injector_library =
            mirror_authlib_injector_library(&data_dir, &download_server_base).await?;
        for metadata in &mut all_metadata {
            if self.replace_download_urls {
                metadata.authlib_injector = authlib_injector_library.clone();
            }
            let instance_dir = InstancesDir::root()
                .instance_dir(metadata.get_name())
                .with_data_dir(data_dir.clone());
            metadata.save(&instance_dir).await?;
        }

        let manifest = InstanceManifest {
            instances: all_metadata
                .iter()
                .map(|metadata| {
                    metadata.to_manifest_entry(metadata.get_name(), &download_server_base)
                })
                .collect::<Result<Vec<_>, _>>()?,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example_deserializes() {
        let content = include_str!("../spec.example.json");
        let spec: Spec = serde_json::from_str(content).expect("spec.example.json should deserialize");
        assert_eq!(spec.instances.len(), 1);
        let instance = &spec.instances[0];
        assert_eq!(instance.name, "Monifactory");
        assert_eq!(instance.content_rules.len(), 2);
        assert!(instance.source_root.is_some());
    }
}
