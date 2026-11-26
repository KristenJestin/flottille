//! One table of the tree's scenarios, each played twice (`docs/remote.md`, "Starting a session on
//! another machine"): **here**, pc's root launching with no `machine`, and **across**, the same
//! call with `machine: "server"`, two daemons linked by a hub served in the test on 127.0.0.1. What
//! the agents and the user get must be the same once ids, machine names, refs and numbers are
//! blanked: an agent launches sessions on another machine as it does on its own, only saying
//! where. A difference is a failure unless [`ALLOWED`] names it with its reason. Each machine has
//! its own temporary HOME and XDG folders, fake agents and a fake judge that allows; throwaway
//! repositories.
//!
//! A row a fix has not landed for yet is ignored, naming the issue that turns it on.

use crate::remote_deliveries::script;
use crate::remote_launch::{
    Machine, TestHub, a_while, allowing, eventually, pc_and_server_with, server_user,
};

use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp};
use flottille_core::store::Store;
use serde_json::{Value, json};

use crate::common;

const TITLE: &str = "Finish the export";
const BRIEF: &str = "Finish the export of the report.";
const QUESTION: &str = "Which format first, CSV or JSON?";

/// The differences the specification documents, by what is compared, each with its reason.
/// Each is what is compared, the start of the lines (blanked) left out of both runs, an empty
/// start leaving every line out, and why.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "child: answer",
        "",
        "a child on another machine works on a copy of its parent's place, and its commits come \
         back on refs/flottille/machines/<machine>/<launch>/<branch> (docs/tools.md, agent_launch)",
    ),
    (
        "*",
        "It made no new commit in ",
        "the commits of a session on another machine come back with its report or its end, which \
         say what came back (docs/remote.md, \"Coming back\")",
    ),
    (
        "child: root's deliveries",
        "It worked in ",
        "a child on another machine works in a workspace of its own there, a copy of its parent's \
         place (docs/tools.md, agent_launch)",
    ),
    (
        "ask: session_send",
        "Sent to ",
        "a message to a session on another machine is kept until that machine takes it, and its \
         answer names that machine (docs/remote.md, \"A tree across machines\"); a refusal still \
         fails the row",
    ),
    (
        "held: answer",
        "It will run as ",
        "a launch held on another machine runs there once allowed, on a copy of its caller's \
         place, and its commits come back on a ref of their own (docs/tools.md, agent_launch)",
    ),
    (
        "held: root's deliveries",
        "Started a session ",
        "the answer of a session started on another machine is led_started_answer's row; this \
         row compares how the user's answer reaches the caller",
    ),
    (
        "led: answer",
        "",
        "a session started on another machine works on a copy of its caller's place, its \
         commits come back on refs/flottille/machines/<machine>/<launch>/<branch>, and its wakes \
         are kept while the caller's machine is off (docs/tools.md, agent_launch)",
    ),
    (
        "handed: answer",
        "",
        "a session handed over on another machine works on a copy of its caller's place, and its \
         commits come back on a ref of their own (docs/tools.md, agent_launch)",
    ),
    (
        "led: first message",
        "While ",
        "the pilot of a session on another machine may be off: its questions then go to the \
         user's inbox there (docs/remote.md, \"Where a question goes when the parent is off\")",
    ),
];

/// Where the launched agent runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Topology {
    /// On pc, the root's own machine: the launch names no `machine`.
    Here,
    /// On server: the same launch with `machine: "server"`.
    Across,
}

/// One scenario played in one topology: the two machines, pc's root and its MCP session.
struct Play {
    topology: Topology,
    _hub: TestHub,
    pc: Machine,
    server: Machine,
    root: String,
    mcp: Mcp,
}

impl Play {
    /// pc and server linked, each judge allowing, the fake agents of the machine the launched
    /// agent runs on playing `turns`, and pc's root in `mode`. `forge` declares a forge for the
    /// project on pc first, so that the root has the `forge` tool.
    fn new(topology: Topology, mode: &str, turns: Value) -> Play {
        Play::with(topology, mode, turns, false)
    }

    fn with(topology: Topology, mode: &str, turns: Value, forge: bool) -> Play {
        let pc_user = server_user(allowing());
        let (hub, pc, server) =
            pc_and_server_with("runner", pc_user, server_user(allowing()), Some(""));
        if forge {
            pc.git(
                &pc.folder(),
                &["remote", "add", "origin", "git@github.com:acme/app.git"],
            );
            pc.user
                .call(
                    "project.set_settings",
                    json!({"name": "app", "repositories": [{"path": ".",
                        "forge": {"kind": "github", "repository": "acme/app"}}]}),
                )
                .unwrap();
        }
        let there = match topology {
            Topology::Here => &pc,
            Topology::Across => &server,
        };
        script(there, turns);
        if topology == Topology::Across {
            pc.wait_for_offer("server", 12);
        }
        let (root, mcp) = pc.root(mode);
        Play {
            topology,
            _hub: hub,
            pc,
            server,
            root,
            mcp,
        }
    }

