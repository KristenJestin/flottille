//! The scenarios of the window's shell (#11), on GPUI's test platform in front of a real daemon
//! whose agents are the fake ACP agent, with a throwaway user (temporary home and XDG folders).
//! No window opens on a screen.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::Mcp;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_ui::state::icon_id;
use flottille_window::Shell;
use flottille_window::actions::Changes;
use flottille_window::composer::{Choice, Composer, Menu, PlaceChoice};
use flottille_window::conversation::Block;
use flottille_window::link::{self, Connect};
use flottille_window::model::{Place, Preparation};
use flottille_window::project::{NewWorkspace, PlaceCard, ProjectPage};
use flottille_window::project_settings::{ForgeKind, ProjectSettingsPage, Said};
use flottille_window::session::{RailRow, Reply, SessionPage};
use flottille_window::settings::SettingsPage;
use flottille_window::shell::{Layout, Selection};
use flottille_window::tree::{place_id, project_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A user whose fake agent answers "ok", reads `.env` when asked to (a call the gate holds for
/// the user), asks the user a question when asked to, and works a few seconds when asked to.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "read the env", "do": [{"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}}]},
        {"when": "ask me", "do": [{"mcp": {"tool": "ask_user", "arguments": {"question": "Which database?"}}}]},
        {"when": "work a while", "do": [{"wait": 4000}, {"say": "done"}]},
        {"when": "work briefly", "do": [{"wait": 1000}, {"say": "done"}]},
        {"when": "work long", "do": [{"wait": 30000}, {"say": "done"}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A project over a throwaway repository with a remote and a `.env`.
fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "TOKEN=made-up-made-up\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path(&format!("remotes/{name}.git"));
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

fn workspace(user: &User, project: &str, name: &str) {
    user.call(
        "workspace.create",
        json!({"project": project, "name": name}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": project, "name": name}))
            .unwrap()["state"]
            == "ready"
    });
}

/// Starts a session: a root agent with its first message. Its id.
fn start(user: &User, project: &str, workspace: Option<&str>, message: &str) -> String {
    let mut params = json!({"project": project, "message": message});
    if let Some(workspace) = workspace {
        params["workspace"] = json!(workspace);
    }
    let answer = user.call("agent.start", params).unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == id.as_str() && a["state"] == "idle")
    });
    id
}

fn send(user: &User, agent: &str, message: &str) {
    user.call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The window's shell in a test window of `width` × 800, following the user's daemon through
/// `connect`.
fn window<'a>(
    user: &User,
    connect: Connect,
    width: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(width), px(800.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the link sent, then draws what changed
/// (nothing else), as a real frame would.
fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        // A frame as the platform asks for one: `Window::draw` alone builds again every view an
        // update touched, even one not notified.
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A fresh read of what the shell shows, on a connection of its own.
fn fresh(user: &User) -> flottille_window::model::Snapshot {
    link::read_once((user.connector())().unwrap()).unwrap()
}

fn mark_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<AgentState> {
    shell.read_with(cx, |shell, cx| {
        shell.session(id).and_then(|row| row.read(cx).mark())
    })
}

#[gpui_kit::test]
fn two_projects_main_and_a_workspace_with_sessions_under_each_appear_in_the_tree(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    project(&user, "notes");
    workspace(&user, "atlas", "export-csv");
    let in_main = start(&user, "atlas", None, "Release notes for 2.4");
    let in_workspace = start(&user, "atlas", Some("export-csv"), "CSV export by tenant");
    let in_notes = start(&user, "notes", None, "Offline sync spike");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);

    until(&shell, cx, "the three sessions are shown", |cx| {
        cx.update(|window, _| {
            [&in_main, &in_workspace, &in_notes]
                .iter()
                .all(|id| window.try_find(session_id(id)).is_some())
        })
    });
    cx.update(|window, _| {
        for id in [
            project_id("atlas"),
            project_id("notes"),
            place_id("atlas", &None),
            place_id("atlas", &Some("export-csv".into())),
            place_id("notes", &None),
        ] {
            assert!(window.try_find(id.clone()).is_some(), "{id:?} is not shown");
        }
        // One level below the other, each on its own guide line.
        let x = |id| window.find(id).bounds().origin.x;
        assert!(x(place_id("atlas", &None)) > x(project_id("atlas")));
        assert_eq!(
            x(place_id("atlas", &None)),
            x(place_id("atlas", &Some("export-csv".into()))),
            "main and the workspaces sit at the same level"
        );
        assert!(x(session_id(&in_main)) > x(place_id("atlas", &None)));
    });
    let sessions_of = |project: &str, place: Option<&str>, cx: &mut VisualTestContext| {
        shell.read_with(cx, |shell, cx| {
            shell
                .place(project, &place.map(str::to_owned))
                .unwrap()
                .read(cx)
                .sessions(cx)
                .iter()
                .map(|row| row.read(cx).title().to_owned())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(sessions_of("atlas", None, cx), ["Release notes for 2.4"]);
    assert_eq!(
        sessions_of("atlas", Some("export-csv"), cx),
        ["CSV export by tenant"]
    );
    assert_eq!(sessions_of("notes", None, cx), ["Offline sync spike"]);
}

#[gpui_kit::test]
fn a_session_that_starts_waiting_marks_its_row_its_folded_place_and_its_folded_project_and_the_bell_count_goes_up(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let id = start(&user, "atlas", None, "Sessions expire too early");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the session is shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    assert_eq!(shell.read_with(cx, |s, cx| s.top().read(cx).count()), 0);

    // Fold the place, then the project (its chevron: its name opens its page and folds nothing).
    cx.update(|window, cx| window.click(place_id("atlas", &None), cx));
    cx.update(|window, cx| window.click(flottille_ui::toggle_id(project_id("atlas")), cx));
    let (place, project) = shell.read_with(cx, |shell, _| {
        (
            shell.place("atlas", &None).unwrap().clone(),
            shell.project("atlas").unwrap().clone(),
        )
    });
    assert!(!place.read_with(cx, |p, _| p.is_open()));
    assert!(!project.read_with(cx, |p, _| p.is_open()));

    send(&user, &id, "read the env");
    until(&shell, cx, "the session waits for the user", |cx| {
        mark_of(&shell, &id, cx) == Some(AgentState::NeedsYou)
    });
    assert_eq!(
        place.read_with(cx, |p, _| p.mark()),
        Some(AgentState::NeedsYou)
    );
    assert_eq!(
        project.read_with(cx, |p, _| p.mark()),
        Some(AgentState::NeedsYou),
        "folding never hides what waits"
    );
    assert_eq!(shell.read_with(cx, |s, cx| s.top().read(cx).count()), 1);
    cx.update(|window, cx| window.render_frame(cx));
    let shown = cx.update(|window, _| window.find_all(icon_id(AgentState::NeedsYou)).len());
    assert_eq!(
        shown, 4,
        "the row, the place and the project show the mark, and the session's row on the project \
         page"
    );
}

#[gpui_kit::test]
fn choosing_an_item_of_the_bell_opens_its_session(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let other = start(&user, "atlas", None, "Tidy the aliases");
    let id = start(&user, "atlas", None, "Sessions expire too early");
    send(&user, &id, "read the env");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the bell counts the request", |cx| {
        shell.read_with(cx, |s, cx| s.top().read(cx).count()) == 1
    });
    cx.update(|window, cx| window.click(session_id(&other), cx));
    cx.update(|window, cx| window.click("bell", cx));
    assert!(shell.read_with(cx, |s, _| s.bell_open()));
    let key = shell.read_with(cx, |s, _| s.snapshot().bell[0].key.clone());
    cx.update(|window, cx| window.click(format!("bell-{key}"), cx));
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(id))
    );
    assert!(
        !shell.read_with(cx, |s, _| s.bell_open()),
        "a choice closes the list"
    );
}

#[gpui_kit::test]
fn a_root_s_question_marks_its_session_and_rings_the_bell_until_the_user_writes(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let id = start(&user, "atlas", None, "Sessions expire too early");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the session is shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });

    send(&user, &id, "ask me");
    until(&shell, cx, "the question waits for the user", |cx| {
        mark_of(&shell, &id, cx) == Some(AgentState::NeedsYou)
    });
    assert_eq!(shell.read_with(cx, |s, cx| s.top().read(cx).count()), 1);
    let item = shell.read_with(cx, |s, _| s.snapshot().bell[0].clone());
    assert_eq!(item.session, id);
    assert_eq!(item.detail, "Which database?");

    send(&user, &id, "postgres");
    until(&shell, cx, "the answer is taken", |cx| {
        shell.read_with(cx, |s, cx| s.top().read(cx).count()) == 0
            && mark_of(&shell, &id, cx) != Some(AgentState::NeedsYou)
    });
}

#[gpui_kit::test]
fn one_session_changing_state_redraws_its_row_and_not_the_others(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    let quiet = [
        start(&user, "atlas", None, "First"),
        start(&user, "atlas", None, "Second"),
        start(&user, "atlas", Some("login"), "Third"),
    ];
    let busy = start(&user, "atlas", Some("login"), "Fourth");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the sessions are shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&busy)).is_some())
    });
    // Atlas's page is open, its details and its lists read: an answer that lands later draws it.
    until(&shell, cx, "the page has its details", |cx| {
        page_of(&shell, cx).read_with(cx, |page, _| page.chips()) == ["atlas · main"]
    });
    settle(cx);
    let renders = |id: &str, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| s.session(id).unwrap().read(cx).renders())
    };
    let before: Vec<usize> = quiet.iter().map(|id| renders(id, cx)).collect();
    let busy_before = renders(&busy, cx);
    let page = page_of(&shell, cx);
    let composer = composer_of(&shell, "atlas", cx);
    let page_and_composer = |cx: &mut VisualTestContext| {
        (
            page.read_with(cx, |p, _| p.renders()),
            composer.read_with(cx, |c, _| c.renders()),
        )
    };
    let page_and_composer_before = page_and_composer(cx);

    send(&user, &busy, "work a while");
    until(&shell, cx, "the session runs", |cx| {
        mark_of(&shell, &busy, cx) == Some(AgentState::Running)
    });
    until(&shell, cx, "the session is at rest again", |cx| {
        mark_of(&shell, &busy, cx).is_none()
    });

    assert!(renders(&busy, cx) > busy_before, "its row is drawn again");
    let after: Vec<usize> = quiet.iter().map(|id| renders(id, cx)).collect();
    assert_eq!(after, before, "the other rows are not drawn again");
    assert_eq!(
        page_and_composer(cx),
        page_and_composer_before,
        "nor the project's page, nor its composer"
    );
}

#[gpui_kit::test]
fn when_the_daemon_is_restarted_the_window_reconnects_and_catches_up_without_a_gap(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let first = start(&user, "atlas", None, "Before the restart");
    // The window's way to the daemon, which the test closes while it changes things.
    let closed = Arc::new(AtomicBool::new(false));
    let connector = user.connector();
    let gate = closed.clone();
    let connect: Connect = Arc::new(move || {
        if gate.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("closed by the test"));
        }
        connector()
    });
    let (shell, cx) = window(&user, connect, 1280., cx);
    until(&shell, cx, "the session is shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&first)).is_some())
    });

    closed.store(true, Ordering::SeqCst);
    let stopped = user.run(&["daemon", "stop"]);
    assert!(
        stopped.status.success(),
        "daemon stop failed: {}\nthe daemon's log:\n{}",
        String::from_utf8_lossy(&stopped.stderr),
        std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap_or_default()
    );
    until(&shell, cx, "the window sees the daemon gone", |cx| {
        !shell.read_with(cx, |s, cx| s.top().read(cx).online())
    });
    // A new daemon, and changes the window has not seen.
    project(&user, "notes");
    let second = start(&user, "notes", None, "After the restart");
    send(&user, &second, "read the env");
    eventually("the request is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    closed.store(false, Ordering::SeqCst);

    until(&shell, cx, "the window catches up", |cx| {
        shell.read_with(cx, |s, _| s.snapshot() == &fresh(&user))
    });
    assert!(shell.read_with(cx, |s, cx| s.top().read(cx).online()));
    assert_eq!(mark_of(&shell, &second, cx), Some(AgentState::NeedsYou));
    let seen = shell.read_with(cx, |s, _| s.seen().clone());
    assert_eq!(seen.gaps.load(Ordering::SeqCst), 0, "no event was missed");
    let latest = (user.connector())().unwrap();
    let mut conn = std::io::BufReader::new(latest.try_clone().unwrap());
    use std::io::{BufRead, Write};
    writeln!(
        &latest,
        r#"{{"id":"1","method":"events.watch","params":{{}}}}"#
    )
    .unwrap();
    let mut line = String::new();
    conn.read_line(&mut line).unwrap();
    let answer: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        seen.last_seq.load(Ordering::SeqCst),
        answer["result"]["after"].as_u64().unwrap(),
        "the window saw every event up to the last"
    );
}

#[gpui_kit::test]
fn below_860_the_tree_is_a_drawer_opened_by_the_menu_button(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let id = start(&user, "atlas", None, "On a portrait screen");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 720., cx);
    until(&shell, cx, "the session is known", |cx| {
        shell.read_with(cx, |s, _| s.session(&id).is_some())
    });
    assert_eq!(shell.read_with(cx, |s, _| s.layout()), Layout::Narrow);
    cx.update(|window, cx| window.render_frame(cx));
    assert!(
        cx.update(|window, _| window.try_find(session_id(&id)).is_none()),
        "the tree is not drawn beside the page"
    );
    cx.update(|window, cx| window.click("open-tree", cx));
    flottille_ui_advance(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(id))
    );
    let drawer = shell.read_with(cx, |s, _| s.left_drawer().clone());
    assert!(
        !drawer.read_with(cx, |d, _| d.is_open()),
        "a choice closes the drawer"
    );

    // Wider again: the tree is beside the page, the rail of a session a drawer.
    cx.simulate_resize(size(px(1000.), px(800.)));
    until(&shell, cx, "the layout follows the width", |cx| {
        shell.read_with(cx, |s, _| s.layout()) == Layout::Medium
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(cx.update(|window, _| window.try_find("open-tree").is_none()));
    assert!(cx.update(|window, _| window.try_find("open-rail").is_some()));
    cx.simulate_resize(size(px(1300.), px(800.)));
    until(&shell, cx, "three columns", |cx| {
        shell.read_with(cx, |s, _| s.layout()) == Layout::Wide
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(cx.update(|window, _| window.try_find("open-rail").is_none()));
}

/// Lets a drawer's slide end.
fn flottille_ui_advance(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

// The project page (#12).

/// A recipe step for every new workspace of `project`: `sh <script>`, the script holding `body`.
fn recipe(user: &User, project: &str, body: &str) {
    let script = user.path(&format!("tools/{project}-recipe.sh"));
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, body).unwrap();
    let file = user.project_file(project);
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&format!(
        "\n[[recipe]]\nrun = \"sh {}\"\n",
        script.display()
    ));
    std::fs::write(file, text).unwrap();
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ProjectPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).project_page().cloned())
        .expect("a project's page is open")
}

fn card_of(
    shell: &Entity<Shell>,
    place: Option<&str>,
    cx: &mut VisualTestContext,
) -> Option<Entity<PlaceCard>> {
    let page = shell.read_with(cx, |s, cx| s.main().read(cx).project_page().cloned())?;
    page.read_with(cx, |page, cx| page.card(&place.map(str::to_owned), cx))
}

/// Lets the page's entrance and every other motion end, then draws.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Types `text` in the composer's field, given the focus by a click as a user would.
fn type_task(text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input(text);
}

fn titles_in(card: &Entity<PlaceCard>, cx: &mut VisualTestContext) -> Vec<String> {
    card.read_with(cx, |card, _| {
        card.data()
            .sessions
            .iter()
            .map(|s| s.title.clone())
            .collect()
    })
}

#[gpui_kit::test]
fn the_page_lists_main_and_two_workspaces_with_their_sessions(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "export-csv");
    workspace(&user, "atlas", "login");
    start(&user, "atlas", None, "Release notes for 2.4");
    start(&user, "atlas", Some("export-csv"), "CSV export by tenant");
    start(&user, "atlas", Some("login"), "Sessions expire too early");
    start(&user, "atlas", Some("login"), "Review the refresh flow");
    let login = PathBuf::from(
        user.call(
            "workspace.show",
            json!({"project": "atlas", "name": "login"}),
        )
        .unwrap()["folder"]
            .as_str()
            .unwrap(),
    );
    std::fs::write(login.join("NOTES.md"), "one\ntwo\nthree\n").unwrap();
    // More new files than the daemon reads: the card says "at least".
    let generated = login.with_file_name("export-csv").join("generated");
    std::fs::create_dir_all(&generated).unwrap();
    for at in 0..1001 {
        std::fs::write(generated.join(format!("{at}.txt")), "x\n").unwrap();
    }
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);

    until(&shell, cx, "the page shows the lines changed", |cx| {
        card_of(&shell, Some("login"), cx).is_some_and(|card| {
            card.read_with(cx, |c, _| c.changes())
                == Some(Changes {
                    added: 3,
                    removed: 0,
                    capped: false,
                })
        })
    });
    assert_eq!(
        card_of(&shell, Some("export-csv"), cx).map(|card| card.read_with(cx, |c, _| c.changes())),
        Some(Some(Changes {
            added: 1000,
            removed: 0,
            capped: true
        }))
    );
    let page = page_of(&shell, cx);
    let places: Vec<Place> = page.read_with(cx, |page, cx| {
        page.cards()
            .iter()
            .map(|card| card.read(cx).data().place.clone())
            .collect()
    });
    assert_eq!(
        places,
        [None, Some("export-csv".into()), Some("login".into())],
        "main first, then each workspace"
    );
    let cards = page.read_with(cx, |page, _| page.cards().to_vec());
    assert_eq!(titles_in(&cards[0], cx), ["Release notes for 2.4"]);
    assert_eq!(titles_in(&cards[1], cx), ["CSV export by tenant"]);
    assert_eq!(
        titles_in(&cards[2], cx),
        ["Review the refresh flow", "Sessions expire too early"]
    );
    assert_eq!(
        cards[1].read_with(cx, |c, _| c.data().meta.clone()),
        Some("from main · atlas".to_owned())
    );
    assert_eq!(cards[0].read_with(cx, |c, _| c.data().meta.clone()), None);
    assert_eq!(
        page.read_with(cx, |page, _| page.chips()),
        ["atlas · main"],
        "a chip per repository with its branch"
    );
    settle(cx);
    cx.update(|window, _| {
        for id in [
            "place-card-main",
            "place-card-export-csv",
            "place-card-login",
            "new-session-in-login",
            "new-workspace-card",
        ] {
            assert!(window.try_find(id).is_some(), "{id} is not shown");
        }
        let (main, workspace) = (
            window.find("place-card-main").bounds(),
            window.find("place-card-export-csv").bounds(),
        );
        assert!(workspace.origin.x > main.origin.x, "two columns");
        assert_eq!(workspace.origin.y, main.origin.y);
    });

    // "New session in login" points the composer there.
    cx.update(|window, cx| window.click("new-session-in-login", cx));
    let composer = composer_of(&shell, "atlas", cx);
    assert_eq!(
        composer.read_with(cx, |c, _| c.choice().clone()),
        Choice::Workspace("login".into())
    );
}

#[gpui_kit::test]
fn a_failing_recipe_shows_its_output_on_the_card(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    recipe(
        &user,
        "atlas",
        "echo 'npm ERR! made-up failure in the install'\nexit 3\n",
    );
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(cx);

    cx.update(|window, cx| window.click("new-workspace", cx));
    cx.simulate_input("broken");
    cx.simulate_keystrokes("enter");

    until(&shell, cx, "the card shows the failure", |cx| {
        card_of(&shell, Some("broken"), cx).is_some_and(|card| {
            matches!(
                card.read_with(cx, |c, _| c.data().preparation.clone()),
                Preparation::Failed { .. }
            )
        })
    });
    let page = page_of(&shell, cx);
    let card = page.read_with(cx, |page, cx| {
        page.card(&Some("broken".into()), cx).unwrap()
    });
    let Preparation::Failed { output, .. } =
        card.read_with(cx, |c, _| c.data().preparation.clone())
    else {
        unreachable!()
    };
    assert!(
        output.contains("npm ERR! made-up failure in the install"),
        "{output}"
    );
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("failure-output-broken").is_some()));
    let agents = user.call("agent.list", json!({})).unwrap();
    assert_eq!(agents["agents"], json!([]), "no session started");
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("atlas".into())),
        "the page stays open"
    );
}

#[gpui_kit::test]
fn the_project_page_has_no_running_agents_tab(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let running = start(&user, "atlas", None, "CSV export by tenant");
    send(&user, &running, "work long");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "a session runs", |cx| {
        mark_of(&shell, &running, cx) == Some(AgentState::Running)
    });
    open_project(&shell, "atlas", cx);
    cx.update(|window, _| {
        assert!(window.try_find("composer-field").is_some());
        for gone in ["tab-task", "tab-running", "running-list"] {
            assert!(window.try_find(gone).is_none(), "{gone} is shown");
        }
        assert!(window.try_find(format!("running-{running}")).is_none());
    });
}

#[gpui_kit::test]
fn new_workspace_opens_a_modal_and_escape_closes_it(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    // A session in main: a page with nothing in it shows its empty state, not the dashed card.
    start(&user, "atlas", None, "Release notes");
    user.call(
        "project.set_settings",
        json!({"name": "atlas",
            "tracker": {"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}}),
    )
    .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "main's card lists the session", |cx| {
        card_of(&shell, None, cx).is_some_and(|card| !titles_in(&card, cx).is_empty())
    });
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();

    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);
    cx.update(|window, _| {
        let modal = window.find("new-workspace-modal").bounds();
        let viewport = window.viewport_size();
        assert!(
            (modal.center().x - viewport.width / 2.).abs() < px(1.),
            "centred"
        );
        assert!(window.find("new-workspace-modal-name").visible());
        assert!(window.find("new-workspace-modal-cancel").visible());
        assert!(window.find("create-workspace").visible());
    });
    // The caret is in the name: what is typed goes there.
    cx.simulate_input("login");
    assert_eq!(
        form.read_with(cx, |f, cx| f.name_field().read(cx).value().to_string()),
        "login"
    );
    cx.simulate_keystrokes("escape");
    assert!(!form.read_with(cx, |f, _| f.is_open()));
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("new-workspace-modal").is_some()
                && window.try_find("new-workspace-modal-scrim").is_some(),
            "the modal and its backdrop leave before they go"
        );
    });
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find("new-workspace-modal").is_none());
        assert!(window.try_find("new-workspace-modal-scrim").is_none());
        assert!(
            window.try_find("new-workspace-card").is_some(),
            "the card stays in the grid"
        );
    });

    // The dashed card opens it too, and Cancel closes it.
    cx.update(|window, cx| window.click("new-workspace-card", cx));
    settle(cx);
    assert!(form.read_with(cx, |f, _| f.is_open()));
    cx.update(|window, cx| window.click("new-workspace-modal-cancel", cx));
    assert!(!form.read_with(cx, |f, _| f.is_open()));
    assert!(
        user.call("workspace.list", json!({"project": "atlas"}))
            .unwrap()["workspaces"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing was made"
    );
}

#[gpui_kit::test]
fn the_composer_has_no_new_task_row(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);

    cx.update(|window, _| {
        let composer = window.find("composer").bounds();
        let field = window.find("composer-field").bounds();
        // The shell's padding and border, and nothing above the field.
        assert!(
            field.origin.y - composer.origin.y <= px(10.),
            "a row sits above the field: {:?} below the composer's top",
            field.origin.y - composer.origin.y
        );
    });
}

#[gpui_kit::test]
fn the_main_card_has_no_second_header_and_a_workspace_card_keeps_where_it_comes_from(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the page shows both cards", |cx| {
        card_of(&shell, Some("login"), cx).is_some()
    });
    settle(cx);

    cx.update(|window, _| {
        assert!(window.try_find("place-card-main").is_some());
        assert!(
            window.try_find("meta-main").is_none(),
            "the project folder's card repeats its title in a second line"
        );
        assert!(window.find("meta-login").visible());
    });
}

#[gpui_kit::test]
fn the_place_picker_s_new_workspace_opens_the_same_modal_as_the_page_s_action(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    let composer = composer_of(&shell, "atlas", cx);

    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);
    let from_page = cx.update(|window, _| window.find("new-workspace-modal").bounds());
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!form.read_with(cx, |f, _| f.is_open()));

    cx.update(|window, cx| window.click("place-picker", cx));
    cx.update(|window, cx| window.click("place-choice-new", cx));
    settle(cx);
    assert!(
        form.read_with(cx, |f, _| f.is_open()),
        "the page's form is open"
    );
    cx.update(|window, _| {
        assert_eq!(
            window.find("new-workspace-modal").bounds(),
            from_page,
            "the same modal"
        );
        assert!(
            window.try_find("new-workspace-name").is_none(),
            "no inline input is left in the picker"
        );
    });
    assert_eq!(
        composer.read_with(cx, |c, _| c.choice().clone()),
        Choice::Main,
        "the picker keeps its place"
    );
    // The caret is in the modal's name, and Enter makes the workspace there.
    cx.simulate_input("from-picker");
    cx.simulate_keystrokes("enter");
    until(
        &shell,
        cx,
        "the workspace is made and the modal closes",
        |cx| !form.read_with(cx, |f, _| f.is_open()),
    );
    assert_eq!(
        user.call("workspace.list", json!({"project": "atlas"}))
            .unwrap()["workspaces"][0]["name"],
        "from-picker"
    );
}

