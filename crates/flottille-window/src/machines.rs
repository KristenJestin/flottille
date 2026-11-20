//! The user's machines, as this machine's daemon says them (`docs/remote.md`): the hub it is paired
//! with, the members of the hub's list, the machines it links to and their state, what this
//! machine proposes for itself when it pairs, and its own addresses. The window reads them on a
//! connection of its own to this machine, whichever machine it shows: the list and the pairing are
//! this machine's, signed with this session's device key ([`crate::device`]).
//!
//! The machine the window shows is chosen in the sidebar's switcher: [`Aim`] makes every other
//! connection of the window go to it, each request signed with the device key and carried by this
//! machine's daemon (`docs/remote.md`, "Requests from another machine's clients").

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use flottille_link::DeviceKey;
use flottille_protocol::Request;
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::device::Device;
use crate::link::{Conn, Connect, Update};

type Result<T> = std::result::Result<T, String>;

/// How long a request of the machines' worker may wait: a setup or a change waits for the hub.
const BOUND: Duration = Duration::from_secs(30);
/// How often the machines are read again with no event: a machine that needs an update, or
/// whose link waits, says it with no event of its own.
const AGAIN: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_secs(1);

/// What a member may do (`docs/remote.md`, "Roles"), with the words the window says it in.
pub const ROLES: [(&str, &str, &str); 3] = [
    (
        "runner",
        "Run agents",
        "It runs agents, and drives the other machines as a controller does.",
    ),
    (
        "controller",
        "Control only",
        "It drives your machines without running agents: a phone, a laptop.",
    ),
    (
        "viewer",
        "View only",
        "It reads your machines and their sessions, and answers nothing.",
    ),
];

/// The icons a device is drawn with (`docs/remote.md`, "A device's card"), with their words.
pub const ICONS: [(&str, &str); 5] = [
    ("desktop", "Desktop"),
    ("laptop", "Laptop"),
    ("server", "Server"),
    ("phone", "Phone"),
    ("tablet", "Tablet"),
];

/// A role as the toast says it: `control only`.
pub fn role_words(role: &str) -> String {
    ROLES
        .iter()
        .find(|(key, _, _)| *key == role)
        .map_or_else(|| role.to_owned(), |(_, words, _)| words.to_lowercase())
}

/// Another machine this one links to, as `remote.machines` says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub name: String,
    pub fingerprint: String,
    /// `online`, `offline` or `needs an update`.
    pub state: String,
    /// Since when, in milliseconds since the Unix epoch.
    pub since: Option<u64>,
    pub reason: Option<String>,
    /// Its role in the list; `None` before the list is checked.
    pub role: Option<String>,
    /// The address its live link goes to or came from; `None` while it is not linked.
    pub reached: Option<Reached>,
    /// The addresses it is known at, as the hub, the user or it told them.
    pub addresses: Vec<String>,
}

/// Where a live link to another machine goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Reached {
    pub address: String,
    /// Whether this machine dialled it; when not, `address` is where the link came from.
    pub dialled: bool,
}

impl Machine {
    fn of(value: &Value) -> Machine {
        Machine {
            name: text(&value["name"]),
            fingerprint: text(&value["fingerprint"]),
            state: text(&value["state"]),
            since: value["since"].as_u64(),
            reason: value["reason"].as_str().map(str::to_owned),
            role: value["role"].as_str().map(str::to_owned),
            reached: value["reached"]["address"].as_str().map(|address| Reached {
                address: address.to_owned(),
                dialled: value["reached"]["dialled"].as_bool().unwrap_or(true),
            }),
            addresses: value["addresses"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        }
    }

    /// Whether it runs agents, as far as this machine knows: a machine whose role is not known
    /// yet is shown, one that only controls or views is not.
    pub fn runs(&self) -> bool {
        self.role.as_deref().is_none_or(|role| role == "runner")
    }

    pub fn online(&self) -> bool {
        self.state == "online"
    }

    /// How it is reached, in words: the address of its link (the host only when it dialled this
    /// machine: its port is the one it dialled from), or how it failed; with the hub out of
    /// reach, the address it was last known at, which only the hub could have told.
    pub fn address_words(&self, hub_usable: bool) -> String {
        if let Some(reached) = &self.reached {
            return if reached.dialled {
                format!("Reached at {}", reached.address)
            } else {
                format!("Linked from {}", host_of(&reached.address))
            };
        }
        let reason = self.reason.as_deref().map(how_it_failed);
        let last = self
            .addresses
            .first()
            .filter(|address| !hub_usable && !reason.is_some_and(|r| r.contains(*address)))
            .map(|address| format!("Last known address {address}."));
        match (reason, last) {
            (Some(reason), Some(last)) => format!("{reason} {last}"),
            (Some(reason), None) => reason.to_owned(),
            (None, Some(last)) => last,
            (None, None) => "Not reached yet".to_owned(),
        }
    }

    /// Its state in words: `online`, `offline since 22:14`, `needs an update`.
    pub fn state_words(&self) -> String {
        match (self.state.as_str(), self.since) {
            ("offline", Some(since)) => format!("offline since {}", crate::session::clock(since)),
            (state, _) => state.to_owned(),
        }
    }
}

/// A member of the hub's list.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub name: String,
    /// Its public key, as the daemon says it: what a change names.
    pub key: Value,
    pub fingerprint: String,
    pub role: String,
    pub added_by: String,
    /// The fingerprint of the machine it declared.
    pub machine: Option<String>,
}

