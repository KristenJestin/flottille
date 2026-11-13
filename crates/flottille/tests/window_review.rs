//! Review in the window (`docs/extensions.md`, "Review"), on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent: a review's agents are one folded "Review" line
//! of the session's rail, with its state, unfolding to its pilot and passes. No window opens on a
//! screen.

mod common;
mod guard;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::Mcp;
use flottille_window::Shell;
use flottille_window::session::{RailRow, SessionPage};
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_design_system::space;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A user whose review's agents hang in their turn, so that the test acts as them, with a project
/// `app`, Review on, its workspace `w` holding a change, and a session in it: the session's id.
fn session(user: &User) -> String {
    session_scripted(
        user,
        review_script(json!(["fake-small", "fake-large"]), false),
    )
}

/// The fake agent's script of a review: agents that offer `models`, the pilot and its check
/// hanging in their turn, and each saying how much of its context it used when `used`.
fn review_script(models: Value, used: bool) -> Value {
    let hang = |tokens: u64| match used {
        true => json!([{"usage": {"used": tokens, "size": 200_000}}, {"hang": true}]),
        false => json!([{"hang": true}]),
    };
    let mut turns = vec![
        json!({"when": "of the change of workspace", "do": hang(90_000)}),
        json!({"when": "The change, in each repository", "do": hang(12_000)}),
    ];
    if used {
        turns.push(json!({"when": "build the login", "do": [
            {"usage": {"used": 150_000, "size": 200_000}}, {"say": "Built."}
        ]}));
    }
    json!({"models": models, "turns": turns})
}

/// [`session`] for a fake agent that follows `script`.
fn session_scripted(user: &User, script: Value) -> String {
    user.fake_script(script);
    session_with_script(user)
}

/// [`session`] for a user whose fake agent's script is already set.
fn session_with_script(user: &User) -> String {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();
    let workspace = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let work = std::path::PathBuf::from(workspace["folder"].as_str().unwrap());
    std::fs::write(work.join("login.rs"), "pub fn login() {}\n").unwrap();
    user.git(&work, &["add", "."]);
    user.git(&work, &["commit", "--quiet", "-m", "login"]);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "build the login"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the session's first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

/// The MCP session of the review's pilot.
fn pilot(user: &User) -> Mcp {
    let mut found = None;
    eventually("the pilot's session", || {
        found = user
            .received_requests("session/new")
            .into_iter()
            .rev()
            .find(|opened| {
                opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"][2]
                    .as_str()
                    .unwrap_or_default()
                    .contains("You are the pilot of a review")
            });
        found.is_some()
    });
    let opened: Value = found.unwrap();
    let server = &opened["params"]["mcpServers"][0];
    Mcp::initialize(
        server["url"].as_str().unwrap(),
        server["headers"][0]["value"].as_str().unwrap(),
    )
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

fn one_frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        one_frame(cx);
    }
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

/// Waits for the element to be drawn, then clicks it and lets the window settle: a click on an
/// element not drawn yet is lost, or panics.
fn click_when_drawn(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str) {
    until(shell, cx, &format!("{id} is drawn"), |cx| present(cx, id));
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

/// Waits until the element says `text`.
fn shows(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str, text: &str) {
    until(shell, cx, &format!("{id} says {text}"), |cx| {
        label(cx, id).as_deref() == Some(text)
    });
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// How far something of a kit component was drawn (0 to 1), `None` while it never was.
fn value(_: &mut VisualTestContext, id: &str) -> Option<f32> {
    gpui_design_system::seen::seen(id.to_owned())
}

/// The height the review's agents take in the rail once a frame is drawn; `None` when they are
/// not drawn at all.
fn behind(pilot: &str, cx: &mut VisualTestContext) -> Option<f32> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        let group = window.try_find(format!("rail-review-children-{pilot}"))?;
        Some(f32::from(group.bounds().size.height))
    })
}

fn page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

/// A review in a session of the window, its pilot having started the check `bugs`; the session's
/// page is open and the rail holds the review's row: the session's id, the pilot's, the check's.
fn review_rail<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    &'a mut VisualTestContext,
    String,
    String,
    String,
    Mcp,
) {
    review_rail_scripted(
        user,
        review_script(json!(["fake-small", "fake-large"]), false),
        cx,
    )
}

/// [`review_rail`] for agents that offer these models.
fn review_rail_of_models<'a>(
    user: &User,
    models: Value,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    &'a mut VisualTestContext,
    String,
    String,
    String,
    Mcp,
) {
    review_rail_scripted(user, review_script(models, false), cx)
}

/// [`review_rail`] for a fake agent that follows `script`.
fn review_rail_scripted<'a>(
    user: &User,
    script: Value,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    &'a mut VisualTestContext,
    String,
    String,
    String,
    Mcp,
) {
    let session = session_scripted(user, script);
    let started = user
        .call("review.start", json!({"agent": session}))
        .unwrap();
    let pilot_id = started["pilot"].as_str().unwrap().to_owned();
    let pilot = pilot(user);
    let ran = pilot.call("review", json!({"action": "run_pass", "pass": "bugs"}));
    assert!(!ran.error, "{}", ran.text);
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&session)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&session), cx));
    let row = format!("rail-row-{pilot_id}");
    until(&shell, cx, "the rail holds the review's row", |cx| {
        present(cx, &row)
    });
    settle(cx);
    let page = page(&shell, cx);
    let check = page
        .read_with(cx, |p, cx| {
            p.rail()
                .read(cx)
                .agents()
                .iter()
                .find(|a| a.parent.as_deref() == Some(pilot_id.as_str()))
                .map(|a| a.id.clone())
        })
        .expect("the check is in the rail");
    (shell, cx, session, pilot_id, check, pilot)
}

/// The id of the chevron at the end of the review's row.
fn chevron(pilot: &str) -> gpui_kit::ElementId {
    toggle_id(gpui_kit::ElementId::Name(
        format!("rail-row-{pilot}").into(),
    ))
}

#[gpui_kit::test]
fn a_review_is_one_row_that_says_its_checks(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx, session, pilot_id, check, _) = review_rail(&user, cx);

    // One row: the review's own, with no line above it that folds the agents under it.
    assert!(!present(cx, &format!("rail-review-{pilot_id}")));
    let page = page(&shell, cx);
    let rows = page.read_with(cx, |p, cx| p.rail().read(cx).rows());
    assert_eq!(rows[0], RailRow::Agent(session.clone()));
    assert_eq!(
        rows[1],
        RailRow::Review {
            pilot: pilot_id.clone(),
            count: 1,
            open: true
        }
    );
    assert_eq!(
        label(cx, &format!("rail-name-{pilot_id}")).as_deref(),
        Some("Review, round 1")
    );
    // What the separate line said is its second line, in the words of the window.
    let said = label(cx, &format!("rail-runs-on-{pilot_id}")).unwrap();
    assert_eq!(said, "0/2 checks");
    assert!(!said.contains("pass"), "{said}");
    // Unfolded while it runs: its check is under it.
    assert!(present(cx, &format!("rail-row-{check}")));
}

