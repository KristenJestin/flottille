//! The vault (`docs/vault.md`, #382): the user's secrets, kept by the keyring, that agents use
//! and never see. Against the real daemon with the fake ACP agent and the in-memory keyring the
//! tests' configuration chooses (`User::new`): the user's own keyring is never reached. No real
//! agent ever runs here.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// The secret set in the vault, which nothing may give back.
const SECRET: &str = "v4ult-s3cret-for-jira";

/// A project `name` over a throwaway repository with one commit.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The entry `jira`, of every project, with all its metadata.
fn set_jira(user: &User) -> Value {
    user.call(
        "vault.set",
        json!({"name": "jira", "secret": SECRET, "description": "Logs time on Jira",
            "url": "https://tracker.example.com", "username": "dev@example.com"}),
    )
    .unwrap()
}

/// Starts an agent of `project` with a first message and waits for its first turn's end: its
/// id.
fn agent(user: &User, project: &str, message: &str) -> String {
    let answer = user
        .call("agent.run", json!({"project": project, "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let after = answer["after"].as_u64().unwrap();
    let of = id.clone();
    user.watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == of.as_str() && e["name"] == "agent.turn_ended");
    id
}

/// Every file under `dir` holding `needle`, as a path.
fn files_holding(dir: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_holding(&path, needle));
        } else if let Ok(bytes) = std::fs::read(&path)
            && bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        {
            found.push(path.display().to_string());
        }
    }
    found
}

#[test]
fn vault_list_shows_metadata_only() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "start", "do": [{"say": "ready"}]}]}));
    project(&user, "app");
    let set = set_jira(&user);
    assert_eq!(set["name"], "jira");

    let listed = user.call("vault.list", json!({})).unwrap();
    assert_eq!(
        listed["entries"],
        json!([{"name": "jira", "description": "Logs time on Jira",
            "url": "https://tracker.example.com", "username": "dev@example.com",
            "project": null, "for": null}]),
        "{listed}"
    );

    // An agent lists the same metadata with its tool, and nothing more.
    agent(&user, "app", "start");
    let (url, authorization) = server_of(&user, 0);
    let mcp = Mcp::initialize(&url, &authorization);
    assert!(mcp.names().iter().any(|name| name == "vault"), "listed");
    let answer = mcp.call("vault", json!({"action": "list"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("jira"), "{}", answer.text);
    assert!(answer.text.contains("Logs time on Jira"), "{}", answer.text);
    assert!(
        answer.text.contains("https://tracker.example.com"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("dev@example.com"));
    assert!(!answer.text.contains(SECRET), "{}", answer.text);
}

#[test]
fn a_secret_is_never_returned_by_any_client() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "start", "do": [{"say": "ready"}]}]}));
    project(&user, "app");
    let set = set_jira(&user);
    // Replaced: the answer of a replacement says no more than the first.
    let replaced = user
        .call(
            "vault.set",
            json!({"name": "jira", "secret": format!("{SECRET}-2"), "description": "Logs time"}),
        )
        .unwrap();
    let mut said = vec![set.to_string(), replaced.to_string()];

    // The methods.
    said.push(user.call("vault.list", json!({})).unwrap().to_string());
    said.push(
        user.call("vault.list", json!({"project": "app"}))
            .unwrap()
            .to_string(),
    );
    // A refusal that names a field of the call: its message never repeats the secret.
    for refused in [
        json!({"name": "other", "secret": SECRET, "project": "nowhere"}),
        json!({"name": "bad name!", "secret": SECRET}),
        json!({"name": "other", "secret": SECRET, "url": 7}),
        json!({"name": "other", "secret": 12345678}),
        json!({"name": "other", "secret": "short"}),
    ] {
        let (_, message) = user.call("vault.set", refused).unwrap_err();
        said.push(message);
    }
    // The command line, in words and in JSON.
    for args in [&["vault", "list"][..], &["vault", "list", "--json"]] {
        let output = user.run(args);
        assert!(output.status.success(), "{output:?}");
        said.push(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    // The events, every one since the first.
    let events = user.watch(&["--after", "0"]).within(Duration::from_secs(1));
    assert!(
        events.iter().any(|e| e["name"] == "vault.changed"),
        "{events:?}"
    );
    said.push(serde_json::to_string(&events).unwrap());
    // An agent's tool, whatever action it asks.
    agent(&user, "app", "start");
    let (url, authorization) = server_of(&user, 0);
    let mcp = Mcp::initialize(&url, &authorization);
    for asked in [
        json!({"action": "list"}),
        json!({"action": "show", "name": "jira"}),
        json!({"action": "get", "name": "jira"}),
        json!({"action": "list", "secret": true}),
    ] {
        said.push(mcp.call("vault", asked).text);
    }

    for text in &said {
        assert!(!text.contains(SECRET), "a secret came back: {text}");
    }
    // Nor is it written anywhere of Flottille's: its store, its log, its files.
    eventually("the store is written", || {
        user.path("state/flottille").join("flottille.db").exists()
    });
    for folder in ["state", "config", "data", "runtime", "home"] {
        let holding = files_holding(&user.path(folder), SECRET);
        assert!(holding.is_empty(), "{holding:?}");
    }
}

#[test]
fn a_project_entry_is_not_seen_by_another_project() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "start", "do": [{"say": "ready"}]}]}));
    project(&user, "app");
    project(&user, "other");
    user.call(
        "vault.set",
        json!({"name": "deploy", "secret": SECRET, "project": "app",
            "description": "The app's deploy key"}),
    )
    .unwrap();
    user.call(
        "vault.set",
        json!({"name": "shared", "secret": "an0ther-s3cret", "description": "For all"}),
    )
    .unwrap();

    let names = |listed: Value| -> Vec<String> {
        listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        names(user.call("vault.list", json!({"project": "app"})).unwrap()),
        ["deploy", "shared"]
    );
    assert_eq!(
        names(
            user.call("vault.list", json!({"project": "other"}))
                .unwrap()
        ),
        ["shared"]
    );

    // An agent of `other` lists only what every project sees.
    agent(&user, "other", "start");
    let (url, authorization) = server_of(&user, 0);
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = mcp.call("vault", json!({"action": "list"}));
    assert!(answer.text.contains("shared"), "{}", answer.text);
    assert!(!answer.text.contains("deploy"), "{}", answer.text);

    // And an agent of `app`, both.
    agent(&user, "app", "start");
    let (url, authorization) = server_of(&user, 1);
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = mcp.call("vault", json!({"action": "list"}));
    assert!(answer.text.contains("shared"), "{}", answer.text);
    assert!(answer.text.contains("deploy"), "{}", answer.text);
}

