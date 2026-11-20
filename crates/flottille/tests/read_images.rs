//! The scenarios of `fs_read` on a picture (`docs/tools.md`, "Files"): an image file is answered
//! as an image, the MCP `image` content a pasted picture also is, with one line naming it; a
//! large one is scaled down first; any other file is read as it always was. The agents are fake
//! ACP agents, in a throwaway repository and temporary XDG folders.

use crate::common;

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

/// A greyscale PNG of `width` by `height` whose pixels are all black, deflated as it is written:
/// a few hundred KB for hundreds of MB once decoded.
fn black(width: u32, height: u32) -> Vec<u8> {
    use std::io::Write as _;
    let chunk = |kind: &[u8; 4], body: &[u8]| {
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(body);
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out.extend_from_slice(&crc.sum().to_be_bytes());
        out
    };
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut deflated = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let row = vec![0u8; width as usize + 1];
    for _ in 0..height {
        deflated.write_all(&row).unwrap();
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(chunk(b"IHDR", &header));
    png.extend(chunk(b"IDAT", &deflated.finish().unwrap()));
    png.extend(chunk(b"IEND", &[]));
    png
}

/// What `fs_read` answers for a file that is left out: no image, a line that says so.
fn left_out(mcp: &Mcp, path: &str) -> String {
    let (text, images) = read(mcp, path);
    assert!(images.is_empty(), "{path}: no image goes: {text}");
    assert!(text.contains("left out"), "{path}: {text}");
    text
}

#[test]
fn an_image_too_large_to_decode_is_left_out() {
    let user = user();
    let folder = project(&user);
    // 2^28 pixels on one row: about 260 KB on disk, 256 MiB decoded, gigabytes to scale.
    let bomb = black(1 << 28, 1);
    assert!(bomb.len() < 2 * 1024 * 1024, "the file is small");
    std::fs::write(folder.join("bomb.png"), &bomb).unwrap();
    // The same, with no pixels behind its header.
    let mut header_only = picture(40, 30, image::ImageFormat::Png);
    header_only[16..20].copy_from_slice(&(1u32 << 28).to_be_bytes());
    std::fs::write(folder.join("header.png"), &header_only).unwrap();
    let mcp = session(&user);

    left_out(&mcp, "bomb.png");
    left_out(&mcp, "header.png");
    // The daemon is alive: the next read is answered.
    std::fs::write(
        folder.join("piano.png"),
        picture(40, 30, image::ImageFormat::Png),
    )
    .unwrap();
    let (_, images) = read(&mcp, "piano.png");
    assert_eq!(images.len(), 1);
}

#[test]
fn a_very_wide_image_is_left_out() {
    let user = user();
    let folder = project(&user);
    // 64 million pixels on one row: within the pixel bound, about 64 KB on disk, and scaling it
    // would ask a gigabyte of buffer a time.
    let wide = black(64_000_000, 1);
    assert!(wide.len() < 2 * 1024 * 1024, "the file is small");
    std::fs::write(folder.join("wide.png"), &wide).unwrap();
    let mcp = session(&user);

    let text = left_out(&mcp, "wide.png");
    assert!(text.contains("px on a side"), "{text}");
    // The daemon is alive: the next read is answered.
    std::fs::write(
        folder.join("piano.png"),
        picture(40, 30, image::ImageFormat::Png),
    )
    .unwrap();
    let (_, images) = read(&mcp, "piano.png");
    assert_eq!(images.len(), 1);
}

#[test]
fn a_truncated_image_is_left_out() {
    let user = user();
    let folder = project(&user);
    // A valid signature and header, the data cut short: the provider cannot read it and keeps
    // it in the history.
    let png = picture(300, 200, image::ImageFormat::Png);
    std::fs::write(folder.join("cut.png"), &png[..png.len() / 2]).unwrap();
    // A signature and nothing else.
    std::fs::write(
        folder.join("bare.png"),
        b"\x89PNG\r\n\x1a\nbut nothing more",
    )
    .unwrap();
    let mcp = session(&user);

    for path in ["cut.png", "bare.png"] {
        left_out(&mcp, path);
    }
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
