//! `agent_launch` with `machine` starts a child or a led session on another of the user's
//! machines (`docs/remote.md`, "Starting a session on another machine"): the receiving machine's
//! gate judges it, the caller's place travels by itself, and the caller is answered at once. A hub
//! served in the test on 127.0.0.1 and daemons, each with its own temporary HOME and XDG folders,
//! in-memory keyring and fake agents; throwaway repositories. No build here travels code by the
//! test gate (`FLOTTILLE_TEST_TRAVEL`): code travels only with a launch.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Endpoint, Git, GitOffer, GitPlace, Hello, LaunchAsk, Link, MachineKey, MachineMessage, Message,
    PublicKey, Traffic,
};
use serde_json::{Value, json};

/// How long a link, an offer or the hub's news may take: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(40);

/// The hub, served from a temporary state folder in a runtime of the test's own.
struct TestHub {
    _runtime: tokio::runtime::Runtime,
    _folder: tempfile::TempDir,
    served: Served<SystemClock>,
    address: SocketAddr,
}

impl TestHub {
    fn new() -> TestHub {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let served = runtime.block_on(async {
            let hub = Hub::open(&folder.path().join("hub"), SystemClock).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            flottille_hub::serve(hub, listener, None, Hello::ours("0.4.0")).unwrap()
        });
        let address = served.address();
        TestHub {
            _runtime: runtime,
            _folder: folder,
            served,
            address,
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn code(&self) -> String {
        self.served
            .with_hub(|hub| hub.setup_code().unwrap().to_owned())
    }
}

struct Machine {
    user: User,
}

/// A user whose agents are the fake agent, Claude enabled and signed in (its login file only
/// there), offers looked at often, the fake escalation programs first in its `PATH`.
fn runner_user() -> User {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_OFFER_MS", "300")
        .with_env("ANTHROPIC_API_KEY", "")
        .with_env("CLAUDE_CODE_OAUTH_TOKEN", "")
        .with_env("ANTHROPIC_AUTH_TOKEN", "");
    let claude = user.path("home/.claude");
    let user = user
        .with_env("CLAUDE_CONFIG_DIR", claude.to_str().unwrap())
        .with_fake_escalation();
    user.write_providers("[claude]\nenabled = true\n");
    user
}

/// Claude's login file on `user`'s machine: its presence is all that is looked at.
fn sign_in(user: &User) {
    let folder = user.path("home/.claude");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(".credentials.json"), "{}").unwrap();
}

impl Machine {
    /// `user`'s machine, with remote on, its link on loopback.
    fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\nlisten = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        user.status();
        Machine { user }
    }

    fn this(&self) -> Value {
        self.user.call("remote.machines", json!({})).unwrap()["this"].clone()
    }

    fn key(&self) -> PublicKey {
        serde_json::from_value(self.this()["key"].clone()).unwrap()
    }

