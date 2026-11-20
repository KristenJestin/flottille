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
pub const VERSION: &str = env!("FLOTTILLE_VERSION_NUMBER");

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
    // Written beside the file and renamed over it, so that the file is the old choice or the new
    // one, never half of one. The process's id keeps two updates from writing the same file.
    let file = choice_file(paths);
    let temp = file.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temp, serde_json::to_vec(choice)?)?;
    std::fs::rename(&temp, &file)?;
    Ok(())
}

/// The channel a plain update follows, and its branch: `channel` and `branch` when given, else
/// the channel last chosen (and its branch), else the build's own (a `local` build takes stable).
fn followed(
    kept: &Choice,
    channel: Option<String>,
    branch: Option<String>,
) -> (String, Option<String>) {
    let channel = channel
        .or(kept.channel.clone())
        .unwrap_or_else(|| match CHANNEL {
            "beta" | "alpha" | "rc" | "next" => CHANNEL.to_owned(),
            _ => "stable".to_owned(),
        });
    let branch = branch.or_else(|| {
        kept.branch
            .clone()
            .filter(|_| kept.channel.as_deref() == Some(&channel))
    });
    (channel, branch)
}

/// The channel and the branch a plain update follows now.
pub fn followed_now(paths: &Paths) -> (String, Option<String>) {
    followed(&remembered(paths), None, None)
}

