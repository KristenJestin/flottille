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
use gpui_kit::test::TestWindowExt;
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
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        // The window's clock goes on: what unfolds and fades in gets there.
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Lets every motion end: the window's clock goes on until it asks for no animation frame over
/// half a second of it, so that a row clicked is where it stays.
fn settle(cx: &mut VisualTestContext) {
    let mut quiet = 0;
    for _ in 0..1000 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        quiet = if asked == 0 { quiet + 1 } else { 0 };
        if quiet >= 30 {
            return;
        }
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
    settle(cx);
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
    assert!(
        visible(cx, "sign-in-row-gh"),
        "gh is a row of the Forges and trackers section"
    );
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
        label(cx, "sign-in-state-gh").as_deref() == Some("Signed in as s***")
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-modal"), "the dialog closed");
}

/// #1248: a program that says its code in no way the daemon can read is never shown a wrong one:
/// the dialog says to finish in the browser with the code the program shows.
#[gpui_kit::test]
fn a_dialog_with_no_code_found_says_to_use_the_code_the_program_shows(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    user.sign_in_script(
        "gh",
        json!({"signed_in": false,
               "sign_in": {"address": "https://github.com/login/device",
                           "code": "ABCD-1234", "wording": "no code"}}),
    );
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    until(&shell, cx, "gh's row says it is not signed in", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Not signed in")
    });
    click(cx, "sign-in-gh");

    until(
        &shell,
        cx,
        "the dialog says there is no code to show",
        |cx| {
            label(cx, "sign-in-address").as_deref() == Some("https://github.com/login/device")
                && label(cx, "sign-in-no-code").is_some()
        },
    );
    assert_eq!(
        label(cx, "sign-in-no-code").as_deref(),
        Some("Finish in your browser with the code the program shows.")
    );
    assert!(!visible(cx, "sign-in-code"), "no code is made up");
    user.call("signin.cancel", json!({"program": "gh"})).ok();
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
        label(cx, "sign-in-state-claude").as_deref() == Some("Signed in as s***@e***")
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
    until(&shell, cx, "gh's row says it is not signed in", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Not signed in")
    });
}

