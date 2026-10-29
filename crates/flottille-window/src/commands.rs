//! The commands the agents run, as the window shows them: the rail's Commands section (every
//! command of the session's tree, running ones first, ended ones greyed until dismissed), the panel
//! that follows one's output live, and the card a command's result is in the conversation.
//!
//! What a program wrote is drawn as the daemon gave it (masked): its colours (`ESC [ … m`) turned
//! into the palette's, every other escape sequence dropped (`flottille_core::ansi`). The stored
//! output is untouched.

use std::collections::HashMap;
use std::time::Duration;

use flottille_core::ansi;
use flottille_ui::color::mix;
use flottille_ui::{Button, Card, Icon, Modal, Palette, Type, icon, palette, section_label};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, ElementId, FocusHandle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Task, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::conversation::{CommandCard, Ending};
use crate::session::{SessionPage, seconds};
use crate::shell::now_ms;

/// The lines a card shows before it is unfolded.
pub const FOLDED_LINES: usize = 5;

/// A command of the session, as `command.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRow {
    pub run: String,
    /// The agent that started it.
    pub agent: String,
    /// The catalogue command it runs, none for a line of the agent's.
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
    match ending {
        Ending::Running => ("●", p.run, "running".into()),
        Ending::Exited(0) => ("✓", p.ok, "exit 0".into()),
        Ending::Exited(code) => ("✗", p.fail, format!("exit {code}")),
        Ending::Killed(signal) => ("✗", p.fail, format!("signal {signal}")),
        Ending::Stopped => ("■", p.faint, "stopped".into()),
        Ending::Interrupted => ("–", p.faint, "interrupted".into()),
    }
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
    div()
        .id(id)
        .flex()
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

/// A command's result in the conversation: its line, its exit code in a colour, its duration, and
/// its output folded to its last lines until `open`. `at` makes the ids; `toggle` unfolds it.
pub fn card(
    card: &CommandCard,
    at: usize,
    open: bool,
    toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    let (glyph, color, said) = mark_of(card.ending, &p);
    let id = |part: &str| ElementId::Name(format!("command-{part}-{at}").into());
    let hidden = if open {
        0
    } else {
        card.lines.len().saturating_sub(FOLDED_LINES)
    };
    let shown = &card.lines[hidden..];
    let title = div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(8.))
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
        .child(
            div()
                .id(id("exit"))
                .flex_none()
                .mono(11.5)
                .text_color(color)
                .aria_label(SharedString::from(said.clone()))
                .child(SharedString::from(format!("{glyph} {said}")))
                .test_support(),
        );
    let body = div()
        .id(id("output"))
        .px(px(14.))
        .py(px(8.))
        .flex()
        .flex_col()
        .children(shown.iter().enumerate().map(|(i, line)| {
            coloured_line(
                ElementId::Name(format!("command-line-{at}-{}", hidden + i).into()),
                line,
                p.mute,
                &p,
            )
        }))
        .when(card.lines.is_empty(), |d| {
            d.child(div().mono(11.5).text_color(p.faint).child("(no output)"))
        })
        .test_support();
    let more = (card.lines.len() > FOLDED_LINES).then(|| {
        let said = match open {
            true => "Fold".to_owned(),
            false => format!("Show {hidden} earlier lines"),
        };
        div()
            .id(id("more"))
            .px(px(14.))
            .pb(px(8.))
            .mono(11.)
            .text_color(p.faint)
            .cursor_pointer()
            .hover(|s| s.text_color(p.text))
            .on_click(toggle)
            .child(SharedString::from(said))
            .test_support()
    });
    let allowed = card.allowed_by.as_ref().map(|by| {
        div()
            .id(id("allowed"))
            .px(px(14.))
            .pb(px(10.))
            .text_size(px(12.))
            .text_color(p.faint)
            .child(SharedString::from(format!("Allowed by {by}")))
            .test_support()
    });
    div()
        .id(id("card"))
        .child(
            Card::new()
                .title(title)
                .meta(seconds_text(card.seconds))
                .child(body)
                .children_of(more)
                .children_of(allowed),
        )
        .test_support()
        .into_any_element()
}

fn seconds_text(seconds_taken: Option<f64>) -> String {
    seconds_taken.map_or_else(String::new, |s| seconds((s * 1000.) as u64))
}

/// The rail's Commands section: nothing while the session ran no command.
pub struct CommandList {
    page: WeakEntity<SessionPage>,
    rows: Vec<CommandRow>,
    /// How the tree names each agent.
    names: HashMap<String, String>,
    error: Option<SharedString>,
    /// Draws the time again each second, while a command runs.
    ticker: Option<Task<()>>,
    renders: usize,
}

impl CommandList {
    pub fn new(page: WeakEntity<SessionPage>) -> Self {
        Self {
            page,
            rows: Vec::new(),
            names: HashMap::new(),
            error: None,
            ticker: None,
            renders: 0,
        }
    }

