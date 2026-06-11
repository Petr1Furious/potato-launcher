use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use relative_path::RelativePathBuf;
use url::Url;
use utils::paths::{DataDir, InstanceDirFS, InstancesDir};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::install_params::{InstallCause, InstallParams};
use crate::instance_metadata::{ModEntry, Object};
use crate::mod_sync::{
    ModSyncMode, ModSyncSettings, ModSyncWarning, OptionalModSet, build_mod_sync_plan,
    resolve_optional_set_enabled,
};

fn write_fabric_mod_jar(path: &Path, mod_id: &str) {
    let file = File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    zip.start_file("fabric.mod.json", options).unwrap();
    zip.write_all(format!(r#"{{"schemaVersion":1,"id":"{mod_id}","version":"1.0.0"}}"#).as_bytes())
        .unwrap();
    zip.finish().unwrap();
}

fn mod_entry(mod_id: &str, filename: &str, sha1: &str) -> ModEntry {
    ModEntry {
        mod_id: mod_id.to_string(),
        object: Object {
            path: RelativePathBuf::from(format!("mods/{filename}")),
            sha1: sha1.to_string(),
            size: 123,
            url: Url::parse(&format!("https://example.com/{filename}")).unwrap(),
        },
    }
}

fn default_settings(mode: ModSyncMode) -> ModSyncSettings {
    ModSyncSettings {
        mode,
        ..ModSyncSettings::default()
    }
}

struct TestDirs {
    root: PathBuf,
    instance_dir: InstanceDirFS,
    mods_dir: PathBuf,
    optional_mods_dir: PathBuf,
}

impl TestDirs {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("potato-mod-sync-test-{}", uuid::Uuid::new_v4()));
        let data_dir = DataDir::new(root.join("launcher"));
        let instance_dir = InstancesDir::root()
            .instance_dir("test-instance")
            .with_data_dir(data_dir);
        let mods_dir = instance_dir.mods_dir();
        let optional_mods_dir = instance_dir.optional_mods_dir();
        std::fs::create_dir_all(&mods_dir).unwrap();
        std::fs::create_dir_all(&optional_mods_dir).unwrap();
        Self {
            root,
            instance_dir,
            mods_dir,
            optional_mods_dir,
        }
    }

    fn install_params(&self, cause: InstallCause) -> InstallParams {
        InstallParams {
            instance_dir: self.instance_dir.clone(),
            cause,
            force_overwrite: false,
            previous_mod_entries: Vec::new(),
            optional_sets_enabled: HashMap::new(),
        }
    }
}

impl Drop for TestDirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn plan(
    new_entries: &[ModEntry],
    settings: &ModSyncSettings,
    params: &InstallParams,
) -> crate::mod_sync::ModSyncPlan {
    build_mod_sync_plan(new_entries, settings, params).unwrap()
}

#[test]
fn delta_adds_check_task_for_new_mod() {
    let dirs = TestDirs::new();
    let entry = mod_entry("fabric-api", "fabric-api.jar", "aaa");

    let result = plan(
        std::slice::from_ref(&entry),
        &default_settings(ModSyncMode::Delta),
        &dirs.install_params(InstallCause::Update),
    );

    assert_eq!(result.tasks.check_tasks.len(), 1);
    assert_eq!(
        result.tasks.check_tasks[0].path,
        dirs.mods_dir.join("fabric-api.jar")
    );
    assert!(result.tasks.delete_tasks.is_empty());
    assert!(result.warnings.is_empty());
}

#[test]
fn delta_allows_user_removed_normal_mod() {
    let dirs = TestDirs::new();
    let entry = mod_entry("fabric-api", "fabric-api.jar", "aaa");

    let result = plan(
        std::slice::from_ref(&entry),
        &default_settings(ModSyncMode::Delta),
        &InstallParams {
            previous_mod_entries: vec![entry.clone()],
            ..dirs.install_params(InstallCause::Update)
        },
    );

    assert!(result.tasks.check_tasks.is_empty());
    assert!(result.tasks.delete_tasks.is_empty());
    assert!(result.warnings.is_empty());
}

