//! Bitbucket, through the user's own `bkt` (<https://github.com/avivsinai/bitbucket-cli>):
//! Cloud and Data Center, read and written the way GitHub is through `gh` (`docs/forges.md`).
//! Spawned without a shell, with the user's own sign-in: Flottille never reads, keeps nor prints
//! a token, never runs `bkt auth`, and of the user's sign-in it reads the names and hosts of
//! `bkt`'s contexts, which hold no secret.
//!
//! What `bkt` prints is taken from its source at the release `docs/forges.md` pins, and from
//! Atlassian's REST documentation for what `bkt api` passes through.

use serde_json::Value;

use super::{
    Comment, Forge, ForgeCall, Gh, MERGE_METHODS, Pull, Seen, blocks, commit_hash, masked,
    method_refused, run,
};
use crate::mcp::Outcome;

/// The size of every page asked of Bitbucket: its pull request list takes 50 at most (`bkt` v0.37.0
/// passes `--limit` as `pagelen`, and Cloud answers `400 Bad Request: Invalid pagelen` above it),
/// while Atlassian's REST documentation gives 100 as the global maximum, says "some APIs" differ
/// and names none for the comments and the diffstat
/// (<https://developer.atlassian.com/cloud/bitbucket/rest/intro/#pagination>): 50 is the size no
/// endpoint refuses.
pub(super) const PAGE: usize = 50;
/// How many pull requests one list asks for: the newest ones. `bkt` follows `next` only up to its
/// `--limit`, which is the page size, so one page is all it can read.
const LISTED: usize = PAGE;
/// How many comments (Data Center: activities) one read follows the pages up to.
const COMMENTS: usize = 100;
/// How many changed files one read of a Cloud pull request's conflicts follows the pages up to.
const DIFFSTAT: usize = 500;

/// `bkt <args>`: whether it succeeded, its output and its errors.
async fn bkt(args: &[&str]) -> Result<(bool, String, String), Gh> {
    run(
        "bkt",
        &[("NO_COLOR", "1")],
        // It would print its requests, headers included.
        &["BKT_HTTP_DEBUG"],
        args,
    )
    .await
}

/// `bkt <args>`'s output, or a sentence saying why there is none, masked.
async fn text(args: &[&str]) -> Result<String, String> {
    let shown = args[..2].join(" ");
    match bkt(args).await {
        Ok((true, stdout, _)) => Ok(masked(&stdout)),
        Ok((false, _, stderr)) => {
            let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            Err(format!(
                "bkt {shown} failed: {}",
                masked(&said.chars().take(300).collect::<String>())
            ))
        }
        Err(Gh::Missing) => Err(missing()),
        Err(Gh::Failed(why)) => Err(format!("bkt {shown}: {why}")),
    }
}

/// `bkt <args>`'s JSON.
async fn json(args: &[&str]) -> Result<Value, String> {
    let shown = args[..2].join(" ");
    serde_json::from_str(&text(args).await?)
        .map_err(|error| format!("bkt {shown} answered what is not JSON: {error}"))
}

fn missing() -> String {
    "bkt, Bitbucket's command line, is not on the daemon's PATH: install it \
     (https://github.com/avivsinai/bitbucket-cli) and sign in with `bkt auth login <url>` to see \
     the pull requests; the rest of Flottille works without it"
        .into()
}

/// A context of `bkt`: a name for a host and defaults, which a command takes with `--context`.
struct Context {
    name: String,
    /// The host's key: its name, with the port when it has one.
    host: String,
    active: bool,
}

/// The contexts `bkt` has, listed once per read.
#[derive(Default)]
pub(super) struct Contexts(Option<Result<Vec<Context>, String>>);

impl Contexts {
    /// The context to give `bkt` for the forge's host (`--context`): the active one when it is on
    /// that host, else the first. `bkt` takes its host from a context alone, and the daemon never
    /// depends on which one the user left active. A sentence saying what to do when `bkt` is
    /// missing or has none.
    pub(super) async fn of(&mut self, forge: &Forge) -> Result<String, String> {
        if self.0.is_none() {
            self.0 = Some(list().await);
        }
        let all = self.0.as_ref().unwrap().as_ref().map_err(Clone::clone)?;
        let Forge::Bitbucket { host, .. } = forge else {
            return Err("not a Bitbucket repository".into());
        };
        let on_host = |context: &&Context| {
            let named = hostname(&context.host);
            named == *host || (host == "bitbucket.org" && named == "api.bitbucket.org")
        };
        let found = all
            .iter()
            .filter(on_host)
            .find(|context| context.active)
            .or_else(|| all.iter().find(on_host));
        found.map(|context| context.name.clone()).ok_or_else(|| {
            format!(
                "bkt has no context for {host}: sign in with `bkt auth login https://{host}`, \
                 then give it a context with `bkt context create <name> --host {host}`; the rest \
                 of Flottille works without it"
            )
        })
    }
}

