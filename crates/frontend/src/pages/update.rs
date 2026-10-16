use gpui_kit::component::{ActiveTheme, button::Button, h_flex, v_flex};
use gpui_kit::{Context, IntoElement, Render, Window, div, prelude::*, px};
use launcher_bridge::{MessageToBackend, UpdateStatusView};
use launcher_i18n as t;

use crate::entity::{DataEntities, update::UpdateState};

pub struct UpdatePage {
    data: DataEntities,
}

impl UpdatePage {
    pub fn new(data: &DataEntities) -> Self {
        Self { data: data.clone() }
    }
}

impl Render for UpdatePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = match &self.data.update.read(cx).state {
            UpdateState::Done => return div(),
            UpdateState::Blocking(s) => s.clone(),
        };

        let content = render_update_content(&status, &self.data, cx);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background)
            .child(content)
    }
}

fn render_update_content(
    status: &UpdateStatusView,
    data: &DataEntities,
    cx: &mut Context<UpdatePage>,
) -> gpui_kit::Div {
    let sender = data.backend_sender.clone();

    let proceed_button = Button::new("update-proceed")
        .label(t::update::proceed_to_launcher())
        .on_click(move |_, _, _| {
            sender.send(MessageToBackend::ProceedAfterUpdateFailure);
        });

    match status {
        UpdateStatusView::Checking => v_flex()
            .gap_3()
            .items_center()
            .child(status_label(t::update::checking())),

        UpdateStatusView::Downloading { current, total } => v_flex()
            .gap_3()
            .items_center()
            .w(px(340.0))
            .child(status_label(t::update::downloading()))
            .child(progress_bar(*current, *total, cx)),

        UpdateStatusView::Replacing => v_flex()
            .gap_3()
            .items_center()
            .child(status_label(t::update::replacing())),

        UpdateStatusView::Error { message, offline } => v_flex()
            .gap_4()
            .items_center()
            .w(px(340.0))
            .child(status_label(if *offline {
                t::update::error_offline().to_string()
            } else {
                t::update::error_generic(message.to_string())
            }))
            .child(proceed_button),

        UpdateStatusView::ReadOnly => v_flex()
            .gap_4()
            .items_center()
            .w(px(340.0))
            .child(status_label(t::update::error_read_only()))
            .child(proceed_button),

        UpdateStatusView::UpToDate | UpdateStatusView::NotApplicable => div(),
    }
}

fn status_label(text: impl Into<gpui_kit::SharedString>) -> gpui_kit::Div {
    div()
        .text_sm()
        .text_color(gpui_kit::Hsla::from(gpui_kit::rgb(0xAAAAAA)))
        .child(text.into())
}

fn progress_bar(current: u64, total: u64, cx: &mut Context<UpdatePage>) -> gpui_kit::Div {
    let fraction = if total > 0 {
        (current as f32 / total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mb_current = current as f32 / (1024.0 * 1024.0);
    let mb_total = total as f32 / (1024.0 * 1024.0);
    let label = format!("{:.1} / {:.1} MB", mb_current, mb_total);

    v_flex()
        .gap_1()
        .w_full()
        .child(
            div()
                .w_full()
                .h(px(6.0))
                .rounded_full()
                .bg(cx.theme().muted)
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(cx.theme().accent)
                        .w(gpui_kit::relative(fraction)),
                ),
        )
        .child(
            h_flex().justify_center().child(
                div()
                    .text_xs()
                    .text_color(gpui_kit::Hsla::from(gpui_kit::rgb(0x888888)))
                    .child(label),
            ),
        )
}
