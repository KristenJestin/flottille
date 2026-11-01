//! Forges (`docs/forges.md`): what the forge of each repository of a workspace says of the
//! workspace's branch, its pull request and their checks, the pull requests of a repository, and
//! the writes of `forge` that a session sends (`docs/commands.md`). Through the user's own command line (`gh` for
//! GitHub, `bkt` for Bitbucket), spawned without a shell, with the user's own sign-in: Flottille
//! never reads, keeps nor prints a token, and of `gh auth status` it reads the exit code alone.
//!
//! A workspace is read on demand (`workspace.forge`, the agents' `forge show`), and again about
//! once a minute while a client watches it or an agent subscribes to its pull request
//! (`events`), less often while `gh` or `bkt` fails. Nothing is read
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
use crate::mcp::{Outcome, arguments, full_detail, workspace_named};
use crate::place::Place;
use crate::projects::{self, Failure};

mod bitbucket;
mod ci;
mod events;
mod logs;

pub use events::{EVENTS, target_named};

/// How long a state read stays fresh, and how often a watched workspace is read again.
pub const EVERY_MS: u64 = 60_000;
/// The longest wait between two reads of a watched workspace whose reads keep failing.
const LONGEST_MS: u64 = 16 * 60_000;
/// A call of `gh` is cut after this.
const GH_TIME: Duration = Duration::from_secs(30);
/// A call of `ssh -G` is cut after this.
const SSH_TIME: Duration = Duration::from_secs(5);
/// What `forge show` shows of one comment at most.
const COMMENT_CHARS: usize = 2000;
/// What `gh pr view` is asked for.
const FIELDS: &str = "number,title,state,isDraft,headRefName,headRefOid,baseRefName,url,\
    mergeable,reviewDecision,statusCheckRollup,comments,reviews";
/// The ways `forge` `merge` merges a pull request, the first by default.
pub const MERGE_METHODS: [&str; 3] = ["squash", "merge", "rebase"];

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

pub use flottille_core::declared::NO_FORGE;

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

/// Where a repository's remote leads, as a workspace records it and a push and `forge`'s writes
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

/// A commit a forge said, kept only as a hash: it is given to git as a revision, never anything
/// that could read as an option.
fn commit_hash(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|oid| oid.len() >= 7 && oid.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_owned)
}

/// A pull request as the forge said it, its texts masked.
#[derive(Clone, Debug, PartialEq)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: &'static str,
    /// The branch it merges.
    pub head: String,
    /// The commit its branch was at, as a hash, when the forge says it.
    pub head_commit: Option<String>,
    pub base: String,
    pub url: String,
    /// Whether it merges into its base without conflict; none while the forge does not know.
    pub mergeable: Option<bool>,
    /// `approved`, `changes requested`, `review required`, or none.
    pub review: Option<&'static str>,
    /// Each check's name and state: `passing`, `failing`, `pending` or `skipped`.
    pub checks: Vec<(String, &'static str)>,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Comment>,
    /// The reviewers asked and their verdict (`approved`, `changes requested`, `pending`), of
    /// the forges that say them (Bitbucket); empty for GitHub, whose reviews say it.
    pub reviewers: Vec<(String, &'static str)>,
    /// The failing checks, whose log `forge show` gives (`logs::attach`).
    pub jobs: Vec<Job>,
    /// How long its repository's CI usually takes, when the forge says it (`ci::attach`).
    pub ci: Option<ci::Runs>,
}

/// A failing check of a pull request.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub name: String,
    /// The link the forge gives for the check, masked.
    pub details: Option<String>,
    /// The end of its log, or why there is none; not read yet while none.
    pub log: Option<Result<Log, String>>,
}

/// The end of a job's log: `shown` of its `total` lines, cleaned and masked.
#[derive(Clone, Debug, PartialEq)]
pub struct Log {
    pub text: String,
    pub shown: usize,
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub author: String,
    /// A review's verdict; none for a comment.
    pub state: Option<String>,
    pub body: String,
    /// Written by a bot (an app), as far as the forge says it.
    pub bot: bool,
}

impl Comment {
    /// The text, without the mark of Flottille, and whether an agent of Flottille posted it.
    pub fn unmarked(&self) -> (&str, bool) {
        unmarked(&self.body)
    }
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

    /// What blocks its merge: no check, its checks failing or still running, or a review asking
    /// for changes; none when nothing does.
    pub fn blocks(&self) -> Option<String> {
        blocks(&self.checks, self.review)
    }

    fn to_json(&self) -> Value {
        let mut pull = json!({
            "number": self.number,
            "title": self.title,
            "state": self.state,
            "head": self.head,
            "base": self.base,
            "mergeable": self.mergeable,
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
                    bot: is_bot(&comment["author"]),
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
            head: text(&value["headRefName"]),
            head_commit: commit_hash(&value["headRefOid"]),
            base: text(&value["baseRefName"]),
            url: text(&value["url"]),
            mergeable: match value["mergeable"].as_str() {
                Some("MERGEABLE") => Some(true),
                Some("CONFLICTING") => Some(false),
                _ => None,
            },
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
            jobs: value["statusCheckRollup"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|check| check_of(check).1 == "failing")
                .map(|check| Job {
                    name: check_of(check).0,
                    details: check["detailsUrl"]
                        .as_str()
                        .or(check["targetUrl"].as_str())
                        .filter(|link| !link.is_empty())
                        .map(masked),
                    log: None,
                })
                .collect(),
            ci: None,
        })
    }
}

