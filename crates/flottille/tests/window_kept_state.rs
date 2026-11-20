//! The scenarios of #1070: what the user leaves in the window (the text and the pictures of a
//! composer, what the sidebar has folded) survives a close of the window, a second window and a
//! restart of the daemon, because the daemon keeps it. On GPUI's test platform in front of a real
//! daemon whose agents are the fake ACP agent, with a throwaway user: no window opens on a
//! screen, and the 500 ms of the debounce are the test platform's clock, never a sleep.

use crate::common;

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::tree::{ended_id, project_id, session_draft_id, session_id};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Image, ImageFormat, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A valid 8 × 8 PNG, so that the window can draw its thumbnail.
const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR4nGM4YWODFTEMLQkAZZlQAVIPr1MAAAAASUVORK5CYII=";

/// The debounce of a draft: the daemon is written to this long after the last change.
const DEBOUNCE: Duration = Duration::from_millis(500);

fn png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(PNG_BASE64)
        .unwrap()
}

/// A user whose fake agent takes images, answers at once and, asked to begin the docs session,
/// launches a session it leads.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true, "turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Starts a session and waits until its first turn ended.
fn start(user: &User, project: &str, message: &str) -> String {
    let id = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

/// A lead the user started and the session it leads.
fn lead_and_led(user: &User, project: &str) -> (String, String) {
    let lead = user
        .call(
            "agent.start",
            json!({"project": project, "message": "begin the docs session"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the launch is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let number = user.call("approval.list", json!({})).unwrap()["requests"][0]["number"]
        .as_u64()
        .unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the session exists", || {
        agents(user).iter().any(|a| a["led_by"] == lead.as_str())
    });
    let led = agents(user)
        .into_iter()
        .find(|a| a["led_by"] == lead.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, led)
}

/// What the daemon keeps of a composer's draft: its text and how many pictures, `None` for none.
fn kept(user: &User, kind: &str, target: &str) -> Option<(String, usize)> {
    let answer = user
        .call("draft.get", json!({"kind": kind, "target": target}))
        .unwrap();
    let draft = answer["draft"].as_object()?;
    Some((
        draft["text"].as_str().unwrap().to_owned(),
        draft["images"].as_array().map_or(0, Vec::len),
    ))
}

/// What the daemon keeps of the sidebar.
fn sidebar(user: &User) -> Value {
    user.call("window.get", json!({})).unwrap()["state"]["sidebar"].clone()
}

/// A window on the user's daemon: a second one is a second app, as a second window of the machine
/// is.
fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    window_via(user, Arc::new(user.connector()), cx)
}

/// A daemon that stops answering: [`Silence::fall`] ends the connections the window holds and
/// refuses the ones it makes next, until [`Silence::rise`]. What the window asks of it fails, as
/// it does when the daemon is restarting.
#[derive(Clone, Default)]
struct Silence {
    down: Arc<AtomicBool>,
    held: Arc<Mutex<Vec<UnixStream>>>,
}

impl Silence {
    fn connect(&self, user: &User) -> Connect {
        self.around(Arc::new(user.connector()))
    }

    /// The connections `inner` makes, held by this silence.
    fn around(&self, inner: Connect) -> Connect {
        let silence = self.clone();
        Arc::new(move || {
            if silence.down.load(Ordering::SeqCst) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "the daemon does not answer",
                ));
            }
            let stream = inner()?;
            // Under the lock `fall` takes: a connection is held before the silence falls (and
            // ended by it) or sees it fallen (and is refused), never made in between and alive.
            let mut held = silence.held.lock().unwrap();
            if silence.down.load(Ordering::SeqCst) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "the daemon does not answer",
                ));
            }
            held.push(stream.try_clone()?);
            Ok(stream)
        })
    }

    fn fall(&self) {
        let mut held = self.held.lock().unwrap();
        self.down.store(true, Ordering::SeqCst);
        for stream in held.drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    fn rise(&self) {
        self.down.store(false, Ordering::SeqCst);
    }
}

