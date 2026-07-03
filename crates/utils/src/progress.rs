use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressEvent {
    pub message: Option<String>,
    pub current: u64,
    pub total: u64,
    pub finished: bool,
}

pub trait ProgressReporter: Sync + Send {
    fn event(&self, event: ProgressEvent);
}

pub trait ProgressTracker: Sync + Send {
    fn set_length(&self, length: u64);

    fn inc(&self, amount: u64);

    /// Roll back previously counted progress, may be ignored.
    fn dec(&self, _amount: u64) {}

    /// Update the human-readable status message, may be ignored.
    fn set_message(&self, _message: &str) {}

    fn finish(&self);
}

impl<T: ProgressTracker + ?Sized> ProgressTracker for &T {
    fn set_length(&self, length: u64) {
        (**self).set_length(length);
    }

    fn inc(&self, amount: u64) {
        (**self).inc(amount);
    }

    fn dec(&self, amount: u64) {
        (**self).dec(amount);
    }

    fn set_message(&self, message: &str) {
        (**self).set_message(message);
    }

    fn finish(&self) {
        (**self).finish();
    }
}

#[derive(Clone, Debug)]
struct ProgressState {
    message: Option<String>,
    current: u64,
    total: u64,
    finished: bool,
}

impl ProgressState {
    fn event(&self) -> ProgressEvent {
        ProgressEvent {
            message: self.message.clone(),
            current: self.current,
            total: self.total,
            finished: self.finished,
        }
    }
}

#[derive(Clone)]
pub struct ProgressHandle<R> {
    reporter: R,
    state: Arc<Mutex<ProgressState>>,
}

impl<R> ProgressHandle<R>
where
    R: ProgressReporter,
{
    pub fn new(reporter: R) -> Self {
        Self {
            reporter,
            state: Arc::new(Mutex::new(ProgressState {
                message: None,
                current: 0,
                total: 0,
                finished: false,
            })),
        }
    }

    pub fn with_message(self, message: impl Into<String>) -> Self {
        self.update(|state| {
            state.message = Some(message.into());
        });
        self
    }

    pub fn set_message(&self, message: impl Into<String>) {
        self.update(|state| {
            state.message = Some(message.into());
        });
    }

    fn update(&self, update: impl FnOnce(&mut ProgressState)) {
        let event = {
            let mut state = self.state.lock().expect("progress state poisoned");
            update(&mut state);
            state.event()
        };
        self.reporter.event(event);
    }
}

impl<R> ProgressTracker for ProgressHandle<R>
where
    R: ProgressReporter,
{
    fn set_length(&self, length: u64) {
        self.update(|state| {
            state.total = length;
            state.current = 0;
            state.finished = false;
        });
    }

    fn inc(&self, amount: u64) {
        self.update(|state| {
            state.current = state.current.saturating_add(amount);
        });
    }

    fn dec(&self, amount: u64) {
        self.update(|state| {
            state.current = state.current.saturating_sub(amount);
        });
    }

    fn set_message(&self, message: &str) {
        self.update(|state| {
            state.message = Some(message.to_string());
        });
    }

    fn finish(&self) {
        self.update(|state| {
            if state.total > 0 && state.current > state.total {
                state.total = state.current;
            }
            state.finished = true;
        });
    }
}

#[derive(Clone, Copy, Default)]
pub struct NoProgressBar;

impl ProgressReporter for NoProgressBar {
    fn event(&self, _event: ProgressEvent) {}
}

impl ProgressTracker for NoProgressBar {
    fn set_length(&self, _length: u64) {}
    fn inc(&self, _amount: u64) {}
    fn finish(&self) {}
}

pub fn no_progress_bar() -> NoProgressBar {
    NoProgressBar
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct CapturingReporter {
        events: Arc<Mutex<Vec<ProgressEvent>>>,
    }

    impl ProgressReporter for CapturingReporter {
        fn event(&self, event: ProgressEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn progress_handle_emits_stateful_events() {
        let reporter = CapturingReporter::default();
        let events = reporter.events.clone();
        let handle = ProgressHandle::new(reporter).with_message("Downloading files");

        handle.set_length(3);
        handle.inc(1);
        handle.inc(2);
        handle.finish();

        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|event| { event.message.as_deref() == Some("Downloading files") })
        );
        assert_eq!(events.last().unwrap().current, 3);
        assert_eq!(events.last().unwrap().total, 3);
        assert!(events.last().unwrap().finished);
    }

    #[test]
    fn progress_handle_dec_saturates() {
        let reporter = CapturingReporter::default();
        let events = reporter.events.clone();
        let handle = ProgressHandle::new(reporter);

        handle.set_length(10);
        handle.inc(4);
        handle.dec(2);
        assert_eq!(events.lock().unwrap().last().unwrap().current, 2);

        handle.dec(100);
        assert_eq!(events.lock().unwrap().last().unwrap().current, 0);
    }
}
