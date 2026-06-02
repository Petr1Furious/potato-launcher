use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    process::Command,
    sync::Arc,
};

use gpui::{
    App, Context, IntoElement, Render, SharedString, Window, div, prelude::*, px, relative, rems,
};
use gpui_component::{
    ActiveTheme, Disableable, IndexPath, StyledExt,
    button::Button,
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement,
    select::{Select, SelectDelegate, SelectEvent, SelectItem, SelectState},
    skeleton::Skeleton,
    v_flex,
};
use instance::storage::sanitize_dir_name;
use launcher_auth::providers::{
    AuthProviderConfig, ElyByAuthProvider, MicrosoftAuthProvider, TGAuthProvider,
};
use launcher_bridge::{
    AccountView, BackendFetchState, BackendSender, BackendStatus, InstanceLiveStatus,
    InstanceOrigin, InstanceView, LauncherSettingsView, LocalLoader, MessageToBackend,
    NotificationLevel,
};
#[cfg(target_os = "linux")]
use launcher_build_config::use_native_glfw_default;
use launcher_i18n as t;
use url::Url;
use uuid::Uuid;

use crate::entity::{
    DataEntities,
    account::AccountsUpdatedEvent,
    backend::BackendsUpdatedEvent,
    instance::InstancesUpdatedEvent,
    java_resolve::{JavaResolveCache, JavaResolvedEvent},
    local_create::{LoaderVersionsUpdatedEvent, LocalCreateVersionsUpdatedEvent},
    notification::NotificationEntries,
    settings::LauncherSettingsUpdatedEvent,
};

pub struct InstancesPage {
    data: DataEntities,
    selected_instance: Option<Uuid>,
    show_global_settings: bool,
    show_backend_settings: bool,
    show_accounts_panel: bool,
    show_create_local_modal: bool,
    create_local_loader: LocalLoader,
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
    create_local_name_input: gpui::Entity<InputState>,
    create_local_mc_version_select: gpui::Entity<SelectState<VersionList>>,
    create_local_loader_version_select: gpui::Entity<SelectState<VersionList>>,
    create_local_show_snapshots: bool,
    create_local_sync_mc_dropdown: bool,
    create_local_sync_loader_dropdown: bool,
    java_path_input: gpui::Entity<InputState>,
    java_path_last_instance: Option<Uuid>,
    java_path_last_stored: Option<Arc<str>>,
    _instances_subscription: gpui::Subscription,
    _backends_subscription: gpui::Subscription,
    _accounts_subscription: gpui::Subscription,
    _settings_subscription: gpui::Subscription,
    _local_create_subscription: gpui::Subscription,
    _local_loader_subscription: gpui::Subscription,
    _mc_version_selected_subscription: gpui::Subscription,
    _java_resolve_subscription: gpui::Subscription,
}

