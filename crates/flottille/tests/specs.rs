//! The scenarios of Specs, the first extension (`docs/extensions.md`, "Specs"), and of the part of
//! the frame it builds: the switch in `extensions.toml` and in a project's file, `extension.list`
//! and `extension.set`, the extension's tables, and the spec kept in them with its sections, its
//! statuses, its lock, its mode and its tickets. Temporary XDG folders, throwaway projects, a Jira
//! tracker that is declared and never read.

mod common;

use common::User;
use common::mcp::Mcp;
use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;

/// A project `name` over a throwaway repository, its tracker Jira's ABC project.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    user.write_project_file(
        name,
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \".\"\n\n[tracker]\nkind = \"jira\"\n\
             site = \"example.atlassian.net\"\nprojects = [\"ABC\"]\n",
            folder.display()
        ),
    );
}

fn extensions_file(user: &User) -> std::path::PathBuf {
    user.path("config/flottille/extensions.toml")
}

fn specs(listed: &Value) -> &Value {
    listed["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|extension| extension["name"] == "specs")
        .unwrap()
}

fn in_project<'a>(specs: &'a Value, project: &str) -> &'a Value {
    specs["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["project"] == project)
        .unwrap()
}

fn turn(user: &User, enabled: bool, project: Option<&str>) {
    user.call(
        "extension.set",
        json!({"name": "specs", "enabled": enabled, "project": project}),
    )
    .unwrap();
}

fn new_spec(user: &User, params: Value) -> Value {
    user.call("spec.new", params).unwrap()
}

fn write(
    user: &User,
    spec: &Value,
    section: &str,
    content: Value,
) -> Result<Value, (String, String)> {
    user.call(
        "spec.write",
        json!({"spec": spec["spec"], "section": section, "content": content}),
    )
}

