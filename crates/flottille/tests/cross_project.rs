//! A session leads a session of another project (#1058, `docs/tools.md`, "Sessions an agent
//! starts", "Sessions that talk to each other", "Sessions that read each other"): a launch in
//! another registered project is held for the user in every mode, and the session it starts is a
//! led root of that project, in its places, with its tools, its vault and its mode, never above
//! the caller's. Writing to it asks the user once per pair, even though the caller started it.
//! Against the real daemon with the fake ACP agent, two throwaway projects and temporary XDG
//! folders.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A user whose agents answer every message, with a judge allowing every call, and two projects,
/// `app` and `kit`.
fn user() -> User {
    let user = User::with_fake_agent().with_allowing_judge();
    project(&user, "app");
    project(&user, "kit");
    user
}

/// A project over a throwaway repository.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// A session the user starts in `project`, its first turn ended: its id and its MCP session.
fn root(user: &User, project: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call("agent.run", json!({"project": project, "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    idle(user, &id);
    (id, Mcp::initialize(&url, &authorization))
}

/// The lead of `app`, titled "Release the fonts", in `mode`.
fn lead(user: &User, mode: &str) -> (String, Mcp) {
    let (id, mcp) = root(user, "app");
    user.call(
        "agent.rename",
        json!({"agent": id, "title": "Release the fonts"}),
    )
    .unwrap();
    set_mode(user, &id, mode);
    (id, mcp)
}

fn idle(user: &User, agent: &str) {
    eventually("the turn's end", || shown(user, agent)["state"] == "idle");
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn store(user: &User) -> Store {
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    user.call("agent.mode", json!({"agent": agent, "mode": mode}))
        .unwrap();
}

fn shown(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn judge_prompts(user: &User) -> usize {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .count()
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The user answers request `number`: the text the agent is handed for it.
fn answer(user: &User, number: u64, method: &str) -> String {
    user.call(method, json!({"number": number})).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// The bodies of the deliveries of `kind` stored for `agent`, newest first.
fn delivered(user: &User, agent: &str, kind: &str) -> Vec<String> {
    store(user)
        .last_deliveries(agent, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| delivery.body)
        .collect()
}

fn written_to(user: &User, agent: &str) -> Vec<String> {
    delivered(user, agent, "session")
}

fn send(from: &Mcp, to: &str, text: &str) -> Answer {
    from.call("session_send", json!({"session": to, "text": text}))
}

fn leaves(user: &User, agent: &str) -> Vec<(String, String)> {
    user.call("leave.list", json!({"session": agent})).unwrap()["leaves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|leave| {
            (
                leave["writer"]["id"].as_str().unwrap().to_owned(),
                leave["session"]["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The arguments of a launch of a led session in `kit`.
fn in_kit(extra: Value) -> Value {
    let mut args = json!({"kind": "led", "project": "kit", "title": "Make the fonts settable",
        "brief": "Make the fonts settable."});
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    args
}

/// The lead's launch in `kit`, held then allowed by the user: the started session's id and MCP
/// session.
fn started_in_kit(user: &User, mcp: &Mcp, extra: Value) -> (String, Mcp) {
    let args = in_kit(extra);
    let title = args["title"].clone();
    let number = held(&mcp.call("agent_launch", args));
    let opened = user.received_requests("session/new").len();
    let done = answer(user, number, "approval.approve");
    assert!(done.contains("and Flottille has now done it"), "{done}");
    let (url, authorization) = server_of(user, opened);
    let id = agents(user)
        .into_iter()
        .find(|a| a["title"] == title)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (id, Mcp::initialize(&url, &authorization))
}

#[test]
fn a_launch_in_another_project_is_held_for_the_user_in_every_mode() {
    let user = user();
    let (lead, mcp) = lead(&user, "auto");
    let judged = judge_prompts(&user);

    for mode in ["ask", "auto", "full"] {
        set_mode(&user, &lead, mode);
        let number = held(&mcp.call("agent_launch", in_kit(json!({}))));

        let listed = requests(&user);
        assert_eq!(listed.len(), 1, "{mode}: {listed:?}");
        let request = &listed[0];
        assert_eq!(request["number"], number);
        assert_eq!(request["class"], "base", "{mode}: {request}");
        let says = request["says"].to_string();
        for word in [
            "app's session \\\"Release the fonts\\\" asks to start a session in kit",
            "Make the fonts settable",
            "Its first message: Make the fonts settable.",
        ] {
            assert!(says.contains(word), "{mode}: {word}: {says}");
        }
        assert!(
            agents(&user)
                .iter()
                .all(|a| a["title"] != "Make the fonts settable"),
            "{mode}: started before the user answered"
        );
        user.call("approval.deny", json!({"number": number}))
            .unwrap();
    }
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
}

#[test]
fn a_launch_in_another_project_the_user_refuses_starts_nothing_and_is_told() {
    let user = user();
    let (_, mcp) = lead(&user, "full");

    let number = held(&mcp.call("agent_launch", in_kit(json!({}))));
    let said = answer(&user, number, "approval.deny");

    assert!(said.contains("was refused by the user"), "{said}");
    assert!(
        agents(&user)
            .iter()
            .all(|a| a["title"] != "Make the fonts settable" && a["project"] != "kit"),
        "{:?}",
        agents(&user)
    );
}

#[test]
fn an_accepted_launch_starts_a_led_root_of_the_other_project_in_its_place() {
    let user = user();
    let (lead, mcp) = lead(&user, "auto");

    let (session, _) = started_in_kit(&user, &mcp, json!({"new_workspace": "fonts"}));

    let agent = shown(&user, &session);
    assert_eq!(agent["project"], "kit", "{agent}");
    assert!(agent["parent"].is_null(), "{agent}");
    assert_eq!(agent["launched_by"], lead.as_str(), "{agent}");
    assert_eq!(agent["led_by"], lead.as_str(), "{agent}");
    assert_eq!(agent["role"], "lead", "{agent}");
    eventually("its workspace", || {
        shown(&user, &session)["workspace"] == "fonts"
    });
    assert!(
        user.call("workspace.show", json!({"project": "kit", "name": "fonts"}))
            .is_ok(),
        "no workspace was made in kit"
    );
    assert!(
        user.call("workspace.show", json!({"project": "app", "name": "fonts"}))
            .is_err(),
        "a workspace was made in the caller's project"
    );
    // The answer said where it went.
    let done = store(&user)
        .last_deliveries(&lead, 64)
        .unwrap()
        .into_iter()
        .find(|d| d.kind == "approval")
        .unwrap()
        .body;
    assert!(done.contains("Started a session led by you"), "{done}");
    assert!(done.contains("of project kit"), "{done}");
}

#[test]
fn the_session_started_in_another_project_wakes_its_pilot_when_it_reports() {
    let user = user();
    user.fake_script(json!({"turns": [
        {"when": "Make the fonts settable.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The fonts are settable."}}},
            {"say": "Reported."}]}
    ]}));
    let (lead, mcp) = lead(&user, "auto");

    let (session, _) = started_in_kit(&user, &mcp, json!({"workspace": "main"}));

    eventually("the pilot woken", || {
        !delivered(&user, &lead, "session-turn-ended").is_empty()
    });
    let woken = delivered(&user, &lead, "session-turn-ended").remove(0);
    assert!(woken.contains("it reported"), "{woken}");
    assert!(woken.contains(&session), "{woken}");
}

#[test]
fn the_session_started_in_another_project_leads_its_own_sessions() {
    let user = user();
    let (lead, mcp) = lead(&user, "auto");
    let (session, kit_mcp) = started_in_kit(&user, &mcp, json!({}));

    // Every tool a root of its project has, as the lead has in its own.
    let names = kit_mcp.names();
    assert_eq!(names, mcp.names());
    for tool in ["agent_launch", "session_send", "session_read"] {
        assert!(names.iter().any(|name| name == tool), "{tool}: {names:?}");
    }
    // Its own session in its own project: started as any launched session's is, the user asked
    // at each step of a chain.
    let number = held(&kit_mcp.call(
        "agent_launch",
        json!({"kind": "led", "title": "Tag the release", "brief": "Tag it."}),
    ));
    answer(&user, number, "approval.approve");
    eventually("its own led session", || {
        agents(&user)
            .iter()
            .any(|a| a["title"] == "Tag the release")
    });
    let own = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Tag the release")
        .unwrap();
    assert_eq!(own["project"], "kit", "{own}");
    let own = shown(&user, own["id"].as_str().unwrap());
    assert_eq!(own["led_by"], session.as_str(), "{own}");
    assert_ne!(own["led_by"], lead.as_str(), "{own}");
}

#[test]
fn the_session_started_in_another_project_runs_in_its_project_s_mode_never_above_the_caller_s() {
    let user = user();
    let (lead, mcp) = lead(&user, "full");

    let (from_full, _) = started_in_kit(&user, &mcp, json!({}));
    set_mode(&user, &lead, "ask");
    let (from_ask, _) = started_in_kit(&user, &mcp, json!({"title": "Second"}));

    // A session kit gives a new session is in Auto: Full is not carried over to another project.
    assert_eq!(shown(&user, &from_full)["gate_mode"], "auto");
    assert_eq!(shown(&user, &from_ask)["gate_mode"], "ask");
}

#[test]
fn a_session_started_in_another_project_uses_that_project_s_vault_entries_not_the_caller_s() {
    let user = user();
    user.call(
        "vault.set",
        json!({"name": "kit-token", "secret": "kit-secret-1", "project": "kit"}),
    )
    .unwrap();
    user.call(
        "vault.set",
        json!({"name": "app-token", "secret": "app-secret-1", "project": "app"}),
    )
    .unwrap();
    let (_, mcp) = lead(&user, "auto");
    let (_, kit_mcp) = started_in_kit(&user, &mcp, json!({}));

    let listed = kit_mcp.call("vault", json!({"action": "list"}));

    assert!(!listed.error, "{}", listed.text);
    assert!(listed.text.contains("kit-token"), "{}", listed.text);
    assert!(!listed.text.contains("app-token"), "{}", listed.text);
    assert!(!listed.text.contains("secret-1"), "{}", listed.text);
}

#[test]
fn the_first_message_to_the_session_started_in_another_project_asks_the_user_once() {
    let user = user();
    let (lead, mcp) = lead(&user, "full");
    let (session, _) = started_in_kit(&user, &mcp, json!({}));
    let judged = judge_prompts(&user);

    let number = held(&send(&mcp, &session, "Use the tokens."));
    let says = requests(&user)[0]["says"].to_string();
    assert!(says.contains("kit"), "{says}");
    assert!(
        says.contains("Its first message: Use the tokens."),
        "{says}"
    );
    assert!(
        !written_to(&user, &session)
            .iter()
            .any(|b| b.contains("Use the tokens."))
    );

    let done = answer(&user, number, "approval.approve");
    assert!(done.contains("and Flottille has now done it"), "{done}");
    eventually("the first message", || {
        written_to(&user, &session)
            .iter()
            .any(|b| b.contains("Use the tokens."))
    });
    assert_eq!(leaves(&user, &session), [(lead.clone(), session.clone())]);
    assert_eq!(leaves(&user, &lead), [(lead.clone(), session.clone())]);

    let again = send(&mcp, &session, "And the sizes.");
    assert!(!again.error, "{}", again.text);
    assert!(again.text.starts_with("Sent to "), "{}", again.text);
    assert!(requests(&user).is_empty(), "{:?}", requests(&user));
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
}

#[test]
fn a_withdrawn_write_access_holds_the_next_message_for_the_user() {
    let user = user();
    let (lead, mcp) = lead(&user, "full");
    let (session, _) = started_in_kit(&user, &mcp, json!({}));
    let number = held(&send(&mcp, &session, "Use the tokens."));
    answer(&user, number, "approval.approve");
    eventually("the leave", || !leaves(&user, &session).is_empty());

    user.call(
        "leave.withdraw",
        json!({"writer": lead, "session": session}),
    )
    .unwrap();

    held(&send(&mcp, &session, "And the sizes."));
}

#[test]
fn the_first_message_to_another_project_s_session_not_started_by_the_caller_asks_the_user_once() {
    let user = user();
    let (lead, mcp) = lead(&user, "full");
    let (other, _) = root(&user, "kit");

    let number = held(&send(&mcp, &other, "Is the release out?"));
    let says = requests(&user)[0]["says"].to_string();
    assert!(says.contains("kit"), "{says}");
    answer(&user, number, "approval.approve");
    eventually("the first message", || {
        written_to(&user, &other)
            .iter()
            .any(|b| b.contains("Is the release out?"))
    });
    assert_eq!(leaves(&user, &other), [(lead.clone(), other.clone())]);

    let again = send(&mcp, &other, "Thanks.");
    assert!(!again.error, "{}", again.text);
    assert!(again.text.starts_with("Sent to "), "{}", again.text);
}

#[test]
fn a_session_of_another_project_is_read_a_page_at_a_time() {
    let user = user();
    let (lead, mcp) = lead(&user, "auto");
    let (session, _) = started_in_kit(&user, &mcp, json!({}));
    idle(&user, &session);
    user.call(
        "agent.send",
        json!({"agent": session, "message": "Also the weights."}),
    )
    .unwrap();
    eventually("its second turn", || {
        store(&user)
            .last_agent_events(&session, "agent.turn_ended", 10)
            .unwrap()
            .len()
            == 2
    });

    // The session it started reads freely: no leave is asked.
    let page = mcp.call("session_read", json!({"session": session, "last": 1}));
    assert!(!page.error, "{}", page.text);
    assert!(
        page.text.contains("another session of project kit"),
        "{}",
        page.text
    );
    assert!(page.text.contains("Message 1 of 1"), "{}", page.text);
    assert!(page.text.contains("Earlier messages: "), "{}", page.text);
    assert!(requests(&user).is_empty(), "{:?}", requests(&user));

    // One it did not start asks the user's leave, once for the pair.
    let (other, _) = root(&user, "kit");
    let number = held(&mcp.call("session_read", json!({"session": other})));
    let done = answer(&user, number, "approval.approve");
    assert!(done.contains("and Flottille has now done it"), "{done}");
    assert!(done.contains("another session of project kit"), "{done}");
    assert_eq!(leaves(&user, &other), [(lead.clone(), other.clone())]);
    let again = mcp.call("session_read", json!({"session": other}));
    assert!(!again.error, "{}", again.text);
    assert!(
        again.text.contains("another session of project kit"),
        "{}",
        again.text
    );
}

#[test]
fn a_child_is_never_launched_in_another_project() {
    let user = user();
    let (_, mcp) = lead(&user, "full");

    let refused = mcp.call(
        "agent_launch",
        json!({"kind": "child", "project": "kit", "title": "worker", "brief": "Go."}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused.text.starts_with("refused: a child"),
        "{}",
        refused.text
    );
    assert!(refused.text.contains("kit"), "{}", refused.text);
    assert!(requests(&user).is_empty());
    assert!(agents(&user).iter().all(|a| a["title"] != "worker"));
}

#[test]
fn a_title_names_a_session_of_the_caller_s_project_only() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    let (in_app, _) = root(&user, "app");
    let (in_kit, _) = root(&user, "kit");
    let (only_kit, _) = root(&user, "kit");
    for (id, title) in [
        (&in_app, "Fonts"),
        (&in_kit, "Fonts"),
        (&only_kit, "Tokens"),
    ] {
        user.call("agent.rename", json!({"agent": id, "title": title}))
            .unwrap();
    }

    // The same title in two projects is not ambiguous: the caller's project's is meant.
    let number = held(&send(&mcp, "Fonts", "Hello."));
    let request = requests(&user)
        .into_iter()
        .find(|r| r["number"] == number)
        .unwrap();
    assert_eq!(request["call"]["session"], in_app.as_str(), "{request}");

    // A title only another project's session has names none of the caller's.
    let refused = send(&mcp, "Tokens", "Hello.");
    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .starts_with("refused: no session of your project has the id or the title \"Tokens\""),
        "{}",
        refused.text
    );
    assert!(!refused.text.contains(&only_kit), "{}", refused.text);
}

#[test]
fn agent_stop_ends_the_session_it_leads_in_another_project_and_holds_any_other() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    let (session, _) = started_in_kit(&user, &mcp, json!({}));
    let (other, _) = root(&user, "kit");

    let ended = mcp.call("agent_stop", json!({"agent_id": session}));
    assert!(!ended.error, "{}", ended.text);
    assert!(
        ended.text.starts_with("Ended the session"),
        "{}",
        ended.text
    );
    eventually("its end", || shown(&user, &session)["state"] == "ended");

    let number = held(&mcp.call("agent_stop", json!({"agent_id": other})));
    let says = requests(&user)[0]["says"].to_string();
    assert!(says.contains("kit"), "{says}");
    user.call("approval.deny", json!({"number": number}))
        .unwrap();
    assert_ne!(shown(&user, &other)["state"], "ended");
}

#[test]
fn ending_a_pilot_ends_a_session_it_leads_in_another_project() {
    let user = user();
    let (lead, mcp) = lead(&user, "full");
    let (session, _) = started_in_kit(&user, &mcp, json!({}));
    idle(&user, &session);
    assert_eq!(shown(&user, &session)["led_by"], lead.as_str());

    user.call("agent.stop", json!({"agent": lead})).unwrap();

    eventually("the session of kit ended with its pilot", || {
        shown(&user, &session)["state"] == "ended"
    });
}

/// A user whose judge takes a while to allow every call, with the projects `app` and `kit`.
fn user_with_a_slow_judge() -> User {
    let user = User::with_fake_agent().with_judge(json!([
        {"wait": 1500},
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    project(&user, "app");
    project(&user, "kit");
    user
}

#[test]
fn a_session_handed_over_in_another_project_is_the_users_and_asks_the_leave_for_its_first_message()
{
    let user = user();
    let (lead, mcp) = lead(&user, "auto");

    let number = held(&mcp.call(
        "agent_launch",
        in_kit(json!({"kind": "handed", "title": "Tag the release"})),
    ));
    let opened = user.received_requests("session/new").len();
    let done = answer(&user, number, "approval.approve");
    assert!(done.contains("handed over"), "{done}");
    assert!(done.contains("of project kit"), "{done}");
    let (_url, _authorization) = server_of(&user, opened);
    let session = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Tag the release")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let agent = shown(&user, &session);
    assert_eq!(agent["project"], "kit", "{agent}");
    assert!(agent["parent"].is_null(), "{agent}");
    assert_eq!(agent["launched_by"], lead.as_str(), "{agent}");
    assert!(agent["led_by"].is_null(), "handed over, not led: {agent}");
    // Handed over, it is the user's: the caller's first message to it still asks the user.
    held(&send(&mcp, &session, "Which tag?"));
}

#[test]
fn a_session_started_in_another_project_by_an_auto_caller_runs_in_auto() {
    let user = user();
    let (_, mcp) = lead(&user, "auto");

    let (session, _) = started_in_kit(&user, &mcp, json!({}));

    assert_eq!(shown(&user, &session)["gate_mode"], "auto");
}

#[test]
fn the_reminder_of_a_session_led_in_another_project_never_invites_removing_a_workspace_of_the_pilots_project()
 {
    let user = user();
    user.fake_script(json!({"turns": [
        {"when": "Make the fonts settable.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The fonts are settable."}}},
            {"say": "Reported."}]}
    ]}));
    let (lead, mcp) = lead(&user, "auto");
    // Both projects hold a workspace of the same name: the pilot's workspace tool reaches its own.
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "fonts"}),
    )
    .unwrap();
    let (session, _) = started_in_kit(&user, &mcp, json!({"new_workspace": "fonts"}));
    eventually("the pilot woken", || {
        !delivered(&user, &lead, "session-turn-ended").is_empty()
    });
    idle(&user, &lead);
    idle(&user, &session);
    std::thread::sleep(Duration::from_millis(500));
    let folder = std::path::PathBuf::from(shown(&user, &session)["folder"].as_str().unwrap());
    std::fs::write(folder.join("fonts.rs"), "fn fonts() {}\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "settable fonts"]);
    let branch = user.git_output(&folder, &["branch", "--show-current"]);
    let kit = user.path("work/kit");
    user.git(
        &kit,
        &["merge", "--quiet", "--no-ff", "-m", "merge", branch.trim()],
    );

    user.call("agent.send", json!({"agent": lead, "message": "Go on."}))
        .unwrap();

    eventually("the reminder", || {
        !delivered(&user, &lead, "led-integrated").is_empty()
    });
    let reminder = delivered(&user, &lead, "led-integrated").remove(0);
    assert!(reminder.contains("stop it"), "{reminder}");
    assert!(reminder.contains("project kit"), "{reminder}");
    assert!(
        !reminder.contains("remove its workspace?"),
        "it invites the pilot to remove a workspace: {reminder}"
    );
    assert!(!reminder.contains("workspace remove"), "{reminder}");
    // The pilot's own workspace of the same name is still there.
    assert!(
        user.call("workspace.show", json!({"project": "app", "name": "fonts"}))
            .is_ok()
    );
}

#[test]
fn a_pending_read_leave_and_a_write_for_the_same_pair_are_one_card() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    let (other, _) = root(&user, "kit");

    let read = held(&mcp.call("session_read", json!({"session": other})));
    let write = send(&mcp, &other, "Is the release out?");

    assert!(write.error, "{}", write.text);
    assert!(
        write
            .text
            .contains(&format!("request #{read} already asks")),
        "{}",
        write.text
    );
    assert_eq!(requests(&user).len(), 1, "{:?}", requests(&user));
    user.call("approval.deny", json!({"number": read})).unwrap();

    // And the other way round.
    let write = held(&send(&mcp, &other, "Is the release out?"));
    let read = mcp.call("session_read", json!({"session": other}));
    assert!(read.error, "{}", read.text);
    assert!(
        read.text
            .contains(&format!("request #{write} already asks")),
        "{}",
        read.text
    );
    assert_eq!(requests(&user).len(), 1, "{:?}", requests(&user));
}

#[test]
fn a_leave_withdrawn_while_a_read_of_another_project_is_judged_stops_it() {
    let user = user_with_a_slow_judge();
    let (lead, mcp) = root(&user, "app");
    let (other, _) = root(&user, "kit");
    let number = held(&mcp.call("session_read", json!({"session": other})));
    answer(&user, number, "approval.approve");
    eventually("the leave", || !leaves(&user, &other).is_empty());

    let judged = std::thread::scope(|scope| {
        let reading = scope.spawn(|| mcp.call("session_read", json!({"session": other})));
        eventually("the judge asked", || judge_prompts(&user) > 0);
        user.call("leave.withdraw", json!({"writer": lead, "session": other}))
            .unwrap();
        reading.join().unwrap()
    });

    assert!(judged.error, "{}", judged.text);
    assert!(
        judged.text.contains("withdrew your leave"),
        "{}",
        judged.text
    );
    assert!(
        !judged.text.contains("Message 1"),
        "the conversation was read: {}",
        judged.text
    );
}

#[test]
fn the_card_of_a_launch_in_another_project_says_the_services_of_that_project_s_setup() {
    let user = user();
    let folder = user.path("home/tools/db");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        user.path("config/flottille/services.toml"),
        format!(
            "[services.db]\nfolder = \"{}\"\nline = \"{} --wait 0 --listen 0\"\nneeds = []\nready = {{ line = \"listening on\" }}\n",
            folder.display(),
            common::fake_server().display()
        ),
    )
    .unwrap();
    // Only kit has the task, which needs the service, and its setup runs it.
    user.call(
        "task.add",
        json!({"project": "kit", "name": "api", "line": "git status --short", "needs": ["db"]}),
    )
    .unwrap();
    user.call(
        "setup.add",
        json!({"project": "kit", "step": {"task": "api"}}),
    )
    .unwrap();
    let (_, mcp) = lead(&user, "auto");

    held(&mcp.call("agent_launch", in_kit(json!({"new_workspace": "fonts"}))));

    let says = requests(&user)[0]["says"].to_string();
    assert!(says.contains("the service db"), "{says}");
}

