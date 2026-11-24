//! What the sidebar and the page show until the daemon has answered: skeletons of the kit, so that
//! nothing looks finished before it is, and nothing is held back once it is ready.
//!
//! The page's skeleton is a veil laid over it that fades out when the daemon answered, whatever
//! the page under it is: what is ready is drawn at once, under the veil, and enters with its own
//! motion. The sidebar's is a block that stands in while it has no row at all and folds away, as a
//! `presence` does, as soon as it has one (or the daemon answered).

use gpui_design_system::motion::PANEL;
use gpui_design_system::{Palette, Skeleton, fade, space};
use gpui_kit::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    TestSupportExt as _, Window, div, px,
};

/// The sidebar's rows, as the projects' will be: a line each.
pub fn sidebar() -> impl IntoElement {
    div()
        .id("sidebar-skeleton")
        .flex()
        .flex_col()
        .gap(space::MD)
        .px(space::MD)
        .pt(space::MD)
        .pb(space::LG)
        .children(
            [150., 118., 138., 96.]
                .into_iter()
                .enumerate()
                .map(|(row, width)| {
                    Skeleton::new(ElementId::Name(format!("sidebar-skeleton-{row}").into()))
                        .w(px(width))
                        .h(px(14.))
                }),
        )
        .test_support()
}

/// The page's skeleton laid over it while `pending`, fading out over [`PANEL`] once the daemon
/// answered; `None` when it has gone. Build it on every frame.
pub fn veil(
    pending: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut App,
) -> Option<impl IntoElement> {
    let shown = fade("main-skeleton-veil", pending, PANEL, window, cx);
    if !pending && shown <= 0.0 {
        return None;
    }
    let block =
        |id: &'static str, width: f32, height: f32| Skeleton::new(id).w(px(width)).h(px(height));
    Some(
        div()
            .id("main-skeleton")
            .absolute()
            .inset_0()
            .bg(p.bg)
            .opacity(shown)
            .px(space::XL)
            .pt(space::XL)
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(block("main-skeleton-title", 220., 24.))
            .child(block("main-skeleton-line", 420., 14.))
            .child(
                div()
                    .pt(space::MD)
                    .child(Skeleton::new("main-skeleton-block").h(px(96.))),
            )
            .test_support(),
    )
}
