use std::collections::HashMap;

use utils::paths::InstanceDirFS;

use crate::instance_metadata::{ContentRule, ModEntry};

#[derive(PartialEq, Clone, Copy)]
pub enum InstallCause {
    Update,
    Run,
}

pub struct InstallParams {
    pub instance_dir: InstanceDirFS,
    pub cause: InstallCause,
    pub force_overwrite: bool,
    /// Previous `mod_entries` from local (previous) meta.json; used for diff calculation
    pub previous_mod_entries: Vec<ModEntry>,
    /// Previous `content_rules` from local (previous) meta.json; used to track each
    /// rule's `version` so one-time rules are applied only once per version bump
    pub previous_content_rules: Vec<ContentRule>,
    /// Resolved optional-set toggles for this instance
    pub optional_sets_enabled: HashMap<String, bool>,
}
