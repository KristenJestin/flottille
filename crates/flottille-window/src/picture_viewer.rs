//! A picture of the conversation (or of the composer) looked at large: in a dialog that wraps it,
//! at its own size, scaled down to fit the window and never up, until it is zoomed. When the
//! message holds several pictures the arrows (the keys and two buttons) go through them, and the
//! header says which; the dialog then resizes to the next one while the pictures cross-fade.
//!
//! The zoom goes by steps: fit (where it opens), 100 %, 200 % and 400 % (the ones above fit). The
//! buttons of the header, the keys `+`, `-` and `0`, Ctrl and the wheel, and a double click on the
//! picture move through them; the scale eases over `PANEL`. A picture larger than the dialog is
//! dragged to pan, or scrolled with the wheel, and the dialog never grows past the window.
//!
//! One viewer is held by each place that draws thumbnails (the session's page for the bubbles, a
//! composer's strip for what is attached) and drawn once, at the end of it: the modal takes no room
//! where it is drawn. A thumbnail's click calls [`PictureViewer::show`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_design_system::motion::{Motion, PANEL};
use gpui_design_system::{Button, Icon, Modal, fade, space};
use gpui_kit::{
    AnyElement, App, Context, ElementId, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, TestSupportExt as _, Window, div, point,
    px,
};

/// What the dialog's header, its margin and its body's padding take of the window's height, which
/// the picture must leave: a little more than they do, so that it never touches the dialog's edge.
const AROUND_HEIGHT: f32 = 72. + 16. + 24.;
/// The same in width: the body's padding on each side.
const AROUND_WIDTH: f32 = 48.;
/// The zoom steps above fit, in percent of the picture's own size.
const STEPS: [u32; 3] = [100, 200, 400];
/// The width the zoom's words are given in the header, so that saying them moves nothing.
const ZOOM_WORDS: f32 = 52.;

/// What the viewer drew last, to know what moved since.
#[derive(Clone, Copy, PartialEq)]
struct Drawn {
    /// Which opening and which picture.
    picture: (usize, usize),
    /// The scale and the size of the body.
    scale: f32,
    stage: (f32, f32),
    content: (f32, f32),
}

pub struct PictureViewer {
    pictures: Vec<Arc<gpui_kit::Image>>,
    at: usize,
    open: bool,
    /// How many times it was opened: the pictures of an earlier opening do not fade out in the
    /// place of this one's.
    opening: usize,
    /// The step the picture is at, in percent of its own size; `None` is fit.
    zoom: Option<u32>,
    /// The scale the picture is drawn at, as a part of its own size: it eases to the step's.
    scale: Motion,
    /// From the body's size when another picture was gone to, to the new one's.
    resize: Motion,
    resize_from: (f32, f32),
    drawn: Option<Drawn>,
    /// What the header said last, which it keeps while it fades out.
    said: SharedString,
    scroll: ScrollHandle,
    /// Where the pointer went down and where the picture was, while it is dragged.
    pan: Option<(Point<Pixels>, Point<Pixels>)>,
}

impl PictureViewer {
    pub fn new() -> Self {
        Self {
            pictures: Vec::new(),
            at: 0,
            open: false,
            opening: 0,
            zoom: None,
            scale: Motion::settled(1.0),
            resize: Motion::settled(1.0),
            resize_from: (0., 0.),
            drawn: None,
            said: SharedString::default(),
            scroll: ScrollHandle::new(),
            pan: None,
        }
    }

