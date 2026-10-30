//! The commands the agents run, as the window shows them: the rail's Runs section (every command
//! of the session's tree, running ones first, the last ended ones in sight and the older ones
//! behind one folded row), the panel that follows one's output live, and the one compact line a
//! command's result is in the conversation.
//!
//! What a program wrote is drawn as the daemon gave it (masked): its colours (`ESC [ … m`) turned
//! into the palette's, every other escape sequence dropped (`flottille_core::ansi`). The stored
//! output is untouched.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::time::Duration;

use flottille_core::ansi;
use flottille_ui::color::mix;
use flottille_ui::drawer::Closed;
use flottille_ui::motion::PANEL;
use flottille_ui::{
    AgentState, Button, Drawer, Icon, Item, Listing, Palette, StateIcon, Status, Toast, Type, fade,
    hoverable, icon, palette, presence, section_label, space, toast, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, ElementId, Entity, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, Transformation,
    WeakEntity, Window, div, px, radians,
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

    fn dismiss(&self, run: &str, cx: &mut App) {
        match self {
            Owner::Session(page) => page
                .update(cx, |page, cx| page.dismiss_command(run, cx))
                .ok(),
            Owner::Project(tasks) => tasks
                .update(cx, |tasks, cx| tasks.dismiss_command(run, cx))
                .ok(),
        };
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
}

impl CommandRow {
    pub fn of(value: &Value) -> CommandRow {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        CommandRow {
            run: text("run"),
            agent: text("agent"),
            name: value["name"].as_str().map(str::to_owned),
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

/// The ended runs that stay in sight under the running ones; the others are behind "N ended".
pub const KEPT_ENDED: usize = 3;

/// The rail's runs, sorted: what is in sight, and what is behind the "N ended" row.
#[derive(Debug, PartialEq)]
pub struct Arranged<'a> {
    /// Running first, in the order the daemon lists them.
    pub running: Vec<&'a CommandRow>,
    /// Ended and in sight: the latest ones, [`KEPT_ENDED`] at most, and none that ended before
    /// the session's last turn began.
    pub kept: Vec<&'a CommandRow>,
    /// Ended and folded away, the latest first.
    pub folded: Vec<&'a CommandRow>,
}

/// Sorts the rows of the section. `turn_at` is when the session's last turn began, when known.
pub fn arrange(rows: &[CommandRow], turn_at: Option<u64>) -> Arranged<'_> {
    arrange_keeping(rows, turn_at, KEPT_ENDED)
}

/// [`arrange`], keeping `kept` ended runs in sight.
pub fn arrange_keeping(rows: &[CommandRow], turn_at: Option<u64>, kept: usize) -> Arranged<'_> {
    let kept_at_most = kept;
    let running = rows.iter().filter(|row| row.running()).collect();
    let mut ended: Vec<&CommandRow> = rows.iter().filter(|row| !row.running()).collect();
    let ended_at = |row: &CommandRow| row.ended_at.unwrap_or(row.started_at);
    ended.sort_by_key(|row| std::cmp::Reverse((ended_at(row), row.started_at)));
    let (mut kept, mut folded) = (Vec::new(), Vec::new());
    for row in ended {
        if kept.len() < kept_at_most && turn_at.is_none_or(|turn| ended_at(row) >= turn) {
            kept.push(row);
        } else {
            folded.push(row);
        }
    }
    Arranged {
        running,
        kept,
        folded,
    }
}

/// What the section lists: a run, or the row that folds the older ended ones.
#[derive(Clone)]
enum Entry {
    Run(CommandRow),
    Ended { count: usize },
}

/// A small text button on a row (Stop, Dismiss): its ink and fill fade with the pointer.
fn text_action(
    id: ElementId,
    label: &'static str,
    p: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let button = div()
        .id(id)
        .flex_none()
        .px(space::SM)
        .py(space::XS)
        .rounded(px(5.))
        .text_size(px(11.5))
        .cursor_pointer()
        .aria_label(label)
        .child(label)
        .on_click(on_click);
    hoverable(button, move |d, t| {
        d.bg(p.hover.opacity(t)).text_color(mix(p.faint, p.text, t))
    })
}

/// The rail's Runs section: nothing while the session ran no command.
pub struct CommandList {
    page: Owner,
    rows: Vec<CommandRow>,
    /// The rows as drawn: one that was dismissed, or that folded behind "N ended", folds away
    /// before it is gone.
    list: Listing<Entry>,
    /// How the tree names each agent.
    names: HashMap<String, String>,
    /// When the session's last turn began: an ended run from before it folds away on its own.
    turn_at: Option<u64>,
    /// The "N ended" row is open: the runs behind it are listed.
    open_ended: bool,
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
            turn_at: None,
            open_ended: false,
            error: None,
            ticker: None,
            renders: 0,
        }
    }

    pub fn rows(&self) -> &[CommandRow] {
        &self.rows
    }

    /// The rows in sight and the ones folded behind "N ended", as the section sorts them now.
    pub fn arranged(&self) -> Arranged<'_> {
        arrange(&self.rows, self.turn_at)
    }

    /// Whether the "N ended" row is open.
    pub fn is_ended_open(&self) -> bool {
        self.open_ended
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

    /// When the session's last turn began (milliseconds since the epoch).
    pub fn set_turn(&mut self, at: Option<u64>, cx: &mut Context<Self>) {
        if self.turn_at != at {
            self.turn_at = at;
            cx.notify();
        }
    }

    /// Opens or folds the "N ended" row.
    pub fn toggle_ended(&mut self, cx: &mut Context<Self>) {
        self.open_ended = !self.open_ended;
        cx.notify();
    }

    /// The daemon refused what was asked of a command, and why; none clears it.
    pub fn failed(&mut self, why: Option<String>, cx: &mut Context<Self>) {
        self.error = why.map(Into::into);
        cx.notify();
    }

    /// Who started a command: the task, then the agent, or the user.
    fn who(&self, row: &CommandRow) -> String {
        let agent = match row.agent.as_str() {
            "" => "you".to_owned(),
            agent => self
                .names
                .get(agent)
                .cloned()
                .unwrap_or_else(|| format!("agent {agent}")),
        };
        match &row.name {
            Some(name) => format!("task {name} · {agent}"),
            None => agent,
        }
    }

    /// One run: its line and how long, who started it and how it ended, the last line of its
    /// output, and Stop or Dismiss.
    fn run_row(
        &self,
        row: &CommandRow,
        now: u64,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ended = !row.running();
        let (glyph, color, said) = mark_of(row.ending(), &p);
        let run = row.run.clone();
        let id = |part: &str| ElementId::Name(format!("command-{part}-{run}").into());
        let open = {
            let page = self.page.clone();
            let run = run.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| page.open(&run, cx)
        };
        // The row opens the output; its buttons do not.
        let action = {
            let (page, run) = (self.page.clone(), run.clone());
            match ended {
                true => text_action(id("dismiss"), "Dismiss", p, move |_, _, cx| {
                    cx.stop_propagation();
                    page.dismiss(&run, cx);
                })
                .into_any_element(),
                false => text_action(id("stop"), "Stop", p, move |_, _, cx| {
                    cx.stop_propagation();
                    page.stop(&run, cx);
                })
                .into_any_element(),
            }
        };
        // A run that ended greys over PANEL; one first drawn ended is simply grey.
        let grey = fade(id("grey"), ended, PANEL, window, cx);
        let last = ansi::strip(&row.last_line);
        let who = self.who(row);
        let card = div()
            .id(ElementId::Name(format!("command-{}", row.run).into()))
            .flex()
            .flex_col()
            .gap(space::XS)
            .px(space::SM)
            .py(space::SM)
            .rounded(px(8.))
            .cursor_pointer()
            .opacity(1.0 - 0.4 * grey)
            .on_click(open)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(
                        div()
                            .id(id("mark"))
                            .flex_none()
                            .mono(12.)
                            .text_color(color)
                            .aria_label(SharedString::from(said.clone()))
                            .child(glyph)
                            .test_support(),
                    )
                    .child(
                        div()
                            .id(id("line"))
                            .flex_1()
                            .min_w_0()
                            .mono(12.)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .aria_label(SharedString::from(row.line.clone()))
                            .child(SharedString::from(row.line.clone()))
                            .test_support(),
                    )
                    .child(
                        div()
                            .id(id("time"))
                            .flex_none()
                            .mono(11.)
                            .text_color(p.faint)
                            .child(SharedString::from(seconds(row.elapsed_ms(now))))
                            .test_support(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .pl(space::XL)
                    .when(ended, |d| {
                        d.child(
                            div()
                                .id(id("status"))
                                .flex_none()
                                .mono(11.)
                                .text_color(color)
                                .aria_label(SharedString::from(said.clone()))
                                .child(SharedString::from(said.clone()))
                                .test_support(),
                        )
                    })
                    .child(
                        div()
                            .id(id("who"))
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.5))
                            .text_color(p.faint)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .aria_label(SharedString::from(who.clone()))
                            .child(SharedString::from(who))
                            .test_support(),
                    )
                    .child(action),
            )
            // A run that wrote nothing has no line to show, not an empty one.
            .when(!last.is_empty(), |d| {
                d.child(
                    div()
                        .id(id("last"))
                        .pl(space::XL)
                        .mono(11.)
                        .text_color(p.faint)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .aria_label(SharedString::from(last.clone()))
                        .child(SharedString::from(last))
                        .test_support(),
                )
            });
        hoverable(card, move |d, t| d.bg(p.hover.opacity(t))).into_any_element()
    }

    /// The row that folds the older ended runs: "3 ended", a chevron that turns as it opens.
    fn ended_row(
        &self,
        count: usize,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = ElementId::from("rail-runs-ended");
        let turn = turning(id.clone(), self.open_ended, window, cx);
        let said = SharedString::from(format!("{count} ended"));
        let row = div()
            .id(id)
            .aria_expanded(self.open_ended)
            .aria_label(said.clone())
            .flex()
            .items_center()
            .gap(space::SM)
            .px(space::SM)
            .py(space::SM)
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(12.5))
            .child(
                div().w(px(14.)).flex_none().flex().justify_center().child(
                    icon(Icon::Chevron)
                        .size(px(12.))
                        .text_color(p.faint)
                        .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * turn))),
                ),
            )
            .child(said)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_ended(cx)));
        hoverable(row, move |d, t| {
            d.bg(p.hover.opacity(t)).text_color(mix(p.faint, p.text, t))
        })
        .into_any_element()
    }
}