/// An address this machine may be reached at: `found` or `declared` by hand.
#[derive(Clone, Debug, PartialEq)]
pub struct Address {
    pub address: String,
    pub how: String,
}

/// The hub this machine is set up with.
#[derive(Clone, Debug, PartialEq)]
pub struct Hub {
    pub url: String,
    /// `connected`, `unreachable`, `reset`, `refused`, `needs an update` or `none`.
    pub state: String,
    /// Since when it is in that state, in milliseconds since the Unix epoch.
    pub since: Option<u64>,
    pub reason: Option<String>,
}

/// A session or a child an agent of this machine started on another (`remote.launches`): what
/// its row under its pilot shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Launched {
    /// The machine it runs on, by name.
    pub machine: String,
    /// The agent of this machine that started it.
    pub caller: String,
    /// `child`, `led` or `handed`.
    pub kind: String,
    pub title: String,
    /// Its id on its machine.
    pub agent: String,
    /// `held`, `started` or `ended`.
    pub state: String,
    /// The kind of the last delivery it sent here, and when.
    pub heard: Option<String>,
    pub heard_at: Option<u64>,
    pub at: u64,
}

impl Launched {
    fn of(value: &Value) -> Launched {
        Launched {
            machine: text(&value["machine"]),
            caller: text(&value["caller"]),
            kind: text(&value["kind"]),
            title: text(&value["title"]),
            agent: text(&value["agent"]),
            state: text(&value["state"]),
            heard: value["heard"].as_str().map(str::to_owned),
            heard_at: value["heard_at"].as_u64(),
            at: value["at"].as_u64().unwrap_or_default(),
        }
    }
}

impl Hub {
    /// Whether what needs the hub (a device added, a change of the list) can work now.
    pub fn usable(&self) -> bool {
        matches!(self.state.as_str(), "connected" | "none")
    }

    /// Its state in words: `reachable`, `unreachable since 22:14`.
    pub fn state_words(&self) -> String {
        match (self.state.as_str(), self.since) {
            ("connected", _) => "reachable".to_owned(),
            ("unreachable", Some(since)) => {
                format!("unreachable since {}", crate::session::clock(since))
            }
            ("unreachable", None) => "unreachable".to_owned(),
            ("none", _) => "not tried yet".to_owned(),
            ("reset", _) => "list reset".to_owned(),
            (state, _) => state.to_owned(),
        }
    }

    /// What does not work while it is not usable: its state is the tag's to say, not this line's.
    pub fn needs_words(&self) -> Option<String> {
        (!self.usable()).then(|| "Changing the list waits for the hub.".to_owned())
    }
}

/// How a failure to reach something went, without the state and the time the row's tag already
/// says: `server is unreachable: tried 192.0.2.10:7421 from pc, no answer in 10 s` and `the hub at
/// hub.example:7420 is unreachable since 22:14: tried 192.0.2.5:7420: refused` keep what follows
/// the colon after `is unreachable`. Any other reason is kept whole.
pub fn how_it_failed(reason: &str) -> &str {
    reason
        .split_once(" is unreachable")
        .and_then(|(_, after)| after.split_once(": "))
        .map_or(reason, |(_, how)| how)
}

