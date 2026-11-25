//! The scenarios of #1262: Allow on a held launch whose setup copies files the receiving machine
//! lacks makes each file a vault entry on the caller's machine, shares it, then allows the launch
//! (`docs/remote.md`, "What travels"). On GPUI's test platform in front of real daemons, each with
//! its own temporary HOME and XDG folders, in-memory vault, fake agents and no keyring, and a hub
//! served in the test on 127.0.0.1: the window signs with the device key file of its machine's key
//! folder. Throwaway repositories. No window opens on a screen.

use crate::common;
use crate::remote::{Machine, NoKeyring, TestHub, pc_and_server_on};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::{Connect, Sign};
use flottille_window::model::elsewhere_id;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// How long a link, a launch, a share or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

/// The text of the caller's ignored `.env` and `config/local.toml`: secrets, which nothing of
/// Flottille's may show.
const ENV: &str = "DATABASE_URL=postgres://robin:s3cret-pass@db.example.test/ledger\n";
const LOCAL: &str = "[mail]\npassword = \"tulip-harbour-417\"\n";
const SECRETS: [&str; 2] = ["s3cret-pass", "tulip-harbour-417"];

/// A user whose agents are the fake agent, Claude enabled and signed in (its login file only
/// there, never read), offers looked at often.
fn runner() -> User {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_OFFER_MS", "300")
        .with_env("ANTHROPIC_API_KEY", "")
        .with_env("CLAUDE_CODE_OAUTH_TOKEN", "")
        .with_env("ANTHROPIC_AUTH_TOKEN", "");
    let claude = user.path("home/.claude");
    let user = user.with_env("CLAUDE_CONFIG_DIR", claude.to_str().unwrap());
    user.write_providers("[claude]\nenabled = true\n");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join(".credentials.json"), "{}").unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A hub, `pc` its first member with the project `app` on `feat/export`, whose setup copies its ignored `.env`