#[gpui_kit::test]
fn a_workspace_made_from_the_composer_s_picker_becomes_its_place(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    let composer = composer_of(&shell, "atlas", cx);

    cx.update(|window, cx| window.click("place-picker", cx));
    cx.update(|window, cx| window.click("place-choice-new", cx));
    settle(cx);
    cx.simulate_input("from-picker");
    cx.simulate_keystrokes("enter");
    until(
        &shell,
        cx,
        "the composer's place is the workspace it asked for",
        |cx| {
            composer.read_with(cx, |c, _| c.choice().clone())
                == Choice::Workspace("from-picker".into())
        },
    );
    assert!(!form.read_with(cx, |f, _| f.is_open()), "the modal closed");
}

#[gpui_kit::test]
fn a_workspace_made_from_the_page_s_action_leaves_the_composer_s_place(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    let composer = composer_of(&shell, "atlas", cx);

    // Opened from the picker first, then closed: the page's own action must not inherit it.
    cx.update(|window, cx| window.click("place-picker", cx));
    cx.update(|window, cx| window.click("place-choice-new", cx));
    settle(cx);
    cx.simulate_keystrokes("escape");
    settle(cx);
    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);
    cx.simulate_input("from-page");
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the workspace is made", |cx| {
        !form.read_with(cx, |f, _| f.is_open())
    });
    until(&shell, cx, "the card of the workspace is shown", |cx| {
        cx.update(|window, _| window.try_find("place-card-from-page").is_some())
    });
    assert_eq!(
        composer.read_with(cx, |c, _| c.choice().clone()),
        Choice::Main,
        "the composer's place did not move"
    );
}

/// The height of the composer's field once its text is `text`.
fn field_height(composer: &Entity<Composer>, text: &str, cx: &mut VisualTestContext) -> f32 {
    let field = composer.read_with(cx, |c, _| c.field().clone());
    field.update_in(cx, |field, window, cx| field.set_value(text, window, cx));
    for _ in 0..4 {
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, _| f32::from(window.find("composer-field").bounds().size.height))
}

#[gpui_kit::test]
fn the_composer_grows_line_by_line_up_to_a_limit_then_scrolls(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    let lines = |n: usize| vec!["a line of the task"; n].join("\n");

    // Padding 12 above and below; a line is 21 px; three lines at least, eight at most.
    let heights: Vec<f32> = [1, 2, 3, 4, 8, 9, 40]
        .map(|n| field_height(&composer, &lines(n), cx))
        .into();
    assert_eq!(
        heights,
        [87., 87., 87., 108., 192., 192., 192.],
        "one, two, three, four, eight, nine and forty lines"
    );
    let back = field_height(&composer, &lines(3), cx);
    assert_eq!(back, 87., "it shrinks with the text");

    // Typed, as a user does: a line at a time, with no gap left under the last one.
    field_height(&composer, "", cx);
    type_task("one", cx);
    let mut typed = Vec::new();
    for line in ["two", "three", "four"] {
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input(line);
        typed.push(field_height(&composer, &field_of(&composer, cx), cx));
    }
    assert_eq!(typed, [87., 87., 108.], "two, three and four typed lines");
}

#[gpui_kit::test]
fn the_project_pages_empty_composer_is_three_lines_high_and_the_replys_input_is_not_changed(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    // Padding 12 above and below, three lines of 21 px.
    assert_eq!(field_height(&composer, "", cx), 87., "no text");

    // The session's reply input keeps the height it had.
    let lead = start(&user, "atlas", None, "Release notes for 2.4");
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the session's page is open", |cx| {
        cx.update(|window, _| window.try_find("reply-field").is_some())
    });
    settle(cx);
    let reply = cx.update(|window, _| f32::from(window.find("reply-field").bounds().size.height));
    assert_eq!(reply, REPLY_FIELD_HEIGHT, "the reply input, empty");
}

/// The height of the session's reply input with no text, as it was before the project page's
/// composer changed.
const REPLY_FIELD_HEIGHT: f32 = 60.;

#[gpui_kit::test]
fn typing_in_the_composer_does_not_redraw_the_page(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    start(&user, "atlas", None, "Release notes for 2.4");
    start(&user, "atlas", Some("login"), "Sessions expire too early");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the page shows its cards", |cx| {
        card_of(&shell, Some("login"), cx).is_some()
    });
    // A click gives the field the focus, and a first key puts the window in keyboard mode: GPUI
    // draws the whole window again for each, once, which is not typing.
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_keystrokes("right");
    settle(cx);
    let page = page_of(&shell, cx);
    let composer = composer_of(&shell, "atlas", cx);
    let cards = page.read_with(cx, |page, _| page.cards().to_vec());
    let renders = |cx: &mut VisualTestContext| {
        (
            page.read_with(cx, |p, _| p.renders()),
            cards
                .iter()
                .map(|c| c.read_with(cx, |c, _| c.renders()))
                .collect::<Vec<_>>(),
            composer.read_with(cx, |c, _| c.renders()),
        )
    };
    let (page_before, cards_before, composer_before) = renders(cx);

    // Each key is drawn as the platform draws it, when the update is flushed.
    cx.simulate_input("Fix the timeout");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("and its test");
    // The second line made the field taller, so what is below it moved: drawn once more for that,
    // and once for the send button waking up in the composer.
    let (page_grown, cards_grown, composer_grown) = renders(cx);
    assert!(
        page_grown <= page_before + 1,
        "{page_grown} after {page_before}"
    );
    for (grown, before) in cards_grown.iter().zip(&cards_before) {
        assert!(grown <= &(before + 1), "{grown} after {before}");
    }
    assert!(composer_grown <= composer_before + 2);
    cx.simulate_input(" and every other detail of it");

    let field = composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string());
    assert_eq!(
        field, "Fix the timeout\nand its test and every other detail of it",
        "Shift+Enter makes a new line"
    );
    assert!(
        !composer.read_with(cx, |c, _| c.is_sending()),
        "nothing was sent"
    );
    let (page_after, cards_after, composer_after) = renders(cx);
    assert_eq!(
        page_after, page_grown,
        "the page is not drawn once a letter"
    );
    assert_eq!(cards_after, cards_grown, "nor its cards");
    assert_eq!(composer_after, composer_grown, "nor the composer");
}

fn composer_of(
    shell: &Entity<Shell>,
    project: &str,
    cx: &mut VisualTestContext,
) -> Entity<Composer> {
    shell
        .read_with(cx, |s, _| s.composer_of(project).cloned())
        .unwrap_or_else(|| panic!("{project}'s page has a composer"))
}

fn field_of(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
    composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string())
}

/// Opens `project`'s page, as a click in the tree would, and lets its entrance end.
fn open_project(shell: &Entity<Shell>, project: &str, cx: &mut VisualTestContext) {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project(project.into()), window, cx)
    });
    until(shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == project)
        })
    });
    settle(cx);
}

/// Connects to the user's daemon through a relay that holds back the answer to every
/// `agent.start` for `delay`, then gives it as it came, or as a refusal saying `refuse`.
fn slow_start(user: &User, delay: Duration, refuse: Option<&'static str>) -> Connect {
    use std::collections::HashSet;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    let connect = user.connector();
    Arc::new(move || {
        let daemon = connect()?;
        let (window_side, relay_side) = UnixStream::pair()?;
        let starts: Arc<Mutex<HashSet<String>>> = Arc::default();
        let (mut to_daemon, from_window) = (daemon.try_clone()?, relay_side.try_clone()?);
        let marks = starts.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(from_window).lines() {
                let Ok(line) = line else { return };
                let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                if request["method"] == "agent.start" {
                    marks.lock().unwrap().insert(request["id"].to_string());
                }
                if writeln!(to_daemon, "{line}").is_err() {
                    return;
                }
            }
        });
        let mut to_window = relay_side;
        std::thread::spawn(move || {
            for line in BufReader::new(daemon).lines() {
                let Ok(line) = line else { return };
                let answer: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let mut line = line;
                if starts.lock().unwrap().remove(&answer["id"].to_string()) {
                    std::thread::sleep(delay);
                    if let Some(why) = refuse {
                        line = json!({"id": answer["id"], "error": {"code": "refused", "message": why}})
                            .to_string();
                    }
                }
                if writeln!(to_window, "{line}").is_err() {
                    return;
                }
            }
        });
        Ok(window_side)
    })
}

/// Sends `task` from atlas's composer, to main.
fn send_to_main(task: &str, cx: &mut VisualTestContext) {
    type_task(task, cx);
    cx.update(|window, cx| window.click("send", cx));
}

#[gpui_kit::test]
fn a_session_started_after_switching_project_stays_with_its_project(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "borealis");
    let (shell, cx) = window(
        &user,
        slow_start(&user, Duration::from_secs(2), None),
        1280.,
        cx,
    );
    open_project(&shell, "atlas", cx);
    send_to_main("Fix the login", cx);
    let atlas = composer_of(&shell, "atlas", cx);
    assert!(atlas.read_with(cx, |c, _| c.is_sending()));

    open_project(&shell, "borealis", cx);
    let borealis = composer_of(&shell, "borealis", cx);
    assert!(
        !borealis.read_with(cx, |c, _| c.is_sending()),
        "another project's send is not this one's"
    );
    assert_eq!(field_of(&borealis, cx), "", "nor its draft");
    type_task("A draft for borealis", cx);
    until(&shell, cx, "atlas's session started", |cx| {
        !atlas.read_with(cx, |c, _| c.is_sending())
    });

    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("borealis".into())),
        "the user is not pulled away from the page they opened"
    );
    assert_eq!(field_of(&borealis, cx), "A draft for borealis");
    assert!(borealis.read_with(cx, |c, _| c.error().is_none() && !c.is_sending()));
    let agents = user.call("agent.list", json!({})).unwrap();
    assert_eq!(agents["agents"].as_array().map(Vec::len), Some(1));
    assert_eq!(field_of(&atlas, cx), "", "atlas's field is emptied");
    open_project(&shell, "atlas", cx);
    assert_eq!(
        shell.read_with(cx, |s, cx| s
            .main()
            .read(cx)
            .project_page()
            .unwrap()
            .read(cx)
            .composer()
            .clone()),
        atlas,
        "atlas's page shows atlas's composer"
    );
}

#[gpui_kit::test]
fn a_send_that_fails_after_switching_project_says_so_on_its_project(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "borealis");
    let slow = slow_start(&user, Duration::from_secs(2), Some("made-up failure"));
    let (shell, cx) = window(&user, slow, 1280., cx);
    open_project(&shell, "atlas", cx);
    send_to_main("Try the broken start", cx);
    let atlas = composer_of(&shell, "atlas", cx);

    open_project(&shell, "borealis", cx);
    let borealis = composer_of(&shell, "borealis", cx);
    type_task("A draft for borealis", cx);
    until(&shell, cx, "atlas's send failed", |cx| {
        atlas.read_with(cx, |c, _| c.error().is_some())
    });

    assert!(
        borealis.read_with(cx, |c, _| c.error().is_none() && !c.is_sending()),
        "the failure is not shown on another project"
    );
    assert_eq!(field_of(&borealis, cx), "A draft for borealis");
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("borealis".into()))
    );
    open_project(&shell, "atlas", cx);
    assert_eq!(
        field_of(&atlas, cx),
        "Try the broken start",
        "atlas keeps its task, to send again"
    );
}

#[gpui_kit::test]
fn a_new_workspace_made_in_one_project_does_not_close_another_project_s_form(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    project(&user, "borealis");
    workspace(&user, "borealis", "login");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let form_of = |project: &str, cx: &mut VisualTestContext| {
        shell
            .read_with(cx, |s, _| s.new_workspace_of(project).cloned())
            .unwrap()
    };
    let create = |form: &Entity<NewWorkspace>, cx: &mut VisualTestContext| {
        form.update_in(cx, |form, window, cx| form.open(window, cx));
        cx.simulate_input("login");
        cx.simulate_keystrokes("enter");
    };
    let atlas = form_of("atlas", cx);
    create(&atlas, cx);
    // Atlas's answer is not taken before borealis asks the same name.
    open_project(&shell, "borealis", cx);
    let borealis = form_of("borealis", cx);
    create(&borealis, cx);
    until(&shell, cx, "both were answered", |cx| {
        atlas.read_with(cx, |f, _| f.creating().is_none())
            && borealis.read_with(cx, |f, _| f.creating().is_none())
    });

    assert!(
        !atlas.read_with(cx, |f, _| f.is_open()),
        "atlas's workspace was made"
    );
    let (open, error) = borealis.read_with(cx, |f, _| (f.is_open(), f.error().map(str::to_owned)));
    assert!(open, "borealis's form stays open");
    assert!(
        error.is_some_and(|e| e.contains("login")),
        "and says why it was refused"
    );
}

#[gpui_kit::test]
fn a_turn_ending_in_another_project_does_not_ask_for_details(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "borealis");
    let here = start(&user, "atlas", None, "Release notes for 2.4");
    let there = start(&user, "borealis", None, "Tidy the aliases");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the page has its details", |cx| {
        page_of(&shell, cx).read_with(cx, |page, _| page.chips()) == ["atlas · main"]
    });
    let asked = |cx: &mut VisualTestContext| shell.read_with(cx, |s, _| s.details_asked());
    let before = asked(cx);

    let turn = |id: &str, cx: &mut VisualTestContext| {
        send(&user, id, "work briefly");
        until(&shell, cx, "the session runs", |cx| {
            mark_of(&shell, id, cx) == Some(AgentState::Running)
        });
        until(&shell, cx, "its turn ended", |cx| {
            mark_of(&shell, id, cx).is_none()
        });
    };
    turn(&there, cx);
    assert_eq!(
        asked(cx),
        before,
        "borealis's work changes nothing on atlas's page"
    );

    turn(&here, cx);
    assert!(
        asked(cx) > before,
        "atlas's own work may have changed its lines"
    );
}

#[gpui_kit::test]
fn a_chosen_workspace_that_stops_being_ready_cannot_take_the_task(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the page shows login", |cx| {
        card_of(&shell, Some("login"), cx).is_some()
    });
    settle(cx);
    cx.update(|window, cx| window.click("new-session-in-login", cx));
    cx.simulate_input("Fix the timeout");
    let composer = composer_of(&shell, "atlas", cx);
    let places = |login_ready: bool| {
        vec![
            PlaceChoice {
                workspace: None,
                ready: true,
            },
            PlaceChoice {
                workspace: Some("login".into()),
                ready: login_ready,
            },
        ]
    };

    // What the next read says when login is prepared again.
    composer.update(cx, |c, cx| c.set_places(places(false), cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.click("send", cx));
    cx.simulate_keystrokes("enter");

    assert!(
        !composer.read_with(cx, |c, _| c.is_sending()),
        "nothing is sent to it, nor to main in its place"
    );
    assert_eq!(
        composer.read_with(cx, |c, _| (
            c.choice().clone(),
            c.blocked().map(str::to_owned)
        )),
        (
            Choice::Workspace("login".into()),
            Some("login is not ready: a session starts there once it is prepared".into())
        )
    );
    composer.update(cx, |c, cx| c.set_places(places(true), cx));
    assert_eq!(
        composer.read_with(cx, |c, _| c.blocked().map(str::to_owned)),
        None
    );
}

// The session page (#13): a lead and its workers, driven through their own MCP sessions as the
// fake agent would (`tests/tree.rs`), while the fake agent plays the turns (`HANG` keeps a worker
// mid-turn).

/// A user whose fake agent keeps a worker mid-turn when its brief says `HANG`, has it report when
/// the brief says `REPORT` (it then ends), and answers "ok" otherwise.
fn team_user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "REPORT", "do": [{"mcp": {"tool": "report", "arguments": {"text": "done"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The project `app`, its `.env` copied into its ready workspace `w`, and a lead the user started
/// there, its first turn ended: the lead's id, its MCP session and the workspace's folder.
fn lead_in_w(user: &User) -> (String, Mcp, PathBuf) {
    project(user, "app");
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[[recipe]]\ncopy = [\".env\"]\n");
    std::fs::write(&file, text).unwrap();
    workspace(user, "app", "w");
    let folder = user
        .call("workspace.show", json!({"project": "app", "name": "w"}))
        .unwrap()["folder"]
        .as_str()
        .unwrap()
        .to_owned();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let mcp = mcp_session(user, |prompt| prompt.contains("You are the root"));
    eventually("the lead's first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, mcp, PathBuf::from(folder))
}

/// The MCP session of the last agent session opened whose instructions `which` accepts.
fn mcp_session(user: &User, which: impl Fn(&str) -> bool) -> Mcp {
    let found = || {
        user.received_requests("session/new")
            .into_iter()
            .rev()
            .find(|opened| {
                which(
                    opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"][2]
                        .as_str()
                        .unwrap_or_default(),
                )
            })
    };
    eventually("the session", || found().is_some());
    let server = found().unwrap()["params"]["mcpServers"][0].clone();
    Mcp::initialize(
        server["url"].as_str().unwrap(),
        server["headers"][0]["value"].as_str().unwrap(),
    )
}

/// Launches a worker of the lead's in `w`, kept mid-turn: its id and its MCP session.
fn worker(user: &User, lead: &Mcp, title: &str) -> (String, Mcp) {
    let id = worker_doing(lead, "HANG", title);
    let task = format!("Your task: {title}.");
    (id, mcp_session(user, |prompt| prompt.contains(&task)))
}

/// Launches a worker of the lead's in `w` whose brief starts with `what`: its id.
fn worker_doing(lead: &Mcp, what: &str, title: &str) -> String {
    let answer = lead.call(
        "agent_launch",
        json!({"title": title, "brief": format!("{what} and {title}"), "workspace": "w"}),
    );
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_owned()
}

/// The number of the request a held call's answer names.
fn held_number(answer: common::mcp::Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    let rest = answer
        .text
        .split_once("request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
        .1
        .to_owned();
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// Opens the session of `lead` as a click on its row in the tree would, and waits for its page to
/// show its first rows.
fn open_session<'a>(
    user: &User,
    lead: &str,
    width: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), width, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    if width < 860. {
        cx.update(|window, cx| window.click("open-tree", cx));
        settle(cx);
    }
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(
        &shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            session_page(&shell, cx).read_with(cx, |page, cx| {
                !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
            })
        },
    );
    (shell, cx)
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn rail_ids(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<String> {
    page.read_with(cx, |page, cx| {
        page.rail()
            .read(cx)
            .agents()
            .iter()
            .map(|a| a.id.clone())
            .collect()
    })
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

#[gpui_kit::test]
fn the_rail_shows_a_lead_and_two_workers(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (first, _a) = worker(&user, &mcp, "build the form");
    let (second, _b) = worker(&user, &mcp, "write the tests");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);

    until(&shell, cx, "the rail shows the three agents", |cx| {
        rail_ids(&page, cx).len() == 3
    });
    settle(cx);
    assert_eq!(
        rail_ids(&page, cx),
        [lead.clone(), first.clone(), second.clone()],
        "the lead first"
    );
    cx.update(|window, _| {
        let x = |id: &str| window.find(format!("rail-row-{id}")).bounds().origin.x;
        assert!(
            x(&first) > x(&lead),
            "a worker sits below its lead, indented"
        );
        assert_eq!(x(&first), x(&second));
        assert!(window.try_find(format!("elbow-{first}")).is_some());
    });
    let (names, states) = page.read_with(cx, |page, cx| {
        let rail = page.rail().read(cx);
        (
            rail.agents()
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>(),
            rail.agents()
                .iter()
                .map(|a| a.status.read(cx).state())
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(names, ["lead", "build the form", "write the tests"]);
    assert_eq!(
        states,
        [AgentState::Idle, AgentState::Running, AgentState::Running]
    );
}

/// The names over the agents' replies of the conversation shown, `None` where there is none.
fn authors(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Option<String>> {
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .rows()
            .iter()
            .map(|row| row.read(cx))
            .filter(|row| matches!(row.block(), Block::Text { .. }))
            .map(|row| row.author().map(str::to_owned))
            .collect()
    })
}

#[gpui_kit::test]
fn a_root_agent_s_reply_has_no_label_while_a_child_s_has_its_name(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let answer = mcp.call(
        "agent_launch",
        json!({"title": "draft the notes", "brief": "draft the notes", "workspace": "w"}),
    );
    assert!(!answer.error, "{}", answer.text);
    let child = answer
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_owned();
    eventually("the child replied", || {
        user.call("agent.show", json!({"agent": child})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the child is in the rail", |cx| {
        rail_ids(&page, cx).contains(&child)
    });
    settle(cx);

    let root = authors(&page, cx);
    assert!(!root.is_empty(), "the root replied");
    assert!(root.iter().all(Option::is_none), "{root:?}");
    cx.update(|window, _| {
        assert!(
            window.try_find(format!("rail-name-{lead}")).is_none(),
            "no name on the root"
        );
        assert!(window.try_find(format!("rail-runs-on-{lead}")).is_some());
        assert!(
            window.try_find(format!("rail-name-{child}")).is_some(),
            "a child keeps its name"
        );
    });
    let runs_on = page.read_with(cx, |p, cx| {
        let agent = &p.rail().read(cx).agents()[0];
        flottille_window::session::runs_on(&agent.provider, agent.model.as_deref())
    });
    assert!(runs_on.starts_with("claude · "), "{runs_on}");

    cx.update(|window, cx| window.click(format!("rail-row-{child}"), cx));
    until(&shell, cx, "the child's reply is shown", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == child)
            && !authors(&page, cx).is_empty()
    });
    assert!(
        authors(&page, cx)
            .iter()
            .all(|name| name.as_deref() == Some("draft the notes")),
        "{:?}",
        authors(&page, cx)
    );
}

#[gpui_kit::test]
fn a_message_arriving_appends_without_rebuilding_other_rows(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    send(&user, &lead, "and the docs");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "both turns are shown", |cx| {
        blocks(&page, cx).len() == 4
    });
    settle(cx);
    let rows = page.read_with(cx, |p, cx| p.conversation().read(cx).rows().to_vec());
    let renders = |cx: &mut VisualTestContext| {
        rows.iter()
            .map(|row| row.read_with(cx, |row, _| row.renders()))
            .collect::<Vec<_>>()
    };
    let before = renders(cx);
    assert!(before.iter().all(|r| *r >= 1), "every row was drawn");

    send(&user, &lead, "and the changelog");
    until(
        &shell,
        cx,
        "the new message and its answer are shown",
        |cx| blocks(&page, cx).len() == 6,
    );
    settle(cx);

    assert_eq!(
        blocks(&page, cx)[4],
        Block::Message {
            from: "you".into(),
            text: "and the changelog".into()
        }
    );
    assert_eq!(renders(cx), before, "no row already there is drawn again");
}

#[gpui_kit::test]
fn replying_delivers_to_the_lead(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    settle(cx);

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("Use the staging database");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("and say when done");
    cx.simulate_keystrokes("enter");

    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    until(&shell, cx, "the daemon took the message", |cx| {
        reply.read_with(cx, |r, cx| {
            !r.is_sending() && r.field().read(cx).value().is_empty()
        })
    });
    assert_eq!(
        reply.read_with(cx, |r, _| r.error().map(str::to_owned)),
        None
    );
    let delivered = user
        .received_requests("session/prompt")
        .iter()
        .filter_map(|p| p["params"]["prompt"][0]["text"].as_str().map(str::to_owned))
        .any(|text| text.contains("Use the staging database\nand say when done"));
    assert!(
        delivered,
        "the lead was sent the message, its new line kept"
    );
    let page = session_page(&shell, cx);
    until(&shell, cx, "the message is in the conversation", |cx| {
        blocks(&page, cx).contains(&Block::Message {
            from: "you".into(),
            text: "Use the staging database\nand say when done".into(),
        })
    });
}

#[gpui_kit::test]
fn selecting_a_worker_shows_its_conversation_with_no_composer_and_nothing_can_send_to_it(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (worker_id, _worker) = worker(&user, &mcp, "build the form");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the worker is in the rail", |cx| {
        rail_ids(&page, cx).contains(&worker_id)
    });
    settle(cx);
    // The lead's field has the focus when the worker is selected.
    cx.update(|window, cx| window.click("reply-field", cx));

    cx.update(|window, cx| window.click(format!("rail-row-{worker_id}"), cx));
    until(&shell, cx, "the worker's brief is shown", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == worker_id)
            && !blocks(&page, cx).is_empty()
    });
    settle(cx);

    match &blocks(&page, cx)[0] {
        Block::Message { from, text } => {
            assert_eq!(from, "the conversation → build the form");
            assert!(text.contains("HANG and build the form"), "{text}");
        }
        other => panic!("the brief comes first, not {other:?}"),
    }
    cx.update(|window, _| {
        for id in ["reply", "reply-field", "reply-send"] {
            assert!(window.try_find(id).is_none(), "{id} is shown");
        }
        assert!(window.try_find("read-only").is_some());
    });
    // No way to send to it: no composer of its own, and the only composer writes to the lead.
    assert!(page.read_with(cx, |p, _| p.reply().is_none()));
    assert!(shell.read_with(cx, |s, _| s.reply_of(&worker_id).is_none()));
    let to = shell.read_with(cx, |s, cx| {
        s.reply_of(&lead).map(|r| r.read(cx).session().to_owned())
    });
    assert_eq!(to.as_deref(), Some(lead.as_str()));
    // Typed while the worker is read, Enter pressed, and everything that follows run.
    cx.simulate_input("are you there?");
    cx.simulate_keystrokes("enter");
    let deadline = Instant::now() + Duration::from_millis(500);
    until(&shell, cx, "half a second has passed", |_| {
        Instant::now() > deadline
    });
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    assert!(
        !reply.read_with(cx, |r, _| r.is_sending()),
        "nothing was sent"
    );
    let delivered = user.call("agent.show", json!({"agent": lead})).unwrap();
    let prompts = user.received_requests("session/prompt");
    assert!(
        !prompts
            .iter()
            .any(|p| p.to_string().contains("are you there?"))
            && !delivered.to_string().contains("are you there?"),
        "nothing typed reached an agent"
    );

    // Back to the lead: its conversation and its composer.
    cx.update(|window, cx| window.click("back-to-root", cx));
    until(&shell, cx, "the lead's conversation is back", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == lead)
    });
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("reply").is_some()));
}

