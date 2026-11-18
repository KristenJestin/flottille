//! The hub on the network and the devices' control sessions to it (`docs/remote.md`, "The hub",
//! "The first member", "Architecture"): a hub served on 127.0.0.1, on a port the system chooses,
//! from a temporary state folder; devices with their machine keys in temporary folders and their
//! device keys made from fixed seeds. A hub that lies is played by a server that holds the real
//! hub's key and says what the test tells it: every device must refuse it by itself.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Action, Change, ControlError, Device, DeviceKey, Hello, HubKey, InvitationAsk, Keyring,
    KeyringError, ListError, MachineKey, MemberList, Pushed, Role, Session, Tls, ToDevice, arrive,
};
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

/// The key the hub of `folder` holds, as a device key would sign with it: what a hub that tries
/// to sign a change itself would sign with.
fn hub_as_signer(folder: &Path) -> DeviceKey {
    let seed: [u8; 32] = std::fs::read(folder.join(flottille_link::HUB_FILE))
        .unwrap()
        .try_into()
        .unwrap();
    DeviceKey::load_or_make(&Seeded(seed), Path::new("/nonexistent")).unwrap()
}

fn hello() -> Hello {
    Hello::ours("0.4.0")
}

/// A test's folders: the hub's state, and each machine's key folder.
struct Place {
    dir: tempfile::TempDir,
}

impl Place {
    fn new() -> Place {
        Place {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn hub_folder(&self) -> PathBuf {
        self.dir.path().join("hub")
    }

    fn machine(&self, name: &str) -> MachineKey {
        MachineKey::load_or_make(&self.dir.path().join("machines").join(name)).unwrap()
    }

    /// The hub of this place, served on `at` (a port the system chooses for `127.0.0.1:0`).
    async fn serve(&self, at: &str) -> Served<SystemClock> {
        let hub = Hub::open(&self.hub_folder(), SystemClock).unwrap();
        let listener = tokio::net::TcpListener::bind(at).await.unwrap();
        flottille_hub::serve(hub, listener, None, hello()).unwrap()
    }
}

fn url(address: SocketAddr) -> String {
    format!("http://{address}")
}

/// The first member's changes: `device`'s own entry, named `name`, then its machine's
/// declaration.
fn first_member(device: &DeviceKey, machine: &MachineKey, name: &str) -> Vec<Change> {
    let mut list = MemberList::start(name, device).unwrap();
    list.declare(device, machine);
    list.changes().to_vec()
}

async fn setup(
    address: SocketAddr,
    code: &str,
    device: &DeviceKey,
    machine: &MachineKey,
    name: &str,
) -> Result<(Device, Session), ControlError> {
    Device::setup(
        &url(address),
        code,
        first_member(device, machine, name),
        machine,
        &hello(),
        &Tls::System,
    )
    .await
}

fn code_of(served: &Served<SystemClock>) -> String {
    served.with_hub(|hub| hub.setup_code().unwrap().to_owned())
}

/// The next list the hub sends this session, within a few seconds.
async fn next_list(session: &mut Session) -> MemberList {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match session.next().await {
                Some(Pushed::List(list)) => return list,
                Some(_) => continue,
                None => panic!("the session ended: {}", session.ended()),
            }
        }
    });
    waited.await.expect("a list within 10 s")
}

/// The first list of `changes` changes the hub sends this session: those before it are the hub
/// catching up with asks the test made.
async fn list_of(session: &mut Session, changes: usize) -> MemberList {
    loop {
        let list = next_list(session).await;
        if list.changes().len() == changes {
            return list;
        }
    }
}

/// `pc`, the root, adds `name` with `role` through an invitation, and the new device declares its
/// machine; both changes go through `pc`'s session, as members' changes. The new device's side of
/// joining is #812's: here the test answers the invitation on the hub.
async fn add_member(
    served: &Served<SystemClock>,
    pc: &DeviceKey,
    session: &Session,
    device: &DeviceKey,
    machine: &MachineKey,
    name: &str,
    role: Role,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    served.with_hub(|hub| {
        let ask = InvitationAsk::sign(&hub.key(), now, pc).unwrap();
        let token = hub.invite(&ask).unwrap();
        hub.join(&token, device.public()).unwrap();
    });
    let head = served.with_hub(|hub| hub.list().head());
    let added = Change::sign(
        head,
        Action::Add {
            key: device.public(),
            name: name.to_owned(),
            role,
        },
        pc,
    );
    session
        .ask(flottille_link::Ask::Submit {
            change: added.clone(),
        })
        .await
        .unwrap();
    let declared = Change::sign(
        added.hash(),
        Action::Machine {
            key: device.public(),
            machine: machine.public(),
        },
        device,
    )
    .claimed_by(machine);
    session
        .ask(flottille_link::Ask::Submit { change: declared })
        .await
        .unwrap();
}

