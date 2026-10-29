//! The scenarios of the session's mode and the Auto judge (`docs/design.md` §7): each class in
//! each mode, the judge allowing, asking, failing and keeping silent, no judge chosen, the mode a
//! child inherits and a resume keeps, and what the judge is given and never given. The agents and
//! the judge are fake ACP agents (`crates/flottille-fake-agent`), in throwaway repositories and
//! temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// A secret of the place's `.env`: never shown to the judge.
const PASSWORD: &str = "hunter2-hunter2";
/// A token of the kind `secrets` recognises anywhere: never shown to the judge either.
const TOKEN: &str = "ghp_fakefakefakefakefakefakefakefake0001";

/// A project `app` over a throwaway repository with a `.env`, its workspace `w` (where the `.env`
/// is copied), and the session of a root agent working there, or in the project folder when
/// `main`: the agent's id, its session, the place's folder.
fn session(user: &User, main: bool) -> (String, Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), format!("DB_PASSWORD={PASSWORD}\n")).unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[[setup]]\ncopy = [\".env\"]\n");
    std::fs::write(&file, text).unwrap();
    let mut params = json!({"project": "app", "message": "add the login page, then stop"});
    let place = if main {
        folder
    } else {
        let workspace = user
            .call("workspace.create", json!({"project": "app", "name": "w"}))
            .unwrap();
        eventually("the workspace", || {
            user.call("workspace.show", json!({"project": "app", "name": "w"}))
                .unwrap()["state"]
                == "ready"
        });
        params["workspace"] = "w".into();
        PathBuf::from(workspace["folder"].as_str().unwrap())
    };
    let opened = user.received_requests("session/new").len();
    let answer = user.call("agent.run", params).unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization), place)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// No fake `sudo`, `doas`, `pkexec`, `run0` or `su` ran: the gate held the call.
fn no_escalation(user: &User) {
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

/// The request a held call made, as `approval.list` gives it.
fn held(user: &User, answer: Answer) -> Value {
    no_escalation(user);
    assert!(!answer.error, "an error: {}", answer.text);
    let number: u64 = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mode_of(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["gate_mode"].clone()
}

/// The prompts the judge was given, in order.
fn judge_prompts(user: &User) -> Vec<String> {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .map(|e| {
            e["received"]["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// The last `agent.tool_served` event of the agent.
fn last_served(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|e| e["name"] == "agent.tool_served")
        .cloned()
        .unwrap()["data"]
        .clone()
}

/// The judge's answer, as the fake judge says it.
fn says(verdict: &str, reason: &str) -> Value {
    json!([{"say": json!({"verdict": verdict, "reason": reason}).to_string()}])
}

#[test]
fn a_new_session_starts_in_auto_and_the_user_sets_its_mode_from_the_command_line() {
    let user = User::with_fake_agent();
    let (id, _, _) = session(&user, false);
    assert_eq!(mode_of(&user, &id), "auto");

    let output = user.run(&["agent", "mode", &id, "ask"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("Agent {id} is in ask mode now.\n")
    );
    assert_eq!(mode_of(&user, &id), "ask");
    let watch = user.watch(&["--after", "0"]);
    let seen = watch.until(|e| e["name"] == "agent.gate_mode_changed");
    let changed = seen.last().unwrap();
    assert_eq!(changed["data"], json!({"agent": id, "mode": "ask"}));
    let refused = user.run(&["agent", "mode", &id, "yolo"]);
    assert!(!refused.status.success());
    assert_eq!(mode_of(&user, &id), "ask");
}

#[test]
fn harmless_calls_pass_in_ask_and_in_auto_without_the_judge() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, _) = session(&user, false);

    for mode in ["auto", "ask"] {
        set_mode(&user, &id, mode);
        done(mcp.call("fs_read", json!({"path": "README.md"})));
        done(mcp.call(
            "fs_write",
            json!({"path": "src/login.rs", "content": "fn login() {}\n"}),
        ));
        let status = done(mcp.call("commands_run", json!({"line": "git status"})));
        assert!(status.contains("exited with code 0"), "{mode}: {status}");
        done(mcp.call(
            "commands_run",
            json!({"line": "pgrep -f nothing-runs-like-this"}),
        ));
    }
    assert!(judge_prompts(&user).is_empty(), "the judge was asked");
}

#[test]
fn a_call_to_judge_in_ask_makes_a_card_and_never_reaches_the_judge() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, _) = session(&user, false);
    set_mode(&user, &id, "ask");

    let request = held(
        &user,
        mcp.call("commands_run", json!({"line": "touch made.txt"})),
    );

    assert_eq!(request["class"], "judge");
    assert!(request["judgement"].is_null(), "{request}");
    assert!(judge_prompts(&user).is_empty());
}

#[test]
fn writing_in_main_in_ask_makes_a_card() {
    let user = User::with_fake_agent();
    let (id, mcp, folder) = session(&user, true);
    set_mode(&user, &id, "ask");

    let request = held(
        &user,
        mcp.call(
            "fs_write",
            json!({"path": "notes.txt", "content": "notes\n"}),
        ),
    );

    assert_eq!(request["class"], "judge");
    assert!(
        request["reasons"].to_string().contains("project folder"),
        "{request}"
    );
    assert!(!folder.join("notes.txt").exists());
}

#[test]
fn in_auto_the_judge_allows_a_call_and_it_is_done_at_once_with_its_verdict_recorded() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, folder) = session(&user, false);

    let started = Instant::now();
    let answer = done(mcp.call(
        "commands_run",
        json!({"line": "touch made.txt", "why": "the build needs the file"}),
    ));
    let took = started.elapsed();

    assert!(answer.contains("exited with code 0"), "{answer}");
    assert!(folder.join("made.txt").exists());
    let served = last_served(&user, &id);
    assert_eq!(served["gate"], "judged");
    assert_eq!(served["judgement"]["verdict"], "allow");
    assert_eq!(served["judgement"]["reason"], "It fits the task.");
    assert_eq!(served["judgement"]["by"], "opencode");
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // One judge process per judgement: how long one takes here, said in the test's output.
    eprintln!("a judged call took {} ms", took.as_millis());
    assert!(took < Duration::from_secs(10), "{took:?}");
}

