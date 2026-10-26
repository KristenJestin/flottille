//! The full version: the version, the channel and, when built from a git checkout, its commit.
//! CI's builds give the version it computed and their channel (`docs/releases.md`); any other
//! build is a local one, at the crate's version.

use std::process::Command;

fn main() {
    let from = |name: &str| {
        println!("cargo:rerun-if-env-changed={name}");
        std::env::var(name).ok().filter(|value| !value.is_empty())
    };
    let version =
        from("FLOTTILLE_BUILD_VERSION").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
    let channel = from("FLOTTILLE_BUILD_CHANNEL").unwrap_or_else(|| "local".into());
    let branch = from("FLOTTILLE_BUILD_BRANCH").unwrap_or_default();
    let git = |args: &[&str]| {
        let output = Command::new("git").args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let full = match git(&["rev-parse", "--short=12", "HEAD"]) {
        Some(commit) => format!("{version} ({channel}, {commit})"),
        None => format!("{version} ({channel})"),
    };
    // A new commit moves HEAD's reflog, in a worktree as in the main checkout.
    for file in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", file]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rustc-env=FLOTTILLE_VERSION={full}");
    println!("cargo:rustc-env=FLOTTILLE_CHANNEL={channel}");
    println!("cargo:rustc-env=FLOTTILLE_BRANCH={branch}");
}
