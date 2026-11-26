//! Signing the agents and the forge programs in from the window (#1185, `docs/agents.md` and
//! `docs/forges.md`, "Signing in", `docs/protocol.md`, "The sign-in methods"): each program says
//! whether it is signed in by its own status, its own sign-in runs without a shell and shows the
//! address and the code, a pasted code goes to it once, and nothing a program prints is kept.
//! The programs are fakes in the user's own folder of programs (`FLOTTILLE_TEST_PROGRAMS`): no
//! real program is ever run, and no real home is read.

use crate::common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A secret-shaped token line `gh auth status` prints, which nothing may keep.
const TOKEN: &str = "gho_inventedTokenOfATestOnly1234567890";

fn list(user: &User) -> Value {
    user.call("signin.list", json!({})).unwrap()
}

fn row(list: &Value, program: &str) -> Value {
    list["programs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["program"] == program)
        .unwrap_or_else(|| panic!("no {program}: {list}"))
        .clone()
}

/// Waits until `check` holds, 20 s at most.
fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Every file under `folder` that holds `text`.
fn files_holding(folder: &Path, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            found.extend(files_holding(&path, text));
        } else if kind.is_file()
            && let Ok(bytes) = std::fs::read(&path)
            && String::from_utf8_lossy(&bytes).contains(text)
        {
            found.push(path.display().to_string());
        }
    }
    found
}

/// What Flottille keeps of the user: its store, its configuration, its data, its logs and its
/// runtime folder. The fakes' own folders are the test's, not Flottille's.
fn kept_holding(user: &User, text: &str) -> Vec<String> {
    ["state", "config", "data", "runtime", "home"]
        .iter()
        .flat_map(|sub| files_holding(&user.path(sub), text))
        .collect()
}

/// Every event the daemon stored, from the first.
fn events(user: &User) -> Vec<Value> {
    user.watch(&["--after", "0"])
        .within(Duration::from_millis(1500))
}

/// A project whose tracker is Jira: `acli` is a program the projects use.
fn tracked(user: &User) {
    let folder = user.path("work/shop");
    std::fs::create_dir_all(&folder).unwrap();
    user.write_project_file(
        "shop",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \"api\"\n\n[tracker]\nkind = \"jira\"\n\
             site = \"example.atlassian.net\"\nprojects = [\"ABC\"]\n",
            folder.display()
        ),
    );
}

