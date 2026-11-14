//! Connections in the window (`docs/extensions.md`, "Connections"): under the page of its row of
//! Settings › Features, while it is on, the user's MCP servers in the list pattern (rule 7), the
//! form dialog that declares one or edits it, and the review of what a server announces against
//! what was accepted, in a large dialog. Everything is a `connection.*` method of the daemon,
//! read again when a `connection.*` event says one changed; a result is a toast. A server's
//! secrets are vault entries: the window names them and never holds a value.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui_design_system::motion::{HOVER, Motion, PANEL};
use gpui_design_system::{
    BulkBar, Button, CheckState, Checkbox, ChoiceRow, Confirm, DiffLine, DiffLines, EmptyLine,
    Field, FilterChip, FilterChips, Group, Grouped, Icon, Item, ListCard, ListRow, Listing, Menu,
    Modal, RadioChoices, SearchField, Section, Select, SelectOption, Skeleton, Summary, Tag, Toast,
    Tone, Type as _, Unfolding, awaiting, changing, enter, fade, fuzzy, icon, palette, presence,
    presence_of, space, tipped_when_cut, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px, relative,
};
use serde_json::{Map, Value, json};

use crate::actions::Command;
use crate::commands::{boxed, line_field, perform_later, problem_line};
use crate::shell::Shell;
use crate::vault::{VaultEntry, VaultSection};

/// Its name in `extension.list`.
pub const NAME: &str = "connections";

/// What the list of servers is for.
pub const SENTENCE: &str = "Each server is one tool of every agent once you have reviewed what \
                            it gives them.";

/// What the empty list says to add, and why.
pub const EMPTY: &str = "No server yet. Add an MCP server you run as a program or reach at a \
                         URL: every agent gets it as one tool, behind the gate.";

/// What the form dialog says of a server.
pub const FORM_SENTENCE: &str = "Flottille starts it and talks to it; agents only see one tool, \
                                 and you review what it gives them before they can call it.";

/// What the variables' field says: where their values come from.
pub const VARIABLES_HINT: &str = "Each one's value is a vault entry every project sees, put \
                                  where it goes when the server starts. Agents never see it.";

/// What the limits' field says.
pub const LIMITS_HINT: &str = "An argument of that name may take only these values, in every \
                               action that has it.";

/// The radio's label of what happens when an agent calls an action.
pub const WHEN_CALLED: &str = "When an agent calls it";

/// Where the setting `word` stands in [`SETTINGS`]: a word it does not name is read as Judged,
/// what the daemon makes of an action it was not told about.
fn slot_of(word: &str) -> usize {
    let find = |word: &str| SETTINGS.iter().position(|(key, ..)| *key == word);
    find(word)
        .or_else(|| find("judged"))
        .expect("Judged is one of the four settings")
}

/// The four answers to it, in the order of the pill's list: the daemon's word, the word said,
/// the sentence under it, and how the pill is drawn while it is the one chosen.
const SETTINGS: [(&str, &str, &str, Tone); 4] = [
    ("off", "Off", "Agents never see it.", Tone::Outline),
    (
        "passes",
        "Allowed",
        "It runs in every mode, without asking.",
        Tone::Ok,
    ),
    (
        "judged",
        "Judged",
        "The judge decides in Auto; you are asked in Ask.",
        Tone::Quiet,
    ),
    (
        "asked",
        "Asked",
        "It always comes to you, never to the judge.",
        Tone::Strong,
    ),
];

/// The groups the review sorts its actions into, in the order they are drawn: what comes to the
/// user first, what is left to the judge last.
const GROUPS: [(&str, &str, &str); 4] = [
    ("asked", "Asked", "Always a card for you."),
    ("passes", "Allowed", "Run without asking."),
    ("off", "Off", "Agents never see them."),
    ("judged", "Judged", "The judge decides in Auto, you in Ask."),
];

/// A connection, as `connection.list` says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub name: String,
    pub about: String,
    pub enabled: bool,
    pub command: Option<Vec<String>>,
    pub url: Option<String>,
    /// Each variable (or header, for a URL) and the vault entry it names.
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub only: BTreeMap<String, Vec<String>>,
    /// `off`, `awaiting review`, `cut off` or `ready`.
    pub state: String,
    /// What changed when it was cut off.
    pub cut_off: Option<String>,
    /// Why its server did not start.
    pub failing: Option<String>,
    /// Why it must sign in again.
    pub signed_out: Option<String>,
    /// The version accepted, once it was.
    pub version: Option<String>,
    /// Each action accepted and its setting (`judged`, `passes`, `asked` or `off`).
    pub actions: Vec<(String, String)>,
}

impl Server {
    pub fn of(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let entries = |value: &Value| {
            value
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, entry)| (name.clone(), text(&entry["vault"])))
                .collect()
        };
        Self {
            name: text(&value["name"]),
            about: text(&value["about"]),
            enabled: value["enabled"] != false,
            command: value["command"]
                .as_array()
                .map(|words| words.iter().map(text).collect()),
            url: value["url"].as_str().map(str::to_owned),
            env: entries(&value["env"]),
            headers: entries(&value["headers"]),
            only: value["only"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(argument, values)| {
                    let values = values.as_array().into_iter().flatten().map(text).collect();
                    (argument.clone(), values)
                })
                .collect(),
            state: text(&value["state"]),
            cut_off: value["cut_off"].as_str().map(str::to_owned),
            failing: value["failing"].as_str().map(str::to_owned),
            signed_out: value["signed_out"].as_str().map(str::to_owned),
            version: value["server"]["version"].as_str().map(str::to_owned),
            actions: value["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|action| (text(&action["name"]), text(&action["setting"])))
                .collect(),
        }
    }

    /// Ready to be called: what the row of Features counts.
    pub fn is_on(&self) -> bool {
        self.state == "ready"
    }

    /// Its state, said once on its row.
    pub fn line(&self) -> String {
        match self.state.as_str() {
            "ready" => match &self.failing {
                Some(why) => format!("Failing: {why}"),
                None => {
                    let on = self.actions.iter().filter(|(_, s)| s != "off").count();
                    match on {
                        1 => "On · 1 action".to_owned(),
                        n => format!("On · {n} actions"),
                    }
                }
            },
            "awaiting review" if self.version.is_none() => "Waiting for its first review".into(),
            "awaiting review" => "Waiting for a review of your edit".into(),
            "signed out" => "Signed out: sign in again".into(),
            "cut off" => format!(
                "Stopped: {}, review it",
                stopped_by(self.cut_off.as_deref().unwrap_or_default())
            ),
            _ => "Off".into(),
        }
    }
}

/// What cut a connection off, in the words of its row: a new version is said as such.
fn stopped_by(changed: &str) -> String {
    let said: Vec<String> = changed
        .split("; ")
        .filter(|part| !part.is_empty())
        .map(|part| match part.starts_with("the server is now ") {
            true => "new version".to_owned(),
            false => part.to_owned(),
        })
        .collect();
    match said.is_empty() {
        true => "it changed".into(),
        false => said.join(", "),
    }
}

/// A command as one line, each word quoted only when it must be.
pub fn command_line(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c));
            match (plain, word.contains('\'')) {
                (true, _) => word.clone(),
                (false, false) => format!("'{word}'"),
                (false, true) => format!("\"{word}\""),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One line of a diff between two texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Same(String),
    Removed(String),
    Added(String),
}

impl Line {
    fn diff_line(&self) -> DiffLine {
        match self {
            Line::Same(text) => DiffLine::kept(text.clone()),
            Line::Removed(text) => DiffLine::removed(text.clone()),
            Line::Added(text) => DiffLine::added(text.clone()),
        }
    }
}

/// `old` against `new`, line by line: the lines both keep, in order, and around them what one
/// removed and the other added.
pub fn line_diff(old: &str, new: &str) -> Vec<Line> {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    // The longest run of lines both keep, by the usual table.
    let mut kept = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            kept[i][j] = match a[i] == b[j] {
                true => kept[i + 1][j + 1] + 1,
                false => kept[i + 1][j].max(kept[i][j + 1]),
            };
        }
    }
    let (mut i, mut j, mut lines) = (0, 0, Vec::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            lines.push(Line::Same(a[i].to_owned()));
            (i, j) = (i + 1, j + 1);
        } else if j < b.len() && (i == a.len() || kept[i][j + 1] >= kept[i + 1][j]) {
            lines.push(Line::Added(b[j].to_owned()));
            j += 1;
        } else {
            lines.push(Line::Removed(a[i].to_owned()));
            i += 1;
        }
    }
    // What was removed is read before what replaced it.
    let mut ordered: Vec<Line> = Vec::with_capacity(lines.len());
    for line in lines {
        if let Line::Removed(_) = line {
            let at = ordered
                .iter()
                .rposition(|l| !matches!(l, Line::Added(_)))
                .map_or(0, |at| at + 1);
            ordered.insert(at, line);
        } else {
            ordered.push(line);
        }
    }
    ordered
}