impl InstancesPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _backends_subscription = cx
            .subscribe(&data.backends, |_, _, _: &BackendsUpdatedEvent, cx| {
                cx.notify()
            });
        let _accounts_subscription = cx
            .subscribe(&data.accounts, |_, _, _: &AccountsUpdatedEvent, cx| {
                cx.notify()
            });
        let _settings_subscription = cx.subscribe(
            &data.settings,
            |_, _, _: &LauncherSettingsUpdatedEvent, cx| cx.notify(),
        );
        let backend_url_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::placeholders::manifest_url()));
        let offline_nickname_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(t::placeholders::offline_nickname()));
        let telegram_base_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t::placeholders::telegram_auth_base_url())
        });
        let elyby_client_id_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::placeholders::client_id()));
        let elyby_client_secret_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::placeholders::client_secret()));
        let elyby_launcher_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::placeholders::launcher_name()));
        let memory_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::placeholders::memory_mib()));
        let jvm_flags_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::placeholders::jvm_flags()));
        let java_path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::instances::java_path_auto()));
        let java_path_input_for_instances = java_path_input.clone();
        let _instances_subscription = cx.subscribe_in(
            &data.instances,
            window,
            move |page, _, _: &InstancesUpdatedEvent, window, cx| {
                let current = page.selected_instance;
                let stored = current.and_then(|id| {
                    page.data
                        .instances
                        .read(cx)
                        .entries
                        .iter()
                        .find(|v| v.id == id)
                        .and_then(|v| v.java_path.clone())
                });
                if current != page.java_path_last_instance || stored != page.java_path_last_stored {
                    page.java_path_last_instance = current;
                    page.java_path_last_stored = stored.clone();
                    let value = stored.as_deref().unwrap_or_default().to_owned();
                    java_path_input_for_instances
                        .update(cx, |state, cx| state.set_value(value, window, cx));
                }
                cx.notify();
            },
        );
        let create_local_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::placeholders::instance_name()));
        let create_local_mc_version_select = cx
            .new(|cx| SelectState::new(VersionList::default(), None, window, cx).searchable(true));
        let create_local_loader_version_select = cx
            .new(|cx| SelectState::new(VersionList::default(), None, window, cx).searchable(true));
        let _mc_version_selected_subscription =
            cx.subscribe(&create_local_mc_version_select, |page, _, event, cx| {
                page.on_minecraft_version_selected(event, cx);
            });
        let _local_create_subscription = cx.subscribe(
            &data.local_create,
            |page, _, _: &LocalCreateVersionsUpdatedEvent, cx| {
                page.create_local_sync_mc_dropdown = true;
                cx.notify();
            },
        );
        let _local_loader_subscription = cx.subscribe(
            &data.local_create,
            |page, _, _: &LoaderVersionsUpdatedEvent, cx| {
                page.create_local_sync_loader_dropdown = true;
                cx.notify();
            },
        );
        let java_path_input_for_sub = java_path_input.clone();
        let _java_resolve_subscription = cx.subscribe_in(
            &data.java_resolve,
            window,
            move |page, _, event: &JavaResolvedEvent, window, cx| {
                if page.selected_instance != Some(event.0) {
                    return;
                }
                use crate::entity::java_resolve::JavaResolveState;
                let value = match page.data.java_resolve.read(cx).state(event.0) {
                    Some(JavaResolveState::Found(p)) => p.to_string(),
                    _ => String::new(),
                };
                java_path_input_for_sub.update(cx, |state, cx| state.set_value(value, window, cx));
            },
        );

        Self {
            data: data.clone(),
            selected_instance: None,
            show_global_settings: false,
            show_backend_settings: false,
            show_accounts_panel: false,
            show_create_local_modal: false,
            create_local_loader: LocalLoader::Vanilla,
            create_local_show_snapshots: false,
            create_local_sync_mc_dropdown: false,
            create_local_sync_loader_dropdown: false,
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
            java_path_input,
            java_path_last_instance: None,
            java_path_last_stored: None,
            create_local_name_input,
            create_local_mc_version_select,
            create_local_loader_version_select,
            _instances_subscription,
            _backends_subscription,
            _accounts_subscription,
            _settings_subscription,
            _local_create_subscription,
            _local_loader_subscription,
            _mc_version_selected_subscription,
            _java_resolve_subscription,
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
        let launcher_settings = self.data.settings.read(cx).settings.clone();
        if launcher_settings.hide_window_after_launch
            && let Some(instance) = instances
                .iter()
                .find(|instance| matches!(instance.status, InstanceLiveStatus::Launching))
            && self.hidden_launches.insert(instance.id)
        {
            window.minimize_window();
        }
        let finished_hidden: Vec<Uuid> = self
            .hidden_launches
            .iter()
            .copied()
            .filter(|id| {
                !instances.iter().any(|instance| {
                    instance.id == *id
                        && matches!(
                            instance.status,
                            InstanceLiveStatus::Launching | InstanceLiveStatus::Running
                        )
                })
            })
            .collect();
        self.hidden_launches.retain(|id| {
            instances.iter().any(|instance| {
                instance.id == *id
                    && matches!(
                        instance.status,
                        InstanceLiveStatus::Launching | InstanceLiveStatus::Running
                    )
            })
        });
        if !finished_hidden.is_empty() {
            window.activate_window();
        }
        let groups = group_instances(&instances);
        let backend_names = backend_display_names(&backends);
        if self
            .selected_instance
            .is_some_and(|id| !instances.iter().any(|instance| instance.id == id))
        {
            self.selected_instance = None;
        }

        let mut sections = Vec::new();

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
                SectionParams {
                    title: backend_names
                        .get(backend.url.as_str())
                        .cloned()
                        .unwrap_or_else(|| backend_display_name(&backend.url, false)),
                    backend: Some(backend),
                    instances,
                    empty_hint: None,
                    header_action: None,
                    hide_usernames: launcher_settings.hide_usernames_in_cards,
                    sender: self.data.backend_sender.clone(),
                },
                cx,
            ));
        }

        let local_instances = groups.local.unwrap_or_default();
        let add_local = Button::new("add-local-instance")
            .label(t::local::add())
            .on_click(cx.listener(|page, _, _, cx| {
                page.open_create_local_modal(cx);
            }));
        sections.push(section(
            SectionParams {
                title: t::common::local().to_string(),
                backend: None,
                instances: local_instances,
                empty_hint: Some(t::instances::local_empty_hint().to_string()),
                header_action: Some(add_local),
                hide_usernames: launcher_settings.hide_usernames_in_cards,
                sender: self.data.backend_sender.clone(),
            },
            cx,
        ));

        let separated_sections = separated_backend_sections(sections, cx);
        let scrollable_list = v_flex()
            .size_full()
            .p_4()
            .overflow_y_scrollbar()
            .children(separated_sections);
        let list = div()
            .size_full()
            .relative()
            .child(scrollable_list)
            .when(self.show_create_local_modal, |this| {
                this.child(self.create_local_modal(window, cx))
            });
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

impl InstancesPage {
    fn open_create_local_modal(&mut self, cx: &mut Context<Self>) {
        self.show_create_local_modal = true;
        self.create_local_show_snapshots = false;
        self.data
            .backend_sender
            .send(MessageToBackend::FetchLocalCreateVersions);
        self.data.local_create.update(cx, |entries, cx| {
            entries.set_minecraft_loading(cx);
        });
        cx.notify();
    }

    fn on_minecraft_version_selected(
        &mut self,
        event: &SelectEvent<VersionList>,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(value) = event;
        let Some(value) = value else {
            return;
        };
        self.request_loader_versions(value.as_str(), cx);
    }

    fn request_loader_versions(&mut self, minecraft_version: &str, cx: &mut Context<Self>) {
        if matches!(self.create_local_loader, LocalLoader::Vanilla) {
            return;
        }

        self.data
            .backend_sender
            .send(MessageToBackend::FetchLoaderVersions {
                minecraft_version: minecraft_version.to_string(),
                loader: self.create_local_loader,
            });
        self.data.local_create.update(cx, |entries, cx| {
            entries.set_loader_loading(minecraft_version.to_string(), self.create_local_loader, cx);
        });
    }

    fn filtered_minecraft_versions(&self, cx: &App) -> Vec<SharedString> {
        let state = self.data.local_create.read(cx).state.clone();
        state
            .minecraft_versions
            .iter()
            .filter(|(_, version_type)| {
                self.create_local_show_snapshots || version_type != "snapshot"
            })
            .map(|(id, _)| SharedString::from(id.as_str()))
            .collect()
    }

    fn update_minecraft_version_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.data.local_create.read(cx).state.clone();
        let versions = self.filtered_minecraft_versions(cx);
        let latest_release = SharedString::from(state.latest_release.as_str());