#[gpui_kit::test]
fn the_review_s_fold_is_on_the_right_and_holds_its_passes_and_findings(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx, session, pilot_id, check, pilot) = review_rail(&user, cx);
    let page = page(&shell, cx);

    // The chevron ends the row, after its title, as the sidebar's rows have it.
    let row = bounds(cx, &format!("rail-row-{pilot_id}"));
    let title = bounds(cx, &format!("rail-name-{pilot_id}"));
    let at = cx.update(|window, _| window.find(chevron(&pilot_id)).bounds());
    assert!(title.left() < at.left(), "{title:?} then {at:?}");
    assert!(row.right() - at.right() < px(8.), "{row:?} ends at {at:?}");

    // A click on it folds the check away, and does not open the review's conversation.
    cx.update(|window, cx| window.click(chevron(&pilot_id), cx));
    assert_eq!(page.read_with(cx, |p, _| p.shown().to_owned()), session);
    settle(cx);
    assert!(!present(cx, &format!("rail-row-{check}")));
    assert_eq!(behind(&pilot_id, cx), None, "folded, nothing is drawn");

    // A second one unfolds it, growing into place.
    cx.update(|window, cx| window.click(chevron(&pilot_id), cx));
    let first = behind(&pilot_id, cx).expect("drawn from the first frame");
    settle(cx);
    let open = behind(&pilot_id, cx).unwrap();
    assert!(first < open, "it grows: {first} then {open}");
    assert!(present(cx, &format!("rail-row-{check}")));

    // Its findings come after its checks, each with its status, which follows the user's word.
    let reported = pilot.call(
        "review",
        json!({"action": "report", "findings": [{"severity": "low", "confidence": "confirmed",
            "concerns": "code", "file": "login.rs", "lines": "1", "text": "no doc comment",
            "case": "read it"}]}),
    );
    assert!(!reported.error, "{}", reported.text);
    let finding = format!("rail-finding-row-{pilot_id}-F1");
    until(&shell, cx, "the review ended", |cx| {
        label(cx, &format!("rail-runs-on-{pilot_id}"))
            .is_some_and(|said| said.ends_with("1 finding"))
    });
    settle(cx);
    // Folded once done, whatever was chosen while it ran: unfolded by hand again.
    assert_eq!(behind(&pilot_id, cx), None);
    cx.update(|window, cx| window.click(chevron(&pilot_id), cx));
    settle(cx);
    assert!(label(cx, &finding).is_some_and(|said| said.contains("no doc comment")));
    assert!(label(cx, &finding).unwrap().contains("· open:"));
    let after = bounds(cx, &finding);
    let checks = bounds(cx, &format!("rail-row-{check}"));
    assert!(checks.bottom() <= after.top(), "{checks:?} then {after:?}");
    user.call(
        "review.finding",
        json!({"session": session, "finding": "F1", "status": "dismissed"}),
    )
    .unwrap();
    until(&shell, cx, "the finding says it is dismissed", |cx| {
        label(cx, &finding).is_some_and(|said| said.contains("· dismissed:"))
    });
    at_rest(cx);
}

#[gpui_kit::test]
fn a_done_review_is_folded_and_says_its_findings(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx, _, pilot_id, check, pilot) = review_rail(&user, cx);
    assert!(behind(&pilot_id, cx).is_some(), "unfolded while it runs");

    let reported = pilot.call(
        "review",
        json!({"action": "report", "findings": [{"severity": "low", "confidence": "confirmed",
            "concerns": "code", "file": "login.rs", "lines": "1", "text": "no doc comment",
            "case": "read it"}]}),
    );
    assert!(!reported.error, "{}", reported.text);
    let said = format!("rail-runs-on-{pilot_id}");
    until(&shell, cx, "the row says the findings", |cx| {
        label(cx, &said).is_some_and(|said| said.ends_with("· 1 finding"))
    });
    // It folds away over a moment rather than at once, and then nothing under it is drawn.
    assert!(behind(&pilot_id, cx).is_some(), "folding, not gone");
    settle(cx);
    assert_eq!(behind(&pilot_id, cx), None);
    assert!(!present(cx, &format!("rail-row-{check}")));
    assert!(!present(cx, &format!("rail-finding-row-{pilot_id}-F1")));
    assert_eq!(label(cx, &said).as_deref(), Some("1/2 checks · 1 finding"));
    at_rest(cx);

    // Unfolded by hand, its findings are under it.
    cx.update(|window, cx| window.click(chevron(&pilot_id), cx));
    settle(cx);
    assert!(present(cx, &format!("rail-finding-row-{pilot_id}-F1")));
}

#[gpui_kit::test]
fn a_rail_row_says_its_state_whole_when_there_is_room(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx, _, pilot_id, check, _) = review_rail(&user, cx);

    for agent in [&pilot_id, &check] {
        let row = bounds(cx, &format!("rail-row-{agent}"));
        let said = bounds(cx, &format!("rail-runs-on-{agent}"));
        // Nothing of the row's width is kept from it but what its own end holds (the chevron or
        // the menu's place, 28 px, and the padding).
        assert!(
            row.right() - said.right() < px(44.),
            "{agent}: the second line ends at {:?}, its row at {:?}",
            said.right(),
            row.right()
        );
        // The text's own measure: the kit shapes it in its font and draws a tooltip under the
        // pointer only when it is wider than its line.
        let line = format!("rail-runs-on-{agent}");
        cx.update(|window, cx| window.hover(line.clone(), cx));
        settle(cx);
        assert!(
            !present(cx, &format!("{line}-tooltip")),
            "{agent}: a whole text has no tooltip"
        );
    }
    // The rail with a review running keeps every text inside it.
    guard::assert_stays_inside(&shell, cx, "rail-agents");
}

#[gpui_kit::test]
fn a_long_state_stays_inside_a_narrow_rail(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    // The check runs on a model that is one long word, which its state says.
    let (shell, cx, _, _, check, _) = review_rail_of_models(&user, json!([guard::UNBROKEN]), cx);
    // The rail is 290 px wide at every width (a drawer in a narrow window), so this is its
    // narrowest: the long word has no room to hide in.
    until(&shell, cx, "the rail says the long model", |cx| {
        label(cx, &format!("rail-runs-on-{check}"))
            .is_some_and(|said| said.contains(guard::UNBROKEN))
    });
    settle(cx);
    guard::assert_stays_inside(&shell, cx, "rail-agents");

    // The state that is cut says itself whole under the pointer.
    let long = format!("rail-runs-on-{check}");
    cx.update(|window, cx| window.hover(long.clone(), cx));
    settle(cx);
    assert_eq!(
        label(cx, &format!("{long}-tooltip")).as_deref(),
        label(cx, &long).as_deref()
    );
    assert!(label(cx, &long).unwrap().starts_with(guard::UNBROKEN));
}