impl Render for CommandList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let arranged = arrange(&self.rows, self.turn_at);
        if arranged.folded.is_empty() {
            self.open_ended = false;
        }
        let mut entries: Vec<(String, Entry)> = arranged
            .running
            .iter()
            .chain(&arranged.kept)
            .map(|row| (row.run.clone(), Entry::Run((*row).clone())))
            .collect();
        if !arranged.folded.is_empty() {
            let count = arranged.folded.len();
            entries.push(("ended".to_owned(), Entry::Ended { count }));
            if self.open_ended {
                entries.extend(
                    arranged
                        .folded
                        .iter()
                        .map(|row| (row.run.clone(), Entry::Run((*row).clone()))),
                );
            }
        }
        let at = cx.background_executor().now();
        self.list.sync(entries, at, cx.reduce_motion());
        let p = palette(cx);
        let now = now_ms();
        let present = !self.list.is_empty();
        let listed: Vec<(String, Entry, flottille_ui::Placement)> = self
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
            .map(|(key, entry, placement)| {
                let (id, content) = match entry {
                    Entry::Run(row) => (
                        format!("command-item-{key}"),
                        self.run_row(&row, now, p, window, cx),
                    ),
                    Entry::Ended { count } => (
                        "command-item-ended".to_owned(),
                        self.ended_row(count, p, window, cx),
                    ),
                };
                Item::new(ElementId::Name(id.into()), placement, content).into_any_element()
            })
            .collect();
        let section = div()
            .id("rail-runs")
            .flex()
            .flex_col()
            .child(div().h(px(1.)).bg(p.line).mx(px(6.)).my(px(16.)))
            .child(
                div()
                    .id("rail-runs-title")
                    .aria_label("Runs")
                    .child(section_label("Runs", cx))
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

/// A run's detail, which follows its output while it is open: a drawer from the right edge
/// (`run-drawer`), the same for a run of the rail, of the conversation and of the project page.
/// It sits over the page that renders it, which is a positioned box the drawer fills the height
/// of.
pub struct CommandPanel {
    page: Owner,
    /// The command whose output is open.
    run: Option<String>,
    shown: Option<CommandOutput>,
    detail: Entity<RunDetail>,
    drawer: Entity<Drawer>,
    /// The drawer takes the focus on the next frame, so that Escape closes it.
    focus: bool,
    renders: usize,
    _closed: Subscription,
}

impl CommandPanel {
    pub fn new(page: impl Into<Owner>, cx: &mut Context<Self>) -> Self {
        let page = page.into();
        let detail = {
            let page = page.clone();
            cx.new(|cx| RunDetail::new(page, cx))
        };
        let drawer = {
            let detail = detail.clone();
            cx.new(|cx| Drawer::detail("run-drawer", detail, cx))
        };
        // Escape or the close button: the owner closes the panel, once this update is over.
        let closed = cx.subscribe(&drawer, |this, _, _: &Closed, cx| {
            if this.run.is_some() {
                let page = this.page.clone();
                cx.defer(move |cx| page.close(cx));
            }
        });
        Self {
            page,
            run: None,
            shown: None,
            detail,
            drawer,
            focus: false,
            renders: 0,
            _closed: closed,
        }
    }

    /// The command whose output is open.
    pub fn run(&self) -> Option<&str> {
        self.run.as_deref()
    }

    pub fn shown(&self) -> Option<&CommandOutput> {
        self.shown.as_ref()
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn drawer(&self) -> &Entity<Drawer> {
        &self.drawer
    }

    pub fn open(&mut self, run: &str, cx: &mut Context<Self>) {
        if self.run.as_deref() != Some(run) {
            self.shown = None;
            self.detail.update(cx, |detail, cx| detail.set(None, cx));
        }
        self.run = Some(run.to_owned());
        self.focus = true;
        self.drawer.update(cx, |drawer, cx| drawer.open(cx));
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.run.take().is_none() {
            return;
        }
        // The output stays in the drawer while it slides away.
        self.focus = false;
        self.drawer.update(cx, |drawer, cx| drawer.close(cx));
        cx.notify();
    }

    /// What the daemon gave of the output; one of a command closed since is dropped.
    pub fn set(&mut self, output: CommandOutput, cx: &mut Context<Self>) {
        if self.run.as_deref() != Some(output.row.run.as_str())
            || self.shown.as_ref() == Some(&output)
        {
            return;
        }
        self.shown = Some(output.clone());
        self.detail
            .update(cx, |detail, cx| detail.set(Some(output), cx));
    }
}

impl Render for CommandPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        if std::mem::take(&mut self.focus) {
            let drawer = self.drawer.clone();
            cx.defer_in(window, move |_, window, cx| {
                drawer.update(cx, |drawer, cx| drawer.show(window, cx));
            });
        }
        self.drawer.clone()
    }
}

