//! Settings, Machines (`docs/remote.md`, "Identity and keys", "The hub", "Pairing", "Roles"):
//! whether remote is on (a **Turn on** button while it is off, a **Remote** switch once it is),
//! **Connect to your hub** while this machine is in no member list, a dialog where the code
//! `flottille-hub pair` printed is pasted and the card this machine proposes confirmed; then the
//! devices of the list, each renamed, drawn another way or removed from its "⋯". The page says the state and holds the buttons: what is typed is a dialog of the
//! window's host. A device is added on the hub only: every new member, and every result, is a
//! toast. What is asked goes to this machine's daemon, signed with the
//! device key ([`crate::machines::Remote`]); what it answers comes back as news.

use std::collections::HashMap;

use gpui_design_system::motion::PANEL;
use gpui_design_system::{
    Button, Confirm, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, RadioChoices,
    Section, Select, SelectOption, Swatch, Swatches, Switch, Toast, changed, changing, enter, fade,
    palette, presence, presence_of, space, tipped_when_cut, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _, Window, div, px,
};
use zeroize::Zeroizing;

use crate::commands::{boxed, line_field, menu_of, problem_line, spaced, stack};
use crate::machines::{
    Ask, Asked, COLOURS, Hub, ICONS, Machines, Member, News, ROLES, Remote, how_it_failed,
    role_words, shade,
};
use crate::modals::Modals;

/// The dialog that connects this machine to the user's hub, drawn by the window's host of dialogs.
pub const CONNECT_MODAL: &str = "hub-connect";
/// The dialog that renames a device.
pub const RENAME_MODAL: &str = "member-rename";
/// The confirmation of a removal, over any page.
pub const REMOVAL_DIALOG: &str = "member-removal-dialog";
/// The dialog that changes a device's icon and colour.
pub const LOOK_MODAL: &str = "member-look";

const REMOTE_OFF: &str = "Remote is off. This machine can join a hub and link to your other \
                          machines.";
const REMOTE_HINT: &str =
    "Turn it off and the links close; this machine stays paired with your hub.";
const CONNECT_SENTENCE: &str = "Your machines find each other through a hub you run on a host \
                                that is always on. Run flottille-hub pair there, then paste the \
                                code it prints: the first machine and the next ones connect the \
                                same way.";
const CONNECT_SAID: &str = "Paste the code flottille-hub pair printed on your hub's host. This \
                            machine proposes its name, how it is drawn and what it may do: change \
                            them before you connect.";
const DEVICES_SENTENCE: &str = "Each device in this list reaches your machines with the role it \
                                was given. A device is added on your hub's host, with \
                                flottille-hub pair, and its role changed there.";
const HUB_SENTENCE: &str = "The hub tells your machines each other's addresses and keeps the \
                            list of your devices. A link between two machines may go through \
                            it, end-to-end encrypted: it carries what they say and cannot read \
                            it.";
const FORGOTTEN: &str = "The hub no longer knows this machine";
const FORGOTTEN_SENTENCE: &str = "The list below is the one this machine saw last, kept as it \
                                  was. Pair again with your hub's code to be one of its devices \
                                  again: what the hub holds now replaces it.";
const PAIR_AGAIN_SAID: &str = "Paste the code your hub's host printed. This machine proposes its \
                               name, how it is drawn and what it may do: change them before you \
                               pair again. What the hub holds now replaces the old list.";

/// What is asked to be removed, until it is confirmed or cancelled.
#[derive(Clone, Debug, PartialEq)]
pub enum Removal {
    Member(String),
    /// Turning remote off: the links close.
    Remote,
}

pub struct MachinesSection {
    modals: Entity<Modals>,
    remote: Remote,
    read: Option<Machines>,
    /// The dialog that connects to the hub: whether it is open, its fields, the icon and the role
    /// chosen, and its refusal, shown in it.
    connect_open: bool,
    connecting: bool,
    connect_code: Option<Entity<InputState>>,
    connect_name: Option<Entity<InputState>>,
    connect_icon: usize,
    connect_role: usize,
    connect_error: Option<SharedString>,
    /// The dialog pairs this machine again, after the hub forgot it: what the hub holds replaces
    /// the old list.
    connect_replaces: bool,
    /// A change of the list, a look or a pairing was asked and not answered yet: the buttons and
    /// the items that ask it are busy, so that one click is one request.
    changing: bool,
    /// The dialog that changes a device's look: whose, and the icon and colour chosen.
    looking: Option<String>,
    look_icon: usize,
    look_colour: usize,
    /// What was read while remote was on: drawn while the settings fold away as it is turned off.
    kept: Option<Machines>,
    /// A change of remote was asked and not answered yet.
    configuring: bool,
    members: Listing<Member>,
    /// A device's "⋯" menu, made again when what it offers changes (the hub refuses, or a change
    /// is being made): the reason its items wait, with it.
    menus: HashMap<String, (Option<String>, Entity<Menu>)>,
    removal: Option<Removal>,
    /// What the confirmation said last, which it still shows while it fades out.
    removal_said: Removal,
    renaming: Option<String>,
    rename: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
}

