use utils::progress::ProgressTracker;

#[derive(Clone)]
pub struct TerminalProgressBar {
    bar: indicatif::ProgressBar,
}

impl TerminalProgressBar {
    pub fn new(message: &str) -> Self {
        let bar = indicatif::ProgressBar::new(0);
        bar.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{msg} {bar:40.cyan/blue} {pos}/{len}")
                .unwrap(),
        );
        bar.set_message(message.to_string());
        Self { bar }
    }
}

impl ProgressTracker for TerminalProgressBar {
    fn set_length(&self, length: u64) {
        self.bar.set_length(length);
    }

    fn inc(&self, amount: u64) {
        self.bar.inc(amount);
    }

    fn finish(&self) {
        self.bar.finish();
    }

    fn reset(&self) {
        self.bar.set_length(0);
        self.bar.set_position(0);
    }
}
