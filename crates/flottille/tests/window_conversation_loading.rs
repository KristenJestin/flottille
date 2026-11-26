//! The scenarios of #1531, a conversation that loads: until the first page of its history has
//! arrived it shows a skeleton, never "Nothing said yet.", which it says only once the history is
//! read and empty; a history that cannot be read says so with "Try again"; a long conversation
//! opens on one page. On GPUI's test platform in front of a real daemon whose agent is the fake
//! ACP agent, with a throwaway user. No window opens on a screen.

use crate::common;

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// How many messages the long conversation holds, each said in a few pieces, as an agent streams.
const MESSAGES: usize = 3000;

/// Every how many messages a tool call with a long output comes.
const CALL_EVERY: usize = 5;

fn script() -> Value {
    json!({"turns": [
        {"when": "write a lot", "do": a_lot()},
    ]})
}

/// Thousands of messages, each in four pieces, a call with a long output between them.
fn a_lot() -> Vec<Value> {
    let output: String = (0..2000)
        .map(|n| format!("line {n}: the build compiled the module and its tests passed\n"))
        .collect();
    let mut steps = Vec::new();
    for n in 0..MESSAGES {
        let id = format!("m{n}");
        for piece in [
            "Message ",
            &n.to_string(),
            ": the work goes on, ",
            "step by step.\n",
        ] {
            steps.push(json!({"say": piece, "message": id}));
        }
        if n % CALL_EVERY == 0 {
            let call = format!("c{n}");
            steps.push(
                json!({"update": {"sessionUpdate": "tool_call", "toolCallId": call,
                "title": "mcp__flottille__fs_read", "kind": "other", "status": "in_progress",
                "rawInput": {"path": "build.log"}}}),
            );
            steps.push(
                json!({"update": {"sessionUpdate": "tool_call_update", "toolCallId": call,
                "status": "completed", "rawOutput": {"text": output}}}),
            );
        }
    }
    steps.push(json!({"say": "Finally.", "message": "last"}));
    steps
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(script());
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn eventually(what: &str, within: Duration, check: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
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

/// A session the user started in the project `app`, its first turn ended.
fn lead(user: &User) -> String {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the lead's first turn", Duration::from_secs(20), || {
        state_of(user, &id) == "idle"
    });
    id
}

/// The lead, once it wrote the long conversation.
fn long_lead(user: &User) -> String {
    let lead = lead(user);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "write a lot"}),
    )
    .unwrap();
    eventually("the long turn ends", Duration::from_secs(300), || {
        user.call("agent.show", json!({"agent": lead, "before": u64::MAX}))
            .unwrap()["events"]
            .as_array()
            .unwrap()
            .last()
            .is_some_and(|e| e["name"] == "agent.turn_ended")
    });
    lead
}

fn window_through<'a>(
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

