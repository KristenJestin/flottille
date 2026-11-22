//! Adding a project from the window, as `flottille project add` does: on which machine first, when
//! the user has others that run agents (choosing one shows it, as opening one of its sessions
//! does); its folder (typed, or picked with the system's own dialog on this machine, picked from
//! the folders that machine lists on another: `docs/remote.md`, "Paths are picked, not typed"),
//! the repositories the daemon detects in it shown before anything is registered, then the
//! project's settings page, where its forges and tracker are declared. The folder is detected by
//! itself: once typing rests, or at once for a picked one.

use std::path::PathBuf;
use std::time::Duration;

use gpui_design_system::{
    Button, Field, ListCard, ListRow, Modal, RadioChoices, Select, SelectOption, Type, enter,
    palette, presence, presence_of, space,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::problem_line;
use crate::project_settings::{field_frame, styled_input};
use crate::shell::Shell;

/// How long typing rests before the folder typed is detected.
const DETECT_AFTER: Duration = Duration::from_millis(400);

/// What the daemon would register.
#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub name: String,
    pub folder: String,
    pub repositories: Vec<String>,
}

/// A repository as the modal names it: the folder's own name for the project's root (`.`).
pub fn repository_name(folder: &str, path: &str) -> String {
    match path {
        "." => std::path::Path::new(folder).file_name().map_or_else(
            || folder.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        ),
        path => path.to_owned(),
    }
}

/// A folder of another machine, as `project.folders` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// It is a Git repository itself: it is picked, not opened.
    pub repository: bool,
    pub repositories: usize,
    /// The project already registered there.
    pub project: Option<String>,
}

/// A folder of another machine and what is in it, as `project.folders` says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Browsed {
    pub path: String,
    /// From `~`: `~/code`.
    pub shown: String,
    pub parent: Option<String>,
    pub folders: Vec<Entry>,
}

/// What a machine said of a folder, or why it could not be read.
pub type Listed = Result<Browsed, String>;

/// The step the dialog is at: on which machine, then the folder there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Machine,
    Folder,
}

