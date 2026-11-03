//! The scenarios of cutting long tool results at the source (`docs/tools.md`, "Long results"): a
//! result over the limit comes back as its beginning and its end with a line saying how much was
//! left out, the whole text is read by ranges with `run_status` and the id of the cut, nothing is
//! cut under the limit or when the setting is off, and what is kept is masked. The agent is the
//! fake ACP agent, in a throwaway repository and temporary XDG folders.

mod common;

use std::path::PathBuf;

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// A token of the kind `secrets` recognises anywhere.
const TOKEN: &str = "ghp_fakefakefakefakefakefakefakefake0001";

/// The default limit, in characters.
const LIMIT: usize = 8_000;

/// A file of `lines` lines, `line 0001 of the log` and so on, with a token on the second line and
/// another one in the middle.
fn log_of(lines: usize) -> String {
    let mut text = String::new();
    for number in 1..=lines {
        if number == 2 || number == lines / 2 {
            text.push_str(&format!("line {number:04} token {TOKEN}\n"));
        } else {
            text.push_str(&format!("line {number:04} of the log\n"));
        }
    }
    text
}

/// A project `app` over a throwaway repository holding `big.txt` (a long log) and `small.txt`,
/// and the session of a root agent working in its folder.
fn session(user: &User) -> (String, Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join("big.txt"), log_of(600)).unwrap();
    std::fs::write(folder.join("small.txt"), "one\ntwo\nthree\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "read the log"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization), folder)
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

fn read(mcp: &Mcp, path: &str) -> String {
    ok(mcp.call("fs_read", json!({"path": path})))
}

/// A run whose output is every line of `big.txt`, by the id the run gave.
fn long_run(mcp: &Mcp) -> String {
    let answer = ok(mcp.call("run", json!({"line": "cat big.txt"})));
    answer
        .strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("no run in {answer}"))
        .to_owned()
}

/// A result that is long whatever the page of the tool: the output of `long_run`. `fs_read` pages
/// its own text under the limit, so it is no way to a cut.
fn long(mcp: &Mcp) -> String {
    ok(mcp.call("run_status", json!({"run": long_run(mcp), "tail": 1000})))
}

fn limit(user: &User, limit: Value) {
    user.call("tool_results.set", json!({"limit": limit}))
        .unwrap();
}

/// The id a cut result names for reading it whole.
fn id_of(text: &str) -> String {
    let at = text
        .find("run_status {\"run\": \"")
        .unwrap_or_else(|| panic!("no way to read it whole in {text}"));
    text[at + "run_status {\"run\": \"".len()..]
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

/// The lines a page of a kept result shows, and the line the next page starts at.
fn page_lines(text: &str) -> (Vec<String>, Option<usize>) {
    let (about, lines) = text.split_once('\n').unwrap_or((text, ""));
    let next = about.split_once("later lines: ").and_then(|(_, rest)| {
        let (_, rest) = rest.split_once("\"start\": ")?;
        rest.split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse().ok())
    });
    (lines.split('\n').map(str::to_owned).collect(), next)
}

#[test]
fn a_long_tool_result_is_cut_with_its_beginning_and_its_end() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);

    let text = long(&mcp);

    assert!(text.chars().count() < LIMIT, "{} characters", text.len());
    assert!(text.contains("line 0001 of the log"), "{text}");
    assert!(text.contains("line 0600 of the log"), "{text}");
    assert!(!text.contains("line 0250 of the log"), "{text}");
    assert!(text.contains("characters left out"), "{text}");
}

#[test]
fn a_cut_result_says_how_to_read_it_whole() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);

    let text = long(&mcp);

    let id = id_of(&text);
    assert!(id.starts_with("res_"), "{id}");
    assert!(text.contains(&format!("run_status {{\"run\": \"{id}\", \"start\": ")));
    let first = ok(mcp.call("run_status", json!({"run": id, "start": 1})));
    assert!(first.contains("line 0001 of the log"), "{first}");
}

