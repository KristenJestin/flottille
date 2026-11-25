//! The values of all machines, as this machine keeps them (`docs/remote.md`, "Settings for all
//! machines"): the last copy it checked of each, `remote/shared.json` (600), read with the hub out
//! of reach. A value is the user's change as their device signed it (`about.share`), kept whole so
//! that it can be sealed again for a machine paired since; the hub keeps the same, sealed for every
//! machine (`flottille_link::Sealed`), and never reads it.
//!
//! The values of all machines are what the user says of themselves (`about.name`, `about.pronouns`,
//! `about.notes`, by `about.share`) and the settings of `crate::settings_share` (`rules`, the
//! guidance files, `tool_results.limit`…, by `settings.share`, one value a request). No secret is
//! one: a vault value, a token, a key and a project's `[variables]` stay on their machine, and no
//! method takes a name that is not listed.
//!
//! ```json
//! {"about.name": {"value": "Sam", "at": 1764000000000, "request": {/* as signed */}}}
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use flottille_link::{MachineKey, Members, PublicKey, SHARED_NAMES, Sealed, check_shared};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The file of the values this machine last checked, in its remote folder.
pub const FILE: &str = "shared.json";

/// The method whose signed request is a change of About you for all machines.
pub const ABOUT_SHARE: &str = "about.share";

/// The method whose signed request is a change of one setting for all machines: `{name, value}`.
pub const SETTINGS_SHARE: &str = "settings.share";

/// The values of About you, by name, and the field of `about.share` each one is.
pub const NAMES: [(&str, &str); 3] = [
    (SHARED_NAMES[0], "name"),
    (SHARED_NAMES[1], "pronouns"),
    (SHARED_NAMES[2], "notes"),
];

/// The method whose signed request carries the value `name`: none for a name that is no value of
/// all machines.
fn method_of(name: &str) -> Option<&'static str> {
    if NAMES.iter().any(|(known, _)| *known == name) {
        Some(ABOUT_SHARE)
    } else if crate::settings_share::known(name) {
        Some(SETTINGS_SHARE)
    } else {
        None
    }
}

/// A value as this machine checked it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub value: String,
    /// When the user's device signed it, in milliseconds since the Unix epoch.
    pub at: u64,
    /// The request the device signed, whole.
    pub request: Value,
}

impl Kept {
    /// What orders two changes of one value: the latest, then the larger signature, so that two
    /// machines given the same two changes keep the same one.
    fn order(&self) -> (u64, String) {
        (
            self.at,
            self.request["sig"].as_str().unwrap_or_default().to_owned(),
        )
    }
}

pub type Values = BTreeMap<String, Kept>;

/// What this machine last checked, nothing while it has no file. A file that cannot be read is
/// said in the log, and taken as none.
pub fn read(folder: &Path) -> Values {
    let file = folder.join(FILE);
    match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            eprintln!("flottille daemon: {} is not read: {error}", file.display());
            Values::new()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Values::new(),
        Err(error) => {
            eprintln!("flottille daemon: {} is not read: {error}", file.display());
            Values::new()
        }
    }
}

/// The values this machine last checked, while it has a hub: none while remote is off or this
/// machine is paired with none. Read from its files alone, so that what reads a setting needs no
/// daemon (`crate::settings_share::all`).
pub fn at(paths: &flottille_core::paths::Paths) -> Option<Values> {
    if !crate::remote::on(paths) {
        return None;
    }
    let folder = crate::remote::folder(paths);
    folder
        .join(crate::hub_session::HUB_FILE)
        .exists()
        .then(|| read(&folder))
}

fn write(folder: &Path, values: &Values) -> std::io::Result<()> {
    crate::link::write_json(&folder.join(FILE), values)
}

/// The value `name` of the user's signed request `request`, as `about.share` carries it and
/// `about.set` would take it (trimmed, within its limits): none when the request does not carry
/// it, or carries what `about.set` refuses.
pub fn kept_of(name: &str, request: &Value, at: u64, params: &Value) -> Option<Kept> {
    let value = match method_of(name)? {
        ABOUT_SHARE => {
            let field = NAMES.iter().find(|(known, _)| *known == name)?.1;
            crate::about::received(field, params.get(field)?.as_str()?)?
        }
        _ => {
            // One value a request: a copy carries the text of its own name alone, and one the
            // machine's own setting would take.
            (params.get("name")?.as_str()? == name).then_some(())?;
            let value = params.get("value")?.as_str()?;
            crate::settings_share::check(name, value).ok()?;
            value.to_owned()
        }
    };
    Some(Kept {
        value,
        at,
        request: request.clone(),
    })
}

