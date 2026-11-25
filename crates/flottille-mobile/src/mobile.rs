//! The core the app holds: its boundary (`connect`, `request`, `answer`, `stop`, and the batches
//! of events a `Listener` takes, which the Kotlin module turns into a `Flow`), and what runs behind
//! it on a runtime of its own.
//!
//! Behind `connect`: the control session to the hub, which says the phone is present and sends
//! the member list (checked back to the pinned root, a list that does not check refused); a link
//! to each runner through the hub, through which every request goes signed with the
//! device key; on each link, `events.watch` resumed after the last event seen of that runner, so
//! that a new link misses nothing and repeats nothing. The events go to the view state in
//! batches: what arrives together is drawn together.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flottille_link::{
    Action, Ask, Change, Device, DeviceKey, Endpoint, Link, MachineKey, Message, Notice,
    NoticeKind, PeerState, PublicKey, Pushed, Request, Role, Session, Tls,
};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::join::pair;
use crate::keys::{Sealed, Sealer, endpoint_key, write_private};
use crate::tunnel::Opened;
use crate::view::{NoticeView, View, ViewState};
use crate::{MobileError, link};

/// The hub this phone joined, as it pinned it and last checked its list.
const HUB_FILE: &str = "hub.json";
/// The number of the last event handed over of each runner, by its machine key.
const SEEN_FILE: &str = "seen.json";
/// How long a batch waits for more events after its first.
const BATCH_WAIT: Duration = Duration::from_millis(30);
/// The most events in one batch.
const BATCH_MOST: usize = 256;
/// The waits between two dials of a runner that could not be linked, or whose link ended.
const FIRST_WAIT: Duration = Duration::from_millis(250);
const LONGEST_WAIT: Duration = Duration::from_secs(30);
/// How long a notice waits for the link to its runner: the phone it woke may be linking still.
const NOTICE_WAIT: Duration = Duration::from_secs(20);

/// What the app is given each time something changed: the events that came, and the view state.
#[derive(Clone, Debug, uniffi::Record)]
pub struct Batch {
    pub events: Vec<Event>,
    pub view: ViewState,
}

/// One event of a runner's, as its daemon emitted it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Event {
    pub machine: String,
    pub seq: u64,
    pub name: String,
    /// Its `data`, as JSON.
    pub data: String,
}

/// A picture the user chose for a message: its type and its bytes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Picture {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub mime: String,
    pub data: Vec<u8>,
}

/// What the user answered to one question of an ask: the options chosen and the words written.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Reply {
    pub choices: Vec<String>,
    pub text: String,
}

/// A message carries 20 pictures at most, 5 MiB each, as the window's composer does
/// (`docs/protocol.md`, `agent.send`).
const PICTURES_MOST: usize = 20;
const PICTURE_MOST: usize = 5 * 1024 * 1024;
const PICTURE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
/// What the daemon says of a request or a question that is decided already (`approvals.rs`,
/// `agents.rs`, `agent_command.rs`): the card that asked for the answer folds.
const GONE: [&str; 3] = [
    " is already decided",
    " does not wait for an answer",
    " has no question waiting for you",
];
/// Why a message whose words alone do not fit one frame of the link is refused: its pictures go in
/// chunks, its words cannot.
const TOO_BIG: &str = "this message is too big for the link: its words take 1 MiB at most";
/// A `before` past every event: the daemon then gives the latest page.
const LATEST: u64 = i64::MAX as u64;

/// What takes the batches: the Kotlin module's, which hands them to a `Flow`.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn batch(&self, batch: Batch);
}

/// The mobile core.
#[derive(uniffi::Object)]
pub struct Core {
    inner: Arc<Inner>,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
}

pub(crate) struct Inner {
    pub(crate) folder: PathBuf,
    pub(crate) sealed: Sealed,
    pub(crate) handle: tokio::runtime::Handle,
    state: Mutex<State>,
    ids: AtomicU64,
    /// The number of the next connection.
    connections: AtomicU64,
    /// How long a tunnel for the user's browser may go with no connection, in milliseconds.
    browser_idle: AtomicU64,
}

pub(crate) struct State {
    /// The key of the phone's endpoint, its machine in the list: a new one each join.
    endpoint: Arc<MachineKey>,
    device: Option<Device>,
    key: Option<Arc<DeviceKey>>,
    pub(crate) view: View,
    /// The tunnels the phone holds, by id: what closes each.
    pub(crate) tunnels: HashMap<String, crate::tunnel::Held>,
    /// The connection: what `disconnect` ends.
    connection: Option<Connection>,
    /// The number of the last event handed over to the app of each runner, by its machine key
    /// (`machine_id`): a name may be another runner's tomorrow.
    seen: BTreeMap<String, u64>,
}

