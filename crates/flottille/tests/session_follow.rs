//! The scenarios of following a session (`docs/tools.md`, "Sessions for the user", "Sessions that
//! talk to each other"): the agent that starts a session for the user chooses with `follow`
//! whether it is woken at each end of a turn of that session, and two sessions that write to each
//! other without the user pause. The fake ACP agent, temporary folders, no real agent.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// The kind of the delivery that wakes a starting agent.
const WAKE: &str = "session-turn-ended";

/// A project `app` on GitHub over a throwaway repository, and a root of it in Full (nothing is judged
/// here), its first turn ended: its id and its MCP session.
fn lead(user: &User) -> (String, Mcp) {
    on_github(user);
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    eventually("its first turn", || turns(user, &id) == 1);
    (id, Mcp::initialize(&url, &authorization))
}

/// The project on GitHub (the forge of its one repository, `acme/app`), its remote a throwaway
/// bare repository.
fn on_github(user: &User) {
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
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}]}),
    )
    .unwrap();
}

/// What the fake `gh` knows: a pull request, so that `forge show` has something to read.
fn pulls(user: &User) {
    user.gh_script(json!({"pulls": {"acme/app": {"app/login": {
        "number": 12, "title": "Add the login page", "state": "OPEN", "isDraft": false,
        "baseRefName": "main", "url": "https://github.com/acme/app/pull/12",
        "statusCheckRollup": [], "comments": [], "reviews": []}}}}));
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// How many turns of the agent ended.
fn turns(user: &User, agent: &str) -> usize {
    store(user)
        .last_agent_events(agent, "agent.turn_ended", 200)
        .unwrap()
        .len()
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

fn wakes(user: &User, agent: &str) -> Vec<String> {
    delivered(user, agent, WAKE)
}

/// A pause long enough for a wake that was going to come to have come.
fn settle() {
    std::thread::sleep(Duration::from_millis(600));
}

/// What the lead gets when it starts a session titled `title` with this `follow` (none: the field
/// is left out): the answer, and the session's id and MCP session once it started.
fn launch(
    user: &User,
    mcp: &Mcp,
    title: &str,
    brief: &str,
    follow: Option<bool>,
) -> (Answer, Option<(String, Mcp)>) {
    let opened = user.received_requests("session/new").len();
    let mut arguments = json!({"title": title, "brief": brief, "root": true});
    if let Some(follow) = follow {
        arguments["follow"] = json!(follow);
    }
    let answer = mcp.call("agent_launch", arguments);
    if answer.error {
        return (answer, None);
    }
    let (url, authorization) = server_of(user, opened);
    let id = agents(user)
        .into_iter()
        .find(|a| a["title"] == title)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (answer, Some((id, Mcp::initialize(&url, &authorization))))
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

/// A followed session whose first turn ended and woke its starter: the lead's id and MCP session,
/// the session's id and MCP session.
fn followed(user: &User, brief: &str) -> (String, Mcp, String, Mcp) {
    let (lead, mcp) = lead(user);
    let (answer, session) = launch(user, &mcp, "Login", brief, Some(true));
    assert!(!answer.error, "{}", answer.text);
    let (session, second) = session.unwrap();
    eventually("the first wake", || wakes(user, &lead).len() == 1);
    (lead, mcp, session, second)
}

fn subscriptions(user: &User, agent: &str) -> Vec<Value> {
    user.call("subscription.list", json!({})).unwrap()["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["agent"] == agent)
        .cloned()
        .collect()
}

fn inbox(user: &User) -> Vec<Value> {
    user.call("inbox.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_root_launch_without_follow_is_refused_naming_the_field() {
    let user = User::with_fake_agent();
    let (_, mcp) = lead(&user);

    let (answer, started) = launch(&user, &mcp, "Login", "Fix the login.", None);

    assert!(answer.error, "{}", answer.text);
    assert!(started.is_none());
    for word in ["refused", "follow", "true", "false", "last message"] {
        assert!(answer.text.contains(word), "{word}: {}", answer.text);
    }
    assert!(agents(&user).iter().all(|a| a["title"] != "Login"));
}

#[test]
fn a_child_launch_does_not_take_follow() {
    let user = User::with_fake_agent();
    let (_, mcp) = lead(&user);

    let answer = mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests.", "follow": true}),
    );

    assert!(answer.error, "{}", answer.text);
    // Not the unknown field it was before following: the refusal says what follow is for.
    assert!(
        answer.text.contains("session for the user"),
        "{}",
        answer.text
    );
    assert!(agents(&user).iter().all(|a| a["title"] != "worker"));
}

#[test]
fn a_followed_session_wakes_its_starter_at_each_turn_end_with_its_last_message() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [{"say": "Should the logout be covered too?"}]},
        {"when": "Yes, cover it.", "do": [{"say": "Logout covered."}]}
    ]}));
    let (lead, _, session, _) = followed(&user, "Fix the login.");

    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "\"Login\"",
        session.as_str(),
        "turn done",
        "Should the logout be covered too?",
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
    user.call(
        "agent.send",
        json!({"agent": session, "message": "Yes, cover it."}),
    )
    .unwrap();
    eventually("the second wake", || wakes(&user, &lead).len() == 2);
    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("Logout covered."), "{woken}");
    assert!(!woken.contains("Should the logout"), "{woken}");
}

