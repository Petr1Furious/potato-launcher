use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    process::Command,
};

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, relative};
use gpui_component::{
    ActiveTheme, Disableable, StyledExt,
    button::Button,
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement,
    v_flex,
};
use launcher_auth::providers::{
    AuthProviderConfig, ElyByAuthProvider, MicrosoftAuthProvider, TGAuthProvider,
};
use launcher_bridge::{
    AccountView, BackendFetchState, BackendStatus, BackendSender, InstanceLiveStatus,
    InstanceOrigin, InstanceView, LauncherSettingsView, MessageToBackend, NotificationLevel,
};
use url::Url;
use uuid::Uuid;

use crate::entity::{
    DataEntities,
    account::AccountsUpdatedEvent,
    backend::BackendsUpdatedEvent,
    instance::InstancesUpdatedEvent,
    notification::NotificationEntries,
    settings::LauncherSettingsUpdatedEvent,
};

pub struct InstancesPage {
    data: DataEntities,
    selected_instance: Option<Uuid>,
    show_global_settings: bool,
    show_backend_settings: bool,
    show_accounts_panel: bool,
    preferred_add_provider: Option<AuthProviderConfig>,
    pending_delete: Option<Uuid>,
    hidden_launches: HashSet<Uuid>,
    backend_url_input: gpui::Entity<InputState>,
    offline_nickname_input: gpui::Entity<InputState>,
    telegram_base_url_input: gpui::Entity<InputState>,
    elyby_client_id_input: gpui::Entity<InputState>,
    elyby_client_secret_input: gpui::Entity<InputState>,
    elyby_launcher_name_input: gpui::Entity<InputState>,
    memory_input: gpui::Entity<InputState>,
    jvm_flags_input: gpui::Entity<InputState>,
    _instances_subscription: gpui::Subscription,
    _backends_subscription: gpui::Subscription,
    _accounts_subscription: gpui::Subscription,
    _settings_subscription: gpui::Subscription,
}

impl InstancesPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _instances_subscription = cx
            .subscribe(&data.instances, |_, _, _: &InstancesUpdatedEvent, cx| {
                cx.notify()
            });
        let _backends_subscription =
            cx.subscribe(&data.backends, |_, _, _: &BackendsUpdatedEvent, cx| cx.notify());
        let _accounts_subscription =
            cx.subscribe(&data.accounts, |_, _, _: &AccountsUpdatedEvent, cx| cx.notify());
        let _settings_subscription = cx
            .subscribe(&data.settings, |_, _, _: &LauncherSettingsUpdatedEvent, cx| {
                cx.notify()
            });
        let backend_url_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("https://example.com/manifest.json"));
        let offline_nickname_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Offline nickname"));
        let telegram_base_url_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Telegram auth base URL"));
        let elyby_client_id_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Client ID"));
        let elyby_client_secret_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Client secret"));
        let elyby_launcher_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Launcher name"));
        let memory_input = cx.new(|cx| InputState::new(window, cx).placeholder("Memory MiB"));
        let jvm_flags_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Extra JVM flags"));

        Self {
            data: data.clone(),
            selected_instance: None,
            show_global_settings: false,
            show_backend_settings: false,
            show_accounts_panel: false,
            preferred_add_provider: None,
            pending_delete: None,
            hidden_launches: HashSet::new(),
            backend_url_input,
            offline_nickname_input,
            telegram_base_url_input,
            elyby_client_id_input,
            elyby_client_secret_input,
            elyby_launcher_name_input,
            memory_input,
            jvm_flags_input,
            _instances_subscription,
            _backends_subscription,
            _accounts_subscription,
            _settings_subscription,
        }
    }

    pub fn open_global_settings(&mut self, cx: &mut Context<Self>) {
        let should_close = self.show_global_settings;
        self.selected_instance = None;
        self.show_global_settings = !should_close;
        self.show_backend_settings = false;
        self.show_accounts_panel = false;
        self.preferred_add_provider = None;
        cx.notify();
    }

    pub fn open_backend_settings(&mut self, cx: &mut Context<Self>) {
        let should_close = self.show_backend_settings;
        self.selected_instance = None;
        self.show_global_settings = false;
        self.show_backend_settings = !should_close;
        self.show_accounts_panel = false;
        self.preferred_add_provider = None;
        cx.notify();
    }

    pub fn open_accounts_panel(&mut self, cx: &mut Context<Self>) {
        let should_close = self.show_accounts_panel;
        self.selected_instance = None;
        self.show_global_settings = false;
        self.show_backend_settings = false;
        self.show_accounts_panel = !should_close;
        self.preferred_add_provider = None;
        cx.notify();
    }
}