impl MachinesSection {
    pub fn new(modals: Entity<Modals>, remote: Remote) -> Self {
        Self {
            modals,
            remote,
            read: None,
            connect_open: false,
            connecting: false,
            connect_code: None,
            connect_name: None,
            connect_icon: 0,
            connect_role: 1,
            connect_error: None,
            connect_replaces: false,
            changing: false,
            looking: None,
            look_icon: 0,
            look_colour: 0,
            kept: None,
            configuring: false,
            members: Listing::default(),
            menus: HashMap::new(),
            removal: None,
            removal_said: Removal::Member(String::new()),
            renaming: None,
            rename: None,
            _subscriptions: Vec::new(),
        }
    }

    /// The machines as last read.
    pub fn machines(&self) -> Option<&Machines> {
        self.read.as_ref()
    }

    pub fn removal(&self) -> Option<&Removal> {
        self.removal.as_ref()
    }

    pub fn connect_is_open(&self) -> bool {
        self.connect_open
    }

    /// The icon chosen in Connect to your hub, as `ICONS` numbers it.
    pub fn connect_icon(&self) -> usize {
        self.connect_icon
    }

    /// Whether a pairing was asked and is not answered yet.
    pub fn is_connecting(&self) -> bool {
        self.connecting
    }

    /// The refusal the dialog shows, when the last pairing was refused.
    pub fn connect_error(&self) -> Option<&SharedString> {
        self.connect_error.as_ref()
    }

    /// Whether the dialog pairs this machine again, after the hub forgot it.
    pub fn connect_replaces(&self) -> bool {
        self.connect_replaces
    }

    /// Whether a change was asked and is not answered yet.
    pub fn is_changing(&self) -> bool {
        self.changing
    }

    /// Whether the list shown is an old one: the hub no longer knows this machine.
    pub fn list_is_old(&self) -> bool {
        self.read.as_ref().is_some_and(|read| read.forgotten())
    }

    pub fn look_is_open(&self) -> bool {
        self.looking.is_some()
    }

    pub fn rename_is_open(&self) -> bool {
        self.renaming.is_some()
    }