#[test]
fn the_whole_result_is_read_by_ranges_each_under_the_limit() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);
    let run = long_run(&mcp);
    let output = json!({"run": run, "tail": 1000});
    limit(&user, json!("off"));
    let whole = ok(mcp.call("run_status", output.clone()));
    limit(&user, json!(LIMIT));
    let id = id_of(&ok(mcp.call("run_status", output)));

    let mut lines: Vec<String> = Vec::new();
    let mut start = 1;
    let mut pages = 0;
    loop {
        let page = ok(mcp.call("run_status", json!({"run": id, "start": start})));
        assert!(page.chars().count() <= LIMIT, "{} characters", page.len());
        assert!(!page.contains("characters left out"), "{page}");
        let (shown, next) = page_lines(&page);
        lines.extend(shown);
        pages += 1;
        match next {
            Some(next) => start = next,
            None => break,
        }
        assert!(pages < 100, "it never ends");
    }

    assert!(pages > 1, "the whole text fits one page");
    // Masked as it is when read whole: the token of the middle is hidden in every page.
    assert_eq!(lines.join("\n"), whole.trim_end_matches('\n'));
}

#[test]
fn a_result_under_the_limit_is_unchanged() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);

    let text = read(&mcp, "small.txt");

    assert!(
        text.contains("     1\tone\n     2\ttwo\n     3\tthree"),
        "{text}"
    );
    assert!(!text.contains("left out"), "{text}");
    assert!(!text.contains("res_"), "{text}");
}

#[test]
fn tool_results_off_returns_them_whole() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);
    limit(&user, json!("off"));

    let text = long(&mcp);

    assert!(text.chars().count() > LIMIT, "{} characters", text.len());
    assert!(text.contains("line 0250 of the log"), "{text}");
    assert!(!text.contains("left out"), "{text}");
}

#[test]
fn a_cut_result_is_masked_like_any_output() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);

    let text = long(&mcp);
    let id = id_of(&text);
    let mut seen = text;
    let mut start = 1;
    loop {
        let page = ok(mcp.call("run_status", json!({"run": id, "start": start})));
        let (_, next) = page_lines(&page);
        seen.push_str(&page);
        match next {
            Some(next) => start = next,
            None => break,
        }
    }

    assert!(!seen.contains("ghp_fake"), "{seen}");
    assert!(seen.contains("[masked]"), "{seen}");
    // The middle one is only in the kept text: line 300 is read from it.
    assert!(seen.contains("line 0300 token [masked]"), "{seen}");
}

#[test]
fn a_result_that_was_never_cut_is_not_readable_by_its_id() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);

    let answer = mcp.call("run_status", json!({"run": "res_000000000000"}));

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("no cut result"), "{}", answer.text);
}

#[test]
fn a_cut_result_is_recorded_with_how_much_was_left_out() {
    let user = User::with_fake_agent().with_cut_results();
    let watch = user.watch(&["--after", "0"]);
    let (_, mcp, _) = session(&user);

    let text = ok(mcp.call_keyed(
        "run_status",
        json!({"run": long_run(&mcp), "tail": 1000}),
        "toolu_1",
    ));

    let events = watch.until(|e| e["name"] == "agent.tool_served" && e["data"]["cut"].is_object());
    let cut = &events.last().unwrap()["data"]["cut"];
    assert_eq!(cut["result"].as_str(), Some(id_of(&text).as_str()));
    assert!(cut["left_out"].as_u64().unwrap() > 7_000, "{cut}");
}

#[test]
fn the_limit_is_8000_characters_until_the_user_chooses_another() {
    let user = User::with_fake_agent().with_cut_results();

    assert_eq!(
        user.call("tool_results.show", json!({})).unwrap(),
        json!({"limit": 8000})
    );
    assert!(!user.path("config/flottille/tool-results.toml").exists());
}

#[test]
fn a_chosen_limit_is_kept_announced_and_read_back_after_a_restart() {
    let user = User::with_fake_agent().with_cut_results();
    let watch = user.watch(&["--after", "0"]);

    let shown = user
        .call("tool_results.set", json!({"limit": 16000}))
        .unwrap();

    assert_eq!(shown, json!({"limit": 16000}));
    let file = std::fs::read_to_string(user.path("config/flottille/tool-results.toml")).unwrap();
    assert!(file.contains("limit = 16000"), "{file}");
    let seen = watch.until(|e| e["name"] == "tool_results.changed");
    assert_eq!(seen.last().unwrap()["data"], json!({"limit": 16000}));
    assert!(user.run(&["daemon", "stop"]).status.success());
    assert_eq!(
        user.call("tool_results.show", json!({})).unwrap(),
        json!({"limit": 16000})
    );
    limit(&user, json!("off"));
    let file = std::fs::read_to_string(user.path("config/flottille/tool-results.toml")).unwrap();
    assert!(file.contains("limit = \"off\""), "{file}");
    assert_eq!(
        user.call("tool_results.show", json!({})).unwrap(),
        json!({"limit": "off"})
    );
}

