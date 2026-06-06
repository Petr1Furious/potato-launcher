use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use utils::files::{CheckTask, DeleteTask, GetFilesInDirError};

use crate::install_params::{InstallCause, InstallParams};
use crate::instance_metadata::{EnableOptionalModTask, ModEntry, TaskSet};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModSyncMode {
    /// Compare previous vs new manifest and preserve user-added and user-removed mods
    #[default]
    Delta,
    /// Ensure client mods match the remote exactly on each update and launch
    Mirror,
    /// Like Mirror, but check tasks compare file size only instead of sha1
    MirrorFast,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OptionalModSet {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub enabled_by_default: bool,
    pub mod_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModSyncSettings {
    pub mode: ModSyncMode,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub blocked: Vec<String>,
    #[serde(default)]
    pub optional_sets: Vec<OptionalModSet>,
}

impl Default for ModSyncSettings {
    fn default() -> Self {
        Self {
            mode: ModSyncMode::Delta,
            required: Vec::new(),
            blocked: Vec::new(),
            optional_sets: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModSyncWarning {
    /// The client had a mod removed by the launcher
    ModRemoved { mod_id: String, path: PathBuf },
    /// The client was missing a mod the launcher restored
    ModAdded { mod_id: String, path: PathBuf },
}

pub fn resolve_optional_set_enabled(
    settings: &ModSyncSettings,
    user_preferences: &HashMap<String, bool>,
) -> HashMap<String, bool> {
    settings
        .optional_sets
        .iter()
        .map(|set| {
            let enabled = user_preferences
                .get(&set.id)
                .copied()
                .unwrap_or(set.enabled_by_default);
            (set.id.clone(), enabled)
        })
        .collect()
}

pub struct ModSyncPlan {
    pub tasks: TaskSet,
    pub warnings: Vec<ModSyncWarning>,
}

#[derive(thiserror::Error, Debug)]
pub enum ModSyncError {
    #[error("failed to enumerate mod files: {0}")]
    EnumerateMods(#[from] GetFilesInDirError),
    #[error("optional mod entry has no jar filename: {0}")]
    OptionalModMissingFilename(String),
}

pub fn build_mod_sync_plan(
    new_entries: &[ModEntry],
    settings: &ModSyncSettings,
    params: &InstallParams,
) -> Result<ModSyncPlan, ModSyncError> {
    ModSyncPlanner::new(new_entries, settings, params)?.build()
}

struct ModSyncPlanner<'a> {
    new_entries: &'a [ModEntry],
    settings: &'a ModSyncSettings,
    params: &'a InstallParams,
    local_mods: HashMap<String, Vec<PathBuf>>,
    previous: HashMap<&'a str, &'a ModEntry>,
    required: HashSet<&'a str>,
    blocked: HashSet<&'a str>,
    optional_mod_to_set: HashMap<&'a str, &'a OptionalModSet>,
    tasks: TaskSet,
    warnings: Vec<ModSyncWarning>,
}

impl<'a> ModSyncPlanner<'a> {
    fn new(
        new_entries: &'a [ModEntry],
        settings: &'a ModSyncSettings,
        params: &'a InstallParams,
    ) -> Result<Self, ModSyncError> {
        Ok(Self {
            new_entries,
            settings,
            params,
            local_mods: scan_local_mods(&params.instance_dir.mods_dir())?,
            previous: params
                .previous_mod_entries
                .iter()
                .map(|entry| (entry.mod_id.as_str(), entry))
                .collect(),
            required: settings.required.iter().map(String::as_str).collect(),
            blocked: settings.blocked.iter().map(String::as_str).collect(),
            optional_mod_to_set: optional_mod_to_set(settings),
            tasks: TaskSet::default(),
            warnings: Vec::new(),
        })
    }

    fn build(mut self) -> Result<ModSyncPlan, ModSyncError> {
        if !self.should_run() {
            return Ok(self.finish());
        }

        if self.settings.mode == ModSyncMode::Delta && !self.params.force_overwrite {
            self.plan_delta()?;
        } else {
            self.plan_mirror()?;
        }

        Ok(self.finish())
    }

    fn finish(self) -> ModSyncPlan {
        ModSyncPlan {
            tasks: self.tasks,
            warnings: self.warnings,
        }
    }

    fn should_run(&self) -> bool {
        self.params.force_overwrite
            || match self.settings.mode {
                ModSyncMode::Delta => self.params.cause == InstallCause::Update,
                ModSyncMode::Mirror | ModSyncMode::MirrorFast => true,
            }
    }

    fn plan_delta(&mut self) -> Result<(), ModSyncError> {
        let new_ids = self
            .new_entries
            .iter()
            .map(|entry| entry.mod_id.clone())
            .collect::<HashSet<_>>();

        for entry in self.new_entries {
            if self.blocked.contains(entry.mod_id.as_str()) {
                self.delete_local_mod(&entry.mod_id, self.should_warn_removed(&entry.mod_id));
                continue;
            }

            if self.optional_mod_to_set.contains_key(entry.mod_id.as_str()) {
                self.plan_optional_entry(entry, false)?;
                continue;
            }

            self.plan_delta_normal_entry(entry);
        }

        for previous_mod_id in self
            .previous
            .keys()
            .copied()
            .filter(|mod_id| !new_ids.contains(*mod_id))
            .collect::<Vec<_>>()
        {
            self.delete_local_mod(previous_mod_id, false);
        }

        self.delete_blocked_and_manual_optional_extras(&new_ids);
        Ok(())
    }

    fn plan_mirror(&mut self) -> Result<(), ModSyncError> {
        let mut desired_mod_ids = HashSet::new();

        for entry in self.new_entries {
            if self.optional_mod_to_set.contains_key(entry.mod_id.as_str()) {
                if self.optional_set_enabled(entry.mod_id.as_str()) {
                    desired_mod_ids.insert(entry.mod_id.as_str());
                }
                self.plan_optional_entry(entry, true)?;
                continue;
            }

            if self.blocked.contains(entry.mod_id.as_str()) {
                continue;
            }

            desired_mod_ids.insert(entry.mod_id.as_str());
            if self.should_warn_added(&entry.mod_id) {
                self.warn_added(&entry.mod_id, self.mod_target_path(entry));
            }
            self.push_check(entry, self.mod_target_path(entry));
        }

        for (mod_id, paths) in self.local_mods.clone() {
            if desired_mod_ids.contains(mod_id.as_str()) {
                continue;
            }
            self.delete_paths(&mod_id, &paths, self.should_warn_removed(&mod_id));
        }

        Ok(())
    }

    fn plan_delta_normal_entry(&mut self, entry: &ModEntry) {
        let mod_id = entry.mod_id.as_str();
        let locally_present = self.local_mods.contains_key(mod_id);
        let is_required = self.required.contains(mod_id);

        match self.previous.get(mod_id) {
            None => self.push_check(entry, self.mod_target_path(entry)),
            Some(previous) if previous.object.sha1 != entry.object.sha1 => {
                if locally_present || is_required {
                    self.replace_local_mod(entry);
                }
                if is_required && !locally_present {
                    self.warn_added(mod_id, self.mod_target_path(entry));
                }
            }
            Some(_) if is_required && !locally_present => {
                self.warn_added(mod_id, self.mod_target_path(entry));
                self.push_check(entry, self.mod_target_path(entry));
            }
            Some(_) => {}
        }
    }

    fn plan_optional_entry(
        &mut self,
        entry: &ModEntry,
        force_check: bool,
    ) -> Result<(), ModSyncError> {
        let mod_id = entry.mod_id.as_str();
        let cache_path = self.optional_cache_path(entry)?;
        let target_path = self.mod_target_path(entry);

        if force_check || self.optional_cache_needs_check(entry) {
            self.push_check(entry, cache_path.clone());
        }

        if self.optional_set_enabled(mod_id) {
            if self.should_warn_added(mod_id) {
                self.warn_added(mod_id, target_path.clone());
            }
            self.tasks
                .enable_optional_mod_tasks
                .push(EnableOptionalModTask {
                    source: cache_path,
                    target: target_path,
                });
        } else {
            self.delete_local_mod(mod_id, self.should_warn_removed(mod_id));
        }

        Ok(())
    }

    fn optional_cache_needs_check(&self, entry: &ModEntry) -> bool {
        match self.previous.get(entry.mod_id.as_str()) {
            None => true,
            Some(previous) => previous.object.sha1 != entry.object.sha1,
        }
    }

    fn delete_blocked_and_manual_optional_extras(&mut self, new_ids: &HashSet<String>) {
        for mod_id in self.local_mods.keys().cloned().collect::<Vec<_>>() {
            if self.blocked.contains(mod_id.as_str()) {
                self.delete_local_mod(&mod_id, self.should_warn_removed(&mod_id));
                continue;
            }

            if self.optional_mod_to_set.contains_key(mod_id.as_str())
                && !new_ids.contains(mod_id.as_str())
            {
                self.delete_local_mod(&mod_id, self.should_warn_removed(&mod_id));
            }
        }
    }

    fn replace_local_mod(&mut self, entry: &ModEntry) {
        let expected_path = self.mod_target_path(entry);
        if let Some(paths) = self.local_mods.get(entry.mod_id.as_str()).cloned() {
            for old_path in paths {
                if old_path != expected_path {
                    self.push_delete(old_path);
                }
            }
        }
        self.push_check(entry, expected_path);
    }

    fn delete_local_mod(&mut self, mod_id: &str, warn: bool) {
        if let Some(paths) = self.local_mods.get(mod_id).cloned() {
            self.delete_paths(mod_id, &paths, warn);
        }
    }

    fn delete_paths(&mut self, mod_id: &str, paths: &[PathBuf], warn: bool) {
        for path in paths {
            self.push_delete(path.clone());
            if warn {
                self.warnings.push(ModSyncWarning::ModRemoved {
                    mod_id: mod_id.to_string(),
                    path: path.clone(),
                });
            }
        }
    }

    fn push_delete(&mut self, path: PathBuf) {
        if !self.tasks.delete_tasks.iter().any(|task| task.path == path) {
            self.tasks.delete_tasks.push(DeleteTask { path });
        }
    }

    fn push_check(&mut self, entry: &ModEntry, path: PathBuf) {
        self.tasks.check_tasks.push(CheckTask {
            url: entry.object.url.clone(),
            remote_sha1: if self.mirror_fast() {
                None
            } else {
                Some(entry.object.sha1.clone())
            },
            remote_size: if self.mirror_fast() {
                entry.object.size
            } else {
                None
            },
            path,
        });
    }

    fn warn_added(&mut self, mod_id: &str, path: PathBuf) {
        self.warnings.push(ModSyncWarning::ModAdded {
            mod_id: mod_id.to_string(),
            path,
        });
    }

    fn should_warn_added(&self, mod_id: &str) -> bool {
        self.previous.contains_key(mod_id) && !self.local_mods.contains_key(mod_id)
    }

    fn should_warn_removed(&self, mod_id: &str) -> bool {
        !self.previous.contains_key(mod_id)
    }

    fn optional_set_enabled(&self, mod_id: &str) -> bool {
        let Some(set) = self.optional_mod_to_set.get(mod_id) else {
            return false;
        };
        self.params
            .optional_sets_enabled
            .get(&set.id)
            .copied()
            .unwrap_or(set.enabled_by_default)
    }

    fn optional_cache_path(&self, entry: &ModEntry) -> Result<PathBuf, ModSyncError> {
        let filename = entry
            .object
            .path
            .file_name()
            .ok_or_else(|| ModSyncError::OptionalModMissingFilename(entry.mod_id.clone()))?;
        Ok(self.params.instance_dir.optional_mods_dir().join(filename))
    }

    fn mod_target_path(&self, entry: &ModEntry) -> PathBuf {
        entry
            .object
            .path
            .to_path(&self.params.instance_dir.minecraft_dir())
    }

    fn mirror_fast(&self) -> bool {
        self.settings.mode == ModSyncMode::MirrorFast
    }
}

fn optional_mod_to_set(settings: &ModSyncSettings) -> HashMap<&str, &OptionalModSet> {
    let mut map = HashMap::new();
    for set in &settings.optional_sets {
        for mod_id in &set.mod_ids {
            map.entry(mod_id.as_str()).or_insert(set);
        }
    }
    map
}

/// Iterate over `*.jar` files and index them by extracted mod id
fn scan_local_mods(mods_dir: &Path) -> Result<HashMap<String, Vec<PathBuf>>, ModSyncError> {
    let mut mods = HashMap::<String, Vec<PathBuf>>::new();

    if !mods_dir.is_dir() {
        return Ok(mods);
    }

    for path in utils::files::get_files_in_dir(mods_dir)? {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jar") {
            continue;
        }
        match utils::mod_id::extract_mod_id(&path) {
            Ok(Some(mod_id)) => mods.entry(mod_id).or_default().push(path),
            Ok(None) => log::debug!("Skipping jar without mod id: {}", path.display()),
            Err(err) => log::warn!("Failed to read mod id from {}: {err:#}", path.display()),
        }
    }

    for (mod_id, paths) in &mods {
        if paths.len() > 1 {
            log::warn!("Duplicate mod id locally: {mod_id}");
        }
    }

    Ok(mods)
}

#[cfg(test)]
mod tests;