    /// The machine the launched agent runs on.
    fn there(&self) -> &Machine {
        match self.topology {
            Topology::Here => &self.pc,
            Topology::Across => &self.server,
        }
    }

    /// The root's `agent_launch` of `kind` titled `title` with `brief`, `machine` named across.
    fn launch(&self, kind: &str, title: &str, brief: &str) -> Answer {
        let mut args = json!({"kind": kind, "title": title, "brief": brief});
        if self.topology == Topology::Across {
            args["machine"] = "server".into();
        }
        self.mcp.call("agent_launch", args)
    }

    /// The root's launch of `kind`, started at once: the answer and the launched agent's id.
    fn started(&self, kind: &str) -> (Answer, String) {
        let answer = self.launch(kind, TITLE, BRIEF);
        assert!(!answer.error, "{}", answer.text);
        (answer, self.titled(TITLE))
    }

    /// The id of the agent titled `title` where the launched agent runs, once there.
    fn titled(&self, title: &str) -> String {
        let find = || {
            self.there()
                .agents()
                .into_iter()
                .find(|agent| agent["title"] == title)
        };
        eventually(&format!("an agent titled {title}"), || find().is_some());
        find().unwrap()["id"].as_str().unwrap().to_owned()
    }

    /// Waits until the agent `id` there ended its first turn.
    fn idle(&self, id: &str) {
        eventually("the launched agent idle", || {
            self.there().agent(id)["state"] == "idle"
        });
    }

    /// The first request waiting for the user where the launched agent runs: its number.
    fn request(&self) -> u64 {
        eventually("a request for the user", || {
            !self.there().requests().is_empty()
        });
        self.there().requests()[0]["number"].as_u64().unwrap()
    }

    /// What the fake agents there were answered by their tools, in order: the launched agents'
    /// calls (the root's calls are the test's own).
    fn tool_answers(&self) -> Vec<String> {
        self.there()
            .user
            .received()
            .iter()
            .filter(|record| record.get("mcp").is_some())
            .map(|record| record["mcp"]["body"].to_string())
            .collect()
    }

    /// The first prompt there holding `text`.
    fn prompt_with(&self, text: &str) -> String {
        eventually(&format!("a prompt with {text}"), || {
            self.there()
                .prompts()
                .iter()
                .any(|prompt| prompt.contains(text))
        });
        self.there()
            .prompts()
            .into_iter()
            .find(|prompt| prompt.contains(text))
            .unwrap()
    }

    /// The deliveries stored for the agent `id` of `machine`, oldest first, each `kind: body`.
    fn deliveries(machine: &Machine, id: &str) -> Vec<String> {
        let store =
            Store::open_read_only(&machine.user.path("state/flottille/flottille.db")).unwrap();
        let mut all: Vec<String> = store
            .last_deliveries(id, 64)
            .unwrap()
            .into_iter()
            .map(|delivery| format!("{}: {}", delivery.kind, delivery.body))
            .collect();
        all.reverse();
        all
    }

    /// The deliveries of pc's root, less what the user wrote to it.
    fn root_deliveries(&self) -> Vec<String> {
        Play::deliveries(&self.pc, &self.root)
            .into_iter()
            .filter(|delivery| !delivery.starts_with("user: "))
            .collect()
    }

    /// The agent `id` there, as `agent.show` gives it.
    fn shown(&self, id: &str) -> Value {
        self.there().agent(id)
    }

    /// `agent.show`'s relation fields of the agent `id` there, present or not, and its depth.
    fn relations(&self, id: &str) -> String {
        let agent = self.shown(id);
        let present = |field: &str| {
            if agent[field].is_null() {
                "none"
            } else {
                "set"
            }
        };
        format!(
            "parent {}, led_by {}, launched_by {}, depth {}, role {}",
            present("parent"),
            present("led_by"),
            present("launched_by"),
            agent["depth"],
            agent["role"]
        )
    }

