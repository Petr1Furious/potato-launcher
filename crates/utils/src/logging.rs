use std::{
    fs::OpenOptions,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use log::LevelFilter;

pub fn setup_logger(
    log_file: &Path,
    append: bool,
    default_level: LevelFilter,
) -> anyhow::Result<()> {
    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(log_file)?;

    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|value| value.parse::<LevelFilter>().ok())
        .unwrap_or(default_level);

    fern::Dispatch::new()
        .level(level)
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {} {} - {}",
                format_system_time(SystemTime::now()),
                record.level(),
                record.target(),
                message
            ));
        })
        .chain(std::io::stdout())
        .chain(file)
        .apply()?;

    Ok(())
}

pub fn format_system_time(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    humantime::format_rfc3339_seconds(UNIX_EPOCH + duration).to_string()
}
