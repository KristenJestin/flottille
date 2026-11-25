//! The values of all machines, as this machine's files read them (`docs/remote.md`, "Settings for
//! all machines"): `remote/shared.json` of Flottille's state folder holds the last copy the daemon
//! checked of each, and a setting's loader reads its value from there when the machine's own file
//! says none. The daemon alone writes the file, after checking each copy; a reader trusts it as it
//! trusts the machine's own configuration.
//!
//! Only text is read, by the value's name: no secret is a value of all machines. A machine with no
//! hub (remote off, or never paired) reads none, whatever the file kept: its settings are its own.

use std::collections::BTreeMap;

use crate::paths::Paths;

/// The folder of the machine's keys and of what the hub shared, in the state folder.
const FOLDER: &str = "remote";
/// The file the daemon keeps the values in.
const FILE: &str = "shared.json";

/// A value as `shared.json` holds it: its text, and the request the user's device signed, which a
/// reader does not use.
#[derive(serde::Deserialize)]
struct Kept {
    value: String,
}

/// The hub this machine pinned, in the same folder.
const HUB: &str = "hub.json";

/// Whether this machine has a hub: remote is on in `daemon.toml`, and a hub was pinned.
fn has_hub(paths: &Paths) -> bool {
    paths.state.join(FOLDER).join(HUB).exists()
        && std::fs::read_to_string(paths.config.join("daemon.toml"))
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|table| table.get("remote")?.get("on")?.as_bool())
            == Some(true)
}

/// Every value of all machines this machine last checked, by name; none without a hub, while it
/// has no file or when the file cannot be read. A value shared empty says nothing.
pub fn all(paths: &Paths) -> BTreeMap<String, String> {
    if !has_hub(paths) {
        return BTreeMap::new();
    }
    let Ok(bytes) = std::fs::read(paths.state.join(FOLDER).join(FILE)) else {
        return BTreeMap::new();
    };
    serde_json::from_slice::<BTreeMap<String, Kept>>(&bytes)
        .map(|values| {
            values
                .into_iter()
                .map(|(name, kept)| (name, kept.value))
                .filter(|(_, value)| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The value `name` of all machines, if there is one.
pub fn value(paths: &Paths, name: &str) -> Option<String> {
    all(paths).remove(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(state: &std::path::Path) -> Paths {
        Paths {
            runtime: state.join("run"),
            state: state.to_path_buf(),
            config: state.join("config"),
            data: state.join("data"),
        }
    }

    /// A machine with a hub, whose `shared.json` holds `values`.
    fn machine(values: &str) -> (tempfile::TempDir, Paths) {
        let folder = tempfile::tempdir().unwrap();
        let paths = paths(folder.path());
        std::fs::create_dir_all(folder.path().join(FOLDER)).unwrap();
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("daemon.toml"), "[remote]\non = true\n").unwrap();
        std::fs::write(folder.path().join(FOLDER).join(HUB), "{}").unwrap();
        std::fs::write(folder.path().join(FOLDER).join(FILE), values).unwrap();
        (folder, paths)
    }

    const NEVER: &str = r#"{"update.check": {"value": "never", "at": 1, "request": {}},
        "judge": {"value": "", "at": 1, "request": {}}}"#;

    #[test]
    fn a_value_is_read_by_its_name() {
        let (_folder, paths) = machine(NEVER);
        assert_eq!(value(&paths, "update.check").as_deref(), Some("never"));
        assert_eq!(value(&paths, "judge"), None);
        assert_eq!(value(&paths, "sessions.end_inactive_days"), None);
    }

    #[test]
    fn a_machine_with_no_hub_reads_none() {
        let (folder, paths) = machine(NEVER);
        std::fs::write(paths.config.join("daemon.toml"), "[remote]\non = false\n").unwrap();
        assert!(all(&paths).is_empty());
        std::fs::write(paths.config.join("daemon.toml"), "[remote]\non = true\n").unwrap();
        std::fs::remove_file(folder.path().join(FOLDER).join(HUB)).unwrap();
        assert!(all(&paths).is_empty());
    }

    #[test]
    fn a_file_that_is_not_read_says_nothing() {
        let (_folder, paths) = machine("{not json");
        assert!(all(&paths).is_empty());
    }
}