impl Core {
    /// How long the tunnels opened from now on for the user's browser may go with no connection
    /// (ten minutes by default). Not part of the bindings: for the tests.
    #[doc(hidden)]
    pub fn set_browser_idle(&self, idle: std::time::Duration) {
        self.inner
            .browser_idle
            .store(idle.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Inner {
    pub(crate) fn browser_idle(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.browser_idle.load(Ordering::SeqCst))
    }
}

/// What `connect` started.
struct Connection {
    /// Its number: a pump of a connection ended hands nothing over.
    id: u64,
    notes: UnboundedSender<Note>,
    asker: flottille_link::Asker,
    endpoint: Endpoint,
    links: HashMap<String, Link>,
    /// The runners a task links to, by name: the machine key it links to, and the task.
    linking: HashMap<String, (PublicKey, tokio::task::AbortHandle)>,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in self
            .tasks
            .iter()
            .chain(self.linking.values().map(|(_, task)| task))
        {
            task.abort();
        }
        for link in self.links.values() {
            link.close("the app left");
        }
    }
}

/// What reaches the batches, in order.
pub(crate) enum Note {
    Event {
        machine: String,
        key: PublicKey,
        event: Value,
    },
    Loaded {
        machine: String,
        agents: Value,
        requests: Value,
    },
    Changed,
}

#[uniffi::export]
impl Core {
    /// The core over `folder`, the app's private folder for it; `sealer` seals the device key.
    #[uniffi::constructor]
    pub fn new(folder: String, sealer: Arc<dyn Sealer>) -> Result<Arc<Core>, MobileError> {
        let folder = PathBuf::from(folder);
        std::fs::create_dir_all(&folder)
            .map_err(|error| MobileError::new(format!("{}: {error}", folder.display())))?;
        let sealed = Sealed::new(&folder, sealer);
        let endpoint = endpoint_key(&folder)?;
        let device: Option<Device> = read_json(&folder.join(HUB_FILE))?;
        // A key the app cannot open (its Keystore key lost: a restore, a new phone) leaves the
        // phone not joined, to pair again, never a core that cannot start.
        let (device, key, lost) = match DeviceKey::load(&sealed, &folder) {
            Ok(key) => (device, key.map(Arc::new), None),
            Err(error) => (
                None,
                None,
                Some(format!(
                    "this phone's device key cannot be opened ({error}): pair it again"
                )),
            ),
        };
        let seen = read_json(&folder.join(SEEN_FILE))?.unwrap_or_default();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("flottille-core")
            .enable_all()
            .build()
            .map_err(|error| MobileError::new(format!("the core cannot start: {error}")))?;
        let mut view = View::default();
        if let (Some(device), Some(key)) = (&device, &key)
            && let Ok(members) = device.members()
        {
            view.members(&members, &key.public());
        }
        if let Some(lost) = lost {
            view.say(lost);
        }
        let inner = Arc::new(Inner {
            folder,
            sealed,
            handle: runtime.handle().clone(),
            state: Mutex::new(State {
                endpoint: Arc::new(endpoint),
                device,
                key,
                view,
                tunnels: HashMap::new(),
                connection: None,
                seen,
            }),
            ids: AtomicU64::new(1),
            connections: AtomicU64::new(1),
            browser_idle: AtomicU64::new(crate::tunnel::BROWSER_IDLE.as_millis() as u64),
        });
        Ok(Arc::new(Core {
            inner,
            runtime: Mutex::new(Some(runtime)),
        }))
    }

    /// The device key's fingerprint, once this phone joined a hub.
    pub fn fingerprint(&self) -> Option<String> {
        let state = self.inner.state();
        state.device.as_ref()?;
        state.key.as_ref().map(|key| key.public().fingerprint())
    }

    /// Whether this phone is a member of a hub.
    pub fn joined(&self) -> bool {
        self.fingerprint().is_some()
    }

