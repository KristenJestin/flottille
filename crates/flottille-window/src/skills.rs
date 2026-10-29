//! A project's skills in its Settings (`docs/skills.md`): texts an agent reads with a tool when its
//! task needs them, each a path of the project or a text Flottille keeps. The window writes
//! nothing itself: an add, an edit and a removal are the daemon's (`skill.*`), and the section
//! shows the list the daemon gives next.
//!
//! The path of a skill is picked inside the project folder: the modal lists the folder's entries,
//! a click on a folder opens it and picks it, a click on a file picks it. The daemon has the last
//! word on the path (it must stay inside the folder, once links are followed).

use std::path::{Path, PathBuf};

use flottille_ui::{Button, Icon, Modal, Switch, Type, palette};
use gpui_kit::base::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Command, SkillWrite};
use crate::commands::{boxed, editor_style, heading, line_field, perform_later};
use crate::shell::Shell;

/// How many entries of a folder the picker lists.
const LISTED: usize = 200;

/// What the switch of the form says it switches.
const AS_TEXT: &str = "A text Flottille keeps, rather than a file of the project";

/// A skill as the daemon lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// `None` for a text Flottille keeps.
    pub path: Option<String>,
    /// A text skill's text, when the daemon gave it whole (`skill.show`).
    pub text: String,
}

impl SkillInfo {
    pub fn of(value: &serde_json::Value) -> Self {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        Self {
            name: text("name"),
            description: text("description"),
            path: value["path"].as_str().map(str::to_owned),
            text: text("text"),
        }
    }

    /// What the list shows of where it is.
    pub fn place(&self) -> String {
        self.path.clone().unwrap_or_else(|| "text".into())
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
    text: Entity<TextareaState>,
}

pub struct SkillsSection {
    shell: WeakEntity<Shell>,
    project: String,
    /// The project's folder, as the page knows it.
    folder: String,
    listed: Option<Vec<SkillInfo>>,
    problem: Option<SharedString>,
    open: bool,
    /// The skill being edited; `None` for a new one.
    editing: Option<String>,
    /// A text Flottille keeps, rather than a path of the project.
    as_text: bool,
    /// The skill being read to be edited.
    reading: Option<String>,
    fields: Option<Fields>,
    /// The folder the picker shows, relative to the project's.
    browsing: PathBuf,
    saving: bool,
    form_error: Option<SharedString>,
    confirming: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SkillsSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            folder: String::new(),
            listed: None,
            problem: None,
            open: false,
            editing: None,
            as_text: false,
            reading: None,
            fields: None,
            browsing: PathBuf::new(),
            saving: false,
            form_error: None,
            confirming: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn skills(&self) -> Option<&[SkillInfo]> {
        self.listed.as_deref()
    }

    /// Whether the form writes a text Flottille keeps rather than a path.
    pub fn is_text(&self) -> bool {
        self.as_text
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

    pub fn text_field(&self) -> Option<&Entity<TextareaState>> {
        self.fields.as_ref().map(|f| &f.text)
    }

    pub fn set_folder(&mut self, folder: &str) {
        if self.folder != folder {
            self.folder = folder.to_owned();
        }
    }

    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::Skills { project }, cx);
    }

