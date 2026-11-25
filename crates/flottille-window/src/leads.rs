//! The Leads section of a pilot's page (`docs/design.md` §10, "A pilot sums up"): one row per
//! session it leads, in launch order, with its mark, its ticket, its workspace (and its machine's
//! icon when it is another machine), its model, its pull request and checks, its runs and what it
//! waits for in the colour of its mark; a launch of the pilot's held for the user is a row until
//! it starts. A click opens the session. Rows unfold, fold away and slide; their marks and words
//! fade as they change.

use gpui_kit::ElementId;

/// The section's title.
pub const LEADS: &str = "Leads";

/// The element id of the section, labelled `Leads 2`, its rows counted.
pub fn leads_id() -> ElementId {
    ElementId::Name("session-leads".into())
}

/// The element id of a row of Leads: `key` is the led session's row id, or `held-<number>` for a
/// launch held for the user.
pub fn lead_row_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-{key}").into())
}

/// The element id of the place a row unfolds in, folds away from and slides in.
pub fn lead_item_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-item-{key}").into())
}

/// The element id of a row's ticket key, its tag beside the title.
pub fn lead_ticket_id(key: &str) -> ElementId {
    ElementId::Name(format!("{}-tag-0", lead_row_id(key)).into())
}

/// The element id of a row's workspace and model: `login · Sonnet 5.5 · medium`.
pub fn lead_place_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-place-{key}").into())
}

/// The element id of the mark of the machine a led session runs on, when it is another one,
/// labelled with its name.
pub fn lead_machine_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-machine-{key}").into())
}

/// The element id of a row's pull request and its checks: `#12, checks failing`.
pub fn lead_pull_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-pull-{key}").into())
}

/// The element id of the runs a row's session follows: `run 900`.
pub fn lead_runs_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-runs-{key}").into())
}

/// The element id of what a row's session waits for, in the colour of its mark.
pub fn lead_waits_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-waits-{key}").into())
}
