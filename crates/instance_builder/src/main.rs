mod progress;
mod spec;

use clap::{Arg, Command};
use fern::Dispatch;
use log::LevelFilter;
use spec::Spec;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

fn setup_logger_with_options(
    logs_path: &Path,
    append: bool,
    default_level: LevelFilter,
) -> anyhow::Result<()> {
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(logs_path)?;

    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|value| value.parse::<LevelFilter>().ok())
        .unwrap_or(default_level);

    Dispatch::new()
        .level(level)
        .format(|out, message, record| {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0);
            out.finish(format_args!("{} {} - {}", ts, record.level(), message));
        })
        .chain(std::io::stdout())
        .chain(log_file)
        .apply()?;

    Ok(())
}

fn parse_path(v: &str) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(v);
    if path.exists() {
        Ok(path)
    } else {
        Err(anyhow::Error::msg("The specified file does not exist"))
    }
}

fn main() -> anyhow::Result<()> {
    unsafe {
        std::env::set_var("RUST_LIB_BACKTRACE", "1");
    }

    let matches = Command::new("generate-instance")
        .about("Generates instances based on a specification file")
        .arg(
            Arg::new("spec_file")
                .help("Path to the specification file")
                .required(true)
                .short('s')
                .value_parser(parse_path),
        )
        .arg(
            Arg::new("output_dir")
                .help("Output directory")
                .default_value("./generated"),
        )
        .arg(
            Arg::new("work_dir")
                .help("Working directory")
                .default_value("./workdir"),
        )
        .arg(
            Arg::new("delete_remote_instances")
                .help("Comma-separated remote instance names to delete from fetched manifest")
                .long("delete-remote")
                .num_args(1..)
                .use_value_delimiter(true)
                .value_delimiter(',')
                .value_name("NAME"),
        )
        .get_matches();

    let spec_file = matches.get_one::<PathBuf>("spec_file").unwrap();
    let output_dir = matches.get_one::<String>("output_dir").unwrap();
    let output_dir = PathBuf::from(output_dir);
    let work_dir = matches.get_one::<String>("work_dir").unwrap();
    let work_dir = PathBuf::from(work_dir);

    let spec_file_path = spec_file.clone();
    let output_dir_path = output_dir.clone();
    let work_dir_path = work_dir.clone();

    std::fs::create_dir_all(&work_dir_path)?;
    setup_logger_with_options(&work_dir_path.join("builder.log"), true, LevelFilter::Info)?;

    let rt = Runtime::new().unwrap();
    let spec = rt.block_on(Spec::from_file(&spec_file_path))?;

    rt.block_on(spec.generate(&output_dir_path, &work_dir_path))
}
