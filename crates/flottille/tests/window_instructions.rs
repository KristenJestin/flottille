//! The "Instructions" page of Settings (`docs/agents.md`, "Roles"): the user's own instructions
//! for every agent, edited and saved on demand, what a refusal looks like, what the page says of
//! a file cut by hand, and what moves. On GPUI's test platform in front of a real daemon,
//! throwaway repositories and temporary XDG folders. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::instructions::InstructionsEditor;
use flottille_window::link::Connect;
use flottille_window::settings::SettingsPage;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
    user
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    let connect: Connect = Arc::new(user.connector());
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    (shell, cx)
}

/// Runs the window until `check` holds, as real frames would.
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

fn run_for(by: Duration, cx: &mut VisualTestContext) {
    let step = Duration::from_millis(16);
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(step);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += step;
    }
}

/// Lets every motion end, then draws.
fn settle(cx: &mut VisualTestContext) {
    run_for(Duration::from_millis(1500), cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

/// Settings, on the Instructions tab (found by its key, never by its place), once the daemon's
/// text is shown.
fn open_instructions(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (Entity<SettingsPage>, Entity<InstructionsEditor>) {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    assert!(
        flottille_window::settings::SECTIONS
            .iter()
            .any(|(key, _)| *key == "instructions")
    );
    cx.update(|window, cx| window.click("settings-tabs-instructions", cx));
    let editor = page.read_with(cx, |page, _| page.instructions().clone());
    until(shell, cx, "the instructions are shown", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);
    (page, editor)
}

fn type_into(editor: &Entity<InstructionsEditor>, text: &str, cx: &mut VisualTestContext) {
    let field = editor
        .read_with(cx, |editor, _| editor.field().cloned())
        .unwrap();
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(text, window, cx)));
    cx.run_until_parked();
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

fn height_of(id: &str, cx: &mut VisualTestContext) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().size.height))
}

fn y_of(id: &str, cx: &mut VisualTestContext) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().origin.y))
}

#[gpui_kit::test]
fn the_instructions_page_saves_the_text_on_demand_and_a_new_session_gets_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let (_, editor) = open_instructions(&shell, cx);

    // Empty, saying when a change takes effect; nothing to save yet.
    assert_eq!(
        editor.read_with(cx, |e, cx| e.typed(cx)),
        Some(String::new())
    );
    assert!(!editor.read_with(cx, |e, cx| e.savable(cx)));
    cx.update(|window, _| {
        assert!(window.find("instructions-text").visible());
        assert!(window.find("instructions-reach").visible());
        assert!(window.find("instructions-save").visible());
        assert!(window.find("instructions-title").visible());
    });
    assert!(flottille_window::instructions::REACH.contains("created from now on"));

    type_into(&editor, "Name branches after the issue.", cx);
    assert!(editor.read_with(cx, |e, cx| e.savable(cx)));
    // Saved on demand: typing alone writes nothing.
    assert!(!user.path("config/flottille/instructions.md").exists());
    cx.update(|window, cx| window.click("instructions-save", cx));
    until(&shell, cx, "the instructions are saved", |cx| {
        user.path("config/flottille/instructions.md").exists()
            && !editor.read_with(cx, |e, cx| e.savable(cx))
    });
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/instructions.md")).unwrap(),
        "Name branches after the issue."
    );
    until(&shell, cx, "the toast says so", |cx| {
        toasts(&shell, cx) == ["Instructions saved"]
    });

    let answer = user
        .call("agent.run", json!({"project": "atlas", "message": "hello"}))
        .unwrap();
    let shown = user
        .call("agent.show", json!({"agent": answer["agent"]["id"]}))
        .unwrap();
    let text = shown["agent"]["instructions"].as_str().unwrap();
    assert!(
        text.contains("# The user's instructions, for every project\n\nName branches"),
        "{text}"
    );

    // Saved empty, the file goes: nothing more is added.
    type_into(&editor, "", cx);
    cx.update(|window, cx| window.click("instructions-save", cx));
    until(&shell, cx, "the file is removed", |_| {
        !user.path("config/flottille/instructions.md").exists()
    });
}

