//! The scenarios of Specs in the window (#334, `docs/extensions.md`, "In the window"): its tab of
//! Settings drawn from the declaration, its section of a project's settings, the project's Specs
//! tab, "Write the spec" on a ticket's row and a spec's page. On GPUI's test platform in front of a
//! real daemon in temporary folders, its agents the fake ACP agent, its forge the fake `gh`, and
//! rmcp's client in place of the writing session's own. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::extensions::specs::SpecPage;
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// What the declaration of Specs says it does, the one line its tab shows while it is off.
const SENTENCE: &str = "Keeps a spec of each task to build: what an agent needs to build it \
                        without guessing. It runs no model on its own.";

const OLD: &str = "2020-01-01T00:00:00Z";

fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_fetcher()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// What the fake `gh` answers of an issue of `acme/app`.
fn issue(number: u64, title: &str) -> Value {
    json!({
        "number": number, "title": title, "state": "OPEN", "body": "What is asked.",
        "url": format!("https://github.com/acme/app/issues/{number}"),
        "author": {"login": "other"}, "assignees": [], "labels": [],
        "updatedAt": OLD, "comments": []
    })
}

/// The project `app` over a throwaway repository whose `origin` is `acme/app` on GitHub, its
/// forge and its tracker declared there, with the issues #5 and #6.
fn project(user: &User) {
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
    user.gh_script(json!({
        "pulls": {},
        "issues": {"acme/app": {
            "5": issue(5, "Users cannot sign in"),
            "6": issue(6, "Typo on the page")
        }}
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
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
}

fn turn(user: &User, enabled: bool, project: Option<&str>) {
    user.call(
        "extension.set",
        json!({"name": "specs", "enabled": enabled, "project": project}),
    )
    .unwrap();
}

/// Specs as `extension.list` says it.
fn listed(user: &User) -> Value {
    user.call("extension.list", json!({})).unwrap()["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "specs")
        .cloned()
        .unwrap()
}

fn show(user: &User, spec: u64) -> Value {
    user.call("spec.show", json!({"spec": spec})).unwrap()
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
    cx.simulate_resize(size(px(1280.), px(1600.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
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

/// Lets every animation end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn at_rest(cx: &mut VisualTestContext) {
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

/// One frame, 16 ms on: how many elements it drew that move.
fn step(cx: &mut VisualTestContext) -> usize {
    cx.executor().advance_clock(Duration::from_millis(16));
    let drawn = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    drawn
}

fn found(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn click(cx: &mut VisualTestContext, id: &str) {
    cx.update(|window, cx| window.click(id.to_owned(), cx));
}

fn open(shell: &Entity<Shell>, cx: &mut VisualTestContext, selection: Selection) {
    shell.update_in(cx, |s, window, cx| s.open(selection, window, cx));
}

/// The window on Settings, the Specs tab chosen once Specs is listed.
fn open_specs_tab(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    open(shell, cx, Selection::Settings);
    until(shell, cx, "the Specs tab is offered", |cx| {
        found(cx, "settings-tabs-specs")
    });
    click(cx, "settings-tabs-specs");
    settle(cx);
}

/// The window on `app`'s page, once its items are read.
fn open_project(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the project page shows", |cx| {
        open(shell, cx, Selection::Project("app".into()));
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).items().read(cx).is_read())
        })
    });
    settle(cx);
}

/// The id of the row of ticket `#number` in the project's Tickets tab, once it is listed.
fn ticket_row(shell: &Entity<Shell>, cx: &mut VisualTestContext, number: &str) -> Option<String> {
    shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).project_page()?.clone();
        let items = page.read(cx).items().clone();
        let rows = items.read(cx).rows();
        rows.iter()
            .find(|row| row.kind == "ticket" && row.key == number)
            .map(|row| row.id())
    })
}

