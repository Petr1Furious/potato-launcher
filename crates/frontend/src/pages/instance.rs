use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Sizable, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    scroll::ScrollableElement,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    App, ClipboardItem, Context, IntoElement, SharedString, Window, div, prelude::*, px,
};
use instance::java::{ZGC_MIN_JVM_VERSION, get_default_gc};
use launcher_auth::providers::{AuthProviderConfig, OfflineAuthProvider};
use launcher_bridge::{
    AccountView, InstanceLiveStatus, InstanceView, JvmGcOpts, MessageToBackend, NotificationLevel,
};
use launcher_i18n as t;

use super::common::{
    error_alert, game_label, group, group_box, instance_busy, is_local, muted, open_path,
    provider_label, row_divider, setting_row, status_badge, status_error,
};
use super::instances::{InstanceTab, InstancesPage, Route};
use super::library::{mod_set_switch, primary_action, start_add_required_account};
use crate::entity::java_resolve::JavaResolveState;

const CONTENT_MAX_WIDTH: f32 = 760.0;

impl InstancesPage {
    pub(super) fn render_instance(
        &mut self,
        instance: InstanceView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accounts = self.data.accounts.read(cx).accounts.clone();
        let has_mods = !instance.optional_mod_sets.is_empty();
        let mut tabs = vec![(InstanceTab::Overview, t::instances::tab_overview())];
        if has_mods {
            tabs.push((InstanceTab::Mods, t::instances::optional_mods_title()));
        }
        tabs.push((InstanceTab::Runtime, t::instances::tab_runtime()));
        tabs.push((InstanceTab::Account, t::accounts::account_section()));
        tabs.push((InstanceTab::Logs, t::instances::logs()));
        if !tabs.iter().any(|(tab, _)| *tab == self.instance_tab) {
            self.instance_tab = InstanceTab::Overview;
        }
        let selected = tabs
            .iter()
            .position(|(tab, _)| *tab == self.instance_tab)
            .unwrap_or(0);
        let tab_ids = tabs.iter().map(|(tab, _)| *tab).collect::<Vec<_>>();
        let page = cx.entity().downgrade();

        let body = match self.instance_tab {
            InstanceTab::Overview => self.overview_tab(&instance, cx).into_any_element(),
            InstanceTab::Mods => self.mods_tab(&instance, cx).into_any_element(),
            InstanceTab::Runtime => self.runtime_tab(&instance, cx).into_any_element(),
            InstanceTab::Account => self
                .account_tab(&instance, accounts.clone(), cx)
                .into_any_element(),
            InstanceTab::Logs => self
                .log_view
                .clone()
                .map(|view| view.into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
        };
        let logs = self.instance_tab == InstanceTab::Logs;

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .w_full()
                    .px_8()
                    .pt_5()
                    .gap_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("instance-back")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ArrowLeft)
                                    .on_click(cx.listener(|page, _, window, cx| {
                                        page.navigate(Route::Library, window, cx)
                                    })),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .gap_3()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xl()
                                                    .font_semibold()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(instance.display_name.to_string()),
                                            )
                                            .children(status_badge(&instance, cx)),
                                    )
                                    .child(muted(
                                        game_label(&instance).unwrap_or_else(|| "\u{00a0}".into()),
                                        cx,
                                    )),
                            )
                            .child(div().w(px(180.0)).child(primary_action(
                                &instance,
                                &accounts,
                                self.data.backend_sender.clone(),
                                page,
                                cx,
                            ))),
                    )
                    .child(
                        TabBar::new("instance-tabs")
                            .underline()
                            .selected_index(selected)
                            .children(tabs.iter().map(|(_, label)| Tab::new().label(*label)))
                            .on_click(cx.listener(move |page, index: &usize, window, cx| {
                                if let Some(tab) = tab_ids.get(*index) {
                                    page.set_instance_tab(*tab, window, cx);
                                }
                            })),
                    ),
            )
            .child(if logs {
                div()
                    .flex_1()
                    .min_h_0()
                    .px_8()
                    .py_5()
                    .child(body)
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(CONTENT_MAX_WIDTH))
                            .px_8()
                            .py_6()
                            .gap_6()
                            .child(body),
                    )
                    .into_any_element()
            })
    }

    fn overview_tab(&self, instance: &InstanceView, cx: &mut Context<Self>) -> gpui_kit::Div {
        let sender = self.data.backend_sender.clone();
        let handle = instance.handle.clone();
        let busy = matches!(instance.status, InstanceLiveStatus::Installing { .. });
        let folder = self
            .data
            .launcher_dir
            .join("instances")
            .join(instance.dir_name.as_ref());
        let tasks = instance
            .status
            .active_tasks()
            .filter(|tasks| tasks.iter().any(|task| !task.finished))
            .map(|tasks| tasks.to_vec());
        let page = cx.entity().downgrade();

        let maintenance = (!is_local(instance)).then(|| {
            let resync = {
                let sender = sender.clone();
                let handle = handle.clone();
                Button::new("instance-resync")
                    .small()
                    .outline()
                    .label(t::instances::resync())
                    .disabled(busy || !instance.locally_installed)
                    .on_click(move |_, _, _| {
                        sender.send(MessageToBackend::InstallInstance {
                            handle: handle.clone(),
                            force_overwrite: false,
                            launch_after_install: None,
                        })
                    })
            };
            let hard = {
                let sender = sender.clone();
                let handle = handle.clone();
                Button::new("instance-hard-resync")
                    .small()
                    .outline()
                    .label(t::instances::hard_resync())
                    .disabled(busy || !instance.locally_installed)
                    .on_click(move |_, _, _| {
                        sender.send(MessageToBackend::InstallInstance {
                            handle: handle.clone(),
                            force_overwrite: true,
                            launch_after_install: None,
                        })
                    })
            };
            group_box(cx)
                .child(setting_row(
                    t::instances::resync(),
                    Some(t::instances::resync_desc().into()),
                    resync,
                    cx,
                ))
                .child(row_divider(cx))
                .child(setting_row(
                    t::instances::hard_resync(),
                    Some(t::instances::hard_resync_desc().into()),
                    hard,
                    cx,
                ))
        });

        let failed_local = matches!(instance.status, InstanceLiveStatus::InstallFailed(_))
            && !instance.locally_installed
            && is_local(instance);
        let delete = if failed_local {
            let sender = sender.clone();
            let handle = handle.clone();
            Button::new("instance-remove")
                .small()
                .danger()
                .label(t::instances::remove())
                .on_click(move |_, _, _| {
                    sender.send(MessageToBackend::CancelInstall(handle.clone()))
                })
        } else {
            let handle = handle.clone();
            Button::new("instance-delete")
                .small()
                .danger()
                .icon(IconName::Trash)
                .label(t::instances::delete())
                .disabled(!instance.locally_installed || instance_busy(&instance.status))
                .on_click(move |_, window, cx| {
                    if let Some(page) = page.upgrade() {
                        super::library::confirm_delete(&page, handle.clone(), window, cx);
                    }
                })
        };

        let dir_name = instance.dir_name.to_string();
        let notifications = self.data.notifications.clone();
        let files = h_flex()
            .gap_1()
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted)
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(dir_name),
            )
            .child(
                Button::new("instance-copy-dir")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Copy)
                    .tooltip(t::instances::copy_path())
                    .on_click({
                        let folder = folder.clone();
                        let notifications = self.data.notifications.clone();
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                folder.to_string_lossy().to_string(),
                            ));
                            notifications.update(cx, |entries, cx| {
                                entries.push(
                                    NotificationLevel::Info,
                                    t::notifications::copied(),
                                    cx,
                                )
                            });
                        }
                    }),
            )
            .child(
                Button::new("instance-open-folder")
                    .ghost()
                    .xsmall()
                    .icon(IconName::FolderOpen)
                    .tooltip(t::instances::open_instance_folder())
                    .disabled(!instance.locally_installed)
                    .on_click(move |_, _, cx| {
                        if let Err(err) = open_path(&folder) {
                            notifications.update(cx, |entries, cx| {
                                entries.push(
                                    NotificationLevel::Error,
                                    t::notifications::failed_open_instance_folder(err.to_string()),
                                    cx,
                                );
                            });
                        }
                    }),
            );

        v_flex()
            .gap_6()
            .when_some(status_error(&instance.status), |this, error| {
                this.child(error_alert(error, cx))
            })
            .when_some(instance.launch_blocked_reason.clone(), |this, reason| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(reason.to_string()),
                )
            })
            .when_some(tasks, |this, tasks| {
                this.child(crate::component::progress::task_progress_rows(&tasks, cx))
            })
            .child(
                group(t::instances::details(), cx).child(
                    group_box(cx)
                        .child(setting_row(t::instances::folder(), None, files, cx))
                        .when_some(instance.required_java_version.clone(), |this, java| {
                            this.child(row_divider(cx)).child(setting_row(
                                t::instances::java_section(),
                                None,
                                div().text_sm().child(java),
                                cx,
                            ))
                        }),
                ),
            )
            .when_some(maintenance, |this, maintenance| {
                this.child(group(t::instances::maintenance(), cx).child(maintenance))
            })
            .child(
                group(t::instances::danger_zone(), cx).child(group_box(cx).child(setting_row(
                    t::instances::delete(),
                    Some(t::instances::delete_description().into()),
                    delete,
                    cx,
                ))),
            )
    }

    fn mods_tab(&self, instance: &InstanceView, cx: &mut Context<Self>) -> gpui_kit::Div {
        let locked = instance_busy(&instance.status);
        let sender = self.data.backend_sender.clone();
        let rows = instance
            .optional_mod_sets
            .iter()
            .enumerate()
            .flat_map(|(index, set)| {
                let mut rows = Vec::new();
                if index > 0 {
                    rows.push(row_divider(cx));
                }
                rows.push(div().px_4().py_1().child(mod_set_switch(
                    instance,
                    set,
                    locked,
                    sender.clone(),
                )));
                rows
            })
            .collect::<Vec<_>>();
        v_flex()
            .gap_2()
            .child(muted(t::instances::optional_mods_desc(), cx))
            .child(group_box(cx).children(rows))
            .when(locked, |this| this.child(muted(t::mods::locked(), cx)))
    }

    fn runtime_tab(&self, instance: &InstanceView, cx: &mut Context<Self>) -> gpui_kit::Div {
        let sender = self.data.backend_sender.clone();
        let handle = instance.handle.clone();
        let java_version = java_version(instance);
        let mut gc_choices = vec![JvmGcOpts::Default];
        if java_version >= ZGC_MIN_JVM_VERSION {
            gc_choices.push(JvmGcOpts::ZGC);
        }
        gc_choices.extend([JvmGcOpts::G1GC, JvmGcOpts::None]);
        let default_gc =
            get_default_gc(java_version, instance.effective_xmx_mb.unwrap_or(4096)).to_string();
        let gc_labels = gc_choices
            .iter()
            .map(|choice| match choice {
                JvmGcOpts::Default => t::instances::jvm_gc_opts_default(default_gc.clone()),
                JvmGcOpts::ZGC => "ZGC".to_string(),
                JvmGcOpts::G1GC => "G1GC".to_string(),
                JvmGcOpts::None => t::instances::jvm_gc_opts_none().to_string(),
            })
            .collect::<Vec<_>>();
        let gc_selected = gc_choices
            .iter()
            .position(|choice| *choice == instance.jvm_gc_opts)
            .unwrap_or(0);
        let gc = super::common::segmented("jvm-gc", gc_labels, gc_selected, {
            let sender = sender.clone();
            let handle = handle.clone();
            move |index, _, _| {
                if let Some(choice) = gc_choices.get(index) {
                    sender.send(MessageToBackend::SetInstanceJvmGcOpts {
                        instance: handle.clone(),
                        choice: *choice,
                    });
                }
            }
        });

        let jvm_opts = h_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.jvm_opts_input).small()),
            )
            .child(
                Button::new("save-jvm-opts")
                    .small()
                    .outline()
                    .label(t::instances::set_opts())
                    .on_click({
                        let input = self.jvm_opts_input.clone();
                        let sender = sender.clone();
                        let handle = handle.clone();
                        move |_, _, cx| {
                            let value = input.read(cx).value().trim().to_string();
                            sender.send(MessageToBackend::SetInstanceJvmOpts {
                                instance: handle.clone(),
                                opts: (!value.is_empty()).then_some(value),
                            });
                        }
                    }),
            );

        v_flex()
            .gap_6()
            .child(
                group(t::memory::title(), cx).child(group_box(cx).p_4().child(self.memory.clone())),
            )
            .child(
                group(t::instances::java_section(), cx).child(
                    group_box(cx)
                        .child(self.java_row(instance, cx))
                        .child(row_divider(cx))
                        .child(
                            v_flex()
                                .px_4()
                                .py_3()
                                .gap_2()
                                .child(div().text_sm().child(t::instances::jvm_gc_opts_title()))
                                .child(gc),
                        )
                        .child(row_divider(cx))
                        .child(
                            v_flex()
                                .px_4()
                                .py_3()
                                .gap_2()
                                .child(div().text_sm().child(t::instances::jvm_opts()))
                                .child(jvm_opts),
                        )
                        .children(native_glfw_row(instance, sender.clone(), cx)),
                ),
            )
    }

    fn java_row(&self, instance: &InstanceView, cx: &mut Context<Self>) -> gpui_kit::Div {
        let sender = self.data.backend_sender.clone();
        let handle = instance.handle.clone();
        if is_local(instance) && !instance.locally_installed {
            return v_flex()
                .px_4()
                .py_3()
                .child(muted(t::instances::java_install_required(), cx));
        }
        let resolve_state = self.data.java_resolve.read(cx).state(&handle);
        let hint = match resolve_state {
            Some(JavaResolveState::Resolving) => Some(t::instances::java_resolving()),
            Some(JavaResolveState::NotFound) => Some(t::instances::java_not_found()),
            _ => None,
        };
        let input = self.java_path_input.clone();
        v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_sm().child(t::instances::java_path()))
                    .when_some(instance.required_java_version.clone(), |this, version| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t::instances::required_java_version(version)),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&input).small()))
                    .child(
                        Button::new("java-set")
                            .small()
                            .outline()
                            .label(t::instances::set_opts())
                            .on_click({
                                let input = input.clone();
                                let sender = sender.clone();
                                let handle = handle.clone();
                                move |_, _, cx| {
                                    let value = input.read(cx).value().trim().to_string();
                                    sender.send(MessageToBackend::SetInstanceJavaPath {
                                        instance: handle.clone(),
                                        path: (!value.is_empty()).then_some(value),
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("java-find")
                            .small()
                            .ghost()
                            .icon(IconName::Search)
                            .tooltip(t::instances::java_find())
                            .on_click({
                                let sender = sender.clone();
                                let handle = handle.clone();
                                cx.listener(move |page, _, _, cx| {
                                    page.data.java_resolve.update(cx, |cache, cx| {
                                        cache.set_resolving(handle.clone(), cx);
                                    });
                                    sender.send(MessageToBackend::ResolveJavaPath(handle.clone()));
                                })
                            }),
                    )
                    .child(
                        Button::new("java-browse")
                            .small()
                            .ghost()
                            .icon(IconName::FolderOpen)
                            .tooltip(t::instances::java_browse())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let receiver = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                                    files: true,
                                    directories: false,
                                    multiple: false,
                                    prompt: None,
                                });
                                let sender = sender.clone();
                                let handle = handle.clone();
                                cx.spawn(async move |_, _| {
                                    if let Ok(Ok(Some(paths))) = receiver.await
                                        && let Some(path) = paths.into_iter().next()
                                    {
                                        sender.send(MessageToBackend::SetInstanceJavaPath {
                                            instance: handle.clone(),
                                            path: Some(path.to_string_lossy().to_string()),
                                        });
                                    }
                                })
                                .detach();
                            })),
                    ),
            )
            .when_some(hint, |this, hint| this.child(muted(hint, cx)))
    }

    fn account_tab(
        &self,
        instance: &InstanceView,
        accounts: Vec<AccountView>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let sender = self.data.backend_sender.clone();
        let Some(required) = instance.auth_provider.clone() else {
            let rows = account_rows(instance, &accounts, false, &sender, cx);
            return v_flex()
                .gap_2()
                .child(muted(t::instances::choose_account(), cx))
                .when(accounts.is_empty(), |this| {
                    this.child(
                        Button::new("add-offline-account")
                            .small()
                            .outline()
                            .icon(IconName::UserPlus)
                            .label(t::accounts::add_offline())
                            .on_click(cx.listener(|page, _, _, cx| {
                                start_add_required_account(
                                    page,
                                    &AuthProviderConfig::Offline(OfflineAuthProvider {}),
                                    cx,
                                );
                            })),
                    )
                })
                .when(!rows.is_empty(), |this| {
                    this.child(group_box(cx).children(rows))
                });
        };

        let matching = accounts
            .iter()
            .filter(|account| account.provider == required)
            .cloned()
            .collect::<Vec<_>>();
        let others = accounts
            .iter()
            .filter(|account| account.provider != required)
            .cloned()
            .collect::<Vec<_>>();
        let matching_rows = account_rows(instance, &matching, false, &sender, cx);
        let other_rows = account_rows(instance, &others, true, &sender, cx);
        let add_provider = required.clone();

        v_flex()
            .gap_6()
            .child(
                group(t::accounts::account_section(), cx)
                    .child(muted(
                        t::accounts::required_provider(provider_label(&required)),
                        cx,
                    ))
                    .when(!matching_rows.is_empty(), |this| {
                        this.child(group_box(cx).children(matching_rows))
                    })
                    .child(
                        h_flex().child(
                            Button::new("add-required-account")
                                .small()
                                .outline()
                                .icon(IconName::UserPlus)
                                .label(t::instances::add_account())
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    start_add_required_account(page, &add_provider, cx);
                                })),
                        ),
                    ),
            )
            .when(!other_rows.is_empty(), |this| {
                this.child(
                    group(t::accounts::account_override_section(), cx)
                        .child(muted(t::accounts::override_desc(), cx))
                        .child(group_box(cx).children(other_rows))
                        .when(instance.account_override.is_some(), |this| {
                            let sender = sender.clone();
                            let handle = instance.handle.clone();
                            this.child(
                                h_flex().child(
                                    Button::new("clear-override")
                                        .small()
                                        .outline()
                                        .label(t::accounts::clear_override())
                                        .on_click(move |_, _, _| {
                                            sender.send(
                                                MessageToBackend::SetInstanceAccountOverride {
                                                    instance: handle.clone(),
                                                    account: None,
                                                },
                                            );
                                        }),
                                ),
                            )
                        }),
                )
            })
    }
}

