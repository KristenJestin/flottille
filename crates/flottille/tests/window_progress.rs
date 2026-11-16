//! The scenarios of a project's Progress page (#801), on GPUI's test platform in front of a real
//! daemon that keeps the items (`docs/items.md`) through the fake `gh` and the fake `acli`: the
//! milestones of the tracker with their bars, a row that unfolds into its tickets grouped, the
//! chips of what Flottille knows of a ticket, a ticket that changes group, and the closed
//! milestones behind their fold. No window opens on a screen, no forge nor tracker is read.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::progress::ProgressPage;
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const OLD: &str = "2020-01-01T00:00:00Z";
const NEW: &str = "2999-01-01T00:00:00Z";
const SITE: &str = "example.atlassian.net";

fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_fake_acli()
        .with_fetcher()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// The project `app` of one repository whose `origin` is `acme/app` on GitHub, its forge declared
/// and its tracker `tracker`.
fn project(user: &User, tracker: Value) {
    let dir = user.path("work/app");
    user.git_init(&dir);
    std::fs::write(dir.join("README.md"), "app\n").unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &dir,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": dir, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": tracker}),
    )
    .unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
}

fn on_github(user: &User) {
    project(user, json!({"kind": "github", "repository": "."}));
}

fn on_jira(user: &User) {
    project(
        user,
        json!({"kind": "jira", "site": SITE, "projects": ["ABC"]}),
    );
}

/// What the fake `gh` answers of an issue of `acme/app`, in the milestone `milestone`.
fn issue(number: u64, title: &str, state: &str, milestone: Option<(u64, &str)>) -> Value {
    json!({
        "number": number, "title": title, "state": state, "body": "",
        "url": format!("https://github.com/acme/app/issues/{number}"),
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "milestone": milestone.map(|(n, t)| json!({"number": n, "title": t})),
        "updatedAt": OLD, "comments": []
    })
}

/// What the fake `gh` answers of a pull request of `acme/app` on `branch`.
fn pull(number: u64, branch: &str, state: &str, body: &str, checks: Value) -> Value {
    json!({
        "number": number, "title": format!("Pull {number}"), "state": state, "isDraft": false,
        "headRefName": branch, "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": format!("https://github.com/acme/app/pull/{number}"),
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "updatedAt": OLD, "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
        "statusCheckRollup": checks, "comments": [], "reviews": [], "body": body
    })
}

fn running_check() -> Value {
    json!([{"__typename": "CheckRun", "name": "test", "status": "IN_PROGRESS",
        "conclusion": "", "detailsUrl": "https://github.com/acme/app/actions/runs/1/job/2"}])
}

fn milestones() -> Value {
    json!([
        {"number": 1, "title": "v0.3", "state": "open", "due_on": "2026-12-05T00:00:00Z",
            "open_issues": 3, "closed_issues": 1,
            "html_url": "https://github.com/acme/app/milestone/1"},
        {"number": 2, "title": "Feedback", "state": "open", "due_on": null,
            "open_issues": 1, "closed_issues": 0,
            "html_url": "https://github.com/acme/app/milestone/2"},
        {"number": 3, "title": "v0.2", "state": "closed", "due_on": "2026-10-01T00:00:00Z",
            "open_issues": 0, "closed_issues": 1,
            "html_url": "https://github.com/acme/app/milestone/3"}
    ])
}

/// v0.3 holds one ticket of each group: #1 to do, #2 in progress (a workspace on it), #3 in
/// review (a pull request that closes it, its checks running), #4 done. Feedback holds #5, and
/// the closed v0.2 holds #6, done.
fn github_script(pulls: Value) -> Value {
    json!({
        "milestones": {"acme/app": milestones()},
        "issues": {"acme/app": {
            "1": issue(1, "Connections in the window", "OPEN", Some((1, "v0.3"))),
            "2": issue(2, "Sign a connection in", "OPEN", Some((1, "v0.3"))),
            "3": issue(3, "Bind a vault entry", "OPEN", Some((1, "v0.3"))),
            "4": issue(4, "Connections, the daemon part", "CLOSED", Some((1, "v0.3"))),
            "5": issue(5, "A CI run starts on its branch", "OPEN", Some((2, "Feedback"))),
            "6": issue(6, "An older one", "CLOSED", Some((3, "v0.2"))),
            "7": issue(7, "In no milestone", "OPEN", None),
        }},
        "pulls": {"acme/app": pulls},
    })
}

