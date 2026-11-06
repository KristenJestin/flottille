//! The commands the agents run, as the window shows them: the rail's Runs section (every command
//! of the session's tree, running ones first, the last ended ones in sight and the older ones
//! behind one folded row), the panel that follows one's output live, and the one compact line a
//! command's result is in the conversation.
//!
//! What a program wrote is drawn as the daemon gave it (masked): its colours (`ESC [ … m`) turned
//! into the palette's, every other escape sequence dropped (`flottille_core::ansi`). The stored
//! output is untouched.

use std::cell::Cell;
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flottille_core::ansi;
use flottille_ui::color::mix;
use flottille_ui::motion::{MODAL, Motion, PANEL};
use flottille_ui::{
    AgentState, Button, Icon, Item, Listing, Modal, Palette, Skeleton, StateIcon, Status, Toast,
    Type, awaiting, changing, fade, fold_frame, hoverable, icon, palette, presence, section_label,
    space, tipped, tipped_when_cut, toast, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Entity, FocusHandle, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Task, TestSupportExt as _, Transformation, WeakEntity,
    Window, div, px, radians,
};
use serde_json::Value;

use crate::conversation::{CommandCard, Ending};
use crate::services::ServicesPage;
use crate::session::{SessionPage, THE_CONVERSATION, seconds};
use crate::shell::now_ms;
use crate::tasks::ProjectTasks;

/// Where a list of commands and its panel are: a session's rail, a project page, which lists
/// the tasks the user ran there, or the services page, which shows a service's run.
#[derive(Clone)]
pub enum Owner {
    Session(WeakEntity<SessionPage>),
    Project(WeakEntity<ProjectTasks>),
    Services(WeakEntity<ServicesPage>),
}

impl From<WeakEntity<ServicesPage>> for Owner {
    fn from(page: WeakEntity<ServicesPage>) -> Self {
        Owner::Services(page)
    }
}

impl From<WeakEntity<SessionPage>> for Owner {
    fn from(page: WeakEntity<SessionPage>) -> Self {
        Owner::Session(page)
    }
}

impl From<WeakEntity<ProjectTasks>> for Owner {
    fn from(tasks: WeakEntity<ProjectTasks>) -> Self {
        Owner::Project(tasks)
    }
}

impl Owner {
    /// Opens a run's address with the system's default handler.
    fn open_link(&self, url: &str, cx: &mut App) {
        match self {
            Owner::Session(page) => {
                if let Some(page) = page.upgrade() {
                    page.read(cx).open_link(url);
                }
            }
            Owner::Project(tasks) => {
                if let Some(tasks) = tasks.upgrade() {
                    tasks.read(cx).open_link(url, cx);
                }
            }
            Owner::Services(page) => {
                if let Some(page) = page.upgrade() {
                    page.read(cx).open_link(url, cx);
                }
            }
        }
    }

    fn open(&self, run: &str, cx: &mut App) {
        match self {
            Owner::Session(page) => page.update(cx, |page, cx| page.open_command(run, cx)).ok(),
            Owner::Project(tasks) => tasks
                .update(cx, |tasks, cx| tasks.open_command(run, cx))
                .ok(),
            Owner::Services(page) => page.update(cx, |page, cx| page.open_run(run, cx)).ok(),
        };
    }

    fn close(&self, cx: &mut App) {
        match self {
            Owner::Session(page) => page.update(cx, |page, cx| page.close_command(cx)).ok(),
            Owner::Project(tasks) => tasks.update(cx, |tasks, cx| tasks.close_command(cx)).ok(),
            Owner::Services(page) => page.update(cx, |page, cx| page.close_output(cx)).ok(),
        };
    }

    fn stop(&self, run: &str, cx: &mut App) {
        match self {
            Owner::Session(page) => page.update(cx, |page, cx| page.stop_command(run, cx)).ok(),
            Owner::Project(tasks) => tasks
                .update(cx, |tasks, cx| tasks.stop_command(run, cx))
                .ok(),
            Owner::Services(page) => page.update(cx, |page, cx| page.stop_run(run, cx)).ok(),
        };
    }

    fn restart(&self, run: &str, cx: &mut App) {
        match self {
            Owner::Session(page) => page
                .update(cx, |page, cx| page.restart_command(run, cx))
                .ok(),
            Owner::Project(tasks) => tasks
                .update(cx, |tasks, cx| tasks.restart_command(run, cx))
                .ok(),
            Owner::Services(page) => page.update(cx, |page, cx| page.start_run(run, cx)).ok(),
        };
    }

    /// Shows the conversation of the agent that started a run: a session's page does.
    fn show_agent(&self, agent: &str, window: &mut Window, cx: &mut App) {
        if let Owner::Session(page) = self {
            page.update(cx, |page, cx| page.show_agent(agent, window, cx))
                .ok();
        }
    }
}

/// A command of the session, as `run.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRow {
    pub run: String,
    /// The agent that started it; empty for a task the user ran.
    pub agent: String,
    /// The task it runs, none for a line of the agent's.
    pub name: Option<String>,
    /// The workspace it runs in, none for the project folder.
    pub workspace: Option<String>,
    pub line: String,
    /// `running`, `exited`, `stopped` or `interrupted`.
    pub state: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    /// The last line of its output with something on it, as the program wrote it.
    pub last_line: String,
    /// The last lines of its output, as the program wrote them: what the card of its result
    /// shows, while it runs as once it ended.
    pub tail: Vec<String>,
    /// A step of a workspace's setup, not a command: the daemon starts it, nobody stops it from
    /// here, and its end is `done` or `failed`.
    pub step: bool,
    /// `starting`, `ready` or `missed` for a task that says when it is ready; none otherwise.
    pub ready: Option<String>,
    /// Where it answers, as its task says.
    pub url: Option<String>,
    /// A service's run: the daemon's, of no agent and no project; `name` is the service's.
    pub service: bool,
}

/// What the drawer follows: a command, or a step of a workspace's setup.
#[derive(Clone, Debug, PartialEq)]
pub enum Watch {
    Run(String),
    Step { workspace: String, position: u32 },
}

impl Watch {
    /// The key the drawer holds it under.
    pub fn key(&self) -> String {
        match self {
            Watch::Run(run) => run.clone(),
            Watch::Step {
                workspace,
                position,
            } => format!("step:{workspace}:{position}"),
        }
    }
}

impl CommandRow {
    pub fn of(value: &Value) -> CommandRow {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        CommandRow {
            run: text("run"),
            agent: text("agent"),
            name: value["name"].as_str().map(str::to_owned),
            workspace: value["workspace"].as_str().map(str::to_owned),
            line: text("line"),
            state: text("state"),
            exit_code: value["exit_code"].as_i64().map(|code| code as i32),
            signal: value["signal"].as_i64().map(|signal| signal as i32),
            started_at: value["started_at"].as_u64().unwrap_or(0),
            ended_at: value["ended_at"].as_u64(),
            last_line: text("last_line"),
            tail: value["tail"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|line| line.as_str().map(str::to_owned))
                .collect(),
            step: false,
            ready: value["ready"].as_str().map(str::to_owned),
            url: value["url"].as_str().map(str::to_owned),
            service: value["project"] == "" && value["agent"] == "",
        }
    }