/// An action as the review shows it: its description and its arguments, one line each.
fn action_text(action: &Value) -> String {
    let mut lines = vec![
        action["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    ];
    let required: Vec<&str> = action["schema"]["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let properties = action["schema"]["properties"].as_object();
    if let Some(properties) = properties.filter(|p| !p.is_empty()) {
        lines.push("Arguments:".into());
        for (name, schema) in properties {
            let kind = schema["type"].as_str().unwrap_or("any");
            let needed = match required.contains(&name.as_str()) {
                true => ", required",
                false => "",
            };
            let about = schema["description"]
                .as_str()
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            lines.push(format!("  {name} ({kind}{needed}){about}"));
        }
    }
    // The server's claims about itself: shown, never used.
    let claims: Vec<&str> = [
        ("readOnlyHint", "only reads"),
        ("destructiveHint", "may destroy"),
        ("idempotentHint", "does the same when repeated"),
        ("openWorldHint", "reaches outside"),
    ]
    .into_iter()
    .filter(|(hint, _)| action["annotations"][hint] == true)
    .map(|(_, said)| said)
    .collect();
    if !claims.is_empty() {
        lines.push(format!("The server says it {}.", claims.join(", ")));
    }
    lines.join("\n")
}

/// What `connection.review` answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub first: bool,
    pub declaration_changed: bool,
    pub changes: Vec<String>,
    pub pinned: Option<String>,
    pub now: String,
    pub instructions_pinned: Option<String>,
    pub instructions_now: Option<String>,
    pub actions: Vec<ActionDiff>,
    pub refused: Vec<String>,
    pub limits: Vec<LimitCover>,
    /// What Flottille's adapter for this server is to it: that it applies, or that this version
    /// has none; nothing for a server no adapter knows.
    pub adapter: Option<AdapterSaid>,
}

/// The review's sentence on the server's adapter, and what it changes.
#[derive(Clone, Debug, PartialEq)]
pub struct AdapterSaid {
    pub said: String,
    pub changed: Vec<String>,
}

/// A limit of the declaration, and the actions it covers and does not.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitCover {
    pub argument: String,
    pub values: Vec<String>,
    pub covers: Vec<String>,
    pub not: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionDiff {
    pub name: String,
    /// `added`, `removed`, `changed` or `unchanged`.
    pub change: String,
    pub pinned: Option<String>,
    pub now: Option<String>,
    pub setting: Option<String>,
    /// Why no setting turns it on, while the adapter that applies keeps it off.
    pub locked: Option<String>,
}

impl Diff {
    pub fn of(value: &Value) -> Self {
        let words = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .or_else(|| v["name"].as_str().map(str::to_owned))
                })
                .collect()
        };
        let server = |value: &Value| {
            value.as_object().map(|_| {
                format!(
                    "{} {}",
                    value["name"].as_str().unwrap_or_default(),
                    value["version"].as_str().unwrap_or_default()
                )
                .trim()
                .to_owned()
            })
        };
        let text = |value: &Value| value.as_str().map(str::to_owned);
        Self {
            first: value["first"] == true,
            declaration_changed: value["declaration_changed"] == true,
            changes: words(&value["changes"]),
            pinned: server(&value["server"]["pinned"]),
            now: server(&value["server"]["now"]).unwrap_or_default(),
            instructions_pinned: text(&value["instructions"]["pinned"]),
            instructions_now: text(&value["instructions"]["now"]),
            actions: value["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|action| ActionDiff {
                    name: action["name"].as_str().unwrap_or_default().to_owned(),
                    change: action["change"].as_str().unwrap_or_default().to_owned(),
                    pinned: action["pinned"]
                        .as_object()
                        .map(|_| action_text(&action["pinned"])),
                    now: action["now"]
                        .as_object()
                        .map(|_| action_text(&action["now"])),
                    setting: text(&action["setting"]),
                    locked: text(&action["locked"]),
                })
                .collect(),
            refused: words(&value["refused"]),
            limits: value["limits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|limit| LimitCover {
                    argument: limit["argument"].as_str().unwrap_or_default().to_owned(),
                    values: words(&limit["values"]),
                    covers: words(&limit["covers"]),
                    not: words(&limit["not"]),
                })
                .collect(),
            adapter: value["adapter"]["said"].as_str().map(|said| AdapterSaid {
                said: said.to_owned(),
                changed: words(&value["adapter"]["changed"]),
            }),
        }
    }

    /// What the dialog says under its title.
    fn sentence(&self) -> String {
        if self.first {
            "Read what it gives agents before they can call it.".into()
        } else if !self.changes.is_empty() {
            format!(
                "Changed since you accepted it: {}.",
                self.changes.join("; ")
            )
        } else if self.declaration_changed {
            "You edited how it is reached: read what it gives agents again.".into()
        } else {
            "Nothing changed since you accepted it; you may set what each action does.".into()
        }
    }
}

/// What a write was, for the toast its answer says.
#[derive(Clone, Debug, PartialEq)]
pub enum Writing {
    Added(String),
    Edited(String),
    Switched(String, bool),
    Removed(String),
}

/// Whether the window opens `url` to sign in: the daemon gives an `https` address, or an `http`
/// one on this machine, and the system's handler is never handed anything else (a `file:` path, a
/// program's own scheme), whatever the answer says.
fn sign_in_address(url: &str) -> Result<(), &'static str> {
    if url.trim().is_empty() {
        return Err("there is no address to open");
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the address is not one a browser is sent to");
    }
    let lower = url.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("https://") {
        return match rest.starts_with(['/', '?', '#']) {
            true => Err("the address names no host"),
            false => Ok(()),
        };
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        // A name before an `@` is not the host: `http://127.0.0.1@elsewhere/` goes elsewhere.
        let host = match authority.strip_prefix('[') {
            Some(inside) => inside.split(']').next().unwrap_or_default(),
            None => authority.split(':').next().unwrap_or_default(),
        };
        let loopback = !authority.contains('@')
            && (host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback()));
        return match loopback {
            true => Ok(()),
            false => Err("a plain http address is only opened on this machine"),
        };
    }
    Err("only an https address is opened")
}

/// What to do with the daemon's answer to a request of the section.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `connection.list`, the read `asked`.
    Listed {
        asked: u64,
    },
    Written(Writing),
    /// `connection.review` of `name`.
    Reviewed {
        name: String,
    },
    /// `connection.accept` of `name`.
    Accepted {
        name: String,
    },
    /// `connection.login` of `name`: the address to open in the browser.
    SignIn {
        name: String,
    },
}

/// How the server is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Command,
    Url,
}

/// What the form dialog is doing.
#[derive(Clone, Debug, PartialEq)]
enum Form {
    Closed,
    /// A new server, or the one named edited.
    Editing(Option<String>),
}

/// The fields of the form dialog.
#[derive(Clone)]
pub struct FormFields {
    pub name: Entity<InputState>,
    pub about: Entity<InputState>,
    pub command: Entity<InputState>,
    pub url: Entity<InputState>,
}

/// A variable of the form (a header, for a URL): its name typed, its entry picked.
struct Variable {
    key: String,
    name: Entity<InputState>,
    entry: Option<String>,
}

/// A limit of the form: an argument and the values it may take, separated by commas.
struct Limit {
    key: String,
    argument: Entity<InputState>,
    values: Entity<InputState>,
}

/// The review open, or the last one while it fades out.
struct Review {
    name: String,
    answer: Option<Result<Diff, String>>,
    /// What each action will be, by its name.
    settings: BTreeMap<String, String>,
    unchanged_open: bool,
    instructions_open: bool,
    /// Whether what the adapter changed is unfolded under the sentence that says it applies.
    adapter_open: bool,
    accepting: bool,
    error: Option<SharedString>,
    /// The field that filters the actions, made with the first frame of the dialog.
    search: Option<Entity<InputState>>,
    /// The chip chosen: `all`, a change (`new`, `changed`, `removed`) or a setting's word.
    filter: String,
    /// The actions ticked, which a pill or the bar sets together.
    picked: BTreeSet<String>,
    /// The actions whose row is unfolded, whichever group they are in.
    opened: HashSet<String>,
    /// The groups folded by a click on their header.
    folded: HashSet<&'static str>,
    /// The actions of the groups, a row that changes setting leaving one and arriving in another.
    rows: Grouped<&'static str, usize>,
    /// The actions of "What changed" in a re-review.
    changes: Listing<usize>,
    /// What the text typed found among the actions, made once for each text.
    found: Found,
    /// The rows folded by a click, with when: their body is still drawn while they fold away.
    folding: HashMap<String, Instant>,
}

/// Which actions a query finds, by their place in the review: matching them is the costly part of
/// a frame, so it is done when the text changes and not on each frame.
#[derive(Default)]
struct Found {
    query: Option<String>,
    hits: Vec<bool>,
    /// How many times an action was matched against a query.
    matched: usize,
}

impl Found {
    /// Matches the actions with `query` unless it is the text they were matched with already.
    fn of(&mut self, diff: &Diff, query: &str) {
        if self.query.as_deref() == Some(query) && self.hits.len() == diff.actions.len() {
            return;
        }
        // No word typed is every action, with nothing to match.
        self.hits = match query.split_whitespace().next() {
            None => vec![true; diff.actions.len()],
            Some(_) => diff
                .actions
                .iter()
                .map(|action| {
                    self.matched += 1;
                    let text = action.now.as_deref().or(action.pinned.as_deref());
                    fuzzy::find(query, &action.name, text.unwrap_or_default()).is_some()
                })
                .collect(),
        };
        self.query = Some(query.to_owned());
    }
}

impl Review {
    /// Whether the query finds the action at `at`.
    fn found(&self, at: usize) -> bool {
        self.found.hits.get(at).copied().unwrap_or(true)
    }

    /// Whether the action at `at` shows under what is searched and the chip chosen.
    fn shows(&self, at: usize, action: &ActionDiff) -> bool {
        self.found(at) && self.counts_for(action, &self.filter)
    }

    /// Whether the action is one the chip `key` counts: a change, or a setting.
    fn counts_for(&self, action: &ActionDiff, key: &str) -> bool {
        match key {
            "all" => true,
            "new" => action.change == "added",
            "changed" => action.change == "changed",
            "removed" => action.change == "removed",
            setting => action.change != "removed" && self.setting_of(&action.name) == setting,
        }
    }

    /// [`picked_shown`](Self::picked_shown) of the answer the review holds, for what a click sets.
    fn shown_picks(&mut self) -> Vec<String> {
        let answer = self.answer.take();
        let shown = match &answer {
            Some(Ok(diff)) => self.picked_shown(diff),
            _ => Vec::new(),
        };
        self.answer = answer;
        shown
    }

    /// The names of the rows ticked that show now: what the bar and a pill set. A ticked row the
    /// search or a chip hides stays ticked, and is set once it shows again.
    fn picked_shown(&self, diff: &Diff) -> Vec<String> {
        diff.actions
            .iter()
            .enumerate()
            .filter(|(at, action)| {
                self.shows(*at, action)
                    && action.locked.is_none()
                    && self.picked.contains(&action.name)
            })
            .map(|(_, action)| action.name.clone())
            .collect()
    }

    /// What the action will be: what was chosen, else what the daemon proposes.
    fn setting_of(&self, name: &str) -> &str {
        self.settings.get(name).map_or("judged", String::as_str)
    }
}

/// A number that moves to `target` over a panel's time whenever it changes, as the kit's `fade`
/// moves a state between 0 and 1: `id` keeps it, and a first value is simply there.
fn eased(id: ElementId, target: f32, window: &mut Window, cx: &mut gpui_kit::App) -> f32 {
    let key = ElementId::NamedChild(Arc::new(id), "eased".into());
    let state = window.use_keyed_state(key, cx, move |_, _| Motion::settled(target));
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    let motion = state.update(cx, |motion, _| {
        if motion.target() != target {
            motion.animate(target, PANEL, now, reduce);
        }
        *motion
    });
    if motion.is_running(now) {
        window.request_animation_frame();
    }
    motion.value(now)
}

