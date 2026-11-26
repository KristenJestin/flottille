//! Settings for all machines, the guidance and the rest of "Shared, personal" (#1461, #1525,
//! `docs/remote.md`, "Settings for all machines"): the rules for every project, the judge's, the
//! compaction's and Review's guidance, long tool results, Specs' language and mode, the automatic
//! subscriptions, Review's passes and the shade. Each is a value of all machines as the settings a
//! machine often overrides are (#1514): `settings.share {values}` signed by the user's device on
//! one machine, kept by the hub without reading it, read on every machine under that machine's
//! own choice. A hub and two daemons served in the test, each with its own temporary folders and
//! the in-memory keyring; invented texts.

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
/// the tests read it and write it for the machine, as the user's clients do.
struct Case {
    name: &'static str,
    /// What `settings.share` carries: a text, or the JSON text of an extension's setting.
    all: &'static str,
    own: &'static str,
    /// What the machine's agents read now, as text.
    read: fn(&Machine) -> String,
    /// The machine's own value written.
    put: fn(&Machine, &str),
    /// Whether the machine says where the text comes from (`source`), as the guidance files do.
    sourced: bool,
}

fn text_of(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn call(machine: &Machine, method: &str, params: Value) -> Value {
    machine
        .user
        .call(method, params)
        .unwrap_or_else(|error| panic!("{method}: {error:?}"))
}

fn extension(machine: &Machine, name: &str, key: &str) -> Value {
    let listed = call(machine, "extension.list", json!({}));
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

fn set_extension(machine: &Machine, name: &str, key: &str, value: &str) {
    let value: Value = serde_json::from_str(value).unwrap();
    call(
        machine,
        "extension.set",
        json!({"name": name, "settings": {key: value}}),
    );
}

/// The guidance texts: each refuses a secret, on sending and on receipt.
const TEXTS: [&str; 8] = [
    "rules",
    "judge.guidance",
    "compaction.guidance",
    "review.guidance",
    "review.guidance.spec",
    "review.guidance.bugs",
    "review.guidance.standards",
    "review.guidance.security",
];

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "rules",
            all: "Small commits, tests first.",
            own: "Rules of this machine alone.",
            read: |m| text_of(&call(m, "rules.show", json!({}))["text"]),
            put: |m, text| {
                call(m, "rules.set", json!({"text": text}));
            },
            sourced: true,
        },
        Case {
            name: "judge.guidance",
            all: "Let a read of the project through.",
            own: "Ask about every write.",
            read: |m| text_of(&call(m, "judge.show", json!({}))["text"]),
            put: |m, text| {
                call(m, "judge.set", json!({"text": text}));
            },
            sourced: true,
        },
        Case {
            name: "compaction.guidance",
            all: "Keep the decisions and the open questions.",
            own: "Keep the file names.",
            read: |m| text_of(&call(m, "compaction.show", json!({}))["guidance"]["text"]),
            put: |m, text| {
                call(m, "compaction.set_guidance", json!({"text": text}));
            },
            sourced: false,
        },
        Case {
            name: "review.guidance",
            all: "Look at the tests first.",
            own: "Look at the docs first.",
            read: |m| text_of(&call(m, "review.show", json!({}))["text"]),
            put: |m, text| {
                call(m, "review.set", json!({"text": text}));
            },
            sourced: true,
        },
        Case {
            name: "review.guidance.bugs",
            all: "Hunt the off-by-one.",
            own: "Hunt the unwrap.",
            read: |m| text_of(&call(m, "review.show", json!({"part": "bugs"}))["text"]),
            put: |m, text| {
                call(m, "review.set", json!({"part": "bugs", "text": text}));
            },
            sourced: true,
        },
        Case {
            name: "tool_results.limit",
            all: "6000",
            own: "5000",
            read: |m| text_of(&call(m, "tool_results.show", json!({}))["limit"]),
            put: |m, text| {
                let limit: u64 = text.parse().unwrap();
                call(m, "tool_results.set", json!({"limit": limit}));
            },
            sourced: false,
        },
        Case {
            name: "tool_results.unchanged",
            all: "true",
            own: "false",
            read: |m| text_of(&call(m, "tool_results.show", json!({}))["unchanged"]),
            put: |m, text| {
                let on: bool = text.parse().unwrap();
                call(m, "tool_results.set", json!({"unchanged": on}));
            },
            sourced: false,
        },
        Case {
            name: "specs.language",
            all: "\"fr-FR\"",
            own: "\"de\"",
            read: |m| extension(m, "specs", "language").to_string(),
            put: |m, text| set_extension(m, "specs", "language", text),
            sourced: false,
        },
        Case {
            name: "specs.mode",
            all: "\"ticket\"",
            // The default written on a machine is still the machine's own.
            own: "\"internal\"",
            read: |m| extension(m, "specs", "mode").to_string(),
            put: |m, text| set_extension(m, "specs", "mode", text),
            sourced: false,
        },
        Case {
            name: "autosubscriptions.created_pull",
            all: "[\"merged\"]",
            own: "[\"closed\"]",
            read: |m| extension(m, "autosubscriptions", "created_pull").to_string(),
            put: |m, text| set_extension(m, "autosubscriptions", "created_pull", text),
            sourced: false,
        },
        Case {
            name: "autosubscriptions.branch_pull",
            all: "[\"checks\"]",
            own: "[\"comment\"]",
            read: |m| extension(m, "autosubscriptions", "branch_pull").to_string(),
            put: |m, text| set_extension(m, "autosubscriptions", "branch_pull", text),
            sourced: false,
        },
        Case {
            name: "autosubscriptions.ticket",
            all: "[\"closed\"]",
            own: "[]",
            read: |m| extension(m, "autosubscriptions", "ticket").to_string(),
            put: |m, text| set_extension(m, "autosubscriptions", "ticket", text),
            sourced: false,
        },
        Case {
            name: "review.passes",
            all: "[\"security\"]",
            own: "[\"bugs\"]",
            read: |m| extension(m, "review", "passes").to_string(),
            put: |m, text| set_extension(m, "review", "passes", text),
            sourced: false,
        },
        Case {
            name: "appearance.shade",
            all: "light",
            // The default written on a machine is still the machine's own.
            own: "dark",
            read: |m| text_of(&call(m, "appearance.show", json!({}))["shade"]),
            put: |m, text| {
                call(m, "appearance.set", json!({"shade": text}));
            },
            sourced: false,
        },
    ]
}