impl Render for InstancesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let instances = self.data.instances.read(cx).entries.clone();
        let backends = self.data.backends.read(cx).backends.clone();
        let accounts = self.data.accounts.read(cx).accounts.clone();
        let launcher_settings = self.data.settings.read(cx).settings;
        if launcher_settings.hide_window_after_launch
            && let Some(instance) = instances
                .iter()
                .find(|instance| matches!(instance.status, InstanceLiveStatus::Launching))
            && self.hidden_launches.insert(instance.id)
        {
            window.minimize_window();
        }
        self.hidden_launches.retain(|id| {
            instances.iter().any(|instance| {
                instance.id == *id
                    && matches!(
                        instance.status,
                        InstanceLiveStatus::Launching | InstanceLiveStatus::Running
                    )
            })
        });
        let groups = group_instances(&instances);
        let backend_names = backend_display_names(&backends);
        if self
            .selected_instance
            .is_some_and(|id| !instances.iter().any(|instance| instance.id == id))
        {
            self.selected_instance = None;
        }

        let mut sections = Vec::new();
        if let Some(local) = groups.local
            && !local.is_empty() {
                sections.push(section(
                    "Local".to_string(),
                    None,
                    local,
                    launcher_settings.hide_usernames_in_cards,
                    &self.data.backend_sender,
                    cx,
                ));
            }

        for backend in &backends {
            let instances = groups
                .backend
                .get(backend.url.as_str())
                .cloned()
                .unwrap_or_default();
            if instances.is_empty() {
                let title = backend_names
                    .get(backend.url.as_str())
                    .cloned()
                    .unwrap_or_else(|| backend_display_name(&backend.url, false));
                if let Some(section) = backend_empty_state_section(backend, title, cx) {
                    sections.push(section);
                }
                continue;
            }

            sections.push(section(
                backend_names
                    .get(backend.url.as_str())
                    .cloned()
                    .unwrap_or_else(|| backend_display_name(&backend.url, false)),
                Some(backend),
                instances,
                launcher_settings.hide_usernames_in_cards,
                &self.data.backend_sender,
                cx,
            ));
        }

        if sections.is_empty()
            && !self.show_global_settings
            && !self.show_backend_settings
            && !self.show_accounts_panel
            && self.selected_instance.is_none()
        {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_xl().child("No instances yet"))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Open Backends to add a manifest URL, or create a local instance later.",
                        ),
                )
                .into_any_element()
        } else {
            let separated_sections = separated_backend_sections(sections, cx);
            let list = v_flex()
                .size_full()
                .p_4()
                .overflow_y_scrollbar()
                .children(separated_sections);
            let side_panel = if let Some(selected) = self
                .selected_instance
                .and_then(|id| instances.iter().find(|instance| instance.id == id).cloned())
            {
                Some(
                    self.settings_panel(selected, accounts.clone(), cx)
                        .into_any_element(),
                )
            } else if self.show_backend_settings {
                Some(
                    self.backend_settings_panel(backends.clone(), backend_names.clone(), cx)
                        .into_any_element(),
                )
            } else if self.show_accounts_panel {
                Some(self.accounts_panel(accounts.clone(), cx).into_any_element())
            } else if self.show_global_settings {
                Some(
                    self.global_settings_panel(launcher_settings, cx)
                        .into_any_element(),
                )
            } else {
                None
            };

            if let Some(side_panel) = side_panel {
                h_flex()
                    .size_full()
                    .child(div().flex_1().min_w_0().size_full().child(list))
                    .child(side_panel)
                    .into_any_element()
            } else {
                list.into_any_element()
            }
        }
    }
}

impl InstancesPage {
    fn settings_panel(
        &self,
        instance: InstanceView,
        accounts: Vec<AccountView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sender = self.data.backend_sender.clone();
        let id = instance.id;
        let log_path = self
            .data
            .launcher_dir
            .join("logs")
            .join("latest_minecraft_launch.log");
        let instance_path = self
            .data
            .launcher_dir
            .join("instances")
            .join(instance.dir_name.as_ref());

        v_flex()
            .w_96()
            .min_w_96()
            .h_full()
            .gap_4()
            .p_4()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .overflow_y_scrollbar()
            .child(
                h_flex()
                    .justify_between()
                    .items_start()
                    .gap_2()
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xl()
                                    .font_semibold()
                                    .line_clamp(1)
                                    .child(instance.display_name.to_string()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(instance.dir_name.to_string()),
                            ),
                    )
                    .child(
                        Button::new("close-instance-details")
                            .label("Close")
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.selected_instance = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(detail_section(
                "Status",
                v_flex()
                    .gap_2()
                    .child(div().child(status_label(&instance)))
                    .when_some(status_error(&instance.status), |this, error| {
                        this.child(error_alert("Details", error, cx))
                    })
                    .when_some(progress_ratio(&instance.status), |this, value| {
                        this.child(progress_bar(value, cx))
                    }),
                cx,
            ))
            .children(account_detail_sections(
                &instance,
                accounts.clone(),
                sender.clone(),
                cx,
            ))
            .child(detail_section(
                "Runtime",
                runtime_section(
                    &instance,
                    self.memory_input.clone(),
                    self.jvm_flags_input.clone(),
                    sender.clone(),
                    cx,
                ),
                cx,
            ))
            .child(detail_section(
                "Actions",
                action_section(
                    instance.clone(),
                    self.pending_delete == Some(id),
                    sender.clone(),
                    cx,
                ),
                cx,
            ))
            .child(detail_section(
                "Logs",
                v_flex()
                    .gap_2()
                    .child(
                        Button::new(format!("open-instance-folder-{id}"))
                            .label("Open Instance Folder")
                            .disabled(matches!(instance.status, InstanceLiveStatus::NotInstalled))
                            .on_click({
                                let notifications = self.data.notifications.clone();
                                move |_, _, cx| {
                                    if let Err(err) = open_path(&instance_path) {
                                        notifications.update(cx, |entries, cx| {
                                            entries.push(
                                                NotificationLevel::Error,
                                                format!("Failed to open instance folder: {err}"),
                                                cx,
                                            );
                                        });
                                    }
                                }
                            }),
                    )
                    .child(
                        Button::new(format!("open-logs-{id}"))
                            .label("Open Latest Launch Log")
                            .on_click({
                                let notifications = self.data.notifications.clone();
                                move |_, _, cx| {
                                    if let Err(err) = open_path(&log_path) {
                                        notifications.update(cx, |entries, cx| {
                                            entries.push(
                                                NotificationLevel::Error,
                                                format!("Failed to open logs: {err}"),
                                                cx,
                                            );
                                        });
                                    }
                                }
                            }),
                    ),
                cx,
            ))
    }

    fn global_settings_panel(
        &self,
        settings: LauncherSettingsView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sender = self.data.backend_sender.clone();
        v_flex()
            .w_96()
            .min_w_96()
            .h_full()
            .gap_4()
            .p_4()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .overflow_y_scrollbar()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_xl().font_semibold().child("Launcher Settings"))
                    .child(
                        Button::new("close-global-settings")
                            .label("Close")
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.show_global_settings = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                launcher_settings_section(
                    settings,
                    self.data.launcher_dir.clone(),
                    self.data.notifications.clone(),
                    sender,
                    cx,
                )
                .w_full()
                .min_w_0(),
            )
    }

