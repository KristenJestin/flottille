//! #1002 in the window: while it is focused, what the daemon would have told the desktop is a
//! toast of the kit at the corner (the same title and excerpt, Allow and Deny for a request, a
//! click opening the session), unless the session is the one open; the window tells the daemon
//! its presence. On GPUI's test platform in front of a real daemon, a private bus with a fake
//! notification server, a fake audio sink. No window opens on a screen.

use crate::common;
use crate::window_notifications::{
    eventually, has, held_number, label, project, rect, session, settle, until, user, window,
};

use common::desktop::{self, Bus, Notifications};
use serde_json::{Value, json};

use gpui_kit::TestAppContext;

fn pending(user: &common::User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// What the daemon knows of the window: `{"focused", "session"}`, or null without one.
fn presence(user: &common::User) -> Value {
    user.call("notifications.status", json!({})).unwrap()["window"].clone()
}

fn frames_pending(cx: &mut gpui_kit::VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

#[gpui_kit::test]
fn the_window_tells_the_daemon_when_it_is_focused_and_what_it_shows(cx: &mut TestAppContext) {
    let user = user();
    let Some(_bus) = Bus::start(&user) else {
        return;
    };
    project(&user);
    let (lead, _) = session(&user, "hello");
    let (shell, cx) = window(&user, cx);
    cx.update(|window, _| window.activate_window());
    until(&shell, cx, "the window says it is focused", |_| {
        presence(&user)["focused"] == true
    });

    cx.deactivate_window();

    until(&shell, cx, "the window says it lost the focus", |_| {
        presence(&user)["focused"] == false
    });
    cx.update(|window, _| window.activate_window());
    crate::window_notifications::open_session(&shell, &lead, cx);
    until(&shell, cx, "the window says which session is open", |_| {
        presence(&user)["session"] == lead
    });
}

#[gpui_kit::test]
fn with_the_window_focused_the_notification_is_a_toast_not_a_system_one(cx: &mut TestAppContext) {
    let user = user();
    let Some(bus) = Bus::start(&user) else {
        return;
    };
    let server = Notifications::start(&bus, &["actions", "body"]);
    let played = desktop::with_fake_audio(&user);
    project(&user);
    let (_, mcp) = session(&user, "hello");
    let (shell, cx) = window(&user, cx);
    cx.update(|window, _| window.activate_window());
    until(&shell, cx, "the window says it is focused", |_| {
        presence(&user)["focused"] == true
    });
    settle(cx);

    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the toast", |cx| has("toast-1", cx));

    // It rises into place with the title and the excerpt of the notification.
    let first = rect("toast-1", cx);
    settle(cx);
    let rested = rect("toast-1", cx);
    assert!(
        first.origin.y > rested.origin.y,
        "it rises: {first:?} to {rested:?}"
    );
    assert_eq!(
        label("toast-1-title", cx).as_deref(),
        Some(format!("Request #{number} waits for you").as_str())
    );
    assert_eq!(label("toast-1-detail", cx).as_deref(), Some("Read .env"));
    assert!(has("toast-1-action", cx) && has("toast-1-action-2", cx));
    // The desktop is told nothing, and the sound plays, once, in the daemon.
    eventually("the sound", || desktop::played(&played) == ["needs-you"]);
    assert!(server.notified().is_empty(), "{:?}", server.notified());

    // Allow answers the request, and the toast folds away; nothing is drawn at rest.
    cx.update(|window, cx| window.click("toast-1-action", cx));
    until(&shell, cx, "the request was decided", |_| {
        pending(&user).is_empty()
    });
    until(&shell, cx, "the toast left", |cx| !has("toast-1", cx));
    settle(cx);
    assert_eq!(frames_pending(cx), 0, "at rest after the toast left");
    assert!(server.notified().is_empty());
}

#[gpui_kit::test]
fn a_toast_opens_its_session_on_a_click(cx: &mut TestAppContext) {
    let user = user();
    let Some(_bus) = Bus::start(&user) else {
        return;
    };
    project(&user);
    let (_, mcp) = session(&user, "hello");
    let (shell, cx) = window(&user, cx);
    cx.update(|window, _| window.activate_window());
    until(&shell, cx, "the window says it is focused", |_| {
        presence(&user)["focused"] == true
    });
    settle(cx);

    held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the toast", |cx| has("toast-1-open", cx));
    cx.update(|window, cx| window.click("toast-1-open", cx));

    until(&shell, cx, "the session's page opens", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    until(&shell, cx, "the toast left", |cx| !has("toast-1", cx));
}

#[gpui_kit::test]
fn the_toast_of_a_request_answered_elsewhere_is_withdrawn(cx: &mut TestAppContext) {
    let user = user();
    let Some(_bus) = Bus::start(&user) else {
        return;
    };
    project(&user);
    let (_, mcp) = session(&user, "hello");
    let (shell, cx) = window(&user, cx);
    cx.update(|window, _| window.activate_window());
    until(&shell, cx, "the window says it is focused", |_| {
        presence(&user)["focused"] == true
    });
    settle(cx);
    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the toast", |cx| has("toast-1", cx));

    user.call("approval.deny", json!({"number": number}))
        .unwrap();

    until(&shell, cx, "the toast folded away", |cx| {
        !has("toast-1", cx)
    });
    settle(cx);
    assert_eq!(frames_pending(cx), 0);
}

#[gpui_kit::test]
fn nothing_is_toasted_for_the_session_the_window_shows(cx: &mut TestAppContext) {
    let user = user();
    let Some(_bus) = Bus::start(&user) else {
        return;
    };
    project(&user);
    let (open, open_mcp) = session(&user, "hello");
    let (_, other_mcp) = session(&user, "hello again");
    let (shell, cx) = window(&user, cx);
    cx.update(|window, _| window.activate_window());
    crate::window_notifications::open_session(&shell, &open, cx);
    until(&shell, cx, "the window says which session is open", |_| {
        presence(&user)["session"] == open
    });

    held_number(open_mcp.call("fs_read", json!({"path": ".env"})));
    let other = held_number(other_mcp.call("fs_read", json!({"path": ".env"})));

    // The other's toast is the first the window drew: the open session's rang nothing.
    until(&shell, cx, "the toast", |cx| has("toast-1", cx));
    assert_eq!(
        label("toast-1-title", cx).as_deref(),
        Some(format!("Request #{other} waits for you").as_str())
    );
    assert!(!has("toast-2", cx));
}
