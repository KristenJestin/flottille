//! The scenarios of #120, the window's polish: one running indicator, a composer that grows, a
//! project click that keeps its tree, pasted images. On GPUI's test platform in front of a real
//! daemon whose agents are the fake ACP agent, with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_ui::state::{icon_id, pill_id};
use flottille_ui::toggle_id;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::{Reply, SessionPage};
use flottille_window::tree::{project_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    ClipboardItem, Entity, Image, ImageFormat, TestAppContext, VisualTestContext, px, size,
};
use serde_json::{Value, json};

/// A valid 8 × 8 PNG, so that the window can draw its thumbnail.
const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR4nGM4YWODFTEMLQkAZZlQAVIPr1MAAAAASUVORK5CYII=";

fn png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(PNG_BASE64)
        .unwrap()
}

fn clipboard_png() -> ClipboardItem {
    ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png()))
}

/// A user whose fake agent takes images (`images`) or not, and whose turns stay a while.
fn user(images: bool) -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": images, "turns": [
        {"when": "think a while", "do": [
            {"think": "Weighing the options at some length, as one does."}, {"wait": 30000}]},
        {"when": "write a while", "do": [
            {"say": "A fairly long sentence, streamed so that the window sees it at once."},
            {"wait": 30000}]},
        {"when": "read a while", "do": [
            {"update": {"sessionUpdate": "tool_call", "toolCallId": "call-1",
                "title": "mcp__flottille__fs_read", "kind": "read", "status": "in_progress",
                "rawInput": {"path": "README.md"}}},
            {"wait": 30000}]},
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

fn send(user: &User, agent: &str, message: &str) {
    user.call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

fn cancel(user: &User, agent: &str) {
    user.call("agent.cancel", json!({"agent": agent})).unwrap();
    eventually("the turn ends", || state_of(user, agent) == "idle");
}

fn window<'a>(
    user: &User,
    connect: Connect,
    width: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(width), px(800.)));
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

/// A frame of the clock and a few of drawing: what a window that is looked at does while a
/// block folds away.
fn moving(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    frames(cx);
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
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
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

fn status_line(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find("reply-status")
            .map(|found| found.label().unwrap_or_default().to_owned())
    })
}

// 1. One place for "running".

#[gpui_kit::test]
fn a_running_agent_is_an_icon_in_the_rail_and_no_text_badge_anywhere(cx: &mut TestAppContext) {
    let user = user(false);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);

    send(&user, &lead, "think a while");
    until(
        &shell,
        cx,
        "the turn runs, in the composer and the rail",
        |cx| {
            reply.read_with(cx, |r, _| r.is_running())
                && cx.update(|window, _| !window.find_all(icon_id(AgentState::Running)).is_empty())
        },
    );
    settle(cx);

    cx.update(|window, _| {
        assert!(
            window.try_find(pill_id(AgentState::Running)).is_none(),
            "no RUNNING badge: not in the header, the rail or the sub-agents"
        );
        assert!(
            !window.find_all(icon_id(AgentState::Running)).is_empty(),
            "the icon stays"
        );
    });
    cancel(&user, &lead);
}

#[gpui_kit::test]
fn the_status_line_says_what_the_agent_does_and_goes_with_the_turn(cx: &mut TestAppContext) {
    let user = user(false);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    assert_eq!(status_line(cx), None, "nothing runs");

    for (message, line) in [
        ("think a while", "Thinking…"),
        ("write a while", "Writing…"),
        ("read a while", "Running fs_read…"),
    ] {
        send(&user, &lead, message);
        until(&shell, cx, line, |cx| {
            frames(cx);
            status_line(cx).as_deref() == Some(line)
        });
        cancel(&user, &lead);
        until(&shell, cx, "the line goes with the turn", |cx| {
            moving(cx);
            status_line(cx).is_none() && !reply.read_with(cx, |r, _| r.is_running())
        });
    }

    // A turn that ends by itself.
    send(&user, &lead, "work briefly");
    until(&shell, cx, "Thinking… while it works", |cx| {
        frames(cx);
        status_line(cx).as_deref() == Some("Thinking…")
    });
    until(&shell, cx, "the line goes when the turn ends", |cx| {
        moving(cx);
        status_line(cx).is_none()
    });
}

