//! The "Forges and trackers" part of `flottille doctor` (#306): the command lines the registered
//! projects' forges and trackers need (`gh`, `bkt`, `acli`), found or not, their version, signed in
//! or not, and what to do when not. Fakes of the three, a PATH of the test's own and temporary XDG
//! folders: no real `gh`, `bkt` nor `acli` is ever run, and none of their folders is read.

mod common;

use common::User;
use serde_json::{Value, json};

/// A project `shop` whose repository `api` is on GitHub, `web` on Bitbucket Cloud and `portal` on
/// a Data Center server, and whose tickets are on Jira.
fn shop(user: &User) {
    let folder = user.path("work/shop");
    std::fs::create_dir_all(&folder).unwrap();
    user.write_project_file(
        "shop",
        &format!(
            "folder = \"{}\"\n\n\
             [[repositories]]\npath = \"api\"\nforge = {{ kind = \"github\", repository = \"acme/api\" }}\n\n\
             [[repositories]]\npath = \"web\"\nforge = {{ kind = \"bitbucket-cloud\", repository = \"acme/web\" }}\n\n\
             [[repositories]]\npath = \"portal\"\nforge = {{ kind = \"bitbucket-data-center\", host = \"git.example.com\", repository = \"PROJ/portal\" }}\n\n\
             [tracker]\nkind = \"jira\"\nsite = \"example.atlassian.net\"\nprojects = [\"ABC\"]\n",
            folder.display()
        ),
    );
}

/// A user whose PATH holds the three fakes and nothing else.
fn user_with_all() -> User {
    let user = User::new().with_fake_gh().with_fake_bkt().with_fake_acli();
    // Only the fakes: not a `gh` of the machine, if it has one.
    let path = ["gh-bin", "bkt-bin", "acli-bin"]
        .map(|dir| user.path(dir).display().to_string())
        .join(":");
    user.with_env("PATH", &path)
}

fn doctor(user: &User) -> (bool, Value, String) {
    let output = user.run(&["doctor", "--json"]);
    let report = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
    let text = String::from_utf8_lossy(&user.run(&["doctor"]).stdout).into_owned();
    (output.status.success(), report, text)
}

/// The report's entry for `program`, on `host`.
fn entry<'a>(report: &'a Value, program: &str, host: &str) -> &'a Value {
    report["forges_and_trackers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["program"] == program && entry["host"] == host)
        .unwrap_or_else(|| panic!("no {program} on {host}: {report}"))
}

#[test]
fn doctor_lists_the_forge_and_tracker_programs_the_projects_use() {
    let user = user_with_all();
    shop(&user);
    user.bkt_script(json!({"contexts": [
        {"name": "cloud", "host": "bitbucket.org"},
        {"name": "work", "host": "git.example.com"}
    ]}));

    let (_, report, text) = doctor(&user);

    let gh = entry(&report, "gh", "github.com");
    assert_eq!(gh["found"], true);
    assert_eq!(gh["path"], user.path("gh-bin/gh").to_str().unwrap());
    assert!(
        gh["version"].as_str().is_some_and(|v| v.contains('.')),
        "{gh}"
    );
    assert_eq!(gh["signed_in"], true);
    assert_eq!(gh["used_by"], json!(["shop: api"]));
    let acli = entry(&report, "acli", "example.atlassian.net");
    assert_eq!(acli["found"], true);
    assert_eq!(acli["signed_in"], true);
    assert_eq!(acli["used_by"], json!(["shop: tracker"]));
    // `bkt` is asked once for each host the projects use: the contexts decide.
    assert_eq!(entry(&report, "bkt", "bitbucket.org")["signed_in"], true);
    assert_eq!(entry(&report, "bkt", "git.example.com")["signed_in"], true);
    assert_eq!(report["warnings"], json!([]), "{report}");

    assert!(text.contains("Forges and trackers"), "{text}");
    for word in ["gh", "bkt", "acli", "bitbucket.org", "git.example.com"] {
        assert!(text.contains(word), "{word}: {text}");
    }
    // What the programs print of the account is never kept, and no token is read.
    assert!(!report.to_string().contains("someone"), "{report}");
    assert!(!text.contains("someone"), "{text}");
}