/// The silence falls while a connection is being made: that connection is ended too, or a
/// window asking the daemon at that very moment is answered by a daemon that "does not answer".
#[test]
fn a_connection_being_made_when_the_silence_falls_is_ended_too() {
    use std::io::Read as _;
    use std::sync::mpsc::channel;

    let (inside, entered) = channel();
    let (go, released) = channel::<()>();
    let (made, _peer) = UnixStream::pair().unwrap();
    let (made, released) = (Mutex::new(Some(made)), Mutex::new(released));
    let slow: Connect = Arc::new(move || {
        inside.send(()).ok();
        released.lock().unwrap().recv().ok();
        Ok(made.lock().unwrap().take().expect("connected once"))
    });
    let silence = Silence::default();
    let connect = silence.around(slow);
    let connecting = std::thread::spawn(move || connect());

    entered.recv().unwrap();
    silence.fall();
    go.send(()).unwrap();
    // Refused, or ended: never a live connection.
    if let Ok(mut stream) = connecting.join().unwrap() {
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            stream.read(&mut byte).ok(),
            Some(0),
            "a connection made while the daemon fell is still alive"
        );
    }
}

fn window_via<'a>(
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
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

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

/// [`until`], the test platform's clock moving `tick` at each try: what the window asks again
/// after a wait is asked again.
fn until_later(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    tick: Duration,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    until(shell, cx, what, |cx| {
        cx.executor().advance_clock(tick);
        check(cx)
    });
}

/// The clock and the frames of a window at rest, to the last frame of any motion.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Opens a session from its row and waits for its composer, without waiting for what the daemon
/// kept of it.
fn open_unread(id: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the session's row", |cx| {
        cx.update(|window, _| window.try_find(session_id(id)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the session's composer is made", |cx| {
        shell.read_with(cx, |s, _| s.reply_of(id).is_some())
    });
    settle(cx);
}

/// [`open_unread`], then waits until the daemon answered the read of the draft: what a test types
/// next is typed in a composer that knows what the daemon holds.
fn open(id: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    open_unread(id, shell, cx);
    until(
        shell,
        cx,
        "the daemon answered the read of the draft",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.reply_of(id).is_some_and(|r| r.read(cx).draft_was_read())
            })
        },
    );
}

/// A window on the project's page, its composer made.
fn open_project(name: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the project's row", |cx| {
        cx.update(|window, _| window.try_find(project_id(name)).is_some())
    });
    cx.update(|window, cx| window.click(project_id(name), cx));
    until(shell, cx, "the project's composer is made", |cx| {
        shell.read_with(cx, |s, _| s.composer_of(name).is_some())
    });
    until(
        shell,
        cx,
        "the daemon answered the read of the draft",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.composer_of(name)
                    .is_some_and(|c| c.read(cx).draft_was_read())
            })
        },
    );
    settle(cx);
}

fn type_in_reply(id: &str, text: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    open(id, shell, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input(text);
}

fn reply_text(id: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) -> String {
    let reply = shell.read_with(cx, |s, _| s.reply_of(id).cloned().expect("a composer"));
    reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string())
}

/// The window is closed: what it holds is written at once.
fn close(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update(cx, |shell, cx| shell.closing(cx));
}

/// The 500 ms the window waits after the last change, on the test platform's clock.
fn debounced(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(DEBOUNCE + Duration::from_millis(1));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_draft_survives_closing_and_reopening_the_window(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");

    let (shell, cx) = window(&user, first);
    type_in_reply(&id, "half an answer", &shell, cx);
    close(&shell, cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id).is_some()
    });
    assert_eq!(
        kept(&user, "session", &id),
        Some(("half an answer".to_owned(), 0))
    );

    let (shell, cx) = window(&user, second);
    open(&id, &shell, cx);
    until(&shell, cx, "the draft is back in its composer", |cx| {
        reply_text(&id, &shell, cx) == "half an answer"
    });
}