    fn accounts_panel(
        &self,
        accounts: Vec<AccountView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sender = self.data.backend_sender.clone();
        v_flex()
            .w_96()
            .min_w_96()
            .h_full()
            .gap_4()
            .p_4()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .overflow_y_scrollbar()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_xl().font_semibold().child("Accounts"))
                    .child(
                        Button::new("close-accounts")
                            .label("Close")
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.show_accounts_panel = false;
                                page.preferred_add_provider = None;
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.preferred_add_provider.as_ref(), |this, provider| {
                this.child(detail_section(
                    "Suggested Account",
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("This instance needs {}", provider_label(provider))),
                        )
                        .child(add_provider_button(
                            provider.clone(),
                            "Add Suggested Account",
                            sender.clone(),
                        )),
                    cx,
                ))
            })
            .child(detail_section(
                "Accounts",
                accounts_section(accounts, sender.clone(), cx),
                cx,
            ))
            .child(detail_section(
                "Add Account",
                add_account_section(
                    self.offline_nickname_input.clone(),
                    self.telegram_base_url_input.clone(),
                    self.elyby_client_id_input.clone(),
                    self.elyby_client_secret_input.clone(),
                    self.elyby_launcher_name_input.clone(),
                    sender,
                    cx,
                ),
                cx,
            ))
    }

    fn backend_settings_panel(
        &self,
        backends: Vec<BackendStatus>,
        backend_names: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sender = self.data.backend_sender.clone();
        let input_value = self.backend_url_input.read(cx).value().to_string();
        let trimmed = input_value.trim().to_string();
        let parsed_url = Url::parse(&trimmed).ok();
        let valid_url = parsed_url
            .as_ref()
            .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
        let show_error = !trimmed.is_empty() && !valid_url;
        let add_backend = Button::new("backend-panel-add")
            .label("Add Backend")
            .disabled(!valid_url)
            .on_click({
                let sender = sender.clone();
                let input = self.backend_url_input.clone();
                move |_, window, cx| {
                    let value = input.read(cx).value().to_string();
                    let Ok(url) = Url::parse(value.trim()) else {
                        return;
                    };
                    if !matches!(url.scheme(), "http" | "https") {
                        return;
                    }
                    sender.send(MessageToBackend::AddBackendUrl(url));
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            });

        v_flex()
            .w_96()
            .min_w_96()
            .h_full()
            .gap_4()
            .p_4()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .overflow_y_scrollbar()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_xl().font_semibold().child("Backends"))
                    .child(
                        Button::new("close-backend-settings")
                            .label("Close")
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.show_backend_settings = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(detail_section(
                "Add Backend",
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(Input::new(&self.backend_url_input))
                    .child(add_backend)
                    .when(show_error, |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().red)
                                .child("Enter an http(s) manifest URL"),
                        )
                    }),
                cx,
            ))
            .child(detail_section(
                "Configured Backends",
                backend_list(backends, backend_names, sender, cx),
                cx,
            ))
    }
}

#[derive(Default)]
struct InstanceGroups {
    local: Option<Vec<InstanceView>>,
    backend: BTreeMap<String, Vec<InstanceView>>,
}

fn group_instances(instances: &[InstanceView]) -> InstanceGroups {
    let mut groups = InstanceGroups::default();
    for instance in instances {
        match &instance.origin {
            InstanceOrigin::Local => {
                groups
                    .local
                    .get_or_insert_with(Vec::new)
                    .push(instance.clone());
            }
            InstanceOrigin::Backend { url } => {
                groups
                    .backend
                    .entry(url.as_str().to_string())
                    .or_default()
                    .push(instance.clone());
            }
        }
    }
    groups
}

fn separated_backend_sections(
    sections: Vec<gpui::Div>,
    cx: &mut Context<InstancesPage>,
) -> Vec<gpui::Div> {
    let mut separated = Vec::with_capacity(sections.len().saturating_mul(2).saturating_sub(1));
    for (index, section) in sections.into_iter().enumerate() {
        if index > 0 {
            separated.push(div().mx_6().my_4().h(px(1.0)).bg(cx.theme().border));
        }
        separated.push(section);
    }
    separated
}

fn section(
    title: String,
    backend: Option<&BackendStatus>,
    instances: Vec<InstanceView>,
    hide_usernames: bool,
    sender: &BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let cards = instances
        .into_iter()
        .map(|instance| instance_card(instance, hide_usernames, sender.clone(), cx))
        .collect::<Vec<_>>();

    let fetch_state = backend
        .map(|backend| fetch_state_label(&backend.fetch_state))
        .unwrap_or_else(|| "local only".to_string());
    let fetch_color = if let Some(backend) = backend {
        match &backend.fetch_state {
            BackendFetchState::Offline | BackendFetchState::Error(_) => cx.theme().red,
            BackendFetchState::Fetching => cx.theme().yellow,
            BackendFetchState::Fetched { .. } => cx.theme().foreground,
            BackendFetchState::NotFetched => cx.theme().muted_foreground,
        }
    } else {
        cx.theme().muted_foreground
    };

    v_flex()
        .gap_3()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(div().text_lg().font_semibold().child(title))
                .child(div().text_sm().text_color(fetch_color).child(fetch_state)),
        )
        .child(h_flex().flex_wrap().gap_3().children(cards))
}

