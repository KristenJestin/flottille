//! The guard of #772: a text never leaves its card. `assert_stays_inside(shell, cx, id)` waits for
//! `id` to be drawn, then looks at every element the window observes (`.test_support()`) under
//! the element `id` and refuses one whose bounds go past the right or left edge of it, or a card
//! that is wider than the window. A card is drawn at 360 px (`NARROW`) by the tests that use it,
//! so that a long word has no room to hide in.
//!
//! The guard sees what is observed: a text that can leave its card gets an id and
//! `.test_support()`, so that the guard sees it.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use flottille_window::Shell;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{ElementId, Entity, VisualTestContext, Window, px, size};

/// The width of the window the guard's tests draw in.
pub const NARROW: f32 = 360.;

/// A word of 90 letters with no space: no way to wrap it but at its end.
pub const UNBROKEN: &str =
    "Supercalifragilisticexpialidocious_Supercalifragilisticexpialidocious_Supercalifragilistic";

/// Words with spaces, long enough for four lines of a card 360 px wide.
pub const SPACED: &str = "the answer is a long sentence of ordinary words that has to wrap \
    inside its card and never run past its right edge, however many of them the user wrote";

/// Draws the window `NARROW` wide.
pub fn narrow(cx: &mut VisualTestContext) {
    cx.simulate_resize(size(px(NARROW), px(1800.)));
}

/// What of the element `id` leaves it, one line each: the observed elements below it that go
/// past its sides, and the element itself when it goes past the window's.
pub fn escapes(window: &mut Window, id: impl Into<ElementId>) -> Vec<String> {
    let id = id.into();
    let frame = window.find(id.clone()).bounds();
    let width = window.viewport_size().width;
    let mut out = Vec::new();
    if frame.right() > width + px(0.5) || frame.left() < px(-0.5) {
        out.push(format!(
            "{id:?} {frame:?} leaves the window ({width:?} wide)"
        ));
    }
    for below in window.within(id.clone()).elements() {
        let at = below.bounds();
        if !below.visible() || at.size.width <= px(0.) {
            continue;
        }
        if at.right() > frame.right() + px(0.5) || at.left() < frame.left() - px(0.5) {
            out.push(format!(
                "{:?} {at:?} leaves {id:?} {frame:?}",
                below.path().last()
            ));
        }
    }
    out
}

/// Asserts that nothing observed under `id` leaves it, and that the guard looked at something.
/// It waits for `id` to be drawn first, as the suites' `until` does: the shell applies what the
/// daemon sent only when it is polled, so a row written a moment ago comes after a poll.
pub fn assert_stays_inside(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    id: impl Into<ElementId>,
) {
    let id = id.into();
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        // Observed under a path of its own or only through its parts: say what is there.
        let (drawn, known) = cx.update(|window, _| {
            let known: Vec<String> = window
                .elements()
                .iter()
                .filter_map(|e| e.path().last().map(|id| format!("{id:?}")))
                .filter(|name| !name.contains("glyph:") && !name.contains("enter-"))
                .collect();
            (known.iter().any(|name| *name == format!("{id:?}")), known)
        });
        if drawn {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{id:?} is not drawn; the rows drawn are: {}",
            known.join(" ")
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let (found, escaped) = cx.update(|window, _| {
        let found = window.within(id.clone()).elements().len();
        if std::env::var_os("GUARD_DUMP").is_some() {
            eprintln!("GUARD frame {id:?} {:?}", window.find(id.clone()).bounds());
            for below in window.within(id.clone()).elements() {
                eprintln!("GUARD {:?} {:?}", below.path().last(), below.bounds());
            }
        }
        (found, escapes(window, id.clone()))
    });
    assert!(
        found > 0,
        "nothing observed under {id:?}: the guard saw nothing"
    );
    assert!(escaped.is_empty(), "{}", escaped.join("\n"));
}
