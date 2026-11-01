//! The scenarios of a project's pull requests and tickets in the window (#258), on GPUI's test
//! platform in front of a real daemon that keeps the items (`docs/items.md`) through the fake
//! `gh`: the page's two tabs list what `items.list` holds, its filters, the rows that change,
//! come and go with the `item.*` events, and the detail that `items.show` fills. No window opens
//! on a screen, no forge is read.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::items::{ItemRow, Kind, ProjectItems};
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A time long past: what the incremental reads never ask again.
const OLD: &str = "2020-01-01T00:00:00Z";
/// A time ahead of any cursor: what changed since the last round.
const NEW: &str = "2999-01-01T00:00:00Z";

fn user() -> User {
    User::new()
        .with_fake_gh()
        .with_fetcher()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// The project `app` and its repositories, each a folder of it whose `origin` is
/// `acme/<name>` on GitHub; nothing declared.
fn project(user: &User, repositories: &[&str]) {
    for name in repositories {
        let dir = user.path(&format!("work/app/{name}"));
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), "app\n").unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
        user.git(
            &dir,
            &[
                "remote",
                "add",
                "origin",
                &format!("git@github.com:acme/{name}.git"),
            ],
        );
    }
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
}

/// Declares each repository's forge on GitHub, and the tracker on `api`'s issues when `tracker`.
fn declare(user: &User, repositories: &[&str], tracker: bool) {
    let declared: Vec<Value> = repositories
        .iter()
        .map(|name| {
            json!({"path": name, "forge": {"kind": "github", "repository": format!("acme/{name}")}})
        })
        .collect();
    let tracker = match tracker {
        true => json!({"kind": "github", "repository": "api"}),
        false => Value::Null,
    };
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": declared, "tracker": tracker}),
    )
    .unwrap();
}

fn check(name: &str, status: &str, conclusion: &str) -> Value {
    json!({"__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion,
        "detailsUrl": format!("https://github.com/acme/api/actions/runs/900/job/{name}")})
}

/// What the fake `gh` answers of a pull request of `acme/<repo>`.
fn pull(repo: &str, number: u64, title: &str, state: &str, updated: &str) -> Value {
    json!({
        "number": number, "title": title, "state": state, "isDraft": false,
        "headRefName": format!("feature-{number}"), "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": format!("https://github.com/acme/{repo}/pull/{number}"),
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "updatedAt": updated, "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
        "statusCheckRollup": [], "comments": [], "reviews": [], "body": ""
    })
}

/// What the fake `gh` answers of an issue of `acme/api`.
fn issue(number: u64, title: &str, state: &str, updated: &str) -> Value {
    json!({
        "number": number, "title": title, "state": state, "body": "",
        "url": format!("https://github.com/acme/api/issues/{number}"),
        "author": {"login": "other"}, "assignees": [{"login": "someone"}], "labels": [],
        "updatedAt": updated, "comments": []
    })
}

fn refresh(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
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
    cx.simulate_resize(size(px(1280.), px(900.)));
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

/// One frame, the clock a frame ahead: what moves has only begun to.
fn a_frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// The window on `app`'s page, once its pull requests are listed.
fn open_page<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<ProjectItems>,
    &'a mut VisualTestContext,
) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the project page shows", |cx| {
        shell.update_in(cx, |s, window, cx| {
            s.open(Selection::Project("app".into()), window, cx)
        });
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    let items = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_page()
            .unwrap()
            .read(cx)
            .items()
            .clone()
    });
    until(&shell, cx, "the items are read", |cx| {
        items.read_with(cx, |items, _| items.is_read())
    });
    settle(cx);
    (shell, items, cx)
}

fn rows(items: &Entity<ProjectItems>, cx: &mut VisualTestContext) -> Vec<ItemRow> {
    items.read_with(cx, |items, _| items.rows())
}

fn keys(items: &Entity<ProjectItems>, cx: &mut VisualTestContext) -> Vec<String> {
    rows(items, cx).into_iter().map(|row| row.key).collect()
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
}

fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().size.height))
}

