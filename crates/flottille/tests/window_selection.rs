//! Text selected in a conversation: its highlight stays drawn on every frame of the drag that
//! makes it and after it, in the message where the drag started alone, and nothing is drawn
//! while the pointer rests over it.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_kit::base::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext,
    VisualTestContext, point, px, size,
};
use serde_json::{Value, json};

const REPLY: &str = "The reply holds a sentence long enough to select a part of it with the mouse, \
    and a second one after it.\n\nA second paragraph, to drag the selection across the gap between \
    two pieces of the same reply.";

/// What the user said, long enough to wrap in its bubble.
const MINE: &str =
    "lead the work: the user's own message, long enough to be selected with the mouse too";

fn script() -> Value {
    json!({"turns": [{"when": "lead the work", "do": [{"say": REPLY}]}]})
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

/// A session the user started in a ready workspace of a throwaway project, its first turn ended.
fn lead(user: &User) -> String {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "w", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": MINE}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the lead's first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
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
    // As the real window does (`gpui_kit::open_window`): under the kit's root, which holds the
    // window's text selection.
    let mut built = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let shell = cx.new(|cx| Shell::new_polled(connect, window, cx));
        built = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let shell = built.unwrap();
    cx.simulate_resize(size(px(1440.), px(800.)));
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

/// One frame of the window, 16 ms on. Something else in the window is drawn again on each (a
/// running agent's clock, a caret): the selected text has to come out of it unchanged.
fn frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| {
        window.refresh();
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn open_session<'a>(
    user: &User,
    agent: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<SessionPage>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(agent), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
    let page = |cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
    };
    until(&shell, cx, "the session's page shows the reply", |cx| {
        page(cx).is_some_and(|page| {
            page.read_with(cx, |page, cx| {
                page.conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .any(|b| matches!(b, Block::Text { text } if text == REPLY))
            })
        })
    });
    settle(cx);
    (page(cx).unwrap(), cx)
}

/// The text regions of the user's message and of the reply.
fn regions(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> (String, String) {
    let row_of = |matches: fn(&Block) -> bool, cx: &mut VisualTestContext| {
        page.read_with(cx, |page, cx| {
            page.conversation()
                .read(cx)
                .blocks()
                .iter()
                .position(matches)
        })
        .expect("the row is in the conversation")
    };
    let mine = row_of(|b| matches!(b, Block::Message { .. }), cx);
    let reply = row_of(|b| matches!(b, Block::Text { .. }), cx);
    (
        format!("row-{mine}-message-region"),
        format!("row-{reply}-text-region"),
    )
}

fn bounds(region: &str, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.update(|window, _| window.find(region.to_owned()).bounds())
}

/// How wide the boxes of the last frame painted in the selection's colour inside `region` are,
/// added up.
fn highlighted(region: &str, cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, cx| {
        let selection = flottille_ui::palette(cx).sel;
        let scale = window.scale_factor();
        let bounds = window.find(region.to_owned()).bounds().scale(scale);
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.background == selection.into() && bounds.intersects(&quad.bounds))
            .map(|quad| quad.bounds.size.width.0 / scale)
            .sum()
    })
}

/// Presses on the first line of `from`, drags along it, then on into `other` (the other
/// message), holds the pointer still and lets go, a frame after each step: what is highlighted
/// in `from` and in `other` on each frame once the drag has selected something.
fn drag(from: &str, other: &str, cx: &mut VisualTestContext) -> Vec<(f32, f32)> {
    let (start, beyond) = (bounds(from, cx), bounds(other, cx));
    let none = Modifiers::default();
    // Inside the line, so that leaving the message upward keeps what lies before the press.
    let pressed = point(start.origin.x + px(120.), start.origin.y + px(8.));
    let along = point(start.origin.x + px(300.), pressed.y);
    let into = point(beyond.origin.x + px(40.), beyond.origin.y + px(8.));
    cx.simulate_mouse_move(pressed, None, none);
    settle(cx);
    cx.simulate_mouse_down(pressed, MouseButton::Left, none);
    frame(cx);
    let mut path = Vec::new();
    for step in 1..=10 {
        path.push(pressed + (along - pressed) * (step as f32 / 10.));
    }
    for step in 1..=10 {
        path.push(along + (into - along) * (step as f32 / 10.));
    }
    let mut seen = Vec::new();
    let mut look = |cx: &mut VisualTestContext| {
        seen.push((highlighted(from, cx), highlighted(other, cx)));
    };
    for at in path {
        cx.simulate_mouse_move(at, MouseButton::Left, none);
        // The pointer held still mid-drag: the frames that follow still show it.
        for _ in 0..3 {
            frame(cx);
            look(cx);
        }
    }
    cx.simulate_mouse_up(into, MouseButton::Left, none);
    for _ in 0..20 {
        frame(cx);
        look(cx);
    }
    seen
}

/// From the first frame that shows the selection on, every frame shows it in the message where
/// the drag started, and none in the other.
fn assert_held_in_its_message(seen: &[(f32, f32)]) {
    let first = seen
        .iter()
        .position(|&(own, _)| own > 0.0)
        .expect("the drag selected something");
    assert!(
        seen[first..]
            .iter()
            .all(|&(own, other)| own > 0.0 && other == 0.0),
        "the selection's highlight left its message or disappeared on some frames: {seen:?}"
    );
}

#[gpui_kit::test]
fn selecting_text_in_a_reply_by_a_drag_keeps_its_highlight_drawn(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (page, cx) = open_session(&user, &lead, cx);
    let (mine, reply) = regions(&page, cx);
    let seen = drag(&reply, &mine, cx);
    assert_held_in_its_message(&seen);
}

#[gpui_kit::test]
fn selecting_text_in_a_user_message_keeps_its_highlight_drawn(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (page, cx) = open_session(&user, &lead, cx);
    let (mine, reply) = regions(&page, cx);
    let seen = drag(&mine, &reply, cx);
    assert_held_in_its_message(&seen);
}

#[gpui_kit::test]
fn at_rest_with_a_selection_no_frame_is_pending(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (page, cx) = open_session(&user, &lead, cx);
    let (mine, reply) = regions(&page, cx);
    drag(&reply, &mine, cx);
    settle(cx);
    assert!(
        highlighted(&reply, cx) > 0.0,
        "the selection is still there"
    );
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing is drawn while the pointer rests over a selection"
    );
}
