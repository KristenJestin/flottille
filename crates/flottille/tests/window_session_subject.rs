//! What a session is about, in the window (#1450): its row says the ticket it works on, the key as
//! a chip and its title, unless the user named it, and says when it holds nothing any more; its
//! page says where that comes from. On GPUI's test platform in front of a real daemon whose agents
//! are the fake ACP agent and whose forge is the fake `gh`. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::tree::{session_id, session_key_beside_id, session_runs_id};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn user() -> User {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A project `app` of one repository on GitHub `acme/api`, its issues the tracker, and its ready
/// workspace `login`.
fn fixture(user: &User) {
    let folder = user.path("work/app");
    let dir = folder.join("api");
    user.git_init(&dir);
    std::fs::write(dir.join("README.md"), "api\n").unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/api.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
    user.git(&dir, &["push", "--quiet", "origin", "main"]);
    user.git(
        &dir,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
            "tracker": {"kind": "github", "repository": "api"}}),
    )
    .unwrap();
}

fn issue(number: u64, title: &str, state: &str) -> Value {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    json!({
        "number": number,
        "title": title,
        "state": state,
        "body": "What it is.",
        "url": format!("https://github.com/acme/api/issues/{number}"),
        "createdAt": "2026-10-01T10:00:00Z",
        "updatedAt": format!("2999-01-01T{:02}:{:02}:00Z", n / 60 % 24, n % 60),
        "comments": []
    })
}

/// The fake `gh` answers these issues, and a round of the items reads them.
fn issues(user: &User, issues: &[Value]) {
    let issues: serde_json::Map<String, Value> = issues
        .iter()
        .map(|issue| (issue["number"].to_string(), issue.clone()))
        .collect();
    user.gh_script(json!({"issues": {"acme/api": issues}, "pulls": {}}));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

fn link(user: &User, ticket: &str) {
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": ticket}),
    )
    .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A session started in `login`, its first turn over.
fn start(user: &User) -> String {
    let id = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "fix the login form"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

fn subject(user: &User, agent: &str) -> Value {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == agent)
        .unwrap()["subject"]
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
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
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

fn label(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

/// The row's height: it never changes with what the row says.
fn row_height(cx: &mut VisualTestContext, session: &str) -> gpui_kit::Pixels {
    cx.update(|window, _| window.find(session_id(session)).bounds().size.height)
}

/// A session about the ticket #5, `The login fails`, and the window showing its row.
fn about_five<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (String, Entity<Shell>, &'a mut VisualTestContext) {
    fixture(user);
    issues(
        user,
        &[
            issue(5, "The login fails", "OPEN"),
            issue(6, "The logout hangs", "OPEN"),
        ],
    );
    link(user, "#5");
    let id = start(user);
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the row says the ticket", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("The login fails")
    });
    settle(cx);
    (id, shell, cx)
}

/// The first line's words, as the row draws them.
fn title_bounds(cx: &mut VisualTestContext, session: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update(|window, _| {
        window
            .find(gpui_kit::ElementId::Name(
                format!("{}-title", session_id(session)).into(),
            ))
            .bounds()
    })
}

#[gpui_kit::test]
fn a_row_says_the_ticket_its_session_is_about_on_its_second_line(cx: &mut TestAppContext) {
    let user = user();
    let (id, _shell, cx) = about_five(&user, cx);
    let height = row_height(cx, &id);

    assert_eq!(label(cx, session_key_beside_id(&id)).as_deref(), Some("#5"));
    // The key is the mono word under the title, never a chip before it.
    let (key, title) = cx.update(|window, _| {
        (
            window.find(session_key_beside_id(&id)).bounds(),
            window
                .find(gpui_kit::ElementId::Name(
                    format!("{}-title", session_id(&id)).into(),
                ))
                .bounds(),
        )
    });
    assert!(key.top() >= title.bottom() - px(1.), "{key:?} {title:?}");
    assert!(key.size.height <= px(16.), "it fits its line: {key:?}");
    assert_eq!(row_height(cx, &id), height);
}

#[gpui_kit::test]
fn a_key_never_pushes_its_row_s_title(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    let with_key = title_bounds(cx, &id);

    issues(
        &user,
        &[
            issue(5, "The login fails", "CLOSED"),
            issue(6, "The logout hangs", "OPEN"),
        ],
    );
    eventually("free", || subject(&user, &id)["kind"] == "free");
    until(&shell, cx, "the row says it holds nothing", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("Free · nothing in hand")
    });
    settle(cx);
    let without = title_bounds(cx, &id);

    assert_eq!(
        (with_key.left(), with_key.right()),
        (without.left(), without.right()),
        "the first line is as wide with the key as without"
    );
}