#[test]
fn a_call_the_judge_let_through_is_recorded_with_the_key_the_window_matches_its_card_by() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, _) = session(&user, false);

    done(mcp.call_keyed(
        "commands_run",
        json!({"line": "touch made.txt", "why": "the build needs the file"}),
        "toolu_card",
    ));

    let served = last_served(&user, &id);
    assert_eq!(served["gate"], "judged");
    assert_eq!(served["call_key"], "toolu_card");
}

#[test]
fn in_auto_the_judge_asks_and_the_card_and_the_command_line_say_its_reason() {
    let user = User::with_fake_agent().with_judge(says("ask", "Nothing in the task asks for it."));
    let (_, mcp, folder) = session(&user, false);

    let request = held(
        &user,
        mcp.call("commands_run", json!({"line": "touch made.txt"})),
    );

    assert_eq!(request["class"], "judge");
    assert_eq!(
        request["judgement"],
        json!({"by": "opencode", "verdict": "ask", "reason": "Nothing in the task asks for it."})
    );
    assert!(!folder.join("made.txt").exists());
    let listed = user.run(&["approvals"]);
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(
        text.contains("judge opencode (ask): Nothing in the task asks for it."),
        "{text}"
    );
}

#[test]
fn an_answer_of_the_judge_that_cannot_be_read_asks() {
    let user = User::with_fake_agent().with_judge(json!([{"say": "Looks fine to me."}]));
    let (_, mcp, _) = session(&user, false);

    let request = held(
        &user,
        mcp.call("commands_run", json!({"line": "touch made.txt"})),
    );

    assert_eq!(request["judgement"]["verdict"], "failed", "{request}");
    assert!(
        request["judgement"]["reason"]
            .as_str()
            .unwrap()
            .contains("not the JSON verdict"),
        "{request}"
    );
}