    /// A step of a setup as `workspace.step_output` gives it, held under `key`.
    pub fn of_step(value: &Value, key: &str) -> CommandRow {
        let place = match value["repository"].as_str() {
            Some(repository) => format!(
                "{} · {repository}",
                value["workspace"].as_str().unwrap_or_default()
            ),
            None => value["workspace"].as_str().unwrap_or_default().to_owned(),
        };
        let (state, exit_code) = match value["state"].as_str() {
            Some("running") => ("running", None),
            Some("failed") => ("exited", Some(1)),
            _ => ("exited", Some(0)),
        };
        CommandRow {
            run: key.to_owned(),
            agent: String::new(),
            name: None,
            workspace: Some(place),
            line: value["line"].as_str().unwrap_or_default().to_owned(),
            state: state.to_owned(),
            exit_code,
            signal: None,
            started_at: value["started_at"].as_u64().unwrap_or(0),
            ended_at: value["ended_at"].as_u64(),
            last_line: String::new(),
            tail: Vec::new(),
            step: true,
            ready: None,
            url: None,
            service: false,
        }
    }

    pub fn running(&self) -> bool {
        self.state == "running"
    }

    /// A run of one of the project's tasks, not of a one-off line (`run {"line": …}`): the daemon
    /// gives a run its task's name, and a line none.
    pub fn is_task(&self) -> bool {
        self.name.is_some()
    }

    /// `starting` or `ready` while it runs, for a task that says when it is ready.
    pub fn readiness(&self) -> Option<&str> {
        self.ready
            .as_deref()
            .filter(|ready| self.running() && matches!(*ready, "starting" | "ready"))
    }

    /// Its address while it runs: a server's, which is shown in place of its last line.
    pub fn address(&self) -> Option<&str> {
        self.url.as_deref().filter(|_| self.running())
    }

    pub fn ending(&self) -> Ending {
        match (self.state.as_str(), self.exit_code, self.signal) {
            ("running", ..) => Ending::Running,
            ("exited", Some(code), _) => Ending::Exited(code),
            ("exited", None, signal) => Ending::Killed(signal.unwrap_or_default()),
            ("stopped", ..) => Ending::Stopped,
            _ => Ending::Interrupted,
        }
    }

    /// How long it has run, or ran.
    pub fn elapsed_ms(&self, now: u64) -> u64 {
        self.ended_at.unwrap_or(now).saturating_sub(self.started_at)
    }
}

/// How many lines the window of the last lines slid by between two readings: the smallest shift
/// that leaves what stayed of `before` at the start of `after`.
fn slid(before: &[String], after: &[String]) -> usize {
    (0..=before.len())
        .find(|&shift| {
            let stayed = &before[shift..];
            after.len() >= stayed.len() && after[..stayed.len()] == *stayed
        })
        .unwrap_or(before.len())
}

/// A command's output: the last lines the daemon gave, as the program wrote them.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandOutput {
    pub row: CommandRow,
    pub lines: Vec<String>,
    /// Lines before these were left out.
    pub earlier: bool,
}

impl CommandOutput {
    /// A step's output as `workspace.step_output` gives it, held under `key`.
    pub fn of_step(value: &Value, key: &str) -> CommandOutput {
        CommandOutput {
            row: CommandRow::of_step(value, key),
            ..CommandOutput::of(value)
        }
    }

    pub fn of(value: &Value) -> CommandOutput {
        CommandOutput {
            row: CommandRow::of(value),
            lines: value["output"]
                .as_str()
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect(),
            earlier: value["earlier"] == true,
        }
    }
}

/// What a command's ending is drawn with: an icon, its ink and its words. The shape says it as
/// well as the colour.
pub fn mark_of(ending: Ending, p: &Palette) -> (Icon, Hsla, String) {
    let (mark, ink) = match ending {
        Ending::Running => (Icon::Running, p.run),
        Ending::Exited(0) => (Icon::Check, p.ok),
        Ending::Exited(_) | Ending::Killed(_) => (Icon::Close, p.fail),
        Ending::Stopped => (Icon::Stopped, p.faint),
        Ending::Interrupted => (Icon::Interrupted, p.faint),
    };
    (mark, ink, ending.said())
}

/// A colour of a program's output, in the palette: the ink of the state it stands for.
fn ink(color: ansi::Color, p: &Palette) -> Hsla {
    match (color.index, color.bright) {
        (0, _) => p.faint,
        (1, _) => p.fail,
        (2, _) => p.ok,
        (3, _) => p.wait,
        (4, _) => p.run,
        (5, _) => mix(p.run, p.fail, 0.5),
        (6, _) => mix(p.run, p.ok, 0.5),
        (_, true) => p.text,
        _ => p.mute,
    }
}

/// A line of output with its colours, on one line: the text is `base`, its colours the palette's.
/// What it says without them is its label.
pub fn coloured_line(id: ElementId, text: &str, base: Hsla, p: &Palette) -> impl IntoElement {
    let spans = ansi::spans(text);
    let plain: String = spans.iter().map(|span| span.text.as_str()).collect();
    // `flex_none`: in a column of fixed height a line that may shrink (it clips its overflow)
    // is squeezed onto the ones around it once the output is longer than the column.
    div()
        .id(id)
        .flex()
        .flex_none()
        .mono(11.5)
        .text_color(base)
        .whitespace_nowrap()
        .overflow_hidden()
        .aria_label(SharedString::from(plain))
        .children(spans.into_iter().map(|span| {
            div()
                .flex_none()
                .when_some(span.color, |d, color| d.text_color(ink(color, p)))
                .when(span.bold, |d| d.semibold())
                .child(SharedString::from(span.text))
        }))
        .test_support()
}
/// A command's result in the conversation, as one compact line: an icon, the line, how it ended
/// (a colour and a shape) and how long it took. The output is not here: a click opens it in the
/// panel the rail opens. `at` makes the ids.
pub fn card(
    card: &CommandCard,
    at: usize,
    open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    let (mark, color, said) = mark_of(card.ending, &p);
    let id = |part: &str| ElementId::Name(format!("command-{part}-{at}").into());
    let line = div()
        .id(id("card"))
        .flex()
        .items_center()
        .gap(space::SM)
        .px(space::MD)
        .py(space::SM)
        .rounded(px(8.))
        .border_1()
        .border_color(p.line)
        .cursor_pointer()
        .on_click(open)
        .child(icon(Icon::Terminal).size(px(13.)).text_color(p.faint))
        .child(
            div()
                .id(id("line"))
                .flex_1()
                .min_w_0()
                .mono(12.5)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .aria_label(SharedString::from(card.line.clone()))
                .child(SharedString::from(card.line.clone()))
                .test_support(),
        )
        .children(card.allowed_by.as_ref().map(|by| {
            div()
                .id(id("allowed"))
                .flex_none()
                .text_size(px(11.5))
                .text_color(p.faint)
                .child(SharedString::from(format!("allowed by {by}")))
                .test_support()
        }))
        .child(
            div()
                .id(id("exit"))
                .flex_none()
                .flex()
                .items_center()
                .gap(space::XS)
                .mono(11.5)
                .text_color(color)
                .aria_label(SharedString::from(said.clone()))
                .child(icon(mark).size(px(12.)).text_color(color))
                .child(SharedString::from(said))
                .test_support(),
        )
        .children(card.seconds.map(|seconds_taken| {
            div()
                .id(id("seconds"))
                .flex_none()
                .mono(11.)
                .text_color(p.faint)
                .child(SharedString::from(seconds((seconds_taken * 1000.) as u64)))
                .test_support()
        }));
    hoverable(line, move |d, t| d.bg(p.hover.opacity(t))).into_any_element()
}

/// The finished runs that stay listed under the running ones, the latest first.
pub const KEPT_ENDED: usize = 5;

/// The rail's runs, sorted: what is listed.
#[derive(Debug, PartialEq)]
pub struct Arranged<'a> {
    /// Running first, in the order the daemon lists them.
    pub running: Vec<&'a CommandRow>,
    /// Then the last [`KEPT_ENDED`] to have finished, the latest first.
    pub kept: Vec<&'a CommandRow>,
}

