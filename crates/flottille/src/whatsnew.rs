//! What's new since the version last run (`docs/releases.md`, "What's new"): the daemon keeps
//! the version the user ran before this one and the changes between them, gathered from the
//! release notes, and says them to the window (`whatsnew.show`) and to `flottille changes`.
//!
//! A build the window downloaded (`update.rs`) brings the notes it kept; a build installed by
//! `install.sh` or `flottille update` reads them once, at its first start. Nothing here is a
//! secret and nothing here opens a page: the window shows a mark, and the user clicks it.

use std::sync::{Arc, Mutex};

use flottille_core::paths::Paths;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::update;

type Answer = Result<Value, Failure>;

/// The folder a pull request's number is a page of.
const PULLS: &str = "https://github.com/KristenJestin/flottille/pull";

/// The areas of the notes (`tools/changelog.sh`), in the order of the page, the fixes last: the key
/// a client draws by, and the title of the heading a release's body carries.
const AREAS: [(&str, &str); 7] = [
    ("remote", "Remote"),
    ("window", "Window"),
    ("agents", "Agents"),
    ("tools", "Tools"),
    ("projects", "Projects"),
    ("other", "Other"),
    ("fixed", "Fixed"),
];

/// The groups of the raw list of subjects (`### All changes` of a body, and the whole body of a
/// release made before the notes), in the changelog's order.
const RAW: [(&str, &str); 5] = [
    ("breaking", "Breaking changes"),
    ("features", "Features"),
    ("fixes", "Fixes"),
    ("performance", "Performance"),
    ("other", "Other changes"),
];

/// `$XDG_STATE_HOME/flottille/whats-new.json`: Flottille's own memory, like the update channel.
fn file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("whats-new.json")
}

/// What the state folder keeps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    /// The version that ran last: this build's, once the daemon started.
    version: String,
    /// The version that ran before it, the one the changes are counted from; none when nothing
    /// is new (the first version run, the same one again, a step back).
    since: Option<String>,
    /// Whether the page was opened since this build is the one that runs.
    seen: bool,
    /// The release notes of every version after `since` up to `version`, oldest first
    /// (`[{"version", "published_at", "body"}]`); none while they were not read.
    notes: Option<Vec<Value>>,
}

/// What a build applied from the window left (`update::apply_staged`).
struct Updated {
    version: String,
    from: String,
    notes: Vec<Value>,
}

fn updated(paths: &Paths) -> Option<Updated> {
    let value: Value =
        serde_json::from_slice(&std::fs::read(update::updated_file(paths)).ok()?).ok()?;
    Some(Updated {
        version: value["version"].as_str()?.to_owned(),
        from: value["from"].as_str()?.to_owned(),
        notes: value["notes"].as_array().cloned().unwrap_or_default(),
    })
}

fn read(paths: &Paths) -> Option<Kept> {
    serde_json::from_slice(&std::fs::read(file(paths)).ok()?).ok()
}