pub struct AddProject {
    shell: WeakEntity<Shell>,
    open: bool,
    folder: Entity<InputState>,
    name: Entity<InputState>,
    detected: Option<Detected>,
    /// What `detected` was kept after, to fold away with its text still there.
    shown: Option<Detected>,
    /// The folder and name the detection (answered, or on its way) is for.
    asked: Option<(String, Option<String>)>,
    /// Counts the changes of the folder and name; an answer to an older one is dropped.
    generation: u64,
    detecting: bool,
    /// The pause before a typed folder is detected; replaced by the next change.
    pause: Option<Task<()>>,
    /// Adding, while the daemon answers.
    busy: Option<&'static str>,
    /// The folders another machine listed (`project.folders`): the machine they are of, and each
    /// one's path and how it is shown (`~/code/app`), or why they could not be read.
    folders: Option<(Option<String>, Listed)>,
    /// The machine the form was last drawn for: what was typed for another is let go.
    drawn_for: Option<String>,
    step: Step,
    /// Whether the dialog asks on which machine: the user has other machines that run agents.
    asks_machine: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl AddProject {
    pub fn new(shell: WeakEntity<Shell>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let folder = cx.new(|cx| styled_input("its folder, for example /work/app", window, cx));
        let name =
            cx.new(|cx| styled_input("name (optional): the folder's by default", window, cx));
        let subscriptions = [&folder, &name]
            .into_iter()
            .map(|field| {
                cx.subscribe_in(field, window, |this, _, event, _, cx| match event {
                    InputEvent::Change => this.changed(cx),
                    InputEvent::PressEnter { .. } => this.next(cx),
                    _ => {}
                })
            })
            .collect();
        Self {
            shell,
            open: false,
            folder,
            name,
            detected: None,
            shown: None,
            asked: None,
            generation: 0,
            detecting: false,
            pause: None,
            busy: None,
            folders: None,
            drawn_for: None,
            step: Step::Folder,
            asks_machine: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn folder_field(&self) -> &Entity<InputState> {
        &self.folder
    }

    pub fn detected(&self) -> Option<&Detected> {
        self.detected.as_ref()
    }

    /// How many times the folder or name changed: what an answer to a detection says it is for.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The step the dialog is at.
    pub fn step(&self) -> Step {
        self.step
    }

    /// Opens the dialog, on which machine first when `asks_machine`, the user having others.
    pub fn open(&mut self, asks_machine: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.error = None;
        self.asks_machine = asks_machine;
        self.step = if asks_machine {
            Step::Machine
        } else {
            Step::Folder
        };
        let focus = self.folder.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn typed(&self, cx: &Context<Self>) -> (String, Option<String>) {
        let folder = self.folder.read(cx).value().trim().to_owned();
        let name = self.name.read(cx).value().trim().to_owned();
        (folder, (!name.is_empty()).then_some(name))
    }

    /// The folder or the name changed: what was detected is for what it was asked, so it goes,
    /// and the new folder is detected once typing rests.
    fn changed(&mut self, cx: &mut Context<Self>) {
        let typed = self.typed(cx);
        if self.asked.as_ref() == Some(&typed) {
            return;
        }
        self.generation += 1;
        self.asked = None;
        self.detected = None;
        self.error = None;
        self.detecting = false;
        self.pause = None;
        if !typed.0.is_empty() {
            self.pause = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(DETECT_AFTER).await;
                this.update(cx, |this, cx| this.detect(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// Asks the daemon what the folder holds, without waiting for the pause to end.
    fn detect(&mut self, cx: &mut Context<Self>) {
        self.pause = None;
        let typed = self.typed(cx);
        if typed.0.is_empty() || self.asked.as_ref() == Some(&typed) {
            return;
        }
        let (folder, name) = typed.clone();
        self.asked = Some(typed);
        self.detecting = true;
        self.shell
            .update(cx, |shell, _| {
                shell.perform(Command::DetectProject {
                    folder,
                    name,
                    seq: self.generation,
                })
            })
            .ok();
        cx.notify();
    }

    /// Enter or the main button: add what was detected; Enter before it was detects at once.
    fn next(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(detected) = &self.detected else {
            self.detect(cx);
            return;
        };
        let command = Command::AddProject {
            folder: detected.folder.clone(),
            name: Some(detected.name.clone()),
            repositories: detected.repositories.clone(),
        };
        self.busy = Some("Adding…");
        self.error = None;
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// The system's own folder dialog; what is picked fills the field.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add this folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.set_folder(path, window, cx))
                .ok();
        })
        .detach();
    }

    pub fn set_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let text = path.display().to_string();
        self.folder
            .update(cx, |field, cx| field.set_value(text, window, cx));
        self.changed(cx);
        self.detect(cx);
    }

    /// What the daemon detected in the folder, or why it cannot be registered.
    pub fn detect_answered(
        &mut self,
        seq: u64,
        read: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        if seq != self.generation {
            return;
        }
        self.detecting = false;
        match read {
            Ok(found) => {
                self.detected = Some(Detected {
                    name: found["name"].as_str().unwrap_or_default().to_owned(),
                    folder: found["folder"].as_str().unwrap_or_default().to_owned(),
                    repositories: found["repositories"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                });
                self.shown = self.detected.clone();
            }
            Err(why) => self.error = Some(why.into()),
        }
        cx.notify();
    }

    /// The folders `machine` listed, or why not: kept for that machine only.
    pub fn folders_answered(
        &mut self,
        machine: Option<String>,
        read: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let text = |value: &Value| value.as_str().map(str::to_owned);
        let listed = read.map(|read| Browsed {
            path: text(&read["path"]).unwrap_or_default(),
            shown: text(&read["shown"]).unwrap_or_default(),
            parent: text(&read["parent"]),
            folders: read["folders"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|folder| Entry {
                    name: text(&folder["name"]).unwrap_or_default(),
                    path: text(&folder["path"]).unwrap_or_default(),
                    repository: folder["repository"] == true,
                    repositories: folder["repositories"].as_u64().unwrap_or_default() as usize,
                    project: text(&folder["project"]),
                })
                .collect(),
        });
        self.folders = Some((machine, listed));
        cx.notify();
    }

    /// Opens the folder `path` of the machine shown: its folders are listed in place of these.
    fn browse_to(&mut self, path: String, cx: &mut Context<Self>) {
        let machine = crate::machines::shown_machine(cx);
        self.shell
            .update(cx, |shell, _| {
                shell.perform(Command::ProjectFolders {
                    machine,
                    path: Some(path),
                })
            })
            .ok();
    }

    /// From on which machine to the folder there, and back.
    fn go_to_step(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        self.error = None;
        cx.notify();
    }

    /// The folders listed by the machine shown, once it answered.
    pub fn folders(&self, cx: &gpui_kit::App) -> Option<&Listed> {
        let shown = crate::machines::shown_machine(cx);
        self.folders
            .as_ref()
            .filter(|(machine, _)| *machine == shown)
            .map(|(_, listed)| listed)
    }

    /// The project registered: its name, for the shell to open its settings; or why not.
    pub fn add_answered(
        &mut self,
        read: Result<Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.busy = None;
        let added = match read {
            Ok(project) => project["name"].as_str().map(str::to_owned),
            Err(why) => {
                self.error = Some(why.into());
                None
            }
        };
        if added.is_some() {
            self.open = false;
            self.detected = None;
            for field in [&self.folder, &self.name] {
                field.update(cx, |field, cx| field.set_value("", window, cx));
            }
        }
        cx.notify();
        added
    }
}

impl AddProject {
    /// On which machine: this one first, then the other machines that run agents, each with its
    /// role and state; choosing one shows it, as opening one of its sessions does.
    fn machine_choice(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let p = palette(cx);
        let read = self
            .shell
            .upgrade()
            .and_then(|shell| shell.read(cx).machines().read(cx).machines().cloned())
            .unwrap_or_default();
        let choices = crate::machines::choices(&read);
        let shown = crate::machines::shown_machine(cx);
        let chosen = choices
            .iter()
            .position(|choice| choice.name == shown)
            .unwrap_or(0);
        let names: Vec<Option<String>> = choices.iter().map(|choice| choice.name.clone()).collect();
        let weak = self.shell.clone();
        let choose = move |at: usize, _: &mut Window, cx: &mut gpui_kit::App| {
            let name = names[at].clone();
            weak.update(cx, |shell, cx| shell.choose_machine(name, cx))
                .ok();
        };
        let control = if choices.len() <= gpui_design_system::radio::MAX_RADIO_CHOICES {
            choices
                .iter()
                .fold(
                    RadioChoices::new("add-project-machine", "On which machine?"),
                    |radios, choice| {
                        radios
                            .choice(choice.key(), choice.title.clone(), choice.line())
                            .leading(crate::glyphs::device(&choice.icon))
                    },
                )
                .chosen(chosen)
                .on_choose(choose)
                .into_any_element()
        } else {
            let options = choices
                .iter()
                .map(|choice| {
                    SelectOption::new(choice.key(), choice.title.clone()).line(choice.line())
                })
                .collect::<Vec<_>>();
            Select::new("add-project-machine", options, chosen)
                .on_choose(choose)
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(Field::new(
                "add-project-machine-field",
                "On which machine?",
                control,
            ))
            .child(
                div().text_size(px(12.5)).text_color(p.mute).child(
                    "The project lives in that machine's folders, and its sessions run there.",
                ),
            )
            .into_any_element()
    }

    /// The folder: typed or picked with the system's dialog on this machine; on another, picked in
    /// that machine's folders, opened one after the other, never typed.
    fn folder_choice(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(machine) = crate::machines::shown_machine(cx) else {
            return div()
                .flex()
                .flex_col()
                .gap(space::LG)
                .child(div().text_size(px(12.5)).text_color(palette(cx).mute).child(
                    "A folder that is a Git repository, or holds some one or two levels down. \
                     Nothing is written in it.",
                ))
                .child(
                    div()
                        .flex()
                        .gap(space::SM)
                        .child(div().flex_1().child(field_frame(
                            "add-project-folder",
                            &self.folder,
                            cx,
                        )))
                        .child(
                            Button::secondary("add-project-browse", "Browse…").on_click(
                                cx.listener(|this, _, window, cx| this.browse(window, cx)),
                            ),
                        ),
                )
                .into_any_element();
        };
        let p = palette(cx);
        let intro = div()
            .text_size(px(12.5))
            .text_color(p.mute)
            .child(SharedString::from(format!(
                "Folders of {machine}. Paths are picked, never typed."
            )));
        let listed = match self.folders(cx) {
            None => {
                return div()
                    .flex()
                    .flex_col()
                    .gap(space::LG)
                    .child(intro)
                    .child(
                        div()
                            .id("add-project-folders-reading")
                            .mono(11.5)
                            .text_color(p.faint)
                            .child("Reading its folders…")
                            .test_support(),
                    )
                    .into_any_element();
            }
            Some(listed) => listed.clone(),
        };
        let browsed = match listed {
            Ok(browsed) => browsed,
            Err(why) => {
                return div()
                    .flex()
                    .flex_col()
                    .gap(space::LG)
                    .child(intro)
                    .child(problem_line(
                        "add-project-folders-error",
                        Some(SharedString::from(format!(
                            "Its folders cannot be read: {why}"
                        ))),
                        px(0.),
                        cx,
                    ))
                    .into_any_element();
            }
        };
        let typed = self.folder.read(cx).value().trim().to_owned();
        let this = cx.entity().downgrade();
        let mut list = ListCard::new("add-project-folders");
        if let Some(parent) = browsed.parent.clone() {
            let this = this.clone();
            list = list.row(
                ListRow::new("add-project-up", "..")
                    .mono()
                    .line("The folder above")
                    .on_click(move |_, _, cx| {
                        let parent = parent.clone();
                        this.update(cx, |this, cx| this.browse_to(parent, cx)).ok();
                    }),
            );
        }
        for entry in &browsed.folders {
            let said = match (&entry.project, entry.repository, entry.repositories) {
                (Some(_), _, _) => Some("already a project".to_owned()),
                (None, true, 1) => Some("a repository".to_owned()),
                (None, true, n) => Some(format!("{n} repositories")),
                (None, false, _) => None,
            };
            let mut row = ListRow::new(
                ElementId::Name(format!("add-project-folder-{}", entry.name).into()),
                SharedString::from(entry.name.clone()),
            )
            .mono()
            .selected(entry.path == typed);
            if let Some(said) = said {
                row = row.line(SharedString::from(said));
            }
            // A repository is picked; any other folder is opened; a project's is neither.
            if entry.project.is_none() {
                let (this, path, pick) = (this.clone(), entry.path.clone(), entry.repository);
                row = row.on_click(move |_, window, cx| {
                    let path = path.clone();
                    this.update(cx, |this, cx| match pick {
                        true => this.set_folder(PathBuf::from(path), window, cx),
                        false => this.browse_to(path, cx),
                    })
                    .ok();
                });
            }
            list = list.row(row);
        }
        let empty = browsed
            .folders
            .is_empty()
            .then(|| SharedString::from(format!("{} holds no folder.", browsed.shown)));
        let path = SharedString::from(format!("{machine}: {}", browsed.shown));
        div()
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(intro)
            .child(
                div()
                    .id("add-project-path")
                    .aria_label(path.clone())
                    .mono(12.)
                    .text_color(p.faint)
                    .child(path)
                    .test_support(),
            )
            // Another folder opened: its list rises in where the last one was.
            .child(enter(
                ElementId::Name(format!("add-project-list-{}", browsed.path).into()),
                0,
                div()
                    .child(list)
                    .child(presence_of("add-project-empty", empty, move |line| {
                        div()
                            .id("add-project-empty")
                            .aria_label(line.clone())
                            .text_size(px(12.5))
                            .text_color(p.faint)
                            .child(line)
                            .test_support()
                            .into_any_element()
                    })),
            ))
            .into_any_element()
    }
}

impl Render for AddProject {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = crate::machines::shown_machine(cx);
        if self.drawn_for != shown {
            // A folder of one machine is no folder of another.
            self.drawn_for = shown.clone();
            for field in [&self.folder, &self.name] {
                field.update(cx, |field, cx| field.set_value("", window, cx));
            }
            self.changed(cx);
        }
        let p = palette(cx);
        let this = cx.entity().downgrade();
        // Folding away, the result still shows what it held.
        let detected = self.shown.clone().map(|detected| {
            div()
                .id("add-project-detected")
                .flex()
                .flex_col()
                .gap(space::XS)
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(SharedString::from(format!(
                    "{} in {}, with {} {}:",
                    detected.name,
                    detected.folder,
                    detected.repositories.len(),
                    if detected.repositories.len() == 1 {
                        "repository"
                    } else {
                        "repositories"
                    }
                )))
                .children(detected.repositories.iter().enumerate().map(|(i, path)| {
                    let name = SharedString::from(repository_name(&detected.folder, path));
                    div()
                        .id(ElementId::Name(
                            format!("add-project-repository-{i}").into(),
                        ))
                        .aria_label(name.clone())
                        .pl(space::MD)
                        .mono(12.)
                        .text_color(p.text)
                        .child(name)
                        .test_support()
                }))
                .child(
                    div()
                        .text_color(p.faint)
                        .child("Its forges and tracker are set next, in its settings."),
                )
                .test_support()
        });
        let action = match self.busy {
            Some(busy) => enter(
                "add-project-busy",
                0,
                div().mono(11.5).text_color(p.faint).child(busy),
            )
            .into_any_element(),
            None => Button::primary("add-project-next", "Add project")
                .disabled(self.detected.is_none() || crate::machines::held(cx).is_some())
                .on_click(cx.listener(|this, _, _, cx| this.next(cx)))
                .into_any_element(),
        };
        let title = match (self.step, &shown) {
            (Step::Folder, Some(machine)) => format!("Add a project on {machine}"),
            _ => "Add a project".to_owned(),
        };
        let modal = Modal::new("add-project-modal", SharedString::from(title))
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        if self.step == Step::Machine {
            // Each step enters as it is shown, where the other was.
            return modal
                .child(enter(
                    "add-project-step-machine",
                    0,
                    self.machine_choice(cx),
                ))
                .action(
                    Button::primary("add-project-step-next", "Next: pick the folder")
                        .on_click(cx.listener(|this, _, _, cx| this.go_to_step(Step::Folder, cx))),
                );
        }
        let body = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::LG)
                    .child(self.folder_choice(cx))
                    .child(field_frame("add-project-name", &self.name, cx)),
            )
            .child(
                presence(
                    "add-project-looking",
                    self.detecting,
                    div()
                        .id("add-project-looking")
                        .mono(11.5)
                        .text_color(p.faint)
                        .child("Looking…")
                        .test_support(),
                )
                .space(space::LG),
            )
            .child(
                presence(
                    "add-project-found",
                    self.detected.is_some(),
                    div().children(detected),
                )
                .space(space::LG),
            )
            .child(problem_line(
                "add-project-error",
                self.error.clone(),
                space::LG,
                cx,
            ));
        let modal = modal.child(enter("add-project-step-folder", 0, body));
        let modal = match self.asks_machine {
            true => modal.action(
                Button::secondary("add-project-back", "Back")
                    .on_click(cx.listener(|this, _, _, cx| this.go_to_step(Step::Machine, cx))),
            ),
            false => modal,
        };
        modal.action(action)
    }
}