fn review_pull() -> Value {
    json!({"bind-vault": pull(9, "bind-vault", "OPEN", "Done here.\n\nCloses #3", running_check())})
}

/// A workspace linked to the ticket `ticket`, made offline from the project's main.
fn workspace_on(user: &User, name: &str, ticket: &str) {
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": name, "offline": true, "ticket": ticket}),
        )
        .unwrap();
    assert_eq!(made["state"], "ready", "{made}");
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
    cx.update(|cx| flottille_window::init_with(env, cx));
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

fn a_frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
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

fn width(cx: &mut VisualTestContext, id: &str) -> f32 {
    cx.update(|window, _| f32::from(window.find(id.to_owned()).bounds().size.width))
}

fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    cx.update(|window, _| f32::from(window.find(id.to_owned()).bounds().size.height))
}

/// The window on `app`'s Progress page, from its entry in the sidebar, once it is read.
fn open_progress<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<ProgressPage>,
    &'a mut VisualTestContext,
) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the project's Progress entry shows", |cx| {
        present(cx, "tree-progress-app")
    });
    cx.update(|window, cx| window.click("tree-progress-app", cx));
    until(&shell, cx, "the progress is read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .progress_page()
                .is_some_and(|page| page.read(cx).is_read())
        })
    });
    let page = shell.read_with(cx, |s, cx| {
        s.main().read(cx).progress_page().unwrap().clone()
    });
    settle(cx);
    (shell, page, cx)
}

/// The keys of each group of the milestone `id`, as the page holds them.
fn groups(
    page: &Entity<ProgressPage>,
    cx: &mut VisualTestContext,
    id: &str,
) -> Vec<(String, Vec<String>)> {
    page.read_with(cx, |page, _| page.groups_of(id))
}

fn unfold(cx: &mut VisualTestContext, id: &str) {
    cx.update(|window, cx| window.click(format!("progress-milestone-{id}"), cx));
    settle(cx);
}

#[gpui_kit::test]
fn the_progress_page_lists_the_open_milestones_with_their_bars(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(review_pull()));
    workspace_on(&user, "sign-in", "#2");
    refresh(&user);

    let (_shell, page, cx) = open_progress(&user, cx);

    let open: Vec<String> = page.read_with(cx, |page, _| page.open_names());
    assert_eq!(open, ["v0.3", "Feedback"], "the open ones, the due first");
    assert_eq!(
        label(cx, "progress-milestone-github-1-name").as_deref(),
        Some("v0.3")
    );
    assert_eq!(
        label(cx, "progress-milestone-github-1-meta").as_deref(),
        Some("GitHub milestone · due 2026-12-05")
    );
    assert_eq!(
        label(cx, "progress-milestone-github-2-meta").as_deref(),
        Some("GitHub milestone · no due date")
    );
    assert_eq!(
        label(cx, "progress-milestone-github-1-counts").as_deref(),
        Some("1 done · 1 in review · 1 in progress · 1 to do")
    );
    // The bar: each part as wide as its share, the rest the track's (to do).
    let whole = width(cx, "progress-milestone-github-1-bar");
    for part in ["done", "review", "progress"] {
        let share = width(cx, &format!("progress-milestone-github-1-bar-{part}"));
        assert!(
            (share - whole / 4.).abs() < 1.5,
            "{part}: {share} of {whole}"
        );
    }
    assert!(present(cx, "progress-legend"));
    for part in ["done", "review", "progress", "todo"] {
        assert!(present(cx, &format!("progress-legend-{part}")), "{part}");
    }
    // Folded at first: no ticket is drawn.
    assert!(!present(cx, "progress-github-1-0-1"));
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