#[gpui_kit::test]
fn sign_out_is_asked_once(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    until(&shell, cx, "gh's row says it is signed in", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Signed in as s***")
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
    let opencode = user.path("sign-in-bin/opencode");
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
    // A program that is not installed says so, once its check ended, and offers nothing.
    until(&shell, cx, "acli's row says it is not installed", |cx| {
        label(cx, "sign-in-state-acli").as_deref() == Some("Not installed")
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-acli"));
}

/// What a row said each time the window was looked at, from the first look.
fn watched(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    program: &str,
    until_said: &str,
) -> Vec<String> {
    let seen = std::cell::RefCell::new(Vec::new());
    until(shell, cx, what, |cx| {
        let said = label(cx, &format!("sign-in-state-{program}"));
        if let Some(said) = &said
            && seen.borrow().last() != Some(said)
        {
            seen.borrow_mut().push(said.clone());
        }
        said.as_deref() == Some(until_said)
    });
    seen.into_inner()
}

/// How many times the fake of `program` was asked for its status.
fn statuses(user: &User, program: &str) -> usize {
    std::fs::read_to_string(user.path(&format!("sign-in-{program}/received.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("\"status\"") || line.contains("\"list\""))
        .count()
}

#[gpui_kit::test]
fn a_row_says_checking_until_its_result_arrives(cx: &mut TestAppContext) {
    let user = user(&["gh", "acli"]);
    user.sign_in_script("gh", json!({"delay_ms": 2500}));
    user.sign_in_script("acli", json!({"signed_in": false}));
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    let seen = watched(
        &shell,
        cx,
        "gh's row says its result",
        "gh",
        "Signed in as s***",
    );

    // Never a guessed state, and never the last result as if it were fresh: it checks, then it says.
    assert_eq!(seen, ["Checking…", "Signed in as s***"], "{seen:?}");
}

#[gpui_kit::test]
fn a_row_being_checked_offers_nothing_and_its_check_again_waits(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    user.sign_in_script("gh", json!({"delay_ms": 2500}));
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    until(&shell, cx, "gh's row says it checks", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Checking…")
    });

    assert!(!visible(cx, "sign-in-gh"), "no Sign in while it checks");
    assert!(!visible(cx, "sign-out-gh"), "no Sign out while it checks");
    until(&shell, cx, "the daemon runs gh's check", |_| {
        statuses(&user, "gh") == 1
    });
    let asked = statuses(&user, "gh");
    cx.update(|window, cx| window.click("sign-in-check-gh".to_owned(), cx));
    settle(cx);
    assert_eq!(statuses(&user, "gh"), asked, "its Check again waited");

    until(&shell, cx, "gh's row says its result", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Signed in as s***")
    });
}

/// The vertical place of each forge row, top first, when all of them are drawn.
fn forge_order(cx: &mut VisualTestContext) -> Option<Vec<&'static str>> {
    let programs = ["gh", "acli", "bkt", "glab"];
    let mut found = Vec::new();
    for program in programs {
        let id = format!("sign-in-state-{program}");
        let y = cx.update(|window, _| {
            window
                .try_find(id)
                .map(|found| f32::from(found.bounds().origin.y))
        })?;
        found.push((y, program));
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(found.into_iter().map(|(_, program)| program).collect())
}

#[gpui_kit::test]
fn the_rows_keep_their_order_when_results_arrive(cx: &mut TestAppContext) {
    let user = user(&["gh", "acli", "bkt", "glab"]);
    // The last to be asked answers first, the first last: results arrive out of order.
    user.sign_in_script("gh", json!({"delay_ms": 2500}));
    user.sign_in_script("acli", json!({"delay_ms": 1200}));
    user.sign_in_script("bkt", json!({"delay_ms": 400, "signed_in": false}));
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    let orders = std::cell::RefCell::new(Vec::new());
    until(&shell, cx, "every row has its result", |cx| {
        if let Some(order) = forge_order(cx) {
            orders.borrow_mut().push(order);
        }
        label(cx, "sign-in-state-gh").as_deref() == Some("Signed in as s***")
    });

    let orders = orders.into_inner();
    assert!(!orders.is_empty());
    for order in orders {
        assert_eq!(order, ["gh", "acli", "bkt", "glab"]);
    }
}

#[gpui_kit::test]
fn a_cli_that_says_it_is_not_signed_in_shows_sign_in(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    user.sign_in_script("gh", json!({"signed_in": false}));
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    until(&shell, cx, "gh's row says it is not signed in", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Not signed in")
    });

    until(&shell, cx, "Sign in is offered", |cx| {
        visible(cx, "sign-in-gh")
    });
    assert!(visible(cx, "sign-in-check-gh"));
    assert!(!visible(cx, "sign-in-reason-gh"));
}

#[gpui_kit::test]
fn a_cli_that_cannot_reach_its_service_shows_could_not_check_with_its_reason(
    cx: &mut TestAppContext,
) {
    let user = user(&["bkt"]);
    user.sign_in_script(
        "bkt",
        json!({"status_fail": "Error: 503 Service Unavailable (api.bitbucket.org)"}),
    );
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    until(&shell, cx, "bkt's row says it could not check", |cx| {
        label(cx, "sign-in-state-bkt").as_deref() == Some("Could not check: bkt failed")
    });

    until(&shell, cx, "the reason is under the row", |cx| {
        label(cx, "sign-in-reason-bkt").as_deref()
            == Some("Error: 503 Service Unavailable (api.bitbucket.org)")
    });

    // Check again, once the service is back: the row says it is signed in.
    user.sign_in_script("bkt", json!({}));
    click(cx, "sign-in-check-bkt");
    until(&shell, cx, "bkt's row says it is signed in", |cx| {
        label(cx, "sign-in-state-bkt").as_deref() == Some("Signed in")
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-reason-bkt"));
}

#[gpui_kit::test]
fn a_failed_check_never_offers_to_sign_in_again(cx: &mut TestAppContext) {
    let user = user(&["gh", "acli"]);
    user.sign_in_script(
        "gh",
        json!({"status_fail": "error connecting to api.github.com"}),
    );
    user.sign_in_script(
        "acli",
        json!({"status_fail": "✗ Error: Something went wrong"}),
    );
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    for program in ["gh", "acli"] {
        until(&shell, cx, "the row says it could not check", |cx| {
            label(cx, &format!("sign-in-state-{program}"))
                .is_some_and(|said| said.starts_with("Could not check"))
        });
    }
    settle(cx);

    for program in ["gh", "acli"] {
        assert!(!visible(cx, &format!("sign-in-{program}")), "{program}");
        assert!(!visible(cx, &format!("sign-out-{program}")), "{program}");
        assert!(
            visible(cx, &format!("sign-in-check-{program}")),
            "{program}"
        );
    }
}

#[gpui_kit::test]
fn a_row_being_checked_waits_behind_the_kit_s_skeleton_not_a_progress_bar(cx: &mut TestAppContext) {
    let user = user(&["gh"]);
    user.sign_in_script("gh", json!({"delay_ms": 2500}));
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    until(&shell, cx, "gh's row says it checks", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Checking…")
    });
    until(&shell, cx, "the skeleton is drawn", |cx| {
        visible(cx, "sign-in-checking-gh-skeleton")
    });
    // The state's text stays; a progress bar is for transfers.
    assert_eq!(label(cx, "sign-in-state-gh").as_deref(), Some("Checking…"));
    assert!(!visible(cx, "sign-in-checking-gh"), "no progress bar");

    until(&shell, cx, "gh's row says its result", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Signed in as s***")
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-checking-gh-skeleton"));
}

#[gpui_kit::test]
fn a_terminal_line_is_offered_only_to_a_program_known_not_to_be_signed_in(cx: &mut TestAppContext) {
    let user = user(&["bkt"]);
    user.sign_in_script("bkt", json!({"delay_ms": 2500}));
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    // While it checks, nothing is said of how to sign in.
    until(&shell, cx, "bkt's row says it checks", |cx| {
        label(cx, "sign-in-state-bkt").as_deref() == Some("Checking…")
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-line-bkt"), "no line while it checks");

    until(&shell, cx, "bkt's row says its result", |cx| {
        label(cx, "sign-in-state-bkt").as_deref() == Some("Signed in")
    });

    // Known not to be signed in: the line to type is there.
    user.sign_in_script("bkt", json!({"signed_in": false}));
    click(cx, "sign-in-check-bkt");
    until(&shell, cx, "bkt's row says it is not signed in", |cx| {
        label(cx, "sign-in-state-bkt").as_deref() == Some("Not signed in")
    });
    until(&shell, cx, "the line to type", |cx| {
        visible(cx, "sign-in-line-bkt")
    });

    // Could not be checked: it says why, and offers no line.
    user.sign_in_script(
        "bkt",
        json!({"status_fail": "Error: 503 Service Unavailable (api.bitbucket.org)"}),
    );
    click(cx, "sign-in-check-bkt");
    until(&shell, cx, "bkt's row says it could not check", |cx| {
        label(cx, "sign-in-state-bkt").as_deref() == Some("Could not check: bkt failed")
    });
    settle(cx);
    assert!(!visible(cx, "sign-in-line-bkt"), "no line after a failure");
}
