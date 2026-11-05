//! The comment a merged or closed pull request was closed with, for `forge show`
//! (`docs/forges.md`, "The closing comment"): the one its closer wrote just before closing it
//! (`gh pr close --comment`, `bkt pr decline --comment`), not any comment they wrote earlier.
//! GitHub gives neither a closing comment nor, for a closed pull request, who closed it, so the
//! closer is read from the issue events; Bitbucket's is found among what its detail already read
//! (`bitbucket`).

use std::sync::atomic::{AtomicBool, Ordering};

use flottille_core::declared;

use super::{Comment, Forge, Pull, gh_text, masked};

/// How long before the closing a comment is still the one it was closed with.
pub(super) const WITHIN_MS: u64 = 60_000;

/// A login without what GitHub adds to an app's: `gh` says `app/stale` where the events say
/// `stale[bot]`.
fn bare(login: &str) -> &str {
    let login = login.strip_prefix("app/").unwrap_or(login);
    login.strip_suffix("[bot]").unwrap_or(login)
}

/// The last comment `closer` wrote in the minute before `ended` (milliseconds), among `comments`
/// (oldest first). None when they wrote none then, or when the forge gives no time.
fn written_before(comments: &[Comment], closer: &str, ended: u64) -> Option<Comment> {
    comments
        .iter()
        .rev()
        .find(|c| {
            bare(&c.author) == bare(closer)
                && !c.body.is_empty()
                && c.at
                    .is_some_and(|at| at <= ended && ended - at <= WITHIN_MS)
        })
        .cloned()
}

/// The last `merged` or `closed` event of an issue's events as `tab`-separated lines (`event`,
/// the actor's login, the time): who ended the pull request and when, in milliseconds.
fn last_ended(lines: &str) -> Option<(String, u64)> {
    let line = lines.lines().rfind(|line| !line.trim().is_empty())?;
    let mut fields = line.split('\t');
    let (_, login, at) = (fields.next()?, fields.next()?, fields.next()?);
    let at = crate::session_read::form(at)?;
    (!login.is_empty()).then(|| (login.to_owned(), at))
}

/// Whether an unreadable event list was already said in the daemon's log.
static SAID: AtomicBool = AtomicBool::new(false);

/// Tells `pull`, when it is a merged or closed one of GitHub, the comment it was closed with. Its
/// closer is the merger `gh pr view` named, else the actor of the issue's last `merged` or
/// `closed` event (one `gh api` call, made for these alone). Best effort: when the events cannot
/// be read, nothing is said of a closing comment and the rest of the pull request is shown as
/// usual, the daemon's log saying it once. None when there is nothing to tell (`pull.closing` is
/// left as it is); else what to set it to.
pub(super) async fn of(forge: &Forge, pull: &Pull) -> Option<Option<Comment>> {
    let Forge::GitHub { owner, name } = forge else {
        return None;
    };
    if !matches!(pull.state, "merged" | "closed") {
        return None;
    }
    let ended = match pull.ended.clone() {
        Some(ended) => ended,
        None => {
            let path = format!(
                "repos/{owner}/{name}/issues/{}/events?per_page=100",
                pull.number
            );
            let keep = r#".[] | select(.event == "merged" or .event == "closed") | [.event, (.actor.login // ""), .created_at] | @tsv"#;
            let args = [
                "api",
                &path,
                "--hostname",
                declared::GITHUB,
                "--paginate",
                "--jq",
                keep,
            ];
            match gh_text(&args).await {
                Ok(lines) => match last_ended(&lines) {
                    Some(ended) => ended,
                    None => return None,
                },
                Err(why) => {
                    if !SAID.swap(true, Ordering::Relaxed) {
                        eprintln!("flottille daemon: forge: closing comment not read: {why}");
                    }
                    return None;
                }
            }
        }
    };
    let (closer, at) = ended;
    Some(written_before(&pull.comments, &masked(&closer), at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(author: &str, at: u64) -> Comment {
        Comment {
            author: author.into(),
            state: None,
            body: "text".into(),
            bot: false,
            at: Some(at),
            id: None,
            recorded: false,
        }
    }

    #[test]
    fn an_app_is_the_same_whether_gh_or_the_events_name_it() {
        assert_eq!(bare("app/stale"), bare("stale[bot]"));
        assert_eq!(bare("alice"), "alice");
    }

    #[test]
    fn only_a_comment_of_the_closer_in_the_minute_before_is_a_closing_comment() {
        let comments = [
            said("bob", 1_000_000),
            said("alice", 1_000_000 + 3_000_000),
            said("bob", 4_000_000 - 1_000),
            said("bob", 4_000_000 + 1_000),
        ];

        let closing = written_before(&comments, "bob", 4_000_000);

        assert_eq!(closing.map(|c| c.at), Some(Some(4_000_000 - 1_000)));
        assert!(written_before(&comments[..2], "bob", 4_000_000).is_none());
        assert!(written_before(&comments, "stale[bot]", 4_000_000).is_none());
    }

    #[test]
    fn the_last_event_that_ended_the_pull_request_is_the_closer() {
        let lines = "closed\talice\t2026-01-01T10:00:00Z\nmerged\tbob\t2026-01-02T10:00:00Z\n";

        assert_eq!(
            last_ended(lines).map(|(login, _)| login),
            Some("bob".to_owned())
        );
        assert_eq!(last_ended(""), None);
    }
}
