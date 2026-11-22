//! The scenarios of a project's Routines tab (#1123, `docs/design.md`, "Routines"), on GPUI's test
//! platform in front of a real daemon whose routines run on the fake ACP agent: the tab and its
//! count of results not yet opened, the list of the project's routines with their last run and
//! an unread dot, a routine's page with its history, the result page (the report without the
//! conversation, and what Flottille knows of a run whose session goes on), Run now and Pause, and
//! the form that creates and edits a routine. No window opens on a screen, no real agent runs.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The prompts of the routines, and what the fake agent does on each (as `routines.rs`).
const LIST_REVIEWS: &str = "List the tickets waiting for my review";
const NIGHTLY: &str = "Read the nightly run of the tests";
const BRANCH: &str = "Open a branch for the fix";
const HOLDING: &str = "Look through the whole backlog slowly";

fn script() -> Value {
    json!({"turns": [
        {"when": LIST_REVIEWS, "do": [
            {"say": "Four tickets wait."},
            {"mcp": {"tool": "routine_result", "arguments": {"summary": "4 to review"}}}]},
        {"when": NIGHTLY, "do": [
            {"mcp": {"tool": "routine_result", "arguments":
                {"summary": "Nightly green: nothing to do", "nothing": true}}}]},
        {"when": BRANCH, "do": [
            {"say": "The branch fix/7 is open and its tests run."},
            {"mcp": {"tool": "routine_result", "arguments":
                {"summary": "fix/7 opened, tests run", "goes_on": true}}}]},
        {"when": HOLDING, "do": [{"hold": "release"}]},
    ]})
}

fn user() -> User {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    user.fake_script(script());
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(25);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A routine of `app`, on weekdays at 08:30 UTC unless `extra` says otherwise.
fn add(user: &User, name: &str, prompt: &str, extra: Value) -> Value {
    let mut params = json!({"name": name, "project": "app", "prompt": prompt,
        "schedule": "weekdays 08:30"});
    for (key, value) in extra.as_object().into_iter().flatten() {
        params[key] = value.clone();
    }
    user.call("routine.add", params).unwrap()["routine"].clone()
}

fn runs(user: &User, name: &str) -> Vec<Value> {
    user.call("routine.show", json!({"routine": name})).unwrap()["runs"]
        .as_array()
        .unwrap()
        .clone()
}

/// Runs the routine now and waits for its result.
fn run_now(user: &User, name: &str) -> Value {
    let before = runs(user, name).len();
    user.call("routine.run", json!({"routine": name})).unwrap();
    eventually("the run's end", || {
        let all = runs(user, name);
        all.len() > before && all[0]["outcome"] != "running"
    });
    runs(user, name)[0].clone()
}

fn unread(user: &User, name: &str) -> u64 {
    user.call("routine.list", json!({"project": "app"}))
        .unwrap()["routines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|routine| routine["name"] == name)
        .unwrap()["unread"]
        .as_u64()
        .unwrap()
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

/// Clicks `id` once what moves has settled: a row that still unfolds has no middle to click.
fn click(cx: &mut VisualTestContext, id: &str) {
    settle(cx);
    cx.update(|window, cx| window.click(id.to_owned(), cx));
    settle(cx);
}

/// The window on `app`'s page, once the page shows its tabs.
fn open_page<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the project's row shows", |cx| {
        present(cx, "project-app")
    });
    click(cx, "project-app");
    until(&shell, cx, "the page shows its tabs", |cx| {
        present(cx, "project-tabs-work")
    });
    (shell, cx)
}

/// The window on `app`'s Routines tab.
fn open_tab<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = open_page(user, cx);
    click(cx, "project-tabs-routines");
    (shell, cx)
}

/// The window on a routine's page.
fn open_routine<'a>(
    user: &User,
    name: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = open_tab(user, cx);
    let row = format!("routine-{name}");
    until(&shell, cx, "the routine's row shows", |cx| {
        present(cx, &row)
    });
    click(cx, &row);
    until(&shell, cx, "the routine's page shows", |cx| {
        present(cx, "routine-header")
    });
    (shell, cx)
}

