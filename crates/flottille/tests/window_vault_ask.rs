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

/// Opens the session `id` and waits for its request's card: its row.
fn card(
    shell: &Entity<Shell>,
    id: &str,
    cx: &mut VisualTestContext,
) -> (Entity<SessionPage>, usize) {
    until(shell, cx, "the session is in the tree", |cx| {
        has(session_id(id), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the card", |cx| {
        let page = session_page(shell, cx);
        ask_row(&page, cx).is_some_and(|at| has(format!("row-{at}-vault-ask-add"), cx))
    });
    let page = session_page(shell, cx);
    let at = ask_row(&page, cx).unwrap();
    // It unfolds into the conversation: it moves before it rests.
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the card enters without moving"
    );
    settle(cx);
    (page, at)
}

fn vault_form(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<VaultSection> {
    shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .expect("Settings is shown")
            .read(cx)
            .vault()
            .clone()
    })
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
    (id, at): (&str, usize),
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
                .any(|block| matches!(block, Block::VaultAsk(ask) if !ask.open))
        }) && !has(format!("row-{at}-vault-ask-add"), cx)
    });
}

#[gpui_kit::test]
fn the_card_of_a_vault_ask_opens_the_vault_form_prefilled(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "log my time");
    let (shell, cx) = window(&user, cx);
    let (_, at) = card(&shell, &id, cx);

    // What the feature prefilled and the agent's reason, each with its visible label.
    assert_eq!(
        label(format!("row-{at}-vault-ask-name"), cx).as_deref(),
        Some("Entry: jira-time")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-project"), cx).as_deref(),
        Some("Project: app")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-for"), cx).as_deref(),
        Some("For: the worklog of the project app")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-why"), cx),
        Some(format!("Why: {WHY}"))
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-add"), cx).as_deref(),
        Some("New entry")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-dismiss"), cx).as_deref(),
        Some("Dismiss")
    );
    // No field of its own: the card is a way in to the vault's form.
    assert!(!has(format!("row-{at}-vault-ask-secret"), cx));

    cx.update(|window, cx| window.click(format!("row-{at}-vault-ask-add"), cx));
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
    let (_, at) = card(&shell, &id, cx);

    // The host on its own line, without the port or the path the agent wrote.
    assert_eq!(
        label(format!("row-{at}-vault-ask-host"), cx).as_deref(),
        Some("Goes to: staging.example.com")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-name"), cx).as_deref(),
        Some("Entry: staging-login")
    );
    assert_eq!(
        label(format!("row-{at}-vault-ask-why"), cx),
        Some(format!("Why: {STAGING}"))
    );

    cx.update(|window, cx| window.click(format!("row-{at}-vault-ask-add"), cx));
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
    let (_, at) = card(&shell, &id, cx);

    assert_eq!(
        label(format!("row-{at}-vault-ask-host"), cx).as_deref(),
        Some("Goes to: evil.example")
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[gpui_kit::test]
fn the_vault_form_shows_the_binding_and_its_host(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to the shop");
    let (shell, cx) = window(&user, cx);
    let (_, at) = card(&shell, &id, cx);

    cx.update(|window, cx| window.click(format!("row-{at}-vault-ask-add"), cx));
    settle(cx);
    let vault = vault_form(&shell, cx);
    assert!(vault.read_with(cx, |v, _| v.is_open()), "the form is open");
    // What the entry will be bound to, and where its secret goes, said in the form itself.
    assert_eq!(
        label("vault-binding-for", cx).as_deref(),
        Some("For: the connection browser")
    );
    assert_eq!(
        label("vault-binding-host", cx).as_deref(),
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
fn dismiss_is_disabled_while_its_call_runs(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "sign in to staging");
    let (shell, cx) = window(&user, cx);
    let (_, at) = card(&shell, &id, cx);
    let number = user.call("vault.asks", json!({})).unwrap()["asks"][0]["ask"]
        .as_u64()
        .unwrap();

    // A second click before the daemon answered sends nothing more.
    cx.update(|window, cx| window.click(format!("row-{at}-vault-ask-dismiss"), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(
        shell.read_with(cx, |s, _| s.is_dismissing(number)),
        "Dismiss is not disabled"
    );
    cx.update(|window, cx| window.click(format!("row-{at}-vault-ask-dismiss"), cx));
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