// 2. The composer grows.

fn reply_height(reply: &Entity<Reply>, text: &str, cx: &mut VisualTestContext) -> f32 {
    let field = reply.read_with(cx, |r, _| r.field().clone());
    field.update_in(cx, |field, window, cx| field.set_value(text, window, cx));
    frames(cx);
    cx.update(|window, _| f32::from(window.find("reply-field").bounds().size.height))
}

#[gpui_kit::test]
fn the_reply_grows_with_its_lines_up_to_ten_then_scrolls_and_shrinks_back(cx: &mut TestAppContext) {
    let user = user(false);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    let lines = |n: usize| vec!["a line of the answer"; n].join("\n");

    // Padding 12 and a border of 1 above and below, a line is 21 px, and the empty field is as high
    // as it was.
    let empty = reply_height(&reply, "", cx);
    assert_eq!(empty, 60., "an empty reply keeps the height it had");
    let heights: Vec<f32> = [1, 2, 3, 4, 10, 11, 40]
        .map(|n| reply_height(&reply, &lines(n), cx))
        .into();
    assert_eq!(
        heights,
        [60., 68., 89., 110., 236., 236., 236.],
        "one, two, three, four, ten, eleven and forty lines"
    );
    assert_eq!(reply_height(&reply, &lines(2), cx), 68., "it shrinks back");

    // Typed, as a user does, then sent: the field is empty again and as low as it was.
    reply_height(&reply, "", cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("one");
    for line in ["two", "three", "four"] {
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input(line);
    }
    frames(cx);
    let typed = cx.update(|window, _| f32::from(window.find("reply-field").bounds().size.height));
    assert_eq!(typed, 110., "four typed lines");
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    eventually("the reply is taken", || {
        user.received_requests("session/prompt").len() >= 2
    });
}

// 3. Clicking a project does not fold it.

#[gpui_kit::test]
fn clicking_a_project_opens_its_home_and_leaves_its_tree_as_it_was(cx: &mut TestAppContext) {
    let user = user(false);
    project(&user, "atlas");
    start(&user, "atlas", "hello");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the project is in the tree", |cx| {
        cx.update(|window, _| window.try_find(project_id("atlas")).is_some())
    });
    let node = shell
        .read_with(cx, |s, _| s.project("atlas").cloned())
        .unwrap();
    assert!(node.read_with(cx, |n, _| n.is_open()), "it starts unfolded");

    cx.update(|window, cx| window.click(project_id("atlas"), cx));
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == "atlas")
        })
    });
    assert!(
        node.read_with(cx, |n, _| n.is_open()),
        "the click opened its home and left the tree unfolded"
    );

    // Only the chevron folds, and unfolds, and opens no page.
    cx.update(|window, cx| window.click(toggle_id(project_id("atlas")), cx));
    assert!(!node.read_with(cx, |n, _| n.is_open()), "the chevron folds");
    cx.update(|window, cx| window.click(toggle_id(project_id("atlas")), cx));
    assert!(node.read_with(cx, |n, _| n.is_open()), "and unfolds");
    cx.update(|window, cx| window.click(toggle_id(project_id("atlas")), cx));
    cx.update(|window, cx| window.click(project_id("atlas"), cx));
    assert!(
        !node.read_with(cx, |n, _| n.is_open()),
        "a click on the name leaves a folded project folded"
    );
}

// 4. Pasting images.

/// Pastes the clipboard in the reply's field, focused as a user would.
fn paste(item: ClipboardItem, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.write_to_clipboard(item);
    cx.simulate_keystrokes("ctrl-v");
    frames(cx);
}

fn attachments(reply: &Entity<Reply>, cx: &mut VisualTestContext) -> usize {
    reply.read_with(cx, |r, _| r.attachments().len())
}

fn sent_images(user: &User, agent: &str) -> Vec<Value> {
    user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "agent.prompted" && e["data"].get("images").is_some())
        .cloned()
        .collect()
}