    /// The tools of the agent `id` there, sorted.
    fn tools(&self, id: &str) -> String {
        let mut tools: Vec<String> = self.shown(id)["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool.as_str().unwrap().to_owned())
            .collect();
        tools.sort();
        tools.join(", ")
    }

    /// How the agent `id` of `machine` stands: its state and why.
    fn end_of(machine: &Machine, id: &str) -> String {
        let agent = machine.agent(id);
        format!("{} ({})", agent["state"], agent["state_reason"])
    }

    /// The kinds of the user's inbox entries, of every machine, sorted.
    fn inbox(&self) -> String {
        let mut kinds: Vec<String> = [&self.pc, &self.server]
            .iter()
            .flat_map(|machine| {
                machine.user.call("inbox.list", json!({})).unwrap()["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["kind"].as_str().unwrap_or("?").to_owned())
                    .collect::<Vec<_>>()
            })
            .collect();
        kinds.sort();
        kinds.join(", ")
    }

    /// How many requests wait for the user, of every machine.
    fn requests(&self) -> usize {
        self.pc.requests().len() + self.server.requests().len()
    }
}

/// Waits up to a while for `check`, and goes on either way: what did not come shows as a
/// difference between the topologies, not as a timeout.
fn soon(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What an answer says once what differs by nature between two runs is the same: the ids (the
/// stand-in's machine part with them), the machine names, the refs of the commits coming back,
/// the cursors and every number (request numbers, times, counts of characters).
fn blank(text: &str) -> String {
    let mut text = text.to_owned();
    // A cursor names an event of its machine's store.
    for marker in ["\"cursor\": \"", "\\\"cursor\\\": \\\""] {
        let mut from = 0;
        while let Some(at) = text[from..].find(marker) {
            let start = from + at + marker.len();
            let end = start + text[start..].find(['"', '\\']).unwrap_or(0);
            text.replace_range(start..end, "<cursor>");
            from = start + "<cursor>".len();
        }
    }
    // The commits of a launch elsewhere come back on a ref of their own.
    let marker = "refs/flottille/machines/";
    while let Some(at) = text.find(marker) {
        let rest = &text[at + marker.len()..];
        let cut = rest
            .splitn(3, '/')
            .take(2)
            .map(|part| part.len() + 1)
            .sum::<usize>();
        text.replace_range(at..at + marker.len() + cut, "");
    }
    for (machine, none) in [
        ("\"machine\": \"server\", ", ""),
        ("; it goes to the user of server", ""),
        ("On server: ", ""),
        (", on server", ""),
        (" (on server)", ""),
        (" on server", ""),
        (" of pc", ""),
        (" on pc", ""),
    ] {
        text = text.replace(machine, none);
    }
    // A number of any length is one `0`: `#4` and `#17` say the same.
    let mut out = String::new();
    for c in ids_blanked(&text).chars() {
        if !c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('0') {
            out.push('0');
        }
    }
    out
}

/// `text` with each agent id (12 hexadecimal digits) as `<id>`, a commit as `<commit>`, and a stand-in's machine part
/// (`@` and 64 hexadecimal digits) left out.
fn ids_blanked(text: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let hex = word
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        if hex && word.len() == 12 {
            out.push_str("<id>");
        } else if hex && (7..=40).contains(&word.len()) && word.chars().any(|c| c.is_ascii_digit())
        {
            // A commit, as git abbreviates it or whole.
            out.push_str("<commit>");
        } else if hex && word.len() == 64 && out.ends_with('@') {
            out.pop();
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// What one scenario saw in one topology, in order: what is compared, and the text seen.
type Seen = Vec<(String, String)>;

/// Plays `scenario` here and across and compares what each saw, blanked; a difference
/// [`ALLOWED`] does not name fails the row, each shown whole.
fn parity(scenario: impl Fn(Topology) -> Seen) {
    let here = scenario(Topology::Here);
    let across = scenario(Topology::Across);
    let keys: Vec<&String> = here.iter().map(|(key, _)| key).collect();
    assert_eq!(
        keys,
        across.iter().map(|(key, _)| key).collect::<Vec<_>>(),
        "the two runs compared different things"
    );
    let mut differences = Vec::new();
    for ((key, seen_here), (_, seen_across)) in here.iter().zip(&across) {
        let (here, across) = (allowed(key, seen_here), allowed(key, seen_across));
        if here == across {
            continue;
        }
        differences.push(format!(
            "── {key}\n   here:   {here}\n   across: {across}\n   (raw here:   {seen_here})\n   (raw across: {seen_across})"
        ));
    }
    assert!(
        differences.is_empty(),
        "here and across differ:\n{}",
        differences.join("\n")
    );
}

/// `text` blanked, less the lines [`ALLOWED`] leaves out of what `key` compares. A line break
/// inside a JSON string (a prompt as the fake agent got it) is a line too.
fn allowed(key: &str, text: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    for line in blank(text).replace("\\n", "\n").lines() {
        let left_out = ALLOWED.iter().any(|(what, start, _)| {
            (*what == "*" || *what == key) && line.trim_start().starts_with(start)
        });
        if !left_out {
            kept.push(line.to_owned());
        // The last line of a prompt closes its JSON: that stays, on the line before, when the
        // line is left out.
        } else if let (Some(at), Some(last)) = (json_end(line), kept.last_mut()) {
            last.push_str(&line[at..]);
        }
    }
    kept.join("\n")
}

/// Where a prompt the fake agent kept, a JSON list of one text block, closes on `line`: after the
/// text when its keys are sorted, after `type` when they keep the order they were written in.
fn json_end(line: &str) -> Option<usize> {
    ["\",\"type\":\"text\"}]", "\"}]"]
        .iter()
        .filter_map(|end| line.find(end))
        .min()
}

/// What a tool answered, said with whether it was an error.
fn said(answer: &Answer) -> String {
    format!(
        "{}{}",
        if answer.error { "error: " } else { "" },
        answer.text
    )
}

fn seen(key: &str, text: impl Into<String>) -> (String, String) {
    (key.to_owned(), text.into())
}

fn list(items: Vec<String>) -> String {
    items.join("\n | ")
}

fn ask(question: &str) -> Value {
    json!({"mcp": {"tool": "ask", "arguments": {"question": question}}})
}

fn report(text: &str) -> Value {
    json!({"mcp": {"tool": "report", "arguments": {"text": text}}})
}

/// The launched agent's first turn plays `steps`, once.
fn first_turn(steps: Value) -> Value {
    json!([{"when": BRIEF, "times": 1, "do": steps}])
}

// The led sessions.

#[test]
fn led_started_answer() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (answer, session) = play.started("led");
        vec![
            seen("led: answer", said(&answer)),
            seen("led: relations", play.relations(&session)),
        ]
    });
}

/// A led launch held for the user, who decides with `method`: what the caller hears.
fn held_then(method: &'static str) -> impl Fn(Topology) -> Seen {
    move |topology| {
        let play = Play::new(topology, "ask", json!([]));
        let answer = play.launch("led", TITLE, BRIEF);
        let number = play.request();
        play.there()
            .user
            .call(method, json!({"number": number}))
            .unwrap();
        soon(|| !play.root_deliveries().is_empty());
        a_while();
        vec![
            seen("held: answer", said(&answer)),
            seen("held: root's deliveries", list(play.root_deliveries())),
        ]
    }
}

/// The prompt as the fake agent kept it, its JSON keys in either order: the order follows the
/// features the build unifies (`preserve_order` when the whole workspace is built, not when one
/// crate is), and the parity must not depend on it.
#[test]
fn a_line_left_out_closes_the_prompt_whatever_the_order_of_its_keys() {
    let guide = "Where a question goes: it is led.\\nWhile pc is not linked to this machine, ask \
                 reaches the user's inbox instead.";
    let here = "[{\"text\":\"Brief.\\n\\nWhere a question goes: it is led.\",\"type\":\"text\"}]";
    let across = format!("[{{\"text\":\"Brief.\\n\\n{guide}\",\"type\":\"text\"}}]");
    assert_eq!(
        allowed("led: first message", here),
        allowed("led: first message", &across)
    );
    let here = "[{\"type\":\"text\",\"text\":\"Brief.\\n\\nWhere a question goes: it is led.\"}]";
    let across = format!("[{{\"type\":\"text\",\"text\":\"Brief.\\n\\n{guide}\"}}]");
    assert_eq!(
        allowed("led: first message", here),
        allowed("led: first message", &across)
    );
    // What the user sees still counts: a different line before it is a difference.
    let other = across.replace("it is led", "it is not");
    assert_ne!(
        allowed("led: first message", here),
        allowed("led: first message", &other)
    );
}

#[test]
fn led_held_then_allowed() {
    parity(held_then("approval.approve"));
}

#[test]
fn led_held_then_refused() {
    parity(held_then("approval.deny"));
}

#[test]
fn led_asks_its_pilot() {
    parity(|topology| {
        let play = Play::new(topology, "auto", first_turn(json!([ask(QUESTION)])));
        let (_, session) = play.started("led");
        soon(|| {
            play.root_deliveries()
                .iter()
                .any(|delivery| delivery.contains(QUESTION))
        });
        let woken = list(play.root_deliveries());
        let asked = list(play.tool_answers());
        let answer = play.mcp.call(
            "session_send",
            json!({"session": session, "text": "CSV first, JSON next."}),
        );
        soon(|| {
            play.there()
                .prompts()
                .iter()
                .any(|prompt| prompt.contains("CSV first, JSON next."))
        });
        let reached = play
            .there()
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("CSV first, JSON next."));
        vec![
            seen("ask: the answer reached the session", reached.to_string()),
            seen("ask: the session's answer", asked),
            seen("ask: root's deliveries", woken),
            seen("ask: session_send", said(&answer)),
            seen("ask: inbox", play.inbox()),
        ]
    });
}

#[test]
fn led_reports() {
    // Longer than a wake carries: the cut is said.
    let long = "The export is done and tested. ".repeat(40);
    parity(|topology| {
        let play = Play::new(topology, "auto", first_turn(json!([report(&long)])));
        play.started("led");
        soon(|| !play.root_deliveries().is_empty());
        a_while();
        vec![
            seen("report: the session's answer", list(play.tool_answers())),
            seen("report: root's deliveries", list(play.root_deliveries())),
        ]
    });
}

#[test]
fn led_ends() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (_, session) = play.started("led");
        play.idle(&session);
        play.there()
            .user
            .call("agent.stop", json!({"agent": session}))
            .unwrap();
        soon(|| !play.root_deliveries().is_empty());
        a_while();
        vec![
            seen("end: session", Play::end_of(play.there(), &session)),
            seen("end: root's deliveries", list(play.root_deliveries())),
        ]
    });
}