/// and `config/local.toml`, and `server` a runner paired with it, which has no `app`; the two
/// linked, `server` offering Claude.
fn pc_and_server() -> (TestHub, Machine, Machine) {
    let (hub, pc, server) = pc_and_server_on(Machine::of(runner()), runner());
    let repo = pc.user.path("work/app");
    pc.user.git_init(&repo);
    std::fs::write(repo.join(".gitignore"), ".env\nconfig/local.toml\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    pc.user.git(&repo, &["add", "."]);
    pc.user.git(&repo, &["commit", "--quiet", "-m", "first"]);
    pc.user
        .git(&repo, &["checkout", "--quiet", "-b", "feat/export"]);
    std::fs::write(repo.join(".env"), ENV).unwrap();
    std::fs::create_dir_all(repo.join("config")).unwrap();
    std::fs::write(repo.join("config/local.toml"), LOCAL).unwrap();
    let text = format!(
        "folder = \"{}\"\nworkspaces_root = \"{}\"\nbranch_prefix = \"\"\n\
         [[setup]]\ncopy = [\".env\", \"config/local.toml\"]\n\
         [[repositories]]\npath = \".\"\nbase = [\"main\"]\n",
        repo.display(),
        pc.user.path("work/app-work").display()
    );
    pc.user.write_project_file("app", &text);
    pc.user.call("project.list", json!({})).unwrap();
    eventually("server offers Claude", || {
        pc.remote()["machines"].as_array().unwrap().iter().any(|m| {
            m["name"] == "server"
                && m["offers"]["agents"]
                    .as_array()
                    .is_some_and(|agents| agents.iter().any(|agent| agent == "claude"))
        })
    });
    (hub, pc, server)
}

/// A session the user starts in `app` on `pc`, its first turn ended, in Auto: its MCP session.
fn root(pc: &Machine) -> Mcp {
    let opened = pc.user.received_requests("session/new").len();
    let started = pc
        .user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(&pc.user, opened);
    eventually("the root's first turn", || {
        pc.user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    pc.user
        .call("agent.mode", json!({"agent": id, "mode": "auto"}))
        .unwrap();
    Mcp::initialize(&url, &authorization)
}

/// A session led by a root of `pc` launched on `server`, held there because its setup lays the
/// two files: its id there and its request's number.
fn held_on_server(pc: &Machine, server: &Machine) -> (String, u64) {
    let mcp = root(pc);
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "led", "machine": "server", "title": "Ledger",
            "brief": "Ledger, whole."}),
    );
    assert!(!answer.error, "{}", answer.text);
    let deadline = Instant::now() + WAIT;
    loop {
        let requests = server.user.call("approval.list", json!({})).unwrap();
        let request = &requests["requests"][0];
        if let (Some(agent), Some(number)) = (request["agent"].as_str(), request["number"].as_u64())
        {
            assert_eq!(request["call"]["needs"]["from"], "pc", "{request:#}");
            return (agent.to_owned(), number);
        }
        assert!(
            Instant::now() < deadline,
            "the launch was never held on server"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pending(server: &Machine) -> Vec<Value> {
    server.user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn entries(machine: &Machine) -> Vec<String> {
    machine.user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
        .collect()
}

/// The folder of `server`'s session `agent`.
fn folder_of(server: &Machine, agent: &str) -> PathBuf {
    let shown = server
        .user
        .call("agent.show", json!({"agent": agent}))
        .unwrap();
    PathBuf::from(shown["agent"]["folder"].as_str().unwrap())
}

fn window<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: machine.user.path("os-release"),
        state_home: machine.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(machine.user.connector());
    let device = Some(Device::in_folder(
        machine.user.path("state/flottille/remote"),
    ));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    shell.update(cx, |s, _| s.sign_with(signing(machine)));
    (shell, cx)
}

/// How the window signs, as its device would: with the device key file of `machine`'s key folder.
fn signing(machine: &Machine) -> Sign {
    let folder = machine.folder();
    Arc::new(move |target, request| {
        let key = DeviceKey::load(&NoKeyring, &folder)
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .ok_or_else(|| std::io::Error::other("no device key"))?;
        let signed = Request::sign(
            &key,
            &request.id,
            target,
            &request.method,
            &request.params,
            SystemTime::now(),
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        serde_json::to_value(signed).map_err(std::io::Error::other)
    })
}

/// One step of the window as the platform would run it.
fn step(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    common::unfold_pilots(shell, cx);
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(shell, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..40 {
        step(shell, cx);
    }
}

/// The window's clock moved on by `by`, frame after frame, nothing asked of the daemons.
fn advance(cx: &mut VisualTestContext, by: Duration) {
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += Duration::from_millis(16);
    }
}

fn id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

fn label(cx: &mut VisualTestContext, name: String) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id(name))
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Every label the window says, as text.
fn every_label(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| {
        window
            .elements()
            .into_iter()
            .filter_map(|element| element.label().map(str::to_owned))
            .collect()
    })
}

/// The page of the window's machine's session `id`, open.
fn open_page(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    until(shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(id)).is_some())
    });
    settle(shell, cx);
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the session's page", |cx| {
        page_of(shell, cx).is_some()
    });
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<SessionPage>> {
    shell.read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
}

/// The card of the request `number`, its files listed: what each of its steps says.
fn card_said(cx: &mut VisualTestContext, number: u64) -> (Vec<Option<String>>, Option<String>) {
    let words = (0..2)
        .map(|at| label(cx, format!("allow-word-{number}-{at}")))
        .collect();
    (words, label(cx, format!("allow-word-{number}-approve")))
}

/// The cards of "Waiting for you" scrolled down over the card `number`, as the user scrolls to its
/// buttons once it grew past the section's height.
fn scroll_cards_down(shell: &Entity<Shell>, number: u64, cx: &mut VisualTestContext) {
    let allow = cx.update(|window, _| {
        window
            .try_find(id(format!("allow-{number}")))
            .map(|found| found.bounds())
    });
    let Some(allow) = allow else { return };
    // Low in the section, which ends at the window's bottom.
    let over = gpui_kit::point(allow.center().x, px(760.));
    for _ in 0..4 {
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: over,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-400.))),
            ..Default::default()
        });
        step(shell, cx);
    }
}

/// The details of the card `number` unfolded, as the user opens them.
fn open_details(shell: &Entity<Shell>, number: u64, cx: &mut VisualTestContext) {
    scroll_cards_down(shell, number, cx);
    cx.update(|window, cx| window.click(id(format!("approval-details-{number}")), cx));
    settle(shell, cx);
    let toggle = id(format!("approval-details-{number}"));
    assert_eq!(
        cx.update(|window, _| window.find(toggle).expanded()),
        Some(true),
        "the details are open"
    );
}

/// What the card `number` says of the entries Deny leaves, its details open.
fn deny_leaves(shell: &Entity<Shell>, number: u64, cx: &mut VisualTestContext) -> Option<String> {
    settle(shell, cx);
    label(cx, format!("approval-shared-stay-{number}"))
}