#[gpui_kit::test]
fn the_gauges_of_the_rows_line_up(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx, session, pilot_id, check, _) = review_rail_scripted(
        &user,
        review_script(json!(["fake-small", "fake-large"]), true),
        cx,
    );
    let gauges: Vec<String> = [&session, &pilot_id, &check]
        .iter()
        .map(|agent| format!("rail-context-{agent}"))
        .collect();
    until(&shell, cx, "every row says its context", |cx| {
        settle(cx);
        gauges.iter().all(|gauge| present(cx, gauge))
    });
    settle(cx);
    // A row that is a level deeper, a shorter share or a longer name moves none of them.
    let first = bounds(cx, &gauges[0]);
    for gauge in &gauges {
        let at = bounds(cx, gauge);
        assert_eq!(
            at.left(),
            first.left(),
            "{gauge} starts as the first: {at:?}"
        );
        assert_eq!(
            at.right(),
            first.right(),
            "{gauge} ends as the first: {at:?}"
        );
    }
    let shares: Vec<Option<String>> = gauges.iter().map(|gauge| label(cx, gauge)).collect();
    assert_eq!(
        shares
            .iter()
            .flatten()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3,
        "{shares:?}"
    );
}

#[gpui_kit::test]
fn the_tree_says_checks_not_passes(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (_, cx, _, pilot_id, _, _) = review_rail(&user, cx);

    let said = label(cx, &format!("rail-runs-on-{pilot_id}")).unwrap();
    assert_eq!(said, "0/2 checks");
    assert!(!said.contains("pass"), "{said}");
}

#[gpui_kit::test]
fn a_review_s_conversation_shows_its_steps_and_what_it_waits_for(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    // The pilot reads, starts a check and waits for it, as the real one does.
    user.fake_script(json!({"turns": [
        {"when": "of the change of workspace", "do": [
            {"mcp": {"tool": "review", "arguments": {"action": "run_pass", "pass": "bugs"}}},
            {"hang": true}
        ]},
        {"when": "The change, in each repository", "do": [{"hang": true}]}
    ]}));
    let session = session_with_script(&user);
    let started = user
        .call("review.start", json!({"agent": session}))
        .unwrap();
    let pilot_id = started["pilot"].as_str().unwrap().to_owned();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&session)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&session), cx));
    let row = format!("rail-row-{pilot_id}");
    until(&shell, cx, "the rail holds the review's row", |cx| {
        present(cx, &row)
    });
    settle(cx);

    // Its row opens its conversation.
    cx.update(|window, cx| window.click(row, cx));
    let page = page(&shell, cx);
    until(&shell, cx, "the review's steps", |cx| {
        page.read_with(cx, |p, cx| {
            p.shown() == pilot_id && !p.conversation().read(cx).blocks().is_empty()
        })
    });
    settle(cx);
    let said: Vec<String> = cx.update(|window, _| {
        window
            .within("session-page".to_owned())
            .elements()
            .into_iter()
            .filter_map(|element| element.label().map(str::to_owned))
            .collect()
    });
    assert!(
        !said.iter().any(|said| said == "Nothing said yet."),
        "an agent that made calls never says nothing was said: {said:?}"
    );
    until(&shell, cx, "the line that says what it waits for", |cx| {
        present(cx, "session-waiting-checks")
    });
    assert_eq!(
        label(cx, "session-waiting-checks").as_deref(),
        Some("Waiting for 1 check: bugs")
    );
}

#[gpui_kit::test]
fn the_waiting_line_fades_its_new_count_in(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx, _, pilot_id, _, pilot) = review_rail(&user, cx);
    cx.update(|window, cx| window.click(format!("rail-row-{pilot_id}"), cx));
    until(&shell, cx, "the line that says what it waits for", |cx| {
        label(cx, "session-waiting-checks").is_some()
    });
    assert_eq!(
        label(cx, "session-waiting-checks").as_deref(),
        Some("Waiting for 1 check: bugs")
    );
    // Folded, the rail has nothing of its own to move when a second check starts.
    cx.update(|window, cx| window.click(chevron(&pilot_id), cx));
    at_rest(cx);

    let ran = pilot.call("review", json!({"action": "run_pass", "pass": "standards"}));
    assert!(!ran.error, "{}", ran.text);
    until(&shell, cx, "the count changes", |cx| {
        label(cx, "session-waiting-checks").is_some_and(|said| said.starts_with("Waiting for 2"))
    });
    // The words go through the kit's `changing`, which fades them in over HOVER (140 ms) from the
    // last frame's: GPUI shows no opacity, so the test sees its element and the frames it asks
    // for, while it fades and not once it is done.
    assert!(present(cx, "session-waiting-checks-said"), "the line fades");
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "fading"
    );
    for _ in 0..6 {
        cx.executor().advance_clock(Duration::from_millis(16));
        assert!(
            cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
            "still fading"
        );
    }
    cx.executor().advance_clock(Duration::from_millis(80));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    at_rest(cx);
}