fn backend_empty_state_section(
    backend: &BackendStatus,
    title: String,
    cx: &mut Context<InstancesPage>,
) -> Option<gpui::Div> {
    let fetch_state = fetch_state_label(&backend.fetch_state);
    let fetch_color = match &backend.fetch_state {
        BackendFetchState::Offline | BackendFetchState::Error(_) => cx.theme().red,
        BackendFetchState::Fetching => cx.theme().yellow,
        BackendFetchState::Fetched { .. } => cx.theme().foreground,
        BackendFetchState::NotFetched => cx.theme().muted_foreground,
    };
    let (message_title, message) = match &backend.fetch_state {
        BackendFetchState::Fetching => (
            format!("Refreshing {title}"),
            "The backend has not returned an instance list yet.".to_string(),
        ),
        BackendFetchState::Offline => (
            format!("{title} is offline"),
            "Installed instances from this backend will appear here when known locally."
                .to_string(),
        ),
        BackendFetchState::Error(error) => (format!("Failed to fetch {title}"), error.to_string()),
        BackendFetchState::NotFetched => (
            format!("{title} has not been fetched"),
            "Use Refresh to try fetching this backend.".to_string(),
        ),
        BackendFetchState::Fetched { .. } => return None,
    };
    let error_like = matches!(
        &backend.fetch_state,
        BackendFetchState::Offline | BackendFetchState::Error(_)
    );

    Some(
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_lg().font_semibold().child(title))
                    .child(div().text_sm().text_color(fetch_color).child(fetch_state)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(if error_like {
                        cx.theme().red
                    } else {
                        cx.theme().border
                    })
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .font_semibold()
                            .line_clamp(2)
                            .child(message_title),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .line_clamp(3)
                            .child(message),
                    ),
            ),
    )
}

fn instance_card(
    instance: InstanceView,
    hide_usernames: bool,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let orphaned = instance.is_orphaned();
    let installed = instance.locally_installed;
    let status = status_label(&instance);
    let progress = progress_ratio(&instance.status);
    let action = action_button(instance.clone(), sender, cx);
    let details_id = instance.id;
    let show_dir_name = instance.dir_name.as_ref() != instance.display_name.as_ref();
    let settings = Button::new(format!("settings-{details_id}"))
        .label("Settings")
        .on_click(cx.listener(move |page, _, _, cx| {
            page.show_global_settings = false;
            page.show_backend_settings = false;
            page.show_accounts_panel = false;
            page.selected_instance =
                (page.selected_instance != Some(details_id)).then_some(details_id);
            cx.notify();
        }));

    v_flex()
        .w_64()
        .min_h_40()
        .p_3()
        .gap_3()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(if installed {
            cx.theme().primary
        } else {
            cx.theme().border
        })
        .child(
            v_flex()
                .min_w_0()
                .child(
                    div()
                        .font_semibold()
                        .line_clamp(1)
                        .child(instance.display_name.to_string()),
                )
                .when(show_dir_name, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .line_clamp(1)
                            .child(instance.dir_name.to_string()),
                    )
                }),
        )
        .when(!hide_usernames, |this| {
            this.child(account_summary(&instance, cx))
        })
        .child(status_badge(&instance, orphaned, status, cx))
        .child(progress_slot(progress, cx))
        .child(card_actions(action, settings))
}

fn progress_slot(value: Option<f32>, cx: &mut Context<InstancesPage>) -> gpui::Div {
    div()
        .w_full()
        .h(px(6.0))
        .when_some(value, |this, value| this.child(progress_bar(value, cx)))
}

fn account_summary(instance: &InstanceView, cx: &mut Context<InstancesPage>) -> gpui::Div {
    let account = if instance.launch_blocked_reason.is_some() {
        "Add account".to_string()
    } else {
        instance
            .effective_account_username
            .as_ref()
            .map(|username| username.to_string())
            .unwrap_or_else(|| "No account selected".to_string())
    };
    let provider = {
        let label = instance
            .effective_auth_provider
            .as_ref()
            .or(instance.auth_provider.as_ref())
            .map(provider_type_label)
            .unwrap_or_else(|| "Any provider".to_string());
        if instance.account_override.is_some() {
            format!("{label}, Override")
        } else {
            label
        }
    };

    h_flex()
        .min_w_0()
        .gap_1()
        .text_xs()
        .line_clamp(1)
        .child(div().text_color(cx.theme().foreground).child(account))
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(format!("({provider})")),
        )
}

fn status_badge(
    instance: &InstanceView,
    orphaned: bool,
    status: String,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let is_error = matches!(
        instance.status,
        InstanceLiveStatus::InstallFailed(_) | InstanceLiveStatus::LaunchFailed(_)
    );
    let is_blocked = instance.launch_blocked_reason.is_some();
    let color = if orphaned || is_error {
        cx.theme().red.opacity(0.16)
    } else if is_blocked {
        cx.theme().yellow.opacity(0.18)
    } else {
        cx.theme().muted
    };

    div()
        .w_full()
        .min_h_8()
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .bg(color)
        .child(div().text_xs().text_center().line_clamp(2).child(status))
}

fn card_actions(primary: Button, settings: Button) -> gpui::Div {
    h_flex()
        .w_full()
        .gap_2()
        .child(div().flex_1().child(primary.w_full()))
        .child(div().flex_1().child(settings.w_full()))
}

