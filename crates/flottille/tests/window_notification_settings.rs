//! What rings, chosen in the window (#1003): the Notifications tab of Settings (what each kind
//! does, its sound, the volume, the quiet hours), the Notifications tab of a project (its own
//! level for each kind) and the Do not disturb switch of the bell's panel. Each control carries its
//! label, a choice is the daemon's and a change made elsewhere is followed, and what moves is
//! animated. On GPUI's test platform in front of a real daemon in temporary folders. No window
//! opens on a screen.

use crate::common;
use crate::window_notifications::{has, label, project, rect, settle, until, user, window};

use common::User;
use flottille_core::notification_settings::{KINDS, label as kind_label};
use flottille_window::Shell;
use flottille_window::project_settings::ProjectSettingsPage;
use flottille_window::settings::SettingsPage;
use flottille_window::shell::Selection;
use gpui_design_system::motion::PANEL;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn click(id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn settings_of(user: &User) -> Value {
    user.call("notifications.settings", json!({})).unwrap()
}

/// Settings on its Notifications tab, once the page knows what rings.
fn open_tab(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    // The tab is long: tall enough that every control is on screen, as a click needs.
    cx.simulate_resize(size(px(1440.), px(2800.)));
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    until(shell, cx, "the tab is there", |cx| {
        has("settings-tabs-notifications", cx)
    });
    click("settings-tabs-notifications", cx);
    until(shell, cx, "the page knows what rings", |cx| {
        let view = page.read_with(cx, |p, _| p.notifications().clone());
        view.read_with(cx, |view, _| view.shown().is_some())
    });
    settle(cx);
    page
}

/// A project's settings on its Notifications tab, once it knows what rings.
fn open_project_tab(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<ProjectSettingsPage> {
    cx.simulate_resize(size(px(1440.), px(2800.)));
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(shell, cx, "the project's settings are read", |cx| {
        shell
            .read_with(cx, |s, cx| {
                s.main().read(cx).project_settings_page().cloned()
            })
            .is_some_and(|page| page.read_with(cx, |page, _| page.name() == "app"))
    });
    let page = shell
        .read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
        .unwrap();
    click("project-settings-tabs-notifications", cx);
    until(shell, cx, "the project knows what rings", |cx| {
        let view = page.read_with(cx, |p, _| p.notifications().clone());
        view.read_with(cx, |view, _| view.shown().is_some())
    });
    settle(cx);
    page
}

#[gpui_kit::test]
fn the_notifications_tab_names_each_kind_and_what_it_does_and_a_request_has_no_off(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_tab(&shell, cx);

    for kind in KINDS {
        assert_eq!(
            label(format!("notify-kind-{kind}-field-label"), cx).as_deref(),
            Some(kind_label(kind)),
            "{kind}"
        );
        for level in ["ring", "centre"] {
            assert!(
                has(format!("notify-kind-{kind}-{level}"), cx),
                "{kind} {level}"
            );
        }
        // Each switch says whose sound it is, not "Sound" alone.
        assert_eq!(
            label(format!("notify-kind-{kind}-sound-label"), cx),
            Some(format!("{} sound", kind_label(kind))),
            "{kind}"
        );
    }
    // What waits for the user can be lowered to the centre, never turned off.
    for kind in ["request", "question", "vault"] {
        assert!(!has(format!("notify-kind-{kind}-off"), cx), "{kind}");
    }
    assert!(has("notify-kind-report-off", cx));
    for section in ["kinds", "sound", "quiet"] {
        assert!(
            has(format!("notifications-{section}-title"), cx),
            "{section}"
        );
    }
    assert_eq!(
        label("notify-sound-switch-label", cx).as_deref(),
        Some("Sound")
    );
    assert_eq!(
        label("notify-quiet-switch-label", cx).as_deref(),
        Some("Quiet hours")
    );
    assert_eq!(
        label("notify-volume-field-label", cx).as_deref(),
        Some("Volume")
    );
}

/// Whether `outer` holds `inner`.
fn holds(outer: &str, inner: &str, cx: &mut VisualTestContext) -> bool {
    let (outer, inner) = (rect(outer.to_owned(), cx), rect(inner.to_owned(), cx));
    outer.origin.y <= inner.origin.y
        && inner.origin.y + inner.size.height <= outer.origin.y + outer.size.height + px(0.5)
}

#[gpui_kit::test]
fn each_section_s_skeleton_fades_into_what_the_daemon_said(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_tab(&shell, cx);

    // The controls are drawn where the skeleton waited, by what fades the one into the other.
    for section in ["kinds", "sound", "quiet"] {
        let waiting = format!("notify-{section}-waiting");
        assert!(has(waiting.clone(), cx), "{section}");
        assert!(
            holds(&waiting, &format!("notify-{section}"), cx),
            "{section}"
        );
    }
    open_project_tab(&shell, cx);
    assert!(holds("notify-project-waiting", "notify-project", cx));
}

#[gpui_kit::test]
fn a_file_that_cannot_be_read_is_said_in_the_tab_and_the_controls_unfold_once_fixed(
    cx: &mut TestAppContext,
) {
    let user = user();
    let file = user.path("config/flottille/notifications.toml");
    std::fs::write(&file, "[kinds\nreport = ").unwrap();
    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1440.), px(2800.)));
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the tab is there", |cx| {
        has("settings-tabs-notifications", cx)
    });
    click("settings-tabs-notifications", cx);
    until(&shell, cx, "the tab says why", |cx| {
        has("notify-kinds-problem", cx)
    });
    settle(cx);
    // Why comes and goes with its block, in place of the skeleton.
    let why = label("notify-kinds-problem", cx).unwrap_or_default();
    assert!(why.contains("notifications.toml"), "{why}");
    assert!(holds(
        "notify-kinds-problem-block",
        "notify-kinds-problem",
        cx
    ));
    assert!(!has("notify-kinds-skeleton", cx), "no skeleton beside why");

    // Fixed, the line folds away and the controls unfold.
    std::fs::remove_file(&file).unwrap();
    user.call(
        "notifications.set_settings",
        json!({"kinds": {"report": "ring"}}),
    )
    .unwrap();
    until(&shell, cx, "the controls start to unfold", |cx| {
        has("notify-kinds-content", cx) && rect("notify-kinds-content", cx).size.height > px(0.)
    });
    let start = rect("notify-kinds-content", cx).size.height;
    cx.executor().advance_clock(PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let part = rect("notify-kinds-content", cx).size.height;
    settle(cx);
    let open = rect("notify-kinds-content", cx).size.height;
    assert!(start < part && part < open, "{start:?} {part:?} {open:?}");
    assert!(!has("notify-kinds-problem", cx), "folded away");
}

