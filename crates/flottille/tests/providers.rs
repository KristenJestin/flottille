//! The providers of agents (`docs/agents.md`, "Providers"): Claude, OpenCode and Codex side by side, each
//! standing on a fake ACP agent of its own, the provider, model and effort chosen per agent and
//! checked against the providers enabled and ready and the models they offer. No real agent runs.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository with one commit.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start(user: &User, params: Value) -> Result<Value, (String, String)> {
    let mut params = params;
    params["project"] = "app".into();
    user.call("agent.start", params)
        .map(|started| started["agent"].clone())
}

fn agent(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

fn id(agent: &Value) -> String {
    agent["id"].as_str().unwrap().to_owned()
}

/// Every agent, in short.
fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", Value::Null).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

/// The prompts the fake agent of that provider was sent.
fn prompts(user: &User, provider: &str) -> Vec<String> {
    user.provider_received(provider)
        .iter()
        .filter(|entry| entry["received"]["method"] == "session/prompt")
        .map(|entry| {
            entry["received"]["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// How many processes of the fake agent of that provider started.
fn starts(user: &User, provider: &str) -> usize {
    user.provider_received(provider)
        .iter()
        .filter(|entry| entry["started"] == true)
        .count()
}

/// The lead, on Claude, launches a child on OpenCode in its own place; the child reports. With
/// `die`, each dies once first, in its first turn.
fn scripts(user: &User, die: bool) {
    let times = if die { 1 } else { 0 };
    user.provider_script(
        "claude",
        json!({"models": ["fake-small"], "turns": [
            {"when": "LEAD-DIES", "times": times, "do": [{"die": 3}]},
            {"when": "LEAD", "do": [{"mcp": {"tool": "agent_launch", "arguments": {
                "title": "other", "brief": "CHILD-DIES CHILD work", "provider": "opencode",
                "model": "open/medium"}}}]}
        ]}),
    );
    user.provider_script(
        "opencode",
        json!({"models": ["open/small", "open/medium"], "efforts": ["low", "high"], "turns": [
            {"when": "CHILD-DIES", "times": times, "do": [{"die": 3}]},
            {"when": "CHILD", "do": [{"mcp": {"tool": "report", "arguments": {"text": "child done"}}}]}
        ]}),
    );
}

/// The child the lead launched, once it exists.
fn child(user: &User, lead: &str) -> Value {
    let mut found = None;
    eventually("the child", || {
        found = agents(user).into_iter().find(|a| a["parent"] == lead);
        found.is_some()
    });
    found.unwrap()
}

#[test]
fn a_lead_on_one_provider_launches_a_child_on_the_other() {
    let user = User::with_fake_providers();
    scripts(&user, false);
    project(&user);

    let lead = id(&start(&user, json!({"message": "LEAD go"})).unwrap());
    let child = id(&child(&user, &lead));

    eventually("the report reaching the lead", || {
        prompts(&user, "claude")
            .iter()
            .any(|p| p.contains("[flottille:child-report]") && p.contains("child done"))
    });
    let (lead, child) = (agent(&user, &lead), agent(&user, &child));
    assert_eq!(lead["provider"], "claude");
    assert_eq!(child["provider"], "opencode");
    assert_eq!(child["chosen"]["model"], "open/medium");
    assert_eq!(child["taken"]["model"], "open/medium");
    // Each ran on its own provider's agent, and only there.
    assert!(
        prompts(&user, "opencode")
            .iter()
            .any(|p| p.contains("CHILD work"))
    );
    assert!(
        !prompts(&user, "claude")
            .iter()
            .any(|p| p.contains("[flottille:brief]"))
    );
    // The OpenCode agent ran in its bare environment.
    let started = user
        .provider_received("opencode")
        .into_iter()
        .find(|entry| entry["started"] == true)
        .unwrap();
    let env = &started["env"];
    assert_eq!(env["OPENCODE_DISABLE_PROJECT_CONFIG"], "1");
    assert_eq!(env["OPENCODE_SERVER_PASSWORD"], "set");
    assert!(
        env["XDG_CONFIG_HOME"]
            .as_str()
            .unwrap()
            .starts_with(user.path("data/flottille").to_str().unwrap()),
        "{env}"
    );
    assert!(env["OPENCODE_CONFIG_CONTENT"].is_string(), "{env}");
    let claude = user
        .provider_received("claude")
        .into_iter()
        .find(|entry| entry["started"] == true)
        .unwrap();
    assert!(claude["env"].get("OPENCODE_CONFIG_CONTENT").is_none());
}

#[test]
fn both_run_report_and_are_replaced_on_their_own_provider() {
    let user = User::with_fake_providers();
    scripts(&user, true);
    project(&user);

    let lead = id(&start(&user, json!({"message": "LEAD-DIES LEAD go"})).unwrap());
    let child = id(&child(&user, &lead));
    eventually("the report reaching the lead", || {
        prompts(&user, "claude")
            .iter()
            .any(|p| p.contains("[flottille:child-report]") && p.contains("child done"))
    });

    let (lead, child) = (agent(&user, &lead), agent(&user, &child));
    assert_eq!(
        (lead["epoch"].as_u64(), child["epoch"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(lead["provider"], "claude");
    assert_eq!(child["provider"], "opencode");
    // Two processes each: the one that died, and its replacement, on the same provider.
    assert_eq!(starts(&user, "claude"), 2);
    assert_eq!(starts(&user, "opencode"), 2);
    assert!(child["reported_at"].is_u64(), "{child}");
}

#[test]
fn a_disabled_or_unready_provider_is_refused_at_start_with_a_sentence_naming_the_ready_ones() {
    // OpenCode disabled.
    let user = User::with_fake_providers();
    user.write_providers("[opencode]\nenabled = false\n");
    project(&user);

    let (code, message) =
        start(&user, json!({"message": "go", "provider": "opencode"})).unwrap_err();

    assert_eq!(code, "refused");
    assert!(message.contains("opencode is not enabled"), "{message}");
    assert!(
        message.contains("the ready providers are claude"),
        "{message}"
    );
    assert!(agents(&user).is_empty());

    // OpenCode enabled, and not on PATH: not ready.
    let user = User::new();
    std::fs::create_dir(user.path("fake-claude")).unwrap();
    std::os::unix::fs::symlink(common::fake_agent(), user.path("fake-claude/agent")).unwrap();
    std::fs::create_dir(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let (agent, bin) = (user.path("fake-claude/agent"), user.path("bin"));
    let user = user
        .with_env("FLOTTILLE_AGENT_COMMAND_CLAUDE", agent.to_str().unwrap())
        .with_env("PATH", bin.to_str().unwrap());
    user.write_providers("[opencode]\nenabled = true\n");
    project(&user);

    let (code, message) =
        start(&user, json!({"message": "go", "provider": "opencode"})).unwrap_err();

    assert_eq!(code, "refused");
    assert!(message.contains("opencode is not ready"), "{message}");
    assert!(message.contains("`opencode` is not on PATH"), "{message}");
    assert!(
        message.contains("the ready providers are claude"),
        "{message}"
    );

    // No provider ready at all.
    user.write_providers("[claude]\nenabled = false\n[opencode]\nenabled = true\n");
    let (_, message) = start(&user, json!({"message": "go"})).unwrap_err();
    assert!(message.contains("no provider is ready"), "{message}");
    assert!(message.contains("flottille doctor"), "{message}");
    assert!(agents(&user).is_empty());
}

#[test]
fn an_unknown_model_is_refused_naming_the_provider_s_models() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}));
    project(&user);

    // Before any session the models are not known: the agent fails once its session says so,
    // and what it offered is kept.
    let first = id(&start(&user, json!({"message": "hello", "model": "fake-huge"})).unwrap());
    eventually("the first agent failing", || {
        agent(&user, &first)["state"] == "failed"
    });

    let (code, message) =
        start(&user, json!({"message": "hello", "model": "fake-huge"})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.contains("claude offers no model fake-huge"),
        "{message}"
    );
    assert!(message.contains("fake-small, fake-large"), "{message}");

    // An effort is checked against the efforts the model was seen to offer: fake-small's are
    // known, the session had it; fake-large's are not yet.
    let listed = user.call("provider.list", Value::Null).unwrap();
    assert_eq!(
        listed["providers"][0]["models"][0],
        json!({"id": "fake-small", "efforts": ["low", "high"], "default": true})
    );
    assert_eq!(listed["providers"][0]["models"][1]["efforts"], Value::Null);
    let (_, message) = start(
        &user,
        json!({"message": "hello", "model": "fake-small", "effort": "max"}),
    )
    .unwrap_err();
    assert!(
        message.contains("claude's model fake-small offers no effort max"),
        "{message}"
    );
    assert!(message.contains("low, high"), "{message}");
    assert_eq!(agents(&user).len(), 1);
    // An effort the model offers starts; on a model whose efforts are not known yet, any effort
    // is taken, and the session says whether it offers it.
    start(
        &user,
        json!({"message": "hello", "model": "fake-small", "effort": "high"}),
    )
    .unwrap();
    start(
        &user,
        json!({"message": "hello", "model": "fake-large", "effort": "high"}),
    )
    .unwrap();
}

#[test]
fn an_effort_without_a_model_is_checked_against_the_default_model_s_efforts() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}));
    project(&user);
    // Not known before a session: taken.
    let first = id(&start(&user, json!({"message": "hello", "effort": "high"})).unwrap());
    eventually("the session", || agent(&user, &first)["state"] == "idle");

    let (code, message) = start(&user, json!({"message": "hello", "effort": "max"})).unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("claude's default model fake-small offers no effort max"),
        "{message}"
    );
    assert!(message.contains("low, high"), "{message}");
    start(&user, json!({"message": "hello", "effort": "low"})).unwrap();
}

