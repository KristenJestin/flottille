//! The services of the machine (`docs/projects.md`, "Services"): programs a project's tasks lean
//! on that belong to no project (a database in a container, an API the apps call), defined once
//! in `$XDG_CONFIG_HOME/flottille/services.toml` and run once for every project and workspace.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::{Ready, Task, TaskUrl, check_task, valid_name};

/// The file's name, in Flottille's configuration folder.
pub const FILE: &str = "services.toml";

/// A service: a task of the machine, with the folder it runs in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    /// Absolute, or `~/…`; the home when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// One command line, run without a shell.
    pub line: String,
    /// The services started, and waited for, before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<Box<Ready>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<Box<TaskUrl>>,
    /// Held for approval each time an agent starts it.
    #[serde(default)]
    pub ask_before_running: bool,
}

impl Service {
    /// Where it runs: its folder, `~/` being the home, or the home itself.
    pub fn folder_in(&self, home: Option<&Path>) -> Result<PathBuf, String> {
        let home = || home.ok_or("the home is not known");
        match self.folder.as_deref() {
            None => Ok(home()?.to_path_buf()),
            Some("~") => Ok(home()?.to_path_buf()),
            Some(folder) => match folder.strip_prefix("~/") {
                Some(rest) => Ok(home()?.join(rest)),
                None => Ok(PathBuf::from(folder)),
            },
        }
    }

    /// The lines besides its own that a start of it runs: its ready's and its address's commands.
    pub fn other_lines(&self) -> Vec<&str> {
        self.ready
            .as_deref()
            .and_then(Ready::command)
            .into_iter()
            .chain(self.url.as_deref().and_then(TaskUrl::command))
            .collect()
    }

