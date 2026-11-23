//! The scenarios of #1090 (the first frame is in the chosen theme, and each part that waits for the
//! daemon says it loads), on GPUI's test platform in front of a real daemon that answers only when
//! the test lets it, with a throwaway user whose desktop is described by temporary files. No window
//! opens on a screen.

use crate::common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::look::Memory;
use flottille_window::tree::project_id;
use gpui_design_system::Palette;
use gpui_design_system::color::hex;
use gpui_design_system::omarchy::Environment;
use gpui_design_system::theme::palette;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// An invented dark Omarchy theme.
const THEME: &str = "background = \"#0e1116\"\nforeground = \"#c9d1d9\"\naccent = \"#f0883e\"\n";

/// A user on an Omarchy desktop with [`THEME`] as its current theme.
fn user() -> User {
    let user = User::new();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=omarchy\n").unwrap();
    let path = env_of(&user).colors_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, THEME).unwrap();
    user
}

fn env_of(user: &User) -> Environment {
    Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    }
}

/// The window's own copy of the theme, in the user's state folder.
fn memory_of(user: &User) -> Memory {
    Memory::in_folder(&user.path("state/flottille"))
}

fn remember(user: &User, theme: &str, shade: &str) {
    let memory = memory_of(user);
    std::fs::create_dir_all(memory.path().parent().unwrap()).unwrap();
    std::fs::write(
        memory.path(),
        json!({"theme": theme, "shade": shade}).to_string(),
    )
    .unwrap();
}

fn remembered(user: &User) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(memory_of(user).path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// A daemon that answers once [`Gate::open`] is called, and at the latest when the test ends.
#[derive(Clone)]
struct Gate(Arc<AtomicBool>);

impl Gate {
    fn closed() -> Gate {
        Gate(Arc::new(AtomicBool::new(false)))
    }

    fn open(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn connect(&self, user: &User) -> Connect {
        let through = user.connector();
        let open = self.0.clone();
        Arc::new(move || {
            while !open.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            through()
        })
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.open();
    }
}

/// A window whose daemon answers when `gate` opens, its theme set up as the program does: from
/// the copy the user's state folder holds.
fn window<'a>(
    user: &User,
    gate: &Gate,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect = gate.connect(user);
    cx.update(|cx| flottille_window::start(env_of(user), Some(memory_of(user)), cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1600.)));
    (shell, cx)
}

fn frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
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

/// Lets every motion end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        frame(cx);
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn shown(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
}

/// One project, for the sidebar to have a row.
fn project(user: &User) {
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
}

#[gpui_kit::test]
fn the_first_frame_is_drawn_in_the_chosen_theme(cx: &mut TestAppContext) {
    let user = user();
    // The user chose Flottille's light theme; their desktop is Omarchy, which is what the window
    // would draw with when it knows nothing of the choice.
    remember(&user, "flottille", "light");
    let gate = Gate::closed();
    let (_shell, cx) = window(&user, &gate, cx);

    // The daemon has not answered: this is the first frame.
    assert_eq!(cx.update(|_, cx| palette(cx)), Palette::light());
    frame(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)), Palette::light());
}

#[gpui_kit::test]
fn a_chosen_omarchy_theme_is_followed_from_the_first_frame(cx: &mut TestAppContext) {
    let user = user();
    remember(&user, "omarchy", "light");
    let gate = Gate::closed();
    let (_shell, cx) = window(&user, &gate, cx);

    // What is drawn is the desktop's theme as it is now, read before the first frame: the copy
    // holds the choice, not the colours.
    assert_eq!(cx.update(|_, cx| palette(cx)).acc, hex(0xf0883e));
    assert_eq!(cx.update(|_, cx| palette(cx)).bg, hex(0x0e1116));
    frame(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)).acc, hex(0xf0883e));
}