#[test]
fn led_fails() {
    parity(|topology| {
        let play = Play::new(
            topology,
            "auto",
            json!([
                {"when": BRIEF, "times": 1, "do": [{"die": 4}]},
                // It goes on in its session, told its turn was cut: it dies there too.
                {"when": "your last turn was cut", "do": [{"die": 4}]}
            ]),
        );
        let (_, session) = play.started("led");
        soon(|| !play.root_deliveries().is_empty());
        a_while();
        vec![
            seen("fail: session", Play::end_of(play.there(), &session)),
            seen("fail: root's deliveries", list(play.root_deliveries())),
        ]
    });
}

#[test]
fn pilot_reads_its_led_session() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (_, session) = play.started("led");
        play.idle(&session);
        let one = play
            .mcp
            .call("session_read", json!({"session": session, "last": 1}));
        let all = play
            .mcp
            .call("session_read", json!({"session": session, "last": 99}));
        let cursor = one
            .text
            .split("\"cursor\": \"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .map(str::to_owned);
        let after = match cursor {
            Some(cursor) => said(&play.mcp.call(
                "session_read",
                json!({"session": session, "cursor": cursor}),
            )),
            None => "no cursor".to_owned(),
        };
        vec![
            seen("read: last 1", said(&one)),
            seen("read: last 99", said(&all)),
            seen("read: from the cursor", after),
        ]
    });
}

