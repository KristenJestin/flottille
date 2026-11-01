//! The scenarios of the rail's Workspace facts (#230), on GPUI's test platform in front of a real
//! daemon whose agents are the fake ACP agent and whose forge is the fake `gh`: the ticket comes
//! before the pull request, the icons share one column, the labels start at one x, the values end
//! at one right edge and are drawn at one text size. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::session::SessionPage;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const TITLE: &str = "Write a README that says what the project is for and how to start";

fn user() -> User {
    User::with_fake_agent().with_fake_gh()
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
    let repository = "api";
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

/// The workspace `login` with the pull request #12 of its branch and the ticket #7 linked.
fn with_ticket_and_pull(user: &User) {
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
            "tracker": {"kind": "github", "repository": "api"}}),
    )
    .unwrap();
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": {
            "number": 12, "title": "Add the login page", "state": "OPEN", "isDraft": false,
            "headRefOid": "aaaaaaa1", "baseRefName": "main",
            "url": "https://github.com/acme/api/pull/12",
            "statusCheckRollup": [
                {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"}
            ],
            "comments": [], "reviews": []
        }}},
        "issues": {"acme/api": {"7": {
            "number": 7, "title": TITLE, "state": "OPEN", "body": "", "comments": [],
            "url": "https://github.com/acme/api/issues/7"
        }}}
    }));
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": "#7"}),
    )
    .unwrap();
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

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn rect(cx: &mut VisualTestContext, id: &str) -> Bounds<Pixels> {
    cx.update(|window, _| window.find(id.to_owned()).bounds())
}

/// The session's page open on the rail with its ticket, its pull request and its facts drawn.
fn open_rail<'a>(user: &User, lead: &str, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(
        &shell,
        cx,
        "the ticket and the pull request in the rail",
        |cx| {
            let _ = session_page(&shell, cx);
            present(cx, "rail-ticket")
                && present(cx, "rail-pull-api")
                && present(cx, "rail-fact-Machine")
        },
    );
    settle(cx);
    cx
}

#[gpui_kit::test]
fn the_session_rail_lists_the_ticket_above_the_pull_request(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    with_ticket_and_pull(&user);
    let cx = open_rail(&user, &lead, cx);

    let ticket = rect(cx, "rail-line-ticket");
    let pull = rect(cx, "rail-line-pull-api");
    assert!(
        ticket.bottom() <= pull.origin.y,
        "the ticket is why the workspace exists: {ticket:?} above {pull:?}"
    );
}

#[gpui_kit::test]
fn the_icons_of_the_repository_the_ticket_and_the_pull_request_share_one_x_and_so_do_their_labels(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, _) = lead(&user);
    with_ticket_and_pull(&user);
    let cx = open_rail(&user, &lead, cx);

    let icons = [
        "rail-repository-api-icon",
        "rail-line-ticket-icon",
        "rail-line-pull-api-icon",
        "rail-fact-Machine-icon",
        "rail-fact-Prepared-icon",
    ];
    let at: Vec<_> = icons.iter().map(|id| rect(cx, id).origin.x).collect();
    assert!(
        at.iter().all(|x| *x == at[0]),
        "one column for the icons: {icons:?} {at:?}"
    );
    // A fact with no icon keeps its place in the column, so its label starts where the others do.
    let labels = [
        "rail-repository-api-label",
        "rail-line-ticket-label",
        "rail-line-pull-api-label",
        "rail-fact-Machine-label",
        "rail-fact-Prepared-label",
    ];
    let at: Vec<_> = labels.iter().map(|id| rect(cx, id).origin.x).collect();
    assert!(
        at.iter().all(|x| *x == at[0]),
        "the labels start at one x: {labels:?} {at:?}"
    );
}

#[gpui_kit::test]
fn every_fact_s_value_ends_at_the_same_right_edge_and_truncates_before_its_label(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, _) = lead(&user);
    with_ticket_and_pull(&user);
    let cx = open_rail(&user, &lead, cx);

    let values = [
        "rail-repository-api-value",
        "rail-ticket",
        "rail-pull-api",
        "rail-fact-Machine-value",
        "rail-fact-Prepared-value",
    ];
    let right: Vec<_> = values.iter().map(|id| rect(cx, id).right()).collect();
    assert!(
        right.iter().all(|edge| *edge == right[0]),
        "one right edge: {values:?} {right:?}"
    );
    let label = rect(cx, "rail-line-ticket-label");
    let value = rect(cx, "rail-ticket");
    assert!(
        value.origin.x >= label.right(),
        "a long title is cut before it reaches its label: {value:?} after {label:?}"
    );
}

#[gpui_kit::test]
fn the_values_of_the_ticket_and_the_pull_request_are_drawn_at_the_size_of_the_other_facts(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, _) = lead(&user);
    with_ticket_and_pull(&user);
    let cx = open_rail(&user, &lead, cx);

    // The line height follows the text size (`mono(size)` is 1.4 times it): one height, one size.
    let others = rect(cx, "rail-fact-Machine-value").size.height;
    for id in ["rail-ticket", "rail-pull-api", "rail-fact-Prepared-value"] {
        assert_eq!(rect(cx, id).size.height, others, "{id}");
    }
}

#[gpui_kit::test]
fn the_project_page_s_workspace_card_lists_the_ticket_above_the_pull_request(
    cx: &mut TestAppContext,
) {
    let user = user();
    lead(&user);
    with_ticket_and_pull(&user);
    let (shell, cx) = window(&user, cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(
        &shell,
        cx,
        "the ticket and the pull request on the card",
        |cx| present(cx, "ticket-login") && present(cx, "pull-login-api"),
    );
    settle(cx);

    let ticket = rect(cx, "ticket-login");
    let pull = rect(cx, "pull-login-api");
    assert!(
        ticket.bottom() <= pull.origin.y,
        "{ticket:?} above {pull:?}"
    );
}
