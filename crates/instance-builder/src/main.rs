mod progress;
mod spec;

use clap::{Arg, Command};
use log::LevelFilter;
use spec::Spec;
use std::path::PathBuf;
use tokio::runtime::Runtime;
use utils::logging::setup_logger;

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
    setup_logger(&work_dir_path.join("builder.log"), true, LevelFilter::Info)?;

    let rt = Runtime::new().unwrap();
    let spec = rt.block_on(Spec::from_file(&spec_file_path))?;

    rt.block_on(spec.generate(&output_dir_path, &work_dir_path))
}
