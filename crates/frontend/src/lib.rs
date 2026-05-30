pub mod component;
pub mod entity;
pub mod pages;
pub mod processor;
pub mod root;
pub mod ui;

use std::path::PathBuf;

use entity::{
    DataEntities, account::AccountEntries, backend::BackendList, instance::InstanceEntries,
    notification::NotificationEntries, settings::LauncherSettingsEntries,
};
use gpui::{
    App, AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, px,
    size,
};
use gpui_component::Root;
use launcher_bridge::{BackendSender, FrontendReceiver};
use launcher_build_config::{launcher_app_id, launcher_name};
use processor::Processor;
use root::LauncherRoot;

actions!(launcher, [Quit]);

pub fn start(
    launcher_dir: PathBuf,
    backend_sender: BackendSender,
    mut receiver: FrontendReceiver,
) -> anyhow::Result<()> {
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        cx.bind_keys([
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-q", Quit, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("alt-f4", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx: &mut App| {
            cx.quit();
        });

        let instances = cx.new(|_| InstanceEntries::default());
        let backends = cx.new(|_| BackendList::default());
        let accounts = cx.new(|_| AccountEntries::default());
        let notifications = cx.new(|_| NotificationEntries::default());
        let settings = cx.new(|_| LauncherSettingsEntries::default());
        let data = DataEntities {
            instances,
            backends,
            accounts,
            notifications,
            settings,
            backend_sender,
            launcher_dir,
        };

        let window_data = data.clone();
        cx.open_window(
            WindowOptions {
                app_id: Some(launcher_app_id().into()),
                titlebar: Some(TitlebarOptions {
                    title: Some(launcher_name().into()),
                    appears_transparent: false,
                    ..Default::default()
                }),
                window_bounds: Some(initial_window_bounds(cx)),
                window_min_size: Some(gpui::size(gpui::px(720.0), gpui::px(420.0))),
                ..Default::default()
            },
            move |window, cx| {
                let root = cx.new(|cx| LauncherRoot::new(&window_data, window, cx));
                cx.new(|cx| Root::new(root, window, cx))
            },
        )
        .expect("failed to open main window");
        cx.activate(true);

        let mut processor = Processor::new(data);
        while let Some(message) = receiver.try_recv() {
            processor.process(message, cx);
        }

        cx.spawn(async move |cx| {
            while let Some(message) = receiver.recv().await {
                cx.update(|cx| processor.process(message, cx));
            }
        })
        .detach();
    });

    Ok(())
}

fn initial_window_bounds(cx: &App) -> WindowBounds {
    let size = cx
        .primary_display()
        .map(|display| {
            let display_size = display.bounds().size;
            let width = (display_size.width.as_f32() * 0.86)
                .min(display_size.width.as_f32() - 96.0)
                .clamp(720.0, 1280.0);
            let height = (display_size.height.as_f32() * 0.86)
                .min(display_size.height.as_f32() - 96.0)
                .clamp(420.0, 820.0);
            size(px(width), px(height))
        })
        .unwrap_or_else(|| size(px(1180.0), px(760.0)));

    WindowBounds::Windowed(Bounds::centered(None, size, cx))
}