#[gpui_kit::test]
fn a_pasted_image_is_a_thumbnail_with_a_remove_button_and_goes_as_an_image_block(
    cx: &mut TestAppContext,
) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);

    paste(clipboard_png(), cx);
    assert_eq!(attachments(&reply, cx), 1);
    // It unfolds into the composer first: its button is there to be clicked once it has.
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find("attachment-0").is_some(), "its thumbnail");
        assert!(
            window.try_find("attachment-remove-0").is_some(),
            "its remove button"
        );
    });

    // Removed, then pasted again.
    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    frames(cx);
    assert_eq!(attachments(&reply, cx), 0);
    cx.update(|window, _| assert!(window.try_find("attachment-0").is_none()));
    paste(clipboard_png(), cx);
    assert_eq!(attachments(&reply, cx), 1);

    // Sent with a message: an image block after the text.
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("what is this?");
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the daemon took the message", |cx| {
        reply.read_with(cx, |r, cx| {
            !r.is_sending() && r.field().read(cx).value().is_empty()
        })
    });
    assert_eq!(
        attachments(&reply, cx),
        0,
        "what was sent leaves the composer"
    );
    eventually("the agent got the prompt", || {
        user.received_requests("session/prompt").len() >= 2
    });
    let prompt = user.received_requests("session/prompt")[1]["params"]["prompt"].clone();
    assert!(
        prompt[0]["text"]
            .as_str()
            .unwrap()
            .contains("what is this?")
    );
    assert_eq!(prompt[1]["type"], "image");
    assert_eq!(prompt[1]["mimeType"], "image/png");
    {
        use base64::Engine as _;
        assert_eq!(
            prompt[1]["data"],
            base64::engine::general_purpose::STANDARD.encode(png())
        );
    }

    // The conversation shows it, and shows it again after a restart: a page opened anew reads
    // the image back from the daemon.
    let page = page_of(&shell, cx);
    until(&shell, cx, "the conversation shows the image", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Message { images, .. } if images.len() == 1))
    });
    let names = sent_images(&user, &lead);
    assert_eq!(names.len(), 1);
}

#[gpui_kit::test]
fn an_image_sent_alone_is_a_message(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);

    paste(clipboard_png(), cx);
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the daemon took the image", |cx| {
        reply.read_with(cx, |r, _| !r.is_sending() && r.attachments().is_empty())
    });
    eventually("the agent got the prompt", || {
        user.received_requests("session/prompt").len() >= 2
    });
}

#[gpui_kit::test]
fn an_image_for_an_agent_that_takes_none_is_a_toast_and_the_text_stays(cx: &mut TestAppContext) {
    let user = user(false);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    until(&shell, cx, "the page knows the agent takes none", |cx| {
        page_of(&shell, cx).read_with(cx, |p, _| {
            p.tree().first().is_some_and(|a| a.images == Some(false))
        })
    });

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("keep this text");
    paste(clipboard_png(), cx);

    assert_eq!(attachments(&reply, cx), 0, "nothing is attached");
    assert_eq!(
        toasts(&shell, cx),
        [(
            "The picture was not attached".to_owned(),
            "This agent takes no images.".to_owned()
        )]
    );
    cx.update(|window, _| assert!(window.try_find("reply-error").is_none(), "no line"));
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "keep this text"
    );
}

#[gpui_kit::test]
fn an_image_dropped_from_a_file_works_like_a_pasted_one(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    let file = user.path("shot.png");
    std::fs::write(&file, png()).unwrap();

    reply.update_in(cx, |reply, _, cx| reply.attach_file(&file, cx));
    frames(cx);
    assert_eq!(attachments(&reply, cx), 1);
    cx.update(|window, _| assert!(window.try_find("attachment-0").is_some()));

    // Not an image: said, and nothing is attached.
    let text = user.path("notes.txt");
    std::fs::write(&text, "not a picture").unwrap();
    reply.update_in(cx, |reply, _, cx| reply.attach_file(&text, cx));
    frames(cx);
    assert_eq!(attachments(&reply, cx), 1);
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|(_, why)| why.contains("png, jpeg, gif and webp"))
    );
}

