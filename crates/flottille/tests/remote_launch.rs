//! `agent_launch` with `machine` starts a child or a led session on another of the user's
//! machines (`docs/remote.md`, "Starting a session on another machine"): the receiving machine's
//! gate judges it, the caller's place travels by itself, and the caller is answered at once. A hub
//! served in the test on 127.0.0.1 and daemons, each with its own temporary HOME and XDG folders,
//! in-memory keyring and fake agents; throwaway repositories. No build here travels code by the
//! test gate (`FLOTTILLE_TEST_TRAVEL`): code travels only with a launch.

use crate::common;
use crate::remote;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Git, GitOffer, GitPlace, Hello, LaunchAsk, Link, MachineMessage, Message, PublicKey, Traffic,
};
use serde_json::{Value, json};

/// How long a link, an offer or the hub's news may take: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(40);

/// The hub, served from a temporary state folder in a runtime of the test's own.
pub(crate) struct TestHub {
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

    /// Pair mode opened, as `flottille-hub pair` opens it on the hub's host: its code.
    fn code(&self) -> String {
        let url = self.url();
        self.served
            .with_hub(|hub| hub.open_pairing(&url, None).unwrap().1.code().unwrap())
    }
}

pub(crate) struct Machine {
    pub(crate) user: User,
}

/// A user whose agents are the fake agent, Claude enabled and signed in (its login file only
/// there), offers looked at often, the fake escalation programs first in its `PATH`.
pub(crate) fn runner_user() -> User {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_OFFER_MS", "300")
        .with_env("ANTHROPIC_API_KEY", "")
        .with_env("CLAUDE_CODE_OAUTH_TOKEN", "")
        .with_env("ANTHROPIC_AUTH_TOKEN", "");
    let claude = user.path("home/.claude");
    let stopping = user.path("stopping");
    let user = user
        .with_env("CLAUDE_CONFIG_DIR", claude.to_str().unwrap())
        .with_env("FLOTTILLE_TEST_STOPPING_FILE", stopping.to_str().unwrap())
        .with_fake_escalation();
    user.write_providers("[claude]\nenabled = true\n");
    user
}

/// Claude's login file on `user`'s machine: its presence is all that is looked at.
pub(crate) fn sign_in(user: &User) {
    let folder = user.path("home/.claude");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(".credentials.json"), "{}").unwrap();
}