const DENY_LEAVES: &str =
    "Deny leaves on server the entries already shared: each is bound to its file alone.";

fn word(cx: &mut VisualTestContext, number: u64, at: usize) -> Option<String> {
    label(cx, format!("allow-word-{number}-{at}"))
}

/// The card `number` waits with its files, from `pc` to `server`, and nothing done yet.
fn the_card_lists_its_files(shell: &Entity<Shell>, number: u64, cx: &mut VisualTestContext) {
    until(shell, cx, "the card with its files", |cx| {
        label(cx, format!("allow-files-{number}")).is_some()
    });
    settle(shell, cx);
    assert_eq!(
        label(cx, format!("allow-files-{number}")).as_deref(),
        Some("From pc → to server · app · 2 files")
    );
    assert_eq!(
        label(cx, format!("allow-step-{number}-0")).as_deref(),
        Some(".env app-.env")
    );
    assert_eq!(
        label(cx, format!("allow-step-{number}-1")).as_deref(),
        Some("config/local.toml app-config-local.toml")
    );
    assert_eq!(
        card_said(cx, number),
        (
            vec![Some("Waiting".to_owned()), Some("Waiting".to_owned())],
            Some("Waiting".to_owned())
        )
    );
    assert_eq!(
        label(cx, format!("allow-step-{number}-approve")).as_deref(),
        Some("Allow on server")
    );
}

/// Allow is clicked on the card `number`, and its launch ends allowed: each file made on `pc`,
/// shared with `server` and laid in the session's folder there, and the conversation says so.
fn allowed_whole(
    shell: &Entity<Shell>,
    (pc, server): (&Machine, &Machine),
    (agent, number): (&str, u64),
    cx: &mut VisualTestContext,
) {
    scroll_cards_down(shell, number, cx);
    let allow = cx.update(|window, _| {
        window
            .try_find(id(format!("allow-{number}")))
            .map(|found| (found.visible(), found.bounds()))
    });
    assert!(
        allow.is_some_and(|(visible, _)| visible),
        "Allow is not visible: {allow:?}"
    );
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    until(shell, cx, "the launch allowed on server", |_| {
        pending(server).is_empty()
    });
    for entry in ["app-.env", "app-config-local.toml"] {
        assert!(entries(pc).contains(&entry.to_owned()), "{entry} made");
        assert!(
            entries(server).contains(&entry.to_owned()),
            "{entry} shared"
        );
    }
    let folder = folder_of(server, agent);
    eventually("the files laid on server", || {
        std::fs::read_to_string(folder.join(".env")).is_ok_and(|text| text == ENV)
            && std::fs::read_to_string(folder.join("config/local.toml"))
                .is_ok_and(|text| text == LOCAL)
    });
    // The card is the result: it folds away, the launch allowed.
    until(shell, cx, "the card folded away", |cx| {
        cx.update(|window, _| window.try_find(id(format!("approval-{number}"))).is_none())
    });
}

#[gpui_kit::test]
fn allowing_a_held_launch_that_needs_files_makes_and_shares_them_then_allows(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    // `.env` was made an entry on pc already: it is shared, never made again.
    pc.user
        .call("vault.from_file", json!({"project": "app", "path": ".env"}))
        .unwrap();

    // The window of the machine it is held on, which reaches pc through the link.
    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);
    allowed_whole(&shell, (&pc, &server), (&agent, number), cx);
    // Made once: no second making of what was there.
    assert_eq!(
        pc.log()
            .matches("the vault entry app-.env was made")
            .count(),
        1,
        "{}",
        pc.log()
    );
}

