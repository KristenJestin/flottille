//! Settings for all machines, part 2 (#1461, `docs/remote.md`, "Settings for all machines"): the
//! guidance files and the rest of "Shared, personal" on the mechanism About you built (#1486). The
//! user's device signs `settings.share {name, value}` on one machine, whose daemon seals it for
//! every machine and gives the hub the sealed copy. Each value is read on three levels: the
//! machine's own, else all machines', else the default. A hub and two daemons served in the test,
//! each with its own temporary folders and the in-memory keyring; invented texts.

use crate::common;
use crate::remote;

use common::User;
use flottille_link::DeviceKey;
use remote::{Machine, TestHub, error_of, eventually, pair, pc_and_server_on, signed};
use serde_json::{Value, json};

/// A vault entry's secret, which never leaves its machine.
const SECRET: &str = "vau1t-s3cret-Qm7Z";

/// A hub and two machines whose daemons cut tool results as they do for everyone: the harness's
/// own `tool-results.toml` would be each machine's own limit.
fn machines() -> (TestHub, Machine, Machine) {
    pc_and_server_on(
        Machine::of(User::new().with_cut_results()),
        User::new().with_cut_results(),
    )
}

/// One value of all machines: its name, what is shared, what a machine writes for itself, and how
/// the tests read it, write it for the machine and drop it, as the user's clients do.
struct Case {
    name: &'static str,
    /// What `settings.share` carries: a text, or the JSON text of an extension's setting.
    all: &'static str,
    own: &'static str,
    /// What the machine's agents read now, as text.
    read: fn(&Machine) -> String,
    /// What the machine's own value is now: none while it follows all machines.
    put: fn(&Machine, &str),
    drop: fn(&Machine),
    /// Whether the machine says where the value comes from (`source`), as the guidance files do.
    sourced: bool,
}

