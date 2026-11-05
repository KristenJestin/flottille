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

/// A hash of a text, as the same words read back give it: masked (a read masks what it reads,
/// and a record hashes what was posted), the old mark of Flottille no part of it, the markup a
/// tracker rewrites (see [`plain`]) gone, whitespace one space, and only the first
/// [`COUNTED_CHARS`] characters counting.
pub(crate) fn fingerprint(text: &str) -> String {
    let masked = forge::masked(text);
    let (text, _) = forge::unmarked(&masked);
    let plain = plain(text);
    let words: Vec<&str> = plain.split_whitespace().collect();
    let counted: String = words.join(" ").chars().take(COUNTED_CHARS).collect();
    Sha256::digest(counted.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The words of a text without the markdown a tracker turns into its own format: Jira keeps the
/// words of what `acli` was given (`tickets::document_text`), not the fences, the emphasis, the
/// code marks, the heading marks nor the list bullets, which it draws again as it likes.
fn plain(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            continue;
        }
        let line = line.trim_start_matches('#').trim_start();
        let line = line.strip_prefix('>').unwrap_or(line).trim_start();
        let digits = line.trim_start_matches(|c: char| c.is_ascii_digit());
        let line = match digits.strip_prefix(['.', ')']) {
            Some(rest) if digits.len() < line.len() && rest.starts_with(' ') => rest,
            _ => line,
        };
        let line = ["- ", "* ", "+ "]
            .iter()
            .find_map(|bullet| line.strip_prefix(bullet))
            .unwrap_or(line);
        out.push_str(line);
        out.push('\n');
    }
    out.retain(|c| !matches!(c, '*' | '`'));
    out
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

/// A comment as a read gives it, to be told from the record.
pub(crate) struct Heard<'a> {
    pub(crate) id: Option<&'a str>,
    pub(crate) at: Option<u64>,
    pub(crate) text: &'a str,
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

    /// Which of the comments read back are ones an agent posted. A record with an id is claimed by
    /// the comment of that id: the same words under another id are a person's. A record without
    /// one (the forge gave none) is claimed by one comment only, the first of its words that was
    /// not written before the post, so that a person's same words, earlier or later, stay a
    /// person's.
    pub(crate) fn marks(&self, read: &[Heard]) -> Vec<bool> {
        let mut marked = vec![false; read.len()];
        let mut posts: Vec<&Post> = self
            .0
            .iter()
            .filter(|post| post.kind == "comment")
            .collect();
        if posts.is_empty() {
            return marked;
        }
        posts.sort_by_key(|post| post.posted_at);
        let mut by_text = Vec::new();
        for post in posts {
            let found = post.comment_id.as_deref().and_then(|posted| {
                read.iter()
                    .enumerate()
                    .position(|(at, heard)| !marked[at] && heard.id == Some(posted))
            });
            match found {
                Some(at) => marked[at] = true,
                None => by_text.push(post),
            }
        }
        if by_text.is_empty() {
            return marked;
        }
        // The time says which comes first, not the order the forge lists them in.
        let mut order: Vec<usize> = (0..read.len()).collect();
        order.sort_by_key(|&at| read[at].at.unwrap_or(u64::MAX));
        let prints: Vec<String> = read.iter().map(|heard| fingerprint(heard.text)).collect();
        for post in by_text {
            let claimed = order.iter().copied().find(|&at| {
                let heard = &read[at];
                // A comment of another id is that comment's, not this post's.
                let same_id = post.comment_id.is_none() || heard.id.is_none();
                !marked[at]
                    && same_id
                    && post.fingerprint.as_deref() == Some(prints[at].as_str())
                    && heard.at.is_none_or(|at| at + SKEW_MS >= post.posted_at)
            });
            if let Some(at) = claimed {
                marked[at] = true;
            }
        }
        marked
    }

    /// Whether a record carries the comment id `id`: only such a record says that comment is an
    /// agent's to change, since a record of words may be claimed by a person's same words.
    pub(crate) fn owns(&self, id: &str) -> bool {
        self.0
            .iter()
            .any(|post| post.kind == "comment" && post.comment_id.as_deref() == Some(id))
    }

    /// The id of the comment just posted with `text`, among those read right after the post: the
    /// newest of its words whose id no record holds yet.
    pub(crate) fn just_posted(&self, read: &[Heard], text: &str) -> Option<String> {
        let print = fingerprint(text);
        read.iter()
            .rev()
            .filter_map(|heard| heard.id.filter(|id| !self.owns(id)).map(|id| (id, heard)))
            .find(|(_, heard)| fingerprint(heard.text) == print)
            .map(|(id, _)| id.to_owned())
    }

    /// Whether the item's own text, as it reads now, is one an agent posted: the user may have
    /// edited it since.
    pub(crate) fn body(&self, text: &str) -> bool {
        let print = fingerprint(text);
        self.0
            .iter()
            .any(|post| post.kind == "body" && post.fingerprint.as_deref() == Some(print.as_str()))
    }

    /// Marks the comments of a pull request an agent posted, and the one it was closed with:
    /// Bitbucket gives that one apart, from the same record.
    pub(crate) fn mark_pull(&self, pull: &mut forge::Pull) {
        fn heard(comment: &forge::Comment) -> Heard<'_> {
            Heard {
                id: comment.id.as_deref(),
                at: comment.at,
                text: comment.body.as_str(),
            }
        }
        // Data Center's closing comment is one of the comments: it is that one.
        let copy_of = pull
            .closing
            .as_ref()
            .and_then(|closing| closing.id.as_deref())
            .and_then(|id| {
                pull.comments
                    .iter()
                    .position(|comment| comment.id.as_deref() == Some(id))
            });
        let mut read: Vec<Heard> = pull.comments.iter().map(heard).collect();
        if let (Some(closing), None) = (&pull.closing, copy_of) {
            read.push(heard(closing));
        }
        let marks = self.marks(&read);
        let last = read.len().saturating_sub(1);
        for (comment, mark) in pull.comments.iter_mut().zip(&marks) {
            comment.recorded |= mark;
        }
        if let Some(closing) = &mut pull.closing {
            closing.recorded |= match copy_of {
                Some(at) => marks[at],
                None => marks[last],
            };
        }
    }

    /// Marks the comments and the description of a ticket an agent posted.
    pub(crate) fn mark_ticket(&self, ticket: &mut Ticket) {
        ticket.description_by_agent |= self.body(&ticket.description);
        let times: Vec<Option<u64>> = ticket
            .comments
            .iter()
            .map(|comment| comment.at.as_deref().and_then(crate::session_read::form))
            .collect();
        let read: Vec<Heard> = ticket
            .comments
            .iter()
            .zip(times)
            .map(|(comment, at)| Heard {
                id: comment.id.as_deref(),
                at,
                text: comment.body.as_str(),
            })
            .collect();
        let marks = self.marks(&read);
        for (comment, mark) in ticket.comments.iter_mut().zip(marks) {
            comment.agent |= mark;
            comment.own = comment.id.as_deref().is_some_and(|id| self.owns(id));
        }
    }

    /// Marks the comments an agent posted among those a round or a live read stores.
    pub(crate) fn mark_said(&self, said: &mut [Said]) {
        let read: Vec<Heard> = said
            .iter()
            .map(|comment| Heard {
                id: comment.id.as_deref(),
                at: comment.at,
                text: comment.body.as_str(),
            })
            .collect();
        let marks = self.marks(&read);
        for (comment, mark) in said.iter_mut().zip(marks) {
            comment.by_agent |= mark;
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

/// A comment an agent changed: one record of its id, with the new words.
pub(crate) fn replace_comment(daemon: &Daemon, on: (&str, &str, &str, &str), id: &str, text: &str) {
    drop_comment(daemon, on, id);
    record_comment(daemon, on, Some(id.to_owned()), text);
}

/// A comment an agent deleted: its record goes, so that no later comment is taken for it.
pub(crate) fn drop_comment(daemon: &Daemon, on: (&str, &str, &str, &str), id: &str) {
    if let Err(error) = daemon.read(|store| store.drop_comment_posts(on, id)) {
        eprintln!(
            "flottille daemon: the record of the comment {id} on {} could not be dropped: {error}",
            on.3
        );
    }
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

/// The wish that the pull request `key` loses its source branch when it is merged, which GitHub
/// has no flag for (`docs/tools.md`, "The forge"). Not a post of text: the record keeps no text.
/// It is also kept under the branch's name, which is all a merge's card knows of the pull request
/// before the forge is asked for its number.
pub(crate) fn record_delete_branch(
    daemon: &Daemon,
    (project, source, repository, key): (&str, &str, &str, &str),
    branch: &str,
) {
    for key in [key, &by_branch(branch)] {
        record(
            daemon,
            post(project, source, repository, key, "delete_branch", None, ""),
        );
    }
}

fn by_branch(branch: &str) -> String {
    format!("branch:{branch}")
}

/// Whether that wish was recorded for the pull request `key`.
pub(crate) fn delete_branch_kept(
    daemon: &Daemon,
    (project, source, repository, key): (&str, &str, &str, &str),
) -> bool {
    daemon
        .read(|store| store.posts(project, source, repository, key))
        .unwrap_or_default()
        .iter()
        .any(|post| post.kind == "delete_branch")
}

/// Whether a pull request of `branch` asked, when it was opened, that the branch be deleted.
pub(crate) fn delete_branch_asked(
    daemon: &Daemon,
    (project, source, repository): (&str, &str, &str),
    branch: &str,
) -> bool {
    delete_branch_kept(daemon, (project, source, repository, &by_branch(branch)))
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

    fn heard<'a>(id: Option<&'a str>, at: Option<u64>, text: &'a str) -> Heard<'a> {
        Heard { id, at, text }
    }

    #[test]
    fn a_comment_with_an_id_is_known_by_the_id_alone() {
        let known = known(vec![posted(Some("issuecomment-501"), "Rebased", 1_000)]);

        let marks = known.marks(&[
            heard(Some("issuecomment-501"), None, "Whatever it says now"),
            heard(Some("issuecomment-502"), None, "Rebased"),
        ]);

        assert_eq!(marks, [true, false]);
    }

    #[test]
    fn a_comment_without_an_id_is_known_by_its_text_and_not_earlier_than_its_post() {
        let known = known(vec![posted(None, "Ready for review", 10_000_000)]);

        let one = |at, text| known.marks(&[heard(None, at, text)])[0];

        assert!(one(None, "Ready  for review\n"));
        assert!(one(Some(10_000_000 - 60_000), "Ready for review"));
        assert!(!one(Some(10_000_000 - SKEW_MS - 1), "Ready for review"));
        assert!(!one(None, "Ready for review, please"));
        // The record has an id, the read none: the text says.
        let with_id = self::known(vec![posted(Some("issuecomment-1"), "Ready", 1)]);
        assert!(with_id.marks(&[heard(None, None, "Ready")])[0]);
    }

    #[test]
    fn a_hash_record_matches_one_comment_only_the_first_at_or_after_the_post() {
        let known = known(vec![posted(None, "Ready", 10_000_000)]);

        // Newest first, as Data Center lists them: the order is not the time.
        let marks = known.marks(&[
            heard(Some("3"), Some(10_000_000 + 2_000), "Ready"),
            heard(Some("2"), Some(10_000_000 - 1_000), "Ready"),
            heard(Some("1"), Some(10_000_000 - SKEW_MS - 1), "Ready"),
        ]);

        assert_eq!(marks, [false, true, false]);
        // Two posts of the same words claim two comments.
        let twice = self::known(vec![
            posted(None, "Ready", 10_000_000),
            posted(None, "Ready", 11_000_000),
        ]);
        let marks = twice.marks(&[
            heard(Some("1"), Some(10_000_500), "Ready"),
            heard(Some("2"), Some(11_000_500), "Ready"),
            heard(Some("3"), Some(12_000_000), "Ready"),
        ]);
        assert_eq!(marks, [true, true, false]);
    }

    #[test]
    fn an_id_the_record_holds_never_gives_its_words_to_another_id() {
        let known = known(vec![posted(Some("issuecomment-501"), "Rebased", 1_000)]);

        // The agent's comment was deleted: a person's under another id is still a person's.
        let marks = known.marks(&[heard(Some("issuecomment-502"), None, "Rebased")]);

        assert_eq!(marks, [false]);
    }

    #[test]
    fn a_hash_is_of_the_masked_text_on_both_sides() {
        let token = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";
        let raw = format!("The key {token} was rotated");

        assert_eq!(
            fingerprint(&raw),
            fingerprint(&forge::masked(&raw)),
            "a read masks what it reads, a record hashes what was posted"
        );
        assert_ne!(fingerprint(&raw), fingerprint("The key was rotated"));
    }

    #[test]
    fn an_agent_comment_holding_a_token_shape_is_still_the_agent_s() {
        let token = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";
        let posted_text = format!("The key {token} was rotated");
        let known = known(vec![posted(None, &posted_text, 1_000)]);

        let read = forge::masked(&posted_text);

        assert!(!read.contains(token));
        assert_eq!(known.marks(&[heard(None, None, &read)]), [true]);
    }

    #[test]
    fn a_body_the_user_edited_is_the_user_s() {
        let known = known(vec![Post {
            kind: "body".into(),
            ..post("app", "jira", "", "ABC-1", "body", None, "Users stay in")
        }]);

        assert!(known.body("Users stay in"));
        assert!(known.body("Users  stay\nin"));
        assert!(!known.body("Users stay in, and out"));
        assert!(!known.body(""));
    }

    #[test]
    fn jira_s_rewriting_of_markdown_does_not_change_a_hash() {
        // `document_text` keeps the words of a Jira document: no fence, no emphasis, no code
        // mark, no heading mark, and its own list bullets.
        let posted = "## Plan\n\n**Fixed** the `login` form:\n\n```rust\nlet x = 1;\n```\n\n\
                      * first\n* second\n1) third";
        let read = "Plan\n\nFixed the login form:\n\nlet x = 1;\n\n- first\n- second\n1. third";

        assert_eq!(fingerprint(posted), fingerprint(read));
        assert_ne!(fingerprint(posted), fingerprint("Fixed the login form"));
    }

    #[test]
    fn nothing_is_known_of_an_item_nothing_was_posted_on() {
        let known = known(Vec::new());

        assert_eq!(
            known.marks(&[heard(Some("issuecomment-1"), None, "Rebased")]),
            [false]
        );
        assert!(!known.body("Rebased"));
    }
}
