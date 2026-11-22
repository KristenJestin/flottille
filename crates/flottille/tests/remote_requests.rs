//! The user drives another machine (`docs/remote.md`, "Requests from another machine's
//! clients"): a hub served in the test on 127.0.0.1 and daemons, each with its own temporary HOME
//! and XDG folders and no keyring, the first machine set up with `flottille remote setup` and the
//! others joined as the user pairs them. A client of `pc` signs each request for `server` with
//! the device key, `pc`'s daemon carries it over its link, and `server` checks it before it serves
//! it as one of its own clients'.

use crate::common;
use crate::remote;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_link::{DeviceKey, Message, PublicKey, Request, Traffic};
use remote::{
    Machine, NoKeyring, TestHub, WAIT, error_of, eventually, json_of, pair, pc_and_server,
    pc_and_server_on, set_up, signed,
};
use serde_json::{Value, json};

/// What the server's log says of each request it served for another machine's client.
const SERVED: &str = "asked by";

/// A throwaway repository with one commit, pushed to a bare remote: its folder.
fn repository(user: &User) -> std::path::PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
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
    folder
}

fn fake_agent_user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "GREET", "do": [{"say": "hello from the server"}]},
    ]}));
    user
}

#[test]
fn the_cli_starts_and_reads_a_session_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(fake_agent_user());
    // The folder is the server's: a path typed on purpose is that machine's.
    let folder = repository(&server.user);
    let added = json_of(pc.on(
        "server",
        &[
            "project",
            "add",
            folder.to_str().unwrap(),
            "--name",
            "app",
            "--yes",
            "--json",
        ],
    ));
    assert_eq!(added["name"], "app", "{added}");
    assert!(
        server
            .user
            .call("project.show", json!({"name": "app"}))
            .is_ok()
    );
    assert!(
        pc.user
            .call("project.show", json!({"name": "app"}))
            .is_err()
    );

    let started = json_of(pc.on("server", &["agent", "start", "app", "GREET", "--json"]));
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    // It runs on the server, not here.
    assert!(server.user.call("agent.show", json!({"agent": id})).is_ok());
    assert!(pc.user.call("agent.show", json!({"agent": id})).is_err());

    let mut read = Value::Null;
    eventually("the server's reply read from pc", || {
        read = json_of(pc.on("server", &["agent", "read", &id, "--json"]));
        read["lines"].to_string().contains("hello from the server")
    });

    // FLOTTILLE_MACHINE aims every command the same way.
    let listed = pc
        .user
        .command(&["agent", "list", "--json"])
        .env("FLOTTILLE_MACHINE", "server")
        .output()
        .unwrap();
    let listed = json_of(listed);
    assert!(listed.to_string().contains(&id), "{listed}");
    let status = json_of(pc.on("server", &["status", "--json"]));
    assert_eq!(status["pid"], server.user.status()["pid"]);

    // The server names the device in its log, and in the events its serving emitted.
    let log = server.log();
    assert!(log.contains("agent.start asked by pc from pc"), "{log}");
    let events = server
        .user
        .watch(&["--after", "0"])
        .within(Duration::from_secs(2));
    assert!(
        events
            .iter()
            .any(|event| event["name"] == "project.added" && event["data"]["device"] == "pc"),
        "{events:#?}"
    );
}

#[test]
fn an_unsigned_or_badly_signed_request_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());

    let unsigned =
        pc.raw(&json!({"id": "1", "machine": "server", "method": "daemon.stop"}).to_string());
    assert_eq!(unsigned["id"], "1");
    assert!(
        error_of(&unsigned).starts_with("refused: this is not a signed request"),
        "{unsigned}"
    );

    // Signed by a key that is no member's.
    let stranger = DeviceKey::make().unwrap();
    let answer = pc.raw(&signed(&stranger, "2", "server", "daemon.stop", json!({})));
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );

    // Signed by pc's device, then changed on the way.
    let line = signed(&pc.device(), "3", "server", "project.list", json!({}));
    let changed = line.replace("project.list", "daemon.stop");
    let answer = pc.raw(&changed);
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );
    // Its answer cannot be taken under another id either.
    let moved = line.replace("\"id\":\"3\"", "\"id\":\"4\"");
    let answer = pc.raw(&moved);
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );

    assert!(!server.log().contains(SERVED), "{}", server.log());
    server.user.status();
}

#[test]
fn a_replayed_request_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let line = signed(&pc.device(), "1", "server", "daemon.status", json!({}));

    let first = pc.raw(&line);
    assert_eq!(first["id"], "1");
    assert_eq!(
        first["result"]["pid"],
        server.user.status()["pid"],
        "{first}"
    );
    let again = pc.raw(&line);
    assert_eq!(
        error_of(&again),
        "refused: this request was received already"
    );
    assert_eq!(server.log().matches(SERVED).count(), 1, "{}", server.log());
}

