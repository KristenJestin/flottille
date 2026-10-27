//! A project's references on its page (`docs/projects.md`, "References"): documents an agent reads
//! with a tool when its task needs them. The window writes nothing itself: an add and a removal
//! are the daemon's (`reference.add`, `reference.remove`), and the section shows the list the
//! daemon gives next.
//!
//! The path of a new reference is picked inside the project folder: the modal lists the folder's
//! entries, a click on a folder opens it and picks it, a click on a file picks it. The daemon has
//! the last word on the path (it must stay inside the folder, once links are followed).

use std::path::{Path, PathBuf};

use flottille_ui::{Button, Icon, Modal, Type, palette};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _,
    WeakEntity, Window, div, px,
};

use crate::actions::Command;
use crate::procedures::{boxed, heading, line_field, perform_later};
use crate::shell::Shell;

/// How many entries of a folder the picker lists.
const LISTED: usize = 200;

/// A reference as the daemon lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    pub name: String,
    pub description: String,
    pub path: String,
}

impl Reference {
    pub fn of(value: &serde_json::Value) -> Self {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        Self {
            name: text("name"),
            description: text("description"),
            path: text("path"),
        }
    }
}

/// An entry of the folder being browsed.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub folder: bool,
}

/// The entries of `dir`, folders first, by name; `.git` is never offered.
pub fn entries(dir: &Path) -> Vec<Entry> {
    let mut found: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name != ".git").then(|| Entry {
                folder: entry.path().is_dir(),
                name,
            })
        })
        .collect();
    found.sort_by(|a, b| (!a.folder, &a.name).cmp(&(!b.folder, &b.name)));
    found.truncate(LISTED);
    found
}

struct Fields {
    name: Entity<InputState>,
    description: Entity<InputState>,
    path: Entity<InputState>,
}

pub struct ReferencesSection {
    shell: WeakEntity<Shell>,
    project: String,
    /// The project's folder, as the page knows it.
    folder: String,
    listed: Option<Vec<Reference>>,
    problem: Option<SharedString>,
    open: bool,
    fields: Option<Fields>,
    /// The folder the picker shows, relative to the project's.
    browsing: PathBuf,
    saving: bool,
    form_error: Option<SharedString>,
    confirming: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ReferencesSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            folder: String::new(),
            listed: None,
            problem: None,
            open: false,
            fields: None,
            browsing: PathBuf::new(),
            saving: false,
            form_error: None,
            confirming: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn references(&self) -> Option<&[Reference]> {
        self.listed.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn form_error(&self) -> Option<&str> {
        self.form_error.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn name_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.name)
    }

