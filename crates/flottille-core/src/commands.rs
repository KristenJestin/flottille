//! Commands (`docs/commands.md`): a project's named instructions, which `/name` in a session's
//! composer, or an agent's message to its child, sends to the agent as a message. A command grants
//! nothing: what its agent does goes through the gate's classes like any call (`docs/design.md`
//! §7). Not to be mixed up with `crate::command`, which splits a command line.

use serde::{Deserialize, Serialize};

/// One command of a project, as its file holds it under `[commands.<name>]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// One line: what it does.
    pub description: String,
    /// What the agent is sent.
    pub instructions: String,
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
     go through the gate's classes, and Ask or Auto decides what comes to you";

impl Command {
    /// A command of these words.
    pub fn new(description: impl Into<String>, instructions: impl Into<String>) -> Command {
        Command {
            description: description.into(),
            instructions: instructions.into(),
            grants: None,
            children: None,
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
        if self.instructions.trim().is_empty() {
            return Err(format!("{key}.instructions: must not be empty"));
        }
        Ok(())
    }
}

/// The message `/name rest` stands for, when `text` starts with the name of one of `commands`:
/// its instructions, then the rest of the text after a blank line. `None` for any other text,
/// which is sent as it is.
pub fn expanded<'a>(
    text: &str,
    commands: impl IntoIterator<Item = (&'a String, &'a Command)>,
) -> Option<(String, String)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (name, after) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(name, after)| (name, after.trim()));
    let (name, command) = commands.into_iter().find(|(known, _)| *known == name)?;
    let mut message = command.instructions.trim().to_owned();
    if !after.is_empty() {
        message.push_str("\n\n");
        message.push_str(after);
    }
    Some((name.clone(), message))
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
   blocks the stack: no pull request, checks failing or still running, a review asking for
   changes, a comment or a review left unanswered. Send the user the state of every pull request
   in one `ask_user`, what blocks first. While anything blocks, stop there, unless the user waives
   it in their answer: pass their words as `waive` to that merge, and Flottille holds the waived
   merge for their approval. Flottille refuses to merge a pull request whose checks fail or run,
   or whose review asks for changes, unless it is waived.
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
4. Otherwise call `workspace` `remove`. Flottille checks again and refuses, naming what would be
   lost: then stop, and say so. Once it is removed there is nothing left to work in: `report` at
   once, with the audit, what was removed, and what is kept (the branches with commits stay in
   the repositories and on their remotes).";

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

    #[test]
    fn slash_name_sends_the_instructions_with_the_trailing_text() {
        let commands = [(
            "ship".to_owned(),
            Command::new("Ship it", "Push, then open the PR.\n"),
        )];
        let each = || commands.iter().map(|(name, command)| (name, command));
        assert_eq!(
            expanded("/ship only the api, please", each()),
            Some((
                "ship".into(),
                "Push, then open the PR.\n\nonly the api, please".into()
            ))
        );
        assert_eq!(
            expanded("  /ship", each()),
            Some(("ship".into(), "Push, then open the PR.".into()))
        );
        assert_eq!(expanded("/shipping now", each()), None);
        assert_eq!(expanded("ship it /ship", each()), None);
        assert_eq!(expanded("/", each()), None);
    }
}