#[test]
fn each_program_says_whether_it_is_signed_in() {
    let user = User::new().with_sign_in_programs(&["gh", "glab", "claude", "codex"]);
    user.sign_in_script("gh", json!({"user": {"login": "octocat"}}));
    user.sign_in_script("glab", json!({"signed_in": false}));
    user.sign_in_script("claude", json!({"account": "someone@example.com"}));
    user.sign_in_script("codex", json!({"signed_in": false}));
    tracked(&user);

    let listed = list(&user);

    // The agents first, then the forges and trackers, always in the page's own order.
    let order: Vec<&str> = listed["programs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["program"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        ["claude", "opencode", "codex", "gh", "acli", "bkt", "glab"]
    );

    let gh = row(&listed, "gh");
    assert_eq!(gh["kind"], "forge", "{gh}");
    assert_eq!(gh["host"], "github.com", "{gh}");
    assert_eq!(gh["found"], true, "{gh}");
    assert_eq!(gh["version"], "2.60.0", "{gh}");
    assert_eq!(gh["signed_in"], true, "{gh}");
    assert_eq!(gh["account"], "o***", "{gh}");
    assert_eq!(gh["can_sign_in"], true, "{gh}");

    let glab = row(&listed, "glab");
    assert_eq!(glab["host"], "gitlab.com", "{glab}");
    assert_eq!(glab["signed_in"], false, "{glab}");
    assert_eq!(glab["account"], Value::Null, "{glab}");

    let claude = row(&listed, "claude");
    assert_eq!(claude["kind"], "agent", "{claude}");
    assert_eq!(claude["signed_in"], true, "{claude}");
    assert_eq!(claude["account"], "s***@e***", "{claude}");

    assert_eq!(row(&listed, "codex")["signed_in"], false);

    for missing in ["opencode", "acli", "bkt"] {
        let missing = row(&listed, missing);
        assert_eq!(missing["found"], false, "{missing}");
        assert_eq!(missing["signed_in"], Value::Null, "{missing}");
        assert_eq!(missing["can_sign_in"], false, "{missing}");
    }
    assert_eq!(row(&listed, "acli")["host"], "example.atlassian.net");

    // Each said it by its own status, and none was asked for its token.
    let gh_calls: Vec<Value> = std::fs::read_to_string(user.path("sign-in-gh/received.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        gh_calls
            .iter()
            .any(|call| call["args"] == json!(["auth", "status", "--hostname", "github.com"])),
        "{gh_calls:?}"
    );
    assert!(
        !gh_calls
            .iter()
            .any(|call| call["args"].to_string().contains("token")),
        "{gh_calls:?}"
    );
}

#[test]
fn a_status_output_never_reaches_the_store_nor_the_log() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    user.sign_in_script(
        "gh",
        json!({"user": {"login": "octocat"}, "status_token": TOKEN}),
    );
    user.run(&["developer", "on"]);

    let listed = list(&user);

    assert_eq!(row(&listed, "gh")["signed_in"], true);
    assert!(!listed.to_string().contains(TOKEN), "{listed}");
    assert!(!listed.to_string().contains("Token"), "{listed}");
    // Signed out and back in: the program's every output passed through the daemon.
    user.call("signin.sign_out", json!({"program": "gh"}))
        .unwrap();
    user.call("signin.start", json!({"program": "gh"})).unwrap();
    user.approve_sign_in("gh");
    eventually("gh signed in again", || {
        row(&list(&user), "gh")["signed_in"] == true
    });

    assert_eq!(kept_holding(&user, TOKEN), Vec::<String>::new());
    assert_eq!(kept_holding(&user, "Logged in to"), Vec::<String>::new());
    assert!(
        !events(&user).iter().any(|event| {
            let said = event.to_string();
            said.contains(TOKEN) || said.contains("octocat")
        }),
        "an event holds what gh printed"
    );
}

#[test]
fn sign_in_shows_the_address_and_the_code_and_ends_signed_in() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    user.sign_in_script(
        "gh",
        json!({"signed_in": false,
               "sign_in": {"address": "https://github.com/login/device", "code": "ABCD-1234"}}),
    );
    assert_eq!(row(&list(&user), "gh")["signed_in"], false);

    let started = user.call("signin.start", json!({"program": "gh"})).unwrap();

    assert_eq!(started["running"], true, "{started}");
    assert_eq!(started["address"], "https://github.com/login/device");
    assert_eq!(started["code"], "ABCD-1234");
    assert_eq!(started["asks_code"], false);
    let login = user.sign_in_record("gh");
    assert_eq!(
        login[0]["login"],
        json!([
            "auth",
            "login",
            "--web",
            "--hostname",
            "github.com",
            "--git-protocol",
            "https"
        ])
    );
    assert_eq!(row(&list(&user), "gh")["running"], true);

    user.approve_sign_in("gh");
    eventually("gh signed in", || {
        let gh = row(&list(&user), "gh");
        gh["signed_in"] == true && gh["running"] == false
    });
    let changed: Vec<Value> = events(&user)
        .into_iter()
        .filter(|event| event["name"] == "signin.changed")
        .collect();
    assert_eq!(
        changed.last().map(|event| event["data"].clone()),
        Some(json!({"program": "gh", "host": "github.com", "state": "signed_in"})),
        "{changed:?}"
    );
    assert!(
        !changed
            .iter()
            .any(|event| event.to_string().contains("ABCD-1234")),
        "{changed:?}"
    );
}

