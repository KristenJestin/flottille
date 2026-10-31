//! The scenarios of #184, the conversation of the session page: a message sent while the agent
//! works waits under the conversation, then is a message like the others; a hover on a message
//! shows its time and Copy; another session's message is folded; a call the Auto judge let
//! through says so. On GPUI's test platform in front of a real daemon whose agents (and judge) are
//! the fake ACP agent and whose `gh` is the fake one, with a throwaway user. No window opens on a
//! screen.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::{Block, JUDGE};
use flottille_window::link::Connect;
use flottille_window::queued::{WAITS, queued_id};
use flottille_window::session::{COPIED, SessionPage, said_at};
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn script() -> Value {
    json!({"turns": [
        {"when": "work a while", "do": [{"wait": 4000}, {"say": "Done with that."}]},
        {"when": "start the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "root": true,
                "workspace": "w"}}}]},
        {"when": "write to the session", "do": [{"mcp": {"tool": "session_send",
            "arguments": {"session": "{prompt:agent}",
                "text": "The tests pass now.\nAll 42 of them, on the branch app/w."}}}]},
        {"when": "open a ticket", "do": [{"mcp": {"tool": "tickets", "keyed": true,
            "arguments": {"action": "create", "title": "Add a logout",
                "body": "Users stay signed in"}}}]}
    ]})
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(script());
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
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

/// The project `app` over a throwaway repository, its `origin` GitHub's `acme/app` (set once its
/// ready workspace `w` is made, offline), its forge and its tracker declared.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "w", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    folder
}

/// A session the user started in the workspace, its first turn ended.
fn lead(user: &User) -> String {
    project(user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the lead's first turn", || state_of(user, &id) == "idle");
    id
}

fn pending(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// Allows the one request that waits.
fn allow_the_request(user: &User) {
    eventually("the call is held", || !pending(user).is_empty());
    let number = pending(user)[0]["number"].as_u64().unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
}

/// The session the lead started for the user, its first turn ended.
fn docs_session(user: &User, lead: &str) -> String {
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "start the docs session"}),
    )
    .unwrap();
    allow_the_request(user);
    let launched = || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["launched_by"] == lead)
            .and_then(|agent| agent["id"].as_str().map(str::to_owned))
    };
    eventually("the session exists", || launched().is_some());
    let id = launched().unwrap();
    eventually("its first turn ends", || state_of(user, &id) == "idle");
    id
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
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

/// Lets every animation end, and checks that nothing is drawn once they have.
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

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
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

fn open_session<'a>(
    user: &User,
    agent: &str,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(agent), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
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
    settle(cx);
    let page = session_page(&shell, cx);
    (shell, page, cx)
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

/// The row of the first block that matches.
fn row_of(
    page: &Entity<SessionPage>,
    cx: &mut VisualTestContext,
    matches: impl Fn(&Block) -> bool,
) -> Option<usize> {
    blocks(page, cx).iter().position(matches)
}

fn is_message(block: &Block, said: &str) -> bool {
    matches!(block, Block::Message { text, .. } if text == said)
}

fn queued(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<String> {
    page.read_with(cx, |page, cx| {
        page.queued()
            .read(cx)
            .messages()
            .iter()
            .map(|m| m.text.clone())
            .collect()
    })
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

#[gpui_kit::test]
fn a_message_sent_during_a_turn_shows_as_waiting_then_as_sent(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "work a while"}),
    )
    .unwrap();
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&lead).cloned())
        .expect("the session has a composer");
    until(&shell, cx, "the turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });

    // Sent from the composer while the agent works: it leaves the field and waits below.
    let field = reply.read_with(cx, |r, _| r.field().clone());
    field.update_in(cx, |field, window, cx| {
        field.set_value("and update the docs", window, cx)
    });
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(
        &shell,
        cx,
        "the message waits under the conversation",
        |cx| queued(&page, cx) == ["and update the docs"],
    );
    settle(cx);
    let delivery = page.read_with(cx, |page, cx| {
        page.queued().read(cx).messages()[0].delivery.clone()
    });
    assert_eq!(
        label(queued_id(&delivery), cx).as_deref(),
        Some("and update the docs")
    );
    assert_eq!(
        label(format!("queued-{delivery}-waits"), cx).as_deref(),
        Some(WAITS)
    );
    assert!(
        row_of(&page, cx, |b| is_message(b, "and update the docs")).is_none(),
        "not a message of the conversation yet"
    );
    let field_text = field.read_with(cx, |f, _| f.value().to_string());
    assert_eq!(field_text, "", "it left the composer");
    let bubble = cx.update(|window, _| window.find(queued_id(&delivery)).bounds());
    let composer = cx.update(|window, _| window.find("reply-field").bounds());
    assert!(
        bubble.bottom() <= composer.top(),
        "above the composer: {bubble:?} over {composer:?}"
    );

    // Once the step is over, the agent reads it: a message like the others, and nothing waits.
    until(&shell, cx, "the message is sent", |cx| {
        row_of(&page, cx, |b| is_message(b, "and update the docs")).is_some()
            && queued(&page, cx).is_empty()
    });
    at_rest(cx);
    assert!(
        !has(queued_id(&delivery), cx),
        "the waiting bubble folded away"
    );
}

