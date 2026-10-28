//! Adding a project from the window, as `flottille project add` does: its folder (typed, or
//! picked with the system's own dialog), the repositories the daemon detects in it shown before
//! anything is registered, then the project's settings page, where its forges and tracker are
//! declared.

use std::path::PathBuf;

use flottille_ui::{Button, Modal, Type, palette};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::project_settings::{field_frame, styled_input};
use crate::shell::Shell;

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

pub struct AddProject {
    shell: WeakEntity<Shell>,
    open: bool,
    folder: Entity<InputState>,
    name: Entity<InputState>,
    detected: Option<Detected>,
    /// What the daemon is asked, while it answers.
    busy: Option<&'static str>,
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
                    // What was detected is for the folder and name it was asked for.
                    InputEvent::Change => {
                        if this.detected.take().is_some() {
                            cx.notify();
                        }
                    }
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
            busy: None,
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

    /// Enter or the main button: detect the repositories, then add what was detected.
    fn next(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let (folder, name) = self.typed(cx);
        if folder.is_empty() {
            return;
        }
        let command = match &self.detected {
            None => {
                self.busy = Some("Detecting…");
                Command::DetectProject { folder, name }
            }
            Some(detected) => {
                self.busy = Some("Adding…");
                Command::AddProject {
                    folder: detected.folder.clone(),
                    name: Some(detected.name.clone()),
                    repositories: detected.repositories.clone(),
                }
            }
        };
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
        self.detected = None;
        cx.notify();
    }

    /// What the daemon detected in the folder, or why it cannot be registered.
    pub fn detect_answered(&mut self, read: Result<Value, String>, cx: &mut Context<Self>) {
        self.busy = None;
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
            }
            Err(why) => self.error = Some(why.into()),
        }
        cx.notify();
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

impl Render for AddProject {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let detected = self.detected.clone().map(|detected| {
            div()
                .id("add-project-detected")
                .flex()
                .flex_col()
                .gap(px(4.))
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
                        .pl(px(10.))
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
            Some(busy) => div()
                .mono(11.5)
                .text_color(p.faint)
                .child(busy)
                .into_any_element(),
            None => Button::primary(
                "add-project-next",
                if self.detected.is_some() {
                    "Add"
                } else {
                    "Detect"
                },
            )
            .on_click(cx.listener(|this, _, _, cx| this.next(cx)))
            .into_any_element(),
        };
        Modal::new("add-project-modal", "Add a project")
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            })
            .child(div().text_size(px(12.5)).text_color(p.mute).child(
                "A folder that is a Git repository, or holds some one or two levels down. \
                 Nothing is written in it.",
            ))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().flex_1().child(field_frame(
                        "add-project-folder",
                        &self.folder,
                        cx,
                    )))
                    .child(
                        Button::secondary("add-project-browse", "Browse…")
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
            .child(field_frame("add-project-name", &self.name, cx))
            .children(detected)
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("add-project-error")
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error)
                        .test_support(),
                )
            })
            .action(action)
    }
}
