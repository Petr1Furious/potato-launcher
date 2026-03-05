use env_logger::Builder;
use env_logger::Env;
use log::LevelFilter;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

#[derive(thiserror::Error, Debug)]
pub enum LoggerSetupError {
    #[error("failed to open log file {}: {source}", path.display())]
    OpenLogFile {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("failed to initialize logger: {0}")]
    InitLogger(#[from] log::SetLoggerError),
}

#[derive(Clone, Debug)]
pub struct LoggerOptions {
    pub append: bool,
    pub default_level: LevelFilter,
}

impl Default for LoggerOptions {
    fn default() -> Self {
        Self {
            append: true,
            default_level: LevelFilter::Info,
        }
    }
}

pub fn setup_logger_with_options(
    logs_path: &Path,
    options: LoggerOptions,
) -> Result<(), LoggerSetupError> {
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(options.append)
        .truncate(!options.append)
        .open(logs_path)
        .map_err(|source| LoggerSetupError::OpenLogFile {
            path: logs_path.to_path_buf(),
            source,
        })?;

    let log_file = Mutex::new(log_file);

    let mut builder =
        Builder::from_env(Env::default().default_filter_or(options.default_level.as_str()));

    builder.format(move |buf, record| {
        let ts = buf.timestamp_millis();
        if let Ok(mut log_file) = log_file.lock() {
            let _ = writeln!(log_file, "{} {} - {}", ts, record.level(), record.args());
        }
        writeln!(buf, "{} {} - {}", ts, record.level(), record.args())
    });

    builder.try_init()?;
    Ok(())
}

pub fn setup_logger(logs_path: &Path) -> Result<(), LoggerSetupError> {
    setup_logger_with_options(logs_path, LoggerOptions::default())
}
