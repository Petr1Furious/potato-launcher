use gpui::App;
use launcher_bridge::{ExitOutcome, MessageToFrontend, NotificationLevel};
use launcher_i18n::{self as t, set_lang};

use crate::{
    entity::{DataEntities, instance::InstanceProgressUpdate},
    notification_text::short_notification_text,
};

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
                handle,
                stage,
                current,
                total,
                message,
            } => {
                self.data.instances.update(cx, |entries, cx| {
                    entries.set_progress(
                        InstanceProgressUpdate::new(handle, stage, current, total, message),
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
                    entries.push(level, short_notification_text(&message), cx);
                });
            }
            MessageToFrontend::AuthPrompt { context, message } => {
                log::info!("Auth prompt received: {context:?} {message:?}");
                self.data.auth.update(cx, |session, cx| {
                    session.set_prompt(context, message, cx);
                });
            }
            MessageToFrontend::AuthPromptCleared => {
                self.data.auth.update(cx, |session, cx| {
                    session.clear(cx);
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
                self.data.notifications.update(cx, |entries, cx| {
                    entries.push(level, short_notification_text(&message), cx)
                });
            }
            MessageToFrontend::LocalCreateVersionsUpdated {
                versions,
                latest_release,
                error,
            } => {
                self.data.local_create.update(cx, |entries, cx| {
                    entries.apply_minecraft_versions(versions, latest_release, error, cx);
                });
            }
            MessageToFrontend::LoaderVersionsUpdated {
                minecraft_version,
                loader,
                versions,
                error,
            } => {
                self.data.local_create.update(cx, |entries, cx| {
                    entries.apply_loader_versions(minecraft_version, loader, versions, error, cx);
                });
            }
            MessageToFrontend::UpdateStatus(status) => {
                self.data
                    .update
                    .update(cx, |entries, cx| entries.apply(status, cx));
            }
            MessageToFrontend::JavaPathResolved { instance, path } => {
                self.data
                    .java_resolve
                    .update(cx, |cache, cx| cache.set(instance, path, cx));
            }
            MessageToFrontend::Quit => {
                cx.quit();
            }
        }
    }
}
