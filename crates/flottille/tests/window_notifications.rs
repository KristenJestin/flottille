//! The scenarios of #185, the shell, the rail and the notifications: a command to allow is shown
//! whole, the context's share sits on its gauge's line, a session an agent started is said as a
//! session, the sidebar no longer says who started a session, a pull request a session opened
//! marks its row and rings the bell, and each notification says why, cut cleanly. On GPUI's test
//! platform in front of a real daemon whose agents are the fake ACP agent and whose `gh` is the
//! fake one, with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::{Block, STARTED_SESSION_KIND};
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::shell::{bell_line_id, bell_session_id};
use flottille_window::tree::{Trailing, session_id, session_mark_id, session_unread_id};
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A command longer than any card is wide.
const LONG: &str = "./deploy.sh --target staging-eu-west --tag release-2026-10-31 \
    --notify ops@example.invalid --confirm-every-step --keep-the-old-version-for-a-week";

/// The title of the lead's session, longer than the bell is wide.
const BRIEF: &str = "lead the work on the login page and the logout button, then the docs of both";

fn user() -> User {
    let user = User::with_fake_agent().with_fake_gh();
    user.fake_script(json!({"turns": [
        {"when": "lead the work", "do": [
            {"usage": {"used": 82_000, "size": 200_000}}, {"say": "On it."}]},
        {"when": "tell me more", "do": [{"say": "There is more."}]},
        {"when": "start the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "root": true,
                "workspace": "login"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The project `app` over a throwaway repository whose `origin` says GitHub's `acme/app`, its
/// forge declared, and its ready workspace `login`, made offline.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
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
    let settings = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    let repositories: Vec<Value> = settings["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({"path": r["path"], "forge": r["proposal"]}))
        .collect();
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": repositories}),
    )
    .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
}

/// A session the user started in `login`, its first turn ended: its id and its MCP session.
fn session(user: &User, message: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("the first turn", || state_of(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

fn lead(user: &User) -> (String, Mcp) {
    project(user);
    session(user, BRIEF)
}

/// The number of the request a held call answered with.
fn held_number(answer: Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    let rest = answer
        .text
        .split_once("request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
        .1
        .to_owned();
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn pending(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(800.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn rect(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    let id = id.into();
    cx.update(|window, _| window.find(id).bounds())
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Every label the window says, as text.
fn every_label(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| {
        window
            .elements()
            .into_iter()
            .filter_map(|element| element.label().map(str::to_owned))
            .collect()
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn open_session(shell: &Entity<Shell>, agent: &str, cx: &mut VisualTestContext) {
    until(shell, cx, "the session is in the tree", |cx| {
        has(session_id(agent), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
    until(
        shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            session_page(shell, cx).read_with(cx, |page, cx| {
                page.id() == agent
                    && !page.conversation().read(cx).rows().is_empty()
                    && !page.tree().is_empty()
            })
        },
    );
    settle(cx);
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

fn bell_count(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> usize {
    shell.read_with(cx, |s, cx| s.top().read(cx).count())
}

fn bell_said(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<(String, String, String)> {
    shell.read_with(cx, |s, cx| s.bell_list().read(cx).said())
}

#[gpui_kit::test]
fn a_long_command_is_whole_in_its_waiting_card(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &lead, cx);

    let number = held_number(mcp.call("run", json!({"line": LONG, "timeout": 5})));
    until(&shell, cx, "the card is in the rail", |cx| {
        has(format!("allow-{number}"), cx)
    });
    settle(cx);

    let call = format!("approval-call-{number}");
    assert_eq!(label(call.clone(), cx).as_deref(), Some(LONG));
    let line = rect(call.clone(), cx);
    let card = rect(format!("approval-{number}"), cx);
    assert!(
        line.size.height > px(28.),
        "the command wraps over several lines: {line:?}"
    );
    assert!(
        line.right() <= card.right() && line.origin.x >= card.origin.x,
        "and stays inside its card: {line:?} in {card:?}"
    );
}

#[gpui_kit::test]
fn the_gauge_s_percentage_is_on_its_line(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &lead, cx);
    until(&shell, cx, "the gauge on the lead's row", |cx| {
        has(format!("rail-context-{lead}"), cx)
    });
    settle(cx);

    let gauge = rect(format!("rail-context-{lead}"), cx);
    assert_eq!(
        label(format!("rail-context-{lead}"), cx).as_deref(),
        Some("41% of the context used")
    );
    assert!(
        gauge.size.height < px(16.),
        "the share and its bar on one line: {gauge:?}"
    );
    assert!(gauge.size.width > px(40.), "side by side: {gauge:?}");
    // The session's own agent says what it runs on, and nothing under it.
    assert!(has(format!("rail-runs-on-{lead}"), cx));
    assert!(!has(format!("rail-role-{lead}"), cx));
    assert!(
        !every_label(cx)
            .iter()
            .any(|said| said.starts_with("the conversation")),
        "no line says the conversation"
    );
}

/// The lead starts the docs session for the user, who allows it: the session's id.
fn docs_session(user: &User, lead: &str) -> String {
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "start the docs session"}),
    )
    .unwrap();
    eventually("the launch is held", || !pending(user).is_empty());
    let number = pending(user)[0]["number"].as_u64().unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let launched = || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["launched_by"] == lead)
            .and_then(|agent| agent["id"].as_str().map(str::to_owned))
    };
    eventually("the session exists", || launched().is_some());
    let id = launched().unwrap();
    eventually("its first turn ends", || state_of(user, &id) == "idle");
    eventually("the lead read the answer", || {
        state_of(user, lead) == "idle"
    });
    id
}

#[gpui_kit::test]
fn a_session_started_by_an_agent_is_said_as_a_session(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    docs_session(&user, &lead);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the session is said", |cx| {
        blocks(&page, cx).iter().any(|b| {
            matches!(b, Block::Notice { kind, text } if kind == STARTED_SESSION_KIND && text == "Docs")
        })
    });
    assert!(
        !blocks(&page, cx).contains(&Block::SubAgents),
        "no sub-agents list: {:?}",
        blocks(&page, cx)
    );
    assert!(
        !every_label(cx)
            .iter()
            .any(|said| said.contains("launched by")),
        "nothing says it was launched as a sub-agent"
    );
}

#[gpui_kit::test]
fn no_started_by_in_the_sidebar(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    let docs = docs_session(&user, &lead);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(&docs), cx)
    });
    settle(cx);
    assert!(
        !every_label(cx)
            .iter()
            .any(|said| said.starts_with("started by")),
        "the sidebar says nobody started a session"
    );

    // Its own page says it, in its header.
    open_session(&shell, &docs, cx);
    let by = label("session-started-by", cx).unwrap();
    assert!(by.starts_with("started by "), "{by}");
    let header = rect("session-header", cx);
    let said = rect("session-started-by", cx);
    assert!(
        said.origin.y >= header.origin.y && said.bottom() <= header.bottom(),
        "in the header: {said:?} in {header:?}"
    );
}

#[gpui_kit::test]
fn a_pull_request_opened_by_a_session_marks_its_row_and_notifies(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (other, _) = session(&user, "look at the other thing");
    user.gh_script(json!({"created": 12}));
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &other, cx);
    assert!(!has(session_unread_id(&lead), cx), "nothing changed yet");

    let create = json!({"action": "create", "repository": ".", "title": "Add the login page",
        "body": "What it does"});
    let number = held_number(mcp.call("forge", create));
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    until(&shell, cx, "the lead's row is marked", |cx| {
        shell.read_with(cx, |s, cx| {
            s.session(&lead)
                .is_some_and(|row| row.read(cx).unread().is_some())
        })
    });
    settle(cx);

    assert_eq!(
        label(session_mark_id(&lead), cx).as_deref(),
        Some("New since you looked: opened a pull request")
    );
    assert!(
        !has(session_unread_id(&other), cx),
        "no mark on another session"
    );
    let row = rect(session_id(&lead), cx);
    let mark = rect(session_unread_id(&lead), cx);
    assert!(
        mark.origin.y >= row.origin.y && mark.bottom() <= row.bottom(),
        "on the session's own row: {mark:?} in {row:?}"
    );
    assert_eq!(bell_count(&shell, cx), 1);
    let said = bell_said(&shell, cx);
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].0, "Opened a pull request");
    assert_eq!(said[0].1, "Add the login page");
    assert_eq!(said[0].2, BRIEF);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "the mark is still once it came"
    );

    // Opened, the session's change is seen: the mark folds away and the bell has nothing.
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the bell is empty", |cx| {
        bell_count(&shell, cx) == 0
    });
    cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(
        has(session_unread_id(&lead), cx),
        "it folds away, drawn until it has"
    );
    settle(cx);
    assert!(!has(session_unread_id(&lead), cx));
}

#[gpui_kit::test]
fn a_notification_says_why_and_is_cut_cleanly(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the bell counts the request", |cx| {
        bell_count(&shell, cx) == 1
    });
    let said = bell_said(&shell, cx);
    assert_eq!(
        said,
        [(
            format!("Request #{number} waits for you"),
            "Read .env".to_owned(),
            BRIEF.to_owned()
        )]
    );

    cx.update(|window, cx| window.click("bell", cx));
    settle(cx);
    let key = format!("request-{number}");
    let list = rect("bell-list", cx);
    for id in [bell_line_id(&key), bell_session_id(&key)] {
        let line = rect(id.clone(), cx);
        assert!(
            line.right() <= list.right() && line.size.height < px(24.),
            "{id:?} is one line inside the bell's list: {line:?} in {list:?}"
        );
    }
    assert_eq!(label(bell_session_id(&key), cx).as_deref(), Some(BRIEF));
    let _ = lead;
}