#[test]
fn an_unknown_provider_is_refused_naming_the_providers() {
    let user = User::with_fake_agent();
    project(&user);

    let (code, message) = start(&user, json!({"message": "go", "provider": "gemini"})).unwrap_err();

    assert_eq!(code, "refused");
    assert_eq!(
        message,
        "there is no provider gemini: the providers are claude, opencode, codex"
    );
}

#[test]
fn the_providers_method_lists_every_provider_enabled_ready_and_why_its_models_and_the_project_default()
 {
    let user = User::with_fake_agent();
    user.fake_script(json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}));
    let folder = project(&user);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("model = \"fake-large\"\n{text}")).unwrap();
    let _ = folder;

    let listed = user
        .call("provider.list", json!({"project": "app"}))
        .unwrap();
    let providers = listed["providers"].as_array().unwrap();
    let names: Vec<&str> = providers
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["claude", "opencode", "codex"]);
    let claude = &providers[0];
    assert_eq!(claude["enabled"], true);
    assert_eq!(claude["ready"], true);
    assert_eq!(claude["models"], Value::Null, "not known before a session");
    let opencode = &providers[1];
    assert_eq!(opencode["enabled"], false);
    assert!(
        opencode["why"].as_str().unwrap().contains("not enabled"),
        "{opencode}"
    );
    assert_eq!(
        listed["project"],
        json!({"name": "app", "provider": "claude", "model": "fake-large"})
    );

    let started = id(&start(&user, json!({"message": "hello"})).unwrap());
    eventually("the session", || agent(&user, &started)["state"] == "idle");

    let listed = user.call("provider.list", Value::Null).unwrap();
    assert_eq!(
        listed["providers"][0]["models"],
        json!([
            {"id": "fake-small", "efforts": ["low", "high"], "default": true},
            {"id": "fake-large", "efforts": ["low", "high"]}
        ])
    );
    assert!(listed.get("project").is_none());
    let (code, _) = user
        .call("provider.list", json!({"project": "nope"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
}

#[test]
fn a_project_s_default_provider_and_model_are_taken_when_an_agent_is_started_without_them() {
    let user = User::with_fake_providers();
    user.provider_script("opencode", json!({"models": ["open/small", "open/medium"]}));
    project(&user);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("provider = \"opencode\"\nmodel = \"open/medium\"\n{text}"),
    )
    .unwrap();

    let defaulted = id(&start(&user, json!({"message": "hello"})).unwrap());
    // On another provider, the project's model is not that provider's.
    let other = id(&start(&user, json!({"message": "hello", "provider": "claude"})).unwrap());

    eventually("both sessions", || {
        agent(&user, &defaulted)["state"] == "idle" && agent(&user, &other)["state"] == "idle"
    });
    let defaulted = agent(&user, &defaulted);
    assert_eq!(defaulted["provider"], "opencode");
    assert_eq!(defaulted["chosen"]["model"], "open/medium");
    assert_eq!(defaulted["taken"]["model"], "open/medium");
    let other = agent(&user, &other);
    assert_eq!(other["provider"], "claude");
    assert_eq!(other["chosen"]["model"], Value::Null);
}

#[test]
fn agent_show_list_and_tree_give_each_agent_s_provider_and_model() {
    let user = User::with_fake_providers();
    user.provider_script(
        "opencode",
        json!({"models": ["open/small", "open/medium"], "efforts": ["low", "high"]}),
    );
    project(&user);

    let output = user.run(&[
        "agent",
        "start",
        "app",
        "hello",
        "--provider",
        "opencode",
        "--model",
        "open/medium",
        "--effort",
        "high",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let started: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = id(&started["agent"]);
    eventually("the session", || agent(&user, &id)["state"] == "idle");
    let shown = agent(&user, &id);
    assert_eq!(shown["provider"], "opencode");
    assert_eq!(shown["taken"]["effort"], "high");

    let text = |args: &[&str]| {
        let output = user.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let show = text(&["agent", "show", &id]);
    assert!(show.contains("provider: opencode"), "{show}");
    assert!(show.contains("model: open/medium"), "{show}");
    for listing in [text(&["agent", "list"]), text(&["agent", "tree", &id])] {
        assert!(listing.contains("opencode"), "{listing}");
        assert!(listing.contains("open/medium"), "{listing}");
    }
    let listed: Value = serde_json::from_str(&text(&["agent", "list", "--json"])).unwrap();
    assert_eq!(listed["agents"][0]["provider"], "opencode");
    assert_eq!(listed["agents"][0]["model"], "open/medium");
}

#[test]
fn a_claude_agent_without_a_login_file_still_starts() {
    // Signed in where the daemon cannot see it (the macOS Keychain, say): the start is not refused.
    let user = User::with_fake_claude();
    std::fs::remove_file(user.path("home/.claude/.credentials.json")).unwrap();
    project(&user);

    let started = id(&start(&user, json!({"message": "hello"})).unwrap());

    eventually("the session", || agent(&user, &started)["state"] == "idle");
}

#[test]
fn an_agent_whose_provider_was_disabled_fails_at_its_next_start_saying_so() {
    let user = User::with_fake_providers().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    project(&user);
    let started = id(&start(&user, json!({"message": "hello", "provider": "opencode"})).unwrap());
    let events = |user: &User| {
        user.call("agent.show", json!({"agent": started})).unwrap()["events"]
            .as_array()
            .unwrap()
            .clone()
    };
    eventually("the release", || {
        events(&user).iter().any(|e| e["name"] == "agent.released")
    });

    user.write_providers("[opencode]\nenabled = false\n");
    user.call("agent.send", json!({"agent": started, "message": "again"}))
        .unwrap();

    eventually("the failure", || {
        agent(&user, &started)["state"] == "failed"
    });
    let failed = events(&user);
    let reason = failed.iter().find(|e| e["name"] == "agent.failed").unwrap()["data"]["reason"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        reason.contains("provider opencode is not enabled"),
        "{reason}"
    );
    // Not started again behind the user's back.
    assert_eq!(starts(&user, "opencode"), 1);
}

/// A user whose `opencode` on PATH answers `--version` with `version` and is otherwise the fake
/// agent, its folder `fake-opencode`; Claude stands on the fake agent too.
fn user_with_opencode(version: &str) -> User {
    let user = User::new();
    for dir in ["bin", "fake-claude", "fake-opencode"] {
        std::fs::create_dir(user.path(dir)).unwrap();
    }
    std::os::unix::fs::symlink(common::fake_agent(), user.path("fake-claude/agent")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let script = user.path("bin/opencode");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo {version}; exit 0; fi\n\
             FLOTTILLE_FAKE_AGENT_DIR={} exec {}\n",
            user.path("fake-opencode").display(),
            common::fake_agent().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let (claude, bin) = (user.path("fake-claude/agent"), user.path("bin"));
    let user = user
        .with_env("FLOTTILLE_AGENT_COMMAND_CLAUDE", claude.to_str().unwrap())
        .with_env("PATH", bin.to_str().unwrap());
    user.write_providers("[opencode]\nenabled = true\n");
    user
}

#[test]
fn opencode_of_another_version_is_refused_unless_the_user_allows_it() {
    let user = user_with_opencode("1.20.0");
    project(&user);

    // Its version is asked when its process would start: the agent fails, saying so.
    let first = id(&start(&user, json!({"message": "hello", "provider": "opencode"})).unwrap());
    eventually("the failure", || agent(&user, &first)["state"] == "failed");
    let shown = user.call("agent.show", json!({"agent": first})).unwrap();
    let reason = shown["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "agent.failed")
        .unwrap()["data"]["reason"]
        .as_str()
        .unwrap()
        .to_owned();
    for part in ["1.20.0", "1.19.0", "allow_unqualified_version = true"] {
        assert!(reason.contains(part), "{part}: {reason}");
    }
    assert_eq!(starts(&user, "opencode"), 0);

    // Once known, a start is refused at once.
    let (code, message) =
        start(&user, json!({"message": "hello", "provider": "opencode"})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("opencode is 1.20.0"), "{message}");
    assert!(
        message.contains("the ready providers are claude"),
        "{message}"
    );
    let doctor: Value = serde_json::from_slice(&user.run(&["doctor", "--json"]).stdout).unwrap();
    assert_eq!(doctor["providers"][1]["ready"], false, "{doctor}");
    assert!(
        doctor["providers"][1]["why"]
            .as_str()
            .unwrap()
            .contains("allow_unqualified_version"),
        "{doctor}"
    );

    // The user allows it: it runs, with a warning that names both versions.
    user.write_providers("[opencode]\nenabled = true\nallow_unqualified_version = true\n");
    let allowed = id(&start(&user, json!({"message": "hello", "provider": "opencode"})).unwrap());
    eventually("the session", || agent(&user, &allowed)["state"] == "idle");
    assert_eq!(starts(&user, "opencode"), 1);
    let doctor: Value = serde_json::from_slice(&user.run(&["doctor", "--json"]).stdout).unwrap();
    assert_eq!(doctor["providers"][1]["ready"], true, "{doctor}");
    let warnings = doctor["warnings"].to_string();
    assert!(
        warnings.contains("1.20.0") && warnings.contains("1.19.0"),
        "{doctor}"
    );
}

#[test]
fn opencode_of_the_qualified_version_runs() {
    let user = user_with_opencode("1.19.0");
    project(&user);

    let started = id(&start(&user, json!({"message": "hello", "provider": "opencode"})).unwrap());

    eventually("the session", || agent(&user, &started)["state"] == "idle");
    let started = user
        .provider_received("opencode")
        .into_iter()
        .find(|entry| entry["started"] == true)
        .unwrap();
    assert_eq!(started["env"]["OPENCODE_DISABLE_PROJECT_CONFIG"], "1");
    // Its state and cache are Flottille's, and its configuration folder cannot be written, so
    // that it installs no package in it.
    let own = user.path("data/flottille/agents/opencode");
    for (name, sub) in [("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")] {
        assert_eq!(started["env"][name], own.join(sub).to_str().unwrap());
        assert!(own.join(sub).is_dir(), "{sub}");
    }
    assert_eq!(common::mode(own.join("config/opencode")), 0o500);
}

#[test]
fn the_models_are_forgotten_when_the_api_keys_set_change_and_learned_again_from_a_session() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"models": ["fake-small", "fake-large"]}));
    project(&user);
    let first = id(&start(&user, json!({"message": "hello"})).unwrap());
    eventually("the session", || agent(&user, &first)["state"] == "idle");
    let models = |user: &User| {
        user.call("provider.list", Value::Null).unwrap()["providers"][0]["models"].clone()
    };
    assert!(models(&user).is_array());

    // The daemon again, with an API key of Claude's set: what it offers may differ.
    assert!(user.run(&["daemon", "stop"]).status.success());
    let user = user.with_env("ANTHROPIC_API_KEY", "sk-ant-invented-0123456789");
    assert_eq!(models(&user), Value::Null);

    let second = id(&start(&user, json!({"message": "hello"})).unwrap());
    eventually("the second session", || {
        agent(&user, &second)["state"] == "idle"
    });
    assert_eq!(models(&user)[1]["id"], "fake-large");
}

/// The user's `providers.toml`, as the daemon left it.
fn providers_file(user: &User) -> String {
    std::fs::read_to_string(user.path("config/flottille/providers.toml")).unwrap()
}

/// The provider of that name, as `provider.list` gives it.
fn listed(user: &User, name: &str) -> Value {
    user.call("provider.list", Value::Null).unwrap()["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()
        .clone()
}

#[test]
fn a_provider_disabled_then_enabled_is_written_in_providers_toml_with_its_comments_and_announced() {
    let user = User::with_fake_providers();
    user.write_providers(
        "# Which agents run here.\n[opencode]\nenabled = true # tried in October\n\
         allow_unqualified_version = false\n",
    );

    let disabled = user
        .call("provider.disable", json!({"name": "opencode"}))
        .unwrap();

    assert_eq!(disabled, json!({"name": "opencode", "enabled": false}));
    let text = providers_file(&user);
    assert!(text.contains("# Which agents run here."), "{text}");
    assert!(
        text.contains("enabled = false # tried in October"),
        "{text}"
    );
    assert!(text.contains("allow_unqualified_version = false"), "{text}");
    let opencode = listed(&user, "opencode");
    assert_eq!(opencode["enabled"], false);
    assert!(
        opencode["why"]
            .as_str()
            .unwrap()
            .starts_with("is not enabled"),
        "{opencode}"
    );

    // Disabled again: nothing changes, nothing is announced.
    let again = user
        .call("provider.disable", json!({"name": "opencode"}))
        .unwrap();
    assert_eq!(again, json!({"name": "opencode", "enabled": false}));
    // Claude has no table yet: one is added for it.
    user.call("provider.disable", json!({"name": "claude"}))
        .unwrap();
    user.call("provider.enable", json!({"name": "claude"}))
        .unwrap();
    user.call("provider.enable", json!({"name": "opencode"}))
        .unwrap();

    let text = providers_file(&user);
    assert!(text.contains("[claude]\nenabled = true"), "{text}");
    assert!(text.contains("enabled = true # tried in October"), "{text}");
    assert_eq!(listed(&user, "claude")["enabled"], true);
    assert_eq!(listed(&user, "opencode")["enabled"], true);
    let watch = user.watch(&["--after", "0"]);
    let changes: Vec<Value> = watch
        .until(|e| {
            e["name"] == "provider.changed"
                && e["data"]["name"] == "opencode"
                && e["data"]["enabled"] == true
        })
        .into_iter()
        .filter(|e| e["name"] == "provider.changed")
        .map(|e| e["data"].clone())
        .collect();
    assert_eq!(
        changes,
        [
            json!({"name": "opencode", "enabled": false}),
            json!({"name": "claude", "enabled": false}),
            json!({"name": "claude", "enabled": true}),
            json!({"name": "opencode", "enabled": true}),
        ]
    );
}

#[test]
fn an_unknown_provider_or_a_providers_toml_that_cannot_be_read_is_refused_and_left_as_it_is() {
    let user = User::with_fake_providers();

    let (code, message) = user
        .call("provider.enable", json!({"name": "gemini"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
    assert_eq!(
        message,
        "there is no provider gemini: the providers are claude, opencode, codex"
    );
    let (code, _) = user
        .call(
            "provider.enable",
            json!({"name": "claude", "enabled": true}),
        )
        .unwrap_err();
    assert_eq!(code, "invalid-params");

    let broken = "[opencode]\nenabled = \"yes\"\n";
    user.write_providers(broken);
    let (code, message) = user
        .call("provider.disable", json!({"name": "opencode"}))
        .unwrap_err();
    assert_eq!(code, "invalid-config");
    assert!(
        message.starts_with("providers.toml: opencode.enabled: "),
        "{message}"
    );
    assert_eq!(providers_file(&user), broken);
}

#[test]
fn without_a_providers_toml_enabling_a_provider_writes_one() {
    let user = User::with_fake_providers();
    std::fs::remove_file(user.path("config/flottille/providers.toml")).unwrap();

    user.call("provider.enable", json!({"name": "opencode"}))
        .unwrap();

    assert_eq!(providers_file(&user), "[opencode]\nenabled = true\n");
    assert_eq!(listed(&user, "opencode")["ready"], true);
}

#[test]
fn the_provider_command_enables_and_disables_a_provider() {
    let user = User::with_fake_providers();

    let output = user.run(&["provider", "disable", "opencode"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "opencode is disabled: its agents running now go on, and fail at their next start.\n"
    );
    assert_eq!(listed(&user, "opencode")["enabled"], false);

    let output = user.run(&["provider", "enable", "opencode"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "opencode is enabled.\n"
    );

    let output = user.run(&["provider", "enable", "gemini"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "flottille: there is no provider gemini: the providers are claude, opencode, codex\n"
    );
}

#[test]
fn a_providers_toml_behind_a_symbolic_link_is_still_a_link_after_a_switch_its_mode_unchanged() {
    use std::os::unix::fs::PermissionsExt;
    let user = User::with_fake_providers();
    let link = user.path("config/flottille/providers.toml");
    let real = user.path("dotfiles/flottille/providers.toml");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(
        &real,
        "# Kept with the dotfiles.\n[opencode]\nenabled = true\n",
    )
    .unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    user.call("provider.disable", json!({"name": "opencode"}))
        .unwrap();

    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced by a file"
    );
    assert_eq!(std::fs::read_link(&link).unwrap(), real);
    let text = std::fs::read_to_string(&real).unwrap();
    assert!(text.contains("# Kept with the dotfiles."), "{text}");
    assert!(text.contains("enabled = false"), "{text}");
    assert_eq!(common::mode(real.clone()) & 0o777, 0o640);
    for dir in [link.parent().unwrap(), real.parent().unwrap()] {
        let names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|name| !name.ends_with(".tmp")),
            "{names:?}"
        );
    }
    assert_eq!(listed(&user, "opencode")["enabled"], false);
}

/// Claude, OpenCode and Codex enabled, each on a fake agent of its own.
fn user_with_codex() -> User {
    let user = User::with_fake_providers();
    user.write_providers("[opencode]\nenabled = true\n[codex]\nenabled = true\n");
    user
}

#[test]
fn a_lead_on_claude_launches_a_child_on_codex() {
    let user = user_with_codex();
    user.provider_script(
        "claude",
        json!({"models": ["fake-small"], "turns": [
            {"when": "LEAD", "do": [{"mcp": {"tool": "agent_launch", "arguments": {
                "title": "other", "brief": "CHILD work", "provider": "codex",
                "model": "gpt-fake-mini"}}}]}
        ]}),
    );
    user.provider_script(
        "codex",
        json!({"models": ["gpt-fake", "gpt-fake-mini"], "efforts": ["low", "high"], "turns": [
            {"when": "CHILD", "do": [{"mcp": {"tool": "report", "arguments": {"text": "child done"}}}]}
        ]}),
    );
    project(&user);

    let lead = id(&start(&user, json!({"message": "LEAD go"})).unwrap());
    let child = id(&child(&user, &lead));

    eventually("the report reaching the lead", || {
        prompts(&user, "claude")
            .iter()
            .any(|p| p.contains("[flottille:child-report]") && p.contains("child done"))
    });
    let child = agent(&user, &child);
    assert_eq!(child["provider"], "codex");
    assert_eq!(child["taken"]["model"], "gpt-fake-mini");
    assert!(
        prompts(&user, "codex")
            .iter()
            .any(|p| p.contains("CHILD work"))
    );
    // Its session carried Codex's options: its instructions, and its tools as Codex names them.
    let opened = user
        .provider_received("codex")
        .into_iter()
        .find(|entry| entry["received"]["method"] == "session/new")
        .unwrap();
    let options = &opened["received"]["params"]["_meta"]["codex"];
    assert!(options["instructions"].is_string(), "{options}");
    let tools: Vec<&str> = options["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert!(tools.contains(&"mcp__flottille__report"), "{tools:?}");
    assert!(tools.contains(&"mcp__flottille__ask_parent"), "{tools:?}");
    // Codex's own folder of Flottille's, the user's login linked in it, never opened.
    let link = user.path("data/flottille/agents/codex/codex/auth.json");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        user.path("home/.codex/auth.json")
    );
    assert_eq!(
        common::mode(user.path("data/flottille/agents/codex/home")),
        0o700
    );
}

#[test]
fn a_disabled_or_unready_codex_is_refused_naming_the_ready_providers() {
    // Disabled, as it is by default.
    let user = User::with_fake_providers();
    project(&user);

    let (code, message) = start(&user, json!({"message": "go", "provider": "codex"})).unwrap_err();

    assert_eq!(code, "refused");
    assert!(message.contains("codex is not enabled"), "{message}");
    assert!(
        message.contains("the ready providers are claude, opencode"),
        "{message}"
    );

    // Enabled, and not on PATH.
    let user = User::new();
    std::fs::create_dir(user.path("fake-claude")).unwrap();
    std::os::unix::fs::symlink(common::fake_agent(), user.path("fake-claude/agent")).unwrap();
    std::fs::create_dir(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let (agent, bin) = (user.path("fake-claude/agent"), user.path("bin"));
    let user = user
        .with_env("FLOTTILLE_AGENT_COMMAND_CLAUDE", agent.to_str().unwrap())
        .with_env("PATH", bin.to_str().unwrap());
    user.write_providers("[codex]\nenabled = true\n");
    project(&user);

    let (_, message) = start(&user, json!({"message": "go", "provider": "codex"})).unwrap_err();

    assert!(message.contains("codex is not ready"), "{message}");
    assert!(message.contains("`codex` is not on PATH"), "{message}");
    assert!(
        message.contains("the ready providers are claude"),
        "{message}"
    );

    // On PATH, of a version its bare mode was not qualified on.
    std::fs::write(
        user.path("bin/codex"),
        "#!/bin/sh\n[ \"$1\" = --version ] && echo 'codex-cli 0.163.0'\n",
    )
    .unwrap();
    std::fs::set_permissions(
        user.path("bin/codex"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    // Its version is asked once: by the list here, else at its process's start.
    user.call("provider.list", Value::Null).unwrap();

    let (_, message) = start(&user, json!({"message": "go", "provider": "codex"})).unwrap_err();

    assert!(message.contains("codex is 0.163.0"), "{message}");
    assert!(message.contains("only 0.162.0 was qualified"), "{message}");
    assert!(agents(&user).is_empty());
}

#[test]
fn an_unknown_model_is_refused_naming_codex_s_models() {
    let user = user_with_codex();
    user.provider_script(
        "codex",
        json!({"models": ["gpt-fake", "gpt-fake-mini"], "efforts": ["low", "high"]}),
    );
    project(&user);
    let first = id(&start(&user, json!({"message": "hello", "provider": "codex"})).unwrap());
    eventually("Codex's models known", || {
        listed(&user, "codex")["models"].is_array()
    });

    let (code, message) = start(
        &user,
        json!({"message": "hello", "provider": "codex", "model": "gpt-huge"}),
    )
    .unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message
            .contains("provider codex offers no model gpt-huge: it offers gpt-fake, gpt-fake-mini"),
        "{message}"
    );
    assert_eq!(agents(&user).len(), 1);
    assert_eq!(agent(&user, &first)["provider"], "codex");
}
