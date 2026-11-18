//! The fields of a work item type (#1065, `docs/tickets.md`, "Fields of a type"): `tickets fields`
//! lists a Jira type's fields with their ids from the create metadata of the site (Jira's REST
//! API, with the vault entry the project names, as `log_time` and `worklogs` use it), a creation
//! refused for a missing required field names it by id and name, and `show` with `detail full`
//! lists a work item's custom fields. Jira is a fake HTTP server on this machine, the vault's
//! keyring the daemon's memory, and the `acli` the fake: no test reaches a site, the user's
//! keyring or anyone's sign-in.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const SITE: &str = "example.atlassian.net";
const SECRET: &str = "f1elds-t0ken-never-shown";
const ACCOUNT: &str = "dev@example.com";
/// What the daemon reads to send its Jira calls to the fake server (a debug build only).
const BASE: &str = "FLOTTILLE_TEST_JIRA_BASE";

/// What the fake Jira received of one request.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    authorization: String,
}

/// A Jira that answers each request as `route` says (a status line and a body, from the path and
/// its query), and remembers what it was asked.
struct Jira {
    base: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Jira {
    fn start(route: impl Fn(&str) -> (&'static str, String) + Send + 'static) -> Jira {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let kept = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                let head = loop {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    raw.extend_from_slice(&chunk[..read]);
                    if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break String::from_utf8_lossy(&raw[..at]).into_owned();
                    }
                    if read == 0 {
                        return;
                    }
                };
                let header = |name: &str| {
                    head.lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.trim().to_owned())
                        .unwrap_or_default()
                };
                let mut line = head.lines().next().unwrap_or_default().split(' ');
                let request = Received {
                    method: line.next().unwrap_or_default().to_owned(),
                    path: line.next().unwrap_or_default().to_owned(),
                    authorization: header("authorization"),
                };
                let (status, said) = route(&request.path);
                kept.lock().unwrap().push(request);
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\nConnection: close\r\n\r\n{said}",
                        said.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Jira { base, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    /// The paths asked, without their query.
    fn paths(&self) -> Vec<String> {
        self.received()
            .iter()
            .map(|request| request.path.split('?').next().unwrap().to_owned())
            .collect()
    }
}

const TYPES: &str = "/rest/api/3/issue/createmeta/ABC/issuetypes";

/// The create metadata of project ABC as Jira's REST API prints it (its documentation's shapes):
/// four types, one of them a custom one, and the fields of each.
fn metadata(path: &str) -> (&'static str, String) {
    let path = path.split('?').next().unwrap();
    let system = |id: &str, name: &str, required: bool| {
        json!({"fieldId": id, "key": id, "name": name, "required": required,
            "hasDefaultValue": false, "operations": ["set"], "schema": {"type": "string"}})
    };
    let option = |id: &str, name: &str, required: bool, values: &[&str]| {
        json!({"fieldId": id, "key": id, "name": name, "required": required,
            "hasDefaultValue": false, "operations": ["set"],
            "schema": {"type": "option",
                "custom": "com.atlassian.jira.plugin.system.customfieldtypes:select",
                "customId": 10100},
            "allowedValues": values.iter().enumerate()
                .map(|(at, value)| json!({"id": at.to_string(), "value": value}))
                .collect::<Vec<_>>()})
    };
    let page = |fields: Vec<Value>| {
        json!({"fields": fields, "startAt": 0, "maxResults": 200, "total": fields.len()})
            .to_string()
    };
    let basics = || {
        vec![
            system("summary", "Summary", true),
            system("project", "Project", true),
            system("issuetype", "Issue Type", true),
        ]
    };
    match path {
        TYPES => (
            "200 OK",
            json!({"issueTypes": [
                {"id": "10001", "name": "Task", "subtask": false},
                {"id": "10002", "name": "Bug", "subtask": false},
                {"id": "10050", "name": "Incident", "subtask": false},
                {"id": "10003", "name": "Sub-task", "subtask": true}],
                "startAt": 0, "maxResults": 200, "total": 4})
            .to_string(),
        ),
        // Task: nothing beyond what every creation says.
        "/rest/api/3/issue/createmeta/ABC/issuetypes/10001" => {
            let mut fields = basics();
            fields.push(system("description", "Description", false));
            ("200 OK", page(fields))
        }
        "/rest/api/3/issue/createmeta/ABC/issuetypes/10002" => {
            let mut fields = basics();
            fields.push(option(
                "customfield_10100",
                "Severity",
                true,
                &["Minor", "Major"],
            ));
            fields.push(system("customfield_10101", "Steps to reproduce", true));
            fields.push(option(
                "customfield_10102",
                "Rate",
                false,
                &["Sometimes", "Always"],
            ));
            let mut priority = option("priority", "Priority", false, &[]);
            priority["schema"] = json!({"type": "priority", "system": "priority"});
            priority["allowedValues"] = json!([{"id": "2", "name": "High"}]);
            fields.push(priority);
            ("200 OK", page(fields))
        }
        // A type the administrator made: covered as the built-in ones are.
        "/rest/api/3/issue/createmeta/ABC/issuetypes/10050" => {
            let mut fields = basics();
            let mut impact = option("customfield_10200", "Impact", true, &["Low", "High"]);
            impact["schema"] = json!({"type": "array", "items": "option",
                "custom": "com.atlassian.jira.plugin.system.customfieldtypes:multiselect",
                "customId": 10200});
            fields.push(impact);
            ("200 OK", page(fields))
        }
        "/rest/api/3/issue/createmeta/ABC/issuetypes/10003" => ("200 OK", page(basics())),
        _ => (
            "404 Not Found",
            json!({"errorMessages": ["No such project or type"], "errors": {}}).to_string(),
        ),
    }
}

fn user(jira: &Jira) -> User {
    User::with_fake_agent()
        .with_fake_acli()
        .with_env(BASE, &jira.base)
}

/// A project `app` over a throwaway repository, its tracker `tracker`, and a ready workspace.
fn fixture(user: &User, tracker: Value) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": tracker}),
    )
    .unwrap();
}