    fn addresses(&self) -> Vec<SocketAddr> {
        self.this()["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|address| address["address"].as_str().unwrap().parse().unwrap())
            .collect()
    }

    /// The key this machine's daemon holds, as a test that stands in for it reads it; its
    /// daemon stopped, so that the other machines take a link of that key again.
    fn stand_in(&self) -> MachineKey {
        let stopped = self.user.run(&["daemon", "stop"]);
        assert!(stopped.status.success(), "{}", self.log());
        MachineKey::load_or_make(&self.user.path("state/flottille/remote")).unwrap()
    }

    fn seen(&self, name: &str) -> Value {
        let machines = self.user.call("remote.machines", json!({})).unwrap();
        machines["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|machine| machine["name"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn wait_for(&self, name: &str) {
        eventually(&format!("{name} online"), || {
            self.seen(name)["state"] == "online"
        });
    }

    /// Waits until `name` offers the project `app` with `slots` free, Claude signed in.
    fn wait_for_offer(&self, name: &str, slots: u64) {
        eventually(&format!("{name} offering {slots} slots"), || {
            let offers = &self.seen(name)["offers"];
            offers["agents"]
                .as_array()
                .is_some_and(|agents| agents.iter().any(|agent| agent == "claude"))
                && offers["projects"].as_array().is_some_and(|projects| {
                    projects
                        .iter()
                        .any(|project| project["name"] == "app" && project["slots"] == slots)
                })
        });
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    /// The project `app` of this machine, its one repository at `work/app`, which need not be
    /// there; `more` the more lines of its file.
    fn register(&self, more: &str) {
        let text = format!(
            "folder = \"{}\"\nworkspaces_root = \"{}\"\nbranch_prefix = \"\"\n{more}\n\
             [[repositories]]\npath = \".\"\nbase = [\"main\"]\n",
            self.folder().display(),
            self.user.path("work/app-work").display()
        );
        self.user.write_project_file("app", &text);
        // A file edited by hand is seen at the next request about projects.
        self.user.call("project.list", json!({})).unwrap();
    }

    fn folder(&self) -> PathBuf {
        self.user.path("work/app")
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        self.user.git_output(dir, args).trim().to_owned()
    }

    fn agents(&self) -> Vec<Value> {
        self.user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn agent(&self, id: &str) -> Value {
        self.user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
    }

    /// A session the user starts in `app`'s folder, its first turn ended, in `mode`: its id and
    /// its MCP session.
    fn root(&self, mode: &str) -> (String, Mcp) {
        let opened = self.user.received_requests("session/new").len();
        let started = self
            .user
            .call("agent.run", json!({"project": "app", "message": "hello"}))
            .unwrap();
        let id = started["agent"]["id"].as_str().unwrap().to_owned();
        let (url, authorization) = server_of(&self.user, opened);
        eventually("the root's first turn", || {
            self.agent(&id)["state"] == "idle"
        });
        self.user
            .call("agent.mode", json!({"agent": id, "mode": mode}))
            .unwrap();
        (id, Mcp::initialize(&url, &authorization))
    }

    /// What this machine's fake agents were sent as prompts, each prompt's text.
    fn prompts(&self) -> Vec<String> {
        self.user
            .received_requests("session/prompt")
            .iter()
            .map(|prompt| prompt["params"]["prompt"].to_string())
            .collect()
    }

    /// The pending requests of this machine's user.
    fn requests(&self) -> Vec<Value> {
        self.user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .clone()
    }
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `pc` set up as the hub's first member.
fn set_up(hub: &TestHub, pc: &Machine) {
    let output = pc.user.run_with_input(
        &["remote", "setup", &hub.url(), "--name", "pc"],
        &format!("{}\n", hub.code()),
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        pc.log()
    );
}

/// `joiner` joins the hub of `inviter` as the user pairs them, as `name` with `role`.
fn pair(inviter: &Machine, joiner: &Machine, name: &str, role: &str) {
    let invited = inviter.user.run(&["remote", "invite", "--json"]);
    assert!(invited.status.success(), "{}", inviter.log());
    let invited: Value = serde_json::from_slice(&invited.stdout).unwrap();
    let code = invited["code"].as_str().unwrap().to_owned();
    let mut joining = joiner
        .user
        .command(&["remote", "join"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = joining.stdin.take().unwrap();
        input.write_all(format!("{code}\n").as_bytes()).unwrap();
    }
    let mut out = BufReader::new(joining.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    let deadline = Instant::now() + WAIT;
    let request = loop {
        let requests = inviter.user.call("remote.requests", json!({})).unwrap();
        if let Some(first) = requests.as_array().and_then(|all| all.first()) {
            break first.clone();
        }
        assert!(Instant::now() < deadline, "no join request came");
        std::thread::sleep(Duration::from_millis(200));
    };
    let id = request["request"].as_str().unwrap();
    let accepted = inviter
        .user
        .run(&["remote", "accept", id, "--name", name, "--role", role]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    let ended = joining.wait_with_output().unwrap();
    assert!(
        ended.status.success(),
        "the join failed: {}\n{}",
        String::from_utf8_lossy(&ended.stderr),
        joiner.log()
    );
}

/// A hub, `pc` its first member and `server` (`server_user`'s machine) paired with it as a
/// runner, the two linked; `pc` has the project `app`, its repository on `feat/export` with two
/// commits, an ignore rule and uncommitted work; `server` has `app` registered with
/// `server_project` as its more lines, or not at all when it is `None`.
fn pc_and_server(server_user: User, server_project: Option<&str>) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let pc_user = runner_user();
    sign_in(&pc_user);
    let pc = Machine::of(pc_user);
    set_up(&hub, &pc);
    let server = Machine::of(server_user);
    pair(&pc, &server, "server", "runner");
    pc.wait_for("server");
    server.wait_for("pc");
    let repo = pc.folder();
    pc.user.git_init(&repo);
    std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    pc.git(&repo, &["add", "."]);
    pc.git(&repo, &["commit", "--quiet", "-m", "first"]);
    pc.git(&repo, &["checkout", "--quiet", "-b", "feat/export"]);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    pc.git(&repo, &["add", "b.txt"]);
    pc.git(&repo, &["commit", "--quiet", "-m", "b"]);
    pc.register("");
    if let Some(more) = server_project {
        server.register(more);
    }
    // The caller's work in progress: a change and a new file, neither committed.
    std::fs::write(repo.join("a.txt"), "a, changed\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    (hub, pc, server)
}

/// A server running agents, Claude signed in, with `judge` its judge's steps when it has one.
fn server_user(judge: Option<Value>) -> User {
    let user = runner_user();
    sign_in(&user);
    match judge {
        Some(steps) => user.with_judge(steps),
        None => user,
    }
}

fn allowing() -> Option<Value> {
    Some(json!([{"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}]))
}

fn launched(mcp: &Mcp, args: Value) -> Answer {
    mcp.call("agent_launch", args)
}

fn done(answer: &Answer) -> &str {
    assert!(!answer.error, "an error: {}", answer.text);
    &answer.text
}

fn refused(answer: &Answer) -> &str {
    assert!(
        answer.text.starts_with("refused"),
        "not refused: {}",
        answer.text
    );
    &answer.text
}

/// The agent the answer of a launch on another machine names: `(agent <id>)`.
fn id_in(text: &str) -> String {
    let at = text
        .find("(agent ")
        .unwrap_or_else(|| panic!("no agent in {text}"));
    text[at + 7..at + 19].to_owned()
}

/// The one agent of `machine` whose title is `title`, once there.
fn titled(machine: &Machine, title: &str) -> Value {
    let mut found = Value::Null;
    eventually(&format!("an agent titled {title}"), || {
        match machine
            .agents()
            .into_iter()
            .find(|agent| agent["title"] == title)
        {
            Some(agent) => {
                found = agent;
                true
            }
            None => false,
        }
    });
    // `eventually` takes a `Fn`: the agent is read once more.
    machine
        .agents()
        .into_iter()
        .find(|agent| agent["title"] == title)
        .unwrap_or(found)
}

/// The folder of the agent's place on its machine.
fn folder_of(agent: &Value) -> PathBuf {
    PathBuf::from(agent["folder"].as_str().unwrap())
}

/// Waits until the agent `id` of `machine` was sent a prompt holding `text`.
fn prompted(machine: &Machine, text: &str) {
    eventually(&format!("a prompt with {text}"), || {
        machine.prompts().iter().any(|prompt| prompt.contains(text))
    });
}

#[test]
fn a_led_session_starts_on_another_machine_with_the_caller_s_work() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export of the report."}),
    );

    let text = done(&answer);
    assert!(text.contains("Started a session led by you"), "{text}");
    assert!(text.contains("on server"), "{text}");
    let session = titled(&server, "Finish the export");
    assert_eq!(id_in(text), session["id"].as_str().unwrap(), "{text}");
    // A session there, not a child: no parent, a lead.
    assert_eq!(session["parent"], Value::Null);
    assert_eq!(session["role"], "lead");
    // On the caller's branch, with its work as uncommitted changes.
    let there = folder_of(&session);
    assert_eq!(
        server.git(&there, &["branch", "--show-current"]),
        "feat/export"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("a.txt")).unwrap(),
        "a, changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("new.txt")).unwrap(),
        "new\n"
    );
    assert_eq!(std::fs::read_to_string(there.join("b.txt")).unwrap(), "b\n");
    // Its first message is the caller's, named as the agent of pc that wrote it, never the user's.
    prompted(&server, "Finish the export of the report.");
    let first = server
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains("Finish the export of the report."))
        .unwrap();
    assert!(first.contains(&format!("agent {root} of pc")), "{first}");
    // The caller's own branch and files are untouched.
    assert_eq!(
        pc.git(&pc.folder(), &["branch", "--show-current"]),
        "feat/export"
    );
    assert_eq!(
        std::fs::read_to_string(pc.folder().join("a.txt")).unwrap(),
        "a, changed\n"
    );
}

#[test]
fn a_child_starts_on_another_machine_in_a_copy_of_its_parent_s_place() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    );

    let text = done(&answer);
    assert!(text.contains("Launched"), "{text}");
    assert!(text.contains("on server"), "{text}");
    assert!(text.contains("refs/flottille/machines/server/"), "{text}");
    let child = titled(&server, "Write the tests");
    assert_eq!(id_in(text), child["id"].as_str().unwrap(), "{text}");
    assert_eq!(child["role"], "worker");
    let there = folder_of(&child);
    assert_eq!(
        server.git(&there, &["branch", "--show-current"]),
        "feat/export"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("new.txt")).unwrap(),
        "new\n"
    );
    // A child's brief, not a session for the user.
    prompted(&server, "Write the tests of the export.");
    // It counts in the server's cap of children of app.
    pc.wait_for_offer("server", 11);
}

#[test]
fn without_machine_agent_launch_is_unchanged() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "child", "title": "Write the tests", "brief": "Write them."}),
    );

    let text = done(&answer);
    assert!(text.contains("in your own place"), "{text}");
    let child = titled(&pc, "Write the tests");
    assert_eq!(child["parent"], root.as_str());
    assert_eq!(folder_of(&child), pc.folder());
    // Nothing went to the server.
    assert!(server.agents().is_empty());
    assert!(!server.log().contains("received"), "{}", server.log());
}

#[test]
fn the_description_says_the_child_works_on_a_copy_and_its_commits_come_back_on_a_branch() {
    let (_hub, pc, _server) = pc_and_server(server_user(None), Some(""));
    let (_root, mcp) = pc.root("auto");

    let tool = mcp
        .tools()
        .into_iter()
        .find(|tool| tool["name"] == "agent_launch")
        .unwrap();

    let description = tool["description"].as_str().unwrap();
    for said in [
        "machine",
        "copy of your place",
        "uncommitted changes",
        "refs/flottille/machines/<machine>/<launch>/<branch>",
        "nothing moves your branch",
    ] {
        assert!(description.contains(said), "{said}: {description}");
    }
    let machine = &tool["inputSchema"]["properties"]["machine"];
    assert_eq!(machine["type"], "string", "{tool:#}");
    let docs =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/tools.md"))
            .unwrap();
    assert!(
        docs.contains("refs/flottille/machines/<machine>/<launch>/<branch>")
            && docs.contains("`machine`"),
        "docs/tools.md does not say it"
    );
}

#[test]
fn the_agent_has_no_tool_or_network_to_move_code() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let (_root, mcp) = pc.root("auto");

