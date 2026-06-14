use gpui::{AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div};
use gpui_component::v_flex;

use crate::{
    component::notifications::notification_layer, entity::DataEntities, pages::update::UpdatePage,
    ui::LauncherUI,
};

pub struct LauncherRoot {
    ui: gpui::Entity<LauncherUI>,
    update_page: gpui::Entity<UpdatePage>,
    update: gpui::Entity<crate::entity::update::UpdateEntries>,
    notifications: gpui::Entity<crate::entity::notification::NotificationEntries>,
    _update_subscription: gpui::Subscription,
    _notifications_subscription: gpui::Subscription,
}

impl LauncherRoot {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ui = cx.new(|cx| LauncherUI::new(data, window, cx));
        let update_page = cx.new(|_| UpdatePage::new(data));
        let update = data.update.clone();
        let notifications = data.notifications.clone();
        let _update_subscription = cx.subscribe(
            &update,
            |_, _, _: &crate::entity::update::UpdateStateChangedEvent, cx| cx.notify(),
        );
        let _notifications_subscription = cx.subscribe(
            &notifications,
            |_, _, _: &crate::entity::notification::NotificationsUpdatedEvent, cx| cx.notify(),
        );
        Self {
            ui,
            update_page,
            update,
            notifications,
            _update_subscription,
            _notifications_subscription,
        }
    }
}

impl Render for LauncherRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_updating = self.update.read(cx).is_blocking();
        let content: gpui::AnyElement = if is_updating {
            v_flex()
                .size_full()
                .child(self.update_page.clone())
                .into_any_element()
        } else {
            v_flex()
                .size_full()
                .child(self.ui.clone())
                .into_any_element()
        };

        div()
            .relative()
            .size_full()
            .child(content)
            .child(notification_layer(self.notifications.clone(), cx))
    }
}
