use std::{collections::HashSet, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Selectable as _, Sizable, WindowExt as _,
    button::{Button, ButtonVariants as _},
    command::{Command, CommandItem, CommandState},
    h_flex,
    input::{InputEvent, InputState},
    select::SelectState,
    sidebar::{
        Sidebar, SidebarFooter, SidebarGroup, SidebarMenu, SidebarMenuItem, SidebarToggleButton,
    },
    v_flex,
};
use gpui_kit::{
    AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*,
};
use instance::storage::InstanceHandle;
use launcher_bridge::{
    BackendFetchState, InstanceLiveStatus, InstanceOrigin, InstanceView, LocalLoader,
    MessageToBackend,
};
use launcher_i18n as t;

use super::common::{backend_display_name, backend_display_names, can_launch, status_dot};
use super::modals::VersionList;
use crate::component::{log_view::LogView, memory::MemoryControl};
use crate::entity::{
    DataEntities,
    account::AccountsUpdatedEvent,
    auth::AuthSessionUpdatedEvent,
    backend::BackendsUpdatedEvent,
    instance::InstancesUpdatedEvent,
    java_resolve::{JavaResolveState, JavaResolvedEvent},
    local_create::{LoaderVersionsUpdatedEvent, LocalCreateVersionsUpdatedEvent},
    settings::LauncherSettingsUpdatedEvent,
};
use crate::{GoBack, OpenPalette, OpenSettings, RefreshAll};

#[derive(Clone, PartialEq)]
pub enum Route {
    Library,
    Instance(InstanceHandle),
    Accounts,
    Backends,
    Settings,
}

#[derive(Clone, PartialEq)]
pub enum LibraryFilter {
    All,
    Backend(String),
    Local,
}

#[derive(Clone, Copy, PartialEq)]
pub enum InstanceTab {
    Overview,
    Mods,
    Runtime,
    Account,
    Logs,
}

