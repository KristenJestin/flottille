//! Omarchy mode: on an Omarchy desktop the palette comes from the current Omarchy theme, and
//! follows it live.
//!
//! The theme is read from `$XDG_STATE_HOME/omarchy/current/theme/colors.toml`. Its backgrounds and
//! foregrounds give the surfaces and text levels, its `accent` the primary, its blue, yellow,
//! green and red the states. Missing keys are derived by mixing `background` and `foreground`;
//! every ink is then checked for contrast and moved until it reads. A theme missing a required
//! key, or unreadable, leaves the base theme in place, with the reason logged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui_kit::{App, Global, Hsla};

use crate::color::{hex, mix, over, parse_hex};
use crate::theme::{self, Palette};

/// Where the desktop is described; injectable so tests never read the real machine.
#[derive(Clone, Debug)]
pub struct Environment {
    pub os_release: PathBuf,
    pub state_home: PathBuf,
}

impl Environment {
    /// `/etc/os-release` and `$XDG_STATE_HOME` (or `~/.local/state`).
    pub fn from_system() -> Self {
        let state_home = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_default();
        Self {
            os_release: PathBuf::from("/etc/os-release"),
            state_home,
        }
    }

    /// `ID=omarchy` in os-release.
    pub fn is_omarchy(&self) -> bool {
        std::fs::read_to_string(&self.os_release).is_ok_and(|text| {
            text.lines().any(|line| {
                line.trim()
                    .strip_prefix("ID=")
                    .is_some_and(|id| id.trim_matches(['"', '\'']) == "omarchy")
            })
        })
    }

    pub fn colors_path(&self) -> PathBuf {
        self.state_home.join("omarchy/current/theme/colors.toml")
    }
}

/// Keys a theme must give; everything else can be derived.
const REQUIRED: [&str; 3] = ["background", "foreground", "accent"];

/// Maps a `colors.toml` to the window's tokens.
pub fn palette_from_toml(text: &str) -> Result<Palette, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("colors.toml does not parse: {e}"))?;
    let mut colors = HashMap::new();
    for (key, value) in &table {
        let Some(text) = value.as_str() else { continue };
        let color = parse_hex(text).ok_or_else(|| format!("`{key}` is not a colour: {text:?}"))?;
        colors.insert(key.as_str(), color);
    }
    for key in REQUIRED {
        if !colors.contains_key(key) {
            return Err(format!("colors.toml has no `{key}`"));
        }
    }
    // The first name present wins; the ANSI names are what older theme files give.
    let get = |names: &[&str]| names.iter().find_map(|n| colors.get(n).copied());
    let bg = colors["background"];
    let fg = colors["foreground"];
    let accent = colors["accent"];
    let toward_fg = |t: f32| mix(bg, fg, t);

    let darker = get(&["darker_background"]).unwrap_or_else(|| toward_fg(-0.05));
    let dark = get(&["dark_background"]).unwrap_or_else(|| toward_fg(-0.02));
    let lighter = get(&["lighter_background"]).unwrap_or_else(|| toward_fg(0.04));
    let bright_fg = get(&["bright_foreground"]).unwrap_or_else(|| mix(fg, bg, -0.15));
    let dark_fg = get(&["dark_foreground"]).unwrap_or_else(|| mix(fg, bg, 0.4));
    let selection =
        get(&["selection", "selection_background"]).unwrap_or_else(|| mix(bg, accent, 0.15));
    let base = Palette::base();
    let blue = get(&["blue", "color4"]).unwrap_or(base.run);
    let yellow = get(&["yellow", "color3"]).unwrap_or(base.wait);
    let green = get(&["green", "color2"]).unwrap_or(base.ok);
    let red = get(&["red", "color1"]).unwrap_or(base.fail);
    let tint = |c: Hsla| over(c, 0.15, bg);

    Ok(Palette {
        body: darker,
        bg,
        rail: dark,
        side: dark,
        card: lighter,
        card2: toward_fg(0.06),
        line: toward_fg(0.08),
        line2: toward_fg(0.12),
        hover: toward_fg(0.04),
        sel: selection,
        text: bright_fg,
        mute: mix(fg, dark_fg, 0.5),
        faint: dark_fg,
        acc: accent,
        on_acc: bg,
        shell: lighter,
        field: dark,
        field_line: toward_fg(0.10),
        focus_line: over(accent, 0.4, dark),
        focus_ring: over(accent, 0.08, lighter),
        run: blue,
        run_bg: tint(blue),
        wait: yellow,
        wait_bg: tint(yellow),
        on_wait: bg,
        wait_line: over(yellow, 0.3, lighter),
        ok: green,
        ok_bg: tint(green),
        fail: red,
        fail_bg: tint(red),
        on_fail: bg,
        fail_line: over(red, 0.3, lighter),
        add: over(green, 0.13, bg),
        del: over(red, 0.13, bg),
        scrim: hex(0x000000).opacity(0.5),
    }
    .with_readable_inks())
}

