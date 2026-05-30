use std::sync::Arc;

use gpui::{Context, EventEmitter};
use launcher_bridge::LocalLoader;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalCreateFormState {
    pub minecraft_versions: Arc<[(String, String)]>,
    pub latest_release: String,
    pub minecraft_loading: bool,
    pub minecraft_error: Option<Arc<str>>,
    pub loader_versions: Arc<[String]>,
    pub loader_loading: bool,
    pub loader_error: Option<Arc<str>>,
    pub loader_minecraft_version: Option<String>,
    pub loader_kind: Option<LocalLoader>,
}

impl LocalCreateFormState {
    pub fn set_minecraft_loading(&mut self) {
        self.minecraft_loading = true;
        self.minecraft_error = None;
    }

    pub fn apply_minecraft_versions(
        &mut self,
        versions: Arc<[(String, String)]>,
        latest_release: String,
        error: Option<Arc<str>>,
    ) {
        self.minecraft_loading = false;
        self.minecraft_versions = versions;
        self.latest_release = latest_release;
        self.minecraft_error = error;
    }

    pub fn set_loader_loading(&mut self, minecraft_version: String, loader: LocalLoader) {
        self.loader_loading = true;
        self.loader_error = None;
        self.loader_minecraft_version = Some(minecraft_version);
        self.loader_kind = Some(loader);
        self.loader_versions = Arc::new([]);
    }

    pub fn apply_loader_versions(
        &mut self,
        minecraft_version: String,
        loader: LocalLoader,
        versions: Arc<[String]>,
        error: Option<Arc<str>>,
    ) {
        if self.loader_minecraft_version.as_deref() != Some(minecraft_version.as_str())
            || self.loader_kind != Some(loader)
        {
            return;
        }

        self.loader_loading = false;
        self.loader_versions = versions;
        self.loader_error = error;
    }
}

#[derive(Clone)]
pub struct LocalCreateVersionsUpdatedEvent;

#[derive(Clone)]
pub struct LoaderVersionsUpdatedEvent;

#[derive(Clone, Default)]
pub struct LocalCreateEntries {
    pub state: LocalCreateFormState,
}

impl EventEmitter<LocalCreateVersionsUpdatedEvent> for LocalCreateEntries {}
impl EventEmitter<LoaderVersionsUpdatedEvent> for LocalCreateEntries {}

impl LocalCreateEntries {
    pub fn set_minecraft_loading(&mut self, cx: &mut Context<Self>) {
        self.state.set_minecraft_loading();
        cx.emit(LocalCreateVersionsUpdatedEvent);
    }

    pub fn apply_minecraft_versions(
        &mut self,
        versions: Arc<[(String, String)]>,
        latest_release: String,
        error: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) {
        self.state
            .apply_minecraft_versions(versions, latest_release, error);
        cx.emit(LocalCreateVersionsUpdatedEvent);
    }

    pub fn set_loader_loading(
        &mut self,
        minecraft_version: String,
        loader: LocalLoader,
        cx: &mut Context<Self>,
    ) {
        self.state.set_loader_loading(minecraft_version, loader);
        cx.emit(LoaderVersionsUpdatedEvent);
    }

    pub fn apply_loader_versions(
        &mut self,
        minecraft_version: String,
        loader: LocalLoader,
        versions: Arc<[String]>,
        error: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) {
        self.state
            .apply_loader_versions(minecraft_version, loader, versions, error);
        cx.emit(LoaderVersionsUpdatedEvent);
    }
}
