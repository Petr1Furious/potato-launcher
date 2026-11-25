use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, Sizable, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _, DropdownButton},
    checkbox::Checkbox,
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    scroll::ScrollableElement,
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    App, Context, Entity, IntoElement, MouseButton, SharedString, WeakEntity, Window, div,
    prelude::*, px,
};
use instance::storage::InstanceHandle;
use launcher_bridge::{
    AccountView, BackendFetchState, BackendSender, BackendStatus, InstanceLiveStatus,
    InstanceOrigin, InstanceView, MessageToBackend, NotificationLevel,
};
use launcher_i18n as t;

use super::common::{
    backend_display_name, backend_display_names, compatible_accounts, fetch_state_color,
    fetch_state_label, game_label, instance_busy, is_local, muted, open_path, page_header,
    provider_type_label, status_badge, status_dot,
};
use super::instances::{InstanceTab, InstancesPage, LibraryFilter, Route};
use crate::entity::instance::InstanceEntries;

const CARD_WIDTH: f32 = 300.0;
const CARD_HEIGHT: f32 = 212.0;

impl InstancesPage {
    pub(super) fn render_library(
        &mut self,
        instances: &[InstanceView],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let backends = self.data.backends.read(cx).backends.clone();
        let names = backend_display_names(&backends);
        let title_for = |backend: &BackendStatus| {
            names
                .get(backend.url.as_str())
                .cloned()
                .unwrap_or_else(|| backend_display_name(&backend.url, false))
        };
        let in_backend = |url: &str| {
            instances
                .iter()
                .filter(|instance| {
                    matches!(&instance.origin, InstanceOrigin::Backend { url: origin } if origin.as_str() == url)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let local = instances
            .iter()
            .filter(|instance| is_local(instance))
            .cloned()
            .collect::<Vec<_>>();

        let mut sections = Vec::new();
        let title: SharedString = match &self.filter {
            LibraryFilter::All => {
                for backend in &backends {
                    sections.push(self.backend_section(
                        backend,
                        title_for(backend),
                        in_backend(backend.url.as_str()),
                        true,
                        cx,
                    ));
                }
                sections.push(self.local_section(local, true, cx));
                t::nav::all().into()
            }
            LibraryFilter::Backend(url) => {
                let backend = backends.iter().find(|backend| backend.url.as_str() == url);
                if let Some(backend) = backend {
                    sections.push(self.backend_section(
                        backend,
                        title_for(backend),
                        in_backend(url),
                        false,
                        cx,
                    ));
                    title_for(backend).into()
                } else {
                    t::nav::all().into()
                }
            }
            LibraryFilter::Local => {
                sections.push(self.local_section(local, true, cx));
                t::common::local().into()
            }
        };

        v_flex().size_full().overflow_y_scrollbar().child(
            v_flex()
                .w_full()
                .px_8()
                .pt_6()
                .pb_8()
                .gap_8()
                .child(
                    page_header(title, cx).child(div().flex_1()).child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(gpui_kit::component::kbd::Kbd::new(
                                gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                                    "cmd-k"
                                } else {
                                    "ctrl-k"
                                })
                                .expect("valid keystroke"),
                            ))
                            .child(t::palette::search()),
                    ),
                )
                .children(sections),
        )
    }

