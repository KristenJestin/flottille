//! `flottille exception list <session>` and `flottille exception withdraw <session> <rule>`
//! (`docs/cli.md`): the exceptions a session holds, and the user's way to take one back
//! (`docs/design.md` §7.3). They are given by `flottille approve` on the request that asked one.

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ExceptionAction {
    /// The rules of the gate you lifted for a session: what each lets it do, and why it asked
    List {
        session: String,
        #[arg(long)]
        json: bool,
    },
    /// Take one back: the rule stops the session again
    Withdraw { session: String, rule: String },
}

pub fn run(paths: &Paths, action: ExceptionAction) -> Result<()> {
    let mut client = Client::connect_or_start(paths)?;
    match action {
        ExceptionAction::List { session, json } => {
            let listed = client.request("exception.list", json!({"session": session}))?;
            if json {
                println!("{listed}");
            } else {
                print!("{}", list_text(&session, &listed));
            }
        }
        ExceptionAction::Withdraw { session, rule } => {
            client.request(
                "exception.withdraw",
                json!({"session": session, "rule": rule}),
            )?;
            println!("The exception {rule} is withdrawn: the rule stops agent {session} again.");
        }
    }
    Ok(())
}

fn list_text(session: &str, listed: &Value) -> String {
    let held = listed["exceptions"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if held.is_empty() {
        return format!("Agent {session} holds no exception.\n");
    }
    let mut text = String::new();
    for exception in held {
        text.push_str(&format!(
            "{}: {} (request #{})\n    {}\n",
            exception["rule"].as_str().unwrap_or("?"),
            exception["said"].as_str().unwrap_or("?"),
            exception["request"],
            exception["why"].as_str().unwrap_or("?"),
        ));
    }
    text.push_str("Take one back with `flottille exception withdraw <session> <rule>`.\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exception_is_listed_with_what_it_lets_and_why_it_was_asked() {
        let listed = json!({"exceptions": [{"session": "3f9a1c0b7d2e", "rule": "force-push",
            "said": "force a push of its own branch", "why": "a check refuses an old commit",
            "request": 4, "granted_at": 0}]});
        assert_eq!(
            list_text("3f9a1c0b7d2e", &listed),
            "force-push: force a push of its own branch (request #4)\n    a check refuses an \
             old commit\nTake one back with `flottille exception withdraw <session> <rule>`.\n"
        );
        assert_eq!(
            list_text("3f9a1c0b7d2e", &json!({"exceptions": []})),
            "Agent 3f9a1c0b7d2e holds no exception.\n"
        );
    }
}
