//! The scenarios of #704, the project page in tabs: Work, one column of the project's places
//! (`main` first, then the workspaces from the newest), each with its sessions and the task runs
//! going on there, the ended runs folded at the bottom; Pull requests and Tickets beside it. On
//! GPUI's test platform in front of a real daemon whose agents are the fake ACP agent, with the
//! fake `gh` and throwaway repositories. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::project::ProjectPage;
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, ElementId, Entity, Pixels, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work briefly", "do": [{"wait": 1500}, {"say": "done"}]},
        {"when": "lead the docs", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"kind": "led", "title": "Docs", "brief": "Write the docs",
                          "workspace": "docs"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The project `name` over a throwaway repository holding `say.sh` (prints two lines and ends)
/// and `wait.sh` (runs until it is stopped), committed so that each workspace has them too.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    for (script, body) in [
        ("say.sh", "#!/bin/sh\necho first\necho second\n"),
        ("wait.sh", "#!/bin/sh\necho waiting\nexec sleep 60\n"),
    ] {
        let path = folder.join(script);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn task(user: &User, project: &str, name: &str, line: &str) {
    user.call(
        "task.add",
        json!({"project": project, "name": name, "line": line}),
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

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn workspace(user: &User, project: &str, name: &str) {
    user.call(
        "workspace.create",
        json!({"project": project, "name": name}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": project, "name": name}))
            .unwrap()["state"]
            == "ready"
    });
}

/// Starts a session in `workspace` (main when none) and waits until its first turn ended.
fn start(user: &User, project: &str, workspace: Option<&str>, message: &str) -> String {
    let mut params = json!({"project": project, "message": message});
    if let Some(workspace) = workspace {
        params["workspace"] = json!(workspace);
    }
    let answer = user.call("agent.start", params).unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
}

/// The runs of the project, the agents' and the user's, the ended ones too.
fn runs(user: &User, project: &str) -> Vec<Value> {
    user.call("run.list", json!({"project": project, "ended": true}))
        .unwrap()["commands"]
        .as_array()
        .unwrap()
        .clone()
}

/// Runs a task as the user does, in `workspace` (main when none): the run's id.
fn run_task(user: &User, project: &str, name: &str, workspace: Option<&str>) -> String {
    let mut params = json!({"project": project, "name": name});
    if let Some(workspace) = workspace {
        params["workspace"] = json!(workspace);
    }
    let answer = user.call("task.run", params).unwrap();
    answer["run"]["run"]
        .as_str()
        .or_else(|| answer["run"].as_str())
        .expect("the run's id")
        .to_owned()
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
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1600.)));
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
        cx.executor().advance_clock(Duration::from_millis(50));
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
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open_project(shell: &Entity<Shell>, project: &str, cx: &mut VisualTestContext) {
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
    settle(cx);
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ProjectPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).project_page().cloned())
        .expect("a project's page is open")
}

