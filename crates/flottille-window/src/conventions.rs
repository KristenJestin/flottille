//! The project's conventions in its Settings (`docs/projects.md`, "Branch format" and "Pull
//! request template"): the regular expression its branches match, a field of the Branches
//! section, and the description of a pull request opened without one, beside the message checks.
//! Each is off when empty. The window writes nothing itself: Save is the daemon's
//! (`project.set_conventions`), and the field shows what the daemon says next.

use gpui_design_system::{
    Button, Field, Section, Toast, Type as _, follow_theme, growing, palette, presence, space,
    toast,
};
use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, TestSupportExt as _, WeakEntity,
    Window, div, px,
};

use crate::actions::Command;
use crate::commands::perform_later;
use crate::project_settings::{field_frame, styled_input};
use crate::shell::Shell;

/// Which convention a section edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    BranchFormat,
    PullRequestTemplate,
}

impl Kind {
    /// Its key in `project.settings` and `project.set_conventions`.
    pub fn key(self) -> &'static str {
        match self {
            Kind::BranchFormat => "branch_format",
            Kind::PullRequestTemplate => "pull_request_template",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Kind::BranchFormat => "branch-format",
            Kind::PullRequestTemplate => "pull-request-template",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::BranchFormat => "Allowed names",
            Kind::PullRequestTemplate => "Template, in Markdown",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Kind::BranchFormat => "Empty: any name. Anchor with ^ and $.",
            Kind::PullRequestTemplate => {
                "Off when empty: a pull request opened without a \
                                          description has none."
            }
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Kind::BranchFormat => "^(feat|fix|chore)/[A-Z]+-[0-9]+-[a-z0-9-]+$",
            Kind::PullRequestTemplate => "## Why\n\n## How to verify",
        }
    }

    /// What a field holds compared with what is kept: a pattern without its surrounding spaces,
    /// a template as typed.
    fn kept(self, typed: &str) -> String {
        match self {
            Kind::BranchFormat => typed.trim().to_owned(),
            Kind::PullRequestTemplate => match typed.trim().is_empty() {
                true => String::new(),
                false => typed.to_owned(),
            },
        }
    }

    fn saved(self, value: &str) -> Toast {
        match (self, value.is_empty()) {
            (Kind::BranchFormat, true) => Toast::success("Branch format saved")
                .detail("Branches are no longer checked against a format."),
            (Kind::BranchFormat, false) => Toast::success("Branch format saved")
                .detail(format!("New branches must match {value}.")),
            (Kind::PullRequestTemplate, true) => Toast::success("Pull request template saved")
                .detail("A pull request opened without a description has none."),
            (Kind::PullRequestTemplate, false) => Toast::success("Pull request template saved")
                .detail("A pull request opened without a description takes it."),
        }
    }
}

/// The field of a section, made when first drawn since a text field needs a window: one line
/// for a pattern, several for a template.
#[derive(Clone)]
enum Text {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

impl Text {
    fn value(&self, cx: &App) -> String {
        match self {
            Text::Line(field) => field.read(cx).value().to_string(),
            Text::Area(field) => field.read(cx).value().to_string(),
        }
    }

