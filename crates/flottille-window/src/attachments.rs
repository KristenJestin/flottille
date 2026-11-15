//! The pictures a composer carries, for both composers: the one of a session, which writes to
//! its root, and the one of a project's page, which starts a session. Attach, paste and drop, the
//! limits, the thumbnails with their remove button, the refusals as toasts: one behaviour, held
//! here, that each composer owns a value of and draws in its own place.
//!
//! The owner is a view, and a picture scaled in the background comes back to it: every method
//! that may wait for one takes `of`, the way to reach this value from its owner.
//!
//! Each picture is also a chip in the owner's field, where it was put (#773): `Image 1`, its name
//! and its size, numbered from 1 in the order of the chips, its thumbnail under the same number.
//! Removing the chip removes the picture, and the thumbnail's remove button removes the chip.
//!
//! A large pasted text is the same without a thumbnail (#848): a chip named `pasted-text.txt`
//! with its size, which the daemon writes as a file the message names. Ctrl+Shift+V pastes the
//! text inline instead.

use std::sync::Arc;

use flottille_core::images::{MAX_IMAGE_BYTES, MAX_IMAGES};
use gpui_design_system::color::mix;
use gpui_design_system::motion::PANEL;
use gpui_design_system::{
    Badge, Button, Icon, Listing, Piece, Skeleton, Toast, Token, TokenEvent, TokenField, TokenLook,
    enter, fade, hoverable, icon, palette, presence, space, toast,
};
use gpui_kit::Action as _;
use gpui_kit::base::input::Paste;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage as _,
    TestSupportExt as _, Window, div, px,
};

use crate::actions::{Pasted, Picture};
use crate::picture_viewer::PictureViewer;
use crate::tokens;

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

/// The key of a thumbnail in the composer: the picture itself, which stays alive while it leaves.
fn thumb_key(image: &Arc<gpui_kit::Image>) -> String {
    format!("{:x}", Arc::as_ptr(image) as usize)
}

/// The extension a picture of that format is named with when it came without a name (pasted).
fn extension(format: gpui_kit::ImageFormat) -> &'static str {
    use gpui_kit::ImageFormat::{Gif, Jpeg, Webp};
    match format {
        Jpeg => "jpg",
        Gif => "gif",
        Webp => "webp",
        _ => "png",
    }
}

/// The number of a thumbnail, its chip's in the text, as the kit's badge: a new number fades in
/// as the chips before it change. `id` names it; one that leaves keeps the number it had.
fn number(id: ElementId, key: &str, number: usize) -> AnyElement {
    let badge = div()
        .id(id)
        .aria_label(SharedString::from(format!("Image {number}")))
        .child(Badge::count(number))
        .test_support();
    div()
        .absolute()
        .left(px(3.))
        .bottom(px(3.))
        .child(enter(
            ElementId::Name(format!("attachment-number-enter-{key}-{number}").into()),
            0,
            badge,
        ))
        .into_any_element()
}

/// What a toast says when a pasted text is not attached as a file.
const TEXT_NOT_ATTACHED: &str = "The text was not attached";
/// What the toast of a large paste says, in its two lines: said once.
const LARGE_PASTE: &str = "Large paste attached as a file";
const LARGE_PASTE_HINT: &str = "Ctrl+Shift+V to paste it inline";

/// How a composer reaches its [`Attachments`].
pub type Of<T> = fn(&mut T) -> &mut Attachments;

/// A picture attached: its chip's id, its name, and whether its chip is in the field yet.
#[derive(Clone)]
struct Attached {
    id: u64,
    image: Arc<gpui_kit::Image>,
    name: String,
    /// Its chip was put in the field (or it came back without one, and goes after the text).
    placed: bool,
}

/// A large paste attached as a text file: its chip's id, its name, its text, and whether its chip
/// is in the field yet.
#[derive(Clone)]
struct PastedText {
    id: u64,
    name: String,
    text: Arc<str>,
    placed: bool,
}