#[test]
fn a_judge_that_never_answers_asks_once_its_time_is_up() {
    let user = User::with_fake_agent()
        .with_judge(json!([{"hang": true}]))
        .with_env("FLOTTILLE_JUDGE_TIMEOUT_MS", "800");
    let (_, mcp, _) = session(&user, false);

    let started = Instant::now();
    let request = held(
        &user,
        mcp.call("commands_run", json!({"line": "touch made.txt"})),
    );

    assert!(started.elapsed() >= Duration::from_millis(800));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(request["judgement"]["verdict"], "failed", "{request}");
    assert!(
        request["judgement"]["reason"]
            .as_str()
            .unwrap()
            .contains("gave no answer within 0.8 seconds"),
        "{request}"
    );
}

#[test]
fn a_judge_that_says_anything_but_allow_or_ask_asks() {
    let user = User::with_fake_agent().with_judge(says("deny", "No."));
    let (_, mcp, _) = session(&user, false);

    let request = held(
        &user,
        mcp.call("commands_run", json!({"line": "touch made.txt"})),
    );

    assert_eq!(request["judgement"]["verdict"], "failed", "{request}");
}

#[test]
fn without_a_judge_auto_asks_as_ask_does() {
    let user = User::with_fake_agent();
    let (id, mcp, _) = session(&user, false);
    assert_eq!(mode_of(&user, &id), "auto");

    let request = held(
        &user,
        mcp.call("commands_run", json!({"line": "touch made.txt"})),
    );

    assert_eq!(request["class"], "judge");
    assert!(request["judgement"].is_null(), "{request}");
}

#[test]
fn a_base_call_in_auto_makes_a_card_and_never_reaches_the_judge() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    for (tool, args) in [
        ("fs_read", json!({"path": ".env"})),
        (
            "commands_run",
            json!({"line": "git push --force origin HEAD"}),
        ),
        ("commands_run", json!({"line": "rm -rf .git"})),
    ] {
        let request = held(&user, mcp.call(tool, args.clone()));
        assert_eq!(request["class"], "base", "{args}");
        assert!(request["judgement"].is_null(), "{args}");
    }
    assert!(judge_prompts(&user).is_empty(), "the judge was asked");
}

#[test]
fn a_base_call_in_ask_makes_a_card_too() {
    let user = User::with_fake_agent();
    let (id, mcp, _) = session(&user, false);
    set_mode(&user, &id, "ask");

    let request = held(&user, mcp.call("fs_read", json!({"path": ".env"})));

    assert_eq!(request["class"], "base");
}

#[test]
fn shell_syntax_and_a_malformed_call_stay_refused_in_both_modes() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, _) = session(&user, false);

    for mode in ["auto", "ask"] {
        set_mode(&user, &id, mode);
        let shell = mcp.call("commands_run", json!({"line": "ls | wc -l"}));
        assert!(shell.error, "{mode}: {}", shell.text);
        let malformed = mcp.call(
            "forge_write",
            json!({"action": "pr-delete", "repository": "."}),
        );
        assert!(malformed.error, "{mode}: {}", malformed.text);
    }
    assert!(judge_prompts(&user).is_empty());
}

#[test]
fn reading_outside_the_project_goes_to_the_judge() {
    let user = User::with_fake_agent().with_allowing_judge();
    let notes = user.path("home/notes.txt");
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(&notes, "made-up notes\n").unwrap();
    let (_, mcp, _) = session(&user, false);

    let read = done(mcp.call("fs_read", json!({"path": notes})));

    assert!(read.contains("made-up notes"), "{read}");
    assert_eq!(judge_prompts(&user).len(), 1);
}

#[test]
fn a_child_inherits_its_parent_s_mode() {
    let user = User::with_fake_agent();
    let (lead, mcp, _) = session(&user, false);
    set_mode(&user, &lead, "ask");

    let launched = mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);

    let agents = user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone();
    let child = agents
        .iter()
        .find(|a| a["parent"] == lead.as_str())
        .unwrap();
    assert_eq!(child["gate_mode"], "ask");
}

#[test]
fn the_mode_survives_a_resume() {
    let user = User::with_fake_agent();
    let (id, _, _) = session(&user, false);
    set_mode(&user, &id, "ask");

    user.call("agent.stop", json!({"agent": id})).unwrap();
    user.resume(&id);

    assert_eq!(mode_of(&user, &id), "ask");
}

