//! The scenarios of #136: one spinner where the work is, nothing left spinning once an agent
//! ended, "Ended" at the bottom of the sidebar, a session ended or resumed and a session started
//! from a row, Stop in Send's place, the user's own messages selectable. On GPUI's test platform
//! in front of a real daemon whose agents are the fake ACP agent, with a throwaway user. No window
//! opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_ui::state::icon_id;
use flottille_ui::toggle_id;
use flottille_window::Shell;
use flottille_window::composer::Choice;
use flottille_window::link::Connect;
use flottille_window::session::{Reply, SessionPage};
use flottille_window::shell::Selection;
use flottille_window::tree::{
    ended_id, place_id, place_new_id, project_id, session_end_id, session_id, session_menu_id,
    session_resume_id,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A user whose fake agent works while it is told to.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "work briefly", "do": [{"wait": 1500}, {"say": "done"}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
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
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Starts a session and waits until its first turn ended.
fn start(user: &User, project: &str, message: &str) -> String {
    let answer = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
}

fn send(user: &User, agent: &str, message: &str) {
    user.call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

fn cancel(user: &User, agent: &str) {
    user.call("agent.cancel", json!({"agent": agent})).unwrap();
    eventually("the turn ends", || state_of(user, agent) == "idle");
}

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
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// A frame of the clock and a few of drawing: what a window that is looked at does while a
/// block folds away.
fn moving(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    frames(cx);
}

/// A few frames, as a window draws them after a change.
fn frames(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
}

fn open_session<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, Entity<Reply>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    // Its row unfolds into the tree: it is there to be clicked once it has.
    settle(cx);
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(
        &shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            page_of(&shell, cx).read_with(cx, |page, cx| {
                !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
            })
        },
    );
    settle(cx);
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(lead).cloned())
        .expect("the session has a composer");
    (shell, reply, cx)
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn has(id: &'static str, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| window.try_find(id).is_some())
}

#[gpui_kit::test]
fn stop_takes_send_s_place_while_the_agent_works_and_the_swap_moves(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let lead = start(&user, "app", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    assert!(
        has("reply-send", cx) && !has("reply-stop", cx),
        "idle: Send alone"
    );

    send(&user, &lead, "HANG");
    until(&shell, cx, "the turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });
    // The swap is a cross-fade: for a few frames both are drawn.
    let mut crossed = false;
    for _ in 0..12 {
        moving(cx);
        crossed |= has("reply-send", cx) && has("reply-stop", cx);
    }
    assert!(crossed, "Send fades out while Stop fades in");
    settle(cx);
    assert!(has("reply-stop", cx), "the turn runs: Stop");
    assert!(
        !has("reply-send", cx),
        "Stop replaces Send, it is not beside it"
    );

    // Something to send brings Send back, and takes Stop away.
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("one more thing");
    settle(cx);
    assert!(
        has("reply-send", cx) && !has("reply-stop", cx),
        "text typed: Send"
    );

    // Stop interrupts the turn, and Send comes back for good.
    for _ in 0..14 {
        cx.simulate_keystrokes("backspace");
    }
    settle(cx);
    assert!(has("reply-stop", cx) && !has("reply-send", cx));
    cx.update(|window, cx| window.click("reply-stop", cx));
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    settle(cx);
    assert!(
        has("reply-send", cx) && !has("reply-stop", cx),
        "the turn ended: Send"
    );
    assert_eq!(state_of(&user, &lead), "idle", "the agent stays");
}

/// The text selecting everything in the row `at` of the conversation gives.
fn selecting_all_in(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) -> String {
    let row = page.read_with(cx, |p, cx| p.conversation().read(cx).rows()[at].clone());
    row.update(cx, |row, cx| row.select_all(cx));
    frames(cx);
    row.read_with(cx, |row, cx| row.selected_text(cx).trim().to_owned())
}

#[gpui_kit::test]
fn the_user_s_own_message_is_selectable_like_the_agent_s(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let lead = start(&user, "app", "keep *this* as_typed [here]");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    settle(cx);
    let page = page_of(&shell, cx);
    let (mine, theirs) = page.read_with(cx, |p, cx| {
        let rows = p.conversation().read(cx).rows().to_vec();
        let drawn = |row: &Entity<flottille_window::session::Row>| {
            row.read(cx).rendered(cx).map(|text| text.trim().to_owned())
        };
        (
            rows.iter()
                .position(|row| drawn(row).as_deref() == Some("keep *this* as_typed [here]")),
            rows.iter()
                .position(|row| drawn(row).as_deref() == Some("ok")),
        )
    });
    let (mine, theirs) = (
        mine.expect("the user's message, as typed"),
        theirs.expect("the reply"),
    );

    assert_eq!(
        selecting_all_in(&page, theirs, cx),
        "ok",
        "the agent's reply"
    );
    assert_eq!(
        selecting_all_in(&page, mine, cx),
        "keep *this* as_typed [here]",
        "the user's own message, marks and all"
    );
}

/// A workspace of `project`, ready.
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

/// The width of the left tree: what is drawn to its right belongs to the page.
const TREE: f32 = 300.;

/// The folds of the left tree: what lies in one that is shut is clipped away, though drawn.
const FOLDS: [&str; 3] = ["fold-project-app", "fold-place-app-main", "fold-ended"];

/// Whether the left tree shows what is drawn at `bounds`: left of the page, and in no fold that
/// is shut.
fn shown_in_tree(
    bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    window: &mut gpui_kit::Window,
) -> bool {
    bounds.origin.x < px(TREE)
        && !FOLDS.iter().any(|fold| {
            window.try_find(*fold).is_some_and(|fold| {
                fold.bounds().size.height == px(0.) && bounds.origin.y >= fold.bounds().origin.y
            })
        })
}

/// The spinners the left tree shows; the project page draws its own beside it.
fn running_icons(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, _| {
        let icons = window.find_all(icon_id(AgentState::Running));
        icons
            .iter()
            .filter(|icon| shown_in_tree(icon.bounds(), window))
            .count()
    })
}

/// Whether the left tree shows `id`.
fn in_tree(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| {
        window
            .try_find(id)
            .is_some_and(|row| shown_in_tree(row.bounds(), window))
    })
}

fn mark_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<AgentState> {
    shell.read_with(cx, |shell, cx| {
        shell.session(id).and_then(|row| row.read(cx).mark())
    })
}

/// A window on the tree of the user's daemon, its rows drawn.
fn tree_of<'a>(
    user: &User,
    shown: &[&str],
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the sessions are in the tree", |cx| {
        cx.update(|window, _| {
            shown
                .iter()
                .all(|id| window.try_find(session_id(id)).is_some())
        })
    });
    settle(cx);
    (shell, cx)
}

