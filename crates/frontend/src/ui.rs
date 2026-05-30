use gpui::{Context, IntoElement, Render, Window, div, prelude::*};
use gpui_component::{ActiveTheme, Disableable, button::Button, h_flex, v_flex};
use launcher_i18n as t;

use crate::{entity::DataEntities, pages::instances::InstancesPage};
use launcher_bridge::{BackendFetchState, MessageToBackend};

pub struct LauncherUI {
    data: DataEntities,
    instances_page: gpui::Entity<InstancesPage>,
}

impl LauncherUI {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instances_page = cx.new(|cx| InstancesPage::new(data, window, cx));
        Self {
            data: data.clone(),
            instances_page,
        }
    }
}

impl Render for LauncherUI {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let backends = self.data.backends.read(cx).backends.clone();
        let sender = self.data.backend_sender.clone();
        let refreshing = backends
            .iter()
            .any(|backend| matches!(backend.fetch_state, BackendFetchState::Fetching));

        let refresh = Button::new("refresh")
            .label(if refreshing {
                t::common::refreshing()
            } else {
                t::common::refresh()
            })
            .disabled(refreshing)
            .on_click({
                let sender = sender.clone();
                move |_, _, _| sender.send(MessageToBackend::Refresh)
            });
        let backends_button = Button::new("open-backends")
            .label(t::nav::configure_backends())
            .on_click({
                let instances_page = self.instances_page.clone();
                move |_, _, cx| {
                    instances_page.update(cx, |page, cx| page.open_backend_settings(cx));
                }
            });
        let accounts = Button::new("open-accounts")
            .label(t::nav::accounts())
            .on_click({
                let instances_page = self.instances_page.clone();
                move |_, _, cx| {
                    instances_page.update(cx, |page, cx| page.open_accounts_panel(cx));
                }
            });
        let settings = Button::new("open-settings")
            .label(t::common::settings())
            .on_click({
                let instances_page = self.instances_page.clone();
                move |_, _, cx| {
                    instances_page.update(cx, |page, cx| page.open_global_settings(cx));
                }
            });

        v_flex()
            .size_full()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.instances_page.clone()),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .p_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(settings)
                            .child(backends_button)
                            .child(accounts)
                            .child(refresh),
                    ),
            )
    }
}
