//! A place's code travels to another machine and its new commits come back (`docs/remote.md`,
//! "What travels"): a hub served in the test on 127.0.0.1 and daemons, each with its own
//! temporary HOME and XDG folders and in-memory keyring, the first machine set up with
//! `flottille remote setup` and the others joined as the user pairs them, each with its role;
//! and throwaway repositories. `agent_launch` is not built yet: the test build's `test.travel`
//! and `test.travel_back` start what a launch will.

use crate::common;
use crate::remote;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Git, GitFetch, GitOffer, GitPlace, Hello, Link, MachineMessage, Message, PublicKey, Traffic,
};
use serde_json::{Value, json};

/// How long a link, or the hub's news, may take: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(40);
/// How long a travel may take: git runs a few times on each side.
const TRAVEL: Duration = Duration::from_secs(60);

/// The hub, served from a temporary state folder in a runtime of the test's own.
struct TestHub {
    /// Serves the hub while the test runs.
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

struct Machine {
    user: User,
}

impl Machine {
    /// A machine whose test build travels code (`FLOTTILLE_TEST_TRAVEL`).
    fn new() -> Machine {
        Machine::of(User::new().with_env("FLOTTILLE_TEST_TRAVEL", "1"))
    }

    /// `user`'s machine, with remote on: its links go through the hub.
    fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\n",
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

    /// The key folder of this machine's daemon, as a test that stands in for it reads it; its
    /// daemon stopped, so that the other machines take a link of that key again, and the hub a
    /// session of it.
    fn stand_in(&self) -> PathBuf {
        let stopped = self.user.run(&["daemon", "stop"]);
        assert!(stopped.status.success(), "{}", self.log());
        self.user.path("state/flottille/remote")
    }

