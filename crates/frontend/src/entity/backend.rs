use std::sync::Arc;

use gpui::{Context, EventEmitter};
use launcher_bridge::BackendStatus;

#[derive(Clone, Default)]
pub struct BackendList {
    pub backends: Vec<BackendStatus>,
}

#[derive(Clone)]
pub struct BackendsUpdatedEvent;

impl EventEmitter<BackendsUpdatedEvent> for BackendList {}

impl BackendList {
    pub fn replace(&mut self, backends: Arc<[BackendStatus]>, cx: &mut Context<Self>) {
        self.backends = backends.iter().cloned().collect();
        cx.emit(BackendsUpdatedEvent);
        cx.notify();
    }
}
