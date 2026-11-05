//! A text cut with an ellipsis says itself whole: the tooltip of [`tipped`], shown only when the
//! text really is cut, so that a text that fits has none.
//!
//! GPUI lays out the element and clips its text, but never says whether it did. A probe, an
//! empty canvas laid over the element, shapes the whole text in the element's own font once the
//! frame is laid out and compares it with the room it has. The answer is kept for the next frame,
//! so the tooltip comes one frame after the text got cut (or whole again), which a pointer
//! resting for [`TOOLTIP`](crate::motion::TOOLTIP) never sees.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    SharedString, Stateful, Styled, TestSupportExt, Window, canvas, px,
};

use crate::button::tipped;

/// What the probe saw the last time the element was laid out.
#[derive(Clone, Copy, Default, PartialEq)]
struct Seen {
    cut: bool,
    height: Pixels,
}

/// `element`, a one-line text clipped by its frame (`overflow_hidden` and `text_ellipsis`, no
/// padding of its own), with `text` for its tooltip when `text` is wider than the element. It
/// needs an id; the tooltip is `<id>-tooltip`.
pub fn tipped_when_cut(
    mut element: Stateful<Div>,
    text: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = element
        .interactivity()
        .element_id
        .clone()
        .expect("an element with a tooltip has an id");
    let text: SharedString = text.into();
    let key = ElementId::NamedChild(Arc::new(id), "cut".into());
    let seen = window.use_keyed_state(key, cx, |_, _| Seen::default());
    let now = *seen.read(cx);
    let probe = {
        let (seen, text) = (seen.clone(), text.clone());
        canvas(
            move |bounds, window, cx| {
                let style = window.text_style();
                let font_size = style.font_size.to_pixels(window.rem_size());
                // Shaping takes one line: a text with breaks is measured by its first.
                let line =
                    SharedString::from(text.split('\n').next().unwrap_or_default().to_owned());
                let whole = window
                    .text_system()
                    .shape_line(line.clone(), font_size, &[style.to_run(line.len())], None)
                    .width;
                let found = Seen {
                    // Half a pixel of rounding is not a cut.
                    cut: whole > bounds.size.width + px(0.5),
                    height: bounds.size.height,
                };
                if *seen.read(cx) != found {
                    seen.update(cx, |seen, cx| {
                        *seen = found;
                        cx.notify();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    };
    let element = element.relative().child(probe);
    if now.cut {
        tipped(element, text, now.height.into(), window, cx).into_any_element()
    } else {
        element.test_support().into_any_element()
    }
}
