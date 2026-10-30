//! A project's skills in its Settings (`docs/skills.md`): texts an agent reads with a tool when its
//! task needs them, each a path of the project or a text Flottille keeps. The window writes
//! nothing itself: an add, an edit and a removal are the daemon's (`skill.*`), and the section
//! shows the list the daemon gives next. The skills of the project's `.claude` folder are offered
//! under the list, each with Import (`skill.detect`, `skill.import`).
//!
//! The path of a skill is picked inside the project folder: the dialog lists the folder's
//! entries, a click on a folder opens it and picks it, a click on a file picks it. The daemon has
//! the last word on the path (it must stay inside the folder, once links are followed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flottille_ui::{
    Button, Confirm, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, RadioChoices,
    Section, Toast, Type, hoverable, icon, palette, presence, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{ClaudeKind, Command, SkillWrite};
use crate::claude::{Found, found_block};
use crate::commands::{boxed, editor_style, line_field, menu_of, perform_later, problem_line};
use crate::shell::Shell;

/// How many entries of a folder the picker lists.
const LISTED: usize = 200;

/// What the section says a skill is for.
pub const SENTENCE: &str = "Texts an agent reads when its task needs them. It sees their names \
                            and one line; it opens one with a tool.";

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

/// What a write was, for the toast its answer says.
#[derive(Clone, Debug, PartialEq)]
enum Writing {
    Added(String),
    Saved(String),
    Removed(String),
}

pub struct SkillsSection {
    shell: WeakEntity<Shell>,
    project: String,
    /// The project's folder, as the page knows it.
    folder: String,
    listed: Option<Vec<SkillInfo>>,
    /// The rows as drawn: a removed skill folds away before it is gone.
    rows: Listing<SkillInfo>,
    menus: HashMap<String, Entity<Menu>>,
    /// Why the list cannot be read.
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
    writing: Option<Writing>,
    form_error: Option<SharedString>,
    confirming: Option<String>,
    /// What the confirmation names, kept while it fades away.
    asked: String,
    /// What the project's `.claude/skills` holds, as the daemon last found it.
    found: Option<Vec<Found>>,
    found_rows: Listing<Found>,
    importing: bool,
    _subscriptions: Vec<Subscription>,
}

