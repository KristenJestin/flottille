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
                "body": "Users stay signed in"}}}]},
        {"when": "work a lot", "do": [
            {"say": "Looking into it."},
            {"think": "Where to start."},
            call("w1", "fs_list", "completed", json!({"path": "."})),
            call("w2", "fs_read", "failed", json!({"path": "gone.md"})),
            {"wait": 1100},
            call("w3", "run", "completed", json!({"line": "cargo test"})),
            call("w4", "fs_read", "completed", json!({"path": "README.md"})),
            {"say": "All read."}]},
        {"when": "hold and run", "do": [
            call("h1", "run", "in_progress", json!({"line": "sleep 30"})),
            {"update": {"sessionUpdate": "tool_call_update", "toolCallId": "h1",
                "status": "completed",
                "rawOutput": "run r9: `sleep 30` is running after 1.0 s\nit keeps going: ask run_status\n(no output)"}},
            call("h2", "fs_list", "completed", json!({"path": "."})),
            {"mcp": {"tool": "agent_launch", "arguments": {"title": "Docs",
                "brief": "Write the docs", "root": true, "workspace": "w"}}}]},
        {"when": "grep a while", "do": [
            call("g1", "fs_list", "completed", json!({"path": "."})),
            call("g2", "run", "in_progress", json!({"line": "git grep -n TODO"})),
            {"hold": "release"},
            {"update": {"sessionUpdate": "tool_call_update", "toolCallId": "g2",
                "status": "completed"}},
            call("g3", "fs_read", "completed", json!({"path": "README.md"})),
            {"hold": "again"},
            {"say": "Found them."}]},
        {"when": "ask through a child", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "curious", "brief": "ask your parent"}}}]},
        {"when": "ask your parent", "do": [{"mcp": {"tool": "ask_parent",
            "arguments": {"question": "Page Équipe asks: apps/front/src/team.tsx, which layout should the grid take?"}}}]},
        {"when": "say a lot", "do": [{"say": LONG}]},
        {"when": "say more", "do": [{"say": "One more thing."}]},
        {"when": "think alone", "do": [{"say": "Let me see."}, {"think": "Which file first."},
            {"say": "The README."}]},
        {"when": "ask the user something", "do": [
            call("q1", "fs_list", "completed", json!({"path": "."})),
            call("q2", "ask_user", "completed", json!({"question": "Which colour?"}))]},
        {"when": "work at length", "do": at_length()},
        {"when": "work past a page", "do": past_a_page()}
    ]})
}

/// A group of 300 rows: a thought and a call, again and again, between two sentences.
fn at_length() -> Vec<Value> {
    let mut steps = vec![json!({"say": "Begin."})];
    for n in 0..150 {
        steps.push(json!({"think": format!("Step {n}.")}));
        steps.push(call(
            &format!("l{n}"),
            "fs_list",
            "completed",
            json!({"path": "."}),
        ));
    }
    steps.push(json!({"say": "Finished."}));
    steps
}

/// More calls than a page of the history holds, then a short group: the last page read cuts
/// the long one.
fn past_a_page() -> Vec<Value> {
    let mut steps = vec![json!({"say": "Start."})];
    for n in 0..1100 {
        steps.push(call(
            &format!("p{n}"),
            "fs_list",
            "completed",
            json!({"path": "."}),
        ));
    }
    steps.push(json!({"say": "Middle."}));
    for n in 0..3 {
        steps.push(call(
            &format!("m{n}"),
            "fs_read",
            "completed",
            json!({"path": "a.md"}),
        ));
    }
    steps.push(json!({"say": "End."}));
    steps
}

/// A tool call of Flottille's, as the agent reports it.
fn call(id: &str, tool: &str, status: &str, input: Value) -> Value {
    json!({"update": {"sessionUpdate": "tool_call", "toolCallId": id,
        "title": format!("mcp__flottille__{tool}"), "kind": "other", "status": status,
        "rawInput": input}})
}