#[test]
fn a_long_last_message_is_cut_and_the_cut_is_said() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [{"say": "word ".repeat(2_000)}]}
    ]}));

    let (lead, _, _, _) = followed(&user, "Fix the login.");

    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.chars().count() < 4_000, "{}", woken.chars().count());
    assert!(woken.contains("cut"), "{woken}");
}

#[test]
fn a_turn_that_ends_with_a_question_or_a_report_says_so() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Ask it.", "do": [
            {"mcp": {"tool": "ask_user", "arguments": {"question": "Commit now?"}}},
            {"say": "I asked."}]},
        {"when": "Report it.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "All done."}}},
            {"say": "Reported."}]}
    ]}));
    let (lead, mcp) = lead(&user);
    let (_, asking) = launch(&user, &mcp, "Asking", "Ask it.", Some(true));
    let (_, reporting) = launch(&user, &mcp, "Reporting", "Report it.", Some(true));
    assert!(asking.is_some() && reporting.is_some());

    eventually("both wakes", || wakes(&user, &lead).len() == 2);

    let woken = wakes(&user, &lead).join("\n---\n");
    assert!(woken.contains("asked the user"), "{woken}");
    assert!(woken.contains("reported"), "{woken}");
}

#[test]
fn an_unfollowed_session_wakes_nobody() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);

    let (answer, session) = launch(&user, &mcp, "Login", "Fix the login.", Some(false));

    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("You follow it"), "{}", answer.text);
    let (session, _) = session.unwrap();
    eventually("the session's turn", || turns(&user, &session) == 1);
    settle();
    assert!(wakes(&user, &lead).is_empty());
    assert!(subscriptions(&user, &lead).is_empty());
}

#[test]
fn the_starter_answers_a_followed_session_with_session_send() {
    let user = User::with_fake_agent();
    let (lead, mcp, session, _) = followed(&user, "Fix the login.");

    let sent = mcp.call(
        "session_send",
        json!({"session": session, "text": "The brief says: no push."}),
    );

    assert!(!sent.error, "{}", sent.text);
    eventually("the answer in the session", || {
        delivered(&user, &session, "session")
            .iter()
            .any(|body| body.contains("The brief says: no push."))
    });
    let body = delivered(&user, &session, "session").remove(0);
    assert!(
        body.starts_with(&format!("agent {lead} wrote to this session:")),
        "{body}"
    );
    // The turn the answer started is followed too.
    eventually("the wake of the answered turn", || {
        wakes(&user, &lead).len() == 2
    });
}

#[test]
fn the_following_shows_in_the_starters_subscriptions_and_in_show() {
    let user = User::with_fake_agent().with_fake_gh();
    pulls(&user);
    let (lead, mcp, session, _) = followed(&user, "Fix the login.");

    let listed = subscriptions(&user, &lead);

    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["source"], "session");
    let named = listed[0]["named"].as_str().unwrap();
    assert!(
        named.contains("Login") && named.contains(&session),
        "{named}"
    );
    let shown = mcp.call("forge", json!({"action": "show", "number": 12}));
    assert!(shown.text.contains("Login"), "{}", shown.text);
    assert!(shown.text.contains(&session), "{}", shown.text);
}

