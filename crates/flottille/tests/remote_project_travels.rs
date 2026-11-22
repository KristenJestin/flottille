//! A project travels with a launch to another machine (`docs/remote.md`, "Which project", "What
//! travels"): the receiving machine recognises the project by its repositories, whatever its name
//! or folder, or makes it from the code that came, never from a remote; the caller's settings
//! travel with each launch to a project Flottille made, never over one the user registered. A hub
//! served in the test on 127.0.0.1 and daemons, each with its own temporary HOME and XDG folders,
//! in-memory keyring and fake agents; throwaway repositories.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::remote_launch::{
    Machine, allowing, done, eventually, folder_of, launched, pair, pc_and_server, prompted,
    runner_user, server_user, sign_in, titled,
};

/// Waits until `pc` sees the server offer Claude, signed in.
fn signed_in_there(pc: &Machine) {
    eventually("server's agents", || {
        pc.seen("server")["offers"]["agents"]
            .as_array()
            .is_some_and(|agents| agents.iter().any(|agent| agent == "claude"))
    });
}

/// The projects of `machine`, by name.
fn projects(machine: &Machine) -> Vec<Value> {
    machine.user.call("project.list", json!({})).unwrap()["projects"]
        .as_array()
        .unwrap()
        .clone()
}

fn project(machine: &Machine, name: &str) -> Value {
    machine
        .user
        .call("project.show", json!({"name": name}))
        .unwrap()
}

fn names(machine: &Machine) -> Vec<String> {
    projects(machine)
        .iter()
        .map(|project| project["name"].as_str().unwrap().to_owned())
        .collect()
}

/// The project `name` of `machine` registered by hand, its one repository at `folder`, with
/// `more` lines.
fn register_as(machine: &Machine, name: &str, folder: &Path, more: &str) {
    let text = format!(
        "folder = \"{}\"\nworkspaces_root = \"{}\"\nbranch_prefix = \"\"\n{more}\n\
         [[repositories]]\npath = \".\"\nbase = [\"main\"]\n",
        folder.display(),
        machine.user.path(&format!("work/{name}-work")).display()
    );
    machine.user.write_project_file(name, &text);
    machine.user.call("project.list", json!({})).unwrap();
}

/// A led session started on the server by a root of `pc` in Auto, the server's judge allowing
/// it: the session there.
fn led_there(pc: &Machine, server: &Machine, title: &str) -> Value {
    let (_root, mcp) = pc.root("auto");
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": title,
            "brief": format!("{title}, whole.")}),
    ));
    titled(server, title)
}

fn there_folder(server: &Machine, name: &str) -> PathBuf {
    server.user.path(&format!("data/flottille/projects/{name}"))
}

#[test]
fn a_launch_to_a_machine_without_the_project_makes_it_there_from_the_bundle() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    signed_in_there(&pc);
    assert!(names(&server).is_empty());

    let session = led_there(&pc, &server, "Finish the export");

    // The project made in the folder for projects from other machines, marked as made from pc.
    let made = project(&server, "app");
    let folder = there_folder(&server, "app");
    assert_eq!(made["folder"], folder.to_str().unwrap(), "{made:#}");
    assert_eq!(made["from_machine"], "pc", "{made:#}");
    assert_eq!(session["project"], "app");
    // Its repository cloned from the bundle: pc's commits are there.
    let log = server.git(&folder, &["log", "--format=%s", "--all"]);
    assert!(log.contains("first") && log.contains("b"), "{log}");
    // The session's workspace on the caller's branch, with its uncommitted work.
    let there = folder_of(&session);
    assert_eq!(
        server.git(&there, &["branch", "--show-current"]),
        "feat/export"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("new.txt")).unwrap(),
        "new\n"
    );
    prompted(&server, "Finish the export, whole.");
    // The project is on the server's side of the list as pc sees it.
    eventually("app offered by server", || {
        pc.seen("server")["offers"]["projects"]
            .as_array()
            .is_some_and(|projects| projects.iter().any(|project| project["name"] == "app"))
    });
}

