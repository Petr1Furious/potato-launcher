use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressStage {
    Checking,
    Downloading,
    Copying,
    Extracting,
    Metadata,
    Java,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressEvent {
    pub stage: ProgressStage,
    pub message: Option<String>,
    pub current: u64,
    pub total: u64,
    pub unit: Option<Unit>,
    pub finished: bool,
}

pub trait ProgressReporter: Sync + Send {
    fn event(&self, event: ProgressEvent);
}

pub trait ProgressTracker: Sync + Send {
    fn set_length(&self, length: u64);

    fn inc(&self, amount: u64);

    fn finish(&self);

    fn reset(&self) {
        self.set_length(0);
    }
}

pub trait ProgressBar<M>: ProgressTracker + Sync + Send {
    fn set_message(&self, message: M);

    fn set_unit(&self, unit: Unit);
}

#[derive(Clone, Debug)]
struct ProgressState {
    stage: ProgressStage,
    message: Option<String>,
    current: u64,
    total: u64,
    unit: Option<Unit>,
    finished: bool,
}

impl ProgressState {
    fn event(&self) -> ProgressEvent {
        ProgressEvent {
            stage: self.stage.clone(),
            message: self.message.clone(),
            current: self.current,
            total: self.total,
            unit: self.unit.clone(),
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
    pub fn new(reporter: R, stage: ProgressStage) -> Self {
        Self {
            reporter,
            state: Arc::new(Mutex::new(ProgressState {
                stage,
                message: None,
                current: 0,
                total: 0,
                unit: None,
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

    pub fn with_unit(self, unit: Unit) -> Self {
        self.update(|state| {
            state.unit = Some(unit);
        });
        self
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

    fn finish(&self) {
        self.update(|state| {
            state.finished = true;
        });
    }

    fn reset(&self) {
        self.update(|state| {
            state.current = 0;
            state.total = 0;
            state.finished = false;
        });
    }
}

impl<R, M> ProgressBar<M> for ProgressHandle<R>
where
    R: ProgressReporter,
    M: Into<String>,
{
    fn set_message(&self, message: M) {
        self.update(|state| {
            state.message = Some(message.into());
        });
    }

    fn set_unit(&self, unit: Unit) {
        self.update(|state| {
            state.unit = Some(unit);
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

impl<M> ProgressBar<M> for NoProgressBar {
    fn set_message(&self, _message: M) {}
    fn set_unit(&self, _unit: Unit) {}
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
        let handle = ProgressHandle::new(reporter, ProgressStage::Downloading)
            .with_message("Downloading files")
            .with_unit(Unit {
                name: "files".to_string(),
                size: 1,
            });

        handle.set_length(3);
        handle.inc(1);
        handle.inc(2);
        handle.finish();

        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| {
            event.stage == ProgressStage::Downloading
                && event.message.as_deref() == Some("Downloading files")
                && event.unit.as_ref().is_some_and(|unit| unit.name == "files")
        }));
        assert_eq!(events.last().unwrap().current, 3);
        assert_eq!(events.last().unwrap().total, 3);
        assert!(events.last().unwrap().finished);
    }

    #[test]
    fn progress_handle_reset_emits_zeroed_unfinished_event() {
        let reporter = CapturingReporter::default();
        let events = reporter.events.clone();
        let handle = ProgressHandle::new(reporter, ProgressStage::Checking);

        handle.set_length(10);
        handle.inc(4);
        handle.reset();

        let last = events.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.current, 0);
        assert_eq!(last.total, 0);
        assert!(!last.finished);
    }
}