fn at_rest(cx: &mut VisualTestContext) {
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

#[gpui_kit::test]
fn slash_review_in_the_composer_starts_a_review_and_sends_nothing(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let session = session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&session)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&session), cx));
    until(&shell, cx, "the session's page", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    settle(cx);

    // The composer offers it among Flottille's own commands, unfolding into its list.
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("/rev");
    until(&shell, cx, "the list offers it", |cx| {
        cx.update(|window, _| window.try_find("slash-review").is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click("slash-review", cx));

    eventually("the review", || {
        user.call("review.list", json!({"session": session}))
            .unwrap()["reviews"]
            .as_array()
            .is_some_and(|all| all.len() == 1)
    });
    let page = page(&shell, cx);
    let reply = page.read_with(cx, |page, _| page.reply().cloned()).unwrap();
    assert_eq!(
        reply.read_with(cx, |r, cx| r.field().read(cx).value().to_string()),
        "",
        "the command is done, not sent"
    );
    let prompts: Vec<Value> = user.received_requests("session/prompt");
    assert!(
        !prompts.iter().any(|p| p.to_string().contains("/review")),
        "the agent read it"
    );
}

fn review_listed(user: &User) -> Value {
    user.call("extension.list", json!({})).unwrap()["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "review")
        .cloned()
        .unwrap()
}

/// One frame, 16 ms on: how many elements it drew that move.
fn step(cx: &mut VisualTestContext) -> usize {
    cx.executor().advance_clock(Duration::from_millis(16));
    let drawn = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    drawn
}

/// The window on Review's page of Settings, Review switched as `on` says, in a window tall
/// enough for the whole page.
fn review_page<'a>(
    user: &User,
    on: bool,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    if on {
        user.call("extension.set", json!({"name": "review", "enabled": true}))
            .unwrap();
    }
    let (shell, cx) = window(user, cx);
    cx.simulate_resize(size(px(1280.), px(3200.)));
    shell.update_in(cx, |s, window, cx| s.open(Selection::Settings, window, cx));
    until(&shell, cx, "the features tab", |cx| {
        present(cx, "settings-tabs-features")
    });
    cx.update(|window, cx| window.click("settings-tabs-features", cx));
    until(&shell, cx, "the Review row is offered", |cx| {
        present(cx, "features-review")
    });
    settle(cx);
    cx.update(|window, cx| window.click("features-review", cx));
    until(&shell, cx, "Review's page", |cx| {
        present(cx, "review-switch")
    });
    if on {
        until(&shell, cx, "the checks are shown", |cx| {
            present(cx, "review-checks")
        });
    }
    settle(cx);
    (shell, cx)
}

fn bounds(cx: &mut VisualTestContext, id: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update(|window, _| window.find(id.to_owned()).bounds())
}

/// The checks of the card, in their order: the key of each, its name and its line.
const CHECKS: [(&str, &str, &str); 4] = [
    (
        "spec",
        "The spec",
        "Each expected behaviour met, nothing out of scope. Only for a session that builds a spec.",
    ),
    (
        "bugs",
        "Bugs",
        "Wrong results, lost data, crashes the change brings.",
    ),
    (
        "standards",
        "Project rules",
        "The project's AGENTS.md, read from the base branch.",
    ),
    (
        "security",
        "Security",
        "Secrets, injections, access. Off by default.",
    ),
];

#[gpui_kit::test]
fn the_usage_line_shows_while_review_is_off(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, false, cx);

    assert_eq!(
        label(cx, "review-switch-label").as_deref(),
        Some("Review changes")
    );
    assert_eq!(
        label(cx, "review-switch-hint").as_deref(),
        Some(
            "Agents read a session's change with fresh eyes and send it what is wrong. Nothing is blocked."
        )
    );
    let usage = label(cx, "review-usage").unwrap();
    assert!(usage.contains("/review"), "{usage}");
    assert!(usage.contains("flottille review <session>"), "{usage}");
    // Both switches are off by default: the usage says the user's doors alone.
    assert!(!usage.contains("pull request"), "{usage}");
    assert!(!usage.contains("agent"), "{usage}");
    // Off, the page is that block alone.
    assert!(!present(cx, "review-checks"));
    assert!(!present(cx, "review-guidance-text"));

    // On, the rest unfolds under it, and the usage line stays.
    cx.update(|window, cx| window.click("review-switch", cx));
    until(&shell, cx, "the daemon has it on", |_| {
        review_listed(&user)["enabled"] == true
    });
    until(&shell, cx, "the checks unfold", |cx| {
        present(cx, "review-checks")
    });
    assert!(step(cx) + step(cx) > 0, "they unfold");
    settle(cx);
    assert!(present(cx, "review-usage"));
    at_rest(cx);
}

#[gpui_kit::test]
fn review_s_page_shows_what_it_checks_as_one_card(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, true, cx);

    // A line per check, inside the one card: its switch, its name, its line, its model on the
    // right.
    let card = bounds(cx, "review-checks");
    let mut above = card.top();
    for (key, name, line) in CHECKS {
        let switch = format!("review-check-{key}-switch");
        assert_eq!(label(cx, &format!("{switch}-label")).as_deref(), Some(name));
        assert_eq!(label(cx, &format!("{switch}-hint")).as_deref(), Some(line));
        let (words, select) = (
            bounds(cx, &format!("{switch}-label")),
            bounds(cx, &format!("review-setting-{key}_model-model")),
        );
        assert!(
            card.top() <= words.top() && select.bottom() <= card.bottom(),
            "{key} in the card"
        );
        assert!(words.top() >= above, "{key} comes after the line above");
        assert!(
            select.left() > words.right(),
            "{key}: the model on the right"
        );
        assert!(select.size.width < card.size.width / 2., "{key}: compact");
        above = words.bottom();
    }
    // The last line checks the findings, with no switch.
    assert_eq!(
        label(cx, "review-setting-verifier_model-label").as_deref(),
        Some("Checking the findings")
    );
    assert!(bounds(cx, "review-setting-verifier_model-label").top() >= above);
    assert!(!present(cx, "review-check-verifier-switch"));
    // Spec, bugs and project rules by default; Security off.
    for (key, on) in [
        ("spec", true),
        ("bugs", true),
        ("standards", true),
        ("security", false),
    ] {
        let knob = bounds(cx, &format!("review-check-{key}-switch-knob"));
        let track = bounds(cx, &format!("review-check-{key}-switch-track"));
        let right = knob.left() - track.left() > track.size.width / 3.;
        assert_eq!(right, on, "{key}");
    }

    // A check switched on is the daemon's, the keys as they were; its switch slides.
    cx.update(|window, cx| window.click("review-check-security-switch", cx));
    until(&shell, cx, "the checks are kept", |_| {
        review_listed(&user)["settings"][0]["value"]
            == json!(["spec", "bugs", "standards", "security"])
    });
    assert!(step(cx) > 0, "the switch moves");
    settle(cx);

    // The guidance every reviewer is told, under the card.
    assert_eq!(
        label(cx, "review-guidance-title").as_deref(),
        Some("What every reviewer is told")
    );
    let sentence = label(cx, "review-guidance-sentence").unwrap();
    assert!(
        sentence.contains("A check's own instructions come after it."),
        "{sentence}"
    );
    assert!(bounds(cx, "review-guidance-title").top() > card.bottom());
    // A block of the page like the others: `space::XXL` under the card of the switch.
    assert_eq!(
        bounds(cx, "review-guidance-block").top() - bounds(cx, "review").bottom(),
        space::XXL
    );
    assert!(present(cx, "review-guidance-text"), "the shared editor");
    assert!(!present(cx, "review-guidance-save"), "no Save at rest");
    // No word of how it is built.
    for id in [
        "review-switch-hint",
        "review-usage",
        "review-guidance-sentence",
    ] {
        let said = label(cx, id).unwrap().to_lowercase();
        for word in ["pilot", "pass", "verifier"] {
            assert!(!said.contains(word), "{id} says {word}: {said}");
        }
    }

    // A project that does otherwise is named under it, fading in.
    let folder = user.path("work/acme-shop");
    user.git_init(&folder);
    user.call(
        "project.add",
        json!({"folder": folder, "name": "acme-shop"}),
    )
    .unwrap();
    user.call(
        "extension.set",
        json!({"name": "review", "settings": {"passes": ["spec", "bugs", "standards"]}}),
    )
    .unwrap();
    user.call(
        "extension.set",
        json!({"name": "review", "project": "acme-shop",
            "settings": {"passes": ["spec", "bugs", "standards", "security"]}}),
    )
    .unwrap();
    until(&shell, cx, "the project is named", |cx| {
        one_frame(cx);
        label(cx, "review-projects").is_some_and(|said| said == "acme-shop runs Security too")
    });
    settle(cx);
    assert!(present(cx, "review-projects-acme-shop"), "a link");
    at_rest(cx);
}

/// The two switches of when a review starts: the key, its label and its help line.
const DOORS: [(&str, &str, &str); 2] = [
    (
        "auto_on_pull_request",
        "Start a review when a session's pull request opens",
        "Off, a review starts only when you ask for one.",
    ),
    (
        "agents_start",
        "Let agents start a review",
        "Off, an agent cannot ask for a review of its own change: only you can.",
    ),
];

/// Whether a switch's knob is on its right: the switch is on.
fn is_on(cx: &mut VisualTestContext, id: &str) -> bool {
    let knob = bounds(cx, &format!("{id}-knob"));
    let track = bounds(cx, &format!("{id}-track"));
    knob.left() - track.left() > track.size.width / 3.
}

