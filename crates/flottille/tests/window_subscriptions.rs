//! The scenarios of a session's subscriptions in the window (#208), on GPUI's test platform in
//! front of a real daemon whose agents are the fake ACP agent and whose forge is the fake `gh`:
//! the rail lists what the agent waits for and Cancel ends it through the daemon, and the
//! delivery that wakes the agent says what woke it. No window opens on a screen, no forge is read.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::protocol::Event;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::session::{SessionPage, state_of};
use flottille_window::session_link::SessionUpdate;
use flottille_window::subscriptions::SubscriptionRow;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The project `app` (repository `api` on GitHub `acme/api`), its ready workspace `login` on the
/// branch `app/login`, and an agent the user started there, its first turn ended: its id and MCP
/// session.
fn lead(user: &User) -> (String, Mcp) {
    lead_in(user, "api")
}

/// [`lead`], the project's one repository being `repository`: `.` is the project folder itself.
fn lead_in(user: &User, repository: &str) -> (String, Mcp) {
    let folder = user.path("work/app");
    let dir = match repository {
        "." => folder.clone(),
        repository => folder.join(repository),
    };
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
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    user.git(
        &dir,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [
            {"path": repository, "forge": {"kind": "github", "repository": "acme/api"}}
        ]}),
    )
    .unwrap();
    // The daemon read the project's pull requests once.
    a_round(user);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "hello"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, Mcp::initialize(&url, &authorization))
}

fn check(name: &str, status: &str, conclusion: &str, job: u64) -> Value {
    json!({"__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion,
        "detailsUrl": format!("https://github.com/acme/api/actions/runs/900/job/{job}")})
}

/// The pull request #12 of `app/login`, its checks running.
fn pull() -> Value {
    json!({
        "number": 12, "title": "Add the login page", "state": "OPEN", "isDraft": false,
        "headRefOid": "aaaaaaa1", "baseRefName": "main",
        "url": "https://github.com/acme/api/pull/12",
        "statusCheckRollup": [
            check("build", "COMPLETED", "SUCCESS", 1),
            check("test", "IN_PROGRESS", "", 456)
        ],
        "comments": [], "reviews": []
    })
}

/// The same pull request, its checks ended, `test` failing.
fn ended() -> Value {
    let mut pull = pull();
    pull["statusCheckRollup"][1] = check("test", "COMPLETED", "FAILURE", 456);
    pull
}

/// The fake `gh` answers `pull`, changed now (a newer `updatedAt`), and the failed job's log.
fn script(user: &User, mut pull: Value) {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    pull["updatedAt"] = format!("2999-01-01T{:02}:{:02}:00Z", n / 60 % 24, n % 60).into();
    let log = "test\tRun cargo test\t2026-01-01T00:00:00.1234567Z test login::form ... FAILED\n";
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": pull}},
        "logs": {"acme/api": {"456": log}}
    }));
}

