//! A file path on one line: its folder, faint, cut at its start when there is no room
//! (`…/components/mail-recipient-sheet/`), and its file name, always whole, cut at its end only
//! when it alone is wider than its frame. The whole path is in a tooltip, and is the label.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::button::tipped;
use crate::theme::palette;
use crate::typography::Type;

/// The height of the line, the tooltip's anchor.
const LINE: f32 = 20.;

/// `path` on one line, in mono at `size`: `<id>` is the line, `<id>-folder` and `<id>-name` its
/// two parts (no folder part for a bare name). The line shrinks inside a flex row
/// (`min_w(0)`), so a parent that holds it needs no more than room for its other children.
pub fn path_line(
    id: impl Into<ElementId>,
    path: &str,
    size: f32,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let id = id.into();
    let p = palette(cx);
    let (folder, name) = match path.rfind('/') {
        Some(at) => path.split_at(at + 1),
        None => ("", path),
    };
    let part = |suffix: &str| ElementId::Name(format!("{id}-{suffix}").into());
    let whole = SharedString::from(path.to_owned());
    let line = div()
        .id(id.clone())
        .flex()
        .items_center()
        .min_w(px(0.))
        .h(px(LINE))
        .overflow_hidden()
        .whitespace_nowrap()
        .mono(size)
        .aria_label(whole.clone())
        .when(!folder.is_empty(), |d| {
            d.child(
                div()
                    .id(part("folder"))
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_ellipsis_start()
                    .text_color(p.faint)
                    .child(SharedString::from(folder.to_owned()))
                    .test_support(),
            )
        })
        .child(
            // Its room is the line's: past it the name is cut at its end, never the line wider.
            div()
                .id(part("name"))
                .flex_none()
                .max_w_full()
                .overflow_hidden()
                .text_ellipsis()
                .aria_label(SharedString::from(name.to_owned()))
                .child(SharedString::from(name.to_owned()))
                .test_support(),
        );
    tipped(line, whole, LINE, window, cx)
}