impl Machine {
    /// `user`'s machine, with remote on: its links go through the hub.
    pub(crate) fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\n",
        )
        .unwrap();
        user.status();
        Machine { user }
    }

    pub(crate) fn this(&self) -> Value {
        self.user.call("remote.machines", json!({})).unwrap()["this"].clone()
    }

    pub(crate) fn key(&self) -> PublicKey {
        serde_json::from_value(self.this()["key"].clone()).unwrap()
    }

    /// The key folder of this machine's daemon, as a test that stands in for it reads it; its
    /// daemon stopped, so that the other machines take a link of that key again, and the hub a
    /// session of it.
    pub(crate) fn stand_in(&self) -> PathBuf {
        let stopped = self.user.run(&["daemon", "stop"]);
        assert!(stopped.status.success(), "{}", self.log());
        // The others let its link go at their next round, once the hub says it is gone: the
        // test's link of its key is not refused for that one.
        std::thread::sleep(Duration::from_secs(2));
        self.user.path("state/flottille/remote")
    }

    pub(crate) fn seen(&self, name: &str) -> Value {
        let machines = self.user.call("remote.machines", json!({})).unwrap();
        machines["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|machine| machine["name"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub(crate) fn wait_for(&self, name: &str) {
        eventually(&format!("{name} online"), || {
            self.seen(name)["state"] == "online"
        });
    }

    /// Waits until `name` offers the project `app` with `slots` free, Claude signed in.
    pub(crate) fn wait_for_offer(&self, name: &str, slots: u64) {
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

    pub(crate) fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    /// The project `app` of this machine, its one repository at `work/app`, which need not be
    /// there; `more` the more lines of its file.
    pub(crate) fn register(&self, more: &str) {
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

    pub(crate) fn folder(&self) -> PathBuf {
        self.user.path("work/app")
    }

    pub(crate) fn git(&self, dir: &Path, args: &[&str]) -> String {
        self.user.git_output(dir, args).trim().to_owned()
    }

    pub(crate) fn agents(&self) -> Vec<Value> {
        self.user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .clone()
    }

    pub(crate) fn agent(&self, id: &str) -> Value {
        self.user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
    }

    /// A session the user starts in `app`'s folder, its first turn ended, in `mode`: its id and
    /// its MCP session.
    pub(crate) fn root(&self, mode: &str) -> (String, Mcp) {
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
    pub(crate) fn prompts(&self) -> Vec<String> {
        self.user
            .received_requests("session/prompt")
            .iter()
            .map(|prompt| prompt["params"]["prompt"].to_string())
            .collect()
    }

    /// The pending requests of this machine's user.
    pub(crate) fn requests(&self) -> Vec<Value> {
        self.user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .clone()
    }
}

pub(crate) fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `machine` paired with `hub` as the user pairs it, as `name` with `role`.
pub(crate) fn pair(hub: &TestHub, machine: &Machine, name: &str, role: &str) {
    let output = machine.user.run_with_input(
        &["remote", "pair", "--name", name, "--role", role],
        &format!(
            "{}
",
            hub.code()
        ),
    );
    assert!(
        output.status.success(),
        "the pairing failed: {}
{}",
        String::from_utf8_lossy(&output.stderr),
        machine.log()
    );
}

/// A hub, `pc` its first member and `server` (`server_user`'s machine) paired with it as a
/// runner, the two linked; `pc` has the project `app`, its repository on `feat/export` with two
/// commits, an ignore rule and uncommitted work; `server` has `app` registered with
/// `server_project` as its more lines, or not at all when it is `None`.
pub(crate) fn pc_and_server(
    server_user: User,
    server_project: Option<&str>,
) -> (TestHub, Machine, Machine) {
    pc_and_server_as("runner", server_user, server_project)
}

/// [`pc_and_server`], `pc` paired as the hub's first device with `pc_role`.
pub(crate) fn pc_and_server_as(
    pc_role: &str,
    server_user: User,
    server_project: Option<&str>,
) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let pc_user = runner_user();
    sign_in(&pc_user);
    let pc = Machine::of(pc_user);
    pair(&hub, &pc, "pc", pc_role);
    let server = Machine::of(server_user);
    pair(&hub, &server, "server", "runner");
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
pub(crate) fn server_user(judge: Option<Value>) -> User {
    let user = runner_user();
    sign_in(&user);
    match judge {
        Some(steps) => user.with_judge(steps),
        None => user,
    }
}

pub(crate) fn allowing() -> Option<Value> {
    Some(json!([{"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}]))
}

pub(crate) fn launched(mcp: &Mcp, args: Value) -> Answer {
    mcp.call("agent_launch", args)
}

pub(crate) fn done(answer: &Answer) -> &str {
    assert!(!answer.error, "an error: {}", answer.text);
    &answer.text
}

pub(crate) fn refused(answer: &Answer) -> &str {
    assert!(
        answer.text.starts_with("refused"),
        "not refused: {}",
        answer.text
    );
    &answer.text
}

/// The agent the answer of a launch on another machine names: `(agent <id>)`.
pub(crate) fn id_in(text: &str) -> String {
    let at = text
        .find("(agent ")
        .unwrap_or_else(|| panic!("no agent in {text}"));
    text[at + 7..at + 19].to_owned()
}

/// The one agent of `machine` whose title is `title`, once there.
pub(crate) fn titled(machine: &Machine, title: &str) -> Value {
    let find = || {
        machine
            .agents()
            .into_iter()
            .find(|agent| agent["title"] == title)
    };
    eventually(&format!("an agent titled {title}"), || find().is_some());
    // The whole agent, as `agent.show` gives it: the list gives a summary.
    machine.agent(find().unwrap()["id"].as_str().unwrap())
}

/// The folder of the agent's place on its machine.
pub(crate) fn folder_of(agent: &Value) -> PathBuf {
    PathBuf::from(agent["folder"].as_str().unwrap())
}

/// Waits until the agent `id` of `machine` was sent a prompt holding `text`.
pub(crate) fn prompted(machine: &Machine, text: &str) {
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
    assert_eq!(
        server.user.call("workspace.list", json!({})).unwrap()["workspaces"],
        json!([])
    );
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
    for said in ["machine", "copy of your place"] {
        assert!(description.contains(said), "{said}: {description}");
    }
    // The description holds a limit; the parameter says the rest.
    let machine = &tool["inputSchema"]["properties"]["machine"];
    assert_eq!(machine["type"], "string", "{tool:#}");
    let said_of_machine = machine["description"].as_str().unwrap();
    for said in [
        "copy of your place",
        "uncommitted changes",
        "refs/flottille/machines/<machine>/<launch>/<branch>",
        "nothing moves your branch",
    ] {
        assert!(said_of_machine.contains(said), "{said}: {said_of_machine}");
    }
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
        json!({"kind": "child", "machine": "127.0.0.1:7447",
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
    let (hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let tablet = Machine::of(User::new());
    pair(&hub, &tablet, "tablet", "controller");
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
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some("max_children = 1"));
    let laptop_user = server_user(None);
    let laptop = Machine::of(laptop_user);
    pair(&hub, &laptop, "laptop", "runner");
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

/// A link to `to` dialled through the hub with the key in the key folder `key`, as the daemon
/// holding that key would dial it.
pub(crate) async fn dial(key: &Path, to: &Machine, to_key: PublicKey) -> Link {
    let endpoint = remote::endpoint_of(key).await;
    let deadline = Instant::now() + WAIT;
    loop {
        match remote::dial(&endpoint, to_key, Traffic::Machine, "to").await {
            Ok(link) => return link,
            Err(error) => assert!(Instant::now() < deadline, "{error}\n{}", to.log()),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// What `machine` answers each of `messages`, sent in turn on one link dialled with `key`.
pub(crate) fn sent_with(
    key: &Path,
    machine: &Machine,
    messages: Vec<MachineMessage>,
) -> Vec<Result<Value, String>> {
    let to_key = machine.key();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let link = dial(key, machine, to_key).await;
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
pub(crate) fn launch_ask(launch: &str, caller: &str) -> LaunchAsk {
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
        identities: Vec::new(),
        settings: None,
        copied: Vec::new(),
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
            MachineMessage::Launch(Box::new(launch_ask("l1", ""))),
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
            .map(|n| MachineMessage::Launch(Box::new(launch_ask(&format!("w{n}"), root))))
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

/// The project `app` of `server`, its setup a step that writes the file it gives: what shows the
/// setup ran.
fn with_marking_setup(server: &Machine) -> PathBuf {
    let mark = server.user.path("setup-ran");
    let script = server.user.path("setup.sh");
    std::fs::write(&script, format!("touch {}\n", mark.display())).unwrap();
    server.register(&format!("[[setup]]\nrun = \"sh {}\"\n", script.display()));
    mark
}

/// The names of the workspaces of `machine`.
fn workspaces(machine: &Machine) -> Vec<String> {
    machine.user.call("workspace.list", json!({})).unwrap()["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap().to_owned())
        .collect()
}

/// `machine`'s daemon stopped and started again.
pub(crate) fn restarted(machine: &Machine) {
    let stopped = machine.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", machine.log());
    machine.user.status();
}

/// Waits long enough for a setup that would run to have run.
pub(crate) fn a_while() {
    std::thread::sleep(Duration::from_secs(2));
}

/// A launch held for the server's user, its code placed there: the request's number.
fn held_launch(pc: &Machine, server: &Machine, mcp: &Mcp, title: &str) -> u64 {
    let before = server.requests().len();
    done(&launched(
        mcp,
        json!({"kind": "led", "machine": "server", "title": title, "brief": "Do the work."}),
    ));
    let requests = server.requests();
    assert_eq!(requests.len(), before + 1, "{requests:#?}\n{}", pc.log());
    requests
        .iter()
        .filter_map(|request| request["number"].as_u64())
        .max()
        .unwrap()
}

#[test]
fn a_held_launch_runs_no_setup_step_until_the_user_allows() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    let mark = with_marking_setup(&server);
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");

    let number = held_launch(&pc, &server, &mcp, "Held setup");

    assert_eq!(workspaces(&server).len(), 1);
    a_while();
    assert!(
        !mark.exists(),
        "the setup ran before the user allowed the launch"
    );
    restarted(&server);
    a_while();
    assert!(
        !mark.exists(),
        "the setup ran at a restart before the user allowed it"
    );

    server
        .user
        .call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the setup, once allowed", || mark.exists());
    prompted(&server, "Do the work.");
}

#[test]
fn a_refused_launch_never_runs_its_setup_even_after_a_restart() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    let mark = with_marking_setup(&server);
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");
    let number = held_launch(&pc, &server, &mcp, "Refused setup");

    server
        .user
        .call("approval.deny", json!({"number": number}))
        .unwrap();

    // Its workspace goes with it, and nothing of its setup ever runs.
    eventually("the refused launch's workspace removed", || {
        workspaces(&server).is_empty()
    });
    restarted(&server);
    a_while();
    assert!(!mark.exists(), "the setup of a refused launch ran");
    assert!(workspaces(&server).is_empty());
    assert!(
        !server
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("Do the work.")),
        "a refused launch started"
    );
}

#[test]
fn a_swept_launch_leaves_no_workspace_to_set_up() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    let mark = with_marking_setup(&server);
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("ask");
    held_launch(&pc, &server, &mcp, "Swept setup");
    let key = pc.stand_in();

    // A launch whose agent was never made, its code placed more than an hour ago: what the sweep
    // forgets the next time that machine asks for a launch.
    let stopped = server.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", server.log());
    {
        let store =
            rusqlite::Connection::open(server.user.path("state/flottille/flottille.db")).unwrap();
        store
            .execute(
                "UPDATE agents SET state = 'ended' WHERE id IN \
                 (SELECT agent FROM launches WHERE way = 'in')",
                [],
            )
            .unwrap();
        store
            .execute(
                "UPDATE launches SET agent = NULL, request = NULL, at = 0 WHERE way = 'in'",
                [],
            )
            .unwrap();
    }
    server.user.status();
    let answers = sent_with(
        &key,
        &server,
        vec![MachineMessage::Launch(Box::new(launch_ask("next", &root)))],
    );
    assert!(answers[0].is_ok(), "{answers:?}");

    eventually("the swept launch's workspace removed", || {
        workspaces(&server).is_empty()
    });
    restarted(&server);
    a_while();
    assert!(!mark.exists(), "the setup of a swept launch ran");
}

#[test]
fn code_sent_for_another_project_than_the_launch_s_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    // A second project on the server, `other`, its own repository.
    let other = server.user.path("work/other");
    server.user.git_init(&other);
    std::fs::write(other.join("o.txt"), "o\n").unwrap();
    server.git(&other, &["add", "."]);
    server.git(&other, &["commit", "--quiet", "-m", "o"]);
    server.user.write_project_file(
        "other",
        &format!(
            "folder = \"{}\"\nworkspaces_root = \"{}\"\n\n[[repositories]]\npath = \".\"\n\
             base = [\"main\"]\n",
            other.display(),
            server.user.path("work/other-work").display()
        ),
    );
    server.user.call("project.list", json!({})).unwrap();
    let workspaces_before = workspaces(&server);
    let key = pc.stand_in();
    let place = |project: &str, branch: &str, repositories: &[&str]| {
        MachineMessage::Git(Git::Place(GitPlace {
            launch: "l1".into(),
            project: project.into(),
            branch: branch.into(),
            repositories: repositories.iter().map(|path| (*path).into()).collect(),
        }))
    };

    let answers = sent_with(
        &key,
        &server,
        vec![
            // Held for the user: recorded, for app on feat/export.
            MachineMessage::Launch(Box::new(launch_ask("l1", "0123456789ab"))),
            MachineMessage::Git(Git::Has(flottille_link::GitRepository {
                project: "other".into(),
                repository: ".".into(),
                launch: Some("l1".into()),
            })),
            MachineMessage::Git(Git::Offer(GitOffer {
                transfer: "l1/0".into(),
                launch: "l1".into(),
                project: "other".into(),
                repository: ".".into(),
                branch: "feat/export".into(),
                size: 1,
                sha256: "00".into(),
                back: false,
            })),
            place("other", "feat/export", &["."]),
            place("app", "main", &["."]),
            place("app", "feat/export", &[".", "lib"]),
        ],
    );

    assert!(answers[0].is_ok(), "{answers:?}");
    for answer in &answers[1..4] {
        let refused = answer.clone().unwrap_err();
        assert!(
            refused.contains("launch l1 is for project app"),
            "{refused}"
        );
    }
    let refused = answers[4].clone().unwrap_err();
    assert!(refused.contains("launch l1 is on feat/export"), "{refused}");
    let refused = answers[5].clone().unwrap_err();
    assert!(
        refused.contains("launch l1 takes the repositories ."),
        "{refused}"
    );
    assert_eq!(workspaces(&server), workspaces_before);
    assert!(
        !server.user.path("work/other/.git/refs/flottille").exists(),
        "code came into the other project"
    );
}

#[test]
fn a_launch_the_judge_denies_is_held_for_the_user_and_never_started() {
    // A judge that will not allow it says to ask the user: it has no verdict that refuses.
    let denying = json!([{"say":
        "{\"verdict\": \"ask\", \"reason\": \"Not for tonight.\"}"}]);
    let (_hub, pc, server) = pc_and_server(server_user(Some(denying)), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Denied", "brief": "Deny me."}),
    );

    let text = done(&answer);
    assert!(text.contains("is waiting for the user"), "{text}");
    assert!(
        !server.user.judge_received().is_empty(),
        "no judge was asked"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:#?}");
    assert!(requests[0].to_string().contains("Not for tonight."));
    a_while();
    assert!(
        !server
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("Deny me.")),
        "a launch the judge denied started"
    );
}

#[test]
fn a_launch_whose_judge_fails_is_held_for_the_user_and_never_started() {
    let failing = json!([{"say": "Looks fine to me."}]);
    let (_hub, pc, server) = pc_and_server(server_user(Some(failing)), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Unjudged", "brief": "Judge me."}),
    );

    let text = done(&answer);
    assert!(text.contains("is waiting for the user"), "{text}");
    assert!(
        !server.user.judge_received().is_empty(),
        "no judge was asked"
    );
    assert_eq!(server.requests().len(), 1);
    a_while();
    assert!(
        !server
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("Judge me.")),
        "a launch whose judge failed started"
    );
}

#[test]
fn a_machine_leaves_a_bounded_number_of_launches_held_for_the_user() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");
    for n in 0..4 {
        held_launch(&pc, &server, &mcp, &format!("Held {n}"));
    }

    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "One more", "brief": "More."}),
    );

    let text = refused(&answer);
    assert!(
        text.contains("pc has 4 launches waiting for the user already"),
        "{text}"
    );
    assert_eq!(server.requests().len(), 4);
}

/// What `machine` answers each of `messages`, all sent at once on one link dialled with `key`.
fn sent_together(
    key: &Path,
    machine: &Machine,
    messages: Vec<MachineMessage>,
) -> Vec<Result<Value, String>> {
    let to_key = machine.key();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let link = dial(key, machine, to_key).await;
        let sending: Vec<_> = messages
            .into_iter()
            .map(|message| {
                let link = link.clone();
                tokio::spawn(async move {
                    link.send(&Message::Machine(message))
                        .await
                        .map_err(|error| error.to_string())
                })
            })
            .collect();
        let mut answers = Vec::new();
        for sent in sending {
            answers.push(sent.await.unwrap());
        }
        answers
    })
}

/// The launches `server` holds for its user, as its store has them.
fn held_in_store(server: &Machine) -> i64 {
    let store =
        rusqlite::Connection::open(server.user.path("state/flottille/flottille.db")).unwrap();
    store
        .query_row(
            "SELECT COUNT(*) FROM launches WHERE way = 'in' AND state = 'held'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn launches_sent_at_once_never_leave_more_held_than_the_cap() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let key = pc.stand_in();

    let answers = sent_together(
        &key,
        &server,
        (0..12)
            .map(|n| MachineMessage::Launch(Box::new(launch_ask(&format!("c{n}"), "0123456789ab"))))
            .collect(),
    );

    let taken = answers.iter().filter(|answer| answer.is_ok()).count();
    assert!(taken <= 4, "{taken} launches taken: {answers:?}");
    assert!(held_in_store(&server) <= 4, "{answers:?}");
}

/// The user ends `agent` of `server`, keeps the work of its workspace on a remote of their own,
/// then removes that workspace and its branch: the workspace's name.
fn removed_by_the_user(server: &Machine, agent: &Value) -> String {
    let id = agent["id"].as_str().unwrap();
    let name = agent["workspace"].as_str().unwrap().to_owned();
    server
        .user
        .call("agent.stop", json!({"agent": id}))
        .unwrap();
    eventually("the agent ended", || {
        matches!(
            server.agent(id)["state"].as_str(),
            Some("ended" | "stopped" | "failed")
        )
    });
    let folder = folder_of(agent);
    let branch = server.git(&folder, &["branch", "--show-current"]);
    let kept = server.user.path(&format!("kept-{name}.git"));
    server.git(
        &server.folder(),
        &["init", "--quiet", "--bare", kept.to_str().unwrap()],
    );
    server.git(&folder, &["add", "-A"]);
    server.git(&folder, &["commit", "--quiet", "-m", "kept"]);
    server.git(&folder, &["remote", "add", "kept", kept.to_str().unwrap()]);
    server.git(&folder, &["push", "--quiet", "kept", &branch]);
    server
        .user
        .call("workspace.remove", json!({"project": "app", "name": name}))
        .unwrap();
    server.git(&server.folder(), &["branch", "-D", &branch]);
    name
}

#[test]
fn an_old_allowed_launch_lets_no_held_launch_of_the_same_name_set_up() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    let mark = with_marking_setup(&server);
    pc.wait_for_offer("server", 12);
    // Allowed by the server's judge: its setup runs.
    let (_allowing, mcp) = pc.root("auto");
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Allowed", "brief": "First."}),
    ));
    eventually("the allowed launch's setup", || mark.exists());
    let allowed = titled(&server, "Allowed");
    let name = removed_by_the_user(&server, &allowed);
    std::fs::remove_file(&mark).unwrap();

    // Held for the user, on the same branch: its workspace takes the name that is free again.
    let (_asking, mcp) = pc.root("ask");
    held_launch(&pc, &server, &mcp, "Held");

    assert_eq!(workspaces(&server), vec![name]);
    a_while();
    assert!(
        !mark.exists(),
        "the setup of a held launch ran on an old allowed launch's word"
    );
}

#[test]
fn a_forgotten_launch_never_removes_a_workspace_of_the_user_s_of_the_same_name() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("ask");
    held_launch(&pc, &server, &mcp, "Forgotten later");
    let held = titled(&server, "Forgotten later");
    // The user ends the held agent (its request goes with it), removes what its launch made, and
    // makes a workspace of their own of that name.
    let name = removed_by_the_user(&server, &held);
    let kept = server.git(
        &server.folder(),
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/remotes/kept",
        ],
    );
    server.git(&server.folder(), &["branch", "main", &kept]);
    let made = server
        .user
        .call(
            "workspace.create",
            json!({"project": "app", "name": name, "offline": true}),
        )
        .unwrap();
    let mine = PathBuf::from(made["folder"].as_str().unwrap());
    eventually("the user's workspace", || mine.join(".git").exists());
    std::fs::write(mine.join("mine.txt"), "mine\n").unwrap();
    let key = pc.stand_in();
    // Its launch, an hour old and its agent gone: what the sweep forgets the next time that
    // machine asks for a launch, removing what was made of its code.
    let stopped = server.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", server.log());
    {
        let store =
            rusqlite::Connection::open(server.user.path("state/flottille/flottille.db")).unwrap();
        store
            .execute(
                "UPDATE launches SET agent = NULL, request = NULL, at = 0 WHERE way = 'in'",
                [],
            )
            .unwrap();
    }
    server.user.status();

    let answers = sent_with(
        &key,
        &server,
        vec![MachineMessage::Launch(Box::new(launch_ask("next", &root)))],
    );

    assert!(answers[0].is_ok(), "{answers:?}");
    a_while();
    assert_eq!(workspaces(&server), vec![name]);
    assert_eq!(
        std::fs::read_to_string(mine.join("mine.txt")).unwrap(),
        "mine\n"
    );
}