#[test]
fn pilot_stops_its_led_session() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (_, session) = play.started("led");
        play.idle(&session);
        let answer = play.mcp.call("agent_stop", json!({"agent_id": session}));
        soon(|| play.shown(&session)["state"] == "ended");
        soon(|| !play.root_deliveries().is_empty());
        a_while();
        vec![
            seen("stop: answer", said(&answer)),
            seen("stop: session", Play::end_of(play.there(), &session)),
            seen("stop: root's deliveries", list(play.root_deliveries())),
        ]
    });
}

#[test]
fn pilot_ends_and_its_led_sessions_end() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (_, session) = play.started("led");
        play.idle(&session);
        let ending = play
            .pc
            .user
            .call("agent.ending", json!({"agent": play.root}))
            .unwrap();
        play.pc
            .user
            .call("agent.stop", json!({"agent": play.root}))
            .unwrap();
        soon(|| play.shown(&session)["state"] == "ended");
        // The End confirmation whole, less what differs by nature: the session there works in a
        // workspace of its own, a copy of the pilot's place, which here it does not
        // (`workspace`), and is said with its machine (`machine`).
        let mut ending = ending;
        for session in ending["sessions"].as_array_mut().unwrap() {
            let session = session.as_object_mut().unwrap();
            session.remove("workspace");
            session.remove("machine");
        }
        vec![
            seen("pilot's end: agent.ending", ending.to_string()),
            seen("pilot's end: session", Play::end_of(play.there(), &session)),
        ]
    });
}

#[test]
fn led_first_message_says_where_questions_go() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        play.started("led");
        vec![seen("led: first message", play.prompt_with(BRIEF))]
    });
}

#[test]
fn led_launches_a_session() {
    parity(|topology| {
        let play = Play::new(
            topology,
            "auto",
            first_turn(json!([{"mcp": {"tool": "agent_launch", "arguments": {
                "kind": "led", "title": "Write the docs", "brief": "Write the docs of the export."}}}])),
        );
        let (_, session) = play.started("led");
        soon(|| !play.tool_answers().is_empty());
        play.idle(&session);
        vec![
            seen("chain: the session's answer", list(play.tool_answers())),
            seen("chain: requests", play.requests().to_string()),
        ]
    });
}

