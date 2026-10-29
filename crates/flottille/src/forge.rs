//! Forges (`docs/forges.md`): what the forge of each repository of a workspace says of the
//! workspace's branch, its pull request and their checks, and the writes of `forge_write` that a
//! what a session sends (`docs/commands.md`). Through the user's own command line (`gh` for
//! GitHub, `bkt` for Bitbucket), spawned without a shell, with the user's own sign-in: Flottille
//! never reads, keeps nor prints a token, and of `gh auth status` it reads the exit code alone.
//!
//! A workspace is read on demand (`workspace.forge`, the agents' `forge_read`), and again about
//! once a minute while a client watches it, less often while `gh` or `bkt` fails. Nothing is read
//! while nobody looks. Every text kept from them is masked before it is shown or stored.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::declared::{self, RepositoryForge};
use flottille_core::project::Project;
use flottille_core::protocol::WorkspaceForge;
use flottille_core::secrets;
use flottille_core::store::now_ms;
use flottille_core::workspace::{Base, Workspace};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::git::{self, Class};
use crate::mcp::{Outcome, arguments, full_detail};
use crate::place::Place;
use crate::projects::{self, Failure};

mod bitbucket;

/// How long a state read stays fresh, and how often a watched workspace is read again.
pub const EVERY_MS: u64 = 60_000;
/// The longest wait between two reads of a watched workspace whose reads keep failing.
const LONGEST_MS: u64 = 16 * 60_000;
/// A call of `gh` is cut after this.
const GH_TIME: Duration = Duration::from_secs(30);
/// A call of `ssh -G` is cut after this.
const SSH_TIME: Duration = Duration::from_secs(5);
/// What `forge_read` shows of one comment at most.
const COMMENT_CHARS: usize = 2000;
/// What `gh pr view` is asked for.
const FIELDS: &str = "number,title,state,isDraft,baseRefName,url,reviewDecision,\
    statusCheckRollup,comments,reviews";

/// The forge a repository's remote is on: GitHub, or Bitbucket (Cloud, or a Data Center host the
/// project names).
#[derive(Clone, Debug, PartialEq)]
pub enum Forge {
    GitHub {
        owner: String,
        name: String,
    },
    Bitbucket {
        /// `bitbucket.org` for Cloud, else the Data Center host, lowercase.
        host: String,
        /// The workspace (Cloud) or the project key (Data Center).
        owner: String,
        name: String,
    },
}

impl Forge {
    pub fn kind(&self) -> &'static str {
        match self {
            Forge::GitHub { .. } => "github",
            Forge::Bitbucket { .. } => "bitbucket",
        }
    }

    /// `owner/name`, as the forge's command line takes it.
    pub fn slug(&self) -> String {
        match self {
            Forge::GitHub { owner, name } | Forge::Bitbucket { owner, name, .. } => {
                format!("{owner}/{name}")
            }
        }
    }

    /// What `gh -R` is given: the host first, so that `GH_HOST` in the user's environment never
    /// sends the call to another host than the one the declaration was checked against.
    pub fn gh_repository(&self) -> String {
        format!("{}/{}", declared::GITHUB, self.slug())
    }
}

impl From<&RepositoryForge> for Forge {
    fn from(declared: &RepositoryForge) -> Forge {
        let (owner, name) = declared.owner_and_name();
        let (owner, name) = (owner.to_owned(), name.to_owned());
        match declared {
            RepositoryForge::Github { .. } => Forge::GitHub { owner, name },
            RepositoryForge::BitbucketCloud { .. }
            | RepositoryForge::BitbucketDataCenter { .. } => Forge::Bitbucket {
                host: declared.host().to_owned(),
                owner,
                name,
            },
        }
    }
}

/// What is said of a repository whose forge is not declared: nothing is read nor written for it.
pub const NO_FORGE: &str = "no forge set for this repository: choose it in the project's \
    Settings (the window), or write its `forge` in the project's file; Flottille never guesses it";

/// One repository of a workspace as a forge is read for it: its path, the folder of its main
/// checkout (whose remotes its worktree shares), its remote and the forge the project declares.
#[derive(Clone, Debug)]
pub struct Checkout {
    pub path: String,
    pub folder: PathBuf,
    pub remote: String,
    pub declared: Option<RepositoryForge>,
}

/// The forge a repository is read and written on: the declared one, once its remote is checked
/// to lead there. `Err(None)` when none is declared, `Err(Some(why))` when the remote leads
/// elsewhere: neither the declaration nor the remote wins.
pub async fn declared_forge(checkout: &Checkout) -> Result<Forge, Option<String>> {
    let Some(declared) = &checkout.declared else {
        return Err(None);
    };
    match disagreement(declared, &checkout.folder, &checkout.remote).await {
        None => Ok(Forge::from(declared)),
        Some(why) => Err(Some(format!(
            "refused: {} is declared on {}, and {why}; nothing was asked of the forge: correct the \
             repository's remote or its forge in the project's settings",
            checkout.path,
            declared.said()
        ))),
    }
}