pub struct ConnectionsSection {
    shell: WeakEntity<Shell>,
    vault: Entity<VaultSection>,
    listed: Option<Vec<Server>>,
    rows: Listing<Server>,
    /// Each row's menu, and whether the server was on when it was made: its entries follow.
    menus: HashMap<String, ((bool, bool), Entity<Menu>)>,
    problem: Option<SharedString>,
    asked: u64,
    listed_at: u64,
    form: Form,
    fields: Option<FormFields>,
    reach: Reach,
    variables: Vec<Variable>,
    variable_rows: Listing<String>,
    limits: Vec<Limit>,
    limit_rows: Listing<String>,
    made: u64,
    saving: bool,
    form_error: Option<SharedString>,
    review: Option<Review>,
    review_open: bool,
    confirming: Option<String>,
    /// The servers whose sign-in waits for the browser.
    signing: std::collections::HashSet<String>,
    /// The server the confirmation names, kept while it fades away.
    asked_name: String,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionsSection {
    pub fn new(shell: WeakEntity<Shell>, vault: Entity<VaultSection>) -> Self {
        Self {
            shell,
            vault,
            listed: None,
            rows: Listing::default(),
            menus: HashMap::new(),
            problem: None,
            asked: 0,
            listed_at: 0,
            form: Form::Closed,
            fields: None,
            reach: Reach::Command,
            variables: Vec::new(),
            variable_rows: Listing::default(),
            limits: Vec::new(),
            limit_rows: Listing::default(),
            made: 0,
            saving: false,
            form_error: None,
            review: None,
            review_open: false,
            confirming: None,
            signing: Default::default(),
            asked_name: String::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// The servers as last listed; `None` until the daemon has said.
    pub fn servers(&self) -> Option<&[Server]> {
        self.listed.as_deref()
    }

    /// How many are ready to be called: the tag of its row of Features.
    pub fn on_count(&self) -> Option<usize> {
        self.listed
            .as_ref()
            .map(|servers| servers.iter().filter(|s| s.is_on()).count())
    }

    pub fn form_fields(&self) -> Option<FormFields> {
        self.fields.clone()
    }

    /// The name field of the variable at `at` of the form.
    pub fn variable_field(&self, at: usize) -> Option<Entity<InputState>> {
        self.variables.get(at).map(|v| v.name.clone())
    }

    /// The argument and values fields of the limit at `at` of the form.
    pub fn limit_fields(&self, at: usize) -> Option<(Entity<InputState>, Entity<InputState>)> {
        self.limits
            .get(at)
            .map(|l| (l.argument.clone(), l.values.clone()))
    }

    /// The review's search field, once the dialog has drawn it.
    pub fn review_search(&self) -> Option<Entity<InputState>> {
        self.review
            .as_ref()
            .and_then(|review| review.search.clone())
    }

    /// How many times an action was matched with what is typed in the review's search: once per
    /// action when the text changes, never on a frame that draws the same text again.
    pub fn review_matches(&self) -> usize {
        self.review.as_ref().map_or(0, |r| r.found.matched)
    }

    fn ask(&self, method: &str, params: Value, then: Then, cx: &mut Context<Self>) {
        let command = Command::Ask {
            method: method.into(),
            params,
            then: super::Then::Connections(then),
        };
        perform_later(&self.shell, command, cx);
    }

    /// Asks the daemon for the list.
    pub fn read(&mut self, cx: &mut Context<Self>) {
        self.asked += 1;
        self.ask(
            "connection.list",
            json!({}),
            Then::Listed { asked: self.asked },
            cx,
        );
    }

    /// What the daemon answered to something the section asked.
    pub fn answered(
        &mut self,
        then: Then,
        answer: Result<Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match then {
            Then::Listed { asked } => {
                if asked < self.listed_at {
                    return;
                }
                self.listed_at = asked;
                match answer {
                    Ok(listed) => {
                        let list: Vec<Server> = listed["connections"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(Server::of)
                            .collect();
                        self.problem = listed["error"]
                            .as_str()
                            .map(|e| SharedString::from(e.to_owned()));
                        if self.listed.as_ref() != Some(&list) {
                            self.listed = Some(list);
                        }
                    }
                    Err(why) => self.problem = Some(why.into()),
                }
            }
            Then::Written(writing) => self.written(writing, answer, cx),
            Then::Reviewed { name } => {
                if let Some(review) = self.review.as_mut().filter(|r| r.name == name) {
                    let answer = answer.map(|diff| Diff::of(&diff));
                    if let Ok(diff) = &answer {
                        review.settings = diff
                            .actions
                            .iter()
                            .filter(|a| a.change != "removed")
                            .map(|a| {
                                let setting = a.setting.clone().unwrap_or_else(|| "judged".into());
                                (a.name.clone(), setting)
                            })
                            .collect();
                    }
                    review.answer = Some(answer);
                    review.found.query = None;
                }
            }
            Then::SignIn { name } => match answer {
                Ok(begun) => {
                    let url = begun["url"].as_str().unwrap_or_default().to_owned();
                    match sign_in_address(&url) {
                        Ok(()) => {
                            // The answer comes while the shell is busy giving it: it opens after.
                            let shell = self.shell.clone();
                            cx.defer(move |cx| {
                                shell.update(cx, |shell, _| shell.open_link(&url)).ok();
                            });
                        }
                        Err(why) => {
                            self.signing.remove(&name);
                            toast(
                                Toast::failure(format!("{name} was not signed in")).detail(why),
                                cx,
                            );
                        }
                    }
                }
                Err(why) => {
                    self.signing.remove(&name);
                    toast(
                        Toast::failure(format!("{name} was not signed in")).detail(why),
                        cx,
                    );
                }
            },
            Then::Accepted { name } => {
                let Some(review) = self.review.as_mut().filter(|r| r.name == name) else {
                    return;
                };
                review.accepting = false;
                match answer {
                    Ok(_) => {
                        self.review_open = false;
                        toast(
                            Toast::success(format!("Server {name} accepted"))
                                .detail("Agents get its tool while it is on."),
                            cx,
                        );
                        self.read(cx);
                    }
                    Err(why) => review.error = Some(why.into()),
                }
            }
        }
        let _ = window;
        cx.notify();
    }

    fn written(&mut self, writing: Writing, answer: Result<Value, String>, cx: &mut Context<Self>) {
        self.saving = false;
        match answer {
            Ok(saved) => {
                // The daemon says whether this very write gave the tool to agents (it pins what a
                // server announces as it is added, edited or turned on): the list the window
                // holds may already be the new one, so the answer is what is read.
                let given = saved["given"] == true;
                let reviewing = |toast: Toast, name: &str, cx: &mut Context<Self>| {
                    let review = cx.weak_entity();
                    let name = name.to_owned();
                    toast.action("Review", move |_, cx| {
                        review
                            .update(cx, |this, cx| this.open_review(&name, cx))
                            .ok();
                    })
                };
                let said = match &writing {
                    Writing::Added(name)
                    | Writing::Edited(name)
                    | Writing::Switched(name, true)
                        if given =>
                    {
                        let detail = match writing {
                            Writing::Added(_) => "Every action is judged.",
                            _ => "Review it to change what each action does.",
                        };
                        reviewing(
                            Toast::success(format!("Server {name} is given to agents"))
                                .detail(detail),
                            name,
                            cx,
                        )
                    }
                    Writing::Added(name) => Toast::success(format!("Server {name} added")),
                    Writing::Edited(name) => Toast::success(format!("Server {name} saved")),
                    Writing::Switched(name, true) => {
                        Toast::success(format!("Server {name} turned on"))
                    }
                    Writing::Switched(name, false) => {
                        Toast::success(format!("Server {name} turned off"))
                            .detail("Its tool left every agent.")
                    }
                    Writing::Removed(name) => Toast::success(format!("Server {name} removed")),
                };
                // What is not given says why: its server did not start, it announces a change
                // nobody read, or it is off.
                let said = match (&writing, saved["state"].as_str(), saved["failing"].as_str()) {
                    (Writing::Removed(_) | Writing::Switched(_, false), ..) => said,
                    (_, Some("awaiting review"), Some(why)) => {
                        said.detail(format!("It waits for its review: {why}"))
                    }
                    (Writing::Added(name), Some("awaiting review"), None) => reviewing(
                        said.detail("Review it to give agents its tool."),
                        name,
                        cx,
                    ),
                    (
                        Writing::Edited(name) | Writing::Switched(name, true),
                        Some("awaiting review"),
                        None,
                    ) => reviewing(
                        said.detail("Its server announces a change: review it to give agents its tool again."),
                        name,
                        cx,
                    ),
                    (Writing::Added(_), Some("off"), _) => {
                        said.detail("It is off: agents get its tool when you turn it on.")
                    }
                    _ => said,
                };
                if matches!(writing, Writing::Added(_) | Writing::Edited(_)) {
                    self.form = Form::Closed;
                    self.form_error = None;
                }
                toast(said, cx);
                self.read(cx);
            }
            Err(why) => match writing {
                Writing::Added(_) | Writing::Edited(_) if self.form != Form::Closed => {
                    self.form_error = Some(why.into())
                }
                Writing::Switched(name, _) => toast(
                    Toast::failure(format!("Server {name} was not switched")).detail(why),
                    cx,
                ),
                Writing::Removed(name) => toast(
                    Toast::failure(format!("Server {name} was not removed")).detail(why),
                    cx,
                ),
                Writing::Added(name) | Writing::Edited(name) => toast(
                    Toast::failure(format!("Server {name} was not saved")).detail(why),
                    cx,
                ),
            },
        }
        cx.notify();
    }

    /// Begins the sign-in of `name`: the daemon answers the address its browser opens.
    pub fn sign_in(&mut self, name: &str, cx: &mut Context<Self>) {
        self.signing.insert(name.to_owned());
        self.ask(
            "connection.login",
            json!({"name": name}),
            Then::SignIn {
                name: name.to_owned(),
            },
            cx,
        );
        cx.notify();
    }

    /// An event of a sign-in: how it ended, or that its tokens were lost or not kept.
    pub fn sign_in_said(
        &mut self,
        said: &str,
        name: &str,
        why: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let why = why.unwrap_or_default();
        let shown = match said {
            "signed_in" => Toast::success(format!("Signed in to {name}"))
                .detail("Its server is reached with your account from now on."),
            "sign_in_failed" => Toast::failure(format!("{name} was not signed in")).detail(why),
            "signed_out" => Toast::failure(format!("{name} is signed out"))
                .detail("Its tool left every agent; sign in again from its menu."),
            _ => Toast::failure(format!("{name}'s sign-in was not saved")).detail(why),
        };
        if matches!(said, "signed_in" | "sign_in_failed") {
            self.signing.remove(name);
        }
        toast(shown, cx);
        self.read(cx);
        cx.notify();
    }

    fn server(&self, name: &str) -> Option<Server> {
        self.listed
            .iter()
            .flatten()
            .find(|server| server.name == name)
            .cloned()
    }

    // ── The form ─────────────────────────────────────────────────────────────────────────────

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> FormFields {
        if self.fields.is_none() {
            let fields = FormFields {
                name: line_field("almanac", window, cx),
                about: line_field("The notes and decisions I keep", window, cx),
                command: line_field("almanac mcp", window, cx),
                url: line_field("https://notes.example.com/mcp", window, cx),
            };
            for input in [&fields.name, &fields.about, &fields.command, &fields.url] {
                self.watch(input, cx);
            }
            self.fields = Some(fields);
        }
        self.fields.clone().expect("made above")
    }

    /// Typing clears what a refusal said.
    fn watch(&mut self, input: &Entity<InputState>, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.subscribe(input, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                    cx.notify();
                }
            }));
    }

    /// "New server": the dialog, empty.
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fill(None, window, cx);
    }