#[test]
fn a_secret_value_is_masked_in_what_is_stored() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "tell", "do": [{"say": format!("the jira token is {SECRET}, keep it")}]}
    ]}));
    project(&user, "app");
    set_jira(&user);

    let id = agent(&user, "app", "tell");

    let page = user
        .call("agent.read", json!({"agent": id, "after": 0}))
        .unwrap();
    let text = page.to_string();
    assert!(!text.contains(SECRET), "{text}");
    assert!(
        text.contains("the jira token is [masked], keep it"),
        "{text}"
    );
    let events = user.watch(&["--after", "0"]).within(Duration::from_secs(1));
    let events = serde_json::to_string(&events).unwrap();
    assert!(!events.contains(SECRET), "{events}");
    assert!(files_holding(&user.path("state"), SECRET).is_empty());

    user.call("vault.remove", json!({"name": "jira"})).unwrap();
    assert_eq!(
        user.call("vault.list", json!({})).unwrap()["entries"],
        json!([])
    );
}

#[test]
fn vault_set_reads_stdin_not_an_argument() {
    let user = User::new();

    // A secret given as an argument would be in the shell's history and the process list.
    let refused = user.run(&["vault", "set", "jira", SECRET]);
    assert!(!refused.status.success(), "{refused:?}");
    assert_eq!(
        user.call("vault.list", json!({})).unwrap()["entries"],
        json!([])
    );

    let output = user.run_with_input(
        &["vault", "set", "jira", "--description", "Logs time on Jira"],
        &format!("{SECRET}\n"),
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("jira"), "{stdout}");
    assert!(!stdout.contains(SECRET), "{stdout}");
    let listed = user.call("vault.list", json!({})).unwrap();
    assert_eq!(listed["entries"][0]["name"], "jira");
    assert_eq!(listed["entries"][0]["description"], "Logs time on Jira");

    // Nothing on stdin is refused, not kept empty.
    let empty = user.run_with_input(&["vault", "set", "empty"], "");
    assert!(!empty.status.success(), "{empty:?}");

    let removed = user.run(&["vault", "remove", "jira", "--json"]);
    assert!(removed.status.success(), "{removed:?}");
    assert_eq!(
        user.call("vault.list", json!({})).unwrap()["entries"],
        json!([])
    );
}

