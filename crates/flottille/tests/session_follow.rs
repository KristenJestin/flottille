//! The scenarios of following a session (`docs/tools.md`, "Following a session", "Sessions that
//! talk to each other"): the pilot of a led session is woken when a turn of it asks or reports,
//! and when it ends, and two sessions that write to each other without the user pause. The fake
//! ACP agent, temporary folders, no real agent.
use crate::common;

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
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
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

/// What the lead gets when it starts a session titled `title` of `kind`: the answer, and the
/// session's id and MCP session once it started.
fn launch(
    user: &User,
    mcp: &Mcp,
    title: &str,
    brief: &str,
    kind: &str,
) -> (Answer, Option<(String, Mcp)>) {
    let opened = user.received_requests("session/new").len();
    let arguments = json!({"kind": kind, "title": title, "brief": brief});
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

fn ask(question: &str) -> Value {
    json!({"mcp": {"tool": "ask", "arguments": {"question": question}}})
}

/// What the agents play: a led session asks at each turn of its own, so that each wake says it
/// asked its pilot; the lead's own turns ask nothing.
fn asking() -> Value {
    json!({"turns": [
        {"when": "Fix the login.", "do": [ask("Which scope?"), {"say": "I asked."}]},
        {"when": "wrote to this session", "do": [ask("Which scope now?"), {"say": "Asked again."}]},
        {"when": "Go on.", "do": [ask("Go on how?"), {"say": "Asked how."}]}
    ]})
}

/// A followed session whose first turn ended and woke its starter: the lead's id and MCP session,
/// the session's id and MCP session.
fn followed(user: &User, brief: &str) -> (String, Mcp, String, Mcp) {
    let (lead, mcp) = lead(user);
    let (answer, session) = launch(user, &mcp, "Login", brief, "led");
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
fn a_led_session_wakes_its_pilot_at_each_question_with_its_last_message() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [ask("Scope?"), {"say": "Should the logout be covered too?"}]},
        {"when": "Yes, cover it.", "do": [ask("Push?"), {"say": "Logout covered."}]}
    ]}));
    let (lead, _, session, _) = followed(&user, "Fix the login.");

    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "\"Login\"",
        session.as_str(),
        "asked you",
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
fn a_lead_is_woken_when_its_led_session_ends_a_turn_without_asking() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [{"say": "Login fixed, nothing left to ask."}]}
    ]}));
    let (lead, mcp) = lead(&user);

    let (answer, session) = launch(&user, &mcp, "Login", "Fix the login.", "led");

    assert!(!answer.error, "{}", answer.text);
    let (session, _) = session.unwrap();
    eventually("the session's turn", || turns(&user, &session) == 1);
    eventually("the wake", || wakes(&user, &lead).len() == 1);
    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "\"Login\"",
        session.as_str(),
        "ended a turn",
        "Login fixed, nothing left to ask.",
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
    assert!(!woken.contains("asked you"), "{woken}");
    assert!(!woken.contains("it reported"), "{woken}");
}

#[test]
fn a_lead_is_woken_once_when_the_turn_asks_and_ends() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [ask("Scope?"), {"say": "Should the logout be covered too?"}]}
    ]}));
    let (lead, _, _, _) = followed(&user, "Fix the login.");

    settle();

    let woken = wakes(&user, &lead);
    assert_eq!(woken.len(), 1, "{woken:?}");
    assert!(woken[0].contains("asked you"), "{}", woken[0]);
}

#[test]
fn a_long_last_message_is_cut_and_the_cut_is_said() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [ask("Scope?"), {"say": "word ".repeat(2_000)}]}
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
    let (_, asking) = launch(&user, &mcp, "Asking", "Ask it.", "led");
    let (_, reporting) = launch(&user, &mcp, "Reporting", "Report it.", "led");
    assert!(asking.is_some() && reporting.is_some());

    eventually("both wakes", || wakes(&user, &lead).len() == 2);

    let woken = wakes(&user, &lead).join("\n---\n");
    assert!(woken.contains("asked you"), "{woken}");
    assert!(woken.contains("reported"), "{woken}");
}

