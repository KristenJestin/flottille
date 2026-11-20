//! The mobile core against a hub and a runner on loopback (`docs/remote.md`, "The Android app"):
//! the hub served in this process from a temporary state folder, the runner `server` paired on it,
//! and the runner `server`'s endpoint, which stands for its daemon: it takes the phone's client
//! link as a daemon does (the peer a member's machine), checks each request as a daemon does
//! (`Requests::check`: a member, its signature, its time, its nonce, its role) and answers it,
//! `events.watch` with its stored events after the number asked, then the new ones. The phone is a
//! `Core` over a temporary folder, its device key sealed by a test sealer, paired from the code of
//! the hub's pair mode (#1146).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Accepted, Action, Ask, Change, Colour, Device, DeviceKey, Endpoint, Hello, HubKey, Icon,
    Keyring, KeyringError, Link, MachineKey, Members, Message, PairCode, Proposed, Pushed,
    Requests, Role, Tls, ToDevice, arrive,
};
use flottille_mobile::{Batch, Core, Listener, MobileError, Sealer, ViewState};
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
    links: Arc<Mutex<Vec<Link>>>,
    requests: Arc<Mutex<Requests>>,
    /// The watches subscribed, which no new event can miss from then on.
    watching: Arc<AtomicUsize>,
}

impl Runner {
    fn new() -> Runner {
        let mut answers = HashMap::new();
        answers.insert("agent.list".to_owned(), json!({"agents": []}));
        answers.insert("approval.list".to_owned(), json!({"requests": []}));
        Runner {
            members: Arc::default(),
            events: Arc::default(),
            emitted: tokio::sync::broadcast::channel(4096).0,
            taken: Arc::default(),
            answers: Arc::new(Mutex::new(answers)),
            links: Arc::default(),
            requests: Arc::new(Mutex::new(Requests::new(SystemTime::now()))),
            watching: Arc::default(),
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

    /// Once the phone's watch is subscribed.
    async fn watched(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.watching.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the phone watches within 10 s");
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
                let Ok(link) = accepting.handshake(&hello(), "phone").await else {
                    return;
                };
                runner.links.lock().unwrap().push(link.clone());
                runner.serve_link(link).await;
            });
        }
    }

    async fn serve_link(&self, link: Link) {
        while let Some(received) = link.next().await {
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
                .check(&request.to_string(), "server", &members, SystemTime::now())
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
                    let result = runner
                        .answers
                        .lock()
                        .unwrap()
                        .get(&accepted.method)
                        .cloned()
                        .unwrap_or_else(|| json!({}));
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
    async fn new() -> Place {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::open(&dir.path().join("hub"), SystemClock).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let served = flottille_hub::serve(hub, listener, None, hello()).unwrap();
        // `server` pairs as a computer does: its machine's session, then its machine declared.
        let server = device_key(1);
        let machine = MachineKey::load_or_make(&dir.path().join("machines/server")).unwrap();
        let code = served.with_hub(|hub| hub.open_pairing(&url(served.address()), None).unwrap().1);
        let mut paired = Device::pair(
            &code,
            Proposed {
                key: server.public(),
                name: "server".to_owned(),
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

        let runner = Runner::new();
        *runner.members.lock().unwrap() = Some(device.members().unwrap());
        let endpoint = Endpoint::bind(&machine, "127.0.0.1:0".parse().unwrap()).unwrap();
        let at: SocketAddr = endpoint.listening().unwrap();
        session
            .ask(Ask::Addresses {
                addresses: vec![at.to_string()],
            })
            .await
            .unwrap();
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
        Place {
            dir,
            served,
            runner,
        }
    }

    /// Pair mode opened on the hub, as `flottille-hub pair` opens it: the code its QR holds.
    fn pair_code(&self) -> String {
        let at = url(self.served.address());
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
        let code = self.pair_code();
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
        phone.connect(taken.clone()).await.unwrap();
        until(&phone, "server online", |view| {
            view.machines
                .iter()
                .any(|machine| machine.name == "server" && machine.online)
        })
        .await;
        self.runner.watched().await;
        (phone, taken)
    }
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

    // The device key is kept sealed by the app's key, never in clear.
    let sealed = std::fs::read(Path::new(&place.phone_folder()).join("device.sealed")).unwrap();
    assert!(sealed.starts_with(MARK));

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
    let code = PairCode::from_code(&place.pair_code()).unwrap();
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

    // The hub's own code pairs, and the hub's key is the one the phone kept.
    place.joined(&phone).await;
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