    /// Opens `pictures`, at the `at`th. The ones it showed last stay in it while it fades out.
    pub fn show(&mut self, pictures: Vec<Arc<gpui_kit::Image>>, at: usize, cx: &mut Context<Self>) {
        if pictures.is_empty() {
            return;
        }
        self.at = at.min(pictures.len() - 1);
        self.pictures = pictures;
        self.zoom = None;
        self.open = true;
        self.opening += 1;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.pan = None;
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
            // Another picture opens at fit.
            self.zoom = None;
            cx.notify();
        }
    }

    /// The steps the picture can be at, fit first: 100 % is the fit of a picture that fits at its
    /// own size, so it is not offered twice.
    fn levels(&self, window: &Window) -> Vec<Option<u32>> {
        if self.pictures.is_empty() {
            return vec![None];
        }
        let fit = self.fit_of(self.at, window);
        let above = STEPS.iter().filter(|p| **p as f32 / 100. > fit + 1e-3);
        std::iter::once(None)
            .chain(above.map(|p| Some(*p)))
            .collect()
    }

    /// One step up (`1`) or down (`-1`) the zoom.
    fn zoom_by(&mut self, by: isize, window: &Window, cx: &mut Context<Self>) {
        let levels = self.levels(window);
        let now = levels.iter().position(|l| *l == self.zoom).unwrap_or(0);
        let to = now
            .saturating_add_signed(by)
            .min(levels.len().saturating_sub(1));
        if to != now {
            self.zoom = levels[to];
            cx.notify();
        }
    }

    /// Fit, or 200 % when it is at fit: a double click.
    fn toggle_zoom(&mut self, cx: &mut Context<Self>) {
        self.zoom = if self.zoom.is_none() { Some(200) } else { None };
        cx.notify();
    }

    fn reset_zoom(&mut self, cx: &mut Context<Self>) {
        if self.zoom.is_some() {
            self.zoom = None;
            cx.notify();
        }
    }

    /// What the window leaves a picture of the dialog's body.
    fn room(window: &Window) -> (f32, f32) {
        let viewport = window.viewport_size();
        (
            (f32::from(viewport.width - space::XL * 2.) - AROUND_WIDTH).max(1.),
            (f32::from(viewport.height - space::XL * 2.) - AROUND_HEIGHT).max(1.),
        )
    }

    /// A picture's own size in points: its pixels are fewer points on a screen that scales. Its
    /// size cannot always be read: it then fills the room, kept in proportion by `Contain`.
    fn own(picture: &gpui_kit::Image, window: &Window) -> (f32, f32) {
        let mime = picture.format.mime_type();
        match flottille_core::images::dimensions(mime, &picture.bytes) {
            Some((w, h)) if w > 0 && h > 0 => {
                let scale = window.scale_factor();
                (w as f32 / scale, h as f32 / scale)
            }
            _ => Self::room(window),
        }
    }

    /// The part of its own size the `n`th picture is drawn at to fit what the window leaves: all of
    /// it, unless it is larger.
    fn fit_of(&self, n: usize, window: &Window) -> f32 {
        let (w, h) = Self::own(&self.pictures[n], window);
        let (room_w, room_h) = Self::room(window);
        (room_w / w).min(room_h / h).min(1.0)
    }
}

impl Default for PictureViewer {
    fn default() -> Self {
        Self::new()
    }
}

impl PictureViewer {
    /// Moves what is animated to `now` and says how the picture is drawn: the size of the dialog's
    /// body, the size of the picture and the scale it is at.
    fn advance(
        &mut self,
        now: Instant,
        reduce: bool,
        window: &mut Window,
    ) -> ((f32, f32), (f32, f32)) {
        let (room_w, room_h) = Self::room(window);
        let own = Self::own(&self.pictures[self.at], window);
        let target = match self.zoom {
            None => self.fit_of(self.at, window),
            Some(percent) => percent as f32 / 100.,
        };
        let picture = (self.opening, self.at);
        let before = self.drawn;
        if before.map(|d| d.picture) != Some(picture) {
            // A picture opens at fit, and the dialog goes from the size it had to the new one's.
            self.scale = Motion::settled(target);
            self.scroll.set_offset(point(px(0.), px(0.)));
            self.pan = None;
            match before {
                Some(d) if d.picture.0 == self.opening => {
                    self.resize_from = d.stage;
                    self.resize.play(0., 1., Duration::ZERO, PANEL, now, reduce);
                }
                _ => self.resize = Motion::settled(1.),
            }
        } else if (self.scale.target() - target).abs() > 1e-4 {
            self.scale.animate(target, PANEL, now, reduce);
        }
        let scale = self.scale.value(now);
        let content = (own.0 * scale, own.1 * scale);
        let wanted = (content.0.min(room_w), content.1.min(room_h));
        let t = self.resize.value(now);
        let stage = (
            self.resize_from.0 + (wanted.0 - self.resize_from.0) * t,
            self.resize_from.1 + (wanted.1 - self.resize_from.1) * t,
        );
        // A zoom keeps what is at the middle of the dialog at its middle.
        if let Some(d) = before
            && d.picture == picture
            && (d.scale - scale).abs() > 1e-6
        {
            let offset = self.scroll.offset();
            let keep = |offset: Pixels, was: f32, was_stage: f32, is: f32, is_stage: f32| {
                if is <= is_stage {
                    return 0.;
                }
                let at = if was > was_stage {
                    (-f32::from(offset) + was_stage / 2.) / was
                } else {
                    0.5
                };
                (is_stage / 2. - at * is).clamp(-(is - is_stage), 0.)
            };
            self.scroll.set_offset(point(
                px(keep(
                    offset.x,
                    own.0 * d.scale,
                    d.stage.0,
                    content.0,
                    stage.0,
                )),
                px(keep(
                    offset.y,
                    own.1 * d.scale,
                    d.stage.1,
                    content.1,
                    stage.1,
                )),
            ));
        }
        self.drawn = Some(Drawn {
            picture,
            scale,
            stage,
            content,
        });
        if self.scale.is_running(now) || self.resize.is_running(now) {
            window.request_animation_frame();
        } else {
            self.scale.settle(now);
            self.resize.settle(now);
        }
        (stage, content)
    }