#[gpui_kit::test]
fn a_github_milestone_groups_its_issues_as_to_do_in_progress_in_review_done(
    cx: &mut TestAppContext,
) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(review_pull()));
    workspace_on(&user, "sign-in", "#2");
    refresh(&user);

    let (_shell, page, cx) = open_progress(&user, cx);
    unfold(cx, "github-1");

    assert_eq!(
        groups(&page, cx, "github-1"),
        [
            ("To do".to_owned(), vec!["1".to_owned()]),
            ("In progress".to_owned(), vec!["2".to_owned()]),
            ("In review".to_owned(), vec!["3".to_owned()]),
            ("Done".to_owned(), vec!["4".to_owned()]),
        ]
    );
    for (index, title) in ["To do", "In progress", "In review", "Done"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            label(cx, &format!("progress-github-1-group-{index}-title")).as_deref(),
            Some(title)
        );
    }
    // Each line: the key and the title.
    assert_eq!(
        label(cx, "progress-github-1-0-1-key").as_deref(),
        Some("#1")
    );
    assert_eq!(
        label(cx, "progress-github-1-0-1-title").as_deref(),
        Some("Connections in the window")
    );
    // A ticket in no milestone shows in none.
    let keys: Vec<String> = groups(&page, cx, "github-2")
        .into_iter()
        .flat_map(|(_, keys)| keys)
        .collect();
    assert_eq!(keys, ["5"]);
}

#[gpui_kit::test]
fn a_jira_version_groups_its_tickets_by_the_project_s_own_statuses_in_category_order(
    cx: &mut TestAppContext,
) {
    let user = user();
    on_jira(&user);
    let item = |key: &str, status: &str, category: Option<&str>| {
        json!({"key": key, "self": format!("https://{SITE}/rest/api/3/issue/1"),
            "fields": {"summary": format!("Work {key}"),
                "status": {"name": status,
                    "statusCategory": category.map(|key| json!({"key": key}))},
                "fixVersions": [{"name": "2.4"}], "labels": [], "updated": OLD}})
    };
    user.acli_script(json!({
        "site": SITE,
        "versions": {"ABC": [
            {"id": "10001", "name": "2.4", "released": false, "releaseDate": "2026-11-28"},
            {"id": "10000", "name": "2.3", "released": true, "releaseDate": "2026-10-01"}
        ]},
        "items": {
            "ABC-1": item("ABC-1", "Code review", Some("indeterminate")),
            "ABC-2": item("ABC-2", "Backlog", Some("new")),
            "ABC-3": item("ABC-3", "Shipped", Some("done")),
            "ABC-4": item("ABC-4", "Building", Some("indeterminate")),
            "ABC-5": item("ABC-5", "Limbo", None),
        }
    }));
    refresh(&user);

    let (_shell, page, cx) = open_progress(&user, cx);

    assert_eq!(
        label(cx, "progress-milestone-jira-ABC-10001-meta").as_deref(),
        Some("Jira version of ABC · due 2026-11-28")
    );
    assert_eq!(
        label(cx, "progress-milestone-jira-ABC-10001-counts").as_deref(),
        Some("1 done · 2 in progress · 1 to do · 1 other")
    );
    unfold(cx, "jira-ABC-10001");
    // The project's own statuses, To Do's first, then In Progress's, then Done's; one whose
    // category is not read is under Other, last.
    assert_eq!(
        groups(&page, cx, "jira-ABC-10001"),
        [
            ("Backlog".to_owned(), vec!["ABC-2".to_owned()]),
            ("Building".to_owned(), vec!["ABC-4".to_owned()]),
            ("Code review".to_owned(), vec!["ABC-1".to_owned()]),
            ("Shipped".to_owned(), vec!["ABC-3".to_owned()]),
            ("Other".to_owned(), vec!["ABC-5".to_owned()]),
        ]
    );
    assert_eq!(
        label(cx, "progress-jira-ABC-10001-group-4-sentence").as_deref(),
        Some("Jira gave no status category for these")
    );
    // A released version is in the Closed fold.
    let closed: Vec<String> = page.read_with(cx, |page, _| page.closed_names());
    assert_eq!(closed, ["2.3"]);
}