/// The state of the launches `server` holds from other machines, by launch name.
fn launch_states(server: &Machine) -> Vec<String> {
    let store =
        rusqlite::Connection::open(server.user.path("state/flottille/flottille.db")).unwrap();
    let mut statement = store
        .prepare("SELECT state FROM launches WHERE way = 'in' ORDER BY launch")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|state| state.unwrap())
        .collect()
}

#[test]
fn stopping_a_held_launch_s_agent_settles_it_and_frees_its_place() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    let mark = with_marking_setup(&server);
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");
    held_launch(&pc, &server, &mcp, "Stopped while held");
    let held = titled(&server, "Stopped while held");
    assert_eq!(launch_states(&server), vec!["held"]);
    assert_eq!(workspaces(&server).len(), 1);

    server
        .user
        .call("agent.stop", json!({"agent": held["id"]}))
        .unwrap();

    // It no longer waits for anyone: it does not count against the cap, and its workspace, made
    // of code that never ran, goes.
    eventually("the launch settled as refused", || {
        launch_states(&server) == vec!["refused"]
    });
    assert_eq!(held_in_store(&server), 0);
    eventually("its workspace removed", || workspaces(&server).is_empty());
    a_while();
    assert!(!mark.exists(), "the setup of a stopped launch ran");
}