/// Whether a GitHub author is a bot: `gh` says an app's login as `app/<name>`, and GitHub names
/// many `<name>[bot]`; `is_bot` when it says so.
fn is_bot(author: &Value) -> bool {
    let login = author["login"].as_str().unwrap_or_default();
    author["is_bot"] == true || login.starts_with("app/") || login.ends_with("[bot]")
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

/// What ends every comment and body Flottille posts for an agent on GitHub and Bitbucket: an HTML
/// comment, which both forges leave out of what they render. It goes out through the user's own
/// `gh` or `bkt`, so it is the only way to tell an agent's words from the user's.
pub(crate) const MARKER: &str = "<!-- flottille -->";
/// The same on Jira, where nothing invisible survives `acli`: a short visible line.
pub(crate) const FOOTER: &str = "— posted by a Flottille agent";
/// How a text posted by an agent is said where it is shown to another.
pub(crate) const BY_AGENT: &str = "posted by an agent";

/// `body`, ended with `end` on a line of its own (`end` alone when there is no body).
fn ended_with(body: &str, end: &str) -> String {
    match body.trim_end() {
        "" => end.to_owned(),
        body => format!("{body}\n\n{end}"),
    }
}

/// What an agent posts on GitHub or Bitbucket: `body`, marked.
pub(crate) fn marked(body: &str) -> String {
    ended_with(body, MARKER)
}

/// What an agent posts on Jira: `body`, with the footer.
pub(crate) fn footed(body: &str) -> String {
    ended_with(body, FOOTER)
}

/// A text read from a forge or a tracker, without the mark of Flottille, and whether it had one:
/// an agent of Flottille posted it. A person's comment that ends with the mark is taken for an
/// agent's, which is accepted.
pub(crate) fn unmarked(text: &str) -> (&str, bool) {
    let text = text.trim_end();
    for end in [MARKER, FOOTER] {
        if let Some(body) = text.strip_suffix(end) {
            return (body.trim_end(), true);
        }
    }
    (text, false)
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
    /// The watches the subscriptions hold, by subscription.
    held: Mutex<HashMap<String, Watch>>,
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
pub async fn state(daemon: &Arc<Daemon>, workspace: &Workspace) -> Value {
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

/// Reads the workspace's forges now and keeps what was read; an event says when it changed, and
/// the subscriptions to its pull requests are offered what happened.
async fn refresh(daemon: &Arc<Daemon>, workspace: &Workspace) -> Value {
    refresh_read(daemon, workspace).await.0
}

/// `refresh`, with what was read.
async fn refresh_read(daemon: &Arc<Daemon>, workspace: &Workspace) -> (Value, Read) {
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
    events::offer(daemon, workspace, &read).await;
    (state, read)
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

/// A subscription's watch of its workspace, held until `release`; false when the workspace is
/// gone.
pub fn hold(
    daemon: &Arc<Daemon>,
    subscription: &flottille_core::subscription::Subscription,
) -> bool {
    let target = &subscription.target;
    let (Some(project), Some(name)) = (target["project"].as_str(), target["workspace"].as_str())
    else {
        return false;
    };
    let Ok(Some(workspace)) = daemon.read(|store| store.workspace(project, name)) else {
        return false;
    };
    let watch = watch(daemon, &workspace);
    let replaced = daemon
        .forges
        .held
        .lock()
        .unwrap()
        .insert(subscription.id.clone(), watch);
    drop(replaced);
    true
}

/// Drops the watch a subscription held.
pub fn release(daemon: &Daemon, subscription: &str) {
    let held = daemon.forges.held.lock().unwrap().remove(subscription);
    drop(held);
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

/// A call of `forge`: its action and what the action takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForgeCall {
    action: String,
    workspace: Option<String>,
    repository: Option<String>,
    number: Option<u64>,
    state: Option<String>,
    branch: Option<String>,
    detail: Option<String>,
    title: Option<String>,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    /// merge: why the user lets a pull request whose checks or review block it be merged; the
    /// gate held the call for them.
    waive: Option<String>,
    /// merge: `squash` (the default), `merge` or `rebase`; the gate refused any other.
    method: Option<String>,
    /// close, reopen: posted on the pull request first.
    comment: Option<String>,
    /// close: also delete the pull request's branch on the forge.
    #[serde(default)]
    delete_branch: bool,
    /// subscribe: the events it waits for (`EVENTS`).
    events: Option<Vec<String>>,
    /// subscribe: whose reviews and comments count (`subscriptions::FROM`).
    from: Option<String>,
    /// subscribe: how long it waits at most.
    minutes: Option<u64>,
}

impl ForgeCall {
    fn method(&self) -> &str {
        self.method.as_deref().unwrap_or(MERGE_METHODS[0])
    }
}

/// The refusal of a merge by `method` that the repository does not allow, naming those it does,
/// and the forge's own strategies that no method reaches, as the forge calls them.
fn method_refused(slug: &str, method: &str, allowed: &[&str], others: &[String]) -> Outcome {
    fn listed<S: AsRef<str>>(words: &[S]) -> String {
        match words {
            [] => String::new(),
            [one] => one.as_ref().to_owned(),
            [rest @ .., last] => format!(
                "{} and {}",
                rest.iter()
                    .map(AsRef::as_ref)
                    .collect::<Vec<_>>()
                    .join(", "),
                last.as_ref()
            ),
        }
    }
    let own = format!(
        "its own strategies {} (no method reaches them)",
        listed(others)
    );
    let allowed = match (allowed.is_empty(), others.is_empty()) {
        (true, true) => "none of squash, merge and rebase".to_owned(),
        (true, false) => format!("none of squash, merge and rebase, only {own}"),
        (false, true) => format!("{} (method)", listed(allowed)),
        (false, false) => format!("{} (method), and {own}", listed(allowed)),
    };
    Outcome::Refused(format!(
        "refused: {slug} does not allow merging by {method}: it allows {allowed}; nothing was \
         merged"
    ))
}

/// `forge`, once the gate let it through: the pull requests of the project's repositories, read
/// and written through the user's own `gh` or `bkt`. Never a repository outside the agent's
/// place.
pub async fn forge(daemon: &Arc<Daemon>, agent: &str, place: &Place, args: Value) -> Outcome {
    let asked: ForgeCall = match arguments("forge", args) {
        Ok(asked) => asked,
        Err(refusal) => return refusal,
    };
    match (asked.action.as_str(), asked.number) {
        ("list", _) => forge_list(daemon, place, asked).await,
        ("show", number) => {
            let shown = match number {
                None => forge_read(daemon, place, asked).await,
                Some(number) => forge_show(daemon, place, asked, number).await,
            };
            match shown {
                Outcome::Done(text) => Outcome::Done(text + &subscriptions_text(daemon, agent)),
                other => other,
            }
        }
        ("subscribe", _) => forge_subscribe(daemon, agent, place, asked).await,
        ("unsubscribe", _) => forge_unsubscribe(daemon, agent, asked),
        ("create" | "merge" | "checks", _) => forge_write(daemon, place, asked).await,
        ("comment" | "edit", _) => forge_note(daemon, place, asked).await,
        ("close" | "reopen", _) => forge_close(daemon, place, asked).await,
        (action, _) => Outcome::Refused(format!(
            "refused: forge has no action {action:?}: its actions are {}",
            crate::gate::FORGE_ACTIONS
        )),
    }
}

/// What `forge show` ends with: the agent's subscriptions, when it has some.
fn subscriptions_text(daemon: &Daemon, agent: &str) -> String {
    let mine: Vec<_> = crate::subscriptions::of(daemon, agent)
        .into_iter()
        .filter(|s| s.source == "forge")
        .collect();
    if mine.is_empty() {
        return String::new();
    }
    let now = now_ms();
    let mut text = "\nYour subscriptions:\n".to_owned();
    for subscription in mine {
        let from = match subscription.from.as_str() {
            "people" => " (from people)",
            _ => "",
        };
        text.push_str(&format!(
            "- {}: {}{from}, for {} min more\n",
            target_named(&subscription.target),
            subscription.events.join(", "),
            subscription.expires_at.saturating_sub(now).div_ceil(60_000)
        ));
    }
    text
}

/// `forge subscribe`: the agent waits, once, for the next of the events it names of the pull
/// request of a workspace's branch; the answer comes at once, the event later as a delivery. What
/// is already so is not an event: this read is what the next ones are compared with.
async fn forge_subscribe(
    daemon: &Arc<Daemon>,
    agent: &str,
    place: &Place,
    asked: ForgeCall,
) -> Outcome {
    let all = EVENTS.join(", ");
    let events = asked.events.clone().unwrap_or_default();
    if events.is_empty() {
        return Outcome::Refused(format!(
            "refused: forge subscribe needs events, among {all}"
        ));
    }
    if let Some(other) = events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
        return Outcome::Refused(format!(
            "refused: a pull request has no event {other:?}: its events are {all}"
        ));
    }
    let from = asked.from.as_deref().unwrap_or("anyone");
    if !crate::subscriptions::FROM.contains(&from) {
        return Outcome::Refused(format!(
            "refused: from is anyone (the default) or people (no bot, not your own), not {from:?}"
        ));
    }
    let longest = crate::subscriptions::LONGEST_MINUTES;
    if let Some(minutes) = asked.minutes
        && !(1..=longest).contains(&minutes)
    {
        return Outcome::Refused(format!(
            "refused: minutes is from 1 to {longest}, 60 by default"
        ));
    }
    if asked.number.is_some() {
        return Outcome::Refused(
            "refused: forge subscribe takes no number: it waits on the pull request of a \
             workspace's branch (workspace, repository)"
                .into(),
        );
    }
    let name = match workspace_named(daemon, place, asked.workspace.as_deref(), "forge") {
        Ok(name) => name,
        Err(refusal) => return refusal,
    };
    let workspace = match daemon.read(|store| store.workspace(&place.project, &name)) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return Outcome::Failed(format!("the workspace {name} is gone")),
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let project = projects::load_project(daemon, &place.project).ok();
    let path = match chosen(
        repositories_of(&workspace, project.as_ref()),
        asked.repository.as_deref(),
        "subscribe",
        true,
    ) {
        Ok(mut checkouts) => checkouts.remove(0).path,
        Err(refusal) => return refusal,
    };
    let (_, read) = refresh_read(daemon, &workspace).await;
    let Some(repository) = read.repositories.iter().find(|r| r.path == path) else {
        return Outcome::Failed(format!("{path} was not read"));
    };
    if let Some(note) = &repository.note {
        return Outcome::Refused(format!("refused: {path}: {note}"));
    }
    if let Some(error) = &repository.error {
        return Outcome::Failed(format!("{path}: the forge could not be read: {error}"));
    }
    if let (true, Some(problem)) = (repository.blocked, &read.problem) {
        return Outcome::Failed(problem.clone());
    }
    let Some(pull) = &repository.pull else {
        return Outcome::Refused(format!(
            "refused: {path} has no pull request for {}: nothing to subscribe to",
            workspace.branch
        ));
    };
    if matches!(pull.state, "merged" | "closed") {
        return Outcome::Refused(format!(
            "refused: the pull request #{} of {path} is {}: nothing more happens to it",
            pull.number, pull.state
        ));
    }
    // The one read of the runs: it tells the agent, and sizes the wait it did not size.
    let runs = match &repository.forge {
        Some(forge) => ci::recent(forge).await,
        None => None,
    };
    let (minutes, duration) = match asked.minutes {
        Some(minutes) => (minutes, None),
        None => {
            let (minutes, why) = ci::wait_for(runs.as_ref());
            (minutes, Some(why))
        }
    };
    let target = events::target(&workspace, &path, pull.number);
    for old in crate::subscriptions::of(daemon, agent) {
        if old.target == target {
            crate::subscriptions::end(daemon, &old, "cancelled");
        }
    }
    let now = now_ms();
    let subscription = flottille_core::subscription::Subscription {
        id: crate::subscriptions::new_subscription_id(),
        agent: agent.to_owned(),
        source: "forge".into(),
        target,
        events,
        from: from.to_owned(),
        seen: events::seen_of(pull),
        created_at: now,
        expires_at: now + minutes * 60_000,
    };
    if let Err(error) = crate::subscriptions::subscribe(daemon, &subscription) {
        return Outcome::Failed(error);
    }
    if !hold(daemon, &subscription) {
        crate::subscriptions::end(daemon, &subscription, "unavailable");
        return Outcome::Failed(format!("the workspace {name} is gone"));
    }
    let from = match from {
        "people" => ", from people",
        _ => "",
    };
    let mut text = format!(
        "Subscribed: the next {}{from} of the {} wakes you once, as a delivery, within {minutes} \
         min. What is so now is no event: forge show reads it.",
        subscription.events.join(" or "),
        target_named(&subscription.target)
    );
    for line in runs.map(|runs| runs.line()).into_iter().chain(duration) {
        text.push_str(&format!("\n{line}"));
    }
    Outcome::Done(text)
}

/// `forge unsubscribe`: the agent's subscriptions to the workspace and repository it names, every
/// one when it names none.
fn forge_unsubscribe(daemon: &Daemon, agent: &str, asked: ForgeCall) -> Outcome {
    let named =
        |s: &flottille_core::subscription::Subscription, key: &str, asked: &Option<String>| {
            asked.as_ref().is_none_or(|a| s.target[key] == a.as_str())
        };
    let cancelled: Vec<_> = crate::subscriptions::of(daemon, agent)
        .into_iter()
        .filter(|s| s.source == "forge")
        .filter(|s| named(s, "workspace", &asked.workspace))
        .filter(|s| named(s, "repository", &asked.repository))
        .collect();
    if cancelled.is_empty() {
        return Outcome::Refused("refused: you have no such subscription".into());
    }
    let mut text = "Cancelled your subscriptions:\n".to_owned();
    for subscription in cancelled {
        crate::subscriptions::end(daemon, &subscription, "cancelled");
        text.push_str(&format!("- {}\n", target_named(&subscription.target)));
    }
    Outcome::Done(text)
}

/// The repositories of the agent's place as a forge is read for them: its workspace's, or the
/// project's in `main`, each with its declared forge.
fn place_checkouts(daemon: &Daemon, place: &Place) -> Result<Vec<Checkout>, Outcome> {
    let project = projects::load_project(daemon, &place.project)
        .map_err(|failure| Outcome::Failed(failure.message))?;
    let Some(name) = &place.workspace else {
        return Ok(project
            .repositories
            .iter()
            .map(|r| Checkout {
                path: r.path.clone(),
                folder: match r.path.as_str() {
                    "." => project.folder.clone(),
                    path => project.folder.join(path),
                },
                remote: r.remote.clone(),
                declared: r.forge.clone(),
            })
            .collect());
    };
    match daemon.read(|store| store.workspace(&place.project, name)) {
        Ok(Some(workspace)) => Ok(repositories_of(&workspace, Some(&project))),
        Ok(None) => Err(Outcome::Failed(format!("the workspace {name} is gone"))),
        Err(error) => Err(Outcome::Failed(error.to_string())),
    }
}

/// The repositories a call acts on: the one it names, or every one when it names none and `one`
/// is not needed.
fn chosen(
    checkouts: Vec<Checkout>,
    asked: Option<&str>,
    action: &str,
    one: bool,
) -> Result<Vec<Checkout>, Outcome> {
    let names = || {
        let names: Vec<&str> = checkouts.iter().map(|r| r.path.as_str()).collect();
        names.join(", ")
    };
    match asked {
        Some(asked) if checkouts.iter().any(|r| r.path == asked) => {
            Ok(checkouts.into_iter().filter(|r| r.path == asked).collect())
        }
        Some(asked) => Err(Outcome::Refused(format!(
            "refused: {asked} is not a repository of this agent's place: it has {}; forge acts \
             only on those",
            names()
        ))),
        None if one && checkouts.len() != 1 => Err(Outcome::Refused(format!(
            "refused: forge {action} needs repository, one of {}",
            names()
        ))),
        None => Ok(checkouts),
    }
}

/// A pull request of a list: enough to name it and pick it.
pub(crate) struct Listed {
    pub number: u64,
    pub title: String,
    pub state: &'static str,
    pub branch: String,
    pub base: String,
    pub url: String,
}

/// `forge list`: the pull requests of the place's repositories, by state and source branch.
async fn forge_list(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    let state = asked.state.as_deref().unwrap_or("open");
    if !["open", "closed", "merged", "all"].contains(&state) {
        return Outcome::Refused(format!(
            "refused: forge list has no state {state:?}: its states are open (the default), \
             closed, merged and all"
        ));
    }
    let checkouts = match place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), "list", false))
    {
        Ok(checkouts) => checkouts,
        Err(refusal) => return refusal,
    };
    let branch = asked.branch.as_deref();
    let mut text = match branch {
        Some(branch) => format!("The {state} pull requests of {branch}:\n"),
        None => format!("The {state} pull requests:\n"),
    };
    let mut contexts = bitbucket::Contexts::default();
    for checkout in &checkouts {
        let listed = match declared_forge(checkout).await {
            Err(None) => Err(NO_FORGE.to_owned()),
            Err(Some(why)) => Err(why),
            Ok(forge @ Forge::GitHub { .. }) => match signed_in().await {
                Err(problem) => Err(problem),
                Ok(()) => gh_list(&forge, state, branch)
                    .await
                    .map(|pulls| (pulls, false)),
            },
            Ok(forge @ Forge::Bitbucket { .. }) => match contexts.of(&forge).await {
                Err(problem) => Err(problem),
                Ok(context) => bitbucket::pulls_in(&forge, &context, state, branch).await,
            },
        };
        match listed {
            Err(why) => text.push_str(&format!("\n{}: {why}.\n", checkout.path)),
            Ok((pulls, cut)) => {
                if pulls.is_empty() {
                    text.push_str(&format!("\n{}: none.\n", checkout.path));
                } else {
                    text.push_str(&format!("\n{} ({}):\n", checkout.path, pulls.len()));
                }
                for pull in pulls {
                    text.push_str(&format!(
                        "- #{} {}, {} into {}: {}  {}\n",
                        pull.number, pull.state, pull.branch, pull.base, pull.title, pull.url
                    ));
                }
                // `bkt` lists one page, and the repository may hold more pull requests than it.
                if cut {
                    text.push_str(&format!(
                        "Only the newest {} pull requests are read; older ones, if any, are not \
                         listed.\n",
                        bitbucket::PAGE
                    ));
                }
            }
        }
    }
    text.push_str("\nforge {\"action\": \"show\", \"repository\": …, \"number\": …} reads one.");
    Outcome::Done(text)
}