/// Waits for the rows of tickets #5 and #6: their ids.
fn ticket_rows(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (String, String) {
    until(shell, cx, "the tickets are listed", |cx| {
        ticket_row(shell, cx, "5").is_some_and(|id| found(cx, &id))
    });
    (
        ticket_row(shell, cx, "5").unwrap(),
        ticket_row(shell, cx, "6").unwrap(),
    )
}

fn spec_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<SpecPage>> {
    shell.read_with(cx, |s, cx| s.main().read(cx).spec_page().cloned())
}

/// The window on the page of spec `spec`, once it is read.
fn open_spec(shell: &Entity<Shell>, cx: &mut VisualTestContext, spec: u64) -> Entity<SpecPage> {
    until(shell, cx, "the spec's page shows the spec", |cx| {
        open(
            shell,
            cx,
            Selection::Spec {
                project: "app".into(),
                spec,
            },
        );
        spec_page(shell, cx).is_some_and(|page| page.read_with(cx, |p, _| p.is_read()))
    });
    settle(cx);
    spec_page(shell, cx).unwrap()
}

/// "New spec" from an idea, on the daemon's side: the spec, and the client of the session that
/// writes it.
fn new_spec(user: &User) -> (u64, String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "spec.new",
            json!({"project": "app", "idea": "Let users log in with a link sent by mail"}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["spec"]["spec"].as_u64().unwrap(),
        answer["agent"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

/// The writing session asks one question: two options, the second recommended.
fn ask(mcp: &Mcp, spec: u64) {
    let answer = mcp.call(
        "spec",
        json!({"action": "ask", "spec": spec, "questions": [{
            "question": "Where does the link lead once it is used?",
            "options": ["The page the user asked for", "The home page"],
            "recommended": "The home page",
            "reason": "No page is remembered across the mail today"
        }]}),
    );
    assert!(!answer.error, "{}", answer.text);
}

fn question_of(user: &User, spec: u64) -> u64 {
    show(user, spec)["asked"][0]["question"].as_u64().unwrap()
}

#[gpui_kit::test]
fn specs_tab_off_shows_the_switch_and_one_line(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (shell, cx) = window(&user, cx);
    open_specs_tab(&shell, cx);
    at_rest(cx);

    // Its title, its switch with the declaration's sentence as hint, and nothing else.
    assert_eq!(label(cx, "specs-title").as_deref(), Some("Specs"));
    assert_eq!(label(cx, "specs-switch-label").as_deref(), Some("On"));
    assert_eq!(label(cx, "specs-switch-hint").as_deref(), Some(SENTENCE));
    assert!(!found(cx, "specs-setting-language-label"));
    // After the Judge, before the Providers.
    let at = |cx: &mut VisualTestContext, id: &str| {
        cx.update(|window, _| window.find(id.to_owned()).bounds().origin.x)
    };
    assert!(at(cx, "settings-tabs-judge") < at(cx, "settings-tabs-specs"));
    assert!(at(cx, "settings-tabs-specs") < at(cx, "settings-tabs-providers"));
}

#[gpui_kit::test]
fn turning_specs_on_unfolds_its_settings(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (shell, cx) = window(&user, cx);
    open_specs_tab(&shell, cx);
    at_rest(cx);
    assert!(!found(cx, "specs-setting-language-label"));

    click(cx, "specs-switch");
    until(&shell, cx, "the daemon has it on", |_| {
        listed(&user)["enabled"] == true
    });
    until(&shell, cx, "the window knows", |cx| {
        found(cx, "specs-setting-language-label")
    });
    // It unfolds: it moves, then it is there at rest, labelled.
    assert!(step(cx) + step(cx) > 0, "the settings unfold");
    at_rest(cx);
    assert_eq!(
        label(cx, "specs-setting-language-label").as_deref(),
        Some("Language")
    );

    // Chosen, a language is the daemon's.
    click(cx, "specs-setting-language-select");
    settle(cx);
    click(cx, "specs-setting-language-select-fr");
    until(&shell, cx, "the language is kept", |_| {
        listed(&user)["settings"][0]["value"] == "fr"
    });

    // Off again, its settings fold away and the switch and its line stay.
    click(cx, "specs-switch");
    until(&shell, cx, "the daemon has it off", |_| {
        listed(&user)["enabled"] == false
    });
    until(&shell, cx, "the settings fold away", |cx| {
        settle(cx);
        !found(cx, "specs-setting-language-label")
    });
    assert_eq!(label(cx, "specs-switch-hint").as_deref(), Some(SENTENCE));
}

#[gpui_kit::test]
fn the_project_section_shows_only_while_specs_is_on_globally(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project's settings show", |cx| {
        open(&shell, cx, Selection::ProjectSettings("app".into()));
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().is_some()
        })
    });
    settle(cx);
    assert!(!found(cx, "project-settings-tabs-specs"));

    turn(&user, true, None);
    until(&shell, cx, "the project's Specs tab shows", |cx| {
        found(cx, "project-settings-tabs-specs")
    });
    click(cx, "project-settings-tabs-specs");
    settle(cx);
    assert_eq!(label(cx, "specs-title").as_deref(), Some("Specs"));
    assert_eq!(
        label(cx, "project-specs-switch-label").as_deref(),
        Some("On in this project")
    );
    assert!(found(cx, "project-specs-setting-language-label"));

    // Its switch is the project's own.
    click(cx, "project-specs-switch");
    until(&shell, cx, "the project has it off", |_| {
        listed(&user)["projects"][0]["switch"] == false
    });
    assert_eq!(listed(&user)["enabled"], true);
    // Off, its values fold away.
    until(&shell, cx, "the project's values fold away", |cx| {
        settle(cx);
        !found(cx, "project-specs-setting-language-label")
    });

    // Off globally, the section goes, the project's switch kept.
    turn(&user, false, None);
    until(&shell, cx, "the project's Specs tab goes", |cx| {
        !found(cx, "project-settings-tabs-specs")
    });
    assert_eq!(listed(&user)["projects"][0]["switch"], false);
}

