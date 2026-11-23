//! The findings in the Developer section of Settings (`docs/developer.md`, "The findings"): a
//! list of what agents reported about Flottille itself, with its count, first and last time and
//! whether it is filed, and a button that exports it to a file. On GPUI's test platform in front
//! of a real daemon in temporary folders. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_window::Shell;
use flottille_window::settings::SettingsPage;
use gpui_design_system::motion::PANEL;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

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

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn open_developer(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    until(shell, cx, "the general tab is there", |cx| {
        present(cx, "settings-tabs-general")
    });
    click(cx, "settings-tabs-general");
    until(shell, cx, "the developer mode is read", |cx| {
        page.read_with(cx, |p, _| p.developer().is_some())
    });
    page
}

/// A project `app` and the session of an agent of it, which reports findings.
fn agent(user: &User) -> Mcp {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn report(mcp: &Mcp, title: &str) {
    let answer = mcp.call(
        "services",
        json!({"action": "report", "title": title, "called": "tickets list",
            "expected": "the open tickets", "happened": "an empty answer"}),
    );
    assert!(!answer.error, "{}", answer.text);
}

fn user_with_findings(titles: &[&str]) -> (User, Mcp) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let output = user.run(&["developer", "on"]);
    assert!(output.status.success());
    let mcp = agent(&user);
    for title in titles {
        report(&mcp, title);
    }
    (user, mcp)
}

#[gpui_kit::test]
fn the_developer_section_lists_the_findings(cx: &mut TestAppContext) {
    let (user, mcp) = user_with_findings(&["Tickets list answers empty"]);
    mcp.call(
        "services",
        json!({"action": "report", "finding": 1, "called": "tickets list", "happened": "empty",
            "differs": "after a retry"}),
    );
    let output = user.run(&[
        "developer",
        "reports",
        "filed",
        "1",
        "https://example.com/acme/app/issues/12",
    ]);
    assert!(output.status.success());
    mcp.call(
        "services",
        json!({"action": "report", "title": "A refusal that makes no sense",
            "called": "fs_write", "expected": "a write", "happened": "refused"}),
    );

    let (shell, cx) = window(&user, cx);
    let page = open_developer(&shell, cx);
    until(&shell, cx, "the findings are listed", |cx| {
        present(cx, "finding-2")
    });
    settle(cx);

    let titles: Vec<String> = page.read_with(cx, |p, _| {
        p.findings().iter().map(|f| f.title.clone()).collect()
    });
    assert_eq!(
        titles,
        [
            "A refusal that makes no sense",
            "Tickets list answers empty"
        ]
    );
    assert!(present(cx, "finding-1"), "its row");
    assert_eq!(
        label(cx, "finding-1-count").as_deref(),
        Some("2 occurrences")
    );
    assert!(present(cx, "finding-1-first"), "its first time");
    assert!(present(cx, "finding-1-last"), "its last time");
    assert_eq!(
        label(cx, "finding-1-filed").as_deref(),
        Some("Filed: https://example.com/acme/app/issues/12")
    );
    assert_eq!(
        label(cx, "finding-2-count").as_deref(),
        Some("1 occurrence")
    );
    assert_eq!(label(cx, "finding-2-filed").as_deref(), Some("Not filed"));
}

