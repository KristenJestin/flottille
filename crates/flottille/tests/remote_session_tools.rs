//! A session on another machine of the user's is the same as one on this machine, for every
//! tool that addresses a session (`docs/remote.md`, "A tree across machines"; `docs/tools.md`,
//! "Sessions that read each other"): found by its id or its title, read, written to, stopped, with
//! the same rules, judged by the machine it runs on. The only differences are a machine that is
//! off and a device that may only view. A hub served in the test on 127.0.0.1 and two daemons,
//! each with its own temporary HOME and XDG folders and fake agents.

use crate::common;
use crate::remote_deliveries::{
    PC_SECRET, child_on_server, launch_in, off, prompts_with, script, stored_or_prompted,
    with_pc_secret,
};
use crate::remote_launch::{
    Machine, a_while, allowing, done, eventually, id_in, launched, pair, pc_and_server,
    pc_and_server_as, prompted, refused, sent_with, server_user, titled,
};

use common::User;
use common::mcp::{Answer, Mcp};
use flottille_link::{Call, Deliver, LightSession, MachineMessage, SessionList};
use serde_json::{Value, json};

/// The title the user gave the session they started on the server.
const NIGHT: &str = "Night lead of the export";
/// The title of a session the user started on pc, the one-machine counterpart.
const DAY: &str = "Day lead of the export";

/// A session the user starts on `machine` in `app`, idle after its first turn, titled `title`:
/// its id.
fn user_session(machine: &Machine, title: &str) -> String {
    let started = machine
        .user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the session's first turn", || {
        machine.agent(&id)["state"] == "idle"
    });
    machine
        .user
        .call("agent.rename", json!({"agent": id, "title": title}))
        .unwrap();
    id
}

/// `pc` (paired as `pc_role`) and `server` linked, both with the project `app`; on the server a
/// session the user started, titled [`NIGHT`], which pc's daemon lists: its id.
fn night_lead(pc_role: &str) -> (crate::remote_launch::TestHub, Machine, Machine, String) {
    night_lead_of(pc_role, server_user(allowing()))
}

/// [`night_lead`], the server being `server`'s machine.
fn night_lead_of(
    pc_role: &str,
    server: User,
) -> (crate::remote_launch::TestHub, Machine, Machine, String) {
    let (hub, pc, server) = match pc_role {
        "runner" => pc_and_server(server, Some("")),
        role => pc_and_server_as(role, server, Some("")),
    };
    let folder = server.folder();
    server.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    server.git(&folder, &["add", "."]);
    server.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let night = user_session(&server, NIGHT);
    listed_on(&pc, &night);
    (hub, pc, server, night)
}

/// Waits until `machine`'s daemon lists the session `id` of another machine.
fn listed_on(machine: &Machine, id: &str) {
    listed_as(machine, id, NIGHT);
}

/// Waits until `machine`'s daemon lists the session `id` of another machine, titled `title`.
fn listed_as(machine: &Machine, id: &str, title: &str) {
    eventually("the other machine's session listed", || {
        let lists = machine.user.call("remote.sessions", json!({})).unwrap();
        lists["machines"].as_array().unwrap().iter().any(|listed| {
            listed["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|session| session["id"] == id && session["title"] == title)
        })
    });
}

/// The number of the request a held answer names.
fn held(answer: &Answer) -> u64 {
    let text = done(answer);
    let at = text
        .find("request #")
        .unwrap_or_else(|| panic!("not held: {text}"))
        + "request #".len();
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

/// The pending request of `machine`'s user numbered `number`.
fn request(machine: &Machine, number: u64) -> Value {
    machine
        .requests()
        .into_iter()
        .find(|request| request["number"] == number)
        .unwrap_or_else(|| panic!("no request #{number}: {:#?}", machine.requests()))
}

fn approve(machine: &Machine, number: u64) {
    machine
        .user
        .call("approval.approve", json!({"number": number}))
        .unwrap();
}

#[test]
fn an_agent_lists_the_sessions_of_its_project_on_every_machine() {
    let (_hub, pc, _server, night) = night_lead("runner");
    let day = user_session(&pc, DAY);
    let (_root, mcp) = pc.root("auto");

    let answer = mcp.call("session_read", json!({"session": "No such session"}));

    let text = refused(&answer);
    assert!(text.contains("The sessions of this project"), "{text}");
    assert!(text.contains(&format!("{DAY} · {day}")), "{text}");
    assert!(
        text.contains(&format!("{NIGHT} · {night} · on server")),
        "{text}"
    );
}

#[test]
fn an_agent_reads_a_session_the_user_started_on_another_machine_as_on_this_one() {
    let (_hub, pc, server, night) = night_lead("runner");

    // In Full, the read passes without anyone asked, as on one machine.
    let (_root, mcp) = pc.root("full");
    let judged = server.user.judge_received().len();
    let answer = mcp.call("session_read", json!({"session": NIGHT}));
    let text = done(&answer);
    assert!(
        text.contains(&format!(
            "The conversation of the session \"{NIGHT}\" (agent {night})"
        )),
        "{text}"
    );
    assert!(text.contains("hello"), "{text}");
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
    assert_eq!(server.user.judge_received().len(), judged);

    // In Ask, the same ask as on one machine: held for the user of the machine it runs on,
    // answered at once, and the page comes later as a delivery.
    let (_asker, asking) = pc.root("ask");
    let answer = asking.call("session_read", json!({"session": night}));
    let number = held(&answer);
    assert!(
        done(&answer).starts_with("Waiting for the user's approval"),
        "{}",
        answer.text
    );
    let asked = request(&server, number);
    assert_eq!(asked["tool"], "session_read", "{asked:#}");
    approve(&server, number);
    prompted(&pc, "was approved by the user");
    prompted(
        &pc,
        &format!("The conversation of the session \\\"{NIGHT}\\\""),
    );
}

#[test]
fn an_agent_writes_to_it_with_the_same_leave_as_on_one_machine() {
    let (_hub, pc, server, night) = night_lead("runner");
    // Full: the leave is asked all the same, as on one machine.
    let (root, mcp) = pc.root("full");

    let answer = mcp.call(
        "session_send",
        json!({"session": NIGHT, "text": "The CSV export is merged."}),
    );

    let number = held(&answer);
    let asked = request(&server, number);
    let reasons = asked["reasons"].to_string();
    assert!(
        reasons.contains("has not written to that one before"),
        "{asked:#}"
    );
    assert!(
        !server
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("The CSV export is merged.")),
        "sent before the user allowed it"
    );
    approve(&server, number);
    prompted(&server, "The CSV export is merged.");
    let written = server
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains("The CSV export is merged."))
        .unwrap();
    assert!(
        written.contains(&format!("agent {root} of pc")),
        "{written}"
    );
    assert!(written.contains("wrote to this session"), "{written}");
    prompted(&pc, "was approved by the user");

    // Allowed once for the pair: the next one passes without asking.
    let answer = mcp.call(
        "session_send",
        json!({"session": night, "text": "The JSON one too."}),
    );
    assert!(done(&answer).starts_with("Sent to"), "{}", answer.text);
    prompted(&server, "The JSON one too.");
}