/// What the run drawer holds: the run's mark, its line and how it stands, Stop and Close; its
/// facts; then its output, followed to the end while it runs, and Copy.
pub struct RunDetail {
    page: Owner,
    shown: Option<CommandOutput>,
    status: Entity<Status>,
    mark: Entity<StateIcon>,
    scroll: ScrollHandle,
}

/// The state a run's mark says.
fn run_state(ending: Ending) -> AgentState {
    match ending {
        Ending::Running => AgentState::Running,
        Ending::Exited(0) => AgentState::Completed,
        Ending::Exited(_) | Ending::Killed(_) => AgentState::Failed,
        Ending::Stopped | Ending::Interrupted => AgentState::Stopped,
    }
}

impl RunDetail {
    fn new(page: Owner, cx: &mut Context<Self>) -> Self {
        let status = cx.new(|_| Status::new(AgentState::Running));
        let mark = {
            let status = status.clone();
            cx.new(|cx| StateIcon::new(status, cx))
        };
        Self {
            page,
            shown: None,
            status,
            mark,
            scroll: ScrollHandle::new(),
        }
    }

    fn set(&mut self, shown: Option<CommandOutput>, cx: &mut Context<Self>) {
        if let Some(shown) = &shown {
            let state = run_state(shown.row.ending());
            self.status.update(cx, |status, cx| status.set(state, cx));
            if shown.row.running() {
                self.scroll.scroll_to_bottom();
            }
        }
        self.shown = shown;
        cx.notify();
    }

