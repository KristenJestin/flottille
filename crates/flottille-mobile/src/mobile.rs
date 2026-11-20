//! The core the app holds: its boundary (`connect`, `request`, `answer`, `stop`, and the batches
//! of events a `Listener` takes, which the Kotlin module turns into a `Flow`), and what runs behind
//! it on a runtime of its own.
//!
//! Behind `connect`: the control session to the hub, which says the phone is present and sends
//! the member list (checked back to the pinned root, a list that does not check refused) and the
//! runners' addresses; a link to each runner, through which every request goes signed with the
//! device key; on each link, `events.watch` resumed after the last event seen of that runner, so
//! that a new link misses nothing and repeats nothing. The events go to the view state in
//! batches: what arrives together is drawn together.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flottille_link::{
    Ask, Device, DeviceKey, Endpoint, Link, MachineKey, Message, PublicKey, Pushed, Request, Role,
    Session, Tls, Told,
};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::join::{Pairing, start};
use crate::keys::{Sealed, Sealer, endpoint_key, write_private};
use crate::view::{View, ViewState};
use crate::{MobileError, link};

/// The hub this phone joined, as it pinned it and last checked its list.
const HUB_FILE: &str = "hub.json";
/// The number of the last event seen of each runner.
const SEEN_FILE: &str = "seen.json";
/// How long a batch waits for more events after its first.
const BATCH_WAIT: Duration = Duration::from_millis(30);
/// The most events in one batch.
const BATCH_MOST: usize = 256;
/// The waits between two dials of a runner that could not be linked, or whose link ended.
const FIRST_WAIT: Duration = Duration::from_millis(250);
const LONGEST_WAIT: Duration = Duration::from_secs(30);

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
    pub(crate) endpoint: MachineKey,
    handle: tokio::runtime::Handle,
    state: Mutex<State>,
    ids: AtomicU64,
    /// Rings when the hub told new addresses: a runner waiting to dial again tries at once.
    told_again: tokio::sync::Notify,
}

#[derive(Default)]
struct State {
    device: Option<Device>,
    key: Option<Arc<DeviceKey>>,
    view: View,
    /// The connection: what `disconnect` ends.
    connection: Option<Connection>,
    /// The number of the last event seen of each runner.
    seen: BTreeMap<String, u64>,
}

/// What `connect` started.
struct Connection {
    notes: UnboundedSender<Note>,
    asker: flottille_link::Asker,
    endpoint: Endpoint,
    links: HashMap<String, Link>,
    /// The runners a task links to, by name.
    linking: HashSet<String>,
    told: HashMap<[u8; 32], Vec<String>>,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for link in self.links.values() {
            link.close("the app left");
        }
    }
}

/// What reaches the batches, in order.
enum Note {
    Event {
        machine: String,
        event: Value,
    },
    Loaded {
        machine: String,
        agents: Value,
        requests: Value,
    },
    Shown {
        machine: String,
        agent: String,
        events: Value,
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
        let key = DeviceKey::load(&sealed, &folder)
            .map_err(|error| MobileError::new(error.to_string()))?
            .map(Arc::new);
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
        let inner = Arc::new(Inner {
            folder,
            sealed,
            endpoint,
            handle: runtime.handle().clone(),
            state: Mutex::new(State {
                device,
                key,
                view,
                connection: None,
                seen,
            }),
            ids: AtomicU64::new(1),
            told_again: tokio::sync::Notify::new(),
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

    /// Starts joining the hub of the QR's `code`: the pairing shows the words, then waits for the
    /// member's answer.
    pub async fn join(&self, code: String) -> Result<Arc<Pairing>, MobileError> {
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                let (words, started) =
                    start(&inner.folder, &inner.sealed, &inner.endpoint, &code).await?;
                Ok(Arc::new(Pairing::new(words, started, inner)))
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

    /// Follows the conversation of the session `agent` of `machine`: what it holds, then each
    /// event, in the view state's `conversation`.
    pub async fn follow(&self, machine: String, agent: String) -> Result<(), MobileError> {
        self.inner.state().view.follow(&machine, &agent);
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                let events = inner
                    .call(&machine, "agent.show", json!({"agent": agent}))
                    .await?;
                inner.note(Note::Shown {
                    machine,
                    agent,
                    events,
                });
                Ok(())
            })
            .await
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
    fn state(&self) -> MutexGuard<'_, State> {
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

    /// The join accepted: the hub and the key kept, the view told.
    pub(crate) fn joined(&self, device: Device, key: DeviceKey) -> Result<(), MobileError> {
        write_json(&self.folder.join(HUB_FILE), &device)?;
        let mut state = self.state();
        if let Ok(members) = device.members() {
            state.view.members(&members, &key.public());
        }
        state.device = Some(device);
        state.key = Some(Arc::new(key));
        Ok(())
    }

    fn note(&self, note: Note) {
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
                state.view.members(&members, &own);
                drop(state);
                if let Err(error) = write_json(&self.folder.join(HUB_FILE), &kept) {
                    self.state().view.say(error.to_string());
                }
                self.link_runners();
            }
            Err(error) => state.view.say(format!(
                "the hub sent a member list that does not check, refused: {error}"
            )),
        }
        self.note(Note::Changed);
    }

    /// The addresses the hub told for the members' machines.
    fn told(self: &Arc<Self>, told: Vec<Told>) {
        if let Some(connection) = &mut self.state().connection {
            connection.told = told
                .into_iter()
                .map(|told| (*told.key.as_bytes(), told.addresses))
                .collect();
        }
        self.told_again.notify_waiters();
        self.link_runners();
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
            if connection.linking.insert(name.clone()) {
                let task = self.handle.spawn(keep_linked(self.clone(), name, machine));
                connection.tasks.push(task.abort_handle());
            }
        }
    }