#[test]
fn unsubscribe_ends_the_following() {
    let user = User::with_fake_agent().with_fake_gh();
    pulls(&user);
    let (lead, mcp, session, _) = followed(&user, "Fix the login.");

    let ended = mcp.call(
        "forge",
        json!({"action": "unsubscribe", "session": session}),
    );

    assert!(!ended.error, "{}", ended.text);
    assert!(subscriptions(&user, &lead).is_empty());
    user.call("agent.send", json!({"agent": session, "message": "Go on."}))
        .unwrap();
    eventually("the session's next turn", || turns(&user, &session) == 2);
    settle();
    assert_eq!(wakes(&user, &lead).len(), 1);
    let again = mcp.call(
        "forge",
        json!({"action": "unsubscribe", "session": session}),
    );
    assert!(again.error, "{}", again.text);
}

#[test]
fn the_following_ends_with_the_session() {
    let user = User::with_fake_agent();
    let (lead, _, session, _) = followed(&user, "Fix the login.");

    user.call("agent.stop", json!({"agent": session})).unwrap();

    eventually("the following over", || {
        subscriptions(&user, &lead).is_empty()
    });
    eventually("the wake that says it ended", || {
        wakes(&user, &lead).len() == 2
    });
    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("Your following of it is over"), "{woken}");
    assert!(!woken.contains("ended a turn"), "{woken}");
}

#[test]
fn the_following_ends_with_the_starter() {
    let user = User::with_fake_agent();
    let (lead, _, _, _) = followed(&user, "Fix the login.");

    user.call("agent.stop", json!({"agent": lead})).unwrap();

    eventually("the following over", || {
        subscriptions(&user, &lead).is_empty()
    });
}

/// Both sides write once more: the lead to the session when `to_session`, else the session to the
/// lead. It returns once the receiver's turn ended.
fn write(user: &User, from: &Mcp, to: &str, text: &str) {
    let before = turns(user, to);
    let sent = from.call("session_send", json!({"session": to, "text": text}));
    assert!(!sent.error, "{}", sent.text);
    eventually("the receiver's turn", || turns(user, to) > before);
}

#[test]
fn five_exchanges_without_the_user_pause_the_following_and_say_so() {
    let user = User::with_fake_agent();
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");

    write(&user, &mcp, &session, "one");
    eventually("the wake of the first", || wakes(&user, &lead).len() == 2);
    write(&user, &second, &lead, "two");
    write(&user, &mcp, &session, "three");
    eventually("the wake of the third", || wakes(&user, &lead).len() == 3);
    write(&user, &second, &lead, "four");
    assert!(inbox(&user).iter().all(|e| e["kind"] != "paused"));
    write(&user, &mcp, &session, "five");

    eventually("the inbox says it", || {
        inbox(&user).iter().any(|e| e["kind"] == "paused")
    });
    settle();
    assert_eq!(wakes(&user, &lead).len(), 3, "a paused following woke");
    let entry = inbox(&user)
        .into_iter()
        .find(|e| e["kind"] == "paused")
        .unwrap();
    let text = entry["text"].as_str().unwrap();
    assert!(text.contains(&session) && text.contains(&lead), "{text}");
    assert!(text.contains("without you"), "{text}");
    assert!(text.contains("message"), "{text}");
}

#[test]
fn a_user_message_resumes_a_paused_following() {
    let user = User::with_fake_agent();
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    write(&user, &mcp, &session, "one");
    write(&user, &second, &lead, "two");
    write(&user, &mcp, &session, "three");
    write(&user, &second, &lead, "four");
    write(&user, &mcp, &session, "five");
    eventually("the inbox says it", || {
        inbox(&user).iter().any(|e| e["kind"] == "paused")
    });
    settle();
    let paused = wakes(&user, &lead).len();

    user.call("agent.send", json!({"agent": session, "message": "Go on."}))
        .unwrap();

    eventually("a wake again", || wakes(&user, &lead).len() == paused + 1);
    // The count starts again: three more exchanges do not pause it.
    write(&user, &mcp, &session, "six");
    write(&user, &second, &lead, "seven");
    write(&user, &mcp, &session, "eight");
    eventually("a wake again", || wakes(&user, &lead).len() == paused + 3);
}