    fn backend_section(
        &self,
        backend: &BackendStatus,
        title: String,
        instances: Vec<InstanceView>,
        show_header: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let color = fetch_state_color(&backend.fetch_state, cx);
        let state_text = match &backend.fetch_state {
            BackendFetchState::Fetched { .. } => None,
            other => Some(fetch_state_label(other)),
        };
        let header = h_flex()
            .items_center()
            .gap_2()
            .child(status_dot(color))
            .child(div().text_base().font_semibold().child(title.clone()))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(instances.len().to_string()),
            )
            .when_some(state_text, |this, text| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(color)
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(text),
                )
            });

        let body = if instances.is_empty() {
            let message = match &backend.fetch_state {
                BackendFetchState::Fetching => t::backends::refreshing_message().to_string(),
                BackendFetchState::Offline => t::backends::offline_message().to_string(),
                BackendFetchState::Error(error) => error.to_string(),
                BackendFetchState::NotFetched => t::backends::not_fetched_message().to_string(),
                BackendFetchState::Fetched { .. } => t::instances::no_instances_yet().to_string(),
            };
            empty_block(message, cx)
        } else {
            self.card_grid(instances, cx)
        };

        v_flex()
            .gap_3()
            .when(show_header, |this| this.child(header))
            .child(body)
    }

    fn local_section(
        &self,
        instances: Vec<InstanceView>,
        show_header: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let add = |cx: &mut Context<Self>| {
            Button::new("add-local-instance")
                .small()
                .outline()
                .icon(IconName::Plus)
                .label(t::local::add())
                .on_click(
                    cx.listener(|page, _, window, cx| page.open_create_local_modal(window, cx)),
                )
        };
        let header = h_flex()
            .items_center()
            .gap_2()
            .child(div().text_base().font_semibold().child(t::common::local()))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(instances.len().to_string()),
            )
            .child(div().flex_1())
            .child(add(cx));
        let body = if instances.is_empty() {
            empty_block(t::instances::local_empty_hint().to_string(), cx)
        } else {
            self.card_grid(instances, cx)
        };
        v_flex()
            .gap_3()
            .map(|this| {
                if show_header {
                    this.child(header)
                } else {
                    this.child(h_flex().justify_end().child(add(cx)))
                }
            })
            .child(body)
    }

    fn card_grid(&self, instances: Vec<InstanceView>, cx: &mut Context<Self>) -> gpui_kit::Div {
        let hide_usernames = self.data.settings.read(cx).settings.hide_usernames_in_cards;
        let accounts = self.data.accounts.read(cx).accounts.clone();
        let mut cards = Vec::with_capacity(instances.len());
        for instance in instances {
            cards.push(self.instance_card(instance, &accounts, hide_usernames, cx));
        }
        h_flex().flex_wrap().gap_4().children(cards)
    }

    fn instance_card(
        &self,
        instance: InstanceView,
        accounts: &[AccountView],
        hide_usernames: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let handle = instance.handle.clone();
        let sender = self.data.backend_sender.clone();
        let page = cx.entity().downgrade();
        let tasks = instance
            .status
            .active_tasks()
            .filter(|tasks| tasks.iter().any(|task| !task.finished))
            .map(|tasks| tasks.to_vec());

        v_flex()
            .id(SharedString::from(format!("card-{handle}")))
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .p_4()
            .gap_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().group_box)
            .hover(|style| style.border_color(cx.theme().muted_foreground.opacity(0.5)))
            .on_click(cx.listener({
                let handle = handle.clone();
                move |page, _, window, cx| {
                    page.navigate(Route::Instance(handle.clone()), window, cx)
                }
            }))
            .child(
                h_flex()
                    .flex_none()
                    .items_start()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .font_semibold()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(instance.display_name.to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(
                                        game_label(&instance).unwrap_or_else(|| "\u{00a0}".into()),
                                    ),
                            ),
                    )
                    .children(status_badge(&instance, cx)),
            )
            .child(div().flex_none().h_5().when(!hide_usernames, |this| {
                this.child(account_line(&instance, cx))
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .justify_center()
                    .gap_1()
                    .map(|this| match tasks {
                        Some(tasks) => this
                            .children(tasks.into_iter().find(|task| !task.finished).map(|task| {
                                crate::component::progress::task_progress_rows(&[task], cx)
                            }))
                            .when_some(instance.launch_after_install, |this, enabled| {
                                let handle = handle.clone();
                                let sender = sender.clone();
                                this.child(
                                    div()
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .child(
                                            Checkbox::new(SharedString::from(format!(
                                                "launch-after-install-{handle}"
                                            )))
                                            .checked(enabled)
                                            .small()
                                            .label(t::instances::launch_after_install())
                                            .on_click(move |_, _, _| {
                                                sender.send(
                                                    MessageToBackend::SetLaunchAfterInstall {
                                                        instance: handle.clone(),
                                                        enabled: !enabled,
                                                    },
                                                );
                                            }),
                                        ),
                                )
                            }),
                        None => this.when(!instance.optional_mod_sets.is_empty(), |this| {
                            this.child(mods_line(
                                &instance,
                                self.data.instances.clone(),
                                sender.clone(),
                                cx,
                            ))
                        }),
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().flex_1().min_w_0().child(primary_action(
                        &instance,
                        accounts,
                        sender.clone(),
                        page.clone(),
                        cx,
                    )))
                    .child(more_menu(
                        &instance,
                        page,
                        self.data.launcher_dir.clone(),
                        cx,
                    )),
            )
    }
}

pub(super) fn confirm_delete(
    page: &Entity<InstancesPage>,
    handle: InstanceHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let page = page.read(cx);
    let sender = page.data.backend_sender.clone();
    let name = page
        .find_instance(&handle, cx)
        .map(|instance| instance.display_name.to_string())
        .unwrap_or_default();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let sender = sender.clone();
        let handle = handle.clone();
        alert
            .title(t::instances::delete_title(name.clone()))
            .description(t::instances::delete_description())
            .ok_text(t::instances::delete())
            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
            .show_cancel(true)
            .cancel_text(t::common::cancel())
            .on_ok(move |_, _, _| {
                sender.send(MessageToBackend::DeleteInstance(handle.clone()));
                true
            })
    });
}

