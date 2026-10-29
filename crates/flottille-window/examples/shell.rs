//! The window's shell in front of a running daemon, with a page or a drawer opened from the
//! command line: nothing needs a pointer to be seen, so it can be captured in a headless
//! compositor.
//!
//! ```text
//! cargo run -p flottille-window --example shell [--features frames] -- [--project <name>] [--add-project <folder>] [--session <id> [--agent <id>]] [--tree] [--rail] [--settings] [--menu place|provider|model|effort] [--new-workspace] [--add-command] [--edit-command <name>] [--add-skill] [--edit-skill <name>] [--slash <text>] [--attach <picture>] [--end] [--ended <project>] [--project-settings <project>] [--size <w>x<h>]
//! ```
//!
//! `--project` opens a project's page, `--add-project` the "Add a project" modal with the
//! repositories detected in a folder, `--session` opens a session's page (`--agent` then reads one of its sub-agents), `--tree` the
//! left drawer (in a narrow window), `--rail` the session page's drawer (in a window narrower than
//! three columns), `--settings` the settings page, `--menu` one of the project page composer's
//! pickers, `--new-workspace` the project page's new-workspace modal, `--add-command`,
//! `--edit-command`, `--add-skill` and `--edit-skill` the modals of the Commands and Skills
//! sections, `--slash` types a text in the open session's composer (its `/` list), `--attach` attaches a picture file to it, `--end` opens the open
//! session's "End the session?" confirmation, `--ended` unfolds a project's "Ended" group of the
//! left tree, `--project-settings` a project's settings page, `--size` the window's size. The daemon is found on
//! `$XDG_RUNTIME_DIR/flottille/daemon.sock`; it is not started.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use flottille_core::paths::Paths;
use flottille_ui::Icons;
use flottille_window::Shell;
use flottille_window::actions::Command;
use flottille_window::composer::Menu;
use flottille_window::shell::Selection;
use gpui_kit::{AppContext as _, Bounds, Focusable as _, WindowBounds, WindowOptions, px, size};

fn main() {
    let (mut session, mut agent) = (None, None);
    let (mut tree, mut rail, mut new_workspace, mut settings) = (false, false, false, false);
    let (mut menu, mut project_settings) = (None, None);
    let (mut project, mut add_project) = (None, None);
    let (mut add_command, mut add_skill) = (false, false);
    let (mut edit_command, mut edit_skill, mut slash, mut attach) = (None, None, None, None);
    let (mut end, mut ended) = (false, None);
    let mut dimensions = (1280., 800.);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session" => session = Some(args.next().expect("a session id")),
            "--project" => project = Some(args.next().expect("a project")),
            "--add-project" => add_project = Some(args.next().expect("a folder")),
            "--agent" => agent = Some(args.next().expect("an agent id")),
            "--tree" => tree = true,
            "--rail" => rail = true,
            "--new-workspace" => new_workspace = true,
            "--settings" => settings = true,
            "--project-settings" => project_settings = Some(args.next().expect("a project")),
            "--add-command" => add_command = true,
            "--add-skill" => add_skill = true,
            "--edit-command" => edit_command = Some(args.next().expect("a command")),
            "--edit-skill" => edit_skill = Some(args.next().expect("a skill")),
            "--slash" => slash = Some(args.next().expect("what the reply's field holds")),
            "--attach" => attach = Some(args.next().expect("a picture file")),
            "--end" => end = true,
            "--ended" => ended = Some(args.next().expect("a project")),
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
                        if let Some(name) = project.clone() {
                            shell.open(Selection::Project(name), window, cx);
                        }
                        if let Some(folder) = add_project.clone() {
                            shell.add_project(window, cx);
                            let form = shell.add_project_form().clone();
                            form.update(cx, |form, cx| {
                                form.set_folder(folder.clone().into(), window, cx)
                            });
                            shell.perform(Command::DetectProject { folder, name: None });
                        }
                        if settings {
                            shell.open(Selection::Settings, window, cx);
                        }
                        if let Some(name) = project_settings.clone() {
                            shell.open(Selection::ProjectSettings(name), window, cx);
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
                        let page = shell.main().read(cx).session_page().cloned();
                        if let (Some(text), Some(page)) = (&slash, &page)
                            && let Some(reply) = page.read(cx).reply().cloned()
                        {
                            let field = reply.read(cx).field().clone();
                            field.update(cx, |f, cx| f.set_value(text.clone(), window, cx));
                            reply.update(cx, |r, cx| r.typed(text, cx));
                            let focus = field.read(cx).focus_handle(cx);
                            window.focus(&focus, cx);
                        }
                        if let (Some(file), Some(page)) = (&attach, &page)
                            && let Some(reply) = page.read(cx).reply().cloned()
                        {
                            reply.update(cx, |r, cx| r.attach_file(std::path::Path::new(file), cx));
                        }
                        if rail && let Some(page) = page {
                            let drawer = page.read(cx).drawer().clone();
                            drawer.update(cx, |d, cx| d.open(cx));
                        }
                    })
                })
                .ok();
            // A section of the project's settings, once the page is drawn and has read them.
            if add_command || add_skill || edit_command.is_some() || edit_skill.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
                window
                    .update(cx, |_, window, cx| {
                        let main = shell.read(cx).main().clone();
                        let Some(page) = main.read(cx).project_settings_page().cloned() else {
                            return;
                        };
                        let (commands, skills) = {
                            let page = page.read(cx);
                            (page.commands().clone(), page.skills().clone())
                        };
                        if add_command {
                            commands.update(cx, |s, cx| s.add(cx));
                        }
                        if let Some(name) = &edit_command {
                            commands.update(cx, |s, cx| s.edit(name, cx));
                        }
                        if add_skill {
                            skills.update(cx, |s, cx| s.add(window, cx));
                        }
                        if let Some(name) = &edit_skill {
                            skills.update(cx, |s, cx| s.edit(name, cx));
                        }
                    })
                    .ok();
            }
            // A sub-agent read, or the end asked, once the page knows its tree.
            if agent.is_some() || end || ended.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                window
                    .update(cx, |_, _, cx| {
                        let page = shell.read(cx).main().read(cx).session_page().cloned();
                        if let Some(page) = page {
                            page.update(cx, |page, cx| {
                                if let Some(agent) = &agent {
                                    page.select(agent, cx);
                                }
                                if end {
                                    page.ask_end(cx);
                                }
                            });
                        }
                        let group = ended
                            .as_deref()
                            .and_then(|p| shell.read(cx).ended(p).cloned());
                        if let Some(group) = group {
                            group.update(cx, |group, cx| group.toggle(cx));
                        }
                    })
                    .ok();
            }
        })
        .detach();
    });
}
