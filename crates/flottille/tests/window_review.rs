//! Review in the window (`docs/extensions.md`, "Review"), on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent: a review's agents are one folded "Review" line
//! of the session's rail, with its state, unfolding to its pilot and passes. No window opens on a
//! screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::Mcp;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::session::{RailRow, SessionPage};
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
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
    user.fake_script(json!({"turns": [
        {"when": "of the change of workspace", "do": [{"hang": true}]},
        {"when": "The change, in each repository", "do": [{"hang": true}]}
    ]}));
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

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
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

#[gpui_kit::test]
fn the_review_s_agents_are_one_folded_line_of_the_tree(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let session = session(&user);
    let started = user
        .call("review.start", json!({"agent": session}))
        .unwrap();
    let pilot_id = started["pilot"].as_str().unwrap().to_owned();
    let pilot = pilot(&user);
    let ran = pilot.call("review", json!({"action": "run_pass", "pass": "bugs"}));
    assert!(!ran.error, "{}", ran.text);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&session)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&session), cx));
    until(&shell, cx, "the rail holds the review's line", |cx| {
        cx.update(|window, _| window.try_find(format!("rail-review-{pilot_id}")).is_some())
    });
    settle(cx);

    // One folded line: neither the pilot nor its pass has a row of its own.
    let page = page(&shell, cx);
    let rows = page.read_with(cx, |p, cx| p.rail().read(cx).rows());
    assert_eq!(rows[0], RailRow::Agent(session.clone()));
    assert_eq!(
        rows[1],
        RailRow::Review {
            pilot: pilot_id.clone(),
            count: 2,
            open: false
        }
    );
    assert!(!present(cx, &format!("rail-row-{pilot_id}")));
    assert_eq!(
        label(cx, &format!("rail-review-{pilot_id}")).as_deref(),
        Some("Review · 0/2 passes")
    );
    assert_eq!(behind(&pilot_id, cx), None, "folded, nothing is drawn");

    // A click unfolds it to the review's agents, growing into place.
    cx.update(|window, cx| window.click(format!("rail-review-{pilot_id}"), cx));
    let first = behind(&pilot_id, cx).expect("drawn from the first frame");
    settle(cx);
    let open = behind(&pilot_id, cx).unwrap();
    assert!(first < open, "it grows: {first} then {open}");
    assert!(present(cx, &format!("rail-row-{pilot_id}")));

    // Its state follows the review: once it reported, it says its findings.
    let reported = pilot.call(
        "review",
        json!({"action": "report", "findings": [{"severity": "low", "confidence": "confirmed",
            "concerns": "code", "file": "login.rs", "lines": "1", "text": "no doc comment",
            "case": "read it"}]}),
    );
    assert!(!reported.error, "{}", reported.text);
    until(&shell, cx, "the line says the findings", |cx| {
        label(cx, &format!("rail-review-{pilot_id}"))
            .is_some_and(|said| said.ends_with("· 1 finding"))
    });

    // Its findings are under it, each with its status, which follows the user's word.
    let row = format!("rail-finding-row-{pilot_id}-F1");
    until(&shell, cx, "the finding is under the line", |cx| {
        label(cx, &row).is_some_and(|said| said.contains("no doc comment"))
    });
    assert!(label(cx, &row).unwrap().contains("· open:"));
    user.call(
        "review.finding",
        json!({"session": session, "finding": "F1", "status": "dismissed"}),
    )
    .unwrap();
    until(&shell, cx, "the finding says it is dismissed", |cx| {
        label(cx, &row).is_some_and(|said| said.contains("· dismissed:"))
    });

    // A second click folds it away again, and nothing moves at rest.
    cx.update(|window, cx| window.click(format!("rail-review-{pilot_id}"), cx));
    settle(cx);
    assert!(!present(cx, &format!("rail-row-{pilot_id}")));
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

#[gpui_kit::test]
fn review_s_tab_has_its_switch_its_passes_its_slots_and_its_guidance(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(2400.)));
    shell.update_in(cx, |s, window, cx| s.open(Selection::Settings, window, cx));
    until(&shell, cx, "the Review tab is offered", |cx| {
        present(cx, "settings-tabs-review")
    });
    cx.update(|window, cx| window.click("settings-tabs-review", cx));
    settle(cx);
    // Off: the switch and its one line.
    assert_eq!(label(cx, "review-switch-label").as_deref(), Some("On"));
    assert!(!present(cx, "review-setting-passes-label"));
    assert!(!present(cx, "review-guidance-text"));

    cx.update(|window, cx| window.click("review-switch", cx));
    until(&shell, cx, "the daemon has it on", |_| {
        review_listed(&user)["enabled"] == true
    });
    until(&shell, cx, "the settings unfold", |cx| {
        present(cx, "review-setting-passes-label")
    });
    assert!(step(cx) + step(cx) > 0, "they unfold");
    settle(cx);
    for id in [
        "review-setting-passes-spec",
        "review-setting-passes-bugs",
        "review-setting-passes-standards",
        "review-setting-passes-security",
        "review-setting-bugs_model-select",
        "review-setting-verifier_model-select",
        "review-guidance-text",
        "review-guidance-save",
    ] {
        assert!(present(cx, id), "{id}");
    }

    // A pass switched on is the daemon's, in the declaration's order.
    cx.update(|window, cx| window.click("review-setting-passes-security", cx));
    until(&shell, cx, "the passes are kept", |_| {
        review_listed(&user)["settings"][0]["value"]
            == json!(["spec", "bugs", "standards", "security"])
    });

    // The guidance is the daemon's to keep, as the judge's is.
    let editor = shell
        .read_with(cx, |s, cx| {
            s.extensions().read(cx).guidance("review").cloned()
        })
        .unwrap();
    until(&shell, cx, "the guidance is read", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    let field = editor.read_with(cx, |e, _| e.field().cloned()).unwrap();
    field.update_in(cx, |field, window, cx| {
        field.set_value("Look at the migrations first.", window, cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click("review-guidance-save", cx));
    until(&shell, cx, "the guidance is kept", |_| {
        user.call("review.show", json!({})).unwrap()["text"] == "Look at the migrations first."
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
