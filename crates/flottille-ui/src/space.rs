//! The spacing scale of the window, and the widths a page and a dialog take.
//!
//! A gap or a padding of a screen is one of these steps: a scan of the window's sources refuses
//! any other (`crates/flottille-window/tests/the_rule.rs`). The components of this crate keep the
//! exact sizes of the design, which are theirs to hold.

use gpui_kit::{Pixels, px};

pub const XS: Pixels = px(4.);
pub const SM: Pixels = px(8.);
pub const MD: Pixels = px(12.);
pub const LG: Pixels = px(16.);
pub const XL: Pixels = px(24.);
/// The page's padding and the gap between its sections.
pub const XXL: Pixels = px(32.);
pub const XXXL: Pixels = px(48.);

/// The widest a page's column gets.
pub const COLUMN: Pixels = px(880.);
/// A dialog's width.
pub const DIALOG: Pixels = px(520.);
/// A dialog with a wide form.
pub const DIALOG_WIDE: Pixels = px(640.);