    /// Pairs with the hub whose pair code the QR holds, as `name` with `role` (`controller` or
    /// `viewer`), both proposed by the app and confirmed by the user: once it returns, the phone
    /// is a member, its endpoint declared and its device key kept.
    pub async fn join(&self, code: String, name: String, role: String) -> Result<(), MobileError> {
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                let (device, key, endpoint) =
                    pair(&inner.folder, &inner.sealed, &code, &name, &role).await?;
                inner.joined(device, key, endpoint)
            })
            .await
    }

    /// Opens the control session to the hub and links to each runner; `listener` takes every
    /// batch until `disconnect`. A hub that refuses the phone, or cannot be reached, is said.
    pub async fn connect(&self, listener: Arc<dyn Listener>) -> Result<(), MobileError> {
        let inner = self.inner.clone();
        self.inner.on(connect(inner, listener)).await
    }

    /// Ends the control session and every link: the app went to the background.
    pub fn disconnect(&self) {
        let ended = self.inner.state().connection.take();
        drop(ended);
    }

    /// Tells the hub whether the user is using the app, and when their last input was.
    pub async fn presence(&self, active: bool) -> Result<(), MobileError> {
        let asker = self
            .inner
            .state()
            .connection
            .as_ref()
            .map(|connection| connection.asker.clone())
            .ok_or_else(|| MobileError::new("the phone is not connected to its hub"))?;
        self.inner
            .on(async move {
                asker
                    .ask(Ask::Presence {
                        active,
                        last_input: now_seconds(),
                    })
                    .await
                    .map_err(|error| MobileError::new(error.to_string()))
            })
            .await
    }

    /// Removes this device from the user's list: its own removal, signed with its device key and
    /// sent to the hub. The connection ends, and the view claims no role and says the phone was
    /// removed; joining again is the way back.
    pub async fn leave(&self) -> Result<(), MobileError> {
        let (asker, change) = {
            let state = self.inner.state();
            let asker = state
                .connection
                .as_ref()
                .map(|connection| connection.asker.clone())
                .ok_or_else(|| MobileError::new("the phone is not connected to its hub"))?;
            let (Some(device), Some(key)) = (&state.device, &state.key) else {
                return Err(MobileError::new("this phone has joined no hub"));
            };
            let change = Change::sign(
                device.list.head(),
                Action::Remove { key: key.public() },
                key,
            );
            (asker, change)
        };
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                asker
                    .ask(Ask::Submit { change })
                    .await
                    .map_err(|error| MobileError::new(error.to_string()))?;
                // The hub has it: it ends this device's session without sending the list, so the
                // links end here and the view says it. The hub's file and the device key go too:
                // started again, the phone is not joined and never dials with a revoked key.
                let mut state = inner.state();
                let ended = state.connection.take();
                let forgotten = match std::fs::remove_file(inner.folder.join(HUB_FILE)) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => inner.sealed.forget_kept(),
                };
                state.device = None;
                state.key = None;
                state.view.removed();
                drop(state);
                drop(ended);
                forgotten.map_err(|error| MobileError::new(format!("this device's files: {error}")))
            })
            .await
    }

    /// `method` with `params` (a JSON object) on the runner `machine`, signed with the device key:
    /// its result, as JSON, or the runner's refusal.
    pub async fn request(
        &self,
        machine: String,
        method: String,
        params: String,
    ) -> Result<String, MobileError> {
        let params: Value = serde_json::from_str(&params)
            .map_err(|error| MobileError::new(format!("the parameters are not JSON: {error}")))?;
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                inner
                    .call(&machine, &method, params)
                    .await
                    .map(|result| result.to_string())
            })
            .await
    }

    /// Allows or denies the request `number` of `machine`.
    pub async fn answer(
        &self,
        machine: String,
        number: u64,
        allow: bool,
    ) -> Result<(), MobileError> {
        let method = if allow {
            "approval.approve"
        } else {
            "approval.deny"
        };
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                inner
                    .call(&machine, method, json!({"number": number}))
                    .await
                    .map(drop)
            })
            .await
    }

    /// Stops the turn the session `agent` of `machine` runs.
    pub async fn stop(&self, machine: String, agent: String) -> Result<(), MobileError> {
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                inner
                    .call(&machine, "agent.cancel", json!({"agent": agent}))
                    .await
                    .map(drop)
            })
            .await
    }

    /// Follows the conversation of the session `agent` of `machine`: its latest page and its tree
    /// read at once, then each event, in the view state's `conversation`.
    pub async fn follow(&self, machine: String, agent: String) -> Result<(), MobileError> {
        self.inner.state().view.follow(&machine, &agent);
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                // The latest page only: a long conversation pages in with `older`, never whole.
                let shown = inner
                    .call(
                        &machine,
                        "agent.show",
                        json!({"agent": agent, "before": LATEST}),
                    )
                    .await?;
                let tree = inner
                    .call(&machine, "agent.tree", json!({"agent": agent}))
                    .await;
                {
                    let mut state = inner.state();
                    state.view.shown(&machine, &agent, &shown);
                    if let Ok(tree) = &tree {
                        state.view.tree(&machine, &agent, tree);
                    }
                }
                inner.note(Note::Changed);
                Ok(())
            })
            .await
    }

    /// The page of `agent` is on screen no more: nothing of it is kept, and no event of it is
    /// followed.
    pub fn unfollow(&self) {
        self.inner.state().view.unfollow();
        self.inner.note(Note::Changed);
    }

    /// The page before the oldest events the followed conversation holds.
    pub async fn older(&self, machine: String, agent: String) -> Result<(), MobileError> {
        let Some(first) = self.inner.state().view.first_followed(&machine, &agent) else {
            return Ok(());
        };
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                let shown = inner
                    .call(
                        &machine,
                        "agent.show",
                        json!({"agent": agent, "before": first}),
                    )
                    .await?;
                inner.state().view.shown(&machine, &agent, &shown);
                inner.note(Note::Changed);
                Ok(())
            })
            .await
    }

    /// A message of the user to the session `agent` of `machine`, with pictures. The window's
    /// limits hold here before anything is sent: 20 pictures, 5 MiB each, png, jpeg, gif or webp.
    pub async fn send(
        &self,
        machine: String,
        agent: String,
        text: String,
        pictures: Vec<Picture>,
    ) -> Result<(), MobileError> {
        if pictures.len() > PICTURES_MOST {
            return Err(MobileError::new(format!(
                "a message carries {PICTURES_MOST} pictures at most"
            )));
        }
        for picture in &pictures {
            if !PICTURE_TYPES.contains(&picture.mime.as_str()) {
                return Err(MobileError::new(format!(
                    "{} is not a picture Flottille takes (png, jpeg, gif or webp)",
                    picture.mime.escape_debug()
                )));
            }
            if picture.data.len() > PICTURE_MOST {
                return Err(MobileError::new("a picture is 5 MiB at most"));
            }
            if picture.data.is_empty() {
                return Err(MobileError::new("a picture is empty"));
            }
        }
        if text.trim().is_empty() && pictures.is_empty() {
            return Err(MobileError::new("the message is empty"));
        }
        let words = json!({"agent": agent, "message": text});
        let inner = self.inner.clone();
        self.inner
            .on(async move { inner.send_message(&machine, words, &pictures).await })
            .await
    }

    /// The user's answers to the ask `asked` of the session `agent` (the number of its
    /// `agent.asked` event), one per question in order: the daemon sends the agent one message.
    pub async fn answer_ask(
        &self,
        machine: String,
        agent: String,
        asked: u64,
        replies: Vec<Reply>,
    ) -> Result<(), MobileError> {
        let answers: Vec<Value> = replies
            .iter()
            .map(|reply| {
                let mut answer = json!({});
                if !reply.choices.is_empty() {
                    answer["choices"] = json!(reply.choices);
                }
                if !reply.text.is_empty() {
                    answer["text"] = json!(reply.text);
                }
                answer
            })
            .collect();
        let params = json!({"agent": agent, "question": asked, "answers": answers});
        let inner = self.inner.clone();
        self.inner
            .on(async move { inner.call(&machine, "agent.answer", params).await.map(drop) })
            .await
    }

    /// Puts the session `agent`'s tree in `mode`: `ask`, `auto` or `full`.
    pub async fn set_mode(
        &self,
        machine: String,
        agent: String,
        mode: String,
    ) -> Result<(), MobileError> {
        if !["ask", "auto", "full"].contains(&mode.as_str()) {
            return Err(MobileError::new(format!(
                "{} is not a mode: ask, auto or full",
                mode.escape_debug()
            )));
        }
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                inner
                    .call(
                        &machine,
                        "agent.mode",
                        json!({"agent": agent, "mode": mode}),
                    )
                    .await
                    .map(drop)
            })
            .await
    }

    /// The bytes of a picture of the conversation of `agent`, by the name its message gave.
    pub async fn image(
        &self,
        machine: String,
        agent: String,
        name: String,
    ) -> Result<Vec<u8>, MobileError> {
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                use base64::Engine;
                let image = inner
                    .call(
                        &machine,
                        "agent.image",
                        json!({"agent": agent, "name": name}),
                    )
                    .await?;
                base64::engine::general_purpose::STANDARD
                    .decode(image["data"].as_str().unwrap_or_default())
                    .map_err(|error| {
                        MobileError::new(format!("the picture is not readable: {error}"))
                    })
            })
            .await
    }

    /// Opens the address `url` of `machine` through a tunnel: `run`'s own address when a run is
    /// named, else the run that serves the port a `localhost` link writes. The tunnel lives while
    /// the core stays connected, until the machine ends it (the run ended), the link goes, or
    /// `close_tunnel`. A tunnel already held to the same run, for the same use, is the one
    /// answered: no second one. `browser` says the address goes to the user's browser, which no
    /// panel of the app closes: that tunnel ends on its own after ten minutes with no connection.
    /// A caller that goes before the machine answers (the panel was left) leaves no tunnel open.
    /// The phone listens on a random port of `127.0.0.1`, which any app of the phone can reach.
    pub async fn open_address(
        &self,
        machine: String,
        run: Option<String>,
        url: String,
        browser: bool,
    ) -> Result<Opened, MobileError> {
        self.inner
            .open_address_for_caller(machine, run, url, browser)
            .await
    }

    /// Closes the tunnel `tunnel`.
    pub fn close_tunnel(&self, tunnel: String) -> Result<(), MobileError> {
        self.inner.close_tunnel(&tunnel)
    }

    /// The view state as it is now.
    pub fn view(&self) -> ViewState {
        self.inner.state().view.state()
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.disconnect();
        // Dropped from anywhere, an async test's task included: nothing waits for its threads.
        if let Some(runtime) = self.runtime.lock().unwrap().take() {
            runtime.shutdown_background();
        }
    }
}

