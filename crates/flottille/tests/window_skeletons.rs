//! The scenarios of the rail waiting for slow reads (#386), on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent and whose forge is a fake `gh` that answers
//! after a delay, as a network does: the runs never wait for the forge, the workspace shows what
//! git knows at once, each pending part shows a skeleton that its content replaces in place, and
//! a part that failed says why where its skeleton was. No window opens on a screen.

mod common;

use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Update;
use flottille_window::session::SessionPage;
use flottille_window::session_link::{self, SessionUpdate, Where};
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const TITLE: &str = "Write a README that says what the project is for and how to start";

/// How long the fake `gh` takes to answer: long next to what the window does with an answer it has.
const SLOW_MS: u64 = 3000;

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

/// The project `app` (repository `api` on GitHub `acme/api`), its ready workspace `login` linked to
/// the ticket #7, a server running in it, and a lead the user started there: its id.
fn lead(user: &User) -> String {
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
        json!({"name": "app",
            "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
            "tracker": {"kind": "github", "repository": "api"}}),
    )
    .unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[commands.server]\nline = \"./server.sh\"\nserve = true\n");
    std::fs::write(&file, text).unwrap();
    let folder = PathBuf::from(
        user.call("workspace.show", json!({"project": "app", "name": "login"}))
            .unwrap()["folder"]
            .as_str()
            .unwrap(),
    );
    let script = folder.join("server.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho listening on 8080\nwhile true; do sleep 1; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user.gh_script(forge(None));
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": "#7"}),
    )
    .unwrap();
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
    let mcp = Mcp::initialize(&url, &authorization);
    let started = mcp.call("run", json!({"task": "server", "timeout": 1}));
    assert!(!started.error, "{}", started.text);
    id
}

/// The fake `gh`'s script: the pull request #12 and the issue #7, each call taking `slow` ms; a
/// `failure` makes every call end with it, after the same wait.
fn forge(slow: Option<(u64, Option<&str>)>) -> Value {
    let mut script = json!({
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
    });
    if let Some((ms, failure)) = slow {
        script["read_delay_ms"] = json!(ms);
        if let Some(failure) = failure {
            script["fail"] = json!({"acme/api": failure});
        }
    }
    script
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
    let deadline = Instant::now() + Duration::from_secs(40);
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

fn listed(cx: &mut VisualTestContext, lead: &str) -> bool {
    cx.update(|window, _| window.try_find(session_id(lead)).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Opens the session `lead` as the user does, from the tree, once the forge is slow.
fn open<'a>(
    user: &User,
    lead: &str,
    slow: (u64, Option<&str>),
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    // Slow before the window opens: the project page's details and the rail both ask the forge.
    user.gh_script(forge(Some(slow)));
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        listed(cx, lead)
    });
    cx.update(|window, cx| window.click(session_id(lead), cx));
    (shell, cx)
}

#[gpui_kit::test]
fn the_runs_show_before_the_forge_answers(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, cx) = open(&user, &lead, (SLOW_MS, None), cx);

    until(&shell, cx, "the run in the rail", |cx| {
        let page = session_page(&shell, cx);
        page.read_with(cx, |page, cx| !page.commands(cx).read(cx).rows().is_empty())
    });
    let pulls = session_page(&shell, cx).read_with(cx, |page, cx| {
        page.rail()
            .read(cx)
            .place()
            .map(|place| place.pulls.len())
            .unwrap_or(0)
    });
    assert_eq!(
        pulls, 0,
        "the run was in the rail while the forge was still being read"
    );
    until(&shell, cx, "the pull request, at last", |cx| {
        present(cx, "rail-pull-api")
    });
}

#[gpui_kit::test]
fn the_workspace_shows_before_the_forge_and_the_ticket(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, cx) = open(&user, &lead, (SLOW_MS, None), cx);

    until(&shell, cx, "the repository in the rail", |cx| {
        present(cx, "rail-repository-api") && present(cx, "rail-fact-Machine")
    });
    assert!(
        !present(cx, "rail-ticket") && !present(cx, "rail-pull-api"),
        "what git knows is drawn while the ticket and the pull request are still read"
    );
    until(&shell, cx, "the ticket and the pull request", |cx| {
        present(cx, "rail-ticket") && present(cx, "rail-pull-api")
    });
    settle(cx);
    let _ = session_page(&shell, cx);
}