fn jira_with(user: &User, entry: Option<&str>) {
    let mut tracker = json!({"kind": "jira", "site": SITE, "projects": ["ABC"]});
    if let Some(entry) = entry {
        tracker["worklog_token"] = entry.into();
    }
    fixture(user, tracker);
}

/// The entry `jira-time` of the project, with its secret and its account.
fn entry(user: &User) {
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app"}),
    )
    .unwrap();
}

/// A Jira project ABC whose vault entry is set, over the fake site.
fn signed_in() -> (Jira, User) {
    let jira = Jira::start(metadata);
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    (jira, user)
}

/// The root agent of `app` and the client of its tools.
fn root_in(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn store(user: &User) -> Store {
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// A read: answered at once, nothing held for the user.
fn read(user: &User, answer: Answer) -> String {
    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("Waiting for"), "{}", answer.text);
    assert!(requests(user).is_empty());
    answer.text
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The user allows request `number`: the text the agent is handed for it.
fn allow(user: &User, number: u64) -> String {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

fn basic() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{ACCOUNT}:{SECRET}"))
    )
}

/// Where `needle` comes in `text`: the test says it does.
fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in: {text}"))
}

#[test]
fn fields_lists_the_required_custom_fields_of_a_type_with_their_ids() {
    let (jira, user) = signed_in();
    let mcp = root_in(&user);

    let said = read(
        &user,
        mcp.call("tickets", json!({"action": "fields", "kind": "Bug"})),
    );

    // Each field: its id, its name, its type and the values the site lists for it.
    assert!(said.contains("Bug"), "{said}");
    let (required, optional) = (at(&said, "Required"), at(&said, "Optional"));
    for line in [
        "customfield_10100: Severity (option), allowed values: Minor, Major",
        "customfield_10101: Steps to reproduce (string)",
    ] {
        let place = at(&said, line);
        assert!(required < place && place < optional, "{line}: {said}");
    }
    for line in [
        "customfield_10102: Rate (option), allowed values: Sometimes, Always",
        "priority: Priority (priority), allowed values: High",
    ] {
        assert!(optional < at(&said, line), "{line}: {said}");
    }
    // What every creation says by itself is not asked of `fields`.
    assert!(!said.contains("issuetype"), "{said}");
    // How to give them.
    assert!(said.contains("fields"), "{said}");
    // The metadata of the type, and of nothing else, asked with the vault entry's account.
    assert_eq!(jira.paths(), [TYPES, &format!("{TYPES}/10002")]);
    for request in jira.received() {
        assert_eq!(request.method, "GET");
        assert_eq!(request.authorization, basic());
    }
    assert!(!said.contains(SECRET), "{said}");
}