    /// What it is and how it stands, under its line: "task say · ran 0.2s", "running for 4m".
    fn standing(row: &CommandRow, now: u64) -> String {
        let took = seconds(row.elapsed_ms(now));
        let how = match row.ending() {
            Ending::Running => format!("running for {took}"),
            ending => format!("{} after {took}", ending.said()),
        };
        match &row.name {
            Some(task) => format!("Task {task} · {how}"),
            None => how,
        }
    }
}

impl Render for RunDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let Some(shown) = self.shown.clone() else {
            return div()
                .id("run-drawer-reading")
                .size_full()
                .p(space::XL)
                .ui_text()
                .text_size(px(13.))
                .text_color(p.faint)
                .child("Reading…")
                .into_any_element();
        };
        let now = now_ms();
        let row = &shown.row;
        let (run, page) = (row.run.clone(), self.page.clone());
        let close = {
            let page = self.page.clone();
            Button::icon("run-drawer-close", Icon::Close, "Close (Esc)")
                .on_click(move |_, _, cx| page.close(cx))
        };
        let stop = row.running().then(|| {
            Button::secondary("command-panel-stop", "Stop")
                .leading(Icon::Stop)
                .on_click(move |_, _, cx| page.stop(&run, cx))
        });
        let head = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(space::MD)
            .py(space::MD)
            .pl(space::XL)
            .pr(space::MD)
            .border_b_1()
            .border_color(p.line)
            .child(self.mark.clone())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("command-panel-line")
                            .mono(14.)
                            .medium()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .aria_label(SharedString::from(row.line.clone()))
                            .child(SharedString::from(row.line.clone()))
                            .test_support(),
                    )
                    .child({
                        let said = Self::standing(row, now);
                        div()
                            .id("command-panel-status")
                            .text_size(px(12.))
                            .text_color(p.faint)
                            .aria_label(SharedString::from(said.clone()))
                            .child(SharedString::from(said))
                            .test_support()
                    }),
            )
            .children(stop)
            .child(close);
        let who = match row.agent.as_str() {
            "" => "you".to_owned(),
            agent => format!("agent {agent}"),
        };
        let mut facts = vec![("Command", row.line.clone())];
        if let Some(task) = &row.name {
            facts.push(("Task", task.clone()));
        }
        facts.push(("Started by", who));
        facts.push((
            "Started",
            format!("{} ago", seconds(now.saturating_sub(row.started_at))),
        ));
        let facts = div()
            .id("run-drawer-facts")
            .flex_none()
            .flex()
            .flex_col()
            .gap(space::XS)
            .py(space::MD)
            .px(space::XL)
            .border_b_1()
            .border_color(p.line)
            .text_size(px(12.5))
            .children(facts.into_iter().map(|(label, value)| {
                div()
                    .id(ElementId::Name(
                        format!("run-fact-{}", label.to_lowercase().replace(' ', "-")).into(),
                    ))
                    .aria_label(SharedString::from(format!("{label} {value}")))
                    .flex()
                    .gap(space::LG)
                    .child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .text_color(p.faint)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .mono(12.)
                            .text_color(p.text)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(value)),
                    )
                    .test_support()
            }));
        let text = shown.lines.join("\n");
        let copy = Button::quiet("run-drawer-copy", "Copy").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(ansi::strip(&text)));
            toast(
                Toast::success("Output copied").detail("The lines the drawer shows."),
                cx,
            );
        });
        let output = div()
            .id("command-panel-output")
            .flex_1()
            .min_h_0()
            .py(space::SM)
            .px(space::MD)
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.field_line)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .when(shown.earlier, |d| {
                d.child(
                    div()
                        .mono(11.)
                        .text_color(p.faint)
                        .child("(earlier lines are left out)"),
                )
            })
            .children(shown.lines.iter().enumerate().map(|(i, line)| {
                coloured_line(
                    ElementId::Name(format!("command-panel-line-{i}").into()),
                    line,
                    p.mute,
                    &p,
                )
            }))
            .when(shown.lines.is_empty(), |d| {
                d.child(
                    div()
                        .mono(11.5)
                        .text_color(p.faint)
                        .child("(no output yet)"),
                )
            })
            .test_support();
        div()
            .id("run-drawer-content")
            .size_full()
            .flex()
            .flex_col()
            .ui_text()
            .text_color(p.text)
            .child(head)
            .child(facts)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .pt(space::MD)
                    .pb(space::LG)
                    .pl(space::XL)
                    .pr(space::MD)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .text_size(px(12.5))
                            .medium()
                            .child("Output")
                            .child(
                                div()
                                    .flex_1()
                                    .mono(11.)
                                    .font_weight(gpui_kit::FontWeight::NORMAL)
                                    .text_color(p.faint)
                                    .child(if row.running() {
                                        "follows the end"
                                    } else {
                                        "ended"
                                    }),
                            )
                            .child(copy),
                    )
                    .child(output),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(run: &str, state: &str, started_at: u64, ended_at: Option<u64>) -> CommandRow {
        CommandRow {
            run: run.into(),
            agent: "a".into(),
            name: None,
            line: run.into(),
            state: state.into(),
            exit_code: Some(0),
            signal: None,
            started_at,
            ended_at,
            last_line: String::new(),
            tail: Vec::new(),
        }
    }

    fn runs(rows: &[&CommandRow]) -> Vec<String> {
        rows.iter().map(|row| row.run.clone()).collect()
    }

    #[test]
    fn running_come_first_then_the_last_three_ended_and_the_older_ones_are_folded() {
        let rows = vec![
            row("e1", "exited", 10, Some(20)),
            row("r1", "running", 15, None),
            row("e2", "exited", 11, Some(30)),
            row("e3", "exited", 12, Some(40)),
            row("e4", "exited", 13, Some(50)),
            row("r2", "running", 16, None),
            row("e5", "stopped", 14, Some(60)),
        ];
        let arranged = arrange(&rows, None);
        assert_eq!(runs(&arranged.running), ["r1", "r2"]);
        assert_eq!(runs(&arranged.kept), ["e5", "e4", "e3"], "the latest first");
        assert_eq!(runs(&arranged.folded), ["e2", "e1"]);
    }

    #[test]
    fn an_ended_run_from_before_the_last_turn_is_folded_whatever_the_count() {
        let rows = vec![
            row("old", "exited", 1, Some(100)),
            row("new", "exited", 150, Some(210)),
        ];
        let arranged = arrange(&rows, Some(200));
        assert_eq!(runs(&arranged.kept), ["new"]);
        assert_eq!(runs(&arranged.folded), ["old"]);
        assert_eq!(
            arrange(&rows, None).folded.len(),
            0,
            "no turn known, no fold"
        );
    }

    #[test]
    fn the_project_page_keeps_the_last_five_finished_runs_in_sight() {
        let rows: Vec<CommandRow> = (0..7)
            .map(|at| row(&format!("e{at}"), "exited", at, Some(10 + at)))
            .chain([row("r", "running", 3, None)])
            .collect();
        let arranged = arrange_keeping(&rows, None, crate::tasks::KEPT_ON_THE_PAGE);
        assert_eq!(runs(&arranged.running), ["r"]);
        assert_eq!(runs(&arranged.kept), ["e6", "e5", "e4", "e3", "e2"]);
        assert_eq!(runs(&arranged.folded), ["e1", "e0"]);
    }

    #[test]
    fn a_running_run_is_never_folded_by_a_turn() {
        let rows = vec![row("long", "running", 1, None)];
        let arranged = arrange(&rows, Some(1_000));
        assert_eq!(runs(&arranged.running), ["long"]);
        assert!(arranged.kept.is_empty() && arranged.folded.is_empty());
    }
}
