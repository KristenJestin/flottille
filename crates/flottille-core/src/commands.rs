//! Commands (`docs/commands.md`): a project's named instructions, which `/name` in a session's
//! composer, or an agent's message to its child, sends to the agent as a message. A command grants
//! nothing: what its agent does goes through the gate's classes like any call (`docs/design.md`
//! §7). Not to be mixed up with `crate::command`, which splits a command line.

use std::collections::BTreeMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};

/// One command of a project, as its file holds it under `[commands.<name>]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// One line: what it does.
    pub description: String,
    /// What the agent is sent; empty for a command that points at a file (`path`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    /// A file of the project folder whose text, after its front matter, is what the agent is
    /// sent, read each time the command is used (`docs/commands.md`, "Importing a `.claude`
    /// folder"). It takes the place of `instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Obsolete since the gate's classes: read, said, never followed, and dropped from the file
    /// by the daemon's migration (`OBSOLETE_GRANTS`).
    #[serde(default, skip_serializing)]
    pub grants: Option<toml::Value>,
    /// Obsolete with `grants`.
    #[serde(default, skip_serializing)]
    pub children: Option<toml::Value>,
}

/// What is said of a command's `grants` or `children`.
pub const OBSOLETE_GRANTS: &str = "no longer read: a command grants nothing; its agent's calls \
     go through the gate's classes, and the session's mode decides what comes to you";

impl Command {
    /// A command of these words.
    pub fn new(description: impl Into<String>, instructions: impl Into<String>) -> Command {
        Command {
            description: description.into(),
            instructions: instructions.into(),
            path: None,
            grants: None,
            children: None,
        }
    }

    /// A command whose instructions are the file `path` of the project folder.
    pub fn file(description: impl Into<String>, path: impl Into<String>) -> Command {
        Command {
            path: Some(path.into()),
            ..Command::new(description, "")
        }
    }

    /// The problem with it, as `<key>: <what is wrong>` under its own key.
    pub fn check(&self, name: &str) -> Result<(), String> {
        let key = format!("commands.{name}");
        if !crate::project::valid_name(name) {
            return Err(format!(
                "{key}: the name must be 1 to 64 letters, digits, dots, dashes or underscores, \
                 starting with a letter or a digit"
            ));
        }
        if self.description.trim().is_empty() {
            return Err(format!("{key}.description: must not be empty"));
        }
        if self.description.trim().chars().any(char::is_control) {
            return Err(format!("{key}.description: must be one line"));
        }
        match &self.path {
            Some(path) => {
                if !self.instructions.trim().is_empty() {
                    return Err(format!(
                        "{key}.instructions: a command that points at a file takes none of its \
                         own: edit the file, or give a path or instructions, not both"
                    ));
                }
                crate::project::check_path(path).map_err(|error| format!("{key}{error}"))?;
            }
            None if self.instructions.trim().is_empty() => {
                return Err(format!("{key}.instructions: must not be empty"));
            }
            None => {}
        }
        Ok(())
    }
}

/// The commands a message uses, where they stand: each `/name` whose `name` is `known`, at the
/// start of the text or after whitespace, with whitespace or the end after it, as the byte range
/// of the whole `/name`, and the name. Anything else after a slash (a path, an address, an
/// unknown name) is text.
pub fn used<'t>(text: &'t str, known: impl Fn(&str) -> bool) -> Vec<(Range<usize>, &'t str)> {
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    for (at, c) in text.char_indices() {
        let starts = previous.is_none_or(char::is_whitespace);
        previous = Some(c);
        if c != '/' || !starts {
            continue;
        }
        let rest = &text[at + 1..];
        let name = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
        if !name.is_empty() && known(name) {
            found.push((at..at + 1 + name.len(), name));
        }
    }
    found
}

/// A message that uses commands, as `expanded` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Expanded {
    /// Each command it uses, in the order of its first use, once: its name and its instructions.
    pub commands: Vec<(String, String)>,
    /// The message as it was typed, each `/name` replaced by `[command name]` at its place.
    pub marked: String,
}

/// What `text` stands for when it uses some of `commands` (`used`): the commands, their
/// instructions read once each by `instructions` (which reads the file of one that points at a
/// file; the reason it cannot is the error), and the text with each one marked. `None` for a text
/// that uses none, which is sent as it is.
pub fn expanded(
    text: &str,
    commands: &BTreeMap<String, Command>,
    mut instructions: impl FnMut(&str, &Command) -> Result<String, String>,
) -> Result<Option<Expanded>, String> {
    let found = used(text, |name| commands.contains_key(name));
    if found.is_empty() {
        return Ok(None);
    }
    let mut given: Vec<(String, String)> = Vec::new();
    let mut marked = String::new();
    let mut after = 0;
    for (range, name) in found {
        if !given.iter().any(|(known, _)| known == name) {
            let read = instructions(name, &commands[name])?.trim().to_owned();
            given.push((name.to_owned(), read));
        }
        marked.push_str(&text[after..range.start]);
        marked.push_str(&format!("[command {name}]"));
        after = range.end;
    }
    marked.push_str(&text[after..]);
    Ok(Some(Expanded {
        commands: given,
        marked,
    }))
}