#[gpui_kit::test]
fn the_specs_tab_lists_each_spec_with_its_status_and_tickets(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    turn(&user, true, None);
    let alone = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Sign in with a link"}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    user.call(
        "spec.write",
        json!({"spec": alone, "section": "behaviour",
            "content": [{"text": "A link signs the user in once."}]}),
    )
    .unwrap();
    user.call("spec.ready", json!({"spec": alone})).unwrap();
    let ticketed = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Fix the sign in", "tickets": ["#5"]}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    user.call(
        "spec.write",
        json!({"spec": ticketed, "section": "questions",
            "content": ["Which browsers?", "Which mails?"]}),
    )
    .unwrap();

    let (shell, cx) = window(&user, cx);
    open_project(&shell, cx);
    until(&shell, cx, "the Specs tab counts them", |cx| {
        label(cx, "project-items-tabs-specs-count").as_deref() == Some("2")
    });
    click(cx, "project-items-tabs-specs");
    at_rest(cx);

    let facts = label(cx, &format!("spec-{alone}-facts")).unwrap();
    assert!(facts.contains("Ready"), "{facts}");
    let facts = label(cx, &format!("spec-{ticketed}-facts")).unwrap();
    for fact in ["Draft", "#5", "2 open questions"] {
        assert!(facts.contains(fact), "{fact}: {facts}");
    }
    assert_eq!(
        label(cx, &format!("spec-{ticketed}")).as_deref(),
        Some("Fix the sign in")
    );

    // A spec made elsewhere unfolds into the list.
    let third = user
        .call("spec.new", json!({"project": "app", "title": "Sign out"}))
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    until(&shell, cx, "the new spec is listed", |cx| {
        found(cx, &format!("spec-{third}"))
    });
    assert!(step(cx) + step(cx) > 0, "the row unfolds");
    at_rest(cx);

    // A click opens its page.
    click(cx, &format!("spec-{alone}"));
    until(&shell, cx, "the spec's page shows", |cx| {
        spec_page(&shell, cx).is_some_and(|page| page.read_with(cx, |p, _| p.is_read()))
    });
    settle(cx);
    assert_eq!(
        label(cx, "spec-header-title").as_deref(),
        Some("Sign in with a link")
    );
    assert_eq!(label(cx, "spec-status").as_deref(), Some("Ready"));
}