#[test]
fn the_judge_s_prompt_holds_the_call_the_place_the_user_s_words_and_the_why_never_a_secret() {
    let user = User::with_fake_agent()
        .with_judge(says("ask", "Let the user see it."))
        .with_env("GH_TOKEN", TOKEN);
    let (_, mcp, _) = session(&user, false);
    let outside = user.path("elsewhere/config.txt");
    let content = format!(
        "password = {PASSWORD}\ntoken = {TOKEN}\n{}",
        (3..=40).map(|n| format!("line {n}\n")).collect::<String>()
    );

    held(
        &user,
        mcp.call(
            "fs_write",
            json!({"path": outside, "content": content,
                "why": format!("the deploy needs {PASSWORD}")}),
        ),
    );

    let prompts = judge_prompts(&user);
    assert_eq!(prompts.len(), 1);
    let prompt = &prompts[0];
    assert!(prompt.contains("Tool: fs_write"), "{prompt}");
    assert!(prompt.contains("is outside this agent's place"), "{prompt}");
    assert!(prompt.contains("workspace w of project app"), "{prompt}");
    assert!(prompt.contains("add the login page, then stop"), "{prompt}");
    assert!(prompt.contains("the deploy needs"), "{prompt}");
    // A file's content is cut to its first lines.
    assert!(prompt.contains("line 3"), "{prompt}");
    assert!(!prompt.contains("line 40"), "{prompt}");
    for secret in [PASSWORD, TOKEN] {
        assert!(!prompt.contains(secret), "{secret} in {prompt}");
    }
    // Nor anywhere else the judge was sent.
    let everything = user
        .judge_received()
        .iter()
        .map(Value::to_string)
        .collect::<String>();
    for secret in [PASSWORD, TOKEN] {
        assert!(!everything.contains(secret), "{secret} sent to the judge");
    }
}

#[test]
fn the_judge_runs_with_no_tool_and_no_mcp_server() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    done(mcp.call("commands_run", json!({"line": "touch made.txt"})));

    let opened: Vec<Value> = user
        .judge_received()
        .into_iter()
        .filter(|e| e["received"]["method"] == "session/new")
        .map(|e| e["received"]["params"].clone())
        .collect();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0]["mcpServers"], json!([]), "{}", opened[0]);
    assert!(!opened[0].to_string().contains("mcp__"), "{}", opened[0]);
    // OpenCode, its judge here, is told its tools in its configuration: every one denied.
    let started = user
        .judge_received()
        .into_iter()
        .find(|e| e["started"] == true)
        .unwrap();
    let config: Value =
        serde_json::from_str(started["env"]["OPENCODE_CONFIG_CONTENT"].as_str().unwrap()).unwrap();
    let permission = config["agent"]["flottille"]["permission"]
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(permission["*"], "deny");
    for (tool, said) in &permission {
        assert!(said == "deny" || tool == "doom_loop", "{tool}: {said}");
    }
    assert!(
        !config.to_string().contains("\"mcp\""),
        "no MCP server: {config}"
    );
    // Every permission it would ask is refused.
    assert!(
        !user
            .judge_received()
            .iter()
            .any(|e| e["permission"]["outcome"]["outcome"] == "selected"),
    );
}

/// Whether the throwaway remote of `session` has the branch.
fn remote_has(user: &User, branch: &str) -> bool {
    std::process::Command::new("git")
        .arg("--git-dir")
        .arg(user.path("remotes/app.git"))
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn a_harmless_command_whose_script_pushes_fails_to_push() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, folder) = session(&user, false);
    let script = folder.join("check.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ngit push origin HEAD:refs/heads/sneaked\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[commands.check]\nline = \"./check.sh\"\n");
    std::fs::write(&file, text).unwrap();

    let ran = done(mcp.call("commands_run", json!({"command": "check"})));

    assert!(!ran.contains("exited with code 0"), "{ran}");
    assert!(!remote_has(&user, "sneaked"));
    assert!(
        judge_prompts(&user).is_empty(),
        "the catalogue command was judged"
    );
}

