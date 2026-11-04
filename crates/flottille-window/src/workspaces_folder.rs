//! Where workspaces go, chosen in the window (`docs/projects.md`, "Where the workspaces go"): the
//! "Workspaces" page of Settings for every project, and the "Workspaces folder" section of a
//! project's own Settings. The window writes nothing itself: a choice is the daemon's
//! (`workspaces_folder.set`), and the section shows what the daemon says next.
//!
//! Only the workspaces made after a change follow it: the section says so.

use flottille_ui::{
    Button, Field, RadioChoices, Section, Type as _, changing, palette, presence, presence_of,
    space,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::{editor_style, perform_later};
use crate::shell::Shell;

/// What the section says of which workspaces a change reaches.
pub const REACH: &str = "A change applies to the workspaces made from now on. The ones that exist \
                         stay where they are: nothing is moved.";

/// The three kinds of default, and for a project the two kinds of choice (`Data` and `Beside`
/// are not offered to it: it follows the default, or has a folder of its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Data,
    Beside,
    Own,
}

impl Place {
    fn word(self) -> &'static str {
        match self {
            Place::Data => "data",
            Place::Beside => "beside",
            Place::Own => "own",
        }
    }
}

/// What `workspaces_folder.show` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The default's place; for a project, `Own` when its file has a folder, else `Data`.
    pub place: Place,
    /// The folder as typed: the default's, or the project's own.
    pub path: Option<String>,
    /// Why the default's file cannot be followed.
    pub error: Option<String>,
    /// A project's: the folder its workspaces are made in now.
    pub folder: Option<String>,
    /// A project's: the folder the default gives it.
    pub default_folder: Option<String>,
}

impl Shown {
    pub fn of(value: &Value, project: bool) -> Shown {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        let place = if project {
            if value["own"].is_string() {
                Place::Own
            } else {
                Place::Data
            }
        } else {
            match value["place"].as_str() {
                Some("beside") => Place::Beside,
                Some("own") => Place::Own,
                _ => Place::Data,
            }
        };
        Shown {
            place,
            path: if project { text("own") } else { text("path") },
            error: text("error"),
            folder: text("folder"),
            default_folder: text("default_folder"),
        }
    }
}

