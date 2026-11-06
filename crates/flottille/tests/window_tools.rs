//! The scenario of #290 in the window: a session whose client ignored that its tools changed is
//! said, above the composer, to run an older list, and the line folds away once the client lists
//! them again. On GPUI's test platform in front of a real daemon whose agent is the fake ACP agent,
//! with a throwaway user and rmcp's client in place of the agent's own. No window opens on a
//! screen.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session::{
    OLDER_TOOLS, RELOAD_TOOLS, RELOADED, RELOADS_AFTER_TURN, SessionPage,
};
use flottille_window::tree::{project_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn shown(user: &User, agent: &str) -> serde_json::Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

/// The project `app` over a throwaway repository whose `origin` is GitHub's `acme/app`, with no
/// forge and no tracker declared.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(800.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
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

/// Lets every animation end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn at_rest(cx: &mut VisualTestContext) {
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

/// One frame, 16 ms on: how many elements it drew that move.
fn step(cx: &mut VisualTestContext) -> usize {
    cx.executor().advance_clock(Duration::from_millis(16));
    let drawn = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    drawn
}

fn label(id: &'static str, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

/// Whether the session's page knows its root runs an older list of tools.
fn outdated(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    session_page(shell, cx).read_with(cx, |page, _| page.tree().iter().any(|a| a.tools_outdated))
}

#[gpui_kit::test]
fn a_session_on_an_older_list_of_tools_says_so_above_its_composer_until_it_lists_them(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    project(&user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(&user, 0);
    eventually("the first turn ends", || {
        shown(&user, &id)["state"] == "idle"
    });
    // rmcp's client, in place of the agent's own: it does nothing with a notice.
    let mcp = Mcp::initialize(&url, &authorization);
    mcp.names();

    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
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
    at_rest(cx);
    assert_eq!(label("session-older-tools", cx), None);

    // The project gains a tracker; told with its next answer, the client calls again on its
    // older list.
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    mcp.call("fs_list", json!({"path": "."}));
    mcp.call("fs_list", json!({"path": "."}));
    until(&shell, cx, "the page knows", |cx| outdated(&shell, cx));
    // It unfolds: it moves, then it is there at rest.
    assert!(step(cx) + step(cx) > 0, "the line unfolds");
    at_rest(cx);
    assert_eq!(
        label("session-older-tools", cx).as_deref(),
        Some(OLDER_TOOLS)
    );

    // Once the client lists them, the line folds away, still drawn while it does.
    assert!(mcp.names().contains(&"tickets".to_owned()));
    until(&shell, cx, "the page knows it is current", |cx| {
        !outdated(&shell, cx)
    });
    step(cx);
    assert!(step(cx) > 0, "the line folds away");
    assert_eq!(
        label("session-older-tools", cx).as_deref(),
        Some(OLDER_TOOLS),
        "drawn while it folds"
    );
    at_rest(cx);
    assert_eq!(label("session-older-tools", cx), None);
}

/// A root whose first turn ended, its session's page open in the window: its id.
fn open_idle_session(
    user: &User,
    shell: &Entity<Shell>,
    message: &str,
    cx: &mut VisualTestContext,
) -> String {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    until(shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(shell, cx, "the session's page shows its tree", |cx| {
        session_page(shell, cx).read_with(cx, |page, _| !page.tree().is_empty())
    });
    id
}

/// The events of the agent named `name`, from the store.
fn named(user: &User, agent: &str, name: &str) -> Vec<serde_json::Value> {
    user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == name)
        .cloned()
        .collect()
}

fn reloads(user: &User, agent: &str) -> usize {
    named(user, agent, "agent.released")
        .iter()
        .filter(|e| e["data"]["reason"] == "reload")
        .count()
}

#[gpui_kit::test]
fn the_outdated_note_offers_reload_tools(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    project(&user);
    let (shell, cx) = window(&user, cx);
    let id = open_idle_session(&user, &shell, "lead the work", cx);
    eventually("the first turn ends", || {
        shown(&user, &id)["state"] == "idle"
    });
    let (url, authorization) = server_of(&user, 0);
    // rmcp's client, in place of the agent's own: it does nothing with a notice.
    let mcp = Mcp::initialize(&url, &authorization);
    mcp.names();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    mcp.call("fs_list", json!({"path": "."}));
    mcp.call("fs_list", json!({"path": "."}));
    until(&shell, cx, "the page knows", |cx| outdated(&shell, cx));
    at_rest(cx);
    assert_eq!(
        label("session-older-tools", cx).as_deref(),
        Some(OLDER_TOOLS)
    );
    assert_eq!(
        label("session-reload-tools", cx).as_deref(),
        Some(RELOAD_TOOLS),
        "the note offers the action"
    );

    cx.update(|window, cx| window.click("session-reload-tools", cx));

    until(&shell, cx, "the process was let go", |_| {
        reloads(&user, &id) == 1
    });
    // Its token revoked, it no longer runs an older list: the note folds away, drawn while it
    // does.
    until(&shell, cx, "the page knows it is current", |cx| {
        !outdated(&shell, cx)
    });
    step(cx);
    assert!(step(cx) > 0, "the note folds away");
    assert_eq!(
        label("session-older-tools", cx).as_deref(),
        Some(OLDER_TOOLS),
        "drawn while it folds"
    );
    at_rest(cx);
    assert_eq!(label("session-older-tools", cx), None);
    assert_eq!(label("session-reload-tools", cx), None);
}

#[gpui_kit::test]
fn slash_reload_tools_during_a_turn_says_it_reloads_after_this_turn(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work", "do": [{"hold": "release"}, {"say": "Done."}]}
    ]}));
    project(&user);
    let (shell, cx) = window(&user, cx);
    let id = open_idle_session(&user, &shell, "work", cx);
    eventually("the turn runs", || shown(&user, &id)["state"] == "working");
    until(&shell, cx, "the page knows it works", |cx| {
        session_page(&shell, cx).read_with(cx, |page, _| {
            page.tree().iter().any(|a| a.state == "working")
        })
    });
    settle(cx);

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("/reload");
    until(&shell, cx, "the list offers it", |cx| {
        cx.update(|window, _| window.try_find("slash-reload-tools").is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click("slash-reload-tools", cx));

    until(&shell, cx, "the page says it waits", |cx| {
        session_page(&shell, cx).read_with(cx, |page, _| page.reloads_after_turn())
    });
    let page = session_page(&shell, cx);
    let reply = page.read_with(cx, |page, _| page.reply().cloned()).unwrap();
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "",
        "the command is done, not sent"
    );
    assert!(step(cx) + step(cx) > 0, "the line unfolds");
    at_rest(cx);
    assert_eq!(
        label("session-reload-after-turn", cx).as_deref(),
        Some(RELOADS_AFTER_TURN)
    );
    assert_eq!(reloads(&user, &id), 0, "the turn is not cut");

    std::fs::write(user.path("fake/release"), "").unwrap();
    until(&shell, cx, "the turn ended", |cx| {
        !session_page(&shell, cx).read_with(cx, |page, _| page.reloads_after_turn())
    });
    eventually("the process was let go", || reloads(&user, &id) == 1);
    step(cx);
    assert!(step(cx) > 0, "the line folds away");
    at_rest(cx);
    assert_eq!(label("session-reload-after-turn", cx), None);
    let ended = named(&user, &id, "agent.turn_ended");
    assert_eq!(ended[0]["data"]["stop_reason"], "end_turn");
}

