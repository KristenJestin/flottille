//! The scenarios of `fs_read` on a picture (`docs/tools.md`, "Files"): an image file is answered
//! as an image, the MCP `image` content a pasted picture also is, with one line naming it; a
//! large one is scaled down first; any other file is read as it always was. The agents are fake
//! ACP agents, in a throwaway repository and temporary XDG folders.

mod common;

use std::io::Cursor;
use std::path::PathBuf;

use base64::Engine as _;
use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// A project `app` over a throwaway repository with one commit.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

/// The session of a root agent of `app`.
fn session(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "start"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    user
}

/// A picture of `width` by `height`, encoded as `format`.
fn picture(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
    })
    .write_to(&mut out, format)
    .unwrap();
    out.into_inner()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

fn size_of(data: &str) -> (u32, u32) {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap()
}

fn read(mcp: &Mcp, path: &str) -> (String, Vec<(String, String)>) {
    let (answer, images) = mcp.call_with_images("fs_read", json!({"path": path}));
    (done(answer), images)
}

#[test]
fn an_image_file_is_read_as_an_image() {
    let user = user();
    let folder = project(&user);
    let png = picture(40, 30, image::ImageFormat::Png);
    let jpeg = picture(64, 48, image::ImageFormat::Jpeg);
    std::fs::write(folder.join("piano.png"), &png).unwrap();
    std::fs::write(folder.join("piano.jpg"), &jpeg).unwrap();
    let mcp = session(&user);

    let (text, images) = read(&mcp, "piano.png");
    assert_eq!(images.len(), 1, "{text}");
    assert_eq!(images[0].0, "image/png");
    assert_eq!(
        images[0].1,
        base64::engine::general_purpose::STANDARD.encode(&png),
        "the file itself, as it is"
    );
    assert!(text.contains("piano.png"), "{text}");
    assert!(text.contains("40×30"), "{text}");
    assert!(!text.contains("scaled"), "{text}");
    assert!(
        !text.contains(&images[0].1[..16]),
        "the bytes are not in the text: {text}"
    );

    let (text, images) = read(&mcp, "piano.jpg");
    assert_eq!(images.len(), 1, "{text}");
    assert_eq!(images[0].0, "image/jpeg");
    assert!(
        text.contains("piano.jpg") && text.contains("64×48"),
        "{text}"
    );
}

#[test]
fn a_large_image_is_scaled_down_before_it_is_sent() {
    let user = user();
    let folder = project(&user);
    // A phone photo's shape: well over the long side an image is shown at.
    let photo = picture(3000, 2000, image::ImageFormat::Jpeg);
    std::fs::write(folder.join("photo.jpg"), &photo).unwrap();
    let mcp = session(&user);

    let (text, images) = read(&mcp, "photo.jpg");
    assert_eq!(images.len(), 1, "{text}");
    assert_eq!(images[0].0, "image/jpeg");
    assert_eq!(size_of(&images[0].1), (2000, 1333), "scaled to 2000 px");
    assert!(
        images[0].1.len() < 2 * 1024 * 1024 * 4 / 3,
        "under the bound"
    );
    assert!(text.contains("scaled"), "{text}");
    assert!(
        text.contains("3000×2000"),
        "the line says what it was: {text}"
    );
}

#[test]
fn a_binary_file_that_is_not_an_image_is_read_as_before() {
    let user = user();
    let folder = project(&user);
    std::fs::write(folder.join("data.bin"), b"\x00\x01\x02 words\nmore\n").unwrap();
    // Named as a picture, it is not one: its bytes say what a file is.
    std::fs::write(folder.join("notes.png"), "just words\n").unwrap();
    let mcp = session(&user);

    for (path, expect) in [("data.bin", "words"), ("notes.png", "just words")] {
        let (text, images) = read(&mcp, path);
        assert!(images.is_empty(), "{path}: no image goes");
        assert!(text.contains(expect), "{path}: {text}");
        assert!(
            text.contains("(bytes 0-"),
            "{path}: a page, as before: {text}"
        );
    }
}

#[test]
fn an_agent_whose_provider_reads_no_image_is_told_so() {
    let user = User::with_fake_providers();
    user.provider_script(
        "opencode",
        json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}),
    );
    let folder = project(&user);
    std::fs::write(
        folder.join("piano.png"),
        picture(40, 30, image::ImageFormat::Png),
    )
    .unwrap();
    user.call(
        "agent.run",
        json!({"project": "app", "message": "start", "provider": "opencode"}),
    )
    .unwrap();
    let mut opened = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while opened.is_none() {
        assert!(std::time::Instant::now() < deadline, "no session opened");
        opened = user
            .provider_received("opencode")
            .into_iter()
            .find(|entry| entry["received"]["method"] == "session/new");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let server: Value = opened.unwrap()["received"]["params"]["mcpServers"][0].clone();
    let authorization = server["headers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "Authorization")
        .unwrap()["value"]
        .as_str()
        .unwrap()
        .to_owned();
    let mcp = Mcp::initialize(server["url"].as_str().unwrap(), &authorization);
    let (text, images) = read(&mcp, "piano.png");
    assert!(images.is_empty(), "no image goes");
    assert!(
        text.contains("piano.png") && text.contains("reads no image"),
        "{text}"
    );
}
