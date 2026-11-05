//! The services of the machine (`docs/projects.md`, "Services"): defined once in
//! `services.toml`, each run once for every project and workspace by the daemon, on demand only,
//! as a run of no agent and no project (`runs.rs`), so that its output is kept, masked and read
//! as a run's is, and it stops with the daemon. The `service.*` methods, the agents' `services`
//! tool, and what starts the services a task needs before it runs.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::agent::GateMode;
use flottille_core::project::{Ready, TaskUrl};
use flottille_core::run::{Readiness, RunState};
use flottille_core::service::{self, Service};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome};
use crate::place::{Place, Resolved};
use crate::projects::Failure;
use crate::runs;

type Answer = Result<Value, Failure>;

/// How long what needs a service waits for it to be ready before it is refused.
pub const READY_LIMIT: Duration = Duration::from_secs(120);
/// The lines of a service's output `show` gives.
const SHOWN_LINES: usize = 20;
/// The lines of output `service.output` gives by default and at most.
const TAIL_LINES: usize = 200;
const TAIL_LINES_MAX: usize = 1000;

/// What the daemon knows of its services besides their file.
#[derive(Default)]
pub struct Live {
    known: Mutex<HashMap<String, Known>>,
    /// One start at a time for each service: a second waits, then finds it running.
    starting: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// One write of the file at a time.
    writing: Mutex<()>,
}

#[derive(Clone, Default)]
struct Known {
    /// Its run, going on or the last one.
    run: Option<String>,
    /// It answered its probe before Flottille started it.
    outside: bool,
    /// Why its last start failed before it ran.
    failed: Option<String>,
}

impl Live {
    fn lock_of(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.starting
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_default()
            .clone()
    }

    fn known(&self, name: &str) -> Known {
        self.known
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    fn set(&self, name: &str, known: Known) {
        self.known.lock().unwrap().insert(name.to_owned(), known);
    }
}

/// How a service is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Starting,
    Ready,
    /// It runs and says no ready.
    Running,
    /// It answered before Flottille started it: someone else runs it.
    Outside,
    Failed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Stopped => "stopped",
            State::Starting => "starting",
            State::Ready => "ready",
            State::Running => "running",
            State::Outside => "outside",
            State::Failed => "failed",
        }
    }

    fn runs(self) -> bool {
        matches!(
            self,
            State::Starting | State::Ready | State::Running | State::Outside
        )
    }
}

/// A service as it is now: its state, why when it failed, its run and its address.
struct Now {
    state: State,
    why: Option<String>,
    run: Option<String>,
    url: Option<String>,
}

// The file.

fn file(daemon: &Daemon) -> std::path::PathBuf {
    daemon.paths.config.join(service::FILE)
}

/// The services of the file, none when there is no file.
pub fn load(daemon: &Daemon) -> Result<BTreeMap<String, Service>, Failure> {
    match std::fs::read_to_string(file(daemon)) {
        Ok(text) => service::parse(&text).map_err(|why| Failure::new("invalid-config", why)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(Failure::new(
            "unavailable",
            format!("{}: cannot be read: {error}", service::FILE),
        )),
    }
}

fn find<'a>(services: &'a BTreeMap<String, Service>, name: &str) -> Result<&'a Service, Failure> {
    services.get(name).ok_or_else(|| {
        Failure::new(
            "not-found",
            format!(
                "there is no service {name} in {}: the services are {}",
                service::FILE,
                listed(services.keys())
            ),
        )
    })
}

fn listed<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let names: Vec<&str> = names.map(String::as_str).collect();
    match names.is_empty() {
        true => "none".into(),
        false => names.join(", "),
    }
}

/// Rewrites the file where it stands, keeping what the user wrote around the entry: refused,
/// nothing written, when the file would no longer be one the daemon can use.
fn rewrite(
    daemon: &Daemon,
    change: impl FnOnce(&BTreeMap<String, Service>, &mut toml_edit::DocumentMut) -> Result<(), Failure>,
) -> Result<BTreeMap<String, Service>, Failure> {
    let _writing = daemon.services.writing.lock().unwrap();
    let path = file(daemon);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(Failure::new("unavailable", error.to_string())),
    };
    let services = service::parse(&text).map_err(|why| Failure::new("invalid-config", why))?;
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| Failure::new("invalid-config", format!("{}: {error}", service::FILE)))?;
    change(&services, &mut document)?;
    let written = document.to_string();
    let services = service::parse(&written).map_err(Failure::refused)?;
    std::fs::create_dir_all(&daemon.paths.config)
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    let writing = daemon.paths.config.join(format!(".{}.tmp", service::FILE));
    std::fs::write(&writing, &written)
        .and_then(|()| std::fs::rename(&writing, &path))
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    Ok(services)
}

