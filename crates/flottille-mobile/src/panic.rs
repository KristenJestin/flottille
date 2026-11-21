//! A panic of the core, kept for the app's next start (`docs/remote.md`, "The Android app"): the
//! hook writes its message to a file of the core's folder, which the app reads once and shows.

use std::path::PathBuf;

/// The file a panic's message is kept in, in the folder the app gives.
pub const PANIC_FILE: &str = "panic.txt";

/// Keeps the message of any panic of the core in `folder`, then lets the hook that was there go
/// on (Android's log). Called once, at the app's start.
#[uniffi::export]
pub fn keep_panics(folder: String) {
    let file = PathBuf::from(folder).join(PANIC_FILE);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The last panic only: the one the app shows is the one that ended it.
        let _ = std::fs::write(&file, info.to_string());
        previous(info);
    }));
}

/// The message of the panic kept in `folder`, once: the next call says none.
#[uniffi::export]
pub fn last_panic(folder: String) -> Option<String> {
    let file = PathBuf::from(folder).join(PANIC_FILE);
    let message = std::fs::read_to_string(&file).ok()?;
    let _ = std::fs::remove_file(&file);
    Some(message)
}
