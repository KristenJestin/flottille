//! A place's code travels to another machine and its new commits come back (`docs/remote.md`,
//! "What travels"): two daemons, each with its own temporary HOME and XDG folders and in-memory
//! keyring, linked over 127.0.0.1 with no hub (the test makes them trust each other's key in the
//! file the hub will keep), and throwaway repositories. `agent_launch` is not built yet: the test
//! build's `test.travel` and `test.travel_back` start what a launch will.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// How long a link may take to come up: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(30);
/// How long a travel may take: git runs a few times on each side.
const TRAVEL: Duration = Duration::from_secs(60);

struct Machine {
    user: User,
}

impl Machine {
    fn new() -> Machine {
        let user = User::new().with_env("FLOTTILLE_TEST_TRAVEL", "1");
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

    fn addresses(&self) -> Vec<String> {
        self.this()["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|address| address["address"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Stands in for the hub: this machine trusts `other`, named `name`.
    fn trust(&self, name: &str, other: &Machine) {
        let file = self.user.path("state/flottille/remote/peers.json");
        let mut peers: Value = std::fs::read_to_string(&file)
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap_or_else(|_| json!({"machines": []}));
        peers["machines"].as_array_mut().unwrap().push(json!({
            "name": name,
            "key": other.this()["key"],
            "addresses": other.addresses(),
        }));
        std::fs::write(file, peers.to_string()).unwrap();
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
                "{name} is not online\n{}",
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
        let text = format!(
            "folder = \"{}\"\nworkspaces_root = \"{}\"\nbranch_prefix = \"\"\n\
             [[repositories]]\npath = \".\"\nbase = [\"main\"]\n",
            self.folder().display(),
            self.user.path("work/app-work").display()
        );
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
}

/// `pc` and `server`, linked, each with the project `app`; `pc`'s repository on `feat/export`
/// with two commits, an ignore rule and a script.
fn pc_and_server() -> (Machine, Machine) {
    let (pc, server) = (Machine::new(), Machine::new());
    pc.trust("server", &server);
    server.trust("pc", &pc);
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
    (pc, server)
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
    let (pc, server) = pc_and_server();
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
    let (pc, _server) = pc_and_server();
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
    // The snapshot is held by a ref of Flottille's own, on top of the branch.
    let held = pc.git(&repo, &["rev-parse", "refs/flottille/outgoing/l1^"]);
    assert_eq!(held, pc.git(&repo, &["rev-parse", "HEAD"]));
}

#[test]
fn only_the_commits_the_other_machine_lacks_are_sent() {
    let (pc, server) = pc_and_server();
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
    let (pc, server) = pc_and_server();
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
        server.git(
            &there,
            &["rev-list", "--count", "refs/flottille/incoming/l1^"]
        ),
        pc.git(&repo, &["rev-list", "--count", "HEAD"])
    );
}

#[test]
fn nothing_is_pushed_and_no_remote_is_contacted() {
    let (pc, server) = pc_and_server();
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
    let (pc, server) = pc_and_server();
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
    let (pc, server) = pc_and_server();
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
    let (pc, server) = pc_and_server();
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
    let (pc, server) = pc_and_server();
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
            &["rev-parse", "refs/flottille/machines/server/feat/export"]
        ),
        server.git(&there, &["rev-parse", "HEAD"])
    );
}

#[test]
fn returned_work_lands_in_the_machines_ref_never_on_the_user_s_branch() {
    let (pc, server) = pc_and_server();
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
    assert_eq!(landed, "refs/flottille/machines/server/feat/export");
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
    use flottille_link::{
        Endpoint, Git, GitFetch, GitOffer, Hello, MachineKey, MachineMessage, Message, PublicKey,
        Traffic,
    };
    let (pc, _server) = pc_and_server();
    let repo = pc.folder();
    pc.travel("server", "l1", json!({})).unwrap();
    let refs = pc.git(&repo, &["for-each-ref"]);
    // Another machine of the user's, which pc trusts: l1 was sent to the server, not to it.
    let folder = tempfile::tempdir().unwrap();
    let key = MachineKey::load_or_make(&folder.path().join("laptop")).unwrap();
    let file = pc.user.path("state/flottille/remote/peers.json");
    let mut peers: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    peers["machines"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "laptop", "key": key.public(), "addresses": []}));
    std::fs::write(&file, peers.to_string()).unwrap();
    let addresses: Vec<std::net::SocketAddr> = pc
        .addresses()
        .iter()
        .map(|address| address.parse().unwrap())
        .collect();
    let pc_key: PublicKey = serde_json::from_value(pc.this()["key"].clone()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let (back, fetched) = runtime.block_on(async {
        let laptop = Endpoint::dialler(&key).unwrap();
        let deadline = Instant::now() + WAIT;
        let link = loop {
            let hello = Hello::ours(env!("CARGO_PKG_VERSION"));
            match laptop
                .connect(pc_key, &addresses, Traffic::Machine, &hello, "pc")
                .await
            {
                Ok(link) => break link,
                Err(error) => assert!(Instant::now() < deadline, "{error}"),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
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
        let mut answers = link
            .ask(&Message::Machine(MachineMessage::Git(Git::Fetch(
                GitFetch {
                    transfer: "l1/0".into(),
                },
            ))))
            .await
            .unwrap();
        (back, answers.next().await)
    });

    let refused = back.unwrap_err().to_string();
    assert!(refused.contains("was not sent to laptop"), "{refused}");
    let fetched = fetched.unwrap().unwrap_err().to_string();
    assert!(fetched.contains("no bundle is offered"), "{fetched}");
    assert_eq!(pc.git(&repo, &["for-each-ref"]), refs);
}