/// How many pull requests one list asks `gh` for.
const LISTED: &str = "100";

/// `gh pr list` of the repository, in `state`, of `branch` when given.
async fn gh_list(forge: &Forge, state: &str, branch: Option<&str>) -> Result<Vec<Listed>, String> {
    let repo = forge.gh_repository();
    let mut args = vec![
        "pr", "list", "-R", &repo, "--state", state, "--limit", LISTED,
    ];
    if let Some(branch) = branch {
        args.extend(["--head", branch]);
    }
    args.extend([
        "--json",
        "number,title,state,isDraft,headRefName,baseRefName,url",
    ]);
    let listed = gh_json(&args).await?;
    Ok(listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pull| {
            let text = |key: &str| masked(pull[key].as_str().unwrap_or_default());
            Some(Listed {
                number: pull["number"].as_u64()?,
                title: text("title"),
                state: Pull::of(pull)?.state,
                branch: text("headRefName"),
                base: text("baseRefName"),
                url: text("url"),
            })
        })
        .collect())
}

/// `forge show` with a number: that pull request of one repository of the place, its checks,
/// review and comments.
async fn forge_show(daemon: &Daemon, place: &Place, asked: ForgeCall, number: u64) -> Outcome {
    let full = match full_detail("forge", asked.detail.as_deref()) {
        Ok(full) => full,
        Err(refusal) => return refusal,
    };
    let checkout = match place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), "show", true))
    {
        Ok(mut checkouts) => checkouts.remove(0),
        Err(refusal) => return refusal,
    };
    let forge = match declared_forge(&checkout).await {
        Ok(forge) => forge,
        Err(None) => return Outcome::Refused(format!("refused: {}: {NO_FORGE}", checkout.path)),
        Err(Some(why)) => return Outcome::Refused(why),
    };
    let pull = match &forge {
        Forge::GitHub { .. } => match signed_in().await {
            Err(problem) => Err(problem),
            Ok(()) => {
                let repo = forge.gh_repository();
                let number = number.to_string();
                gh_json(&["pr", "view", &number, "-R", &repo, "--json", FIELDS])
                    .await
                    .map(|viewed| Pull::of(&viewed))
            }
        },
        Forge::Bitbucket { .. } => match bitbucket::Contexts::default().of(&forge).await {
            Err(problem) => Err(problem),
            Ok(context) => bitbucket::pull_numbered(&forge, &context, number).await,
        },
    };
    match pull {
        Err(why) => Outcome::Failed(why),
        Ok(None) => Outcome::Refused(format!(
            "refused: {} has no pull request #{number}",
            forge.slug()
        )),
        Ok(Some(mut pull)) => {
            logs::attach(&forge, &mut pull).await;
            ci::attach(&forge, &mut pull).await;
            let again = format!(
                "{{\"action\": \"show\", \"repository\": {}, \"number\": {number}, \
                 \"detail\": \"full\"}}",
                Value::from(checkout.path.as_str())
            );
            let mut text = String::new();
            pull_text(
                &mut text,
                &format!("{} ({})", checkout.path, forge.slug()),
                &pull,
                full,
                &again,
            );
            Outcome::Done(text.trim_start().to_owned())
        }
    }
}

