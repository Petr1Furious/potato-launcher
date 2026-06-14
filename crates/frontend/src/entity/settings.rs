use gpui::{Context, EventEmitter};
use launcher_bridge::LauncherSettingsView;

#[derive(Clone, Default)]
pub struct LauncherSettingsEntries {
    pub settings: LauncherSettingsView,
}

#[derive(Clone)]
pub struct LauncherSettingsUpdatedEvent;

impl EventEmitter<LauncherSettingsUpdatedEvent> for LauncherSettingsEntries {}

impl LauncherSettingsEntries {
    pub fn replace(&mut self, settings: LauncherSettingsView, cx: &mut Context<Self>) {
        self.settings = settings;
        cx.emit(LauncherSettingsUpdatedEvent);
        cx.notify();
    }
}
