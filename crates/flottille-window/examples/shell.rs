//! The window's shell in front of a running daemon, with a page or a drawer opened from the
//! command line: nothing needs a pointer to be seen, so it can be captured in a headless
//! compositor.
//!
//! ```text
//! cargo run -p flottille-window --example shell [--features frames] -- [--project <name>] [--add-project <folder>] [--session <id> [--agent <id>]] [--tree] [--rail] [--settings] [--menu place|provider|model|effort] [--new-workspace] [--add-command] [--edit-command <name>] [--add-skill] [--edit-skill <name>] [--edit-task <name>] [--edit-setup <number>] [--run-task <name>] [--tickets] [--item <key>] [--open-step <workspace> <position>] [--card-menu <workspace>] [--remove <workspace>] [--slash <text>] [--attach <picture>] [--end] [--ended] [--open-run <text of its line>] [--details] [--workspace-menu] [--project-settings <project> [--repository <path>] [--tracker] [--section <n>] [--use-forge <path>] [--remove-repository <path>] [--remove-project]] [--how] [--size <w>x<h>]
//! ```
//!
//! `--project` opens a project's page, `--add-project` the "Add a project" modal with the
//! repositories detected in a folder, `--session` opens a session's page (`--agent` then reads one of its sub-agents), `--tree` the
//! left drawer (in a narrow window), `--rail` the session page's drawer (in a window narrower than
//! three columns), `--settings` the settings page, `--menu` one of the project page composer's
//! pickers, `--new-workspace` the project page's new-workspace modal, `--add-command`,
//! `--edit-command`, `--add-skill` and `--edit-skill` the modals of the Commands and Skills
//! sections, `--edit-task` and `--edit-setup` those of the Tasks and Setup sections, `--run-task`
//! runs a task of the open project's page and opens its output, `--tickets` chooses its Tickets
//! tab and `--item` opens the detail of the item of that key in the tab shown, `--open-step` opens a step of a
//! workspace's setup (its position as `workspace.show` gives it) in the same drawer, `--card-menu` opens a workspace card's "⋯" menu and `--remove` its removal confirmation, `--slash` types a text in the open session's composer (its `/` list), `--attach` attaches a picture file to it, `--end` opens the open
//! session's "End the session?" confirmation, `--ended` unfolds the "Ended" group at the bottom of the
//! left tree, `--open-run` opens the detail of the open session's first run whose line holds the text, in its drawer,
//! `--details` unfolds the details of the first request waiting in the rail, `--workspace-menu` opens the
//! rail's Workspace "⋯" menu, `--project-settings` a project's settings page (`--repository` and `--tracker` open its dialogs, `--section` chooses its n-th tab, `--use-forge` uses the forge offered for a repository, `--remove-repository` and `--remove-project` ask their confirmation), `--how` opens "How Auto works" in the settings, `--size` the window's size. The daemon is found on
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
    let (mut end, mut ended) = (false, false);
    let (mut details, mut workspace_menu) = (false, false);
    let mut open_run = None::<String>;
    let (mut edit_task, mut edit_setup, mut run_task) = (None, None, None);
    let (mut items_tab, mut item) = (false, None::<String>);
    let mut open_step = None::<(String, u32)>;
    let (mut card_menu, mut remove) = (None::<String>, None::<String>);
    let (mut repository, mut tracker, mut section, mut use_forge) = (None, false, None, None);
    let (mut remove_repository, mut remove_project, mut how) = (None, false, false);
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
            "--edit-task" => edit_task = Some(args.next().expect("a task")),
            "--edit-setup" => {
                let number = args.next().expect("a step's number");
                edit_setup = Some(number.parse::<usize>().expect("a number"));
            }
            "--card-menu" => card_menu = Some(args.next().expect("a workspace")),
            "--remove" => remove = Some(args.next().expect("a workspace")),
            "--run-task" => run_task = Some(args.next().expect("a task")),
            "--tickets" => items_tab = true,
            "--item" => item = Some(args.next().expect("an item's key")),
            "--open-step" => {
                let workspace = args.next().expect("a workspace");
                let position = args.next().and_then(|n| n.parse().ok());
                open_step = Some((workspace, position.expect("a step position")));
            }
            "--repository" => repository = Some(args.next().expect("a repository's path")),
            "--tracker" => tracker = true,
            "--section" => {
                section = Some(
                    args.next()
                        .expect("a number")
                        .parse::<usize>()
                        .expect("a number"),
                )
            }
            "--use-forge" => use_forge = Some(args.next().expect("a repository's path")),
            "--remove-repository" => {
                remove_repository = Some(args.next().expect("a repository's path"))
            }
            "--remove-project" => remove_project = true,
            "--how" => how = true,
            "--slash" => slash = Some(args.next().expect("what the reply's field holds")),
            "--attach" => attach = Some(args.next().expect("a picture file")),
            "--end" => end = true,
            "--ended" => ended = true,
            "--details" => details = true,
            "--workspace-menu" => workspace_menu = true,
            "--open-run" => open_run = Some(args.next().expect("the text of a run's line")),
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
            if add_command
                || add_skill
                || edit_command.is_some()
                || edit_skill.is_some()
                || edit_task.is_some()
                || edit_setup.is_some()
            {
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
                        if let Some(name) = &edit_task {
                            let tasks = page.read(cx).tasks().clone();
                            let info = tasks.read(cx).tasks().and_then(|listed| {
                                listed.iter().find(|t| &t.name == name).cloned()
                            });
                            tasks.update(cx, |s, cx| s.open(info.as_ref(), window, cx));
                        }
                        if let Some(number) = edit_setup {
                            let setup = page.read(cx).setup().clone();
                            setup.update(cx, |s, cx| s.open(Some(number), window, cx));
                        }
                    })
                    .ok();
            }
            // A workspace card's menu or removal confirmation, once its card is drawn.
            if card_menu.is_some() || remove.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(2500))
                    .await;
                window
                    .update(cx, |_, window, cx| {
                        let Some(page) = shell.read(cx).main().read(cx).project_page().cloned()
                        else {
                            return;
                        };
                        let place = |name: &str| Some(name.to_owned());
                        if let Some(name) = &card_menu
                            && let Some(card) = page.read(cx).card(&place(name), cx)
                        {
                            card.update(cx, |c, cx| c.open_menu(window, cx));
                        }
                        if let Some(name) = &remove
                            && let Some(card) = page.read(cx).card(&place(name), cx)
                        {
                            card.update(cx, |c, cx| c.ask_removal(cx));
                        }
                    })
                    .ok();
            }
            // The project's settings or Settings in one of their states, once they are read.
            if repository.is_some()
                || tracker
                || section.is_some()
                || use_forge.is_some()
                || remove_repository.is_some()
                || remove_project
                || how
            {
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
                window
                    .update(cx, |_, window, cx| {
                        let main = shell.read(cx).main().clone();
                        if how && let Some(page) = main.read(cx).settings_page().cloned() {
                            page.update(cx, |page, cx| page.toggle_how(cx));
                        }
                        let Some(page) = main.read(cx).project_settings_page().cloned() else {
                            return;
                        };
                        page.update(cx, |page, cx| {
                            if let Some(path) = &repository {
                                page.open_repository(path, window, cx);
                            }
                            if tracker {
                                page.open_tracker(window, cx);
                            }
                            if let Some(entry) = section {
                                page.choose(entry, cx);
                            }
                            if let Some(path) = &use_forge {
                                page.use_proposal(path, cx);
                            }
                            if let Some(path) = &remove_repository {
                                let repositories = page.repositories().clone();
                                repositories.update(cx, |r, cx| r.remove(path, cx));
                            }
                            if remove_project {
                                page.ask_removal(cx);
                            }
                        });
                    })
                    .ok();
            }
            // The project page's Tickets tab, or an item's detail, once the items are read.
            if items_tab || item.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                window
                    .update(cx, |_, _, cx| {
                        let page = shell.read(cx).main().read(cx).project_page().cloned()?;
                        let items = page.read(cx).items().clone();
                        items.update(cx, |items, cx| {
                            if items_tab {
                                items.choose(flottille_window::items::Kind::Tickets, cx);
                            }
                            let row = item.as_ref().and_then(|key| {
                                items.rows().into_iter().find(|row| &row.key == key)
                            });
                            if let Some(row) = row {
                                items.open_detail(row, cx);
                            }
                        });
                        Some(())
                    })
                    .ok();
            }
            // A task run from the project's page, then its output opened once it is listed.
            if let Some(name) = run_task {
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
                let tasks = window
                    .update(cx, |_, _, cx| {
                        let page = shell.read(cx).main().read(cx).project_page().cloned()?;
                        let tasks = page.read(cx).tasks().clone();
                        tasks.update(cx, |t, cx| t.run(&name, cx));
                        Some(tasks)
                    })
                    .ok()
                    .flatten();
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                if let Some(tasks) = tasks {
                    window
                        .update(cx, |_, _, cx| {
                            let rows = tasks.read(cx).commands().read(cx).rows().to_vec();
                            let run = rows
                                .iter()
                                .find(|row| row.name.as_deref() == Some(name.as_str()))
                                .or(rows.first())
                                .cloned();
                            if let Some(row) = run {
                                tasks.update(cx, |t, cx| t.open_command(&row.run, cx));
                            }
                        })
                        .ok();
                }
            }
            // A step of a workspace's setup, opened in the drawer once the page is read.
            if let Some((workspace, position)) = open_step {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                window
                    .update(cx, |_, _, cx| {
                        let page = shell.read(cx).main().read(cx).project_page().cloned();
                        if let Some(page) = page {
                            let tasks = page.read(cx).tasks().clone();
                            tasks.update(cx, |t, cx| t.open_step(&workspace, position, cx));
                        }
                    })
                    .ok();
            }
            // A run's detail, once the rail has listed it.
            if open_run.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(2500))
                    .await;
                window
                    .update(cx, |_, _, cx| {
                        let page = shell.read(cx).main().read(cx).session_page().cloned();
                        if let Some(page) = page {
                            let runs = page.read(cx).commands(cx);
                            let found = open_run.as_ref().and_then(|text| {
                                let list = runs.read(cx);
                                list.rows()
                                    .iter()
                                    .find(|row| row.line.contains(text))
                                    .cloned()
                            });
                            if let Some(row) = found {
                                page.update(cx, |page, cx| page.open_command(&row.run, cx));
                            }
                        }
                    })
                    .ok();
            }
            // A sub-agent read, or the end asked, once the page knows its tree.
            if agent.is_some() || end || ended || details || workspace_menu {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                window
                    .update(cx, |_, window, cx| {
                        let page = shell.read(cx).main().read(cx).session_page().cloned();
                        if let Some(page) = &page {
                            let waiting = page.read(cx).waiting(cx);
                            let first = waiting.read(cx).cards().first().map(|c| c.number);
                            if let (true, Some(number)) = (details, first) {
                                waiting.update(cx, |w, cx| w.toggle_details(number, cx));
                            }
                            if workspace_menu {
                                let menu = page.read(cx).rail().read(cx).menu().clone();
                                menu.update(cx, |m, cx| m.open(window, cx));
                            }
                        }
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
                        let group = ended.then(|| shell.read(cx).ended().cloned()).flatten();
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
