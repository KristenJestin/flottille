//! What rings, and when (#1003): the user's settings by kind and by project, the sound, the quiet
//! hours and Do not disturb, kept in `notifications.toml` and read by the daemon at every ring. A
//! private `dbus-daemon` with a fake notification server on it, a fake audio sink and the fake
//! ACP agent: never the machine's bus, nor its sound.

use crate::common;
use crate::desktop_notifications::{project, root};

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::desktop::{self, Bus, Notifications};
use common::mcp::{Answer, Mcp};
use serde_json::{Value, json};

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A user with a private bus, a notification server on it, a fake audio sink and a root session
/// of `app` with its first turn done; `None` when this machine has no `dbus-daemon`.
struct Desk {
    user: User,
    _bus: Bus,
    server: Notifications,
    played: PathBuf,
    mcp: Mcp,
}

fn desk(user: User) -> Option<Desk> {
    let bus = Bus::start(&user)?;
    let server = Notifications::start(&bus, &["actions", "body"]);
    let played = desktop::with_fake_audio(&user);
    project(&user);
    let (_, mcp) = root(&user, "hello");
    Some(Desk {
        user,
        _bus: bus,
        server,
        played,
        mcp,
    })
}

fn held_number(answer: Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .split_once("request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
        .1
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// A request the gate holds for the user.
fn held(mcp: &Mcp) -> u64 {
    held_number(mcp.call("fs_read", json!({"path": ".env"})))
}

/// A report of the root session to the user.
fn report(mcp: &Mcp) {
    let answer = mcp.call("report", json!({"text": "Login fixed."}));
    assert!(!answer.error, "{}", answer.text);
}

fn rows(user: &User) -> Vec<Value> {
    user.call("notifications.list", json!({})).unwrap()["notifications"]
        .as_array()
        .unwrap()
        .clone()
}

fn kinds(user: &User) -> Vec<String> {
    rows(user)
        .iter()
        .map(|row| row["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn set(user: &User, params: Value) -> Value {
    user.call("notifications.set_settings", params).unwrap()
}

fn refused(user: &User, params: Value) -> (String, String) {
    user.call("notifications.set_settings", params)
        .expect_err("the settings were refused")
}

fn settings(user: &User) -> Value {
    user.call("notifications.settings", json!({})).unwrap()
}

fn config(user: &User) -> PathBuf {
    user.path("config/flottille/notifications.toml")
}

/// The time of the day of the daemon's clock, in a test build: the quiet hours read it.
fn at(user: User, time: &str) -> User {
    user.with_env("FLOTTILLE_TEST_LOCAL_TIME", time)
}

#[test]
fn the_settings_say_what_rings_until_the_user_chooses_otherwise() {
    let user = User::with_fake_agent();

    let said = settings(&user);

    for kind in ["request", "question", "report", "failure"] {
        assert_eq!(said["kinds"][kind], "ring", "{kind}: {said}");
    }
    for kind in [
        "pull_request",
        "ticket",
        "paused",
        "vault",
        "daemon",
        "routine",
    ] {
        assert_eq!(said["kinds"][kind], "centre", "{kind}: {said}");
    }
    assert_eq!(said["sound"]["on"], true, "{said}");
    assert_eq!(said["sound"]["volume"], 100, "{said}");
    assert_eq!(said["sound"]["kinds"]["report"], true, "{said}");
    assert_eq!(said["quiet"]["on"], false, "{said}");
    assert_eq!(said["do_not_disturb"]["on"], false, "{said}");
    assert_eq!(said["muted"], false, "{said}");
    assert_eq!(said["projects"], json!({}), "{said}");
    assert!(!config(&user).exists(), "nothing is written until a choice");
}

#[test]
fn a_choice_is_kept_in_the_configuration_folder_and_told_to_the_clients() {
    let user = User::with_fake_agent();
    let watch = user.watch(&[]);

    let said = set(
        &user,
        json!({"kinds": {"report": "centre", "ticket": "off"},
            "sound": {"on": true, "volume": 40, "kinds": {"report": false}},
            "quiet": {"on": true, "from": "22:00", "to": "08:00", "days": ["mon", "tue"]}}),
    );

    assert_eq!(said["kinds"]["report"], "centre", "{said}");
    assert_eq!(said["kinds"]["ticket"], "off", "{said}");
    assert_eq!(said["sound"]["volume"], 40, "{said}");
    assert_eq!(said["sound"]["kinds"]["report"], false, "{said}");
    assert_eq!(said["quiet"]["from"], "22:00", "{said}");
    assert_eq!(said["quiet"]["days"], json!(["mon", "tue"]), "{said}");
    let seen = watch.until(|event| event["name"] == "notifications.settings_changed");
    assert_eq!(seen.last().unwrap()["data"]["kinds"]["report"], "centre");
    // Another client reads the same; and the file is the one the user can edit by hand.
    assert_eq!(settings(&user), said);
    let text = std::fs::read_to_string(config(&user)).unwrap();
    assert!(text.contains("volume = 40"), "{text}");
    assert!(text.contains("report = \"centre\""), "{text}");
    // A choice that follows the default is not written: the default may change.
    set(&user, json!({"kinds": {"report": "default"}}));
    let text = std::fs::read_to_string(config(&user)).unwrap();
    assert!(!text.contains("report = \"centre\""), "{text}");
    assert_eq!(settings(&user)["kinds"]["report"], "ring");
}

#[test]
fn a_choice_that_makes_no_sense_is_refused_and_nothing_is_kept() {
    let user = User::with_fake_agent();
    project(&user);
    let bad = [
        json!({"kinds": {"nonsense": "ring"}}),
        json!({"kinds": {"report": "loud"}}),
        json!({"sound": {"volume": 101}}),
        json!({"sound": {"kinds": {"nonsense": true}}}),
        json!({"quiet": {"from": "25:00"}}),
        json!({"quiet": {"from": "22:00", "to": "22:00"}}),
        json!({"quiet": {"days": ["someday"]}}),
        json!({"quiet": {"days": []}}),
        json!({"project": "nope", "kinds": {"report": "ring"}}),
        json!({"unknown": true}),
        json!({}),
    ];
    for params in bad {
        let (code, _) = refused(&user, params.clone());
        assert_eq!(code, "invalid-params", "{params}");
    }
    assert!(!config(&user).exists());
}

#[test]
fn a_kind_set_to_centre_only_rings_no_desktop_notification() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };
    set(&desk.user, json!({"kinds": {"report": "centre"}}));

    report(&desk.mcp);
    let number = held(&desk.mcp);

    // The daemon follows the centre in order: the request ringing says the report was seen and
    // left alone, in the desktop and in the sound alike. It is kept in the centre.
    let told = desk.server.wait_for(1);
    assert_eq!(told[0].summary, format!("Request #{number} waits for you"));
    eventually("the sound", || !desktop::played(&desk.played).is_empty());
    assert_eq!(desktop::played(&desk.played), ["needs-you"]);
    assert_eq!(desk.server.notified().len(), 1);
    assert_eq!(kinds(&desk.user), ["report", "request"]);
}

#[test]
fn a_kind_set_to_off_leaves_nothing_in_the_centre() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };
    set(&desk.user, json!({"kinds": {"report": "off"}}));

    report(&desk.mcp);
    held(&desk.mcp);

    desk.server.wait_for(1);
    assert_eq!(kinds(&desk.user), ["request"]);
    assert_eq!(desk.server.notified().len(), 1);
}

