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
use flottille_window::queued::{
    CANCEL, EDIT, SEND_NOW, WAITS, WAITS_FULL, queued_button, queued_id,
};
use flottille_window::session::{
    COPIED, RETRY, Reply, SENDING, SessionPage, pending_part, said_at,
};
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn script() -> Value {
    json!({"turns": [
        {"when": "work a while", "do": [{"wait": 4000}, {"say": "Done with that."}]},
        {"when": "think it over", "do": [{"wait": 1500}, {"say": "Thought over."}]},
        {"when": "work until stopped", "do": [{"hold": "release"}, {"say": "Done with that."}]},
        {"when": "start the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "follow": false, "root": true,
                "workspace": "w"}}}]},
        {"when": "start the long session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": LONG_TITLE, "brief": "Write the docs", "follow": false,
                "root": true,
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
                "brief": "Write the docs", "follow": false, "root": true, "workspace": "w"}}}]},
        {"when": "grep a while", "do": [
            call("g1", "fs_list", "completed", json!({"path": "."})),
            call("g2", "run", "in_progress", json!({"line": "git grep -n TODO"})),
            {"hold": "release"},
            {"update": {"sessionUpdate": "tool_call_update", "toolCallId": "g2",
                "status": "completed"}},
            call("g3", "fs_read", "completed", json!({"path": "README.md"})),
            {"hold": "again"},
            {"say": "Found them."}]},
        {"when": "run long", "do": [
            call("r1", "fs_list", "completed", json!({"path": "."})),
            call("r2", "run", "in_progress", json!({"line": LONG_LINE})),
            {"hold": "release"},
            {"update": {"sessionUpdate": "tool_call_update", "toolCallId": "r2",
                "status": "completed"}}]},
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
        {"when": "work past a page", "do": past_a_page()},
        {"when": "talk at length", "do": talk_at_length()}
    ]})
}

/// A sentence and a thought, again and again: three pages of the history (a page holds a
/// thousand events), each a few hundred blocks to scroll through.
fn talk_at_length() -> Vec<Value> {
    let mut steps = Vec::new();
    for n in 0..1050 {
        steps.push(json!({"say": format!("Paragraph {n}.")}));
        steps.push(json!({"think": format!("Thought {n}.")}));
    }
    steps.push(json!({"say": "Finally."}));
    steps
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

/// What a user typed to start a session: its first words are the session's title.
const LONG_TITLE: &str =
    "lis mes 4 story en cours, ca me parait simple ? on peut voir pour les faires en meme temps ?";

/// A command as long as a front-end test run's, as the steps summary names it.
const LONG_LINE: &str = "pnpm --filter @app/front exec ng test --no-watch --include \
    \"src/app/{pages/**/analytics,pages/**/reports,pages/**/settings,shared/**/charts}/**/*.spec.ts\"";

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
    lead_saying(user, "lead the work")
}

