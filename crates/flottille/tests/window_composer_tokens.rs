//! The scenarios of #773 and #846, one mechanism for both: a picture and a command are chips in
//! the composer's text, where they were put, and the conversation shows the message the same
//! way. On GPUI's test platform in front of a real daemon whose agents are the fake ACP agent,
//! with a throwaway user. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::{Reply, SessionPage};
use flottille_window::tree::session_id;
use gpui_design_system::Piece;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    ClipboardItem, Entity, Image, ImageFormat, TestAppContext, VisualTestContext, px, size,
};
use serde_json::{Value, json};

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 9) as u8, ((x + y) * 3) as u8])
    })
    .write_to(&mut out, image::ImageFormat::Png)
    .unwrap();
    out.into_inner()
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn clipboard_png(width: u32) -> ClipboardItem {
    ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png(width, 8)))
}

/// A user whose fake agent takes images.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true, "turns": [
        {"when": "work briefly", "do": [{"wait": 1500}, {"say": "done"}]}
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

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
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
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
}

fn window<'a>(
    user: &User,
    connect: Connect,
    extent: (f32, f32),
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(extent.0), px(extent.1)));
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
    let (shell, cx) = window(user, Arc::new(user.connector()), (1280., 800.), cx);
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

/// Pastes the clipboard in the reply's field, where its caret is.
fn paste(item: ClipboardItem, cx: &mut VisualTestContext) {
    cx.write_to_clipboard(item);
    cx.simulate_keystrokes("ctrl-v");
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

/// The widths of the pictures attached, in the order of their thumbnails.
fn widths(reply: &Entity<Reply>, cx: &mut VisualTestContext) -> Vec<u32> {
    reply.read_with(cx, |r, _| {
        r.attachments()
            .iter()
            .map(|image| {
                image::load_from_memory(&image.bytes)
                    .map(|i| i.width())
                    .unwrap_or_default()
            })
            .collect()
    })
}

fn caret_at(reply: &Entity<Reply>, at: usize, cx: &mut VisualTestContext) {
    reply.update_in(cx, |r, _, cx| {
        r.field()
            .clone()
            .update(cx, |f, cx| f.set_selected_range(at..at, cx))
    });
}

fn focus(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("reply-field", cx));
    frames(cx);
}

/// The blocks of the last prompt the agent received.
fn last_prompt(user: &User) -> Value {
    user.received_requests("session/prompt").last().unwrap()["params"]["prompt"].clone()
}

/// The project's command `ship`.
fn with_ship(user: &User) {
    user.call(
        "command.add",
        json!({"project": "atlas", "name": "ship", "description": "Ship it",
            "instructions": "Push the branch, then open its pull request."}),
    )
    .unwrap();
}

/// The session's composer, once it knows the project's commands.
fn knowing_commands(shell: &Entity<Shell>, reply: &Entity<Reply>, cx: &mut VisualTestContext) {
    until(shell, cx, "the composer knows the commands", |cx| {
        reply.read_with(cx, |r, _| r.knows_commands())
    });
    settle(cx);
}

/// The user's message that used commands, and where it is.
fn commanded(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<(usize, Block)> {
    blocks(page, cx).into_iter().enumerate().find(|(_, block)| {
        matches!(
            block,
            Block::Message {
                commands: Some(_),
                ..
            }
        )
    })
}

#[gpui_kit::test]
fn a_pasted_picture_puts_its_chip_at_the_caret_and_numbers_its_thumbnail(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    cx.simulate_input("this one  is fine");
    caret_at(&reply, 9, cx);

    paste(clipboard_png(8), cx);
    settle(cx);

    assert_eq!(
        pieces(&reply, cx),
        ["text:this one ", "chip:Image 1", "text: is fine"]
    );
    let detail = reply.read_with(cx, |r, cx| match &r.tokens().read(cx).value(cx)[1] {
        Piece::Token(token) => token.look.detail.clone().unwrap_or_default().to_string(),
        Piece::Text(_) => String::new(),
    });
    assert!(
        detail.starts_with("image.png · "),
        "its name and size: {detail}"
    );
    assert_eq!(widths(&reply, cx), [8]);
    assert!(has("attachment-0", cx), "its thumbnail");
    assert_eq!(
        cx.update(|window, _| window
            .find("attachment-number-0")
            .label()
            .map(str::to_owned)),
        Some("Image 1".into()),
        "the thumbnail has the chip's number"
    );
}

#[gpui_kit::test]
fn backspace_removes_a_chip_whole_and_its_picture(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    cx.simulate_input("look ");
    paste(clipboard_png(8), cx);
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["text:look ", "chip:Image 1"]);

    cx.simulate_keystrokes("backspace");
    frames(cx);
    assert_eq!(pieces(&reply, cx), ["text:look "], "the chip went whole");
    assert!(widths(&reply, cx).is_empty(), "and its picture with it");
    settle(cx);
    assert!(!has("attachment-0", cx), "the thumbnail folded away");
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "look "
    );
}