    /// "Edit", or a click on its row: the dialog on its declaration.
    pub fn edit(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(server) = self.server(name) {
            self.fill(Some(&server), window, cx);
        }
    }

    fn fill(&mut self, server: Option<&Server>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(window, cx);
        let line = server
            .and_then(|s| s.command.as_deref())
            .map(command_line)
            .unwrap_or_default();
        for (field, value) in [
            (
                &fields.name,
                server.map(|s| s.name.clone()).unwrap_or_default(),
            ),
            (
                &fields.about,
                server.map(|s| s.about.clone()).unwrap_or_default(),
            ),
            (&fields.command, line),
            (
                &fields.url,
                server.and_then(|s| s.url.clone()).unwrap_or_default(),
            ),
        ] {
            field.update(cx, |f, cx| f.set_value(value, window, cx));
        }
        self.reach = match server.is_some_and(|s| s.url.is_some()) {
            true => Reach::Url,
            false => Reach::Command,
        };
        self.variables.clear();
        self.limits.clear();
        if let Some(server) = server {
            let entries = match self.reach {
                Reach::Command => &server.env,
                Reach::Url => &server.headers,
            };
            for (name, entry) in entries.clone() {
                self.new_variable(Some((name, entry)), window, cx);
            }
            for (argument, values) in server.only.clone() {
                self.new_limit(Some((argument, values.join(", "))), window, cx);
            }
        }
        // The rows the dialog opens with are there at once.
        self.variable_rows = Listing::default();
        self.limit_rows = Listing::default();
        self.form = Form::Editing(server.map(|s| s.name.clone()));
        self.form_error = None;
        self.saving = false;
        self.vault.update(cx, |vault, cx| vault.read(cx));
        let focus = match server {
            Some(_) => &fields.about,
            None => &fields.name,
        }
        .read(cx)
        .focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn next_key(&mut self) -> String {
        self.made += 1;
        self.made.to_string()
    }

    fn new_variable(
        &mut self,
        given: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = line_field("NOTES_TOKEN", window, cx);
        let (typed, entry) = match given {
            Some((typed, entry)) => (typed, Some(entry)),
            None => (String::new(), None),
        };
        name.update(cx, |f, cx| f.set_value(typed, window, cx));
        self.watch(&name, cx);
        let key = self.next_key();
        self.variables.push(Variable { key, name, entry });
        cx.notify();
    }

    fn new_limit(
        &mut self,
        given: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let argument = line_field("app", window, cx);
        let values = line_field("gedit, Files", window, cx);
        let (a, v) = given.unwrap_or_default();
        argument.update(cx, |f, cx| f.set_value(a, window, cx));
        values.update(cx, |f, cx| f.set_value(v, window, cx));
        self.watch(&argument, cx);
        self.watch(&values, cx);
        let key = self.next_key();
        self.limits.push(Limit {
            key,
            argument,
            values,
        });
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.form = Form::Closed;
        self.form_error = None;
        cx.notify();
    }

    /// Saves the dialog: a new server, or the one edited.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Form::Editing(original) = self.form.clone() else {
            return;
        };
        if self.saving {
            return;
        }
        match self.declaration(original.as_deref(), cx) {
            Ok((params, writing)) => {
                let method = match original {
                    Some(_) => "connection.edit",
                    None => "connection.add",
                };
                self.saving = true;
                self.form_error = None;
                self.ask(method, params, Then::Written(writing), cx);
            }
            Err(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    /// The declaration the dialog says, or why it cannot be sent.
    fn declaration(
        &self,
        original: Option<&str>,
        cx: &Context<Self>,
    ) -> Result<(Value, Writing), String> {
        let fields = self.fields.as_ref().ok_or("no form")?;
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let name = original.map_or_else(|| value(&fields.name), str::to_owned);
        let about = value(&fields.about);
        if name.is_empty() || about.is_empty() {
            return Err("A server needs a name and a sentence on what it is for.".into());
        }
        let mut entries = Map::new();
        for variable in &self.variables {
            let typed = value(&variable.name);
            match (&variable.entry, typed.is_empty()) {
                (None, true) => {}
                (Some(entry), false) => {
                    entries.insert(typed, json!({"vault": entry}));
                }
                (None, false) => return Err(format!("Pick the vault entry {typed} is given.")),
                (Some(_), true) => return Err("Name each variable its entry is given as.".into()),
            }
        }
        let mut only = Map::new();
        for limit in &self.limits {
            let argument = value(&limit.argument);
            let values: Vec<String> = value(&limit.values)
                .split(',')
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .collect();
            match (argument.is_empty(), values.is_empty()) {
                (true, true) => {}
                (false, false) => {
                    only.insert(argument, json!(values));
                }
                _ => return Err("A limit needs an argument and the values it may take.".into()),
            }
        }
        let editing = original.is_some();
        // In an edit, an empty table removes what was there.
        let table = |map: Map<String, Value>| match (map.is_empty(), editing) {
            (true, true) => Value::Null,
            _ => Value::Object(map),
        };
        let mut params = json!({"name": name, "about": about});
        match self.reach {
            Reach::Command => {
                let line = value(&fields.command);
                if line.is_empty() {
                    return Err("Say the command that starts the server.".into());
                }
                let words = flottille_core::command::split_line(&line)?;
                params["command"] = json!(words);
                params["env"] = table(entries);
                if editing {
                    params["headers"] = Value::Null;
                }
            }
            Reach::Url => {
                let url = value(&fields.url);
                if url.is_empty() {
                    return Err("Say the URL of the server.".into());
                }
                params["url"] = json!(url);
                params["headers"] = table(entries);
                if editing {
                    params["env"] = Value::Null;
                }
            }
        }
        params["only"] = table(only);
        if !editing {
            for key in ["env", "headers", "only"] {
                if params[key].as_object().is_some_and(Map::is_empty) {
                    params.as_object_mut().unwrap().remove(key);
                }
            }
        }
        let writing = match original {
            Some(_) => Writing::Edited(name),
            None => Writing::Added(name),
        };
        Ok((params, writing))
    }

    // ── A row's gestures ─────────────────────────────────────────────────────────────────────

    fn switch(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(server) = self.server(name) else {
            return;
        };
        let on = !server.enabled;
        self.ask(
            "connection.edit",
            json!({"name": name, "enabled": on}),
            Then::Written(Writing::Switched(name.to_owned(), on)),
            cx,
        );
    }

    /// Asks to remove, in a confirmation; called again from it, removes.
    pub fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(name) {
            self.confirming = Some(name.to_owned());
            self.asked_name = name.to_owned();
            cx.notify();
            return;
        }
        self.confirming = None;
        self.ask(
            "connection.remove",
            json!({"name": name}),
            Then::Written(Writing::Removed(name.to_owned())),
            cx,
        );
        cx.notify();
    }

    /// Opens the review of `name`: the daemon starts its server and says what it announces.
    pub fn open_review(&mut self, name: &str, cx: &mut Context<Self>) {
        self.review = Some(Review {
            name: name.to_owned(),
            answer: None,
            settings: BTreeMap::new(),
            unchanged_open: false,
            instructions_open: false,
            adapter_open: false,
            accepting: false,
            error: None,
            search: None,
            filter: "all".into(),
            picked: BTreeSet::new(),
            opened: HashSet::new(),
            folded: HashSet::new(),
            rows: Grouped::new("connection-review-row"),
            changes: Listing::default(),
            found: Found::default(),
            folding: HashMap::new(),
        });
        self.review_open = true;
        self.ask(
            "connection.review",
            json!({"name": name}),
            Then::Reviewed {
                name: name.to_owned(),
            },
            cx,
        );
        cx.notify();
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.review.as_mut().filter(|r| !r.accepting) else {
            return;
        };
        review.accepting = true;
        review.error = None;
        let (name, settings) = (review.name.clone(), review.settings.clone());
        self.ask(
            "connection.accept",
            json!({"name": name, "actions": settings}),
            Then::Accepted { name: name.clone() },
            cx,
        );
        cx.notify();
    }

    fn menu(&mut self, server: &Server, cx: &mut Context<Self>) -> Entity<Menu> {
        let url = server.url.is_some();
        if let Some((made, menu)) = self.menus.get(&server.name)
            && *made == (server.enabled, url)
        {
            return menu.clone();
        }
        let this = cx.entity().downgrade();
        let name = server.name.clone();
        let on = server.enabled;
        let id = ElementId::Name(format!("connection-more-{name}").into());
        let menu = cx.new(|cx| {
            let (edit, review, switch, remove) =
                (this.clone(), this.clone(), this.clone(), this.clone());
            let (to_edit, to_review, to_switch, to_remove) =
                (name.clone(), name.clone(), name.clone(), name.clone());
            let menu = Menu::new(id, "More", cx)
                .entry(
                    format!("edit-connection-{name}"),
                    "Edit",
                    Some(Icon::Edit),
                    move |window, cx| {
                        edit.update(cx, |this, cx| this.edit(&to_edit, window, cx))
                            .ok();
                    },
                )
                .entry(
                    format!("review-connection-{name}"),
                    "Review",
                    Some(Icon::Lucide("scan-search")),
                    move |_, cx| {
                        review
                            .update(cx, |this, cx| this.open_review(&to_review, cx))
                            .ok();
                    },
                )
                .entry(
                    format!("switch-connection-{name}"),
                    if on { "Turn off" } else { "Turn on" },
                    Some(Icon::Lucide("power")),
                    move |_, cx| {
                        switch
                            .update(cx, |this, cx| this.switch(&to_switch, cx))
                            .ok();
                    },
                );
            // Signing in is a URL's: a program reaches no account of the user's.
            let menu = match url {
                true => {
                    let (sign, to_sign) = (this.clone(), name.clone());
                    menu.entry(
                        format!("sign-in-connection-{name}"),
                        "Sign in",
                        Some(Icon::Lucide("log-in")),
                        move |_, cx| {
                            sign.update(cx, |this, cx| this.sign_in(&to_sign, cx)).ok();
                        },
                    )
                }
                false => menu,
            };
            menu.danger(
                format!("remove-connection-{name}"),
                "Remove…",
                move |_, cx| {
                    remove
                        .update(cx, |this, cx| this.remove(&to_remove, cx))
                        .ok();
                },
            )
        });
        self.menus.insert(name, ((on, url), menu.clone()));
        menu
    }

    fn row(&mut self, server: &Server, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let name = server.name.clone();
        let menu = self.menu(server, cx);
        let mut row = ListRow::new(
            ElementId::Name(format!("connection-{name}").into()),
            SharedString::from(name.clone()),
        )
        .mono()
        .leading(icon(Icon::Lucide("plug")).size(px(14.)).text_color(p.faint));
        if let Some(version) = &server.version {
            row = row.tag(SharedString::from(version.clone()));
        }
        let line = match self.signing.contains(&server.name) {
            true => "Signing in: finish it in your browser".to_owned(),
            false => server.line(),
        };
        row.line(SharedString::from(line))
            .more(menu)
            .on_click(cx.listener(move |this, _, window, cx| {
                if !this.saving {
                    this.edit(&name, window, cx)
                }
            }))
    }

    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let name = self.asked_name.clone();
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("connection-removal", format!("Remove the server {name}?"))
            .sentence(
                "Its tool leaves every agent, its server is stopped, and what you accepted of it \
                 is forgotten.",
            )
            .action("Remove server")
            .open(self.confirming.is_some())
            .on_confirm(move |_, cx| {
                let name = name.clone();
                this.update(cx, |this, cx| this.remove(&name, cx)).ok();
            })
            .on_cancel(move |_, cx| {
                again
                    .update(cx, |this, cx| {
                        this.confirming = None;
                        cx.notify();
                    })
                    .ok();
            })
    }