/// [`lead`], started with `message`: its title is the message's first words.
fn lead_saying(user: &User, message: &str) -> String {
    project(user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": message}),
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
    session_started_by(user, lead, "start the docs session")
}

/// The session the lead started when told `message`, its first turn ended.
fn session_started_by(user: &User, lead: &str, message: &str) -> String {
    user.call("agent.send", json!({"agent": lead, "message": message}))
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

fn window_through<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
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
    open_session_through(user, agent, Arc::new(user.connector()), cx)
}

fn open_session_through<'a>(
    user: &User,
    agent: &str,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
) {
    let (shell, cx) = window_through(user, connect, cx);
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
    // It is drawn under the key it was sent with, which it keeps while it waits.
    let delivery = format!("pending-{}", reply.read_with(cx, |r, _| r.pending()[0].key));
    until(
        &shell,
        cx,
        "the message waits under the conversation",
        |cx| queued(&page, cx) == ["and update the docs"],
    );
    settle(cx);
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

/// The lead works on `long`, and a message the user wrote to it waits under the conversation:
/// the composer, and the waiting message's delivery.
fn with_a_waiting_message<'a>(
    user: &User,
    lead: &str,
    long: &str,
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
    user.call("agent.send", json!({"agent": lead, "message": long}))
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
        with_a_waiting_message(&user, &lead, "work a while", "and update the docs", cx);

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
    let (shell, page, _, delivery, cx) =
        with_a_waiting_message(&user, &lead, "work a while", "azdsqdsq", cx);

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
        with_a_waiting_message(&user, &lead, "work a while", "and update the docs", cx);
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
    // The turn lasts until Send now stops it: one that ends first hands the root both messages
    // in one prompt, and Send now is refused, however slow the window is to the click.
    let (shell, page, _, first, cx) = with_a_waiting_message(
        &user,
        &lead,
        "work until stopped",
        "and update the docs",
        cx,
    );
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
    // The window shows a message as read once its turn is recorded, before the prompt has reached
    // the agent: what the agent received is waited for, not assumed.
    eventually("the root to receive the first message", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("and update the docs"))
    });
    let sent = prompts(&user);
    let said: Vec<Vec<&str>> = sent
        .iter()
        .map(|p| {
            [
                "work until stopped",
                "then fix the tests",
                "and update the docs",
            ]
            .into_iter()
            .filter(|m| p.contains(m))
            .collect()
        })
        .collect();
    let seen = format!("prompts {said:?}, toasts {:?}", toasts(&shell, cx));
    let next = sent.len() - 2;
    assert!(
        sent[next].contains("then fix the tests") && !sent[next].contains("update the docs"),
        "{seen}"
    );
    assert!(sent[next + 1].contains("and update the docs"), "{seen}");
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
fn the_failed_count_is_on_the_group_s_line(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, page, cx, group) = after_much_work(&user, cx);
    let id = line_of(&page, &group, cx);
    let line = cx.update(|window, _| window.find(id.clone()).bounds());
    let failed = cx.update(|window, _| window.find(format!("{id}-failed")).bounds());
    // On a line of its own, the count would sit below the middle of a line grown twice as high.
    let (count, whole) = (
        f32::from(failed.origin.y + failed.size.height / 2.),
        f32::from(line.origin.y + line.size.height / 2.),
    );
    assert!(
        (count - whole).abs() < 1.,
        "the count is on the line: {failed:?} in {line:?}"
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
fn an_ask_user_call_folds_with_its_siblings_its_card_says_the_question(cx: &mut TestAppContext) {
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
    // The question shows on its own card, from the daemon's open questions: the call that
    // asked it folds with the others (this one is only reported, never asked to the daemon).
    assert!(!has(format!("row-{at}-steps"), cx), "in the fold");
    cx.update(|window, cx| window.click(line, cx));
    settle(cx);
    assert_eq!(
        label(format!("row-{at}-steps-toggle"), cx).as_deref(),
        Some("2 steps · ask_user")
    );
    at_rest(cx);
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

fn offset(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> f32 {
    page.read_with(cx, |page, cx| {
        f32::from(page.conversation().read(cx).scroll_offset())
    })
}

/// The lead's long answer, scrolled up.
fn scrolled_up<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (String, Entity<SessionPage>, &'a mut VisualTestContext) {
    let lead = lead(user);
    worked(user, &lead, "say a lot");
    let (shell, page, cx) = open_session(user, &lead, cx);
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
    (lead, page, cx)
}

fn frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_scroll_to_the_end_eases_over_frames(cx: &mut TestAppContext) {
    let user = user();
    let (_lead, page, cx) = scrolled_up(&user, cx);
    let from = offset(&page, cx);
    cx.update(|window, cx| window.click("conversation-to-end", cx));
    for _ in 0..5 {
        frame(cx);
    }
    let midway = offset(&page, cx);
    assert!(!following(&page, cx), "still on its way");
    at_rest(cx);
    let to = offset(&page, cx);
    assert!(from < midway && midway < to, "{from} < {midway} < {to}");
    assert!(following(&page, cx));
}

#[gpui_kit::test]
fn a_wheel_scroll_stops_the_scroll_to_the_end(cx: &mut TestAppContext) {
    let user = user();
    let (lead, page, cx) = scrolled_up(&user, cx);
    cx.update(|window, cx| window.click("conversation-to-end", cx));
    frame(cx);
    frame(cx);
    wheel_up(&lead, cx);
    let stopped = offset(&page, cx);
    at_rest(cx);
    assert_eq!(
        offset(&page, cx),
        stopped,
        "it stays where the user scrolled"
    );
    assert!(!following(&page, cx));
    assert!(has("conversation-to-end", cx), "the button is back");
}

#[gpui_kit::test]
fn the_button_to_the_end_rises_into_place(cx: &mut TestAppContext) {
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
    wheel_up(&lead, cx);
    cx.run_until_parked();
    frame(cx);
    let arriving = cx.update(|window, _| window.find("conversation-to-end").bounds());
    at_rest(cx);
    let placed = cx.update(|window, _| window.find("conversation-to-end").bounds());
    assert!(
        arriving.top() > placed.top(),
        "from below: {arriving:?} then {placed:?}"
    );
}

/// A valid 8 x 8 PNG, so that the window can draw its thumbnail.
const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR4nGM4YWODFTEMLQkAZZlQAVIPr1MAAAAASUVORK5CYII=";

fn bubble_height(delivery: &str, cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, _| {
        window
            .try_find(format!("queued-item-{delivery}"))
            .map_or(0., |item| f32::from(item.bounds().size.height))
    })
}

#[gpui_kit::test]
fn a_queued_message_says_queued_in_one_short_line(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (_shell, _page, _, delivery, cx) =
        with_a_waiting_message(&user, &lead, "work a while", "and update the docs", cx);

    let waits = format!("queued-{delivery}-waits");
    assert_eq!(WAITS, "Queued · after this step");
    assert_eq!(label(waits.clone(), cx).as_deref(), Some(WAITS));
    // The long sentence is the tooltip's, once the pointer has rested on the line.
    cx.update(|window, cx| window.hover(waits.clone(), cx));
    settle(cx);
    assert_eq!(
        label(format!("{waits}-tooltip"), cx).as_deref(),
        Some(WAITS_FULL)
    );
    assert_eq!(WAITS_FULL, "Sent when the agent finishes its step");
}

#[gpui_kit::test]
fn a_queued_message_shows_its_pictures_as_thumbnails(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let mut with_images = script();
    with_images["images"] = true.into();
    user.fake_script(with_images);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let lead = lead(&user);
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
    let picture = json!({"mime": "image/png", "data": PNG_BASE64});
    let delivery = user
        .call(
            "agent.send",
            json!({"agent": lead, "message": "look at these", "images": [picture, picture]}),
        )
        .unwrap()["delivery"]
        .as_str()
        .unwrap()
        .to_owned();
    until(
        &shell,
        cx,
        "the message waits with its pictures read",
        |cx| {
            queued(&page, cx) == ["look at these"]
                && page.read_with(cx, |p, cx| p.queued().read(cx).pictures_read() == 2)
        },
    );
    settle(cx);

    for n in 0..2 {
        assert!(
            has(format!("queued-{delivery}-image-{n}"), cx),
            "thumbnail {n}"
        );
    }
    assert!(!has(format!("queued-{delivery}-image-2"), cx));
    // The count is not said in words any more: the pictures are there to see.
    assert_eq!(
        label(format!("queued-{delivery}-waits"), cx).as_deref(),
        Some(WAITS)
    );
    // #353 (the large viewer) is not merged: a click on one opens nothing and breaks nothing.
    cx.update(|window, cx| window.click(format!("queued-{delivery}-image-0"), cx));
    settle(cx);
    assert!(has(queued_id(&delivery), cx));
}

#[gpui_kit::test]
fn a_queued_message_unfolds_into_the_composer_and_folds_away_when_sent(cx: &mut TestAppContext) {
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
        .unwrap();
    until(&shell, cx, "the turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });
    settle(cx);
    let delivery = user
        .call(
            "agent.send",
            json!({"agent": lead, "message": "and the docs"}),
        )
        .unwrap()["delivery"]
        .as_str()
        .unwrap()
        .to_owned();
    until(&shell, cx, "the message waits", |cx| {
        queued(&page, cx) == ["and the docs"]
    });

    // It unfolds: one frame in, it is not at its full height yet.
    let first = bubble_height(&delivery, cx);
    settle(cx);
    let full = bubble_height(&delivery, cx);
    assert!(full > 40., "its room: {full}");
    assert!(first < full, "unfolding: {first} of {full}");

    // Cancelled (or sent), it folds away: still there a frame later, then gone.
    cx.update(|window, cx| window.click(queued_button(&delivery, "cancel"), cx));
    until(&shell, cx, "it leaves the queue", |cx| {
        queued(&page, cx).is_empty()
    });
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let folding = bubble_height(&delivery, cx);
    assert!(
        folding > 0. && folding < full,
        "folding: {folding} of {full}"
    );
    at_rest(cx);
    assert!(!has(queued_id(&delivery), cx));
}

/// The project's command `ship`.
fn with_ship(user: &User) {
    user.call(
        "command.add",
        json!({"project": "app", "name": "ship", "description": "Ship it",
            "instructions": "Push the branch, then open its pull request."}),
    )
    .unwrap();
}

/// The block of the user's message that used commands, and where it is.
fn commanded(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<(usize, Block)> {
    blocks(page, cx).into_iter().enumerate().find(|(_, block)| {
        matches!(
            block,
            Block::Message {
                commands: Some(_),
                ..
            }
        )
    })
}

/// The heights of `id` on each frame, until every animation ended.
fn heights_while_it_moves(id: &str, cx: &mut VisualTestContext) -> Vec<f32> {
    let mut seen = Vec::new();
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        seen.extend(cx.update(|window, _| {
            window
                .try_find(id.to_owned())
                .map(|found| f32::from(found.bounds().size.height))
        }));
    }
    seen
}

#[gpui_kit::test]
fn a_command_message_shows_what_the_user_typed(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    with_ship(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);

    user.call(
        "agent.send",
        json!({"agent": lead, "message": "fix the test, then /ship"}),
    )
    .unwrap();
    until(&shell, cx, "the message shows", |cx| {
        commanded(&page, cx).is_some()
    });
    settle(cx);

    let (at, block) = commanded(&page, cx).unwrap();
    let Block::Message {
        text,
        commands: Some(commanded),
        ..
    } = block
    else {
        unreachable!()
    };
    assert_eq!(text, "fix the test, then /ship", "what the user typed");
    assert_eq!(commanded.names, ["ship"]);
    assert!(
        commanded
            .received
            .contains("Push the branch, then open its pull request."),
        "{}",
        commanded.received
    );
    assert_eq!(
        flottille_window::session::command_chips(&text, &commanded.names),
        vec![19..24usize],
        "the chip is the /ship"
    );
    assert!(has(format!("row-{at}-message-text"), cx));
    assert_eq!(
        label(format!("row-{at}-received-toggle"), cx).as_deref(),
        Some(flottille_window::session::RECEIVED)
    );
    assert!(
        !has(format!("row-{at}-received-text"), cx),
        "what the agent received is folded at first"
    );
    // The agent received the commands' text, never the bare `/ship`.
    assert!(
        prompts(&user)
            .iter()
            .any(|p| p.contains("Push the branch") && p.contains("[command ship]")),
        "{:?}",
        prompts(&user)
    );
}

#[gpui_kit::test]
fn what_the_agent_received_unfolds_under_the_message(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    with_ship(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    user.call("agent.send", json!({"agent": lead, "message": "/ship now"}))
        .unwrap();
    until(&shell, cx, "the message shows", |cx| {
        commanded(&page, cx).is_some()
    });
    at_rest(cx);
    let at = commanded(&page, cx).unwrap().0;
    let received = format!("row-{at}-received");

    cx.update(|window, cx| window.click(format!("row-{at}-received-toggle"), cx));
    let unfolding = heights_while_it_moves(&received, cx);
    assert!(
        unfolding.len() > 2 && unfolding.windows(2).any(|w| w[0] < w[1]),
        "it unfolds over frames, never at once: {unfolding:?}"
    );
    at_rest(cx);
    assert!(has(format!("row-{at}-received-text"), cx));
    let shown = label(format!("row-{at}-received-text"), cx).unwrap_or_default();
    assert!(shown.contains("Push the branch"), "{shown}");
    assert!(shown.contains("[command ship] now"), "{shown}");

    cx.update(|window, cx| window.click(format!("row-{at}-received-toggle"), cx));
    let folding = heights_while_it_moves(&received, cx);
    assert!(
        folding.len() > 2 && folding.windows(2).any(|w| w[0] > w[1]),
        "and folds away over frames: {folding:?}"
    );
    at_rest(cx);
    assert!(!has(format!("row-{at}-received-text"), cx), "folded again");
}

#[gpui_kit::test]
fn a_queued_command_message_shows_what_the_user_typed(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    with_ship(&user);
    let (_shell, page, _, delivery, cx) =
        with_a_waiting_message(&user, &lead, "work a while", "then /ship it", cx);

    let waiting = page.read_with(cx, |page, cx| page.queued().read(cx).messages().to_vec());
    assert_eq!(waiting[0].text, "then /ship it");
    assert_eq!(waiting[0].commands, ["ship"]);
    assert!(has(format!("queued-{delivery}-text"), cx));
}

/// The bounds of an element, or `None` while it is not drawn.
fn bounds_of(
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut VisualTestContext,
) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).map(|found| found.bounds()))
}

/// The lead's conversation once it sent a message to a session titled `title` (the lead's own
/// title is `lead the work`).
fn after_sending_to<'a>(
    user: &User,
    title_of: impl Fn(&User, &str) -> String,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, usize, &'a mut VisualTestContext) {
    let lead = lead(user);
    let session = title_of(user, &lead);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": format!("write to the session {session}")}),
    )
    .unwrap();
    allow_the_request(user);
    let (shell, page, cx) = open_session(user, &lead, cx);
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
    (shell, at, cx)
}