fn has(id: impl Into<ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn bounds(id: impl Into<ElementId>, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    let id = id.into();
    cx.update(|window, _| window.find(id).bounds())
}

fn label(id: impl Into<ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn count(id: impl Into<ElementId>, cx: &mut VisualTestContext) -> usize {
    let id = id.into();
    cx.update(|window, _| window.find_all(id).len())
}

/// The places of Work, top to bottom, as they are drawn.
fn places_drawn(names: &[&str], cx: &mut VisualTestContext) -> Vec<String> {
    let mut drawn: Vec<(Pixels, String)> = Vec::new();
    for name in names {
        if has(format!("place-{name}"), cx) {
            drawn.push((
                bounds(format!("place-{name}"), cx).origin.y,
                name.to_string(),
            ));
        }
    }
    drawn.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    drawn.into_iter().map(|(_, name)| name).collect()
}

/// Whether the element `id` is drawn inside the block of the place `place`.
fn inside(id: impl Into<ElementId>, place: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    let (row, block) = (bounds(id, cx), bounds(format!("place-{place}"), cx));
    row.origin.y >= block.origin.y && row.bottom() <= block.bottom()
}

fn session_row(id: &str) -> String {
    format!("place-session-{id}")
}

#[gpui_kit::test]
fn the_project_page_has_work_pull_requests_and_tickets_tabs(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);

    for tab in ["work", "pulls", "tickets"] {
        assert!(has(format!("project-tabs-{tab}"), cx), "the tab {tab}");
    }
    assert!(
        !has("project-tabs-specs", cx),
        "Specs is off in the project"
    );
    // Work is chosen: the places are drawn, the lists are not.
    assert!(has("project-work", cx));
    assert!(has("place-main", cx));
    assert!(!has("project-items-list-block", cx));
    // What the page no longer says.
    for gone in [
        "project-places",
        "project-tasks",
        "project-runs",
        "new-workspace-card",
        "new-session-in-main",
    ] {
        assert!(!has(gone, cx), "{gone} is gone");
    }

    cx.update(|window, cx| window.click("project-tabs-pulls", cx));
    until(
        &shell,
        cx,
        "the pull requests' tab says there is no forge",
        |cx| has("project-items-not-set", cx),
    );
    settle(cx);
    assert!(!has("project-work", cx), "one tab at a time");
    assert_eq!(
        label("project-items-not-set", cx).as_deref(),
        Some("No forge set for this project.")
    );
    cx.update(|window, cx| window.click("project-tabs-work", cx));
    settle(cx);
    assert!(has("project-work", cx));
}

#[gpui_kit::test]
fn work_lists_main_first_then_workspaces_newest_first(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    for name in ["beta", "alpha", "gamma"] {
        workspace(&user, "atlas", name);
        // Each a millisecond apart at least.
        std::thread::sleep(Duration::from_millis(5));
    }
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the places are drawn", |cx| {
        has("place-gamma", cx)
    });
    settle(cx);

    assert_eq!(
        places_drawn(&["main", "alpha", "beta", "gamma"], cx),
        ["main", "gamma", "alpha", "beta"]
    );
}

#[gpui_kit::test]
fn activity_does_not_reorder_the_places(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "older");
    std::thread::sleep(Duration::from_millis(5));
    workspace(&user, "atlas", "newer");
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the places are drawn", |cx| {
        has("place-newer", cx)
    });
    settle(cx);
    let before = places_drawn(&["main", "older", "newer"], cx);
    assert_eq!(before, ["main", "newer", "older"]);

    // A session works in the oldest workspace, then in main: nothing moves.
    let id = start(&user, "atlas", Some("older"), "work briefly");
    until(&shell, cx, "its row is in its place", |cx| {
        has(session_row(&id), cx)
    });
    settle(cx);
    assert_eq!(places_drawn(&["main", "older", "newer"], cx), before);
    let other = start(&user, "atlas", None, "work briefly");
    until(&shell, cx, "its row is in main", |cx| {
        has(session_row(&other), cx)
    });
    settle(cx);
    assert_eq!(places_drawn(&["main", "older", "newer"], cx), before);
}

#[gpui_kit::test]
fn a_session_is_listed_once_in_the_place_it_works_in(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "alpha");
    let in_main = start(&user, "atlas", None, "Release notes");
    let in_alpha = start(&user, "atlas", Some("alpha"), "Fix the login");
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "both sessions are listed", |cx| {
        has(session_row(&in_main), cx) && has(session_row(&in_alpha), cx)
    });
    settle(cx);

    assert_eq!(count(session_row(&in_main), cx), 1);
    assert_eq!(count(session_row(&in_alpha), cx), 1);
    assert!(inside(session_row(&in_main), "main", cx));
    assert!(inside(session_row(&in_alpha), "alpha", cx));
    assert!(!inside(session_row(&in_alpha), "main", cx));

    // A click opens it.
    cx.update(|window, cx| window.click(session_row(&in_alpha), cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(in_alpha))
    );
}