#[test]
fn fields_without_a_kind_lists_the_types_of_the_project() {
    let (jira, user) = signed_in();
    let mcp = root_in(&user);

    let said = read(&user, mcp.call("tickets", json!({"action": "fields"})));

    // Each type with its required fields: the built-in ones, the custom one and the sub-task.
    let (task, bug, incident, sub) = (
        at(&said, "Task"),
        at(&said, "Bug"),
        at(&said, "Incident"),
        at(&said, "Sub-task"),
    );
    assert!(task < bug && bug < incident && incident < sub, "{said}");
    assert!(said[task..bug].contains("no required field"), "{said}");
    assert!(
        said[bug..incident].contains("customfield_10100: Severity")
            && said[bug..incident].contains("customfield_10101: Steps to reproduce"),
        "{said}"
    );
    assert!(
        !said[bug..incident].contains("Rate"),
        "an optional field is not a required one: {said}"
    );
    assert!(
        said[incident..sub].contains("customfield_10200: Impact"),
        "{said}"
    );
    assert!(said[sub..].contains("a sub-task"), "{said}");
    // One read of the types and one of each type's fields.
    assert_eq!(jira.paths().len(), 5, "{:?}", jira.paths());
}

#[test]
fn fields_of_a_custom_type_lists_its_required_fields() {
    let (jira, user) = signed_in();
    let mcp = root_in(&user);

    // The type is spelled as the agent wrote it, in any case.
    let said = read(
        &user,
        mcp.call("tickets", json!({"action": "fields", "kind": "incident"})),
    );

    assert!(said.contains("Incident"), "{said}");
    assert!(
        said.contains("customfield_10200: Impact (array of option), allowed values: Low, High"),
        "{said}"
    );
    assert_eq!(jira.paths(), [TYPES, &format!("{TYPES}/10050")]);

    // A type the project has not is refused, naming those it has.
    let unknown = mcp.call("tickets", json!({"action": "fields", "kind": "Tâche"}));
    assert!(unknown.error, "{}", unknown.text);
    assert!(
        unknown
            .text
            .contains("\"Tâche\" is not a work item type of ABC: its types are Task, Bug, Incident and Sub-task"),
        "{}",
        unknown.text
    );
}

#[test]
fn fields_without_a_vault_entry_say_how_to_set_one() {
    let jira = Jira::start(metadata);
    let user = user(&jira);
    jira_with(&user, None);
    let mcp = root_in(&user);

    let answer = mcp.call("tickets", json!({"action": "fields", "kind": "Bug"}));

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.starts_with("refused:"), "{}", answer.text);
    for how in ["worklog_token", "[tracker]", "vault", "Settings"] {
        assert!(
            answer.text.contains(how),
            "{how} is not said: {}",
            answer.text
        );
    }
    assert!(requests(&user).is_empty());
    assert!(jira.received().is_empty());
}

#[test]
fn a_site_that_refuses_is_said_without_the_token() {
    let jira = Jira::start(|_| {
        (
            "401 Unauthorized",
            json!({"errorMessages": ["Unauthorized"]}).to_string(),
        )
    });
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let mcp = root_in(&user);

    let answer = mcp.call("tickets", json!({"action": "fields", "kind": "Bug"}));

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("401"), "{}", answer.text);
    assert!(!answer.text.contains(SECRET), "{}", answer.text);
}

#[test]
fn fields_on_github_says_what_an_issue_has() {
    let jira = Jira::start(metadata);
    let user = user(&jira);
    fixture(&user, json!({"kind": "github", "repository": "."}));
    let mcp = root_in(&user);

    let said = read(
        &user,
        mcp.call("tickets", json!({"action": "fields", "kind": "bug"})),
    );

    assert!(said.contains("no custom fields"), "{said}");
    for has in ["labels", "assignees", "milestone", "type"] {
        assert!(said.contains(has), "{has} is not said: {said}");
    }
    // Nothing was asked of any site.
    assert!(jira.received().is_empty());
}

/// A creation `acli` refuses for the required fields of its type, as the fake plays it: Jira's
/// words name each by its name only.
fn requiring(user: &User) {
    user.acli_script(json!({
        "items": {},
        "created": "ABC-12",
        "types": {"ABC": ["Task", "Bug"]},
        "required": {"ABC": {"Bug": {
            "customfield_10100": {"name": "Severity", "allowed": ["Minor", "Major"]},
            "customfield_10101": {"name": "Steps to reproduce"}
        }}},
    }));
}

