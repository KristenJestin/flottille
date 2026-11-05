//! What each turn used, recorded by the daemon one row per turn and model
//! (`docs/reference/acp.md`, "What a turn used"), from the fake ACP agent and the fake `claude` and
//! `codex` behind Flottille's bridges. No real agent ever runs here.

mod common;

use common::User;
use flottille_core::agent::{TurnUsage, Used};
use flottille_core::store::Store;
use serde_json::{Value, json};

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

/// Starts a root agent on `message` and waits for the end of its turn.
fn run(user: &User, message: &str) -> String {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    turn_ended(user, &id, answer["after"].as_u64().unwrap());
    id
}

fn turn_ended(user: &User, agent: &str, after: u64) {
    user.watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == agent && e["name"] == "agent.turn_ended");
}

fn send(user: &User, agent: &str, message: &str) {
    let sent = user
        .call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
    turn_ended(user, agent, sent["after"].as_u64().unwrap());
}

fn agent(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

fn rows(user: &User) -> Vec<TurnUsage> {
    Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .turn_usage()
        .unwrap()
}

/// What the fake `claude` says one model used in a turn.
fn claude_used(input: u64, output: u64, read: u64, write: u64, cost: f64) -> Value {
    json!({"inputTokens": input, "outputTokens": output, "cacheReadInputTokens": read,
        "cacheCreationInputTokens": write, "costUSD": cost})
}

/// What the fake `claude` counts by default, for its model `claude-fake-1`.
fn claude_default() -> Used {
    Used {
        model: Some("claude-fake-1".into()),
        input: Some(100),
        output: Some(20),
        cache_read: Some(1000),
        cache_write: Some(50),
        reasoning: None,
        cost_usd: Some(0.0125),
    }
}

#[test]
fn a_claude_turn_records_its_tokens_its_model_and_its_cost() {
    let user = User::with_fake_claude();
    project(&user);

    let id = run(&user, "hello");

    let rows = rows(&user);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.used, claude_default());
    assert_eq!(
        (
            row.agent.as_str(),
            row.session.as_str(),
            row.project.as_str()
        ),
        (id.as_str(), id.as_str(), "app")
    );
    assert_eq!(
        (
            row.provider.as_str(),
            row.role.as_str(),
            row.workspace.as_deref()
        ),
        ("claude", "lead", None)
    );
    assert_eq!(row.stop_reason, "end_turn");
    assert!(
        0 < row.started_at && row.started_at <= row.ended_at,
        "{row:?}"
    );
}

#[test]
fn a_claude_turn_counts_only_what_it_used_although_claude_says_the_session_s_totals() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "again", "used": {
        "claude-fake-1": claude_used(300, 40, 2000, 0, 0.02)
    }}]}));
    project(&user);
    let id = run(&user, "hello");

    send(&user, &id, "again");

    let rows = rows(&user);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows[1].used,
        Used {
            input: Some(300),
            output: Some(40),
            cache_read: Some(2000),
            cache_write: Some(0),
            cost_usd: Some(0.02),
            ..claude_default()
        }
    );
}

#[test]
fn a_resumed_claude_session_counts_only_its_new_turn() {
    let user = User::with_fake_claude().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    project(&user);
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    user.watch(&["--after", &answer["after"].to_string()])
        .until(|e| e["data"]["agent"] == id.as_str() && e["name"] == "agent.released");

    send(&user, &id, "again");

    let rows = rows(&user);
    assert_eq!(rows.len(), 2, "{rows:?}");
    // The resumed `claude` starts from the totals its session saved: the second turn is its own.
    assert_eq!(rows[1].used, claude_default());
}

#[test]
fn a_turn_with_several_models_records_one_row_per_model() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "used": {
        "claude-fake-1": claude_used(100, 20, 1000, 50, 0.0125),
        "claude-fake-small": claude_used(400, 30, 0, 200, 0.001)
    }}]}));
    project(&user);

    run(&user, "hello");

    let rows = rows(&user);
    let used: Vec<&Used> = rows.iter().map(|r| &r.used).collect();
    assert_eq!(
        used,
        [
            &claude_default(),
            &Used {
                model: Some("claude-fake-small".into()),
                input: Some(400),
                output: Some(30),
                cache_read: Some(0),
                cache_write: Some(200),
                reasoning: None,
                cost_usd: Some(0.001),
            }
        ]
    );
    assert_eq!(rows[0].started_at, rows[1].started_at);
    assert_eq!(rows[0].ended_at, rows[1].ended_at);
}

#[test]
fn a_codex_turn_records_its_tokens_and_its_model() {
    let user = User::with_fake_codex();
    user.write_providers("[claude]\nenabled = false\n[codex]\nenabled = true\n");
    project(&user);

    let id = run(&user, "hello");

    let rows = rows(&user);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let model = agent(&user, &id)["taken"]["model"]
        .as_str()
        .map(str::to_owned);
    assert!(model.is_some());
    assert_eq!(rows[0].provider, "codex");
    // Codex counts the cached tokens in its input: the input recorded is the rest, as Claude's.
    assert_eq!(
        rows[0].used,
        Used {
            model,
            input: Some(200),
            output: Some(100),
            cache_read: Some(1000),
            cache_write: Some(0),
            reasoning: Some(40),
            cost_usd: None,
        }
    );
}

#[test]
fn an_acp_turn_records_the_usage_its_answer_gives() {
    let user = User::with_fake_agent();
    std::fs::write(
        user.path("fake/script.json"),
        json!({"turns": [{"when": "hello", "do": [{"say": "Hi."}], "answer_usage": {
            "totalTokens": 1240, "inputTokens": 1000, "outputTokens": 200, "thoughtTokens": 40,
            "cachedReadTokens": 30000
        }}]})
        .to_string(),
    )
    .unwrap();
    project(&user);

    let id = run(&user, "hello");

    let rows = rows(&user);
    assert_eq!(rows.len(), 1, "{rows:?}");
    // OpenCode names no model: the row's is the one the agent had taken.
    let model = agent(&user, &id)["taken"]["model"]
        .as_str()
        .map(str::to_owned);
    assert!(model.is_some());
    assert_eq!(
        rows[0].used,
        Used {
            model,
            input: Some(1000),
            output: Some(200),
            cache_read: Some(30000),
            cache_write: None,
            reasoning: Some(40),
            cost_usd: None,
        }
    );
}

#[test]
fn a_turn_an_agent_says_nothing_about_is_recorded_without_counts() {
    let user = User::with_fake_agent();
    project(&user);

    let id = run(&user, "hello");

    let rows = rows(&user);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].agent, id);
    assert_eq!(
        rows[0].used,
        Used {
            model: agent(&user, &id)["taken"]["model"]
                .as_str()
                .map(str::to_owned),
            ..Used::default()
        }
    );
}

#[test]
fn a_cancelled_turn_records_what_it_used() {
    let user = User::with_fake_claude();
    user.claude_script(
        json!({"turns": [{"when": "long", "do": [{"say": "Working on it. ".repeat(20)}, {"hang": true}]}]}),
    );
    project(&user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "a long task"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let after = answer["after"].as_u64().unwrap();
    user.watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == id.as_str() && e["name"] == "agent.message");

    user.call("agent.cancel", json!({"agent": id})).unwrap();
    turn_ended(&user, &id, after);

    let rows = rows(&user);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].stop_reason, "cancelled");
    assert_eq!(rows[0].used, claude_default());
}
