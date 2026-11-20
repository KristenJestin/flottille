//! A user clicking around the window: from every page there is, every navigable element on screen
//! is clicked, and the window must still run after each (#997). No window opens on a screen.

use crate::common::User;
use crate::common::mcp::Mcp;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flottille_window::Shell;
use flottille_window::history::Way;
use flottille_window::page::Page;
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const OLD: &str = "2020-01-01T00:00:00Z";

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn issue(number: u64, title: &str, state: &str, milestone: Option<(u64, &str)>) -> Value {
    json!({
        "number": number, "title": title, "state": state, "body": "",
        "url": format!("https://github.com/acme/app/issues/{number}"),
        "author": {"login": "someone"}, "assignees": [{"login": "someone"}], "labels": [],
        "milestone": milestone.map(|(n, t)| json!({"number": n, "title": t})),
        "updatedAt": OLD, "comments": []
    })
}

/// A throwaway repository at `folder`, its `origin` `remote` when there is one.
fn repository(user: &User, folder: &PathBuf, remote: Option<&str>) {
    user.git_init(folder);
    std::fs::write(folder.join("README.md"), "text\n").unwrap();
    user.git(folder, &["add", "."]);
    user.git(folder, &["commit", "--quiet", "-m", "start"]);
    if let Some(remote) = remote {
        user.git(folder, &["remote", "add", "origin", remote]);
    }
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

/// What the fixture holds that the sweep needs to name.
struct Fixture {
    user: User,
    spec: u64,
}

/// Two projects: `app` (a GitHub tracker with milestones, so a Progress tab; Specs on, a spec; a
/// lead session with a child) and `web` (a bare repository, one session of its own).
fn fixture() -> Fixture {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fetcher()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
        .with_env("GIT_SSH_COMMAND", "false");
    user.fake_script(json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "REPORT", "do": [{"mcp": {"tool": "report", "arguments": {"text": "done"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();

    repository(
        &user,
        &user.path("work/app"),
        Some("git@github.com:acme/app.git"),
    );
    repository(&user, &user.path("work/web"), None);
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    user.call(
        "project.add",
        json!({"folder": user.path("work/web"), "name": "web"}),
    )
    .unwrap();
    user.gh_script(json!({
        "milestones": {"acme/app": [
            {"number": 1, "title": "v0.3", "state": "open", "due_on": null,
                "open_issues": 2, "closed_issues": 0,
                "html_url": "https://github.com/acme/app/milestone/1"}
        ]},
        "issues": {"acme/app": {
            "1": issue(1, "Sign a connection in", "OPEN", Some((1, "v0.3"))),
            "2": issue(2, "Bind a vault entry", "OPEN", Some((1, "v0.3")))
        }},
        "pulls": {"acme/app": {}},
    }));
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    user.call(
        "extension.set",
        json!({"name": "specs", "enabled": true, "project": null}),
    )
    .unwrap();

    // A lead with a child in `app`, and a session in `web`.
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "w", "offline": true}),
        )
        .unwrap();
    assert_eq!(made["state"], "ready", "{made}");
    let lead = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "lead the work"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mcp = mcp_session(&user, |prompt| prompt.contains("You are the root"));
    eventually("the lead's first turn", || {
        user.call("agent.show", json!({"agent": lead})).unwrap()["agent"]["state"] == "idle"
    });
    let child = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "Child", "brief": "REPORT and Child", "workspace": "w"}),
    );
    assert!(!child.error, "{}", child.text);
    user.call(
        "agent.run",
        json!({"project": "web", "message": "REPORT in web"}),
    )
    .unwrap();
    let spec = user
        .call(
            "spec.new",
            json!({"project": "app", "idea": "Let users log in with a link sent by mail"}),
        )
        .unwrap()["spec"]["spec"]
        .as_u64()
        .unwrap();
    Fixture { user, spec }
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
    cx.simulate_resize(size(px(1440.), px(900.)));
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
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

/// How the sweep sees an element of the window by its id.
#[derive(Debug, PartialEq)]
enum Kind {
    /// A click on it navigates: the sweep clicks it.
    Navigates,
    /// A click on it does something else (a dialog, a menu, a field, the system's handler): not
    /// swept, and said so.
    Left,
}

/// The projects of the fixture, as their sidebar rows are named.
const PROJECTS: [&str; 2] = ["app", "web"];

/// Elements that navigate, by exact id: the top bar, the headers.
const NAVIGATES: [&str; 6] = [
    "settings",
    "services",
    "new-session",
    "project-settings",
    "settings-crumb",
    "spec-crumb",
];

/// Elements whose click is not a navigation, by exact id.
const LEFT: [&str; 16] = [
    "progress-open",
    "bell",
    "top-search",
    "add-project",
    "new-workspace",
    "project-header-path",
    "project-settings-header-path",
    "attach",
    "send",
    "reply-send",
    "providers-check",
    "add-service",
    "session-rename",
    "workspace-menu",
    "session-place",
    "project-more",
];

/// Elements that are not navigation, by the start of their id.
const LEFT_PREFIXES: [&str; 3] = ["tree-row-", "project-items-state-", "provider-"];

fn is_hex12(id: &str) -> bool {
    id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_session_row(id: &str) -> bool {
    id.strip_prefix("session-").is_some_and(is_hex12)
}

/// What the sweep does with an element of id `id`, `None` when it is not one it knows: most of
/// the window's elements are text, icons and wrappers, which are neither.
fn classify(id: &str, selected: Option<bool>) -> Option<Kind> {
    if NAVIGATES.contains(&id) {
        return Some(Kind::Navigates);
    }
    // Wrappers of rows and the filters of a list: what is clicked in them is an inner element.
    if LEFT.contains(&id) || LEFT_PREFIXES.iter().any(|prefix| id.starts_with(prefix)) {
        return Some(Kind::Left);
    }
    if PROJECTS.iter().any(|name| id == format!("project-{name}")) || is_session_row(id) {
        return Some(Kind::Navigates);
    }
    if id.strip_prefix("place-session-").is_some_and(is_hex12)
        || id.starts_with("progress-milestone-")
        || id.starts_with("place-row-")
    {
        return Some(Kind::Navigates);
    }
    // The tabs of a page: `<page>-tabs-<tab>`, which say whether they are the chosen one.
    if id.contains("-tabs-") && selected.is_some() {
        return Some(Kind::Navigates);
    }
    None
}

/// An element that looks like it navigates by the way the window names those, and that the
/// sweep has no class for: the test says so, so that its author chooses.
fn looks_navigable(id: &str) -> bool {
    !id.contains("skeleton")
        && id
            .split('-')
            .any(|word| ["row", "crumb", "link", "open", "back"].contains(&word))
}

/// What is on screen: each element's id (when it has a name), its class, whether it is visible.
fn on_screen(cx: &mut VisualTestContext) -> Vec<(String, Option<Kind>, bool)> {
    let mut found: Vec<(String, Option<Kind>, bool)> = cx.update(|window, _| {
        window
            .elements()
            .into_iter()
            .filter_map(|e| match e.path().last() {
                Some(gpui_kit::ElementId::Name(name)) => {
                    let name = name.to_string();
                    let kind = classify(&name, e.selected());
                    Some((name, kind, e.visible()))
                }
                _ => None,
            })
            .collect()
    });
    // A row of a list (a ticket, a spec) holds its own `<id>-item`: the list names its rows by
    // the item they show, so the rows are the ids that have one.
    let names: Vec<String> = found.iter().map(|(id, ..)| id.clone()).collect();
    for (id, kind, _) in &mut found {
        if kind.is_none() && names.contains(&format!("{id}-item")) {
            *kind = Some(Kind::Navigates);
        }
    }
    found
}

/// The ids of the elements the sweep clicks on screen, each once.
fn navigable(cx: &mut VisualTestContext) -> Vec<String> {
    let mut ids = Vec::new();
    for (id, kind, visible) in on_screen(cx) {
        if kind == Some(Kind::Navigates) && visible && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn click(cx: &mut VisualTestContext, id: &str) {
    cx.update(|window, cx| window.click(id.to_owned(), cx));
    quiet(cx);
}

/// A page the sweep starts from: a selection, and the tab chosen on it by a click.
#[derive(Clone, Debug)]
struct Start {
    selection: Selection,
    tab: Option<String>,
}

/// The window on `start`: its page opened, its tab chosen, its content read.
fn go_to(shell: &Entity<Shell>, cx: &mut VisualTestContext, start: &Start) {
    shell.update_in(cx, |s, window, cx| {
        s.open(start.selection.clone(), window, cx)
    });
    quiet(cx);
    if let Some(tab) = &start.tab {
        if !present(cx, tab) {
            until(shell, cx, &format!("{tab} shows on {start:?}"), |cx| {
                present(cx, tab)
            });
        }
        click(cx, tab);
        if tab.ends_with("-progress") {
            until(shell, cx, "the milestones are read", |cx| {
                on_screen(cx)
                    .iter()
                    .any(|(id, _, _)| id.starts_with("progress-milestone-"))
            });
        }
        quiet(cx);
    }
}

/// One frame: what a click did is drawn, its animations are not waited for.
fn quiet(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// The window still runs and shows a page.
fn still_runs(shell: &Entity<Shell>, cx: &mut VisualTestContext, after: &str) {
    let page = shell.read_with(cx, |s, cx| s.main().read(cx).page().clone());
    assert_ne!(page, Page::Empty, "{after} left the main area empty");
    assert!(
        present(cx, "sidebar"),
        "{after}: the window shows no sidebar"
    );
}

#[gpui_kit::test]
fn every_navigable_element_of_every_page_can_be_clicked(cx: &mut TestAppContext) {
    let fixture = fixture();
    let (shell, cx) = window(&fixture.user, cx);
    until(&shell, cx, "the sidebar lists the sessions", |cx| {
        on_screen(cx)
            .iter()
            .filter(|(id, ..)| is_session_row(id))
            .count()
            >= 3
    });
    settle(cx);

    let sessions: Vec<String> = on_screen(cx)
        .into_iter()
        .map(|(id, ..)| id)
        .filter(|id| is_session_row(id))
        .map(|id| id["session-".len()..].to_owned())
        .collect();
    let mut selections = vec![
        Selection::Project("app".into()),
        Selection::Project("web".into()),
        Selection::Settings,
        Selection::ProjectSettings("app".into()),
        Selection::Spec {
            project: "app".into(),
            spec: fixture.spec,
        },
        Selection::Services,
    ];
    selections.extend(sessions.into_iter().map(Selection::Session));

    // Each page, and each of its tabs as a page of its own.
    let mut starts = Vec::new();
    for selection in selections {
        let page = Start {
            selection,
            tab: None,
        };
        go_to(&shell, cx, &page);
        let tabs: Vec<String> = navigable(cx)
            .into_iter()
            .filter(|id| id.contains("-tabs-"))
            .collect();
        // A page remembers its tab: with tabs, each is a start, so that one is always chosen.
        if tabs.is_empty() {
            starts.push(page.clone());
        }
        starts.extend(tabs.into_iter().map(|tab| Start {
            tab: Some(tab),
            ..page.clone()
        }));
    }

    let mut clicked = 0;
    let mut unclassified = Vec::new();
    for start in &starts {
        go_to(&shell, cx, start);
        let ids = navigable(cx);
        for (id, kind, _) in on_screen(cx) {
            if kind.is_none() && looks_navigable(&id) && !unclassified.contains(&id) {
                unclassified.push(id);
            }
        }
        let mut walked = false;
        for id in ids {
            go_to(&shell, cx, start);
            // A page that is drawn in as it opens gives its elements a frame or two later.
            if !present(cx, &id) {
                until(&shell, cx, &format!("{id} is back on {start:?}"), |cx| {
                    present(cx, &id)
                });
            }
            click(cx, &id);
            still_runs(&shell, cx, &format!("{id} on {start:?}"));
            // The history only has somewhere to go when the click opened a page.
            if shell.read_with(cx, |s, _| s.selected() != Some(&start.selection)) {
                for way in [Way::Back, Way::Forward] {
                    shell.update_in(cx, |s, window, cx| s.go(way, window, cx));
                    cx.run_until_parked();
                    // Drawn once per page the sweep starts from, not after each of its clicks.
                    if !walked {
                        quiet(cx);
                    }
                    still_runs(&shell, cx, &format!("{way:?} after {id} on {start:?}"));
                }
                walked = true;
            }
            clicked += 1;
        }
    }
    assert!(
        clicked >= 100,
        "the sweep clicked {clicked} elements on {} pages: it found almost nothing to click",
        starts.len()
    );
    assert!(
        unclassified.is_empty(),
        "elements that look like they navigate, and the sweep has no class for: {unclassified:?}. \
         Say in `classify` whether a click on each navigates or is left."
    );
}

#[test]
fn an_element_that_looks_navigable_and_has_no_class_is_named_by_the_sweep() {
    // A new kind of row, link or crumb with an id the sweep has no class for: it is said, not
    // skipped; one with a class is clicked or left on purpose.
    assert!(looks_navigable("milestone-row-3"));
    assert_eq!(classify("milestone-row-3", None), None);
    assert_eq!(classify("settings-crumb", None), Some(Kind::Navigates));
    assert_eq!(classify("progress-open", None), Some(Kind::Left));
    assert!(!looks_navigable("rail-runs-skeleton-row"));
    assert!(!looks_navigable("provider-switch-opencode"));
}
