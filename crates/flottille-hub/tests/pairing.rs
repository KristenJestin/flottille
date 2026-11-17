//! Pairing through the hub (`docs/remote.md`, "Pairing"): a hub served on 127.0.0.1 from a
//! temporary state folder, with a clock the tests move; the root `pc` set up with its machine, and
//! new devices that join with an invitation, their machine keys in temporary folders and their
//! device keys made from fixed seeds. A hub that lies is played by a server that holds the real
//! hub's key and says what the test tells it.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use flottille_hub::{Clock, Hub, Served};
use flottille_link::{
    Action, Ask, Change, ControlError, Device, DeviceKey, Hello, HubKey, Invitation, InvitationAsk,
    Joined, Keyring, KeyringError, MachineKey, MemberList, Pushed, Requested, Role, Session, Tls,
    ToDevice, arrive, verification_words,
};
use zeroize::Zeroizing;

/// The hub's clock, which a test moves; it starts at the system's time.
#[derive(Clone)]
struct Time(Arc<AtomicU64>);

impl Time {
    fn new() -> Time {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Time(Arc::new(AtomicU64::new(now)))
    }

    fn pass(&self, seconds: u64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for Time {
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

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
    Hello::ours("0.4.0")
}

fn url(address: SocketAddr) -> String {
    format!("http://{address}")
}

/// A test's folders, its hub served with its clock, and the root `pc` set up on it.
struct Place {
    dir: tempfile::TempDir,
    time: Time,
    served: Served<Time>,
    pc: DeviceKey,
    pc_device: Device,
    pc_session: Session,
}

impl Place {
    async fn new() -> Place {
        let dir = tempfile::tempdir().unwrap();
        let time = Time::new();
        let hub = Hub::open(&dir.path().join("hub"), time.clone()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let served = flottille_hub::serve(hub, listener, None, hello()).unwrap();
        let code = served.with_hub(|hub| hub.setup_code().unwrap().to_owned());
        let pc = device_key(1);
        let pc_machine = machine_in(dir.path(), "pc");
        let mut list = MemberList::start("pc", &pc).unwrap();
        list.declare(&pc, &pc_machine);
        let (pc_device, pc_session) = Device::setup(
            &url(served.address()),
            &code,
            list.changes().to_vec(),
            &pc_machine,
            &hello(),
            &Tls::System,
        )
        .await
        .unwrap();
        Place {
            dir,
            time,
            served,
            pc,
            pc_device,
            pc_session,
        }
    }

    fn hub_folder(&self) -> PathBuf {
        self.dir.path().join("hub")
    }

    fn machine(&self, name: &str) -> MachineKey {
        machine_in(self.dir.path(), name)
    }

    /// An invitation `inviter` asks for through `session`, as the code it shows decodes.
    async fn invite_by(&self, inviter: &DeviceKey, session: &Session) -> Invitation {
        let hub = self.served.with_hub(|hub| hub.key());
        let ask = InvitationAsk::sign(&hub, self.time.now(), inviter).unwrap();
        let token = session.asker().invite(ask).await.unwrap();
        let code = Invitation {
            hub_url: self.pc_device.url.clone(),
            hub_key: hub,
            token,
            inviter: inviter.public(),
        }
        .code()
        .unwrap();
        Invitation::from_code(&code).unwrap()
    }

    async fn invite(&self) -> Invitation {
        self.invite_by(&self.pc, &self.pc_session).await
    }

    /// The hub looks again at what it holds, as it does when anything changes.
    fn nudge(&self) {
        self.served.with_hub(|_| ());
    }

    /// `pc` signs the addition of `device` as `name` with `role`, after the list it holds.
    fn addition(&self, device: &DeviceKey, name: &str, role: Role) -> Change {
        let head = self.served.with_hub(|hub| hub.list().head());
        Change::sign(
            head,
            Action::Add {
                key: device.public(),
                name: name.to_owned(),
                role,
            },
            &self.pc,
        )
    }

    fn changes(&self) -> usize {
        self.served.with_hub(|hub| hub.list().changes().len())
    }
}

fn machine_in(dir: &Path, name: &str) -> MachineKey {
    MachineKey::load_or_make(&dir.join("machines").join(name)).unwrap()
}

async fn join(invitation: &Invitation, device: &DeviceKey, machine: &MachineKey) -> Joined {
    Device::join(
        invitation,
        &device.public(),
        machine,
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap()
}

/// The next requests the hub sends this session, within a few seconds.
async fn next_requests(session: &mut Session) -> Vec<Requested> {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match session.next().await {
                Some(Pushed::Requests(requests)) => return requests,
                Some(_) => continue,
                None => panic!("the session ended: {}", session.ended()),
            }
        }
    });
    waited.await.expect("requests within 10 s")
}

/// The requests that wait for this session's member, once some do.
async fn requests_of(session: &mut Session) -> Vec<Requested> {
    loop {
        let requests = next_requests(session).await;
        if !requests.is_empty() {
            return requests;
        }
    }
}

/// What a joining device is told of its request, within a few seconds.
async fn outcome(joined: &mut Joined) -> Result<(Device, flottille_link::Members), ControlError> {
    tokio::time::timeout(Duration::from_secs(10), joined.accepted())
        .await
        .expect("an outcome within 10 s")
}

/// `laptop` joins with an invitation of `pc`, which adds it with `role`; the device then
/// declares its machine through the session it joined with.
async fn joined_as(place: &mut Place, role: Role) -> (DeviceKey, MachineKey, Device) {
    let (laptop, machine) = (device_key(3), place.machine("laptop"));
    let invitation = place.invite().await;
    let mut joined = join(&invitation, &laptop, &machine).await;
    requests_of(&mut place.pc_session).await;
    let added = place.addition(&laptop, "laptop", role);
    place
        .pc_device
        .submit(&mut place.pc_session, added)
        .await
        .unwrap();
    let (mut device, _) = outcome(&mut joined).await.unwrap();
    let declared = Change::sign(
        device.list.head(),
        Action::Machine {
            key: laptop.public(),
            machine: machine.public(),
        },
        &laptop,
    )
    .claimed_by(&machine);
    device.submit(&mut joined.session, declared).await.unwrap();
    (laptop, machine, device)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_works_once_and_expires_after_five_minutes() {
    let place = Place::new().await;
    let invitation = place.invite().await;
    let _joined = join(&invitation, &device_key(3), &place.machine("laptop")).await;

    // Once: another device that answers it is refused.
    let again = Device::join(
        &invitation,
        &device_key(4).public(),
        &place.machine("tablet"),
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap_err();
    assert!(
        again.to_string().contains("was answered already"),
        "{again}"
    );

    // Five minutes: an invitation not answered by then is refused.
    let late = place.invite().await;
    place.time.pass(5 * 60);
    let expired = Device::join(
        &late,
        &device_key(4).public(),
        &place.machine("tablet"),
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap_err();
    assert!(
        expired.to_string().contains("unknown or expired"),
        "{expired}"
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_new_device_connects_to_the_hub_not_to_the_inviting_member() {
    let mut place = Place::new().await;
    let invitation = place.invite().await;
    // The code holds the hub, its key, the token and the inviter: no address of the inviter's.
    assert_eq!(invitation.hub_url, url(place.served.address()) + "/");
    assert_eq!(invitation.hub_key, place.served.with_hub(|hub| hub.key()));
    assert_eq!(invitation.inviter, place.pc.public());

    // `pc` listens nowhere: the laptop reaches the hub, which holds its request for `pc`.
    let laptop = device_key(3);
    let _joined = join(&invitation, &laptop, &place.machine("laptop")).await;
    let pending = place.served.with_hub(|hub| hub.pending(&place.pc.public()));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key, laptop.public());
    let requests = requests_of(&mut place.pc_session).await;
    assert_eq!(
        requests,
        vec![Requested {
            token: invitation.token,
            key: laptop.public()
        }]
    );

    // A hub with another key than the invitation's is refused before anything is said.
    let other = Invitation {
        hub_key: device_key(9).public(),
        ..invitation.clone()
    };
    let refused = Device::join(
        &other,
        &device_key(4).public(),
        &place.machine("tablet"),
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(refused, ControlError::AnotherKey { .. }),
        "{refused}"
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn both_sides_show_the_same_four_words() {
    let mut place = Place::new().await;
    let invitation = place.invite().await;
    let laptop = device_key(3);
    let _joined = join(&invitation, &laptop, &place.machine("laptop")).await;

    let shown_by_the_new_device = invitation.words(&laptop.public());
    let request = requests_of(&mut place.pc_session).await[0];
    let shown_by_the_inviter = verification_words(&request.key, &place.pc.public(), &request.token);
    assert_eq!(shown_by_the_new_device, shown_by_the_inviter);
    // A device between them, with a key of its own, shows other words.
    assert_ne!(
        invitation.words(&device_key(4).public()),
        shown_by_the_inviter
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_or_expired_request_adds_nothing() {
    let mut place = Place::new().await;
    let before = place.changes();

    // Refused by the member that invited: the new device is told, and nothing is added.
    let laptop = device_key(3);
    let invitation = place.invite().await;
    let mut joined = join(&invitation, &laptop, &place.machine("laptop")).await;
    let request = requests_of(&mut place.pc_session).await[0];
    place
        .pc_session
        .ask(Ask::Refuse {
            token: request.token,
        })
        .await
        .unwrap();
    let refused = outcome(&mut joined).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("this join request was refused, or its invitation expired"),
        "{refused}"
    );
    let late = place
        .pc_session
        .ask(Ask::Submit {
            change: place.addition(&laptop, "laptop", Role::Runner),
        })
        .await
        .unwrap_err();
    assert!(late.to_string().contains("no join request"), "{late}");
    assert_eq!(place.changes(), before);

    // Expired: the same.
    let tablet = device_key(4);
    let invitation = place.invite().await;
    let mut joined = join(&invitation, &tablet, &place.machine("tablet")).await;
    requests_of(&mut place.pc_session).await;
    place.time.pass(5 * 60);
    place.nudge();
    let expired = outcome(&mut joined).await.unwrap_err();
    assert!(
        expired
            .to_string()
            .contains("this join request was refused, or its invitation expired"),
        "{expired}"
    );
    let late = place
        .pc_session
        .ask(Ask::Submit {
            change: place.addition(&tablet, "tablet", Role::Runner),
        })
        .await
        .unwrap_err();
    assert!(late.to_string().contains("no join request"), "{late}");
    assert_eq!(place.changes(), before);
    // The member that invited is told none waits any more.
    assert!(next_requests(&mut place.pc_session).await.is_empty());
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_role_chosen_at_acceptance_is_in_the_signed_entry() {
    let mut place = Place::new().await;
    let (laptop, machine, device) = joined_as(&mut place, Role::Controller).await;

    let members = device.members().unwrap();
    let entry = members.get(&laptop.public()).unwrap();
    assert_eq!(
        (entry.name.as_str(), entry.role, entry.added_by),
        ("laptop", Role::Controller, place.pc.public())
    );
    assert_eq!(entry.machine, Some(machine.public()));
    // The role is in what `pc` signed: another role breaks the signature.
    let mut changes = device.list.changes().to_vec();
    let at = changes
        .iter()
        .position(
            |change| matches!(&change.action, Action::Add { key, .. } if *key == laptop.public()),
        )
        .unwrap();
    if let Action::Add { role, .. } = &mut changes[at].action {
        *role = Role::Runner;
    }
    let edited = MemberList::from_changes(changes).check(&place.pc.public());
    assert!(
        edited
            .unwrap_err()
            .to_string()
            .contains("does not carry its signer's signature")
    );

    // Joined, its machine declared: it connects as any member's machine.
    let mut session = device
        .connect(&machine, &hello(), &Tls::System)
        .await
        .unwrap();
    let list = loop {
        if let Some(Pushed::List(list)) = session.next().await {
            break list;
        }
    };
    assert_eq!(list, device.list);
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
                let _ = session.ask().await;
            });
        }
    });
    address
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entry_not_signed_by_the_fingerprint_of_the_invitation_is_refused_by_the_new_device() {
    let place = Place::new().await;
    let invitation = place.invite().await;
    let laptop = device_key(3);
    // A list that checks, but whose entry of the laptop another member signed: `server`, whose
    // key a thief may hold, adds it, not `pc`, whom the invitation names.
    let server = device_key(2);
    let mut list = place.pc_device.list.clone();
    list.sign(
        Action::Add {
            key: server.public(),
            name: "server".to_owned(),
            role: Role::Runner,
        },
        &place.pc,
    );
    list.sign(
        Action::Add {
            key: laptop.public(),
            name: "laptop".to_owned(),
            role: Role::Runner,
        },
        &server,
    );
    assert!(list.check(&place.pc.public()).is_ok());
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(list)]).await;

    let mut joined = join(
        &Invitation {
            hub_url: url(lying),
            ..invitation
        },
        &laptop,
        &place.machine("laptop"),
    )
    .await;
    let refused = outcome(&mut joined).await.unwrap_err();
    assert!(
        refused.to_string().contains(&format!(
            "this device's entry is not signed by the member that invited it ({})",
            place.pc.public().fingerprint()
        )),
        "{refused}"
    );
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_off_during_a_join_learns_it_from_the_hub_when_it_connects_again() {
    let mut place = Place::new().await;
    // `server`, a member, whose machine is off while the laptop joins.
    let (server, server_machine) = (device_key(2), place.machine("server"));
    let invitation = place.invite().await;
    let mut joined = join(&invitation, &server, &server_machine).await;
    requests_of(&mut place.pc_session).await;
    let added = place.addition(&server, "server", Role::Runner);
    place
        .pc_device
        .submit(&mut place.pc_session, added)
        .await
        .unwrap();
    let (mut server_device, _) = outcome(&mut joined).await.unwrap();
    let declared = Change::sign(
        server_device.list.head(),
        Action::Machine {
            key: server.public(),
            machine: server_machine.public(),
        },
        &server,
    )
    .claimed_by(&server_machine);
    server_device
        .submit(&mut joined.session, declared)
        .await
        .unwrap();
    drop(joined);

    let (laptop, _, _) = joined_as(&mut place, Role::Viewer).await;

    // On again: the first thing the hub says is the list, which holds the laptop.
    let mut session = server_device
        .connect(&server_machine, &hello(), &Tls::System)
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), session.next())
        .await
        .unwrap();
    let Some(Pushed::List(list)) = first else {
        panic!("the hub said first {first:?}");
    };
    let members = server_device.take(list).unwrap();
    let entry = members.get(&laptop.public()).unwrap();
    assert_eq!((entry.name.as_str(), entry.role), ("laptop", Role::Viewer));
    place.served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_device_is_cut_at_once_and_cannot_join_again() {
    let mut place = Place::new().await;
    let (laptop, machine, device) = joined_as(&mut place, Role::Runner).await;
    let mut session = device
        .connect(&machine, &hello(), &Tls::System)
        .await
        .unwrap();
    let removal = Change::sign(
        place.served.with_hub(|hub| hub.list().head()),
        Action::Remove {
            key: laptop.public(),
        },
        &place.pc,
    );
    place
        .pc_device
        .submit(&mut place.pc_session, removal)
        .await
        .unwrap();

    // Its open session is cut at once.
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        while session.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the removed machine's session goes on");
    assert!(
        session.ended().contains("is not one of this hub's members"),
        "{}",
        session.ended()
    );

    // Its key cannot join again, through a new invitation.
    let invitation = place.invite().await;
    let refused = Device::join(
        &invitation,
        &laptop.public(),
        &place.machine("laptop-again"),
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("was removed from the member list"),
        "{refused}"
    );
    place.served.stop().await;
}