/// `forge comment` and `forge edit`: a comment on a pull request of one repository of the place,
/// or a new title or description of it.
async fn forge_note(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    let action = asked.action.as_str();
    let Some(number) = asked.number else {
        return Outcome::Refused(format!(
            "refused: forge {action} needs number, the pull request's: forge list lists them"
        ));
    };
    let body = asked.body.as_deref().filter(|b| !b.trim().is_empty());
    let title = asked.title.as_deref().filter(|t| !t.trim().is_empty());
    match (action, body, title) {
        ("comment", None, _) => {
            return Outcome::Refused("refused: forge comment needs body, the comment".into());
        }
        ("edit", None, None) => {
            return Outcome::Refused(
                "refused: forge edit needs title, body or both: what changes".into(),
            );
        }
        _ => {}
    }
    let checkout = match place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), action, true))
    {
        Ok(mut checkouts) => checkouts.remove(0),
        Err(refusal) => return refusal,
    };
    let forge = match declared_forge(&checkout).await {
        Ok(forge) => forge,
        Err(None) => {
            return Outcome::Refused(format!(
                "refused: {}: {NO_FORGE}; nothing was written: ask the user to set it",
                checkout.path
            ));
        }
        Err(Some(why)) => return Outcome::Refused(why),
    };
    let slug = forge.slug();
    let number = number.to_string();
    let marked_body = body.map(marked);
    let body = marked_body.as_deref();
    let done = match &forge {
        Forge::Bitbucket { .. } => bitbucket::note(&forge, action, &number, title, body).await,
        Forge::GitHub { .. } => {
            if let Err(problem) = signed_in().await {
                return Outcome::Failed(problem);
            }
            let repo = forge.gh_repository();
            let mut args = vec!["pr", action, &number, "-R", &repo];
            // `gh pr comment` takes no title, and the gate refused one there.
            if let Some(title) = title.filter(|_| action == "edit") {
                args.extend(["--title", title]);
            }
            if let Some(body) = body {
                args.extend(["--body", body]);
            }
            gh_text(&args).await.map(|_| ())
        }
    };
    match (done, action) {
        (Err(why), _) => Outcome::Failed(why),
        (Ok(()), "comment") => Outcome::Done(format!(
            "The comment was posted on pull request #{number} of {slug}."
        )),
        (Ok(()), _) => Outcome::Done(format!("Pull request #{number} of {slug} was changed.")),
    }
}

