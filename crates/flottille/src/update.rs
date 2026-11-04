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
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::Client;
use crate::desktop;
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

/// What going down does to the store: no schema is read from a release, so this is the only guard.
const NOT_DOWN: &str =
    "the store is not migrated down, and an older build refuses a store a newer one migrated";

/// The channel last chosen with `--channel`, kept in the state folder (not the configuration: it
/// is Flottille's own memory, not a setting to edit). A plain `flottille update` follows it.
#[derive(Default, Serialize, Deserialize)]
struct Choice {
    channel: Option<String>,
    branch: Option<String>,
}

fn choice_file(paths: &Paths) -> PathBuf {
    paths.state.join("update-channel.json")
}

/// The remembered choice; none when there is no file, or one that is not ours to read.
fn remembered(paths: &Paths) -> Choice {
    std::fs::read(choice_file(paths))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn remember(paths: &Paths, choice: &Choice) -> Result<()> {
    flottille_core::paths::ensure_private_dir(&paths.state)?;
    std::fs::write(choice_file(paths), serde_json::to_vec(choice)?)?;
    Ok(())
}

pub fn run(
    paths: &Paths,
    channel: Option<String>,
    branch: Option<String>,
    version: Option<String>,
    force: bool,
) -> Result<ExitCode> {
    if branch.is_some() && channel.as_deref() != Some("alpha") {
        return Err("--branch names an alpha: give it with --channel alpha".into());
    }
    let wanted = version.as_deref().map(exact).transpose()?;
    // A version says nothing of the channel: it is neither read from nor written to the memory.
    let chosen = channel.is_some();
    let kept = remembered(paths);
    let channel = channel
        .or(kept.channel.clone())
        .unwrap_or_else(|| match CHANNEL {
            "beta" | "alpha" => CHANNEL.to_owned(),
            _ => "stable".to_owned(),
        });
    let branch = branch.or_else(|| {
        kept.branch
            .filter(|_| kept.channel.as_deref() == Some(&channel))
    });
    if Client::connect(paths)?.is_some() {
        return Err(format!(
            "the daemon is running, and would go on from a binary that is gone: stop it first \
             with `{} daemon stop` (its agents stop with it), then update.",
            program::name()
        )
        .into());
    }
    let url = match &wanted {
        Some(version) => exact_url(version)?,
        None => archive_url(&channel, branch.as_deref())?,
    };
    let name = url.rsplit('/').next().unwrap_or_default().to_owned();
    let version = version_of(&name);
    // What the user asked for, to say it right when it is refused.
    let which = match &wanted {
        Some(_) => "",
        None => &channel,
    };
    if let (Some(new), Ok(this)) = (&version, Version::parse(VERSION)) {
        if *new == this && !force {
            println!("Flottille is already {this}: nothing to update (--force installs it again).");
            // The binary may have moved since the entry was written.
            let followed = std::env::current_exe()
                .and_then(|binary| binary.canonicalize())
                .and_then(|binary| desktop::follow(paths, &binary));
            if let Err(error) = followed {
                eprintln!("flottille: the launcher entry was not updated: {error}");
            }
            if chosen {
                remember_choice(paths, &channel, branch.as_deref());
            }
            return Ok(ExitCode::SUCCESS);
        }
        if *new < this {
            if !force {
                return Err(if which.is_empty() {
                    format!("{new} is older than this {this}: --force goes back to it ({NOT_DOWN}).")
                } else {
                    format!(
                        "{new} is the latest of {which}, but older than this {this}: --force goes to it ({NOT_DOWN})."
                    )
                }
                .into());
            }
            println!("{new} is older than this {this}: {NOT_DOWN}.");
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
    let version = replace(paths, &binary, &name, &archive)?;
    println!("{} is now {version}.", binary.display());
    if chosen {
        remember_choice(paths, &channel, branch.as_deref());
    }
    Ok(ExitCode::SUCCESS)
}

/// The binary is in place: a memory that cannot be written is said, not undone.
fn remember_choice(paths: &Paths, channel: &str, branch: Option<&str>) {
    let choice = Choice {
        channel: Some(channel.to_owned()),
        branch: branch.filter(|_| channel == "alpha").map(str::to_owned),
    };
    if let Err(error) = remember(paths, &choice) {
        eprintln!("flottille: the channel was not remembered: {error}");
    }
}

/// `text` as the exact version of a release: a semantic version, nothing else (it is written
/// into an address).
fn exact(text: &str) -> Result<Version> {
    match Version::parse(text) {
        Ok(version) if version.build.is_empty() && version.to_string() == text => Ok(version),
        _ => Err(format!("{text} is not a version (X.Y.Z, or X.Y.Z-pre)").into()),
    }
}

/// The tag a version is published under: an alpha's is its branch's, replaced in place
/// (`0.5.0-alpha.feat-x.7` is in `alpha-feat-x`), any other's is `v` and the version.
fn tag_of(version: &Version) -> String {
    version
        .pre
        .as_str()
        .strip_prefix("alpha.")
        .and_then(|rest| rest.rsplit_once('.'))
        .filter(|(branch, _)| !branch.is_empty())
        .map_or_else(
            || format!("v{version}"),
            |(branch, _)| format!("alpha-{branch}"),
        )
}

/// The archive of exactly this version; when there is none, the error names the versions
/// published nearest to it.
fn exact_url(version: &Version) -> Result<String> {
    let name = format!("flottille-{version}-x86_64-linux.tar.gz");
    let tagged = json(&format!("{}/releases/tags/{}", api(), tag_of(version)))?;
    let found = tagged
        .iter()
        .flat_map(archives)
        .find(|url| url.ends_with(&format!("/{name}")));
    if let Some(url) = found {
        return Ok(url);
    }
    let mut known: Vec<Version> = json(&format!("{}/releases?per_page=100", api()))?
        .unwrap_or_default()
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(archives)
        .filter_map(|url| version_of(url.rsplit('/').next()?))
        .collect();
    known.sort();
    known.dedup();
    let at = known.partition_point(|v| v < version);
    let close: Vec<String> = known
        .iter()
        .skip(at.saturating_sub(2))
        .take(5)
        .map(Version::to_string)
        .collect();
    let mut said = format!("there is no release {version} for Linux x86_64");
    if !close.is_empty() {
        said.push_str(&format!(" (close versions: {})", close.join(", ")));
    }
    Err(said.into())
}

/// The archive of the release published last among these (ties go to the higher version): a
/// channel's latest is the newest build, not the highest number, which a numbering change can
/// leave behind (#413). The dates are UTC and all of one shape, so they compare as text.
fn latest_published(releases: &[Value]) -> Option<String> {
    releases
        .iter()
        .flat_map(|release| {
            let at = release["published_at"].as_str().unwrap_or_default();
            archives(release)
                .into_iter()
                .filter_map(move |url| Some(((at, version_of(url.rsplit('/').next()?)?), url)))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, url)| url)
}

/// The archive of the highest version among these addresses.
fn highest(urls: Vec<String>) -> Option<String> {
    urls.into_iter()
        .filter_map(|url| Some((version_of(url.rsplit('/').next()?)?, url)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, url)| url)
}

/// The version an archive is of, `0.3.0-beta.12` in `flottille-0.3.0-beta.12-x86_64-linux.tar.gz`.
fn version_of(name: &str) -> Option<Version> {
    let version = name
        .strip_prefix("flottille-")?
        .strip_suffix("-x86_64-linux.tar.gz")?;
    Version::parse(version).ok()
}

/// The addresses of the archives for this machine that a release holds.
fn archives(release: &Value) -> Vec<String> {
    release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["browser_download_url"].as_str())
        .filter(|url| url.ends_with("-x86_64-linux.tar.gz"))
        .map(str::to_owned)
        .collect()
}

/// The address of the channel's latest archive for this machine.
fn archive_url(channel: &str, branch: Option<&str>) -> Result<String> {
    let found = match channel {
        "stable" => {
            let release = json(&format!("{}/releases/latest", api()))?
                .ok_or("there is no stable release yet")?;
            archives(&release).into_iter().next()
        }
        "beta" => latest_published(
            &json(&format!("{}/releases?per_page=100", api()))?
                .unwrap_or_default()
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["tag_name"].as_str().is_some_and(|t| t.contains("-beta.")))
                .cloned()
                .collect::<Vec<_>>(),
        ),
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
            // Two while it is replaced: the newer.
            highest(archives(&release))
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
/// the old one or the new one, never half of one, and nothing is left beside it. The launcher's
/// files of the archive are written too. The version the archive is of.
fn replace(paths: &Paths, binary: &Path, name: &str, archive: &[u8]) -> Result<String> {
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
        // The binary is replaced: failing to write the launcher's files is said, not undone.
        if let Err(error) = desktop::write(paths, &work.join(folder).join("share"), binary) {
            eprintln!("flottille: the launcher entry was not updated: {error}");
        }
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
