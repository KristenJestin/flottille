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
use flottille_window::actions::Command;
use flottille_window::conversation::{Block, JUDGE};
use flottille_window::link::Connect;
use flottille_window::queued::{CANCEL, EDIT, SEND_NOW, WAITS, queued_button, queued_id};
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

/// The lead works, and a message the user wrote to it waits under the conversation: the
/// composer, and the waiting message's delivery.
fn with_a_waiting_message<'a>(
    user: &User,
    lead: &str,
    text: &str,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SessionPage>,
    Entity<flottille_window::session::Reply>,
    String,
    &'a mut VisualTestContext,
) {
    let (shell, page, cx) = open_session(user, lead, cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "work a while"}),
    )
    .unwrap();
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(lead).cloned())
        .expect("the session has a composer");
    until(&shell, cx, "the turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });
    let delivery = user
        .call("agent.send", json!({"agent": lead, "message": text}))
        .unwrap()["delivery"]
        .as_str()
        .unwrap()
        .to_owned();
    until(&shell, cx, "the message waits", |cx| {
        queued(&page, cx) == [text]
    });
    settle(cx);
    (shell, page, reply, delivery, cx)
}

fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|request| request.to_string())
        .collect()
}

#[gpui_kit::test]
fn a_waiting_message_has_send_now_edit_and_cancel_buttons_a_sent_one_has_none(
    cx: &mut TestAppContext,
) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, _, delivery, cx) =
        with_a_waiting_message(&user, &lead, "and update the docs", cx);

    for (which, words) in [("send-now", SEND_NOW), ("edit", EDIT), ("cancel", CANCEL)] {
        assert_eq!(
            label(queued_button(&delivery, which), cx).as_deref(),
            Some(words),
            "a visible label on {which}"
        );
    }
    // Once the root read it, it is a message like the others, with none of them.
    until(&shell, cx, "the message is sent", |cx| {
        row_of(&page, cx, |b| is_message(b, "and update the docs")).is_some()
            && queued(&page, cx).is_empty()
    });
    at_rest(cx);
    for which in ["send-now", "edit", "cancel"] {
        assert!(!has(queued_button(&delivery, which), cx), "{which}");
    }
}

#[gpui_kit::test]
fn cancelling_a_waiting_message_folds_its_bubble_away_and_the_root_never_receives_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, _, delivery, cx) = with_a_waiting_message(&user, &lead, "azdsqdsq", cx);

    cx.update(|window, cx| window.click(queued_button(&delivery, "cancel"), cx));

    until(&shell, cx, "the bubble leaves the queue", |cx| {
        queued(&page, cx).is_empty()
    });
    at_rest(cx);
    assert!(!has(queued_id(&delivery), cx), "the bubble folded away");
    assert!(waiting_in_the_daemon(&user, &lead).is_empty());
    // The turn goes on and ends: the root never got the message, and the conversation never
    // shows it.
    user.call("agent.cancel", json!({"agent": lead})).unwrap();
    until(&shell, cx, "the turn ends", |cx| {
        row_of(&page, cx, |b| is_message(b, "work a while")).is_some()
            && state_of(&user, &lead) == "idle"
    });
    assert!(prompts(&user).iter().all(|p| !p.contains("azdsqdsq")));
    assert!(row_of(&page, cx, |b| is_message(b, "azdsqdsq")).is_none());
}

fn waiting_in_the_daemon(user: &User, lead: &str) -> Vec<Value> {
    user.call("agent.tree", json!({"agent": lead})).unwrap()["agents"][0]["queued"]
        .as_array()
        .unwrap()
        .clone()
}