/// The commands Flottille ships, which a project adopts with `flottille command add --template
/// <name>` and then edits as its own: the three of `docs/design.md` §13.
pub const TEMPLATES: [&str; 3] = ["open-prs", "merge-stack", "close"];

/// The template of that name, as it is written into a project's file.
pub fn template(name: &str) -> Option<Command> {
    let (description, instructions) = match name {
        "open-prs" => (
            "Open the pull requests of the workspace, one per repository with commits",
            OPEN_PRS,
        ),
        "merge-stack" => (
            "Merge the workspace's pull requests level by level, in dependency order",
            MERGE_STACK,
        ),
        "close" => (
            "Close the workspace once nothing in it would be lost",
            CLOSE,
        ),
        _ => return None,
    };
    Some(Command::new(description, instructions))
}

const OPEN_PRS: &str = "\
Open the pull requests of this workspace: one per repository whose branch has commits of its own,
none for the others. You change no file and make no commit.

1. Report before anything else. For each repository of the workspace (the list above), read, in
   that repository (`run` with its `repository`):
   - uncommitted work: `git status --porcelain`;
   - commits on no remote: `git log --oneline HEAD --not --remotes`;
   - commits ahead of its base: `git log --oneline <remote>/<base>..HEAD`;
   - its pull request, if it has one already: `forge` `show`.
   Send the report to the user with `ask_user`: one short block per repository, then the exact
   list of what you will do: which branch you push where, which pull request you open, with its
   title.
2. If a repository holds uncommitted work, stop there and change nothing: name the files, and say
   that the user commits them, or tells you they stay out of the pull request.
3. Otherwise, for each repository with commits ahead of its base and no open pull request, in the
   order of the list:
   - push its branch: `git push <remote> <branch>`, in that repository;
   - open its pull request: `forge` `create`. Its title follows the project's
     format when one is given above, else says in one line what the change does; its body says
     what changed and why, in a few lines drawn from the commits and the ticket (`tickets` `show`).
   Each of these may be held for the user's approval: that approval is their confirmation of that
   one operation, so do not ask for it again, and never request anything the report did not show.
   A repository with no commit of its own gets nothing.
4. Once every request is answered, `report`: per repository, what was pushed and the pull
   request's link, what was refused or left out, and why.";

const MERGE_STACK: &str = "\
Merge the pull requests of this workspace into their bases, level by level, in the order of the
dependency levels above: a level is merged only once every level before it is merged and what
follows its merges is done. The repositories of one level are merged together.

1. Check everything before merging anything. For each repository with commits of its own, read
   its pull request (`forge` `show`): its review, its comments and reviews, its checks. Each of these
   blocks the stack: no pull request, no check at all, checks failing or still running, a review
   asking for changes, a comment or a review left unanswered. Send the user the state of every
   pull request in one `ask_user`, what blocks first. While anything blocks, stop there, unless the
   user waives it in their answer: pass their words as `waive` to that merge, and Flottille holds
   the waived merge for their approval. Flottille refuses to merge a pull request no check ran on
   or whose checks were all skipped, whose checks fail or run, or whose review asks for changes,
   unless it is waived.