/// Sorts the task runs of the section: the running ones, then the last ones to have finished.
pub fn arrange(rows: &[CommandRow]) -> Arranged<'_> {
    arrange_of(rows, None)
}

/// [`arrange`] for the runs `agent` started, or for every agent's when none is given. A one-off
/// line is left out: its card in the conversation is where it is seen.
pub fn arrange_of<'a>(rows: &'a [CommandRow], agent: Option<&str>) -> Arranged<'a> {
    let mine = |row: &&CommandRow| row.is_task() && agent.is_none_or(|agent| row.agent == agent);
    let running = rows
        .iter()
        .filter(mine)
        .filter(|row| row.running())
        .collect();
    let mut ended: Vec<&CommandRow> = rows
        .iter()
        .filter(mine)
        .filter(|row| !row.running())
        .collect();
    let ended_at = |row: &CommandRow| row.ended_at.unwrap_or(row.started_at);
    ended.sort_by_key(|row| std::cmp::Reverse((ended_at(row), row.started_at)));
    ended.truncate(KEPT_ENDED);
    Arranged {
        running,
        kept: ended,
    }
}

/// How far the children behind a folded "N ended" row are unfolded: they stay drawn until the
/// fold ends.
pub(crate) struct EndedFold {
    pub motion: Motion,
    /// The children's height, measured while they are drawn.
    pub height: Rc<Cell<Pixels>>,
}

impl EndedFold {
    pub fn new() -> Self {
        Self {
            motion: Motion::settled(0.0),
            height: Rc::new(Cell::new(px(0.))),
        }
    }

    /// Unfolds or folds over [`PANEL`].
    pub fn animate(&mut self, open: bool, now: Instant, reduce: bool) {
        self.motion
            .animate(if open { 1.0 } else { 0.0 }, PANEL, now, reduce);
    }
}

/// The row that gathers what ended, under the Agents' and under the runs: a chevron that turns
/// as its level folds, and `label`, which `said` names.
pub(crate) fn ended_row(
    id: ElementId,
    said: SharedString,
    label: impl IntoElement,
    open: bool,
    turn: f32,
    p: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let row = div()
        .id(id)
        .aria_expanded(open)
        .aria_label(said)
        .h(px(28.))
        .flex()
        .items_center()
        .gap(space::SM)
        .px(space::SM)
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(12.))
        .child(
            icon(Icon::Chevron)
                .size(px(11.))
                .text_color(p.faint)
                .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * (1.0 - turn)))),
        )
        .child(label)
        .on_click(on_click);
    hoverable(row, move |d, t| {
        d.bg(p.hover.opacity(t)).text_color(mix(p.faint, p.text, t))
    })
}

/// Who started a command: the task, then the agent as `names` calls it, or the user. The root
/// agent is not named: its conversation is the page the rail is on, so the name says nothing.
fn who(row: &CommandRow, names: &HashMap<String, String>) -> Option<String> {
    // A server that says when it is ready says that instead of who started it.
    if let (Some(name), Some(ready)) = (&row.name, row.readiness()) {
        return Some(format!("task {name} · {ready}"));
    }
    let agent = match row.agent.as_str() {
        "" => Some("you".to_owned()),
        agent => names
            .get(agent)
            .cloned()
            .or_else(|| Some(format!("agent {agent}"))),
    }
    .filter(|name| name != THE_CONVERSATION);
    match (&row.name, agent) {
        (Some(name), Some(agent)) => Some(format!("task {name} · {agent}")),
        (Some(name), None) => Some(format!("task {name}")),
        (None, agent) => agent,
    }
}

/// A command's mark: a spinner while it runs, else the icon of how it ended, in a 16 px box. A
/// server that says when it is ready crossfades from its spinner to a solid dot once it is.
fn run_mark(
    id: ElementId,
    dot: ElementId,
    row: &CommandRow,
    spinner: Option<AnyElement>,
    p: &Palette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (mark, color, said) = mark_of(row.ending(), p);
    if let Some(readiness) = row.readiness() {
        // Where no spinner turns (the drawer), the running mark stands for it.
        let spinner = spinner.unwrap_or_else(|| {
            icon(Icon::Running)
                .size(px(12.))
                .text_color(p.run)
                .into_any_element()
        });
        return ready_mark(id, dot, readiness == "ready", Some(spinner), p, window, cx);
    }
    div()
        .id(id)
        .flex_none()
        .w(px(16.))
        .h(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .aria_label(SharedString::from(said))
        .map(|d| match spinner.filter(|_| row.running()) {
            Some(spinner) => d.child(spinner),
            None => d.child(icon(mark).size(px(12.)).text_color(color)),
        })
        .test_support()
        .into_any_element()
}

/// The mark of a server that says when it is ready: its spinner while it starts, then a solid dot
/// of the ok colour, the one fading into the other over `PANEL`. Its label says which; the dot is
/// the element `dot` once it shows.
pub fn ready_mark(
    id: ElementId,
    dot: ElementId,
    ready: bool,
    spinner: Option<AnyElement>,
    p: &Palette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = fade(
        ElementId::NamedChild(Arc::new(id.clone()), "ready".into()),
        ready,
        PANEL,
        window,
        cx,
    );
    let layer = || {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
    };
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(16.))
        .h(px(18.))
        .aria_label(if ready { "ready" } else { "starting" })
        .when(t < 1.0, |d| {
            d.children(spinner.map(|spinner| layer().opacity(1.0 - t).child(spinner)))
        })
        .when(t > 0.0, |d| {
            d.child(
                layer().opacity(t).child(
                    div()
                        .id(dot)
                        .size(px(8.))
                        .rounded_full()
                        .bg(p.ok)
                        .test_support(),
                ),
            )
        })
        .test_support()
        .into_any_element()
}

/// An address as a row shows it: its host and path, without its scheme.
pub fn address_shown(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    rest.trim_end_matches('/').to_owned()
}