/// Why the remote `remote` of the repository in `folder` does not lead to `declared`, masked;
/// none when every one of its fetch and push URLs does, since git fetches from one and pushes to
/// another while the forge is asked of the declared repository.
pub async fn disagreement(
    declared: &RepositoryForge,
    folder: &Path,
    remote: &str,
) -> Option<String> {
    let urls = urls_of(folder, remote).await;
    if urls.is_empty() {
        return Some(format!("it has no remote {remote}"));
    }
    for url in &urls {
        let agrees = match declared::address(url) {
            Some((host, path)) if host != declared.host() && declared::over_ssh(url) => {
                match ssh_host(&host).await {
                    Ok(real) if real == host => declared.agrees_at(&host, &path),
                    Ok(real) => declared
                        .agrees_at(&real, &path)
                        .map_err(|why| format!("{why} ({host} is an SSH alias of {real})")),
                    Err(not) => declared
                        .agrees_at(&host, &path)
                        .map_err(|why| format!("{why} ({not})")),
                }
            }
            _ => declared.agrees(url),
        };
        if let Err(why) = agrees {
            return Some(masked(&format!("its remote {remote}: {why}")));
        }
    }
    None
}

/// The host `ssh` connects to for `host`, an alias of the user's SSH configuration or not: the
/// `hostname` line of `ssh -G <host>`, and nothing else of what it prints.
async fn ssh_host(host: &str) -> Result<String, String> {
    let not = |why: String| format!("ssh -G {host} did not say its host: {why}");
    // A name that could pass for an option is never given to `ssh`.
    if host.starts_with('-')
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
    {
        return Err(not("it is not a host name".into()));
    }
    match run_for("ssh", SSH_TIME, &[], &[], &["-G", host]).await {
        Ok((true, stdout, _)) => stdout
            .lines()
            .find_map(|line| {
                let (key, value) = line.trim().split_once(char::is_whitespace)?;
                key.eq_ignore_ascii_case("hostname")
                    .then(|| value.trim().to_ascii_lowercase())
            })
            .filter(|real| !real.is_empty())
            .ok_or_else(|| not("no hostname line".into())),
        Ok((false, _, stderr)) => Err(not(stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("it failed")
            .chars()
            .take(200)
            .collect())),
        Err(Gh::Missing) => Err(not("ssh is not on the daemon's PATH".into())),
        Err(Gh::Failed(why)) => Err(not(why)),
    }
}

/// Where a repository's remote leads, as a workspace records it and a push and `forge_write`
/// check it: its fetch and push URLs as git uses them (`insteadOf` applied), credentials taken
/// out, sorted; empty when git names none.
pub async fn urls_of(folder: &Path, remote: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for push in [&[][..], &["--push"][..]] {
        let args = [&["remote", "get-url", "--all"][..], push, &[remote]].concat();
        match git::output(folder, &args, Class::Read).await {
            Ok(said) => urls.extend(
                said.lines()
                    .map(str::trim)
                    .filter(|url| !url.is_empty())
                    .map(without_credentials),
            ),
            Err(_) => return Vec::new(),
        }
    }
    urls.sort();
    urls.dedup();
    urls
}

pub(crate) use flottille_core::declared::without_credentials;

/// Refuses unless the repository's remote leads where it led when the workspace was created: an
/// agent never changes it, and a push or a forge write an agent makes goes nowhere else.
pub async fn remote_kept(folder: &Path, base: &Base) -> Result<(), String> {
    if base.urls.is_empty() {
        return Err(format!(
            "refused: this workspace was created before Flottille recorded where its remote {} \
             leads, and an agent's push or forge write goes only where that is known: say \
             what should be done, and the user does it",
            base.remote
        ));
    }
    let now = urls_of(folder, &base.remote).await;
    if now != base.urls {
        return Err(format!(
            "refused: the remote {} no longer leads where it led when the workspace was created \
             ({}): it leads to {}; nothing was sent: say so, and the user checks the \
             repository's remotes",
            base.remote,
            masked(&base.urls.join(", ")),
            if now.is_empty() {
                "nothing".to_owned()
            } else {
                masked(&now.join(", "))
            }
        ));
    }
    Ok(())
}

/// A pull request as the forge said it, its texts masked.
#[derive(Clone, Debug, PartialEq)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: &'static str,
    pub base: String,
    pub url: String,
    /// `approved`, `changes requested`, `review required`, or none.
    pub review: Option<&'static str>,
    /// Each check's name and state: `passing`, `failing`, `pending` or `skipped`.
    pub checks: Vec<(String, &'static str)>,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Comment>,
    /// The reviewers asked and their verdict (`approved`, `changes requested`, `pending`), of
    /// the forges that say them (Bitbucket); empty for GitHub, whose reviews say it.
    pub reviewers: Vec<(String, &'static str)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub author: String,
    /// A review's verdict; none for a comment.
    pub state: Option<String>,
    pub body: String,
}

impl Pull {
    /// The checks as one mark: `failing` when one fails, else `pending` when one runs, else
    /// `passing`; none without checks.
    pub fn mark(&self) -> Option<&'static str> {
        let any = |state| self.checks.iter().any(|(_, s)| *s == state);
        if self.checks.is_empty() {
            None
        } else if any("failing") {
            Some("failing")
        } else if any("pending") {
            Some("pending")
        } else {
            Some("passing")
        }
    }

    /// What blocks its merge: its checks failing or still running, or a review asking for
    /// changes; none when nothing does.
    pub fn blocks(&self) -> Option<String> {
        blocks(&self.checks, self.review)
    }

    fn to_json(&self) -> Value {
        let mut pull = json!({
            "number": self.number,
            "title": self.title,
            "state": self.state,
            "base": self.base,
            "url": self.url,
            "review": self.review,
            "checks": self.checks.iter()
                .map(|(name, state)| json!({"name": name, "state": state}))
                .collect::<Vec<_>>(),
            "mark": self.mark(),
            "comments": self.comments.len(),
        });
        if !self.reviewers.is_empty() {
            pull["reviewers"] = self
                .reviewers
                .iter()
                .map(|(name, state)| json!({"name": name, "state": state}))
                .collect();
        }
        pull
    }

    fn of(value: &Value) -> Option<Pull> {
        let text = |value: &Value| masked(value.as_str().unwrap_or_default());
        let comments = |key: &str| {
            value[key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|comment| Comment {
                    author: text(&comment["author"]["login"]),
                    state: comment["state"]
                        .as_str()
                        .map(|s| s.to_lowercase().replace('_', " ")),
                    body: text(&comment["body"]),
                })
                .collect()
        };
        Some(Pull {
            number: value["number"].as_u64()?,
            title: text(&value["title"]),
            state: match (value["state"].as_str(), value["isDraft"] == true) {
                (Some("MERGED"), _) => "merged",
                (Some("CLOSED"), _) => "closed",
                (_, true) => "draft",
                _ => "open",
            },
            base: text(&value["baseRefName"]),
            url: text(&value["url"]),
            review: match value["reviewDecision"].as_str() {
                Some("APPROVED") => Some("approved"),
                Some("CHANGES_REQUESTED") => Some("changes requested"),
                Some("REVIEW_REQUIRED") => Some("review required"),
                _ => None,
            },
            checks: value["statusCheckRollup"]
                .as_array()
                .into_iter()
                .flatten()
                .map(check_of)
                .collect(),
            comments: comments("comments"),
            reviews: comments("reviews"),
            reviewers: Vec::new(),
        })
    }
}

