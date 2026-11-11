//! The project's message checks, in the Rules section of its Settings (`docs/projects.md`,
//! "Message checks"): one command line each, run on an agent's commit message, a pull request's
//! title and a squash message. The window writes nothing itself: Save is the daemon's
//! (`project.set_message_checks`), and the field shows what the daemon says next.

use gpui_design_system::{
    Button, Field, Section, Toast, Type as _, follow_theme, growing, palette, presence, space,
    toast,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::Command;
use crate::commands::perform_later;
use crate::shell::Shell;

/// What the section says they are for.
pub const SENTENCE: &str = "Command lines run on a message before an agent commits, opens a pull \
                            request (its title) or merges by squash; {message} stands for the \
                            file that holds it. One that fails refuses it, and the agent is told \
                            why. They are yours: the repository's own hooks never run.";

pub struct MessageChecks {
    shell: WeakEntity<Shell>,
    project: String,
    /// Made when first drawn, since a text field needs a window.
    field: Option<Entity<TextareaState>>,
    /// The checks as the daemon last said them; none until it answered.
    saved: Option<Vec<String>>,
    /// A save waits for the daemon.
    waiting: bool,
    /// Why the last save was refused, under the field until the text changes.
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl MessageChecks {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            field: None,
            saved: None,
            waiting: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn saved(&self) -> Option<&Vec<String>> {
        self.saved.as_ref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn field(&self) -> Option<&Entity<TextareaState>> {
        self.field.as_ref()
    }

    /// The text in the field.
    pub fn typed(&self, cx: &gpui_kit::App) -> Option<String> {
        self.field
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
    }

    /// The checks the field holds: one a line, blank lines left out.
    fn lines(&self, cx: &gpui_kit::App) -> Option<Vec<String>> {
        self.typed(cx).map(|typed| {
            typed
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        })
    }

    /// Whether Save has something to write: the field differs from what the daemon last said.
    pub fn savable(&self, cx: &gpui_kit::App) -> bool {
        !self.waiting
            && match (&self.saved, self.lines(cx)) {
                (Some(saved), Some(lines)) => *saved != lines,
                _ => false,
            }
    }

    /// What the settings say the file declares, read or saved: the field follows it unless
    /// something is being typed.
    pub fn read(&mut self, checks: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let typing = match (&self.saved, self.lines(cx)) {
            (Some(saved), Some(lines)) => *saved != lines,
            _ => false,
        };
        if !typing && let Some(field) = self.field.clone() {
            let text = checks.join("\n");
            field.update(cx, |field, cx| field.set_value(&text, window, cx));
        }
        self.saved = Some(checks);
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(checks) = self.lines(cx) else {
            return;
        };
        if !self.savable(cx) {
            return;
        }
        self.waiting = true;
        self.problem = None;
        let command = Command::SaveMessageChecks {
            project: self.project.clone(),
            checks,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to a save: the checks it wrote, or why it wrote nothing, said under
    /// the field.
    pub fn answered(
        &mut self,
        saved: Result<Vec<String>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.waiting = false;
        match saved {
            Ok(checks) => {
                let detail = match checks.len() {
                    0 => "No check runs on your agents' messages.".to_owned(),
                    1 => "It runs on your agents' next messages.".to_owned(),
                    count => format!("The {count} run on your agents' next messages, in order."),
                };
                toast(Toast::success("Message checks saved").detail(detail), cx);
                if let Some(field) = self.field.clone() {
                    let text = checks.join("\n");
                    field.update(cx, |field, cx| field.set_value(&text, window, cx));
                }
                self.saved = Some(checks);
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    fn make_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder("pnpm commitlint --edit {message}");
            follow_theme(&mut field, cx);
            field
        });
        self._subscriptions
            .push(cx.subscribe(&field, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.problem = None;
                    cx.notify();
                }
            }));
        if let Some(saved) = &self.saved {
            let text = saved.join("\n");
            field.update(cx, |field, cx| field.set_value(&text, window, cx));
        }
        self.field = Some(field.clone());
        field
    }
}

impl Render for MessageChecks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let field = match self.field.clone() {
            Some(field) => field,
            None => self.make_field(window, cx),
        };
        let project = self.project.clone();
        let id = |part: &str| ElementId::Name(format!("message-checks-{part}-{project}").into());
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
        // The field grows by a line as a check is added, and its height eases there.
        let text = growing(id("text-height"), text);
        let save = div().flex().justify_end().child(
            presence(
                id("save-block"),
                self.savable(cx),
                Button::secondary(id("save"), "Save")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .gap(space::SM),
        );
        Section::new(
            format!("message-checks-{project}"),
            "Commit and pull request checks",
        )
        .sentence(SENTENCE)
        .child(
            Field::new(id("field"), "One command line per line", text)
                .optional()
                .hint("None by default. A check that cannot run refuses too.")
                .error(self.problem.clone()),
        )
        .child(save)
    }
}