#[test]
fn a_request_of_a_removed_device_is_refused() {
    let (hub, pc, server) = pc_and_server(User::new());
    let laptop = Machine::new();
    pair(&hub, &laptop, "laptop", "controller");
    server.wait_for_member("laptop", true);
    let device = laptop.device();

    // Carried by pc's daemon, which holds no device key: the signature is laptop's.
    let answer = pc.raw(&signed(&device, "1", "server", "project.list", json!({})));
    assert!(answer["result"]["projects"].is_array(), "{answer}");

    // A client link laptop's machine key opened before its removal, kept open by a machine that
    // does not let it go.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server_key: PublicKey =
        serde_json::from_value(server.remote()["this"]["key"].clone()).unwrap();
    // Laptop's daemon stopped: the hub keeps one session a key, and the test's is the one.
    assert!(laptop.user.run(&["daemon", "stop"]).status.success());
    let endpoint = runtime.block_on(remote::endpoint_of(&laptop.folder()));
    let kept = runtime
        .block_on(remote::dial(
            &endpoint,
            server_key,
            Traffic::Client,
            "server",
        ))
        .unwrap();

    let removed = pc.user.run(&["remote", "remove", "laptop"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    server.wait_for_member("laptop", false);
    let answer = pc.raw(&signed(&device, "2", "server", "project.list", json!({})));
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );
    let line = signed(&pc.device(), "3", "server", "project.list", json!({}));
    let refused = runtime.block_on(async {
        let request = Message::Client(serde_json::from_str(&line).unwrap());
        match kept.ask(&request).await {
            Ok(mut answers) => answers
                .next()
                .await
                .map(|answer| answer.map_err(|e| e.to_string())),
            Err(error) => Some(Err(error.to_string())),
        }
    });
    // The server cuts the link at once, and the hub carries nothing of a device removed: the
    // request is answered by no one, and never served.
    assert!(matches!(&refused, Some(Err(_)) | None), "{refused:?}");
    assert_eq!(server.log().matches(SERVED).count(), 1, "{}", server.log());
    runtime.block_on(endpoint.close());
}

#[test]
fn a_request_to_an_offline_machine_is_refused_saying_since_when() {
    let (_hub, pc, server) = pc_and_server(User::new());
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");

    let refused = pc.on("server", &["project", "list"]);
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    let (_, since) = said
        .split_once("server is offline since ")
        .unwrap_or_else(|| panic!("{said}"));
    let since = since.trim();
    assert!(
        since.len() == 5 && since.as_bytes()[2] == b':',
        "not a time of day: {said}"
    );

    // Nothing was kept for it: the server, back, serves nothing it did not take.
    server.user.status();
    pc.wait_for("server", "online");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!server.log().contains(SERVED), "{}", server.log());
}

#[test]
fn events_of_another_machine_resume_after_the_last_seen_when_the_link_drops() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let first_pid = server.user.status()["pid"].clone();

    let watch = pc.user.watch(&["--machine", "server", "--after", "0"]);
    let seen =
        watch.until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == first_pid);
    let last = seen.last().unwrap()["seq"].as_u64().unwrap();

    // The server stops and starts again: the link drops, and comes back.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    let second_pid = server.user.status()["pid"].clone();
    assert_ne!(first_pid, second_pid);
    let after = watch
        .until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == second_pid);
    // Every event after the last seen, none twice, none missing.
    let seqs: Vec<u64> = after
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    let stored: Vec<u64> = server
        .user
        .watch(&["--after", &last.to_string()])
        .until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == second_pid)
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, stored);
    assert!(seqs[0] > last, "{seqs:?} after {last}");
    // The server's events, not pc's: pc's own daemon never stopped.
    assert!(
        after.iter().any(|event| event["name"] == "daemon.stopping"),
        "{after:#?}"
    );
}

#[test]
fn a_machine_message_cannot_carry_a_user_method() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let server_key: PublicKey =
        serde_json::from_value(server.remote()["this"]["key"].clone()).unwrap();
    // pc's daemon is stopped, so that no link of its own takes the one below (two links of one
    // machine are not kept: the second closes the first, whoever dials again): the key is only
    // what the test holds.
    assert!(pc.user.run(&["daemon", "stop"]).status.success());
    server.wait_for("pc", "offline");
    // pc's machine key, as an agent that read it would hold it: it signs what a machine may
    // send, never what the user asks.
    let folder = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let user_request = signed(&pc.device(), "1", "server", "daemon.stop", json!({}));
    let refusals = runtime.block_on(async {
        let endpoint = remote::endpoint_of(&pc.folder()).await;
        let link = tokio::time::timeout(
            WAIT,
            remote::dial(&endpoint, server_key, Traffic::Machine, "server"),
        )
        .await
        .unwrap()
        .unwrap();
        let mut refusals = Vec::new();
        for frame in [
            // A user's request, even one a member's device signed, on the machine side.
            json!({"client": serde_json::from_str::<Value>(&user_request).unwrap()}),
            // A user method dressed as a machine's message.
            json!({"machine": {"call": {"method": "daemon.stop", "params": {}}}}),
            json!({"machine": {"daemon.stop": {}}}),
            json!({"machine": {"method": "project.add", "params": {"folder": "/"}}}),
        ] {
            let answer = link.send_raw(frame.to_string().as_bytes()).await;
            refusals.push(answer.unwrap_err().to_string());
        }
        // And a client link takes no request the machine key signed.
        let client = remote::dial(&endpoint, server_key, Traffic::Client, "server")
            .await
            .unwrap();
        let forged = Request::sign(
            &machine_as_device(&pc.folder(), folder.path()),
            "2",
            "server",
            "daemon.stop",
            &json!({}),
            SystemTime::now(),
        )
        .unwrap();
        let mut answers = client
            .ask(&Message::Client(serde_json::to_value(&forged).unwrap()))
            .await
            .unwrap();
        refusals.push(answers.next().await.unwrap().unwrap().to_string());
        endpoint.close().await;
        refusals
    });
    assert_eq!(
        refusals[0],
        "server refused: a client's request is not taken on a machine link"
    );
    for refusal in &refusals[1..4] {
        assert_eq!(
            refusal, "server refused: not a message a machine may send",
            "{refusals:?}"
        );
    }
    assert!(
        refusals[4].contains("refused: this request is not signed by a member"),
        "{refusals:?}"
    );
    // The server still runs, and served nothing.
    server.user.status();
    assert!(!server.log().contains(SERVED), "{}", server.log());
}