#[test]
fn detach_a_led_session() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (_, session) = play.started("led");
        play.idle(&session);
        let detached = match play
            .there()
            .user
            .call("agent.detach", json!({"agent": session}))
        {
            Ok(_) => "detached".to_owned(),
            Err((_, message)) => format!("refused: {message}"),
        };
        soon(|| !play.root_deliveries().is_empty());
        a_while();
        vec![
            seen("detach: answer", detached),
            seen("detach: relations", play.relations(&session)),
            seen("detach: root's deliveries", list(play.root_deliveries())),
        ]
    });
}

#[test]
fn unsubscribe_from_a_led_session() {
    parity(|topology| {
        let play = Play::with(topology, "auto", json!([]), true);
        let (_, session) = play.started("led");
        play.idle(&session);
        let following = || {
            play.pc.user.call("subscription.list", json!({})).unwrap()["subscriptions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|subscription| subscription["agent"] == play.root.as_str())
                .map(|subscription| subscription["source"].to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let before = following();
        let answer = play.mcp.call(
            "forge",
            json!({"action": "unsubscribe", "session": session}),
        );
        vec![
            seen("unsubscribe: subscriptions before", before),
            seen("unsubscribe: answer", said(&answer)),
            seen("unsubscribe: subscriptions after", following()),
        ]
    });
}

// The children.

#[test]
#[ignore = "#1512, next round: a child launched across has no parent there, its caller a stand-in"]
fn child_tools_and_depth() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (answer, child) = play.started("child");
        vec![
            seen("child: answer", said(&answer)),
            seen("child: relations", play.relations(&child)),
            seen("child: tools", play.tools(&child)),
        ]
    });
}

/// A child whose first turn plays `steps`: the deliveries its parent got, and what its tools
/// answered it.
fn child_does(steps: Value) -> impl Fn(Topology) -> Seen {
    move |topology| {
        let play = Play::new(topology, "auto", first_turn(steps.clone()));
        let (_, child) = play.started("child");
        soon(|| !play.root_deliveries().is_empty());
        a_while();
        vec![
            seen("child: its answers", list(play.tool_answers())),
            seen("child: root's deliveries", list(play.root_deliveries())),
            seen("child: state", Play::end_of(play.there(), &child)),
            seen("child: requests", play.requests().to_string()),
            seen("child: inbox", play.inbox()),
        ]
    }
}

#[test]
fn child_reports() {
    parity(child_does(json!([report("The export tests are written.")])));
}

#[test]
fn child_asks() {
    parity(child_does(json!([ask(QUESTION)])));
}

#[test]
fn child_request_held_for_its_parent() {
    parity(child_does(
        json!([{"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}}]),
    ));
}

#[test]
fn child_goes_idle() {
    parity(child_does(json!([{"say": "Nothing to report yet."}])));
}

#[test]
fn parent_stops_and_its_child_stops() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        let (_, child) = play.started("child");
        play.idle(&child);
        play.pc
            .user
            .call("agent.stop", json!({"agent": play.root}))
            .unwrap();
        soon(|| play.shown(&child)["state"] == "ended");
        vec![seen(
            "parent's stop: child",
            Play::end_of(play.there(), &child),
        )]
    });
}

#[test]
fn handed_tells_nothing() {
    parity(|topology| {
        let play = Play::new(
            topology,
            "auto",
            first_turn(json!([report("The export is done.")])),
        );
        let (answer, session) = play.started("handed");
        soon(|| !play.tool_answers().is_empty());
        a_while();
        vec![
            seen("handed: answer", said(&answer)),
            seen("handed: relations", play.relations(&session)),
            seen("handed: its answers", list(play.tool_answers())),
            seen("handed: root's deliveries", list(play.root_deliveries())),
        ]
    });
}

#[test]
fn a_child_starts_a_session() {
    parity(|topology| {
        let play = Play::new(
            topology,
            "auto",
            first_turn(json!([{"mcp": {"tool": "agent_launch", "arguments": {
                "kind": "led", "title": "Write the docs", "brief": "Write the docs of the export."}}}])),
        );
        let (_, child) = play.started("child");
        soon(|| !play.tool_answers().is_empty());
        play.idle(&child);
        vec![
            seen("child's launch: its answer", list(play.tool_answers())),
            seen("child's launch: requests", play.requests().to_string()),
        ]
    });
}

// Machines are one (#1512): what one machine would do, the other does.

