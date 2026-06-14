use std::time::Duration;

use gpui::{Context, EventEmitter, Task};
use launcher_bridge::NotificationLevel;

#[derive(Clone, Debug)]
pub struct NotificationEntry {
    pub id: u64,
    pub level: NotificationLevel,
    pub message: String,
}

#[derive(Default)]
pub struct NotificationEntries {
    next_id: u64,
    pub entries: Vec<NotificationEntry>,
    _dismiss_tasks: Vec<Task<()>>,
}

#[derive(Clone)]
pub struct NotificationsUpdatedEvent;

impl EventEmitter<NotificationsUpdatedEvent> for NotificationEntries {}

impl NotificationEntries {
    pub fn push(
        &mut self,
        level: NotificationLevel,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(NotificationEntry {
            id,
            level,
            message: message.into(),
        });
        if self.entries.len() > 5 {
            self.entries.remove(0);
        }
        if matches!(
            level,
            NotificationLevel::Info | NotificationLevel::Success | NotificationLevel::Warning
        ) {
            let delay = match level {
                NotificationLevel::Warning => Duration::from_secs(8),
                _ => Duration::from_secs(5),
            };
            self._dismiss_tasks.push(cx.spawn(async move |entries, cx| {
                cx.background_executor().timer(delay).await;
                let _ = entries.update(cx, |entries, cx| entries.dismiss(id, cx));
            }));
        }
        cx.emit(NotificationsUpdatedEvent);
        cx.notify();
    }

    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        self.entries.retain(|entry| entry.id != id);
        cx.emit(NotificationsUpdatedEvent);
        cx.notify();
    }
}