#[gpui_kit::test]
fn a_running_agent_shows_one_spinner_on_its_own_row_and_a_folded_parent_shows_one_for_what_runs_inside(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "hello");
    send(&user, &id, "HANG");
    let (shell, cx) = tree_of(&user, &[&id], cx);
    until(&shell, cx, "the session runs", |cx| {
        mark_of(&shell, &id, cx) == Some(AgentState::Running)
    });
    settle(cx);
    let (project, main) = shell.read_with(cx, |s, _| {
        (
            s.project("app").unwrap().clone(),
            s.place("app", &None).unwrap().clone(),
        )
    });
    assert_eq!(
        running_icons(cx),
        1,
        "its row, and neither its place nor its project"
    );
    assert_eq!(project.read_with(cx, |p, _| p.mark()), None);
    assert_eq!(main.read_with(cx, |p, _| p.mark()), None);

    // The place folded: it shows one for what runs inside, and the row is out of sight.
    cx.update(|window, cx| window.click(place_id("app", &None), cx));
    settle(cx);
    assert_eq!(
        main.read_with(cx, |p, _| p.mark()),
        Some(AgentState::Running)
    );
    assert_eq!(project.read_with(cx, |p, _| p.mark()), None);
    assert_eq!(running_icons(cx), 1, "the place alone");

    // The project folded too: it shows the one, the place is out of sight.
    cx.update(|window, cx| window.click(toggle_id(project_id("app")), cx));
    settle(cx);
    assert_eq!(
        project.read_with(cx, |p, _| p.mark()),
        Some(AgentState::Running)
    );
    assert_eq!(running_icons(cx), 1, "the project alone");

    // Unfolded again, the mark goes back to the row: it goes with a fade, not at once.
    cx.update(|window, cx| window.click(toggle_id(project_id("app")), cx));
    settle(cx);
    cx.update(|window, cx| window.click(place_id("app", &None), cx));
    moving(cx);
    assert!(
        running_icons(cx) >= 1,
        "the marks are still drawn while they fold away"
    );
    settle(cx);
    assert_eq!(running_icons(cx), 1, "its row, again alone");
    assert_eq!(project.read_with(cx, |p, _| p.mark()), None);
    assert_eq!(main.read_with(cx, |p, _| p.mark()), None);
}

