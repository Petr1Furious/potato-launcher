mod progress;
mod spec;

use clap::{Arg, Command};
use env_logger::Env;
use spec::Spec;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::path::PathBuf;
use tokio::runtime::Runtime;

struct TeeWriter {
    file: std::fs::File,
}

impl std::io::Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        self.file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.file.flush()
    }
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
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(work_dir_path.join("builder.log"))?;
    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(TeeWriter {
            file: log_file,
        })))
        .init();

    let rt = Runtime::new().unwrap();
    let spec = rt.block_on(Spec::from_file(&spec_file_path))?;
    let delete_remote_set: Option<HashSet<String>> = matches
        .get_many::<String>("delete_remote_instances")
        .map(|vals| vals.map(|s| s.to_string()).collect());

    rt.block_on(spec.generate(&output_dir_path, &work_dir_path, delete_remote_set.as_ref()))
}