    // ── The form dialog ──────────────────────────────────────────────────────────────────────

    /// The vault entries a server may be given: every project's, and not bound to another use.
    fn offered(&self, name: &str, cx: &Context<Self>) -> Vec<VaultEntry> {
        let bound = flottille_core::vault::connection(name);
        self.vault
            .read(cx)
            .entries()
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.project.is_none())
            .filter(|entry| entry.bound.as_ref().is_none_or(|b| *b == bound))
            .cloned()
            .collect()
    }

    fn variable_row(
        &self,
        at: usize,
        variable: &Variable,
        offered: &[VaultEntry],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("connection-variable-{at}");
        let mut entries: Vec<String> = offered.iter().map(|e| e.name.clone()).collect();
        // An entry the declaration names that is no longer offered still says itself.
        if let Some(own) = &variable.entry
            && !entries.contains(own)
        {
            entries.push(own.clone());
        }
        let mut options = vec![SelectOption::new("none", "Pick an entry")];
        options.extend(entries.iter().map(|name| {
            let line = offered
                .iter()
                .find(|e| &e.name == name)
                .and_then(|e| e.description.clone())
                .unwrap_or_default();
            SelectOption::new(
                SharedString::from(name.clone()),
                SharedString::from(name.clone()),
            )
            .line(SharedString::from(line))
        }));
        let chosen = variable
            .entry
            .as_ref()
            .and_then(|own| entries.iter().position(|e| e == own))
            .map_or(0, |at| at + 1);
        let key = variable.key.clone();
        let this = cx.entity().downgrade();
        let select = Select::new(SharedString::from(format!("{id}-entry")), options, chosen)
            .on_choose(move |index, _, cx| {
                let entry = index.checked_sub(1).and_then(|i| entries.get(i).cloned());
                let key = key.clone();
                this.update(cx, |this, cx| {
                    if let Some(v) = this.variables.iter_mut().find(|v| v.key == key) {
                        v.entry = entry;
                    }
                    this.form_error = None;
                    cx.notify();
                })
                .ok();
            });
        let key = variable.key.clone();
        let remove = Button::icon(
            SharedString::from(format!("{id}-remove")),
            Icon::Close,
            "Remove this variable",
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.variables.retain(|v| v.key != key);
            cx.notify();
        }));
        let (name, entry) = match self.reach {
            Reach::Command => ("Variable", "Vault entry"),
            Reach::Url => ("Header", "Vault entry"),
        };
        div()
            .id(SharedString::from(id.clone()))
            .flex()
            .items_end()
            .gap(space::SM)
            .child(div().flex_1().min_w_0().child(Field::new(
                SharedString::from(format!("{id}-name-field")),
                name,
                boxed(format!("{id}-name"), Input::new(&variable.name), cx),
            )))
            .child(div().flex_1().min_w_0().child(Field::new(
                SharedString::from(format!("{id}-entry-field")),
                entry,
                select,
            )))
            .child(remove)
            .into_any_element()
    }

    fn limit_row(&self, at: usize, limit: &Limit, cx: &mut Context<Self>) -> AnyElement {
        let id = format!("connection-limit-{at}");
        let key = limit.key.clone();
        let remove = Button::icon(
            SharedString::from(format!("{id}-remove")),
            Icon::Close,
            "Remove this limit",
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.limits.retain(|l| l.key != key);
            cx.notify();
        }));
        div()
            .id(SharedString::from(id.clone()))
            .flex()
            .items_end()
            .gap(space::SM)
            .child(div().w(px(160.)).child(Field::new(
                SharedString::from(format!("{id}-argument-field")),
                "Argument",
                boxed(format!("{id}-argument"), Input::new(&limit.argument), cx),
            )))
            .child(div().flex_1().min_w_0().child(Field::new(
                SharedString::from(format!("{id}-values-field")),
                "Allowed values",
                boxed(format!("{id}-values"), Input::new(&limit.values), cx),
            )))
            .child(remove)
            .into_any_element()
    }

    fn modal(&mut self, window: &Window, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let title = match &self.form {
            Form::Editing(Some(name)) => format!("Edit {name}"),
            _ => "New server".to_owned(),
        };
        let modal = Modal::new("connection-modal", title)
            .wide()
            .open(self.form != Form::Closed)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let Form::Editing(original) = self.form.clone() else {
            return modal;
        };
        let Some(fields) = self.fields.clone() else {
            return modal;
        };
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let keys: Vec<(String, String)> = self
            .variables
            .iter()
            .map(|v| (v.key.clone(), v.key.clone()))
            .collect();
        self.variable_rows.sync(keys, now, reduce);
        let keys: Vec<(String, String)> = self
            .limits
            .iter()
            .map(|l| (l.key.clone(), l.key.clone()))
            .collect();
        self.limit_rows.sync(keys, now, reduce);

        let name = match &original {
            Some(name) => div()
                .id("connection-name")
                .h(px(32.))
                .flex()
                .items_center()
                .text_color(p.mute)
                .child(SharedString::from(name.clone()))
                .test_support()
                .into_any_element(),
            None => boxed("connection-name", Input::new(&fields.name), cx).into_any_element(),
        };
        let reach = {
            let this = cx.entity().downgrade();
            RadioChoices::new("connection-reach", "How it is reached")
                .choice(
                    "command",
                    "A program",
                    "Flottille starts it on this machine and stops it when no session runs.",
                )
                .choice(
                    "url",
                    "A URL",
                    "A server you run yourself, or one elsewhere over HTTPS.",
                )
                .chosen(match self.reach {
                    Reach::Command => 0,
                    Reach::Url => 1,
                })
                .on_choose(move |index, _, cx| {
                    this.update(cx, |this, cx| {
                        this.reach = if index == 0 {
                            Reach::Command
                        } else {
                            Reach::Url
                        };
                        this.form_error = None;
                        cx.notify();
                    })
                    .ok();
                })
        };
        let command = Field::new(
            "connection-command-field",
            "Command",
            boxed("connection-command", Input::new(&fields.command), cx),
        )
        .hint("The program and its arguments, run without a shell.");
        let url = Field::new(
            "connection-url-field",
            "URL",
            boxed("connection-url", Input::new(&fields.url), cx),
        )
        .hint("https://, or http:// on this machine.");
        let offered = self.offered(original.as_deref().unwrap_or_default(), cx);
        let mut variables = div().flex().flex_col();
        for row in self
            .variable_rows
            .rows()
            .map(|r| (r.key.to_owned(), r.placement))
            .collect::<Vec<_>>()
        {
            let (key, placement) = row;
            let at = self.variables.iter().position(|v| v.key == key);
            let content = match at {
                Some(at) => self.variable_row(at, &self.variables[at], &offered, cx),
                None => div().into_any_element(),
            };
            variables = variables.child(
                gpui_design_system::Item::new(
                    ElementId::Name(format!("connection-variable-item-{key}").into()),
                    placement,
                    content,
                )
                .gap(space::SM),
            );
        }
        let mut limits = div().flex().flex_col();
        for (key, placement) in self
            .limit_rows
            .rows()
            .map(|r| (r.key.to_owned(), r.placement))
            .collect::<Vec<_>>()
        {
            let at = self.limits.iter().position(|l| l.key == key);
            let content = match at {
                Some(at) => self.limit_row(at, &self.limits[at], cx),
                None => div().into_any_element(),
            };
            limits = limits.child(
                gpui_design_system::Item::new(
                    ElementId::Name(format!("connection-limit-item-{key}").into()),
                    placement,
                    content,
                )
                .gap(space::SM),
            );
        }
        let (variables_label, add_label) = match self.reach {
            Reach::Command => ("Variables of its environment", "New variable"),
            Reach::Url => ("Headers of its requests", "New header"),
        };
        let new_variable = Button::secondary("connection-new-variable", add_label)
            .leading(Icon::Plus)
            .on_click(cx.listener(|this, _, window, cx| this.new_variable(None, window, cx)));
        let new_limit = Button::secondary("connection-new-limit", "New limit")
            .leading(Icon::Plus)
            .on_click(cx.listener(|this, _, window, cx| this.new_limit(None, window, cx)));
        let body = div()
            .id("connection-form")
            .max_h(window.viewport_size().height - px(240.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(
                Field::new("connection-name-field", "Name", name)
                    .hint("Lower-case letters, digits and _; its tool is conn_ and this name."),
            )
            .child(Field::new(
                "connection-about-field",
                "What it is for",
                boxed("connection-about", Input::new(&fields.about), cx),
            ))
            .child(Field::new(
                "connection-reach-field",
                "How it is reached",
                reach,
            ))
            .child(presence(
                "connection-command-block",
                self.reach == Reach::Command,
                command,
            ))
            .child(presence(
                "connection-url-block",
                self.reach == Reach::Url,
                url,
            ))
            .child(
                Field::new(
                    "connection-variables-field",
                    variables_label,
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(div().w_full().child(variables))
                        .child(div().pt(space::SM).child(new_variable)),
                )
                .optional()
                .hint(VARIABLES_HINT),
            )
            .child(
                Field::new(
                    "connection-limits-field",
                    "Limits on arguments",
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(div().w_full().child(limits))
                        .child(div().pt(space::SM).child(new_limit)),
                )
                .optional()
                .hint(LIMITS_HINT),
            )
            .test_support();
        let error = problem_line("connection-error", self.form_error.clone(), space::LG, cx);
        modal
            .sentence(FORM_SENTENCE)
            .child(body)
            .kept(error)
            .action(
                Button::primary(
                    "save-connection",
                    match (self.saving, original.is_some()) {
                        (true, true) => "Saving…",
                        (true, false) => "Adding…",
                        (false, true) => "Save",
                        (false, false) => "Add",
                    },
                )
                .disabled(self.saving)
                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
    }

    // ── The review dialog ────────────────────────────────────────────────────────────────────

    /// Takes the rows the review shows now into its lists, so that a row that left folds away and
    /// one that arrived unfolds: a changed one goes to "What changed" (a re-review), the others to
    /// the group of their setting.
    fn sync_review(review: &mut Review, diff: &Diff, now: Instant, reduce: bool) {
        let (mut grouped, mut changes) = (Vec::new(), Vec::new());
        for (at, action) in diff.actions.iter().enumerate() {
            if !review.shows(at, action) {
                continue;
            }
            match !diff.first && action.change != "unchanged" {
                true => changes.push((action.name.clone(), at)),
                false => {
                    let group = GROUPS
                        .iter()
                        .find(|(key, ..)| *key == review.setting_of(&action.name))
                        .map_or("judged", |(key, ..)| *key);
                    grouped.push((action.name.clone(), group, at));
                }
            }
        }
        review
            .rows
            .sync(GROUPS.map(|(key, ..)| key), grouped, now, reduce);
        review.changes.sync(changes, now, reduce);
    }

    /// The description, arguments and hints of an action as the lines it shows: both texts where it
    /// changed, the removed one alone where it went.
    fn action_lines(action: &ActionDiff) -> Vec<Line> {
        match (action.pinned.as_deref(), action.now.as_deref()) {
            (Some(old), Some(new)) if old != new => line_diff(old, new),
            (_, Some(now)) => now.lines().map(|l| Line::Same(l.to_owned())).collect(),
            (Some(old), None) => old.lines().map(|l| Line::Removed(l.to_owned())).collect(),
            (None, None) => Vec::new(),
        }
    }

    /// One row of the review: a checkbox, the name, the first line of what it does and a pill with
    /// its setting; it unfolds into its whole text and the four choices. `part` names what the row
    /// holds: a row of a group and a row of "What changed" have ids of their own.
    fn action_row(
        &self,
        review: &Review,
        diff: &Diff,
        at: usize,
        part: &dyn Fn(&str) -> ElementId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let action = &diff.actions[at];
        let removed = action.change == "removed";
        // An action the adapter locks is off whatever was chosen, and no setting turns it on.
        let locked = action.locked.as_deref();
        let setting = match locked {
            Some(_) => "off".to_owned(),
            None => review.setting_of(&action.name).to_owned(),
        };
        let change = match (diff.first, action.change.as_str()) {
            (true, _) | (_, "unchanged") => None,
            (_, "added") => Some("New"),
            (_, "removed") => Some("Removed"),
            _ => Some("Changed"),
        };
        let about = match removed {
            true => "Leaves the pin.".to_owned(),
            false => action
                .now
                .as_deref()
                .and_then(|text| text.lines().next())
                .unwrap_or_default()
                .to_owned(),
        };
        // What is off is dimmed, over a hover's time when its setting changes.
        let dim = fade(part("dim"), removed || setting == "off", HOVER, window, cx);
        let name = div()
            .id(part("name"))
            .aria_label(SharedString::from(action.name.clone()))
            // One width for every name, so that the pills and the sentences line up.
            .w(space::XL * 7.)
            .flex_none()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .mono(12.)
            .medium()
            .text_color(p.text)
            .child(SharedString::from(action.name.clone()));
        let name = match removed {
            true => name.line_through(),
            false => name,
        };
        let said = div()
            .id(part("about"))
            .aria_label(SharedString::from(about.clone()))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(p.mute)
            .child(SharedString::from(about.clone()));
        let line = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .opacity(1.0 - 0.45 * dim)
            .child(tipped_when_cut(
                name,
                SharedString::from(action.name.clone()),
                window,
                cx,
            ))
            .children(change.map(|change| Tag::new(part("change"), change)))
            .child(tipped_when_cut(said, SharedString::from(about), window, cx));

        let this = cx.entity().downgrade();
        let key = action.name.clone();
        let opened = review.opened.contains(&action.name);
        let mut row =
            Unfolding::new(part("row"), opened)
                .line(line)
                .on_toggle(move |open, _, cx| {
                    let now = cx.background_executor().now();
                    this.update(cx, |this, cx| {
                        if let Some(review) = this.review.as_mut() {
                            match open {
                                true => {
                                    review.folding.remove(&key);
                                    review.opened.insert(key.clone())
                                }
                                false => {
                                    review.folding.insert(key.clone(), now);
                                    review.opened.remove(&key)
                                }
                            };
                        }
                        cx.notify();
                    })
                    .ok();
                });

        // A row's text and choices are drawn while it is open and while it folds away, not on
        // every frame of a long list.
        let now = cx.background_executor().now();
        let folding = !cx.reduce_motion()
            && review
                .folding
                .get(&action.name)
                .is_some_and(|since| now.saturating_duration_since(*since) < PANEL + HOVER);
        let drawn = opened || folding;
        let mut body = div().flex().flex_col().gap(space::SM);
        if drawn {
            body = body.child(DiffLines::new(
                part("text"),
                Self::action_lines(action).iter().map(Line::diff_line),
            ));
        }
        if !removed {
            // Ticked, it is one of the rows a pill or the bar sets together; a locked row is
            // never one of them, so it has no box.
            if locked.is_none() {
                let state = match review.picked.contains(&action.name) {
                    true => CheckState::Checked,
                    false => CheckState::Empty,
                };
                let this = cx.entity().downgrade();
                let key = action.name.clone();
                row = row.leading(
                    Checkbox::new(part("pick"), state, format!("Select {}", action.name))
                        .on_change(move |state, _, cx| {
                            this.update(cx, |this, cx| {
                                if let Some(review) = this.review.as_mut() {
                                    match state {
                                        CheckState::Checked => review.picked.insert(key.clone()),
                                        _ => review.picked.remove(&key),
                                    };
                                }
                                cx.notify();
                            })
                            .ok();
                        }),
                );
            }
            let chosen = slot_of(&setting);
            let (this, key) = (cx.entity().downgrade(), action.name.clone());
            row = row.trailing(
                Select::new(
                    part("setting"),
                    SETTINGS.map(|(key, title, line, tone)| {
                        SelectOption::new(key, title).line(line).tone(tone)
                    }),
                    chosen,
                )
                .compact()
                .disabled(locked.is_some())
                .on_choose(move |index, _, cx| {
                    this.update(cx, |this, cx| this.set_actions(&key, SETTINGS[index].0, cx))
                        .ok();
                }),
            );
            if let (true, Some(why)) = (drawn, locked) {
                let said = format!("Off while Flottille's adapter applies: {why}.");
                body = body.child(
                    div()
                        .id(part("locked"))
                        .aria_label(SharedString::from(said.clone()))
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(SharedString::from(said))
                        .test_support(),
                );
            } else if drawn {
                let choose = part("choose");
                let choices = SETTINGS
                    .iter()
                    .enumerate()
                    .map(|(index, (key, title, line, _))| {
                        let (this, name) = (cx.entity().downgrade(), action.name.clone());
                        let id = match &choose {
                            ElementId::Name(id) => ElementId::Name(format!("{id}-{key}").into()),
                            other => ElementId::NamedChild(Arc::new(other.clone()), (*key).into()),
                        };
                        ChoiceRow::new(id, *title)
                            .description(*line)
                            .chosen(index == chosen)
                            .on_click(move |_, cx| {
                                this.update(cx, |this, cx| this.set_actions(&name, key, cx))
                                    .ok();
                            })
                    });
                body = body.child(Field::new(
                    part("when"),
                    WHEN_CALLED,
                    div().flex().flex_col().gap(space::XS).children(choices),
                ));
            }
        }
        row.body(body).into_any_element()
    }

    /// The instructions as one row that unfolds into their lines: what the server says to agents
    /// about itself, third-party text.
    fn instructions_row(&self, diff: &Diff, review: &Review, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let now = diff.instructions_now.as_deref();
        let changed = !diff.first && diff.instructions_pinned.as_deref() != now;
        let lines = match (&diff.instructions_pinned, &diff.instructions_now) {
            (None, None) => vec![Line::Same("It gives no instructions.".into())],
            (old, new) if !diff.first => line_diff(
                old.as_deref().unwrap_or_default(),
                new.as_deref().unwrap_or_default(),
            ),
            _ => now
                .unwrap_or_default()
                .lines()
                .map(|l| Line::Same(l.to_owned()))
                .collect(),
        };
        let about = match changed {
            true => {
                let n = lines.iter().filter(|l| !matches!(l, Line::Same(_))).count();
                format!("{n} lines changed")
            }
            false => match now {
                Some(text) => text.lines().next().unwrap_or_default().to_owned(),
                None => "It gives no instructions.".to_owned(),
            },
        };
        let this = cx.entity().downgrade();
        Unfolding::new("connection-review-instructions", review.instructions_open)
            .line(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(
                        div()
                            .id("connection-review-instructions-title")
                            .aria_label("Instructions")
                            .flex_none()
                            .medium()
                            .text_color(p.text)
                            .child("Instructions")
                            .test_support(),
                    )
                    .children(
                        changed
                            .then(|| Tag::new("connection-review-instructions-change", "Changed")),
                    )
                    .child(
                        div()
                            .id("connection-review-instructions-about")
                            .aria_label(SharedString::from(about.clone()))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(p.mute)
                            .child(SharedString::from(about))
                            .test_support(),
                    ),
            )
            .body(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.faint)
                            .child("Third-party text, given to an agent only when it asks for it."),
                    )
                    .child(DiffLines::new(
                        "connection-review-instructions-text",
                        lines.iter().map(Line::diff_line),
                    )),
            )
            .on_toggle(move |open, _, cx| {
                this.update(cx, |this, cx| {
                    if let Some(review) = this.review.as_mut() {
                        review.instructions_open = open;
                    }
                    cx.notify();
                })
                .ok();
            })
            .into_any_element()
    }

    /// What the review lists: the instructions, the rows (of "What changed" and of the groups, or
    /// of the groups alone in a first review), the limits and the names refused.
    fn review_body(
        &self,
        diff: &Diff,
        review: &Review,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let heading = |id: &str, title: &str| {
            div()
                .id(SharedString::from(id.to_owned()))
                .text_size(px(13.))
                .medium()
                .text_color(p.text)
                .child(SharedString::from(title.to_owned()))
        };

        // "What changed": the rows that are new, changed or gone, flat, before anything else.
        let changes = (!diff.first).then(|| {
            let items: Vec<AnyElement> = review
                .changes
                .rows()
                .map(|row| {
                    let (name, at) = (row.key.to_owned(), *row.value);
                    let part = move |part: &str| -> ElementId {
                        ElementId::Name(format!("connection-review-change-{name}-{part}").into())
                    };
                    let content = self.action_row(review, diff, at, &part, window, cx);
                    Item::new(
                        ElementId::Name(format!("connection-review-change-{}", row.key).into()),
                        row.placement,
                        content,
                    )
                    .into_any_element()
                })
                .collect();
            let this = cx.entity().downgrade();
            let since = diff
                .pinned
                .as_deref()
                .map(|pinned| format!("since {}", pinned.rsplit(' ').next().unwrap_or(pinned)));
            let mut group = Group::new(
                "connection-review-changes",
                "What changed",
                review.changes.len(),
            )
            .open(!review.folded.contains("changes"))
            .on_toggle(move |open, _, cx| {
                this.update(cx, |this, cx| this.fold_group("changes", !open, cx))
                    .ok();
            })
            .children(items);
            if let Some(since) = since {
                group = group.sentence(since);
            }
            group.gap(space::SM)
        });

        // The groups, one per setting: all the rows of a first review, the unchanged ones of a
        // re-review under their line.
        let mut groups = Vec::new();
        for (rows, (key, title, sentence)) in review.rows.groups().zip(GROUPS) {
            let items: Vec<AnyElement> = rows
                .rows()
                .map(|row| {
                    let part = |name: &str| row.part(name);
                    let content = self.action_row(review, diff, *row.value, &part, window, cx);
                    Item::new(row.id(), row.placement, content).into_any_element()
                })
                .collect();
            let this = cx.entity().downgrade();
            groups.push(
                Group::new(format!("connection-review-group-{key}"), title, rows.len())
                    .sentence(sentence)
                    .open(!review.folded.contains(key))
                    .on_toggle(move |open, _, cx| {
                        this.update(cx, |this, cx| this.fold_group(key, !open, cx))
                            .ok();
                    })
                    .children(items)
                    .gap(space::SM),
            );
        }
        let in_groups: usize = review.rows.groups().map(|g| g.len()).sum();
        let unchanged = diff
            .actions
            .iter()
            .filter(|a| a.change == "unchanged")
            .collect::<Vec<_>>();
        // A first review lists its groups as they are; a re-review holds them under one line.
        let listed = match diff.first || unchanged.is_empty() {
            true => div()
                .flex()
                .flex_col()
                .children(groups.into_iter().map(IntoElement::into_any_element))
                .into_any_element(),
            false => {
                let this = cx.entity().downgrade();
                let finding = (!query.is_empty() || review.filter != "all") && in_groups > 0;
                Summary::new(
                    "connection-review-unchanged",
                    format!("{} unchanged", unchanged.len()),
                    review.unchanged_open,
                )
                .detail(Self::tally(review, &unchanged))
                .unfolded_by(finding)
                .on_toggle(move |open, _, cx| {
                    this.update(cx, |this, cx| {
                        if let Some(review) = this.review.as_mut() {
                            review.unchanged_open = open;
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .children(groups.into_iter().map(IntoElement::into_any_element))
                .into_any_element()
            }
        };
        // What no row shows says so, with what was asked for, folding as the rows come back.
        let nothing =
            (in_groups == 0 && review.changes.is_empty()).then(|| match query.is_empty() {
                true => "No action here.".to_owned(),
                false => format!("No action matches \u{201c}{query}\u{201d}."),
            });
        let none = presence_of(
            "connection-review-none-block",
            nothing.map(SharedString::from),
            move |said| {
                div()
                    .id("connection-review-none")
                    .aria_label(said.clone())
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(said)
                    .test_support()
                    .into_any_element()
            },
        )
        .gap(space::MD);

        let limits = diff.limits.iter().map(|limit| {
            let LimitCover {
                argument,
                values,
                covers,
                not,
            } = limit;
            let mut said = format!("{argument} may only be {}", values.join(", "));
            if !covers.is_empty() {
                said.push_str(&format!(", in {}", covers.join(", ")));
            }
            if !not.is_empty() {
                said.push_str(&format!("; not limited in {}", not.join(", ")));
            }
            div()
                .id(SharedString::from(format!(
                    "connection-review-limit-{argument}"
                )))
                .aria_label(SharedString::from(said.clone()))
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(SharedString::from(said))
                .test_support()
        });
        let refused = (!diff.refused.is_empty()).then(|| {
            let said = format!(
                "Never offered, their names are not allowed: {}.",
                diff.refused.join(", ")
            );
            div()
                .id("connection-review-refused")
                .aria_label(SharedString::from(said.clone()))
                .text_size(px(12.5))
                .text_color(p.wait)
                .child(SharedString::from(said))
                .test_support()
        });
        div()
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(self.instructions_row(diff, review, cx))
            .children(changes)
            .child(listed)
            .child(none)
            .children((!diff.limits.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .child(heading("connection-review-limits-title", "Its limits"))
                    .children(limits)
            }))
            .children(refused)
            .into_any_element()
    }

    /// What the actions are set to, said in words: `1 allowed · 1 asked · 7 judged`.
    fn tally(review: &Review, actions: &[&ActionDiff]) -> String {
        [
            ("passes", "allowed"),
            ("asked", "asked"),
            ("off", "off"),
            ("judged", "judged"),
        ]
        .iter()
        .filter_map(|(key, word)| {
            let n = actions
                .iter()
                .filter(|a| a.change != "removed" && review.setting_of(&a.name) == *key)
                .count();
            (n > 0).then(|| format!("{n} {word}"))
        })
        .collect::<Vec<_>>()
        .join(" · ")
    }

    /// The chips that filter the review: every action, what changed (a re-review), then each
    /// setting. Each counts what it would show under what is searched.
    fn review_chips(&self, diff: &Diff, review: &Review, cx: &mut Context<Self>) -> FilterChips {
        let count = |key: &str| {
            diff.actions
                .iter()
                .enumerate()
                .filter(|(at, a)| review.found(*at) && review.counts_for(a, key))
                .count()
        };
        let mut keys: Vec<(&str, &str)> = vec![("all", "All")];
        if !diff.first {
            keys.extend([
                ("new", "New"),
                ("changed", "Changed"),
                ("removed", "Removed"),
            ]);
        }
        keys.extend([
            ("passes", "Allowed"),
            ("judged", "Judged"),
            ("asked", "Asked"),
            ("off", "Off"),
        ]);
        let chosen = keys
            .iter()
            .position(|(key, _)| *key == review.filter)
            .unwrap_or(0);
        let order: Vec<String> = keys.iter().map(|(key, _)| (*key).to_owned()).collect();
        let this = cx.entity().downgrade();
        FilterChips::new(
            "connection-review-filter",
            "Filter the actions",
            keys.iter()
                .map(|(key, title)| FilterChip::new(*key, *title, count(key))),
            chosen,
        )
        .on_choose(move |index, _, cx| {
            this.update(cx, |this, cx| {
                if let Some(review) = this.review.as_mut() {
                    review.filter = order[index].clone();
                }
                cx.notify();
            })
            .ok();
        })
    }

    /// What the review leaves to the judge and where it does not, over every action it holds
    /// (not the ones the search shows): a sentence and a thin bar split by setting. The numbers
    /// change as the text does, the split over a panel's time.
    fn review_split(
        diff: &Diff,
        review: &Review,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let held: Vec<&ActionDiff> = diff
            .actions
            .iter()
            .filter(|a| a.change != "removed")
            .collect();
        let count = |key: &str| {
            held.iter()
                .filter(|a| review.setting_of(&a.name) == key)
                .count()
        };
        let (judged, exceptions) = (
            count("judged"),
            [("passes", "allowed"), ("asked", "asked"), ("off", "off")]
                .map(|(key, word)| (key, count(key), word)),
        );
        let said: Vec<String> = exceptions
            .iter()
            .filter(|(_, n, _)| *n > 0)
            .map(|(_, n, word)| format!("{n} {word}"))
            .collect();
        let sentence = match said.is_empty() {
            true => format!("All {judged} left to the judge"),
            false => format!(
                "{judged} left to the judge \u{b7} your exceptions: {}",
                said.join(", ")
            ),
        };
        let total = held.len().max(1) as f32;
        let segments = [
            ("judged", judged, p.line2),
            ("passes", exceptions[0].1, p.ok),
            ("asked", exceptions[1].1, p.text),
            ("off", exceptions[2].1, p.faint),
        ]
        .map(|(key, n, color)| {
            let id = ElementId::Name(format!("connection-review-bar-{key}").into());
            let share = eased(id.clone(), n as f32 / total, window, cx);
            div()
                .id(id)
                .h_full()
                .w(relative(share))
                .bg(color)
                .test_support()
        });
        div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .child(
                div()
                    .id("connection-review-left")
                    .aria_label(SharedString::from(sentence.clone()))
                    .text_size(px(12.5))
                    .child(changing(
                        "connection-review-left-words",
                        sentence,
                        move |said| {
                            let (lead, rest) = said
                                .split_once(" \u{b7} ")
                                .map_or((said.as_ref(), ""), |(a, b)| (a, b));
                            div()
                                .flex()
                                .gap(space::XS)
                                .text_color(p.mute)
                                .child(
                                    div()
                                        .medium()
                                        .text_color(p.text)
                                        .child(SharedString::from(lead.to_owned())),
                                )
                                .children(
                                    (!rest.is_empty())
                                        .then(|| SharedString::from(format!("\u{b7} {rest}"))),
                                )
                                .into_any_element()
                        },
                    ))
                    .test_support(),
            )
            .child(
                div()
                    .id("connection-review-bar")
                    .w_full()
                    .h(px(7.))
                    .flex()
                    .rounded(px(7.))
                    .overflow_hidden()
                    .bg(p.line)
                    .children(segments)
                    .test_support(),
            )
            .into_any_element()
    }

    /// What Flottille's adapter is to this server, in sight above the bar: that it applies (its
    /// name and range), and why `login` is refused while an action outside its list is on, which
    /// the user reads before Accept. What it changed unfolds under it.
    fn adapter_block(
        &self,
        adapter: &AdapterSaid,
        review: &Review,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let said = div()
            .id("connection-review-adapter")
            .aria_label(SharedString::from(adapter.said.clone()))
            .text_size(px(13.))
            .text_color(p.text)
            .child(SharedString::from(adapter.said.clone()))
            .test_support();
        let changed = Unfolding::new("connection-review-adapter-changes", review.adapter_open)
            .line(
                div()
                    .id("connection-review-adapter-changes-title")
                    .aria_label("What it changed")
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child("What it changed")
                    .test_support(),
            )
            .body(div().flex().flex_col().gap(space::XS).children(
                adapter.changed.iter().enumerate().map(|(at, line)| {
                    let line = format!("\u{b7} {line}.");
                    div()
                        .id(SharedString::from(format!(
                            "connection-review-adapter-change-{at}"
                        )))
                        .aria_label(SharedString::from(line.clone()))
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(SharedString::from(line))
                        .test_support()
                }),
            ))
            .on_toggle(move |open, _, cx| {
                this.update(cx, |this, cx| {
                    if let Some(review) = this.review.as_mut() {
                        review.adapter_open = open;
                    }
                    cx.notify();
                })
                .ok();
            });
        let changed = (!adapter.changed.is_empty()).then_some(changed);
        div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .child(said)
            .children(changed)
            .into_any_element()
    }

    /// The bar of the rows ticked, and what the review is set to, which stay in sight.
    fn review_kept(&self, diff: &Diff, review: &Review, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let mut bar =
            BulkBar::new("connection-review-bulk", review.picked_shown(diff).len()).gap(space::SM);
        for (key, title, ..) in SETTINGS {
            let this = cx.entity().downgrade();
            bar = bar.action(
                Button::secondary(format!("connection-review-bulk-{key}"), title).on_click(
                    move |_, _, cx| {
                        this.update(cx, |this, cx| this.set_picked(key, cx)).ok();
                    },
                ),
            );
        }
        let this = cx.entity().downgrade();
        let bar = bar.on_clear(move |_, cx| {
            this.update(cx, |this, cx| {
                if let Some(review) = this.review.as_mut() {
                    review.picked.clear();
                }
                cx.notify();
            })
            .ok();
        });
        let all: Vec<&ActionDiff> = diff.actions.iter().collect();
        let tally = Self::tally(review, &all);
        div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(bar)
            .child(
                div()
                    .id("connection-review-tally")
                    .aria_label(SharedString::from(tally.clone()))
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(tally))
                    .test_support(),
            )
            .into_any_element()
    }

    /// Sets the action `from`, and with it every row ticked when it is one of them.
    fn set_actions(&mut self, from: &str, setting: &str, cx: &mut Context<Self>) {
        let Some(review) = self.review.as_mut() else {
            return;
        };
        let shown = review.shown_picks();
        let names = match shown.iter().any(|name| name == from) {
            true => shown,
            false => vec![from.to_owned()],
        };
        for name in names {
            review.picked.remove(&name);
            review.settings.insert(name, setting.to_owned());
        }
        cx.notify();
    }

    /// Sets every row ticked, and unticks them.
    fn set_picked(&mut self, setting: &str, cx: &mut Context<Self>) {
        let Some(review) = self.review.as_mut() else {
            return;
        };
        for name in review.shown_picks() {
            review.picked.remove(&name);
            review.settings.insert(name, setting.to_owned());
        }
        cx.notify();
    }

    /// Folds a group of rows, or unfolds it.
    fn fold_group(&mut self, group: &'static str, folded: bool, cx: &mut Context<Self>) {
        if let Some(review) = self.review.as_mut() {
            match folded {
                true => review.folded.insert(group),
                false => review.folded.remove(group),
            };
            cx.notify();
        }
    }

    fn review_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Modal {
        let this = cx.entity().downgrade();
        let title = self
            .review
            .as_ref()
            .map_or_else(String::new, |r| format!("Review {}", r.name));
        let modal = Modal::new("connection-review", title)
            .wide()
            .takes_focus()
            .open(self.review_open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.review_open = false;
                    cx.notify();
                })
                .ok();
            });
        let Some(mut review) = self.review.take() else {
            return modal;
        };
        let p = palette(cx);
        if review.search.is_none() {
            review.search = Some(line_field("Search the actions", window, cx));
        }
        let search = review.search.clone().expect("made above");
        let query = search.read(cx).value().to_string();
        // The answer is out of the review while the frame is drawn, so that it is borrowed and
        // not copied on every frame.
        let answer = review.answer.take();
        let (sentence, content, under, kept, ready) = match &answer {
            Some(Ok(diff)) => {
                review.found.of(diff, &query);
                let now = cx.background_executor().now();
                review
                    .folding
                    .retain(|_, since| now.saturating_duration_since(*since) < PANEL + HOVER);
                Self::sync_review(&mut review, diff, now, cx.reduce_motion());
                let server = match &diff.pinned {
                    Some(pinned) if *pinned != diff.now => {
                        let version = diff.now.rsplit(' ').next().unwrap_or_default();
                        format!("{pinned} → {version}")
                    }
                    _ => diff.now.clone(),
                };
                let total = diff
                    .actions
                    .iter()
                    .filter(|a| a.change != "removed")
                    .count();
                // The sections come one after the other, the adapter's among them.
                let sections: Vec<(&str, AnyElement)> = [
                    Some((
                        "server",
                        div()
                            .id("connection-review-server")
                            .aria_label(SharedString::from(server.clone()))
                            .mono(13.)
                            .text_color(p.text)
                            .child(SharedString::from(server))
                            .test_support()
                            .into_any_element(),
                    )),
                    diff.adapter
                        .as_ref()
                        .map(|adapter| ("adapter", self.adapter_block(adapter, &review, cx))),
                    Some(("split", Self::review_split(diff, &review, window, cx))),
                    Some((
                        "search",
                        SearchField::new("connection-review-search", &search, "Search the actions")
                            .placeholder(format!("Search {total} actions by name or description"))
                            .into_any_element(),
                    )),
                    Some((
                        "chips",
                        self.review_chips(diff, &review, cx).into_any_element(),
                    )),
                ]
                .into_iter()
                .flatten()
                .collect();
                let under = div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .children(
                        sections
                            .into_iter()
                            .enumerate()
                            .map(|(at, (key, section))| {
                                enter(format!("connection-review-section-{key}"), at, section)
                            }),
                    )
                    .into_any_element();
                (
                    Some(diff.sentence()),
                    self.review_body(diff, &review, &query, window, cx),
                    Some(under),
                    Some(self.review_kept(diff, &review, cx)),
                    true,
                )
            }
            Some(Err(why)) => (
                Some("Its server did not start, so there is nothing to read yet.".to_owned()),
                div()
                    .id("connection-review-error")
                    .aria_label(SharedString::from(why.clone()))
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(SharedString::from(why.clone()))
                    .test_support()
                    .into_any_element(),
                None,
                None,
                false,
            ),
            None => (None, div().into_any_element(), None, None, false),
        };
        let pending = answer.is_none();
        let error = problem_line(
            "connection-accept-error",
            review.error.clone(),
            space::MD,
            cx,
        );
        let accepting = review.accepting;
        review.answer = answer;
        self.review = Some(review);
        let body = div()
            .id("connection-review-body")
            .w_full()
            .flex()
            .flex_col()
            .child(awaiting(
                "connection-review-waiting",
                pending,
                div()
                    .flex()
                    .flex_col()
                    .gap(space::MD)
                    .child(Skeleton::new("connection-review-skeleton-0").w(px(220.)))
                    .child(Skeleton::new("connection-review-skeleton-1").h(px(80.)))
                    .child(Skeleton::new("connection-review-skeleton-2").h(px(120.))),
                content,
            ))
            .child(error);
        let mut modal = modal.under_title(under).child(body);
        if let Some(kept) = kept {
            modal = modal.kept(kept);
        }
        if let Some(sentence) = sentence {
            modal = modal.sentence(sentence);
        }
        if ready {
            modal = modal.action(
                Button::primary("accept-connection", "Accept")
                    .disabled(accepting)
                    .on_click(cx.listener(|this, _, _, cx| this.accept(cx))),
            );
        }
        modal
    }
}