#[test]
fn the_project_is_recognised_by_its_repositories_whatever_its_name() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    // The server has pc's repository as `ledger`, in a folder of its own, and an `app` that is
    // another repository altogether.
    let ledger = server.user.path("elsewhere/ledger");
    server.git(
        &server.user.path(""),
        &[
            "clone",
            "--quiet",
            pc.folder().to_str().unwrap(),
            ledger.to_str().unwrap(),
        ],
    );
    register_as(&server, "ledger", &ledger, "");
    let other = server.user.path("elsewhere/app");
    server.user.git_init(&other);
    std::fs::write(other.join("x.txt"), "x\n").unwrap();
    server.git(&other, &["add", "."]);
    server.git(&other, &["commit", "--quiet", "-m", "unrelated"]);
    register_as(&server, "app", &other, "");
    signed_in_there(&pc);

    let session = led_there(&pc, &server, "Ledger work");

    // In `ledger`: no project made, and `app`, another repository, left alone.
    assert_eq!(session["project"], "ledger", "{session:#}");
    let mut listed = names(&server);
    listed.sort();
    assert_eq!(listed, ["app", "ledger"]);
    let there = folder_of(&session);
    assert!(
        there.starts_with(server.user.path("work/ledger-work")),
        "{there:?}"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("new.txt")).unwrap(),
        "new\n"
    );
    // The launch's commits came into ledger's repository, never into the other.
    assert_eq!(
        server.git(&other, &["log", "--format=%s", "--all"]),
        "unrelated"
    );
}

#[test]
fn the_project_settings_travel_and_are_updated_at_each_launch() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    pc.register("rules = \"Small commits.\"\n[tasks.hello]\nline = \"echo hi\"\n");
    signed_in_there(&pc);
    // Rules are the user's to set, as `set_rules` is: the user allows each launch that sets them.
    let first = held_there(&pc, &server, "auto", "First");
    approved(&server, &first);
    eventually("the first settings laid", || {
        project(&server, "app")["rules"] == "Small commits."
    });
    let made = project(&server, "app");
    assert_eq!(made["tasks"]["hello"]["line"], "echo hi", "{made:#}");
    // The server's own setting of it, which no launch changes.
    let file = server.user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("max_children = 12\n"), "{text}");
    std::fs::write(
        &file,
        text.replace("max_children = 12\n", "max_children = 3\n"),
    )
    .unwrap();

    pc.register("rules = \"Tiny commits.\"\n");
    let second = held_there(&pc, &server, "auto", "Second");
    approved(&server, &second);
    eventually("the second settings laid", || {
        project(&server, "app")["rules"] == "Tiny commits."
    });

    let updated = project(&server, "app");
    assert!(updated["tasks"].get("hello").is_none(), "{updated:#}");
    assert_eq!(updated["max_children"], 3, "{updated:#}");
    assert_eq!(updated["folder"], made["folder"]);
    assert_eq!(names(&server), ["app"]);
}

/// The user of `server` allows `request`.
fn approved(server: &Machine, request: &Value) {
    server
        .user
        .call(
            "approval.approve",
            json!({"number": request["number"].as_u64().unwrap()}),
        )
        .unwrap();
}

#[test]
fn a_project_registered_by_hand_keeps_its_own_settings() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some("rules = \"Ours.\""));
    pc.register("rules = \"Theirs.\"\n[tasks.hello]\nline = \"echo hi\"\n");
    signed_in_there(&pc);

    led_there(&pc, &server, "Kept");

    let kept = project(&server, "app");
    assert_eq!(kept["rules"], "Ours.", "{kept:#}");
    assert!(
        kept.get("tasks")
            .is_none_or(|tasks| tasks.get("hello").is_none())
    );
    assert!(kept.get("from_machine").is_none(), "{kept:#}");
}