/// The name in the header of the relayed card of row `at`, drawn inside the card.
fn assert_inside_its_card(at: usize, cx: &mut VisualTestContext) {
    let card = bounds_of(format!("row-{at}-relayed-card"), cx).expect("the card is drawn");
    let name = bounds_of(format!("row-{at}-relayed-name"), cx).expect("the name is drawn");
    assert!(
        name.right() <= card.right() && name.origin.x >= card.origin.x,
        "the name {name:?} runs out of its card {card:?}"
    );
    assert!(card.size.width <= px(541.), "the card grew: {card:?}");
}

#[gpui_kit::test]
fn a_long_session_title_in_a_relayed_header_is_cut_inside_its_bubble(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, at, cx) = after_sending_to(
        &user,
        |user, lead| session_started_by(user, lead, "start the long session"),
        cx,
    );
    assert_eq!(
        label(format!("row-{at}-relayed-peer"), cx).as_deref(),
        Some(LONG_TITLE)
    );
    assert_inside_its_card(at, cx);
    // The header keeps its one line, and the text below its width.
    let from = bounds_of(format!("row-{at}-relayed-from"), cx).unwrap();
    assert!(from.size.height < px(24.), "one line: {from:?}");
    let card = bounds_of(format!("row-{at}-relayed-card"), cx).unwrap();
    let text = bounds_of(format!("row-{at}-relayed-region"), cx).expect("the text is drawn");
    assert!(text.right() <= card.right(), "{text:?} in {card:?}");
    at_rest(cx);
}

#[gpui_kit::test]
fn the_whole_title_of_a_cut_relayed_header_shows_on_hover(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, at, cx) = after_sending_to(
        &user,
        |user, lead| session_started_by(user, lead, "start the long session"),
        cx,
    );
    let name = format!("row-{at}-relayed-name");
    assert!(!has(format!("{name}-tooltip"), cx), "no tooltip at rest");

    cx.update(|window, cx| window.hover(name.clone(), cx));
    settle(cx);
    assert_eq!(
        label(format!("{name}-tooltip"), cx).as_deref(),
        Some(LONG_TITLE)
    );

    // The pointer leaves: it fades out and goes.
    cx.simulate_mouse_move(
        gpui_kit::point(px(1430.), px(790.)),
        None,
        Default::default(),
    );
    settle(cx);
    assert!(!has(format!("{name}-tooltip"), cx), "the tooltip left");
    at_rest(cx);
}

