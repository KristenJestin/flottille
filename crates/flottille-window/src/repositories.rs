//! A project's repositories on its page (`docs/projects.md`): each with the branch its checkout
//! is on, where its remote leads (or "local only") and its forge; the ones the folder holds that
//! the project does not name, detected again, each with Add; a field that adds one by its path;
//! and Remove, which only forgets a repository: its folder, its branches and the worktrees of the
//! workspaces that took it stay. The window writes nothing itself: the daemon writes the project's
//! file, its comments kept (`project.repositories`, `project.add_repository`,
//! `project.remove_repository`).

use flottille_core::declared::RepositoryForge;
use flottille_ui::{Button, Icon, Item, Listing, Type, enter, icon, palette};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    App, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _,
    WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{line_field, perform_later, problem_line};
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
    /// Its forge in words (`GitHub acme/api`); none when none is set.
    pub forge: Option<String>,
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
            forge: serde_json::from_value::<RepositoryForge>(value["forge"].clone())
                .ok()
                .map(|forge| forge.said()),
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
        parts.push(self.forge.clone().unwrap_or_else(|| "no forge".to_owned()));
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

/// A row of the section: a repository of the project, or one found in its folder.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Known(RepositoryInfo),
    Found(String),
}

/// What the daemon is doing for the section.
#[derive(Clone, Debug, PartialEq)]
enum Busy {
    Detecting,
    Adding(String),
    Removing(String),
}

pub struct ProjectRepositories {
    shell: WeakEntity<Shell>,
    project: String,
    read: Option<Repositories>,
    rows: Listing<Row>,
    /// The repository whose Remove waits for its confirmation.
    confirming: Option<String>,
    busy: Option<Busy>,
    /// How many requests were made: an answer to an older one never replaces a newer one's.
    requests: u64,
    problem: Option<SharedString>,
    /// Where a repository's path is typed to add it, made at the first draw.
    field: Option<Entity<InputState>>,
    _subscription: Option<Subscription>,
}

impl ProjectRepositories {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            read: None,
            rows: Listing::default(),
            confirming: None,
            busy: None,
            requests: 0,
            problem: None,
            field: None,
            _subscription: None,
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

    pub fn field(&self) -> Option<&Entity<InputState>> {
        self.field.as_ref()
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

    /// Asks the daemon for the repositories as their checkouts stand. Not while a request of the
    /// user's is on its way: its answer is the list, and a newer read would make it the older one,
    /// so that its refusal would never be said.
    pub fn read(&mut self, cx: &mut App) {
        if self.busy.is_some() {
            return;
        }
        let params = json!({"name": self.project});
        self.send("project.repositories", params, cx);
    }

    /// Detect again: the same read, said on its button while it runs.
    pub fn detect(&mut self, cx: &mut Context<Self>) {
        let params = json!({"name": self.project});
        self.ask("project.repositories", params, Some(Busy::Detecting), cx);
        cx.notify();
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

    /// Asks to forget `path`; the second click confirms.
    pub fn remove(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(path) {
            self.confirming = Some(path.to_owned());
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_latest(serial) {
            return;
        }
        let added = matches!(self.busy.take(), Some(Busy::Adding(_)));
        match read {
            Ok(value) => {
                self.problem = None;
                self.read = Some(Repositories::of(&value));
                if added && let Some(field) = &self.field {
                    field.update(cx, |field, cx| field.set_value("", window, cx));
                }
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    fn field_of(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(field) = &self.field {
            return field.clone();
        }
        let field = line_field(
            "add a repository of the folder by its path, for example libs/core",
            window,
            cx,
        );
        self._subscription = Some(cx.subscribe(&field, |this, field, event, cx| {
            match event {
                InputEvent::PressEnter { .. } => {
                    let typed = field.read(cx).value().to_string();
                    this.add(&typed, cx);
                }
                // Typing clears what a refusal said.
                InputEvent::Change if this.problem.take().is_some() => cx.notify(),
                _ => {}
            }
        }));
        self.field = Some(field.clone());
        field
    }

    fn known_row(&self, info: &RepositoryInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let path = info.path.clone();
        let id = |part: &str| ElementId::Name(format!("{part}-{path}").into());
        let label = repository_label(&self.project, &info.path);
        let confirming = self.confirming.as_deref() == Some(&info.path);
        let removing = self.busy == Some(Busy::Removing(info.path.clone()));
        let action = if removing {
            div()
                .mono(11.5)
                .text_color(p.faint)
                .child("Removing…")
                .into_any_element()
        } else if confirming {
            let (cancel, remove) = (path.clone(), path.clone());
            let group = div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(6.))
                .child(
                    div()
                        .id(id("forget-note"))
                        .max_w(px(300.))
                        .text_size(px(12.))
                        .text_color(p.mute)
                        .child(
                            "Flottille forgets it: its folder, its branches and the worktrees of \
                             its workspaces stay as they are.",
                        )
                        .test_support(),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .child(
                            Button::quiet(id("cancel-remove-repository"), "Cancel").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if this.confirming.as_deref() == Some(&cancel) {
                                        this.confirming = None;
                                        cx.notify();
                                    }
                                }),
                            ),
                        )
                        .child(
                            Button::secondary(
                                id("confirm-remove-repository"),
                                format!("Forget {label}"),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx))),
                        ),
                );
            enter(id("confirm-repository"), 0, group).into_any_element()
        } else {
            let remove = path.clone();
            Button::quiet(id("remove-repository"), "Remove")
                .disabled(
                    self.busy.is_some() || self.read.as_ref().is_some_and(|r| r.listed.len() < 2),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx)))
                .into_any_element()
        };
        div()
            .id(id("repository-row"))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .flex()
            .items_start()
            .gap(px(10.))
            .child(
                div()
                    .pt(px(2.))
                    .child(icon(Icon::Repository).size(px(14.)).text_color(p.faint)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .medium()
                            .child(SharedString::from(label)),
                    )
                    .child(
                        div()
                            .id(id("repository-meta"))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .mono(11.5)
                            .text_color(if info.found { p.faint } else { p.fail })
                            .aria_label(SharedString::from(info.line()))
                            .child(SharedString::from(info.line()))
                            .test_support(),
                    ),
            )
            .child(action)
            .test_support()
    }

    fn found_row(&self, path: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("{part}-{path}").into());
        let adding = self.busy == Some(Busy::Adding(path.to_owned()));
        let added = path.to_owned();
        div()
            .id(id("found-repository"))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_dashed()
            .border_color(p.line2)
            .flex()
            .items_center()
            .gap(px(10.))
            .child(icon(Icon::Repository).size(px(14.)).text_color(p.faint))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(p.mute)
                            .child(SharedString::from(path.to_owned())),
                    )
                    .child(
                        div()
                            .mono(11.5)
                            .text_color(p.faint)
                            .child("found in the folder, not in the project"),
                    ),
            )
            .child(
                Button::secondary(
                    id("add-found-repository"),
                    if adding { "Adding…" } else { "Add" },
                )
                .disabled(self.busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.add(&added, cx))),
            )
            .test_support()
    }
}

