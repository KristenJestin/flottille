//! The socket protocol: one JSON object per line (`docs/protocol.md`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A client's request. The id is the client's, opaque to the daemon, and comes back on the answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// What the daemon writes: the answer to a request (`id` with `result` or `error`), or an event of
/// a subscription (`event`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Message {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Event>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

/// Something the daemon changed, numbered in the order it happened and stored (`docs/events.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub name: String,
    pub data: Value,
}

/// The parameters of `events.watch`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WatchParams {
    /// Send the stored events after this number first; without it, only what is new.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
}

/// The parameters of `project.detect` and `project.add`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectFolder {
    /// Absolute: the daemon does not know where the client stands.
    pub folder: String,
    /// The project's name; a slug of the folder's name when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `project.add` only: the repositories the user confirmed, as `project.detect` listed them.
    /// The daemon refuses to register anything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repositories: Option<Vec<String>>,
}

/// The parameters of `project.show`, `project.remove` and `project.check`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectName {
    pub name: String,
}

/// The parameters of `workspace.create`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreate {
    pub project: String,
    pub name: String,
    /// The repositories it takes; the project's default set when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repositories: Option<Vec<String>>,
    /// `<branch prefix>/<name>` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Fetch nothing: base the branches on the tracking refs as they are.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offline: bool,
}

/// The parameters of `workspace.show`, `workspace.prepare` and `workspace.remove`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceName {
    pub project: String,
    pub name: String,
}

/// The parameters of `workspace.list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceList {
    /// Only this project's; every workspace when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Message {
    pub fn result(id: String, result: Value) -> Message {
        Message {
            id: Some(id),
            result: Some(result),
            ..Message::default()
        }
    }

    pub fn error(id: Option<String>, code: &str, message: impl Into<String>) -> Message {
        Message {
            id,
            error: Some(ErrorBody {
                code: code.into(),
                message: message.into(),
            }),
            ..Message::default()
        }
    }

    pub fn event(event: Event) -> Message {
        Message {
            event: Some(event),
            ..Message::default()
        }
    }
}
