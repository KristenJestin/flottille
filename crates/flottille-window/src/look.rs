//! The window's own copy of the theme the user chose, so that its first frame is drawn in it.
//!
//! The choice is the daemon's (`appearance.show`), and the window cannot ask before it draws: until
//! the daemon answered, the window drew in the kit's default and then switched. The copy is a small
//! file in the window's state folder, in the words of `appearance.show`, written each time the
//! user chooses and each time the daemon says what it keeps; the daemon stays the owner, and its
//! answer only confirms the copy, which a choice made from another client changes.

use std::path::{Path, PathBuf};

use gpui_design_system::Appearance;
use gpui_design_system::appearance;
use gpui_kit::{App, Global};

use crate::actions::{appearance_json, appearance_of};

/// Where the copy is kept, and what it last held (a global of the window, once it is started).
pub struct Memory {
    path: PathBuf,
    held: Option<Appearance>,
}

impl Global for Memory {}

impl Memory {
    /// The copy kept in the state folder `state` (`$XDG_STATE_HOME/flottille`).
    pub fn in_folder(state: &Path) -> Memory {
        Memory {
            path: state.join("window-appearance.json"),
            held: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the copy holds: nothing when there is none yet, or when it cannot be read (the window
    /// then draws in the kit's default, as before the copy existed).
    pub fn read(&self) -> Option<Appearance> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        appearance_of(&serde_json::from_str(&text).ok()?).ok()
    }

    /// Replaces the copy: written beside it and renamed, so that a window opening meanwhile never
    /// reads half of it.
    fn write(&self, appearance: &Appearance) -> std::io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let beside = self.path.with_extension("json.tmp");
        std::fs::write(&beside, appearance_json(appearance.clone()).to_string())?;
        std::fs::rename(beside, &self.path)
    }
}

/// Starts from the copy `memory` holds: the kit shows it from its first frame, with no cross-fade
/// from its default, and the window keeps `memory` to write it again when the choice changes.
pub fn begin(mut memory: Memory, cx: &mut App) {
    if let Some(kept) = memory.read() {
        // A fade from the default to the copy would be drawn from the first frame on: the copy
        // is applied as reduced motion applies a change, at once.
        let reduce = cx.reduce_motion();
        cx.set_reduce_motion(true);
        appearance::appear(kept.clone(), cx);
        cx.set_reduce_motion(reduce);
        memory.held = Some(kept);
    }
    cx.set_global(memory);
}

/// Keeps `chosen` as the copy, unless it already is: the choice the user made, or the one the
/// daemon says it keeps.
pub fn remember(chosen: &Appearance, cx: &mut App) {
    let Some(memory) = cx.try_global::<Memory>() else {
        return;
    };
    if memory.held.as_ref() == Some(chosen) {
        return;
    }
    if let Err(error) = memory.write(chosen) {
        // Not fatal: the next window draws its first frame in the kit's default, as it used to.
        eprintln!(
            "flottille: the window's copy of its theme cannot be written ({}): {error}",
            memory.path.display()
        );
        return;
    }
    cx.global_mut::<Memory>().held = Some(chosen.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_design_system::Source;
    use gpui_design_system::theme::Shade;

    /// A new empty folder of the machine's temporary one, never the user's.
    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("flottille-look-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn light() -> Appearance {
        Appearance {
            source: Source::Theme(crate::OWN_THEME.into()),
            shade: Shade::Light,
        }
    }

    #[test]
    fn what_is_written_is_read_back_and_a_folder_that_is_missing_is_made() {
        let folder = scratch("round-trip");
        let memory = Memory::in_folder(&folder.join("state/flottille"));
        assert_eq!(memory.read(), None, "nothing yet");
        memory.write(&light()).unwrap();
        assert_eq!(memory.read(), Some(light()));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_copy_that_cannot_be_read_is_no_copy() {
        let folder = scratch("unreadable");
        let memory = Memory::in_folder(&folder);
        std::fs::write(memory.path(), "{\"theme\": ").unwrap();
        assert_eq!(memory.read(), None);
        std::fs::write(
            memory.path(),
            "{\"theme\": \"nowhere\", \"shade\": \"dark\"}",
        )
        .unwrap();
        assert_eq!(memory.read(), None);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
