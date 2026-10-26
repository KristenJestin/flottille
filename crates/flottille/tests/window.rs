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
use flottille_window::composer::{Choice, Composer, PlaceChoice, Tab};
use flottille_window::conversation::Block;
use flottille_window::link::{self, Connect};
use flottille_window::model::{Place, Preparation};
use flottille_window::project::{NewWorkspace, PlaceCard, ProjectPage};
use flottille_window::session::SessionPage;
use flottille_window::settings::SettingsPage;
use flottille_window::shell::{Layout, Selection};
use flottille_window::tree::{place_id, project_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
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

    // Fold the place, then the project.
    cx.update(|window, cx| window.click(place_id("atlas", &None), cx));
    cx.update(|window, cx| window.click(project_id("atlas"), cx));
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
    // Atlas's page is open, its details read.
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
    assert!(stopped.status.success());
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

/// A recipe script that waits for `gate` to exist, then runs `then`: the test says when the
/// preparation ends.
fn gated(then: &str, gate: &std::path::Path) -> String {
    format!(
        "while [ ! -e '{}' ]; do sleep 0.05; done\n{then}\n",
        gate.display()
    )
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
        "from main · atlas"
    );
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
fn starting_a_task_in_a_new_workspace_prepares_it_then_starts_the_session_in_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let gate = user.path("gate");
    recipe(&user, "atlas", &gated("echo prepared", &gate));
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(cx);

    type_task("Fix the login\nwith a second line", cx);
    cx.update(|window, cx| window.click("place-picker", cx));
    cx.update(|window, cx| window.click("place-choice-new", cx));
    cx.simulate_input("login-fix");
    cx.simulate_keystrokes("enter");
    let composer = composer_of(&shell, "atlas", cx);
    assert!(composer.read_with(cx, |c, _| c.is_sending()));

    until(
        &shell,
        cx,
        "the workspace's card shows its preparation",
        |cx| {
            card_of(&shell, Some("login-fix"), cx).is_some_and(|card| {
                matches!(
                    card.read_with(cx, |c, _| c.data().preparation.clone()),
                    Preparation::Preparing { .. }
                )
            })
        },
    );
    assert!(
        composer.read_with(cx, |c, _| c.is_sending()),
        "it waits for the preparation"
    );
    assert_eq!(
        user.call("agent.list", json!({})).unwrap()["agents"],
        json!([]),
        "nothing starts before the workspace is ready"
    );
    std::fs::write(&gate, "").unwrap();
    until(&shell, cx, "the session's page opens", |cx| {
        matches!(
            shell.read_with(cx, |s, _| s.selected().cloned()),
            Some(Selection::Session(_))
        )
    });
    let Some(Selection::Session(id)) = shell.read_with(cx, |s, _| s.selected().cloned()) else {
        unreachable!()
    };
    let agent = user.call("agent.status", json!({"agent": id})).unwrap();
    assert_eq!(agent["agent"]["workspace"], "login-fix", "it started in it");
    let workspace = user
        .call(
            "workspace.show",
            json!({"project": "atlas", "name": "login-fix"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "prepared before it started");
    let first = user.call("agent.show", json!({"agent": id})).unwrap();
    assert_eq!(first["agent"]["brief"], "Fix the login\nwith a second line");
    assert!(
        !composer.read_with(cx, |c, cx| c.is_sending()
            || !c.field().read(cx).value().is_empty()),
        "the field is emptied for the next task"
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

    type_task("Try the broken recipe", cx);
    cx.update(|window, cx| window.click("place-picker", cx));
    cx.update(|window, cx| window.click("place-choice-new", cx));
    cx.simulate_input("broken");
    cx.update(|window, cx| window.click("send", cx));

    let composer = composer_of(&shell, "atlas", cx);
    until(&shell, cx, "the composer says no session started", |cx| {
        composer.read_with(cx, |c, _| c.error().is_some())
    });
    let error = composer.read_with(cx, |c, _| c.error().unwrap().to_owned());
    assert!(error.contains("no session started"), "{error}");
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
fn the_running_agents_tab_lists_a_waiting_session_first(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let waiting = start(&user, "atlas", None, "Sessions expire too early");
    send(&user, &waiting, "read the env");
    let at_rest = start(&user, "atlas", None, "Tidy the aliases");
    let running = start(&user, "atlas", None, "CSV export by tenant");
    send(&user, &running, "work long");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "one waits and one runs", |cx| {
        mark_of(&shell, &waiting, cx) == Some(AgentState::NeedsYou)
            && mark_of(&shell, &running, cx) == Some(AgentState::Running)
    });
    settle(cx);

    cx.update(|window, cx| window.click("tab-running", cx));
    let composer = composer_of(&shell, "atlas", cx);
    assert_eq!(composer.read_with(cx, |c, _| c.tab()), Tab::Running);
    let listed: Vec<String> = composer.read_with(cx, |c, cx| {
        c.running(cx).iter().map(|s| s.id.clone()).collect()
    });
    assert_eq!(
        listed,
        [waiting.clone(), running.clone()],
        "the waiting session first, though it is the older one; the one at rest not at all"
    );
    settle(cx);
    cx.update(|window, _| {
        let y = |id: &str| window.find(format!("running-{id}")).bounds().origin.y;
        assert!(y(&waiting) < y(&running));
        assert!(window.try_find(format!("running-{at_rest}")).is_none());
        assert!(
            window.try_find("composer-field").is_none(),
            "the list replaces the field"
        );
    });
    cx.update(|window, cx| window.click(format!("running-{waiting}"), cx));
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(waiting))
    );
}

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

    let field = composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string());
    assert_eq!(
        field, "Fix the timeout\nand its test",
        "Shift+Enter makes a new line"
    );
    assert!(
        !composer.read_with(cx, |c, _| c.is_sending()),
        "nothing was sent"
    );
    let (page_after, cards_after, composer_after) = renders(cx);
    assert_eq!(page_after, page_before, "the page is not drawn again");
    assert_eq!(cards_after, cards_before, "nor its cards");
    assert_eq!(
        composer_after,
        composer_before + 1,
        "the composer is drawn again once, when its send button wakes up, not once a letter"
    );
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

