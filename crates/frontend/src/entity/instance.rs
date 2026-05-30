use std::sync::Arc;

use gpui::{Context, EventEmitter};
use launcher_bridge::{InstanceLiveStatus, InstanceView, ProgressStage};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct InstanceEntries {
    pub entries: Vec<InstanceView>,
}

#[derive(Clone)]
pub struct InstancesUpdatedEvent;

impl EventEmitter<InstancesUpdatedEvent> for InstanceEntries {}

impl InstanceEntries {
    pub fn replace(&mut self, entries: Arc<[InstanceView]>, cx: &mut Context<Self>) {
        self.entries = entries.iter().cloned().collect();
        cx.emit(InstancesUpdatedEvent);
        cx.notify();
    }

    pub fn set_progress(
        &mut self,
        id: Uuid,
        stage: ProgressStage,
        current: u64,
        total: u64,
        message: Arc<str>,
        show_bar: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(instance) = self.entries.iter_mut().find(|entry| entry.id == id) {
            instance.status = InstanceLiveStatus::Installing {
                stage,
                current,
                total,
                message,
                show_bar,
            };
            cx.emit(InstancesUpdatedEvent);
            cx.notify();
        }
    }
}