#[test]
fn a_secret_set_while_an_agent_runs_is_masked_in_what_it_says() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "tell", "do": [
            {"wait": 3000},
            {"say": format!("the jira token is {SECRET}, keep it")}
        ]}
    ]}));
    project(&user, "app");
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "tell"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let after = answer["after"].as_u64().unwrap();

    // The agent's process runs, its masking already made: the secret comes after it.
    let of = id.clone();
    user.watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == of.as_str() && e["name"] == "agent.prompted");
    set_jira(&user);
    let of = id.clone();
    user.watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == of.as_str() && e["name"] == "agent.turn_ended");

    let page = user
        .call("agent.read", json!({"agent": id, "after": 0}))
        .unwrap()
        .to_string();
    assert!(!page.contains(SECRET), "{page}");
    assert!(
        page.contains("the jira token is [masked], keep it"),
        "{page}"
    );
    let events = user.watch(&["--after", "0"]).within(Duration::from_secs(1));
    let changed = events
        .iter()
        .position(|e| e["name"] == "vault.changed")
        .unwrap();
    let said = events
        .iter()
        .rposition(|e| e["name"] == "agent.message")
        .unwrap();
    assert!(
        changed < said,
        "the secret was set before it was said: {events:?}"
    );
    let events = serde_json::to_string(&events).unwrap();
    assert!(!events.contains(SECRET), "{events}");
    assert!(files_holding(&user.path("state"), SECRET).is_empty());
}

#[test]
fn a_remove_the_locked_keyring_refuses_keeps_the_entry_to_try_again() {
    let user = User::new();
    set_jira(&user);
    let keyring = user.path("config/flottille/keyring.toml");
    std::fs::write(&keyring, "backend = \"memory\"\nlocked = true\n").unwrap();

    // The daemon says once, at its start, that its keyring is its memory.
    let log = std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap();
    assert_eq!(
        log.matches("the vault's keyring is the daemon's memory")
            .count(),
        1,
        "{log}"
    );

    let (code, message) = user
        .call("vault.remove", json!({"name": "jira"}))
        .unwrap_err();
    assert_eq!(code, "unavailable");
    assert!(message.contains("locked"), "{message}");
    assert_eq!(
        listed_names(&user),
        ["jira"],
        "the entry stays, to be removed once the keyring is unlocked"
    );

    std::fs::write(&keyring, "backend = \"memory\"\n").unwrap();
    user.call("vault.remove", json!({"name": "jira"})).unwrap();
    assert!(listed_names(&user).is_empty());
}

fn listed_names(user: &User) -> Vec<String> {
    user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn ctrl_c_at_the_secret_s_prompt_gives_the_terminal_its_echo_back() {
    use std::io::Read;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let user = User::new();
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills the two descriptors it opens.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0);
    // SAFETY: both were just opened and are owned here alone.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let echoes = || {
        // SAFETY: tcgetattr only reads the terminal's flags.
        let mut flags: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(slave_raw(&slave), &mut flags) }, 0);
        flags.c_lflag & libc::ECHO != 0
    };
    fn slave_raw(fd: &OwnedFd) -> i32 {
        use std::os::fd::AsRawFd;
        fd.as_raw_fd()
    }
    assert!(echoes());

    let mut command = user.command(&["vault", "set", "jira"]);
    command
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: setsid and ioctl are async-signal-safe: the pty becomes the child's terminal, so
    // that ^C sends it SIGINT as a shell's terminal would.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut prompt = Vec::new();
    let mut byte = [0u8; 1];
    while !prompt.ends_with(b": ") {
        assert_eq!(stderr.read(&mut byte).unwrap(), 1, "the prompt");
        prompt.push(byte[0]);
    }
    eventually("the echo is off at the prompt", || !echoes());

    let mut master = std::fs::File::from(master);
    std::io::Write::write_all(&mut master, b"\x03").unwrap();
    let status = child.wait().unwrap();

    assert!(!status.success(), "{status:?}");
    assert!(
        echoes(),
        "the terminal echoes again once the command is interrupted"
    );
}

/// The project `app`'s tracker, a Jira site whose worklog names the vault entry `entry`.
fn worklog_names(user: &User, entry: &str) {
    let folder = user.path("work/app");
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    tracker_names(user, entry);
}

fn tracker_names(user: &User, entry: &str) {
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"],
                "worklog_token": entry}}),
    )
    .unwrap();
}

#[test]
fn removing_an_entry_in_use_names_its_feature_and_is_refused() {
    let user = User::with_fake_agent();
    project(&user, "app");
    worklog_names(&user, "jira-time");
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": "dev@example.com",
            "project": "app", "for": "worklog:app"}),
    )
    .unwrap();

    let (_, refused) = user
        .call("vault.remove", json!({"name": "jira-time"}))
        .unwrap_err();
    assert!(
        refused.contains("the worklog of the project app"),
        "{refused}"
    );
    assert!(!refused.contains(SECRET), "{refused}");
    assert_eq!(listed_names(&user), ["jira-time"], "kept whole");

    // With the confirmation that names what stops working, it goes.
    user.call(
        "vault.remove",
        json!({"name": "jira-time", "confirmed": true}),
    )
    .unwrap();
    assert!(listed_names(&user).is_empty());

    // Once its feature names another entry, a bound entry goes as any other.
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "for": "worklog:app"}),
    )
    .unwrap();
    tracker_names(&user, "another-entry");
    user.call("vault.remove", json!({"name": "jira-time"}))
        .unwrap();
    assert!(listed_names(&user).is_empty());
}