/// The id of the next chip, unique across composers: a chip copied from one into another is not
/// taken there for an attachment of its own.
fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Default)]
pub struct Attachments {
    /// Pictures pasted or dropped, sent with the next message, in the order of their chips.
    images: Vec<Attached>,
    /// The ones whose chip the user removed: an undo or a paste of the chip brings them back.
    gone: Vec<Attached>,
    /// Large texts pasted, sent as files with the next message.
    texts: Vec<PastedText>,
    /// The texts whose chip the user removed.
    gone_texts: Vec<PastedText>,
    /// The toast of a large paste was shown: it says it once.
    told: bool,
    /// The next paste is Ctrl+Shift+V's: the text goes in the field as it is.
    inline: bool,
    /// The field the chips are in, set by the owner.
    field: Option<Entity<TokenField>>,
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
    /// The number each thumbnail last showed, by its key: one that folds away keeps it.
    numbers: std::collections::HashMap<String, usize>,
    /// A thumbnail opened large; made when the strip is first drawn.
    viewer: Option<Entity<PictureViewer>>,
}

impl Attachments {
    /// The pictures of the chips of `field`.
    pub fn new(field: Entity<TokenField>) -> Self {
        Self {
            field: Some(field),
            ..Self::default()
        }
    }

    /// The pictures that go with the next message, in the order of their chips.
    pub fn pictures(&self) -> Vec<Arc<gpui_kit::Image>> {
        self.images.iter().map(|a| a.image.clone()).collect()
    }

    /// The chip of the picture `at` (in the order of [`Attachments::pictures`]) is in the field.
    pub fn has_chip(&self, at: usize, cx: &gpui_kit::App) -> bool {
        let (Some(field), Some(attached)) = (&self.field, self.images.get(at)) else {
            return false;
        };
        let id = tokens::picture_id(attached.id);
        field
            .read(cx)
            .value(cx)
            .iter()
            .any(|piece| matches!(piece, Piece::Token(token) if token.id == id))
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty() && self.texts.is_empty()
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
        let name = format!("image.{}", extension(image.format));
        self.attach_named(image, name, cx, of);
    }