fn door_value(user: &User, key: &str) -> Value {
    review_listed(user)["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["key"] == key)
        .unwrap()["value"]
        .clone()
}

#[gpui_kit::test]
fn review_s_page_has_both_switches_off_by_default(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, true, cx);

    // Each with its visible label and its help line, off.
    for (key, name, line) in DOORS {
        let switch = format!("review-door-{key}-switch");
        assert_eq!(label(cx, &format!("{switch}-label")).as_deref(), Some(name));
        assert_eq!(label(cx, &format!("{switch}-hint")).as_deref(), Some(line));
        assert!(!is_on(cx, &switch), "{key} is off by default");
        assert_eq!(door_value(&user, key), false);
    }
    // They come before what a review checks.
    assert!(
        bounds(cx, "review-door-agents_start-switch").bottom()
            <= bounds(cx, "review-check-spec-switch").top()
    );
    let usage = label(cx, "review-usage").unwrap();
    assert!(
        !usage.contains("pull request") && !usage.contains("agent"),
        "{usage}"
    );

    // Turned on, a switch is the daemon's and slides; the usage line then says what it adds.
    cx.update(|window, cx| window.click("review-door-auto_on_pull_request-switch", cx));
    until(&shell, cx, "the daemon has it on", |_| {
        door_value(&user, "auto_on_pull_request") == true
    });
    // The window reads the list again a moment after the event; the switch then slides, and the
    // usage line says what it adds.
    let moved = std::cell::Cell::new(0);
    until(&shell, cx, "the usage says the pull request", |cx| {
        moved.set(moved.get() + step(cx));
        label(cx, "review-usage").is_some_and(|usage| usage.contains("pull request opens"))
    });
    settle(cx);
    assert!(moved.get() > 0, "the switch moves");
    assert!(is_on(cx, "review-door-auto_on_pull_request-switch"));
    assert!(!is_on(cx, "review-door-agents_start-switch"));
    cx.update(|window, cx| window.click("review-door-agents_start-switch", cx));
    until(&shell, cx, "the daemon has agents on", |_| {
        door_value(&user, "agents_start") == true
    });
    until(&shell, cx, "the usage says the agents", |cx| {
        label(cx, "review-usage").is_some_and(|usage| usage.contains("agent"))
    });
    let usage = label(cx, "review-usage").unwrap();
    assert!(usage.contains("/review") && usage.contains("flottille review <session>"));
    at_rest(cx);
}

#[gpui_kit::test]
fn a_project_s_two_switches_carry_the_dot_and_go_back_to_settings(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = project_page(&user, cx);

    for (key, name, line) in DOORS {
        let switch = format!("project-review-door-{key}-switch");
        assert_eq!(label(cx, &format!("{switch}-label")).as_deref(), Some(name));
        assert_eq!(label(cx, &format!("{switch}-hint")).as_deref(), Some(line));
        assert!(!is_on(cx, &switch), "{key}");
        assert!(!present(cx, &format!("project-review-door-{key}-set")));
    }
    cx.update(|window, cx| window.click("project-review-door-agents_start-switch", cx));
    until(&shell, cx, "the project lets agents start", |_| {
        review_listed(&user)["projects"][0]["settings"]["agents_start"] == true
    });
    until(&shell, cx, "the dot is shown", |cx| {
        settle(cx);
        present(cx, "project-review-door-agents_start-set")
    });
    assert!(!present(cx, "project-review-door-auto_on_pull_request-set"));
    assert_eq!(door_value(&user, "agents_start"), false, "Settings' own");
    cx.update(|window, cx| window.click("project-review-door-agents_start-reset", cx));
    until(
        &shell,
        cx,
        "the project goes back to Settings' value",
        |_| {
            review_listed(&user)["projects"][0]["settings"]
                .get("agents_start")
                .is_none_or(Value::is_null)
        },
    );
    until(&shell, cx, "the dot goes", |cx| {
        settle(cx);
        !present(cx, "project-review-door-agents_start-set")
    });
    at_rest(cx);
}