#[gpui_kit::test]
fn the_session_menu_offers_reload_tools(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    project(&user);
    let (shell, cx) = window(&user, cx);
    let id = open_idle_session(&user, &shell, "lead the work", cx);
    eventually("the first turn ends", || {
        shown(&user, &id)["state"] == "idle"
    });
    until(&shell, cx, "the page knows it is idle", |cx| {
        session_page(&shell, cx)
            .read_with(cx, |page, _| page.tree().iter().any(|a| a.state == "idle"))
    });
    settle(cx);

    cx.update(|window, cx| window.click("rail-agent-menu", cx));
    settle(cx);
    cx.update(|window, _| assert!(window.find("rail-agent-reload-tools").visible()));
    cx.update(|window, cx| window.click("rail-agent-reload-tools", cx));

    until(&shell, cx, "the process was let go", |_| {
        reloads(&user, &id) == 1
    });
    // Nothing restarts before its next message: the toast says when the tools come.
    until(&shell, cx, "the toast says it", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|(title, _)| title == RELOADED)
    });
    let (title, detail) = toasts(&shell, cx)
        .into_iter()
        .find(|(title, _)| title == RELOADED)
        .unwrap();
    assert_eq!(title, "Tools reload at its next message");
    assert!(
        detail.contains("at its next message") && !detail.contains("start"),
        "{detail}"
    );
    at_rest(cx);
}

/// The toasts shown, each its title and its second line.
fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<(String, String)> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| {
                (
                    toast.title().to_owned(),
                    toast.second_line().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    })
}

fn reloads_after_turn(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    session_page(shell, cx).read_with(cx, |page, _| page.reloads_after_turn())
}

#[gpui_kit::test]
fn a_reload_from_the_command_line_during_a_turn_says_it_reloads_after_this_turn(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work", "do": [{"hold": "release"}, {"say": "Done."}]}
    ]}));
    project(&user);
    let (shell, cx) = window(&user, cx);
    let id = open_idle_session(&user, &shell, "work", cx);
    eventually("the turn runs", || shown(&user, &id)["state"] == "working");
    settle(cx);
    assert_eq!(label("session-reload-after-turn", cx), None);

    let out = user.run(&["agent", "reload-tools", &id]);
    assert!(out.status.success(), "{out:?}");

    until(&shell, cx, "the page says it waits", |cx| {
        reloads_after_turn(&shell, cx)
    });
    assert!(step(cx) + step(cx) > 0, "the line unfolds");
    at_rest(cx);
    assert_eq!(
        label("session-reload-after-turn", cx).as_deref(),
        Some(RELOADS_AFTER_TURN)
    );

    // Away from the page and back while the turn runs: the daemon still says it.
    cx.update(|window, cx| window.click(project_id("app"), cx));
    until(&shell, cx, "the session's page is left", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_none())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the page says it waits again", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
            && reloads_after_turn(&shell, cx)
    });
    at_rest(cx);
    assert_eq!(
        label("session-reload-after-turn", cx).as_deref(),
        Some(RELOADS_AFTER_TURN)
    );
    assert_eq!(reloads(&user, &id), 0, "the turn is not cut");

    // Reloaded once the turn ended: the line folds away.
    std::fs::write(user.path("fake/release"), "").unwrap();
    until(&shell, cx, "the turn ended", |cx| {
        !reloads_after_turn(&shell, cx)
    });
    step(cx);
    assert!(step(cx) > 0, "the line folds away");
    at_rest(cx);
    assert_eq!(label("session-reload-after-turn", cx), None);
    eventually("the process was let go", || reloads(&user, &id) == 1);
}