#[test]
fn a_pasted_code_is_written_once_and_never_kept() {
    const PASTED: &str = "pasted-code-of-a-test-0042";
    let user = User::new().with_sign_in_programs(&["claude"]);
    user.sign_in_script(
        "claude",
        json!({"signed_in": false, "sign_in": {"accepts": PASTED}}),
    );

    let started = user
        .call("signin.start", json!({"program": "claude"}))
        .unwrap();
    assert_eq!(started["asks_code"], true, "{started}");
    assert_eq!(started["address"], "https://login.example.test/device");
    assert_eq!(
        user.sign_in_record("claude")[0]["login"],
        json!(["auth", "login"])
    );

    user.call("signin.code", json!({"program": "claude", "code": PASTED}))
        .unwrap();
    let again = user.call("signin.code", json!({"program": "claude", "code": PASTED}));
    assert!(again.is_err(), "a second code is refused: {again:?}");

    eventually("claude signed in", || {
        row(&list(&user), "claude")["signed_in"] == true
    });
    let pasted: Vec<Value> = user
        .sign_in_record("claude")
        .into_iter()
        .filter(|entry| entry.get("pasted").is_some() || entry.get("after").is_some())
        .collect();
    assert_eq!(pasted, [json!({"pasted": PASTED})]);
    assert_eq!(kept_holding(&user, PASTED), Vec::<String>::new());
    assert!(
        !events(&user)
            .iter()
            .any(|event| event.to_string().contains(PASTED)),
        "an event holds the pasted code"
    );
}

#[test]
fn cancel_stops_the_sign_in() {
    let user = User::new().with_sign_in_programs(&["codex"]);
    user.sign_in_script("codex", json!({"signed_in": false}));

    let started = user
        .call("signin.start", json!({"program": "codex"}))
        .unwrap();
    assert_eq!(started["code"], "WDJB-MJHT", "{started}");
    assert_eq!(
        user.sign_in_record("codex")[0]["login"],
        json!(["login", "--device-auth"])
    );
    let pid = user.sign_in_record("codex")[0]["pid"].as_u64().unwrap();

    user.call("signin.cancel", json!({"program": "codex"}))
        .unwrap();

    eventually("the sign-in's process ended", || {
        !Path::new(&format!("/proc/{pid}")).exists()
    });
    let codex = row(&list(&user), "codex");
    assert_eq!(codex["running"], false, "{codex}");
    assert_eq!(codex["signed_in"], false, "{codex}");
    assert!(
        events(&user)
            .iter()
            .any(|event| event["name"] == "signin.changed"
                && event["data"]
                    == json!({"program": "codex", "host": null, "state": "cancelled"})),
        "no cancel said"
    );
}

#[test]
fn sign_out_runs_the_program_s_own_sign_out() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    assert_eq!(row(&list(&user), "gh")["signed_in"], true);

    let answer = user
        .call("signin.sign_out", json!({"program": "gh"}))
        .unwrap();

    assert_eq!(answer["signed_in"], false, "{answer}");
    let logouts: Vec<Value> = user
        .sign_in_record("gh")
        .into_iter()
        .filter_map(|entry| entry.get("logout").cloned())
        .collect();
    assert_eq!(
        logouts,
        [json!(["auth", "logout", "--hostname", "github.com"])]
    );
    assert_eq!(row(&list(&user), "gh")["signed_in"], false);
}

