//! What an application tells the kit about itself at [`init`](crate::init): its name, which the
//! kit says in the lines it logs and the reasons it gives, and its own look.

use gpui_kit::{App, Global};

use crate::theme::{Palette, Shade};

/// The application using the kit.
#[derive(Clone, Copy, Debug)]
pub struct Identity {
    /// Its name as a person reads it (`"Notes"`); lowercased, it starts the lines the kit logs.
    pub name: &'static str,
    /// Its own palette, in a shade: what is shown when the desktop's theme is not followed.
    pub palette: fn(Shade) -> Palette,
}

impl Identity {
    /// An application called `name`, with the kit's own look ([`Palette::own`]).
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            palette: Palette::own,
        }
    }

    /// The same application with the look it gives itself.
    pub fn palette(mut self, palette: fn(Shade) -> Palette) -> Self {
        self.palette = palette;
        self
    }
}

impl Global for Identity {}

/// The application's identity, as given to [`init`](crate::init).
pub fn identity(cx: &App) -> Identity {
    cx.try_global::<Identity>()
        .copied()
        .unwrap_or_else(|| Identity::new("app"))
}

/// A line for the standard error, in the application's name.
pub(crate) fn log(cx: &App, line: impl std::fmt::Display) {
    eprintln!("{}: {line}", identity(cx).name.to_lowercase());
}