/// A launch of `kind` by a root the user put in Full from this machine, with no switch signed
/// from a device: the mode the launched session runs in, and what waits for the user.
fn in_full(kind: &'static str) -> impl Fn(Topology) -> Seen {
    move |topology| {
        let play = Play::new(topology, "full", json!([]));
        let (_, session) = play.started(kind);
        play.idle(&session);
        vec![
            seen("full: mode", play.shown(&session)["gate_mode"].to_string()),
            seen("full: requests", play.requests().to_string()),
            seen("full: inbox", play.inbox()),
        ]
    }
}

#[test]
fn full_follows_the_tree_to_a_child() {
    parity(in_full("child"));
}

#[test]
fn full_follows_the_tree_to_a_led_session() {
    parity(in_full("led"));
}

#[test]
fn full_follows_the_tree_to_a_handed_session() {
    parity(in_full("handed"));
}

/// A session of `kind` whose first turn writes to the root that launched it, with
/// `session_send`: what it is answered, whether the root got it, and what waits for the user.
fn writes_to_its_launcher(kind: &'static str) -> impl Fn(Topology) -> Seen {
    move |topology| {
        let play = Play::new(topology, "auto", json!([]));
        script(
            play.there(),
            first_turn(json!([{"mcp": {"tool": "session_send", "arguments": {
                "session": play.root, "text": "The export is half done."}}}])),
        );
        let (_, session) = play.started(kind);
        soon(|| !play.tool_answers().is_empty());
        play.idle(&session);
        soon(|| {
            play.root_deliveries()
                .iter()
                .any(|delivery| delivery.contains("The export is half done."))
        });
        let reached = play
            .root_deliveries()
            .iter()
            .any(|delivery| delivery.contains("The export is half done."));
        vec![
            seen("write back: its answer", list(play.tool_answers())),
            seen("write back: reached the launcher", reached.to_string()),
            seen("write back: requests", play.requests().to_string()),
        ]
    }
}

#[test]
fn a_led_session_writes_to_its_launcher() {
    parity(writes_to_its_launcher("led"));
}

#[test]
fn a_handed_session_writes_to_its_launcher() {
    parity(writes_to_its_launcher("handed"));
}

/// A session handed over at its start writes to its launcher in its first turn, before the
/// launcher's machine recorded which session the launch started (pc records it once its file
/// `slow-call` is gone): it still writes as the session the launcher started, with no leave, as
/// on one machine.
#[test]
fn a_handed_session_writes_to_its_launcher_before_its_launch_is_recorded() {
    let play = Play::new(Topology::Across, "auto", json!([]));
    script(
        play.there(),
        first_turn(json!([{"mcp": {"tool": "session_send", "arguments": {
            "session": play.root, "text": "The export is half done."}}}])),
    );
    let slow = play.pc.user.path("slow-call");
    std::fs::write(&slow, "").unwrap();
    std::thread::scope(|scope| {
        let launching = scope.spawn(|| play.launch("handed", TITLE, BRIEF));
        play.titled(TITLE);
        eventually("the write back come to pc", || {
            !play.pc.requests().is_empty() || play.pc.log().contains("waits for its launches")
        });
        std::fs::remove_file(&slow).unwrap();
        let launched = launching.join().unwrap();
        assert!(!launched.error, "{}", launched.text);
    });
    soon(|| !play.tool_answers().is_empty());
    let answers = play.tool_answers();
    assert!(
        answers
            .first()
            .is_some_and(|answer| answer.contains("Sent to agent")),
        "{answers:#?}"
    );
    soon(|| {
        play.root_deliveries()
            .iter()
            .any(|delivery| delivery.contains("The export is half done."))
    });
    assert_eq!(play.pc.requests().len(), 0, "{:#}", play.pc.log());
}

/// A led session whose launcher paused their following (fifty messages without the user, the
/// last asking it to write back) writes to its launcher with `session_send`: refused as paused,
/// and nothing reaches the launcher, on one machine as on two.
#[test]
fn a_paused_launcher_refuses_a_write_back_as_on_one_machine() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        script(
            play.there(),
            json!([{"when": "Write back now", "times": 1, "do": [{"mcp": {"tool": "session_send",
                "arguments": {"session": play.root, "text": "The export is half done."}}}]}]),
        );
        let (_, session) = play.started("led");
        play.idle(&session);
        let texts = (1..50).map(|_| "Go on.").chain(["Write back now"]);
        let sent: Vec<String> = texts
            .map(|text| {
                said(
                    &play
                        .mcp
                        .call("session_send", json!({"session": session, "text": text})),
                )
            })
            .filter(|said| said.starts_with("error: "))
            .collect();
        soon(|| !play.tool_answers().is_empty());
        play.idle(&session);
        a_while();
        let reached = play
            .root_deliveries()
            .iter()
            .any(|delivery| delivery.contains("The export is half done."));
        vec![
            seen("paused: the launcher's messages refused", list(sent)),
            seen("paused: its answer", list(play.tool_answers())),
            seen("paused: reached the launcher", reached.to_string()),
            seen("paused: requests", play.requests().to_string()),
        ]
    });
}