#[test]
fn stopping_a_session_it_did_not_start_is_held_for_the_user_as_on_one_machine() {
    let (_hub, pc, server, night) = night_lead("runner");
    // Full: ending another session is the user's all the same, as on one machine.
    let (_root, mcp) = pc.root("full");

    let answer = mcp.call("agent_stop", json!({"agent_id": NIGHT}));

    let number = held(&answer);
    let asked = request(&server, number);
    assert!(
        asked["reasons"]
            .to_string()
            .contains("the user decides, every time"),
        "{asked:#}"
    );
    assert_eq!(server.agent(&night)["state"], "idle");
    approve(&server, number);
    eventually("the session ended", || {
        server.agent(&night)["state"] == "ended"
    });
    prompted(&pc, "was approved by the user");
}

/// A held call of pc's caller, `agent_stop` of the night lead, on the server, which the user has
/// not answered yet: the request's number. The next keeping of a message for pc is played as
/// `waiting` messages already waiting for it (empty: the most, 100).
fn held_stop(
    pc: &Machine,
    server: &Machine,
    night: &str,
    waiting: &str,
) -> (u64, std::path::PathBuf) {
    let (_root, mcp) = pc.root("full");
    let number = held(&mcp.call("agent_stop", json!({"agent_id": NIGHT})));
    assert_eq!(server.agent(night)["state"], "idle");
    let full = server.user.path("kept-full");
    std::fs::write(&full, waiting).unwrap();
    (number, full)
}

#[test]
fn the_user_s_answer_to_a_held_call_of_another_machine_is_kept_past_the_cap() {
    let (_hub, pc, server, night) = night_lead("runner");
    // As many messages wait for pc as may be kept: the answer is kept all the same.
    let (number, full) = held_stop(&pc, &server, &night, "");

    approve(&server, number);

    eventually("the session ended", || {
        server.agent(&night)["state"] == "ended"
    });
    prompted(&pc, "was approved by the user");
    assert!(!full.exists(), "the answer's keeping was played full");
}

#[test]
fn the_user_s_answer_to_a_held_call_past_the_hard_ceiling_is_dropped_with_its_inbox_entry() {
    let (_hub, pc, server, night) = night_lead("runner");
    // As many messages wait for pc as an answer may be kept beyond: the most, and the launches
    // held at once (`MOST_KEPT` and `MOST_HELD`, 100 and 4).
    let (number, full) = held_stop(&pc, &server, &night, "104");

    approve(&server, number);

    eventually("an inbox entry for the answer not kept", || {
        server.user.call("inbox.list", json!({})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                let text = entry["text"].as_str().unwrap_or_default();
                entry["kind"] == "dropped"
                    && text.contains(&format!("request #{number}"))
                    && text.contains("pc")
                    && text.contains("was not sent")
            })
    });
    assert!(!full.exists(), "the answer's keeping was played full");
}

#[test]
fn a_session_of_an_offline_machine_is_listed_with_its_state_and_a_call_says_why() {
    let (_hub, pc, server, night) = night_lead("runner");
    let (_root, mcp) = pc.root("auto");
    off(&server);
    eventually("server offline", || pc.seen("server")["state"] != "online");

    let answer = mcp.call("session_read", json!({"session": NIGHT}));

    let text = refused(&answer);
    assert!(text.contains("server is offline since"), "{text}");
    assert!(text.contains(&night), "{text}");

    let listing = mcp.call("session_send", json!({"session": "Nothing", "text": "Hi."}));
    let text = refused(&listing);
    assert!(
        text.contains(&format!("{NIGHT} · {night} · on server, offline since")),
        "{text}"
    );
}

