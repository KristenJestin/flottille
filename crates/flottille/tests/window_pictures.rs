//! The scenarios of #353, a picture of the conversation opened large: a click on a thumbnail of a
//! message (or of the composer) opens it in the large modal, fitted to the window and never
//! scaled up, and the arrows go through the pictures of the message. On GPUI's test platform in
//! front of a real daemon whose agents are the fake ACP agent, with a throwaway user. No window
//! opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::{Reply, SessionPage};
use flottille_window::tree::session_id;
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

fn clipboard_png() -> ClipboardItem {
    ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png(8, 8)))
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

/// Sends `pictures` (png bytes) in one message to the session, and waits until its bubble is
/// drawn with them read: the index of the block that holds them.
fn say_with_pictures(
    user: &User,
    lead: &str,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    pictures: &[Vec<u8>],
) -> usize {
    let images: Vec<Value> = pictures
        .iter()
        .map(|bytes| json!({"mime": "image/png", "data": base64_of(bytes)}))
        .collect();
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "look at these", "images": images}),
    )
    .unwrap();
    let page = page_of(shell, cx);
    let before = page.read_with(cx, |p, _| p.pictures_read());
    until(shell, cx, "the pictures are read", |cx| {
        page.read_with(cx, |p, _| p.pictures_read() == before + pictures.len())
            && blocks(&page, cx).iter().any(
                |b| matches!(b, Block::Message { images, .. } if images.len() == pictures.len()),
            )
    });
    settle(cx);
    blocks(&page, cx)
        .iter()
        .rposition(|b| matches!(b, Block::Message { images, .. } if images.len() == pictures.len()))
        .unwrap()
}

/// A picture of `width` × `height` pixels, as the size it is drawn at on this window: a pixel is
/// `1 / scale` of a point on a screen that scales.
fn points(width: f32, height: f32, cx: &mut VisualTestContext) -> (f32, f32) {
    let scale = cx.update(|window, _| window.scale_factor());
    (width / scale, height / scale)
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label_of(id: &'static str, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id)
            .map(|found| found.label().unwrap_or_default().to_owned())
    })
}

#[gpui_kit::test]
fn a_click_on_a_picture_opens_it_large_in_a_modal(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let at = say_with_pictures(&user, &lead, &shell, cx, &[png(60, 40)]);
    assert!(
        !has("picture-modal", cx),
        "nothing is large before the click"
    );

    cx.update(|window, cx| window.click(format!("row-{at}-image-0"), cx));
    settle(cx);

    assert!(has("picture-modal", cx), "the picture opens in a modal");
    assert!(has("picture-modal-scrim", cx), "over the scrim");
    assert!(has("picture-large-0", cx), "its picture is drawn large");
    assert!(
        !has("picture-previous", cx) && !has("picture-next", cx),
        "one picture has no arrows"
    );
}

#[gpui_kit::test]
fn a_large_picture_fits_the_window_and_is_never_scaled_up(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let at = say_with_pictures(&user, &lead, &shell, cx, &[png(2000, 1400)]);
    cx.update(|window, cx| window.click(format!("row-{at}-image-0"), cx));
    settle(cx);

    // Larger than the window: scaled down, its proportions kept, a margin kept in the dialog.
    let (dialog, picture) = cx.update(|window, _| {
        (
            window.find("picture-modal").bounds(),
            window.find("picture-large-0").bounds(),
        )
    });
    let (w, h) = (
        f32::from(picture.size.width),
        f32::from(picture.size.height),
    );
    let own = points(2000., 1400., cx);
    assert!(
        w < own.0 && h < own.1,
        "scaled down from {own:?}: {w} × {h}"
    );
    assert!(
        (w / h - 2000. / 1400.).abs() < 0.01,
        "proportions kept: {w} × {h}"
    );
    let margin = px(16.);
    assert!(
        picture.left() >= dialog.left() + margin
            && picture.right() <= dialog.right() - margin
            && picture.top() >= dialog.top() + margin
            && picture.bottom() <= dialog.bottom() - margin,
        "a margin around it: {picture:?} in {dialog:?}"
    );
    let (cx_of, cy_of) = (
        f32::from(picture.center().x - dialog.center().x),
        f32::from(picture.center().y - dialog.center().y),
    );
    assert!(
        cx_of.abs() < 1. && cy_of.abs() < 40.,
        "centred: {cx_of} {cy_of}"
    );

    // Smaller than the window: at its own size, never more.
    cx.simulate_keystrokes("escape");
    settle(cx);
    let small = say_with_pictures(&user, &lead, &shell, cx, &[png(60, 40), png(60, 40)]);
    cx.update(|window, cx| window.click(format!("row-{small}-image-0"), cx));
    settle(cx);
    let picture = cx.update(|window, _| window.find("picture-large-0").bounds());
    assert_eq!(
        (
            f32::from(picture.size.width),
            f32::from(picture.size.height)
        ),
        points(60., 40., cx),
        "its own size"
    );
}