/// An answer longer than the window is high.
const LONG: &str = "One.\n\nTwo.\n\nThree.\n\nFour.\n\nFive.\n\nSix.\n\nSeven.\n\nEight.\n\n\
    Nine.\n\nTen.\n\nEleven.\n\nTwelve.\n\nThirteen.\n\nFourteen.\n\nFifteen.\n\nSixteen.\n\n\
    Seventeen.\n\nEighteen.\n\nNineteen.\n\nTwenty.\n\nTwenty-one.\n\nTwenty-two.\n\n\
    Twenty-three.\n\nTwenty-four.\n\nTwenty-five.\n\nTwenty-six.\n\nTwenty-seven.\n\n\
    Twenty-eight.\n\nTwenty-nine.\n\nThirty.";

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
    unfold(&page, at, cx);
    let line = label(format!("row-{at}-steps-toggle"), cx).unwrap();
    assert!(line.ends_with("· allowed by the judge"), "{line}");
    cx.update(|window, cx| window.click(format!("row-{at}-steps-toggle"), cx));
    at_rest(cx);
    assert_eq!(
        label(format!("step-{at}-{n}-allowed"), cx).as_deref(),
        Some("allowed by the judge")
    );
}

#[gpui_kit::test]
fn a_step_whose_result_was_cut_says_so_with_the_characters_left_out(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_cut_results();
    user.fake_script(json!({"turns": [
        {"when": "read the log", "do": [{"mcp": {"tool": "search", "keyed": true,
            "arguments": {"pattern": "line", "glob": "big.txt"}}}]},
        {"when": "read the note", "do": [{"mcp": {"tool": "search", "keyed": true,
            "arguments": {"pattern": "short", "glob": "note.txt"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let lead = lead(&user);
    let folder = user
        .call("workspace.show", json!({"project": "app", "name": "w"}))
        .unwrap()["folder"]
        .as_str()
        .unwrap()
        .to_owned();
    // Long lines: a search shows a hundred of them, and that is more than the limit. (`fs_read`
    // pages its text under the limit, so it is never cut.)
    let log: String = (1..=120)
        .map(|n| format!("line {n:04} {}\n", "of the log ".repeat(13)))
        .collect();
    std::fs::write(PathBuf::from(&folder).join("big.txt"), &log).unwrap();
    std::fs::write(PathBuf::from(&folder).join("note.txt"), "short\n").unwrap();
    let (shell, page, cx) = open_session(&user, &lead, cx);

    user.call(
        "agent.send",
        json!({"agent": lead, "message": "read the log"}),
    )
    .unwrap();
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "read the note"}),
    )
    .unwrap();
    // Every `search` step: its block, its place in it, how many characters were left out.
    let reads = |cx: &mut VisualTestContext| {
        let mut found = Vec::new();
        for (at, block) in blocks(&page, cx).iter().enumerate() {
            if let Block::Steps { calls } = block {
                for (n, call) in calls.iter().enumerate() {
                    if call.name == "search" && call.until.is_some() {
                        found.push((at, n, call.cut));
                    }
                }
            }
        }
        found
    };
    until(&shell, cx, "both reads are done", |cx| reads(cx).len() == 2);
    settle(cx);

    let found = reads(cx);
    let (at, n, cut) = found[0];
    assert!(cut.is_some_and(|left_out| left_out > 8_000), "{found:?}");
    let (short_at, short_n, short_cut) = found[1];
    assert_eq!(short_cut, None, "the short note was not cut");
    let mut opened = Vec::new();
    for toggle in [at, short_at] {
        unfold(&page, toggle, cx);
        if !opened.contains(&toggle) {
            cx.update(|window, cx| window.click(format!("row-{toggle}-steps-toggle"), cx));
            at_rest(cx);
            opened.push(toggle);
        }
    }
    let said = label(format!("step-{at}-{n}-cut"), cx).expect("the cut is said on its step");
    assert!(said.starts_with("cut at the source · "), "{said}");
    assert!(said.ends_with(" characters left out"), "{said}");
    assert!(
        said.contains(','),
        "the count is grouped by thousands: {said}"
    );
    assert!(
        label(format!("step-{short_at}-{short_n}-cut"), cx).is_none(),
        "a result that was not cut says nothing"
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

/// Unfolds the group that holds the row `at`, when it is folded: its rows are drawn.
fn unfold(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) {
    let groups = page.read_with(cx, |page, cx| page.conversation().read(cx).groups());
    let Some(group) = groups.into_iter().find(|group| group.contains(&at)) else {
        return;
    };
    let line = page
        .read_with(cx, |page, cx| page.conversation().read(cx).group_id(&group))
        .expect("a group's line");
    if cx.update(|window, _| window.find(line.clone()).expanded()) == Some(false) {
        cx.update(|window, cx| window.click(line, cx));
        settle(cx);
    }
}

/// Sends the lead `message`: the window's checks wait for what it does.
fn worked(user: &User, lead: &str, message: &str) {
    user.call("agent.send", json!({"agent": lead, "message": message}))
        .unwrap();
}

/// The id of the line of a group.
fn line_of(
    page: &Entity<SessionPage>,
    group: &std::ops::Range<usize>,
    cx: &mut VisualTestContext,
) -> String {
    page.read_with(cx, |page, cx| page.conversation().read(cx).group_id(group))
        .expect("a group's line")
}

/// The folded groups of the conversation, each the rows it holds.
fn groups(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<std::ops::Range<usize>> {
    page.read_with(cx, |page, cx| page.conversation().read(cx).groups())
}

/// The group that holds the row of the first block that matches.
fn group_of(
    page: &Entity<SessionPage>,
    cx: &mut VisualTestContext,
    matches: impl Fn(&Block) -> bool,
) -> std::ops::Range<usize> {
    let at = row_of(page, cx, matches).expect("the block is shown");
    groups(page, cx)
        .into_iter()
        .find(|group| group.contains(&at))
        .unwrap_or_else(|| panic!("row {at} is in no group"))
}

fn is_text(block: &Block, said: &str) -> bool {
    matches!(block, Block::Text { text } if text == said)
}

/// The lead's conversation once it worked a lot between two of its sentences.
fn after_much_work<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
    std::ops::Range<usize>,
) {
    let lead = lead(user);
    worked(user, &lead, "work a lot");
    let (shell, page, cx) = open_session(user, &lead, cx);
    until(&shell, cx, "the work is read", |cx| {
        row_of(&page, cx, |b| is_text(b, "All read.")).is_some()
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Thought { .. }));
    (shell, page, cx, group)
}

#[gpui_kit::test]
fn steps_between_two_messages_are_one_folded_group(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, page, cx, group) = after_much_work(&user, cx);
    let line = line_of(&page, &group, cx);
    let before = row_of(&page, cx, |b| is_text(b, "Looking into it.")).unwrap();
    let after = row_of(&page, cx, |b| is_text(b, "All read.")).unwrap();
    assert_eq!(group, before + 1..after, "everything between the two");
    assert!(has(line.clone(), cx), "one line");
    for at in group.clone() {
        assert!(!has(format!("row-{at}-thought"), cx), "row {at} is folded");
        assert!(!has(format!("row-{at}-steps"), cx), "row {at} is folded");
    }
    assert_eq!(
        cx.update(|window, _| { window.find(line.clone()).expanded() }),
        Some(false)
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn the_group_line_counts_steps_kinds_failures_and_time(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, page, cx, group) = after_much_work(&user, cx);
    let id = line_of(&page, &group, cx);
    let line = label(id.clone(), cx).expect("the group's line");
    // A thought and four calls; their kinds in order of first use; the failed read; the time.
    let (said, time) = line.rsplit_once(" · ").unwrap();
    assert_eq!(said, "5 steps · fs_list, fs_read, run · 1 failed", "{line}");
    let seconds: f64 = time.strip_suffix('s').unwrap().parse().unwrap();
    assert!(seconds >= 1.1, "{line}");
    assert_eq!(
        label(format!("{id}-failed"), cx).as_deref(),
        Some("1 failed"),
        "the failures in their own ink"
    );
}

#[gpui_kit::test]
fn a_click_unfolds_the_group_into_today_s_view(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, page, cx, group) = after_much_work(&user, cx);
    let line = line_of(&page, &group, cx);
    let (thought, steps) = (group.start, group.start + 1);
    cx.update(|window, cx| window.click(line.clone(), cx));
    settle(cx);
    assert!(has(format!("row-{thought}-thought"), cx));
    assert!(has(format!("row-{steps}-steps"), cx));
    // Each step keeps its own fold.
    assert_eq!(
        label(format!("row-{steps}-steps-toggle"), cx).as_deref(),
        Some("4 steps · fs_read")
    );
    assert_eq!(
        cx.update(|window, _| { window.find(line.clone()).expanded() }),
        Some(true)
    );
    at_rest(cx);

    cx.update(|window, cx| window.click(line.clone(), cx));
    at_rest(cx);
    assert!(!has(format!("row-{thought}-thought"), cx), "folded again");
    assert!(!has(format!("row-{steps}-steps"), cx));
}

#[gpui_kit::test]
fn the_agent_s_prose_and_the_user_s_messages_are_never_folded(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, page, cx, _) = after_much_work(&user, cx);
    let groups = groups(&page, cx);
    for (at, block) in blocks(&page, cx).iter().enumerate() {
        let part = match block {
            Block::Text { .. } => "text",
            Block::Message { .. } => "message",
            _ => continue,
        };
        assert!(
            !groups.iter().any(|group| group.contains(&at)),
            "row {at} is in no group: {block:?}"
        );
        assert!(has(format!("row-{at}-{part}"), cx), "row {at} is drawn");
    }
}

#[gpui_kit::test]
fn a_held_call_or_a_running_command_shows_outside_the_fold(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "hold and run");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the request and the command are read", |cx| {
        row_of(&page, cx, |b| matches!(b, Block::Approval { .. })).is_some()
            && row_of(&page, cx, |b| matches!(b, Block::Command(_))).is_some()
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Steps { .. }));
    let line = line_of(&page, &group, cx);
    let steps = row_of(&page, cx, |b| matches!(b, Block::Steps { .. })).unwrap();
    let card = row_of(&page, cx, |b| matches!(b, Block::Command(_))).unwrap();
    let request = row_of(&page, cx, |b| matches!(b, Block::Approval { .. })).unwrap();
    let Block::Approval { number } = blocks(&page, cx)[request] else {
        unreachable!()
    };
    assert!(
        group.contains(&card) && group.contains(&request),
        "{group:?}"
    );
    assert!(has(line.clone(), cx));
    assert!(
        !has(format!("row-{steps}-steps"), cx),
        "the steps are folded"
    );
    assert!(
        has(format!("command-card-{card}"), cx),
        "the command still running shows under the line"
    );
    assert!(
        has(format!("approval-line-{number}"), cx),
        "the call held for the user shows under the line"
    );
    let line = cx.update(|window, _| window.find(line).bounds());
    let held = cx.update(|window, _| window.find(format!("approval-line-{number}")).bounds());
    assert!(line.bottom() <= held.top(), "under the group's line");

    // Answered, it folds away into the group.
    user.call("approval.deny", json!({"number": number}))
        .unwrap();
    until(&shell, cx, "the request is answered", |cx| {
        page.read_with(cx, |page, _| page.decision(number).is_some())
    });
    settle(cx);
    assert!(
        !has(format!("approval-line-{number}"), cx),
        "an answered request folds away"
    );
    assert!(
        has(format!("command-card-{card}"), cx),
        "the command still runs"
    );
}

#[gpui_kit::test]
fn the_group_in_progress_names_its_running_step(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "grep a while"}),
    )
    .unwrap();
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the grep runs", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Steps { calls } if calls.len() == 2))
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Steps { .. }));
    let id = line_of(&page, &group, cx);
    let line = label(id.clone(), cx).unwrap();
    assert!(
        line.starts_with("2 steps · fs_list, run · running git grep -n TODO"),
        "{line}"
    );
    assert!(!has(format!("row-{}-steps", group.start), cx), "folded too");

    // The step ends and another comes: the line follows.
    std::fs::write(user.path("fake/release"), "").unwrap();
    until(&shell, cx, "the next step", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Steps { calls } if calls.len() == 3))
    });
    settle(cx);
    let line = label(id, cx).unwrap();
    assert!(
        line.starts_with("3 steps · fs_list, run, fs_read"),
        "{line}"
    );
    assert!(!line.contains("running"), "{line}");
    std::fs::write(user.path("fake/again"), "").unwrap();
}

