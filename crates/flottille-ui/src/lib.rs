//! Flottille's shared window components.
//!
//! Every screen of the window is built from these pieces, with the same tokens, the same states
//! and the same motion. A second, slightly different version of one of them is a defect.

pub mod color;
pub mod icons;
pub mod motion;
pub mod omarchy;
pub mod reveal;
pub mod state;
pub mod theme;
pub mod typography;

use gpui_kit::App;

pub use icons::{Icon, Icons, icon};
pub use omarchy::Environment;
pub use reveal::{Fold, PulsingLabel, enter};
pub use state::{AgentState, Pill, StateIcon, Status};
pub use theme::{Palette, palette};
pub use typography::Type;

/// Initialises gpui-kit (which reads the system's reduced-motion setting), the bundled fonts and
/// the theme of this desktop. Register [`Icons`] as the application's assets.
pub fn init(cx: &mut App) {
    init_with(Environment::from_system(), cx);
}

/// [`init`] with the desktop described by `env` rather than read from the machine.
pub fn init_with(env: Environment, cx: &mut App) {
    gpui_kit::init(cx);
    typography::register(cx);
    omarchy::install(env, cx);
}