#[gpui_kit::test]
fn a_short_relayed_header_is_not_cut(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, at, cx) = after_sending_to(&user, docs_session, cx);
    assert_inside_its_card(at, cx);
    let name = format!("row-{at}-relayed-name");
    cx.update(|window, cx| window.hover(name.clone(), cx));
    settle(cx);
    assert!(
        !has(format!("{name}-tooltip"), cx),
        "a title that fits has no tooltip"
    );
    assert_eq!(
        label(format!("row-{at}-relayed-from"), cx).as_deref(),
        Some("to Docs")
    );
    cx.simulate_mouse_move(
        gpui_kit::point(px(1430.), px(790.)),
        None,
        Default::default(),
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn a_long_session_title_in_a_received_header_is_cut_inside_its_bubble(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead_saying(&user, LONG_TITLE);
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
    assert_inside_its_card(at, cx);
    // The chevron that unfolds it stays in the card, and the first line keeps its width.
    let card = bounds_of(format!("row-{at}-relayed-card"), cx).unwrap();
    let toggle = bounds_of(format!("row-{at}-relayed-toggle"), cx).unwrap();
    assert!(toggle.right() <= card.right(), "{toggle:?} in {card:?}");
    let first = bounds_of(format!("row-{at}-relayed-first"), cx).unwrap();
    assert!(first.right() <= card.right(), "{first:?} in {card:?}");
    at_rest(cx);
}

#[gpui_kit::test]
fn a_long_steps_summary_is_cut_inside_the_conversation(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "run long");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the long command runs", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Steps { calls } if calls.len() == 2))
    });
    settle(cx);
    let group = group_of(&page, cx, |b| matches!(b, Block::Steps { .. }));
    let id = line_of(&page, &group, cx);
    let line = label(id.clone(), cx).unwrap();
    assert!(
        line.starts_with("2 steps · fs_list, run · running pnpm --filter"),
        "{line}"
    );
    let row = bounds_of(id.clone(), cx).expect("the line is drawn");
    let said = bounds_of(format!("{id}-said"), cx).expect("the summary is drawn");
    assert!(
        said.right() <= row.right(),
        "the summary {said:?} runs out of its line {row:?}"
    );
    assert!(row.size.height < px(28.), "one line: {row:?}");

    // The whole command shows on hover, and goes when the pointer leaves.
    let running = format!("{id}-running");
    assert!(!has(format!("{running}-tooltip"), cx));
    cx.update(|window, cx| window.hover(running.clone(), cx));
    settle(cx);
    assert_eq!(
        label(format!("{running}-tooltip"), cx).as_deref(),
        Some(format!("running {LONG_LINE}").as_str())
    );
    cx.simulate_mouse_move(
        gpui_kit::point(px(1430.), px(790.)),
        None,
        Default::default(),
    );
    settle(cx);
    assert!(!has(format!("{running}-tooltip"), cx));
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn a_long_command_in_a_step_is_cut_and_whole_on_hover(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "run long");
    let (shell, page, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the long command runs", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Steps { calls } if calls.len() == 2))
    });
    settle(cx);
    let at = row_of(&page, cx, |b| matches!(b, Block::Steps { .. })).unwrap();
    unfold(&page, at, cx);
    // Each step keeps its own fold.
    cx.update(|window, cx| window.click(format!("row-{at}-steps-toggle"), cx));
    settle(cx);
    let args = format!("step-{at}-1-args");
    let (row, line) = (
        bounds_of(format!("step-{at}-1"), cx).expect("the step is drawn"),
        bounds_of(args.clone(), cx).expect("its command is drawn"),
    );
    assert!(line.right() <= row.right(), "{line:?} runs out of {row:?}");
    assert!(!has(format!("{args}-tooltip"), cx), "no tooltip at rest");

    cx.update(|window, cx| window.hover(args.clone(), cx));
    settle(cx);
    let whole = label(format!("{args}-tooltip"), cx).expect("the whole command shows");
    assert!(whole.contains("pnpm --filter @app/front"), "{whole}");
    cx.simulate_mouse_move(
        gpui_kit::point(px(1430.), px(790.)),
        None,
        Default::default(),
    );
    settle(cx);
    assert!(!has(format!("{args}-tooltip"), cx), "the tooltip left");
    std::fs::write(user.path("fake/release"), "").unwrap();
}

/// The composer of the session, its field holding `text`, and the pictures it holds.
fn composer(shell: &Entity<Shell>, lead: &str, cx: &mut VisualTestContext) -> Entity<Reply> {
    shell
        .read_with(cx, |s, _| s.reply_of(lead).cloned())
        .expect("the session has a composer")
}

/// Types `text` in the composer and sends it: the key of the message being sent.
fn send(reply: &Entity<Reply>, text: &str, cx: &mut VisualTestContext) -> u64 {
    let field = reply.read_with(cx, |r, _| r.field().clone());
    field.update_in(cx, |field, window, cx| field.set_value(text, window, cx));
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    reply.read_with(cx, |r, _| {
        r.pending().last().expect("a message is sent").key
    })
}

/// Draws the window without taking anything from the daemon: what shows before it answers.
fn draw_alone(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
}

fn user_messages(page: &Entity<SessionPage>, said: &str, cx: &mut VisualTestContext) -> usize {
    blocks(page, cx)
        .iter()
        .filter(|b| is_message(b, said))
        .count()
}

fn field_text(reply: &Entity<Reply>, cx: &mut VisualTestContext) -> String {
    reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string())
}