#[gpui_kit::test]
fn the_pull_requests_tab_lists_the_projects_pulls_from_items_list_with_their_ci_state(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, &["api"]);
    let mut green = pull("api", 1, "Add the login", "OPEN", OLD);
    green["statusCheckRollup"] = json!([check("build", "COMPLETED", "SUCCESS")]);
    let mut red = pull("api", 2, "Fix the session", "OPEN", OLD);
    red["statusCheckRollup"] = json!([check("test", "COMPLETED", "FAILURE")]);
    let mut running = pull("api", 3, "Speed up the build", "OPEN", OLD);
    running["statusCheckRollup"] = json!([check("build", "IN_PROGRESS", "")]);
    running["assignees"] = json!([{"login": "dana"}]);
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": green, "feature-2": red, "feature-3": running
    }}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (_shell, items, cx) = open_page(&user, cx);

    assert_eq!(items.read_with(cx, |items, _| items.tab()), Kind::Pulls);
    let listed = rows(&items, cx);
    let mut listed_keys: Vec<&str> = listed.iter().map(|row| row.key.as_str()).collect();
    listed_keys.sort();
    assert_eq!(listed_keys, ["1", "2", "3"], "{listed:?}");
    let ci = |key: &str| {
        items.read_with(cx, |items, cx| {
            items
                .ci_state(&listed.iter().find(|r| r.key == key).unwrap().id(), cx)
                .unwrap()
        })
    };
    assert_eq!(ci("1"), AgentState::Completed);
    assert_eq!(ci("2"), AgentState::Failed);
    assert_eq!(ci("3"), AgentState::Running);
    // The row says its number, title, state, author, assignee and when it changed.
    let row = listed.iter().find(|r| r.key == "3").unwrap().id();
    assert_eq!(label(cx, &row).unwrap(), "Speed up the build");
    let facts = label(cx, &format!("{row}-facts")).unwrap();
    for fact in ["#3", "open", "someone", "dana", "Updated"] {
        assert!(facts.contains(fact), "{fact}: {facts}");
    }
    // The tabs count what they list.
    assert_eq!(label(cx, "project-items-tabs-pulls-count").unwrap(), "3");
}

#[gpui_kit::test]
fn the_tickets_tab_lists_the_tickets_with_their_status(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    user.gh_script(json!({
        "pulls": {"acme/api": {"feature-1": pull("api", 1, "Add the login", "OPEN", OLD)}},
        "issues": {"acme/api": {
            "5": issue(5, "Users cannot sign in", "OPEN", OLD),
            "6": issue(6, "Typo on the page", "OPEN", OLD)
        }}
    }));
    declare(&user, &["api"], true);
    refresh(&user);
    let (_shell, items, cx) = open_page(&user, cx);
    assert_eq!(label(cx, "project-items-tabs-tickets-count").unwrap(), "2");

    click(cx, "project-items-tabs-tickets");
    settle(cx);

    assert_eq!(items.read_with(cx, |items, _| items.tab()), Kind::Tickets);
    let mut listed = keys(&items, cx);
    listed.sort();
    assert_eq!(listed, ["5", "6"]);
    let row = rows(&items, cx)
        .into_iter()
        .find(|r| r.key == "5")
        .unwrap()
        .id();
    assert_eq!(label(cx, &row).unwrap(), "Users cannot sign in");
    let facts = label(cx, &format!("{row}-facts")).unwrap();
    for fact in ["#5", "open", "other", "someone"] {
        assert!(facts.contains(fact), "{fact}: {facts}");
    }
    // Nothing moves once the tab is shown: nothing is drawn again at rest.
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn a_filter_closed_mine_or_repository_changes_the_rows_animated(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api", "web"]);
    let mut theirs = pull("api", 2, "Their merged work", "MERGED", OLD);
    theirs["author"] = json!({"login": "other"});
    let mut web = pull("web", 7, "Restyle the page", "OPEN", OLD);
    web["author"] = json!({"login": "other"});
    user.gh_script(json!({
        "user": {"login": "someone"},
        "pulls": {
            "acme/api": {"feature-1": pull("api", 1, "Mine, open", "OPEN", OLD), "feature-2": theirs},
            "acme/web": {"feature-7": web}
        }
    }));
    declare(&user, &["api", "web"], false);
    refresh(&user);
    let (_shell, items, cx) = open_page(&user, cx);
    let mut open = keys(&items, cx);
    open.sort();
    assert_eq!(open, ["1", "7"], "open by default");
    let first = rows(&items, cx)
        .into_iter()
        .find(|r| r.key == "1")
        .unwrap()
        .id();

    // Closed: the open rows fold away, still drawn on the first frame.
    click(cx, "project-items-state-closed");
    a_frame(cx);
    assert_eq!(keys(&items, cx), ["2"]);
    assert!(present(cx, &first), "an open row folds away, not at once");
    settle(cx);
    assert!(!present(cx, &first));

    // All, then mine: only the user's own.
    click(cx, "project-items-state-all");
    settle(cx);
    assert_eq!(keys(&items, cx).len(), 3);
    click(cx, "project-items-mine");
    settle(cx);
    assert_eq!(keys(&items, cx), ["1"]);
    click(cx, "project-items-mine");
    settle(cx);

    // A repository: only its pull requests.
    click(cx, "project-items-repository");
    settle(cx);
    click(cx, "project-items-repository-web");
    settle(cx);
    assert_eq!(keys(&items, cx), ["7"]);
    assert_eq!(label(cx, "project-items-tabs-pulls-count").unwrap(), "1");
}

#[gpui_kit::test]
fn an_item_changed_event_updates_a_row_in_place_and_item_opened_inserts_one_unfolding(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, &["api"]);
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": pull("api", 1, "Add the login", "OPEN", OLD)
    }}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (shell, items, cx) = open_page(&user, cx);
    let row = rows(&items, cx)[0].id();

    // The forge says another title, and a new pull request: the next round emits
    // `item.changed` and `item.opened`.
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": pull("api", 1, "Add the login page", "OPEN", NEW),
        "feature-4": pull("api", 4, "Remember the user", "OPEN", NEW)
    }}}));
    refresh(&user);
    until(&shell, cx, "the new pull request is listed", |cx| {
        keys(&items, cx).contains(&"4".to_owned())
    });
    a_frame(cx);
    let new = rows(&items, cx)
        .into_iter()
        .find(|r| r.key == "4")
        .unwrap()
        .id();
    let item = format!("{new}-item");
    let unfolding = height(cx, &item);
    settle(cx);
    let unfolded = height(cx, &item);
    assert!(
        unfolding < unfolded,
        "a row that arrives unfolds: {unfolding} then {unfolded}"
    );
    // The same row, its title changed in place.
    assert_eq!(label(cx, &row).unwrap(), "Add the login page");
    assert_eq!(rows(&items, cx).len(), 2);
}

