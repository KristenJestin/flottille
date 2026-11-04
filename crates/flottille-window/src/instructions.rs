//! The user's own instructions for every agent, edited in the window (`docs/agents.md`, "Roles"):
//! the "Instructions" page of Settings. The window writes nothing itself: Save is the daemon's
//! (`instructions.set`), and the editor shows what the daemon says next.
//!
//! The text is frozen in an agent when it is created, so a change reaches the agents created
//! after it only: the editor says so.

use flottille_ui::{Button, Section, Toast, growing, palette, presence_of, space, toast};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::{editor_style, perform_later};
use crate::shell::Shell;

/// What the editor says of when a change takes effect.
pub const REACH: &str = "A change reaches the agents created from now on; an agent already \
                         created keeps the instructions it was created with.";

/// The instructions as the daemon says them (`instructions.show`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The file, in Flottille's configuration folder.
    pub path: String,
    pub text: String,
    /// Why the stored text was cut to the limit: edited by hand past it.
    pub cut: Option<String>,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        Shown {
            path: value["path"].as_str().unwrap_or_default().to_owned(),
            text: value["text"].as_str().unwrap_or_default().to_owned(),
            cut: value["cut"].as_str().map(str::to_owned),
        }
    }
}

pub struct InstructionsEditor {
    shell: WeakEntity<Shell>,
    /// Made when first drawn, since a text field needs a window.
    field: Option<Entity<TextareaState>>,
    shown: Option<Shown>,
    /// A save waits for the daemon.
    waiting: bool,
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl InstructionsEditor {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            field: None,
            shown: None,
            waiting: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    /// Asks the daemon for the instructions.
    pub fn read(&self, cx: &mut Context<Self>) {
        perform_later(&self.shell, Command::InstructionsRead, cx);
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

    /// Whether Save has something to write: an edit, or a stored text cut to the limit, which
    /// saving makes fit.
    pub fn savable(&self, cx: &gpui_kit::App) -> bool {
        self.is_edited(cx) || self.shown.as_ref().is_some_and(|shown| shown.cut.is_some())
    }

    fn make_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(8, 18)
                .placeholder("your rules for every agent, in every project");
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

    /// The daemon's answer to a read or a save: what it says the instructions are, or why not.
    /// Typing in progress is kept when a read comes back, and replaced by what was saved.
    pub fn answered(
        &mut self,
        said: Result<Shown, String>,
        written: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.waiting = false;
        if written {
            let said = match &said {
                Ok(_) => Toast::success("Instructions saved")
                    .detail("The agents created from now on get them."),
                Err(why) => Toast::failure("Instructions not saved").detail(why.clone()),
            };
            toast(said, cx);
        }
        match said {
            Ok(shown) => {
                self.problem = None;
                let keep = !written && self.is_edited(cx);
                if !keep && let Some(field) = self.field.clone() {
                    field.update(cx, |field, cx| field.set_value(&shown.text, window, cx));
                }
                self.shown = Some(shown);
            }
            Err(why) if !written => self.problem = Some(why.into()),
            Err(_) => {}
        }
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.typed(cx) else {
            return;
        };
        if self.waiting || !self.savable(cx) {
            return;
        }
        self.waiting = true;
        self.problem = None;
        perform_later(&self.shell, Command::InstructionsWrite { text }, cx);
        cx.notify();
    }
}

impl Render for InstructionsEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let field = match self.field.clone() {
            Some(field) => field,
            None => self.make_field(window, cx),
        };
        let id = |part: &str| ElementId::Name(format!("instructions-{part}").into());
        let footer = div()
            .mt(space::XS)
            .flex()
            .items_center()
            .gap(space::SM)
            .child(
                div()
                    .id(id("reach"))
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child(REACH)
                    .test_support(),
            )
            .child(
                Button::primary(id("save"), "Save")
                    .disabled(!self.savable(cx) || self.waiting)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        let text = div()
            .id(id("text"))
            .min_h(px(150.))
            .px(space::MD)
            .py(space::MD)
            .rounded(px(10.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .text_size(px(13.))
            .line_height(px(20.))
            .child(field)
            .test_support();
        let line = move |id: ElementId| {
            move |said: SharedString| {
                div()
                    .id(id.clone())
                    .aria_label(said.clone())
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(said)
                    .test_support()
                    .into_any_element()
            }
        };
        Section::new("instructions", "Instructions")
            .tag(match &self.shown {
                None => "asking the daemon",
                Some(_) => "your own",
            })
            .sentence(
                "Your rules for every agent in every project. They come after Flottille's own \
                 and before the project's files, so a project can narrow them. Flottille reads \
                 none of your providers' own files.",
            )
            // The field grows with its lines: the page eases to the new height.
            .child(growing(id("grow"), text))
            .child(
                presence_of(
                    id("cut-block"),
                    self.shown
                        .as_ref()
                        .and_then(|shown| shown.cut.clone())
                        .map(|cut| SharedString::from(format!("Cut: {cut}."))),
                    line(id("cut")),
                )
                .gap(space::MD),
            )
            .child(
                presence_of(
                    id("problem-block"),
                    self.problem.clone(),
                    line(id("problem")),
                )
                .gap(space::MD),
            )
            .child(footer)
    }
}