    pub fn listed(&mut self, listed: Result<Vec<SkillInfo>, String>, cx: &mut Context<Self>) {
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

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.fields.is_none() {
            let name = line_field("name, for example react", window, cx);
            let description = line_field("one line: when it is worth reading", window, cx);
            let path = line_field("path in the project, for example docs/react", window, cx);
            let text = cx.new(|cx| {
                let mut field = TextareaState::new(window, cx)
                    .auto_grow(6, 14)
                    .placeholder("the text, in Markdown: what the agent reads");
                field.set_editor_style(editor_style(cx));
                field
            });
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
                text,
            });
        }
        self.fields.as_ref().expect("made above")
    }

    /// Opens the modal, empty, the caret in the name.
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fill(None, window, cx);
    }

    /// Opens the modal on `info`, or empty for a new skill.
    fn fill(&mut self, info: Option<&SkillInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(window, cx);
        let (name, description, path, text) = (
            fields.name.clone(),
            fields.description.clone(),
            fields.path.clone(),
            fields.text.clone(),
        );
        let value = |f: fn(&SkillInfo) -> String| info.map(f).unwrap_or_default();
        name.update(cx, |f, cx| {
            f.set_value(value(|i| i.name.clone()), window, cx)
        });
        description.update(cx, |f, cx| {
            f.set_value(value(|i| i.description.clone()), window, cx)
        });
        path.update(cx, |f, cx| {
            f.set_value(value(|i| i.path.clone().unwrap_or_default()), window, cx)
        });
        text.update(cx, |f, cx| {
            f.set_value(value(|i| i.text.clone()), window, cx)
        });
        self.editing = info.map(|info| info.name.clone());
        self.as_text = info.is_some_and(|info| info.path.is_none());
        let focus = if info.is_some() { description } else { name }
            .read(cx)
            .focus_handle(cx);
        window.focus(&focus, cx);
        self.browsing = info
            .and_then(|info| info.path.as_deref())
            .map(Path::new)
            .and_then(Path::parent)
            .map(Path::to_owned)
            .unwrap_or_default();
        self.open = true;
        self.saving = false;
        self.form_error = None;
        cx.notify();
    }

    /// Reads the skill whole (a text skill's text), then opens it for editing.
    pub fn edit(&mut self, name: &str, cx: &mut Context<Self>) {
        self.reading = Some(name.to_owned());
        self.problem = None;
        let command = Command::SkillRead {
            project: self.project.clone(),
            name: name.to_owned(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's reading of the skill to edit.
    pub fn read_whole(
        &mut self,
        name: &str,
        read: Result<SkillInfo, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading.as_deref() != Some(name) {
            return;
        }
        self.reading = None;
        match read {
            Ok(info) => self.fill(Some(&info), window, cx),
            Err(why) => {
                self.problem = Some(why.into());
                cx.notify();
            }
        }
    }

    /// A path of the project, or a text Flottille keeps.
    pub fn set_text(&mut self, as_text: bool, cx: &mut Context<Self>) {
        self.as_text = as_text;
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
        let name = match &self.editing {
            Some(name) => name.clone(),
            None => fields.name.read(cx).value().trim().to_owned(),
        };
        let description = fields.description.read(cx).value().trim().to_owned();
        let (path, text) = if self.as_text {
            (None, Some(fields.text.read(cx).value().to_string()))
        } else {
            (Some(fields.path.read(cx).value().trim().to_owned()), None)
        };
        let write = match self.editing {
            Some(_) => SkillWrite::Edit {
                name,
                description,
                path,
                text,
            },
            None => SkillWrite::Add {
                name,
                description,
                path,
                text,
            },
        };
        let command = Command::SkillWrite {
            project: self.project.clone(),
            write,
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
        let command = Command::SkillWrite {
            project: self.project.clone(),
            write: SkillWrite::Remove {
                name: name.to_owned(),
            },
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to an add, an edit or a removal.
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

    fn row(&self, skill: &SkillInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let name = skill.name.clone();
        let id = |part: &str| ElementId::Name(format!("{part}-{}", skill.name).into());
        let confirming = self.confirming.as_deref() == Some(&skill.name);
        let place = skill.place();
        div()
            .id(id("skill"))
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
                                    .id(id("skill-path"))
                                    .aria_label(SharedString::from(place.clone()))
                                    .mono(11.5)
                                    .text_color(p.faint)
                                    .child(SharedString::from(place))
                                    .test_support(),
                            ),
                    )
                    .child(
                        div()
                            .id(id("skill-description"))
                            .aria_label(SharedString::from(skill.description.clone()))
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(SharedString::from(skill.description.clone()))
                            .test_support(),
                    ),
            )
            .child({
                let edit = name.clone();
                Button::quiet(id("edit-skill"), "Edit")
                    .disabled(self.reading.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| this.edit(&edit, cx)))
            })
            .child(if confirming {
                let (cancel, remove) = (name.clone(), name.clone());
                div()
                    .flex()
                    .gap(px(4.))
                    .child(Button::quiet(id("cancel-remove-skill"), "Cancel").on_click(
                        cx.listener(move |this, _, _, cx| {
                            if this.confirming.as_deref() == Some(&cancel) {
                                this.confirming = None;
                                cx.notify();
                            }
                        }),
                    ))
                    .child(
                        Button::secondary(id("confirm-remove-skill"), format!("Remove {name}"))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx))),
                    )
                    .into_any_element()
            } else {
                let remove = name.clone();
                Button::quiet(id("remove-skill"), "Remove")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx)))
                    .into_any_element()
            })
            .test_support()
    }

    fn modal(&mut self, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let title = if self.editing.is_some() {
            "Edit skill"
        } else {
            "Add a skill"
        };
        let modal = Modal::new("skill-modal", title)
            .width(px(560.))
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
            .id("skill-picker")
            .max_h(px(180.))
            .overflow_y_scroll()
            .rounded(px(8.))
            .border_1()
            .border_color(p.line)
            .flex()
            .flex_col()
            .child(
                div()
                    .id("skill-location")
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
                        .id("skill-up")
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
                        format!("skill-entry-{}", entry.name).into(),
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
        let kind = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.5))
            .text_color(p.mute)
            .child(
                Switch::new("skill-as-text", self.as_text, AS_TEXT).on_click(cx.listener(
                    |this, _, _, cx| {
                        let as_text = !this.as_text;
                        this.set_text(as_text, cx)
                    },
                )),
            )
            .child(
                div()
                    .id("skill-as-text-label")
                    .aria_label(AS_TEXT)
                    .child(AS_TEXT)
                    .test_support(),
            );
        let text = div()
            .id("skill-text")
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .text_size(px(12.5))
            .child(fields.text.clone())
            .test_support();
        modal
            .child(match &self.editing {
                Some(name) => div()
                    .mono(12.5)
                    .semibold()
                    .child(SharedString::from(name.clone()))
                    .into_any_element(),
                None => boxed("skill-name", Input::new(&fields.name), cx).into_any_element(),
            })
            .child(boxed(
                "skill-description-field",
                Input::new(&fields.description),
                cx,
            ))
            .child(kind)
            .when(self.as_text, |m| m.child(text))
            .when(!self.as_text, |m| {
                m.child(boxed("skill-path-field", Input::new(&fields.path), cx))
                    .child(picker)
            })
            .when_some(self.form_error.clone(), |m, error| {
                m.child(
                    div()
                        .id("skill-error")
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
                Button::primary(
                    "save-skill",
                    if self.editing.is_some() {
                        "Save"
                    } else {
                        "Add"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                .into_any_element()
            })
    }
}

impl Render for SkillsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let rows: Vec<_> = self
            .listed
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|skill| self.row(skill, cx).into_any_element())
            .collect();
        let count = rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(cx);
        div()
            .id("skills")
            .child(heading(
                "Skills",
                count,
                Button::secondary("add-skill", "Add skill")
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
                                .id("no-skills")
                                .text_size(px(12.5))
                                .text_color(p.faint)
                                .child(
                                    "No skill yet: add a file of the project, or a text, that \
                                     your agents read when their task needs it.",
                                )
                                .test_support(),
                        )
                    })
                    .when_some(self.problem.clone(), |d, problem| {
                        d.child(
                            div()
                                .id("skills-problem")
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
