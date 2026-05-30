pub mod account;
pub mod backend;
pub mod instance;
pub mod notification;
pub mod settings;

use account::AccountEntries;
use backend::BackendList;
use gpui::Entity;
use instance::InstanceEntries;
use launcher_bridge::BackendSender;
use notification::NotificationEntries;
use settings::LauncherSettingsEntries;
use std::path::PathBuf;

#[derive(Clone)]
pub struct DataEntities {
    pub instances: Entity<InstanceEntries>,
    pub backends: Entity<BackendList>,
    pub accounts: Entity<AccountEntries>,
    pub notifications: Entity<NotificationEntries>,
    pub settings: Entity<LauncherSettingsEntries>,
    pub backend_sender: BackendSender,
    pub launcher_dir: PathBuf,
}