#[gpui_kit::test]
fn no_theme_switch_happens_at_startup_when_the_choice_did_not_change(cx: &mut TestAppContext) {
    let user = user();
    user.call(
        "appearance.set",
        json!({"theme": "flottille", "shade": "light"}),
    )
    .unwrap();
    remember(&user, "flottille", "light");
    let gate = Gate::closed();
    let (shell, cx) = window(&user, &gate, cx);
    gate.open();

    // From the first frame to the daemon's answer and for a good while after: not one frame in
    // another palette, not one cross-fade.
    let mut answered = false;
    for _ in 0..400 {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        frame(cx);
        assert_eq!(
            cx.update(|_, cx| palette(cx)),
            Palette::light(),
            "the palette never moved"
        );
        answered |= shell.read_with(cx, |shell, cx| shell.main().read(cx).read());
        if answered {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(answered, "the daemon answered");
    for _ in 0..30 {
        frame(cx);
        assert_eq!(cx.update(|_, cx| palette(cx)), Palette::light());
    }
}

#[gpui_kit::test]
fn the_daemons_answer_is_kept_as_the_copy_for_the_next_first_frame(cx: &mut TestAppContext) {
    let user = user();
    user.call(
        "appearance.set",
        json!({"theme": "flottille", "shade": "light"}),
    )
    .unwrap();
    assert_eq!(remembered(&user), None, "nothing yet");
    let gate = Gate::closed();
    let (shell, cx) = window(&user, &gate, cx);
    gate.open();
    until(&shell, cx, "the copy is written", |_| {
        remembered(&user) == Some(json!({"theme": "flottille", "shade": "light"}))
    });
}

#[gpui_kit::test]
fn a_part_waiting_for_the_daemon_says_it_loads(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let gate = Gate::closed();
    let (shell, cx) = window(&user, &gate, cx);
    for _ in 0..5 {
        frame(cx);
    }
    assert!(shown("sidebar-skeleton-0", cx), "the sidebar says it loads");
    assert!(shown("main-skeleton", cx), "the page says it loads");
    assert!(!shown("project-atlas", cx), "nothing looks finished");

    gate.open();
    until(&shell, cx, "the projects are shown", |cx| {
        cx.update(|window, _| window.try_find(project_id("atlas")).is_some())
    });
    settle(cx);
    assert!(!shown("sidebar-skeleton-0", cx), "the skeleton went");
    assert!(!shown("main-skeleton", cx), "the skeleton went");
    assert!(shown("project-atlas", cx));
}

#[gpui_kit::test]
fn content_that_arrives_enters_with_its_motion(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let gate = Gate::closed();
    let (shell, cx) = window(&user, &gate, cx);
    for _ in 0..5 {
        frame(cx);
    }
    gate.open();
    until(&shell, cx, "the daemon answered", |cx| {
        shell.read_with(cx, |shell, cx| shell.main().read(cx).read())
    });

    // Partway, the skeleton and what replaces it are both drawn: the content does not jump in.
    for _ in 0..4 {
        frame(cx);
    }
    assert!(shown("main-skeleton", cx), "the skeleton is still fading");
    assert!(
        cx.update(|window, _| window.try_find(project_id("atlas")).is_some()),
        "the row is already arriving"
    );
    settle(cx);
    assert!(!shown("main-skeleton", cx));
    assert!(!shown("sidebar-skeleton-0", cx));
}

#[gpui_kit::test]
fn a_daemon_that_does_not_answer_ends_on_the_unreachable_state_not_on_a_skeleton(
    cx: &mut TestAppContext,
) {
    let user = user();
    // Nothing listens, and nothing starts: the link says it is reconnecting.
    let connect: Connect = Arc::new(|| Err(std::io::Error::other("nobody listens")));
    cx.update(|cx| flottille_window::start(env_of(&user), Some(memory_of(&user)), cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1600.)));
    until(&shell, cx, "the window says it reconnects", |cx| {
        !shell.read_with(cx, |shell, cx| shell.top().read(cx).online())
    });
    settle(cx);
    assert!(!shown("main-skeleton", cx), "no skeleton pulses for ever");
    assert!(!shown("sidebar-skeleton-0", cx));
}
