//! `flottille update` (`docs/releases.md`): replaces this binary with the latest build of a
//! channel, its SHA-256 sum checked first. Never while the daemon runs: it would go on from a
//! file that is gone (`/proc/self/exe` then names `… (deleted)`), and its next start of an
//! agent's process, which is this binary too, would fail.
//!
//! The releases are read from GitHub's API with the user's own `curl` and unpacked with their
//! own `tar`, both spawned without a shell, as git is.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use flottille_core::paths::Paths;
use semver::Version;
use serde_json::Value;

use crate::client::Client;
use crate::files::sha256;
use crate::program;

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
/// This build's version alone, `0.3.0-beta.12`.
const VERSION: &str = env!("FLOTTILLE_VERSION_NUMBER");

pub fn run(
    paths: &Paths,
    channel: Option<String>,
    branch: Option<String>,
    force: bool,
) -> Result<ExitCode> {
    if branch.is_some() && channel.as_deref() != Some("alpha") {
        return Err("--branch names an alpha: give it with --channel alpha".into());
    }
    let channel = channel.unwrap_or_else(|| match CHANNEL {
        "beta" | "alpha" => CHANNEL.to_owned(),
        _ => "stable".to_owned(),
    });
    if Client::connect(paths)?.is_some() {
        return Err(format!(
            "the daemon is running, and would go on from a binary that is gone: stop it first \
             with `{} daemon stop` (its agents stop with it), then update.",
            program::name()
        )
        .into());
    }
    let url = archive_url(&channel, branch.as_deref())?;
    let name = url.rsplit('/').next().unwrap_or_default().to_owned();
    let version = version_of(&name);
    if let (Some(new), Ok(this)) = (&version, Version::parse(VERSION))
        && !force
    {
        if *new == this {
            println!("Flottille is already {this}: nothing to update (--force installs it again).");
            return Ok(ExitCode::SUCCESS);
        }
        if *new < this {
            return Err(
                format!("{new} is older than this {this}: --force goes back to it.").into(),
            );
        }
    }
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
    Ok(ExitCode::SUCCESS)
}

/// The version an archive is of, `0.3.0-beta.12` in `flottille-0.3.0-beta.12-x86_64-linux.tar.gz`.
fn version_of(name: &str) -> Option<Version> {
    let version = name
        .strip_prefix("flottille-")?
        .strip_suffix("-x86_64-linux.tar.gz")?;
    Version::parse(version).ok()
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
        "stable" => {
            let release = json(&format!("{}/releases/latest", api()))?
                .ok_or("there is no stable release yet")?;
            urls(&release).into_iter().next()
        }
        // The newest by version, whatever order they were published in.
        "beta" => json(&format!("{}/releases?per_page=100", api()))?
            .unwrap_or_default()
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["tag_name"].as_str().is_some_and(|t| t.contains("-beta.")))
            .flat_map(&urls)
            .filter_map(|url| Some((version_of(url.rsplit('/').next()?)?, url)))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, url)| url),
        "alpha" => {
            let branch = branch
                .or(Some(BRANCH).filter(|b| !b.is_empty()))
                .ok_or("the alpha channel needs --branch <name>")?;
            // Each byte but ASCII letters, digits and dashes is a dash, as CI names the alpha.
            let tag: String = branch
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b == b'-' {
                        b as char
                    } else {
                        '-'
                    }
                })
                .collect();
            let release = json(&format!("{}/releases/tags/alpha-{tag}", api()))?
                .ok_or_else(|| format!("there is no alpha release for {branch}"))?;
            urls(&release).into_iter().next()
        }
        _ => return Err("the channel is stable, beta or alpha".into()),
    };
    found.ok_or_else(|| format!("there is no {channel} release for Linux x86_64").into())
}

/// The API's answer at `url`; none when it is not there (404), which is not the failure of the
/// network or of GitHub.
fn json(url: &str) -> Result<Option<Value>> {
    // The status follows the body, on a line of its own.
    let body = curl(url, &["--write-out", "\n%{http_code}"])?;
    let text = String::from_utf8(body)?;
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    match code {
        "200" => Ok(Some(serde_json::from_str(body)?)),
        "404" => Ok(None),
        code => Err(format!("cannot read {url}: GitHub answered {code}").into()),
    }
}

/// The file at `url`.
fn fetch(url: &str) -> Result<Vec<u8>> {
    curl(url, &["--fail"])
}

/// What the user's `curl` reads at `url`.
fn curl(url: &str, options: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("curl")
        .args(options)
        .args(["--silent", "--show-error", "--location", "--", url])
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