/// A run's address as a link: its host and path in the accent, cut with an ellipsis, a small
/// external-link icon after it, the whole address in a tooltip. A click opens it with the
/// system's handler, and only that: the row's own click is not given it. Its ink fades on hover.
pub fn address_link(
    id: ElementId,
    url: &str,
    size: f32,
    open: impl Fn(&str, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let p = palette(cx);
    let shown = address_shown(url);
    let url = url.to_owned();
    // The words are the element `id`, which a test finds; the link around them takes the click.
    let link = div()
        .id(ElementId::Name(format!("{id}-click").into()))
        .flex()
        .items_center()
        .gap(space::XS)
        .min_w_0()
        .cursor_pointer()
        .mono(size)
        .child(
            div()
                .id(id.clone())
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .aria_label(SharedString::from(shown.clone()))
                .child(SharedString::from(shown))
                .test_support(),
        )
        .child(icon(Icon::OpenInEditor).size(px(size - 1.)).flex_none())
        .on_click({
            let url = url.clone();
            move |_, _, cx| {
                cx.stop_propagation();
                open(&url, cx);
            }
        });
    let link = hoverable(link, move |d, t| d.text_color(mix(p.acc, p.text, t)));
    let tipped_frame = div()
        .id(ElementId::Name(format!("{id}-tip").into()))
        .flex()
        .min_w_0()
        .child(link);
    tipped(tipped_frame, url, size + 6., window, cx).into_any_element()
}

/// The rail's Runs section: nothing while the session ran no command.
pub struct CommandList {
    page: Owner,
    rows: Vec<CommandRow>,
    /// The agent whose runs are listed; none lists every agent's (the project page's).
    agent: Option<String>,
    /// The running runs as drawn: one that goes folds away before it is gone.
    running: Listing<CommandRow>,
    /// The ended runs as drawn, behind the `N ended` row.
    ended: Listing<CommandRow>,
    /// The `N ended` row is open.
    open_ended: bool,
    fold: EndedFold,
    /// How many ended runs the row last said, drawn while it folds away.
    ended_said: usize,
    /// How the tree names each agent.
    names: HashMap<String, String>,
    /// The spinner of each running command.
    spinners: HashMap<String, Entity<StateIcon>>,
    error: Option<SharedString>,
    /// The daemon listed the commands once: until then the section waits as a skeleton.
    known: bool,
    /// Draws the time again each second, while a command runs.
    ticker: Option<Task<()>>,
    renders: usize,
}

impl CommandList {
    pub fn new(page: impl Into<Owner>) -> Self {
        Self {
            page: page.into(),
            rows: Vec::new(),
            agent: None,
            running: Listing::default(),
            ended: Listing::default(),
            open_ended: false,
            fold: EndedFold::new(),
            ended_said: 0,
            names: HashMap::new(),
            spinners: HashMap::new(),
            error: None,
            known: false,
            ticker: None,
            renders: 0,
        }
    }

    /// Lists the runs of `agent` only.
    pub fn of_agent(mut self, agent: String) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Every run of the session's tree, whoever started it and whatever it is.
    pub fn rows(&self) -> &[CommandRow] {
        &self.rows
    }

    /// The runs listed, as the section sorts them now: the task runs of the agent shown.
    pub fn arranged(&self) -> Arranged<'_> {
        arrange_of(&self.rows, self.agent.as_deref())
    }

    /// The agent whose runs are listed changed: the other's runs fade in, and what ended is
    /// folded again.
    pub fn show_agent(&mut self, agent: &str, cx: &mut Context<Self>) {
        if self.agent.as_deref() == Some(agent) {
            return;
        }
        self.agent = Some(agent.to_owned());
        self.open_ended = false;
        self.fold = EndedFold::new();
        cx.notify();
    }

    /// The `N ended` row opens or folds; the runs behind it come and go over [`PANEL`].
    fn toggle_ended(&mut self, cx: &mut Context<Self>) {
        self.open_ended = !self.open_ended;
        let now = cx.background_executor().now();
        self.fold.animate(self.open_ended, now, cx.reduce_motion());
        cx.notify();
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// What the daemon listed; nothing is drawn again when it is what was drawn.
    pub fn set(&mut self, rows: Vec<CommandRow>, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.known, true) && self.rows == rows {
            // Nothing to list: the skeleton gives way to the empty section.
            cx.notify();
            return;
        }
        if self.rows == rows {
            return;
        }
        let running = rows.iter().any(CommandRow::running);
        self.spinners
            .retain(|run, _| rows.iter().any(|row| &row.run == run && row.running()));
        for row in rows.iter().filter(|row| row.running()) {
            self.spinners.entry(row.run.clone()).or_insert_with(|| {
                let status = cx.new(|_| Status::new(AgentState::Running));
                cx.new(|cx| StateIcon::new(status, cx))
            });
        }
        self.rows = rows;
        if running && self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
        if !running {
            self.ticker = None;
        }
        cx.notify();
    }

    pub fn set_names(&mut self, names: HashMap<String, String>, cx: &mut Context<Self>) {
        if self.names != names {
            self.names = names;
            cx.notify();
        }
    }

    /// The daemon refused what was asked of a command, and why; none clears it.
    pub fn failed(&mut self, why: Option<String>, cx: &mut Context<Self>) {
        self.error = why.map(Into::into);
        cx.notify();
    }

    /// One run: its mark, its line, who started it and how long (or how it ended), the last line
    /// of its output, and Stop while it runs. A click opens its drawer.
    fn run_row(
        &self,
        row: &CommandRow,
        now: u64,
        p: Palette,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let ended = !row.running();
        let (_, color, said) = mark_of(row.ending(), &p);
        let run = row.run.clone();
        let id = |part: &str| ElementId::Name(format!("command-{part}-{run}").into());
        let open = {
            let page = self.page.clone();
            let run = run.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| page.open(&run, cx)
        };
        let last = ansi::strip(&row.last_line);
        // The address while it runs; once it ends, the last one is drawn while it folds away.
        let address = row.address().map(str::to_owned);
        let link = match &row.url {
            Some(url) => {
                let owner = self.page.clone();
                address_link(
                    id("link"),
                    url,
                    11.,
                    move |url, cx| owner.open_link(url, cx),
                    window,
                    cx,
                )
            }
            None => div().into_any_element(),
        };
        let who = who(row, &self.names);
        let has_who = who.is_some();
        let time = seconds(row.elapsed_ms(now));
        let line = |id: ElementId, text: String, window: &mut Window, cx: &mut App| {
            tipped_when_cut(
                div()
                    .id(id)
                    .mono(11.)
                    .text_color(p.faint)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .aria_label(SharedString::from(text.clone()))
                    .child(SharedString::from(text.clone())),
                SharedString::from(text),
                window,
                cx,
            )
        };
        let stop = (!ended).then(|| {
            let (page, run) = (self.page.clone(), run.clone());
            Button::icon(id("stop"), Icon::Stop, "Stop").on_click(move |_, _, cx| {
                cx.stop_propagation();
                page.stop(&run, cx);
            })
        });
        let card = div()
            .id(ElementId::Name(format!("command-{}", row.run).into()))
            .flex()
            .items_start()
            .gap(space::SM)
            .p(space::SM)
            .rounded(px(8.))
            .cursor_pointer()
            .on_click(open)
            .child(run_mark(
                id("mark"),
                id("dot"),
                row,
                self.spinners
                    .get(&row.run)
                    .cloned()
                    .map(IntoElement::into_any_element),
                &p,
                window,
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(tipped_when_cut(
                        div()
                            .id(id("line"))
                            .mono(12.5)
                            .medium()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .aria_label(SharedString::from(row.line.clone()))
                            .child(SharedString::from(row.line.clone())),
                        SharedString::from(row.line.clone()),
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::XS)
                            .mono(11.)
                            .text_color(p.faint)
                            .when(ended, |d| {
                                d.child(
                                    div()
                                        .id(id("status"))
                                        .flex_none()
                                        .text_color(color)
                                        .aria_label(SharedString::from(said.clone()))
                                        .child(SharedString::from(said.clone()))
                                        .test_support(),
                                )
                                .child("·")
                            })
                            .children(who.map(|who| {
                                tipped_when_cut(
                                    div()
                                        .id(id("who"))
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .aria_label(SharedString::from(who.clone()))
                                        .child(SharedString::from(who.clone())),
                                    SharedString::from(who),
                                    window,
                                    cx,
                                )
                            }))
                            .children(has_who.then_some("·"))
                            .child(
                                div()
                                    .id(id("time"))
                                    .flex_none()
                                    .child(SharedString::from(time))
                                    .test_support(),
                            ),
                    )
                    // A server's address takes the place of its last line, which its output
                    // still shows; a run that wrote nothing has no line to show, not an empty one.
                    .child(presence(id("link-presence"), address.is_some(), link))
                    .child(presence(
                        id("last-presence"),
                        address.is_none() && !last.is_empty(),
                        line(id("last"), last, window, cx),
                    )),
            )
            .children(stop)
            .opacity(if ended { 0.75 } else { 1.0 });
        hoverable(card, move |d, t| d.bg(p.hover.opacity(t))).into_any_element()
    }
}

impl Render for CommandList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let arranged = self.arranged();
        let keyed = |rows: &[&CommandRow]| -> Vec<(String, CommandRow)> {
            rows.iter()
                .map(|row| (row.run.clone(), (*row).clone()))
                .collect()
        };
        let (running, ended) = (keyed(&arranged.running), keyed(&arranged.kept));
        let at = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.running.sync(running, at, reduce);
        self.ended.sync(ended, at, reduce);
        let ended_count = self.ended.len();
        if ended_count > 0 {
            self.ended_said = ended_count;
        }
        // A folding row still has its runs until the fold ends.
        let mut drawn_open = self.open_ended;
        if self.fold.motion.is_running(at) {
            window.request_animation_frame();
            drawn_open = true;
        } else {
            self.fold.motion.settle(at);
        }
        let p = palette(cx);
        let now = now_ms();
        let present = !self.running.is_empty() || ended_count > 0;
        let draw =
            |this: &Self, listing: &Listing<CommandRow>, window: &mut Window, cx: &mut App| {
                let listed: Vec<(String, CommandRow, flottille_ui::Placement)> = listing
                    .rows()
                    .map(|listed| {
                        (
                            listed.key.to_owned(),
                            listed.value.clone(),
                            listed.placement,
                        )
                    })
                    .collect();
                listed
                    .into_iter()
                    .map(|(key, row, placement)| {
                        let content = this.run_row(&row, now, p, window, cx);
                        Item::new(
                            ElementId::Name(format!("command-item-{key}").into()),
                            placement,
                            content,
                        )
                        .into_any_element()
                    })
                    .collect::<Vec<AnyElement>>()
            };
        let running_rows = draw(self, &self.running, window, cx);
        let ended_rows = match drawn_open {
            true => draw(self, &self.ended, window, cx),
            false => Vec::new(),
        };
        let ended_id = ElementId::Name("rail-runs-ended".into());
        let turn = turning(ended_id.clone(), self.open_ended, window, cx);
        let said = SharedString::from(format!("{} ended", self.ended_said));
        let ended_block = presence(
            "rail-runs-ended-block",
            ended_count > 0,
            div()
                .flex()
                .flex_col()
                .child(ended_row(
                    ended_id,
                    said.clone(),
                    changing("rail-runs-ended-count", said, |said| {
                        div().child(said).into_any_element()
                    }),
                    self.open_ended,
                    turn,
                    p,
                    cx.listener(|this, _, _, cx| this.toggle_ended(cx)),
                ))
                .when(drawn_open, |d| {
                    d.child(fold_frame(
                        "rail-runs-ended-children",
                        self.open_ended,
                        self.fold.motion.is_running(at),
                        self.fold.motion.value(at),
                        &self.fold.height,
                        div().flex().flex_col().children(ended_rows),
                    ))
                }),
        );
        // Switching the agent shown fades the other's runs in.
        let body = div()
            .flex()
            .flex_col()
            .children(running_rows)
            .child(ended_block)
            .children(self.error.clone().map(|error| {
                div()
                    .id("rail-runs-error")
                    .px(space::SM)
                    .py(space::XS)
                    .text_size(px(11.5))
                    .text_color(p.fail)
                    .child(error)
                    .test_support()
            }))
            .into_any_element();
        let section = div()
            .id("rail-runs")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .pb(space::MD)
            .child(
                div()
                    .id("rail-runs-title")
                    .aria_label("Runs")
                    .child(rail_label("Runs", None, cx))
                    .test_support(),
            )
            .child(changing(
                "rail-runs-body",
                self.agent.clone().unwrap_or_default(),
                move |_| body,
            ))
            .test_support();
        // Before the first listing the section is a skeleton: a run's row, which folds away when
        // the session ran nothing.
        let waiting = div()
            .id("rail-runs-skeleton")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .pb(space::MD)
            .child(
                div().h(px(32.)).flex().items_center().child(
                    Skeleton::new("rail-runs-skeleton-title")
                        .w(px(40.))
                        .h(px(10.)),
                ),
            )
            .child(
                div()
                    .px(space::SM)
                    .py(space::XS)
                    .child(Skeleton::new("rail-runs-skeleton-row").h(px(40.))),
            )
            .test_support();
        awaiting(
            "rail-runs-wait",
            !self.known,
            waiting,
            presence("rail-runs-presence", present, section),
        )
    }
}