#[test]
fn a_push_the_judge_allows_reaches_the_remote() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    let pushed = done(mcp.call(
        "commands_run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "allowed"));
}

#[test]
fn a_line_longer_than_the_judge_s_input_asks_without_the_judge() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);
    let line = format!("touch {}", "ab ".repeat(3_000));

    let request = held(&user, mcp.call("commands_run", json!({"line": line})));

    assert_eq!(request["class"], "judge", "{request}");
    assert_eq!(request["judgement"]["verdict"], "failed", "{request}");
    assert!(
        request["judgement"]["reason"]
            .as_str()
            .unwrap()
            .contains("longer than"),
        "{request}"
    );
    assert!(judge_prompts(&user).is_empty(), "the judge was asked");
}

#[test]
fn a_long_line_reaches_the_judge_whole() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);
    let line = format!("touch {}z", "ab ".repeat(500));

    let _ = mcp.call("commands_run", json!({"line": line}));

    let prompts = judge_prompts(&user);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(&line), "{}", prompts[0]);
}

/// Makes the fake judge of `with_judge` leave a `sleep` behind in its own process group, its pid
/// written to `fake-judge/sleepers`.
fn judge_leaving_a_child(user: &User) -> PathBuf {
    let launcher = user.path("fake-judge/agent");
    let sleepers = user.path("fake-judge/sleepers");
    let script = std::fs::read_to_string(&launcher).unwrap();
    let script = script.replacen(
        "#!/bin/sh\n",
        &format!(
            "#!/bin/sh\nsleep 300 &\necho $! >> '{}'\n",
            sleepers.display()
        ),
        1,
    );
    std::fs::write(&launcher, script).unwrap();
    sleepers
}

/// Whether a process still runs (a zombie does not).
fn runs(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

#[test]
fn the_judge_s_own_child_processes_end_with_it() {
    for hangs in [false, true] {
        let user = if hangs {
            User::with_fake_agent()
                .with_judge(json!([{"hang": true}]))
                .with_env("FLOTTILLE_JUDGE_TIMEOUT_MS", "800")
        } else {
            User::with_fake_agent().with_allowing_judge()
        };
        let sleepers = judge_leaving_a_child(&user);
        let (_, mcp, _) = session(&user, false);

        let _ = mcp.call("commands_run", json!({"line": "touch made.txt"}));

        let pid: u32 = std::fs::read_to_string(&sleepers)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        eventually("the judge's child ends", || !runs(pid));
        assert!(!runs(pid), "hangs {hangs}");
    }
}

#[test]
fn switching_the_root_to_ask_makes_a_child_s_next_call_a_card() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (lead, mcp, _) = session(&user, false);
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);
    done(child.call("commands_run", json!({"line": "touch first.txt"})));
    assert_eq!(judge_prompts(&user).len(), 1);

    set_mode(&user, &lead, "ask");

    let answer = child.call("commands_run", json!({"line": "touch second.txt"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.starts_with("Waiting for"), "{}", answer.text);
    assert_eq!(judge_prompts(&user).len(), 1, "the judge was asked");
}

/// Each call is held for the user, as base, though the judge allows everything, which is never
/// asked.
fn base_in_auto(user: &User, mcp: &Mcp, calls: &[(&str, Value)]) {
    for (tool, args) in calls {
        let request = held(user, mcp.call(tool, args.clone()));
        assert_eq!(request["class"], "base", "{args}: {request}");
        assert!(request["judgement"].is_null(), "{args}: {request}");
    }
    assert!(judge_prompts(user).is_empty(), "the judge was asked");
}

fn line(text: &str) -> (&'static str, Value) {
    ("commands_run", json!({ "line": text }))
}

#[test]
fn a_git_setting_whose_value_git_runs_is_base() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    base_in_auto(
        &user,
        &mcp,
        &[
            line("git -c diff.external=./x diff"),
            line("git -c pager.log=./x log"),
            line("git config sequence.editor ./x"),
            line("git config filter.x.smudge ./x"),
            line("git --config-env gpg.program=X log --show-signature"),
            line("env GIT_EXTERNAL_DIFF=./x git diff"),
        ],
    );
}

