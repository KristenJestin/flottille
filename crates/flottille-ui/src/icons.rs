//! One stroked icon family (Lucide), only the icons the window uses embedded, and the few glyphs
//! of Flottille's own that Lucide has no stroked drawing for.

use std::borrow::Cow;

use gpui_kit::assets::IconName;
use gpui_kit::{AssetSource, ElementId, Result, SharedString, Styled, Svg, px, svg};

/// The stop action: a small filled square, not Lucide's square in a circle, which blurs into a
/// muddy ring when it is drawn at 14 px.
const STOP: (&str, &[u8]) = (
    "icons/flottille-stop.svg",
    include_bytes!("../assets/icons/stop.svg"),
);

gpui_kit::assets::icon_assets!(
    Lucide,
    [
        Folder,
        GitBranch,
        Book,
        ChevronDown,
        Plus,
        Search,
        Bell,
        ExternalLink,
        Network,
        Menu,
        ArrowUp,
        ArrowDown,
        ChevronsUpDown,
        Settings,
        Ellipsis,
        Terminal,
        ChevronUp,
        TriangleAlert,
        Check,
        X,
        File,
        FileText,
        Slash,
        Play,
        GitPullRequest,
        Ticket,
        GripVertical,
        Copy,
        Clock,
        Square,
        Minus,
        CircleDot,
        Pencil,
        Maximize2,
        RotateCw,
        Server,
        KeyRound
    ]
);

/// The assets of the window: the Lucide icons it uses and Flottille's own glyphs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == STOP.0 {
            return Ok(Some(Cow::Borrowed(STOP.1)));
        }
        Lucide.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Lucide.list(path)?;
        if STOP.0.starts_with(path) {
            paths.push(STOP.0.into());
        }
        Ok(paths)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Folder,
    Branch,
    Repository,
    Chevron,
    Plus,
    Search,
    Bell,
    OpenInEditor,
    Agents,
    Menu,
    /// The composer's send button.
    Send,
    /// A picker's button: it opens a list to choose from.
    Choose,
    /// The settings page.
    Settings,
    /// A "⋯" button: it opens a small menu.
    More,
    /// The composer's button that interrupts the turn that runs.
    Stop,
    /// A command is running (the left tree, the rail's Runs).
    Terminal,
    /// A button that moves a row up (a chevron whose tip points up).
    ChevronUp,
    /// Something needs a look: the composer's Auto with no judge chosen.
    Warning,
    /// Done: a success's toast, the chosen option of a select.
    Check,
    /// Closes or dismisses: a toast, a drawer.
    Close,
    /// A file of the project.
    File,
    /// A text Flottille keeps.
    Text,
    /// A command of the project, typed `/name`.
    Slash,
    /// Runs a task.
    Play,
    /// A pull request.
    PullRequest,
    /// A ticket of the tracker.
    Ticket,
    /// Drag to move.
    Grip,
    /// Copy to the clipboard.
    Copy,
    /// Waiting for its time.
    Clock,
    /// A command that was stopped, a stop mark.
    Stopped,
    /// A command that was interrupted before its end.
    Interrupted,
    /// Under way: a command that runs, checks that are pending.
    Running,
    /// Changes what was written.
    Edit,
    /// Shows a command large, in a modal.
    Expand,
    /// Runs a command again.
    Restart,
    /// Back to the end of a conversation.
    ToEnd,
    /// A service of the machine: the services page.
    Service,
    /// An entry of the vault: a secret of the user's.
    Key,
}

impl Icon {
    pub const ALL: [Self; 38] = [
        Self::Folder,
        Self::Branch,
        Self::Repository,
        Self::Chevron,
        Self::Plus,
        Self::Search,
        Self::Bell,
        Self::OpenInEditor,
        Self::Agents,
        Self::Menu,
        Self::Send,
        Self::Choose,
        Self::Settings,
        Self::More,
        Self::Stop,
        Self::Terminal,
        Self::ChevronUp,
        Self::Warning,
        Self::Check,
        Self::Close,
        Self::File,
        Self::Text,
        Self::Slash,
        Self::Play,
        Self::PullRequest,
        Self::Ticket,
        Self::Grip,
        Self::Copy,
        Self::Clock,
        Self::Stopped,
        Self::Interrupted,
        Self::Running,
        Self::Edit,
        Self::Expand,
        Self::Restart,
        Self::ToEnd,
        Self::Service,
        Self::Key,
    ];