#[gpui_kit::test]
fn a_failed_share_keeps_the_launch_held_and_says_which_file(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    // The server has an entry of its own of that name: it refuses the copy shared from pc.
    server
        .user
        .call(
            "vault.set",
            json!({"name": "app-config-local.toml", "secret": "of-the-server-own"}),
        )
        .unwrap();

    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);
    // Nothing shared yet: Deny would leave nothing.
    open_details(&shell, number, cx);
    assert_eq!(deny_leaves(&shell, number, cx), None);
    scroll_cards_down(&shell, number, cx);
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    until(&shell, cx, "the failure on the card", |cx| {
        label(cx, format!("approval-error-{number}")).is_some()
    });
    settle(&shell, cx);

    let error = label(cx, format!("approval-error-{number}")).unwrap();
    assert!(
        error.starts_with("config/local.toml was not shared with server: "),
        "{error}"
    );
    assert!(error.contains("its own vault entry"), "{error}");
    // What was done stays done, the failed step says so, and nothing was allowed.
    assert_eq!(word(cx, number, 0).as_deref(), Some("Shared"));
    assert_eq!(word(cx, number, 1).as_deref(), Some("Not shared"));
    assert_eq!(
        label(cx, format!("allow-word-{number}-approve")).as_deref(),
        Some("Waiting")
    );
    assert_eq!(pending(&server).len(), 1, "the launch is still held");
    assert_eq!(
        label(cx, format!("allow-{number}")).as_deref(),
        Some("Try again")
    );
    // `.env` is shared: Deny says it stays.
    assert_eq!(
        deny_leaves(&shell, number, cx).as_deref(),
        Some(DENY_LEAVES)
    );
    // Held and failed, the card draws nothing more once its marks and its dot rested.
    advance(cx, Duration::from_secs(7));
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "a frame asked at rest"
    );

    // Once the server's own entry is gone, Try again resumes at that file and allows.
    server
        .user
        .call("vault.remove", json!({"name": "app-config-local.toml"}))
        .unwrap();
    allowed_whole(&shell, (&pc, &server), (&agent, number), cx);
}

/// The bounds of the element `name`, as the last frame drew them.
fn bounds_of(
    cx: &mut VisualTestContext,
    name: String,
) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    cx.update(|window, _| window.try_find(id(name)).map(|found| found.bounds()))
}

/// Whether the element `name` is drawn inside the "Waiting for you" section, the part of the
/// cards the user sees.
fn in_the_section(cx: &mut VisualTestContext, name: String) -> bool {
    let section = bounds_of(cx, "rail-waiting".to_owned());
    let found = cx.update(|window, _| {
        window
            .try_find(id(name))
            .map(|found| (found.visible(), found.bounds()))
    });
    match (section, found) {
        (Some(section), Some((visible, at))) => {
            visible && at.top() >= section.top() && at.bottom() <= section.bottom()
        }
        _ => false,
    }
}

/// A server whose own entry of that name refuses the second file: Allow fails on it, the card
/// listing its error and Try again.
fn server_refusing_the_second_file(server: &Machine) {
    server
        .user
        .call(
            "vault.set",
            json!({"name": "app-config-local.toml", "secret": "of-the-server-own"}),
        )
        .unwrap();
}

#[gpui_kit::test]
fn a_failed_allow_brings_its_error_and_buttons_into_view(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    server_refusing_the_second_file(&server);
    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);
    // The user scrolled to Allow, as the card is taller than the section.
    scroll_cards_down(&shell, number, cx);
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    until(&shell, cx, "the failure on the card", |cx| {
        label(cx, format!("approval-error-{number}")).is_some()
    });
    // The section moves to the error over `PANEL`: it is not at it at once, then it is.
    let mut at = Vec::new();
    for _ in 0..40 {
        advance(cx, Duration::from_millis(16));
        at.extend(bounds_of(cx, format!("allow-{number}")).map(|found| found.top()));
    }
    settle(&shell, cx);
    assert_eq!(
        label(cx, format!("allow-{number}")).as_deref(),
        Some("Try again")
    );
    for part in ["approval-error", "deny", "allow"] {
        assert!(
            in_the_section(cx, format!("{part}-{number}")),
            "{part} is not in view: {:?} in {:?}",
            bounds_of(cx, format!("{part}-{number}")),
            bounds_of(cx, "rail-waiting".to_owned())
        );
    }
    let (first, last) = (at.first().unwrap(), at.last().unwrap());
    assert!(
        at.iter().any(|y| y != first && y != last),
        "the section jumped: {at:?}"
    );
}

/// How far the cards of "Waiting for you" are scrolled.
fn scrolled(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    let page = page_of(shell, cx).expect("the session's page");
    cx.update(|_, cx| {
        let waiting = page.read(cx).waiting(cx);
        waiting.read(cx).scrolled()
    })
}

#[gpui_kit::test]
fn a_successful_allow_scrolls_nothing(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);
    scroll_cards_down(&shell, number, cx);
    let before = scrolled(&shell, cx);
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    // Each step reaches the page: the section stays where the user put it while it goes.
    let deadline = Instant::now() + WAIT;
    while !pending(&server).is_empty() {
        step(&shell, cx);
        assert_eq!(
            scrolled(&shell, cx),
            before,
            "the section scrolled while the steps went"
        );
        assert!(Instant::now() < deadline, "the launch was never allowed");
    }
    for _ in 0..20 {
        advance(cx, Duration::from_millis(16));
        assert_eq!(
            scrolled(&shell, cx),
            before,
            "the section scrolled once allowed"
        );
    }
}

