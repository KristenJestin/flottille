//! The scenarios of the rules proposed from the failures agents keep making, in the window
//! (`docs/projects.md`, "Proposed rules"): a proposal rings the bell, which opens the project's
//! Settings, where its rule is edited then added to the project's rules, or dismissed; either way
//! it folds away. On GPUI's test platform in front of a real daemon whose agents and judge are the
//! fake ACP agent, with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::project_settings::ProjectSettingsPage;
use flottille_window::proposed_rules::{ProposedRules, part_id};
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The rule the fake judge writes.
const RULE: &str = "Never read the login folders of the home such as ~/.ssh: ask the user.";

/// A user whose sessions of the project `app` make, on `read the key <n>`, a call of `fs_read`
/// refused for a login's place; their judge writes `RULE`. Three sessions made it: one proposal,
/// whose id is returned.
fn proposed() -> (User, u64) {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_RULES_EVERY_MS", "0")
        .with_judge(json!([{"say": RULE}]));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let turns: Vec<Value> = (1..=3)
        .map(|n| {
            json!({"when": format!("read the key {n}"), "do": [
                {"mcp": {"tool": "fs_read", "arguments": {"path": format!("~/.ssh/id_{n}")}}},
                {"say": "done"}
            ]})
        })
        .collect();
    user.fake_script(json!({ "turns": turns }));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    for n in 1..=3 {
        let answer = user
            .call(
                "agent.run",
                json!({"project": "app", "message": format!("read the key {n}")}),
            )
            .unwrap();
        let id = answer["agent"]["id"].as_str().unwrap().to_owned();
        eventually("the turn's end", || {
            user.call("agent.show", json!({"agent": id})).unwrap()["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == "agent.turn_ended")
        });
    }
    eventually("the proposal", || !proposals(&user).is_empty());
    let id = proposals(&user)[0]["id"].as_u64().unwrap();
    (user, id)
}

fn proposals(user: &User) -> Vec<Value> {
    user.call("rules.proposals", json!({})).unwrap()["proposals"]
        .as_array()
        .unwrap()
        .clone()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
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
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
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
        cx.executor().advance_clock(Duration::from_millis(16));
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
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn present(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
    let id = id.into();
    cx.update(|window, cx| window.click(id, cx));
}

fn settings_page(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Option<Entity<ProjectSettingsPage>> {
    shell.read_with(cx, |s, cx| {
        s.main().read(cx).project_settings_page().cloned()
    })
}

fn section(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ProposedRules> {
    settings_page(shell, cx)
        .expect("the project's settings are open")
        .read_with(cx, |page, _| page.proposed_rules().clone())
}

/// The project's rules as the daemon says them.
fn rules_of(user: &User) -> String {
    user.call("rules.show", json!({"project": "app"})).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The proposal's row is drawn while the section folds it away, then goes.
fn folds_away(shell: &Entity<Shell>, id: u64, cx: &mut VisualTestContext) {
    let row = part_id(id, "row");
    until(shell, cx, "the answer", |cx| {
        section(shell, cx).read_with(cx, |section, cx| section.proposals(cx).is_empty())
    });
    assert!(present(cx, row.clone()), "it folds away, not gone at once");
    settle(cx);
    until(shell, cx, "the row gone", |cx| !present(cx, row.clone()));
}

#[gpui_kit::test]
fn a_proposed_rule_rings_the_bell_and_is_edited_then_added(cx: &mut TestAppContext) {
    let (user, id) = proposed();
    std::fs::write(
        user.project_file("app"),
        format!(
            "rules = \"Write tests first.\"\n{}",
            std::fs::read_to_string(user.project_file("app")).unwrap()
        ),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);

    let key = format!("rule-{id}");
    until(&shell, cx, "the bell says the proposal", |cx| {
        shell.read_with(cx, |s, cx| {
            s.bell_list().read(cx).said().contains(&(
                "Proposes a rule".to_owned(),
                RULE.to_owned(),
                "For app".to_owned(),
            ))
        })
    });
    shell.update(cx, |s, cx| s.toggle_bell(cx));
    settle(cx);
    click(cx, ElementId::Name(format!("bell-{key}").into()));
    until(&shell, cx, "the proposal in the project's settings", |cx| {
        present(cx, part_id(id, "row"))
    });
    settle(cx);
    assert!(
        settings_page(&shell, cx)
            .is_some_and(|page| page.read_with(cx, |page, _| page.name() == "app"))
    );

    let edited = "Never read ~/.ssh: ask the user instead.";
    let field = section(&shell, cx)
        .read_with(cx, |section, _| section.field(id).cloned())
        .expect("its field is drawn");
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        RULE
    );
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(edited, window, cx)));
    click(cx, part_id(id, "add"));

    folds_away(&shell, id, cx);
    assert_eq!(rules_of(&user), format!("Write tests first.\n\n{edited}"));
    assert!(proposals(&user).is_empty());
    until(&shell, cx, "the bell rid of it", |cx| {
        shell.read_with(cx, |s, cx| s.bell_list().read(cx).said().is_empty())
    });
}

#[gpui_kit::test]
fn a_dismissed_proposed_rule_folds_away(cx: &mut TestAppContext) {
    let (user, id) = proposed();
    let (shell, cx) = window(&user, cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings", |cx| {
        settings_page(&shell, cx).is_some()
    });
    click(cx, "project-settings-tabs-instructions");
    until(&shell, cx, "the proposal in the project's settings", |cx| {
        present(cx, part_id(id, "row"))
    });
    settle(cx);
    let before = rules_of(&user);

    click(cx, part_id(id, "dismiss"));

    folds_away(&shell, id, cx);
    assert!(proposals(&user).is_empty());
    assert_eq!(rules_of(&user), before, "nothing was added");
    // The section folds away with its last proposal.
    assert!(!present(cx, "proposed-rules-app"));
    // Nothing of the answered proposal is kept: its field is gone with its row.
    assert!(section(&shell, cx).read_with(cx, |section, _| section.field(id).is_none()));
}

#[gpui_kit::test]
fn a_rule_added_under_an_edit_of_the_rules_says_so(cx: &mut TestAppContext) {
    let (user, id) = proposed();
    let (shell, cx) = window(&user, cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings", |cx| {
        settings_page(&shell, cx).is_some()
    });
    click(cx, "project-settings-tabs-instructions");
    until(&shell, cx, "the proposal in the project's settings", |cx| {
        present(cx, part_id(id, "row"))
    });
    settle(cx);
    let editor = settings_page(&shell, cx)
        .unwrap()
        .read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the rules editor", |cx| {
        editor.read_with(cx, |editor, _| {
            editor.field().is_some() && editor.shown().is_some()
        })
    });
    let field = editor
        .read_with(cx, |editor, _| editor.field().cloned())
        .unwrap();
    cx.update(|window, cx| {
        field.update(cx, |field, cx| {
            field.set_value("My own rules, half written.", window, cx)
        })
    });
    assert!(editor.read_with(cx, |editor, cx| editor.is_edited(cx)));

    click(cx, part_id(id, "add"));
    folds_away(&shell, id, cx);

    let said: Vec<String> = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    });
    assert!(
        said.iter()
            .any(|title| title.starts_with("The rules changed while you were editing")),
        "{said:?}"
    );
    // What was typed stays: the toast only tells.
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        "My own rules, half written."
    );
}
