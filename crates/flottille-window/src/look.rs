//! The window's own copy of the theme the user chose, so that its first frame is drawn in it.

use std::path::{Path, PathBuf};

/// Where the copy is kept.
pub struct Memory {
    path: PathBuf,
}

impl Memory {
    /// The copy kept in the state folder `state` (`$XDG_STATE_HOME/flottille`).
    pub fn in_folder(state: &Path) -> Memory {
        Memory {
            path: state.join("window-appearance.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