/// A host key without its port, lowercase.
fn hostname(key: &str) -> String {
    key.split(':').next().unwrap_or(key).to_ascii_lowercase()
}

/// `bkt context list --json`: names, hosts and defaults, no secret.
async fn list() -> Result<Vec<Context>, String> {
    let listed = json(&["context", "list", "--json"])
        .await
        .map_err(|error| match error == missing() {
            true => error,
            false => format!("{error}: run `bkt auth login <url>` to sign in"),
        })?;
    Ok(listed["contexts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|context| {
            Some(Context {
                name: context["name"].as_str()?.to_owned(),
                host: context["host"].as_str()?.to_owned(),
                active: context["active"] == true,
            })
        })
        .collect())
}

/// `--workspace <w>` for Cloud, `--project <key>` for Data Center, then `--repo`.
fn place(forge: &Forge) -> [&str; 4] {
    match forge {
        Forge::Bitbucket {
            host, owner, name, ..
        } => [
            if host == "bitbucket.org" {
                "--workspace"
            } else {
                "--project"
            },
            owner,
            "--repo",
            name,
        ],
        Forge::GitHub { .. } => unreachable!("a Bitbucket repository"),
    }
}

pub(super) fn cloud(forge: &Forge) -> bool {
    matches!(forge, Forge::Bitbucket { host, .. } if host == "bitbucket.org")
}

/// `head`, then the context and the repository's flags, then `tail`.
fn command<'a>(
    forge: &'a Forge,
    context: &'a str,
    head: &[&'a str],
    tail: &[&'a str],
) -> Vec<&'a str> {
    let mut args = head.to_vec();
    args.extend(["--context", context]);
    args.extend(place(forge));
    args.extend(tail);
    args
}

