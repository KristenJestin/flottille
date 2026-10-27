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
}

impl Grant {
    pub const ALL: [Grant; 7] = [
        Grant::Commit,
        Grant::Push,
        Grant::PushForceWithLease,
        Grant::Rebase,
        Grant::PrCreate,
        Grant::PrMergeSquash,
        Grant::PipelineRerun,
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
    /// Whether the children of a run's agent get the run's grants too; they get none otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub children: bool,
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
        assert_eq!(Grant::of("workspace-remove"), None);
        assert_eq!(Mode::of("allow"), Some(Mode::Allow));
        assert_eq!(Mode::of("deny"), None);
    }

    #[test]
    fn grants_are_shown_in_one_line() {
        let grants = Grants::from([(Grant::PrCreate, Mode::Ask), (Grant::Push, Mode::Allow)]);
        assert_eq!(shown(&grants), "push (allow), pr-create (ask)");
        assert_eq!(shown(&Grants::new()), "none");
    }
}