#[test]
fn a_request_cannot_be_turned_off() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };

    for params in [
        json!({"kinds": {"request": "off"}}),
        json!({"kinds": {"question": "off"}}),
        json!({"project": "app", "kinds": {"request": "off"}}),
    ] {
        let (code, message) = refused(&desk.user, params);
        assert_eq!(code, "invalid-params");
        assert!(message.contains("needs you"), "{message}");
    }
    assert!(!config(&desk.user).exists());
    // Only its ringing can be lowered: it stays in the centre, silent.
    set(&desk.user, json!({"kinds": {"request": "centre"}}));
    let number = held(&desk.mcp);
    set(&desk.user, json!({"kinds": {"request": "default"}}));
    let second = held(&desk.mcp);

    let told = desk.server.wait_for(1);
    assert_eq!(told[0].summary, format!("Request #{second} waits for you"));
    assert_eq!(desk.server.notified().len(), 1);
    let titles: Vec<String> = rows(&desk.user)
        .iter()
        .map(|row| row["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        titles,
        [
            format!("Request #{number} waits for you"),
            format!("Request #{second} waits for you")
        ]
    );
}

#[test]
fn a_file_that_turns_a_request_off_is_refused_and_the_request_still_rings() {
    let user = User::with_fake_agent();
    std::fs::write(config(&user), "[kinds]\nrequest = \"off\"\n").unwrap();
    let Some(desk) = desk(user) else {
        return;
    };

    let (code, message) = desk
        .user
        .call("notifications.settings", json!({}))
        .unwrap_err();
    assert_eq!(code, "invalid-config");
    assert!(message.contains("notifications.toml"), "{message}");
    held(&desk.mcp);

    desk.server.wait_for(1);
}