#[gpui_kit::test]
fn a_worker_s_request_held_by_the_lead_waits_on_it_then_forwarded_needs_me_with_its_card(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (worker_id, worker) = worker(&user, &mcp, "read the settings");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);

    // The worker hits an ask: its lead holds it.
    let number = held_number(worker.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the worker waits on its lead", |cx| {
        page.read_with(cx, |p, cx| p.state(&worker_id, cx)) == Some(AgentState::Waiting)
    });
    settle(cx);
    assert_ne!(
        page.read_with(cx, |p, cx| p.state(&lead, cx)),
        Some(AgentState::NeedsYou)
    );
    assert_ne!(mark_of(&shell, &lead, cx), Some(AgentState::NeedsYou));
    cx.update(|window, _| {
        assert!(window.try_find(format!("approval-{number}")).is_none());
    });

    // The lead forwards it: it is the user's now.
    let answer = mcp.call(
        "approval_answer",
        json!({"request_id": number, "decision": "forward", "why": "the form needs the database name"}),
    );
    assert!(!answer.error, "{}", answer.text);
    until(
        &shell,
        cx,
        "the lead and the session need me, with the card",
        |cx| {
            page.read_with(cx, |p, cx| p.state(&lead, cx)) == Some(AgentState::NeedsYou)
                && mark_of(&shell, &lead, cx) == Some(AgentState::NeedsYou)
                && cx.update(|window, _| window.try_find(format!("allow-{number}")).is_some())
        },
    );
    assert_eq!(
        page.read_with(cx, |p, cx| p.state(&worker_id, cx)),
        Some(AgentState::Waiting),
        "only the root needs me: its worker still waits, in grey"
    );
    let card = page
        .read_with(cx, |p, cx| {
            p.conversation()
                .read(cx)
                .rows()
                .iter()
                .find_map(|row| row.read(cx).card().cloned())
        })
        .unwrap();
    assert_eq!(card.asked.as_deref(), Some("Read .env"));
    let why = card.why.unwrap();
    assert!(
        why.contains("the conversation forwards it: the form needs the database name"),
        "{why}"
    );
    assert_eq!(
        card.source.as_deref(),
        Some(
            format!("asked by read the settings · forwarded by the conversation · #{number}")
                .as_str()
        )
    );
}

#[gpui_kit::test]
fn approving_once_from_the_card_performs_the_call_and_leaves_the_record(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the card is shown", |cx| {
        cx.update(|window, _| window.try_find(format!("allow-{number}")).is_some())
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("allow-{number}"), cx));
    until(&shell, cx, "the card folds into its record", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("approval-record-{number}"))
                .is_some()
                && window.try_find(format!("allow-{number}")).is_none()
        })
    });
    let record = session_page(&shell, cx).read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .rows()
            .iter()
            .find_map(|row| row.read(cx).card().and_then(|c| c.record()))
    });
    assert_eq!(
        record.as_deref(),
        Some("Allowed · Read .env · the conversation")
    );
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // The call was made: its result wakes the idle lead, masked.
    let result = || {
        user.received_requests("session/prompt")
            .iter()
            .filter_map(|p| p["params"]["prompt"][0]["text"].as_str().map(str::to_owned))
            .find(|text| {
                text.contains(&format!(
                    "Request #{number} (fs_read) was approved by the user"
                ))
            })
    };
    eventually("the result", || result().is_some());
    let seen = result().unwrap();
    assert!(seen.contains("TOKEN"), "{seen}");
    assert!(!seen.contains("made-up-made-up"), "{seen}");
}

/// The card of a request, once the session page shows it.
fn card_for(
    shell: &Entity<Shell>,
    number: u64,
    cx: &mut VisualTestContext,
) -> flottille_window::session::ApprovalCard {
    until(shell, cx, "the card is shown", |cx| {
        cx.update(|window, _| window.try_find(format!("allow-{number}")).is_some())
    });
    settle(cx);
    session_page(shell, cx)
        .read_with(cx, |p, cx| {
            p.conversation()
                .read(cx)
                .rows()
                .iter()
                .find_map(|row| row.read(cx).card().filter(|c| c.number == number).cloned())
        })
        .unwrap()
}

#[gpui_kit::test]
fn the_card_of_a_command_an_agent_adds_says_what_it_writes_and_is_confirmed_once(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);

    let number = held_number(mcp.call(
        "commands",
        json!({"action": "add", "name": "ship", "description": "Ship the workspace",
            "instructions": "Push the branch."}),
    ));
    let card = card_for(&shell, number, cx);

    assert_eq!(card.asked.as_deref(), Some("Add the command ship"));
    assert_eq!(
        card.says,
        [
            "Add the command /ship: Ship the workspace",
            "Its instructions: Push the branch.",
        ]
    );
    cx.update(|window, _| {
        assert!(window.find(format!("approval-says-{number}-1")).visible());
    });
    cx.update(|window, cx| window.click(format!("allow-{number}"), cx));
    until(&shell, cx, "the card folds into its record", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("approval-record-{number}"))
                .is_some()
        })
    });
    eventually("the command is written", || {
        user.call("command.show", json!({"project": "app", "name": "ship"}))
            .is_ok_and(|shown| shown["instructions"] == "Push the branch.")
    });
}

#[gpui_kit::test]
fn the_card_of_a_settings_change_or_a_skill_says_what_changes(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);

    let tracker = held_number(mcp.call(
        "project",
        json!({"action": "set_settings", "tracker": {"kind": "jira", "site": "acme.atlassian.net",
            "projects": ["ABC"]}}),
    ));
    let card = card_for(&shell, tracker, cx);
    assert_eq!(card.asked.as_deref(), Some("Change the project's settings"));
    assert_eq!(
        card.says,
        ["Set the tracker to Jira on acme.atlassian.net (ABC)"]
    );
    let skill = held_number(mcp.call(
        "skills",
        json!({"action": "add", "name": "readme", "description": "What the app is",
            "path": "README.md"}),
    ));
    let card = card_for(&shell, skill, cx);
    assert_eq!(card.asked.as_deref(), Some("Add the skill readme"));
    assert_eq!(
        card.says,
        ["Add the skill readme (README.md): What the app is"]
    );
    cx.update(|window, _| {
        assert!(window.find(format!("approval-says-{skill}-0")).visible());
    });
}

/// A lead in the project folder of `app`, its MCP session, and its session open in the window.
fn lead_in_main<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (String, Mcp, Entity<Shell>, &'a mut VisualTestContext) {
    project(user, "app");
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "lead the work"}),
        )
        .unwrap();
    let lead = answer["agent"]["id"].as_str().unwrap().to_owned();
    let mcp = mcp_session(user, |prompt| prompt.contains("You are the root"));
    eventually("the lead's first turn", || {
        user.call("agent.show", json!({"agent": lead})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = open_session(user, &lead, 1280., cx);
    (lead, mcp, shell, cx)
}

#[gpui_kit::test]
fn writing_in_main_in_ask_makes_a_card_to_judge_with_allow_and_deny_only(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, shell, cx) = lead_in_main(&user, cx);
    user.call("agent.mode", json!({"agent": lead, "mode": "ask"}))
        .unwrap();

    let number = held_number(mcp.call(
        "fs_write",
        json!({"path": "notes.txt", "content": "made-up notes"}),
    ));
    let card = card_for(&shell, number, cx);

    assert!(!card.base);
    assert_eq!(card.judgement, None, "in Ask, no judge is asked");
    assert_eq!(
        text_of(&format!("approval-class-{number}"), cx).as_deref(),
        Some("To judge")
    );
    cx.update(|window, _| {
        assert!(window.find(format!("allow-{number}")).visible());
        assert!(window.find(format!("deny-{number}")).visible());
        assert!(window.try_find(format!("approval-base-{number}")).is_none());
    });
    cx.update(|window, cx| window.click(format!("allow-{number}"), cx));
    until(&shell, cx, "the card folds into its record", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("approval-record-{number}"))
                .is_some()
        })
    });
    eventually("the file is written", || {
        std::fs::read_to_string(user.path("work/app/notes.txt")).is_ok_and(|t| t == "made-up notes")
    });
}

/// What the mode switch of the open session's composer shows, and the sentence beside it.
fn mode_shown(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (Option<String>, Option<String>) {
    let page = session_page(shell, cx);
    page.read_with(cx, |page, cx| {
        let mode = page.reply().unwrap().read(cx).mode.read(cx);
        (mode.mode().map(str::to_owned), mode.said())
    })
}

#[gpui_kit::test]
fn the_mode_switch_shows_auto_says_once_to_choose_a_judge_and_puts_the_session_in_ask(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, _mcp, shell, cx) = lead_in_main(&user, cx);
    let no_judge = Some(flottille_window::mode::NO_JUDGE.to_owned());
    until(
        &shell,
        cx,
        "a new session shows Auto, without a judge",
        |cx| mode_shown(&shell, cx) == (Some("auto".into()), no_judge.clone()),
    );
    cx.update(|window, _| {
        assert!(window.find("reply-mode-switch").visible());
        assert!(window.find("reply-mode-said").visible());
    });

    cx.update(|window, cx| window.click("reply-mode-switch", cx));
    eventually("the daemon puts the session in Ask", || {
        user.call("agent.show", json!({"agent": lead})).unwrap()["agent"]["gate_mode"] == "ask"
    });
    until(&shell, cx, "the switch shows Ask, and no sentence", |cx| {
        mode_shown(&shell, cx) == (Some("ask".into()), None)
    });

    // The command line changes it too, and the window follows.
    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();
    until(&shell, cx, "the switch follows the daemon", |cx| {
        mode_shown(&shell, cx).0.as_deref() == Some("auto")
    });
}

#[gpui_kit::test]
fn the_auto_judge_is_chosen_in_settings_which_say_it_uses_the_quota(cx: &mut TestAppContext) {
    let user = providers_user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "the judge's picker shows none", |cx| {
        page.read_with(cx, |page, _| page.judge_shown().0 == "None")
    });
    cx.update(|window, _| assert!(window.find("judge-provider").visible()));
    assert_eq!(
        text_of("judge-quota", cx).as_deref(),
        Some(flottille_window::settings::JUDGE_QUOTA)
    );

    cx.update(|window, cx| window.click("judge-provider", cx));
    cx.update(|window, cx| window.click("judge-choice-opencode", cx));
    until(&shell, cx, "the page shows the judge chosen", |cx| {
        page.read_with(cx, |page, _| {
            page.judge_shown() == ("OpenCode".into(), Some("default model".into()))
        })
    });
    let file = std::fs::read_to_string(user.path("config/flottille/providers.toml")).unwrap();
    assert!(
        file.contains("[judge]") && file.contains("provider = \"opencode\""),
        "{file}"
    );
    cx.update(|window, _| assert!(window.find("judge-model").visible()));

    cx.update(|window, cx| window.click("judge-provider", cx));
    cx.update(|window, cx| window.click("judge-choice-none", cx));
    until(&shell, cx, "the page shows no judge again", |cx| {
        page.read_with(cx, |page, _| page.judge_shown().0 == "None")
    });
    let file = std::fs::read_to_string(user.path("config/flottille/providers.toml")).unwrap();
    assert!(!file.contains("[judge]"), "{file}");
}

#[gpui_kit::test]
fn a_base_card_in_auto_is_distinct_and_says_why_it_always_comes_back(cx: &mut TestAppContext) {
    let user = team_user();
    let (_lead, mcp, shell, cx) = lead_in_main(&user, cx);

    let number = held_number(mcp.call("commands_run", json!({"line": "cat .env"})));
    let card = card_for(&shell, number, cx);

    assert!(card.base);
    assert_eq!(
        text_of(&format!("approval-class-{number}"), cx).as_deref(),
        Some("Base")
    );
    assert_eq!(
        text_of(&format!("approval-base-{number}"), cx).as_deref(),
        Some(flottille_window::session::BASE_SAYS)
    );
    cx.update(|window, _| {
        assert!(window.find(format!("approval-base-{number}")).visible());
        assert!(window.find(format!("allow-{number}")).visible());
        assert!(window.find(format!("deny-{number}")).visible());
    });
}

#[gpui_kit::test]
fn a_child_appearing_adds_its_row_to_the_rail_and_the_sub_agents_list(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);
    assert_eq!(rail_ids(&page, cx), std::slice::from_ref(&lead));
    assert!(!blocks(&page, cx).contains(&Block::SubAgents));

    let (child, _worker) = worker(&user, &mcp, "build the form");
    until(
        &shell,
        cx,
        "the child is in the rail and in the list",
        |cx| {
            rail_ids(&page, cx) == [lead.clone(), child.clone()]
                && cx.update(|window, _| window.try_find(format!("sub-agent-{child}")).is_some())
        },
    );
    let listed = page.read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .rows()
            .iter()
            .flat_map(|row| row.read(cx).sub_agents().to_vec())
            .map(|a| (a.id, a.name))
            .collect::<Vec<_>>()
    });
    assert_eq!(listed, [(child.clone(), "build the form".to_owned())]);

    // Selecting it from the list shows its conversation.
    settle(cx);
    cx.update(|window, cx| window.click(format!("sub-agent-{child}"), cx));
    until(&shell, cx, "the child's conversation", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == child)
    });
}

#[gpui_kit::test]
fn the_header_has_no_editor_button_and_the_workspace_menu_opens_the_place_s_folder_and_closes_on_escape(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, _mcp, folder) = lead_in_w(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let given: Arc<std::sync::Mutex<Vec<PathBuf>>> = Arc::default();
    let opened = given.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            opened.lock().unwrap().push(path.to_owned())
        }))
    });
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    settle(cx);

    cx.update(|window, _| {
        assert!(window.try_find("session-header").is_some());
        assert!(
            window.try_find("open-in-editor").is_none(),
            "no editor button, in the header or anywhere while the menu is shut"
        );
        assert!(window.try_find("workspace-menu").is_some(), "the ⋯ button");
    });

    // It opens animated: it starts below its place and rises into it.
    cx.update(|window, cx| window.click("workspace-menu", cx));
    let entering = cx.update(|window, _| window.find("workspace-menu-menu").bounds().origin.y);
    settle(cx);
    let at_rest = cx.update(|window, _| window.find("workspace-menu-menu").bounds().origin.y);
    assert!(entering > at_rest, "{entering:?} then {at_rest:?}");
    cx.update(|window, _| assert!(window.find("open-in-editor").visible()));

    // Escape closes it, animated too, and nothing was opened.
    cx.simulate_keystrokes("escape");
    cx.update(|window, _| {
        assert!(
            window.try_find("open-in-editor").is_some(),
            "still on its way out"
        )
    });
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("open-in-editor").is_none()));
    assert!(given.lock().unwrap().is_empty());

    // A click outside it closes it too.
    cx.update(|window, cx| window.click("workspace-menu", cx));
    settle(cx);
    cx.update(|window, cx| window.click("session-header", cx));
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("open-in-editor").is_none()));

    // The entry opens the place's folder with the system's handler, and shuts the menu.
    cx.update(|window, cx| window.click("workspace-menu", cx));
    settle(cx);
    cx.update(|window, cx| window.click("open-in-editor", cx));
    settle(cx);
    assert_eq!(*given.lock().unwrap(), [folder]);
    cx.update(|window, _| assert!(window.try_find("open-in-editor").is_none()));
}

#[gpui_kit::test]
fn ended_children_are_gathered_under_a_folded_row_that_opens_on_a_click(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let done = worker_doing(&mcp, "REPORT", "write the notes");
    let (stopped, _s) = worker(&user, &mcp, "try the idea");
    let (running, _r) = worker(&user, &mcp, "build the form");
    let stop = mcp.call("agent_stop", json!({"agent_id": stopped}));
    assert!(!stop.error, "{}", stop.text);
    eventually("both have ended", || {
        [&done, &stopped].iter().all(|id| {
            user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "ended"
        })
    });
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail knows the three children", |cx| {
        rail_ids(&page, cx).len() == 4
            && page.read_with(cx, |p, cx| {
                [&done, &stopped].iter().all(|id| {
                    matches!(
                        p.state(id, cx),
                        Some(AgentState::Completed | AgentState::Stopped)
                    )
                })
            })
    });
    settle(cx);

    // The running child keeps its place; the two ended ones are behind one folded row.
    cx.update(|window, _| {
        assert!(window.try_find(format!("rail-row-{lead}")).is_some());
        assert!(window.try_find(format!("rail-row-{running}")).is_some());
        assert!(window.try_find(format!("rail-row-{done}")).is_none());
        assert!(window.try_find(format!("rail-row-{stopped}")).is_none());
        assert!(window.try_find(format!("rail-ended-{lead}")).is_some());
    });
    assert_eq!(
        page.read_with(cx, |p, cx| p.rail().read(cx).rows()),
        [
            RailRow::Agent(lead.clone()),
            RailRow::Agent(running.clone()),
            RailRow::Ended {
                parent: lead.clone(),
                count: 2,
                open: false
            }
        ]
    );

    // A click opens it, and the ended children come back; a second click folds it again.
    cx.update(|window, cx| window.click(format!("rail-ended-{lead}"), cx));
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find(format!("rail-row-{done}")).is_some());
        assert!(window.try_find(format!("rail-row-{stopped}")).is_some());
    });
    cx.update(|window, cx| window.click(format!("rail-ended-{lead}"), cx));
    settle(cx);
    cx.update(|window, _| assert!(window.try_find(format!("rail-row-{done}")).is_none()));

    // An ended child shown in the conversation stays reachable from the row.
    cx.update(|window, cx| window.click(format!("rail-ended-{lead}"), cx));
    settle(cx);
    cx.update(|window, cx| window.click(format!("rail-row-{stopped}"), cx));
    until(&shell, cx, "the ended child's conversation", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == stopped)
    });
}

/// Runs the clock `by` forward, a frame at a time, as the platform would.
fn run_for(by: Duration, cx: &mut VisualTestContext) {
    let step = Duration::from_millis(16);
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(step);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += step;
    }
}

/// The height the ended children of `parent` take in the rail, once a frame is drawn; `None`
/// when they are not drawn at all.
fn ended_height(parent: &str, cx: &mut VisualTestContext) -> Option<f32> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        let group = window.try_find(format!("rail-ended-children-{parent}"))?;
        Some(f32::from(group.bounds().size.height))
    })
}

#[gpui_kit::test]
fn the_ended_row_opens_and_folds_its_children_with_an_animation(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let done = worker_doing(&mcp, "REPORT", "write the notes");
    eventually("it has ended", || {
        user.call("agent.show", json!({"agent": done})).unwrap()["agent"]["state"] == "ended"
    });
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail knows the ended child", |cx| {
        rail_ids(&page, cx).contains(&done)
            && page.read_with(cx, |p, cx| {
                matches!(
                    p.state(&done, cx),
                    Some(AgentState::Completed | AgentState::Stopped)
                )
            })
    });
    settle(cx);
    assert_eq!(ended_height(&lead, cx), None, "folded, nothing is drawn");

    // Opening: the child grows into place, then the rail asks for no more frames.
    cx.update(|window, cx| window.click(format!("rail-ended-{lead}"), cx));
    let first = ended_height(&lead, cx).expect("drawn from the first frame");
    run_for(Duration::from_millis(60), cx);
    let part_way = ended_height(&lead, cx).unwrap();
    run_for(Duration::from_millis(600), cx);
    let open = ended_height(&lead, cx).unwrap();
    assert!(
        first < part_way && part_way < open,
        "it grows: {first} then {part_way} then {open}"
    );
    assert!(open > 20., "a whole row: {open}");
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);

    // Folding: still drawn part way, shrinking, gone once the animation ended.
    cx.update(|window, cx| window.click(format!("rail-ended-{lead}"), cx));
    run_for(Duration::from_millis(60), cx);
    let leaving = ended_height(&lead, cx).expect("still drawn, leaving");
    assert!(
        leaving < open && leaving > 0.,
        "it shrinks: {leaving} of {open}"
    );
    run_for(Duration::from_millis(600), cx);
    assert_eq!(ended_height(&lead, cx), None, "gone once the fold ended");
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

fn row_ids(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<gpui_kit::EntityId> {
    page.read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .rows()
            .iter()
            .map(|row| row.entity_id())
            .collect()
    })
}

#[gpui_kit::test]
fn a_session_whose_workspace_is_removed_keeps_its_page_without_its_facts_and_reads_nothing_again(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the workspace's facts", |cx| {
        page.read_with(cx, |p, cx| {
            p.rail()
                .read(cx)
                .place()
                .is_some_and(|place| !place.repositories.is_empty())
        })
    });
    let rows = row_ids(&page, cx);
    assert_eq!(page.read_with(cx, |p, _| p.histories()), 1);

    user.call("workspace.remove", json!({"project": "app", "name": "w"}))
        .unwrap();
    until(&shell, cx, "the facts are gone", |cx| {
        page.read_with(cx, |p, cx| p.rail().read(cx).place().is_none())
    });
    // A turn ending reads the place again: the daemon does not know it any more.
    send(&user, &lead, "still there?");
    until(&shell, cx, "the lead answered", |cx| {
        blocks(&page, cx).len() == rows.len() + 2
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    until(&shell, cx, "a second has passed", |_| {
        Instant::now() > deadline
    });

    assert!(page.read_with(cx, |p, cx| p.rail().read(cx).place().is_none()));
    assert_eq!(
        page.read_with(cx, |p, _| p.histories()),
        1,
        "the conversation was read once"
    );
    assert_eq!(
        row_ids(&page, cx)[..rows.len()],
        rows[..],
        "the rows already there were kept"
    );
}

#[gpui_kit::test]
fn a_card_appears_while_another_session_streams_without_pause(cx: &mut TestAppContext) {
    let user = team_user();
    let pieces: Vec<serde_json::Value> = (0..3000)
        .flat_map(|n| [json!({"say": format!("{n} ")}), json!({"wait": 10})])
        .collect();
    user.fake_script(json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "STREAM", "do": pieces}
    ]}));
    let (lead, mcp, _) = lead_in_w(&user);
    let streamer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "STREAM for a while"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the other session streams", || {
        user.call("agent.show", json!({"agent": streamer})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "agent.message")
            .count()
            > 20
    });
    let (shell, cx) = open_session(&user, &lead, 1280., cx);

    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    let asked = Instant::now();
    until(&shell, cx, "the card is shown", |cx| {
        cx.update(|window, _| window.try_find(format!("allow-{number}")).is_some())
    });

    let state =
        user.call("agent.show", json!({"agent": streamer})).unwrap()["agent"]["state"].clone();
    assert_eq!(state, "working", "the other session still streams");
    assert!(
        asked.elapsed() < Duration::from_secs(5),
        "{:?}",
        asked.elapsed()
    );
}

/// Events of the shown agent made up for the page, numbered from `seq`.
fn made_up(seq: u64, name: &str, data: serde_json::Value) -> flottille_core::protocol::Event {
    flottille_core::protocol::Event {
        seq,
        at: seq,
        name: name.into(),
        data,
    }
}

/// Gives the page a conversation of `turns` messages and replies, as its link would.
fn read_turns(page: &Entity<SessionPage>, turns: u64, cx: &mut VisualTestContext) {
    let events = (0..turns)
        .flat_map(|n| {
            [
                made_up(
                    2 * n + 1,
                    "agent.prompted",
                    json!({"text": format!("[flottille:user]\nmessage {n}")}),
                ),
                made_up(
                    2 * n + 2,
                    "agent.message",
                    json!({"text": format!("reply {n}")}),
                ),
            ]
        })
        .collect();
    page.update(cx, |p, cx| {
        let token = p.token().unwrap();
        p.apply(
            token,
            flottille_window::session_link::SessionUpdate::History {
                events,
                earlier: false,
            },
            cx,
        )
    });
}

#[gpui_kit::test]
fn a_streamed_piece_costs_one_row_however_long_the_conversation(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);
    let mut costs = Vec::new();
    for turns in [5, 500] {
        read_turns(&page, turns, cx);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let rows = row_ids(&page, cx).len();
        assert_eq!(rows as u64, 2 * turns);
        let touched = |cx: &mut VisualTestContext| {
            page.read_with(cx, |p, cx| p.conversation().read(cx).touched())
        };
        let (before, started) = (touched(cx), Instant::now());
        let mut drawn = Duration::ZERO;
        for piece in 0..100 {
            page.update(cx, |p, cx| {
                let token = p.token().unwrap();
                p.apply(
                    token,
                    flottille_window::session_link::SessionUpdate::Live(made_up(
                        10_000 + piece,
                        "agent.message",
                        json!({"text": " more"}),
                    )),
                    cx,
                )
            });
            let frame = Instant::now();
            cx.update(|window, cx| window.simulate_next_frame(cx));
            drawn += frame.elapsed();
        }
        let applied = started.elapsed() - drawn;
        assert_eq!(
            touched(cx) - before,
            100,
            "one row per piece, with {rows} rows"
        );
        costs.push((rows, applied / 100, drawn / 100));
    }
    for (rows, applied, drawn) in costs {
        eprintln!("{rows} rows: {applied:?} to apply a piece, {drawn:?} to draw its frame");
    }
}