#[gpui_kit::test]
fn a_refused_save_is_one_failure_toast_and_keeps_what_was_typed(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let (_, editor) = open_instructions(&shell, cx);

    let long = "x".repeat(8_001);
    type_into(&editor, &long, cx);
    cx.update(|window, cx| window.click("instructions-save", cx));
    until(&shell, cx, "the refusal is said", |cx| {
        toasts(&shell, cx) == ["Instructions not saved"]
    });

    // Nothing was written, what was typed is still there to be shortened, and Save is open again.
    assert!(!user.path("config/flottille/instructions.md").exists());
    assert_eq!(editor.read_with(cx, |e, cx| e.typed(cx)), Some(long));
    assert!(editor.read_with(cx, |e, cx| e.savable(cx)));
    let detail = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .filter_map(|toast| toast.second_line().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(detail.iter().any(|d| d.contains("8000")), "{detail:?}");
}

#[gpui_kit::test]
fn a_file_cut_by_hand_is_said_on_the_page_and_saving_makes_it_fit(cx: &mut TestAppContext) {
    let user = user();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/instructions.md"),
        format!("{}TAIL", "x".repeat(8_000)),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    let (_, editor) = open_instructions(&shell, cx);

    let cut = editor.read_with(cx, |e, _| e.shown().unwrap().cut.clone());
    let cut = cut.expect("the cut is said");
    assert!(
        cut.contains("8004 characters") && cut.contains("8000"),
        "{cut}"
    );
    cx.update(|window, _| assert!(window.find("instructions-cut").visible()));
    assert_eq!(
        editor.read_with(cx, |e, cx| e.typed(cx)),
        Some("x".repeat(8_000))
    );

    // The cut line folds away with its gap once the file fits, and the page then rests.
    let before = height_of("instructions-cut-block", cx);
    assert!(before > 0.0, "the cut line has room");
    cx.update(|window, cx| window.click("instructions-save", cx));
    until(&shell, cx, "the cut text is saved", |cx| {
        editor.read_with(cx, |e, _| e.shown().unwrap().cut.is_none())
    });
    run_for(Duration::from_millis(60), cx);
    let folding = height_of("instructions-cut-block", cx);
    assert!(
        folding < before,
        "the line is folding away: {folding} of {before}"
    );
    settle(cx);
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn at rest");
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/instructions.md")).unwrap(),
        "x".repeat(8_000)
    );
}

#[gpui_kit::test]
fn a_change_made_elsewhere_shows_in_the_open_page(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let (_, editor) = open_instructions(&shell, cx);

    let output = user.run(&["config", "instructions", "--file", {
        let file = user.path("from-the-cli.md");
        std::fs::write(&file, "Set from the command line.\n").unwrap();
        file.to_str().unwrap().to_owned().leak()
    }]);
    assert!(output.status.success());

    until(&shell, cx, "the page shows the new text", |cx| {
        editor.read_with(cx, |e, cx| {
            e.typed(cx).as_deref() == Some("Set from the command line.\n")
        })
    });
}

#[gpui_kit::test]
fn the_page_and_its_field_move_instead_of_jumping(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    cx.update(|window, cx| window.click("settings-tabs-instructions", cx));

    // The section enters, rising into its place, as every section does when its tab is chosen.
    run_for(Duration::from_millis(20), cx);
    let rising = y_of("enter-settings-instructions", cx);
    settle(cx);
    let there = y_of("enter-settings-instructions", cx);
    assert!(
        rising - there > 1.0,
        "still rising when chosen: {}",
        rising - there
    );
    assert_eq!(pending_frames(cx), 0);

    // A field that gains lines grows by easing, not by a jump.
    let editor = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap()
        .read_with(cx, |page, _| page.instructions().clone());
    until(&shell, cx, "the instructions are shown", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);
    let small = height_of("instructions-grow", cx);
    type_into(&editor, &"a line\n".repeat(30), cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    run_for(Duration::from_millis(40), cx);
    let easing = height_of("instructions-grow", cx);
    settle(cx);
    let grown = height_of("instructions-grow", cx);
    assert!(grown > small, "the field grew: {small} -> {grown}");
    assert!(
        easing < grown,
        "and was still on its way: {easing} of {grown}"
    );
    assert_eq!(pending_frames(cx), 0);
}