#[test]
fn nothing_is_cloned_from_a_remote_and_no_file_is_copied() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    // A remote that leads nowhere: anything fetched from it would fail.
    pc.git(
        &pc.folder(),
        &[
            "remote",
            "add",
            "origin",
            "https://forge.invalid/acme/app.git",
        ],
    );
    // An ignored file of the caller's: it stays there.
    std::fs::write(pc.folder().join(".env"), "TOKEN=kept-here\n").unwrap();
    signed_in_there(&pc);

    let session = led_there(&pc, &server, "No remote");

    let folder = there_folder(&server, "app");
    assert_eq!(server.git(&folder, &["remote"]), "", "a remote was set");
    assert!(!folder.join(".env").exists(), "an ignored file travelled");
    assert!(!folder_of(&session).join(".env").exists());
    assert!(
        !server.log().contains("forge.invalid"),
        "the remote was reached: {}",
        server.log()
    );
}

/// The launch `pc`'s root in `mode` sends to the server, held there for the user: the request.
fn held_there(pc: &Machine, server: &Machine, mode: &str, title: &str) -> Value {
    let before = server.requests().len();
    let (_root, mcp) = pc.root(mode);
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": title,
            "brief": format!("{title}, whole.")}),
    ));
    let requests = server.requests();
    assert_eq!(
        requests.len(),
        before + 1,
        "{requests:#?}\n{}",
        server.log()
    );
    requests
        .into_iter()
        .max_by_key(|request| request["number"].as_u64())
        .unwrap()
}

#[test]
fn a_travelled_sudo_task_makes_the_launch_the_user_s_to_decide() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    pc.register("[tasks.install]\nline = \"sudo make install\"\n");
    signed_in_there(&pc);

    // In Auto, the server's judge would allow it: a line that would be base to run is the user's.
    let request = held_there(&pc, &server, "auto", "Install");

    assert_eq!(request["class"], "base", "{request:#}");
    let card = request.to_string();
    assert!(card.contains("sudo make install"), "{card}");
    assert!(card.contains("the line it sets"), "{card}");
    assert!(
        server.user.judge_received().is_empty(),
        "the judge was asked of a base launch"
    );
    // Nothing of the task is laid while the user has not answered.
    let made = project(&server, "app");
    assert!(
        made.get("tasks")
            .is_none_or(|tasks| tasks.get("install").is_none()),
        "{made:#}"
    );
}

#[test]
fn every_travelled_line_is_named_in_the_launch() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    pc.register(
        "rules = \"Small commits.\"\nnever = [[\"rm\", \"-rf\"]]\n\
         message_checks = [\"check-message {message}\"]\n\
         [variables]\nTOKEN = \"/opt/kept-here\"\n\
         [tasks.deploy]\nline = \"make deploy\"\nask_before_running = true\n\
         [[setup]]\nrun = \"make deps\"\n",
    );
    signed_in_there(&pc);

    let first = held_there(&pc, &server, "ask", "First");

    let card = first.to_string();
    for named in [
        "make deploy",
        "make deps",
        "check-message {message}",
        "Small commits.",
        "rm -rf",
    ] {
        assert!(card.contains(named), "{named} is not named: {card}");
    }
    // The variables stay the caller's: they never travel.
    assert!(!card.contains("/opt/kept-here"), "{card}");
    approved(&server, &first);
    eventually("the first launch's settings laid", || {
        project(&server, "app")["rules"] == "Small commits."
    });
    let made = project(&server, "app");
    assert!(
        made.get("variables")
            .is_none_or(|variables| variables.get("TOKEN").is_none()),
        "{made:#}"
    );

    // The caller turns asking off, empties its never list and keeps its rules alone.
    pc.register(
        "rules = \"Small commits.\"\nrules_alone = true\nnever = []\n\
         message_checks = [\"check-message {message}\"]\n\
         [tasks.deploy]\nline = \"make deploy\"\n\
         [[setup]]\nrun = \"make deps\"\n",
    );
    let second = held_there(&pc, &server, "ask", "Second");

    assert_eq!(second["class"], "base", "{second:#}");
    let card = second.to_string();
    for named in [
        "It turns off asking before the task deploy runs",
        "It empties the never list",
        "rules alone",
    ] {
        assert!(card.contains(named), "{named} is not said: {card}");
    }
}

