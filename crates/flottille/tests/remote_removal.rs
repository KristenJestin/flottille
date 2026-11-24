//! The workspace of a session or child started on another machine (`docs/remote.md`, "Coming
//! back"): its commits come back when it ends, and the agent that started it removes it as it
//! removes one of this machine, judged there by the same checks. A hub served in the test on
//! 127.0.0.1 and two daemons, each with its own temporary HOME and XDG folders and fake agents;
//! throwaway repositories.

use crate::common::mcp::Answer;
use crate::remote_deliveries::{answered, launch_in, off, on, release, script};
use crate::remote_launch::{
    Machine, allowing, done, eventually, folder_of, id_in, launched, pc_and_server, prompted,
    refused, sent_with, server_user,
};

use flottille_core::store::{LaunchFilter, Store};
use flottille_link::{Call, MachineMessage};
use serde_json::{Value, json};

/// A session led by a root of `pc` in Auto, started on `server` with `brief`: its id there, the
/// launch's name and the root's MCP session.
fn led_on_server(
    pc: &Machine,
    server: &Machine,
    brief: &str,
) -> (String, String, crate::common::mcp::Mcp) {
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export", "brief": brief}),
    );
    let text = done(&answer).to_owned();
    prompted(server, brief);
    (id_in(&text), launch_in(&text), mcp)
}

/// Everything in `dir`, committed as `message`.
fn commit_all(machine: &Machine, dir: &std::path::Path, file: &str, message: &str) {
    std::fs::write(dir.join(file), format!("{message}\n")).unwrap();
    machine.git(dir, &["add", "--all"]);
    machine.git(dir, &["commit", "--quiet", "-m", message]);
}

/// The subject of the last commit of `reference` in `pc`'s repository.
fn last_of(pc: &Machine, reference: &str) -> String {
    pc.git(&pc.folder(), &["log", "-1", "--format=%s", reference])
}

/// The first of `machine`'s prompts holding `text`, once there.
fn prompt_with(machine: &Machine, text: &str) -> String {
    prompted(machine, text);
    machine
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains(text))
        .unwrap()
}

/// The number of the request a held answer names.
fn held(answer: &Answer) -> u64 {
    let text = done(answer);
    let at = text
        .find("request #")
        .unwrap_or_else(|| panic!("not held: {text}"))
        + "request #".len();
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

/// The pending request of `machine`'s user numbered `number`.
fn request(machine: &Machine, number: u64) -> Value {
    machine
        .requests()
        .into_iter()
        .find(|request| request["number"] == number)
        .unwrap_or_else(|| panic!("no request #{number}: {:#?}", machine.requests()))
}

/// The session `session` of `server` ended by its user, once it has.
fn ended_there(server: &Machine, session: &str) {
    server
        .user
        .call("agent.stop", json!({"agent": session}))
        .unwrap();
    eventually("the session ended", || {
        server.agent(session)["state"] == "ended"
    });
}

#[test]
fn the_pilot_removes_the_workspace_of_its_led_session_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, launch, mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let there = folder_of(&agent);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    // Its work, and the caller's uncommitted work laid there, committed in its place there.
    commit_all(&server, &there, "export.txt", "the export");
    ended_there(&server, &session);

    let answer = mcp.call(
        "workspace",
        json!({"action": "remove", "name": workspace, "machine": "server"}),
    );

    // Held for the user of the machine it is on, as a removal is on one machine; its commits went
    // back first, so nothing would be lost.
    let number = held(&answer);
    let asked = request(&server, number);
    assert!(
        asked["says"].to_string().contains("Nothing would be lost"),
        "{asked:#}"
    );
    let reference = format!("refs/flottille/machines/server/{launch}/feat/export");
    assert_eq!(last_of(&pc, &reference), "the export");
    assert!(there.exists());
    server
        .user
        .call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the workspace removed there", || !there.exists());
    let approved = prompt_with(&pc, "was approved by the user");
    assert!(
        approved.contains(&format!("Request #{number}")),
        "{approved}"
    );
    assert!(approved.contains("is removed"), "{approved}");
}

#[test]
fn commits_made_after_the_last_report_come_back_when_the_session_ends() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "The export is reported."}}}
        ]}]),
    );
    let (session, launch, _mcp) = led_on_server(&pc, &server, "Finish the export.");
    let there = folder_of(&server.agent(&session));
    commit_all(&server, &there, "export.txt", "the export");
    release(&server);
    // Its pilot is woken by its report, the commits it made so far come back with it.
    prompted(&pc, "it reported");
    let reference = format!("refs/flottille/machines/server/{launch}/feat/export");
    assert_eq!(last_of(&pc, &reference), "the export");
    // Work committed after its report, which no report brings back.
    commit_all(&server, &there, "late.txt", "the late fix");

    ended_there(&server, &session);

    let woken = prompt_with(&pc, "Your following of it is over.");
    assert!(woken.contains(&reference), "{woken}");
    assert_eq!(last_of(&pc, &reference), "the late fix");
}