#[test]
fn a_viewer_device_reads_and_cannot_write() {
    let (_hub, pc, server, night) = night_lead("viewer");
    // In Auto, the server's judge allows the read.
    let (_root, mcp) = pc.root("auto");

    let answer = mcp.call("session_read", json!({"session": NIGHT}));
    assert!(
        done(&answer).contains(&format!("(agent {night})")),
        "{}",
        answer.text
    );

    let answer = mcp.call(
        "session_send",
        json!({"session": NIGHT, "text": "The CSV export is merged."}),
    );
    let text = refused(&answer);
    assert!(text.contains("pc may only view"), "{text}");
    let answer = mcp.call("agent_stop", json!({"agent_id": night}));
    assert!(
        refused(&answer).contains("pc may only view"),
        "{}",
        answer.text
    );
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

/// What an answer says once the ids, the titles, the numbers, the times and the cursor in it are the same
/// for every session: what is left is what the tool says.
fn shape(text: &str, id: &str, title: &str) -> String {
    // A cursor names an event of its machine's store, whose numbers are that machine's own.
    let text = match text.find("\"cursor\": \"") {
        Some(at) => {
            let start = at + "\"cursor\": \"".len();
            let end = start + text[start..].find('"').unwrap_or(0);
            format!("{}<cursor>{}", &text[..start], &text[end..])
        }
        None => text.to_owned(),
    };
    text.replace(id, "<id>")
        .replace(title, "<title>")
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect()
}

#[test]
fn the_session_tools_answer_a_session_on_another_machine_as_one_on_this_machine() {
    same_answers("full");
}

#[test]
fn the_session_tools_answer_a_session_on_another_machine_as_one_on_this_machine_in_ask() {
    same_answers("ask");
}

/// The table of the session tools' calls, by id and by title, against a session on this machine
/// and one on the other, from a caller in `mode`: the same answers.
fn same_answers(mode: &str) {
    let (_hub, pc, _server, night) = night_lead("runner");
    let day = user_session(&pc, DAY);
    let (_root, mcp) = pc.root(mode);
    // Each call of a scenario of one machine, with `{s}` the session it addresses.
    let calls: Vec<(&str, Value)> = vec![
        ("session_read", json!({"session": "{s}", "last": 1})),
        ("session_read", json!({"session": "{s}", "last": 99})),
        (
            "session_read",
            json!({"session": "{s}", "since": "not a date"}),
        ),
        ("session_send", json!({"session": "{s}", "text": "  "})),
        (
            "session_send",
            json!({"session": "{s}", "text": "First words."}),
        ),
        (
            "session_send",
            json!({"session": "{s}", "text": "Second words."}),
        ),
        ("agent_status", json!({"agent_id": "{s}"})),
        ("agent_send", json!({"agent_id": "{s}", "message": "Hi."})),
        ("agent_stop", json!({"agent_id": "{s}"})),
    ];
    let aimed = |args: &Value, session: &str| -> Value {
        serde_json::from_str(&args.to_string().replace("{s}", session)).unwrap()
    };
    for (tool, args) in &calls {
        let names: [fn(&str, &str) -> String; 2] =
            [|id, _| id.to_owned(), |_, title| title.to_owned()];
        for named in names {
            let here = mcp.call(tool, aimed(args, &named(&day, DAY)));
            let there = mcp.call(tool, aimed(args, &named(&night, NIGHT)));
            assert_eq!(
                (here.error, shape(&here.text, &day, DAY)),
                (there.error, shape(&there.text, &night, NIGHT)),
                "{tool} {args}: on this machine\n{}\non the other\n{}",
                here.text,
                there.text
            );
        }
    }
}

/// The key of `machine`, as hex: the machine part of the id its callers are stood in for by.
fn hex_of(machine: &Machine) -> String {
    machine
        .key()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// How many callers of other machines `machine` stands in for, as its store holds them.
fn stand_ins(machine: &Machine) -> i64 {
    let store =
        rusqlite::Connection::open(machine.user.path("state/flottille/flottille.db")).unwrap();
    store
        .query_row(
            "SELECT count(*) FROM agents WHERE id LIKE '%@%'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

/// A child pc's root launched on the server, with `brief`: the hub, the two machines, the
/// child's id there, the MCP session of another root of pc's and of a session the server's user
/// started, both in Full. The child is a session of neither.
fn child_and_others(
    brief: &str,
) -> (
    crate::remote_launch::TestHub,
    Machine,
    Machine,
    String,
    Mcp,
    Mcp,
) {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let folder = server.folder();
    server.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    server.git(&folder, &["add", "."]);
    server.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let (child, _launch, _parent) = child_on_server(&pc, &server, brief);
    let (_other, other) = pc.root("full");
    let (_local, local) = server.root("full");
    (hub, pc, server, child, other, local)
}

/// Whether a request of `machine`'s user holds a call of `tool` (the child's launch may wait
/// for the user there too).
fn asked_for(machine: &Machine, tool: &str) -> bool {
    machine
        .requests()
        .iter()
        .any(|request| request["tool"] == tool)
}

#[test]
fn a_child_launched_from_another_machine_is_not_read_as_a_session() {
    let (_hub, _pc, _server, child, other, local) = child_and_others("Write the tests.");

    for mcp in [&local, &other] {
        let answer = mcp.call("session_read", json!({"session": child}));
        let text = refused(&answer);
        assert!(!text.contains("Write the tests."), "{text}");
    }
    let answer = local.call("session_read", json!({"session": child}));
    assert!(
        refused(&answer).contains("is a child agent"),
        "{}",
        answer.text
    );
}

#[test]
fn a_child_launched_from_another_machine_is_not_written_to_as_a_session() {
    let (_hub, _pc, server, child, other, local) = child_and_others("Write the tests.");

    let answer = local.call(
        "session_send",
        json!({"session": child, "text": "Stop the tests."}),
    );
    assert!(
        refused(&answer).contains("is a child of another session"),
        "{}",
        answer.text
    );
    let answer = other.call(
        "session_send",
        json!({"session": child, "text": "Stop the tests."}),
    );
    refused(&answer);
    a_while();
    assert_eq!(prompts_with(&server, "Stop the tests."), 0);
    assert!(
        !asked_for(&server, "session_send"),
        "{:#?}",
        server.requests()
    );
}

#[test]
fn a_child_launched_from_another_machine_is_not_stopped_as_a_session() {
    let (_hub, _pc, server, child, other, local) = child_and_others("Write the tests.");

    let answer = local.call("agent_stop", json!({"agent_id": child}));
    assert!(
        refused(&answer).contains("is a child of another session"),
        "{}",
        answer.text
    );
    refused(&other.call("agent_stop", json!({"agent_id": child})));
    assert!(
        !asked_for(&server, "agent_stop"),
        "{:#?}",
        server.requests()
    );
    assert_ne!(server.agent(&child)["state"], "ended");
}

#[test]
fn a_secret_of_the_calling_machine_in_the_other_machine_s_answer_comes_back_masked() {
    let (_hub, pc, server, night) = night_lead("runner");
    // A secret only pc knows, in a secret file of the caller's place.
    std::fs::write(pc.folder().join(".env"), "API_TOKEN=pc-only-token-7f3a9c\n").unwrap();
    server
        .user
        .call(
            "agent.send",
            json!({"agent": night, "message": "The token is pc-only-token-7f3a9c."}),
        )
        .unwrap();
    eventually("the session's second turn", || {
        prompts_with(&server, "pc-only-token-7f3a9c") > 0 && server.agent(&night)["state"] == "idle"
    });
    let (_root, mcp) = pc.root("full");

    let answer = mcp.call("session_read", json!({"session": NIGHT}));

    let text = done(&answer);
    assert!(text.contains("The token is"), "{text}");
    assert!(!text.contains("pc-only-token-7f3a9c"), "{text}");
}

#[test]
fn a_caller_whose_project_is_not_on_the_other_machine_reaches_no_folder_there() {
    let (_hub, pc, server, night) = night_lead("runner");
    // pc's project `web`, which the server has not.
    let web = pc.user.path("work/web");
    pc.user.git_init(&web);
    std::fs::write(web.join("README.md"), "web\n").unwrap();
    pc.git(&web, &["add", "."]);
    pc.git(&web, &["commit", "--quiet", "-m", "start"]);
    pc.user.write_project_file(
        "web",
        &format!(
            "folder = \"{}\"\n[[repositories]]\npath = \".\"\nbase = [\"main\"]\n",
            web.display()
        ),
    );
    pc.user.call("project.list", json!({})).unwrap();
    let opened = pc.user.received_requests("session/new").len();
    let started = pc
        .user
        .call("agent.run", json!({"project": "web", "message": "hello"}))
        .unwrap();
    let root = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = common::mcp::server_of(&pc.user, opened);
    eventually("the root's first turn", || {
        pc.agent(&root)["state"] == "idle"
    });
    pc.user
        .call("agent.mode", json!({"agent": root, "mode": "ask"}))
        .unwrap();
    let mcp = Mcp::initialize(&url, &authorization);

    let answer = mcp.call("session_read", json!({"session": night}));

    let number = held(&answer);
    let asked = request(&server, number);
    let stand_in = server.agent(asked["agent"].as_str().unwrap());
    let folder = std::path::PathBuf::from(stand_in["folder"].as_str().unwrap());
    assert!(!folder.exists(), "{}", folder.display());
    assert!(
        !folder.starts_with(server.user.path("data")),
        "{}",
        folder.display()
    );
    // The read of a session of another project is served all the same, once allowed.
    approve(&server, number);
    prompted(
        &pc,
        &format!("The conversation of the session \\\"{NIGHT}\\\""),
    );
}

#[test]
fn a_machine_forgets_the_callers_of_a_machine_removed_from_the_members() {
    let (_hub, pc, server, _night) = night_lead("runner");
    let day = user_session(&pc, DAY);
    listed_as(&server, &day, DAY);
    let (_local, mcp) = server.root("full");
    // Held or served there, the caller is stood in for on pc.
    mcp.call("session_read", json!({"session": day}));
    assert_eq!(stand_ins(&pc), 1);

    let removed = pc.user.run(&["remote", "remove", "server"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );

    eventually("the server's caller forgotten", || stand_ins(&pc) == 0);
}

#[test]
fn a_session_it_started_on_another_machine_named_by_its_title_is_called_as_by_its_id() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("full");
    let started = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export of the report."}),
    );
    let led = id_in(done(&started));
    titled(&server, "Finish the export");
    listed_as(&pc, &led, "Finish the export");
    let idle = || {
        eventually("the led session idle", || {
            server.agent(&led)["state"] == "idle"
        })
    };
    idle();

    let by_id = mcp.call(
        "session_send",
        json!({"session": led, "text": "The CSV first."}),
    );
    prompted(&server, "The CSV first.");
    idle();
    let by_title = mcp.call(
        "session_send",
        json!({"session": "Finish the export", "text": "The CSV first."}),
    );

    assert_eq!(
        (
            by_title.error,
            by_title.text.replace("Finish the export", &led)
        ),
        (by_id.error, by_id.text.replace("Finish the export", &led)),
        "by id\n{}\nby title\n{}",
        by_id.text,
        by_title.text
    );
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
    // The pilot's only, which its led session records there: no caller stood in for by a call.
    assert_eq!(stand_ins(&server), 1);
}

/// The cursor an answer of `session_read` gives for its next page.
fn cursor_in(text: &str) -> String {
    let at = text
        .find("\"cursor\": \"")
        .unwrap_or_else(|| panic!("no cursor in {text}"))
        + "\"cursor\": \"".len();
    text[at..].split('"').next().unwrap().to_owned()
}

/// Waits until the agent `id` of `machine` is idle.
fn idle(machine: &Machine, id: &str) {
    eventually("the agent idle", || machine.agent(id)["state"] == "idle");
}

#[test]
fn the_session_tools_answer_a_session_it_started_on_another_machine_as_on_this_one() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("full");
    // A led session and a child, each started here and on the server: their id and title.
    let started = |kind: &str, machine: Option<&str>, title: &str| -> (String, String) {
        let mut args = json!({"kind": kind, "title": title, "brief": "Finish the export."});
        if let Some(machine) = machine {
            args["machine"] = json!(machine);
        }
        done(&launched(&mcp, args));
        let on = if machine.is_some() { &server } else { &pc };
        let id = titled(on, title)["id"].as_str().unwrap().to_owned();
        idle(on, &id);
        (id, title.to_owned())
    };
    let day = started("led", None, "Day lead");
    let night = started("led", Some("server"), "Night lead");
    let day_child = started("child", None, "Day child");
    let night_child = started("child", Some("server"), "Night child");
    // Each call of a scenario of one machine, with `{s}` the session or child it addresses.
    let session_calls: Vec<(&str, Value)> = vec![
        ("session_read", json!({"session": "{s}", "last": 1})),
        ("session_read", json!({"session": "{s}", "last": 99})),
        ("session_read", json!({"session": "{s}", "last": 0})),
        (
            "session_read",
            json!({"session": "{s}", "since": "not a date"}),
        ),
        (
            "session_read",
            json!({"session": "{s}", "since": "2020-01-01", "until": "2099-01-01"}),
        ),
        // The agent_* tools do not reach a session it leads, here or there.
        ("agent_status", json!({"agent_id": "{s}"})),
    ];
    let child_calls: Vec<(&str, Value)> = vec![
        ("agent_status", json!({"agent_id": "{s}"})),
        (
            "agent_status",
            json!({"agent_id": "{s}", "detail": "short"}),
        ),
        (
            "agent_status",
            json!({"agent_id": "{s}", "action": "conversation"}),
        ),
        (
            "agent_status",
            json!({"agent_id": "{s}", "action": "conversation", "after": 1}),
        ),
        ("session_read", json!({"session": "{s}"})),
    ];
    let aimed = |args: &Value, id: &str| -> Value {
        serde_json::from_str(&args.to_string().replace("{s}", id)).unwrap()
    };
    let same = |tool: &str,
                here: Value,
                there: Value,
                day: &(String, String),
                night: &(String, String)| {
        let here_answer = mcp.call(tool, here.clone());
        let there_answer = mcp.call(tool, there);
        assert_eq!(
            (here_answer.error, shape(&here_answer.text, &day.0, &day.1)),
            (
                there_answer.error,
                shape(&there_answer.text, &night.0, &night.1)
            ),
            "{tool} {here}: on this machine\n{}\non the other\n{}",
            here_answer.text,
            there_answer.text
        );
    };
    for (tool, args) in &session_calls {
        same(
            tool,
            aimed(args, &day.0),
            aimed(args, &night.0),
            &day,
            &night,
        );
    }
    for (tool, args) in &child_calls {
        same(
            tool,
            aimed(args, &day_child.0),
            aimed(args, &night_child.0),
            &day_child,
            &night_child,
        );
    }
    // The next page, by the cursor each machine gave, once each session said more than a page.
    for (machine, id) in [(&pc, &day.0), (&server, &night.0)] {
        machine
            .user
            .call("agent.send", json!({"agent": id, "message": "Go on."}))
            .unwrap();
        eventually("the session's second turn", || {
            prompts_with(machine, "Go on.") > 0 && machine.agent(id)["state"] == "idle"
        });
    }
    let cursor = |id: &str| {
        cursor_in(done(
            &mcp.call("session_read", json!({"session": id, "last": 1})),
        ))
    };
    same(
        "session_read",
        json!({"session": day.0, "cursor": cursor(&day.0), "last": 1}),
        json!({"session": night.0, "cursor": cursor(&night.0), "last": 1}),
        &day,
        &night,
    );
}

#[test]
fn a_read_of_a_session_it_started_elsewhere_comes_back_masked() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    // Said by the led session's agent, across the 600th character of its message: a cut made
    // before the caller's machine masks it would leave the secret's start.
    let said = format!("{} The token is pc-only-token-7f3a9c.", "x".repeat(570));
    script(
        &server,
        json!([{"when": "Say the token.", "do": [{"say": said}]}]),
    );
    let (_root, mcp) = pc.root("full");
    let led = id_in(done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Night lead",
            "brief": "Finish the export."}),
    )));
    idle(&server, &led);
    // A secret only pc knows, in a secret file of the caller's place.
    std::fs::write(pc.folder().join(".env"), "API_TOKEN=pc-only-token-7f3a9c\n").unwrap();
    server
        .user
        .call(
            "agent.send",
            json!({"agent": led, "message": "Say the token."}),
        )
        .unwrap();
    eventually("the session's second turn", || {
        prompts_with(&server, "Say the token.") > 0 && server.agent(&led)["state"] == "idle"
    });

    for (tool, args) in [
        ("session_read", json!({"session": led})),
        ("session_read", json!({"session": led, "last": 1})),
    ] {
        let answer = mcp.call(tool, args);
        let text = done(&answer);
        assert!(text.contains("The token is"), "{text}");
        assert!(!text.contains("pc-only"), "{text}");
    }
}