#[gpui_kit::test]
fn a_draft_is_written_500_ms_after_the_last_change(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    open(&id, &shell, cx);
    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    let sent = |reply: &Entity<_>, cx: &mut VisualTestContext| {
        reply.read_with(cx, |r: &flottille_window::session::Reply, _| {
            r.drafts_sent()
        })
    };

    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("one");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(sent(&reply, cx), 0, "nothing is written before the 500 ms");

    // Another change starts the wait again: 400 ms after it is not 500 ms after the last one.
    cx.simulate_input(" two");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(sent(&reply, cx), 0, "a change restarts the wait");

    debounced(cx);
    assert_eq!(
        sent(&reply, cx),
        1,
        "one write, 500 ms after the last change"
    );
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id) == Some(("one two".to_owned(), 0))
    });

    // Nothing changed: nothing is written again.
    debounced(cx);
    assert_eq!(sent(&reply, cx), 1);
}

#[gpui_kit::test]
fn a_draft_with_a_picture_comes_back_with_its_picture(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");

    let (shell, cx) = window(&user, first);
    type_in_reply(&id, "look at this ", &shell, cx);
    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    reply.update(cx, |reply, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png()), cx)
    });
    settle(cx);
    close(&shell, cx);
    eventually("the daemon keeps the picture", || {
        kept(&user, "session", &id).is_some_and(|(_, pictures)| pictures == 1)
    });
    // The picture is a file of the daemon's state folder, private, and not the agent's.
    let folder = user.path("state/flottille/drafts");
    assert_eq!(common::mode(folder.clone()) & 0o777, 0o700);

    let (shell, cx) = window(&user, second);
    open(&id, &shell, cx);
    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    until(&shell, cx, "the picture is back", |cx| {
        reply.read_with(cx, |r, _| r.attachments().len() == 1)
    });
    assert!(
        reply_text(&id, &shell, cx).starts_with("look at this "),
        "its text comes back with it"
    );
    assert_eq!(
        reply.read_with(cx, |r, _| r.attachments()[0].bytes.clone()),
        png()
    );
}

#[gpui_kit::test]
fn the_first_message_of_a_new_session_survives_the_window(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    project(&user, "other");

    let (shell, cx) = window(&user, first);
    open_project("app", &shell, cx);
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input("fix the login page");
    close(&shell, cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "project", "app").is_some()
    });
    assert_eq!(
        kept(&user, "project", "app"),
        Some(("fix the login page".to_owned(), 0))
    );
    assert_eq!(kept(&user, "project", "other"), None, "it is app's alone");

    let (shell, cx) = window(&user, second);
    open_project("app", &shell, cx);
    let composer = shell.read_with(cx, |s, _| s.composer_of("app").cloned().unwrap());
    until(&shell, cx, "the draft is back in its composer", |cx| {
        composer.read_with(cx, |c, cx| {
            c.field().read(cx).value() == "fix the login page"
        })
    });
}

#[gpui_kit::test]
fn sending_the_message_removes_its_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    type_in_reply(&id, "ready to go", &shell, cx);
    debounced(cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id).is_some()
    });

    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the message is sent", |cx| {
        reply.read_with(cx, |r, cx| {
            !r.is_sending() && r.field().read(cx).value().is_empty()
        })
    });
    // At once, not 500 ms later: a close right after sending must not bring the sent text back.
    eventually("the daemon forgets the draft", || {
        kept(&user, "session", &id).is_none()
    });
}

#[gpui_kit::test]
fn emptying_the_composer_removes_its_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    type_in_reply(&id, "oops", &shell, cx);
    debounced(cx);
    eventually("the daemon keeps the draft", || {
        kept(&user, "session", &id).is_some()
    });

    for _ in 0..4 {
        cx.simulate_keystrokes("backspace");
    }
    debounced(cx);
    eventually("the daemon forgets the draft", || {
        kept(&user, "session", &id).is_none()
    });
}