/// A check of `statusCheckRollup`: a check run (`status`, `conclusion`) or a commit status
/// (`state`).
fn check_of(check: &Value) -> (String, &'static str) {
    let name = check["name"]
        .as_str()
        .or(check["context"].as_str())
        .unwrap_or("check");
    let state = match check["__typename"].as_str() {
        Some("StatusContext") => match check["state"].as_str() {
            Some("SUCCESS") => "passing",
            Some("FAILURE" | "ERROR") => "failing",
            _ => "pending",
        },
        _ if check["status"] != "COMPLETED" => "pending",
        _ => match check["conclusion"].as_str() {
            Some("SUCCESS") => "passing",
            Some("NEUTRAL" | "SKIPPED") => "skipped",
            _ => "failing",
        },
    };
    (masked(name), state)
}

/// What reading one repository's forge found.
#[derive(Debug)]
pub struct Repository {
    pub path: String,
    pub remote: String,
    pub forge: Option<Forge>,
    /// Why there is nothing to read: no forge declared, or its remote disagrees.
    pub note: Option<String>,
    pub pull: Option<Pull>,
    /// The forge could not be read.
    pub error: Option<String>,
    /// Not read, because its forge's command line is missing or has no sign-in (`Read::problem`).
    pub blocked: bool,
}

/// What reading a workspace's forges found.
#[derive(Debug)]
pub struct Read {
    /// `gh` or `bkt` missing or not signed in: said once for the workspace, with what to do.
    pub problem: Option<String>,
    pub repositories: Vec<Repository>,
}

impl Read {
    fn failed(&self) -> bool {
        self.problem.is_some() || self.repositories.iter().any(|r| r.error.is_some())
    }
}

/// Reads the forge each repository declares, for `branch`. A repository with none is said as
/// such; one whose remote leads elsewhere is refused, and its forge is not asked.
pub async fn read(repositories: &[Checkout], branch: &str, obsolete: bool) -> Read {
    let mut found = Vec::new();
    for checkout in repositories {
        let (forge, note, error) = match declared_forge(checkout).await {
            Ok(forge) => (Some(forge), None, None),
            Err(None) if obsolete => (
                None,
                Some(format!(
                    "{NO_FORGE} (the project's [forges] table is no longer read)"
                )),
                None,
            ),
            Err(None) => (None, Some(NO_FORGE.to_owned()), None),
            Err(Some(why)) => (None, None, Some(why)),
        };
        found.push(Repository {
            path: checkout.path.clone(),
            remote: checkout.remote.clone(),
            forge,
            note,
            pull: None,
            error,
            blocked: false,
        });
    }
    // Each command line's problem is said once, and stops only the repositories it would read.
    let mut problems: Vec<String> = Vec::new();
    let github = found
        .iter()
        .any(|r| matches!(r.forge, Some(Forge::GitHub { .. })));
    let gh_problem = match github {
        true => signed_in().await.err(),
        false => None,
    };
    problems.extend(gh_problem.clone());
    let mut contexts = bitbucket::Contexts::default();
    for repository in &mut found {
        let Some(forge) = repository.forge.clone() else {
            continue;
        };
        let read = match &forge {
            Forge::GitHub { .. } => match gh_problem {
                Some(_) => Err(None),
                None => pull_of(&forge, branch).await.map_err(Some),
            },
            Forge::Bitbucket { .. } => match contexts.of(&forge).await {
                Err(problem) => {
                    if !problems.contains(&problem) {
                        problems.push(problem);
                    }
                    Err(None)
                }
                Ok(context) => bitbucket::pull_of(&forge, &context, branch)
                    .await
                    .map_err(Some),
            },
        };
        match read {
            Ok(pull) => repository.pull = pull,
            Err(Some(error)) => repository.error = Some(error),
            Err(None) => repository.blocked = true,
        }
    }
    Read {
        problem: (!problems.is_empty()).then(|| problems.join("; ")),
        repositories: found,
    }
}