#[test]
fn the_descriptions_say_where_a_question_goes() {
    let user = User::with_fake_agent();
    let (lead, mcp, session, _) = followed(&user, "Fix the login.");
    let opened = user.received_requests("session/new").len();
    let child = mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!child.error, "{}", child.text);
    let (url, authorization) = server_of(&user, opened);
    let worker = Mcp::initialize(&url, &authorization);
    let description = |tools: &[Value], name: &str| -> String {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no tool {name}"))["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let tools = mcp.tools();
    for name in ["ask_user", "report", "session_send"] {
        let said = description(&tools, name);
        assert!(said.chars().count() < 2_048, "{name}: {said}");
        assert!(
            name == "ask_user" || said.contains("ask_user"),
            "{name}: {said}"
        );
        assert!(
            name == "session_send" || said.contains("session_send"),
            "{name}: {said}"
        );
        assert!(said.contains("follow-up"), "{name}: {said}");
    }
    let launched = delivered(&user, &session, "launched").remove(0);
    assert!(launched.contains("ask_user"), "{launched}");
    assert!(
        launched.contains(&format!("session_send {{\"session\": \"{lead}\"")),
        "{launched}"
    );
    assert!(launched.contains("follow-up"), "{launched}");
    assert!(launched.contains("Fix the login."), "{launched}");
    // The same for a child, with ask_parent instead of ask_user.
    let tools = worker.tools();
    for name in ["ask_parent", "report"] {
        let said = description(&tools, name);
        assert!(said.chars().count() < 2_048, "{name}: {said}");
        assert!(
            name == "ask_parent" || said.contains("ask_parent"),
            "{name}: {said}"
        );
        assert!(said.contains("follow-up"), "{name}: {said}");
    }
}

/// The five exchanges that pause the following of the session by the lead.
fn pause(user: &User, mcp: &Mcp, session: &str, second: &Mcp, lead: &str) {
    write(user, mcp, session, "one");
    write(user, second, lead, "two");
    write(user, mcp, session, "three");
    write(user, second, lead, "four");
    write(user, mcp, session, "five");
    eventually("the inbox says it", || {
        inbox(user).iter().any(|e| e["kind"] == "paused")
    });
}

/// What a `session_send` of `from` to `to` is answered, the receiver's turns unchanged.
fn refused_write(user: &User, from: &Mcp, to: &str) -> String {
    let before = turns(user, to);
    let sent = from.call("session_send", json!({"session": to, "text": "again"}));
    assert!(sent.error, "{}", sent.text);
    settle();
    assert_eq!(turns(user, to), before, "a refused message started a turn");
    sent.text
}

#[test]
fn a_session_send_between_a_paused_pair_is_refused_naming_why() {
    let user = User::with_fake_agent();
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    pause(&user, &mcp, &session, &second, &lead);

    for refused in [
        refused_write(&user, &mcp, &session),
        refused_write(&user, &second, &lead),
    ] {
        assert!(
            refused.starts_with(
                "refused: paused: you two wrote to each other five times without the user; \
                 wait for their message"
            ),
            "{refused}"
        );
    }

    user.call("agent.send", json!({"agent": lead, "message": "Go on."}))
        .unwrap();
    eventually("the lead's turn", || turns(&user, &lead) >= 2);
    write(&user, &mcp, &session, "six");
    write(&user, &second, &lead, "seven");
}

#[test]
fn the_last_message_is_of_the_turn_that_ended() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [{"say": "Should the logout be covered too?"}]},
        {"when": "Report it.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "All done."}}}]}
    ]}));
    let (lead, _, session, _) = followed(&user, "Fix the login.");

    user.call(
        "agent.send",
        json!({"agent": session, "message": "Report it."}),
    )
    .unwrap();

    eventually("the second wake", || wakes(&user, &lead).len() == 2);
    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("it reported"), "{woken}");
    assert!(woken.contains("(no message this turn)"), "{woken}");
    assert!(!woken.contains("Should the logout"), "{woken}");
}

