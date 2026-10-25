//! One stroked icon family (Lucide), only the icons the window uses embedded.

use gpui_kit::assets::IconName;
use gpui_kit::{SharedString, Styled, Svg, px, svg};

gpui_kit::assets::icon_assets!(pub Icons, [Folder, GitBranch, Book, ChevronDown, Plus, Search, Bell, ExternalLink, Network, Menu]);

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
}

impl Icon {
    pub const ALL: [Self; 10] = [
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
        }
        .path()
    }
}

/// An icon at 14 px. An svg does not inherit the text colour: give it one.
pub fn icon(icon: Icon) -> Svg {
    svg().path(icon.path()).size(px(14.)).flex_none()
}