/// A rail section's title row: 32 px, its label in small capitals, and what goes on its right.
pub fn rail_label(text: &str, right: Option<AnyElement>, cx: &App) -> impl IntoElement {
    div()
        .h(px(32.))
        .flex()
        .items_center()
        .justify_between()
        .child(section_label(text, cx))
        .children(right)
}

/// The detail of one run, in the drawer that slides in from the right: what it runs, where, who
/// started it, since when or how it ended, its output following the end, Copy, and Stop while it
/// runs. The session's rail and its conversation, and the project page, open the same.
pub struct CommandPanel {
    page: Owner,
    /// The command whose detail is open.
    run: Option<String>,
    /// Its row, as the owner's list gives it.
    row: Option<CommandRow>,
    shown: Option<CommandOutput>,
    /// How the session's tree names each agent, and the session's title.
    names: HashMap<String, String>,
    session: Option<String>,
    /// Where the focus rests while it is open, so that Escape reaches the drawer.
    focus: Option<FocusHandle>,
    focused: bool,
    renders: usize,
    /// The output as the drawer draws it.
    feed: Feed,
    /// The number of the first line of `shown`, counted from the first line read (the daemon does
    /// not say how many it left out before it): it gives the last lines only, so it moves on as
    /// the output grows past what it gives.
    first: usize,
    /// The command seen large, in a modal over the window.
    expanded: bool,
    /// The output as the modal draws it: its own, since it opens when the drawer has long been
    /// open.
    modal: Feed,
    /// When the modal was closed: it still draws its content while it fades out.
    collapsed_at: Option<Instant>,
    /// The run whose restart was asked, until the new run has taken its place: Stop and Restart
    /// wait, so that a second click starts nothing.
    restarting: Option<String>,
}

/// An output drawn in a scrolling box: the lines there when it was first read are drawn as they
/// are, the ones that follow unfold as they arrive.
struct Feed {
    scroll: ScrollHandle,
    /// How many lines the output had when it was first read, counted from the first line read.
    base: Option<usize>,
    arrivals: Listing<String>,
}

impl Feed {
    fn new(base: Option<usize>) -> Self {
        Feed {
            scroll: ScrollHandle::new(),
            base,
            arrivals: Listing::default(),
        }
    }
}

impl CommandPanel {
    pub fn new(page: impl Into<Owner>) -> Self {
        Self {
            page: page.into(),
            run: None,
            row: None,
            shown: None,
            names: HashMap::new(),
            session: None,
            focus: None,
            focused: false,
            renders: 0,
            feed: Feed::new(None),
            first: 0,
            expanded: false,
            modal: Feed::new(None),
            collapsed_at: None,
            restarting: None,
        }
    }

    /// Whether the command is seen large.
    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Shows the command large, in a modal over the window: what is already written is drawn as
    /// it is, the lines that follow unfold.
    pub fn expand(&mut self, cx: &mut Context<Self>) {
        let base = self
            .shown
            .as_ref()
            .map(|shown| self.first + shown.lines.len());
        self.modal = Feed::new(base);
        self.expanded = true;
        self.collapsed_at = None;
        cx.notify();
    }

