//! `flottille inbox`: what the root agents sent to the user (`docs/cli.md`).

use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn inbox(paths: &Paths, after: Option<u64>, limit: Option<usize>, json: bool) -> Result<()> {
    let listed = Client::connect_or_start(paths)?
        .request("inbox.list", json!({"after": after, "limit": limit}))?;
    if json {
        println!("{listed}");
    } else {
        print!("{}", inbox_text(&listed));
    }
    Ok(())
}

fn inbox_text(listed: &Value) -> String {
    let entries = listed["entries"].as_array().map_or(&[][..], Vec::as_slice);
    if entries.is_empty() {
        return "Nothing came for you.\n".into();
    }
    let mut out = String::new();
    for entry in entries {
        let place = match entry["workspace"].as_str() {
            Some(workspace) => format!("{}/{workspace}", entry["project"].as_str().unwrap_or("?")),
            None => entry["project"].as_str().unwrap_or("?").to_owned(),
        };
        let what = match entry["kind"].as_str() {
            Some("report") => "report of",
            Some("question") => "question of",
            _ => "failure of",
        };
        out.push_str(&format!(
            "#{} {what} agent {} ({place}):\n",
            entry["seq"],
            entry["agent"].as_str().unwrap_or("?")
        ));
        for line in entry["text"].as_str().unwrap_or_default().lines() {
            out.push_str(&format!("    {line}\n"));
        }
    }
    out.push_str(
        "Answer a question with `flottille agent send <agent> <message>`; \
         `flottille inbox --after <n>` lists what came since.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_shown_with_its_agent_its_place_and_its_text() {
        let listed = json!({"entries": [
            {"seq": 12, "at": 1, "kind": "question", "agent": "3f9a1c0b7d2e", "project": "app",
                "workspace": "login", "text": "which database?"},
            {"seq": 20, "at": 2, "kind": "failure", "agent": "0a1b2c3d4e5f", "project": "app",
                "workspace": null, "text": "it keeps failing\nafter 2 replacements"}]});
        let text = inbox_text(&listed);
        assert!(text.starts_with(
            "#12 question of agent 3f9a1c0b7d2e (app/login):\n    which database?\n\
             #20 failure of agent 0a1b2c3d4e5f (app):\n    it keeps failing\n    after 2 replacements\n"
        ));
        assert_eq!(
            inbox_text(&json!({"entries": []})),
            "Nothing came for you.\n"
        );
    }
}
