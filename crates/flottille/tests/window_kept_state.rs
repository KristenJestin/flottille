//! The scenarios of #1070: what the user leaves in the window (the text and the pictures of a
//! composer, what the sidebar has folded) survives a close of the window, a second window and a
//! restart of the daemon, because the daemon keeps it. On GPUI's test platform in front of a real
//! daemon whose agents are the fake ACP agent, with a throwaway user: no window opens on a
//! screen, and the 500 ms of the debounce are the test platform's clock, never a sleep.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::tree::{ended_id, project_id, session_id};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Image, ImageFormat, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A valid 8 × 8 PNG, so that the window can draw its thumbnail.
const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR4nGM4YWODFTEMLQkAZZlQAVIPr1MAAAAASUVORK5CYII=";

/// The debounce of a draft: the daemon is written to this long after the last change.
const DEBOUNCE: Duration = Duration::from_millis(500);

fn png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(PNG_BASE64)
        .unwrap()
}

/// A user whose fake agent takes images, answers at once and, asked to begin the docs session,
/// launches a session it leads.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true, "turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
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

/// Starts a session and waits until its first turn ended.
fn start(user: &User, project: &str, message: &str) -> String {
    let id = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

/// A lead the user started and the session it leads.
fn lead_and_led(user: &User, project: &str) -> (String, String) {
    let lead = user
        .call(
            "agent.start",
            json!({"project": project, "message": "begin the docs session"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the launch is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let number = user.call("approval.list", json!({})).unwrap()["requests"][0]["number"]
        .as_u64()
        .unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the session exists", || {
        agents(user).iter().any(|a| a["led_by"] == lead.as_str())
    });
    let led = agents(user)
        .into_iter()
        .find(|a| a["led_by"] == lead.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, led)
}

/// What the daemon keeps of a composer's draft: its text and how many pictures, `None` for none.
fn kept(user: &User, kind: &str, target: &str) -> Option<(String, usize)> {
    let answer = user
        .call("draft.get", json!({"kind": kind, "target": target}))
        .unwrap();
    let draft = answer["draft"].as_object()?;
    Some((
        draft["text"].as_str().unwrap().to_owned(),
        draft["images"].as_array().map_or(0, Vec::len),
    ))
}

/// What the daemon keeps of the sidebar.
fn sidebar(user: &User) -> Value {
    user.call("window.get", json!({})).unwrap()["state"]["sidebar"].clone()
}

/// A window on the user's daemon: a second one is a second app, as a second window of the machine
/// is.
fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

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

/// The clock and the frames of a window at rest, to the last frame of any motion.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Opens a session from its row and waits for its composer, and for what the daemon kept of it.
fn open(id: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the session's row", |cx| {
        cx.update(|window, _| window.try_find(session_id(id)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the session's composer is made", |cx| {
        shell.read_with(cx, |s, _| s.reply_of(id).is_some())
    });
    settle(cx);
}

/// A window on the project's page, its composer made.
fn open_project(name: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the project's row", |cx| {
        cx.update(|window, _| window.try_find(project_id(name)).is_some())
    });
    cx.update(|window, cx| window.click(project_id(name), cx));
    until(shell, cx, "the project's composer is made", |cx| {
        shell.read_with(cx, |s, _| s.composer_of(name).is_some())
    });
    settle(cx);
}

fn type_in_reply(id: &str, text: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    open(id, shell, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input(text);
}

fn reply_text(id: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) -> String {
    let reply = shell.read_with(cx, |s, _| s.reply_of(id).cloned().expect("a composer"));
    reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string())
}

/// The window is closed: what it holds is written at once.
fn close(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update(cx, |shell, cx| shell.closing(cx));
}

/// The 500 ms the window waits after the last change, on the test platform's clock.
fn debounced(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(DEBOUNCE + Duration::from_millis(1));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_draft_survives_closing_and_reopening_the_window(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");

    let (shell, cx) = window(&user, first);
    type_in_reply(&id, "half an answer", &shell, cx);
    close(&shell, cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id).is_some()
    });
    assert_eq!(
        kept(&user, "session", &id),
        Some(("half an answer".to_owned(), 0))
    );

    let (shell, cx) = window(&user, second);
    open(&id, &shell, cx);
    until(&shell, cx, "the draft is back in its composer", |cx| {
        reply_text(&id, &shell, cx) == "half an answer"
    });
}

#[gpui_kit::test]
fn a_draft_is_written_500_ms_after_the_last_change(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    open(&id, &shell, cx);
    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    let sent = |reply: &Entity<_>, cx: &mut VisualTestContext| {
        reply.read_with(cx, |r: &flottille_window::session::Reply, _| {
            r.drafts_sent()
        })
    };

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("one");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(sent(&reply, cx), 0, "nothing is written before the 500 ms");

    // Another change starts the wait again: 400 ms after it is not 500 ms after the last one.
    cx.simulate_input(" two");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(sent(&reply, cx), 0, "a change restarts the wait");

    debounced(cx);
    assert_eq!(
        sent(&reply, cx),
        1,
        "one write, 500 ms after the last change"
    );
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id) == Some(("one two".to_owned(), 0))
    });

    // Nothing changed: nothing is written again.
    debounced(cx);
    assert_eq!(sent(&reply, cx), 1);
}