#[test]
fn a_refused_launch_leaves_no_project() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    signed_in_there(&pc);
    let request = held_there(&pc, &server, "ask", "Refused");
    assert_eq!(names(&server), ["app"]);

    server
        .user
        .call(
            "approval.deny",
            json!({"number": request["number"].as_u64().unwrap()}),
        )
        .unwrap();

    eventually("the project made for it removed", || {
        names(&server).is_empty()
    });
    eventually("its folder removed", || {
        !there_folder(&server, "app").exists()
    });
}

/// A third machine, `laptop`, paired as a runner with `pc`'s hub and linked to `server`, with
/// `pc`'s repository cloned where its project `app` is to be.
fn laptop(hub: &crate::remote_launch::TestHub, pc: &Machine, server: &Machine) -> Machine {
    let laptop_user = runner_user();
    sign_in(&laptop_user);
    let laptop = Machine::of(laptop_user);
    pair(hub, &laptop, "laptop", "runner");
    laptop.wait_for("server");
    server.wait_for("laptop");
    laptop.git(
        &laptop.user.path(""),
        &[
            "clone",
            "--quiet",
            pc.folder().to_str().unwrap(),
            laptop.folder().to_str().unwrap(),
        ],
    );
    laptop
}

#[test]
fn another_machine_never_takes_over_a_project_made_for_one() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), None);
    pc.register("rules = \"Pc's.\"\n");
    signed_in_there(&pc);
    let from_pc = held_there(&pc, &server, "auto", "From pc");
    approved(&server, &from_pc);
    eventually("pc's settings laid", || {
        project(&server, "app")["rules"] == "Pc's."
    });

    // A third machine with the same repository, its project of its own settings.
    let laptop = laptop(&hub, &pc, &server);
    laptop.register("rules = \"Laptop's.\"\n[tasks.hello]\nline = \"echo hi\"\n");
    signed_in_there(&laptop);

    let session = led_there(&laptop, &server, "From laptop");

    // The launch runs in pc's project, which keeps pc's settings, and says so.
    assert_eq!(session["project"], "app", "{session:#}");
    let kept = project(&server, "app");
    assert_eq!(kept["rules"], "Pc's.", "{kept:#}");
    assert_eq!(kept["from_machine"], "pc", "{kept:#}");
    assert!(
        kept.get("tasks")
            .is_none_or(|tasks| tasks.get("hello").is_none()),
        "{kept:#}"
    );
    let judged = server
        .user
        .judge_received()
        .iter()
        .map(Value::to_string)
        .collect::<String>();
    assert!(
        judged.contains("it keeps pc's settings, not laptop's"),
        "{judged}"
    );
}

#[test]
fn two_launches_of_a_new_project_at_once_both_start() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), None);
    // Two machines, each sending one thing at a time: their launches meet in the making.
    let laptop = laptop(&hub, &pc, &server);
    laptop.register("");
    signed_in_there(&pc);
    signed_in_there(&laptop);
    let (_first, one) = pc.root("auto");
    let (_second, two) = laptop.root("auto");

    let answers: Vec<_> = [(one, "One"), (two, "Two")]
        .into_iter()
        .map(|(mcp, title)| {
            std::thread::spawn(move || {
                launched(
                    &mcp,
                    json!({"kind": "led", "machine": "server", "title": title,
                        "brief": format!("{title}, whole.")}),
                )
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();

    for answer in &answers {
        done(answer);
    }
    assert_eq!(titled(&server, "One")["project"], "app");
    assert_eq!(titled(&server, "Two")["project"], "app");
    assert_eq!(names(&server), ["app"]);
}