fn tab_count(cx: &mut VisualTestContext) -> Option<String> {
    label(cx, "project-tabs-routines-count")
}

#[gpui_kit::test]
fn the_routines_tab_lists_the_project_routines_with_their_last_run(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    add(
        &user,
        "Nightly",
        NIGHTLY,
        json!({"schedule": "every day 07:00", "mode": "ask"}),
    );
    run_now(&user, "Tickets to review");

    let (shell, cx) = open_tab(&user, cx);
    until(&shell, cx, "the routines are listed", |cx| {
        present(cx, "routine-Tickets to review") && present(cx, "routine-Nightly")
    });

    assert_eq!(
        label(cx, "routine-Tickets to review").as_deref(),
        Some("Tickets to review"),
        "a row's label is its name"
    );
    assert_eq!(
        label(cx, "routine-Tickets to review-line").as_deref(),
        Some("weekdays 08:30")
    );
    assert_eq!(
        label(cx, "routine-Tickets to review-tag-0").as_deref(),
        Some("Read only")
    );
    let last = label(cx, "routine-Tickets to review-meta").unwrap();
    assert!(last.ends_with("4 to review"), "its last run: {last}");
    assert_eq!(label(cx, "routine-Nightly-tag-0").as_deref(), Some("Ask"));
    assert_eq!(
        label(cx, "routine-Nightly-line").as_deref(),
        Some("every day 07:00")
    );
    assert_eq!(
        label(cx, "routine-Nightly-meta").as_deref(),
        Some("Not run yet")
    );
}

#[gpui_kit::test]
fn the_tab_counts_unread_results(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    add(&user, "Nightly", NIGHTLY, json!({}));

    let (shell, cx) = open_page(&user, cx);
    settle(cx);
    assert!(
        present(cx, "project-tabs-routines"),
        "the tab is in every project"
    );
    assert_eq!(tab_count(cx), None, "nothing to open: no count");

    run_now(&user, "Nightly");
    run_now(&user, "Tickets to review");
    until(&shell, cx, "the count shows", |cx| tab_count(cx).is_some());
    assert_eq!(
        tab_count(cx).as_deref(),
        Some("1"),
        "a result with nothing to say is not one to open"
    );

    run_now(&user, "Tickets to review");
    until(&shell, cx, "the count follows", |cx| {
        tab_count(cx).as_deref() == Some("2")
    });
}

#[gpui_kit::test]
fn an_unread_result_has_a_dot_until_opened(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    add(&user, "Nightly", NIGHTLY, json!({}));
    let run = run_now(&user, "Tickets to review");
    run_now(&user, "Nightly");
    assert_eq!(unread(&user, "Tickets to review"), 1);

    let (shell, cx) = open_tab(&user, cx);
    until(&shell, cx, "the dot shows", |cx| {
        present(cx, "routine-Tickets to review-dot")
    });
    assert!(
        !present(cx, "routine-Nightly-dot"),
        "a result with nothing to say has no dot"
    );

    click(cx, "routine-Tickets to review");
    until(&shell, cx, "the history is listed", |cx| {
        present(cx, &format!("routine-run-{}", run["id"]))
    });
    click(cx, &format!("routine-run-{}", run["id"]));
    until(&shell, cx, "the daemon knows it was opened", |_| {
        unread(&user, "Tickets to review") == 0
    });
    click(cx, "routines-back");
    settle(cx);
    assert!(
        !present(cx, "routine-Tickets to review-dot"),
        "the dot is gone once the result was opened"
    );
    assert_eq!(tab_count(cx), None, "and so is the count");
}

