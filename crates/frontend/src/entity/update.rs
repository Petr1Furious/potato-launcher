use gpui::{Context, EventEmitter};
use launcher_bridge::UpdateStatusView;

pub enum UpdateState {
    Blocking(UpdateStatusView),
    Done,
}

pub struct UpdateStateChangedEvent;

pub struct UpdateEntries {
    pub state: UpdateState,
}

impl Default for UpdateEntries {
    fn default() -> Self {
        Self {
            state: UpdateState::Blocking(UpdateStatusView::Checking),
        }
    }
}

impl EventEmitter<UpdateStateChangedEvent> for UpdateEntries {}

impl UpdateEntries {
    pub fn is_blocking(&self) -> bool {
        matches!(self.state, UpdateState::Blocking(_))
    }

    pub fn apply(&mut self, status: UpdateStatusView, cx: &mut Context<Self>) {
        self.state = match status {
            UpdateStatusView::UpToDate | UpdateStatusView::NotApplicable => UpdateState::Done,
            other => UpdateState::Blocking(other),
        };
        cx.emit(UpdateStateChangedEvent);
        cx.notify();
    }
}