#[gpui_kit::test]
fn a_pending_part_shows_a_skeleton_then_its_content(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, cx) = open(&user, &lead, (SLOW_MS, None), cx);

    until(&shell, cx, "the repository in the rail", |cx| {
        present(cx, "rail-repository-api")
    });
    assert!(
        present(cx, "rail-ticket-skeleton"),
        "the ticket line waits as a skeleton"
    );
    assert!(
        present(cx, "rail-pull-skeleton-api"),
        "so does the pull request's row"
    );
    until(&shell, cx, "the ticket and the pull request", |cx| {
        present(cx, "rail-ticket") && present(cx, "rail-pull-api")
    });
    settle(cx);
    assert!(
        !present(cx, "rail-ticket-skeleton") && !present(cx, "rail-pull-skeleton-api"),
        "the content took the skeletons' place"
    );
    assert_eq!(
        label(cx, "rail-ticket")
            .as_deref()
            .map(|l| l.contains(TITLE)),
        Some(true)
    );
}

#[gpui_kit::test]
fn the_whole_workspace_and_runs_sections_wait_as_skeletons_before_their_first_answer(
    cx: &mut TestAppContext,
) {
    let user = user();
    let lead = lead(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        listed(cx, &lead)
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    // The page is open, nothing was read yet.
    let _ = session_page(&shell, cx);
    assert!(present(cx, "rail-workspace-skeleton"), "Workspace waits");
    assert!(present(cx, "rail-runs-skeleton"), "Runs waits");
    until(&shell, cx, "the facts and the run", |cx| {
        present(cx, "rail-fact-Machine") && present(cx, "rail-runs")
    });
    settle(cx);
    assert!(!present(cx, "rail-workspace-skeleton") && !present(cx, "rail-runs-skeleton"));
}

#[gpui_kit::test]
fn a_failed_part_says_why_in_place_of_its_skeleton(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    // Linking the ticket read it, and a read is given again for 30 s (`docs/tickets.md`): the
    // window opens later than that.
    user.call("test.advance", json!({"ms": 31_000})).unwrap();
    let (shell, cx) = open(&user, &lead, (1500, Some("HTTP 502: Bad Gateway")), cx);

    until(&shell, cx, "the repository in the rail", |cx| {
        present(cx, "rail-repository-api")
    });
    assert!(present(cx, "rail-ticket-skeleton"), "it waits first");
    until(&shell, cx, "the reason", |cx| present(cx, "rail-ticket"));
    settle(cx);
    assert!(
        !present(cx, "rail-ticket-skeleton"),
        "the reason took the skeleton's place"
    );
    let said = label(cx, "rail-ticket").unwrap_or_default();
    assert!(
        said.contains("#7") && !said.contains(TITLE),
        "the ticket line says it could not be read, in the skeleton's place: {said}"
    );
    assert!(
        !present(cx, "rail-pull-skeleton-api"),
        "the pull request's skeleton is gone with nothing in its place"
    );
}

/// The page of the project `app`, open on its cards, the forge slow.
fn open_project<'a>(
    user: &User,
    slow: (u64, Option<&str>),
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    user.gh_script(forge(Some(slow)));
    let (shell, cx) = window(user, cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    (shell, cx)
}

/// What the workspace `login`'s card knows: its lines changed and its pull requests.
fn card_knows(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<(bool, usize)> {
    shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).project_page()?.clone();
        let card = page
            .read(cx)
            .cards()
            .iter()
            .find(|card| card.read(cx).data().place.as_deref() == Some("login"))?
            .clone();
        let card = card.read(cx);
        Some((card.changes().is_some(), card.pulls().len()))
    })
}

#[gpui_kit::test]
fn the_project_page_s_card_shows_what_git_knows_before_the_forge_answers(cx: &mut TestAppContext) {
    let user = user();
    lead(&user);
    let (shell, cx) = open_project(&user, (SLOW_MS, None), cx);

    until(&shell, cx, "the card's lines changed", |cx| {
        card_knows(&shell, cx).is_some_and(|(changes, _)| changes)
    });
    let (_, pulls) = card_knows(&shell, cx).unwrap();
    assert_eq!(
        pulls, 0,
        "the card knew its changes while the forge was still being read"
    );
    until(&shell, cx, "the pull request, at last", |cx| {
        present(cx, "pull-login-api")
    });
}

#[gpui_kit::test]
fn the_card_waits_with_skeletons_for_its_ticket_and_pull_request_then_shows_them(
    cx: &mut TestAppContext,
) {
    let user = user();
    lead(&user);
    let (shell, cx) = open_project(&user, (SLOW_MS, None), cx);

    until(&shell, cx, "the card's skeletons", |cx| {
        present(cx, "ticket-skeleton-login")
    });
    assert!(present(cx, "pull-skeleton-login-api"));
    assert!(!present(cx, "ticket-login") && !present(cx, "pull-login-api"));
    until(&shell, cx, "the ticket and the pull request", |cx| {
        present(cx, "ticket-login") && present(cx, "pull-login-api")
    });
    settle(cx);
    assert!(!present(cx, "ticket-skeleton-login") && !present(cx, "pull-skeleton-login-api"));
}