#[test]
fn an_unfollowed_session_wakes_nobody() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);

    let (answer, session) = launch(&user, &mcp, "Login", "Fix the login.", "handed");

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("handed over"), "{}", answer.text);
    let (session, _) = session.unwrap();
    eventually("the session's turn", || turns(&user, &session) == 1);
    settle();
    assert!(wakes(&user, &lead).is_empty());
    assert!(subscriptions(&user, &lead).is_empty());
}

#[test]
fn the_starter_answers_a_followed_session_with_session_send() {
    let user = User::with_fake_agent();
    user.fake_script(asking());
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
    user.fake_script(asking());
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
fn a_pilot_that_unsubscribes_with_a_question_open_hands_it_to_the_user() {
    let user = User::with_fake_agent().with_fake_gh();
    user.fake_script(asking());
    pulls(&user);
    let (_, mcp, session, _) = followed(&user, "Fix the login.");
    let questions = || -> Vec<Value> {
        inbox(&user)
            .into_iter()
            .filter(|e| e["kind"] == "question" && e["agent"] == session.as_str())
            .collect()
    };
    assert!(questions().is_empty());

    let ended = mcp.call(
        "forge",
        json!({"action": "unsubscribe", "session": session}),
    );

    assert!(!ended.error, "{}", ended.text);
    eventually("the question in the inbox", || questions().len() == 1);
    assert_eq!(questions()[0]["text"], "Which scope?");
    eventually("the session told", || {
        !delivered(&user, &session, "unfollowed").is_empty()
    });
    settle();
    let told = delivered(&user, &session, "unfollowed");
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("went to the user"), "{}", told[0]);
}

#[test]
fn unsubscribe_ends_the_following() {
    let user = User::with_fake_agent().with_fake_gh();
    user.fake_script(asking());
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
    user.fake_script(asking());
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
    user.fake_script(asking());
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

/// The exchanges after which a following pauses (`EXCHANGES` of the daemon, which a test cannot
/// reach: the daemon is a binary).
const EXCHANGES: usize = 50;

/// `count` exchanges more, starting with the session's message to the lead and alternating, so
/// that the last of an even number is the lead's: the one a pause leaves without its wake.
fn exchange(user: &User, mcp: &Mcp, session: &str, second: &Mcp, lead: &str, count: usize) {
    for i in 0..count {
        match i % 2 {
            0 => write(user, second, lead, "from the session"),
            _ => write(user, mcp, session, "from the lead"),
        }
    }
}

#[test]
fn fifty_exchanges_without_the_user_pause_the_following_and_say_so() {
    let user = User::with_fake_agent();
    user.fake_script(asking());
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");

    exchange(&user, &mcp, &session, &second, &lead, EXCHANGES - 1);
    settle();
    assert!(
        inbox(&user).iter().all(|e| e["kind"] != "paused"),
        "paused before the fiftieth"
    );
    // The wake of the first question, and one for each message of the lead's.
    eventually("the wakes", || {
        wakes(&user, &lead).len() == 1 + EXCHANGES / 2 - 1
    });
    write(&user, &mcp, &session, "the fiftieth");

    eventually("the inbox says it", || {
        inbox(&user).iter().any(|e| e["kind"] == "paused")
    });
    settle();
    assert_eq!(
        wakes(&user, &lead).len(),
        EXCHANGES / 2,
        "a paused following woke"
    );
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
    user.fake_script(asking());
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    pause(&user, &mcp, &session, &second, &lead);
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
    user.fake_script(asking());
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    let opened = user.received_requests("session/new").len();
    let child = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
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
    for name in ["ask", "report", "session_send"] {
        let said = description(&tools, name);
        assert!(said.chars().count() < 2_048, "{name}: {said}");
        assert!(name == "ask" || said.contains("ask"), "{name}: {said}");
        assert!(said.contains("follow-up"), "{name}: {said}");
    }
    let tools = second.tools();
    let ask = description(&tools, "ask");
    assert!(ask.contains("goes to the agent that leads"), "{ask}");
    assert!(ask.contains("session_send"), "{ask}");
    let launched = delivered(&user, &session, "launched").remove(0);
    assert!(launched.contains("ask"), "{launched}");
    assert!(
        launched.contains(&format!("led by agent {lead}")),
        "{launched}"
    );
    assert!(launched.contains("follow-up"), "{launched}");
    assert!(launched.contains("Fix the login."), "{launched}");
    // A child's goes to its parent.
    let tools = worker.tools();
    for name in ["ask", "report"] {
        let said = description(&tools, name);
        assert!(said.chars().count() < 2_048, "{name}: {said}");
        assert!(said.contains("follow-up"), "{name}: {said}");
    }
    assert!(description(&tools, "ask").contains("goes to your parent"));
}

/// The fifty exchanges that pause the following of the session by the lead.
fn pause(user: &User, mcp: &Mcp, session: &str, second: &Mcp, lead: &str) {
    exchange(user, mcp, session, second, lead, EXCHANGES);
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
    user.fake_script(asking());
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    pause(&user, &mcp, &session, &second, &lead);

    for refused in [
        refused_write(&user, &mcp, &session),
        refused_write(&user, &second, &lead),
    ] {
        assert!(
            refused.starts_with(
                "refused: paused: you two wrote to each other fifty times without the user; \
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
        {"when": "Fix the login.", "do": [ask("Scope?"), {"say": "Should the logout be covered too?"}]},
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
fn it_asked_you_only_in_the_turn_that_asked() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Ask it.", "do": [ask("Commit now?"), {"say": "I asked."}]},
        {"when": "The brief says", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "Done."}}},
            {"say": "Understood."}]}
    ]}));
    let (lead, mcp, session, _) = followed(&user, "Ask it.");
    assert!(wakes(&user, &lead)[0].contains("asked you"));

    write(&user, &mcp, &session, "The brief says: no push.");

    eventually("the second wake", || wakes(&user, &lead).len() == 2);
    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("it reported"), "{woken}");
    assert!(!woken.contains("asked you"), "{woken}");
    assert!(!woken.contains("Commit now?"), "{woken}");
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

    let (answer, session) = launch(&user, &mcp, "Login", "Fix the login.", "led");

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
            ask("Scope?"),
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
    user.fake_script(asking());
    let (lead, mcp, session, second) = followed(&user, "Fix the login.");
    pause(&user, &mcp, &session, &second, &lead);

    assert!(user.run(&["daemon", "stop"]).status.success());

    let listed = subscriptions(&user, &lead);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["paused"], true, "{listed:?}");
    let kept = store(&user).subscriptions().unwrap();
    assert_eq!(kept[0].seen["exchanges"], EXCHANGES, "{:?}", kept[0].seen);
}

