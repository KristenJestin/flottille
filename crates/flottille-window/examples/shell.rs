//! The window's shell in front of a running daemon, with a page or a drawer opened from the
//! command line: nothing needs a pointer to be seen, so it can be captured in a headless
//! compositor.
//!
//! ```text
//! cargo run -p flottille-window --example shell [--features frames] -- [--session <id>] [--tree] [--rail] [--running]
//! ```
//!
//! `--session` opens a session's page, `--tree` the left drawer (in a narrow window), `--rail` the
//! session page's drawer (in a window narrower than three columns), `--running` the project page's
//! "Running agents" tab. The daemon is found on
//! `$XDG_RUNTIME_DIR/flottille/daemon.sock`; it is not started.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use flottille_core::paths::Paths;
use flottille_ui::Icons;
use flottille_window::Shell;
use flottille_window::composer::Tab;
use flottille_window::shell::Selection;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

fn main() {
    let mut session = None;
    let (mut tree, mut rail, mut running) = (false, false, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session" => session = Some(args.next().expect("a session id")),
            "--tree" => tree = true,
            "--rail" => rail = true,
            "--running" => running = true,
            other => panic!("unknown argument {other}"),
        }
    }
    let paths = Paths::from_env().expect("the XDG folders");
    let socket = paths.socket();
    gpui_kit::application().with_assets(Icons).run(move |cx| {
        flottille_ui::init(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Maximized(bounds)),
            ..Default::default()
        };
        let connect = Arc::new(move || UnixStream::connect(&socket));
        let (window, shell) = gpui_kit::open_window(options, cx, |window, cx| {
            flottille_window::frame_counter(window, cx);
            cx.new(|cx| Shell::new(connect, window, cx))
        })
        .expect("a window");
        // Once the first read of the daemon has come.
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(800))
                .await;
            window
                .update(cx, |_, window, cx| {
                    shell.update(cx, |shell, cx| {
                        if let Some(id) = session {
                            shell.open(Selection::Session(id), window, cx);
                        }
                        if tree {
                            shell.open_tree(cx);
                        }
                        if running {
                            let open = shell.main().read(cx).project_page().cloned();
                            if let Some(page) = open {
                                let composer = page.read(cx).composer().clone();
                                composer.update(cx, |c, cx| c.select_tab(Tab::Running, cx));
                            }
                        }
                        let page = shell.main().read(cx).session_page().cloned();
                        if rail && let Some(page) = page {
                            let drawer = page.read(cx).drawer().clone();
                            drawer.update(cx, |d, cx| d.open(cx));
                        }
                    })
                })
                .ok();
        })
        .detach();
    });
}