    pub fn connect_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.connect_code
            .as_ref()
            .map(|field| field.focus_handle(cx))
    }

    pub fn rename_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.rename.as_ref().map(|field| field.focus_handle(cx))
    }

    /// The fields of the dialogs, made once with the window.
    pub fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connect_code.is_some() {
            return;
        }
        // The code lets a device pair while pair mode lasts: not shown as it is typed.
        let code = cx.new(|cx| {
            let mut field = InputState::new(window, cx).masked(true);
            gpui_design_system::follow_theme(&mut field, cx);
            field
        });
        let name = line_field("laptop", window, cx);
        for input in [&code, &name] {
            self._subscriptions
                .push(cx.subscribe(input, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) && this.connect_error.take().is_some() {
                        cx.notify();
                    }
                }));
        }
        self.connect_code = Some(code);
        self.connect_name = Some(name);
        self.rename = Some(line_field("laptop", window, cx));
    }

    pub fn connect_code_field(&self) -> Option<&Entity<InputState>> {
        self.connect_code.as_ref()
    }

    pub fn connect_name_field(&self) -> Option<&Entity<InputState>> {
        self.connect_name.as_ref()
    }

    pub fn rename_field(&self) -> Option<&Entity<InputState>> {
        self.rename.as_ref()
    }

    /// What the machines' worker said.
    pub fn news(&mut self, news: News, window: &mut Window, cx: &mut Context<Self>) {
        match news {
            News::Read(read) => {
                if read.off.is_none() {
                    self.kept = Some((*read).clone());
                }
                if self.read.as_ref() != Some(&*read) {
                    self.read = Some(*read);
                    cx.notify();
                }
            }
            News::Added {
                name,
                role,
                added_by,
                at,
            } => {
                let this = cx.entity().downgrade();
                let removed = name.clone();
                toast(
                    Toast::success(format!(
                        "New device: {name} ({}), added at {} by {added_by}",
                        role_words(&role),
                        crate::session::clock(at)
                    ))
                    .action("Remove", move |_, cx| {
                        this.update(cx, |this, cx| {
                            this.ask_removal(Removal::Member(removed.clone()), cx)
                        })
                        .ok();
                    }),
                    cx,
                );
            }
            News::Done(asked, done) => self.done(asked, done, window, cx),
        }
    }

    fn done(
        &mut self,
        asked: Asked,
        done: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (asked, done) {
            (Asked::Pair, Ok(())) => {
                self.connecting = false;
                self.connect_open = false;
                self.connect_replaces = false;
                if let Some(code) = &self.connect_code {
                    code.update(cx, |field, cx| field.set_value("", window, cx));
                }
                toast(Toast::success("This machine is connected to your hub"), cx);
            }
            // The refusal stays in the dialog, beside what the user typed.
            (Asked::Pair, Err(why)) => {
                self.connecting = false;
                self.connect_error = Some(why.into());
            }
            (Asked::Configure { on }, done) => {
                self.configuring = false;
                match done {
                    Ok(_) => {
                        let said = if on { "Remote is on" } else { "Remote is off" };
                        toast(Toast::success(said), cx);
                    }
                    Err(why) => toast(Toast::failure("Remote was not changed").detail(why), cx),
                }
            }
            (Asked::Rename(name), Ok(_)) => {
                self.changing = false;
                toast(Toast::success(format!("Renamed {name}")), cx)
            }
            (Asked::Rename(name), Err(why)) => {
                self.changing = false;
                toast(
                    Toast::failure(format!("Not renamed {name}")).detail(why),
                    cx,
                )
            }
            (Asked::Remove(name), Ok(_)) => {
                self.changing = false;
                toast(Toast::success(format!("Removed {name}")), cx)
            }
            (Asked::Remove(name), Err(why)) => {
                self.changing = false;
                toast(
                    Toast::failure(format!("{name} was not removed")).detail(why),
                    cx,
                )
            }
            (Asked::Look(name), Ok(_)) => {
                self.changing = false;
                toast(Toast::success(format!("Changed the look of {name}")), cx)
            }
            (Asked::Look(name), Err(why)) => {
                self.changing = false;
                toast(
                    Toast::failure(format!("The look of {name} was not changed")).detail(why),
                    cx,
                )
            }
        }
        cx.notify();
    }

    // ── What the user does ────────────────────────────────────────────────────────────────

    /// Opens Pair again: the same dialog as Connect to your hub, for a machine the hub forgot.
    pub fn pair_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_connect(window, cx);
        self.connect_replaces = true;
        cx.notify();
    }

    /// Opens Connect to your hub, its card as this machine proposes it.
    pub fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.connect_replaces = false;
        let proposed = self
            .read
            .as_ref()
            .map(|read| read.proposed.clone())
            .unwrap_or_default();
        if let Some(name) = &self.connect_name {
            name.update(cx, |field, cx| field.set_value(proposed.name, window, cx));
        }
        self.connect_icon = ICONS
            .iter()
            .position(|(key, _)| *key == proposed.icon)
            .unwrap_or(0);
        self.connect_role = ROLES
            .iter()
            .position(|(key, _, _)| *key == proposed.role)
            .unwrap_or(1);
        self.connect_error = None;
        self.connect_open = true;
        self.modals
            .update(cx, |modals, cx| modals.open(CONNECT_MODAL, cx));
        cx.notify();
    }

    /// Turns remote on or off.
    fn configure(&mut self, on: bool, cx: &mut Context<Self>) {
        self.configuring = true;
        self.remote.ask(Ask::Configure { on });
        cx.notify();
    }

    pub fn choose_icon(&mut self, icon: usize, cx: &mut Context<Self>) {
        if self.connect_icon != icon {
            self.connect_icon = icon;
            cx.notify();
        }
    }

    pub fn choose_role(&mut self, role: usize, cx: &mut Context<Self>) {
        if self.connect_role != role {
            self.connect_role = role;
            cx.notify();
        }
    }

    /// Connects: the code pasted, the card as the user left it.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        let (Some(code), Some(name)) = (&self.connect_code, &self.connect_name) else {
            return;
        };
        if self.connecting {
            return;
        }
        let code = Zeroizing::new(code.read(cx).value().trim().to_owned());
        let name = name.read(cx).value().trim().to_owned();
        if code.is_empty() || name.is_empty() {
            self.connect_error = Some("Paste the code, and give this machine a name.".into());
            cx.notify();
            return;
        }
        self.connecting = true;
        self.connect_error = None;
        self.remote.ask(Ask::Pair {
            code,
            name,
            icon: ICONS[self.connect_icon].0.to_owned(),
            role: ROLES[self.connect_role].0.to_owned(),
            replace: self.connect_replaces,
        });
        cx.notify();
    }

    /// Asks to confirm a removal: the one confirmation dialog.
    pub fn ask_removal(&mut self, removal: Removal, cx: &mut Context<Self>) {
        self.removal_said = removal.clone();
        self.removal = Some(removal);
        cx.notify();
    }

    fn confirm_removal(&mut self, cx: &mut Context<Self>) {
        match self.removal.take() {
            Some(Removal::Member(name)) => {
                self.changing = true;
                self.remote.ask(Ask::Remove { name })
            }
            Some(Removal::Remote) => self.configure(false, cx),
            None => {}
        }
        cx.notify();
    }

    fn cancel_removal(&mut self, cx: &mut Context<Self>) {
        if self.removal.take().is_some() {
            cx.notify();
        }
    }

    pub fn ask_rename(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = &self.rename {
            field.update(cx, |field, cx| field.set_value(name.clone(), window, cx));
        }
        self.renaming = Some(name);
        self.modals
            .update(cx, |modals, cx| modals.open(RENAME_MODAL, cx));
        cx.notify();
    }

    fn rename(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.renaming.clone() else {
            return;
        };
        let new_name = self
            .rename
            .as_ref()
            .map(|field| field.read(cx).value().trim().to_owned())
            .unwrap_or_default();
        if new_name.is_empty() || new_name == name {
            return;
        }
        self.renaming = None;
        self.changing = true;
        self.remote.ask(Ask::Rename { name, new_name });
        cx.notify();
    }

    /// Opens Change look: the icon and colour the device has now, chosen already.
    pub fn ask_look(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(member) = self
            .read
            .as_ref()
            .and_then(|read| read.members.iter().find(|member| member.name == name))
        else {
            return;
        };
        let (icon, colour) = member.look();
        self.look_icon = ICONS.iter().position(|(key, _)| *key == icon).unwrap_or(0);
        self.look_colour = COLOURS
            .iter()
            .position(|(key, _, _)| *key == colour)
            .unwrap_or(0);
        self.looking = Some(name);
        self.modals
            .update(cx, |modals, cx| modals.open(LOOK_MODAL, cx));
        cx.notify();
    }

    pub fn choose_look_icon(&mut self, icon: usize, cx: &mut Context<Self>) {
        if self.look_icon != icon {
            self.look_icon = icon;
            cx.notify();
        }
    }

    pub fn choose_look_colour(&mut self, colour: usize, cx: &mut Context<Self>) {
        if self.look_colour != colour {
            self.look_colour = colour;
            cx.notify();
        }
    }

    /// Saves the look chosen: a change of the list, signed with this device's key.
    fn save_look(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.looking.take() else {
            return;
        };
        self.changing = true;
        self.remote.ask(Ask::Look {
            name,
            icon: ICONS[self.look_icon].0.to_owned(),
            colour: COLOURS[self.look_colour].0.to_owned(),
        });
        cx.notify();
    }

    // ── The dialogs ───────────────────────────────────────────────────────────────────────

    /// Connect to your hub: the code, then the card this machine proposes, confirmed.
    pub fn connect_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Modal {
        let this = cx.entity().downgrade();
        let chosen = cx.entity().downgrade();
        // Five kinds: a select, as the kit's radio choices take four at most.
        let icons = Select::new(
            "connect-icon",
            ICONS
                .iter()
                .map(|(key, title)| SelectOption::new(*key, *title)),
            self.connect_icon,
        )
        .on_choose(move |at, _, cx| {
            chosen.update(cx, |this, cx| this.choose_icon(at, cx)).ok();
        });
        let chosen = cx.entity().downgrade();
        let roles = ROLES
            .iter()
            .fold(
                RadioChoices::new("connect-role", "This device can"),
                |choices, (key, title, line)| choices.choice(*key, *title, *line),
            )
            .chosen(self.connect_role)
            .on_choose(move |at, _, cx| {
                chosen.update(cx, |this, cx| this.choose_role(at, cx)).ok();
            });
        let code = self.connect_code.clone().map(|field| {
            spaced(
                space::LG,
                Field::new(
                    "connect-code-field",
                    "Pair code",
                    boxed("connect-code", Input::new(&field), cx),
                )
                .hint("It holds your hub's address and key: nothing else to type."),
            )
        });
        let name = self.connect_name.clone().map(|field| {
            spaced(
                space::LG,
                Field::new(
                    "connect-name-field",
                    "Name",
                    boxed("connect-name", Input::new(&field), cx),
                ),
            )
        });
        let (title, said, save) = match self.connect_replaces {
            true => ("Pair again", PAIR_AGAIN_SAID, "Pair again"),
            false => ("Connect to your hub", CONNECT_SAID, "Connect"),
        };
        Modal::new(CONNECT_MODAL, title)
            .sentence(said)
            .open(self.connect_open)
            .takes_focus()
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.connect_open = false;
                    cx.notify();
                })
                .ok();
            })
            .child(
                stack(space::LG)
                    .children(code)
                    .children(name)
                    .child(spaced(
                        space::LG,
                        Field::new("connect-icon-field", "This device is", icons),
                    ))
                    .child(spaced(
                        space::LG,
                        Field::new("connect-role-field", "This device can", roles),
                    ))
                    .child(problem_line(
                        "connect-problem",
                        self.connect_error.clone(),
                        space::MD,
                        cx,
                    )),
            )
            .action(
                Button::primary("connect-save", save)
                    .disabled(self.connecting)
                    .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
            )
    }

    pub fn rename_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Modal {
        let said = self.renaming.clone().unwrap_or_default();
        let this = cx.entity().downgrade();
        let field = self.rename.clone().map(|field| {
            Field::new(
                "member-rename-field",
                "Name",
                boxed("member-rename-name", Input::new(&field), cx),
            )
        });
        Modal::new(RENAME_MODAL, format!("Rename {said}"))
            .open(self.renaming.is_some())
            .takes_focus()
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.renaming = None;
                    cx.notify();
                })
                .ok();
            })
            .children(field)
            .action(
                Button::primary("member-rename-save", "Rename")
                    .disabled(self.changing)
                    .on_click(cx.listener(|this, _, _, cx| this.rename(cx))),
            )
    }

    /// Change look: the five icons as a select, the eight colours as swatches, and the row as it
    /// will be, which follows the choice before anything is saved.
    pub fn look_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Modal {
        let said = self.looking.clone().unwrap_or_default();
        let this = cx.entity().downgrade();
        let chosen = cx.entity().downgrade();
        let icons = Select::new(
            "look-icon",
            ICONS
                .iter()
                .map(|(key, title)| SelectOption::new(*key, *title)),
            self.look_icon,
        )
        .on_choose(move |at, _, cx| {
            chosen
                .update(cx, |this, cx| this.choose_look_icon(at, cx))
                .ok();
        });
        let chosen = cx.entity().downgrade();
        let colours = COLOURS
            .iter()
            .fold(
                Swatches::new("look-colour", "This device's colour"),
                |swatches, (key, title, _)| swatches.swatch(Swatch::new(*key, *title, shade(key))),
            )
            .chosen(self.look_colour)
            .on_choose(move |at, _, cx| {
                chosen
                    .update(cx, |this, cx| this.choose_look_colour(at, cx))
                    .ok();
            });
        let words = SharedString::from(format!(
            "{} in {}",
            ICONS[self.look_icon].0, COLOURS[self.look_colour].0
        ));
        let p = palette(cx);
        let name = SharedString::from(said.clone());
        let preview = changing("look-preview-fade", words, move |words| {
            let (icon, colour) = words.split_once(" in ").unwrap_or(("desktop", "grey"));
            div()
                .id("look-preview")
                .aria_label(words.clone())
                .flex()
                .items_center()
                .gap(space::SM)
                .child(mark(icon, colour))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(p.text)
                        .child(name.clone()),
                )
                .test_support()
                .into_any_element()
        });
        let _ = window;
        Modal::new(LOOK_MODAL, format!("Change the look of {said}"))
            .sentence("How this device is drawn here and on your other machines.")
            .open(self.looking.is_some())
            .takes_focus()
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.looking = None;
                    cx.notify();
                })
                .ok();
            })
            .child(
                stack(space::LG)
                    .child(spaced(
                        space::LG,
                        Field::new("look-icon-field", "Icon", icons),
                    ))
                    .child(spaced(
                        space::LG,
                        Field::new("look-colour-field", "Colour", colours),
                    ))
                    .child(spaced(
                        space::LG,
                        Field::new("look-preview-field", "Preview", preview),
                    )),
            )
            .action(
                Button::primary("member-look-save", "Save")
                    .disabled(self.changing)
                    .on_click(cx.listener(|this, _, _, cx| this.save_look(cx))),
            )
    }

    // ── The page ──────────────────────────────────────────────────────────────────────────

    /// Whether remote is on: its notice and **Turn on** while it is off, the **Remote** switch once
    /// it is. Each folds away as the other unfolds.
    fn remote_section(&mut self, off: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let notice = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("remote-notice")
                    .aria_label(REMOTE_OFF)
                    .text_size(px(13.))
                    .text_color(p.mute)
                    .child(REMOTE_OFF)
                    .test_support(),
            )
            .child(spaced(
                space::MD,
                div().child(
                    Button::primary("turn-on-remote", "Turn on")
                        .disabled(self.configuring)
                        .on_click(cx.listener(|this, _, _, cx| this.configure(true, cx))),
                ),
            ));
        let switch = Switch::new("remote-switch", true, "Remote")
            .hint(REMOTE_HINT)
            .disabled(self.configuring)
            .on_click(cx.listener(|this, _, _, cx| this.ask_removal(Removal::Remote, cx)));
        div()
            .id("remote")
            .flex()
            .flex_col()
            .child(presence("remote-off-block", off, notice))
            .child(presence("remote-on-block", !off, switch))
            .test_support()
    }

    /// With no hub: what a hub is, and the one way in.
    fn connect_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        Section::new("hub-connect", "Connect to your hub")
            .sentence(CONNECT_SENTENCE)
            .child(spaced(
                space::LG,
                Button::primary("connect-hub", "Connect to your hub")
                    .disabled(self.connecting)
                    .on_click(cx.listener(|this, _, window, cx| this.open_connect(window, cx))),
            ))
    }

    /// The hub: its address, whether it answers, and when it does not, which address was tried
    /// and how it failed.
    fn hub_section(&mut self, hub: &Hub, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let row = ListRow::new("hub-state", hub.url.clone())
            .mono()
            .tag(hub.state_words());
        let reason = hub
            .reason
            .as_deref()
            .map(|reason| SharedString::from(how_it_failed(reason).to_owned()));
        Section::new("hub", "Hub").sentence(HUB_SENTENCE).child(
            stack(space::MD)
                .child(spaced(space::MD, ListCard::new("hub-card").row(row)))
                .child(
                    presence_of("hub-reason-block", reason, move |said| {
                        // A reason that changes while it is shown fades back in.
                        changing("hub-reason-text", said, move |said| {
                            div()
                                .id("hub-reason")
                                .aria_label(said.clone())
                                .text_size(px(12.5))
                                .text_color(p.mute)
                                .child(said)
                                .test_support()
                                .into_any_element()
                        })
                        .into_any_element()
                    })
                    .space(space::MD),
                ),
        )
    }

    /// The "⋯" menu of a device: Rename, Change look… and Remove…, each waiting, with its reason,
    /// while the hub refuses or a change is being made. Made again when that reason changes.
    fn member_menu(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let reason = match (&self.read, self.changing) {
            (Some(read), _) if read.hub.as_ref().is_some_and(|hub| !hub.usable()) => {
                read.hub.as_ref().map(|hub| hub.waits_words().to_owned())
            }
            (_, true) => Some("A change is being made".to_owned()),
            _ => None,
        };
        if let Some((kept, menu)) = self.menus.get(name)
            && *kept == reason
        {
            return menu.clone();
        }
        let id = ElementId::Name(format!("member-more-{name}").into());
        let (renaming, looking, removing) = (
            cx.entity().downgrade(),
            cx.entity().downgrade(),
            cx.entity().downgrade(),
        );
        let (renamed, looked, removed) = (name.to_owned(), name.to_owned(), name.to_owned());
        let waits = reason.clone();
        let menu = cx.new(|cx| {
            let mut menu = Menu::new(id, "More", cx).entry(
                ElementId::Name(format!("member-rename-{renamed}").into()),
                "Rename",
                Some(Icon::Edit),
                move |window, cx| {
                    renaming
                        .update(cx, |this, cx| this.ask_rename(renamed.clone(), window, cx))
                        .ok();
                },
            );
            if let Some(why) = &waits {
                menu = menu.disabled(why.clone());
            }
            menu = menu.entry(
                ElementId::Name(format!("member-look-{looked}").into()),
                "Change look…",
                Some(crate::glyphs::LOOK),
                move |_, cx| {
                    looking
                        .update(cx, |this, cx| this.ask_look(looked.clone(), cx))
                        .ok();
                },
            );
            if let Some(why) = &waits {
                menu = menu.disabled(why.clone());
            }
            menu = menu.danger(
                ElementId::Name(format!("member-remove-{removed}").into()),
                "Remove…",
                move |_, cx| {
                    removing
                        .update(cx, |this, cx| {
                            this.ask_removal(Removal::Member(removed.clone()), cx)
                        })
                        .ok();
                },
            );
            if let Some(why) = &waits {
                menu = menu.disabled(why.clone());
            }
            menu
        });
        self.menus.insert(name.to_owned(), (reason, menu.clone()));
        menu
    }

    fn member_row(
        &mut self,
        member: &Member,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ListRow {
        let name = member.name.clone();
        let menu = self.member_menu(&name, cx);
        let read = self.read.clone().unwrap_or_default();
        let this_one = read.this.as_deref() == Some(member.name.as_str());
        let state = if this_one {
            "this machine".to_owned()
        } else {
            match read.seen(member) {
                Some(machine) => machine.state_words(),
                None if member.machine.is_none() => "no machine".to_owned(),
                None => "not linked".to_owned(),
            }
        };
        let added = match member.added_by.as_str() {
            by if by.is_empty() || by == member.name => "The list's first device.".to_owned(),
            by => format!("Added by {by}."),
        };
        let said = ROLES
            .iter()
            .find(|(key, _, _)| *key == member.role)
            .map_or(member.role.as_str(), |(_, words, _)| words);
        // Where a machine is reached, or why not: said for each machine this one links to.
        let address = (!this_one)
            .then(|| read.seen(member))
            .flatten()
            .map(|machine| {
                let usable = read.hub.as_ref().is_none_or(|hub| hub.usable());
                address_line(
                    format!("member-address-{name}"),
                    machine.address_words(usable).into(),
                    window,
                    cx,
                )
            });
        let (icon, colour) = member.look();
        let words = SharedString::from(member.look_words());
        let leading = div()
            .id(ElementId::Name(format!("member-icon-{name}").into()))
            .aria_label(words)
            .flex_none()
            .child(mark(&icon, &colour))
            .test_support();
        ListRow::new(ElementId::Name(format!("member-{name}").into()), name)
            .leading(leading)
            .tag(state)
            .line(format!("{said}. {added}"))
            .when_some(address, |row, address| row.below(address))
            .more(menu)
    }

    fn devices_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let listed = self.read.as_ref().map(|read| read.members.clone());
        if let Some(listed) = listed {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.members
                .sync(listed.into_iter().map(|m| (m.name.clone(), m)), now, reduce);
        }
        let drawn: Vec<_> = self
            .members
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("member-list");
        for (key, placement, member) in drawn {
            let row = self.member_row(&member, window, cx);
            list = list.item(
                ElementId::Name(format!("member-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.members.len();
        let forgotten = self.read.as_ref().is_some_and(|read| read.forgotten());
        // Said once: a hub that forgot this machine says it in the banner, not in this line too.
        let hub_down = self
            .read
            .as_ref()
            .and_then(|read| read.hub.as_ref())
            .filter(|hub| !hub.forgotten)
            .and_then(|hub| hub.needs_words());
        // The list a hub that forgot this machine left is old: it fades to say so.
        let old = fade("member-list-old", forgotten, PANEL, window, cx);
        let list = div().opacity(1.0 - 0.5 * old).child(list);
        let banner = div()
            .flex()
            .items_center()
            .gap(space::MD)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(
                        div()
                            .id("devices-forgotten")
                            .aria_label(FORGOTTEN)
                            .text_size(px(13.))
                            .text_color(p.text)
                            .child(FORGOTTEN)
                            .test_support(),
                    )
                    .child(
                        div()
                            .id("devices-forgotten-sentence")
                            .aria_label(FORGOTTEN_SENTENCE)
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(FORGOTTEN_SENTENCE)
                            .test_support(),
                    ),
            )
            .child(
                Button::primary("pair-again", "Pair again")
                    .disabled(self.connecting)
                    .on_click(cx.listener(|this, _, window, cx| this.pair_again(window, cx))),
            );
        Section::new("devices", "Devices")
            .count(count.to_string())
            .sentence(DEVICES_SENTENCE)
            .child(
                stack(space::MD)
                    .child(presence("devices-forgotten-block", forgotten, banner).space(space::MD))
                    .child(
                        presence_of(
                            "devices-hub-down",
                            hub_down.map(SharedString::from),
                            move |said| {
                                changing("devices-hub-down-text", said, move |said| {
                                    div()
                                        .id("devices-hub-down-line")
                                        .aria_label(said.clone())
                                        .text_size(px(12.5))
                                        .text_color(p.mute)
                                        .child(said)
                                        .test_support()
                                        .into_any_element()
                                })
                                .into_any_element()
                            },
                        )
                        .space(space::MD),
                    )
                    .child(presence("member-list-block", count > 0, list).space(space::MD)),
            )
    }

    fn removal_confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let (question, sentence, action) = match &self.removal_said {
            Removal::Member(name)
                if self.read.as_ref().and_then(|read| read.this.as_deref()) == Some(name) =>
            {
                (
                    format!("Remove {name}, this machine?"),
                    "This machine leaves the hub and its links close; it comes back only paired \
                     again on your hub.",
                    "Remove device",
                )
            }
            Removal::Member(name) => (
                format!("Remove {name}?"),
                "Its links are cut on every machine at once; it comes back only paired again on \
                 your hub.",
                "Remove device",
            ),
            Removal::Remote => (
                "Turn remote off?".to_owned(),
                "The links close; this machine stays paired with your hub.",
                "Turn off",
            ),
        };
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("member-removal", question)
            .sentence(sentence)
            .action(action)
            .open(self.removal.is_some())
            .on_confirm(move |_, cx| {
                this.update(cx, |this, cx| this.confirm_removal(cx)).ok();
            })
            .on_cancel(move |_, cx| {
                again.update(cx, |this, cx| this.cancel_removal(cx)).ok();
            })
    }

    /// The sections of the tab, top to bottom, each entering on its own: whether remote is on, then
    /// (while it is) the hub and the devices: every link goes through the hub, so no address is shown. They
    /// fold away, as they were, when remote is turned off.
    pub fn blocks(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, gpui_kit::AnyElement)> {
        let Some(read) = self.read.clone() else {
            let waiting = gpui_design_system::Skeleton::new("machines-skeleton").h(px(120.));
            return vec![("machines-loading", waiting.into_any_element())];
        };
        let off = read.off.is_some();
        let mut blocks = vec![("remote", self.remote_section(off, cx).into_any_element())];
        let shown = if off { self.kept.clone() } else { Some(read) };
        if let Some(shown) = shown {
            let live = self.read.replace(shown);
            let rest = self.rest(window, cx);
            self.read = live;
            let column = div()
                .flex()
                .flex_col()
                .children(rest.into_iter().enumerate().map(|(n, (part, block))| {
                    div().when(n > 0, |d| d.mt(space::XXL)).child(enter(
                        ElementId::Name(format!("enter-machines-{part}").into()),
                        n,
                        block,
                    ))
                }));
            blocks.push((
                "machines-rest",
                presence("machines-rest", !off, column)
                    .space(space::XXL)
                    .into_any_element(),
            ));
        }
        blocks
    }

    /// What remote on shows under its switch.
    fn rest(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, gpui_kit::AnyElement)> {
        let read = self.read.clone().unwrap_or_default();
        let mut blocks = Vec::new();
        // A hub that forgot this machine keeps its old list on the page, and Pair again.
        if !read.member() && !read.forgotten() {
            blocks.push(("hub-connect", self.connect_section(cx).into_any_element()));
            return blocks;
        }
        let hub = read.hub.as_ref().map(|hub| self.hub_section(hub, cx));
        blocks.extend(hub.map(|hub| ("hub", hub.into_any_element())));
        blocks.push((
            "devices",
            self.devices_section(window, cx).into_any_element(),
        ));
        blocks
    }
}

impl Render for MachinesSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fields(window, cx);
        let blocks = self.blocks(window, cx);
        div()
            .id("machines-blocks")
            .flex()
            .flex_col()
            .children(blocks.into_iter().enumerate().map(|(n, (part, block))| {
                // The settings under the switch hold their own space and enter section by section.
                if part == "machines-rest" {
                    return div().child(block);
                }
                div().when(n > 0, |d| d.mt(space::XXL)).child(enter(
                    ElementId::Name(format!("enter-machines-{part}").into()),
                    n,
                    block,
                ))
            }))
            .test_support()
    }
}