#[gpui_kit::test]
fn a_draft_survives_a_restart_of_the_daemon(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "kept across a restart"}),
    )
    .unwrap();

    user.call("daemon.stop", json!({})).unwrap();
    eventually("the daemon is gone", || {
        !user.path("runtime/flottille/daemon.sock").exists()
    });

    // The window starts the daemon again, which reads its store.
    let (shell, cx) = window(&user, cx);
    open(&id, &shell, cx);
    until(&shell, cx, "the draft is back in its composer", |cx| {
        reply_text(&id, &shell, cx) == "kept across a restart"
    });
}

#[gpui_kit::test]
fn a_draft_is_never_logged_nor_in_an_event(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let watch = user.watch(&[]);
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "my private words"}),
    )
    .unwrap();
    user.call("draft.get", json!({"kind": "session", "target": id}))
        .unwrap();
    // An event that follows the write is read after it: the draft's would have come before.
    user.call("project.list", json!({})).unwrap();
    let seen = watch.within(Duration::from_millis(500));
    assert!(
        !serde_json::to_string(&seen)
            .unwrap()
            .contains("my private words"),
        "no event carries a draft"
    );
    let log = std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap_or_default();
    assert!(!log.contains("my private words"), "the log holds no draft");
    let _ = cx;
}

/// The daemon's answer to `draft.set` on a session: the code of its refusal, `None` when it kept
/// the draft.
fn refusal(user: &User, id: &str, text: &str) -> Option<String> {
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": text}),
    )
    .err()
    .map(|(code, _)| code)
}

/// Whether the daemon keeps a draft of the session, whatever it answers a composer of its own.
fn listed(user: &User, id: &str) -> bool {
    user.call("draft.list", json!({})).unwrap()["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|key| *key == format!("session:{id}"))
}

fn picture_files(user: &User) -> usize {
    std::fs::read_dir(user.path("state/flottille/drafts"))
        .map(|folders| folders.count())
        .unwrap_or(0)
}

/// A draft with a picture, for a session.
fn draft_with_picture(user: &User, id: &str) {
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "to be forgotten",
               "images": [{"mime": "image/png", "data": PNG_BASE64}]}),
    )
    .unwrap();
    assert_eq!(picture_files(user), 1, "the picture is a file of the state");
}

#[gpui_kit::test]
fn a_session_ended_for_good_forgets_its_draft(cx: &mut TestAppContext) {
    // Its agent cannot resume: once it ended, nothing of it is kept (`agent_task`).
    let user = User::with_fake_agent();
    user.fake_script(json!({"load": false, "resume": false}));
    project(&user, "app");
    let id = start(&user, "app", "first");
    draft_with_picture(&user, &id);

    user.call("agent.stop", json!({"agent": id})).unwrap();
    eventually("the draft goes with its session", || !listed(&user, &id));
    assert_eq!(picture_files(&user), 0, "the picture goes with its draft");
    // A debounce, the close of a window or a composer still open writes it again: refused.
    assert_eq!(
        refusal(&user, &id, "too late").as_deref(),
        Some("not-found")
    );
    assert!(!listed(&user, &id), "a forgotten session gets no draft");
    let _ = cx;
}