#[test]
fn a_launch_with_an_unfit_title_caller_or_branch_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    let key = pc.stand_in();
    let unfit = |change: &dyn Fn(&mut LaunchAsk)| {
        let mut ask = launch_ask("u", "0123456789ab");
        change(&mut ask);
        MachineMessage::Launch(Box::new(ask))
    };

    let answers = sent_with(
        &key,
        &server,
        vec![
            unfit(&|ask| ask.title = "Two\nlines".into()),
            unfit(&|ask| ask.title = "t".repeat(201)),
            unfit(&|ask| ask.caller = "../../etc".into()),
            unfit(&|ask| ask.tree = "a b".into()),
            unfit(&|ask| ask.branch = "feat/..x".into()),
            unfit(&|ask| ask.branch = format!("feat/{}", "b".repeat(251))),
        ],
    );

    let said: Vec<String> = answers
        .into_iter()
        .map(|answer| answer.unwrap_err())
        .collect();
    assert!(said[0].contains("a title is one line"), "{}", said[0]);
    assert!(
        said[1].contains("a title is at most 200 characters"),
        "{}",
        said[1]
    );
    assert!(said[2].contains("is not an agent's id"), "{}", said[2]);
    assert!(said[3].contains("is not an agent's id"), "{}", said[3]);
    assert!(said[4].contains("is not a branch name"), "{}", said[4]);
    assert!(said[5].contains("is not a branch name"), "{}", said[5]);
    assert!(server.agents().is_empty());
    assert!(server.requests().is_empty());
}

