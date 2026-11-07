//! A picture of the conversation (or of the composer) looked at large: in the large modal, at its
//! own size, scaled down to fit the window and never up, centred. When the message holds several
//! pictures the arrows (the keys and two buttons) go through them, and the header says which.
//!
//! One viewer is held by each place that draws thumbnails (the session's page for the bubbles, a
//! composer's strip for what is attached) and drawn once, at the end of it: the modal takes no room
//! where it is drawn. A thumbnail's click calls [`PictureViewer::show`].

use std::sync::Arc;

use flottille_ui::motion::PANEL;
use flottille_ui::{Button, Icon, Modal, fade, space};
use gpui_kit::{
    AnyElement, App, Context, ElementId, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, StyledImage as _, TestSupportExt as _, Window, div, px,
};

/// What the dialog's header, its margin and its body's padding take of the window's height, which
/// the picture must leave: a little more than they do, so that it never touches the dialog's edge.
const AROUND_HEIGHT: f32 = 72. + 16. + 24.;
/// The same in width: the body's padding on each side.
const AROUND_WIDTH: f32 = 48.;

pub struct PictureViewer {
    pictures: Vec<Arc<gpui_kit::Image>>,
    at: usize,
    open: bool,
    /// How many times it was opened: the pictures of an earlier opening do not fade out in the
    /// place of this one's.
    opening: usize,
}

impl PictureViewer {
    pub fn new() -> Self {
        Self {
            pictures: Vec::new(),
            at: 0,
            open: false,
            opening: 0,
        }
    }

    /// Opens `pictures`, at the `at`th. The ones it showed last stay in it while it fades out.
    pub fn show(&mut self, pictures: Vec<Arc<gpui_kit::Image>>, at: usize, cx: &mut Context<Self>) {
        if pictures.is_empty() {
            return;
        }
        self.at = at.min(pictures.len() - 1);
        self.pictures = pictures;
        self.open = true;
        self.opening += 1;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    /// Whether it is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The previous (`-1`) or the next (`1`) picture; the first has none before it, nor the last
    /// one after it.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let to = self
            .at
            .checked_add_signed(by)
            .filter(|to| *to < self.pictures.len());
        if let Some(to) = to {
            self.at = to;
            cx.notify();
        }
    }

    /// The size a picture is drawn at: its own, scaled down to fit what the dialog leaves it.
    fn fitted(&self, picture: &gpui_kit::Image, window: &Window) -> (f32, f32) {
        let viewport = window.viewport_size();
        let room = (
            f32::from(viewport.width - space::XL * 2.) - AROUND_WIDTH,
            f32::from(viewport.height - space::XL * 2.) - AROUND_HEIGHT,
        );
        let (room_w, room_h) = (room.0.max(1.), room.1.max(1.));
        let mime = picture.format.mime_type();
        match flottille_core::images::dimensions(mime, &picture.bytes) {
            Some((w, h)) if w > 0 && h > 0 => {
                // A picture is its pixels: on a screen that scales, they are fewer points.
                let scale = window.scale_factor();
                let (w, h) = (w as f32 / scale, h as f32 / scale);
                let fit = (room_w / w).min(room_h / h).min(1.0);
                (w * fit, h * fit)
            }
            // Its size cannot be read: it fills the room, kept in proportion by `Contain`.
            _ => (room_w, room_h),
        }
    }
}

impl Default for PictureViewer {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for PictureViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewer = cx.entity().downgrade();
        let count = self.pictures.len();
        let mut modal = Modal::new("picture-modal", "Picture")
            .large()
            .takes_focus()
            .open(self.open)
            .on_close({
                let viewer = viewer.clone();
                move |_, cx| {
                    viewer.update(cx, |viewer, cx| viewer.close(cx)).ok();
                }
            });
        if count > 1 {
            let step = |by: isize| {
                let viewer = viewer.clone();
                move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                    viewer.update(cx, |viewer, cx| viewer.step(by, cx)).ok();
                }
            };
            modal = modal
                .sentence(format!("{} of {count}", self.at + 1))
                .action(
                    Button::icon("picture-previous", Icon::Previous, "Previous picture (←)")
                        .disabled(self.at == 0)
                        .on_click(step(-1)),
                )
                .action(
                    Button::icon("picture-next", Icon::Next, "Next picture (→)")
                        .disabled(self.at + 1 == count)
                        .on_click(step(1)),
                )
                .on_key({
                    let viewer = viewer.clone();
                    move |event: &KeyDownEvent, _, cx| {
                        let by = match event.keystroke.key.as_str() {
                            "left" => -1,
                            "right" => 1,
                            _ => return,
                        };
                        cx.stop_propagation();
                        viewer.update(cx, |viewer, cx| viewer.step(by, cx)).ok();
                    }
                });
        }
        // Each picture is a layer of the same stage, fading in as it becomes the one shown and
        // out as another does: going to another cross-fades.
        let layers: Vec<AnyElement> = (0..count)
            .filter_map(|n| {
                let shown = fade(
                    ElementId::Name(format!("picture-shown-{}-{n}", self.opening).into()),
                    n == self.at,
                    PANEL,
                    window,
                    cx,
                );
                if shown == 0.0 && n != self.at {
                    return None;
                }
                let picture = self.pictures[n].clone();
                let (w, h) = self.fitted(&picture, window);
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id(ElementId::Name(format!("picture-large-{n}").into()))
                                .flex_none()
                                .w(px(w))
                                .h(px(h))
                                .opacity(shown)
                                .child(
                                    gpui_kit::img(picture)
                                        .size_full()
                                        .object_fit(gpui_kit::ObjectFit::Contain),
                                )
                                .test_support(),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        if count > 0 {
            modal = modal.child(
                div()
                    .id("picture-stage")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .children(layers),
            );
        }
        modal
    }
}