#[test]
fn a_push_with_a_receive_pack_or_exec_is_base() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    base_in_auto(
        &user,
        &mcp,
        &[
            line("git push --receive-pack=./x origin HEAD"),
            line("git push --exec ./x origin HEAD"),
        ],
    );
}

#[test]
fn sudo_doas_and_pkexec_are_base_in_any_form() {
    let user = User::with_fake_agent()
        .with_fake_escalation()
        .with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    base_in_auto(
        &user,
        &mcp,
        &[
            line("sudo npm test"),
            line("sudo -E ls"),
            line("doas ls"),
            line("pkexec ls"),
            line("nice sudo ls"),
        ],
    );
    no_escalation(&user);
}

#[test]
fn removing_git_s_safety_net_is_base_in_main_and_in_a_workspace() {
    for main in [true, false] {
        let user = User::with_fake_agent().with_allowing_judge();
        let (_, mcp, _) = session(&user, main);

        base_in_auto(
            &user,
            &mcp,
            &[
                line("git reflog expire --expire=now --all"),
                line("git gc --prune=now"),
                line("git prune"),
            ],
        );
    }
}

#[test]
fn a_rebase_exec_line_is_read_as_a_line_it_runs() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    base_in_auto(
        &user,
        &mcp,
        &[
            line("git rebase -x 'git push -f' main"),
            line("git rebase --exec='git push --force origin HEAD' main"),
        ],
    );
}

#[test]
fn a_program_in_the_user_s_bin_folders_is_not_taken_for_installed() {
    let user = User::with_fake_agent();
    let bin = user.path("home/.local/bin");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let user = user.with_allowing_judge().with_env("PATH", &path);
    std::fs::create_dir_all(&bin).unwrap();
    // What an agent the judge allowed may have written there, named as a reader.
    std::fs::write(bin.join("ls"), "#!/bin/sh\ntouch \"$HOME/ran\"\n").unwrap();
    std::fs::set_permissions(
        bin.join("ls"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let (_, mcp, _) = session(&user, false);

    done(mcp.call("commands_run", json!({"line": "ls"})));

    let prompts = judge_prompts(&user);
    assert_eq!(prompts.len(), 1, "a harmless call, never judged");
    assert!(prompts[0].contains("agent's own file"), "{}", prompts[0]);
}

#[test]
fn the_path_of_a_run_is_resolved_in_the_repository_it_runs_in() {
    let user = User::with_fake_agent().with_allowing_judge();
    let folder = user.path("work/app");
    for name in ["api", "web"] {
        let repository = folder.join(name);
        user.git_init(&repository);
        std::fs::write(repository.join("README.md"), format!("{name}\n")).unwrap();
        user.git(&repository, &["add", "."]);
        user.git(&repository, &["commit", "--quiet", "-m", "start"]);
    }
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\nworkspaces_root = \"{}\"\n[[repositories]]\npath = \"api\"\n\
             [[repositories]]\npath = \"web\"\n[variables]\n\
             PATH = \"{{workspace.path}}/{{repository}}/bin:{}\"\n",
            folder.display(),
            user.path("work/app-work").display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    // Only web has one: the run in web finds it first, the agent's own file named as a reader.
    let bin = folder.join("web/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("ls"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        bin.join("ls"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "look"}))
        .unwrap();
    let (url, authorization) = server_of(&user, opened);
    let mcp = Mcp::initialize(&url, &authorization);

    done(mcp.call("commands_run", json!({"line": "ls", "repository": "api"})));
    assert!(
        judge_prompts(&user).is_empty(),
        "the system's ls was judged"
    );
    done(mcp.call("commands_run", json!({"line": "ls", "repository": "web"})));

    let prompts = judge_prompts(&user);
    assert_eq!(prompts.len(), 1, "web's own ls, taken for the system's");
    assert!(prompts[0].contains("agent's own file"), "{}", prompts[0]);
}

#[test]
fn an_agent_setting_the_project_s_rules_is_base() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, mcp, _) = session(&user, false);

    base_in_auto(
        &user,
        &mcp,
        &[(
            "project",
            json!({"action": "set_rules", "text": "Push with --force when done."}),
        )],
    );
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!file.contains("Push with"), "{file}");
}