#[gpui_kit::test]
fn a_call_left_running_shows_interrupted_when_the_conversation_is_read(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    page.update(cx, |p, cx| {
        let token = p.token().unwrap();
        p.apply(
            token,
            flottille_window::session_link::SessionUpdate::History {
                events: vec![
                    made_up(1, "agent.thought", json!({"text": "Let me look"})),
                    made_up(
                        2,
                        "agent.tool_call",
                        json!({"id": "t", "title": "fs_read", "status": "in_progress"}),
                    ),
                    made_up(9, "agent.turn_ended", json!({"stop_reason": "cancelled"})),
                ],
                earlier: false,
            },
            cx,
        )
    });
    settle(cx);

    let blocks = blocks(&page, cx);
    let Block::Steps { calls } = &blocks[1] else {
        panic!("{blocks:?}");
    };
    assert_eq!(
        calls[0].state,
        flottille_window::conversation::CallState::Interrupted
    );
    assert!(matches!(blocks[0], Block::Thought { until: Some(_), .. }));
    let marks = page.read_with(cx, |p, cx| {
        p.conversation().read(cx).rows()[1].read(cx).marks(cx)
    });
    assert_eq!(marks, [AgentState::Stopped], "no spinner at rest");
    cx.update(|window, cx| {
        assert_eq!(
            window.find("row-1-steps-toggle").label(),
            Some("1 step · fs_read")
        );
        assert!(window.try_find("step-1-0").is_none(), "folded");
        window.click("row-1-steps-toggle", cx);
    });
    frame(cx);
    cx.update(|window, _| assert!(window.try_find("step-1-0").is_some()));
}

#[gpui_kit::test]
fn an_answered_request_keeps_its_record_when_the_page_is_opened_again(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (_worker_id, worker) = worker(&user, &mcp, "read the settings");
    let forwarded = held_number(worker.call("fs_read", json!({"path": ".env"})));
    let answer = mcp.call(
        "approval_answer",
        json!({"request_id": forwarded, "decision": "forward", "why": "it needs the name"}),
    );
    assert!(!answer.error, "{}", answer.text);
    user.call("approval.deny", json!({"number": forwarded}))
        .unwrap();
    let own = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    user.call("approval.approve", json!({"number": own}))
        .unwrap();

    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    until(&shell, cx, "both records are shown", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("approval-record-{forwarded}"))
                .is_some()
                && window.try_find(format!("approval-record-{own}")).is_some()
        })
    });
    let records = session_page(&shell, cx).read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .rows()
            .iter()
            .filter_map(|row| row.read(cx).card().and_then(|c| c.record()))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        records,
        [
            "Denied · Read .env · read the settings",
            "Allowed · Read .env · the conversation"
        ]
    );
}

#[gpui_kit::test]
fn a_long_conversation_shows_its_last_page_and_earlier_ones_on_demand(cx: &mut TestAppContext) {
    let user = team_user();
    let pieces: Vec<serde_json::Value> =
        (0..1100).map(|n| json!({"say": format!("{n} ")})).collect();
    user.fake_script(json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "talk", "do": pieces}
    ]}));
    let (lead, mcp, _) = lead_in_w(&user);
    let (child, _worker) = worker(&user, &mcp, "build the form");
    send(&user, &lead, "talk at length");
    eventually("the long turn ends", || {
        user.call("agent.show", json!({"agent": lead, "before": u64::MAX}))
            .unwrap()["events"]
            .as_array()
            .unwrap()
            .last()
            .is_some_and(|e| e["name"] == "agent.turn_ended")
    });
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the sub-agents' list", |cx| {
        blocks(&page, cx).contains(&Block::SubAgents)
    });
    settle(cx);

    assert!(page.read_with(cx, |p, cx| p.conversation().read(cx).has_earlier()));
    let shown = blocks(&page, cx);
    assert_eq!(
        shown[0],
        Block::SubAgents,
        "launched before the page read: first, not where the tree came"
    );
    assert!(
        !shown.contains(&Block::Message {
            from: "you".into(),
            text: "lead the work".into()
        }),
        "the first message is on an earlier page"
    );
    assert!(matches!(shown.last(), Some(Block::Text { text }) if text.ends_with("1099 ")));

    // Scrolled up to the start of what is shown, as a reader would.
    let middle = cx.update(|window, _| {
        window
            .find(format!("conversation-{lead}"))
            .bounds()
            .center()
    });
    for _ in 0..20 {
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: middle,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(2000.))),
            ..Default::default()
        });
        settle(cx);
    }
    cx.update(|window, cx| window.click("show-earlier", cx));
    until(&shell, cx, "the earlier page", |cx| {
        blocks(&page, cx)[0]
            == Block::Message {
                from: "you".into(),
                text: "lead the work".into(),
            }
    });
    settle(cx);
    assert!(!page.read_with(cx, |p, cx| p.conversation().read(cx).has_earlier()));
    let listed = page.read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .rows()
            .iter()
            .flat_map(|row| row.read(cx).sub_agents().to_vec())
            .map(|a| a.id)
            .collect::<Vec<_>>()
    });
    assert_eq!(listed, [child]);
}

#[gpui_kit::test]
fn an_answer_given_on_one_session_s_page_is_not_applied_to_another(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let other = start(&user, "app", None, "another task");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the card is shown", |cx| {
        cx.update(|window, _| window.try_find(format!("allow-{number}")).is_some())
    });
    settle(cx);

    // Answered, then another session opened before the daemon's answer is taken.
    cx.update(|window, cx| window.click(format!("allow-{number}"), cx));
    cx.update(|window, cx| window.click(session_id(&other), cx));
    eventually("the request is decided", || {
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    until(&shell, cx, "the other session's page", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.id() == other)
    });
    settle(cx);
    let page = session_page(&shell, cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.decision(number).map(str::to_owned)),
        None
    );

    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the record on its own page", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("approval-record-{number}"))
                .is_some()
        })
    });
}

#[gpui_kit::test]
fn text_typed_while_a_reply_sends_is_kept(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    settle(cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("first");
    cx.simulate_keystrokes("enter");
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    assert!(reply.read_with(cx, |r, _| r.is_sending()));

    cx.simulate_input(" and then");
    until(&shell, cx, "the daemon took the message", |cx| {
        reply.read_with(cx, |r, _| !r.is_sending())
    });

    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "and then"
    );
    let prompts = user.received_requests("session/prompt");
    assert!(prompts.iter().any(|p| p.to_string().contains("first")));
}

// Providers (#27).

/// A user whose Claude and OpenCode each stand on a fake ACP agent, both enabled. OpenCode's lead
/// launches a helper on Claude when told to.
fn providers_user() -> User {
    let user = User::with_fake_providers();
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}),
    );
    user.provider_script(
        "opencode",
        json!({"models": ["open/small", "open/medium"], "efforts": ["low", "high"], "turns": [
            {"when": "launch a helper", "do": [{"mcp": {"tool": "agent_launch", "arguments": {
                "title": "helper", "brief": "check the docs", "provider": "claude",
                "model": "fake-large"}}}]}
        ]}),
    );
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn composer_offers(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> Vec<String> {
    composer.read_with(cx, |c, _| c.offered())
}

fn open_settings(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(cx);
    shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap()
}

/// What the settings page says of a provider, line by line.
fn said_of(page: &Entity<SettingsPage>, name: &str, cx: &mut VisualTestContext) -> Vec<String> {
    page.read_with(cx, |page, _| page.said_of(name))
}

#[gpui_kit::test]
fn disabling_one_in_settings_removes_it_from_the_composer_s_picker(cx: &mut TestAppContext) {
    let user = providers_user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "both providers are offered", |cx| {
        composer_offers(&composer, cx) == ["claude", "opencode"]
    });

    let page = open_settings(&shell, cx);
    assert!(said_of(&page, "opencode", cx).contains(&"Ready".to_owned()));
    cx.update(|window, cx| window.click("provider-switch-opencode", cx));
    until(&shell, cx, "the page shows it disabled", |cx| {
        said_of(&page, "opencode", cx)
            .iter()
            .any(|line| line.starts_with("opencode is not enabled"))
    });

    let file = std::fs::read_to_string(user.path("config/flottille/providers.toml")).unwrap();
    assert!(file.contains("enabled = false"), "{file}");
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "only claude is offered", |cx| {
        composer_offers(&composer, cx) == ["claude"]
    });
}

#[gpui_kit::test]
fn choosing_the_other_and_a_model_starts_the_session_on_them_and_the_tree_shows_each_agent_s_provider_and_model(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    // A first session of OpenCode says its models.
    let first = user
        .call(
            "agent.start",
            json!({"project": "atlas", "provider": "opencode", "message": "hello"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("its first turn ends", || {
        user.call("agent.show", json!({"agent": first})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "the composer starts on the first ready", |cx| {
        composer.read_with(cx, |c, _| c.pick().provider.as_deref() == Some("claude"))
    });

    cx.update(|window, cx| window.click("provider-picker", cx));
    cx.update(|window, cx| window.click("provider-choice-opencode", cx));
    cx.update(|window, cx| window.click("model-picker", cx));
    cx.update(|window, cx| window.click("model-choice-open/small", cx));
    cx.update(|window, cx| window.click("effort-picker", cx));
    cx.update(|window, cx| window.click("effort-choice-high", cx));
    type_task("Please launch a helper", cx);
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the session's page opens", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });

    let page = session_page(&shell, cx);
    let lead = page.read_with(cx, |p, _| p.id().to_owned());
    let shown = user.call("agent.show", json!({"agent": lead})).unwrap()["agent"].clone();
    assert_eq!(shown["provider"], "opencode");
    assert_eq!(shown["chosen"]["model"], "open/small");
    assert_eq!(shown["chosen"]["effort"], "high");
    until(&shell, cx, "the rail shows both agents", |cx| {
        page.read_with(cx, |p, cx| p.rail().read(cx).agents().len() == 2)
    });
    let rail: Vec<(String, String, Option<String>)> = page.read_with(cx, |p, cx| {
        p.rail()
            .read(cx)
            .agents()
            .iter()
            .map(|a| (a.name.clone(), a.provider.clone(), a.model.clone()))
            .collect()
    });
    assert_eq!(rail[0].1, "opencode");
    assert_eq!(rail[0].2.as_deref(), Some("open/small"));
    assert_eq!(rail[1].1, "claude");
    assert_eq!(rail[1].2.as_deref(), Some("fake-large"));
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    assert_eq!(
        reply.read_with(cx, |r, cx| r.facts().read(cx).shown()),
        "opencode · open/small"
    );
}

/// A user whose `opencode` on PATH says it is 9.9.9, a version its bare mode was not qualified
/// on; Claude stands on the fake agent.
fn unready_opencode_user() -> User {
    let user = User::new();
    for dir in ["bin", "fake-claude"] {
        std::fs::create_dir(user.path(dir)).unwrap();
    }
    std::os::unix::fs::symlink(common::fake_agent(), user.path("fake-claude/agent")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let script = user.path("bin/opencode");
    std::fs::write(&script, "#!/bin/sh\necho 9.9.9\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let (claude, bin) = (user.path("fake-claude/agent"), user.path("bin"));
    let user = user
        .with_env("FLOTTILLE_AGENT_COMMAND_CLAUDE", claude.to_str().unwrap())
        .with_env("PATH", bin.to_str().unwrap());
    user.write_providers("[opencode]\nenabled = true\n");
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

#[gpui_kit::test]
fn an_unready_provider_shows_its_reason_in_settings_and_is_not_offered(cx: &mut TestAppContext) {
    let user = unready_opencode_user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "the reason is shown", |cx| {
        said_of(&page, "opencode", cx).iter().any(|line| {
            line.starts_with(
                "opencode is not ready: opencode is 9.9.9, and only 1.19.0 was qualified",
            )
        })
    });
    assert!(said_of(&page, "claude", cx).contains(&"Ready".to_owned()));

    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "the composer knows the providers", |cx| {
        !composer_offers(&composer, cx).is_empty()
    });
    assert_eq!(composer_offers(&composer, cx), ["claude"]);
}

#[gpui_kit::test]
fn when_nothing_is_ready_the_composer_says_why_and_links_to_the_settings_page(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    user.write_providers("[claude]\nenabled = false\n[opencode]\nenabled = false\n");
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "the composer says why", |cx| {
        composer.read_with(cx, |c, _| !c.not_ready().is_empty())
    });
    let why = composer.read_with(cx, |c, _| c.not_ready().to_vec());
    assert!(why[0].starts_with("claude is not enabled"), "{why:?}");
    assert!(why[1].starts_with("opencode is not enabled"), "{why:?}");

    type_task("Fix the login", cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(!composer.read_with(cx, |c, _| c.is_sending()));
    assert!(
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    cx.update(|window, cx| window.click("open-settings", cx));
    until(&shell, cx, "the settings page opens", |cx| {
        shell.read_with(cx, |s, _| s.selected() == Some(&Selection::Settings))
    });
}

#[gpui_kit::test]
fn the_settings_page_says_how_each_provider_signs_in_and_never_a_key_s_value(
    cx: &mut TestAppContext,
) {
    let user = providers_user().with_env("OPENAI_API_KEY", "made-up-key-value");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "the providers are shown", |cx| {
        !said_of(&page, "opencode", cx).is_empty()
    });

    let opencode = said_of(&page, "opencode", cx);
    assert!(
        opencode
            .iter()
            .any(|line| line == "Signs in with the API key in OPENAI_API_KEY"),
        "{opencode:?}"
    );
    let everything = format!("{opencode:?}{:?}", said_of(&page, "claude", cx));
    assert!(!everything.contains("made-up-key-value"), "{everything}");
}

#[gpui_kit::test]
fn the_pickers_start_on_the_project_s_default_then_on_the_last_ones_used(cx: &mut TestAppContext) {
    let user = providers_user();
    project(&user, "atlas");
    project(&user, "borealis");
    let file = user.project_file("atlas");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("provider = \"opencode\"\nmodel = \"open/medium\"\n{text}"),
    )
    .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let atlas = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "atlas starts on its own default", |cx| {
        atlas.read_with(cx, |c, _| {
            let pick = c.pick();
            pick.provider.as_deref() == Some("opencode")
                && pick.model.as_deref() == Some("open/medium")
        })
    });

    type_task("A first task", cx);
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the session's page opens", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    open_project(&shell, "borealis", cx);
    let borealis = composer_of(&shell, "borealis", cx);
    until(&shell, cx, "borealis starts on the last ones used", |cx| {
        borealis.read_with(cx, |c, _| {
            let pick = c.pick();
            pick.provider.as_deref() == Some("opencode")
                && pick.model.as_deref() == Some("open/medium")
        })
    });
}

// Providers' follow-ups (#48).

/// What the composer's model picker shows.
fn model_shown(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
    composer.read_with(cx, |c, _| c.model_shown())
}

/// Starts a session on that provider and model and waits for its first turn: its sessions then
/// say the models they offer, and that model's efforts.
fn learn_models(user: &User, project: &str, provider: &str, model: &str) {
    let id = user
        .call(
            "agent.start",
            json!({"project": project, "provider": provider, "model": model, "message": "hello"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("its first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
}

#[gpui_kit::test]
fn a_project_with_a_model_and_no_provider_shows_that_model_and_its_efforts_and_starts_on_it(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    learn_models(&user, "atlas", "claude", "fake-large");
    let file = user.project_file("atlas");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("model = \"fake-large\"\n{text}")).unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(
        &shell,
        cx,
        "the composer starts on the project's model",
        |cx| {
            composer.read_with(cx, |c, _| {
                let pick = c.pick();
                pick.provider.as_deref() == Some("claude")
                    && pick.model.as_deref() == Some("fake-large")
            })
        },
    );
    assert_eq!(model_shown(&composer, cx), "fake-large · the project's");
    cx.update(|window, cx| window.click("model-picker", cx));
    cx.update(|window, _| {
        assert!(window.try_find("model-choice-fake-large").is_some());
    });
    cx.update(|window, cx| window.click("model-choice-fake-large", cx));
    cx.update(|window, cx| window.click("effort-picker", cx));
    cx.update(|window, cx| window.click("effort-choice-high", cx));

    // Another provider is not the one the project's model goes to: its own default there.
    cx.update(|window, cx| window.click("provider-picker", cx));
    cx.update(|window, cx| window.click("provider-choice-opencode", cx));
    assert_eq!(model_shown(&composer, cx), "default model");
    cx.update(|window, cx| window.click("provider-picker", cx));
    cx.update(|window, cx| window.click("provider-choice-claude", cx));
    assert_eq!(model_shown(&composer, cx), "fake-large · the project's");

    type_task("A first task", cx);
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the session's page opens", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    let page = session_page(&shell, cx);
    let lead = page.read_with(cx, |p, _| p.id().to_owned());
    let shown = user.call("agent.show", json!({"agent": lead})).unwrap()["agent"].clone();
    assert_eq!(shown["provider"], "claude");
    assert_eq!(shown["chosen"]["model"], "fake-large");
}

#[gpui_kit::test]
fn a_project_model_its_provider_is_known_not_to_offer_is_said_instead_of_dropped(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    learn_models(&user, "atlas", "opencode", "open/small");
    let file = user.project_file("atlas");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("provider = \"opencode\"\nmodel = \"open/huge\"\n{text}"),
    )
    .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(
        &shell,
        cx,
        "the composer says the model is not offered",
        |cx| composer.read_with(cx, |c, _| c.model_refused().is_some()),
    );
    assert_eq!(
        composer.read_with(cx, |c, _| c.model_refused()).unwrap(),
        "provider opencode offers no model open/huge, the project's: it offers open/small, \
         open/medium"
    );
    assert_eq!(
        composer
            .read_with(cx, |c, _| c.pick().model.clone())
            .as_deref(),
        Some("open/huge")
    );
    type_task("A first task", cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(!composer.read_with(cx, |c, _| c.is_sending()));

    // Another model picked, it starts.
    cx.update(|window, cx| window.click("model-picker", cx));
    cx.update(|window, cx| window.click("model-choice-open/medium", cx));
    assert!(composer.read_with(cx, |c, _| c.model_refused()).is_none());
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the session's page opens", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    let page = session_page(&shell, cx);
    let lead = page.read_with(cx, |p, _| p.id().to_owned());
    let shown = user.call("agent.show", json!({"agent": lead})).unwrap()["agent"].clone();
    assert_eq!(shown["provider"], "opencode");
    assert_eq!(shown["chosen"]["model"], "open/medium");
}

/// What the relay of [`holding_provider_lists`] saw of the `provider.list` requests.
#[derive(Default)]
struct Relayed {
    /// Asked by the window, over every connection.
    asked: AtomicUsize,
    /// Held answers the relay received from the daemon, as soon as they came.
    answered: AtomicUsize,
    /// Held answers let through to the window, after their delay.
    released: AtomicUsize,
    /// Asked and not yet let through to the window, and the most there were at once.
    pending: AtomicUsize,
    peak: AtomicUsize,
}

/// Connects to the user's daemon through a relay that holds back the answer to the `n`th
/// `provider.list` asked (from 0, over every connection) for `hold(n)`.
fn holding_provider_lists(
    user: &User,
    hold: impl Fn(usize) -> Option<Duration> + Send + Sync + 'static,
) -> (Connect, Arc<Relayed>) {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    let connect = user.connector();
    let relayed = Arc::new(Relayed::default());
    let hold = Arc::new(hold);
    let seen = relayed.clone();
    let relay: Connect = Arc::new(move || {
        let daemon = connect()?;
        let (window_side, relay_side) = UnixStream::pair()?;
        let held: Arc<Mutex<HashMap<String, Option<Duration>>>> = Arc::default();
        let (mut to_daemon, from_window) = (daemon.try_clone()?, relay_side.try_clone()?);
        let (relayed, hold, marks) = (seen.clone(), hold.clone(), held.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(from_window).lines() {
                let Ok(line) = line else { return };
                let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                if request["method"] == "provider.list" {
                    let n = relayed.asked.fetch_add(1, Ordering::SeqCst);
                    let now = relayed.pending.fetch_add(1, Ordering::SeqCst) + 1;
                    relayed.peak.fetch_max(now, Ordering::SeqCst);
                    marks
                        .lock()
                        .unwrap()
                        .insert(request["id"].to_string(), hold(n));
                }
                if writeln!(to_daemon, "{line}").is_err() {
                    return;
                }
            }
        });
        let (mut to_window, relayed) = (relay_side, seen.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(daemon).lines() {
                let Ok(line) = line else { return };
                let answer: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let listed = held.lock().unwrap().remove(&answer["id"].to_string());
                let delay = listed.flatten();
                if let Some(delay) = delay {
                    relayed.answered.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(delay);
                }
                if writeln!(to_window, "{line}").is_err() {
                    return;
                }
                if delay.is_some() {
                    relayed.released.fetch_add(1, Ordering::SeqCst);
                }
                if listed.is_some() {
                    relayed.pending.fetch_sub(1, Ordering::SeqCst);
                }
            }
        });
        Ok(window_side)
    });
    (relay, relayed)
}

#[gpui_kit::test]
fn a_slow_provider_list_answer_that_arrives_after_a_newer_one_changes_nothing(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    // The link's first read is held: what it answers is the file as it was then.
    let (connect, relayed) =
        holding_provider_lists(&user, |n| (n == 0).then_some(Duration::from_secs(3)));
    let (shell, cx) = window(&user, connect, 1280., cx);
    until(&shell, cx, "the link asked for the providers", |_| {
        relayed.asked.load(Ordering::SeqCst) >= 1
    });
    // The daemon's answer is in the relay's hands, held, before the file is rewritten: it is the
    // older state, not just a slow answer.
    until(&shell, cx, "the daemon answered the held read", |_| {
        relayed.answered.load(Ordering::SeqCst) >= 1
    });
    assert_eq!(relayed.released.load(Ordering::SeqCst), 0);
    user.write_providers("[opencode]\nenabled = false\n");
    let page = open_settings(&shell, cx);
    until(&shell, cx, "the newer read shows opencode disabled", |cx| {
        said_of(&page, "opencode", cx)
            .iter()
            .any(|line| line.starts_with("opencode is not enabled"))
    });

    until(&shell, cx, "the held answer arrived", |_| {
        relayed.released.load(Ordering::SeqCst) == 1
    });
    for _ in 0..10 {
        until(&shell, cx, "a frame", |_| true);
    }
    assert!(
        said_of(&page, "opencode", cx)
            .iter()
            .any(|line| line.starts_with("opencode is not enabled")),
        "{:?}",
        said_of(&page, "opencode", cx)
    );
}

#[gpui_kit::test]
fn many_events_while_a_provider_read_is_held_give_one_read_at_a_time_and_a_last_one_after_them(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    // Every answer is held: a read lasts, and the sessions open meanwhile.
    let (connect, relayed) = holding_provider_lists(&user, |_| Some(Duration::from_secs(4)));
    let (shell, cx) = window(&user, connect, 1280., cx);
    until(&shell, cx, "the link asked for the providers", |_| {
        relayed.asked.load(Ordering::SeqCst) >= 1
    });
    let sessions: Vec<String> = (0..6)
        .map(|n| start(&user, "atlas", None, &format!("hello {n}")))
        .collect();
    until(&shell, cx, "every session is shown", |cx| {
        shell.read_with(cx, |s, _| sessions.iter().all(|id| s.session(id).is_some()))
    });
    // A read is still held when one more session opens: it is the one to be read after.
    let asked = relayed.asked.load(Ordering::SeqCst);
    let released = relayed.released.load(Ordering::SeqCst);
    start(&user, "atlas", None, "the last one");
    assert_eq!(
        (
            relayed.asked.load(Ordering::SeqCst),
            relayed.released.load(Ordering::SeqCst)
        ),
        (asked, released),
        "the read held at the start of the last session ended before it: too slow to prove"
    );
    until(&shell, cx, "a read follows the last session", |_| {
        relayed.asked.load(Ordering::SeqCst) > asked
    });
    until(&shell, cx, "every read is answered", |_| {
        relayed.pending.load(Ordering::SeqCst) == 0
            && relayed.released.load(Ordering::SeqCst) == relayed.asked.load(Ordering::SeqCst)
    });
    assert_eq!(
        relayed.peak.load(Ordering::SeqCst),
        1,
        "two provider.list were pending at once"
    );
}

#[gpui_kit::test]
fn the_providers_read_after_a_session_opened_does_not_hold_the_link_s_events(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    let (connect, relayed) = holding_provider_lists(&user, |_| Some(Duration::from_secs(15)));
    let (shell, cx) = window(&user, connect, 1280., cx);
    until(&shell, cx, "the project is shown", |cx| {
        shell.read_with(cx, |s, _| s.project("atlas").is_some())
    });
    until(&shell, cx, "the link asked for the providers", |_| {
        relayed.asked.load(Ordering::SeqCst) >= 1
    });
    // The first read is held for longer than the test: the sessions below mark that another is
    // due, which waits for it (one read at a time) and must not hold the events.
    let first = start(&user, "atlas", None, "hello");

    let begun = Instant::now();
    let second = start(&user, "atlas", None, "hello again");
    until(&shell, cx, "both sessions are shown", |cx| {
        shell.read_with(cx, |s, _| {
            s.session(&first).is_some() && s.session(&second).is_some()
        })
    });
    assert!(
        begun.elapsed() < Duration::from_secs(8),
        "{:?}",
        begun.elapsed()
    );
}

/// Whether the settings page's switch of that provider shows it on.
fn switch_on(page: &Entity<SettingsPage>, name: &str, cx: &mut VisualTestContext) -> bool {
    page.read_with(cx, |page, _| page.switch_on(name))
}

#[gpui_kit::test]
fn a_switch_the_daemon_refuses_goes_back_and_shows_the_daemon_s_sentence(cx: &mut TestAppContext) {
    let user = providers_user();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "opencode is shown on", |cx| {
        switch_on(&page, "opencode", cx)
    });
    // The file can be read, not written: the daemon refuses the switch.
    let config = user.path("config/flottille");
    let mode = |mode| std::os::unix::fs::PermissionsExt::from_mode(mode);
    std::fs::set_permissions(&config, mode(0o500)).unwrap();

    cx.update(|window, cx| window.click("provider-switch-opencode", cx));
    until(&shell, cx, "the daemon's sentence is shown", |cx| {
        said_of(&page, "opencode", cx)
            .iter()
            .any(|line| line.starts_with("providers.toml: Permission denied"))
    });
    std::fs::set_permissions(&config, mode(0o700)).unwrap();
    assert!(switch_on(&page, "opencode", cx), "the switch went back");
    assert!(!page.read_with(cx, |page, _| page.is_pending("opencode")));
}

#[gpui_kit::test]
fn a_switch_to_what_the_file_already_says_ends_without_an_event_and_without_a_pending_state(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "opencode is shown on", |cx| {
        switch_on(&page, "opencode", cx)
    });
    // Disabled by hand: the page does not know yet.
    user.write_providers("[opencode]\nenabled = false\n");

    cx.update(|window, cx| window.click("provider-switch-opencode", cx));
    until(&shell, cx, "the page shows it off, nothing pending", |cx| {
        !switch_on(&page, "opencode", cx)
            && !page.read_with(cx, |page, _| page.is_pending("opencode"))
            && said_of(&page, "opencode", cx)
                .iter()
                .any(|line| line.starts_with("opencode is not enabled"))
    });
    // A change after it, to read every event up to it.
    user.call("provider.disable", json!({"name": "claude"}))
        .unwrap();
    let watch = user.watch(&["--after", "0"]);
    let events = watch.until(|e| e["name"] == "provider.changed" && e["data"]["name"] == "claude");
    assert!(
        events
            .iter()
            .all(|e| !(e["name"] == "provider.changed" && e["data"]["name"] == "opencode")),
        "{events:?}"
    );
}

#[gpui_kit::test]
fn an_open_dropdown_is_above_the_elements_under_it_and_inside_the_window(cx: &mut TestAppContext) {
    let user = providers_user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    learn_models(&user, "atlas", "claude", "fake-small");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    cx.simulate_resize(size(px(1280.), px(560.)));
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "the efforts are known", |cx| {
        composer.read_with(cx, |c, _| !c.entries(Menu::Effort).is_empty())
    });
    settle(cx);

    for (picker, menu) in [
        ("place-picker", "place-picker-menu"),
        ("provider-picker", "provider-picker-menu"),
        ("model-picker", "model-picker-menu"),
        ("effort-picker", "effort-picker-menu"),
    ] {
        cx.update(|window, cx| window.click(picker, cx));
        settle(cx);
        cx.update(|window, _| {
            let list = window.find(menu).bounds();
            let viewport = window.viewport_size();
            assert!(
                list.left() >= px(0.)
                    && list.top() >= px(0.)
                    && list.right() <= viewport.width
                    && list.bottom() <= viewport.height,
                "{menu} {list:?} inside {viewport:?}"
            );
        });
        // A click outside closes it.
        cx.update(|window, cx| {
            window.click_at(
                format!("{picker}-backdrop"),
                gpui_kit::point(px(4.), px(4.)),
                cx,
            )
        });
        settle(cx);
        cx.update(|window, _| assert!(window.try_find(menu).is_none(), "{menu} stays open"));
    }

    // The effort's list opens over the places' cards and the new-workspace card: what is under
    // the pointer is the entry, not the card.
    cx.update(|window, cx| window.click("effort-picker", cx));
    settle(cx);
    let covers = cx.update(|window, _| {
        let list = window.find("effort-picker-menu").bounds();
        ["place-card-main", "place-card-login", "new-workspace-card"]
            .iter()
            .any(|card| {
                window
                    .try_find(*card)
                    .is_some_and(|c| c.bounds().intersects(&list))
            })
    });
    assert!(covers, "the list is drawn over a card");
    cx.update(|window, cx| window.click("effort-choice-high", cx));
    assert_eq!(
        composer
            .read_with(cx, |c, _| c.pick().effort.clone())
            .as_deref(),
        Some("high")
    );
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    assert!(
        !form.read_with(cx, |f, _| f.is_open()),
        "no card took the click"
    );
}

#[gpui_kit::test]
fn the_model_picker_lists_claude_s_models_by_name_with_default_model_once(cx: &mut TestAppContext) {
    let user = User::with_fake_claude();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    project(&user, "atlas");
    // A session opened before the window: its models are what the picker offers.
    start(&user, "atlas", None, "hello");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let composer = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "claude's models are known", |cx| {
        composer.read_with(cx, |c, _| {
            c.entries(Menu::Model)
                .iter()
                .all(|e| e.id != "models-unknown")
                && !c.entries(Menu::Model).is_empty()
        })
    });

    let labels = |menu: Menu, cx: &mut VisualTestContext| -> Vec<String> {
        composer.read_with(cx, |c, _| {
            c.entries(menu).into_iter().map(|e| e.label).collect()
        })
    };
    assert_eq!(model_shown(&composer, cx), "default model");
    assert_eq!(labels(Menu::Model, cx), ["default model", "Fable", "Haiku"]);
    assert_eq!(
        labels(Menu::Effort, cx),
        ["default effort", "low", "medium", "high", "xhigh"],
        "one default entry"
    );
    cx.update(|window, cx| window.click("model-picker", cx));
    cx.update(|window, cx| window.click("model-choice-fable", cx));
    assert_eq!(model_shown(&composer, cx), "Fable");
}

/// A secret the daemon knows through its environment (#63).
const SECRET: &str = "s3cr3t-token-value";

#[gpui_kit::test]
fn a_token_streamed_over_three_pieces_never_shows_in_clear_in_the_window_s_conversation(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent().with_env("API_TOKEN", SECRET);
    user.fake_script(json!({"turns": [{"when": "tell", "do": [
        {"say": "the token is s3cr3t-"}, {"say": "token-"}, {"say": "value, keep it quiet"}
    ]}]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    project(&user, "app");
    let lead = start(&user, "app", None, "hello");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);

    // Streamed live to the open page.
    send(&user, &lead, "tell");
    let texts = |cx: &mut VisualTestContext| -> Vec<String> {
        blocks(&page, cx)
            .into_iter()
            .filter_map(|block| match block {
                Block::Text { text } => Some(text),
                _ => None,
            })
            .collect()
    };
    until(&shell, cx, "the reply is shown whole", |cx| {
        texts(cx).iter().any(|t| t.ends_with("keep it quiet"))
    });
    settle(cx);

    let shown = texts(cx);
    assert!(
        shown.iter().all(|t| !t.contains(SECRET)),
        "{}",
        shown.join("|").replace(SECRET, "<clear>")
    );
    assert!(
        shown.contains(&"the token is [masked], keep it quiet".to_owned()),
        "{shown:?}"
    );
}

// The conversation as it reads (#68): Markdown replies, no "you" or "lead" on screen, folded
// reports, and no empty thought.

/// Gives the open page these events of the shown agent, as its link would have read them.
fn read_events(
    page: &Entity<SessionPage>,
    events: Vec<flottille_core::protocol::Event>,
    cx: &mut VisualTestContext,
) {
    page.update(cx, |p, cx| {
        let token = p.token().unwrap();
        p.apply(
            token,
            flottille_window::session_link::SessionUpdate::History {
                events,
                earlier: false,
            },
            cx,
        )
    });
}

/// Gives the open page one event of the shown agent, live.
fn live(
    page: &Entity<SessionPage>,
    event: flottille_core::protocol::Event,
    cx: &mut VisualTestContext,
) {
    page.update(cx, |p, cx| {
        let token = p.token().unwrap();
        p.apply(
            token,
            flottille_window::session_link::SessionUpdate::Live(event),
            cx,
        )
    });
}

/// The text the row `at` shows, its Markdown drawn.
fn rendered(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) -> String {
    page.read_with(cx, |p, cx| {
        p.conversation().read(cx).rows()[at]
            .read(cx)
            .rendered(cx)
            .unwrap_or_default()
    })
}

/// The lines drawn over the rows of the conversation shown.
fn labels(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<String> {
    page.read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .rows()
            .iter()
            .filter_map(|row| row.read(cx).label())
            .collect()
    })
}

/// Draws a frame and runs what it started, the Markdown parsed in the background included.
fn frame(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

const MARKDOWN: &str = "Here is **the plan**:\n\n- read the form\n- write the tests\n\n```rust\nfn main() {}\n```\n\nSee `cargo test` and [the docs](https://example.com/docs).";

#[gpui_kit::test]
fn a_reply_with_bold_a_list_and_a_code_block_renders_them_not_their_marks(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);

    read_events(
        &page,
        vec![
            made_up(
                1,
                "agent.prompted",
                json!({"text": "[flottille:user]\nplan it"}),
            ),
            made_up(2, "agent.message", json!({"text": MARKDOWN})),
        ],
        cx,
    );
    frame(cx);

    let shown = rendered(&page, 1, cx);
    for said in [
        "the plan",
        "read the form",
        "write the tests",
        "fn main() {}",
        "cargo test",
        "the docs",
    ] {
        assert!(shown.contains(said), "{said} in {shown:?}");
    }
    for mark in ["**", "```", "- read", "`cargo", "](https"] {
        assert!(!shown.contains(mark), "{mark} in {shown:?}");
    }

    // A reply still streamed renders as it grows.
    for (seq, piece) in [(3, "\n\nThen **ship"), (4, " it**.")] {
        live(
            &page,
            made_up(seq, "agent.message", json!({"text": piece})),
            cx,
        );
        frame(cx);
    }
    let shown = rendered(&page, 1, cx);
    assert!(shown.trim_end().ends_with("Then ship it."), "{shown:?}");
    assert!(!shown.contains("**"), "{shown:?}");
}

const TABLE: &str = "A table:\n\n| Tool | Result |\n| --- | --- |\n| `cargo test` | passes |\n| clippy | clean |\n\nAfter it.";

/// Opens a session whose reply holds a table, in a window of the palette `colors` describes (the
/// default dark one when `None`), and gives the solid backgrounds drawn, with the palette.
fn drawn_table(
    colors: Option<&str>,
    cx: &mut TestAppContext,
) -> (flottille_ui::Palette, Vec<gpui_kit::Hsla>) {
    let user = team_user();
    if let Some(colors) = colors {
        std::fs::write(user.path("os-release"), "NAME=Test\nID=omarchy\n").unwrap();
        let file = user.path("state/omarchy/current/theme/colors.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, colors).unwrap();
    }
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);
    read_events(
        &page,
        vec![
            made_up(
                1,
                "agent.prompted",
                json!({"text": "[flottille:user]\nlist them"}),
            ),
            made_up(2, "agent.message", json!({"text": TABLE})),
        ],
        cx,
    );
    frame(cx);
    settle(cx);
    let palette = cx.update(|_, cx| flottille_ui::palette(cx));
    let quads = cx.update(|window, _| {
        window
            .painted_quads()
            .iter()
            .filter_map(|q| q.background.as_solid())
            .collect()
    });
    (palette, quads)
}

/// The table's frame is on the palette's card and its header on the quiet surface: no quad of the
/// kit's own (white) surface is left.
fn assert_table_in_palette(p: &flottille_ui::Palette, quads: &[gpui_kit::Hsla]) {
    assert!(
        quads.contains(&p.card),
        "the table's frame is the card colour"
    );
    assert!(
        quads.contains(&p.card2),
        "the table's header is the quiet surface"
    );
    assert!(
        !quads.contains(&gpui_kit::white()),
        "no white quad is drawn"
    );
}

#[gpui_kit::test]
fn a_markdown_table_is_drawn_with_the_palette_on_the_dark_theme(cx: &mut TestAppContext) {
    let (p, quads) = drawn_table(None, cx);
    assert_table_in_palette(&p, &quads);
    assert!(
        quads
            .iter()
            .all(|bg| bg.a < 1.0 || bg.l < 0.9 || *bg == p.acc),
        "nothing light is under the cells on a dark theme"
    );
}

#[gpui_kit::test]
fn a_markdown_table_is_drawn_with_the_palette_on_a_light_theme(cx: &mut TestAppContext) {
    let (p, quads) = drawn_table(
        Some("background = \"#fafafa\"\nforeground = \"#1c1c1c\"\naccent = \"#2f5fd0\"\n"),
        cx,
    );
    assert!(p.bg.l > 0.9, "the theme is light: {:?}", p.bg);
    assert_table_in_palette(&p, &quads);
}

#[gpui_kit::test]
fn a_user_message_has_no_you_label(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);

    assert_eq!(
        blocks(&page, cx)[0],
        Block::Message {
            from: "you".into(),
            text: "lead the work".into()
        }
    );
    assert_eq!(
        page.read_with(cx, |p, cx| p.conversation().read(cx).rows()[0]
            .read(cx)
            .label()),
        None
    );
    cx.update(|window, _| {
        assert!(window.try_find("row-0-message").is_some());
        assert!(window.try_find("row-0-message-from").is_none());
    });
}

#[gpui_kit::test]
fn a_report_is_folded_until_clicked(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);

    read_events(
        &page,
        vec![made_up(
            1,
            "agent.prompted",
            json!({"text": "[flottille:child-report]\nThe form is **built**; the tests were not run."}),
        )],
        cx,
    );
    frame(cx);

    assert_eq!(labels(&page, cx), ["Report from a sub-agent"]);
    cx.update(|window, _| {
        assert!(window.try_find("row-0-report-toggle").is_some());
        assert!(window.try_find("row-0-report-text").is_none(), "folded");
    });

    cx.update(|window, cx| window.click("row-0-report-toggle", cx));
    frame(cx);
    cx.update(|window, _| assert!(window.try_find("row-0-report-text").is_some()));

    cx.update(|window, cx| window.click("row-0-report-toggle", cx));
    frame(cx);
    cx.update(|window, _| assert!(window.try_find("row-0-report-text").is_none()));
}

