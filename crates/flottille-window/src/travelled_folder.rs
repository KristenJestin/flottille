//! Where this machine makes the projects that come with another machine's launches, chosen in
//! Settings, General (`docs/remote.md`, "Which project"): Flottille's data folder unless the user
//! types a folder of their own. The window writes nothing itself: a choice is the daemon's
//! (`travelled_folder.set`) of the machine shown, this one or another through "Settings of", and
//! the section shows what that daemon says next.
//!
//! Only the projects made after a change go there: the section says so.

use gpui_design_system::{
    Button, Field, Section, Skeleton, Type as _, awaiting, changing, follow_theme, palette,
    presence, presence_of, space,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    TestSupportExt as _, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::perform_later;

/// What the section says of which projects a change reaches.
pub const REACH: &str = "A change applies to the projects made from now on. The ones that exist \
                         stay where they are: nothing is moved.";

/// What `travelled_folder.show` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// Where the next project is made.
    pub folder: String,
    /// The user's folder as written; `None` while the default is followed.
    pub own: Option<String>,
    /// Why the user's file cannot be followed.
    pub error: Option<String>,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        Shown {
            folder: text("folder").unwrap_or_default(),
            own: text("own"),
            error: text("error"),
        }
    }
}

pub struct TravelledFolder {
    shell: crate::commands::ShellOf,
    /// Made when first drawn, since a text field needs a window.
    field: Option<Entity<InputState>>,
    shown: Option<Shown>,
    /// A write waits for the daemon.
    waiting: bool,
    /// Why the last write was refused, under the field.
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl TravelledFolder {
    pub fn new(shell: crate::commands::ShellOf) -> Self {
        Self {
            shell,
            field: None,
            shown: None,
            waiting: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn field(&self) -> Option<&Entity<InputState>> {
        self.field.as_ref()
    }

    /// The text in the field.
    pub fn typed(&self, cx: &gpui_kit::App) -> Option<String> {
        self.field
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
    }

    /// Asks the daemon.
    pub fn read(&self, cx: &mut Context<Self>) {
        perform_later(&self.shell, Command::TravelledFolderRead, cx);
    }

    fn write(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        if self.waiting {
            return;
        }
        self.waiting = true;
        self.problem = None;
        perform_later(&self.shell, Command::TravelledFolderSet { folder }, cx);
        cx.notify();
    }

    /// Whether Save has something to write: a folder typed that is not the one kept.
    pub fn savable(&self, cx: &gpui_kit::App) -> bool {
        let kept = self.shown.as_ref().map(|shown| shown.folder.clone());
        !self.waiting
            && self.shown.is_some()
            && self
                .typed(cx)
                .is_some_and(|typed| !typed.trim().is_empty() && Some(&typed) != kept.as_ref())
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if !self.savable(cx) {
            return;
        }
        let folder = self.typed(cx);
        self.write(folder, cx);
    }

    /// Back to Flottille's data folder.
    pub fn use_default(&mut self, cx: &mut Context<Self>) {
        if self.shown.as_ref().is_some_and(|shown| shown.own.is_some()) {
            self.write(None, cx);
        }
    }

    /// The daemon's answer to a read or a write: what it keeps now, or why not. A refusal is
    /// said under the field, and what is typed stays.
    pub fn answered(
        &mut self,
        said: Result<Shown, String>,
        written: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if written {
            self.waiting = false;
        }
        match said {
            Ok(shown) => {
                self.problem = None;
                if let Some(field) = self.field.clone()
                    && (written || field.read(cx).value().is_empty())
                {
                    field.update(cx, |field, cx| field.set_value(&shown.folder, window, cx));
                }
                self.shown = Some(shown);
            }
            Err(why) if written || self.shown.is_none() => self.problem = Some(why.into()),
            Err(_) => {}
        }
        cx.notify();
    }

    fn make_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let field = cx.new(|cx| {
            let mut field = InputState::new(window, cx).placeholder("~/Projects/from-others");
            follow_theme(&mut field, cx);
            field
        });
        self._subscriptions.push(cx.subscribe_in(
            &field,
            window,
            |this, _, event, _, cx| match event {
                // Save follows what is typed, and a refusal goes once it changes.
                InputEvent::Change => {
                    this.problem = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.save(cx),
                _ => {}
            },
        ));
        if let Some(shown) = &self.shown {
            field.update(cx, |field, cx| field.set_value(&shown.folder, window, cx));
        }
        self.field = Some(field.clone());
        field
    }
}

impl Render for TravelledFolder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let field = match self.field.clone() {
            Some(field) => field,
            None => self.make_field(window, cx),
        };
        let id = |part: &str| ElementId::Name(format!("travelled-folder-{part}").into());
        let known = self.shown.is_some();
        let own = self.shown.as_ref().is_some_and(|shown| shown.own.is_some());

        let frame = div()
            .id(id("path"))
            .h(px(32.))
            .px(space::MD)
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .flex()
            .items_center()
            .mono(12.5)
            .child(Input::new(&field))
            .test_support();
        let save = Button::secondary(id("save"), "Save")
            .disabled(!self.savable(cx))
            .on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        // Save shows once what is typed differs from what is kept (rule 4). A presence is as
        // wide as its parent: the button keeps its own width.
        let save = div()
            .flex_none()
            .child(presence(id("save-block"), self.savable(cx), save));
        // "Use the data folder" shows while a folder of the user's is kept.
        let default = Button::quiet(id("default"), "Use the data folder")
            .disabled(self.waiting)
            .on_click(cx.listener(|this, _, _, cx| this.use_default(cx)));
        let default = div()
            .flex_none()
            .child(presence(id("default-block"), own, default));
        let path = Field::new(
            id("field"),
            "Folder",
            div()
                .flex()
                .items_center()
                .gap(space::SM)
                .child(div().flex_1().min_w_0().child(frame))
                .child(save)
                .child(default),
        );

        let named = |part: &'static str| move || id(part);
        let (problem_id, error_id) = (named("problem"), named("error"));
        let (change_id, made_id) = (named("made-change"), named("made"));
        let problem = presence_of(id("problem-block"), self.problem.clone(), move |problem| {
            div()
                .id(problem_id())
                .aria_label(problem.clone())
                .text_size(px(12.5))
                .text_color(p.fail)
                .child(problem)
                .test_support()
                .into_any_element()
        })
        .space(space::MD);
        let unread = presence_of(
            id("error-block"),
            self.shown
                .as_ref()
                .and_then(|shown| shown.error.clone())
                .map(|error| {
                    SharedString::from(format!(
                        "travelled.toml cannot be followed, so the data folder is used: {error}."
                    ))
                }),
            move |error| {
                div()
                    .id(error_id())
                    .aria_label(error.clone())
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(error)
                    .test_support()
                    .into_any_element()
            },
        )
        .space(space::MD);
        // Where the next one is made, and that a change reaches only those: the line fades when
        // the folder changes.
        let made = self.shown.as_ref().map(|shown| {
            SharedString::from(format!(
                "The next one is made in {}/<project>. {REACH}",
                shown.folder
            ))
        });
        let made = presence_of(id("made-block"), made, move |line| {
            changing(change_id(), line, move |line| {
                div()
                    .id(made_id())
                    .aria_label(line.clone())
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(line)
                    .test_support()
                    .into_any_element()
            })
            .into_any_element()
        })
        .space(space::MD);

        Section::new("travelled-folder", "Projects from other machines")
            .sentence(
                "Where this machine makes a project that comes with another machine's launch.",
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    // A skeleton stands for the field until the daemon says it.
                    .child(awaiting(
                        id("waiting"),
                        !known && self.problem.is_none(),
                        Skeleton::new(id("skeleton")).h(px(56.)),
                        path,
                    ))
                    .child(problem)
                    .child(unread)
                    .child(made),
            )
    }
}