/// What `forge close` and `reopen` read of a pull request before they act.
pub(crate) struct Seen {
    /// `open`, `merged` or `closed`.
    pub state: &'static str,
    pub branch: String,
    /// From the repository itself, not from a fork of it.
    pub own: bool,
}

/// `forge close` and `reopen` of a pull request of a repository of the place, by its number: only
/// an open one is closed and only a closed one reopened, the refusal saying its state. The
/// branch is deleted on request, unless a workspace is still on it.
async fn forge_close(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    let action = asked.action.as_str();
    let Some(number) = asked.number else {
        return Outcome::Refused(format!(
            "refused: forge {action} needs number, the pull request's: forge list lists them"
        ));
    };
    let checkout = match place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), action, true))
    {
        Ok(mut checkouts) => checkouts.remove(0),
        Err(refusal) => return refusal,
    };
    let forge = match declared_forge(&checkout).await {
        Ok(forge) => forge,
        Err(None) => {
            return Outcome::Refused(format!(
                "refused: {}: {NO_FORGE}; nothing was written: ask the user to set it",
                checkout.path
            ));
        }
        Err(Some(why)) => return Outcome::Refused(why),
    };
    let slug = forge.slug();
    let bitbucket = matches!(forge, Forge::Bitbucket { .. });
    if bitbucket && action == "reopen" {
        return Outcome::Refused(format!(
            "refused: Bitbucket cannot reopen a declined pull request: nothing was reopened; \
             open a new one for #{number} of {slug}"
        ));
    }
    let comment = asked
        .comment
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(marked);
    let comment = comment.as_deref();
    let numbered = number.to_string();
    let repo = forge.gh_repository();
    if !bitbucket && let Err(problem) = signed_in().await {
        return Outcome::Failed(problem);
    }
    let seen = if bitbucket {
        match bitbucket::seen(&forge, number).await {
            Ok(Some(seen)) => seen,
            Ok(None) => {
                return Outcome::Refused(format!(
                    "refused: {slug} has no pull request #{number}: forge list lists them"
                ));
            }
            Err(why) => return Outcome::Failed(why),
        }
    } else {
        let fields = "state,headRefName,isCrossRepository";
        match gh_json(&["pr", "view", &numbered, "-R", &repo, "--json", fields]).await {
            Ok(viewed) => Seen {
                state: match viewed["state"].as_str() {
                    Some("MERGED") => "merged",
                    Some("CLOSED") => "closed",
                    _ => "open",
                },
                branch: viewed["headRefName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                own: viewed["isCrossRepository"] != true,
            },
            Err(why) => return Outcome::Failed(why),
        }
    };
    let (wanted, other) = match action {
        "close" => ("open", "closed"),
        _ => ("closed", "reopened"),
    };
    if seen.state != wanted {
        return Outcome::Refused(format!(
            "refused: the pull request #{number} of {slug} is {}, not {wanted}; nothing was {other}",
            seen.state
        ));
    }
    // The branch of a pull request a workspace is still on is that workspace's.
    let on_it = daemon
        .read(|store| store.workspaces(Some(&place.project)))
        .unwrap_or_default()
        .into_iter()
        .find(|w| {
            w.branch == seen.branch && w.repositories.iter().any(|r| r.path == checkout.path)
        });
    let mut left = None;
    if asked.delete_branch {
        left = match (&on_it, seen.own) {
            (Some(workspace), _) => Some(format!("the workspace {} is on it", workspace.name)),
            (None, false) => Some("it comes from a fork".to_owned()),
            (None, true) => None,
        };
        if left.is_none() && bitbucket::cloud(&forge) {
            return Outcome::Refused(
                "refused: bkt cannot delete the source branch of a Bitbucket Cloud pull \
                 request it declines: decline it without delete_branch; nothing was declined"
                    .into(),
            );
        }
    }
    let delete = asked.delete_branch && left.is_none();
    let done = if bitbucket {
        bitbucket::decline(&forge, &numbered, comment, delete).await
    } else {
        let mut args = vec!["pr", action, &numbered, "-R", &repo];
        if let Some(comment) = comment {
            args.extend(["--comment", comment]);
        }
        if delete {
            args.push("--delete-branch");
        }
        gh_text(&args).await.map(|_| ())
    };
    if let Err(why) = done {
        return Outcome::Failed(why);
    }
    let verb = match (action, bitbucket) {
        ("close", true) => "declined",
        ("close", false) => "closed",
        _ => "reopened",
    };
    let branch = masked(&seen.branch);
    let tail = match (delete, left) {
        (true, _) => format!(" and its branch {branch} was deleted"),
        (false, Some(why)) => format!("; its branch {branch} was left: {why}"),
        (false, None) => String::new(),
    };
    Outcome::Done(format!(
        "The pull request #{number} of {slug} was {verb}{tail}."
    ))
}