#[gpui_kit::test]
fn a_choice_in_the_tab_is_the_daemon_s_and_the_tab_says_what_it_keeps(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_tab(&shell, cx);

    click("notify-kind-report-centre", cx);
    until(&shell, cx, "the daemon keeps it", |_| {
        settings_of(&user)["kinds"]["report"] == "centre"
    });
    let view = page.read_with(cx, |p, _| p.notifications().clone());
    until(&shell, cx, "the tab shows it", |cx| {
        view.read_with(cx, |view, _| {
            view.shown()
                .is_some_and(|shown| shown.level("report") == "centre")
        })
    });

    click("notify-kind-report-sound", cx);
    until(&shell, cx, "the daemon keeps the kind's sound", |_| {
        settings_of(&user)["sound"]["kinds"]["report"] == false
    });

    click("notify-volume", cx);
    click("notify-volume-50", cx);
    until(&shell, cx, "the daemon keeps the volume", |_| {
        settings_of(&user)["sound"]["volume"] == 50
    });

    click("notify-sound-switch", cx);
    until(&shell, cx, "the daemon keeps the sound off", |_| {
        settings_of(&user)["sound"]["on"] == false
    });
    // The file is the user's: what the window chose is in it.
    let text = std::fs::read_to_string(user.path("config/flottille/notifications.toml")).unwrap();
    assert!(text.contains("volume = 50"), "{text}");
}

#[gpui_kit::test]
fn a_change_from_the_command_line_is_followed_by_the_tab(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_tab(&shell, cx);
    let view = page.read_with(cx, |p, _| p.notifications().clone());

    let output = user.run(&["notifications", "settings", "--kind", "ticket=off"]);
    assert!(output.status.success());

    until(&shell, cx, "the tab follows", |cx| {
        view.read_with(cx, |view, _| {
            view.shown()
                .is_some_and(|shown| shown.level("ticket") == "off")
        })
    });
}

