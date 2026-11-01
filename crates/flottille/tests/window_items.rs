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
use flottille_window::items::{ItemRow, Kind, ProjectItems, Read};
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
        // The reads an event asks wait for a short quiet.
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

/// What the project page scrolls under: the first of them in sight.
const ANCHORS: [&str; 5] = [
    "project-header-title",
    "project-items-title",
    "project-items-tabs",
    "project-items-list",
    "project-tasks-title",
];

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .is_some_and(|found| found.visible())
    })
}

/// Whether `id` is whole in the window: `visible` holds from its first pixel in sight, while a
/// click lands on its centre, which may still be below the fold.
fn in_sight(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| {
        let height = window.viewport_size().height;
        window.try_find(id.to_owned()).is_some_and(|found| {
            let bounds = found.bounds();
            found.visible() && bounds.top() >= px(0.) && bounds.bottom() <= height
        })
    })
}

/// Scrolls the project page down until `id` is whole in sight, as a user would: where the fold
/// of the test window falls depends on the fonts of the machine, so no test counts on it.
fn reveal(cx: &mut VisualTestContext, id: &str) {
    for _ in 0..80 {
        if !present(cx, id) || in_sight(cx, id) {
            return;
        }
        let Some(anchor) = ANCHORS.into_iter().find(|anchor| visible(cx, anchor)) else {
            return;
        };
        cx.update(|window, cx| {
            window.scroll(
                anchor,
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-120.))),
                cx,
            )
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

/// Clicks `id`, scrolled into sight first.
fn click(cx: &mut VisualTestContext, id: &str) {
    reveal(cx, id);
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
    for fact in ["#3", "someone", "dana", "Updated"] {
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
    for fact in ["#5", "other", "someone"] {
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
    assert_eq!(label(cx, "item-detail-check-build").unwrap(), "build");
    assert_eq!(label(cx, "item-detail-check-test").unwrap(), "test");
    // A check says its state with the mark the rows use.
    let marks = items.read_with(cx, |items, cx| {
        let Some(Ok(full)) = &items.detail().unwrap().full else {
            panic!("read")
        };
        full.checks
            .iter()
            .map(|check| check.status.read(cx).state())
            .collect::<Vec<_>>()
    });
    assert_eq!(marks, [AgentState::Completed, AgentState::Failed]);
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

/// The detail of the row `key`, once `items.show` answered and what it brought has unfolded.
fn open_detail(
    shell: &Entity<Shell>,
    items: &Entity<ProjectItems>,
    cx: &mut VisualTestContext,
    key: &str,
) {
    let row = rows(items, cx)
        .into_iter()
        .find(|r| r.key == key)
        .unwrap()
        .id();
    click(cx, &row);
    until(shell, cx, "the detail is read", |cx| {
        items.read_with(cx, |items, _| {
            items.detail().is_some_and(|detail| detail.read())
        })
    });
    settle(cx);
}

#[gpui_kit::test]
fn a_burst_of_item_events_asks_one_read_once_it_goes_quiet(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    declare(&user, &["api"], false);
    let (_shell, items, cx) = open_page(&user, cx);
    let before = items.read_with(cx, |items, _| items.lists_asked());

    items.update(cx, |items, cx| {
        for _ in 0..40 {
            items.changed(None, cx);
        }
    });
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        items.read_with(cx, |items, _| items.lists_asked()),
        before,
        "nothing while the events still come"
    );
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(
        items.read_with(cx, |items, _| items.lists_asked()),
        before + 1
    );
}

#[gpui_kit::test]
fn an_answer_older_than_the_one_applied_changes_nothing(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    declare(&user, &["api"], false);
    let (_shell, items, cx) = open_page(&user, cx);
    let read = |title: &str| Read {
        items: vec![ItemRow::of(&json!({"kind": "pull", "repository": "api",
            "key": "1", "title": title, "state": "open"}))],
        forges: vec!["api".into()],
        repositories: vec!["api".into()],
        ..Read::default()
    };

    items.update(cx, |items, cx| {
        items.listed(1_000_001, Ok(read("Newer")), cx);
        items.listed(1_000_000, Ok(read("Older")), cx);
    });

    let titles: Vec<String> = rows(&items, cx).into_iter().map(|r| r.title).collect();
    assert_eq!(titles, ["Newer"]);
}

#[gpui_kit::test]
fn a_description_never_read_says_so_and_an_empty_one_says_there_is_none(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    // A first round reads in full the open pull requests alone.
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": pull("api", 1, "Add the login", "OPEN", OLD),
        "feature-2": pull("api", 2, "Old work", "MERGED", OLD)
    }}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (shell, items, cx) = open_page(&user, cx);
    click(cx, "project-items-state-all");
    settle(cx);

    open_detail(&shell, &items, cx, "2");
    assert_eq!(
        label(cx, "item-detail-description").unwrap(),
        "Not read in full yet."
    );
    cx.simulate_keystrokes("escape");
    settle(cx);
    open_detail(&shell, &items, cx, "1");
    assert_eq!(
        label(cx, "item-detail-description").unwrap(),
        "No description."
    );
}

#[gpui_kit::test]
fn a_rows_state_is_said_only_where_it_tells_the_rows_apart(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    let mut draft = pull("api", 2, "Draft work", "OPEN", OLD);
    draft["isDraft"] = json!(true);
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": pull("api", 1, "Open work", "OPEN", OLD),
        "feature-2": draft,
        "feature-3": pull("api", 3, "Merged work", "MERGED", OLD),
        "feature-4": pull("api", 4, "Closed work", "CLOSED", OLD)
    }}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (_shell, items, cx) = open_page(&user, cx);
    let facts = |cx: &mut VisualTestContext, key: &str| {
        let row = rows(&items, cx)
            .into_iter()
            .find(|r| r.key == key)
            .unwrap()
            .id();
        label(cx, &format!("{row}-facts")).unwrap()
    };

    // Open: a draft says so, an open one says nothing of its state.
    assert!(!facts(cx, "1").contains("State"), "{}", facts(cx, "1"));
    assert!(facts(cx, "2").contains("State draft"), "{}", facts(cx, "2"));
    // Closed: merged and closed tell them apart.
    click(cx, "project-items-state-closed");
    settle(cx);
    assert!(
        facts(cx, "3").contains("State merged"),
        "{}",
        facts(cx, "3")
    );
    assert!(
        facts(cx, "4").contains("State closed"),
        "{}",
        facts(cx, "4")
    );
    // All: every row says it.
    click(cx, "project-items-state-all");
    settle(cx);
    assert!(facts(cx, "1").contains("State open"), "{}", facts(cx, "1"));
    assert!(
        facts(cx, "3").contains("State merged"),
        "{}",
        facts(cx, "3")
    );
}