#[test]
fn delta_updates_locally_present_changed_mod() {
    let dirs = TestDirs::new();
    write_fabric_mod_jar(&dirs.mods_dir.join("fabric-api-old.jar"), "fabric-api");

    let previous = mod_entry("fabric-api", "fabric-api-old.jar", "old");
    let new = mod_entry("fabric-api", "fabric-api-new.jar", "new");
    let result = plan(
        std::slice::from_ref(&new),
        &default_settings(ModSyncMode::Delta),
        &InstallParams {
            previous_mod_entries: vec![previous],
            ..dirs.install_params(InstallCause::Update)
        },
    );

    assert_eq!(
        result.tasks.delete_tasks[0].path,
        dirs.mods_dir.join("fabric-api-old.jar")
    );
    assert_eq!(
        result.tasks.check_tasks[0].path,
        dirs.mods_dir.join("fabric-api-new.jar")
    );
    assert!(result.warnings.is_empty());
}

#[test]
fn delta_restores_missing_required_mod_with_warning() {
    let dirs = TestDirs::new();
    let entry = mod_entry("fabric-api", "fabric-api.jar", "aaa");
    let settings = ModSyncSettings {
        required: vec!["fabric-api".to_string()],
        ..default_settings(ModSyncMode::Delta)
    };

    let result = plan(
        std::slice::from_ref(&entry),
        &settings,
        &InstallParams {
            previous_mod_entries: vec![entry.clone()],
            ..dirs.install_params(InstallCause::Update)
        },
    );

    assert_eq!(result.tasks.check_tasks.len(), 1);
    assert!(matches!(
        result.warnings[0],
        ModSyncWarning::ModAdded { .. }
    ));
}

#[test]
fn delta_deletes_user_added_blocked_mod_with_warning() {
    let dirs = TestDirs::new();
    let blocked_path = dirs.mods_dir.join("blocked.jar");
    write_fabric_mod_jar(&blocked_path, "blocked");
    let settings = ModSyncSettings {
        blocked: vec!["blocked".to_string()],
        ..default_settings(ModSyncMode::Delta)
    };

    let result = plan(&[], &settings, &dirs.install_params(InstallCause::Update));

    assert_eq!(result.tasks.delete_tasks[0].path, blocked_path);
    assert!(matches!(
        result.warnings[0],
        ModSyncWarning::ModRemoved { .. }
    ));
}

#[test]
fn delta_deletes_server_owned_blocked_mod_without_warning() {
    let dirs = TestDirs::new();
    let blocked_path = dirs.mods_dir.join("blocked.jar");
    write_fabric_mod_jar(&blocked_path, "blocked");
    let previous = mod_entry("blocked", "blocked.jar", "old");
    let settings = ModSyncSettings {
        blocked: vec!["blocked".to_string()],
        ..default_settings(ModSyncMode::Delta)
    };

    let result = plan(
        &[],
        &settings,
        &InstallParams {
            previous_mod_entries: vec![previous],
            ..dirs.install_params(InstallCause::Update)
        },
    );

    assert_eq!(result.tasks.delete_tasks[0].path, blocked_path);
    assert!(result.warnings.is_empty());
}

#[test]
fn optional_set_downloads_to_cache_and_links_when_enabled() {
    let dirs = TestDirs::new();
    let entry = mod_entry("jei", "jei.jar", "aaa");
    let settings = ModSyncSettings {
        optional_sets: vec![OptionalModSet {
            id: "extras".to_string(),
            display_name: "Extras".to_string(),
            enabled_by_default: false,
            mod_ids: vec!["jei".to_string()],
        }],
        ..default_settings(ModSyncMode::Delta)
    };

    let result = plan(
        std::slice::from_ref(&entry),
        &settings,
        &InstallParams {
            optional_sets_enabled: HashMap::from([("extras".to_string(), true)]),
            ..dirs.install_params(InstallCause::Update)
        },
    );

    assert_eq!(
        result.tasks.check_tasks[0].path,
        dirs.optional_mods_dir.join("jei.jar")
    );
    assert_eq!(
        result.tasks.enable_optional_mod_tasks[0].source,
        dirs.optional_mods_dir.join("jei.jar")
    );
    assert_eq!(
        result.tasks.enable_optional_mod_tasks[0].target,
        dirs.mods_dir.join("jei.jar")
    );
}