#[gpui_kit::test]
fn a_sent_message_shows_before_the_daemon_answers(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let mut with_images = script();
    with_images["images"] = true.into();
    user.fake_script(with_images);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let lead = lead(&user);
    let gate = Gate::closed();
    let (shell, page, cx) = open_session_through(&user, &lead, gate.connect(&user), cx);
    let reply = composer(&shell, &lead, cx);
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(PNG_BASE64)
        .unwrap();
    reply.update_in(cx, |reply, _, cx| {
        reply.attach(
            gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png),
            cx,
        )
    });
    settle(cx);

    let key = send(&reply, "think it over", cx);
    draw_alone(cx);

    // The daemon's answer is held: the message is in the conversation, with its picture,
    // quieter and saying it is being sent; the composer is empty.
    assert!(reply.read_with(cx, |r, _| r.pending()[0].accepted.is_none()));
    assert_eq!(
        label(pending_part(key, "bubble"), cx).as_deref(),
        Some("think it over")
    );
    assert!(has(pending_part(key, "image-0"), cx), "its picture");
    assert_eq!(
        label(pending_part(key, "sending"), cx).as_deref(),
        Some(SENDING)
    );
    assert_eq!(user_messages(&page, "think it over", cx), 0);
    assert_eq!(field_text(&reply, cx), "");
    assert!(reply.read_with(cx, |r, _| r.attachments().is_empty()));
    let bubble = cx.update(|window, _| window.find(pending_part(key, "bubble")).bounds());
    let composer = cx.update(|window, _| window.find("reply-field").bounds());
    assert!(bubble.bottom() <= composer.top(), "in the conversation");

    gate.open();
    until(&shell, cx, "the turn ends", |cx| {
        user_messages(&page, "think it over", cx) == 1
            && !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
}

#[gpui_kit::test]
fn the_pending_message_becomes_the_recorded_one_without_a_second_copy(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let reply = composer(&shell, &lead, cx);

    let key = send(&reply, "think it over", cx);
    settle(cx);
    let pending = cx.update(|window, _| window.find(pending_part(key, "bubble")).bounds());

    until(&shell, cx, "the daemon records it", |cx| {
        user_messages(&page, "think it over", cx) == 1
    });
    // The frame it is recorded on: one bubble, where the pending one was.
    assert!(!has(pending_part(key, "bubble"), cx), "no second copy");
    let at = row_of(&page, cx, |b| is_message(b, "think it over")).unwrap();
    let recorded = cx.update(|window, _| window.find(format!("row-{at}-bubble")).bounds());
    assert_eq!(recorded, pending, "in place");
    assert!(reply.read_with(cx, |r, _| r.pending().is_empty()));
    settle(cx);
    assert!(
        !has(pending_part(key, "sending"), cx),
        "the line folded away"
    );

    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
    assert_eq!(user_messages(&page, "think it over", cx), 1);
}

/// A project command that reads a file which is gone: a message that uses it is refused.
fn a_command_that_cannot_be_read(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    let file = folder.join(".claude/commands/review.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "Review the diff.\n").unwrap();
    user.call("command.import", json!({"project": "app"}))
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    file
}

#[gpui_kit::test]
fn a_refused_message_stays_with_retry_and_edit(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    a_command_that_cannot_be_read(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let reply = composer(&shell, &lead, cx);

    let key = send(&reply, "/review now", cx);
    until(&shell, cx, "the daemon refuses it", |cx| {
        has(pending_part(key, "failed"), cx)
    });
    settle(cx);

    let said = label(pending_part(key, "failed"), cx).unwrap_or_default();
    assert!(said.contains("nothing was sent"), "{said}");
    assert_eq!(
        label(pending_part(key, "bubble"), cx).as_deref(),
        Some("/review now")
    );
    assert!(!has(pending_part(key, "sending"), cx));
    assert_eq!(
        label(pending_part(key, "retry"), cx).as_deref(),
        Some(RETRY)
    );
    assert_eq!(label(pending_part(key, "edit"), cx).as_deref(), Some(EDIT));
    assert_eq!(
        toasts(&shell, cx),
        Vec::<String>::new(),
        "said once, in place"
    );
    assert_eq!(user_messages(&page, "/review now", cx), 0);

    // Edit puts it back in the composer, and the failed message folds away.
    cx.update(|window, cx| window.click(pending_part(key, "edit"), cx));
    at_rest(cx);
    assert_eq!(field_text(&reply, cx), "/review now");
    assert!(!has(pending_part(key, "bubble"), cx));
    assert!(reply.read_with(cx, |r, _| r.pending().is_empty()));
}

#[gpui_kit::test]
fn retry_sends_the_failed_message_again(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let file = a_command_that_cannot_be_read(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let reply = composer(&shell, &lead, cx);
    let key = send(&reply, "/review now", cx);
    until(&shell, cx, "the daemon refuses it", |cx| {
        has(pending_part(key, "failed"), cx)
    });
    settle(cx);

    std::fs::write(&file, "Review the diff.\n").unwrap();
    cx.update(|window, cx| window.click(pending_part(key, "retry"), cx));
    draw_alone(cx);
    assert_eq!(
        label(pending_part(key, "sending"), cx).as_deref(),
        Some(SENDING),
        "sent again, in place"
    );

    until(&shell, cx, "the agent takes it", |cx| {
        user_messages(&page, "/review now", cx) == 1
            && reply.read_with(cx, |r, _| r.pending().is_empty())
    });
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
    assert!(!has(pending_part(key, "bubble"), cx));
    assert_eq!(user_messages(&page, "/review now", cx), 1);
}

#[gpui_kit::test]
fn a_message_sent_during_a_turn_keeps_its_queued_state(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "work a while"}),
    )
    .unwrap();
    let reply = composer(&shell, &lead, cx);
    until(&shell, cx, "the turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });
    settle(cx);

    let key = send(&reply, "and update the docs", cx);
    draw_alone(cx);
    // Before the daemon answers, it is under the conversation where it is to wait, being sent.
    let shown = format!("pending-{key}");
    assert_eq!(
        label(queued_id(&shown), cx).as_deref(),
        Some("and update the docs")
    );
    assert_eq!(
        label(format!("queued-{shown}-waits"), cx).as_deref(),
        Some(SENDING)
    );
    assert!(
        !has(pending_part(key, "bubble"), cx),
        "not in the conversation"
    );

    until(&shell, cx, "it waits", |cx| {
        queued(&page, cx) == ["and update the docs"]
    });
    let delivery = page.read_with(cx, |page, cx| {
        page.queued().read(cx).messages()[0].delivery.clone()
    });
    // The same bubble, now waiting as today, with its buttons.
    assert!(has(format!("queued-item-{shown}"), cx), "in place");
    assert!(
        !has(format!("queued-item-{delivery}"), cx),
        "no second copy"
    );
    settle(cx);
    assert_eq!(
        label(format!("queued-{shown}-waits"), cx).as_deref(),
        Some(WAITS)
    );
    assert_eq!(
        label(queued_button(&shown, "edit"), cx).as_deref(),
        Some(EDIT)
    );
    assert!(reply.read_with(cx, |r, _| r.pending().is_empty()));

    until(&shell, cx, "the message is sent", |cx| {
        user_messages(&page, "and update the docs", cx) == 1 && queued(&page, cx).is_empty()
    });
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
    assert_eq!(user_messages(&page, "and update the docs", cx), 1);
}

/// The window's connection to the daemon, through a relay that holds the daemon's answers to
/// `agent.send` until the gate opens: what the daemon did is done, the window does not know it
/// yet. Any other request goes through as it is.
struct Gate(
    Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    Arc<std::sync::atomic::AtomicUsize>,
);

impl Gate {
    fn closed() -> Gate {
        Gate(
            Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
            Arc::default(),
        )
    }

    /// How many answers of the daemon wait at the gate.
    fn holding(&self) -> usize {
        self.1.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn open(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }

    fn connect(&self, user: &User) -> Connect {
        let (real, gate, holding) = (user.connector(), self.0.clone(), self.1.clone());
        Arc::new(move || {
            use std::io::{Read, Write};
            use std::sync::atomic::{AtomicBool, Ordering};
            let mut daemon = real()?;
            let (window, mut relay) = std::os::unix::net::UnixStream::pair()?;
            let held = Arc::new(AtomicBool::new(false));
            let (mut to_daemon, mut from_daemon, mut to_window) =
                (daemon.try_clone()?, daemon.try_clone()?, relay.try_clone()?);
            let asks = held.clone();
            let (gate, holding) = (gate.clone(), holding.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                while let Ok(n) = relay.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if buf[..n].windows(12).any(|w| w == b"\"agent.send\"") {
                        asks.store(true, Ordering::SeqCst);
                    }
                    if to_daemon.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                let _ = to_daemon.shutdown(std::net::Shutdown::Write);
            });
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                while let Ok(n) = from_daemon.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if held.load(Ordering::SeqCst) {
                        let (open, waiting) = &*gate;
                        let mut open = open.lock().unwrap();
                        if !*open {
                            holding.fetch_add(1, Ordering::SeqCst);
                        }
                        while !*open {
                            open = waiting.wait(open).unwrap();
                        }
                    }
                    if to_window.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                let _ = to_window.shutdown(std::net::Shutdown::Both);
            });
            drop(daemon.flush());
            Ok(window)
        })
    }
}