fn empty_block(message: String, cx: &App) -> gpui_kit::Div {
    div()
        .w_full()
        .p_6()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_dashed()
        .border_color(cx.theme().border)
        .flex()
        .justify_center()
        .child(muted(message, cx))
}

fn account_line(instance: &InstanceView, cx: &App) -> gpui_kit::Div {
    let blocked = instance.launch_blocked_reason.is_some();
    let username = instance
        .effective_account_username
        .as_ref()
        .filter(|_| !blocked)
        .map(|username| username.to_string());
    let name = if blocked {
        t::instances::add_account_card().to_string()
    } else {
        username
            .clone()
            .unwrap_or_else(|| t::instances::no_account_selected().to_string())
    };
    let provider = instance
        .effective_auth_provider
        .as_ref()
        .or(instance.auth_provider.as_ref())
        .map(provider_type_label)
        .unwrap_or_else(|| t::instances::any_provider().to_string());

    h_flex()
        .min_w_0()
        .gap_2()
        .items_center()
        .text_xs()
        .map(|this| match &username {
            Some(username) => this.child(super::common::avatar(username).xsmall()),
            None => this.child(
                div()
                    .size_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_dashed()
                    .border_color(cx.theme().border)
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::new(IconName::User).xsmall()),
            ),
        })
        .child(
            div()
                .min_w_0()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(if blocked {
                    cx.theme().warning
                } else {
                    cx.theme().foreground
                })
                .child(name),
        )
        .child(
            div()
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(if instance.account_override.is_some() {
                    format!("{provider} · {}", t::instances::override_short())
                } else {
                    provider
                }),
        )
}

fn mods_line(
    instance: &InstanceView,
    instances: Entity<InstanceEntries>,
    sender: BackendSender,
    cx: &App,
) -> impl IntoElement {
    let handle = instance.handle.clone();
    let sets = &instance.optional_mod_sets;
    let enabled = sets.iter().filter(|set| set.enabled).count();
    let enabled_names = sets
        .iter()
        .filter(|set| set.enabled)
        .map(|set| set.display_name.as_ref())
        .collect::<Vec<_>>()
        .join(", ");

    h_flex()
        .min_w_0()
        .gap_2()
        .items_center()
        .child(
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Popover::new(SharedString::from(format!("mods-{handle}")))
                        .trigger(
                            Button::new(SharedString::from(format!("mods-trigger-{handle}")))
                                .xsmall()
                                .outline()
                                .icon(IconName::Puzzle)
                                .label(t::mods::count(enabled, sets.len())),
                        )
                        .content(move |_, _, cx| mods_popover(&handle, &instances, &sender, cx)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if enabled_names.is_empty() {
                    t::mods::none_enabled().to_string()
                } else {
                    enabled_names
                }),
        )
}

fn mods_popover(
    handle: &InstanceHandle,
    instances: &Entity<InstanceEntries>,
    sender: &BackendSender,
    cx: &App,
) -> gpui_kit::Div {
    let Some(instance) = instances
        .read(cx)
        .entries
        .iter()
        .find(|instance| &instance.handle == handle)
        .cloned()
    else {
        return v_flex();
    };
    let locked = instance_busy(&instance.status);
    v_flex()
        .w(px(260.0))
        .gap_1()
        .child(
            div()
                .pb_1()
                .text_sm()
                .font_semibold()
                .child(t::instances::optional_mods_title()),
        )
        .children(
            instance
                .optional_mod_sets
                .iter()
                .map(|set| mod_set_switch(&instance, set, locked, sender.clone())),
        )
        .when(locked, |this| {
            this.child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t::mods::locked()),
            )
        })
}

