#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod constants;
mod lang;
mod launcher;
mod update_app;
mod utils;
mod version;

use clap::{Arg, ArgAction, Command};
use config::runtime_config::{Config, get_logs_path};
use utils::set_sigint_handler;

use shared::logs::setup_logger;

fn main() {
    unsafe {
        std::env::set_var("RUST_LIB_BACKTRACE", "1");
        // for some reason this is needed on macOS for minecraft process not to crash with
        // "Assertion failed: (count <= len && "snprintf() output has been truncated"), function LOAD_ERROR, file dispatch.c, line 74."
        std::env::remove_var("DYLD_FALLBACK_LIBRARY_PATH");
    }

    set_sigint_handler();
    setup_logger(&get_logs_path());

    let matches = Command::new("generate-instance")
        .about("Generates instances based on a specification file")
        .arg(
            Arg::new("launch")
                .help("Launch the game in the last used configuration")
                .long("launch")
                .short('l')
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let config = Config::load();
    app::unified_app::run_gui(config, matches.get_flag("launch"));
}