#[gpui_kit::test]
fn a_session_forgotten_after_its_kept_week_forgets_its_draft(cx: &mut TestAppContext) {
    // Its agent resumes: its session is kept 7 days after it ended (`forget_kept_sessions`).
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    user.fake_script(json!({"images": true}));
    project(&user, "app");
    let id = start(&user, "app", "first");
    draft_with_picture(&user, &id);
    user.call("agent.stop", json!({"agent": id})).unwrap();
    user.call("test.advance", json!({"ms": 6 * 24 * 3_600_000_u64}))
        .unwrap();
    assert!(listed(&user, &id), "kept while it can still be resumed");

    user.call("test.advance", json!({"ms": 2 * 24 * 3_600_000_u64}))
        .unwrap();
    assert!(!listed(&user, &id), "the draft goes with the kept session");
    assert_eq!(picture_files(&user), 0, "the picture goes with its draft");
    assert_eq!(
        refusal(&user, &id, "too late").as_deref(),
        Some("not-found")
    );
    assert!(!listed(&user, &id));
    let _ = cx;
}

#[gpui_kit::test]
fn a_resumable_ended_session_keeps_its_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    draft_with_picture(&user, &id);

    user.call("agent.stop", json!({"agent": id})).unwrap();
    eventually("the session ended", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "ended"
    });
    // It can be resumed, and the user may be writing to it: the draft stays, and takes changes.
    assert_eq!(kept(&user, "session", &id).map(|d| d.1), Some(1));
    assert_eq!(refusal(&user, &id, "still writing"), None);
    assert_eq!(
        kept(&user, "session", &id),
        Some(("still writing".to_owned(), 1))
    );
    let _ = cx;
}

#[gpui_kit::test]
fn a_removed_project_forgets_its_sessions_drafts_and_takes_no_other(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    draft_with_picture(&user, &id);
    user.call(
        "draft.set",
        json!({"kind": "project", "target": "app", "text": "a first message"}),
    )
    .unwrap();

    user.call("project.remove", json!({"name": "app"})).unwrap();
    eventually("the drafts go with the project", || {
        user.call("draft.list", json!({})).unwrap()["drafts"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    assert_eq!(picture_files(&user), 0);
    // Its agents' rows are kept, but their project is gone: nothing of them is written again.
    assert_eq!(
        refusal(&user, &id, "too late").as_deref(),
        Some("not-found")
    );
    let again = user.call(
        "draft.set",
        json!({"kind": "project", "target": "app", "text": "too late"}),
    );
    assert_eq!(again.unwrap_err().0, "not-found");
    let _ = cx;
}

#[gpui_kit::test]
fn the_drafts_left_without_their_session_are_swept_when_the_daemon_starts(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    let id = start(&user, "app", "first");
    let kept_id = start(&user, "other", "second");
    draft_with_picture(&user, &id);
    user.call(
        "draft.set",
        json!({"kind": "session", "target": kept_id, "text": "stays"}),
    )
    .unwrap();
    user.call(
        "draft.set",
        json!({"kind": "project", "target": "app", "text": "first of app"}),
    )
    .unwrap();
    user.call("daemon.stop", json!({})).unwrap();
    eventually("the daemon is gone", || {
        !user.path("runtime/flottille/daemon.sock").exists()
    });
    // The project's file goes while the daemon is down: its drafts are left without it. A folder
    // of pictures no draft names is left too (a write that ended halfway).
    std::fs::remove_file(user.path("config/flottille/projects/app.toml")).unwrap();
    let stray = user.path("state/flottille/drafts/6162");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join("a-0.png"), png()).unwrap();

    let drafts = user.call("draft.list", json!({})).unwrap()["drafts"].clone();
    assert_eq!(drafts, json!([format!("session:{kept_id}")]));
    assert_eq!(
        picture_files(&user),
        0,
        "no picture is left without its draft"
    );
    let _ = cx;
}

#[gpui_kit::test]
fn a_folded_project_stays_folded_after_reopening(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    start(&user, "app", "first");

    let (shell, cx) = window(&user, first);
    until(&shell, cx, "the project's row", |cx| {
        cx.update(|window, _| window.try_find(project_id("app")).is_some())
    });
    cx.update(|window, cx| window.click(toggle_id(project_id("app")), cx));
    settle(cx);
    assert!(!shell.read_with(cx, |s, cx| s.project("app").unwrap().read(cx).is_open()));
    eventually("the daemon keeps the fold", || {
        sidebar(&user)["projects"]["app"] == false
    });
    assert_eq!(sidebar(&user)["projects"]["other"], true);

    let (shell, cx) = window(&user, second);
    until(&shell, cx, "the project's row", |cx| {
        cx.update(|window, _| window.try_find(project_id("app")).is_some())
    });
    let open = |name: &str, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| s.project(name).unwrap().read(cx).is_open())
    };
    assert!(!open("app", cx), "the folded project is folded");
    assert!(open("other", cx), "the other keeps its default");
}

