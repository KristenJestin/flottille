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
use flottille_window::tree::{session_id, session_key_beside_id, session_key_id};
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

#[gpui_kit::test]
fn a_row_says_the_ticket_its_session_is_about(cx: &mut TestAppContext) {
    let user = user();
    let (id, _shell, cx) = about_five(&user, cx);

    assert_eq!(label(cx, session_key_id(&id)).as_deref(), Some("#5"));
    assert!(
        cx.update(|window, _| window.try_find(session_key_beside_id(&id)).is_none()),
        "the key is on the first line only"
    );
    // The key comes before the title, on its line.
    let (key, title) = cx.update(|window, _| {
        (
            window.find(session_key_id(&id)).bounds(),
            window
                .find(gpui_kit::ElementId::Name(
                    format!("{}-title", session_id(&id)).into(),
                ))
                .bounds(),
        )
    });
    assert!(key.right() <= title.left(), "{key:?} {title:?}");
    assert!(key.top() < title.bottom() && title.top() < key.bottom());
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
    assert!(
        cx.update(|window, _| window.try_find(session_key_id(&id)).is_none()),
        "and leaves the first"
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
    until(&shell, cx, "the key beside", |cx| {
        label(cx, session_key_beside_id(&id)).as_deref() == Some("#5")
    });
    settle(cx);
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
        cx.update(|window, _| window.try_find(session_key_id(&id)).is_none()),
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
    assert_eq!(label(cx, session_key_id(&id)).as_deref(), Some("#6"));
    assert_eq!(row_height(cx, &id), height, "the row keeps its height");
    settle(cx);
    assert_eq!(pending_frames(cx), 0, "then nothing moves");
}

#[gpui_kit::test]
fn a_session_page_says_where_its_subject_comes_from(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);

    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the page says where it comes from", |cx| {
        label(cx, "session-about-from-words").as_deref()
            == Some("What it is about, from the workspace's linked ticket.")
    });
    assert_eq!(label(cx, "session-about-ticket").as_deref(), Some("#5"));
    // The header keeps the session's own title.
    assert_eq!(
        label(cx, "session-title").as_deref(),
        Some("fix the login form")
    );
}