/// The values a request `method` (`about.share` or `settings.share`) changes, checked again as
/// another machine would.
pub fn from_request(
    method: &str,
    text: &str,
    members: &Members,
) -> Result<Vec<(String, Kept)>, String> {
    let (_, request, params) =
        check_shared(text.as_bytes(), method, members).map_err(|error| error.to_string())?;
    let at = request.signed.at;
    let request = serde_json::to_value(&request).map_err(|error| error.to_string())?;
    let names: Vec<String> = match method {
        ABOUT_SHARE => NAMES.iter().map(|(name, _)| name.to_string()).collect(),
        _ => params["name"]
            .as_str()
            .map(str::to_owned)
            .into_iter()
            .collect(),
    };
    Ok(names
        .into_iter()
        .filter_map(|name| {
            let kept = kept_of(&name, &request, at, &params)?;
            Some((name, kept))
        })
        .collect())
}

/// `sealed` opened with this machine's key and checked: a value of all machines, the user's own
/// change signed by a member's device that may make it, of the name and time the copy says. None
/// otherwise, whatever sealed it.
fn opened(sealed: &Sealed, key: &MachineKey, members: &Members) -> Option<Kept> {
    let method = method_of(&sealed.name)?;
    let plain = sealed.open(key)?;
    let (_, request, params) = check_shared(&plain, method, members)
        .map_err(|error| {
            eprintln!(
                "flottille daemon: a value of all machines, {}, is not taken: {error}",
                sealed.name
            );
        })
        .ok()?;
    if request.signed.at != sealed.at {
        return None;
    }
    let at = request.signed.at;
    let request = serde_json::to_value(&request).ok()?;
    kept_of(&sealed.name, &request, at, &params)
}

/// Whether `kept`, the value `name`, is still the change of a member that may make it.
fn still_signed(name: &str, kept: &Kept, members: &Members) -> bool {
    let Some(method) = method_of(name) else {
        return false;
    };
    serde_json::to_vec(&kept.request).is_ok_and(|text| check_shared(&text, method, members).is_ok())
}

/// Every machine of the members: what a value is sealed for.
pub fn machines(members: &Members) -> Vec<PublicKey> {
    members
        .members()
        .iter()
        .filter_map(|member| member.machine)
        .collect()
}

/// `kept`, the value `name`, sealed for every machine of `members`.
pub fn seal(name: &str, kept: &Kept, key: &MachineKey, members: &Members) -> Option<Sealed> {
    let plain = serde_json::to_vec(&kept.request).ok()?;
    Sealed::seal(name, kept.at, &plain, &machines(members), key)
        .map_err(|error| eprintln!("flottille daemon: {name} is not sealed: {error}"))
        .ok()
}

/// What this machine keeps of `changes`: each newer than its own copy replaces it, and the file is
/// written. The names it changed.
pub fn keep(folder: &Path, changes: Vec<(String, Kept)>) -> Vec<String> {
    let mut values = read(folder);
    let mut changed = Vec::new();
    for (name, kept) in changes {
        let newer = values
            .get(&name)
            .is_none_or(|own| kept.order() > own.order());
        if newer {
            values.insert(name.clone(), kept);
            changed.push(name);
        }
    }
    if !changed.is_empty()
        && let Err(error) = write(folder, &values)
    {
        eprintln!("flottille daemon: the values of all machines are not kept: {error}");
    }
    changed
}

/// What the hub sent, taken: the values it holds newer than this machine's are kept, and the names
/// of those this machine must give the hub again (its own newer, one the hub lacks, or one sealed
/// for fewer machines than the members have now, a machine having been paired since).
pub fn took(
    folder: &Path,
    hub: &[Sealed],
    key: &MachineKey,
    members: &Members,
) -> (Vec<String>, Vec<String>) {
    let opened: Vec<(String, Kept)> = hub
        .iter()
        .filter_map(|sealed| Some((sealed.name.clone(), opened(sealed, key, members)?)))
        .collect();
    let mut changed = keep(folder, opened.clone());
    let mut values = read(folder);
    // A value whose device left the members, or may only view now, is dropped, never given again:
    // the hub's copy of it fails on every machine, and giving it again would have each machine
    // seal it once more at each copy the hub sends. Shown as if none were shared.
    let left: Vec<String> = values
        .iter()
        .filter(|(name, own)| !still_signed(name, own, members))
        .map(|(name, _)| name.clone())
        .collect();
    if !left.is_empty() {
        for name in &left {
            values.remove(name);
            if !changed.contains(name) {
                changed.push(name.clone());
            }
        }
        if let Err(error) = write(folder, &values) {
            eprintln!("flottille daemon: the values of all machines are not kept: {error}");
        }
    }
    let all = machines(members);
    let again = values
        .iter()
        .filter(|(name, own)| {
            let theirs = hub.iter().find(|sealed| sealed.name == **name);
            let open = opened.iter().find(|(opened, _)| opened == *name);
            match (theirs, open) {
                (None, _) => true,
                // Sealed for this machine, but not by it, or for no member it could check.
                (Some(_), None) => true,
                (Some(sealed), Some((_, theirs))) => {
                    own.order() > theirs.order()
                        || all
                            .iter()
                            .any(|machine| !sealed.recipients().any(|to| to == machine))
                }
            }
        })
        .map(|(name, _)| name.clone())
        .collect();
    (changed, again)
}
