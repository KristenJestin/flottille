//! What Flottille posts for an agent carries nothing of Flottille: no marker, no footer
//! (`docs/forges.md`, "What Flottille posts is recorded"). The daemon knows its posts by what it
//! recorded in the store when the forge or the tracker took them, and a text read back is told
//! from a person's by that record.
//!
//! A comment is known by the id the forge answered when it was posted, where it answers one
//! (GitHub's link), else by a hash of its text. An item's own text (a pull request's or a
//! ticket's description) is known by the item itself.

use flottille_core::item::Post;
use flottille_core::store::now_ms;
use sha2::{Digest, Sha256};

use crate::daemon::Daemon;
use crate::forge;
use crate::items::Said;
use crate::tickets::Ticket;

/// How much of a text counts in its hash: a read cuts the long ones, not at the same place
/// everywhere, and the first words say enough.
const COUNTED_CHARS: usize = 500;
/// How much earlier than its record a comment may be read: the forge's clock is not the
/// daemon's. A comment older than the post cannot be it.
const SKEW_MS: u64 = 5 * 60_000;

/// A hash of a text, as the same words read back give it: whitespace is one space, the old mark
/// of Flottille is no part of it, and only the first [`COUNTED_CHARS`] characters count.
pub(crate) fn fingerprint(text: &str) -> String {
    let (text, _) = forge::unmarked(text);
    let words: Vec<&str> = text.split_whitespace().collect();
    let counted: String = words.join(" ").chars().take(COUNTED_CHARS).collect();
    Sha256::digest(counted.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The id of a comment as a forge's link says it: GitHub's `…#issuecomment-123` gives
/// `issuecomment-123`. None for a link with no such part.
pub(crate) fn comment_id(link: &str) -> Option<String> {
    let (_, fragment) = link.trim().rsplit_once('#')?;
    let digits = fragment.trim_start_matches(|c: char| !c.is_ascii_digit());
    let named = fragment.len() > digits.len();
    (named && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        .then(|| fragment.to_owned())
}

/// The id of the comment `gh` printed the link of, among what it printed.
pub(crate) fn comment_id_in(said: &str) -> Option<String> {
    said.split_whitespace().find_map(comment_id)
}

/// What was posted on one item.
pub(crate) struct Known(Vec<Post>);

impl Known {
    pub(crate) fn of(
        daemon: &Daemon,
        project: &str,
        source: &str,
        repository: &str,
        key: &str,
    ) -> Known {
        let posts = daemon
            .read(|store| store.posts(project, source, repository, key))
            .unwrap_or_else(|error| {
                eprintln!("flottille daemon: posts of {key}: {error}");
                Vec::new()
            });
        Known(posts)
    }

    /// Whether a comment read back is one an agent posted. With an id it is known by the id: the
    /// same words under another id are a person's. Without one (the forge gave none then or now)
    /// it is known by its text, and by its time where it has one: it was written after the post.
    pub(crate) fn comment(&self, id: Option<&str>, at: Option<u64>, text: &str) -> bool {
        if self.0.is_empty() {
            return false;
        }
        let print = fingerprint(text);
        self.0
            .iter()
            .filter(|post| post.kind == "comment")
            .any(|post| match (&post.comment_id, id) {
                (Some(posted), Some(read)) => posted == read,
                _ => {
                    post.fingerprint.as_deref() == Some(print.as_str())
                        && at.is_none_or(|at| at + SKEW_MS >= post.posted_at)
                }
            })
    }

    /// Whether the item's own text is one an agent posted.
    pub(crate) fn body(&self) -> bool {
        self.0.iter().any(|post| post.kind == "body")
    }

    /// Marks the comments of a pull request an agent posted.
    pub(crate) fn mark_pull(&self, pull: &mut forge::Pull) {
        for comment in &mut pull.comments {
            comment.recorded |= self.comment(comment.id.as_deref(), comment.at, &comment.body);
        }
    }

    /// Marks the comments and the description of a ticket an agent posted.
    pub(crate) fn mark_ticket(&self, ticket: &mut Ticket) {
        ticket.description_by_agent |= self.body();
        for comment in &mut ticket.comments {
            let at = comment.at.as_deref().and_then(crate::session_read::form);
            comment.agent |= self.comment(comment.id.as_deref(), at, &comment.body);
        }
    }

    /// Marks the comments an agent posted among those a round or a live read stores.
    pub(crate) fn mark_said(&self, said: &mut [Said]) {
        for comment in said {
            comment.by_agent |= self.comment(comment.id.as_deref(), comment.at, &comment.body);
        }
    }
}

/// Records what an agent posted, once the forge or the tracker took it. A failure to record is
/// said and changes nothing for the agent: the post is done.
fn record(daemon: &Daemon, post: Post) {
    if let Err(error) = daemon.read(|store| store.add_post(&post)) {
        eprintln!(
            "flottille daemon: the post on {} could not be recorded: {error}",
            post.key
        );
    }
}

/// A comment posted on the item `key` of `repository` (empty for Jira), with `id` where the forge
/// answered one.
pub(crate) fn record_comment(
    daemon: &Daemon,
    (project, source, repository, key): (&str, &str, &str, &str),
    id: Option<String>,
    text: &str,
) {
    record(
        daemon,
        post(project, source, repository, key, "comment", id, text),
    );
}

/// The text of the item itself (its description), posted by an agent.
pub(crate) fn record_body(
    daemon: &Daemon,
    (project, source, repository, key): (&str, &str, &str, &str),
    text: &str,
) {
    record(
        daemon,
        post(project, source, repository, key, "body", None, text),
    );
}

fn post(
    project: &str,
    source: &str,
    repository: &str,
    key: &str,
    kind: &str,
    comment_id: Option<String>,
    text: &str,
) -> Post {
    Post {
        project: project.to_owned(),
        source: source.to_owned(),
        repository: repository.to_owned(),
        key: key.to_owned(),
        kind: kind.to_owned(),
        comment_id,
        fingerprint: Some(fingerprint(text)),
        posted_at: now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(posts: Vec<Post>) -> Known {
        Known(posts)
    }

    fn posted(comment_id: Option<&str>, text: &str, at: u64) -> Post {
        Post {
            kind: "comment".into(),
            comment_id: comment_id.map(str::to_owned),
            posted_at: at,
            ..post("app", "github", "acme/app", "12", "comment", None, text)
        }
    }

    #[test]
    fn a_hash_ignores_whitespace_the_old_mark_and_what_is_past_the_counted_text() {
        assert_eq!(
            fingerprint("Rebased  on\nmain\r\n"),
            fingerprint("Rebased on main")
        );
        assert_eq!(
            fingerprint("Rebased on main\n\n<!-- flottille -->"),
            fingerprint("Rebased on main")
        );
        assert_ne!(
            fingerprint("Rebased on main"),
            fingerprint("Rebased on dev")
        );
        let long = "word ".repeat(400);
        assert_eq!(fingerprint(&long), fingerprint(&long[..COUNTED_CHARS * 2]));
    }

    #[test]
    fn a_comment_id_is_read_from_the_link_gh_prints() {
        let link = "https://github.com/acme/app/pull/12#issuecomment-501";
        assert_eq!(comment_id(link).as_deref(), Some("issuecomment-501"));
        assert_eq!(
            comment_id_in(&format!("{link}\n")).as_deref(),
            Some("issuecomment-501")
        );
        assert_eq!(comment_id("https://github.com/acme/app/pull/12"), None);
        assert_eq!(comment_id("https://github.com/acme/app/pull/12#"), None);
        assert_eq!(comment_id("https://github.com/acme/app/pull/12#top"), None);
        assert_eq!(comment_id(""), None);
    }

    #[test]
    fn a_comment_with_an_id_is_known_by_the_id_alone() {
        let known = known(vec![posted(Some("issuecomment-501"), "Rebased", 1_000)]);

        assert!(known.comment(Some("issuecomment-501"), None, "Whatever it says now"));
        assert!(!known.comment(Some("issuecomment-502"), None, "Rebased"));
    }

    #[test]
    fn a_comment_without_an_id_is_known_by_its_text_and_not_earlier_than_its_post() {
        let known = known(vec![posted(None, "Ready for review", 10_000_000)]);

        assert!(known.comment(None, None, "Ready  for review\n"));
        assert!(known.comment(None, Some(10_000_000 - 60_000), "Ready for review"));
        assert!(!known.comment(None, Some(10_000_000 - SKEW_MS - 1), "Ready for review"));
        assert!(!known.comment(None, None, "Ready for review, please"));
        // The record has an id, the read none: the text says.
        let with_id = self::known(vec![posted(Some("issuecomment-1"), "Ready", 1)]);
        assert!(with_id.comment(None, None, "Ready"));
    }

    #[test]
    fn nothing_is_known_of_an_item_nothing_was_posted_on() {
        let known = known(Vec::new());

        assert!(!known.comment(Some("issuecomment-1"), None, "Rebased"));
        assert!(!known.body());
    }
}