/// The row of the user's message that says `said`, drawn: its bubble is in the window.
fn drawn_record(page: &Entity<SessionPage>, said: &str, cx: &mut VisualTestContext) -> bool {
    let Some(at) = row_of(page, cx, |b| is_message(b, said)) else {
        return false;
    };
    has(format!("row-{at}-bubble"), cx)
}

fn is_masked(block: &Block) -> bool {
    matches!(block, Block::Message { text, .. }
        if text.starts_with("think it over with token=") && !text.contains("ghp_ABCDEF"))
}

#[gpui_kit::test]
fn a_message_holding_a_secret_is_recorded_in_place(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let reply = composer(&shell, &lead, cx);
    // The daemon masks the token before it keeps the message: the record says it otherwise.
    let text = "think it over with token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let key = send(&reply, text, cx);
    settle(cx);
    let pending = cx.update(|window, _| window.find(pending_part(key, "bubble")).bounds());

    until(&shell, cx, "the daemon records it", |cx| {
        row_of(&page, cx, is_masked).is_some()
    });
    assert!(!has(pending_part(key, "bubble"), cx), "no second copy");
    let at = row_of(&page, cx, is_masked).unwrap();
    let drawn = cx.update(|window, _| window.find(format!("row-{at}-bubble")).bounds());
    assert_eq!(drawn.top(), pending.top(), "in place");
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
    assert!(reply.read_with(cx, |r, _| r.pending().is_empty()));
    assert!(!has(pending_part(key, "sending"), cx));
    assert_eq!(blocks(&page, cx).iter().filter(|b| is_masked(b)).count(), 1);
}

#[gpui_kit::test]
fn a_message_whose_picture_was_not_sent_is_recorded_in_place(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    let mut with_images = script();
    with_images["images"] = true.into();
    user.fake_script(with_images);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let lead = lead(&user);
    eventually("the lead's process is released", || {
        user.call("agent.show", json!({"agent": lead})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.released")
    });
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let reply = composer(&shell, &lead, cx);
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(PNG_BASE64)
        .unwrap();
    reply.update_in(cx, |reply, _, cx| {
        reply.attach(
            gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png),
            cx,
        )
    });
    settle(cx);
    // Its next process takes no pictures: the daemon says so in the text it records.
    user.fake_script(script());

    let key = send(&reply, "think it over", cx);
    settle(cx);
    let pending = cx.update(|window, _| window.find(pending_part(key, "bubble")).bounds());
    let recorded = "think it over\n\n[1 image not sent: this agent takes no images]";
    until(&shell, cx, "the daemon records it", |cx| {
        row_of(&page, cx, |b| is_message(b, recorded)).is_some()
    });
    assert!(!has(pending_part(key, "bubble"), cx), "no second copy");
    let at = row_of(&page, cx, |b| is_message(b, recorded)).unwrap();
    let drawn = cx.update(|window, _| window.find(format!("row-{at}-bubble")).bounds());
    assert_eq!(drawn.top(), pending.top(), "in place");
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
    assert!(reply.read_with(cx, |r, _| r.pending().is_empty()));
    assert_eq!(user_messages(&page, recorded, cx), 1);
}

#[gpui_kit::test]
fn a_message_whose_agent_dies_before_taking_it_is_marked_failed(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    user.fake_script(script());
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let lead = lead(&user);
    eventually("the lead's process is released", || {
        user.call("agent.show", json!({"agent": lead})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.released")
    });
    let (shell, page, cx) = open_session(&user, &lead, cx);
    let reply = composer(&shell, &lead, cx);
    // Each process it starts dies while it opens its session, until the agent fails.
    let mut dying = script();
    dying["die_on_open"] = 3.into();
    user.fake_script(dying);

    let key = send(&reply, "think it over", cx);
    until(&shell, cx, "the message is marked failed", |cx| {
        has(pending_part(key, "failed"), cx)
    });
    at_rest(cx);
    let said = label(pending_part(key, "failed"), cx).unwrap_or_default();
    assert!(said.contains("before it read it"), "{said}");
    assert!(!has(pending_part(key, "sending"), cx));
    assert_eq!(
        label(pending_part(key, "retry"), cx).as_deref(),
        Some(RETRY)
    );
    assert_eq!(label(pending_part(key, "edit"), cx).as_deref(), Some(EDIT));
    assert_eq!(user_messages(&page, "think it over", cx), 0);
}

#[gpui_kit::test]
fn the_same_text_from_the_cli_does_not_take_the_window_s_message(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let file = a_command_that_cannot_be_read(&user);
    let gate = Gate::closed();
    let (shell, page, cx) = open_session_through(&user, &lead, gate.connect(&user), cx);
    let reply = composer(&shell, &lead, cx);

    // The window's is refused, its answer held; the same words from the command line, once the
    // command reads again, are taken and recorded first.
    let key = send(&reply, "/review now", cx);
    eventually("the daemon refused the window's", || gate.holding() == 1);
    std::fs::write(&file, "Review the diff.\n").unwrap();
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "/review now"}),
    )
    .unwrap();
    until(&shell, cx, "the command line's is recorded", |cx| {
        user_messages(&page, "/review now", cx) == 1
    });
    settle(cx);
    assert_eq!(
        label(pending_part(key, "sending"), cx).as_deref(),
        Some(SENDING),
        "the window's still waits for its answer"
    );
    assert!(!drawn_record(&page, "/review now", cx), "not drawn as it");

    gate.open();
    until(&shell, cx, "the refusal reaches the window's", |cx| {
        has(pending_part(key, "failed"), cx)
    });
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
    assert!(drawn_record(&page, "/review now", cx), "the command line's");
    assert_eq!(user_messages(&page, "/review now", cx), 1);
    assert_eq!(
        label(pending_part(key, "retry"), cx).as_deref(),
        Some(RETRY)
    );
}

