//! Omarchy mode, on GPUI's test platform, with the desktop described by temporary files.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Desktop, init};
use flottille_ui::Palette;
use flottille_ui::color::{TEXT_CONTRAST, contrast, hex};
use flottille_ui::omarchy;
use flottille_ui::theme::{palette, target_palette};
use gpui_kit::TestAppContext;

/// A theme of invented colours, every key given.
const THEME: &str = r##"
accent = "#f0883e"
darker_background = "#07090c"
dark_background = "#0a0c10"
background = "#0e1116"
lighter_background = "#151a21"
selection = "#2a2117"
dark_foreground = "#7d8590"
foreground = "#c9d1d9"
bright_foreground = "#f0f6fc"
blue = "#58a6ff"
yellow = "#d29922"
green = "#3fb950"
red = "#f85149"
"##;

fn tokens_of(p: &Palette) -> [(&'static str, u32); 8] {
    let rgb = |c: gpui_kit::Hsla| {
        let c = c.to_rgb();
        let b = |v: f32| (v * 255.0).round() as u32;
        (b(c.r) << 16) | (b(c.g) << 8) | b(c.b)
    };
    [
        ("bg", rgb(p.bg)),
        ("rail", rgb(p.rail)),
        ("card", rgb(p.card)),
        ("sel", rgb(p.sel)),
        ("text", rgb(p.text)),
        ("faint", rgb(p.faint)),
        ("acc", rgb(p.acc)),
        ("run", rgb(p.run)),
    ]
}

#[gpui_kit::test]
fn omarchy_a_temporary_colors_toml_is_mapped_to_the_expected_tokens(cx: &mut TestAppContext) {
    let desktop = Desktop::new(true);
    desktop.write_colors(THEME);
    init(cx, desktop.env());
    let p = cx.update(|cx| palette(cx));
    assert_eq!(
        tokens_of(&p),
        [
            ("bg", 0x0e1116),
            ("rail", 0x0a0c10),
            ("card", 0x151a21),
            ("sel", 0x2a2117),
            ("text", 0xf0f6fc),
            ("faint", 0x7d8590),
            ("acc", 0xf0883e),
            ("run", 0x58a6ff),
        ]
    );
    assert_eq!(p.body, hex(0x07090c));
    assert_eq!(
        p.on_acc,
        hex(0x0e1116),
        "the ink on the primary is the background"
    );
    assert_eq!(
        [p.wait, p.ok, p.fail],
        [hex(0xd29922), hex(0x3fb950), hex(0xf85149)]
    );
    // The derived tokens sit between the background and the foreground.
    assert!(p.bg.l < p.card2.l && p.card2.l < p.line.l && p.line.l < p.line2.l);
    assert!(omarchy::last_fallback(&cx.app.borrow()).is_none());
}

#[gpui_kit::test]
fn omarchy_a_theme_with_a_low_contrast_accent_gets_a_readable_ink(cx: &mut TestAppContext) {
    let desktop = Desktop::new(true);
    desktop
        .write_colors("background = \"#1e1e1e\"\nforeground = \"#d4d4d4\"\naccent = \"#262626\"\n");
    init(cx, desktop.env());
    let p = cx.update(|cx| palette(cx));
    assert_eq!(p.acc, hex(0x262626));
    assert!(
        contrast(hex(0x1e1e1e), p.acc) < TEXT_CONTRAST,
        "the theme's own ink would not read"
    );
    assert!(
        contrast(p.on_acc, p.acc) >= TEXT_CONTRAST,
        "the ink on the primary reads"
    );
}

#[gpui_kit::test]
fn omarchy_a_missing_required_key_falls_back_to_the_base_theme_with_the_reason_logged(
    cx: &mut TestAppContext,
) {
    let desktop = Desktop::new(true);
    desktop.write_colors("background = \"#0e1116\"\nforeground = \"#c9d1d9\"\n");
    init(cx, desktop.env());
    assert_eq!(cx.update(|cx| palette(cx)), Palette::base());
    let reason = omarchy::last_fallback(&cx.app.borrow()).expect("a reason is logged");
    assert!(
        reason.contains("accent"),
        "the reason names the key: {reason}"
    );
}

#[gpui_kit::test]
fn omarchy_changing_the_file_changes_the_palette(cx: &mut TestAppContext) {
    let desktop = Desktop::new(true);
    desktop.write_colors(THEME);
    init(cx, desktop.env());
    assert_eq!(cx.update(|cx| palette(cx)).acc, hex(0xf0883e));

    // A theme switch points `current` elsewhere; here the file is rewritten, later in time.
    std::thread::sleep(Duration::from_millis(20));
    desktop.write_colors(&THEME.replace("#f0883e", "#7aa2f7"));
    touch_later(&desktop.env().colors_path());
    cx.executor()
        .advance_clock(omarchy::POLL + Duration::from_millis(10));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| target_palette(cx)).acc, hex(0x7aa2f7));
    // The palette cross-fades, then settles on the new theme.
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| palette(cx)).acc, hex(0x7aa2f7));
}

/// Makes sure the rewritten file's modification time differs, whatever the filesystem's
/// resolution.
fn touch_later(path: &Path) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    file.set_modified(later).unwrap();
}

#[gpui_kit::test]
fn without_omarchy_the_base_theme_is_used(cx: &mut TestAppContext) {
    let desktop = Desktop::new(false);
    // An Omarchy theme left on disk is not enough: the desktop must be Omarchy.
    desktop.write_colors(THEME);
    init(cx, desktop.env());
    assert_eq!(cx.update(|cx| palette(cx)), Palette::base());
    assert!(omarchy::last_fallback(&cx.app.borrow()).is_none());
}