#[gpui_kit::test]
fn a_clipboard_with_an_image_and_its_url_attaches_the_image_and_pastes_no_text(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{ClipboardEntry, ClipboardString};
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("keep this");

    // A copy in a browser: the picture, and the address of the picture as text.
    let copied = ClipboardItem {
        entries: vec![
            ClipboardEntry::String(ClipboardString::new("https://example.org/p.png".into())),
            ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, png())),
        ],
    };
    paste(copied, cx);

    assert_eq!(attachments(&reply, cx), 1, "the image wins over the text");
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "keep this",
        "and the text is not pasted"
    );

    // A clipboard with text only still goes to the field.
    paste(ClipboardItem::new_string(" and more".into()), cx);
    assert_eq!(attachments(&reply, cx), 1);
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "keep this and more"
    );
}

#[gpui_kit::test]
fn a_send_removes_the_thumbnails_it_sent_and_not_those_changed_meanwhile(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    let other: Vec<u8> = png().into_iter().rev().collect();

    // Sent with picture A; while the daemon answers, B is added.
    reply.update_in(cx, |reply, window, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png()), cx);
        reply.send(window, cx);
        reply.attach(Image::from_bytes(ImageFormat::Png, other.clone()), cx);
        let key = reply.pending()[0].key;
        reply.replied(key, Ok(accepted()), cx);
    });

    let left = reply.read_with(cx, |r, _| {
        r.attachments()
            .iter()
            .map(|i| i.bytes.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(left, vec![other], "picture B was not sent: it stays");
}

#[gpui_kit::test]
fn the_image_shows_again_when_the_page_is_opened_after_a_restart(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    use base64::Engine as _;
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "look at this", "images": [
            {"mime": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png())}]}),
    )
    .unwrap();
    eventually("the turn is played", || {
        sent_images(&user, &lead).len() == 1 && state_of(&user, &lead) == "idle"
    });

    // A window that never saw the message live: it reads the conversation, and the image.
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let page = page_of(&shell, cx);
    until(&shell, cx, "the conversation shows the image", |cx| {
        frames(cx);
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Message { images, .. } if images.len() == 1))
            && page.read_with(cx, |p, _| p.pictures_read() == 1)
    });
}

#[gpui_kit::test]
fn a_pasted_picture_is_drawn_inside_its_bubble_above_the_text(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    let page = page_of(&shell, cx);

    paste(clipboard_png(), cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("look at this");
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the message carries its picture", |cx| {
        blocks(&page, cx).iter().any(
            |b| matches!(b, Block::Message { images, text, .. } if images.len() == 1 && text == "look at this"),
        )
    });
    settle(cx);

    let at = blocks(&page, cx)
        .iter()
        .position(|b| matches!(b, Block::Message { images, .. } if images.len() == 1))
        .unwrap();
    cx.update(|window, _| {
        let bubble = window.find(format!("row-{at}-bubble")).bounds();
        let image = window.find(format!("row-{at}-image-0")).bounds();
        let text = window.find(format!("row-{at}-message-text")).bounds();
        assert!(
            bubble.contains(&image.origin) && bubble.contains(&image.bottom_right()),
            "the picture is inside the bubble: {image:?} in {bubble:?}"
        );
        assert!(
            bubble.contains(&text.origin),
            "the text is in the same bubble"
        );
        assert!(
            image.bottom() <= text.top(),
            "the picture sits above the text: {image:?} over {text:?}"
        );
    });
}

// 5. What the composer says, and what moves in it (#396).

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

/// The height of the whole composer, as drawn now.
fn composer_height(cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, _| f32::from(window.find("reply").bounds().size.height))
}

fn attach_png(reply: &Entity<Reply>, cx: &mut VisualTestContext) {
    reply.update_in(cx, |reply, _, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png()), cx)
    });
}