#[test]
fn a_switch_to_full_older_than_twelve_hours_starts_the_launch_in_auto() {
    let ahead = tempfile::NamedTempFile::new().unwrap();
    let user = server_user(allowing()).with_env(
        "FLOTTILLE_TEST_WALL_AHEAD_FILE",
        ahead.path().to_str().unwrap(),
    );
    let (_hub, pc, server) = pc_and_server(user, Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("auto");
    let switched = server
        .user
        .run(&["--machine", "pc", "agent", "mode", &root, "full"]);
    assert!(switched.status.success(), "{}", pc.log());
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Fresh", "brief": "One."}),
    ));
    assert_eq!(titled(&server, "Fresh")["gate_mode"], "full");

    // Thirteen hours later on the server's clock: the switch no longer counts there.
    std::fs::write(ahead.path(), (13 * 60 * 60).to_string()).unwrap();
    pc.wait_for_offer("server", 12);
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Stale", "brief": "Two."}),
    ));

    assert_eq!(titled(&server, "Stale")["gate_mode"], "auto");
}

#[test]
fn a_switch_to_full_made_on_a_child_counts_for_its_tree() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("full");
    let child = done(&launched(
        &mcp,
        json!({"kind": "child", "title": "Local child", "brief": "Here."}),
    ))
    .to_owned();
    let child = pc
        .agents()
        .into_iter()
        .find(|agent| agent["title"] == "Local child")
        .unwrap_or_else(|| panic!("no child: {child}"));

    // The user switches the tree from its child, on a device: the switch names the child.
    let switched = server.user.run(&[
        "--machine",
        "pc",
        "agent",
        "mode",
        child["id"].as_str().unwrap(),
        "full",
    ]);
    assert!(switched.status.success(), "{}", pc.log());
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "From the child", "brief": "One."}),
    ));

    assert_eq!(titled(&server, "From the child")["gate_mode"], "full");
}