#[test]
fn an_unbound_entry_works_as_before() {
    let user = User::with_fake_agent();
    project(&user, "app");
    worklog_names(&user, "jira");
    set_jira(&user);

    let listed = user.call("vault.list", json!({})).unwrap();
    assert_eq!(listed["entries"][0]["for"], Value::Null, "{listed}");
    // Named by a feature, it is removed without a word, as before.
    user.call("vault.remove", json!({"name": "jira"})).unwrap();
    assert!(listed_names(&user).is_empty());

    // An unknown use is refused rather than kept.
    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "jira", "secret": SECRET, "for": "everything"}),
        )
        .unwrap_err();
    assert!(refused.contains("everything"), "{refused}");
    assert!(listed_names(&user).is_empty());
}

#[test]
fn an_entry_bound_to_a_worklog_is_never_shown_to_another_project_s_agents() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "start", "do": [{"say": "ready"}]}]}));
    project(&user, "alpha");
    project(&user, "beta");
    // Bound to alpha's worklog without a project: the entry is alpha's, not every project's.
    let entry = user
        .call(
            "vault.set",
            json!({"name": "jira-time", "secret": SECRET, "description": "Logs time",
                "for": "worklog:alpha"}),
        )
        .unwrap();
    assert_eq!(entry["project"], "alpha", "{entry}");
    user.call(
        "vault.set",
        json!({"name": "shared", "secret": "an0ther-s3cret", "description": "For all"}),
    )
    .unwrap();
    // An entry of another project cannot be bound to alpha's worklog.
    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "other-time", "secret": SECRET, "project": "beta",
                "for": "worklog:alpha"}),
        )
        .unwrap_err();
    assert!(refused.contains("beta"), "{refused}");

    // The agent of beta learns neither the entry nor the name of the project alpha.
    agent(&user, "beta", "start");
    let (url, authorization) = server_of(&user, 0);
    let answer = Mcp::initialize(&url, &authorization).call("vault", json!({"action": "list"}));
    assert!(answer.text.contains("shared"), "{}", answer.text);
    assert!(!answer.text.contains("jira-time"), "{}", answer.text);
    assert!(!answer.text.contains("alpha"), "{}", answer.text);

    // The agent of alpha sees its entry, as the docs say: name, description, URL, username.
    agent(&user, "alpha", "start");
    let (url, authorization) = server_of(&user, 1);
    let answer = Mcp::initialize(&url, &authorization).call("vault", json!({"action": "list"}));
    assert!(answer.text.contains("jira-time"), "{}", answer.text);
    assert!(!answer.text.contains("for:"), "{}", answer.text);
}

#[test]
fn setting_a_bound_entry_again_checks_its_binding() {
    let user = User::with_fake_agent();
    project(&user, "alpha");
    project(&user, "beta");
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "for": "worklog:alpha"}),
    )
    .unwrap();
    user.call(
        "vault.set",
        json!({"name": "almanac-token", "secret": SECRET, "for": "connection:almanac"}),
    )
    .unwrap();

    // Another project, without naming the use again: the binding is checked, not kept as is.
    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "jira-time", "secret": SECRET, "project": "beta"}),
        )
        .unwrap_err();
    assert!(refused.contains("beta"), "{refused}");
    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "almanac-token", "secret": SECRET, "project": "alpha"}),
        )
        .unwrap_err();
    assert!(refused.contains("the connection almanac"), "{refused}");

    let listed = user.call("vault.list", json!({})).unwrap();
    let project_of = |name: &str| {
        listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .map(|entry| entry["project"].clone())
            .unwrap()
    };
    assert_eq!(project_of("jira-time"), "alpha", "{listed}");
    assert_eq!(project_of("almanac-token"), Value::Null, "{listed}");
}

#[test]
fn a_binding_to_an_impossible_connection_name_is_refused() {
    let user = User::with_fake_agent();
    for bound in [
        "connection:a:b",
        "connection:Foo",
        "connection:with-dash",
        "connection:",
    ] {
        let (_, refused) = user
            .call(
                "vault.set",
                json!({"name": "token", "secret": SECRET, "for": bound}),
            )
            .unwrap_err();
        assert!(refused.contains(bound), "{refused}");
    }
    assert!(listed_names(&user).is_empty());
    // A name a connection can have is kept.
    user.call(
        "vault.set",
        json!({"name": "token", "secret": SECRET, "for": "connection:almanac_2"}),
    )
    .unwrap();
    assert_eq!(listed_names(&user), ["token"]);
}
