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
    Button, Field, Modal, Select, SelectOption, Type, enter, palette, presence, presence_of, space,
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

/// The folders a machine lists, each one's path and how it is shown, or why they cannot be read.
pub type Listed = Result<Vec<(String, String)>, String>;

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

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.error = None;
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
        let listed = read.map(|read| {
            read["folders"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|folder| {
                    Some((
                        folder["path"].as_str()?.to_owned(),
                        folder["shown"].as_str()?.to_owned(),
                    ))
                })
                .collect()
        });
        self.folders = Some((machine, listed));
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
    /// state. None while the user has no other.
    fn machine_choice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let shell = self.shell.upgrade()?;
        let read = shell.read(cx).machines().read(cx).machines()?.clone();
        if !read.member() || read.runners().next().is_none() {
            return None;
        }
        let this = read
            .this
            .clone()
            .unwrap_or_else(|| "This machine".to_owned());
        let runners: Vec<(String, String)> = read
            .runners()
            .map(|machine| (machine.name.clone(), machine.state_words()))
            .collect();
        let shown = crate::machines::shown_machine(cx);
        let chosen = shown
            .as_ref()
            .and_then(|shown| runners.iter().position(|(name, _)| name == shown))
            .map_or(0, |at| at + 1);
        let options = std::iter::once(SelectOption::new("here", this).line("This machine"))
            .chain(runners.iter().map(|(name, state)| {
                SelectOption::new(
                    SharedString::from(name.clone()),
                    SharedString::from(name.clone()),
                )
                .line(SharedString::from(state.clone()))
            }))
            .collect::<Vec<_>>();
        let names: Vec<Option<String>> = std::iter::once(None)
            .chain(runners.into_iter().map(|(name, _)| Some(name)))
            .collect();
        let weak = self.shell.clone();
        let select =
            Select::new("add-project-machine", options, chosen).on_choose(move |at, _, cx| {
                let name = names[at].clone();
                weak.update(cx, |shell, cx| shell.choose_machine(name, cx))
                    .ok();
            });
        Some(Field::new("add-project-machine-field", "On", select))
    }

    /// The folder: typed or picked with the system's dialog on this machine; picked from the
    /// folders another machine lists on that one, never typed.
    fn folder_choice(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(machine) = crate::machines::shown_machine(cx) else {
            return div()
                .flex()
                .gap(space::SM)
                .child(
                    div()
                        .flex_1()
                        .child(field_frame("add-project-folder", &self.folder, cx)),
                )
                .child(
                    Button::secondary("add-project-browse", "Browse…")
                        .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                )
                .into_any_element();
        };
        let p = palette(cx);
        let listed = match self.folders(cx) {
            None => {
                return div()
                    .id("add-project-folders-reading")
                    .mono(11.5)
                    .text_color(p.faint)
                    .child("Reading its folders…")
                    .test_support()
                    .into_any_element();
            }
            Some(Err(why)) => Err(why.clone()),
            Some(Ok(listed)) => Ok(listed.clone()),
        };
        let none = match &listed {
            Err(why) => Some(format!("Its folders cannot be read: {why}")),
            Ok(listed) if listed.is_empty() => Some(format!(
                "No Git repository in the home folder of {machine}, three levels down."
            )),
            Ok(_) => None,
        };
        let listed = listed.unwrap_or_default();
        let typed = self.folder.read(cx).value().trim().to_owned();
        let chosen = listed
            .iter()
            .position(|(path, _)| *path == typed)
            .map_or(0, |at| at + 1);
        let options = std::iter::once(SelectOption::new("none", "Choose a folder")).chain(
            listed.iter().map(|(_, shown)| {
                SelectOption::new(
                    SharedString::from(shown.clone()),
                    SharedString::from(shown.clone()),
                )
            }),
        );
        let paths: Vec<Option<String>> = std::iter::once(None)
            .chain(listed.iter().map(|(path, _)| Some(path.clone())))
            .collect();
        let this = cx.entity().downgrade();
        let select = Select::new("add-project-folders", options, chosen)
            .disabled(listed.is_empty())
            .on_choose(move |at, window, cx| {
                let Some(path) = paths[at].clone() else {
                    return;
                };
                this.update(cx, |this, cx| {
                    this.set_folder(PathBuf::from(path), window, cx)
                })
                .ok();
            });
        div()
            .flex()
            .flex_col()
            .child(Field::new(
                "add-project-folders-field",
                SharedString::from(format!("Folder on {machine}")),
                select,
            ))
            .child(
                presence_of(
                    "add-project-no-folders",
                    none.map(SharedString::from),
                    move |line| {
                        div()
                            .id("add-project-no-folders")
                            .aria_label(line.clone())
                            .text_size(px(12.5))
                            .text_color(p.faint)
                            .child(line)
                            .test_support()
                            .into_any_element()
                    },
                )
                .space(space::SM),
            )
            .into_any_element()
    }
}

impl Render for AddProject {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = crate::machines::shown_machine(cx);
        if self.drawn_for != shown {
            // A folder of one machine is no folder of another.
            self.drawn_for = shown;
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
        Modal::new("add-project-modal", "Add a project")
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            })
            .child(
                // The body's gap is the kit's: the blocks sit in a column of their own, after the
                // lines that are always there, with the space above them themselves.
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::LG)
                            .child(div().text_size(px(12.5)).text_color(p.mute).child(
                                "A folder that is a Git repository, or holds some one or two \
                                 levels down. Nothing is written in it.",
                            ))
                            .children(self.machine_choice(cx))
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
                    )),
            )
            .action(action)
    }
}