#[test]
fn a_setup_step_the_gate_refuses_in_the_other_project_refuses_the_launch() {
    let user = user();
    let file = user.project_file("kit");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[[setup]]\nrun = \"tmux new-session\"\n");
    std::fs::write(&file, text).unwrap();
    let (_, mcp) = lead(&user, "auto");

    let refused = mcp.call("agent_launch", in_kit(json!({"new_workspace": "fonts"})));

    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.starts_with("refused: "), "{}", refused.text);
    assert!(refused.text.contains("tmux"), "{}", refused.text);
    assert!(requests(&user).is_empty(), "{:?}", requests(&user));
}

#[test]
fn the_first_approved_message_to_the_session_started_in_another_project_is_counted() {
    let user = user();
    let (_, mcp) = lead(&user, "auto");
    let (session, _) = started_in_kit(&user, &mcp, json!({}));
    let exchanges = || {
        store(&user)
            .subscriptions()
            .unwrap()
            .into_iter()
            .find(|s| s.target["session"] == session.as_str())
            .map(|s| s.seen["exchanges"].as_u64().unwrap_or(0))
    };
    assert_eq!(exchanges(), Some(0));

    let number = held(&send(&mcp, &session, "Use the tokens."));
    answer(&user, number, "approval.approve");

    eventually("the first message", || {
        written_to(&user, &session)
            .iter()
            .any(|b| b.contains("Use the tokens."))
    });
    eventually("it is counted", || exchanges() == Some(1));
}