pub struct InstancesPage {
    pub(super) data: DataEntities,
    pub(super) route: Route,
    pub(super) filter: LibraryFilter,
    pub(super) instance_tab: InstanceTab,
    pub(super) sidebar_collapsed: bool,
    pub(super) show_create_local_modal: bool,
    pub(super) create_local_loader: LocalLoader,
    pub(super) create_local_show_snapshots: bool,
    pub(super) create_local_sync_mc_dropdown: bool,
    pub(super) create_local_sync_loader_dropdown: bool,
    hidden_launches: HashSet<InstanceHandle>,
    pub(super) backend_url_input: Entity<InputState>,
    pub(super) offline_nickname_input: Entity<InputState>,
    pub(super) telegram_base_url_input: Entity<InputState>,
    pub(super) elyby_client_id_input: Entity<InputState>,
    pub(super) elyby_client_secret_input: Entity<InputState>,
    pub(super) elyby_launcher_name_input: Entity<InputState>,
    pub(super) jvm_opts_input: Entity<InputState>,
    pub(super) java_path_input: Entity<InputState>,
    pub(super) create_local_name_input: Entity<InputState>,
    pub(super) create_local_mc_version_select: Entity<SelectState<VersionList>>,
    pub(super) create_local_loader_version_select: Entity<SelectState<VersionList>>,
    pub(super) memory: Entity<MemoryControl>,
    pub(super) log_view: Option<Entity<LogView>>,
    palette: Entity<CommandState>,
    palette_open: bool,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl InstancesPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let backend_url_input = input(t::placeholders::manifest_url(), window, cx);
        let offline_nickname_input = input(t::placeholders::offline_nickname(), window, cx);
        let telegram_base_url_input = input(t::placeholders::telegram_auth_base_url(), window, cx);
        let elyby_client_id_input = input(t::placeholders::client_id(), window, cx);
        let elyby_client_secret_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t::placeholders::client_secret())
                .masked(true)
        });
        let elyby_launcher_name_input = input(t::placeholders::launcher_name(), window, cx);
        let jvm_opts_input = input(t::placeholders::jvm_opts(), window, cx);
        let java_path_input = input(t::instances::java_path_auto_detect(), window, cx);
        let create_local_name_input = input(t::placeholders::instance_name(), window, cx);
        let create_local_mc_version_select = cx
            .new(|cx| SelectState::new(VersionList::default(), None, window, cx).searchable(true));
        let create_local_loader_version_select = cx
            .new(|cx| SelectState::new(VersionList::default(), None, window, cx).searchable(true));
        let memory = cx.new(|cx| MemoryControl::new(data.backend_sender.clone(), window, cx));
        let palette = cx.new(|cx| CommandState::new(window, cx));
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let notify = |cx: &mut Context<Self>| cx.notify();
        let on_enter = |input: &Entity<InputState>,
                        action: fn(&mut Self, &mut Window, &mut Context<Self>),
                        window: &mut Window,
                        cx: &mut Context<Self>| {
            cx.subscribe_in(
                input,
                window,
                move |page, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        action(page, window, cx);
                    }
                },
            )
        };
        let enter_subscriptions = [
            on_enter(&jvm_opts_input, Self::save_jvm_opts, window, cx),
            on_enter(&java_path_input, Self::save_java_path, window, cx),
            on_enter(
                &backend_url_input,
                |page, window, cx| page.add_backend(window, cx),
                window,
                cx,
            ),
            on_enter(
                &offline_nickname_input,
                |page, window, cx| page.add_offline(window, cx),
                window,
                cx,
            ),
        ];
        let subscriptions = vec![
            cx.subscribe(&data.backends, move |_, _, _: &BackendsUpdatedEvent, cx| {
                notify(cx)
            }),
            cx.subscribe(&data.accounts, move |_, _, _: &AccountsUpdatedEvent, cx| {
                notify(cx)
            }),
            cx.subscribe(
                &data.settings,
                move |_, _, _: &LauncherSettingsUpdatedEvent, cx| notify(cx),
            ),
            cx.subscribe(&data.auth, move |_, _, _: &AuthSessionUpdatedEvent, cx| {
                notify(cx)
            }),
            cx.subscribe_in(
                &data.instances,
                window,
                |page, _, _: &InstancesUpdatedEvent, window, cx| {
                    page.sync_instance_inputs(false, window, cx);
                    cx.notify();
                },
            ),
            cx.subscribe(&create_local_mc_version_select, |page, _, event, cx| {
                page.on_minecraft_version_selected(event, cx)
            }),
            cx.subscribe(
                &data.local_create,
                |page, _, _: &LocalCreateVersionsUpdatedEvent, cx| {
                    page.create_local_sync_mc_dropdown = true;
                    cx.notify();
                },
            ),
            cx.subscribe(
                &data.local_create,
                |page, _, _: &LoaderVersionsUpdatedEvent, cx| {
                    page.create_local_sync_loader_dropdown = true;
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &data.java_resolve,
                window,
                |page, _, event: &JavaResolvedEvent, window, cx| {
                    if page.selected_instance() != Some(&event.0) {
                        return;
                    }
                    let value = match page.data.java_resolve.read(cx).state(&event.0) {
                        Some(JavaResolveState::Found(path)) => path.to_string(),
                        _ => String::new(),
                    };
                    page.java_path_input
                        .update(cx, |state, cx| state.set_value(value, window, cx));
                },
            ),
        ];

        let mut subscriptions = subscriptions;
        subscriptions.extend(enter_subscriptions);
        Self {
            data: data.clone(),
            route: Route::Library,
            filter: LibraryFilter::All,
            instance_tab: InstanceTab::Overview,
            sidebar_collapsed: false,
            show_create_local_modal: false,
            create_local_loader: LocalLoader::Vanilla,
            create_local_show_snapshots: false,
            create_local_sync_mc_dropdown: false,
            create_local_sync_loader_dropdown: false,
            hidden_launches: HashSet::new(),
            backend_url_input,
            offline_nickname_input,
            telegram_base_url_input,
            elyby_client_id_input,
            elyby_client_secret_input,
            elyby_launcher_name_input,
            jvm_opts_input,
            java_path_input,
            create_local_name_input,
            create_local_mc_version_select,
            create_local_loader_version_select,
            memory,
            log_view: None,
            palette,
            palette_open: false,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn selected_instance(&self) -> Option<&InstanceHandle> {
        match &self.route {
            Route::Instance(handle) => Some(handle),
            _ => None,
        }
    }

    pub(super) fn find_instance(
        &self,
        handle: &InstanceHandle,
        cx: &gpui_kit::App,
    ) -> Option<InstanceView> {
        self.data
            .instances
            .read(cx)
            .entries
            .iter()
            .find(|instance| &instance.handle == handle)
            .cloned()
    }

    pub fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        let opening_instance =
            matches!(&route, Route::Instance(handle) if self.selected_instance() != Some(handle));
        if let Route::Instance(handle) = &route
            && let Some(instance) = self.find_instance(handle, cx)
            && !self.filter_contains(&instance)
        {
            self.filter = LibraryFilter::All;
        }
        self.route = route;
        if opening_instance {
            self.instance_tab = InstanceTab::Overview;
            self.log_view = None;
            self.sync_instance_inputs(true, window, cx);
        }
        cx.notify();
    }

    pub fn open_instance_tab(
        &mut self,
        handle: InstanceHandle,
        tab: InstanceTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(Route::Instance(handle), window, cx);
        self.set_instance_tab(tab, window, cx);
    }

    pub(super) fn set_instance_tab(
        &mut self,
        tab: InstanceTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.instance_tab = tab;
        if tab == InstanceTab::Logs
            && self.log_view.is_none()
            && let Some(instance) = self
                .selected_instance()
                .cloned()
                .and_then(|handle| self.find_instance(&handle, cx))
        {
            let path = crate::launch_log_path(&self.data.launcher_dir, &instance.dir_name);
            let game_dir = crate::game_dir(&self.data.launcher_dir, &instance.dir_name);
            let sender = self.data.backend_sender.clone();
            let notifications = self.data.notifications.clone();
            self.log_view =
                Some(cx.new(|cx| LogView::new(path, game_dir, sender, notifications, window, cx)));
        }
        cx.notify();
    }

    fn sync_instance_inputs(&mut self, opened: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(instance) = self
            .selected_instance()
            .cloned()
            .and_then(|handle| self.find_instance(&handle, cx))
        else {
            return;
        };
        self.memory
            .update(cx, |memory, cx| memory.sync_from(&instance, window, cx));
        if opened {
            let jvm_opts = instance.jvm_opts.as_deref().unwrap_or_default().to_string();
            self.jvm_opts_input
                .update(cx, |input, cx| input.set_value(jvm_opts, window, cx));
        }
        let java_path = instance
            .java_path
            .as_deref()
            .unwrap_or_default()
            .to_string();
        self.java_path_input.update(cx, |input, cx| {
            if opened || input.value() != java_path {
                input.set_value(java_path, window, cx)
            }
        });
    }

    pub(super) fn save_jvm_opts(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(instance) = self.selected_instance().cloned() else {
            return;
        };
        let value = self.jvm_opts_input.read(cx).value().trim().to_string();
        self.data
            .backend_sender
            .send(MessageToBackend::SetInstanceJvmOpts {
                instance,
                opts: (!value.is_empty()).then_some(value),
            });
    }

    pub(super) fn save_java_path(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(instance) = self.selected_instance().cloned() else {
            return;
        };
        let value = self.java_path_input.read(cx).value().trim().to_string();
        self.data
            .backend_sender
            .send(MessageToBackend::SetInstanceJavaPath {
                instance,
                path: (!value.is_empty()).then_some(value),
            });
    }

    pub(super) fn refresh(&self) {
        self.data.backend_sender.send(MessageToBackend::Refresh);
    }

    fn filter_contains(&self, instance: &InstanceView) -> bool {
        match (&self.filter, &instance.origin) {
            (LibraryFilter::All, _) => true,
            (LibraryFilter::Local, InstanceOrigin::Local) => true,
            (LibraryFilter::Backend(url), InstanceOrigin::Backend { url: origin }) => {
                origin.as_str() == url
            }
            _ => false,
        }
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
            window.close_dialog(cx);
            return;
        }
        if window.has_active_dialog(cx)
            || self.show_create_local_modal
            || self.data.auth.read(cx).is_active()
        {
            return;
        }
        self.palette_open = true;
        let instances = self.data.instances.read(cx).entries.clone();
        let mut targets = Vec::new();
        let mut items = Vec::new();
        for instance in instances.iter().filter(|instance| can_launch(instance)) {
            items.push(
                CommandItem::new()
                    .label(t::palette::play(instance.display_name.to_string()))
                    .icon(IconName::Play)
                    .keywords([instance.dir_name.to_string()]),
            );
            targets.push(PaletteTarget::Play(instance.handle.clone()));
        }
        for instance in &instances {
            items.push(
                CommandItem::new()
                    .label(instance.display_name.to_string())
                    .icon(IconName::Box)
                    .keywords([instance.dir_name.to_string()]),
            );
            targets.push(PaletteTarget::Route(Route::Instance(
                instance.handle.clone(),
            )));
        }
        for (label, icon, route) in [
            (t::nav::library(), IconName::LayoutGrid, Route::Library),
            (t::nav::accounts(), IconName::Users, Route::Accounts),
            (t::backends::title(), IconName::Network, Route::Backends),
            (t::common::settings(), IconName::Settings, Route::Settings),
        ] {
            items.push(CommandItem::new().label(label).icon(icon));
            targets.push(PaletteTarget::Route(route));
        }
        items.push(
            CommandItem::new()
                .label(t::common::refresh())
                .icon(IconName::RefreshCw),
        );
        targets.push(PaletteTarget::Refresh);
        items.push(
            CommandItem::new()
                .label(t::local::create_title())
                .icon(IconName::Plus),
        );
        targets.push(PaletteTarget::CreateLocal);

        let targets = Rc::new(targets);
        let items = Rc::new(items);
        let state = self.palette.clone();
        let page = cx.entity().downgrade();
        state.update(cx, |state, cx| state.set_query("", window, cx));
        let initial_focus = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                state.update(cx, |state, cx| state.focus(window, cx));
            }
            let targets = targets.clone();
            let page = page.clone();
            let close_page = page.clone();
            dialog
                .on_close(move |_, _, cx| {
                    let _ = close_page.update(cx, |page, _| page.palette_open = false);
                })
                .w(gpui_kit::px(520.0))
                .margin_top(gpui_kit::px(96.0))
                .close_button(false)
                .p_0()
                .child(
                    div()
                        .on_action(|_: &OpenPalette, window, cx| window.close_dialog(cx))
                        .child(
                            Command::new(&state)
                                .items(items.iter().cloned())
                                .placeholder(t::palette::placeholder())
                                .max_h(gpui_kit::px(360.0))
                                .on_cancel(|window, cx| window.close_dialog(cx))
                                .on_confirm(move |index, window, cx| {
                                    let Some(target) = targets.get(index.row).cloned() else {
                                        return;
                                    };
                                    window.close_dialog(cx);
                                    let _ = page.update(cx, |page, cx| {
                                        page.run_palette(target, window, cx)
                                    });
                                }),
                        ),
                )
        });
    }

    fn run_palette(&mut self, target: PaletteTarget, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        match target {
            PaletteTarget::Play(instance) => {
                self.data.backend_sender.send(MessageToBackend::Launch {
                    instance,
                    account: None,
                })
            }
            PaletteTarget::Route(route) => self.navigate(route, window, cx),
            PaletteTarget::Refresh => self.refresh(),
            PaletteTarget::CreateLocal => {
                self.navigate(Route::Library, window, cx);
                self.open_create_local_modal(window, cx);
            }
        }
        self.focus_handle.focus(window, cx);
    }

    fn hide_while_running(
        &mut self,
        instances: &[InstanceView],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hide = self
            .data
            .settings
            .read(cx)
            .settings
            .hide_window_after_launch;
        let active = |instance: &InstanceView| {
            matches!(
                instance.status,
                InstanceLiveStatus::LaunchPreparing { .. } | InstanceLiveStatus::Running
            )
        };
        if hide
            && let Some(instance) = instances
                .iter()
                .find(|instance| matches!(instance.status, InstanceLiveStatus::Running))
            && self.hidden_launches.insert(instance.handle.clone())
        {
            window.minimize_window();
        }
        let before = self.hidden_launches.len();
        self.hidden_launches.retain(|handle| {
            instances
                .iter()
                .any(|instance| &instance.handle == handle && active(instance))
        });
        if self.hidden_launches.len() < before {
            window.activate_window();
        }
    }

    fn render_sidebar(
        &self,
        instances: &[InstanceView],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let backends = self.data.backends.read(cx).backends.clone();
        let names = backend_display_names(&backends);
        let refreshing = backends
            .iter()
            .any(|backend| matches!(backend.fetch_state, BackendFetchState::Fetching));
        let collapsed = self.sidebar_collapsed;
        let on_library = matches!(self.route, Route::Library | Route::Instance(_));
        let count = |filter: &LibraryFilter| {
            instances
                .iter()
                .filter(|instance| match (filter, &instance.origin) {
                    (LibraryFilter::All, _) => true,
                    (LibraryFilter::Local, InstanceOrigin::Local) => true,
                    (LibraryFilter::Backend(url), InstanceOrigin::Backend { url: origin }) => {
                        origin.as_str() == url
                    }
                    _ => false,
                })
                .count()
        };
        let count_suffix = |n: usize| {
            move |_: &mut Window, cx: &mut gpui_kit::App| {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(n.to_string())
            }
        };

        let mut library = SidebarMenu::new().child(
            SidebarMenuItem::new(t::nav::all())
                .icon(IconName::LayoutGrid)
                .active(on_library && self.filter == LibraryFilter::All)
                .suffix(count_suffix(count(&LibraryFilter::All)))
                .on_click(cx.listener(|page, _, window, cx| {
                    page.filter = LibraryFilter::All;
                    page.navigate(Route::Library, window, cx);
                })),
        );
        for backend in &backends {
            let url = backend.url.as_str().to_string();
            let filter = LibraryFilter::Backend(url.clone());
            let title = names
                .get(&url)
                .cloned()
                .unwrap_or_else(|| backend_display_name(&backend.url, false));
            let color = super::common::fetch_state_color(&backend.fetch_state, cx);
            let n = count(&filter);
            library = library.child(
                SidebarMenuItem::new(title)
                    .icon(IconName::Globe)
                    .active(on_library && self.filter == filter)
                    .suffix(move |_, cx| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(n.to_string()),
                            )
                            .child(status_dot(color))
                    })
                    .on_click(cx.listener(move |page, _, window, cx| {
                        page.filter = filter.clone();
                        page.navigate(Route::Library, window, cx);
                    })),
            );
        }
        library = library.child(
            SidebarMenuItem::new(t::common::local())
                .icon(IconName::HardDrive)
                .active(on_library && self.filter == LibraryFilter::Local)
                .suffix(count_suffix(count(&LibraryFilter::Local)))
                .on_click(cx.listener(|page, _, window, cx| {
                    page.filter = LibraryFilter::Local;
                    page.navigate(Route::Library, window, cx);
                })),
        );

        let manage = SidebarMenu::new()
            .child(
                SidebarMenuItem::new(t::backends::title())
                    .icon(IconName::Network)
                    .active(self.route == Route::Backends)
                    .on_click(cx.listener(|page, _, window, cx| {
                        page.navigate(Route::Backends, window, cx)
                    })),
            )
            .child(
                SidebarMenuItem::new(t::common::settings())
                    .icon(IconName::Settings)
                    .active(self.route == Route::Settings)
                    .on_click(cx.listener(|page, _, window, cx| {
                        page.navigate(Route::Settings, window, cx)
                    })),
            );

        let accounts = self.data.accounts.read(cx).accounts.clone();
        let account = accounts
            .iter()
            .find(|account| account.selected)
            .or(accounts.first())
            .cloned();
        let account_name: SharedString = account
            .as_ref()
            .map(|account| account.data.user_info.username.clone().into())
            .unwrap_or_else(|| t::nav::accounts().into());
        let account_detail = account
            .as_ref()
            .map(|account| super::common::provider_type_label(&account.provider))
            .unwrap_or_else(|| t::accounts::no_accounts_short().to_string());

        Sidebar::new("launcher-sidebar")
            .collapsed(collapsed)
            .w(gpui_kit::px(232.0))
            .header(
                div().w_full().child(
                    h_flex()
                        .w_full()
                        .when(!collapsed, |this| this.h_8())
                        .items_center()
                        .map(|this| {
                            if collapsed {
                                this.justify_center()
                            } else {
                                this.justify_between()
                            }
                        })
                        .gap_2()
                        .when(!collapsed, |this| {
                            this.child(
                                div()
                                    .pl_1()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(launcher_build_config::launcher_name()),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .when(collapsed, |this| this.flex_col())
                                .gap_1()
                                .child(
                                    Button::new("sidebar-refresh")
                                        .ghost()
                                        .small()
                                        .icon(IconName::RefreshCw)
                                        .loading(refreshing)
                                        .tooltip(t::common::refresh())
                                        .on_click(cx.listener(|page, _, _, _| page.refresh())),
                                )
                                .child(SidebarToggleButton::new().collapsed(collapsed).on_click(
                                    cx.listener(|page, _, _, cx| {
                                        page.sidebar_collapsed = !page.sidebar_collapsed;
                                        cx.notify();
                                    }),
                                )),
                        ),
                ),
            )
            .child(SidebarGroup::new(t::nav::library()).child(library))
            .child(SidebarGroup::new(t::nav::manage()).child(manage))
            .footer(
                SidebarFooter::new()
                    .selected(self.route == Route::Accounts)
                    .w_full()
                    .child(
                        h_flex()
                            .id("sidebar-account")
                            .on_click(cx.listener(|page, _, window, cx| {
                                page.navigate(Route::Accounts, window, cx)
                            }))
                            .w_full()
                            .gap_2()
                            .items_center()
                            .when(collapsed, |this| this.justify_center())
                            .child(super::common::avatar(&account_name).small())
                            .when(!collapsed, |this| {
                                this.child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_sm()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(account_name),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(account_detail),
                                        ),
                                )
                                .child(
                                    gpui_kit::component::Icon::new(IconName::ChevronRight)
                                        .small()
                                        .text_color(cx.theme().muted_foreground),
                                )
                            }),
                    ),
            )
    }
}