    let names = mcp.names();
    // Only the tools an agent always had: none sends, fetches or pushes code.
    for name in &names {
        for word in [
            "git", "bundle", "travel", "push", "fetch", "machine", "link",
        ] {
            assert!(!name.contains(word), "{name}");
        }
    }
    // `machine` is the name of one of the user's machines, never an address.
    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": server.addresses()[0].to_string(),
            "title": "Elsewhere", "brief": "Go."}),
    );
    let text = refused(&answer);
    assert!(text.contains("no machine of the user's is named"), "{text}");
    // A push by a line is the gate's as ever: base, held for the user, never run.
    let answer = mcp.call("run", json!({"line": "git push origin feat/export"}));
    assert!(
        answer.text.contains("Waiting for the user's approval"),
        "{}",
        answer.text
    );
    assert!(server.agents().is_empty());
}

#[test]
fn a_launch_to_an_offline_machine_is_refused_at_once() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let (_root, mcp) = pc.root("auto");
    let stopped = server.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success());
    eventually("server offline", || pc.seen("server")["state"] == "offline");

    let started = Instant::now();
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Night", "brief": "Work."}),
    );

    let text = refused(&answer);
    assert!(text.contains("server is offline since"), "{text}");
    assert!(started.elapsed() < Duration::from_secs(5), "it waited");
}