#[gpui_kit::test]
fn a_question_is_answered_in_one_click(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    turn(&user, true, None);
    let (spec, writer, mcp) = new_spec(&user);
    ask(&mcp, spec);
    let question = question_of(&user, spec);

    let (shell, cx) = window(&user, cx);
    open_spec(&shell, cx, spec);
    at_rest(cx);
    let id = format!("spec-question-{question}");
    assert_eq!(
        label(cx, &format!("{id}-option-2-recommended")).as_deref(),
        Some("Recommended")
    );
    assert!(!found(cx, &format!("{id}-option-1-recommended")));
    assert_eq!(
        label(cx, &format!("{id}-reason")).as_deref(),
        Some("No page is remembered across the mail today")
    );
    // The session that writes it, which opens it.
    assert!(found(cx, &format!("spec-session-{writer}")));

    click(cx, &format!("{id}-option-2"));
    until(&shell, cx, "the answer is the daemon's", |_| {
        show(&user, spec)["asked"][0]["state"] == "answered"
    });
    let shown = show(&user, spec);
    assert_eq!(shown["decisions"][0]["decision"], "The home page");
    // The question folds away and the decision it became unfolds.
    until(&shell, cx, "the decision shows", |cx| {
        found(cx, "spec-decision-D1")
    });
    assert!(step(cx) + step(cx) > 0, "they move");
    at_rest(cx);
    assert!(!found(cx, &id));
    let decision = label(cx, "spec-decision-D1").unwrap();
    assert!(decision.contains("The home page"), "{decision}");
}

#[gpui_kit::test]
fn ready_is_disabled_with_an_open_question_and_says_why(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    turn(&user, true, None);
    let (spec, _writer, mcp) = new_spec(&user);
    ask(&mcp, spec);
    let answer = mcp.call(
        "spec",
        json!({"action": "write", "spec": spec, "section": "behaviour",
            "content": [{"text": "A link signs the user in once."}]}),
    );
    assert!(!answer.error, "{}", answer.text);

    let (shell, cx) = window(&user, cx);
    let page = open_spec(&shell, cx, spec);
    at_rest(cx);
    assert!(!page.read_with(cx, |p, _| p.can_ready()));
    assert_eq!(
        label(cx, "spec-ready-why").as_deref(),
        Some("1 question is open: answer it first.")
    );
    // A click does nothing.
    click(cx, "spec-ready");
    settle(cx);
    assert_eq!(show(&user, spec)["status"], "draft");

    let question = question_of(&user, spec);
    click(cx, &format!("spec-question-{question}-option-1"));
    until(&shell, cx, "Ready is enabled", |cx| {
        page.read_with(cx, |p, _| p.can_ready())
    });
    until(&shell, cx, "the line saying why folds away", |cx| {
        settle(cx);
        !found(cx, "spec-ready-why")
    });

    click(cx, "spec-ready");
    until(&shell, cx, "the spec is Ready", |_| {
        show(&user, spec)["status"] == "ready"
    });
    until(&shell, cx, "the page says it", |cx| {
        label(cx, "spec-status").as_deref() == Some("Ready")
    });
    // Back to draft, from Ready, is one click.
    settle(cx);
    click(cx, "spec-draft");
    until(&shell, cx, "the spec is a Draft again", |_| {
        show(&user, spec)["status"] == "draft"
    });
}

