//! The pictures a composer carries, for both composers: the one of a session, which writes to
//! its root, and the one of a project's page, which starts a session. Attach, paste and drop, the
//! limits, the thumbnails with their remove button, the refusals as toasts: one behaviour, held
//! here, that each composer owns a value of and draws in its own place.
//!
//! The owner is a view, and a picture scaled in the background comes back to it: every method
//! that may wait for one takes `of`, the way to reach this value from its owner.

use std::sync::Arc;

use flottille_core::images::{MAX_IMAGE_BYTES, MAX_IMAGES};
use flottille_ui::color::mix;
use flottille_ui::motion::PANEL;
use flottille_ui::{
    Button, Icon, Listing, Skeleton, Toast, enter, fade, hoverable, icon, palette, presence, space,
    toast,
};
use gpui_kit::base::input::Paste;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage as _,
    TestSupportExt as _, Window, div, px,
};

use crate::actions::Picture;

/// What a toast says of a picture the composer does not attach: its title, and why.
pub const NOT_ATTACHED: &str = "The picture was not attached";
pub const NO_IMAGES: &str = "This agent takes no images.";
pub const IMAGE_KINDS: &str = "Flottille takes png, jpeg, gif and webp images.";
const IMAGE_TOO_BIG: &str = "An image can be 5 MiB at most.";
const NOT_READ: &str = "That file could not be read.";
/// What a toast says when a message is sent while a picture is still being scaled down.
pub const STILL_SCALING: &str = "A picture is still being scaled down: send once it is there.";
/// The biggest file read to be attached: a screenshot of a big screen weighs more than a message
/// may, and is scaled down before its size is judged.
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// The picture a file holds, by its extension: or why it is none (`IMAGE_KINDS`, `IMAGE_TOO_BIG`,
/// `NOT_READ`). Read as it is; the composer scales it down, a card of links only draws it.
pub fn read_picture(path: &std::path::Path) -> Result<gpui_kit::Image, &'static str> {
    use gpui_kit::ImageFormat::{Gif, Jpeg, Png, Webp};
    let format = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Png,
        Some("jpg" | "jpeg") => Jpeg,
        Some("gif") => Gif,
        Some("webp") => Webp,
        _ => return Err(IMAGE_KINDS),
    };
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        return Err(IMAGE_TOO_BIG);
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(gpui_kit::Image::from_bytes(format, bytes)),
        Err(_) => Err(NOT_READ),
    }
}

/// The key of a thumbnail in the composer: the picture itself, which stays alive while it leaves.
fn thumb_key(image: &Arc<gpui_kit::Image>) -> String {
    format!("{:x}", Arc::as_ptr(image) as usize)
}

/// How a composer reaches its [`Attachments`].
pub type Of<T> = fn(&mut T) -> &mut Attachments;

#[derive(Default)]
pub struct Attachments {
    /// Pictures pasted or dropped, sent with the next message.
    images: Vec<Arc<gpui_kit::Image>>,
    /// The ones the message being sent holds: they leave when it is taken, whatever was removed
    /// or added meanwhile.
    sent: Vec<Arc<gpui_kit::Image>>,
    /// Whether the agent takes images; `None` until it is known.
    takes: Option<bool>,
    /// The thumbnails, which unfold as a picture is attached and fold away as it leaves.
    thumbs: Listing<Arc<gpui_kit::Image>>,
    /// The big pictures being scaled down off the window's thread, each held by a skeleton in
    /// the strip until its thumbnail is ready.
    scaling: Vec<u64>,
    places: Listing<()>,
    scaled: u64,
}

impl Attachments {
    /// The pictures that go with the next message.
    pub fn pictures(&self) -> &[Arc<gpui_kit::Image>] {
        &self.images
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// A big picture is still being scaled down.
    pub fn is_scaling(&self) -> bool {
        !self.scaling.is_empty()
    }

    pub fn set_takes(&mut self, takes: Option<bool>) {
        self.takes = takes;
    }

    /// Says in a toast why a picture was not attached: the composer never changes for it.
    fn not_attached<T: 'static>(why: impl Into<SharedString>, cx: &mut Context<T>) {
        toast(Toast::failure(NOT_ATTACHED).detail(why), cx);
    }

