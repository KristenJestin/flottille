//! An entry of the vault (`docs/vault.md`): what the store keeps of a secret of the user's, its
//! metadata. The secret itself is the keyring's alone.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The handle agents use: unique, a name as `project::valid_name` takes it.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The only project whose agents see it; every project's when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub updated_at: u64,
    /// The one use that reads it (`worklog:<project>`, `connection:<name>`); any feature that
    /// names it when `None`.
    #[serde(default, rename = "for", skip_serializing_if = "Option::is_none")]
    pub bound: Option<String>,
}

/// The use of the worklog of `project`: the token its tracker logs time with.
pub fn worklog(project: &str) -> String {
    format!("worklog:{project}")
}

/// The use of the connection `name`: what its server is given.
pub fn connection(name: &str) -> String {
    format!("connection:{name}")
}

/// What the daemon says when the server of a connection asks to be signed in (a `401`), and the
/// part of what it says when the authorization server refused to refresh its tokens: the window
/// knows a sign-in is what is missing by them, and offers its button instead of a command.
pub const SIGN_IN_ASKED: &str = "the server asks to be signed in";
pub const SIGNED_OUT: &str = " is signed out (";

/// What the daemon says when a connection's server answers `401` to the secrets its vault entries
/// gave (a header, a query parameter, a cookie, basic authentication): the server refused what it
/// was given, which no sign-in changes. It names where each went and the entry, never the value.
pub const REFUSED_ENTRY: &str = "the server refused the request (401) with";

/// How the daemon begins what it says of a connection whose server did not start.
pub const NOT_STARTED: &str = "the connection could not start: ";

/// Whether what the daemon said is that the server refused the vault entries its requests carried:
/// by how it begins, so that a failure that only quotes it is none.
pub fn refused_entry(said: &str) -> bool {
    said.strip_prefix(NOT_STARTED)
        .unwrap_or(said)
        .starts_with(REFUSED_ENTRY)
}

/// Whether what the daemon said of a connection's server is that a sign-in is missing.
pub fn asks_sign_in(said: &str) -> bool {
    said.contains(SIGN_IN_ASKED) || said.contains(SIGNED_OUT)
}

/// A use as a sentence names it, or `None` when it is no use Flottille knows.
pub fn use_said(bound: &str) -> Option<String> {
    match bound.split_once(':')? {
        ("worklog", project) if !project.is_empty() => {
            Some(format!("the worklog of the project {project}"))
        }
        ("connection", name) if !name.is_empty() => Some(format!("the connection {name}")),
        _ => None,
    }
}

/// What stops working when the entry a use reads is removed while that use still names it.
pub fn use_stops(bound: &str) -> Option<String> {
    match bound.split_once(':')? {
        ("worklog", project) if !project.is_empty() => Some(format!(
            "tickets log_time can no longer log time for the project {project}"
        )),
        ("connection", name) if !name.is_empty() => {
            Some(format!("the connection {name} can no longer start"))
        }
        _ => None,
    }
}

impl Entry {
    /// Whether the agents of `project` see it.
    pub fn seen_by(&self, project: &str) -> bool {
        self.project.as_deref().is_none_or(|own| own == project)
    }

    /// Whether `bound`, the use asking for it, may read it: the use it is bound to, or any when
    /// it is bound to none. The refusal names its use, never its secret.
    pub fn usable_for(&self, bound: &str) -> Result<(), String> {
        match &self.bound {
            Some(own) if own != bound => Err(format!(
                "the vault entry {} is bound to {} and serves no other use",
                self.name,
                use_said(own).unwrap_or_else(|| own.clone())
            )),
            _ => Ok(()),
        }
    }

    /// As every client and tool shows it: its metadata, each field present.
    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "description": self.description, "url": self.url,
            "username": self.username, "project": self.project, "for": self.bound})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_entry_is_recognised_by_its_prefix_only() {
        let refused = format!(
            "the connection could not start: {REFUSED_ENTRY} the header Authorization from the \
             vault entry notes-token: check the entry's value"
        );
        assert!(refused_entry(&refused));
        assert!(refused_entry(&format!(
            "{REFUSED_ENTRY} the query parameter userToken from the vault entry notes-token"
        )));
        // A failure that only quotes it is no refusal of the entries.
        assert!(!refused_entry(&format!(
            "the connection could not start: the server said \"{REFUSED_ENTRY} nothing\""
        )));
        assert!(!refused_entry(&format!("it failed: {REFUSED_ENTRY} x")));
    }
}