impl Inner {
    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Runs `work` on the core's runtime, whoever awaits it (the Kotlin side's executor).
    pub(crate) async fn on<T: Send + 'static>(
        &self,
        work: impl Future<Output = Result<T, MobileError>> + Send + 'static,
    ) -> Result<T, MobileError> {
        self.handle
            .spawn(work)
            .await
            .map_err(|error| MobileError::new(format!("the core failed: {error}")))?
    }

    /// The join accepted: the hub and the key kept, the view told. A join again starts anew:
    /// the connection to the hub before, and what the view held of it, end.
    fn joined(
        &self,
        device: Device,
        key: DeviceKey,
        endpoint: MachineKey,
    ) -> Result<(), MobileError> {
        write_json(&self.folder.join(HUB_FILE), &device)?;
        let mut state = self.state();
        let ended = state.connection.take();
        state.view = View::default();
        if let Ok(members) = device.members() {
            state.view.members(&members, &key.public());
        }
        state.device = Some(device);
        state.key = Some(Arc::new(key));
        state.endpoint = Arc::new(endpoint);
        drop(state);
        drop(ended);
        Ok(())
    }

    pub(crate) fn note(&self, note: Note) {
        if let Some(connection) = &self.state().connection {
            let _ = connection.notes.send(note);
        }
    }

    fn key(&self) -> Result<Arc<DeviceKey>, MobileError> {
        self.state()
            .key
            .clone()
            .ok_or_else(|| MobileError::new("this phone has joined no hub yet"))
    }

    /// A list the hub sent: kept when it checks, refused and said when it does not.
    fn took_list(self: &Arc<Self>, list: flottille_link::MemberList) {
        let mut state = self.state();
        let Some(own) = state.key.as_ref().map(|key| key.public()) else {
            return;
        };
        let Some(device) = state.device.as_mut() else {
            return;
        };
        match device.take(list) {
            Ok(members) => {
                let kept = device.clone();
                let gone = Inner::unlink_gone(&mut state);
                for name in &gone {
                    state.view.forget(name);
                }
                state.view.members(&members, &own);
                drop(state);
                if let Err(error) = write_json(&self.folder.join(HUB_FILE), &kept) {
                    self.state().view.say(error.to_string());
                }
                self.link_runners();
            }
            Err(error) => {
                state.view.say(format!(
                    "the hub sent a member list that does not check, refused: {error}"
                ));
                drop(state);
            }
        }
        self.note(Note::Changed);
    }

    /// Ends the links to the runners that left the list, or whose machine key changed, and their
    /// tasks: nothing of theirs reaches the phone any more, nothing of the phone's reaches them.
    /// Their names.
    fn unlink_gone(state: &mut State) -> Vec<String> {
        let runners = Inner::runners(state);
        let Some(connection) = &mut state.connection else {
            return Vec::new();
        };
        let mut gone = Vec::new();
        connection.linking.retain(|name, (machine, task)| {
            let still = runners
                .iter()
                .any(|(runner, key)| runner == name && key == machine);
            if !still {
                task.abort();
                gone.push(name.clone());
            }
            still
        });
        for name in &gone {
            if let Some(link) = connection.links.remove(name) {
                link.close("this machine left the user's list");
            }
        }
        gone
    }

    /// The runners of the list, by name, with their machine keys.
    fn runners(state: &State) -> Vec<(String, PublicKey)> {
        let (Some(device), Some(key)) = (&state.device, &state.key) else {
            return Vec::new();
        };
        let Ok(members) = device.members() else {
            return Vec::new();
        };
        members
            .members()
            .iter()
            .filter(|member| member.role == Role::Runner && member.key != key.public())
            .filter_map(|member| Some((member.name.clone(), member.machine?)))
            .collect()
    }

    /// A task for each runner no task links to yet.
    fn link_runners(self: &Arc<Self>) {
        let mut state = self.state();
        let runners = Inner::runners(&state);
        let Some(connection) = &mut state.connection else {
            return;
        };
        for (name, machine) in runners {
            if let std::collections::hash_map::Entry::Vacant(vacant) =
                connection.linking.entry(name)
            {
                let task =
                    self.handle
                        .spawn(keep_linked(self.clone(), vacant.key().clone(), machine));
                vacant.insert((machine, task.abort_handle()));
            }
        }
    }

    /// The link to `machine`, or why there is none.
    pub(crate) fn link_of(&self, machine: &str) -> Result<Link, MobileError> {
        let mut state = self.state();
        let why = state
            .view
            .state()
            .machines
            .into_iter()
            .find(|view| view.name == machine)
            .map(|view| view.why);
        let connection = state
            .connection
            .as_mut()
            .ok_or_else(|| MobileError::new("the phone is not connected to its hub"))?;
        match connection.links.get(machine) {
            Some(link) => Ok(link.clone()),
            None => Err(MobileError::new(match why {
                Some(why) => format!("{machine} is not linked: {why}"),
                None => format!(
                    "no machine of the user's is named {}",
                    machine.escape_debug()
                ),
            })),
        }
    }

    /// Sends `method` with `params` to `machine` on `link`, signed with the device key: what comes
    /// back on its stream.
    pub(crate) async fn ask(
        &self,
        machine: &str,
        link: &Link,
        method: &str,
        params: Value,
    ) -> Result<flottille_link::Answers, MobileError> {
        let key = self.key()?;
        let id = self.ids.fetch_add(1, Ordering::Relaxed).to_string();
        let request = Request::sign(&key, &id, machine, method, &params, SystemTime::now())
            .map_err(|error| MobileError::new(error.to_string()))?;
        let request = serde_json::to_value(&request).expect("a request is JSON");
        let message = Message::Client(request);
        // A request is one frame of the link: what cannot fit, once signed (its parameters travel
        // as a string, escaped again, besides the key, the signature and the nonce), is refused
        // here saying why, rather than ending the link.
        let size = serde_json::to_vec(&message)
            .expect("a message is JSON")
            .len();
        if size > flottille_link::FRAME_LIMIT {
            return Err(MobileError::new(TOO_BIG));
        }
        link.ask(&message)
            .await
            .map_err(|error| MobileError::new(error.to_string()))
    }

    /// The message `words` to `machine` with its `pictures`: inline when, signed, it fits one frame
    /// of the link, else the pictures sent first in chunks, each a signed request of its own, then
    /// the message naming their upload (`docs/remote.md`, "Pictures sent to another machine"). It
    /// is judged from the pictures' sizes, and one is encoded only as it goes: a phone never holds
    /// 20 large pictures encoded at once. What the runner refuses says which picture and why.
    async fn send_message(
        &self,
        machine: &str,
        mut words: Value,
        pictures: &[Picture],
    ) -> Result<(), MobileError> {
        let files: Vec<flottille_link::Attached> = pictures
            .iter()
            .map(|picture| flottille_link::Attached::Picture {
                mime: &picture.mime,
                bytes: &picture.data,
            })
            .collect();
        if files.is_empty() || flottille_link::fits_with("agent.send", &words, &files) {
            if !files.is_empty() {
                use base64::Engine;
                words["images"] = pictures
                    .iter()
                    .map(|picture| {
                        json!({"mime": picture.mime,
                            "data": base64::engine::general_purpose::STANDARD.encode(&picture.data)})
                    })
                    .collect();
            }
            return self.call(machine, "agent.send", words).await.map(drop);
        }
        // The words still travel in one frame: when they cannot, nothing is sent.
        if !flottille_link::fits("agent.send", &words) {
            return Err(MobileError::new(TOO_BIG));
        }
        let link = self.link_of(machine)?;
        let upload = flottille_link::upload_name().map_err(MobileError::new)?;
        for chunk in flottille_link::upload_chunks(&upload, &files) {
            self.call_on(machine, &link, "agent.upload", chunk).await?;
        }
        words["upload"] = json!(upload);
        self.call_on(machine, &link, "agent.send", words)
            .await
            .map(drop)
    }

    /// `method` on `machine`: its result, or its refusal.
    pub(crate) async fn call(
        &self,
        machine: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, MobileError> {
        let link = self.link_of(machine)?;
        self.call_on(machine, &link, method, params).await
    }

    async fn call_on(
        &self,
        machine: &str,
        link: &Link,
        method: &str,
        params: Value,
    ) -> Result<Value, MobileError> {
        let mut answers = self.ask(machine, link, method, params).await?;
        match answers.next().await {
            Some(Ok(answer)) => match answer.get("error") {
                Some(error) => {
                    let why = error["message"]
                        .as_str()
                        .unwrap_or("the runner refused it")
                        .to_owned();
                    // The daemon's own words for a request or a question it does not wait for any
                    // more.
                    let gone = error["code"] == "not-found"
                        || GONE.iter().any(|words| why.contains(words));
                    Err(match gone {
                        true => MobileError::Gone { why },
                        false => MobileError::new(why),
                    })
                }
                None => Ok(answer["result"].clone()),
            },
            Some(Err(error)) => Err(MobileError::new(error.to_string())),
            None => Err(MobileError::new(format!(
                "the link to {machine} ended before it answered"
            ))),
        }
    }

    /// What the runner lists when it links: its sessions and the requests that wait.
    async fn load(&self, machine: &str, link: &Link) -> Result<(), MobileError> {
        let agents = self.call_on(machine, link, "agent.list", json!({})).await?;
        let requests = self
            .call_on(machine, link, "approval.list", json!({}))
            .await?;
        self.note(Note::Loaded {
            machine: machine.to_owned(),
            agents,
            requests,
        });
        Ok(())
    }

    /// The runner's events on `link`, after the last one handed over, until the link ends. The
    /// pump hands them over, and only then counts them seen.
    async fn watch(&self, machine: &str, key: PublicKey, link: &Link) {
        let after = self.state().seen.get(&machine_id(&key)).copied();
        let params = match after {
            Some(after) => json!({"after": after}),
            None => json!({}),
        };
        let Ok(mut answers) = self.ask(machine, link, "events.watch", params).await else {
            return;
        };
        while let Some(Ok(frame)) = answers.next().await {
            if let Some(event) = frame.get("event") {
                if event["seq"].as_u64().is_none() {
                    continue;
                }
                self.note(Note::Event {
                    machine: machine.to_owned(),
                    key,
                    event: event.clone(),
                });
            } else if let Some(error) = frame.get("error") {
                let mut state = self.state();
                state.view.say(format!(
                    "{machine} refused to send its events: {}",
                    error["message"].as_str().unwrap_or_default()
                ));
                return;
            } else if let Some(started) = frame["result"]["after"].as_u64() {
                // The first watch of a runner: where its events start for this phone, a position
                // and no event, kept at once so that the next start resumes there.
                let mut state = self.state();
                if let std::collections::btree_map::Entry::Vacant(vacant) =
                    state.seen.entry(machine_id(&key))
                {
                    vacant.insert(started);
                    keep_seen(&self.folder, &state.seen);
                }
            }
        }
    }

    /// The link to the runner `machine`, whose machine key is `key`, up or down. A task the list
    /// ended in the meantime changes nothing.
    fn online(
        &self,
        machine: &str,
        key: PublicKey,
        link: Option<Link>,
        peer: PeerState,
        why: &str,
    ) {
        let mut state = self.state();
        let current = state.connection.as_ref().is_some_and(|connection| {
            connection
                .linking
                .get(machine)
                .is_some_and(|(linked, _)| *linked == key)
        });
        if !current {
            if let Some(link) = link {
                link.close("this machine left the user's list");
            }
            return;
        }
        // Every link goes through the hub.
        let through_hub = link.is_some();
        state.view.online(machine, peer, through_hub, why);
        if let Some(connection) = &mut state.connection {
            match link {
                Some(link) => {
                    connection.links.insert(machine.to_owned(), link);
                }
                None => {
                    connection.links.remove(machine);
                }
            }
        }
        drop(state);
        self.note(Note::Changed);
    }
}

