//! Opening a folder in the user's editor, or a link, with the system's default handler (`docs/design.md`
//! §10): spawned without a shell, the path as one argument. The tests give their own opener and
//! look at what it was given.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

pub type Opener = Arc<dyn Fn(&Path) + Send + Sync>;

/// The system's default handler.
pub fn system() -> Opener {
    Arc::new(|path: &Path| {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let child = Command::new(program)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            // Waited for off the UI thread, so that it leaves no zombie behind.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(error) => eprintln!("flottille: {program} cannot start: {error}"),
        }
    })
}
