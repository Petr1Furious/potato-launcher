use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Sizable, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    h_flex,
    input::Input,
    scroll::ScrollableElement,
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    App, Context, Focusable as _, IntoElement, SharedString, Window, div, prelude::*, px,
};
use launcher_auth::providers::{
    AuthProviderConfig, ElyByAuthProvider, MicrosoftAuthProvider, TGAuthProvider,
};
use launcher_bridge::{
    BackendFetchState, LauncherSettingsView, MessageToBackend, NotificationLevel,
};
use launcher_build_config::launcher_name;
use launcher_i18n as t;
use url::Url;

use super::common::{
    backend_display_name, backend_display_names, fetch_state_color, fetch_state_label, group,
    group_box, muted, open_path, page_header, provider_label, row_divider, setting_row, status_dot,
};
use super::instances::InstancesPage;

const CONTENT_MAX_WIDTH: f32 = 720.0;

fn page(title: &'static str, cx: &App) -> gpui_kit::Div {
    v_flex()
        .w_full()
        .max_w(px(CONTENT_MAX_WIDTH))
        .px_8()
        .pt_6()
        .pb_8()
        .gap_6()
        .child(page_header(title, cx))
}

fn scroll(content: gpui_kit::Div) -> impl IntoElement {
    v_flex().size_full().overflow_y_scrollbar().child(content)
}

impl InstancesPage {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.data.settings.read(cx).settings.clone();
        let sender = self.data.backend_sender.clone();
        let save = {
            let sender = sender.clone();
            move |settings: LauncherSettingsView| {
                sender.send(MessageToBackend::SetLauncherSettings(settings))
            }
        };
        let launcher_dir = self.data.launcher_dir.clone();
        let notifications = self.data.notifications.clone();