2. Then, level by level:
   - a branch behind its base is brought up to date first: `git fetch <remote> <base>`, `git rebase
     <remote>/<base>`, the project's checks for it (its tasks, `tasks` and `run`), then `git push
     --force-with-lease <remote> <branch>`, and its checks are read again. A conflict you cannot
     resolve with certainty is `git rebase --abort` and a question to the user;
   - merge the level's pull requests with `forge` `merge`, one after the other;
   - before the next level, wait for what the project says follows a merge (its instructions, its
     references): the pipelines of the bases, the release of a library, a version to bump in its
     consumers. A pipeline that failed for a reason outside the change (a runner, the network) is
     re-run once with `forge` `checks`; one that fails again, or fails because of the
     change, stops the stack.
   Nothing waits inside a tool call. When you must wait for something outside (a pipeline, a
   release), tell the user with `ask_user` what you wait for and where to see it, and end your
   turn; go on when you are written to.
3. Never merge with `git merge`, never merge a level before those it depends on. If something goes
   wrong halfway, stop: say what is merged, what is not, and what the user should look at.
4. `report`: what was merged, level by level, with the links; what was rebased and re-run; what is
   left, and why.";

const CLOSE: &str = "\
Close this workspace: check that nothing in it would be lost, then remove it.

1. Audit each repository of the workspace (the list above), reading in that repository
   (`run` with its `repository`):
   - uncommitted work: `git status --porcelain`;
   - commits on no remote: `git log --oneline HEAD --not --remotes`;
   - commits of its own: `git log --oneline <remote>/<base>..HEAD`;
   - its pull request: `forge` `show`.
2. Then, for each repository:
   - uncommitted work or commits on no remote: the closing stops;
   - no commit of its own: its branch is empty, and Flottille deletes it with the workspace;
   - commits of its own: its pull request must be merged; open, closed or missing, the closing
     stops.
3. If the closing stops, change nothing: send the user (`ask_user`) the audit, what keeps the
   workspace open and what they can do (commit and push, merge the pull request, or open it
   first), then `report`.
4. Otherwise the workspace can go: remove it (`workspace` `remove`). The user is asked; once they
   allow it, your commands running in it are stopped, it is removed, and you go on in the
   project's folder (main). Then `report` the audit and what its removal kept (the branches
   with commits stay in the repositories and on their remotes; the empty ones are deleted with
   it).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_template_is_a_valid_command_of_its_name_and_grants_nothing() {
        for name in TEMPLATES {
            let command = template(name).unwrap();
            command.check(name).unwrap();
            assert_eq!(command.grants, None, "{name}");
        }
        assert!(template("deploy").is_none());
    }

    fn ship_and_deploy() -> BTreeMap<String, Command> {
        BTreeMap::from([
            (
                "ship".to_owned(),
                Command::new("Ship it", "Push, then open the PR.\n"),
            ),
            (
                "deploy-preview".to_owned(),
                Command::new("Deploy a preview", "Deploy the branch.\n"),
            ),
        ])
    }

    fn own(_: &str, command: &Command) -> Result<String, String> {
        Ok(command.instructions.clone())
    }

    #[test]
    fn slash_name_sends_the_instructions_with_the_trailing_text() {
        let commands = ship_and_deploy();
        assert_eq!(
            expanded("/ship only the api, please", &commands, own),
            Ok(Some(Expanded {
                commands: vec![("ship".into(), "Push, then open the PR.".into())],
                marked: "[command ship] only the api, please".into(),
            }))
        );
        assert_eq!(
            expanded("  /ship", &commands, own).unwrap().unwrap().marked,
            "  [command ship]"
        );
        assert_eq!(expanded("/shipping now", &commands, own), Ok(None));
        // A command whose file cannot be read sends nothing: the reason is the answer.
        assert_eq!(
            expanded("/ship", &commands, |_, _| Err("it is not there".into())),
            Err("it is not there".into())
        );
        assert_eq!(expanded("/", &commands, own), Ok(None));
    }

    #[test]
    fn a_command_after_text_is_expanded_with_the_text_kept_in_order() {
        let commands = ship_and_deploy();
        assert_eq!(
            expanded("test /deploy-preview then tell me", &commands, own),
            Ok(Some(Expanded {
                commands: vec![("deploy-preview".into(), "Deploy the branch.".into())],
                marked: "test [command deploy-preview] then tell me".into(),
            }))
        );
        assert_eq!(
            expanded("ship it\n/ship", &commands, own)
                .unwrap()
                .unwrap()
                .marked,
            "ship it\n[command ship]"
        );
    }

    #[test]
    fn two_commands_keep_the_order_of_their_first_use() {
        let commands = ship_and_deploy();
        let read = expanded(
            "first /ship, no: /ship then /deploy-preview",
            &commands,
            own,
        );
        // `/ship,` is not a command: a comma follows the name.
        assert_eq!(
            read,
            Ok(Some(Expanded {
                commands: vec![
                    ("ship".into(), "Push, then open the PR.".into()),
                    ("deploy-preview".into(), "Deploy the branch.".into()),
                ],
                marked: "first /ship, no: [command ship] then [command deploy-preview]".into(),
            }))
        );
        let read = expanded("/deploy-preview /ship", &commands, own)
            .unwrap()
            .unwrap();
        assert_eq!(
            read.commands
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["deploy-preview", "ship"],
            "in the order of their first use, not the file's"
        );
    }

    #[test]
    fn a_command_used_twice_is_given_once() {
        let commands = ship_and_deploy();
        let mut read = 0;
        let got = expanded("/ship this, then /ship that", &commands, |_, command| {
            read += 1;
            Ok(command.instructions.clone())
        })
        .unwrap()
        .unwrap();
        assert_eq!(read, 1, "its file is read once");
        assert_eq!(got.commands.len(), 1);
        assert_eq!(got.marked, "[command ship] this, then [command ship] that");
    }

    #[test]
    fn a_path_or_an_unknown_name_after_a_slash_stays_text() {
        let commands = ship_and_deploy();
        for text in [
            "look at /usr/bin",
            "https://example.org/ship",
            "a/ship b",
            "/deploy now",
            "/ship/",
            "(/ship)",
        ] {
            assert_eq!(expanded(text, &commands, own), Ok(None), "{text}");
        }
        let known = |name: &str| commands.contains_key(name);
        assert_eq!(
            used("x /ship\t/deploy-preview\n/nope", known),
            [(2..7, "ship"), (8..23, "deploy-preview")]
        );
        assert_eq!(used("é /ship", known), [(3..8, "ship")]);
    }
}