#[test]
fn a_project_s_own_levels_are_its_kinds_in_the_file() {
    let user = User::with_fake_agent();
    project(&user);
    // As `docs/projects.md` says it: `[projects.<name>.kinds]`, written by hand.
    std::fs::write(
        config(&user),
        "[kinds]\nreport = \"centre\"\n\n[projects.app.kinds]\nreport = \"ring\"\n",
    )
    .unwrap();

    let said = settings(&user);
    assert_eq!(said["projects"]["app"]["report"], "ring", "{said}");
    assert_eq!(said["kinds"]["report"], "centre", "{said}");

    // And the daemon writes it so.
    set(
        &user,
        json!({"project": "app", "kinds": {"ticket": "centre"}}),
    );
    let text = std::fs::read_to_string(config(&user)).unwrap();
    assert!(text.contains("[projects.app.kinds]"), "{text}");
}

#[test]
fn a_file_that_cannot_be_read_is_said_in_the_log_and_in_settings() {
    let user = User::with_fake_agent();
    std::fs::write(config(&user), "[kinds\nreport = ").unwrap();
    let Some(desk) = desk(user) else {
        return;
    };

    // Settings says it, with the file.
    let (code, message) = desk
        .user
        .call("notifications.settings", json!({}))
        .unwrap_err();
    assert_eq!(code, "invalid-config");
    assert!(message.contains("notifications.toml"), "{message}");
    // A ring follows the defaults, and the log says why, once.
    held(&desk.mcp);
    held(&desk.mcp);
    desk.server.wait_for(2);
    eventually("the log said it", || {
        desktop::daemon_log(&desk.user).contains("notifications.toml")
    });
    let log = desktop::daemon_log(&desk.user);
    assert_eq!(log.matches("notifications.toml").count(), 1, "{log}");
}

#[test]
fn a_kind_without_sound_rings_silently() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };
    set(&desk.user, json!({"sound": {"kinds": {"request": false}}}));

    held(&desk.mcp);
    report(&desk.mcp);

    // The request rings, on the desktop, with no sound; the report's is the only one played.
    desk.server.wait_for(2);
    eventually("the report's sound", || {
        !desktop::played(&desk.played).is_empty()
    });
    assert_eq!(desktop::played(&desk.played), ["tell"]);
}

#[test]
fn the_sound_is_off_or_as_loud_as_the_user_says() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };
    set(&desk.user, json!({"sound": {"volume": 50}}));
    held(&desk.mcp);
    desk.server.wait_for(1);
    eventually("the sound", || !desktop::played(&desk.played).is_empty());
    assert_eq!(desktop::played(&desk.played), ["needs-you 0.5"]);

    set(&desk.user, json!({"sound": {"on": false}}));
    report(&desk.mcp);

    desk.server.wait_for(2);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(desktop::played(&desk.played), ["needs-you 0.5"]);
}

#[test]
fn a_project_s_own_setting_wins_over_the_default() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };
    // The default says the reports are for the centre; the project's own says they ring.
    set(&desk.user, json!({"kinds": {"report": "centre"}}));
    let said = set(
        &desk.user,
        json!({"project": "app", "kinds": {"report": "ring"}}),
    );
    assert_eq!(said["projects"]["app"]["report"], "ring", "{said}");
    assert_eq!(said["kinds"]["report"], "centre", "{said}");

    report(&desk.mcp);

    let told = desk.server.wait_for(1);
    assert_eq!(told[0].summary, "Reported");

    // It follows the default again: the next report does not ring.
    let said = set(
        &desk.user,
        json!({"project": "app", "kinds": {"report": "default"}}),
    );
    assert_eq!(said["projects"], json!({}), "{said}");
    report(&desk.mcp);
    let number = held(&desk.mcp);
    let told = desk.server.wait_for(2);
    assert_eq!(told[1].summary, format!("Request #{number} waits for you"));
    assert_eq!(desk.server.notified().len(), 2);
}

