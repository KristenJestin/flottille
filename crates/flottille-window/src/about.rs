//! What the user says of themselves, edited in the window (`docs/design.md` §8): the "About you"
//! section of Settings, General. Three fields, each optional: what agents call the user, how to
//! refer to them, and anything every agent should know. The window writes nothing itself: Save
//! and Reset are the daemon's (`about.set`, `about.reset`), and the editor shows what the daemon
//! says next.
//!
//! The text is frozen in an agent when it is created, so a change reaches the agents created
//! after it only: the toast that follows a save says so.

use gpui_design_system::{
    Button, Field, Section, Toast, follow_theme, palette, presence, presence_of, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::{boxed, line_field, perform_later, spaced, stack};
use crate::shell::Shell;

/// What the section says under its title.
const SENTENCE: &str = "What every agent is told about you, before the rules, and what an agent \
                        writes for another one follows. Each field is optional.";

/// The notes' placeholder: invented examples, so a new user sees what fits.
const NOTES_EXAMPLE: &str = "who you are, what you do, the language and tone you want, how much \
                             detail, what to avoid. For example: I'm a backend developer, new to \
                             Rust. Answer in English, short and direct.";

/// What the daemon says of it (`about.show`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    pub name: String,
    pub pronouns: String,
    pub notes: String,
    /// The notes' limit, in characters.
    pub limit: usize,
    /// Why a text edited by hand past a limit is cut for the agents.
    pub cut: Option<String>,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        Shown {
            name: text("name"),
            pronouns: text("pronouns"),
            notes: text("notes"),
            limit: value["limit"]
                .as_u64()
                .map_or(8_000, |limit| limit as usize),
            cut: value["cut"].as_str().map(str::to_owned),
        }
    }

    /// Nothing is said: there is nothing to reset.
    pub fn is_empty(&self) -> bool {
        [&self.name, &self.pronouns, &self.notes]
            .iter()
            .all(|text| text.trim().is_empty())
    }
}

pub struct AboutEditor {
    shell: WeakEntity<Shell>,
    /// Made when first drawn, since a text field needs a window.
    fields: Option<Fields>,
    shown: Option<Shown>,
    /// A save or a reset waits for the daemon.
    waiting: bool,
    /// What waits is a reset.
    resetting: bool,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct Fields {
    name: Entity<InputState>,
    pronouns: Entity<InputState>,
    notes: Entity<TextareaState>,
}

impl AboutEditor {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            fields: None,
            shown: None,
            waiting: false,
            resetting: false,
            _subscriptions: Vec::new(),
        }
    }

    /// Asks the daemon for what the user said.
    pub fn read(&self, cx: &mut Context<Self>) {
        perform_later(&self.shell, Command::AboutRead, cx);
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn name_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|fields| &fields.name)
    }

    pub fn pronouns_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|fields| &fields.pronouns)
    }

    pub fn notes_field(&self) -> Option<&Entity<TextareaState>> {
        self.fields.as_ref().map(|fields| &fields.notes)
    }

    /// What the fields hold: name, how to refer to the user, notes.
    pub fn typed(&self, cx: &gpui_kit::App) -> Option<(String, String, String)> {
        self.fields.as_ref().map(|fields| {
            (
                fields.name.read(cx).value().to_string(),
                fields.pronouns.read(cx).value().to_string(),
                fields.notes.read(cx).value().to_string(),
            )
        })
    }

    /// A field differs from what the daemon last said.
    pub fn is_edited(&self, cx: &gpui_kit::App) -> bool {
        match (&self.shown, self.typed(cx)) {
            (Some(shown), Some((name, pronouns, notes))) => {
                shown.name != name || shown.pronouns != pronouns || shown.notes != notes
            }
            _ => false,
        }
    }

    /// Whether Reset has something to forget.
    pub fn is_set(&self) -> bool {
        self.shown.as_ref().is_some_and(|shown| !shown.is_empty())
    }

    fn make_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Fields {
        let name = line_field("Sam", window, cx);
        let pronouns = line_field("he/him, she/her, they", window, cx);
        let notes = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(6, 18)
                .placeholder(NOTES_EXAMPLE);
            follow_theme(&mut field, cx);
            field
        });
        for input in [&name, &pronouns] {
            self._subscriptions
                .push(cx.subscribe(input, |_, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }));
        }
        self._subscriptions
            .push(cx.subscribe(&notes, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        if let Some(shown) = &self.shown {
            Self::put(&name, &pronouns, &notes, shown, window, cx);
        }
        let fields = Fields {
            name,
            pronouns,
            notes,
        };
        self.fields = Some(fields.clone());
        fields
    }

    fn put(
        name: &Entity<InputState>,
        pronouns: &Entity<InputState>,
        notes: &Entity<TextareaState>,
        shown: &Shown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        name.update(cx, |field, cx| field.set_value(&shown.name, window, cx));
        pronouns.update(cx, |field, cx| field.set_value(&shown.pronouns, window, cx));
        notes.update(cx, |field, cx| field.set_value(&shown.notes, window, cx));
    }

    /// The daemon's answer to a read, a save or a reset. Typing in progress is kept when a read
    /// comes back, and replaced by what was saved.
    pub fn answered(
        &mut self,
        said: Result<Shown, String>,
        written: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.waiting = false;
        let resetting = std::mem::take(&mut self.resetting);
        if written {
            toast(
                match &said {
                    Ok(_) if resetting => Toast::success("About you reset")
                        .detail("Agents started from now on are told nothing of you."),
                    Ok(_) => Toast::success("About you saved")
                        .detail("Agents started from now on read it."),
                    Err(why) => Toast::failure("About you not saved").detail(why.clone()),
                },
                cx,
            );
        }
        if let Ok(shown) = said {
            let keep = !written && self.is_edited(cx);
            if !keep && let Some(fields) = self.fields.clone() {
                Self::put(
                    &fields.name,
                    &fields.pronouns,
                    &fields.notes,
                    &shown,
                    window,
                    cx,
                );
            }
            self.shown = Some(shown);
        }
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some((name, pronouns, notes)) = self.typed(cx) else {
            return;
        };
        if self.waiting || !self.is_edited(cx) {
            return;
        }
        self.waiting = true;
        perform_later(
            &self.shell,
            Command::AboutWrite {
                text: Some((name, pronouns, notes)),
            },
            cx,
        );
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.waiting || !self.is_set() {
            return;
        }
        self.waiting = true;
        self.resetting = true;
        perform_later(&self.shell, Command::AboutWrite { text: None }, cx);
        cx.notify();
    }
}

