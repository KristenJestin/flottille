//! Signing the agents and the forge programs in from Settings, Agents (#1185, `docs/agents.md`,
//! "Signing in", `docs/reference/gpui.md`): a row says each program's state, Sign in opens the
//! program's own flow in a dialog (the address, opened with this machine's handler, and the code
//! with a copy button, or a hidden field for a code to paste), Cancel stops it, Sign out is asked
//! once, and a program with no flow outside a terminal says the line to type. On GPUI's test
//! platform in front of a real daemon in temporary folders, its programs fakes: no window opens
//! on a screen and no real program runs.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn user(programs: &[&str]) -> User {
    let user = User::with_fake_providers().with_sign_in_programs(programs);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
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

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .filter(|found| found.visible())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

/// Settings, on its Agents tab, where the programs' rows are.
fn open_settings(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the window is ready", |cx| {
        visible(cx, "settings")
    });
    click(cx, "settings");
    until(shell, cx, "the settings page is shown", |cx| {
        visible(cx, "settings-tabs-agents")
    });
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

fn logouts(user: &User, program: &str) -> Vec<Value> {
    user.sign_in_record(program)
        .into_iter()
        .filter_map(|entry| entry.get("logout").cloned())
        .collect()
}

#[gpui_kit::test]
fn sign_in_shows_the_address_and_the_code_and_ends_signed_in(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    user.sign_in_script(
        "gh",
        json!({"signed_in": false,
               "sign_in": {"address": "https://github.com/login/device", "code": "ABCD-1234"}}),
    );
    let (shell, cx) = window(&user, cx);
    let opened = opened_links(&shell, cx);
    open_settings(&shell, cx);

    // The section of the forges and trackers holds gh's row, not signed in.
    until(&shell, cx, "gh's row says it is not signed in", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Not signed in")
    });
    assert!(visible(cx, "forges"), "the Forges and trackers section");
    click(cx, "sign-in-gh");

    until(
        &shell,
        cx,
        "the dialog shows the address and the code",
        |cx| {
            visible(cx, "sign-in-modal")
                && label(cx, "sign-in-address").as_deref()
                    == Some("https://github.com/login/device")
                && label(cx, "sign-in-code").as_deref() == Some("ABCD-1234")
        },
    );
    click(cx, "sign-in-code-copy");
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("ABCD-1234".to_owned())
    );
    click(cx, "sign-in-address");
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://github.com/login/device")],
        "opened with this machine's handler"
    );

    // The user finishes in their browser: the dialog closes and the row says it.
    user.approve_sign_in("gh");
    until(&shell, cx, "gh's row says it is signed in", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Signed in as someone")
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-modal"), "the dialog closed");
}

#[gpui_kit::test]
fn a_pasted_code_is_written_once_and_never_kept(cx: &mut TestAppContext) {
    const PASTED: &str = "pasted-code-of-a-test-0042";
    let user = user(&["claude"]);
    user.sign_in_script(
        "claude",
        json!({"signed_in": false, "sign_in": {"accepts": PASTED}}),
    );
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    until(&shell, cx, "Claude's row offers to sign in", |cx| {
        visible(cx, "sign-in-claude")
    });
    click(cx, "sign-in-claude");
    until(&shell, cx, "the dialog asks for the code", |cx| {
        visible(cx, "sign-in-pasted")
    });

    click(cx, "sign-in-pasted");
    cx.simulate_input(PASTED);
    click(cx, "sign-in-send-code");

    until(&shell, cx, "Claude's row says it is signed in", |cx| {
        label(cx, "sign-in-state-claude").as_deref() == Some("Signed in as someone@example.com")
    });
    let pasted: Vec<Value> = user
        .sign_in_record("claude")
        .into_iter()
        .filter(|entry| entry.get("pasted").is_some() || entry.get("after").is_some())
        .collect();
    assert_eq!(pasted, [json!({"pasted": PASTED})]);
}

#[gpui_kit::test]
fn cancel_stops_the_sign_in(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    user.sign_in_script("gh", json!({"signed_in": false}));
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    until(&shell, cx, "gh's row offers to sign in", |cx| {
        visible(cx, "sign-in-gh")
    });
    click(cx, "sign-in-gh");
    until(&shell, cx, "the dialog shows the code", |cx| {
        label(cx, "sign-in-code").is_some()
    });

    click(cx, "sign-in-modal-cancel");

    let pid = user.sign_in_record("gh")[0]["pid"].as_u64().unwrap();
    until(&shell, cx, "the sign-in's process ended", |_| {
        !Path::new(&format!("/proc/{pid}")).exists()
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-modal"), "the dialog closed");
    assert_eq!(
        label(cx, "sign-in-state-gh").as_deref(),
        Some("Not signed in")
    );
}

#[gpui_kit::test]
fn sign_out_is_asked_once(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    until(&shell, cx, "gh's row says it is signed in", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Signed in as someone")
    });

    click(cx, "sign-out-gh");
    until(&shell, cx, "the confirmation asks", |cx| {
        visible(cx, "sign-out-action")
    });
    assert_eq!(
        logouts(&user, "gh"),
        Vec::<Value>::new(),
        "nothing before it is confirmed"
    );
    click(cx, "sign-out-action");

    until(&shell, cx, "gh's row says it is signed out", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Not signed in")
    });
    assert_eq!(
        logouts(&user, "gh"),
        [json!(["auth", "logout", "--hostname", "github.com"])]
    );
}

#[gpui_kit::test]
fn a_program_without_a_flow_says_the_line_to_type(cx: &mut TestAppContext) {
    let user = user(&[]);
    let opencode = user.path("programs/opencode");
    std::fs::write(&opencode, "#!/bin/sh\necho 1.19.0\n").unwrap();
    let mut permissions = std::fs::metadata(&opencode).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&opencode, permissions).unwrap();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    until(&shell, cx, "OpenCode's row says the line to type", |cx| {
        label(cx, "sign-in-line-opencode").as_deref() == Some("opencode auth login")
    });
    assert!(
        !visible(cx, "sign-in-opencode"),
        "no Sign in without a flow"
    );
    // A program that is not installed says so, and offers nothing.
    assert_eq!(
        label(cx, "sign-in-state-acli").as_deref(),
        Some("Not installed")
    );
    assert!(!visible(cx, "sign-in-acli"));
}
