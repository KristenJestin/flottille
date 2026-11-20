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

/// The groups of a changelog (`tools/changelog.sh`), in its order: the key a client draws by,
/// and the title of the heading the notes carry.
const GROUPS: [(&str, &str); 5] = [
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

/// One change of the changelog.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    scope: Option<String>,
    text: String,
    pr: Option<u64>,
}

/// The changes a release's notes list, under the group each heading names, as `tools/changelog.sh`
/// writes them: `### Features`, then `- **scope**: text (#123)`.
fn parse(body: &str) -> Vec<(&'static str, Change)> {
    let mut group = None;
    let mut found = Vec::new();
    for line in body.lines() {
        let line = line.trim_end();
        if let Some(title) = line.strip_prefix("### ") {
            group = GROUPS
                .iter()
                .find(|(_, name)| *name == title.trim())
                .map(|(key, _)| *key);
        } else if let (Some(key), Some(change)) = (group, line.strip_prefix("- ")) {
            found.push((key, change_of(change)));
        }
    }
    found
}

fn change_of(line: &str) -> Change {
    let (scope, text) = match line
        .strip_prefix("**")
        .and_then(|rest| rest.split_once("**: "))
    {
        Some((scope, text)) => (Some(scope.to_owned()), text),
        None => (None, line),
    };
    let text = text.trim();
    // The pull request the squash kept, at the end.
    let (text, pr) = match text
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once("(#"))
        .and_then(|(rest, number)| Some((rest.trim_end(), number.parse::<u64>().ok()?)))
    {
        Some((text, pr)) => (text, Some(pr)),
        None => (text, None),
    };
    Change {
        scope,
        text: text.to_owned(),
        pr,
    }
}

/// The changes of every note, oldest first, each once, as the changelog groups them and then by
/// scope (the changes without one last): `[{"key", "title", "scopes": [{"scope", "changes":
/// [{"text", "pr", "url"}]}]}]`, the groups that hold something.
fn groups(notes: &[Value]) -> Vec<Value> {
    let mut all: Vec<(&'static str, Change)> = Vec::new();
    for note in notes {
        for change in parse(note["body"].as_str().unwrap_or_default()) {
            if !all.contains(&change) {
                all.push(change);
            }
        }
    }
    GROUPS
        .iter()
        .filter_map(|(key, title)| {
            let mut mine: Vec<&Change> = all
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
                let said = json!({
                    "text": change.text,
                    "pr": change.pr,
                    "url": change.pr.map(|pr| format!("{PULLS}/{pr}")),
                });
                match scopes.last_mut() {
                    Some((scope, changes)) if *scope == change.scope.as_deref() => {
                        changes.push(said)
                    }
                    _ => scopes.push((change.scope.as_deref(), vec![said])),
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
        .collect()
}

/// What `whatsnew.show` answers: `{"version", "since", "seen", "fetching", "failed", "groups"}`.
fn shown(daemon: &Daemon, notes: Option<(&str, &[Value])>) -> Value {
    let kept = read(&daemon.paths).unwrap_or_default();
    let state = daemon.whatsnew.state.lock().unwrap();
    let (since, notes) = match notes {
        Some((since, notes)) => (Some(since.to_owned()), Some(notes.to_vec())),
        None => (kept.since.clone(), kept.notes.clone()),
    };
    json!({
        "version": update::VERSION,
        "since": since,
        "seen": kept.seen,
        "fetching": state.fetching,
        "failed": state.failed,
        "groups": since.as_ref().and(notes.as_deref().map(groups)),
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
    fn the_notes_of_a_release_are_read_by_the_changelog_s_headings() {
        let found = parse(
            "Intro\n\n### Features\n\n- **gui**: one (#1)\n\n### Unknown\n\n- skipped (#2)\n\n### Other changes\n\n- two (#3)\nafter",
        );
        assert_eq!(
            found.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["features", "other"]
        );
    }

    #[test]
    fn several_betas_are_one_list_each_change_once_grouped_then_by_scope() {
        let listed = groups(&[
            note("1", "### Fixes\n\n- **gui**: b (#2)\n- **cli**: a (#1)"),
            note(
                "2",
                "### Fixes\n\n- **gui**: b (#2)\n- **cli**: c (#3)\n\n### Features\n\n- z (#4)",
            ),
        ]);
        let keys: Vec<&str> = listed.iter().map(|g| g["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["features", "fixes"]);
        let fixes = &listed[1]["scopes"];
        assert_eq!(fixes[0]["scope"], "cli");
        assert_eq!(fixes[0]["changes"].as_array().unwrap().len(), 2);
        assert_eq!(
            fixes[0]["changes"][0]["text"], "a",
            "oldest first in a scope"
        );
        assert_eq!(fixes[1]["scope"], "gui");
        assert_eq!(fixes[1]["changes"].as_array().unwrap().len(), 1);
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
