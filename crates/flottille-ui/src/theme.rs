//! The tokens every screen reads, and the theme that holds them.
//!
//! A screen never names a colour: it reads [`palette`]. Shapes, type, sizes and motion are not
//! tokens of the palette; they never change with the theme.

use std::time::Duration;

use gpui_kit::{App, Global, Hsla, Pixels, px};

use crate::color::{TEXT_CONTRAST, hex, mix, over, readable};
use crate::motion::{COVER, Motion};

/// The colours of the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Behind everything.
    pub body: Hsla,
    /// The main area.
    pub bg: Hsla,
    /// The top bar.
    pub rail: Hsla,
    /// The left tree and the right rail.
    pub side: Hsla,
    pub card: Hsla,
    /// The user's bubble, inline code, quiet pills.
    pub card2: Hsla,
    /// Borders and guides.
    pub line: Hsla,
    /// Stronger borders.
    pub line2: Hsla,
    pub hover: Hsla,
    /// The selected row (the open session).
    pub sel: Hsla,
    pub text: Hsla,
    pub mute: Hsla,
    pub faint: Hsla,
    /// The primary: primary buttons, the logo mark, the focus ring.
    pub acc: Hsla,
    /// The ink on the primary.
    pub on_acc: Hsla,
    /// The composer's outer shell.
    pub shell: Hsla,
    /// The composer's field.
    pub field: Hsla,
    pub field_line: Hsla,
    pub focus_line: Hsla,
    pub focus_ring: Hsla,
    pub run: Hsla,
    pub run_bg: Hsla,
    /// Waits for the user, and nothing else.
    pub wait: Hsla,
    pub wait_bg: Hsla,
    /// The ink on a `wait` badge.
    pub on_wait: Hsla,
    /// The border of an approval card.
    pub wait_line: Hsla,
    pub ok: Hsla,
    pub ok_bg: Hsla,
    pub fail: Hsla,
    pub fail_bg: Hsla,
    /// Diff line backgrounds; their inks are `ok` and `fail`.
    pub add: Hsla,
    pub del: Hsla,
    /// What dims the content under a drawer.
    pub scrim: Hsla,
}

impl Palette {
    /// Flottille's own dark palette: neutral greys, an off-white primary.
    pub fn base() -> Self {
        Self {
            body: hex(0x0b0b0b),
            bg: hex(0x141414),
            rail: hex(0x101010),
            side: hex(0x121212),
            card: hex(0x1b1b1b),
            card2: hex(0x222222),
            line: hex(0x262626),
            line2: hex(0x303030),
            hover: hex(0x212121),
            sel: hex(0x262626),
            text: hex(0xececec),
            mute: hex(0xa3a3a3),
            faint: hex(0x8a8a8a),
            acc: hex(0xececec),
            on_acc: hex(0x111111),
            shell: hex(0x1a1a1a),
            field: hex(0x121212),
            field_line: hex(0x2a2a2a),
            focus_line: hex(0x3a3a3a),
            focus_ring: hex(0x1f1f1f),
            run: hex(0x7aa7ff),
            run_bg: hex(0x18233a),
            wait: hex(0xf5a524),
            wait_bg: hex(0x2e2210),
            on_wait: hex(0x1d1300),
            wait_line: hex(0x4a3511),
            ok: hex(0x4ade80),
            ok_bg: hex(0x15291c),
            fail: hex(0xf87171),
            fail_bg: over(hex(0xf87171), 0.15, hex(0x141414)),
            add: hex(0x132a1b),
            del: hex(0x2c1515),
            scrim: gpui_kit::rgba(0x00000080).into(),
        }
    }

    /// Every ink checked against what it is drawn on, and moved until it reads.
    pub fn with_readable_inks(mut self) -> Self {
        let min = TEXT_CONTRAST;
        self.text = readable(self.text, self.card, min);
        self.mute = readable(self.mute, self.card, min);
        self.faint = readable(self.faint, self.card2, min);
        self.on_acc = readable(self.on_acc, self.acc, min);
        self.run = readable(self.run, self.run_bg, min);
        self.wait = readable(self.wait, self.wait_bg, min);
        self.on_wait = readable(self.on_wait, self.wait, min);
        self.ok = readable(self.ok, self.ok_bg, min);
        self.fail = readable(self.fail, self.fail_bg, min);
        self
    }

