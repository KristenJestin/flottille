//! Flottille's shared window components.
//!
//! Every screen of the window is built from these pieces, with the same tokens, the same states
//! and the same motion. A second, slightly different version of one of them is a defect.

pub mod icons;
pub mod motion;
pub mod typography;

use gpui_kit::App;

pub use icons::{Icon, Icons, icon};
pub use typography::Type;

/// Initialises gpui-kit (which reads the system's reduced-motion setting) and the bundled fonts.
/// Register [`Icons`] as the application's assets.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    typography::register(cx);
}
