//! One agent's conversation as the session page shows it: its events, as the daemon stored and
//! masked them, folded into blocks (a message to it, its reasoning, its text, its tool calls, the
//! sub-agents it launched, a report, a diff, a request that reached the user). Nothing here talks
//! to the socket or draws: the page turns each change into one row drawn again or one row added.

use std::collections::{HashMap, HashSet, VecDeque};

use flottille_core::protocol::Event;
use serde_json::Value;

/// Who sent the user's messages: a role, never drawn (the bubble says it).
pub const USER: &str = "you";

/// What one row of the conversation shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// A message the agent was sent: the user's (`from` is [`USER`]), or its parent's
    /// (`the conversation → web-form`), its brief first.
    Message {
        from: String,
        text: String,
    },
    /// Its reasoning: running while `until` is `None`.
    Thought {
        text: String,
        since: u64,
        until: Option<u64>,
    },
    /// What it said.
    Text {
        text: String,
    },
    /// The images that went with the message before it: names the daemon gives back as pictures.
    Images {
        names: Vec<String>,
    },
    /// Its tool calls, one after the other.
    Steps {
        calls: Vec<Call>,
    },
    /// The children it launched: the rail's tree says which, and what each does.
    SubAgents,
    Report {
        from: String,
        text: String,
    },
    /// A delivery that is none of the above (a child's question, a reminder), or the agent's end.
    Notice {
        kind: String,
        text: String,
    },
    /// The edits of a file it changed.
    Diff {
        path: String,
        lines: Vec<DiffLine>,
    },
    /// A request that reached the user.
    Approval {
        number: u64,
    },
    /// A command's result.
    Command(CommandCard),
}

/// How a command ended, as its result says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Running,
    Exited(i32),
    Killed(i32),
    Stopped,
    Interrupted,
}

/// A command's result as a card: what the daemon answered the call with, read back.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandCard {
    pub run: String,
    pub line: String,
    pub ending: Ending,
    pub seconds: Option<f64>,
    /// The lines of its output the answer held, as the program wrote them (masked): colours
    /// included, for the card to draw or strip.
    pub lines: Vec<String>,
    /// Who allowed the call when it came back through an approval.
    pub allowed_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Removed(String),
    Added(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallState {
    Running,
    Done,
    Failed,
    /// Still running when the turn or the agent ended: it will not end.
    Interrupted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    /// The tool's name, without Flottille's prefix.
    pub name: String,
    /// Its arguments on one line.
    pub args: String,
    pub state: CallState,
    pub since: u64,
    pub until: Option<u64>,
}

impl Call {
    /// How long it took, once it ended.
    pub fn duration_ms(&self) -> Option<u64> {
        self.until.map(|until| until.saturating_sub(self.since))
    }
}

/// What the agent does while a turn runs, as the line above the composer says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    Thinking,
    Writing,
    /// A tool call still running, by its name.
    Running(String),
}

impl Activity {
    pub fn line(&self) -> String {
        match self {
            Activity::Thinking => "Thinking…".into(),
            Activity::Writing => "Writing…".into(),
            Activity::Running(tool) => format!("Running {tool}…"),
        }
    }
}

/// What applying an event changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A row was added at the end.
    Added,
    /// The row at this index shows something else.
    Changed(usize),
    /// The text of the row at this index (a reply or a thought streaming) goes on with this.
    Appended(usize, String),
}

/// The prefix Claude Code gives the tools of Flottille's MCP server.
const PREFIX: &str = "mcp__flottille__";
/// Where a note pinned on a tool result begins.
const NOTE_START: &str = "<flottille-note";

pub struct Conversation {
    blocks: Vec<Block>,
    /// The number of the last event applied: an event read twice (history, then live) is applied
    /// once.
    last_seq: u64,
    /// The agent shown, as the page names it.
    me: String,
    /// Its parent's name; `None` for the root.
    parent: Option<String>,
    /// The names of the agents of the tree, by id, to say which child reported.
    names: HashMap<String, String>,
    /// A child was launched since the last row that was not a tool call: its list follows the
    /// calls.
    launched: bool,
    has_sub_agents: bool,
    /// The arguments of each `fs_edit` call still running, for its diff once it is done: its
    /// updates do not repeat them.
    edits: HashMap<String, Value>,
    /// The row of each tool call, by its id.
    calls: HashMap<String, usize>,
    /// The rows of steps with a call still running.
    running: HashSet<usize>,
    /// The requests that have a row.
    approvals: HashSet<u64>,
    /// Requests asked by another agent, each with when it was asked, the soonest first: each
    /// gets its row before the first event that came after it.
    pending_approvals: VecDeque<(u64, u64)>,
}

impl Conversation {
    pub fn new(me: String, parent: Option<String>) -> Self {
        Self {
            blocks: Vec::new(),
            last_seq: 0,
            me,
            parent,
            names: HashMap::new(),
            launched: false,
            has_sub_agents: false,
            edits: HashMap::new(),
            calls: HashMap::new(),
            running: HashSet::new(),
            approvals: HashSet::new(),
            pending_approvals: VecDeque::new(),
        }
    }