#[gpui_kit::test]
fn editing_a_waiting_message_puts_its_text_and_pictures_in_the_composer_after_what_was_typed(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    let mut with_images = script();
    with_images["images"] = true.into();
    user.fake_script(with_images);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let lead = lead(&user);
    let (shell, page, reply, delivery, cx) = {
        let (shell, page, cx) = open_session(&user, &lead, cx);
        user.call(
            "agent.send",
            json!({"agent": lead, "message": "work a while"}),
        )
        .unwrap();
        let reply = shell
            .read_with(cx, |s, _| s.reply_of(&lead).cloned())
            .unwrap();
        until(&shell, cx, "the turn runs", |cx| {
            reply.read_with(cx, |r, _| r.is_running())
        });
        let delivery = user
            .call(
                "agent.send",
                json!({"agent": lead, "message": "look at this",
                    "images": [{"mime": "image/png", "data": "iVBORw0KGgo="}]}),
            )
            .unwrap()["delivery"]
            .as_str()
            .unwrap()
            .to_owned();
        until(&shell, cx, "the message waits", |cx| {
            queued(&page, cx) == ["look at this"]
        });
        settle(cx);
        (shell, page, reply, delivery, cx)
    };
    let field = reply.read_with(cx, |r, _| r.field().clone());
    field.update_in(cx, |field, window, cx| {
        field.set_value("a draft", window, cx)
    });

    cx.update(|window, cx| window.click(queued_button(&delivery, "edit"), cx));

    until(&shell, cx, "the message is in the composer", |cx| {
        field.read_with(cx, |f, _| f.value().to_string()) == "a draft\nlook at this"
    });
    until(&shell, cx, "its picture is attached", |cx| {
        reply.read_with(cx, |r, _| r.attachments().len() == 1)
    });
    until(&shell, cx, "it left the queue", |cx| {
        queued(&page, cx).is_empty()
    });
    assert!(waiting_in_the_daemon(&user, &lead).is_empty());
    // Sent again, it waits again at the end of the queue.
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "it waits again", |cx| {
        queued(&page, cx) == ["a draft\nlook at this"]
    });
}

#[gpui_kit::test]
fn withdrawing_a_message_the_root_already_read_says_so_in_a_toast_and_the_message_shows_as_sent(
    cx: &mut TestAppContext,
) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, _, delivery, cx) =
        with_a_waiting_message(&user, &lead, "and update the docs", cx);
    // The root takes it first.
    user.call("agent.cancel", json!({"agent": lead})).unwrap();
    eventually("the root to read it", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("and update the docs"))
    });

    shell.update(cx, |shell, _| {
        shell.perform(Command::Unqueue {
            session: lead.clone(),
            delivery: delivery.clone(),
            edit: false,
        })
    });

    until(&shell, cx, "the toast says so", |cx| {
        toasts(&shell, cx) == ["Already read by the agent"]
    });
    until(&shell, cx, "it shows as sent", |cx| {
        row_of(&page, cx, |b| is_message(b, "and update the docs")).is_some()
            && queued(&page, cx).is_empty()
    });
}

#[gpui_kit::test]
fn send_now_stops_the_turn_and_the_root_receives_that_message_next_before_the_others(
    cx: &mut TestAppContext,
) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, _, first, cx) =
        with_a_waiting_message(&user, &lead, "and update the docs", cx);
    let second = user
        .call(
            "agent.send",
            json!({"agent": lead, "message": "then fix the tests"}),
        )
        .unwrap()["delivery"]
        .as_str()
        .unwrap()
        .to_owned();
    until(&shell, cx, "both wait", |cx| queued(&page, cx).len() == 2);
    settle(cx);

    cx.update(|window, cx| window.click(queued_button(&second, "send-now"), cx));

    // The long turn is cut short; the next prompt is the second message alone, and the first
    // follows it.
    until(&shell, cx, "both were read", |cx| {
        queued(&page, cx).is_empty()
            && row_of(&page, cx, |b| is_message(b, "and update the docs")).is_some()
            && row_of(&page, cx, |b| is_message(b, "then fix the tests")).is_some()
    });
    let sent = prompts(&user);
    let next = sent.len() - 2;
    assert!(sent[next].contains("then fix the tests") && !sent[next].contains("update the docs"));
    assert!(sent[next + 1].contains("and update the docs"));
    let _ = first;
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

/// The width of an element, or `None` while it is not drawn.
fn width_of(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<f32> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map(|found| f32::from(found.bounds().size.width))
    })
}