#[gpui_kit::test]
fn a_lead_says_whom_it_leads_and_a_led_session_its_lead(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "docs");
    let lead = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": "lead the docs"}),
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
    let led = || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["led_by"] == lead.as_str())
            .and_then(|agent| agent["id"].as_str().map(str::to_owned))
    };
    eventually("the led session exists", || led().is_some());
    let led = led().unwrap();
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "both sessions are listed", |cx| {
        has(format!("place-leads-{lead}"), cx) && has(format!("place-led-by-{led}"), cx)
    });
    settle(cx);

    assert!(inside(session_row(&lead), "main", cx));
    assert!(inside(session_row(&led), "docs", cx));
    assert_eq!(
        label(format!("place-leads-{lead}"), cx).as_deref(),
        Some("leads 1: docs")
    );
    assert!(
        label(format!("place-led-by-{led}"), cx).is_some_and(|said| said.starts_with("led by ")),
        "{:?}",
        label(format!("place-led-by-{led}"), cx)
    );

    // The place is a link to its block, the lead a link to its session.
    cx.update(|window, cx| window.click(format!("place-leads-{lead}-docs"), cx));
    settle(cx);
    assert!(
        matches!(
            shell.read_with(cx, |s, _| s.selected().cloned()),
            Some(Selection::Project(_))
        ),
        "the page stays"
    );
    cx.update(|window, cx| window.click(format!("place-led-by-{led}"), cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(lead))
    );
}

#[gpui_kit::test]
fn run_a_task_here_runs_it_in_that_workspace(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    task(&user, "atlas", "say", "./say.sh");
    workspace(&user, "atlas", "alpha");
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the place is drawn", |cx| {
        has("place-alpha", cx)
    });
    settle(cx);

    cx.update(|window, cx| window.hover("place-row-alpha", cx));
    settle(cx);
    cx.update(|window, cx| window.click("place-run-alpha", cx));
    settle(cx);
    cx.update(|window, cx| window.click("place-run-alpha-say", cx));
    eventually("the task ran", || !runs(&user, "atlas").is_empty());
    let ran = runs(&user, "atlas");
    assert_eq!(ran[0]["name"], "say");
    assert_eq!(
        ran[0]["workspace"], "alpha",
        "in the workspace of the place"
    );
}

#[gpui_kit::test]
fn a_running_task_shows_in_its_place_with_stop(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    task(&user, "atlas", "wait", "./wait.sh");
    workspace(&user, "atlas", "alpha");
    let run = run_task(&user, "atlas", "wait", Some("alpha"));
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    let row = format!("place-run-row-{run}");
    until(&shell, cx, "the run is in its place", |cx| {
        has(row.clone(), cx)
    });
    settle(cx);
    assert!(inside(row.clone(), "alpha", cx));
    assert!(!inside(row.clone(), "main", cx));
    assert_eq!(
        label(format!("command-stop-{run}"), cx).as_deref(),
        Some("Stop wait")
    );

    cx.update(|window, cx| window.click(format!("command-stop-{run}"), cx));
    eventually("the run is stopped", || {
        runs(&user, "atlas")
            .iter()
            .any(|r| r["run"] == run.as_str() && r["state"] != "running")
    });
    until(&shell, cx, "its row left its place", |cx| {
        !has(row.clone(), cx)
    });
}

