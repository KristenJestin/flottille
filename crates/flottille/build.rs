//! The full version: the crate's version and, when built from a git checkout, its commit.

use std::process::Command;

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    let git = |args: &[&str]| {
        let output = Command::new("git").args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let full = match git(&["rev-parse", "--short=12", "HEAD"]) {
        Some(commit) => format!("{version} ({commit})"),
        None => version.to_owned(),
    };
    // A new commit moves HEAD's reflog, in a worktree as in the main checkout.
    for file in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", file]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rustc-env=FLOTTILLE_VERSION={full}");
}
