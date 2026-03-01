#[derive(Clone)]
pub struct Unit {
    pub name: String,
    pub size: u64,
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

#[derive(Clone, Copy, Default)]
pub struct NoProgressBar;

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