#[test]
fn a_program_without_a_flow_says_the_line_to_type() {
    let user = User::new().with_sign_in_programs(&["bkt"]);
    // OpenCode's own sign-in asks its questions in a terminal: a stand-in that only says its
    // version is all the row needs.
    let opencode = user.path("sign-in-bin/opencode");
    std::fs::write(&opencode, "#!/bin/sh\necho 1.19.0\n").unwrap();
    let mut permissions = std::fs::metadata(&opencode).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&opencode, permissions).unwrap();
    // A Bitbucket Data Center server signs in with a token typed in a terminal.
    let folder = user.path("work/portal");
    std::fs::create_dir_all(&folder).unwrap();
    user.write_project_file(
        "portal",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \".\"\nforge = {{ kind = \
             \"bitbucket-data-center\", host = \"git.example.com\", repository = \"PROJ/portal\" \
             }}\n",
            folder.display()
        ),
    );

    let listed = list(&user);

    let opencode = row(&listed, "opencode");
    assert_eq!(opencode["found"], true, "{opencode}");
    assert_eq!(opencode["can_sign_in"], false, "{opencode}");
    assert_eq!(opencode["line"], "opencode auth login", "{opencode}");
    let bkt = row(&listed, "bkt");
    assert_eq!(bkt["host"], "git.example.com", "{bkt}");
    assert_eq!(bkt["can_sign_in"], false, "{bkt}");
    assert_eq!(
        bkt["line"], "bkt auth login https://git.example.com --web-token",
        "{bkt}"
    );
    let refused = user
        .call("signin.start", json!({"program": "opencode"}))
        .unwrap_err();
    assert_eq!(refused.0, "refused", "{refused:?}");
    assert!(
        refused.1.contains("`opencode auth login`") && refused.1.contains("terminal"),
        "{refused:?}"
    );
}

/// #1185 round 2: the account a status names is masked for real, its first letter and its
/// domain's first letter kept at most, wherever the list is asked from.
#[test]
fn an_account_name_is_shown_masked() {
    let user = User::new().with_sign_in_programs(&["gh", "claude"]);
    user.sign_in_script("gh", json!({"user": {"login": "octocat"}}));
    user.sign_in_script("claude", json!({"account": "someone@example.com"}));

    let listed = list(&user);

    assert_eq!(row(&listed, "gh")["account"], "o***");
    assert_eq!(row(&listed, "claude")["account"], "s***@e***");
    for said in ["octocat", "someone", "example.com"] {
        assert!(!listed.to_string().contains(said), "{said}: {listed}");
    }
}

/// #1185 round 2: a sign-in started while the last one of the same program is ending (its
/// status still being read) is its own: the end of the last one leaves it running, and it can be
/// cancelled.
#[test]
fn a_sign_in_started_while_the_last_one_ends_can_be_cancelled() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    // Each status takes 3 s: the window in which the last sign-in is ending.
    user.sign_in_script("gh", json!({"signed_in": false, "delay_ms": 3000}));
    let logins = |user: &User| {
        user.sign_in_record("gh")
            .into_iter()
            .filter(|entry| entry.get("login").is_some())
            .collect::<Vec<_>>()
    };

    user.call("signin.start", json!({"program": "gh"})).unwrap();
    std::fs::write(user.path("sign-in-gh/refuse"), "").unwrap();
    eventually("the first sign-in ended refused", || {
        user.sign_in_record("gh")
            .iter()
            .any(|entry| entry["ended"] == "refused")
    });
    std::fs::remove_file(user.path("sign-in-gh/refuse")).unwrap();
    // Started again while the first one's status is read: a second program runs.
    let deadline = Instant::now() + Duration::from_millis(2500);
    while logins(&user).len() < 2 {
        assert!(Instant::now() < deadline, "no second sign-in started");
        user.call("signin.start", json!({"program": "gh"})).unwrap();
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid = logins(&user)[1]["pid"].as_u64().unwrap();
    // The first one's end is said once its status was read.
    eventually("the first sign-in's end was said", || {
        events(&user)
            .iter()
            .any(|event| event["name"] == "signin.changed" && event["data"]["state"] == "failed")
    });

    let cancelled = user.call("signin.cancel", json!({"program": "gh"}));

    assert!(cancelled.is_ok(), "{cancelled:?}");
    eventually("the second sign-in's process ended", || {
        !Path::new(&format!("/proc/{pid}")).exists()
    });
}