#[gpui_kit::test]
fn no_visible_text_of_a_session_holds_lead(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (worker_id, _worker) = worker(&user, &mcp, "build the form");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the worker is in the rail", |cx| {
        rail_ids(&page, cx).contains(&worker_id)
    });
    settle(cx);
    let mut said = labels(&page, cx);
    assert!(
        said.contains(&"launched by the conversation".to_owned()),
        "{said:?}"
    );
    said.push(flottille_window::session::REPLY_PLACEHOLDER.to_owned());
    said.push(flottille_window::session::SEND.to_owned());
    assert_eq!(page.read_with(cx, |p, _| p.read_only_text()), None);

    cx.update(|window, cx| window.click(format!("rail-row-{worker_id}"), cx));
    until(&shell, cx, "the worker's brief is shown", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == worker_id)
            && !blocks(&page, cx).is_empty()
    });
    settle(cx);
    let child = labels(&page, cx);
    assert_eq!(child[0], "the conversation → build the form");
    said.extend(child);
    let (bar, back) = page.read_with(cx, |p, _| p.read_only_text()).unwrap();
    assert_eq!(
        bar,
        "Reading build the form. It belongs to the conversation: only the conversation takes \
         your messages."
    );
    assert_eq!(back, "Back to the conversation");
    said.extend([bar, back]);

    assert!(said.iter().all(|text| !text.contains("lead")), "{said:?}");
    assert!(said.contains(&"Reply…".to_owned()) && said.contains(&"Send".to_owned()));
}

#[gpui_kit::test]
fn a_thought_s_text_reaches_the_window_and_one_claude_sent_without_text_shows_nothing(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "do": [
        {"think_hidden": true},
        {"say": "Looking."},
        {"think": "weighing **two** options"},
        {"say": "Done."}
    ]}]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    project(&user, "app");
    let lead = start(&user, "app", None, "hello");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the reply is shown", |cx| {
        blocks(&page, cx).iter().any(|b| {
            *b == Block::Text {
                text: "Done.".into(),
            }
        })
    });
    settle(cx);

    let thoughts: Vec<(usize, String)> = blocks(&page, cx)
        .into_iter()
        .enumerate()
        .filter_map(|(at, block)| match block {
            Block::Thought { text, .. } => Some((at, text)),
            _ => None,
        })
        .collect();
    assert_eq!(thoughts.len(), 1, "{thoughts:?}");
    let (at, text) = &thoughts[0];
    assert_eq!(text, "weighing **two** options");

    cx.update(|window, cx| window.click(format!("row-{at}-thought-toggle"), cx));
    frame(cx);
    cx.update(|window, _| assert!(window.try_find(format!("row-{at}-thought-text")).is_some()));
    assert_eq!(rendered(&page, *at, cx).trim_end(), "weighing two options");
}

#[gpui_kit::test]
fn a_thought_with_no_text_shows_its_duration_and_opens_on_nothing(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);

    read_events(
        &page,
        vec![
            made_up(1, "agent.thought", json!({"text": ""})),
            made_up(2, "agent.message", json!({"text": "ok"})),
        ],
        cx,
    );
    frame(cx);

    assert!(matches!(
        blocks(&page, cx)[0],
        Block::Thought { until: Some(_), .. }
    ));
    cx.update(|window, _| {
        assert!(window.try_find("row-0-thought").is_some(), "its duration");
        assert!(
            window.try_find("row-0-thought-toggle").is_none(),
            "nothing to open"
        );
    });
}

#[gpui_kit::test]
fn a_reply_of_thousands_of_lines_streams_in_pieces_that_stay_cheap(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);
    read_events(
        &page,
        vec![made_up(
            1,
            "agent.message",
            json!({"text": "# A long reply\n"}),
        )],
        cx,
    );
    frame(cx);

    // 3,000 lines in 100 pieces of 30: a list item with bold and code on each.
    let (mut applied, mut whole) = (Vec::new(), Vec::new());
    for piece in 0..100u64 {
        let text: String = (0..30)
            .map(|line| {
                let n = piece * 30 + line;
                format!("- line **{n}** of the reply, with `code {n}`\n")
            })
            .collect();
        let started = Instant::now();
        live(
            &page,
            made_up(2 + piece, "agent.message", json!({"text": text})),
            cx,
        );
        applied.push(started.elapsed());
        // The parse, in the background in the window, runs here before the frame.
        frame(cx);
        whole.push(started.elapsed());
    }

    let shown = rendered(&page, 0, cx);
    assert!(
        shown.contains("line 2999 of the reply, with code 2999"),
        "the last line is drawn"
    );
    assert!(!shown.contains("**"));
    let mean = |costs: &[Duration]| costs.iter().sum::<Duration>() / costs.len() as u32;
    eprintln!(
        "a piece of 30 lines, on the window's thread then with its parse and frame: \
         {:?}, {:?} at the start; {:?}, {:?} past 2,700 lines",
        mean(&applied[..10]),
        mean(&whole[..10]),
        mean(&applied[90..]),
        mean(&whole[90..])
    );
    // What matters is that the cost does not grow with the reply (4.8 s a piece past 2,700
    // lines when the reply was one view). An absolute bound fails on a slower machine, so the
    // cost past 2,700 lines is compared with the start of the same run, which is floored at a
    // few milliseconds so a very fast start cannot make the test fail. A spike reached 6 times the
    // start on a busy machine; the regression was over a hundred.
    let bound = |start: &[Duration]| mean(start).max(Duration::from_millis(5)) * 10;
    assert!(mean(&applied[90..]) <= bound(&applied[..10]));
    assert!(mean(&whole[90..]) <= bound(&whole[..10]));
}

// Forges (#75): each repository's pull request in the session's WORKSPACE section and on the
// place's card, read through the fake `gh`.

/// Declares `app`'s forge, GitHub's `acme/app`, and its tracker, the issues of it, as the user
/// does in the project's settings.
fn declare_on_github(user: &User) {
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
}

#[gpui_kit::test]
fn the_workspace_section_shows_the_pull_request_and_its_checks_mark_and_opens_it_on_a_click(
    cx: &mut TestAppContext,
) {
    let user = team_user().with_fake_gh();
    let (lead, _mcp, _) = lead_in_w(&user);
    user.git(
        &user.path("work/app"),
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    declare_on_github(&user);
    let url = "https://github.com/acme/app/pull/7";
    user.gh_script(json!({"pulls": {"acme/app": {"app/w": {
        "number": 7, "title": "Add the form", "state": "OPEN", "isDraft": false,
        "baseRefName": "main", "url": url, "reviewDecision": null,
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "FAILURE"}
        ],
        "comments": [], "reviews": []
    }}}}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let given: Arc<std::sync::Mutex<Vec<PathBuf>>> = Arc::default();
    let opened = given.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            opened.lock().unwrap().push(path.to_owned())
        }))
    });
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));

    until(&shell, cx, "the pull request in the rail", |cx| {
        cx.update(|window, _| window.try_find("rail-pull-app").is_some())
    });
    let page = session_page(&shell, cx);
    let pulls = page.read_with(cx, |p, cx| p.rail().read(cx).place().unwrap().pulls.clone());
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0].label(), "#7 open");
    assert_eq!(pulls[0].mark.as_deref(), Some("failing"));
    // The session page watches the workspace while it is open: the daemon reads it again.
    settle(cx);
    cx.update(|window, cx| window.click("rail-pull-app", cx));
    settle(cx);
    assert_eq!(*given.lock().unwrap(), [PathBuf::from(url)]);

    // The place's card on the project page shows it too, and opens it the same way.
    open_project(&shell, "app", cx);
    until(&shell, cx, "the pull request on the card", |cx| {
        card_of(&shell, Some("w"), cx)
            .is_some_and(|card| card.read_with(cx, |c, _| !c.pulls().is_empty()))
    });
    settle(cx);
    cx.update(|window, cx| window.click("pull-w-app", cx));
    settle(cx);
    assert_eq!(given.lock().unwrap().len(), 2);
}

// Tickets (#79): the workspace's ticket in the session's WORKSPACE section, linked and unlinked
// there, and linked from the form that makes a workspace, read through the fake `gh`.

#[gpui_kit::test]
fn the_workspace_section_shows_the_ticket_opens_it_on_a_click_and_links_and_unlinks_it(
    cx: &mut TestAppContext,
) {
    let user = team_user().with_fake_gh();
    let (lead, _mcp, _) = lead_in_w(&user);
    user.git(
        &user.path("work/app"),
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    declare_on_github(&user);
    let issue = |number: u64, title: &str| {
        json!({"number": number, "title": title, "state": "OPEN", "body": "", "comments": [],
               "url": format!("https://github.com/acme/app/issues/{number}")})
    };
    user.gh_script(json!({"issues": {"acme/app": {
        "12": issue(12, "The form fails"), "13": issue(13, "The menu fails")
    }}}));
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "w", "ticket": "#12"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let given: Arc<std::sync::Mutex<Vec<PathBuf>>> = Arc::default();
    let opened = given.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            opened.lock().unwrap().push(path.to_owned())
        }))
    });
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));

    until(&shell, cx, "the ticket in the rail", |cx| {
        cx.update(|window, _| window.try_find("rail-ticket").is_some())
    });
    let page = session_page(&shell, cx);
    let ticket = page
        .read_with(cx, |p, cx| {
            p.rail().read(cx).place().unwrap().ticket.clone()
        })
        .unwrap();
    assert_eq!(ticket.reference, "#12");
    assert_eq!(ticket.title.as_deref(), Some("The form fails"));
    assert_eq!(ticket.state.as_deref(), Some("open"));
    settle(cx);
    cx.update(|window, cx| window.click("rail-ticket", cx));
    settle(cx);
    assert_eq!(
        *given.lock().unwrap(),
        [PathBuf::from("https://github.com/acme/app/issues/12")]
    );

    // Unlinked from the rail: the daemon forgets it and the row goes.
    cx.update(|window, cx| window.click("rail-ticket-unlink", cx));
    until(&shell, cx, "the ticket gone from the rail", |cx| {
        cx.update(|window, _| window.try_find("rail-ticket").is_none())
    });
    let shown = user
        .call("workspace.show", json!({"project": "app", "name": "w"}))
        .unwrap();
    assert_eq!(shown["ticket"], serde_json::Value::Null);

    // Linked from the field: Enter links what is typed.
    let field = page
        .read_with(cx, |p, cx| p.rail().read(cx).ticket_field().cloned())
        .expect("the rail has its field");
    cx.update(|window, cx| {
        let focus = field.read(cx).focus_handle(cx);
        window.focus(&focus, cx)
    });
    cx.simulate_input("13");
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the new ticket in the rail", |cx| {
        cx.update(|window, _| window.try_find("rail-ticket").is_some())
    });
    let ticket = page
        .read_with(cx, |p, cx| {
            p.rail().read(cx).place().unwrap().ticket.clone()
        })
        .unwrap();
    assert_eq!(ticket.reference, "#13");
    assert_eq!(ticket.title.as_deref(), Some("The menu fails"));
    assert_eq!(field.read_with(cx, |f, _| f.value().to_string()), "");

    // A reference that is no ticket is refused, and the rail says why.
    cx.update(|window, cx| {
        let focus = field.read(cx).focus_handle(cx);
        window.focus(&focus, cx)
    });
    cx.simulate_input("nope");
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the refusal in the rail", |cx| {
        page.read_with(cx, |p, cx| p.rail().read(cx).ticket_error().is_some())
    });
    let said = page.read_with(cx, |p, cx| {
        p.rail().read(cx).ticket_error().unwrap().to_owned()
    });
    assert!(said.contains("is not a ticket"), "{said}");
}