#[gpui_kit::test]
fn a_click_opens_the_detail_modal_with_the_description_the_checks_and_the_comments_and_escape_closes_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, &["api"]);
    let mut open = pull("api", 1, "Add the login", "OPEN", OLD);
    open["body"] = json!("Adds the **login** form.");
    open["statusCheckRollup"] = json!([
        check("build", "COMPLETED", "SUCCESS"),
        check("test", "COMPLETED", "FAILURE")
    ]);
    open["comments"] = json!([
        {"author": {"login": "dana"}, "body": "Looks good"},
        {"author": {"login": "someone"}, "body": "Rebased\n\n<!-- flottille -->"}
    ]);
    user.gh_script(json!({"pulls": {"acme/api": {"feature-1": open}}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (shell, items, cx) = open_page(&user, cx);
    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    let row = rows(&items, cx)[0].id();

    click(cx, &row);
    until(&shell, cx, "the detail is read", |cx| {
        items.read_with(cx, |items, _| {
            items.detail().is_some_and(|detail| detail.read())
        })
    });
    settle(cx);

    assert!(items.read_with(cx, |items, _| items.is_detail_open()));
    assert_eq!(label(cx, "item-detail-title").unwrap(), "#1 Add the login");
    assert!(
        label(cx, "item-detail-description")
            .unwrap()
            .contains("Adds the **login** form."),
        "the description"
    );
    assert_eq!(
        label(cx, "item-detail-check-build").unwrap(),
        "build: passing"
    );
    assert_eq!(
        label(cx, "item-detail-check-test").unwrap(),
        "test: failing"
    );
    assert!(
        label(cx, "item-detail-said-0").unwrap().contains("dana"),
        "a comment says who wrote it"
    );
    let agent = label(cx, "item-detail-said-1").unwrap();
    assert!(agent.contains("posted by an agent"), "{agent}");
    assert_eq!(
        label(cx, "item-detail-branches").unwrap(),
        "Branches feature-1 into main"
    );

    click(cx, "item-detail-browser");
    assert_eq!(
        opened.lock().unwrap().as_slice(),
        [PathBuf::from("https://github.com/acme/api/pull/1")]
    );

    cx.simulate_keystrokes("escape");
    a_frame(cx);
    assert!(!items.read_with(cx, |items, _| items.is_detail_open()));
    assert!(present(cx, "item-detail"), "it fades out, not at once");
    settle(cx);
    assert!(!present(cx, "item-detail"));
}

#[gpui_kit::test]
fn a_project_with_no_forge_set_shows_the_not_set_state_with_its_settings_link(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, &["api"]);
    let (shell, _items, cx) = open_page(&user, cx);

    let said = label(cx, "project-items-not-set").unwrap();
    assert!(said.contains("No forge is set"), "{said}");
    click(cx, "project-items-tabs-tickets");
    settle(cx);
    let said = label(cx, "project-items-not-set").unwrap();
    assert!(said.contains("No tracker is set"), "{said}");

    click(cx, "project-items-settings");
    settle(cx);
    let tab = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_settings_page()
            .map(|page| page.read(cx).tab())
    });
    assert_eq!(tab, Some(1), "the settings open on the tracker's section");
}

#[gpui_kit::test]
fn a_source_that_fails_says_why(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    user.gh_script(json!({"signed_in": false}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (_shell, _items, cx) = open_page(&user, cx);

    let said = label(cx, "project-items-problem").unwrap();
    assert!(said.contains("gh is not signed in"), "{said}");
}

#[gpui_kit::test]
fn an_open_window_has_the_items_read_every_minute(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": pull("api", 1, "Add the login", "OPEN", OLD)
    }}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (shell, _items, cx) = open_page(&user, cx);
    let lists = |user: &User| {
        user.gh_received()
            .iter()
            .filter(|call| call["args"][0] == "pr" && call["args"][1] == "list")
            .count()
    };
    let before = lists(&user);

    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    until(&shell, cx, "a round a minute on", |_| lists(&user) > before);
}