// A title finds a session of another project (#1087).

/// A root of `project` that the user titled.
fn titled(user: &User, project: &str, title: &str) -> String {
    let (id, _) = root(user, project);
    user.call("agent.rename", json!({"agent": id, "title": title}))
        .unwrap();
    id
}

/// A write to the session of `project` titled `title`.
fn send_in(from: &Mcp, project: &str, title: &str, text: &str) -> Answer {
    from.call(
        "session_send",
        json!({"project": project, "session": title, "text": text}),
    )
}

#[test]
fn a_session_of_another_project_is_reached_by_its_project_and_title() {
    let user = user();
    let (lead, mcp) = lead(&user, "full");
    let other = titled(&user, "kit", "Tokens");
    // The same title in the caller's own project is another session: `project` says which.
    titled(&user, "app", "Tokens");

    let number = held(&send_in(&mcp, "kit", "Tokens", "Is the release out?"));
    let request = requests(&user)
        .into_iter()
        .find(|r| r["number"] == number)
        .unwrap();
    assert_eq!(request["call"]["session"], other.as_str(), "{request}");
    answer(&user, number, "approval.approve");
    eventually("the message", || {
        written_to(&user, &other)
            .iter()
            .any(|b| b.contains("Is the release out?"))
    });
    assert_eq!(leaves(&user, &other), [(lead.clone(), other.clone())]);

    // With the leave, the title writes and reads as the id does.
    let again = send_in(&mcp, "kit", "Tokens", "Thanks.");
    assert!(again.text.starts_with("Sent to "), "{}", again.text);
    let page = mcp.call(
        "session_read",
        json!({"project": "kit", "session": "Tokens"}),
    );
    assert!(!page.error, "{}", page.text);
    assert!(
        page.text.contains("another session of project kit"),
        "{}",
        page.text
    );
}