#[gpui_kit::test]
fn a_folded_lead_and_the_ended_group_stay_as_they_were(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let (lead, led) = lead_and_led(&user, "app");
    let gone = start(&user, "app", "an ordinary one");
    user.call("agent.stop", json!({"agent": gone})).unwrap();
    eventually("the session ended", || {
        user.call("agent.show", json!({"agent": gone})).unwrap()["agent"]["state"] == "ended"
    });

    let (shell, cx) = window(&user, first);
    until(&shell, cx, "the led row and the Ended group", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&led)).is_some() && window.try_find(ended_id()).is_some()
        })
    });
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    eventually("the daemon keeps both", || {
        let state = sidebar(&user);
        state["leads"][&lead] == false && state["ended"] == true
    });

    let (shell, cx) = window(&user, second);
    until(&shell, cx, "the lead's row and the Ended group", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&lead)).is_some() && window.try_find(ended_id()).is_some()
        })
    });
    assert!(
        !shell.read_with(cx, |s, cx| s.session(&lead).unwrap().read(cx).is_open()),
        "the lead's led rows are folded"
    );
    assert!(
        cx.update(|window, _| window.try_find(session_id(&led)).is_none()),
        "the led row is not drawn"
    );
    assert!(shell.read_with(cx, |s, cx| s.ended().unwrap().read(cx).is_open()));
}

#[gpui_kit::test]
fn a_project_removed_forgets_its_fold(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both projects' rows", |cx| {
        cx.update(|window, _| {
            window.try_find(project_id("app")).is_some()
                && window.try_find(project_id("other")).is_some()
        })
    });
    cx.update(|window, cx| window.click(toggle_id(project_id("other")), cx));
    settle(cx);
    eventually("the daemon keeps the fold", || {
        sidebar(&user)["projects"]["other"] == false
    });

    user.call("project.remove", json!({"name": "other"}))
        .unwrap();
    // Its row folds away: the clock moves while the window waits for it.
    until(&shell, cx, "the project is gone from the window", |cx| {
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.update(|window, _| window.try_find(project_id("other")).is_none())
    });
    eventually("the daemon forgets its fold", || {
        sidebar(&user)["projects"].get("other").is_none()
    });
    assert_eq!(sidebar(&user)["projects"]["app"], true);
}