#[gpui_kit::test]
fn the_history_lists_runs_and_opens_an_ended_session(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    let first = run_now(&user, "Tickets to review");
    let second = run_now(&user, "Tickets to review");
    let session = first["session"].as_str().unwrap().to_owned();
    eventually("its session ended", || {
        user.call("agent.show", json!({"agent": session})).unwrap()["agent"]["state"] == "ended"
    });

    let (shell, cx) = open_routine(&user, "Tickets to review", cx);
    for run in [&first, &second] {
        let row = format!("routine-run-{}", run["id"]);
        until(&shell, cx, "the run is listed", |cx| present(cx, &row));
        assert_eq!(
            label(cx, &format!("{row}-line")).as_deref(),
            Some("4 to review")
        );
    }
    // The last run first.
    let (newer, older) = cx.update(|window, _| {
        let at = |run: &Value| {
            f32::from(
                window
                    .find(format!("routine-run-{}", run["id"]))
                    .bounds()
                    .origin
                    .y,
            )
        };
        (at(&second), at(&first))
    });
    assert!(newer < older, "the last run is the first row");

    click(cx, &format!("routine-run-{}", first["id"]));
    until(&shell, cx, "the result page shows", |cx| {
        present(cx, "result-open-session")
    });
    click(cx, "result-open-session");
    until(&shell, cx, "the ended session opens", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .session_page()
                .is_some_and(|page| page.read(cx).id() == session)
        })
    });
}

#[gpui_kit::test]
fn the_result_page_shows_the_report_without_the_conversation(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    let run = run_now(&user, "Tickets to review");

    let (shell, cx) = open_routine(&user, "Tickets to review", cx);
    let row = format!("routine-run-{}", run["id"]);
    until(&shell, cx, "the run is listed", |cx| present(cx, &row));
    click(cx, &row);
    until(&shell, cx, "the result page shows", |cx| {
        present(cx, "result-report")
    });

    assert_eq!(
        label(cx, "routines-path")
            .as_deref()
            .map(|p| p.starts_with("Routines / Tickets to review")),
        Some(true),
        "a path line leads back"
    );
    assert_eq!(
        label(cx, "result-summary").as_deref(),
        Some("4 to review"),
        "the run's summary"
    );
    assert!(
        label(cx, "result-report").unwrap().contains("4 to review"),
        "the report is rendered"
    );
    // The conversation is another page: its composer, and what the agent said while it worked.
    assert!(!present(cx, "session-composer"), "no composer");
    assert!(
        !present(cx, "conversation"),
        "no conversation around the report"
    );
    assert!(
        present(cx, "result-open-session"),
        "the session is one click away"
    );
    assert!(!present(cx, "result-no-session"));
}

#[gpui_kit::test]
fn a_run_under_way_shows_its_session_state_branch_and_pull_request(cx: &mut TestAppContext) {
    let user = user();
    add(
        &user,
        "Fix",
        BRANCH,
        json!({"mode": "auto", "schedule": "every day 07:00"}),
    );
    let run = run_now(&user, "Fix");
    assert_eq!(run["outcome"], "goes_on", "{run}");
    let session = run["session"].as_str().unwrap().to_owned();
    eventually("its turn ends", || {
        user.call("agent.show", json!({"agent": session})).unwrap()["agent"]["state"] == "idle"
    });

    let (shell, cx) = open_routine(&user, "Fix", cx);
    let row = format!("routine-run-{}", run["id"]);
    until(&shell, cx, "the run is listed", |cx| present(cx, &row));
    click(cx, &row);
    until(&shell, cx, "what is known of the session is read", |cx| {
        present(cx, "result-status-state") && present(cx, "result-status-last")
    });

    let state = label(cx, "result-status-state").unwrap();
    assert!(
        state.contains("Waiting") || state.contains("Running") || state.contains("Idle"),
        "its state: {state}"
    );
    assert!(
        label(cx, "result-status-branch").is_some_and(|b| !b.is_empty()),
        "its branch"
    );
    assert_eq!(
        label(cx, "result-status-pull").as_deref(),
        Some("Pull request No pull request yet"),
        "its pull request, none here"
    );
    assert_eq!(
        label(cx, "result-status-requests").as_deref(),
        Some("Requests No request waits for you")
    );
    let last = label(cx, "result-status-last").unwrap();
    assert!(
        last.contains("The branch fix/7 is open"),
        "the agent's last message: {last}"
    );
    assert!(present(cx, "result-open-session"));
}

