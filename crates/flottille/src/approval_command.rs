//! `flottille approvals`, `flottille approve` and `flottille deny`: the requests that reach the
//! user, and their answers (`docs/design.md` §7).

use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn approvals(paths: &Paths, json: bool) -> Result<()> {
    let listed = Client::connect_or_start(paths)?.request("approval.list", Value::Null)?;
    if json {
        println!("{listed}");
    } else {
        print!("{}", approvals_text(&listed));
    }
    Ok(())
}

pub fn approve(paths: &Paths, number: u64, for_tree: bool) -> Result<()> {
    let decided = Client::connect_or_start(paths)?.request(
        "approval.approve",
        json!({"number": number, "for_tree": for_tree}),
    )?;
    let agent = decided["request"]["agent"].as_str().unwrap_or("?");
    if for_tree {
        println!(
            "Request #{number} is allowed, and the same call for the tree of agent {agent} from \
             now on; Flottille performs it and hands the result to the agent."
        );
    } else {
        println!(
            "Request #{number} is allowed once; Flottille performs it and hands the result to \
             agent {agent}."
        );
    }
    Ok(())
}

pub fn deny(paths: &Paths, number: u64) -> Result<()> {
    let decided =
        Client::connect_or_start(paths)?.request("approval.deny", json!({"number": number}))?;
    let agent = decided["request"]["agent"].as_str().unwrap_or("?");
    println!("Request #{number} is denied; agent {agent} is told nothing was done.");
    Ok(())
}

fn approvals_text(listed: &Value) -> String {
    let requests = listed["requests"].as_array().map_or(&[][..], Vec::as_slice);
    if requests.is_empty() {
        return "No request waits for you.\n".into();
    }
    let mut text = request_lines(requests);
    text.push_str(
        "Answer with `flottille approve <n>` (`--for-tree` for the same call from now on) or \
         `flottille deny <n>`.\n",
    );
    text
}

/// The requests, each with its call, its reasons and who forwarded it.
pub fn request_lines(requests: &[Value]) -> String {
    let mut text = String::new();
    for request in requests {
        text.push_str(&format!(
            "#{} agent {}: {} {}\n",
            request["number"],
            request["agent"].as_str().unwrap_or("?"),
            request["tool"].as_str().unwrap_or("?"),
            request["call"]
        ));
        for reason in request["reasons"].as_array().into_iter().flatten() {
            text.push_str(&format!("    {}\n", reason.as_str().unwrap_or("?")));
        }
        for forward in request["forwards"].as_array().into_iter().flatten() {
            text.push_str(&format!(
                "    forwarded by {}: {}\n",
                forward["by"].as_str().unwrap_or("?"),
                forward["why"].as_str().unwrap_or("?")
            ));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_shown_with_its_call_its_reasons_and_its_forwards() {
        let listed = json!({"requests": [{"number": 3, "agent": "3f9a1c0b7d2e", "tool": "fs_read",
            "call": {"path": ".env"}, "reasons": [".env is a sensitive place (.env)"],
            "forwards": [{"by": "0a1b2c3d4e5f", "why": "it needs the key"}]}]});
        assert_eq!(
            approvals_text(&listed),
            "#3 agent 3f9a1c0b7d2e: fs_read {\"path\":\".env\"}\n    .env is a sensitive place \
             (.env)\n    forwarded by 0a1b2c3d4e5f: it needs the key\nAnswer with `flottille \
             approve <n>` (`--for-tree` for the same call from now on) or `flottille deny <n>`.\n"
        );
        assert_eq!(
            approvals_text(&json!({"requests": []})),
            "No request waits for you.\n"
        );
    }
}