    /// The picture dragged by `to`, from where it was when the pointer went down.
    fn pan_to(&mut self, to: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some((from, offset)), Some(drawn)) = (self.pan, self.drawn) else {
            return;
        };
        let moved = |offset: Pixels, from: Pixels, to: Pixels, content: f32, stage: f32| {
            px((f32::from(offset + to - from)).clamp(-(content - stage).max(0.), 0.))
        };
        self.scroll.set_offset(point(
            moved(offset.x, from.x, to.x, drawn.content.0, drawn.stage.0),
            moved(offset.y, from.y, to.y, drawn.content.1, drawn.stage.1),
        ));
        cx.notify();
    }
}

impl Render for PictureViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewer = cx.entity().downgrade();
        let count = self.pictures.len();
        let now = cx.background_executor().now();
        let ((stage_w, stage_h), (content_w, content_h)) = if count > 0 {
            self.advance(now, cx.reduce_motion(), window)
        } else {
            ((0., 0.), (0., 0.))
        };
        let mut modal = Modal::new("picture-modal", "Picture")
            .fitting()
            .takes_focus()
            .open(self.open)
            .on_close({
                let viewer = viewer.clone();
                move |_, cx| {
                    viewer.update(cx, |viewer, cx| viewer.close(cx)).ok();
                }
            });
        if count > 1 {
            modal = modal.sentence(format!("{} of {count}", self.at + 1));
        }

        // The header says the zoom when it is not fit: its place is kept, so that nothing moves,
        // and it fades in and out.
        let levels = self.levels(window);
        let level = levels.iter().position(|l| *l == self.zoom).unwrap_or(0);
        if let Some(percent) = self.zoom {
            self.said = format!("{percent} %").into();
        }
        let zoomed = fade(
            ElementId::Name("picture-zoom-shown".into()),
            self.zoom.is_some(),
            PANEL,
            window,
            cx,
        );
        let zoom = |by: isize| {
            let viewer = viewer.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                viewer
                    .update(cx, |viewer, cx| viewer.zoom_by(by, window, cx))
                    .ok();
            }
        };
        let palette = gpui_design_system::theme::palette(cx);
        modal = modal
            .action(
                Button::icon("picture-zoom-out", Icon::Minus, "Zoom out (\u{2212})")
                    .disabled(level == 0)
                    .on_click(zoom(-1)),
            )
            .action(
                div()
                    .id("picture-zoom-level")
                    .aria_label(if self.zoom.is_some() {
                        self.said.clone()
                    } else {
                        SharedString::default()
                    })
                    .flex_none()
                    .w(px(ZOOM_WORDS))
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.))
                    .text_color(palette.mute)
                    .opacity(zoomed)
                    .child(self.said.clone())
                    .test_support(),
            )
            .action(
                Button::icon("picture-zoom-in", Icon::Plus, "Zoom in (+)")
                    .disabled(level + 1 == levels.len())
                    .on_click(zoom(1)),
            );
        if count > 1 {
            let step = |by: isize| {
                let viewer = viewer.clone();
                move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                    viewer.update(cx, |viewer, cx| viewer.step(by, cx)).ok();
                }
            };
            modal = modal
                .action(
                    Button::icon("picture-previous", Icon::Previous, "Previous picture (←)")
                        .disabled(self.at == 0)
                        .on_click(step(-1)),
                )
                .action(
                    Button::icon("picture-next", Icon::Next, "Next picture (→)")
                        .disabled(self.at + 1 == count)
                        .on_click(step(1)),
                );
        }
        modal = modal.on_key({
            let viewer = viewer.clone();
            move |event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                if keystroke.modifiers.control
                    || keystroke.modifiers.alt
                    || keystroke.modifiers.platform
                {
                    return;
                }
                // `+` is shift and `=` on some layouts, and `0` is shift and `à` on others: the
                // character typed says it, the key is the fallback.
                let typed = keystroke.key_char.as_deref().unwrap_or(&keystroke.key);
                let typed = if typed.is_empty() {
                    &keystroke.key
                } else {
                    typed
                };
                match (keystroke.key.as_str(), typed) {
                    ("left", _) | ("right", _) => {
                        let by = if keystroke.key == "left" { -1 } else { 1 };
                        cx.stop_propagation();
                        viewer.update(cx, |viewer, cx| viewer.step(by, cx)).ok();
                    }
                    (_, "+" | "=") | ("+" | "=" | "add", _) => {
                        cx.stop_propagation();
                        viewer
                            .update(cx, |viewer, cx| viewer.zoom_by(1, window, cx))
                            .ok();
                    }
                    (_, "-") | ("-" | "minus" | "subtract", _) => {
                        cx.stop_propagation();
                        viewer
                            .update(cx, |viewer, cx| viewer.zoom_by(-1, window, cx))
                            .ok();
                    }
                    (_, "0") | ("0", _) => {
                        cx.stop_propagation();
                        viewer.update(cx, |viewer, cx| viewer.reset_zoom(cx)).ok();
                    }
                    _ => {}
                }
            }
        });

        // Each picture is a layer of the same stage, fading in as it becomes the one shown and
        // out as another does: going to another cross-fades. The one shown is panned and zoomed;
        // the one that leaves keeps the size it had.
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
                let (w, h) = if n == self.at {
                    (content_w, content_h)
                } else {
                    let own = Self::own(&picture, window);
                    let fit = self.fit_of(n, window);
                    (own.0 * fit, own.1 * fit)
                };
                let drawn = div()
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
                    .test_support();
                // Centred while it is smaller than the dialog, and scrolled once it is larger.
                let held = div()
                    .w(px(w.max(stage_w)))
                    .h(px(h.max(stage_h)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(drawn);
                if n != self.at {
                    return Some(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(held)
                            .into_any_element(),
                    );
                }
                let on = |viewer: &gpui_kit::WeakEntity<Self>| viewer.clone();
                Some(
                    div()
                        .id("picture-pan")
                        .absolute()
                        .inset_0()
                        .overflow_scroll()
                        .cursor_pointer()
                        .track_scroll(&self.scroll)
                        .on_mouse_down(MouseButton::Left, {
                            let viewer = on(&viewer);
                            move |event, _, cx| {
                                viewer
                                    .update(cx, |viewer, _| {
                                        viewer.pan = Some((event.position, viewer.scroll.offset()));
                                    })
                                    .ok();
                            }
                        })
                        .on_mouse_move({
                            let viewer = on(&viewer);
                            move |event: &gpui_kit::MouseMoveEvent, _, cx| {
                                viewer
                                    .update(cx, |viewer, cx| {
                                        if event.pressed_button == Some(MouseButton::Left) {
                                            viewer.pan_to(event.position, cx);
                                        } else {
                                            viewer.pan = None;
                                        }
                                    })
                                    .ok();
                            }
                        })
                        .on_mouse_up(MouseButton::Left, {
                            let viewer = on(&viewer);
                            move |_, _, cx| {
                                viewer.update(cx, |viewer, _| viewer.pan = None).ok();
                            }
                        })
                        .on_mouse_up_out(MouseButton::Left, {
                            let viewer = on(&viewer);
                            move |_, _, cx| {
                                viewer.update(cx, |viewer, _| viewer.pan = None).ok();
                            }
                        })
                        .on_click({
                            let viewer = on(&viewer);
                            move |event, _, cx| {
                                if event.click_count() == 2 {
                                    viewer.update(cx, |viewer, cx| viewer.toggle_zoom(cx)).ok();
                                }
                            }
                        })
                        .on_scroll_wheel({
                            let viewer = on(&viewer);
                            move |event: &gpui_kit::ScrollWheelEvent, window, cx| {
                                if !event.modifiers.control {
                                    return;
                                }
                                cx.stop_propagation();
                                let up = match event.delta {
                                    ScrollDelta::Pixels(delta) => delta.y,
                                    ScrollDelta::Lines(delta) => px(delta.y),
                                };
                                let by = if up > px(0.) {
                                    1
                                } else if up < px(0.) {
                                    -1
                                } else {
                                    return;
                                };
                                viewer
                                    .update(cx, |viewer, cx| viewer.zoom_by(by, window, cx))
                                    .ok();
                            }
                        })
                        .child(held)
                        .into_any_element(),
                )
            })
            .collect();
        if count > 0 {
            modal = modal.child(
                div()
                    .id("picture-stage")
                    .flex_none()
                    .relative()
                    .overflow_hidden()
                    .w(px(stage_w))
                    .h(px(stage_h))
                    .children(layers)
                    .test_support(),
            );
        }
        modal
    }
}
