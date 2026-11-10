//! The scenarios of #506: a session whose composer holds a draft the user has left is marked by a
//! pencil in the side list, and the mark goes once the draft is sent or cleared. On GPUI's test
//! platform in front of a real daemon whose agents are the fake ACP agent, with a throwaway
//! user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session::Reply;
use flottille_window::tree::{Trailing, session_draft_id, session_id};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Image, ImageFormat, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A valid 8 × 8 PNG, so that the window can draw its thumbnail.
const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR4nGM4YWODFTEMLQkAZZlQAVIPr1MAAAAASUVORK5CYII=";

fn png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(PNG_BASE64)
        .unwrap()
}

/// A user whose fake agent takes images and answers at once.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true, "turns": []}));
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
    let answer = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
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

/// The clock and the frames of a window at rest, to the last frame of any motion.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// A window on the user's two sessions, the first one open, its composer made.
fn two_sessions<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, String, String, &'a mut VisualTestContext) {
    project(user, "app");
    let (a, b) = (start(user, "app", "first"), start(user, "app", "second"));
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    until(&shell, cx, "both sessions are in the tree", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&a)).is_some() && window.try_find(session_id(&b)).is_some()
        })
    });
    settle(cx);
    open(&a, &shell, cx);
    (shell, a, b, cx)
}

/// Opens a session from its row and waits for its composer.
fn open(id: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the session's composer is made", |cx| {
        shell.read_with(cx, |s, _| s.reply_of(id).is_some())
    });
    settle(cx);
}

fn reply_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Entity<Reply> {
    shell.read_with(cx, |s, _| s.reply_of(id).cloned().expect("a composer"))
}

fn type_in(id: &str, shell: &Entity<Shell>, text: &str, cx: &mut VisualTestContext) {
    open(id, shell, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input(text);
    settle(cx);
}

/// Whether the side list draws the pencil on the row of `id`.
fn marked(id: &str, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| window.try_find(session_draft_id(id)).is_some())
}

/// The words the pencil says.
fn said(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(session_draft_id(id))
            .and_then(|mark| mark.label().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn a_session_left_with_unsent_text_is_marked_in_the_side_list(cx: &mut TestAppContext) {
    let user = user();
    let (shell, a, b, cx) = two_sessions(&user, cx);
    type_in(&a, &shell, "half an answer", cx);
    open(&b, &shell, cx);

    assert!(marked(&a, cx), "the session left with a draft is marked");
    assert_eq!(said(&a, cx).as_deref(), Some("Draft not sent"));
    assert!(!marked(&b, cx), "the other one holds nothing");
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "the mark at rest asks for no frame"
    );
    let trailing = shell.read_with(cx, |s, cx| s.session(&a).unwrap().read(cx).trailing());
    assert_eq!(
        trailing,
        Trailing::Ago,
        "the row's own mark, here the time, stays beside it"
    );

    // Back in it, the draft is where it was, and the mark goes: it fades out, it does not jump.
    cx.update(|window, cx| window.click(session_id(&a), cx));
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(marked(&a, cx), "still drawn while it fades away");
    settle(cx);
    assert!(!marked(&a, cx));
    assert_eq!(
        reply_of(&shell, &a, cx).read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "half an answer"
    );
}

#[gpui_kit::test]
fn a_session_left_with_an_unsent_image_is_marked(cx: &mut TestAppContext) {
    let user = user();
    let (shell, a, b, cx) = two_sessions(&user, cx);
    let reply = reply_of(&shell, &a, cx);
    reply.update(cx, |reply, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png()), cx)
    });
    settle(cx);
    assert!(!marked(&a, cx), "not while it is the open session");
    open(&b, &shell, cx);

    assert!(marked(&a, cx), "a picture alone is a draft");
    assert!(!marked(&b, cx));
}

#[gpui_kit::test]
fn the_mark_goes_once_the_draft_is_sent(cx: &mut TestAppContext) {
    let user = user();
    let (shell, a, b, cx) = two_sessions(&user, cx);
    type_in(&a, &shell, "ready to go", cx);
    open(&b, &shell, cx);
    assert!(marked(&a, cx));

    open(&a, &shell, cx);
    let reply = reply_of(&shell, &a, cx);
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    // Gone from the page before the daemon answered: the draft is still the field's.
    open(&b, &shell, cx);
    until(&shell, cx, "the message is sent", |cx| {
        reply.read_with(cx, |r, cx| {
            !r.is_sending() && r.field().read(cx).value().is_empty()
        })
    });
    settle(cx);
    assert!(!marked(&a, cx), "nothing is left to send");
}

#[gpui_kit::test]
fn the_mark_goes_once_the_draft_is_cleared(cx: &mut TestAppContext) {
    let user = user();
    let (shell, a, b, cx) = two_sessions(&user, cx);
    type_in(&a, &shell, "oops", cx);
    reply_of(&shell, &a, cx).update(cx, |reply, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png()), cx)
    });
    open(&b, &shell, cx);
    assert!(marked(&a, cx));

    // The picture removed: the text still holds the mark.
    open(&a, &shell, cx);
    reply_of(&shell, &a, cx).update(cx, |reply, cx| reply.detach(0, cx));
    open(&b, &shell, cx);
    assert!(marked(&a, cx), "the text is still unsent");

    // The text erased: nothing holds it.
    open(&a, &shell, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    for _ in 0..4 {
        cx.simulate_keystrokes("backspace");
    }
    open(&b, &shell, cx);
    assert!(!marked(&a, cx), "the draft is cleared");
}

#[gpui_kit::test]
fn the_session_being_typed_in_is_not_marked(cx: &mut TestAppContext) {
    let user = user();
    let (shell, a, b, cx) = two_sessions(&user, cx);
    type_in(&a, &shell, "typing right now", cx);
    settle(cx);
    assert!(!marked(&a, cx), "it is the open session");
    assert!(!marked(&b, cx));

    // A page that is none of the sessions' counts as having left it.
    cx.update(|window, cx| window.click("new-session", cx));
    settle(cx);
    assert!(marked(&a, cx), "left for a project's page");
}
