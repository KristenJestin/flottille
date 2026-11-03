//! Reading a conversation whose messages arrive in pieces (`docs/agents.md` "The conversation"):
//! the pieces of one message are one line, a secret split over several of them is masked, a long
//! message goes on over the next lines and pages, and two messages are never glued. Against the
//! real daemon with the fake ACP agent and the fake `codex`, throwaway repositories and temporary
//! XDG folders. No real agent ever runs here.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::Mcp;
use serde_json::{Value, json};

/// A secret the daemon knows through its environment, which no single piece holds whole.
const SECRET: &str = "s3cr3t-token-value";

/// A project `app` over a throwaway repository with one commit.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// A user whose agents are the fake ACP agent playing `turns`, on a project `app`.
fn user(turns: Value) -> User {
    let user = User::with_fake_agent().with_env("API_TOKEN", SECRET);
    user.fake_script(json!({"turns": turns}));
    project(&user);
    user
}

/// A user whose agents are Codex's, through its bridge, in front of the fake `codex`.
fn user_with_codex(turns: Value) -> User {
    let user = User::with_fake_codex().with_env("API_TOKEN", SECRET);
    user.write_providers("[claude]\nenabled = false\n[codex]\nenabled = true\n");
    user.codex_script(json!({"turns": turns}));
    project(&user);
    user
}