    fn set(&self, text: &str, window: &mut Window, cx: &mut App) {
        match self {
            Text::Line(field) => field.update(cx, |field, cx| field.set_value(text, window, cx)),
            Text::Area(field) => field.update(cx, |field, cx| field.set_value(text, window, cx)),
        }
    }
}

pub struct Convention {
    shell: WeakEntity<Shell>,
    project: String,
    kind: Kind,
    field: Option<Text>,
    /// The convention as the daemon last said it, empty when off; none until it answered.
    saved: Option<String>,
    /// A save waits for the daemon.
    waiting: bool,
    /// Why the last save was refused, under the field until the text changes.
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl Convention {
    pub fn new(shell: WeakEntity<Shell>, project: String, kind: Kind) -> Self {
        Self {
            shell,
            project,
            kind,
            field: None,
            saved: None,
            waiting: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The convention the file has, empty when off.
    pub fn saved(&self) -> Option<&str> {
        self.saved.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The text in the field, none before it is drawn.
    pub fn typed(&self, cx: &App) -> Option<String> {
        self.field.as_ref().map(|field| field.value(cx))
    }

    /// Types `text` into the field, as the user would.
    pub fn type_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        if let Some(field) = &self.field {
            field.set(text, window, cx);
        }
    }

    /// Whether Save has something to write: the field differs from what the daemon last said.
    pub fn savable(&self, cx: &App) -> bool {
        !self.waiting
            && match (&self.saved, self.typed(cx)) {
                (Some(saved), Some(typed)) => *saved != self.kind.kept(&typed),
                _ => false,
            }
    }

    /// What the settings say the file holds, read or saved: the field follows it unless something
    /// is being typed.
    pub fn read(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let typing = self.savable(cx) || self.waiting;
        if !typing && let Some(field) = self.field.clone() {
            field.set(&value, window, cx);
        }
        self.saved = Some(value);
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(typed) = self.typed(cx) else {
            return;
        };
        if !self.savable(cx) {
            return;
        }
        self.waiting = true;
        self.problem = None;
        let command = Command::SaveConvention {
            project: self.project.clone(),
            key: self.kind.key(),
            value: self.kind.kept(&typed),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to a save: the convention it wrote, or why it wrote nothing, said
    /// under the field.
    pub fn answered(
        &mut self,
        saved: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.waiting = false;
        match saved {
            Ok(value) => {
                toast(self.kind.saved(&value), cx);
                if let Some(field) = self.field.clone() {
                    field.set(&value, window, cx);
                }
                self.saved = Some(value);
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    fn make_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Text {
        let placeholder = self.kind.placeholder();
        let field = match self.kind {
            Kind::BranchFormat => {
                let field = cx.new(|cx| styled_input(placeholder, window, cx));
                self._subscriptions
                    .push(cx.subscribe(&field, |this, _, event, cx| match event {
                        InputEvent::Change => this.changed(cx),
                        InputEvent::PressEnter { .. } => this.save(cx),
                        _ => {}
                    }));
                Text::Line(field)
            }
            Kind::PullRequestTemplate => {
                let field = cx.new(|cx| {
                    let mut field = TextareaState::new(window, cx)
                        .auto_grow(4, 14)
                        .placeholder(placeholder);
                    follow_theme(&mut field, cx);
                    field
                });
                self._subscriptions
                    .push(cx.subscribe(&field, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.changed(cx);
                        }
                    }));
                Text::Area(field)
            }
        };
        if let Some(saved) = &self.saved {
            field.set(saved, window, cx);
        }
        self.field = Some(field.clone());
        field
    }

    /// Save follows what is typed, and a refusal leaves once the text changes.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        cx.notify();
    }
}

impl Render for Convention {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let field = match self.field.clone() {
            Some(field) => field,
            None => self.make_field(window, cx),
        };
        let (project, kind) = (self.project.clone(), self.kind);
        let id = |part: &str| ElementId::Name(format!("{}-{part}-{project}", kind.id()).into());
        // Save unfolds beside the field once what is typed differs from what is kept (rule 4); a
        // presence is as wide as its parent, so the button keeps its own width.
        let save = div().flex_none().child(
            presence(
                id("save-block"),
                self.savable(cx),
                Button::secondary(id("save"), "Save")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .gap(space::SM),
        );
        let control = match field {
            Text::Line(field) => div()
                .flex()
                .items_center()
                .gap(space::SM)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(field_frame(id("input"), &field, cx)),
                )
                .child(save)
                .into_any_element(),
            Text::Area(field) => {
                let text = div()
                    .id(id("text"))
                    .px(space::MD)
                    .py(space::SM)
                    .rounded(px(8.))
                    .bg(p.field)
                    .border_1()
                    .border_color(p.line2)
                    .mono(12.5)
                    .line_height(px(20.))
                    .child(field)
                    .test_support();
                // The field grows by a line as the template does, and its height eases there.
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .child(growing(id("text-height"), text))
                    .child(div().flex().justify_end().child(save))
                    .into_any_element()
            }
        };
        let field = Field::new(id("field"), kind.label(), control)
            .optional()
            .hint(kind.hint())
            .error(self.problem.clone());
        // The branch format is a field of the page's Branches section, beside the prefix.
        match kind {
            Kind::BranchFormat => field.into_any_element(),
            Kind::PullRequestTemplate => Section::new(
                format!("{}-section-{project}", kind.id()),
                "Pull request template",
            )
            .sentence(
                "The description of a pull request an agent opens without one, in Markdown. \
                 Agents are told it exists, so that they fill it rather than replace it.",
            )
            .child(field)
            .into_any_element(),
        }
    }
}
