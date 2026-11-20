//! `flottille changes [--since <version>]`: what changed since the version last run, or since
//! one named, as the window's "What's new" page lists it (`docs/cli.md`).

use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn run(paths: &Paths, since: Option<String>, json: bool) -> Result<()> {
    let mut client = Client::connect_or_start(paths)?;
    let mut shown = client.request("whatsnew.show", json!({"since": since}))?;
    // Counted from the version last run, whose notes were not read yet (a build installed by
    // hand, or the check for updates off): read now, since the user asks.
    if since.is_none() && shown["groups"].is_null() && !shown["since"].is_null() {
        shown = client.request("whatsnew.show", json!({"since": shown["since"]}))?;
    }
    if json {
        println!("{shown}");
    } else {
        print!("{}", text(&shown));
    }
    Ok(())
}

fn text(shown: &Value) -> String {
    let (Some(since), Some(groups)) = (shown["since"].as_str(), shown["groups"].as_array()) else {
        return "Nothing new since the version you last ran.\n".into();
    };
    let this = shown["version"].as_str().unwrap_or("this build");
    let mut out = format!("What's new since {since} (now {this})\n");
    if groups.is_empty() {
        out.push_str("\nNo change is listed for these versions.\n");
    }
    for group in groups {
        out.push_str(&format!("\n{}\n", group["title"].as_str().unwrap_or("?")));
        for scope in group["scopes"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  {}\n",
                scope["scope"].as_str().unwrap_or("general")
            ));
            for change in scope["changes"].as_array().into_iter().flatten() {
                let pr = change["pr"]
                    .as_u64()
                    .map(|pr| format!(" (#{pr})"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "    - {}{pr}\n",
                    change["text"].as_str().unwrap_or_default()
                ));
            }
        }
    }
    out
}
