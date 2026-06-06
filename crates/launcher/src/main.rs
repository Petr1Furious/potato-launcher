#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;

use launcher_build_config::data_dir_name;
use log::LevelFilter;
use utils::logging::setup_logger;

fn main() -> anyhow::Result<()> {
    let launcher_dir = launcher_dir();
    setup_logger(
        &launcher_dir.join("logs").join("launcher.log"),
        true,
        LevelFilter::Info,
    )?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;

    let (backend_sender, backend_receiver, frontend_sender, frontend_receiver) =
        launcher_bridge::channel();

    let backend_dir = launcher_dir.clone();
    runtime.spawn(async move {
        if let Err(err) =
            launcher_backend::run(backend_dir, backend_receiver, frontend_sender).await
        {
            log::error!("Launcher backend stopped with an error: {err:?}");
        }
    });

    launcher_frontend::start(launcher_dir, backend_sender, frontend_receiver)
}

fn launcher_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(data_dir_name())
}
