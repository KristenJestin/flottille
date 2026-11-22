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

/// What `flottille notifications settings` was given to change; nothing is a read.
pub struct Changes {
    pub project: Option<String>,
    pub kinds: Vec<String>,
    pub sound: Option<String>,
    pub sound_kinds: Vec<String>,
    pub volume: Option<u64>,
    pub quiet: Option<String>,
    pub days: Vec<String>,
    pub dnd: Option<String>,
}

/// `KIND=WORD` pairs as a JSON object, each value made by `value`.
fn pairs(
    what: &str,
    given: &[String],
    value: impl Fn(&str) -> std::result::Result<Value, String>,
) -> std::result::Result<Value, String> {
    let mut object = serde_json::Map::new();
    for pair in given {
        let (kind, word) = pair
            .split_once('=')
            .ok_or_else(|| format!("{what} is `<kind>=<value>`, not `{pair}`"))?;
        object.insert(kind.to_owned(), value(word)?);
    }
    Ok(Value::Object(object))
}

/// The request the options make, or `None` when they ask for nothing to change.
fn request_of(changes: &Changes, now: u64) -> std::result::Result<Option<Value>, String> {
    let mut params = serde_json::Map::new();
    if let Some(project) = &changes.project {
        params.insert("project".into(), json!(project));
    }
    if !changes.kinds.is_empty() {
        let kinds = pairs("--kind", &changes.kinds, |word| Ok(json!(word)))?;
        params.insert("kinds".into(), kinds);
    }
    let mut sound = serde_json::Map::new();
    if let Some(on) = &changes.sound {
        sound.insert("on".into(), json!(on == "on"));
    }
    if let Some(volume) = changes.volume {
        sound.insert("volume".into(), json!(volume));
    }
    if !changes.sound_kinds.is_empty() {
        let kinds = pairs("--sound-kind", &changes.sound_kinds, |word| match word {
            "on" => Ok(json!(true)),
            "off" => Ok(json!(false)),
            word => Err(format!("a kind's sound is on or off, not `{word}`")),
        })?;
        sound.insert("kinds".into(), kinds);
    }
    if !sound.is_empty() {
        params.insert("sound".into(), Value::Object(sound));
    }
    let mut quiet = serde_json::Map::new();
    match changes.quiet.as_deref() {
        None => {}
        Some("off") => {
            quiet.insert("on".into(), json!(false));
        }
        Some(range) => {
            let (from, to) = range
                .split_once('-')
                .ok_or_else(|| format!("--quiet is `HH:MM-HH:MM` or `off`, not `{range}`"))?;
            quiet.extend([
                ("on".into(), json!(true)),
                ("from".into(), json!(from)),
                ("to".into(), json!(to)),
            ]);
        }
    }
    if !changes.days.is_empty() {
        quiet.insert("days".into(), json!(changes.days));
    }
    if !quiet.is_empty() {
        params.insert("quiet".into(), Value::Object(quiet));
    }
    if let Some(dnd) = changes.dnd.as_deref() {
        let hour = 3_600_000;
        let patch = match dnd {
            "on" => json!({"on": true}),
            "off" => json!({"on": false}),
            "1h" => json!({"on": true, "until": now + hour}),
            "tomorrow" => json!({
                "on": true,
                "until": flottille_core::notification_settings::next_morning(now)
            }),
            other => {
                return Err(format!(
                    "--dnd is `on`, `1h`, `tomorrow` or `off`, not `{other}`"
                ));
            }
        };
        params.insert("do_not_disturb".into(), patch);
    }
    Ok((params.keys().any(|key| key != "project")).then_some(Value::Object(params)))
}

/// Reads what rings and when, or changes it and says what it is now.
pub fn settings(paths: &Paths, changes: Changes, json: bool) -> Result<()> {
    let now = flottille_core::store::now_ms();
    let mut client = Client::connect_or_start(paths)?;
    let answer = match request_of(&changes, now)? {
        Some(params) => client.request("notifications.set_settings", params)?,
        None => client.request("notifications.settings", Value::Null)?,
    };
    if json {
        println!("{answer}");
    } else {
        print!("{}", settings_text(&answer, changes.project.as_deref()));
    }
    Ok(())
}

fn level_words(level: &str) -> &str {
    match level {
        "ring" => "rings",
        "centre" => "centre only",
        other => other,
    }
}