#[gpui_kit::test]
fn run_now_starts_a_run(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));

    let (shell, cx) = open_routine(&user, "Tickets to review", cx);
    assert!(runs(&user, "Tickets to review").is_empty());
    click(cx, "routine-run");
    until(&shell, cx, "the run is listed", |cx| {
        runs(&user, "Tickets to review")
            .first()
            .is_some_and(|run| present(cx, &format!("routine-run-{}", run["id"])))
    });
    let all = runs(&user, "Tickets to review");
    assert_eq!(all.len(), 1, "one run, started by the click");
    assert!(all[0]["session"].is_string(), "a new session: {}", all[0]);
}

#[gpui_kit::test]
fn pause_stops_the_next_runs(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    let paused = |user: &User| {
        user.call("routine.show", json!({"routine": "Tickets to review"}))
            .unwrap()["routine"]["paused"]
            == true
    };

    let (shell, cx) = open_routine(&user, "Tickets to review", cx);
    assert_eq!(label(cx, "routine-pause").as_deref(), Some("Pause"));
    click(cx, "routine-pause");
    until(&shell, cx, "it is paused", |_| paused(&user));
    let routine = user
        .call("routine.show", json!({"routine": "Tickets to review"}))
        .unwrap();
    assert!(
        routine["routine"]["next_at"].is_null(),
        "no next run while paused"
    );
    until(&shell, cx, "the button offers to resume", |cx| {
        label(cx, "routine-pause").as_deref() == Some("Resume")
    });

    click(cx, "routine-pause");
    until(&shell, cx, "it runs again", |_| !paused(&user));
}

#[gpui_kit::test]
fn a_routine_never_run_says_when_it_will(cx: &mut TestAppContext) {
    let user = user();
    add(
        &user,
        "Nightly",
        NIGHTLY,
        json!({"schedule": "every day 07:00"}),
    );

    let (_shell, cx) = open_routine(&user, "Nightly", cx);
    let next = label(cx, "routine-next").expect("the page says its next run");
    assert!(
        next.starts_with("Next: ") && next.ends_with("07:00"),
        "the next run: {next}"
    );
    assert!(!present(cx, "routine-history-list"), "no run to list yet");
    assert!(
        present(cx, "routine-history-empty"),
        "and the history says so"
    );
}

#[gpui_kit::test]
fn a_project_without_routine_offers_one(cx: &mut TestAppContext) {
    let user = user();

    let (shell, cx) = open_tab(&user, cx);
    until(&shell, cx, "the empty state shows", |cx| {
        present(cx, "routines-empty")
    });
    let said = label(cx, "routines-empty").unwrap();
    assert!(
        said.contains("routine") && said.contains("starts"),
        "it says what a routine is: {said}"
    );
    click(cx, "routines-empty-new");
    until(&shell, cx, "the form opens", |cx| {
        present(cx, "routine-form")
    });
}

fn open_form<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = open_tab(user, cx);
    until(&shell, cx, "the tab is read", |cx| {
        present(cx, "routines-empty") || present(cx, "routines-new")
    });
    let button = if present(cx, "routines-new") {
        "routines-new"
    } else {
        "routines-empty-new"
    };
    click(cx, button);
    until(&shell, cx, "the form opens", |cx| {
        present(cx, "routine-form")
    });
    (shell, cx)
}

#[gpui_kit::test]
fn the_form_shows_the_event_fields_when_an_event_is_chosen(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = open_form(&user, cx);

    // On a schedule at first: days and an hour, never a cron line.
    assert!(present(cx, "routine-days"), "days");
    assert!(present(cx, "routine-hour-box"), "an hour");
    assert!(!present(cx, "routine-event-kind"), "no event kind yet");

    click(cx, "routine-trigger-choice-event");
    until(&shell, cx, "the event's kind shows", |cx| {
        present(cx, "routine-event-kind")
    });
    assert!(
        !present(cx, "routine-days"),
        "the schedule's fields folded away"
    );
    // The kind chosen first: a workflow run ends.
    for field in [
        "routine-event-repository-box",
        "routine-event-workflow-box",
        "routine-event-branch-box",
        "routine-event-started-by",
        "routine-event-outcome",
    ] {
        assert!(present(cx, field), "a workflow run's {field}");
    }
    // Every kind: not before, at most once, a new session.
    for field in [
        "routine-event-not-before-box",
        "routine-event-at-most-once",
        "routine-event-new-session",
    ] {
        assert!(present(cx, field), "{field}");
    }

    click(cx, "routine-event-kind");
    click(cx, "routine-event-kind-review_requested");
    until(&shell, cx, "the fields follow the kind", |cx| {
        !present(cx, "routine-event-workflow-box")
    });
    assert!(
        present(cx, "routine-event-repository-box"),
        "a review's repository"
    );

    click(cx, "routine-event-kind");
    click(cx, "routine-event-kind-assigned");
    until(&shell, cx, "the fields follow the kind", |cx| {
        present(cx, "routine-event-tracker-type-box")
    });
    assert!(present(cx, "routine-event-tracker-project-box"));
    assert!(!present(cx, "routine-event-repository-box"));
}