fn write(paths: &Paths, kept: &Kept) {
    let written = flottille_core::paths::ensure_private_dir(&paths.state)
        .map_err(|error| error.to_string())
        .and_then(|()| {
            crate::providers::write_in_place(&file(paths), &json!(kept).to_string())
                .map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        eprintln!("flottille daemon: what is new was not kept: {error}");
    }
}

/// What the daemon's start makes of what the last one left. Pure: `prev` is what was kept,
/// `version` this build's, `updated` what the window's update applied.
fn next(prev: Option<&Kept>, version: &str, updated: Option<&Updated>) -> Kept {
    let nothing = || Kept {
        version: version.to_owned(),
        since: None,
        seen: true,
        notes: None,
    };
    // The notes the update kept, when they are for the step from `from` to this build: an update
    // that read none (the releases were not reachable) kept an empty list, which says nothing.
    let applied = |from: &str| {
        updated
            .filter(|updated| updated.version == version && updated.from == from)
            .map(|updated| updated.notes.clone())
            .filter(|notes| !notes.is_empty())
    };
    let Some(prev) = prev else {
        // The first start with a memory: an update applied from the window says where from.
        return match updated.filter(|updated| updated.version == version) {
            Some(updated) => Kept {
                version: version.to_owned(),
                since: Some(updated.from.clone()),
                seen: false,
                notes: applied(&updated.from),
            },
            None => nothing(),
        };
    };
    if prev.version == version {
        return prev.clone();
    }
    let newer = match (Version::parse(version), Version::parse(&prev.version)) {
        (Ok(this), Ok(before)) => this != before && !update::is_older(&this, &before),
        _ => false,
    };
    if !newer {
        return nothing();
    }
    match (&prev.since, prev.seen) {
        // A jump over several versions without opening the page: it counts from the first.
        (Some(since), false) => {
            let notes = match (&prev.notes, applied(&prev.version)) {
                (Some(kept), Some(fresh)) => {
                    let mut all = kept.clone();
                    for note in fresh {
                        if !all.iter().any(|n| n["version"] == note["version"]) {
                            all.push(note);
                        }
                    }
                    Some(all)
                }
                _ => None,
            };
            Kept {
                version: version.to_owned(),
                since: Some(since.clone()),
                seen: false,
                notes,
            }
        }
        _ => Kept {
            version: version.to_owned(),
            since: Some(prev.version.clone()),
            seen: false,
            notes: applied(&prev.version),
        },
    }
}

/// The daemon's start: what ran before is remembered, and this build is the one that runs.
pub fn record(paths: &Paths) {
    let prev = read(paths);
    let kept = next(prev.as_ref(), update::VERSION, updated(paths).as_ref());
    if prev.as_ref() != Some(&kept) {
        write(paths, &kept);
    }
}

/// What the daemon knows beside what is on disk.
#[derive(Default)]
pub struct Live {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    fetching: bool,
    /// Why the notes were not read.
    failed: Option<String>,
}

/// One line of the changelog: a subject with its scope, or a note.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    scope: Option<String>,
    text: String,
    pr: Option<u64>,
}

