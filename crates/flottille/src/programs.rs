//! The command lines a project's forges and tracker need (`gh`, `bkt`, `acli`), checked as the
//! providers' programs are (#306): found or not, its version, signed in or not, and what to do
//! when it is not. Read for the project's Settings (`project.programs`) and for `flottille
//! doctor`. Signed in is what `docs/forges.md` and `docs/tickets.md` say it is: `gh auth status`'s
//! exit code, `acli jira auth status`'s exit code, a `bkt` context for the host. Nothing here
//! reads a program's configuration folder, runs `bkt auth` nor prints what a program prints of the
//! account; Flottille installs nothing, it says how.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use flottille_core::declared::{self, ProjectTracker, RepositoryForge};
use flottille_core::paths::Paths;
use flottille_core::project::Project;
use flottille_core::registry::Registry;
use serde_json::{Value, json};

use crate::forge::{self, Gh, masked};

/// `<program> --version` is given this long.
const VERSION_TIME: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Program {
    Gh,
    Bkt,
    Acli,
}

impl Program {
    pub fn name(self) -> &'static str {
        match self {
            Program::Gh => "gh",
            Program::Bkt => "bkt",
            Program::Acli => "acli",
        }
    }

    /// The program's own installation page.
    fn install_page(self) -> &'static str {
        match self {
            Program::Gh => "https://cli.github.com/",
            Program::Bkt => "https://github.com/avivsinai/bitbucket-cli",
            Program::Acli => "https://developer.atlassian.com/cloud/acli/guides/install-acli/",
        }
    }

    /// A command only where one fits most systems: `gh` and `acli` are installed differently on
    /// each, `bkt`'s README leads with Homebrew, which macOS and Linux share.
    fn install_command(self) -> Option<&'static str> {
        match self {
            Program::Bkt => Some("brew install bitbucket-cli"),
            Program::Gh | Program::Acli => None,
        }
    }

    /// The commands that sign in, in order, to `host`; for `bkt`, `scope` is the workspace (Cloud)
    /// or the project (Data Center) of the repository, which its context names.
    fn sign_in(self, host: &str, scope: &str) -> Vec<String> {
        match self {
            Program::Gh => vec!["gh auth login".into()],
            Program::Acli => vec!["acli jira auth login".into()],
            // As `bkt`'s README gives them (<https://github.com/avivsinai/bitbucket-cli>): Cloud
            // by the browser, its API host in the context; Data Center by a personal access
            // token, a project in the context. Cloud's context names `api.bitbucket.org`, which
            // Flottille's own check accepts and `bkt`'s API needs: never `bitbucket.org` there.
            Program::Bkt if host == declared::BITBUCKET_CLOUD => vec![
                format!("bkt auth login https://{host} --kind cloud --web"),
                format!(
                    "bkt context create {}-{} --host api.{host} --workspace {scope}",
                    context_name(host),
                    context_name(scope)
                ),
            ],
            Program::Bkt => vec![
                format!("bkt auth login https://{host} --web-token"),
                format!(
                    "bkt context create {}-{} --host {host} --project {scope}",
                    context_name(host),
                    context_name(scope)
                ),
            ],
        }
    }
}

/// A name for the context of `host`: its letters and digits, the rest a hyphen.
fn context_name(host: &str) -> String {
    host.chars()
        .map(|c| match c.is_ascii_alphanumeric() {
            true => c.to_ascii_lowercase(),
            false => '-',
        })
        .collect()
}

/// What of a project needs a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Use {
    Repository(String),
    Tracker,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub program: Program,
    /// GitHub's, the Bitbucket server's, or the Jira site.
    pub host: String,
    /// What of the host the sign-in advice names: the workspace or project of a Bitbucket
    /// repository, empty for the others.
    pub scope: String,
    pub used: Use,
}

/// The programs a project's declarations need, and for what. Nothing is guessed: a repository
/// with no forge and a project with no tracker need none.
pub fn needs(project: &Project) -> Vec<Need> {
    let mut needs = Vec::new();
    for repository in &project.repositories {
        let Some(forge) = &repository.forge else {
            continue;
        };
        let program = match forge {
            RepositoryForge::Github { .. } => Program::Gh,
            _ => Program::Bkt,
        };
        let scope = match program {
            Program::Bkt => forge.owner_and_name().0.to_owned(),
            _ => String::new(),
        };
        needs.push(Need {
            program,
            host: forge.host().to_owned(),
            scope,
            used: Use::Repository(repository.path.clone()),
        });
    }
    match &project.tracker {
        Some(ProjectTracker::Github { .. }) => needs.push(Need {
            program: Program::Gh,
            host: declared::GITHUB.to_owned(),
            scope: String::new(),
            used: Use::Tracker,
        }),
        Some(ProjectTracker::Jira { site, .. }) => needs.push(Need {
            program: Program::Acli,
            host: site.clone(),
            scope: String::new(),
            used: Use::Tracker,
        }),
        None => {}
    }
    needs
}