#[test]
fn a_limit_too_small_or_too_large_is_refused() {
    let user = User::with_fake_agent().with_cut_results();
    for bad in [
        json!(0),
        json!(3999),
        json!(40001),
        json!("big"),
        json!(-1),
        json!(null),
    ] {
        let (code, _) = user
            .call("tool_results.set", json!({"limit": bad}))
            .unwrap_err();
        assert_eq!(code, "invalid-params", "{bad}");
    }
    assert!(!user.path("config/flottille/tool-results.toml").exists());
}

#[test]
fn a_file_naming_another_word_than_off_is_an_invalid_config() {
    let user = User::with_fake_agent().with_cut_results();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/tool-results.toml"),
        "limit = \"some\"\n",
    )
    .unwrap();

    let (code, message) = user.call("tool_results.show", json!({})).unwrap_err();

    assert_eq!(code, "invalid-config");
    assert!(message.contains("tool-results.toml"), "{message}");
}

/// The pages of `big.txt` as `fs_read` gives them from the start, each under the limit: the line
/// numbers of their lines, and how many pages there were.
fn pages_of_big(mcp: &Mcp) -> (Vec<usize>, usize) {
    let (mut numbers, mut pages, mut offset) = (Vec::new(), 0, 0);
    loop {
        let text = ok(mcp.call(
            "fs_read",
            json!({"path": "big.txt", "range": {"offset": offset}}),
        ));
        pages += 1;
        assert!(!text.contains("left out"), "{text}");
        assert!(!text.contains("res_"), "{text}");
        numbers.extend(text.lines().filter_map(|line| {
            let (number, rest) = line.split_once('\t')?;
            rest.starts_with("line ")
                .then(|| number.trim().parse::<usize>().unwrap())
        }));
        let range: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        match range["next"].as_u64() {
            Some(next) => offset = next,
            None => return (numbers, pages),
        }
        assert!(pages < 100, "it never ends");
    }
}

#[test]
fn a_page_of_fs_read_is_sized_so_that_its_result_is_not_cut() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);

    let (numbers, pages) = pages_of_big(&mcp);

    // Every line of the file was shown whole, once, so that reading it means having seen it.
    assert_eq!(numbers, (1..=600).collect::<Vec<_>>());
    assert!(pages > 1, "the file fits one page");
}

#[test]
fn a_page_of_fs_read_follows_the_limit_the_user_chose() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);
    let (_, at_default) = pages_of_big(&mcp);
    limit(&user, json!(4_000));
    let first = read(&mcp, "big.txt");
    let (numbers, at_small) = pages_of_big(&mcp);
    limit(&user, json!("off"));
    let (_, whole) = pages_of_big(&mcp);

    assert!(first.chars().count() < 4_000, "{} characters", first.len());
    assert_eq!(numbers, (1..=600).collect::<Vec<_>>());
    assert!(at_small > at_default, "{at_small} pages, then {at_default}");
    assert_eq!(whole, 1);
}

/// The session of a second root agent of the project `app`, which `session` set up.
fn another_root(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "message": "read another log"}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

/// The session of a child the agent launched.
fn child_of(user: &User, mcp: &Mcp) -> Mcp {
    let opened = user.received_requests("session/new").len();
    ok(mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Read the log."}),
    ));
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

#[test]
fn a_child_and_another_agent_cannot_read_the_cut_result_of_a_session() {
    let user = User::with_fake_agent().with_cut_results();
    let (_, mcp, _) = session(&user);
    let id = id_of(&long(&mcp));
    let child = child_of(&user, &mcp);
    let other = another_root(&user);

    let own = ok(mcp.call("run_status", json!({"run": id})));
    assert!(own.contains("line 0001 of the log"), "{own}");
    for (who, session) in [("child", &child), ("other agent", &other)] {
        let answer = session.call("run_status", json!({"run": id}));
        assert!(answer.error, "{who}: {}", answer.text);
        assert!(
            answer.text.contains("no cut result of this session"),
            "{who}: {}",
            answer.text
        );
        assert!(!answer.text.contains("line 0001"), "{who}: {}", answer.text);
    }
}
