//! Settings, General, Machines (`docs/remote.md`, "The hub", "Pairing", "Roles"): **Connect to your
//! hub** while this machine is in no member list, a dialog where the code `flottille-hub pair`
//! printed is pasted and the card this machine proposes confirmed; then the devices of the list,
//! each renamed or removed from its "⋯", and the addresses this machine is reached at. A device is
//! added on the hub only: every new member is a toast. What is asked goes to this machine's daemon, signed with the
//! device key ([`crate::machines::Remote`]); what it answers comes back as news.

use std::collections::HashMap;

use gpui_design_system::{
    Button, Confirm, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, RadioChoices,
    Section, Select, SelectOption, Toast, changed, changing, enter, palette, presence, presence_of,
    space, tipped_when_cut, toast,
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
    Ask, Asked, Hub, ICONS, Machines, Member, News, ROLES, Remote, how_it_failed, role_words,
};
use crate::modals::Modals;

/// The dialog that connects this machine to the user's hub, drawn by the window's host of dialogs.
pub const CONNECT_MODAL: &str = "hub-connect";
/// The dialog that renames a device.
pub const RENAME_MODAL: &str = "member-rename";
/// The confirmation of a removal, over any page.
pub const REMOVAL_DIALOG: &str = "member-removal-dialog";
/// The dialog that declares an address.
pub const ADDRESS_MODAL: &str = "address-new";

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
                            list of your devices. It carries nothing of what they say to each \
                            other.";
const ADDRESSES_SENTENCE: &str = "Where your other machines reach this one: found on its network, \
                                  or declared by hand (a VPN address, a name, a forwarded port).";

/// What is asked to be removed, until it is confirmed or cancelled.
#[derive(Clone, Debug, PartialEq)]
pub enum Removal {
    Member(String),
    Address(String),
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
    members: Listing<Member>,
    addresses: Listing<crate::machines::Address>,
    menus: HashMap<String, Entity<Menu>>,
    removal: Option<Removal>,
    /// What the confirmation said last, which it still shows while it fades out.
    removal_said: Removal,
    renaming: Option<String>,
    rename: Option<Entity<InputState>>,
    adding_address: bool,
    address: Option<Entity<InputState>>,
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
            members: Listing::default(),
            addresses: Listing::default(),
            menus: HashMap::new(),
            removal: None,
            removal_said: Removal::Member(String::new()),
            renaming: None,
            rename: None,
            adding_address: false,
            address: None,
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

    pub fn rename_is_open(&self) -> bool {
        self.renaming.is_some()
    }

    pub fn address_is_open(&self) -> bool {
        self.adding_address
    }