/// The branch of the workspace `login`.
fn branch(user: &User) -> String {
    user.call("workspace.list", json!({"project": "app"}))
        .unwrap()["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "login")
        .unwrap()["branch"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// What the fake `gh` answers of the pull request #12 of `acme/api` on `branch`, its checks one
/// run of `status` and `conclusion`.
fn pull(branch: &str, status: &str, conclusion: &str) -> Value {
    json!({
        "number": 12, "title": "Fix the login", "state": "OPEN", "isDraft": false,
        "headRefName": branch, "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": "https://github.com/acme/api/pull/12",
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "updatedAt": format!("2999-02-01T00:00:{:02}Z", match status { "" => 0, "COMPLETED" => 2, _ => 1 }),
        "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
        "statusCheckRollup": if status.is_empty() { json!([]) } else {
            json!([{"__typename": "CheckRun", "name": "build", "status": status,
                "conclusion": conclusion,
                "detailsUrl": "https://github.com/acme/api/actions/runs/900/job/build"}])
        },
        "comments": [], "reviews": [], "body": ""
    })
}

/// The fake `gh` answers these issues and pull requests, and a round of the items reads them.
fn forge(user: &User, issues: &[Value], pulls: &[Value]) {
    let issues: serde_json::Map<String, Value> = issues
        .iter()
        .map(|issue| (issue["number"].to_string(), issue.clone()))
        .collect();
    let pulls: serde_json::Map<String, Value> = pulls
        .iter()
        .map(|pull| {
            (
                pull["headRefName"].as_str().unwrap().to_owned(),
                pull.clone(),
            )
        })
        .collect();
    user.gh_script(json!({"issues": {"acme/api": issues}, "pulls": {"acme/api": pulls}}));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

/// How much of the chip `id` is drawn: the strongest ink of its border, 0 when it is not drawn.
fn chip_drawn(cx: &mut VisualTestContext, id: &str) -> f32 {
    cx.update(|window, _| {
        let Some(found) = window.try_find(id.to_owned()) else {
            return 0.0;
        };
        let bounds = found.bounds().scale(window.scale_factor());
        if bounds.size.width.0 <= 0.0 {
            return 0.0;
        }
        window
            .painted_quads()
            .iter()
            .filter(|quad| bounds.contains(&quad.bounds.center()))
            .map(|quad| quad.border_color.a)
            .fold(0.0, f32::max)
    })
}

/// The chip `id` drawn at each of 30 frames of 16 ms.
fn chip_frames(cx: &mut VisualTestContext, id: &str) -> Vec<f32> {
    (0..30)
        .map(|_| {
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update(|window, cx| window.simulate_next_frame(cx));
            chip_drawn(cx, id)
        })
        .collect()
}

/// Whether the element `id` is drawn inside the element `within`.
fn drawn_within(cx: &mut VisualTestContext, id: &str, within: &str) -> bool {
    cx.update(|window, _| {
        window.try_find(id.to_owned()).is_some_and(|found| {
            found
                .path()
                .contains(&gpui_kit::ElementId::Name(within.to_owned().into()))
        })
    })
}

#[gpui_kit::test]
fn the_page_s_chips_unfold_fade_and_fold_away_as_its_subject_changes(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, cx) = about_five(&user, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the page's ticket chip", |cx| {
        label(cx, "session-about-ticket").as_deref() == Some("#5")
    });
    settle(cx);
    let open = [
        issue(5, "The login fails", "OPEN"),
        issue(6, "The logout hangs", "OPEN"),
    ];
    let branch = branch(&user);

    // Its branch's pull request comes, no checks yet: its chip fades in beside the ticket's.
    forge(&user, &open, &[pull(&branch, "", "")]);
    until(&shell, cx, "the pull request's chip", |cx| {
        label(cx, "session-about-pull").as_deref() == Some("#12")
    });
    let coming = chip_frames(cx, "session-about-pull-key");
    let full = *coming.last().unwrap();
    assert!(full > 0.0, "it ends drawn: {coming:?}");
    assert!(
        coming.iter().any(|&a| a > 0.0 && a < full),
        "it fades in: {coming:?}"
    );

    // Its checks start: the chip widens to its mark over frames, never at once.
    settle(cx);
    let width = |cx: &mut VisualTestContext| {
        cx.update(|window, _| window.find("session-about-pull").bounds().size.width)
    };
    let before = width(cx);
    forge(&user, &open, &[pull(&branch, "IN_PROGRESS", "")]);
    until(&shell, cx, "the checks' first mark", |cx| {
        label(cx, "session-about-pull-checks").as_deref() == Some("pending")
    });
    let widths: Vec<_> = (0..30)
        .map(|_| {
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update(|window, cx| window.simulate_next_frame(cx));
            width(cx)
        })
        .collect();
    let after = *widths.last().unwrap();
    assert!(after > before, "it ends wider: {before:?} {widths:?}");
    assert!(
        widths.iter().any(|&w| before < w && w < after),
        "it widens over frames: {before:?} {widths:?}"
    );

    // Its checks pass: the mark fades to the new one.
    forge(&user, &open, &[pull(&branch, "COMPLETED", "SUCCESS")]);
    until(&shell, cx, "the checks' new mark", |cx| {
        label(cx, "session-about-pull-checks").as_deref() == Some("passing")
    });
    assert!(
        drawn_within(
            cx,
            "session-about-pull-checks",
            "session-about-pull-checks-change"
        ),
        "the mark is drawn by a fade of the kit"
    );
    assert!(pending_frames(cx) > 0, "the new mark fades in");
    settle(cx);

    // The ticket is done: the session is free, its ticket chip folds away, the pull request's
    // stays.
    forge(
        &user,
        &[
            issue(5, "The login fails", "CLOSED"),
            issue(6, "The logout hangs", "OPEN"),
        ],
        &[pull(&branch, "COMPLETED", "SUCCESS")],
    );
    eventually("free", || subject(&user, &id)["kind"] == "free");
    until(&shell, cx, "the page says what it last held", |cx| {
        label(cx, "session-about-from-words").is_some_and(|words| words.contains("#5 done"))
    });
    let going = chip_frames(cx, "session-about-ticket");
    let whole = chip_drawn(cx, "session-about-pull-key");
    assert!(
        going.iter().any(|&a| a > 0.0 && a < whole),
        "it does not leave at once: {going:?}"
    );
    assert_eq!(*going.last().unwrap(), 0.0, "it ends gone: {going:?}");
    assert!(chip_drawn(cx, "session-about-pull-key") > 0.0);
}