    /// [`Attachments::attach`], the picture named as its chip says it.
    fn attach_named<T: 'static>(
        &mut self,
        image: gpui_kit::Image,
        name: String,
        cx: &mut Context<T>,
        of: Of<T>,
    ) {
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
            return self.keep(image, name, cx);
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
                attachments.keep(gpui_kit::Image::from_bytes(format, bytes), name, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keeps a picture that is ready to be sent, if it weighs what a message may carry. Its chip
    /// goes in at the caret when the owner is next drawn ([`Attachments::place`]).
    fn keep<T: 'static>(&mut self, image: gpui_kit::Image, name: String, cx: &mut Context<T>) {
        if image.bytes.len() > MAX_IMAGE_BYTES {
            return Self::not_attached(IMAGE_TOO_BIG, cx);
        }
        self.push(Arc::new(image), name, self.field.is_none());
        cx.notify();
    }

    fn push(&mut self, image: Arc<gpui_kit::Image>, name: String, placed: bool) -> u64 {
        let id = next_id();
        self.images.push(Attached {
            id,
            image,
            name,
            placed,
        });
        id
    }

    /// Attaches a large pasted text as a file, or says in a toast why not; the field is left as it
    /// was. Its chip goes in at the caret when the owner is next drawn ([`Attachments::place`]).
    fn attach_text<T: 'static>(&mut self, text: String, cx: &mut Context<T>) {
        if text.len() > flottille_core::pasted::MAX_TEXT_BYTES {
            let detail = format!(
                "A pasted text can be {} MiB at most.",
                flottille_core::pasted::MAX_TEXT_BYTES / 1024 / 1024
            );
            return toast(Toast::failure(TEXT_NOT_ATTACHED).detail(detail), cx);
        }
        if self.texts.len() >= flottille_core::pasted::MAX_TEXTS {
            let detail = format!(
                "A message carries {} pasted texts at most.",
                flottille_core::pasted::MAX_TEXTS
            );
            return toast(Toast::failure(TEXT_NOT_ATTACHED).detail(detail), cx);
        }
        self.texts.push(PastedText {
            id: next_id(),
            name: flottille_core::pasted::NAME.to_owned(),
            text: text.into(),
            placed: self.field.is_none(),
        });
        if !std::mem::replace(&mut self.told, true) {
            toast(Toast::success(LARGE_PASTE).detail(LARGE_PASTE_HINT), cx);
        }
        cx.notify();
    }

    /// Attaches the picture a file holds (a file dropped on the composer, or picked).
    pub fn attach_file<T: 'static>(
        &mut self,
        path: &std::path::Path,
        cx: &mut Context<T>,
        of: Of<T>,
    ) {
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
            _ => return Self::not_attached(IMAGE_KINDS, cx),
        };
        if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            return Self::not_attached(IMAGE_TOO_BIG, cx);
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("image.{}", extension(format)));
        match std::fs::read(path) {
            Ok(bytes) => {
                self.attach_named(gpui_kit::Image::from_bytes(format, bytes), name, cx, of)
            }
            Err(_) => Self::not_attached(NOT_READ, cx),
        }
    }

    /// The remove button of the picture `at`: the picture leaves, and its chip folds away.
    pub fn detach<T: 'static>(&mut self, at: usize, window: &mut Window, cx: &mut Context<T>) {
        if at < self.images.len() {
            let gone = self.images.remove(at);
            if let Some(field) = &self.field {
                let id = tokens::picture_id(gone.id);
                field.update(cx, |field, cx| field.remove(&id, window, cx));
            }
            // An undo brings its chip back, and the chip its picture.
            self.gone.push(gone);
            cx.notify();
        }
    }

    /// What the user did to the chips: a picture's chip removed takes its picture away, one that
    /// came back (an undo, a paste) brings it back.
    pub fn chip_changed<T: 'static>(&mut self, event: &TokenEvent, cx: &mut Context<T>) {
        match event {
            TokenEvent::Removed(id) => {
                if let Some(id) = tokens::pasted_of(id) {
                    if let Some(at) = self.texts.iter().position(|t| t.id == id) {
                        self.gone_texts.push(self.texts.remove(at));
                        cx.notify();
                    }
                    return;
                }
                let Some(id) = tokens::picture_of(id) else {
                    return;
                };
                if let Some(at) = self.images.iter().position(|a| a.id == id) {
                    self.gone.push(self.images.remove(at));
                    cx.notify();
                    // The text's change may have come before its picture left (an undo, a redo,
                    // a cut): the chips after it are numbered again now.
                    if let Some(field) = self.field.clone() {
                        self.follow_chips(&field, cx);
                    }
                }
            }
            TokenEvent::Added(id) => {
                if let Some(id) = tokens::pasted_of(id) {
                    if let Some(at) = self.gone_texts.iter().position(|t| t.id == id) {
                        self.texts.push(self.gone_texts.remove(at));
                        cx.notify();
                    }
                    return;
                }
                let Some(id) = tokens::picture_of(id) else {
                    return;
                };
                if let Some(at) = self.gone.iter().position(|a| a.id == id) {
                    let back = self.gone.remove(at);
                    self.images.push(back);
                    cx.notify();
                    // The text's change may have come before its picture did: the numbers and
                    // the order are settled now.
                    if let Some(field) = self.field.clone() {
                        self.follow_chips(&field, cx);
                    }
                }
            }
        }
    }

    /// Puts in the chips of the pictures attached since the owner was drawn, at the caret. Called
    /// as the owner is drawn: it reads the field only when a picture waits, so that the owner is
    /// not drawn again for each key typed.
    pub fn place<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        if self.images.iter().all(|a| a.placed) && self.texts.iter().all(|t| t.placed) {
            return;
        }
        let Some(field) = self.field.clone() else {
            return;
        };
        for pasted in self.texts.iter_mut().filter(|t| !t.placed) {
            pasted.placed = true;
            let token = Token::new(
                tokens::pasted_id(pasted.id),
                tokens::pasted_look(&pasted.name, Some(pasted.text.len())),
            );
            field.update(cx, |field, cx| field.insert(token, window, cx).ok());
        }
        let waiting: Vec<u64> = self
            .images
            .iter()
            .filter(|a| !a.placed)
            .map(|a| a.id)
            .collect();
        for id in waiting {
            let Some(attached) = self.images.iter_mut().find(|a| a.id == id) else {
                continue;
            };
            attached.placed = true;
            // Its number is its place among the chips before the caret: one put at the end needs
            // no other renumbered, and unfolds as it is.
            let before = {
                let input = field.read(cx).input().read(cx);
                let caret = input.cursor();
                input
                    .tokens()
                    .iter()
                    .filter(|span| span.range().end <= caret)
                    .filter(|span| tokens::picture_of(span.token().id()).is_some())
                    .count()
            };
            let token = Token::new(
                tokens::picture_id(id),
                tokens::picture_look(before + 1, &attached.name, Some(attached.image.bytes.len())),
            );
            field.update(cx, |field, cx| field.insert(token, window, cx).ok());
        }
    }

    /// The text changed: the numbers and the thumbnails follow the order of the chips.
    pub fn follow<T: 'static>(&mut self, cx: &mut Context<T>) {
        if let Some(field) = self.field.clone() {
            self.follow_chips(&field, cx);
        }
    }

    /// The thumbnails in the order of the chips (those without one after them), and each chip
    /// numbered by its place. A chip still folding away waits: the numbers are not set under it.
    fn follow_chips<T: 'static>(&mut self, field: &Entity<TokenField>, cx: &mut Context<T>) {
        let value = field.read(cx).value(cx);
        let chips = value
            .iter()
            .filter(|piece| matches!(piece, Piece::Token(_)))
            .count();
        if field.read(cx).input().read(cx).tokens().len() != chips {
            return;
        }
        let order: Vec<u64> = value
            .iter()
            .filter_map(|piece| match piece {
                Piece::Token(token) => tokens::picture_of(&token.id),
                Piece::Text(_) => None,
            })
            .collect();
        let rank = |a: &Attached| {
            order
                .iter()
                .position(|id| *id == a.id)
                .unwrap_or(usize::MAX)
        };
        if self.images.windows(2).any(|w| rank(&w[0]) > rank(&w[1])) {
            self.images.sort_by_key(rank);
            cx.notify();
        }
        // A chip is renumbered in place: no undo step of its own, so the person's undo still
        // reaches what they did (a removed picture comes back with its number).
        let renumbered: Vec<(SharedString, TokenLook)> = value
            .into_iter()
            .filter_map(|piece| {
                let Piece::Token(token) = piece else {
                    return None;
                };
                let at = tokens::picture_of(&token.id)
                    .and_then(|id| self.images.iter().position(|a| a.id == id))?;
                let attached = &self.images[at];
                let look =
                    tokens::picture_look(at + 1, &attached.name, Some(attached.image.bytes.len()));
                (look != token.look).then_some((token.id, look))
            })
            .collect();
        field.update(cx, |field, cx| {
            for (id, look) in renumbered {
                field.relabel(&id, look, cx);
            }
        });
    }

    /// The message as it is sent: its text, each picture's chip as its marker
    /// (`[Image 1: photo.png]`), and the pictures in that order. A picture without a chip goes
    /// after the text, where the daemon marks it. A chip copied again in the text is its label:
    /// the picture goes once, at its first chip.
    pub fn message(&self, cx: &gpui_kit::App) -> String {
        let Some(field) = &self.field else {
            return String::new();
        };
        let pieces = field.read(cx).value(cx);
        let mut marked = Vec::new();
        let mut texts = Vec::new();
        tokens::sent_text(&pieces, |id| {
            // A pasted text's chip is its marker, numbered in the order of the chips.
            if let Some(id) = tokens::pasted_of(id) {
                let pasted = self.texts.iter().find(|t| t.id == id)?;
                if texts.contains(&id) {
                    return None;
                }
                texts.push(id);
                return Some(flottille_core::pasted::marker(texts.len(), &pasted.name));
            }
            let id = tokens::picture_of(id)?;
            if marked.contains(&id) {
                return None;
            }
            marked.push(id);
            let at = self.images.iter().position(|a| a.id == id)?;
            Some(flottille_core::images::marker(
                at + 1,
                &self.images[at].name,
            ))
        })
    }

    /// The pasted texts that go with the message [`Attachments::message`] makes, in the order of
    /// their markers: the order of their chips, the ones without a chip after.
    pub fn texts(&self, cx: &gpui_kit::App) -> Vec<Pasted> {
        let in_field: Vec<u64> = self
            .field
            .as_ref()
            .map(|field| {
                field
                    .read(cx)
                    .value(cx)
                    .iter()
                    .filter_map(|piece| match piece {
                        Piece::Token(token) => tokens::pasted_of(&token.id),
                        Piece::Text(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut order: Vec<&PastedText> = Vec::new();
        for id in &in_field {
            if let Some(pasted) = self.texts.iter().find(|t| t.id == *id)
                && !order.iter().any(|seen| seen.id == pasted.id)
            {
                order.push(pasted);
            }
        }
        for pasted in &self.texts {
            if !order.iter().any(|seen| seen.id == pasted.id) {
                order.push(pasted);
            }
        }
        order
            .into_iter()
            .map(|pasted| Pasted {
                name: pasted.name.clone(),
                text: pasted.text.to_string(),
            })
            .collect()
    }

    /// The pictures a message taken back carries, with its text: each marker of the text is the
    /// chip of the picture of its number, the others go after the text. The pieces for the field.
    /// The limits of an attachment hold: an agent that takes no images gets none back, and a
    /// message carries [`MAX_IMAGES`] at most, each refusal said in a toast; a marker left without
    /// its picture stays text.
    pub fn restore_with<T: 'static>(
        &mut self,
        text: &str,
        pictures: Vec<Picture>,
        pasted: Vec<Pasted>,
        cx: &mut Context<T>,
    ) -> Vec<Piece> {
        let mut ids = Vec::new();
        // A pasted text comes back as it was: its chip, where its marker is.
        let room = flottille_core::pasted::MAX_TEXTS.saturating_sub(self.texts.len());
        let mut kept = Vec::new();
        for pasted in pasted.into_iter().take(room) {
            let id = next_id();
            kept.push((id, pasted.name.clone(), pasted.text.len()));
            self.texts.push(PastedText {
                id,
                name: pasted.name,
                text: pasted.text.into(),
                placed: true,
            });
        }
        if !pictures.is_empty() && self.takes == Some(false) {
            Self::not_attached(NO_IMAGES, cx);
        }
        let room = match self.takes {
            Some(false) => 0,
            _ => MAX_IMAGES.saturating_sub(self.images.len() + self.scaling.len()),
        };
        if self.takes != Some(false) && pictures.len() > room {
            Self::not_attached(
                format!("A message carries {MAX_IMAGES} images at most."),
                cx,
            );
        }
        for picture in pictures.into_iter().take(room) {
            let Some(format) = gpui_kit::ImageFormat::from_mime_type(&picture.mime) else {
                continue;
            };
            let image = Arc::new(gpui_kit::Image::from_bytes(format, picture.bytes));
            ids.push(self.push(image, format!("image.{}", extension(format)), true));
        }
        let images = &mut self.images;
        let pieces = tokens::pieces_of(
            text,
            |_| false,
            |picture, found| {
                let id = *ids.get(picture)?;
                let attached = images.iter_mut().find(|a| a.id == id)?;
                attached.name = found.name.clone();
                Some(Token::new(
                    tokens::picture_id(id),
                    tokens::picture_look(
                        found.number,
                        &found.name,
                        Some(attached.image.bytes.len()),
                    ),
                ))
            },
        );
        // The marker of a number is the text of that number, once.
        let mut used = Vec::new();
        tokens::text_chips(pieces, |found| {
            let (id, name, bytes) = kept.get(found.number.checked_sub(1)?)?;
            if found.file.is_some() || used.contains(id) {
                return None;
            }
            used.push(*id);
            Some(Token::new(
                tokens::pasted_id(*id),
                tokens::pasted_look(name, Some(*bytes)),
            ))
        })
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
            // Ctrl+Shift+V asked for this one inline; the next is as any other.
            if std::mem::take(&mut self.inline) {
                return false;
            }
            return match clipboard
                .text()
                .filter(|text| flottille_core::pasted::is_large(text))
            {
                Some(text) => {
                    self.attach_text(text, cx);
                    true
                }
                None => false,
            };
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
        self.sent = self.pictures();
        self.images
            .iter()
            .map(|a| Picture {
                mime: a.image.format.mime_type().to_owned(),
                bytes: a.image.bytes.clone(),
            })
            .collect()
    }

    /// The pasted texts of a message that leaves the composer at once, in the order of its
    /// markers: they go with it.
    pub fn take_texts(&mut self, cx: &gpui_kit::App) -> Vec<Pasted> {
        let texts = self.texts(cx);
        self.texts.clear();
        self.gone_texts.clear();
        texts
    }

    /// The pictures of a message that leaves the composer at once: they go with it, and fold
    /// away from the strip.
    pub fn take<T: 'static>(&mut self, cx: &mut Context<T>) -> Vec<Arc<gpui_kit::Image>> {
        cx.notify();
        self.gone.clear();
        std::mem::take(&mut self.images)
            .into_iter()
            .map(|a| a.image)
            .collect()
    }

    /// The message was taken: the pictures it held leave, and those changed meanwhile stay.
    pub fn sent(&mut self) {
        let sent = std::mem::take(&mut self.sent);
        self.images
            .retain(|a| !sent.iter().any(|sent| Arc::ptr_eq(sent, &a.image)));
    }

    /// A session started: nothing stays attached for the next one.
    pub fn clear(&mut self) {
        self.sent.clear();
        self.images.clear();
        self.gone.clear();
        self.texts.clear();
        self.gone_texts.clear();
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
            // Ctrl+Shift+V is a Paste that keeps a large text inline (#848).
            .capture_key_down(cx.listener(
                move |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    let keys = &event.keystroke;
                    if keys.key == "v" && keys.modifiers.secondary() && keys.modifiers.shift {
                        of(this).inline = true;
                        window.dispatch_action(Paste.boxed_clone(), cx);
                        cx.stop_propagation();
                    }
                },
            ))
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
        detach: fn(&mut T, usize, &mut Window, &mut Context<T>),
    ) -> AnyElement {
        let p = palette(cx);
        let viewer = self
            .viewer
            .get_or_insert_with(|| cx.new(|_| PictureViewer::new()))
            .clone();
        let attached = self.pictures();
        let now = cx.background_executor().now();
        self.thumbs.sync(
            attached.iter().map(|i| (thumb_key(i), i.clone())),
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
        self.numbers
            .retain(|key, _| listed.iter().any(|(listed, ..)| listed == key));
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
                if !gone {
                    self.numbers.insert(key.clone(), at + 1);
                }
                // The badge folds away with its thumbnail, its number as it was.
                let badge = self.numbers.get(&key).map(|&shown| {
                    let id = match gone {
                        true => format!("attachment-leaving-number-{key}"),
                        false => format!("attachment-number-{at}"),
                    };
                    number(ElementId::Name(id.into()), &key, shown)
                });
                let thumb = div()
                    .id(id)
                    .relative()
                    .flex_none()
                    .size(px(56.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(p.line2)
                    .child(
                        gpui_kit::img(image)
                            .size_full()
                            .rounded(px(7.))
                            .object_fit(gpui_kit::ObjectFit::Cover),
                    )
                    .children(badge)
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
                                // Removing is not opening: the click stays with the button.
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    detach(this, at, window, cx)
                                })),
                            // An icon does not inherit the ink: it is drawn here, in the ink this frame.
                            move |d, t| {
                                d.bg(mix(p.card2, p.hover, t)).child(
                                    icon(Icon::Close)
                                        .size(px(12.))
                                        .text_color(mix(p.mute, p.text, t)),
                                )
                            },
                        ))
                    });
                let thumb = if gone {
                    thumb.test_support().into_any_element()
                } else {
                    let (viewer, attached) = (viewer.clone(), attached.clone());
                    // A click opens it large; its border fades in on the hover.
                    hoverable(
                        thumb.cursor_pointer().on_click(move |_, _, cx| {
                            viewer.update(cx, |viewer, cx| viewer.show(attached.clone(), at, cx));
                        }),
                        move |d, t| d.border_color(mix(p.line2, p.mute, t)),
                    )
                    .into_any_element()
                };
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
        div()
            .child(presence(
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
            ))
            .child(viewer)
            .into_any_element()
    }
}