impl SkillsSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            folder: String::new(),
            listed: None,
            rows: Listing::default(),
            menus: HashMap::new(),
            problem: None,
            open: false,
            editing: None,
            as_text: false,
            reading: None,
            fields: None,
            browsing: PathBuf::new(),
            saving: false,
            writing: None,
            form_error: None,
            confirming: None,
            asked: String::new(),
            found: None,
            found_rows: Listing::default(),
            importing: false,
            _subscriptions: Vec::new(),
        }
    }

    pub fn skills(&self) -> Option<&[SkillInfo]> {
        self.listed.as_deref()
    }

    /// What `.claude/skills` holds, as the daemon last found it.
    pub fn found(&self) -> Option<&[Found]> {
        self.found.as_deref()
    }

    /// Whether the form writes a text Flottille keeps rather than a path.
    pub fn is_text(&self) -> bool {
        self.as_text
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_confirming(&self) -> bool {
        self.confirming.is_some()
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

    /// Asks the daemon for the list, and for what `.claude/skills` holds.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(
            &self.shell,
            Command::Skills {
                project: project.clone(),
            },
            cx,
        );
        let kind = ClaudeKind::Skill;
        perform_later(&self.shell, Command::ClaudeFound { project, kind }, cx);
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

    /// What the daemon found in `.claude/skills`. A folder that cannot be read offers nothing.
    pub fn found_listed(&mut self, found: Result<Vec<Found>, String>, cx: &mut Context<Self>) {
        let found = found.unwrap_or_default();
        if self.found.as_ref() != Some(&found) {
            self.found = Some(found);
            cx.notify();
        }
    }

    /// Imports `name` from `.claude/skills`, or all that can be.
    pub fn import(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.importing {
            return;
        }
        self.importing = true;
        let command = Command::ClaudeImport {
            project: self.project.clone(),
            kind: ClaudeKind::Skill,
            names: name.map(|name| vec![name]),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to an import: a toast says it, and the lists are read again.
    pub fn imported(&mut self, read: Result<serde_json::Value, String>, cx: &mut Context<Self>) {
        self.importing = false;
        toast(crate::claude::imported_toast(ClaudeKind::Skill, &read), cx);
        self.read(cx);
        cx.notify();
    }

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.fields.is_none() {
            let name = line_field("style", window, cx);
            let description = line_field("writing code", window, cx);
            let path = line_field("docs/STYLE.md", window, cx);
            let text = cx.new(|cx| {
                let mut field = TextareaState::new(window, cx)
                    .auto_grow(6, 14)
                    .placeholder("The text, in Markdown");
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

    /// Opens the dialog, empty, the caret in the name.
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fill(None, window, cx);
    }

    /// Opens the dialog on `info`, or empty for a new skill.
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
                toast(
                    Toast::failure(format!("The skill {name} cannot be read")).detail(why),
                    cx,
                );
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
        let (write, writing) = match self.editing {
            Some(_) => (
                SkillWrite::Edit {
                    name: name.clone(),
                    description,
                    path,
                    text,
                },
                Writing::Saved(name),
            ),
            None => (
                SkillWrite::Add {
                    name: name.clone(),
                    description,
                    path,
                    text,
                },
                Writing::Added(name),
            ),
        };
        let command = Command::SkillWrite {
            project: self.project.clone(),
            write,
        };
        self.saving = true;
        self.writing = Some(writing);
        self.form_error = None;
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// Asks to remove, in a confirmation; called again from it, removes.
    pub fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(name) {
            self.confirming = Some(name.to_owned());
            self.asked = name.to_owned();
            cx.notify();
            return;
        }
        self.confirming = None;
        self.problem = None;
        self.writing = Some(Writing::Removed(name.to_owned()));
        let command = Command::SkillWrite {
            project: self.project.clone(),
            write: SkillWrite::Remove {
                name: name.to_owned(),
            },
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to an add, an edit or a removal: a toast says it; a refusal of the
    /// dialog's stays under its fields.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        let writing = self.writing.take();
        match refused {
            None => {
                self.open = false;
                self.form_error = None;
                let said = match writing {
                    Some(Writing::Added(name)) => Toast::success(format!("Skill {name} added"))
                        .detail("Agents created from now on see it."),
                    Some(Writing::Saved(name)) => Toast::success(format!("Skill {name} saved"))
                        .detail("Agents created from now on read it so."),
                    Some(Writing::Removed(name)) => Toast::success(format!("Skill {name} removed"))
                        .detail("Agents created from now on no longer see it."),
                    None => Toast::success("Skills saved"),
                };
                toast(said, cx);
                self.read(cx);
            }
            Some(why) if !self.open => {
                toast(Toast::failure("The skill was not removed").detail(why), cx)
            }
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn menu(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.entity().downgrade();
        menu_of(&mut self.menus, "skill", name, cx, move |name, menu| {
            let (edit, remove) = (this.clone(), this.clone());
            let (to_edit, to_remove) = (name.to_owned(), name.to_owned());
            menu.entry(format!("edit-skill-{name}"), "Edit", None, move |_, cx| {
                edit.update(cx, |this, cx| this.edit(&to_edit, cx)).ok();
            })
            .danger(format!("remove-skill-{name}"), "Remove…", move |_, cx| {
                remove
                    .update(cx, |this, cx| this.remove(&to_remove, cx))
                    .ok();
            })
        })
    }

    fn row(&mut self, skill: &SkillInfo, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let name = skill.name.clone();
        let glyph = if skill.path.is_some() {
            Icon::File
        } else {
            Icon::Text
        };
        let menu = self.menu(&name, cx);
        ListRow::new(
            ElementId::Name(format!("skill-{name}").into()),
            SharedString::from(name.clone()),
        )
        .mono()
        .leading(icon(glyph).size(px(15.)).text_color(p.faint))
        .tag(SharedString::from(skill.place()))
        .line(SharedString::from(skill.description.clone()))
        .more(menu)
        .on_click(cx.listener(move |this, _, _, cx| {
            if this.reading.is_none() {
                this.edit(&name, cx)
            }
        }))
    }

    /// The confirmation of a removal.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let name = self.asked.clone();
        let file = self
            .listed
            .iter()
            .flatten()
            .find(|skill| skill.name == name)
            .and_then(|skill| skill.path.clone());
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("skill-removal", format!("Remove the skill {name}?"))
            .sentence(match file {
                Some(file) => {
                    format!("Agents created from now on no longer see it. The file {file} stays.")
                }
                None => "Agents created from now on no longer see it, and its text is lost.".into(),
            })
            .action("Remove skill")
            .open(self.confirming.is_some())
            .on_confirm(move |_, cx| {
                let name = name.clone();
                this.update(cx, |this, cx| this.remove(&name, cx)).ok();
            })
            .on_cancel(move |_, cx| {
                again
                    .update(cx, |this, cx| {
                        this.confirming = None;
                        cx.notify();
                    })
                    .ok();
            })
    }

    fn modal(&mut self, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let title = match &self.editing {
            Some(name) => format!("Skill {name}"),
            None => "Add a skill".to_owned(),
        };
        let modal = Modal::new("skill-modal", title)
            .sentence(
                "A text an agent reads when its task needs it. It sees the name and the line; it \
                 opens the skill with a tool.",
            )
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let Some(fields) = self.fields.as_ref().filter(|_| self.open) else {
            return modal;
        };
        let here = Path::new(&self.folder).join(&self.browsing);
        let listed = entries(&here);
        let mut location = vec![self.project.clone()];
        location.extend(
            self.browsing
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned()),
        );
        let picker = div()
            .id("skill-picker")
            .max_h(px(150.))
            .overflow_y_scroll()
            .p(space::XS)
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .flex()
            .flex_col()
            .child(
                div()
                    .id("skill-location")
                    .px(space::SM)
                    .py(space::XS)
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(format!("{} /", location.join(" / "))))
                    .test_support(),
            )
            .when(!self.browsing.as_os_str().is_empty(), |d| {
                d.child(hoverable(
                    div()
                        .id("skill-up")
                        .h(px(28.))
                        .px(space::SM)
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .mono(12.5)
                        .text_color(p.mute)
                        .child("..")
                        .on_click(cx.listener(|this, _, window, cx| this.up(window, cx))),
                    move |d, t| d.bg(p.hover.opacity(t)),
                ))
            })
            .children(listed.into_iter().map(|entry| {
                let name = entry.name.clone();
                let picked = self.picked(cx) == self.browsing.join(&entry.name).to_string_lossy();
                let row = div()
                    .id(ElementId::Name(
                        format!("skill-entry-{}", entry.name).into(),
                    ))
                    .h(px(28.))
                    .px(space::SM)
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .mono(12.5)
                    .child(
                        icon(if entry.folder {
                            Icon::Folder
                        } else {
                            Icon::File
                        })
                        .size(px(13.))
                        .text_color(p.faint),
                    )
                    .child(SharedString::from(if entry.folder {
                        format!("{}/", entry.name)
                    } else {
                        entry.name.clone()
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick(&name, entry.folder, window, cx)
                    }));
                flottille_ui::selectable(row, picked, move |d, t, chosen| {
                    d.bg(flottille_ui::color::mix(p.hover.opacity(t), p.sel, chosen))
                        .text_color(flottille_ui::color::mix(
                            flottille_ui::color::mix(p.mute, p.text, t),
                            p.text,
                            chosen,
                        ))
                })
            }));
        let this = cx.entity().downgrade();
        let kind = RadioChoices::new("skill-kind", "What it is")
            .choice(
                "file",
                "A file of the project",
                "Read where the agent works: its workspace's copy when it has one.",
            )
            .leading(Icon::File)
            .choice(
                "text",
                "A text Flottille keeps",
                "Kept in Flottille's data folder, never in the project.",
            )
            .leading(Icon::Text)
            .chosen(usize::from(self.as_text))
            .on_choose(move |index, _, cx| {
                this.update(cx, |this, cx| this.set_text(index == 1, cx))
                    .ok();
            });
        let text = div()
            .id("skill-text")
            .min_h(px(110.))
            .px(space::MD)
            .py(space::SM)
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .text_size(px(12.5))
            .child(fields.text.clone())
            .test_support();
        let name = match &self.editing {
            Some(name) => div()
                .id("skill-name")
                .h(px(32.))
                .flex()
                .items_center()
                .mono(12.5)
                .text_color(p.mute)
                .child(SharedString::from(name.clone()))
                .test_support()
                .into_any_element(),
            None => boxed("skill-name", Input::new(&fields.name), cx).into_any_element(),
        };
        modal
            .child(
                div()
                    .flex()
                    .gap(space::MD)
                    .child(div().flex_1().min_w_0().child(Field::new(
                        "skill-name-field",
                        "Name",
                        name,
                    )))
                    .child(div().flex_1().min_w_0().child(Field::new(
                        "skill-description",
                        "Read it when",
                        boxed(
                            "skill-description-field",
                            Input::new(&fields.description),
                            cx,
                        ),
                    ))),
            )
            .child(Field::new("skill-kind-field", "What it is", kind))
            // The text and the path with its picker fold into each other with the choice.
            .child(
                presence(
                    "skill-path-block",
                    !self.as_text,
                    Field::new(
                        "skill-file",
                        "File",
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::SM)
                            .child(boxed("skill-path-field", Input::new(&fields.path), cx))
                            .child(picker),
                    ),
                )
                .gap(space::LG),
            )
            .child(
                presence(
                    "skill-text-block",
                    self.as_text,
                    Field::new("skill-text-field", "Text", text),
                )
                .gap(space::LG),
            )
            .child(problem_line(
                "skill-error",
                self.form_error.clone(),
                space::LG,
                cx,
            ))
            .action(
                Button::primary(
                    "save-skill",
                    if self.editing.is_some() {
                        "Save"
                    } else {
                        "Add skill"
                    },
                )
                .disabled(self.saving)
                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
    }
}

impl Render for SkillsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|s| (s.name.clone(), s)), now, reduce);
        }
        crate::claude::sync(&mut self.found_rows, self.found.as_deref(), cx);
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("skill-list");
        for (key, placement, skill) in drawn {
            let row = self.row(&skill, cx);
            list = list.item(
                ElementId::Name(format!("skill-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(cx);
        let this = cx.entity().downgrade();
        let found = found_block(
            "skill",
            "skills",
            &self.found_rows,
            self.importing,
            move |name, _, cx| {
                this.update(cx, |this, cx| this.import(name, cx)).ok();
            },
            cx,
        );
        let section = Section::new("skills", "Skills")
            .count(count.to_string())
            .sentence(SENTENCE)
            .action(
                Button::secondary("add-skill", "Add skill")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
            )
            .child(presence("skill-list-block", count > 0, list).gap(space::MD))
            .child(
                presence(
                    "no-skills-block",
                    empty,
                    EmptyLine::new(
                        "no-skills",
                        Icon::File,
                        "No skill yet: add a file of the project, or a text, that your agents \
                         read when their task needs it.",
                    ),
                )
                .gap(space::MD),
            )
            .child(found)
            .child(problem_line(
                "skills-problem",
                self.problem.clone(),
                space::MD,
                cx,
            ));
        div()
            .text_color(p.text)
            .child(section)
            .child(modal)
            .child(self.confirm(cx))
    }
}