#[test]
fn an_agent_cannot_resume_a_paused_following() {
    let user = User::with_fake_agent();
    user.fake_script(asking());
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
    user.fake_script(asking());
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

#[test]
fn unsubscribe_takes_the_followed_session_s_title() {
    let user = User::with_fake_agent().with_fake_gh();
    user.fake_script(asking());
    pulls(&user);
    let (lead, mcp, _, _) = followed(&user, "Fix the login.");

    let ended = mcp.call(
        "forge",
        json!({"action": "unsubscribe", "session": " LOGIN "}),
    );

    assert!(!ended.error, "{}", ended.text);
    assert!(subscriptions(&user, &lead).is_empty());
    let unknown = mcp.call(
        "forge",
        json!({"action": "unsubscribe", "session": "Nothing of the kind"}),
    );
    assert!(
        unknown.error
            && unknown.text.starts_with(
                "refused: no session of your project has the id or the title \"Nothing of the kind\": forge unsubscribe"
            ),
        "{}",
        unknown.text
    );
}

/// A call of the tool `tool` with `arguments`, in a turn of the fake agent.
fn call(tool: &str, arguments: Value) -> Value {
    json!({"mcp": {"tool": tool, "arguments": arguments}})
}

/// The led session asks the user for the vault entry `staging-login`.
fn vault_ask() -> Value {
    call(
        "vault",
        json!({"action": "ask", "name": "staging-login", "why": "to sign in to staging"}),
    )
}

/// The requests for a vault entry that wait for the user.
fn vault_asks(user: &User) -> Vec<Value> {
    user.call("vault.asks", json!({})).unwrap()["asks"]
        .as_array()
        .unwrap()
        .clone()
}

/// The requests of `agent` held for the user.
fn held_for_the_user(user: &User, agent: &str) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|request| request["agent"] == agent)
        .cloned()
        .collect()
}