#[gpui_kit::test]
fn the_user_s_own_scroll_after_a_failed_allow_is_left_alone(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    server_refusing_the_second_file(&server);
    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);
    scroll_cards_down(&shell, number, cx);
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    until(&shell, cx, "the failure on the card", |cx| {
        label(cx, format!("approval-error-{number}")).is_some()
    });
    settle(&shell, cx);
    let in_view = scrolled(&shell, cx);

    // The user scrolls back up to read the files: the section stays where they put it.
    let section = bounds_of(cx, "rail-waiting".to_owned()).unwrap();
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: section.center(),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(200.))),
        ..Default::default()
    });
    settle(&shell, cx);
    let moved = scrolled(&shell, cx);
    assert!(
        moved < in_view,
        "the user scrolled up: {moved:?} {in_view:?}"
    );
    advance(cx, Duration::from_secs(2));
    assert_eq!(scrolled(&shell, cx), moved, "the section was brought back");
}

/// A daemon that lists `files` (path and entry) for the card `number`, whatever it listed before:
/// the list the window gets, set on its waiting section as a poll would, with no poll after it.
fn list_files(
    shell: &Entity<Shell>,
    number: u64,
    files: &[(&str, &str)],
    cx: &mut VisualTestContext,
) {
    let waiting = page_of(shell, cx)
        .map(|page| page.read_with(cx, |p, cx| p.waiting(cx)))
        .unwrap();
    waiting.update(cx, |waiting, cx| {
        let mut cards = waiting.cards().to_vec();
        for card in cards.iter_mut().filter(|card| card.number == number) {
            card.needs.as_mut().unwrap().files = files
                .iter()
                .map(|(path, entry)| flottille_window::waiting::NeededFile {
                    path: (*path).to_owned(),
                    entry: (*entry).to_owned(),
                })
                .collect();
        }
        waiting.set(cards, cx);
    });
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_long_file_path_is_shown_whole_on_its_card(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);

    // The consent is read, not glanced at: a path longer than the card is not cut at its start.
    let long = "pkg/web/config/environments/production/secrets/regional/primary/.env.production.local.2026";
    list_files(&shell, number, &[(long, "app-secrets")], cx);

    let (row, path, entry) = cx.update(|window, _| {
        let found = |name: String| window.try_find(id(name)).map(|found| found.bounds());
        (
            found(format!("allow-step-{number}-0")),
            found(format!("allow-step-{number}-0-path")),
            found(format!("allow-step-{number}-0-object")),
        )
    });
    let (row, path, entry) = (
        row.expect("the row"),
        path.expect("the path is an element of its own"),
        entry.expect("the entry"),
    );
    assert!(
        path.size.height > entry.size.height * 1.5,
        "the path wraps over lines: {path:?}, {entry:?}"
    );
    assert!(
        path.left() >= row.left() && path.right() <= row.right(),
        "the path stays inside its row: {path:?} in {row:?}"
    );
    assert!(
        entry.size.width > px(20.),
        "the entry stays readable: {entry:?}"
    );
    let said = label(cx, format!("allow-step-{number}-0-path"));
    assert_eq!(said.as_deref(), Some(long), "the whole path");
}

