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
}

impl Entry {
    /// Whether the agents of `project` see it.
    pub fn seen_by(&self, project: &str) -> bool {
        self.project.as_deref().is_none_or(|own| own == project)
    }

    /// As every client and tool shows it: its metadata, each field present.
    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "description": self.description, "url": self.url,
            "username": self.username, "project": self.project})
    }
}
