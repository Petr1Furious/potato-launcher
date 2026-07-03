use std::sync::Arc;

use gpui::{Context, EventEmitter};
use instance::storage::InstanceHandle;
use launcher_bridge::{InstanceLiveStatus, InstanceTaskView, InstanceView};

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

    pub fn set_tasks(
        &mut self,
        handle: InstanceHandle,
        tasks: Arc<[InstanceTaskView]>,
        cx: &mut Context<Self>,
    ) {
        let Some(instance) = self.entries.iter_mut().find(|entry| entry.handle == handle) else {
            return;
        };
        match &mut instance.status {
            InstanceLiveStatus::Installing { tasks: current }
            | InstanceLiveStatus::LaunchPreparing { tasks: current } => {
                *current = tasks;
                cx.emit(InstancesUpdatedEvent);
                cx.notify();
            }
            _ => {}
        }
    }
}