#[test]
fn doctor_names_only_the_programs_the_projects_use() {
    // A GitHub forge and no tracker: `bkt` and `acli` are not asked, not even named.
    let user = user_with_all();
    let folder = user.path("work/solo");
    std::fs::create_dir_all(&folder).unwrap();
    user.write_project_file(
        "solo",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \"api\"\nforge = {{ kind = \"github\", repository = \"acme/api\" }}\n",
            folder.display()
        ),
    );

    let (_, report, text) = doctor(&user);

    assert_eq!(report["forges_and_trackers"].as_array().unwrap().len(), 1);
    assert_eq!(report["forges_and_trackers"][0]["program"], "gh");
    assert!(!text.contains("acli") && !text.contains("bkt"), "{text}");
    assert!(user.bkt_received().is_empty(), "bkt was never run");
    assert!(user.acli_received().is_empty(), "acli was never run");
}

#[test]
fn a_repository_with_no_forge_and_a_project_with_no_tracker_need_no_program() {
    let user = user_with_all();
    let folder = user.path("work/plain");
    std::fs::create_dir_all(&folder).unwrap();
    user.write_project_file(
        "plain",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \"api\"\n",
            folder.display()
        ),
    );

    let (_, report, text) = doctor(&user);

    assert_eq!(report["forges_and_trackers"], json!([]), "{report}");
    assert!(!text.contains("Forges and trackers"), "{text}");
    assert!(user.gh_received().is_empty(), "gh was never run");
}

#[test]
fn a_missing_forge_program_is_a_doctor_warning_not_a_failure() {
    // A PATH with the fake `gh` and `acli` only: `bkt` is not there, the machine's own included.
    let user = User::new().with_fake_gh().with_fake_acli();
    let path = ["gh-bin", "acli-bin"]
        .map(|dir| user.path(dir).display().to_string())
        .join(":");
    let user = user.with_env("PATH", &path);
    // What `ready` says before any project is registered: the providers', as it was.
    let providers = user.run(&["doctor", "--json"]);
    let before = serde_json::from_slice::<Value>(&providers.stdout).unwrap()["ready"].clone();
    let exit_before = providers.status.code();
    shop(&user);

    let after = user.run(&["doctor", "--json"]);
    assert_eq!(
        after.status.code(),
        exit_before,
        "a missing forge program changes no exit code"
    );
    let (_, report, text) = doctor(&user);

    let bkt = entry(&report, "bkt", "bitbucket.org");
    assert_eq!(bkt["found"], false, "{bkt}");
    assert_eq!(bkt["signed_in"], Value::Null, "{bkt}");
    assert_eq!(
        bkt["install"]["page"],
        "https://github.com/avivsinai/bitbucket-cli"
    );
    // The rest of Flottille works: what `ready` says is the providers', as it was without projects.
    assert_eq!(report["ready"], before, "{report}");
    let warnings = report["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("bkt") && w.as_str().unwrap().contains("install")),
        "{warnings:?}"
    );
    assert!(text.contains("warn bkt"), "{text}");
    assert!(
        text.contains("https://github.com/avivsinai/bitbucket-cli"),
        "{text}"
    );
    // Said once, in the part, not again among the other warnings.
    assert_eq!(text.matches("warn bkt").count(), 2, "one per host: {text}");
}

#[test]
fn a_signed_out_program_gives_the_command_to_sign_in_in_the_doctor() {
    let user = user_with_all();
    shop(&user);
    user.gh_script(json!({"signed_in": false}));
    user.acli_script(json!({"signed_in": false}));
    user.bkt_script(json!({"contexts": []}));

    let (_, report, text) = doctor(&user);

    assert_eq!(
        entry(&report, "gh", "github.com")["sign_in"]["commands"],
        json!(["gh auth login"])
    );
    assert_eq!(
        entry(&report, "acli", "example.atlassian.net")["sign_in"]["commands"],
        json!(["acli jira auth login"])
    );
    assert_eq!(
        entry(&report, "bkt", "git.example.com")["sign_in"]["commands"],
        json!([
            "bkt auth login https://git.example.com --web-token",
            "bkt context create git-example-com-proj --host git.example.com --project PROJ"
        ])
    );
    assert_eq!(
        entry(&report, "bkt", "bitbucket.org")["sign_in"]["commands"],
        json!([
            "bkt auth login https://bitbucket.org --kind cloud --web",
            "bkt context create bitbucket-org-acme --host api.bitbucket.org --workspace acme"
        ])
    );
    assert_eq!(entry(&report, "gh", "github.com")["signed_in"], false);
    for command in [
        "gh auth login",
        "acli jira auth login",
        "bkt auth login https://bitbucket.org --kind cloud --web",
    ] {
        assert!(text.contains(command), "{command}: {text}");
    }
    // `bkt auth status` is never asked: a context for the host decides.
    for call in user.bkt_received() {
        assert_ne!(call["args"][0], "auth", "{call}");
    }
}