#[test]
fn a_launch_to_a_controller_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let tablet = Machine::of(User::new());
    pair(&pc, &tablet, "tablet", "controller");
    pc.wait_for("tablet");
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "tablet", "title": "Draw", "brief": "Draw it."}),
    );

    let text = refused(&answer);
    assert!(
        text.contains("tablet is a controller: it runs no agent, a runner does"),
        "{text}"
    );
    assert!(tablet.agents().is_empty());
    assert!(server.agents().is_empty());
}

#[test]
fn a_runner_with_no_free_slot_is_refused_naming_those_that_would_take_it() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some("max_children = 1"));
    let laptop_user = server_user(None);
    let laptop = Machine::of(laptop_user);
    pair(&pc, &laptop, "laptop", "runner");
    laptop.register("");
    pc.wait_for("laptop");
    pc.wait_for_offer("server", 1);
    pc.wait_for_offer("laptop", 12);
    let (_root, mcp) = pc.root("auto");
    done(&launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "First", "brief": "One."}),
    ));
    pc.wait_for_offer("server", 0);

    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Second", "brief": "Two."}),
    );

    let text = refused(&answer);
    assert!(
        text.contains("server has no slot free for project app"),
        "{text}"
    );
    assert!(text.contains("laptop would take it"), "{text}");
    // Never queued: the server has its one child, and nothing waits.
    assert_eq!(server.agents().len(), 1);
}