#[test]
fn a_refusal_of_a_session_it_started_elsewhere_comes_back_masked() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("full");
    let led = id_in(done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Night lead",
            "brief": "Finish the export."}),
    )));
    idle(&server, &led);
    // A secret only pc knows, in a secret file of the caller's place.
    std::fs::write(pc.folder().join(".env"), "API_TOKEN=pc-only-token-7f3a9c\n").unwrap();

    // The server's refusal says the date it was given back.
    let answer = mcp.call(
        "session_read",
        json!({"session": led, "since": "pc-only-token-7f3a9c"}),
    );

    let text = refused(&answer);
    assert!(text.contains("is not a date"), "{text}");
    assert!(!text.contains("pc-only"), "{text}");
}

#[test]
fn a_refusal_of_an_answer_to_a_request_held_elsewhere_comes_back_masked() {
    let (_hub, pc, _server, _root, mcp, _child, _launch) = started_on_server("child");
    // A secret only pc knows, in a secret file of the caller's place: a number, which the
    // server's refusal says back as the request it does not hold.
    std::fs::write(pc.folder().join(".env"), "API_PIN=73519246\n").unwrap();

    let answer = mcp.call(
        "approval_answer",
        json!({"request_id": 73519246, "machine": "server", "decision": "deny",
            "why": "Not needed."}),
    );

    let text = refused(&answer);
    assert!(text.contains("request #[masked] is not held"), "{text}");
    assert!(!text.contains("735192"), "{text}");
}