fn trailing_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<Trailing> {
    shell.read_with(cx, |s, cx| s.session(id).map(|row| row.read(cx).trailing()))
}

fn unread_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |s, cx| {
        s.session(id)
            .and_then(|row| row.read(cx).unread().map(str::to_owned))
    })
}

/// The pointer rests on a session's mark until its tooltip has come: what it says.
fn words_of_mark(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    // The row offers its "⋯" under the pointer and the mark makes room for it: the pointer comes
    // to the row first, then to the mark where it is by then.
    cx.update(|window, cx| window.hover(session_id(id), cx));
    settle(cx);
    cx.update(|window, cx| window.hover(session_mark_id(id), cx));
    settle(cx);
    settle(cx);
    let words = label(format!("{}-tooltip", session_mark_id(id)), cx);
    cx.simulate_mouse_move(
        gpui_kit::point(px(1430.), px(790.)),
        None,
        Default::default(),
    );
    settle(cx);
    words
}

/// The lead opened a pull request while the user looked at `other`: the lead's row holds news.
fn lead_with_news(
    user: &User,
    shell: &Entity<Shell>,
    lead: &str,
    mcp: &Mcp,
    cx: &mut VisualTestContext,
) {
    let create = json!({"action": "create", "repository": ".", "title": "Add the login page",
        "body": "What it does"});
    let number = held_number(mcp.call("forge", create));
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    until(shell, cx, "the lead's row holds news", |cx| {
        unread_of(shell, lead, cx).is_some()
    });
    settle(cx);
}