/// The states of the launches `pc` recorded on other machines, by launch name.
fn launches_out(pc: &Machine) -> Vec<String> {
    let store = rusqlite::Connection::open(pc.user.path("state/flottille/flottille.db")).unwrap();
    let mut statement = store
        .prepare("SELECT state FROM launches WHERE way = 'out' ORDER BY launch")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|state| state.unwrap())
        .collect()
}

/// The rows `pc` shows under their pilots of the sessions its agents started on other machines.
fn rows_under_pilots(pc: &Machine) -> Vec<Value> {
    pc.user.call("remote.launches", json!({})).unwrap()["launches"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The prompt `machine` was sent that holds `text`, once it was.
fn prompt_with(machine: &Machine, text: &str) -> String {
    prompted(machine, text);
    machine
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains(text))
        .unwrap()
}

#[test]
fn a_held_launch_allowed_there_tells_its_caller() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");
    let number = held_launch(&pc, &server, &mcp, "Allowed there");
    assert_eq!(launches_out(&pc), vec!["held"]);

    server
        .user
        .call("approval.approve", json!({"number": number}))
        .unwrap();

    let told = prompt_with(&pc, &format!("Request #{number} on server"));
    assert!(told.contains("allowed by the user of server"), "{told}");
    assert!(told.contains("runs now"), "{told}");
    assert!(
        told.contains("its questions, reports and end wake you"),
        "{told}"
    );
    assert!(told.contains("session_send"), "{told}");
    eventually("the launch recorded as started", || {
        launches_out(&pc) == vec!["started"]
    });
    let rows = rows_under_pilots(&pc);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["state"], "started", "{rows:#?}");
}