/// The machine key kept in `folder` taken for a device key, as code that read it could: its 32
/// bytes written where a client keeps a device key, in `apart`.
fn machine_as_device(folder: &Path, apart: &Path) -> DeviceKey {
    use std::os::unix::fs::PermissionsExt;
    let apart = apart.join("remote");
    std::fs::create_dir(&apart).unwrap();
    std::fs::set_permissions(&apart, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::copy(folder.join("machine.key"), apart.join("device.key")).unwrap();
    DeviceKey::load(&NoKeyring, &apart).unwrap().unwrap()
}

#[test]
fn a_client_started_by_an_agent_is_still_refused_with_machine() {
    // The daemon's refusal by the client's ancestry is what is tested: the run's private mount
    // namespace, which hides the daemon's socket before that, is left out.
    let pc_user = User::with_fake_agent()
        .with_allowing_judge()
        .with_env("FLOTTILLE_TEST_NO_USER_NAMESPACES", "1");
    let (_hub, pc, server) = pc_and_server_on(Machine::of(pc_user), User::new());
    let repository = repository(&pc.user);
    pc.user
        .call("project.add", json!({"folder": repository, "name": "app"}))
        .unwrap();
    let workspace = pc
        .user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let folder = std::path::PathBuf::from(workspace["folder"].as_str().unwrap());
    let opened = pc.user.received_requests("session/new").len();
    pc.user
        .call(
            "agent.run",
            json!({"project": "app", "message": "hello", "workspace": "w"}),
        )
        .unwrap();
    let (url, authorization) = server_of(&pc.user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    // The user's own command line, aimed at the server, run by the agent: the device key is
    // there to sign with, and still nothing is carried.
    std::fs::write(
        folder.join("client.sh"),
        format!(
            "XDG_RUNTIME_DIR='{}' XDG_STATE_HOME='{}' XDG_CONFIG_HOME='{}' exec '{}' \
             --machine server status > client.out 2>&1\n",
            pc.user.path("runtime").display(),
            pc.user.path("state").display(),
            pc.user.path("config").display(),
            env!("CARGO_BIN_EXE_flottille")
        ),
    )
    .unwrap();
    let answer = mcp.call("run", json!({"line": "sh client.sh"}));
    assert!(
        answer.text.contains("exited with code 1"),
        "{}",
        answer.text
    );
    let said = std::fs::read_to_string(folder.join("client.out")).unwrap();
    // Refused by its ancestry; where the kernel's Landlock reaches sockets by their path, the
    // run cannot even open the socket.
    assert!(
        said.contains("refused: this client was started by an agent")
            || said.contains("daemon.sock: Permission denied"),
        "{said}"
    );
    server.user.status();
    assert!(!server.log().contains(SERVED), "{}", server.log());
}

/// What a machine's log says when a watch it served for a device ends.
const WATCH_ENDED: &str = "the watch asked by pc ended";

#[test]
fn a_watch_of_another_machine_ends_with_its_client_on_a_quiet_machine() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let pid = server.user.status()["pid"].clone();
    let watch = pc.user.watch(&["--machine", "server", "--after", "0"]);
    watch.until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == pid);
    assert!(!server.log().contains(WATCH_ENDED), "{}", server.log());

    // The client leaves; nothing happens on the server meanwhile, so no event is sent that
    // could tell the carrying daemon its client is gone.
    drop(watch);
    eventually("the server's subscription to end", || {
        server.log().contains(WATCH_ENDED)
    });
}

#[test]
fn a_watch_of_this_machine_by_its_name_ends_with_its_client_too() {
    let (_hub, pc, _server) = pc_and_server(User::new());
    let watch = pc.user.watch(&["--machine", "pc", "--after", "0"]);
    watch.until(|event| event["name"] == "daemon.started");
    assert!(!pc.log().contains(WATCH_ENDED), "{}", pc.log());

    drop(watch);
    eventually("the subscription to end", || pc.log().contains(WATCH_ENDED));
}

