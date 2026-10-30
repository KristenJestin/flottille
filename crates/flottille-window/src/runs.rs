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
use flottille_ui::motion::PANEL;
use flottille_ui::{
    Button, Icon, Item, Listing, Modal, Palette, Type, fade, hoverable, icon, palette, presence,
    section_label, space, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, ElementId, FocusHandle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Task, TestSupportExt as _, Transformation, WeakEntity, Window, div, px, radians,
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
    let running = rows.iter().filter(|row| row.running()).collect();
    let mut ended: Vec<&CommandRow> = rows.iter().filter(|row| !row.running()).collect();
    let ended_at = |row: &CommandRow| row.ended_at.unwrap_or(row.started_at);
    ended.sort_by_key(|row| std::cmp::Reverse((ended_at(row), row.started_at)));
    let (mut kept, mut folded) = (Vec::new(), Vec::new());
    for row in ended {
        if kept.len() < KEPT_ENDED && turn_at.is_none_or(|turn| ended_at(row) >= turn) {
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

/// The panel that follows one command's output while it is open.
pub struct CommandPanel {
    page: Owner,
    /// The command whose output is open.
    run: Option<String>,
    shown: Option<CommandOutput>,
    scroll: ScrollHandle,
    /// Where the focus rests while the panel is open, so that Escape reaches the modal.
    focus: Option<FocusHandle>,
    focused: bool,
    renders: usize,
}

impl CommandPanel {
    pub fn new(page: impl Into<Owner>) -> Self {
        Self {
            page: page.into(),
            run: None,
            shown: None,
            scroll: ScrollHandle::new(),
            focus: None,
            focused: false,
            renders: 0,
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

    pub fn open(&mut self, run: &str, cx: &mut Context<Self>) {
        if self.run.as_deref() != Some(run) {
            self.shown = None;
        }
        self.run = Some(run.to_owned());
        self.focused = false;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.run = None;
        self.focused = false;
        self.shown = None;
        cx.notify();
    }

    /// What the daemon gave of the output; one of a command closed since is dropped.
    pub fn set(&mut self, output: CommandOutput, cx: &mut Context<Self>) {
        if self.run.as_deref() != Some(output.row.run.as_str())
            || self.shown.as_ref() == Some(&output)
        {
            return;
        }
        self.shown = Some(output);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }
}

impl Render for CommandPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let page = self.page.clone();
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if self.run.is_some() && !self.focused {
            self.focused = true;
            window.focus(&focus, cx);
        }
        let mut body = div()
            .id("command-panel-body")
            .flex()
            .flex_col()
            .gap(space::LG)
            .track_focus(&focus);
        let mut modal = Modal::new("command-panel", "Output")
            .wide()
            .cancel_label("Close")
            .open(self.run.is_some())
            .on_close(move |_, cx| page.close(cx));
        if let Some(shown) = &self.shown {
            let (glyph, color, said) = mark_of(shown.row.ending(), &p);
            let run = shown.row.run.clone();
            let page = self.page.clone();
            if shown.row.running() {
                modal = modal.action(
                    Button::primary("command-panel-stop", "Stop")
                        .on_click(move |_, _, cx| page.stop(&run, cx)),
                );
            }
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::SM)
                        .child(
                            div()
                                .id("command-panel-line")
                                .flex_1()
                                .min_w_0()
                                .mono(12.5)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .aria_label(SharedString::from(shown.row.line.clone()))
                                .child(SharedString::from(shown.row.line.clone()))
                                .test_support(),
                        )
                        .child(
                            div()
                                .id("command-panel-status")
                                .flex_none()
                                .mono(11.5)
                                .text_color(color)
                                .aria_label(SharedString::from(said.clone()))
                                .child(SharedString::from(format!("{glyph} {said}")))
                                .test_support(),
                        ),
                )
                .child(
                    div()
                        .id("command-panel-output")
                        .h(px(340.))
                        .p(space::MD)
                        .rounded(px(8.))
                        .bg(p.field)
                        .border_1()
                        .border_color(p.line)
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
                                p.text,
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
                        .test_support(),
                );
        } else if self.run.is_some() {
            body = body.child(div().text_color(p.faint).child("Reading…"));
        }
        modal.child(body)
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
    fn a_running_run_is_never_folded_by_a_turn() {
        let rows = vec![row("long", "running", 1, None)];
        let arranged = arrange(&rows, Some(1_000));
        assert_eq!(runs(&arranged.running), ["long"]);
        assert!(arranged.kept.is_empty() && arranged.folded.is_empty());
    }
}