/// Writes the service under `[services.<name>]`, in place of what was there.
fn write(
    document: &mut toml_edit::DocumentMut,
    name: &str,
    service: &Service,
) -> Result<(), Failure> {
    for line in std::iter::once(service.line.as_str()).chain(service.other_lines()) {
        flottille_core::command::split_line(line).map_err(Failure::refused)?;
    }
    let not_a_table = |key: String| {
        Failure::new(
            "invalid-config",
            format!("{}: {key}: must be a table", service::FILE),
        )
    };
    let services = document.entry("services").or_insert_with(|| {
        let mut services = toml_edit::Table::new();
        services.set_implicit(true);
        toml_edit::Item::Table(services)
    });
    let services = services
        .as_table_like_mut()
        .ok_or_else(|| not_a_table("services".into()))?;
    if services.get(name).is_none() {
        services.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let entry = services
        .get_mut(name)
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| not_a_table(format!("services.{name}")))?;
    match &service.folder {
        Some(folder) => entry.insert("folder", toml_edit::value(folder)),
        None => entry.remove("folder"),
    };
    entry.insert("line", toml_edit::value(&service.line));
    if service.needs.is_empty() {
        entry.remove("needs");
    } else {
        entry.insert(
            "needs",
            toml_edit::value(service.needs.iter().collect::<toml_edit::Array>()),
        );
    }
    let inline = |key: &str, text: &str| {
        let mut table = toml_edit::InlineTable::new();
        table.insert(key, text.into());
        toml_edit::value(table)
    };
    match service.ready.as_deref() {
        Some(Ready::Line(text)) => entry.insert("ready", inline("line", text)),
        Some(Ready::Url(url)) => entry.insert("ready", inline("url", url)),
        Some(Ready::Command(line)) => entry.insert("ready", inline("command", line)),
        None => entry.remove("ready"),
    };
    match service.url.as_deref() {
        Some(TaskUrl::Address(url)) => entry.insert("url", toml_edit::value(url)),
        Some(TaskUrl::Command { command }) => entry.insert("url", inline("command", command)),
        None => entry.remove("url"),
    };
    if service.ask_before_running {
        entry.insert("ask_before_running", toml_edit::value(true));
    } else {
        entry.remove("ask_before_running");
    }
    Ok(())
}

/// What an add or an edit gives: each key left out is unchanged (an add starts from nothing);
/// `null` removes `folder`, `ready` and `url`.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    #[serde(default, deserialize_with = "present")]
    folder: Option<Option<String>>,
    line: Option<String>,
    needs: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    ready: Option<Option<Box<Ready>>>,
    #[serde(default, deserialize_with = "present")]
    url: Option<Option<Box<TaskUrl>>>,
    ask_before_running: Option<bool>,
}

/// A field given, `null` (and `""`, which the window sends for none) included.
fn present<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(Some(None)),
        Value::String(text) if text.is_empty() => Ok(Some(None)),
        value => serde_json::from_value(value)
            .map(|value| Some(Some(value)))
            .map_err(serde::de::Error::custom),
    }
}

impl Keys {
    /// The service as these keys leave `was`; an add needs a line.
    fn applied(&self, was: Option<&Service>) -> Result<Service, String> {
        let mut service = match was {
            Some(was) => was.clone(),
            None => Service {
                folder: None,
                line: self.line.clone().ok_or("an add needs a line")?,
                needs: Vec::new(),
                ready: None,
                url: None,
                ask_before_running: false,
            },
        };
        if let Some(folder) = &self.folder {
            service.folder = folder.clone().filter(|f| !f.trim().is_empty());
        }
        if let Some(line) = &self.line {
            service.line = line.trim().to_owned();
        }
        if let Some(needs) = &self.needs {
            service.needs = needs.clone();
        }
        if let Some(ready) = &self.ready {
            service.ready = ready.clone();
        }
        if let Some(url) = &self.url {
            service.url = url.clone();
        }
        if let Some(ask) = self.ask_before_running {
            service.ask_before_running = ask;
        }
        Ok(service)
    }
}

/// Adds the service (`edit` false) or changes it, and says so (`service.changed`): `by` the agent
/// that wrote it and the mode of its tree, `None` for the user.
fn put(
    daemon: &Daemon,
    name: &str,
    keys: &Keys,
    edit: bool,
    by: Option<(&str, GateMode)>,
) -> Result<Service, Failure> {
    let services = rewrite(daemon, |services, document| {
        let was = services.get(name);
        match (edit, was) {
            (false, Some(_)) => {
                return Err(Failure::refused(format!(
                    "there is already a service {name}: edit it, or remove it first"
                )));
            }
            (true, None) => {
                find(services, name)?;
            }
            _ => {}
        }
        let service = keys.applied(was).map_err(Failure::refused)?;
        write(document, name, &service)
    })?;
    written_by(daemon, name, by)?;
    let change = if edit { "edited" } else { "added" };
    let _ = daemon.emit(
        "service.changed",
        json!({"service": name, "change": change}),
    );
    Ok(services[name].clone())
}

