use gpui::App;
use launcher_auth::flow::AuthMessage;
use launcher_bridge::{ExitOutcome, MessageToFrontend, NotificationLevel};

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
                            "Minecraft exited successfully".to_string(),
                        )
                    }
                    ExitOutcome::ExitCode(code) => {
                        log::warn!("Instance {instance} exited with code {code}");
                        (
                            NotificationLevel::Error,
                            format!("Minecraft exited with code {code}"),
                        )
                    }
                    ExitOutcome::Terminated => {
                        log::info!("Instance {instance} was terminated");
                        (
                            NotificationLevel::Info,
                            "Minecraft was terminated".to_string(),
                        )
                    }
                    ExitOutcome::Error(error) => {
                        log::error!("Instance {instance} failed to launch: {error}");
                        (NotificationLevel::Error, format!("Launch failed: {error}"))
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
        AuthMessage::Link { url } => format!("Continue authentication in your browser: {url}"),
        AuthMessage::LinkCode { url, code } => {
            format!("Continue authentication in your browser: {url}\nCode: {code}")
        }
    }
}