#[gpui_kit::test]
fn a_draft_with_a_picture_comes_back_with_its_picture(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");

    let (shell, cx) = window(&user, first);
    type_in_reply(&id, "look at this ", &shell, cx);
    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    reply.update(cx, |reply, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png()), cx)
    });
    settle(cx);
    close(&shell, cx);
    eventually("the daemon keeps the picture", || {
        kept(&user, "session", &id).is_some_and(|(_, pictures)| pictures == 1)
    });
    // The picture is a file of the daemon's state folder, private, and not the agent's.
    let folder = user.path("state/flottille/drafts");
    assert_eq!(common::mode(folder.clone()) & 0o777, 0o700);

    let (shell, cx) = window(&user, second);
    open(&id, &shell, cx);
    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    until(&shell, cx, "the picture is back", |cx| {
        reply.read_with(cx, |r, _| r.attachments().len() == 1)
    });
    assert!(
        reply_text(&id, &shell, cx).starts_with("look at this "),
        "its text comes back with it"
    );
    assert_eq!(
        reply.read_with(cx, |r, _| r.attachments()[0].bytes.clone()),
        png()
    );
}

#[gpui_kit::test]
fn the_first_message_of_a_new_session_survives_the_window(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    project(&user, "other");

    let (shell, cx) = window(&user, first);
    open_project("app", &shell, cx);
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input("fix the login page");
    close(&shell, cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "project", "app").is_some()
    });
    assert_eq!(
        kept(&user, "project", "app"),
        Some(("fix the login page".to_owned(), 0))
    );
    assert_eq!(kept(&user, "project", "other"), None, "it is app's alone");

    let (shell, cx) = window(&user, second);
    open_project("app", &shell, cx);
    let composer = shell.read_with(cx, |s, _| s.composer_of("app").cloned().unwrap());
    until(&shell, cx, "the draft is back in its composer", |cx| {
        composer.read_with(cx, |c, cx| {
            c.field().read(cx).value() == "fix the login page"
        })
    });
}

#[gpui_kit::test]
fn sending_the_message_removes_its_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    type_in_reply(&id, "ready to go", &shell, cx);
    debounced(cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id).is_some()
    });

    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the message is sent", |cx| {
        reply.read_with(cx, |r, cx| {
            !r.is_sending() && r.field().read(cx).value().is_empty()
        })
    });
    // At once, not 500 ms later: a close right after sending must not bring the sent text back.
    eventually("the daemon forgets the draft", || {
        kept(&user, "session", &id).is_none()
    });
}

#[gpui_kit::test]
fn emptying_the_composer_removes_its_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    type_in_reply(&id, "oops", &shell, cx);
    debounced(cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id).is_some()
    });

    for _ in 0..4 {
        cx.simulate_keystrokes("backspace");
    }
    debounced(cx);
    eventually("the daemon forgets the draft", || {
        kept(&user, "session", &id).is_none()
    });
}

#[gpui_kit::test]
fn a_draft_survives_a_restart_of_the_daemon(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "kept across a restart"}),
    )
    .unwrap();

    user.call("daemon.stop", json!({})).unwrap();
    eventually("the daemon is gone", || {
        !user.path("runtime/flottille/daemon.sock").exists()
    });

    // The window starts the daemon again, which reads its store.
    let (shell, cx) = window(&user, cx);
    open(&id, &shell, cx);
    until(&shell, cx, "the draft is back in its composer", |cx| {
        reply_text(&id, &shell, cx) == "kept across a restart"
    });
}

#[gpui_kit::test]
fn a_draft_is_never_logged_nor_in_an_event(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let watch = user.watch(&[]);
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "my private words"}),
    )
    .unwrap();
    user.call("draft.get", json!({"kind": "session", "target": id}))
        .unwrap();
    // An event that follows the write is read after it: the draft's would have come before.
    user.call("project.list", json!({})).unwrap();
    let seen = watch.within(Duration::from_millis(500));
    assert!(
        !serde_json::to_string(&seen)
            .unwrap()
            .contains("my private words"),
        "no event carries a draft"
    );
    let log = std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap_or_default();
    assert!(!log.contains("my private words"), "the log holds no draft");
    let _ = cx;
}