    /// Closes the modal and leaves the drawer as it was.
    pub fn collapse(&mut self, cx: &mut Context<Self>) {
        if self.expanded {
            self.expanded = false;
            self.collapsed_at = Some(cx.background_executor().now());
            // The focus goes back to the drawer, so that Escape reaches it.
            self.focused = false;
            cx.notify();
        }
    }

    /// The command whose detail is open.
    pub fn run(&self) -> Option<&str> {
        self.run.as_deref()
    }

    pub fn shown(&self) -> Option<&CommandOutput> {
        self.shown.as_ref()
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn open(&mut self, run: &str, cx: &mut Context<Self>) {
        if self.run.as_deref() != Some(run) {
            self.shown = None;
            self.row = None;
            self.feed = Feed::new(None);
            self.modal = Feed::new(None);
            self.first = 0;
            self.restarting = None;
        }
        self.run = Some(run.to_owned());
        self.focused = false;
        cx.notify();
    }

    /// A restart was asked: Stop and Restart wait for the new run.
    pub fn restart_asked(&mut self, run: &str, cx: &mut Context<Self>) {
        self.restarting = Some(run.to_owned());
        cx.notify();
    }

    /// The restart of `run` was refused: the buttons are back, unless what waits is another
    /// restart.
    pub fn restart_refused(&mut self, run: &str, cx: &mut Context<Self>) {
        if self.restarting.as_deref() == Some(run) {
            self.restarting = None;
            cx.notify();
        }
    }

    /// Whether a restart waits for its new run: Stop and Restart are faded meanwhile.
    pub fn restarting(&self) -> bool {
        self.restarting.is_some()
    }

    /// The new run of a restart takes the open command's place: its row and its output stay
    /// drawn, the modal too, until the new run's own arrive and replace them in place.
    pub fn follow(&mut self, run: &str, cx: &mut Context<Self>) {
        if self.run.is_none() {
            self.open(run, cx);
            return;
        }
        self.run = Some(run.to_owned());
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.run = None;
        self.restarting = None;
        self.focused = false;
        self.collapse(cx);
        cx.notify();
    }

    /// What the daemon gave of the output; one of a command closed since is dropped.
    pub fn set(&mut self, output: CommandOutput, cx: &mut Context<Self>) {
        if self.run.as_deref() != Some(output.row.run.as_str())
            || self.shown.as_ref() == Some(&output)
        {
            return;
        }
        if self
            .shown
            .as_ref()
            .is_some_and(|before| before.row.run != output.row.run)
        {
            // The new run of a restart: its output replaces the old one's from its first line.
            self.shown = None;
            self.first = 0;
            self.feed = Feed::new(None);
            self.modal = Feed::new(None);
        }
        if self
            .row
            .as_ref()
            .is_some_and(|row| row.run != output.row.run)
        {
            self.row = None;
        }
        if let Some(before) = &self.shown {
            self.first += slid(&before.lines, &output.lines);
        }
        let end = self.first + output.lines.len();
        for feed in [&mut self.feed, &mut self.modal] {
            feed.base.get_or_insert(end);
            feed.scroll.scroll_to_bottom();
        }
        if self.restarting.as_ref() != Some(&output.row.run) {
            self.restarting = None;
        }
        self.shown = Some(output);
        cx.notify();
    }

    /// The owner's list of commands: the open one's row, which says where it runs.
    pub fn set_rows(&mut self, rows: &[CommandRow], cx: &mut Context<Self>) {
        let row = self
            .run
            .as_ref()
            .and_then(|run| rows.iter().find(|row| &row.run == run))
            .cloned();
        if row.is_some() && self.row != row {
            if self.restarting.as_ref() != row.as_ref().map(|row| &row.run) {
                self.restarting = None;
            }
            self.row = row;
            cx.notify();
        }
    }

    /// How the agents are named, and the title of the session they belong to.
    pub fn set_names(
        &mut self,
        names: HashMap<String, String>,
        session: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.names != names || self.session != session {
            self.names = names;
            self.session = session;
            cx.notify();
        }
    }

    /// The whole output shown, as the program wrote it without its colours.
    pub fn text(&self) -> String {
        self.shown
            .iter()
            .flat_map(|shown| shown.lines.iter())
            .map(|line| ansi::strip(line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What the drawer says of a command, one line each.
struct Fact {
    id: &'static str,
    key: &'static str,
    value: String,
}

/// A fact of the drawer: a label and its value, the value cut to `lines` lines when it has a
/// limit.
fn fact(
    prefix: &str,
    fact: &Fact,
    value: impl IntoElement,
    lines: Option<usize>,
    p: &Palette,
) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(space::LG)
        .text_size(px(12.5))
        .child(
            div()
                .w(px(96.))
                .flex_none()
                .text_color(p.faint)
                .child(fact.key),
        )
        .child(
            div()
                .id(ElementId::Name(format!("{prefix}-{}", fact.id).into()))
                .flex_1()
                .min_w_0()
                .mono(12.)
                .when_some(lines, |d, lines| d.line_clamp(lines).text_ellipsis())
                .aria_label(SharedString::from(fact.value.clone()))
                .child(value)
                .test_support(),
        )
        .into_any_element()
}

/// A fact of the modal, which has the room: the label and the whole value, side by side, several
/// to a line.
fn inline_fact(fact: &Fact, p: &Palette) -> AnyElement {
    div()
        .id(ElementId::Name(
            format!("run-modal-fact-{}", fact.id).into(),
        ))
        .flex()
        .gap(space::SM)
        .min_w_0()
        .max_w_full()
        .text_size(px(12.5))
        .aria_label(SharedString::from(fact.value.clone()))
        .child(div().flex_none().text_color(p.faint).child(fact.key))
        .child(
            div()
                .min_w_0()
                .mono(12.)
                .child(SharedString::from(fact.value.clone())),
        )
        .test_support()
        .into_any_element()
}

impl CommandPanel {
    /// The output in its box, with its title and Copy: `modal` draws the modal's, which has its
    /// own ids and its own scroll.
    fn output_box(
        &mut self,
        modal: bool,
        at: Instant,
        reduce: bool,
        p: &Palette,
        copy: AnyElement,
    ) -> gpui_kit::Div {
        let name = if modal {
            "command-modal"
        } else {
            "command-panel"
        };
        let (feed, shown, first) = (
            if modal {
                &mut self.modal
            } else {
                &mut self.feed
            },
            &self.shown,
            self.first,
        );
        let base = feed.base.unwrap_or(0);
        // The lines of `shown` before this index were there when it opened (or are gone).
        let opened = match shown {
            Some(shown) => base.saturating_sub(first).min(shown.lines.len()),
            None => 0,
        };
        if let Some(shown) = shown {
            feed.arrivals.sync(
                shown
                    .lines
                    .iter()
                    .enumerate()
                    .skip(opened)
                    .map(|(i, line)| ((first + i).to_string(), line.clone())),
                at,
                reduce,
            );
        }
        let arrived: Vec<AnyElement> = feed
            .arrivals
            .rows()
            .map(|row| {
                let i = row.key;
                Item::new(
                    ElementId::Name(format!("{name}-item-{i}").into()),
                    row.placement,
                    coloured_line(
                        ElementId::Name(format!("{name}-line-{i}").into()),
                        row.value,
                        p.mute,
                        p,
                    ),
                )
                .into_any_element()
            })
            .collect();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(div().text_size(px(12.5)).medium().child("Output"))
                    .child(
                        div()
                            .flex_1()
                            .mono(11.)
                            .text_color(p.faint)
                            .child("follows the end"),
                    )
                    .child(copy),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{name}-output").into()))
                    .flex_1()
                    .min_h_0()
                    .py(space::SM)
                    .px(space::MD)
                    .rounded(px(8.))
                    .bg(p.field)
                    .border_1()
                    .border_color(p.line)
                    .overflow_y_scroll()
                    .track_scroll(&feed.scroll)
                    .flex()
                    .flex_col()
                    .when_some(shown.as_ref(), |d, shown| {
                        d.when(shown.earlier, |d| {
                            d.child(
                                div()
                                    .mono(11.)
                                    .text_color(p.faint)
                                    .child("(earlier lines are left out)"),
                            )
                        })
                        .children(
                            shown
                                .lines
                                .iter()
                                .enumerate()
                                .take(opened)
                                .map(|(i, line)| {
                                    coloured_line(
                                        ElementId::Name(
                                            format!("{name}-line-{}", first + i).into(),
                                        ),
                                        line,
                                        p.mute,
                                        p,
                                    )
                                }),
                        )
                        .children(arrived)
                    })
                    .when(shown.as_ref().is_none_or(|s| s.lines.is_empty()), |d| {
                        d.child(div().mono(11.5).text_color(p.faint).child(match shown {
                            Some(_) => "(no output yet)",
                            None => "Reading…",
                        }))
                    })
                    .test_support(),
            )
    }
}

impl Render for CommandPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if self.run.is_some() && !self.focused && !self.expanded {
            self.focused = true;
            window.focus(&focus, cx);
        }
        let row = self
            .row
            .clone()
            .or_else(|| self.shown.as_ref().map(|shown| shown.row.clone()));
        let root = div()
            .id("run-drawer-body")
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col();
        let Some(row) = row else {
            return root.child(
                div()
                    .p(space::XL)
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child("Reading…"),
            );
        };
        let now = now_ms();
        let running = row.running();
        let (_, color, said) = mark_of(row.ending(), &p);
        // A step of a setup ends `done` or `failed`, not with an exit code.
        let said = match (row.step, row.ending()) {
            (true, Ending::Exited(0)) => "done".to_owned(),
            (true, Ending::Exited(_)) => "failed".to_owned(),
            _ => said,
        };
        let time = seconds(row.elapsed_ms(now));
        let kind = if row.service { "Service" } else { "Task" };
        let sub = match (&row.name, running) {
            _ if row.step && running => format!("Setup step · running for {time}"),
            _ if row.step => format!("Setup step · {said} after {time}"),
            (Some(task), true) => match row.readiness() {
                Some(ready) => format!("{kind} {task} · {ready} · {time}"),
                None => format!("{kind} {task} · running for {time}"),
            },
            (None, true) => format!("Running for {time}"),
            (Some(task), false) => format!("{kind} {task} · {said} after {time}"),
            (None, false) => format!("{} after {time}", capitalized(&said)),
        };
        let page = self.page.clone();
        let owner = self.page.clone();
        // Built from the address it had, so that it still shows while it folds away.
        let link = |id: &str, size: f32, window: &mut Window, cx: &mut App| {
            row.url
                .as_deref()
                .map(|url| {
                    let owner = owner.clone();
                    address_link(
                        ElementId::Name(id.to_owned().into()),
                        url,
                        size,
                        move |url, cx| owner.open_link(url, cx),
                        window,
                        cx,
                    )
                })
                .unwrap_or_else(|| div().into_any_element())
        };
        let linked = row.address().is_some();
        let drawer_link = presence(
            "run-drawer-link-presence",
            linked,
            link("run-drawer-link", 12., window, cx),
        );
        let modal_link = presence(
            "run-modal-link-presence",
            linked,
            link("run-modal-link", 12.5, window, cx),
        );
        // Stop and Restart, in the drawer's header and in the modal's: a step of a setup is the
        // daemon's, nobody runs it again from here.
        // They wait, faded, while a restart is on its way.
        let waiting = self.restarting.is_some();
        let panel = cx.entity().downgrade();
        let actions = |prefix: &str| {
            let restart = (!row.step).then(|| {
                let (page, run, panel) = (owner.clone(), row.run.clone(), panel.clone());
                Button::secondary(format!("{prefix}-restart"), "Restart")
                    .leading(Icon::Restart)
                    .disabled(waiting)
                    .on_click(move |_, _, cx| {
                        panel
                            .update(cx, |panel, cx| panel.restart_asked(&run, cx))
                            .ok();
                        page.restart(&run, cx)
                    })
            });
            let stop = (running && !row.step).then(|| {
                let (page, run) = (owner.clone(), row.run.clone());
                Button::secondary(format!("{prefix}-stop"), "Stop")
                    .leading(Icon::Stop)
                    .disabled(waiting)
                    .on_click(move |_, _, cx| page.stop(&run, cx))
            });
            (restart, stop)
        };
        let header = {
            let (restart, stop) = actions("run-drawer");
            let expand = {
                let panel = cx.entity().downgrade();
                Button::icon("run-drawer-expand", Icon::Expand, "Expand").on_click(
                    move |_, _, cx| {
                        panel.update(cx, |panel, cx| panel.expand(cx)).ok();
                    },
                )
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(space::MD)
                .pl(space::XL)
                .pr(space::MD)
                .py(space::MD)
                .border_b_1()
                .border_color(p.line)
                .child(run_mark(
                    "run-drawer-mark".into(),
                    "run-drawer-dot".into(),
                    &row,
                    None,
                    &p,
                    window,
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("run-drawer-line")
                                .mono(14.)
                                .medium()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .aria_label(SharedString::from(row.line.clone()))
                                .child(SharedString::from(row.line.clone()))
                                .test_support(),
                        )
                        .child(
                            div()
                                .id("run-drawer-sub")
                                .text_size(px(12.))
                                .text_color(if running { p.faint } else { color })
                                .aria_label(SharedString::from(sub.clone()))
                                .child(SharedString::from(sub.clone()))
                                .test_support(),
                        )
                        .child(drawer_link),
                )
                .children(restart)
                .children(stop)
                .child(expand)
                .child(
                    Button::icon("run-drawer-close", Icon::Close, "Close (Esc)")
                        .on_click(move |_, _, cx| page.close(cx)),
                )
        };
        let by = match row.agent.as_str() {
            _ if row.step => "the workspace's setup".to_owned(),
            "" => "you".to_owned(),
            agent => {
                let name = self
                    .names
                    .get(agent)
                    .cloned()
                    .unwrap_or_else(|| format!("agent {agent}"));
                match &self.session {
                    Some(session) => format!("{name}, in {session}"),
                    None => name,
                }
            }
        };
        let mut by_link = Some({
            let (page, agent) = (self.page.clone(), row.agent.clone());
            let link = div()
                .id("run-fact-by-link")
                .cursor_pointer()
                .child(SharedString::from(by.clone()))
                .on_click(move |_, window, cx| page.show_agent(&agent, window, cx));
            hoverable(link, move |d, t| {
                d.underline().text_color(mix(p.mute, p.text, t))
            })
        });
        let place = row.workspace.clone().unwrap_or_else(|| "main".to_owned());
        let when = match running {
            true => format!("{time} ago"),
            false => format!(
                "{said} · {} ago",
                seconds(now.saturating_sub(row.ended_at.unwrap_or(row.started_at)))
            ),
        };
        // What the drawer and the modal both say, after the command itself. Runs are the daemon's,
        // on this machine, until a remote one runs them (design §12).
        let mut facts: Vec<Fact> = Vec::new();
        if let Some(task) = &row.name {
            facts.push(Fact {
                id: "task",
                key: "Task",
                value: task.clone(),
            });
        }
        facts.push(Fact {
            id: "where",
            key: "Where",
            value: place,
        });
        facts.push(Fact {
            id: "by",
            key: "Started by",
            value: by.clone(),
        });
        facts.push(Fact {
            id: "since",
            key: if running { "Started" } else { "Ended" },
            value: when,
        });
        facts.push(Fact {
            id: "machine",
            key: "Machine",
            value: "this PC".to_owned(),
        });
        let command = Fact {
            id: "command",
            key: "Command",
            value: row.line.clone(),
        };
        let mut shown_facts: Vec<AnyElement> = vec![fact(
            "run-fact",
            &command,
            SharedString::from(row.line.clone()),
            None,
            &p,
        )];
        for entry in &facts {
            // Who started it can be a whole first message: two lines in the drawer, which has the
            // room of a column; the modal says it whole.
            let (lines, element) = match (entry.id, row.agent.is_empty()) {
                ("by", true) => (Some(2), SharedString::from(by.clone()).into_any_element()),
                ("by", false) => (
                    Some(2),
                    by_link
                        .take()
                        .expect("the facts hold one \"by\"")
                        .into_any_element(),
                ),
                _ => (
                    None,
                    SharedString::from(entry.value.clone()).into_any_element(),
                ),
            };
            shown_facts.push(fact("run-fact", entry, element, lines, &p));
        }
        let facts_view = div()
            .id("run-drawer-facts")
            .flex_none()
            .flex()
            .flex_col()
            .gap(space::SM)
            .px(space::XL)
            .py(space::MD)
            .border_b_1()
            .border_color(p.line)
            .children(shown_facts);
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let written = self.text();
        let copy = |id: &'static str| {
            let text = written.clone();
            Button::quiet(id, "Copy")
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone()));
                    toast(
                        Toast::success("Output copied").detail("What the drawer shows of it."),
                        cx,
                    );
                })
                .into_any_element()
        };
        let output = self
            .output_box(false, at, reduce, &p, copy("run-drawer-copy"))
            .pl(space::XL)
            .pr(space::MD)
            .pt(space::SM)
            .pb(space::LG);
        // The modal draws its content while it is open and while it fades out.
        let showing = self.expanded
            || self
                .collapsed_at
                .is_some_and(|at_close| at.saturating_duration_since(at_close) < MODAL * 2);
        let modal = {
            let panel = cx.entity().downgrade();
            let mut modal = Modal::new("run-modal", SharedString::from(row.line.clone()))
                .sentence(sub)
                .under_title(Some(modal_link))
                .large()
                .takes_focus()
                .open(self.expanded)
                .on_close(move |_, cx| {
                    panel.update(cx, |panel, cx| panel.collapse(cx)).ok();
                });
            if showing {
                let (restart, stop) = actions("run-modal");
                let output = self.output_box(true, at, reduce, &p, copy("run-modal-copy"));
                for button in [restart, stop].into_iter().flatten() {
                    modal = modal.action(button);
                }
                modal = modal
                    .child(
                        div()
                            .id("run-modal-facts")
                            .flex_none()
                            .flex()
                            .flex_wrap()
                            .gap_x(space::XL)
                            .gap_y(space::XS)
                            .children(facts.iter().map(|entry| inline_fact(entry, &p))),
                    )
                    .child(output);
            }
            modal
        };
        root.child(header)
            .child(facts_view)
            .child(output)
            .child(modal)
    }
}

