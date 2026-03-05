use gpui::*;
use gpui_component::{
    ActiveTheme, Root, button::Button, h_flex, scroll::ScrollableElement, v_flex,
};

pub struct MainView;
impl Render for MainView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = div()
            .w_full()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xl()
            .child("Instances");

        let children = (0..1000_usize)
            .map(|i| {
                Button::new(("inst", i))
                    .w_10()
                    .h_10()
                    .on_click(move |_, _, _| {
                        println!("Clicked instance {i}");
                    })
            })
            .collect::<Vec<_>>();
        let grid = h_flex()
            .flex_wrap()
            .flex_1()
            .gap_2()
            .px_4()
            .py_2()
            .children(children);
        let grid_wrapper = div()
            .flex_1()
            .size_full()
            .overflow_y_scrollbar()
            .child(grid);
        v_flex()
            .gap_2()
            .size_full()
            .child(title)
            .child(grid_wrapper)
    }
}

fn main() {
    let app = Application::new();

    app.run(move |cx| {
        gpui_component::init(cx);

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|_| MainView);
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
