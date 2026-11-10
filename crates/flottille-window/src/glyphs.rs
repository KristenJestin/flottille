//! The icons the window adds to the kit's base set (`gpui-design-system`), and the assets that
//! serve them.

use std::borrow::Cow;

use gpui_design_system::{Icon, Icons};
use gpui_kit::{AssetSource, Result, SharedString};

/// The agents: a network of nodes.
pub const AGENTS: Icon = Icon::Lucide("network");
/// A pull request.
pub const PULL_REQUEST: Icon = Icon::Lucide("git-pull-request");
/// A ticket of the tracker.
pub const TICKET: Icon = Icon::Lucide("ticket");
/// The stop action: a small filled square, not Lucide's square in a circle, which blurs into a
/// muddy ring when it is drawn at 14 px.
pub const STOP: Icon = Icon::Filled(STOP_PATH);

const STOP_PATH: &str = "icons/flottille-stop.svg";
const STOP_SVG: &[u8] = include_bytes!("../assets/icons/stop.svg");

gpui_kit::assets::icon_assets!(
    Lucide,
    [
        GitPullRequest,
        Network,
        Ticket,
        FileText,
        ScanSearch,
        Shrink,
        Plug,
        Blocks
    ]
);

/// The assets of the window: the icons of the kit, the Lucide icons the window adds and its own
/// glyphs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == STOP_PATH {
            return Ok(Some(Cow::Borrowed(STOP_SVG)));
        }
        match Lucide.load(path)? {
            Some(found) => Ok(Some(found)),
            None => Icons.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Icons.list(path)?;
        paths.extend(Lucide.list(path)?);
        if STOP_PATH.starts_with(path) {
            paths.push(STOP_PATH.into());
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(icon: Icon) -> String {
        let bytes = Assets.load(&icon.path()).unwrap().unwrap();
        String::from_utf8(bytes.into_owned()).unwrap()
    }

    #[test]
    fn every_icon_the_window_adds_is_embedded_and_listed() {
        let listed = Assets.list("").unwrap();
        for icon in [AGENTS, PULL_REQUEST, TICKET, STOP] {
            assert!(
                Assets.load(&icon.path()).unwrap().is_some(),
                "{icon:?} is not embedded"
            );
            assert!(listed.contains(&icon.path()), "{icon:?} is not listed");
        }
        assert!(
            Assets.load(&Icon::Plus.path()).unwrap().is_some(),
            "the base set is served too"
        );
    }

    #[test]
    fn every_icon_a_declaration_can_name_is_embedded() {
        for name in [
            "file-text",
            "scan-search",
            "shrink",
            "plug",
            "something-else",
        ] {
            let icon = crate::extensions::icon_named(name);
            assert!(
                Assets.load(&icon.path()).unwrap().is_some(),
                "{name}: {icon:?} is not embedded"
            );
        }
    }

    #[test]
    fn stop_is_a_small_filled_rounded_square_of_flottille_s_own() {
        let svg = text(STOP);
        assert!(svg.contains("fill=\"currentColor\""), "{svg}");
        assert!(!svg.contains("stroke"), "no outline: {svg}");
        assert!(!svg.contains("<circle"), "no circle: {svg}");
        assert!(svg.contains("viewBox=\"0 0 24 24\""));
        let number = |name: &str| -> f32 {
            let at = svg.find(&format!(" {name}=\"")).unwrap() + name.len() + 3;
            svg[at..].split('"').next().unwrap().parse().unwrap()
        };
        let side = number("width");
        assert!((side / 24.0 - 0.45).abs() <= 0.03, "about 45 %: {side}");
        assert!(number("rx") > 0.0, "rounded corners");
        assert!(
            (number("x") + side / 2.0 - 12.0).abs() < 0.01
                && (number("y") + side / 2.0 - 12.0).abs() < 0.01,
            "centred"
        );
    }

    #[test]
    fn the_circled_stop_of_lucide_is_not_embedded_any_more() {
        let listed = Assets.list("").unwrap();
        assert!(!listed.iter().any(|path| path.contains("circle-stop")));
    }

    #[test]
    fn the_stop_action_and_the_ended_mark_are_told_apart() {
        assert_ne!(STOP.path(), Icon::Stopped.path());
        // A small filled square against a large outlined one.
        assert!(!text(STOP).contains("stroke"));
        assert!(text(Icon::Stopped).contains("stroke=\"currentColor\""));
        assert!(text(Icon::Stopped).contains("fill=\"none\""));
    }
}