#[test]
fn a_workspace_on_another_machine_is_removed_only_by_the_agent_that_started_its_session() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, _launch, _mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    ended_there(&server, &session);
    let (_other, other) = pc.root("auto");

    let answer = other.call(
        "workspace",
        json!({"action": "remove", "name": workspace, "machine": "server"}),
    );

    let text = refused(&answer);
    assert!(text.contains("started no session"), "{text}");
    assert!(folder_of(&agent).exists());
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

/// The references `pc` holds of what came back from other machines.
fn came_back(pc: &Machine) -> String {
    pc.git(
        &pc.folder(),
        &["for-each-ref", "--format=%(refname)", "refs/flottille/"],
    )
}

/// The id of `pc`'s root that `mcp` is the MCP session of: the one whose parent is none.
fn root_id(pc: &Machine) -> String {
    pc.agents()
        .into_iter()
        .find(|agent| agent["parent"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A `workspace` call as `pc`'s daemon sends it for its root `caller`, of the session `agent` it
/// started with the launch `launch`.
fn removal_call(id: &str, launch: &str, caller: &str, agent: &str, args: Value) -> MachineMessage {
    MachineMessage::Call(Box::new(Call {
        id: id.into(),
        launch: launch.into(),
        caller: caller.into(),
        agent: agent.into(),
        tool: "workspace".into(),
        arguments: args.to_string(),
        project: "app".into(),
        ..Call::default()
    }))
}

#[test]
fn a_device_that_may_only_view_removes_nothing_and_takes_no_commit() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, _launch, mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let there = folder_of(&agent);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    commit_all(&server, &there, "export.txt", "the export");
    hub.make_viewer("pc");
    eventually("server sees pc as a viewer", || {
        server.user.call("remote.members", json!({})).unwrap()["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["name"] == "pc" && member["role"] == "viewer")
    });

    let answer = mcp.call(
        "workspace",
        json!({"action": "remove", "name": workspace, "machine": "server"}),
    );

    let text = refused(&answer);
    assert!(text.contains("pc may only view"), "{text}");
    // Refused before anything of it was done: no commit sent to a device that may only view.
    assert_eq!(came_back(&pc), "");
    assert!(there.exists());
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn a_removal_takes_the_workspace_of_its_own_launch_when_two_projects_hold_its_name() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    eventually("server's agents", || {
        pc.seen("server")["offers"]["agents"]
            .as_array()
            .is_some_and(|agents| agents.iter().any(|agent| agent == "claude"))
    });
    // No setting that would make each launch wait for the user there.
    let file = pc.user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("branch_prefix = \"\"\n", "")).unwrap();
    pc.user.call("project.list", json!({})).unwrap();
    let (caller, mcp) = pc.root("auto");
    let first = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "First export", "brief": "One."}),
    );
    let first = (id_in(done(&first)), launch_in(done(&first)));
    // pc's branch, made again from a root of its own: server recognises its project no more and
    // makes another, where the second launch's workspace takes the same name.
    let repo = pc.folder();
    pc.git(&repo, &["checkout", "--quiet", "--orphan", "fresh"]);
    pc.git(&repo, &["add", "--all"]);
    pc.git(&repo, &["commit", "--quiet", "-m", "fresh"]);
    pc.git(&repo, &["branch", "--quiet", "-M", "feat/export"]);
    let second = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Second export", "brief": "Two."}),
    );
    let second = (id_in(done(&second)), launch_in(done(&second)));
    prompted(&server, "One.");
    prompted(&server, "Two.");
    let (one, two) = (server.agent(&first.0), server.agent(&second.0));
    assert!(one["workspace"].is_string(), "{one:#}");
    assert_eq!(one["workspace"], two["workspace"], "{one:#}\n{two:#}");
    assert_ne!(one["project"], two["project"], "{one:#}\n{two:#}");
    let workspace = one["workspace"].as_str().unwrap().to_owned();
    let (one_there, two_there) = (folder_of(&one), folder_of(&two));
    // Ended with no commit of their own: nothing is left to send back, which pc, stood in for
    // by the test, is never asked to take.
    ended_there(&server, &first.0);
    ended_there(&server, &second.0);
    let key = pc.stand_in();

    // Each removal names its launch: the workspace removed is that launch's, whichever project
    // the other is in, asked and allowed alike.
    for (n, (session, launch), gone, kept) in [
        (1, &second, &two_there, Some(&one_there)),
        (2, &first, &one_there, None),
    ] {
        let answers = sent_with(
            &key,
            &server,
            vec![removal_call(
                &format!("rm{n}"),
                launch,
                &caller,
                session,
                json!({"action": "remove", "name": workspace}),
            )],
        );
        let answer = answers[0].clone().unwrap_or_else(|error| {
            let launches = Store::open_read_only(&server.user.path("state/flottille/flottille.db"))
                .unwrap()
                .launches("in", None, LaunchFilter::All)
                .unwrap();
            panic!("{error}\ncalled {launch} of {session} as {caller}\n{launches:#?}")
        });
        let text = answer["text"].as_str().unwrap_or_default();
        let at = text
            .find("request #")
            .unwrap_or_else(|| panic!("not held: {answer:#}"))
            + "request #".len();
        let number: u64 = text[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap();
        // The caller's uncommitted work laid there is lost with it: the user says to discard it.
        server
            .user
            .call(
                "approval.approve",
                json!({"number": number, "discard": true}),
            )
            .unwrap();
        eventually("the workspace of the launch named removed", || {
            !gone.exists()
        });
        if let Some(kept) = kept {
            assert!(kept.exists(), "the other launch's workspace was removed");
        }
    }
}

