//! Where a section of Settings is drawn while it holds settings that can be shared with every
//! machine (`docs/remote.md`, "Settings for all machines"), and what it draws there, the way
//! About you does (`crate::about`): on All machines it edits the values every machine uses, a
//! change signed with the device key; on a machine with a hub it says once that the values are all
//! machines' unless changed there, a value changed there is the machine's own (its edge in the
//! colour of what is set apart, one line `Changed on <machine>` and Use all machines' value), and
//! the way back drops it. Everything here moves: the line and the mark unfold and fold, the edge
//! fades over `PANEL`.

use gpui_design_system::motion::PANEL;
use gpui_design_system::{SetHere, palette, presence, space, tint};
use gpui_kit::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, Window, div, px,
};

pub use crate::about::{SHARED_LINE, USE_ALL};

/// Where a section is drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Place {
    /// On All machines: the section edits the values every machine uses.
    pub on_all: bool,
    /// The machine whose page it is, by name: what a value changed there says.
    pub machine: String,
    /// The machine has a hub and another machine to share with: Settings offers All machines.
    pub hub: bool,
}

impl Place {
    /// Whether a value the machine holds itself (`held`) is drawn as its own: a machine's page
    /// with a hub, never All machines.
    pub fn is_own(&self, held: bool) -> bool {
        held && self.follows()
    }

    /// Whether the page is a machine's, which follows the values of all machines unless it
    /// changed them.
    pub fn follows(&self) -> bool {
        !self.on_all && self.hub
    }
}

/// The line a machine's section says once: its values are all machines', unless changed there.
/// It unfolds and folds away with `shown`.
pub fn follows_line(id: &str, shown: bool, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    presence(
        ElementId::Name(format!("{id}-shared-block").into()),
        shown,
        div()
            .id(ElementId::Name(format!("{id}-shared").into()))
            .aria_label(SHARED_LINE)
            .text_size(px(12.5))
            .text_color(p.mute)
            .child(SHARED_LINE)
            .test_support(),
    )
    .space(space::MD)
}

/// `field` under the mark of a value the machine holds itself: `Changed on <machine>` and the way
/// back, both moving as it becomes the machine's own or follows all machines again.
pub fn set_here(
    id: &str,
    place: &Place,
    field: impl IntoElement,
    held: bool,
    on_reset: impl Fn(&mut Window, &mut App) + 'static,
) -> SetHere {
    SetHere::new(
        ElementId::Name(format!("{id}-own").into()),
        field,
        SharedString::from(format!("Changed on {}", place.machine)),
        USE_ALL,
    )
    .set(place.is_own(held))
    .on_reset(on_reset)
}

/// The edge of a value's box: the colour of what is set apart while the machine holds it itself,
/// the line's otherwise, fading over `PANEL`.
pub fn edge(id: &str, place: &Place, held: bool, window: &mut Window, cx: &mut App) -> Hsla {
    let p = palette(cx);
    let target = if place.is_own(held) { p.wait } else { p.line2 };
    tint(
        ElementId::Name(format!("{id}-edge").into()),
        target,
        PANEL,
        window,
        cx,
    )
}