fn subscribe(mcp: &Mcp, events: Value) -> String {
    let answer: Answer = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": events, "minutes": 30}),
    );
    assert!(!answer.error, "{}", answer.text);
    answer.text
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
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn open_session<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(&shell, cx, "the session's page shows", |cx| {
        session_page(&shell, cx).read_with(cx, |page, cx| {
            !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
        })
    });
    (shell, cx)
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn rows(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<SubscriptionRow> {
    page.read_with(cx, |page, cx| {
        page.subscriptions(cx).read(cx).rows().to_vec()
    })
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn mark_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<AgentState> {
    shell.read_with(cx, |shell, cx| {
        shell.session(id).and_then(|row| row.read(cx).mark())
    })
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

/// A round of the project's items, as the daemon makes every few minutes: what a subscription
/// hears.
fn a_round(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

#[gpui_kit::test]
fn a_subscription_shows_in_the_rail_with_its_pull_request_its_events_and_when_it_expires(
    cx: &mut TestAppContext,
) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead(&user);
    subscribe(&mcp, json!(["checks", "review"]));
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    until(&shell, cx, "the rail lists the subscription", |cx| {
        !rows(&page, cx).is_empty()
    });
    settle(cx);

    let listed = rows(&page, cx);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let id = &listed[0].id;
    assert_eq!(
        label(cx, &format!("subscription-pull-{id}")).unwrap(),
        "#12 of api"
    );
    assert_eq!(
        label(cx, &format!("subscription-events-{id}")).unwrap(),
        "checks, review"
    );
    let times = label(cx, &format!("subscription-times-{id}")).unwrap();
    assert!(
        times.contains("since") && times.contains("expires in 30 min"),
        "{times}"
    );
    assert!(
        present(cx, &format!("subscription-cancel-{id}")),
        "a subscription has its Cancel"
    );
    assert_eq!(
        label(cx, &format!("subscription-cancel-{id}")).unwrap(),
        "Cancel"
    );
}

#[gpui_kit::test]
fn a_session_with_no_subscription_has_no_section(cx: &mut TestAppContext) {
    let user = user();
    script(&user, pull());
    let (lead, _mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    settle(cx);
    let _ = shell;

    assert!(!present(cx, "rail-subscriptions-title"));
}

#[gpui_kit::test]
fn cancel_ends_the_subscription_through_the_daemon_and_the_row_folds_away(cx: &mut TestAppContext) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead(&user);
    subscribe(&mcp, json!(["checks"]));
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the subscription", |cx| {
        !rows(&page, cx).is_empty()
    });
    settle(cx);
    let id = rows(&page, cx)[0].id.clone();

    cx.update(|window, cx| window.click(format!("subscription-cancel-{id}"), cx));
    until(&shell, cx, "the subscription is gone", |cx| {
        rows(&page, cx).is_empty()
    });
    settle(cx);

    // The daemon ended it: its list is empty, and what the agent reads says so.
    let listed = user
        .call("subscription.list", json!({"tree": lead}))
        .unwrap();
    assert_eq!(listed["subscriptions"], json!([]), "{listed}");
    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(!shown.text.contains("Your subscriptions"), "{}", shown.text);
    assert!(!present(cx, &format!("subscription-pull-{id}")));
    assert!(!present(cx, "rail-subscriptions-title"));
    // Nothing is delivered of what the agent no longer waits for.
    script(&user, ended());
    a_round(&user);
    std::thread::sleep(Duration::from_millis(500));
    let prompts = user.received_requests("session/prompt");
    assert!(
        prompts
            .iter()
            .all(|p| !p.to_string().contains("[flottille:subscription]")),
        "{prompts:?}"
    );
}

#[gpui_kit::test]
fn a_subscription_that_delivers_stays_in_the_rail_until_its_pull_request_is_merged(
    cx: &mut TestAppContext,
) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead(&user);
    // No deadline: it lasts until the pull request is merged or closed.
    let answer: Answer = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": ["checks"]}),
    );
    assert!(!answer.error, "{}", answer.text);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the subscription", |cx| {
        !rows(&page, cx).is_empty()
    });
    let id = rows(&page, cx)[0].id.clone();
    let times = label(cx, &format!("subscription-times-{id}")).unwrap();
    assert!(
        times.starts_with("since") && times.ends_with(" · until merged or closed"),
        "{times}"
    );

    script(&user, ended());
    a_round(&user);
    until(&shell, cx, "the delivery is in the conversation", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Notice { kind, .. } if kind == "woken by"))
    });
    settle(cx);

    let woken: Vec<String> = blocks(&page, cx)
        .into_iter()
        .filter_map(|b| match b {
            Block::Notice { kind, text } if kind == "woken by" => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(woken.len(), 1, "{woken:?}");
    // Its first line says what woke the agent, the forge's text follows it.
    assert_eq!(
        woken[0].lines().next(),
        Some("checks of #12 ended, 1 failed")
    );
    assert!(
        woken[0].contains("test login::form ... FAILED"),
        "{woken:?}"
    );
    assert_eq!(rows(&page, cx).len(), 1, "it goes on");

    let mut merged = ended();
    merged["state"] = "MERGED".into();
    script(&user, merged);
    a_round(&user);
    until(&shell, cx, "the subscription left the rail", |cx| {
        rows(&page, cx).is_empty()
    });
}

/// Unfolds the group that holds the row `at`, when it is folded: its rows are drawn.
fn unfold(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) {
    let groups = page.read_with(cx, |page, cx| page.conversation().read(cx).groups());
    let Some(group) = groups.into_iter().find(|group| group.contains(&at)) else {
        return;
    };
    let line = page
        .read_with(cx, |page, cx| page.conversation().read(cx).group_id(&group))
        .expect("a group's line");
    if cx.update(|window, _| window.find(line.clone()).expanded()) == Some(false) {
        cx.update(|window, cx| window.click(line, cx));
        settle(cx);
    }
}

/// The row of the conversation that shows the delivery, as the page numbers it.
fn woken_row(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<usize> {
    blocks(page, cx)
        .iter()
        .position(|b| matches!(b, Block::Notice { kind, .. } if kind == "woken by"))
}

#[gpui_kit::test]
fn the_delivery_is_a_folded_step_that_says_what_woke_the_agent_and_a_click_unfolds_the_forge_text(
    cx: &mut TestAppContext,
) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead(&user);
    subscribe(&mcp, json!(["checks"]));
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    script(&user, ended());
    a_round(&user);
    until(&shell, cx, "the step is in the conversation", |cx| {
        woken_row(&page, cx).is_some()
    });
    settle(cx);
    let at = woken_row(&page, cx).unwrap();
    unfold(&page, at, cx);

    assert_eq!(
        label(cx, &format!("row-{at}-notice-summary")).unwrap(),
        "checks of #12 ended, 1 failed"
    );
    assert!(
        !present(cx, &format!("row-{at}-notice-text")),
        "the forge's text is folded"
    );
    assert!(
        !blocks(&page, cx).iter().any(|b| matches!(
            b,
            Block::Notice { kind, .. } if kind == "subscription"
        )),
        "never the raw delivery"
    );

    cx.update(|window, cx| window.click(format!("row-{at}-notice-toggle"), cx));
    settle(cx);
    let text = label(cx, &format!("row-{at}-notice-text")).expect("a click unfolds it");
    assert!(text.contains("test login::form ... FAILED"), "{text}");

    cx.update(|window, cx| window.click(format!("row-{at}-notice-toggle"), cx));
    settle(cx);
    assert!(
        !present(cx, &format!("row-{at}-notice-text")),
        "and folds again"
    );
}

#[gpui_kit::test]
fn the_error_of_a_cancel_unfolds_and_folds_away_it_does_not_jump(cx: &mut TestAppContext) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead(&user);
    subscribe(&mcp, json!(["checks"]));
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the subscription", |cx| {
        !rows(&page, cx).is_empty()
    });
    settle(cx);
    let list = page.read_with(cx, |page, cx| page.subscriptions(cx));

    list.update(cx, |list, cx| {
        list.failed(Some("the daemon said no".into()), cx)
    });
    settle(cx);
    assert_eq!(
        label(cx, "rail-subscriptions-error").as_deref(),
        Some("the daemon said no")
    );

    list.update(cx, |list, cx| list.failed(None, cx));
    one_frame(cx);
    assert!(
        present(cx, "rail-subscriptions-error"),
        "the line is still drawn while it folds away"
    );
    settle(cx);
    assert!(
        !present(cx, "rail-subscriptions-error"),
        "and gone once folded"
    );
}

