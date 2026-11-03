//! `project.programs` (#306): the command lines a project's forges and tracker need, checked.
//! Fakes of `gh`, `bkt` and `acli`, a PATH of the test's own and temporary XDG folders: no real
//! program is ever run, and none of their folders is read.

mod common;

use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A user whose PATH holds the three fakes and nothing else.
fn user_with_all() -> User {
    let user = User::new().with_fake_gh().with_fake_bkt().with_fake_acli();
    let path = ["gh-bin", "bkt-bin", "acli-bin"]
        .map(|dir| user.path(dir).display().to_string())
        .join(":");
    user.with_env("PATH", &path)
}

/// A project `shop`: `api` on GitHub, `web` on Bitbucket Cloud, `portal` on a Data Center server,
/// and Jira for tickets.
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

fn programs(user: &User) -> Value {
    user.call("project.programs", json!({"name": "shop"}))
        .unwrap()
}

fn repository<'a>(read: &'a Value, path: &str) -> &'a Value {
    read["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|state| state["repository"] == path)
        .unwrap_or_else(|| panic!("no {path}: {read}"))
}

#[test]
fn a_repository_s_state_keeps_the_program_s_path_and_names_the_repository_apart() {
    let user = user_with_all();
    shop(&user);

    let read = programs(&user);

    let api = repository(&read, "api");
    assert_eq!(api["repository"], "api");
    assert_eq!(
        api["path"],
        user.path("gh-bin/gh").to_str().unwrap(),
        "`path` is the program's"
    );
    assert_eq!(
        read["tracker"]["path"],
        user.path("acli-bin/acli").to_str().unwrap()
    );
}

#[test]
fn acli_signed_in_to_another_site_says_so_and_how_to_switch() {
    let user = user_with_all();
    shop(&user);
    user.acli_script(json!({"site": "other.atlassian.net"}));

    let tracker = programs(&user)["tracker"].clone();

    assert_eq!(tracker["found"], true);
    assert_eq!(tracker["signed_in"], false, "{tracker}");
    assert_eq!(tracker["signed_in_to"], "other.atlassian.net");
    assert_eq!(
        tracker["sign_in"]["commands"],
        json!(["acli jira auth switch --site example.atlassian.net"])
    );
    assert_eq!(tracker["problem"], Value::Null);
}

#[test]
fn acli_signed_in_to_the_trackers_site_is_signed_in() {
    let user = user_with_all();
    shop(&user);
    // The site is compared as the tickets do: lowercase, without a scheme.
    user.acli_script(json!({"site": "https://Example.Atlassian.net/"}));

    let tracker = programs(&user)["tracker"].clone();

    assert_eq!(tracker["signed_in"], true, "{tracker}");
    assert_eq!(tracker["signed_in_to"], Value::Null);
}

#[test]
fn acli_that_does_not_say_its_site_could_not_be_checked() {
    let user = user_with_all();
    shop(&user);
    user.acli_script(json!({"site_line": false}));

    let tracker = programs(&user)["tracker"].clone();

    assert_eq!(tracker["signed_in"], Value::Null, "{tracker}");
    assert!(
        tracker["problem"].as_str().unwrap().contains("Site"),
        "{tracker}"
    );
}

#[test]
fn bitbucket_cloud_is_signed_in_to_with_the_readme_s_commands() {
    let user = user_with_all();
    shop(&user);
    user.bkt_script(json!({"contexts": []}));

    let read = programs(&user);

    // `bkt`'s README: browser login to Cloud, then a context on the API host with the workspace.
    assert_eq!(
        repository(&read, "web")["sign_in"]["commands"],
        json!([
            "bkt auth login https://bitbucket.org --kind cloud --web",
            "bkt context create bitbucket-org-acme --host api.bitbucket.org --workspace acme"
        ])
    );
}

#[test]
fn bitbucket_data_center_is_signed_in_to_with_the_readme_s_commands() {
    let user = user_with_all();
    shop(&user);
    user.bkt_script(json!({"contexts": []}));

    let read = programs(&user);

    // A personal access token, then a context with the project.
    assert_eq!(
        repository(&read, "portal")["sign_in"]["commands"],
        json!([
            "bkt auth login https://git.example.com --web-token",
            "bkt context create git-example-com-proj --host git.example.com --project PROJ"
        ])
    );
}

#[test]
fn the_context_the_commands_create_is_one_flottille_s_own_check_accepts() {
    // The advice and the check agree: a context made as the commands say makes `bkt` signed in.
    let user = user_with_all();
    shop(&user);
    user.bkt_script(json!({"contexts": [
        {"name": "bitbucket-org-acme", "host": "api.bitbucket.org"},
        {"name": "git-example-com-proj", "host": "git.example.com"}
    ]}));

    let read = programs(&user);

    assert_eq!(repository(&read, "web")["signed_in"], true, "{read}");
    assert_eq!(repository(&read, "portal")["signed_in"], true, "{read}");
}

#[test]
fn the_programs_of_a_project_are_checked_together_not_one_after_the_other() {
    let user = user_with_all();
    shop(&user);
    // Each sign-in check takes 1.5 s, as a program waiting on a slow network does.
    user.gh_script(json!({"delay_ms": 1500}));
    user.bkt_script(json!({"delay_ms": 1500}));
    user.acli_script(json!({"delay_ms": 1500}));

    let begun = Instant::now();
    let read = programs(&user);
    let took = begun.elapsed();

    assert_eq!(read["repositories"].as_array().unwrap().len(), 3);
    // Four checks (gh, acli and bkt for two hosts) of 1.5 s: together, about one of them.
    assert!(
        took < Duration::from_millis(4000),
        "the checks added up: {took:?}"
    );
}
