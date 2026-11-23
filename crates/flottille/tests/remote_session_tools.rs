//! A session on another machine of the user's is the same as one on this machine, for every
//! tool that addresses a session (`docs/remote.md`, "A tree across machines"; `docs/tools.md`,
//! "Sessions that read each other"): found by its id or its title, read, written to, stopped, with
//! the same rules, judged by the machine it runs on. The only differences are a machine that is
//! off and a device that may only view. A hub served in the test on 127.0.0.1 and two daemons,
//! each with its own temporary HOME and XDG folders and fake agents.

use crate::common;
use crate::remote_deliveries::off;
use crate::remote_launch::{
    Machine, allowing, done, eventually, pc_and_server, pc_and_server_as, prompted, refused,
    server_user,
};

use common::mcp::{Answer, Mcp};
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
    let (hub, pc, server) = match pc_role {
        "runner" => pc_and_server(server_user(allowing()), Some("")),
        role => pc_and_server_as(role, server_user(allowing()), Some("")),
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
    eventually("the other machine's session listed", || {
        let lists = machine.user.call("remote.sessions", json!({})).unwrap();
        lists["machines"].as_array().unwrap().iter().any(|listed| {
            listed["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|session| session["id"] == id && session["title"] == NIGHT)
        })
    });
}

/// The root of pc's, in `mode`, switched by the user from a device when it is Full (a machine
/// alone never raises another machine's calls to Full).
fn caller(pc: &Machine, server: &Machine, mode: &str) -> (String, Mcp) {
    let (root, mcp) = pc.root(if mode == "full" { "auto" } else { mode });
    if mode == "full" {
        let switched = server
            .user
            .run(&["--machine", "pc", "agent", "mode", &root, "full"]);
        assert!(
            switched.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&switched.stderr),
            pc.log()
        );
    }
    (root, mcp)
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
    let (_hub, pc, server, night) = night_lead("runner");
    let day = user_session(&pc, DAY);
    let (_root, mcp) = caller(&pc, &server, "auto");

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
    let (_root, mcp) = caller(&pc, &server, "full");
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
    let (_asker, asking) = caller(&pc, &server, "ask");
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
    let (root, mcp) = caller(&pc, &server, "full");

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
    let (_root, mcp) = caller(&pc, &server, "full");

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

#[test]
fn a_session_of_an_offline_machine_is_listed_with_its_state_and_a_call_says_why() {
    let (_hub, pc, server, night) = night_lead("runner");
    let (_root, mcp) = caller(&pc, &server, "auto");
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
    let (_root, mcp) = caller(&pc, &server, "auto");

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

/// What an answer says once the ids, the titles, the numbers and the times in it are the same
/// for every session: what is left is what the tool says.
fn shape(text: &str, id: &str, title: &str) -> String {
    text.replace(id, "<id>")
        .replace(title, "<title>")
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect()
}

#[test]
fn the_session_tools_answer_a_session_on_another_machine_as_one_on_this_machine() {
    let (_hub, pc, server, night) = night_lead("runner");
    let day = user_session(&pc, DAY);
    let (_root, mcp) = caller(&pc, &server, "full");
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
