//! The notification centre in the window (#1001): the bell's panel shows what the daemon keeps,
//! grouped by session, the unread first; a second window shows the same, and the state read or
//! answered anywhere reaches it. On GPUI's test platform in front of a real daemon whose agent is
//! the fake ACP agent, with a throwaway user. No window opens on a screen.

use crate::common;
use crate::window_notifications::{
    BRIEF, bell_count, bell_said, eventually, has, held_number, label, lead, open_session, session,
    settle, until, user, window,
};

use common::User;
use flottille_window::Shell;
use flottille_window::shell::{bell_group_id, bell_group_title_id};
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

/// The request the call `.env` of a session's agent held.
fn hold(mcp: &common::mcp::Mcp) -> u64 {
    held_number(mcp.call("fs_read", json!({"path": ".env"})))
}

fn rows(user: &User) -> Vec<Value> {
    user.call("notifications.list", json!({})).unwrap()["notifications"]
        .as_array()
        .unwrap()
        .clone()
}

fn counts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (usize, usize) {
    shell.read_with(cx, |s, cx| s.top().read(cx).counts())
}

#[gpui_kit::test]
fn a_notification_survives_the_window_closing(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    let (_, mcp) = lead(&user);
    let number = hold(&mcp);

    let (shell, cx) = window(&user, first);
    until(&shell, cx, "the bell counts the request", |cx| {
        bell_count(&shell, cx) == 1
    });
    shell.update(cx, |shell, cx| shell.closing(cx));

    // A window opened later, on the same daemon, has it.
    let (shell, cx) = window(&user, second);
    until(&shell, cx, "the new window's bell counts it", |cx| {
        bell_count(&shell, cx) == 1
    });
    assert_eq!(
        bell_said(&shell, cx),
        [(
            format!("Request #{number} waits for you"),
            "Read .env".to_owned(),
            BRIEF.to_owned()
        )]
    );
    assert_eq!(counts(&shell, cx), (1, 0), "a request needs the user");
}

#[gpui_kit::test]
fn opening_the_session_marks_its_notifications_read(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let number = hold(&mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the bell counts the request", |cx| {
        bell_count(&shell, cx) == 1
    });
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(&lead), cx)
    });

    cx.update(|window, cx| window.click(session_id(&lead), cx));

    until(&shell, cx, "the bell has nothing unread", |cx| {
        bell_count(&shell, cx) == 0
    });
    let kept = rows(&user);
    assert_eq!(kept.len(), 1, "it stays in the centre, read: {kept:?}");
    assert!(kept[0]["read_at"].is_number(), "{kept:?}");
    // Reading is not answering: the request still waits in the rail.
    assert_eq!(kept[0]["resolved"], false, "{kept:?}");
    assert_eq!(kept[0]["title"], format!("Request #{number} waits for you"));
}

#[gpui_kit::test]
fn a_request_answered_elsewhere_is_resolved_in_the_centre(cx: &mut TestAppContext) {
    let user = user();
    let (_, mcp) = lead(&user);
    let number = hold(&mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the bell counts the request", |cx| {
        bell_count(&shell, cx) == 1
    });

    // From the command line, not from this window.
    let output = user.run(&["deny", &number.to_string()]);
    assert!(output.status.success(), "{output:?}");

    until(&shell, cx, "the centre says it is answered", |cx| {
        bell_count(&shell, cx) == 0
            && shell.read_with(cx, |s, _| {
                s.snapshot().bell.iter().all(|item| item.resolved)
            })
    });
    // The panel says it too: the row no longer says it waits.
    let key = shell.read_with(cx, |s, _| s.snapshot().bell[0].key.clone());
    cx.update(|window, cx| window.click("bell", cx));
    settle(cx);
    assert_eq!(
        label(format!("bell-{key}"), cx),
        Some(format!("Request #{number} answered"))
    );
    assert_eq!(
        bell_said(&shell, cx)[0].0,
        format!("Request #{number} answered")
    );
}

#[gpui_kit::test]
fn a_request_arriving_in_the_open_session_stays_unread(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &lead, cx);

    hold(&mcp);

    until(&shell, cx, "the window has the request", |cx| {
        shell.read_with(cx, |s, _| s.snapshot().bell.len() == 1)
    });
    // Whatever the window would have asked of the daemon, it had the time to.
    settle(cx);
    std::thread::sleep(std::time::Duration::from_millis(500));
    until(&shell, cx, "the window took what the daemon said", |_| true);
    let kept = rows(&user);
    assert!(kept[0]["read_at"].is_null(), "nobody read it: {kept:?}");
    assert_eq!(counts(&shell, cx), (1, 0), "it needs the user");
}