#[gpui_kit::test]
fn the_form_that_makes_a_workspace_links_the_ticket_typed_in_it(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    user.call(
        "project.set_settings",
        json!({"name": "atlas",
            "tracker": {"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}}),
    )
    .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();

    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);
    assert!(cx.update(|window, _| window.find("new-workspace-modal-ticket").visible()));
    form.update_in(cx, |form, window, cx| {
        form.ticket_field()
            .update(cx, |ticket, cx| ticket.set_value("abc-5", window, cx))
    });
    cx.simulate_input("with-ticket");
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the workspace is made", |cx| {
        !form.read_with(cx, |f, _| f.is_open())
    });

    let shown = user
        .call(
            "workspace.show",
            json!({"project": "atlas", "name": "with-ticket"}),
        )
        .unwrap();
    assert_eq!(shown["ticket"], "ABC-5");
    assert_eq!(
        form.read_with(cx, |f, cx| f.ticket_field().read(cx).value().to_string()),
        "",
        "the field is empty for the next one"
    );
}

// The window's history (#73): the mouse's back and forward buttons and Alt+Left / Alt+Right.

/// A press of the mouse's back or forward button with the pointer at `(x, 400)`: the pointer
/// is wherever it happens to be, over the tree or the page.
fn navigate_at(direction: gpui_kit::NavigationDirection, x: f32, cx: &mut VisualTestContext) {
    let at = gpui_kit::point(px(x), px(400.));
    let button = gpui_kit::MouseButton::Navigate(direction);
    cx.simulate_mouse_down(at, button, gpui_kit::Modifiers::default());
    cx.simulate_mouse_up(at, button, gpui_kit::Modifiers::default());
    settle(cx);
}

fn back(cx: &mut VisualTestContext) {
    navigate_at(gpui_kit::NavigationDirection::Back, 700., cx);
}

fn forward(cx: &mut VisualTestContext) {
    navigate_at(gpui_kit::NavigationDirection::Forward, 700., cx);
}

fn selected(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Selection> {
    shell.read_with(cx, |s, _| s.selected().cloned())
}

/// Opens `id`'s page with a click on its row, as a user would, and lets the entrance end.
fn open_row(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the session's page is shown", |cx| {
        selected(shell, cx) == Some(Selection::Session(id.into()))
            && shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    settle(cx);
}

/// A window on the user's daemon with `atlas` open and the sessions started in it listed.
fn atlas_with_sessions<'a>(
    user: &User,
    titles: &[&str],
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext, Vec<String>) {
    project(user, "atlas");
    let ids: Vec<String> = titles
        .iter()
        .map(|title| start(user, "atlas", None, title))
        .collect();
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the sessions are in the tree", |cx| {
        ids.iter()
            .all(|id| cx.update(|window, _| window.try_find(session_id(id)).is_some()))
            && selected(&shell, cx) == Some(Selection::Project("atlas".into()))
    });
    settle(cx);
    (shell, cx, ids)
}

#[gpui_kit::test]
fn back_from_a_session_shows_the_project_page_and_forward_shows_the_session_again(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx, ids) = atlas_with_sessions(&user, &["Sessions expire too early"], cx);
    open_row(&shell, &ids[0], cx);

    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
    assert!(
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some()),
        "the project's page is shown"
    );
    forward(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(ids[0].clone()))
    );
    assert!(shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some()));
    // Nothing is left to go forward to, nor back before the first page.
    forward(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(ids[0].clone()))
    );
    back(cx);
    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
}

#[gpui_kit::test]
fn the_buttons_work_wherever_the_pointer_is(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx, ids) = atlas_with_sessions(&user, &["Tidy the aliases"], cx);
    open_row(&shell, &ids[0], cx);

    // Over the tree, then over the top bar.
    navigate_at(gpui_kit::NavigationDirection::Back, 100., cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
    cx.simulate_mouse_down(
        gpui_kit::point(px(900.), px(10.)),
        gpui_kit::MouseButton::Navigate(gpui_kit::NavigationDirection::Forward),
        gpui_kit::Modifiers::default(),
    );
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(ids[0].clone()))
    );
}

#[gpui_kit::test]
fn alt_left_and_alt_right_do_what_the_mouse_buttons_do(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx, ids) = atlas_with_sessions(&user, &["Tidy the aliases"], cx);
    open_row(&shell, &ids[0], cx);

    cx.simulate_keystrokes("alt-left");
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
    cx.simulate_keystrokes("alt-right");
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(ids[0].clone()))
    );
}

#[gpui_kit::test]
fn alt_left_works_with_the_caret_in_a_composer(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    type_task("Fix the timeout", cx);
    assert_eq!(
        field_of(&composer_of(&shell, "bravo", cx), cx),
        "Fix the timeout"
    );

    cx.simulate_keystrokes("alt-left");
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
}

#[gpui_kit::test]
fn a_child_shown_in_a_session_is_a_place_back_shows_the_conversation(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let child = worker_doing(&mcp, "WRITE", "draft the notes");
    eventually("the child replied", || {
        user.call("agent.show", json!({"agent": child})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the child is in the rail", |cx| {
        rail_ids(&page, cx).contains(&child)
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("rail-row-{child}"), cx));
    until(&shell, cx, "the child is shown", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == child)
    });

    back(cx);
    let page = session_page(&shell, cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(lead.clone())),
        "still the same session"
    );
    assert_eq!(page.read_with(cx, |p, _| p.shown().to_owned()), lead);
    forward(cx);
    let page = session_page(&shell, cx);
    assert_eq!(page.read_with(cx, |p, _| p.shown().to_owned()), child);
    // Back past the session reaches the first project's page.
    back(cx);
    back(cx);
    assert_eq!(selected(&shell, cx), Some(Selection::Project("app".into())));
}

#[gpui_kit::test]
fn going_back_to_a_session_shows_the_agent_that_was_shown_there(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let child = worker_doing(&mcp, "WRITE", "draft the notes");
    eventually("the child replied", || {
        user.call("agent.show", json!({"agent": child})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the child is in the rail", |cx| {
        rail_ids(&page, cx).contains(&child)
    });
    settle(cx);
    cx.update(|window, cx| window.click(format!("rail-row-{child}"), cx));
    until(&shell, cx, "the child is shown", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == child)
    });

    // Leave the session for its project, then come back to it.
    cx.update(|window, cx| window.click(project_id("app"), cx));
    settle(cx);
    assert!(shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_none()));
    back(cx);
    assert_eq!(selected(&shell, cx), Some(Selection::Session(lead.clone())));
    let page = session_page(&shell, cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.shown().to_owned()),
        child,
        "the child that was shown"
    );
    until(&shell, cx, "the child's conversation is read", |cx| {
        !blocks(&page, cx).is_empty()
    });
}

#[gpui_kit::test]
fn back_from_the_settings_returns_to_where_the_user_was(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx, ids) = atlas_with_sessions(&user, &["Tidy the aliases"], cx);
    open_row(&shell, &ids[0], cx);
    open_settings(&shell, cx);

    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(ids[0].clone()))
    );
    forward(cx);
    assert_eq!(selected(&shell, cx), Some(Selection::Settings));
}

#[gpui_kit::test]
fn showing_a_new_page_cuts_what_was_ahead_and_the_same_page_again_adds_nothing(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx, ids) = atlas_with_sessions(&user, &["Tidy the aliases", "Fix the login"], cx);
    // The newest is listed first, so `ids[1]` is the second started.
    open_row(&shell, &ids[0], cx);
    cx.update(|window, cx| window.click(session_id(&ids[0]), cx));
    settle(cx);

    // The same session again added nothing: one step back is the project's page.
    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
    // A new page cuts the session that was ahead.
    open_row(&shell, &ids[1], cx);
    forward(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(ids[1].clone())),
        "nothing ahead"
    );
    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
}

/// Connects to the user's daemon through a relay that leaves out of every `agent.list` and
/// `agent.tree` answer the agents named in `hidden`: a session the daemon forgot, an agent that
/// left a tree.
fn forgetting(
    user: &User,
    hidden: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
) -> Connect {
    use std::collections::HashSet;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    let connect = user.connector();
    Arc::new(move || {
        let daemon = connect()?;
        let (window_side, relay_side) = UnixStream::pair()?;
        let lists: Arc<Mutex<HashSet<String>>> = Arc::default();
        let (mut to_daemon, from_window) = (daemon.try_clone()?, relay_side.try_clone()?);
        let marks = lists.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(from_window).lines() {
                let Ok(line) = line else { return };
                let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                if request["method"] == "agent.list" || request["method"] == "agent.tree" {
                    marks.lock().unwrap().insert(request["id"].to_string());
                }
                if writeln!(to_daemon, "{line}").is_err() {
                    return;
                }
            }
        });
        let mut to_window = relay_side;
        let hidden = hidden.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(daemon).lines() {
                let Ok(line) = line else { return };
                let mut answer: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let mut line = line;
                if lists.lock().unwrap().remove(&answer["id"].to_string()) {
                    let hidden = hidden.lock().unwrap();
                    if let Some(agents) = answer["result"]["agents"].as_array_mut() {
                        // `agent.list` gives the agents, `agent.tree` an entry holding each.
                        agents.retain(|a| {
                            let id = a["id"].as_str().or(a["agent"]["id"].as_str());
                            !hidden.contains(id.unwrap_or_default())
                        });
                    }
                    line = answer.to_string();
                }
                if writeln!(to_window, "{line}").is_err() {
                    return;
                }
            }
        });
        Ok(window_side)
    })
}

#[gpui_kit::test]
fn a_forgotten_session_is_skipped_going_back_and_forward(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let first = start(&user, "atlas", None, "Tidy the aliases");
    let second = start(&user, "atlas", None, "Fix the login");
    let third = start(&user, "atlas", None, "Sessions expire too early");
    let hidden: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Arc::default();
    let (shell, cx) = window(&user, forgetting(&user, hidden.clone()), 1280., cx);
    until(&shell, cx, "the sessions are in the tree", |cx| {
        [&first, &second, &third]
            .iter()
            .all(|id| cx.update(|window, _| window.try_find(session_id(id)).is_some()))
    });
    open_row(&shell, &second, cx);
    open_row(&shell, &third, cx);

    // The daemon forgets the second one; something happening makes the window read again.
    hidden.lock().unwrap().insert(second.clone());
    send(&user, &first, "hello");
    until(&shell, cx, "the second session leaves the tree", |cx| {
        shell.read_with(cx, |s, _| s.snapshot().session(&second).is_none())
    });

    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into())),
        "the forgotten session is skipped"
    );
    forward(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(third.clone())),
        "and skipped again going forward"
    );
}

#[gpui_kit::test]
fn a_removed_project_is_skipped_going_back(cx: &mut TestAppContext) {
    let user = user();
    for name in ["atlas", "bravo", "charlie"] {
        project(&user, name);
    }
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    open_project(&shell, "charlie", cx);

    user.call("project.remove", json!({"name": "bravo"}))
        .unwrap();
    until(&shell, cx, "bravo leaves the tree", |cx| {
        shell.read_with(cx, |s, _| s.project("bravo").is_none())
    });
    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
}

#[gpui_kit::test]
fn back_with_a_dropdown_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    cx.update(|window, cx| window.click("provider-picker", cx));
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("provider-picker-menu").is_some()));

    back(cx);
    assert!(cx.update(|window, _| window.try_find("provider-picker-menu").is_none()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into())),
        "the button only closed the list"
    );
    back(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
}

#[gpui_kit::test]
fn forward_with_a_modal_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    back(cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);
    assert!(form.read_with(cx, |f, _| f.is_open()));

    forward(cx);
    assert!(!form.read_with(cx, |f, _| f.is_open()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into())),
        "the button only closed the modal"
    );
    forward(cx);
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into()))
    );
}

#[gpui_kit::test]
fn back_with_the_bell_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    cx.update(|window, cx| window.click("bell", cx));
    assert!(shell.read_with(cx, |s, _| s.bell_open()));

    back(cx);
    assert!(!shell.read_with(cx, |s, _| s.bell_open()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into()))
    );
}

#[gpui_kit::test]
fn back_with_the_tree_drawer_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 700., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    cx.update(|window, cx| window.click("open-tree", cx));
    settle(cx);
    assert!(shell.read_with(cx, |s, cx| s.left_drawer().read(cx).is_open()));

    navigate_at(gpui_kit::NavigationDirection::Back, 500., cx);
    assert!(!shell.read_with(cx, |s, cx| s.left_drawer().read(cx).is_open()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into()))
    );
}

/// A key press of the window's history with a list, modal, drawer or bell open: it closes that
/// first and goes nowhere, as the mouse's buttons do.
#[gpui_kit::test]
fn alt_left_with_a_dropdown_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    cx.update(|window, cx| window.click("provider-picker", cx));
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("provider-picker-menu").is_some()));

    cx.simulate_keystrokes("alt-left");
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("provider-picker-menu").is_none()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into())),
        "the key only closed the list"
    );
    cx.simulate_keystrokes("alt-left");
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into()))
    );
}

#[gpui_kit::test]
fn alt_right_with_a_modal_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    back(cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);
    assert!(form.read_with(cx, |f, _| f.is_open()));

    cx.simulate_keystrokes("alt-right");
    assert!(!form.read_with(cx, |f, _| f.is_open()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("atlas".into())),
        "the key only closed the modal"
    );
    cx.simulate_keystrokes("alt-right");
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into()))
    );
}

#[gpui_kit::test]
fn alt_left_with_the_bell_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    cx.update(|window, cx| window.click("bell", cx));
    assert!(shell.read_with(cx, |s, _| s.bell_open()));

    cx.simulate_keystrokes("alt-left");
    assert!(!shell.read_with(cx, |s, _| s.bell_open()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into()))
    );
}

#[gpui_kit::test]
fn alt_left_with_the_tree_drawer_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "bravo");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 700., cx);
    open_project(&shell, "atlas", cx);
    open_project(&shell, "bravo", cx);
    cx.update(|window, cx| window.click("open-tree", cx));
    settle(cx);
    assert!(shell.read_with(cx, |s, cx| s.left_drawer().read(cx).is_open()));

    cx.simulate_keystrokes("alt-left");
    assert!(!shell.read_with(cx, |s, cx| s.left_drawer().read(cx).is_open()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Project("bravo".into()))
    );
}

#[gpui_kit::test]
fn going_back_to_an_agent_that_left_the_tree_shows_the_conversation(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let child = worker_doing(&mcp, "WRITE", "draft the notes");
    eventually("the child replied", || {
        user.call("agent.show", json!({"agent": child})).unwrap()["agent"]["state"] == "idle"
    });
    let hidden: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Arc::default();
    let (shell, cx) = window(&user, forgetting(&user, hidden.clone()), 1280., cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    open_row(&shell, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the child is in the rail", |cx| {
        rail_ids(&page, cx).contains(&child)
    });
    settle(cx);
    cx.update(|window, cx| window.click(format!("rail-row-{child}"), cx));
    until(&shell, cx, "the child is shown", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.shown() == child)
    });

    // The child leaves the tree while the user is elsewhere.
    cx.update(|window, cx| window.click(project_id("app"), cx));
    settle(cx);
    hidden.lock().unwrap().insert(child.clone());
    back(cx);
    assert_eq!(selected(&shell, cx), Some(Selection::Session(lead.clone())));
    let page = session_page(&shell, cx);
    until(&shell, cx, "the tree is read", |cx| {
        !page.read_with(cx, |p, _| p.tree().is_empty())
    });
    assert_eq!(
        page.read_with(cx, |p, _| p.shown().to_owned()),
        lead,
        "the session's conversation, not `agent <id>`"
    );
}

#[gpui_kit::test]
fn alt_left_with_a_menu_open_closes_it_and_goes_nowhere(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx, ids) = atlas_with_sessions(&user, &["Tidy the aliases"], cx);
    open_row(&shell, &ids[0], cx);
    cx.update(|window, cx| window.click("workspace-menu", cx));
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("workspace-menu-menu").is_some()));

    cx.simulate_keystrokes("alt-left");
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("workspace-menu-menu").is_none()));
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(ids[0].clone())),
        "the key only closed the menu"
    );
}

// Stopping, ending and resuming (#100, #109): the composer's Stop and Escape interrupt the turn
// that runs, the agent staying alive and the conversation going on; "End session" in the root's
// menu ends the tree after a confirmation that counts the agents, moves the session to its
// project's "Ended" group and puts a line in place of the composer; Resume, when the agent can,
// brings the composer back.

fn tree_states(user: &User, lead: &str) -> Vec<(String, String)> {
    user.call("agent.tree", json!({"agent": lead})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["agent"]["id"].as_str().unwrap().to_owned(),
                entry["agent"]["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A lead with two workers kept mid-turn, and the window on its session.
fn lead_with_two_workers<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (String, Entity<Shell>, &'a mut VisualTestContext) {
    let (lead, mcp, _) = lead_in_w(user);
    worker(user, &mcp, "first");
    worker(user, &mcp, "second");
    let (shell, cx) = open_session(user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail shows the three agents", |cx| {
        rail_ids(&page, cx).len() == 3
    });
    settle(cx);
    (lead, shell, cx)
}

/// The sessions a place of the left tree lists.
fn listed_in(shell: &Entity<Shell>, place: Place, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.place("app", &place)
            .map(|node| {
                node.read(cx)
                    .sessions(cx)
                    .iter()
                    .map(|row| row.read(cx).id().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// The sessions the "Ended" group of `app` lists, and whether it is open.
fn ended_in_app(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (Vec<String>, bool) {
    shell.read_with(cx, |s, cx| {
        s.ended("app")
            .map(|node| {
                let node = node.read(cx);
                let ids = node
                    .sessions(cx)
                    .iter()
                    .map(|row| row.read(cx).id().to_owned())
                    .collect();
                (ids, node.is_open())
            })
            .unwrap_or_default()
    })
}

/// Opens an ended session from its project's "Ended" group, as a user would.
fn open_ended<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the Ended group is in the tree", |cx| {
        cx.update(|window, _| window.try_find("ended-app").is_some())
    });
    cx.update(|window, cx| window.click("ended-app", cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(
        &shell,
        cx,
        "the ended session's page, its process gone",
        |cx| {
            session_page(&shell, cx).read_with(cx, |page, cx| {
                !page.conversation().read(cx).rows().is_empty()
                    && page.tree().first().is_some_and(|root| !root.stopping)
            })
        },
    );
    settle(cx);
    (shell, cx)
}

#[gpui_kit::test]
fn stop_during_a_turn_cancels_it_and_the_next_message_is_taken(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    settle(cx);
    cx.update(|window, _| {
        assert!(
            window.try_find("session-stop").is_none(),
            "nothing on the page ends the agent but End session"
        )
    });

    send(&user, &lead, "HANG");
    until(&shell, cx, "the turn runs and Stop is offered", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
            && cx.update(|window, _| window.try_find("reply-stop").is_some())
    });
    cx.update(|window, cx| window.click("reply-stop", cx));
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    settle(cx);

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("go on with the plan");
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the daemon took the message", |cx| {
        reply.read_with(cx, |r, cx| {
            !r.is_sending() && r.field().read(cx).value().is_empty()
        })
    });
    assert_eq!(
        reply.read_with(cx, |r, _| r.error().map(str::to_owned)),
        None
    );
    eventually("the message is played as a new turn", || {
        user.received_requests("session/prompt")
            .iter()
            .filter_map(|p| p["params"]["prompt"][0]["text"].as_str().map(str::to_owned))
            .any(|text| text.contains("go on with the plan"))
    });
    let agent = user.call("agent.show", json!({"agent": lead})).unwrap();
    assert_ne!(agent["agent"]["state"], "ended", "the agent stays");
}

#[gpui_kit::test]
fn end_session_asks_ends_the_tree_moves_it_to_ended_and_replaces_the_composer(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, shell, cx) = lead_with_two_workers(&user, cx);
    let page = session_page(&shell, cx);
    assert_eq!(page.read_with(cx, |p, _| p.end_question()), None);
    assert_eq!(
        listed_in(&shell, Some("w".into()), cx),
        std::slice::from_ref(&lead)
    );

    cx.update(|window, cx| window.click("rail-agent-menu", cx));
    settle(cx);
    cx.update(|window, cx| window.click("rail-agent-end", cx));
    settle(cx);
    let asked = page.read_with(cx, |p, _| p.end_question()).unwrap();
    assert!(asked.contains("3 agents"), "{asked}");
    cx.update(|window, _| assert!(window.find("session-end-confirm").visible()));

    // Cancel ends nothing.
    cx.update(|window, cx| window.click("session-end-modal-cancel", cx));
    settle(cx);
    assert_eq!(page.read_with(cx, |p, _| p.end_question()), None);
    assert!(
        tree_states(&user, &lead).iter().all(|(_, s)| s != "ended"),
        "nothing ended"
    );

    // Confirmed: the root and its children end.
    cx.update(|window, cx| window.click("rail-agent-menu", cx));
    settle(cx);
    cx.update(|window, cx| window.click("rail-agent-end", cx));
    settle(cx);
    cx.update(|window, cx| window.click("session-end-confirm", cx));
    until(&shell, cx, "the tree ended", |cx| {
        page.read_with(cx, |p, cx| {
            p.tree().len() == 3
                && p.tree()
                    .iter()
                    .all(|a| p.state(&a.id, cx) == Some(AgentState::Stopped))
        })
    });
    let states = tree_states(&user, &lead);
    assert!(states.iter().all(|(_, s)| s == "ended"), "{states:?}");

    // A line in place of the composer, with Resume: the fake agent resumes its sessions.
    settle(cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.ended_line()),
        Some(("This session ended: stopped by you.".to_owned(), true))
    );
    cx.update(|window, _| {
        assert!(window.try_find("session-ended").is_some());
        assert!(window.try_find("session-resume").is_some());
        assert!(window.try_find("reply-field").is_none(), "no composer");
        assert!(
            window.try_find("rail-agent-menu").is_none(),
            "an ended session has nothing to end"
        );
    });

    // It left its place's list for its project's "Ended" group, folded, and opens from there.
    until(&shell, cx, "the session moved to Ended", |cx| {
        listed_in(&shell, Some("w".into()), cx).is_empty()
            && ended_in_app(&shell, cx).0 == [lead.clone()]
    });
    assert!(!ended_in_app(&shell, cx).1, "the group is folded");
    settle(cx);
    cx.update(|window, cx| window.click("ended-app", cx));
    settle(cx);
    assert!(ended_in_app(&shell, cx).1);
    cx.update(|window, _| assert!(window.find(session_id(&lead)).visible()));
}

#[gpui_kit::test]
fn resume_on_an_ended_session_brings_the_composer_back_and_a_message_is_answered(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    user.call("agent.stop", json!({"agent": lead})).unwrap();
    let (shell, cx) = open_ended(&user, &lead, cx);
    cx.update(|window, _| assert!(window.try_find("reply-field").is_none()));

    cx.update(|window, cx| window.click("session-resume", cx));
    until(&shell, cx, "the composer is back", |cx| {
        cx.update(|window, _| {
            window.try_find("reply-field").is_some() && window.try_find("session-ended").is_none()
        })
    });
    settle(cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("carry on");
    cx.simulate_keystrokes("enter");
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    until(&shell, cx, "the daemon took the message", |cx| {
        reply.read_with(cx, |r, cx| {
            !r.is_sending() && r.field().read(cx).value().is_empty()
        })
    });
    assert_eq!(
        reply.read_with(cx, |r, _| r.error().map(str::to_owned)),
        None
    );
    eventually("the message is answered on the same session", || {
        let shown = user.call("agent.show", json!({"agent": lead})).unwrap();
        shown["agent"]["state"] == "idle"
            && shown["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == "agent.started" && e["data"]["opened"] == "resumed")
            && shown["events"].as_array().unwrap().last().unwrap()["name"] == "agent.turn_ended"
    });
    let native =
        user.call("agent.show", json!({"agent": lead})).unwrap()["agent"]["native_id"].clone();
    let resumes = user.received_requests("session/resume");
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0]["params"]["sessionId"], native);
    until(&shell, cx, "the session is back in its place", |cx| {
        listed_in(&shell, Some("w".into()), cx) == [lead.clone()]
            && ended_in_app(&shell, cx).0.is_empty()
    });
}

#[gpui_kit::test]
fn no_resume_when_the_agent_can_neither_resume_nor_load(cx: &mut TestAppContext) {
    let user = team_user();
    user.fake_script(json!({"resume": false, "load": false}));
    let (lead, _mcp, _) = lead_in_w(&user);
    user.call("agent.stop", json!({"agent": lead})).unwrap();

    let (shell, cx) = open_ended(&user, &lead, cx);

    let page = session_page(&shell, cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.ended_line()),
        Some(("This session ended: stopped by you.".to_owned(), false))
    );
    cx.update(|window, _| {
        assert!(window.try_find("session-ended").is_some());
        assert!(window.try_find("session-resume").is_none());
        assert!(window.try_find("reply-field").is_none());
    });
}

/// The composer's error as it says it.
fn reply_error(reply: &Entity<Reply>, cx: &mut VisualTestContext) -> Option<String> {
    reply.read_with(cx, |r, _| r.error().map(str::to_owned))
}

/// The daemon refusing what the composer sent, as it answers.
fn refuse_reply(reply: &Entity<Reply>, why: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| reply.update(cx, |r, cx| r.replied(Some(why.to_owned()), window, cx)));
}