/// The host of a socket address, without its port.
fn host_of(address: &str) -> String {
    address
        .parse::<std::net::SocketAddr>()
        .map_or_else(|_| address.to_owned(), |socket| socket.ip().to_string())
}

/// What a machine proposes for itself when it pairs, as `remote.machines` says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proposed {
    pub name: String,
    pub icon: String,
    pub role: String,
}

/// Everything the window shows of the user's machines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machines {
    /// Why the machines cannot be read (remote is off), when they cannot.
    pub off: Option<String>,
    /// This machine's member's name, when it is one.
    pub this: Option<String>,
    pub addresses: Vec<Address>,
    pub hub: Option<Hub>,
    pub machines: Vec<Machine>,
    pub members: Vec<Member>,
    /// The hash of the list's last change, which the next one is signed after.
    pub head: Value,
    /// What this machine proposes for itself when it pairs: its name, icon and role.
    pub proposed: Proposed,
    /// The sessions this machine's agents started on the others.
    pub launches: Vec<Launched>,
}

impl Machines {
    /// Whether this machine is in a member list: with none, Settings offers to connect to a hub.
    pub fn member(&self) -> bool {
        self.hub.is_some() && self.this.is_some()
    }

    /// The machines the switcher offers beside this one: those that run agents.
    pub fn runners(&self) -> impl Iterator<Item = &Machine> {
        self.machines.iter().filter(|machine| machine.runs())
    }

    pub fn machine(&self, name: &str) -> Option<&Machine> {
        self.machines.iter().find(|machine| machine.name == name)
    }

    /// When the member `name` was last seen, as its machine's state says it.
    pub fn seen(&self, member: &Member) -> Option<&Machine> {
        let machine = member.machine.as_deref()?;
        self.machines
            .iter()
            .find(|seen| seen.fingerprint == machine)
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// What the user asks of the machines, off the UI's thread.
pub enum Ask {
    /// Pairs this machine with the hub of `code`, what `flottille-hub pair` printed, as the card
    /// the user confirmed.
    Pair {
        code: Zeroizing<String>,
        name: String,
        icon: String,
        role: String,
    },
    Rename {
        name: String,
        new_name: String,
    },
    Remove {
        name: String,
    },
    AddressAdd(String),
    AddressRemove(String),
    /// Turns remote on or off (`remote.configure`).
    Configure {
        on: bool,
    },
}

impl Ask {
    fn kind(&self) -> Asked {
        match self {
            Ask::Pair { .. } => Asked::Pair,
            Ask::Rename { new_name, .. } => Asked::Rename(new_name.clone()),
            Ask::Remove { name } => Asked::Remove(name.clone()),
            Ask::AddressAdd(address) => Asked::AddressAdd(address.clone()),
            Ask::AddressRemove(address) => Asked::AddressRemove(address.clone()),
            Ask::Configure { on } => Asked::Configure { on: *on },
        }
    }
}

/// What was asked, as its answer says it.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    Pair,
    Rename(String),
    Remove(String),
    AddressAdd(String),
    AddressRemove(String),
    Configure { on: bool },
}

/// What the machines' worker tells the window.
#[derive(Clone, Debug, PartialEq)]
pub enum News {
    /// A fresh read of the machines.
    Read(Box<Machines>),
    /// What was asked was done, or why not.
    Done(Asked, std::result::Result<(), String>),
    /// A member was added to the list, whoever added it.
    Added {
        name: String,
        role: String,
        added_by: String,
        at: u64,
    },
}

/// The machines' worker: a watch of this machine's events that reads the machines again when one
/// is about them, and a thread for each thing asked.
#[derive(Clone)]
pub struct Remote {
    local: Connect,
    device: Option<Device>,
    updates: UnboundedSender<Update>,
}

impl Remote {
    /// Starts the watch on `local`, this machine's daemon.
    pub fn spawn(
        local: Connect,
        device: Option<Device>,
        updates: UnboundedSender<Update>,
    ) -> Remote {
        let remote = Remote {
            local,
            device,
            updates,
        };
        let watching = remote.clone();
        std::thread::Builder::new()
            .name("flottille-machines".into())
            .spawn(move || watching.watch())
            .expect("a thread for the machines");
        remote
    }

    pub fn device(&self) -> Option<&Device> {
        self.device.as_ref()
    }

    fn tell(&self, news: News) -> bool {
        self.updates
            .unbounded_send(Update::Machines(Box::new(news)))
            .is_ok()
    }