#[gpui_kit::test]
fn removing_a_thumbnail_removes_its_chip(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    paste(clipboard_png(8), cx);
    cx.simulate_input(" here");
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["chip:Image 1", "text: here"]);

    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    frames(cx);
    assert!(widths(&reply, cx).is_empty(), "the picture went");
    assert_eq!(pieces(&reply, cx), ["text: here"], "and its chip");
    settle(cx);
    assert!(!has("attachment-0", cx));
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        " here",
        "the chip's text left once it folded away"
    );
}

#[gpui_kit::test]
fn the_prompt_interleaves_text_markers_and_pictures_in_their_order(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let sent = user.received_requests("session/prompt").len();
    focus(cx);
    cx.simulate_input("this one ");
    paste(clipboard_png(8), cx);
    cx.simulate_input(" is fine, but in ");
    paste(clipboard_png(9), cx);
    cx.simulate_input(" the text overflows");
    settle(cx);
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the agent got the message", |_| {
        user.received_requests("session/prompt").len() > sent
    });

    let prompt = last_prompt(&user);
    let kinds: Vec<&str> = prompt
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["text", "image", "text", "image", "text"],
        "{prompt}"
    );
    assert!(
        prompt[0]["text"]
            .as_str()
            .unwrap()
            .ends_with("this one [Image 1: image.png]"),
        "{prompt}"
    );
    assert_eq!(prompt[1]["data"], base64_of(&png(8, 8)));
    assert_eq!(prompt[2]["text"], " is fine, but in [Image 2: image.png]");
    assert_eq!(prompt[3]["data"], base64_of(&png(9, 8)));
    assert_eq!(prompt[4]["text"], " the text overflows");
}

#[gpui_kit::test]
fn a_message_in_the_conversation_shows_its_chips_and_opens_the_picture(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let page = page_of(&shell, cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "this one [Image 1: shot.png] overflows",
            "images": [{"mime": "image/png", "data": base64_of(&png(60, 40))}]}),
    )
    .unwrap();
    until(
        &shell,
        cx,
        "the message shows with its picture read",
        |cx| {
            page.read_with(cx, |p, _| p.pictures_read() == 1)
                && blocks(&page, cx)
                    .iter()
                    .any(|b| matches!(b, Block::Message { images, .. } if images.len() == 1))
        },
    );
    settle(cx);
    let at = blocks(&page, cx)
        .iter()
        .rposition(|b| matches!(b, Block::Message { images, .. } if images.len() == 1))
        .unwrap();

    let chip = chip(&format!("row-{at}-message-tokens"), 0);
    assert_eq!(
        cx.update(|window, _| window.find(chip.clone()).label().map(str::to_owned)),
        Some("Image 1".into()),
        "the chip is in the message's text"
    );
    assert!(!has("picture-modal", cx));
    cx.update(|window, cx| window.click(chip, cx));
    settle(cx);
    assert!(has("picture-modal", cx), "the chip opens the picture large");
    assert!(has("picture-large-0", cx));
}

#[gpui_kit::test]
fn numbers_follow_the_order_of_the_chips_in_the_text(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    cx.simulate_input("a  b ");
    paste(clipboard_png(8), cx);
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["text:a  b ", "chip:Image 1"]);

    // A second picture put before the first: it is Image 1, the other Image 2.
    caret_at(&reply, 2, cx);
    paste(clipboard_png(9), cx);
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["text:a ", "chip:Image 1", "text: b ", "chip:Image 2"]
    );
    assert_eq!(
        widths(&reply, cx),
        [9, 8],
        "the thumbnails follow the chips"
    );
    assert_eq!(
        cx.update(|window, _| window
            .find("attachment-number-1")
            .label()
            .map(str::to_owned)),
        Some("Image 2".into())
    );

    // The first removed: the other is Image 1 again.
    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["text:a  b ", "chip:Image 1"]);
    assert_eq!(widths(&reply, cx), [8]);
}