/// Removes the service: refused while another service or a project's task needs it.
fn remove(daemon: &Daemon, name: &str) -> Result<(), Failure> {
    let needed_by = needed_by(daemon, name);
    if !needed_by.is_empty() {
        return Err(Failure::refused(format!(
            "{} need the service {name}: change them first",
            needed_by.join(", ")
        )));
    }
    rewrite(daemon, |services, document| {
        find(services, name)?;
        if let Some((other, _)) = services
            .iter()
            .find(|(_, service)| service.needs.iter().any(|need| need == name))
        {
            return Err(Failure::refused(format!(
                "the service {other} needs {name}: change it first"
            )));
        }
        if let Some(table) = document
            .get_mut("services")
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            table.remove(name);
        }
        Ok(())
    })?;
    written_by(daemon, name, None)?;
    let _ = daemon.emit(
        "service.changed",
        json!({"service": name, "change": "removed"}),
    );
    Ok(())
}

fn written_by(daemon: &Daemon, name: &str, by: Option<(&str, GateMode)>) -> Result<(), Failure> {
    daemon
        .write(|store| {
            store
                .set_service_writer(name, by)
                .map(|()| ((), None::<flottille_core::protocol::Event>))
        })
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}

/// Whether an agent wrote the service last: its lines are judged again when another agent's call
/// starts it. One that cannot be read is taken as an agent's.
pub fn by_an_agent(daemon: &Daemon, name: &str) -> bool {
    !matches!(daemon.read(|store| store.service_writer(name)), Ok(None))
}

/// The projects' tasks that need the service, through other services too: `app (api:dev)`.
fn needed_by(daemon: &Daemon, name: &str) -> Vec<String> {
    let services = load(daemon).unwrap_or_default();
    let loaded = daemon
        .projects
        .lock()
        .unwrap()
        .registry()
        .load_all()
        .unwrap_or_default();
    loaded
        .projects
        .iter()
        .filter_map(|project| {
            let tasks: Vec<&str> = project
                .tasks
                .iter()
                .filter(|(_, task)| {
                    service::in_order(&services, &task.needs)
                        .iter()
                        .any(|need| need == name)
                })
                .map(|(task, _)| task.as_str())
                .collect();
            (!tasks.is_empty()).then(|| format!("{} ({})", project.name, tasks.join(", ")))
        })
        .collect()
}

// How a service is.

fn now(daemon: &Daemon, name: &str) -> Now {
    let known = daemon.services.known(name);
    if let Some(id) = &known.run
        && let Some(view) = runs::view(daemon, id)
    {
        let run = view.run;
        let url = run.url.clone();
        let (state, why) = match (run.state, run.ready) {
            (RunState::Running, Some(Readiness::Starting)) => (State::Starting, None),
            (RunState::Running, Some(Readiness::Ready)) => (State::Ready, None),
            (RunState::Running, _) => (State::Running, None),
            (RunState::Exited, ready) => {
                let how = match (run.exit_code, run.signal) {
                    (Some(code), _) => format!("it exited with code {code}"),
                    (None, Some(signal)) => format!("it was killed by signal {signal}"),
                    _ => "it ended".to_owned(),
                };
                let before = if ready == Some(Readiness::Missed) {
                    " before it was ready"
                } else {
                    ""
                };
                let why = match last_line(&view.output) {
                    Some(line) => format!("{how}{before}: {line}"),
                    None => format!("{how}{before}"),
                };
                (State::Failed, Some(why))
            }
            (RunState::Stopped | RunState::Interrupted, _) => (State::Stopped, None),
        };
        if state.runs() || !known.outside {
            return Now {
                state,
                why,
                run: Some(run.id),
                url,
            };
        }
    }
    if known.outside {
        return Now {
            state: State::Outside,
            why: None,
            run: None,
            url: None,
        };
    }
    if let Some(why) = known.failed {
        return Now {
            state: State::Failed,
            why: Some(why),
            run: None,
            url: None,
        };
    }
    Now {
        state: State::Stopped,
        why: None,
        run: known.run,
        url: None,
    }
}

fn last_line(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect())
}

/// A service as the protocol shows it.
fn to_json(daemon: &Daemon, services: &BTreeMap<String, Service>, name: &str) -> Value {
    let service = &services[name];
    let now = now(daemon, name);
    let home = flottille_core::place::home();
    let folder = service.folder_in(home.as_deref()).ok();
    let address = now.url.clone().or_else(|| match service.url.as_deref() {
        Some(TaskUrl::Address(url)) => Some(url.clone()),
        _ => None,
    });
    let stopped: Vec<&String> = service
        .needs
        .iter()
        .filter(|need| !now_of(daemon, need).runs())
        .collect();
    json!({
        "name": name,
        "folder": folder,
        "line": service.line,
        "needs": service.needs,
        "ready": service.ready,
        "url": service.url,
        "ask_before_running": service.ask_before_running,
        "state": now.state.as_str(),
        "why": now.why,
        "run": now.run,
        "address": address,
        "needs_stopped": stopped,
    })
}