#[gpui_kit::test]
fn the_form_offers_the_efforts_of_the_model(cx: &mut TestAppContext) {
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"],
            "model_efforts": {"fake-large": ["low", "medium", "max"]}, "said_efforts": true}),
    );
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();

    let (shell, cx) = open_form(&user, cx);
    // Agent, model and effort side by side, as the session's composer offers them.
    until(&shell, cx, "the models are offered", |cx| {
        present(cx, "routine-model-model")
    });
    click(cx, "routine-model-model");
    click(cx, "routine-model-model-claude-fake-large");
    until(&shell, cx, "its efforts are offered", |cx| {
        present(cx, "routine-model-effort")
    });
    click(cx, "routine-model-effort");
    for effort in ["low", "medium", "max"] {
        assert!(
            present(cx, &format!("routine-model-effort-{effort}")),
            "the effort {effort} of the model"
        );
    }
    assert!(
        !present(cx, "routine-model-effort-high"),
        "an effort the model does not offer"
    );
}

#[gpui_kit::test]
fn the_form_says_the_next_run(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = open_form(&user, cx);

    let hour = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_page()
            .unwrap()
            .read(cx)
            .routines()
            .read(cx)
            .form()
            .read(cx)
            .hour_field()
            .clone()
    });
    cx.update(|window, cx| hour.update(cx, |field, cx| field.set_value("08:30", window, cx)));
    click(cx, "routine-days");
    click(cx, "routine-days-weekdays");
    settle(cx);
    let next = label(cx, "routine-next").expect("the form says the next run");
    assert!(
        next.starts_with("Next: ") && next.ends_with("08:30"),
        "the next run: {next}"
    );

    cx.update(|window, cx| hour.update(cx, |field, cx| field.set_value("25:99", window, cx)));
    settle(cx);
    assert!(
        label(cx, "routine-hour-error").is_some(),
        "an hour that is none is said under its field"
    );
    assert!(
        label(cx, "routine-next").is_none_or(|next| !next.ends_with("25:99")),
        "and no next run is said for it"
    );
}

#[gpui_kit::test]
fn the_form_saves_a_routine_the_daemon_keeps(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = open_form(&user, cx);
    let form = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_page()
            .unwrap()
            .read(cx)
            .routines()
            .read(cx)
            .form()
            .clone()
    });
    cx.update(|window, cx| {
        form.update(cx, |form, cx| {
            form.name_field()
                .clone()
                .update(cx, |f, cx| f.set_value("Tickets to review", window, cx));
            form.prompt_field()
                .clone()
                .update(cx, |f, cx| f.set_value(LIST_REVIEWS, window, cx));
            form.hour_field()
                .clone()
                .update(cx, |f, cx| f.set_value("08:30", window, cx));
        })
    });
    settle(cx);
    click(cx, "routine-days");
    click(cx, "routine-days-weekdays");
    click(cx, "routine-save");
    until(&shell, cx, "the routine is kept", |_| {
        user.call("routine.list", json!({"project": "app"}))
            .unwrap()["routines"]
            .as_array()
            .is_some_and(|all| all.len() == 1)
    });
    let kept = user
        .call("routine.list", json!({"project": "app"}))
        .unwrap()["routines"][0]
        .clone();
    assert_eq!(kept["name"], "Tickets to review");
    assert_eq!(kept["schedule"], "weekdays 08:30");
    assert_eq!(kept["mode"], "read_only", "Read only is the default");
    until(&shell, cx, "the form closes and the row shows", |cx| {
        !present(cx, "routine-form") && present(cx, "routine-Tickets to review")
    });
}

