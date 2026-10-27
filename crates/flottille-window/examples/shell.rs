//! The window's shell in front of a running daemon, with a page or a drawer opened from the
//! command line: nothing needs a pointer to be seen, so it can be captured in a headless
//! compositor.
//!
//! ```text
//! cargo run -p flottille-window --example shell [--features frames] -- [--session <id> [--agent <id>]] [--tree] [--rail] [--settings] [--menu place|provider|model|effort] [--new-workspace] [--add-procedure] [--edit-procedure <name>] [--run <workspace>] [--add-reference] [--size <w>x<h>]
//! ```
//!
//! `--session` opens a session's page (`--agent` then reads one of its sub-agents), `--tree` the
//! left drawer (in a narrow window), `--rail` the session page's drawer (in a window narrower than
//! three columns), `--settings` the settings page, `--menu` one of the project page composer's
//! pickers, `--new-workspace` the project page's new-workspace modal, `--add-procedure`,
//! `--edit-procedure`, `--run` and `--add-reference` the modals of the procedures and references
//! sections, `--size` the window's size. The daemon is found on
//! `$XDG_RUNTIME_DIR/flottille/daemon.sock`; it is not started.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use flottille_core::paths::Paths;
use flottille_ui::Icons;
use flottille_window::Shell;
use flottille_window::composer::Menu;
use flottille_window::shell::Selection;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

fn main() {
    let (mut session, mut agent) = (None, None);
    let (mut tree, mut rail, mut new_workspace, mut settings) = (false, false, false, false);
    let mut menu = None;
    let (mut add_procedure, mut add_reference) = (false, false);
    let (mut edit_procedure, mut run) = (None, None);
    let mut dimensions = (1280., 800.);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session" => session = Some(args.next().expect("a session id")),
            "--agent" => agent = Some(args.next().expect("an agent id")),
            "--tree" => tree = true,
            "--rail" => rail = true,
            "--new-workspace" => new_workspace = true,
            "--settings" => settings = true,
            "--add-procedure" => add_procedure = true,
            "--add-reference" => add_reference = true,
            "--edit-procedure" => edit_procedure = Some(args.next().expect("a procedure")),
            "--run" => run = Some(args.next().expect("a workspace")),
            "--size" => {
                let size = args.next().expect("a size like 1280x1400");
                let (w, h) = size.split_once('x').expect("WxH");
                dimensions = (w.parse().expect("a width"), h.parse().expect("a height"));
            }
            "--menu" => {
                menu = Some(match args.next().as_deref() {
                    Some("place") => Menu::Place,
                    Some("provider") => Menu::Provider,
                    Some("model") => Menu::Model,
                    Some("effort") => Menu::Effort,
                    other => panic!("no picker {other:?}"),
                })
            }
            other => panic!("unknown argument {other}"),
        }
    }
    let paths = Paths::from_env().expect("the XDG folders");
    let socket = paths.socket();
    gpui_kit::application().with_assets(Icons).run(move |cx| {
        flottille_ui::init(cx);
        let bounds = Bounds::centered(None, size(px(dimensions.0), px(dimensions.1)), cx);
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
                        if settings {
                            shell.open(Selection::Settings, window, cx);
                        }
                        if tree {
                            shell.open_tree(cx);
                        }
                        if let Some(menu) = menu {
                            let open = shell.main().read(cx).project_page().cloned();
                            if let Some(page) = open {
                                let composer = page.read(cx).composer().clone();
                                composer.update(cx, |c, cx| c.open_menu(menu, cx));
                            }
                        }
                        if new_workspace {
                            let open = shell.main().read(cx).project_page().cloned();
                            if let Some(page) = open {
                                let name = page.read(cx).name().to_owned();
                                if let Some(form) = shell.new_workspace_of(&name).cloned() {
                                    form.update(cx, |f, cx| f.open(window, cx));
                                }
                            }
                        }
                        if let Some(page) = shell.main().read(cx).project_page().cloned() {
                            let name = page.read(cx).name().to_owned();
                            if add_procedure {
                                let section = page.read(cx).procedures().clone();
                                section.update(cx, |s, cx| s.add(cx));
                            }
                            if let Some(procedure) = &edit_procedure {
                                let section = page.read(cx).procedures().clone();
                                section.update(cx, |s, cx| s.edit(procedure, cx));
                            }
                            if add_reference {
                                let section = page.read(cx).references().clone();
                                section.update(cx, |s, cx| s.add(window, cx));
                            }
                            if let Some(workspace) = &run {
                                shell.run_procedure(&name, workspace, cx);
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
            // A sub-agent read once the page knows its tree.
            if let Some(agent) = agent {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                window
                    .update(cx, |_, _, cx| {
                        let page = shell.read(cx).main().read(cx).session_page().cloned();
                        if let Some(page) = page {
                            page.update(cx, |page, cx| page.select(&agent, cx));
                        }
                    })
                    .ok();
            }
        })
        .detach();
    });
}
