//! Flottille's window: one more client of the daemon's socket, like the command line. It never
//! reads the store, never runs an agent and never decides anything on its own: it shows what the
//! daemon gives, as the daemon gives it.

pub mod actions;
pub mod composer;
pub mod conversation;
pub mod history;
pub mod link;
pub mod model;
pub mod open;
pub mod page;
pub mod project;
pub mod providers;
pub mod session;
pub mod session_link;
pub mod settings;
pub mod shell;
pub mod tree;

use flottille_ui::Icons;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

pub use link::Connect;
pub use shell::Shell;

/// Opens the window and runs until it is closed. `connect` opens a connection to the daemon,
/// starting it when nobody listens.
pub fn run(connect: Connect) {
    gpui_kit::application().with_assets(Icons).run(move |cx| {
        flottille_ui::init(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("flottille".into()),
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
            cx.new(|cx| Shell::new(connect, window, cx))
        });
        if let Err(error) = window {
            eprintln!("flottille: the window cannot open: {error}");
            cx.quit();
        }
    });
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