#[gpui_kit::test]
fn write_the_spec_on_a_ticket_becomes_open_the_spec_once_it_has_one(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    turn(&user, true, None);
    let (shell, cx) = window(&user, cx);
    open_project(&shell, cx);
    click(cx, "project-items-tabs-tickets");
    let (five, six) = ticket_rows(&shell, cx);
    at_rest(cx);
    assert_eq!(
        label(cx, &format!("{five}-spec")).as_deref(),
        Some("Write the spec")
    );

    click(cx, &format!("{five}-spec"));
    settle(cx);
    assert!(found(cx, "write-spec-mode"));
    click(cx, "write-spec-mode-ticket");
    settle(cx);
    click(cx, "write-spec-start");
    until(&shell, cx, "the daemon wrote a spec of the ticket", |_| {
        user.call("spec.list", json!({"project": "app"})).unwrap()["specs"]
            .as_array()
            .is_some_and(|specs| specs.len() == 1)
    });
    let spec = &user.call("spec.list", json!({"project": "app"})).unwrap()["specs"][0];
    assert_eq!(spec["tickets"], json!(["#5"]));
    assert_eq!(spec["mode"], "ticket");
    let id = spec["spec"].as_u64().unwrap();

    until(&shell, cx, "the row offers to open it", |cx| {
        label(cx, &format!("{five}-spec")).as_deref() == Some("Open the spec")
    });
    // The other ticket has none.
    assert_eq!(
        label(cx, &format!("{six}-spec")).as_deref(),
        Some("Write the spec")
    );
    settle(cx);
    click(cx, &format!("{five}-spec"));
    until(&shell, cx, "the spec's page shows", |cx| {
        spec_page(&shell, cx).is_some_and(|page| page.read_with(cx, |p, _| p.spec() == id))
    });
}

#[gpui_kit::test]
fn no_specs_screen_is_shown_where_specs_is_off(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    turn(&user, true, None);
    let spec = user
        .call("spec.new", json!({"project": "app", "title": "Sign out"}))
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    turn(&user, false, None);

    let (shell, cx) = window(&user, cx);
    open_project(&shell, cx);
    click(cx, "project-items-tabs-tickets");
    let (five, _) = ticket_rows(&shell, cx);
    at_rest(cx);
    let action = format!("{five}-spec");
    let none =
        |cx: &mut VisualTestContext| !found(cx, "project-items-tabs-specs") && !found(cx, &action);
    assert!(none(cx), "off globally, no screen of Specs");

    // On globally and off in the project: still none there.
    turn(&user, false, Some("app"));
    turn(&user, true, None);
    until(&shell, cx, "the window read the switches", |cx| {
        shell.read_with(cx, |s, cx| {
            s.extensions()
                .read(cx)
                .listed("specs")
                .is_some_and(|e| e.enabled)
        })
    });
    settle(cx);
    assert!(none(cx), "off in the project, no screen of Specs there");

    // Nor the page of one of its specs.
    open(
        &shell,
        cx,
        Selection::Spec {
            project: "app".into(),
            spec,
        },
    );
    settle(cx);
    assert!(spec_page(&shell, cx).is_none());

    // On in the project, they come.
    turn(&user, true, Some("app"));
    until(&shell, cx, "the window knows it is on there", |cx| {
        shell.read_with(cx, |s, cx| s.extensions().read(cx).active("specs", "app"))
    });
    // The spec's page asked for while it was off now shows.
    until(&shell, cx, "the spec's page shows", |cx| {
        spec_page(&shell, cx).is_some()
    });
    open_project(&shell, cx);
    until(&shell, cx, "the Specs tab comes", |cx| {
        found(cx, "project-items-tabs-specs")
    });
    click(cx, "project-items-tabs-tickets");
    until(&shell, cx, "the row's action comes", |cx| {
        found(cx, &action)
    });
}

/// The sections Ready needs, written from the user's door.
fn complete(user: &User, spec: u64) {
    for (section, content) in [
        ("problem", json!("Users forget their passwords.")),
        (
            "behaviour",
            json!([{"text": "A link sent by mail signs the user in once."}]),
        ),
    ] {
        user.call(
            "spec.write",
            json!({"spec": spec, "section": section, "content": content}),
        )
        .unwrap();
    }
}

/// The calls of the fake `gh` that start with `words`.
fn gh_calls(user: &User, words: &[&str]) -> usize {
    user.gh_received()
        .iter()
        .filter(|call| {
            let args = call["args"].as_array().unwrap();
            words.iter().enumerate().all(|(at, word)| args[at] == *word)
        })
        .count()
}

