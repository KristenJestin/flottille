//! The links' packets the hub carries between the user's devices (`docs/remote.md`, "The
//! transport", "The hub"): a hub served on 127.0.0.1 from a temporary state folder, and devices
//! whose machines hold control sessions to it, each with the relay port of its session. What a
//! packet holds is the devices' own (QUIC, encrypted end to end): here it is plain bytes, since
//! the hub never looks at them.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use flottille_hub::{Hub, RELAY_BURST, RELAY_RATE, Served, SystemClock, Waits};
use flottille_link::{
    Action, Ask, Change, Colour, Device, DeviceKey, Hello, Icon, Keyring, KeyringError, MachineKey,
    Notice, NoticeKind, PairCode, Paired, Proposed, PublicKey, Pushed, RelayPort, Role, Session,
    Tls,
};
use zeroize::Zeroizing;

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
    Hello::ours("0.4.0")
}

fn url(address: SocketAddr) -> String {
    format!("http://{address}")
}

/// A hub, `pc` paired with it, its machine's session open, and the folder of the machines' keys.
struct Place {
    dir: tempfile::TempDir,
    served: Served<SystemClock>,
    pc_device: Device,
    pc_machine: MachineKey,
    pc_session: Session,
}

impl Place {
    async fn new() -> Place {
        Place::with(Waits::default()).await
    }

    /// [`Place::new`], the hub's waits `waits`.
    async fn with(waits: Waits) -> Place {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::open(&dir.path().join("hub"), SystemClock).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let served = flottille_hub::serve_with(hub, listener, None, hello(), waits).unwrap();
        let pc_machine = machine_in(dir.path(), "pc");
        let mut paired = pair(&served, &device_key(1), &pc_machine, "pc").await;
        let (pc_device, _) = paired.listed().await.unwrap();
        let pc_session = declare(pc_device.clone(), paired, &device_key(1), &pc_machine).await;
        Place {
            dir,
            served,
            pc_device,
            pc_machine,
            pc_session,
        }
    }

    fn machine(&self, name: &str) -> MachineKey {
        machine_in(self.dir.path(), name)
    }

    /// `device` pairs as `name`, a runner, and declares `machine`: its machine's session, a
    /// member's machine's now.
    async fn add(&self, device: &DeviceKey, machine: &MachineKey, name: &str) -> Session {
        let mut paired = pair(&self.served, device, machine, name).await;
        let (pinned, _) = paired.listed().await.unwrap();
        declare(pinned, paired, device, machine).await
    }

    /// The member `name` removed on the hub's host.
    fn remove(&self, name: &str) {
        self.served.with_hub(|hub| hub.remove(name).unwrap());
    }

    /// A session of `machine` to the hub `pc` paired with, as a member's machine opens it.
    async fn connect(&self, machine: &MachineKey) -> Session {
        self.pc_device
            .connect(machine, &hello(), &Tls::System)
            .await
            .unwrap()
    }
}

/// Pair mode opened on the hub, and `device` paired with its code through `machine`'s session,
/// its machine not declared yet.
async fn pair(
    served: &Served<SystemClock>,
    device: &DeviceKey,
    machine: &MachineKey,
    name: &str,
) -> Paired {
    let code: PairCode =
        served.with_hub(|hub| hub.open_pairing(&url(served.address()), None).unwrap().1);
    let proposed = Proposed {
        key: device.public(),
        name: name.to_owned(),
        role: Role::Runner,
        icon: Icon::Desktop,
        colour: Colour::of(&device.public()),
    };
    Device::pair(&code, proposed, machine, &hello(), &Tls::System)
        .await
        .unwrap()
}

/// The device of `paired` declares `machine`, through its session: that session.
async fn declare(
    mut pinned: Device,
    mut paired: Paired,
    device: &DeviceKey,
    machine: &MachineKey,
) -> Session {
    let declared = Change::sign(
        pinned.list.head(),
        Action::Machine {
            key: device.public(),
            machine: machine.public(),
        },
        device,
    )
    .claimed_by(machine);
    pinned.submit(&mut paired.session, declared).await.unwrap();
    paired.session
}

fn machine_in(dir: &Path, name: &str) -> MachineKey {
    MachineKey::load_or_make(&dir.join("machines").join(name)).unwrap()
}

/// The relay port of `session`, taken once.
fn port(session: &mut Session) -> RelayPort {
    session.relay().expect("a relay port, taken once")
}

