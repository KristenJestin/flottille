//! The scenarios of #739: every fenced code block of a reply has a Copy button in its top-right
//! corner, beside the language label when there is one; a click puts the block's text alone on the
//! clipboard and says it in a toast. On GPUI's test platform in front of a real daemon whose agent
//! is the fake ACP agent, with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::session::COPIED;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A reply with a block that names its language and one that does not, the second with an
/// indented line and a trailing space, which a copy keeps.
const REPLY: &str = "Run this:\n\n```sh\ncargo test -p app --lib\n```\n\nThen, as it is:\n\n\
    ```\nfirst line\n  indented second \n```\n\nThat is all.";

const SHELL: &str = "cargo test -p app --lib";
const PLAIN: &str = "first line\n  indented second ";

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "show the blocks", "do": [{"say": REPLY}]}]}));
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

/// A session in a ready workspace of a throwaway project, its first turn ended on [`REPLY`].
fn session(user: &User) -> String {
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
            json!({"project": "app", "workspace": "w", "message": "show the blocks"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn", || {
        let shown: Value = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["agent"]["state"] == "idle"
    });
    id
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

/// The session's page open on its reply.
fn open<'a>(
    user: &User,
    agent: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(900.)));
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(agent)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
    until(&shell, cx, "the reply shows", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).session_page().is_some_and(|page| {
                let page = page.read(cx);
                let blocks = page.conversation().read(cx);
                blocks
                    .blocks()
                    .iter()
                    .any(|b| matches!(b, Block::Text { text } if text == REPLY))
            })
        })
    });
    settle(cx);
    (shell, cx)
}

/// The Copy buttons of the blocks on screen, in the order of their blocks.
fn copies(cx: &mut VisualTestContext) -> Vec<ElementId> {
    cx.update(|window, _| {
        (0..REPLY.len())
            .map(|at| ElementId::from(format!("copy-code-{at}")))
            .filter(|id| window.try_find(id.clone()).is_some())
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

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

#[gpui_kit::test]
fn a_code_block_has_a_copy_button(cx: &mut TestAppContext) {
    let user = user();
    let agent = session(&user);
    let (_shell, cx) = open(&user, &agent, cx);

    let buttons = copies(cx);
    assert_eq!(buttons.len(), 2, "one Copy for each of the two blocks");
    cx.update(|window, _| {
        for id in &buttons {
            let found = window.find(id.clone());
            assert_eq!(found.label(), Some("Copy"), "a button whose label is Copy");
        }
    });
}

#[gpui_kit::test]
fn copy_puts_the_block_s_text_alone_on_the_clipboard(cx: &mut TestAppContext) {
    let user = user();
    let agent = session(&user);
    let (shell, cx) = open(&user, &agent, cx);
    let buttons = copies(cx);
    assert_eq!(buttons.len(), 2, "both blocks have their Copy");

    cx.update(|window, cx| window.click(buttons[0].clone(), cx));
    settle(cx);
    assert_eq!(
        clipboard(cx).as_deref(),
        Some(SHELL),
        "no language label, no added newline"
    );
    assert_eq!(toasts(&shell, cx), [COPIED]);

    // The other block copies its own text, indentation and trailing space as they are.
    cx.update(|window, cx| window.click(buttons[1].clone(), cx));
    settle(cx);
    assert_eq!(clipboard(cx).as_deref(), Some(PLAIN));
}

#[gpui_kit::test]
fn a_block_without_a_language_has_its_copy_button_too(cx: &mut TestAppContext) {
    let user = user();
    let agent = session(&user);
    let (_shell, cx) = open(&user, &agent, cx);
    let buttons = copies(cx);

    // The kit draws the language itself only where there is no action: the window draws it
    // beside the button, for the first block and not for the second.
    let labelled = cx.update(|window, _| {
        (0..REPLY.len())
            .filter(|at| {
                window
                    .try_find(ElementId::from(format!("code-language-{at}")))
                    .is_some()
            })
            .count()
    });
    assert_eq!(labelled, 1, "the language is said for the one that has it");
    assert_eq!(buttons.len(), 2, "and both blocks have their Copy");
}
