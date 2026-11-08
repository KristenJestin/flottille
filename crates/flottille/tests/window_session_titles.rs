//! A session's title in the window (#589): the user renames a session from its row's "⋯" menu or
//! its page's header in a dialog with a labelled field, a title that changes fades to the new one
//! in the row and the header, the note of a led session names no session, and a title quoted in a
//! sentence stays on one line. On GPUI's test platform in front of a real daemon whose agents are
//! the fake ACP agent. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session::LED_NOTE;
use flottille_window::tree::{session_id, session_menu_id, session_rename_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The lead's first message: a whole paragraph on its first line.
const LONG: &str = "begin the docs session and write every page of the guide, the reference, \
    the tutorials and the release notes, with screenshots of each screen of the window";

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
    ]}));
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

fn project(user: &User) {
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

fn start(user: &User, message: &str) -> String {
    let id = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": message}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

/// The lead the user started with `message` and the session it leads: their ids.
fn lead_and_led(user: &User, message: &str) -> (String, String) {
    project(user);
    let lead = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": message}),
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
    eventually("the session exists", || agents(user).len() == 2);
    let led = agents(user)
        .into_iter()
        .find(|agent| agent["led_by"] == lead.as_str())
        .expect("a session led by the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, led)
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

fn label(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The title the session's row says.
fn row_title(cx: &mut VisualTestContext, session: &str) -> Option<String> {
    label(cx, session_id(session))
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
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

fn title_of(user: &User, agent: &str) -> (Value, Value) {
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    (
        shown["agent"]["title"].clone(),
        shown["agent"]["titled_by"].clone(),
    )
}

/// Types `title` in the open rename dialog and saves it.
fn rename_to(shell: &Entity<Shell>, title: &str, cx: &mut VisualTestContext) {
    assert!(shell.read_with(cx, |s, cx| s.rename_form().read(cx).is_open()));
    assert_eq!(
        label(cx, "rename-title-field-label").as_deref(),
        Some("Title"),
        "the field says what it is"
    );
    let field = shell.read_with(cx, |s, cx| s.rename_form().read(cx).field().clone());
    let title = title.to_owned();
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(&title, window, cx)));
    settle(cx);
    cx.update(|window, cx| window.click("rename-save", cx));
}

#[gpui_kit::test]
fn the_window_renames_a_session_from_its_rows_menu(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let id = start(&user, "write the release notes");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    // The row unfolds as it comes: it is clicked once it is there in full.
    settle(cx);

    cx.update(|window, cx| window.hover(session_id(&id), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_menu_id(&id), cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find(session_rename_id(&id)).is_some()),
        "the menu offers Rename…"
    );
    cx.update(|window, cx| window.click(session_rename_id(&id), cx));
    settle(cx);
    // The field holds the title it has now.
    let field = shell.read_with(cx, |s, cx| s.rename_form().read(cx).field().clone());
    assert_eq!(
        cx.update(|_, cx| field.read(cx).value().to_string()),
        "write the release notes"
    );
    rename_to(&shell, "Release notes", cx);

    eventually("the session renamed", || {
        title_of(&user, &id) == (json!("Release notes"), json!("user"))
    });
    until(&shell, cx, "the row says its new title", |cx| {
        row_title(cx, &id).as_deref() == Some("Release notes")
    });
    assert_eq!(toasts(&shell, cx), ["Session renamed"]);
    assert!(!shell.read_with(cx, |s, cx| s.rename_form().read(cx).is_open()));
}

#[gpui_kit::test]
fn the_window_renames_a_session_from_its_header_and_the_title_fades_in(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let id = start(&user, "write the release notes");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    // The row unfolds as it comes: it is clicked once it is there in full.
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the page", |cx| {
        label(cx, "session-title").as_deref() == Some("write the release notes")
    });
    settle(cx);
    assert_eq!(pending_frames(cx), 0, "at rest before");

    cx.update(|window, cx| window.click("session-rename", cx));
    settle(cx);
    rename_to(&shell, "Release notes", cx);
    until(
        &shell,
        cx,
        "the header and the row say the new title",
        |cx| {
            label(cx, "session-title").as_deref() == Some("Release notes")
                && row_title(cx, &id).as_deref() == Some("Release notes")
        },
    );
    // The new title fades in over the old one, then nothing moves.
    assert!(pending_frames(cx) > 0, "the new title fades in");
    settle(cx);
    assert_eq!(pending_frames(cx), 0);

    // A title given from elsewhere (the CLI's method) moves the same way, without a refresh.
    user.call(
        "agent.rename",
        json!({"agent": id, "title": "Notes for 1.2"}),
    )
    .unwrap();
    until(&shell, cx, "the header follows", |cx| {
        label(cx, "session-title").as_deref() == Some("Notes for 1.2")
    });
    assert!(pending_frames(cx) > 0, "the new title fades in");
}

#[gpui_kit::test]
fn the_led_sessions_note_does_not_quote_the_leads_title(cx: &mut TestAppContext) {
    let user = user();
    let (_, led) = lead_and_led(&user, "begin the docs session");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led session's row", |cx| {
        label(cx, session_id(&led)).is_some()
    });
    // The row unfolds as it comes: it is clicked once it is there in full.
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&led), cx));
    until(&shell, cx, "the note", |cx| {
        label(cx, "session-led-note").is_some()
    });
    let note = label(cx, "session-led-note").unwrap();
    assert_eq!(note, LED_NOTE);
    assert_eq!(
        note,
        "Your messages go straight to this session; its lead is not asked."
    );
    assert!(!note.contains("begin the docs session"), "{note}");
}

#[gpui_kit::test]
fn a_title_quoted_in_a_sentence_never_takes_more_than_one_line(cx: &mut TestAppContext) {
    let user = user();
    let (_, led) = lead_and_led(&user, LONG);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led session's row", |cx| {
        label(cx, session_id(&led)).is_some()
    });
    // The row unfolds as it comes: it is clicked once it is there in full.
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&led), cx));
    until(&shell, cx, "the line that says who leads it", |cx| {
        label(cx, "session-led-by").is_some()
    });
    settle(cx);
    // The sentence says the whole title to assistive technology, and draws it on one line.
    assert_eq!(
        label(cx, "session-led-by").as_deref(),
        Some(format!("led by {LONG}").as_str())
    );
    let height = |cx: &mut VisualTestContext, id: &str| {
        let id = id.to_owned();
        cx.update(|window, _| f32::from(window.find(id).bounds().size.height))
    };
    // A line of the header's small text, beside it; two lines would be twice its height.
    let line = height(cx, "session-place");
    let sentence = height(cx, "session-led-by");
    let title = height(cx, "session-led-by-title");
    assert!(
        sentence < 1.5 * line,
        "one line (about {line} px), not {sentence} px"
    );
    assert_eq!(title, sentence, "the title on the sentence's one line");

    // A toast says the whole title and draws it on its one line, cut to the room it has.
    cx.update(|window, cx| window.hover(session_id(&led), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_menu_id(&led), cx));
    settle(cx);
    cx.update(|window, cx| window.click(flottille_window::tree::session_detach_id(&led), cx));
    until(&shell, cx, "the detached toast", |cx| {
        !toasts(&shell, cx).is_empty()
    });
    settle(cx);
    let toast = toasts(&shell, cx).remove(0);
    assert_eq!(toast, format!("Detached from {LONG}"));
    let line = cx.update(|window, _| {
        let found = window.find("toast-1-title");
        f32::from(found.bounds().size.height)
    });
    assert!(line < 30., "the toast's title is one line, not {line} px");
}
