//! The shared window components of an application: tokens, rows, cards, menus, dialogs, toasts and
//! the motion they share.
//!
//! Every screen is built from these pieces, with the same tokens, the same states and the same
//! motion. A second, slightly different version of one of them is a defect. The kit knows nothing
//! of what an application is about: the application tells it its name and its look at [`init`].

pub mod button;
pub mod card;
pub mod choice_row;
pub mod color;
pub mod confirm;
pub mod cut;
pub mod drawer;
pub mod dropdown;
pub mod field;
pub mod fuzzy;
pub mod icons;
pub mod identity;
mod interact;
pub mod layer;
pub mod lines;
pub mod listing;
pub mod menu;
pub mod modal;
pub mod model_choice;
pub mod motion;
pub mod omarchy;
pub mod page;
pub mod path;
pub mod radio;
pub mod reveal;
pub mod rows;
pub mod segmented;
pub mod select;
pub mod skeleton;
pub mod space;
pub mod subpage;
pub mod switch;
pub mod tabs;
pub mod theme;
pub mod toast;
pub mod typography;

use gpui_kit::App;

pub use button::{Badge, Button, ButtonKind, Tooltip, tipped};
pub use card::{Block, Card};
pub use choice_row::ChoiceRow;
pub use confirm::Confirm;
pub use cut::{QUOTED_CHARS, quoted, quoted_title, tipped_when_cut};
pub use drawer::{Drawer, Side};
pub use dropdown::{Align, Dropdown, MenuItem};
pub use field::{Field, StyledField, field_style, follow_theme};
pub use icons::{Icon, Icons, icon};
pub use identity::{Identity, identity};
pub use interact::{
    Hoverable, dismiss_on_navigate, dismiss_top_overlay, fade, forget_overlays, hoverable,
    keys_held, open_overlay, pressable, selectable, text_region, tint,
};
pub use lines::{
    CardHead, Chip, FlatRow, FooterAction, LINE_GAP, LabelledLine, caption, lines_block,
};
pub use listing::{Item, Listing, Placement};
pub use menu::Menu;
pub use modal::Modal;
pub use model_choice::{Chosen, ModelChoice, ModelEntry};
pub use omarchy::Environment;
pub use page::{EmptyLine, PageHeader, Section};
pub use path::path_line;
pub use radio::RadioChoices;
pub use reveal::{
    Changing, Fold, Growing, Presence, PresenceOf, PulsingLabel, changed, changing, enter,
    fold_frame, growing, presence, presence_of,
};
pub use rows::{
    ListCard, ListRow, Tie, TreeRow, meta, section_label, tie_below_id, tie_id, tie_through_id,
    tie_turn_id, toggle_id, tree_children, tree_tie, turning,
};
pub use segmented::Segmented;
pub use select::{Select, SelectOption};
pub use skeleton::{Awaiting, Skeleton, awaiting};
pub use subpage::Subpage;
pub use switch::Switch;
pub use tabs::Tabs;
pub use theme::{Palette, palette};
pub use toast::{Toast, ToastKind, ToastStack, toast};
pub use typography::Type;

/// Initialises gpui-kit (which reads the system's reduced-motion setting), the bundled fonts and
/// the theme of this desktop, for the application `identity`. Register [`Icons`], or assets that
/// serve it, as the application's assets.
pub fn init(identity: Identity, cx: &mut App) {
    init_with(identity, Environment::from_system(), cx);
}

/// [`init`] with the desktop described by `env` rather than read from the machine.
pub fn init_with(identity: Identity, env: Environment, cx: &mut App) {
    gpui_kit::init(cx);
    cx.set_global(identity);
    typography::register(cx);
    omarchy::install(env, cx);
}