/// The deliveries of `kind` stored for `agent`, newest first, with their state.
fn delivered_with_state(user: &User, agent: &str, kind: &str) -> Vec<(String, String)> {
    store(user)
        .last_deliveries(agent, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| (delivery.body, delivery.state))
        .collect()
}

/// The kind of the delivery that tells a lead how the user answered its led session's request.
const ANSWERED: &str = "session-request-answered";

#[test]
fn a_lead_is_woken_with_the_question_its_led_session_asks_its_number_and_its_text() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [ask("Which scope?"), {"say": "I asked."}]}
    ]}));

    let (lead, _, session, _) = followed(&user, "Fix the login.");

    let number = store(&user)
        .last_agent_events(&session, "agent.asked", 1)
        .unwrap()[0]
        .seq;
    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "asked you",
        &format!("Its question #{number} to you"),
        "Which scope?",
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
}

#[test]
fn a_lead_is_woken_with_the_vault_entry_its_led_session_asks_its_number_and_its_reason() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [vault_ask(),
            {"say": "Waiting for the user's answer on the vault entry."}]}
    ]}));

    let (lead, _, session, _) = followed(&user, "Fix the login.");

    let asks = vault_asks(&user);
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!(asks[0]["agent"], session.as_str());
    let number = asks[0]["ask"].as_u64().unwrap();
    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "ended a turn: it waits for the user.",
        "It waits for the user",
        &format!("request #{number}, a vault entry asked: staging-login: to sign in to staging"),
        "Waiting for the user's answer on the vault entry.",
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
}

#[test]
fn a_lead_is_woken_with_the_call_of_its_led_session_held_for_approval_its_number_and_its_reasons() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [{"say": "Ready."}]},
        {"when": "Read the env.", "do": [call("fs_read", json!({"path": ".env"})),
            {"say": "Waiting for the user's approval."}]}
    ]}));
    let (lead, _, session, _) = followed(&user, "Fix the login.");
    user.call("agent.mode", json!({"agent": session, "mode": "ask"}))
        .unwrap();

    user.call(
        "agent.send",
        json!({"agent": session, "message": "Read the env."}),
    )
    .unwrap();

    eventually("the second wake", || wakes(&user, &lead).len() == 2);
    let held = held_for_the_user(&user, &session);
    assert_eq!(held.len(), 1, "{held:?}");
    let number = held[0]["number"].as_u64().unwrap();
    let reason = held[0]["reasons"][0].as_str().unwrap().to_owned();
    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "ended a turn: it waits for the user.",
        &format!("request #{number}, a call held for approval: fs_read"),
        reason.as_str(),
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
}

#[test]
fn a_lead_is_woken_with_the_connection_its_led_session_asks_its_number_and_its_reason() {
    let user = User::with_fake_agent();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [
            call("connections", json!({"action": "add", "name": "notes", "about": "Notes.",
                "url": "https://notes.example.com/mcp"})),
            {"say": "Waiting for the user to accept the connection."}]}
    ]}));

    let (lead, _, session, _) = followed(&user, "Fix the login.");

    let held = held_for_the_user(&user, &session);
    assert_eq!(held.len(), 1, "{held:?}");
    let number = held[0]["number"].as_u64().unwrap();
    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "ended a turn: it waits for the user.",
        &format!("request #{number}, a connection asked"),
        "Add the connection notes",
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
}

#[test]
fn a_session_with_no_lead_wakes_nobody_with_its_requests() {
    let user = User::with_fake_agent();
    on_github(&user);
    user.fake_script(json!({"turns": [
        {"when": "hello", "do": [vault_ask(), {"say": "Waiting for you."}]}
    ]}));

    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();

    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("its turn", || turns(&user, &id) == 1);
    assert_eq!(vault_asks(&user).len(), 1);
    settle();
    for agent in agents(&user) {
        let agent = agent["id"].as_str().unwrap();
        assert!(wakes(&user, agent).is_empty(), "{agent} was woken");
        assert!(delivered(&user, agent, ANSWERED).is_empty(), "{agent}");
    }
}

