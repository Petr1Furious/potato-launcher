use std::collections::HashSet;

use gpui_kit::component::{
    ActiveTheme, Disableable as _, IndexPath, StyledExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::Input,
    select::{Select, SelectDelegate, SelectEvent, SelectItem},
    skeleton::Skeleton,
    text::TextView,
    v_flex,
};
use gpui_kit::{
    App, Context, ImageSource, IntoElement, MouseButton, ObjectFit, SharedString, Window, div, img,
    prelude::*, px, rems,
};
use instance::storage::allocate_local_dir_name;
use launcher_auth::flow::AuthMessage;
use launcher_bridge::{AuthPromptContext, LocalLoader, MessageToBackend};
use launcher_i18n as t;

use super::instances::InstancesPage;

fn modal_shell(title: &'static str, cx: &App) -> gpui_kit::Div {
    v_flex()
        .w(px(440.0))
        .gap_4()
        .p_6()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().text_lg().font_semibold().child(title))
}

fn modal_overlay(cx: &App) -> gpui_kit::Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(96.0))
        .bg(cx.theme().overlay)
}

impl InstancesPage {
    pub(super) fn open_create_local_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_create_local_modal = true;
        self.create_local_show_snapshots = false;
        self.create_local_loader = LocalLoader::Vanilla;
        self.create_local_name_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.data
            .backend_sender
            .send(MessageToBackend::FetchLocalCreateVersions);
        self.data.local_create.update(cx, |entries, cx| {
            entries.set_minecraft_loading(cx);
        });
        cx.notify();
    }

    pub(super) fn on_minecraft_version_selected(
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
            .map(|(handle, _)| SharedString::from(handle.as_str()))
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

    pub(super) fn create_local_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
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
        let form_issue = create_local_form_issue(self, cx);
        let name_entered = !self
            .create_local_name_input
            .read(cx)
            .value()
            .trim()
            .is_empty();
        let form_valid = form_issue.is_none();
        let folder_preview = {
            let name = self
                .create_local_name_input
                .read(cx)
                .value()
                .trim()
                .to_string();
            (!name.is_empty()).then(|| {
                let taken = instances
                    .iter()
                    .map(|instance| instance.dir_name.as_ref())
                    .collect::<HashSet<_>>();
                allocate_local_dir_name(&taken, &name)
            })
        };

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

        modal_overlay(cx)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|page, _, _, cx| {
                    page.show_create_local_modal = false;
                    cx.notify();
                }),
            )
            .child(
                modal_shell(t::local::create_title(), cx)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().child(t::local::instance_name()))
                            .child(Input::new(&name_input))
                            .when_some(folder_preview.clone(), |this, dir_name| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(t::local::folder_name_preview(dir_name)),
                                )
                            }),
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
                            .child(h_flex().child(loader_toggle(selected_loader, cx))),
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
                                    .primary()
                                    .label(t::local::create())
                                    .disabled(
                                        !form_valid || minecraft_loading || minecraft_has_error,
                                    )
                                    .on_click({
                                        let sender = sender.clone();
                                        cx.listener(move |page, _, _, cx| {
                                            if create_local_form_issue(page, cx).is_some() {
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
                    .when_some(form_issue.filter(|_| name_entered), |this, hint| {
                        this.child(div().text_sm().text_color(cx.theme().danger).child(hint))
                    }),
            )
    }

    pub(super) fn auth_modal(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let auth_session = self.data.auth.read(cx).clone();
        let Some(message) = auth_session.message.clone() else {
            return div();
        };
        let instances = self.data.instances.read(cx).entries.clone();
        let sender = self.data.backend_sender.clone();
        let (url, instruction, device_code) = match &message {
            AuthMessage::Link { url } => {
                (url.clone(), t::auth::continue_in_browser(url.clone()), None)
            }
            AuthMessage::LinkCode { url, code } => (
                url.clone(),
                t::auth::continue_in_browser_with_code(url.clone(), code.clone()),
                Some(code.clone()),
            ),
        };
        let context_hint = auth_session.context.as_ref().map(|context| match context {
            AuthPromptContext::AddAccount => t::accounts::add_account_section().to_string(),
            AuthPromptContext::Instance { handle } => instances
                .iter()
                .find(|entry| &entry.handle == handle)
                .map(|entry| t::auth::launch_context(entry.display_name.to_string()))
                .unwrap_or_else(|| t::auth::launch_context(handle.to_string())),
        });

        modal_overlay(cx).child(
            modal_shell(t::auth::authorization(), cx)
                .when_some(context_hint, |this, hint| {
                    this.child(TextView::markdown("auth-context", hint).selectable(true))
                })
                .child(TextView::markdown("auth-instruction", instruction).selectable(true))
                .when_some(device_code, |this, code| {
                    this.child(
                        TextView::markdown("auth-device-code", t::auth::device_code(code))
                            .selectable(true),
                    )
                })
                .when_some(auth_session.qr_image.clone(), |this, qr_image| {
                    let qr_size = qr_image.size(0);
                    let w = px(u32::from(qr_size.width) as f32);
                    let h = px(u32::from(qr_size.height) as f32);
                    this.child(
                        div().flex().justify_center().child(
                            div()
                                .p(px(4.0))
                                .bg(gpui_kit::white())
                                .rounded(cx.theme().radius)
                                .child(
                                    img(ImageSource::Render(qr_image))
                                        .object_fit(ObjectFit::Fill)
                                        .w(w)
                                        .h(h),
                                ),
                        ),
                    )
                })
                .child(
                    Button::new("auth-open-link")
                        .primary()
                        .icon(gpui_kit::assets::IconName::ExternalLink)
                        .label(t::auth::open_link())
                        .on_click({
                            let url = url.clone();
                            move |_, _, _| {
                                let _ = open::that(&url);
                            }
                        }),
                )
                .child(
                    h_flex().justify_end().child(
                        Button::new("auth-cancel")
                            .label(t::common::cancel())
                            .on_click(move |_, _, _| {
                                sender.send(MessageToBackend::CancelAuth);
                            }),
                    ),
                ),
        )
    }
}

const LOADERS: [LocalLoader; 4] = [
    LocalLoader::Vanilla,
    LocalLoader::Fabric,
    LocalLoader::Forge,
    LocalLoader::Neoforge,
];

fn loader_toggle(selected: LocalLoader, cx: &mut Context<InstancesPage>) -> impl IntoElement {
    let labels = LOADERS.iter().map(|loader| {
        match loader {
            LocalLoader::Vanilla => t::local::loader_vanilla(),
            LocalLoader::Fabric => t::local::loader_fabric(),
            LocalLoader::Forge => t::local::loader_forge(),
            LocalLoader::Neoforge => t::local::loader_neoforge(),
        }
        .to_string()
    });
    let selected = LOADERS
        .iter()
        .position(|loader| *loader == selected)
        .unwrap_or(0);
    let page = cx.entity().downgrade();
    super::common::segmented(
        "create-local-loader",
        labels,
        selected,
        move |index, _, cx| {
            let loader = LOADERS[index];
            let _ = page.update(cx, |page, cx| {
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
            });
        },
    )
}

fn create_local_form_issue(page: &InstancesPage, cx: &App) -> Option<String> {
    let name = page
        .create_local_name_input
        .read(cx)
        .value()
        .trim()
        .to_string();
    if name.is_empty() {
        return Some(t::notifications::local_instance_name_empty().to_string());
    }

    if page
        .create_local_mc_version_select
        .read(cx)
        .selected_value()
        .is_none()
    {
        return Some(t::local::select_minecraft_version().to_string());
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
pub struct VersionList {
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

    fn perform_search(
        &mut self,
        query: &str,
        _window: &mut Window,
        _: &mut App,
    ) -> gpui_kit::Task<()> {
        let lower_query = query.to_lowercase();
        self.matched_versions = self
            .versions
            .iter()
            .filter(|item| item.to_lowercase().starts_with(&lower_query))
            .cloned()
            .collect();
        gpui_kit::Task::ready(())
    }
}
