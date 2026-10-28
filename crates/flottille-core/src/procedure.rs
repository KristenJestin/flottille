//! Procedures (`docs/procedures.md`): named multi-step actions of a project, run by an agent in a
//! workspace, with the grants each run gives written in the procedure and shown before it runs.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// What a procedure can grant: a fixed vocabulary, never free text. Each lets through, for the
/// agents of a run, one kind of operation the gate refuses to agents otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Grant {
    /// `git commit`, never `--amend` nor `--no-verify`.
    Commit,
    /// `git push` of the workspace's own branch to its repository's remote, never forced.
    Push,
    /// `git push --force-with-lease` of the workspace's own branch.
    PushForceWithLease,
    /// `git rebase` of the workspace's branch, never interactive nor with `--exec`.
    Rebase,
    /// `forge_write` creates the pull request of the workspace's branch.
    PrCreate,
    /// `forge_write` merges the pull request of the workspace's branch, by squash.
    PrMergeSquash,
    /// `forge_write` re-runs the failed jobs of a pipeline of the workspace's branch.
    PipelineRerun,
    /// `workspace_remove` removes the run's own workspace, as `flottille workspace remove` does.
    WorkspaceRemove,
}

impl Grant {
    pub const ALL: [Grant; 8] = [
        Grant::Commit,
        Grant::Push,
        Grant::PushForceWithLease,
        Grant::Rebase,
        Grant::PrCreate,
        Grant::PrMergeSquash,
        Grant::PipelineRerun,
        Grant::WorkspaceRemove,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Grant::Commit => "commit",
            Grant::Push => "push",
            Grant::PushForceWithLease => "push-force-with-lease",
            Grant::Rebase => "rebase",
            Grant::PrCreate => "pr-create",
            Grant::PrMergeSquash => "pr-merge-squash",
            Grant::PipelineRerun => "pipeline-rerun",
            Grant::WorkspaceRemove => "workspace-remove",
        }
    }

    pub fn of(word: &str) -> Option<Grant> {
        Grant::ALL.into_iter().find(|grant| grant.word() == word)
    }

    /// What it lets the agents of a run do, as shown before a run.
    pub fn says(self) -> &'static str {
        match self {
            Grant::Commit => "make commits (git commit)",
            Grant::Push => "push the workspace's branch to its remote (git push)",
            Grant::PushForceWithLease => {
                "force-push the workspace's branch with a lease (git push --force-with-lease)"
            }
            Grant::Rebase => "rebase the workspace's branch (git rebase)",
            Grant::PrCreate => "open the workspace's pull requests (forge_write pr-create)",
            Grant::PrMergeSquash => {
                "merge the workspace's pull requests by squash (forge_write pr-merge-squash)"
            }
            Grant::PipelineRerun => {
                "re-run the failed pipelines of the workspace's branch (forge_write \
                 pipeline-rerun)"
            }
            Grant::WorkspaceRemove => {
                "remove the workspace once nothing in it would be lost (workspace_remove)"
            }
        }
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// How a grant lets an operation through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// At once.
    Allow,
    /// Held for the user each time, as the gate holds a call it asks about.
    Ask,
}

impl Mode {
    pub fn word(self) -> &'static str {
        match self {
            Mode::Allow => "allow",
            Mode::Ask => "ask",
        }
    }

    pub fn of(word: &str) -> Option<Mode> {
        match word {
            "allow" => Some(Mode::Allow),
            "ask" => Some(Mode::Ask),
            _ => None,
        }
    }
}

/// The grants of a procedure, or of a run.
pub type Grants = BTreeMap<Grant, Mode>;

