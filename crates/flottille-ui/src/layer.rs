//! The order in which what floats over the page is drawn.
//!
//! GPUI Fast draws every deferred layer in one list sorted by priority, nested ones included: a
//! layer opened from inside a dialog is drawn with the others, not over its parent. A priority is
//! therefore a place in the whole window's stack, and these are the only ones in use.

/// A modal dialog, with its scrim.
pub const MODAL: usize = 2;
/// The list of a select, a dropdown or a "⋯" menu: over the dialog it may be opened from.
pub const LIST: usize = 3;
/// A toast, over everything.
pub const TOAST: usize = 4;