#[gpui_kit::test]
fn a_ticket_shows_the_session_on_it_its_pull_request_and_its_checks(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    // #8 is read open first: a pull request merged before the first round is listed without its
    // description, which says what it closes.
    user.gh_script(github_script(json!({
        "bind-vault": pull(9, "bind-vault", "OPEN", "Closes #3", running_check()),
        "daemon-part": pull(8, "daemon-part", "OPEN", "Closes #4", json!([])),
    })));
    refresh(&user);
    let mut merged = pull(8, "daemon-part", "MERGED", "Closes #4", json!([]));
    merged["updatedAt"] = json!(NEW);
    user.gh_script(github_script(json!({
        "bind-vault": pull(9, "bind-vault", "OPEN", "Closes #3", running_check()),
        "daemon-part": merged,
    })));
    workspace_on(&user, "sign-in", "#2");
    let run = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "sign-in", "message": "Sign it in"}),
        )
        .unwrap();
    let session = run["agent"]["id"].as_str().unwrap().to_owned();
    refresh(&user);

    let (shell, _page, cx) = open_progress(&user, cx);
    unfold(cx, "github-1");

    let chip = format!("progress-github-1-1-2-session-{session}");
    until(&shell, cx, "the session's chip shows", |cx| {
        present(cx, &chip)
    });
    let said = label(cx, &chip).unwrap();
    assert!(said.starts_with("Session "), "{said}");
    assert!(
        ["running", "waits for you", "at rest"]
            .iter()
            .any(|state| said.ends_with(state)),
        "{said}"
    );
    assert_eq!(
        label(cx, "progress-github-1-2-3-pull-.-9").as_deref(),
        Some("PR #9")
    );
    assert_eq!(
        label(cx, "progress-github-1-2-3-checks-.-9").as_deref(),
        Some("checks running")
    );
    assert_eq!(
        label(cx, "progress-github-1-3-4-merged-.-8").as_deref(),
        Some("PR #8 merged")
    );

    // The session ends: its chip says so.
    user.call("agent.stop", json!({"agent": session})).unwrap();
    until(&shell, cx, "the chip says the session ended", |cx| {
        label(cx, &chip).is_some_and(|said| said.ends_with("ended"))
    });
}

#[gpui_kit::test]
fn an_unfolded_milestone_scrolls_inside_its_maximum_height(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    let mut script = github_script(json!({}));
    for number in 10..70 {
        script["issues"]["acme/app"][number.to_string()] = issue(
            number,
            &format!("Ticket {number}"),
            "OPEN",
            Some((2, "Feedback")),
        );
    }
    user.gh_script(script);
    refresh(&user);

    let (_shell, _page, cx) = open_progress(&user, cx);
    unfold(cx, "github-2");
    unfold(cx, "github-1");

    let most = f32::from(flottille_window::progress::TICKETS_HEIGHT);
    let tickets = height(cx, "progress-milestone-github-2-tickets");
    assert!(
        tickets <= most + 0.5,
        "the tickets are {tickets} high, at most {most}"
    );
    assert!(tickets > most - 40., "a long list fills it: {tickets}");
    // Both stay unfolded, the second below the first's frame.
    assert!(present(cx, "progress-github-1-0-1"));
    assert!(present(cx, "progress-github-2-0-10"));
    // Its rows scroll inside it: the last is drawn below the frame's bottom.
    let (frame, last) = cx.update(|window, _| {
        (
            window.find("progress-milestone-github-2-tickets").bounds(),
            window.find("progress-github-2-0-69").bounds(),
        )
    });
    assert!(last.top() >= frame.bottom(), "{last:?} under {frame:?}");
}

