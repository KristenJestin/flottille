//! The commands the agents run, as the window shows them: the rail's Runs section (every command
//! of the session's tree, running ones first, the last ended ones in sight and the older ones
//! behind one folded row), the panel that follows one's output live, and the one compact line a
//! command's result is in the conversation.
//!
//! What a program wrote is drawn as the daemon gave it (masked): its colours (`ESC [ … m`) turned
//! into the palette's, every other escape sequence dropped (`flottille_core::ansi`). The stored
//! output is untouched.

use std::collections::HashMap;
use std::time::Duration;

use flottille_core::ansi;
use flottille_ui::color::mix;
use flottille_ui::{
    AgentState, Button, Icon, Item, Listing, Palette, StateIcon, Status, Toast, Type, hoverable,
    icon, palette, presence, section_label, space, toast,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Entity, FocusHandle, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Task, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::conversation::{CommandCard, Ending};
use crate::session::{SessionPage, seconds};
use crate::shell::now_ms;
use crate::tasks::ProjectTasks;

/// Where a list of commands and its panel are: a session's rail, or a project page, which lists
/// the tasks the user ran there.
#[derive(Clone)]
pub enum Owner {
    Session(WeakEntity<SessionPage>),
    Project(WeakEntity<ProjectTasks>),
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
    fn open(&self, run: &str, cx: &mut App) {
        match self {
            Owner::Session(page) => page.update(cx, |page, cx| page.open_command(run, cx)).ok(),
            Owner::Project(tasks) => tasks
                .update(cx, |tasks, cx| tasks.open_command(run, cx))
                .ok(),
        };
    }

    fn close(&self, cx: &mut App) {
        match self {
            Owner::Session(page) => page.update(cx, |page, cx| page.close_command(cx)).ok(),
            Owner::Project(tasks) => tasks.update(cx, |tasks, cx| tasks.close_command(cx)).ok(),
        };
    }

    fn stop(&self, run: &str, cx: &mut App) {
        match self {
            Owner::Session(page) => page.update(cx, |page, cx| page.stop_command(run, cx)).ok(),
            Owner::Project(tasks) => tasks
                .update(cx, |tasks, cx| tasks.stop_command(run, cx))
                .ok(),
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
        }
    }

    pub fn running(&self) -> bool {
        self.state == "running"
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

/// What a command's ending is drawn with: a glyph, its ink and its words. The shape says it as
/// well as the colour.
pub fn mark_of(ending: Ending, p: &Palette) -> (&'static str, Hsla, String) {
    let (glyph, ink) = match ending {
        Ending::Running => ("●", p.run),
        Ending::Exited(0) => ("✓", p.ok),
        Ending::Exited(_) | Ending::Killed(_) => ("✗", p.fail),
        Ending::Stopped => ("■", p.faint),
        Ending::Interrupted => ("–", p.faint),
    };
    (glyph, ink, ending.said())
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
    let (glyph, color, said) = mark_of(card.ending, &p);
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
                .mono(11.5)
                .text_color(color)
                .aria_label(SharedString::from(said.clone()))
                .child(SharedString::from(format!("{glyph} {said}")))
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

/// Sorts the rows of the section: the running ones, then the last ones to have finished.
pub fn arrange(rows: &[CommandRow]) -> Arranged<'_> {
    let running = rows.iter().filter(|row| row.running()).collect();
    let mut ended: Vec<&CommandRow> = rows.iter().filter(|row| !row.running()).collect();
    let ended_at = |row: &CommandRow| row.ended_at.unwrap_or(row.started_at);
    ended.sort_by_key(|row| std::cmp::Reverse((ended_at(row), row.started_at)));
    ended.truncate(KEPT_ENDED);
    Arranged {
        running,
        kept: ended,
    }
}

/// Who started a command: the task, then the agent as `names` calls it, or the user.
fn who(row: &CommandRow, names: &HashMap<String, String>) -> String {
    let agent = match row.agent.as_str() {
        "" => "you".to_owned(),
        agent => names
            .get(agent)
            .cloned()
            .unwrap_or_else(|| format!("agent {agent}")),
    };
    match &row.name {
        Some(name) => format!("task {name} · {agent}"),
        None => agent,
    }
}

/// A command's mark: a spinner while it runs, else the glyph of how it ended, in a 16 px box.
fn run_mark(
    id: ElementId,
    row: &CommandRow,
    spinner: Option<Entity<StateIcon>>,
    p: &Palette,
) -> AnyElement {
    let (glyph, color, said) = mark_of(row.ending(), p);
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
            None => d.mono(12.).text_color(color).child(glyph),
        })
        .test_support()
        .into_any_element()
}

/// The rail's Runs section: nothing while the session ran no command.
pub struct CommandList {
    page: Owner,
    rows: Vec<CommandRow>,
    /// The rows as drawn: one that goes folds away before it is gone.
    list: Listing<CommandRow>,
    /// How the tree names each agent.
    names: HashMap<String, String>,
    /// The spinner of each running command.
    spinners: HashMap<String, Entity<StateIcon>>,
    error: Option<SharedString>,
    /// Draws the time again each second, while a command runs.
    ticker: Option<Task<()>>,
    renders: usize,
}

impl CommandList {
    pub fn new(page: impl Into<Owner>) -> Self {
        Self {
            page: page.into(),
            rows: Vec::new(),
            list: Listing::default(),
            names: HashMap::new(),
            spinners: HashMap::new(),
            error: None,
            ticker: None,
            renders: 0,
        }
    }

    pub fn rows(&self) -> &[CommandRow] {
        &self.rows
    }

    /// The rows listed, as the section sorts them now.
    pub fn arranged(&self) -> Arranged<'_> {
        arrange(&self.rows)
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// What the daemon listed; nothing is drawn again when it is what was drawn.
    pub fn set(&mut self, rows: Vec<CommandRow>, cx: &mut Context<Self>) {
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
    fn run_row(&self, row: &CommandRow, now: u64, p: Palette) -> AnyElement {
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
        let who = who(row, &self.names);
        let time = seconds(row.elapsed_ms(now));
        let line = |id: ElementId, text: String| {
            div()
                .id(id)
                .mono(11.)
                .text_color(p.faint)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .aria_label(SharedString::from(text.clone()))
                .child(SharedString::from(text))
                .test_support()
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
                row,
                self.spinners.get(&row.run).cloned(),
                &p,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(id("line"))
                            .mono(12.5)
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
                            .child(
                                div()
                                    .id(id("who"))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .aria_label(SharedString::from(who.clone()))
                                    .child(SharedString::from(who))
                                    .test_support(),
                            )
                            .child("·")
                            .child(
                                div()
                                    .id(id("time"))
                                    .flex_none()
                                    .child(SharedString::from(time))
                                    .test_support(),
                            ),
                    )
                    // A run that wrote nothing has no line to show, not an empty one.
                    .when(!last.is_empty(), |d| d.child(line(id("last"), last))),
            )
            .children(stop)
            .opacity(if ended { 0.75 } else { 1.0 });
        hoverable(card, move |d, t| d.bg(p.hover.opacity(t))).into_any_element()
    }
}

impl Render for CommandList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let arranged = arrange(&self.rows);
        let entries: Vec<(String, CommandRow)> = arranged
            .running
            .iter()
            .chain(&arranged.kept)
            .map(|row| (row.run.clone(), (*row).clone()))
            .collect();
        let at = cx.background_executor().now();
        self.list.sync(entries, at, cx.reduce_motion());
        let p = palette(cx);
        let now = now_ms();
        let present = !self.list.is_empty();
        let listed: Vec<(String, CommandRow, flottille_ui::Placement)> = self
            .list
            .rows()
            .map(|listed| {
                (
                    listed.key.to_owned(),
                    listed.value.clone(),
                    listed.placement,
                )
            })
            .collect();
        let rows: Vec<AnyElement> = listed
            .into_iter()
            .map(|(key, row, placement)| {
                let content = self.run_row(&row, now, p);
                Item::new(
                    ElementId::Name(format!("command-item-{key}").into()),
                    placement,
                    content,
                )
                .into_any_element()
            })
            .collect();
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
            .children(rows)
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
            .test_support();
        presence("rail-runs-presence", present, section)
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
    scroll: ScrollHandle,
    /// Where the focus rests while it is open, so that Escape reaches the drawer.
    focus: Option<FocusHandle>,
    focused: bool,
    renders: usize,
    /// How many lines the output had when it was first read: they are there as the drawer
    /// opens, the ones that follow unfold as they arrive.
    base: Option<usize>,
    arrivals: Listing<String>,
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
            scroll: ScrollHandle::new(),
            focus: None,
            focused: false,
            renders: 0,
            base: None,
            arrivals: Listing::default(),
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
            self.base = None;
            self.arrivals = Listing::default();
        }
        self.run = Some(run.to_owned());
        self.focused = false;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.run = None;
        self.focused = false;
        cx.notify();
    }

    /// What the daemon gave of the output; one of a command closed since is dropped.
    pub fn set(&mut self, output: CommandOutput, cx: &mut Context<Self>) {
        if self.run.as_deref() != Some(output.row.run.as_str())
            || self.shown.as_ref() == Some(&output)
        {
            return;
        }
        if self.base.is_none() {
            self.base = Some(output.lines.len());
        }
        self.shown = Some(output);
        self.scroll.scroll_to_bottom();
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

/// A label and its value, on one line of the drawer's facts.
fn fact(id: &str, key: &str, value: impl IntoElement, said: String, p: &Palette) -> AnyElement {
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
                .child(key.to_owned()),
        )
        .child(
            div()
                .id(ElementId::Name(format!("run-fact-{id}").into()))
                .flex_1()
                .min_w_0()
                .mono(12.)
                .aria_label(SharedString::from(said))
                .child(value)
                .test_support(),
        )
        .into_any_element()
}