async fn connect(inner: Arc<Inner>, listener: Arc<dyn Listener>) -> Result<(), MobileError> {
    let ended = inner.state().connection.take();
    drop(ended);
    let (device, machine) = {
        let state = inner.state();
        let device = state
            .device
            .clone()
            .ok_or_else(|| MobileError::new("this phone has joined no hub yet"))?;
        (device, state.endpoint.clone())
    };
    let said = |error: flottille_link::ControlError| MobileError::new(error.to_string());
    let mut session = device
        .connect(&machine, &crate::hello(), &Tls::System)
        .await
        .map_err(said)?;
    let asker = session.asker();
    asker
        .ask(Ask::About {
            device: "phone".to_owned(),
            system: "android".to_owned(),
            // This core reads a notice (drawn by #828): the hub may send them.
            notices: true,
        })
        .await
        .map_err(said)?;
    asker
        .ask(Ask::Presence {
            active: true,
            last_input: now_seconds(),
        })
        .await
        .map_err(said)?;
    let endpoint = Endpoint::new(&machine).map_err(|error| MobileError::new(error.to_string()))?;
    // Its links to the runners go through this session to the hub, as a daemon's do.
    if let Some(port) = session.relay() {
        endpoint.through(port);
    }
    if let Some(name) = Some(inner.state().view.state().name).filter(|name| !name.is_empty()) {
        endpoint.name_as(&name);
    }
    let (notes, received) = tokio::sync::mpsc::unbounded_channel();
    let id = inner.connections.fetch_add(1, Ordering::Relaxed);
    let pump = tokio::spawn(pump(inner.clone(), id, received, listener));
    let control = tokio::spawn(control(inner.clone(), session));
    {
        let mut state = inner.state();
        state.view.hub("");
        state.connection = Some(Connection {
            id,
            notes,
            asker,
            endpoint,
            links: HashMap::new(),
            linking: HashMap::new(),
            tasks: vec![pump.abort_handle(), control.abort_handle()],
        });
    }
    inner.note(Note::Changed);
    inner.link_runners();
    Ok(())
}

