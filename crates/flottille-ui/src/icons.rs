//! One stroked icon family (Lucide), only the icons the components use embedded. An application
//! that needs more declares them itself: another Lucide icon by its name ([`Icon::Lucide`]) or a
//! glyph of its own ([`Icon::Asset`], [`Icon::Filled`]), and serves their files from the assets it
//! registers (compose [`Icons`] with them).

use std::borrow::Cow;

use gpui_kit::assets::IconName;
use gpui_kit::{AssetSource, ElementId, Result, SharedString, Styled, Svg, px, svg};

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
        KeyRound,
        ChevronLeft,
        ChevronRight,
        PanelRightOpen,
        FoldVertical,
        Unlink,
        Unplug,
        Trash,
        Replace
    ]
);

/// The assets of the components: the Lucide icons the base set of [`Icon`] uses. An application
/// registers its own assets, which serve [`Icons`] first or last, with the files of the icons it
/// adds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Lucide.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Lucide.list(path)
    }
}

/// An icon: one of the base set, or one the application declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Any other Lucide icon, by its file name (`"git-pull-request"`). The application embeds it
    /// in its assets.
    Lucide(&'static str),
    /// A stroked glyph of the application's own: the path of an asset it registers.
    Asset(&'static str),
    /// A solid glyph of the application's own, which a button draws larger than a stroked one
    /// (see [`Icon::scale`]): the path of an asset it registers.
    Filled(&'static str),
    /// A folder.
    Folder,
    /// A branch of a repository.
    Branch,
    /// A repository, a book of its history.
    Repository,
    /// Points down; a row turns it as it folds, a picker shows it closed.
    Chevron,
    /// Adds: a new item.
    Plus,
    /// Searches.
    Search,
    /// Notifications.
    Bell,
    /// Opens something in another program.
    OpenInEditor,
    /// Opens a list of places.
    Menu,
    /// The composer's send button.
    Send,
    /// A picker's button: it opens a list to choose from.
    Choose,
    /// The settings page.
    Settings,
    /// A "⋯" button: it opens a small menu.
    More,
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
    /// A file.
    File,
    /// A text document.
    Text,
    /// A command, typed `/name`.
    Slash,
    /// Runs a task.
    Play,
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
    /// Goes to the previous one: a picture of a message.
    Previous,
    /// Goes to the next one: a picture of a message.
    Next,
    /// Opens the right panel: the details of what is shown.
    PanelRight,
    /// Folds a conversation into less: Compact.
    Fold,
    /// Cuts a link: a workspace's ticket.
    Unlink,
    /// Cuts one thing off another: Detach.
    Detach,
    /// Removes: the destructive row of a menu.
    Trash,
    /// Swaps one thing for another: Change the ticket.
    Change,
}

impl Icon {
    pub const ALL: [Self; 42] = [
        Self::Folder,
        Self::Branch,
        Self::Repository,
        Self::Chevron,
        Self::Plus,
        Self::Search,
        Self::Bell,
        Self::OpenInEditor,
        Self::Menu,
        Self::Send,
        Self::Choose,
        Self::Settings,
        Self::More,
        Self::Terminal,
        Self::ChevronUp,
        Self::Warning,
        Self::Check,
        Self::Close,
        Self::File,
        Self::Text,
        Self::Slash,
        Self::Play,
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
        Self::Previous,
        Self::Next,
        Self::PanelRight,
        Self::Fold,
        Self::Unlink,
        Self::Detach,
        Self::Trash,
        Self::Change,
    ];

    /// The path of the icon's file in the assets.
    pub fn path(self) -> SharedString {
        match self {
            Self::Lucide(name) => return format!("icons/{name}.svg").into(),
            Self::Asset(path) | Self::Filled(path) => return path.into(),
            _ => {}
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
            Self::Menu => IconName::Menu,
            Self::Send => IconName::ArrowUp,
            Self::Choose => IconName::ChevronsUpDown,
            Self::Settings => IconName::Settings,
            Self::More => IconName::Ellipsis,
            Self::Terminal => IconName::Terminal,
            Self::ChevronUp => IconName::ChevronUp,
            Self::Warning => IconName::TriangleAlert,
            Self::Check => IconName::Check,
            Self::Close => IconName::X,
            Self::File => IconName::File,
            Self::Text => IconName::FileText,
            Self::Slash => IconName::Slash,
            Self::Play => IconName::Play,
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
            Self::Previous => IconName::ChevronLeft,
            Self::Next => IconName::ChevronRight,
            Self::PanelRight => IconName::PanelRightOpen,
            Self::Fold => IconName::FoldVertical,
            Self::Unlink => IconName::Unlink,
            Self::Detach => IconName::Unplug,
            Self::Trash => IconName::Trash,
            Self::Change => IconName::Replace,
            Self::Lucide(_) | Self::Asset(_) | Self::Filled(_) => {
                unreachable!("an icon of the application, returned above")
            }
        }
        .path()
    }
}

impl Icon {
    /// The factor by which a button draws this glyph larger than the others: a small filled
    /// square would otherwise look smaller than a stroked glyph of the same box.
    pub fn scale(self) -> f32 {
        if matches!(self, Self::Filled(_)) {
            1.4
        } else {
            1.0
        }
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
    fn an_icon_of_the_application_is_found_by_its_name_or_its_path() {
        assert_eq!(Icon::Lucide("network").path(), "icons/network.svg");
        assert_eq!(Icon::Asset("icons/mark.svg").path(), "icons/mark.svg");
        assert_eq!(Icon::Filled("icons/solid.svg").path(), "icons/solid.svg");
    }

    #[test]
    fn only_a_solid_glyph_is_drawn_larger() {
        assert_eq!(Icon::Filled("icons/solid.svg").scale(), 1.4);
        assert_eq!(Icon::Asset("icons/mark.svg").scale(), 1.0);
        assert_eq!(Icon::Plus.scale(), 1.0);
    }

    #[test]
    fn the_ended_mark_is_a_stroked_outline() {
        let bytes = Icons.load(&Icon::Stopped.path()).unwrap().unwrap();
        let text = String::from_utf8(bytes.into_owned()).unwrap();
        assert!(text.contains("stroke=\"currentColor\""));
        assert!(text.contains("fill=\"none\""));
    }
}
