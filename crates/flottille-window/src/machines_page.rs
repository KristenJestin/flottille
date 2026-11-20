//! Settings, General, Machines (`docs/remote.md`, "The hub", "Pairing", "Roles"): **Set up a hub** while
//! this machine is in no member list; then the devices of the list, each renamed or removed from
//! its "⋯", **Add a device** with the invitation's QR and its code until it expires, and the
//! addresses this machine is reached at. A join request is the window's dialog wherever the user
//! is, and every new member a toast. What is asked goes to this machine's daemon, signed with the
//! device key ([`crate::machines::Remote`]); what it answers comes back as news.

use std::collections::HashMap;
use std::time::Duration;

use gpui_design_system::{
    Button, Card, Confirm, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, QrCode,
    RadioChoices, Section, Toast, changed, changing, enter, palette, presence, presence_of, space,
    tipped_when_cut, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window, div, px,
};
use zeroize::Zeroizing;

use crate::commands::{boxed, line_field, menu_of, problem_line, spaced, stack};
use crate::machines::{
    Ask, Asked, Hub, Invitation, JoinRequest, Machines, Member, News, ROLES, Remote, how_it_failed,
    role_words,
};
use crate::modals::Modals;

/// The dialog of a join request, drawn by the window's host of dialogs.
pub const JOIN_MODAL: &str = "join-request";
/// The dialog that renames a device.
pub const RENAME_MODAL: &str = "member-rename";
/// The confirmation of a removal, over any page.
pub const REMOVAL_DIALOG: &str = "member-removal-dialog";
/// The dialog that declares an address.
pub const ADDRESS_MODAL: &str = "address-new";

const SETUP_SENTENCE: &str = "Your machines find each other through a hub you run on a host that \
                              is always on. Give its address and the setup code it printed: this \
                              machine becomes its first device.";