#[gpui_kit::test]
fn removing_a_routine_is_asked_once_in_the_dialog_of_the_window(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Nightly", NIGHTLY, json!({}));

    let (shell, cx) = open_routine(&user, "Nightly", cx);
    click(cx, "routine-more");
    click(cx, "routine-remove");
    until(&shell, cx, "the dialog asks", |cx| {
        present(cx, "routine-remove-confirm-action")
    });
    assert_eq!(
        user.call("routine.list", json!({"project": "app"}))
            .unwrap()["routines"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "nothing is removed before the dialog is confirmed"
    );
    click(cx, "routine-remove-confirm-action");
    until(&shell, cx, "the routine is gone", |_| {
        user.call("routine.list", json!({"project": "app"}))
            .unwrap()["routines"]
            .as_array()
            .is_some_and(|all| all.is_empty())
    });
    until(&shell, cx, "the tab shows its empty state", |cx| {
        present(cx, "routines-empty")
    });
}

#[gpui_kit::test]
fn a_routine_notification_opens_the_result_page(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    let run = run_now(&user, "Tickets to review");
    assert_eq!(unread(&user, "Tickets to review"), 1);

    // The window is on no page of the project: the notification's row leads there.
    let (shell, cx) = window(&user, cx);
    until(
        &shell,
        cx,
        "the bell has the routine's notification",
        |cx| {
            shell.read_with(cx, |s, _| {
                s.snapshot().bell.iter().any(|n| n.kind == "routine")
            })
        },
    );
    let key = shell.read_with(cx, |s, _| {
        s.snapshot()
            .bell
            .iter()
            .find(|n| n.kind == "routine")
            .unwrap()
            .key
            .clone()
    });
    shell.update_in(cx, |s, window, cx| s.choose(&key, window, cx));
    until(&shell, cx, "the result page shows", |cx| {
        present(cx, "result-report")
    });

    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(flottille_window::shell::Selection::Project(
            "app".to_owned()
        )),
        "the project's page, not the session's"
    );
    assert!(!present(cx, "session-composer"), "no conversation");
    assert_eq!(
        label(cx, "result-summary").as_deref(),
        Some("4 to review"),
        "the run's result: run {}",
        run["id"]
    );
    until(&shell, cx, "the daemon knows it was opened", |_| {
        unread(&user, "Tickets to review") == 0
    });
    until(&shell, cx, "the notification is read", |cx| {
        shell.read_with(cx, |s, _| s.snapshot().bell.iter().all(|n| n.read))
    });
}

#[gpui_kit::test]
fn the_routines_tab_has_its_place_in_the_history(cx: &mut TestAppContext) {
    use flottille_window::history::Way;
    let user = user();
    add(&user, "Tickets to review", LIST_REVIEWS, json!({}));
    let (shell, cx) = open_tab(&user, cx);
    until(&shell, cx, "the routine's row shows", |cx| {
        present(cx, "routine-Tickets to review")
    });

    shell.update_in(cx, |s, window, cx| {
        s.open(flottille_window::shell::Selection::Settings, window, cx)
    });
    settle(cx);
    assert!(!present(cx, "routine-Tickets to review"));

    shell.update_in(cx, |s, window, cx| s.go(Way::Back, window, cx));
    until(&shell, cx, "the Routines tab is back", |cx| {
        present(cx, "routine-Tickets to review")
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(flottille_window::shell::Selection::Project(
            "app".to_owned()
        ))
    );

    // Back again is the page on the tab it was opened on; forward is the Routines tab.
    shell.update_in(cx, |s, window, cx| s.go(Way::Back, window, cx));
    settle(cx);
    assert!(!present(cx, "routine-Tickets to review"));
    shell.update_in(cx, |s, window, cx| s.go(Way::Forward, window, cx));
    until(&shell, cx, "the Routines tab is back", |cx| {
        present(cx, "routine-Tickets to review")
    });
}

// The list a result gives (#1124).

const LISTING: &str = "List the tickets with their sentences";
const TRACKER: &str = "https://tracker.example/ABC-12";

/// The fake agent ends its turn with `items`.
fn listing(user: &User, items: Value) {
    user.fake_script(json!({"turns": [
        {"when": LISTING, "do": [
            {"mcp": {"tool": "routine_result", "arguments":
                {"summary": "Tickets to review", "items": items}}}]},
    ]}));
}

fn item(id: &str, title: &str) -> Value {
    json!({"id": id, "title": title})
}

/// The window on the result page of the routine's last run.
fn open_result<'a>(
    user: &User,
    name: &str,
    run: &Value,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = open_routine(user, name, cx);
    let row = format!("routine-run-{}", run["id"]);
    until(&shell, cx, "the run is listed", |cx| present(cx, &row));
    click(cx, &row);
    until(&shell, cx, "the result page shows", |cx| {
        present(cx, "result-report")
    });
    (shell, cx)
}

