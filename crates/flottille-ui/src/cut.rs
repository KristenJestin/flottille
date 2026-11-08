//! A text cut with an ellipsis says itself whole: the tooltip of [`tipped`], shown only when the
//! text really is cut, so that a text that fits has none.
//!
//! GPUI lays out the element and clips its text, but never says whether it did. A probe, an
//! empty canvas laid over the element, shapes the whole text in the element's own font once the
//! frame is laid out and compares it with the room it has (its width less its horizontal
//! padding). The answer is kept for the next frame,
//! so the tooltip comes one frame after the text got cut (or whole again), which a pointer
//! resting for [`TOOLTIP`](crate::motion::TOOLTIP) never sees.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, DefiniteLength, Div, ElementId, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement, Styled,
    TestSupportExt, Window, canvas, px,
};

use crate::button::tipped;

/// What the probe saw the last time the element was laid out.
#[derive(Clone, Copy, Default, PartialEq)]
struct Seen {
    cut: bool,
    height: Pixels,
}

/// `element`, a one-line text clipped by its frame (`overflow_hidden` and `text_ellipsis`), with `text` for its tooltip when `text` is wider than the element. It
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
    // The probe covers the padding box: the text's room is what the padding leaves.
    let padding = element.style().padding.clone();
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
                let rem = window.rem_size();
                let pad = |side: Option<DefiniteLength>| match side {
                    Some(DefiniteLength::Absolute(length)) => length.to_pixels(rem),
                    _ => px(0.),
                };
                let room = bounds.size.width - pad(padding.left) - pad(padding.right);
                let found = Seen {
                    // Half a pixel of rounding is not a cut.
                    cut: whole > room + px(0.5),
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

/// How many characters of a title a sentence that is only a string (a toast, a menu entry, a
/// tooltip) quotes before it cuts it with an ellipsis.
pub const QUOTED_CHARS: usize = 40;

/// A title quoted in a sentence that is only a string: its first line, cut at [`QUOTED_CHARS`]
/// with an ellipsis, so that it never wraps the sentence onto lines of its own.
pub fn quoted_title(title: &str) -> String {
    let line = title.lines().next().unwrap_or_default().trim();
    let more = title.trim().lines().nth(1).is_some();
    let mut chars = line.chars();
    let cut: String = chars.by_ref().take(QUOTED_CHARS).collect();
    if chars.next().is_some() || more {
        format!("{}…", cut.trim_end())
    } else {
        cut
    }
}

/// A sentence that quotes a session's title: `before`, the title, then `after`, on one line. The
/// title takes what room the words leave, in the window's emphasis (medium, in the text's ink), cut
/// with an ellipsis and said whole in its tooltip when cut; it never wraps the sentence. The
/// element is `id`, labelled with the whole sentence; the title is `<id>-title`. Style the size and
/// colour of the words on a parent: they inherit it.
pub fn quoted(
    id: impl Into<SharedString>,
    before: &str,
    title: &str,
    after: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    use gpui_kit::FontWeight;
    let id: SharedString = id.into();
    let p = crate::theme::palette(cx);
    let title: SharedString = title.lines().next().unwrap_or_default().trim().to_owned().into();
    let whole = format!("{before}{title}{after}");
    let words = |text: &str| {
        (!text.is_empty()).then(|| {
            gpui_kit::div()
                .flex_none()
                .whitespace_nowrap()
                .child(SharedString::from(text.replace(' ', "\u{a0}")))
        })
    };
    let quoted = tipped_when_cut(
        gpui_kit::div()
            .id(ElementId::Name(format!("{id}-title").into()))
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.text)
            .child(title.clone()),
        title,
        window,
        cx,
    );
    gpui_kit::div()
        .id(ElementId::Name(id))
        .min_w_0()
        .flex()
        .items_baseline()
        .overflow_hidden()
        .whitespace_nowrap()
        .aria_label(SharedString::from(whole))
        .children(words(before))
        .child(quoted)
        .children(words(after))
        .test_support()
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_title_is_its_first_line_cut_with_an_ellipsis() {
        assert_eq!(quoted_title("Fix the login"), "Fix the login");
        assert_eq!(quoted_title("Fix the login\nand more"), "Fix the login…");
        let long = "Rework every page of the site with a new design system and fonts";
        let cut = quoted_title(long);
        assert!(cut.ends_with('…') && cut.chars().count() <= QUOTED_CHARS + 1, "{cut}");
    }
}