/// A led session the user detached writes to its former launcher with `session_send`: it is the
/// user's alone, so it needs a leave there as any other session does, on one machine as on two.
#[test]
fn a_detached_session_across_needs_a_leave_to_write_to_its_former_launcher() {
    parity(|topology| {
        let play = Play::new(topology, "auto", json!([]));
        script(
            play.there(),
            json!([{"when": "Write back now", "times": 1, "do": [{"mcp": {"tool": "session_send",
                "arguments": {"session": play.root, "text": "The export is half done."}}}]}]),
        );
        let (_, session) = play.started("led");
        play.idle(&session);
        play.there()
            .user
            .call("agent.detach", json!({"agent": session}))
            .unwrap();
        soon(|| !play.root_deliveries().is_empty());
        play.there()
            .user
            .call(
                "agent.send",
                json!({"agent": session, "message": "Write back now"}),
            )
            .unwrap();
        soon(|| !play.tool_answers().is_empty());
        play.idle(&session);
        a_while();
        let reached = play
            .root_deliveries()
            .iter()
            .any(|delivery| delivery.contains("The export is half done."));
        vec![
            seen("detached: its answer", list(play.tool_answers())),
            seen("detached: reached the launcher", reached.to_string()),
            seen("detached: requests", play.requests().to_string()),
        ]
    });
}

/// The root, in `mode`, with uncommitted work in its place, launches a led session in a workspace
/// of its own, which ends; then removes that workspace: what it is answered, and what waits for
/// the user. Here the session is given a new workspace; there it works in the copy of the root's
/// place made for it, uncommitted work included, which is the root's and still on its machine.
fn removes_the_workspace_of_its_led_session(mode: &'static str) -> impl Fn(Topology) -> Seen {
    move |topology| {
        let play = Play::new(topology, mode, json!([]));
        let mut args = json!({"kind": "led", "title": TITLE, "brief": BRIEF});
        match topology {
            Topology::Here => args["new_workspace"] = "feat-export".into(),
            Topology::Across => args["machine"] = "server".into(),
        }
        let answer = play.mcp.call("agent_launch", args);
        assert!(!answer.error, "{}", answer.text);
        let session = play.titled(TITLE);
        play.idle(&session);
        let workspace = play.shown(&session)["workspace"]
            .as_str()
            .unwrap_or_else(|| panic!("no workspace: {:#}", play.shown(&session)))
            .to_owned();
        play.there()
            .user
            .call("agent.stop", json!({"agent": session}))
            .unwrap();
        soon(|| play.shown(&session)["state"] == "ended");
        let mut args = json!({"action": "remove", "name": workspace});
        if topology == Topology::Across {
            args["machine"] = "server".into();
        }
        let answer = play.mcp.call("workspace", args);
        a_while();
        // Its branch there is the root's, copied; here the one its new workspace made.
        let answer = said(&answer).replace("feat/export", "feat-export");
        vec![
            seen("removal: answer", answer),
            seen("removal: requests", play.requests().to_string()),
        ]
    }
}

#[test]
fn the_pilot_removes_the_workspace_of_its_led_session() {
    parity(removes_the_workspace_of_its_led_session("auto"));
}

#[test]
fn in_full_the_pilot_removes_the_workspace_of_its_led_session() {
    parity(removes_the_workspace_of_its_led_session("full"));
}

#[test]
fn the_blanker_leaves_only_what_the_tools_say() {
    let here = "Started a session led by you (agent 0a1b2c3d4e5f): request #4, 12 min.";
    let across = format!(
        "Started a session led by you on server (agent 9f8e7d6c5b4a): request #17, 3 min, of \
         refs/flottille/machines/server/7c1d2e3f4a5b/feat/export by 0a1b2c3d4e5f@{}.",
        "ab".repeat(32)
    );
    assert_eq!(
        blank(here),
        "Started a session led by you (agent <id>): request #0, 0 min."
    );
    assert_eq!(
        blank(&across),
        "Started a session led by you (agent <id>): request #0, 0 min, of feat/export by <id>."
    );
    assert_eq!(
        blank("Last commit in .: 68d1a7c b, then \"machine\": \"server\", \"why\""),
        "Last commit in .: <commit> b, then \"why\""
    );
}