/// A pull request of `bkt pr list --json`, in Cloud's shape (`source`, `destination`) or Data
/// Center's (`fromRef`, `toRef`).
struct Listed {
    id: u64,
    title: String,
    state: &'static str,
    branch: String,
    /// The commit the source branch was at (Cloud `source.commit.hash`, Data Center
    /// `fromRef.latestCommit`), when `bkt` says one as a hash.
    head_commit: Option<String>,
    base: String,
    url: String,
    /// From the repository itself, not from a fork of it.
    own: bool,
    /// Data Center says its reviewers in the list; Cloud does not.
    reviewers: Vec<(String, &'static str)>,
}

fn listed(forge: &Forge, pull: &Value) -> Option<Listed> {
    let text = |value: &Value| masked(value.as_str().unwrap_or_default());
    let state = match pull["state"].as_str() {
        Some("MERGED") => "merged",
        Some("DECLINED" | "SUPERSEDED") => "closed",
        _ => "open",
    };
    let id = pull["id"].as_u64()?;
    let title = text(&pull["title"]);
    if cloud(forge) {
        let (from, to) = (
            pull["source"]["repository"]["full_name"].as_str(),
            pull["destination"]["repository"]["full_name"].as_str(),
        );
        return Some(Listed {
            id,
            title,
            state,
            branch: text(&pull["source"]["branch"]["name"]),
            head_commit: commit_hash(&pull["source"]["commit"]["hash"]),
            base: text(&pull["destination"]["branch"]["name"]),
            url: text(&pull["links"]["html"]["href"]),
            own: matches!((from, to), (Some(from), Some(to))
                if !from.is_empty() && from.eq_ignore_ascii_case(to)),
            reviewers: Vec::new(),
        });
    }
    let repository = |side: &Value| {
        (
            side["repository"]["slug"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            side["repository"]["project"]["key"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase(),
        )
    };
    let (from, to) = (repository(&pull["fromRef"]), repository(&pull["toRef"]));
    Some(Listed {
        id,
        title,
        state,
        branch: text(&pull["fromRef"]["displayId"]),
        head_commit: commit_hash(&pull["fromRef"]["latestCommit"]),
        base: text(&pull["toRef"]["displayId"]),
        url: text(&pull["links"]["self"][0]["href"]),
        own: !from.0.is_empty() && from == to,
        reviewers: pull["participants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["role"] == "REVIEWER")
            .map(|p| {
                let state = match p["status"].as_str() {
                    Some("NEEDS_WORK") => "changes requested",
                    Some("APPROVED") => "approved",
                    _ if p["approved"] == true => "approved",
                    _ => "pending",
                };
                (person(&p["user"], &["displayName", "name"]), state)
            })
            .collect(),
    })
}

/// A person's name, masked: the first of `keys` that says one.
fn person(user: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| user[*key].as_str().filter(|name| !name.is_empty()))
        .map_or_else(|| "someone".to_owned(), masked)
}

/// The values of a Cloud collection read through `bkt api`, `PAGE` at a time: the following pages
/// are asked by the `next` link the server gives, up to `total` values. Whether the collection
/// goes on past what was read.
async fn cloud_values(
    context: &str,
    path: &str,
    total: usize,
) -> Result<(Vec<Value>, bool), String> {
    cloud_values_with(context, path, total, &[]).await
}

/// `cloud_values` with more query parameters on the first page, each given as `--param`: the path
/// never holds a `?`, `bkt api` is not known to merge a param into one.
async fn cloud_values_with(
    context: &str,
    path: &str,
    total: usize,
    params: &[&str],
) -> Result<(Vec<Value>, bool), String> {
    let size = format!("pagelen={PAGE}");
    let mut args = vec!["api", path, "--context", context, "--param", &size];
    for param in params {
        args.extend(["--param", param]);
    }
    let mut page = json(&args).await?;
    let mut values = Vec::new();
    loop {
        values.extend(page["values"].as_array().into_iter().flatten().cloned());
        let Some(next) = page["next"].as_str() else {
            return Ok((values, false));
        };
        if values.len() >= total {
            values.truncate(total);
            return Ok((values, true));
        }
        // The link is followed on the API's own host only: `bkt api` sends the sign-in to any
        // address it is given.
        let Some(rest) = next.strip_prefix("https://api.bitbucket.org/") else {
            return Ok((values, true));
        };
        page = json(&["api", &format!("/{rest}"), "--context", context]).await?;
    }
}

/// The activities of a Data Center pull request, `PAGE` at a time up to `total`, from where each
/// page says the next starts.
async fn dc_values(context: &str, path: &str, total: usize) -> Result<Vec<Value>, String> {
    let size = format!("limit={PAGE}");
    let mut values = Vec::new();
    let mut start = 0;
    loop {
        let from = format!("start={start}");
        let mut args = vec!["api", path, "--context", context, "--param", &size];
        if start > 0 {
            args.extend(["--param", &from]);
        }
        let page = json(&args).await?;
        let read = page["values"].as_array().map_or(0, |values| values.len());
        values.extend(page["values"].as_array().into_iter().flatten().cloned());
        match page["nextPageStart"].as_u64() {
            Some(next) if page["isLastPage"] != true && read > 0 && values.len() < total => {
                start = next as usize
            }
            _ => break,
        }
    }
    values.truncate(total);
    Ok(values)
}

/// The pull requests in `state` (`all`, `OPEN`…), the newest `LISTED`.
async fn pulls(forge: &Forge, context: &str, state: &str) -> Result<Vec<Listed>, String> {
    let limit = LISTED.to_string();
    let args = command(
        forge,
        context,
        &["pr", "list"],
        &["--state", state, "--limit", &limit, "--json"],
    );
    let said = json(&args).await?;
    Ok(said["pull_requests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pull| listed(forge, pull))
        .collect())
}

/// `forge list` of a Bitbucket repository: its newest pull requests in `state`, of `branch` when
/// given, and whether the repository may hold older ones `bkt` did not list. `bkt` filters by
/// state and not by branch, so the branch is picked from them; it has no draft and no state for
/// `closed` and `merged` together.
pub(super) async fn pulls_in(
    forge: &Forge,
    context: &str,
    state: &str,
    branch: Option<&str>,
) -> Result<(Vec<super::Listed>, bool), String> {
    let asked = match state {
        "open" => "OPEN",
        "merged" => "MERGED",
        "closed" => "DECLINED",
        _ => "all",
    };
    let all = pulls(forge, context, asked).await?;
    let cut = all.len() >= LISTED;
    let listed = all
        .into_iter()
        .filter(|pull| branch.is_none_or(|branch| pull.branch == branch))
        .map(|pull| super::Listed {
            number: pull.id,
            title: pull.title,
            state: pull.state,
            branch: pull.branch,
            base: pull.base,
            url: pull.url,
        })
        .collect();
    Ok((listed, cut))
}

/// The pull request of `branch`, the newest when there are several; none when there is none.
/// `bkt` filters by state and not by branch, so the newest are listed and the branch is picked
/// from them.
pub(super) async fn pull_of(
    forge: &Forge,
    context: &str,
    branch: &str,
) -> Result<Option<Pull>, String> {
    let found = pulls(forge, context, "all")
        .await?
        .into_iter()
        .filter(|pull| pull.branch == branch)
        .max_by_key(|pull| pull.id);
    match found {
        Some(found) => detailed(forge, context, found).await.map(Some),
        None => Ok(None),
    }
}

/// The pull request numbered `number`, among the newest; none when they do not hold it, and an
/// error when they may not, since `bkt` lists no more of them.
pub(super) async fn pull_numbered(
    forge: &Forge,
    context: &str,
    number: u64,
) -> Result<Option<Pull>, String> {
    let all = pulls(forge, context, "all").await?;
    let cut = all.len() >= LISTED;
    match all.into_iter().find(|pull| pull.id == number) {
        Some(found) => detailed(forge, context, found).await.map(Some),
        None if cut => Err(format!(
            "the pull request #{number} is not among the newest {LISTED} of {}, the most `bkt` lists: it may be older",
            forge.slug()
        )),
        None => Ok(None),
    }
}

/// A listed pull request with its checks, reviewers and comments.
async fn detailed(forge: &Forge, context: &str, found: Listed) -> Result<Pull, String> {
    let id = found.id.to_string();
    let statuses = statuses_of(forge, context, &id).await?;
    let checks: Vec<_> = statuses.iter().map(check_of).collect();
    let mergeable = mergeable(forge, context, &id).await;
    let (reviewers, comments) = if cloud(forge) {
        let base = cloud_page(forge, &id);
        let pull = json(&["api", &base, "--context", context]).await?;
        let (comments, _) = cloud_values(context, &format!("{base}/comments"), COMMENTS).await?;
        (cloud_reviewers(&pull), cloud_comments(&comments))
    } else {
        let Forge::Bitbucket { owner, name, .. } = forge else {
            unreachable!()
        };
        let activities = dc_values(
            context,
            &format!("/rest/api/1.0/projects/{owner}/repos/{name}/pull-requests/{id}/activities"),
            COMMENTS,
        )
        .await?;
        (found.reviewers, dc_comments(&activities))
    };
    let jobs = statuses
        .iter()
        .zip(&checks)
        .filter(|(_, (_, state))| *state == "failing")
        .map(|(status, (name, _))| super::Job {
            name: name.clone(),
            details: status["url"]
                .as_str()
                .filter(|link| !link.is_empty())
                .map(masked),
            log: None,
        })
        .collect();
    Ok(Pull {
        number: found.id,
        title: found.title,
        state: found.state,
        head: found.branch,
        head_commit: found.head_commit,
        base: found.base,
        url: found.url,
        mergeable,
        review: decision(&reviewers),
        checks,
        comments,
        reviews: Vec::new(),
        reviewers,
        jobs,
    })
}

/// The checks of the pull request `id`: the commit statuses of its source commit.
async fn checks_of(
    forge: &Forge,
    context: &str,
    id: &str,
) -> Result<Vec<(String, &'static str)>, String> {
    let statuses = statuses_of(forge, context, id).await?;
    Ok(statuses.iter().map(check_of).collect())
}

/// The commit statuses of the pull request `id`, as `bkt pr checks --json` gives them.
async fn statuses_of(forge: &Forge, context: &str, id: &str) -> Result<Vec<Value>, String> {
    let said = json(&command(forge, context, &["pr", "checks", id], &["--json"])).await?;
    match said.get("statuses") {
        // A commit with no status: `bkt` answers `null` (a nil Go slice), and no check ran.
        Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(statuses)) => Ok(statuses.clone()),
        // Without its `statuses` (or in another shape), `bkt` said nothing about them.
        _ => Err(format!(
            "the checks of the pull request #{id} could not be read: `bkt pr checks --json` \
             answered without `statuses`"
        )),
    }
}

/// Whether the pull request `id` merges without conflict: on Cloud, no file of its diffstat is
/// `merge conflict` (a diffstat that goes on past the `DIFFSTAT` files read, and no conflict among
/// them, says nothing); on Data Center, its merge's page is not `conflicted`. None when it cannot
/// be read: the rest of the pull request is still shown.
async fn mergeable(forge: &Forge, context: &str, id: &str) -> Option<bool> {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!()
    };
    if cloud(forge) {
        let path = format!("{}/diffstat", cloud_page(forge, id));
        let (files, more) = cloud_values(context, &path, DIFFSTAT).await.ok()?;
        if files.iter().any(|file| file["status"] == "merge conflict") {
            return Some(false);
        }
        // More files may hold the conflict: without it, nothing is known.
        return (!more).then_some(true);
    }
    let path = format!("/rest/api/1.0/projects/{owner}/repos/{name}/pull-requests/{id}/merge");
    let page = json(&["api", &path, "--context", context]).await.ok()?;
    page["conflicted"].as_bool().map(|conflicted| !conflicted)
}

/// The strategy `bkt pr merge --strategy` takes for a method, by host: none when `bkt` has none
/// (a Cloud rebase: it knows `merge_commit`, `squash` and `fast_forward` only).
fn strategy(forge: &Forge, method: &str) -> Option<&'static str> {
    match (cloud(forge), method) {
        (_, "squash") => Some("squash"),
        (true, "merge") => Some("merge_commit"),
        (false, "merge") => Some("no-ff"),
        (false, "rebase") => Some("rebase-ff-only"),
        _ => None,
    }
}

/// The methods the repository allows its pull request `id` to be merged by: Cloud says them on
/// the pull request's destination branch, Data Center in the repository's pull request settings.
/// Then the strategies it allows that no method reaches (`ff`, `squash-ff-only`, `fast_forward`…),
/// as it names them.
async fn allowed(
    forge: &Forge,
    context: &str,
    id: &str,
) -> Result<(Vec<&'static str>, Vec<String>), String> {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!()
    };
    let strategies: Vec<String> = if cloud(forge) {
        let page = json(&["api", &cloud_page(forge, id), "--context", context]).await?;
        page["destination"]["branch"]["merge_strategies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect()
    } else {
        let path = format!("/rest/api/1.0/projects/{owner}/repos/{name}/settings/pull-requests");
        let page = json(&["api", &path, "--context", context]).await?;
        page["mergeConfig"]["strategies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["enabled"] == true)
            .filter_map(|s| s["id"].as_str().map(str::to_owned))
            .collect()
    };
    let reached = |strategy: &String| {
        MERGE_METHODS
            .into_iter()
            .any(|method| self::strategy(forge, method) == Some(strategy.as_str()))
    };
    let others = strategies.iter().filter(|s| !reached(s)).cloned().collect();
    let methods = MERGE_METHODS
        .into_iter()
        .filter(|method| strategy(forge, method).is_some_and(|s| strategies.iter().any(|t| t == s)))
        .collect();
    Ok((methods, others))
}

/// The path of a Cloud repository, for `bkt api`.
fn cloud_repository(forge: &Forge) -> String {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!()
    };
    format!("/repositories/{owner}/{name}")
}

/// The branch a pipeline of Bitbucket's API ran for: the branch's own (`ref_name`), or the source
/// branch of the pull request.
fn pipeline_branch(pipeline: &Value) -> Option<&str> {
    let target = &pipeline["target"];
    match target["type"].as_str()? {
        "pipeline_pullrequest_target" => target["source"].as_str(),
        _ => target["ref_name"].as_str(),
    }
}

/// The build a check's link names: `https://bitbucket.org/<workspace>/<repo>/pipelines/results/<n>`
/// of this very repository.
pub(super) fn build_of(forge: &Forge, link: Option<&str>) -> Option<u64> {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        return None;
    };
    let prefix = format!("https://bitbucket.org/{owner}/{name}/pipelines/results/").to_lowercase();
    let link = link?.to_lowercase();
    let rest = link.strip_prefix(&prefix)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|build| *build > 0)
}