#[test]
fn in_full_a_removal_from_another_machine_that_would_lose_work_is_held() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("full");
    let switched = server
        .user
        .run(&["--machine", "pc", "agent", "mode", &root, "full"]);
    assert!(switched.status.success(), "{}", pc.log());
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export."}),
    );
    let session = id_in(done(&answer));
    prompted(&server, "Finish the export.");
    let agent = server.agent(&session);
    assert_eq!(agent["gate_mode"], "full", "{agent:#}");
    let there = folder_of(&agent);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    // Work never committed, which cannot come back.
    std::fs::write(there.join("draft.txt"), "a draft\n").unwrap();
    ended_there(&server, &session);

    let answer = mcp.call(
        "workspace",
        json!({"action": "remove", "name": workspace, "machine": "server"}),
    );

    // Held for the user there, as the removal of its own workspace is in Full on one machine.
    let number = held(&answer);
    let asked = request(&server, number);
    assert!(asked["says"].to_string().contains("draft.txt"), "{asked:#}");
    assert!(there.exists());
}

#[test]
fn in_full_a_removal_from_another_machine_that_loses_nothing_is_done() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("full");
    let switched = server
        .user
        .run(&["--machine", "pc", "agent", "mode", &root, "full"]);
    assert!(switched.status.success(), "{}", pc.log());
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export."}),
    );
    let session = id_in(done(&answer));
    prompted(&server, "Finish the export.");
    let agent = server.agent(&session);
    let there = folder_of(&agent);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    // Its work, committed: it comes back before the removal is judged.
    commit_all(&server, &there, "export.txt", "the export");
    ended_there(&server, &session);

    let answer = mcp.call(
        "workspace",
        json!({"action": "remove", "name": workspace, "machine": "server"}),
    );

    // Nothing to lose: done at once, as Full does any other, no request waiting there.
    let text = done(&answer);
    assert!(!text.contains("request #"), "{text}");
    eventually("the workspace removed there", || !there.exists());
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn in_full_a_removal_from_another_machine_is_not_done_when_work_came_after_its_card() {
    let user = server_user(allowing());
    let after_card = user.path("after-card");
    let reached = user.path("after-card.reached");
    let user = user.with_env(
        "FLOTTILLE_TEST_AFTER_CARD_FILE",
        after_card.to_str().unwrap(),
    );
    let (_hub, pc, server) = pc_and_server(user, Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("full");
    let switched = server
        .user
        .run(&["--machine", "pc", "agent", "mode", &root, "full"]);
    assert!(switched.status.success(), "{}", pc.log());
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export."}),
    );
    let session = id_in(done(&answer));
    prompted(&server, "Finish the export.");
    let agent = server.agent(&session);
    let there = folder_of(&agent);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    commit_all(&server, &there, "export.txt", "the export");
    ended_there(&server, &session);
    std::fs::write(&after_card, "").unwrap();

    let (answer, card_made) = std::thread::scope(|scope| {
        // Once the card is made (nothing would be lost), a commit lands in the workspace there;
        // the call is let go whatever happens here.
        let late = scope.spawn(|| {
            let made = (0..300).any(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                reached.exists()
            });
            let committed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if made {
                    commit_all(&server, &there, "late.txt", "a late commit");
                }
            }));
            std::fs::remove_file(&after_card).unwrap();
            committed.is_ok() && made
        });
        let answer = mcp.call(
            "workspace",
            json!({"action": "remove", "name": workspace, "machine": "server"}),
        );
        (answer, late.join().unwrap())
    });

    assert!(card_made, "the card was never made, or the commit failed");
    // What the card did not say would be lost is not discarded unseen: refused, or held.
    let text = &answer.text;
    assert!(
        text.contains("refused") || text.contains("request #"),
        "{text}"
    );
    assert!(there.exists(), "the workspace was removed: {text}");
    assert!(there.join("late.txt").exists());
}

