//! The card of a request for a vault entry in the window (#566, #947): an agent asks the user for
//! an entry; the card shows what was prefilled, the host the secret goes to and the agent's
//! reason, unfolds into the conversation, opens Settings, Vault's own form prefilled, and folds
//! away once the user saved the entry. On GPUI's test platform in front of a real daemon whose agent is
//! the fake ACP agent, with the in-memory keyring; no window opens on a screen.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use flottille_window::vault::VaultSection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::base::input::InputState;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SECRET: &str = "w0rklog-t0ken-from-the-form";
const WHY: &str = "to log the two hours spent on ABC-1";
const STAGING: &str = "to sign in to staging";
const BANK: &str = "to sign in to the bank";
const SHOP: &str = "to sign in to the shop";
const NOTES: &str = "to read the notes";

fn user() -> User {
    let user = User::with_fake_agent().with_fake_acli();
    user.fake_script(json!({"turns": [
        {"when": "log my time", "do": [
            {"mcp": {"tool": "vault", "arguments":
                {"action": "ask", "name": "jira-time", "why": WHY}}},
            {"say": "Asked for the token."},
            {"hold": "release"}]},
        {"when": "sign in to staging", "do": [
            {"mcp": {"tool": "vault", "arguments":
                {"action": "ask", "name": "staging-login", "why": STAGING,
                 "url": "https://staging.example.com:8443/login?next=/",
                 "username": "tester@example.com"}}},
            {"say": "Asked for the login."},
            {"hold": "release"}]},
        {"when": "sign in to the bank", "do": [
            {"mcp": {"tool": "vault", "arguments":
                {"action": "ask", "name": "bank-login", "why": BANK,
                 "url": "https://evil.example\\@bank.example/"}}},
            {"say": "Asked for the bank."},
            {"hold": "release"}]},
        {"when": "sign in to the shop", "do": [
            {"mcp": {"tool": "vault", "arguments":
                {"action": "ask", "name": "shop-test", "why": SHOP,
                 "url": "https://shop.example.com/login", "for": "connection:browser"}}},
            {"say": "Asked for the shop."},
            {"hold": "release"}]},
        {"when": "read the notes", "do": [
            {"mcp": {"tool": "vault", "arguments":
                {"action": "ask", "name": "notes-key", "why": NOTES,
                 "for": "connection:notes"}}},
            {"say": "Asked for the notes."},
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

/// A session of `app`, whose Jira tracker names the missing entry `jira-time`, told `message`:
/// its turn holds until the test releases it, so the card stays in sight while it is answered.
fn session(user: &User, message: &str) -> String {
    if message.contains("shop") {
        connection(user, "browser");
    }
    if message.contains("notes") {
        connection(user, "notes");
        // Its server is given the entry when it starts, once for every project's agents.
        user.call(
            "connection.edit",
            json!({"name": "notes", "env": {"NOTES_KEY": {"vault": "notes-key"}}}),
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
        .call("agent.run", json!({"project": "app", "message": message}))
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
    cx.update(|cx| flottille_window::init_with(env, cx));
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

fn vault_form(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<VaultSection> {
    // Over the page shown, whichever it is: the form is a dialog of the window.
    shell.read_with(cx, |s, cx| s.vault(cx))
}

fn value(
    vault: &Entity<VaultSection>,
    field: fn(&VaultSection) -> Option<&Entity<InputState>>,
    cx: &mut VisualTestContext,
) -> String {
    let field = vault
        .read_with(cx, |v, _| field(v).cloned())
        .expect("the form's field");
    field.read_with(cx, |f, _| f.value().to_string())
}

/// Types the secret in the open form and saves it; once the daemon closed the request, back in
/// the session `id`, the card folds away.
fn save(
    user: &User,
    shell: &Entity<Shell>,
    vault: &Entity<VaultSection>,
    (id, at): (&str, u64),
    cx: &mut VisualTestContext,
) {
    let secret = vault
        .read_with(cx, |v, _| v.secret_field().cloned())
        .unwrap();
    assert!(
        secret.read_with(cx, |s, _| s.presentation().is_masked()),
        "the secret's field is masked"
    );
    cx.update(|window, cx| secret.update(cx, |f, cx| f.set_value(SECRET, window, cx)));
    cx.update(|window, cx| window.click("save-vault-entry", cx));
    until(shell, cx, "the entry is saved", |cx| {
        if let Some(why) = vault.read_with(cx, |v, _| v.form_error().map(str::to_owned)) {
            panic!("the form refused: {why}");
        }
        user.call("vault.asks", json!({})).unwrap()["asks"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the card folds away", |cx| {
        settle(cx);
        // The page shown again, whichever entity draws it.
        let Some(page) = shell.read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        else {
            cx.update(|window, cx| window.click(session_id(id), cx));
            return false;
        };
        page.read_with(cx, |page, cx| {
            page.conversation()
                .read(cx)
                .blocks()
                .iter()
                .any(|block| matches!(block, Block::VaultAsk(ask) if !ask.open()))
        }) && !has(format!("vault-ask-{at}-add"), cx)
    });
}

#[gpui_kit::test]
fn the_card_of_a_vault_ask_opens_the_vault_form_prefilled(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);

    // What the feature prefilled and the agent's reason, each with its visible label.
    assert_eq!(
        label(format!("vault-ask-{at}-name"), cx).as_deref(),
        Some("Entry: jira-time")
    );
    assert_eq!(
        label(format!("vault-ask-{at}-project"), cx).as_deref(),
        Some("Project: app")
    );
    // What is for and who asked are under Details.
    cx.update(|window, cx| window.click(format!("vault-ask-{at}-details"), cx));
    settle(cx);
    assert_eq!(
        label(format!("vault-ask-{at}-for"), cx).as_deref(),
        Some("For: the worklog of the project app")
    );
    assert!(
        label(format!("vault-ask-{at}-source"), cx)
            .is_some_and(|said| said.starts_with("asked by the conversation")),
        "who asked"
    );
    assert_eq!(
        label(format!("vault-ask-{at}-why"), cx),
        Some(format!("Why: {WHY}"))
    );
    assert_eq!(
        label(format!("vault-ask-{at}-add"), cx).as_deref(),
        Some("New entry")
    );
    assert_eq!(
        label(format!("vault-ask-{at}-dismiss"), cx).as_deref(),
        Some("Dismiss")
    );
    // No field of its own: the card is a way in to the vault's form.
    assert!(!has(format!("vault-ask-{at}-secret"), cx));

    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);
    let vault = vault_form(&shell, cx);
    assert!(vault.read_with(cx, |v, _| v.is_open()), "the form is open");
    assert_eq!(value(&vault, VaultSection::name_field, cx), "jira-time");
    assert!(
        value(&vault, VaultSection::description_field, cx).contains("Jira"),
        "what it is for"
    );
    // The agent's reason is the hint of what it is for.
    assert_eq!(
        label("vault-description-field-hint", cx),
        Some(format!("Why the agent asks: {WHY}"))
    );
    cx.update(|window, cx| {
        vault
            .read(cx)
            .username_field()
            .unwrap()
            .clone()
            .update(cx, |f, cx| f.set_value("dev@example.com", window, cx))
    });
    save(&user, &shell, &vault, (&id, at), cx);

    std::fs::write(user.path("fake/release"), "").unwrap();
    let set = user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(set["name"], "jira-time");
    assert_eq!(set["project"], "app");
    assert_eq!(
        set["for"], "worklog:app",
        "bound as the feature asks: {set}"
    );
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

#[gpui_kit::test]
fn the_card_shows_the_host_the_secret_goes_to(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to staging");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);

    // The host on its own line, without the port or the path the agent wrote.
    assert_eq!(
        label(format!("vault-ask-{at}-host"), cx).as_deref(),
        Some("Goes to: staging.example.com")
    );
    assert_eq!(
        label(format!("vault-ask-{at}-name"), cx).as_deref(),
        Some("Entry: staging-login")
    );
    assert_eq!(
        label(format!("vault-ask-{at}-why"), cx),
        Some(format!("Why: {STAGING}"))
    );

    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);
    let vault = vault_form(&shell, cx);
    assert_eq!(value(&vault, VaultSection::name_field, cx), "staging-login");
    assert_eq!(
        value(&vault, VaultSection::url_field, cx),
        "https://staging.example.com:8443/login?next=/"
    );
    assert_eq!(
        value(&vault, VaultSection::username_field, cx),
        "tester@example.com"
    );
    // The user may change any field before saving.
    cx.update(|window, cx| {
        vault
            .read(cx)
            .url_field()
            .unwrap()
            .clone()
            .update(cx, |f, cx| {
                f.set_value("https://staging.example.com/login", window, cx)
            })
    });
    save(&user, &shell, &vault, (&id, at), cx);

    std::fs::write(user.path("fake/release"), "").unwrap();
    let set = user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(set["name"], "staging-login");
    assert_eq!(set["url"], "https://staging.example.com/login");
    assert_eq!(set["project"], "app");
}

/// The connection `name` declared on the fake MCP server, never reviewed: it exists, and gives
/// no tool.
fn connection(user: &User, name: &str) {
    let fake = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(fake.exists(), "`cargo build -p flottille-fake-mcp` first");
    let script = user.path(&format!("{name}-script.json"));
    std::fs::write(
        &script,
        json!({"server": {"name": name, "version": "1.0.0"}, "tools": []}).to_string(),
    )
    .unwrap();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    user.call(
        "connection.add",
        json!({"name": name, "about": format!("The {name} of the tests."),
            "command": [fake, "--script", script]}),
    )
    .unwrap();
}

#[gpui_kit::test]
fn an_ask_with_a_backslash_url_shows_the_host_login_would_use(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to the bank");
    // A web address reads a backslash as "/": the host is evil.example, whatever the text after it says.
    let asked = user.call("vault.asks", json!({})).unwrap()["asks"][0].clone();
    assert_eq!(asked["host"], "evil.example", "{asked}");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);

    assert_eq!(
        label(format!("vault-ask-{at}-host"), cx).as_deref(),
        Some("Goes to: evil.example")
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn the_vault_form_shows_the_binding_and_its_host(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to the shop");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);

    // A login's entry: the login reads it for the agent's project, so it is that project's.
    assert_eq!(
        label(format!("vault-ask-{at}-project"), cx).as_deref(),
        Some("Project: app")
    );
    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);
    let vault = vault_form(&shell, cx);
    assert!(vault.read_with(cx, |v, _| v.is_open()), "the form is open");
    assert_eq!(
        vault.read_with(cx, |v, _| v.project().map(str::to_owned)),
        Some("app".to_owned()),
        "Seen by agents of: the project, as the card says"
    );
    assert!(!has("vault-connection-note", cx), "no rule to explain");
    // What the entry will be bound to, and where its secret goes, said in the form itself.
    assert_eq!(
        label("vault-binding-for", cx).as_deref(),
        Some("For: the connection browser")
    );
    assert_eq!(
        label("vault-goes-to", cx).as_deref(),
        Some("Goes to: shop.example.com")
    );
    assert_eq!(
        label("vault-binding-remove", cx).as_deref(),
        Some("Remove the binding")
    );

    // The user may remove the binding: it folds away, moving, and the entry is saved bound to
    // no use.
    cx.update(|window, cx| window.click("vault-binding-remove", cx));
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the binding leaves without moving"
    );
    settle(cx);
    assert!(!has("vault-binding-for", cx), "still shown");
    save(&user, &shell, &vault, (&id, at), cx);

    std::fs::write(user.path("fake/release"), "").unwrap();
    let set = user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(set["name"], "shop-test");
    assert_eq!(set["for"], serde_json::Value::Null, "{set}");
}

#[gpui_kit::test]
fn the_vault_form_says_where_an_unbound_secret_goes(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to the bank");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);

    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);
    let vault = vault_form(&shell, cx);
    assert!(vault.read_with(cx, |v, _| v.is_open()), "the form is open");
    // The entry is bound to no use, and the URL's text reads like bank.example: the form, where
    // the secret is typed, says the host as login reads it.
    assert!(!has("vault-binding-for", cx), "the entry is unbound");
    assert_eq!(
        value(&vault, VaultSection::url_field, cx),
        "https://evil.example\\@bank.example/"
    );
    assert_eq!(
        label("vault-goes-to", cx).as_deref(),
        Some("Goes to: evil.example")
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn the_goes_to_line_folds_with_the_url(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to the bank");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);

    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);
    let vault = vault_form(&shell, cx);
    let url = vault
        .read_with(cx, |v, _| v.url_field().cloned())
        .expect("the URL's field");
    let asked = url.read_with(cx, |f, _| f.value().to_string());
    assert!(
        has("vault-goes-to", cx),
        "said while the URL is the one asked"
    );

    // Cleared, the line folds away, moving; it is no longer the URL the host was read from.
    cx.update(|window, cx| url.update(cx, |f, cx| f.set_value("", window, cx)));
    cx.run_until_parked();
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the line leaves without moving"
    );
    settle(cx);
    assert!(!has("vault-goes-to", cx), "still shown");

    // Another URL says no host the daemon did not read.
    cx.update(|window, cx| {
        url.update(cx, |f, cx| {
            f.set_value("https://other.example/", window, cx)
        })
    });
    settle(cx);
    assert!(!has("vault-goes-to", cx), "said of another URL");

    // The URL asked, typed again, brings it back, unfolding.
    cx.update(|window, cx| url.update(cx, |f, cx| f.set_value(asked, window, cx)));
    cx.run_until_parked();
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the line comes without moving"
    );
    settle(cx);
    assert_eq!(
        label("vault-goes-to", cx).as_deref(),
        Some("Goes to: evil.example")
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn dismiss_is_disabled_while_its_call_runs(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to staging");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);
    let number = user.call("vault.asks", json!({})).unwrap()["asks"][0]["ask"]
        .as_u64()
        .unwrap();

    // A second click before the daemon answered sends nothing more.
    cx.update(|window, cx| window.click(format!("vault-ask-{at}-dismiss"), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(
        shell.read_with(cx, |s, _| s.is_dismissing(number)),
        "Dismiss is not disabled"
    );
    cx.update(|window, cx| window.click(format!("vault-ask-{at}-dismiss"), cx));
    until(&shell, cx, "the request is dismissed", |_| {
        user.call("vault.asks", json!({})).unwrap()["asks"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    for _ in 0..20 {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        settle(cx);
    }
    let toasts: Vec<String> = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    });
    assert!(
        !toasts.iter().any(|t| t.contains("not dismissed")),
        "a second dismissal was sent: {toasts:?}"
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn the_new_entry_form_does_not_offer_a_project_it_would_refuse(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "read the notes");
    let (shell, cx) = window(&user, cx);
    let (_, at) = waiting_card(&user, &shell, &id, cx);
    // The card says whose entry it will be, as the form saves it.
    assert_eq!(
        label(format!("vault-ask-{at}-project"), cx).as_deref(),
        Some("Project: For the connection notes (every project)")
    );

    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);
    let vault = vault_form(&shell, cx);
    assert_eq!(
        vault.read_with(cx, |v, _| v.project().map(str::to_owned)),
        None
    );
    let why = label("vault-connection-note", cx).unwrap_or_default();
    assert!(
        why.contains("notes") && why.contains("every project"),
        "said under the field before anything is typed: {why}"
    );
    // The project's choice, which Add would refuse, is not offered.
    cx.update(|window, cx| window.click("vault-project", cx));
    settle(cx);
    assert!(has("vault-project-every", cx), "every project is offered");
    assert!(!has("vault-project-app", cx), "the project is offered");
    cx.update(|window, cx| window.click("vault-project-every", cx));
    settle(cx);
    save(&user, &shell, &vault, (&id, at), cx);
    std::fs::write(user.path("fake/release"), "").unwrap();
    let set = user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(set["name"], "notes-key");
    assert_eq!(set["project"], serde_json::Value::Null, "{set}");

    // Edited later from Settings, Vault: still not offered, and said why.
    cx.update(|window, cx| window.click("settings", cx));
    settle(cx);
    cx.update(|window, cx| window.click("settings-tabs-vault", cx));
    until(&shell, cx, "the entry is listed", |cx| {
        has("vault-notes-key", cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click("vault-notes-key", cx));
    settle(cx);
    assert!(vault.read_with(cx, |v, _| v.is_open()), "the form is open");
    let why = label("vault-connection-note", cx).unwrap_or_default();
    assert!(why.contains("notes"), "{why}");
    cx.update(|window, cx| window.click("vault-project", cx));
    settle(cx);
    assert!(has("vault-project-every", cx));
    assert!(
        !has("vault-project-app", cx),
        "the project is offered on edit"
    );
}

// ── A dialog opens over the page it is asked from (#1018) ─────────────────────────────────────

/// The vault's section, whichever page is shown: the form is a dialog of the window.
fn the_vault(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<VaultSection> {
    shell.read_with(cx, |s, cx| s.vault(cx))
}

fn selected(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Selection> {
    shell.read_with(cx, |s, _| s.selected().cloned())
}

#[gpui_kit::test]
fn new_entry_on_a_vault_ask_opens_over_the_session(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (page, at) = waiting_card(&user, &shell, &id, cx);
    assert_eq!(selected(&shell, cx), Some(Selection::Session(id.clone())));

    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);

    // The form is open, drawn, and nothing else was shown: the session and its card are behind it.
    let vault = the_vault(&shell, cx);
    assert!(vault.read_with(cx, |v, _| v.is_open()), "the form is open");
    assert!(
        cx.update(|window, _| window.try_find("vault-modal").is_some_and(|f| f.visible())),
        "the form is drawn"
    );
    assert_eq!(
        selected(&shell, cx),
        Some(Selection::Session(id.clone())),
        "the window stays on the session"
    );
    assert_eq!(
        session_page(&shell, cx),
        page,
        "the session's page is the one shown"
    );
    assert!(
        has(format!("vault-ask-{at}-add"), cx),
        "its card is still behind"
    );
    assert_eq!(
        shell.read_with(cx, |s, cx| s.modal_stack(cx)),
        ["vault-modal"]
    );
    assert_eq!(value(&vault, VaultSection::name_field, cx), "jira-time");
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn saving_the_entry_from_a_request_answers_it_and_leaves_the_page_as_it_was(
    cx: &mut TestAppContext,
) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (page, at) = waiting_card(&user, &shell, &id, cx);
    cx.update(|window, cx| window.click(format!("vault-ask-{at}-add"), cx));
    settle(cx);
    let vault = the_vault(&shell, cx);
    let secret = vault
        .read_with(cx, |v, _| v.secret_field().cloned())
        .expect("the form's secret");
    cx.update(|window, cx| secret.update(cx, |f, cx| f.set_value(SECRET, window, cx)));
    let username = vault
        .read_with(cx, |v, _| v.username_field().cloned())
        .expect("the form's username");
    cx.update(|window, cx| username.update(cx, |f, cx| f.set_value("dev@example.com", window, cx)));
    cx.update(|window, cx| window.click("save-vault-entry", cx));
    until(&shell, cx, "the request is answered", |cx| {
        if let Some(why) = vault.read_with(cx, |v, _| v.form_error().map(str::to_owned)) {
            panic!("the form refused: {why}");
        }
        user.call("vault.asks", json!({})).unwrap()["asks"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    // The daemon answered the request when it stored the entry; the window learns it from the
    // reply to its write, on a later poll: the form closes then, not before.
    until(&shell, cx, "the form closes", |cx| {
        !vault.read_with(cx, |v, _| v.is_open())
    });
    settle(cx);

    // No click on the session was needed to be back on it: it was never left.
    assert_eq!(selected(&shell, cx), Some(Selection::Session(id.clone())));
    assert_eq!(session_page(&shell, cx), page);
    assert!(
        !vault.read_with(cx, |v, _| v.is_open()),
        "the form is closed"
    );
    assert!(
        shell.read_with(cx, |s, cx| s.modal_stack(cx)).is_empty(),
        "no dialog is left open"
    );
    until(&shell, cx, "the card folds away", |cx| {
        settle(cx);
        !has(format!("vault-ask-{at}-add"), cx)
    });
    let set = user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(set["name"], "jira-time", "{set}");
    std::fs::write(user.path("fake/release"), "").unwrap();
}

/// The number of the request the daemon holds open.
fn open_ask(user: &User) -> u64 {
    user.call("vault.asks", json!({})).unwrap()["asks"][0]["ask"]
        .as_u64()
        .expect("an open request")
}

/// Opens the session `id` and waits for the request's card in "Waiting for you": its number.
fn waiting_card(
    user: &User,
    shell: &Entity<Shell>,
    id: &str,
    cx: &mut VisualTestContext,
) -> (Entity<SessionPage>, u64) {
    let number = open_ask(user);
    until(shell, cx, "the session is in the tree", |cx| {
        has(session_id(id), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the card of Waiting for you", |cx| {
        has(format!("vault-ask-{number}-add"), cx)
    });
    until(shell, cx, "the conversation's line", |cx| {
        line_of(&session_page(shell, cx), cx).is_some()
    });
    settle(cx);
    (session_page(shell, cx), number)
}

/// The conversation's row for the request: the line that records it.
fn line_of(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<String> {
    let at = ask_row(page, cx)?;
    label(format!("row-{at}-vault-ask-line"), cx)
}

#[gpui_kit::test]
fn a_vault_ask_is_a_card_of_waiting_for_you_not_of_the_conversation(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (page, number) = waiting_card(&user, &shell, &id, cx);

    // Its entry, project and why are lines of the card; New entry and Dismiss are its two buttons.
    assert_eq!(
        label(format!("vault-ask-{number}-name"), cx).as_deref(),
        Some("Entry: jira-time")
    );
    assert_eq!(
        label(format!("vault-ask-{number}-project"), cx).as_deref(),
        Some("Project: app")
    );
    assert_eq!(
        label(format!("vault-ask-{number}-why"), cx),
        Some(format!("Why: {WHY}"))
    );
    assert_eq!(
        label(format!("vault-ask-{number}-add"), cx).as_deref(),
        Some("New entry")
    );
    assert_eq!(
        label(format!("vault-ask-{number}-dismiss"), cx).as_deref(),
        Some("Dismiss")
    );
    assert!(has("rail-waiting", cx), "the section is shown");

    // The conversation holds no card of it: no line of the card, no button.
    let at = ask_row(&page, cx).expect("the conversation records the ask");
    for part in ["add", "dismiss", "name", "project", "why"] {
        assert!(
            !has(format!("row-{at}-vault-ask-{part}"), cx),
            "the conversation draws {part}"
        );
    }
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn the_conversation_says_a_vault_ask_was_made_without_a_button(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (page, _) = waiting_card(&user, &shell, &id, cx);

    assert_eq!(
        line_of(&page, cx).as_deref(),
        Some("asked for the vault entry jira-time · waiting for you")
    );
    let at = ask_row(&page, cx).unwrap();
    assert!(!has(format!("row-{at}-vault-ask-add"), cx), "no button");
    assert!(!has(format!("row-{at}-vault-ask-dismiss"), cx), "no button");
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn saving_the_entry_answers_the_card_and_it_leaves(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (_, number) = waiting_card(&user, &shell, &id, cx);

    cx.update(|window, cx| window.click(format!("vault-ask-{number}-add"), cx));
    settle(cx);
    let vault = the_vault(&shell, cx);
    let secret = vault
        .read_with(cx, |v, _| v.secret_field().cloned())
        .expect("the form's secret");
    cx.update(|window, cx| secret.update(cx, |f, cx| f.set_value(SECRET, window, cx)));
    let username = vault
        .read_with(cx, |v, _| v.username_field().cloned())
        .expect("the form's username");
    cx.update(|window, cx| username.update(cx, |f, cx| f.set_value("dev@example.com", window, cx)));
    cx.update(|window, cx| window.click("save-vault-entry", cx));
    until(&shell, cx, "the card leaves", |cx| {
        settle(cx);
        !has(format!("vault-ask-{number}-add"), cx)
    });
    assert_eq!(
        line_of(&session_page(&shell, cx), cx).as_deref(),
        Some("asked for the vault entry jira-time · saved")
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn dismissing_a_vault_ask_leaves_the_card_and_says_it_in_the_conversation(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (_, number) = waiting_card(&user, &shell, &id, cx);

    cx.update(|window, cx| window.click(format!("vault-ask-{number}-dismiss"), cx));
    until(&shell, cx, "the card leaves", |cx| {
        settle(cx);
        !has(format!("vault-ask-{number}-add"), cx)
    });
    assert_eq!(
        line_of(&session_page(&shell, cx), cx).as_deref(),
        Some("asked for the vault entry jira-time · dismissed")
    );
    assert!(
        !has("rail-waiting", cx),
        "nothing waits: the section folds away"
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn a_vault_ask_marks_the_session_as_waiting_in_the_sidebar(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);

    // The session is not opened: its row says it waits for the user, and the bell rings.
    until(&shell, cx, "the row says it waits", |cx| {
        shell.read_with(cx, |shell, cx| {
            shell.session(&id).and_then(|row| row.read(cx).mark())
        }) == Some(flottille_window::state::AgentState::NeedsYou)
    });
    until(&shell, cx, "the bell rings", |cx| {
        shell.read_with(cx, |shell, cx| shell.bell_sessions(cx).len()) == 1
    });
    std::fs::write(user.path("fake/release"), "").unwrap();
}