#[test]
fn a_title_of_another_project_still_asks_the_user_s_leave_first() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    let other = titled(&user, "kit", "Tokens");

    let number = held(&send_in(&mcp, "kit", "Tokens", "Is the release out?"));
    let says = requests(&user)[0]["says"].to_string();
    assert!(says.contains("kit"), "{says}");
    assert!(says.contains("Is the release out?"), "{says}");
    assert!(written_to(&user, &other).is_empty());
    assert!(leaves(&user, &other).is_empty());

    // A read by title asks too, as a read by id does, and one leave covers both.
    let read = mcp.call(
        "session_read",
        json!({"project": "kit", "session": "Tokens"}),
    );
    assert!(
        read.text
            .contains(&format!("request #{number} already asks")),
        "{}",
        read.text
    );
    assert_eq!(requests(&user).len(), 1, "{:?}", requests(&user));
    user.call("approval.deny", json!({"number": number}))
        .unwrap();
    assert!(written_to(&user, &other).is_empty());
}

#[test]
fn an_ambiguous_title_in_another_project_is_refused_with_the_ids() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    let first = titled(&user, "kit", "Fonts");
    let second = titled(&user, "kit", "Fonts");

    let refused = send_in(&mcp, "kit", "Fonts", "Hello.");

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .starts_with("refused: 2 sessions of project kit are titled \"Fonts\""),
        "{}",
        refused.text
    );
    assert!(refused.text.contains(&first), "{}", refused.text);
    assert!(refused.text.contains(&second), "{}", refused.text);
    assert!(requests(&user).is_empty());

    // Read the same way.
    let refused = mcp.call(
        "session_read",
        json!({"project": "kit", "session": "Fonts"}),
    );
    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.contains(&first), "{}", refused.text);
    assert!(requests(&user).is_empty());
}

