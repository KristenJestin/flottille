//! `flottille notifications`: the notifications the daemon keeps for the user, and their marks
//! (`docs/cli.md`).

use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// At most this many are listed: the centre keeps no more.
const LIMIT: usize = flottille_core::notification::KEPT_ROWS;

pub fn list(paths: &Paths, unread: bool, json: bool) -> Result<()> {
    let listed = Client::connect_or_start(paths)?.request(
        "notifications.list",
        json!({"unread": unread, "limit": LIMIT}),
    )?;
    if json {
        println!("{listed}");
    } else {
        print!("{}", text(&listed, unread));
    }
    Ok(())
}

/// Marks one notification read by its number, or all of them.
pub fn read(paths: &Paths, id: Option<u64>, all: bool) -> Result<()> {
    let params = match (id, all) {
        (Some(id), false) => json!({"ids": [id]}),
        _ => json!({"all": true}),
    };
    let read = Client::connect_or_start(paths)?.request("notifications.read", params)?;
    let count = read["read"].as_array().map_or(0, Vec::len);
    println!(
        "{count} notification{} marked read.",
        if count == 1 { "" } else { "s" }
    );
    Ok(())
}

fn text(listed: &Value, unread: bool) -> String {
    let rows = listed["notifications"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if rows.is_empty() {
        return match unread {
            true => "Nothing is new.\n".into(),
            false => "Nothing came for you.\n".into(),
        };
    }
    let count = |key: &str| listed["unread"][key].as_u64().unwrap_or_default();
    let mut out = format!("{} need you · {} new\n", count("needs_you"), count("new"));
    for row in rows {
        let mark = match (row["resolved"] == true, row["read_at"].is_null()) {
            (true, _) => "done",
            (false, true) => "new",
            (false, false) => "read",
        };
        let place = match (row["project"].as_str(), row["workspace"].as_str()) {
            (Some(project), Some(workspace)) => format!(" ({project}/{workspace})"),
            (Some(project), None) => format!(" ({project})"),
            _ => String::new(),
        };
        let excerpt = match row["excerpt"]
            .as_str()
            .filter(|excerpt| !excerpt.is_empty())
        {
            Some(excerpt) => format!(": {excerpt}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "#{} {mark} {}{place}{excerpt}\n",
            row["id"],
            row["title"].as_str().unwrap_or_default()
        ));
    }
    out.push_str("Mark one read with `flottille notifications read <id>`, all with `--all`.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_is_listed_with_its_mark_its_place_and_its_excerpt() {
        let listed = json!({"unread": {"needs_you": 1, "new": 1}, "notifications": [
            {"id": 3, "kind": "request", "title": "Request #1 waits for you",
                "excerpt": "Read .env", "project": "app", "workspace": "login",
                "read_at": null, "resolved": false},
            {"id": 4, "kind": "report", "title": "Reported", "excerpt": "",
                "project": "app", "workspace": null, "read_at": 5, "resolved": false},
            {"id": 5, "kind": "request", "title": "Request #2 waits for you",
                "excerpt": "Run ls", "project": null, "workspace": null,
                "read_at": 6, "resolved": true}]});
        let said = text(&listed, false);
        assert!(said.starts_with("1 need you · 1 new\n"), "{said}");
        assert!(
            said.contains("#3 new Request #1 waits for you (app/login): Read .env\n"),
            "{said}"
        );
        assert!(said.contains("#4 read Reported (app)\n"), "{said}");
        assert!(
            said.contains("#5 done Request #2 waits for you: Run ls\n"),
            "{said}"
        );
        assert_eq!(
            text(&json!({"notifications": []}), true),
            "Nothing is new.\n"
        );
    }
}