fn the_composer_is_back(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the composer is back", |cx| {
        cx.update(|window, _| {
            window.try_find("reply-field").is_some() && window.try_find("session-ended").is_none()
        })
    });
}

#[gpui_kit::test]
fn the_composer_s_old_error_goes_when_the_session_ends_and_when_it_is_resumed(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    refuse_reply(&reply, "refused: an old error", cx);
    assert!(reply_error(&reply, cx).is_some());

    user.call("agent.stop", json!({"agent": lead})).unwrap();
    let page = session_page(&shell, cx);
    until(&shell, cx, "the session ended, its process gone", |cx| {
        page.read_with(cx, |p, _| {
            p.ended_line().is_some() && p.tree().first().is_some_and(|root| !root.stopping)
        })
    });
    assert_eq!(reply_error(&reply, cx), None, "gone with the end");

    // An answer that came late, while it was ended, is not shown once it is resumed.
    refuse_reply(&reply, "refused: a late error", cx);
    settle(cx);
    cx.update(|window, cx| window.click("session-resume", cx));
    the_composer_is_back(&shell, cx);
    assert_eq!(reply_error(&reply, cx), None, "gone with the resume");
}

#[gpui_kit::test]
fn a_refused_resume_s_error_goes_when_the_session_is_resumed_and_stays_gone_at_its_next_end(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    user.call("agent.stop", json!({"agent": lead})).unwrap();
    let (shell, cx) = open_ended(&user, &lead, cx);
    let page = session_page(&shell, cx);
    // Taken, Resume stays disabled until the tree shows the root going again.
    page.update(cx, |p, cx| p.resumed(None, cx));
    assert!(!page.read_with(cx, |p, _| p.can_resume()));
    page.update(cx, |p, cx| {
        p.resumed(Some("refused: it is still stopping".into()), cx)
    });
    settle(cx);
    assert!(
        page.read_with(cx, |p, _| p.can_resume()),
        "refused, it can be asked again"
    );
    cx.update(|window, _| assert!(window.try_find("session-resume-error").is_some()));

    // Resumed from elsewhere (the command line), then ended again.
    user.resume(&lead);
    the_composer_is_back(&shell, cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.resume_error().map(str::to_owned)),
        None
    );
    user.call("agent.stop", json!({"agent": lead})).unwrap();
    until(&shell, cx, "the session ended again", |cx| {
        cx.update(|window, _| window.try_find("session-ended").is_some())
    });

    assert_eq!(
        page.read_with(cx, |p, _| p.resume_error().map(str::to_owned)),
        None
    );
    cx.update(|window, _| assert!(window.try_find("session-resume-error").is_none()));
}

#[gpui_kit::test]
fn resume_waits_for_the_ended_process_to_be_gone(cx: &mut TestAppContext) {
    // An agent slow to stop: deaf to SIGTERM and running on once its input closes, it is killed
    // 2 s after the stop.
    let user = team_user();
    let slow = user.path("slow-agent");
    std::fs::write(
        &slow,
        format!(
            "#!/bin/sh\ntrap '' TERM\nexec '{}' \"$@\"\n",
            common::fake_agent().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&slow, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let user = user.with_env("FLOTTILLE_AGENT_COMMAND", slow.to_str().unwrap());
    user.fake_script(json!({"linger": true}));
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);

    user.call("agent.stop", json!({"agent": lead})).unwrap();
    until(
        &shell,
        cx,
        "the session ended, its process stopping",
        |cx| {
            page.read_with(cx, |p, _| {
                p.ended_line().is_some() && p.tree().first().is_some_and(|root| root.stopping)
            })
        },
    );
    settle(cx);
    cx.update(|window, _| assert!(window.find("session-resume").visible()));
    assert!(!page.read_with(cx, |p, _| p.can_resume()), "disabled");

    until(&shell, cx, "its process is gone", |cx| {
        page.read_with(cx, |p, _| p.can_resume())
    });
    settle(cx);
    cx.update(|window, cx| window.click("session-resume", cx));
    the_composer_is_back(&shell, cx);
}

#[gpui_kit::test]
fn the_conversation_says_the_session_ended_then_was_resumed(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    user.call("agent.stop", json!({"agent": lead})).unwrap();
    let (shell, cx) = open_ended(&user, &lead, cx);
    let page = session_page(&shell, cx);

    cx.update(|window, cx| window.click("session-resume", cx));
    the_composer_is_back(&shell, cx);

    until(&shell, cx, "the resumed notice", |cx| {
        let kinds: Vec<String> = blocks(&page, cx)
            .iter()
            .filter_map(|block| match block {
                Block::Notice { kind, .. } => Some(kind.clone()),
                _ => None,
            })
            .collect();
        kinds.ends_with(&["ended".to_owned(), "resumed".to_owned()])
    });
}

#[gpui_kit::test]
fn an_ended_session_s_header_says_stopped_or_failed_as_its_rail_does(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    assert_eq!(
        page.read_with(cx, |p, cx| p.header_state(cx)),
        AgentState::Idle
    );

    user.call("agent.stop", json!({"agent": lead})).unwrap();
    until(&shell, cx, "the header says stopped", |cx| {
        page.read_with(cx, |p, cx| p.header_state(cx) == AgentState::Stopped)
    });

    // Resumed, it says what the left tree says again.
    user.resume(&lead);
    until(&shell, cx, "the header follows the left tree again", |cx| {
        page.read_with(cx, |p, cx| p.header_state(cx) == AgentState::Idle)
    });

    // Failed: its process dies at each message, until it is replaced too often.
    user.fake_script(json!({"turns": [{"when": "DIE", "do": [{"die": 3}]}]}));
    send(&user, &lead, "DIE");
    until(&shell, cx, "the header says failed", |cx| {
        page.read_with(cx, |p, cx| p.header_state(cx) == AgentState::Failed)
    });
}

#[gpui_kit::test]
fn a_sub_agent_has_no_stop_of_its_own(cx: &mut TestAppContext) {
    let user = team_user();
    let (_lead, shell, cx) = lead_with_two_workers(&user, cx);
    let page = session_page(&shell, cx);
    let worker = rail_ids(&page, cx)[1].clone();
    cx.update(|window, cx| window.click(format!("rail-row-{worker}"), cx));
    settle(cx);
    cx.update(|window, _| {
        assert!(
            window
                .try_find(format!("rail-agent-menu-{worker}"))
                .is_none()
                && window.try_find("reply-stop").is_none(),
            "only the root is stopped, and the daemon refuses a child"
        )
    });
}

#[gpui_kit::test]
fn stop_and_escape_cancel_the_turn_that_runs_and_the_agent_stays_alive(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .unwrap();
    settle(cx);
    cx.update(|window, _| {
        assert!(
            window.try_find("reply-stop").is_none(),
            "no Stop while no turn runs"
        )
    });
    let cancels = |user: &User| user.received_requests("session/cancel").len();

    // Escape with no turn running does nothing.
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(cancels(&user), 0);

    // The Stop button.
    send(&user, &lead, "HANG");
    until(
        &shell,
        cx,
        "the turn runs and the composer offers Stop",
        |cx| {
            reply.read_with(cx, |r, _| r.is_running())
                && cx.update(|window, _| window.try_find("reply-stop").is_some())
        },
    );
    cx.update(|window, cx| window.click("reply-stop", cx));
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    assert_eq!(cancels(&user), 1);
    let agent = user.call("agent.show", json!({"agent": lead})).unwrap();
    assert_eq!(agent["agent"]["state"], "idle", "the agent stays alive");

    // Escape in the composer.
    send(&user, &lead, "HANG");
    until(&shell, cx, "the second turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_keystrokes("escape");
    until(&shell, cx, "the second turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    assert_eq!(cancels(&user), 2);
    let agent = user.call("agent.show", json!({"agent": lead})).unwrap();
    assert_eq!(agent["agent"]["state"], "idle");
    cx.update(|window, _| assert!(window.try_find("reply-stop").is_none()));
}

// The WORKSPACE section names each repository's forge as the daemon reads it.

#[gpui_kit::test]
fn the_workspace_section_names_each_repository_s_forge(cx: &mut TestAppContext) {
    let user = team_user().with_fake_gh();
    let (lead, _mcp, _) = lead_in_w(&user);
    user.git(
        &user.path("work/app"),
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    declare_on_github(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the forge in the rail", |cx| {
        page.read_with(cx, |p, cx| {
            p.rail()
                .read(cx)
                .place()
                .is_some_and(|p| !p.forges.is_empty())
        })
    });
    let forges = page.read_with(cx, |p, cx| {
        p.rail().read(cx).place().unwrap().forges.clone()
    });
    assert_eq!(forges, [(".".to_owned(), "github · acme/app".to_owned())]);
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("rail-forge-app").is_some()));
}

#[gpui_kit::test]
fn a_repository_with_no_forge_says_why_in_the_workspace_section(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the forge in the rail", |cx| {
        page.read_with(cx, |p, cx| {
            p.rail()
                .read(cx)
                .place()
                .is_some_and(|p| !p.forges.is_empty())
        })
    });
    let forges = page.read_with(cx, |p, cx| {
        p.rail().read(cx).place().unwrap().forges.clone()
    });
    assert_eq!(
        forges,
        [(
            ".".to_owned(),
            "no forge set for this repository: choose it in the project's Settings (the \
             window), or write its `forge` in the project's file; Flottille never guesses it"
                .to_owned()
        )]
    );
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("rail-forge-app").is_some()));
}

// ── Commands and skills in the project's settings (#127) ──────────────────────────────────────

/// The project's settings open in a window tall enough to show its Skills and Commands, which
/// come after its forges, tracker and rules.
fn sections_of(shell: &Entity<Shell>, project: &str, cx: &mut VisualTestContext) {
    cx.simulate_resize(size(px(1280.), px(2400.)));
    open_project_settings(shell, project, cx);
}

fn commands_section(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::commands::CommandsSection> {
    settings_of(shell, cx)
        .expect("the project's settings")
        .read_with(cx, |page, _| page.commands().clone())
}

fn skills_section(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::skills::SkillsSection> {
    settings_of(shell, cx)
        .expect("the project's settings")
        .read_with(cx, |page, _| page.skills().clone())
}

fn commands_listed(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Vec<flottille_window::commands::CommandInfo> {
    commands_section(shell, cx).read_with(cx, |section, _| {
        section.commands().unwrap_or_default().to_vec()
    })
}

fn skills_listed(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Vec<flottille_window::skills::SkillInfo> {
    skills_section(shell, cx).read_with(cx, |section, _| {
        section.skills().unwrap_or_default().to_vec()
    })
}

fn text_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// How many agents the daemon has, started or not.
fn agents_started(user: &User) -> usize {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .len()
}

#[gpui_kit::test]
fn a_command_added_from_a_template_appears_in_the_project_s_settings(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    sections_of(&shell, "atlas", cx);
    until(&shell, cx, "the empty list is shown", |cx| {
        cx.update(|window, _| window.try_find("no-commands").is_some())
    });

    cx.update(|window, cx| window.click("add-command", cx));
    settle(cx);
    cx.update(|window, _| {
        for template in ["open-prs", "merge-stack", "close"] {
            assert!(window.find(format!("template-{template}")).visible());
        }
        assert!(window.find("blank-command").visible());
    });
    cx.update(|window, cx| window.click("template-open-prs", cx));
    until(&shell, cx, "the command is listed", |cx| {
        !commands_listed(&shell, cx).is_empty()
    });
    settle(cx);

    let listed = commands_listed(&shell, cx);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "open-prs");
    assert!(
        text_of("command-description-open-prs", cx)
            .unwrap()
            .contains("pull requests")
    );
    // The daemon wrote it into the project's file, as the command line does.
    assert!(
        std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("[commands.open-prs]")
    );
}

#[gpui_kit::test]
fn a_blank_command_is_written_from_the_form_and_a_refusal_says_why(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    sections_of(&shell, "atlas", cx);
    let section = commands_section(&shell, cx);

    cx.update(|window, cx| window.click("add-command", cx));
    settle(cx);
    cx.update(|window, cx| window.click("blank-command", cx));
    settle(cx);
    // Nothing typed: the daemon refuses and the form says why, staying open.
    cx.update(|window, cx| window.click("save-command", cx));
    until(&shell, cx, "the refusal is shown", |cx| {
        section.read_with(cx, |s, _| s.form_error().is_some())
    });
    assert!(section.read_with(cx, |s, _| s.is_open()));

    cx.simulate_input("tidy");
    let description = section.read_with(cx, |s, _| s.description_field().unwrap().clone());
    cx.update(|window, cx| {
        description.update(cx, |f, cx| f.set_value("Tidy the branch", window, cx))
    });
    let instructions = section.read_with(cx, |s, _| s.instructions_field().unwrap().clone());
    cx.update(|window, cx| {
        instructions.update(cx, |f, cx| f.set_value("Remove the dead code.", window, cx))
    });
    cx.update(|window, cx| window.click("save-command", cx));
    until(&shell, cx, "the command is listed", |cx| {
        !commands_listed(&shell, cx).is_empty()
    });

    let listed = commands_listed(&shell, cx);
    assert_eq!(listed[0].name, "tidy");
    assert_eq!(listed[0].description, "Tidy the branch");
    assert!(
        !section.read_with(cx, |s, _| s.is_open()),
        "the modal closed"
    );
}

#[gpui_kit::test]
fn editing_a_command_in_the_modal_writes_the_project_s_file_and_a_removal_asks_twice(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    user.call(
        "command.add",
        json!({"project": "atlas", "template": "close"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    sections_of(&shell, "atlas", cx);
    until(&shell, cx, "the command is listed", |cx| {
        !commands_listed(&shell, cx).is_empty()
    });
    settle(cx);

    cx.update(|window, cx| window.click("edit-command-close", cx));
    let section = commands_section(&shell, cx);
    until(&shell, cx, "the form is open on it", |cx| {
        section.read_with(cx, |s, _| s.is_open() && s.description_field().is_some())
    });
    settle(cx);
    let description = section.read_with(cx, |s, _| s.description_field().unwrap().clone());
    cx.update(|window, cx| description.update(cx, |f, cx| f.set_value("Close it all", window, cx)));
    cx.update(|window, cx| window.click("save-command", cx));
    until(&shell, cx, "the file says so", |_| {
        std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("description = \"Close it all\"")
    });
    until(&shell, cx, "the list follows", |cx| {
        commands_listed(&shell, cx)
            .first()
            .is_some_and(|c| c.description == "Close it all")
    });
    settle(cx);

    // A removal needs a second click.
    cx.update(|window, cx| window.click("remove-command-close", cx));
    settle(cx);
    assert!(
        std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("[commands.close]")
    );
    cx.update(|window, cx| window.click("confirm-remove-command-close", cx));
    until(&shell, cx, "the list is empty", |cx| {
        commands_listed(&shell, cx).is_empty()
    });
    assert!(
        !std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("commands.close")
    );
}

/// A project with a ready workspace `login` and the commands `open-prs` and `close`.
fn user_with_commands() -> User {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    for template in ["open-prs", "close"] {
        user.call(
            "command.add",
            json!({"project": "atlas", "template": template}),
        )
        .unwrap();
    }
    user
}

/// The reply of the session open now.
fn reply_open(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<Reply> {
    let page = session_page(shell, cx);
    page.read_with(cx, |page, _| page.reply().cloned())
        .expect("the session's composer")
}

/// The names the `/` list offers now.
fn slash_offered(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    let reply = reply_open(shell, cx);
    reply.read_with(cx, |reply, _| {
        reply
            .slash_entries()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    })
}

/// What the session's root was sent, in order.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .filter_map(|p| p["params"]["prompt"][0]["text"].as_str().map(str::to_owned))
        .collect()
}

#[gpui_kit::test]
fn slash_lists_the_commands_and_filters_them_as_the_user_types(cx: &mut TestAppContext) {
    let user = user_with_commands();
    let lead = start(&user, "atlas", Some("login"), "Fix the login");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    settle(cx);

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("/");
    until(&shell, cx, "the list offers the commands", |cx| {
        slash_offered(&shell, cx).len() == 2
    });
    settle(cx);
    assert_eq!(slash_offered(&shell, cx), ["close", "open-prs"]);
    let reply = reply_open(&shell, cx);
    let entries = reply.read_with(cx, |r, _| r.slash_entries());
    assert!(
        entries[1].1.contains("pull requests"),
        "each with its description: {entries:?}"
    );
    cx.update(|window, _| {
        assert!(window.find("slash-list").visible());
        assert!(window.find("slash-open-prs").visible());
        assert!(window.find("slash-close").visible());
    });

    cx.simulate_input("op");
    settle(cx);
    assert_eq!(slash_offered(&shell, cx), ["open-prs"]);
    cx.update(|window, _| assert!(window.try_find("slash-close").is_none()));

    // Escape closes it; the text stays.
    cx.simulate_keystrokes("escape");
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("slash-list").is_none()));
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "/op"
    );
}

#[gpui_kit::test]
fn picking_a_command_with_slash_puts_its_name_in_the_field_and_sends_nothing(
    cx: &mut TestAppContext,
) {
    let user = user_with_commands();
    let lead = start(&user, "atlas", Some("login"), "Fix the login");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    settle(cx);
    let before = agents_started(&user);
    let sent = prompts(&user).len();

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("/");
    until(&shell, cx, "the list offers the commands", |cx| {
        slash_offered(&shell, cx).len() == 2
    });
    // The keyboard picks: down to the second, Enter.
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("enter");
    settle(cx);
    let reply = reply_open(&shell, cx);
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "/open-prs ",
        "the name waits for what the command is about"
    );
    cx.update(|window, _| assert!(window.try_find("slash-list").is_none()));
    assert_eq!(prompts(&user).len(), sent, "nothing was sent to the lead");
    assert_eq!(agents_started(&user), before, "nothing started");

    // A click on an entry picks it too.
    reply.update_in(cx, |r, window, cx| {
        r.field()
            .clone()
            .update(cx, |f, cx| f.set_value("", window, cx));
        r.typed("", cx);
    });
    cx.simulate_input("/");
    until(&shell, cx, "the list is back", |cx| {
        slash_offered(&shell, cx).len() == 2
    });
    settle(cx);
    cx.update(|window, cx| window.click("slash-close", cx));
    settle(cx);
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "/close "
    );
    assert_eq!(prompts(&user).len(), sent);
}

#[gpui_kit::test]
fn slash_name_sends_the_instructions_with_the_trailing_text(cx: &mut TestAppContext) {
    let user = user_with_commands();
    let instructions = user
        .call("command.show", json!({"project": "atlas", "name": "close"}))
        .unwrap()["instructions"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    let lead = start(&user, "atlas", Some("login"), "Fix the login");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    settle(cx);

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("/close keep the branch");
    settle(cx);
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the lead was sent the command", |_| {
        prompts(&user)
            .iter()
            .any(|text| text.contains(&instructions))
    });

    let sent = prompts(&user)
        .into_iter()
        .find(|text| text.contains(&instructions))
        .unwrap();
    assert!(
        sent.contains("\nThe command close of project atlas"),
        "{sent}"
    );
    assert!(
        sent.ends_with(&format!("{instructions}\n\nkeep the branch")),
        "the trailing text after the instructions: {sent}"
    );
    assert!(!sent.contains("/close"), "{sent}");
}

#[gpui_kit::test]
fn in_main_slash_lists_the_commands_too(cx: &mut TestAppContext) {
    let user = user_with_commands();
    let lead = start(&user, "atlas", None, "Read the notes");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    settle(cx);

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("/op");
    until(&shell, cx, "the list offers it", |cx| {
        slash_offered(&shell, cx) == ["open-prs"]
    });
    settle(cx);
    cx.update(|window, _| assert!(window.find("slash-open-prs").visible()));
}

#[gpui_kit::test]
fn an_unknown_slash_word_is_sent_as_text(cx: &mut TestAppContext) {
    let user = user_with_commands();
    let lead = start(&user, "atlas", Some("login"), "Fix the login");
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    settle(cx);
    let before = agents_started(&user);

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("/deploy");
    until(&shell, cx, "the commands are known", |cx| {
        reply_open(&shell, cx).read_with(cx, |r, _| r.knows_commands())
    });
    settle(cx);
    assert!(
        slash_offered(&shell, cx).is_empty(),
        "no command is named so"
    );
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the lead was sent the text", |_| {
        prompts(&user)
            .iter()
            .any(|text| text.ends_with("\n/deploy"))
    });
    assert_eq!(agents_started(&user), before);
}

#[gpui_kit::test]
fn the_template_list_adds_a_template_and_a_blank_one(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    sections_of(&shell, "atlas", cx);
    until(&shell, cx, "the empty list is shown", |cx| {
        cx.update(|window, _| window.try_find("no-commands").is_some())
    });
    let section = commands_section(&shell, cx);

    cx.update(|window, cx| window.click("add-command", cx));
    settle(cx);
    // One list: each template with its description, then the blank one last.
    let rows = cx.update(|window, _| {
        ["open-prs", "merge-stack", "close"]
            .map(|t| format!("template-{t}"))
            .into_iter()
            .chain(["blank-command".to_owned()])
            .map(|id| {
                let found = window.find(id.clone());
                assert!(found.visible(), "{id}");
                (
                    found.label().unwrap_or_default().to_owned(),
                    found.bounds().top(),
                )
            })
            .collect::<Vec<_>>()
    });
    assert!(rows[0].0.contains("open-prs"), "{rows:?}");
    assert!(
        rows.windows(2).all(|pair| pair[0].1 < pair[1].1),
        "one under the other, the blank one last: {rows:?}"
    );
    assert!(
        text_of("template-open-prs-line", cx)
            .unwrap()
            .contains("pull requests")
    );

    cx.update(|window, cx| window.click("template-close", cx));
    until(&shell, cx, "the template is listed", |cx| {
        commands_listed(&shell, cx)
            .iter()
            .any(|c| c.name == "close")
    });
    settle(cx);

    cx.update(|window, cx| window.click("add-command", cx));
    settle(cx);
    cx.update(|window, cx| window.click("blank-command", cx));
    settle(cx);
    cx.simulate_input("tidy");
    let description = section.read_with(cx, |s, _| s.description_field().unwrap().clone());
    cx.update(|window, cx| {
        description.update(cx, |f, cx| f.set_value("Tidy the branch", window, cx))
    });
    let instructions = section.read_with(cx, |s, _| s.instructions_field().unwrap().clone());
    cx.update(|window, cx| {
        instructions.update(cx, |f, cx| f.set_value("Remove the dead code.", window, cx))
    });
    cx.update(|window, cx| window.click("save-command", cx));
    until(&shell, cx, "the blank one is listed", |cx| {
        commands_listed(&shell, cx).iter().any(|c| c.name == "tidy")
    });
}

#[gpui_kit::test]
fn the_card_and_the_session_menu_have_no_run_procedure_entry(cx: &mut TestAppContext) {
    let user = user_with_commands();
    let lead = start(&user, "atlas", Some("login"), "Fix the login");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the workspace's card is drawn", |cx| {
        cx.update(|window, _| window.try_find("place-card-login").is_some())
    });
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("run-procedure-login").is_none()));

    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Session(lead.clone()), window, cx)
    });
    until(&shell, cx, "the session is open", |cx| {
        selected(&shell, cx) == Some(Selection::Session(lead.clone()))
    });
    settle(cx);
    cx.update(|window, cx| window.click("workspace-menu", cx));
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find("open-in-editor").is_some());
        assert!(window.try_find("run-procedure").is_none());
    });
}

#[gpui_kit::test]
fn a_skill_is_added_with_a_path_picked_in_the_project_folder_and_removed(cx: &mut TestAppContext) {
    let user = user();
    let folder = project(&user, "atlas");
    std::fs::create_dir_all(folder.join("docs")).unwrap();
    std::fs::write(folder.join("docs/react.md"), "How we write components.\n").unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    sections_of(&shell, "atlas", cx);
    let section = skills_section(&shell, cx);
    until(&shell, cx, "the empty list is shown", |cx| {
        cx.update(|window, _| window.try_find("no-skills").is_some())
    });

    cx.update(|window, cx| window.click("add-skill", cx));
    settle(cx);
    cx.simulate_input("react");
    let description = section.read_with(cx, |s, _| s.description_field().unwrap().clone());
    cx.update(|window, cx| {
        description.update(cx, |f, cx| {
            f.set_value("How we write components", window, cx)
        })
    });
    // The picker lists the project folder (never `.git`): a folder opens and is picked, a file
    // inside it is picked.
    cx.update(|window, _| {
        assert!(window.try_find("skill-entry-docs").is_some());
        assert!(window.try_find("skill-entry-README.md").is_some());
        assert!(window.try_find("skill-entry-.git").is_none());
    });
    cx.update(|window, cx| window.click("skill-entry-docs", cx));
    settle(cx);
    assert_eq!(section.read_with(cx, |s, cx| s.picked(cx)), "docs");
    cx.update(|window, cx| window.click("skill-entry-react.md", cx));
    settle(cx);
    assert_eq!(section.read_with(cx, |s, cx| s.picked(cx)), "docs/react.md");
    cx.update(|window, cx| window.click("save-skill", cx));
    until(&shell, cx, "the skill is listed", |cx| {
        !skills_listed(&shell, cx).is_empty()
    });
    settle(cx);

    let listed = skills_listed(&shell, cx);
    assert_eq!(
        (listed[0].name.as_str(), listed[0].path.as_deref()),
        ("react", Some("docs/react.md"))
    );
    assert!(
        text_of("skill-path-react", cx)
            .unwrap()
            .contains("docs/react.md")
    );
    assert!(
        text_of("skill-description-react", cx)
            .unwrap()
            .contains("components")
    );
    assert!(!section.read_with(cx, |s, _| s.is_open()));
    assert!(
        std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("[skills.react]")
    );

    cx.update(|window, cx| window.click("remove-skill-react", cx));
    settle(cx);
    assert_eq!(skills_listed(&shell, cx).len(), 1, "the first click asks");
    cx.update(|window, cx| window.click("confirm-remove-skill-react", cx));
    until(&shell, cx, "the list is empty", |cx| {
        skills_listed(&shell, cx).is_empty()
    });
    assert!(
        !std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("skills.react")
    );
}