/// A user whose provider offers `opus` and `sonnet`, the daemon having listed them, with Review on.
fn user_with_models() -> User {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.fake_script(json!({"models": ["opus", "sonnet"], "efforts": ["low", "high"]}));
    user.call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();
    eventually("the models the daemon picks", || {
        review_listed(&user)["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "bugs_model" && s["default"]["gives"]["claude"] == "opus")
    });
    user
}

/// What a check's model slot holds, `null` while unset.
fn slot(user: &User, key: &str) -> Value {
    let listed = review_listed(user);
    let setting = listed["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap();
    if setting["set"] == true {
        setting["value"].clone()
    } else {
        Value::Null
    }
}

#[gpui_kit::test]
fn a_check_s_select_opens_on_the_recommended_model_selected(cx: &mut TestAppContext) {
    let user = user_with_models();
    let (shell, cx) = review_page(&user, true, cx);

    // Each check's select opens on the model it runs on, chosen as if the user had, marked
    // recommended; never a row named Default.
    for (key, _, _) in CHECKS {
        shows(
            &shell,
            cx,
            &format!("review-setting-{key}_model-model"),
            "Claude · opus · recommended for Claude sessions",
        );
    }
    // The last check runs one size smaller: its recommended model is that one.
    shows(
        &shell,
        cx,
        "review-setting-verifier_model-model",
        "Claude · sonnet · recommended for Claude sessions",
    );
    let card = bounds(cx, "review-checks");
    let models = label(cx, "review-checks-models").unwrap();
    assert_eq!(
        models,
        "Models: the strongest of the session's provider unless you choose one. Each check is a \
         turn on your own quota."
    );
    assert_eq!(
        label(cx, "review-checks-rounds").as_deref(),
        Some("After fixes, one more review of what changed; two at most.")
    );
    assert!(bounds(cx, "review-checks-models").top() >= card.bottom());
    assert!(
        bounds(cx, "review-checks-rounds").top() >= bounds(cx, "review-checks-models").bottom()
    );
    // Opened, no row is the rule: the models, the recommended one marked.
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
    shows(
        &shell,
        cx,
        "review-setting-bugs_model-model-claude-sonnet",
        "Claude · sonnet",
    );
    assert!(!present(cx, "review-setting-bugs_model-model-first"));
    assert_eq!(
        slot(&user, "bugs_model"),
        Value::Null,
        "nothing was chosen by looking"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn choosing_the_recommended_model_back_clears_the_choice(cx: &mut TestAppContext) {
    let user = user_with_models();
    let (shell, cx) = review_page(&user, true, cx);

    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model-claude-sonnet");
    until(&shell, cx, "the choice is stored", |cx| {
        slot(&user, "bugs_model") == json!({"provider": "claude", "model": "sonnet"})
            && label(cx, "review-setting-bugs_model-model").as_deref() == Some("Claude · sonnet")
    });
    settle(cx);

    // Back on the recommended one: it keeps following the rule, nothing is stored.
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model-claude-opus");
    until(&shell, cx, "the choice is cleared", |cx| {
        slot(&user, "bugs_model") == Value::Null
            && label(cx, "review-setting-bugs_model-model").as_deref()
                == Some("Claude · opus · recommended for Claude sessions")
    });
    settle(cx);
    at_rest(cx);
}

#[gpui_kit::test]
fn a_review_check_chooses_its_effort(cx: &mut TestAppContext) {
    let user = user_with_models();
    let (shell, cx) = review_page(&user, true, cx);

    // The effort select is beside the check's model, as the compactor's is: the model's
    // recommended effort selected and marked.
    // The model proposes `low`: choosing `high` stores it.
    shows(
        &shell,
        cx,
        "review-setting-bugs_model-effort",
        "low · recommended",
    );
    click_when_drawn(&shell, cx, "review-setting-bugs_model-effort");
    click_when_drawn(&shell, cx, "review-setting-bugs_model-effort-high");
    until(&shell, cx, "the effort is stored and shown", |cx| {
        slot(&user, "bugs_model")
            == json!({"provider": "claude", "model": "opus", "effort": "high"})
            && label(cx, "review-setting-bugs_model-effort").as_deref() == Some("high")
    });
    settle(cx);
    // The other checks are untouched.
    assert_eq!(slot(&user, "spec_model"), Value::Null);
    at_rest(cx);
}

/// A user with two providers whose models the daemon listed, Claude first, with Review on.
fn user_with_two_providers() -> User {
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.provider_script(
        "claude",
        json!({"models": ["opus", "sonnet"], "efforts": ["low", "high"]}),
    );
    user.provider_script(
        "opencode",
        json!({"models": ["open/small", "open/medium"], "efforts": ["low", "high"]}),
    );
    user.call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();
    eventually("the models the daemon picks", || {
        review_listed(&user)["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "bugs_model" && s["default"]["gives"]["claude"] == "opus")
    });
    user
}

#[gpui_kit::test]
fn a_review_slot_with_no_choice_opens_on_the_first_provider_s_strongest(cx: &mut TestAppContext) {
    let user = user_with_two_providers();
    let (shell, cx) = review_page(&user, true, cx);
    // The select is opened once, and the window left to learn OpenCode's models while it is.
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
    until(&shell, cx, "OpenCode's models are shown", |cx| {
        present(cx, "review-setting-bugs_model-model-opencode-open-small")
    });
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
    // Each provider's strongest is marked for its own sessions; nothing chosen, the first's is
    // selected.
    shows(
        &shell,
        cx,
        "review-setting-bugs_model-model",
        "Claude · opus · recommended for Claude sessions",
    );
    shows(
        &shell,
        cx,
        "review-setting-verifier_model-model",
        "Claude · sonnet · recommended for Claude sessions",
    );
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
    shows(
        &shell,
        cx,
        "review-setting-bugs_model-model-opencode-open-small",
        "OpenCode · open/small · recommended for OpenCode sessions",
    );
    shows(
        &shell,
        cx,
        "review-setting-bugs_model-model-claude-sonnet",
        "Claude · sonnet",
    );
    assert_eq!(slot(&user, "bugs_model"), Value::Null);
    at_rest(cx);
}

#[gpui_kit::test]
fn choosing_any_provider_s_recommended_model_clears_a_review_slot(cx: &mut TestAppContext) {
    let user = user_with_two_providers();
    let (shell, cx) = review_page(&user, true, cx);
    let pick = |key: &str, cx: &mut VisualTestContext| {
        click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
        click_when_drawn(
            &shell,
            cx,
            &format!("review-setting-bugs_model-model-{key}"),
        );
    };
    // The select is opened once, and the window left to learn OpenCode's models while it is.
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");
    until(&shell, cx, "OpenCode's models are shown", |cx| {
        present(cx, "review-setting-bugs_model-model-opencode-open-small")
    });
    click_when_drawn(&shell, cx, "review-setting-bugs_model-model");

    // Pinned: a model that is not recommended.
    pick("opencode-open-medium", cx);
    until(&shell, cx, "the slot is pinned", |_| {
        slot(&user, "bugs_model") == json!({"provider": "opencode", "model": "open/medium"})
    });
    settle(cx);
    // OpenCode's recommended model, not the first provider's: the slot follows the rule again.
    pick("opencode-open-small", cx);
    until(&shell, cx, "the slot is cleared", |cx| {
        slot(&user, "bugs_model") == Value::Null
            && label(cx, "review-setting-bugs_model-model").as_deref()
                == Some("Claude · opus · recommended for Claude sessions")
    });
    settle(cx);
    at_rest(cx);
}

#[gpui_kit::test]
fn instructions_for_this_check_fold_under_each_check(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, true, cx);

    // Folded under each check, saying there are none; none under the last line.
    for (key, _, _) in CHECKS {
        let fold = format!("review-check-{key}-instructions");
        assert_eq!(
            label(cx, &fold).as_deref(),
            Some("Instructions for this check"),
            "{key}"
        );
        until(&shell, cx, "the instructions are read", |cx| {
            label(cx, &format!("{fold}-summary")).as_deref() == Some("none")
        });
        assert!(
            bounds(cx, &fold).top() > bounds(cx, &format!("review-check-{key}-switch")).bottom(),
            "{key}: under its line"
        );
        assert!(!present(cx, &format!("review-{key}-guidance-text")));
    }
    assert!(!present(cx, "review-check-verifier-instructions"));

    // A click unfolds an editor, its chevron turning; Save unfolds once it changed.
    cx.update(|window, cx| window.click("review-check-bugs-instructions", cx));
    one_frame(cx);
    assert!(present(cx, "review-bugs-guidance-text"), "drawn at once");
    let first = bounds(cx, "review-check-bugs-instructions-body")
        .size
        .height;
    settle(cx);
    let open = bounds(cx, "review-check-bugs-instructions-body")
        .size
        .height;
    assert!(first < open, "it unfolds: {first} then {open}");
    assert!(!present(cx, "review-bugs-guidance-save"), "no Save at rest");
    let editor = shell
        .read_with(cx, |s, cx| {
            s.extensions()
                .read(cx)
                .check_guidance("review", "bugs")
                .cloned()
        })
        .unwrap();
    let field = editor.read_with(cx, |e, _| e.field().cloned()).unwrap();
    field.update_in(cx, |field, window, cx| {
        field.set_value("Look at the retries of the login first.", window, cx)
    });
    assert!(step(cx) > 0, "Save unfolds");
    settle(cx);
    cx.update(|window, cx| window.click("review-bugs-guidance-save", cx));
    // The window's editor takes the daemon's answer on a later poll than the write: the fold's
    // summary reads it from there, so it is the editor that is waited for, not the daemon.
    until(&shell, cx, "the check's instructions are kept", |cx| {
        editor.read_with(cx, |e, _| {
            e.shown()
                .is_some_and(|shown| shown.text == "Look at the retries of the login first.")
        })
    });
    assert_eq!(
        user.call("review.show", json!({"part": "bugs"})).unwrap()["text"],
        "Look at the retries of the login first."
    );
    assert_eq!(
        user.call("review.show", json!({})).unwrap()["source"],
        "default",
        "the shared guidance is untouched"
    );

    // Folded again, it shows the start of its text.
    cx.update(|window, cx| window.click("review-check-bugs-instructions", cx));
    settle(cx);
    assert!(!present(cx, "review-bugs-guidance-text"));
    let preview = label(cx, "review-check-bugs-instructions-summary").unwrap();
    assert!(preview.starts_with("Look at the retries"), "{preview}");
    at_rest(cx);
}

#[gpui_kit::test]
fn a_project_s_review_page_has_the_same_card(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(3200.)));
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at(
            "app",
            flottille_window::project_settings::Reveal::Feature("review".into()),
            window,
            cx,
        )
    });
    until(&shell, cx, "the project's page of Review", |cx| {
        present(cx, "project-review-checks")
    });
    settle(cx);

    for (key, name, _) in CHECKS {
        assert_eq!(
            label(cx, &format!("project-review-check-{key}-switch-label")).as_deref(),
            Some(name)
        );
        assert!(!present(cx, &format!("project-review-check-{key}-set")));
    }
    // Instructions are Settings' alone.
    assert!(!present(cx, "project-review-check-bugs-instructions"));
    cx.update(|window, cx| window.click("project-review-setting-bugs_model-model", cx));
    settle(cx);
    assert_eq!(
        label(cx, "project-review-setting-bugs_model-model-first").as_deref(),
        Some("As in Settings")
    );
    cx.update(|window, cx| window.click("project-review-setting-bugs_model-model", cx));
    settle(cx);

    // A check turned on here carries the dot, and goes back to Settings' value.
    cx.update(|window, cx| window.click("project-review-check-security-switch", cx));
    until(&shell, cx, "the project runs Security", |_| {
        review_listed(&user)["projects"][0]["settings"]["passes"]
            == json!(["spec", "bugs", "standards", "security"])
    });
    until(&shell, cx, "the dot is shown", |cx| {
        settle(cx);
        present(cx, "project-review-check-security-set")
    });
    assert!(
        !present(cx, "project-review-check-bugs-set"),
        "only that check"
    );
    cx.update(|window, cx| window.click("project-review-check-security-reset", cx));
    until(
        &shell,
        cx,
        "the project goes back to Settings' value",
        |_| {
            review_listed(&user)["projects"][0]["settings"]
                .get("passes")
                .is_none_or(Value::is_null)
        },
    );
    until(&shell, cx, "the dot goes", |cx| {
        settle(cx);
        !present(cx, "project-review-check-security-set")
    });
    at_rest(cx);
}