/// A session or child pc's root started on the server, in Full: the hub, the two machines, the
/// root's id, its MCP session, and the id and launch of what it started.
fn started_on_server(
    kind: &str,
) -> (
    crate::remote_launch::TestHub,
    Machine,
    Machine,
    String,
    Mcp,
    String,
    String,
) {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("full");
    let text = done(&launched(
        &mcp,
        json!({"kind": kind, "machine": "server", "title": "Night lead",
            "brief": "Finish the export."}),
    ))
    .to_owned();
    let id = id_in(&text).to_owned();
    idle(&server, &id);
    (hub, pc, server, root, mcp, id, launch_in(&text))
}

/// A read of `caller` of pc of what it started on the server as `launch`, `led`, with `args`,
/// from a daemon of pc sending `project`.
fn own_read(
    id: &str,
    caller: &str,
    launch: &str,
    led: &str,
    args: Value,
    project: &str,
) -> MachineMessage {
    MachineMessage::Call(Box::new(Call {
        id: id.into(),
        launch: launch.into(),
        caller: caller.into(),
        agent: led.into(),
        tool: "session_read".into(),
        arguments: args.to_string(),
        project: project.into(),
        mode: "auto".into(),
        ..Call::default()
    }))
}

#[test]
fn a_read_of_a_session_it_started_from_an_older_daemon_is_refused_saying_to_update_it() {
    let (_hub, pc, server, root, _mcp, led, launch) = started_on_server("led");
    let key = pc.stand_in();

    // An older daemon of pc names no project with a read of what its agent started.
    let answers = sent_with(
        &key,
        &server,
        vec![own_read(
            "call-r1",
            &root,
            &launch,
            &led,
            json!({"session": led}),
            "",
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("daemon of pc"), "{refused}");
    assert!(refused.contains("needs updating"), "{refused}");
}

#[test]
fn a_read_naming_its_launch_and_another_session_is_judged_as_by_a_caller_that_started_neither() {
    let (_hub, pc, server, root, _mcp, led, launch) = started_on_server("led");
    let other = user_session(&server, NIGHT);
    let key = pc.stand_in();

    // From another project: a session it did not start asks the user's leave, the one it
    // started does not.
    let answers = sent_with(
        &key,
        &server,
        vec![
            own_read(
                "call-r2",
                &root,
                &launch,
                &led,
                json!({"session": led}),
                "elsewhere",
            ),
            own_read(
                "call-r3",
                &root,
                &launch,
                &led,
                json!({"session": other}),
                "elsewhere",
            ),
        ],
    );

    let ok = answers[0].clone().unwrap();
    assert_eq!(ok["outcome"], "ok", "{ok:#}");
    let asked = answers[1].clone().unwrap();
    assert_eq!(asked["outcome"], "held", "{asked:#}");
    let number = asked["request"].as_u64().unwrap();
    let request = request(&server, number);
    assert_eq!(request["tool"], "session_read", "{request:#}");
    assert!(
        request["reasons"]
            .to_string()
            .contains("has not reached that one, of another project, before"),
        "{request:#}"
    );
}

/// A call of `caller` of pc to the session `night`, as pc's daemon would send it, its tree `tree`
/// in `mode`.
fn forged_call(caller: &str, night: &str, tree: &str, mode: &str) -> MachineMessage {
    MachineMessage::Call(Box::new(Call {
        id: format!("call-{}", caller.replace('@', "-")),
        launch: String::new(),
        caller: caller.into(),
        agent: night.into(),
        tool: "session_read".into(),
        arguments: json!({"session": night}).to_string(),
        project: "app".into(),
        title: String::new(),
        tree: tree.into(),
        mode: mode.into(),
        switch: None,
    }))
}

#[test]
fn a_call_of_a_tree_in_full_is_taken_in_full_with_no_switch_signed() {
    let ahead = tempfile::NamedTempFile::new().unwrap();
    let user = server_user(allowing()).with_env(
        "FLOTTILLE_TEST_WALL_AHEAD_FILE",
        ahead.path().to_str().unwrap(),
    );
    let (_hub, pc, server, night) = night_lead_of("runner", user);
    let (root, _mcp) = pc.root("full");
    let pc_hex = hex_of(&pc);
    let key = pc.stand_in();
    let judged = server.user.judge_received().len();

    let answers = sent_with(
        &key,
        &server,
        vec![
            forged_call("infull", &night, &root, "full"),
            forged_call("inauto", &night, &root, "auto"),
            forged_call("evil@thing", &night, "", "auto"),
        ],
    );

    for answer in &answers[..2] {
        assert!(answer.is_ok(), "{answer:?}");
    }
    let refused = answers[2].clone().unwrap_err();
    assert!(refused.contains("short words"), "{refused}");
    let mode = |caller: &str| server.agent(&format!("{caller}@{pc_hex}"))["gate_mode"].clone();
    // A runner is the user: the mode of its tree is taken as it is, as one of this machine.
    assert_eq!(mode("infull"), "full");
    assert_eq!(mode("inauto"), "auto");
    let judged_then = server.user.judge_received().len();
    assert!(judged_then > judged, "the judge was not asked in Auto");

    // Thirteen hours later on the server's clock, Full still follows the tree.
    std::fs::write(ahead.path(), (13 * 60 * 60).to_string()).unwrap();
    let answers = sent_with(
        &key,
        &server,
        vec![forged_call("later", &night, &root, "full")],
    );
    assert!(answers[0].is_ok(), "{:?}", answers[0]);
    assert_eq!(mode("later"), "full");

    // None of them is an agent of the server's, nor a session it lists.
    assert!(
        server
            .agents()
            .iter()
            .all(|agent| !agent["id"].as_str().unwrap().contains('@')),
        "{:#?}",
        server.agents()
    );
    let lists = server.user.call("remote.sessions", json!({})).unwrap();
    assert!(
        !lists.to_string().contains(&format!("@{pc_hex}")),
        "{lists:#}"
    );
}

#[test]
fn a_controller_s_call_in_full_is_taken_in_auto() {
    let (_hub, pc, server, night) = night_lead("controller");
    let (root, _mcp) = pc.root("full");
    let pc_hex = hex_of(&pc);
    let key = pc.stand_in();

    // A call that says its tree is in Full, as a forged one would.
    let answers = sent_with(
        &key,
        &server,
        vec![forged_call("forged", &night, &root, "full")],
    );

    // A controller stays bounded by its role: never taken in Full, judged as Auto is.
    assert!(answers[0].is_ok(), "{:?}", answers[0]);
    let stand_in = server.agent(&format!("forged@{pc_hex}"));
    assert_eq!(stand_in["gate_mode"], "auto", "{stand_in:#}");
}

/// The answer of request `number` of another machine, for pc's agent `agent`.
fn answer_of(id: &str, number: u64, agent: &str, body: &str) -> MachineMessage {
    MachineMessage::Deliver(Box::new(Deliver {
        id: id.into(),
        launch: format!("request-{number}"),
        agent: agent.into(),
        kind: "approval".into(),
        body: body.into(),
        urgency: "urgent".into(),
        ended: false,
    }))
}

#[test]
fn an_answer_of_another_machine_is_taken_only_for_the_request_awaited_from_it_and_once() {
    let (hub, pc, server, night) = night_lead("runner");
    let laptop = Machine::of(server_user(None));
    pair(&hub, &laptop, "laptop", "runner");
    pc.wait_for("laptop");
    let (asker, asking) = pc.root("ask");
    let number = held(&asking.call("session_read", json!({"session": night})));

    // From another machine than the one that holds it.
    let laptop_key = laptop.stand_in();
    let answers = sent_with(
        &laptop_key,
        &pc,
        vec![answer_of(
            &format!("approval-{number}"),
            number,
            &asker,
            "From the laptop.",
        )],
    );
    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("waits for no answer"), "{refused}");

    approve(&server, number);
    prompted(&pc, "was approved by the user");

    // Sent again, or another answer of it, or one of a request never awaited.
    let server_key = server.stand_in();
    let answers = sent_with(
        &server_key,
        &pc,
        vec![
            answer_of(&format!("approval-{number}"), number, &asker, "Replayed."),
            answer_of("approval-again", number, &asker, "Replayed again."),
            answer_of("approval-never", number + 50, &asker, "Never asked."),
        ],
    );
    assert!(answers[0].is_ok(), "{:?}", answers[0]);
    for answer in &answers[1..] {
        let refused = answer.clone().unwrap_err();
        assert!(refused.contains("waits for no answer"), "{refused}");
    }
    a_while();
    for body in ["From the laptop.", "Replayed", "Never asked."] {
        assert_eq!(prompts_with(&pc, body), 0, "{body}");
    }
}

#[test]
fn an_approval_answer_from_another_machine_is_masked_with_the_caller_s_env() {
    let (_hub, pc, server, night) = night_lead("runner");
    let (asker, asking) = pc.root("ask");
    let number = held(&asking.call("session_read", json!({"session": night})));
    with_pc_secret(&pc);

    let key = server.stand_in();
    let answers = sent_with(
        &key,
        &pc,
        vec![answer_of(
            &format!("approval-{number}"),
            number,
            &asker,
            &format!("Approved, sign with {PC_SECRET} there."),
        )],
    );

    assert!(answers[0].is_ok(), "{:?}", answers[0]);
    prompted(&pc, "Approved, sign with [masked] there.");
    assert_eq!(stored_or_prompted(&pc, PC_SECRET), 0, "{:#?}", pc.prompts());
}

#[test]
fn a_session_id_two_machines_list_is_refused_naming_both() {
    let (hub, pc, server, night) = night_lead("runner");
    let laptop = Machine::of(server_user(None));
    pair(&hub, &laptop, "laptop", "runner");
    pc.wait_for("laptop");
    let key = laptop.stand_in();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let answers = sent_with(
        &key,
        &pc,
        vec![MachineMessage::Sessions(SessionList {
            sessions: vec![LightSession {
                id: night.clone(),
                title: "Impostor".into(),
                project: "app".into(),
                state: "idle".into(),
                exchanged_at: now,
                created_at: now,
                pilot: None,
            }],
        })],
    );
    assert!(answers[0].is_ok(), "{:?}", answers[0]);
    listed_as(&pc, &night, "Impostor");
    let (_root, mcp) = pc.root("auto");
    let judged = server.user.judge_received().len();

    let answer = mcp.call("session_read", json!({"session": night}));

    let text = refused(&answer);
    assert!(text.contains("server") && text.contains("laptop"), "{text}");
    assert_eq!(server.user.judge_received().len(), judged);
    assert_eq!(stand_ins(&server), 0);
}

#[test]
fn an_exception_asked_on_a_call_that_another_machine_answers_is_refused_with_why() {
    let (_hub, pc, server, night) = night_lead("runner");
    let (_root, mcp) = pc.root("ask");

    // Exceptions are each machine's own: the agent is told so, and nothing reaches the server.
    let answer = mcp.call(
        "agent_stop",
        json!({"agent_id": NIGHT, "exception": "it is stuck on a check"}),
    );

    assert!(answer.error, "not refused: {}", answer.text);
    assert!(
        answer.text.contains("exception") && answer.text.contains("machine"),
        "{}",
        answer.text
    );
    a_while();
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
    assert_eq!(server.agent(&night)["state"], "idle");
}

#[test]
fn a_stand_in_is_not_a_session_this_machine_s_agents_can_reach() {
    let (_hub, pc, server, night) = night_lead("runner");
    let (root, mcp) = pc.root("full");
    // The call stands pc's root in on the server.
    done(&mcp.call("session_read", json!({"session": night})));
    assert_eq!(stand_ins(&server), 1);
    let stand_in = format!("{root}@{}", hex_of(&pc));
    let (_local, local) = server.root("full");
    let opened = server.user.received_requests("session/new").len();

    let read = local.call("session_read", json!({"session": stand_in}));
    assert!(
        refused(&read).contains("there is no session"),
        "{}",
        read.text
    );
    let sent = local.call(
        "session_send",
        json!({"session": stand_in, "text": "Are you there?"}),
    );
    assert!(refused(&sent).contains("names no session"), "{}", sent.text);
    let stopped = local.call("agent_stop", json!({"agent_id": stand_in}));
    assert!(
        refused(&stopped).contains("names no agent"),
        "{}",
        stopped.text
    );

    // Nothing was asked of the user, and nothing was started for it.
    a_while();
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
    assert_eq!(server.user.received_requests("session/new").len(), opened);
    assert_eq!(server.agent(&stand_in)["state"], "idle");
}

#[test]
fn an_unreadable_member_list_keeps_the_stand_ins() {
    let (hub, pc, server, _night) = night_lead("runner");
    let day = user_session(&pc, DAY);
    listed_as(&server, &day, DAY);
    let (_local, mcp) = server.root("full");
    done(&mcp.call("session_read", json!({"session": day})));
    assert_eq!(stand_ins(&pc), 1);

    // pc starts again with a member list it cannot read, and no hub to give it again: it knows
    // no machine, which is not to say that the server left the members.
    drop(hub);
    let stopped = pc.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", pc.log());
    std::fs::write(
        pc.user.path("state/flottille/remote/peers.json"),
        r#"{"machines": [{"name": "ser"#,
    )
    .unwrap();
    pc.user.status();

    // The sweep runs every five seconds.
    std::thread::sleep(std::time::Duration::from_secs(12));
    assert!(
        pc.log().contains("peers.json is not read"),
        "the file was not read: {}",
        pc.log()
    );
    assert_eq!(stand_ins(&pc), 1, "{}", pc.log());
}

#[test]
fn a_stand_in_never_relays_a_call_to_a_third_machine() {
    let (hub, pc, server, _night) = night_lead("runner");
    let laptop = Machine::of(server_user(None));
    pair(&hub, &laptop, "laptop", "runner");
    laptop.register("");
    let folder = laptop.folder();
    laptop.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    laptop.git(&folder, &["add", "."]);
    laptop.git(&folder, &["commit", "--quiet", "-m", "start"]);
    server.wait_for("laptop");
    let far = user_session(&laptop, "Laptop lead");
    listed_as(&server, &far, "Laptop lead");
    // pc's daemon is stopped: the test plays it, as it would send the call.
    let key = pc.stand_in();

    let answers = sent_with(&key, &server, vec![forged_call("c0", &far, "c0", "auto")]);

    let answer = answers[0].clone().unwrap();
    let text = answer["text"].as_str().unwrap_or_default();
    assert_eq!(answer["outcome"], "refused", "{answer:#}");
    assert!(
        text.contains("reaches only the sessions of this machine"),
        "{text}"
    );
    a_while();
    // Nothing was sent on to the laptop, which would have stood nobody in for it.
    assert_eq!(stand_ins(&laptop), 0);
    assert!(
        !laptop.log().contains("from another machine was refused"),
        "{}",
        laptop.log()
    );
}

#[test]
fn an_answer_before_its_wait_is_recorded_is_sent_again() {
    let (_hub, pc, server, night) = night_lead("runner");
    let (_asker, asking) = pc.root("ask");
    // pc takes its time to record what its agent waits for, once the server answered the call.
    let slow = pc.user.path("slow-call");
    std::fs::write(&slow, "").unwrap();

    std::thread::scope(|scope| {
        let call = scope.spawn(|| asking.call("session_read", json!({"session": night})));
        eventually("the call held on the server", || {
            !server.requests().is_empty()
        });
        let number = server.requests()[0]["number"].as_u64().unwrap();
        // The user answers before pc recorded the wait: not never, later.
        approve(&server, number);
        eventually("the answer taken later", || {
            pc.log().contains("waits for no answer of request")
                && pc.log().contains("is taken later")
        });
        std::fs::remove_file(&slow).unwrap();
        assert_eq!(held(&call.join().unwrap()), number);
    });

    prompted(&pc, "was approved by the user");
}

#[test]
fn an_answer_nothing_awaits_holds_back_nothing_kept_behind_it() {
    let (_hub, pc, server, night) = night_lead("runner");
    let (_waiting, waits) = pc.root("ask");
    let awaited = held(&waits.call("session_read", json!({"session": night})));
    // Another call is held there, and pc stops before it records the wait, as when the call's
    // answer is lost with the link: nothing will ever await that answer.
    let (_gone, gone) = pc.root("ask");
    let slow = pc.user.path("slow-call");
    std::fs::write(&slow, "").unwrap();
    std::thread::scope(|scope| {
        let call = scope.spawn(|| gone.call("session_read", json!({"session": night})));
        eventually("the second call held on the server", || {
            server.requests().len() == 2
        });
        a_while();
        let stopped = pc.user.run(&["daemon", "stop"]);
        assert!(stopped.status.success(), "{}", pc.log());
        std::fs::remove_file(&slow).unwrap();
        let _ = call.join();
    });
    let never = server
        .requests()
        .into_iter()
        .map(|request| request["number"].as_u64().unwrap())
        .find(|number| *number != awaited)
        .unwrap();

    // Kept for pc in this order: the answer nothing awaits, then the one awaited.
    approve(&server, never);
    approve(&server, awaited);
    pc.user.status();

    eventually("the answer nothing awaits dropped", || {
        server
            .log()
            .contains("refused a message kept for it, which is dropped")
    });
    prompted(&pc, "was approved by the user");
    assert_eq!(prompts_with(&pc, "was approved by the user"), 1);
}