fn now_of(daemon: &Daemon, name: &str) -> State {
    now(daemon, name).state
}

/// What a call may start before what it names, which Flottille does not run now: the services a
/// task needs (`run`) or a service needs (`services start`), in the order they start. One seen
/// running outside Flottille is among them: it is started if it no longer answers.
pub fn started_first(daemon: &Daemon, place: &Place, tool: &str, args: &Value) -> Vec<String> {
    let Ok(services) = load(daemon) else {
        return Vec::new();
    };
    let (needs, own) = match tool {
        "run" => (runs::needs_of(daemon, place, args), None),
        "services" if args["action"] == "start" => match args["name"].as_str() {
            Some(name) if services.contains_key(name) => (vec![name.to_owned()], Some(name)),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    service::in_order(&services, &needs)
        .into_iter()
        .filter(|name| Some(name.as_str()) != own && !flottille_runs(daemon, name))
        .collect()
}

/// Whether Flottille runs the service now: what it started and has not ended.
fn flottille_runs(daemon: &Daemon, name: &str) -> bool {
    let known = daemon.services.known(name);
    known.run.is_some_and(|id| runs::is_going(daemon, &id))
}

/// Probes again each service seen running outside Flottille: one that no longer answers is shown
/// as it is, stopped. A service being started is left to its start.
async fn probed(daemon: &Arc<Daemon>, services: &BTreeMap<String, Service>) {
    let home = flottille_core::place::home();
    for (name, service) in services {
        let lock = daemon.services.lock_of(name);
        let Ok(_starting) = lock.try_lock() else {
            continue;
        };
        let known = daemon.services.known(name);
        if !known.outside
            || known
                .run
                .as_ref()
                .is_some_and(|id| runs::is_going(daemon, id))
        {
            continue;
        }
        let answers = match service.folder_in(home.as_deref()) {
            Ok(folder) => match runs::service_start(daemon, service, folder).await {
                Ok(prepared) => prepared.has_ready() && prepared.answers(daemon, name).await,
                Err(_) => false,
            },
            Err(_) => false,
        };
        if !answers {
            daemon.services.set(
                name,
                Known {
                    outside: false,
                    ..known
                },
            );
        }
    }
}

// Starting and stopping.

/// Starts each service `needs` names that does not run, those it needs first, and waits for
/// each that says when it is ready: the names of those it started. `Err` names the service that
/// failed and why. For an agent's call, `judged` holds what the gate judged it to start
/// (`started_first`): any other service it would start stopped since, and is refused.
pub async fn start_needed(
    daemon: &Arc<Daemon>,
    needs: &[String],
    judged: Option<&[String]>,
) -> Result<Vec<String>, String> {
    if needs.is_empty() {
        return Ok(Vec::new());
    }
    let services = load(daemon).map_err(|failure| failure.message)?;
    if let Some(unknown) = needs.iter().find(|need| !services.contains_key(*need)) {
        return Err(format!(
            "it needs the service {unknown}, which {} does not hold",
            service::FILE
        ));
    }
    let mut started = Vec::new();
    for name in service::in_order(&services, needs) {
        if start_one(daemon, &services, &name, true, judged).await? {
            started.push(name);
        }
    }
    Ok(started)
}

/// Starts the service, what it needs first and waited for: the names of those it started.
/// `judged`, for an agent's call, as `start_needed`'s; the service itself is always judged.
pub async fn start(
    daemon: &Arc<Daemon>,
    name: &str,
    judged: Option<&[String]>,
) -> Result<Vec<String>, Failure> {
    let services = load(daemon)?;
    find(&services, name)?;
    let mut started = Vec::new();
    for each in service::in_order(&services, &[name.to_owned()]) {
        let own = each == name;
        let judged = judged.filter(|_| !own);
        if start_one(daemon, &services, &each, !own, judged)
            .await
            .map_err(Failure::refused)?
        {
            started.push(each);
        }
    }
    Ok(started)
}

/// Starts one service unless it runs, or answers already (outside Flottille), then waits for it
/// to be ready when `wait`: whether it was started. One start at a time for each service, so that
/// two that need it at once share one instance.
async fn start_one(
    daemon: &Arc<Daemon>,
    services: &BTreeMap<String, Service>,
    name: &str,
    wait: bool,
    judged: Option<&[String]>,
) -> Result<bool, String> {
    let failed = |why: String| format!("the service {name} failed: {why}");
    let lock = daemon.services.lock_of(name);
    let starting = lock.lock().await;
    let known = daemon.services.known(name);
    let going = known.run.as_ref().filter(|id| runs::is_going(daemon, id));
    let (started, run) = match going {
        Some(id) => (false, Some(id.clone())),
        // Its lines, its probe's included, were not judged for this call: it ran then.
        None if judged.is_some_and(|judged| !judged.iter().any(|each| each == name)) => {
            return Err(format!(
                "the service {name} stopped since the call was judged: call again"
            ));
        }
        None => {
            let service = &services[name];
            let home = flottille_core::place::home();
            let prepared = match service.folder_in(home.as_deref()) {
                Ok(folder) => runs::service_start(daemon, service, folder).await,
                Err(why) => Err(why),
            };
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(why) => {
                    daemon.services.set(
                        name,
                        Known {
                            failed: Some(why.clone()),
                            ..known
                        },
                    );
                    return Err(failed(why));
                }
            };
            if prepared.has_ready() && prepared.answers(daemon, name).await {
                daemon.services.set(
                    name,
                    Known {
                        run: known.run,
                        outside: true,
                        failed: None,
                    },
                );
                let _ = daemon.emit(
                    "service.changed",
                    json!({"service": name, "change": "outside"}),
                );
                return Ok(false);
            }
            let (line, folder) = (prepared.line.clone(), prepared.folder.clone());
            let has_ready = prepared.has_ready();
            match runs::start_service(daemon, name, prepared) {
                Ok(id) => {
                    daemon.services.set(
                        name,
                        Known {
                            run: Some(id.clone()),
                            outside: false,
                            failed: None,
                        },
                    );
                    let _ = daemon.emit(
                        "service.started",
                        json!({"service": name, "run": id, "line": line, "folder": folder,
                            "ready": has_ready.then_some("starting")}),
                    );
                    tokio::spawn(follow(daemon.clone(), name.to_owned(), id.clone()));
                    (true, Some(id))
                }
                Err(why) => {
                    daemon.services.set(
                        name,
                        Known {
                            failed: Some(why.clone()),
                            ..known
                        },
                    );
                    return Err(failed(why));
                }
            }
        }
    };
    drop(starting);
    let (Some(id), true) = (run, wait) else {
        return Ok(started);
    };
    match tokio::time::timeout(READY_LIMIT, runs::wait_ready(daemon, &id)).await {
        Ok(true) => Ok(started),
        Ok(false) => Err(failed(
            now(daemon, name)
                .why
                .unwrap_or_else(|| "it ended before it was ready".into()),
        )),
        Err(_) => Err(format!(
            "the service {name} was not ready within {} s",
            READY_LIMIT.as_secs()
        )),
    }
}

/// Follows a service's run to its end, which it says (its ready is said by the run itself).
async fn follow(daemon: Arc<Daemon>, name: String, id: String) {
    runs::wait_for_end(&daemon, &id).await;
    let now = now(&daemon, &name);
    let _ = daemon.emit(
        "service.ended",
        json!({"service": name, "run": id, "state": now.state.as_str(), "why": now.why}),
    );
}

/// Stops the service: its run's whole process group, as a run is stopped. What it needs and
/// what needs it go on.
pub async fn stop(daemon: &Daemon, name: &str) -> Result<(), Failure> {
    let services = load(daemon)?;
    find(&services, name)?;
    let known = daemon.services.known(name);
    match known.run.as_ref().filter(|id| runs::is_going(daemon, id)) {
        Some(id) => {
            runs::stop_by_user(daemon, id).await;
            Ok(())
        }
        None if known.outside => {
            // What ran outside may have stopped since: its next start probes it again.
            daemon.services.set(
                name,
                Known {
                    outside: false,
                    ..known
                },
            );
            Err(Failure::refused(format!(
                "the service {name} runs outside Flottille, which did not start it and does not \
                 stop it"
            )))
        }
        None => Err(Failure::refused(format!(
            "the service {name} is not running"
        ))),
    }
}

// The protocol (`docs/protocol.md`, "The service methods").

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    name: String,
}

