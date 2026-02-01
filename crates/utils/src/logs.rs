use env_logger::Builder;
use log::LevelFilter;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

pub fn setup_logger(logs_path: &Path) {
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_path)
        .unwrap();

    let log_file = Mutex::new(log_file);

    let mut builder = Builder::new();
    builder.filter(None, LevelFilter::Info);
    builder.parse_default_env();

    builder.format(move |buf, record| {
        let mut log_file = log_file.lock().unwrap();
        writeln!(log_file, "{} - {}", record.level(), record.args()).unwrap();
        writeln!(buf, "{} - {}", record.level(), record.args())
    });

    builder.init();
}
