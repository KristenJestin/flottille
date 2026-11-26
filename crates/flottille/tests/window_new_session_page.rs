//! The scenarios of #1579, a new session opens its page at once: from the first message sent, the
//! window shows the session's page with its place, the message and a loading state where the
//! answer will come, and the session the daemon started takes that page's place; no page change
//! says "No project yet." while there are projects. On GPUI's test platform in front of a real
//! daemon whose agent is the fake ACP agent, through a relay that holds or refuses the answers the
//! test names, with a throwaway user. No window opens on a screen.

use crate::common;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::page::Page;
use flottille_window::session::SessionPage;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, SharedString, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// What the empty page's button is: never drawn while projects are listed.
const EMPTY_PAGE: &str = "add-first-project";

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A session the user started from the command line, its first turn ended.
fn started(user: &User, project: &str, message: &str) -> String {
    let answer = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

/// The roots the daemon has, by id.
fn roots(user: &User) -> Vec<String> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["parent"].is_null())
        .filter_map(|a| a["id"].as_str().map(str::to_owned))
        .collect()
}

/// Stands between the window and the daemon: the answers to what it holds wait until it lets
/// them through, those to what it refuses are a refusal. What is held is named by its method, and
/// a history's read (an `agent.show` with `before`) by `agent.show:before`.
#[derive(Clone, Default)]
struct Relay {
    held: Arc<Mutex<HashSet<String>>>,
    refused: Arc<Mutex<HashMap<String, String>>>,
}

impl Relay {
    fn hold(&self, what: &str) {
        self.held.lock().unwrap().insert(what.to_owned());
    }

    fn release(&self, what: &str) {
        self.held.lock().unwrap().remove(what);
    }

    fn refuse(&self, what: &str, why: Option<&str>) {
        let mut refused = self.refused.lock().unwrap();
        match why {
            Some(why) => refused.insert(what.to_owned(), why.to_owned()),
            None => refused.remove(what),
        };
    }

    fn connect(&self, user: &User) -> Connect {
        let (real, relay) = (user.connector(), self.clone());
        Arc::new(move || {
            let daemon = real()?;
            let (window_side, relay_side) = UnixStream::pair()?;
            let asked: Arc<Mutex<HashMap<String, String>>> = Arc::default();
            let (mut to_daemon, from_window) = (daemon.try_clone()?, relay_side.try_clone()?);
            let marks = asked.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(from_window).lines() {
                    let Ok(line) = line else { return };
                    let request: Value = serde_json::from_str(&line).unwrap_or_default();
                    if let Some(method) = request["method"].as_str() {
                        let what = match method == "agent.show"
                            && !request["params"]["before"].is_null()
                        {
                            true => "agent.show:before".to_owned(),
                            false => method.to_owned(),
                        };
                        marks
                            .lock()
                            .unwrap()
                            .insert(request["id"].to_string(), what);
                    }
                    if writeln!(to_daemon, "{line}").is_err() {
                        return;
                    }
                }
            });
            let (mut to_window, relay) = (relay_side, relay.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(daemon).lines() {
                    let Ok(mut line) = line else { return };
                    let answer: Value = serde_json::from_str(&line).unwrap_or_default();
                    let what = asked.lock().unwrap().remove(&answer["id"].to_string());
                    if let Some(what) = &what {
                        let deadline = Instant::now() + Duration::from_secs(60);
                        while relay.held.lock().unwrap().contains(what) && Instant::now() < deadline
                        {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        if let Some(why) = relay.refused.lock().unwrap().get(what) {
                            line = json!({"id": answer["id"],
                                "error": {"code": "refused", "message": why}})
                            .to_string();
                        }
                    }
                    if writeln!(to_window, "{line}").is_err() {
                        return;
                    }
                }
            });
            Ok(window_side)
        })
    }
}

fn window<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(800.)));
    (shell, cx)
}