#[gpui_kit::test]
fn ended_runs_are_folded_at_the_bottom_with_their_place(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    task(&user, "atlas", "say", "./say.sh");
    workspace(&user, "atlas", "alpha");
    let in_main = run_task(&user, "atlas", "say", None);
    let in_alpha = run_task(&user, "atlas", "say", Some("alpha"));
    eventually("both ended", || {
        runs(&user, "atlas")
            .iter()
            .filter(|r| r["state"] != "running")
            .count()
            == 2
    });
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the ended runs are said", |cx| {
        has("project-ended-runs", cx)
    });
    settle(cx);

    assert_eq!(
        label("project-ended-runs", cx).as_deref(),
        Some("2 ended runs")
    );
    assert!(
        bounds("project-ended-runs", cx).origin.y > bounds("place-alpha", cx).bottom(),
        "at the bottom of Work"
    );
    assert_eq!(
        bounds("project-ended-fold", cx).size.height,
        px(0.),
        "folded"
    );

    cx.update(|window, cx| window.click("project-ended-runs", cx));
    settle(cx);
    assert_eq!(
        label(format!("ended-run-{in_main}-place"), cx).as_deref(),
        Some("in main")
    );
    assert_eq!(
        label(format!("ended-run-{in_alpha}-place"), cx).as_deref(),
        Some("in alpha")
    );

    // A click opens its drawer.
    cx.update(|window, cx| window.click(format!("ended-run-{in_alpha}"), cx));
    settle(cx);
    let page = page_of(&shell, cx);
    assert_eq!(
        page.read_with(cx, |p, cx| p
            .tasks()
            .read(cx)
            .panel()
            .read(cx)
            .run()
            .map(str::to_owned)),
        Some(in_alpha)
    );
}

#[gpui_kit::test]
fn an_empty_workspace_is_one_row_saying_no_session(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "alpha");
    workspace(&user, "atlas", "busy");
    let id = start(&user, "atlas", Some("busy"), "Fix the login");
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the places are drawn", |cx| {
        has("place-alpha", cx) && has(session_row(&id), cx)
    });
    settle(cx);

    assert_eq!(
        label("place-no-session-alpha", cx).as_deref(),
        Some("no session")
    );
    assert!(!has("place-no-session-busy", cx));
    assert!(!has("place-no-session-main", cx), "main is no workspace");
    // Its row and its second line, nothing under them.
    let (block, row) = (bounds("place-alpha", cx), bounds("place-row-alpha", cx));
    assert!(
        block.size.height < row.size.height * 3.,
        "{block:?} around {row:?}"
    );
}

#[gpui_kit::test]
fn a_pull_request_of_a_workspace_says_in_which(cx: &mut TestAppContext) {
    let user = user().with_fake_gh();
    project(&user, "app");
    let folder = user.path("work/app");
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [
            {"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}
        ]}),
    )
    .unwrap();
    workspace(&user, "app", "checkout-retry");
    let pull = |number: u64, title: &str, branch: &str| {
        json!({
            "number": number, "title": title, "state": "OPEN", "isDraft": false,
            "headRefName": branch, "baseRefName": "main",
            "headRefOid": "0123456789abcdef0123456789abcdef01234567",
            "url": format!("https://github.com/acme/app/pull/{number}"),
            "author": {"login": "someone"}, "assignees": [], "labels": [],
            "updatedAt": "2020-01-01T00:00:00Z", "mergeable": "MERGEABLE",
            "reviewDecision": null, "statusCheckRollup": [], "comments": [], "reviews": [],
            "body": ""
        })
    };
    user.gh_script(json!({"pulls": {"acme/app": {
        "app/checkout-retry": pull(7, "Retry the checkout", "app/checkout-retry"),
        "elsewhere": pull(8, "Something else", "elsewhere")
    }}}));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    open_project(&shell, "app", cx);
    // The workspace's pull request is known on Work first, then said in the list.
    until(&shell, cx, "the workspace's pull request", |cx| {
        has("place-pull-checkout-retry", cx)
    });
    cx.update(|window, cx| window.click("project-tabs-pulls", cx));
    until(&shell, cx, "the pull requests are listed", |cx| {
        has("item-pull---7-facts", cx) && has("item-pull---8-facts", cx)
    });
    settle(cx);

    assert!(
        label("item-pull---7-facts", cx).is_some_and(|said| said.contains("in checkout-retry")),
        "{:?}",
        label("item-pull---7-facts", cx)
    );
    assert!(
        label("item-pull---8-facts", cx).is_some_and(|said| !said.contains(" in ")),
        "{:?}",
        label("item-pull---8-facts", cx)
    );
}