/// One frame of the clock and of drawing.
fn one_frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_failure_and_an_ending_say_their_whole_reason_and_are_never_folded(cx: &mut TestAppContext) {
    let user = user();
    script(&user, pull());
    let (lead, _mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    let token = page.read_with(cx, |page, _| page.token()).unwrap();
    let reason = "the agent's process died three times:\nfirst: exit 3\nlast: exit 3, no output";
    page.update(cx, |page, cx| {
        let event = Event {
            seq: 1 << 40,
            at: 1 << 40,
            name: "agent.failed".into(),
            data: json!({"agent": lead, "reason": reason}),
        };
        page.apply(token, SessionUpdate::Live(event), cx);
    });
    settle(cx);
    let at = blocks(&page, cx)
        .iter()
        .position(|b| matches!(b, Block::Notice { kind, .. } if kind == "failed"))
        .expect("the failure is in the conversation");

    assert!(
        !present(cx, &format!("row-{at}-notice-toggle")),
        "a failure has nothing folded behind a chevron"
    );
    assert_eq!(label(cx, &format!("row-{at}-notice")).unwrap(), reason);
}

#[gpui_kit::test]
fn a_session_that_waits_on_a_subscription_is_waiting_not_idle_in_the_sidebar_and_the_rail(
    cx: &mut TestAppContext,
) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead(&user);
    subscribe(&mcp, json!(["checks", "review"]));
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    until(&shell, cx, "the sidebar says it waits", |cx| {
        mark_of(&shell, &lead, cx) == Some(AgentState::Waiting)
    });
    // The rail's row of the agent: the state, and what it waits for, shortest.
    until(&shell, cx, "the rail's row says it waits", |cx| {
        page.read_with(cx, |page, _| {
            page.tree()
                .first()
                .is_some_and(|agent| state_of(agent) == AgentState::Waiting)
        })
    });
    let doing = page.read_with(cx, |page, _| page.tree()[0].doing.clone());
    assert_eq!(doing, "waiting for #12: checks, review");
    assert_eq!(
        label(cx, &format!("rail-waiting-{lead}")).as_deref(),
        Some("waiting for #12: checks, review")
    );

    // Ended by the user: it is at rest again.
    let id = rows(&page, cx)
        .first()
        .map(|row| row.id.clone())
        .unwrap_or_else(|| {
            user.call("subscription.list", json!({"tree": lead}))
                .unwrap()["subscriptions"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        });
    user.call("subscription.cancel", json!({"subscription": id}))
        .unwrap();
    until(&shell, cx, "the sidebar is at rest again", |cx| {
        mark_of(&shell, &lead, cx).is_none()
    });
    until(&shell, cx, "the rail's row is idle again", |cx| {
        page.read_with(cx, |page, _| {
            page.tree()
                .first()
                .is_some_and(|agent| state_of(agent) == AgentState::Idle)
        })
    });
}

#[gpui_kit::test]
fn a_repository_that_is_the_project_folder_is_named_by_the_project_in_the_rail(
    cx: &mut TestAppContext,
) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead_in(&user, ".");
    subscribe(&mcp, json!(["checks"]));
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the subscription", |cx| {
        !rows(&page, cx).is_empty()
    });
    settle(cx);

    let id = rows(&page, cx)[0].id.clone();
    assert_eq!(
        label(cx, &format!("subscription-pull-{id}")).unwrap(),
        "#12 of app"
    );
}

