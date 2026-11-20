//! The links' packets the hub carries between the user's devices (`docs/remote.md`, "The
//! transport", "The hub"): a hub served on 127.0.0.1 from a temporary state folder, and devices
//! whose machines hold control sessions to it, each with the relay port of its session. What a
//! packet holds is the devices' own (QUIC, encrypted end to end): here it is plain bytes, since
//! the hub never looks at them.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use flottille_hub::{Hub, RELAY_BURST, RELAY_RATE, Served, SystemClock};
use flottille_link::{
    Action, Change, Colour, Device, DeviceKey, Hello, Icon, Keyring, KeyringError, MachineKey,
    PairCode, Paired, Proposed, PublicKey, RelayPort, Role, Session, Tls,
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
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::open(&dir.path().join("hub"), SystemClock).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let served = flottille_hub::serve(hub, listener, None, hello()).unwrap();
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
