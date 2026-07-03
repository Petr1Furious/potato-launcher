use gpui::{App, div, prelude::*, px, relative};
use gpui_component::{ActiveTheme, v_flex};
use launcher_bridge::{InstanceTaskView, ProgressUnit};

pub fn format_progress(current: u64, total: u64, unit: ProgressUnit) -> String {
    let current = current.min(total);
    match unit {
        ProgressUnit::Bytes => {
            let mb_current = current as f32 / (1024.0 * 1024.0);
            let mb_total = total as f32 / (1024.0 * 1024.0);
            format!("{mb_current:.1} / {mb_total:.1} MB")
        }
        ProgressUnit::Items => format!("{current}/{total}"),
    }
}

fn task_fraction(task: &InstanceTaskView) -> f32 {
    if task.total == 0 {
        return 0.0;
    }
    (task.current as f32 / task.total as f32).clamp(0.0, 1.0)
}

fn task_row(task: &InstanceTaskView, cx: &App) -> gpui::Div {
    let label = if task.total > 0 {
        format!(
            "{} · {}",
            task.message,
            format_progress(task.current, task.total, task.unit)
        )
    } else {
        task.message.to_string()
    };
    let color = cx.theme().muted;
    let radius = cx.theme().radius;

    div()
        .w_full()
        .min_h(px(22.0))
        .relative()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .rounded(radius)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(relative(task_fraction(task)))
                .rounded(radius)
                .bg(color),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(radius)
                .border_1()
                .border_color(color),
        )
        .child(
            div()
                .relative()
                .text_xs()
                .text_center()
                .line_clamp(1)
                .child(label),
        )
}

pub fn task_progress_rows(tasks: &[InstanceTaskView], cx: &App) -> gpui::Div {
    v_flex().w_full().gap_1().children(
        tasks
            .iter()
            .filter(|task| !task.finished)
            .map(|task| task_row(task, cx)),
    )
}