/// What the row of `session` is shown: its model on its second line, drawn or not.
fn runs_drawn(cx: &mut VisualTestContext, session: &str) -> bool {
    cx.update(|window, _| window.try_find(session_runs_id(session)).is_some())
}

#[gpui_kit::test]
fn a_key_takes_the_model_s_place_on_the_second_line(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    let row = shell.read_with(cx, |shell, _| shell.session(&id).unwrap().clone());

    row.update(cx, |row, cx| row.set_runs("Fake Large 5.5 · high", cx));
    settle(cx);
    assert_eq!(label(cx, session_key_beside_id(&id)).as_deref(), Some("#5"));
    assert!(!runs_drawn(cx, &id), "the model gives way to the key");
}

#[gpui_kit::test]
fn a_session_about_no_ticket_keeps_its_model_on_the_second_line(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    issues(
        &user,
        &[
            issue(5, "The login fails", "CLOSED"),
            issue(6, "The logout hangs", "OPEN"),
        ],
    );
    eventually("free", || subject(&user, &id)["kind"] == "free");
    until(&shell, cx, "the row says it holds nothing", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("Free · nothing in hand")
    });
    settle(cx);
    let row = shell.read_with(cx, |shell, _| shell.session(&id).unwrap().clone());

    row.update(cx, |row, cx| row.set_runs("Fake Large 5.5 · high", cx));
    settle(cx);
    assert_eq!(
        label(cx, session_runs_id(&id)).as_deref(),
        Some("Fake Large 5.5 · high")
    );
    assert!(
        cx.update(|window, _| window.try_find(session_key_beside_id(&id)).is_none()),
        "no key"
    );
}

#[gpui_kit::test]
fn the_key_and_the_model_cross_fade_on_the_second_line(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    let row = shell.read_with(cx, |shell, _| shell.session(&id).unwrap().clone());
    let about = row.read_with(cx, |row, _| row.subject().cloned());
    // Free, with its model, then the ticket comes back: the key fades in as the model fades out.
    row.update(cx, |row, cx| row.set_subject(None, false, cx));
    row.update(cx, |row, cx| row.set_runs("Fake Large 5.5 · high", cx));
    settle(cx);
    assert!(runs_drawn(cx, &id));
    assert_eq!(pending_frames(cx), 0, "at rest before");

    row.update(cx, |row, cx| row.set_subject(about.as_ref(), false, cx));
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(
        runs_drawn(cx, &id) && label(cx, session_key_beside_id(&id)).is_some(),
        "both are there while they cross"
    );
    settle(cx);
    assert!(!runs_drawn(cx, &id), "the model is gone");
    assert_eq!(label(cx, session_key_beside_id(&id)).as_deref(), Some("#5"));
    assert_eq!(pending_frames(cx), 0, "then nothing moves");
}