    /// Attaches a picture, or says in a toast why not; the text in the field stays. A big one is
    /// scaled down first, so that its weight is judged as it will be stored: decoding and
    /// resizing a 4K screenshot takes long enough to freeze the window, so it happens on the
    /// background executor and the picture is attached once it is done.
    pub fn attach<T: 'static>(&mut self, image: gpui_kit::Image, cx: &mut Context<T>, of: Of<T>) {
        use gpui_kit::ImageFormat::{Gif, Jpeg, Png, Webp};
        if self.takes == Some(false) {
            return Self::not_attached(NO_IMAGES, cx);
        }
        if !matches!(image.format, Png | Jpeg | Gif | Webp) {
            return Self::not_attached(IMAGE_KINDS, cx);
        }
        if self.images.len() + self.scaling.len() >= MAX_IMAGES {
            return Self::not_attached(
                format!("A message carries {MAX_IMAGES} images at most."),
                cx,
            );
        }
        let mime = image.format.mime_type();
        if !flottille_core::images::needs_scaling(mime, &image.bytes) {
            return self.keep(image, cx);
        }
        let (id, format) = (self.scaled, image.format);
        self.scaled += 1;
        self.scaling.push(id);
        let scaling =
            cx.background_spawn(
                async move { flottille_core::images::scaled_down(mime, image.bytes) },
            );
        cx.spawn(async move |this, cx| {
            let bytes = scaling.await;
            this.update(cx, |this, cx| {
                let attachments = of(this);
                attachments.scaling.retain(|scaling| *scaling != id);
                attachments.keep(gpui_kit::Image::from_bytes(format, bytes), cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keeps a picture that is ready to be sent, if it weighs what a message may carry.
    fn keep<T: 'static>(&mut self, image: gpui_kit::Image, cx: &mut Context<T>) {
        if image.bytes.len() > MAX_IMAGE_BYTES {
            return Self::not_attached(IMAGE_TOO_BIG, cx);
        }
        self.images.push(Arc::new(image));
        cx.notify();
    }

    /// Attaches the picture a file holds (a file dropped on the composer, or picked).
    pub fn attach_file<T: 'static>(
        &mut self,
        path: &std::path::Path,
        cx: &mut Context<T>,
        of: Of<T>,
    ) {
        match read_picture(path) {
            Ok(image) => self.attach(image, cx, of),
            Err(why) => Self::not_attached(why, cx),
        }
    }

    pub fn detach<T: 'static>(&mut self, at: usize, cx: &mut Context<T>) {
        if at < self.images.len() {
            self.images.remove(at);
            cx.notify();
        }
    }

    /// Paste: a clipboard that holds a picture attaches it, whatever text comes with it (a copy in
    /// a browser gives the picture's address or its HTML as well); else the field takes it.
    pub fn pasted<T: 'static>(&mut self, cx: &mut Context<T>, of: Of<T>) -> bool {
        use gpui_kit::ClipboardEntry;
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        let has_image = clipboard
            .entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(_)));
        if clipboard.text().is_some() && !has_image {
            return false;
        }
        let mut taken = false;
        for entry in clipboard.into_entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    self.attach(image, cx, of);
                    taken = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    for path in paths.paths() {
                        self.attach_file(path, cx, of);
                        taken = true;
                    }
                }
                ClipboardEntry::String(_) => {}
            }
        }
        taken
    }

    /// The Attach button's click: pictures picked in the system's file chooser.
    fn pick_files<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>, of: Of<T>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            this.update(cx, |this, cx| {
                for path in &paths {
                    of(this).attach_file(path, cx, of);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The pictures of the message being sent, which stay attached until it is taken.
    pub fn sending(&mut self) -> Vec<Picture> {
        self.sent = self.images.clone();
        self.images
            .iter()
            .map(|image| Picture {
                mime: image.format.mime_type().to_owned(),
                bytes: image.bytes.clone(),
            })
            .collect()
    }

    /// The pictures of a message that leaves the composer at once: they go with it, and fold
    /// away from the strip.
    pub fn take<T: 'static>(&mut self, cx: &mut Context<T>) -> Vec<Arc<gpui_kit::Image>> {
        cx.notify();
        std::mem::take(&mut self.images)
    }

    /// The message was taken: the pictures it held leave, and those changed meanwhile stay.
    pub fn sent(&mut self) {
        let sent = std::mem::take(&mut self.sent);
        self.images
            .retain(|image| !sent.iter().any(|sent| Arc::ptr_eq(sent, image)));
    }

    /// A session started: nothing stays attached for the next one.
    pub fn clear(&mut self) {
        self.sent.clear();
        self.images.clear();
    }

    /// The pictures of a message taken back join the ones already attached.
    pub fn restore<T: 'static>(&mut self, pictures: Vec<Picture>, cx: &mut Context<T>, of: Of<T>) {
        for picture in pictures {
            if let Some(format) = gpui_kit::ImageFormat::from_mime_type(&picture.mime) {
                self.attach(gpui_kit::Image::from_bytes(format, picture.bytes), cx, of);
            }
        }
    }

    /// The Attach button, which opens the system's file chooser.
    pub fn button<T: 'static>(cx: &mut Context<T>, of: Of<T>) -> Button {
        Button::quiet("attach", "Attach")
            .tooltip("Attach pictures")
            .on_click(cx.listener(move |this, _, window, cx| {
                of(this).pick_files(window, cx, of);
            }))
    }

    /// Makes `root` take pictures: a clipboard with one attaches it, and files dropped on it
    /// attach theirs.
    pub fn taken_by<T: 'static, E: InteractiveElement>(
        root: E,
        cx: &mut Context<T>,
        of: Of<T>,
    ) -> E {
        root
            // A picture on the clipboard is an attachment; text is left to the field.
            .capture_action(cx.listener(move |this, _: &Paste, _, cx| {
                if of(this).pasted(cx, of) {
                    cx.stop_propagation();
                }
            }))
            .on_drop(
                cx.listener(move |this, paths: &gpui_kit::ExternalPaths, _, cx| {
                    for path in paths.paths() {
                        of(this).attach_file(path, cx, of);
                    }
                }),
            )
    }

    /// The strip of thumbnails above the field. A picture unfolds into it as it is attached and
    /// folds away as it leaves: the gone ones stay in the listing until they have. `detach` is
    /// what the owner does when the remove button of the picture `at` is pressed.
    pub fn strip<T: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<T>,
        detach: fn(&mut T, usize, &mut Context<T>),
    ) -> AnyElement {
        let p = palette(cx);
        let now = cx.background_executor().now();
        self.thumbs.sync(
            self.images.iter().map(|i| (thumb_key(i), i.clone())),
            now,
            cx.reduce_motion(),
        );
        let listed: Vec<(String, Arc<gpui_kit::Image>, bool)> = self
            .thumbs
            .rows()
            .map(|row| {
                (
                    row.key.to_owned(),
                    row.value.clone(),
                    row.placement.is_gone(),
                )
            })
            .collect();
        self.places.sync(
            self.scaling.iter().map(|id| (format!("scaling-{id}"), ())),
            now,
            cx.reduce_motion(),
        );
        let places: Vec<(String, bool)> = self
            .places
            .rows()
            .map(|row| (row.key.to_owned(), row.placement.is_gone()))
            .collect();
        let mut live = 0;
        let thumbs: Vec<AnyElement> = listed
            .into_iter()
            .map(|(key, image, gone)| {
                let at = live;
                let id = match gone {
                    true => ElementId::Name(format!("attachment-leaving-{key}").into()),
                    false => ElementId::Name(format!("attachment-{at}").into()),
                };
                live += usize::from(!gone);
                let shown = fade(
                    ElementId::Name(format!("attachment-shown-{key}").into()),
                    !gone,
                    PANEL,
                    window,
                    cx,
                );
                let thumb = div()
                    .id(id)
                    .relative()
                    .flex_none()
                    .size(px(56.))
                    .child(
                        gpui_kit::img(image)
                            .size_full()
                            .rounded(px(8.))
                            .object_fit(gpui_kit::ObjectFit::Cover),
                    )
                    .when(!gone, |d| {
                        d.child(hoverable(
                            div()
                                .id(ElementId::Name(format!("attachment-remove-{at}").into()))
                                .absolute()
                                .top(px(-5.))
                                .right(px(-5.))
                                .size(px(18.))
                                .rounded_full()
                                .bg(p.card2)
                                .border_1()
                                .border_color(p.line2)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .aria_label("Remove the picture")
                                .on_click(cx.listener(move |this, _, _, cx| detach(this, at, cx))),
                            // An icon does not inherit the ink: it is drawn here, in the ink this frame.
                            move |d, t| {
                                d.bg(mix(p.card2, p.hover, t)).child(
                                    icon(Icon::Close)
                                        .size(px(12.))
                                        .text_color(mix(p.mute, p.text, t)),
                                )
                            },
                        ))
                    })
                    .test_support();
                // The slot is as wide as the picture and the gap after it, narrowing as it leaves.
                let slot = div()
                    .flex_none()
                    .w(px(68. * shown))
                    .pr(space::MD)
                    .pb(space::SM)
                    .opacity(shown)
                    .when(shown < 1.0, |d| d.overflow_hidden())
                    .child(thumb);
                enter(
                    ElementId::Name(format!("attachment-enter-{key}").into()),
                    0,
                    slot,
                )
                .into_any_element()
            })
            .collect();
        // A skeleton holds the place of a picture being scaled, and folds away as its thumbnail
        // unfolds.
        let mut held = 0;
        let places: Vec<AnyElement> = places
            .into_iter()
            .map(|(key, gone)| {
                let id = match gone {
                    true => ElementId::Name(format!("attachment-scaling-leaving-{key}").into()),
                    false => ElementId::Name(format!("attachment-scaling-{held}").into()),
                };
                held += usize::from(!gone);
                let shown = fade(
                    ElementId::Name(format!("attachment-scaling-shown-{key}").into()),
                    !gone,
                    PANEL,
                    window,
                    cx,
                );
                let slot = div()
                    .flex_none()
                    .w(px(68. * shown))
                    .pr(space::MD)
                    .pb(space::SM)
                    .opacity(shown)
                    .when(shown < 1.0, |d| d.overflow_hidden())
                    .child(Skeleton::new(id).w(px(56.)).h(px(56.)));
                enter(
                    ElementId::Name(format!("attachment-scaling-enter-{key}").into()),
                    0,
                    slot,
                )
                .into_any_element()
            })
            .collect();
        presence(
            "attachments",
            !self.images.is_empty() || !self.places.is_empty(),
            div()
                .flex()
                .flex_wrap()
                .px(space::SM)
                .pt(space::SM)
                .pb(space::SM)
                .children(thumbs)
                .children(places),
        )
        .into_any_element()
    }
}
