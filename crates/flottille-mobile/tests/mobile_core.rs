//! The mobile core against a hub and a runner on loopback (`docs/remote.md`, "The Android app"):
//! the hub served in this process from a temporary state folder, the runner `server` paired on it,
//! and the runner `server`'s endpoint, which stands for its daemon: it takes the phone's client
//! link as a daemon does (the peer a member's machine), checks each request as a daemon does
//! (`Requests::check`: a member, its signature, its time, its nonce, its role) and answers it,
//! `events.watch` with its stored events after the number asked, then the new ones. The phone is a
//! `Core` over a temporary folder, its device key sealed by a test sealer, paired from the code of
//! the hub's pair mode (#1146).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine;
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Accepted, Action, Change, Colour, Device, DeviceKey, Endpoint, Hello, HubKey, Icon, Keyring,
    KeyringError, Link, MachineKey, Members, Message, PairCode, Proposed, PublicKey, Pushed,
    Requests, Role, Tls, ToDevice, arrive,
};
use flottille_mobile::{
    Batch, Core, Listener, MobileError, Picture, Reply, Sealer, ViewState, hub_card,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

/// A keyring that holds one device key, made from a fixed seed.
struct Seeded([u8; 32]);

impl Keyring for Seeded {
    fn read(
        &self,
        _attributes: &[(&str, &str)],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        Ok(Some(Zeroizing::new(self.0.to_vec())))
    }

    fn store(
        &self,
        _label: &str,
        _attributes: &[(&str, &str)],
        _secret: &[u8],
    ) -> Result<(), KeyringError> {
        unreachable!("the key is there")
    }
}

fn device_key(n: u8) -> DeviceKey {
    DeviceKey::load_or_make(&Seeded([n; 32]), Path::new("/nonexistent")).unwrap()
}

fn hello() -> Hello {
    Hello::ours(env!("CARGO_PKG_VERSION"))
}

/// What the app's Keystore does, played: the bytes turned and marked, so that a file holding the
/// key in clear is seen.
struct TestSealer;

const MARK: &[u8] = b"sealed:";

impl Sealer for TestSealer {
    fn seal(&self, plain: Vec<u8>) -> Result<Vec<u8>, MobileError> {
        let mut sealed = MARK.to_vec();
        sealed.extend(plain.iter().map(|byte| byte ^ 0x5a));
        Ok(sealed)
    }

    fn open(&self, sealed: Vec<u8>) -> Result<Vec<u8>, MobileError> {
        let Some(body) = sealed.strip_prefix(MARK) else {
            return Err(MobileError::Refused {
                why: "not sealed here".to_owned(),
            });
        };
        Ok(body.iter().map(|byte| byte ^ 0x5a).collect())
    }
}

/// The batches the app's `Flow` would take.
#[derive(Default)]
struct Taken(Mutex<Vec<Batch>>);

impl Listener for Taken {
    fn batch(&self, batch: Batch) {
        self.0.lock().unwrap().push(batch);
    }
}

impl Taken {
    fn batches(&self) -> Vec<Batch> {
        self.0.lock().unwrap().clone()
    }

    /// The texts of the events of every batch, in order.
    fn texts(&self) -> Vec<String> {
        self.batches()
            .iter()
            .flat_map(|batch| batch.events.iter())
            .map(|event| {
                serde_json::from_str::<Value>(&event.data).unwrap()["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    /// Until `holds` says yes of the texts, within 10 s.
    async fn until(&self, what: &str, holds: impl Fn(&[String]) -> bool) {
        for _ in 0..500 {
            if holds(&self.texts()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{what} within 10 s: the texts are {:?}", self.texts());
    }

    /// The events of every batch, in order: `(seq, name)`.
    fn events(&self) -> Vec<(u64, String)> {
        self.batches()
            .iter()
            .flat_map(|batch| batch.events.iter())
            .map(|event| (event.seq, event.name.clone()))
            .collect()
    }
}

/// Waits until the core's view says yes, within 10 s.
async fn until(core: &Core, what: &str, holds: impl Fn(&ViewState) -> bool) -> ViewState {
    for _ in 0..500 {
        let view = core.view();
        if holds(&view) {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{what} within 10 s: the view is {:#?}", core.view());
}

/// A request the runner took, and what its check said.
type Took = (Value, Result<Accepted, String>);

/// The runner `server`: its endpoint, its daemon's events, and every request it took.
#[derive(Clone)]
struct Runner {
    members: Arc<Mutex<Option<Members>>>,
    events: Arc<Mutex<Vec<Value>>>,
    emitted: tokio::sync::broadcast::Sender<Value>,
    taken: Arc<Mutex<Vec<Took>>>,
    answers: Arc<Mutex<HashMap<String, Value>>>,
    /// The methods it refuses, with the code and the sentence it says: a daemon's own refusal.
    refusals: Arc<Mutex<HashMap<String, (String, String)>>>,
    links: Arc<Mutex<Vec<Link>>>,
    requests: Arc<Mutex<Requests>>,
    /// The watches subscribed, which no new event can miss from then on.
    watching: Arc<AtomicUsize>,
    /// The phone's links that ended.
    ended: Arc<AtomicUsize>,
    /// Where a tunnel's connections go on this machine: a fake server standing for the run's port.
    tunnel_target: Arc<Mutex<Option<SocketAddr>>>,
    /// Says `closed` with the reason to every tunnel it serves: the run ended.
    tunnel_ends: tokio::sync::broadcast::Sender<String>,
    /// The `tunnel.open` streams the phone dropped (it closed the tunnel).
    tunnel_gone: Arc<AtomicUsize>,
    /// The connections of a tunnel it took.
    tunnel_connections: Arc<AtomicUsize>,
    /// The tunnels it opened: each is `t<n>`.
    tunnels_opened: Arc<AtomicUsize>,
    /// What it says first on a link: another protocol than the phone's makes it a runner that needs
    /// an update.
    hello: Hello,
    name: String,
    /// Its endpoint's key: the machine the phone dials.
    machine: PublicKey,
}

impl Runner {
    fn new(name: &str, machine: PublicKey) -> Runner {
        let mut answers = HashMap::new();
        answers.insert("agent.list".to_owned(), json!({"agents": []}));
        answers.insert("approval.list".to_owned(), json!({"requests": []}));
        Runner {
            members: Arc::default(),
            events: Arc::default(),
            emitted: tokio::sync::broadcast::channel(4096).0,
            taken: Arc::default(),
            answers: Arc::new(Mutex::new(answers)),
            refusals: Arc::default(),
            links: Arc::default(),
            requests: Arc::new(Mutex::new(Requests::new(SystemTime::now()))),
            watching: Arc::default(),
            ended: Arc::default(),
            tunnel_target: Arc::default(),
            tunnel_ends: tokio::sync::broadcast::channel(8).0,
            tunnel_gone: Arc::default(),
            tunnel_connections: Arc::default(),
            tunnels_opened: Arc::default(),
            hello: hello(),
            name: name.to_owned(),
            machine,
        }
    }

    /// An event of its daemon's, numbered after the last.
    fn emit(&self, name: &str, data: Value) -> u64 {
        let mut events = self.events.lock().unwrap();
        let seq = events.len() as u64 + 1;
        let event = json!({
            "seq": seq,
            "at": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64,
            "name": name,
            "data": data,
        });
        events.push(event.clone());
        let _ = self.emitted.send(event);
        seq
    }

    fn answer(&self, method: &str, result: Value) {
        self.answers
            .lock()
            .unwrap()
            .insert(method.to_owned(), result);
    }

    /// From now on it refuses `method` with `why`, as its daemon would.
    fn refuse(&self, method: &str, why: &str) {
        self.refuse_as(method, "refused", why);
    }

    /// As [`Runner::refuse`], with the code of the daemon's error (`not-found`, `refused`).
    fn refuse_as(&self, method: &str, code: &str, why: &str) {
        self.refusals
            .lock()
            .unwrap()
            .insert(method.to_owned(), (code.to_owned(), why.to_owned()));
    }

    /// What its daemon gives of the session `agent` as `agent.show`: the events it emitted of it
    /// (the last `last` ones when that is given), whether older ones remain, and the agent.
    fn show(&self, agent: Value, last: Option<usize>) {
        let all: Vec<Value> = self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["data"]["agent"] == agent["id"])
            .cloned()
            .collect();
        let from = last.map_or(0, |last| all.len().saturating_sub(last));
        self.answer(
            "agent.show",
            json!({"agent": agent, "events": all[from..], "more": from > 0}),
        );
    }

    /// The requests it took of `method`.
    fn taken(&self, method: &str) -> Vec<Took> {
        self.taken
            .lock()
            .unwrap()
            .iter()
            .filter(|(request, _)| request["signed"]["method"] == method)
            .cloned()
            .collect()
    }

    /// Once the phone's watch is subscribed: one more than the `before` already were.
    async fn watched_after(&self, before: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.watching.load(Ordering::SeqCst) <= before {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the phone watches within 10 s");
    }

    async fn watched(&self) {
        self.watched_after(0).await;
    }

    /// Ends every client link it holds.
    fn cut(&self) {
        for link in self.links.lock().unwrap().drain(..) {
            link.close("cut by the test");
        }
    }

    /// Takes the phone's links, as a daemon does: a member's machine only.
    async fn serve(self, endpoint: Endpoint) {
        while let Some(accepting) = endpoint.accept().await {
            let runner = self.clone();
            tokio::spawn(async move {
                // The list that declares the phone's machine may reach the runner a moment after
                // the phone.
                let mut known = false;
                for _ in 0..100 {
                    let members = runner.members.lock().unwrap().clone();
                    if members
                        .is_some_and(|members| members.by_machine(&accepting.peer()).is_some())
                    {
                        known = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                if !known {
                    accepting.refuse("refused: this machine's key is not one of the user's");
                    return;
                }
                let Ok(link) = accepting.handshake(&runner.hello, "phone").await else {
                    return;
                };
                runner.links.lock().unwrap().push(link.clone());
                runner.serve_link(link).await;
                runner.ended.fetch_add(1, Ordering::SeqCst);
            });
        }
    }

    async fn serve_link(&self, link: Link) {
        while let Some(received) = link.next().await {
            // A tunnel's connection: raw bytes to the fake server that stands for the run's port.
            let received = match received.take_tunnel() {
                Ok((_id, stream)) => {
                    self.tunnel_connections.fetch_add(1, Ordering::SeqCst);
                    let target = *self.tunnel_target.lock().unwrap();
                    tokio::spawn(async move {
                        let Some(target) = target else { return };
                        let Ok(mut tcp) = tokio::net::TcpStream::connect(target).await else {
                            return;
                        };
                        let mut stream = stream;
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                    });
                    continue;
                }
                Err(received) => *received,
            };
            let Message::Client(request) = received.message().clone() else {
                received.answer(Err(
                    "a machine's message is not taken on a client link".into()
                ));
                continue;
            };
            let members = self.members.lock().unwrap().clone().unwrap();
            let checked = self
                .requests
                .lock()
                .unwrap()
                .check(
                    &request.to_string(),
                    &self.name,
                    &members,
                    SystemTime::now(),
                )
                .map_err(|error| error.to_string());
            self.taken
                .lock()
                .unwrap()
                .push((request.clone(), checked.clone()));
            let runner = self.clone();
            tokio::spawn(async move {
                let mut stream = received.stream();
                let line = |value: Value| serde_json::to_vec(&value).unwrap();
                let accepted = match checked {
                    Ok(accepted) => accepted,
                    Err(why) => {
                        let id = request["id"].clone();
                        let refused =
                            json!({"id": id, "error": {"code": "refused", "message": why}});
                        stream.send(&line(refused)).await;
                        stream.end().await;
                        return;
                    }
                };
                if accepted.method != "events.watch" {
                    let refusal = runner
                        .refusals
                        .lock()
                        .unwrap()
                        .get(&accepted.method)
                        .cloned();
                    if let Some((code, why)) = refusal {
                        let refused =
                            json!({"id": accepted.id, "error": {"code": code, "message": why}});
                        stream.send(&line(refused)).await;
                        stream.end().await;
                        return;
                    }
                    let result = runner
                        .answers
                        .lock()
                        .unwrap()
                        .get(&accepted.method)
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    if accepted.method == "tunnel.open" {
                        // As the daemon does: the answer, then the stream stays open until the
                        // tunnel ends, and its last frame says why.
                        let mut ends = runner.tunnel_ends.subscribe();
                        let gone = stream.gone();
                        let number = runner.tunnels_opened.fetch_add(1, Ordering::SeqCst) + 1;
                        let shown = json!({"tunnel": format!("t{number}"), "run": accepted.params["run"],
                            "port": accepted.params["port"].as_u64().unwrap_or(result["port"].as_u64().unwrap_or(5173))});
                        if !stream
                            .send(&line(json!({"id": accepted.id, "result": shown})))
                            .await
                        {
                            return;
                        }
                        tokio::select! {
                            reason = ends.recv() => {
                                let reason = reason.unwrap_or_default();
                                stream.send(&line(json!({"closed": reason}))).await;
                                stream.end().await;
                            }
                            _ = gone => {
                                runner.tunnel_gone.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                        return;
                    }
                    stream
                        .send(&line(json!({"id": accepted.id, "result": result})))
                        .await;
                    stream.end().await;
                    return;
                }
                // Subscribed before the stored events are read: none falls between.
                let mut emitted = runner.emitted.subscribe();
                runner.watching.fetch_add(1, Ordering::SeqCst);
                let stored = runner.events.lock().unwrap().clone();
                let last = stored.len() as u64;
                let after = accepted.params["after"].as_u64();
                let started = after.unwrap_or(last);
                if !stream
                    .send(&line(
                        json!({"id": accepted.id, "result": {"after": started}}),
                    ))
                    .await
                {
                    return;
                }
                let mut sent = started;
                if after.is_some() {
                    for event in stored.iter().skip(started as usize) {
                        if !stream.send(&line(json!({"event": event}))).await {
                            return;
                        }
                        sent = event["seq"].as_u64().unwrap();
                    }
                }
                while let Ok(event) = emitted.recv().await {
                    if event["seq"].as_u64().unwrap() <= sent {
                        continue;
                    }
                    sent = event["seq"].as_u64().unwrap();
                    if !stream.send(&line(json!({"event": event}))).await {
                        return;
                    }
                }
            });
        }
    }
}

/// A test's folders, its hub, the runner `server` paired on it, and the runner's endpoint.
struct Place {
    dir: tempfile::TempDir,
    served: Served<SystemClock>,
    runner: Runner,
}

impl Place {
    /// A hub and its runner `server`, reached through the hub.
    async fn new() -> Place {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::open(&dir.path().join("hub"), SystemClock).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let served = flottille_hub::serve(hub, listener, None, hello()).unwrap();
        let runner = add_runner(&served, dir.path(), "server", 1).await;
        Place {
            dir,
            served,
            runner,
        }
    }

    /// Another runner paired on the hub, named `name`, its device key made from `seed`.
    async fn runner(&self, name: &str, seed: u8) -> Runner {
        add_runner(&self.served, self.dir.path(), name, seed).await
    }

    /// Another runner, which differs as `behaviour` says.
    async fn runner_as(&self, name: &str, seed: u8, behaviour: Behaviour) -> Runner {
        add_runner_as(&self.served, self.dir.path(), name, seed, behaviour).await
    }

    /// Pair mode opened on the hub, as `flottille-hub pair` opens it: the code its QR holds.
    fn pair_code(&self) -> String {
        self.pair_code_at(self.served.address())
    }

    /// The same code, naming `address` as the hub's: a proxy in front of it.
    fn pair_code_at(&self, address: SocketAddr) -> String {
        let at = url(address);
        let code = self
            .served
            .with_hub(|hub| hub.open_pairing(&at, None).unwrap().1);
        code.code().unwrap()
    }

    fn phone_folder(&self) -> String {
        self.dir.path().join("phone").to_string_lossy().into_owned()
    }

    fn phone(&self) -> Arc<Core> {
        Core::new(self.phone_folder(), Arc::new(TestSealer)).unwrap()
    }

    /// The phone paired from the QR of the hub's pair mode, as `phone`, a controller.
    async fn joined(&self, phone: &Core) {
        self.joined_by(phone, self.pair_code()).await;
    }

    async fn joined_by(&self, phone: &Core, code: String) {
        tokio::time::timeout(
            Duration::from_secs(10),
            phone.join(code, "phone".to_owned(), "controller".to_owned()),
        )
        .await
        .expect("paired within 10 s")
        .unwrap();
    }

    /// The phone joined, connected, and linked to `server`.
    async fn linked(&self) -> (Arc<Core>, Arc<Taken>) {
        let phone = self.phone();
        self.joined(&phone).await;
        let taken = Arc::new(Taken::default());
        connected(&phone, taken.clone(), &self.runner).await;
        (phone, taken)
    }
}

/// A runner paired on the hub as a computer pairs: its machine's session, then its machine
/// declared; its endpoint serving the phone's links.
async fn add_runner(served: &Served<SystemClock>, dir: &Path, name: &str, seed: u8) -> Runner {
    add_runner_as(served, dir, name, seed, Behaviour::default()).await
}

/// How a runner of a test differs from the one a phone links to.
#[derive(Default)]
struct Behaviour {
    /// Its session to the hub closed once it is paired: the phone cannot reach it.
    offline: bool,
    /// What it says first on a link, when not this build's.
    hello: Option<Hello>,
}

async fn add_runner_as(
    served: &Served<SystemClock>,
    dir: &Path,
    name: &str,
    seed: u8,
    behaviour: Behaviour,
) -> Runner {
    let server = device_key(seed);
    let machine = MachineKey::load_or_make(&dir.join(format!("machines/{name}-{seed}"))).unwrap();
    let code = served.with_hub(|hub| hub.open_pairing(&url(served.address()), None).unwrap().1);
    let mut paired = Device::pair(
        &code,
        Proposed {
            key: server.public(),
            name: name.to_owned(),
            role: Role::Runner,
            icon: Icon::Server,
            colour: Colour::of(&server.public()),
        },
        &machine,
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap();
    let (mut device, _) = paired.listed().await.unwrap();
    let declared = Change::sign(
        device.list.head(),
        Action::Machine {
            key: server.public(),
            machine: machine.public(),
        },
        &server,
    )
    .claimed_by(&machine);
    device.submit(&mut paired.session, declared).await.unwrap();
    let mut session = paired.session;

    let mut runner = Runner::new(name, machine.public());
    if let Some(hello) = behaviour.hello {
        runner.hello = hello;
    }
    *runner.members.lock().unwrap() = Some(device.members().unwrap());
    if behaviour.offline {
        drop(session);
        return runner;
    }
    // As the daemon does: the links to this machine go through its session to the hub.
    let endpoint = Endpoint::new(&machine).unwrap();
    endpoint.through(session.relay().expect("the session's relay"));
    tokio::spawn(runner.clone().serve(endpoint));
    // From then on the runner takes every list the hub sends, as its daemon does.
    let members = runner.members.clone();
    tokio::spawn(async move {
        while let Some(pushed) = session.next().await {
            if let Pushed::List(list) = pushed
                && let Ok(checked) = device.take(list)
            {
                *members.lock().unwrap() = Some(checked);
            }
        }
    });
    runner
}

/// `phone` connected, `listener` taking its batches, and linked to `runner`, which it watches.
async fn connected(phone: &Core, listener: Arc<dyn Listener>, runner: &Runner) {
    let before = runner.watching.load(Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(10), phone.connect(listener))
        .await
        .expect("connected within 10 s")
        .unwrap();
    until(phone, "the runner online", |view| {
        view.machines
            .iter()
            .any(|machine| machine.name == runner.name && machine.online)
    })
    .await;
    runner.watched_after(before).await;
}

fn url(address: SocketAddr) -> String {
    format!("http://{address}")
}
#[tokio::test(flavor = "multi_thread")]
async fn the_core_joins_through_the_hub_from_a_qr_and_links_to_the_runner() {
    let place = Place::new().await;
    let phone = place.phone();
    assert!(!phone.joined());
    place.joined(&phone).await;
    assert!(phone.joined());

    // A member of the hub's list, a controller, its endpoint declared as its machine.
    let members = place.served.with_hub(|hub| hub.members().unwrap().clone());
    let member = members
        .members()
        .iter()
        .find(|member| member.name == "phone")
        .expect("the phone in the hub's list")
        .clone();
    assert_eq!(member.role, Role::Controller);
    assert_eq!(Some(member.key.fingerprint()), phone.fingerprint());
    assert!(member.machine.is_some());

    // The device key is kept sealed by the app's key, never in clear, and no other copy of it is
    // left in the folder: neither the pending one nor a file half written.
    let sealed = std::fs::read(Path::new(&place.phone_folder()).join("device.sealed")).unwrap();
    assert!(sealed.starts_with(MARK));
    let mut files: Vec<String> = std::fs::read_dir(place.phone_folder())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["device.sealed", "endpoint", "hub.json"]);

    // Connected: present on the hub, linked to the runner, its role in the view.
    let taken = Arc::new(Taken::default());
    phone.connect(taken.clone()).await.unwrap();
    let view = until(&phone, "server online", |view| {
        view.machines
            .iter()
            .any(|machine| machine.name == "server" && machine.online)
    })
    .await;
    assert_eq!(view.role, "controller");
    assert_eq!(view.name, "phone");
    assert!(
        place
            .served
            .with_hub(|hub| hub.presence(&member.key))
            .is_some_and(|presence| presence.active)
    );
    place.runner.watched().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while taken.batches().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a batch within 10 s");

    // A new core over the same folder, as at the app's next start, is the same member.
    let fingerprint = phone.fingerprint();
    drop(phone);
    let again = place.phone();
    assert_eq!(again.fingerprint(), fingerprint);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_core_pins_the_hub_its_pair_code_names() {
    let place = Place::new().await;
    let phone = place.phone();
    // A code whose hub key is not the hub's: another hub answers at its address.
    let good = place.pair_code();
    let code = PairCode::from_code(&good).unwrap();
    let other = PairCode {
        hub_key: device_key(9).public(),
        ..code
    }
    .code()
    .unwrap();
    let refused = phone
        .join(other, "phone".to_owned(), "controller".to_owned())
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("another key than the one this device pinned"),
        "{refused}"
    );
    assert!(!phone.joined());

    // The hub's own code, still open, pairs; the hub's key is the one the phone kept.
    phone
        .join(good, "phone".to_owned(), "controller".to_owned())
        .await
        .unwrap();
    assert!(phone.joined());
    let file = Path::new(&place.phone_folder()).join("hub.json");
    let kept: Device = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(kept.hub, place.served.with_hub(|hub| hub.key()));
    place.served.stop().await;
}

/// A hub that lies: it holds the real hub's key and, to every device that connects, welcomes it
/// and says `frames`.
async fn lying_hub(folder: &Path, frames: Vec<ToDevice>) -> SocketAddr {
    let key = HubKey::load_or_make(folder).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let key = Arc::new(key);
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (key, frames) = (key.clone(), frames.clone());
            tokio::spawn(async move {
                let arriving = arrive(Box::new(stream), &key, &hello()).await.unwrap();
                let mut session = arriving.welcome().await.unwrap();
                for frame in &frames {
                    session.send(frame).await.unwrap();
                }
                while let Ok(asked) = session.ask().await {
                    let answer = ToDevice::Answer {
                        id: asked.id,
                        refused: None,
                    };
                    if session.send(&answer).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    address
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_change_no_member_signed_is_refused_by_the_core() {
    let place = Place::new().await;
    let phone = place.phone();
    place.joined(&phone).await;
    drop(phone);

    // The phone's hub, as it kept it, and a list one change longer that no member signed: a key
    // of no one's adds itself as a runner.
    let file = Path::new(&place.phone_folder()).join("hub.json");
    let mut kept: Device = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let before = kept.list.changes().len();
    let mut forged = kept.list.clone();
    let stranger = device_key(9);
    forged.sign(
        Action::Add {
            key: stranger.public(),
            name: "stranger".to_owned(),
            role: Role::Runner,
        },
        &stranger,
    );
    let lying = lying_hub(&place.dir.path().join("hub"), vec![ToDevice::List(forged)]).await;
    kept.url = format!("http://{lying}/");
    std::fs::write(&file, serde_json::to_vec(&kept).unwrap()).unwrap();

    let phone = place.phone();
    let taken = Arc::new(Taken::default());
    tokio::time::timeout(Duration::from_secs(10), phone.connect(taken.clone()))
        .await
        .expect("connected within 10 s")
        .unwrap();
    let view = until(&phone, "the list refused", |view| !view.said.is_empty()).await;
    assert!(
        view.said[0].contains("refused") && view.said[0].contains("neither a member nor the hub"),
        "{:?}",
        view.said
    );
    assert!(
        view.machines
            .iter()
            .all(|machine| machine.name != "stranger")
    );
    // The list kept is the one checked before.
    let after: Device = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(after.list.changes().len(), before);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_are_signed_with_the_device_key() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    place
        .runner
        .answer("project.list", json!({"projects": [{"name": "app"}]}));

    let result = phone
        .request(
            "server".to_owned(),
            "project.list".to_owned(),
            "{}".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap(),
        json!({"projects": [{"name": "app"}]})
    );

    // The runner took it as the phone's: a member's signature over it, checked as a daemon checks.
    let taken = place.runner.taken("project.list");
    assert_eq!(taken.len(), 1);
    let (request, checked) = &taken[0];
    let accepted = checked.as_ref().expect("the runner accepted it");
    assert_eq!(accepted.device.name, "phone");
    assert_eq!(Some(accepted.device.key.fingerprint()), phone.fingerprint());
    assert_eq!(request["machine"], "server");

    // The same request with another signature is refused: nothing but the device key signs.
    let mut forged: flottille_link::Request = serde_json::from_value(request.clone()).unwrap();
    forged.signed.nonce = flottille_link::Nonce([7; 16]);
    let members = place.runner.members.lock().unwrap().clone().unwrap();
    let refused = place
        .runner
        .requests
        .lock()
        .unwrap()
        .check(
            &serde_json::to_string(&forged).unwrap(),
            "server",
            &members,
            SystemTime::now(),
        )
        .unwrap_err();
    assert!(
        refused.to_string().contains("not signed by a member"),
        "{refused}"
    );

    // Answer and stop go the same way.
    phone.answer("server".to_owned(), 3, true).await.unwrap();
    phone
        .stop("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();
    let approved = place.runner.taken("approval.approve");
    assert_eq!(
        approved[0].0["signed"]["params"],
        json!({"number": 3}).to_string()
    );
    assert!(approved[0].1.is_ok());
    assert!(place.runner.taken("agent.cancel")[0].1.is_ok());
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn events_resume_after_the_last_seen_on_a_new_link() {
    let place = Place::new().await;
    let (phone, taken) = place.linked().await;
    for n in 1..=3 {
        place.runner.emit(
            "agent.message",
            json!({"agent": "a1", "text": format!("{n}")}),
        );
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while taken.events().len() < 3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("three events within 10 s");

    // The link ends; the runner's daemon goes on meanwhile.
    place.runner.cut();
    place
        .runner
        .emit("agent.message", json!({"agent": "a1", "text": "4"}));
    place
        .runner
        .emit("agent.message", json!({"agent": "a1", "text": "5"}));

    tokio::time::timeout(Duration::from_secs(10), async {
        while taken.events().len() < 5 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the two missed events within 10 s");
    let watches = place.runner.taken("events.watch");
    assert!(watches.len() >= 2, "a new link watches again");
    let resumed: Value =
        serde_json::from_str(watches[1].0["signed"]["params"].as_str().unwrap()).unwrap();
    assert_eq!(resumed["after"], 3, "resumed after the last event seen");
    // Every event once, in order.
    let seqs: Vec<u64> = taken.events().iter().map(|(seq, _)| *seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
    drop(phone);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_view_state_follows_a_conversation_in_batches() {
    let place = Place::new().await;
    place.runner.answer(
        "agent.list",
        json!({"agents": [{"id": "a1", "project": "app", "title": "Fix the login", "state": "idle",
            "parent": null, "exchanged_at": 1}]}),
    );
    let (phone, taken) = place.linked().await;
    until(&phone, "the session listed", |view| {
        !view.projects.is_empty()
    })
    .await;
    phone
        .follow("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();

    place
        .runner
        .emit("agent.prompted", json!({"agent": "a1", "text": "hello"}));
    let mut reply = String::new();
    for n in 0..120 {
        let piece = format!("w{n} ");
        reply.push_str(&piece);
        place.runner.emit(
            "agent.message",
            json!({"agent": "a1", "text": piece, "message_id": "m1", "replay": false}),
        );
    }
    let number = 7;
    place.runner.emit(
        "approval.requested",
        json!({"number": number, "agent": "a1", "tool": "command", "reasons": ["it runs make"],
            "held_by": "user"}),
    );
    let asked = place.runner.emit(
        "agent.asked",
        json!({"agent": "a1", "question": "Which branch?", "to": "user"}),
    );

    let view = until(&phone, "the whole conversation", |view| {
        view.conversation.as_ref().is_some_and(|conversation| {
            conversation
                .entries
                .last()
                .is_some_and(|entry| entry.text == reply)
        }) && !view.asks.is_empty()
    })
    .await;
    let conversation = view.conversation.unwrap();
    assert_eq!(
        (conversation.machine.as_str(), conversation.agent.as_str()),
        ("server", "a1")
    );
    let kinds: Vec<(&str, &str)> = conversation
        .entries
        .iter()
        .map(|entry| (entry.kind.as_str(), entry.text.as_str()))
        .collect();
    assert_eq!(kinds, vec![("user", "hello"), ("agent", reply.as_str())]);

    // One list for every machine, by project, each session tagged with its machine.
    assert_eq!(view.projects.len(), 1);
    assert_eq!(view.projects[0].name, "app");
    let session = &view.projects[0].sessions[0];
    assert_eq!(
        (
            session.machine.as_str(),
            session.agent.as_str(),
            session.title.as_str()
        ),
        ("server", "a1", "Fix the login")
    );
    assert_eq!(session.state, "working");
    assert_eq!(view.requests.len(), 1);
    assert_eq!(view.requests[0].number, number);
    assert_eq!(view.requests[0].tool, "command");
    assert_eq!(view.asks[0].asked, asked);
    assert_eq!(view.asks[0].question, "Which branch?");
    assert_eq!(view.role, "controller");

    // In batches: 123 events, far fewer batches, each with the view as it was then.
    let batches: Vec<Batch> = taken
        .batches()
        .into_iter()
        .filter(|batch| !batch.events.is_empty())
        .collect();
    let events: usize = batches.iter().map(|batch| batch.events.len()).sum();
    assert_eq!(events, 123);
    assert!(
        batches.len() < 40,
        "{} batches for 123 events",
        batches.len()
    );
    let last = batches.last().unwrap();
    assert_eq!(last.events.last().unwrap().name, "agent.asked");
    assert_eq!(last.view.asks.len(), 1);

    // Answered and denied: they leave the view.
    place.runner.emit(
        "approval.decided",
        json!({"number": number, "agent": "a1", "decision": "deny"}),
    );
    place
        .runner
        .emit("agent.answered", json!({"agent": "a1", "asked": asked}));
    until(&phone, "nothing waits", |view| {
        view.requests.is_empty() && view.asks.is_empty()
    })
    .await;
    place.served.stop().await;
}

/// `text` from the runner's daemon, as an agent's message.
fn say(runner: &Runner, text: &str) -> u64 {
    runner.emit("agent.message", json!({"agent": "a1", "text": text}))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runner_removed_from_the_list_is_unlinked_at_once() {
    let place = Place::new().await;
    let (phone, taken) = place.linked().await;
    say(&place.runner, "before");
    taken
        .until("the first event", |texts| texts == ["before"])
        .await;

    // The user removes the runner: its link ends at once, nothing of it reaches the phone any
    // more, and nothing of the phone's reaches it.
    place
        .served
        .with_hub(|hub| hub.remove("server").map(drop))
        .unwrap();
    until(&phone, "the runner gone from the view", |view| {
        view.machines.is_empty()
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while place.runner.ended.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the removed runner's link ends within 5 s");
    say(&place.runner, "after");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(taken.texts(), ["before"]);
    let refused = phone
        .answer("server".to_owned(), 1, true)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("server"), "{refused}");
    assert!(place.runner.taken("approval.approve").is_empty());
    let view = phone.view();
    assert!(view.projects.is_empty() && view.requests.is_empty() && view.asks.is_empty());
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_joins_again_after_it_was_removed() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    let before = phone.fingerprint();
    place
        .served
        .with_hub(|hub| hub.remove("phone").map(drop))
        .unwrap();
    // The hub ends the session of a device it removed.
    until(&phone, "the hub's refusal said", |view| {
        view.hub.contains("not one of this hub's members")
    })
    .await;

    // Paired again: the key the hub admitted is the one that signs.
    place.joined(&phone).await;
    assert_ne!(phone.fingerprint(), before);
    let member = place.served.with_hub(|hub| {
        hub.members()
            .unwrap()
            .members()
            .iter()
            .find(|member| member.name == "phone")
            .cloned()
            .unwrap()
    });
    assert_eq!(Some(member.key.fingerprint()), phone.fingerprint());
    let taken = Arc::new(Taken::default());
    connected(&phone, taken, &place.runner).await;
    phone
        .request(
            "server".to_owned(),
            "project.list".to_owned(),
            "{}".to_owned(),
        )
        .await
        .unwrap();
    let (_, checked) = place.runner.taken("project.list").pop().unwrap();
    assert_eq!(
        Some(checked.unwrap().device.key.fingerprint()),
        phone.fingerprint()
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_joins_again_after_its_hub_was_reset() {
    let place = Place::new().await;
    let phone = place.phone();
    place.joined(&phone).await;
    let before = phone.fingerprint();
    drop(phone);

    // A hub started anew, with a runner of its own: the phone, over the same folder, pairs with it.
    let reset = Place::new().await;
    let phone = place.phone();
    reset.joined(&phone).await;
    assert_ne!(phone.fingerprint(), before);
    let taken = Arc::new(Taken::default());
    connected(&phone, taken, &reset.runner).await;
    phone
        .request(
            "server".to_owned(),
            "project.list".to_owned(),
            "{}".to_owned(),
        )
        .await
        .unwrap();
    let (_, checked) = reset.runner.taken("project.list").pop().unwrap();
    assert_eq!(
        Some(checked.unwrap().device.key.fingerprint()),
        phone.fingerprint()
    );
    // At the next start, the same member.
    let fingerprint = phone.fingerprint();
    drop(phone);
    assert_eq!(place.phone().fingerprint(), fingerprint);
    place.served.stop().await;
    reset.served.stop().await;
}

/// Another Keystore's key, as after a restore on a new phone: it opens nothing the first sealed.
struct OtherSealer;

const OTHER: &[u8] = b"other:";

impl Sealer for OtherSealer {
    fn seal(&self, plain: Vec<u8>) -> Result<Vec<u8>, MobileError> {
        let mut sealed = OTHER.to_vec();
        sealed.extend(plain.iter().map(|byte| byte ^ 0x33));
        Ok(sealed)
    }

    fn open(&self, sealed: Vec<u8>) -> Result<Vec<u8>, MobileError> {
        let Some(body) = sealed.strip_prefix(OTHER) else {
            return Err(MobileError::Refused {
                why: "the Keystore has no such key".to_owned(),
            });
        };
        Ok(body.iter().map(|byte| byte ^ 0x33).collect())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_key_that_will_not_open_at_start_leaves_the_phone_not_joined() {
    let place = Place::new().await;
    let phone = place.phone();
    place.joined(&phone).await;
    drop(phone);

    // The app starts and its Keystore cannot open the key: the phone is not joined, and says so.
    let phone = Core::new(place.phone_folder(), Arc::new(OtherSealer)).unwrap();
    assert!(!phone.joined());
    assert_eq!(phone.fingerprint(), None);
    let view = phone.view();
    assert!(
        view.said.iter().any(|said| said.contains("pair it again")),
        "{:?}",
        view.said
    );
    assert!(view.role.is_empty() && view.machines.is_empty());

    // Paired again, under another name (its old key, which nothing opens now, is still listed),
    // it is a member with a key this Keystore sealed.
    phone
        .join(
            place.pair_code(),
            "new phone".to_owned(),
            "controller".to_owned(),
        )
        .await
        .unwrap();
    assert!(phone.joined());
    let sealed = std::fs::read(Path::new(&place.phone_folder()).join("device.sealed")).unwrap();
    assert!(sealed.starts_with(OTHER));
    let taken = Arc::new(Taken::default());
    connected(&phone, taken, &place.runner).await;
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn events_of_a_new_runner_on_a_freed_name_are_all_shown() {
    let place = Place::new().await;
    let (phone, taken) = place.linked().await;
    for text in ["a1", "a2", "a3"] {
        say(&place.runner, text);
    }
    taken
        .until("the old runner's events", |texts| texts.len() == 3)
        .await;

    // The runner removed, another machine paired under its name: its events start at 1 again.
    place
        .served
        .with_hub(|hub| hub.remove("server").map(drop))
        .unwrap();
    until(&phone, "the old runner gone", |view| {
        view.machines.is_empty()
    })
    .await;
    let new = place.runner("server", 2).await;
    until(&phone, "the new runner online", |view| {
        view.machines
            .iter()
            .any(|machine| machine.name == "server" && machine.online)
    })
    .await;
    new.watched().await;
    say(&new, "b1");
    say(&new, "b2");
    taken
        .until("the new runner's events", |texts| texts.len() == 5)
        .await;
    assert_eq!(taken.texts(), ["a1", "a2", "a3", "b1", "b2"]);
    place.served.stop().await;
}

/// The seen numbers the core kept in its folder.
fn seen_file(place: &Place) -> HashMap<String, u64> {
    std::fs::read(Path::new(&place.phone_folder()).join("seen.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn events_resume_after_the_last_seen_at_the_next_start() {
    let place = Place::new().await;
    let (phone, taken) = place.linked().await;
    for text in ["1", "2", "3"] {
        say(&place.runner, text);
    }
    taken.until("three events", |texts| texts.len() == 3).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seen_file(&place).values().any(|seen| *seen == 3) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the third event kept as seen within 10 s");
    // Kept by the runner's machine key, never by its name, which another runner may take.
    assert!(!seen_file(&place).contains_key("server"));

    // The app is gone; the runner's daemon goes on; the app starts again.
    drop(phone);
    say(&place.runner, "4");
    say(&place.runner, "5");
    let phone = place.phone();
    let again = Arc::new(Taken::default());
    connected(&phone, again.clone(), &place.runner).await;
    again
        .until("the two missed events", |texts| texts.len() >= 2)
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(again.texts(), ["4", "5"]);
    place.served.stop().await;
}

/// An app that dies as its first batch with events comes, before it took it: the batch never
/// reaches the screens, nor does any after it.
#[derive(Default)]
struct Dying(std::sync::atomic::AtomicBool);

impl Listener for Dying {
    fn batch(&self, batch: Batch) {
        if !batch.events.is_empty() || self.0.load(Ordering::SeqCst) {
            self.0.store(true, Ordering::SeqCst);
            panic!("the app died before it took the batch");
        }
    }
}

/// The phone joined and connected, its batches to an app that dies at the first event; three
/// events sent, which it never took.
async fn three_events_never_taken(place: &Place) -> Arc<Core> {
    let phone = place.phone();
    place.joined(&phone).await;
    let dying = Arc::new(Dying::default());
    connected(&phone, dying.clone(), &place.runner).await;
    for text in ["1", "2", "3"] {
        say(&place.runner, text);
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while !dying.0.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the app dies within 10 s");
    // What is still on its way reaches the core meanwhile.
    tokio::time::sleep(Duration::from_millis(300)).await;
    phone
}

#[tokio::test(flavor = "multi_thread")]
async fn an_event_not_handed_over_at_disconnect_is_sent_again() {
    let place = Place::new().await;
    let phone = three_events_never_taken(&place).await;
    phone.disconnect();

    // Connected again: what the app never took comes, once.
    let again = Arc::new(Taken::default());
    connected(&phone, again.clone(), &place.runner).await;
    again
        .until("the three events", |texts| texts.len() >= 3)
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(again.texts(), ["1", "2", "3"]);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_event_not_handed_over_before_the_app_was_killed_comes_at_the_next_start() {
    let place = Place::new().await;
    let phone = three_events_never_taken(&place).await;
    drop(phone);

    let phone = place.phone();
    let again = Arc::new(Taken::default());
    connected(&phone, again.clone(), &place.runner).await;
    again
        .until("the three events", |texts| texts.len() >= 3)
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(again.texts(), ["1", "2", "3"]);
    place.served.stop().await;
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_code_shows_the_hub_s_card_before_anything_is_sent() {
    let place = Place::new().await;
    let code = place.pair_code();
    let card = hub_card(code).unwrap();
    assert_eq!(card.url, url(place.served.address()));
    assert_eq!(
        card.fingerprint,
        place.served.with_hub(|hub| hub.key().fingerprint())
    );
    // Nothing crossed the network: the hub is not asked, and no pair mode was spent.
    let refused = hub_card("not a code".to_owned()).unwrap_err();
    assert!(!refused.to_string().is_empty());
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_says_its_state_and_since_when() {
    let place = Place::new().await;
    // Not connected to the hub: offline, and why.
    place
        .runner_as(
            "far",
            2,
            Behaviour {
                offline: true,
                ..Behaviour::default()
            },
        )
        .await;
    // Another protocol than the phone's: it needs an update, which is not being offline.
    let mut other = hello();
    other.flottille = "9.9.9".to_owned();
    other.protocols = vec![99];
    place
        .runner_as(
            "old",
            3,
            Behaviour {
                hello: Some(other),
                ..Behaviour::default()
            },
        )
        .await;
    let before = now_millis();
    let (phone, _taken) = place.linked().await;
    let view = until(&phone, "each machine said", |view| {
        let state = |name: &str| {
            view.machines
                .iter()
                .find(|machine| machine.name == name)
                .map(|machine| machine.state.clone())
        };
        state("server").as_deref() == Some("online")
            && state("far").as_deref() == Some("offline")
            && state("old").as_deref() == Some("needs an update")
    })
    .await;
    let machine = |name: &str| view.machines.iter().find(|m| m.name == name).unwrap();
    for name in ["server", "far", "old"] {
        let since = machine(name).since;
        assert!(
            since >= before && since <= now_millis(),
            "{name} since {since}"
        );
    }
    assert!(machine("server").why.is_empty());
    assert!(
        !machine("far").why.is_empty(),
        "the sentence says why it is not linked"
    );
    assert!(
        machine("old").why.contains("9.9.9"),
        "the sentence names the version to update: {}",
        machine("old").why
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_view_lists_the_members_and_marks_this_device() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    let view = phone.view();
    let names: Vec<&str> = view
        .members
        .iter()
        .map(|member| member.name.as_str())
        .collect();
    assert_eq!(names, ["server", "phone"]);
    let me = view.members.iter().find(|member| member.you).unwrap();
    assert_eq!(
        (me.name.as_str(), me.role.as_str(), me.icon.as_str()),
        ("phone", "controller", "phone")
    );
    assert!(!view.members[0].you);
    assert_eq!(view.members[0].role, "runner");
    assert_eq!(view.members[0].icon, "server");
    assert_eq!(me.added_by, "the hub");
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_removes_this_device_from_the_list_and_says_it() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    tokio::time::timeout(Duration::from_secs(10), phone.leave())
        .await
        .expect("left within 10 s")
        .unwrap();
    let gone = place.served.with_hub(|hub| {
        hub.members()
            .unwrap()
            .members()
            .iter()
            .all(|member| member.name != "phone")
    });
    assert!(gone, "the hub's list no longer holds the phone");
    let view = until(&phone, "the removal said", |view| view.role.is_empty()).await;
    assert!(view.members.iter().all(|member| !member.you));
    assert!(
        view.said.iter().any(|said| said.contains("removed")),
        "{:?}",
        view.said
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_that_left_is_not_joined_when_it_starts_again() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    tokio::time::timeout(Duration::from_secs(10), phone.leave())
        .await
        .expect("left within 10 s")
        .unwrap();
    assert!(!phone.joined(), "the phone that left says it is not joined");
    // The app starts again over the same folder: the hub's file and the key are gone, so it does
    // not dial the hub with a key the hub revoked.
    let again = Core::new(place.phone_folder(), Arc::new(TestSealer)).unwrap();
    assert!(!again.joined(), "the phone starts again as not joined");
    assert!(again.fingerprint().is_none());
    assert!(again.view().machines.is_empty(), "{:#?}", again.view());
    place.served.stop().await;
}

/// The view's line of the runner `name`.
fn machine(view: &ViewState, name: &str) -> flottille_mobile::MachineView {
    view.machines
        .iter()
        .find(|machine| machine.name == name)
        .unwrap_or_else(|| panic!("{name} in the view: {view:#?}"))
        .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_links_to_a_runner_through_the_hub() {
    let place = Place::new().await;
    assert!(
        place
            .served
            .with_hub(|hub| hub.members().unwrap().clone())
            .members()
            .iter()
            .any(|member| member.name == "server")
    );
    let (phone, _taken) = place.linked().await;

    let view = phone.view();
    let server = machine(&view, "server");
    assert!(server.online, "{view:#?}");
    assert!(server.through_hub, "{view:#?}");
    // What the phone asks goes over that link, signed and answered as on any other.
    place.runner.watched().await;
    assert!(!place.runner.taken("agent.list").is_empty());
    assert!(
        place
            .runner
            .taken("agent.list")
            .iter()
            .all(|(_, checked)| checked.is_ok())
    );
    place.served.stop().await;
}

/// A proxy between the phone and its hub, as a hub that lies would be: what the phone sends for
/// the runner `real` it sends for `other`, and what the hub says is from `other` it says is from
/// `real`. Both are members, and `other` answers, with its own key.
struct Lying {
    address: SocketAddr,
    /// The keys `(real, other)` swapped, while it lies.
    swap: Arc<Mutex<Option<(Value, Value)>>>,
    /// The frames it changed.
    changed: Arc<AtomicUsize>,
}

impl Lying {
    fn to(hub: SocketAddr, real: &PublicKey, other: &PublicKey) -> Lying {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let lying = Lying {
            address: listener.local_addr().unwrap(),
            swap: Arc::new(Mutex::new(Some((json!(real), json!(other))))),
            changed: Arc::default(),
        };
        let (swap, changed) = (lying.swap.clone(), lying.changed.clone());
        std::thread::spawn(move || {
            for phone in listener.incoming() {
                let Ok(phone) = phone else { return };
                let Ok(hub) = TcpStream::connect(hub) else {
                    continue;
                };
                let (swap_up, changed_up) = (swap.clone(), changed.clone());
                let (phone_in, mut hub_out) =
                    (phone.try_clone().unwrap(), hub.try_clone().unwrap());
                std::thread::spawn(move || {
                    // Up, what the phone sends: its packets for `real` go to `other`.
                    pass(phone_in, &mut hub_out, |frame| {
                        let swapped = swap_up.lock().unwrap().clone();
                        if let Some((real, other)) = swapped
                            && frame["relay"]["to"] == real
                        {
                            frame["relay"]["to"] = other;
                            changed_up.fetch_add(1, Ordering::SeqCst);
                        }
                    });
                    let _ = hub_out.shutdown(Shutdown::Both);
                });
                let (swap_down, changed_down) = (swap.clone(), changed.clone());
                std::thread::spawn(move || {
                    let mut phone_out = phone;
                    // Down, what the hub sends: the packets of `other` are said to be `real`'s.
                    pass(hub, &mut phone_out, |frame| {
                        let swapped = swap_down.lock().unwrap().clone();
                        if let Some((real, other)) = swapped
                            && frame["relayed"]["from"] == other
                        {
                            frame["relayed"]["from"] = real;
                            changed_down.fetch_add(1, Ordering::SeqCst);
                        }
                    });
                    let _ = phone_out.shutdown(Shutdown::Both);
                });
            }
        });
        lying
    }

    /// From now on it tells the truth.
    fn tells_truth(&self) {
        self.swap.lock().unwrap().take();
    }

    fn changed(&self) -> usize {
        self.changed.load(Ordering::SeqCst)
    }
}

/// Copies the lines of `from` to `to`, `change` applied to each one that is a JSON frame.
fn pass(from: TcpStream, to: &mut TcpStream, change: impl Fn(&mut Value)) {
    let mut lines = BufReader::new(from);
    let mut line = Vec::new();
    while lines
        .read_until(b'\n', &mut line)
        .is_ok_and(|read| read > 0)
    {
        if let Ok(mut frame) = serde_json::from_slice::<Value>(&line) {
            change(&mut frame);
            line = serde_json::to_vec(&frame).unwrap();
            line.push(b'\n');
        }
        if to.write_all(&line).is_err() {
            return;
        }
        line.clear();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_through_the_hub_checks_the_runner_s_key() {
    // `server` and `other` are members, both reached through the hub.
    let place = Place::new().await;
    let other = add_runner(&place.served, place.dir.path(), "other", 2).await;
    let lying = Lying::to(
        place.served.address(),
        &place.runner.machine,
        &other.machine,
    );
    let phone = place.phone();
    place
        .joined_by(&phone, place.pair_code_at(lying.address))
        .await;
    let taken = Arc::new(Taken::default());
    phone.connect(taken).await.unwrap();

    // The hub hands the phone `other`'s packets as `server`'s: the link it dials does not
    // check, `server`'s key is not the one the answer proves, and nothing is asked of it.
    let within = |what: &'static str, holds: Box<dyn Fn(&ViewState) -> bool>| {
        let phone = phone.clone();
        async move {
            for _ in 0..1500 {
                let view = phone.view();
                if holds(&view) {
                    return view;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("{what} within 30 s: the view is {:#?}", phone.view());
        }
    };
    let view = within(
        "the link to server failed through the hub",
        Box::new(|view| {
            machine(view, "server").why.contains("through the hub") && lying.changed() > 0
        }),
    )
    .await;
    assert!(!machine(&view, "server").online, "{view:#?}");
    assert!(place.runner.taken("agent.list").is_empty());
    assert_eq!(place.runner.watching.load(Ordering::SeqCst), 0);
    // The hub tells the truth: the same dial, through the hub, links.
    lying.tells_truth();
    let view = within(
        "server linked through the hub once it is told apart",
        Box::new(|view| machine(view, "server").online),
    )
    .await;
    assert!(machine(&view, "server").through_hub, "{view:#?}");
    place.runner.watched().await;
    place.served.stop().await;
}

// ---- The session page (#826): the conversation, the composer, the cards, the controls ----

/// The agent `a1` as its daemon's `agent.show` and `agent.tree` give it.
fn a1(mode: &str, state: &str) -> Value {
    json!({"id": "a1", "title": "Fix the login", "project": "app", "parent": null,
        "state": state, "gate_mode": mode})
}

fn tree_of(agents: Value) -> Value {
    json!({"agents": agents})
}

fn the_page(view: &ViewState) -> &flottille_mobile::ConversationView {
    view.conversation.as_ref().expect("a page is followed")
}

fn params_of(took: &Took) -> Value {
    serde_json::from_str(took.0["signed"]["params"].as_str().unwrap()).unwrap()
}

fn png() -> Vec<u8> {
    vec![0x89, b'P', b'N', b'G', 1, 2, 3]
}

#[tokio::test(flavor = "multi_thread")]
async fn the_conversation_follows_live() {
    let place = Place::new().await;
    let runner = &place.runner;
    runner.emit(
        "agent.prompted",
        json!({"agent": "a1", "text": "Fix it [Image 1: shot.png]", "images": ["shot.png"]}),
    );
    runner.emit(
        "agent.message",
        json!({"agent": "a1", "text": "On it.", "message_id": "m1"}),
    );
    runner.emit(
        "agent.tool_call",
        json!({"agent": "a1", "id": "t1", "title": "Read login.rs", "update": false}),
    );
    runner.show(a1("auto", "idle"), None);
    runner.answer(
        "agent.tree",
        tree_of(
            json!([{"agent": a1("auto", "idle"), "doing": "idle", "waiting_on": null,
            "needs_user": false, "questions": []}]),
        ),
    );
    let (phone, _taken) = place.linked().await;
    phone
        .follow("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();

    let view = phone.view();
    let page = the_page(&view);
    assert_eq!(
        (page.title.as_str(), page.mode.as_str(), page.child),
        ("Fix the login", "auto", false)
    );
    let shown: Vec<(&str, &str)> = page
        .entries
        .iter()
        .map(|entry| (entry.kind.as_str(), entry.text.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            ("user", "Fix it [Image 1: shot.png]"),
            ("agent", "On it."),
            ("tool", "Read login.rs")
        ]
    );
    assert_eq!(page.entries[0].images, ["shot.png"]);
    assert!(page.entries.iter().all(|entry| entry.seq > 0));
    assert!(!page.working);

    // Live: a turn starts, its words come, then the mode changes under the user's hand.
    runner.emit(
        "agent.prompted",
        json!({"agent": "a1", "text": "And the tests"}),
    );
    runner.emit(
        "agent.message",
        json!({"agent": "a1", "text": "Done.", "message_id": "m2"}),
    );
    let view = until(&phone, "the live words", |view| {
        view.conversation.as_ref().is_some_and(|page| {
            page.entries
                .last()
                .is_some_and(|entry| entry.text == "Done.")
        })
    })
    .await;
    assert!(the_page(&view).working, "a turn runs");
    runner.emit("agent.turn_ended", json!({"agent": "a1"}));
    runner.emit(
        "agent.gate_mode_changed",
        json!({"agent": "a1", "mode": "full"}),
    );
    let view = until(&phone, "the mode and the end of the turn", |view| {
        view.conversation
            .as_ref()
            .is_some_and(|page| page.mode == "full" && !page.working)
    })
    .await;
    assert_eq!(the_page(&view).entries.len(), 5);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_conversation_pages_in_and_is_never_loaded_whole() {
    let place = Place::new().await;
    let runner = &place.runner;
    for n in 1..=30 {
        runner.emit(
            "agent.prompted",
            json!({"agent": "a1", "text": format!("message {n}")}),
        );
    }
    runner.show(a1("auto", "idle"), Some(10));
    let (phone, _taken) = place.linked().await;
    phone
        .follow("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();

    let view = phone.view();
    let page = the_page(&view);
    assert_eq!(page.entries.len(), 10);
    assert_eq!(page.entries[0].text, "message 21");
    assert!(page.more, "older messages remain");
    // One page was asked, the latest: a `before` past every event, never the whole conversation.
    let asked = runner.taken("agent.show");
    assert_eq!(asked.len(), 1);
    assert!(params_of(&asked[0])["before"].as_u64().unwrap() > 30);

    // Older ones come on request, before the first shown.
    runner.show(a1("auto", "idle"), Some(30));
    runner.answer(
        "agent.show",
        json!({"agent": a1("auto", "idle"), "more": false,
            "events": runner.events.lock().unwrap()[10..20].to_vec()}),
    );
    phone
        .older("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();
    let view = phone.view();
    let page = the_page(&view);
    assert_eq!(page.entries.len(), 20);
    assert_eq!(page.entries[0].text, "message 11");
    assert!(!page.more);
    let asked = runner.taken("agent.show");
    assert_eq!(params_of(&asked[1])["before"], 21);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_the_page_stops_following_it() {
    let place = Place::new().await;
    place.runner.show(a1("auto", "idle"), None);
    let (phone, _taken) = place.linked().await;
    phone
        .follow("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();
    assert!(phone.view().conversation.is_some());
    phone.unfollow();
    assert!(phone.view().conversation.is_none());
    place
        .runner
        .emit("agent.message", json!({"agent": "a1", "text": "late"}));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(phone.view().conversation.is_none(), "nothing is kept of it");
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_with_a_picture_is_sent() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    phone
        .send(
            "server".to_owned(),
            "a1".to_owned(),
            "look at this".to_owned(),
            vec![Picture {
                mime: "image/png".to_owned(),
                data: png(),
            }],
        )
        .await
        .unwrap();
    let sent = place.runner.taken("agent.send");
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.is_ok(), "signed as a member's");
    let params = params_of(&sent[0]);
    assert_eq!(params["agent"], "a1");
    assert_eq!(params["message"], "look at this");
    assert_eq!(params["images"][0]["mime"], "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(params["images"][0]["data"].as_str().unwrap())
            .unwrap(),
        png()
    );

    // A picture alone is a message.
    phone
        .send(
            "server".to_owned(),
            "a1".to_owned(),
            String::new(),
            vec![Picture {
                mime: "image/jpeg".to_owned(),
                data: vec![0xFF, 0xD8, 1],
            }],
        )
        .await
        .unwrap();
    assert_eq!(place.runner.taken("agent.send").len(), 2);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_s_limits_on_pictures_hold_before_anything_is_sent() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    let send = |text: &str, pictures: Vec<Picture>| {
        phone.send(
            "server".to_owned(),
            "a1".to_owned(),
            text.to_owned(),
            pictures,
        )
    };
    let picture = |mime: &str, size: usize| Picture {
        mime: mime.to_owned(),
        data: vec![7; size],
    };
    let refused = |why: Result<(), MobileError>| why.unwrap_err().to_string();
    assert!(refused(send("x", vec![picture("image/png", 1); 21]).await).contains("20"));
    assert!(
        refused(send("x", vec![picture("image/png", 5 * 1024 * 1024 + 1)]).await).contains("5 MiB")
    );
    assert!(refused(send("x", vec![picture("image/bmp", 1)]).await).contains("image/bmp"));
    assert!(refused(send("", vec![]).await).contains("empty"));
    // A message travels in one frame of the link (1 MiB, pictures in base64 included): what cannot
    // fit is refused here with its reason, never sent to end the link.
    assert!(
        refused(send("x", vec![picture("image/webp", 5 * 1024 * 1024)]).await).contains("link")
    );
    assert!(refused(send("x", vec![picture("image/png", 400_000); 2]).await).contains("link"));
    send("x", vec![picture("image/webp", 600_000)])
        .await
        .unwrap();
    assert_eq!(
        place.runner.taken("agent.send").len(),
        1,
        "only the last was sent"
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picture_of_the_conversation_is_read_back() {
    let place = Place::new().await;
    place.runner.answer(
        "agent.image",
        json!({"mime": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png())}),
    );
    let (phone, _taken) = place.linked().await;
    let bytes = phone
        .image("server".to_owned(), "a1".to_owned(), "shot.png".to_owned())
        .await
        .unwrap();
    assert_eq!(bytes, png());
    let asked = place.runner.taken("agent.image");
    assert_eq!(
        params_of(&asked[0]),
        json!({"agent": "a1", "name": "shot.png"})
    );
    place.served.stop().await;
}

fn ask_event(runner: &Runner) -> u64 {
    runner.emit(
        "agent.asked",
        json!({"agent": "a1", "to": "user", "question": "1. Push?\n2. Title?",
            "questions": [
                {"question": "Push `feat/42-export` and open the pull request?",
                 "options": [{"label": "Yes", "description": "Squash first"}, {"label": "No"}]},
                {"question": "Title of the pull request?"}]}),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_is_answered_from_its_card() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    let asked = ask_event(&place.runner);
    let view = until(&phone, "the ask", |view| !view.asks.is_empty()).await;
    let card = &view.asks[0];
    assert_eq!((card.agent.as_str(), card.asked), ("a1", asked));
    assert_eq!(card.questions.len(), 2);
    assert_eq!(card.questions[0].options[0].label, "Yes");
    assert_eq!(card.questions[0].options[0].description, "Squash first");
    assert!(!card.questions[0].multiple);
    assert!(card.questions[1].options.is_empty());

    phone
        .answer_ask(
            "server".to_owned(),
            "a1".to_owned(),
            asked,
            vec![
                Reply {
                    choices: vec!["Yes".to_owned()],
                    text: "Squash it first.".to_owned(),
                },
                Reply {
                    choices: vec![],
                    text: "Export orders as CSV".to_owned(),
                },
            ],
        )
        .await
        .unwrap();
    let answered = place.runner.taken("agent.answer");
    assert_eq!(
        params_of(&answered[0]),
        json!({"agent": "a1", "question": asked, "answers": [
            {"choices": ["Yes"], "text": "Squash it first."},
            {"text": "Export orders as CSV"}]})
    );

    // An ask the daemon no longer waits for is refused, and the sentence reaches the card.
    place
        .runner
        .refuse("agent.answer", "that question does not wait for an answer");
    let why = phone
        .answer_ask("server".to_owned(), "a1".to_owned(), asked, vec![])
        .await
        .unwrap_err();
    assert_eq!(why.to_string(), "that question does not wait for an answer");
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_that_waits_when_the_page_opens_is_on_its_card() {
    let place = Place::new().await;
    place.runner.show(a1("auto", "idle"), None);
    place.runner.answer(
        "agent.tree",
        tree_of(
            json!([{"agent": a1("auto", "idle"), "doing": "waiting for you",
            "waiting_on": "user", "needs_user": true,
            "questions": [{"seq": 12, "at": 1, "question": "Which branch?",
                "questions": [{"question": "Which branch?",
                    "options": [{"label": "main"}, {"label": "dev"}], "multiple": true}]}]}]),
        ),
    );
    let (phone, _taken) = place.linked().await;
    phone
        .follow("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();
    let view = phone.view();
    assert_eq!(view.asks.len(), 1);
    assert_eq!(view.asks[0].asked, 12);
    assert!(view.asks[0].questions[0].multiple);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_is_allowed_or_denied() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    place.runner.emit(
        "approval.requested",
        json!({"number": 4, "agent": "a1", "tool": "run", "call": {"line": "rm -rf build"},
            "reasons": ["it deletes files", "outside the project"], "class": "base",
            "kind": "call", "says": [], "judgement": null, "held_by": "user"}),
    );
    place.runner.emit(
        "approval.requested",
        json!({"number": 5, "agent": "a1", "tool": "fs_read", "call": {"path": ".env"},
            "reasons": ["a sensitive place"], "class": "judge", "kind": "call", "says": [],
            "judgement": {"by": "claude haiku", "verdict": "ask", "reason": "it reads secrets"},
            "held_by": "user"}),
    );
    let view = until(&phone, "both requests", |view| view.requests.len() == 2).await;
    let base = view.requests.iter().find(|r| r.number == 4).unwrap();
    // The call as the window shows it, and the base class that always asks.
    assert_eq!(base.call, "rm -rf build");
    assert!(base.base);
    assert_eq!(base.kind, "call");
    let judged = view.requests.iter().find(|r| r.number == 5).unwrap();
    assert_eq!(judged.call, r#"fs_read {"path":".env"}"#);
    assert!(!judged.base);
    assert_eq!(
        judged.judgement,
        "The judge (claude haiku) asks you: it reads secrets"
    );

    phone.answer("server".to_owned(), 4, true).await.unwrap();
    phone.answer("server".to_owned(), 5, false).await.unwrap();
    assert_eq!(
        params_of(&place.runner.taken("approval.approve")[0]),
        json!({"number": 4})
    );
    assert_eq!(
        params_of(&place.runner.taken("approval.deny")[0]),
        json!({"number": 5})
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_card_says_what_the_window_says() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    place.runner.emit(
        "approval.requested",
        json!({"number": 6, "agent": "a1", "tool": "run", "call": {"line": "rm -rf build"},
            "reasons": ["it deletes files", "outside the project"], "class": "base",
            "kind": "call", "says": [], "judgement": null, "held_by": "user",
            "forwards": [{"by": "a2", "why": "it needs the build gone"}]}),
    );
    place.runner.emit(
        "approval.requested",
        json!({"number": 7, "agent": "a1", "tool": "session_send", "call": {"line": "hello"},
            "reasons": ["this session has not written to that one before: the user lets it"],
            "class": "base", "kind": "call", "says": [], "judgement": null, "held_by": "user"}),
    );
    let view = until(&phone, "both requests", |view| view.requests.len() == 2).await;
    let base = view.requests.iter().find(|r| r.number == 6).unwrap();
    // Every reason and who forwarded it, as the window says them, and who asked.
    assert_eq!(
        base.why,
        "It deletes files. Outside the project. Agent a2 forwards it: it needs the build gone."
    );
    assert_eq!(
        base.source,
        "asked by agent a1 · forwarded by agent a2 · #6"
    );
    // The strong warning of a base request: the window's own sentence, shared.
    assert_eq!(base.always, flottille_protocol::words::BASE_SAYS);
    assert!(base.always.contains("in Ask and in Auto"));
    // The first message to another session says what it asks, not the generic warning.
    let leave = view.requests.iter().find(|r| r.number == 7).unwrap();
    assert_eq!(leave.always, flottille_protocol::words::LEAVE_SAYS);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_card_says_which_agent_of_the_tree_asked() {
    let place = Place::new().await;
    place.runner.show(a1("ask", "idle"), None);
    place.runner.answer(
        "agent.tree",
        tree_of(json!([
            {"agent": a1("ask", "idle"), "doing": "idle", "waiting_on": null,
                "needs_user": false, "questions": []},
            {"agent": {"id": "c1", "title": "Check the tests", "parent": "a1", "depth": 1,
                "role": "worker", "state": "working", "model": "sonnet"},
                "doing": "working", "waiting_on": null, "needs_user": false, "questions": []}])),
    );
    let (phone, _taken) = place.linked().await;
    phone
        .follow("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();
    for (number, agent) in [(1, "a1"), (2, "c1")] {
        place.runner.emit(
            "approval.requested",
            json!({"number": number, "agent": agent, "tool": "run", "call": {"line": "ls"},
                "reasons": ["it runs"], "class": "judge", "kind": "call", "says": [],
                "judgement": null, "held_by": "user"}),
        );
    }
    let view = until(&phone, "both requests", |view| view.requests.len() == 2).await;
    let source = |number| {
        view.requests
            .iter()
            .find(|r| r.number == number)
            .unwrap()
            .source
            .clone()
    };
    assert_eq!(source(1), "asked by the conversation · #1");
    assert_eq!(source(2), "asked by Check the tests · #2");
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_proposal_is_an_action_the_phone_cannot_allow() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    place.runner.emit(
        "approval.requested",
        json!({"number": 8, "agent": "a1", "tool": "connections", "call": {"action": "save"},
            "reasons": ["it writes a connection"], "class": "judge", "kind": "action",
            "says": ["Adds the connection `docs`."], "judgement": null, "held_by": "user"}),
    );
    let view = until(&phone, "the request", |view| !view.requests.is_empty()).await;
    assert_eq!(view.requests[0].kind, "action");
    assert_eq!(view.requests[0].tool, "connections");
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_for_a_request_that_is_decided_or_gone_says_so() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    let answer = || phone.answer("server".to_owned(), 4, true);
    // The daemon's own words for a request already decided, and for one it has not.
    place.runner.refuse(
        "approval.approve",
        "request #4 is already decided: it was once",
    );
    assert!(matches!(
        answer().await.unwrap_err(),
        MobileError::Gone { .. }
    ));
    place
        .runner
        .refuse_as("approval.approve", "not-found", "there is no request #4");
    assert!(matches!(
        answer().await.unwrap_err(),
        MobileError::Gone { .. }
    ));
    // Any other refusal is not the end of the request: its card stays.
    place
        .runner
        .refuse("approval.approve", "waiting for the vault entries it names");
    assert!(matches!(
        answer().await.unwrap_err(),
        MobileError::Refused { .. }
    ));
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_whose_escapes_pass_the_frame_is_refused() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    // 400,000 quotes are 800 KB as parameters, but the request carries them as a string, escaped
    // a second time: the frame is past 1 MiB while the parameters are not.
    let quotes = "\"".repeat(400_000);
    let why = phone
        .send("server".to_owned(), "a1".to_owned(), quotes, vec![])
        .await
        .unwrap_err()
        .to_string();
    assert!(why.contains("too big for the link"), "{why}");
    assert!(
        place.runner.taken("agent.send").is_empty(),
        "nothing was sent"
    );
    // The link is still there for what follows.
    phone
        .send(
            "server".to_owned(),
            "a1".to_owned(),
            "hello".to_owned(),
            vec![],
        )
        .await
        .unwrap();
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mode_is_switched_and_stop_interrupts_the_turn() {
    let place = Place::new().await;
    let (phone, _taken) = place.linked().await;
    for mode in ["ask", "auto", "full"] {
        phone
            .set_mode("server".to_owned(), "a1".to_owned(), mode.to_owned())
            .await
            .unwrap();
    }
    let modes: Vec<Value> = place
        .runner
        .taken("agent.mode")
        .iter()
        .map(params_of)
        .collect();
    assert_eq!(
        modes,
        [
            json!({"agent": "a1", "mode": "ask"}),
            json!({"agent": "a1", "mode": "auto"}),
            json!({"agent": "a1", "mode": "full"})
        ]
    );
    let why = phone
        .set_mode("server".to_owned(), "a1".to_owned(), "reckless".to_owned())
        .await
        .unwrap_err();
    assert!(why.to_string().contains("reckless"), "{why}");
    assert_eq!(place.runner.taken("agent.mode").len(), 3);

    phone
        .stop("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();
    assert_eq!(
        params_of(&place.runner.taken("agent.cancel")[0]),
        json!({"agent": "a1"})
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_child_s_conversation_is_a_child_s_and_the_tree_is_read() {
    let place = Place::new().await;
    place.runner.show(
        json!({"id": "c1", "title": "Check the tests", "parent": "a1", "state": "working",
            "gate_mode": "ask"}),
        None,
    );
    place.runner.answer(
        "agent.tree",
        tree_of(json!([
            {"agent": a1("ask", "working"), "doing": "working", "waiting_on": null,
                "needs_user": false, "questions": []},
            {"agent": {"id": "c1", "title": "Check the tests", "parent": "a1", "depth": 1,
                "role": "worker", "state": "working", "model": "sonnet"},
                "doing": "running `cargo test` for 12 min", "waiting_on": null,
                "needs_user": false, "questions": []}])),
    );
    let (phone, _taken) = place.linked().await;
    phone
        .follow("server".to_owned(), "c1".to_owned())
        .await
        .unwrap();
    let view = phone.view();
    let page = the_page(&view);
    assert!(page.child, "a child's page has no composer");
    assert_eq!(page.children.len(), 1);
    assert_eq!(page.children[0].agent, "c1");
    assert_eq!(page.children[0].doing, "running `cargo test` for 12 min");
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_viewer_s_writes_are_refused_by_the_machine() {
    let place = Place::new().await;
    let phone = place.phone();
    tokio::time::timeout(
        Duration::from_secs(10),
        phone.join(place.pair_code(), "tablet".to_owned(), "viewer".to_owned()),
    )
    .await
    .expect("paired within 10 s")
    .unwrap();
    let taken = Arc::new(Taken::default());
    connected(&phone, taken, &place.runner).await;
    assert_eq!(phone.view().role, "viewer");

    let (server, agent) = ("server".to_owned(), "a1".to_owned());
    let refusals = [
        (
            "send a message",
            phone
                .send(server.clone(), agent.clone(), "hi".into(), vec![])
                .await,
        ),
        (
            "answer a question",
            phone
                .answer_ask(server.clone(), agent.clone(), 1, vec![])
                .await,
        ),
        (
            "answer a request",
            phone.answer(server.clone(), 4, true).await,
        ),
        (
            "answer a request",
            phone.answer(server.clone(), 4, false).await,
        ),
        (
            "switch the mode",
            phone
                .set_mode(server.clone(), agent.clone(), "full".into())
                .await,
        ),
        (
            "stop a turn",
            phone.stop(server.clone(), agent.clone()).await,
        ),
    ];
    for (deed, result) in refusals {
        let why = result.unwrap_err().to_string();
        assert!(
            why.contains(&format!(
                "may only view: a controller or a runner may {deed}"
            )),
            "{why}"
        );
    }
    // The machine served none of them, and reads still work.
    for method in [
        "agent.send",
        "agent.answer",
        "approval.approve",
        "approval.deny",
        "agent.mode",
        "agent.cancel",
    ] {
        assert!(
            place
                .runner
                .taken(method)
                .iter()
                .all(|(_, checked)| checked.is_err()),
            "{method}"
        );
    }
    place.runner.show(a1("auto", "idle"), None);
    phone.follow(server, agent).await.unwrap();
    place.served.stop().await;
}

// ---- Tunnels on the phone (#831): a run's page, reached through the machine's link ----

/// A server that stands for a run's port: it answers every request with `hello from the run`.
async fn fake_run() -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 512];
                let _ = tcp.read(&mut request).await;
                let _ = tcp
                    .write_all(b"HTTP/1.0 200 OK\r\n\r\nhello from the run")
                    .await;
            });
        }
    });
    address
}

/// A server that stands for a run's port and keeps every connection open, saying nothing.
async fn holding_run() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 64];
                // Until the phone's end goes.
                while let Ok(read) = tokio::io::AsyncReadExt::read(&mut tcp, &mut buffer).await {
                    if read == 0 {
                        break;
                    }
                }
            });
        }
    });
    address
}

/// What the phone reads at `127.0.0.1:<port>`: the whole answer.
async fn fetch(port: u16) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tcp.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    let mut body = String::new();
    tokio::time::timeout(Duration::from_secs(10), tcp.read_to_string(&mut body))
        .await
        .expect("the run answers within 10 s")
        .unwrap();
    body
}

/// A runner whose run `r1` listens on the fake server, and a phone linked to it.
async fn with_a_run(place: &Place) -> (Arc<Core>, Arc<Taken>) {
    *place.runner.tunnel_target.lock().unwrap() = Some(fake_run().await);
    place.runner.answer(
        "run.list",
        json!({"commands": [
            {"run": "r1", "state": "running", "url": "http://localhost:5173/app?x=1#top"},
            {"run": "r2", "state": "running", "url": "http://localhost:8080/"},
        ]}),
    );
    place.linked().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_s_address_opens_through_a_tunnel_on_the_phone() {
    let place = Place::new().await;
    let (phone, taken) = with_a_run(&place).await;
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/app?x=1#top".to_owned(),
            false,
        )
        .await
        .unwrap();
    // The request is the one the command line signs: the run, no port.
    let asked = place.runner.taken("tunnel.open");
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1.is_ok(), "{:?}", asked[0].1);
    assert_eq!(params_of(&asked[0]), json!({"run": "r1"}));
    // The address is read on the phone, its path, query and fragment kept.
    assert_eq!(
        opened.url,
        format!("http://localhost:{}/app?x=1#top", opened.tunnel.local_port)
    );
    assert_eq!(opened.tunnel.run, "r1");
    // The bytes cross: what the phone's local port says is what the run says.
    assert_eq!(
        fetch(opened.tunnel.local_port).await,
        "HTTP/1.0 200 OK\r\n\r\nhello from the run"
    );
    assert_eq!(place.runner.tunnel_connections.load(Ordering::SeqCst), 1);
    let view = until(&phone, "the tunnel in the view", |view| {
        view.tunnels.len() == 1
    })
    .await;
    assert_eq!(view.tunnels[0], opened.tunnel);
    drop(taken);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tunnel_listens_on_the_loopback_only() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    let port = opened.tunnel.local_port;
    // Reachable on 127.0.0.1, and on no other address of the phone's.
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_an_agent_shows_names_the_run_that_serves_its_port() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let opened = phone
        .open_address(
            "server".to_owned(),
            None,
            "http://localhost:8080/docs".to_owned(),
            false,
        )
        .await
        .unwrap();
    // `run.list` was asked, then the run that reports port 8080 was named with that port.
    assert_eq!(place.runner.taken("run.list").len(), 1);
    assert_eq!(
        params_of(&place.runner.taken("tunnel.open")[0]),
        json!({"run": "r2", "port": 8080})
    );
    assert_eq!(opened.tunnel.run, "r2");
    assert_eq!(
        opened.url,
        format!("http://localhost:{}/docs", opened.tunnel.local_port)
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_of_the_web_opens_through_no_tunnel() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let why = phone
        .open_address(
            "server".to_owned(),
            None,
            "https://example.org/a".to_owned(),
            false,
        )
        .await
        .unwrap_err();
    assert!(why.to_string().contains("open it as it is"), "{why}");
    assert!(place.runner.taken("tunnel.open").is_empty());
    assert!(!flottille_mobile::address_is_local(
        "https://example.org/a".to_owned()
    ));
    assert!(flottille_mobile::address_is_local(
        "http://127.0.0.1:3000".to_owned()
    ));
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_tap_on_the_same_address_makes_no_second_tunnel() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let first = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    let again = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/other".to_owned(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(again.tunnel, first.tunnel);
    assert_eq!(
        again.url,
        format!("http://localhost:{}/other", first.tunnel.local_port)
    );
    assert_eq!(place.runner.taken("tunnel.open").len(), 1);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_tunnel_says_the_machine_s_reason_and_opens_nothing() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    place
        .runner
        .refuse("tunnel.open", "refused: port 22 is none of run r1's");
    let why = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:22/".to_owned(),
            false,
        )
        .await
        .unwrap_err();
    assert_eq!(why.to_string(), "refused: port 22 is none of run r1's");
    assert!(phone.view().tunnels.is_empty());
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tunnel_ends_with_its_run_and_the_view_says_why() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    place
        .runner
        .tunnel_ends
        .send("run r1 ended".to_owned())
        .unwrap();
    let view = until(&phone, "the tunnel to end", |view| view.tunnels.is_empty()).await;
    assert_eq!(
        view.tunnels_ended,
        [flottille_mobile::TunnelEnd {
            tunnel: opened.tunnel.tunnel.clone(),
            machine: "server".to_owned(),
            run: "r1".to_owned(),
            reason: "run r1 ended".to_owned(),
        }]
    );
    // Nothing listens any more.
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", opened.tunnel.local_port))
            .await
            .is_err()
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_tunnel_stops_its_listener_and_tells_the_machine() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    phone.close_tunnel(opened.tunnel.tunnel.clone()).unwrap();
    let view = until(&phone, "the tunnel to close", |view| {
        view.tunnels.is_empty()
    })
    .await;
    assert_eq!(view.tunnels_ended[0].reason, "closed by the user");
    for _ in 0..500 {
        if place.runner.tunnel_gone.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        place.runner.tunnel_gone.load(Ordering::SeqCst),
        1,
        "the machine saw it closed"
    );
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", opened.tunnel.local_port))
            .await
            .is_err()
    );
    // A tunnel that is not held is said.
    let why = phone.close_tunnel(opened.tunnel.tunnel).unwrap_err();
    assert!(why.to_string().contains("no tunnel"), "{why}");
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tunnel_ends_with_its_link() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    place.runner.cut();
    let view = until(&phone, "the tunnel to end with its link", |view| {
        view.tunnels.is_empty()
    })
    .await;
    assert_eq!(view.tunnels_ended.len(), 1);
    assert_eq!(view.tunnels_ended[0].tunnel, opened.tunnel.tunnel);
    assert!(!view.tunnels_ended[0].reason.is_empty());
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_lists_the_runs_that_have_an_address_and_the_links_shown() {
    let place = Place::new().await;
    let runner = &place.runner;
    runner.emit(
        "run.started",
        json!({"agent": "a1", "run": "r1", "name": null, "line": "pnpm dev", "folder": "/w",
            "ready": "starting", "url": null, "url_error": null}),
    );
    runner.emit(
        "run.started",
        json!({"agent": "a1", "run": "r2", "name": null, "line": "cargo test", "folder": "/w",
            "ready": null, "url": null, "url_error": null}),
    );
    runner.emit(
        "run.ready",
        json!({"agent": "a1", "run": "r1", "url": "http://localhost:5173/"}),
    );
    runner.emit(
        "agent.tool_call",
        json!({"agent": "a1", "id": "w", "title": "mcp__flottille__web", "update": false,
            "raw_input": {"action": "show_link", "url": "http://localhost:5173/"}}),
    );
    runner.emit(
        "agent.link_shown",
        json!({"agent": "a1", "kind": "url", "target": "http://localhost:5173/", "title": "The app",
            "host": "localhost", "shown": null}),
    );
    runner.emit(
        "agent.link_shown",
        json!({"agent": "a1", "kind": "file", "target": "/w/report.html", "title": null,
            "host": null, "shown": "report.html"}),
    );
    runner.show(a1("auto", "idle"), None);
    runner.answer(
        "agent.tree",
        tree_of(json!([{"agent": a1("auto", "idle"), "doing": "idle",
        "waiting_on": null, "needs_user": false, "questions": []}])),
    );
    let (phone, _taken) = place.linked().await;
    phone
        .follow("server".to_owned(), "a1".to_owned())
        .await
        .unwrap();
    let view = phone.view();
    let page = the_page(&view);
    // Only the run with an address, and the link of the web as an entry of its own (the step that
    // showed it is not drawn, the file of the machine is not on the phone).
    assert_eq!(
        page.runs,
        [flottille_mobile::RunView {
            run: "r1".to_owned(),
            line: "pnpm dev".to_owned(),
            url: "http://localhost:5173/".to_owned(),
        }]
    );
    let links: Vec<(&str, &str, &str)> = page
        .entries
        .iter()
        .map(|entry| {
            (
                entry.kind.as_str(),
                entry.text.as_str(),
                entry.link.as_str(),
            )
        })
        .collect();
    assert_eq!(links, [("link", "The app", "http://localhost:5173/")]);
    // The run ends: it is no longer on the page.
    runner.emit(
        "run.ended",
        json!({"agent": "a1", "run": "r1", "state": "exited", "exit_code": 0}),
    );
    until(&phone, "the run to leave the page", |view| {
        view.conversation
            .as_ref()
            .is_some_and(|page| page.runs.is_empty())
    })
    .await;
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tunnel_listens_on_a_random_port_never_the_run_s_own() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    // The run's port is a guessable one (5173), free on this machine: it is still not the one taken,
    // for another app of the phone would find it first.
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(opened.tunnel.port, 5173);
    assert_ne!(opened.tunnel.local_port, 5173);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_carries_at_most_64_connections_at_once() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    *place.runner.tunnel_target.lock().unwrap() = Some(holding_run().await);
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    let port = opened.tunnel.local_port;
    let mut held = Vec::new();
    for _ in 0..64 {
        held.push(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    }
    for _ in 0..500 {
        if place.runner.tunnel_connections.load(Ordering::SeqCst) == 64 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(place.runner.tunnel_connections.load(Ordering::SeqCst), 64);
    // The 65th is closed at once, and never reaches the machine.
    let mut past = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::AsyncReadExt::read(&mut past, &mut byte),
    )
    .await
    .expect("the connection past 64 is closed, not kept");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    assert_eq!(place.runner.tunnel_connections.load(Ordering::SeqCst), 64);
    // One of the 64 goes: a place is free again.
    drop(held.pop());
    let mut carried = false;
    for _ in 0..250 {
        let _again = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        if place.runner.tunnel_connections.load(Ordering::SeqCst) > 64 {
            carried = true;
            break;
        }
    }
    assert!(carried, "a connection is carried again once one went");
    place.served.stop().await;
}

/// A tunnel the machine stops being asked about: for how long the view still holds it.
async fn still_held(phone: &Core, tunnel: &str, for_how_long: Duration) -> bool {
    let end = tokio::time::Instant::now() + for_how_long;
    while tokio::time::Instant::now() < end {
        if !phone.view().tunnels.iter().any(|held| held.tunnel == tunnel) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_for_the_browser_ends_when_it_is_idle_and_the_view_says_why() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    phone.set_browser_idle(Duration::from_millis(300));
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            true,
        )
        .await
        .unwrap();
    // One page load, then nothing: ended 300 ms after it.
    assert!(fetch(opened.tunnel.local_port).await.contains("hello"));
    let view = until(&phone, "the idle tunnel to end", |view| {
        view.tunnels.is_empty()
    })
    .await;
    assert!(
        view.tunnels_ended[0].reason.starts_with("no connection for"),
        "{:?}",
        view.tunnels_ended
    );
    for _ in 0..500 {
        if place.runner.tunnel_gone.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(place.runner.tunnel_gone.load(Ordering::SeqCst), 1);
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", opened.tunnel.local_port))
            .await
            .is_err()
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_for_the_browser_is_not_idle_while_a_connection_is_open() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    *place.runner.tunnel_target.lock().unwrap() = Some(holding_run().await);
    phone.set_browser_idle(Duration::from_millis(300));
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            true,
        )
        .await
        .unwrap();
    let held = tokio::net::TcpStream::connect(("127.0.0.1", opened.tunnel.local_port))
        .await
        .unwrap();
    assert!(still_held(&phone, &opened.tunnel.tunnel, Duration::from_millis(900)).await);
    // Its last connection goes: the idle time runs from then.
    drop(held);
    until(&phone, "the tunnel to end once idle", |view| {
        view.tunnels.is_empty()
    })
    .await;
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_of_the_app_has_no_idle_end() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    phone.set_browser_idle(Duration::from_millis(200));
    let opened = phone
        .open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            false,
        )
        .await
        .unwrap();
    assert!(still_held(&phone, &opened.tunnel.tunnel, Duration::from_millis(800)).await);
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_app_and_the_browser_never_share_a_tunnel() {
    let place = Place::new().await;
    let (phone, _taken) = with_a_run(&place).await;
    let open = |browser| {
        phone.open_address(
            "server".to_owned(),
            Some("r1".to_owned()),
            "http://localhost:5173/".to_owned(),
            browser,
        )
    };
    let app = open(false).await.unwrap();
    let browser = open(true).await.unwrap();
    assert_ne!(app.tunnel.tunnel, browser.tunnel.tunnel);
    assert_ne!(app.tunnel.local_port, browser.tunnel.local_port);
    // Each use finds its own again.
    assert_eq!(open(false).await.unwrap().tunnel, app.tunnel);
    assert_eq!(open(true).await.unwrap().tunnel, browser.tunnel);
    // The app closes its own: the browser's stays.
    phone.close_tunnel(app.tunnel.tunnel.clone()).unwrap();
    until(&phone, "the app's tunnel to close", |view| {
        view.tunnels.len() == 1
    })
    .await;
    assert!(still_held(&phone, &browser.tunnel.tunnel, Duration::from_millis(200)).await);
    place.served.stop().await;
}