/// The log of the first failed step of the build `build`: `bkt pipeline view` names the steps,
/// `bkt pipeline logs --step` prints one. Masked.
pub(super) async fn failed_step_log(
    forge: &Forge,
    context: &str,
    build: u64,
) -> Result<String, String> {
    let build = build.to_string();
    let seen = json(&command(
        forge,
        context,
        &["pipeline", "view", &build],
        &["--json"],
    ))
    .await?;
    let failed = seen["steps"].as_array().into_iter().flatten().find(|step| {
        matches!(
            step["state"]["result"]["name"]
                .as_str()
                .or(step["result"]["name"].as_str()),
            Some("FAILED" | "ERROR")
        )
    });
    // The uuid goes to a command line: only what a uuid is made of.
    let uuid = failed
        .and_then(|step| step["uuid"].as_str())
        .filter(|uuid| {
            uuid.chars()
                .all(|c| c.is_ascii_hexdigit() || "{}-".contains(c))
        });
    let Some(uuid) = uuid else {
        return Err(format!("the pipeline #{build} has no failed step"));
    };
    text(&command(
        forge,
        context,
        &["pipeline", "logs", &build],
        &["--step", uuid],
    ))
    .await
}

/// The path of a Cloud pull request's own page, for `bkt api`.
fn cloud_page(forge: &Forge, id: &str) -> String {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!()
    };
    format!("/repositories/{owner}/{name}/pullrequests/{id}")
}