#[derive(Clone)]
enum PaletteTarget {
    Play(InstanceHandle),
    Route(Route),
    Refresh,
    CreateLocal,
}

impl Focusable for InstancesPage {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InstancesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let instances = self.data.instances.read(cx).entries.clone();
        self.hide_while_running(&instances, window, cx);
        if let Route::Instance(handle) = &self.route
            && !instances.iter().any(|instance| &instance.handle == handle)
        {
            self.route = Route::Library;
        }
        if let LibraryFilter::Backend(url) = &self.filter
            && !self
                .data
                .backends
                .read(cx)
                .backends
                .iter()
                .any(|backend| backend.url.as_str() == url)
        {
            self.filter = LibraryFilter::All;
        }

        let content = match self.route.clone() {
            Route::Library => self
                .render_library(&instances, window, cx)
                .into_any_element(),
            Route::Instance(handle) => match instances.iter().find(|i| i.handle == handle) {
                Some(instance) => self
                    .render_instance(instance.clone(), window, cx)
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Route::Accounts => self.render_accounts(cx).into_any_element(),
            Route::Backends => self.render_backends(window, cx).into_any_element(),
            Route::Settings => self.render_settings(cx).into_any_element(),
        };

        let modal_open = self.show_create_local_modal;
        let auth_open = self.data.auth.read(cx).is_active();
        h_flex()
            .id("launcher")
            .relative()
            .key_context("Launcher")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|page, _: &GoBack, window, cx| {
                if page.show_create_local_modal {
                    page.show_create_local_modal = false;
                    page.focus_handle.focus(window, cx);
                    cx.notify();
                } else if page.data.auth.read(cx).is_active() {
                    page.data.backend_sender.send(MessageToBackend::CancelAuth);
                } else if matches!(page.route, Route::Instance(_)) {
                    page.navigate(Route::Library, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(
                cx.listener(|page, _: &OpenPalette, window, cx| page.toggle_palette(window, cx)),
            )
            .on_action(cx.listener(|page, _: &OpenSettings, window, cx| {
                page.navigate(Route::Settings, window, cx)
            }))
            .on_action(cx.listener(|page, _: &RefreshAll, _, _| page.refresh()))
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_sidebar(&instances, cx))
            .child(div().relative().flex_1().min_w_0().h_full().child(content))
            .when(modal_open, |this| {
                this.child(self.create_local_modal(window, cx))
            })
            .when(auth_open, |this| this.child(self.auth_modal(cx)))
    }
}
