//! `tickets me` (`docs/tickets.md`, "Who the user is"): which account of the tracker is the
//! user's, as the connection already says it, so a prompt can say "my comments" without a name.
//! A read: Jira's `GET /rest/api/3/myself` with the project's vault entry (the daemon uses the
//! secret itself, it is in no answer), GitHub's `gh api user`.

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;

use super::worklog::no_entry_to;
use super::worklogs::{own_account_named, signed_in};
use crate::daemon::Daemon;
use crate::forge::{self, masked};
use crate::mcp::Outcome;

pub async fn me(daemon: &Daemon, project: &Project) -> Outcome {
    match &project.tracker {
        Some(ProjectTracker::Jira { site, .. }) => {
            let none = no_entry_to("say which account is the user's", "reads an account");
            let (jira, name, stamp) = match signed_in(daemon, project, site, none).await {
                Ok(signed) => signed,
                Err(why) => return Outcome::Refused(why),
            };
            match own_account_named(project, &jira, (&name, stamp)).await {
                Ok((account, shown)) => Outcome::Done(format!(
                    "The user's account on {site} (Jira): {shown}, account {account}.\n\
                     The comments whose author account is {account} are theirs, and a mention \
                     of them carries that account (show, detail full)."
                )),
                Err(Outcome::Failed(why)) => {
                    Outcome::Failed(format!("the user's account could not be read: {why}"))
                }
                Err(other) => other,
            }
        }
        Some(ProjectTracker::Github { .. }) => {
            if let Err(why) = forge::signed_in().await {
                return Outcome::Failed(format!("the user's account could not be read: {why}"));
            }
            match forge::gh_json(&["api", "user"]).await {
                Ok(user) => match user["login"].as_str().filter(|login| !login.is_empty()) {
                    Some(login) => Outcome::Done(format!(
                        "The user's account on GitHub: {}. The comments written by it are \
                         theirs, and @{} mentions them.",
                        masked(login),
                        masked(login)
                    )),
                    None => Outcome::Failed("gh did not say which account is signed in".into()),
                },
                Err(why) => Outcome::Failed(format!("the user's account could not be read: {why}")),
            }
        }
        None => Outcome::Refused(format!("refused: {}", super::NO_TRACKER)),
    }
}