/// One turn of the window: what the links sent, then a frame.
fn turn(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        turn(shell, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<SessionPage>> {
    shell.read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
}

fn histories(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> usize {
    session_page(shell, cx).map_or(0, |page| page.read_with(cx, |page, _| page.histories()))
}

/// The times of opening the long conversation (the pull request gives them before and after):
/// what the daemon takes to read a page, and what the window takes from the click to the rows.
#[gpui_kit::test]
fn opening_a_long_conversation_reads_one_page(cx: &mut TestAppContext) {
    let user = user();
    let made = Instant::now();
    let lead = long_lead(&user);
    eprintln!("measure: fixture written in {:?}", made.elapsed());

    let mut size = 0;
    for _ in 0..3 {
        let asked = Instant::now();
        let shown = user
            .call("agent.show", json!({"agent": lead, "before": u64::MAX}))
            .unwrap();
        size = shown.to_string().len();
        eprintln!(
            "measure: agent.show (one page) {:?}, {} events, {size} bytes, more {}",
            asked.elapsed(),
            shown["events"].as_array().unwrap().len(),
            shown["more"]
        );
        assert_eq!(shown["more"], true);
    }
    // The page the window opens on is a page whatever the outputs it holds: a thousand events
    // of them were ten megabytes.
    assert!(size < 1_500_000, "a page of {size} bytes");

    let proxy = Proxy::new(History::Through);
    let connect = proxy.connect(&user);
    let (shell, cx) = window_through(&user, connect, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    let clicked = Instant::now();
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let before = histories(&shell, cx);
        let polled = Instant::now();
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        let poll = polled.elapsed();
        let framed = Instant::now();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let frame = framed.elapsed();
        if histories(&shell, cx) > before {
            eprintln!(
                "measure: history arrived {:?} after the click; its rebuild {poll:?}, its first frame {frame:?}",
                clicked.elapsed() - poll - frame,
            );
            break;
        }
        assert!(Instant::now() < deadline, "the history never came");
        std::thread::sleep(Duration::from_millis(5));
    }
    let page = session_page(&shell, cx).unwrap();
    let (blocks, pages) = page.read_with(cx, |page, cx| {
        (page.conversation().read(cx).blocks().len(), page.pages())
    });
    let framed = Instant::now();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    eprintln!(
        "measure: {blocks} blocks made, {pages} page asked; a frame at rest {:?}; total {:?}",
        framed.elapsed(),
        clicked.elapsed()
    );
    assert_eq!(pages, 1, "opening reads one page");
    assert_eq!(proxy.asked(), 1, "and asks the daemon for one");
}

/// What the window's connections answer to its read of a conversation's history (an
/// `agent.show` with `before`): the daemon's answer, held until let through, an error, or an
/// empty history. Every other request goes to the daemon as it is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum History {
    Through,
    Held,
    Unreadable,
    Empty,
}

/// The error the daemon is made to answer a history it cannot read.
const UNREADABLE: &str = "the store cannot be read";

/// Stands between the window and the daemon, line by line, and answers its reads of a history
/// as [`History`] says.
#[derive(Clone)]
struct Proxy {
    mode: Arc<(Mutex<History>, Condvar)>,
    /// How many reads of a history the window asked.
    asked: Arc<AtomicUsize>,
}

impl Proxy {
    fn new(mode: History) -> Proxy {
        Proxy {
            mode: Arc::new((Mutex::new(mode), Condvar::new())),
            asked: Arc::default(),
        }
    }

    fn set(&self, mode: History) {
        *self.mode.0.lock().unwrap() = mode;
        self.mode.1.notify_all();
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }

    fn connect(&self, user: &User) -> Connect {
        let (real, proxy) = (user.connector(), self.clone());
        Arc::new(move || {
            let daemon = real()?;
            let (window, relay) = UnixStream::pair()?;
            let to_window = Arc::new(Mutex::new(relay.try_clone()?));
            let (from_daemon, mut to_daemon) = (daemon.try_clone()?, daemon);
            let back = to_window.clone();
            std::thread::spawn(move || {
                let mut lines = BufReader::new(from_daemon);
                let mut line = Vec::new();
                while matches!(lines.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                    if back.lock().unwrap().write_all(&line).is_err() {
                        break;
                    }
                    line.clear();
                }
                let _ = back.lock().unwrap().shutdown(Shutdown::Write);
            });
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                let mut lines = BufReader::new(relay);
                let mut line = Vec::new();
                while matches!(lines.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                    let asked: Value = serde_json::from_slice(&line).unwrap_or_default();
                    if asked["method"] == "agent.show" && !asked["params"]["before"].is_null() {
                        proxy.asked.fetch_add(1, Ordering::SeqCst);
                        let (mode, changed) = &*proxy.mode;
                        let mut mode = mode.lock().unwrap();
                        while *mode == History::Held {
                            mode = changed.wait(mode).unwrap();
                        }
                        let answer = match *mode {
                            History::Unreadable => Some(json!({"id": asked["id"],
                                "error": {"code": "internal", "message": UNREADABLE}})),
                            History::Empty => Some(json!({"id": asked["id"],
                                "result": {"agent": {}, "events": [], "more": false}})),
                            _ => None,
                        };
                        drop(mode);
                        if let Some(answer) = answer {
                            let said = format!("{answer}\n");
                            if to_window
                                .lock()
                                .unwrap()
                                .write_all(said.as_bytes())
                                .is_err()
                            {
                                break;
                            }
                            line.clear();
                            continue;
                        }
                    }
                    if to_daemon.write_all(&line).is_err() {
                        break;
                    }
                    line.clear();
                }
                let _ = to_daemon.shutdown(Shutdown::Write);
            });
            Ok(window)
        })
    }
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Lets every animation end, the window's clock going on.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn skeleton(lead: &str) -> String {
    format!("conversation-skeleton-{lead}")
}

fn empty(lead: &str) -> String {
    format!("conversation-empty-{lead}")
}

fn rows(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> usize {
    session_page(shell, cx).map_or(0, |page| {
        page.read_with(cx, |page, cx| page.conversation().read(cx).blocks().len())
    })
}

/// The window, its connections through `proxy`, once the user clicked the session `lead`.
fn opened<'a>(
    user: &User,
    lead: &str,
    proxy: &Proxy,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window_through(user, proxy.connect(user), cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(lead), cx));
    (shell, cx)
}