/// `forge show` without a number: the pull requests of a workspace's branch, their checks and
/// their review comments. Never another repository.
async fn forge_read(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    let full = match full_detail("forge", asked.detail.as_deref()) {
        Ok(full) => full,
        Err(refusal) => return refusal,
    };
    let name = match workspace_named(daemon, place, asked.workspace.as_deref(), "forge") {
        Ok(name) => name,
        Err(refusal) => return refusal,
    };
    let workspace = match daemon.read(|store| store.workspace(&place.project, &name)) {
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
                 forge reads only those",
                names.join(", ")
            ));
        }
        repositories.retain(|r| &r.path == asked);
    }
    let mut read = read(&repositories, &workspace.branch, obsolete).await;
    for repository in &mut read.repositories {
        if let (Some(forge), Some(pull)) = (&repository.forge, &mut repository.pull) {
            logs::attach(forge, pull).await;
            ci::attach(forge, pull).await;
        }
    }
    Outcome::Done(read_text(
        &workspace,
        &read,
        full,
        asked.repository.as_deref(),
    ))
}

/// What blocks the merge of a pull request with these checks and this review; none when nothing
/// does. A pull request no check ran on, or whose checks were all skipped, is blocked as a failing
/// one is: nothing verified it.
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
    if checks.is_empty() {
        blocks.push("no check ran on this pull request".to_owned());
    } else if let Some(names) = named("skipped")
        && checks.iter().all(|(_, s)| *s == "skipped")
    {
        blocks.push(format!(
            "all its checks were skipped ({names}), none passed"
        ));
    }
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

