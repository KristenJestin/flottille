//! Colour arithmetic: hex parsing, mixing, and the WCAG contrast check every ink goes through.

use gpui_kit::{Hsla, Rgba};

/// Text must reach this contrast ratio against its background (WCAG AA).
pub const TEXT_CONTRAST: f32 = 4.5;

pub fn hex(value: u32) -> Hsla {
    gpui_kit::rgb(value).into()
}

/// Parses `#rrggbb` or `#rrggbbaa`, the form theme files use.
pub fn parse_hex(text: &str) -> Option<Hsla> {
    let digits = text.trim().strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        6 => Some(gpui_kit::rgb(value).into()),
        8 => Some(gpui_kit::rgba(value).into()),
        _ => None,
    }
}

/// `a` moved toward `b` by `t` (0 is `a`, 1 is `b`), in RGB. A negative `t` moves away from `b`.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let lerp = |x: f32, y: f32| (x + (y - x) * t).clamp(0.0, 1.0);
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

/// `color` at `alpha` laid over an opaque `base`, as an opaque colour.
pub fn over(color: Hsla, alpha: f32, base: Hsla) -> Hsla {
    mix(base, color.alpha(1.0), alpha)
}

fn relative_luminance(color: Hsla) -> f32 {
    let c = color.to_rgb();
    let channel = |v: f32| {
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// The WCAG 2 contrast ratio, from 1 to 21.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `ink` unchanged if it reads on `background`, otherwise moved toward white or black (whichever
/// the background lets it reach) until it reaches `minimum`.
pub fn readable(ink: Hsla, background: Hsla, minimum: f32) -> Hsla {
    if contrast(ink, background) >= minimum {
        return ink;
    }
    let white = hex(0xffffff);
    let black = hex(0x000000);
    let toward = if contrast(white, background) >= contrast(black, background) {
        white
    } else {
        black
    };
    let mut t: f32 = 0.0;
    while t < 1.0 {
        t += 0.02;
        let moved = mix(ink, toward, t.min(1.0));
        if contrast(moved, background) >= minimum {
            return moved;
        }
    }
    toward
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_hex_values() {
        assert_eq!(parse_hex("#f0883e"), Some(hex(0xf0883e)));
        assert_eq!(parse_hex(" #0e1116 "), Some(hex(0x0e1116)));
        assert!(parse_hex("#ff82").is_none());
        assert!(parse_hex("f0883e").is_none());
        assert!(parse_hex("#gg8226").is_none());
    }

    #[test]
    fn contrast_follows_wcag() {
        assert!((contrast(hex(0xffffff), hex(0x000000)) - 21.0).abs() < 0.01);
        assert!((contrast(hex(0x777777), hex(0x777777)) - 1.0).abs() < 0.01);
        // A known pair: #767676 on white is the lightest grey that passes AA.
        assert!((contrast(hex(0x767676), hex(0xffffff)) - 4.54).abs() < 0.02);
    }

    #[test]
    fn an_ink_that_reads_is_kept() {
        let ink = hex(0xececec);
        assert_eq!(readable(ink, hex(0x141414), TEXT_CONTRAST), ink);
    }

    #[test]
    fn a_low_contrast_ink_is_moved_until_it_reads() {
        let background = hex(0x202020);
        let ink = readable(hex(0x2a2a2a), background, TEXT_CONTRAST);
        assert!(contrast(ink, background) >= TEXT_CONTRAST);
        let light = readable(hex(0xdddddd), hex(0xeeeeee), TEXT_CONTRAST);
        assert!(contrast(light, hex(0xeeeeee)) >= TEXT_CONTRAST);
        assert!(light.l < 0.5, "on a light background the ink darkens");
    }
}
