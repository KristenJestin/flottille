//! What an agent runs on, in the window (#786): a child's row in the rail and the row of a led
//! session under its pilot say its model and its effort, by name, and a change of either fades in
//! as the words of a row do. On GPUI's test platform in front of a real daemon whose agents are
//! the fake ACP agent, with a throwaway user. No window opens on a screen.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::Mcp;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::{session_id, session_runs_id};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A fake agent whose models carry a name of their own, as a provider's list names them.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "model_rows": {
            "fake-small": {"name": "Fake Small 5.5"},
            "fake-large": {"name": "Fake Large 5.5"}
        },
        "efforts": ["low", "medium", "high"],
        "turns": [{"when": "HANG", "do": [{"hang": true}]}]
    }));
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

/// The project `app` over a throwaway repository with a remote and a `.env`, its ready
/// workspace `w`, and a lead the user started there, its first turn ended: the lead's id and its
/// MCP session.
fn lead(user: &User) -> (String, Mcp) {
    let folder: PathBuf = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "TOKEN=made-up-made-up\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
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
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[[recipe]]\ncopy = [\".env\"]\n");
    std::fs::write(&file, text).unwrap();
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let mcp = mcp_session(user, |prompt| prompt.contains("You are the root"));
    eventually("the lead's first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, mcp)
}

/// The MCP session of the last agent session opened whose instructions `which` accepts.
fn mcp_session(user: &User, which: impl Fn(&str) -> bool) -> Mcp {
    let found = || {
        user.received_requests("session/new")
            .into_iter()
            .rev()
            .find(|opened| {
                which(
                    opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"][2]
                        .as_str()
                        .unwrap_or_default(),
                )
            })
    };
    eventually("the session", || found().is_some());
    let server = found().unwrap()["params"]["mcpServers"][0].clone();
    Mcp::initialize(
        server["url"].as_str().unwrap(),
        server["headers"][0]["value"].as_str().unwrap(),
    )
}

fn open_session<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(lead), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(&shell, cx, "the session's page", |cx| {
        session_page(&shell, cx).read_with(cx, |page, cx| {
            !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
        })
    });
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the link sent, then draws what changed.
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

/// Lets every animation end.
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

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

/// What `agent_launch` of `arguments` made: the new agent's id.
fn launch(lead: &Mcp, arguments: serde_json::Value) -> String {
    let answer = lead.call("agent_launch", arguments);
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_owned()
}

/// Frames the clock needs to bring the words to rest after a change.
fn frames_to_rest(cx: &mut VisualTestContext) -> usize {
    let mut frames = 0;
    for _ in 0..60 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            break;
        }
        frames += 1;
    }
    frames
}

#[gpui_kit::test]
fn a_child_row_says_its_model_and_effort(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let child = launch(
        &mcp,
        json!({"kind": "child", "title": "worker", "brief": "PLAIN work", "workspace": "w",
            "model": "fake-small", "effort": "low"}),
    );
    eventually("the child at rest", || {
        user.call("agent.show", json!({"agent": child})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = open_session(&user, &lead, cx);
    let row = format!("rail-runs-on-{child}");

    until(&shell, cx, "the row says what the child runs on", |cx| {
        label(row.clone(), cx).is_some_and(|said| said.starts_with("Fake Small 5.5 · low"))
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing asks for a frame while the words rest"
    );

    // The lead changes the child's effort: the row says the new one, its words fading in.
    let sent = mcp.call(
        "agent_send",
        json!({"agent_id": child, "message": "Go on.", "effort": "high"}),
    );
    assert!(!sent.error, "{}", sent.text);
    until(&shell, cx, "the row says the new effort", |cx| {
        label(row.clone(), cx).is_some_and(|said| said.starts_with("Fake Small 5.5 · high"))
    });
    let frames = frames_to_rest(cx);
    assert!(frames >= 4, "the words fade over HOVER, {frames} frames");
}

#[gpui_kit::test]
fn a_led_session_card_says_its_model_and_effort(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    user.call("agent.mode", json!({"agent": lead, "mode": "full"}))
        .unwrap();
    let led = launch(
        &mcp,
        json!({"kind": "led", "title": "Docs", "brief": "PLAIN docs",
            "model": "fake-large", "effort": "high"}),
    );
    eventually("the led session at rest", || {
        user.call("agent.show", json!({"agent": led})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = open_session(&user, &lead, cx);
    let card = session_runs_id(&led);

    until(&shell, cx, "the card says what the session runs on", |cx| {
        label(card.clone(), cx).as_deref() == Some("Fake Large 5.5 · high")
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing asks for a frame while the words rest"
    );
    // Its pilot's own row, which no one leads, says nothing of the kind.
    assert!(!has(session_runs_id(&lead), cx));

    // The user changes the effort: the card says the new one, its words fading in.
    user.call("agent.options", json!({"agent": led, "effort": "low"}))
        .unwrap();
    until(&shell, cx, "the card says the new effort", |cx| {
        label(card.clone(), cx).as_deref() == Some("Fake Large 5.5 · low")
    });
    let frames = frames_to_rest(cx);
    assert!(frames >= 4, "the words fade over HOVER, {frames} frames");
}
