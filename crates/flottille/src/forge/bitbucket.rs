//! Bitbucket, through the user's own `bkt` (<https://github.com/avivsinai/bitbucket-cli>):
//! Cloud and Data Center, read and written the way GitHub is through `gh` (`docs/forges.md`).
//! Spawned without a shell, with the user's own sign-in: Flottille never reads, keeps nor prints
//! a token, never runs `bkt auth`, and of the user's sign-in it reads the names and hosts of
//! `bkt`'s contexts, which hold no secret.
//!
//! What `bkt` prints is taken from its source at the release `docs/forges.md` pins, and from
//! Atlassian's REST documentation for what `bkt api` passes through.

use serde_json::{Value, json};

use super::ending::{Ending, Rerun};
use super::{
    Branch, Comment, Deletion, Forge, ForgeCall, Gh, MERGE_METHODS, Pull, Reviewer, Seen, after,
    blocks, commit_hash, masked, method_refused, run,
};
use crate::daemon::Daemon;
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
pub(super) async fn text(args: &[&str]) -> Result<String, String> {
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
pub(super) async fn json(args: &[&str]) -> Result<Value, String> {
    let shown = args[..2].join(" ");
    serde_json::from_str(&text(args).await?)
        .map_err(|error| format!("bkt {shown} answered what is not JSON: {error}"))
}

fn missing() -> String {
    "Flottille cannot find bkt, Bitbucket's command line: install it \
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
        let on_host = |context: &&Context| on_host(context, host);
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

/// Whether the context is on `host`: Cloud's contexts name `api.bitbucket.org`.
fn on_host(context: &Context, host: &str) -> bool {
    let named = hostname(&context.host);
    named == host || (host == "bitbucket.org" && named == "api.bitbucket.org")
}

/// Whether `bkt` has a context for `host`, which is what signed in means for it; the sentence of
/// why it could not be said when `bkt` could not list them.
pub(crate) async fn has_context(host: &str) -> Result<bool, String> {
    Ok(list().await?.iter().any(|context| on_host(context, host)))
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

/// The seconds the newest finished pipelines of a Cloud repository took, newest first: from the
/// moment each was created to the moment it completed, among the 50 `bkt pipeline list` gives. A
/// stopped pipeline, one still running and one with no end measure nothing.
pub(super) async fn pipelines(forge: &Forge, context: &str) -> Result<Vec<u64>, String> {
    let limit = LISTED.to_string();
    let listed = json(&command(
        forge,
        context,
        &["pipeline", "list"],
        &["--limit", &limit, "--json"],
    ))
    .await?;
    Ok(listed["pipelines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| {
            matches!(
                p["state"]["result"]["name"].as_str(),
                Some("SUCCESSFUL" | "FAILED" | "ERROR")
            )
        })
        .filter_map(|p| super::ci::between(&p["created_on"], &p["completed_on"]))
        .collect())
}

/// `head`, then the context and the repository's flags, then `tail`.
pub(super) fn command<'a>(
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
    /// What its author wrote of it.
    description: String,
    /// From the repository itself, not from a fork of it.
    own: bool,
    /// When Cloud says the pull request last changed (a push does), in milliseconds.
    updated: Option<u64>,
    /// Data Center says its reviewers in the list; Cloud does not.
    reviewers: Vec<Reviewer>,
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
            description: text(&pull["description"]),
            own: matches!((from, to), (Some(from), Some(to))
                if !from.is_empty() && from.eq_ignore_ascii_case(to)),
            updated: pull["updated_on"]
                .as_str()
                .and_then(crate::session_read::form),
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
        description: text(&pull["description"]),
        own: !from.0.is_empty() && from == to,
        updated: None,
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
                (
                    person(&p["user"], &["displayName", "name"]),
                    state,
                    handle_of(&p["user"], &["name"]),
                )
            })
            .collect(),
    })
}

/// What a person is named by besides their display name, masked: the first of `keys` that says
/// one, else empty.
fn handle_of(user: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| user[*key].as_str().filter(|name| !name.is_empty()))
        .map_or_else(String::new, masked)
}

