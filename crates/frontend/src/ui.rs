use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, Window};

use crate::{entity::DataEntities, pages::instances::InstancesPage};

pub struct LauncherUI {
    instances_page: Entity<InstancesPage>,
}

impl LauncherUI {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            instances_page: cx.new(|cx| InstancesPage::new(data, window, cx)),
        }
    }
}

impl Render for LauncherUI {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.instances_page.clone()
    }
}