/// The widths of `id` on each frame, until every animation ended.
fn widths_while_it_moves(id: &str, cx: &mut VisualTestContext) -> Vec<f32> {
    let mut seen = Vec::new();
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        seen.extend(width_of(id.to_owned(), cx));
    }
    seen
}

/// The page of `agent` is the one the window shows.
fn shows(shell: &Entity<Shell>, agent: &str, cx: &mut VisualTestContext) -> bool {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .is_some_and(|page| page.read_with(cx, |p, _| p.id() == agent))
}

#[gpui_kit::test]
fn a_relayed_card_keeps_one_width_while_it_unfolds_and_folds(cx: &mut TestAppContext) {
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
    let card = format!("row-{at}-relayed-card");
    let folded = width_of(card.clone(), cx).expect("the card is drawn");

    cx.update(|window, cx| window.click(format!("row-{at}-relayed-toggle"), cx));
    let unfolding = widths_while_it_moves(&card, cx);
    assert!(unfolding.len() > 2, "{unfolding:?}");
    assert!(
        unfolding.iter().all(|w| (*w - folded).abs() < 0.5),
        "the card is {folded} px folded and moved through {unfolding:?} while it unfolded"
    );

    cx.update(|window, cx| window.click(format!("row-{at}-relayed-toggle"), cx));
    let folding = widths_while_it_moves(&card, cx);
    assert!(
        folding.iter().all(|w| (*w - folded).abs() < 0.5),
        "the card is {folded} px open and moved through {folding:?} while it folded"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn the_name_on_a_from_card_opens_that_session(cx: &mut TestAppContext) {
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
    assert_eq!(
        label(format!("row-{at}-relayed-peer"), cx).as_deref(),
        Some("lead the work")
    );

    cx.update(|window, cx| window.click(format!("row-{at}-relayed-peer"), cx));
    until(&shell, cx, "the sender's session opens", |cx| {
        shows(&shell, &lead, cx)
    });
}

#[gpui_kit::test]
fn the_name_on_a_to_card_opens_that_session(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let docs = docs_session(&user, &lead);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": format!("write to the session {docs}")}),
    )
    .unwrap();
    allow_the_request(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the lead shows what it sent", |cx| {
        row_of(&page, cx, |b| {
            matches!(b, Block::Relayed { sent: true, .. })
        })
        .is_some()
    });
    settle(cx);
    let at = row_of(&page, cx, |b| {
        matches!(b, Block::Relayed { sent: true, .. })
    })
    .unwrap();
    assert_eq!(
        label(format!("row-{at}-relayed-peer"), cx).as_deref(),
        Some("Docs")
    );

    cx.update(|window, cx| window.click(format!("row-{at}-relayed-peer"), cx));
    until(&shell, cx, "the target's session opens", |cx| {
        shows(&shell, &docs, cx)
    });
}

#[gpui_kit::test]
fn a_relayed_card_whose_session_no_longer_exists_has_its_name_and_no_link(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let event = |seq: u64, name: &str, data: Value| flottille_core::protocol::Event {
        seq,
        at: seq,
        name: name.into(),
        data,
    };
    page.update(cx, |p, cx| {
        let token = p.token().unwrap();
        p.apply(
            token,
            flottille_window::session_link::SessionUpdate::History {
                events: vec![event(
                    1,
                    "agent.prompted",
                    json!({"text": "[flottille:session]\n\"Gone\" (agent ffffffffffff) wrote to this session:\n\nBye."}),
                )],
                earlier: false,
            },
            cx,
        )
    });
    settle(cx);
    let at = row_of(&page, cx, |b| {
        matches!(b, Block::Relayed { sent: false, .. })
    })
    .expect("the card is drawn");
    assert_eq!(
        label(format!("row-{at}-relayed-from"), cx).as_deref(),
        Some("from Gone")
    );
    assert!(
        !has(format!("row-{at}-relayed-peer"), cx),
        "no link to a session that is gone"
    );
    let _ = shell;
}