    pub fn rows(&self) -> &[CommandRow] {
        &self.rows
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

    /// The daemon refused what was asked of a command, and why; none clears it.
    pub fn failed(&mut self, why: Option<String>, cx: &mut Context<Self>) {
        self.error = why.map(Into::into);
        cx.notify();
    }

    /// Who started a command: the catalogue command, then the agent.
    fn who(&self, row: &CommandRow) -> String {
        let agent = self
            .names
            .get(&row.agent)
            .cloned()
            .unwrap_or_else(|| format!("agent {}", row.agent));
        match &row.name {
            Some(name) => format!("task {name} · {agent}"),
            None => agent,
        }
    }
}

impl Render for CommandList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        if self.rows.is_empty() {
            return div().id("rail-commands").into_any_element();
        }
        let p = palette(cx);
        let now = now_ms();
        let rows = self.rows.iter().map(|row| {
            let ended = !row.running();
            let (glyph, color, said) = mark_of(row.ending(), &p);
            let run = row.run.clone();
            let open = {
                let page = self.page.clone();
                let run = run.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    page.update(cx, |page, cx| page.open_command(&run, cx)).ok();
                }
            };
            let button = if ended {
                let page = self.page.clone();
                let run = run.clone();
                Button::quiet(
                    ElementId::Name(format!("command-dismiss-{run}").into()),
                    "Dismiss",
                )
                .on_click(move |_, _, cx| {
                    // The row opens the output; its buttons do not.
                    cx.stop_propagation();
                    page.update(cx, |page, cx| page.dismiss_command(&run, cx))
                        .ok();
                })
            } else {
                let page = self.page.clone();
                let run = run.clone();
                Button::quiet(
                    ElementId::Name(format!("command-stop-{run}").into()),
                    "Stop",
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    page.update(cx, |page, cx| page.stop_command(&run, cx)).ok();
                })
            };
            let last = ansi::strip(&row.last_line);
            div()
                .id(ElementId::Name(format!("command-{}", row.run).into()))
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(p.hover))
                .when(ended, |d| d.opacity(0.6))
                .on_click(open)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .id(ElementId::Name(format!("command-mark-{}", row.run).into()))
                                .flex_none()
                                .mono(12.)
                                .text_color(color)
                                .aria_label(SharedString::from(said.clone()))
                                .child(glyph)
                                .test_support(),
                        )
                        .child(
                            div()
                                .id(ElementId::Name(format!("command-line-{}", row.run).into()))
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
                                .id(ElementId::Name(format!("command-time-{}", row.run).into()))
                                .flex_none()
                                .mono(11.)
                                .text_color(p.faint)
                                .child(SharedString::from(seconds(row.elapsed_ms(now))))
                                .test_support(),
                        ),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("command-who-{}", row.run).into()))
                        .pl(px(20.))
                        .text_size(px(11.5))
                        .text_color(p.faint)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .aria_label(SharedString::from(self.who(row)))
                        .child(SharedString::from(self.who(row)))
                        .test_support(),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("command-last-{}", row.run).into()))
                        .pl(px(20.))
                        .mono(11.)
                        .text_color(p.faint)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .aria_label(SharedString::from(last.clone()))
                        .child(SharedString::from(last))
                        .test_support(),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .pl(px(20.))
                        .when(ended, |d| {
                            d.child(
                                div()
                                    .id(ElementId::Name(
                                        format!("command-status-{}", row.run).into(),
                                    ))
                                    .mono(11.)
                                    .text_color(color)
                                    .aria_label(SharedString::from(said.clone()))
                                    .child(SharedString::from(said.clone()))
                                    .test_support(),
                            )
                        })
                        .child(div().flex_1())
                        .child(button),
                )
                .test_support()
        });
        div()
            .id("rail-commands")
            .flex()
            .flex_col()
            .child(div().h(px(1.)).bg(p.line).mx(px(6.)).my(px(16.)))
            .child(section_label("Commands", cx))
            .children(rows)
            .children(self.error.clone().map(|error| {
                div()
                    .id("rail-commands-error")
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(11.5))
                    .text_color(p.fail)
                    .child(error)
                    .test_support()
            }))
            .test_support()
            .into_any_element()
    }
}

/// The panel that follows one command's output while it is open.
pub struct CommandPanel {
    page: WeakEntity<SessionPage>,
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
    pub fn new(page: WeakEntity<SessionPage>) -> Self {
        Self {
            page,
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
            .gap(px(14.))
            .track_focus(&focus);
        let mut modal = Modal::new("command-panel", "Output")
            .width(px(720.))
            .cancel_label("Close")
            .open(self.run.is_some())
            .on_close(move |_, cx| {
                page.update(cx, |page, cx| page.close_command(cx)).ok();
            });
        if let Some(shown) = &self.shown {
            let (glyph, color, said) = mark_of(shown.row.ending(), &p);
            let run = shown.row.run.clone();
            let page = self.page.clone();
            if shown.row.running() {
                modal = modal.action(Button::primary("command-panel-stop", "Stop").on_click(
                    move |_, _, cx| {
                        page.update(cx, |page, cx| page.stop_command(&run, cx)).ok();
                    },
                ));
            }
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
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
                        .p(px(10.))
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

/// `Card`'s optional child: nothing for none.
trait ChildrenOf: Sized {
    fn children_of(self, child: Option<impl IntoElement>) -> Self;
}

impl ChildrenOf for Card {
    fn children_of(self, child: Option<impl IntoElement>) -> Self {
        match child {
            Some(child) => self.child(child),
            None => self,
        }
    }
}