fn action_button(
    instance: InstanceView,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> Button {
    match instance.status {
        InstanceLiveStatus::Installing { .. } => Button::new(format!("cancel-{}", instance.id))
            .label("Cancel")
            .on_click(move |_, _, _| sender.send(MessageToBackend::CancelInstall(instance.id))),
        InstanceLiveStatus::NotInstalled | InstanceLiveStatus::Outdated => {
            Button::new(format!("install-{}", instance.id))
                .label(if matches!(instance.status, InstanceLiveStatus::Outdated) {
                    "Update"
                } else {
                    "Install"
                })
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::InstallInstance {
                        id: instance.id,
                        force_overwrite: false,
                    });
                })
        }
        InstanceLiveStatus::Launching => Button::new(format!("launching-{}", instance.id))
            .label("Launching")
            .disabled(true),
        InstanceLiveStatus::Running => Button::new(format!("kill-{}", instance.id))
            .label("Kill")
            .on_click(move |_, _, _| sender.send(MessageToBackend::KillInstance(instance.id))),
        InstanceLiveStatus::Installed | InstanceLiveStatus::OrphanedFromBackend => {
            if instance.launch_blocked_reason.is_some() {
                let provider = instance.auth_provider.clone();
                return Button::new(format!("add-account-{}", instance.id))
                    .label("Add Account")
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.selected_instance = None;
                        page.show_accounts_panel = true;
                        page.show_global_settings = false;
                        page.show_backend_settings = false;
                        page.preferred_add_provider = provider.clone();
                        cx.notify();
                    }));
            }
            Button::new(format!("play-{}", instance.id))
                .label("Play")
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::Launch {
                        instance: instance.id,
                        account: None,
                    });
                })
        }
        InstanceLiveStatus::InstallFailed(_) => Button::new(format!("retry-{}", instance.id))
            .label("Retry")
            .on_click(move |_, _, _| {
                sender.send(MessageToBackend::InstallInstance {
                    id: instance.id,
                    force_overwrite: false,
                });
            }),
        InstanceLiveStatus::LaunchFailed(_) => Button::new(format!("play-again-{}", instance.id))
            .label(if instance.launch_blocked_reason.is_some() {
                "Add Account"
            } else {
                "Play Again"
            })
            .on_click({
                let provider = instance.auth_provider.clone();
                cx.listener(move |page, _, _, cx| {
                    if instance.launch_blocked_reason.is_some() {
                        page.selected_instance = None;
                        page.show_accounts_panel = true;
                        page.show_global_settings = false;
                        page.show_backend_settings = false;
                        page.preferred_add_provider = provider.clone();
                        cx.notify();
                    } else {
                        sender.send(MessageToBackend::Launch {
                            instance: instance.id,
                            account: None,
                        });
                    }
                })
            }),
    }
}

fn progress_bar(value: f32, cx: &mut Context<InstancesPage>) -> gpui::Div {
    div()
        .w_full()
        .relative()
        .h(px(6.0))
        .rounded(px(4.0))
        .bg(cx.theme().muted)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(relative(value))
                .rounded(px(4.0))
                .bg(cx.theme().progress_bar),
        )
}

fn detail_section(title: &str, content: gpui::Div, cx: &mut Context<InstancesPage>) -> gpui::Div {
    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .p_3()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .w_full()
                .min_w_0()
                .font_semibold()
                .child(title.to_string()),
        )
        .child(content.w_full().min_w_0())
}

fn error_alert(title: &str, message: String, cx: &mut Context<InstancesPage>) -> gpui::Div {
    v_flex()
        .gap_1()
        .p_2()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().red)
        .bg(cx.theme().red.opacity(0.12))
        .child(div().font_semibold().child(title.to_string()))
        .child(div().text_sm().child(message))
}

fn account_detail_sections(
    instance: &InstanceView,
    accounts: Vec<AccountView>,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> Vec<gpui::Div> {
    let Some(required_provider) = instance.auth_provider.as_ref() else {
        return Vec::new();
    };

    let matching_accounts = accounts
        .iter()
        .filter(|account| &account.provider == required_provider)
        .cloned()
        .collect::<Vec<_>>();
    let other_accounts = accounts
        .iter()
        .filter(|account| &account.provider != required_provider)
        .cloned()
        .collect::<Vec<_>>();

    let mut sections = Vec::new();
    let required_provider_for_add = required_provider.clone();
    sections.push(detail_section(
        "Account",
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Required provider: {}",
                        provider_label(required_provider)
                    )),
            )
            .when(matching_accounts.is_empty(), |this| {
                this.child(
                    Button::new(format!("add-required-account-{}", instance.id))
                        .label("Add Account")
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.selected_instance = None;
                            page.show_accounts_panel = true;
                            page.show_backend_settings = false;
                            page.show_global_settings = false;
                            page.preferred_add_provider = Some(required_provider_for_add.clone());
                            cx.notify();
                        })),
                )
            })
            .children(
                matching_accounts.into_iter().map(|account| {
                    account_select_row(instance, account, sender.clone(), false, cx)
                }),
            ),
        cx,
    ));

    if !other_accounts.is_empty() {
        sections.push(detail_section(
            "Account Override",
            v_flex().gap_2().children(
                other_accounts
                    .into_iter()
                    .map(|account| account_select_row(instance, account, sender.clone(), true, cx)),
            ),
            cx,
        ));
    }

    sections
}

fn account_is_selected(
    instance: &InstanceView,
    account: &AccountView,
    override_section: bool,
) -> bool {
    if override_section {
        return instance.account_override.as_ref() == Some(&account.key);
    }

    if instance.account_override.is_some() {
        return false;
    }

    if let Some(selected) = instance.selected_account.as_ref() {
        return selected == &account.key;
    }

    instance.effective_auth_provider.as_ref() == Some(&account.provider)
        && instance.effective_account_username.as_deref()
            == Some(account.data.user_info.username.as_str())
}

fn account_select_row(
    instance: &InstanceView,
    account: AccountView,
    sender: BackendSender,
    override_account: bool,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let selected = account_is_selected(instance, &account, override_account);
    let instance_id = instance.id;
    let key = account.key.clone();
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Button::new(format!(
                "{}-account-{}-{}",
                if override_account {
                    "override"
                } else {
                    "select"
                },
                account.key.0,
                account.key.1
            ))
            .label(if selected { "Selected" } else { "Use" })
            .disabled(selected)
            .on_click(move |_, _, _| {
                if override_account {
                    sender.send(MessageToBackend::SetInstanceAccountOverride {
                        instance: instance_id,
                        account: Some(key.clone()),
                    });
                } else {
                    sender.send(MessageToBackend::SetInstanceSelectedAccount {
                        instance: instance_id,
                        account: Some(key.clone()),
                    });
                }
            }),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().line_clamp(1).child(account.data.user_info.username))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(provider_label(&account.provider)),
                ),
        )
}