/// Where `name` is on the daemon's PATH, the way a spawn finds it.
fn locate(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|file| {
            std::fs::metadata(file)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// `<program> --version`: the first dotted number it prints, none when it prints none.
async fn version_of(program: &Path) -> Option<String> {
    let child = tokio::process::Command::new(program)
        .arg("--version")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let output = tokio::time::timeout(VERSION_TIME, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    first_version(&String::from_utf8_lossy(&output.stdout))
}

/// The first word that is a number with a dot in it (`2.60.0`, `1.3.2-stable`), without a `v`.
fn first_version(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        let word = word.trim_start_matches('v');
        let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        (word.starts_with(|c: char| c.is_ascii_digit())
            && word.contains('.')
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c)))
        .then(|| word.to_owned())
    })
}

/// What a program says of its sign-in.
enum SignIn {
    Yes,
    No,
    /// Signed in, to another site than the one asked about (`acli` keeps one active site).
    Elsewhere(String),
}

/// Whether the program is signed in to `host`; the sentence of why it could not be said otherwise.
async fn signed_in(program: Program, host: &str) -> Result<SignIn, String> {
    let ran = |result: Result<(bool, String, String), Gh>| match result {
        Ok((ok, stdout, _)) => Ok((ok, stdout)),
        Err(Gh::Missing) => Err(format!("{} went missing", program.name())),
        Err(Gh::Failed(why)) => Err(masked(&why)),
    };
    let yes_or_no = |ok| if ok { SignIn::Yes } else { SignIn::No };
    match program {
        Program::Gh => ran(forge::gh(&["auth", "status", "--hostname", host]).await)
            .map(|(ok, _)| yes_or_no(ok)),
        Program::Bkt => forge::has_bitbucket_context(host).await.map(yes_or_no),
        Program::Acli => {
            let (ok, stdout) = ran(crate::tickets::acli(&["jira", "auth", "status"]).await)?;
            if !ok {
                return Ok(SignIn::No);
            }
            // The tickets' own reading of the `Site:` line, so that both say the same.
            match crate::tickets::signed_in_site(&stdout) {
                Some(active) if active.eq_ignore_ascii_case(host) => Ok(SignIn::Yes),
                Some(active) => Ok(SignIn::Elsewhere(masked(&active))),
                None => Err(
                    "acli did not say which site it is signed in to (no Site: line in \
                             `acli jira auth status`)"
                        .into(),
                ),
            }
        }
    }
}

/// One program's state for `host`, as the protocol says it:
/// `{program, host, found, path, version, signed_in, signed_in_to, install, sign_in, problem}`.
/// `signed_in` is null when the program is not found or could not say (`problem` then says why);
/// `signed_in_to` is the other site `acli` is signed in to, when it is; `install` is there when it
/// is not found, `sign_in` when it is found and signed out (or signed in elsewhere). The version
/// and the sign-in are asked together, each with its own time.
pub async fn check(program: Program, host: &str, scope: &str) -> Value {
    let mut state = json!({
        "program": program.name(),
        "host": host,
        "found": false,
        "path": null,
        "version": null,
        "signed_in": null,
        "signed_in_to": null,
        "install": null,
        "sign_in": null,
        "problem": null,
    });
    let Some(path) = locate(program.name()) else {
        state["install"] = json!({
            "page": program.install_page(),
            "command": program.install_command(),
        });
        return state;
    };
    state["found"] = json!(true);
    state["path"] = json!(path.display().to_string());
    let (version, signed) = tokio::join!(version_of(&path), signed_in(program, host));
    state["version"] = json!(version);
    match signed {
        Ok(SignIn::Yes) => state["signed_in"] = json!(true),
        Ok(SignIn::No) => {
            state["signed_in"] = json!(false);
            state["sign_in"] = json!({"commands": program.sign_in(host, scope)});
        }
        Ok(SignIn::Elsewhere(site)) => {
            state["signed_in"] = json!(false);
            state["signed_in_to"] = json!(site);
            state["sign_in"] =
                json!({"commands": [format!("acli jira auth switch --site {host}")]});
        }
        Err(why) => state["problem"] = json!(why),
    }
    state
}

/// What a check is known by: the program, its host and the scope the advice names.
type Key = (Program, String, String);

fn key_of(need: &Need) -> Key {
    (need.program, need.host.clone(), need.scope.clone())
}

/// The checks of `needs`, each program, host and scope asked once, all together: every program
/// has its own time, so one that hangs costs its own and not the sum of them.
async fn checks(needs: &[Need]) -> HashMap<Key, Value> {
    let mut distinct: Vec<&Need> = Vec::new();
    for need in needs {
        if !distinct.iter().any(|known| key_of(known) == key_of(need)) {
            distinct.push(need);
        }
    }
    let states = futures::future::join_all(
        distinct
            .iter()
            .map(|need| check(need.program, &need.host, &need.scope)),
    )
    .await;
    distinct
        .iter()
        .map(|need| key_of(need))
        .zip(states)
        .collect()
}

