//! A workspace: one git worktree per repository it takes, on one branch, in one folder, and the
//! steps that prepare it (`docs/workspaces.md`). What the store keeps and the protocol shows.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A workspace as the store keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// The store's, never shown: a workspace is named by its project and its name.
    #[serde(skip)]
    pub id: i64,
    pub project: String,
    pub name: String,
    pub branch: String,
    /// The workspace folder, absolute.
    pub folder: PathBuf,
    /// The project's folder when the workspace was created: the main checkout steps copy from.
    pub project_folder: PathBuf,
    /// Milliseconds since the Unix epoch.
    pub created_at: u64,
    /// The project's variables as they were at creation, templates not yet filled.
    pub variables: BTreeMap<String, String>,
    /// The repositories it takes, the root one first, then in dependency order.
    pub repositories: Vec<Worktree>,
    pub steps: Vec<Step>,
}

/// One repository of a workspace and the base its branch starts from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Worktree {
    /// Relative to the project folder, `.` for the folder itself; the worktree is at the same path
    /// in the workspace folder.
    pub path: String,
    pub base: Base,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Base {
    /// The branch of the base rule that was taken.
    pub branch: String,
    pub remote: String,
    /// `refs/remotes/<remote>/<branch>`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub commit: String,
    /// Whether the tracking ref was fetched for this workspace; false when created `--offline`.
    pub fetched_now: bool,
    /// When the tracking ref was last updated, in milliseconds since the Unix epoch; unknown when
    /// git kept no trace of it.
    pub fetched_at: Option<u64>,
}

/// One step of a workspace's preparation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Its place in the preparation, from 0.
    pub position: u32,
    /// Where it is done; the workspace folder when none.
    pub repository: Option<String>,
    #[serde(flatten)]
    pub action: Action,
    pub state: StepState,
    /// What it was doing when it failed, and the last lines it wrote, masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}

/// What a step does, snapshotted from the recipe when the workspace was created.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// `git worktree add` of the step's repository.
    Worktree,
    Copy {
        globs: Vec<String>,
    },
    Link {
        path: String,
    },
    /// `except`: the top-level names of the project's repositories, never linked.
    RootEntries {
        except: Vec<String>,
    },
    /// A line, or the line of a command of the catalogue, which is then named.
    Run {
        line: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// A command-like description of what failed.
    pub doing: String,
    /// The last lines of its output, masked.
    pub output: String,
}

/// Where a workspace's preparation stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Steps are left and none failed.
    Preparing,
    /// A step failed: `workspace prepare` resumes from it.
    Failed,
    /// Every step is done or skipped.
    Ready,
}

impl StepState {
    pub fn as_str(self) -> &'static str {
        match self {
            StepState::Pending => "pending",
            StepState::Running => "running",
            StepState::Done => "done",
            StepState::Failed => "failed",
            StepState::Skipped => "skipped",
        }
    }

    pub fn parse(text: &str) -> Option<StepState> {
        [
            StepState::Pending,
            StepState::Running,
            StepState::Done,
            StepState::Failed,
            StepState::Skipped,
        ]
        .into_iter()
        .find(|state| state.as_str() == text)
    }
}

impl Workspace {
    pub fn state(&self) -> State {
        let any = |state| self.steps.iter().any(|step| step.state == state);
        if any(StepState::Failed) {
            State::Failed
        } else if any(StepState::Pending) || any(StepState::Running) {
            State::Preparing
        } else {
            State::Ready
        }
    }

    /// The folder of one of its repositories: the workspace folder itself for `.`.
    pub fn folder_of(&self, repository: &str) -> PathBuf {
        match repository {
            "." => self.folder.clone(),
            path => self.folder.join(path),
        }
    }

    /// What the protocol shows: the record, with its state.
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("a workspace is always JSON");
        value["state"] = serde_json::to_value(self.state()).expect("a state is always JSON");
        value
    }
}

impl Step {
    /// A command-like description of the step, for the events and a failure.
    pub fn doing(&self) -> String {
        let place = self.repository.as_deref().unwrap_or(".");
        match &self.action {
            Action::Worktree => format!("git worktree add {place}"),
            Action::Copy { globs } => format!("copy {} in {place}", globs.join(" ")),
            Action::Link { path } => format!("link {path} in {place}"),
            Action::RootEntries { .. } => "link the project folder's entries".into(),
            Action::Run { line, .. } => format!("{line} (in {place})"),
        }
    }
}