fn runtime_section(
    instance: &InstanceView,
    memory_input: gpui::Entity<InputState>,
    jvm_flags_input: gpui::Entity<InputState>,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let id = instance.id;
    v_flex()
        .gap_3()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "Effective memory: {} MB",
                    instance.effective_xmx_mb.unwrap_or(4096)
                )),
        )
        .child(
            h_flex()
                .gap_2()
                .child(Input::new(&memory_input))
                .child(
                    Button::new(format!("save-memory-{id}"))
                        .label("Set Memory")
                        .on_click({
                            let input = memory_input.clone();
                            let sender = sender.clone();
                            move |_, _, cx| {
                                let value = input.read(cx).value().trim().parse::<u64>().ok();
                                sender.send(MessageToBackend::SetInstanceMemory {
                                    instance: id,
                                    xmx_mb: value,
                                });
                            }
                        }),
                )
                .child(
                    Button::new(format!("clear-memory-{id}"))
                        .label("Default")
                        .on_click({
                            let sender = sender.clone();
                            move |_, _, _| {
                                sender.send(MessageToBackend::SetInstanceMemory {
                                    instance: id,
                                    xmx_mb: None,
                                });
                            }
                        }),
                ),
        )
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "JVM flags: {}",
                            instance.jvm_flags.as_deref().unwrap_or("default")
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Input::new(&jvm_flags_input))
                        .child(
                            Button::new(format!("save-jvm-flags-{id}"))
                                .label("Set Flags")
                                .on_click({
                                    let input = jvm_flags_input.clone();
                                    let sender = sender.clone();
                                    move |_, _, cx| {
                                        let value = input.read(cx).value().to_string();
                                        sender.send(MessageToBackend::SetInstanceJvmFlags {
                                            instance: id,
                                            flags: Some(value),
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new(format!("clear-jvm-flags-{id}"))
                                .label("Default")
                                .on_click({
                                    let sender = sender.clone();
                                    move |_, _, _| {
                                        sender.send(MessageToBackend::SetInstanceJvmFlags {
                                            instance: id,
                                            flags: None,
                                        });
                                    }
                                }),
                        ),
                ),
        )
}

fn launcher_settings_section(
    settings: LauncherSettingsView,
    launcher_dir: PathBuf,
    notifications: gpui::Entity<NotificationEntries>,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().font_semibold().child("Hide launcher after launch"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Minimize the launcher window when Minecraft starts."),
                        ),
                )
                .child(
                    Button::new("toggle-hide-after-launch")
                        .label(if settings.hide_window_after_launch {
                            "On"
                        } else {
                            "Off"
                        })
                        .on_click({
                            let sender = sender.clone();
                            move |_, _, _| {
                                sender.send(MessageToBackend::SetLauncherSettings(
                                    LauncherSettingsView {
                                        hide_window_after_launch: !settings
                                            .hide_window_after_launch,
                                        hide_usernames_in_cards: settings.hide_usernames_in_cards,
                                    },
                                ));
                            }
                        }),
                ),
        )
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().font_semibold().child("Hide usernames in cards"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Hide the account line on instance cards."),
                        ),
                )
                .child(
                    Button::new("toggle-hide-usernames")
                        .label(if settings.hide_usernames_in_cards {
                            "On"
                        } else {
                            "Off"
                        })
                        .on_click(move |_, _, _| {
                            sender.send(MessageToBackend::SetLauncherSettings(
                                LauncherSettingsView {
                                    hide_window_after_launch: settings.hide_window_after_launch,
                                    hide_usernames_in_cards: !settings.hide_usernames_in_cards,
                                },
                            ));
                        }),
                ),
        )
        .child(
            Button::new("open-launcher-directory")
                .label("Open Launcher Directory")
                .on_click(move |_, _, cx| {
                    if let Err(err) = open_path(&launcher_dir) {
                        notifications.update(cx, |entries, cx| {
                            entries.push(
                                NotificationLevel::Error,
                                format!("Failed to open launcher directory: {err}"),
                                cx,
                            );
                        });
                    }
                }),
        )
}

fn accounts_section(
    accounts: Vec<AccountView>,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    if accounts.is_empty() {
        return v_flex().gap_1().child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No accounts have been added yet."),
        );
    }

    v_flex()
        .gap_2()
        .children(accounts.into_iter().map(|account| {
            let remove_key = account.key.clone();
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().line_clamp(1).child(account.data.user_info.username))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(provider_label(&account.provider)),
                        ),
                )
                .child(
                    Button::new(format!(
                        "settings-remove-account-{}-{}",
                        remove_key.0, remove_key.1
                    ))
                    .label("Remove")
                    .on_click({
                        let sender = sender.clone();
                        move |_, _, _| {
                            sender.send(MessageToBackend::RemoveAccount(remove_key.clone()))
                        }
                    }),
                )
        }))
}