/// `handle_of` as a comment keeps it.
fn handle_in(user: &Value, keys: &[&str]) -> Option<String> {
    Some(handle_of(user, keys)).filter(|handle| !handle.is_empty())
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
pub(super) async fn cloud_values_with(
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
/// page says the next starts; `params` are more query parameters, each `key=value`.
pub(super) async fn dc_values(
    context: &str,
    path: &str,
    total: usize,
    params: &[&str],
) -> Result<Vec<Value>, String> {
    let size = format!("limit={PAGE}");
    let mut values = Vec::new();
    let mut start = 0;
    loop {
        let from = format!("start={start}");
        let mut args = vec!["api", path, "--context", context, "--param", &size];
        for param in params {
            args.extend(["--param", param]);
        }
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
    detailed_with(forge, context, found)
        .await
        .map(|(pull, _)| pull)
}

/// `detailed`, with the commit statuses its checks are of.
async fn detailed_with(
    forge: &Forge,
    context: &str,
    found: Listed,
) -> Result<(Pull, Vec<Value>), String> {
    let id = found.id.to_string();
    // The statuses, the conflicts and what people said do not depend on one another: read
    // together, a failure is still the first of the statuses, the page and the comments, as when
    // they were read one after the other.
    let people = async {
        if cloud(forge) {
            let base = cloud_page(forge, &id);
            let comments_path = format!("{base}/comments");
            let page_args = ["api", &base, "--context", context];
            let (page, comments) = futures::join!(
                json(&page_args),
                cloud_values(context, &comments_path, COMMENTS)
            );
            let page = page?;
            let (comments, _) = comments?;
            let closing = cloud_closing(found.state, &page);
            Ok::<_, String>((cloud_reviewers(&page), cloud_comments(&comments), closing))
        } else {
            let Forge::Bitbucket { owner, name, .. } = forge else {
                unreachable!()
            };
            let activities = dc_values(
                context,
                &format!(
                    "/rest/api/1.0/projects/{owner}/repos/{name}/pull-requests/{id}/activities"
                ),
                COMMENTS,
                &[],
            )
            .await?;
            let closing = dc_closing(found.state, &activities);
            Ok((found.reviewers.clone(), dc_comments(&activities), closing))
        }
    };
    let (statuses, mergeable, people) = futures::join!(
        statuses_of(forge, context, &id),
        mergeable(forge, context, &id),
        people
    );
    let statuses = statuses?;
    let checks: Vec<_> = statuses.iter().map(check_of).collect();
    let not_started = match (&found.head_commit, found.updated) {
        (Some(head), Some(updated)) if checks.is_empty() => {
            not_started(forge, context, head, updated).await
        }
        _ => None,
    };
    let (reviewers, comments, closing) = people?;
    let jobs = checks_of_statuses(&statuses)
        .iter()
        .filter(|check| check.state == "failing")
        .map(super::Job::of)
        .collect();
    let pull = Pull {
        number: found.id,
        title: found.title,
        state: found.state,
        head: found.branch,
        head_commit: found.head_commit,
        base: found.base,
        url: found.url,
        description: found.description,
        mergeable,
        review: decision(&reviewers),
        checks,
        not_started,
        comments,
        reviews: Vec::new(),
        reviewers,
        jobs,
        ci: None,
        closing,
        ended: None,
    };
    Ok((pull, statuses))
}

/// The states Bitbucket Cloud lists a pull request in: all of them, since it lists the open ones
/// alone unless asked (Atlassian's REST documentation, "List pull requests": the `state`
/// parameter is repeated for several).
const CLOUD_STATES: [&str; 4] = [
    "state=OPEN",
    "state=MERGED",
    "state=DECLINED",
    "state=SUPERSEDED",
];
/// The most pull requests one round of the items lists of a repository.
const ALL: usize = 1000;

/// A listed pull request as an item, its cheap fields alone.
fn cheap(forge: &Forge, path: &str, found: Listed) -> crate::items::Fetched {
    let pull = Pull {
        number: found.id,
        title: found.title,
        state: found.state,
        head: found.branch,
        head_commit: found.head_commit,
        base: found.base,
        url: found.url,
        description: found.description,
        mergeable: None,
        review: None,
        checks: Vec::new(),
        not_started: None,
        comments: Vec::new(),
        reviews: Vec::new(),
        reviewers: found.reviewers,
        jobs: Vec::new(),
        ci: None,
        closing: None,
        ended: None,
    };
    let mut fetched = super::fetched_pull(path, "bitbucket", &pull, false);
    if !cloud(forge) {
        // Data Center says its reviewers in the list: their verdicts are its review.
        fetched.item.details["review"] = serde_json::json!(decision(&pull.reviewers));
    }
    fetched
}

/// A time Bitbucket says, an RFC 3339 text (Cloud) or milliseconds (Data Center), as a text.
fn time_of(value: &Value) -> Option<String> {
    match value {
        Value::String(at) => Some(masked(at)),
        Value::Number(at) => Some(at.to_string()),
        _ => None,
    }
}

/// The pull requests of the repository the items list (`docs/items.md`), their cheap fields
/// alone. Cloud is asked through `bkt api` with Bitbucket's own filter, `q=updated_on>="<since>"`,
/// newest change first, the filter and the order given as `--param` (never in the path: `bkt
/// api` is not known to merge them). Data Center has no such filter: every open pull request is
/// listed, page after page, and one stored open that left the list is read by itself, to say
/// whether it was merged or declined.
pub(super) async fn list_pulls(
    forge: &Forge,
    context: &str,
    path: &str,
    since: Option<&str>,
    stored: &[flottille_core::item::Item],
) -> Result<crate::items::Listing, String> {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!("a Bitbucket repository")
    };
    let mut items = Vec::new();
    if cloud(forge) {
        let filter = since.map(|since| format!("q=updated_on>=\"{since}\""));
        let mut params = vec!["sort=-updated_on"];
        params.extend(CLOUD_STATES);
        if let Some(filter) = &filter {
            params.push(filter);
        }
        let (values, more) = cloud_values_with(
            context,
            &format!("/repositories/{owner}/{name}/pullrequests"),
            ALL,
            &params,
        )
        .await?;
        for value in values {
            let Some(found) = listed(forge, &value) else {
                continue;
            };
            let mut fetched = cheap(forge, path, found);
            fetched.item.author = value["author"]["display_name"].as_str().map(masked);
            fetched.item.updated_at = value["updated_on"].as_str().map(masked);
            fetched.created_at = value["created_on"]
                .as_str()
                .and_then(crate::session_read::form);
            items.push(fetched);
        }
        return Ok(crate::items::Listing {
            items,
            note: more
                .then(|| format!("only the {ALL} most recently changed pull requests are listed")),
            details: true,
            log: Vec::new(),
        });
    }
    let base = format!("/rest/api/1.0/projects/{owner}/repos/{name}/pull-requests");
    // The open ones: the endpoint's default state.
    let values = dc_values(context, &base, ALL, &[]).await?;
    let mut notes =
        vec!["mine is not known on Bitbucket Data Center: bkt says no user there".to_owned()];
    if values.len() >= ALL {
        notes.push(format!("only {ALL} open pull requests are listed"));
    }
    let dc_item = |value: &Value| {
        let found = listed(forge, value)?;
        let mut fetched = cheap(forge, path, found);
        fetched.item.author = Some(person(&value["author"]["user"], &["displayName", "name"]));
        fetched.item.updated_at = time_of(&value["updatedDate"]);
        fetched.item.details["version"] = value["version"].clone();
        fetched.created_at = value["createdDate"].as_u64();
        Some(fetched)
    };
    items.extend(values.iter().filter_map(dc_item));
    let left: Vec<&flottille_core::item::Item> = stored
        .iter()
        .filter(|item| {
            matches!(item.state.as_str(), "open" | "draft")
                && !items.iter().any(|listed| listed.item.key == item.key)
        })
        .collect();
    for gone in left {
        let fetched =
            match json(&["api", &format!("{base}/{}", gone.key), "--context", context]).await {
                Ok(value) => dc_item(&value),
                Err(error) if super::not_found(&error) => {
                    Some(crate::items::Fetched::removed(gone.clone()))
                }
                Err(error) => return Err(error),
            };
        items.extend(fetched);
    }
    Ok(crate::items::Listing {
        items,
        note: Some(notes.join("; ")),
        details: true,
        log: Vec::new(),
    })
}

/// What a listed pull request is, as `detailed` takes it, from what the items keep of it.
fn listed_of(item: &flottille_core::item::Item) -> Option<Listed> {
    let text = |key: &str| item.details[key].as_str().unwrap_or_default().to_owned();
    let state = match item.state.as_str() {
        "merged" => "merged",
        "closed" => "closed",
        _ => "open",
    };
    let verdict = |state: &str| match state {
        "approved" => "approved",
        "changes requested" => "changes requested",
        _ => "pending",
    };
    Some(Listed {
        id: item.key.parse().ok()?,
        title: item.title.clone(),
        state,
        branch: text("head"),
        head_commit: item.details["head_commit"].as_str().map(str::to_owned),
        base: text("base"),
        url: item.url.clone().unwrap_or_default(),
        description: text("description"),
        own: true,
        updated: item
            .updated_at
            .as_deref()
            .and_then(crate::session_read::form),
        reviewers: item.details["reviewers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                (
                    r["name"].as_str().unwrap_or_default().to_owned(),
                    verdict(r["state"].as_str().unwrap_or_default()),
                    r["handle"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
    })
}

/// Reads the checks, reviewers and comments of a listed pull request, as `forge show` reads
/// them; false when Bitbucket no longer has it. A part of it not found (its checks, its
/// comments) while the pull request itself is found leaves it not `detailed`: what is stored of
/// it is kept, and it is said.
pub(super) async fn detail(
    forge: &Forge,
    context: &str,
    fetched: &mut crate::items::Fetched,
) -> Result<bool, String> {
    let Some(found) = listed_of(&fetched.item) else {
        return Ok(true);
    };
    let id = found.id;
    let (pull, statuses) = match detailed_with(forge, context, found).await {
        Ok(read) => read,
        Err(error) if super::not_found(&error) => {
            let page = match cloud(forge) {
                true => cloud_page(forge, &id.to_string()),
                false => {
                    let Forge::Bitbucket { owner, name, .. } = forge else {
                        unreachable!()
                    };
                    format!("/rest/api/1.0/projects/{owner}/repos/{name}/pull-requests/{id}")
                }
            };
            return match json(&["api", &page, "--context", context]).await {
                Ok(_) => {
                    eprintln!(
                        "flottille daemon: items: a part of the pull request #{id} of {} was not \
                         found ({error}); what is kept of it stays",
                        forge.slug()
                    );
                    Ok(true)
                }
                Err(gone) if super::not_found(&gone) => Ok(false),
                Err(other) => Err(other),
            };
        }
        Err(error) => return Err(error),
    };
    let version = fetched.item.details["version"].take();
    let read = super::fetched_pull(&fetched.item.repository, "bitbucket", &pull, true);
    fetched.item.details = read.item.details;
    if !version.is_null() {
        fetched.item.details["version"] = version;
    }
    fetched.item.checks = checks_of_statuses(&statuses);
    fetched.comments = read.comments;
    fetched.reviews = read.reviews;
    fetched.detailed = true;
    Ok(true)
}

/// The checks of the pull request `number` now.
pub(super) async fn checks_now(
    forge: &Forge,
    context: &str,
    number: u64,
) -> Result<Vec<flottille_core::item::Check>, String> {
    let statuses = statuses_of(forge, context, &number.to_string()).await?;
    Ok(checks_of_statuses(&statuses))
}

/// Commit statuses as the items keep them: when each was made, when it last changed once it
/// ended, and its link.
fn checks_of_statuses(statuses: &[Value]) -> Vec<flottille_core::item::Check> {
    let time = |status: &Value, keys: [&str; 2]| keys.iter().find_map(|key| time_of(&status[*key]));
    statuses
        .iter()
        .map(|status| {
            let (name, state) = check_of(status);
            flottille_core::item::Check {
                name,
                state: state.into(),
                started_at: time(status, ["created_on", "dateAdded"]),
                ended_at: match state {
                    "pending" => None,
                    _ => time(status, ["updated_on", "dateAdded"]),
                },
                link: status["url"]
                    .as_str()
                    .filter(|link| !link.is_empty())
                    .map(masked),
                outcome: (state == "failing")
                    .then(|| status["state"].as_str().map(str::to_ascii_lowercase))
                    .flatten(),
            }
        })
        .collect()
}

/// The display name Bitbucket Cloud knows the signed-in user by (`/user`, assumed from
/// Atlassian's REST documentation).
pub(super) async fn me(context: &str) -> Option<String> {
    let user = json(&["api", "/user", "--context", context]).await.ok()?;
    user["display_name"].as_str().map(masked)
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

/// The commit statuses of the pull request `id`, as `bkt pr checks --json` gives them, on Cloud
/// only the ones that count (`counted`).
async fn statuses_of(forge: &Forge, context: &str, id: &str) -> Result<Vec<Value>, String> {
    let said = json(&command(forge, context, &["pr", "checks", id], &["--json"])).await?;
    let statuses = match said.get("statuses") {
        // A commit with no status: `bkt` answers `null` (a nil Go slice), and no check ran.
        Some(Value::Null) => Vec::new(),
        Some(Value::Array(statuses)) => statuses.clone(),
        // Without its `statuses` (or in another shape), `bkt` said nothing about them.
        _ => {
            return Err(format!(
                "the checks of the pull request #{id} could not be read: `bkt pr checks --json` \
                 answered without `statuses`"
            ));
        }
    };
    if statuses
        .iter()
        .any(|s| build_of(forge, s["url"].as_str()).is_some())
        && let Some(pipelines) = recent_pipelines(forge, context).await
    {
        return Ok(counted(forge, statuses, &pipelines));
    }
    Ok(statuses)
}

/// The newest 50 pipelines of a Cloud repository, as the API gives them; none on Data Center and
/// when they cannot be read, which leaves the statuses as `bkt` gave them.
async fn recent_pipelines(forge: &Forge, context: &str) -> Option<Vec<Value>> {
    if !cloud(forge) {
        return None;
    }
    let path = format!("{}/pipelines/", cloud_repository(forge));
    let (pipelines, _) = cloud_values_with(context, &path, PAGE, &["sort=-created_on"])
        .await
        .ok()?;
    Some(pipelines)
}

/// Which pipeline a build is of, so that a rerun is told from another pipeline: the pull request's
/// own (by its destination and the pattern it was selected by), or any other (a branch's push, a
/// tag, a custom one: by its target and its pattern). A pull request's pipeline has a destination
/// of its own: one branch can be the source of two pull requests.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Pipeline {
    target: String,
    destination: String,
    pattern: String,
}

fn pipeline_of(pipeline: &Value) -> Pipeline {
    let target = &pipeline["target"];
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let selector = &target["selector"];
    let kind = match target["type"].as_str() {
        Some("pipeline_pullrequest_target") => "pull request".to_owned(),
        // A run started by `bkt pipeline run --selector-type pull-requests`, which is how Flottille
        // runs a pull request's pipeline again: a branch run, without a destination.
        _ if selector["type"] == "pull-requests" => "pull request rerun".to_owned(),
        other => format!(
            "{} {} {} {}",
            other.unwrap_or_default(),
            text(&target["ref_type"]),
            text(&target["ref_name"]),
            text(&selector["type"])
        ),
    };
    Pipeline {
        target: kind,
        destination: text(&target["destination"]),
        pattern: text(&selector["pattern"]),
    }
}

/// The statuses that count on the pull request: every pipeline that ran on its head counts, a
/// branch's own as well (Bitbucket's own merge check counts every status of the source commit), but
/// only the latest build of each. A status that no pipeline of the list explains (another CI's, or a
/// build older than the 50 newest pipelines) is kept.
///
/// A rerun is a new build of the same pipeline with new statuses, and the earlier build's stay on
/// the commit: they are what made a fixed run still count as failed. A run that `forge` `checks`
/// started again has no destination: it replaces the run of the one pull request pipeline of its
/// pattern, and with two (two destinations) it is not known which it ran for, so it replaces
/// none and the failed one still counts.
fn counted(forge: &Forge, statuses: Vec<Value>, pipelines: &[Value]) -> Vec<Value> {
    let known: Vec<Option<(u64, Pipeline)>> = statuses
        .iter()
        .map(|status| {
            let build = build_of(forge, status["url"].as_str())?;
            let found = pipelines
                .iter()
                .find(|p| p["build_number"].as_u64() == Some(build))?;
            Some((build, pipeline_of(found)))
        })
        .collect();
    let own = |pattern: &str| {
        let destinations: std::collections::HashSet<&str> = known
            .iter()
            .flatten()
            .filter(|(_, p)| p.target == "pull request" && p.pattern == pattern)
            .map(|(_, p)| p.destination.as_str())
            .collect();
        (destinations.len() == 1).then(|| destinations.into_iter().next().unwrap().to_owned())
    };
    let keyed: Vec<Option<(u64, Pipeline)>> = known
        .iter()
        .map(|known| {
            let (build, mut pipeline) = known.clone()?;
            if pipeline.target == "pull request rerun"
                && let Some(destination) = own(&pipeline.pattern)
            {
                pipeline.target = "pull request".to_owned();
                pipeline.destination = destination;
            }
            Some((build, pipeline))
        })
        .collect();
    let mut latest: std::collections::HashMap<&Pipeline, u64> = Default::default();
    for (build, pipeline) in keyed.iter().flatten() {
        let newest = latest.entry(pipeline).or_default();
        *newest = (*newest).max(*build);
    }
    statuses
        .iter()
        .zip(&keyed)
        .filter(|(_, keyed)| keyed.as_ref().is_none_or(|(build, p)| latest[p] == *build))
        .map(|(status, _)| status.clone())
        .collect()
}

/// How long after a pull request last changed its head may still be waiting for its pipeline: past
/// it, a head with no check is one nothing ran for.
const STARTING: u64 = 10 * 60 * 1000;

/// The short commit whose checks have not started yet: the head of a Cloud pull request that
/// changed a moment ago, in a repository that runs pipelines (one of its newest pipelines exists),
/// with no check yet. None when it is rather that none ran.
async fn not_started(forge: &Forge, context: &str, head: &str, updated: u64) -> Option<String> {
    if !cloud(forge) || flottille_core::store::now_ms().saturating_sub(updated) > STARTING {
        return None;
    }
    let pipelines = recent_pipelines(forge, context).await?;
    (!pipelines.is_empty()).then(|| head.chars().take(7).collect())
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
pub(super) fn cloud_repository(forge: &Forge) -> String {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!()
    };
    format!("/repositories/{owner}/{name}")
}

/// The branch a pipeline of Bitbucket's API ran for: the branch's own (`ref_name`), or the source
/// branch of the pull request.
pub(super) fn pipeline_branch(pipeline: &Value) -> Option<&str> {
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

/// What a Cloud build says: its failed step's log, and how it ended when that was not the code.
pub(super) struct Built {
    /// The uuid of the first failed step and its log, or why there is none.
    pub log: Result<(String, String), String>,
    /// A step in `ERROR` (Bitbucket's system error) or a pipeline that ended outside any step is
    /// the CI's, not the code's; a pipeline that was stopped, by whom when it says it.
    pub ending: Option<Ending>,
    /// The call that runs the build's pipeline again, when the build failed (`forge` `checks`
    /// refuses a pipeline that did not) and the tool accepts a call for that pipeline.
    pub rerun: Option<Rerun>,
}

/// The log of the first failed step of the build `build`: `bkt pipeline view` names the steps,
/// `bkt pipeline logs --step` prints one. The step's uuid and its log, masked.
pub(super) async fn failed_step_log(forge: &Forge, context: &str, build: u64) -> Built {
    let build = build.to_string();
    let seen = match json(&command(
        forge,
        context,
        &["pipeline", "view", &build],
        &["--json"],
    ))
    .await
    {
        Ok(seen) => seen,
        Err(why) => {
            return Built {
                log: Err(why),
                ending: None,
                rerun: None,
            };
        }
    };
    fn result(step: &Value) -> Option<&str> {
        step["state"]["result"]["name"]
            .as_str()
            .or(step["result"]["name"].as_str())
    }
    let failed = seen["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|step| matches!(result(step), Some("FAILED" | "ERROR")));
    // The uuid goes to a command line: only what a uuid is made of.
    let uuid = failed
        .and_then(|step| step["uuid"].as_str())
        .filter(|uuid| {
            uuid.chars()
                .all(|c| c.is_ascii_hexdigit() || "{}-".contains(c))
        });
    let ending = match failed {
        Some(step) if result(step) == Some("ERROR") => Some(Ending::Ci(format!(
            "the step {} ended in error",
            masked(step["name"].as_str().unwrap_or("?"))
        ))),
        Some(_) => None,
        None => match seen["pipeline"]["state"]["result"]["name"].as_str() {
            Some("STOPPED") => Some(Ending::Stopped(
                seen["pipeline"]["state"]["result"]["terminator"]["display_name"]
                    .as_str()
                    .map(masked),
            )),
            _ => Some(Ending::Ci(format!(
                "the pipeline #{build} ended outside any step"
            ))),
        },
    };
    let rerun = rerun_of(&seen["pipeline"]);
    let Some(uuid) = uuid else {
        return Built {
            log: Err(format!("the pipeline #{build} has no failed step")),
            ending,
            rerun,
        };
    };
    let log = text(&command(
        forge,
        context,
        &["pipeline", "logs", &build],
        &["--step", uuid],
    ))
    .await
    .map(|log| (uuid.to_owned(), log));
    Built { log, ending, rerun }
}

/// The call that runs the failed `pipeline` again. Bitbucket runs a whole pipeline, never a step.
/// A pull request's pipeline (`bkt` starts one only as the branch run `forge` `checks` makes);
/// another one (a branch's push, a custom one) is `forge` `run` of its selector on its branch.
/// None when it did not fail (`checks` refuses it), or when `forge` `run` would refuse it or run
/// something else: a tag's pipeline, which `bkt pipeline run` starts as a branch run.
fn rerun_of(pipeline: &Value) -> Option<Rerun> {
    if !matches!(
        pipeline["state"]["result"]["name"].as_str(),
        Some("FAILED" | "ERROR")
    ) {
        return None;
    }
    let target = &pipeline["target"];
    if target["type"] == "pipeline_pullrequest_target"
        || target["selector"]["type"] == "pull-requests"
    {
        return Some(Rerun::Checks);
    }
    let branch = target["ref_name"]
        .as_str()
        .filter(|b| !b.is_empty() && !b.starts_with('-'))?;
    if !matches!(target["ref_type"].as_str(), None | Some("branch")) {
        return None;
    }
    let pipeline = super::pipelines::pipeline_named(pipeline);
    super::pipelines::bitbucket_selector(&pipeline).ok()?;
    Some(Rerun::Run {
        pipeline,
        branch: branch.to_owned(),
    })
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
fn decision(reviewers: &[Reviewer]) -> Option<&'static str> {
    let any = |state| reviewers.iter().any(|(_, s, _)| *s == state);
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
fn cloud_reviewers(pull: &Value) -> Vec<Reviewer> {
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
            (
                person(&p["user"], &["display_name", "nickname"]),
                state,
                handle_of(&p["user"], &["nickname", "account_id"]),
            )
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
            handle: handle_in(&c["user"], &["nickname", "account_id"]),
            state: None,
            body: masked(c["content"]["raw"].as_str().unwrap_or_default()),
            // What says a Bitbucket bot is not read: every author is taken for a person.
            bot: false,
            at: c["created_on"].as_str().and_then(crate::session_read::form),
            id: id_of(&c["id"]),
            recorded: false,
        })
        .collect()
}

/// A comment's number, as Bitbucket gives it, to tell it from another of the same words.
fn id_of(id: &Value) -> Option<String> {
    id.as_u64().map(|id| id.to_string())
}

/// What a merged or declined Cloud pull request was closed with, from its own page: the reason it
/// was declined with (`reason`), said without an author when the page names no `closed_by`. Cloud
/// keeps no comment as the one a pull request was closed with, and gives no time of the closing to
/// tell one from the others, so no comment stands in for it. None for an open one, or with no
/// reason.
fn cloud_closing(state: &str, page: &Value) -> Option<Comment> {
    let reason = page["reason"].as_str().filter(|r| !r.trim().is_empty());
    let reason = reason.filter(|_| state != "open")?;
    Some(Comment {
        author: if page["closed_by"].is_object() {
            person(&page["closed_by"], &["display_name", "nickname"])
        } else {
            String::new()
        },
        handle: handle_in(&page["closed_by"], &["nickname", "account_id"]),
        state: None,
        body: masked(reason),
        bot: false,
        at: None,
        id: None,
        recorded: false,
    })
}

/// What a merged or declined Data Center pull request was closed with: the newest comment of whoever
/// merged or declined it, written in the minute before that activity (`createdDate`, in
/// milliseconds, on both). `activities` come newest first. None for an open one, when they wrote
/// none then, or when the activities give no time.
fn dc_closing(state: &str, activities: &[Value]) -> Option<Comment> {
    if state == "open" {
        return None;
    }
    let ended = activities
        .iter()
        .find(|a| matches!(a["action"].as_str(), Some("MERGED" | "DECLINED")))?;
    let at = ended["createdDate"].as_u64()?;
    let closer = person(&ended["user"], &["displayName", "name"]);
    activities
        .iter()
        .filter(|a| a["action"] == "COMMENTED")
        .find(|a| {
            let comment = &a["comment"];
            person(&comment["author"], &["displayName", "name"]) == closer
                && !comment["text"].as_str().unwrap_or_default().is_empty()
                && a["createdDate"]
                    .as_u64()
                    .is_some_and(|w| w <= at && at - w <= super::closing::WITHIN_MS)
        })
        .map(|a| Comment {
            author: closer.clone(),
            handle: handle_in(&ended["user"], &["name"]),
            state: None,
            body: masked(a["comment"]["text"].as_str().unwrap_or_default()),
            bot: false,
            at: a["comment"]["createdDate"].as_u64(),
            // The same comment as the one among the comments: it is told by its id.
            id: id_of(&a["comment"]["id"]),
            recorded: false,
        })
}

/// The comments among a Data Center pull request's activities, replies after what they answer.
fn dc_comments(activities: &[Value]) -> Vec<Comment> {
    fn walk(comment: &Value, out: &mut Vec<Comment>) {
        out.push(Comment {
            author: person(&comment["author"], &["displayName", "name"]),
            handle: handle_in(&comment["author"], &["name"]),
            state: None,
            body: masked(comment["text"].as_str().unwrap_or_default()),
            bot: false,
            at: comment["createdDate"].as_u64(),
            id: id_of(&comment["id"]),
            recorded: false,
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

/// `forge edit` of a Bitbucket pull request: `bkt pr edit <id> --title --description --reviewer
/// --remove-reviewer`, `bkt pr publish <id> [--undo]` for the draft, and on Cloud the destination
/// through the API.
pub(super) async fn edit(
    forge: &Forge,
    number: &str,
    asked: &ForgeCall,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<(), String> {
    let context = Contexts::default().of(forge).await?;
    let mut flags = Vec::new();
    if let Some(title) = title {
        flags.extend(["--title", title]);
    }
    if let Some(body) = body {
        flags.extend(["--description", body]);
    }
    for name in asked.reviewers_add.iter().flatten() {
        flags.extend(["--reviewer", name.as_str()]);
    }
    for name in asked.reviewers_remove.iter().flatten() {
        flags.extend(["--remove-reviewer", name.as_str()]);
    }
    let mut done = Vec::new();
    if !flags.is_empty() {
        text(&command(forge, &context, &["pr", "edit", number], &flags)).await?;
        done.push("bkt pr edit");
    }
    if let Some(draft) = asked.draft {
        let flags: &[&str] = if draft { &["--undo"] } else { &[] };
        text(&command(forge, &context, &["pr", "publish", number], flags))
            .await
            .map_err(|why| after(why, &done))?;
        done.push("bkt pr publish");
    }
    if let Some(base) = &asked.base {
        // Cloud's pull request page takes a new destination (assumed shape, `docs/forges.md`).
        let path = cloud_page(forge, number);
        let input = json!({"destination": {"branch": {"name": base}}}).to_string();
        let args = [
            "api",
            &path,
            "--context",
            &context,
            "--method",
            "PUT",
            "--input",
            &input,
        ];
        text(&args).await.map_err(|why| after(why, &done))?;
    }
    Ok(())
}

/// Whether the branch `name` exists in a Bitbucket Cloud repository: an error saying `404` when
/// it does not.
pub(super) async fn branch_exists(forge: &Forge, name: &str) -> Result<(), String> {
    let context = Contexts::default().of(forge).await?;
    let path = format!("{}/refs/branches/{name}", cloud_repository(forge));
    json(&["api", &path, "--context", &context])
        .await
        .map(|_| ())
}

/// `forge` `create`, `merge` and `checks` for a Bitbucket repository, once the gate let it
/// through and the remote was found where the workspace was created: the same actions and the
/// same checks as for GitHub, through `bkt`.
pub(super) async fn write(
    (daemon, project, path): (&Daemon, &str, &str),
    forge: &Forge,
    (branch, base): (&str, &str),
    reviewers: &[String],
    deletion: &Deletion,
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
            if asked.draft == Some(true) {
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
            let close_source = asked.delete_branch_on_merge == Some(true);
            if close_source {
                flags.push("--close-source");
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
            if let Some(number) = number {
                let number = number.to_string();
                let on = (project, "bitbucket", path, number.as_str());
                if !body.trim().is_empty() {
                    crate::posts::record_body(daemon, on, &body);
                }
                // The pull request's own "close source branch": a merge then says it again,
                // since `bkt pr merge` sends its own with every merge.
                if close_source {
                    crate::posts::record_delete_branch(daemon, on, branch);
                }
            }
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
                let starting = match (&pull.head_commit, pull.updated) {
                    (Some(head), Some(updated)) if checks.is_empty() => {
                        not_started(forge, context, head, updated).await
                    }
                    _ => None,
                };
                if let Some(blocks) = blocks(&checks, decision(&reviewers), starting.as_deref()) {
                    return Outcome::Refused(format!(
                        "refused: the pull request #{number} of {branch} in {slug} is not \
                         merged: {blocks}; wait for its checks, or have it waived by the user \
                         (waive, with their words)"
                    ));
                }
            }
            // `bkt` closes the source branch unless told not to; `gh pr merge` leaves it.
            let kept = deletion.decide(daemon, (project, "bitbucket", path, number.as_str()));
            let close = match kept {
                Branch::Delete => "--close-source=true",
                _ => "--close-source=false",
            };
            let mut flags = vec!["--strategy", strategy, close];
            if let Some(message) = &asked.message {
                flags.extend(["--message", message.as_str()]);
            }
            let args = command(forge, context, &["pr", "merge", &number], &flags);
            match text(&args).await {
                Ok(_) => Outcome::Done(format!(
                    "The pull request #{number} of {branch} was merged by {method} into {base} \
                     in {slug}{}.",
                    kept.said(branch)
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

/// The state of the pull request `number` from its own page, not from the list's newest: `open`,
/// `merged`, `declined` or `superseded`. None when it cannot be read (no context, no such page, a
/// state not known), which `forge edit` takes for a pull request it may write to.
pub(super) async fn state_now(forge: &Forge, number: u64) -> Option<&'static str> {
    page_now(forge, number).await.ok().map(|(state, _)| state)
}

/// The state of the pull request `number` from its own page, and the commit its source branch is
/// at (Cloud `source.commit.hash`, Data Center `fromRef.latestCommit`) when the page says one. An
/// error saying why when the page cannot be read or its state is not known.
pub(super) async fn page_now(
    forge: &Forge,
    number: u64,
) -> Result<(&'static str, Option<String>), String> {
    let context = Contexts::default().of(forge).await?;
    let id = number.to_string();
    let path = if cloud(forge) {
        cloud_page(forge, &id)
    } else {
        let Forge::Bitbucket { owner, name, .. } = forge else {
            unreachable!()
        };
        format!("/rest/api/1.0/projects/{owner}/repos/{name}/pull-requests/{id}")
    };
    let page = json(&["api", &path, "--context", &context]).await?;
    let state = match page["state"].as_str() {
        Some("OPEN") => "open",
        Some("MERGED") => "merged",
        Some("DECLINED") => "declined",
        Some("SUPERSEDED") => "superseded",
        _ => {
            return Err(format!(
                "the state of its page is not one it knows: {}",
                page["state"]
            ));
        }
    };
    let head = [
        &page["source"]["commit"]["hash"],
        &page["fromRef"]["latestCommit"],
    ]
    .into_iter()
    .find_map(|hash| hash.as_str().filter(|hash| !hash.is_empty()))
    .map(str::to_owned);
    Ok((state, head))
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
