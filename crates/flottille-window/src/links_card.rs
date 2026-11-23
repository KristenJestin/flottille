//! The card of the links an agent handed the user in a row (`docs/tools.md`, "Showing a link"):
//! one line each (title, host or path, Open), and a thumbnail on the line of a picture of the
//! agent's place. The window fetches nothing from the network, so an address never has one; a
//! file is read on the background executor, after the checks Open makes, and a picture that
//! cannot be read keeps its plain line.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_design_system::Type as _;
use gpui_design_system::color::mix;
use gpui_design_system::{
    Button, Card, Icon, Item, Listing, Skeleton, awaiting, enter, icon, palette, presence,
    pressable, space,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage as _,
    TestSupportExt as _, div, px,
};

use crate::conversation::ShownLink;

/// Where a card finds its row.
pub type Of<T> = fn(&mut T) -> &mut LinksCard;

/// Opens a target, after the checks Open makes (`open::shown`).
pub type Open = Rc<dyn Fn(&str, &mut App)>;

/// The thumbnail's frame: a bounded size, whatever the picture's.
const THUMB: (f32, f32) = (64., 44.);
/// ... made of twice the pixels, for a screen that has them.
const THUMB_PIXELS: (u32, u32) = (128, 88);

/// What is known of the picture on a line.
enum Picture {
    Reading,
    Ready(Arc<gpui_kit::Image>),
    /// Removed, too large, not a picture, or no longer a document: the line stays plain.
    Plain,
}

#[derive(Default)]
pub struct LinksCard {
    lines: Listing<ShownLink>,
    pictures: HashMap<String, Picture>,
}

/// The type of the picture a file's extension names, a thumbnail being drawn for these only.
fn picture_type(target: &str) -> Option<&'static str> {
    match Path::new(target)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

/// Whether the target is a picture, by its extension.
pub fn is_picture(target: &str) -> bool {
    picture_type(target).is_some()
}

/// Whether the target is a file of the place: an address gets no thumbnail.
fn is_file(target: &str) -> bool {
    !(target.starts_with("http://") || target.starts_with("https://"))
}

/// The most of a file that is read, as for an attachment.
const MAX_READ: u64 = 20 * 1024 * 1024;