#[gpui_kit::test]
fn a_message_carries_up_to_n_images(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);

    for _ in 0..20 {
        attach_png(&reply, cx);
    }
    frames(cx);
    assert_eq!(attachments(&reply, cx), 20, "the cap is taken");
    assert_eq!(toasts(&shell, cx), []);

    // And it all goes to the daemon in one message.
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the daemon took the twenty", |cx| {
        reply.read_with(cx, |r, _| !r.is_sending() && r.attachments().is_empty())
    });
    eventually("the agent got twenty images", || {
        user.received_requests("session/prompt")
            .get(1)
            .is_some_and(|p| {
                p["params"]["prompt"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|b| b["type"] == "image")
                    .count()
                    == 20
            })
    });
}

#[gpui_kit::test]
fn too_many_images_is_a_toast_and_the_composer_keeps_its_height(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    for _ in 0..20 {
        attach_png(&reply, cx);
    }
    settle(cx);
    let before = composer_height(cx);

    attach_png(&reply, cx);
    frames(cx);
    assert_eq!(
        composer_height(cx),
        before,
        "not a pixel taller on the first frame"
    );
    settle(cx);

    assert_eq!(attachments(&reply, cx), 20);
    assert_eq!(
        toasts(&shell, cx),
        [(
            "The picture was not attached".to_owned(),
            "A message carries 20 images at most.".to_owned()
        )]
    );
    assert_eq!(composer_height(cx), before, "nor once it has settled");
    cx.update(|window, _| assert!(window.try_find("reply-error").is_none(), "no line"));
}

#[gpui_kit::test]
fn an_image_over_five_mib_is_a_toast_and_the_composer_keeps_its_height(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    let before = composer_height(cx);

    // Not a picture the window can read, so nothing can make it smaller.
    let heavy = vec![0u8; 5 * 1024 * 1024 + 1];
    reply.update_in(cx, |reply, _, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, heavy), cx)
    });
    settle(cx);

    assert_eq!(attachments(&reply, cx), 0);
    assert_eq!(
        toasts(&shell, cx),
        [(
            "The picture was not attached".to_owned(),
            "An image can be 5 MiB at most.".to_owned()
        )]
    );
    assert_eq!(composer_height(cx), before);
}

#[gpui_kit::test]
fn a_file_that_cannot_be_read_is_a_toast_and_the_composer_keeps_its_height(
    cx: &mut TestAppContext,
) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    let before = composer_height(cx);

    let gone = user.path("gone.png");
    reply.update_in(cx, |reply, _, cx| reply.attach_file(&gone, cx));
    settle(cx);

    assert_eq!(
        toasts(&shell, cx),
        [(
            "The picture was not attached".to_owned(),
            "That file could not be read.".to_owned()
        )]
    );
    assert_eq!(composer_height(cx), before);
}

/// What the daemon says when it takes a message, made up.
fn accepted() -> flottille_window::session::Accepted {
    flottille_window::session::Accepted {
        delivery: "d-made-up".into(),
        after: 0,
    }
}

#[gpui_kit::test]
fn a_refused_send_is_said_on_the_message_and_the_composer_keeps_its_height_and_its_text(
    cx: &mut TestAppContext,
) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    reply.update_in(cx, |reply, window, cx| {
        reply
            .field()
            .update(cx, |field, cx| field.set_value("the first one", window, cx));
        reply.send(window, cx);
    });
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("keep this text");
    settle(cx);
    let before = composer_height(cx);

    let key = reply.read_with(cx, |r, _| r.pending()[0].key);
    reply.update(cx, |r, cx| {
        r.replied(key, Err("refused: made-up reason".to_owned()), cx)
    });
    settle(cx);

    // Said once, on the message that was refused: no toast.
    assert_eq!(toasts(&shell, cx), []);
    assert_eq!(
        reply.read_with(cx, |r, _| r.pending()[0].refused.clone()),
        Some("refused: made-up reason".to_owned())
    );
    assert_eq!(composer_height(cx), before);
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "keep this text"
    );
}

#[gpui_kit::test]
fn a_refused_interruption_is_a_toast_and_the_composer_keeps_its_height(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    let before = composer_height(cx);

    reply.update(cx, |r, cx| {
        r.interrupted(Some("refused: nothing runs".to_owned()), cx)
    });
    settle(cx);

    assert_eq!(
        toasts(&shell, cx),
        [(
            "The turn was not interrupted".to_owned(),
            "refused: nothing runs".to_owned()
        )]
    );
    assert_eq!(composer_height(cx), before);
}