pub fn run(
    paths: &Paths,
    channel: Option<String>,
    branch: Option<String>,
    version: Option<String>,
    force: bool,
    stage: bool,
) -> Result<ExitCode> {
    if branch.is_some() && channel.as_deref() != Some("alpha") {
        return Err("--branch names an alpha: give it with --channel alpha".into());
    }
    let wanted = version.as_deref().map(exact).transpose()?;
    // A version says nothing of the channel: it is neither read from nor written to the memory.
    let chosen = channel.is_some();
    let kept = remembered(paths);
    let (channel, branch) = followed(&kept, channel, branch);
    if !chosen && wanted.is_none() && kept.channel.is_some() {
        match (channel.as_str(), branch.as_deref()) {
            ("alpha", Some(branch)) => println!(
                "Following the channel last chosen: alpha ({branch}); --channel beta goes back to dev's builds."
            ),
            _ => println!("Following the channel last chosen: {channel}."),
        }
    }
    // Staging touches nothing that runs: the daemon may run on.
    if !stage && Client::connect(paths)?.is_some() {
        return Err(format!(
            "Flottille is running, and would go on from a binary that is gone: stop it first \
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
        if stage && (*new == this || is_older(new, &this)) {
            // The daemon applies a newer build only (`apply_staged`).
            println!("{new} is not newer than this {this}: nothing to stage.");
            return Ok(ExitCode::SUCCESS);
        }
        if *new == this && !force {
            println!("Flottille is already {this}: nothing to update (--force installs it again).");
            // The binary may have moved since the entry was written.
            let binary = std::env::current_exe().and_then(|binary| binary.canonicalize());
            if let Ok(binary) = &binary {
                forget_staged(binary);
            }
            if let Err(error) = binary.and_then(|binary| desktop::follow(paths, &binary)) {
                eprintln!("flottille: the launcher entry was not updated: {error}");
            }
            if chosen {
                remember_choice(paths, &channel, branch.as_deref());
            }
            return Ok(ExitCode::SUCCESS);
        }
        if is_older(new, &this) {
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
    if stage {
        let binary = binary()?;
        let staged = stage_from(&binary, &url, &|_, _| {})?;
        println!(
            "{staged} is staged beside {}: applied at the next start of the daemon.",
            binary.display()
        );
        if chosen {
            remember_choice(paths, &channel, branch.as_deref());
        }
        return Ok(ExitCode::SUCCESS);
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
    // What was staged is older than what is installed now, or the same.
    forget_staged(&binary);
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

/// The tag a version is published under: `v` and the version, whatever its channel (each alpha is
/// a release of its own, as a beta is).
fn tag_of(version: &Version) -> String {
    format!("v{version}")
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
/// leave behind (#413). The dates are UTC and all of one shape, so they compare as text. A draft
/// has no date and is not published: it is not offered.
fn latest_published(releases: &[Value]) -> Option<String> {
    releases
        .iter()
        .filter_map(|release| Some((release["published_at"].as_str()?, release)))
        .flat_map(|(at, release)| {
            archives(release)
                .into_iter()
                .filter_map(move |url| Some(((at, version_of(url.rsplit('/').next()?)?), url)))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, url)| url)
}

/// The archive of the highest version among the alphas of the branch whose slug this is, in
/// these releases: `*-alpha.<slug>.<n>`, the slug whole (`feat-a` is not `feat-a-b`), each alpha
/// being a release of its own. A draft has no date and is not published: it is not offered.
fn highest_alpha(releases: &[Value], slug: &str) -> Option<String> {
    releases
        .iter()
        .filter(|release| release["published_at"].as_str().is_some())
        .flat_map(archives)
        .filter_map(|url| Some((version_of(url.rsplit('/').next()?)?, url)))
        .filter(|(version, _)| {
            version
                .pre
                .as_str()
                .strip_prefix("alpha.")
                .and_then(|rest| rest.rsplit_once('.'))
                .is_some_and(|(of, n)| of == slug && n.bytes().all(|b| b.is_ascii_digit()))
        })
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

/// The archive an rc or a next build follows: its own line's (`docs/releases.md`, "Updating").
/// An rc build of `X.Y.Z` takes the highest of the line's `-rc.` builds and stable releases (a
/// line's stable is above its rcs); a next build of `X.Y.0` the most recently published of the
/// line's `-next.` builds and, once the line is merged into dev, of dev's betas from `X.Y.0` on.
/// Other lines' builds, whatever they are, are never taken.
fn along_line(releases: &[Value], this: &Version) -> Option<String> {
    let rc = this.pre.as_str().starts_with("rc.");
    let candidates = releases
        .iter()
        .filter(|release| release["published_at"].as_str().is_some())
        .flat_map(|release| {
            let at = release["published_at"].as_str().unwrap_or_default();
            archives(release)
                .into_iter()
                .filter_map(move |url| Some((at, version_of(url.rsplit('/').next()?)?, url)))
        })
        .filter(|(_, version, _)| {
            let pre = version.pre.as_str();
            let line = (version.major, version.minor) == (this.major, this.minor);
            if rc {
                line && (pre.is_empty() || pre.starts_with("rc."))
            } else {
                // Next is above dev (`docs/releases.md`, "The branches"): a beta from this line's
                // `X.Y.0` on is dev with the line merged in, the next build's own successor.
                (line && pre.starts_with("next."))
                    || (pre.starts_with("beta.")
                        && (version.major, version.minor, version.patch)
                            >= (this.major, this.minor, this.patch))
            }
        });
    if rc {
        candidates
            .max_by(|a, b| a.1.cmp(&b.1))
            .map(|(_, _, url)| url)
    } else {
        candidates
            .max_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)))
            .map(|(_, _, url)| url)
    }
}

/// The pages of releases read at most: dev's betas fill a page within weeks, a line's rc sits
/// some pages down, and a build whose own release is gone must not read the whole history.
const MAX_PAGES: u32 = 10;

/// `along_line` over the pages of the releases, newest first, as GitHub gives them, a page at a
/// time through `page` (1 is the first). The reading stops at the page that holds this build's own
/// release (what is newer is above it), at an empty page, or after `MAX_PAGES`.
fn along_line_paged(
    this: &Version,
    mut page: impl FnMut(u32) -> Result<Vec<Value>>,
) -> Result<Option<String>> {
    let mut releases = Vec::new();
    for n in 1..=MAX_PAGES {
        let next = page(n)?;
        if next.is_empty() {
            break;
        }
        let own = next.iter().flat_map(archives).any(|url| {
            url.rsplit('/')
                .next()
                .and_then(version_of)
                .is_some_and(|version| version == *this)
        });
        releases.extend(next);
        if own {
            break;
        }
    }
    Ok(along_line(&releases, this))
}

/// Whether `new` is older than `this`: by version, but dev's beta of a line is not older than the
/// next build of that line it was merged from (`0.5.0-beta.1` follows `0.5.0-next.7`). This trusts
/// that next is above dev: a beta of `X.Y.0` or above is taken from a next build of `X.Y.0` without
/// `--force`, because next only holds the version after dev's, so such a beta is that line merged
/// into dev, not a step back.
pub(crate) fn is_older(new: &Version, this: &Version) -> bool {
    let merged = this.pre.as_str().starts_with("next.")
        && new.pre.as_str().starts_with("beta.")
        && (new.major, new.minor, new.patch) >= (this.major, this.minor, this.patch);
    new < this && !merged
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
            let slug: String = branch
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b == b'-' {
                        b as char
                    } else {
                        '-'
                    }
                })
                .collect();
            let releases = json(&format!("{}/releases?per_page=100", api()))?.unwrap_or_default();
            let Some(url) = highest_alpha(releases.as_array().map_or(&[], Vec::as_slice), &slug)
            else {
                return Err(format!(
                    "there is no alpha release for {branch}: --channel beta (or another --branch) \
                     chooses another channel."
                )
                .into());
            };
            Some(url)
        }
        // The build's own line: it says its channel, which `--channel` does not offer.
        "rc" | "next" => {
            let this = Version::parse(VERSION)?;
            along_line_paged(&this, |page| {
                Ok(
                    json(&format!("{}/releases?per_page=100&page={page}", api()))?
                        .and_then(|releases| releases.as_array().cloned())
                        .unwrap_or_default(),
                )
            })?
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

/// The user's `curl`, reading no `curlrc`: an agent can write the user's (`~/.curlrc`,
/// `$XDG_CONFIG_HOME/curlrc`), and a `proxy`, `resolve` or `insecure` there would serve another
/// build with a matching sum. GitHub's releases are read over HTTPS only, redirections
/// included; the tests' server (`FLOTTILLE_RELEASES`) is plain HTTP on loopback.
fn curl_command() -> Command {
    let mut command = Command::new("curl");
    // curl honours `-q` only as its very first argument.
    command.arg("-q");
    if std::env::var_os("FLOTTILLE_RELEASES").is_none() {
        command.args(["--proto", "=https", "--proto-redir", "=https"]);
    }
    command
}

/// What the user's `curl` reads at `url`.
fn curl(url: &str, options: &[&str]) -> Result<Vec<u8>> {
    // Recorded as the daemon's own, which runs this too: its sweep of strays leaves it alone.
    let output = crate::children::spawn_std(
        curl_command()
            .args(options)
            .args(["--silent", "--show-error", "--location", "--", url])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .and_then(std::process::Child::wait_with_output)
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
        let folder = unpack(archive, &work, name)?;
        std::fs::rename(folder.join("flottille"), binary)?;
        // The binary is replaced: failing to write the launcher's files is said, not undone.
        if let Err(error) = desktop::write(paths, &folder.join("share"), binary) {
            eprintln!("flottille: the launcher entry was not updated: {error}");
        }
        Ok(name
            .trim_end_matches("-x86_64-linux.tar.gz")
            .trim_start_matches("flottille-")
            .to_owned())
    })();
    let _ = std::fs::remove_dir_all(&work);
    unpacked
}

/// Unpacks `archive`, named `name`, into `work` with the user's `tar`, from the bytes whose sum
/// was checked, not from a file that could change since: the package's folder.
fn unpack(archive: &[u8], work: &Path, name: &str) -> Result<PathBuf> {
    tar(
        archive,
        Command::new("tar").args(["-xzf", "-", "-C"]).arg(work),
    )
    .map_err(|e| format!("cannot unpack {name}: {e}"))?;
    Ok(work.join(package_of(name)))
}

/// The folder an archive named `name` holds.
fn package_of(name: &str) -> &str {
    name.trim_end_matches("-x86_64-linux.tar.gz")
}

/// The package's binary as `archive`, named `name`, holds it: read from the bytes whose sum was
/// checked, never from the disk.
fn binary_in(archive: &[u8], name: &str) -> Result<Vec<u8>> {
    let member = format!("{}/flottille", package_of(name));
    tar(
        archive,
        Command::new("tar").args(["-xzOf", "-", "--"]).arg(member),
    )
    .map_err(|e| format!("{name} holds no binary ({e}): nothing was staged").into())
}

/// What the user's `tar` writes, given `archive` on its input.
fn tar(archive: &[u8], command: &mut Command) -> std::result::Result<Vec<u8>, String> {
    let mut child = crate::children::spawn_std(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|e| format!("cannot run tar: {e}"))?;
    let mut input = child.stdin.take().ok_or("tar has no input")?;
    let output = std::thread::scope(|scope| {
        // Written beside the read of its output, which would fill otherwise; a tar that stops
        // reading says why in its status.
        scope.spawn(move || {
            let _ = std::io::Write::write_all(&mut input, archive);
        });
        child.wait_with_output()
    })
    .map_err(|e| format!("cannot run tar: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

/// The start of the name of the folder a download is unpacked in, beside the binary: an agent
/// never writes there (the gate).
pub const WORK_PREFIX: &str = ".flottille-update-";

/// A folder of its own beside the binary, so that the rename stays on one file system.
fn tempfile_dir(dir: &Path) -> Result<PathBuf> {
    for n in 0..100 {
        let path = dir.join(format!("{WORK_PREFIX}{}-{n}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err("cannot make a folder beside the binary".into())
}

// A build staged beside the binary (`docs/releases.md`, "Updating"): downloaded while Flottille
// runs, applied by the daemon's next start.

/// The binary this process runs from, or, when it was replaced while it ran, the file now at its
/// path: the one an update replaces and stages beside.
pub fn binary() -> Result<PathBuf> {
    Ok(crate::client::program_to_start(std::env::current_exe()?).canonicalize()?)
}

/// Where a build is staged: beside the binary, under its name and `.next`.
pub fn staging_of(binary: &Path) -> PathBuf {
    let mut name = binary.file_name().unwrap_or_default().to_owned();
    name.push(".next");
    binary.with_file_name(name)
}

/// The file of the staging that says what it holds: its version, the SHA-256 sum of its binary,
/// and the notes of the releases between this build and it.
const MANIFEST: &str = "staged.json";

/// What a staging holds, as its manifest says it.
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: String,
    sha256: String,
    channel: String,
    /// The notes of each release after this build up to the staged one, oldest first.
    #[serde(default)]
    notes: Vec<Value>,
}

/// The version staged beside `binary`, when a staging is there that looks whole; whether it is
/// applied is decided at the daemon's start (`apply_staged`).
pub fn staged(binary: &Path) -> Option<String> {
    let staging = staging_of(binary);
    if !std::fs::symlink_metadata(&staging).ok()?.is_dir() {
        return None;
    }
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(staging.join(MANIFEST)).ok()?).ok()?;
    Some(manifest.version)
}

/// The channel's latest build, when it is newer than this one: what the daemon checks.
pub fn latest(paths: &Paths) -> Result<Option<Version>> {
    let (channel, branch) = followed(&remembered(paths), None, None);
    let url = archive_url(&channel, branch.as_deref())?;
    let new = version_of(url.rsplit('/').next().unwrap_or_default())
        .ok_or("the latest release holds no archive of Flottille")?;
    let this = Version::parse(VERSION)?;
    Ok((new != this && !is_older(&new, &this)).then_some(new))
}

/// Downloads the channel's latest build, when it is newer than this one, and stages it beside
/// the binary: what the window's Download does. `progress` is told the bytes read, and of how
/// many when the server says it. The version staged.
pub fn stage(paths: &Paths, progress: &dyn Fn(u64, Option<u64>)) -> Result<Version> {
    let (channel, branch) = followed(&remembered(paths), None, None);
    let url = archive_url(&channel, branch.as_deref())?;
    let this = Version::parse(VERSION)?;
    match version_of(url.rsplit('/').next().unwrap_or_default()) {
        Some(new) if new != this && !is_older(&new, &this) => {}
        Some(new) => return Err(format!("{new} is not newer than this {this}").into()),
        None => return Err("the latest release holds no archive of Flottille".into()),
    }
    stage_from(&binary()?, &url, progress)
}

/// Downloads the archive at `url` and its sum, checks the sum, and unpacks it into the staging of
/// `binary`, with its manifest. Nothing that runs is touched; a wrong sum stages nothing, and
/// nothing is left beside the binary but the staging.
fn stage_from(binary: &Path, url: &str, progress: &dyn Fn(u64, Option<u64>)) -> Result<Version> {
    let name = url.rsplit('/').next().unwrap_or_default().to_owned();
    let version = version_of(&name).ok_or("the release holds no archive of Flottille")?;
    let staging = staging_of(binary);
    // Before anything is read: a link there is refused, and left for the user to see.
    if is_link(&staging) {
        return Err(not_followed(&staging));
    }
    let dir = binary.parent().ok_or("the binary has no folder")?;
    let work = tempfile_dir(dir)?;
    let staged = (|| -> Result<()> {
        let file = work.join(&name);
        download(url, &file, progress)?;
        let published = String::from_utf8(fetch(&format!("{url}.sha256"))?)?;
        let sum = published.split_whitespace().next().unwrap_or_default();
        // Read once: what is unpacked and staged is what was checked.
        let archive = std::fs::read(&file)?;
        if sum != sha256(&archive) {
            return Err(format!(
                "the SHA-256 sum of {name} is not the one published: nothing was staged"
            )
            .into());
        }
        let folder = unpack(&archive, &work, &name)?;
        let new = folder.join("flottille");
        if !std::fs::symlink_metadata(&new).is_ok_and(|meta| meta.is_file()) {
            return Err(format!("{name} holds no binary: nothing was staged").into());
        }
        // The binary staged is the archive's, written again from it: its sum in the manifest is
        // of what was checked, whatever the unpacked file became meanwhile.
        let program = binary_in(&archive, &name)?;
        std::fs::remove_file(&new)?;
        std::io::Write::write_all(
            &mut std::os::unix::fs::OpenOptionsExt::mode(
                std::fs::OpenOptions::new().write(true).create_new(true),
                0o755,
            )
            .open(&new)?,
            &program,
        )?;
        let manifest = Manifest {
            version: version.to_string(),
            sha256: sha256(&program),
            channel: CHANNEL.to_owned(),
            notes: notes(&Version::parse(VERSION)?, &version),
        };
        std::fs::write(folder.join(MANIFEST), serde_json::to_vec(&manifest)?)?;
        // What an earlier download staged is replaced.
        remove_staging(&staging)?;
        std::fs::rename(&folder, &staging)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    staged.map(|()| version)
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn not_followed(staging: &Path) -> Box<dyn std::error::Error> {
    format!(
        "{} is a link, not a staging of Flottille's: nothing was staged (remove it first)",
        staging.display()
    )
    .into()
}

/// Removes what is at the staging name, a folder or a file; a link is refused, never followed.
fn remove_staging(staging: &Path) -> Result<()> {
    match std::fs::symlink_metadata(staging) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(meta) if meta.file_type().is_symlink() => Err(not_followed(staging)),
        Ok(meta) if meta.is_dir() => Ok(std::fs::remove_dir_all(staging)?),
        Ok(_) => Ok(std::fs::remove_file(staging)?),
    }
}

/// What a plain update does with a staged build: it installed the latest, so the staging goes. A
/// link at its name is left alone.
fn forget_staged(binary: &Path) {
    let staging = staging_of(binary);
    if !is_link(&staging) && std::fs::symlink_metadata(&staging).is_ok() {
        match remove_staging(&staging) {
            Ok(()) => println!("The build staged beside it was removed."),
            Err(error) => eprintln!("flottille: the staged build was not removed: {error}"),
        }
    }
}

/// The file at `url` into `to` with the user's `curl`, its progress told every 200 ms.
fn download(url: &str, to: &Path, progress: &dyn Fn(u64, Option<u64>)) -> Result<()> {
    let total = size_of(url);
    let mut child = crate::children::spawn_std(
        curl_command()
            .args(["--fail", "--silent", "--show-error", "--location"])
            .arg("--output")
            .arg(to)
            .args(["--", url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    )
    .map_err(|e| format!("cannot run curl: {e}"))?;
    let read = || std::fs::metadata(to).map_or(0, |meta| meta.len());
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        progress(read(), total);
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    if !status.success() {
        let mut said = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut said);
        }
        return Err(format!("cannot read {url}: {}", said.trim()).into());
    }
    progress(read(), total);
    Ok(())
}

/// The size the server says of the file at `url`, following its redirections; none when it does
/// not say.
fn size_of(url: &str) -> Option<u64> {
    let headers = curl(url, &["--head", "--fail"]).ok()?;
    String::from_utf8_lossy(&headers)
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .next_back()
}

/// The notes of the releases after `this` up to `new`, of `new`'s kind (stable, the betas, an
/// rc's or a next's line, a branch's alphas), oldest first: what "What's new" says once it is
/// applied. None when the releases cannot be read: they are not worth failing a download for.
fn notes(this: &Version, new: &Version) -> Vec<Value> {
    notes_between(this, new).unwrap_or_default()
}

/// [`notes`], saying why the releases cannot be read: what "What's new" reads for a build that
/// was installed by hand (`whatsnew.rs`).
pub fn notes_between(this: &Version, new: &Version) -> Result<Vec<Value>> {
    // The kind of a version: its pre-release's first word, an alpha's with its branch.
    let kind = |version: &Version| -> String {
        let pre = version.pre.as_str();
        match pre.split('.').next() {
            Some("alpha") => pre
                .rsplit_once('.')
                .map_or(pre, |(kind, _)| kind)
                .to_owned(),
            Some(first) => first.to_owned(),
            None => String::new(),
        }
    };
    let releases = json(&format!("{}/releases?per_page=100", api()))?
        .ok_or("the releases cannot be read: GitHub has none")?;
    let mut found: Vec<(Version, Value)> = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|release| {
            let at = release["published_at"].as_str()?;
            let version = Version::parse(release["tag_name"].as_str()?.strip_prefix('v')?).ok()?;
            (version > *this && version <= *new && kind(&version) == kind(new)).then(|| {
                let said = serde_json::json!({
                    "version": version.to_string(),
                    "published_at": at,
                    "body": release["body"].as_str().unwrap_or_default(),
                });
                (version, said)
            })
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found.into_iter().map(|(_, said)| said).collect())
}

/// What the daemon's start found staged.
pub enum Staged {
    /// No staging.
    Nothing,
    /// The staged build is now the binary, at that path: the daemon starts again from it.
    Applied(PathBuf),
    /// What was staged was removed, for the reason said.
    Refused(String),
}

/// `$XDG_STATE_HOME/flottille/updated.json`: the last build applied, the version it replaced and
/// the notes of the releases between them.
pub fn updated_file(paths: &Paths) -> PathBuf {
    paths.state.join("updated.json")
}

/// Applies the build staged beside the binary, when there is one that is whole, newer than this
/// one and whose binary has the sum it was downloaded with: it is renamed over the binary, the
/// launcher entry written as `flottille update` writes it. What is not is removed. Only the
/// daemon's start calls it, holding the daemon's lock: no other daemon runs from the binary.
pub fn apply_staged(paths: &Paths) -> Staged {
    let Ok(binary) = binary() else {
        return Staged::Nothing;
    };
    let staging = staging_of(&binary);
    let meta = match std::fs::symlink_metadata(&staging) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Staged::Nothing,
        Err(error) => {
            return Staged::Refused(format!("{} cannot be read: {error}", staging.display()));
        }
        Ok(meta) => meta,
    };
    let refuse = |why: String| {
        // A link is removed, never what it names.
        let removed = if meta.is_dir() {
            std::fs::remove_dir_all(&staging)
        } else {
            std::fs::remove_file(&staging)
        };
        let removed = match removed {
            Ok(()) => "removed".to_owned(),
            Err(error) => format!("not removed ({error})"),
        };
        Staged::Refused(format!(
            "the build staged at {} was {removed}: {why}",
            staging.display()
        ))
    };
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no precondition and cannot fail.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid {
        return refuse("not a staged build (a link, a file, or another user's)".into());
    }
    let Some(manifest) = std::fs::read(staging.join(MANIFEST))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
    else {
        return refuse("not a staged build (its manifest is missing or unreadable)".into());
    };
    let new = staging.join("flottille");
    if !std::fs::symlink_metadata(&new).is_ok_and(|meta| meta.is_file()) {
        return refuse("not a staged build (its binary is missing or not a file)".into());
    }
    let this = Version::parse(VERSION);
    match (Version::parse(&manifest.version), &this) {
        (Ok(version), Ok(this)) if version != *this && !is_older(&version, this) => {}
        (_, Ok(this)) => {
            return refuse(format!(
                "{} is not newer than this {this}",
                manifest.version
            ));
        }
        (_, Err(error)) => return refuse(format!("this build's version: {error}")),
    }
    let sum = std::fs::File::open(&new).and_then(|mut file| crate::files::sha256_of(&mut file));
    if sum.ok().as_deref() != Some(manifest.sha256.as_str()) {
        return refuse("its SHA-256 sum is not the one it was downloaded with".into());
    }
    if let Err(error) = std::fs::rename(&new, &binary) {
        return refuse(format!(
            "it could not replace {}: {error}",
            binary.display()
        ));
    }
    if let Err(error) = desktop::write(paths, &staging.join("share"), &binary) {
        eprintln!("flottille daemon: the launcher entry was not updated: {error}");
    }
    let applied = serde_json::json!({
        "version": manifest.version,
        "from": VERSION,
        "channel": manifest.channel,
        "notes": manifest.notes,
    });
    let kept = flottille_core::paths::ensure_private_dir(&paths.state)
        .map_err(|error| error.to_string())
        .and_then(|()| {
            std::fs::write(updated_file(paths), applied.to_string()).map_err(|e| e.to_string())
        });
    if let Err(error) = kept {
        eprintln!("flottille daemon: what was applied was not kept: {error}");
    }
    let _ = std::fs::remove_dir_all(&staging);
    Staged::Applied(binary)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A published release of `version`, at `at` (UTC, one shape).
    fn release(version: &str, at: &str) -> Value {
        json!({
            "tag_name": format!("v{version}"),
            "published_at": at,
            "assets": [{"browser_download_url": format!("https://example.test/flottille-{version}-x86_64-linux.tar.gz")}],
        })
    }

    fn taken(releases: &[Value], this: &str) -> Option<String> {
        let url = along_line(releases, &Version::parse(this).unwrap())?;
        Some(version_of(url.rsplit('/').next()?)?.to_string())
    }

    #[test]
    fn an_rc_build_updates_along_its_line() {
        let mut releases = vec![
            release("0.3.0", "2026-11-01T10:00:00Z"),
            release("0.3.1-rc.1", "2026-11-02T10:00:00Z"),
            release("0.3.1-rc.2", "2026-11-03T10:00:00Z"),
            release("0.3.1-rc.3", "2026-11-04T10:00:00Z"),
            // Newer, and other lines' or other channels': never taken.
            release("0.2.4-rc.1", "2026-11-05T10:00:00Z"),
            release("0.4.0-beta.9", "2026-11-06T10:00:00Z"),
            release("0.5.0-next.1", "2026-11-07T10:00:00Z"),
            release("0.3.1-alpha.feat-a.1", "2026-11-08T10:00:00Z"),
        ];
        // The next rc of its line.
        assert_eq!(
            taken(&releases, "0.3.1-rc.2").as_deref(),
            Some("0.3.1-rc.3")
        );
        // Then the line's stable, which is above its rcs.
        releases.push(release("0.3.1", "2026-11-09T10:00:00Z"));
        assert_eq!(taken(&releases, "0.3.1-rc.3").as_deref(), Some("0.3.1"));
        // And the next fix's rc, for a build that did not move to the stable in time.
        releases.push(release("0.3.2-rc.1", "2026-11-10T10:00:00Z"));
        assert_eq!(
            taken(&releases, "0.3.1-rc.3").as_deref(),
            Some("0.3.2-rc.1")
        );
    }

    #[test]
    fn a_next_build_updates_along_its_line() {
        let mut releases = vec![
            release("0.5.0-next.1", "2026-11-01T10:00:00Z"),
            release("0.5.0-next.2", "2026-11-02T10:00:00Z"),
            release("0.5.0-next.3", "2026-11-03T10:00:00Z"),
            // Dev's betas before the line is merged, and other lines': never taken.
            release("0.4.0-beta.9", "2026-11-04T10:00:00Z"),
            release("0.3.1-rc.1", "2026-11-05T10:00:00Z"),
            release("0.6.0-next.1", "2026-11-06T10:00:00Z"),
        ];
        assert_eq!(
            taken(&releases, "0.5.0-next.2").as_deref(),
            Some("0.5.0-next.3")
        );
        // The line is merged into dev: dev's betas of it come after the last next.
        releases.push(release("0.5.0-beta.1", "2026-11-07T10:00:00Z"));
        let this = Version::parse("0.5.0-next.3").unwrap();
        assert_eq!(
            taken(&releases, "0.5.0-next.3").as_deref(),
            Some("0.5.0-beta.1")
        );
        // Which the build follows, though a beta sorts below a next.
        assert!(!is_older(&Version::parse("0.5.0-beta.1").unwrap(), &this));
        assert!(is_older(&Version::parse("0.5.0-next.1").unwrap(), &this));
        assert!(is_older(&Version::parse("0.4.0-beta.9").unwrap(), &this));
    }

    /// `n` pages of releases, a page being what `releases` hold, newest first.
    fn taken_paged(pages: &[Vec<Value>], this: &str) -> Option<String> {
        let url = along_line_paged(&Version::parse(this).unwrap(), |page| {
            Ok(pages.get(page as usize - 1).cloned().unwrap_or_default())
        })
        .unwrap()?;
        Some(version_of(url.rsplit('/').next()?)?.to_string())
    }

    /// Dev's betas, which fill a page of releases within weeks.
    fn betas(from: u32, count: u32) -> Vec<Value> {
        (from..from + count)
            .map(|n| release(&format!("0.4.0-beta.{n}"), "2026-11-20T10:00:00Z"))
            .collect()
    }

    #[test]
    fn an_rc_build_already_the_newest_of_its_line_says_already() {
        let releases = [
            release("0.3.1-rc.2", "2026-11-03T10:00:00Z"),
            release("0.3.1-rc.3", "2026-11-04T10:00:00Z"),
            release("0.4.0-beta.9", "2026-11-06T10:00:00Z"),
        ];
        // The archive taken is the build's own: `run` says "already", it does not update.
        assert_eq!(
            taken(&releases, "0.3.1-rc.3").as_deref(),
            Some("0.3.1-rc.3")
        );
    }

    #[test]
    fn an_rc_build_whose_release_is_on_page_two_says_already() {
        let pages = [
            betas(1, 100),
            vec![
                release("0.3.1-rc.2", "2026-11-03T10:00:00Z"),
                release("0.3.1-rc.1", "2026-11-02T10:00:00Z"),
            ],
        ];
        assert_eq!(
            taken_paged(&pages, "0.3.1-rc.2").as_deref(),
            Some("0.3.1-rc.2")
        );
    }

    #[test]
    fn an_rc_build_finds_a_newer_rc_on_page_two() {
        let pages = [
            betas(1, 100),
            vec![
                release("0.3.1-rc.3", "2026-11-04T10:00:00Z"),
                release("0.3.1-rc.2", "2026-11-03T10:00:00Z"),
            ],
        ];
        assert_eq!(
            taken_paged(&pages, "0.3.1-rc.2").as_deref(),
            Some("0.3.1-rc.3")
        );
    }

    #[test]
    fn an_rc_build_reads_the_pages_only_as_far_as_it_needs() {
        let mut asked = Vec::new();
        let this = Version::parse("0.3.1-rc.1").unwrap();
        // Its own release is never found: the reading stops at the ten pages.
        along_line_paged(&this, |page| {
            asked.push(page);
            Ok(betas(page * 100, 100))
        })
        .unwrap();
        assert_eq!(asked, (1..=10).collect::<Vec<_>>());
        // Found on the first page: the second is not read.
        asked.clear();
        along_line_paged(&this, |page| {
            asked.push(page);
            Ok(vec![release("0.3.1-rc.1", "2026-11-02T10:00:00Z")])
        })
        .unwrap();
        assert_eq!(asked, [1]);
    }
}
