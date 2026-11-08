//! The confirmation of a project's removal (`docs/projects.md`, "Removing a project"): before
//! anything is done, it reads what the removal would do (`project.removal`) and says it in
//! labelled lines: the sessions and command runs still running, which it stops, then each
//! workspace with its state, removed or kept, and the choice to also remove those holding unsaved
//! work. What the removal did is said in a toast.

use flottille_ui::{
    Confirm, Icon, LabelledLine, Skeleton, Switch, Toast, awaiting, caption, changing, enter,
    presence, space, toast,
};
use gpui_kit::{
    AnyElement, App, Context, ElementId, IntoElement, ParentElement, Render, Styled, WeakEntity,
    Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::perform_later;
use crate::shell::Shell;

/// What `project.removal` says the removal would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Removal {
    /// The sessions still running: their agent and title.
    pub sessions: Vec<(String, String)>,
    /// The command runs still running: their id and line.
    pub runs: Vec<(String, String)>,
    pub workspaces: Vec<WorkspaceLoss>,
}

/// A workspace of the project and what removing it would lose.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceLoss {
    pub name: String,
    /// `clean`, `uncommitted`, `unpushed` or `unknown`.
    pub state: String,
    pub losses: Vec<String>,
}

impl WorkspaceLoss {
    /// Whether removing it loses work, or might: it is kept unless the user says to discard.
    pub fn unsaved(&self) -> bool {
        self.state != "clean"
    }

    fn state_said(&self) -> &'static str {
        match self.state.as_str() {
            "clean" => "clean",
            "uncommitted" => "uncommitted changes",
            "unpushed" => "commits not pushed",
            _ => "cannot be checked",
        }
    }
}

impl Removal {
    pub fn of(value: &Value) -> Self {
        let list = |key: &str| value[key].as_array().cloned().unwrap_or_default();
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Self {
            sessions: list("sessions")
                .iter()
                .map(|s| {
                    let title = s["title"]
                        .as_str()
                        .map_or_else(|| text(&s["id"]), str::to_owned);
                    (text(&s["id"]), title)
                })
                .collect(),
            runs: list("runs")
                .iter()
                .map(|r| (text(&r["run"]), text(&r["line"])))
                .collect(),
            workspaces: list("workspaces")
                .iter()
                .map(|w| WorkspaceLoss {
                    name: text(&w["name"]),
                    state: text(&w["state"]),
                    losses: w["losses"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(text)
                        .collect(),
                })
                .collect(),
        }
    }
}

pub struct ProjectRemoval {
    shell: WeakEntity<Shell>,
    project: String,
    open: bool,
    /// How many reads were asked: an answer to an older one is dropped.
    serial: u64,
    read: Option<Removal>,
    discard: bool,
    removing: bool,
    refused: Option<String>,
}

impl ProjectRemoval {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            open: false,
            serial: 0,
            read: None,
            discard: false,
            removing: false,
            refused: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn read(&self) -> Option<&Removal> {
        self.read.as_ref()
    }

    pub fn discards(&self) -> bool {
        self.discard
    }

    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    /// Whether a removal was asked and has not answered.
    pub fn is_removing(&self) -> bool {
        self.removing
    }

