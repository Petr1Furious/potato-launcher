use std::{
    process::{ExitStatus, exit},
    sync::Arc,
};

use launcher_auth::AccountData;
use log::error;
use shared::paths::get_logs_dir;
use tokio::{process::Child, runtime::Runtime, sync::Mutex};

use crate::{
    config::runtime_config::Config, lang::LangMessage, launcher::launch,
    version::complete_version_metadata::CompleteVersionMetadata,
};

enum LauncherStatus {
    NotLaunched,
    Running { child: Arc<Mutex<Child>> },
    Error,
    ProcessErrorCode(String),
}

pub struct LaunchState {
    status: LauncherStatus,
    force_launch: bool,
    launch_from_start: bool,
    ctx: egui::Context,
    watcher_handle: Option<tokio::task::JoinHandle<ExitStatus>>,
}

pub enum ForceLaunchResultSelect {
    Nothing,
    ForceLaunch,
    Cancel,
}

pub struct RenderUiParams {
    pub online: bool,
    pub disabled: bool,
}

impl LaunchState {
    pub fn new(launch_from_start: bool, ctx: egui::Context) -> Self {
        LaunchState {
            status: LauncherStatus::NotLaunched,
            force_launch: false,
            launch_from_start,
            ctx,
            watcher_handle: None,
        }
    }

    async fn child_watcher(child: Arc<Mutex<Child>>, ctx: egui::Context) -> ExitStatus {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let result = child.lock().await.try_wait();
            match result {
                Ok(Some(status)) => {
                    if cfg!(windows) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                            [670.0, 450.0].into(),
                        ));
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    }
                    ctx.request_repaint();
                    return status;
                }
                Ok(None) => {}
                Err(_) => {
                    ExitStatus::default();
                }
            }
        }
    }

    fn launch(
        &mut self,
        runtime: &Runtime,
        config: &Config,
        selected_instance: &CompleteVersionMetadata,
        account_data: &AccountData,
        online: bool,
    ) {
        match runtime.block_on(launch::launch(
            selected_instance,
            config,
            account_data,
            online,
        )) {
            Ok(child) => {
                let arc_child = Arc::new(Mutex::new(child));
                if config.hide_launcher_after_launch {
                    if cfg!(windows) {
                        self.ctx
                            .send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                        self.ctx
                            .send_viewport_cmd(egui::ViewportCommand::InnerSize([0.0, 0.0].into()));
                    } else {
                        self.ctx
                            .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                }
                self.watcher_handle =
                    Some(runtime.spawn(Self::child_watcher(arc_child.clone(), self.ctx.clone())));
                self.status = LauncherStatus::Running {
                    child: arc_child.clone(),
                };
            }
            Err(e) => {
                error!("Error launching Minecraft:\n{e:?}");
                self.status = LauncherStatus::Error;
            }
        }
    }

    pub fn update(&mut self, runtime: &Runtime, config: &Config) {
        match self.watcher_handle.take_if(|handle| handle.is_finished()) {
            None => {}
            Some(handle) => {
                let exit_status = runtime.block_on(handle).unwrap_or_default();
                if exit_status.success() {
                    if config.hide_launcher_after_launch {
                        exit(0);
                    }
                    self.status = LauncherStatus::NotLaunched;
                } else {
                    self.status = LauncherStatus::ProcessErrorCode(
                        exit_status.code().unwrap_or(-1).to_string(),
                    );
                }
            }
        }
    }

    fn big_button_clicked(ui: &mut egui::Ui, text: &str) -> bool {
        let button_text = egui::RichText::new(text)
            .size(20.0)
            .text_style(egui::TextStyle::Button);
        let button = egui::Button::new(button_text);
        ui.add_sized([ui.available_width(), 50.0], button).clicked()
    }

    pub fn render_ui(
        &mut self,
        runtime: &Runtime,
        ui: &mut egui::Ui,
        config: &mut Config,
        selected_instance: Option<Arc<CompleteVersionMetadata>>,
        account_data: Option<AccountData>,
        params: RenderUiParams,
    ) {
        let RenderUiParams { online, disabled } = params;

        let lang = config.lang;

        match &mut self.status {
            LauncherStatus::Running { child } => {
                ui.label(LangMessage::Running.to_string(lang));
                if ui
                    .button(LangMessage::KillMinecraft.to_string(lang))
                    .clicked()
                {
                    let mut child_lock = runtime.block_on(child.lock());
                    let _ = runtime.block_on(child_lock.kill());
                }
            }
            _ => {
                let button_text = if online {
                    LangMessage::Launch.to_string(lang)
                } else {
                    format!(
                        "{} ({})",
                        LangMessage::Launch.to_string(lang),
                        LangMessage::Offline.to_string(lang)
                    )
                };
                let enabled = selected_instance.is_some() && account_data.is_some() && !disabled;
                ui.add_enabled_ui(enabled, |ui| {
                    if Self::big_button_clicked(ui, &button_text)
                        || (enabled && (self.force_launch || self.launch_from_start))
                    {
                        self.launch_from_start = false;

                        self.force_launch = false;
                        self.launch(
                            runtime,
                            config,
                            &selected_instance.unwrap(),
                            &account_data.unwrap(),
                            online,
                        );
                    }
                });
            }
        }

        match &self.status {
            LauncherStatus::Error => {
                ui.label(LangMessage::LaunchError.to_string(lang));
            }
            LauncherStatus::ProcessErrorCode(e) => {
                ui.label(LangMessage::ProcessErrorCode(e.clone()).to_string(lang));
                if ui.button(LangMessage::OpenLogs.to_string(lang)).clicked() {
                    open::that(get_logs_dir(&config.get_launcher_dir())).unwrap();
                }
            }
            _ => {}
        }
    }

    pub fn render_download_ui(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        disabled: bool,
    ) -> ForceLaunchResultSelect {
        let lang = config.lang;

        if !self.force_launch {
            let mut button_clicked = false;
            ui.add_enabled_ui(!disabled, |ui| {
                if LaunchState::big_button_clicked(
                    ui,
                    &LangMessage::DownloadAndLaunch.to_string(lang),
                ) || (!disabled && self.launch_from_start)
                {
                    self.launch_from_start = false;

                    button_clicked = true;
                }
            });
            if button_clicked {
                self.force_launch = true;
                return ForceLaunchResultSelect::ForceLaunch;
            }
        } else {
            let mut cancel_clicked = false;
            if LaunchState::big_button_clicked(ui, &LangMessage::CancelLaunch.to_string(lang)) {
                self.force_launch = false;
                cancel_clicked = true;
            }
            if cancel_clicked {
                return ForceLaunchResultSelect::Cancel;
            }
        }
        ForceLaunchResultSelect::Nothing
    }
}

impl Drop for LaunchState {
    fn drop(&mut self) {}
}