#[gpui_kit::test]
fn an_unfolded_group_stays_unfolded_while_new_steps_arrive(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "grep a while"}),
    )
    .unwrap();
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the grep runs", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Steps { calls } if calls.len() == 2))
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Steps { .. }));
    let line = line_of(&page, &group, cx);
    cx.update(|window, cx| window.click(line.clone(), cx));
    settle(cx);
    assert!(has(format!("row-{}-steps", group.start), cx));

    std::fs::write(user.path("fake/release"), "").unwrap();
    until(&shell, cx, "the next step", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Steps { calls } if calls.len() == 3))
    });
    settle(cx);
    assert!(
        has(format!("row-{}-steps", group.start), cx),
        "still unfolded"
    );
    std::fs::write(user.path("fake/again"), "").unwrap();
    until(&shell, cx, "the turn ends", |cx| {
        row_of(&page, cx, |b| is_text(b, "Found them.")).is_some()
    });
    settle(cx);
    assert!(
        has(format!("row-{}-steps", group.start), cx),
        "still unfolded once the turn ended"
    );
}

#[gpui_kit::test]
fn a_delivery_is_one_folded_line_even_in_an_unfolded_group(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "ask through a child"}),
    )
    .unwrap();
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let is_question =
        |b: &Block| matches!(b, Block::Notice { kind, .. } if kind == "child-question");
    until(&shell, cx, "the child's question reached the lead", |cx| {
        row_of(&page, cx, is_question).is_some()
    });
    settle(cx);
    let at = row_of(&page, cx, is_question).unwrap();
    let group = group_of(&page, cx, is_question);
    let line = line_of(&page, &group, cx);
    assert!(!has(format!("row-{at}-notice"), cx), "in the fold");
    cx.update(|window, cx| window.click(line.clone(), cx));
    settle(cx);

    // One line: its kind and its first words.
    assert!(has(format!("row-{at}-notice"), cx));
    let first = label(format!("row-{at}-notice-summary"), cx).unwrap();
    assert!(
        first.contains("asks: Page Équipe asks: apps/front/src/team.tsx"),
        "{first}"
    );
    assert!(
        !has(format!("row-{at}-notice-text"), cx),
        "its text is folded"
    );
    let height =
        cx.update(|window, _| window.find(format!("row-{at}-notice")).bounds().size.height);
    assert!(height < px(24.), "one line: {height:?}");

    cx.update(|window, cx| window.click(format!("row-{at}-notice-toggle"), cx));
    settle(cx);
    let whole = label(format!("row-{at}-notice-text"), cx).unwrap();
    assert!(
        whole.contains("which layout should the grid take?"),
        "{whole}"
    );
    at_rest(cx);
}

