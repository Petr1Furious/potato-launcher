use gpui::{AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div};
use gpui_component::v_flex;

use crate::{component::notifications::notification_layer, entity::DataEntities, ui::LauncherUI};

pub struct LauncherRoot {
    ui: gpui::Entity<LauncherUI>,
    notifications: gpui::Entity<crate::entity::notification::NotificationEntries>,
    _notifications_subscription: gpui::Subscription,
}

impl LauncherRoot {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ui = cx.new(|cx| LauncherUI::new(data, window, cx));
        let notifications = data.notifications.clone();
        let _notifications_subscription = cx.subscribe(
            &notifications,
            |_, _, _: &crate::entity::notification::NotificationsUpdatedEvent, cx| cx.notify(),
        );
        Self {
            ui,
            notifications,
            _notifications_subscription,
        }
    }
}

impl Render for LauncherRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(v_flex().size_full().child(self.ui.clone()))
            .child(notification_layer(self.notifications.clone(), cx))
    }
}
