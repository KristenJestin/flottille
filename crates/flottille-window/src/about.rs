//! What the user says of themselves, edited in the window (`docs/design.md` §8): the "About you"
//! section of Settings, General. Three fields, each optional: what agents call the user, how to
//! refer to them, and anything every agent should know. The window writes nothing itself: Save
//! and Reset are the daemon's (`about.set`, `about.reset`), and the editor shows what the daemon
//! says next.
//!
//! The text is frozen in an agent when it is created, so a change reaches the agents created
//! after it only: the toast that follows a save says so.
//!
//! With a hub (`docs/remote.md`, "Settings for all machines"), the section is drawn on All machines
//! or on one machine. On All machines it edits the values every machine uses, a change signed with
//! the device key (`about.share`). On a machine it says once that the values are all machines'
//! unless changed there; a field changed there is the machine's own, its edge in the colour of
//! what is set apart and one line under it, "Changed on pc", with Use all machines' value, which
//! drops it (`about.set` of the field empty).

use gpui_design_system::motion::PANEL;
use gpui_design_system::{
    Button, Field, Section, SetHere, Toast, follow_theme, palette, presence, presence_of, space,
    tint, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, TestSupportExt as _,
    Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::{boxed_edged, line_field, perform_later, spaced, stack};

/// What a machine's section says once while it follows the values of all machines.
pub const SHARED_LINE: &str = "The values of all machines, unless changed here.";

/// The way back of a field changed on a machine.
pub const USE_ALL: &str = "Use all machines' value";

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
    /// The fields the machine holds itself, by key (`name`, `pronouns`, `notes`).
    pub own: Vec<String>,
    /// The values of all machines (name, how to refer to the user, notes), with a hub.
    pub all: Option<(String, String, String)>,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        let all = value["all"].as_object().map(|all| {
            let text = |key: &str| {
                all.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            (text("name"), text("pronouns"), text("notes"))
        });
        Shown {
            name: text("name"),
            pronouns: text("pronouns"),
            notes: text("notes"),
            own: value["own"]
                .as_array()
                .map(|own| {
                    own.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            all,
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
    shell: crate::commands::ShellOf,
    /// Made when first drawn, since a text field needs a window.
    fields: Option<Fields>,
    shown: Option<Shown>,
    /// A save or a reset waits for the daemon.
    waiting: bool,
    /// What waits is a reset.
    resetting: bool,
    /// The section is drawn on All machines: it edits the values every machine uses.
    on_all: bool,
    /// The machine whose section this is, by name: what a field changed there says.
    machine: String,
    /// The machine has a hub and another machine to share with: Settings offers All machines.
    hub: bool,
    _subscriptions: Vec<Subscription>,
}

/// The keys of the fields, in their order.
const KEYS: [&str; 3] = ["name", "pronouns", "notes"];

#[derive(Clone)]
struct Fields {
    name: Entity<InputState>,
    pronouns: Entity<InputState>,
    notes: Entity<TextareaState>,
}

impl AboutEditor {
    pub fn new(shell: crate::commands::ShellOf) -> Self {
        Self {
            shell,
            fields: None,
            shown: None,
            waiting: false,
            resetting: false,
            on_all: false,
            machine: String::new(),
            hub: false,
            _subscriptions: Vec::new(),
        }
    }

    /// Where the section is drawn: on All machines, or on `machine`, which has a hub and another
    /// machine to share with or not. Coming to All machines or leaving it puts in the fields what
    /// that place holds.
    pub fn place(
        &mut self,
        on_all: bool,
        machine: String,
        hub: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hub = hub;
        self.machine = machine;
        if self.on_all == on_all {
            return;
        }
        self.on_all = on_all;
        if let (Some(fields), Some(shown)) = (self.fields.clone(), self.shown.clone()) {
            let (name, pronouns, notes) = self.values_of(&shown);
            Self::put(&fields, [&name, &pronouns, &notes], window, cx);
        }
    }

    /// Whether the section is drawn on All machines.
    pub fn on_all(&self) -> bool {
        self.on_all
    }

    /// What the fields show of `shown` where the section is drawn: all machines' values on All
    /// machines, what the machine's agents read on a machine.
    fn values_of(&self, shown: &Shown) -> (String, String, String) {
        match (self.on_all, &shown.all) {
            (true, Some(all)) => all.clone(),
            (true, None) => Default::default(),
            (false, _) => (
                shown.name.clone(),
                shown.pronouns.clone(),
                shown.notes.clone(),
            ),
        }
    }

    /// The fields that differ from what the daemon last said, with what they hold.
    fn changed(&self, cx: &gpui_kit::App) -> Vec<(String, String)> {
        let (Some(shown), Some((name, pronouns, notes))) = (&self.shown, self.typed(cx)) else {
            return Vec::new();
        };
        let (was_name, was_pronouns, was_notes) = self.values_of(shown);
        KEYS.iter()
            .zip([
                (name, was_name),
                (pronouns, was_pronouns),
                (notes, was_notes),
            ])
            .filter(|(_, (now, was))| now != was)
            .map(|(key, (now, _))| (key.to_string(), now))
            .collect()
    }

    /// Whether the machine holds the field `key` itself, while it has a hub.
    pub fn is_own(&self, key: &str) -> bool {
        !self.on_all
            && self.hub
            && self
                .shown
                .as_ref()
                .is_some_and(|shown| shown.all.is_some() && shown.own.iter().any(|own| own == key))
    }

    /// Drops the machine's own `key`: the field follows all machines again.
    pub fn use_all(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.waiting || !self.is_own(key) {
            return;
        }
        self.waiting = true;
        perform_later(
            &self.shell,
            Command::AboutWrite {
                fields: Some(vec![(key.to_owned(), String::new())]),
            },
            cx,
        );
        cx.notify();
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
        !self.changed(cx).is_empty()
    }

    /// Whether Reset has something to forget: on All machines a value, on a machine with a hub
    /// a field of its own, else a field.
    pub fn is_set(&self) -> bool {
        let Some(shown) = &self.shown else {
            return false;
        };
        match (self.on_all, &shown.all) {
            (true, _) => {
                let (name, pronouns, notes) = self.values_of(shown);
                [name, pronouns, notes]
                    .iter()
                    .any(|text| !text.trim().is_empty())
            }
            (false, Some(_)) if self.hub => !shown.own.is_empty(),
            (false, _) => !shown.is_empty(),
        }
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
        let fields = Fields {
            name,
            pronouns,
            notes,
        };
        if let Some(shown) = self.shown.clone() {
            let (name, pronouns, notes) = self.values_of(&shown);
            Self::put(&fields, [&name, &pronouns, &notes], window, cx);
        }
        self.fields = Some(fields.clone());
        fields
    }

    fn put(
        fields: &Fields,
        [name, pronouns, notes]: [&String; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        fields
            .name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        fields
            .pronouns
            .update(cx, |field, cx| field.set_value(pronouns, window, cx));
        fields
            .notes
            .update(cx, |field, cx| field.set_value(notes, window, cx));
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
                    Ok(_) if resetting && self.on_all => {
                        Toast::success("About you reset for all machines")
                            .detail("A machine that changed a field keeps its own.")
                    }
                    Ok(_) if resetting => Toast::success("About you reset")
                        .detail("Agents started from now on are told nothing of you."),
                    Ok(_) if self.on_all => Toast::success("About you saved for all machines")
                        .detail("Agents started from now on read it, on every machine."),
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
                let (name, pronouns, notes) = self.values_of(&shown);
                Self::put(&fields, [&name, &pronouns, &notes], window, cx);
            }
            self.shown = Some(shown);
        }
        cx.notify();
    }

    /// Saves the fields changed: for all machines on All machines, else as the machine's own.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let fields = self.changed(cx);
        if self.waiting || fields.is_empty() {
            return;
        }
        self.waiting = true;
        let command = match self.on_all {
            true => Command::AboutShare { fields },
            false => Command::AboutWrite {
                fields: Some(fields),
            },
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.waiting || !self.is_set() {
            return;
        }
        self.waiting = true;
        self.resetting = true;
        let command = match self.on_all {
            true => Command::AboutShare {
                fields: KEYS
                    .iter()
                    .map(|key| (key.to_string(), String::new()))
                    .collect(),
            },
            false => Command::AboutWrite { fields: None },
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// `field`, under the mark of a field the machine holds itself, its edge in the colour of what
    /// is set apart, both moving as it becomes the machine's own or follows all machines again.
    fn own_field(
        &self,
        key: &'static str,
        field: impl IntoElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SetHere {
        let editor = cx.entity().downgrade();
        let _ = window;
        SetHere::new(
            gpui_kit::ElementId::Name(format!("about-{key}-own").into()),
            field,
            format!("Changed on {}", self.machine),
            USE_ALL,
        )
        .set(self.is_own(key))
        .on_reset(move |_, cx| {
            editor.update(cx, |editor, cx| editor.use_all(key, cx)).ok();
        })
    }

    /// The edge of the field `key`'s box: the colour of what is set apart while the machine holds
    /// it itself, the line's otherwise, fading over `PANEL`.
    fn edge(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::Hsla {
        let p = palette(cx);
        let target = if self.is_own(key) { p.wait } else { p.line2 };
        tint(
            gpui_kit::ElementId::Name(format!("about-{key}-edge").into()),
            target,
            PANEL,
            window,
            cx,
        )
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
        let edges = KEYS.map(|key| self.edge(key, window, cx));
        let name = Field::new(
            "about-name",
            "Name",
            boxed_edged("about-name-box", Input::new(&fields.name), edges[0], cx),
        )
        .optional()
        .hint("What agents call you.");
        let pronouns = Field::new(
            "about-pronouns",
            "How to refer to you",
            boxed_edged(
                "about-pronouns-box",
                Input::new(&fields.pronouns),
                edges[1],
                cx,
            ),
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
                .border_color(edges[2])
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
        // Said once, on a machine that has a hub: the fields follow all machines unless changed.
        let follows = !self.on_all
            && self.hub
            && self.shown.as_ref().is_some_and(|shown| shown.all.is_some());
        let name = self.own_field("name", name, window, cx);
        let pronouns = self.own_field("pronouns", pronouns, window, cx);
        let notes = self.own_field("notes", notes, window, cx);
        Section::new("about", "About you")
            .sentence(SENTENCE)
            .child(
                stack(space::MD)
                    .child(
                        presence(
                            "about-shared-block",
                            follows,
                            div()
                                .id("about-shared")
                                .aria_label(SHARED_LINE)
                                .text_size(px(12.5))
                                .text_color(p.mute)
                                .child(SHARED_LINE)
                                .test_support(),
                        )
                        .space(space::MD),
                    )
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