impl Render for CommandPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if self.run.is_some() && !self.focused {
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
        let sub = match (&row.name, running) {
            _ if row.step && running => format!("Setup step · running for {time}"),
            _ if row.step => format!("Setup step · {said} after {time}"),
            (Some(task), true) => format!("Task {task} · running for {time}"),
            (None, true) => format!("Running for {time}"),
            (Some(task), false) => format!("Task {task} · {said} after {time}"),
            (None, false) => format!("{} after {time}", capitalized(&said)),
        };
        let page = self.page.clone();
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(space::MD)
            .pl(space::XL)
            .pr(space::MD)
            .py(space::MD)
            .border_b_1()
            .border_color(p.line)
            .child(run_mark("run-drawer-mark".into(), &row, None, &p))
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
                            .child(SharedString::from(sub))
                            .test_support(),
                    ),
            )
            .when(running && !row.step, |d| {
                let (page, run) = (self.page.clone(), row.run.clone());
                d.child(
                    Button::secondary("run-drawer-stop", "Stop")
                        .leading(Icon::Stop)
                        .on_click(move |_, _, cx| page.stop(&run, cx)),
                )
            })
            .child(
                Button::icon("run-drawer-close", Icon::Close, "Close (Esc)")
                    .on_click(move |_, _, cx| page.close(cx)),
            );
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
        let by_link = {
            let (page, agent) = (self.page.clone(), row.agent.clone());
            let link = div()
                .id("run-fact-by-link")
                .cursor_pointer()
                .child(SharedString::from(by.clone()))
                .on_click(move |_, window, cx| page.show_agent(&agent, window, cx));
            hoverable(link, move |d, t| {
                d.underline().text_color(mix(p.mute, p.text, t))
            })
        };
        let place = row.workspace.clone().unwrap_or_else(|| "main".to_owned());
        let when = match running {
            true => format!("{time} ago"),
            false => format!(
                "{said} · {} ago",
                seconds(now.saturating_sub(row.ended_at.unwrap_or(row.started_at)))
            ),
        };
        let mut facts: Vec<AnyElement> = vec![fact(
            "command",
            "Command",
            SharedString::from(row.line.clone()),
            row.line.clone(),
            &p,
        )];
        if let Some(task) = &row.name {
            facts.push(fact(
                "task",
                "Task",
                SharedString::from(task.clone()),
                task.clone(),
                &p,
            ));
        }
        facts.push(fact(
            "where",
            "Where",
            SharedString::from(place.clone()),
            place,
            &p,
        ));
        facts.push(match row.agent.is_empty() {
            true => fact("by", "Started by", SharedString::from(by.clone()), by, &p),
            false => fact("by", "Started by", by_link, by, &p),
        });
        facts.push(fact(
            "since",
            if running { "Started" } else { "Ended" },
            SharedString::from(when.clone()),
            when,
            &p,
        ));
        // Runs are the daemon's, on this machine, until a remote one runs them (design §12).
        facts.push(fact(
            "machine",
            "Machine",
            SharedString::from("this PC"),
            "this PC".to_owned(),
            &p,
        ));
        let facts = div()
            .id("run-drawer-facts")
            .flex_none()
            .flex()
            .flex_col()
            .gap(space::SM)
            .px(space::XL)
            .py(space::MD)
            .border_b_1()
            .border_color(p.line)
            .children(facts);
        let copy = {
            let text = self.text();
            Button::quiet("run-drawer-copy", "Copy").on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone()));
                toast(
                    Toast::success("Output copied").detail("What the drawer shows of it."),
                    cx,
                );
            })
        };
        // The lines there when it was first read are drawn as they are; each one that follows
        // unfolds as it arrives.
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let base = match (&self.shown, self.base) {
            (Some(shown), Some(base)) => base.min(shown.lines.len()),
            _ => 0,
        };
        if let Some(shown) = &self.shown {
            self.arrivals.sync(
                shown
                    .lines
                    .iter()
                    .enumerate()
                    .skip(base)
                    .map(|(i, line)| (i.to_string(), line.clone())),
                at,
                reduce,
            );
        }
        let arrived: Vec<AnyElement> = self
            .arrivals
            .rows()
            .map(|row| {
                let i = row.key;
                Item::new(
                    ElementId::Name(format!("command-panel-item-{i}").into()),
                    row.placement,
                    coloured_line(
                        ElementId::Name(format!("command-panel-line-{i}").into()),
                        row.value,
                        p.mute,
                        &p,
                    ),
                )
                .into_any_element()
            })
            .collect();
        let output =
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(space::SM)
                .pl(space::XL)
                .pr(space::MD)
                .pt(space::SM)
                .pb(space::LG)
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
                        .id("command-panel-output")
                        .flex_1()
                        .min_h_0()
                        .py(space::SM)
                        .px(space::MD)
                        .rounded(px(8.))
                        .bg(p.field)
                        .border_1()
                        .border_color(p.line)
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .flex()
                        .flex_col()
                        .when_some(self.shown.as_ref(), |d, shown| {
                            d.when(shown.earlier, |d| {
                                d.child(
                                    div()
                                        .mono(11.)
                                        .text_color(p.faint)
                                        .child("(earlier lines are left out)"),
                                )
                            })
                            .children(shown.lines.iter().enumerate().take(base).map(|(i, line)| {
                                coloured_line(
                                    ElementId::Name(format!("command-panel-line-{i}").into()),
                                    line,
                                    p.mute,
                                    &p,
                                )
                            }))
                            .children(arrived)
                        })
                        .when(
                            self.shown.as_ref().is_none_or(|s| s.lines.is_empty()),
                            |d| {
                                d.child(div().mono(11.5).text_color(p.faint).child(
                                    match self.shown {
                                        Some(_) => "(no output yet)",
                                        None => "Reading…",
                                    },
                                ))
                            },
                        )
                        .test_support(),
                );
        root.child(header).child(facts).child(output)
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
        }
    }

    fn runs(rows: &[&CommandRow]) -> Vec<String> {
        rows.iter().map(|row| row.run.clone()).collect()
    }

    #[test]
    fn running_come_first_then_the_last_five_finished_the_latest_first() {
        let rows = vec![
            row("e1", "exited", 10, Some(20)),
            row("r1", "running", 15, None),
            row("e2", "exited", 11, Some(30)),
            row("e3", "exited", 12, Some(40)),
            row("e4", "exited", 13, Some(50)),
            row("r2", "running", 16, None),
            row("e5", "stopped", 14, Some(60)),
            row("e6", "exited", 15, Some(70)),
        ];
        let arranged = arrange(&rows);
        assert_eq!(runs(&arranged.running), ["r1", "r2"]);
        assert_eq!(
            runs(&arranged.kept),
            ["e6", "e5", "e4", "e3", "e2"],
            "the latest first, the sixth left out"
        );
    }
}