    fn wait_for(&self, name: &str) {
        let deadline = Instant::now() + WAIT;
        loop {
            let machines = self.user.call("remote.machines", json!({})).unwrap();
            if machines["machines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|machine| machine["name"] == name && machine["state"] == "online")
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{name} is not online\n{machines:#}\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    /// The project `app` of this machine, its one repository at `work/app`, which need not be
    /// there.
    fn register(&self) {
        self.register_with(&["."], "");
    }

    /// The project `app` of this machine, with `repositories` and the more lines `more`.
    fn register_with(&self, repositories: &[&str], more: &str) {
        let mut text = format!(
            "folder = \"{}\"\nworkspaces_root = \"{}\"\nbranch_prefix = \"\"\n{more}",
            self.folder().display(),
            self.user.path("work/app-work").display()
        );
        for path in repositories {
            text.push_str(&format!(
                "[[repositories]]\npath = \"{path}\"\nbase = [\"main\"]\n"
            ));
        }
        self.user.write_project_file("app", &text);
    }

    fn folder(&self) -> PathBuf {
        self.user.path("work/app")
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        self.user.git_output(dir, args).trim().to_owned()
    }

    /// `test.travel`: the code of `place` (a workspace of `app`, or the project's own folder)
    /// sent to `machine` for the launch `launch`.
    fn travel(&self, machine: &str, launch: &str, more: Value) -> Result<Value, (String, String)> {
        let mut params = json!({"machine": machine, "project": "app", "launch": launch});
        params
            .as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        self.user.call_within("test.travel", params, TRAVEL)
    }

    fn travel_back(&self, launch: &str) -> Result<Value, (String, String)> {
        self.user
            .call_within("test.travel_back", json!({"launch": launch}), TRAVEL)
    }

    /// The refs of `repository` under `refs/flottille/<kind>`.
    fn refs(&self, repository: &Path, kind: &str) -> String {
        self.git(
            repository,
            &[
                "for-each-ref",
                "--format=%(refname)",
                &format!("refs/flottille/{kind}"),
            ],
        )
    }
}

/// `pc` paired as the hub's first device, a runner.
fn set_up(hub: &TestHub, pc: &Machine) {
    pair(hub, pc, "pc", "runner");
}

/// `machine` paired with `hub` as the user pairs it, as `name` with `role`.
fn pair(hub: &TestHub, machine: &Machine, name: &str, role: &str) {
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

/// A link to `to` dialled through the hub with the key in the key folder `key`, as the daemon
/// holding that key would dial it.
async fn dial(key: &Path, to: &Machine, to_key: PublicKey) -> Link {
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

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// What `machine` answers `git` sent on a link dialled with the key in the key folder `key`.
fn sent_with(key: &Path, machine: &Machine, git: Git) -> Result<Value, String> {
    let to_key = machine.key();
    runtime().block_on(async {
        let link = dial(key, machine, to_key).await;
        link.send(&Message::Machine(MachineMessage::Git(git)))
            .await
            .map_err(|error| error.to_string())
    })
}

fn offer(launch: &str) -> Git {
    Git::Offer(GitOffer {
        transfer: format!("{launch}/0"),
        launch: launch.into(),
        project: "app".into(),
        repository: ".".into(),
        branch: "feat/export".into(),
        size: 1,
        sha256: "00".into(),
        back: false,
    })
}

fn place(launch: &str) -> Git {
    Git::Place(GitPlace {
        launch: launch.into(),
        project: "app".into(),
        branch: "feat/export".into(),
        repositories: vec![".".into()],
    })
}

/// A hub, `pc` its first member and `server` paired with it as a runner, the two linked, each
/// with the project `app`; `pc`'s repository on `feat/export` with two commits and an ignore
/// rule.
fn pc_and_server() -> (TestHub, Machine, Machine) {
    pc_and_server_with(Machine::new())
}

fn pc_and_server_with(server: Machine) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    pair(&hub, &server, "server", "runner");
    pc.wait_for("server");
    server.wait_for("pc");
    pc.register();
    server.register();
    let repo = pc.folder();
    pc.user.git_init(&repo);
    std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    std::fs::write(repo.join("gone.txt"), "gone\n").unwrap();
    pc.git(&repo, &["add", "."]);
    pc.git(&repo, &["commit", "--quiet", "-m", "first"]);
    pc.git(&repo, &["checkout", "--quiet", "-b", "feat/export"]);
    commit(&pc, &repo, "b.txt", "b\n");
    (hub, pc, server)
}

fn commit(machine: &Machine, repo: &Path, file: &str, text: &str) -> String {
    std::fs::write(repo.join(file), text).unwrap();
    machine.git(repo, &["add", "--", file]);
    machine.git(repo, &["commit", "--quiet", "-m", file]);
    machine.git(repo, &["rev-parse", "HEAD"])
}

/// Uncommitted work in `pc`'s repository: a change, a removal, a new file, an ignored one.
fn uncommitted(pc: &Machine) {
    let repo = pc.folder();
    std::fs::write(repo.join("a.txt"), "a, changed\n").unwrap();
    std::fs::remove_file(repo.join("gone.txt")).unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    std::fs::write(repo.join(".env"), "TOKEN=invented\n").unwrap();
}

/// The folder of the workspace a travel made on the other machine.
fn folder_of(sent: &Value) -> PathBuf {
    PathBuf::from(sent["folder"].as_str().unwrap_or_else(|| panic!("{sent}")))
}

fn repository<'a>(sent: &'a Value, path: &str) -> &'a Value {
    sent["repositories"]
        .as_array()
        .unwrap_or_else(|| panic!("{sent}"))
        .iter()
        .find(|repository| repository["path"] == path)
        .unwrap_or_else(|| panic!("no {path} in {sent}"))
}

fn status(machine: &Machine, dir: &Path) -> Vec<String> {
    let mut lines: Vec<String> = machine
        .user
        .git_output(dir, &["status", "--porcelain", "--untracked-files=all"])
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

#[test]
fn uncommitted_and_untracked_files_travel_and_ignored_files_do_not() {
    let (_hub, pc, server) = pc_and_server();
    uncommitted(&pc);

    let sent = pc.travel("server", "l1", json!({})).unwrap();

    let there = folder_of(&sent);
    assert_eq!(
        std::fs::read_to_string(there.join("a.txt")).unwrap(),
        "a, changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("new.txt")).unwrap(),
        "new\n"
    );
    assert_eq!(std::fs::read_to_string(there.join("b.txt")).unwrap(), "b\n");
    assert!(!there.join("gone.txt").exists());
    assert!(!there.join(".env").exists(), "an ignored file travelled");
    // Nowhere in what the server received.
    let objects = server.git(&server.folder(), &["rev-list", "--all", "--objects"]);
    assert!(!objects.contains(".env"), "{objects}");
}

#[test]
fn the_user_s_branch_index_and_files_are_untouched_by_the_snapshot() {
    let (_hub, pc, _server) = pc_and_server();
    let repo = pc.folder();
    uncommitted(&pc);
    // Something staged too: the index the user built stays as it is.
    std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
    pc.git(&repo, &["add", "staged.txt"]);
    let state = |pc: &Machine| {
        vec![
            format!("{:?}", std::fs::read(repo.join(".git/index")).unwrap()),
            format!("{:?}", status(pc, &repo)),
            pc.git(&repo, &["rev-parse", "HEAD"]),
            pc.git(&repo, &["symbolic-ref", "HEAD"]),
            pc.git(&repo, &["for-each-ref", "refs/heads", "refs/stash"]),
            pc.git(&repo, &["reflog", "show", "HEAD"]),
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            std::fs::read_to_string(repo.join("new.txt")).unwrap(),
            std::fs::read_to_string(repo.join(".env")).unwrap(),
        ]
    };
    let before = state(&pc);

    pc.travel("server", "l1", json!({})).unwrap();

    assert_eq!(state(&pc), before);
}

#[test]
fn only_the_commits_the_other_machine_lacks_are_sent() {
    let (_hub, pc, server) = pc_and_server();
    let repo = pc.folder();
    let first = pc.travel("server", "l1", json!({})).unwrap();
    // The whole branch the first time: two commits and the snapshot.
    assert_eq!(repository(&first, ".")["commits"], 3, "{first}");
    let tip = pc.git(&repo, &["rev-parse", "HEAD"]);

    commit(&pc, &repo, "c.txt", "c\n");
    let second = pc.travel("server", "l2", json!({})).unwrap();

    // The server has the first tip, on the branch of its workspace: only the new commit and the
    // snapshot go, with that tip as the bundle's prerequisite.
    let sent = repository(&second, ".");
    assert_eq!(sent["commits"], 2, "{second}");
    assert_eq!(sent["prerequisites"], json!([tip]), "{second}");
    let there = folder_of(&second);
    assert_eq!(std::fs::read_to_string(there.join("c.txt")).unwrap(), "c\n");
    assert_eq!(
        server.git(&there, &["rev-parse", "HEAD"]),
        pc.git(&repo, &["rev-parse", "HEAD"])
    );
}

#[test]
fn a_repository_with_no_remote_travels_whole_the_first_time() {
    let (_hub, pc, server) = pc_and_server();
    let repo = pc.folder();
    assert_eq!(pc.git(&repo, &["remote"]), "");
    assert!(!server.folder().exists());

    let sent = pc.travel("server", "l1", json!({})).unwrap();

    let sent = repository(&sent, ".");
    assert_eq!(sent["prerequisites"], json!([]));
    // Cloned from the bundle: the whole history of the branch, and no remote.
    let there = server.folder();
    assert_eq!(server.git(&there, &["remote"]), "");
    assert_eq!(
        server.git(&there, &["rev-list", "--count", "feat/export"]),
        pc.git(&repo, &["rev-list", "--count", "HEAD"])
    );
}

#[test]
fn nothing_is_pushed_and_no_remote_is_contacted() {
    let (_hub, pc, server) = pc_and_server();
    let repo = pc.folder();
    // A remote of the user's own, pushed to once: it must not change.
    let bare = pc.user.path("remotes/app.git");
    pc.git(
        &pc.user.path(""),
        &["init", "--quiet", "--bare", bare.to_str().unwrap()],
    );
    pc.git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    pc.git(&repo, &["push", "--quiet", "origin", "main"]);
    // And one that leaves a mark when git contacts it.
    let mark = pc.user.path("contacted");
    let url = format!("ext::sh -c touch% {}", mark.display());
    pc.git(&repo, &["remote", "add", "other", &url]);
    let pushed = pc.git(&bare, &["for-each-ref"]);
    let tracking = pc.git(&repo, &["for-each-ref", "refs/remotes"]);
    uncommitted(&pc);

    let sent = pc.travel("server", "l1", json!({})).unwrap();
    let there = folder_of(&sent);
    server.git(&there, &["add", "--all"]);
    server.git(&there, &["commit", "--quiet", "-m", "work"]);
    server.travel_back("l1").unwrap();

    assert!(!mark.exists(), "a remote was contacted");
    assert_eq!(
        pc.git(&bare, &["for-each-ref"]),
        pushed,
        "something was pushed"
    );
    assert_eq!(pc.git(&repo, &["for-each-ref", "refs/remotes"]), tracking);
    assert_eq!(server.git(&server.folder(), &["remote"]), "");
}

#[test]
fn the_workspace_on_the_other_machine_has_the_uncommitted_changes_uncommitted() {
    let (_hub, pc, server) = pc_and_server();
    uncommitted(&pc);
    let repo = pc.folder();

    let sent = pc.travel("server", "l1", json!({})).unwrap();

    let there = folder_of(&sent);
    assert_eq!(sent["branch"], "feat/export");
    assert_eq!(
        server.git(&there, &["symbolic-ref", "--short", "HEAD"]),
        "feat/export"
    );
    // On the sender's commit, the work on top of it uncommitted and unstaged, the new file
    // untracked.
    assert_eq!(
        server.git(&there, &["rev-parse", "HEAD"]),
        pc.git(&repo, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        status(&server, &there),
        vec![" D gone.txt", " M a.txt", "?? new.txt"]
    );
    // A workspace of the project's there, as any.
    let shown = server
        .user
        .call(
            "workspace.show",
            json!({"project": "app", "name": sent["workspace"]}),
        )
        .unwrap();
    assert_eq!(shown["branch"], "feat/export", "{shown}");
}

#[test]
fn a_branch_name_taken_there_gets_a_suffix() {
    let (_hub, pc, server) = pc_and_server();
    let first = pc.travel("server", "l1", json!({})).unwrap();
    assert_eq!(first["branch"], "feat/export");

    let second = pc.travel("server", "l2", json!({})).unwrap();
    let third = pc.travel("server", "l3", json!({})).unwrap();

    assert_eq!(second["branch"], "feat/export-2");
    assert_eq!(third["branch"], "feat/export-3");
    assert_eq!(
        server.git(&folder_of(&third), &["symbolic-ref", "--short", "HEAD"]),
        "feat/export-3"
    );
    assert_ne!(first["workspace"], second["workspace"]);
}

#[test]
fn a_corrupt_bundle_is_refused_and_fetches_nothing() {
    let (_hub, pc, server) = pc_and_server();
    pc.travel("server", "l1", json!({})).unwrap();
    let there = server.folder();
    let refs = server.git(&there, &["for-each-ref"]);
    let workspaces = server.user.call("workspace.list", json!({})).unwrap();
    commit(&pc, &pc.folder(), "c.txt", "c\n");

    let (_, refused) = pc
        .travel("server", "l2", json!({"corrupt": true}))
        .unwrap_err();

    // Refused by git, at its verification or its fetch: either way, nothing is fetched.
    assert!(
        refused.contains("the bundle is refused") || refused.contains("could not be fetched"),
        "{refused}"
    );
    assert!(
        !server
            .git(&there, &["for-each-ref"])
            .contains("incoming/l2")
    );
    assert_eq!(server.git(&there, &["for-each-ref"]), refs);
    assert_eq!(
        server.user.call("workspace.list", json!({})).unwrap(),
        workspaces
    );
}

#[test]
fn only_the_new_commits_come_back() {
    let (_hub, pc, server) = pc_and_server();
    uncommitted(&pc);
    let sent = pc.travel("server", "l1", json!({})).unwrap();
    let there = folder_of(&sent);
    server.git(&there, &["add", "--all"]);
    server.git(&there, &["commit", "--quiet", "-m", "the work"]);
    commit(&server, &there, "more.txt", "more\n");

    let back = server.travel_back("l1").unwrap();

    // The two commits made there, not the history it was sent.
    assert_eq!(repository(&back, ".")["commits"], 2, "{back}");
    commit(&server, &there, "again.txt", "again\n");
    let again = server.travel_back("l1").unwrap();
    assert_eq!(repository(&again, ".")["commits"], 1, "{again}");
    assert_eq!(
        pc.git(
            &pc.folder(),
            &["rev-parse", "refs/flottille/machines/server/l1/feat/export"]
        ),
        server.git(&there, &["rev-parse", "HEAD"])
    );
}

#[test]
fn returned_work_lands_in_the_machines_ref_never_on_the_user_s_branch() {
    let (_hub, pc, server) = pc_and_server();
    let repo = pc.folder();
    uncommitted(&pc);
    let sent = pc.travel("server", "l1", json!({})).unwrap();
    let there = folder_of(&sent);
    server.git(&there, &["add", "--all"]);
    server.git(&there, &["commit", "--quiet", "-m", "the work"]);
    let branches = pc.git(&repo, &["for-each-ref", "refs/heads"]);
    let head = pc.git(&repo, &["rev-parse", "HEAD"]);
    let files = status(&pc, &repo);

    let back = server.travel_back("l1").unwrap();

    let landed = repository(&back, ".")["ref"].clone();
    assert_eq!(landed, "refs/flottille/machines/server/l1/feat/export");
    assert_eq!(
        pc.git(&repo, &["rev-parse", landed.as_str().unwrap()]),
        server.git(&there, &["rev-parse", "HEAD"])
    );
    assert_eq!(pc.git(&repo, &["for-each-ref", "refs/heads"]), branches);
    assert_eq!(pc.git(&repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(status(&pc, &repo), files);
}

#[test]
fn a_machine_the_launch_was_not_sent_to_brings_nothing_back_and_reads_no_bundle() {
    let (hub, pc, server) = pc_and_server();
    // Another machine of the user's, a runner too: l1 is offered to the server, not to it.
    let laptop = Machine::new();
    pair(&hub, &laptop, "laptop", "runner");
    pc.wait_for("laptop");
    server.wait_for("pc");
    let repo = pc.folder();
    let refs = pc.git(&repo, &["for-each-ref"]);

    std::thread::scope(|scope| {
        // The offer held open a while, as a big bundle's is while the server reads it.
        let travel = scope.spawn(|| pc.travel("server", "l1", json!({"hold_ms": 4000})));
        let deadline = Instant::now() + WAIT;
        while !pc.log().contains("offers . of launch l1 to server") {
            assert!(Instant::now() < deadline, "no offer was held\n{}", pc.log());
            std::thread::sleep(Duration::from_millis(50));
        }
        let key = laptop.stand_in();
        let pc_key = pc.key();
        let (fetched, back) = runtime().block_on(async {
            let link = dial(&key, &pc, pc_key).await;
            let mut answers = link
                .ask(&Message::Machine(MachineMessage::Git(Git::Fetch(
                    GitFetch {
                        transfer: "l1/0".into(),
                    },
                ))))
                .await
                .unwrap();
            let fetched = answers.next().await;
            let back = link
                .send(&Message::Machine(MachineMessage::Git(Git::Offer(
                    GitOffer {
                        transfer: "l1/back/0".into(),
                        launch: "l1".into(),
                        project: "app".into(),
                        repository: ".".into(),
                        branch: "feat/export".into(),
                        size: 1,
                        sha256: "00".into(),
                        back: true,
                    },
                ))))
                .await;
            (fetched, back)
        });
        let fetched = fetched.unwrap().unwrap_err().to_string();
        assert!(fetched.contains("no bundle is offered"), "{fetched}");
        let refused = back.unwrap_err().to_string();
        assert!(refused.contains("was not sent to laptop"), "{refused}");
        // The server, which it is offered to, read it all the same.
        travel.join().unwrap().unwrap();
    });
    assert_eq!(pc.git(&repo, &["for-each-ref"]), refs);
}

#[test]
fn a_viewer_s_machine_can_neither_offer_nor_place_code() {
    let (hub, _pc, server) = pc_and_server();
    let tablet = Machine::new();
    pair(&hub, &tablet, "tablet", "viewer");
    server.wait_for("tablet");
    let workspaces = server.user.call("workspace.list", json!({})).unwrap();

    // A machine of no member is no longer tried: the hub carries nothing of it.
    {
        let (key, name) = (tablet.stand_in(), "tablet");
        // A project the server does not have: what refuses it is who sends it, before the
        // project is looked for.
        let mut offered = offer("l1");
        if let Git::Offer(offer) = &mut offered {
            offer.project = "nowhere".into();
        }
        let refused = sent_with(&key, &server, offered).unwrap_err();
        assert!(
            refused.contains("only a runner's or a controller's machine sends code"),
            "{name}: {refused}"
        );
        let refused = sent_with(&key, &server, place("l1")).unwrap_err();
        assert!(
            refused.contains("only a runner's or a controller's machine sends code"),
            "{name}: {refused}"
        );
    }
    assert_eq!(
        server.user.call("workspace.list", json!({})).unwrap(),
        workspaces
    );
}

#[test]
fn the_receiver_takes_no_code_in_a_build_without_the_travel_gate() {
    // The server's daemon runs without `FLOTTILLE_TEST_TRAVEL`, as a release build is.
    let (_hub, pc, server) = pc_and_server_with(Machine::of(User::new()));
    let workspaces = server.user.call("workspace.list", json!({})).unwrap();

    let (_, refused) = pc.travel("server", "l1", json!({})).unwrap_err();

    assert!(refused.contains("takes no code"), "{refused}");
    assert!(!server.folder().exists());
    assert_eq!(
        server.user.call("workspace.list", json!({})).unwrap(),
        workspaces
    );
}

#[test]
fn a_workspace_made_of_received_code_runs_no_setup() {
    let (_hub, pc, server) = pc_and_server();
    let mark = server.user.path("setup-ran");
    let script = server.user.path("setup.sh");
    std::fs::write(&script, format!("touch {}\n", mark.display())).unwrap();
    server.register_with(
        &["."],
        &format!("[[setup]]\nrun = \"sh {}\"\n", script.display()),
    );

    let sent = pc.travel("server", "l1", json!({})).unwrap();

    let deadline = Instant::now() + WAIT;
    let shown = loop {
        let shown = server
            .user
            .call(
                "workspace.show",
                json!({"project": "app", "name": sent["workspace"]}),
            )
            .unwrap();
        if shown["state"] != "preparing" {
            break shown;
        }
        assert!(Instant::now() < deadline, "still preparing: {shown}");
        std::thread::sleep(Duration::from_millis(50));
    };
    let kinds: Vec<&str> = shown["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["worktree"], "{shown}");
    assert!(!mark.exists(), "the setup ran");
}

#[test]
fn the_refs_of_a_travel_are_removed_once_it_ends() {
    let (_hub, pc, server) = pc_and_server();
    let repo = pc.folder();
    uncommitted(&pc);

    let sent = pc.travel("server", "l1", json!({})).unwrap();

    assert_eq!(pc.refs(&repo, "outgoing"), "");
    assert_eq!(server.refs(&server.folder(), "incoming"), "");
    // The workspace has the work all the same.
    let there = folder_of(&sent);
    assert_eq!(
        std::fs::read_to_string(there.join("new.txt")).unwrap(),
        "new\n"
    );

    // And a travel that failed leaves none either.
    commit(&pc, &repo, "c.txt", "c\n");
    pc.travel("server", "l2", json!({"corrupt": true}))
        .unwrap_err();
    assert_eq!(pc.refs(&repo, "outgoing"), "");
    assert_eq!(server.refs(&server.folder(), "incoming"), "");
}

#[test]
fn two_launches_from_one_branch_come_back_to_refs_of_their_own() {
    let (_hub, pc, server) = pc_and_server();
    let first = pc.travel("server", "l1", json!({})).unwrap();
    let second = pc.travel("server", "l2", json!({})).unwrap();
    let one = commit(&server, &folder_of(&first), "one.txt", "one\n");
    let two = commit(&server, &folder_of(&second), "two.txt", "two\n");

    let back_one = server.travel_back("l1").unwrap();
    let back_two = server.travel_back("l2").unwrap();

    assert_eq!(
        repository(&back_one, ".")["ref"],
        "refs/flottille/machines/server/l1/feat/export"
    );
    assert_eq!(
        repository(&back_two, ".")["ref"],
        "refs/flottille/machines/server/l2/feat/export"
    );
    let repo = pc.folder();
    let at = |reference: &str| pc.git(&repo, &["rev-parse", reference]);
    assert_eq!(at("refs/flottille/machines/server/l1/feat/export"), one);
    assert_eq!(at("refs/flottille/machines/server/l2/feat/export"), two);
}

#[test]
fn a_launch_whose_sending_failed_part_way_is_sent_again() {
    let (_hub, pc, server) = pc_and_server();
    // Two repositories; the server has only the first in its project for now.
    pc.register_with(&["api", "web"], "");
    server.register_with(&["api"], "");
    for path in ["api", "web"] {
        let repo = pc.folder().join(path);
        pc.user.git_init(&repo);
        pc.git(&repo, &["checkout", "--quiet", "-b", "feat/export"]);
        commit(&pc, &repo, "x.txt", &format!("{path}\n"));
    }

    let (_, refused) = pc.travel("server", "l1", json!({})).unwrap_err();
    assert!(refused.contains("no repository web"), "{refused}");
    server.register_with(&["api", "web"], "");
    let sent = pc.travel("server", "l1", json!({})).unwrap();

    let there = folder_of(&sent);
    for path in ["api", "web"] {
        assert_eq!(
            std::fs::read_to_string(there.join(path).join("x.txt")).unwrap(),
            format!("{path}\n")
        );
    }
}

#[test]
fn a_place_whose_worktree_fails_fails_and_is_sent_again() {
    let (_hub, pc, server) = pc_and_server();
    pc.travel("server", "l0", json!({})).unwrap();
    // The next launch's branch is being made there meanwhile: git holds the lock of its name.
    let lock = server.folder().join(".git/refs/heads/feat/export-2.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    std::fs::write(&lock, "").unwrap();

    let (_, failed) = pc.travel("server", "l1", json!({})).unwrap_err();

    assert!(failed.contains("feat/export-2"), "{failed}");
    // Nothing is kept as placed: once the lock is gone, the launch is sent again.
    std::fs::remove_file(&lock).unwrap();
    let sent = pc.travel("server", "l1", json!({})).unwrap();
    assert_eq!(
        std::fs::read_to_string(folder_of(&sent).join("b.txt")).unwrap(),
        "b\n"
    );
}