/// One step of the window as the platform would run it; how many animation frames were asked.
fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) -> usize {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    asked
}

/// What the window drew and asked for over ten seconds of its clock: the frames drawn, the
/// animation frames asked.
fn frames_at_rest(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (u64, usize) {
    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        asked += step(shell, Duration::from_millis(16), cx);
        gone += Duration::from_millis(16);
        std::thread::sleep(Duration::from_millis(5));
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    (drawn, asked)
}

/// Lets every motion and every late answer of the daemon end, in real time too.
fn rest(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(2) {
        step(shell, Duration::from_millis(16), cx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn a_paused_following_says_so_in_the_rail_and_resume_resumes_it(cx: &mut TestAppContext) {
    let user = user();
    script(&user, pull());
    let (lead, mcp) = lead(&user);
    // Nothing is judged: the launch starts the session at once.
    user.call("agent.mode", json!({"agent": lead, "mode": "full"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call(
        "agent_launch",
        json!({"title": "Docs", "brief": "Write the docs.", "root": true, "follow": true}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let second = Mcp::initialize(&url, &authorization);
    let session = user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Docs")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the following", |cx| {
        rows(&page, cx).iter().any(|row| row.following)
    });
    settle(cx);
    let id = rows(&page, cx)[0].id.clone();
    assert_eq!(
        label(cx, &format!("subscription-pull-{id}")).unwrap(),
        "Following Docs"
    );
    let times = label(cx, &format!("subscription-times-{id}")).unwrap();
    assert!(
        times.starts_with("since") && !times.contains("expires"),
        "{times}"
    );
    assert!(!present(cx, &format!("subscription-paused-{id}")));

    for (from, to) in [(&mcp, &session), (&second, &lead)]
        .into_iter()
        .cycle()
        .take(5)
    {
        let sent = from.call("session_send", json!({"session": to, "text": "again"}));
        assert!(!sent.error, "{}", sent.text);
    }

    until(&shell, cx, "the rail says the following paused", |cx| {
        rows(&page, cx).iter().any(|row| row.paused)
            && present(cx, &format!("subscription-paused-{id}"))
    });
    settle(cx);
    assert_eq!(
        label(cx, &format!("subscription-paused-{id}")).unwrap(),
        flottille_window::subscriptions::PAUSED
    );
    assert_eq!(
        label(cx, &format!("subscription-resume-{id}")).unwrap(),
        "Resume"
    );

    cx.update(|window, cx| window.click(format!("subscription-resume-{id}"), cx));

    until(&shell, cx, "the following resumed", |cx| {
        !rows(&page, cx).iter().any(|row| row.paused)
    });
    rest(&shell, cx);
    assert!(
        !present(cx, &format!("subscription-paused-{id}")),
        "the pause folded away"
    );
    let listed = user
        .call("subscription.list", json!({"tree": lead}))
        .unwrap();
    assert_eq!(listed["subscriptions"][0]["paused"], false, "{listed}");
    eventually("the agents at rest", || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["state"] != "working" && a["state"] != "starting")
    });
    rest(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}
