//! How agents are told to work, edited in the window (`docs/projects.md`, "Rules"): the "Rules"
//! page of Settings for every project, and the "Rules" section of a project's own Settings. The
//! window writes nothing itself: Save and Reset to default are the daemon's (`rules.set`,
//! `rules.reset`), and the editor shows what the daemon says next.
//!
//! The text is frozen in an agent when it is created, so a change reaches the agents created
//! after it only: the editor says so.

use flottille_ui::{Button, Card, palette};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::procedures::{editor_style, perform_later};
use crate::shell::Shell;

/// What the editor says of when a change takes effect.
pub const REACH: &str = "A change reaches the agents created from now on; an agent already \
                         created keeps the rules it was created with.";

/// Where the text the daemon last gave comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Flottille's own text.
    Default,
    /// The user's, for every project.
    Global,
    /// The project's own.
    Project,
}

/// The rules as the daemon says them (`rules.show`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    pub source: Source,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        Shown {
            text: value["text"].as_str().unwrap_or_default().to_owned(),
            source: match value["source"].as_str() {
                Some("project") => Source::Project,
                Some("global") => Source::Global,
                _ => Source::Default,
            },
        }
    }
}

pub struct RulesEditor {
    shell: WeakEntity<Shell>,
    /// The project it edits the rules of; the user's own for every project when none.
    project: Option<String>,
    /// Made when first drawn, since a text field needs a window.
    field: Option<Entity<TextareaState>>,
    shown: Option<Shown>,
    /// A save or a reset waits for the daemon.
    waiting: bool,
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl RulesEditor {
    pub fn new(shell: WeakEntity<Shell>, project: Option<String>) -> Self {
        Self {
            shell,
            project,
            field: None,
            shown: None,
            waiting: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    /// Asks the daemon for the rules.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::RulesRead { project }, cx);
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn field(&self) -> Option<&Entity<TextareaState>> {
        self.field.as_ref()
    }

    /// The text in the editor.
    pub fn typed(&self, cx: &gpui_kit::App) -> Option<String> {
        self.field
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
    }

    /// The text differs from what the daemon last said.
    pub fn is_edited(&self, cx: &gpui_kit::App) -> bool {
        match (&self.shown, self.typed(cx)) {
            (Some(shown), Some(typed)) => shown.text != typed,
            _ => false,
        }
    }

    /// Whether Reset to default has something to remove: rules of its own at this level.
    pub fn is_own(&self) -> bool {
        let own = match self.project {
            Some(_) => Source::Project,
            None => Source::Global,
        };
        self.shown.as_ref().is_some_and(|shown| shown.source == own)
    }

    fn make_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(8, 18)
                .placeholder("how your agents should work");
            field.set_editor_style(editor_style(cx));
            field
        });
        self._subscriptions
            .push(cx.subscribe(&field, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.problem = None;
                    cx.notify();
                }
            }));
        if let Some(shown) = &self.shown {
            field.update(cx, |field, cx| field.set_value(&shown.text, window, cx));
        }
        self.field = Some(field.clone());
        field
    }

    /// The daemon's answer to a read, a save or a reset: what it says the rules are, or why not.
    /// Typing in progress is kept when a read comes back, and replaced by what was saved.
    pub fn answered(
        &mut self,
        said: Result<Shown, String>,
        written: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.waiting = false;
        match said {
            Ok(shown) => {
                self.problem = None;
                let keep = !written && self.is_edited(cx);
                if !keep && let Some(field) = self.field.clone() {
                    field.update(cx, |field, cx| field.set_value(&shown.text, window, cx));
                }
                self.shown = Some(shown);
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.typed(cx) else {
            return;
        };
        if self.waiting || !self.is_edited(cx) {
            return;
        }
        self.waiting = true;
        self.problem = None;
        let project = self.project.clone();
        perform_later(
            &self.shell,
            Command::RulesWrite {
                project,
                text: Some(text),
            },
            cx,
        );
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.waiting || !self.is_own() {
            return;
        }
        self.waiting = true;
        self.problem = None;
        let project = self.project.clone();
        perform_later(
            &self.shell,
            Command::RulesWrite {
                project,
                text: None,
            },
            cx,
        );
        cx.notify();
    }

    /// What the editor says of where the text comes from.
    fn origin(&self) -> &'static str {
        match (&self.project, self.shown.as_ref().map(|shown| shown.source)) {
            (_, None) => "Asking the daemon…",
            (None, Some(Source::Global)) => "Your own rules, for every project.",
            (None, Some(_)) => "Flottille's default rules: edit them to make them your own.",
            (Some(_), Some(Source::Project)) => "This project's own rules.",
            (Some(_), Some(Source::Global)) => {
                "This project follows your rules for every project. Edit and save to give it \
                 rules of its own."
            }
            (Some(_), Some(Source::Default)) => {
                "This project follows Flottille's default rules. Edit and save to give it rules \
                 of its own."
            }
        }
    }
}

impl Render for RulesEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let field = match self.field.clone() {
            Some(field) => field,
            None => self.make_field(window, cx),
        };
        let scope = self.project.clone().unwrap_or_default();
        let id = |part: &str| ElementId::Name(format!("rules-{part}-{scope}").into());
        let edited = self.is_edited(cx);
        let loaded = self.shown.is_some();
        let buttons = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Button::secondary(id("reset"), "Reset to default")
                    .disabled(!self.is_own() || self.waiting)
                    .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
            )
            .child(div().flex_1())
            .child(
                Button::primary(id("save"), "Save")
                    .disabled(!edited || self.waiting || !loaded)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        let body = div()
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id(id("origin"))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(self.origin())
                    .test_support(),
            )
            .child(
                div()
                    .id(id("text"))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(p.field)
                    .border_1()
                    .border_color(p.line2)
                    .text_size(px(12.5))
                    .child(field)
                    .test_support(),
            )
            .child(
                div()
                    .id(id("reach"))
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child(REACH)
                    .test_support(),
            )
            .when_some(self.problem.clone(), |d, problem| {
                d.child(
                    div()
                        .id(id("problem"))
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(problem)
                        .test_support(),
                )
            })
            .child(buttons);
        Card::new().title("Rules").child(body)
    }
}