    /// The task its run is, for what runs a task's line: the same keys, nowhere in a project.
    pub fn as_task(&self) -> Task {
        Task {
            line: self.line.clone(),
            repository: None,
            serve: true,
            check: false,
            read_only: false,
            ask_before_running: self.ask_before_running,
            ready: self.ready.clone(),
            url: self.url.clone(),
            needs: self.needs.clone().into(),
            added_by: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    services: BTreeMap<String, Service>,
}

/// Reads and checks the text of `services.toml`: `Err` says the file, the key and what is wrong.
pub fn parse(text: &str) -> Result<BTreeMap<String, Service>, String> {
    let refuse = |message: String| format!("{FILE}: {message}");
    let reading = toml::Deserializer::parse(text).map_err(|e| refuse(e.message().to_owned()))?;
    let file: File = serde_path_to_error::deserialize(reading).map_err(|error| {
        let key = error.path().to_string();
        let cause = error.inner().message().to_owned();
        refuse(match key.as_str() {
            "." => cause,
            key => format!("{key}: {cause}"),
        })
    })?;
    check(&file.services).map_err(refuse)?;
    Ok(file.services)
}

/// What is wrong with the services, as `services.<name>.<key>: <what>`: the first found.
pub fn check(services: &BTreeMap<String, Service>) -> Result<(), String> {
    for (name, service) in services {
        check_service(name, service, &|need| services.contains_key(need))
            .map_err(|why| format!("services.{name}{why}"))?;
    }
    match cycle(services) {
        Some(cycle) => Err(format!(
            "services.{}.needs: a cycle: {}",
            cycle[0],
            cycle
                .windows(2)
                .map(|pair| format!("{} needs {}", pair[0], pair[1]))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        None => Ok(()),
    }
}

/// What is wrong with one service, as `<.key>: <what>` to follow `services.<name>`; `known` says
/// whether a name it needs is a service.
pub fn check_service(
    name: &str,
    service: &Service,
    known: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    if !valid_name(name) {
        return Err(
            ": the name must be 1 to 64 letters, digits, dots, dashes or underscores, \
                    starting with a letter or a digit"
                .into(),
        );
    }
    check_task(name, &service.as_task(), &|_| false)?;
    // A service has no workspace: only its address is filled.
    let only_url = |key: &str, template: &str, url: bool| {
        crate::variables::check_only(template, if url { &["url"] } else { &[] })
            .map_err(|why| format!(".{key}: {why}: a service has no workspace"))
    };
    match service.ready.as_deref() {
        Some(Ready::Line(text) | Ready::Url(text)) => only_url("ready", text, true)?,
        Some(Ready::Command(line)) => only_url("ready", line, false)?,
        None => {}
    }
    match service.url.as_deref() {
        Some(TaskUrl::Address(url)) => only_url("url", url, false)?,
        Some(TaskUrl::Command { command }) => only_url("url", command, false)?,
        None => {}
    }
    if let Some(folder) = &service.folder
        && !(folder == "~" || folder.starts_with("~/") || Path::new(folder).is_absolute())
    {
        return Err(".folder: must be absolute, or start with ~/ for the home".into());
    }
    check_needs(&service.needs, known).map_err(|why| format!(".needs: {why}"))?;
    if service.needs.iter().any(|need| need == name) {
        return Err(format!(".needs: a cycle: {name} needs {name}"));
    }
    Ok(())
}

/// What is wrong with a list of services needed: a name that is no service, or one twice.
pub fn check_needs(needs: &[String], known: &dyn Fn(&str) -> bool) -> Result<(), String> {
    for (at, need) in needs.iter().enumerate() {
        if !known(need) {
            return Err(format!("unknown service \"{need}\""));
        }
        if needs[..at].contains(need) {
            return Err(format!("\"{need}\" is named twice"));
        }
    }
    Ok(())
}

/// A cycle among the services' needs, as the names along it, the first again at its end.
pub fn cycle(services: &BTreeMap<String, Service>) -> Option<Vec<String>> {
    fn walk<'a>(
        services: &'a BTreeMap<String, Service>,
        name: &'a str,
        trail: &mut Vec<&'a str>,
        done: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        if let Some(at) = trail.iter().position(|seen| *seen == name) {
            let mut cycle: Vec<String> = trail[at..].iter().map(|n| (*n).to_owned()).collect();
            cycle.push(name.to_owned());
            return Some(cycle);
        }
        if done.contains(&name) {
            return None;
        }
        trail.push(name);
        for need in services.get(name).map_or(&[][..], |s| &s.needs[..]) {
            if let Some(cycle) = walk(services, need, trail, done) {
                return Some(cycle);
            }
        }
        trail.pop();
        done.push(name);
        None
    }
    let mut done = Vec::new();
    services
        .keys()
        .find_map(|name| walk(services, name, &mut Vec::new(), &mut done))
}

/// The services `needs` leads to, through their own needs, each after those it needs: the order
/// they start in. The file was checked: no cycle, no unknown name (one is left out).
pub fn in_order(services: &BTreeMap<String, Service>, needs: &[String]) -> Vec<String> {
    fn visit(services: &BTreeMap<String, Service>, name: &str, order: &mut Vec<String>) {
        let Some(service) = services.get(name) else {
            return;
        };
        if order.iter().any(|done| done == name) {
            return;
        }
        for need in &service.needs {
            visit(services, need, order);
        }
        if !order.iter().any(|done| done == name) {
            order.push(name.to_owned());
        }
    }
    let mut order = Vec::new();
    for need in needs {
        visit(services, need, &mut order);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE_TEXT: &str = r#"
[services.mongo]
folder = "~/tools/docker/mongo"
line = "docker compose up"
ready = { command = "docker compose exec -T mongo mongosh --quiet --eval 1" }

[services.redis]
line = "redis-server"

[services.auth]
folder = "/work/auth-api"
needs = ["mongo", "redis"]
line = "npm run start:dev"
url = "http://localhost:3001"
ready = { url = "{url}" }
"#;

    #[test]
    fn a_service_is_read_from_its_file_and_a_bad_key_is_refused_naming_it() {
        let services = parse(FILE_TEXT).unwrap();
        assert_eq!(services.len(), 3);
        let auth = &services["auth"];
        assert_eq!(auth.needs, ["mongo", "redis"]);
        assert_eq!(
            services["mongo"].folder_in(Some(Path::new("/home/me"))),
            Ok(PathBuf::from("/home/me/tools/docker/mongo"))
        );
        assert_eq!(
            services["redis"].folder_in(Some(Path::new("/home/me"))),
            Ok(PathBuf::from("/home/me"))
        );

        let typo = FILE_TEXT.replace("line = \"redis-server\"", "lines = \"redis-server\"");
        let refused = parse(&typo).unwrap_err();
        assert!(
            refused.starts_with("services.toml: services.redis"),
            "{refused}"
        );
        assert!(refused.contains("lines"), "{refused}");

        let relative = FILE_TEXT.replace("\"/work/auth-api\"", "\"work/auth-api\"");
        assert_eq!(
            parse(&relative).unwrap_err(),
            "services.toml: services.auth.folder: must be absolute, or start with ~/ for the home"
        );
        let workspace = FILE_TEXT.replace("\"{url}\"", "\"{url}/{workspace}\"");
        let refused = parse(&workspace).unwrap_err();
        assert!(
            refused.starts_with("services.toml: services.auth.ready: "),
            "{refused}"
        );
        assert!(refused.contains("a service has no workspace"), "{refused}");
    }

    #[test]
    fn an_unknown_service_in_a_service_s_needs_is_refused_naming_it() {
        let text = FILE_TEXT.replace("[\"mongo\", \"redis\"]", "[\"mongo\", \"postgres\"]");
        assert_eq!(
            parse(&text).unwrap_err(),
            "services.toml: services.auth.needs: unknown service \"postgres\""
        );
    }

    #[test]
    fn a_cycle_in_the_services_needs_is_refused_naming_it() {
        let text = format!("{FILE_TEXT}\n[services.mongo2]\nline = \"x\"\n").replace(
            "line = \"redis-server\"",
            "line = \"redis-server\"\nneeds = [\"auth\"]",
        );
        assert_eq!(
            parse(&text).unwrap_err(),
            "services.toml: services.auth.needs: a cycle: auth needs redis, redis needs auth"
        );
        let own = "[services.a]\nline = \"x\"\nneeds = [\"a\"]\n";
        assert_eq!(
            parse(own).unwrap_err(),
            "services.toml: services.a.needs: a cycle: a needs a"
        );
    }

    #[test]
    fn the_services_needed_come_after_those_they_need() {
        let services = parse(FILE_TEXT).unwrap();
        assert_eq!(
            in_order(&services, &["auth".into()]),
            ["mongo", "redis", "auth"]
        );
        assert_eq!(
            in_order(&services, &["redis".into(), "auth".into()]),
            ["redis", "mongo", "auth"]
        );
    }
}