#[test]
fn quiet_hours_hold_the_rings_and_say_how_many_waited() {
    let user = at(User::with_fake_agent(), "Fri 23:30");
    let Some(desk) = desk(user) else {
        return;
    };
    let said = set(
        &desk.user,
        json!({"quiet": {"on": true, "from": "22:00", "to": "08:00"}}),
    );
    assert_eq!(said["muted"], true, "{said}");

    held(&desk.mcp);
    held(&desk.mcp);
    report(&desk.mcp);

    // Inside the range nothing rings, and everything is in the centre.
    eventually("the three rows", || rows(&desk.user).len() == 3);
    std::thread::sleep(Duration::from_millis(300));
    assert!(desk.server.notified().is_empty());
    assert!(desktop::played(&desk.played).is_empty());

    // The range ends (here: the user turns it off): the first ring says how many waited.
    let said = set(&desk.user, json!({"quiet": {"on": false}}));
    assert_eq!(said["muted"], false, "{said}");
    held(&desk.mcp);
    let told = desk.server.wait_for(1);
    assert!(told[0].body.contains("3 new while quiet"), "{:?}", told[0]);
    // Said once: the next ring does not repeat it.
    held(&desk.mcp);
    let told = desk.server.wait_for(2);
    assert!(!told[1].body.contains("while quiet"), "{:?}", told[1]);
}

#[test]
fn quiet_hours_are_the_days_the_user_chose_and_the_hours_of_their_clock() {
    // Saturday 07:00: inside 22:00-08:00 only as Friday night's end.
    let user = at(User::with_fake_agent(), "Sat 07:00");
    let range =
        |days: Value| json!({"quiet": {"on": true, "from": "22:00", "to": "08:00", "days": days}});

    assert_eq!(set(&user, range(json!(["fri"])))["muted"], true);
    assert_eq!(set(&user, range(json!(["sat"])))["muted"], false);
    assert_eq!(
        set(
            &user,
            json!({"quiet": {"days": ["fri", "sat"], "from": "08:00", "to": "20:00"}})
        )["muted"],
        false
    );
    // The same day, a range that does not cross midnight.
    assert_eq!(
        set(
            &user,
            json!({"quiet": {"days": ["sat"], "from": "06:00", "to": "08:00"}})
        )["muted"],
        true
    );
}

#[test]
fn do_not_disturb_for_an_hour_ends_by_itself() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };
    let until = now_ms() + 2_500;
    let said = set(
        &desk.user,
        json!({"do_not_disturb": {"on": true, "until": until}}),
    );
    assert_eq!(said["do_not_disturb"]["on"], true, "{said}");
    assert_eq!(said["do_not_disturb"]["until"], until, "{said}");
    assert_eq!(said["muted"], true, "{said}");

    held(&desk.mcp);
    eventually("the row", || rows(&desk.user).len() == 1);
    assert!(desk.server.notified().is_empty());

    // Nobody turned it off: its time is over.
    eventually("the end of it", || settings(&desk.user)["muted"] == false);
    assert_eq!(settings(&desk.user)["do_not_disturb"]["on"], false);
    held(&desk.mcp);
    let told = desk.server.wait_for(1);
    assert!(told[0].body.contains("1 new while quiet"), "{:?}", told[0]);
}

#[test]
fn do_not_disturb_until_turned_off_holds_every_ring() {
    let Some(desk) = desk(User::with_fake_agent()) else {
        return;
    };
    set(&desk.user, json!({"do_not_disturb": {"on": true}}));

    held(&desk.mcp);
    eventually("the row", || rows(&desk.user).len() == 1);
    std::thread::sleep(Duration::from_millis(300));
    assert!(desk.server.notified().is_empty());
    assert!(desktop::played(&desk.played).is_empty());

    let said = set(&desk.user, json!({"do_not_disturb": {"on": false}}));
    assert_eq!(said["muted"], false, "{said}");
    report(&desk.mcp);
    let told = desk.server.wait_for(1);
    assert!(told[0].body.contains("1 new while quiet"), "{:?}", told[0]);
}