/// The name (and the action, for the tool) of a call, and the keys it gives.
fn split(
    args: Value,
    names: &[&str],
) -> Result<(BTreeMap<String, Value>, Keys), serde_json::Error> {
    let mut args = match args {
        Value::Object(args) => args,
        other => {
            return Err(serde::de::Error::custom(format!(
                "expected an object, not {other}"
            )));
        }
    };
    let taken = names
        .iter()
        .filter_map(|name| args.remove(*name).map(|value| ((*name).to_owned(), value)))
        .collect();
    Ok((taken, serde_json::from_value(Value::Object(args))?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputAsked {
    name: String,
    tail: Option<usize>,
}

pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "service.list" => {
            let services = load(&daemon)?;
            probed(&daemon, &services).await;
            let listed: Vec<Value> = services
                .keys()
                .map(|name| to_json(&daemon, &services, name))
                .collect();
            Ok(json!({"services": listed}))
        }
        "service.show" => {
            let Named { name } = serde_json::from_value(params)?;
            let services = load(&daemon)?;
            find(&services, &name)?;
            probed(&daemon, &services).await;
            Ok(to_json(&daemon, &services, &name))
        }
        "service.add" | "service.edit" => {
            let (taken, keys) = split(params, &["name"])?;
            let Some(name) = taken.get("name").and_then(Value::as_str) else {
                return Err(Failure::new("invalid-params", "missing field `name`"));
            };
            put(&daemon, name, &keys, method == "service.edit", None)?;
            let services = load(&daemon)?;
            Ok(to_json(&daemon, &services, name))
        }
        "service.remove" => {
            let Named { name } = serde_json::from_value(params)?;
            remove(&daemon, &name)?;
            Ok(json!({"removed": name}))
        }
        "service.start" => {
            let Named { name } = serde_json::from_value(params)?;
            let started = start(&daemon, &name, None).await?;
            let services = load(&daemon)?;
            Ok(json!({"service": to_json(&daemon, &services, &name), "started": started}))
        }
        "service.stop" => {
            let Named { name } = serde_json::from_value(params)?;
            stop(&daemon, &name).await?;
            let services = load(&daemon)?;
            Ok(json!({"service": to_json(&daemon, &services, &name)}))
        }
        "service.output" => {
            let asked: OutputAsked = serde_json::from_value(params)?;
            let tail = asked.tail.unwrap_or(TAIL_LINES);
            if !(1..=TAIL_LINES_MAX).contains(&tail) {
                return Err(Failure::refused(format!(
                    "tail must be 1 to {TAIL_LINES_MAX} lines"
                )));
            }
            let services = load(&daemon)?;
            find(&services, &asked.name)?;
            let run = daemon.services.known(&asked.name).run;
            let view = run.as_deref().and_then(|id| runs::view(&daemon, id));
            let output = view.as_ref().map_or("", |view| view.output.as_str());
            let lines: Vec<&str> = output.lines().collect();
            let from = lines.len().saturating_sub(tail);
            Ok(json!({"service": asked.name, "run": run,
                "output": lines[from..].join("\n"),
                "earlier": from > 0 || view.is_some_and(|view| view.cut)}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

// The agents' tool (`docs/tools.md`, "Services").

struct ToolCall {
    action: String,
    name: Option<String>,
    keys: Keys,
}

fn tool_call(args: &Value) -> Result<ToolCall, Outcome> {
    let refused = |error: serde_json::Error| {
        Outcome::Refused(format!(
            "refused: the arguments of services do not fit: {error}"
        ))
    };
    let (taken, keys) = split(args.clone(), &["action", "name"]).map_err(refused)?;
    let text = |key: &str| taken.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(ToolCall {
        action: text("action").ok_or_else(|| {
            Outcome::Refused(format!(
                "refused: services needs action: one of {}",
                actions()
            ))
        })?,
        name: text("name"),
        keys,
    })
}

const ACTIONS: [&str; 7] = ["list", "show", "start", "stop", "add", "edit", "remove"];

/// The actions, in words.
fn actions() -> String {
    format!("{} and {}", ACTIONS[..6].join(", "), ACTIONS[6])
}

/// Whether a call stops a service: held for the user every time, in every mode.
pub fn stops(tool: &str, args: &Value) -> bool {
    tool == "services" && args["action"] == "stop"
}

/// Whether a call writes the file: judged as a task's write is.
pub fn writes(tool: &str, args: &Value) -> bool {
    tool == "services" && matches!(args["action"].as_str(), Some("add" | "edit" | "remove"))
}

/// The service a call starts or leaves written, resolved in the place: its folder, the words of
/// its line and the service as a task, for the gate, which judges it as a run of its line in its
/// folder. `None` for any other call, and one that does not fit (the tool refuses it).
pub fn reaches(
    daemon: &Daemon,
    place: &Place,
    args: &Value,
) -> Option<(Resolved, Vec<String>, String, flottille_core::project::Task)> {
    let call = tool_call(args).ok()?;
    let name = call.name?;
    let services = load(daemon).ok()?;
    let service = match call.action.as_str() {
        "start" => services.get(&name)?.clone(),
        "add" => call.keys.applied(None).ok()?,
        "edit" => call.keys.applied(Some(services.get(&name)?)).ok()?,
        _ => return None,
    };
    let folder = service.folder_in(place.home.as_deref()).ok()?;
    let words = flottille_core::command::split_line(&service.line).ok()?;
    Some((place.resolve(&folder, "."), words, name, service.as_task()))
}

/// A call checked before the gate is asked: what it changes, in words (nothing for a read or a
/// start), or the tool's refusal. A child writes nothing of the machine's.
pub fn says(
    daemon: &Daemon,
    place: &Place,
    root: bool,
    args: &Value,
) -> Result<Vec<String>, Outcome> {
    let call = tool_call(args)?;
    let action = call.action.as_str();
    if !ACTIONS.contains(&action) {
        return Err(Outcome::Refused(format!(
            "refused: services has no action {action:?}: its actions are {}",
            actions()
        )));
    }
    if writes("services", args) && !root {
        return Err(Outcome::Refused(
            "refused: a child reads the services of the machine and starts them; the agent the \
             user talks to changes them: ask your parent (ask_parent)"
                .into(),
        ));
    }
    if action == "list" {
        return Ok(Vec::new());
    }
    let Some(name) = call.name.as_deref() else {
        return Err(Outcome::Refused(format!(
            "refused: {action} needs name: {{\"action\": \"{action}\", \"name\": \"mongo\"}}"
        )));
    };
    let services = load(daemon)
        .map_err(|failure| Outcome::Refused(format!("refused: {}", failure.message)))?;
    let unknown = || {
        Outcome::Refused(format!(
            "refused: there is no service {name}: the services are {}",
            listed(services.keys())
        ))
    };
    let check = |service: &Service| {
        let mut all = services.clone();
        all.insert(name.to_owned(), service.clone());
        service::check(&all).map_err(|why| Outcome::Refused(format!("refused: {why}")))?;
        for line in std::iter::once(service.line.as_str()).chain(service.other_lines()) {
            flottille_core::command::split_line(line).map_err(Outcome::Refused)?;
        }
        Ok::<(), Outcome>(())
    };
    match action {
        "add" => {
            if services.contains_key(name) {
                return Err(Outcome::Refused(format!(
                    "refused: there is already a service {name}: edit it"
                )));
            }
            let service = call
                .keys
                .applied(None)
                .map_err(|why| Outcome::Refused(format!("refused: {why}")))?;
            check(&service)?;
            Ok(vec![format!(
                "Add the service {name}, shared by every project: {}",
                said(&service)
            )])
        }
        "edit" => {
            let was = services.get(name).ok_or_else(unknown)?;
            let service = call
                .keys
                .applied(Some(was))
                .map_err(|why| Outcome::Refused(format!("refused: {why}")))?;
            check(&service)?;
            if service == *was {
                return Err(Outcome::Refused(format!(
                    "refused: this edit changes nothing of the service {name}"
                )));
            }
            Ok(vec![
                format!("Change the service {name} to: {}", said(&service)),
                format!("It was: {}", said(was)),
            ])
        }
        "remove" => {
            let was = services.get(name).ok_or_else(unknown)?;
            Ok(vec![format!("Remove the service {name}: {}", said(was))])
        }
        "stop" => {
            services.get(name).ok_or_else(unknown)?;
            // Only what Flottille runs can be stopped: the user is not asked to approve a refusal.
            match now_of(daemon, name) {
                State::Outside => {
                    return Err(Outcome::Refused(format!(
                        "refused: the service {name} runs outside Flottille, which did not start \
                         it and does not stop it"
                    )));
                }
                state if !state.runs() => {
                    return Err(Outcome::Refused(format!(
                        "refused: the service {name} is not running"
                    )));
                }
                _ => {}
            }
            let needed = needed_by(daemon, name);
            let by = match needed.is_empty() {
                true => "no project's task needs it".to_owned(),
                false => format!("the tasks that need it: {}", needed.join("; ")),
            };
            Ok(vec![format!(
                "Stop the service {name}, shared by every project ({by})"
            )])
        }
        _ => {
            services.get(name).ok_or_else(unknown)?;
            Ok(said_first(
                daemon,
                &services,
                started_first(daemon, place, "services", args),
            ))
        }
    }
}

/// What a call starts before what it names, in words for the user who approves it.
pub fn starts_said(daemon: &Daemon, place: &Place, tool: &str, args: &Value) -> Vec<String> {
    let first = started_first(daemon, place, tool, args);
    match load(daemon) {
        Ok(services) => said_first(daemon, &services, first),
        Err(_) => Vec::new(),
    }
}

fn said_first(
    daemon: &Daemon,
    services: &BTreeMap<String, Service>,
    first: Vec<String>,
) -> Vec<String> {
    let said: Vec<String> = first
        .iter()
        .filter_map(|name| {
            let service = services.get(name)?;
            let by = match by_an_agent(daemon, name) {
                true => ", written by an agent",
                false => "",
            };
            let outside = match now_of(daemon, name) {
                State::Outside => {
                    " (it runs outside Flottille now: started if it no longer answers)"
                }
                _ => "",
            };
            Some(format!(
                "the service {name}{by}{outside}: {}",
                said(service)
            ))
        })
        .collect();
    match said.is_empty() {
        true => Vec::new(),
        false => vec![format!(
            "It starts first what it needs, shared by every project: {}",
            said.join("; ")
        )],
    }
}

/// Why the gate holds a stop: whatever the mode, only the user stops what every project shares.
pub fn stop_reason(args: &Value) -> String {
    let name = args["name"].as_str().unwrap_or("a service");
    format!(
        "it stops the service {name}, which every project shares: only the user stops a service"
    )
}

/// A service in words: its line, where it runs and what it needs.
fn said(service: &Service) -> String {
    let mut notes = vec![format!(
        "in {}",
        service.folder.as_deref().unwrap_or("the home")
    )];
    if !service.needs.is_empty() {
        notes.push(format!("needs {}", service.needs.join(", ")));
    }
    if let Some(ready) = &service.ready {
        notes.push(ready.said());
    }
    if let Some(url) = &service.url {
        notes.push(url.said());
    }
    if service.ask_before_running {
        notes.push("judged each time it starts".into());
    }
    format!("`{}` ({})", service.line, notes.join(", "))
}

/// The `services` tool, once the gate let it through.
/// `judged`, what the gate judged a start to start first.
pub async fn tool(daemon: &Arc<Daemon>, grant: &Grant, args: Value, judged: &[String]) -> Outcome {
    let call = match tool_call(&args) {
        Ok(call) => call,
        Err(refusal) => return refusal,
    };
    let services = match load(daemon) {
        Ok(services) => services,
        Err(failure) => return Outcome::Refused(format!("refused: {}", failure.message)),
    };
    let name = call.name.clone().unwrap_or_default();
    if matches!(call.action.as_str(), "list" | "show") {
        probed(daemon, &services).await;
    }
    match call.action.as_str() {
        "list" => Outcome::Done(list_text(daemon, &services)),
        "show" => match services.get(&name) {
            Some(_) => Outcome::Done(show_text(daemon, &services, &name)),
            None => Outcome::Refused(format!("refused: there is no service {name}")),
        },
        "start" => match start(daemon, &name, Some(judged)).await {
            Ok(started) => {
                let services = load(daemon).unwrap_or(services);
                let mut text = match started.is_empty() {
                    true => format!("Nothing started: the service {name} runs.\n"),
                    false => format!("Started: {}.\n", started.join(", ")),
                };
                text.push_str(&line_text(daemon, &services, &name));
                Outcome::Done(text)
            }
            Err(failure) => Outcome::Refused(format!("refused: {}", failure.message)),
        },
        "stop" => match stop(daemon, &name).await {
            Ok(()) => Outcome::Done(format!(
                "The service {name} is stopped; what it needs and what needs it go on."
            )),
            Err(failure) => Outcome::Refused(format!("refused: {}", failure.message)),
        },
        action @ ("add" | "edit") => match put(
            daemon,
            &name,
            &call.keys,
            action == "edit",
            Some((&grant.agent, crate::judge::mode(daemon, &grant.agent))),
        ) {
            Ok(_) => Outcome::Done(format!(
                "The service {name} is {}: services {{\"action\": \"start\", \"name\": \
                 \"{name}\"}} starts it.",
                if action == "add" { "added" } else { "changed" }
            )),
            Err(failure) => Outcome::Refused(format!("refused: {}", failure.message)),
        },
        "remove" => match remove(daemon, &name) {
            Ok(()) => Outcome::Done(format!("The service {name} is removed.")),
            Err(failure) => Outcome::Refused(format!("refused: {}", failure.message)),
        },
        other => Outcome::Refused(format!(
            "refused: services has no action {other:?}: its actions are {}",
            actions()
        )),
    }
}

/// One line of a service: its name, its state, its folder and its address.
fn line_text(daemon: &Daemon, services: &BTreeMap<String, Service>, name: &str) -> String {
    let shown = to_json(daemon, services, name);
    let mut text = format!(
        "- {name}: {} in {}",
        shown["state"].as_str().unwrap_or_default(),
        shown["folder"].as_str().unwrap_or("the home")
    );
    if let Some(why) = shown["why"].as_str() {
        text.push_str(&format!(" ({why})"));
    }
    if let Some(address) = shown["address"].as_str() {
        text.push_str(&format!(", at {address}"));
    }
    if let Some(stopped) = shown["needs_stopped"].as_array()
        && !stopped.is_empty()
    {
        let names: Vec<&str> = stopped.iter().filter_map(Value::as_str).collect();
        text.push_str(&format!(
            ", but what it needs is not running: {}",
            names.join(", ")
        ));
    }
    text
}

fn list_text(daemon: &Daemon, services: &BTreeMap<String, Service>) -> String {
    if services.is_empty() {
        return format!(
            "The machine has no service: the user defines them in {}.",
            service::FILE
        );
    }
    let mut text = "The services of the machine, shared by every project (a task's needs starts \
                    them):\n"
        .to_owned();
    for name in services.keys() {
        text.push_str(&line_text(daemon, services, name));
        text.push('\n');
    }
    text
}

fn show_text(daemon: &Daemon, services: &BTreeMap<String, Service>, name: &str) -> String {
    let mut text = line_text(daemon, services, name);
    text.push_str(&format!("\nline: {}", said(&services[name])));
    let run = daemon.services.known(name).run;
    let output = run
        .as_deref()
        .and_then(|id| runs::view(daemon, id))
        .map(|view| view.output)
        .unwrap_or_default();
    let lines: Vec<&str> = output.lines().collect();
    match lines.is_empty() {
        true => text.push_str("\n(no output)"),
        false => {
            let from = lines.len().saturating_sub(SHOWN_LINES);
            text.push_str(&format!(
                "\nthe end of its output (lines {}-{} of {}):\n{}",
                from + 1,
                lines.len(),
                lines.len(),
                lines[from..].join("\n")
            ));
        }
    }
    text
}