/// #1248: gh as it was before (the code after a colon) and gh as 2.104.0 prints it without a
/// clipboard utility (a sentence under the line that names the code) are both read right.
#[test]
fn the_code_of_every_wording_of_gh_is_shown() {
    for wording in ["older", "no clipboard"] {
        let user = User::new().with_sign_in_programs(&["gh"]);
        user.sign_in_script(
            "gh",
            json!({"signed_in": false,
                   "sign_in": {"address": "https://github.com/login/device",
                               "code": "ABCD-1234", "wording": wording}}),
        );

        let started = user.call("signin.start", json!({"program": "gh"})).unwrap();

        assert_eq!(started["code"], "ABCD-1234", "{wording}: {started}");
        assert_eq!(
            started["address"], "https://github.com/login/device",
            "{wording}"
        );
        user.call("signin.cancel", json!({"program": "gh"}))
            .unwrap();
    }
}

/// #1185 round 2: an address a program prints with a user and a token in it is shown without
/// them, and the code beside it is left as it is.
#[test]
fn an_address_is_shown_without_its_credentials() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    user.sign_in_script(
        "gh",
        json!({"signed_in": false,
               "sign_in": {"address": "https://oauth2:glpat-inventedTokenOfATest0042@git.example.com/login/device",
                           "code": "ABCD-1234"}}),
    );

    let started = user.call("signin.start", json!({"program": "gh"})).unwrap();

    assert_eq!(started["address"], "https://git.example.com/login/device");
    assert_eq!(started["code"], "ABCD-1234");
    assert!(!started.to_string().contains("glpat"), "{started}");
    user.call("signin.cancel", json!({"program": "gh"}))
        .unwrap();
}