#[test]
fn a_child_of_another_project_is_not_reached_by_its_title() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    let (_, kit_mcp) = root(&user, "kit");
    let launched = kit_mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "Hidden worker", "brief": "Wait."}),
    );
    assert!(!launched.error, "{}", launched.text);
    eventually("the child", || {
        agents(&user).iter().any(|a| a["title"] == "Hidden worker")
    });
    let child = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Hidden worker")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let refused = send_in(&mcp, "kit", "Hidden worker", "Hello.");

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused.text.starts_with(
            "refused: no session of project kit has the id or the title \"Hidden worker\""
        ),
        "{}",
        refused.text
    );
    assert!(!refused.text.contains(&child), "{}", refused.text);
    assert!(requests(&user).is_empty());
}

#[test]
fn an_unregistered_project_is_refused_before_anything_is_asked() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    titled(&user, "kit", "Tokens");
    let judged = judge_prompts(&user);

    for refused in [
        send_in(&mcp, "ghost", "Tokens", "Hello."),
        mcp.call(
            "session_read",
            json!({"project": "ghost", "session": "Tokens"}),
        ),
    ] {
        assert!(refused.error, "{}", refused.text);
        assert!(
            refused
                .text
                .starts_with("refused: project ghost is not registered"),
            "{}",
            refused.text
        );
        assert!(
            refused.text.contains("app") && refused.text.contains("kit"),
            "{}",
            refused.text
        );
    }
    assert!(requests(&user).is_empty());
    assert_eq!(judge_prompts(&user), judged);
}

#[test]
fn without_project_a_title_is_still_matched_in_the_caller_s_project_only() {
    let user = user();
    let (_, mcp) = lead(&user, "full");
    let in_app = titled(&user, "app", "Fonts");
    titled(&user, "kit", "Fonts");
    let only_kit = titled(&user, "kit", "Tokens");

    let number = held(&send(&mcp, "Fonts", "Hello."));
    let request = requests(&user)
        .into_iter()
        .find(|r| r["number"] == number)
        .unwrap();
    assert_eq!(request["call"]["session"], in_app.as_str(), "{request}");

    for refused in [
        send(&mcp, "Tokens", "Hello."),
        mcp.call("session_read", json!({"session": "Tokens"})),
    ] {
        assert!(refused.error, "{}", refused.text);
        assert!(
            refused.text.starts_with(
                "refused: no session of your project has the id or the title \"Tokens\""
            ),
            "{}",
            refused.text
        );
        assert!(!refused.text.contains(&only_kit), "{}", refused.text);
    }
}