const DEVICES_SENTENCE: &str = "Each device in this list reaches your machines with the role it \
                                was given. Add one from here, and check the four words on both \
                                screens.";
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
    url: Option<Entity<InputState>>,
    code: Option<Entity<InputState>>,
    setting_up: bool,
    setup_error: Option<SharedString>,
    inviting: bool,
    invitation: Option<Invitation>,
    /// The last invitation's code, drawn while its panel folds away.
    shown_code: SharedString,
    expiry: Option<Task<()>>,
    members: Listing<Member>,
    addresses: Listing<crate::machines::Address>,
    menus: HashMap<String, Entity<Menu>>,
    removal: Option<Removal>,
    /// What the confirmation said last, which it still shows while it fades out.
    removal_said: Removal,
    /// The join request shown in the dialog, the name typed and the role chosen.
    join: Option<JoinRequest>,
    join_said: Option<JoinRequest>,
    join_name: Option<Entity<InputState>>,
    join_role: usize,
    /// The requests the user closed without answering: not shown again.
    set_aside: Vec<String>,
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
            url: None,
            code: None,
            setting_up: false,
            setup_error: None,
            inviting: false,
            invitation: None,
            shown_code: SharedString::default(),
            expiry: None,
            members: Listing::default(),
            addresses: Listing::default(),
            menus: HashMap::new(),
            removal: None,
            removal_said: Removal::Member(String::new()),
            join: None,
            join_said: None,
            join_name: None,
            join_role: 1,
            set_aside: Vec::new(),
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

    pub fn invitation(&self) -> Option<&Invitation> {
        self.invitation.as_ref()
    }

    /// Whether an invitation is being asked of the hub.
    pub fn is_inviting(&self) -> bool {
        self.inviting
    }

    pub fn join(&self) -> Option<&JoinRequest> {
        self.join.as_ref()
    }

    pub fn removal(&self) -> Option<&Removal> {
        self.removal.as_ref()
    }

    pub fn join_is_open(&self) -> bool {
        self.join.is_some()
    }

    pub fn rename_is_open(&self) -> bool {
        self.renaming.is_some()
    }

    pub fn address_is_open(&self) -> bool {
        self.adding_address
    }

    pub fn join_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.join_name.as_ref().map(|field| field.focus_handle(cx))
    }

    pub fn rename_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.rename.as_ref().map(|field| field.focus_handle(cx))
    }

    pub fn address_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.address.as_ref().map(|field| field.focus_handle(cx))
    }

    /// The fields of the setup, of the join dialog and of the others, made once with the window.
    pub fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.url.is_some() {
            return;
        }
        let url = line_field("https://hub.example.net", window, cx);
        let code = cx.new(|cx| {
            let mut field = InputState::new(window, cx).masked(true);
            gpui_design_system::follow_theme(&mut field, cx);
            field
        });
        for input in [&url, &code] {
            self._subscriptions
                .push(cx.subscribe(input, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) && this.setup_error.take().is_some() {
                        cx.notify();
                    }
                }));
        }
        self.url = Some(url);
        self.code = Some(code);
        self.join_name = Some(line_field("pixel", window, cx));
        self.rename = Some(line_field("laptop", window, cx));
        self.address = Some(line_field("100.64.0.7", window, cx));
    }

    pub fn url_field(&self) -> Option<&Entity<InputState>> {
        self.url.as_ref()
    }

    pub fn code_field(&self) -> Option<&Entity<InputState>> {
        self.code.as_ref()
    }

    pub fn join_name_field(&self) -> Option<&Entity<InputState>> {
        self.join_name.as_ref()
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
                // A request that waits shows its dialog, once, wherever the user is.
                if self.join.is_none()
                    && let Some(waiting) = read
                        .requests
                        .iter()
                        .find(|request| !self.set_aside.contains(&request.request))
                {
                    self.ask_join(waiting.clone(), window, cx);
                }
                // A request answered elsewhere, or expired, closes its dialog.
                if let Some(open) = &self.join
                    && !read.requests.iter().any(|r| r.request == open.request)
                {
                    self.join = None;
                }
                if self.read.as_ref() != Some(&*read) {
                    self.read = Some(*read);
                    cx.notify();
                }
            }
            News::JoinRequested(request) => {
                if self.join.is_none() && !self.set_aside.contains(&request.request) {
                    self.ask_join(request, window, cx);
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
                        "New device: {name} ({}), added at {} from {added_by}",
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
        done: Result<Option<Invitation>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (asked, done) {
            (Asked::Setup, Ok(_)) => {
                self.setting_up = false;
                if let Some(code) = &self.code {
                    code.update(cx, |field, cx| field.set_value("", window, cx));
                }
                toast(Toast::success("This machine set up the hub"), cx);
            }
            (Asked::Setup, Err(why)) => {
                self.setting_up = false;
                toast(Toast::failure("The hub was not set up").detail(why), cx);
            }
            (Asked::Invite, Ok(Some(invitation))) => {
                self.inviting = false;
                self.show_invitation(invitation, cx);
            }
            (Asked::Invite, Ok(None)) => self.inviting = false,
            (Asked::Invite, Err(why)) => {
                self.inviting = false;
                toast(Toast::failure("No invitation was made").detail(why), cx);
            }
            (Asked::Accept(name), Ok(_)) => {
                // Its toast is the new member's, which every client says.
                let _ = name;
            }
            (Asked::Accept(name), Err(why)) => toast(
                Toast::failure(format!("{name} was not added")).detail(why),
                cx,
            ),
            (Asked::Refuse, Ok(_)) => toast(Toast::success("The join request was refused"), cx),
            (Asked::Refuse, Err(why)) => toast(
                Toast::failure("The join request was not refused").detail(why),
                cx,
            ),
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

    fn show_invitation(&mut self, invitation: Invitation, cx: &mut Context<Self>) {
        let left = invitation.expires.saturating_sub(crate::shell::now_ms());
        self.shown_code = invitation.code.clone().into();
        self.invitation = Some(invitation);
        // It folds away when it expires: on the window's clock, which a test moves.
        self.expiry = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(left))
                .await;
            this.update(cx, |this, cx| {
                this.invitation = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ── What the user does ────────────────────────────────────────────────────────────────

    /// Set up: the hub's URL and its setup code, this machine its first device.
    pub fn set_up(&mut self, cx: &mut Context<Self>) {
        let (Some(url), Some(code)) = (&self.url, &self.code) else {
            return;
        };
        let url = url.read(cx).value().trim().to_owned();
        let code = Zeroizing::new(code.read(cx).value().trim().to_owned());
        if url.is_empty() || code.is_empty() {
            self.setup_error = Some("Give the hub's address and its setup code.".into());
            cx.notify();
            return;
        }
        self.setting_up = true;
        self.remote.ask(Ask::Setup {
            url,
            code,
            name: crate::machines::host_name(),
        });
        cx.notify();
    }

    /// Add a device: an invitation from the hub, its QR and its code.
    pub fn invite(&mut self, cx: &mut Context<Self>) {
        // The hub makes the invitation: out of reach, there is nothing to ask.
        if self.inviting
            || self
                .read
                .as_ref()
                .and_then(|read| read.hub.as_ref())
                .is_some_and(|hub| !hub.usable())
        {
            return;
        }
        self.inviting = true;
        self.remote.ask(Ask::Invite);
        cx.notify();
    }

    fn ask_join(&mut self, request: JoinRequest, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = &self.join_name {
            name.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.join_role = 1;
        self.join_said = Some(request.clone());
        self.join = Some(request);
        self.modals
            .update(cx, |modals, cx| modals.open(JOIN_MODAL, cx));
        cx.notify();
    }

    pub fn choose_role(&mut self, role: usize, cx: &mut Context<Self>) {
        if self.join_role != role {
            self.join_role = role;
            cx.notify();
        }
    }

    pub fn accept(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.join.clone() else {
            return;
        };
        let name = self
            .join_name
            .as_ref()
            .map(|field| field.read(cx).value().trim().to_owned())
            .unwrap_or_default();
        if name.is_empty() {
            return;
        }
        // Answered: it stays listed until the hub took the answer, and is not asked again.
        self.join = None;
        self.set_aside.push(request.request.clone());
        self.remote.ask(Ask::Accept {
            request,
            name,
            role: ROLES[self.join_role].0.to_owned(),
        });
        cx.notify();
    }

    pub fn refuse(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.join.take() else {
            return;
        };
        self.set_aside.push(request.request.clone());
        self.remote.ask(Ask::Refuse {
            request: request.request,
        });
        cx.notify();
    }

    fn close_join(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.join.take() {
            self.set_aside.push(request.request);
            cx.notify();
        }
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

    /// The join request's dialog: the four words, the device's name, its role.
    pub fn join_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let request = self.join.clone().or_else(|| self.join_said.clone());
        let words: SharedString = request
            .as_ref()
            .map(|request| request.words.join(" "))
            .unwrap_or_default()
            .into();
        let this = cx.entity().downgrade();
        let roles = ROLES.iter().fold(
            RadioChoices::new("join-role", "This device can"),
            |choices, (key, title, line)| choices.choice(*key, *title, *line),
        );
        let chosen = cx.entity().downgrade();
        let roles = roles.chosen(self.join_role).on_choose(move |at, _, cx| {
            chosen.update(cx, |this, cx| this.choose_role(at, cx)).ok();
        });
        let name = self.join_name.clone().map(|field| {
            Field::new(
                "join-name-field",
                "Name",
                boxed("join-name", Input::new(&field), cx),
            )
        });
        Modal::new(JOIN_MODAL, "A device asks to join")
            .sentence(
                "Accept it only if the new device shows the same four words. Choose its name and \
                 what it may do.",
            )
            .open(self.join.is_some())
            .takes_focus()
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close_join(cx)).ok();
            })
            .child(
                stack(space::LG)
                    .child(spaced(
                        space::LG,
                        div()
                            .id("join-words")
                            .aria_label(words.clone())
                            .text_size(px(18.))
                            .text_color(p.text)
                            .child(words)
                            .test_support(),
                    ))
                    .children(name.map(|name| spaced(space::LG, name)))
                    .child(spaced(
                        space::LG,
                        Field::new("join-role-field", "This device can", roles),
                    )),
            )
            .action(
                Button::quiet("join-refuse", "Refuse")
                    .on_click(cx.listener(|this, _, _, cx| this.refuse(cx))),
            )
            .action(
                Button::primary("join-accept", "Accept")
                    .on_click(cx.listener(|this, _, _, cx| this.accept(cx))),
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

    fn setup_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let url = self.url.clone().map(|field| {
            spaced(
                space::LG,
                Field::new(
                    "hub-url-field",
                    "Hub address",
                    boxed("hub-url", Input::new(&field), cx),
                ),
            )
        });
        let code = self.code.clone().map(|field| {
            spaced(
                space::LG,
                Field::new(
                    "hub-code-field",
                    "Setup code",
                    boxed("hub-code", Input::new(&field), cx),
                )
                .hint("Printed once in the hub's log at its first start."),
            )
        });
        Section::new("hub-setup", "Set up a hub")
            .sentence(SETUP_SENTENCE)
            .child(
                stack(space::LG)
                    .children(url)
                    .children(code)
                    .child(problem_line(
                        "hub-setup-problem",
                        self.setup_error.clone(),
                        space::MD,
                        cx,
                    ))
                    .child(spaced(
                        space::LG,
                        Button::primary("set-up-hub", "Set up")
                            .disabled(self.setting_up)
                            .on_click(cx.listener(|this, _, _, cx| this.set_up(cx))),
                    )),
            )
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
        let code = self.shown_code.clone();
        let until = self
            .invitation
            .as_ref()
            .map(|invitation| crate::session::clock(invitation.expires))
            .unwrap_or_default();
        let shown = self.invitation.is_some();
        let copied = code.clone();
        let content =
            div()
                .id("invitation-panel")
                .flex()
                .gap(space::LG)
                .p(space::LG)
                // As wide as its plate: the code's column takes the rest.
                .child(div().flex_none().child(
                    QrCode::new("invitation-qr", code.clone(), "Invitation code").shown(shown),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::SM)
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(p.mute)
                                .child("On the new device, scan the code, or give it this code:"),
                        )
                        .child(
                            div()
                                .id("invitation-code")
                                .aria_label(code.clone())
                                .text_size(px(12.5))
                                .text_color(p.text)
                                .child(code)
                                .test_support(),
                        )
                        .child({
                            let said: SharedString = format!("Valid once, until {until}.").into();
                            div()
                                .id("invitation-expiry")
                                .aria_label(said.clone())
                                .text_size(px(12.5))
                                .text_color(p.mute)
                                .child(said)
                                .test_support()
                        })
                        .child(
                            div().child(
                                Button::quiet("copy-invitation", "Copy code")
                                    .leading(Icon::Copy)
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                            copied.to_string(),
                                        ));
                                        toast(Toast::success("Code copied"), cx);
                                    }),
                            ),
                        ),
                )
                .test_support();
        let panel = Card::new().id("invitation-card").child(content);
        let count = self.members.len();
        let hub_down = self
            .read
            .as_ref()
            .and_then(|read| read.hub.as_ref())
            .and_then(|hub| hub.needs_words());
        let hub_usable = hub_down.is_none();
        Section::new("devices", "Devices")
            .count(count.to_string())
            .sentence(DEVICES_SENTENCE)
            .action(
                Button::secondary("add-device", "Add a device")
                    .leading(Icon::Plus)
                    .disabled(self.inviting || !hub_usable)
                    .on_click(cx.listener(|this, _, _, cx| this.invite(cx))),
            )
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
                    .child(presence("invitation", shown, panel).space(space::MD))
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
                "Its links are cut on every machine at once, and its key never joins again.",
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
            return vec![("hub-setup", self.setup_section(cx).into_any_element())];
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