/// #1185 round 2: the list runs each program's status once for callers that ask it together or
/// right after one another, not once each.
#[test]
fn a_list_asked_at_once_runs_each_status_once() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    user.sign_in_script("gh", json!({"delay_ms": 500}));

    std::thread::scope(|scope| {
        let asked: Vec<_> = (0..3).map(|_| scope.spawn(|| list(&user))).collect();
        for asked in asked {
            assert_eq!(row(&asked.join().unwrap(), "gh")["signed_in"], true);
        }
    });
    assert_eq!(row(&list(&user), "gh")["signed_in"], true);

    let statuses = std::fs::read_to_string(user.path("sign-in-gh/received.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| line.contains(r#""args":["auth","status""#))
        .count();
    assert_eq!(statuses, 1);
}

/// The programs of the list, in the order it gives them.
fn order(list: &Value) -> Vec<String> {
    list["programs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["program"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_rows_keep_their_order_whatever_the_projects_use() {
    let user = User::new().with_sign_in_programs(&["gh", "glab", "acli", "bkt"]);
    let bare = order(&list(&user));
    tracked(&user);
    let with_a_tracker = order(&list(&user));

    let page = ["claude", "opencode", "codex", "gh", "acli", "bkt", "glab"];
    assert_eq!(bare, page);
    assert_eq!(with_a_tracker, page);
}

#[test]
fn a_cli_that_says_it_is_not_signed_in_shows_sign_in() {
    let user = User::new().with_sign_in_programs(&["gh", "glab", "acli", "bkt"]);
    for program in ["gh", "glab", "acli", "bkt"] {
        user.sign_in_script(program, json!({"signed_in": false}));
    }

    let listed = list(&user);

    for program in ["gh", "glab", "acli", "bkt"] {
        let said = row(&listed, program);
        assert_eq!(said["signed_in"], false, "{said}");
        assert_eq!(said["problem"], Value::Null, "{said}");
        assert_eq!(said["can_sign_in"], true, "{said}");
    }
}

#[test]
fn a_cli_that_cannot_reach_its_service_shows_could_not_check_with_its_reason() {
    let user = User::new().with_sign_in_programs(&["gh", "glab", "acli", "bkt"]);
    let said = [
        ("gh", "error connecting to api.github.com"),
        ("glab", "502 Bad Gateway"),
        ("acli", "✗ Error: Something went wrong"),
        ("bkt", "Error: 503 Service Unavailable (api.bitbucket.org)"),
    ];
    for (program, why) in said {
        // A token the command line printed with its error is never shown.
        let failing = format!("{why}\n  - Token: {TOKEN}");
        user.sign_in_script(
            program,
            json!({"status_fail": failing, "user": {"login": "octocat"}}),
        );
    }

    let listed = list(&user);

    for (program, why) in said {
        let row = row(&listed, program);
        assert_eq!(row["found"], true, "{row}");
        assert_eq!(row["signed_in"], Value::Null, "{row}");
        // glab's own line says its API call failed, not the host it asked.
        let why = match program {
            "glab" => format!("API call failed: {why}"),
            _ => why.to_owned(),
        };
        assert_eq!(row["problem"], why, "{row}");
    }
    assert!(!listed.to_string().contains(TOKEN), "{listed}");
    assert!(!listed.to_string().contains("octocat"), "{listed}");
}

#[test]
fn a_check_that_hangs_is_could_not_check_with_no_answer_in_n_seconds() {
    let user = User::new()
        .with_sign_in_programs(&["gh", "acli"])
        .with_env("FLOTTILLE_FORGE_TIMEOUT_MS", "300");
    user.sign_in_script("gh", json!({"delay_ms": 5000}));
    user.sign_in_script("acli", json!({"delay_ms": 5000}));

    let listed = list(&user);

    for program in ["gh", "acli"] {
        let row = row(&listed, program);
        assert_eq!(row["signed_in"], Value::Null, "{row}");
        assert_eq!(row["problem"], "no answer in 1 s", "{row}");
    }
}

#[test]
fn a_list_asked_without_its_checks_runs_none_and_a_row_asked_alone_is_checked() {
    let user = User::new().with_sign_in_programs(&["gh", "acli"]);
    user.sign_in_script("gh", json!({"user": {"login": "octocat"}}));

    let unchecked = user.call("signin.list", json!({"checked": false})).unwrap();

    assert_eq!(
        order(&unchecked),
        ["claude", "opencode", "codex", "gh", "acli", "bkt", "glab"]
    );
    for said in unchecked["programs"].as_array().unwrap() {
        assert_eq!(said["checking"], true, "{said}");
        assert_eq!(said["signed_in"], Value::Null, "{said}");
    }
    assert!(
        !user.path("sign-in-gh/received.jsonl").exists(),
        "no check was run"
    );

    let alone = user.call("signin.list", json!({"program": "gh"})).unwrap();

    assert_eq!(order(&alone), ["gh"]);
    let gh = row(&alone, "gh");
    assert_eq!(gh["checking"], false, "{gh}");
    assert_eq!(gh["signed_in"], true, "{gh}");
}

/// The last `signin.changed` that said `state` for `program`.
fn changed_to(user: &User, program: &str, state: &str) -> Option<Value> {
    events(user)
        .into_iter()
        .filter(|event| event["name"] == "signin.changed")
        .map(|event| event["data"].clone())
        .rfind(|data| data["program"] == program && data["state"] == state)
}

#[test]
fn acli_signs_in_to_the_row_s_site_from_the_window() {
    let user = User::new().with_sign_in_programs(&["acli"]);
    tracked(&user);
    user.sign_in_script(
        "acli",
        json!({"signed_in": false, "sign_in": {"address": "https://id.example.test/authorize"}}),
    );

    let started = user
        .call("signin.start", json!({"program": "acli"}))
        .unwrap();

    assert_eq!(started["running"], true, "{started}");
    assert_eq!(started["address"], "https://id.example.test/authorize");
    assert_eq!(
        user.sign_in_record("acli")[0]["login"],
        json!([
            "jira",
            "auth",
            "login",
            "--web",
            "--site",
            "example.atlassian.net"
        ])
    );
    user.approve_sign_in("acli");
    eventually("acli signed in", || {
        let acli = row(&list(&user), "acli");
        acli["signed_in"] == true && acli["running"] == false
    });
}

#[test]
fn a_failed_sign_in_says_what_the_program_said() {
    let user = User::new().with_sign_in_programs(&["gh", "glab", "acli", "bkt"]);
    tracked(&user);
    for program in ["gh", "glab", "acli", "bkt"] {
        // A token the program printed with its error is masked, as every output is.
        let said = format!("Error: {program} said this on its error output\n  Token: {TOKEN}");
        user.sign_in_script(
            program,
            json!({"signed_in": false, "sign_in": {"fail": said}}),
        );
    }

    for program in ["gh", "glab", "acli", "bkt"] {
        user.call("signin.start", json!({"program": program}))
            .unwrap();
        eventually(&format!("{program}'s sign-in failed"), || {
            changed_to(&user, program, "failed").is_some()
        });
        let failed = changed_to(&user, program, "failed").unwrap();
        let why = failed["why"].as_str().unwrap_or_default();
        assert!(
            why.contains(&format!("Error: {program} said this on its error output")),
            "{failed}"
        );
        assert!(why.contains("exit code 1"), "{failed}");
        assert!(!why.contains(TOKEN), "{failed}");
    }
    assert!(
        !events(&user)
            .iter()
            .any(|event| event.to_string().contains(TOKEN)),
        "an event holds the token"
    );
}

#[test]
fn a_failed_sign_in_never_says_an_address_or_a_code() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    // What a program prints for the dialog, on its error output too (gh does), then why it failed.
    let said = "Open https://id.example.test/device?code=XYZW\n\
                one-time code: ABCD-1234\n\
                Error: the sign-in needs a terminal";
    user.sign_in_script("gh", json!({"signed_in": false, "sign_in": {"fail": said}}));

    user.call("signin.start", json!({"program": "gh"})).unwrap();

    eventually("gh's sign-in failed", || {
        changed_to(&user, "gh", "failed").is_some()
    });
    let failed = changed_to(&user, "gh", "failed").unwrap();
    let why = failed["why"].as_str().unwrap_or_default();
    assert!(
        why.contains("Error: the sign-in needs a terminal"),
        "{failed}"
    );
    for never in ["id.example.test", "XYZW", "ABCD-1234", "one-time code"] {
        assert!(!why.contains(never), "{never}: {failed}");
    }
}

#[test]
fn a_failed_sign_in_never_repeats_the_code_the_user_pasted() {
    const PASTED: &str = "pasted-code-of-a-test-0042";
    let user = User::new().with_sign_in_programs(&["claude"]);
    // The code is refused, and the program repeats it in its error, as some do.
    user.sign_in_script(
        "claude",
        json!({"signed_in": false, "sign_in": {"accepts": "another-code", "echo_refused": true}}),
    );
    user.call("signin.start", json!({"program": "claude"}))
        .unwrap();

    user.call("signin.code", json!({"program": "claude", "code": PASTED}))
        .unwrap();

    eventually("claude's sign-in failed", || {
        changed_to(&user, "claude", "failed").is_some()
    });
    let failed = changed_to(&user, "claude", "failed").unwrap();
    let why = failed["why"].as_str().unwrap_or_default();
    assert!(why.contains("Error: the sign-in was refused"), "{failed}");
    assert!(!why.contains(PASTED), "{failed}");
    assert!(
        !events(&user)
            .iter()
            .any(|event| event.to_string().contains(PASTED)),
        "an event holds the pasted code"
    );
}

#[test]
fn a_failed_sign_in_says_only_the_last_lines_it_said() {
    let user = User::new().with_sign_in_programs(&["gh"]);
    let lines: Vec<String> = (1..=8).map(|n| format!("line {n} of the error")).collect();
    user.sign_in_script(
        "gh",
        json!({"signed_in": false, "sign_in": {"fail": lines.join("\n")}}),
    );

    user.call("signin.start", json!({"program": "gh"})).unwrap();

    eventually("gh's sign-in failed", || {
        changed_to(&user, "gh", "failed").is_some()
    });
    let failed = changed_to(&user, "gh", "failed").unwrap();
    let why = failed["why"].as_str().unwrap_or_default();
    assert!(why.contains("line 8 of the error"), "{failed}");
    assert!(why.contains("line 6 of the error"), "{failed}");
    assert!(!why.contains("line 5 of the error"), "{failed}");
}