/// Whether `gh` is there and signed in: its exit code, nothing it prints.
pub(crate) async fn signed_in() -> Result<(), String> {
    match gh(&["auth", "status", "--hostname", "github.com"]).await {
        Err(Gh::Missing) => Err(
            "gh, GitHub's command line, is not on the daemon's PATH: install \
             it (https://cli.github.com) and sign in with `gh auth login` to see the pull \
             requests; the rest of Flottille works without it"
                .into(),
        ),
        Err(Gh::Failed(why)) => Err(format!("gh cannot run: {why}")),
        Ok((false, _, _)) => Err(
            "gh is not signed in to github.com: run `gh auth login` to see \
             the pull requests; the rest of Flottille works without it"
                .into(),
        ),
        Ok((true, _, _)) => Ok(()),
    }
}

/// The pull request of `branch`, the newest when there are several; none when there is none.
async fn pull_of(forge: &Forge, branch: &str) -> Result<Option<Pull>, String> {
    let repo = forge.gh_repository();
    let listed = gh_json(&[
        "pr", "list", "-R", &repo, "--head", branch, "--state", "all", "--limit", "1", "--json",
        "number",
    ])
    .await?;
    let Some(number) = listed[0]["number"].as_u64() else {
        return Ok(None);
    };
    let number = number.to_string();
    let viewed = gh_json(&["pr", "view", &number, "-R", &repo, "--json", FIELDS]).await?;
    Pull::of(&viewed)
        .map(Some)
        .ok_or_else(|| format!("gh pr view {number} answered no pull request"))
}

pub(crate) enum Gh {
    Missing,
    Failed(String),
}

/// `gh <args>`: whether it succeeded, its output and its errors.
pub(crate) async fn gh(args: &[&str]) -> Result<(bool, String, String), Gh> {
    run(
        "gh",
        &[
            // Nothing waits for an answer, and nothing but the asked JSON is printed.
            ("GH_PROMPT_DISABLED", "1"),
            ("GH_NO_UPDATE_NOTIFIER", "1"),
            ("NO_COLOR", "1"),
        ],
        &[],
        args,
    )
    .await
}

/// `program <args>`, spawned without a shell with the daemon's environment (the user's own
/// sign-in) but for what is set or removed, cut after `GH_TIME`: whether it succeeded, its output
/// and its errors.
async fn run(
    program: &str,
    set: &[(&str, &str)],
    remove: &[&str],
    args: &[&str],
) -> Result<(bool, String, String), Gh> {
    run_for(program, GH_TIME, set, remove, args).await
}

/// `run`, cut after `time`.
async fn run_for(
    program: &str,
    time: Duration,
    set: &[(&str, &str)],
    remove: &[&str],
    args: &[&str],
) -> Result<(bool, String, String), Gh> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).envs(set.iter().copied());
    for name in remove {
        command.env_remove(name);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Gh::Missing,
        _ => Gh::Failed(error.to_string()),
    })?;
    match tokio::time::timeout(time, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )),
        Ok(Err(error)) => Err(Gh::Failed(error.to_string())),
        Err(_) => Err(Gh::Failed(format!(
            "{program} {} took more than {} s",
            args[..2].join(" "),
            time.as_secs()
        ))),
    }
}

/// `gh <args>`'s JSON, or a sentence saying why there is none, masked.
pub(crate) async fn gh_json(args: &[&str]) -> Result<Value, String> {
    let shown = args[..2].join(" ");
    match gh(args).await {
        Ok((true, stdout, _)) => serde_json::from_str(&stdout)
            .map_err(|error| format!("gh {shown} answered what is not JSON: {error}")),
        Ok((false, _, stderr)) => {
            let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            Err(format!(
                "gh {shown} failed: {}",
                masked(&said.chars().take(300).collect::<String>())
            ))
        }
        Err(Gh::Missing) => Err("gh is not on the daemon's PATH".into()),
        Err(Gh::Failed(why)) => Err(format!("gh {shown}: {why}")),
    }
}

/// Masked of the daemon's secrets, and of every token shaped like one.
pub(crate) fn masked(text: &str) -> String {
    let env: std::collections::BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    secrets::mask(text, &env)
}

/// What the protocol shows of a workspace's forges (`docs/protocol.md`, `workspace.forge`).
fn state_of(workspace: &Workspace, read: &Read) -> Value {
    json!({
        "project": workspace.project,
        "workspace": workspace.name,
        "branch": workspace.branch,
        "read_at": now_ms(),
        "problem": read.problem,
        "repositories": read.repositories.iter().map(|r| json!({
            "path": r.path,
            "remote": r.remote,
            "forge": r.forge.as_ref().map(Forge::kind),
            "repository": r.forge.as_ref().map(Forge::slug),
            "note": r.note,
            "pull_request": r.pull.as_ref().map(Pull::to_json),
            "error": r.error,
        })).collect::<Vec<_>>(),
    })
}

/// The repositories of a workspace, as `read` takes them, with the forges `project` declares
/// (none when its file cannot be read, which the rest of Flottille says in its own words).
pub fn repositories_of(workspace: &Workspace, project: Option<&Project>) -> Vec<Checkout> {
    workspace
        .repositories
        .iter()
        .map(|r| {
            let folder = match r.path.as_str() {
                "." => workspace.project_folder.clone(),
                path => workspace.project_folder.join(path),
            };
            let declared = project
                .and_then(|p| p.repositories.iter().find(|d| d.path == r.path))
                .and_then(|d| d.forge.clone());
            Checkout {
                path: r.path.clone(),
                folder,
                remote: r.base.remote.clone(),
                declared,
            }
        })
        .collect()
}