/// The reviewers' verdicts as one: `changes requested` when one asks for it, else `approved` when
/// every one approved, else `review required`; none without reviewers.
fn decision(reviewers: &[(String, &'static str)]) -> Option<&'static str> {
    let any = |state| reviewers.iter().any(|(_, s)| *s == state);
    if reviewers.is_empty() {
        None
    } else if any("changes requested") {
        Some("changes requested")
    } else if any("pending") {
        Some("review required")
    } else {
        Some("approved")
    }
}

/// A commit status of `bkt pr checks --json` (`statuses`): `SUCCESSFUL`, `FAILED`, `INPROGRESS`
/// or `STOPPED`.
fn check_of(status: &Value) -> (String, &'static str) {
    let name = status["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .or(status["key"].as_str())
        .unwrap_or("check");
    let state = match status["state"]
        .as_str()
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("SUCCESSFUL" | "SUCCESS") => "passing",
        Some("FAILED" | "FAILURE" | "ERROR" | "STOPPED" | "CANCELLED") => "failing",
        _ => "pending",
    };
    (masked(name), state)
}

/// The reviewers of a Cloud pull request's own page (`participants`).
fn cloud_reviewers(pull: &Value) -> Vec<(String, &'static str)> {
    pull["participants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["role"] == "REVIEWER")
        .map(|p| {
            let state = match p["state"].as_str() {
                Some("changes_requested") => "changes requested",
                Some("approved") => "approved",
                _ if p["approved"] == true => "approved",
                _ => "pending",
            };
            (person(&p["user"], &["display_name", "nickname"]), state)
        })
        .collect()
}

/// The comments of a Cloud pull request, the deleted ones left out.
fn cloud_comments(comments: &[Value]) -> Vec<Comment> {
    comments
        .iter()
        .filter(|c| c["deleted"] != true)
        .map(|c| Comment {
            author: person(&c["user"], &["display_name", "nickname"]),
            state: None,
            body: masked(c["content"]["raw"].as_str().unwrap_or_default()),
            // What says a Bitbucket bot is not read: every author is taken for a person.
            bot: false,
        })
        .collect()
}

/// The comments among a Data Center pull request's activities, replies after what they answer.
fn dc_comments(activities: &[Value]) -> Vec<Comment> {
    fn walk(comment: &Value, out: &mut Vec<Comment>) {
        out.push(Comment {
            author: person(&comment["author"], &["displayName", "name"]),
            state: None,
            body: masked(comment["text"].as_str().unwrap_or_default()),
            bot: false,
        });
        for reply in comment["comments"].as_array().into_iter().flatten() {
            walk(reply, out);
        }
    }
    let mut comments = Vec::new();
    for activity in activities {
        if activity["action"] == "COMMENTED" {
            walk(&activity["comment"], &mut comments);
        }
    }
    comments
}

/// `forge comment` and `forge edit` of a Bitbucket pull request: `bkt pr comment <id> --text` and
/// `bkt pr edit <id> --title --description` (`pkg/cmd/pr/pr.go:1654-1669`, `:719-762`).
pub(super) async fn note(
    forge: &Forge,
    action: &str,
    number: &str,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<(), String> {
    let context = Contexts::default().of(forge).await?;
    let mut flags = Vec::new();
    match action {
        "comment" => flags.extend(["--text", body.unwrap_or_default()]),
        _ => {
            if let Some(title) = title {
                flags.extend(["--title", title]);
            }
            if let Some(body) = body {
                flags.extend(["--description", body]);
            }
        }
    }
    text(&command(forge, &context, &["pr", action, number], &flags))
        .await
        .map(|_| ())
}

/// `forge` `create`, `merge` and `checks` for a Bitbucket repository, once the gate let it
/// through and the remote was found where the workspace was created: the same actions and the
/// same checks as for GitHub, through `bkt`.
pub(super) async fn write(
    forge: &Forge,
    branch: &str,
    base: &str,
    reviewers: &[String],
    asked: ForgeCall,
) -> Outcome {
    let slug = forge.slug();
    let context = match Contexts::default().of(forge).await {
        Ok(context) => context,
        Err(problem) => return Outcome::Failed(problem),
    };
    let context = context.as_str();
    match asked.action.as_str() {
        "create" => {
            let Some(title) = asked.title.filter(|t| !t.trim().is_empty()) else {
                return Outcome::Refused(
                    "refused: create needs a title: say what the pull request does".into(),
                );
            };
            if asked.draft {
                return Outcome::Refused(
                    "refused: bkt opens no draft pull request: open it without `draft`, and the \
                     user makes it a draft on Bitbucket"
                        .into(),
                );
            }
            let body = asked.body.unwrap_or_default();
            let mut flags = vec![
                "--source",
                branch,
                "--target",
                base,
                "--title",
                &title,
                "--description",
                &body,
            ];
            for reviewer in reviewers {
                flags.extend(["--reviewer", reviewer.as_str()]);
            }
            let args = command(forge, context, &["pr", "create"], &flags);
            let said = match text(&args).await {
                Ok(said) => said,
                Err(why) => return Outcome::Failed(why),
            };
            // `bkt` says `✓ Created pull request #12` and no link: the link is read back.
            let number = said
                .split('#')
                .nth(1)
                .map(|rest| {
                    rest.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                })
                .and_then(|digits| digits.parse::<u64>().ok());
            let url = match pulls(forge, context, "all").await {
                Ok(listed) => listed
                    .into_iter()
                    .find(|pull| Some(pull.id) == number)
                    .map(|pull| pull.url),
                Err(_) => None,
            };
            Outcome::Done(format!(
                "The pull request of {branch} into {base} was opened in {slug}: {}",
                url.unwrap_or_else(|| said.trim().to_owned())
            ))
        }
        "merge" => {
            let method = asked.method();
            let Some(strategy) = strategy(forge, method) else {
                return Outcome::Refused(format!(
                    "refused: bkt merges a Bitbucket Cloud pull request by squash or merge, never \
                     by {method}: its strategies are merge_commit, squash and fast_forward; \
                     nothing was merged"
                ));
            };
            let open = match pulls(forge, context, "OPEN").await {
                Ok(open) => open,
                Err(why) => return Outcome::Failed(why),
            };
            let on_branch: Vec<&Listed> = open.iter().filter(|p| p.branch == branch).collect();
            // The branch's name is also that of a branch in any fork of the repository.
            let ours = |pull: &&&Listed| pull.own && pull.base == base;
            let Some(pull) = on_branch.iter().find(ours) else {
                let why = match on_branch.first() {
                    None => format!("{branch} has no open pull request in {slug}: open it first"),
                    Some(pull) if !pull.own => format!(
                        "the open pull request #{} of a branch {branch} in {slug} comes from a \
                         fork, not from the workspace's repository",
                        pull.id
                    ),
                    Some(pull) => format!(
                        "the open pull request #{} of {branch} in {slug} goes into {}, not \
                         {base}, the workspace's base",
                        pull.id, pull.base
                    ),
                };
                return Outcome::Refused(format!(
                    "refused: {why}: only the pull request of the workspace's own branch into \
                     its base is merged"
                ));
            };
            let number = pull.id.to_string();
            if asked.waive.is_none() {
                let checks = match checks_of(forge, context, &number).await {
                    Ok(checks) => checks,
                    Err(why) => return Outcome::Failed(why),
                };
                // Data Center says the reviewers in the list; Cloud on the pull request's page.
                let reviewers = if cloud(forge) {
                    let page = cloud_page(forge, &number);
                    match json(&["api", &page, "--context", context]).await {
                        Ok(page) => cloud_reviewers(&page),
                        Err(why) => return Outcome::Failed(why),
                    }
                } else {
                    pull.reviewers.clone()
                };
                if let Some(blocks) = blocks(&checks, decision(&reviewers)) {
                    return Outcome::Refused(format!(
                        "refused: the pull request #{number} of {branch} in {slug} is not \
                         merged: {blocks}; wait for its checks, or have it waived by the user \
                         (waive, with their words)"
                    ));
                }
            }
            // `bkt` closes the source branch unless told not to; `gh pr merge` leaves it.
            let args = command(
                forge,
                context,
                &["pr", "merge", &number],
                &["--strategy", strategy, "--close-source=false"],
            );
            match text(&args).await {
                Ok(_) => Outcome::Done(format!(
                    "The pull request #{number} of {branch} was merged by {method} into {base} \
                     in {slug}."
                )),
                // Refused by the server: when the repository forbids the method, say which ones
                // it allows.
                Err(why) => match allowed(forge, context, &number).await {
                    Ok((allowed, others)) if !allowed.contains(&method) => {
                        method_refused(&slug, method, &allowed, &others)
                    }
                    _ => Outcome::Failed(why),
                },
            }
        }
        "checks" => {
            if !cloud(forge) {
                return Outcome::Refused(format!(
                    "refused: Bitbucket Data Center has no pipelines bkt can run, and {slug} is \
                     on a Data Center host: its builds are another tool's"
                ));
            }
            let path = format!("{}/pipelines/", cloud_repository(forge));
            let pipelines =
                match cloud_values_with(context, &path, PAGE, &["sort=-created_on"]).await {
                    Ok((pipelines, _)) => pipelines,
                    Err(why) => return Outcome::Failed(why),
                };
            // The newest pipeline of the branch, whether it ran for a push or for a pull request:
            // `bkt pipeline list` leaves the branch out (`target.ref.name` is always empty), the
            // API's own targets name it.
            let last = pipelines
                .iter()
                .filter(|p| pipeline_branch(p) == Some(branch))
                .max_by_key(|p| p["build_number"].as_u64().unwrap_or(0));
            let Some(last) = last else {
                return Outcome::Refused(format!(
                    "refused: no pipeline of {branch} in {slug}: there is nothing to re-run"
                ));
            };
            let number = last["build_number"].as_u64().unwrap_or(0);
            if !matches!(
                last["state"]["result"]["name"].as_str(),
                Some("FAILED" | "ERROR")
            ) {
                return Outcome::Refused(format!(
                    "refused: the last pipeline of {branch} in {slug} (#{number}) did not fail: \
                     there is nothing to re-run"
                ));
            }
            // A pull request's pipeline is started again by its own selector.
            let pattern = last["target"]["selector"]["pattern"]
                .as_str()
                .unwrap_or("**");
            let mut flags = vec!["--ref", branch];
            if last["target"]["type"] == "pipeline_pullrequest_target" {
                flags.extend([
                    "--selector-type",
                    "pull-requests",
                    "--selector-pattern",
                    pattern,
                ]);
            }
            match text(&command(forge, context, &["pipeline", "run"], &flags)).await {
                Ok(_) => Outcome::Done(format!(
                    "The pipeline of {branch} was started again in {slug}, after #{number} \
                     failed."
                )),
                Err(why) => Outcome::Failed(why),
            }
        }
        action => Outcome::Refused(format!("refused: forge {action} writes nothing here")),
    }
}

/// The state, source branch and origin of the pull request `number`, for `forge close`: none when
/// the newest pull requests `bkt` lists do not hold it and it may be older (an error then).
pub(super) async fn seen(forge: &Forge, number: u64) -> Result<Option<Seen>, String> {
    let context = Contexts::default().of(forge).await?;
    let all = pulls(forge, &context, "all").await?;
    let cut = all.len() >= LISTED;
    match all.into_iter().find(|pull| pull.id == number) {
        Some(pull) => Ok(Some(Seen {
            state: pull.state,
            branch: pull.branch,
            own: pull.own,
        })),
        None if cut => Err(format!(
            "the pull request #{number} is not among the newest {LISTED} of {}, the most `bkt` \
             lists: it may be older",
            forge.slug()
        )),
        None => Ok(None),
    }
}

/// `forge close` of a Bitbucket pull request: `bkt pr decline <id> [--comment T]
/// [--delete-source]` (v0.37.0, `pkg/cmd/pr/pr.go`, `newDeclineCmd`).
pub(super) async fn decline(
    forge: &Forge,
    number: &str,
    comment: Option<&str>,
    delete_source: bool,
) -> Result<(), String> {
    let context = Contexts::default().of(forge).await?;
    let mut flags = Vec::new();
    if let Some(comment) = comment {
        flags.extend(["--comment", comment]);
    }
    if delete_source {
        flags.push("--delete-source");
    }
    text(&command(
        forge,
        &context,
        &["pr", "decline", number],
        &flags,
    ))
    .await
    .map(|_| ())
}