#[test]
fn a_project_not_registered_there_is_refused_with_what_to_do() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    eventually("server's offer", || {
        pc.seen("server")["offers"]["agents"]
            .as_array()
            .is_some_and(|agents| !agents.is_empty())
    });
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Night", "brief": "Work."}),
    );

    let text = refused(&answer);
    assert!(
        text.contains(
            "the project app is not on server: register it there with flottille --machine \
             server project add <folder>"
        ),
        "{text}"
    );
    assert!(server.agents().is_empty());
}

#[test]
fn the_receiving_machine_s_gate_judges_the_launch_in_ask_and_auto() {
    let judging = Some(json!([{"say":
        "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}]));
    let (_hub, pc, server) = pc_and_server(server_user(judging), Some(""));
    pc.wait_for_offer("server", 12);

    // Ask: held for the user of the server, the caller answered with its number there.
    let (asking, mcp) = pc.root("ask");
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export of the report, whole."}),
    );
    let text = done(&answer);
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:#?}");
    let request = &requests[0];
    let number = request["number"].as_u64().unwrap();
    assert!(
        text.contains(&format!(
            "request #{number} on server is waiting for the user"
        )),
        "{text}"
    );
    let card = request.to_string();
    assert!(
        card.contains(&format!(
            "Start a session led by agent {asking} of pc on this machine, in a new workspace on \
             feat/export with its uncommitted changes"
        )),
        "{card}"
    );
    assert!(
        card.contains("Finish the export of the report, whole."),
        "{card}"
    );
    // Nothing started: its first message waits for the user.
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !server
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("Finish the export of the report")),
        "a session started before the user allowed it"
    );
    assert!(
        server.user.judge_received().is_empty(),
        "the judge was asked in Ask"
    );
    server
        .user
        .call("approval.approve", json!({"number": number}))
        .unwrap();
    prompted(&server, "Finish the export of the report, whole.");

    // Auto: the server's judge decides, given the words and the brief whole.
    let (_judged, mcp) = pc.root("auto");
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Second export",
            "brief": "Export the second report."}),
    );
    let text = done(&answer);
    let session = titled(&server, "Second export");
    assert_eq!(id_in(text), session["id"].as_str().unwrap());
    let judged = server
        .user
        .judge_received()
        .iter()
        .map(Value::to_string)
        .collect::<String>();
    assert!(judged.contains("of pc on this machine"), "{judged}");
    assert!(judged.contains("Export the second report."), "{judged}");
    prompted(&server, "Export the second report.");
}