impl Render for ProjectRepositories {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if let Some(read) = &self.read {
            let rows = read
                .listed
                .iter()
                .map(|info| (format!("known-{}", info.path), Row::Known(info.clone())))
                .chain(
                    read.new
                        .iter()
                        .map(|path| (format!("found-{path}"), Row::Found(path.clone()))),
                );
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows.sync(rows, now, reduce);
        }
        let rows: Vec<_> = self
            .rows
            .rows()
            .map(|row| {
                let content = match row.value {
                    Row::Known(info) => self.known_row(info, cx).into_any_element(),
                    Row::Found(path) => self.found_row(path, cx).into_any_element(),
                };
                Item::new(
                    ElementId::Name(format!("repository-item-{}", row.key).into()),
                    row.placement,
                    content,
                )
                .gap(px(8.))
            })
            .collect();
        let count = self.read.as_ref().map_or(0, |r| r.listed.len());
        let detecting = self.busy == Some(Busy::Detecting);
        let field = self.field_of(window, cx);
        let typed = !field.read(cx).value().trim().is_empty();
        let adding_typed = matches!(&self.busy, Some(Busy::Adding(path))
            if self.read.as_ref().is_none_or(|r| !r.new.contains(path)));
        div()
            .id("project-repositories")
            .mb(px(30.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mb(px(12.))
                    .text_size(px(13.))
                    .semibold()
                    .child("Repositories")
                    .child(
                        div()
                            .mono(11.)
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(p.faint)
                            .child(count.to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::quiet(
                            "detect-repositories",
                            if detecting {
                                "Detecting…"
                            } else {
                                "Detect again"
                            },
                        )
                        .leading(Icon::Search)
                        .disabled(self.busy.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.detect(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(rows)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .id("repository-path-field")
                                    .flex_1()
                                    .h(px(32.))
                                    .px(px(10.))
                                    .rounded(px(8.))
                                    .bg(p.field)
                                    .border_1()
                                    .border_color(p.line2)
                                    .flex()
                                    .items_center()
                                    .mono(12.5)
                                    .child(Input::new(&field))
                                    .test_support(),
                            )
                            .child(
                                Button::secondary(
                                    "add-repository",
                                    if adding_typed { "Adding…" } else { "Add" },
                                )
                                .leading(Icon::Plus)
                                .disabled(!typed || self.busy.is_some())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let typed = this
                                            .field
                                            .as_ref()
                                            .map(|f| f.read(cx).value().to_string())
                                            .unwrap_or_default();
                                        this.add(&typed, cx);
                                    },
                                )),
                            ),
                    )
                    .child(problem_line(
                        "repositories-problem",
                        self.problem.clone(),
                        8.,
                        cx,
                    )),
            )
            .test_support()
    }
}

/// What the new-workspace form needs of the repositories: each one's base rule.
pub fn rules_of(read: &Repositories) -> std::collections::HashMap<String, Vec<String>> {
    read.listed
        .iter()
        .map(|info| (info.path.clone(), info.base.clone()))
        .collect()
}
