//! A project's repositories as the daemon reads their checkouts (`docs/projects.md`): each with
//! the branch its checkout is on, its base rule, where its remote leads (or "local only") and its
//! forge, and the ones the folder holds that the project does not name. The project settings list
//! them (Add, Remove…) and read them again when the tab opens and each time the window takes the
//! focus back, in silence; the project page reads them. Remove
//! only forgets a repository: its folder, its branches and the worktrees of the workspaces that
//! took it stay. The window writes nothing itself: the daemon writes the project's file, its
//! comments kept (`project.repositories`, `project.add_repository`,
//! `project.remove_repository`). The view of this model is the confirmation of a removal; the
//! rows are the settings page's.

use flottille_core::declared::RepositoryForge;
use gpui_design_system::{Confirm, Toast, toast};
use gpui_kit::{App, Context, IntoElement, Render, SharedString, WeakEntity, Window};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::perform_later;
use crate::project::repository_label;
use crate::shell::Shell;

/// A repository of the project as the daemon reads its checkout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepositoryInfo {
    pub path: String,
    /// Whether the folder still holds it.
    pub found: bool,
    pub branch: Option<String>,
    pub remote: String,
    /// Where its remote leads, masked; none when it has no such remote: local only.
    pub url: Option<String>,
    /// Its forge; none when none is set.
    pub forge: Option<RepositoryForge>,
    /// Its base rule.
    pub base: Vec<String>,
    /// Its local branches.
    pub branches: Vec<String>,
}

impl RepositoryInfo {
    fn of(value: &Value) -> Self {
        let words = |key: &str| -> Vec<String> {
            value[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        Self {
            path: value["path"].as_str().unwrap_or_default().to_owned(),
            found: value["found"] == true,
            branch: value["branch"].as_str().map(str::to_owned),
            remote: value["remote"].as_str().unwrap_or("origin").to_owned(),
            url: value["url"].as_str().map(str::to_owned),
            forge: serde_json::from_value::<RepositoryForge>(value["forge"].clone()).ok(),
            base: words("base"),
            branches: words("branches"),
        }
    }

    /// Where it stands, in one line: `main · origin git@github.com:acme/api.git · GitHub
    /// acme/api`, or `main · local only · no forge`.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if !self.found {
            parts.push("not found in the folder".to_owned());
        }
        parts.extend(self.branch.clone());
        parts.push(match &self.url {
            Some(url) => format!("{} {url}", self.remote),
            None => "local only".to_owned(),
        });
        parts.push(
            self.forge
                .as_ref()
                .map_or_else(|| "no forge".to_owned(), RepositoryForge::said),
        );
        parts.join(" · ")
    }
}

/// The project's repositories and the ones its folder holds besides, as the daemon answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repositories {
    pub listed: Vec<RepositoryInfo>,
    /// Found in the folder, not in the project.
    pub new: Vec<String>,
}