/// A hub that lies: it holds the real hub's key (so a device's pin holds) and, to every device
/// that connects, welcomes it and says `frames`.
async fn lying_hub(folder: &Path, frames: Vec<ToDevice>) -> SocketAddr {
    let key = HubKey::load_or_make(folder).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let key = std::sync::Arc::new(key);
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

/// `device`, whose pinned hub now answers at `address`, sent a list there: what it says of it.
async fn told_by(device: &mut Device, address: SocketAddr, machine: &MachineKey) -> ListError {
    let kept = device.list.clone();
    let lied_to = Device {
        url: url(address),
        ..device.clone()
    };
    let mut session = lied_to
        .connect(machine, &hello(), &Tls::System)
        .await
        .unwrap();
    let list = next_list(&mut session).await;
    let refused = device.take(list).unwrap_err();
    assert_eq!(device.list, kept, "a list refused leaves the one kept");
    refused
}

/// The root `pc` and a runner `server` it added, each with its device pinned to the hub; the
/// hub's list as they hold it.
struct Two {
    pc: DeviceKey,
    pc_machine: MachineKey,
    pc_device: Device,
    pc_session: Session,
    server_machine: MachineKey,
    server_device: Device,
}

async fn two(place: &Place, served: &Served<SystemClock>) -> Two {
    let (pc, server) = (device_key(1), device_key(2));
    let (pc_machine, server_machine) = (place.machine("pc"), place.machine("server"));
    let (mut pc_device, mut pc_session) =
        setup(served.address(), &code_of(served), &pc, &pc_machine, "pc")
            .await
            .unwrap();
    add_member(
        served,
        &pc,
        &pc_session,
        &server,
        &server_machine,
        "server",
        Role::Runner,
    )
    .await;
    pc_device.take(list_of(&mut pc_session, 4).await).unwrap();
    let mut server_device = Device {
        url: pc_device.url.clone(),
        hub: pc_device.hub,
        root: pc.public(),
        list: MemberList::default(),
    };
    let mut server_session = server_device
        .connect(&server_machine, &hello(), &Tls::System)
        .await
        .unwrap();
    let members = server_device
        .take(next_list(&mut server_session).await)
        .unwrap();
    assert_eq!(members.members().len(), 2);
    Two {
        pc,
        pc_machine,
        pc_device,
        pc_session,
        server_machine,
        server_device,
    }
}

// --------------------------------------------------------------------------------------------
// The binary.

/// The hub binary, serving from `state` on a port the system chooses; its log read line by line.
struct Running {
    child: Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Running {
    fn start(home: &Path, state: &Path) -> Running {
        Running::start_with(home, state, None)
    }

    /// As `start`, the hub let open at most `files` file descriptors when it is given.
    fn start_with(home: &Path, state: &Path, files: Option<u64>) -> Running {
        let mut command = Command::new(env!("CARGO_BIN_EXE_flottille-hub"));
        command
            .args([
                "--state",
                state.to_str().unwrap(),
                "--listen",
                "127.0.0.1:0",
            ])
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .stderr(Stdio::piped());
        if let Some(files) = files {
            use std::os::unix::process::CommandExt;
            // SAFETY: setrlimit is async-signal-safe, and touches nothing of the parent's.
            unsafe {
                command.pre_exec(move || {
                    let limit = libc::rlimit {
                        rlim_cur: files,
                        rlim_max: files,
                    };
                    if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (send, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { return };
                if send.send(line).is_err() {
                    return;
                }
            }
        });
        Running { child, lines }
    }

    /// The log's lines up to the one that says where the hub listens, and that address.
    fn until_listening(&self) -> (Vec<String>, SocketAddr) {
        let mut seen = Vec::new();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("the hub said no address: {seen:?}"));
            if let Some(rest) = line.strip_prefix("flottille-hub: listening on ") {
                let address = rest.split(' ').next().unwrap().parse().unwrap();
                seen.push(line);
                return (seen, address);
            }
            seen.push(line);
        }
    }

    fn stop(mut self) {
        // SAFETY: kill has no precondition; the child is ours and not reaped yet.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let status = self.child.wait().unwrap();
        assert!(status.success(), "a hub stopped by SIGTERM exits cleanly");
    }
}

/// A test that fails before it stops the hub kills it: an orphan would hold the test's output.
impl Drop for Running {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_setup_code_is_printed_once_and_makes_the_first_member() {
    let place = Place::new();
    let state = place.hub_folder();
    let running = Running::start(place.dir.path(), &state);
    let (log, address) = running.until_listening();
    let codes: Vec<&String> = log
        .iter()
        .filter(|line| line.contains("setup code: "))
        .collect();
    assert_eq!(codes.len(), 1, "{log:#?}");
    let code = codes[0]
        .split("setup code: ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(code.len(), 19, "{code}");

    let (pc, machine) = (device_key(1), place.machine("pc"));
    // The code read in lower case and without its hyphens, as a person may type it.
    let typed = code.replace('-', "").to_lowercase();
    let (device, session) = setup(address, &typed, &pc, &machine, "pc").await.unwrap();
    let members = device.members().unwrap();
    let root = members.get(&pc.public()).unwrap();
    assert_eq!((root.name.as_str(), root.role), ("pc", Role::Runner));
    assert_eq!(
        root.added_by,
        pc.public(),
        "the root's entry is signed by itself"
    );
    assert_eq!(root.machine, Some(machine.public()));
    assert_eq!(members.root(), pc.public());
    drop(session);
    running.stop();

    // Started again: no code in the log, and none on demand; the member connects.
    let running = Running::start(place.dir.path(), &state);
    let (log, address) = running.until_listening();
    assert!(
        !log.iter().any(|line| line.contains("setup code")),
        "{log:#?}"
    );
    let again = Command::new(env!("CARGO_BIN_EXE_flottille-hub"))
        .args(["--state", state.to_str().unwrap(), "setup-code"])
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("has a member already"));
    let moved = Device {
        url: url(address),
        ..device.clone()
    };
    let mut session = moved
        .connect(&machine, &hello(), &Tls::System)
        .await
        .unwrap();
    assert_eq!(next_list(&mut session).await, device.list);
    drop(session);
    running.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_setup_code_works_only_while_no_member_exists() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let code = code_of(&served);

    // A wrong code is refused, and leaves the code working.
    let (pc, laptop) = (device_key(1), device_key(2));
    let wrong = setup(
        served.address(),
        "0000-0000-0000-0000",
        &pc,
        &place.machine("pc"),
        "pc",
    )
    .await
    .unwrap_err();
    assert!(
        wrong
            .to_string()
            .contains("this setup code is not the hub's"),
        "{wrong}"
    );

    // A setup whose machine is not the one that presents the code is refused.
    let other = first_member(&pc, &place.machine("elsewhere"), "pc");
    let refused = Device::setup(
        &url(served.address()),
        &code,
        other,
        &place.machine("pc"),
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap_err();
    assert!(
        refused.to_string().contains("must declare the machine"),
        "{refused}"
    );

    let (_device, _session) = setup(served.address(), &code, &pc, &place.machine("pc"), "pc")
        .await
        .unwrap();
    // The same code again, by another device: refused, before its session goes any further.
    let again = setup(
        served.address(),
        &code,
        &laptop,
        &place.machine("laptop"),
        "laptop",
    )
    .await
    .unwrap_err();
    assert!(
        again
            .to_string()
            .contains("is not one of this hub's members"),
        "{again}"
    );
    // Gone from the hub's state.
    served.with_hub(|hub| assert!(hub.setup_code().is_err()));
    assert!(flottille_hub::setup_code(&place.hub_folder()).is_err());
    let members = served.with_hub(|hub| hub.members().unwrap().members().len());
    assert_eq!(members, 1);
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_on_the_hub_is_refused_by_every_device_that_pinned_the_old_root() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let address = served.address();
    let mut two = two(&place, &served).await;
    drop(two.pc_session);
    served.stop().await;

    // On the hub's machine: the list emptied, a new code.
    let code = flottille_hub::reset(&place.hub_folder()).unwrap();
    let served = place.serve(&address.to_string()).await;

    // Every device that connects is told the list it pinned was reset, and keeps its own.
    for (device, machine) in [
        (&mut two.pc_device, &two.pc_machine),
        (&mut two.server_device, &two.server_machine),
    ] {
        let kept = device.list.clone();
        let refused = device
            .connect(machine, &hello(), &Tls::System)
            .await
            .unwrap_err();
        assert!(matches!(refused, ControlError::Reset), "{refused}");
        assert_eq!(
            refused.to_string(),
            "the hub's member list was reset: set this device up again"
        );
        assert_eq!(device.list, kept);
    }

    // Set up again by someone else: the old devices are refused at once, saying it was reset.
    let (_new, _session) = setup(
        address,
        &code,
        &device_key(9),
        &place.machine("stranger"),
        "stranger",
    )
    .await
    .unwrap();
    for (device, machine) in [
        (&two.pc_device, &two.pc_machine),
        (&two.server_device, &two.server_machine),
    ] {
        let refused = device
            .connect(machine, &hello(), &Tls::System)
            .await
            .unwrap_err();
        assert!(matches!(refused, ControlError::Reset), "{refused}");
        assert_eq!(
            refused.to_string(),
            "the hub's member list was reset: set this device up again"
        );
    }
    let _ = two.pc;
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_change_no_member_signed_is_refused_by_the_hub_and_by_every_device() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut two = two(&place, &served).await;
    let stranger = device_key(7);
    let head = two.pc_device.list.head();
    let forged = Change::sign(
        head,
        Action::Add {
            key: stranger.public(),
            name: "stranger".to_owned(),
            role: Role::Runner,
        },
        &stranger,
    );

    // The hub refuses it, from a member's session.
    let refused = two
        .pc_session
        .ask(flottille_link::Ask::Submit {
            change: forged.clone(),
        })
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("refuses this change"),
        "{refused}"
    );
    let renamed = Change::sign(
        head,
        Action::Rename {
            key: two.pc.public(),
            name: "mine".to_owned(),
        },
        &stranger,
    );
    let refused = two
        .pc_session
        .ask(flottille_link::Ask::Submit { change: renamed })
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("is signed by a key that is not a member"),
        "{refused}"
    );

    // A hub that sends it anyway is refused by every device.
    let mut lie = two.pc_device.list.changes().to_vec();
    lie.push(forged);
    let lying = lying_hub(
        &place.hub_folder(),
        vec![ToDevice::List(MemberList::from_changes(lie))],
    )
    .await;
    for (device, machine) in [
        (&mut two.pc_device, &two.pc_machine),
        (&mut two.server_device, &two.server_machine),
    ] {
        let refused = told_by(device, lying, machine).await;
        assert!(
            refused
                .to_string()
                .contains("change 5 is signed by a key that is not a member"),
            "{refused}"
        );
    }

    // A change signed by a member that was removed before it.
    let pc = &two.pc;
    let server = device_key(2);
    let mut removed = two.pc_device.list.clone();
    removed.sign(
        Action::Remove {
            key: server.public(),
        },
        pc,
    );
    removed.sign(
        Action::Add {
            key: stranger.public(),
            name: "stranger".to_owned(),
            role: Role::Runner,
        },
        &server,
    );
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(removed)]).await;
    let refused = told_by(&mut two.pc_device, lying, &two.pc_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 6 is signed by a key that is not a member"),
        "{refused}"
    );

    // An older list played again: it lacks changes the device checked.
    let old = MemberList::from_changes(two.pc_device.list.changes()[..2].to_vec());
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(old)]).await;
    let refused = told_by(&mut two.server_device, lying, &two.server_machine).await;
    assert!(
        refused
            .to_string()
            .contains("it lacks changes this device has already checked"),
        "{refused}"
    );
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hub_cannot_add_a_member_or_change_a_role() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut two = two(&place, &served).await;
    let hub_key = hub_as_signer(&place.hub_folder());
    let intruder = device_key(8);
    let head = two.pc_device.list.head();

    // The hub signing an addition itself: refused by the hub's own rules...
    let by_hub = Change::sign(
        head,
        Action::Add {
            key: intruder.public(),
            name: "intruder".to_owned(),
            role: Role::Runner,
        },
        &hub_key,
    );
    let refused = two
        .pc_session
        .ask(flottille_link::Ask::Submit {
            change: by_hub.clone(),
        })
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("refuses this change"),
        "{refused}"
    );
    // ...and an addition signed by a member but answering no join request is refused too.
    let unasked = Change::sign(
        head,
        Action::Add {
            key: intruder.public(),
            name: "intruder".to_owned(),
            role: Role::Runner,
        },
        &two.pc,
    );
    let refused = two
        .pc_session
        .ask(flottille_link::Ask::Submit { change: unasked })
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("no join request"), "{refused}");

    // A hub that sends its own addition anyway: refused by every device.
    let mut lie = two.pc_device.list.changes().to_vec();
    lie.push(by_hub);
    let lying = lying_hub(
        &place.hub_folder(),
        vec![ToDevice::List(MemberList::from_changes(lie))],
    )
    .await;
    for (device, machine) in [
        (&mut two.pc_device, &two.pc_machine),
        (&mut two.server_device, &two.server_machine),
    ] {
        let refused = told_by(device, lying, machine).await;
        assert!(
            refused
                .to_string()
                .contains("change 5 is signed by a key that is not a member"),
            "{refused}"
        );
    }

    // A hub that changes a role in a signed entry: the signature no longer holds.
    let mut json = serde_json::to_value(&two.pc_device.list).unwrap();
    assert_eq!(json[2]["action"]["role"], "runner");
    json[2]["action"]["role"] = "viewer".into();
    let changed: MemberList = serde_json::from_value(json).unwrap();
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(changed)]).await;
    let refused = told_by(&mut two.server_device, lying, &two.server_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 3 does not carry its signer's signature"),
        "{refused}"
    );

    // A hub that gives a member another machine: only the member itself declares its own.
    let mut lie = two.pc_device.list.clone();
    lie.sign(
        Action::Machine {
            key: two.pc.public(),
            machine: place.machine("intruder").public(),
        },
        &hub_key,
    );
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(lie)]).await;
    let refused = told_by(&mut two.server_device, lying, &two.server_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 5 is signed by a key that is not a member"),
        "{refused}"
    );
    // Nothing of it was kept: the members and roles are those the members signed.
    let members = two.server_device.members().unwrap();
    let roles: Vec<(String, Role)> = members
        .members()
        .iter()
        .map(|member| (member.name.clone(), member.role))
        .collect();
    assert_eq!(
        roles,
        [
            ("pc".to_owned(), Role::Runner),
            ("server".to_owned(), Role::Runner)
        ]
    );
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hub_with_another_key_is_refused() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let address = served.address();
    let two = two(&place, &served).await;
    drop(two.pc_session);
    served.stop().await;

    // Another hub, with a state and a key of its own, at the very address.
    let other = Place::new();
    let impostor = other.serve(&address.to_string()).await;
    for (device, machine) in [
        (&two.pc_device, &two.pc_machine),
        (&two.server_device, &two.server_machine),
    ] {
        let refused = device
            .connect(machine, &hello(), &Tls::System)
            .await
            .unwrap_err();
        assert!(
            matches!(refused, ControlError::AnotherKey { .. }),
            "{refused}"
        );
        assert_eq!(
            refused.to_string(),
            format!("the hub at {address} has another key than when this device joined")
        );
    }
    impostor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_removed_is_refused_its_session() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let two = two(&place, &served).await;
    let server = device_key(2);
    let removal = Change::sign(
        two.pc_device.list.head(),
        Action::Remove {
            key: server.public(),
        },
        &two.pc,
    );
    two.pc_session
        .ask(flottille_link::Ask::Submit { change: removal })
        .await
        .unwrap();
    let refused = two
        .server_device
        .connect(&two.server_machine, &hello(), &Tls::System)
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("is not one of this hub's members"),
        "{refused}"
    );
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hub_tells_every_member_the_addresses_their_machines_told() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut two = two(&place, &served).await;
    let server_session = two
        .server_device
        .connect(&two.server_machine, &hello(), &Tls::System)
        .await
        .unwrap();
    server_session
        .ask(flottille_link::Ask::Addresses {
            addresses: vec!["192.0.2.10:7447".to_owned()],
        })
        .await
        .unwrap();
    // Not an address: refused, the ones told kept.
    let refused = server_session
        .ask(flottille_link::Ask::Addresses {
            addresses: vec!["not an address".to_owned()],
        })
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("not addresses"), "{refused}");
    let told = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(Pushed::Addresses(told)) = two.pc_session.next().await
                && !told.is_empty()
            {
                return told;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].key, two.server_machine.public());
    assert_eq!(told[0].addresses, ["192.0.2.10:7447"]);
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_is_not_a_device_is_answered_as_http_and_closed() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut stream = tokio::net::TcpStream::connect(served.address())
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: hub\r\n\r\n")
        .await
        .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 426"), "{answer}");
    assert!(answer.contains("this is a flottille-hub"), "{answer}");
    served.stop().await;
}