#[test]
fn optional_set_disabled_keeps_cache_check_and_removes_link() {
    let dirs = TestDirs::new();
    let local_path = dirs.mods_dir.join("jei.jar");
    write_fabric_mod_jar(&local_path, "jei");
    let entry = mod_entry("jei", "jei.jar", "aaa");
    let settings = ModSyncSettings {
        optional_sets: vec![OptionalModSet {
            id: "extras".to_string(),
            display_name: "Extras".to_string(),
            enabled_by_default: true,
            mod_ids: vec!["jei".to_string()],
        }],
        ..default_settings(ModSyncMode::Delta)
    };

    let result = plan(
        std::slice::from_ref(&entry),
        &settings,
        &InstallParams {
            optional_sets_enabled: HashMap::from([("extras".to_string(), false)]),
            ..dirs.install_params(InstallCause::Update)
        },
    );

    assert_eq!(
        result.tasks.check_tasks[0].path,
        dirs.optional_mods_dir.join("jei.jar")
    );
    assert_eq!(result.tasks.delete_tasks[0].path, local_path);
    assert!(matches!(
        result.warnings[0],
        ModSyncWarning::ModRemoved { .. }
    ));
}

#[test]
fn resolves_optional_set_defaults_and_user_preferences() {
    let settings = ModSyncSettings {
        optional_sets: vec![OptionalModSet {
            id: "extras".to_string(),
            display_name: "Extras".to_string(),
            enabled_by_default: true,
            mod_ids: vec!["jei".to_string()],
        }],
        ..default_settings(ModSyncMode::Delta)
    };

    let enabled =
        resolve_optional_set_enabled(&settings, &HashMap::from([("extras".to_string(), false)]));

    assert_eq!(enabled.get("extras"), Some(&false));
}

#[test]
fn mirror_removes_extra_user_mod_with_warning() {
    let dirs = TestDirs::new();
    let extra_path = dirs.mods_dir.join("extra.jar");
    write_fabric_mod_jar(&extra_path, "extra");

    let result = plan(
        &[],
        &default_settings(ModSyncMode::Mirror),
        &dirs.install_params(InstallCause::Run),
    );

    assert_eq!(result.tasks.delete_tasks[0].path, extra_path);
    assert!(matches!(
        result.warnings[0],
        ModSyncWarning::ModRemoved { .. }
    ));
}

#[test]
fn force_overwrite_uses_mirror_behavior_in_delta_mode() {
    let dirs = TestDirs::new();
    let extra_path = dirs.mods_dir.join("extra.jar");
    write_fabric_mod_jar(&extra_path, "extra");

    let result = plan(
        &[],
        &default_settings(ModSyncMode::Delta),
        &InstallParams {
            force_overwrite: true,
            ..dirs.install_params(InstallCause::Update)
        },
    );

    assert_eq!(result.tasks.delete_tasks[0].path, extra_path);
    assert!(matches!(
        result.warnings[0],
        ModSyncWarning::ModRemoved { .. }
    ));
}

#[test]
fn mirror_fast_uses_size_checks() {
    let dirs = TestDirs::new();
    let entry = mod_entry("fabric-api", "fabric-api.jar", "aaa");

    let result = plan(
        std::slice::from_ref(&entry),
        &default_settings(ModSyncMode::MirrorFast),
        &dirs.install_params(InstallCause::Run),
    );

    assert_eq!(result.tasks.check_tasks[0].remote_size, Some(123));
    assert_eq!(result.tasks.check_tasks[0].remote_sha1, None);
}

#[test]
fn extract_mod_id_tolerates_unescaped_newlines_in_fabric_mod_json() {
    let dir = std::env::temp_dir().join(format!(
        "potato-mod-id-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("some_mod.jar");
    let file = File::create(&path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    zip.start_file("fabric.mod.json", options).unwrap();
    zip.write_all(
        br#"{
  "schemaVersion": 1,
  "id": "some_mod",
  "version": "1.2.3",
  "description": "line one
line two"
}"#,
    )
    .unwrap();
    zip.finish().unwrap();

    assert_eq!(
        utils::mod_id::extract_mod_id(&path).unwrap(),
        Some("some_mod".to_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}