fn text_of(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn extension(machine: &Machine, name: &str, key: &str) -> Value {
    let listed = machine.user.call("extension.list", json!({})).unwrap();
    let extension = listed["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|extension| extension["name"] == name)
        .unwrap_or_else(|| panic!("no extension {name}: {listed}"))
        .clone();
    extension["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["key"] == key)
        .unwrap_or_else(|| panic!("no setting {key}: {extension}"))["value"]
        .clone()
}

fn set_extension(machine: &Machine, name: &str, key: &str, value: Value) {
    machine
        .user
        .call(
            "extension.set",
            json!({"name": name, "settings": {key: value}}),
        )
        .unwrap();
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "rules",
            all: "Small commits, tests first.",
            own: "Rules of this machine alone.",
            read: |m| text_of(&m.user.call("rules.show", json!({})).unwrap()["text"]),
            put: |m, text| {
                m.user.call("rules.set", json!({"text": text})).unwrap();
            },
            drop: |m| {
                m.user.call("rules.reset", json!({})).unwrap();
            },
            sourced: true,
        },
        Case {
            name: "judge.guidance",
            all: "Let a read of the project through.",
            own: "Ask about every write.",
            read: |m| text_of(&m.user.call("judge.show", json!({})).unwrap()["text"]),
            put: |m, text| {
                m.user.call("judge.set", json!({"text": text})).unwrap();
            },
            drop: |m| {
                m.user.call("judge.reset", json!({})).unwrap();
            },
            sourced: true,
        },
        Case {
            name: "compaction.guidance",
            all: "Keep the decisions and the open questions.",
            own: "Keep the file names.",
            read: |m| {
                text_of(&m.user.call("compaction.show", json!({})).unwrap()["guidance"]["text"])
            },
            put: |m, text| {
                m.user
                    .call("compaction.set_guidance", json!({"text": text}))
                    .unwrap();
            },
            drop: |m| {
                m.user.call("compaction.reset_guidance", json!({})).unwrap();
            },
            sourced: false,
        },
        Case {
            name: "review.guidance",
            all: "Look at the tests first.",
            own: "Look at the docs first.",
            read: |m| text_of(&m.user.call("review.show", json!({})).unwrap()["text"]),
            put: |m, text| {
                m.user.call("review.set", json!({"text": text})).unwrap();
            },
            drop: |m| {
                m.user.call("review.reset", json!({})).unwrap();
            },
            sourced: true,
        },
        Case {
            name: "review.guidance.bugs",
            all: "Hunt the off-by-one.",
            own: "Hunt the unwrap.",
            read: |m| {
                text_of(&m.user.call("review.show", json!({"part": "bugs"})).unwrap()["text"])
            },
            put: |m, text| {
                m.user
                    .call("review.set", json!({"part": "bugs", "text": text}))
                    .unwrap();
            },
            drop: |m| {
                m.user
                    .call("review.reset", json!({"part": "bugs"}))
                    .unwrap();
            },
            sourced: true,
        },
        Case {
            name: "tool_results.limit",
            all: "6000",
            own: "5000",
            read: |m| text_of(&m.user.call("tool_results.show", json!({})).unwrap()["limit"]),
            put: |m, text| {
                let limit: u64 = text.parse().unwrap();
                m.user
                    .call("tool_results.set", json!({"limit": limit}))
                    .unwrap();
            },
            drop: |m| {
                m.user
                    .call("tool_results.set", json!({"reset": ["limit"]}))
                    .unwrap();
            },
            sourced: false,
        },
        Case {
            name: "tool_results.unchanged",
            all: "true",
            own: "false",
            read: |m| text_of(&m.user.call("tool_results.show", json!({})).unwrap()["unchanged"]),
            put: |m, text| {
                let on: bool = text.parse().unwrap();
                m.user
                    .call("tool_results.set", json!({"unchanged": on}))
                    .unwrap();
            },
            drop: |m| {
                m.user
                    .call("tool_results.set", json!({"reset": ["unchanged"]}))
                    .unwrap();
            },
            sourced: false,
        },
        Case {
            name: "specs.language",
            all: "\"fr-FR\"",
            own: "\"de\"",
            read: |m| extension(m, "specs", "language").to_string(),
            put: |m, text| {
                set_extension(m, "specs", "language", serde_json::from_str(text).unwrap())
            },
            drop: |m| set_extension(m, "specs", "language", Value::Null),
            sourced: false,
        },
        Case {
            name: "specs.mode",
            all: "\"ticket\"",
            // The default written on a machine is still the machine's own.
            own: "\"internal\"",
            read: |m| extension(m, "specs", "mode").to_string(),
            put: |m, text| set_extension(m, "specs", "mode", serde_json::from_str(text).unwrap()),
            drop: |m| set_extension(m, "specs", "mode", Value::Null),
            sourced: false,
        },
        Case {
            name: "autosubscriptions.created_pull",
            all: "[\"merged\"]",
            own: "[\"closed\"]",
            read: |m| extension(m, "autosubscriptions", "created_pull").to_string(),
            put: |m, text| {
                set_extension(
                    m,
                    "autosubscriptions",
                    "created_pull",
                    serde_json::from_str(text).unwrap(),
                )
            },
            drop: |m| set_extension(m, "autosubscriptions", "created_pull", Value::Null),
            sourced: false,
        },
        Case {
            name: "autosubscriptions.branch_pull",
            all: "[\"checks\"]",
            own: "[\"comment\"]",
            read: |m| extension(m, "autosubscriptions", "branch_pull").to_string(),
            put: |m, text| {
                set_extension(
                    m,
                    "autosubscriptions",
                    "branch_pull",
                    serde_json::from_str(text).unwrap(),
                )
            },
            drop: |m| set_extension(m, "autosubscriptions", "branch_pull", Value::Null),
            sourced: false,
        },
        Case {
            name: "autosubscriptions.ticket",
            all: "[\"closed\"]",
            own: "[]",
            read: |m| extension(m, "autosubscriptions", "ticket").to_string(),
            put: |m, text| {
                set_extension(
                    m,
                    "autosubscriptions",
                    "ticket",
                    serde_json::from_str(text).unwrap(),
                )
            },
            drop: |m| set_extension(m, "autosubscriptions", "ticket", Value::Null),
            sourced: false,
        },
        Case {
            name: "review.passes",
            all: "[\"security\"]",
            own: "[\"bugs\"]",
            read: |m| extension(m, "review", "passes").to_string(),
            put: |m, text| {
                set_extension(m, "review", "passes", serde_json::from_str(text).unwrap())
            },
            drop: |m| set_extension(m, "review", "passes", Value::Null),
            sourced: false,
        },
        Case {
            name: "appearance.shade",
            all: "light",
            // The default written on a machine is still the machine's own.
            own: "dark",
            read: |m| text_of(&m.user.call("appearance.show", json!({})).unwrap()["shade"]),
            put: |m, text| {
                m.user
                    .call("appearance.set", json!({"shade": text}))
                    .unwrap();
            },
            drop: |m| {
                m.user
                    .call("appearance.set", json!({"reset": ["shade"]}))
                    .unwrap();
            },
            sourced: false,
        },
    ]
}

/// `value` of `name` shared from `machine`, as the user's device signs it there.
fn share(machine: &Machine, on: &str, name: &str, value: &str) -> Value {
    let answer = machine.raw(&signed(
        &machine.device(),
        &format!("share-{}", rand_id()),
        on,
        "settings.share",
        json!({"name": name, "value": value}),
    ));
    assert!(answer["error"].is_null(), "{name}: {answer}");
    answer["result"].clone()
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// What the hub keeps of the values, as it would write it.
fn hub_copy(hub: &TestHub) -> String {
    hub.with_hub(|hub| serde_json::to_string(hub.shared()).unwrap())
}

fn reads(machine: &Machine, case: &Case, what: &str, shared: &str) {
    eventually(&format!("{} {what}", case.name), || {
        (case.read)(machine) == shared
    });
}

#[test]
fn every_setting_of_all_machines_reaches_every_machine() {
    let (_hub, pc, server) = machines();
    for case in cases() {
        share(&pc, "pc", case.name, case.all);
        reads(&server, &case, "reaches the server", case.all);
        reads(&pc, &case, "reaches pc", case.all);
    }
}

#[test]
fn a_machine_that_was_off_gets_each_setting_at_its_connect() {
    let (_hub, pc, server) = machines();
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    for case in cases() {
        share(&pc, "pc", case.name, case.all);
    }

    for case in cases() {
        reads(&server, &case, "got it at its connect", case.all);
    }
}

#[test]
fn an_override_wins_on_its_machine_only() {
    let (_hub, pc, server) = machines();
    for case in cases() {
        share(&pc, "pc", case.name, case.all);
        reads(&server, &case, "has the value of all machines", case.all);

        (case.put)(&server, case.own);

        assert_eq!((case.read)(&server), case.own, "{}", case.name);
        reads(&pc, &case, "keeps all machines' value", case.all);
    }
}

#[test]
fn use_all_machines_value_restores_the_hub_s() {
    let (_hub, pc, server) = machines();
    for case in cases() {
        share(&pc, "pc", case.name, case.all);
        reads(&server, &case, "has the value of all machines", case.all);
        (case.put)(&server, case.own);

        (case.drop)(&server);

        reads(&server, &case, "is back on all machines' value", case.all);
    }
}

#[test]
fn the_text_of_a_guidance_says_where_it_comes_from() {
    let (_hub, pc, server) = machines();
    for case in cases().into_iter().filter(|case| case.sourced) {
        let show = |machine: &Machine| match case.name {
            "rules" => machine.user.call("rules.show", json!({})).unwrap(),
            "judge.guidance" => machine.user.call("judge.show", json!({})).unwrap(),
            "review.guidance" => machine.user.call("review.show", json!({})).unwrap(),
            _ => machine
                .user
                .call("review.show", json!({"part": "bugs"}))
                .unwrap(),
        };
        // Nothing shared: a machine with a hub says there is nothing of all machines.
        assert!(
            show(&server)["all"].is_string(),
            "{}: {}",
            case.name,
            show(&server)
        );
        assert_ne!(show(&server)["source"], "all", "{}", case.name);

        share(&pc, "pc", case.name, case.all);
        reads(&server, &case, "reaches the server", case.all);
        assert_eq!(show(&server)["source"], "all", "{}", case.name);
        assert_eq!(show(&server)["all"], case.all, "{}", case.name);
        assert_eq!(show(&server)["own"], false, "{}", case.name);

        (case.put)(&server, case.own);
        let own = show(&server);
        assert_eq!(own["source"], "global", "{}: {own}", case.name);
        assert_eq!(own["own"], true, "{}: {own}", case.name);
        assert_eq!(own["all"], case.all, "{}: {own}", case.name);
    }
}

#[test]
fn the_latest_of_two_changes_wins() {
    let (_hub, pc, server) = machines();
    let case = &cases()[0];
    share(&pc, "pc", case.name, "First rules.");
    reads(&server, case, "got the first", "First rules.");

    share(&server, "server", case.name, "Second rules.");

    reads(&pc, case, "got the latest", "Second rules.");
}

#[test]
fn the_hub_cannot_read_a_value_and_no_secret_is_in_it() {
    let (hub, pc, server) = machines();
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    for case in cases() {
        share(&pc, "pc", case.name, case.all);
        reads(&server, &case, "reaches the server", case.all);
    }

    let kept = hub_copy(&hub);
    let file = std::fs::read_to_string(hub.state_file()).unwrap();

    for case in cases() {
        assert!(
            kept.contains(case.name),
            "the hub keeps {}: {kept}",
            case.name
        );
        for text in [case.all.trim_matches('"'), case.all] {
            if text.len() > 8 {
                assert!(!kept.contains(text), "the hub reads {text}: {kept}");
                assert!(!file.contains(text), "hub.json holds {text}: {file}");
            }
        }
    }
    assert!(!kept.contains(SECRET) && !file.contains(SECRET));
    // Only the names of the values are there: a vault entry, a variable or a token has none.
    let names: Vec<String> = hub.with_hub(|hub| {
        hub.shared()
            .iter()
            .map(|sealed| sealed.name.clone())
            .collect()
    });
    for name in &names {
        assert!(
            cases().iter().any(|case| case.name == name),
            "{name} is not a setting of all machines"
        );
    }
    for name in [
        "vault.tracker",
        "variables.TOKEN",
        "keys.device",
        "hub",
        "providers",
    ] {
        let answer = pc.raw(&signed(
            &pc.device(),
            "s",
            "pc",
            "settings.share",
            json!({"name": name, "value": SECRET}),
        ));
        assert!(!error_of(&answer).is_empty(), "{name}: {answer}");
    }
    assert!(!hub_copy(&hub).contains(SECRET));
}

#[test]
fn a_machine_with_the_hub_out_of_reach_keeps_its_last_values() {
    let (mut hub, pc, server) = machines();
    for case in cases() {
        share(&pc, "pc", case.name, case.all);
        reads(&server, &case, "reaches the server", case.all);
    }

    hub.stop();
    assert!(server.user.run(&["daemon", "stop"]).status.success());

    // Started again with the hub out of reach: what it last checked is what its agents get.
    for case in cases() {
        assert_eq!((case.read)(&server), case.all, "{}", case.name);
    }
}

#[test]
fn a_setting_needs_the_device_s_signature() {
    let (_hub, pc, _server) = machines();

    let (_, unsigned) = pc
        .user
        .call("settings.share", json!({"name": "rules", "value": "x"}))
        .unwrap_err();
    assert!(
        unsigned.contains("signed by the user's device"),
        "{unsigned}"
    );
    let stranger = DeviceKey::make().unwrap();
    let answer = pc.raw(&signed(
        &stranger,
        "1",
        "pc",
        "settings.share",
        json!({"name": "rules", "value": "x"}),
    ));
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );
}

#[test]
fn a_viewer_cannot_change_the_settings_of_all_machines() {
    let (hub, pc, server) = machines();
    let phone = Machine::new();
    pair(&hub, &phone, "phone", "viewer");
    pc.wait_for_member("phone", true);

    let answer = pc.raw(&signed(
        &phone.device(),
        "1",
        "pc",
        "settings.share",
        json!({"name": "rules", "value": "x"}),
    ));

    assert!(error_of(&answer).contains("may only view"), "{answer}");
    let rules = server.user.call("rules.show", json!({})).unwrap();
    assert_ne!(rules["source"], "all", "{rules}");
}

#[test]
fn a_value_that_would_be_refused_on_a_machine_is_refused_for_all() {
    let (hub, pc, _server) = machines();
    let refused = |name: &str, value: &str| {
        let answer = pc.raw(&signed(
            &pc.device(),
            &format!("bad-{}", rand_id()),
            "pc",
            "settings.share",
            json!({"name": name, "value": value}),
        ));
        assert!(!error_of(&answer).is_empty(), "{name}: {answer}");
    };
    refused("rules", &"a".repeat(8_001));
    refused("judge.guidance", &"a".repeat(4_001));
    refused("compaction.guidance", &"a".repeat(4_001));
    refused("review.guidance.bugs", &"a".repeat(4_001));
    refused("review.guidance.nothing", "x");
    refused("tool_results.limit", "12");
    refused("tool_results.unchanged", "maybe");
    refused("specs.mode", "\"somewhere\"");
    refused("review.passes", "[\"nothing\"]");
    refused("autosubscriptions.ticket", "[\"nothing\"]");
    refused("appearance.shade", "grey");
    assert_eq!(hub_copy(&hub), "[]");
}

#[test]
fn the_rules_at_their_limit_of_four_byte_characters_fit_a_sealed_copy() {
    let (hub, pc, server) = machines();
    let widest = "𝔸".repeat(8_000);

    share(&pc, "pc", "rules", &widest);

    eventually("the widest rules reached the server", || {
        server.user.call("rules.show", json!({})).unwrap()["text"] == widest.as_str()
    });
    let size = hub.with_hub(|hub| hub.shared().iter().map(|sealed| sealed.size()).max());
    assert!(size.unwrap() <= flottille_link::SEALED_LIMIT);
}

#[test]
fn a_hub_keeps_every_name_of_all_machines_and_the_next_ones() {
    // The settings of part 2 and part 3 together are more than the 32 of before.
    assert!(
        flottille_link::MOST_VALUES >= 64,
        "{}",
        flottille_link::MOST_VALUES
    );
}

#[test]
fn without_a_hub_each_setting_is_as_today() {
    let user = User::new();

    let rules = user.call("rules.set", json!({"text": "Mine."})).unwrap();

    assert_eq!(rules["text"], "Mine.", "{rules}");
    assert!(rules["all"].is_null(), "{rules}");
    let (_, refused) = user
        .call("settings.share", json!({"name": "rules", "value": "x"}))
        .unwrap_err();
    assert!(!refused.is_empty());
}