impl Render for ConnectionsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|s| (s.name.clone(), s)), now, reduce);
        }
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("connection-list");
        for (key, placement, server) in drawn {
            let row = self.row(&server, cx);
            list = list.item(
                ElementId::Name(format!("connection-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(window, cx);
        let review = self.review_modal(window, cx);
        let section = Section::new("connection-servers", "Servers")
            .count(count.to_string())
            .sentence(SENTENCE)
            .action(
                Button::secondary("new-connection", "New server")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
            )
            .child(presence("connection-list-block", count > 0, list).gap(space::MD))
            .child(
                presence(
                    "no-connection-block",
                    empty,
                    EmptyLine::new("no-connection", Icon::Lucide("plug"), EMPTY),
                )
                .gap(space::MD),
            )
            .child(problem_line(
                "connection-problem",
                self.problem.clone(),
                space::MD,
                cx,
            ));
        div()
            .text_color(p.text)
            .child(section)
            .child(modal)
            .child(review)
            .child(self.confirm(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_line_is_read_removed_then_added() {
        assert_eq!(
            line_diff(
                "Search first.\nKeep notes short.",
                "Search first.\nWrite all."
            ),
            vec![
                Line::Same("Search first.".into()),
                Line::Removed("Keep notes short.".into()),
                Line::Added("Write all.".into()),
            ]
        );
        assert_eq!(
            line_diff("", "a"),
            vec![Line::Added("a".into())],
            "instructions given"
        );
        assert_eq!(line_diff("a", ""), vec![Line::Removed("a".into())]);
    }

    #[test]
    fn a_command_is_one_line_its_words_quoted_only_when_they_must_be() {
        let words = |w: &[&str]| w.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(command_line(&words(&["almanac", "mcp"])), "almanac mcp");
        assert_eq!(
            command_line(&words(&["notes", "--dir", "My notes"])),
            "notes --dir 'My notes'"
        );
        let line = command_line(&words(&["a b", "--x=1", "it's"]));
        assert_eq!(
            flottille_core::command::split_line(&line).unwrap(),
            words(&["a b", "--x=1", "it's"])
        );
    }

    #[test]
    fn a_row_says_its_state_once() {
        let server = |state: &str, more: Value| {
            let mut value = json!({"name": "almanac", "about": "a", "command": ["a"],
                "state": state, "actions": [{"name": "a", "setting": "judged"},
                    {"name": "b", "setting": "off"}]});
            for (k, v) in more.as_object().unwrap() {
                value[k] = v.clone();
            }
            Server::of(&value).line()
        };
        assert_eq!(
            server(
                "ready",
                json!({"server": {"name": "almanac", "version": "1"}})
            ),
            "On · 1 action"
        );
        assert_eq!(server("off", json!({})), "Off");
        assert_eq!(
            server("awaiting review", json!({})),
            "Waiting for its first review"
        );
        assert_eq!(
            server(
                "cut off",
                json!({"cut_off": "the server is now almanac 2, not almanac 1"})
            ),
            "Stopped: new version, review it"
        );
        assert_eq!(
            server("cut off", json!({"cut_off": "its instructions changed"})),
            "Stopped: its instructions changed, review it"
        );
        assert_eq!(
            server("ready", json!({"failing": "no such program"})),
            "Failing: no such program"
        );
        assert_eq!(
            server("signed out", json!({"signed_out": "invalid_grant"})),
            "Signed out: sign in again"
        );
    }
}
