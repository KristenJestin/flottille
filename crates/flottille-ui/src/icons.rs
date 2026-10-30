//! One stroked icon family (Lucide), only the icons the window uses embedded.

use gpui_kit::assets::IconName;
use gpui_kit::{SharedString, Styled, Svg, px, svg};

gpui_kit::assets::icon_assets!(pub Icons, [Folder, GitBranch, Book, ChevronDown, Plus, Search, Bell, ExternalLink, Network, Menu, ArrowUp, ChevronsUpDown, Settings, Ellipsis, CircleStop, Terminal, ChevronUp, TriangleAlert, Check, X, File, FileText, Slash, Play, GitPullRequest, Ticket]);

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
}

impl Icon {
    pub const ALL: [Self; 26] = [
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
    ];

    pub fn path(self) -> SharedString {
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
            Self::Stop => IconName::CircleStop,
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
        }
        .path()
    }
}

/// An icon at 14 px. An svg does not inherit the text colour: give it one.
pub fn icon(icon: Icon) -> Svg {
    svg().path(icon.path()).size(px(14.)).flex_none()
}
