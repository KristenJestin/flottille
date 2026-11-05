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
    /// The ink on a `fail` fill: a danger button.
    pub on_fail: Hsla,
    /// The border of what failed: a failure's toast.
    pub fail_line: Hsla,
    /// Diff line backgrounds; their inks are `ok` and `fail`.
    pub add: Hsla,
    pub del: Hsla,
    /// What dims the content under a drawer.
    pub scrim: Hsla,
    /// The black of the shadow of a drawer, a menu, a dialog or a toast, at full strength: each
    /// takes its own share (`p.shadow.opacity(0.5)`). Lighter in Light, where the same share
    /// would be heavy.
    pub shadow: Hsla,
}

/// Flottille's own palette comes light or dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shade {
    Light,
    #[default]
    Dark,
}

impl Palette {
    /// Flottille's own palette in that shade.
    pub fn flottille(shade: Shade) -> Self {
        match shade {
            Shade::Light => Self::light(),
            Shade::Dark => Self::base(),
        }
    }

    /// Flottille's own light palette: the dark one's roles on a near-white page. The bars and the
    /// cards sit a step toward the ink (never white, which would be the page itself), the states
    /// are deepened until they read as text on every surface, and the primary is a near-black.
    pub fn light() -> Self {
        Self {
            body: hex(0xebebea),
            bg: hex(0xfbfbfa),
            rail: hex(0xf2f2f1),
            side: hex(0xf5f5f4),
            card: hex(0xf4f4f3),
            card2: hex(0xebebea),
            line: hex(0xe2e2e0),
            line2: hex(0xd4d4d2),
            hover: hex(0xededec),
            sel: hex(0xe5e5e3),
            text: hex(0x171717),
            mute: hex(0x4f4f4e),
            faint: hex(0x636362),
            acc: hex(0x1c1c1c),
            on_acc: hex(0xfafafa),
            shell: hex(0xf4f4f3),
            field: hex(0xfdfdfd),
            field_line: hex(0xdadad8),
            focus_line: hex(0xb8b8b6),
            focus_ring: hex(0xe9e9e7),
            run: hex(0x2251c4),
            run_bg: hex(0xe7edfb),
            wait: hex(0x8f5200),
            wait_bg: hex(0xfaefdc),
            on_wait: hex(0xffffff),
            wait_line: hex(0xe8cf9e),
            ok: hex(0x17703a),
            ok_bg: hex(0xe2f2e7),
            fail: hex(0xb3261e),
            fail_bg: hex(0xfbe7e5),
            on_fail: hex(0xffffff),
            fail_line: hex(0xeab8b3),
            add: hex(0xe4f3e8),
            del: hex(0xfbe8e6),
            scrim: gpui_kit::rgba(0x0000004d).into(),
            shadow: gpui_kit::rgba(0x0000004d).into(),
        }
    }

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
            on_fail: hex(0x1a0606),
            fail_line: hex(0x5a2a2a),
            add: hex(0x132a1b),
            del: hex(0x2c1515),
            scrim: gpui_kit::rgba(0x00000080).into(),
            shadow: gpui_kit::rgba(0x000000ff).into(),
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
        self.on_fail = readable(self.on_fail, self.fail, min);
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
            on_fail: m(self.on_fail, to.on_fail),
            fail_line: m(self.fail_line, to.fail_line),
            add: m(self.add, to.add),
            del: m(self.del, to.del),
            scrim: m(self.scrim, to.scrim),
            shadow: m(self.shadow, to.shadow),
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
                // The fields that follow the theme restyle themselves a frame at a time.
                cx.global_mut::<Theme>();
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

    #[test]
    fn the_shadows_of_light_are_lighter_and_those_of_dark_are_unchanged() {
        let (light, dark) = (Palette::flottille(Shade::Light), Palette::base());
        // Dark: a black at full strength, each overlay takes its own share, as before.
        assert_eq!((dark.shadow.l, dark.shadow.a), (0.0, 1.0));
        // A shadow that is deep on a dark page is heavy on a light one.
        assert_eq!(light.shadow.l, 0.0);
        assert!(light.shadow.a < 0.5, "a lighter shadow: {}", light.shadow.a);
    }

    #[test]
    fn the_light_palette_has_every_role_and_its_text_reads() {
        let light = Palette::flottille(Shade::Light);
        let dark = Palette::flottille(Shade::Dark);
        assert_eq!(dark, Palette::base(), "Flottille Dark is today's palette");
        assert!(light.bg.l > 0.9, "a light background");
        assert_eq!(light.with_readable_inks(), light, "it needs no correction");
        // The surfaces keep the dark palette's order of distance from the background: the body
        // and the bars set back, the cards a step toward the ink, never white.
        assert!(light.body.l < light.rail.l && light.rail.l < light.bg.l);
        assert!(light.card.l < light.bg.l && light.card2.l < light.card.l);
        assert!(light.line.l < light.card.l && light.line2.l < light.line.l);
        // Every ink against every surface it is drawn on.
        let surfaces = [
            ("body", light.body),
            ("bg", light.bg),
            ("rail", light.rail),
            ("side", light.side),
            ("card", light.card),
            ("card2", light.card2),
            ("hover", light.hover),
            ("sel", light.sel),
            ("shell", light.shell),
            ("field", light.field),
        ];
        let inks = [
            ("text", light.text),
            ("mute", light.mute),
            ("faint", light.faint),
            ("run", light.run),
            ("wait", light.wait),
            ("ok", light.ok),
            ("fail", light.fail),
        ];
        for (ink_name, ink) in inks {
            for (surface_name, surface) in surfaces {
                let ratio = contrast(ink, surface);
                assert!(
                    ratio >= TEXT_CONTRAST,
                    "{ink_name} on {surface_name}: {ratio:.2}"
                );
            }
        }
        for (name, ink, fill) in [
            ("on_acc", light.on_acc, light.acc),
            ("run", light.run, light.run_bg),
            ("wait", light.wait, light.wait_bg),
            ("on_wait", light.on_wait, light.wait),
            ("ok", light.ok, light.ok_bg),
            ("fail", light.fail, light.fail_bg),
            ("on_fail", light.on_fail, light.fail),
            ("ok on add", light.ok, light.add),
            ("fail on del", light.fail, light.del),
            ("text on add", light.text, light.add),
            ("text on del", light.text, light.del),
        ] {
            let ratio = contrast(ink, fill);
            assert!(ratio >= TEXT_CONTRAST, "{name}: {ratio:.2}");
        }
    }
}