    /// The link to `machine`, or why there is none.
    fn link_of(&self, machine: &str) -> Result<Link, MobileError> {
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
    async fn ask(
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
        link.ask(&Message::Client(request))
            .await
            .map_err(|error| MobileError::new(error.to_string()))
    }

    /// `method` on `machine`: its result, or its refusal.
    async fn call(&self, machine: &str, method: &str, params: Value) -> Result<Value, MobileError> {
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
                Some(error) => Err(MobileError::new(
                    error["message"]
                        .as_str()
                        .unwrap_or("the runner refused it")
                        .to_owned(),
                )),
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

    /// The runner's events on `link`, after the last one seen, until the link ends.
    async fn watch(&self, machine: &str, link: &Link) {
        let after = self.state().seen.get(machine).copied();
        let params = match after {
            Some(after) => json!({"after": after}),
            None => json!({}),
        };
        let Ok(mut answers) = self.ask(machine, link, "events.watch", params).await else {
            return;
        };
        while let Some(Ok(frame)) = answers.next().await {
            if let Some(event) = frame.get("event") {
                let Some(seq) = event["seq"].as_u64() else {
                    continue;
                };
                {
                    let mut state = self.state();
                    // Never twice: a resumed watch starts after the last one seen.
                    if state.seen.get(machine).is_some_and(|seen| seq <= *seen) {
                        continue;
                    }
                    state.seen.insert(machine.to_owned(), seq);
                }
                self.note(Note::Event {
                    machine: machine.to_owned(),
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
                self.state()
                    .seen
                    .entry(machine.to_owned())
                    .or_insert(started);
            }
        }
    }

    fn online(&self, machine: &str, link: Option<Link>, why: &str) {
        let mut state = self.state();
        state.view.online(machine, link.is_some(), why);
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
    let device = inner
        .state()
        .device
        .clone()
        .ok_or_else(|| MobileError::new("this phone has joined no hub yet"))?;
    let said = |error: flottille_link::ControlError| MobileError::new(error.to_string());
    let session = device
        .connect(&inner.endpoint, &crate::hello(), &Tls::System)
        .await
        .map_err(said)?;
    let asker = session.asker();
    asker
        .ask(Ask::Presence {
            active: true,
            last_input: now_seconds(),
        })
        .await
        .map_err(said)?;
    let endpoint =
        Endpoint::dialler(&inner.endpoint).map_err(|error| MobileError::new(error.to_string()))?;
    if let Some(name) = Some(inner.state().view.state().name).filter(|name| !name.is_empty()) {
        endpoint.name_as(&name);
    }
    let (notes, received) = tokio::sync::mpsc::unbounded_channel();
    let pump = tokio::spawn(pump(inner.clone(), received, listener));
    let control = tokio::spawn(control(inner.clone(), session));
    {
        let mut state = inner.state();
        state.view.hub("");
        state.connection = Some(Connection {
            notes,
            asker,
            endpoint,
            links: HashMap::new(),
            linking: HashSet::new(),
            told: HashMap::new(),
            tasks: vec![pump.abort_handle(), control.abort_handle()],
        });
    }
    inner.note(Note::Changed);
    inner.link_runners();
    Ok(())
}

/// What the hub pushes, until the session ends.
async fn control(inner: Arc<Inner>, mut session: Session) {
    while let Some(pushed) = session.next().await {
        match pushed {
            Pushed::List(list) => inner.took_list(list),
            Pushed::Addresses(told) => inner.told(told),
            Pushed::Presence(_) | Pushed::Requests(_) => {}
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
            state.connection.as_ref().map(|connection| {
                (
                    connection.endpoint.clone(),
                    connection
                        .told
                        .get(machine.as_bytes())
                        .cloned()
                        .unwrap_or_default(),
                )
            })
        };
        let Some((endpoint, told)) = found else {
            return;
        };
        match link::dial(&endpoint, machine, &told, &name).await {
            Ok(linked) => {
                wait = FIRST_WAIT;
                inner.online(&name, Some(linked.clone()), "");
                if !loaded {
                    loaded = inner.load(&name, &linked).await.is_ok();
                }
                inner.watch(&name, &linked).await;
                inner.online(&name, None, "its link ended");
            }
            Err(error) => inner.online(&name, None, &error.to_string()),
        }
        let still = Inner::runners(&inner.state())
            .iter()
            .any(|(runner, key)| *runner == name && *key == machine);
        if !still {
            if let Some(connection) = &mut inner.state().connection {
                connection.linking.remove(&name);
            }
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = inner.told_again.notified() => {}
        }
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

/// The notes, in batches: the first, and whatever comes within `BATCH_WAIT` of it.
async fn pump(
    inner: Arc<Inner>,
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
        let (batch, seen) = {
            let mut state = inner.state();
            let mut events = Vec::new();
            for note in notes {
                match note {
                    Note::Event { machine, event } => {
                        state.view.apply(&machine, &event);
                        events.push(Event {
                            seq: event["seq"].as_u64().unwrap_or_default(),
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
                    Note::Shown {
                        machine,
                        agent,
                        events,
                    } => state.view.shown(&machine, &agent, &events),
                    Note::Changed => {}
                }
            }
            let batch = Batch {
                events,
                view: state.view.state(),
            };
            (batch, state.seen.clone())
        };
        let _ = write_json(&inner.folder.join(SEEN_FILE), &seen);
        listener.batch(batch);
    }
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