#[test]
fn a_removal_there_is_refused_for_another_workspace_a_handed_session_or_another_action() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, launch, mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let handed = launched(
        &mcp,
        json!({"kind": "handed", "machine": "server", "title": "Let go", "brief": "Yours."}),
    );
    let handed_session = id_in(done(&handed));
    let handed_agent = server.agent(&handed_session);
    let handed_launch = Store::open_read_only(&server.user.path("state/flottille/flottille.db"))
        .unwrap()
        .launches("in", None, LaunchFilter::Agent(&handed_session))
        .unwrap()
        .pop()
        .unwrap()
        .launch;
    ended_there(&server, &session);
    let caller = root_id(&pc);
    let key = pc.stand_in();

    let answers = sent_with(
        &key,
        &server,
        vec![
            // A workspace that is not the launch's.
            removal_call(
                "w1",
                &launch,
                &caller,
                &session,
                json!({"action": "remove", "name": "elsewhere"}),
            ),
            // The workspace of a session handed to the user.
            removal_call(
                "w2",
                &handed_launch,
                &caller,
                &handed_session,
                json!({"action": "remove", "name": handed_agent["workspace"]}),
            ),
            // An action other than remove.
            removal_call(
                "w3",
                &launch,
                &caller,
                &session,
                json!({"action": "create", "name": "more"}),
            ),
        ],
    );

    let first = answers[0].clone().unwrap();
    assert_eq!(first["outcome"], "refused", "{first:#}");
    let text = first["text"].as_str().unwrap();
    assert!(
        text.contains("the workspace elsewhere is not the one of agent"),
        "{text}"
    );
    let second = answers[1].clone().unwrap_err();
    assert!(second.contains("handed to the user"), "{second}");
    let third = answers[2].clone().unwrap_err();
    assert!(third.contains("only remove is"), "{third}");
    assert!(folder_of(&agent).exists());
    assert!(folder_of(&handed_agent).exists());
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn a_child_removes_no_workspace_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, _launch, mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    ended_there(&server, &session);
    script(
        &pc,
        json!([{"when": "Tidy the server.", "times": 1, "do": [
            {"mcp": {"tool": "workspace", "arguments":
                {"action": "remove", "name": workspace, "machine": "server"}}}
        ]}]),
    );

    done(&mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "Tidy", "brief": "Tidy the server."}),
    ));

    // A child manages no workspace, here or there: refused before anything is sent.
    answered(
        &pc,
        "the agent the user talks to manages the project's workspaces",
    );
    assert!(folder_of(&agent).exists());
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn another_action_than_remove_on_another_machine_is_refused_in_words() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");

    let answer = mcp.call(
        "workspace",
        json!({"action": "create", "name": "more", "machine": "server"}),
    );

    let text = refused(&answer);
    assert!(
        text.contains("with machine, workspace only removes"),
        "{text}"
    );
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn an_end_told_once_its_workspace_is_gone_says_nothing_of_commits_that_came_back() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "The export is reported."}}}
        ]}]),
    );
    let (session, _launch, _mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let there = folder_of(&agent);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    commit_all(&server, &there, "export.txt", "the export");
    release(&server);
    // Every commit came back with its report.
    prompted(&pc, "it reported");
    off(&pc);
    // Its end's delivery is kept until pc is back, and composed then.
    eventually("server sees pc offline", || {
        server.seen("pc")["state"] != "online"
    });
    ended_there(&server, &session);
    // The user of the server removes it: nothing is lost, all of it came back.
    server
        .user
        .call(
            "workspace.remove",
            json!({"project": "app", "name": workspace}),
        )
        .unwrap();
    eventually("the workspace removed", || !there.exists());

    on(&pc);

    let woken = prompt_with(&pc, "Your following of it is over.");
    assert!(!woken.contains("could not come back"), "{woken}");
}