#[test]
fn it_asked_the_user_only_in_the_turn_that_asked() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Ask it.", "do": [
            {"mcp": {"tool": "ask_user", "arguments": {"question": "Commit now?"}}},
            {"say": "I asked."}]},
        {"when": "The brief says", "do": [{"say": "Understood."}]}
    ]}));
    let (lead, mcp, session, _) = followed(&user, "Ask it.");
    assert!(wakes(&user, &lead)[0].contains("asked the user"));

    write(&user, &mcp, &session, "The brief says: no push.");

    eventually("the second wake", || wakes(&user, &lead).len() == 2);
    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("turn done"), "{woken}");
    assert!(!woken.contains("asked the user"), "{woken}");
}

#[test]
fn a_turn_that_wrote_to_its_starter_does_not_wake_it_again() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [
            {"mcp": {"tool": "session_send",
                "arguments": {"session": lead, "text": "Which branch?"}}},
            {"say": "I asked my starter."}]}
    ]}));

    let (answer, session) = launch(&user, &mcp, "Login", "Fix the login.", Some(true));

    assert!(!answer.error, "{}", answer.text);
    let (session, _) = session.unwrap();
    eventually("the session's turn", || turns(&user, &session) == 1);
    eventually("its message to the lead", || {
        delivered(&user, &lead, "session")
            .iter()
            .any(|body| body.contains("Which branch?"))
    });
    settle();
    assert!(wakes(&user, &lead).is_empty(), "{:?}", wakes(&user, &lead));
}

#[test]
fn a_secret_in_the_last_message_is_masked() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [
            {"say": "The token is ghp_fakefakefakefakefakefakefakefake0001 here."}]}
    ]}));

    let (lead, _, _, _) = followed(&user, "Fix the login.");

    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("The token is"), "{woken}");
    assert!(!woken.contains("ghp_fake"), "{woken}");
}

#[test]
fn the_pause_and_the_count_survive_a_restart() {
    let user = User::with_fake_agent();
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    pause(&user, &mcp, &session, &second, &lead);

    assert!(user.run(&["daemon", "stop"]).status.success());

    let listed = subscriptions(&user, &lead);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["paused"], true, "{listed:?}");
    let kept = store(&user).subscriptions().unwrap();
    assert_eq!(kept[0].seen["exchanges"], 5, "{:?}", kept[0].seen);
}

#[test]
fn an_agent_cannot_resume_a_paused_following() {
    let user = User::with_fake_agent();
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    pause(&user, &mcp, &session, &second, &lead);

    refused_write(&user, &mcp, &session);
    refused_write(&user, &second, &lead);
    for tools in [mcp.tools(), second.tools()] {
        assert!(
            tools
                .iter()
                .all(|t| !t["name"].as_str().unwrap().contains("resume")),
            "{tools:?}"
        );
    }
    assert_eq!(subscriptions(&user, &lead)[0]["paused"], true);

    // The user's Resume does what a message of theirs does, and the inbox says it resumed.
    let id = subscriptions(&user, &lead)[0]["id"].clone();
    user.call("subscription.resume", json!({"subscription": id}))
        .unwrap();
    assert_eq!(subscriptions(&user, &lead)[0]["paused"], false);
    let entry = inbox(&user)
        .into_iter()
        .find(|e| e["kind"] == "paused")
        .unwrap();
    assert_eq!(entry["resolved"], true, "{entry}");
    assert!(
        entry["text"].as_str().unwrap().contains("resumed since"),
        "{entry}"
    );
    write(&user, &mcp, &session, "six");
    assert!(
        user.call("subscription.resume", json!({"subscription": id}))
            .is_err(),
        "a following that is not paused is not resumed"
    );
}

#[test]
fn a_following_holds_neither_the_inactivity_sweep_nor_waiting_for() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (lead, _, _, _) = followed(&user, "Fix the login.");

    let tree = user.call("agent.tree", json!({"agent": lead})).unwrap();
    let entry = &tree["agents"][0];
    assert_eq!(entry["agent"]["id"], lead.as_str(), "{tree}");
    assert!(entry["waiting_on"].is_null(), "{tree}");
    assert!(
        entry["doing"].as_str().unwrap().starts_with("idle for"),
        "{tree}"
    );
    user.call("sessions.set", json!({"days": 3})).unwrap();
    user.call("test.advance", json!({"ms": 10 * 24 * 3_600_000u64}))
        .unwrap();

    eventually("the lead ended as inactive", || {
        user.call("agent.show", json!({"agent": lead})).unwrap()["agent"]["state"] == "ended"
    });
}