/// A PNG of `width` x `height` pixels.
fn png_of(width: u32, height: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x / 12) as u8, (y / 12) as u8, ((x + y) / 24) as u8])
    })
    .write_to(&mut out, image::ImageFormat::Png)
    .unwrap();
    out.into_inner()
}

#[gpui_kit::test]
fn a_big_image_is_scaled_down_when_it_is_attached(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);

    reply.update_in(cx, |reply, _, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png_of(3000, 1500)), cx)
    });
    cx.run_until_parked();

    let (format, bytes) = reply.read_with(cx, |r, _| {
        (r.attachments()[0].format, r.attachments()[0].bytes.clone())
    });
    assert_eq!(format, ImageFormat::Png);
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!((width, height), (2000, 1000));
}

#[gpui_kit::test]
fn a_big_pasted_picture_does_not_hold_the_window(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);

    reply.update_in(cx, |reply, _, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png_of(3000, 1500)), cx)
    });

    // The attach has returned and the scaling has not run: its place is held by a skeleton.
    assert_eq!(attachments(&reply, cx), 0);
    assert!(reply.read_with(cx, |r, _| r.is_scaling()));
    cx.update(|window, cx| window.render_frame(cx));
    cx.update(|window, _| assert!(window.try_find("attachment-scaling-0").is_some()));

    // Once the scaling is done the thumbnail is there, scaled, and the place is not.
    cx.run_until_parked();
    assert_eq!(attachments(&reply, cx), 1);
    assert!(!reply.read_with(cx, |r, _| r.is_scaling()));
    let bytes = reply.read_with(cx, |r, _| r.attachments()[0].bytes.clone());
    let size = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!(size, (2000, 1000));
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find("attachment-0").is_some());
        assert!(window.try_find("attachment-scaling-0").is_none());
    });
    assert_eq!(toasts(&shell, cx), []);
}

#[gpui_kit::test]
fn sending_while_a_picture_still_scales_is_refused_in_a_toast(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);

    reply.update_in(cx, |reply, window, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png_of(3000, 1500)), cx);
        reply.send(window, cx);
    });

    assert!(!reply.read_with(cx, |r, _| r.is_sending()));
    assert_eq!(
        toasts(&shell, cx),
        [(
            "The message was not sent".to_owned(),
            "A picture is still being scaled down: send once it is there.".to_owned()
        )]
    );
    cx.run_until_parked();
    assert_eq!(attachments(&reply, cx), 1);
}

#[gpui_kit::test]
fn an_attached_image_unfolds_and_folds_away(cx: &mut TestAppContext) {
    let user = user(true);
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, reply, cx) = open_session(&user, &lead, cx);
    let bare = composer_height(cx);

    // It unfolds: the composer grows over several frames, not at once.
    attach_png(&reply, cx);
    moving(cx);
    let first = composer_height(cx);
    settle(cx);
    let full = composer_height(cx);
    assert!(
        full > bare + 40.,
        "the thumbnail's room: {bare} then {full}"
    );
    assert!(
        first < full,
        "still unfolding after a frame: {first} of {full}"
    );
    cx.update(|window, _| assert!(window.try_find("attachment-0").is_some()));
    // The id the final check looks for is the strip's: it is drawn, with room, while it is there.
    cx.update(|window, _| {
        assert!(
            window
                .try_find("attachments")
                .is_some_and(|a| a.bounds().size.height > px(1.)),
            "the strip is drawn under the id the fold check looks for"
        )
    });

    // It folds away: still drawn a frame after, gone once it has.
    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    moving(cx);
    let folding = composer_height(cx);
    assert!(
        folding > bare && folding <= full,
        "folding: {bare} < {folding} <= {full}"
    );
    assert_eq!(attachments(&reply, cx), 0);
    settle(cx);
    assert_eq!(composer_height(cx), bare, "back to the bare composer");
    cx.update(|window, _| {
        assert!(
            window
                .try_find("attachments")
                .is_none_or(|a| a.bounds().size.height <= px(1.))
        )
    });
}