    pub fn path(self) -> SharedString {
        if self == Self::Stop {
            return STOP.0.into();
        }
        match self {
            Self::Folder => IconName::Folder,
            Self::Branch => IconName::GitBranch,
            Self::Repository => IconName::Book,
            Self::Chevron => IconName::ChevronDown,
            Self::Plus => IconName::Plus,
            Self::Search => IconName::Search,
            Self::Bell => IconName::Bell,
            Self::OpenInEditor => IconName::ExternalLink,
            Self::Agents => IconName::Network,
            Self::Menu => IconName::Menu,
            Self::Send => IconName::ArrowUp,
            Self::Choose => IconName::ChevronsUpDown,
            Self::Settings => IconName::Settings,
            Self::More => IconName::Ellipsis,
            Self::Stop => unreachable!("Flottille's own glyph, returned above"),
            Self::Terminal => IconName::Terminal,
            Self::ChevronUp => IconName::ChevronUp,
            Self::Warning => IconName::TriangleAlert,
            Self::Check => IconName::Check,
            Self::Close => IconName::X,
            Self::File => IconName::File,
            Self::Text => IconName::FileText,
            Self::Slash => IconName::Slash,
            Self::Play => IconName::Play,
            Self::PullRequest => IconName::GitPullRequest,
            Self::Ticket => IconName::Ticket,
            Self::Grip => IconName::GripVertical,
            Self::Copy => IconName::Copy,
            Self::Clock => IconName::Clock,
            Self::Stopped => IconName::Square,
            Self::Interrupted => IconName::Minus,
            Self::Running => IconName::CircleDot,
            Self::Edit => IconName::Pencil,
            Self::Expand => IconName::Maximize2,
            Self::Restart => IconName::RotateCw,
            Self::ToEnd => IconName::ArrowDown,
            Self::Service => IconName::Server,
            Self::Key => IconName::KeyRound,
        }
        .path()
    }
}

impl Icon {
    /// The factor by which a button draws this glyph larger than the others: a filled square
    /// of 45 % of its box would otherwise look smaller than a stroked glyph of the same box.
    pub fn scale(self) -> f32 {
        if self == Self::Stop { 1.4 } else { 1.0 }
    }

    /// The id of the element that draws this glyph in a button, so a test finds it.
    pub fn glyph_id(self) -> ElementId {
        ElementId::Name(format!("glyph:{}", self.path()).into())
    }
}

/// An icon at 14 px. An svg does not inherit the text colour: give it one.
pub fn icon(icon: Icon) -> Svg {
    svg().path(icon.path()).size(px(14.)).flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(icon: Icon) -> String {
        let bytes = Icons.load(&icon.path()).unwrap().unwrap();
        String::from_utf8(bytes.into_owned()).unwrap()
    }

    #[test]
    fn every_icon_of_the_list_is_embedded() {
        for icon in Icon::ALL {
            assert!(
                Icons.load(&icon.path()).unwrap().is_some(),
                "{icon:?} is not embedded"
            );
        }
        let listed = Icons.list("").unwrap();
        for icon in Icon::ALL {
            assert!(listed.contains(&icon.path()), "{icon:?} is not listed");
        }
    }

    #[test]
    fn stop_is_a_small_filled_rounded_square_of_flottille_s_own() {
        let svg = text(Icon::Stop);
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
        let listed = Icons.list("").unwrap();
        assert!(!listed.iter().any(|path| path.contains("circle-stop")));
    }

    #[test]
    fn the_stop_action_and_the_ended_mark_are_told_apart() {
        assert_ne!(Icon::Stop.path(), Icon::Stopped.path());
        // A small filled square against a large outlined one.
        assert!(!text(Icon::Stop).contains("stroke"));
        assert!(text(Icon::Stopped).contains("stroke=\"currentColor\""));
        assert!(text(Icon::Stopped).contains("fill=\"none\""));
    }
}