#[test]
fn a_create_missing_a_required_field_is_refused_naming_its_id() {
    let (jira, user) = signed_in();
    requiring(&user);
    let mcp = root_in(&user);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "The form loses its input", "type": "Bug",
            "fields": {"customfield_10100": {"value": "Major"}}}),
    ));
    let answer = allow(&user, number);

    // Jira's own words, then the field it lacks, by its name and its id, and where to find more.
    assert!(answer.contains("nothing was created"), "{answer}");
    assert!(
        answer.contains("Steps to reproduce (customfield_10101)"),
        "{answer}"
    );
    // The one the call gave is not said as missing.
    assert!(!answer.contains("Severity (customfield_10100)"), "{answer}");
    assert!(answer.contains("tickets fields"), "{answer}");
    // The type's metadata, read with the entry, answered it.
    assert_eq!(jira.paths(), [TYPES, &format!("{TYPES}/10002")]);
    assert!(!answer.contains(SECRET), "{answer}");

    // With nothing given, each required field is named, with its id and its allowed values.
    let number = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Another", "type": "Bug"}),
    ));
    let answer = allow(&user, number);
    assert!(
        answer.contains("Severity (customfield_10100: Minor, Major)"),
        "{answer}"
    );
    assert!(
        answer.contains("Steps to reproduce (customfield_10101)"),
        "{answer}"
    );
}

#[test]
fn a_create_missing_a_required_field_without_an_entry_points_to_fields() {
    let jira = Jira::start(metadata);
    let user = user(&jira);
    jira_with(&user, None);
    requiring(&user);
    let mcp = root_in(&user);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "The form loses its input", "type": "Bug"}),
    ));
    let answer = allow(&user, number);

    // What it said before: Jira's words, the way to give a field; no site was asked.
    assert!(answer.contains("Severity is required"), "{answer}");
    assert!(answer.contains("customfield_"), "{answer}");
    assert!(jira.received().is_empty());
}

#[test]
fn show_full_lists_the_custom_fields_of_a_jira_work_item_with_their_ids() {
    let jira = Jira::start(metadata);
    let user = user(&jira);
    jira_with(&user, None);
    user.acli_script(json!({"items": {"ABC-1": {
        "key": "ABC-1",
        "self": format!("https://{SITE}/rest/api/3/issue/1"),
        "names": {"customfield_10100": "Severity"},
        "fields": {
            "summary": "The login fails", "status": {"name": "To Do"},
            "assignee": {"displayName": "Ana"},
            // A select: its chosen option by its text.
            "customfield_10100": {"self": "https://example.atlassian.net/rest/api/3/customFieldOption/10001",
                "value": "Major", "id": "10001"},
            "customfield_10101": "Open the form, then reload it.",
            "customfield_10102": ["Chrome", "Firefox"],
            "customfield_10103": [{"value": "Linux"}, {"value": "macOS"}],
            "customfield_10104": 3.5,
            // Without a value: not listed.
            "customfield_10105": null,
            "customfield_10106": [],
            // The sprint field is said as the sprint, not twice.
            "customfield_10020": [{"id": 4, "name": "Sprint 4", "state": "active", "boardId": 7}],
            // A system field is not a custom one.
            "labels": ["bug"]
        }}}}));
    let mcp = root_in(&user);

    let full = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
        ),
    );

    // The name when the site said it, the id always, and the value.
    assert!(
        full.contains("Severity (customfield_10100): Major"),
        "{full}"
    );
    assert!(
        full.contains("customfield_10101: Open the form, then reload it."),
        "{full}"
    );
    assert!(
        full.contains("customfield_10102: Chrome, Firefox"),
        "{full}"
    );
    assert!(full.contains("customfield_10103: Linux, macOS"), "{full}");
    assert!(full.contains("customfield_10104: 3.5"), "{full}");
    assert!(!full.contains("customfield_10105"), "{full}");
    assert!(!full.contains("customfield_10106"), "{full}");
    assert!(!full.contains("customfield_10020"), "{full}");
    assert!(full.contains("Sprint: Sprint 4 (active)"), "{full}");

    // The short read keeps to what it said.
    let short = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "ABC-1", "fresh": true}),
        ),
    );
    assert!(!short.contains("customfield_"), "{short}");
    // No site was asked: the fields come with the work item.
    assert!(jira.received().is_empty());
}