#[gpui_kit::test]
fn a_renamed_session_keeps_its_title_and_shows_the_key_beside_it(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    let height = row_height(cx, &id);

    user.call("agent.rename", json!({"agent": id, "title": "Login work"}))
        .unwrap();
    until(&shell, cx, "the row says the user's title", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("Login work")
    });
    settle(cx);

    assert_eq!(
        label(cx, session_key_beside_id(&id)).as_deref(),
        Some("#5"),
        "the key moves to the second line"
    );
    let beside = cx.update(|window, _| window.find(session_key_beside_id(&id)).bounds());
    assert!(
        beside.size.height <= px(16.),
        "it fits its 16 px line: {beside:?}"
    );
    assert_eq!(row_height(cx, &id), height, "the row keeps its height");
}

#[gpui_kit::test]
fn a_renamed_session_s_key_fades_to_the_new_one(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    user.call("agent.rename", json!({"agent": id, "title": "Login work"}))
        .unwrap();
    until(&shell, cx, "the row says the user's title", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("Login work")
    });
    settle(cx);
    assert_eq!(label(cx, session_key_beside_id(&id)).as_deref(), Some("#5"));
    assert_eq!(pending_frames(cx), 0, "at rest before");

    link(&user, "#6");
    until(&shell, cx, "the new key beside", |cx| {
        label(cx, session_key_beside_id(&id)).as_deref() == Some("#6")
    });
    assert!(pending_frames(cx) > 0, "the new key fades in, not a jump");
    settle(cx);
    assert_eq!(pending_frames(cx), 0, "then nothing moves");
}

#[gpui_kit::test]
fn a_free_session_says_it_holds_nothing(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    let height = row_height(cx, &id);

    issues(
        &user,
        &[
            issue(5, "The login fails", "CLOSED"),
            issue(6, "The logout hangs", "OPEN"),
        ],
    );
    eventually("free", || subject(&user, &id)["kind"] == "free");
    until(&shell, cx, "the row says it holds nothing", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("Free · nothing in hand")
    });
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find(session_key_beside_id(&id)).is_none()),
        "no key is left"
    );
    assert_eq!(row_height(cx, &id), height, "the row keeps its height");
    assert_eq!(
        title_ink(cx, &id),
        cx.update(|_, cx| gpui_design_system::palette(cx).faint),
        "its words are faint: it has nothing to say"
    );
}

/// The ink the session row's title was last drawn in.
fn title_ink(cx: &mut VisualTestContext, session: &str) -> gpui_kit::Hsla {
    cx.update(|window, cx| window.simulate_next_frame(cx));
    gpui_design_system::seen::colour(gpui_kit::ElementId::Name(
        format!("{}-title", session_id(session)).into(),
    ))
    .expect("the title notes its ink")
}

#[gpui_kit::test]
fn the_subject_fades_in_when_it_changes(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    assert_eq!(pending_frames(cx), 0, "at rest before");
    let height = row_height(cx, &id);

    // The workspace goes to another ticket: the session is about it now.
    link(&user, "#6");
    until(&shell, cx, "the row says the new ticket", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("The logout hangs")
    });
    assert!(pending_frames(cx) > 0, "the new subject fades in");
    assert_eq!(label(cx, session_key_beside_id(&id)).as_deref(), Some("#6"));
    assert_eq!(row_height(cx, &id), height, "the row keeps its height");
    settle(cx);
    assert_eq!(pending_frames(cx), 0, "then nothing moves");
}

#[gpui_kit::test]
fn a_session_page_says_its_ticket_in_workspace_not_in_its_header(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);

    cx.update(|window, cx| window.click(session_id(&id), cx));
    // The ticket is the workspace's own: its line says it once, and no second line repeats it.
    until(&shell, cx, "the page says its ticket in Workspace", |cx| {
        label(cx, "rail-line-ticket").is_some()
    });
    settle(cx);
    assert!(label(cx, "rail-about-ticket").is_none(), "said once");
    // The header keeps the session's own title, and no line of what it is about.
    assert_eq!(
        label(cx, "session-title").as_deref(),
        Some("fix the login form")
    );
    assert!(label(cx, "session-about-ticket").is_none());
    assert!(label(cx, "session-about-from-words").is_none());
}
