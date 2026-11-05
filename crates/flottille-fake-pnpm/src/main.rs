//! A launcher that does what pnpm's does to its subcommands, for the tests of how Flottille ends
//! and waits for what its runs leave (`docs/tools.md`, "Running"): it starts `sh -c <line>` in a
//! process group of its own, writes that shell's pid to a file, and exits without waiting for it.
//!
//! ```text
//! flottille-fake-pnpm <pid file> <line>
//! ```

use std::os::unix::process::CommandExt;
use std::process::Command;

#[expect(
    clippy::zombie_processes,
    reason = "the shell is left to whoever takes it in, as pnpm leaves it"
)]
fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(file), Some(line)) = (args.next(), args.next()) else {
        panic!("usage: flottille-fake-pnpm <pid file> <line>");
    };
    let child = Command::new("sh")
        .args(["-c", &line])
        .process_group(0)
        .spawn()
        .expect("sh starts");
    // Written whole before it is read: the test reads it once the run ended.
    let partial = format!("{file}.partial");
    std::fs::write(&partial, child.id().to_string()).unwrap();
    std::fs::rename(&partial, &file).unwrap();
}