fn account_rows(
    instance: &InstanceView,
    accounts: &[AccountView],
    override_section: bool,
    sender: &launcher_bridge::BackendSender,
    cx: &App,
) -> Vec<gpui_kit::Div> {
    let mut rows = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        if index > 0 {
            rows.push(row_divider(cx));
        }
        let selected = account_is_selected(instance, account, override_section);
        let sender = sender.clone();
        let handle = instance.handle.clone();
        let key = account.key.clone();
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
                        "{}-account-{}-{}",
                        if override_section {
                            "override"
                        } else {
                            "select"
                        },
                        account.key.0,
                        account.key.1
                    )))
                    .small()
                    .map(|button| {
                        if selected {
                            button.ghost().icon(IconName::Check)
                        } else {
                            button.outline()
                        }
                    })
                    .label(if selected {
                        t::common::selected()
                    } else {
                        t::common::select()
                    })
                    .on_click(move |_, _, _| {
                        if selected {
                            return;
                        }
                        if override_section {
                            sender.send(MessageToBackend::SetInstanceAccountOverride {
                                instance: handle.clone(),
                                account: Some(key.clone()),
                            });
                        } else {
                            sender.send(MessageToBackend::SetInstanceSelectedAccount {
                                instance: handle.clone(),
                                account: Some(key.clone()),
                            });
                        }
                    }),
                ),
        );
    }
    rows
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