#[gpui_kit::test]
fn a_card_whose_files_change_starts_its_steps_again(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    // The server has an entry of its own of that name: `.env` is shared, `config/local.toml` not.
    server
        .user
        .call(
            "vault.set",
            json!({"name": "app-config-local.toml", "secret": "of-the-server-own"}),
        )
        .unwrap();
    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);
    scroll_cards_down(&shell, number, cx);
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    until(&shell, cx, "the failure on the card", |cx| {
        label(cx, format!("approval-error-{number}")).is_some()
    });
    settle(&shell, cx);
    assert_eq!(word(cx, number, 0).as_deref(), Some("Shared"));
    assert_eq!(
        label(cx, format!("allow-{number}")).as_deref(),
        Some("Try again")
    );

    // The daemon lists one file more: no step is past its end, and none is shared yet.
    list_files(
        &shell,
        number,
        &[
            (".env", "app-.env"),
            ("config/local.toml", "app-config-local.toml"),
            ("extra.toml", "app-extra.toml"),
        ],
        cx,
    );
    for at in 0..3 {
        assert_eq!(
            word(cx, number, at).as_deref(),
            Some("Waiting"),
            "file {at}"
        );
    }
    assert_eq!(
        label(cx, format!("allow-word-{number}-approve")).as_deref(),
        Some("Waiting")
    );
    assert_eq!(
        label(cx, format!("allow-{number}")).as_deref(),
        Some("Allow")
    );

    // Other files: nothing of the files listed before is said shared.
    list_files(&shell, number, &[("other.env", "app-other.env")], cx);
    assert_eq!(word(cx, number, 0).as_deref(), Some("Waiting"));
    assert_eq!(word(cx, number, 1), None, "one file listed");
    assert_eq!(
        label(cx, format!("allow-{number}")).as_deref(),
        Some("Allow")
    );
    assert_eq!(pending(&server).len(), 1, "the launch is still held");
}

#[gpui_kit::test]
fn allow_never_shares_a_hand_made_entry_of_the_file_s_name(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    // On pc, `app-.env` is an entry the user made by hand: another secret, no file's.
    pc.user
        .call(
            "vault.set",
            json!({"name": "app-.env", "secret": "a-hand-made-secret-of-pc"}),
        )
        .unwrap();

    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);
    scroll_cards_down(&shell, number, cx);
    scroll_cards_down(&shell, number, cx);
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    until(&shell, cx, "the refusal on the card", |cx| {
        label(cx, format!("approval-error-{number}")).is_some()
    });
    settle(&shell, cx);

    let error = label(cx, format!("approval-error-{number}")).unwrap();
    assert!(error.starts_with(".env was not made: "), "{error}");
    assert!(error.contains("holds another secret"), "{error}");
    assert_eq!(word(cx, number, 0).as_deref(), Some("Not made"));
    assert_eq!(word(cx, number, 1).as_deref(), Some("Waiting"));
    assert!(
        !entries(&server).contains(&"app-.env".to_owned()),
        "the hand-made entry never left pc"
    );
    assert_eq!(pending(&server).len(), 1, "the launch is still held");
    assert_eq!(deny_leaves(&shell, number, cx), None);
}

#[gpui_kit::test]
fn it_works_when_the_window_shows_another_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);

    // The window of pc, the files' machine, showing server's page of the held session.
    let (shell, cx) = window(&pc, cx);
    let row = elsewhere_id("server", &agent);
    until(&shell, cx, "the row of the session on server", |cx| {
        cx.update(|window, _| window.try_find(session_id(&row)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&row), cx));
    until(&shell, cx, "server's page of the session", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown().as_deref() == Some("server"))
            && page_of(&shell, cx).is_some()
    });
    the_card_lists_its_files(&shell, number, cx);
    allowed_whole(&shell, (&pc, &server), (&agent, number), cx);
}

#[gpui_kit::test]
fn no_secret_is_in_an_event_a_log_or_the_window(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (agent, number) = held_on_server(&pc, &server);
    let (shell, cx) = window(&server, cx);
    open_page(&shell, &agent, cx);
    the_card_lists_its_files(&shell, number, cx);

    // Every label the window drew while it made, shared and allowed.
    cx.update(|window, cx| window.click(id(format!("allow-{number}")), cx));
    let mut said = Vec::new();
    let deadline = Instant::now() + WAIT;
    while !pending(&server).is_empty() {
        step(&shell, cx);
        said.extend(every_label(cx));
        assert!(Instant::now() < deadline, "the launch was never allowed");
    }
    settle(&shell, cx);
    said.extend(every_label(cx));
    let events = |machine: &Machine| {
        let events = machine
            .user
            .watch(&["--after", "0"])
            .within(Duration::from_secs(1));
        serde_json::to_string(&events).unwrap()
    };
    for secret in SECRETS {
        assert!(
            !said.iter().any(|text| text.contains(secret)),
            "the window said {secret}"
        );
        for (name, machine) in [("pc", &pc), ("server", &server)] {
            assert!(!machine.log().contains(secret), "{name}'s log has {secret}");
            assert!(
                !events(machine).contains(secret),
                "{name}'s events have {secret}"
            );
        }
    }
    assert!(
        said.iter()
            .any(|text| text == "app-config-local.toml" || text.contains("app-config-local.toml")),
        "the entries are named"
    );
}