/// Sends `task` from atlas's composer into a new workspace named `name`.
fn send_in_new(task: &str, name: &str, cx: &mut VisualTestContext) {
    type_task(task, cx);
    cx.update(|window, cx| window.click("place-picker", cx));
    cx.update(|window, cx| window.click("place-choice-new", cx));
    cx.simulate_input(name);
    cx.simulate_keystrokes("enter");
}

#[gpui_kit::test]
fn a_session_started_after_switching_project_stays_with_its_project(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    project(&user, "borealis");
    let gate = user.path("gate");
    recipe(&user, "atlas", &gated("echo prepared", &gate));
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    send_in_new("Fix the login", "login-fix", cx);
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
    std::fs::write(&gate, "").unwrap();
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
    assert_eq!(agents["agents"][0]["workspace"], "login-fix");
    assert_eq!(field_of(&atlas, cx), "", "atlas's field is emptied");
    assert_eq!(
        atlas.read_with(cx, |c, cx| (
            c.name_field().read(cx).value().to_string(),
            c.choice().clone()
        )),
        (String::new(), Choice::Main),
        "and the new workspace's name with it"
    );
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
    let gate = user.path("gate");
    recipe(
        &user,
        "atlas",
        &gated("echo 'made-up failure'\nexit 3", &gate),
    );
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "atlas", cx);
    send_in_new("Try the broken recipe", "broken", cx);
    let atlas = composer_of(&shell, "atlas", cx);

    open_project(&shell, "borealis", cx);
    let borealis = composer_of(&shell, "borealis", cx);
    type_task("A draft for borealis", cx);
    std::fs::write(&gate, "").unwrap();
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
        "Try the broken recipe",
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

/// A user whose fake agent keeps a worker mid-turn when its brief says `HANG`, and answers "ok"
/// otherwise.
fn team_user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "HANG", "do": [{"hang": true}]}]}));
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
    let answer = lead.call(
        "agent_launch",
        json!({"title": title, "brief": format!("HANG and {title}"), "workspace": "w"}),
    );
    assert!(!answer.error, "{}", answer.text);
    let id = answer
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_owned();
    let task = format!("Your task: {title}.");
    (id, mcp_session(user, |prompt| prompt.contains(&task)))
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
            assert_eq!(from, "lead → build the form");
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
                && cx.update(|window, _| window.try_find(format!("allow-once-{number}")).is_some())
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
        why.contains("lead forwards it: the form needs the database name"),
        "{why}"
    );
    assert_eq!(
        card.source.as_deref(),
        Some(format!("asked by read the settings · forwarded by lead · #{number}").as_str())
    );
}

#[gpui_kit::test]
fn approving_once_from_the_card_performs_the_call_and_leaves_the_record(cx: &mut TestAppContext) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the card is shown", |cx| {
        cx.update(|window, _| window.try_find(format!("allow-once-{number}")).is_some())
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("allow-once-{number}"), cx));
    until(&shell, cx, "the card folds into its record", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("approval-record-{number}"))
                .is_some()
                && window.try_find(format!("allow-once-{number}")).is_none()
        })
    });
    let record = session_page(&shell, cx).read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .rows()
            .iter()
            .find_map(|row| row.read(cx).card().and_then(|c| c.record()))
    });
    assert_eq!(record.as_deref(), Some("Allowed once · Read .env · lead"));
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
fn the_editor_button_opens_the_place_s_folder_with_the_system_handler(cx: &mut TestAppContext) {
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

    cx.update(|window, cx| window.click("open-in-editor", cx));

    assert_eq!(*given.lock().unwrap(), [folder]);
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
        cx.update(|window, _| window.try_find(format!("allow-once-{number}")).is_some())
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
            "Allowed once · Read .env · lead"
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
        cx.update(|window, _| window.try_find(format!("allow-once-{number}")).is_some())
    });
    settle(cx);

    // Answered, then another session opened before the daemon's answer is taken.
    cx.update(|window, cx| window.click(format!("allow-once-{number}"), cx));
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