#[gpui_kit::test]
fn the_branches_of_the_detail_unfold_when_items_show_answers(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": pull("api", 1, "Add the login", "OPEN", OLD)
    }}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (shell, items, cx) = open_page(&user, cx);
    let row = rows(&items, cx)[0].id();

    click(cx, &row);
    until(&shell, cx, "the detail is read", |cx| {
        items.read_with(cx, |items, _| {
            items.detail().is_some_and(|detail| detail.read())
        })
    });
    a_frame(cx);
    let unfolding = height(cx, "item-detail-branches-block");
    settle(cx);
    let unfolded = height(cx, "item-detail-branches-block");
    assert!(
        unfolding < unfolded,
        "the branches unfold: {unfolding} then {unfolded}"
    );
}

#[gpui_kit::test]
fn a_ci_mark_whose_checks_go_fades_into_the_plain_icon(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api"]);
    let mut green = pull("api", 1, "Add the login", "OPEN", OLD);
    green["statusCheckRollup"] = json!([check("build", "COMPLETED", "SUCCESS")]);
    user.gh_script(json!({"pulls": {"acme/api": {"feature-1": green}}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (shell, items, cx) = open_page(&user, cx);
    let row = rows(&items, cx)[0].id();
    let mark = format!("{row}-ci");
    assert!(present(cx, &mark));

    // Its title changed too: the round says `item.changed`.
    user.gh_script(json!({"pulls": {"acme/api": {
        "feature-1": pull("api", 1, "Add the login form", "OPEN", NEW)
    }}}));
    refresh(&user);
    until(&shell, cx, "the checks are gone", |cx| {
        items.read_with(cx, |items, cx| items.ci_state(&row, cx).is_none())
    });
    a_frame(cx);
    assert!(present(cx, &mark), "the mark fades out, not at once");
    settle(cx);
    assert!(!present(cx, &mark));
}

#[gpui_kit::test]
fn an_open_detail_is_read_again_when_its_item_changes_and_says_when_it_leaves(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, &["api"]);
    let mut open = pull("api", 1, "Add the login", "OPEN", OLD);
    open["body"] = json!("First words.");
    user.gh_script(json!({"pulls": {"acme/api": {"feature-1": open}}}));
    declare(&user, &["api"], false);
    refresh(&user);
    let (shell, items, cx) = open_page(&user, cx);
    open_detail(&shell, &items, cx, "1");
    assert_eq!(
        label(cx, "item-detail-description").unwrap(),
        "First words."
    );

    // Its title changed too: the round says `item.changed`.
    let mut changed = pull("api", 1, "Add the login form", "OPEN", NEW);
    changed["body"] = json!("Second words.");
    user.gh_script(json!({"pulls": {"acme/api": {"feature-1": changed}}}));
    refresh(&user);
    until(&shell, cx, "the detail is read again", |cx| {
        label(cx, "item-detail-description").as_deref() == Some("Second words.")
    });
    assert!(!present(cx, "item-detail-gone"));

    // A read that no longer lists it (its repository's forge unset, say): the open detail says
    // so. The daemon forgets such items without an event, so the read is given here.
    items.update(cx, |items, cx| {
        let read = Read {
            forges: vec!["api".into()],
            repositories: vec!["api".into()],
            ..Read::default()
        };
        items.listed(1_000_000, Ok(read), cx);
    });
    a_frame(cx);
    assert!(present(cx, "item-detail-gone"));
    let unfolding = height(cx, "item-detail-gone-block");
    settle(cx);
    assert!(
        unfolding < height(cx, "item-detail-gone-block"),
        "it unfolds"
    );
    assert!(label(cx, "item-detail-gone").unwrap().contains("no longer"));
}

#[gpui_kit::test]
fn the_answer_for_another_repositorys_item_of_the_same_number_is_not_shown(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, &["api", "web"]);
    declare(&user, &["api", "web"], false);
    let (_shell, items, cx) = open_page(&user, cx);
    let web = ItemRow::of(&json!({"kind": "pull", "repository": "web", "key": "12",
        "title": "Restyle", "state": "open"}));

    let read = items.update(cx, |items, cx| {
        items.open_detail(web, cx);
        let api = json!({"kind": "pull", "repository": "api", "key": "12", "title": "Other",
            "details": {"description": "Not this one."}});
        items.shown("pull", "12", Some("api".into()), u64::MAX, Ok(api), cx);
        items.detail().unwrap().read()
    });

    assert!(!read, "api's #12 is not web's");
}

#[gpui_kit::test]
fn the_repository_filter_has_a_visible_label(cx: &mut TestAppContext) {
    let user = user();
    project(&user, &["api", "web"]);
    declare(&user, &["api", "web"], false);
    let (_shell, _items, cx) = open_page(&user, cx);

    assert_eq!(
        label(cx, "project-items-repository-label").unwrap(),
        "Repository"
    );
}

#[gpui_kit::test]
fn a_tab_shown_again_does_not_fold_away_rows_that_left_while_it_was_hidden(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, &["api"]);
    let mut theirs = issue(6, "Typo on the page", "OPEN", OLD);
    theirs["assignees"] = json!([]);
    user.gh_script(json!({
        "user": {"login": "someone"},
        "issues": {"acme/api": {"5": issue(5, "Users cannot sign in", "OPEN", OLD), "6": theirs}}
    }));
    declare(&user, &["api"], true);
    refresh(&user);
    let (_shell, items, cx) = open_page(&user, cx);
    click(cx, "project-items-tabs-tickets");
    settle(cx);
    let gone = rows(&items, cx)
        .into_iter()
        .find(|r| r.key == "6")
        .unwrap()
        .id();
    assert!(present(cx, &gone));
    click(cx, "project-items-tabs-pulls");
    settle(cx);

    click(cx, "project-items-mine");
    settle(cx);
    click(cx, "project-items-tabs-tickets");
    a_frame(cx);

    assert!(!present(cx, &gone), "it left while the tab was hidden");
}