/// One procedure of a project, as its file holds it under `[procedures.<name>]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Procedure {
    /// One line: what it does.
    pub description: String,
    /// What the agent of a run is given as its first message.
    pub instructions: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub grants: Grants,
    /// Whether the children of a run's agent get the run's grants too: they do unless the file
    /// says `children = false`.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub children: bool,
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl Procedure {
    /// The problem with it, as `<key>: <what is wrong>` under its own key.
    pub fn check(&self, name: &str) -> Result<(), String> {
        let key = format!("procedures.{name}");
        if !crate::project::valid_name(name) {
            return Err(format!(
                "{key}: the name must be 1 to 64 letters, digits, dots, dashes or underscores, \
                 starting with a letter or a digit"
            ));
        }
        if self.description.trim().is_empty() {
            return Err(format!("{key}.description: must not be empty"));
        }
        if self.instructions.trim().is_empty() {
            return Err(format!("{key}.instructions: must not be empty"));
        }
        Ok(())
    }
}

/// The grants as one line: `push (allow), pr-create (ask)`, or `none`.
pub fn shown(grants: &Grants) -> String {
    if grants.is_empty() {
        return "none".into();
    }
    grants
        .iter()
        .map(|(grant, mode)| format!("{grant} ({})", mode.word()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The procedures Flottille ships, which a project adopts with `flottille procedure add
/// --template <name>` and then edits as its own: the three of `docs/design.md` §13.
pub const TEMPLATES: [&str; 3] = ["open-prs", "merge-stack", "close"];

/// The template of that name, as it is written into a project's file.
pub fn template(name: &str) -> Option<Procedure> {
    let (description, instructions, grants): (&str, &str, &[(Grant, Mode)]) = match name {
        "open-prs" => (
            "Open the pull requests of the workspace, one per repository with commits",
            OPEN_PRS,
            // One confirmation per operation: each push and each pull request is the user's.
            &[(Grant::Push, Mode::Ask), (Grant::PrCreate, Mode::Ask)],
        ),
        "merge-stack" => (
            "Merge the workspace's pull requests level by level, in dependency order",
            MERGE_STACK,
            // What running it authorizes, as the case's own command did: Flottille refuses a
            // merge its checks or a review block, unless the user waives it.
            &[
                (Grant::Rebase, Mode::Allow),
                (Grant::PushForceWithLease, Mode::Allow),
                (Grant::PrMergeSquash, Mode::Allow),
                (Grant::PipelineRerun, Mode::Allow),
            ],
        ),
        "close" => (
            "Close the workspace once nothing in it would be lost",
            CLOSE,
            &[(Grant::WorkspaceRemove, Mode::Allow)],
        ),
        _ => return None,
    };
    Some(Procedure {
        description: description.into(),
        instructions: instructions.into(),
        grants: grants.iter().copied().collect(),
        children: true,
    })
}

const OPEN_PRS: &str = "\
Open the pull requests of this workspace: one per repository whose branch has commits of its own,
none for the others. You change no file and make no commit.

1. Report before anything else. For each repository of the workspace (the list above), read, in
   that repository (`commands_run` with its `repository`):
   - uncommitted work: `git status --porcelain`;
   - commits on no remote: `git log --oneline HEAD --not --remotes`;
   - commits ahead of its base: `git log --oneline <remote>/<base>..HEAD`;
   - its pull request, if it has one already: `forge_read`.
   Send the report to the user with `ask_user`: one short block per repository, then the exact
   list of what you will do: which branch you push where, which pull request you open, with its
   title.
2. If a repository holds uncommitted work, stop there and change nothing: name the files, and say
   that the user commits them, or tells you they stay out of the pull request.
3. Otherwise, for each repository with commits ahead of its base and no open pull request, in the
   order of the list:
   - push its branch: `git push <remote> <branch>`, in that repository;
   - open its pull request: `forge_write` with `pr-create`. Its title follows the project's
     format when one is given above, else says in one line what the change does; its body says
     what changed and why, in a few lines drawn from the commits and the ticket (`ticket_read`).
   Each of these is held for the user's approval: that approval is their confirmation of that one
   operation, so do not ask for it again, and never request anything the report did not show. A
   repository with no commit of its own gets nothing.
4. Once every request is answered, `report`: per repository, what was pushed and the pull
   request's link, what was refused or left out, and why.";

const MERGE_STACK: &str = "\
Merge the pull requests of this workspace into their bases, level by level, in the order of the
dependency levels above: a level is merged only once every level before it is merged and what
follows its merges is done. The repositories of one level are merged together.

1. Check everything before merging anything. For each repository with commits of its own, read
   its pull request (`forge_read`): its review, its comments and reviews, its checks. Each of these
   blocks the stack: no pull request, checks failing or still running, a review asking for
   changes, a comment or a review left unanswered. Send the user the state of every pull request
   in one `ask_user`, what blocks first. While anything blocks, stop there, unless the user waives
   it in their answer: pass their words as `waive` to that merge, and Flottille holds the waived
   merge for their approval. Flottille refuses to merge a pull request whose checks fail or run,
   or whose review asks for changes, unless it is waived.
2. Then, level by level:
   - a branch behind its base is brought up to date first: `git fetch <remote> <base>`, `git rebase
     <remote>/<base>`, the project's checks for it (`commands_list`, `commands_run`), then `git push
     --force-with-lease <remote> <branch>`, and its checks are read again. A conflict you cannot
     resolve with certainty is `git rebase --abort` and a question to the user;
   - merge the level's pull requests with `forge_write` `pr-merge-squash`, one after the other;
   - before the next level, wait for what the project says follows a merge (its instructions, its
     references): the pipelines of the bases, the release of a library, a version to bump in its
     consumers. A pipeline that failed for a reason outside the change (a runner, the network) is
     re-run once with `forge_write` `pipeline-rerun`; one that fails again, or fails because of the
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
   (`commands_run` with its `repository`):
   - uncommitted work: `git status --porcelain`;
   - commits on no remote: `git log --oneline HEAD --not --remotes`;
   - commits of its own: `git log --oneline <remote>/<base>..HEAD`;
   - its pull request: `forge_read`.
2. Then, for each repository:
   - uncommitted work or commits on no remote: the closing stops;
   - no commit of its own: its branch is empty, and Flottille deletes it with the workspace;
   - commits of its own: its pull request must be merged; open, closed or missing, the closing
     stops.
3. If the closing stops, change nothing: send the user (`ask_user`) the audit, what keeps the
   workspace open and what they can do (commit and push, merge the pull request, or open it
   first), then `report`.
4. Otherwise call `workspace_remove`. Flottille checks again and refuses, naming what would be
   lost: then stop, and say so. Once it is removed there is nothing left to work in: `report` at
   once, with the audit, what was removed, and what is kept (the branches with commits stay in
   the repositories and on their remotes).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grant_has_its_word_and_back() {
        for grant in Grant::ALL {
            assert_eq!(Grant::of(grant.word()), Some(grant));
            let value = serde_json::to_value(grant).unwrap();
            assert_eq!(value, grant.word());
        }
        assert_eq!(Grant::of("workspace-delete"), None);
        assert_eq!(Mode::of("allow"), Some(Mode::Allow));
        assert_eq!(Mode::of("deny"), None);
    }

    #[test]
    fn each_template_is_a_valid_procedure_of_its_name_and_grants_what_it_says_it_does() {
        for name in TEMPLATES {
            let procedure = template(name).unwrap();
            procedure.check(name).unwrap();
            assert!(procedure.children, "{name}");
        }
        assert_eq!(
            shown(&template("open-prs").unwrap().grants),
            "push (ask), pr-create (ask)"
        );
        assert_eq!(
            shown(&template("merge-stack").unwrap().grants),
            "push-force-with-lease (allow), rebase (allow), pr-merge-squash (allow), \
             pipeline-rerun (allow)"
        );
        assert_eq!(
            shown(&template("close").unwrap().grants),
            "workspace-remove (allow)"
        );
        assert!(template("deploy").is_none());
    }

    #[test]
    fn grants_are_shown_in_one_line() {
        let grants = Grants::from([(Grant::PrCreate, Mode::Ask), (Grant::Push, Mode::Allow)]);
        assert_eq!(shown(&grants), "push (allow), pr-create (ask)");
        assert_eq!(shown(&Grants::new()), "none");
    }
}