/// What a release's body says.
#[derive(Default)]
struct Body {
    /// The few lines written by hand at the top of a version's notes.
    highlights: Vec<String>,
    /// The notes, under the area each heading names.
    notes: Vec<(&'static str, Change)>,
    /// The raw list of subjects, under the group each heading names.
    raw: Vec<(&'static str, Change)>,
    /// A body made before the notes: its `### Features` are the raw list itself.
    legacy: bool,
}

/// Reads a release's body, as `tools/changelog.sh` writes it: `### Remote`, then `- A sentence.
/// (#123)`, the areas in turn, `### All changes` and its `#### Features`, then `- **scope**: text
/// (#123)`; `### Highlights` holds the lines written by hand. A body of before the notes lists
/// the subjects under `### Features`: they are said as notes of Other, with a capital and no scope.
fn parse(body: &str) -> Body {
    enum In {
        Nothing,
        Highlights,
        Area(&'static str),
        Raw(&'static str),
    }
    let mut at = In::Nothing;
    let mut found = Body::default();
    for line in body.lines() {
        let line = line.trim_end();
        let heading = line
            .strip_prefix("#### ")
            .map(|title| (title, false))
            .or_else(|| line.strip_prefix("### ").map(|title| (title, true)));
        if let Some((title, top)) = heading {
            let title = title.trim();
            let raw = RAW.iter().find(|(_, name)| *name == title);
            at = if top && title == "Highlights" {
                In::Highlights
            } else if let (true, Some((key, _))) = (top, AREAS.iter().find(|(_, t)| *t == title)) {
                In::Area(key)
            } else if let Some((key, _)) = raw {
                found.legacy |= top;
                In::Raw(key)
            } else {
                In::Nothing
            };
        } else if let Some(item) = line.strip_prefix("- ") {
            match at {
                In::Nothing => {}
                In::Highlights => found.highlights.push(item.trim().to_owned()),
                In::Area(key) => found.notes.push((key, with_pr(item))),
                In::Raw(key) => found.raw.push((key, change_of(item))),
            }
        }
    }
    if found.legacy {
        found.notes = found
            .raw
            .iter()
            .map(|(_, change)| {
                let mut chars = change.text.chars();
                let text = chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default();
                (
                    "other",
                    Change {
                        scope: None,
                        text,
                        pr: change.pr,
                    },
                )
            })
            .collect();
    }
    found
}

/// A line with the number of its pull request at the end: `Said. (#12)`.
fn with_pr(line: &str) -> Change {
    let text = line.trim();
    let (text, pr) = match text
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once("(#"))
        .and_then(|(rest, number)| Some((rest.trim_end(), number.parse::<u64>().ok()?)))
    {
        Some((text, pr)) => (text, Some(pr)),
        None => (text, None),
    };
    Change {
        scope: None,
        text: text.to_owned(),
        pr,
    }
}

fn change_of(line: &str) -> Change {
    let (scope, text) = match line
        .strip_prefix("**")
        .and_then(|rest| rest.split_once("**: "))
    {
        Some((scope, text)) => (Some(scope.to_owned()), text),
        None => (None, line),
    };
    Change {
        scope: scope.map(str::to_owned),
        ..with_pr(text)
    }
}

fn said(change: &Change) -> Value {
    json!({
        "text": change.text,
        "pr": change.pr,
        "url": change.pr.map(|pr| format!("{PULLS}/{pr}")),
    })
}

/// What the notes of the versions say, each line once, oldest first: `{"groups": [{"key", "title",
/// "changes": [{"text", "pr", "url"}]}]}` (the areas that hold a note, in the order of the page),
/// `"highlights"` (lines) and `"all"`, the raw list as the changelog groups it and then by scope
/// (the changes without one last): `[{"key", "title", "scopes": [{"scope", "changes": [..]}]}]`.
fn digest(notes: &[Value]) -> (Vec<Value>, Vec<String>, Vec<Value>) {
    let mut highlights: Vec<String> = Vec::new();
    let mut said_notes: Vec<(&'static str, Change)> = Vec::new();
    let mut raw: Vec<(&'static str, Change)> = Vec::new();
    for note in notes {
        let body = parse(note["body"].as_str().unwrap_or_default());
        for line in body.highlights {
            if !highlights.contains(&line) {
                highlights.push(line);
            }
        }
        for note in body.notes {
            if !said_notes.contains(&note) {
                said_notes.push(note);
            }
        }
        for change in body.raw {
            if !raw.contains(&change) {
                raw.push(change);
            }
        }
    }
    let areas = AREAS
        .iter()
        .filter_map(|(key, title)| {
            let mine: Vec<Value> = said_notes
                .iter()
                .filter(|(area, _)| area == key)
                .map(|(_, change)| said(change))
                .collect();
            (!mine.is_empty()).then(|| json!({"key": key, "title": title, "changes": mine}))
        })
        .collect();
    let all = RAW
        .iter()
        .filter_map(|(key, title)| {
            let mut mine: Vec<&Change> = raw
                .iter()
                .filter(|(group, _)| group == key)
                .map(|(_, change)| change)
                .collect();
            if mine.is_empty() {
                return None;
            }
            // Stable: within a scope, the oldest first.
            mine.sort_by_key(|change| {
                (
                    change.scope.is_none(),
                    change.scope.as_deref().map(str::to_lowercase),
                )
            });
            let mut scopes: Vec<(Option<&str>, Vec<Value>)> = Vec::new();
            for change in mine {
                match scopes.last_mut() {
                    Some((scope, changes)) if *scope == change.scope.as_deref() => {
                        changes.push(said(change))
                    }
                    _ => scopes.push((change.scope.as_deref(), vec![said(change)])),
                }
            }
            Some(json!({
                "key": key,
                "title": title,
                "scopes": scopes
                    .into_iter()
                    .map(|(scope, changes)| json!({"scope": scope, "changes": changes}))
                    .collect::<Vec<_>>(),
            }))
        })
        .collect();
    (areas, highlights, all)
}

/// What `whatsnew.show` answers: `{"version", "since", "seen", "fetching", "failed", "groups",
/// "highlights", "all"}`, the last three null while the notes were not read.
fn shown(daemon: &Daemon, notes: Option<(&str, &[Value])>) -> Value {
    let kept = read(&daemon.paths).unwrap_or_default();
    let state = daemon.whatsnew.state.lock().unwrap();
    let (since, notes) = match notes {
        Some((since, notes)) => (Some(since.to_owned()), Some(notes.to_vec())),
        None => (kept.since.clone(), kept.notes.clone()),
    };
    let read = since.as_ref().and(notes.as_deref().map(digest));
    json!({
        "version": update::VERSION,
        "since": since,
        "seen": kept.seen,
        "fetching": state.fetching,
        "failed": state.failed,
        "groups": read.as_ref().map(|(groups, _, _)| groups),
        "highlights": read.as_ref().map(|(_, highlights, _)| highlights),
        "all": read.as_ref().map(|(_, _, all)| all),
    })
}

fn changed(daemon: &Daemon) {
    if let Err(error) = daemon.emit("whatsnew.changed", shown(daemon, None)) {
        eprintln!("flottille daemon: whatsnew.changed was not stored: {error}");
    }
}

/// The notes of the versions from `since` to this build, off the daemon's workers.
async fn read_notes(since: String) -> Result<Vec<Value>, String> {
    tokio::task::spawn_blocking(move || {
        let since = Version::parse(&since).map_err(|error| format!("{since}: {error}"))?;
        let this = Version::parse(update::VERSION).map_err(|error| error.to_string())?;
        update::notes_between(&since, &this).map_err(|error| error.to_string())
    })
    .await
    .unwrap_or_else(|error| Err(error.to_string()))
}

/// Reads the notes the memory lacks and keeps them; the reason when it cannot is said.
async fn fetch(daemon: &Arc<Daemon>) {
    let Some(since) = read(&daemon.paths).and_then(|kept| kept.since) else {
        return;
    };
    {
        let mut state = daemon.whatsnew.state.lock().unwrap();
        if state.fetching {
            return;
        }
        state.fetching = true;
        state.failed = None;
    }
    changed(daemon);
    let found = read_notes(since.clone()).await;
    {
        let mut state = daemon.whatsnew.state.lock().unwrap();
        state.fetching = false;
        match found {
            Ok(notes) => {
                // Only for the step they were read for: the memory may have moved meanwhile.
                if let Some(mut kept) = read(&daemon.paths)
                    && kept.since.as_deref() == Some(&since)
                    && kept.version == update::VERSION
                {
                    kept.notes = Some(notes);
                    write(&daemon.paths, &kept);
                }
            }
            Err(why) => state.failed = Some(why),
        }
    }
    changed(daemon);
}

/// At the start: the notes a build installed by hand lacks are read once, unless the user turned
/// the check for updates off (then the daemon makes no request at all, and says so).
pub async fn watch(daemon: Arc<Daemon>) {
    let lacking =
        read(&daemon.paths).is_some_and(|kept| kept.since.is_some() && kept.notes.is_none());
    if !lacking {
        return;
    }
    if crate::updates::checks_automatically(&daemon.paths).unwrap_or(false) {
        fetch(&daemon).await;
    } else {
        daemon.whatsnew.state.lock().unwrap().failed =
            Some("checking for updates is off: ask again to read the notes".to_owned());
        changed(&daemon);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Show {
    since: Option<String>,
}

/// Answers `whatsnew.show`, `whatsnew.seen` and `whatsnew.fetch` (`docs/protocol.md`).
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "whatsnew.show" => {
            let Show { since } = serde_json::from_value(params)?;
            // Counted from a version the user names (`flottille changes --since`): read now,
            // whatever the setting, and not kept.
            let Some(since) = since.filter(|since| {
                read(&daemon.paths).and_then(|kept| kept.since).as_deref() != Some(since)
            }) else {
                return Ok(shown(&daemon, None));
            };
            Version::parse(&since).map_err(|error| {
                Failure::new(
                    "invalid-params",
                    format!("{since} is not a version: {error}"),
                )
            })?;
            let notes = read_notes(since.clone())
                .await
                .map_err(|why| Failure::new("unavailable", why))?;
            Ok(shown(&daemon, Some((&since, &notes))))
        }
        "whatsnew.seen" => {
            if let Some(mut kept) = read(&daemon.paths).filter(|kept| !kept.seen) {
                kept.seen = true;
                write(&daemon.paths, &kept);
                changed(&daemon);
            }
            Ok(shown(&daemon, None))
        }
        "whatsnew.fetch" => {
            let fetching = daemon.clone();
            tokio::spawn(async move { fetch(&fetching).await });
            Ok(shown(&daemon, None))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(version: &str, body: &str) -> Value {
        json!({"version": version, "published_at": "2026-11-10T10:00:00Z", "body": body})
    }

    fn kept(version: &str, since: Option<&str>, seen: bool, notes: Option<Vec<Value>>) -> Kept {
        Kept {
            version: version.to_owned(),
            since: since.map(str::to_owned),
            seen,
            notes,
        }
    }

    fn updated(version: &str, from: &str, notes: Vec<Value>) -> Updated {
        Updated {
            version: version.to_owned(),
            from: from.to_owned(),
            notes,
        }
    }

    #[test]
    fn a_change_is_its_scope_its_words_and_its_pull_request() {
        assert_eq!(
            change_of("**gui**: a link opens (#1103)"),
            Change {
                scope: Some("gui".into()),
                text: "a link opens".into(),
                pr: Some(1103)
            }
        );
        assert_eq!(
            change_of("no scope, (not a pull request) (#7)"),
            Change {
                scope: None,
                text: "no scope, (not a pull request)".into(),
                pr: Some(7)
            }
        );
        assert_eq!(change_of("**cli**: bare").pr, None);
    }

    #[test]
    fn a_body_is_read_by_the_headings_the_changelog_writes() {
        let body = parse(
            "### Highlights\n\n- By hand.\n\n### Window\n\n- A note. (#1)\n\n### Unknown\n\n- skipped (#2)\n\n### Fixed\n\n- Fixed. (#3)\n\n### All changes\n\n#### Features\n\n- **gui**: one (#1)\n\n#### Other changes\n\n- two (#4)\nafter",
        );
        assert_eq!(body.highlights, ["By hand."]);
        assert_eq!(
            body.notes.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["window", "fixed"]
        );
        assert_eq!(
            body.raw.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["features", "other"]
        );
        assert!(!body.legacy);
    }

    #[test]
    fn a_body_of_before_the_notes_says_its_subjects_as_notes_of_other() {
        let body = parse("### Features\n\n- **gui**: a link opens (#1)\n- no scope (#2)");
        assert!(body.legacy);
        assert_eq!(
            body.notes,
            [
                (
                    "other",
                    Change {
                        scope: None,
                        text: "A link opens".into(),
                        pr: Some(1)
                    }
                ),
                (
                    "other",
                    Change {
                        scope: None,
                        text: "No scope".into(),
                        pr: Some(2)
                    }
                ),
            ]
        );
    }

    #[test]
    fn several_betas_are_one_list_each_note_once_the_fixes_last() {
        let (areas, highlights, all) = digest(&[
            note(
                "1",
                "### Fixed\n\n- B. (#2)\n\n### Remote\n\n- A. (#1)\n\n### All changes\n\n#### Fixes\n\n- **gui**: b (#2)\n- **cli**: a (#1)",
            ),
            note(
                "2",
                "### Highlights\n\n- One.\n\n### Fixed\n\n- B. (#2)\n- C. (#3)\n\n### All changes\n\n#### Fixes\n\n- **gui**: b (#2)\n- **cli**: c (#3)\n\n#### Features\n\n- z (#4)",
            ),
        ]);
        let keys: Vec<&str> = areas.iter().map(|g| g["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["remote", "fixed"]);
        let texts: Vec<&str> = areas[1]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, ["B.", "C."], "oldest first, each once");
        assert_eq!(highlights, ["One."]);
        // The raw list: the changelog's groups, then by scope.
        assert_eq!(all[0]["key"], "features");
        assert_eq!(all[1]["scopes"][0]["scope"], "cli");
        assert_eq!(all[1]["scopes"][0]["changes"].as_array().unwrap().len(), 2);
        assert_eq!(all[1]["scopes"][1]["scope"], "gui");
    }

    #[test]
    fn a_newer_build_counts_from_the_one_that_ran_before() {
        let before = kept("0.4.0-beta.1", None, true, None);
        let now = next(Some(&before), "0.4.0-beta.2", None);
        assert_eq!(now, kept("0.4.0-beta.2", Some("0.4.0-beta.1"), false, None));
    }

    #[test]
    fn the_same_build_again_keeps_what_it_had() {
        let before = kept("0.4.0-beta.2", Some("0.4.0-beta.1"), true, None);
        assert_eq!(next(Some(&before), "0.4.0-beta.2", None), before);
    }

    #[test]
    fn the_first_start_and_a_step_back_show_nothing_new() {
        assert_eq!(
            next(None, "0.4.0-beta.2", None),
            kept("0.4.0-beta.2", None, true, None)
        );
        let before = kept("0.4.0-beta.3", Some("0.4.0-beta.1"), false, Some(vec![]));
        assert_eq!(
            next(Some(&before), "0.4.0-beta.2", None),
            kept("0.4.0-beta.2", None, true, None)
        );
    }

    #[test]
    fn an_update_from_the_window_brings_its_notes() {
        let notes = vec![note("0.4.0-beta.2", "### Fixes\n\n- a (#1)")];
        let got = updated("0.4.0-beta.2", "0.4.0-beta.1", notes.clone());
        let before = kept("0.4.0-beta.1", None, true, None);
        assert_eq!(
            next(Some(&before), "0.4.0-beta.2", Some(&got)).notes,
            Some(notes.clone())
        );
        // The first build to remember anything: the update says where from.
        let first = next(None, "0.4.0-beta.2", Some(&got));
        assert_eq!(first.since.as_deref(), Some("0.4.0-beta.1"));
        assert_eq!(first.notes, Some(notes));
        // An update that read no notes says nothing: they are read at the start.
        let none = updated("0.4.0-beta.2", "0.4.0-beta.1", vec![]);
        assert_eq!(next(Some(&before), "0.4.0-beta.2", Some(&none)).notes, None);
    }

    #[test]
    fn a_jump_over_betas_not_opened_counts_from_the_first() {
        let first = vec![note("0.4.0-beta.2", "### Fixes\n\n- a (#1)")];
        let before = kept(
            "0.4.0-beta.2",
            Some("0.4.0-beta.1"),
            false,
            Some(first.clone()),
        );
        let second = vec![note("0.4.0-beta.3", "### Fixes\n\n- b (#2)")];
        let got = updated("0.4.0-beta.3", "0.4.0-beta.2", second.clone());

        let now = next(Some(&before), "0.4.0-beta.3", Some(&got));

        assert_eq!(now.since.as_deref(), Some("0.4.0-beta.1"));
        assert!(!now.seen);
        assert_eq!(now.notes, Some([first, second].concat()));
        // Without the second's notes, they are read again for the whole jump.
        assert_eq!(next(Some(&before), "0.4.0-beta.3", None).notes, None);
    }
}