#[gpui_kit::test]
fn a_conversation_loading_shows_its_skeleton_not_nothing_said_yet(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let proxy = Proxy::new(History::Held);
    let (shell, cx) = opened(&user, &lead, &proxy, cx);
    until(&shell, cx, "the history is asked", |_| proxy.asked() > 0);
    settle(cx);

    assert!(present(cx, &skeleton(&lead)), "the conversation loads");
    assert_eq!(
        label(cx, &skeleton(&lead)).as_deref(),
        Some("Reading the conversation…")
    );
    assert!(
        !present(cx, &empty(&lead)),
        "nothing says that nothing was said while it is read"
    );

    proxy.set(History::Through);
    until(&shell, cx, "the messages", |cx| rows(&shell, cx) > 0);
    // The messages are drawn under the skeleton, which fades out over them.
    assert!(
        present(cx, &skeleton(&lead)),
        "it fades, it does not vanish"
    );
    settle(cx);
    assert!(!present(cx, &skeleton(&lead)), "gone once it faded");
    assert!(!present(cx, &empty(&lead)));
}

#[gpui_kit::test]
fn nothing_said_yet_only_once_the_history_is_empty(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let proxy = Proxy::new(History::Held);
    let (shell, cx) = opened(&user, &lead, &proxy, cx);
    until(&shell, cx, "the history is asked", |_| proxy.asked() > 0);
    settle(cx);
    assert!(!present(cx, &empty(&lead)), "not before it is read");

    proxy.set(History::Empty);
    until(&shell, cx, "the history is read", |cx| {
        histories(&shell, cx) > 0
    });
    settle(cx);
    assert_eq!(
        label(cx, &empty(&lead)).as_deref(),
        Some("Nothing said yet.")
    );
    assert!(!present(cx, &skeleton(&lead)));
}

#[gpui_kit::test]
fn a_fast_open_shows_no_skeleton_flash(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let proxy = Proxy::new(History::Through);
    let (shell, cx) = opened(&user, &lead, &proxy, cx);
    // The history comes before the window's clock moved: within the delay of the skeleton.
    let deadline = Instant::now() + Duration::from_secs(60);
    while rows(&shell, cx) == 0 {
        turn(&shell, cx);
        assert!(
            !present(cx, &skeleton(&lead)),
            "no skeleton for a fast open"
        );
        assert!(Instant::now() < deadline, "the messages never came");
        std::thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        assert!(!present(cx, &skeleton(&lead)), "nor once its delay passed");
    }
}

#[gpui_kit::test]
fn a_history_that_cannot_be_read_says_so_with_try_again(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let proxy = Proxy::new(History::Unreadable);
    let (shell, cx) = opened(&user, &lead, &proxy, cx);
    let unread = format!("conversation-unread-{lead}");
    until(
        &shell,
        cx,
        "the conversation says it cannot be read",
        |cx| present(cx, &unread),
    );
    settle(cx);
    let said = label(cx, &unread).unwrap_or_default();
    assert!(said.contains(UNREADABLE), "{said}");
    assert!(!present(cx, &empty(&lead)), "never the empty state");
    assert!(!present(cx, &skeleton(&lead)), "nor the skeleton");
    let again = format!("conversation-try-again-{lead}");
    assert_eq!(label(cx, &again).as_deref(), Some("Try again"));

    proxy.set(History::Through);
    let asked = proxy.asked();
    cx.update(|window, cx| window.click(again, cx));
    until(&shell, cx, "the messages", |cx| rows(&shell, cx) > 0);
    assert!(proxy.asked() > asked, "it was read again");
    settle(cx);
    assert!(!present(cx, &unread), "the error folded away");
}

#[gpui_kit::test]
fn a_session_opened_again_shows_what_the_window_holds_at_once(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let other = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "another thing"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the other's first turn", Duration::from_secs(20), || {
        state_of(&user, &other) == "idle"
    });
    let proxy = Proxy::new(History::Through);
    let (shell, cx) = opened(&user, &lead, &proxy, cx);
    until(&shell, cx, "the lead's messages", |cx| rows(&shell, cx) > 0);
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&other), cx));
    until(&shell, cx, "the other's messages", |cx| {
        session_page(&shell, cx).is_some_and(|p| p.read_with(cx, |p, _| p.id() == other))
            && rows(&shell, cx) > 0
    });
    settle(cx);

    // Whatever the daemon is slow to answer now, the lead's conversation is held here.
    proxy.set(History::Held);
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    turn(&shell, cx);
    let page = session_page(&shell, cx).unwrap();
    assert_eq!(page.read_with(cx, |p, _| p.id().to_owned()), lead);
    assert!(rows(&shell, cx) > 0, "its messages are shown at once");
    assert!(!present(cx, &skeleton(&lead)));
    proxy.set(History::Through);
}