#[gpui_kit::test]
fn the_export_button_writes_the_findings_to_a_file(cx: &mut TestAppContext) {
    let (user, _mcp) = user_with_findings(&["Tickets list answers empty"]);
    let (shell, cx) = window(&user, cx);
    let page = open_developer(&shell, cx);
    until(&shell, cx, "the findings are listed", |cx| {
        present(cx, "finding-1")
    });
    settle(cx);

    click(cx, "findings-export");

    let file = user.path("state/flottille/findings.json");
    let shown = |cx: &mut VisualTestContext| -> Vec<String> {
        shell.read_with(cx, |s, cx| {
            s.toasts()
                .read(cx)
                .toasts()
                .map(|t| format!("{} / {:?}", t.title(), t.second_line()))
                .collect()
        })
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while !file.exists() {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(
            Instant::now() < deadline,
            "the file was never written; toasts: {:?}; exporting: {}; findings: {}; button: {:?}",
            shown(cx),
            page.read_with(cx, |p, _| p.exporting()),
            page.read_with(cx, |p, _| p.findings().len()),
            cx.update(|window, _| window.find("findings-export").bounds())
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let exported: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        exported["findings"][0]["title"],
        "Tickets list answers empty"
    );
    // A result is a toast: the file's path is its second line.
    until(&shell, cx, "a toast says where", |cx| {
        shell.read_with(cx, |s, cx| {
            s.toasts()
                .read(cx)
                .toasts()
                .find(|toast| toast.title() == "Findings exported")
                .and_then(|toast| toast.second_line().map(str::to_owned))
                .as_deref()
                == file.to_str()
        })
    });
}

#[gpui_kit::test]
fn a_finding_that_arrives_unfolds_and_fades_in_and_the_list_folds_with_the_mode(
    cx: &mut TestAppContext,
) {
    let (user, mcp) = user_with_findings(&["Tickets list answers empty"]);
    let (shell, cx) = window(&user, cx);
    open_developer(&shell, cx);
    until(&shell, cx, "the first finding is listed", |cx| {
        present(cx, "finding-1")
    });
    settle(cx);
    let height = |cx: &mut VisualTestContext| {
        cx.update(|window, _| window.find("findings-item-2").bounds().size.height)
    };

    report(&mcp, "A refusal that makes no sense");
    until(&shell, cx, "the new finding is there", |cx| {
        present(cx, "finding-2")
    });
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let start = height(cx);
    cx.executor().advance_clock(PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let part = height(cx);
    settle(cx);
    let whole = height(cx);

    assert!(
        start < whole,
        "it is not there at once: {start:?} of {whole:?}"
    );
    assert!(part > start && part < whole, "unfolding: {part:?}");

    // The list belongs to the mode: off, it folds away.
    click(cx, "developer-switch");
    until(&shell, cx, "the list folds away", |cx| {
        !present(cx, "finding-1")
    });
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

#[gpui_kit::test]
fn a_finding_is_marked_filed_with_its_address(cx: &mut TestAppContext) {
    let (user, _mcp) = user_with_findings(&[
        "Tickets list answers empty",
        "A refusal that makes no sense",
    ]);
    let (shell, cx) = window(&user, cx);
    let page = open_developer(&shell, cx);
    until(&shell, cx, "the findings are listed", |cx| {
        present(cx, "finding-1") && present(cx, "finding-2")
    });
    settle(cx);
    let order = |page: &Entity<SettingsPage>, cx: &mut VisualTestContext| -> Vec<u64> {
        page.read_with(cx, |p, _| p.findings().iter().map(|f| f.id).collect())
    };
    // The newest first, both open.
    assert_eq!(order(&page, cx), [2, 1]);

    // Mark filed… is in the finding's ⋯, and asks the address in a dialog over the page.
    click(cx, "finding-2-more");
    assert!(present(cx, "finding-2-mark-filed"));
    click(cx, "finding-2-mark-filed");
    until(&shell, cx, "the dialog opens", |cx| {
        present(cx, "finding-filed-modal")
    });
    assert_eq!(
        label(cx, "finding-filed-address-field-label").as_deref(),
        Some("Address of its issue")
    );
    // No address, nothing is sent: the dialog says so under its field.
    click(cx, "finding-filed-save");
    assert!(present(cx, "finding-filed-modal"));
    let reports = user.call("developer.reports", json!({})).unwrap();
    assert!(
        reports["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["filed"].is_null())
    );

    let address = page.read_with(cx, |p, cx| p.filing().read(cx).address().clone());
    cx.update(|window, cx| {
        address.update(cx, |field, cx| {
            field.set_value("https://example.com/acme/app/issues/12", window, cx)
        })
    });
    click(cx, "finding-filed-save");
    until(&shell, cx, "the daemon answers", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|toast| toast == "Finding #2 is marked filed")
    });
    // The dialog folds away over its motion.
    settle(cx);
    assert!(!present(cx, "finding-filed-modal"), "the dialog closes");
    // The daemon holds what `developer reports filed` writes.
    let reports = user.call("developer.reports", json!({})).unwrap();
    let filed = reports["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["id"] == 2)
        .unwrap()["filed"]
        .clone();
    assert_eq!(filed, "https://example.com/acme/app/issues/12");
    // Its toast, and the finding leaves the open ones: the row says where it went.
    until(&shell, cx, "the toast says it", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|toast| toast == "Finding #2 is marked filed")
    });
    until(&shell, cx, "the finding leaves the open list", |cx| {
        order(&page, cx) == [1, 2]
    });
    settle(cx);
    assert_eq!(
        label(cx, "finding-2-filed").as_deref(),
        Some("Filed: https://example.com/acme/app/issues/12")
    );
    // A filed finding is filed: it has no ⋯ to offer it again.
    assert!(!present(cx, "finding-2-more"));
}