    fn watch(&self) {
        while !self.updates.is_closed() {
            if let Err(why) = self.follow() {
                // Said by the main link; the machines are read again once it answers.
                let _ = why;
            }
            std::thread::sleep(RETRY);
        }
    }

    fn follow(&self) -> Result<()> {
        let stream = (self.local)().map_err(|error| error.to_string())?;
        let mut conn = Conn::new(stream).map_err(|error| error.to_string())?;
        conn.request("events.watch", json!({}))?;
        let mut last = None;
        loop {
            let read = read(&mut conn)?;
            if last.as_ref() != Some(&read) {
                last = Some(read.clone());
                if !self.tell(News::Read(Box::new(read))) {
                    return Ok(());
                }
            }
            if let Some(event) = conn.next_event(Some(AGAIN))? {
                let data = &event.data;
                let news = match event.name.as_str() {
                    "remote.member_added" => Some(News::Added {
                        name: text(&data["name"]),
                        role: text(&data["role"]),
                        added_by: text(&data["added_by"]),
                        at: data["at"].as_u64().unwrap_or(event.at),
                    }),
                    _ => None,
                };
                if let Some(news) = news
                    && !self.tell(news)
                {
                    return Ok(());
                }
            }
            if self.updates.is_closed() {
                return Ok(());
            }
        }
    }

    /// Asks `ask` of this machine's daemon, off the UI's thread: the answer comes back as news.
    pub fn ask(&self, ask: Ask) {
        let remote = self.clone();
        std::thread::Builder::new()
            .name("flottille-machines-ask".into())
            .spawn(move || {
                let kind = ask.kind();
                let done = remote.perform(ask);
                remote.tell(News::Done(kind, done));
                // What changed is read again at once, not at the next event.
                if let Ok(stream) = (remote.local)()
                    && let Ok(mut conn) = Conn::new(stream)
                    && let Ok(read) = read(&mut conn)
                {
                    remote.tell(News::Read(Box::new(read)));
                }
            })
            .expect("a thread for what is asked of the machines");
    }

    /// The device key, which signs the requests for the machine the window shows.
    pub fn key(&self) -> Result<DeviceKey> {
        self.device
            .as_ref()
            .ok_or("this window holds no device key".to_owned())?
            .key()
    }