    /// The same agent's conversation, empty, to be read again from its start.
    pub fn restarted(&self) -> Self {
        let mut empty = Self::new(self.me.clone(), self.parent.clone());
        empty.names = self.names.clone();
        empty
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The agent shown, as the page names it.
    pub fn me(&self) -> &str {
        &self.me
    }

    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// What the agent does now, from the last thing it did: a tool call that runs, text it
    /// writes, else it thinks (a turn just started, a call just done).
    pub fn activity(&self) -> Activity {
        for block in self.blocks.iter().rev() {
            match block {
                Block::Steps { calls } => {
                    return match calls.iter().rev().find(|c| c.state == CallState::Running) {
                        Some(call) => Activity::Running(call.name.clone()),
                        None => Activity::Thinking,
                    };
                }
                Block::Text { .. } => return Activity::Writing,
                Block::Thought { .. } | Block::Message { .. } => return Activity::Thinking,
                _ => {}
            }
        }
        Activity::Thinking
    }

    pub fn set_names(&mut self, names: HashMap<String, String>) {
        self.names = names;
    }

    /// Adds the card of a request that reached the user, once.
    pub fn add_approval(&mut self, number: u64) -> Vec<Change> {
        if self.approvals.contains(&number) {
            return Vec::new();
        }
        self.approvals.insert(number);
        let mut changes = self.close(None);
        changes.extend(self.push(Block::Approval { number }));
        changes
    }

    pub fn has_approval(&self, number: u64) -> bool {
        self.approvals.contains(&number)
    }

    /// Requests that reached the user, each `(when it was asked, its number)`, to be placed
    /// among the events read next: the events of another agent's request are not this one's.
    pub fn place_approvals(&mut self, mut requests: Vec<(u64, u64)>) {
        requests.sort();
        self.pending_approvals = requests.into();
    }

    /// The list of sub-agents, when a child was launched and none is shown yet.
    pub fn add_sub_agents(&mut self) -> Vec<Change> {
        if self.has_sub_agents {
            return Vec::new();
        }
        self.launched = true;
        self.close(None)
    }

    /// The list of sub-agents first, before any row is drawn: launched before the events read.
    pub fn prepend_sub_agents(&mut self) {
        if self.has_sub_agents {
            return;
        }
        self.has_sub_agents = true;
        self.launched = false;
        self.blocks.insert(0, Block::SubAgents);
        let shift = |at: &mut usize| *at += 1;
        self.calls.values_mut().for_each(shift);
        self.running = self.running.iter().map(|at| at + 1).collect();
    }

    /// Folds one event of the agent in. Events numbered at or before the last one applied are
    /// left out.
    pub fn apply(&mut self, event: &Event) -> Vec<Change> {
        if event.seq <= self.last_seq {
            return Vec::new();
        }
        self.last_seq = event.seq;
        let mut changes = Vec::new();
        while let Some(&(at, number)) = self.pending_approvals.front()
            && at <= event.at
        {
            self.pending_approvals.pop_front();
            changes.extend(self.add_approval(number));
        }
        changes.extend(self.fold(event));
        changes
    }

    fn fold(&mut self, event: &Event) -> Vec<Change> {
        let data = &event.data;
        let text = |field: &str| data[field].as_str().unwrap_or_default().to_owned();
        let last = self.blocks.len().saturating_sub(1);
        match event.name.as_str() {
            "agent.thought" => match self.blocks.last_mut() {
                Some(Block::Thought {
                    text: thought,
                    until: None,
                    ..
                }) => {
                    let piece = text("text");
                    thought.push_str(&piece);
                    vec![Change::Appended(last, piece)]
                }
                _ => {
                    let mut changes = self.close(Some(event.at));
                    changes.extend(self.push(Block::Thought {
                        text: text("text"),
                        since: event.at,
                        until: None,
                    }));
                    changes
                }
            },
            "agent.message" => {
                let mut changes = self.end_thought(event.at);
                match self.blocks.last_mut() {
                    Some(Block::Text { text: said }) if changes.is_empty() => {
                        let piece = text("text");
                        said.push_str(&piece);
                        changes.push(Change::Appended(last, piece));
                    }
                    _ => {
                        changes.extend(self.close(Some(event.at)));
                        changes.extend(self.push(Block::Text { text: text("text") }));
                    }
                }
                changes
            }
            "agent.tool_call" if data["update"] != true => {
                let mut changes = self.end_thought(event.at);
                let name = text("title");
                let name = name.strip_prefix(PREFIX).unwrap_or(&name).to_owned();
                if name == "agent_launch" {
                    self.launched = !self.has_sub_agents;
                }
                if name == "fs_edit" && !data["raw_input"].is_null() {
                    self.edits.insert(text("id"), data["raw_input"].clone());
                }
                let call = Call {
                    id: text("id"),
                    name,
                    args: shown_args(&data["raw_input"]),
                    state: call_state(data["status"].as_str()),
                    since: event.at,
                    until: None,
                };
                let call = Call {
                    until: (call.state != CallState::Running).then_some(event.at),
                    ..call
                };
                let running = call.state == CallState::Running;
                let id = call.id.clone();
                match self.blocks.last_mut() {
                    Some(Block::Steps { calls }) => {
                        calls.push(call);
                        changes.push(Change::Changed(self.blocks.len() - 1));
                    }
                    _ => changes.extend(self.push(Block::Steps { calls: vec![call] })),
                }
                let at = self.blocks.len() - 1;
                self.calls.insert(id, at);
                if running {
                    self.running.insert(at);
                }
                changes
            }
            "agent.tool_call" => self.update_call(event),
            "agent.prompted" => {
                let mut changes = self.close(Some(event.at));
                let names: Vec<String> = data["images"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect();
                for (kind, body) in sections(&text("text")) {
                    // A picture sent alone has no words: its thumbnails are the message.
                    if body.trim().is_empty() && !names.is_empty() {
                        continue;
                    }
                    let block = self.delivered(&kind, body);
                    changes.extend(self.push(block));
                }
                if !names.is_empty() {
                    changes.extend(self.push(Block::Images { names }));
                }
                changes
            }
            "agent.reported" => {
                let mut changes = self.close(Some(event.at));
                changes.extend(self.push(Block::Report {
                    from: self.me.clone(),
                    text: text("text"),
                }));
                changes
            }
            "approval.requested" if data["held_by"] == "user" => {
                let number = data["number"].as_u64().unwrap_or(0);
                self.add_approval(number)
            }
            "agent.turn_ended" | "agent.died" => {
                let mut changes = self.close(Some(event.at));
                changes.extend(self.interrupt(event.at));
                changes
            }
            "agent.failed" | "agent.ended" => {
                let mut changes = self.close(Some(event.at));
                changes.extend(self.interrupt(event.at));
                let kind = event.name.trim_start_matches("agent.").to_owned();
                changes.extend(self.push(Block::Notice {
                    kind,
                    text: text("reason"),
                }));
                changes
            }
            "agent.resumed" => self.push(Block::Notice {
                kind: "resumed".into(),
                text: "by you, on its own session".into(),
            }),
            _ => Vec::new(),
        }
    }

    fn push(&mut self, block: Block) -> Vec<Change> {
        self.blocks.push(block);
        vec![Change::Added]
    }

    /// A row that is not a tool call comes: the reasoning that ran has ended, and the list of
    /// sub-agents launched by the calls before comes first.
    fn close(&mut self, at: Option<u64>) -> Vec<Change> {
        let mut changes = at.map(|at| self.end_thought(at)).unwrap_or_default();
        if self.launched && !self.has_sub_agents {
            self.launched = false;
            self.has_sub_agents = true;
            changes.extend(self.push(Block::SubAgents));
        }
        changes
    }

    fn end_thought(&mut self, at: u64) -> Vec<Change> {
        let last = self.blocks.len().saturating_sub(1);
        match self.blocks.last_mut() {
            Some(Block::Thought { until, .. }) if until.is_none() => {
                *until = Some(at);
                vec![Change::Changed(last)]
            }
            _ => Vec::new(),
        }
    }

    /// The turn or the agent ended: the calls still running will not end.
    fn interrupt(&mut self, at: u64) -> Vec<Change> {
        let mut rows: Vec<usize> = self.running.drain().collect();
        rows.sort();
        for row in &rows {
            if let Some(Block::Steps { calls }) = self.blocks.get_mut(*row) {
                for call in calls.iter_mut().filter(|c| c.state == CallState::Running) {
                    call.state = CallState::Interrupted;
                    call.until = Some(at);
                }
            }
        }
        rows.into_iter().map(Change::Changed).collect()
    }

    fn update_call(&mut self, event: &Event) -> Vec<Change> {
        let data = &event.data;
        let id = data["id"].as_str().unwrap_or_default();
        let Some(&at) = self.calls.get(id) else {
            return Vec::new();
        };
        let Some(Block::Steps { calls }) = self.blocks.get_mut(at) else {
            return Vec::new();
        };
        let Some(call) = calls.iter_mut().find(|call| call.id == id) else {
            return Vec::new();
        };
        if let Some(title) = data["title"].as_str() {
            call.name = title.strip_prefix(PREFIX).unwrap_or(title).to_owned();
        }
        if !data["raw_input"].is_null() {
            call.args = shown_args(&data["raw_input"]);
            if call.name == "fs_edit" {
                self.edits
                    .insert(call.id.clone(), data["raw_input"].clone());
            }
        }
        if let Some(status) = data["status"].as_str()
            && call.state != CallState::Interrupted
        {
            call.state = call_state(Some(status));
            if call.state != CallState::Running && call.until.is_none() {
                call.until = Some(event.at);
            }
        }
        let call = call.clone();
        if !calls.iter().any(|c| c.state == CallState::Running) {
            self.running.remove(&at);
        }
        let mut changes = vec![Change::Changed(at)];
        let ended = call.state == CallState::Done && call.until == Some(event.at);
        let input = match call.state {
            CallState::Running => None,
            _ => self.edits.remove(&call.id),
        };
        let edit = input.filter(|_| ended).and_then(|input| diff_of(&input));
        if let Some(diff) = edit {
            changes.extend(self.push(diff));
        }
        if ended && matches!(call.name.as_str(), "commands_run" | "commands_status") {
            let mut texts = Vec::new();
            strings(&data["raw_output"], &mut texts);
            if let Some(card) = texts.into_iter().find_map(command_card) {
                changes.extend(self.push(Block::Command(card)));
            }
        }
        // A report delivered as a note on the result.
        for (kind, body) in notes(&data["raw_output"]) {
            if kind == "child-report" {
                let block = self.delivered(&kind, body);
                changes.extend(self.push(block));
            }
        }
        changes
    }

    /// The block of one delivery the agent was sent.
    fn delivered(&self, kind: &str, body: String) -> Block {
        if kind == "approval" {
            return approved(&body);
        }
        match (kind, &self.parent) {
            ("" | "user" | "brief", None) => Block::Message {
                from: USER.into(),
                text: body,
            },
            ("brief" | "parent", Some(parent)) => Block::Message {
                from: format!("{parent} → {}", self.me),
                text: body,
            },
            ("child-report", _) => Block::Report {
                from: self.child_named_in(&body),
                text: body,
            },
            _ => Block::Notice {
                kind: kind.to_owned(),
                text: body,
            },
        }
    }

    /// The child a delivery names by its id, as the tree names it.
    fn child_named_in(&self, text: &str) -> String {
        self.names
            .iter()
            .filter(|(id, _)| text.contains(id.as_str()))
            .map(|(_, name)| name.clone())
            .next()
            .unwrap_or_else(|| "a sub-agent".into())
    }
}

/// The card of a command's answer (`runs.rs`: `run <id>: <line> <how it is> after <s> s`, then
/// the advice while it runs, the range of lines, the lines); `None` for any other text.
pub fn command_card(text: &str) -> Option<CommandCard> {
    // A note pinned on the result is not part of it.
    let text = text.split(NOTE_START).next().unwrap_or(text).trim();
    let (run, rest) = text.strip_prefix("run ")?.split_once(": ")?;
    let (header, body) = rest.split_once('\n').unwrap_or((rest, ""));
    let open = header.find('`')?;
    let close = open + 1 + header[open + 1..].find('`')?;
    let line = &header[open + 1..close];
    let after = header[close + 1..].trim_start_matches(')').trim();
    let (how, seconds) = match after.rsplit_once(" after ") {
        Some((how, time)) => (
            how,
            time.strip_suffix(" s").and_then(|t| t.parse::<f64>().ok()),
        ),
        None => (after, None),
    };
    let ending = match how {
        "is running" => Ending::Running,
        "was stopped" => Ending::Stopped,
        how if how.starts_with("was interrupted") => Ending::Interrupted,
        how => match (
            how.strip_prefix("exited with code "),
            how.strip_prefix("was killed by signal "),
        ) {
            (Some(code), _) => Ending::Exited(code.parse().ok()?),
            (_, Some(signal)) => Ending::Killed(signal.parse().ok()?),
            _ => return None,
        },
    };
    let mut lines = body.lines().peekable();
    if lines
        .peek()
        .is_some_and(|l| l.starts_with("it keeps going:"))
    {
        lines.next();
    }
    if lines
        .peek()
        .is_some_and(|l| l.starts_with("(lines ") || *l == "(no output)")
    {
        lines.next();
    }
    Some(CommandCard {
        run: run.to_owned(),
        line: line.to_owned(),
        ending,
        seconds,
        lines: lines.map(str::to_owned).collect(),
        allowed_by: None,
    })
}

/// What the daemon stored for an agent when the user answered a request (`approvals.rs`), as the
/// conversation says it: a command's result is its card with who allowed it, any other answer a
/// sentence. The daemon's own words are the agent's, never shown.
fn approved(body: &str) -> Block {
    let notice = |text: String| Block::Notice {
        kind: "approval".into(),
        text,
    };
    let parsed = body
        .strip_prefix("Request #")
        .and_then(|rest| rest.split_once(" ("))
        .and_then(|(number, rest)| {
            let (tool, said) = rest.split_once(") ")?;
            Some((number, tool, said))
        });
    let Some((number, tool, said)) = parsed else {
        return notice(body.to_owned());
    };
    if let Some((_, result)) = said.split_once("has now done it. Its result:\n\n") {
        return match command_card(result) {
            Some(card) => Block::Command(CommandCard {
                allowed_by: Some(USER.into()),
                ..card
            }),
            None => notice(format!("you allowed {tool} (request #{number})")),
        };
    }
    notice(if said.starts_with("was refused by the user") {
        format!("you refused {tool} (request #{number})")
    } else if let Some(why) = said.strip_prefix("was not executed: ") {
        format!("{tool} (request #{number}) was not done: {why}")
    } else if let Some(why) = said.strip_prefix("was approved by the user, but nothing was done: ")
    {
        format!("you allowed {tool} (request #{number}) but nothing was done: {why}")
    } else {
        format!("{tool} (request #{number}) {said}")
    })
}

fn call_state(status: Option<&str>) -> CallState {
    match status {
        Some("completed") => CallState::Done,
        Some("failed") => CallState::Failed,
        _ => CallState::Running,
    }
}

/// A value on one line, as compact JSON; nothing for no value.
pub fn one_line(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

/// The longest a call's arguments are kept for its line: the line shows one line of them, and a
/// file written whole would otherwise stay in memory once per call.
const ARGS: usize = 300;

/// A call's arguments on one line, cut for its line.
fn shown_args(value: &Value) -> String {
    let line = one_line(value);
    match line.char_indices().nth(ARGS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

/// The diff of an `fs_edit` call's arguments: each edit's old lines, then its new ones.
fn diff_of(input: &Value) -> Option<Block> {
    let path = input["path"].as_str()?.to_owned();
    let mut lines = Vec::new();
    for edit in input["edits"].as_array()? {
        let old = edit["old"].as_str().unwrap_or_default();
        let new = edit["new"].as_str().unwrap_or_default();
        lines.extend(old.lines().map(|line| DiffLine::Removed(line.to_owned())));
        lines.extend(new.lines().map(|line| DiffLine::Added(line.to_owned())));
    }
    Some(Block::Diff { path, lines })
}

/// The deliveries of a prompt, each `(kind, body)`: the daemon starts each with a line
/// `[flottille:<kind>]`, which no data can forge (it escapes the marker in every body). Text
/// before the first marker is of no kind.
pub fn sections(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        match marker(line) {
            Some(kind) => found.push((kind.to_owned(), Vec::new())),
            None => match found.last_mut() {
                Some((_, lines)) => lines.push(line),
                None => found.push((String::new(), vec![line])),
            },
        }
    }
    found
        .into_iter()
        .map(|(kind, lines)| (kind, lines.join("\n").trim().to_owned()))
        .filter(|(kind, body)| !kind.is_empty() || !body.is_empty())
        .collect()
}

fn marker(line: &str) -> Option<&str> {
    let kind = line.strip_prefix("[flottille:")?.strip_suffix(']')?;
    (!kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase() || c == '-')).then_some(kind)
}

/// The deliveries pinned as notes on a tool's result: `<flottille-note id="…">`, the delivery,
/// `</flottille-note>`, in any text of the result.
fn notes(output: &Value) -> Vec<(String, String)> {
    let mut texts = Vec::new();
    strings(output, &mut texts);
    let mut found = Vec::new();
    for text in texts {
        let mut rest = text;
        while let Some(start) = rest.find("<flottille-note") {
            let after = &rest[start..];
            let Some(open_end) = after.find('>') else {
                break;
            };
            let inner = &after[open_end + 1..];
            let Some(end) = inner.find("</flottille-note>") else {
                break;
            };
            found.extend(sections(&inner[..end]));
            rest = &inner[end..];
        }
    }
    found
}

fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| strings(item, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(seq: u64, name: &str, data: Value) -> Event {
        Event {
            seq,
            at: seq * 100,
            name: name.into(),
            data,
        }
    }

    fn root() -> Conversation {
        Conversation::new("lead".into(), None)
    }

    #[test]
    fn a_reply_streamed_in_pieces_is_one_row() {
        let mut c = root();
        c.apply(&event(
            1,
            "agent.prompted",
            json!({"text": "[flottille:user]\nFix it"}),
        ));
        assert_eq!(
            c.apply(&event(2, "agent.message", json!({"text": "On "}))),
            [Change::Added]
        );
        assert_eq!(
            c.apply(&event(3, "agent.message", json!({"text": "it."}))),
            [Change::Appended(1, "it.".into())],
            "the piece alone, to append in place"
        );
        assert_eq!(
            c.blocks(),
            [
                Block::Message {
                    from: "you".into(),
                    text: "Fix it".into()
                },
                Block::Text {
                    text: "On it.".into()
                }
            ]
        );
        assert!(
            c.apply(&event(3, "agent.message", json!({"text": "again"})))
                .is_empty(),
            "an event read twice is applied once"
        );
    }

    #[test]
    fn a_thought_runs_until_something_else_comes() {
        let mut c = root();
        c.apply(&event(1, "agent.thought", json!({"text": "Hm"})));
        c.apply(&event(2, "agent.thought", json!({"text": "m."})));
        assert_eq!(
            c.apply(&event(5, "agent.message", json!({"text": "Done"}))),
            [Change::Changed(0), Change::Added]
        );
        assert_eq!(
            c.blocks()[0],
            Block::Thought {
                text: "Hmm.".into(),
                since: 100,
                until: Some(500)
            }
        );
    }

    #[test]
    fn tool_calls_are_steps_and_launches_list_the_sub_agents_after_them() {
        let mut c = root();
        for (seq, id) in [(1, "a"), (3, "b")] {
            c.apply(&event(
                seq,
                "agent.tool_call",
                json!({"id": id, "title": "mcp__flottille__agent_launch", "status": "in_progress",
                    "raw_input": {"title": id}}),
            ));
            c.apply(&event(
                seq + 1,
                "agent.tool_call",
                json!({"id": id, "update": true, "status": "completed"}),
            ));
        }
        c.apply(&event(9, "agent.message", json!({"text": "Two workers."})));
        let Block::Steps { calls } = &c.blocks()[0] else {
            panic!("{:?}", c.blocks());
        };
        assert_eq!(calls.len(), 2, "one card for calls in a row");
        assert_eq!(calls[0].name, "agent_launch");
        assert_eq!(calls[0].args, r#"{"title":"a"}"#);
        assert_eq!(calls[0].state, CallState::Done);
        assert_eq!(calls[0].duration_ms(), Some(100));
        assert_eq!(c.blocks()[1], Block::SubAgents);
        assert!(matches!(c.blocks()[2], Block::Text { .. }));
    }

    #[test]
    fn a_child_reads_its_brief_from_its_parent() {
        let mut c = Conversation::new("web-form".into(), Some("lead".into()));
        c.apply(&event(
            1,
            "agent.prompted",
            json!({"text": "[flottille:brief]\nFix the timer\n\n[flottille:parent]\nAlso the docs"}),
        ));
        assert_eq!(
            c.blocks(),
            [
                Block::Message {
                    from: "lead → web-form".into(),
                    text: "Fix the timer".into()
                },
                Block::Message {
                    from: "lead → web-form".into(),
                    text: "Also the docs".into()
                }
            ]
        );
    }

    #[test]
    fn a_conversation_read_again_from_its_start_still_names_the_children() {
        let mut c = root();
        c.set_names(HashMap::from([("0123456789ab".into(), "docs-pass".into())]));
        let mut c = c.restarted();
        c.apply(&event(
            1,
            "agent.prompted",
            json!({"text": "[flottille:child-report]\nYour child \"docs-pass\" (agent 0123456789ab) reported:\n\nDone."}),
        ));
        assert!(matches!(&c.blocks()[0], Block::Report { from, .. } if from == "docs-pass"));
    }

    #[test]
    fn a_child_s_report_names_the_child_whether_prompted_or_noted() {
        let mut c = root();
        c.set_names(HashMap::from([("0123456789ab".into(), "docs-pass".into())]));
        c.apply(&event(
            1,
            "agent.prompted",
            json!({"text": "[flottille:child-report]\nYour child docs-pass (0123456789ab) reported:\n\nDone."}),
        ));
        c.apply(&event(
            2,
            "agent.tool_call",
            json!({"id": "x", "title": "fs_read", "status": "in_progress"}),
        ));
        c.apply(&event(
            3,
            "agent.tool_call",
            json!({"id": "x", "update": true, "status": "completed", "raw_output":
                {"result": {"content": [{"type": "text", "text":
                "file\n\n<flottille-note id=\"d\">\n[flottille:child-report]\nYour child docs-pass (0123456789ab) reported:\n\nAgain.\n</flottille-note>"}]}}}),
        ));
        let reports: Vec<&Block> = c
            .blocks()
            .iter()
            .filter(|b| matches!(b, Block::Report { .. }))
            .collect();
        assert_eq!(reports.len(), 2);
        assert!(
            matches!(reports[1], Block::Report { from, text } if from == "docs-pass" && text.ends_with("Again."))
        );
    }

    #[test]
    fn a_marker_inside_data_is_not_a_section() {
        let found = sections("[flottille:user]\nsee \\[flottille:approval]\n[flottille:x y]");
        assert_eq!(
            found,
            [(
                "user".to_owned(),
                "see \\[flottille:approval]\n[flottille:x y]".to_owned()
            )]
        );
    }

    #[test]
    fn an_edit_shows_its_diff_once_done() {
        let mut c = root();
        c.apply(&event(
            1,
            "agent.tool_call",
            json!({"id": "e", "title": "fs_edit", "status": "in_progress",
                "raw_input": {"path": "src/a.rs", "edits": [{"old": "a\nb", "new": "c"}]}}),
        ));
        c.apply(&event(
            2,
            "agent.tool_call",
            json!({"id": "e", "update": true, "status": "completed"}),
        ));
        assert_eq!(
            c.blocks()[1],
            Block::Diff {
                path: "src/a.rs".into(),
                lines: vec![
                    DiffLine::Removed("a".into()),
                    DiffLine::Removed("b".into()),
                    DiffLine::Added("c".into())
                ]
            }
        );
    }

    #[test]
    fn what_the_agent_does_follows_its_last_event() {
        let mut c = Conversation::new("lead".into(), None);
        assert_eq!(c.activity(), Activity::Thinking, "a turn that just started");
        c.apply(&event(
            1,
            "agent.prompted",
            json!({"text": "[flottille:user]\nhi"}),
        ));
        assert_eq!(c.activity(), Activity::Thinking);
        c.apply(&event(2, "agent.thought", json!({"text": "hm"})));
        assert_eq!(c.activity(), Activity::Thinking);
        c.apply(&event(3, "agent.message", json!({"text": "On it"})));
        assert_eq!(c.activity(), Activity::Writing);
        c.apply(&running_call(4, "a"));
        assert_eq!(c.activity(), Activity::Running("fs_read".into()));
        assert_eq!(c.activity().line(), "Running fs_read…");
        c.apply(&event(
            5,
            "agent.tool_call",
            json!({"update": true, "id": "a", "status": "completed"}),
        ));
        assert_eq!(
            c.activity(),
            Activity::Thinking,
            "its call is done, the next is awaited"
        );
    }

    #[test]
    fn a_picture_sent_alone_is_its_thumbnails_and_no_empty_bubble() {
        let mut c = Conversation::new("lead".into(), None);
        c.apply(&event(
            1,
            "agent.prompted",
            json!({"text": "[flottille:user]\n", "images": ["a-0.png"]}),
        ));
        assert_eq!(
            c.blocks(),
            [Block::Images {
                names: vec!["a-0.png".into()]
            }]
        );
        c.apply(&event(
            2,
            "agent.prompted",
            json!({"text": "[flottille:user]\nlook", "images": ["b-0.png"]}),
        ));
        assert!(matches!(&c.blocks()[1], Block::Message { text, .. } if text == "look"));
        assert!(matches!(&c.blocks()[2], Block::Images { .. }));
    }

    fn running_call(seq: u64, id: &str) -> Event {
        event(
            seq,
            "agent.tool_call",
            json!({"id": id, "title": "fs_read", "status": "in_progress"}),
        )
    }

    #[test]
    fn a_call_left_running_when_the_turn_or_the_agent_ends_is_interrupted() {
        for end in [
            "agent.turn_ended",
            "agent.died",
            "agent.failed",
            "agent.ended",
        ] {
            let mut c = root();
            c.apply(&running_call(1, "a"));
            c.apply(&event(2, "agent.thought", json!({"text": "Hm"})));
            c.apply(&running_call(3, "b"));
            c.apply(&event(
                4,
                "agent.tool_call",
                json!({"id": "b", "update": true, "status": "completed"}),
            ));

            let changes = c.apply(&event(9, end, json!({"reason": "gone"})));

            assert!(changes.contains(&Change::Changed(0)), "{end}: {changes:?}");
            let Block::Steps { calls } = &c.blocks()[0] else {
                panic!("{:?}", c.blocks());
            };
            assert_eq!(calls[0].state, CallState::Interrupted, "{end}");
            assert_eq!(calls[0].duration_ms(), Some(800), "{end}");
            assert!(
                matches!(c.blocks()[1], Block::Thought { until: Some(_), .. }),
                "{end}: the thought ended"
            );
            let Block::Steps { calls } = &c.blocks()[2] else {
                panic!("{:?}", c.blocks());
            };
            assert_eq!(
                calls[0].state,
                CallState::Done,
                "{end}: a call that ended stays so"
            );
            // A late update does not start it again.
            c.apply(&event(
                10,
                "agent.tool_call",
                json!({"id": "a", "update": true, "status": "in_progress"}),
            ));
            let Block::Steps { calls } = &c.blocks()[0] else {
                panic!();
            };
            assert_eq!(calls[0].state, CallState::Interrupted, "{end}");
        }
    }

    #[test]
    fn a_request_another_agent_asked_is_placed_when_it_was_asked() {
        let mut c = root();
        c.place_approvals(vec![(450, 7), (50, 3)]);
        c.apply(&event(1, "agent.message", json!({"text": "Reading."})));
        c.apply(&event(4, "agent.message", json!({"text": " Done."})));
        c.apply(&event(9, "agent.thought", json!({"text": "Next"})));
        assert_eq!(
            c.blocks(),
            [
                Block::Approval { number: 3 },
                Block::Text {
                    text: "Reading. Done.".into()
                },
                Block::Approval { number: 7 },
                Block::Thought {
                    text: "Next".into(),
                    since: 900,
                    until: None
                },
            ]
        );
        assert!(c.add_approval(7).is_empty(), "once");
    }

    const RUN_ANSWER: &str = "run 5d2c0f1a9b3e: `cargo test` exited with code 101 after 12.4 s\n(lines 1-3 of 3)\n\u{1b}[32mok\u{1b}[0m one\n\u{1b}[1;31mFAILED\u{1b}[0m two\nthree";

    fn answered(id: &str, tool: &str, text: &str) -> [Event; 2] {
        let at = if id == "b" { 10 } else { 1 };
        [
            event(
                at,
                "agent.tool_call",
                json!({"id": id, "title": format!("mcp__flottille__{tool}"),
                    "status": "in_progress", "raw_input": {"line": "cargo test"}}),
            ),
            event(
                at + 1,
                "agent.tool_call",
                json!({"id": id, "update": true, "status": "completed", "raw_output":
                    {"result": {"content": [{"type": "text", "text": text}]}}}),
            ),
        ]
    }

    #[test]
    fn a_command_s_result_is_a_card_with_its_code_its_duration_and_its_lines() {
        let mut c = root();
        for event in answered("a", "commands_run", RUN_ANSWER) {
            c.apply(&event);
        }

        let Block::Command(card) = &c.blocks()[1] else {
            panic!("{:?}", c.blocks());
        };
        assert_eq!(card.line, "cargo test");
        assert_eq!(card.ending, Ending::Exited(101));
        assert_eq!(card.seconds, Some(12.4));
        assert_eq!(card.allowed_by, None);
        // The program's bytes, for the card to colour or strip.
        assert_eq!(
            card.lines,
            [
                "\u{1b}[32mok\u{1b}[0m one",
                "\u{1b}[1;31mFAILED\u{1b}[0m two",
                "three"
            ]
        );
        assert!(
            matches!(c.blocks()[0], Block::Steps { .. }),
            "the call's line stays"
        );
    }

    #[test]
    fn a_command_still_running_when_it_answered_is_a_running_card_without_the_advice() {
        let mut c = root();
        let text = "run r1: server (`npm run dev`) is running after 1.0 s\nit keeps going: read it with commands_status {\"run\": \"r1\"}, stop it with commands_status {\"run\": \"r1\", \"action\": \"stop\"}\n(lines 1-1 of 1)\nlistening";
        for event in answered("a", "commands_run", text) {
            c.apply(&event);
        }

        let Block::Command(card) = &c.blocks()[1] else {
            panic!("{:?}", c.blocks());
        };
        assert_eq!(card.line, "npm run dev");
        assert_eq!(card.ending, Ending::Running);
        assert_eq!(card.lines, ["listening"]);
    }

    #[test]
    fn a_result_that_is_no_command_s_is_no_card() {
        let mut c = root();
        for event in answered(
            "a",
            "commands_run",
            "refused: the catalogue has no command x",
        ) {
            c.apply(&event);
        }
        for event in answered("b", "fs_read", RUN_ANSWER) {
            c.apply(&event);
        }

        assert!(c.blocks().iter().all(|b| !matches!(b, Block::Command(_))));
    }

    #[test]
    fn an_approved_command_is_the_same_card_and_says_who_allowed_it_never_the_raw_text() {
        let mut c = root();
        let body = format!(
            "[flottille:approval]\nRequest #7 (commands_run) was approved by the user, and \
             Flottille has now done it. Its result:\n\n{RUN_ANSWER}"
        );
        c.apply(&event(1, "agent.prompted", json!({ "text": body })));

        let Block::Command(card) = &c.blocks()[0] else {
            panic!("{:?}", c.blocks());
        };
        assert_eq!(card.ending, Ending::Exited(101));
        assert_eq!(card.allowed_by.as_deref(), Some("you"));
        assert_eq!(card.lines.len(), 3);
        assert!(c.blocks().iter().all(|b| !matches!(
            b,
            Block::Notice { text, .. } if text.contains("Request #")
        )));
    }

    #[test]
    fn the_other_answers_to_a_request_are_a_sentence_and_never_the_raw_text() {
        let mut c = root();
        for (n, body) in [
            "Request #1 (commands_run) was refused by the user. Nothing was done.",
            "Request #2 (fs_write) was not executed: the agent that asked has ended.",
            "Request #3 (fs_write) was approved by the user, and Flottille has now done it. Its result:\n\nwrote 3 bytes",
            "Request #4 (commands_run) was approved by the user, but nothing was done: cannot run it",
        ]
        .into_iter()
        .enumerate()
        {
            c.apply(&event(
                n as u64 + 1,
                "agent.prompted",
                json!({"text": format!("[flottille:approval]\n{body}")}),
            ));
        }

        let said: Vec<&str> = c
            .blocks()
            .iter()
            .map(|b| match b {
                Block::Notice { text, .. } => text.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            said,
            [
                "you refused commands_run (request #1)",
                "fs_write (request #2) was not done: the agent that asked has ended.",
                "you allowed fs_write (request #3)",
                "you allowed commands_run (request #4) but nothing was done: cannot run it",
            ]
        );
    }

    #[test]
    fn a_call_s_arguments_are_cut_for_its_line() {
        let mut c = root();
        let content = "x".repeat(5_000);
        c.apply(&event(
            1,
            "agent.tool_call",
            json!({"id": "w", "title": "fs_write", "status": "in_progress",
                "raw_input": {"path": "a.txt", "content": content}}),
        ));
        let Block::Steps { calls } = &c.blocks()[0] else {
            panic!();
        };
        assert_eq!(calls[0].args.chars().count(), ARGS + 1);
        assert!(calls[0].args.ends_with('…'));
    }
}