        self.create_local_mc_version_select
            .update(cx, |dropdown, cx| {
                let mut to_select = dropdown.selected_value().cloned();
                if to_select
                    .as_ref()
                    .is_none_or(|selected| !versions.contains(selected))
                {
                    to_select = None;
                }
                if to_select.is_none() && versions.contains(&latest_release) {
                    to_select = Some(latest_release.clone());
                }
                if to_select.is_none() {
                    to_select = versions.first().cloned();
                }

                dropdown.set_items(
                    VersionList {
                        versions: versions.clone(),
                        matched_versions: versions.clone(),
                    },
                    window,
                    cx,
                );

                if let Some(to_select) = to_select {
                    dropdown.set_selected_value(&to_select, window, cx);
                }
            });

        if let Some(selected) = self
            .create_local_mc_version_select
            .read(cx)
            .selected_value()
            .cloned()
        {
            let needs_fetch = !matches!(self.create_local_loader, LocalLoader::Vanilla)
                && (state.loader_minecraft_version.as_deref() != Some(selected.as_str())
                    || state.loader_kind != Some(self.create_local_loader)
                    || (!state.loader_loading
                        && state.loader_versions.is_empty()
                        && state.loader_error.is_none()));
            if needs_fetch {
                self.request_loader_versions(selected.as_str(), cx);
            }
        }
    }

    fn update_loader_version_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.create_local_loader, LocalLoader::Vanilla) {
            return;
        }

        let state = self.data.local_create.read(cx).state.clone();
        let versions: Vec<SharedString> = state
            .loader_versions
            .iter()
            .map(|version| SharedString::from(version.as_str()))
            .collect();

        self.create_local_loader_version_select
            .update(cx, |dropdown, cx| {
                let mut to_select = dropdown.selected_value().cloned();
                if to_select
                    .as_ref()
                    .is_none_or(|selected| !versions.contains(selected))
                {
                    to_select = versions.first().cloned();
                }

                dropdown.set_items(
                    VersionList {
                        versions: versions.clone(),
                        matched_versions: versions.clone(),
                    },
                    window,
                    cx,
                );

                if let Some(to_select) = to_select {
                    dropdown.set_selected_value(&to_select, window, cx);
                }
            });
    }

    fn create_local_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        if self.create_local_sync_mc_dropdown {
            self.create_local_sync_mc_dropdown = false;
            self.update_minecraft_version_dropdown(window, cx);
        }
        if self.create_local_sync_loader_dropdown {
            self.create_local_sync_loader_dropdown = false;
            self.update_loader_version_dropdown(window, cx);
        }

        let sender = self.data.backend_sender.clone();
        let name_input = self.create_local_name_input.clone();
        let mc_select = self.create_local_mc_version_select.clone();
        let loader_version_select = self.create_local_loader_version_select.clone();
        let selected_loader = self.create_local_loader;
        let local_create_state = self.data.local_create.read(cx).state.clone();
        let loader_version_disabled = matches!(selected_loader, LocalLoader::Vanilla);
        let minecraft_loading = local_create_state.minecraft_loading;
        let minecraft_has_error = local_create_state.minecraft_error.is_some();
        let minecraft_error = local_create_state.minecraft_error.clone();
        let loader_loading = local_create_state.loader_loading;
        let loader_error = local_create_state.loader_error.clone();
        let show_snapshots = self.create_local_show_snapshots;
        let instances = self.data.instances.read(cx).entries.clone();
        let form_issue = create_local_form_issue(self, &instances, cx);
        let form_valid = form_issue.is_none();

        let version_section = if let Some(error) = minecraft_error {
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error.to_string()),
                )
                .child(
                    Button::new("reload-local-create-versions")
                        .label(t::local::versions_reload())
                        .on_click({
                            let sender = sender.clone();
                            cx.listener(move |page, _, _, cx| {
                                sender.send(MessageToBackend::FetchLocalCreateVersions);
                                page.data.local_create.update(cx, |entries, cx| {
                                    entries.set_minecraft_loading(cx);
                                });
                            })
                        }),
                )
                .into_any_element()
        } else if minecraft_loading {
            Skeleton::new()
                .w_full()
                .min_h_8()
                .max_h_8()
                .rounded_md()
                .into_any_element()
        } else {
            v_flex()
                .gap_2()
                .child(Select::new(&mc_select).w_full().menu_max_h(rems(16.)))
                .child(
                    Checkbox::new("show-local-create-snapshots")
                        .checked(show_snapshots)
                        .label(t::local::show_snapshots())
                        .on_click(cx.listener(|page, checked, window, cx| {
                            page.create_local_show_snapshots = *checked;
                            page.update_minecraft_version_dropdown(window, cx);
                            cx.notify();
                        })),
                )
                .into_any_element()
        };

        let loader_version_section = if loader_version_disabled {
            div().into_any_element()
        } else if let Some(error) = loader_error {
            div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error.to_string())
                .into_any_element()
        } else if loader_loading {
            Skeleton::new()
                .w_full()
                .min_h_8()
                .max_h_8()
                .rounded_md()
                .into_any_element()
        } else {
            Select::new(&loader_version_select)
                .w_full()
                .menu_max_h(rems(16.))
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background.opacity(0.88))
            .child(
                v_flex()
                    .w(px(420.0))
                    .gap_3()
                    .p_4()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .child(t::local::create_title()),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().child(t::local::instance_name()))
                            .child(Input::new(&name_input)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().child(t::local::minecraft_version()))
                            .child(version_section),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().child(t::local::loader()))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(local_loader_button(
                                        LocalLoader::Vanilla,
                                        selected_loader,
                                        cx,
                                    ))
                                    .child(local_loader_button(
                                        LocalLoader::Fabric,
                                        selected_loader,
                                        cx,
                                    ))
                                    .child(local_loader_button(
                                        LocalLoader::Forge,
                                        selected_loader,
                                        cx,
                                    ))
                                    .child(local_loader_button(
                                        LocalLoader::Neoforge,
                                        selected_loader,
                                        cx,
                                    )),
                            ),
                    )
                    .when(!loader_version_disabled, |this| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .child(div().text_sm().child(t::local::loader_version()))
                                .child(loader_version_section),
                        )
                    })
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("cancel-create-local")
                                    .label(t::common::cancel())
                                    .on_click(cx.listener(|page, _, _, cx| {
                                        page.show_create_local_modal = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("submit-create-local")
                                    .label(t::local::create())
                                    .disabled(
                                        !form_valid || minecraft_loading || minecraft_has_error,
                                    )
                                    .on_click({
                                        let sender = sender.clone();
                                        cx.listener(move |page, _, _, cx| {
                                            let instances =
                                                page.data.instances.read(cx).entries.clone();
                                            if create_local_form_issue(page, &instances, cx)
                                                .is_some()
                                            {
                                                return;
                                            }
                                            let display_name =
                                                page.create_local_name_input.read(cx).value();
                                            let Some(minecraft_version) = page
                                                .create_local_mc_version_select
                                                .read(cx)
                                                .selected_value()
                                                .cloned()
                                            else {
                                                return;
                                            };
                                            let loader_version = page
                                                .create_local_loader_version_select
                                                .read(cx)
                                                .selected_value()
                                                .cloned();
                                            sender.send(MessageToBackend::CreateLocalInstance {
                                                display_name: display_name.trim().to_string(),
                                                minecraft_version: minecraft_version.to_string(),
                                                loader: page.create_local_loader,
                                                loader_version: loader_version
                                                    .map(|version| version.to_string()),
                                            });
                                            page.show_create_local_modal = false;
                                            cx.notify();
                                        })
                                    }),
                            ),
                    )
                    .when_some(form_issue, |this, hint| {
                        this.child(div().text_sm().text_color(cx.theme().danger).child(hint))
                    }),
            )
    }

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
                            .label(t::common::close())
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.selected_instance = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(detail_section(
                t::instances::status(),
                v_flex()
                    .gap_2()
                    .child(div().child(status_label(&instance)))
                    .when_some(status_error(&instance.status), |this, error| {
                        this.child(error_alert(t::common::details(), error, cx))
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
                t::instances::runtime(),
                runtime_section(
                    &instance,
                    self.memory_input.clone(),
                    self.jvm_flags_input.clone(),
                    self.java_path_input.clone(),
                    self.data.java_resolve.clone(),
                    sender.clone(),
                    cx,
                ),
                cx,
            ))
            .child(detail_section(
                t::instances::actions(),
                action_section(
                    instance.clone(),
                    self.pending_delete == Some(id),
                    sender.clone(),
                    cx,
                ),
                cx,
            ))
            .child(detail_section(
                t::instances::logs(),
                v_flex()
                    .gap_2()
                    .child(
                        Button::new(format!("open-instance-folder-{id}"))
                            .label(t::instances::open_instance_folder())
                            .disabled(matches!(instance.status, InstanceLiveStatus::NotInstalled))
                            .on_click({
                                let notifications = self.data.notifications.clone();
                                move |_, _, cx| {
                                    if let Err(err) = open_path(&instance_path) {
                                        notifications.update(cx, |entries, cx| {
                                            entries.push(
                                                NotificationLevel::Error,
                                                t::notifications::failed_open_instance_folder(
                                                    err.to_string(),
                                                ),
                                                cx,
                                            );
                                        });
                                    }
                                }
                            }),
                    )
                    .child(
                        Button::new(format!("open-logs-{id}"))
                            .label(t::instances::open_latest_launch_log())
                            .on_click({
                                let notifications = self.data.notifications.clone();
                                move |_, _, cx| {
                                    if let Err(err) = open_path(&log_path) {
                                        notifications.update(cx, |entries, cx| {
                                            entries.push(
                                                NotificationLevel::Error,
                                                t::notifications::failed_open_logs(err.to_string()),
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
                    .child(div().text_xl().font_semibold().child(t::settings::title()))
                    .child(
                        Button::new("close-global-settings")
                            .label(t::common::close())
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
                    .child(div().text_xl().font_semibold().child(t::accounts::title()))
                    .child(
                        Button::new("close-accounts")
                            .label(t::common::close())
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.show_accounts_panel = false;
                                page.preferred_add_provider = None;
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.preferred_add_provider.as_ref(), |this, provider| {
                this.child(detail_section(
                    t::accounts::suggested_account(),
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(t::accounts::suggested_account_needs(provider_label(
                                    provider,
                                ))),
                        )
                        .child(add_provider_button(
                            provider.clone(),
                            t::accounts::add_suggested_account(),
                            sender.clone(),
                        )),
                    cx,
                ))
            })
            .child(detail_section(
                t::accounts::section(),
                accounts_section(accounts, sender.clone(), cx),
                cx,
            ))
            .child(detail_section(
                t::accounts::add_account_section(),
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
            .label(t::backends::add_backend())
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
                    .child(div().text_xl().font_semibold().child(t::backends::title()))
                    .child(
                        Button::new("close-backend-settings")
                            .label(t::common::close())
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.show_backend_settings = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(detail_section(
                t::backends::add_backend_section(),
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
                                .child(t::backends::manifest_url_hint()),
                        )
                    }),
                cx,
            ))
            .child(detail_section(
                t::backends::configured_backends(),
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

struct SectionParams<'a> {
    title: String,
    backend: Option<&'a BackendStatus>,
    instances: Vec<InstanceView>,
    empty_hint: Option<String>,
    header_action: Option<Button>,
    hide_usernames: bool,
    sender: BackendSender,
}

fn section(params: SectionParams<'_>, cx: &mut Context<InstancesPage>) -> gpui::Div {
    let SectionParams {
        title,
        backend,
        instances,
        empty_hint,
        header_action,
        hide_usernames,
        sender,
    } = params;
    let cards = instances
        .into_iter()
        .map(|instance| instance_card(instance, hide_usernames, sender.clone(), cx))
        .collect::<Vec<_>>();

    let backend_fetch = backend.map(|backend| {
        let fetch_color = match &backend.fetch_state {
            BackendFetchState::Offline | BackendFetchState::Error(_) => cx.theme().red,
            BackendFetchState::Fetching => cx.theme().yellow,
            BackendFetchState::Fetched { .. } => cx.theme().foreground,
            BackendFetchState::NotFetched => cx.theme().muted_foreground,
        };
        (fetch_state_label(&backend.fetch_state), fetch_color)
    });

    v_flex()
        .gap_3()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .gap_2()
                .child(div().text_lg().font_semibold().child(title))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when_some(header_action, |this, action| this.child(action))
                        .when_some(backend_fetch, |this, (fetch_state, fetch_color)| {
                            this.child(div().text_sm().text_color(fetch_color).child(fetch_state))
                        }),
                ),
        )
        .when(cards.is_empty(), |this| {
            this.when_some(empty_hint, |this, hint| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                )
            })
        })
        .when(!cards.is_empty(), |this| {
            this.child(h_flex().flex_wrap().gap_3().children(cards))
        })
}

fn local_loader_button(
    loader: LocalLoader,
    selected: LocalLoader,
    cx: &mut Context<InstancesPage>,
) -> Button {
    let label = match loader {
        LocalLoader::Vanilla => t::local::loader_vanilla(),
        LocalLoader::Fabric => t::local::loader_fabric(),
        LocalLoader::Forge => t::local::loader_forge(),
        LocalLoader::Neoforge => t::local::loader_neoforge(),
    };
    let id = match loader {
        LocalLoader::Vanilla => "vanilla",
        LocalLoader::Fabric => "fabric",
        LocalLoader::Forge => "forge",
        LocalLoader::Neoforge => "neoforge",
    };
    Button::new(format!("create-local-loader-{id}"))
        .label(label)
        .disabled(selected == loader)
        .on_click(cx.listener(move |page, _, _, cx| {
            page.create_local_loader = loader;
            if let Some(minecraft_version) = page
                .create_local_mc_version_select
                .read(cx)
                .selected_value()
                .cloned()
            {
                page.request_loader_versions(minecraft_version.as_str(), cx);
            }
            cx.notify();
        }))
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
            t::backends::refreshing_title(title.clone()),
            t::backends::refreshing_message().to_string(),
        ),
        BackendFetchState::Offline => (
            t::backends::offline_title(title.clone()),
            t::backends::offline_message().to_string(),
        ),
        BackendFetchState::Error(error) => (
            t::backends::fetch_failed_title(title.clone()),
            error.to_string(),
        ),
        BackendFetchState::NotFetched => (
            t::backends::not_fetched_title(title.clone()),
            t::backends::not_fetched_message().to_string(),
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
        .label(t::common::settings())
        .on_click(cx.listener(move |page, _, window, cx| {
            page.show_global_settings = false;
            page.show_backend_settings = false;
            page.show_accounts_panel = false;
            page.selected_instance =
                (page.selected_instance != Some(details_id)).then_some(details_id);
            let stored = page.selected_instance.and_then(|id| {
                page.data
                    .instances
                    .read(cx)
                    .entries
                    .iter()
                    .find(|v| v.id == id)
                    .and_then(|v| v.java_path.clone())
            });
            let value = stored.as_deref().unwrap_or_default().to_owned();
            page.java_path_last_instance = page.selected_instance;
            page.java_path_last_stored = stored;
            page.java_path_input
                .update(cx, |state, cx| state.set_value(value, window, cx));
            cx.notify();
        }));

    v_flex()
        .w_80()
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
        t::instances::add_account_card().to_string()
    } else {
        instance
            .effective_account_username
            .as_ref()
            .map(|username| username.to_string())
            .unwrap_or_else(|| t::instances::no_account_selected().to_string())
    };
    let provider = {
        let label = instance
            .effective_auth_provider
            .as_ref()
            .or(instance.auth_provider.as_ref())
            .map(provider_type_label)
            .unwrap_or_else(|| t::instances::any_provider().to_string());
        if instance.account_override.is_some() {
            format!("{label}{}", t::instances::override_suffix())
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
            .label(t::common::cancel())
            .on_click(move |_, _, _| sender.send(MessageToBackend::CancelInstall(instance.id))),
        InstanceLiveStatus::NotInstalled | InstanceLiveStatus::Outdated => {
            if matches!(instance.origin, InstanceOrigin::Local) {
                Button::new(format!("local-unavailable-{}", instance.id))
                    .label(t::instances::install())
                    .disabled(true)
            } else {
                Button::new(format!("install-{}", instance.id))
                    .label(if matches!(instance.status, InstanceLiveStatus::Outdated) {
                        t::instances::update()
                    } else {
                        t::instances::install()
                    })
                    .on_click(move |_, _, _| {
                        sender.send(MessageToBackend::InstallInstance {
                            id: instance.id,
                            force_overwrite: false,
                        });
                    })
            }
        }
        InstanceLiveStatus::Launching => Button::new(format!("launching-{}", instance.id))
            .label(t::instances::launching())
            .disabled(true),
        InstanceLiveStatus::Running => Button::new(format!("kill-{}", instance.id))
            .label(t::instances::kill())
            .on_click(move |_, _, _| sender.send(MessageToBackend::KillInstance(instance.id))),
        InstanceLiveStatus::Installed | InstanceLiveStatus::OrphanedFromBackend => {
            if instance.launch_blocked_reason.is_some() {
                let provider = instance.auth_provider.clone();
                return Button::new(format!("add-account-{}", instance.id))
                    .label(t::accounts::add_account_section())
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
                .label(t::instances::play())
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::Launch {
                        instance: instance.id,
                        account: None,
                    });
                })
        }
        InstanceLiveStatus::InstallFailed(_) => {
            if matches!(instance.origin, InstanceOrigin::Local) {
                Button::new(format!("retry-failed-local-{}", instance.id))
                    .label(t::common::retry())
                    .on_click(move |_, _, _| {
                        sender.send(MessageToBackend::RetryCreateLocal(instance.id))
                    })
            } else {
                Button::new(format!("retry-{}", instance.id))
                    .label(t::common::retry())
                    .on_click(move |_, _, _| {
                        sender.send(MessageToBackend::InstallInstance {
                            id: instance.id,
                            force_overwrite: false,
                        });
                    })
            }
        }
        InstanceLiveStatus::LaunchFailed(_) => Button::new(format!("play-again-{}", instance.id))
            .label(if instance.launch_blocked_reason.is_some() {
                t::instances::add_account()
            } else {
                t::instances::play_again()
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
        t::accounts::account_section(),
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t::accounts::required_provider(provider_label(
                        required_provider,
                    ))),
            )
            .when(matching_accounts.is_empty(), |this| {
                this.child(
                    Button::new(format!("add-required-account-{}", instance.id))
                        .label(t::instances::add_account())
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
            t::accounts::account_override_section(),
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
            .label(if selected {
                t::common::selected()
            } else {
                t::common::select()
            })
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
    java_path_input: gpui::Entity<InputState>,
    java_resolve: gpui::Entity<JavaResolveCache>,
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
                .child(t::instances::effective_memory(
                    instance.effective_xmx_mb.unwrap_or(4096),
                )),
        )
        .child(
            h_flex()
                .gap_2()
                .child(Input::new(&memory_input))
                .child(
                    Button::new(format!("save-memory-{id}"))
                        .label(t::instances::set_memory())
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
                        .label(t::common::default())
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
                        .child(t::instances::jvm_flags(
                            instance
                                .jvm_flags
                                .as_deref()
                                .unwrap_or(t::instances::jvm_flags_default())
                                .to_string(),
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Input::new(&jvm_flags_input))
                        .child(
                            Button::new(format!("save-jvm-flags-{id}"))
                                .label(t::instances::set_flags())
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
                                .label(t::common::default())
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
        .child(java_section(
            instance,
            java_path_input,
            java_resolve,
            sender,
            cx,
        ))
}

fn java_section(
    instance: &InstanceView,
    java_path_input: gpui::Entity<InputState>,
    java_resolve: gpui::Entity<JavaResolveCache>,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let id = instance.id;
    let local_install_in_progress =
        matches!(instance.origin, InstanceOrigin::Local) && !instance.locally_installed;

    v_flex()
        .gap_1()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .child(t::instances::java_section()),
        )
        .when(local_install_in_progress, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t::instances::java_install_required()),
            )
        })
        .when(!local_install_in_progress, |this| {
            let resolve_state = java_resolve.read(cx).state(id);
            this.when_some(instance.required_java_version.clone(), |this, version| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t::instances::required_java_version(version.to_string())),
                )
            })
            .child(
                h_flex().gap_2().child(Input::new(&java_path_input)).child(
                    Button::new(format!("java-set-{id}"))
                        .label(t::instances::set_java_path())
                        .on_click({
                            let input = java_path_input.clone();
                            let sender = sender.clone();
                            move |_, _, cx| {
                                let value = input.read(cx).value().trim().to_string();
                                if !value.is_empty() {
                                    sender.send(MessageToBackend::SetInstanceJavaPath {
                                        instance: id,
                                        path: Some(value),
                                    });
                                }
                            }
                        }),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(format!("java-auto-{id}"))
                            .label(t::instances::java_auto())
                            .on_click({
                                let sender = sender.clone();
                                cx.listener(move |page, _, _, cx| {
                                    page.data.java_resolve.update(cx, |cache, cx| {
                                        cache.set_resolving(id, cx);
                                    });
                                    sender.send(MessageToBackend::ResolveJavaPath(id));
                                })
                            }),
                    )
                    .child(
                        Button::new(format!("java-clear-{id}"))
                            .label(t::common::default())
                            .disabled(instance.java_path.is_none())
                            .on_click({
                                let sender = sender.clone();
                                let input = java_path_input.clone();
                                cx.listener(move |_, _, window, cx| {
                                    input.update(cx, |state, cx| {
                                        state.set_value(String::new(), window, cx)
                                    });
                                    sender.send(MessageToBackend::SetInstanceJavaPath {
                                        instance: id,
                                        path: None,
                                    });
                                })
                            }),
                    )
                    .child(
                        Button::new(format!("java-browse-{id}"))
                            .label(t::instances::java_browse())
                            .on_click({
                                let sender = sender.clone();
                                cx.listener(move |_, _, _window, cx| {
                                    let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
                                        files: true,
                                        directories: false,
                                        multiple: false,
                                        prompt: None,
                                    });
                                    let sender = sender.clone();
                                    cx.spawn(async move |_, _cx| {
                                        if let Ok(Ok(Some(paths))) = receiver.await
                                            && let Some(path) = paths.into_iter().next()
                                        {
                                            let path_str = path.to_string_lossy().to_string();
                                            sender.send(MessageToBackend::SetInstanceJavaPath {
                                                instance: id,
                                                path: Some(path_str),
                                            });
                                        }
                                    })
                                    .detach();
                                })
                            }),
                    ),
            )
            .when_some(resolve_state, |this, state| {
                use crate::entity::java_resolve::JavaResolveState;
                if matches!(
                    state,
                    JavaResolveState::Resolving | JavaResolveState::NotFound
                ) {
                    let text = match state {
                        JavaResolveState::Resolving => t::instances::java_resolving(),
                        _ => t::instances::java_not_found(),
                    };
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(text),
                    )
                } else {
                    this
                }
            })
            .child(native_glfw_toggle(instance, sender.clone(), cx))
        })
}

#[cfg(target_os = "linux")]
fn native_glfw_toggle(
    instance: &InstanceView,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let id = instance.id;
    let enabled = instance
        .use_native_glfw
        .unwrap_or_else(use_native_glfw_default);
    h_flex()
        .justify_between()
        .items_center()
        .gap_2()
        .child(
            v_flex()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(t::instances::use_native_glfw_title()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t::instances::use_native_glfw_desc()),
                ),
        )
        .child(
            Button::new(format!("toggle-native-glfw-{id}"))
                .label(if enabled {
                    t::common::on()
                } else {
                    t::common::off()
                })
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::SetInstanceUseNativeGlfw {
                        instance: id,
                        enabled: !enabled,
                    });
                }),
        )
}