#[test]
fn full_needs_the_user_s_signed_switch_otherwise_auto() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    // Full, switched on the caller's machine itself: no device signed it.
    let (root, mcp) = pc.root("full");

    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Unsigned", "brief": "One."}),
    ));

    let session = titled(&server, "Unsigned");
    assert_eq!(session["gate_mode"], "auto", "{session:#}");
    let judged = server.user.judge_received().len();
    assert!(judged > 0, "the judge was not asked");

    // Full, switched by the user from a device, signed: kept and sent with the launch.
    let switched = server
        .user
        .run(&["--machine", "pc", "agent", "mode", &root, "full"]);
    assert!(
        switched.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&switched.stderr),
        pc.log()
    );
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Signed", "brief": "Two."}),
    ));
    let session = titled(&server, "Signed");
    assert_eq!(session["gate_mode"], "full", "{session:#}");
    assert_eq!(
        server.user.judge_received().len(),
        judged,
        "the judge was asked in Full"
    );

    // Switched again on the caller's machine: the signed switch is no longer kept.
    pc.user
        .call("agent.mode", json!({"agent": root, "mode": "auto"}))
        .unwrap();
    pc.user
        .call("agent.mode", json!({"agent": root, "mode": "full"}))
        .unwrap();
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Again", "brief": "Three."}),
    ));
    assert_eq!(titled(&server, "Again")["gate_mode"], "auto");
}

/// A link to `to` dialled with `key`, as the daemon holding that key would dial it.
async fn dial(key: &MachineKey, to: &Machine, to_key: PublicKey, addresses: &[SocketAddr]) -> Link {
    let endpoint = Endpoint::dialler(key).unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let hello = Hello::ours(env!("CARGO_PKG_VERSION"));
        match endpoint
            .connect(to_key, addresses, Traffic::Machine, &hello, "to")
            .await
        {
            Ok(link) => return link,
            Err(error) => assert!(Instant::now() < deadline, "{error}\n{}", to.log()),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// What `machine` answers each of `messages`, sent in turn on one link dialled with `key`.
fn sent_with(
    key: &MachineKey,
    machine: &Machine,
    messages: Vec<MachineMessage>,
) -> Vec<Result<Value, String>> {
    let (to_key, addresses) = (machine.key(), machine.addresses());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let link = dial(key, machine, to_key, &addresses).await;
        let mut answers = Vec::new();
        for message in messages {
            answers.push(
                link.send(&Message::Machine(message))
                    .await
                    .map_err(|error| error.to_string()),
            );
        }
        answers
    })
}

/// A launch as a machine's daemon sends it, for `caller`.
fn launch_ask(launch: &str, caller: &str) -> LaunchAsk {
    LaunchAsk {
        launch: launch.into(),
        caller: caller.into(),
        tree: caller.into(),
        kind: "led".into(),
        project: "app".into(),
        title: "Forged".into(),
        brief: "Do it.".into(),
        provider: "claude".into(),
        model: None,
        effort: None,
        role: None,
        mode: "full".into(),
        switch: None,
        branch: "feat/export".into(),
        repositories: vec![".".into()],
    }
}