impl Render for AboutEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let fields = match self.fields.clone() {
            Some(fields) => fields,
            None => self.make_fields(window, cx),
        };
        let limit = self.shown.as_ref().map_or(8_000, |shown| shown.limit);
        let name = Field::new(
            "about-name",
            "Name",
            boxed("about-name-box", Input::new(&fields.name), cx),
        )
        .optional()
        .hint("What agents call you.");
        let pronouns = Field::new(
            "about-pronouns",
            "How to refer to you",
            boxed("about-pronouns-box", Input::new(&fields.pronouns), cx),
        )
        .optional()
        .hint("One line: he/him, she/her, they.");
        let notes = Field::new(
            "about-notes",
            "Anything every agent should know",
            div()
                .id("about-notes-box")
                .min_h(px(120.))
                .px(space::MD)
                .py(space::MD)
                .rounded(px(10.))
                .bg(p.field)
                .border_1()
                .border_color(p.line2)
                .text_size(px(13.))
                .line_height(px(20.))
                .child(fields.notes.clone())
                .test_support(),
        )
        .optional()
        .hint(format!("At most {limit} characters."));
        let footer = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(space::SM)
            .child(
                Button::quiet("about-reset", "Reset")
                    .disabled(!self.is_set() || self.waiting)
                    .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
            )
            // Save is not drawn at rest: it unfolds beside Reset once a field differs from what
            // is saved, and folds away after it is saved or set back.
            .child(
                div().flex_none().child(presence(
                    "about-save-block",
                    self.is_edited(cx),
                    Button::primary("about-save", "Save")
                        .disabled(self.waiting)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )),
            );
        Section::new("about", "About you")
            .sentence(SENTENCE)
            .child(
                stack(space::MD)
                    .child(spaced(space::MD, name))
                    .child(spaced(space::MD, pronouns))
                    .child(spaced(space::MD, notes))
                    .child(
                        presence_of(
                            "about-cut-block",
                            self.shown
                                .as_ref()
                                .and_then(|shown| shown.cut.clone())
                                .map(|cut| SharedString::from(format!("Cut: {cut}."))),
                            move |cut| {
                                div()
                                    .id("about-cut")
                                    .text_size(px(12.5))
                                    .text_color(p.fail)
                                    .child(cut)
                                    .test_support()
                                    .into_any_element()
                            },
                        )
                        .space(space::MD),
                    )
                    .child(spaced(space::MD + space::XS, footer)),
            )
            .into_any_element()
    }
}