#[gpui_kit::test]
fn the_restored_sidebar_draws_no_animation_at_opening(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    let (lead, led) = lead_and_led(&user, "app");
    // The folded project has a session: unfolded, its fold has a height to see.
    start(&user, "other", "a session to fold away");
    let gone = start(&user, "app", "an ordinary one");
    user.call("agent.stop", json!({"agent": gone})).unwrap();
    eventually("the session ended", || {
        user.call("agent.show", json!({"agent": gone})).unwrap()["agent"]["state"] == "ended"
    });

    let (shell, cx) = window(&user, first);
    until(&shell, cx, "the led row and the Ended group", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&led)).is_some() && window.try_find(ended_id()).is_some()
        })
    });
    cx.update(|window, cx| window.click(toggle_id(project_id("other")), cx));
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    eventually("the daemon keeps the folds", || {
        let state = sidebar(&user);
        state["projects"]["other"] == false
            && state["leads"][&lead] == false
            && state["ended"] == true
    });

    // Every frame of the new window, the clock moving, shows the folded project folded, the
    // folded lead without its led row and the "Ended" group left open open: nothing is drawn on
    // its way to what it was left as.
    let (shell, cx) = window(&user, second);
    let deadline = Instant::now() + Duration::from_secs(25);
    let (mut seen, mut ended) = (0, 0);
    while seen < 40 || ended < 40 {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        assert!(
            cx.update(|window, _| window.try_find(session_id(&led)).is_none()),
            "the led row is never drawn: its lead was left folded"
        );
        if let Some(fold) = cx.update(|window, _| window.try_find("fold-project-other")) {
            assert_eq!(fold.expanded(), Some(false), "folded from its first frame");
            assert_eq!(
                cx.update(|window, _| window.find("fold-project-other").bounds().size.height),
                px(0.),
                "its height is never seen open"
            );
            seen += 1;
        }
        if let Some(fold) = cx.update(|window, _| window.try_find("fold-ended")) {
            assert_eq!(fold.expanded(), Some(true), "unfolded from its first frame");
            assert!(
                cx.update(|window, _| window.find("fold-ended").bounds().size.height) > px(0.),
                "its height is never seen closed"
            );
            ended += 1;
        }
        assert!(Instant::now() < deadline, "the folds are never drawn");
        std::thread::sleep(Duration::from_millis(10));
    }
    // The lead's row itself was drawn, folded: its chevron says so.
    assert!(cx.update(|window, _| window.try_find(session_id(&lead)).is_some()));

    // Restored without motion, the folds still move when the user folds them: a project
    // unfolded is seen part of the way, not all at once.
    settle(cx);
    cx.update(|window, cx| window.click(toggle_id(project_id("other")), cx));
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let part = cx.update(|window, _| window.find("fold-project-other").bounds().size.height);
    settle(cx);
    let whole = cx.update(|window, _| window.find("fold-project-other").bounds().size.height);
    assert!(
        part > px(0.) && part < whole,
        "a later unfold is animated: {part:?} of {whole:?} after 40 ms"
    );
}

/// A window on the session `id`'s row, whose page is opened while the daemon does not answer.
fn window_with_a_daemon_that_does_not_answer<'a>(
    user: &User,
    id: &str,
    silence: &Silence,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window_via(user, silence.connect(user), cx);
    until(&shell, cx, "the session's row", |cx| {
        cx.update(|window, _| window.try_find(session_id(id)).is_some())
    });
    silence.fall();
    open_unread(id, &shell, cx);
    (shell, cx)
}

#[gpui_kit::test]
fn a_draft_read_that_failed_is_asked_again_and_not_taken_for_no_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "what the daemon keeps"}),
    )
    .unwrap();
    let silence = Silence::default();
    let (shell, cx) = window_with_a_daemon_that_does_not_answer(&user, &id, &silence, cx);

    // Said nothing yet, the composer is empty and may be typed in: it does not know the draft.
    assert!(!shell.read_with(cx, |s, cx| {
        s.reply_of(&id).unwrap().read(cx).draft_was_read()
    }));
    assert_eq!(reply_text(&id, &shell, cx), "");
    silence.rise();
    until_later(
        &shell,
        cx,
        "the draft is read again and comes back",
        Duration::from_secs(1),
        |cx| reply_text(&id, &shell, cx) == "what the daemon keeps",
    );
    assert_eq!(
        kept(&user, "session", &id),
        Some(("what the daemon keeps".to_owned(), 0)),
        "nothing replaced what the daemon holds"
    );
}

