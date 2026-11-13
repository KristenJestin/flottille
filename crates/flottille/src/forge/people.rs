//! The people a repository's forge lets an agent name (#795): `forge people` lists them, and a
//! reviewer an agent gives as a display name, a nickname, an account id or an e-mail is found among
//! them before anything is sent (`docs/tools.md`, "People"). Through the user's own `gh` or `bkt`
//! with their sign-in, read only; what a command line cannot list is not listed, and said.

use serde_json::Value;

use super::bitbucket::{self, Contexts};
use super::{Checkout, Forge, declared, gh_json, gh_text, is_bot, masked};
use crate::mcp::Outcome;
use crate::people::{People, Person};

/// How many people one Bitbucket collection is read up to: the members of a workspace may be many
/// more, and the people seen on the pull requests come first.
const READ: usize = 200;

/// What a forge lets an agent name: the people, where they were looked for, and why a source
/// could not be read.
pub(crate) struct Listing {
    pub people: People,
    /// Where the people were looked for, as a refusal says it.
    pub looked: String,
    /// Whether the forge's own list of who can review was read at all: the people seen on the pull
    /// requests are no list of everyone, and a name missing from them is not refused.
    pub known: bool,
    /// Whether one of the forge's own lists was cut (a bound, a collection that holds more) or
    /// only partly what the forge holds (Data Center's direct grants), or could not be read.
    cut: bool,
    /// Why the sources that failed did.
    pub why: Vec<String>,
}

/// The people of `forge`: those seen on its pull requests first, then those its own listings give.
pub(super) async fn read(forge: &Forge) -> Listing {
    read_for(forge, "pr").await
}

/// The same for a tracker on GitHub, whose people are seen on `issue`s.
pub(crate) async fn read_for(forge: &Forge, seen_on: &str) -> Listing {
    let mut listing = Listing {
        people: People::default(),
        looked: String::new(),
        known: false,
        cut: false,
        why: Vec::new(),
    };
    match forge {
        Forge::GitHub { .. } => github(forge, seen_on, &mut listing).await,
        Forge::Bitbucket { .. } => bitbucket(forge, &mut listing).await,
    }
    listing
}

impl Listing {
    /// Whether the people are everyone the forge lists: a name that matches one of them and no
    /// other cannot be matched by another who is missing. A part of a name is only trusted then.
    pub fn complete(&self) -> bool {
        self.known && !self.cut
    }
}

/// The people a source gave (and whether it gave them all), or why it could not.
fn took(listing: &mut Listing, source: Result<(Vec<Person>, bool), String>, own: bool) {
    match source {
        Ok((found, whole)) => {
            listing.known |= own;
            listing.cut |= own && !whole;
            for person in found {
                listing.people.add(person);
            }
        }
        Err(why) => {
            listing.cut |= own;
            listing.why.push(why);
        }
    }
}

async fn github(forge: &Forge, seen_on: &str, listing: &mut Listing) {
    let slug = forge.slug();
    listing.looked =
        format!("{slug}: its assignable users, its collaborators and the people on its {seen_on}s");
    // Seen first: `gh` says a display name for those it names in a pull request, which the
    // listings of users do not.
    let repo = forge.gh_repository();
    let fields = match seen_on {
        "issue" => "author,assignees",
        _ => "author,reviews,reviewRequests,assignees",
    };
    let seen = gh_json(&[
        seen_on, "list", "-R", &repo, "--state", "all", "--limit", "100", "--json", fields,
    ])
    .await
    .map(|pulls| (github_seen(&pulls), true));
    took(listing, seen, false);
    for source in ["assignees", "collaborators"] {
        let logins = github_logins(&format!("repos/{slug}/{source}")).await;
        took(listing, logins, true);
    }
}

/// How many pages of a hundred one GitHub list is read up to.
const GITHUB_PAGES: usize = 5;

/// The logins of a GitHub user list, a page of a hundred at a time, and whether that was all of
/// them: a list that fills its last page is taken as cut.
async fn github_logins(path: &str) -> Result<(Vec<Person>, bool), String> {
    let mut found = Vec::new();
    for page in 1..=GITHUB_PAGES {
        let page = format!("page={page}");
        let said = gh_text(&[
            "api",
            path,
            "--hostname",
            declared::GITHUB,
            "-F",
            "per_page=100",
            "-F",
            &page,
            "--jq",
            ".[].login",
        ])
        .await?;
        let logins: Vec<&str> = said
            .lines()
            .map(str::trim)
            .filter(|login| !login.is_empty())
            .collect();
        found.extend(
            logins
                .iter()
                .filter(|login| !login.ends_with("[bot]"))
                .map(|login| Person::new("", &masked(login), &masked(login), &[])),
        );
        if logins.len() < 100 {
            return Ok((found, true));
        }
    }
    Ok((found, false))
}