#[gpui_kit::test]
fn clicking_a_bell_row_marks_it_read(cx: &mut TestAppContext) {
    let user = user();
    let (_, mcp) = lead(&user);
    hold(&mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the bell counts the request", |cx| {
        bell_count(&shell, cx) == 1
    });
    let key = shell.read_with(cx, |s, _| s.snapshot().bell[0].key.clone());
    cx.update(|window, cx| window.click("bell", cx));
    settle(cx);

    cx.update(|window, cx| window.click(format!("bell-{key}"), cx));

    until(&shell, cx, "the bell has nothing unread", |cx| {
        bell_count(&shell, cx) == 0
    });
    eventually("the daemon has it read", || {
        rows(&user)[0]["read_at"].is_number()
    });
}

#[gpui_kit::test]
fn the_centre_groups_by_session_unread_first(cx: &mut TestAppContext) {
    let user = user();
    let (lead, lead_mcp) = lead(&user);
    let (other, other_mcp) = session(&user, "write the docs");
    let first = hold(&lead_mcp);
    let second = hold(&other_mcp);
    let third = hold(&lead_mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the bell counts the three requests", |cx| {
        bell_count(&shell, cx) == 3
    });
    // The lead's are read, from anywhere.
    user.call("notifications.read", json!({"session": lead}))
        .unwrap();
    until(&shell, cx, "the lead's are read", |cx| {
        bell_count(&shell, cx) == 1
    });

    let keys: Vec<String> = shell.read_with(cx, |s, _| {
        s.snapshot()
            .bell
            .iter()
            .map(|item| item.key.clone())
            .collect()
    });
    let order = shell.read_with(cx, |s, cx| s.bell_list().read(cx).order());
    // The session with something unread first; under the lead, its notifications, the newest
    // first, all read.
    assert_eq!(
        order,
        [
            format!("bell-group-{other}"),
            keys[1].clone(),
            format!("bell-group-{lead}"),
            keys[2].clone(),
            keys[0].clone()
        ],
        "{first} {second} {third}"
    );

    cx.update(|window, cx| window.click("bell", cx));
    settle(cx);
    assert!(has(bell_group_id(&lead), cx) && has(bell_group_id(&other), cx));
    assert_eq!(
        label(bell_group_title_id(&lead), cx).as_deref(),
        Some(BRIEF)
    );
    assert_eq!(
        label("bell-counts", cx).as_deref(),
        Some("1 need you"),
        "the count of what needs you, apart"
    );
    for key in &keys {
        assert!(has(format!("bell-{key}-time"), cx), "{key} says when");
    }
}

#[gpui_kit::test]
fn mark_all_read_empties_the_count(cx: &mut TestAppContext) {
    let user = user();
    let (_, lead_mcp) = lead(&user);
    let (_, other_mcp) = session(&user, "write the docs");
    hold(&lead_mcp);
    hold(&other_mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the bell counts the requests", |cx| {
        bell_count(&shell, cx) == 2
    });
    cx.update(|window, cx| window.click("bell", cx));
    settle(cx);

    cx.update(|window, cx| window.click("bell-mark-all", cx));

    until(&shell, cx, "nothing is unread", |cx| {
        counts(&shell, cx) == (0, 0)
    });
    eventually("the daemon says so", || {
        user.call("notifications.list", json!({"unread": true}))
            .unwrap()["notifications"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    // They stay in the panel, read; its count says nothing is unread.
    assert_eq!(rows(&user).len(), 2);
    until(&shell, cx, "the panel says nothing is unread", |cx| {
        label("bell-counts", cx).as_deref() == Some("Nothing unread")
    });
}

#[gpui_kit::test]
fn two_clients_see_the_same_read_state(first: &mut TestAppContext, second: &mut TestAppContext) {
    let user = user();
    let (_, mcp) = lead(&user);
    hold(&mcp);
    let (one, cx_one) = window(&user, first);
    let (two, cx_two) = window(&user, second);
    until(&one, cx_one, "the first window counts it", |cx| {
        bell_count(&one, cx) == 1
    });
    until(&two, cx_two, "the second window counts it", |cx| {
        bell_count(&two, cx) == 1
    });

    // Read in the first.
    cx_one.update(|window, cx| window.click("bell", cx));
    settle(cx_one);
    cx_one.update(|window, cx| window.click("bell-mark-all", cx));

    until(&one, cx_one, "the first window reads it", |cx| {
        bell_count(&one, cx) == 0
    });
    until(&two, cx_two, "the second window reads it too", |cx| {
        bell_count(&two, cx) == 0
    });
}