/// `project.programs`: `{"repositories": [{"repository", …state}], "tracker": state | null}`, for the
/// repositories that declare a forge and the tracker the project declares.
pub async fn of_project(project: &Project) -> Value {
    let needs = needs(project);
    let known = checks(&needs).await;
    let mut repositories = Vec::new();
    let mut tracker = Value::Null;
    for need in &needs {
        let mut state = known[&key_of(need)].clone();
        match &need.used {
            Use::Repository(path) => {
                state["repository"] = json!(path);
                repositories.push(state);
            }
            Use::Tracker => tracker = state,
        }
    }
    json!({"repositories": repositories, "tracker": tracker})
}

/// What `flottille doctor` adds: the programs of every registered project, once each, with what
/// of the projects uses them (`used_by`) and a `warning` for one that is missing or signed out.
pub fn doctor(paths: &Paths) -> Vec<Value> {
    let Ok(loaded) = Registry::new(paths).load_all() else {
        return Vec::new();
    };
    let mut needs = Vec::new();
    let mut used: Vec<(Key, Vec<String>)> = Vec::new();
    for project in &loaded.projects {
        for need in self::needs(project) {
            let what = match &need.used {
                Use::Repository(path) => format!("{}: {path}", project.name),
                Use::Tracker => format!("{}: tracker", project.name),
            };
            let key = key_of(&need);
            match used.iter_mut().find(|(known, _)| *known == key) {
                Some((_, by)) => by.push(what),
                None => {
                    used.push((key, vec![what]));
                    needs.push(need);
                }
            }
        }
    }
    if needs.is_empty() {
        return Vec::new();
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the checks");
    let known = runtime.block_on(checks(&needs));
    used.into_iter()
        .map(|(key, by)| {
            let mut state = known[&key].clone();
            state["used_by"] = json!(by);
            state["warning"] = json!(warning(&state));
            state
        })
        .collect()
}

/// The sentence that says what is wrong with a state and what to do; none when nothing is.
pub fn warning(state: &Value) -> Option<String> {
    let text = |key: &str| state[key].as_str().unwrap_or_default();
    let (program, host) = (text("program"), text("host"));
    let by = state["used_by"]
        .as_array()
        .map(|by| {
            let by: Vec<_> = by.iter().filter_map(Value::as_str).collect();
            format!(" ({})", by.join(", "))
        })
        .unwrap_or_default();
    if state["found"] != true {
        let install = &state["install"];
        let command = install["command"]
            .as_str()
            .map(|command| format!(", or run `{command}`"))
            .unwrap_or_default();
        return Some(format!(
            "{program} is not on PATH, and {host}{by} needs it: install it from {}{command}; the rest of Flottille works without it",
            install["page"].as_str().unwrap_or_default()
        ));
    }
    if let Some(problem) = state["problem"].as_str() {
        return Some(format!(
            "{program} could not be checked for {host}{by}: {problem}"
        ));
    }
    if state["signed_in"] == false {
        let signed_in_to = state["signed_in_to"]
            .as_str()
            .map(|site| format!(" (it is signed in to {site})"))
            .unwrap_or_default();
        let commands: Vec<String> = state["sign_in"]["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|command| format!("`{command}`"))
            .collect();
        return Some(format!(
            "{program} is not signed in to {host}{signed_in_to}{by}: run {}; the rest of Flottille works without it",
            commands.join(", then ")
        ));
    }
    None
}

/// The part of `flottille doctor`'s text for these entries, and the warnings it already says.
pub fn doctor_text(entries: &[Value]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut shown = String::from("Forges and trackers:\n");
    for state in entries {
        let text = |key: &str| state[key].as_str().unwrap_or_default();
        match state["warning"].as_str() {
            Some(warning) => shown.push_str(&format!("warn {warning}\n")),
            None => shown.push_str(&format!(
                "ok   {}: {} ({}), signed in to {}\n",
                text("program"),
                state["version"].as_str().unwrap_or("version unknown"),
                text("path"),
                text("host"),
            )),
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_the_first_dotted_number_a_program_prints() {
        assert_eq!(
            first_version("gh version 2.60.0 (2026-01-01)\nhttps://x/v2.60.0"),
            Some("2.60.0".into())
        );
        assert_eq!(
            first_version("acli version 1.3.2-stable"),
            Some("1.3.2-stable".into())
        );
        assert_eq!(first_version("bkt v0.37.0"), Some("0.37.0".into()));
        assert_eq!(first_version("no number here 2026-01-01"), None);
    }

    #[test]
    fn a_context_is_named_after_its_host() {
        assert_eq!(context_name("git.example.com"), "git-example-com");
    }

    #[test]
    fn bitbucket_cloud_signs_in_by_the_browser_and_its_context_names_the_api_host() {
        assert_eq!(
            Program::Bkt.sign_in("bitbucket.org", "acme"),
            [
                "bkt auth login https://bitbucket.org --kind cloud --web",
                "bkt context create bitbucket-org-acme --host api.bitbucket.org --workspace acme"
            ]
        );
    }
}
