//! The user's machines, as this machine's daemon says them (`docs/remote.md`): the hub it is set
//! up with, the members of the hub's list, the machines it links to and their state, the join
//! requests that wait for this machine's member, and its own addresses. The window reads them on
//! a connection of its own to this machine, whichever machine it shows: the list and the pairing
//! are this machine's, signed with this session's device key ([`crate::device`]).
//!
//! The machine the window shows is chosen in the sidebar's switcher: [`Aim`] makes every other
//! connection of the window go to it, each request signed with the device key and carried by this
//! machine's daemon (`docs/remote.md`, "Requests from another machine's clients").

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
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

/// A join request that waits for this machine's member.
#[derive(Clone, Debug, PartialEq)]
pub struct JoinRequest {
    /// The new device's fingerprint, which names it.
    pub request: String,
    pub key: Value,
    pub words: Vec<String>,
}

impl JoinRequest {
    fn of(value: &Value) -> JoinRequest {
        JoinRequest {
            request: text(&value["request"]),
            key: value["key"].clone(),
            words: value["words"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        }
    }
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
    pub requests: Vec<JoinRequest>,
    /// The sessions this machine's agents started on the others.
    pub launches: Vec<Launched>,
}

impl Machines {
    /// Whether this machine is in a member list: with none, Settings offers to set a hub up.
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
    /// Makes this machine the first member of the hub at `url`, with its setup code.
    Setup {
        url: String,
        code: Zeroizing<String>,
        name: String,
    },
    /// An invitation: a code and its QR, valid 5 minutes, once.
    Invite,
    /// Accepts a join request with the new device's name and role.
    Accept {
        request: JoinRequest,
        name: String,
        role: String,
    },
    Refuse {
        request: String,
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
}

impl Ask {
    fn kind(&self) -> Asked {
        match self {
            Ask::Setup { .. } => Asked::Setup,
            Ask::Invite => Asked::Invite,
            Ask::Accept { name, .. } => Asked::Accept(name.clone()),
            Ask::Refuse { .. } => Asked::Refuse,
            Ask::Rename { new_name, .. } => Asked::Rename(new_name.clone()),
            Ask::Remove { name } => Asked::Remove(name.clone()),
            Ask::AddressAdd(address) => Asked::AddressAdd(address.clone()),
            Ask::AddressRemove(address) => Asked::AddressRemove(address.clone()),
        }
    }
}

/// What was asked, as its answer says it.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    Setup,
    Invite,
    Accept(String),
    Refuse,
    Rename(String),
    Remove(String),
    AddressAdd(String),
    AddressRemove(String),
}

/// An invitation the hub gave: the code (the QR's text) and when it expires.
#[derive(Clone, Debug, PartialEq)]
pub struct Invitation {
    pub code: String,
    /// In milliseconds since the Unix epoch.
    pub expires: u64,
}

/// What the machines' worker tells the window.
#[derive(Clone, Debug, PartialEq)]
pub enum News {
    /// A fresh read of the machines.
    Read(Machines),
    /// What was asked was done, or why not.
    Done(Asked, std::result::Result<Option<Invitation>, String>),
    /// A new device asks to join through this machine's member.
    JoinRequested(JoinRequest),
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
                if !self.tell(News::Read(read)) {
                    return Ok(());
                }
            }
            if let Some(event) = conn.next_event(Some(AGAIN))? {
                let data = &event.data;
                let news = match event.name.as_str() {
                    "remote.join_requested" => Some(News::JoinRequested(JoinRequest::of(data))),
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
                    remote.tell(News::Read(read));
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

    fn perform(&self, ask: Ask) -> std::result::Result<Option<Invitation>, String> {
        let stream = (self.local)().map_err(|error| format!("cannot reach Flottille: {error}"))?;
        let mut conn = Conn::new(stream).map_err(|error| error.to_string())?;
        let mut request = |method: &str, params: Value| -> Result<Value> {
            conn.ask_within(method, params, BOUND)?
        };
        match ask {
            Ask::Setup { url, code, name } => {
                let device = self
                    .device
                    .as_ref()
                    .ok_or("this window holds no device key".to_owned())?;
                device.settle_pending(&mut request)?;
                let (key, new) = device.key_to_join()?;
                let machine: flottille_link::PublicKey = serde_json::from_value(
                    request("remote.machines", json!({}))?["this"]["key"].clone(),
                )
                .map_err(|error| error.to_string())?;
                let mut list = flottille_link::MemberList::start(&name, &key)
                    .map_err(|error| error.to_string())?;
                // Claimed by the machine's daemon, which holds its key.
                list.sign(
                    flottille_link::Action::Machine {
                        key: key.public(),
                        machine,
                    },
                    &key,
                );
                request(
                    "remote.setup",
                    json!({"url": url, "code": code.as_str(), "changes": list.changes()}),
                )?;
                if new {
                    device.keep_new(key)?;
                }
                Ok(None)
            }
            Ask::Invite => {
                let hub: flottille_link::PublicKey = serde_json::from_value(
                    request("remote.machines", json!({}))?["hub"]["key"].clone(),
                )
                .map_err(|_| "this machine is not set up with a hub".to_owned())?;
                let key = self.key()?;
                let asked = flottille_link::InvitationAsk::sign(&hub, unix_now(), &key)
                    .map_err(|error| error.to_string())?;
                let invited = request("remote.invite", json!({"ask": asked}))?;
                Ok(Some(Invitation {
                    code: text(&invited["code"]),
                    expires: invited["expires"].as_u64().unwrap_or_default(),
                }))
            }
            Ask::Accept {
                request: join,
                name,
                role,
            } => {
                let key = self.key()?;
                let new: flottille_link::PublicKey =
                    serde_json::from_value(join.key).map_err(|error| error.to_string())?;
                let role = match role.as_str() {
                    "runner" => flottille_link::Role::Runner,
                    "controller" => flottille_link::Role::Controller,
                    _ => flottille_link::Role::Viewer,
                };
                let members = request("remote.members", json!({}))?;
                let change = signed_after(
                    &members,
                    flottille_link::Action::Add {
                        key: new,
                        name,
                        role,
                    },
                    &key,
                )?;
                request("remote.change", json!({"change": change}))?;
                Ok(None)
            }
            Ask::Refuse { request: join } => {
                request("remote.refuse", json!({"request": join}))?;
                Ok(None)
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
                Ok(None)
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
                Ok(None)
            }
            Ask::AddressAdd(address) => {
                request("remote.address.add", json!({"address": address}))?;
                Ok(None)
            }
            Ask::AddressRemove(address) => {
                request("remote.address.remove", json!({"address": address}))?;
                Ok(None)
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
        requests: Vec::new(),
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
        if let Ok(requests) = conn.ask_within("remote.requests", json!({}), BOUND)? {
            read.requests = requests
                .as_array()
                .into_iter()
                .flatten()
                .map(JoinRequest::of)
                .collect();
        }
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// The host's name: this device's name when it sets a hub up, as `flottille remote setup` takes it.
pub fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is as long as said; gethostname writes within it.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return "this-machine".to_owned();
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    match String::from_utf8_lossy(&buffer[..end]).into_owned() {
        name if name.is_empty() => "this-machine".to_owned(),
        name => name,
    }
}

/// The machine the window shows, when it is not this one: what the cards of a session there say
/// of where it runs.
#[derive(Default)]
pub struct Shown(pub Option<String>);

impl gpui_kit::Global for Shown {}

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
pub struct Aim(Arc<Mutex<Aiming>>);

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
        self.0
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
            let mut aiming = self.0.lock().unwrap();
            aiming.machine = machine;
            std::mem::take(&mut aiming.made)
        };
        for pair in made.iter().filter_map(Weak::upgrade) {
            pair.cut();
        }
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
            let mut aiming = self.0.lock().unwrap();
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