fn add_account_section(
    offline_nickname: gpui::Entity<InputState>,
    telegram_base_url: gpui::Entity<InputState>,
    elyby_client_id: gpui::Entity<InputState>,
    elyby_client_secret: gpui::Entity<InputState>,
    elyby_launcher_name: gpui::Entity<InputState>,
    sender: BackendSender,
    _cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    v_flex()
        .gap_3()
        .child(add_provider_button(
            AuthProviderConfig::Microsoft(MicrosoftAuthProvider {}),
            "Add Microsoft",
            sender.clone(),
        ))
        .child(
            h_flex().gap_2().child(Input::new(&offline_nickname)).child(
                Button::new("settings-add-offline")
                    .label("Add Offline")
                    .on_click({
                        let input = offline_nickname.clone();
                        let sender = sender.clone();
                        move |_, window, cx| {
                            let value = input.read(cx).value().to_string();
                            sender.send(MessageToBackend::SubmitOfflineNickname(value));
                            input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    }),
            ),
        )
        .child(
            h_flex()
                .gap_2()
                .child(Input::new(&telegram_base_url))
                .child(
                    Button::new("settings-add-telegram")
                        .label("Add Telegram")
                        .on_click({
                            let input = telegram_base_url.clone();
                            let sender = sender.clone();
                            move |_, _, cx| {
                                let auth_base_url = input.read(cx).value().trim().to_string();
                                if !auth_base_url.is_empty() {
                                    sender.send(MessageToBackend::StartAddAccount(
                                        AuthProviderConfig::Telegram(TGAuthProvider {
                                            auth_base_url,
                                        }),
                                    ));
                                }
                            }
                        }),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(Input::new(&elyby_client_id))
                .child(Input::new(&elyby_client_secret))
                .child(Input::new(&elyby_launcher_name))
                .child(
                    Button::new("settings-add-elyby")
                        .label("Add Ely.by")
                        .on_click({
                            let client_id_input = elyby_client_id.clone();
                            let client_secret_input = elyby_client_secret.clone();
                            let launcher_name_input = elyby_launcher_name.clone();
                            let sender = sender.clone();
                            move |_, _, cx| {
                                let client_id = client_id_input.read(cx).value().trim().to_string();
                                let client_secret =
                                    client_secret_input.read(cx).value().trim().to_string();
                                let launcher_name =
                                    launcher_name_input.read(cx).value().trim().to_string();
                                if !client_id.is_empty() && !client_secret.is_empty() {
                                    sender.send(MessageToBackend::StartAddAccount(
                                        AuthProviderConfig::ElyBy(ElyByAuthProvider::new(
                                            client_id,
                                            client_secret,
                                            if launcher_name.is_empty() {
                                                "Potato Launcher".to_string()
                                            } else {
                                                launcher_name
                                            },
                                        )),
                                    ));
                                }
                            }
                        }),
                ),
        )
}

fn add_provider_button(
    provider: AuthProviderConfig,
    label: &'static str,
    sender: BackendSender,
) -> Button {
    Button::new(format!("add-provider-{}", provider_label(&provider)))
        .label(label)
        .on_click(move |_, _, _| {
            sender.send(MessageToBackend::StartAddAccount(provider.clone()));
        })
}

fn backend_list(
    backends: Vec<BackendStatus>,
    backend_names: BTreeMap<String, String>,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    if backends.is_empty() {
        return v_flex().gap_1().child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No backends configured yet."),
        );
    }

    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .children(backends.into_iter().map(|backend| {
            let title = backend_names
                .get(backend.url.as_str())
                .cloned()
                .unwrap_or_else(|| backend_display_name(&backend.url, false));
            let url = backend.url.clone();
            let url_label = url.as_str().to_string();
            let status_color = match &backend.fetch_state {
                BackendFetchState::Offline | BackendFetchState::Error(_) => cx.theme().red,
                BackendFetchState::Fetching => cx.theme().yellow,
                BackendFetchState::Fetched { .. } => cx.theme().foreground,
                BackendFetchState::NotFetched => cx.theme().muted_foreground,
            };
            let origin_note = if backend.configured {
                "Configured".to_string()
            } else if backend.referenced_by_instances {
                "Used by installed instances".to_string()
            } else {
                "Discovered".to_string()
            };

            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .p_3()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(
                    if matches!(
                        &backend.fetch_state,
                        BackendFetchState::Offline | BackendFetchState::Error(_)
                    ) {
                        cx.theme().red
                    } else {
                        cx.theme().border
                    },
                )
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .justify_between()
                        .items_start()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_semibold()
                                .line_clamp(1)
                                .child(title),
                        )
                        .child(
                            Button::new(format!("backend-panel-remove-{}", url.as_str()))
                                .label("Remove")
                                .disabled(!backend.configured)
                                .on_click({
                                    let sender = sender.clone();
                                    move |_, _, _| {
                                        sender
                                            .send(MessageToBackend::RemoveBackendUrl(url.clone()));
                                    }
                                }),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(url_label),
                )
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_sm()
                                .text_color(status_color)
                                .child(fetch_state_label(&backend.fetch_state)),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .line_clamp(1)
                                .child(origin_note),
                        ),
                )
        }))
}

fn action_section(
    instance: InstanceView,
    pending_delete: bool,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let id = instance.id;
    let can_launch = matches!(
        instance.status,
        InstanceLiveStatus::Installed
            | InstanceLiveStatus::OrphanedFromBackend
            | InstanceLiveStatus::LaunchFailed(_)
    );
    let launch_blocked = instance.launch_blocked_reason.is_some();
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(format!("detail-play-{id}"))
                        .label("Play")
                        .disabled(!can_launch || launch_blocked)
                        .on_click({
                            let sender = sender.clone();
                            move |_, _, _| {
                                sender.send(MessageToBackend::Launch {
                                    instance: id,
                                    account: None,
                                });
                            }
                        }),
                )
                .child(
                    Button::new(format!("detail-kill-{id}"))
                        .label("Kill")
                        .disabled(!matches!(instance.status, InstanceLiveStatus::Running))
                        .on_click({
                            let sender = sender.clone();
                            move |_, _, _| sender.send(MessageToBackend::KillInstance(id))
                        }),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(format!("detail-resync-{id}"))
                        .label("Resync")
                        .disabled(matches!(
                            instance.status,
                            InstanceLiveStatus::Installing { .. }
                        ))
                        .on_click({
                            let sender = sender.clone();
                            move |_, _, _| {
                                sender.send(MessageToBackend::InstallInstance {
                                    id,
                                    force_overwrite: false,
                                });
                            }
                        }),
                )
                .child(
                    Button::new(format!("detail-hard-resync-{id}"))
                        .label("Hard Resync")
                        .disabled(matches!(
                            instance.status,
                            InstanceLiveStatus::Installing { .. }
                        ))
                        .on_click({
                            let sender = sender.clone();
                            move |_, _, _| {
                                sender.send(MessageToBackend::InstallInstance {
                                    id,
                                    force_overwrite: true,
                                });
                            }
                        }),
                ),
        )
        .child(
            h_flex().gap_2().child(
                Button::new(format!("detail-delete-{id}"))
                    .label(if pending_delete {
                        "Confirm Delete"
                    } else {
                        "Delete"
                    })
                    .disabled(!instance.locally_installed)
                    .on_click({
                        let sender = sender.clone();
                        cx.listener(move |page, _, _, cx| {
                            if page.pending_delete == Some(id) {
                                page.pending_delete = None;
                                page.selected_instance = None;
                                sender.send(MessageToBackend::DeleteInstance(id));
                            } else {
                                page.pending_delete = Some(id);
                            }
                            cx.notify();
                        })
                    }),
            ),
        )
        .when(pending_delete, |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().red)
                    .child("Click Confirm Delete to remove this instance from disk."),
            )
        })
        .when_some(instance.launch_blocked_reason.clone(), |this, reason| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().yellow)
                    .child(reason.to_string()),
            )
        })
        .when(
            matches!(instance.status, InstanceLiveStatus::Outdated),
            |this| {
                this.child(
                    Button::new(format!("detail-update-{id}"))
                        .label("Update")
                        .on_click(move |_, _, _| {
                            sender.send(MessageToBackend::InstallInstance {
                                id,
                                force_overwrite: false,
                            });
                        }),
                )
            },
        )
        .when_some(instance.default_xmx_mb, |this, xmx| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("Recommended memory: {xmx} MB")),
            )
        })
}