/// The people of `gh pr list --json author,reviews,reviewRequests,assignees`: a login, and the
/// name `gh` says when the person set one.
fn github_seen(pulls: &Value) -> Vec<Person> {
    let mut found = Vec::new();
    let mut user = |who: &Value| {
        let login = who["login"].as_str().unwrap_or_default();
        if !login.is_empty() && !is_bot(who) {
            let name = masked(who["name"].as_str().unwrap_or_default());
            found.push(Person::new(&name, &masked(login), &masked(login), &[]));
        }
    };
    for pull in pulls.as_array().into_iter().flatten() {
        user(&pull["author"]);
        for review in pull["reviews"].as_array().into_iter().flatten() {
            user(&review["author"]);
        }
        for who in pull["reviewRequests"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(pull["assignees"].as_array().into_iter().flatten())
        {
            user(who);
        }
    }
    found
}

async fn bitbucket(forge: &Forge, listing: &mut Listing) {
    let Forge::Bitbucket { owner, name, .. } = forge else {
        return;
    };
    let context = match Contexts::default().of(forge).await {
        Ok(context) => context,
        Err(why) => {
            listing.why.push(why);
            return;
        }
    };
    let context = context.as_str();
    let pulls = bitbucket::json(&bitbucket::command(
        forge,
        context,
        &["pr", "list"],
        &["--state", "all", "--limit", "50", "--json"],
    ))
    .await
    .map(|said| (seen(forge, &said["pull_requests"]), true));
    if bitbucket::cloud(forge) {
        listing.looked = format!(
            "{owner}/{name}: the members of the workspace {owner}, its default reviewers and the \
             people on its pull requests"
        );
        took(listing, pulls, false);
        let places = [
            format!("/workspaces/{owner}/members"),
            format!("/repositories/{owner}/{name}/default-reviewers"),
            format!("/repositories/{owner}/{name}/effective-default-reviewers"),
        ];
        for path in places {
            let read = bitbucket::cloud_values_with(context, &path, READ, &[])
                .await
                .map(|(values, cut)| (values.iter().filter_map(cloud_user).collect(), !cut));
            took(listing, read, true);
        }
        return;
    }
    listing.looked = format!(
        "{owner}/{name}: the users with access to the repository and the people on its pull \
         requests"
    );
    took(listing, pulls, false);
    // The repository's own permissions (which its administrators read), else the users with at
    // least the right to read it. The first holds the direct grants only (a group's members, a
    // project's or the instance's are not in it), so it is never the whole list.
    let own = format!("/rest/api/1.0/projects/{owner}/repos/{name}/permissions/users");
    let mut read = bitbucket::dc_values_cut(context, &own, READ, &[])
        .await
        .map(|(values, _)| (values.iter().filter_map(data_center_user).collect(), false));
    if read.is_err() {
        // The permission filter of `GET /rest/api/1.0/users`, `permission.1=REPO_READ` with the
        // `projectKey` and `repositorySlug` of the repository, as Atlassian's REST reference of
        // Data Center gives it (`docs/forges.md`, "People").
        let params = [
            "permission.1=REPO_READ",
            &format!("permission.1.projectKey={owner}"),
            &format!("permission.1.repositorySlug={name}"),
        ];
        let users = bitbucket::dc_values_cut(context, "/rest/api/1.0/users", READ, &params).await;
        match users {
            Ok((values, cut)) => {
                read = Ok((values.iter().filter_map(data_center_user).collect(), !cut))
            }
            Err(why) => listing.why.push(why),
        }
    }
    took(listing, read, true);
}

/// The people of the pull requests `bkt pr list` gave: a Cloud pull request's author, a Data
/// Center one's author, reviewers and participants.
fn seen(forge: &Forge, pulls: &Value) -> Vec<Person> {
    let mut found = Vec::new();
    for pull in pulls.as_array().into_iter().flatten() {
        if bitbucket::cloud(forge) {
            found.extend(cloud_user(&pull["author"]));
        } else {
            found.extend(data_center_user(&pull["author"]["user"]));
            for who in ["reviewers", "participants"] {
                for participant in pull[who].as_array().into_iter().flatten() {
                    found.extend(data_center_user(&participant["user"]));
                }
            }
        }
    }
    found
}

/// A Bitbucket Cloud user, as a member, a default reviewer or an author says it: its own object, or
/// wrapped in `user`. It is named by its UUID (`{…}`), which `bkt pr create --reviewer` and
/// `--remove-reviewer` both take; a nickname alone is no longer a name Cloud's API takes.
fn cloud_user(value: &Value) -> Option<Person> {
    let user = match value["user"].is_object() {
        true => &value["user"],
        false => value,
    };
    let text = |key: &str| masked(user[key].as_str().unwrap_or_default().trim());
    let (uuid, account) = (text("uuid"), text("account_id"));
    let bare = uuid.trim_matches(['{', '}']).to_owned();
    let handle = [
        text("nickname"),
        text("username"),
        account.clone(),
        uuid.clone(),
    ]
    .into_iter()
    .find(|handle| !handle.is_empty())?;
    let send = [uuid.clone(), account.clone(), handle.clone()]
        .into_iter()
        .find(|send| !send.is_empty())?;
    Some(Person::new(
        &text("display_name"),
        &handle,
        &send,
        &[&text("nickname"), &account, &uuid, &bare],
    ))
}

/// A Data Center user: `name` is the login `bkt` takes, `slug` its address form.
fn data_center_user(value: &Value) -> Option<Person> {
    let user = match value["user"].is_object() {
        true => &value["user"],
        false => value,
    };
    let text = |key: &str| masked(user[key].as_str().unwrap_or_default().trim());
    let login = text("name");
    if login.is_empty() {
        return None;
    }
    Some(Person::new(
        &text("displayName"),
        &login,
        &login,
        &[&text("slug"), &text("emailAddress")],
    ))
}

/// `forge people {repository?, query?}`: who can be named for a reviewer in the place's
/// repositories, those seen on the pull requests first.
pub(super) async fn people(checkouts: &[Checkout], query: Option<&str>) -> Outcome {
    let mut text = String::from("The people who can be named as a reviewer");
    if let Some(query) = query {
        text.push_str(&format!(", matching {query:?}"));
    }
    text.push_str(":\n");
    for checkout in checkouts {
        let forge = match super::declared_forge(checkout).await {
            Ok(forge) => forge,
            Err(None) => {
                text.push_str(&format!("\n{}: {}.\n", checkout.path, super::NO_FORGE));
                continue;
            }
            Err(Some(why)) => {
                text.push_str(&format!("\n{}: {why}.\n", checkout.path));
                continue;
            }
        };
        if matches!(forge, Forge::GitHub { .. })
            && let Err(problem) = super::signed_in().await
        {
            text.push_str(&format!("\n{}: {problem}.\n", checkout.path));
            continue;
        }
        let listing = read(&forge).await;
        if !listing.known && listing.people.is_empty() {
            text.push_str(&format!(
                "\n{}: its people cannot be listed: {}.\n",
                checkout.path,
                listing.why.join("; ")
            ));
            continue;
        }
        let found: Vec<&Person> = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(query) => listing.people.matching(query),
            None => listing.people.all().iter().collect(),
        };
        text.push_str(&format!(
            "\n{} ({} in {}):\n",
            checkout.path,
            found.len(),
            listing.looked
        ));
        text.push_str(&crate::people::lines(&found));
        if !listing.known {
            text.push_str(&format!(
                "Only the people on its pull requests: the forge's own list could not be read ({}).\n",
                listing.why.join("; ")
            ));
        }
    }
    text.push_str(
        "\nA reviewer is named by display name, nickname, account id or e-mail: \
         forge {\"action\": \"edit\", \"number\": …, \"reviewers_add\": [\"Jane Doe\"]}.",
    );
    Outcome::Done(text)
}