#[gpui_kit::test]
fn hover_on_a_message_shows_its_time_and_copy(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (_shell, page, cx) = open_session(&user, &lead, cx);
    let mine = row_of(&page, cx, |b| is_message(b, "lead the work")).unwrap();
    let theirs = row_of(&page, cx, |b| matches!(b, Block::Text { .. })).unwrap();

    for at in [mine, theirs] {
        assert!(!has(format!("row-{at}-copy"), cx), "nothing before a hover");
        cx.update(|window, cx| window.hover(format!("row-{at}-said"), cx));
        settle(cx);
        let when = page.read_with(cx, |page, cx| page.conversation().read(cx).at(at));
        assert_eq!(
            label(format!("row-{at}-time"), cx),
            when.map(said_at),
            "its date and time"
        );
        assert!(has(format!("row-{at}-copy"), cx), "and Copy");
    }
    // The hover left the message: what it showed fades away.
    cx.update(|window, cx| window.hover(format!("row-{mine}-said"), cx));
    settle(cx);
    assert!(!has(format!("row-{theirs}-copy"), cx));
    at_rest(cx);
}

#[gpui_kit::test]
fn copy_puts_the_text_on_the_clipboard_and_says_it(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let mine = row_of(&page, cx, |b| is_message(b, "lead the work")).unwrap();

    cx.update(|window, cx| window.hover(format!("row-{mine}-said"), cx));
    settle(cx);
    cx.update(|window, cx| window.click(format!("row-{mine}-copy"), cx));
    settle(cx);

    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("lead the work"));
    assert_eq!(toasts(&shell, cx), [COPIED]);
}

#[gpui_kit::test]
fn another_session_s_message_is_folded_then_unfolds(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let docs = docs_session(&user, &lead);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": format!("write to the session {docs}")}),
    )
    .unwrap();
    allow_the_request(&user);
    let (shell, page, cx) = open_session(&user, &docs, cx);
    until(&shell, cx, "the message reached the session", |cx| {
        row_of(&page, cx, |b| {
            matches!(b, Block::Relayed { sent: false, .. })
        })
        .is_some()
    });
    settle(cx);
    let at = row_of(&page, cx, |b| {
        matches!(b, Block::Relayed { sent: false, .. })
    })
    .unwrap();

    // Folded: who sent it and its first line.
    assert_eq!(
        label(format!("row-{at}-relayed-from"), cx).as_deref(),
        Some("from lead the work")
    );
    assert_eq!(
        label(format!("row-{at}-relayed-first"), cx).as_deref(),
        Some("The tests pass now.")
    );
    assert!(
        !has(format!("row-{at}-relayed-text"), cx),
        "the rest is folded"
    );

    cx.update(|window, cx| window.click(format!("row-{at}-relayed-toggle"), cx));
    settle(cx);
    assert!(
        has(format!("row-{at}-relayed-text"), cx),
        "a click unfolds it"
    );
    assert!(!has(format!("row-{at}-relayed-first"), cx));
    let rendered = page.read_with(cx, |page, cx| {
        page.conversation().read(cx).rows()[at]
            .read(cx)
            .rendered(cx)
            .unwrap()
    });
    assert!(rendered.contains("All 42 of them"), "{rendered}");

    cx.update(|window, cx| window.click(format!("row-{at}-relayed-toggle"), cx));
    at_rest(cx);
    assert!(
        !has(format!("row-{at}-relayed-text"), cx),
        "and folds again"
    );
}

#[gpui_kit::test]
fn a_ticket_created_in_auto_says_allowed_by_the_judge(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_fake_gh().with_allowing_judge();
    user.fake_script(script());
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let lead = lead(&user);
    user.gh_script(json!({"issues": {"acme/app": {}}, "created_issue": 46}));
    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();
    let (shell, page, cx) = open_session(&user, &lead, cx);

    user.call(
        "agent.send",
        json!({"agent": lead, "message": "open a ticket"}),
    )
    .unwrap();
    let ticket = |cx: &mut VisualTestContext| {
        blocks(&page, cx)
            .iter()
            .enumerate()
            .find_map(|(at, b)| match b {
                Block::Steps { calls } => calls
                    .iter()
                    .position(|c| c.name == "tickets" && c.until.is_some())
                    .map(|n| (at, n, calls[n].allowed_by.clone())),
                _ => None,
            })
    };
    until(&shell, cx, "the ticket is created", |cx| {
        ticket(cx).is_some()
    });
    settle(cx);
    assert!(pending(&user).is_empty(), "no question to the user");
    let (at, n, allowed_by) = ticket(cx).unwrap();
    assert_eq!(allowed_by.as_deref(), Some(JUDGE));

    // Folded, the steps' line says it; open, the step does.
    let line = label(format!("row-{at}-steps-toggle"), cx).unwrap();
    assert!(line.ends_with("· allowed by the judge"), "{line}");
    cx.update(|window, cx| window.click(format!("row-{at}-steps-toggle"), cx));
    at_rest(cx);
    assert_eq!(
        label(format!("step-{at}-{n}-allowed"), cx).as_deref(),
        Some("allowed by the judge")
    );
}