#[gpui_kit::test]
fn a_ticket_that_changes_group_folds_out_and_in_and_the_bar_follows(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(review_pull()));
    refresh(&user);

    let (shell, page, cx) = open_progress(&user, cx);
    unfold(cx, "github-1");
    let done_before = width(cx, "progress-milestone-github-1-bar-done");
    assert!(present(cx, "progress-github-1-2-3"));

    // #3's pull request is merged and the issue closed: it goes from In review to Done.
    let mut script = github_script(json!({
        "bind-vault": pull(9, "bind-vault", "MERGED", "Closes #3", json!([]))
    }));
    script["issues"]["acme/app"]["3"]["state"] = json!("CLOSED");
    script["issues"]["acme/app"]["3"]["updatedAt"] = json!(NEW);
    script["pulls"]["acme/app"]["bind-vault"]["updatedAt"] = json!(NEW);
    user.gh_script(script);
    refresh(&user);
    until(&shell, cx, "the page reads the change", |cx| {
        groups(&page, cx, "github-1")
            .iter()
            .any(|(group, keys)| group == "Done" && keys.contains(&"3".to_owned()))
    });

    // Mid-way: it unfolds in Done while it folds away from In review, and the bar's done part
    // grows toward its new share.
    a_frame(cx);
    a_frame(cx);
    assert!(present(cx, "progress-github-1-3-3"), "it arrives in Done");
    let mid = width(cx, "progress-milestone-github-1-bar-done");
    assert!(mid > done_before + 0.5, "the done part grows: {mid}");
    let whole = width(cx, "progress-milestone-github-1-bar");
    assert!(
        mid < whole / 2. - 0.5,
        "it has not reached its share yet: {mid}"
    );
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "it moves"
    );

    settle(cx);
    assert!(!present(cx, "progress-github-1-2-3"), "it left In review");
    let after = width(cx, "progress-milestone-github-1-bar-done");
    assert!((after - whole / 2.).abs() < 1.5, "{after} of {whole}");
    assert_eq!(
        label(cx, "progress-milestone-github-1-counts").as_deref(),
        Some("2 done · 0 in review · 0 in progress · 2 to do")
    );
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn closed_milestones_sit_in_a_closed_fold(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(json!({})));
    refresh(&user);

    let (_shell, page, cx) = open_progress(&user, cx);

    assert_eq!(page.read_with(cx, |page, _| page.closed_names()), ["v0.2"]);
    assert_eq!(label(cx, "progress-closed").as_deref(), Some("1 closed"));
    assert!(
        !present(cx, "progress-milestone-github-3"),
        "folded at first"
    );
    cx.update(|window, cx| window.click("progress-closed", cx));
    a_frame(cx);
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "it unfolds"
    );
    settle(cx);
    assert!(present(cx, "progress-milestone-github-3"));
    assert_eq!(
        label(cx, "progress-milestone-github-3-counts").as_deref(),
        Some("1 done · 0 in review · 0 in progress · 0 to do")
    );
}

#[gpui_kit::test]
fn a_chip_opens_its_session(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(json!({})));
    workspace_on(&user, "sign-in", "#2");
    let run = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "sign-in", "message": "Sign it in"}),
        )
        .unwrap();
    let session = run["agent"]["id"].as_str().unwrap().to_owned();
    refresh(&user);

    let (shell, _page, cx) = open_progress(&user, cx);
    unfold(cx, "github-1");
    let chip = format!("progress-github-1-1-2-session-{session}");
    until(&shell, cx, "the session's chip shows", |cx| {
        present(cx, &chip)
    });

    cx.update(|window, cx| window.click(chip.clone(), cx));
    until(&shell, cx, "the session opens", |cx| {
        shell.read_with(cx, |s, _| {
            s.selected() == Some(&Selection::Session(session.clone()))
        })
    });
}

#[gpui_kit::test]
fn the_line_between_two_milestones_folds_with_the_one_that_leaves(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(json!({})));
    refresh(&user);

    let (shell, page, cx) = open_progress(&user, cx);
    // One line, between the two open milestones.
    assert!(present(cx, "progress-open-separator-1"));
    assert!(!present(cx, "progress-open-separator-2"));
    let before = height(cx, "progress-open");

    // Feedback is closed on the tracker (its issue is touched, so that the round says so).
    let mut script = github_script(json!({}));
    script["milestones"]["acme/app"][1]["state"] = json!("closed");
    script["issues"]["acme/app"]["5"]["title"] = json!("A CI run starts on its branch, again");
    script["issues"]["acme/app"]["5"]["updatedAt"] = json!(NEW);
    user.gh_script(script);
    refresh(&user);
    until(&shell, cx, "the page reads the change", |cx| {
        page.read_with(cx, |page, _| page.open_names()) == ["v0.3"]
    });

    // Mid-way: the row is still there, folding with its line, the card shrinking by degrees.
    a_frame(cx);
    a_frame(cx);
    assert!(present(cx, "progress-milestone-github-2"), "it folds away");
    let mid = height(cx, "progress-open");
    assert!(mid < before - 0.5, "the card has begun to fold: {mid}");
    settle(cx);
    let after = height(cx, "progress-open");
    assert!(after < mid - 0.5, "it kept folding: {after} after {mid}");
    assert!(!present(cx, "progress-milestone-github-2"));
    assert!(
        !present(cx, "progress-open-separator-1"),
        "a line between two milestones is drawn only while there are two"
    );
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