fn present(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(cx: &mut VisualTestContext, id: &'static str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// One frame of the window, as the platform draws them, time passing: what the links sent, then
/// a frame. No frame of it ever says there is no project.
fn frame(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert!(
        !present(cx, EMPTY_PAGE),
        "a frame says there is no project while there are, on {:?}",
        shell.read_with(cx, |s, cx| s.main().read(cx).page().clone())
    );
}

/// Frames until `check` holds, the clock moving as real time does.
fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    const STEP: Duration = Duration::from_millis(25);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        frame(shell, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(STEP);
        cx.executor().advance_clock(STEP);
    }
}

/// A few frames, a little time passing between them.
fn frames(shell: &Entity<Shell>, cx: &mut VisualTestContext, count: usize) {
    for _ in 0..count {
        frame(shell, cx);
        cx.executor().advance_clock(Duration::from_millis(16));
    }
}

fn type_task(text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input(text);
}

fn open_project(shell: &Entity<Shell>, project: &str, cx: &mut VisualTestContext) {
    until(shell, cx, "the project is listed", |cx| {
        present(cx, flottille_window::tree::project_id(project))
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project(project.into()), window, cx)
    });
    until(shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == project)
        })
    });
    frames(shell, cx, 30);
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<SessionPage>> {
    shell.read_with(cx, |s, cx| {
        matches!(s.main().read(cx).page(), Page::Session { .. })
            .then(|| s.main().read(cx).session_page().cloned())
            .flatten()
    })
}

fn page_id(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    session_page(shell, cx).map(|page| page.read_with(cx, |page, _| page.id().to_owned()))
}

/// The user's messages the page's conversation holds.
fn messages(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<String> {
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::Message { from, text, .. } if from == "you" => Some(text.clone()),
                _ => None,
            })
            .collect()
    })
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

/// The page of the session being started, as the user sees it before the daemon answers: its
/// place in the header, the message, and the answer's loading state.
fn says_it_starts(cx: &mut VisualTestContext, message: &str) {
    assert_eq!(label(cx, "session-title").as_deref(), Some(message));
    assert_eq!(label(cx, "session-place").as_deref(), Some("main"));
    assert_eq!(
        label(cx, "session-starting-message").as_deref(),
        Some(message)
    );
    assert!(
        present(cx, "session-starting-answer"),
        "the answer's loading state is where the answer will come"
    );
}

#[gpui_kit::test]
fn a_new_session_opens_its_page_before_the_daemon_answers(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let relay = Relay::default();
    relay.hold("agent.start");
    let (shell, cx) = window(&user, relay.connect(&user), cx);
    open_project(&shell, "atlas", cx);

    type_task("Fix the login", cx);
    cx.update(|window, cx| window.click("send", cx));
    frames(&shell, cx, 2);
    assert!(
        session_page(&shell, cx).is_some(),
        "the session's page is shown the moment the message is sent: {:?}",
        shell.read_with(cx, |s, cx| s.main().read(cx).page().clone())
    );
    says_it_starts(cx, "Fix the login");

    // However long the daemon takes, the page stays, and says it waits.
    frames(&shell, cx, 60);
    assert!(session_page(&shell, cx).is_some());
    says_it_starts(cx, "Fix the login");
    assert!(
        roots(&user).len() == 1,
        "the daemon started it, the window was not told yet"
    );
    relay.release("agent.start");
}

#[gpui_kit::test]
fn a_started_session_takes_the_place_of_its_placeholder(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let relay = Relay::default();
    relay.hold("agent.start");
    let (shell, cx) = window(&user, relay.connect(&user), cx);
    open_project(&shell, "atlas", cx);
    type_task("Fix the login", cx);
    cx.update(|window, cx| window.click("send", cx));
    until(&shell, cx, "the page of the session being started", |cx| {
        session_page(&shell, cx).is_some()
    });
    let placeholder = session_page(&shell, cx).unwrap();
    eventually("the daemon started it", || roots(&user).len() == 1);
    let agent = roots(&user).remove(0);

    relay.release("agent.start");
    until(&shell, cx, "the started session is the page's", |cx| {
        page_id(&shell, cx).as_deref() == Some(agent.as_str())
    });
    assert_eq!(
        session_page(&shell, cx).unwrap(),
        placeholder,
        "the same page carries on: no second page"
    );
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(agent.clone()))
    );
    until(&shell, cx, "its conversation shows the message", |cx| {
        !messages(&placeholder, cx).is_empty() && !present(cx, "session-starting-message")
    });
    assert_eq!(
        messages(&placeholder, cx),
        vec!["Fix the login".to_owned()],
        "the message once"
    );
    // The page stays the session's while the list catches up, and once it has.
    frames(&shell, cx, 40);
    assert_eq!(session_page(&shell, cx).unwrap(), placeholder);
    assert!(!present(cx, "session-starting-answer"));
    assert_eq!(label(cx, "session-title").as_deref(), Some("Fix the login"));
}

