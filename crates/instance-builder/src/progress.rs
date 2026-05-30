use utils::progress::{ProgressEvent, ProgressHandle, ProgressReporter, ProgressStage};

#[derive(Clone)]
pub struct TerminalProgress {
    bar: indicatif::ProgressBar,
}

impl TerminalProgress {
    pub fn new() -> Self {
        let bar = indicatif::ProgressBar::new(0);
        bar.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{msg} {bar:40.cyan/blue} {pos}/{len}")
                .unwrap(),
        );
        Self { bar }
    }

    pub fn handle(self, stage: ProgressStage, message: impl Into<String>) -> ProgressHandle<Self> {
        ProgressHandle::new(self, stage).with_message(message)
    }
}

impl Default for TerminalProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressReporter for TerminalProgress {
    fn event(&self, event: ProgressEvent) {
        self.bar.set_length(event.total);
        self.bar.set_position(event.current);
        if let Some(message) = event.message {
            self.bar.set_message(message);
        }
        if event.finished {
            self.bar.finish();
        }
    }
}