/// A reviewer an agent named, found: what the forge's command line takes for them, and how a
/// result says them.
pub(super) struct Named {
    pub send: String,
    pub said: String,
}

/// The reviewers `names` are, for `key` (`reviewers`, `reviewers_add`, `reviewers_remove`), found
/// among the people of `forge` before anything is sent. A forge whose own list could not be read
/// sends a name of one word as it was given, as before, and refuses one with spaces: that is a
/// display name, which `gh` and `bkt` take for nothing.
pub(super) async fn named(
    forge: &Forge,
    asked: &[(&str, &[String])],
) -> Result<Vec<Vec<Named>>, Outcome> {
    if asked.iter().all(|(_, names)| names.is_empty()) {
        return Ok(asked.iter().map(|_| Vec::new()).collect());
    }
    let listing = read(forge).await;
    let mut all = Vec::new();
    for (key, names) in asked {
        let mut found = Vec::new();
        for given in *names {
            if !listing.known {
                if given.contains(char::is_whitespace) {
                    return Err(Outcome::Refused(format!(
                        "refused: {key}: {given:?} is a name with spaces, which {} takes for \
                         nobody, and the forge's people cannot be listed to find them ({}); give \
                         their identifier; nothing was written",
                        match forge {
                            Forge::GitHub { .. } => "gh",
                            Forge::Bitbucket { .. } => "bkt",
                        },
                        listing.why.join("; ")
                    )));
                }
                found.push(Named {
                    send: given.clone(),
                    said: given.clone(),
                });
                continue;
            }
            match listing.people.find(given, listing.complete()) {
                Ok(person) => found.push(Named {
                    send: person.send.clone(),
                    said: person.said(),
                }),
                Err(miss) => {
                    return Err(Outcome::Refused(format!(
                        "{}; nothing was written",
                        miss.refusal(key, given, &listing.looked)
                    )));
                }
            }
        }
        all.push(found);
    }
    Ok(all)
}

/// What an answer says of the reviewers it asked: `Reviewers: added Jane Doe (jdoe); removed …`.
pub(super) fn told(added: &[Named], removed: &[Named]) -> String {
    let said = |list: &[Named]| {
        list.iter()
            .map(|n| n.said.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match (added.is_empty(), removed.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!(" Reviewers: added {}.", said(added)),
        (true, false) => format!(" Reviewers: removed {}.", said(removed)),
        (false, false) => format!(
            " Reviewers: added {}; removed {}.",
            said(added),
            said(removed)
        ),
    }
}
