//! The scenarios of #501, pictures in the project page's composer: Attach, paste and drop work
//! there as they do in a session's composer, and the pictures go with the first message of the
//! session it starts. On GPUI's test platform in front of a real daemon whose agents are the fake
//! ACP agent, with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::composer::Composer;
use flottille_window::link::Connect;
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    ClipboardItem, Entity, Image, ImageFormat, TestAppContext, VisualTestContext, px, size,
};
use serde_json::json;

fn png_of(width: u32, height: u32) -> Vec<u8> {
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

/// A user whose fake agent takes images, with the project `atlas` over a throwaway repository.
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

fn window<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
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

/// Lets every motion end, then draws: the window at rest.
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

/// `atlas`'s page, open in a window, with its composer.
fn open_atlas<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, Entity<Composer>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), cx);
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
    let composer = shell
        .read_with(cx, |s, _| s.composer_of("atlas").cloned())
        .expect("atlas's page has a composer");
    (shell, composer, cx)
}

fn paste(item: ClipboardItem, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.write_to_clipboard(item);
    cx.simulate_keystrokes("ctrl-v");
    frames(cx);
}

fn attached(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> Vec<Vec<u8>> {
    composer.read_with(cx, |c, _| {
        c.attachments().iter().map(|i| i.bytes.clone()).collect()
    })
}

fn has(id: &'static str, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| window.try_find(id).is_some())
}

/// The titles and second lines of the toasts the window shows, newest first.
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

/// Types `text` in the composer's field, given the focus by a click as a user would.
fn type_task(text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input(text);
}

/// The image blocks of the first prompt the fake agent received, once it has.
fn first_prompt_images(user: &User) -> Vec<serde_json::Value> {
    eventually("the agent got its first prompt", || {
        !user.received_requests("session/prompt").is_empty()
    });
    user.received_requests("session/prompt")[0]["params"]["prompt"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "image")
        .cloned()
        .collect()
}

#[gpui_kit::test]
fn an_image_attached_on_the_project_page_reaches_the_new_session_s_first_prompt(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, composer, cx) = open_atlas(&user, cx);
    let file = user.path("shot.png");
    std::fs::write(&file, png_of(8, 8)).unwrap();

    type_task("what is on this picture?", cx);
    composer.update_in(cx, |composer, _, cx| composer.attach_file(&file, cx));
    frames(cx);
    assert_eq!(attached(&composer, cx), vec![png_of(8, 8)]);
    assert!(has("attach", cx), "the Attach button is in the composer");
    cx.update(|window, cx| window.click("send", cx));

    let images = first_prompt_images(&user);
    assert_eq!(images.len(), 1, "the first prompt carries the picture");
    assert_eq!(images[0]["mimeType"], "image/png");
    assert_eq!(images[0]["data"], base64_of(&png_of(8, 8)));
    until(&shell, cx, "the composer let the picture go", |cx| {
        attached(&composer, cx).is_empty() && !composer.read_with(cx, |c, _| c.is_sending())
    });
    settle(cx);
    assert!(
        !has("attachment-0", cx),
        "the thumbnail folded away with the start"
    );
}

#[gpui_kit::test]
fn an_image_pasted_on_the_project_page_shows_its_thumbnail(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, composer, cx) = open_atlas(&user, cx);

    paste(
        ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png_of(8, 8))),
        cx,
    );
    settle(cx);

    assert_eq!(attached(&composer, cx), vec![png_of(8, 8)]);
    assert!(has("attachment-0", cx), "its thumbnail is drawn");
    assert!(
        has("attachment-remove-0", cx),
        "with its remove button on it"
    );
    assert_eq!(
        composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string()),
        "",
        "the field took no text"
    );
}

#[gpui_kit::test]
fn a_file_that_is_no_image_is_refused_on_the_project_page(cx: &mut TestAppContext) {
    let user = user();
    let (shell, composer, cx) = open_atlas(&user, cx);
    let text = user.path("notes.txt");
    std::fs::write(&text, "not a picture").unwrap();

    type_task("keep this text", cx);
    composer.update_in(cx, |composer, _, cx| composer.attach_file(&text, cx));
    frames(cx);

    assert!(attached(&composer, cx).is_empty(), "nothing is attached");
    assert_eq!(
        toasts(&shell, cx),
        [(
            "The picture was not attached".to_owned(),
            "Flottille takes png, jpeg, gif and webp images.".to_owned()
        )]
    );
    assert_eq!(
        composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string()),
        "keep this text"
    );
    settle(cx);
}

#[gpui_kit::test]
fn an_image_removed_before_sending_is_not_sent(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, composer, cx) = open_atlas(&user, cx);

    type_task("compare these", cx);
    composer.update_in(cx, |composer, _, cx| {
        composer.attach(Image::from_bytes(ImageFormat::Png, png_of(8, 8)), cx);
        composer.attach(Image::from_bytes(ImageFormat::Png, png_of(9, 9)), cx);
    });
    settle(cx);
    assert_eq!(attached(&composer, cx).len(), 2);

    // The first one's remove button, as a user would press it.
    cx.update(|window, cx| window.click("attachment-remove-0", cx));
    settle(cx);
    assert_eq!(attached(&composer, cx), vec![png_of(9, 9)]);
    cx.update(|window, cx| window.click("send", cx));

    let images = first_prompt_images(&user);
    assert_eq!(images.len(), 1, "only the picture left is sent");
    assert_eq!(images[0]["data"], base64_of(&png_of(9, 9)));
    settle(cx);
}