#[gpui_kit::test]
fn a_routine_ends_with_items_shown_as_rows(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LISTING, json!({}));
    listing(
        &user,
        json!([
            {"id": "ABC-12", "title": "Login fails", "link": TRACKER,
             "text": "Reported twice this week.", "tags": ["blocks 3", "urgent"],
             "group": "Blocking"},
            {"id": "ABC-30", "title": "Slow start", "group": "Later"},
            {"id": "ABC-14", "title": "Typo in the footer", "group": "Blocking"},
        ]),
    );
    let run = run_now(&user, "Tickets to review");

    let (shell, cx) = open_routine(&user, "Tickets to review", cx);
    let opened: Arc<std::sync::Mutex<Vec<std::path::PathBuf>>> = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    let row = format!("routine-run-{}", run["id"]);
    until(&shell, cx, "the run is listed", |cx| present(cx, &row));
    click(cx, &row);
    until(&shell, cx, "the items show", |cx| {
        present(cx, "result-item-ABC-12")
    });
    settle(cx);

    assert_eq!(
        label(cx, "result-item-ABC-12").as_deref(),
        Some("Login fails"),
        "the title"
    );
    assert_eq!(
        label(cx, "result-item-ABC-12-line").as_deref(),
        Some("Reported twice this week."),
        "the sentence under it"
    );
    // The first run has nothing to compare with: every item is new, and the chips follow it.
    assert_eq!(
        label(cx, "result-item-ABC-12-tag-0").as_deref(),
        Some("New")
    );
    assert_eq!(
        label(cx, "result-item-ABC-12-tag-1").as_deref(),
        Some("blocks 3")
    );
    assert_eq!(
        label(cx, "result-item-ABC-12-tag-2").as_deref(),
        Some("urgent")
    );
    for group in ["Blocking", "Later"] {
        assert_eq!(
            label(cx, &format!("result-group-{group}-title")).as_deref(),
            Some(group),
            "a heading for {group}"
        );
    }
    assert!(present(cx, "result-item-ABC-14") && present(cx, "result-item-ABC-30"));
    // The link opens with the system's handler, as it is.
    click(cx, "result-item-ABC-12");
    assert_eq!(*opened.lock().unwrap(), [std::path::PathBuf::from(TRACKER)]);
    click(cx, "result-item-ABC-30");
    assert_eq!(opened.lock().unwrap().len(), 1, "no link, nothing opens");
}

#[gpui_kit::test]
fn items_new_since_the_last_run_are_marked(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LISTING, json!({}));
    listing(&user, json!([item("ABC-1", "One"), item("ABC-2", "Two")]));
    run_now(&user, "Tickets to review");
    listing(&user, json!([item("ABC-2", "Two"), item("ABC-3", "Three")]));
    let run = run_now(&user, "Tickets to review");

    let (shell, cx) = open_result(&user, "Tickets to review", &run, cx);
    until(&shell, cx, "the items show", |cx| {
        present(cx, "result-item-ABC-3")
    });
    settle(cx);

    assert_eq!(label(cx, "result-item-ABC-3-tag-0").as_deref(), Some("New"));
    assert!(present(cx, "result-item-ABC-2"));
    assert!(
        !present(cx, "result-item-ABC-2-tag-0"),
        "seen at the last run: not marked"
    );
}