impl Inner {
    /// What the runner whose machine key is `from` told this phone: its row read back over the
    /// link, then in the view for the app to draw; a dismissal leaves the view at once. The notice
    /// holds only a kind and ids: the words are the runner's own, read from its centre.
    async fn noticed(self: Arc<Self>, from: PublicKey, notice: Notice) {
        let runner = Inner::runners(&self.state())
            .into_iter()
            .find(|(_, key)| *key == from)
            .map(|(name, _)| name);
        // The hub relays a runner's notices only; one of a machine not in the list is no one's.
        let Some(machine) = runner else {
            return;
        };
        let number = notice.notification;
        let kind = match notice.kind {
            NoticeKind::Request => "request",
            NoticeKind::Question => "question",
            NoticeKind::Report => "report",
            NoticeKind::Failure => "failure",
            NoticeKind::Dismissed => {
                self.state().view.dismissed(&machine, number);
                self.note(Note::Changed);
                return;
            }
        };
        let asked = json!({"after": number.saturating_sub(1), "limit": 1});
        let row = match self
            .linked_call(&machine, "notifications.list", asked)
            .await
        {
            Ok(listed) => listed["notifications"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["id"] == number).cloned())
                .unwrap_or_default(),
            Err(_) => Value::Null,
        };
        let said = |field: &str| row[field].as_str().unwrap_or_default().to_owned();
        self.state().view.noticed(NoticeView {
            machine,
            notification: number,
            kind: kind.to_owned(),
            session: notice
                .session
                .map(|session| session.as_str().to_owned())
                .unwrap_or_default(),
            title: said("title"),
            excerpt: said("excerpt"),
            // The row's own request, fixed now: Allow answers it or nothing, never another
            // request that waits later in the same session.
            request: match kind {
                "request" => row["request"].as_u64().unwrap_or_default(),
                _ => 0,
            },
        });
        self.note(Note::Changed);
    }

    /// `method` on `machine` once it is linked, within `NOTICE_WAIT`.
    async fn linked_call(
        &self,
        machine: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, MobileError> {
        let until = tokio::time::Instant::now() + NOTICE_WAIT;
        loop {
            match self.link_of(machine) {
                Ok(link) => return self.call_on(machine, &link, method, params).await,
                Err(error) if tokio::time::Instant::now() >= until => return Err(error),
                Err(_) => tokio::time::sleep(FIRST_WAIT).await,
            }
        }
    }
}