/// `forge` `create`, `merge` and `checks`: one write to the forge of one repository of a
/// workspace, for the workspace's branch only, through the user's own `gh` or `bkt`.
async fn forge_write(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    let Some(asked_repository) = asked.repository.clone() else {
        return Outcome::Refused(format!(
            "refused: forge {} needs repository, one of the workspace's",
            asked.action
        ));
    };
    let name = match workspace_named(daemon, place, asked.workspace.as_deref(), "forge") {
        Ok(name) => name,
        Err(refusal) => return refusal,
    };
    let workspace = match daemon.read(|store| store.workspace(&place.project, &name)) {
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
        .find(|(checkout, _)| checkout.path == asked_repository);
    let Some((checkout, repository)) = found else {
        let names: Vec<&str> = repositories.iter().map(|r| r.path.as_str()).collect();
        return Outcome::Refused(format!(
            "refused: {asked_repository} is not a repository of the workspace {name}: it has \
             {}; forge writes only for those",
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
                "refused: {asked_repository}: {NO_FORGE}; nothing was written: ask the user to \
                 set it"
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
        "create" => {
            let Some(title) = asked.title.filter(|t| !t.trim().is_empty()) else {
                return Outcome::Refused(
                    "refused: create needs a title: say what the pull request does".into(),
                );
            };
            let base = &repository.base.branch;
            let body = marked(&asked.body.unwrap_or_default());
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
        "merge" => {
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
                // A pull request that cannot be read is one nothing verified.
                let blocked = match Pull::of(&viewed) {
                    Some(pull) => pull.blocks(),
                    None => Some("it could not be read, so nothing verified it".to_owned()),
                };
                if let Some(blocks) = blocked {
                    return Outcome::Refused(format!(
                        "refused: the pull request #{number} of {branch} in {slug} is not \
                         merged: {blocks}; wait for its checks, or have it waived by the user \
                         (waive, with their words)"
                    ));
                }
            }
            let method = asked.method();
            let flag = format!("--{method}");
            match gh_text(&["pr", "merge", &number, "-R", &repo, &flag]).await {
                Ok(_) => Outcome::Done(format!(
                    "The pull request #{number} of {branch} was merged by {method} into {base} \
                     in {slug}."
                )),
                Err(why) => {
                    // Refused by the forge: when the repository forbids the method, say which
                    // ones it allows rather than GitHub's own sentence.
                    let fields = "squashMergeAllowed,mergeCommitAllowed,rebaseMergeAllowed";
                    match gh_json(&["repo", "view", &repo, "--json", fields]).await {
                        Ok(allows) => {
                            let allowed: Vec<&str> = MERGE_METHODS
                                .into_iter()
                                .zip(fields.split(','))
                                .filter(|(_, field)| allows[*field] == true)
                                .map(|(method, _)| method)
                                .collect();
                            if allowed.contains(&method) {
                                Outcome::Failed(why)
                            } else {
                                method_refused(&slug, method, &allowed, &[])
                            }
                        }
                        Err(_) => Outcome::Failed(why),
                    }
                }
            }
        }
        "checks" => {
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
        action => Outcome::Refused(format!("refused: forge {action} writes nothing here")),
    }
}

/// `gh <args>`'s output, masked, or a sentence saying why it failed.
pub(crate) async fn gh_text(args: &[&str]) -> Result<String, String> {
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

/// What `forge show` answers for a workspace: the texts of the comments and reviews only when `full`, otherwise
/// their number and the way to get them, for the `repository` that was asked when one was.
fn read_text(workspace: &Workspace, read: &Read, full: bool, asked: Option<&str>) -> String {
    let again = match asked {
        Some(repository) => format!(
            "{{\"action\": \"show\", \"workspace\": {}, \"repository\": {}, \"detail\": \
             \"full\"}}",
            Value::from(workspace.name.as_str()),
            Value::from(repository)
        ),
        None => format!(
            "{{\"action\": \"show\", \"workspace\": {}, \"detail\": \"full\"}}",
            Value::from(workspace.name.as_str())
        ),
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
        pull_text(&mut text, &named, pull, full, &again);
    }
    text
}

/// A pull request, as `forge show` says it: the texts of its comments and reviews only when
/// `full`, otherwise their number and `again`, the call that gives them.
fn pull_text(text: &mut String, named: &str, pull: &Pull, full: bool, again: &str) {
    text.push_str(&format!(
        "\n{named}: pull request #{}, {}, {} into {}: {}\n  {}\n",
        pull.number, pull.state, pull.head, pull.base, pull.title, pull.url
    ));
    match pull.mergeable {
        Some(true) => text.push_str(&format!(
            "  mergeable: yes, no conflict with {}\n",
            pull.base
        )),
        Some(false) => text.push_str(&format!(
            "  mergeable: no, it conflicts with {}\n",
            pull.base
        )),
        None => text.push_str("  mergeable: not known yet\n"),
    }
    text.push_str(&format!("  review: {}\n", pull.review.unwrap_or("none")));
    if !pull.reviewers.is_empty() {
        let each: Vec<String> = pull
            .reviewers
            .iter()
            .map(|(name, state)| format!("{name} {state}"))
            .collect();
        text.push_str(&format!("  reviewers: {}\n", each.join(", ")));
    }
    match pull.mark() {
        None => text.push_str("  checks: none ran on this pull request\n"),
        Some(mark) => {
            let each: Vec<String> = pull
                .checks
                .iter()
                .map(|(name, state)| format!("{name} {state}"))
                .collect();
            text.push_str(&format!("  checks: {mark} ({})\n", each.join(", ")));
        }
    }
    text.push_str(&logs::text(&pull.jobs));
    if let Some(ci) = &pull.ci {
        text.push_str(&format!("  {}\n", ci.line()));
    }
    for (title, comments) in [("comments", &pull.comments), ("reviews", &pull.reviews)] {
        let said: Vec<&Comment> = comments.iter().filter(|c| !c.body.is_empty()).collect();
        if said.is_empty() {
            continue;
        }
        if !full {
            text.push_str(&format!(
                "  {title}: {}, read them with forge {again}\n",
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
            let (body, agent) = comment.unmarked();
            let body: String = body.chars().take(COMMENT_CHARS).collect();
            let agent = if agent {
                format!(" ({BY_AGENT})")
            } else {
                String::new()
            };
            text.push_str(&format!(
                "  - {}{agent}{state}: {}\n",
                comment.author,
                body.replace('\n', "\n    ")
            ));
        }
    }
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
    fn nothing_that_passed_blocks_the_merge_as_much_as_a_failure() {
        let blocked = |checks: &[(&str, &'static str)]| {
            let checks: Vec<(String, &'static str)> =
                checks.iter().map(|(n, s)| ((*n).to_owned(), *s)).collect();
            blocks(&checks, None)
        };
        assert_eq!(
            blocked(&[]).as_deref(),
            Some("no check ran on this pull request")
        );
        assert_eq!(
            blocked(&[("build", "skipped"), ("lint", "skipped")]).as_deref(),
            Some("all its checks were skipped (build, lint), none passed")
        );
        assert_eq!(blocked(&[("build", "passing"), ("lint", "skipped")]), None);
        assert_eq!(
            blocked(&[("build", "skipped"), ("lint", "failing")]).as_deref(),
            Some("its checks fail (lint)")
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
    #[test]
    fn a_posted_text_ends_with_the_mark_and_is_read_back_without_it() {
        assert_eq!(marked("Rebased"), "Rebased\n\n<!-- flottille -->");
        assert_eq!(marked("Rebased\n"), "Rebased\n\n<!-- flottille -->");
        assert_eq!(marked(""), "<!-- flottille -->");
        assert_eq!(
            footed("Rebased"),
            "Rebased\n\n— posted by a Flottille agent"
        );
        for posted in [
            marked("Rebased"),
            footed("Rebased"),
            marked("Rebased") + "\n",
        ] {
            assert_eq!(unmarked(&posted), ("Rebased", true), "{posted:?}");
        }
        assert_eq!(unmarked(&marked("")), ("", true));
        assert_eq!(unmarked("Thanks"), ("Thanks", false));
        // Only the end counts: a mark quoted in the middle of a person's text is theirs.
        let quoted = "Do not add <!-- flottille --> to it";
        assert_eq!(unmarked(quoted), (quoted, false));
    }
}