/// The thumbnail of the picture a file holds: the file is opened once, without following a link,
/// checked as Open checks it on the file opened, and read up to [`MAX_READ`]; its size is read
/// from its header before it is decoded, and only a small copy of it is kept (see
/// `flottille_core::images::thumbnail`).
fn read(path: PathBuf) -> Option<Arc<gpui_kit::Image>> {
    use std::io::Read as _;
    let mime = picture_type(path.to_str()?)?;
    let file = flottille_core::shown::open_document(&path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_READ + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_READ {
        return None;
    }
    let small = flottille_core::images::thumbnail(mime, &bytes, THUMB_PIXELS).ok()?;
    Some(Arc::new(gpui_kit::Image::from_bytes(
        gpui_kit::ImageFormat::Png,
        small,
    )))
}

impl LinksCard {
    /// The card shows these links: the pictures among them are read, once each.
    pub fn show<T: 'static>(&mut self, links: &[ShownLink], cx: &mut Context<T>, of: Of<T>) {
        // A file of another machine is that machine's: a path of the same name here is none of
        // its picture, so that machine is asked for it (`crate::shown_files`).
        let elsewhere = crate::machines::shown_machine(cx).is_some();
        let connect = crate::shown_files::connect(cx);
        for link in links {
            let Some(mime) = picture_type(&link.target) else {
                continue;
            };
            if !is_file(&link.target) || self.pictures.contains_key(&link.target) {
                continue;
            }
            let reading = match (elsewhere, &connect) {
                (false, _) => {
                    let path = PathBuf::from(&link.target);
                    cx.background_spawn(async move { read(path) })
                }
                (true, Some(connect)) => {
                    let (connect, link) = (connect.clone(), link.clone());
                    cx.background_spawn(async move {
                        crate::shown_files::picture(&connect, &link, mime, THUMB_PIXELS)
                    })
                }
                (true, None) => continue,
            };
            self.pictures.insert(link.target.clone(), Picture::Reading);
            let target = link.target.clone();
            cx.spawn(async move |this, cx| {
                let picture = reading.await;
                this.update(cx, |this, cx| {
                    of(this)
                        .pictures
                        .insert(target, picture.map_or(Picture::Plain, Picture::Ready));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The pictures read so far.
    pub fn pictures_read(&self) -> usize {
        self.pictures
            .values()
            .filter(|picture| matches!(picture, Picture::Ready(_)))
            .count()
    }

    /// The card of row `at`: a line is added to it as the agent shows another link, and the
    /// count above the lines unfolds with the second.
    pub fn draw<T: 'static>(
        &mut self,
        at: usize,
        links: &[ShownLink],
        open: &Open,
        cx: &mut Context<T>,
    ) -> AnyElement {
        let p = palette(cx);
        let card = |part: &str| ElementId::Name(format!("row-{at}-{part}").into());
        self.lines.sync(
            links
                .iter()
                .enumerate()
                .map(|(n, link)| (n.to_string(), link.clone())),
            cx.background_executor().now(),
            cx.reduce_motion(),
        );
        let machine = crate::machines::shown_machine(cx);
        let lines: Vec<AnyElement> = self
            .lines
            .rows()
            .map(|row| {
                let n: usize = row.key.parse().unwrap_or_default();
                let link = row.value;
                // A file of the machine shown is said where it is, and not opened here, but for a
                // picture or a video, which that machine sends.
                let elsewhere = machine.as_ref().filter(|_| is_file(&link.target));
                let video = crate::shown_files::is_video(&link.target);
                let media = picture_type(&link.target).is_some() || video;
                let sent = elsewhere.filter(|_| media);
                let kind = if video { "Video" } else { "Picture" };
                let said = match (elsewhere, sent) {
                    (Some(machine), Some(_)) => format!(
                        "{kind} of {machine} shared by the agent: {}, {}",
                        link.name(),
                        link.whence
                    ),
                    (Some(machine), None) => format!(
                        "File of {machine} shared by the agent: {}, {}",
                        link.name(),
                        link.target
                    ),
                    _ => format!("Link shared by the agent: {}, {}", link.name(), link.whence),
                };
                let reading = matches!(self.pictures.get(&link.target), Some(Picture::Reading));
                let picture = match self.pictures.get(&link.target) {
                    Some(Picture::Ready(image)) => Some(image.clone()),
                    _ => None,
                };
                let words = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.))
                            .medium()
                            .truncate()
                            .child(SharedString::from(link.name().to_owned())),
                    )
                    .when(link.title.is_some() || elsewhere.is_some(), |d| {
                        let whence = match (elsewhere, sent) {
                            (Some(machine), Some(_)) => format!("{machine} · {}", link.whence),
                            (Some(machine), None) => format!("{machine}: {}", link.target),
                            _ => link.whence.clone(),
                        };
                        d.child(
                            div()
                                .mono(12.)
                                .text_color(p.faint)
                                .truncate()
                                .child(SharedString::from(whence)),
                        )
                    });
                let target = link.target.clone();
                // Open of a picture or a video another machine sends opens a copy of it here.
                let opens: Open = match sent {
                    Some(_) => {
                        let link = link.clone();
                        Rc::new(move |_: &str, cx: &mut App| crate::shown_files::open(&link, cx))
                    }
                    None => open.clone(),
                };
                let thumb = {
                    let (target, opens) = (target.clone(), opens.clone());
                    // The click goes where Open goes, through the same checks.
                    let frame = pressable(
                        div()
                            .id(card(&format!("link-{n}-thumb")))
                            .flex_none()
                            .w(px(THUMB.0))
                            .h(px(THUMB.1))
                            .rounded(px(6.))
                            .border_1()
                            .overflow_hidden()
                            .cursor_pointer()
                            .aria_label(SharedString::from(format!("Open {}", link.name())))
                            .when_some(picture.clone(), |d, image| {
                                d.child(
                                    gpui_kit::img(image)
                                        .size_full()
                                        .object_fit(gpui_kit::ObjectFit::Cover),
                                )
                            })
                            .on_click(move |_, _, cx| opens(&target, cx)),
                        move |d, hover, press| {
                            d.border_color(mix(p.line2, p.acc, hover))
                                .opacity(1.0 - 0.2 * press)
                        },
                    );
                    let remote = sent.is_some() && !video;
                    // Nothing came from the other machine: the placeholder gives way to nothing.
                    let shown = div()
                        .flex_none()
                        .pr(space::MD)
                        .when(!remote || picture.is_some(), |d| d.child(frame));
                    match remote {
                        // Another machine sends it: a placeholder stands in its place while it
                        // comes, and fades into it.
                        true => awaiting(
                            card(&format!("link-{n}-thumb-awaiting")),
                            reading,
                            div().flex_none().pr(space::MD).child(
                                Skeleton::new(card(&format!("link-{n}-thumb-skeleton")))
                                    .w(px(THUMB.0))
                                    .h(px(THUMB.1)),
                            ),
                            shown,
                        )
                        .into_any_element(),
                        // It unfolds and fades in once the picture is read; a line that has none
                        // takes no room.
                        false => presence(
                            card(&format!("link-{n}-thumb-presence")),
                            picture.is_some(),
                            shown,
                        )
                        .into_any_element(),
                    }
                };
                let line = div()
                    .id(card(&format!("link-{n}")))
                    .aria_label(SharedString::from(said))
                    .flex()
                    .items_center()
                    .px(space::LG)
                    .py(space::MD)
                    .child(
                        icon(Icon::OpenInEditor)
                            .size(px(14.))
                            .flex_none()
                            .mr(space::MD)
                            .text_color(p.faint),
                    )
                    .child(thumb)
                    .child(words)
                    .when(elsewhere.is_none() || sent.is_some(), |d| {
                        let words = if video && sent.is_some() {
                            "Play"
                        } else {
                            "Open"
                        };
                        d.child(
                            div().flex_none().ml(space::MD).child(
                                Button::secondary(card(&format!("link-{n}-open")), words)
                                    .on_click(move |_, _, cx| opens(&target, cx)),
                            ),
                        )
                    })
                    .test_support();
                Item::new(card(&format!("link-{n}-item")), row.placement, line).into_any_element()
            })
            .collect();
        let count = links.len();
        let head = div()
            .id(card("links-count"))
            .px(space::LG)
            .pt(space::MD)
            .pb(space::XS)
            .mono(12.)
            .text_color(p.faint)
            .aria_label(SharedString::from(format!("{count} links")))
            .child(SharedString::from(format!("{count} links")))
            .test_support();
        let body = div()
            .flex()
            .flex_col()
            .child(presence(card("links-count-presence"), count > 1, head))
            .children(lines);
        enter(
            card("links-enter"),
            0,
            div()
                .id(card("links"))
                .child(Card::new().child(body))
                .test_support(),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own in the temporary folder, removed with it.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("flottille-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Folder(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn only_a_file_with_a_picture_extension_gets_a_thumbnail() {
        assert_eq!(picture_type("/work/app/shot.PNG"), Some("image/png"));
        assert_eq!(picture_type("/work/app/shot.jpeg"), Some("image/jpeg"));
        assert_eq!(picture_type("/work/app/report.html"), None);
        assert_eq!(picture_type("/work/app/shot.png.sh"), None);
        assert!(is_file("/work/app/shot.png"));
        assert!(!is_file("https://example.org/shot.png"));
    }

    #[test]
    fn a_file_that_is_no_picture_is_not_read_as_one() {
        let folder = Folder::new("links-fake");
        let fake = folder.0.join("fake.png");
        std::fs::write(&fake, "<p>not a picture</p>").unwrap();
        assert!(read(fake).is_none());
        assert!(read(folder.0.join("missing.png")).is_none());
    }

    #[test]
    fn a_file_swapped_for_a_link_to_a_device_is_not_read() {
        let folder = Folder::new("links-swap");
        let shot = folder.0.join("shot.png");
        std::os::unix::fs::symlink("/dev/zero", &shot).unwrap();
        // Returns at once: nothing follows the link, so nothing reads without end.
        assert!(read(shot).is_none());
    }
}