    fn perform(&self, ask: Ask) -> std::result::Result<(), String> {
        let stream = (self.local)().map_err(|error| format!("cannot reach Flottille: {error}"))?;
        let mut conn = Conn::new(stream).map_err(|error| error.to_string())?;
        let mut request = |method: &str, params: Value| -> Result<Value> {
            conn.ask_within(method, params, BOUND)?
        };
        match ask {
            Ask::Pair {
                code,
                name,
                icon,
                role,
            } => {
                let device = self
                    .device
                    .as_ref()
                    .ok_or("this window holds no device key".to_owned())?;
                let key = device.key_to_pair()?;
                let machine: flottille_link::PublicKey = serde_json::from_value(
                    request("remote.machines", json!({}))?["this"]["key"].clone(),
                )
                .map_err(|error| error.to_string())?;
                let proposed = flottille_link::Proposed {
                    key: key.public(),
                    name,
                    role: flottille_link::Role::from_word(&role)
                        .ok_or_else(|| format!("{role} is no role"))?,
                    icon: flottille_link::Icon::from_word(&icon)
                        .ok_or_else(|| format!("{icon} is no icon"))?,
                    colour: flottille_link::Colour::of(&key.public()),
                };
                let paired = request(
                    "remote.pair",
                    json!({"code": code.as_str(), "proposed": proposed}),
                )?;
                let head: flottille_link::ChangeHash =
                    serde_json::from_value(paired["head"].clone())
                        .map_err(|error| error.to_string())?;
                // Claimed by the machine's daemon, which holds its key.
                let declared = flottille_link::Change::sign(
                    head,
                    flottille_link::Action::Machine {
                        key: key.public(),
                        machine,
                    },
                    &key,
                );
                request("remote.declare", json!({"change": declared}))?;
                Ok(())
            }
            Ask::Rename { name, new_name } => {
                let key = self.key()?;
                let members = request("remote.members", json!({}))?;
                let member = member_key(&members, &name)?;
                let change = signed_after(
                    &members,
                    flottille_link::Action::Rename {
                        key: member,
                        name: new_name,
                    },
                    &key,
                )?;
                request("remote.change", json!({"change": change}))?;
                Ok(())
            }
            Ask::Remove { name } => {
                let key = self.key()?;
                let members = request("remote.members", json!({}))?;
                let member = member_key(&members, &name)?;
                let change = signed_after(
                    &members,
                    flottille_link::Action::Remove { key: member },
                    &key,
                )?;
                request("remote.change", json!({"change": change}))?;
                Ok(())
            }
            Ask::AddressAdd(address) => {
                request("remote.address.add", json!({"address": address}))?;
                Ok(())
            }
            Ask::AddressRemove(address) => {
                request("remote.address.remove", json!({"address": address}))?;
                Ok(())
            }
            Ask::Configure { on } => {
                request("remote.configure", json!({"on": on}))?;
                Ok(())
            }
        }
    }
}

/// Everything the window shows of the machines, on one connection to this machine.
fn read(conn: &mut Conn) -> Result<Machines> {
    let machines = match conn.ask_within("remote.machines", json!({}), BOUND)? {
        Ok(machines) => machines,
        Err(why) => {
            return Ok(Machines {
                off: Some(why),
                ..Machines::default()
            });
        }
    };
    let hub = (!machines["hub"].is_null()).then(|| Hub {
        url: text(&machines["hub"]["url"]),
        state: text(&machines["hub"]["state"]),
        since: machines["hub"]["since"].as_u64(),
        reason: machines["hub"]["reason"].as_str().map(str::to_owned),
    });
    let mut read = Machines {
        off: None,
        this: None,
        addresses: machines["this"]["addresses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|address| Address {
                address: text(&address["address"]),
                how: text(&address["how"]),
            })
            .collect(),
        machines: machines["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Machine::of)
            .collect(),
        hub,
        members: Vec::new(),
        head: Value::Null,
        proposed: Proposed {
            name: text(&machines["proposed"]["name"]),
            icon: text(&machines["proposed"]["icon"]),
            role: text(&machines["proposed"]["role"]),
        },
        launches: Vec::new(),
    };
    if let Ok(launched) = conn.ask_within("remote.launches", json!({}), BOUND)? {
        read.launches = launched["launches"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Launched::of)
            .collect();
    }
    if read.hub.is_some()
        && let Ok(members) = conn.ask_within("remote.members", json!({}), BOUND)?
    {
        read.this = members["this"].as_str().map(str::to_owned);
        read.head = members["head"].clone();
        read.members = members["members"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|member| Member {
                name: text(&member["name"]),
                key: member["key"].clone(),
                fingerprint: text(&member["fingerprint"]),
                role: text(&member["role"]),
                added_by: text(&member["added_by"]),
                machine: member["machine"].as_str().map(str::to_owned),
            })
            .collect();
    }
    Ok(read)
}

/// The key of the member named `name` in `remote.members`' answer.
fn member_key(members: &Value, name: &str) -> Result<flottille_link::PublicKey> {
    let found = members["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|member| member["name"] == name)
        .ok_or_else(|| format!("{name} is not a member any more"))?;
    serde_json::from_value(found["key"].clone()).map_err(|error| error.to_string())
}

/// `action`, signed with `key` after the head of the list `remote.members` answered.
fn signed_after(
    members: &Value,
    action: flottille_link::Action,
    key: &DeviceKey,
) -> Result<flottille_link::Change> {
    let head: flottille_link::ChangeHash =
        serde_json::from_value(members["head"].clone()).map_err(|error| error.to_string())?;
    Ok(flottille_link::Change::sign(head, action, key))
}

/// The machine the window shows, when it is not this one: what the cards of a session there say
/// of where it runs.
#[derive(Default)]
pub struct Shown(pub Option<String>);

impl gpui_kit::Global for Shown {}

/// The machine the window shows, when it is not this one.
pub fn shown_machine(cx: &gpui_kit::App) -> Option<String> {
    cx.try_global::<Shown>().and_then(|shown| shown.0.clone())
}

/// Where a card of the machine shown says it runs: `on server`, nothing on this machine.
pub fn shown_on(cx: &gpui_kit::App) -> Option<String> {
    cx.try_global::<Shown>()
        .and_then(|shown| shown.0.clone())
        .map(|machine| format!("on {machine}"))
}

// ── The machine shown ─────────────────────────────────────────────────────────────────────────

/// The machine the window shows, and the connections made for it: every connection the window
/// opens through [`Aim::connect`] goes to that machine, each request signed with the device key
/// and carried by this machine's daemon. Choosing another machine cuts them all, and each part of
/// the window connects again, to the new one, as after any broken connection.
#[derive(Clone, Default)]
pub struct Aim {
    aiming: Arc<Mutex<Aiming>>,
    /// How many times a machine was chosen: what the window's link stamps on what it reads, so
    /// that a read of the machine shown before is told from one of the machine shown now.
    generation: Arc<AtomicU64>,
}

#[derive(Default)]
struct Aiming {
    machine: Option<(String, Arc<DeviceKey>)>,
    /// The connections made, each the window's end and the daemon's: cut at the next choice.
    made: Vec<Weak<Pair>>,
}

/// One connection made through the aim: the window's end of a pair and the daemon's stream.
struct Pair {
    window: UnixStream,
    daemon: UnixStream,
}

impl Pair {
    fn cut(&self) {
        let _ = self.window.shutdown(std::net::Shutdown::Both);
        let _ = self.daemon.shutdown(std::net::Shutdown::Both);
    }
}

impl Aim {
    /// The machine shown, when it is not this one.
    pub fn machine(&self) -> Option<String> {
        self.aiming
            .lock()
            .unwrap()
            .machine
            .as_ref()
            .map(|(name, _)| name.clone())
    }

    /// Shows `machine` (`None`: this one), signing with `key`, and cuts every connection made for
    /// the one shown before.
    pub fn set(&self, machine: Option<(String, Arc<DeviceKey>)>) {
        let made = {
            let mut aiming = self.aiming.lock().unwrap();
            aiming.machine = machine;
            // Under the lock that `open` reads the machine under: a connection opened between a
            // read of the generation and the next one that finds it unchanged goes to the
            // machine of that generation.
            self.generation.fetch_add(1, Ordering::SeqCst);
            std::mem::take(&mut aiming.made)
        };
        for pair in made.iter().filter_map(Weak::upgrade) {
            pair.cut();
        }
    }

    /// The generation of the machine shown: one more at each choice.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// [`Aim::generation`], for the link to read beside each connection it opens.
    pub fn generations(&self) -> Arc<AtomicU64> {
        self.generation.clone()
    }

    /// `local`'s connections, made to the machine shown.
    pub fn connect(&self, local: Connect) -> Connect {
        let aim = self.clone();
        Arc::new(move || aim.open(&local))
    }

    fn open(&self, local: &Connect) -> std::io::Result<UnixStream> {
        let daemon = local()?;
        let (window, ours) = UnixStream::pair()?;
        let pair = Arc::new(Pair {
            window: ours,
            daemon,
        });
        let machine = {
            let mut aiming = self.aiming.lock().unwrap();
            aiming.made.retain(|made| made.strong_count() > 0);
            aiming.made.push(Arc::downgrade(&pair));
            aiming.machine.clone()
        };
        let (to, back) = (pair.clone(), pair);
        std::thread::Builder::new()
            .name("flottille-aim-to".into())
            .spawn(move || {
                let _ = carry(&to, machine.as_ref());
                to.cut();
            })?;
        std::thread::Builder::new()
            .name("flottille-aim-back".into())
            .spawn(move || {
                let _ = std::io::copy(&mut &back.daemon, &mut &back.window);
                back.cut();
            })?;
        Ok(window)
    }
}

/// Writes each line the window sends to the daemon: as it is for this machine, signed for
/// another.
fn carry(pair: &Pair, machine: Option<&(String, Arc<DeviceKey>)>) -> std::io::Result<()> {
    let Some((name, key)) = machine else {
        std::io::copy(&mut &pair.window, &mut &pair.daemon)?;
        return Ok(());
    };
    let mut lines = BufReader::new(&pair.window);
    let mut line = String::new();
    loop {
        line.clear();
        if lines.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let request: Request =
            serde_json::from_str(line.trim_end()).map_err(std::io::Error::other)?;
        let signed = flottille_link::Request::sign(
            key,
            &request.id,
            name,
            &request.method,
            &request.params,
            std::time::SystemTime::now(),
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        let mut out = serde_json::to_vec(&signed).map_err(std::io::Error::other)?;
        out.push(b'\n');
        (&pair.daemon).write_all(&out)?;
    }
}
