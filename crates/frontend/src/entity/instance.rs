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

#[derive(Clone, Debug)]
pub struct InstanceProgressUpdate {
    pub id: Uuid,
    pub stage: ProgressStage,
    pub current: u64,
    pub total: u64,
    pub message: Arc<str>,
    pub show_bar: bool,
}

impl InstanceProgressUpdate {
    pub fn new(
        id: Uuid,
        stage: ProgressStage,
        current: u64,
        total: u64,
        message: Arc<str>,
    ) -> Self {
        Self {
            id,
            stage,
            current,
            total,
            message,
            show_bar: total > 1,
        }
    }
}

impl EventEmitter<InstancesUpdatedEvent> for InstanceEntries {}

impl InstanceEntries {
    pub fn replace(&mut self, entries: Arc<[InstanceView]>, cx: &mut Context<Self>) {
        self.entries = entries.iter().cloned().collect();
        cx.emit(InstancesUpdatedEvent);
        cx.notify();
    }

    pub fn set_progress(&mut self, update: InstanceProgressUpdate, cx: &mut Context<Self>) {
        if let Some(instance) = self.entries.iter_mut().find(|entry| entry.id == update.id) {
            if let InstanceLiveStatus::Installing {
                stage,
                current,
                total,
                ..
            } = &instance.status
                && *stage == update.stage
                && update.total == *total
                && update.current < *current
            {
                return;
            }
            instance.status = InstanceLiveStatus::Installing {
                stage: update.stage,
                current: update.current,
                total: update.total,
                message: update.message,
                show_bar: update.show_bar,
            };
            cx.emit(InstancesUpdatedEvent);
            cx.notify();
        }
    }
}