fn opened_links(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Arc<Mutex<Vec<PathBuf>>> {
    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    opened
}

#[gpui_kit::test]
fn a_ticket_key_opens_the_ticket(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(json!({})));
    refresh(&user);

    let (shell, _page, cx) = open_progress(&user, cx);
    unfold(cx, "github-1");
    let opened = opened_links(&shell, cx);

    cx.update(|window, cx| window.click("progress-github-1-0-1-key", cx));

    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://github.com/acme/app/issues/1")]
    );
}

#[gpui_kit::test]
fn jira_s_other_tickets_have_a_colour_of_their_own_in_the_bar_and_the_legend(
    cx: &mut TestAppContext,
) {
    let user = user();
    on_jira(&user);
    let item = |key: &str, status: &str, category: Option<&str>| {
        json!({"key": key, "self": format!("https://{SITE}/rest/api/3/issue/1"),
            "fields": {"summary": format!("Work {key}"),
                "status": {"name": status,
                    "statusCategory": category.map(|key| json!({"key": key}))},
                "fixVersions": [{"name": "2.4"}], "labels": [], "updated": OLD}})
    };
    user.acli_script(json!({
        "site": SITE,
        "versions": {"ABC": [
            {"id": "10001", "name": "2.4", "released": false, "releaseDate": "2026-11-28"}
        ]},
        "items": {
            "ABC-1": item("ABC-1", "Building", Some("indeterminate")),
            "ABC-2": item("ABC-2", "Backlog", Some("new")),
            "ABC-3": item("ABC-3", "Shipped", Some("done")),
            "ABC-4": item("ABC-4", "Building", Some("indeterminate")),
            "ABC-5": item("ABC-5", "Limbo", None),
        }
    }));
    refresh(&user);

    let (_shell, _page, cx) = open_progress(&user, cx);

    assert!(present(cx, "progress-legend-other"));
    let whole = width(cx, "progress-milestone-jira-ABC-10001-bar");
    let other = width(cx, "progress-milestone-jira-ABC-10001-bar-other");
    assert!((other - whole / 5.).abs() < 1.5, "{other} of {whole}");
}

#[gpui_kit::test]
fn no_other_in_the_legend_when_no_ticket_is_other(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(review_pull()));
    refresh(&user);

    let (_shell, _page, cx) = open_progress(&user, cx);

    assert!(!present(cx, "progress-legend-other"));
}

#[gpui_kit::test]
fn a_version_whose_search_fails_says_so_on_its_row(cx: &mut TestAppContext) {
    let user = user();
    on_jira(&user);
    let item = |key: &str, version: &str| {
        json!({"key": key, "self": format!("https://{SITE}/rest/api/3/issue/1"),
            "fields": {"summary": format!("Work {key}"),
                "status": {"name": "Building", "statusCategory": {"key": "indeterminate"}},
                "fixVersions": [{"name": version}], "labels": [], "updated": OLD}})
    };
    user.acli_script(json!({
        "site": SITE,
        "versions": {"ABC": [
            {"id": "10001", "name": "2.4", "released": false, "releaseDate": "2026-11-28"},
            {"id": "10002", "name": "2.5", "released": false, "releaseDate": "2026-12-20"}
        ]},
        "items": {"ABC-1": item("ABC-1", "2.4"), "ABC-2": item("ABC-2", "2.5")},
        "fail": {"search_jql": {"fixVersion = \"2.5\"": "✗ Error: the search broke"}},
    }));
    refresh(&user);

    let (_shell, _page, cx) = open_progress(&user, cx);

    assert_eq!(
        label(cx, "progress-milestone-jira-ABC-10001-counts").as_deref(),
        Some("0 done · 1 in progress · 0 to do")
    );
    let said = label(cx, "progress-milestone-jira-ABC-10002-counts").unwrap();
    assert!(said.contains("could not be read"), "{said}");
    assert!(!present(cx, "progress-problem"), "the page still shows");
}