    pub fn description_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.description)
    }

    pub fn path_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.path)
    }

    pub fn set_folder(&mut self, folder: &str) {
        if self.folder != folder {
            self.folder = folder.to_owned();
        }
    }

    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::References { project }, cx);
    }

    pub fn listed(&mut self, listed: Result<Vec<Reference>, String>, cx: &mut Context<Self>) {
        match listed {
            Ok(list) => {
                self.problem = None;
                if self.listed.as_ref() != Some(&list) {
                    self.listed = Some(list);
                }
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    /// Opens the modal, empty, the caret in the name.
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fields.is_none() {
            let name = line_field("name, for example react", window, cx);
            let description =
                line_field("one line: when the document is worth reading", window, cx);
            let path = line_field("path in the project, for example docs/react", window, cx);
            for input in [&name, &description, &path] {
                self._subscriptions
                    .push(cx.subscribe(input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(Fields {
                name,
                description,
                path,
            });
        }
        let fields = self.fields.as_ref().expect("made above");
        for field in [&fields.name, &fields.description, &fields.path] {
            field.update(cx, |f, cx| f.set_value("", window, cx));
        }
        let focus = fields.name.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.browsing = PathBuf::new();
        self.open = true;
        self.saving = false;
        self.form_error = None;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.form_error = None;
        cx.notify();
    }

    /// The path picked in the field, relative to the project's folder.
    pub fn picked(&self, cx: &gpui_kit::App) -> String {
        self.fields
            .as_ref()
            .map(|f| f.path.read(cx).value().trim().to_owned())
            .unwrap_or_default()
    }

    /// A click on an entry of the picker: a folder is opened and picked, a file is picked.
    pub fn pick(&mut self, name: &str, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        let relative = self.browsing.join(name);
        if folder {
            self.browsing = relative.clone();
        }
        if let Some(fields) = &self.fields {
            let text = relative.to_string_lossy().into_owned();
            fields
                .path
                .update(cx, |f, cx| f.set_value(&text, window, cx));
        }
        cx.notify();
    }

    /// The picker goes up one folder; the folder it leaves is no longer the pick.
    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browsing.pop();
        if let Some(fields) = &self.fields {
            let text = self.browsing.to_string_lossy().into_owned();
            fields
                .path
                .update(cx, |f, cx| f.set_value(&text, window, cx));
        }
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.fields else {
            return;
        };
        let command = Command::ReferenceAdd {
            project: self.project.clone(),
            name: fields.name.read(cx).value().trim().to_owned(),
            description: fields.description.read(cx).value().trim().to_owned(),
            path: fields.path.read(cx).value().trim().to_owned(),
        };
        self.saving = true;
        self.form_error = None;
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// Asks to remove; the second click confirms.
    pub fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(name) {
            self.confirming = Some(name.to_owned());
            cx.notify();
            return;
        }
        self.confirming = None;
        self.problem = None;
        let command = Command::ReferenceRemove {
            project: self.project.clone(),
            name: name.to_owned(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to an add or a removal.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        match refused {
            None => {
                self.open = false;
                self.form_error = None;
                self.read(cx);
            }
            Some(why) if !self.open => self.problem = Some(why.into()),
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn row(&self, reference: &Reference, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let name = reference.name.clone();
        let id = |part: &str| ElementId::Name(format!("{part}-{}", reference.name).into());
        let confirming = self.confirming.as_deref() == Some(&reference.name);
        div()
            .id(id("reference"))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .flex()
            .items_start()
            .gap(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .items_baseline()
                            .child(
                                div()
                                    .mono(12.5)
                                    .semibold()
                                    .child(SharedString::from(name.clone())),
                            )
                            .child(
                                div()
                                    .id(id("reference-path"))
                                    .aria_label(SharedString::from(reference.path.clone()))
                                    .mono(11.5)
                                    .text_color(p.faint)
                                    .child(SharedString::from(reference.path.clone()))
                                    .test_support(),
                            ),
                    )
                    .child(
                        div()
                            .id(id("reference-description"))
                            .aria_label(SharedString::from(reference.description.clone()))
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(SharedString::from(reference.description.clone()))
                            .test_support(),
                    ),
            )
            .child(if confirming {
                let (cancel, remove) = (name.clone(), name.clone());
                div()
                    .flex()
                    .gap(px(4.))
                    .child(
                        Button::quiet(id("cancel-remove-reference"), "Cancel").on_click(
                            cx.listener(move |this, _, _, cx| {
                                if this.confirming.as_deref() == Some(&cancel) {
                                    this.confirming = None;
                                    cx.notify();
                                }
                            }),
                        ),
                    )
                    .child(
                        Button::secondary(id("confirm-remove-reference"), format!("Remove {name}"))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx))),
                    )
                    .into_any_element()
            } else {
                let remove = name.clone();
                Button::quiet(id("remove-reference"), "Remove")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx)))
                    .into_any_element()
            })
            .test_support()
    }

    fn modal(&mut self, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let modal = Modal::new("reference-modal", "Add a reference")
            .width(px(520.))
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let Some(fields) = self.fields.as_ref().filter(|_| self.open) else {
            return modal;
        };
        let here = Path::new(&self.folder).join(&self.browsing);
        let listed = entries(&here);
        let location = if self.browsing.as_os_str().is_empty() {
            "the project folder".to_owned()
        } else {
            self.browsing.to_string_lossy().into_owned()
        };
        let picker = div()
            .id("reference-picker")
            .max_h(px(180.))
            .overflow_y_scroll()
            .rounded(px(8.))
            .border_1()
            .border_color(p.line)
            .flex()
            .flex_col()
            .child(
                div()
                    .id("reference-location")
                    .px(px(10.))
                    .py(px(5.))
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(location))
                    .test_support(),
            )
            .when(!self.browsing.as_os_str().is_empty(), |d| {
                d.child(
                    div()
                        .id("reference-up")
                        .px(px(10.))
                        .py(px(4.))
                        .mono(12.)
                        .text_color(p.mute)
                        .hover(|d| d.bg(p.hover))
                        .child("..")
                        .on_click(cx.listener(|this, _, window, cx| this.up(window, cx)))
                        .test_support(),
                )
            })
            .children(listed.into_iter().map(|entry| {
                let name = entry.name.clone();
                div()
                    .id(ElementId::Name(
                        format!("reference-entry-{}", entry.name).into(),
                    ))
                    .px(px(10.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .mono(12.)
                    .hover(|d| d.bg(p.hover))
                    .child(
                        flottille_ui::icon(if entry.folder {
                            Icon::Folder
                        } else {
                            Icon::Repository
                        })
                        .size(px(12.))
                        .text_color(p.faint),
                    )
                    .child(SharedString::from(if entry.folder {
                        format!("{}/", entry.name)
                    } else {
                        entry.name.clone()
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick(&name, entry.folder, window, cx)
                    }))
                    .test_support()
            }));
        modal
            .child(boxed("reference-name", Input::new(&fields.name), cx))
            .child(boxed(
                "reference-description-field",
                Input::new(&fields.description),
                cx,
            ))
            .child(boxed("reference-path-field", Input::new(&fields.path), cx))
            .child(picker)
            .when_some(self.form_error.clone(), |m, error| {
                m.child(
                    div()
                        .id("reference-error")
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error)
                        .test_support(),
                )
            })
            .action(if self.saving {
                div()
                    .mono(11.5)
                    .text_color(p.faint)
                    .child("Saving…")
                    .into_any_element()
            } else {
                Button::primary("save-reference", "Add")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    .into_any_element()
            })
    }
}

impl Render for ReferencesSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let rows: Vec<_> = self
            .listed
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|reference| self.row(reference, cx).into_any_element())
            .collect();
        let count = rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(cx);
        div()
            .id("references")
            .child(heading(
                "References",
                count,
                Button::secondary("add-reference", "Add reference")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(rows)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .id("no-references")
                                .text_size(px(12.5))
                                .text_color(p.faint)
                                .child(
                                    "No reference yet: add a document your agents read when \
                                     their task needs it.",
                                )
                                .test_support(),
                        )
                    })
                    .when_some(self.problem.clone(), |d, problem| {
                        d.child(
                            div()
                                .id("references-problem")
                                .text_size(px(12.5))
                                .text_color(p.fail)
                                .child(problem)
                                .test_support(),
                        )
                    }),
            )
            .child(modal)
    }
}