    /// Every token `t` of the way from `self` to `to`.
    pub fn mix(&self, to: &Self, t: f32) -> Self {
        let m = |a: Hsla, b: Hsla| mix(a, b, t);
        Self {
            body: m(self.body, to.body),
            bg: m(self.bg, to.bg),
            rail: m(self.rail, to.rail),
            side: m(self.side, to.side),
            card: m(self.card, to.card),
            card2: m(self.card2, to.card2),
            line: m(self.line, to.line),
            line2: m(self.line2, to.line2),
            hover: m(self.hover, to.hover),
            sel: m(self.sel, to.sel),
            text: m(self.text, to.text),
            mute: m(self.mute, to.mute),
            faint: m(self.faint, to.faint),
            acc: m(self.acc, to.acc),
            on_acc: m(self.on_acc, to.on_acc),
            shell: m(self.shell, to.shell),
            field: m(self.field, to.field),
            field_line: m(self.field_line, to.field_line),
            focus_line: m(self.focus_line, to.focus_line),
            focus_ring: m(self.focus_ring, to.focus_ring),
            run: m(self.run, to.run),
            run_bg: m(self.run_bg, to.run_bg),
            wait: m(self.wait, to.wait),
            wait_bg: m(self.wait_bg, to.wait_bg),
            on_wait: m(self.on_wait, to.on_wait),
            wait_line: m(self.wait_line, to.wait_line),
            ok: m(self.ok, to.ok),
            ok_bg: m(self.ok_bg, to.ok_bg),
            fail: m(self.fail, to.fail),
            fail_bg: m(self.fail_bg, to.fail_bg),
            add: m(self.add, to.add),
            del: m(self.del, to.del),
            scrim: m(self.scrim, to.scrim),
        }
    }
}

/// Radii, in pixels. They never change with the theme.
pub mod radius {
    use super::*;
    pub const ROW: Pixels = px(7.);
    pub const LIST_ROW: Pixels = px(8.);
    pub const CARD: Pixels = px(12.);
    pub const COMPOSER: Pixels = px(16.);
    pub const FIELD: Pixels = px(11.);
    pub const PILL: Pixels = px(5.);
    pub const BUTTON: Pixels = px(7.);
}

/// Spacing steps, in pixels.
pub mod space {
    use super::*;
    pub const XS: Pixels = px(4.);
    pub const SM: Pixels = px(6.);
    pub const MD: Pixels = px(8.);
    pub const LG: Pixels = px(12.);
    pub const XL: Pixels = px(14.);
    pub const PAGE: Pixels = px(28.);
    /// Paddings in a narrow window.
    pub const NARROW: Pixels = px(16.);
}

/// The palette in use, cross-fading toward a new one when the theme changes.
pub struct Theme {
    from: Palette,
    to: Palette,
    fade: Motion,
}

impl Global for Theme {}

impl Theme {
    fn new(palette: Palette) -> Self {
        Self {
            from: palette,
            to: palette,
            fade: Motion::settled(1.0),
        }
    }
}

/// The tokens to draw with now.
pub fn palette(cx: &App) -> Palette {
    let Some(theme) = cx.try_global::<Theme>() else {
        return Palette::base();
    };
    let now = cx.background_executor().now();
    if !theme.fade.is_running(now) {
        return theme.to;
    }
    theme.from.mix(&theme.to, theme.fade.value(now))
}

/// The palette the theme is heading to.
pub fn target_palette(cx: &App) -> Palette {
    cx.try_global::<Theme>()
        .map_or_else(Palette::base, |theme| theme.to)
}

/// Switches to `palette`, cross-fading every window over [`COVER`] (at once with reduced
/// motion). The fade redraws the windows at a fixed rate while it runs, then stops.
pub fn set_palette(palette: Palette, cx: &mut App) {
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    if !cx.has_global::<Theme>() {
        cx.set_global(Theme::new(palette));
        cx.refresh_windows();
        return;
    }
    let current = self::palette(cx);
    let theme = cx.global_mut::<Theme>();
    if theme.to == palette {
        return;
    }
    theme.from = current;
    theme.to = palette;
    theme
        .fade
        .play(0.0, 1.0, Duration::ZERO, COVER, now, reduce);
    cx.refresh_windows();
    if reduce {
        return;
    }
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(FADE_FRAME).await;
            let running = cx.update(|cx| {
                cx.refresh_windows();
                let now = cx.background_executor().now();
                cx.try_global::<Theme>()
                    .is_some_and(|theme| theme.fade.is_running(now))
            });
            if !running {
                break;
            }
        }
    })
    .detach();
}

/// The interval between two frames of the palette's cross-fade.
const FADE_FRAME: Duration = Duration::from_millis(16);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::contrast;

    #[test]
    fn the_base_palette_reads() {
        let p = Palette::base();
        assert_eq!(
            p.with_readable_inks(),
            p,
            "the base theme needs no correction"
        );
        assert!(contrast(p.text, p.bg) >= TEXT_CONTRAST);
        assert!(contrast(p.on_acc, p.acc) >= TEXT_CONTRAST);
    }
}