#[gpui_kit::test]
fn a_command_is_a_token_in_the_composer(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    with_ship(&user);
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    knowing_commands(&shell, &reply, cx);
    focus(cx);

    // Picked from the list.
    cx.simulate_input("/sh");
    until(&shell, cx, "the list offers it", |cx| {
        reply.read_with(cx, |r, _| r.slash_entries().len() == 1)
    });
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["chip:/ship", "text: "]);
    // Typed by hand, it becomes one once the word is finished.
    cx.simulate_input("then /ship");
    frames(cx);
    assert_eq!(pieces(&reply, cx), ["chip:/ship", "text: then /ship"]);
    cx.simulate_input(" now");
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["chip:/ship", "text: then ", "chip:/ship", "text: now"]
    );
    assert!(
        !has("reply-commands-line", cx),
        "no line of chips under the field"
    );
}

#[gpui_kit::test]
fn a_command_token_is_deleted_as_one(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    with_ship(&user);
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    knowing_commands(&shell, &reply, cx);
    focus(cx);
    cx.simulate_input("go /ship ");
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["text:go ", "chip:/ship", "text: "]);

    cx.simulate_keystrokes("backspace");
    cx.simulate_keystrokes("backspace");
    frames(cx);
    assert_eq!(pieces(&reply, cx), ["text:go "], "the chip went as one");
    settle(cx);
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "go "
    );
}

#[gpui_kit::test]
fn a_sent_command_shows_as_typed(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    with_ship(&user);
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    knowing_commands(&shell, &reply, cx);
    let page = page_of(&shell, cx);
    focus(cx);
    // The word finished, it is a chip and the `/` list is closed: Enter sends.
    cx.simulate_input("fix the test, then /ship ");
    settle(cx);
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the message shows", |cx| {
        commanded(&page, cx).is_some()
    });
    settle(cx);

    let (at, block) = commanded(&page, cx).unwrap();
    let Block::Message { text, .. } = block else {
        unreachable!()
    };
    assert_eq!(text, "fix the test, then /ship", "what the user typed");
    assert_eq!(
        cx.update(|window, _| {
            window
                .find(chip(&format!("row-{at}-message-tokens"), 0))
                .label()
                .map(str::to_owned)
        }),
        Some("/ship".into()),
        "the command is the same chip as in the composer"
    );
    // The agent received the command's text, never the bare `/ship`.
    let prompt = last_prompt(&user);
    let sent = prompt[0]["text"].as_str().unwrap();
    assert!(
        sent.contains("Push the branch") && sent.contains("[command ship]"),
        "{sent}"
    );
}

#[gpui_kit::test]
fn what_the_agent_received_is_not_shown(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    with_ship(&user);
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let page = page_of(&shell, cx);
    user.call("agent.send", json!({"agent": lead, "message": "/ship now"}))
        .unwrap();
    until(&shell, cx, "the message shows", |cx| {
        commanded(&page, cx).is_some()
    });
    settle(cx);
    let (at, _) = commanded(&page, cx).unwrap();

    for part in [
        "received-toggle",
        "received-part",
        "received",
        "received-text",
    ] {
        assert!(!has(format!("row-{at}-{part}"), cx), "no {part}");
    }
    assert!(
        has(format!("row-{at}-message-text"), cx),
        "what was typed shows"
    );
}