#[gpui_kit::test]
fn a_text_skill_is_written_in_the_settings_and_read_back_to_edit(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    sections_of(&shell, "atlas", cx);
    let section = skills_section(&shell, cx);

    cx.update(|window, cx| window.click("add-skill", cx));
    settle(cx);
    cx.simulate_input("style");
    cx.update(|window, cx| window.click("skill-as-text", cx));
    settle(cx);
    assert!(section.read_with(cx, |s, _| s.is_text()));
    let description = section.read_with(cx, |s, _| s.description_field().unwrap().clone());
    let text = section.read_with(cx, |s, _| s.text_field().unwrap().clone());
    cx.update(|window, cx| {
        description.update(cx, |f, cx| f.set_value("The code style", window, cx));
        text.update(cx, |f, cx| f.set_value("Short functions.", window, cx));
    });
    cx.update(|window, cx| window.click("save-skill", cx));
    until(&shell, cx, "the skill is listed", |cx| {
        !skills_listed(&shell, cx).is_empty()
    });
    settle(cx);

    let listed = skills_listed(&shell, cx);
    assert_eq!(listed[0].name, "style");
    assert_eq!(listed[0].path, None, "a text Flottille keeps");
    assert!(
        !std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("Short functions."),
        "the text is not in the project's file"
    );

    cx.update(|window, cx| window.click("edit-skill-style", cx));
    until(&shell, cx, "the text is read back", |cx| {
        section.read_with(cx, |s, cx| {
            s.is_open()
                && s.text_field()
                    .is_some_and(|f| f.read(cx).value() == "Short functions.")
        })
    });
    settle(cx);
    let text = section.read_with(cx, |s, _| s.text_field().unwrap().clone());
    cx.update(|window, cx| text.update(cx, |f, cx| f.set_value("Short and pure.", window, cx)));
    cx.update(|window, cx| window.click("save-skill", cx));
    until(&shell, cx, "the modal closes", |cx| {
        !section.read_with(cx, |s, _| s.is_open())
    });
    let shown = user
        .call("skill.show", json!({"project": "atlas", "name": "style"}))
        .unwrap();
    assert_eq!(shown["text"], "Short and pure.");
}

#[gpui_kit::test]
fn a_skill_the_daemon_refuses_says_why_in_the_modal_and_adds_nothing(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    sections_of(&shell, "atlas", cx);
    let section = skills_section(&shell, cx);

    cx.update(|window, cx| window.click("add-skill", cx));
    settle(cx);
    cx.simulate_input("elsewhere");
    let description = section.read_with(cx, |s, _| s.description_field().unwrap().clone());
    let path = section.read_with(cx, |s, _| s.path_field().unwrap().clone());
    cx.update(|window, cx| {
        description.update(cx, |f, cx| f.set_value("Outside", window, cx));
        path.update(cx, |f, cx| f.set_value("../outside", window, cx));
    });
    cx.update(|window, cx| window.click("save-skill", cx));
    until(&shell, cx, "the refusal is shown", |cx| {
        section.read_with(cx, |s, _| s.form_error().is_some())
    });
    settle(cx);

    assert!(
        section.read_with(cx, |s, _| s.is_open()),
        "the modal stays open"
    );
    let why = section.read_with(cx, |s, _| s.form_error().unwrap().to_owned());
    assert!(
        why.contains("../outside"),
        "the daemon's sentence names the path: {why}"
    );
    assert!(cx.update(|window, _| window.try_find("skill-error").is_some()));
    assert!(
        user.call("skill.list", json!({"project": "atlas"}))
            .unwrap()["skills"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn settings_of(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Option<Entity<ProjectSettingsPage>> {
    shell.read_with(cx, |s, cx| {
        s.main().read(cx).project_settings_page().cloned()
    })
}

/// Opens the project's settings from its page's header, and waits for what the daemon read.
fn open_project_settings(
    shell: &Entity<Shell>,
    project: &str,
    cx: &mut VisualTestContext,
) -> Entity<ProjectSettingsPage> {
    open_project(shell, project, cx);
    cx.update(|window, cx| window.click("project-settings", cx));
    until(shell, cx, "the settings are read", |cx| {
        settings_of(shell, cx).is_some_and(|page| {
            page.read_with(cx, |page, _| page.name() == project && page.is_loaded())
        })
    });
    settle(cx);
    settings_of(shell, cx).unwrap()
}

#[gpui_kit::test]
fn the_settings_page_proposes_the_remote_s_forge_and_nothing_is_read_until_it_is_saved(
    cx: &mut TestAppContext,
) {
    let user = user().with_fake_gh();
    let folder = project(&user, "atlas");
    user.git(
        &folder,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:acme/atlas.git",
        ],
    );
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let page = open_project_settings(&shell, "atlas", cx);

    page.read_with(cx, |page, _| {
        let row = &page.repositories()[0];
        assert_eq!(row.kind, ForgeKind::Github);
        assert!(row.proposed(), "pre-filled from the remote, not declared");
        assert_eq!(row.declared, None);
    });
    cx.update(|window, _| {
        assert!(window.find("forge-kind-.").visible());
        assert!(window.find("save-project-settings").visible());
    });
    let before = user
        .call("project.settings", json!({"name": "atlas"}))
        .unwrap();
    assert_eq!(before["repositories"][0]["forge"], serde_json::Value::Null);
    assert!(
        !std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("forge"),
        "nothing is active before the user saves"
    );
    assert!(user.gh_received().is_empty(), "nothing was asked of GitHub");

    cx.update(|window, cx| window.click("save-project-settings", cx));
    until(&shell, cx, "the settings are saved", |cx| {
        page.read_with(cx, |page, _| matches!(page.saved(), Some(Said::Fine(_))))
    });
    let after = user
        .call("project.settings", json!({"name": "atlas"}))
        .unwrap();
    assert_eq!(
        after["repositories"][0]["forge"],
        json!({"kind": "github", "repository": "acme/atlas"}),
        "{after}"
    );
    let file = std::fs::read_to_string(user.project_file("atlas")).unwrap();
    assert!(file.contains("forge = {"), "{file}");
    page.read_with(cx, |page, _| {
        assert!(!page.repositories()[0].proposed(), "now declared");
    });
}

#[gpui_kit::test]
fn a_project_is_added_from_the_window_checked_again_and_removed(cx: &mut TestAppContext) {
    let user = user();
    let folder = user.path("work/beta");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "beta\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the window is read", |cx| {
        cx.update(|window, _| window.try_find("add-first-project").is_some())
    });

    cx.update(|window, cx| window.click("add-first-project", cx));
    settle(cx);
    let form = shell.read_with(cx, |s, _| s.add_project_form().clone());
    assert!(form.read_with(cx, |f, _| f.is_open()));
    cx.simulate_input(folder.to_str().unwrap());
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the repositories are detected", |cx| {
        form.read_with(cx, |f, _| f.detected().is_some())
    });
    let detected = form.read_with(cx, |f, _| f.detected().cloned().unwrap());
    assert_eq!(detected.name, "beta");
    assert_eq!(detected.repositories, vec![".".to_owned()]);
    assert!(
        user.call("project.list", json!({})).unwrap()["projects"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing is registered before Add"
    );
    settle(cx);
    cx.update(|window, _| assert!(window.find("add-project-detected").visible()));

    cx.update(|window, cx| window.click("add-project-next", cx));
    until(&shell, cx, "its settings open", |cx| {
        settings_of(&shell, cx).is_some_and(|page| {
            page.read_with(cx, |page, _| page.name() == "beta" && page.is_loaded())
        })
    });
    assert!(!form.read_with(cx, |f, _| f.is_open()));
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::ProjectSettings("beta".into()))
    );
    let page = settings_of(&shell, cx).unwrap();
    settle(cx);

    cx.update(|window, cx| window.click("check-project", cx));
    until(&shell, cx, "the project is checked", |cx| {
        page.read_with(cx, |page, _| page.checked().is_some())
    });
    assert!(
        matches!(
            page.read_with(cx, |p, _| p.checked().cloned()),
            Some(Said::Fine(_))
        ),
        "the folder holds the project's repositories"
    );

    settle(cx);
    cx.update(|window, cx| window.click("remove-project", cx));
    settle(cx);
    cx.update(|window, cx| window.click("confirm-remove-project", cx));
    until(&shell, cx, "the project is gone", |cx| {
        shell.read_with(cx, |s, _| {
            !matches!(s.selected(), Some(Selection::ProjectSettings(_)))
        })
    });
    assert!(
        user.call("project.list", json!({})).unwrap()["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        folder.join("README.md").exists(),
        "its folder is left as it is"
    );
}

#[gpui_kit::test]
fn the_new_workspace_modal_previews_the_ticket_as_it_is_typed(cx: &mut TestAppContext) {
    let user = user().with_fake_gh();
    let folder = project(&user, "atlas");
    user.git(
        &folder,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:acme/atlas.git",
        ],
    );
    user.call(
        "project.set_settings",
        json!({"name": "atlas",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/atlas"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    user.gh_script(json!({"issues": {"acme/atlas": {"12": {
        "number": 12, "title": "The login fails", "state": "OPEN", "body": "",
        "url": "https://github.com/acme/atlas/issues/12", "comments": []
    }}}}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);

    cx.update(|window, cx| window.click("new-workspace-modal-ticket", cx));
    cx.simulate_input("12");
    assert_eq!(
        form.read_with(cx, |f, _| f.preview().cloned()),
        Some(("12".to_owned(), None)),
        "read once typing rests"
    );
    cx.executor().advance_clock(Duration::from_millis(400));
    until(&shell, cx, "the ticket is previewed", |cx| {
        form.read_with(cx, |f, _| {
            f.preview().is_some_and(|(_, read)| read.is_some())
        })
    });
    let read = form
        .read_with(cx, |f, _| f.preview().cloned())
        .unwrap()
        .1
        .unwrap()
        .unwrap();
    assert_eq!(read["title"], "The login fails", "{read}");
    assert_eq!(read["tracker_said"], "GitHub issues of .");
    settle(cx);
    cx.update(|window, _| assert!(window.find("new-workspace-ticket-preview").visible()));

    // A key of another tracker says why it is not taken, before anything is created.
    cx.simulate_keystrokes("backspace backspace");
    cx.simulate_input("ABC-1");
    cx.executor().advance_clock(Duration::from_millis(400));
    until(&shell, cx, "the other key is previewed", |cx| {
        form.read_with(cx, |f, _| {
            f.preview()
                .is_some_and(|(ticket, read)| ticket == "ABC-1" && read.is_some())
        })
    });
    let other = form
        .read_with(cx, |f, _| f.preview().cloned())
        .unwrap()
        .1
        .unwrap()
        .unwrap();
    assert!(
        other["problem"]
            .as_str()
            .unwrap()
            .starts_with("ABC-1 is a Jira key"),
        "{other}"
    );
}

#[gpui_kit::test]
fn the_new_workspace_modal_takes_the_chosen_repositories_branch_and_offline(
    cx: &mut TestAppContext,
) {
    let user = user();
    let folder = user.path("work/duo");
    for repository in ["api", "web"] {
        let dir = folder.join(repository);
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{repository}\n")).unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
        let remote = user.path(&format!("remotes/{repository}.git"));
        user.git(
            &user.path(""),
            &["init", "--quiet", "--bare", remote.to_str().unwrap()],
        );
        user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        user.git(&dir, &["push", "--quiet", "origin", "main"]);
    }
    user.call("project.add", json!({"folder": folder, "name": "duo"}))
        .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "duo", cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("duo").cloned())
        .unwrap();
    assert_eq!(
        form.read_with(cx, |f, _| f.chosen().to_vec()),
        vec!["api".to_owned(), "web".to_owned()],
        "every repository by default"
    );
    cx.update(|window, cx| window.click("new-workspace", cx));
    settle(cx);

    cx.simulate_input("login");
    cx.update(|window, cx| window.click("new-workspace-take-web", cx));
    cx.update(|window, cx| window.click("new-workspace-modal-branch", cx));
    cx.simulate_input("feature/sign-in");
    cx.update(|window, cx| window.click("new-workspace-offline", cx));
    assert_eq!(
        form.read_with(cx, |f, _| f.chosen().to_vec()),
        vec!["api".to_owned()]
    );
    assert!(form.read_with(cx, |f, _| f.offline()));

    cx.update(|window, cx| window.click("create-workspace", cx));
    until(&shell, cx, "the workspace is made", |cx| {
        !form.read_with(cx, |f, _| f.is_open() || f.creating().is_some())
    });
    let shown = user
        .call("workspace.show", json!({"project": "duo", "name": "login"}))
        .unwrap();
    assert_eq!(shown["branch"], "feature/sign-in", "{shown}");
    let taken: Vec<&str> = shown["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().or(r["path"].as_str()).unwrap())
        .collect();
    assert_eq!(taken, ["api"], "{shown}");
    assert!(
        !form.read_with(cx, |f, _| f.offline()),
        "the form starts afresh"
    );
}

#[gpui_kit::test]
fn the_project_at_the_root_of_the_picked_folder_is_named_after_its_folder(cx: &mut TestAppContext) {
    let user = user();
    let folder = user.path("work/shop");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "shop\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the window is read", |cx| {
        cx.update(|window, _| window.try_find("add-first-project").is_some())
    });

    cx.update(|window, cx| window.click("add-first-project", cx));
    settle(cx);
    let form = shell.read_with(cx, |s, _| s.add_project_form().clone());
    cx.simulate_input(folder.to_str().unwrap());
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the repositories are detected", |cx| {
        form.read_with(cx, |f, _| f.detected().is_some())
    });
    settle(cx);

    assert_eq!(
        form.read_with(cx, |f, _| f.detected().unwrap().repositories.clone()),
        vec![".".to_owned()],
        "the daemon names the root `.`"
    );
    assert_eq!(
        text_of("add-project-repository-0", cx).as_deref(),
        Some("shop"),
        "the window names it after its folder"
    );
}

#[gpui_kit::test]
fn a_project_with_only_main_and_no_session_shows_an_empty_state(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let page = page_of(&shell, cx);
    until(&shell, cx, "the page knows its places", |cx| {
        page.read_with(cx, |page, _| page.is_empty())
    });
    settle(cx);

    cx.update(|window, _| {
        assert!(window.find("project-empty").visible());
        assert!(window.find("empty-new-session").visible());
        assert!(window.find("empty-new-workspace").visible());
        assert!(
            window.try_find("place-card-main").is_none(),
            "no bare card of main"
        );
        assert!(window.try_find("new-workspace-card").is_none());
    });

    // Its two actions start the work.
    cx.update(|window, cx| window.click("empty-new-workspace", cx));
    settle(cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("atlas").cloned())
        .unwrap();
    assert!(form.read_with(cx, |f, _| f.is_open()), "the same modal");
    cx.update(|window, cx| window.click("new-workspace-modal-cancel", cx));
    settle(cx);

    cx.update(|window, cx| window.click("empty-new-session", cx));
    let composer = composer_of(&shell, "atlas", cx);
    assert_eq!(
        composer.read_with(cx, |c, _| c.choice().clone()),
        Choice::Main
    );
    cx.simulate_input("Fix the timeout");
    assert_eq!(
        composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string()),
        "Fix the timeout",
        "the caret is in the composer"
    );
}

#[gpui_kit::test]
fn a_project_with_a_workspace_does_not_show_the_empty_state(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the page shows login", |cx| {
        card_of(&shell, Some("login"), cx).is_some()
    });
    settle(cx);

    assert!(!page_of(&shell, cx).read_with(cx, |page, _| page.is_empty()));
    cx.update(|window, _| {
        assert!(window.try_find("project-empty").is_none());
        assert!(window.try_find("place-card-main").is_some());
        assert!(window.try_find("new-workspace-card").is_some());
    });
}

#[gpui_kit::test]
fn a_project_with_a_session_in_main_does_not_show_the_empty_state(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    start(&user, "atlas", None, "Release notes");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "main's card lists the session", |cx| {
        card_of(&shell, None, cx).is_some_and(|card| !titles_in(&card, cx).is_empty())
    });
    settle(cx);

    assert!(!page_of(&shell, cx).read_with(cx, |page, _| page.is_empty()));
    cx.update(|window, _| {
        assert!(window.try_find("project-empty").is_none());
        assert!(window.try_find("new-session-in-main").is_some());
    });
}

#[gpui_kit::test]
fn the_empty_state_gives_way_to_the_page_once_a_workspace_exists(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    let page = page_of(&shell, cx);
    until(&shell, cx, "the empty state is shown", |cx| {
        page.read_with(cx, |page, _| page.is_empty())
    });

    workspace(&user, "atlas", "login");
    until(&shell, cx, "the page shows login", |cx| {
        card_of(&shell, Some("login"), cx).is_some()
    });
    settle(cx);
    assert!(!page.read_with(cx, |page, _| page.is_empty()));
    cx.update(|window, _| {
        assert!(window.try_find("project-empty").is_none());
        assert!(window.try_find("new-workspace-card").is_some());
    });
}

#[gpui_kit::test]
fn agent_steps_are_folded_until_clicked_and_stay_open_while_the_session_is_shown(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, _mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let page = session_page(&shell, cx);
    settle(cx);

    let call = |seq: u64, id: &str, name: &str| {
        made_up(
            seq,
            "agent.tool_call",
            json!({"id": id, "title": name, "status": "completed"}),
        )
    };
    read_events(
        &page,
        vec![call(1, "a", "fs_read"), call(2, "b", "fs_list")],
        cx,
    );
    frame(cx);

    cx.update(|window, _| {
        assert_eq!(
            window.find("row-0-steps-toggle").label(),
            Some("2 steps · fs_list"),
            "how many, and the last one's name"
        );
        assert!(window.try_find("step-0-0").is_none(), "folded");
    });

    cx.update(|window, cx| window.click("row-0-steps-toggle", cx));
    frame(cx);
    cx.update(|window, _| {
        assert!(window.find("step-0-0").visible());
        assert!(window.find("step-0-1").visible());
    });

    // Opened, it stays so while the session goes on.
    live(&page, call(3, "c", "fs_edit"), cx);
    frame(cx);
    cx.update(|window, _| assert!(window.find("step-0-2").visible()));

    cx.update(|window, cx| window.click("row-0-steps-toggle", cx));
    frame(cx);
    cx.update(|window, _| {
        assert!(window.try_find("step-0-0").is_none());
        assert_eq!(
            window.find("row-0-steps-toggle").label(),
            Some("3 steps · fs_edit")
        );
    });
}

/// Types `text` in a rules editor, as the user would have: the field takes it and fires its change.
fn type_rules(
    editor: &Entity<flottille_window::rules::RulesEditor>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    let field = editor
        .read_with(cx, |editor, _| editor.field().cloned())
        .unwrap();
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(text, window, cx)));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_rules_page_edits_the_rules_of_every_project_saves_them_and_resets_them_to_the_default(
    cx: &mut TestAppContext,
) {
    use flottille_window::rules::Source;
    let user = user();
    project(&user, "atlas");
    let default = user.call("rules.show", json!({})).unwrap()["default"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    // Tall enough to show the whole page: a click reaches only what is on screen.
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let page = open_settings(&shell, cx);
    let editor = page.read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the default rules are shown", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);

    // The editor shows Flottille's default, says where a change goes, and has nothing to reset.
    assert_eq!(
        editor.read_with(cx, |e, cx| e.typed(cx)),
        Some(default.clone())
    );
    editor.read_with(cx, |e, _| {
        assert_eq!(e.shown().unwrap().source, Source::Default);
        assert!(!e.is_own());
    });
    cx.update(|window, _| {
        assert!(window.find("rules-text-").visible());
        assert!(window.find("rules-reach-").visible());
        assert!(flottille_window::rules::REACH.contains("reaches the agents created from now on"));
        assert!(window.find("rules-save-").visible());
        assert!(window.find("rules-reset-").visible());
    });

    type_rules(&editor, "Answer in French.", cx);
    assert!(editor.read_with(cx, |e, cx| e.is_edited(cx)));
    cx.update(|window, cx| window.click("rules-save-", cx));
    until(&shell, cx, "the rules are saved", |cx| {
        editor.read_with(cx, |e, _| e.is_own())
    });
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/rules.md")).unwrap(),
        "Answer in French."
    );
    assert!(!editor.read_with(cx, |e, cx| e.is_edited(cx)));

    // A new session gets them.
    let agent = start(&user, "atlas", None, "hello");
    let text = user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["instructions"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.contains("# How to work\n\nAnswer in French."),
        "{text}"
    );

    cx.update(|window, cx| window.click("rules-reset-", cx));
    until(&shell, cx, "the rules are reset", |cx| {
        editor.read_with(cx, |e, _| !e.is_own())
    });
    assert_eq!(editor.read_with(cx, |e, cx| e.typed(cx)), Some(default));
    assert!(!user.path("config/flottille/rules.md").exists());
}

#[gpui_kit::test]
fn a_project_s_settings_have_a_rules_section_that_replaces_the_global_rules_and_resets(
    cx: &mut TestAppContext,
) {
    use flottille_window::rules::Source;
    let user = user();
    project(&user, "atlas");
    user.call("rules.set", json!({"text": "Answer in French."}))
        .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let page = open_project_settings(&shell, "atlas", cx);
    let editor = page.read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the rules are shown", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);

    // Without rules of its own the project follows the global ones, and shows them.
    editor.read_with(cx, |e, _| {
        assert_eq!(e.project(), Some("atlas"));
        assert_eq!(e.shown().unwrap().source, Source::Global);
        assert!(!e.is_own());
    });
    assert_eq!(
        editor.read_with(cx, |e, cx| e.typed(cx)),
        Some("Answer in French.".to_owned())
    );
    cx.update(|window, _| {
        assert!(window.find("rules-text-atlas").visible());
        assert!(window.find("rules-reach-atlas").visible());
    });

    type_rules(&editor, "Answer in Spanish.", cx);
    cx.update(|window, cx| window.click("rules-save-atlas", cx));
    until(&shell, cx, "the project's rules are saved", |cx| {
        editor.read_with(cx, |e, _| e.is_own())
    });
    let file = std::fs::read_to_string(user.project_file("atlas")).unwrap();
    assert!(file.contains("Answer in Spanish."), "{file}");
    // The user's own are untouched.
    assert_eq!(
        user.call("rules.show", json!({})).unwrap()["text"],
        "Answer in French."
    );

    cx.update(|window, cx| window.click("rules-reset-atlas", cx));
    until(
        &shell,
        cx,
        "the project follows the global rules again",
        |cx| editor.read_with(cx, |e, _| !e.is_own()),
    );
    assert_eq!(
        editor.read_with(cx, |e, cx| e.typed(cx)),
        Some("Answer in French.".to_owned())
    );
    assert!(
        !std::fs::read_to_string(user.project_file("atlas"))
            .unwrap()
            .contains("Spanish")
    );
}

#[gpui_kit::test]
fn a_project_s_rules_can_be_saved_identical_to_the_global_text(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    user.call("rules.set", json!({"text": "Answer in French."}))
        .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let page = open_project_settings(&shell, "atlas", cx);
    let editor = page.read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the rules are shown", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);

    // What is shown is the user's text; what the project stores is nothing. Saving it unchanged
    // gives the project the same text as its own, so a later change of the global rules leaves it.
    assert!(editor.read_with(cx, |e, cx| e.savable(cx)));
    cx.update(|window, cx| window.click("rules-save-atlas", cx));
    until(&shell, cx, "the project's rules are saved", |cx| {
        editor.read_with(cx, |e, _| e.is_own())
    });
    let shown = user
        .call("rules.show", json!({"project": "atlas"}))
        .unwrap();
    assert_eq!(shown["source"], "project", "{shown}");
    assert_eq!(shown["text"], "Answer in French.");
    // Its own now, unchanged: nothing more to save.
    assert!(!editor.read_with(cx, |e, cx| e.savable(cx)));
}

#[gpui_kit::test]
fn the_rules_editor_says_when_rules_edited_by_hand_were_cut(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/rules.md"),
        format!("{}TAIL", "x".repeat(8_000)),
    )
    .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let page = open_settings(&shell, cx);
    let editor = page.read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the rules are shown", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);

    let cut = editor.read_with(cx, |e, _| e.shown().unwrap().cut.clone());
    let cut = cut.expect("the cut is said");
    assert!(
        cut.contains("8004 characters") && cut.contains("8000"),
        "{cut}"
    );
    cx.update(|window, _| assert!(window.find("rules-cut-").visible()));
    // The editor holds what agents get; saving it makes the file fit.
    assert!(editor.read_with(cx, |e, cx| e.savable(cx)));
    cx.update(|window, cx| window.click("rules-save-", cx));
    until(&shell, cx, "the cut rules are saved", |cx| {
        editor.read_with(cx, |e, _| e.shown().unwrap().cut.is_none())
    });
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/rules.md")).unwrap(),
        "x".repeat(8_000)
    );
}