pub struct WorkspacesFolder {
    shell: WeakEntity<Shell>,
    /// The project it sets the folder of; the default for every project when none.
    project: Option<String>,
    /// Made when first drawn, since a text field needs a window.
    field: Option<Entity<InputState>>,
    shown: Option<Shown>,
    /// The kind chosen, until the daemon answers: "a folder of my own" is chosen before its path
    /// is typed.
    chosen: Option<Place>,
    /// A write waits for the daemon.
    waiting: bool,
    /// Why the last write was refused, under the field.
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl WorkspacesFolder {
    pub fn new(shell: WeakEntity<Shell>, project: Option<String>) -> Self {
        Self {
            shell,
            project,
            field: None,
            shown: None,
            chosen: None,
            waiting: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
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

    /// The kind the radio shows: the one chosen, else the daemon's.
    pub fn place(&self) -> Place {
        self.chosen
            .or(self.shown.as_ref().map(|shown| shown.place))
            .unwrap_or(Place::Data)
    }

    /// The folder this section results in for a project, as the daemon last said it.
    pub fn folder(&self) -> Option<&str> {
        self.shown
            .as_ref()
            .and_then(|shown| shown.folder.as_deref())
    }

    /// The text in the field.
    pub fn typed(&self, cx: &gpui_kit::App) -> Option<String> {
        self.field
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
    }

    /// Asks the daemon.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::WorkspacesFolderRead { project }, cx);
    }

    fn write(&mut self, place: Place, path: Option<String>, cx: &mut Context<Self>) {
        if self.waiting {
            return;
        }
        self.waiting = true;
        self.problem = None;
        let command = match &self.project {
            // A project follows the default, or has its own folder.
            Some(project) => Command::WorkspacesFolderSet {
                project: Some(project.clone()),
                place: None,
                path: (place == Place::Own).then_some(path).flatten(),
            },
            None => Command::WorkspacesFolderSet {
                project: None,
                place: Some(place.word().to_owned()),
                path,
            },
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// A click on a kind: "a folder of my own" shows its field first; the others are kept at once.
    pub fn choose(&mut self, place: Place, cx: &mut Context<Self>) {
        if self.waiting || self.shown.is_none() || self.place() == place {
            return;
        }
        self.problem = None;
        if place == Place::Own {
            self.chosen = Some(Place::Own);
            cx.notify();
        } else {
            self.chosen = Some(place);
            self.write(place, None, cx);
        }
    }

    /// Whether Save has something to write: a path typed that is not the one kept.
    pub fn savable(&self, cx: &gpui_kit::App) -> bool {
        let kept = self.shown.as_ref().and_then(|shown| shown.path.clone());
        self.place() == Place::Own
            && !self.waiting
            && self
                .typed(cx)
                .is_some_and(|typed| !typed.trim().is_empty() && Some(&typed) != kept.as_ref())
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if !self.savable(cx) {
            return;
        }
        let path = self.typed(cx);
        self.write(Place::Own, path, cx);
    }

    /// The daemon's answer to a read or a write: what it keeps now, or why not. A refusal is
    /// said under the field and the choice goes back to what is kept.
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
                // A kind chosen but not yet saved ("my own", its path to type) is kept on a read.
                if written || self.chosen != Some(Place::Own) || shown.place == Place::Own {
                    self.chosen = None;
                }
                if let Some(field) = self.field.clone()
                    && (written || field.read(cx).value().is_empty())
                    && let Some(path) = shown.path.clone()
                {
                    field.update(cx, |field, cx| field.set_value(&path, window, cx));
                }
                self.shown = Some(shown);
            }
            Err(why) if written => {
                self.problem = Some(why.into());
                // The kind goes back, unless a folder of one's own is being typed.
                if self.chosen != Some(Place::Own) {
                    self.chosen = None;
                }
            }
            Err(why) if self.shown.is_none() => self.problem = Some(why.into()),
            Err(_) => {}
        }
        cx.notify();
    }

    fn make_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let field = cx.new(|cx| {
            let mut field = InputState::new(window, cx).placeholder("~/Worktrees/<project>");
            field.set_editor_style(editor_style(cx));
            field
        });
        self._subscriptions.push(cx.subscribe_in(
            &field,
            window,
            |this, _, event, _, cx| match event {
                // Save follows what is typed.
                InputEvent::Change => {
                    this.problem = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.save(cx),
                _ => {}
            },
        ));
        if let Some(path) = self.shown.as_ref().and_then(|shown| shown.path.clone()) {
            field.update(cx, |field, cx| field.set_value(&path, window, cx));
        }
        self.field = Some(field.clone());
        field
    }
}

impl Render for WorkspacesFolder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let field = match self.field.clone() {
            Some(field) => field,
            None => self.make_field(window, cx),
        };
        let scope = self.project.clone().unwrap_or_else(|| "default".to_owned());
        let id = |part: &str| ElementId::Name(format!("workspaces-{part}-{scope}").into());
        let named = |part: &'static str| {
            let scope = scope.clone();
            move || ElementId::Name(format!("workspaces-{part}-{scope}").into())
        };
        let project = self.project.is_some();
        let known = self.shown.is_some();
        let place = self.place();

        let this = cx.entity().downgrade();
        let mut kinds = RadioChoices::new(id("choice"), "Workspaces folder");
        let order: &[Place] = if project {
            &[Place::Data, Place::Own]
        } else {
            &[Place::Data, Place::Beside, Place::Own]
        };
        for kind in order {
            kinds = match (kind, project) {
                (Place::Data, true) => kinds.choice(
                    "data",
                    "Use the default",
                    "The folder set for every project in Settings.",
                ),
                (Place::Data, false) => kinds.choice(
                    "data",
                    "Flottille's data folder",
                    "~/.local/share/flottille/workspaces/<project>, where they are made today.",
                ),
                (Place::Beside, _) => kinds.choice(
                    "beside",
                    "Beside the project",
                    "<parent of the project folder>/<project>-work.",
                ),
                (Place::Own, true) => kinds.choice(
                    "own",
                    "A folder of its own",
                    "Where this project's workspaces go, whatever the default is.",
                ),
                (Place::Own, false) => kinds.choice(
                    "own",
                    "A folder of my own",
                    "A path where <project> is replaced by the project's name and ~ is your home.",
                ),
            };
        }
        let chosen = order.iter().position(|kind| *kind == place).unwrap_or(0);
        let kinds = kinds.chosen(chosen).on_choose(move |index, _, cx| {
            let kind = order[index];
            this.update(cx, |this, cx| this.choose(kind, cx)).ok();
        });

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
        let save = Button::primary(id("save"), "Save")
            .disabled(!self.savable(cx))
            .on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        // The field unfolds and fades in when "a folder of my own" is chosen, and folds away when
        // another is.
        let path = presence(
            id("path-block"),
            place == Place::Own,
            Field::new(
                id("path-field"),
                "Folder",
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(div().flex_1().min_w_0().child(frame))
                    .child(save),
            ),
        )
        .gap(space::MD);

        let (problem_id, error_id) = (named("problem"), named("error"));
        let (folder_change_id, folder_id) = (named("folder-change"), named("folder"));
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
        .gap(space::MD);
        let unread = presence_of(
            id("error-block"),
            self.shown
                .as_ref()
                .and_then(|shown| shown.error.clone())
                .map(|error| {
                    SharedString::from(format!(
                        "workspaces.toml cannot be followed, so the data folder is used: {error}."
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
        .gap(space::MD);

        // The folder that results, for a project: its line fades when it changes.
        let resulting = self
            .folder()
            .map(|folder| SharedString::from(format!("Its workspaces are made in {folder}")));
        let resulting = presence_of(id("folder-block"), resulting, move |line| {
            changing(folder_change_id(), line, move |line| {
                div()
                    .id(folder_id())
                    .aria_label(line.clone())
                    .mono(12.5)
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(line)
                    .test_support()
                    .into_any_element()
            })
            .into_any_element()
        })
        .gap(space::MD);

        let (title, sentence) = if project {
            (
                "Workspaces folder",
                "Where this project's new workspaces are made. It follows the default for every \
                 project unless it has a folder of its own.",
            )
        } else {
            (
                "Workspaces folder",
                "Where the new workspaces of every project are made, unless a project has a \
                 folder of its own.",
            )
        };
        let section = Section::new(
            // The global page's tab is "workspaces", and its section is named by its tab.
            if project {
                format!("workspaces-section-{scope}")
            } else {
                "workspaces".to_owned()
            },
            title,
        )
        .sentence(format!("{sentence} {REACH}"))
        .child(presence(
            id("asking"),
            !known && self.problem.is_none(),
            div()
                .text_size(px(12.5))
                .text_color(p.faint)
                .child("Asking the daemon…"),
        ))
        .child(Field::new(id("choice-field"), "Workspaces folder", kinds))
        .child(path)
        .child(problem)
        .child(unread);
        if project {
            section.child(resulting)
        } else {
            section
        }
    }
}