#[gpui_kit::test]
fn a_session_that_needs_the_user_and_has_news_shows_the_state_mark_alone(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (other, _) = session(&user, "look at the other thing");
    user.gh_script(json!({"created": 12}));
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &other, cx);
    lead_with_news(&user, &shell, &lead, &mcp, cx);
    assert_eq!(trailing_of(&shell, &lead, cx), Some(Trailing::Unread));
    assert!(has(session_unread_id(&lead), cx));

    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the lead needs the user", |cx| {
        trailing_of(&shell, &lead, cx) == Some(Trailing::State(AgentState::NeedsYou))
    });
    // The dot folds away as the state mark comes: both are drawn on the way, one at rest.
    assert!(
        has(session_unread_id(&lead), cx),
        "the dot is still drawn on the way"
    );
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the swap has frames in between"
    );
    settle(cx);
    assert!(!has(session_unread_id(&lead), cx), "the state mark alone");
    assert_eq!(
        label(session_mark_id(&lead), cx).as_deref(),
        Some("Needs you: a permission")
    );
    assert!(
        unread_of(&shell, &lead, cx).is_some(),
        "the news is kept for when the state goes"
    );

    // The request answered, the state goes and the dot comes back, with frames in between.
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    until(&shell, cx, "the state goes", |cx| {
        trailing_of(&shell, &lead, cx) == Some(Trailing::Unread)
    });
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the swap back has frames in between"
    );
    settle(cx);
    assert!(has(session_unread_id(&lead), cx));
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing is redrawn at rest"
    );
}

#[gpui_kit::test]
fn an_idle_session_with_a_new_message_shows_the_unread_dot_alone_and_says_why_on_hover(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, _) = lead(&user);
    let (other, _) = session(&user, "look at the other thing");
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &lead, cx);
    assert_eq!(trailing_of(&shell, &other, cx), Some(Trailing::Ago));

    // What an agent says in the session in sight is in sight: no dot.
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "tell me more"}),
    )
    .unwrap();
    user.call(
        "agent.send",
        json!({"agent": other, "message": "tell me more"}),
    )
    .unwrap();
    until(&shell, cx, "the other session holds news", |cx| {
        unread_of(&shell, &other, cx).is_some()
    });
    eventually("the lead's turn ends", || state_of(&user, &lead) == "idle");
    eventually("the other's turn ends", || {
        state_of(&user, &other) == "idle"
    });
    until(&shell, cx, "both rest", |cx| {
        trailing_of(&shell, &other, cx) == Some(Trailing::Unread)
    });
    settle(cx);

    assert_eq!(trailing_of(&shell, &lead, cx), Some(Trailing::Ago));
    assert!(!has(session_unread_id(&lead), cx));
    assert!(has(session_unread_id(&other), cx));
    assert_eq!(
        trailing_of(&shell, &other, cx),
        Some(Trailing::Unread),
        "the dot, not the time ago and no state"
    );
    assert_eq!(
        words_of_mark(&other, cx).as_deref(),
        Some("New since you looked: a new message")
    );
    assert!(
        label(session_mark_id(&lead), cx).is_none_or(|said| said.is_empty()),
        "the time ago has no words"
    );
}

#[gpui_kit::test]
fn opening_the_session_clears_the_unread_dot(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (other, _) = session(&user, "look at the other thing");
    user.gh_script(json!({"created": 12}));
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &other, cx);
    lead_with_news(&user, &shell, &lead, &mcp, cx);

    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the news is read", |cx| {
        unread_of(&shell, &lead, cx).is_none()
    });
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "it folds away, it does not jump"
    );
    assert!(has(session_unread_id(&lead), cx), "drawn until it has");
    settle(cx);
    assert!(!has(session_unread_id(&lead), cx));
    assert_eq!(trailing_of(&shell, &lead, cx), Some(Trailing::Ago));
    // Leaving it, nothing comes back by itself.
    cx.update(|window, cx| window.click(session_id(&other), cx));
    settle(cx);
    assert_eq!(unread_of(&shell, &lead, cx), None);
}