fn status_error(status: &InstanceLiveStatus) -> Option<String> {
    match status {
        InstanceLiveStatus::InstallFailed(error) | InstanceLiveStatus::LaunchFailed(error) => {
            Some(error.to_string())
        }
        _ => None,
    }
}

fn provider_label(provider: &AuthProviderConfig) -> String {
    match provider {
        AuthProviderConfig::Microsoft(_) => "Microsoft".to_string(),
        AuthProviderConfig::Telegram(provider) => format!("Telegram ({})", provider.auth_base_url),
        AuthProviderConfig::ElyBy(_) => "Ely.by".to_string(),
        AuthProviderConfig::Offline(_) => "Offline".to_string(),
    }
}

fn provider_type_label(provider: &AuthProviderConfig) -> String {
    match provider {
        AuthProviderConfig::Microsoft(_) => "Microsoft".to_string(),
        AuthProviderConfig::Telegram(_) => "Telegram".to_string(),
        AuthProviderConfig::ElyBy(_) => "Ely.by".to_string(),
        AuthProviderConfig::Offline(_) => "Offline".to_string(),
    }
}

fn open_path(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]).arg(path);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };

    command.spawn().map(|_| ())
}

fn backend_display_names(backends: &[BackendStatus]) -> BTreeMap<String, String> {
    let mut host_counts = BTreeMap::<String, usize>::new();
    for backend in backends {
        if let Some(host) = backend.url.host_str() {
            *host_counts.entry(host.to_string()).or_default() += 1;
        }
    }

    backends
        .iter()
        .map(|backend| {
            let duplicate_host = backend
                .url
                .host_str()
                .and_then(|host| host_counts.get(host))
                .is_some_and(|count| *count > 1);
            (
                backend.url.as_str().to_string(),
                backend_display_name(&backend.url, duplicate_host),
            )
        })
        .collect()
}

fn backend_display_name(url: &Url, include_path: bool) -> String {
    let Some(host) = url.host_str() else {
        return url.as_str().to_string();
    };
    if !include_path {
        return host.to_string();
    }

    let mut path = url.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        path = "/".to_string();
    }
    format!("{host}{path}")
}

fn fetch_state_label(fetch_state: &BackendFetchState) -> String {
    match fetch_state {
        BackendFetchState::NotFetched => "not fetched".to_string(),
        BackendFetchState::Fetching => "Refreshing...".to_string(),
        BackendFetchState::Fetched { instance_count } => format!("{instance_count} published"),
        BackendFetchState::Offline => "offline".to_string(),
        BackendFetchState::Error(error) => error.to_string(),
    }
}

fn status_label(instance: &InstanceView) -> String {
    let label = match &instance.status {
        InstanceLiveStatus::NotInstalled => "Available".to_string(),
        InstanceLiveStatus::Installed => "Installed".to_string(),
        InstanceLiveStatus::Outdated => "Outdated".to_string(),
        InstanceLiveStatus::Installing {
            current,
            total,
            message,
            show_bar,
            ..
        } => {
            if !*show_bar || *total <= 1 {
                message.to_string()
            } else {
                format!("{message} {}%", current.saturating_mul(100) / total)
            }
        }
        InstanceLiveStatus::InstallFailed(_) => "Failed".to_string(),
        InstanceLiveStatus::Launching => "Launching".to_string(),
        InstanceLiveStatus::Running => "Running".to_string(),
        InstanceLiveStatus::LaunchFailed(_) => "Launch failed".to_string(),
        InstanceLiveStatus::OrphanedFromBackend => "Orphaned".to_string(),
    };
    if instance.launch_blocked_reason.is_some()
        && matches!(
            instance.status,
            InstanceLiveStatus::Installed
                | InstanceLiveStatus::Outdated
                | InstanceLiveStatus::OrphanedFromBackend
                | InstanceLiveStatus::LaunchFailed(_)
        )
    {
        format!("{label}, no account")
    } else {
        label
    }
}

fn progress_ratio(status: &InstanceLiveStatus) -> Option<f32> {
    let InstanceLiveStatus::Installing {
        current,
        total,
        show_bar,
        ..
    } = status
    else {
        return None;
    };
    if !*show_bar || *total <= 1 {
        None
    } else {
        Some((*current as f32 / *total as f32).clamp(0.0, 1.0))
    }
}

#[allow(dead_code)]
fn _url_key(url: &Url) -> String {
    url.as_str().to_string()
}
