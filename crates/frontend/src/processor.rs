use gpui::App;
use launcher_auth::flow::AuthMessage;
use launcher_bridge::{ExitOutcome, MessageToFrontend, NotificationLevel};
use launcher_i18n::{self as t, set_lang};

use crate::entity::{DataEntities, instance::InstanceProgressUpdate};

pub struct Processor {
    data: DataEntities,
}

impl Processor {
    pub fn new(data: DataEntities) -> Self {
        Self { data }
    }

    pub fn process(&mut self, message: MessageToFrontend, cx: &mut App) {
        match message {
            MessageToFrontend::InstancesUpdated(instances) => {
                self.data
                    .instances
                    .update(cx, |entries, cx| entries.replace(instances, cx));
            }
            MessageToFrontend::InstanceProgress {
                id,
                stage,
                current,
                total,
                message,
            } => {
                self.data.instances.update(cx, |entries, cx| {
                    entries.set_progress(
                        InstanceProgressUpdate::new(id, stage, current, total, message),
                        cx,
                    );
                });
            }
            MessageToFrontend::AccountsUpdated(accounts) => {
                self.data
                    .accounts
                    .update(cx, |entries, cx| entries.replace(accounts, cx));
            }
            MessageToFrontend::BackendsUpdated { backends } => {
                self.data
                    .backends
                    .update(cx, |entries, cx| entries.replace(backends, cx));
            }
            MessageToFrontend::SettingsUpdated(settings) => {
                set_lang(&settings.language);
                self.data
                    .settings
                    .update(cx, |entries, cx| entries.replace(settings, cx));
            }
            MessageToFrontend::Notification { level, message } => {
                match level {
                    NotificationLevel::Error => log::error!("{message}"),
                    NotificationLevel::Warning => log::warn!("{message}"),
                    NotificationLevel::Info | NotificationLevel::Success => log::info!("{message}"),
                }
                self.data.notifications.update(cx, |entries, cx| {
                    entries.push(level, message.to_string(), cx);
                });
            }
            MessageToFrontend::AuthPrompt(prompt) => {
                log::info!("Auth prompt received: {prompt:?}");
                self.data.notifications.update(cx, |entries, cx| {
                    entries.push(NotificationLevel::Info, auth_prompt_message(prompt), cx);
                });
            }
            MessageToFrontend::LaunchFinished { instance, exit } => {
                let (level, message) = match exit {
                    ExitOutcome::Success => {
                        log::info!("Instance {instance} exited successfully");
                        (
                            NotificationLevel::Success,
                            t::notifications::minecraft_exited_successfully().to_string(),
                        )
                    }
                    ExitOutcome::ExitCode(code) => {
                        log::warn!("Instance {instance} exited with code {code}");
                        (
                            NotificationLevel::Error,
                            t::notifications::minecraft_exited_with_code(code),
                        )
                    }
                    ExitOutcome::Terminated => {
                        log::info!("Instance {instance} was terminated");
                        (
                            NotificationLevel::Info,
                            t::notifications::minecraft_terminated().to_string(),
                        )
                    }
                    ExitOutcome::Error(error) => {
                        log::error!("Instance {instance} failed to launch: {error}");
                        (
                            NotificationLevel::Error,
                            t::notifications::launch_failed(error.to_string()),
                        )
                    }
                };
                self.data
                    .notifications
                    .update(cx, |entries, cx| entries.push(level, message, cx));
            }
            MessageToFrontend::Quit => {
                cx.quit();
            }
        }
    }
}

fn auth_prompt_message(prompt: AuthMessage) -> String {
    match prompt {
        AuthMessage::Link { url } => t::auth::continue_in_browser(url),
        AuthMessage::LinkCode { url, code } => t::auth::continue_in_browser_with_code(url, code),
    }
}