pub(super) fn mod_set_switch(
    instance: &InstanceView,
    set: &launcher_bridge::OptionalModSetView,
    locked: bool,
    sender: BackendSender,
) -> gpui_kit::Div {
    let handle = instance.handle.clone();
    let set_id = set.set_id.to_string();
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_3()
        .py_1()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(set.display_name.to_string()),
        )
        .child(
            Switch::new(SharedString::from(format!("mod-set-{handle}-{set_id}")))
                .checked(set.enabled)
                .disabled(locked)
                .small()
                .on_click(move |enabled, _, _| {
                    sender.send(MessageToBackend::SetOptionalModSetEnabled {
                        instance: handle.clone(),
                        set_id: set_id.clone(),
                        enabled: *enabled,
                    });
                }),
        )
}

pub(super) fn start_add_required_account(
    page: &InstancesPage,
    provider: &launcher_auth::providers::AuthProviderConfig,
    cx: &App,
) {
    use launcher_auth::providers::AuthProviderConfig;
    let sender = &page.data.backend_sender;
    match provider {
        AuthProviderConfig::Offline(_) => {
            let nickname = page
                .offline_nickname_input
                .read(cx)
                .value()
                .trim()
                .to_string();
            let nickname = if nickname.is_empty() {
                "Player".to_string()
            } else {
                nickname
            };
            sender.send(MessageToBackend::SubmitOfflineNickname(nickname));
        }
        _ => sender.send(MessageToBackend::StartAddAccount(provider.clone())),
    }
}

/// The card's and the instance page's main button, which follows the status.
pub(super) fn primary_action(
    instance: &InstanceView,
    accounts: &[AccountView],
    sender: BackendSender,
    page: WeakEntity<InstancesPage>,
    _cx: &App,
) -> gpui_kit::AnyElement {
    let handle = instance.handle.clone();
    let id = |kind: &str| SharedString::from(format!("{kind}-{handle}"));
    let blocked = instance.launch_blocked_reason.is_some();
    let add_account = |kind: &str| {
        let provider = instance.auth_provider.clone();
        let page = page.clone();
        Button::new(id(kind))
            .w_full()
            .primary()
            .icon(IconName::UserPlus)
            .label(t::instances::add_account())
            .on_click(move |_, _, cx| {
                if let Some(provider) = provider.clone() {
                    let _ = page.update(cx, |page, cx| {
                        start_add_required_account(page, &provider, cx)
                    });
                }
            })
            .into_any_element()
    };
    let install = |label: &'static str| {
        let sender = sender.clone();
        let handle = handle.clone();
        Button::new(id("install"))
            .w_full()
            .primary()
            .icon(IconName::Download)
            .label(label)
            .on_click(move |_, _, _| {
                sender.send(MessageToBackend::InstallInstance {
                    handle: handle.clone(),
                    force_overwrite: false,
                    launch_after_install: Some(true),
                });
            })
            .into_any_element()
    };

    match &instance.status {
        InstanceLiveStatus::Installing { .. } => {
            let sender = sender.clone();
            Button::new(id("cancel"))
                .w_full()
                .outline()
                .icon(IconName::X)
                .label(t::common::cancel())
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::CancelInstall(handle.clone()))
                })
                .into_any_element()
        }
        InstanceLiveStatus::LaunchPreparing { .. } | InstanceLiveStatus::Running => {
            let running = matches!(instance.status, InstanceLiveStatus::Running);
            Button::new(id("kill"))
                .w_full()
                .outline()
                .icon(IconName::Square)
                .label(if running {
                    t::instances::kill()
                } else {
                    t::common::cancel()
                })
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::KillInstance(handle.clone()))
                })
                .into_any_element()
        }
        InstanceLiveStatus::NotInstalled | InstanceLiveStatus::Outdated => {
            if is_local(instance) {
                Button::new(id("unavailable"))
                    .w_full()
                    .label(t::instances::install())
                    .disabled(true)
                    .into_any_element()
            } else if blocked {
                add_account("add-account-install")
            } else if matches!(instance.status, InstanceLiveStatus::Outdated) {
                install(t::instances::update())
            } else {
                install(t::instances::install())
            }
        }
        InstanceLiveStatus::NeedsLauncherUpdate if !instance.locally_installed => {
            Button::new(id("needs-update"))
                .w_full()
                .label(t::instances::install())
                .disabled(true)
                .into_any_element()
        }
        InstanceLiveStatus::InstallFailed(_) => {
            let local = is_local(instance);
            Button::new(id("retry"))
                .w_full()
                .primary()
                .icon(IconName::RotateCcw)
                .label(t::common::retry())
                .on_click(move |_, _, _| {
                    if local {
                        sender.send(MessageToBackend::RetryCreateLocal(handle.clone()))
                    } else {
                        sender.send(MessageToBackend::InstallInstance {
                            handle: handle.clone(),
                            force_overwrite: false,
                            launch_after_install: Some(true),
                        })
                    }
                })
                .into_any_element()
        }
        _ if blocked => add_account("add-account"),
        _ => {
            let play = {
                let sender = sender.clone();
                let handle = handle.clone();
                Button::new(id("play"))
                    .primary()
                    .icon(IconName::Play)
                    .label(
                        if matches!(instance.status, InstanceLiveStatus::LaunchFailed(_)) {
                            t::instances::play_again()
                        } else {
                            t::instances::play()
                        },
                    )
                    .on_click(move |_, _, _| {
                        sender.send(MessageToBackend::Launch {
                            instance: handle.clone(),
                            account: None,
                        });
                    })
            };
            let others = compatible_accounts(instance, accounts);
            if others.len() < 2 {
                return play.w_full().into_any_element();
            }
            DropdownButton::new(id("play-split"))
                .w_full()
                .button(play.flex_1())
                .primary()
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu.label(t::instances::play_as());
                    for account in &others {
                        let sender = sender.clone();
                        let handle = handle.clone();
                        let key = account.key.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!(
                                "{} · {}",
                                account.data.user_info.username,
                                provider_type_label(&account.provider)
                            ))
                            .on_click(move |_, _, _| {
                                sender.send(MessageToBackend::Launch {
                                    instance: handle.clone(),
                                    account: Some(key.clone()),
                                });
                            }),
                        );
                    }
                    menu
                })
                .into_any_element()
        }
    }
}