/// Where the page says it could not read the milestones: the line is a block of its own, which
/// holds the space above it (#884), and sits where it sat under the gap of the page's column.
#[gpui_kit::test]
fn the_line_that_says_the_milestones_were_not_read_sits_where_the_gap_put_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    on_github(&user);
    let mut script = github_script(json!({}));
    script["fail"] = json!({"acme/app": "HTTP 502: Bad Gateway"});
    user.gh_script(script);
    user.call("items.refresh", json!({"project": "app"})).ok();

    let (_shell, _page, cx) = open_progress(&user, cx);

    assert!(present(cx, "progress-problem"), "the page says why");
    let (header, line) = cx.update(|window, _| {
        (
            window.find("progress-header").bounds(),
            window.find("progress-problem").bounds(),
        )
    });
    let under = f32::from(line.top() - header.bottom());
    // Measured on `origin/dev`, where the column's gap put it: legend, an empty list, then the line.
    assert_eq!(under, 84.0);
}

#[gpui_kit::test]
fn an_open_issue_whose_only_pull_request_was_closed_unmerged_is_to_do(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    // Read open first: a pull request first read closed is listed without its description.
    user.gh_script(github_script(json!({
        "bind-vault": pull(9, "bind-vault", "OPEN", "Closes #3", json!([]))
    })));
    refresh(&user);
    let mut closed = pull(9, "bind-vault", "CLOSED", "Closes #3", json!([]));
    closed["updatedAt"] = json!(NEW);
    user.gh_script(github_script(json!({"bind-vault": closed})));
    refresh(&user);

    let (_shell, page, cx) = open_progress(&user, cx);

    assert_eq!(
        groups(&page, cx, "github-1"),
        [
            (
                "To do".to_owned(),
                vec!["1".to_owned(), "2".to_owned(), "3".to_owned()]
            ),
            ("Done".to_owned(), vec!["4".to_owned()]),
        ]
    );
}

#[gpui_kit::test]
fn closed_milestones_are_sorted_by_when_they_closed(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    let mut script = github_script(json!({}));
    // v0.2 is due after v0.1 but was closed before it.
    script["milestones"]["acme/app"][2]["closed_at"] = json!("2026-09-01T00:00:00Z");
    script["milestones"]["acme/app"]
        .as_array_mut()
        .unwrap()
        .push(json!({"number": 4, "title": "v0.1", "state": "closed",
            "due_on": "2026-08-01T00:00:00Z", "closed_at": "2026-11-01T00:00:00Z",
            "open_issues": 0, "closed_issues": 0,
            "html_url": "https://github.com/acme/app/milestone/4"}));
    user.gh_script(script);
    refresh(&user);

    let (_shell, page, cx) = open_progress(&user, cx);

    assert_eq!(
        page.read_with(cx, |page, _| page.closed_names()),
        ["v0.1", "v0.2"],
        "the last closed first"
    );
}

#[gpui_kit::test]
fn a_milestone_that_goes_leaves_nothing_kept_of_it(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    user.gh_script(github_script(json!({})));
    refresh(&user);

    let (shell, page, cx) = open_progress(&user, cx);
    unfold(cx, "github-2");
    assert_eq!(page.read_with(cx, |page, _| page.kept()), (3, 3));

    // Feedback is deleted on the tracker (its issue is touched, so that the round says so).
    let mut script = github_script(json!({}));
    script["milestones"]["acme/app"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    script["issues"]["acme/app"]["5"]["milestone"] = json!(null);
    script["issues"]["acme/app"]["5"]["updatedAt"] = json!(NEW);
    user.gh_script(script);
    refresh(&user);
    until(&shell, cx, "the page reads the change", |cx| {
        page.read_with(cx, |page, _| page.open_names()) == ["v0.3"]
    });
    settle(cx);

    assert_eq!(page.read_with(cx, |page, _| page.kept()), (2, 2));
}

#[gpui_kit::test]
fn the_project_s_row_opens_its_page_from_its_progress(cx: &mut TestAppContext) {
    let user = user();
    on_github(&user);
    let (shell, _page, cx) = open_progress(&user, cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Progress("app".to_owned()))
    );

    // The click runs in the project's own node, which is also the one that holds the Progress
    // row's mark: selecting must not update it twice.
    cx.update(|window, cx| window.click("project-app", cx));
    settle(cx);

    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("app".to_owned()))
    );
    assert!(shell.read_with(cx, |s, cx| s.main().read(cx).progress_page().is_none()));
    assert_eq!(
        cx.update(|window, _| window.find("tree-progress-app").selected()),
        Some(false)
    );
}