/// What the daemon keeps of each workspace's forges, in memory: the last state read, who watches
/// it, and when it is read again.
#[derive(Default)]
pub struct Forges {
    entries: Mutex<HashMap<i64, Entry>>,
}

#[derive(Default)]
struct Entry {
    project: String,
    name: String,
    state: Option<Value>,
    watchers: usize,
    /// Reads that failed in a row: each doubles the wait before the next one.
    failures: u32,
    next_at: u64,
    reading: bool,
}

/// `workspace.forge`.
pub async fn call(daemon: Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let asked: WorkspaceForge = serde_json::from_value(params)?;
    let workspace = daemon
        .read(|store| store.workspace(&asked.project, &asked.name))?
        .ok_or_else(|| {
            Failure::new(
                "not-found",
                format!(
                    "project {} has no workspace named {}",
                    asked.project, asked.name
                ),
            )
        })?;
    Ok(state(&daemon, &workspace).await)
}

/// The workspace's forge state: the last one read when it is fresh, else read now.
pub async fn state(daemon: &Daemon, workspace: &Workspace) -> Value {
    let kept = daemon
        .forges
        .entries
        .lock()
        .unwrap()
        .get(&workspace.id)
        .and_then(|entry| entry.state.clone());
    if let Some(state) = kept
        && now_ms().saturating_sub(state["read_at"].as_u64().unwrap_or(0)) < EVERY_MS
    {
        return state;
    }
    refresh(daemon, workspace).await
}

/// Reads the workspace's forges now and keeps what was read; an event says when it changed.
async fn refresh(daemon: &Daemon, workspace: &Workspace) -> Value {
    let project = projects::load_project(daemon, &workspace.project).ok();
    let obsolete = project.as_ref().is_some_and(|p| {
        p.obsolete
            .iter()
            .any(|o| o == flottille_core::project::OBSOLETE_FORGES)
    });
    let repositories = repositories_of(workspace, project.as_ref());
    let read = read(&repositories, &workspace.branch, obsolete).await;
    let state = state_of(workspace, &read);
    let now = now_ms();
    let previous = {
        let mut entries = daemon.forges.entries.lock().unwrap();
        let entry = entries.entry(workspace.id).or_default();
        entry.project = workspace.project.clone();
        entry.name = workspace.name.clone();
        entry.reading = false;
        entry.failures = match read.failed() {
            true => entry.failures.saturating_add(1),
            false => 0,
        };
        entry.next_at = now + (EVERY_MS << entry.failures.min(8)).min(LONGEST_MS);
        entry.state.replace(state.clone())
    };
    let same = |a: &Value, b: &Value| {
        let (mut a, mut b) = (a.clone(), b.clone());
        a["read_at"] = Value::Null;
        b["read_at"] = Value::Null;
        a == b
    };
    // A workspace with no forge to read has nothing to say until it has one.
    let changed = match &previous {
        None => read.problem.is_some() || read.repositories.iter().any(|r| r.forge.is_some()),
        Some(previous) => !same(previous, &state),
    };
    if changed {
        if let Some(problem) = &read.problem
            && previous
                .as_ref()
                .is_none_or(|p| p["problem"] != problem.as_str())
        {
            eprintln!("flottille daemon: forges: {problem}");
        }
        if let Err(error) = daemon.emit("workspace.forge", state.clone()) {
            eprintln!("flottille daemon: workspace.forge: {error}");
        }
    }
    state
}

/// A client watching a workspace: while one does, the daemon reads its forges again about once
/// a minute. Dropped when the client's connection closes.
pub struct Watch {
    daemon: Arc<Daemon>,
    id: i64,
}

pub fn watch(daemon: &Arc<Daemon>, workspace: &Workspace) -> Watch {
    let mut entries = daemon.forges.entries.lock().unwrap();
    let entry = entries.entry(workspace.id).or_default();
    entry.project = workspace.project.clone();
    entry.name = workspace.name.clone();
    entry.watchers += 1;
    Watch {
        daemon: daemon.clone(),
        id: workspace.id,
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(entry) = self.daemon.forges.entries.lock().unwrap().get_mut(&self.id) {
            entry.watchers = entry.watchers.saturating_sub(1);
        }
    }
}

/// Reads again the watched workspaces whose time has come (from the sweep).
pub fn poll(daemon: &Arc<Daemon>) {
    let now = now_ms();
    let due: Vec<(String, String)> = daemon
        .forges
        .entries
        .lock()
        .unwrap()
        .values_mut()
        .filter(|entry| entry.watchers > 0 && !entry.reading && entry.next_at <= now)
        .map(|entry| {
            entry.reading = true;
            (entry.project.clone(), entry.name.clone())
        })
        .collect();
    for (project, name) in due {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            match daemon.read(|store| store.workspace(&project, &name)) {
                Ok(Some(workspace)) => {
                    refresh(&daemon, &workspace).await;
                }
                // Removed: nothing to read any more.
                _ => {
                    let mut entries = daemon.forges.entries.lock().unwrap();
                    entries.retain(|_, e| e.project != project || e.name != name);
                }
            }
        });
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgeRead {
    repository: Option<String>,
    detail: Option<String>,
}