#[gpui_kit::test]
fn a_changed_ticket_offers_take_and_overwrite_on_the_spec_page(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    turn(&user, true, None);
    // Never read by Flottille, the ticket is not known to be as it last read it: Ready writes
    // nothing and the user chooses.
    let spec = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Sign in", "mode": "ticket", "tickets": ["#5"]}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    complete(&user, spec);
    let ready = user.call("spec.ready", json!({"spec": spec})).unwrap();
    assert_eq!(ready["ticket_write"]["state"], "changed", "{ready}");

    let (shell, cx) = window(&user, cx);
    open_spec(&shell, cx, spec);
    at_rest(cx);
    let said = label(cx, "spec-ticket-said").unwrap_or_default();
    assert!(said.contains("#5 changed on its tracker"), "{said}");
    assert!(found(cx, "spec-ticket-take") && found(cx, "spec-ticket-overwrite"));

    // Overwrite asks first: a person's change is lost on the tracker.
    click(cx, "spec-ticket-overwrite");
    settle(cx);
    assert!(found(cx, "spec-ticket-confirm-action"));
    assert_eq!(gh_calls(&user, &["issue", "edit"]), 0);
    click(cx, "spec-ticket-confirm-action");
    until(&shell, cx, "the ticket is written", |_| {
        gh_calls(&user, &["issue", "edit"]) == 1
    });
    until(&shell, cx, "the page says it", |cx| {
        settle(cx);
        label(cx, "spec-ticket-said").is_some_and(|said| said.contains("written to #5"))
            && !found(cx, "spec-ticket-take")
    });

    // Take: another spec whose ticket changed goes back to Draft in one click.
    let other = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Typo", "mode": "ticket", "tickets": ["#6"]}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    complete(&user, other);
    user.call("spec.ready", json!({"spec": other})).unwrap();
    open_spec(&shell, cx, other);
    settle(cx);
    click(cx, "spec-ticket-take");
    until(&shell, cx, "the spec is a Draft again", |_| {
        show(&user, other)["status"] == "draft"
    });
    until(&shell, cx, "the choice folds away", |cx| {
        settle(cx);
        !found(cx, "spec-ticket-said") && label(cx, "spec-status").as_deref() == Some("Draft")
    });
    assert_eq!(gh_calls(&user, &["issue", "edit"]), 1);
}

#[gpui_kit::test]
fn the_departures_are_offered_at_done_and_posted_with_one_click(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    turn(&user, true, None);
    let spec = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Typo", "mode": "internal", "tickets": ["#6"]}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    complete(&user, spec);
    let departures = "The ticket asks for the footer too: left out.";
    user.call(
        "spec.write",
        json!({"spec": spec, "section": "departures", "content": departures}),
    )
    .unwrap();
    user.call("spec.ready", json!({"spec": spec})).unwrap();

    let (shell, cx) = window(&user, cx);
    open_spec(&shell, cx, spec);
    at_rest(cx);
    // Read before Done, and not offered to post.
    assert_eq!(
        label(cx, "spec-departures-text").as_deref(),
        Some(departures)
    );
    assert!(!found(cx, "spec-departures-post"));

    // Done is read from what Flottille sees (#336): set beside the daemon, then read again.
    let connection = rusqlite::Connection::open(user.path("state/flottille/flottille.db")).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection
        .execute(
            "UPDATE ext_specs_specs SET status = 'done' WHERE id = ?1",
            [spec],
        )
        .unwrap();
    let page = spec_page(&shell, cx).unwrap();
    page.update(cx, |page, cx| page.read(cx));
    until(&shell, cx, "Post is offered", |cx| {
        settle(cx);
        found(cx, "spec-departures-post")
    });
    assert_eq!(
        gh_calls(&user, &["issue", "comment"]),
        0,
        "nothing is posted on its own"
    );

    click(cx, "spec-departures-post");

    until(&shell, cx, "the comment is posted", |_| {
        gh_calls(&user, &["issue", "comment"]) == 1
    });
    until(&shell, cx, "the page says it", |cx| {
        settle(cx);
        label(cx, "spec-departures-posted").is_some_and(|said| said.contains("#6"))
            && !found(cx, "spec-departures-post")
    });
}