    /// Opens the confirmation and reads what the removal would do, anew each time.
    pub fn ask(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.discard = false;
        self.read = None;
        self.serial += 1;
        let command = Command::ReadRemoval {
            project: self.project.clone(),
            serial: self.serial,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The answer to read `serial`. A refusal closes the dialog and says why in a toast.
    pub fn answered(&mut self, serial: u64, read: Result<Value, String>, cx: &mut Context<Self>) {
        if serial != self.serial || !self.open {
            return;
        }
        match read {
            Ok(value) => self.read = Some(Removal::of(&value)),
            Err(why) => {
                self.open = false;
                toast(
                    Toast::failure(format!(
                        "Could not read what removing {} does",
                        self.project
                    ))
                    .detail(why),
                    cx,
                );
            }
        }
        cx.notify();
    }

    pub fn set_discard(&mut self, discard: bool, cx: &mut Context<Self>) {
        self.discard = discard;
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.removing = true;
        self.refused = None;
        let command = Command::RemoveProject {
            project: self.project.clone(),
            discard: self.discard,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// What the daemon answered to the removal: a toast says what it did, or why it did not.
    pub fn removed(&mut self, result: Result<Value, String>, cx: &mut Context<Self>) {
        self.removing = false;
        match result {
            Ok(done) => {
                let mut toasted =
                    Toast::success(format!("The project {} was removed", self.project));
                if let Some(detail) = done_said(&done) {
                    toasted = toasted.detail(detail);
                }
                toast(toasted, cx);
            }
            Err(why) => {
                toast(
                    Toast::failure(format!("The project {} was not removed", self.project))
                        .detail(why.clone()),
                    cx,
                );
                self.refused = Some(why);
            }
        }
        cx.notify();
    }
}

/// What a removal did, in one line: `Stopped 2 agents and 1 command run · removed 1 workspace ·
/// kept draft (uncommitted changes)`; none when it only forgot the project.
pub fn done_said(done: &Value) -> Option<String> {
    let count = |value: &Value| value.as_array().map_or(0, Vec::len);
    let plural = |n: usize, word: &str| match n {
        1 => format!("1 {word}"),
        n => format!("{n} {word}s"),
    };
    let (agents, runs) = (
        count(&done["stopped"]["agents"]),
        count(&done["stopped"]["runs"]),
    );
    let mut parts = Vec::new();
    match (agents, runs) {
        (0, 0) => {}
        (a, 0) => parts.push(format!("stopped {}", plural(a, "agent"))),
        (0, r) => parts.push(format!("stopped {}", plural(r, "command run"))),
        (a, r) => parts.push(format!(
            "stopped {} and {}",
            plural(a, "agent"),
            plural(r, "command run")
        )),
    }
    let removed = count(&done["workspaces"]["removed"]);
    if removed > 0 {
        parts.push(format!("removed {}", plural(removed, "workspace")));
    }
    let kept: Vec<String> = done["workspaces"]["kept"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| w["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    if !kept.is_empty() {
        parts.push(format!("kept {} (unsaved work)", kept.join(", ")));
    }
    let mut said = parts.join(" · ");
    if let Some(first) = said.get(..1) {
        said = first.to_uppercase() + &said[1..];
    }
    (!said.is_empty()).then_some(said)
}

fn id(part: impl std::fmt::Display) -> ElementId {
    ElementId::Name(format!("project-removal-{part}").into())
}

/// One labelled line of the dialog.
fn line(key: String, glyph: Icon, label: &str, value: String) -> AnyElement {
    LabelledLine::new(id(key), glyph, label.to_owned(), value.clone())
        .child(value)
        .into_any_element()
}

/// A block of lines under its caption, waiting for the read with a skeleton of its size.
fn block(
    key: &str,
    title: &str,
    count: String,
    pending: bool,
    lines: Vec<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let skeleton = div()
        .flex()
        .flex_col()
        .gap(space::SM)
        .py(space::SM)
        .child(Skeleton::new(id(format!("{key}-skeleton-1"))).w(px(240.)))
        .child(Skeleton::new(id(format!("{key}-skeleton-2"))).w(px(180.)));
    div()
        .flex()
        .flex_col()
        .child(caption(
            id(format!("{key}-caption")),
            title.to_owned(),
            count,
            cx,
        ))
        .child(awaiting(
            id(format!("{key}-wait")),
            pending,
            skeleton,
            div().flex().flex_col().children(lines),
        ))
}

impl Render for ProjectRemoval {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = self.read.is_none();
        let read = self.read.clone().unwrap_or_default();
        let discard = self.discard;

        let mut running: Vec<AnyElement> = read
            .sessions
            .iter()
            .map(|(agent, title)| {
                line(
                    format!("session-{agent}"),
                    Icon::Agents,
                    "Session",
                    title.clone(),
                )
            })
            .collect();
        running.extend(read.runs.iter().map(|(run, command)| {
            line(
                format!("run-{run}"),
                Icon::Terminal,
                "Command",
                command.clone(),
            )
        }));
        if running.is_empty() {
            running.push(line(
                "nothing-runs".to_owned(),
                Icon::Check,
                "Running",
                "nothing".to_owned(),
            ));
        }
        let runs_count = (read.sessions.len() + read.runs.len()).to_string();

        let mut workspaces: Vec<AnyElement> = read
            .workspaces
            .iter()
            .map(|workspace| {
                let fate = match workspace.unsaved() && !discard {
                    true => "kept",
                    false => "removed",
                };
                let said = format!("{} · {fate}", workspace.state_said());
                let key = format!("workspace-{}", workspace.name);
                let (name, built) = (workspace.name.clone(), said.clone());
                changing(id(format!("{key}-fade")), said, move |_| {
                    line(key, Icon::Folder, &name, built)
                })
                .into_any_element()
            })
            .collect();
        if workspaces.is_empty() {
            workspaces.push(line(
                "no-workspace".to_owned(),
                Icon::Check,
                "Workspaces",
                "none".to_owned(),
            ));
        }
        let workspaces_count = read.workspaces.len().to_string();
        let unsaved = read.workspaces.iter().any(WorkspaceLoss::unsaved);

        let switch = Switch::new(
            id("discard"),
            discard,
            "Also remove workspaces with unsaved work",
        )
        .hint("Their uncommitted changes and commits not pushed are lost.")
        .on_click(cx.listener(move |this, _, _, cx| this.set_discard(!discard, cx)));

        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new(
            "project-removal",
            format!("Remove the project {}?", self.project),
        )
        .sentence(
            "Flottille stops what runs in it, removes its workspaces and forgets it. Its \
                 folder and its repositories are not touched.",
        )
        .action(match discard && unsaved {
            true => "Remove project and unsaved work",
            false => "Remove project",
        })
        .open(self.open)
        .on_confirm(move |_, cx| {
            this.update(cx, |this, cx| this.remove(cx)).ok();
        })
        .on_cancel(move |_, cx| {
            again.update(cx, |this, cx| this.cancel(cx)).ok();
        })
        .child(enter(
            id("running"),
            0,
            block("running", "Stopped", runs_count, pending, running, cx),
        ))
        .child(enter(
            id("workspaces"),
            1,
            block(
                "workspaces",
                "Workspaces",
                workspaces_count,
                pending,
                workspaces,
                cx,
            ),
        ))
        .child(enter(
            id("discard-enter"),
            2,
            presence(id("discard-block"), unsaved, switch).gap(space::LG),
        ))
    }
}