/// `forge_read`: the pull requests of the agent's own workspace's repositories, their checks and
/// their review comments. Never another repository.
pub async fn forge_read(daemon: &Daemon, place: &Place, args: Value) -> Outcome {
    let asked: ForgeRead = match arguments("forge_read", args) {
        Ok(asked) => asked,
        Err(refusal) => return refusal,
    };
    let full = match full_detail("forge_read", asked.detail.as_deref()) {
        Ok(full) => full,
        Err(refusal) => return refusal,
    };
    let Some(name) = &place.workspace else {
        return Outcome::Refused(
            "refused: this agent works in the project folder, not in a workspace: forge_read \
             reads a workspace's pull requests"
                .into(),
        );
    };
    let workspace = match daemon.read(|store| store.workspace(&place.project, name)) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return Outcome::Failed(format!("the workspace {name} is gone")),
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let project = projects::load_project(daemon, &place.project).ok();
    let obsolete = project.as_ref().is_some_and(|p| {
        p.obsolete
            .iter()
            .any(|o| o == flottille_core::project::OBSOLETE_FORGES)
    });
    let mut repositories = repositories_of(&workspace, project.as_ref());
    if let Some(asked) = &asked.repository {
        if !repositories.iter().any(|r| &r.path == asked) {
            let names: Vec<&str> = repositories.iter().map(|r| r.path.as_str()).collect();
            return Outcome::Refused(format!(
                "refused: {asked} is not a repository of this agent's workspace: it has {}; \
                 forge_read reads only those",
                names.join(", ")
            ));
        }
        repositories.retain(|r| &r.path == asked);
    }
    let read = read(&repositories, &workspace.branch, obsolete).await;
    Outcome::Done(read_text(
        &workspace,
        &read,
        full,
        asked.repository.as_deref(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgeWrite {
    action: String,
    repository: String,
    title: Option<String>,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    /// pr-merge-squash: why the user lets a pull request whose checks or review block it be
    /// merged; the gate held the call for them.
    waive: Option<String>,
}

/// What blocks the merge of a pull request with these checks and this review; none when nothing
/// does.
fn blocks(checks: &[(String, &'static str)], review: Option<&str>) -> Option<String> {
    let named = |state: &str| {
        let names: Vec<&str> = checks
            .iter()
            .filter(|(_, s)| *s == state)
            .map(|(name, _)| name.as_str())
            .collect();
        (!names.is_empty()).then(|| names.join(", "))
    };
    let mut blocks = Vec::new();
    if let Some(names) = named("failing") {
        blocks.push(format!("its checks fail ({names})"));
    }
    if let Some(names) = named("pending") {
        blocks.push(format!("its checks are still running ({names})"));
    }
    if review == Some("changes requested") {
        blocks.push("a review asks for changes".to_owned());
    }
    (!blocks.is_empty()).then(|| blocks.join(", "))
}

/// `forge_write`, once the gate let it through: one write to the forge of
/// one repository of the agent's own workspace, for the workspace's branch only, through the
/// user's own `gh`.
pub async fn forge_write(daemon: &Daemon, place: &Place, args: Value) -> Outcome {
    let asked: ForgeWrite = match arguments("forge_write", args) {
        Ok(asked) => asked,
        Err(refusal) => return refusal,
    };
    let Some(name) = &place.workspace else {
        return Outcome::Refused(
            "refused: this agent works in the project folder, not in a workspace: forge_write \
             writes for a workspace's branch"
                .into(),
        );
    };
    let workspace = match daemon.read(|store| store.workspace(&place.project, name)) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return Outcome::Failed(format!("the workspace {name} is gone")),
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    // The project's reviewers and declared forges, as its file says them now.
    let project = match projects::load_project(daemon, &place.project) {
        Ok(project) => project,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    let repositories = repositories_of(&workspace, Some(&project));
    let found = repositories
        .iter()
        .zip(&workspace.repositories)
        .find(|(checkout, _)| checkout.path == asked.repository);
    let Some((checkout, repository)) = found else {
        let names: Vec<&str> = repositories.iter().map(|r| r.path.as_str()).collect();
        return Outcome::Refused(format!(
            "refused: {} is not a repository of this agent's workspace: it has {}; forge_write \
             writes only for those",
            asked.repository,
            names.join(", ")
        ));
    };
    if let Err(refusal) = remote_kept(&checkout.folder, &repository.base).await {
        return Outcome::Refused(refusal);
    }
    let forge = match declared_forge(checkout).await {
        Ok(forge) => forge,
        Err(None) => {
            return Outcome::Refused(format!(
                "refused: {}: {NO_FORGE}; nothing was written: ask the user to set it",
                asked.repository
            ));
        }
        Err(Some(why)) => return Outcome::Refused(why),
    };
    let branch = workspace.branch.as_str();
    let reviewers = project.pull_requests.reviewers;
    if matches!(forge, Forge::Bitbucket { .. }) {
        let base = &repository.base.branch;
        return bitbucket::write(&forge, branch, base, &reviewers, asked).await;
    }
    if let Err(problem) = signed_in().await {
        return Outcome::Failed(problem);
    }
    let slug = forge.slug();
    let repo = forge.gh_repository();
    match asked.action.as_str() {
        "pr-create" => {
            let Some(title) = asked.title.filter(|t| !t.trim().is_empty()) else {
                return Outcome::Refused(
                    "refused: pr-create needs a title: say what the pull request does".into(),
                );
            };
            let base = &repository.base.branch;
            let body = asked.body.unwrap_or_default();
            let mut args = vec![
                "pr", "create", "-R", &repo, "--head", branch, "--base", base, "--title", &title,
                "--body", &body,
            ];
            if asked.draft {
                args.push("--draft");
            }
            for reviewer in &reviewers {
                args.extend(["--reviewer", reviewer]);
            }
            match gh_text(&args).await {
                Ok(said) => Outcome::Done(format!(
                    "The pull request of {branch} into {base} was opened in {slug}: {}",
                    said.trim()
                )),
                Err(why) => Outcome::Failed(why),
            }
        }
        "pr-merge-squash" => {
            let base = &repository.base.branch;
            let listed = match gh_json(&[
                "pr",
                "list",
                "-R",
                &repo,
                "--head",
                branch,
                "--state",
                "open",
                "--json",
                "number,isCrossRepository,baseRefName",
            ])
            .await
            {
                Ok(listed) => listed.as_array().cloned().unwrap_or_default(),
                Err(why) => return Outcome::Failed(why),
            };
            // `--head` names a branch, in this repository or in any fork of it.
            let ours = |pull: &&Value| {
                pull["isCrossRepository"] == false && pull["baseRefName"].as_str() == Some(base)
            };
            let Some(number) = listed.iter().find(ours).and_then(|p| p["number"].as_u64()) else {
                let why = match listed.first() {
                    None => format!("{branch} has no open pull request in {slug}: open it first"),
                    Some(pull) if pull["isCrossRepository"] != false => format!(
                        "the open pull request #{} of a branch {branch} in {slug} comes from a \
                         fork, not from the workspace's repository",
                        pull["number"]
                    ),
                    Some(pull) => format!(
                        "the open pull request #{} of {branch} in {slug} goes into {}, not \
                         {base}, the workspace's base",
                        pull["number"],
                        masked(pull["baseRefName"].as_str().unwrap_or("another branch"))
                    ),
                };
                return Outcome::Refused(format!(
                    "refused: {why}: only the pull request of the workspace's own branch into \
                     its base is merged"
                ));
            };
            let number = number.to_string();
            if asked.waive.is_none() {
                let viewed =
                    match gh_json(&["pr", "view", &number, "-R", &repo, "--json", FIELDS]).await {
                        Ok(viewed) => viewed,
                        Err(why) => return Outcome::Failed(why),
                    };
                if let Some(blocks) = Pull::of(&viewed).as_ref().and_then(Pull::blocks) {
                    return Outcome::Refused(format!(
                        "refused: the pull request #{number} of {branch} in {slug} is not \
                         merged: {blocks}; wait for its checks, or have it waived by the user \
                         (waive, with their words)"
                    ));
                }
            }
            match gh_text(&["pr", "merge", &number, "-R", &repo, "--squash"]).await {
                Ok(_) => Outcome::Done(format!(
                    "The pull request #{number} of {branch} was merged by squash into {base} in \
                     {slug}."
                )),
                Err(why) => Outcome::Failed(why),
            }
        }
        "pipeline-rerun" => {
            let runs = match gh_json(&[
                "run",
                "list",
                "-R",
                &repo,
                "--branch",
                branch,
                "--limit",
                "50",
                "--json",
                "databaseId,headSha,conclusion,workflowName",
            ])
            .await
            {
                Ok(runs) => runs,
                Err(why) => return Outcome::Failed(why),
            };
            let runs = runs.as_array().cloned().unwrap_or_default();
            // The runs of the branch's last commit, the newest listed first.
            let head = runs.first().and_then(|run| run["headSha"].as_str());
            let failed: Vec<&Value> = runs
                .iter()
                .filter(|run| run["headSha"].as_str() == head && run["conclusion"] == "failure")
                .collect();
            if failed.is_empty() {
                return Outcome::Refused(format!(
                    "refused: no pipeline of {branch}'s last commit failed in {slug}: there is \
                     nothing to re-run"
                ));
            }
            let mut done = Vec::new();
            for run in failed {
                let id = run["databaseId"].as_u64().unwrap_or(0).to_string();
                if let Err(why) = gh_text(&["run", "rerun", &id, "-R", &repo, "--failed"]).await {
                    return Outcome::Failed(why);
                }
                done.push(format!(
                    "{} ({id})",
                    masked(run["workflowName"].as_str().unwrap_or("pipeline"))
                ));
            }
            Outcome::Done(format!(
                "The failed jobs of {} were started again in {slug}.",
                done.join(", ")
            ))
        }
        action => Outcome::Refused(format!(
            "refused: forge_write has no action {action:?}: its actions are pr-create, \
             pr-merge-squash and pipeline-rerun"
        )),
    }
}

/// `gh <args>`'s output, masked, or a sentence saying why it failed.
async fn gh_text(args: &[&str]) -> Result<String, String> {
    let shown = args[..2].join(" ");
    match gh(args).await {
        Ok((true, stdout, _)) => Ok(masked(&stdout)),
        Ok((false, _, stderr)) => {
            let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            Err(format!(
                "gh {shown} failed: {}",
                masked(&said.chars().take(300).collect::<String>())
            ))
        }
        Err(Gh::Missing) => Err("gh is not on the daemon's PATH".into()),
        Err(Gh::Failed(why)) => Err(format!("gh {shown}: {why}")),
    }
}

/// What `forge_read` answers: the texts of the comments and reviews only when `full`, otherwise
/// their number and the way to get them, for the `repository` that was asked when one was.
fn read_text(workspace: &Workspace, read: &Read, full: bool, asked: Option<&str>) -> String {
    let again = match asked {
        Some(repository) => format!(
            "{{\"repository\": {}, \"detail\": \"full\"}}",
            Value::from(repository)
        ),
        None => "{\"detail\": \"full\"}".to_owned(),
    };
    let mut text = format!(
        "The pull requests of workspace {}, branch {}:\n",
        workspace.name, workspace.branch
    );
    if let Some(problem) = &read.problem {
        text.push_str(&format!("{problem}.\n"));
    }
    for repository in &read.repositories {
        let named = match &repository.forge {
            Some(forge) => format!("{} ({})", repository.path, forge.slug()),
            None => repository.path.clone(),
        };
        if let Some(note) = &repository.note {
            text.push_str(&format!("\n{named}: {note}.\n"));
            continue;
        }
        if let Some(error) = &repository.error {
            text.push_str(&format!(
                "\n{named}: the forge could not be read: {error}\n"
            ));
            continue;
        }
        if repository.blocked {
            continue;
        }
        let Some(pull) = &repository.pull else {
            text.push_str(&format!(
                "\n{named}: no pull request for {}.\n",
                workspace.branch
            ));
            continue;
        };
        text.push_str(&format!(
            "\n{named}: pull request #{}, {}, into {}: {}\n  {}\n",
            pull.number, pull.state, pull.base, pull.title, pull.url
        ));
        if let Some(review) = pull.review {
            text.push_str(&format!("  review: {review}\n"));
        }
        if !pull.reviewers.is_empty() {
            let each: Vec<String> = pull
                .reviewers
                .iter()
                .map(|(name, state)| format!("{name} {state}"))
                .collect();
            text.push_str(&format!("  reviewers: {}\n", each.join(", ")));
        }
        match pull.mark() {
            None => text.push_str("  checks: none\n"),
            Some(mark) => {
                let each: Vec<String> = pull
                    .checks
                    .iter()
                    .map(|(name, state)| format!("{name} {state}"))
                    .collect();
                text.push_str(&format!("  checks: {mark} ({})\n", each.join(", ")));
            }
        }
        for (title, comments) in [("comments", &pull.comments), ("reviews", &pull.reviews)] {
            let said: Vec<&Comment> = comments.iter().filter(|c| !c.body.is_empty()).collect();
            if said.is_empty() {
                continue;
            }
            if !full {
                text.push_str(&format!(
                    "  {title}: {}, read them with forge_read {again}\n",
                    said.len()
                ));
                continue;
            }
            text.push_str(&format!("  {title} ({}):\n", said.len()));
            for comment in said {
                let state = comment
                    .state
                    .as_deref()
                    .map_or(String::new(), |s| format!(", {s}"));
                let body: String = comment.body.chars().take(COMMENT_CHARS).collect();
                text.push_str(&format!(
                    "  - {}{state}: {}\n",
                    comment.author,
                    body.replace('\n', "\n    ")
                ));
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_url_keeps_no_credentials() {
        for (url, kept) in [
            (
                "https://someone:ghp_secret@github.com/acme/app.git",
                "https://github.com/acme/app.git",
            ),
            (
                "https://ghp_secret@github.com/acme/app.git",
                "https://github.com/acme/app.git",
            ),
            (
                "ssh://git@github.com/acme/app.git",
                "ssh://github.com/acme/app.git",
            ),
            ("git@github.com:acme/app.git", "git@github.com:acme/app.git"),
            ("/srv/git/app.git", "/srv/git/app.git"),
        ] {
            assert_eq!(without_credentials(url), kept, "{url}");
        }
    }

    #[test]
    fn the_checks_make_one_mark() {
        let pull = |checks: Value| {
            Pull::of(&json!({"number": 1, "state": "OPEN", "statusCheckRollup": checks})).unwrap()
        };
        let run = |status: &str, conclusion: &str| json!({"__typename": "CheckRun", "name": "build", "status": status, "conclusion": conclusion});
        let status =
            |state: &str| json!({"__typename": "StatusContext", "context": "ci", "state": state});
        assert_eq!(pull(json!([])).mark(), None);
        assert_eq!(
            pull(json!([
                run("COMPLETED", "SUCCESS"),
                run("COMPLETED", "SKIPPED")
            ]))
            .mark(),
            Some("passing")
        );
        assert_eq!(
            pull(json!([run("COMPLETED", "SUCCESS"), run("IN_PROGRESS", "")])).mark(),
            Some("pending")
        );
        assert_eq!(
            pull(json!([run("IN_PROGRESS", ""), status("ERROR")])).mark(),
            Some("failing")
        );
        assert_eq!(
            pull(json!([status("SUCCESS")])).checks,
            [("ci".to_owned(), "passing")]
        );
    }

    #[test]
    fn a_pull_request_s_state_says_draft_merged_and_closed() {
        let state = |value: Value| Pull::of(&value).unwrap().state;
        assert_eq!(state(json!({"number": 1, "state": "OPEN"})), "open");
        assert_eq!(
            state(json!({"number": 1, "state": "OPEN", "isDraft": true})),
            "draft"
        );
        assert_eq!(state(json!({"number": 1, "state": "MERGED"})), "merged");
        assert_eq!(state(json!({"number": 1, "state": "CLOSED"})), "closed");
    }
}