#[gpui_kit::test]
fn a_text_typed_before_the_draft_was_read_is_kept_when_the_window_closes(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "older words"}),
    )
    .unwrap();
    let silence = Silence::default();
    let (shell, cx) = window_with_a_daemon_that_does_not_answer(&user, &id, &silence, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("typed meanwhile");
    assert!(!shell.read_with(cx, |s, cx| {
        s.reply_of(&id).unwrap().read(cx).draft_was_read()
    }));

    // The daemon is back, and the window closes before the read was asked again.
    silence.rise();
    close(&shell, cx);
    eventually("the daemon keeps what was typed", || {
        kept(&user, "session", &id) == Some(("typed meanwhile".to_owned(), 0))
    });
}

#[gpui_kit::test]
fn a_text_typed_while_the_daemon_does_not_answer_never_replaces_its_draft(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    user.call(
        "draft.set",
        json!({"kind": "session", "target": id, "text": "older words"}),
    )
    .unwrap();
    let silence = Silence::default();
    let (shell, cx) = window_with_a_daemon_that_does_not_answer(&user, &id, &silence, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("typed meanwhile");

    // The window closes while the daemon is still silent: nothing is known of its draft, so
    // nothing is written over it.
    close(&shell, cx);
    silence.rise();
    assert_eq!(
        kept(&user, "session", &id),
        Some(("older words".to_owned(), 0))
    );
}

#[gpui_kit::test]
fn a_draft_the_daemon_did_not_take_is_asked_again(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    open(&id, &shell, cx);
    let reply = shell.read_with(cx, |s, _| s.reply_of(&id).cloned().unwrap());
    // The folder of the pictures cannot be written: the daemon cannot keep this draft yet.
    let folder = user.path("state/flottille/drafts");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::set_permissions(&folder, std::os::unix::fs::PermissionsExt::from_mode(0o500)).unwrap();
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("a picture follows ");
    reply.update(cx, |reply, cx| {
        reply.attach(Image::from_bytes(ImageFormat::Png, png()), cx)
    });
    settle(cx);
    debounced(cx);
    let sent = |cx: &mut VisualTestContext| reply.read_with(cx, |r, _| r.drafts_sent());
    until(&shell, cx, "the write is asked", |cx| sent(cx) >= 1);
    assert_eq!(kept(&user, "session", &id), None, "the daemon could not");

    std::fs::set_permissions(&folder, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    until_later(
        &shell,
        cx,
        "the draft is asked again, and kept",
        Duration::from_secs(1),
        |_| kept(&user, "session", &id).is_some_and(|(_, pictures)| pictures == 1),
    );
    assert_eq!(
        kept(&user, "session", &id).map(|(text, _)| text),
        Some("a picture follows [Image 1: image.png]".to_owned())
    );
}

#[gpui_kit::test]
fn a_quit_that_is_not_a_window_close_keeps_what_the_composers_hold(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "first");
    let (shell, cx) = window(&user, cx);
    type_in_reply(&id, "typed a moment ago", &shell, cx);
    assert_eq!(kept(&user, "session", &id), None, "the 500 ms are not over");

    // The program ends by its own way, the desktop's quit or a signal: no close of the window.
    cx.quit();
    eventually("the daemon keeps what was typed", || {
        kept(&user, "session", &id) == Some(("typed a moment ago".to_owned(), 0))
    });
}

#[gpui_kit::test]
fn a_row_says_its_draft_at_once_after_reopening_without_opening_the_session(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let with = start(&user, "app", "first");
    let without = start(&user, "app", "second");
    user.call(
        "draft.set",
        json!({"kind": "session", "target": with, "text": "left half written"}),
    )
    .unwrap();

    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the rows", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&with)).is_some()
                && window.try_find(session_id(&without)).is_some()
        })
    });
    until(&shell, cx, "the pencil of the session with a draft", |cx| {
        cx.update(|window, _| window.try_find(session_draft_id(&with)).is_some())
    });
    assert!(
        shell.read_with(cx, |s, _| s.reply_of(&with).is_none()),
        "its page was not opened"
    );
    assert!(
        cx.update(|window, _| window.try_find(session_draft_id(&without)).is_none()),
        "the other session has no pencil"
    );
}