/// Starts an agent with a first message: its id and the event number to follow it from.
fn run(user: &User, message: &str) -> (String, u64) {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits for the end of the agent's `turns`-th turn.
fn turn_ended(user: &User, agent: &str, after: u64, turns: usize) {
    let of = |event: &Value| event["data"]["agent"] == agent;
    let seen = std::cell::Cell::new(0);
    user.watch(&["--after", &after.to_string()]).until(|e| {
        seen.set(seen.get() + usize::from(of(e) && e["name"] == "agent.turn_ended"));
        seen.get() == turns
    });
}

/// A page of the conversation, from `after`.
fn read(user: &User, agent: &str, after: u64) -> Value {
    user.call("agent.read", json!({"agent": agent, "after": after}))
        .unwrap()
}

fn lines(page: &Value) -> Vec<String> {
    page["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_owned())
        .collect()
}

/// What the agent said on each line, in order.
fn said(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| Some(line.split_once(" said: ")?.1.to_owned()))
        .collect()
}

/// The number an agent's line starts with.
fn number(line: &str) -> u64 {
    line.strip_prefix('#')
        .and_then(|rest| rest.split(' ').next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("no event number in {line}"))
}

/// The pieces of a reply that tell the secret in three of them.
fn secret_in_pieces() -> Value {
    json!([
        {"say": "the token is s3cr3t-"},
        {"say": "token-"},
        {"say": "value, keep it quiet"}
    ])
}

#[test]
fn a_secret_split_over_several_pieces_reads_masked_in_a_conversation_page() {
    let user = user(json!([{"when": "go", "do": secret_in_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let page = read(&user, &id, 0);

    let text = lines(&page).join("\n");
    assert!(!text.contains(SECRET), "{text}");
    assert_eq!(
        said(&lines(&page)),
        ["the token is [masked], keep it quiet"],
        "{text}"
    );
}

#[test]
fn a_secret_split_over_several_pieces_reads_masked_in_the_pilot_door_s_agent_read_and_agent_status()
{
    let user = user(json!([{"when": "go", "do": secret_in_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);
    let pilot = Mcp::pilot(&user);

    let read = pilot.call("agent_read", json!({"agent_id": id}));
    let status = pilot.call("agent_status", json!({"agent_id": id}));

    for answer in [&read, &status] {
        assert!(!answer.error, "{}", answer.text);
        assert!(!answer.text.contains(SECRET), "{}", answer.text);
        assert!(
            answer.text.contains("the token is [masked], keep it quiet"),
            "{}",
            answer.text
        );
    }
}

#[test]
fn a_secret_split_over_several_pieces_reads_masked_in_the_last_messages_of_agent_status() {
    let user = user(json!([{"when": "go", "do": secret_in_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let status = user.call("agent.status", json!({"agent": id})).unwrap();

    let messages = status["messages"].as_array().unwrap();
    assert_eq!(
        messages,
        &[json!("the token is [masked], keep it quiet")],
        "{status}"
    );
}

#[test]
fn a_secret_streamed_by_codex_in_several_deltas_reads_masked() {
    let user = user_with_codex(json!([{"when": "go", "do": [
        {"say": ["the token is s3cr3t-", "token-", "value, keep it quiet"]}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let page = read(&user, &id, 0);
    let status = user.call("agent.status", json!({"agent": id})).unwrap();

    let text = format!("{}\n{status}", lines(&page).join("\n"));
    assert!(!text.contains(SECRET), "{text}");
    assert_eq!(
        said(&lines(&page)),
        ["the token is [masked], keep it quiet"],
        "{text}"
    );
    assert_eq!(
        status["messages"],
        json!(["the token is [masked], keep it quiet"])
    );
}

/// `count` pieces of `size` characters, each led by its own number, as `say` steps.
fn numbered_pieces(count: usize, size: usize) -> (Vec<Value>, String) {
    let pieces: Vec<String> = (0..count)
        .map(|n| format!("{n:03}{}", "x".repeat(size - 3)))
        .collect();
    let steps = pieces.iter().map(|piece| json!({"say": piece})).collect();
    (steps, pieces.concat())
}

#[test]
fn a_reply_over_2000_characters_goes_on_whole_on_the_next_lines() {
    let (steps, whole) = numbered_pieces(6, 500);
    let user = user(json!([{"when": "go", "do": steps}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let page = read(&user, &id, 0);

    let said = said(&lines(&page));
    assert_eq!(said.len(), 2, "{page}");
    assert!(said.iter().all(|line| line.chars().count() <= 2_000));
    assert_eq!(said.concat(), whole);
    assert!(!said.concat().contains('‹'));
}

#[test]
fn a_page_that_fills_up_in_the_middle_of_a_joined_reply_goes_on_where_the_line_stopped() {
    // 40 pieces of 1,000 characters: 20 lines of 2,000, over one page of 30,000.
    let (steps, whole) = numbered_pieces(40, 1_000);
    let user = user(json!([{"when": "go", "do": steps}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let first = read(&user, &id, 0);
    assert_eq!(first["more"], true, "the page should be full");
    let after = first["after"].as_u64().unwrap();
    let second = read(&user, &id, after);

    let (first_lines, second_lines) = (lines(&first), lines(&second));
    assert!(first_lines.len() < 20, "{}", first_lines.len());
    // The next page starts at the piece after the last one the first covered: no piece is lost
    // or read twice.
    assert_eq!(number(&second_lines[0]), after + 1, "{second_lines:?}");
    let all = [said(&first_lines), said(&second_lines)].concat();
    assert_eq!(all.concat(), whole);
    assert_eq!(second["more"], false);
}

#[test]
fn a_reply_still_streaming_when_the_events_run_out_is_read_as_far_as_it_is_settled() {
    // Both long enough for a part of each to settle: two pieces stored.
    let (first, then) = ("this reply is long ".repeat(10), "and goes on ".repeat(20));
    let user = user(json!([{"when": "go", "do": [
        {"say": &first}, {"say": format!("{then}still going")}, {"hang": true}
    ]}]));
    let (id, _) = run(&user, "go");
    let whole = format!("{first}{then}still going");
    let mut page = Value::Null;
    // Held back: the last word, and as much as the longest secret the daemon knows.
    eventually("the settled part of both pieces", || {
        page = read(&user, &id, 0);
        let said = said(&lines(&page));
        said.len() == 1 && said[0].len() > first.len()
    });
    let said = said(&lines(&page));
    assert!(
        whole.starts_with(&said[0]) && said[0].ends_with(' '),
        "{said:?}"
    );
    assert!(!said[0].contains("still"), "{said:?}");

    assert_eq!(page["more"], false);
    let last = number(lines(&page).last().unwrap());
    // The piece that comes next is read from where this page stopped.
    assert!(page["after"].as_u64().unwrap() > last);
}

#[test]
fn joining_stops_at_a_tool_call() {
    let user = user(json!([{"when": "go", "do": [
        {"say": "before "}, {"say": "the call"},
        {"mcp": {"tool": "fs_list", "arguments": {}}},
        {"say": "after "}, {"say": "the call"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let said = said(&lines(&read(&user, &id, 0)));

    assert_eq!(said, ["before the call", "after the call"]);
}

#[test]
fn joining_stops_at_the_end_of_a_turn() {
    let user = user(json!([
        {"when": "one", "do": [{"say": "first "}, {"say": "reply"}]},
        {"when": "two", "do": [{"say": "second "}, {"say": "reply"}]}
    ]));
    let (id, after) = run(&user, "one");
    turn_ended(&user, &id, after, 1);
    user.call("agent.send", json!({"agent": id, "message": "two"}))
        .unwrap();
    turn_ended(&user, &id, after, 2);

    let lines = lines(&read(&user, &id, 0));

    assert_eq!(said(&lines), ["first reply", "second reply"], "{lines:?}");
}

#[test]
fn joining_stops_across_events_that_give_no_line() {
    let user = user(json!([{"when": "go", "do": [
        {"say": "a"}, {"think": "hmm"}, {"say": "b"},
        {"update": {"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed"}},
        {"say": "c"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let lines = lines(&read(&user, &id, 0));

    // A thought and a tool-call update give no line, and the pieces on both sides stay apart.
    assert_eq!(said(&lines), ["a", "b", "c"], "{lines:?}");
    assert_eq!(lines.iter().filter(|l| l.contains(" said: ")).count(), 3);
}

#[test]
fn two_messages_the_agent_marks_apart_are_two_lines() {
    let user = user(json!([{"when": "go", "do": [
        {"say": "I'll ask ", "message": "m1"}, {"say": "you.", "message": "m1"},
        {"say": "Which ", "message": "m2"}, {"say": "DB?", "message": "m2"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let page = read(&user, &id, 0);

    assert_eq!(
        said(&lines(&page)),
        ["I'll ask you.", "Which DB?"],
        "{page}"
    );
    // The stream stays piece by piece, and the last messages are whole and apart.
    let status = user.call("agent.status", json!({"agent": id})).unwrap();
    assert_eq!(status["messages"], json!(["I'll ask you.", "Which DB?"]));
}

#[test]
fn pieces_with_no_message_mark_keep_joining_as_one_message() {
    let user = user(json!([{"when": "go", "do": [
        {"say": "Claude "}, {"say": "streams "}, {"say": "words"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let page = read(&user, &id, 0);

    assert_eq!(said(&lines(&page)), ["Claude streams words"], "{page}");
}

#[test]
fn two_replies_codex_gives_in_a_row_are_two_lines() {
    let user = user_with_codex(json!([{"when": "go", "do": [
        {"say": ["I'll ask ", "you."]},
        {"say": ["Which ", "DB?"]}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let page = read(&user, &id, 0);

    assert_eq!(
        said(&lines(&page)),
        ["I'll ask you.", "Which DB?"],
        "{page}"
    );
}

#[test]
fn a_question_codex_asks_whole_after_a_reply_is_not_glued_to_it() {
    let user = user_with_codex(json!([{"when": "go", "do": [
        {"say": ["I'll ", "ask you."]},
        {"completed": {"type": "agentMessage", "id": "question-1", "text": "Which DB?"}}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let page = read(&user, &id, 0);
    let status = user.call("agent.status", json!({"agent": id})).unwrap();

    assert_eq!(
        said(&lines(&page)),
        ["I'll ask you.", "Which DB?"],
        "{page}"
    );
    assert_eq!(status["messages"], json!(["I'll ask you.", "Which DB?"]));
}

#[test]
fn a_single_piece_over_2000_characters_goes_on_whole_on_the_next_line() {
    let user = user(json!([{"when": "go", "do": [{"say": "q".repeat(2_500)}]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let said = said(&lines(&read(&user, &id, 0)));

    assert_eq!(said, ["q".repeat(2_000), "q".repeat(500)]);
}

// A message is masked whole before it is stored or shown (#63): every reader gets masked text.

/// What the agent said, its pieces as `watch` streams them, joined.
fn watched_reply(user: &User, agent: &str, after: u64) -> String {
    let events = user
        .watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == agent && e["name"] == "agent.turn_ended");
    events
        .iter()
        .filter(|e| e["data"]["agent"] == agent && e["name"] == "agent.message")
        .map(|e| e["data"]["text"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_usage_update_between_two_pieces_that_split_a_secret_does_not_end_the_reply() {
    let user = user(json!([{"when": "go", "do": [
        {"say": "the token is s3cr3t-"},
        {"usage": {"used": 1000, "size": 200000}},
        {"say": "token-value, keep it quiet"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let lines = lines(&read(&user, &id, 0));

    let text = lines.join("\n");
    assert!(
        !text.contains("s3cr3t-"),
        "{}",
        text.replace("s3cr3t-", "<clear>")
    );
    assert_eq!(said(&lines), ["the token is [masked], keep it quiet"]);
}

#[test]
fn a_token_streamed_over_three_pieces_never_shows_in_clear_in_watch() {
    let user = user(json!([{"when": "go", "do": secret_in_pieces()}]));
    let (id, after) = run(&user, "go");

    let reply = watched_reply(&user, &id, after);

    assert!(
        !reply.contains(SECRET),
        "{}",
        reply.replace(SECRET, "<clear>")
    );
    assert_eq!(reply, "the token is [masked], keep it quiet");
}

#[test]
fn a_token_streamed_over_three_pieces_never_shows_in_clear_in_agent_show_json() {
    let user = user(json!([{"when": "go", "do": secret_in_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let output = user.run(&["agent", "show", &id, "--json"]);
    assert!(output.status.success());
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();

    let text = shown.to_string();
    assert!(
        !text.contains(SECRET),
        "{}",
        text.replace(SECRET, "<clear>")
    );
    let reply: String = shown["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "agent.message")
        .map(|e| e["data"]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(reply, "the token is [masked], keep it quiet");
}

#[test]
fn a_token_streamed_over_three_pieces_never_shows_in_clear_in_the_store() {
    let user = user(json!([{"when": "go", "do": secret_in_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let store =
        flottille_core::store::Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let events = store.agent_events(&id, 0, 200).unwrap();

    let reply: String = events
        .iter()
        .filter(|e| e.name == "agent.message")
        .map(|e| e.data["text"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !reply.contains(SECRET),
        "{}",
        reply.replace(SECRET, "<clear>")
    );
    assert_eq!(reply, "the token is [masked], keep it quiet");
}

#[test]
fn a_token_cut_by_the_line_break_of_a_long_reply_reads_masked() {
    // The first piece fills a line but for the start of the token; the second ends it.
    let filler = "word ".repeat(395);
    let user = user(json!([{"when": "go", "do": [
        {"say": format!("{filler}s3cr3t-tok")},
        {"say": "en-value, keep it quiet"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let said = said(&lines(&read(&user, &id, 0)));

    let text = said.concat();
    assert!(
        !text.contains("s3cr3t-tok") && !text.contains("en-value"),
        "{said:?}"
    );
    assert_eq!(text, format!("{filler}[masked], keep it quiet"));
    assert!(said.iter().all(|line| line.chars().count() <= 2_000));
}

#[test]
fn a_provider_variable_s_value_streamed_in_pieces_reads_masked() {
    let user = User::with_fake_providers();
    user.provider_script(
        "opencode",
        json!({"turns": [{"when": "go", "do": [
            {"say": "the password is "},
            {"say_variable": "OPENCODE_SERVER_PASSWORD"},
            {"say": " and no more"}
        ]}]}),
    );
    project(&user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "go", "provider": "opencode"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let after = answer["after"].as_u64().unwrap();

    let watched = watched_reply(&user, &id, after);
    let said = said(&lines(&read(&user, &id, 0)));

    // Compared without a message: a failure would print the password.
    assert!(watched == "the password is [masked] and no more");
    assert!(said == ["the password is [masked] and no more"]);
}

#[test]
fn the_held_tail_of_a_reply_is_kept_when_the_agent_ends() {
    // Played once: the session that replaces it answers `ok`. The wait lets the pieces be read
    // before the process dies.
    let user = user(json!([{"when": "go", "times": 1, "do": [
        {"say": "last "}, {"say": "words"}, {"wait": 300}, {"die": 3}
    ]}]));
    let (id, _) = run(&user, "go");

    let mut said_so_far = Vec::new();
    eventually("the last words, then the next session's reply", || {
        said_so_far = said(&lines(&read(&user, &id, 0)));
        said_so_far.len() == 2
    });
    assert_eq!(said_so_far, ["last words", "ok"]);
}

// The leftovers of the streamed masking (#66).

/// A reply of id `m1` that tells the secret over two pieces, a tool the daemon serves landing
/// between them, as an MCP call does when it comes before the session updates around it.
fn a_tool_served_between_the_pieces() -> Value {
    json!([
        {"say": "I will look at the files now, then the token is s3cr3t-", "message": "m1"},
        {"mcp": {"tool": "fs_list", "arguments": {}, "quiet": true}},
        {"say": "token-value, keep it quiet", "message": "m1"}
    ])
}

const THE_REPLY: &str = "I will look at the files now, then the token is [masked], keep it quiet";

#[test]
fn a_tool_served_between_two_pieces_of_a_reply_does_not_split_it_in_a_conversation_page() {
    let user = user(json!([{"when": "go", "do": a_tool_served_between_the_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let lines = lines(&read(&user, &id, 0));

    let text = lines.join("\n");
    assert!(!text.contains("s3cr3t-"), "{text}");
    assert_eq!(said(&lines), [THE_REPLY], "{text}");
    // The reply is led by its first piece; the tool, served after it began, comes after it.
    let reply = lines.iter().position(|l| l.contains(" said: ")).unwrap();
    let tool = lines.iter().position(|l| l.contains("fs_list (")).unwrap();
    assert!(reply < tool, "{text}");
    assert!(number(&lines[reply]) < number(&lines[tool]), "{text}");
}

#[test]
fn a_tool_served_between_two_pieces_of_a_reply_does_not_split_it_in_agent_status() {
    let user = user(json!([{"when": "go", "do": a_tool_served_between_the_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let status = user.call("agent.status", json!({"agent": id})).unwrap();

    assert_eq!(status["messages"], json!([THE_REPLY]), "{status}");
}

#[test]
fn a_page_after_a_reply_split_by_a_tool_served_reads_neither_again() {
    let user = user(json!([{"when": "go", "do": a_tool_served_between_the_pieces()}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);
    let first = read(&user, &id, 0);

    let again = read(&user, &id, first["after"].as_u64().unwrap());

    let text = lines(&again).join("\n");
    assert!(
        !text.contains(" said: ") && !text.contains("fs_list ("),
        "{text}"
    );
}

/// The pieces of `name` stored for the agent, in order.
fn stored(user: &User, agent: &str, name: &str) -> Vec<String> {
    let store =
        flottille_core::store::Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    store
        .agent_events(agent, 0, 200)
        .unwrap()
        .iter()
        .filter(|e| e.name == name)
        .map(|e| e.data["text"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_mask_at_the_line_break_of_a_long_reply_is_never_cut_in_two() {
    // Masked, the token starts 5 characters before the 2,000th, and enough follows it for it to
    // be settled with what is around it.
    let filler = "word ".repeat(399);
    let rest = " and more".repeat(20);
    let user = user(json!([{"when": "go", "do": [
        {"say": format!("{filler}{SECRET}{rest}")}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let pieces = stored(&user, &id, "agent.message");
    let said = said(&lines(&read(&user, &id, 0)));

    let whole = format!("{filler}[masked]{rest}");
    assert_eq!(pieces.concat(), whole);
    assert_eq!(said.concat(), whole);
    for piece in pieces.iter().chain(&said) {
        assert!(
            !piece.replace("[masked]", "").contains(['[', ']']),
            "{pieces:?} {said:?}"
        );
    }
}

#[test]
fn a_token_streamed_over_three_thoughts_never_shows_in_clear_in_the_store() {
    let user = user(json!([{"when": "go", "do": [
        {"think": "the token is s3cr3t-"},
        {"think": "token-"},
        {"think": "value, keep it quiet"},
        {"say": "done"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let thought = stored(&user, &id, "agent.thought");

    let text = thought.concat();
    assert!(
        !text.contains("s3cr3t-") && !text.contains("token-"),
        "{thought:?}"
    );
    assert_eq!(text, "the token is [masked], keep it quiet");
}

#[test]
fn a_token_streamed_over_three_pieces_of_a_user_message_never_shows_in_clear_in_the_store() {
    let echo = |text: &str| {
        json!({"update": {"sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": text}}})
    };
    let user = user(json!([{"when": "go", "do": [
        echo("the token is s3cr3t-"),
        echo("token-"),
        echo("value, keep it quiet"),
        {"say": "done"}
    ]}]));
    let (id, after) = run(&user, "go");
    turn_ended(&user, &id, after, 1);

    let echoed = stored(&user, &id, "agent.user_message");

    let text = echoed.concat();
    assert!(
        !text.contains("s3cr3t-") && !text.contains("token-"),
        "{echoed:?}"
    );
    assert_eq!(text, "the token is [masked], keep it quiet");
}