    pub fn connect_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.connect_code
            .as_ref()
            .map(|field| field.focus_handle(cx))
    }

    pub fn rename_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.rename.as_ref().map(|field| field.focus_handle(cx))
    }

    pub fn address_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.address.as_ref().map(|field| field.focus_handle(cx))
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
        self.address = Some(line_field("100.64.0.7", window, cx));
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

    pub fn address_field(&self) -> Option<&Entity<InputState>> {
        self.address.as_ref()
    }

    /// What the machines' worker said.
    pub fn news(&mut self, news: News, window: &mut Window, cx: &mut Context<Self>) {
        match news {
            News::Read(read) => {
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
            (Asked::Rename(name), Ok(_)) => toast(Toast::success(format!("Renamed {name}")), cx),
            (Asked::Rename(name), Err(why)) => toast(
                Toast::failure(format!("Not renamed {name}")).detail(why),
                cx,
            ),
            (Asked::Remove(name), Ok(_)) => toast(Toast::success(format!("Removed {name}")), cx),
            (Asked::Remove(name), Err(why)) => toast(
                Toast::failure(format!("{name} was not removed")).detail(why),
                cx,
            ),
            (Asked::AddressAdd(address), Ok(_)) => {
                toast(Toast::success(format!("Declared {address}")), cx)
            }
            (Asked::AddressAdd(address), Err(why)) => toast(
                Toast::failure(format!("{address} was not declared")).detail(why),
                cx,
            ),
            (Asked::AddressRemove(address), Ok(_)) => {
                toast(Toast::success(format!("Removed {address}")), cx)
            }
            (Asked::AddressRemove(address), Err(why)) => toast(
                Toast::failure(format!("{address} was not removed")).detail(why),
                cx,
            ),
        }
        cx.notify();
    }

    // ── What the user does ────────────────────────────────────────────────────────────────

    /// Opens Connect to your hub, its card as this machine proposes it.
    pub fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            Some(Removal::Member(name)) => self.remote.ask(Ask::Remove { name }),
            Some(Removal::Address(address)) => self.remote.ask(Ask::AddressRemove(address)),
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
        self.remote.ask(Ask::Rename { name, new_name });
        cx.notify();
    }

    pub fn ask_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = &self.address {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.adding_address = true;
        self.modals
            .update(cx, |modals, cx| modals.open(ADDRESS_MODAL, cx));
        cx.notify();
    }

    fn declare(&mut self, cx: &mut Context<Self>) {
        let address = self
            .address
            .as_ref()
            .map(|field| field.read(cx).value().trim().to_owned())
            .unwrap_or_default();
        if address.is_empty() {
            return;
        }
        self.adding_address = false;
        self.remote.ask(Ask::AddressAdd(address));
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
        Modal::new(CONNECT_MODAL, "Connect to your hub")
            .sentence(CONNECT_SAID)
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
                Button::primary("connect-save", "Connect")
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
                    .on_click(cx.listener(|this, _, _, cx| this.rename(cx))),
            )
    }

    pub fn address_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Modal {
        let this = cx.entity().downgrade();
        let field = self.address.clone().map(|field| {
            Field::new(
                "address-new-field",
                "Address",
                boxed("address-new-value", Input::new(&field), cx),
            )
            .hint("A host name or an IP address, and its port when it is not the link's own.")
        });
        Modal::new(ADDRESS_MODAL, "New address")
            .open(self.adding_address)
            .takes_focus()
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.adding_address = false;
                    cx.notify();
                })
                .ok();
            })
            .children(field)
            .action(
                Button::primary("address-new-save", "Declare")
                    .on_click(cx.listener(|this, _, _, cx| this.declare(cx))),
            )
    }

    // ── The page ──────────────────────────────────────────────────────────────────────────

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

    fn member_row(
        &mut self,
        member: &Member,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ListRow {
        let name = member.name.clone();
        let this = cx.entity().downgrade();
        let menu = menu_of(&mut self.menus, "member", &name, cx, move |key, menu| {
            let (renamed, removed) = (key.to_owned(), key.to_owned());
            let (renaming, removing) = (this.clone(), this);
            menu.entry(
                ElementId::Name(format!("member-rename-{key}").into()),
                "Rename",
                Some(Icon::Edit),
                move |window, cx| {
                    renaming
                        .update(cx, |this, cx| this.ask_rename(renamed.clone(), window, cx))
                        .ok();
                },
            )
            .danger(
                ElementId::Name(format!("member-remove-{key}").into()),
                "Remove…",
                move |_, cx| {
                    removing
                        .update(cx, |this, cx| {
                            this.ask_removal(Removal::Member(removed.clone()), cx)
                        })
                        .ok();
                },
            )
        });
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
        ListRow::new(ElementId::Name(format!("member-{name}").into()), name)
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
        let hub_down = self
            .read
            .as_ref()
            .and_then(|read| read.hub.as_ref())
            .and_then(|hub| hub.needs_words());
        Section::new("devices", "Devices")
            .count(count.to_string())
            .sentence(DEVICES_SENTENCE)
            .child(
                stack(space::MD)
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

    fn address_row(
        &mut self,
        address: &crate::machines::Address,
        cx: &mut Context<Self>,
    ) -> ListRow {
        let key = address.address.clone();
        let row = ListRow::new(
            ElementId::Name(format!("address-{key}").into()),
            key.clone(),
        )
        .mono()
        .tag(address.how.clone());
        if address.how != "declared" {
            return row;
        }
        let this = cx.entity().downgrade();
        let menu = menu_of(&mut self.menus, "address", &key, cx, move |key, menu| {
            let removed = key.to_owned();
            menu.danger(
                ElementId::Name(format!("address-remove-{key}").into()),
                "Remove…",
                move |_, cx| {
                    this.update(cx, |this, cx| {
                        this.ask_removal(Removal::Address(removed.clone()), cx)
                    })
                    .ok();
                },
            )
        });
        row.more(menu)
    }

    fn addresses_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let listed = self.read.as_ref().map(|read| read.addresses.clone());
        if let Some(listed) = listed {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.addresses.sync(
                listed.into_iter().map(|a| (a.address.clone(), a)),
                now,
                reduce,
            );
        }
        let drawn: Vec<_> = self
            .addresses
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("address-list");
        for (key, placement, address) in drawn {
            let row = self.address_row(&address, cx);
            list = list.item(
                ElementId::Name(format!("address-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.addresses.len();
        let empty = self.read.as_ref().is_some_and(|r| r.addresses.is_empty());
        Section::new("addresses", "Addresses")
            .count(count.to_string())
            .sentence(ADDRESSES_SENTENCE)
            .action(
                Button::secondary("add-address", "New address")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.ask_address(window, cx))),
            )
            .child(
                stack(space::MD)
                    .child(presence("address-list-block", count > 0, list).space(space::MD))
                    .child(
                        presence(
                            "no-address-block",
                            empty,
                            EmptyLine::new(
                                "no-address",
                                Icon::Service,
                                "No address yet: this machine listens on none. Declare one your \
                                 other machines reach it at.",
                            ),
                        )
                        .space(space::MD),
                    ),
            )
    }

    fn removal_confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let (question, sentence, action) = match &self.removal_said {
            Removal::Member(name) => (
                format!("Remove {name}?"),
                "Its links are cut on every machine at once; it comes back only paired again on \
                 your hub.",
                "Remove device",
            ),
            Removal::Address(address) => (
                format!("Remove the address {address}?"),
                "Your other machines no longer try this machine there.",
                "Remove address",
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

    /// The sections of the tab, top to bottom, each entering on its own.
    pub fn blocks(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, gpui_kit::AnyElement)> {
        let Some(read) = self.read.clone() else {
            let waiting = gpui_design_system::Skeleton::new("machines-skeleton").h(px(120.));
            return vec![("machines-loading", waiting.into_any_element())];
        };
        if let Some(off) = read.off {
            let line = EmptyLine::new(
                "machines-off",
                Icon::Service,
                format!("Your other machines are not reached from this one: {off}"),
            );
            return vec![("machines-off", line.into_any_element())];
        }
        if !read.member() {
            return vec![("hub-connect", self.connect_section(cx).into_any_element())];
        }
        let hub = read.hub.as_ref().map(|hub| self.hub_section(hub, cx));
        let mut blocks = Vec::new();
        blocks.extend(hub.map(|hub| ("hub", hub.into_any_element())));
        blocks.push((
            "devices",
            self.devices_section(window, cx).into_any_element(),
        ));
        blocks.push(("addresses", self.addresses_section(cx).into_any_element()));
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
