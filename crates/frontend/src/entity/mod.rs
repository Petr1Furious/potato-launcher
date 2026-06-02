pub mod account;
pub mod backend;
pub mod instance;
pub mod local_create;
pub mod notification;
pub mod settings;
pub mod update;

use account::AccountEntries;
use backend::BackendList;
use gpui::Entity;
use instance::InstanceEntries;
use launcher_bridge::BackendSender;
use local_create::LocalCreateEntries;
use notification::NotificationEntries;
use settings::LauncherSettingsEntries;
use std::path::PathBuf;
use update::UpdateEntries;

#[derive(Clone)]
pub struct DataEntities {
    pub instances: Entity<InstanceEntries>,
    pub backends: Entity<BackendList>,
    pub accounts: Entity<AccountEntries>,
    pub notifications: Entity<NotificationEntries>,
    pub settings: Entity<LauncherSettingsEntries>,
    pub local_create: Entity<LocalCreateEntries>,
    pub update: Entity<UpdateEntries>,
    pub backend_sender: BackendSender,
    pub launcher_dir: PathBuf,
}