#[gpui_kit::test]
fn a_message_recorded_while_its_page_was_left_is_recorded_in_place(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let other = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "another thing"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the other's first turn", || {
        state_of(&user, &other) == "idle"
    });
    let gate = Gate::closed();
    let (shell, _, cx) = open_session_through(&user, &lead, gate.connect(&user), cx);
    let reply = composer(&shell, &lead, cx);

    let key = send(&reply, "think it over", cx);
    eventually("the daemon took it", || gate.holding() == 1);
    eventually("the daemon records it", || {
        user.received_requests("session/prompt")
            .iter()
            .any(|p| p.to_string().contains("think it over"))
    });
    until(&shell, cx, "the other session is listed", |cx| {
        has(session_id(&other), cx)
    });
    // Away and back while its answer is held: the page reads the history again, record included.
    cx.update(|window, cx| window.click(session_id(&other), cx));
    until(&shell, cx, "the other session shows", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.id() == other)
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the lead's page read its history", |cx| {
        let page = session_page(&shell, cx);
        page.read_with(cx, |p, _| p.id() == lead) && user_messages(&page, "think it over", cx) == 1
    });
    settle(cx);
    let page = session_page(&shell, cx);
    assert!(
        !drawn_record(&page, "think it over", cx),
        "one bubble: the one being sent"
    );
    assert!(has(pending_part(key, "bubble"), cx));

    gate.open();
    until(&shell, cx, "the answer comes", |cx| {
        reply.read_with(cx, |r, _| r.pending().is_empty())
    });
    assert!(!has(pending_part(key, "bubble"), cx), "no second copy");
    assert!(drawn_record(&page, "think it over", cx));
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    at_rest(cx);
    assert_eq!(user_messages(&page, "think it over", cx), 1);
}

#[gpui_kit::test]
fn a_queued_message_taken_back_before_the_tree_lists_it_leaves(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let gate = Gate::closed();
    let (shell, page, cx) = open_session_through(&user, &lead, gate.connect(&user), cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "work a while"}),
    )
    .unwrap();
    let reply = composer(&shell, &lead, cx);
    until(&shell, cx, "the turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });
    settle(cx);

    let key = send(&reply, "and update the docs", cx);
    eventually("the daemon queued it", || {
        waiting_in_the_daemon(&user, &lead).len() == 1
    });
    let delivery = waiting_in_the_daemon(&user, &lead)[0]["delivery"]
        .as_str()
        .unwrap()
        .to_owned();
    // Another client takes it back before the window heard the daemon's answer.
    user.call(
        "agent.unqueue",
        json!({"agent": lead, "delivery": delivery}),
    )
    .unwrap();
    gate.open();
    // The tree the window read when the daemon queued it may still list it: the read that
    // follows the withdrawal (`delivery.withdrawn` is a tree event) takes it out.
    until(&shell, cx, "it leaves", |cx| {
        reply.read_with(cx, |r, _| r.pending().is_empty()) && queued(&page, cx).is_empty()
    });
    at_rest(cx);
    assert!(!has(queued_id(&format!("pending-{key}")), cx));
    assert!(queued(&page, cx).is_empty());
}

// #505: earlier events load on their own when the user scrolls near the top.

/// The lead's long talk, read from its last page: earlier pages remain.
fn talked<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    String,
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
) {
    let lead = lead(user);
    worked(user, &lead, "talk at length");
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
        row_of(&page, cx, |b| is_text(b, "Finally.")).is_some()
    });
    settle(cx);
    assert!(page.read_with(cx, |p, cx| p.conversation().read(cx).has_earlier()));
    (lead, shell, page, cx)
}

fn pages(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> usize {
    page.read_with(cx, |page, _| page.pages())
}

/// One notch of the wheel, up by `by` pixels, then a frame: nothing is polled, so what the
/// window asked of the daemon is not answered yet.
fn wheel(agent: &str, by: f32, cx: &mut VisualTestContext) {
    let middle = cx.update(|window, _| {
        window
            .find(format!("conversation-{agent}"))
            .bounds()
            .center()
    });
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: middle,
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(by))),
        ..Default::default()
    });
    frame(cx);
}

/// Scrolls up, notch by notch, until a page is asked.
fn scroll_until_asked(
    agent: &str,
    page: &Entity<SessionPage>,
    asked: usize,
    cx: &mut VisualTestContext,
) {
    for _ in 0..100 {
        if pages(page, cx) > asked {
            return;
        }
        wheel(agent, 600., cx);
    }
    panic!("scrolling up never asked for a page (still {asked})");
}

/// Answers what the window asked: the page is read and drawn.
fn page_arrives(shell: &Entity<Shell>, page: &Entity<SessionPage>, cx: &mut VisualTestContext) {
    let before = blocks(page, cx).len();
    until(shell, cx, "the earlier page", |cx| {
        blocks(page, cx).len() != before
    });
    settle(cx);
}

#[gpui_kit::test]
fn scrolling_near_the_top_loads_earlier_events(cx: &mut TestAppContext) {
    let user = user();
    let (lead, shell, page, cx) = talked(&user, cx);
    let before = blocks(&page, cx).len();
    assert_eq!(pages(&page, cx), 1);

    // Far from the top, nothing is asked.
    wheel(&lead, 400., cx);
    wheel(&lead, 400., cx);
    assert_eq!(pages(&page, cx), 1, "far from the top");

    scroll_until_asked(&lead, &page, 1, cx);
    page_arrives(&shell, &page, cx);
    assert!(blocks(&page, cx).len() > before, "events came in above");
    assert_eq!(pages(&page, cx), 2);
    at_rest(cx);
}

#[gpui_kit::test]
fn loading_earlier_events_keeps_what_is_read_in_place(cx: &mut TestAppContext) {
    let user = user();
    let (lead, shell, page, cx) = talked(&user, cx);
    scroll_until_asked(&lead, &page, 1, cx);
    settle(cx);

    // The first paragraph drawn in the conversation, and where it is.
    let (read, top) = cx.update(|window, cx| {
        let area = window.find(format!("conversation-{lead}")).bounds();
        let shown = page.read(cx).conversation().read(cx).blocks().to_vec();
        shown
            .iter()
            .enumerate()
            .filter(
                |(_, block)| matches!(block, Block::Text { text } if text.starts_with("Paragraph")),
            )
            .find_map(|(at, block)| {
                let bounds = window.try_find(format!("row-{at}-text"))?.bounds();
                (bounds.top() >= area.top() && bounds.bottom() <= area.bottom())
                    .then(|| (block.clone(), bounds.top()))
            })
            .expect("a paragraph in sight")
    });
    // From the first frame of the new rows, not once their entrances ended: what is read does
    // not enter again, nor rise.
    let before = blocks(&page, cx).len();
    until(&shell, cx, "the earlier page", |cx| {
        blocks(&page, cx).len() != before
    });
    let at_top = |cx: &mut VisualTestContext| {
        let at = row_of(&page, cx, |block| *block == read).expect("still there");
        let now = cx.update(|window, _| window.find(format!("row-{at}-text")).bounds().top());
        f32::from(now)
    };
    assert!(
        (at_top(cx) - f32::from(top)).abs() <= 1.0,
        "it was at {top:?}, it is at {}",
        at_top(cx)
    );
    settle(cx);
    assert!(
        (at_top(cx) - f32::from(top)).abs() <= 1.0,
        "it was at {top:?}, it settled at {}",
        at_top(cx)
    );
}