#[test]
fn an_exported_machine_does_not_stop_the_local_daemon_from_starting() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let pid = server.user.status()["pid"].clone();
    // No daemon runs here: the command starts one, which must not read the variable the command
    // was given.
    assert!(pc.user.run(&["daemon", "stop"]).status.success());
    eventually(
        "the server's status read through a daemon the command started",
        || {
            let output = pc
                .user
                .command(&["status", "--json"])
                .env("FLOTTILLE_MACHINE", "server")
                .output()
                .unwrap();
            serde_json::from_slice::<Value>(&output.stdout).is_ok_and(|status| status["pid"] == pid)
        },
    );
    // `daemon` itself ignores the variable and refuses the flag.
    let refused = pc.user.run(&["--machine", "server", "daemon"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("this machine's own"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn the_commands_that_run_here_refuse_the_flag() {
    let pc = Machine::new();
    for line in [
        &["doctor"][..],
        &["vault", "provide", "/nonexistent/login.sock"],
        &["read-page", "web", "http://app.example.com/"],
        &["mcp"],
        &["update"],
        &["remote", "members"],
    ] {
        let flagged = pc.user.run(&[&["--machine", "server"], line].concat());
        let said = String::from_utf8_lossy(&flagged.stderr);
        assert!(
            !flagged.status.success() && said.contains("this machine's own"),
            "{line:?}: {said}"
        );
    }
}

#[test]
fn the_variable_alone_does_not_refuse_the_commands_that_run_here() {
    let pc = Machine::new();
    // Only the ones that touch nothing outside the test's folders.
    for line in [
        &["vault", "provide", "/nonexistent/login.sock"][..],
        &["read-page", "web", "http://app.example.com/"],
    ] {
        let exported = pc
            .user
            .command(line)
            .env("FLOTTILLE_MACHINE", "server")
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&exported.stderr);
        assert!(!said.contains("this machine's own"), "{line:?}: {said}");
    }
}

/// The pull request of `app/login` in `acme/api` as the fake `gh` answers it.
fn pull(checks: &str) -> Value {
    json!({
        "number": 12,
        "title": "Add the login page",
        "state": "OPEN",
        "isDraft": false,
        "baseRefName": "main",
        "url": "https://github.com/acme/api/pull/12",
        "reviewDecision": "",
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": checks}
        ],
        "comments": [],
        "reviews": []
    })
}

#[test]
fn a_forge_watched_on_another_machine_stays_read_while_the_watch_runs() {
    let server_user = User::new()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (_hub, pc, server) = pc_and_server(server_user);
    let folder = repository(&server.user);
    server
        .user
        .call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    server
        .user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    server.user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    let settings = server
        .user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    let declared: Vec<Value> = settings["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({"path": r["path"], "forge": r["proposal"]}))
        .collect();
    server
        .user
        .call(
            "project.set_settings",
            json!({"name": "app", "repositories": declared}),
        )
        .unwrap();
    let pulls = |checks: &str| {
        server
            .user
            .gh_script(json!({"pulls": {"acme/api": {"app/login": pull(checks)}}}));
    };
    pulls("SUCCESS");

    let watch = pc
        .user
        .watch(&["--machine", "server", "--forge", "app", "login"]);
    let first = watch
        .until(|event| event["name"] == "workspace.forge")
        .pop()
        .unwrap();
    assert_eq!(first["data"]["project"], "app", "{first}");

    // The checks fail; a minute later the server reads the forge again, for the watch is still
    // there, and says so.
    pulls("FAILURE");
    server
        .user
        .call("test.advance", json!({"ms": 61_000}))
        .unwrap();
    let changed = watch
        .until(|event| event["name"] == "workspace.forge")
        .pop()
        .unwrap();
    assert!(changed["data"].to_string().contains("failing"), "{changed}");
}

/// A hub, `pc`, `server` (a runner on `server_user`) and a third machine on `third_user`, paired
/// as `name` with `role`, the three linked.
fn pc_server_and(
    server_user: User,
    third_user: User,
    name: &str,
    role: &str,
) -> (TestHub, Machine, Machine, Machine) {
    let (hub, pc, server) = pc_and_server(server_user);
    let third = Machine::of(third_user);
    pair(&hub, &third, name, role);
    pc.wait_for(name, "online");
    server.wait_for(name, "online");
    third.wait_for("server", "online");
    (hub, pc, server, third)
}

/// A user whose agents are the fake agent, Claude enabled, its login where its own home says and
/// no sign-in variable: Claude is signed in once its login file is there, and seen so within a
/// moment.
fn runner_user() -> User {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_OFFER_MS", "300")
        .with_env("ANTHROPIC_API_KEY", "")
        .with_env("CLAUDE_CODE_OAUTH_TOKEN", "")
        .with_env("ANTHROPIC_AUTH_TOKEN", "");
    let claude = user.path("home/.claude");
    let user = user.with_env("CLAUDE_CONFIG_DIR", claude.to_str().unwrap());
    user.write_providers("[claude]\nenabled = true\n");
    user
}

/// Claude's login file on `user`'s machine, holding `text`.
fn sign_in(user: &User, text: &str) -> std::path::PathBuf {
    let folder = user.path("home/.claude");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join(".credentials.json");
    std::fs::write(&file, text).unwrap();
    file
}

/// What `machine` offers as `seen` (`remote.machines`) says it.
fn offers_of<'a>(seen: &'a Value, machine: &str) -> &'a Value {
    seen["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|seen| seen["name"] == machine)
        .map_or(&Value::Null, |seen| &seen["offers"])
}

fn slots_of(offers: &Value, project: &str) -> Option<u64> {
    offers["projects"]
        .as_array()?
        .iter()
        .find(|offered| offered["name"] == project)
        .and_then(|offered| offered["slots"].as_u64())
}

fn agents_of(offers: &Value) -> Vec<String> {
    offers["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| agent.as_str().map(str::to_owned))
        .collect()
}

/// A project `app` on `user`'s machine over a throwaway repository.
fn app_on(user: &User) -> std::path::PathBuf {
    let folder = repository(user);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

#[test]
fn a_viewer_reads_but_cannot_send_answer_or_start() {
    let (_hub, _pc, server, laptop) = pc_server_and(User::new(), User::new(), "laptop", "viewer");
    let device = laptop.device();

    let read = laptop.raw(&signed(&device, "1", "server", "agent.list", json!({})));
    assert!(read["error"].is_null(), "{read}");
    assert!(read["result"]["agents"].is_array(), "{read}");

    for (n, (method, deed)) in [
        ("agent.send", "send a message"),
        ("agent.answer", "answer a question"),
        ("approval.approve", "answer a request"),
        // What the Android session page writes (#826): a viewer's page draws none of it, and the
        // machine refuses each, so a device that bypassed the page changes nothing either.
        ("approval.deny", "answer a request"),
        ("agent.mode", "switch the mode"),
        ("agent.cancel", "stop a turn"),
        ("agent.send_now", "send a message"),
        ("agent.start", "start a session"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("{}", n + 2);
        let answer = laptop.raw(&signed(&device, &id, "server", method, json!({})));
        assert_eq!(answer["id"], id.as_str());
        assert_eq!(
            error_of(&answer),
            format!("refused: laptop may only view: a controller or a runner may {deed}"),
            "{method}"
        );
    }
    // The viewer's own command line is told the same.
    let sent = laptop.on("server", &["agent", "send", "a1", "go on"]);
    assert!(!sent.status.success());
    let said = String::from_utf8_lossy(&sent.stderr);
    assert!(
        said.contains("laptop may only view: a controller or a runner may send a message"),
        "{said}"
    );
    // Only the read was served.
    assert_eq!(server.log().matches(SERVED).count(), 1, "{}", server.log());
}

#[test]
fn a_controller_answers_a_request_and_starts_a_session_on_a_runner() {
    let (_hub, _pc, server, tablet) =
        pc_server_and(User::with_fake_agent(), User::new(), "tablet", "controller");
    let folder = app_on(&server.user);
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "DB_PASSWORD=invented-for-a-test\n").unwrap();
    server
        .user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        server
            .user
            .call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let device = tablet.device();
    let opened = server.user.received_requests("session/new").len();
    let started = tablet.raw(&signed(
        &device,
        "1",
        "server",
        "agent.run",
        json!({"project": "app", "workspace": "w", "message": "hello"}),
    ));
    let id = started["result"]["agent"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not started: {started}"))
        .to_owned();
    assert!(server.user.call("agent.show", json!({"agent": id})).is_ok());

    // The session's agent asks for what the gate holds for the user.
    let (url, authorization) = server_of(&server.user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = mcp.call("fs_read", json!({"path": ".env"}));
    let number: u64 = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text));

    let approved = tablet.raw(&signed(
        &device,
        "2",
        "server",
        "approval.approve",
        json!({"number": number}),
    ));
    assert!(approved["error"].is_null(), "{approved}");
    let listed = server.user.call("approval.list", json!({})).unwrap();
    assert!(
        listed["requests"].as_array().unwrap().is_empty(),
        "{listed}"
    );
    let log = server.log();
    assert!(
        log.contains("agent.run asked by tablet from tablet"),
        "{log}"
    );
    assert!(
        log.contains("approval.approve asked by tablet from tablet"),
        "{log}"
    );
}

#[test]
fn a_viewer_cannot_remove_a_device() {
    let (_hub, pc, server, laptop) = pc_server_and(User::new(), User::new(), "laptop", "viewer");

    // On its own machine, the list refuses it.
    let removed = laptop.user.run(&["remote", "remove", "server"]);
    assert!(!removed.status.success());
    let said = String::from_utf8_lossy(&removed.stderr);
    assert!(
        said.contains("a controller or a runner may remove"),
        "{said}"
    );

    // Aimed at the runner, its signed requests are refused before they are served.
    let device = laptop.device();
    let answer = laptop.raw(&signed(&device, "1", "server", "remote.change", json!({})));
    assert_eq!(
        error_of(&answer),
        "refused: laptop may only view: a controller or a runner may remove, rename or restyle a \
         device"
    );
    assert!(!server.log().contains(SERVED), "{}", server.log());
    pc.wait_for_member("server", true);
    server.wait_for_member("laptop", true);
}

#[test]
fn a_runner_s_capabilities_reach_its_peers_and_follow_a_change() {
    let (_hub, pc, server) = pc_and_server(runner_user());
    pc.wait_until("server offers no project, and no agent", |seen| {
        let offers = offers_of(seen, "server");
        offers["projects"] == json!([]) && !agents_of(offers).contains(&"claude".to_owned())
    });
    assert_eq!(
        pc.remote()["machines"][0]["role"],
        "runner",
        "{}",
        pc.remote()
    );

    // A project registered: its slots are its cap.
    app_on(&server.user);
    pc.wait_until("server offers app with 12 slots", |seen| {
        slots_of(offers_of(seen, "server"), "app") == Some(12)
    });
    // Its cap changed.
    let file = server.user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("max_children = 12"), "{text}");
    std::fs::write(&file, text.replace("max_children = 12", "max_children = 3")).unwrap();
    // The daemon sees a file edited by hand at the next request about projects.
    server.user.call("project.list", json!({})).unwrap();
    pc.wait_until("server offers app with 3 slots", |seen| {
        slots_of(offers_of(seen, "server"), "app") == Some(3)
    });
    // Claude signed in, then out.
    let login = sign_in(&server.user, "{}");
    pc.wait_until("server offers claude", |seen| {
        agents_of(offers_of(seen, "server")).contains(&"claude".to_owned())
    });
    std::fs::remove_file(login).unwrap();
    pc.wait_until("server no longer offers claude", |seen| {
        !agents_of(offers_of(seen, "server")).contains(&"claude".to_owned())
    });
    // The other way too: pc is a runner, and the server sees what it offers.
    server.wait_until("pc offers no project", |seen| {
        offers_of(seen, "pc")["projects"] == json!([])
    });
}

#[test]
fn a_login_file_is_checked_for_presence_and_never_read() {
    use std::os::unix::fs::PermissionsExt;
    let (_hub, pc, server) = pc_and_server(runner_user());
    // A login no one may read, holding what a token would look like: its presence is enough.
    let marker = "sk-invented-login-0123456789";
    let login = sign_in(&server.user, &format!("{{\"token\": \"{marker}\"}}"));
    std::fs::set_permissions(&login, std::fs::Permissions::from_mode(0o000)).unwrap();
    pc.wait_until("server offers claude", |seen| {
        agents_of(offers_of(seen, "server")).contains(&"claude".to_owned())
    });

    let seen = pc.remote().to_string();
    assert!(!seen.contains(marker), "{seen}");
    for log in [server.log(), pc.log()] {
        assert!(!log.contains(marker), "{log}");
    }
    // Left as it was.
    let metadata = std::fs::metadata(&login).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0);
    std::fs::set_permissions(&login, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        std::fs::read_to_string(&login).unwrap().contains(marker),
        "the login file was changed"
    );
}

#[test]
fn only_runners_with_the_agent_signed_in_and_a_free_slot_are_offered() {
    let (_hub, pc, server, laptop) =
        pc_server_and(runner_user(), runner_user(), "laptop", "runner");
    // Both have the project; only the server's Claude is signed in.
    app_on(&server.user);
    app_on(&laptop.user);
    sign_in(&server.user, "{}");
    pc.wait_until("both offer app, the server Claude", |seen| {
        slots_of(offers_of(seen, "server"), "app").is_some()
            && slots_of(offers_of(seen, "laptop"), "app").is_some()
            && agents_of(offers_of(seen, "server")).contains(&"claude".to_owned())
    });

    let offered = |params: Value| -> Value {
        pc.user.call("remote.machines", params).unwrap()["offered"].clone()
    };
    assert_eq!(
        offered(json!({"project": "app", "agent": "claude"})),
        json!(["server"])
    );
    // No agent named: any signed in, and the laptop has none.
    assert_eq!(offered(json!({"project": "app"})), json!(["server"]));
    assert_eq!(
        offered(json!({"project": "shop", "agent": "claude"})),
        json!([])
    );

    // The command line lists the runners with what they offer, and only those that take it.
    let listed = pc.user.run(&[
        "remote",
        "members",
        "--runners",
        "--project",
        "app",
        "--agent",
        "claude",
    ]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.contains("server"), "{text}");
    assert!(!text.contains("laptop"), "{text}");
    let all = pc.user.run(&["remote", "members", "--runners"]);
    let text = String::from_utf8_lossy(&all.stdout);
    assert!(
        text.contains("server") && text.contains("laptop") && text.contains("app"),
        "{text}"
    );
}

#[test]
fn a_launch_aimed_at_a_controller_is_refused() {
    let (_hub, pc, _server, tablet) =
        pc_server_and(User::new(), User::new(), "tablet", "controller");
    const NO_AGENT: &str = "refused: tablet is a controller: it runs no agent, a runner does";

    // Refused by the machine that would carry it.
    let answer = pc.raw(&signed(
        &pc.device(),
        "1",
        "tablet",
        "agent.start",
        json!({"project": "app", "message": "go"}),
    ));
    assert_eq!(error_of(&answer), NO_AGENT);
    let started = pc.on("tablet", &["agent", "start", "app", "go"]);
    assert!(!started.status.success());
    let said = String::from_utf8_lossy(&started.stderr);
    assert!(said.contains(NO_AGENT), "{said}");

    // And by the controller's own daemon, asked by its name.
    let answer = tablet.raw(&signed(
        &tablet.device(),
        "2",
        "tablet",
        "agent.run",
        json!({"project": "app", "message": "go"}),
    ));
    assert_eq!(error_of(&answer), NO_AGENT);
    assert!(!tablet.log().contains(SERVED), "{}", tablet.log());
}

#[test]
fn an_offer_of_a_machine_that_is_no_runner_is_refused() {
    let (_hub, pc, server, tablet) =
        pc_server_and(User::new(), User::new(), "tablet", "controller");
    // A controller's daemon offers nothing, and is seen as what it is.
    let seen = server.remote();
    let entry = seen["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|machine| machine["name"] == "tablet")
        .unwrap()
        .clone();
    assert_eq!(entry["role"], "controller", "{seen}");
    assert!(entry["offers"].is_null(), "{seen}");
    assert_eq!(pc.remote()["this"]["role"], "runner");

    // Its machine key, as code that read it would hold it, says it offers a project.
    let server_key: PublicKey = serde_json::from_value(seen["this"]["key"].clone()).unwrap();
    assert!(tablet.user.run(&["daemon", "stop"]).status.success());
    server.wait_for("tablet", "offline");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let refusal = runtime.block_on(async {
        let endpoint = remote::endpoint_of(&tablet.folder()).await;
        let link = tokio::time::timeout(
            WAIT,
            remote::dial(&endpoint, server_key, Traffic::Machine, "server"),
        )
        .await
        .unwrap()
        .unwrap();
        let offer = json!({"machine": {"capabilities": {
            "projects": [{"name": "app", "slots": 12}],
            "agents": ["claude"],
        }}});
        let answer = link.send_raw(offer.to_string().as_bytes()).await;
        endpoint.close().await;
        answer.unwrap_err().to_string()
    });
    assert_eq!(
        refusal,
        "server refused: tablet is a controller: only a runner offers"
    );
    let seen = server
        .user
        .call("remote.machines", json!({"project": "app"}))
        .unwrap();
    assert_eq!(seen["offered"], json!([]), "{seen}");
}

/// A hub, `pc` its first member, and another machine paired as `name` with `role`, the two
/// linked.
fn pc_and(name: &str, role: &str) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let (pc, other) = (Machine::new(), Machine::new());
    set_up(&hub, &pc);
    pair(&hub, &other, name, role);
    pc.wait_for(name, "online");
    other.wait_for("pc", "online");
    (hub, pc, other)
}

/// `method`, which starts an agent, aimed at the controller `tablet`: refused by `pc`, which
/// would carry it, and by the tablet's own daemon, which serves nothing of it.
fn refused_on_a_controller(method: &str, params: Value) {
    let (_hub, pc, tablet) = pc_and("tablet", "controller");
    const NO_AGENT: &str = "refused: tablet is a controller: it runs no agent, a runner does";
    let carried = pc.raw(&signed(&pc.device(), "1", "tablet", method, params.clone()));
    assert_eq!(error_of(&carried), NO_AGENT, "{method}");
    let asked = tablet.raw(&signed(&tablet.device(), "2", "tablet", method, params));
    assert_eq!(error_of(&asked), NO_AGENT, "{method}");
    assert!(!tablet.log().contains(SERVED), "{}", tablet.log());
}

#[test]
fn a_resume_aimed_at_a_controller_is_refused() {
    refused_on_a_controller("agent.resume", json!({"agent": "a1"}));
}

#[test]
fn a_resume_after_the_quota_aimed_at_a_controller_is_refused() {
    refused_on_a_controller("agent.quota_resume", json!({"agent": "a1", "resume": true}));
}

#[test]
fn a_spec_written_by_a_session_aimed_at_a_controller_is_refused() {
    refused_on_a_controller("spec.new", json!({"project": "app", "idea": "a page"}));
}

#[test]
fn a_spec_of_a_ticket_aimed_at_a_controller_is_refused() {
    refused_on_a_controller(
        "spec.from_ticket",
        json!({"project": "app", "ticket": "APP-1"}),
    );
}

#[test]
fn a_spec_developed_aimed_at_a_controller_is_refused() {
    refused_on_a_controller("spec.develop", json!({"spec": 1}));
}

#[test]
fn a_review_aimed_at_a_controller_is_refused() {
    refused_on_a_controller("review.start", json!({"agent": "a1"}));
}

#[test]
fn a_watch_carried_for_a_removed_device_ends() {
    let (hub, pc, server) = pc_and_server(User::new());
    let laptop = Machine::new();
    pair(&hub, &laptop, "laptop", "controller");
    server.wait_for_member("laptop", true);
    let device = laptop.device();

    // The laptop's watch of the server, carried by pc, a machine that stays trusted.
    pc.user.status();
    let mut stream = UnixStream::connect(pc.user.path("runtime/flottille/daemon.sock")).unwrap();
    let line = signed(&device, "1", "server", "events.watch", json!({"after": 0}));
    writeln!(stream, "{line}").unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let mut reading = BufReader::new(stream);
    let mut first = String::new();
    reading.read_line(&mut first).unwrap();
    let answer: Value = serde_json::from_str(&first).unwrap();
    assert!(answer["error"].is_null(), "{answer}");
    assert!(
        server.log().contains("events.watch asked by laptop"),
        "{}",
        server.log()
    );

    let removed = pc.user.run(&["remote", "remove", "laptop"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    server.wait_for_member("laptop", false);
    eventually("the removed device's watch to end", || {
        server.log().contains("the watch asked by laptop ended")
    });
}

/// What `pid` read so far: its read calls, and their bytes.
fn reads(pid: u64) -> (u64, u64) {
    let io = std::fs::read_to_string(format!("/proc/{pid}/io")).unwrap();
    let field = |name: &str| {
        io.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap()
    };
    (field("syscr:"), field("rchar:"))
}

/// How many times the threads of `pid` gave the processor back or had it taken.
fn switches(pid: u64) -> std::collections::HashMap<String, u64> {
    let mut threads = std::collections::HashMap::new();
    for task in std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .flatten()
    {
        let Ok(status) = std::fs::read_to_string(task.path().join("status")) else {
            continue;
        };
        let count = status
            .lines()
            .filter(|line| line.contains("ctxt_switches:"))
            .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
            .sum::<u64>();
        threads.insert(task.file_name().to_string_lossy().into_owned(), count);
    }
    threads
}

/// The wake-ups between two counts, thread by thread: a thread that ended in between takes its
/// count with it, so a sum over the whole process could go down.
fn woken(
    before: &std::collections::HashMap<String, u64>,
    after: &std::collections::HashMap<String, u64>,
) -> u64 {
    after
        .iter()
        .map(|(thread, count)| count.saturating_sub(before.get(thread).copied().unwrap_or(0)))
        .sum()
}

/// The read calls, the bytes read and the wake-ups of `machine`'s daemon in ten quiet seconds,
/// each per second.
fn at_rest(machine: &Machine) -> (f64, f64, f64) {
    let pid = machine.user.status()["pid"].as_u64().unwrap();
    // What the last request left to finish ends first.
    std::thread::sleep(Duration::from_secs(3));
    let span = Duration::from_secs(10);
    let ((calls, bytes), woken) = (reads(pid), switches(pid));
    std::thread::sleep(span);
    let ((calls_after, bytes_after), woken_after) = (reads(pid), switches(pid));
    let per_second = |before: u64, after: u64| (after - before) as f64 / span.as_secs_f64();
    (
        per_second(calls, calls_after),
        per_second(bytes, bytes_after),
        self::woken(&woken, &woken_after) as f64 / span.as_secs_f64(),
    )
}

/// A runner at rest with remote on reads next to nothing, its offer looked at only when something
/// changed. Measured over ten seconds, a second: 1.4 read calls and no byte with no link, 3.8 and
/// 250 bytes with one; when the offer was looked at every second, 19.5 calls and 3.3 kB with no
/// link, 21.5 and 4.8 kB with one.
const READ_CALLS_PER_SECOND: f64 = 8.0;
const BYTES_PER_SECOND: f64 = 1_500.0;
/// The daemon's own timers and the link's round (measured: 2.4 to 4.3; 8.7 to 11.7 when the offer
/// was looked at every second), with room for a loaded machine.
const WAKE_UPS_PER_SECOND: f64 = 8.0;

#[test]
fn a_runner_looks_at_what_it_offers_only_when_it_changed() {
    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    // Projects whose files, sessions and slots an offer looked at every second.
    for n in 0..8 {
        let folder = pc.user.path(&format!("work/p{n}"));
        pc.user.git_init(&folder);
        std::fs::write(folder.join("README.md"), "p\n").unwrap();
        pc.user.git(&folder, &["add", "README.md"]);
        pc.user.git(&folder, &["commit", "--quiet", "-m", "start"]);
        pc.user
            .call(
                "project.add",
                json!({"folder": folder, "name": format!("p{n}")}),
            )
            .unwrap();
    }

    let check = |when: &str, (calls, bytes, woken): (f64, f64, f64)| {
        assert!(
            calls <= READ_CALLS_PER_SECOND && bytes <= BYTES_PER_SECOND,
            "{when}: {calls:.1} read calls and {bytes:.0} bytes a second (at most \
             {READ_CALLS_PER_SECOND} and {BYTES_PER_SECOND})"
        );
        assert!(
            woken <= WAKE_UPS_PER_SECOND,
            "{when}: {woken:.1} wake-ups a second (at most {WAKE_UPS_PER_SECOND})"
        );
    };
    let no_link = at_rest(&pc);

    let server = Machine::new();
    pair(&hub, &server, "server", "runner");
    pc.wait_for("server", "online");
    // The offer was sent when the link was made.
    server.wait_until("pc offers its projects", |seen| {
        slots_of(offers_of(seen, "pc"), "p0").is_some()
    });
    let one_link = at_rest(&pc);
    check("with no link", no_link);
    check("with one link", one_link);
}