#[test]
fn a_turn_that_wrote_to_its_starter_and_waits_for_the_user_wakes_it_with_the_request() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [
            call("session_send", json!({"session": lead, "text": "I need a login."})),
            vault_ask(),
            {"say": "Waiting for the vault entry."}]}
    ]}));

    let (answer, session) = launch(&user, &mcp, "Login", "Fix the login.", "led");

    assert!(!answer.error, "{}", answer.text);
    eventually("the wake", || wakes(&user, &lead).len() == 1);
    let woken = wakes(&user, &lead).remove(0);
    assert!(
        woken.contains("a vault entry asked: staging-login"),
        "{woken}"
    );
    settle();
    assert_eq!(wakes(&user, &lead).len(), 1);
    drop(session);
}

#[test]
fn each_request_is_said_once_in_the_turn_that_asked_it() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [vault_ask(), {"say": "Waiting."}]},
        {"when": "Go on.", "do": [{"say": "Still waiting."}]}
    ]}));
    let (lead, _, session, _) = followed(&user, "Fix the login.");

    user.call("agent.send", json!({"agent": session, "message": "Go on."}))
        .unwrap();

    eventually("the second wake", || wakes(&user, &lead).len() == 2);
    let woken = wakes(&user, &lead).remove(0);
    assert!(!woken.contains("staging-login"), "{woken}");
}

#[test]
fn the_lead_is_told_on_its_next_wake_when_the_user_dismisses_the_vault_entry() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [vault_ask(), {"say": "Waiting."}]}
    ]}));
    let (lead, _, _, _) = followed(&user, "Fix the login.");
    let number = vault_asks(&user)[0]["ask"].as_u64().unwrap();
    let lead_turns = turns(&user, &lead);

    user.call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();

    eventually("the lead is told", || {
        !delivered(&user, &lead, ANSWERED).is_empty()
    });
    let (body, state) = delivered_with_state(&user, &lead, ANSWERED).remove(0);
    for word in [
        "\"Login\"",
        &format!("request #{number}"),
        "staging-login",
        "dismissed",
    ] {
        assert!(body.contains(word), "{word}: {body}");
    }
    // Queued for its next wake: it does not ring the lead.
    settle();
    assert_eq!(state, "queued", "{body}");
    assert_eq!(turns(&user, &lead), lead_turns);
}

#[test]
fn the_lead_is_told_on_its_next_wake_when_the_user_denies_a_held_call() {
    let user = User::with_fake_agent();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [
            call("connections", json!({"action": "add", "name": "notes", "about": "Notes.",
                "url": "https://notes.example.com/mcp"})),
            {"say": "Waiting."}]}
    ]}));
    let (lead, _, session, _) = followed(&user, "Fix the login.");
    let number = held_for_the_user(&user, &session)[0]["number"]
        .as_u64()
        .unwrap();

    user.call("approval.deny", json!({"number": number}))
        .unwrap();

    eventually("the lead is told", || {
        !delivered(&user, &lead, ANSWERED).is_empty()
    });
    let (body, state) = delivered_with_state(&user, &lead, ANSWERED).remove(0);
    for word in ["\"Login\"", &format!("request #{number}"), "denied"] {
        assert!(body.contains(word), "{word}: {body}");
    }
    assert_eq!(state, "queued", "{body}");
}

#[test]
fn the_following_lists_what_its_session_waits_for_the_user() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [vault_ask(), {"say": "Waiting."}]}
    ]}));
    let (lead, _, _, _) = followed(&user, "Fix the login.");
    let number = vault_asks(&user)[0]["ask"].as_u64().unwrap();

    let following = subscriptions(&user, &lead).remove(0);

    assert_eq!(
        following["waits"],
        json!([{"kind": "vault", "number": number,
            "what": "a vault entry asked: staging-login: to sign in to staging"}]),
        "{following}"
    );
    user.call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();
    let following = subscriptions(&user, &lead).remove(0);
    assert_eq!(following["waits"], json!([]), "{following}");
}