#[test]
fn an_agent_cannot_change_the_notification_settings() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, mcp) = root(&user, "hello");
    user.run(&["agent", "mode", &id, "full"]);

    // No tool of the agent's reaches them, and no action of a tool the agent has.
    let names = mcp.names();
    assert!(
        names.iter().all(|name| !name.contains("notification")),
        "{names:?}"
    );
    let called = mcp.call(
        "notifications",
        json!({"action": "set_settings", "kinds": {"report": "off"}}),
    );
    assert!(called.error, "{}", called.text);
    let project_tool = mcp.call(
        "project",
        json!({"action": "set_settings", "notifications": {"report": "off"}}),
    );
    assert!(project_tool.error, "{}", project_tool.text);
    assert!(!config(&user).exists());
    assert_eq!(settings(&user)["kinds"]["report"], "ring");
}

#[test]
fn the_command_line_reads_and_sets_them() {
    let user = User::with_fake_agent();
    project(&user);

    let output = user.run(&[
        "notifications",
        "settings",
        "--kind",
        "report=centre",
        "--sound-kind",
        "failure=off",
        "--volume",
        "70",
        "--quiet",
        "22:00-08:00",
        "--days",
        "mon,tue,wed",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let said: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(said["kinds"]["report"], "centre", "{said}");
    assert_eq!(said["sound"]["kinds"]["failure"], false, "{said}");
    assert_eq!(said["sound"]["volume"], 70, "{said}");
    assert_eq!(said["quiet"]["on"], true, "{said}");
    assert_eq!(said["quiet"]["to"], "08:00", "{said}");
    assert_eq!(
        said["quiet"]["days"],
        json!(["mon", "tue", "wed"]),
        "{said}"
    );

    let output = user.run(&[
        "notifications",
        "settings",
        "--project",
        "app",
        "--kind",
        "ticket=ring",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = user.run(&["notifications", "settings", "--dnd", "1h"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Do not disturb"), "{text}");

    let output = user.run(&["notifications", "settings"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Report"), "{text}");
    assert!(text.contains("centre only"), "{text}");
    assert!(text.contains("22:00–08:00"), "{text}");
    assert!(text.contains("volume 70"), "{text}");
    assert!(text.contains("app: ticket rings"), "{text}");
    let said = settings(&user);
    assert_eq!(said["do_not_disturb"]["on"], true, "{said}");
    assert!(said["do_not_disturb"]["until"].is_number(), "{said}");

    let output = user.run(&["notifications", "settings", "--kind", "request=off"]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("needs you"), "{said}");
}

#[test]
fn a_removed_project_takes_what_rings_with_it() {
    let user = User::with_fake_agent();
    project(&user);
    set(
        &user,
        json!({"project": "app", "kinds": {"report": "centre"}}),
    );
    assert_eq!(settings(&user)["projects"]["app"]["report"], "centre");

    user.call("project.remove", json!({"name": "app"})).unwrap();

    let said = settings(&user);
    assert!(said["projects"].get("app").is_none(), "{said}");
    let text = std::fs::read_to_string(config(&user)).unwrap_or_default();
    assert!(!text.contains("projects.app"), "{text}");
    // A new project of the same name starts at the default (its folder was never touched).
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    let said = settings(&user);
    assert!(said["projects"].get("app").is_none(), "{said}");
}

#[test]
fn what_was_kept_quiet_survives_a_restart() {
    let user = at(User::with_fake_agent(), "Fri 23:30");
    let Some(desk) = desk(user) else {
        return;
    };
    set(
        &desk.user,
        json!({"quiet": {"on": true, "from": "22:00", "to": "08:00"}}),
    );
    held(&desk.mcp);
    held(&desk.mcp);
    eventually("the two rows", || rows(&desk.user).len() == 2);
    std::thread::sleep(Duration::from_millis(300));

    assert!(desk.user.run(&["daemon", "stop"]).status.success());
    // The agent of before went with the daemon: a root of the new one.
    let (_, mcp) = root(&desk.user, "hello again");
    set(&desk.user, json!({"quiet": {"on": false}}));
    held(&mcp);

    let told = desk.server.wait_for(1);
    assert!(told[0].body.contains("2 new while quiet"), "{:?}", told[0]);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