#[gpui_kit::test]
fn the_quiet_hours_unfold_under_their_switch_and_the_days_and_times_are_chosen(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_tab(&shell, cx);
    assert!(!has("notify-quiet-range-block", cx), "off: no range");

    cx.update(|window, cx| window.click("notify-quiet-switch", cx));
    until(&shell, cx, "the daemon keeps it on", |_| {
        settings_of(&user)["quiet"]["on"] == true
    });
    // The range is not there at its full height at once: it unfolds.
    until(&shell, cx, "the range starts to unfold", |cx| {
        has("notify-quiet-range-block", cx)
    });
    let start = rect("notify-quiet-range-block", cx).size.height;
    cx.executor().advance_clock(PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let part = rect("notify-quiet-range-block", cx).size.height;
    settle(cx);
    let open = rect("notify-quiet-range-block", cx).size.height;
    assert!(start < part && part < open, "{start:?} {part:?} {open:?}");
    for id in [
        "notify-quiet-from-field-label",
        "notify-quiet-to-field-label",
        "notify-quiet-days-field-label",
    ] {
        assert!(has(id, cx), "{id}");
    }
    assert_eq!(
        label("notify-quiet-from-field-label", cx).as_deref(),
        Some("From")
    );

    click("notify-quiet-day-sat", cx);
    until(&shell, cx, "the daemon keeps the days", |_| {
        let days = settings_of(&user)["quiet"]["days"].clone();
        days.as_array()
            .is_some_and(|days| days.len() == 6 && !days.iter().any(|day| day == "sat"))
    });
    click("notify-quiet-from", cx);
    click("notify-quiet-from-21:00", cx);
    until(&shell, cx, "the daemon keeps the start", |_| {
        settings_of(&user)["quiet"]["from"] == "21:00"
    });

    // Off again: the range folds away.
    click("notify-quiet-switch", cx);
    until(&shell, cx, "the daemon keeps it off", |_| {
        settings_of(&user)["quiet"]["on"] == false
    });
    settle(cx);
    assert!(!has("notify-quiet-range-block", cx), "folded away");
}

#[gpui_kit::test]
fn a_refused_choice_is_a_toast_and_the_tab_keeps_what_the_daemon_said(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_tab(&shell, cx);
    // The only day left cannot be taken off: the daemon refuses a range with no day.
    user.call(
        "notifications.set_settings",
        json!({"quiet": {"on": true, "days": ["mon"]}}),
    )
    .unwrap();
    until(&shell, cx, "the tab shows one day", |cx| {
        let view = page.read_with(cx, |p, _| p.notifications().clone());
        view.read_with(cx, |view, _| {
            view.shown()
                .is_some_and(|shown| shown.quiet_days == ["mon"])
        })
    });
    settle(cx);

    cx.update(|window, cx| window.click("notify-quiet-day-mon", cx));
    until(&shell, cx, "a toast says it was refused", |cx| {
        has("toast-1", cx)
    });

    assert_eq!(settings_of(&user)["quiet"]["days"], json!(["mon"]));
}

#[gpui_kit::test]
fn a_project_has_its_own_level_for_each_kind_and_follows_the_default_until_it_sets_one(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_project_tab(&shell, cx);

    assert_eq!(
        label("notifications-project-title", cx).as_deref(),
        Some("Notifications")
    );
    for kind in KINDS {
        assert_eq!(
            label(format!("notify-project-{kind}-field-label"), cx).as_deref(),
            Some(kind_label(kind)),
            "{kind}"
        );
    }
    // It follows the defaults, and says what they are.
    let follows = label("notify-project-report", cx).unwrap_or_default();
    assert!(follows.starts_with("As Settings"), "{follows}");

    click("notify-project-report", cx);
    click("notify-project-report-centre", cx);
    until(&shell, cx, "the daemon keeps the project's own", |_| {
        settings_of(&user)["projects"]["app"]["report"] == "centre"
    });
    // The default is untouched: the project's own wins for it alone.
    assert_eq!(settings_of(&user)["kinds"]["report"], "ring");
    let view = page.read_with(cx, |p, _| p.notifications().clone());
    until(&shell, cx, "the section shows it", |cx| {
        view.read_with(cx, |view, _| {
            view.shown()
                .is_some_and(|shown| shown.own("app", "report") == Some("centre"))
        })
    });

    // Following the default again removes what it set.
    click("notify-project-report", cx);
    click("notify-project-report-default", cx);
    until(&shell, cx, "the daemon forgets it", |_| {
        settings_of(&user)["projects"] == json!({})
    });
}

#[gpui_kit::test]
fn the_bell_turns_do_not_disturb_on_for_an_hour_and_off(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the window knows what rings", |cx| {
        shell.read_with(cx, |s, cx| {
            s.bell_list().read(cx).do_not_disturb().is_some()
        })
    });
    click("bell", cx);
    assert_eq!(
        label("bell-dnd-switch-label", cx).as_deref(),
        Some("Do not disturb")
    );
    // It says how long before it is on, and not once it is.
    assert!(has("bell-dnd-how-field-label", cx));
    click("bell-dnd-how", cx);
    click("bell-dnd-how-one-hour", cx);

    click("bell-dnd-switch", cx);
    until(&shell, cx, "the daemon holds it", |_| {
        settings_of(&user)["do_not_disturb"]["on"] == true
    });
    let said = settings_of(&user);
    let until_ms = said["do_not_disturb"]["until"].as_u64().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(
        until_ms > now + 3_000_000 && until_ms <= now + 3_700_000,
        "an hour: {until_ms} from {now}"
    );
    until(&shell, cx, "the bell says it is on", |cx| {
        shell.read_with(cx, |s, cx| {
            s.bell_list().read(cx).do_not_disturb() == Some(true)
        })
    });
    settle(cx);
    assert!(
        !has("bell-dnd-how-field-label", cx),
        "on: the length folded away"
    );
    let line = label("bell-dnd-line", cx).unwrap_or_default();
    assert!(line.starts_with("Until "), "{line}");

    click("bell-dnd-switch", cx);
    until(&shell, cx, "the daemon lets it go", |_| {
        settings_of(&user)["do_not_disturb"]["on"] == false
    });
    until(&shell, cx, "the bell says it is off", |cx| {
        shell.read_with(cx, |s, cx| {
            s.bell_list().read(cx).do_not_disturb() == Some(false)
        })
    });
    settle(cx);
    assert!(
        has("bell-dnd-how-field-label", cx),
        "off: the length is offered"
    );
}