#[gpui_kit::test]
fn the_card_offers_allow_for_this_tree_only_when_the_place_is_not_sensitive(
    cx: &mut TestAppContext,
) {
    let user = team_user();
    let (lead, mcp, _) = lead_in_w(&user);
    let (shell, cx) = open_session(&user, &lead, 1280., cx);
    let sensitive = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    let outside = user.path("elsewhere/notes.txt");
    let plain = held_number(mcp.call(
        "fs_write",
        json!({"path": outside, "content": "made-up notes"}),
    ));
    until(&shell, cx, "both cards are shown", |cx| {
        cx.update(|window, _| {
            window.try_find(format!("allow-once-{sensitive}")).is_some()
                && window.try_find(format!("allow-once-{plain}")).is_some()
        })
    });

    cx.update(|window, _| {
        assert!(
            window.try_find(format!("allow-tree-{sensitive}")).is_none(),
            "never for a sensitive place"
        );
        assert!(window.try_find(format!("allow-tree-{plain}")).is_some());
    });
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
}

/// Connects to the user's daemon through a relay that holds back the answer to the `n`th
/// `provider.list` asked (from 0, over every connection) for `hold(n)`. Also how many were asked,
/// and how many held answers were let through.
fn holding_provider_lists(
    user: &User,
    hold: impl Fn(usize) -> Option<Duration> + Send + Sync + 'static,
) -> (Connect, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    let connect = user.connector();
    let asked = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicUsize::new(0));
    let hold = Arc::new(hold);
    let (counted, let_through) = (asked.clone(), released.clone());
    let relay: Connect = Arc::new(move || {
        let daemon = connect()?;
        let (window_side, relay_side) = UnixStream::pair()?;
        let held: Arc<Mutex<HashMap<String, Duration>>> = Arc::default();
        let (mut to_daemon, from_window) = (daemon.try_clone()?, relay_side.try_clone()?);
        let (asked, hold, marks) = (counted.clone(), hold.clone(), held.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(from_window).lines() {
                let Ok(line) = line else { return };
                let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                if request["method"] == "provider.list" {
                    let n = asked.fetch_add(1, Ordering::SeqCst);
                    if let Some(delay) = hold(n) {
                        marks
                            .lock()
                            .unwrap()
                            .insert(request["id"].to_string(), delay);
                    }
                }
                if writeln!(to_daemon, "{line}").is_err() {
                    return;
                }
            }
        });
        let (mut to_window, released) = (relay_side, let_through.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(daemon).lines() {
                let Ok(line) = line else { return };
                let answer: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let delay = held.lock().unwrap().remove(&answer["id"].to_string());
                if let Some(delay) = delay {
                    std::thread::sleep(delay);
                }
                if writeln!(to_window, "{line}").is_err() {
                    return;
                }
                if delay.is_some() {
                    released.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        Ok(window_side)
    });
    (relay, asked, released)
}

#[gpui_kit::test]
fn a_slow_provider_list_answer_that_arrives_after_a_newer_one_changes_nothing(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    // The link's first read is held: what it answers is the file as it was then.
    let (connect, asked, released) =
        holding_provider_lists(&user, |n| (n == 0).then_some(Duration::from_secs(3)));
    let (shell, cx) = window(&user, connect, 1280., cx);
    until(&shell, cx, "the link asked for the providers", |_| {
        asked.load(Ordering::SeqCst) >= 1
    });
    user.write_providers("[opencode]\nenabled = false\n");
    let page = open_settings(&shell, cx);
    until(&shell, cx, "the newer read shows opencode disabled", |cx| {
        said_of(&page, "opencode", cx)
            .iter()
            .any(|line| line.starts_with("opencode is not enabled"))
    });

    until(&shell, cx, "the held answer arrived", |_| {
        released.load(Ordering::SeqCst) == 1
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
fn the_providers_read_after_a_session_opened_does_not_hold_the_link_s_events(
    cx: &mut TestAppContext,
) {
    let user = providers_user();
    project(&user, "atlas");
    let (connect, asked, _) = holding_provider_lists(&user, |_| Some(Duration::from_secs(15)));
    let (shell, cx) = window(&user, connect, 1280., cx);
    until(&shell, cx, "the project is shown", |cx| {
        shell.read_with(cx, |s, _| s.project("atlas").is_some())
    });
    until(&shell, cx, "the link asked for the providers", |_| {
        asked.load(Ordering::SeqCst) >= 1
    });
    let first = start(&user, "atlas", None, "hello");
    until(
        &shell,
        cx,
        "the link read the providers after it started",
        |_| asked.load(Ordering::SeqCst) >= 2,
    );

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