/// Scrolls the conversation of `agent` up, as a reader would.
fn scroll_up(agent: &str, cx: &mut VisualTestContext) {
    let middle = cx.update(|window, _| {
        window
            .find(format!("conversation-{agent}"))
            .bounds()
            .center()
    });
    for _ in 0..4 {
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: middle,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(400.))),
            ..Default::default()
        });
        settle(cx);
    }
}

fn following(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> bool {
    page.read_with(cx, |page, cx| page.conversation().read(cx).is_following())
}

#[gpui_kit::test]
fn a_button_brings_back_to_the_end_when_scrolled_up(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "say a lot");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the long answer is read", |cx| {
        row_of(
            &page,
            cx,
            |b| matches!(b, Block::Text { text } if text.ends_with("Thirty.")),
        )
        .is_some()
    });
    settle(cx);
    assert!(following(&page, cx));
    assert!(!has("conversation-to-end", cx), "at the end, no button");

    scroll_up(&lead, cx);
    assert!(!following(&page, cx));
    assert!(has("conversation-to-end", cx), "scrolled up, the button");
    // Over the conversation, centred, above the composer.
    let button = cx.update(|window, _| window.find("conversation-to-end").bounds());
    let conversation = cx.update(|window, _| window.find(format!("conversation-{lead}")).bounds());
    let composer = cx.update(|window, _| window.find("reply-field").bounds());
    assert!(button.bottom() <= composer.top(), "{button:?} {composer:?}");
    assert!(
        conversation.contains(&button.center()),
        "over the conversation"
    );
    let centre = conversation.center().x;
    assert!(
        (button.center().x - centre).abs() < px(2.),
        "centred: {button:?}"
    );
    assert!(!has("conversation-to-end-dot", cx), "nothing new");

    cx.update(|window, cx| window.click("conversation-to-end", cx));
    at_rest(cx);
    assert!(following(&page, cx), "it follows the end again");
    assert!(!has("conversation-to-end", cx), "and the button left");
}

