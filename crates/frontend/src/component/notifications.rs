use gpui::{Context, IntoElement, ParentElement, Styled, div, px};
use gpui_component::{ActiveTheme, StyledExt, button::Button, v_flex};
use launcher_bridge::NotificationLevel;

use crate::entity::notification::{NotificationEntries, NotificationEntry};

pub fn notification_layer(
    notifications: gpui::Entity<NotificationEntries>,
    cx: &mut Context<crate::root::LauncherRoot>,
) -> impl IntoElement {
    let entries = notifications.read(cx).entries.clone();
    v_flex()
        .absolute()
        .top_4()
        .right_4()
        .w(px(360.0))
        .gap_2()
        .children(
            entries
                .into_iter()
                .map(|entry| notification_card(entry, notifications.clone(), cx)),
        )
}

fn notification_card(
    entry: NotificationEntry,
    notifications: gpui::Entity<NotificationEntries>,
    cx: &mut Context<crate::root::LauncherRoot>,
) -> gpui::Div {
    let color = match entry.level {
        NotificationLevel::Error => cx.theme().red,
        NotificationLevel::Warning => cx.theme().yellow,
        NotificationLevel::Success => cx.theme().green,
        NotificationLevel::Info => cx.theme().blue,
    };
    let title = match entry.level {
        NotificationLevel::Error => "Error",
        NotificationLevel::Warning => "Warning",
        NotificationLevel::Success => "Success",
        NotificationLevel::Info => "Info",
    };

    div()
        .p_3()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(color)
        .bg(cx.theme().popover)
        .shadow_lg()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(div().font_semibold().child(title))
                        .child(
                            Button::new(format!("dismiss-notification-{}", entry.id))
                                .label("Dismiss")
                                .on_click(move |_, _, cx| {
                                    notifications
                                        .update(cx, |entries, cx| entries.dismiss(entry.id, cx));
                                }),
                        ),
                )
                .child(div().text_sm().child(entry.message)),
        )
}