#[gpui_kit::test]
fn only_one_page_is_asked_at_a_time(cx: &mut TestAppContext) {
    let user = user();
    let (lead, shell, page, cx) = talked(&user, cx);
    scroll_until_asked(&lead, &page, 1, cx);
    for _ in 0..6 {
        wheel(&lead, 600., cx);
    }
    assert_eq!(pages(&page, cx), 2, "the first is not answered yet");

    page_arrives(&shell, &page, cx);
    scroll_until_asked(&lead, &page, 2, cx);
    for _ in 0..6 {
        wheel(&lead, 600., cx);
    }
    assert_eq!(pages(&page, cx), 3, "the second is not answered yet either");
}

/// Scrolls up and reads the pages as they are asked, until none remains.
fn load_all(
    agent: &str,
    shell: &Entity<Shell>,
    page: &Entity<SessionPage>,
    cx: &mut VisualTestContext,
) {
    for _ in 0..10 {
        if !page.read_with(cx, |p, cx| p.conversation().read(cx).has_earlier()) {
            return;
        }
        let asked = pages(page, cx);
        scroll_until_asked(agent, page, asked, cx);
        page_arrives(shell, page, cx);
    }
    panic!("the history never ended");
}

#[gpui_kit::test]
fn nothing_is_asked_once_everything_is_loaded(cx: &mut TestAppContext) {
    let user = user();
    let (lead, shell, page, cx) = talked(&user, cx);
    load_all(&lead, &shell, &page, cx);
    let all = pages(&page, cx);
    assert!(all >= 3, "the talk takes several pages: {all}");
    at_rest(cx);

    for _ in 0..40 {
        wheel(&lead, 2000., cx);
    }
    settle(cx);
    assert_eq!(pages(&page, cx), all, "nothing more is asked");
    assert!(!has("show-earlier", cx), "no button");
    assert!(!has("earlier-loading", cx), "no line");
    assert!(blocks(&page, cx).len() > 100);
    at_rest(cx);
}

#[gpui_kit::test]
fn opening_a_long_conversation_does_not_load_earlier_events(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _shell, page, cx) = talked(&user, cx);
    settle(cx);
    assert_eq!(pages(&page, cx), 1, "opened at its end");

    // A jump back to the end is no scroll toward the top.
    wheel(&lead, 400., cx);
    cx.update(|window, cx| window.click("conversation-to-end", cx));
    at_rest(cx);
    assert_eq!(pages(&page, cx), 1);
    assert!(following(&page, cx));
}

#[gpui_kit::test]
fn a_short_loaded_part_waits_for_the_user_to_scroll(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, page, cx) = past_a_page_read(&user, cx);
    settle(cx);
    assert_eq!(
        pages(&page, cx),
        1,
        "everything shown is in sight, still nothing is asked"
    );
}

#[gpui_kit::test]
fn a_failed_load_shows_show_earlier_with_the_reason(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let user = user();
    let lead = lead(&user);
    worked(&user, &lead, "talk at length");
    eventually("the long turn ends", || {
        user.call("agent.show", json!({"agent": lead, "before": u64::MAX}))
            .unwrap()["events"]
            .as_array()
            .unwrap()
            .last()
            .is_some_and(|e| e["name"] == "agent.turn_ended")
    });
    let (down, real) = (Arc::new(AtomicBool::new(false)), user.connector());
    let connect: Connect = {
        let down = down.clone();
        Arc::new(move || match down.load(Ordering::SeqCst) {
            true => Err(std::io::Error::other("the daemon is not answering")),
            false => real(),
        })
    };
    let (shell, page, cx) = open_session_through(&user, &lead, connect, cx);
    until(&shell, cx, "the last page is read", |cx| {
        row_of(&page, cx, |b| is_text(b, "Finally.")).is_some()
    });
    settle(cx);

    down.store(true, Ordering::SeqCst);
    scroll_until_asked(&lead, &page, 1, cx);
    for _ in 0..4 {
        wheel(&lead, 2000., cx);
    }
    until(&shell, cx, "the load failed", |cx| {
        has("show-earlier-failed", cx)
    });
    settle(cx);
    let said = label("show-earlier-reason", cx).unwrap();
    assert!(said.contains("the daemon is not answering"), "{said}");
    assert!(has("show-earlier", cx), "the button stays");
    assert!(!has("earlier-loading", cx));

    // Scrolling does not try again by itself.
    for _ in 0..4 {
        wheel(&lead, 2000., cx);
    }
    assert_eq!(pages(&page, cx), 2);

    // The button does, and the failure goes.
    down.store(false, Ordering::SeqCst);
    let before = blocks(&page, cx).len();
    cx.update(|window, cx| window.click("show-earlier", cx));
    until(&shell, cx, "the earlier page", |cx| {
        blocks(&page, cx).len() > before
    });
    settle(cx);
    assert!(!has("show-earlier-failed", cx));
    assert_eq!(pages(&page, cx), 2, "the same page, asked again");
}

#[gpui_kit::test]
fn the_loading_line_unfolds_and_folds_away(cx: &mut TestAppContext) {
    let user = user();
    let (shell, page, cx) = past_a_page_read(&user, cx);
    let lead = page.read_with(cx, |p, _| p.id().to_owned());
    settle(cx);
    assert!(has("show-earlier", cx), "the button waits at the top");
    assert!(!has("earlier-loading", cx));

    // The wheel at the top asks the page: the line unfolds as the button folds away.
    wheel(&lead, 400., cx);
    let mut heights = Vec::new();
    for _ in 0..30 {
        frame(cx);
        heights.push(cx.update(|window, _| {
            window
                .try_find("earlier-loading")
                .map(|l| l.bounds().size.height)
        }));
    }
    let shown: Vec<f32> = heights.iter().flatten().map(|h| f32::from(*h)).collect();
    assert_eq!(
        label("earlier-loading-line", cx).as_deref(),
        Some("Loading earlier…")
    );
    assert!(
        shown.first() < shown.last() && shown.windows(2).all(|w| w[0] <= w[1]),
        "it unfolds: {shown:?}"
    );
    assert!(!has("show-earlier", cx), "the button gave way");

    // The page arrives: the line folds away with nothing left in its place.
    until(&shell, cx, "the earlier page", |cx| {
        !page.read_with(cx, |p, cx| p.conversation().read(cx).has_earlier())
    });
    let mut folding = Vec::new();
    for _ in 0..30 {
        frame(cx);
        folding.push(cx.update(|window, _| {
            window
                .try_find("earlier-loading")
                .map(|l| f32::from(l.bounds().size.height))
        }));
    }
    let folding: Vec<f32> = folding.into_iter().flatten().collect();
    assert!(
        folding.windows(2).all(|w| w[0] >= w[1]),
        "it folds away: {folding:?}"
    );
    at_rest(cx);
    assert!(!has("earlier-loading", cx));
    assert!(!has("show-earlier", cx));
}