/// The theme to use, and why the base theme was kept when Omarchy's could not be.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Not an Omarchy desktop.
    Base,
    Omarchy(Box<Palette>),
    /// An Omarchy desktop whose theme could not be used.
    Fallback(String),
}

impl Choice {
    pub fn palette(&self) -> Palette {
        match self {
            Self::Omarchy(palette) => **palette,
            Self::Base | Self::Fallback(_) => Palette::base(),
        }
    }
}

pub fn choose(env: &Environment) -> Choice {
    if !env.is_omarchy() {
        return Choice::Base;
    }
    let path = env.colors_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            palette_from_toml(&text).map_or_else(Choice::Fallback, |p| Choice::Omarchy(Box::new(p)))
        }
        Err(e) => Choice::Fallback(format!("cannot read {}: {e}", path.display())),
    }
}

/// The last reason the Omarchy theme was set aside, kept beside the log line for whoever asks.
#[derive(Default)]
struct ThemeLog {
    fallback: Option<String>,
}

impl Global for ThemeLog {}

pub fn last_fallback(cx: &App) -> Option<String> {
    cx.try_global::<ThemeLog>()
        .and_then(|log| log.fallback.clone())
}

fn apply(choice: Choice, cx: &mut App) {
    let fallback = match &choice {
        Choice::Fallback(reason) => {
            eprintln!("flottille: the Omarchy theme is not used, the base theme is: {reason}");
            Some(reason.clone())
        }
        Choice::Base | Choice::Omarchy(_) => None,
    };
    cx.default_global::<ThemeLog>().fallback = fallback;
    theme::set_palette(choice.palette(), cx);
}

/// How often the theme file is looked at. A look is one `stat`; nothing is redrawn unless the
/// file changed.
pub const POLL: Duration = Duration::from_secs(1);

/// What identifies one version of the theme file: the file behind the `current` link, and its
/// modification time.
fn stamp(path: &Path) -> Option<(PathBuf, SystemTime)> {
    let real = std::fs::canonicalize(path).ok()?;
    let modified = std::fs::metadata(&real).and_then(|m| m.modified()).ok()?;
    Some((real, modified))
}

/// Sets the palette from `env`, and on an Omarchy desktop keeps following the theme file.
pub fn install(env: Environment, cx: &mut App) {
    apply(choose(&env), cx);
    if !env.is_omarchy() {
        return;
    }
    let path = env.colors_path();
    let mut seen = stamp(&path);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            let now = stamp(&path);
            if now != seen {
                seen = now;
                let choice = choose(&env);
                cx.update(|cx| apply(choice, cx));
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{TEXT_CONTRAST, contrast};

    #[test]
    fn os_release_names_omarchy() {
        let dir = tempfile::tempdir().unwrap();
        let env = Environment {
            os_release: dir.path().join("os-release"),
            state_home: dir.path().into(),
        };
        assert!(!env.is_omarchy(), "no file, no Omarchy");
        std::fs::write(&env.os_release, "NAME=\"Arch Linux\"\nID=arch\n").unwrap();
        assert!(!env.is_omarchy());
        std::fs::write(
            &env.os_release,
            "NAME=Omarchy\nID=\"omarchy\"\nID_LIKE=arch\n",
        )
        .unwrap();
        assert!(env.is_omarchy());
    }

    #[test]
    fn derived_keys_fill_a_minimal_theme() {
        let p = palette_from_toml(
            "background = \"#1a1b26\"\nforeground = \"#a9b1d6\"\naccent = \"#7aa2f7\"\n",
        )
        .unwrap();
        assert_eq!(p.bg, hex(0x1a1b26));
        assert_eq!(p.acc, hex(0x7aa2f7));
        assert!(p.card.l > p.bg.l && p.rail.l < p.bg.l && p.body.l < p.rail.l);
        assert!(contrast(p.text, p.card) >= TEXT_CONTRAST);
    }

    #[test]
    fn ansi_names_give_the_state_colours() {
        let p = palette_from_toml(
            "background = \"#101010\"\nforeground = \"#d0d0d0\"\naccent = \"#88aaff\"\n\
             color1 = \"#ff5f5f\"\ncolor2 = \"#5fd75f\"\ncolor3 = \"#ffd75f\"\ncolor4 = \"#5fafff\"\n",
        )
        .unwrap();
        assert_eq!(
            [p.fail, p.ok, p.wait, p.run],
            [hex(0xff5f5f), hex(0x5fd75f), hex(0xffd75f), hex(0x5fafff)]
        );
    }

    #[test]
    fn a_bad_value_is_a_reason() {
        let err = palette_from_toml(
            "background = \"#101010\"\nforeground = \"nope\"\naccent = \"#88aaff\"\n",
        )
        .unwrap_err();
        assert!(err.contains("foreground"), "{err}");
    }
}