/// The confirmation of a removal, drawn by the window's host of dialogs over any page: the toast
/// of a new member asks it from wherever the user is.
pub struct RemovalDialog {
    section: Entity<MachinesSection>,
    open: bool,
    _watching: Subscription,
}

impl RemovalDialog {
    pub fn new(section: Entity<MachinesSection>, cx: &mut Context<Self>) -> Self {
        Self {
            _watching: cx.observe(&section, |this, section, cx| {
                this.open = section.read(cx).removal().is_some();
                cx.notify();
            }),
            section,
            open: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
}

impl Render for RemovalDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.section
            .update(cx, |section, cx| section.removal_confirm(cx))
    }
}

/// The line under a machine's row that says where it is reached, or why it is not: the text
/// fades back in when it changes, and is cut with its whole text in a tooltip when it is wider
/// than the row.
fn address_line(
    id: String,
    said: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let p = palette(cx);
    let id = ElementId::Name(id.into());
    let fading = changed(&id, &said, window, cx);
    let line = div()
        .id(id)
        .aria_label(said.clone())
        .opacity(fading)
        .text_size(px(12.5))
        .text_color(p.faint)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(said.clone());
    tipped_when_cut(line, said, window, cx)
}

/// A device's mark: its icon in its colour, saying both for what the colour cannot (`server in
/// teal`). The element is the caller's to name.
fn mark(icon: &str, colour: &str) -> impl IntoElement {
    gpui_design_system::icon(crate::glyphs::device(icon))
        .size(px(16.))
        .text_color(shade(colour))
}