#[gpui_kit::test]
fn the_button_marks_new_messages_and_leaves_at_the_end(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "say a lot");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the long answer is read", |cx| {
        row_of(
            &page,
            cx,
            |b| matches!(b, Block::Text { text } if text.ends_with("Thirty.")),
        )
        .is_some()
    });
    settle(cx);
    scroll_up(&lead, cx);
    assert!(has("conversation-to-end", cx));
    assert!(!has("conversation-to-end-dot", cx));

    user.call("agent.send", json!({"agent": lead, "message": "say more"}))
        .unwrap();
    until(&shell, cx, "the new answer is read", |cx| {
        row_of(&page, cx, |b| is_text(b, "One more thing.")).is_some()
    });
    settle(cx);
    assert!(!following(&page, cx), "still where the user is");
    assert!(
        has("conversation-to-end-dot", cx),
        "the dot says something new"
    );

    // Back at the end by hand: the button leaves, with its dot.
    let middle = cx.update(|window, _| {
        window
            .find(format!("conversation-{lead}"))
            .bounds()
            .center()
    });
    for _ in 0..8 {
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: middle,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-800.))),
            ..Default::default()
        });
        settle(cx);
    }
    at_rest(cx);
    assert!(!has("conversation-to-end", cx), "the end is in sight");
    assert!(!has("conversation-to-end-dot", cx));
}