/// A sentence's first letter in capital.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(range: std::ops::RangeInclusive<u32>) -> Vec<String> {
        range.map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn a_window_of_lines_that_only_grows_has_not_slid() {
        assert_eq!(slid(&lines(1..=3), &lines(1..=5)), 0);
        assert_eq!(slid(&lines(1..=3), &lines(1..=3)), 0);
    }

    #[test]
    fn a_window_that_dropped_its_first_lines_says_by_how_many() {
        assert_eq!(slid(&lines(1..=5), &lines(3..=8)), 2);
        assert_eq!(slid(&lines(1..=5), &lines(6..=10)), 5);
    }

    #[test]
    fn a_window_with_nothing_in_common_slid_past_all_of_what_it_had() {
        assert_eq!(slid(&lines(1..=3), &lines(50..=52)), 3);
    }

    fn row(run: &str, state: &str, started_at: u64, ended_at: Option<u64>) -> CommandRow {
        CommandRow {
            run: run.into(),
            agent: "a".into(),
            name: None,
            workspace: None,
            line: run.into(),
            state: state.into(),
            exit_code: Some(0),
            signal: None,
            started_at,
            ended_at,
            last_line: String::new(),
            tail: Vec::new(),
            step: false,
            ready: None,
            url: None,
            service: false,
        }
    }

    #[test]
    fn the_root_agent_is_not_named_on_its_own_commands() {
        let names = HashMap::from([
            ("a".to_owned(), crate::session::THE_CONVERSATION.to_owned()),
            ("b".to_owned(), "build the form".to_owned()),
        ]);
        let by = |agent: &str, task: Option<&str>| {
            let mut run = row("r", "exited", 1, Some(2));
            run.agent = agent.into();
            run.name = task.map(str::to_owned);
            who(&run, &names)
        };
        assert_eq!(by("a", None), None, "the root says nothing of itself");
        assert_eq!(by("a", Some("build")).as_deref(), Some("task build"));
        assert_eq!(by("b", None).as_deref(), Some("build the form"));
        assert_eq!(
            by("b", Some("build")).as_deref(),
            Some("task build · build the form")
        );
        assert_eq!(by("", None).as_deref(), Some("you"));
    }

    /// The rows as runs of a task: a one-off line has no name.
    fn tasks(rows: Vec<CommandRow>) -> Vec<CommandRow> {
        rows.into_iter()
            .map(|mut run| {
                run.name = Some("build".into());
                run
            })
            .collect()
    }

    #[test]
    fn a_one_off_line_is_never_arranged_and_another_agent_s_run_only_when_none_is_asked() {
        let mut rows = tasks(vec![
            row("mine", "running", 1, None),
            row("theirs", "running", 2, None),
            row("done", "exited", 3, Some(4)),
        ]);
        rows[1].agent = "b".into();
        let mut lines = vec![
            row("line", "running", 5, None),
            row("old", "exited", 6, Some(7)),
        ];
        rows.append(&mut lines);

        let all = arrange_of(&rows, None);
        assert_eq!(runs(&all.running), ["mine", "theirs"]);
        assert_eq!(runs(&all.kept), ["done"]);
        let mine = arrange_of(&rows, Some("a"));
        assert_eq!(runs(&mine.running), ["mine"]);
        assert_eq!(runs(&mine.kept), ["done"]);
        assert_eq!(runs(&arrange_of(&rows, Some("c")).running), [] as [&str; 0]);
    }

    fn runs(rows: &[&CommandRow]) -> Vec<String> {
        rows.iter().map(|row| row.run.clone()).collect()
    }

    #[test]
    fn running_come_first_then_the_last_five_finished_the_latest_first() {
        let rows = tasks(vec![
            row("e1", "exited", 10, Some(20)),
            row("r1", "running", 15, None),
            row("e2", "exited", 11, Some(30)),
            row("e3", "exited", 12, Some(40)),
            row("e4", "exited", 13, Some(50)),
            row("r2", "running", 16, None),
            row("e5", "stopped", 14, Some(60)),
            row("e6", "exited", 15, Some(70)),
        ]);
        let arranged = arrange(&rows);
        assert_eq!(runs(&arranged.running), ["r1", "r2"]);
        assert_eq!(
            runs(&arranged.kept),
            ["e6", "e5", "e4", "e3", "e2"],
            "the latest first, the sixth left out"
        );
    }
}