/// Until `port` is told that `keys` are all online, within a few seconds.
async fn online(port: &mut RelayPort, keys: &[PublicKey]) {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let now = port.online();
            if keys.iter().all(|key| now.contains(key)) {
                return;
            }
            port.online_changed().await;
        }
    });
    waited
        .await
        .unwrap_or_else(|_| panic!("not all online: {:?}", port.online()));
}

/// The packets `port` receives within `wait`.
async fn received_within(port: &mut RelayPort, wait: Duration) -> Vec<(PublicKey, Vec<u8>)> {
    let mut packets = Vec::new();
    let until = tokio::time::Instant::now() + wait;
    while let Ok(Some(packet)) = tokio::time::timeout_at(until, port.recv()).await {
        packets.push(packet);
    }
    packets
}

/// Keeps a session's pushes read, so that the hub's news never wait on it.
fn drain(mut session: Session) -> tokio::task::JoinHandle<Session> {
    tokio::spawn(async move {
        while session.next().await.is_some() {}
        session
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hub_forwards_only_between_members() {
    let mut place = Place::new().await;
    let (server, server_machine) = (device_key(2), place.machine("server"));
    let (laptop, laptop_machine) = (device_key(3), place.machine("laptop"));
    let mut server_session = place.add(&server, &server_machine, "server").await;
    drop(place.add(&laptop, &laptop_machine, "laptop").await);
    let mut pc_port = port(&mut place.pc_session);
    let mut server_port = port(&mut server_session);
    let _server_session = drain(server_session);
    online(
        &mut pc_port,
        &[place.pc_machine.public(), server_machine.public()],
    )
    .await;

    // Between two members' machines, both online, a packet goes through, said to be from its
    // sender.
    assert!(
        pc_port
            .send(&server_machine.public(), b"between members".to_vec())
            .await
    );
    let got = received_within(&mut server_port, Duration::from_secs(5)).await;
    assert_eq!(
        got,
        vec![(place.pc_machine.public(), b"between members".to_vec())]
    );

    // To a key that is no member's, and to a member's machine whose session is not open, the hub
    // keeps nothing: the laptop, connecting later, is sent nothing of what was sent before.
    let stranger = place.machine("stranger");
    pc_port
        .send(&stranger.public(), b"to a stranger".to_vec())
        .await;
    pc_port
        .send(&laptop_machine.public(), b"while it is off".to_vec())
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut laptop_session = place.connect(&laptop_machine).await;
    let mut laptop_port = port(&mut laptop_session);
    let _laptop_session = drain(laptop_session);
    online(&mut pc_port, &[laptop_machine.public()]).await;
    assert!(
        received_within(&mut laptop_port, Duration::from_secs(1))
            .await
            .is_empty()
    );
    assert!(!pc_port.online().contains(&stranger.public()));

    // A machine that pairs and has not declared itself yet is welcome, and is no member's: nothing
    // goes to it or from it, and it is not said online.
    let phone_machine = place.machine("phone");
    let mut paired = pair(&place.served, &device_key(4), &phone_machine, "phone").await;
    let mut phone_port = port(&mut paired.session);
    phone_port
        .send(&server_machine.public(), b"from a joiner".to_vec())
        .await;
    server_port
        .send(&phone_machine.public(), b"to a joiner".to_vec())
        .await;
    assert!(
        received_within(&mut server_port, Duration::from_secs(1))
            .await
            .is_empty()
    );
    assert!(
        received_within(&mut phone_port, Duration::from_secs(1))
            .await
            .is_empty()
    );
    assert!(!pc_port.online().contains(&phone_machine.public()));

    // A member removed is cut, and nothing goes to its machine any more.
    place.remove("laptop");
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        while pc_port.online().contains(&laptop_machine.public()) {
            pc_port.online_changed().await;
        }
    });
    waited.await.expect("the removed laptop is said offline");
    pc_port
        .send(&laptop_machine.public(), b"after its removal".to_vec())
        .await;
    assert!(
        received_within(&mut laptop_port, Duration::from_secs(1))
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn one_device_cannot_fill_the_hub() {
    let mut place = Place::new().await;
    let (server, server_machine) = (device_key(2), place.machine("server"));
    let (laptop, laptop_machine) = (device_key(3), place.machine("laptop"));
    let (tablet, tablet_machine) = (device_key(4), place.machine("tablet"));
    let pc_port = port(&mut place.pc_session);
    let mut ports = Vec::new();
    for (device, machine, name) in [
        (&server, &server_machine, "server"),
        (&laptop, &laptop_machine, "laptop"),
        (&tablet, &tablet_machine, "tablet"),
    ] {
        let mut session = place.add(device, machine, name).await;
        ports.push(port(&mut session));
        drain(session);
    }
    let mut tablet_port = ports.pop().unwrap();
    let mut laptop_port = ports.pop().unwrap();
    let mut server_port = ports.pop().unwrap();
    online(
        &mut laptop_port,
        &[
            place.pc_machine.public(),
            server_machine.public(),
            tablet_machine.public(),
        ],
    )
    .await;

    // The pc sends the server all it can for two seconds, a packet of a link's size at a time.
    let target = server_machine.public();
    let flooding = tokio::spawn(async move {
        let packet = vec![7u8; 1200];
        let started = Instant::now();
        let mut sent = 0usize;
        while started.elapsed() < Duration::from_secs(2) {
            if pc_port.send(&target, packet.clone()).await {
                sent += packet.len();
            }
        }
        (sent, started.elapsed(), pc_port)
    });
    let counting = tokio::spawn(async move {
        let mut bytes = 0usize;
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        while let Ok(Some((_, packet))) = tokio::time::timeout_at(until, server_port.recv()).await {
            bytes += packet.len();
        }
        bytes
    });

    // Meanwhile the laptop and the tablet go on: each packet one sends, the other gets.
    let mut answered = 0;
    for n in 0..20u8 {
        laptop_port
            .send(&tablet_machine.public(), vec![n; 100])
            .await;
        let got = tokio::time::timeout(Duration::from_secs(2), tablet_port.recv())
            .await
            .expect("the tablet gets the laptop's packet while the pc floods the server");
        if got == Some((laptop_machine.public(), vec![n; 100])) {
            answered += 1;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(answered, 20);

    let (sent, took, pc_port) = flooding.await.unwrap();
    let forwarded = counting.await.unwrap();
    // The hub forwarded no more than the bound lets through, and the rest was dropped: the pc
    // was slowed, not given the hub's memory.
    let allowed = RELAY_BURST as f64 + RELAY_RATE as f64 * (took.as_secs_f64() + 1.0);
    assert!(
        (forwarded as f64) <= allowed,
        "{forwarded} bytes forwarded of {sent} sent in {took:?}, past the bound {allowed}"
    );
    assert!(
        forwarded < sent,
        "{forwarded} forwarded of {sent}: nothing dropped"
    );
    // The pc's session goes on, its packets taken again once under its bound.
    tokio::time::sleep(Duration::from_secs(1)).await;
    pc_port
        .send(&laptop_machine.public(), b"after the flood".to_vec())
        .await;
    let got = received_within(&mut laptop_port, Duration::from_secs(3)).await;
    assert!(
        got.contains(&(place.pc_machine.public(), b"after the flood".to_vec())),
        "{} packets",
        got.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_leaves_the_other_sessions_answered() {
    let mut place = Place::new().await;
    let (server, server_machine) = (device_key(2), place.machine("server"));
    let (laptop, laptop_machine) = (device_key(3), place.machine("laptop"));
    let server_session = place.add(&server, &server_machine, "server").await;
    let mut laptop_session = place.add(&laptop, &laptop_machine, "laptop").await;
    let mut laptop_port = port(&mut laptop_session);
    let laptop = laptop_session.asker();
    let _laptop_session = drain(laptop_session);
    let mut server_session = server_session;
    let mut server_port = port(&mut server_session);
    let _server_session = drain(server_session);
    let pc_port = port(&mut place.pc_session);
    online(
        &mut laptop_port,
        &[place.pc_machine.public(), server_machine.public()],
    )
    .await;

    // The pc sends all it can for three seconds, to the server and to a key that is no member's.
    let (target, stranger) = (server_machine.public(), place.machine("stranger").public());
    let flooding = tokio::spawn(async move {
        let packet = vec![7u8; 1200];
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            pc_port.send(&target, packet.clone()).await;
            pc_port.send(&stranger, packet.clone()).await;
        }
        pc_port
    });
    let reading = tokio::spawn(async move {
        let until = tokio::time::Instant::now() + Duration::from_secs(4);
        while let Ok(Some(_)) = tokio::time::timeout_at(until, server_port.recv()).await {}
    });

    // Meanwhile each ask of the laptop is answered at once, as with no flood.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut slowest = Duration::ZERO;
    for n in 0..20u64 {
        let asked = Instant::now();
        laptop
            .ask(Ask::Presence {
                active: true,
                last_input: n,
            })
            .await
            .unwrap();
        slowest = slowest.max(asked.elapsed());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        slowest < Duration::from_millis(200),
        "an ask of the laptop waited {slowest:?} while the pc flooded"
    );
    drop(flooding.await.unwrap());
    reading.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_packet_to_no_one_costs_its_sender() {
    let mut place = Place::new().await;
    let (server, server_machine) = (device_key(2), place.machine("server"));
    let mut server_session = place.add(&server, &server_machine, "server").await;
    let mut server_port = port(&mut server_session);
    let _server_session = drain(server_session);
    let mut pc_port = port(&mut place.pc_session);
    online(&mut pc_port, &[server_machine.public()]).await;

    // Three bursts' worth to a key that is no member's, then a hundred packets to the server at
    // once: the first spent what the pc may send, and the server is sent little of the rest.
    let stranger = place.machine("stranger").public();
    let packet = vec![7u8; 1200];
    for _ in 0..(3 * RELAY_BURST as usize).div_ceil(packet.len()) {
        pc_port.send(&stranger, packet.clone()).await;
    }
    for _ in 0..100 {
        pc_port.send(&server_machine.public(), packet.clone()).await;
    }
    let got = received_within(&mut server_port, Duration::from_secs(2)).await;
    assert!(
        got.len() < 50,
        "{} of 100 packets went through after three bursts to no one",
        got.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_removed_is_sent_nothing_while_its_session_is_open() {
    let mut place = Place::new().await;
    let (laptop, laptop_machine) = (device_key(3), place.machine("laptop"));
    let mut laptop_session = place.add(&laptop, &laptop_machine, "laptop").await;
    let mut laptop_port = port(&mut laptop_session);
    let _laptop_session = drain(laptop_session);
    let mut pc_port = port(&mut place.pc_session);
    online(&mut pc_port, &[laptop_machine.public()]).await;

    // Right after its removal, before its session is closed, the laptop is no member: what the pc
    // sends it is dropped.
    place.remove("laptop");
    for n in 0..20u8 {
        assert!(pc_port.try_send(&laptop_machine.public(), vec![n; 100]));
    }
    assert!(
        received_within(&mut laptop_port, Duration::from_secs(1))
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_device_is_said_offline() {
    // A session lost after two seconds without a word, not eighty.
    let waits = Waits {
        silence: Duration::from_secs(2),
        ..Waits::default()
    };
    let place = Place::with(waits).await;
    let (laptop, laptop_machine) = (device_key(3), place.machine("laptop"));
    let mut laptop_session = place.add(&laptop, &laptop_machine, "laptop").await;
    let mut laptop_port = port(&mut laptop_session);
    let laptop = laptop_session.asker();
    let _laptop_session = drain(laptop_session);
    let present = Ask::Presence {
        active: true,
        last_input: 0,
    };
    place.pc_session.ask(present.clone()).await.unwrap();
    online(&mut laptop_port, &[place.pc_machine.public()]).await;

    // The laptop keeps talking; the pc says nothing more after its ask, as a device whose
    // connection died says nothing: the hub lets it go, and the laptop is told.
    let started = Instant::now();
    while laptop_port.online().contains(&place.pc_machine.public()) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the silent pc is still said online"
        );
        laptop.ask(present.clone()).await.unwrap();
        let _ =
            tokio::time::timeout(Duration::from_millis(300), laptop_port.online_changed()).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_packet_waits_for_nothing_the_hub_does() {
    let mut place = Place::new().await;
    let (server, server_machine) = (device_key(2), place.machine("server"));
    let mut server_session = place.add(&server, &server_machine, "server").await;
    let mut server_port = port(&mut server_session);
    let _server_session = drain(server_session);
    let pc_port = port(&mut place.pc_session);
    online(&mut server_port, &[place.pc_machine.public()]).await;

    // The hub busy for two seconds, as with a long change of its list: the links do not wait.
    let served = std::sync::Arc::new(place.served);
    let busy = tokio::task::spawn_blocking({
        let served = served.clone();
        move || served.with_hub(|_| std::thread::sleep(Duration::from_secs(2)))
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    for n in 0..5u8 {
        assert!(pc_port.send(&server_machine.public(), vec![n; 100]).await);
    }
    let got = received_within(&mut server_port, Duration::from_secs(1)).await;
    assert_eq!(got.len(), 5, "the links waited for the hub");
    busy.await.unwrap();
}

/// A phone paired with the hub of `place` as the app pairs it: a controller whose endpoint is
/// declared as its machine, saying it is a phone and, with `notices`, that it takes the notices.
/// Its session.
async fn phone(place: &Place, key: &DeviceKey, name: &str, notices: bool) -> Session {
    let endpoint = place.machine(name);
    let code: PairCode = place.served.with_hub(|hub| {
        hub.open_pairing(&url(place.served.address()), None)
            .unwrap()
            .1
    });
    let proposed = Proposed {
        key: key.public(),
        name: name.to_owned(),
        role: Role::Controller,
        icon: Icon::Phone,
        colour: Colour::of(&key.public()),
    };
    let mut paired = Device::pair(&code, proposed, &endpoint, &hello(), &Tls::System)
        .await
        .unwrap();
    let (pinned, _) = paired.listed().await.unwrap();
    let session = declare(pinned, paired, key, &endpoint).await;
    session
        .ask(Ask::About {
            device: "phone".to_owned(),
            system: "android".to_owned(),
            notices,
        })
        .await
        .unwrap();
    session
}

/// The notices `session` is told within `wait`, each with the key it is from.
async fn notices_within(session: &mut Session, wait: Duration) -> Vec<(PublicKey, Notice)> {
    let mut told = Vec::new();
    let until = tokio::time::Instant::now() + wait;
    while let Ok(Some(pushed)) = tokio::time::timeout_at(until, session.next()).await {
        if let Pushed::Notice { from, notice } = pushed {
            told.push((from, notice));
        }
    }
    told
}

fn request_notice() -> Notice {
    Notice {
        kind: NoticeKind::Request,
        notification: 7,
        session: Some("3f2a91bc0d4e".parse().unwrap()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runners_notice_goes_to_the_phones_only() {
    let place = Place::new().await;
    let mut pixel = phone(&place, &device_key(4), "pixel", true).await;
    let (laptop, laptop_machine) = (device_key(3), place.machine("laptop"));
    let mut laptop_session = place.add(&laptop, &laptop_machine, "laptop").await;
    laptop_session
        .ask(Ask::About {
            device: "computer".to_owned(),
            system: "linux".to_owned(),
            notices: false,
        })
        .await
        .unwrap();

    place
        .pc_session
        .ask(Ask::Notify {
            notice: request_notice(),
        })
        .await
        .unwrap();

    let told = notices_within(&mut pixel, Duration::from_secs(2)).await;
    assert_eq!(told, [(place.pc_machine.public(), request_notice())]);
    let told = notices_within(&mut laptop_session, Duration::from_millis(500)).await;
    assert!(told.is_empty(), "a computer is told no notice: {told:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_runners_machine_tells_the_phones() {
    let place = Place::new().await;
    let pixel = phone(&place, &device_key(4), "pixel", true).await;
    let mut other = phone(&place, &device_key(5), "other", true).await;

    let refused = pixel
        .ask(Ask::Notify {
            notice: request_notice(),
        })
        .await
        .unwrap_err();

    assert!(
        refused.to_string().contains("only a runner's machine"),
        "{refused}"
    );
    let told = notices_within(&mut other, Duration::from_millis(500)).await;
    assert!(told.is_empty(), "{told:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_that_did_not_ask_for_notices_is_told_none() {
    let place = Place::new().await;
    // A phone of before #827: its session would end at a frame it does not know.
    let mut older = phone(&place, &device_key(4), "older", false).await;
    let mut newer = phone(&place, &device_key(5), "newer", true).await;

    place
        .pc_session
        .ask(Ask::Notify {
            notice: request_notice(),
        })
        .await
        .unwrap();

    let told = notices_within(&mut newer, Duration::from_secs(2)).await;
    assert_eq!(told.len(), 1, "{told:?}");
    let told = notices_within(&mut older, Duration::from_millis(500)).await;
    assert!(told.is_empty(), "{told:?}");
}