#[cfg(not(target_os = "linux"))]
fn native_glfw_toggle(
    _instance: &InstanceView,
    _sender: BackendSender,
    _cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    div()
}

fn launcher_settings_section(
    settings: LauncherSettingsView,
    launcher_dir: PathBuf,
    notifications: gpui::Entity<NotificationEntries>,
    sender: BackendSender,
    cx: &mut Context<InstancesPage>,
) -> gpui::Div {
    let language = settings.language.clone();
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
                        .child(
                            div()
                                .font_semibold()
                                .child(t::settings::hide_after_launch_title()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t::settings::hide_after_launch_desc()),
                        ),
                )
                .child(
                    Button::new("toggle-hide-after-launch")
                        .label(if settings.hide_window_after_launch {
                            t::common::on()
                        } else {
                            t::common::off()
                        })
                        .on_click({
                            let sender = sender.clone();
                            let settings = settings.clone();
                            move |_, _, _| {
                                sender.send(MessageToBackend::SetLauncherSettings(
                                    LauncherSettingsView {
                                        hide_window_after_launch: !settings
                                            .hide_window_after_launch,
                                        hide_usernames_in_cards: settings.hide_usernames_in_cards,
                                        language: settings.language.clone(),
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
                        .child(
                            div()
                                .font_semibold()
                                .child(t::settings::hide_usernames_title()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t::settings::hide_usernames_desc()),
                        ),
                )
                .child(
                    Button::new("toggle-hide-usernames")
                        .label(if settings.hide_usernames_in_cards {
                            t::common::on()
                        } else {
                            t::common::off()
                        })
                        .on_click({
                            let sender = sender.clone();
                            let settings = settings.clone();
                            move |_, _, _| {
                                sender.send(MessageToBackend::SetLauncherSettings(
                                    LauncherSettingsView {
                                        hide_window_after_launch: settings.hide_window_after_launch,
                                        hide_usernames_in_cards: !settings.hide_usernames_in_cards,
                                        language: settings.language.clone(),
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
                        .child(div().font_semibold().child(t::settings::language_title()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t::settings::language_desc()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("language-en")
                                .label(t::settings::language_english())
                                .disabled(language == "en")
                                .on_click({
                                    let sender = sender.clone();
                                    let settings = settings.clone();
                                    move |_, _, _| {
                                        sender.send(MessageToBackend::SetLauncherSettings(
                                            LauncherSettingsView {
                                                hide_window_after_launch: settings
                                                    .hide_window_after_launch,
                                                hide_usernames_in_cards: settings
                                                    .hide_usernames_in_cards,
                                                language: "en".to_string(),
                                            },
                                        ));
                                    }
                                }),
                        )
                        .child(
                            Button::new("language-ru")
                                .label(t::settings::language_russian())
                                .disabled(language == "ru")
                                .on_click({
                                    let sender = sender.clone();
                                    let settings = settings.clone();
                                    move |_, _, _| {
                                        sender.send(MessageToBackend::SetLauncherSettings(
                                            LauncherSettingsView {
                                                hide_window_after_launch: settings
                                                    .hide_window_after_launch,
                                                hide_usernames_in_cards: settings
                                                    .hide_usernames_in_cards,
                                                language: "ru".to_string(),
                                            },
                                        ));
                                    }
                                }),
                        ),
                ),
        )
        .child(
            Button::new("open-launcher-directory")
                .label(t::instances::open_launcher_directory())
                .on_click(move |_, _, cx| {
                    if let Err(err) = open_path(&launcher_dir) {
                        notifications.update(cx, |entries, cx| {
                            entries.push(
                                NotificationLevel::Error,
                                t::notifications::failed_open_launcher_directory(err.to_string()),
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
                .child(t::accounts::no_accounts_yet()),
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
                    .label(t::common::remove())
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
            t::accounts::add_microsoft(),
            sender.clone(),
        ))
        .child(
            h_flex().gap_2().child(Input::new(&offline_nickname)).child(
                Button::new("settings-add-offline")
                    .label(t::accounts::add_offline())
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
                        .label(t::accounts::add_telegram())
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
                        .label(t::accounts::add_elyby())
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
                                                t::auth::default_launcher_name().to_string()
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
                .child(t::backends::no_backends_yet()),
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
                t::backends::configured().to_string()
            } else if backend.referenced_by_instances {
                t::instances::used_by_installed().to_string()
            } else {
                t::backends::discovered().to_string()
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
                                .label(t::common::remove())
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
    let show_remove = matches!(instance.status, InstanceLiveStatus::InstallFailed(_))
        && !instance.locally_installed
        && matches!(instance.origin, InstanceOrigin::Local);
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(format!("detail-play-{id}"))
                        .label(t::instances::play())
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
                        .label(t::instances::kill())
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
                .when(!matches!(instance.origin, InstanceOrigin::Local), |this| {
                    this.child(
                        Button::new(format!("detail-resync-{id}"))
                            .label(t::instances::resync())
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
                            .label(t::instances::hard_resync())
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
                    )
                }),
        )
        .child(h_flex().gap_2().child(if show_remove {
            Button::new(format!("detail-remove-{id}"))
                .label(t::instances::remove())
                .on_click({
                    let sender = sender.clone();
                    cx.listener(move |page, _, _, cx| {
                        page.selected_instance = None;
                        sender.send(MessageToBackend::CancelInstall(id));
                        cx.notify();
                    })
                })
                .into_any_element()
        } else {
            Button::new(format!("detail-delete-{id}"))
                .label(if pending_delete {
                    t::instances::confirm_delete()
                } else {
                    t::instances::delete()
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
                })
                .into_any_element()
        }))
        .when(pending_delete && !show_remove, |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().red)
                    .child(t::instances::confirm_delete_hint()),
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
            matches!(instance.status, InstanceLiveStatus::Outdated)
                && !matches!(instance.origin, InstanceOrigin::Local),
            |this| {
                this.child(
                    Button::new(format!("detail-update-{id}"))
                        .label(t::instances::update())
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
                    .child(t::instances::recommended_memory(xmx)),
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
        AuthProviderConfig::Microsoft(_) => t::providers::microsoft().to_string(),
        AuthProviderConfig::Telegram(provider) => {
            t::providers::telegram_with_url(provider.auth_base_url.clone())
        }
        AuthProviderConfig::ElyBy(_) => t::providers::elyby().to_string(),
        AuthProviderConfig::Offline(_) => t::providers::offline().to_string(),
    }
}

fn provider_type_label(provider: &AuthProviderConfig) -> String {
    match provider {
        AuthProviderConfig::Microsoft(_) => t::providers::microsoft().to_string(),
        AuthProviderConfig::Telegram(_) => t::providers::telegram().to_string(),
        AuthProviderConfig::ElyBy(_) => t::providers::elyby().to_string(),
        AuthProviderConfig::Offline(_) => t::providers::offline().to_string(),
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
        BackendFetchState::NotFetched => t::backends::not_fetched().to_string(),
        BackendFetchState::Fetching => t::common::refreshing().to_string(),
        BackendFetchState::Fetched { instance_count } => {
            t::backends::published_count(*instance_count)
        }
        BackendFetchState::Offline => t::backends::offline().to_string(),
        BackendFetchState::Error(error) => error.to_string(),
    }
}

fn status_label(instance: &InstanceView) -> String {
    let label = match &instance.status {
        InstanceLiveStatus::NotInstalled => t::instances::status_available().to_string(),
        InstanceLiveStatus::Installed => t::instances::status_installed().to_string(),
        InstanceLiveStatus::Outdated => t::instances::status_outdated().to_string(),
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
        InstanceLiveStatus::InstallFailed(_) => t::instances::status_failed().to_string(),
        InstanceLiveStatus::Launching => t::instances::launching().to_string(),
        InstanceLiveStatus::Running => t::instances::status_running().to_string(),
        InstanceLiveStatus::LaunchFailed(_) => t::instances::status_launch_failed().to_string(),
        InstanceLiveStatus::OrphanedFromBackend => t::instances::status_orphaned().to_string(),
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
        t::instances::status_no_account(label)
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

fn create_local_form_issue(
    page: &InstancesPage,
    instances: &[InstanceView],
    cx: &App,
) -> Option<String> {
    let name = page
        .create_local_name_input
        .read(cx)
        .value()
        .trim()
        .to_string();
    if name.is_empty() {
        return Some(t::notifications::local_instance_name_empty().to_string());
    }

    let sanitized = sanitize_dir_name(&name);
    for instance in instances {
        if instance.dir_name.as_ref() == sanitized
            || instance.display_name.as_ref() == name
            || instance.display_name.as_ref() == sanitized
        {
            return Some(t::notifications::local_instance_name_exists(name));
        }
    }

    if page
        .create_local_mc_version_select
        .read(cx)
        .selected_value()
        .is_none()
    {
        return Some(format!(
            "{} {}",
            t::common::select(),
            t::local::minecraft_version()
        ));
    }

    let local_create = page.data.local_create.read(cx);
    let state = &local_create.state;
    if state.minecraft_loading {
        return Some(t::local::versions_loading_game_versions().to_string());
    }
    if state.minecraft_error.is_some() {
        return Some(t::local::versions_error().to_string());
    }

    let selected_mc = page
        .create_local_mc_version_select
        .read(cx)
        .selected_value()
        .map(|version| version.to_string());

    match page.create_local_loader {
        LocalLoader::Vanilla => {}
        LocalLoader::Fabric => {
            if state.loader_loading
                && state.loader_minecraft_version == selected_mc
                && state.loader_kind == Some(LocalLoader::Fabric)
            {
                return Some(t::local::loader_versions_loading().to_string());
            }
            if state.loader_error.is_some()
                && state.loader_minecraft_version == selected_mc
                && state.loader_kind == Some(LocalLoader::Fabric)
            {
                return Some(t::local::loader_versions_error().to_string());
            }
        }
        LocalLoader::Forge | LocalLoader::Neoforge => {
            if page
                .create_local_loader_version_select
                .read(cx)
                .selected_value()
                .is_none()
            {
                return Some(
                    t::notifications::local_instance_loader_version_required().to_string(),
                );
            }
            if state.loader_loading
                && state.loader_minecraft_version == selected_mc
                && state.loader_kind == Some(page.create_local_loader)
            {
                return Some(t::local::loader_versions_loading().to_string());
            }
            if state.loader_error.is_some()
                && state.loader_minecraft_version == selected_mc
                && state.loader_kind == Some(page.create_local_loader)
            {
                return Some(t::local::loader_versions_error().to_string());
            }
        }
    }

    None
}

#[derive(Default)]
struct VersionList {
    versions: Vec<SharedString>,
    matched_versions: Vec<SharedString>,
}

impl SelectDelegate for VersionList {
    type Item = SharedString;

    fn items_count(&self, _section: usize) -> usize {
        self.matched_versions.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        self.matched_versions.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SelectItem<Value = V>,
        V: PartialEq,
    {
        for (index, item) in self.matched_versions.iter().enumerate() {
            if item.value() == value {
                return Some(IndexPath::default().row(index));
            }
        }
        None
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _: &mut App) -> gpui::Task<()> {
        let lower_query = query.to_lowercase();
        self.matched_versions = self
            .versions
            .iter()
            .filter(|item| item.to_lowercase().starts_with(&lower_query))
            .cloned()
            .collect();
        gpui::Task::ready(())
    }
}
