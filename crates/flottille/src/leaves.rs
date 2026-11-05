//! The user's leaves for a session to write to another it did not start (`session_send`,
//! `docs/tools.md`, "Sessions that talk to each other"): `leave.list`, what the window shows where
//! a session's rights are, and `leave.withdraw`, the user's way to take one back.

use flottille_core::approval::Leave;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    /// The leaves that name it, as the writer or as the session written to.
    session: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Withdraw {
    writer: String,
    session: String,
}

/// A session as a leave names it: its id, title and state.
fn session(daemon: &Daemon, id: &str) -> Value {
    match daemon.read(|store| store.agent(id)) {
        Ok(Some(agent)) => json!({"id": agent.id, "title": agent.title, "state": agent.state}),
        _ => json!({"id": id}),
    }
}

fn listed(daemon: &Daemon, leave: &Leave) -> Value {
    json!({"writer": session(daemon, &leave.writer), "session": session(daemon, &leave.session),
        "request": leave.request, "granted_at": leave.granted_at})
}

pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "leave.list" => {
            let List { session } = serde_json::from_value(params)?;
            if daemon.read(|store| store.agent(&session))?.is_none() {
                return Err(Failure::new(
                    "not-found",
                    format!("there is no agent {session}"),
                ));
            }
            let leaves = daemon.read(|store| store.leaves_of(&session))?;
            Ok(
                json!({"leaves": leaves.iter().map(|leave| listed(daemon, leave)).collect::<Vec<_>>()}),
            )
        }
        "leave.withdraw" => {
            let Withdraw { writer, session } = serde_json::from_value(params)?;
            match daemon.write(|store| store.withdraw_leave(&writer, &session))? {
                Some(leave) => Ok(json!({"leave": listed(daemon, &leave)})),
                None => Err(Failure::new(
                    "not-found",
                    format!("agent {writer} has no leave to write to agent {session}"),
                )),
            }
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}