#[gpui_kit::test]
fn a_session_that_fails_to_start_says_so_on_its_page(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let relay = Relay::default();
    relay.refuse("agent.start", Some("made-up failure"));
    let (shell, cx) = window(&user, relay.connect(&user), cx);
    open_project(&shell, "atlas", cx);
    type_task("Try the broken start", cx);
    cx.update(|window, cx| window.click("send", cx));

    until(
        &shell,
        cx,
        "the page says the session did not start",
        |cx| present(cx, "session-not-started"),
    );
    let said = label(cx, "session-not-started").unwrap();
    assert!(said.contains("made-up failure"), "{said}");
    assert!(session_page(&shell, cx).is_some(), "the page stays");
    assert_eq!(
        label(cx, "session-starting-message").as_deref(),
        Some("Try the broken start"),
        "the message stays"
    );
    assert!(
        toasts(&shell, cx).contains(&"The session did not start".to_owned()),
        "{:?}",
        toasts(&shell, cx)
    );
    let composer = shell.read_with(cx, |s, _| s.composer_of("atlas").cloned().unwrap());
    assert_eq!(
        composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string()),
        "Try the broken start",
        "kept to send again"
    );

    // Sent again from the page, it starts, there.
    relay.refuse("agent.start", None);
    let placeholder = session_page(&shell, cx).unwrap();
    cx.update(|window, cx| window.click("session-start-again", cx));
    until(&shell, cx, "the session started", |cx| {
        page_id(&shell, cx).is_some_and(|id| roots(&user).contains(&id))
    });
    assert_eq!(session_page(&shell, cx).unwrap(), placeholder);
    until(&shell, cx, "the failure folded away", |cx| {
        !present(cx, "session-not-started")
    });
}

#[gpui_kit::test]
fn the_empty_page_is_only_said_when_there_is_no_project(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let first = started(&user, "atlas", "Read the code");
    let relay = Relay::default();
    let (shell, cx) = window(&user, relay.connect(&user), cx);

    // Opening a session.
    until(&shell, cx, "the session is listed", |cx| {
        present(cx, session_id(&first))
    });
    cx.update(|window, cx| window.click(session_id(&first), cx));
    until(&shell, cx, "the session's page", |cx| {
        page_id(&shell, cx).as_deref() == Some(first.as_str())
    });
    frames(&shell, cx, 20);

    // Opening a project.
    open_project(&shell, "atlas", cx);

    // A new session, its answer in before the list has it.
    relay.hold("agent.list");
    type_task("Fix the login", cx);
    cx.update(|window, cx| window.click("send", cx));
    eventually("the daemon started it", || roots(&user).len() == 2);
    let second = roots(&user).into_iter().find(|id| *id != first).unwrap();
    until(&shell, cx, "the started session's page", |cx| {
        page_id(&shell, cx).as_deref() == Some(second.as_str())
    });
    frames(&shell, cx, 40);
    relay.release("agent.list");
    until(&shell, cx, "the list has it", |cx| {
        present(cx, session_id(&second))
    });
    frames(&shell, cx, 20);
    assert_eq!(page_id(&shell, cx).as_deref(), Some(second.as_str()));

    // And back to the first session, through the project.
    open_project(&shell, "atlas", cx);
    cx.update(|window, cx| window.click(session_id(&first), cx));
    until(&shell, cx, "the first session's page again", |cx| {
        page_id(&shell, cx).as_deref() == Some(first.as_str())
    });
    frames(&shell, cx, 20);
}

#[gpui_kit::test]
fn opening_a_session_shows_its_page_while_it_loads(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let session = started(&user, "atlas", "Read the code");
    let relay = Relay::default();
    let (shell, cx) = window(&user, relay.connect(&user), cx);
    until(&shell, cx, "the session is listed", |cx| {
        present(cx, session_id(&session))
    });
    open_project(&shell, "atlas", cx);

    relay.hold("agent.show:before");
    cx.update(|window, cx| window.click(session_id(&session), cx));
    frames(&shell, cx, 1);
    assert_eq!(
        page_id(&shell, cx).as_deref(),
        Some(session.as_str()),
        "the session's page is shown at once"
    );
    assert_eq!(label(cx, "session-title").as_deref(), Some("Read the code"));
    // Past the delay of a fast open, its conversation says it is being read.
    until(&shell, cx, "the conversation's loading state", |cx| {
        present(
            cx,
            SharedString::from(format!("conversation-skeleton-{session}")),
        )
    });
    assert!(!present(
        cx,
        SharedString::from(format!("conversation-empty-{session}"))
    ));

    relay.release("agent.show:before");
    let page = session_page(&shell, cx).unwrap();
    until(&shell, cx, "its conversation is read", |cx| {
        !messages(&page, cx).is_empty()
    });
    until(&shell, cx, "the loading state faded out", |cx| {
        !present(
            cx,
            SharedString::from(format!("conversation-skeleton-{session}")),
        )
    });
}