#[test]
fn acme_is_refused_saying_what_to_do_instead() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_flottille-hub"))
        .args([
            "--state",
            dir.path().join("hub").to_str().unwrap(),
            "--acme",
            "hub.example.net",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("ACME is not built yet"), "{said}");
    assert!(said.contains("--tls-cert"), "{said}");
}

// --------------------------------------------------------------------------------------------
// What a stranger, someone on the path, or a hub that lies may try.

/// A hub that holds `key`, welcomes every device, says `frames`, then answers every ask as done;
/// a setup with its proof that it holds `code`, when it is given.
async fn answering_hub(key: HubKey, code: Option<&str>, frames: Vec<ToDevice>) -> SocketAddr {
    let code = code.map(|code| flottille_link::SetupCode::parse(code).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let key = std::sync::Arc::new(key);
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (key, frames, code) = (key.clone(), frames.clone(), code.clone());
            tokio::spawn(async move {
                let arriving = arrive(Box::new(stream), &key, &hello()).await.unwrap();
                let mut session = arriving.welcome().await.unwrap();
                for frame in &frames {
                    session.send(frame).await.unwrap();
                }
                while let Ok(asked) = session.ask().await {
                    let proof = match (&asked.ask, &code) {
                        (flottille_link::Ask::Setup { changes, .. }, Some(code)) => {
                            let head = MemberList::from_changes(changes.clone()).head();
                            Some(session.transcript().setup_proof(code, 1, &head))
                        }
                        _ => None,
                    };
                    let answer = ToDevice::Answer {
                        id: asked.id,
                        refused: None,
                        proof,
                        token: None,
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

/// A relay on the path between devices and the hub at `hub`, as a router of a plain network is:
/// it passes every line on, what the device says rewritten by `rewrite`; every byte the devices
/// sent is kept in the second value.
async fn relay(
    hub: SocketAddr,
    rewrite: std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>,
) -> (SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((device, _)) = listener.accept().await else {
                return;
            };
            let upstream = tokio::net::TcpStream::connect(hub).await.unwrap();
            let (device_read, mut device_write) = device.into_split();
            let (hub_read, mut hub_write) = upstream.into_split();
            let (rewrite, kept) = (rewrite.clone(), kept.clone());
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(device_read);
                let mut line = String::new();
                while lines.read_line(&mut line).await.is_ok_and(|read| read > 0) {
                    kept.lock().unwrap().extend_from_slice(line.as_bytes());
                    let passed = rewrite(&line);
                    if hub_write.write_all(passed.as_bytes()).await.is_err() {
                        return;
                    }
                    line.clear();
                }
            });
            tokio::spawn(async move {
                let mut hub_read = hub_read;
                let _ = tokio::io::copy(&mut hub_read, &mut device_write).await;
            });
        }
    });
    (address, seen)
}

/// A device that comes to a hub whose list is empty: pinned to the hub's key, with its own key as
/// a root it never set up.
fn stranger_at(served: &Served<SystemClock>, stranger: &DeviceKey) -> Device {
    Device {
        url: url(served.address()),
        hub: served.with_hub(|hub| hub.key()),
        root: stranger.public(),
        list: MemberList::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_set_up_again_after_a_reset_does_not_bring_back_a_removed_member() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let address = served.address();
    let two = two(&place, &served).await;
    // The server is lost: pc removes it.
    let removal = Change::sign(
        two.pc_device.list.head(),
        Action::Remove {
            key: device_key(2).public(),
        },
        &two.pc,
    );
    two.pc_session
        .ask(flottille_link::Ask::Submit { change: removal })
        .await
        .unwrap();
    let old = served.with_hub(|hub| hub.list().clone());
    assert_eq!(old.changes().len(), 5);
    drop(two.pc_session);
    served.stop().await;

    // The hub reset, and set up again by the very device, machine and name.
    let code = flottille_hub::reset(&place.hub_folder()).unwrap();
    let served = place.serve(&address.to_string()).await;
    let (mut again, session) = setup(address, &code, &two.pc, &two.pc_machine, "pc")
        .await
        .unwrap();
    let fresh = again.list.clone();
    assert_eq!(fresh.changes().len(), 2);

    // The old list's changes do not follow on the new one: the hub refuses them, even for a join
    // request that would take them...
    served.with_hub(|hub| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ask = InvitationAsk::sign(&hub.key(), now, &two.pc).unwrap();
        let token = hub.invite(&ask).unwrap();
        hub.join(&token, device_key(2).public()).unwrap();
    });
    let refused = session
        .ask(flottille_link::Ask::Submit {
            change: old.changes()[2].clone(),
        })
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("does not follow the change before it"),
        "{refused}"
    );
    // ...and the device set up again refuses a hub that serves them anyway.
    let mut lie = fresh.changes().to_vec();
    lie.extend_from_slice(&old.changes()[2..4]);
    let lying = lying_hub(
        &place.hub_folder(),
        vec![ToDevice::List(MemberList::from_changes(lie))],
    )
    .await;
    let refused = told_by(&mut again, lying, &two.pc_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 3 does not follow the change before it"),
        "{refused}"
    );

    // The devices that pinned the old list are told it was reset: the one whose machine is in the
    // new list when it is sent it, the other when it is refused.
    let mut pc_before = two.pc_device.clone();
    let mut old_session = pc_before
        .connect(&two.pc_machine, &hello(), &Tls::System)
        .await
        .unwrap();
    let refused = pc_before
        .take(next_list(&mut old_session).await)
        .unwrap_err();
    assert!(matches!(refused, ListError::Reset), "{refused}");
    let refused = two
        .server_device
        .connect(&two.server_machine, &hello(), &Tls::System)
        .await
        .unwrap_err();
    assert!(matches!(refused, ControlError::Reset), "{refused}");
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_carried_by_a_session_that_is_not_a_member_is_refused() {
    let place = Place::new();
    let served = std::sync::Arc::new(place.serve("127.0.0.1:0").await);
    let code = code_of(&served);
    // A session opened while the list is empty, by someone who is not setting the hub up.
    let stranger = device_key(7);
    let session = stranger_at(&served, &stranger)
        .connect(&place.machine("stranger"), &hello(), &Tls::System)
        .await
        .unwrap();

    // pc sets the hub up while the stranger's ask is on its way: the stranger's ask comes in, then
    // waits for the hub, which holds pc's setup; the session is not refused yet when it does.
    let (pc, pc_machine) = (device_key(1), place.machine("pc"));
    let changes = first_member(&pc, &pc_machine, "pc");
    let renamed = Change::sign(
        changes[1].hash(),
        Action::Rename {
            key: pc.public(),
            name: "desk".to_owned(),
        },
        &pc,
    );
    let holding = served.clone();
    let setting_up = tokio::task::spawn_blocking(move || {
        holding.with_hub(|hub| {
            hub.setup(&code, changes).unwrap();
            std::thread::sleep(Duration::from_millis(1500));
        })
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let refused = session
        .ask(flottille_link::Ask::Submit { change: renamed })
        .await
        .unwrap_err();
    setting_up.await.unwrap();
    assert!(
        refused
            .to_string()
            .contains("is not one of this hub's members"),
        "{refused}"
    );
    served.with_hub(|hub| assert_eq!(hub.list().changes().len(), 2));
    std::sync::Arc::into_inner(served).unwrap().stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handshake_line_longer_than_a_few_kib_ends_the_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut stream = tokio::net::TcpStream::connect(served.address())
        .await
        .unwrap();
    stream
        .write_all(
            b"GET / HTTP/1.1\r\nHost: hub\r\nConnection: Upgrade\r\nUpgrade: flottille-hub\r\n\r\n",
        )
        .await
        .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 101"));
    // A device's hello is some hundred bytes: 16 KiB of one, with no end of line yet.
    let _ = stream.write_all(&vec![b' '; 16 << 10]).await;
    let mut rest = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut rest)).await;
    assert!(
        ended.is_ok(),
        "the hub still reads a handshake line of 16 KiB"
    );
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_that_have_not_proved_a_member_are_capped() {
    use tokio::io::AsyncReadExt;
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut idle = Vec::new();
    for _ in 0..flottille_hub::MOST_ARRIVING + flottille_hub::KEPT_FOR_MEMBERS {
        idle.push(
            tokio::net::TcpStream::connect(served.address())
                .await
                .unwrap(),
        );
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    // One more is let go at once.
    let mut over = tokio::net::TcpStream::connect(served.address())
        .await
        .unwrap();
    let mut byte = [0u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(3), over.read(&mut byte)).await;
    assert!(
        matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
        "a connection past the places that wait is still held: {closed:?}"
    );
    // Once they are gone, a device is served again.
    drop(idle);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_device, _session) = setup(
        served.address(),
        &code_of(&served),
        &device_key(1),
        &place.machine("pc"),
        "pc",
    )
    .await
    .unwrap();
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_opened_before_setup_that_sets_nothing_up_is_closed() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut session = stranger_at(&served, &device_key(7))
        .connect(&place.machine("stranger"), &hello(), &Tls::System)
        .await
        .unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(25), async {
        while session.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the session is still open after 25 s");
    assert!(session.ended().contains("no setup"), "{}", session.ended());
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_keeps_asking_before_setup_is_still_closed() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut session = stranger_at(&served, &device_key(7))
        .connect(&place.machine("stranger"), &hello(), &Tls::System)
        .await
        .unwrap();
    // It asks every 2 s, within the 10 s a session that asks nothing is given: still closed.
    let asker = session.asker();
    let asking = tokio::spawn(async move {
        loop {
            let _ = asker
                .ask(flottille_link::Ask::Addresses {
                    addresses: Vec::new(),
                })
                .await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    let ended = tokio::time::timeout(Duration::from_secs(15), async {
        while session.next().await.is_some() {}
    })
    .await;
    asking.abort();
    assert!(
        ended.is_ok(),
        "the session that keeps asking is still open after 15 s"
    );
    assert!(session.ended().contains("no setup"), "{}", session.ended());
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn members_still_connect_while_strangers_hold_the_unauthenticated_places() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let (pc, pc_machine) = (device_key(1), place.machine("pc"));
    let (pc_device, pc_session) =
        setup(served.address(), &code_of(&served), &pc, &pc_machine, "pc")
            .await
            .unwrap();
    drop(pc_session);
    // Strangers open connections and say nothing: every place for those that have not proved a
    // member's key is held.
    let mut idle = Vec::new();
    for _ in 0..flottille_hub::MOST_ARRIVING {
        idle.push(
            tokio::net::TcpStream::connect(served.address())
                .await
                .unwrap(),
        );
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut session = pc_device
        .connect(&pc_machine, &hello(), &Tls::System)
        .await
        .expect("a member connects while strangers hold the places");
    let list = next_list(&mut session).await;
    assert_eq!(list.changes().len(), 2);
    drop(idle);
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_says_nothing_is_let_go_within_a_few_seconds() {
    use tokio::io::AsyncReadExt;
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut stream = tokio::net::TcpStream::connect(served.address())
        .await
        .unwrap();
    let mut byte = [0u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(8), stream.read(&mut byte)).await;
    assert!(
        matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
        "a connection that says nothing is still held after 8 s: {closed:?}"
    );
    served.stop().await;
}

/// The time the process `pid` used on the processors, in clock ticks.
fn ticks(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    // utime and stime: the 14th and 15th fields, the 12th and 13th after the name.
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

#[test]
fn a_hub_out_of_file_descriptors_waits_before_it_accepts_again() {
    let place = Place::new();
    let running = Running::start_with(place.dir.path(), &place.hub_folder(), Some(24));
    let (_, address) = running.until_listening();
    // More connections than the hub may open files: those it cannot accept wait in the queue.
    let _waiting: Vec<std::net::TcpStream> = (0..64)
        .map(|_| std::net::TcpStream::connect(address).unwrap())
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    let before = ticks(running.child.id());
    std::thread::sleep(Duration::from_secs(2));
    let used = ticks(running.child.id()) - before;
    // SAFETY: sysconf has no precondition.
    let per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    assert!(
        used < per_second / 2,
        "the hub used {used} ticks of 2 s ({per_second} a second): it loops on a failing accept"
    );
    running.stop();
}

#[test]
fn the_hub_listens_on_loopback_unless_told_otherwise() {
    assert_eq!(flottille_hub::LISTEN, "127.0.0.1:7448");
    let place = Place::new();
    let state = place.hub_folder();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flottille-hub"))
            .args(["--state", state.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    };
    // Another address, plain: refused before anything listens, saying what to give.
    let output = run(&["--listen", "192.0.2.1:7448"]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("--tls-cert"), "{said}");
    assert!(said.contains("--plain"), "{said}");
    // Asked plain, it is taken: the address is no host's here, so it fails only to listen.
    let output = run(&["--plain", "--listen", "192.0.2.1:7448"]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("cannot listen on 192.0.2.1:7448"), "{said}");

    // Plain on loopback, it says the traffic is in clear.
    let running = Running::start(place.dir.path(), &state);
    let (log, _) = running.until_listening();
    let listening = log.last().unwrap();
    assert!(listening.contains("the traffic is in clear"), "{listening}");
    running.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_setup_code_never_crosses_the_network() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let code = code_of(&served);
    let (relayed, seen) = relay(served.address(), std::sync::Arc::new(str::to_owned)).await;
    let (_device, _session) = setup(relayed, &code, &device_key(1), &place.machine("pc"), "pc")
        .await
        .unwrap();
    let seen = String::from_utf8_lossy(&seen.lock().unwrap()).to_uppercase();
    assert!(!seen.is_empty());
    for form in [code.clone(), code.replace('-', "")] {
        assert!(
            !seen.contains(&form),
            "the setup code went over the network"
        );
    }
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setup_whose_changes_are_changed_on_the_way_is_refused() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let code = code_of(&served);
    let (pc, machine) = (device_key(1), place.machine("pc"));
    // Someone on the path puts its own root in, declaring the machine that set up.
    let attacker = device_key(6);
    let theirs = first_member(&attacker, &machine, "mine");
    let theirs = serde_json::to_value(&theirs).unwrap();
    let rewrite = std::sync::Arc::new(move |line: &str| {
        let Ok(mut said) = serde_json::from_str::<serde_json::Value>(line) else {
            return line.to_owned();
        };
        if said["ask"]["kind"] != "setup" {
            return line.to_owned();
        }
        said["ask"]["changes"] = theirs.clone();
        format!("{said}\n")
    });
    let (relayed, _) = relay(served.address(), rewrite).await;
    let refused = setup(relayed, &code, &pc, &machine, "pc")
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("not the hub's"), "{refused}");
    served.with_hub(|hub| assert!(hub.members().is_none()));
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_trusts_a_hub_only_once_it_proved_the_setup_code() {
    let place = Place::new();
    let other = Place::new();
    // Not the user's hub: another key, and no code of the user's, yet it says yes to everything.
    let impostor = answering_hub(
        HubKey::load_or_make(&other.hub_folder()).unwrap(),
        None,
        Vec::new(),
    )
    .await;
    let refused = setup(
        impostor,
        "7KQ2-M4XP-9TRW-H3CD",
        &device_key(1),
        &place.machine("pc"),
        "pc",
    )
    .await
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("did not prove it knows the setup code"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hub_that_withholds_a_setup_is_refused_by_the_device() {
    let place = Place::new();
    // A hub that holds the code, as a hub broken into does: it says yes, then sends a list that
    // is not the one the device set up.
    let other_root = first_member(&device_key(6), &place.machine("elsewhere"), "elsewhere");
    let withholding = answering_hub(
        HubKey::load_or_make(&place.hub_folder()).unwrap(),
        Some("7KQ2-M4XP-9TRW-H3CD"),
        vec![ToDevice::List(MemberList::from_changes(other_root))],
    )
    .await;
    let refused = setup(
        withholding,
        "7KQ2-M4XP-9TRW-H3CD",
        &device_key(1),
        &place.machine("pc"),
        "pc",
    )
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("withheld"), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_learns_nothing_of_the_list_from_its_refusal() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let pc = device_key(1);
    let (_device, _session) = setup(
        served.address(),
        &code_of(&served),
        &pc,
        &place.machine("pc"),
        "pc",
    )
    .await
    .unwrap();
    let refused = stranger_at(&served, &device_key(7))
        .connect(&place.machine("stranger"), &hello(), &Tls::System)
        .await
        .unwrap_err();
    assert!(matches!(refused, ControlError::Refused { .. }), "{refused}");
    assert!(
        !refused.to_string().contains(&pc.public().fingerprint()),
        "{refused}"
    );
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn text_from_the_network_is_shown_without_control_characters() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = tokio::io::BufReader::new(stream);
        let mut line = String::new();
        while stream.read_line(&mut line).await.unwrap() > 0 && line != "\r\n" {
            line.clear();
        }
        stream
            .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: flottille-hub\r\n\r\n")
            .await
            .unwrap();
        line.clear();
        stream.read_line(&mut line).await.unwrap();
        stream
            .write_all(b"{\"refused\": \"no\\u001b[2J\\u0007 way\"}\n")
            .await
            .unwrap();
    });
    let place = Place::new();
    let refused = stranger_at_url(address, &device_key(7))
        .connect(&place.machine("pc"), &hello(), &Tls::System)
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("no"), "{refused:?}");
    assert!(!refused.chars().any(char::is_control), "{refused:?}");
}

/// A device pinned to some hub at `address`, with its own key as a root.
fn stranger_at_url(address: SocketAddr, stranger: &DeviceKey) -> Device {
    Device {
        url: url(address),
        hub: device_key(5).public(),
        root: stranger.public(),
        list: MemberList::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_the_hub_withholds_is_said_to_its_submitter() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut two = two(&place, &served).await;
    let known = two.pc_device.list.clone();
    // The real hub sends back the change: kept.
    let renamed = Change::sign(
        known.head(),
        Action::Rename {
            key: two.pc.public(),
            name: "desk".to_owned(),
        },
        &two.pc,
    );
    let members = two
        .pc_device
        .submit(&mut two.pc_session, renamed)
        .await
        .unwrap();
    assert!(
        members
            .get(&two.pc.public())
            .is_some_and(|pc| pc.name == "desk")
    );

    // A hub that says yes and sends back the list without it: said, the list kept as it was.
    let withholding = answering_hub(
        HubKey::load_or_make(&place.hub_folder()).unwrap(),
        None,
        vec![ToDevice::List(two.pc_device.list.clone())],
    )
    .await;
    let mut lied_to = Device {
        url: url(withholding),
        ..two.pc_device.clone()
    };
    let mut session = lied_to
        .connect(&two.pc_machine, &hello(), &Tls::System)
        .await
        .unwrap();
    let removal = Change::sign(
        lied_to.list.head(),
        Action::Remove {
            key: device_key(2).public(),
        },
        &two.pc,
    );
    let refused = lied_to.submit(&mut session, removal).await.unwrap_err();
    assert!(
        refused.to_string().contains("the hub withheld this change"),
        "{refused}"
    );
    assert_eq!(lied_to.list, two.pc_device.list);
    served.stop().await;
}