#[gpui_kit::test]
fn a_lone_thought_is_a_group_of_one_step(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "think alone");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the thought is read", |cx| {
        row_of(&page, cx, |b| is_text(b, "The README.")).is_some()
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Thought { .. }));
    assert_eq!(group.len(), 1, "the thought alone");
    let line = label(line_of(&page, &group, cx), cx).unwrap();
    assert!(line.starts_with("1 step · thought · "), "{line}");
    assert!(!has(format!("row-{}-thought", group.start), cx), "folded");
    at_rest(cx);
}

#[gpui_kit::test]
fn an_ask_user_call_stays_outside_the_fold_without_its_siblings(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "ask the user something");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let asks = |b: &Block| matches!(b, Block::Steps { calls } if calls.len() == 2);
    until(&shell, cx, "the question is read", |cx| {
        row_of(&page, cx, asks).is_some()
    });
    settle(cx);
    let at = row_of(&page, cx, asks).unwrap();
    let group = group_of(&page, cx, asks);
    let line = line_of(&page, &group, cx);
    assert_eq!(
        cx.update(|window, _| window.find(line.clone()).expanded()),
        Some(false)
    );
    assert!(has(format!("row-{at}-steps"), cx), "the question shows");
    assert_eq!(
        label(format!("row-{at}-steps-toggle"), cx).as_deref(),
        Some("1 step · ask_user"),
        "alone, without the call merged with it"
    );
    cx.update(|window, cx| window.click(format!("row-{at}-steps-toggle"), cx));
    settle(cx);
    assert!(has(format!("step-{at}-1"), cx), "the question");
    assert!(!has(format!("step-{at}-0"), cx), "not its sibling");

    // Unfolded, the group shows the sibling too.
    cx.update(|window, cx| window.click(line.clone(), cx));
    settle(cx);
    assert!(has(format!("step-{at}-0"), cx));
    cx.update(|window, cx| window.click(line, cx));
    settle(cx);
    assert!(!has(format!("step-{at}-0"), cx), "folded again");
    assert!(has(format!("step-{at}-1"), cx));

    // The user answered: the group is no longer the last, and its question folds in.
    worked(&user, &lead, "say more");
    until(&shell, cx, "the answer is read", |cx| {
        row_of(&page, cx, |b| is_text(b, "One more thing.")).is_some()
    });
    at_rest(cx);
    assert!(!has(format!("row-{at}-steps"), cx), "in the fold");
}

