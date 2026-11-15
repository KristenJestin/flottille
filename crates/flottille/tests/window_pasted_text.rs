//! The scenarios of #848, a large paste becomes an attached text file: a text longer than the
//! threshold is a chip in the composer's text, named `pasted-text.txt` with its size, instead of
//! filling the field; Ctrl+Shift+V pastes inline; the agent is told where the whole file is, and
//! the conversation shows the chip and opens the text read-only. On GPUI's test platform in front
//! of a real daemon whose agents are the fake ACP agent, with a throwaway user. No window opens on
//! a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::composer::Composer;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::{Reply, SessionPage};
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_design_system::Piece;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// What the toast of a large paste says, in its two lines.
const TOAST: (&str, &str) = (
    "Large paste attached as a file",
    "Ctrl+Shift+V to paste it inline",
);

/// A log of 120 lines, 4,092 characters: over both thresholds.
fn long_log() -> String {
    (1..=120)
        .map(|n| format!("2026-11-15 INFO request {n} served\n"))
        .collect()
}

/// A text under both thresholds.
const SHORT: &str = "just\nthree\nlines";

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true, "turns": [
        {"when": "work briefly", "do": [{"wait": 1500}, {"say": "done"}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
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

/// Starts a session and waits until its first turn ended.
fn start(user: &User, message: &str) -> String {
    let answer = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
}

fn window<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1000.)));
    (shell, cx)
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// A few frames, as a window draws them after a change.
fn frames(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
}

fn open_session<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, Entity<Reply>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    // Its row unfolds into the tree: it is there to be clicked once it has.
    settle(cx);
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(
        &shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            page_of(&shell, cx).read_with(cx, |page, cx| {
                !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
            })
        },
    );
    settle(cx);
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(lead).cloned())
        .expect("the session has a composer");
    (shell, reply, cx)
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

/// The element of the chip `n` (from 0, in its order) of a text of chips, as the kit names it.
fn chip(of: &str, n: u64) -> gpui_kit::ElementId {
    gpui_kit::ElementId::NamedInteger(format!("{of}-token").into(), n)
}

fn focus(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("reply-field", cx));
    frames(cx);
}

/// Puts `text` on the clipboard and presses `keys` in the focused field.
fn paste(text: &str, keys: &str, cx: &mut VisualTestContext) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
    cx.simulate_keystrokes(keys);
    frames(cx);
}

/// The reply's text and chips, in order: `text:` a run of text, `chip:` a chip's label.
fn pieces(reply: &Entity<Reply>, cx: &mut VisualTestContext) -> Vec<String> {
    reply.read_with(cx, |r, cx| {
        r.tokens()
            .read(cx)
            .value(cx)
            .into_iter()
            .map(|piece| match piece {
                Piece::Text(text) => format!("text:{text}"),
                Piece::Token(token) => format!("chip:{}", token.look.label),
            })
            .collect()
    })
}

/// The detail (the size) the reply's first chip says.
fn detail(reply: &Entity<Reply>, cx: &mut VisualTestContext) -> String {
    reply.read_with(cx, |r, cx| {
        r.tokens()
            .read(cx)
            .value(cx)
            .into_iter()
            .find_map(|piece| match piece {
                Piece::Token(token) => token.look.detail.map(|d| d.to_string()),
                Piece::Text(_) => None,
            })
            .unwrap_or_default()
    })
}

/// The titles and second lines of the toasts shown, oldest first.
fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<(String, String)> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| {
                (
                    toast.title().to_owned(),
                    toast.second_line().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    })
}

