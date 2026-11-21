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
            "folder = \"{}\"\n\n[tracker]\nkind = \"jira\"\nsite = \"example.atlassian.net\"\n\
             projects = [\"ABC\"]\n",
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

    // The agents first, then the forges and trackers, those the projects use first.
    let order: Vec<&str> = listed["programs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["program"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        ["claude", "opencode", "codex", "acli", "gh", "glab", "bkt"]
    );

    let gh = row(&listed, "gh");
    assert_eq!(gh["kind"], "forge", "{gh}");
    assert_eq!(gh["host"], "github.com", "{gh}");
    assert_eq!(gh["found"], true, "{gh}");
    assert_eq!(gh["version"], "2.60.0", "{gh}");
    assert_eq!(gh["signed_in"], true, "{gh}");
    assert_eq!(gh["account"], "octocat", "{gh}");
    assert_eq!(gh["can_sign_in"], true, "{gh}");

    let glab = row(&listed, "glab");
    assert_eq!(glab["host"], "gitlab.com", "{glab}");
    assert_eq!(glab["signed_in"], false, "{glab}");
    assert_eq!(glab["account"], Value::Null, "{glab}");

    let claude = row(&listed, "claude");
    assert_eq!(claude["kind"], "agent", "{claude}");
    assert_eq!(claude["signed_in"], true, "{claude}");
    assert_eq!(claude["account"], "someone@example.com", "{claude}");

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
    let opencode = user.path("programs/opencode");
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