#[gpui_kit::test]
fn a_held_call_and_a_running_command_of_an_older_group_stay_outside_the_fold(
    cx: &mut TestAppContext,
) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "hold and run");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the request and the command are read", |cx| {
        row_of(&page, cx, |b| matches!(b, Block::Approval { .. })).is_some()
            && row_of(&page, cx, |b| matches!(b, Block::Command(_))).is_some()
    });
    worked(&user, &lead, "say more");
    until(&shell, cx, "a later turn", |cx| {
        row_of(&page, cx, |b| is_text(b, "One more thing.")).is_some()
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Command(_)));
    let later = row_of(&page, cx, |b| is_text(b, "One more thing.")).unwrap();
    assert!(group.end <= later, "an older group: {group:?}");
    let card = row_of(&page, cx, |b| matches!(b, Block::Command(_))).unwrap();
    let request = row_of(&page, cx, |b| matches!(b, Block::Approval { .. })).unwrap();
    let Block::Approval { number } = blocks(&page, cx)[request] else {
        unreachable!()
    };
    assert!(group.contains(&request), "{group:?}");
    assert!(has(format!("command-card-{card}"), cx));
    assert!(has(format!("approval-line-{number}"), cx));
    at_rest(cx);
}

#[gpui_kit::test]
fn the_sub_agents_list_stays_in_sight_amid_the_work(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "ask through a child");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the sub-agents list", |cx| {
        row_of(&page, cx, |b| *b == Block::SubAgents).is_some()
    });
    settle(cx);
    let at = row_of(&page, cx, |b| *b == Block::SubAgents).unwrap();
    assert!(
        !groups(&page, cx).iter().any(|group| group.contains(&at)),
        "in no group"
    );
    assert!(has(format!("row-{at}-sub-agents"), cx), "drawn");
}

/// The lead's conversation once it worked through 300 rows between two sentences.
fn after_long_work<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<SessionPage>,
    &'a mut VisualTestContext,
    std::ops::Range<usize>,
) {
    let lead = lead(user);
    worked(user, &lead, "work at length");
    let (shell, page, cx) = open_session(user, &lead, cx);
    until(&shell, cx, "the work is read", |cx| {
        row_of(&page, cx, |b| is_text(b, "Finished.")).is_some()
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Thought { .. }));
    (page, cx, group)
}

fn items(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> usize {
    page.read_with(cx, |page, cx| page.conversation().read(cx).item_count())
}

fn walked(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> usize {
    page.read_with(cx, |page, cx| page.conversation().read(cx).walked())
}

#[gpui_kit::test]
fn an_unfolded_group_is_one_list_item_per_row(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx, group) = after_long_work(&user, cx);
    assert_eq!(group.len(), 300, "{group:?}");
    let folded = items(&page, cx);
    let line = line_of(&page, &group, cx);
    cx.update(|window, cx| window.click(line.clone(), cx));
    at_rest(cx);
    assert_eq!(items(&page, cx), folded + 300, "each row its own item");
    // The list lays out only what is in view.
    let drawn = (group.start..group.end)
        .filter(|at| has(format!("row-{at}-thought"), cx) || has(format!("row-{at}-steps"), cx))
        .count();
    assert!(drawn < 100, "{drawn} rows drawn");

    // Back up to its line, out of view below its rows.
    let lead = page.read_with(cx, |page, _| page.id().to_owned());
    for _ in 0..60 {
        if has(line.clone(), cx) {
            break;
        }
        wheel_up(&lead, cx);
        settle(cx);
    }
    cx.update(|window, cx| window.click(line, cx));
    at_rest(cx);
    assert_eq!(items(&page, cx), folded, "folded, its rows leave the list");
}

#[gpui_kit::test]
fn drawing_a_folded_group_walks_none_of_its_rows(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx, _group) = after_long_work(&user, cx);
    let before = walked(&page, cx);
    for _ in 0..10 {
        page.update(cx, |page, cx| {
            page.conversation().update(cx, |_, cx| cx.notify())
        });
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    assert_eq!(
        walked(&page, cx),
        before,
        "its line is kept, not said again"
    );
}

/// The lead's conversation read from its last page, which cuts its long group.
fn past_a_page_read<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
) {
    let lead = lead(user);
    worked(user, &lead, "work past a page");
    eventually("the long turn ends", || {
        user.call("agent.show", json!({"agent": lead, "before": u64::MAX}))
            .unwrap()["events"]
            .as_array()
            .unwrap()
            .last()
            .is_some_and(|e| e["name"] == "agent.turn_ended")
    });
    let (shell, page, cx) = open_session(user, &lead, cx);
    until(&shell, cx, "the last page is read", |cx| {
        row_of(&page, cx, |b| is_text(b, "End.")).is_some()
    });
    settle(cx);
    assert!(page.read_with(cx, |p, cx| p.conversation().read(cx).has_earlier()));
    (shell, page, cx)
}