/// The text of the blocks of the last prompt the agent received, in one.
fn last_prompt(user: &User) -> String {
    user.received_requests("session/prompt").last().unwrap()["params"]["prompt"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect()
}

/// The file the prompt names after its `Pasted text` marker: where the agent reads it.
fn named_file(prompt: &str) -> std::path::PathBuf {
    let marker = &prompt[prompt
        .find("[Pasted text 1: ")
        .expect("the prompt has the marker")..];
    let path = &marker[marker
        .find("read it at ")
        .expect("the marker says where to read it")
        + "read it at ".len()..];
    std::path::PathBuf::from(&path[..path.find(']').unwrap()])
}

/// Sends the reply's message and waits for the agent to get it.
fn send(shell: &Entity<Shell>, user: &User, cx: &mut VisualTestContext) {
    let sent = user.received_requests("session/prompt").len();
    cx.simulate_keystrokes("enter");
    until(shell, cx, "the agent got the message", |_| {
        user.received_requests("session/prompt").len() > sent
    });
}

#[gpui_kit::test]
fn a_long_paste_becomes_a_file_token(cx: &mut TestAppContext) {
    let user = user();
    let lead = start(&user, "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    cx.simulate_input("look at this: ");

    paste(&long_log(), "ctrl-v", cx);
    settle(cx);

    assert_eq!(
        pieces(&reply, cx),
        ["text:look at this: ", "chip:pasted-text.txt"],
        "the text is a chip where it was pasted, and not in the field"
    );
    assert_eq!(detail(&reply, cx), "4 KiB", "the chip says its size");
    assert_eq!(
        toasts(&shell, cx),
        [(TOAST.0.to_owned(), TOAST.1.to_owned())],
        "a toast says it once"
    );
    paste(&long_log(), "ctrl-v", cx);
    assert_eq!(
        toasts(&shell, cx).len(),
        1,
        "a second large paste does not say it again"
    );
}

#[gpui_kit::test]
fn ctrl_shift_v_pastes_inline(cx: &mut TestAppContext) {
    let user = user();
    let lead = start(&user, "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);

    paste(&long_log(), "ctrl-shift-v", cx);

    assert_eq!(
        pieces(&reply, cx),
        [format!("text:{}", long_log())],
        "the whole text is in the field, with no chip"
    );
    assert!(toasts(&shell, cx).is_empty(), "nothing is said");
    // Only that paste: the next Ctrl+V of the same text is a file again.
    paste(&long_log(), "ctrl-v", cx);
    assert!(
        pieces(&reply, cx).contains(&"chip:pasted-text.txt".to_owned()),
        "{:?}",
        pieces(&reply, cx)
    );
}

#[gpui_kit::test]
fn a_short_paste_stays_inline(cx: &mut TestAppContext) {
    let user = user();
    let lead = start(&user, "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);

    paste(SHORT, "ctrl-v", cx);

    assert_eq!(pieces(&reply, cx), [format!("text:{SHORT}")]);
    assert!(toasts(&shell, cx).is_empty(), "nothing is said");
}

#[gpui_kit::test]
fn the_agent_receives_the_pasted_file_whole(cx: &mut TestAppContext) {
    let user = user();
    let lead = start(&user, "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    cx.simulate_input("what fails here? ");
    paste(&long_log(), "ctrl-v", cx);
    cx.simulate_input(" and why");
    settle(cx);
    send(&shell, &user, cx);

    let prompt = last_prompt(&user);
    assert!(
        prompt.contains("what fails here? [Pasted text 1: pasted-text.txt"),
        "the message names the file where the chip was: {prompt}"
    );
    assert!(prompt.ends_with("] and why"), "{prompt}");
    let file = named_file(&prompt);
    assert!(
        file.starts_with(user.path("data")) && file.to_string_lossy().contains(&lead),
        "the file is in the agent's folder: {file:?}"
    );
    // What the agent reads at that path is the whole text: nothing is cut.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), long_log());
    assert!(
        !prompt.contains("request 60 served"),
        "the text itself is not in the prompt"
    );
}

#[gpui_kit::test]
fn a_secret_in_the_paste_never_reaches_the_file(cx: &mut TestAppContext) {
    let user = user();
    let lead = start(&user, "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    // Invented values, shaped as a token and a private key.
    let token = format!("ghp_{}", "InventedValue1".repeat(3));
    let key = "-----BEGIN PRIVATE KEY-----\nSGVyZSBpcyBub3RoaW5n\n-----END PRIVATE KEY-----";
    let text = format!("{}token={token}\n{key}\n", long_log());
    paste(&text, "ctrl-v", cx);
    settle(cx);
    send(&shell, &user, cx);

    let file = std::fs::read_to_string(named_file(&last_prompt(&user))).unwrap();
    assert!(
        !file.contains("InventedValue1") && !file.contains("SGVyZSBp"),
        "a secret reached the file the agent reads"
    );
    assert!(file.contains("[masked]"), "it is masked, not dropped");
    assert!(
        file.contains("request 120 served"),
        "the rest is there whole"
    );
}

#[gpui_kit::test]
fn the_file_token_is_removed_as_one_and_its_file_is_not_sent(cx: &mut TestAppContext) {
    let user = user();
    let lead = start(&user, "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    paste(&long_log(), "ctrl-v", cx);
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["chip:pasted-text.txt"]);

    cx.simulate_keystrokes("backspace");
    frames(cx);
    assert!(
        pieces(&reply, cx).is_empty(),
        "one Backspace takes it whole"
    );

    cx.simulate_input("only words");
    send(&shell, &user, cx);
    let prompt = last_prompt(&user);
    assert!(!prompt.contains("Pasted text"), "{prompt}");
}

#[gpui_kit::test]
fn a_sent_pasted_text_shows_its_token_and_opens_read_only(cx: &mut TestAppContext) {
    let user = user();
    let lead = start(&user, "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let page = page_of(&shell, cx);
    focus(cx);
    cx.simulate_input("here: ");
    paste(&long_log(), "ctrl-v", cx);
    settle(cx);
    send(&shell, &user, cx);
    until(&shell, cx, "the daemon recorded the message", |cx| {
        blocks(&page, cx).iter().any(
            |b| matches!(b, Block::Message { text, .. } if text.contains("Pasted text 1") && text.contains("read it at")),
        )
    });
    settle(cx);
    let at = blocks(&page, cx)
        .iter()
        .rposition(|b| matches!(b, Block::Message { text, .. } if text.contains("Pasted text 1")))
        .unwrap();

    let token = chip(&format!("row-{at}-message-tokens"), 0);
    assert_eq!(
        cx.update(|window, _| window.find(token.clone()).label().map(str::to_owned)),
        Some("pasted-text.txt".into()),
        "the message shows the token, not 4,000 characters"
    );
    assert!(!has("pasted-text-modal", cx));
    cx.update(|window, cx| window.click(token, cx));
    until(&shell, cx, "the text is opened", |cx| {
        has("pasted-text-modal", cx)
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, _| window.find("pasted-text-line-0").label().map(str::to_owned)),
        Some("2026-11-15 INFO request 1 served".into()),
        "its text is there, read-only"
    );
}

#[gpui_kit::test]
fn a_long_paste_in_the_project_composer_goes_with_the_first_message(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("atlas".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(cx);
    let _composer: Entity<Composer> = shell
        .read_with(cx, |s, _| s.composer_of("atlas").cloned())
        .expect("atlas's page has a composer");

    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input("work briefly on ");
    paste(&long_log(), "ctrl-v", cx);
    settle(cx);
    cx.update(|window, cx| window.click("send", cx));
    eventually("the agent got its first prompt", || {
        !user.received_requests("session/prompt").is_empty()
    });

    let prompt: Value = user.received_requests("session/prompt")[0]["params"]["prompt"].clone();
    let text: String = prompt
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect();
    assert!(text.contains("[Pasted text 1: pasted-text.txt"), "{text}");
    assert_eq!(
        std::fs::read_to_string(named_file(&text)).unwrap(),
        long_log()
    );
}