#[gpui_kit::test]
fn the_findings_are_on_the_spec_s_page(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    // `session` makes the project, its workspace and a change; the spec's development is another
    // session, in a workspace of its own, which changes the code too.
    session(&user);
    user.call("extension.set", json!({"name": "specs", "enabled": true}))
        .unwrap();
    let spec = user
        .call("spec.new", json!({"project": "app", "title": "Magic link"}))
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    for (section, content) in [
        ("problem", json!("Users forget their passwords.")),
        (
            "behaviour",
            json!([{"text": "A link sent by mail signs the user in once."}]),
        ),
        (
            "verification",
            json!([{"id": "C1", "text": "Open the link twice: the second is refused."}]),
        ),
    ] {
        user.call(
            "spec.write",
            json!({"spec": spec, "section": section, "content": content}),
        )
        .unwrap();
    }
    user.call("spec.ready", json!({"spec": spec})).unwrap();
    let developed = user.call("spec.develop", json!({"spec": spec})).unwrap();
    let developer = developed["agent"].as_str().unwrap().to_owned();
    let workspace = user
        .call("agent.show", json!({"agent": developer}))
        .unwrap()["agent"]["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the development's workspace", || {
        user.call(
            "workspace.show",
            json!({"project": "app", "name": workspace}),
        )
        .unwrap()["state"]
            == "ready"
    });
    let folder = std::path::PathBuf::from(
        user.call(
            "workspace.show",
            json!({"project": "app", "name": workspace}),
        )
        .unwrap()["folder"]
            .as_str()
            .unwrap(),
    );
    std::fs::write(folder.join("link.rs"), "pub fn link() {}\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "link"]);
    let started = user
        .call("review.start", json!({"agent": developer}))
        .unwrap();
    let pilot = pilot(&user);
    let reported = pilot.call(
        "review",
        json!({"action": "report", "findings": [{"severity": "high", "confidence": "confirmed",
            "concerns": "C1", "quote": "signs the user in once", "file": "link.rs",
            "lines": "1", "text": "the link signs in twice", "case": "open it twice"}]}),
    );
    assert!(!reported.error, "{}", reported.text);

    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let review = started["review"].as_u64().unwrap();
    let row = format!("spec-finding-row-{review}-F1");
    until(&shell, cx, "the spec's page shows the finding", |cx| {
        shell.update_in(cx, |s, window, cx| {
            s.open(
                Selection::Spec {
                    project: "app".into(),
                    spec,
                },
                window,
                cx,
            )
        });
        label(cx, &row).is_some()
    });
    let said = label(cx, &row).unwrap();
    assert!(said.contains("C1"), "{said}");
    assert!(said.contains("the link signs in twice"), "{said}");
    assert!(said.contains("· open:"), "{said}");
}

#[gpui_kit::test]
fn an_off_check_s_model_select_is_disabled_and_fades(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, true, cx);
    let enabled = |cx: &mut VisualTestContext, key: &str| {
        value(cx, &format!("review-setting-{key}_model-model-root"))
    };
    // Security is off by default: its select is faded from the first frame, the others are not.
    assert_eq!(enabled(cx, "security"), Some(0.0));
    assert_eq!(enabled(cx, "bugs"), Some(1.0));

    // Turning a check off fades its select, it does not jump.
    cx.update(|window, cx| window.click("review-check-bugs-switch", cx));
    until(&shell, cx, "bugs is off", |_| {
        review_listed(&user)["settings"][0]["value"] == json!(["spec", "standards"])
    });
    // The window reads the list again a moment after the event; the select then starts to fade.
    until(&shell, cx, "the select starts to fade", |cx| {
        one_frame(cx);
        enabled(cx, "bugs").is_some_and(|enabled| enabled < 1.0)
    });
    let part = enabled(cx, "bugs").unwrap();
    assert!(0.0 < part && part < 1.0, "part way: {part}");
    settle(cx);
    assert_eq!(enabled(cx, "bugs"), Some(0.0));
    assert_eq!(enabled(cx, "standards"), Some(1.0), "only that check");

    // It takes no click: no list opens.
    cx.update(|window, cx| window.click("review-setting-bugs_model-model", cx));
    settle(cx);
    assert!(!present(cx, "review-setting-bugs_model-model-menu"));
    // And it comes back with its switch.
    cx.update(|window, cx| window.click("review-check-bugs-switch", cx));
    until(&shell, cx, "bugs is on", |_| {
        review_listed(&user)["settings"][0]["value"] == json!(["spec", "bugs", "standards"])
    });
    until(&shell, cx, "the select is enabled again", |cx| {
        settle(cx);
        enabled(cx, "bugs") == Some(1.0)
    });
    cx.update(|window, cx| window.click("review-setting-bugs_model-model", cx));
    settle(cx);
    assert!(present(cx, "review-setting-bugs_model-model-menu"));
    cx.update(|window, cx| window.click("review-setting-bugs_model-model", cx));
    at_rest(cx);
}