/// A user with Specs on and a project `app`.
fn with_specs() -> User {
    let user = User::new();
    project(&user, "app");
    turn(&user, true, None);
    user
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn specs_is_off_until_the_user_turns_it_on() {
    let user = User::new();
    project(&user, "app");

    let listed = user.call("extension.list", json!({})).unwrap();
    let off = specs(&listed);
    assert_eq!(off["enabled"], false);
    assert_eq!(off["on_by_default"], false);
    assert_eq!(off["title"], "Specs");
    assert_eq!(off["scope"], "project");
    assert!(off["sentence"].as_str().unwrap().len() > 20);
    assert_eq!(in_project(off, "app")["enabled"], false);
    let (code, message) = user
        .call("spec.new", json!({"project": "app", "title": "Login"}))
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.contains("flottille extension on specs"),
        "{message}"
    );
    assert!(!extensions_file(&user).exists());

    let watch = user.watch(&["--after", "0"]);
    let output = user.run(&["extension", "on", "specs"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Specs is on.\n");
    let seen = watch.until(|event| event["name"] == "extension.changed");
    assert_eq!(
        seen.last().unwrap()["data"],
        json!({"name": "specs", "project": null})
    );
    let text = std::fs::read_to_string(extensions_file(&user)).unwrap();
    assert!(text.contains("[specs]\nenabled = true"), "{text}");
    let listed = user.call("extension.list", json!({})).unwrap();
    assert_eq!(specs(&listed)["enabled"], true);
    new_spec(&user, json!({"project": "app", "title": "Login"}));
}

#[test]
fn specs_on_globally_is_on_in_every_project() {
    let user = User::new();
    project(&user, "app");
    project(&user, "web");
    turn(&user, true, None);

    let listed = user.call("extension.list", json!({})).unwrap();
    for name in ["app", "web"] {
        let entry = in_project(specs(&listed), name);
        assert_eq!(entry["enabled"], true, "{name}");
        assert_eq!(
            entry["switch"],
            Value::Null,
            "{name}: nothing written in its file"
        );
        new_spec(&user, json!({"project": name, "title": "A task"}));
    }
    let listed = user.call("spec.list", json!({})).unwrap();
    assert_eq!(listed["specs"].as_array().unwrap().len(), 2);
    assert!(
        !std::fs::read_to_string(user.project_file("app"))
            .unwrap()
            .contains("extensions")
    );
}

#[test]
fn a_project_turns_specs_off_and_keeps_its_switch_when_specs_comes_back() {
    let user = User::new();
    project(&user, "app");
    project(&user, "web");
    turn(&user, true, None);
    let output = user.run(&["extension", "off", "specs", "--project", "app"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(
        file.contains("[extensions.specs]\nenabled = false"),
        "{file}"
    );

    let listed = user.call("extension.list", json!({})).unwrap();
    assert_eq!(in_project(specs(&listed), "app")["enabled"], false);
    assert_eq!(in_project(specs(&listed), "app")["switch"], false);
    assert_eq!(in_project(specs(&listed), "web")["enabled"], true);

    // Off globally is off everywhere; the project's switch stays where it is.
    turn(&user, false, None);
    let listed = user.call("extension.list", json!({})).unwrap();
    assert_eq!(in_project(specs(&listed), "web")["enabled"], false);
    assert_eq!(in_project(specs(&listed), "app")["switch"], false);
    turn(&user, true, None);

    let listed = user.call("extension.list", json!({})).unwrap();
    assert_eq!(in_project(specs(&listed), "app")["enabled"], false);
    assert_eq!(in_project(specs(&listed), "web")["enabled"], true);
    let (_, message) = user
        .call("spec.new", json!({"project": "app", "title": "Login"}))
        .unwrap_err();
    assert!(
        message.contains("flottille extension on specs --project app"),
        "{message}"
    );
    new_spec(&user, json!({"project": "web", "title": "Login"}));
}

#[test]
fn an_unknown_key_in_extensions_toml_is_refused_naming_it() {
    let user = User::new();
    project(&user, "app");
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        extensions_file(&user),
        "[specs]\nenabled = true\nlangage = \"fr\"\n",
    )
    .unwrap();

    let listed = user.call("extension.list", json!({})).unwrap();
    let error = specs(&listed)["error"].as_str().unwrap().to_owned();
    assert!(
        error.starts_with("extensions.toml: specs.langage: unknown key"),
        "{error}"
    );
    assert_eq!(specs(&listed)["enabled"], false);
    let (code, message) = user
        .call("spec.new", json!({"project": "app", "title": "Login"}))
        .unwrap_err();
    assert_eq!(code, "invalid-config");
    assert!(message.contains("specs.langage"), "{message}");
    // Nothing is written over a file the daemon cannot read.
    let (code, _) = user
        .call("extension.set", json!({"name": "specs", "enabled": false}))
        .unwrap_err();
    assert_eq!(code, "invalid-config");

    std::fs::write(extensions_file(&user), "[specs]\nenabled = \"yes\"\n").unwrap();
    let listed = user.call("extension.list", json!({})).unwrap();
    let error = specs(&listed)["error"].as_str().unwrap();
    assert!(
        error.starts_with("extensions.toml: specs.enabled: "),
        "{error}"
    );

    let (code, message) = user
        .call(
            "extension.set",
            json!({"name": "specs", "project": "app", "settings": {"language": "French"}}),
        )
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.starts_with("extensions.specs.language: "),
        "{message}"
    );
    let (_, message) = user
        .call(
            "extension.set",
            json!({"name": "specs", "settings": {"colour": "blue"}}),
        )
        .unwrap_err();
    assert!(
        message.starts_with("specs.colour: unknown key"),
        "{message}"
    );
}

#[test]
fn extension_set_is_not_reachable_from_an_agent_tool() {
    let user = User::with_fake_agent();
    project(&user, "app");
    turn(&user, true, None);
    user.call(
        "agent.run",
        json!({"project": "app", "message": "lead the work"}),
    )
    .unwrap();
    let (url, authorization) = common::mcp::server_of(&user, 0);
    let lead = Mcp::initialize(&url, &authorization);

    // The root has the `spec` tool Specs gives (`tests/spec_writing.rs`), none that switches it.
    let names = lead.names();
    assert!(names.iter().any(|name| name == "report"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.contains("extension")),
        "{names:?}"
    );
    for tool in ["extension_set", "extension.set", "spec_ready"] {
        let answer = lead.call(tool, json!({"name": "specs", "enabled": false}));
        assert!(answer.error, "{tool}: {}", answer.text);
    }
    let pilot = Mcp::pilot(&user);
    let names = pilot.names();
    assert!(
        !names
            .iter()
            .any(|name| name.contains("extension") || name.contains("spec")),
        "{names:?}"
    );
    let listed = user.call("extension.list", json!({})).unwrap();
    assert_eq!(specs(&listed)["enabled"], true);
}

/// The tables of the store whose name starts with `ext_specs_`, read beside the daemon.
fn tables(user: &User) -> Vec<String> {
    let connection = rusqlite::Connection::open_with_flags(
        user.path("state/flottille/flottille.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'ext_specs_%' ORDER BY name")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn the_extension_s_tables_are_created_by_its_migrations_and_kept_when_off() {
    let user = User::new();
    project(&user, "app");
    user.status();
    // Made at the daemon's start, Specs on or off.
    assert_eq!(
        tables(&user),
        [
            "ext_specs_amendments",
            "ext_specs_developments",
            "ext_specs_questions",
            "ext_specs_specs",
            "ext_specs_tickets"
        ]
    );

    turn(&user, true, None);
    let spec = new_spec(&user, json!({"project": "app", "title": "Login"}));
    turn(&user, false, None);
    assert!(user.run(&["daemon", "stop"]).status.success());
    assert_eq!(
        tables(&user),
        [
            "ext_specs_amendments",
            "ext_specs_developments",
            "ext_specs_questions",
            "ext_specs_specs",
            "ext_specs_tickets"
        ]
    );
    let (code, _) = user
        .call("spec.show", json!({"spec": spec["spec"]}))
        .unwrap_err();
    assert_eq!(code, "refused");

    turn(&user, true, None);
    let shown = user
        .call("spec.show", json!({"spec": spec["spec"]}))
        .unwrap();
    assert_eq!(shown["title"], "Login");
}

#[test]
fn a_spec_keeps_its_sections_and_never_reuses_an_identifier() {
    let user = with_specs();
    let watch = user.watch(&["--after", "0"]);
    let spec = new_spec(&user, json!({"project": "app", "title": "Login"}));
    assert_eq!(spec["status"], "draft");

    write(&user, &spec, "problem", json!("Users cannot sign in.")).unwrap();
    let written = write(
        &user,
        &spec,
        "behaviour",
        json!([{"text": "The form signs in"}, {"text": "A wrong password is said"}, {"text": "Three tries lock"}]),
    )
    .unwrap();
    assert_eq!(ids(&written["behaviour"]), ["C1", "C2", "C3"]);
    write(
        &user,
        &spec,
        "decisions",
        json!([{"question": "Where is the error said?", "decision": "Under the field", "set_aside": ["A toast"]}]),
    )
    .unwrap();
    write(
        &user,
        &spec,
        "out_of_scope",
        json!([{"text": "Sign-up", "ticket": "abc-9"}, {"text": "Two factors", "reason": "Later"}]),
    )
    .unwrap();
    write(&user, &spec, "notes", json!("The form is in login.rs.")).unwrap();
    write(
        &user,
        &spec,
        "verification",
        json!([{"id": "C2", "text": "Type a wrong password"}, {"id": "C3", "text": "Fail three times"}]),
    )
    .unwrap();
    // C2 removed, a new one written: it gets C4, never C2 again.
    let written = write(
        &user,
        &spec,
        "behaviour",
        json!([{"id": "C1", "text": "The form signs in"}, {"id": "C3", "text": "Three tries lock"}, {"text": "The session lasts a day"}]),
    )
    .unwrap();
    assert_eq!(ids(&written["behaviour"]), ["C1", "C3", "C4"]);
    assert_eq!(
        ids(&written["verification"]),
        ["C3"],
        "C2's verification left with it"
    );
    let (code, message) = write(
        &user,
        &spec,
        "behaviour",
        json!([{"id": "C2", "text": "Back again"}]),
    )
    .unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("C2 was removed"), "{message}");
    let (_, message) =
        write(&user, &spec, "out_of_scope", json!([{"text": "Sign-up"}])).unwrap_err();
    assert!(message.contains("its reason, or the ticket"), "{message}");
    let (_, message) = write(&user, &spec, "summary", json!("x")).unwrap_err();
    assert!(message.contains("not a section of a spec"), "{message}");
    write(&user, &spec, "decisions", json!([])).unwrap();
    let written = write(
        &user,
        &spec,
        "decisions",
        json!([{"question": "Lock for how long?", "decision": "An hour"}]),
    )
    .unwrap();
    assert_eq!(ids(&written["decisions"]), ["D2"]);

    // Through the command line, a file, read back after a restart.
    let file = user.path("problem.md");
    std::fs::write(&file, "People forget their password.\n").unwrap();
    let id = spec["spec"].to_string();
    let output = user.run(&[
        "spec",
        "write",
        &id,
        "problem",
        "--file",
        file.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(user.run(&["daemon", "stop"]).status.success());
    let shown = user
        .call("spec.show", json!({"spec": spec["spec"]}))
        .unwrap();
    assert_eq!(shown["problem"], "People forget their password.");
    assert_eq!(shown["notes"], "The form is in login.rs.");
    assert_eq!(
        shown["out_of_scope"],
        json!([{"text": "Sign-up", "reason": null, "ticket": "ABC-9"}, {"text": "Two factors", "reason": "Later", "ticket": null}])
    );
    assert_eq!(
        shown["decisions"],
        json!([{"id": "D2", "question": "Lock for how long?", "decision": "An hour", "set_aside": []}])
    );
    let output = user.run(&["spec", "show", &id]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("C4: The session lasts a day"), "{text}");

    let seen =
        watch.until(|event| event["name"] == "spec.changed" && event["data"]["what"] == "problem");
    let created = seen
        .iter()
        .find(|event| event["name"] == "spec.created")
        .unwrap();
    assert_eq!(created["data"]["title"], "Login");
    assert_eq!(created["data"]["mode"], "alone");
}

#[test]
fn a_spec_links_several_tickets_and_a_ticket_has_at_most_one_spec() {
    let user = with_specs();
    let first = new_spec(
        &user,
        json!({"project": "app", "title": "Login", "tickets": ["ABC-1", "abc-2"]}),
    );
    assert_eq!(first["tickets"], json!(["ABC-1", "ABC-2"]));
    assert_eq!(first["mode"], "internal");
    let linked = user
        .call(
            "spec.link",
            json!({"spec": first["spec"], "ticket": "ABC-3"}),
        )
        .unwrap();
    assert_eq!(linked["tickets"], json!(["ABC-1", "ABC-2", "ABC-3"]));

    let (code, message) = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Again", "tickets": ["ABC-2"]}),
        )
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.contains(&format!(
            "ticket ABC-2 already has a spec: spec {}",
            first["spec"]
        )),
        "{message}"
    );
    let second = new_spec(
        &user,
        json!({"project": "app", "title": "Other", "mode": "ticket", "tickets": ["ABC-4"]}),
    );
    let (_, message) = user
        .call(
            "spec.link",
            json!({"spec": second["spec"], "ticket": "ABC-1"}),
        )
        .unwrap_err();
    assert!(message.contains("already has a spec"), "{message}");
    let (_, message) = user
        .call(
            "spec.link",
            json!({"spec": second["spec"], "ticket": "#12"}),
        )
        .unwrap_err();
    assert!(message.contains("Jira"), "{message}");

    let output = user.run(&["spec", "unlink", &first["spec"].to_string(), "ABC-2"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let linked = user
        .call(
            "spec.link",
            json!({"spec": second["spec"], "ticket": "ABC-2"}),
        )
        .unwrap();
    assert_eq!(linked["tickets"], json!(["ABC-4", "ABC-2"]));
    let (_, message) = user
        .call(
            "spec.unlink",
            json!({"spec": first["spec"], "ticket": "ABC-2"}),
        )
        .unwrap_err();
    assert!(message.contains("does not cover"), "{message}");
}

#[test]
fn each_of_the_three_modes_is_stored_with_the_spec() {
    let user = with_specs();
    let alone = new_spec(&user, json!({"project": "app", "title": "An idea"}));
    let output = user.run(&[
        "spec", "new", "app", "--title", "Read", "--mode", "internal", "--ticket", "ABC-1",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let internal: Value = serde_json::from_slice(&output.stdout).unwrap();
    let ticket = new_spec(
        &user,
        json!({"project": "app", "title": "Written", "mode": "ticket", "tickets": ["ABC-2"]}),
    );

    assert!(user.run(&["daemon", "stop"]).status.success());
    for (spec, mode, tickets) in [
        (&alone, "alone", json!([])),
        (&internal, "internal", json!(["ABC-1"])),
        (&ticket, "ticket", json!(["ABC-2"])),
    ] {
        let shown = user
            .call("spec.show", json!({"spec": spec["spec"]}))
            .unwrap();
        assert_eq!(shown["mode"], mode);
        assert_eq!(shown["tickets"], tickets);
    }
    let (_, message) = user
        .call(
            "spec.link",
            json!({"spec": alone["spec"], "ticket": "ABC-5"}),
        )
        .unwrap_err();
    assert!(message.contains("alone"), "{message}");
    let (_, message) = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "No ticket", "mode": "internal"}),
        )
        .unwrap_err();
    assert!(message.contains("one ticket at least"), "{message}");
    // The ticket is the spec: one with no ticket yet creates it at its first Ready (#335).
    let created = new_spec(
        &user,
        json!({"project": "app", "title": "No ticket yet", "mode": "ticket"}),
    );
    assert_eq!(created["tickets"], json!([]));
    let (_, message) = user
        .call(
            "spec.unlink",
            json!({"spec": ticket["spec"], "ticket": "ABC-2"}),
        )
        .unwrap_err();
    assert!(message.contains("the last"), "{message}");
}

#[test]
fn a_null_inside_a_setting_s_value_is_refused_by_extension_set() {
    let user = User::new();
    project(&user, "app");
    turn(&user, true, None);
    let before = std::fs::read_to_string(extensions_file(&user)).unwrap();
    let project_before = std::fs::read_to_string(user.project_file("app")).unwrap();

    for project in [None, Some("app")] {
        let (code, message) = user
            .call(
                "extension.set",
                json!({"name": "specs", "project": project, "settings": {"language": ["fr", null]}}),
            )
            .unwrap_err();
        assert_eq!(code, "refused");
        assert!(message.contains("language: "), "{message}");
    }

    assert_eq!(
        std::fs::read_to_string(extensions_file(&user)).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        project_before
    );
}

#[test]
fn a_spec_is_written_in_the_project_s_language_or_the_user_s() {
    let user = User::new()
        .with_env("LC_ALL", "")
        .with_env("LANG", "de_DE.UTF-8");
    project(&user, "app");
    project(&user, "web");
    turn(&user, true, None);
    user.call(
        "extension.set",
        json!({"name": "specs", "project": "web", "settings": {"language": "fr"}}),
    )
    .unwrap();

    let app = new_spec(&user, json!({"project": "app", "title": "Login"}));
    let web = new_spec(&user, json!({"project": "web", "title": "Login"}));

    assert_eq!(app["language"], "de-DE");
    assert_eq!(web["language"], "fr");
    // Kept with the spec: a later change of the setting does not move it.
    user.call(
        "extension.set",
        json!({"name": "specs", "project": "web", "settings": {"language": null}}),
    )
    .unwrap();
    assert_eq!(
        user.call("spec.show", json!({"spec": web["spec"]}))
            .unwrap()["language"],
        "fr"
    );
}

#[test]
fn ready_is_refused_with_an_open_question() {
    let user = with_specs();
    let spec = new_spec(&user, json!({"project": "app", "title": "Login"}));
    let id = spec["spec"].to_string();

    let output = user.run(&["spec", "ready", &id]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("no expected behaviour"), "{said}");

    write(
        &user,
        &spec,
        "behaviour",
        json!([{"text": "The form signs in"}]),
    )
    .unwrap();
    write(
        &user,
        &spec,
        "questions",
        json!(["Lock after how many tries?"]),
    )
    .unwrap();
    let (code, message) = user
        .call("spec.ready", json!({"spec": spec["spec"]}))
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("1 open question"), "{message}");

    write(&user, &spec, "questions", json!([])).unwrap();
    let watch = user.watch(&["--after", "0"]);
    let output = user.run(&["spec", "ready", &id]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let seen = watch.until(|event| event["name"] == "spec.status_changed");
    assert_eq!(
        seen.last().unwrap()["data"],
        json!({"spec": spec["spec"], "project": "app", "status": "ready", "was": "draft"})
    );
    let listed = user
        .call("spec.list", json!({"project": "app", "status": "ready"}))
        .unwrap();
    assert_eq!(listed["specs"].as_array().unwrap().len(), 1);
    let listed = user.call("spec.list", json!({"status": "draft"})).unwrap();
    assert!(listed["specs"].as_array().unwrap().is_empty());
}

#[test]
fn a_write_to_a_ready_spec_is_refused_until_it_is_back_to_draft() {
    let user = with_specs();
    let spec = new_spec(
        &user,
        json!({"project": "app", "title": "Login", "tickets": ["ABC-1"]}),
    );
    write(
        &user,
        &spec,
        "behaviour",
        json!([{"text": "The form signs in"}]),
    )
    .unwrap();
    user.call("spec.ready", json!({"spec": spec["spec"]}))
        .unwrap();

    for (method, params) in [
        (
            "spec.write",
            json!({"spec": spec["spec"], "section": "notes", "content": "x"}),
        ),
        (
            "spec.link",
            json!({"spec": spec["spec"], "ticket": "ABC-2"}),
        ),
        (
            "spec.unlink",
            json!({"spec": spec["spec"], "ticket": "ABC-1"}),
        ),
    ] {
        let (code, message) = user.call(method, params).unwrap_err();
        assert_eq!(code, "refused", "{method}");
        assert!(
            message.contains("is Ready: it is locked"),
            "{method}: {message}"
        );
        assert!(message.contains("flottille spec draft"), "{message}");
    }
    let (_, message) = user
        .call("spec.ready", json!({"spec": spec["spec"]}))
        .unwrap_err();
    assert!(message.contains("is Ready"), "{message}");
    let shown = user
        .call("spec.show", json!({"spec": spec["spec"]}))
        .unwrap();
    assert_eq!(shown["notes"], "");

    let output = user.run(&["spec", "draft", &spec["spec"].to_string()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = write(&user, &spec, "notes", json!("Now it can")).unwrap();
    assert_eq!(written["status"], "draft");
    assert_eq!(written["notes"], "Now it can");
}

#[test]
fn spec_commands_are_refused_where_specs_is_off() {
    let user = with_specs();
    project(&user, "web");
    let spec = new_spec(&user, json!({"project": "app", "title": "Login"}));
    turn(&user, false, Some("app"));
    let id = spec["spec"].to_string();
    let file = user.path("notes.md");
    std::fs::write(&file, "x").unwrap();

    for args in [
        vec!["spec", "new", "app", "--title", "Other"],
        vec!["spec", "list", "--project", "app"],
        vec!["spec", "show", &id],
        vec![
            "spec",
            "write",
            &id,
            "notes",
            "--file",
            file.to_str().unwrap(),
        ],
        vec!["spec", "link", &id, "ABC-1"],
        vec!["spec", "unlink", &id, "ABC-1"],
        vec!["spec", "ready", &id],
        vec!["spec", "draft", &id],
    ] {
        let output = user.run(&args);
        assert!(!output.status.success(), "{args:?}");
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(
            said.contains("Specs is off in project app")
                && said.contains("flottille extension on specs --project app"),
            "{args:?}: {said}"
        );
    }
    // Elsewhere it is on: the list says only what it may.
    let listed = user.call("spec.list", json!({})).unwrap();
    assert!(listed["specs"].as_array().unwrap().is_empty());

    turn(&user, false, None);
    let output = user.run(&["spec", "list"]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        said.contains("Specs is off: turn it on with `flottille extension on specs`"),
        "{said}"
    );
}

#[test]
fn an_array_of_tables_in_extensions_toml_is_refused() {
    let user = User::new();
    project(&user, "app");
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        extensions_file(&user),
        "[specs]\nenabled = true\n\n[[specs.language]]\nvalue = \"fr\"\n",
    )
    .unwrap();

    let listed = user.call("extension.list", json!({})).unwrap();

    let error = specs(&listed)["error"].as_str().unwrap();
    assert!(
        error.starts_with("extensions.toml: specs.language: "),
        "{error}"
    );
    assert_eq!(specs(&listed)["enabled"], false);
}

#[test]
fn a_daemon_refuses_to_start_on_a_newer_extension_schema() {
    let user = User::new();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(user.path("state/flottille"))
        .unwrap();
    // The tables of Specs as a later Flottille leaves them: more migrations than this one has.
    flottille_core::store::Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .migrate_extension("specs", &["SELECT 1;"; 99])
        .unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("tables of specs are at schema 99, made by a newer Flottille"),
        "unexpected refusal: {stderr}"
    );
}

#[test]
fn an_unknown_extension_in_a_project_file_is_reported_in_extension_list() {
    let user = User::new();
    project(&user, "app");
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("{text}\n[extensions.notes]\nenabled = true\n"),
    )
    .unwrap();

    let listed = user.call("extension.list", json!({})).unwrap();

    let errors: Vec<&str> = listed["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error.as_str().unwrap())
        .collect();
    assert_eq!(
        errors,
        [
            "projects/app.toml: extensions.notes: unknown extension: the extensions are specs, review"
        ]
    );
    assert_eq!(in_project(specs(&listed), "app")["error"], Value::Null);
}

/// The issue does not say: an internal spec keeps one ticket at least, as it is made with one; to
/// have none it is made alone.
#[test]
fn unlinking_the_last_ticket_of_an_internal_spec() {
    let user = with_specs();
    let spec = new_spec(
        &user,
        json!({"project": "app", "title": "Login", "tickets": ["ABC-1"]}),
    );
    assert_eq!(spec["mode"], "internal");

    let (code, message) = user
        .call(
            "spec.unlink",
            json!({"spec": spec["spec"], "ticket": "ABC-1"}),
        )
        .unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("ticket ABC-1 is the last of spec"),
        "{message}"
    );
    let shown = user
        .call("spec.show", json!({"spec": spec["spec"]}))
        .unwrap();
    assert_eq!(shown["tickets"], json!(["ABC-1"]));
}