#[gpui_kit::test]
fn a_session_stops_spinning_as_soon_as_its_agent_ended(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "hello");
    send(&user, &id, "HANG");
    let (shell, cx) = tree_of(&user, &[&id], cx);
    until(&shell, cx, "the session runs", |cx| {
        mark_of(&shell, &id, cx) == Some(AgentState::Running)
    });
    user.call("agent.stop", json!({"agent": id})).unwrap();
    until(&shell, cx, "the row stops spinning", |cx| {
        mark_of(&shell, &id, cx).is_none()
    });
    settle(cx);
    assert_eq!(running_icons(cx), 0, "nothing spins anywhere in the window");
}

/// The sessions the "Ended" group lists, and whether it is open.
fn ended_group(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (Vec<String>, bool) {
    shell.read_with(cx, |s, cx| {
        let Some(node) = s.ended() else {
            return (Vec::new(), false);
        };
        let node = node.read(cx);
        let ids = node
            .sessions(cx)
            .iter()
            .map(|row| row.read(cx).id().to_owned())
            .collect();
        (ids, node.is_open())
    })
}

#[gpui_kit::test]
fn ended_is_one_folded_group_at_the_bottom_of_the_sidebar_below_every_project(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    project(&user, "notes");
    let old_app = start(&user, "app", "old in app");
    let old_notes = start(&user, "notes", "old in notes");
    let live_app = start(&user, "app", "alive in app");
    let live_notes = start(&user, "notes", "alive in notes");
    for id in [&old_app, &old_notes] {
        user.call("agent.stop", json!({"agent": id})).unwrap();
    }
    let (shell, cx) = tree_of(&user, &[&live_app, &live_notes], cx);
    until(
        &shell,
        cx,
        "both ended sessions are in the Ended group",
        |cx| ended_group(&shell, cx).0.len() == 2,
    );
    settle(cx);

    let (ids, open) = ended_group(&shell, cx);
    assert!(!open, "folded by default");
    assert!(
        ids.contains(&old_app) && ids.contains(&old_notes),
        "the sessions of every project"
    );
    cx.update(|window, _| {
        let y = |id| window.find(id).bounds().origin.y;
        assert!(
            y(ended_id()) > y(session_id(&live_notes)),
            "below the last project's sessions, not under a project's"
        );
        assert!(y(ended_id()) > y(project_id("notes")));
    });
    assert!(
        !in_tree(session_id(&old_app), cx),
        "its sessions are folded away"
    );

    // It opens on a click, and the sessions of every project are there.
    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    assert!(in_tree(session_id(&old_app), cx));
    assert!(in_tree(session_id(&old_notes), cx));
}

#[gpui_kit::test]
fn a_session_row_ends_its_session_from_its_menu_without_opening_it(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "hello");
    let (shell, cx) = tree_of(&user, &[&id], cx);

    // The menu is there while the pointer is on the row, not at rest.
    assert!(!has_id(session_menu_id(&id), cx), "no menu at rest");
    cx.update(|window, cx| window.hover(session_id(&id), cx));
    settle(cx);
    assert!(
        has_id(session_menu_id(&id), cx),
        "the row offers its menu under the pointer"
    );
    cx.update(|window, cx| window.click(session_menu_id(&id), cx));
    settle(cx);
    assert!(
        has_id(session_resume_id(&id), cx) == false,
        "a running session has no Resume"
    );
    cx.update(|window, cx| window.click(session_end_id(&id), cx));
    settle(cx);
    let selected = shell.read_with(cx, |s, _| s.selected().cloned());
    assert!(
        !matches!(selected, Some(Selection::Session(_))),
        "no session opened"
    );
    cx.update(|window, _| assert!(window.find("sidebar-end-modal").visible()));

    // Cancel ends nothing.
    cx.update(|window, cx| window.click("sidebar-end-modal-cancel", cx));
    settle(cx);
    assert_eq!(state_of(&user, &id), "idle");

    // Confirmed, it ends and moves to Ended.
    cx.update(|window, cx| window.hover(session_id(&id), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_menu_id(&id), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_end_id(&id), cx));
    settle(cx);
    cx.update(|window, cx| window.click("sidebar-end-confirm", cx));
    until(&shell, cx, "the session is in Ended", |cx| {
        ended_group(&shell, cx).0 == [id.clone()]
    });
    assert_eq!(state_of(&user, &id), "ended");
    assert!(
        !matches!(
            shell.read_with(cx, |s, _| s.selected().cloned()),
            Some(Selection::Session(_))
        ),
        "no session opened"
    );
}