fn show_earlier(shell: &Entity<Shell>, page: &Entity<SessionPage>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("show-earlier", cx));
    until(shell, cx, "the earlier page", |cx| {
        !page.read_with(cx, |p, cx| p.conversation().read(cx).has_earlier())
    });
}

fn is_long(block: &Block) -> bool {
    matches!(block, Block::Steps { calls } if calls[0].name == "fs_list")
}

fn is_short(block: &Block) -> bool {
    matches!(block, Block::Steps { calls } if calls[0].name == "fs_read")
}

#[gpui_kit::test]
fn an_unfolded_group_stays_unfolded_when_earlier_history_loads(cx: &mut TestAppContext) {
    let user = user();
    let (shell, page, cx) = past_a_page_read(&user, cx);
    let group = group_of(&page, cx, is_long);
    let line = line_of(&page, &group, cx);
    let said = label(line.clone(), cx).unwrap();
    assert!(said.contains(" loaded"), "the part read only: {said}");
    cx.update(|window, cx| window.click(line, cx));
    settle(cx);

    show_earlier(&shell, &page, cx);
    settle(cx);
    let group = group_of(&page, cx, is_long);
    let line = line_of(&page, &group, cx);
    assert_eq!(
        cx.update(|window, _| window.find(line.clone()).expanded()),
        Some(true),
        "still unfolded"
    );
    let said = label(line, cx).unwrap();
    assert!(
        said.starts_with("1100 steps · fs_list"),
        "whole now: {said}"
    );
}

#[gpui_kit::test]
fn loading_earlier_history_animates_no_fold(cx: &mut TestAppContext) {
    let user = user();
    let (shell, page, cx) = past_a_page_read(&user, cx);
    let short = group_of(&page, cx, is_short);
    let line = line_of(&page, &short, cx);
    cx.update(|window, cx| window.click(line, cx));
    settle(cx);

    show_earlier(&shell, &page, cx);
    let (long, short) = (group_of(&page, cx, is_long), group_of(&page, cx, is_short));
    let (long_line, short_line) = (line_of(&page, &long, cx), line_of(&page, &short, cx));
    let row = format!("{short_line}-row-{}", short.start);
    let mut heights = Vec::new();
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        assert_eq!(
            cx.update(|window, _| window.find(long_line.clone()).expanded()),
            Some(false)
        );
        assert_eq!(
            cx.update(|window, _| window.find(short_line.clone()).expanded()),
            Some(true)
        );
        assert!(
            !has(format!("{long_line}-row-{}", long.start), cx),
            "the folded group shows no row"
        );
        heights.push(cx.update(|window, _| window.find(row.clone()).bounds().size.height));
    }
    assert!(
        heights.windows(2).all(|pair| pair[0] == pair[1]),
        "the unfolded row never folds: {heights:?}"
    );
}

/// Scrolls the conversation of `agent` up once, without letting it settle.
fn wheel_up(agent: &str, cx: &mut VisualTestContext) {
    let middle = cx.update(|window, _| {
        window
            .find(format!("conversation-{agent}"))
            .bounds()
            .center()
    });
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: middle,
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(400.))),
        ..Default::default()
    });
}