impl Repositories {
    pub fn of(value: &Value) -> Self {
        Self {
            listed: value["repositories"]
                .as_array()
                .into_iter()
                .flatten()
                .map(RepositoryInfo::of)
                .collect(),
            new: value["new"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        }
    }
}

/// What the daemon is doing for the section.
#[derive(Clone, Debug, PartialEq)]
enum Busy {
    Adding(String),
    Removing(String),
}

pub struct ProjectRepositories {
    shell: WeakEntity<Shell>,
    project: String,
    read: Option<Repositories>,
    /// The repository whose Remove waits for its confirmation.
    confirming: Option<String>,
    /// The repository the confirmation names, kept while it fades away.
    asked: String,
    busy: Option<Busy>,
    /// A read of the window's own is on its way: another is not asked meanwhile.
    reading: bool,
    /// How many requests were made: an answer to an older one never replaces a newer one's.
    requests: u64,
    problem: Option<SharedString>,
}

impl ProjectRepositories {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            read: None,
            confirming: None,
            asked: String::new(),
            busy: None,
            reading: false,
            requests: 0,
            problem: None,
        }
    }

    pub fn repositories(&self) -> Option<&Repositories> {
        self.read.as_ref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Whether a read of the window's own is on its way.
    pub fn is_reading(&self) -> bool {
        self.reading
    }

    /// Whether the user's request is this one.
    pub fn is_adding(&self, path: &str) -> bool {
        matches!(&self.busy, Some(Busy::Adding(adding)) if adding == path)
    }

    pub fn is_confirming(&self) -> bool {
        self.confirming.is_some()
    }

    /// The number of the last request: its answer is the one that counts.
    pub fn last_request(&self) -> u64 {
        self.requests
    }

    fn ask(&mut self, method: &'static str, params: Value, busy: Option<Busy>, cx: &mut App) {
        self.busy = busy;
        self.problem = None;
        self.send(method, params, cx);
    }

    fn send(&mut self, method: &'static str, params: Value, cx: &mut App) {
        self.requests += 1;
        let command = Command::Repositories {
            project: self.project.clone(),
            method,
            params,
            serial: self.requests,
        };
        perform_later(&self.shell, command, cx);
    }

    /// Asks the daemon for the repositories as their checkouts stand, in silence: when the tab
    /// opens and when the window takes the focus back. Not while one is on its way, nor while a
    /// request of the user's is: its answer is the list, and a newer read would make it the older
    /// one, so that its refusal would never be said.
    pub fn read(&mut self, cx: &mut App) {
        if self.busy.is_some() || self.reading {
            return;
        }
        self.reading = true;
        let params = json!({"name": self.project});
        self.send("project.repositories", params, cx);
    }

    /// Adds the repository at `path` of the project's folder.
    pub fn add(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = path.trim().to_owned();
        if path.is_empty() || self.busy.is_some() {
            return;
        }
        let params = json!({"name": self.project, "path": path});
        self.ask(
            "project.add_repository",
            params,
            Some(Busy::Adding(path)),
            cx,
        );
        cx.notify();
    }

    /// Asks to forget `path`, in a confirmation; called again from it, forgets it.
    pub fn remove(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(path) {
            self.confirming = Some(path.to_owned());
            self.asked = path.to_owned();
            cx.notify();
            return;
        }
        self.confirming = None;
        let params = json!({"name": self.project, "path": path});
        let busy = Some(Busy::Removing(path.to_owned()));
        self.ask("project.remove_repository", params, busy, cx);
        cx.notify();
    }

    /// Whether the answer to request `serial` is still the one that counts.
    pub fn is_latest(&self, serial: u64) -> bool {
        serial >= self.requests
    }

    /// The daemon's answer to request `serial`: the list as it is now, or why not. An answer to
    /// an older request than the last is dropped, and the busy words stay until the last one.
    pub fn answered(
        &mut self,
        serial: u64,
        read: Result<Value, String>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_latest(serial) {
            return;
        }
        let busy = self.busy.take();
        self.reading = false;
        let project = self.project.clone();
        let label = |path: &str| repository_label(&project, path);
        match read {
            Ok(value) => {
                self.problem = None;
                let read = Repositories::of(&value);
                let said = match &busy {
                    None => None,
                    Some(Busy::Adding(path)) => Some(
                        Toast::success(format!("{} added to {project}", label(path)))
                            .detail("Each new workspace gets its own copy of it."),
                    ),
                    Some(Busy::Removing(path)) => Some(
                        Toast::success(format!("{} removed from {project}", label(path)))
                            .detail("It shows again as found in the folder, and Add puts it back."),
                    ),
                };
                if let Some(said) = said {
                    toast(said, cx);
                }
                self.read = Some(read);
            }
            Err(why) => match &busy {
                None => self.problem = Some(why.into()),
                Some(Busy::Adding(path)) => toast(
                    Toast::failure(format!("{} was not added", label(path))).detail(why),
                    cx,
                ),
                Some(Busy::Removing(path)) => toast(
                    Toast::failure(format!("{} was not removed", label(path))).detail(why),
                    cx,
                ),
            },
        }
        cx.notify();
    }

    /// The confirmation of forgetting a repository.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let path = self.asked.clone();
        let label = repository_label(&self.project, &path);
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new(
            "repository-removal",
            format!("Remove {label} from the project?"),
        )
        .sentence(
            "New workspaces no longer take it. The repository itself is not touched, nor the \
             workspaces that took it. The folder still holds it: it shows as found there, and \
             Add puts it back.",
        )
        .action("Remove repository")
        .open(self.confirming.is_some())
        .on_confirm(move |_, cx| {
            let path = path.clone();
            this.update(cx, |this, cx| this.remove(&path, cx)).ok();
        })
        .on_cancel(move |_, cx| {
            again
                .update(cx, |this, cx| {
                    this.confirming = None;
                    cx.notify();
                })
                .ok();
        })
    }
}

impl Render for ProjectRepositories {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.confirm(cx)
    }
}

/// What the new-workspace form needs of the repositories: each one's base rule.
pub fn rules_of(read: &Repositories) -> std::collections::HashMap<String, Vec<String>> {
    read.listed
        .iter()
        .map(|info| (info.path.clone(), info.base.clone()))
        .collect()
}