fn java_version(instance: &InstanceView) -> u64 {
    instance
        .required_java_version
        .as_deref()
        .and_then(|version| version.parse().ok())
        .unwrap_or(8)
}

#[cfg(target_os = "linux")]
fn native_glfw_row(
    instance: &InstanceView,
    sender: launcher_bridge::BackendSender,
    cx: &App,
) -> Vec<gpui_kit::Div> {
    let handle = instance.handle.clone();
    let enabled = instance
        .use_native_glfw
        .unwrap_or_else(launcher_build_config::use_native_glfw_default);
    vec![
        row_divider(cx),
        setting_row(
            t::instances::use_native_glfw_title(),
            Some(t::instances::use_native_glfw_desc().into()),
            gpui_kit::component::switch::Switch::new("native-glfw")
                .checked(enabled)
                .on_click(move |enabled, _, _| {
                    sender.send(MessageToBackend::SetInstanceUseNativeGlfw {
                        instance: handle.clone(),
                        enabled: *enabled,
                    });
                }),
            cx,
        ),
    ]
}

#[cfg(not(target_os = "linux"))]
fn native_glfw_row(
    _instance: &InstanceView,
    _sender: launcher_bridge::BackendSender,
    _cx: &App,
) -> Vec<gpui_kit::Div> {
    Vec::new()
}