#[test]
fn a_forged_launch_without_a_recorded_caller_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let workspaces = server.user.call("workspace.list", json!({})).unwrap();
    let key = pc.stand_in();

    let answers = sent_with(
        &key,
        &server,
        vec![
            // A launch that names no agent of the sender's.
            MachineMessage::Launch(launch_ask("l1", "")),
            // Code for a launch this machine never recorded.
            MachineMessage::Git(Git::Offer(GitOffer {
                transfer: "l2/0".into(),
                launch: "l2".into(),
                project: "app".into(),
                repository: ".".into(),
                branch: "feat/export".into(),
                size: 1,
                sha256: "00".into(),
                back: false,
            })),
            MachineMessage::Git(Git::Place(GitPlace {
                launch: "l2".into(),
                project: "app".into(),
                branch: "feat/export".into(),
                repositories: vec![".".into()],
            })),
        ],
    );

    let launch = answers[0].clone().unwrap_err();
    assert!(launch.contains("names no agent"), "{launch}");
    for answer in &answers[1..] {
        let refused = answer.clone().unwrap_err();
        assert!(
            refused.contains("launch l2 was not accepted by this machine"),
            "{refused}"
        );
    }
    assert!(server.agents().is_empty());
    assert!(server.requests().is_empty());
    assert_eq!(
        server.user.call("workspace.list", json!({})).unwrap(),
        workspaces
    );
    // A full mode asked without the user's signed switch is not taken either: see
    // `full_needs_the_user_s_signed_switch_otherwise_auto`.
}

#[test]
fn a_provider_not_signed_in_there_is_refused_naming_it() {
    // A runner with Claude enabled and not signed in.
    let (_hub, pc, server) = pc_and_server(runner_user(), Some(""));
    eventually("server's offer", || {
        pc.seen("server")["offers"]["projects"]
            .as_array()
            .is_some_and(|projects| !projects.is_empty())
    });
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Night", "brief": "Work."}),
    );

    let text = refused(&answer);
    assert!(text.contains("claude is not signed in on server"), "{text}");
    assert!(server.agents().is_empty());
}

#[test]
fn a_child_above_either_cap_is_refused_not_queued() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some("max_children = 2"));
    // The parent's project allows one child at once.
    pc.register("max_children = 1");
    pc.wait_for_offer("server", 2);
    let (_root, mcp) = pc.root("auto");
    done(&launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "First", "brief": "One."}),
    ));

    // Above the parent's cap on its own machine: the child there counts in it.
    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Second", "brief": "Two."}),
    );
    let text = refused(&answer);
    assert!(
        text.contains("1 children already run in project app, its cap"),
        "{text}"
    );
    let local = launched(
        &mcp,
        json!({"kind": "child", "title": "Local", "brief": "Here."}),
    );
    assert!(refused(&local).contains("its cap"), "{}", local.text);

    // Above the receiving machine's cap: refused there, whatever the parent's allows.
    pc.register("max_children = 4");
    server.register("max_children = 1");
    pc.wait_for_offer("server", 0);
    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Third", "brief": "Three."}),
    );
    let text = refused(&answer);
    assert!(
        text.contains("server has no slot free for project app"),
        "{text}"
    );
    // Never queued: the server has its one child, and nothing waits.
    assert_eq!(server.agents().len(), 1);
    assert!(server.requests().is_empty());
}

#[test]
fn a_machine_leaves_a_bounded_number_of_launches_waiting_for_their_code() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let root = "0123456789ab";
    let key = pc.stand_in();

    let answers = sent_with(
        &key,
        &server,
        (0..5)
            .map(|n| MachineMessage::Launch(launch_ask(&format!("w{n}"), root)))
            .collect(),
    );

    // Held for the server's user (no signed switch: Auto, and no judge there asks the user).
    for answer in &answers[..4] {
        assert!(answer.is_ok(), "{answer:?}");
    }
    let refused = answers[4].clone().unwrap_err();
    assert!(
        refused.contains("pc has 4 launches waiting for their code already"),
        "{refused}"
    );
    assert!(server.agents().is_empty());
}