#[gpui_kit::test]
fn a_chip_pasted_twice_sends_its_picture_once(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    let sent = user.received_requests("session/prompt").len();
    focus(cx);
    cx.simulate_input("look ");
    paste(clipboard_png(8), cx);
    settle(cx);
    // The text and its chip copied, and pasted after it: the chip is there twice.
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_keystrokes("ctrl-c");
    let end = reply.read_with(cx, |r, cx| r.field().read(cx).value().len());
    caret_at(&reply, end, cx);
    cx.simulate_keystrokes("ctrl-v");
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["text:look ", "chip:Image 1", "text:look ", "chip:Image 1"]
    );
    assert_eq!(widths(&reply, cx), [8], "one picture");

    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the agent got the message", |_| {
        user.received_requests("session/prompt").len() > sent
    });
    let prompt = last_prompt(&user);
    let kinds: Vec<&str> = prompt
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["text", "image", "text"], "{prompt}");
    assert!(
        prompt[0]["text"]
            .as_str()
            .unwrap()
            .ends_with("look [Image 1: image.png]"),
        "{prompt}"
    );
    assert_eq!(
        prompt[2]["text"], "look Image 1",
        "the copy is its label, not a second marker"
    );
}

#[gpui_kit::test]
fn undoing_a_thumbnail_removal_brings_its_picture_back(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    focus(cx);
    paste(clipboard_png(8), cx);
    cx.simulate_input(" here");
    settle(cx);
    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["text: here"]);
    assert!(widths(&reply, cx).is_empty());

    focus(cx);
    cx.simulate_keystrokes("ctrl-z");
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "text: here"],
        "the chip came back"
    );
    assert_eq!(widths(&reply, cx), [8], "and its picture with it");
    assert!(has("attachment-0", cx), "its thumbnail");
}

/// Three pictures pasted one after the other, settled: their chips are `Image 1` to `Image 3`.
fn three_pictures(reply: &Entity<Reply>, cx: &mut VisualTestContext) {
    focus(cx);
    for width in [8, 9, 10] {
        paste(clipboard_png(width), cx);
    }
    settle(cx);
    assert_eq!(
        pieces(reply, cx),
        ["chip:Image 1", "chip:Image 2", "chip:Image 3"]
    );
    assert_eq!(widths(reply, cx), [8, 9, 10]);
}

/// The id of the chip `n` (from 0) of the reply's text.
fn chip_id(reply: &Entity<Reply>, n: usize, cx: &mut VisualTestContext) -> String {
    reply.read_with(cx, |r, cx| {
        r.tokens().read(cx).input().read(cx).tokens()[n]
            .token()
            .id()
            .to_string()
    })
}

#[gpui_kit::test]
fn removing_a_middle_picture_keeps_the_undo_history(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    three_pictures(&reply, cx);

    cx.update(|window, cx| window.click("attachment-remove-1", cx));
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2"],
        "the chip after the removed one is renumbered"
    );
    assert_eq!(widths(&reply, cx), [8, 10]);

    // One step undoes the removal: the renumbering was no step of its own.
    focus(cx);
    cx.simulate_keystrokes("ctrl-z");
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2", "chip:Image 3"],
        "the removed chip is back with its number, the last one is numbered back"
    );
    assert_eq!(widths(&reply, cx), [8, 9, 10], "and its picture with it");
    assert!(has("attachment-1", cx), "its thumbnail");
}

#[gpui_kit::test]
fn a_renumbered_chip_fades_to_its_new_number(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    three_pictures(&reply, cx);
    let second = chip_id(&reply, 1, cx);

    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    // Frame by frame: the chip that was the second one is the first now, and its ink moves.
    let ink = format!("reply-text-{second}-0-ink");
    let mut dipped = false;
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        dipped |= gpui_design_system::seen::seen(ink.clone()).is_some_and(|v| 0.0 < v && v < 1.0);
    }
    assert_eq!(pieces(&reply, cx), ["chip:Image 1", "chip:Image 2"]);
    assert!(dipped, "the ink of the renumbered chip moved, not a jump");
    assert_eq!(
        gpui_design_system::seen::seen(ink),
        Some(1.0),
        "and is whole again at rest"
    );
}

