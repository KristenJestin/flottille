//! The card of a request for a vault entry in the window (#566): an agent asks the user for the
//! entry its project's tracker names and the vault lacks; the card shows what Flottille
//! prefilled, the agent's reason and a masked field, unfolds into the conversation, and folds away
//! once the user set the entry. On GPUI's test platform in front of a real daemon whose agent is
//! the fake ACP agent, with the in-memory keyring; no window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SECRET: &str = "w0rklog-t0ken-from-the-card";
const WHY: &str = "to log the two hours spent on ABC-1";

fn user() -> User {
    let user = User::with_fake_agent().with_fake_acli();
    user.fake_script(json!({"turns": [
        {"when": "log my time", "do": [
            {"mcp": {"tool": "vault", "arguments":
                {"action": "ask", "name": "jira-time", "why": WHY}}},
            {"say": "Asked for the token."},
            {"hold": "release"}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A session of `app`, whose Jira tracker names the missing entry `jira-time`, told to log time:
/// its turn holds until the test releases it, so the card stays in sight while it is answered.
fn session(user: &User) -> String {
    session_with(user, false)
}

/// As `session`; `taken` has another project `other` own the entry `jira-time` first.
fn session_with(user: &User, taken: bool) -> String {
    if taken {
        let folder = user.path("work/other");
        user.git_init(&folder);
        std::fs::write(folder.join("README.md"), "other\n").unwrap();
        user.git(&folder, &["add", "."]);
        user.git(&folder, &["commit", "--quiet", "-m", "start"]);
        user.call("project.add", json!({"folder": folder, "name": "other"}))
            .unwrap();
        user.call(
            "vault.set",
            json!({"name": "jira-time", "secret": "the-other-projects-secret",
                "project": "other"}),
        )
        .unwrap();
    }
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
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"],
                "worklog_token": "jira-time"}}),
    )
    .unwrap();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "log my time"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the ask", || {
        !user.call("vault.asks", json!({})).unwrap()["asks"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    id
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
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(800.)));
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
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

/// The row of the request's card.
fn ask_row(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<usize> {
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .position(|block| matches!(block, Block::VaultAsk(_)))
    })
}

#[gpui_kit::test]
fn the_card_of_a_vault_ask_is_prefilled_masked_and_folds_once_set(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(&id), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the card", |cx| {
        let page = session_page(&shell, cx);
        ask_row(&page, cx).is_some_and(|at| has(format!("row-{at}-vault-ask-secret"), cx))
    });
    let page = session_page(&shell, cx);
    let at = ask_row(&page, cx).unwrap();

    // It unfolds into the conversation: it moves before it rests.
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the card enters without moving"
    );
    settle(cx);

    // Prefilled by Flottille, the agent's reason beside it, each with its visible label.
    assert_eq!(
        label(format!("row-{at}-vault-ask-name"), cx).as_deref(),
        Some("Entry: jira-time")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-project"), cx).as_deref(),
        Some("Project: app")
    );
    assert!(
        label(format!("row-{at}-vault-ask-what"), cx).is_some_and(|l| l.contains("Jira")),
        "what it is for"
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-why"), cx),
        Some(format!("Why: {WHY}"))
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-set"), cx).as_deref(),
        Some("Set")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-dismiss"), cx).as_deref(),
        Some("Dismiss")
    );
    assert!(
        has(format!("row-{at}-vault-ask-username"), cx),
        "Jira takes a username"
    );

    let fields = page
        .read_with(cx, |page, cx| {
            page.conversation().read(cx).rows()[at]
                .read(cx)
                .vault_ask()
                .cloned()
        })
        .expect("the card's fields");
    let (secret, username) =
        fields.read_with(cx, |f, _| (f.secret().clone(), f.username().clone()));
    assert!(
        secret.read_with(cx, |s, _| s.presentation().is_masked()),
        "the secret's field is masked"
    );
    secret.update_in(cx, |field, window, cx| field.set_value(SECRET, window, cx));
    username.update_in(cx, |field, window, cx| {
        field.set_value("dev@example.com", window, cx)
    });
    fields.update_in(cx, |fields, window, cx| fields.set(window, cx));
    assert_eq!(
        secret.read_with(cx, |s, _| s.value().to_string()),
        "",
        "the secret leaves the field at once"
    );

    until(
        &shell,
        cx,
        "the entry is set and the card folds away",
        |cx| {
            settle(cx);
            // No longer needing the user, it folds away into its turn's group.
            !has(format!("row-{at}-vault-ask-secret"), cx)
        },
    );
    let answered = page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks()[at].clone()
    });
    assert!(
        matches!(answered, Block::VaultAsk(ref ask) if !ask.open),
        "{answered:?}"
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
    let entries = user.call("vault.list", json!({})).unwrap();
    assert_eq!(entries["entries"][0]["name"], "jira-time");
    assert_eq!(entries["entries"][0]["project"], "app");
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

#[gpui_kit::test]
fn the_card_refuses_a_name_another_project_uses_until_it_is_edited(cx: &mut TestAppContext) {
    let user = user();
    let id = session_with(&user, true);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(&id), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the card", |cx| {
        let page = session_page(&shell, cx);
        ask_row(&page, cx).is_some_and(|at| has(format!("row-{at}-vault-ask-secret"), cx))
    });
    let page = session_page(&shell, cx);
    let at = ask_row(&page, cx).unwrap();
    settle(cx);

    // The name line says it, with the sentence the daemon refuses with.
    assert_eq!(
        label(format!("row-{at}-vault-ask-name"), cx).as_deref(),
        Some("Entry: `jira-time` is used by the project other: choose another name")
    );
    assert!(
        has(format!("row-{at}-vault-ask-new-name"), cx),
        "a name to edit"
    );

    let fields = page
        .read_with(cx, |page, cx| {
            page.conversation().read(cx).rows()[at]
                .read(cx)
                .vault_ask()
                .cloned()
        })
        .expect("the card's fields");
    let (secret, username, name) = fields.read_with(cx, |f, _| {
        (f.secret().clone(), f.username().clone(), f.name().clone())
    });
    secret.update_in(cx, |field, window, cx| field.set_value(SECRET, window, cx));
    username.update_in(cx, |field, window, cx| {
        field.set_value("dev@example.com", window, cx)
    });

    // Unedited, Set does nothing: the secret stays in its field and nothing reached the vault.
    fields.update_in(cx, |fields, window, cx| fields.set(window, cx));
    assert_eq!(secret.read_with(cx, |s, _| s.value().to_string()), SECRET);
    let entries = user.call("vault.list", json!({})).unwrap();
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1, "{entries}");
    assert_eq!(entries["entries"][0]["project"], "other");

    // Edited, it sets the entry under the new name and leaves the other project's alone.
    name.update_in(cx, |field, window, cx| {
        field.set_value("app-jira-time", window, cx)
    });
    fields.update_in(cx, |fields, window, cx| fields.set(window, cx));
    until(&shell, cx, "the card folds away", |cx| {
        settle(cx);
        !has(format!("row-{at}-vault-ask-secret"), cx)
    });
    let entries = user.call("vault.list", json!({})).unwrap();
    let find = |name: &str| {
        entries["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("no {name}: {entries}"))
            .clone()
    };
    assert_eq!(find("jira-time")["project"], "other");
    assert_eq!(find("app-jira-time")["project"], "app");
    std::fs::write(user.path("fake/release"), "").unwrap();
}