#[gpui_kit::test]
fn items_that_left_are_said_once(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "Tickets to review", LISTING, json!({}));
    listing(
        &user,
        json!([item("ABC-377", "Old bug"), item("ABC-1", "One")]),
    );
    run_now(&user, "Tickets to review");
    listing(&user, json!([item("ABC-1", "One")]));
    let second = run_now(&user, "Tickets to review");
    listing(&user, json!([item("ABC-1", "One")]));
    let third = run_now(&user, "Tickets to review");

    let (shell, cx) = open_result(&user, "Tickets to review", &second, cx);
    until(&shell, cx, "the items show", |cx| {
        present(cx, "result-item-ABC-1")
    });
    settle(cx);
    assert_eq!(
        label(cx, "result-left").as_deref(),
        Some("Left since the last run: ABC-377"),
        "said at the foot"
    );

    // The run after it compares with this one alone: nothing is said again.
    click(cx, "routines-back-to-routine");
    let row = format!("routine-run-{}", third["id"]);
    until(&shell, cx, "the third run is listed", |cx| {
        present(cx, &row)
    });
    click(cx, &row);
    until(&shell, cx, "the items show", |cx| {
        present(cx, "result-item-ABC-1")
    });
    settle(cx);
    assert!(!present(cx, "result-left"), "said once");
}

#[gpui_kit::test]
fn the_form_chooses_to_ring_only_when_something_is_new(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = open_form(&user, cx);
    let form = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_page()
            .unwrap()
            .read(cx)
            .routines()
            .read(cx)
            .form()
            .clone()
    });
    cx.update(|window, cx| {
        form.update(cx, |form, cx| {
            form.name_field()
                .clone()
                .update(cx, |f, cx| f.set_value("Tickets to review", window, cx));
            form.prompt_field()
                .clone()
                .update(cx, |f, cx| f.set_value(LISTING, window, cx));
            form.hour_field()
                .clone()
                .update(cx, |f, cx| f.set_value("08:30", window, cx));
        })
    });
    settle(cx);
    click(cx, "routine-days");
    click(cx, "routine-days-weekdays");
    // The choice is below the fold of a form in a window this high: a taller one shows it.
    cx.simulate_resize(size(px(1280.), px(2600.)));
    settle(cx);
    click(cx, "routine-rings-choice-new");
    click(cx, "routine-save");
    until(&shell, cx, "the routine is kept", |_| {
        user.call("routine.list", json!({"project": "app"}))
            .unwrap()["routines"]
            .as_array()
            .is_some_and(|all| all.len() == 1)
    });
    let kept = user
        .call("routine.list", json!({"project": "app"}))
        .unwrap()["routines"][0]
        .clone();
    assert_eq!(kept["rings"], "new", "{kept}");
}

#[gpui_kit::test]
fn a_routine_made_by_an_agent_says_so(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "By hand", LIST_REVIEWS, json!({}));
    // A session of the user's, in Full, names itself and makes a routine with the tool.
    let opened = user.received_requests("session/new").len();
    let started = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = common::mcp::server_of(&user, opened);
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    let titled = mcp.call("session_title", json!({"title": "Plan the mornings"}));
    assert!(!titled.error, "{}", titled.text);
    let made = mcp.call(
        "routine",
        json!({"action": "create", "name": "Mornings", "prompt": LIST_REVIEWS,
            "schedule": "weekdays 08:30"}),
    );
    assert!(!made.error, "{}", made.text);

    let (shell, cx) = open_routine(&user, "Mornings", cx);
    until(&shell, cx, "its author shows", |cx| {
        present(cx, "routine-made-by")
    });
    assert_eq!(
        label(cx, "routine-made-by").as_deref(),
        Some("Made by Plan the mornings"),
        "the session's title"
    );
    click(cx, "routines-back");
    let row = "routine-By hand";
    until(&shell, cx, "the user's own is listed", |cx| {
        present(cx, row)
    });
    click(cx, row);
    until(&shell, cx, "its page shows", |cx| {
        present(cx, "routine-header")
    });
    settle(cx);
    assert!(
        !present(cx, "routine-made-by"),
        "the user's own says nothing"
    );
}
