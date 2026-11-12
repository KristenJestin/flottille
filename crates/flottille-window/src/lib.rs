//! Flottille's window: one more client of the daemon's socket, like the command line. It never
//! reads the store, never runs an agent and never decides anything on its own: it shows what the
//! daemon gives, as the daemon gives it.

pub mod actions;
pub mod add_project;
pub mod ask_dock;
pub mod attachments;
pub mod claude;
pub mod commands;
pub mod composer;
pub mod conventions;
pub mod conversation;
pub mod extensions;
pub mod glyphs;
pub mod history;
pub mod instance;
pub mod items;
pub mod leaves;
pub mod link;
pub mod links_card;
pub mod message_checks;
pub mod mode;
pub mod model;
pub mod open;
pub mod page;
pub mod picture_viewer;
pub mod programs;
pub mod project;
pub mod project_removal;
pub mod project_settings;
pub mod providers;
pub mod question_card;
pub mod queued;
pub mod rename;
pub mod repositories;
pub mod rules;
pub mod runs;
pub mod services;
pub mod session;
pub mod session_link;
pub mod session_options;
pub mod settings;
pub mod setup;
pub mod shell;
pub mod skills;
pub mod state;
pub mod steps;
pub mod subscriptions;
pub mod tasks;
pub mod to_end;
pub mod tree;
pub mod vault;
pub mod vault_ask;
pub mod waiting;
pub mod workspaces_folder;

use futures::StreamExt as _;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

/// The id of Flottille's own theme, as the daemon keeps the user's choice (`appearance.show`).
pub const OWN_THEME: &str = "flottille";

/// The id of the Omarchy desktop source, as the daemon keeps the user's choice.
pub const OMARCHY: &str = "omarchy";

/// What the window tells the kit about itself: its name, its own look, and the desktop described
/// by `env`, whose Omarchy theme it offers to follow.
pub fn identity(env: gpui_design_system::omarchy::Environment) -> gpui_design_system::Identity {
    gpui_design_system::Identity::new("Flottille")
        .themes([gpui_design_system::Theme::new(
            OWN_THEME,
            "Flottille",
            gpui_design_system::Palette::own,
        )])
        .desktop(gpui_design_system::omarchy::Omarchy::new(env))
}

/// Initialises the kit for the window.
pub fn init(cx: &mut gpui_kit::App) {
    init_with(gpui_design_system::omarchy::Environment::from_system(), cx);
}

/// [`init`] with the desktop described by `env` rather than read from the machine.
pub fn init_with(env: gpui_design_system::omarchy::Environment, cx: &mut gpui_kit::App) {
    gpui_design_system::init(identity(env), cx);
}

pub use instance::Instance;
pub use link::Connect;
pub use shell::Shell;

/// Opens the window and runs until it is closed. `connect` opens a connection to the daemon,
/// starting it when nobody listens. The window comes to the front when `instance` is asked to
/// raise it (`instance.rs`).
pub fn run(connect: Connect, instance: Option<Instance>) {
    gpui_kit::application()
        .with_assets(glyphs::Assets)
        .run(move |cx| {
            init(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("flottille".into()),
                icon: Some(icon()),
                ..Default::default()
            };
            let window = gpui_kit::open_window(options, cx, |window, cx| {
                // Closing the window ends the program (GPUI Fast differs from GPUI on
                // `on_window_closed`).
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                frame_counter(window, cx);
                if let Some(instance) = instance {
                    raise_on_request(instance, window, cx);
                }
                cx.new(|cx| Shell::new(connect, window, cx))
            });
            if let Err(error) = window {
                eprintln!("flottille: the window cannot open: {error}");
                cx.quit();
            }
        });
}

/// The window's icon, the 256 px PNG of the package, decoded once. GPUI uses it on X11 only; on
/// Wayland the desktop takes the icon from the entry the window's app id names.
fn icon() -> std::sync::Arc<image::RgbaImage> {
    const PNG: &[u8] =
        include_bytes!("../../../packaging/linux/icons/hicolor/256x256/apps/flottille.png");
    let decoded = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .expect("the icon committed in packaging/linux is a PNG");
    std::sync::Arc::new(decoded.into_rgba8())
}

/// Brings the window to the front each time another `flottille window` asks for it. Where the
/// desktop does not let a program take the focus (a Wayland compositor often only marks the
/// window as asking for attention), that is all that can be done.
pub fn raise_on_request(instance: Instance, window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    let (ask, mut asked) = futures::channel::mpsc::unbounded();
    instance.serve(move || {
        let _ = ask.unbounded_send(());
    });
    window
        .spawn(cx, async move |cx| {
            while asked.next().await.is_some() {
                if cx.update(|window, _| window.activate_window()).is_err() {
                    break;
                }
            }
        })
        .detach();
}

/// With the `frames` feature, the frames the window drew are printed every second on the standard
/// error (`frames <total> <in the last second>`), to measure it at rest.
#[cfg(feature = "frames")]
pub fn frame_counter(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window
        .spawn(cx, async move |cx| {
            let mut last = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let Ok(total) = cx.update(|window, _| {
                    window
                        .frame_duration_snapshot()
                        .draw_duration_histogram
                        .len()
                }) else {
                    break;
                };
                eprintln!("frames {total} {}", total - last);
                last = total;
            }
        })
        .detach();
}

#[cfg(not(feature = "frames"))]
pub fn frame_counter(_: &mut gpui_kit::Window, _: &mut gpui_kit::App) {}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_is_the_256_px_png_of_the_package() {
        let icon = super::icon();
        assert_eq!(icon.dimensions(), (256, 256));
        // The tile is light and opaque at its middle, transparent at its rounded corner.
        assert_eq!(icon.get_pixel(128, 20).0[3], 255);
        assert_eq!(icon.get_pixel(0, 0).0[3], 0);
    }
}