#[gpui_kit::test]
fn a_session_ended_for_good_forgets_its_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "to be forgotten",
               "images": [{"mime": "image/png", "data": PNG_BASE64}]}),
    )
    .unwrap();
    let folder = user.path("state/flottille/drafts");
    assert!(
        std::fs::read_dir(&folder).unwrap().next().is_some(),
        "the picture is a file of the daemon's state folder"
    );

    user.call("project.remove", json!({"name": "app"})).unwrap();
    eventually("the draft goes with the project's sessions", || {
        kept(&user, "session", &id).is_none()
    });
    assert!(
        std::fs::read_dir(&folder).unwrap().next().is_none(),
        "the picture goes with its draft"
    );
    let _ = cx;
}

#[gpui_kit::test]
fn a_folded_project_stays_folded_after_reopening(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    start(&user, "app", "first");

    let (shell, cx) = window(&user, first);
    until(&shell, cx, "the project's row", |cx| {
        cx.update(|window, _| window.try_find(project_id("app")).is_some())
    });
    cx.update(|window, cx| window.click(toggle_id(project_id("app")), cx));
    settle(cx);
    assert!(!shell.read_with(cx, |s, cx| s.project("app").unwrap().read(cx).is_open()));
    eventually("the daemon keeps the fold", || {
        sidebar(&user)["projects"]["app"] == false
    });
    assert_eq!(sidebar(&user)["projects"]["other"], true);

    let (shell, cx) = window(&user, second);
    until(&shell, cx, "the project's row", |cx| {
        cx.update(|window, _| window.try_find(project_id("app")).is_some())
    });
    let open = |name: &str, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| s.project(name).unwrap().read(cx).is_open())
    };
    assert!(!open("app", cx), "the folded project is folded");
    assert!(open("other", cx), "the other keeps its default");
}

#[gpui_kit::test]
fn a_folded_lead_and_the_ended_group_stay_as_they_were(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let (lead, led) = lead_and_led(&user, "app");
    let gone = start(&user, "app", "an ordinary one");
    user.call("agent.stop", json!({"agent": gone})).unwrap();
    eventually("the session ended", || {
        user.call("agent.show", json!({"agent": gone})).unwrap()["agent"]["state"] == "ended"
    });

    let (shell, cx) = window(&user, first);
    until(&shell, cx, "the led row and the Ended group", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&led)).is_some() && window.try_find(ended_id()).is_some()
        })
    });
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    eventually("the daemon keeps both", || {
        let state = sidebar(&user);
        state["leads"][&lead] == false && state["ended"] == true
    });

    let (shell, cx) = window(&user, second);
    until(&shell, cx, "the lead's row and the Ended group", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&lead)).is_some() && window.try_find(ended_id()).is_some()
        })
    });
    assert!(
        !shell.read_with(cx, |s, cx| s.session(&lead).unwrap().read(cx).is_open()),
        "the lead's led rows are folded"
    );
    assert!(
        cx.update(|window, _| window.try_find(session_id(&led)).is_none()),
        "the led row is not drawn"
    );
    assert!(shell.read_with(cx, |s, cx| s.ended().unwrap().read(cx).is_open()));
}

#[gpui_kit::test]
fn a_project_removed_forgets_its_fold(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both projects' rows", |cx| {
        cx.update(|window, _| {
            window.try_find(project_id("app")).is_some()
                && window.try_find(project_id("other")).is_some()
        })
    });
    cx.update(|window, cx| window.click(toggle_id(project_id("other")), cx));
    settle(cx);
    eventually("the daemon keeps the fold", || {
        sidebar(&user)["projects"]["other"] == false
    });

    user.call("project.remove", json!({"name": "other"}))
        .unwrap();
    until(&shell, cx, "the project is gone from the window", |cx| {
        cx.update(|window, _| window.try_find(project_id("other")).is_none())
    });
    eventually("the daemon forgets its fold", || {
        sidebar(&user)["projects"].get("other").is_none()
    });
    assert_eq!(sidebar(&user)["projects"]["app"], true);
}

#[gpui_kit::test]
fn the_restored_sidebar_draws_no_animation_at_opening(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let (lead, led) = lead_and_led(&user, "app");

    let (shell, cx) = window(&user, first);
    until(&shell, cx, "the led row", |cx| {
        cx.update(|window, _| window.try_find(session_id(&led)).is_some())
    });
    cx.update(|window, cx| window.click(toggle_id(project_id("app")), cx));
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    eventually("the daemon keeps the folds", || {
        let state = sidebar(&user);
        state["projects"]["app"] == false && state["leads"][&lead] == false
    });

    // The first frame the folded project is drawn in is its folded one: no frame is spent
    // closing it from the default, and the clock is never advanced meanwhile.
    let (shell, cx) = window(&user, second);
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if let Some(fold) = cx.update(|window, _| window.try_find("fold-project-app")) {
            assert_eq!(fold.expanded(), Some(false), "folded from its first frame");
            assert_eq!(
                cx.update(|window, _| window.find("fold-project-app").bounds().size.height),
                px(0.),
                "its height is never seen open"
            );
            break;
        }
        assert!(Instant::now() < deadline, "the project is never drawn");
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "it asks for no frame to fold"
    );
}
