//! `flottille update` (`docs/releases.md`): replaces this binary with the latest build of a
//! channel, its SHA-256 sum checked first. Never while agents run: their processes are this
//! binary too, and the daemon that runs them would be left on a file that is gone.
//!
//! The releases are read from GitHub's API with the user's own `curl` and unpacked with their
//! own `tar`, both spawned without a shell, as git is.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use flottille_core::paths::Paths;
use serde_json::Value;

use crate::client::Client;
use crate::files::sha256;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Where the releases are read: GitHub's API for the repository, or, for tests, what
/// `FLOTTILLE_RELEASES` names.
fn api() -> String {
    std::env::var("FLOTTILLE_RELEASES")
        .unwrap_or_else(|_| "https://api.github.com/repos/KristenJestin/flottille".into())
}

/// The channel this binary was built on (`build.rs`); a local build updates from stable.
pub const CHANNEL: &str = env!("FLOTTILLE_CHANNEL");
const BRANCH: &str = env!("FLOTTILLE_BRANCH");

pub fn run(paths: &Paths, channel: Option<String>, branch: Option<String>) -> Result<ExitCode> {
    let channel = channel.unwrap_or_else(|| match CHANNEL {
        "beta" | "alpha" => CHANNEL.to_owned(),
        _ => "stable".to_owned(),
    });
    refuse_while_agents_run(paths)?;
    let url = archive_url(&channel, branch.as_deref())?;
    let name = url.rsplit('/').next().unwrap_or_default().to_owned();
    let archive = fetch(&url)?;
    let published = String::from_utf8(fetch(&format!("{url}.sha256"))?)?;
    let sum = published.split_whitespace().next().unwrap_or_default();
    if sum != sha256(&archive) {
        return Err(format!(
            "the SHA-256 sum of {name} is not the one published: nothing was replaced"
        )
        .into());
    }
    let binary = std::env::current_exe()?.canonicalize()?;
    let version = replace(&binary, &name, &archive)?;
    println!("{} is now {version}.", binary.display());
    if Client::connect(paths)?.is_some() {
        println!(
            "The daemon still runs the version before: `flottille daemon stop` stops it, and the \
             next command starts the new one."
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Refuses while an agent starts or works, naming the roots to stop. Without a daemon, none runs.
fn refuse_while_agents_run(paths: &Paths) -> Result<()> {
    let Some(mut client) = Client::connect(paths)? else {
        return Ok(());
    };
    let listed = client.request("agent.list", Value::Null)?;
    let agents = listed["agents"].as_array().cloned().unwrap_or_default();
    let running: Vec<&Value> = agents
        .iter()
        .filter(|a| matches!(a["state"].as_str(), Some("starting" | "working")))
        .collect();
    if running.is_empty() {
        return Ok(());
    }
    // A child stops with its root: the roots are what the user stops.
    let root_of = |agent: &Value| {
        let mut agent = agent.clone();
        while let Some(parent) = agent["parent"].as_str() {
            match agents.iter().find(|a| a["id"] == parent) {
                Some(found) => agent = found.clone(),
                None => break,
            }
        }
        agent["id"].as_str().unwrap_or_default().to_owned()
    };
    let mut roots: Vec<String> = running.iter().map(|a| root_of(a)).collect();
    roots.sort();
    roots.dedup();
    let count = match running.len() {
        1 => "1 agent is running".to_owned(),
        n => format!("{n} agents are running"),
    };
    let stops: Vec<String> = roots
        .iter()
        .map(|root| format!("`flottille agent stop {root}`"))
        .collect();
    Err(format!(
        "{count}: Flottille is not replaced under them. Stop them first ({}), then update.",
        stops.join(", ")
    )
    .into())
}

/// The address of the channel's latest archive for this machine.
fn archive_url(channel: &str, branch: Option<&str>) -> Result<String> {
    let is_archive = |url: &&str| url.ends_with("-x86_64-linux.tar.gz");
    let urls = |release: &Value| -> Vec<String> {
        release["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["browser_download_url"].as_str())
            .filter(is_archive)
            .map(str::to_owned)
            .collect()
    };
    let found = match channel {
        "stable" => urls(&json(&format!("{}/releases/latest", api()))?)
            .into_iter()
            .next(),
        // The releases come newest first: the first beta is the latest.
        "beta" => json(&format!("{}/releases?per_page=100", api()))?
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["tag_name"].as_str().is_some_and(|t| t.contains("-beta.")))
            .flat_map(&urls)
            .next(),
        "alpha" => {
            let branch = branch
                .or(Some(BRANCH).filter(|b| !b.is_empty()))
                .ok_or("the alpha channel needs --branch <name>")?;
            let tag: String = branch
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            let release = json(&format!("{}/releases/tags/alpha-{tag}", api()))
                .map_err(|_| format!("there is no alpha release for {branch}"))?;
            urls(&release).into_iter().next()
        }
        _ => return Err("the channel is stable, beta or alpha".into()),
    };
    found.ok_or_else(|| format!("there is no {channel} release for Linux x86_64").into())
}

fn json(url: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&fetch(url)?)?)
}

/// The body at `url`, through the user's `curl`.
fn fetch(url: &str) -> Result<Vec<u8>> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--",
            url,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(output.stdout)
}

/// Unpacks the archive beside the binary and renames its `flottille` over it: the binary is
/// the old one or the new one, never half of one, and nothing is left beside it. The version
/// the archive is of.
fn replace(binary: &Path, name: &str, archive: &[u8]) -> Result<String> {
    let dir = binary.parent().ok_or("the binary has no folder")?;
    let work = tempfile_dir(dir)?;
    let unpacked = (|| -> Result<String> {
        let file = work.join(name);
        std::fs::write(&file, archive)?;
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(&file)
            .arg("-C")
            .arg(&work)
            .stdin(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(format!("cannot unpack {name}").into());
        }
        let folder = name.trim_end_matches("-x86_64-linux.tar.gz");
        std::fs::rename(work.join(folder).join("flottille"), binary)?;
        Ok(folder.trim_start_matches("flottille-").to_owned())
    })();
    let _ = std::fs::remove_dir_all(&work);
    unpacked
}

/// A folder of its own beside the binary, so that the rename stays on one file system.
fn tempfile_dir(dir: &Path) -> Result<PathBuf> {
    for n in 0..100 {
        let path = dir.join(format!(".flottille-update-{}-{n}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err("cannot make a folder beside the binary".into())
}