#[gpui_kit::test]
fn a_click_outside_or_escape_closes_the_picture(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let at = say_with_pictures(&user, &lead, &shell, cx, &[png(60, 40)]);

    cx.update(|window, cx| window.click(format!("row-{at}-image-0"), cx));
    settle(cx);
    assert!(has("picture-modal", cx));
    cx.update(|window, cx| {
        window.click_at("picture-modal-scrim", gpui_kit::point(px(3.), px(3.)), cx)
    });
    settle(cx);
    assert!(!has("picture-modal", cx), "a click outside closes it");

    cx.update(|window, cx| window.click(format!("row-{at}-image-0"), cx));
    settle(cx);
    assert!(has("picture-modal", cx));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!has("picture-modal", cx), "Escape closes it");

    cx.update(|window, cx| window.click(format!("row-{at}-image-0"), cx));
    settle(cx);
    cx.update(|window, cx| window.click("picture-modal-close", cx));
    settle(cx);
    assert!(!has("picture-modal", cx), "its close button closes it");
}

#[gpui_kit::test]
fn the_arrows_go_through_the_pictures_of_the_message(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (shell, _reply, cx) = open_session(&user, &lead, cx);
    let at = say_with_pictures(
        &user,
        &lead,
        &shell,
        cx,
        &[png(60, 40), png(50, 30), png(40, 20)],
    );

    cx.update(|window, cx| window.click(format!("row-{at}-image-1"), cx));
    settle(cx);
    assert_eq!(
        label_of("picture-modal-sentence", cx).as_deref(),
        Some("2 of 3")
    );
    let size_of = |cx: &mut VisualTestContext, n: usize| {
        cx.update(|window, _| {
            let b = window.find(format!("picture-large-{n}")).bounds();
            (f32::from(b.size.width), f32::from(b.size.height))
        })
    };
    assert_eq!(
        size_of(cx, 1),
        points(50., 30., cx),
        "the second picture is the one shown"
    );

    cx.simulate_keystrokes("right");
    settle(cx);
    assert_eq!(
        label_of("picture-modal-sentence", cx).as_deref(),
        Some("3 of 3")
    );
    assert_eq!(size_of(cx, 2), points(40., 20., cx));
    assert!(!has("picture-large-1", cx), "the cross-fade is over");

    cx.update(|window, cx| window.click("picture-previous", cx));
    settle(cx);
    cx.update(|window, cx| window.click("picture-previous", cx));
    settle(cx);
    assert_eq!(
        label_of("picture-modal-sentence", cx).as_deref(),
        Some("1 of 3")
    );

    cx.simulate_keystrokes("left");
    settle(cx);
    assert_eq!(
        label_of("picture-modal-sentence", cx).as_deref(),
        Some("1 of 3"),
        "the first has no one before it"
    );
    cx.update(|window, cx| window.click("picture-next", cx));
    settle(cx);
    assert_eq!(
        label_of("picture-modal-sentence", cx).as_deref(),
        Some("2 of 3")
    );
}

#[gpui_kit::test]
fn an_attached_picture_in_the_composer_opens_large_too(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let lead = start(&user, "atlas", "hello");
    let (_shell, _reply, cx) = open_session(&user, &lead, cx);

    paste(clipboard_png(), cx);
    settle(cx);
    assert!(has("attachment-0", cx), "its thumbnail");
    cx.update(|window, cx| window.click("attachment-0", cx));
    settle(cx);

    assert!(has("picture-modal", cx), "the attached picture opens large");
    let picture = cx.update(|window, _| window.find("picture-large-0").bounds());
    assert_eq!(
        (
            f32::from(picture.size.width),
            f32::from(picture.size.height)
        ),
        points(8., 8., cx),
        "at its own size"
    );
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!has("picture-modal", cx), "Escape closes it");
}

/// Pastes the clipboard in the reply's field, focused as a user would.
fn paste(item: ClipboardItem, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.write_to_clipboard(item);
    cx.simulate_keystrokes("ctrl-v");
    frames(cx);
}