        let hide_after_launch = {
            let save = save.clone();
            let settings = settings.clone();
            Switch::new("hide-after-launch")
                .checked(settings.hide_window_after_launch)
                .on_click(move |checked, _, _| {
                    save(LauncherSettingsView {
                        hide_window_after_launch: *checked,
                        ..settings.clone()
                    })
                })
        };
        let hide_usernames = {
            let save = save.clone();
            let settings = settings.clone();
            Switch::new("hide-usernames")
                .checked(settings.hide_usernames_in_cards)
                .on_click(move |checked, _, _| {
                    save(LauncherSettingsView {
                        hide_usernames_in_cards: *checked,
                        ..settings.clone()
                    })
                })
        };
        let languages = [
            ("en", t::settings::language_english()),
            ("ru", t::settings::language_russian()),
        ];
        let language = {
            let save = save.clone();
            let settings = settings.clone();
            let selected = languages
                .iter()
                .position(|(code, _)| settings.language == *code)
                .unwrap_or(0);
            super::common::segmented(
                "language",
                languages.iter().map(|(_, label)| label.to_string()),
                selected,
                move |index, _, _| {
                    if let Some((code, _)) = languages.get(index) {
                        save(LauncherSettingsView {
                            language: code.to_string(),
                            ..settings.clone()
                        })
                    }
                },
            )
        };
        let open_dir = Button::new("open-launcher-directory")
            .small()
            .outline()
            .icon(IconName::FolderOpen)
            .label(t::common::open())
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
            });
        let delete_all = Button::new("delete-all-launcher-data")
            .small()
            .danger()
            .label(t::settings::delete_all_data())
            .on_click(move |_, window, cx| {
                let sender = sender.clone();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let sender = sender.clone();
                    alert
                        .title(t::settings::delete_all_data_title())
                        .description(t::settings::delete_all_data_desc())
                        .ok_text(t::settings::delete_all_data())
                        .ok_variant(ButtonVariant::Danger)
                        .show_cancel(true)
                        .cancel_text(t::common::cancel())
                        .on_ok(move |_, _, _| {
                            sender.send(MessageToBackend::DeleteAllLauncherData);
                            true
                        })
                });
            });

        scroll(
            page(t::common::settings(), cx)
                .child(
                    group(t::settings::general(), cx).child(
                        group_box(cx)
                            .child(setting_row(
                                t::settings::hide_after_launch_title(),
                                Some(t::settings::hide_after_launch_desc().into()),
                                hide_after_launch,
                                cx,
                            ))
                            .child(row_divider(cx))
                            .child(setting_row(
                                t::settings::hide_usernames_title(),
                                Some(t::settings::hide_usernames_desc().into()),
                                hide_usernames,
                                cx,
                            ))
                            .child(row_divider(cx))
                            .child(setting_row(
                                t::settings::language_title(),
                                None,
                                language,
                                cx,
                            )),
                    ),
                )
                .child(
                    group(t::settings::storage(), cx).child(
                        group_box(cx)
                            .child(setting_row(
                                t::instances::open_launcher_directory(),
                                None,
                                open_dir,
                                cx,
                            ))
                            .child(row_divider(cx))
                            .child(setting_row(
                                t::settings::delete_all_data_title(),
                                Some(t::settings::delete_all_data_desc().into()),
                                delete_all,
                                cx,
                            )),
                    ),
                )
                .child(
                    group(t::settings::shortcuts(), cx).child(
                        group_box(cx)
                            .child(shortcut_row(t::palette::title(), "cmd-k", "ctrl-k", cx))
                            .child(row_divider(cx))
                            .child(shortcut_row(t::common::refresh(), "cmd-r", "ctrl-r", cx))
                            .child(row_divider(cx))
                            .child(shortcut_row(t::common::settings(), "cmd-,", "ctrl-,", cx)),
                    ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} {}",
                            launcher_name(),
                            launcher_build_config::version().unwrap_or("dev")
                        )),
                ),
        )
    }

    pub(super) fn render_accounts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let accounts = self.data.accounts.read(cx).accounts.clone();
        let sender = self.data.backend_sender.clone();
        let mut rows = Vec::new();
        for (index, account) in accounts.iter().enumerate() {
            if index > 0 {
                rows.push(row_divider(cx));
            }
            let key = account.key.clone();
            let sender = sender.clone();
            let name = account.data.user_info.username.clone();
            rows.push(
                h_flex()
                    .px_4()
                    .py_2p5()
                    .gap_3()
                    .items_center()
                    .child(super::common::avatar(&account.data.user_info.username).small())
                    .child(
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
                                    .child(account.data.user_info.username.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(provider_label(&account.provider)),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "remove-account-{}-{}",
                            key.0, key.1
                        )))
                        .small()
                        .ghost()
                        .icon(IconName::Trash)
                        .tooltip(t::common::remove())
                        .on_click(move |_, window, cx| {
                            let sender = sender.clone();
                            let key = key.clone();
                            confirm_remove(name.clone(), window, cx, move || {
                                sender.send(MessageToBackend::RemoveAccount(key.clone()))
                            });
                        }),
                    ),
            );
        }

        let add_provider = |id: &'static str, provider: AuthProviderConfig, label: &'static str| {
            let sender = sender.clone();
            Button::new(id)
                .small()
                .outline()
                .label(label)
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::StartAddAccount(provider.clone()))
                })
        };
        let offline = h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(200.0))
                    .child(Input::new(&self.offline_nickname_input).small()),
            )
            .child(
                Button::new("add-offline")
                    .small()
                    .outline()
                    .label(t::accounts::add_offline())
                    .on_click(cx.listener(|page, _, window, cx| page.add_offline(window, cx))),
            );
        let advanced = Button::new("add-account-advanced")
            .small()
            .outline()
            .label(t::accounts::advanced())
            .on_click(cx.listener(|page, _, window, cx| page.open_advanced_accounts(window, cx)));

        scroll(
            page(t::accounts::title(), cx)
                .map(|this| {
                    if rows.is_empty() {
                        this.child(muted(t::accounts::no_accounts_yet(), cx))
                    } else {
                        this.child(group_box(cx).children(rows))
                    }
                })
                .child(
                    group(t::accounts::add_account_section(), cx).child(
                        group_box(cx)
                            .child(setting_row(
                                t::providers::microsoft(),
                                None,
                                add_provider(
                                    "add-microsoft",
                                    AuthProviderConfig::Microsoft(MicrosoftAuthProvider {}),
                                    t::accounts::add_microsoft(),
                                ),
                                cx,
                            ))
                            .child(row_divider(cx))
                            .child(setting_row(t::providers::offline(), None, offline, cx))
                            .child(row_divider(cx))
                            .child(setting_row(
                                t::accounts::advanced_section(),
                                Some(t::accounts::advanced_desc().into()),
                                advanced,
                                cx,
                            )),
                    ),
                ),
        )
    }

    fn open_advanced_accounts(&self, window: &mut Window, cx: &mut App) {
        let sender = self.data.backend_sender.clone();
        let telegram = self.telegram_base_url_input.clone();
        let client_id = self.elyby_client_id_input.clone();
        let client_secret = self.elyby_client_secret_input.clone();
        let launcher = self.elyby_launcher_name_input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let telegram_sender = sender.clone();
            let elyby_sender = sender.clone();
            let telegram = telegram.clone();
            let client_id = client_id.clone();
            let client_secret = client_secret.clone();
            let launcher = launcher.clone();
            dialog
                .w(px(440.0))
                .title(t::accounts::advanced_section())
                .child(
                    v_flex()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(t::providers::telegram()),
                                )
                                .child(Input::new(&telegram).small())
                                .child(
                                    Button::new("add-telegram")
                                        .small()
                                        .outline()
                                        .label(t::accounts::add_telegram())
                                        .on_click({
                                            let telegram = telegram.clone();
                                            move |_, window, cx| {
                                                let auth_base_url =
                                                    telegram.read(cx).value().trim().to_string();
                                                if auth_base_url.is_empty() {
                                                    return;
                                                }
                                                telegram_sender.send(
                                                    MessageToBackend::StartAddAccount(
                                                        AuthProviderConfig::Telegram(
                                                            TGAuthProvider { auth_base_url },
                                                        ),
                                                    ),
                                                );
                                                window.close_dialog(cx);
                                            }
                                        }),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(div().text_sm().font_semibold().child(t::providers::elyby()))
                                .child(labeled(
                                    t::placeholders::client_id(),
                                    Input::new(&client_id).small(),
                                ))
                                .child(labeled(
                                    t::placeholders::client_secret(),
                                    Input::new(&client_secret).small(),
                                ))
                                .child(labeled(
                                    t::placeholders::launcher_name(),
                                    Input::new(&launcher).small(),
                                ))
                                .child(
                                    Button::new("add-elyby")
                                        .small()
                                        .outline()
                                        .label(t::accounts::add_elyby())
                                        .on_click(move |_, window, cx| {
                                            let id = client_id.read(cx).value().trim().to_string();
                                            let secret =
                                                client_secret.read(cx).value().trim().to_string();
                                            let name = launcher.read(cx).value().trim().to_string();
                                            if id.is_empty() || secret.is_empty() {
                                                return;
                                            }
                                            elyby_sender.send(MessageToBackend::StartAddAccount(
                                                AuthProviderConfig::ElyBy(ElyByAuthProvider::new(
                                                    id,
                                                    secret,
                                                    if name.is_empty() {
                                                        t::auth::default_launcher_name().to_string()
                                                    } else {
                                                        name
                                                    },
                                                )),
                                            ));
                                            window.close_dialog(cx);
                                        }),
                                ),
                        ),
                )
        });
    }

    pub(super) fn add_offline(&self, window: &mut Window, cx: &mut App) {
        let input = self.offline_nickname_input.clone();
        let value = input.read(cx).value().trim().to_string();
        if value.is_empty() {
            return;
        }
        self.data
            .backend_sender
            .send(MessageToBackend::SubmitOfflineNickname(value));
        input.update(cx, |input, cx| input.set_value("", window, cx));
    }

    pub(super) fn add_backend(&self, window: &mut Window, cx: &mut App) {
        let input = self.backend_url_input.clone();
        let value = input.read(cx).value().trim().to_string();
        let Ok(url) = Url::parse(&value) else {
            return;
        };
        if !matches!(url.scheme(), "http" | "https") {
            return;
        }
        self.data
            .backend_sender
            .send(MessageToBackend::AddBackendUrl(url));
        input.update(cx, |input, cx| input.set_value("", window, cx));
    }

    pub(super) fn render_backends(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let backends = self.data.backends.read(cx).backends.clone();
        let names = backend_display_names(&backends);
        let sender = self.data.backend_sender.clone();
        let value = self.backend_url_input.read(cx).value().trim().to_string();
        let valid = Url::parse(&value)
            .ok()
            .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
        let editing = self
            .backend_url_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let show_error = !value.is_empty() && !valid && !editing;

        let mut rows = Vec::new();
        for (index, backend) in backends.iter().enumerate() {
            if index > 0 {
                rows.push(row_divider(cx));
            }
            let title = names
                .get(backend.url.as_str())
                .cloned()
                .unwrap_or_else(|| backend_display_name(&backend.url, false));
            let color = fetch_state_color(&backend.fetch_state, cx);
            let origin = if backend.configured {
                None
            } else if backend.referenced_by_instances {
                Some(t::instances::used_by_installed())
            } else {
                Some(t::backends::discovered())
            };
            let state = (!matches!(backend.fetch_state, BackendFetchState::Fetched { .. }))
                .then(|| fetch_state_label(&backend.fetch_state));
            let name = title.clone();
            let url = backend.url.clone();
            let sender = sender.clone();
            let failed = matches!(
                backend.fetch_state,
                BackendFetchState::Error(_) | BackendFetchState::Offline
            );
            rows.push(
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .child(status_dot(color))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(title),
                                    )
                                    .children(origin.map(|origin| {
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(origin)
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(backend.url.as_str().to_string()),
                            )
                            .children(state.map(|state| {
                                div()
                                    .text_xs()
                                    .text_color(if failed {
                                        color
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .line_clamp(2)
                                    .child(state)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "remove-backend-{}",
                            url.as_str()
                        )))
                        .small()
                        .ghost()
                        .icon(IconName::Trash)
                        .tooltip(t::common::remove())
                        .disabled(!backend.configured)
                        .on_click(move |_, window, cx| {
                            let sender = sender.clone();
                            let url = url.clone();
                            confirm_remove(name.clone(), window, cx, move || {
                                sender.send(MessageToBackend::RemoveBackendUrl(url.clone()))
                            });
                        }),
                    ),
            );
        }

        let add = Button::new("add-backend")
            .small()
            .primary()
            .icon(IconName::Plus)
            .label(t::backends::add_backend())
            .disabled(!valid)
            .on_click(cx.listener(|page, _, window, cx| page.add_backend(window, cx)));

        scroll(
            page(t::backends::title(), cx)
                .child(group(t::backends::configured_backends(), cx).map(|this| {
                    if rows.is_empty() {
                        this.child(muted(t::backends::no_backends_yet(), cx))
                    } else {
                        this.child(group_box(cx).children(rows))
                    }
                }))
                .child(
                    group(t::backends::add_backend_section(), cx)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(Input::new(&self.backend_url_input).small()),
                                )
                                .child(add),
                        )
                        .when(show_error, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(t::backends::manifest_url_hint()),
                            )
                        }),
                ),
        )
    }
}

fn labeled(label: &'static str, field: impl IntoElement) -> gpui_kit::Div {
    v_flex()
        .gap_1()
        .child(div().text_xs().child(label))
        .child(field)
}

fn confirm_remove(
    name: impl Into<String>,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn() + 'static,
) {
    let name = name.into();
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        alert
            .title(t::common::remove_title(name.clone()))
            .ok_text(t::common::remove())
            .ok_variant(ButtonVariant::Danger)
            .show_cancel(true)
            .cancel_text(t::common::cancel())
            .on_ok(move |_, _, _| {
                on_ok();
                true
            })
    });
}

fn shortcut_row(
    label: &'static str,
    mac: &'static str,
    other: &'static str,
    cx: &App,
) -> gpui_kit::Div {
    let keys = if cfg!(target_os = "macos") {
        mac
    } else {
        other
    };
    setting_row(
        label,
        None,
        gpui_kit::component::kbd::Kbd::new(
            gpui_kit::Keystroke::parse(keys).expect("valid keystroke"),
        ),
        cx,
    )
}