/// The settings in words: each kind and what it does (a project's own, when one is asked), the
/// sound, the quiet hours and Do not disturb.
fn settings_text(said: &Value, project: Option<&str>) -> String {
    use flottille_core::notification_settings::{KINDS, label, local_time};
    let own = project.map(|project| &said["projects"][project]);
    let mut out = match project {
        Some(project) => format!("Notifications of {project}\n"),
        None => "Notifications\n".to_owned(),
    };
    for kind in KINDS {
        let mine = own.and_then(|own| own[*kind].as_str());
        let level = mine.or(said["kinds"][*kind].as_str()).unwrap_or("centre");
        let follows = match (project.is_some(), mine) {
            (true, None) => " (as the default)",
            _ => "",
        };
        let silent = said["sound"]["kinds"][*kind] == false;
        out.push_str(&format!(
            "  {:<13} {}{follows}{}\n",
            label(kind),
            level_words(level),
            if silent && level == "ring" {
                ", no sound"
            } else {
                ""
            }
        ));
    }
    let sound = &said["sound"];
    out.push_str(&match sound["on"] == true {
        true => format!("Sound: on, volume {}\n", sound["volume"]),
        false => "Sound: off\n".to_owned(),
    });
    let quiet = &said["quiet"];
    out.push_str(&match quiet["on"] == true {
        true => format!(
            "Quiet hours: {}–{} on {}\n",
            quiet["from"].as_str().unwrap_or_default(),
            quiet["to"].as_str().unwrap_or_default(),
            quiet["days"]
                .as_array()
                .map(|days| days
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ),
        false => "Quiet hours: off\n".to_owned(),
    });
    let dnd = &said["do_not_disturb"];
    out.push_str(&match (dnd["on"] == true, dnd["until"].as_u64()) {
        (true, Some(until)) => {
            let at = local_time(until).minute;
            format!("Do not disturb: on until {:02}:{:02}\n", at / 60, at % 60)
        }
        (true, None) => "Do not disturb: on until turned off\n".to_owned(),
        (false, _) => "Do not disturb: off\n".to_owned(),
    });
    if project.is_none()
        && let Some(projects) = said["projects"].as_object()
    {
        for (name, kinds) in projects {
            for (kind, level) in kinds.as_object().into_iter().flatten() {
                out.push_str(&format!(
                    "{name}: {kind} {}\n",
                    level_words(level.as_str().unwrap_or_default())
                ));
            }
        }
    }
    out
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

    fn nothing() -> Changes {
        Changes {
            project: None,
            kinds: vec![],
            sound: None,
            sound_kinds: vec![],
            volume: None,
            quiet: None,
            days: vec![],
            dnd: None,
        }
    }

    #[test]
    fn no_option_is_a_read_and_a_project_alone_is_too() {
        assert_eq!(request_of(&nothing(), 0).unwrap(), None);
        let mut project = nothing();
        project.project = Some("app".into());
        assert_eq!(request_of(&project, 0).unwrap(), None);
    }

    #[test]
    fn the_options_make_one_request() {
        let mut changes = nothing();
        changes.project = Some("app".into());
        changes.kinds = vec!["report=centre".into(), "ticket=default".into()];
        changes.sound_kinds = vec!["report=off".into()];
        changes.volume = Some(70);
        changes.quiet = Some("22:00-08:00".into());
        changes.days = vec!["mon".into(), "tue".into()];
        changes.dnd = Some("1h".into());

        let params = request_of(&changes, 1_000).unwrap().unwrap();

        assert_eq!(
            params,
            json!({"project": "app",
                "kinds": {"report": "centre", "ticket": "default"},
                "sound": {"volume": 70, "kinds": {"report": false}},
                "quiet": {"on": true, "from": "22:00", "to": "08:00", "days": ["mon", "tue"]},
                "do_not_disturb": {"on": true, "until": 3_601_000}})
        );
    }

    #[test]
    fn an_option_that_is_not_what_it_says_is_refused_before_the_daemon_is_asked() {
        for (kinds, quiet, dnd) in [
            (vec!["report"], None, None),
            (vec![], Some("22:00"), None),
            (vec![], None, Some("2h")),
        ] {
            let mut changes = nothing();
            changes.kinds = kinds.into_iter().map(String::from).collect();
            changes.quiet = quiet.map(String::from);
            changes.dnd = dnd.map(String::from);
            assert!(request_of(&changes, 0).is_err());
        }
        let mut changes = nothing();
        changes.sound_kinds = vec!["report=loud".into()];
        assert!(request_of(&changes, 0).is_err());
    }

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