fn share_line(machine: &Machine, on: &str, values: Value) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    signed(
        &machine.device(),
        &format!("values-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        on,
        "settings.share",
        json!({"values": values}),
    )
}

/// `value` of `name` shared from `machine`, as the user's device signs it there.
fn share(machine: &Machine, on: &str, name: &str, value: &str) -> Value {
    let answer = machine.raw(&share_line(machine, on, json!({name: value})));
    assert!(answer["error"].is_null(), "{name}: {answer}");
    answer["result"].clone()
}

fn refused(machine: &Machine, name: &str, value: &str) -> String {
    error_of(&machine.raw(&share_line(machine, "pc", json!({name: value}))))
}

fn shared(machine: &Machine) -> Value {
    call(machine, "settings.shared", json!({}))
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
        let own = shared(&server)["own"].clone();
        assert!(
            own.as_array().unwrap().contains(&json!(case.name)),
            "{}: {own}",
            case.name
        );
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

        let back = call(&server, "settings.use_all", json!({"names": [case.name]}));

        assert!(
            !back["own"].as_array().unwrap().contains(&json!(case.name)),
            "{}: {back}",
            case.name
        );
        reads(&server, &case, "is back on all machines' value", case.all);
    }
}

#[test]
fn the_text_of_a_guidance_says_where_it_comes_from() {
    let (_hub, pc, server) = machines();
    for case in cases().into_iter().filter(|case| case.sourced) {
        let show = |machine: &Machine| match case.name {
            "rules" => call(machine, "rules.show", json!({})),
            "judge.guidance" => call(machine, "judge.show", json!({})),
            "review.guidance" => call(machine, "review.show", json!({})),
            _ => call(machine, "review.show", json!({"part": "bugs"})),
        };
        assert_ne!(show(&server)["source"], "all", "{}", case.name);

        share(&pc, "pc", case.name, case.all);
        reads(&server, &case, "reaches the server", case.all);
        assert_eq!(show(&server)["source"], "all", "{}", case.name);

        (case.put)(&server, case.own);
        let own = show(&server);
        assert_eq!(own["source"], "global", "{}: {own}", case.name);
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
fn a_secret_written_into_a_guidance_is_never_shared() {
    let (hub, pc, server) = machines();
    call(
        &pc,
        "vault.set",
        json!({"name": "tracker", "secret": SECRET}),
    );
    let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
    for name in TEXTS {
        for leaked in [SECRET.to_owned(), token.clone()] {
            let value = format!("Sign the commits with {leaked} before pushing.");

            let error = refused(&pc, name, &value);

            assert!(
                error.contains("holds a secret: keep secrets in the vault"),
                "{name}: {error}"
            );
            let kept = hub_copy(&hub);
            assert!(
                !kept.contains(name),
                "{name}: the hub keeps nothing: {kept}"
            );
            let mine = std::fs::read_to_string(pc.folder().join("shared.json")).unwrap_or_default();
            assert!(!mine.contains(&leaked), "{name}: kept on pc: {mine}");
            let theirs = shared(&server);
            assert!(!theirs.to_string().contains(&leaked), "{name}: {theirs}");
        }
    }
}

#[test]
fn the_hub_cannot_read_a_value_and_no_secret_is_in_it() {
    let (hub, pc, server) = machines();
    call(
        &pc,
        "vault.set",
        json!({"name": "tracker", "secret": SECRET}),
    );
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
    for name in ["vault.tracker", "variables.TOKEN", "keys.device", "hub"] {
        assert!(!refused(&pc, name, SECRET).is_empty(), "{name}");
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
        .call("settings.share", json!({"values": {"rules": "x"}}))
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
        json!({"values": {"rules": "x"}}),
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
        json!({"values": {"rules": "x"}}),
    ));

    assert!(error_of(&answer).contains("may only view"), "{answer}");
    let rules = call(&server, "rules.show", json!({}));
    assert_ne!(rules["source"], "all", "{rules}");
}

#[test]
fn a_value_that_would_be_refused_on_a_machine_is_refused_for_all() {
    let (hub, pc, _server) = machines();
    for (name, value) in [
        ("rules", "a".repeat(8_001)),
        ("judge.guidance", "a".repeat(4_001)),
        ("compaction.guidance", "a".repeat(4_001)),
        ("review.guidance", "a".repeat(4_001)),
        ("review.guidance.bugs", "a".repeat(4_001)),
        ("review.guidance.nothing", "x".to_owned()),
        ("tool_results.limit", "12".to_owned()),
        ("tool_results.unchanged", "maybe".to_owned()),
        ("specs.mode", "\"somewhere\"".to_owned()),
        ("specs.language", "fr".to_owned()),
        ("review.passes", "[\"nothing\"]".to_owned()),
        ("autosubscriptions.ticket", "[\"nothing\"]".to_owned()),
        ("appearance.shade", "grey".to_owned()),
        ("appearance.theme", "flottille".to_owned()),
    ] {
        assert!(!refused(&pc, name, &value).is_empty(), "{name}");
    }
    assert_eq!(hub_copy(&hub), "[]");
}

#[test]
fn the_rules_at_their_limit_of_four_byte_characters_fit_a_sealed_copy() {
    let (hub, pc, server) = machines();
    let widest = "𝔸".repeat(8_000);

    share(&pc, "pc", "rules", &widest);

    eventually("the widest rules reached the server", || {
        call(&server, "rules.show", json!({}))["text"] == widest.as_str()
    });
    let size = hub.with_hub(|hub| hub.shared().iter().map(|sealed| sealed.size()).max());
    assert!(size.unwrap() <= flottille_link::SEALED_LIMIT);
}

#[test]
fn a_hub_keeps_every_name_of_all_machines() {
    assert!(flottille_link::SHARED_NAMES.len() <= flottille_link::MOST_VALUES);
    for case in cases() {
        assert!(
            flottille_link::SHARED_NAMES.contains(&case.name),
            "{}",
            case.name
        );
    }
}

/// Point 1 of #1525: a file a version of before wrote whole (`appearance.toml` with its theme and
/// shade, `tool-results.toml` with its limit) said Flottille's default for what the user never
/// chose; that is no choice of this machine's, and all machines' value applies over it.
#[test]
fn a_file_of_before_follows_all_machines_unless_set_here() {
    let (_hub, pc, server) = machines();
    let config = server.user.path("config/flottille");
    std::fs::write(
        config.join("appearance.toml"),
        "theme = \"omarchy\"\nshade = \"dark\"\n",
    )
    .unwrap();
    std::fs::write(
        config.join("tool-results.toml"),
        "limit = 8000\nunchanged = true\n",
    )
    .unwrap();

    share(&pc, "pc", "appearance.shade", "light");
    share(&pc, "pc", "tool_results.limit", "6000");

    eventually("the server follows all machines' shade and limit", || {
        let appearance = call(&server, "appearance.show", json!({}));
        let results = call(&server, "tool_results.show", json!({}));
        appearance["shade"] == "light" && results["limit"] == 6000
    });
    let own = shared(&server)["own"].clone();
    assert_eq!(own, json!([]), "nothing is the server's own: {own}");
    // What the file said that is no default is still the machine's.
    let results = call(&server, "tool_results.show", json!({}));
    assert_eq!(results["unchanged"], true, "{results}");
    let theme = call(&server, "appearance.show", json!({}))["theme"].clone();
    assert_eq!(theme, "omarchy");

    // Chosen here, even Flottille's default, it is the machine's own.
    call(&server, "appearance.set", json!({"shade": "dark"}));
    call(&server, "tool_results.set", json!({"limit": 8000}));

    assert_eq!(call(&server, "appearance.show", json!({}))["shade"], "dark");
    assert_eq!(call(&server, "tool_results.show", json!({}))["limit"], 8000);
    let own = shared(&server)["own"].clone();
    for name in ["appearance.shade", "tool_results.limit"] {
        assert!(own.as_array().unwrap().contains(&json!(name)), "{own}");
    }
}

#[test]
fn a_theme_chosen_here_leaves_the_shade_to_all_machines() {
    let (_hub, pc, server) = machines();
    share(&pc, "pc", "appearance.shade", "light");
    eventually("the server is light", || {
        call(&server, "appearance.show", json!({}))["shade"] == "light"
    });

    call(&server, "appearance.set", json!({"theme": "flottille"}));

    let shown = call(&server, "appearance.show", json!({}));
    assert_eq!(shown["shade"], "light", "{shown}");
    assert_eq!(shown["theme"], "flottille", "{shown}");
    assert_eq!(shared(&server)["own"], json!([]));
}

#[test]
fn all_machines_say_the_default_of_each_value() {
    let (_hub, pc, _server) = machines();

    let defaults = shared(&pc)["defaults"].clone();

    for case in cases() {
        assert!(defaults[case.name].is_string(), "{}: {defaults}", case.name);
    }
    assert_eq!(defaults["tool_results.limit"], "8000");
    assert_eq!(defaults["appearance.shade"], "dark");
    assert_eq!(defaults["review.guidance.bugs"], "");
    assert_eq!(
        defaults["rules"],
        call(&pc, "rules.show", json!({}))["default"]
    );
}

#[test]
fn without_a_hub_each_setting_is_as_today() {
    let user = User::new();

    let rules = user.call("rules.set", json!({"text": "Mine."})).unwrap();

    assert_eq!(rules["text"], "Mine.", "{rules}");
    assert_eq!(rules["source"], "global", "{rules}");
    let (_, refused) = user
        .call("settings.share", json!({"values": {"rules": "x"}}))
        .unwrap_err();
    assert!(!refused.is_empty());
}