#[gpui_kit::test]
fn a_copied_chip_pasted_after_a_renumbering_keeps_its_new_number(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    three_pictures(&reply, cx);

    // The last chip is copied while it is "Image 3".
    let last = reply.read_with(cx, |r, cx| {
        r.tokens().read(cx).input().read(cx).tokens()[2].range()
    });
    reply.update_in(cx, |r, _, cx| {
        r.field()
            .clone()
            .update(cx, |f, cx| f.set_selected_range(last, cx))
    });
    cx.simulate_keystrokes("ctrl-c");

    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    settle(cx);
    assert_eq!(pieces(&reply, cx), ["chip:Image 1", "chip:Image 2"]);

    focus(cx);
    let end = reply.read_with(cx, |r, cx| r.field().read(cx).value().len());
    caret_at(&reply, end, cx);
    let copied = chip_id(&reply, 1, cx);
    cx.simulate_keystrokes("ctrl-v");
    // Frame by frame, as the paste settles: neither copy's ink ever dips, so no frame shows
    // the "Image 3" it was copied as.
    let inks = [1, 2].map(|n| format!("reply-text-{copied}-{n}-ink"));
    let mut sampled = false;
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        for ink in &inks {
            if let Some(v) = gpui_design_system::seen::seen(ink.clone()) {
                sampled = true;
                assert!(v >= 1.0, "{ink} dipped to {v}: a flash of the old number");
            }
        }
    }
    settle(cx);
    assert!(sampled, "the ink notes were looked at");
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2", "chip:Image 2"],
        "the copy is the number the chip has now, not the one it had when it was copied"
    );
    assert_eq!(widths(&reply, cx), [9, 10], "no picture was added");
}

#[gpui_kit::test]
fn redoing_a_removal_renumbers_the_chips(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    three_pictures(&reply, cx);

    cx.update(|window, cx| window.click("attachment-remove-1", cx));
    settle(cx);
    focus(cx);
    cx.simulate_keystrokes("ctrl-z");
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2", "chip:Image 3"]
    );

    cx.simulate_keystrokes("ctrl-y");
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2"],
        "at once, not a frame later"
    );
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2"],
        "the chip after the removed one is renumbered again by the redo"
    );
    assert_eq!(
        widths(&reply, cx),
        [8, 10],
        "and the picture left with its chip"
    );
}

#[gpui_kit::test]
fn cutting_a_chip_renumbers_the_others(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    three_pictures(&reply, cx);

    let middle = reply.read_with(cx, |r, cx| {
        r.tokens().read(cx).input().read(cx).tokens()[1].range()
    });
    reply.update_in(cx, |r, _, cx| {
        r.field()
            .clone()
            .update(cx, |f, cx| f.set_selected_range(middle, cx))
    });
    cx.simulate_keystrokes("ctrl-x");
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2"],
        "at once, not a frame later"
    );
    settle(cx);
    assert_eq!(
        pieces(&reply, cx),
        ["chip:Image 1", "chip:Image 2"],
        "the chip after the cut one is renumbered"
    );
    assert_eq!(widths(&reply, cx), [8, 10]);
}

#[gpui_kit::test]
fn a_typed_marker_without_a_picture_stays_text(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let page = page_of(&shell, cx);
    user.call(
        "agent.send",
        json!({"agent": lead,
            "message": "this one [Image 1: shot.png] and [Image 2: other.png] typed by hand",
            "images": [{"mime": "image/png", "data": base64_of(&png(60, 40))}]}),
    )
    .unwrap();
    until(&shell, cx, "the message shows with its picture", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Message { images, .. } if images.len() == 1))
    });
    settle(cx);
    let at = blocks(&page, cx)
        .iter()
        .rposition(|b| matches!(b, Block::Message { images, .. } if images.len() == 1))
        .unwrap();

    let tokens = format!("row-{at}-message-tokens");
    assert_eq!(
        cx.update(|window, _| window.find(chip(&tokens, 0)).label().map(str::to_owned)),
        Some("Image 1".into()),
        "the picture's marker is its chip"
    );
    assert!(
        !has(chip(&tokens, 1), cx),
        "a marker with no picture of its number is no chip"
    );
}

#[gpui_kit::test]
fn a_message_taken_back_keeps_twenty_pictures_at_most(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    let pictures: Vec<flottille_window::actions::Picture> = (0..21)
        .map(|n| flottille_window::actions::Picture {
            mime: "image/png".into(),
            bytes: png(8 + n, 8),
        })
        .collect();

    reply.update_in(cx, |reply, window, cx| {
        reply.restore("taken back", pictures, window, cx)
    });
    settle(cx);
    assert_eq!(
        widths(&reply, cx).len(),
        20,
        "a message carries 20 pictures at most"
    );
}