/// What the hub pushes, until the session ends.
async fn control(inner: Arc<Inner>, mut session: Session) {
    while let Some(pushed) = session.next().await {
        match pushed {
            Pushed::List(list) => inner.took_list(list),
            Pushed::Notice { from, notice } => {
                tokio::spawn(inner.clone().noticed(from, notice));
            }
            Pushed::Presence(_) | Pushed::Facts(_) => {}
        }
    }
    let why = session.ended();
    inner.state().view.hub(&why);
    inner.note(Note::Changed);
}

/// Keeps a link to the runner `name`: dialled, listed once, its events watched; dialled again
/// when it ends, after a wait that grows while it fails, until the runner leaves the list.
async fn keep_linked(inner: Arc<Inner>, name: String, machine: PublicKey) {
    let mut wait = FIRST_WAIT;
    let mut loaded = false;
    loop {
        let found = {
            let state = inner.state();
            state
                .connection
                .as_ref()
                .map(|connection| connection.endpoint.clone())
        };
        let Some(endpoint) = found else {
            return;
        };
        match link::dial(&endpoint, machine, &name).await {
            Ok(linked) => {
                wait = FIRST_WAIT;
                inner.online(&name, machine, Some(linked.clone()), PeerState::Online, "");
                if !loaded {
                    loaded = inner.load(&name, &linked).await.is_ok();
                }
                inner.watch(&name, machine, &linked).await;
                inner.online(&name, machine, None, PeerState::Offline, "its link ended");
            }
            Err(error) => inner.online(&name, machine, None, error.state(), &error.to_string()),
        }
        let still = Inner::runners(&inner.state())
            .iter()
            .any(|(runner, key)| *runner == name && *key == machine);
        if !still {
            if let Some(connection) = &mut inner.state().connection
                && connection
                    .linking
                    .get(&name)
                    .is_some_and(|(linked, _)| *linked == machine)
            {
                connection.linking.remove(&name);
            }
            return;
        }
        // A runner that comes online is dialled at once.
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = endpoint.online_changed() => {}
        }
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

/// The notes, in batches: the first, and whatever comes within `BATCH_WAIT` of it. An event
/// counts as seen once its batch is handed over, never before: a connection that ends, or an app
/// killed, with events on their way, has them sent again. A pump of a connection that ended hands
/// nothing over.
async fn pump(
    inner: Arc<Inner>,
    connection: u64,
    mut received: UnboundedReceiver<Note>,
    listener: Arc<dyn Listener>,
) {
    while let Some(first) = received.recv().await {
        let mut notes = vec![first];
        let until = tokio::time::Instant::now() + BATCH_WAIT;
        while notes.len() < BATCH_MOST {
            match tokio::time::timeout_at(until, received.recv()).await {
                Ok(Some(note)) => notes.push(note),
                _ => break,
            }
        }
        let (batch, handed) = {
            let mut state = inner.state();
            if state.connection.as_ref().map(|current| current.id) != Some(connection) {
                return;
            }
            let listed = Inner::runners(&state);
            // The last number of each runner this batch hands over.
            let mut handed: BTreeMap<String, u64> = BTreeMap::new();
            let mut events = Vec::new();
            for note in notes {
                match note {
                    Note::Event {
                        machine,
                        key,
                        event,
                    } => {
                        // A runner the list no longer holds says nothing.
                        if !listed
                            .iter()
                            .any(|(runner, listed)| *runner == machine && *listed == key)
                        {
                            continue;
                        }
                        let id = machine_id(&key);
                        let seq = event["seq"].as_u64().unwrap_or_default();
                        // Never twice: a watch again resends what was on its way.
                        let seen = handed.get(&id).or_else(|| state.seen.get(&id));
                        if seen.is_some_and(|seen| seq <= *seen) {
                            continue;
                        }
                        handed.insert(id, seq);
                        state.view.apply(&machine, &event);
                        events.push(Event {
                            seq,
                            name: event["name"].as_str().unwrap_or_default().to_owned(),
                            data: event["data"].to_string(),
                            machine,
                        });
                    }
                    Note::Loaded {
                        machine,
                        agents,
                        requests,
                    } => state.view.load(&machine, &agents, &requests),
                    Note::Changed => {}
                }
            }
            let batch = Batch {
                events,
                view: state.view.state(),
            };
            (batch, handed)
        };
        listener.batch(batch);
        if handed.is_empty() {
            continue;
        }
        let mut state = inner.state();
        for (id, last) in handed {
            let seen = state.seen.entry(id).or_insert(last);
            *seen = (*seen).max(last);
        }
        keep_seen(&inner.folder, &state.seen);
    }
}

/// `seen.json`, written under the state's lock: two writers never cross.
fn keep_seen(folder: &Path, seen: &BTreeMap<String, u64>) {
    let _ = write_json(&folder.join(SEEN_FILE), seen);
}

/// A machine key as `seen.json` names it: its bytes in hexadecimal.
fn machine_id(key: &PublicKey) -> String {
    key.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

fn read_json<T: for<'de> serde::Deserialize<'de>>(file: &Path) -> Result<Option<T>, MobileError> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| MobileError::new(format!("{}: {error}", file.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(MobileError::new(format!("{}: {error}", file.display()))),
    }
}

fn write_json(file: &Path, value: &impl serde::Serialize) -> Result<(), MobileError> {
    let bytes = serde_json::to_vec(value).expect("a state is JSON");
    write_private(file, &bytes)
        .map_err(|error| MobileError::new(format!("{}: {error}", file.display())))
}