fn more_menu(
    instance: &InstanceView,
    page: WeakEntity<InstancesPage>,
    launcher_dir: std::path::PathBuf,
    _cx: &App,
) -> impl IntoElement {
    let handle = instance.handle.clone();
    let folder = launcher_dir
        .join("instances")
        .join(instance.dir_name.as_ref());
    let can_delete = instance.locally_installed && !instance_busy(&instance.status);
    let has_folder = instance.locally_installed;
    Button::new(SharedString::from(format!("more-{handle}")))
        .outline()
        .icon(IconName::EllipsisVertical)
        .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let settings_page = page.clone();
            let settings_handle = handle.clone();
            let logs_page = page.clone();
            let logs_handle = handle.clone();
            let folder_page = page.clone();
            let folder = folder.clone();
            let delete_page = page.clone();
            let delete_handle = handle.clone();
            menu.item(
                PopupMenuItem::new(t::instances::tab_runtime())
                    .icon(IconName::Settings)
                    .on_click(move |_, window, cx| {
                        let _ = settings_page.update(cx, |page, cx| {
                            page.open_instance_tab(
                                settings_handle.clone(),
                                InstanceTab::Runtime,
                                window,
                                cx,
                            )
                        });
                    }),
            )
            .item(
                PopupMenuItem::new(t::instances::logs())
                    .icon(IconName::ScrollText)
                    .on_click(move |_, window, cx| {
                        let _ = logs_page.update(cx, |page, cx| {
                            page.open_instance_tab(
                                logs_handle.clone(),
                                InstanceTab::Logs,
                                window,
                                cx,
                            )
                        });
                    }),
            )
            .item(
                PopupMenuItem::new(t::instances::open_instance_folder())
                    .icon(IconName::FolderOpen)
                    .disabled(!has_folder)
                    .on_click(move |_, _, cx| {
                        if let Err(err) = open_path(&folder) {
                            let _ = folder_page.update(cx, |page, cx| {
                                page.data.notifications.update(cx, |entries, cx| {
                                    entries.push(
                                        NotificationLevel::Error,
                                        t::notifications::failed_open_instance_folder(
                                            err.to_string(),
                                        ),
                                        cx,
                                    );
                                });
                            });
                        }
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(t::instances::delete())
                    .icon(IconName::Trash)
                    .disabled(!can_delete)
                    .on_click(move |_, window, cx| {
                        if let Some(page) = delete_page.upgrade() {
                            confirm_delete(&page, delete_handle.clone(), window, cx);
                        }
                    }),
            )
        })
}