#[test]
fn a_held_launch_refused_there_tells_its_caller() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");
    let number = held_launch(&pc, &server, &mcp, "Refused there");

    server
        .user
        .call("approval.deny", json!({"number": number}))
        .unwrap();

    let told = prompt_with(&pc, &format!("Request #{number} on server"));
    assert!(told.contains("refused by the user of server"), "{told}");
    assert!(told.contains("Refused there"), "{told}");
    eventually("the launch recorded as refused", || {
        launches_out(&pc) == vec!["refused"]
    });
}

#[test]
fn a_refused_launch_leaves_no_waiting_row_under_its_pilot() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");
    let number = held_launch(&pc, &server, &mcp, "No row left");
    let rows = rows_under_pilots(&pc);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["state"], "held", "{rows:#?}");

    server
        .user
        .call("approval.deny", json!({"number": number}))
        .unwrap();

    eventually("no row left under the pilot", || {
        rows_under_pilots(&pc).is_empty()
    });
}

#[test]
fn a_held_launch_on_another_machine_says_what_follows_once_allowed() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("ask");

    let led = done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Led, held", "brief": "Do it."}),
    ))
    .to_owned();
    assert!(!led.contains("told nothing more"), "{led}");
    assert!(led.contains("Once the user allows it there"), "{led}");
    assert!(
        led.contains("its questions, reports and end wake you"),
        "{led}"
    );
    assert!(led.contains("session_send"), "{led}");
    assert!(led.contains("You are told when the user decides"), "{led}");

    let child = done(&launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Child, held", "brief": "Do it."}),
    ))
    .to_owned();
    assert!(child.contains("request #"), "{child}");
    assert!(!child.contains("told nothing more"), "{child}");
    assert!(child.contains("Once the user allows it there"), "{child}");
    assert!(child.contains("agent_send"), "{child}");
    assert!(
        child.contains("You are told when the user decides"),
        "{child}"
    );
}