#[gpui_kit::test]
fn a_card_whose_repository_has_no_pull_request_loses_its_skeleton(cx: &mut TestAppContext) {
    let user = user();
    lead(&user);
    let mut script = forge(Some((1500, None)));
    script["pulls"] = json!({});
    let (shell, cx) = {
        user.gh_script(script);
        let (shell, cx) = window(&user, cx);
        shell.update_in(cx, |s, window, cx| {
            s.open(Selection::Project("app".into()), window, cx)
        });
        (shell, cx)
    };
    until(&shell, cx, "the skeleton", |cx| {
        present(cx, "pull-skeleton-login-api")
    });
    until(&shell, cx, "the ticket", |cx| present(cx, "ticket-login"));
    settle(cx);
    assert!(
        !present(cx, "pull-skeleton-login-api") && !present(cx, "pull-login-api"),
        "nothing is left where the pull request was waited for"
    );
}

/// The next `Place` the link sends; the other updates it takes on the way go to `seen`.
fn next_place(
    updates: &mut futures::channel::mpsc::UnboundedReceiver<Update>,
    seen: &mut Vec<SessionUpdate>,
) -> Box<flottille_window::session_link::PlaceFacts> {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match updates.try_recv() {
            Ok(Update::Session { update, .. }) => {
                if let SessionUpdate::Place(Some(facts)) = update {
                    return facts;
                }
                seen.push(update);
            }
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "the place never came");
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

#[test]
fn a_reconnect_keeps_the_known_pull_requests_and_ticket() {
    let user = user();
    let lead = lead(&user);
    let streams: Arc<Mutex<Vec<UnixStream>>> = Arc::default();
    let connector = user.connector();
    let kept = streams.clone();
    let connect: flottille_window::Connect = Arc::new(move || {
        let stream = connector()?;
        kept.lock().unwrap().push(stream.try_clone()?);
        Ok(stream)
    });
    let (sender, mut updates) = futures::channel::mpsc::unbounded();
    let _link = session_link::spawn(
        connect,
        sender,
        lead.clone(),
        lead,
        Where {
            project: "app".into(),
            workspace: Some("login".into()),
        },
        1,
    );
    let mut seen = Vec::new();
    let first = next_place(&mut updates, &mut seen);
    assert!(first.forge_pending && first.ticket_pending, "it is read");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        while let Ok(Update::Session { update, .. }) = updates.try_recv() {
            seen.push(update);
        }
        if seen
            .iter()
            .any(|u| matches!(u, SessionUpdate::PlaceTicket(Some(_))))
            && seen
                .iter()
                .any(|u| matches!(u, SessionUpdate::PlaceForge(_)))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the forge and the ticket never came"
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    // The socket drops, as when the daemon restarts: the link connects again by itself.
    for stream in streams.lock().unwrap().drain(..) {
        let _ = stream.shutdown(Shutdown::Both);
    }
    let again = next_place(&mut updates, &mut seen);
    assert!(
        !again.forge_pending && !again.ticket_pending,
        "the pull requests and the ticket flashed back to skeletons over what was shown"
    );
    assert_eq!(again.pulls.len(), 1);
    assert!(again.ticket.is_some());
}

#[gpui_kit::test]
fn an_unlinked_ticket_folds_away_instead_of_vanishing(cx: &mut TestAppContext) {
    let user = user();
    let lead = lead(&user);
    let (shell, cx) = open(&user, &lead, (0, None), cx);
    until(&shell, cx, "the ticket", |cx| present(cx, "rail-ticket"));
    settle(cx);

    cx.update(|window, cx| window.click("workspace-menu", cx));
    settle(cx);
    cx.update(|window, cx| window.click("workspace-unlink-ticket", cx));
    until(&shell, cx, "the ticket forgotten by the rail", |cx| {
        session_page(&shell, cx).read_with(cx, |page, cx| {
            page.rail()
                .read(cx)
                .place()
                .is_some_and(|place| place.ticket.is_none())
        })
    });
    assert!(
        present(cx, "rail-ticket"),
        "the line vanished at once instead of folding away"
    );
    settle(cx);
    assert!(!present(cx, "rail-ticket"), "it is gone once folded");
    assert!(!present(cx, "rail-line-ticket"));
}