#[gpui_kit::test]
fn a_check_s_instructions_chevron_turns_and_its_summary_gives_way(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, true, cx);
    until(&shell, cx, "the instructions are read", |cx| {
        label(cx, "review-check-bugs-instructions-summary").as_deref() == Some("none")
    });
    settle(cx);
    assert_eq!(
        value(cx, "review-check-bugs-instructions-chevron"),
        Some(1.0)
    );
    assert_eq!(
        value(cx, "review-check-bugs-instructions-summary"),
        Some(1.0)
    );

    cx.update(|window, cx| window.click("review-check-bugs-instructions", cx));
    one_frame(cx);
    one_frame(cx);
    let turned = value(cx, "review-check-bugs-instructions-chevron").unwrap();
    assert!(0.0 < turned && turned < 1.0, "turning: {turned}");
    let shown = value(cx, "review-check-bugs-instructions-summary").unwrap();
    assert!(0.0 < shown && shown < 1.0, "giving way: {shown}");
    settle(cx);
    assert_eq!(
        value(cx, "review-check-bugs-instructions-chevron"),
        Some(0.0)
    );
    assert_eq!(
        value(cx, "review-check-bugs-instructions-summary"),
        Some(0.0)
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn the_projects_line_unfolds_and_fades_in_when_a_project_does_otherwise(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, true, cx);
    assert!(!present(cx, "review-projects"));
    let folder = user.path("work/acme-shop");
    user.git_init(&folder);
    user.call(
        "project.add",
        json!({"folder": folder, "name": "acme-shop"}),
    )
    .unwrap();
    user.call(
        "extension.set",
        json!({"name": "review", "project": "acme-shop",
            "settings": {"passes": ["spec", "bugs", "standards", "security"]}}),
    )
    .unwrap();
    until(&shell, cx, "the line starts to unfold", |cx| {
        one_frame(cx);
        present(cx, "review-projects-block")
    });
    let first = bounds(cx, "review-projects-block").size.height;
    settle(cx);
    let full = bounds(cx, "review-projects-block").size.height;
    assert!(first < full, "it unfolds: {first:?} then {full:?}");
    assert_eq!(
        label(cx, "review-projects").as_deref(),
        Some("acme-shop runs Security too")
    );
    at_rest(cx);
}

/// A project `app` with Review on, its page open, and the project running every check but
/// Security as Settings does.
fn project_page<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();
    // The project chooses the model of Bugs itself.
    user.call(
        "extension.set",
        json!({"name": "review", "project": "app",
            "settings": {"bugs_model": {"provider": "claude"}}}),
    )
    .unwrap();
    let (shell, cx) = window(user, cx);
    cx.simulate_resize(size(px(1280.), px(3200.)));
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at(
            "app",
            flottille_window::project_settings::Reveal::Feature("review".into()),
            window,
            cx,
        )
    });
    until(&shell, cx, "the project's page of Review", |cx| {
        present(cx, "project-review-checks")
    });
    until(&shell, cx, "the dot of Bugs' model", |cx| {
        settle(cx);
        present(cx, "project-review-setting-bugs_model-set")
    });
    (shell, cx)
}

fn project_bugs_model(user: &User) -> Option<Value> {
    review_listed(user)["projects"][0]["settings"]
        .get("bugs_model")
        .filter(|model| !model.is_null())
        .cloned()
}

#[gpui_kit::test]
fn choosing_as_in_settings_for_a_check_s_model_stores_null(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = project_page(&user, cx);
    assert!(project_bugs_model(&user).is_some());
    cx.update(|window, cx| window.click("project-review-setting-bugs_model-model", cx));
    settle(cx);
    cx.update(|window, cx| window.click("project-review-setting-bugs_model-model-first", cx));
    until(&shell, cx, "the project's model is Settings'", |_| {
        project_bugs_model(&user).is_none()
    });
    until(&shell, cx, "the dot goes", |cx| {
        settle(cx);
        !present(cx, "project-review-setting-bugs_model-set")
    });
}

#[gpui_kit::test]
fn the_dot_and_the_way_back_of_a_model_still_work_while_its_check_is_off(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = project_page(&user, cx);
    // The project turns Bugs off: its select fades, its dot and way back stay.
    cx.update(|window, cx| window.click("project-review-check-bugs-switch", cx));
    until(&shell, cx, "the project skips Bugs", |_| {
        review_listed(&user)["projects"][0]["settings"]["passes"] == json!(["spec", "standards"])
    });
    until(&shell, cx, "the select fades", |cx| {
        settle(cx);
        value(cx, "project-review-setting-bugs_model-model-root") == Some(0.0)
    });
    assert!(present(cx, "project-review-setting-bugs_model-set"));
    cx.update(|window, cx| window.click("project-review-setting-bugs_model-reset", cx));
    until(&shell, cx, "the project's model is Settings'", |_| {
        project_bugs_model(&user).is_none()
    });
}

#[gpui_kit::test]
fn a_check_s_unreadable_instructions_say_the_check_and_the_file_only(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/review-standards.md"),
        b"a text \xff\xfe that is not UTF-8",
    )
    .unwrap();
    std::fs::write(
        user.path("config/flottille/review.md"),
        b"a text \xff\xfe that is not UTF-8",
    )
    .unwrap();
    let (shell, cx) = review_page(&user, true, cx);
    cx.update(|window, cx| window.click("review-check-standards-instructions", cx));
    let said = |cx: &mut VisualTestContext, id: &str| {
        until(&shell, cx, "the error is said", |cx| {
            settle(cx);
            label(cx, id).is_some()
        });
        label(cx, id).unwrap()
    };
    let check = said(cx, "review-standards-guidance-error");
    assert_eq!(
        check,
        "The Project rules check does not run until this is fixed: review-standards.md is not \
         UTF-8 text: write it again, or reset it."
    );
    let shared = said(cx, "review-guidance-error");
    assert!(shared.contains("review.md"), "{shared}");
    for said in [&check, &shared] {
        assert!(!said.contains(&user.path("config").to_string_lossy().into_owned()));
        assert!(!said.contains("guidance for"), "{said}");
    }
}

#[gpui_kit::test]
fn a_part_set_from_the_command_line_is_not_stale_in_an_open_window(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let (shell, cx) = review_page(&user, true, cx);
    until(&shell, cx, "the instructions are read", |cx| {
        label(cx, "review-check-security-instructions-summary").as_deref() == Some("none")
    });
    // `flottille review set`, with a part, while the window is open: review.changed {part}.
    user.call(
        "review.set",
        json!({"part": "security", "text": "Look at the tokens first."}),
    )
    .unwrap();
    until(&shell, cx, "the part is read again", |cx| {
        one_frame(cx);
        label(cx, "review-check-security-instructions-summary")
            .is_some_and(|said| said.starts_with("Look at the tokens"))
    });
    // The shared guidance, set without a part, is read again too.
    user.call("review.set", json!({"text": "Be brief."}))
        .unwrap();
    let editor = shell
        .read_with(cx, |s, cx| {
            s.extensions().read(cx).guidance("review").cloned()
        })
        .unwrap();
    until(&shell, cx, "the shared guidance is read again", |cx| {
        one_frame(cx);
        editor.read_with(cx, |e, _| {
            e.shown().is_some_and(|shown| shown.text == "Be brief.")
        })
    });
}