#[gpui_kit::test]
fn an_ended_session_row_resumes_from_its_menu_when_its_agent_can(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "hello");
    user.call("agent.stop", json!({"agent": id})).unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the Ended group", |cx| {
        cx.update(|window, _| window.try_find(ended_id()).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    until(&shell, cx, "the process is gone", |_| {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == id.as_str() && a["stopping"] == false)
    });
    cx.update(|window, cx| window.hover(session_id(&id), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_menu_id(&id), cx));
    settle(cx);
    assert!(
        !has_id(session_end_id(&id), cx),
        "an ended session has no End"
    );
    cx.update(|window, cx| window.click(session_resume_id(&id), cx));
    until(&shell, cx, "the session is back in its place", |cx| {
        listed_in_main(&shell, cx) == [id.clone()]
    });
    assert!(
        !matches!(
            shell.read_with(cx, |s, _| s.selected().cloned()),
            Some(Selection::Session(_))
        ),
        "no session opened"
    );
}

fn has_id(id: gpui_kit::ElementId, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| window.try_find(id).is_some())
}

fn listed_in_main(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.place("app", &None)
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

#[gpui_kit::test]
fn a_plus_on_a_place_row_starts_a_session_there_with_the_field_focused(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    workspace(&user, "app", "login");
    let id = start(&user, "app", "hello");
    let (shell, cx) = tree_of(&user, &[&id], cx);
    cx.update(|window, _| {
        assert!(
            window
                .try_find(place_new_id("app", &Some("login".into())))
                .is_none(),
            "no + at rest"
        )
    });

    // On a workspace.
    cx.update(|window, cx| window.hover(place_id("app", &Some("login".into())), cx));
    settle(cx);
    cx.update(|window, cx| window.click(place_new_id("app", &Some("login".into())), cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("app".into()))
    );
    let composer = shell
        .read_with(cx, |s, _| s.composer_of("app").cloned())
        .unwrap();
    assert_eq!(
        composer.read_with(cx, |c, _| c.choice().clone()),
        Choice::Workspace("login".into())
    );
    cx.simulate_input("fix the login");
    assert_eq!(
        composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string()),
        "fix the login",
        "the field has the focus"
    );

    // On `main`.
    cx.update(|window, cx| window.hover(place_id("app", &None), cx));
    settle(cx);
    cx.update(|window, cx| window.click(place_new_id("app", &None), cx));
    settle(cx);
    assert_eq!(
        composer.read_with(cx, |c, _| c.choice().clone()),
        Choice::Main
    );
}
