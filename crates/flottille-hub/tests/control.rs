//! The hub on the network and the devices' control sessions to it (`docs/remote.md`, "The hub",
//! "Pairing", "Architecture"): a hub served on 127.0.0.1, on a port the system chooses, from a
//! temporary state folder; devices with their machine keys in temporary folders and their device
//! keys made from fixed seeds, paired in pair mode. A hub that lies is played by a server that
//! holds the real hub's key and says what the test tells it: every device must refuse what no key
//! it trusts signed.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Action, Change, Colour, ControlError, Device, DeviceKey, Hello, HubKey, Icon, Keyring,
    KeyringError, ListError, MachineKey, MemberList, PairCode, Proposed, Pushed, Role, Session,
    Tls, ToDevice, arrive,
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

/// The key the hub of `folder` holds, as a key a test signs with: what a hub that lies signs
/// with, or what another hub's key signs.
fn hub_signer(folder: &Path) -> HubKey {
    HubKey::load_or_make(folder).unwrap()
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

    /// [`Place::serve`], its waits short: a pairing's session lasts `QUICK_PAIR` and a connection
    /// has `QUICK_ARRIVAL` to arrive, not thirty and five seconds.
    async fn serve_quickly(&self, at: &str) -> Served<SystemClock> {
        let hub = Hub::open(&self.hub_folder(), SystemClock).unwrap();
        let listener = tokio::net::TcpListener::bind(at).await.unwrap();
        let waits = flottille_hub::Waits {
            arrival: QUICK_ARRIVAL,
            pair_idle: QUICK_PAIR,
        };
        flottille_hub::serve_with(hub, listener, None, hello(), waits).unwrap()
    }
}

const QUICK_PAIR: Duration = Duration::from_secs(2);
const QUICK_ARRIVAL: Duration = Duration::from_secs(1);

fn url(address: SocketAddr) -> String {
    format!("http://{address}")
}

/// The card `device` proposes, named `name`, with `role`.
fn proposed(device: &DeviceKey, name: &str, role: Role) -> Proposed {
    Proposed {
        key: device.public(),
        name: name.to_owned(),
        role,
        icon: Icon::Desktop,
        colour: Colour::of(&device.public()),
    }
}

/// Pair mode opened on the hub, as `flottille-hub pair` opens it, its code holding `at`.
fn code_at(served: &Served<SystemClock>, at: SocketAddr) -> PairCode {
    served.with_hub(|hub| hub.open_pairing(&url(at), None).unwrap().1)
}

/// `device` pairs with `code` through `machine`'s session, as `name` with `role`, then declares its
/// machine, as a computer does: what it pinned, and its session, a member's machine's now.
async fn pair_with(
    code: &PairCode,
    device: &DeviceKey,
    machine: &MachineKey,
    name: &str,
    role: Role,
) -> Result<(Device, Session), ControlError> {
    let mut paired = Device::pair(
        code,
        proposed(device, name, role),
        machine,
        &hello(),
        &Tls::System,
    )
    .await?;
    let (mut pinned, _) = paired.listed().await?;
    let declared = Change::sign(
        pinned.list.head(),
        Action::Machine {
            key: device.public(),
            machine: machine.public(),
        },
        device,
    )
    .claimed_by(machine);
    pinned.submit(&mut paired.session, declared).await?;
    Ok((pinned, paired.session))
}

/// Pair mode opened, and `device` paired with its code.
async fn pair_device(
    served: &Served<SystemClock>,
    device: &DeviceKey,
    machine: &MachineKey,
    name: &str,
    role: Role,
) -> Result<(Device, Session), ControlError> {
    let code = code_at(served, served.address());
    pair_with(&code, device, machine, name, role).await
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

/// A hub that lies: it holds the real hub's key (so a device's pin holds) and, to every device
/// that connects, welcomes it and says `frames`.
async fn lying_hub(folder: &Path, frames: Vec<ToDevice>) -> SocketAddr {
    answering_hub(HubKey::load_or_make(folder).unwrap(), frames).await
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

/// `pc`, a runner, and `server`, a runner, both paired, each with its device pinned to the hub;
/// the hub's list as they hold it: the hub's root, then each one's entry and machine.
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
    let (mut pc_device, mut pc_session) = pair_device(served, &pc, &pc_machine, "pc", Role::Runner)
        .await
        .unwrap();
    let (server_device, server_session) =
        pair_device(served, &server, &server_machine, "server", Role::Runner)
            .await
            .unwrap();
    drop(server_session);
    pc_device.take(list_of(&mut pc_session, 5).await).unwrap();
    assert_eq!(server_device.members().unwrap().members().len(), 2);
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

/// The pair command's output, line by line, from its process.
fn pair_command(state: &Path, args: &[&str]) -> (Child, std::sync::mpsc::Receiver<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_flottille-hub"))
        .args(["--state", state.to_str().unwrap(), "pair"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if send.send(line).is_err() {
                return;
            }
        }
    });
    (child, lines)
}

/// The lines a command said up to the pair code, and the code: the line after the blank line
/// that follows "Pair mode is open".
fn read_code(lines: &std::sync::mpsc::Receiver<String>) -> (Vec<String>, String) {
    let mut seen = Vec::new();
    loop {
        let line = lines
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("no code was printed: {seen:?}"));
        seen.push(line);
        if seen.len() >= 3 && seen[seen.len() - 3].starts_with("Pair mode is open") {
            let code = seen.last().unwrap().clone();
            return (seen, code);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_mode_prints_a_code_and_its_qr_and_pairs_one_device() {
    let place = Place::new();
    let state = place.hub_folder();
    let running = Running::start(place.dir.path(), &state);
    let (log, address) = running.until_listening();
    assert!(
        log.iter()
            .any(|line| line.contains("no device is paired yet: run flottille-hub --state")),
        "{log:#?}"
    );
    assert!(
        !log.iter().any(|line| line.contains("setup code")),
        "{log:#?}"
    );

    // `flottille-hub pair` beside the hub that runs: the code holds the hub's address and key.
    let (mut child, lines) = pair_command(&state, &["--public-url", &url(address)]);
    let (said, code) = read_code(&lines);
    assert!(
        said[0].contains(&format!("with the hub at http://{address}")),
        "{said:?}"
    );
    let parsed = PairCode::from_code(&code).unwrap();
    assert_eq!(parsed.hub_url, url(address));
    let mut rest = Vec::new();
    while let Ok(line) = lines.recv_timeout(Duration::from_secs(5)) {
        let waiting = line.contains("Waiting...");
        rest.push(line);
        if waiting {
            break;
        }
    }
    // Its QR, in half blocks, before what to do with it.
    assert!(
        rest.iter()
            .any(|line| line.contains('▀') || line.contains('▄')),
        "{rest:?}"
    );
    assert!(
        rest.iter().any(|line| line.contains("keep it to yourself")),
        "{rest:?}"
    );

    let (pc, machine) = (device_key(1), place.machine("pc"));
    let (device, session) = pair_with(&parsed, &pc, &machine, "pc", Role::Runner)
        .await
        .unwrap();
    let members = device.members().unwrap();
    let entry = members.get(&pc.public()).unwrap();
    assert_eq!((entry.name.as_str(), entry.role), ("pc", Role::Runner));
    assert_eq!(
        entry.added_by, parsed.hub_key,
        "the entry is signed by the hub"
    );
    assert_eq!(entry.machine, Some(machine.public()));
    assert_eq!(device.hub, parsed.hub_key);
    assert_eq!(members.root(), parsed.hub_key);

    // The command says who paired, and returns.
    let paired = lines.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        paired.starts_with(&format!(
            "pc ({}) is paired, a runner",
            pc.public().fingerprint()
        )),
        "{paired}"
    );
    assert!(child.wait().unwrap().success());
    drop(session);
    running.stop();

    // Started again: the member connects; nothing is printed of pair mode.
    let running = Running::start(place.dir.path(), &state);
    let (log, address) = running.until_listening();
    assert!(
        !log.iter().any(|line| line.contains("no device is paired")),
        "{log:#?}"
    );
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
async fn pair_prints_no_code_nobody_can_dial_and_says_how() {
    let place = Place::new();
    let state = place.hub_folder();
    // A hub on the loopback, as the systemd example runs it: its own address is no one's.
    let running = Running::start(place.dir.path(), &state);
    running.until_listening();
    let output = Command::new(env!("CARGO_BIN_EXE_flottille-hub"))
        .args(["--state", state.to_str().unwrap(), "pair"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("loopback"), "{said}");
    assert!(said.contains("--public-url"), "{said}");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("Pair mode is open"),
        "no code was printed"
    );
    // Nothing was opened: the command that gives a URL opens pair mode, and no other is open.
    let (mut child, lines) = pair_command(&state, &["--public-url", "https://hub.example.net"]);
    let (_, code) = read_code(&lines);
    assert_eq!(
        PairCode::from_code(&code).unwrap().hub_url,
        "https://hub.example.net"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    running.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_mode_ends_at_its_minutes_or_when_its_command_stops() {
    let place = Place::new();
    let state = place.hub_folder();
    // No hub runs: the command says so.
    let output = Command::new(env!("CARGO_BIN_EXE_flottille-hub"))
        .args(["--state", state.to_str().unwrap(), "pair"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("no flottille-hub runs with"), "{said}");

    let running = Running::start(place.dir.path(), &state);
    let (_, address) = running.until_listening();
    let (mut child, lines) = pair_command(&state, &["--public-url", &url(address)]);
    let (_, code) = read_code(&lines);
    let parsed = PairCode::from_code(&code).unwrap();
    // A second pair mode while one is open: refused.
    let output = Command::new(env!("CARGO_BIN_EXE_flottille-hub"))
        .args(["--state", state.to_str().unwrap(), "pair"])
        .args(["--public-url", &url(address)])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("pair mode is open already"));

    // The command stopped: pair mode is closed, its code pairs nothing.
    child.kill().unwrap();
    child.wait().unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let refused = pair_with(
        &parsed,
        &device_key(1),
        &place.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap_err();
    assert!(
        refused.to_string().contains("pair mode is not open"),
        "{refused}"
    );
    running.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_code_pairs_one_device_even_two_at_once() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let code = code_at(&served, served.address());
    let (laptop, phone) = (device_key(2), device_key(3));
    let (laptop_machine, phone_machine) = (place.machine("laptop"), place.machine("phone"));
    let (one, other) = tokio::join!(
        pair_with(&code, &laptop, &laptop_machine, "laptop", Role::Runner),
        pair_with(&code, &phone, &phone_machine, "phone", Role::Controller),
    );
    assert!(one.is_ok() != other.is_ok(), "{one:?} {other:?}");
    let refused = one.err().or(other.err()).unwrap();
    assert!(
        refused.to_string().contains("pair mode is not open"),
        "{refused}"
    );
    served.with_hub(|hub| assert_eq!(hub.members().unwrap().members().len(), 1));

    // Played again later, the code pairs nothing either.
    let late = pair_with(
        &code,
        &device_key(4),
        &place.machine("late"),
        "late",
        Role::Viewer,
    )
    .await
    .unwrap_err();
    assert!(late.to_string().contains("pair mode is not open"), "{late}");
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hub_with_another_key_than_its_code_says_is_refused() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    // The code of another hub, whose key is not this one's, pointed at this hub's address.
    let other = Place::new();
    let mut code = code_at(&served, served.address());
    code.hub_key = hub_signer(&other.hub_folder()).public();
    let refused = pair_with(
        &code,
        &device_key(1),
        &place.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(refused, ControlError::AnotherKey { .. }),
        "{refused}"
    );
    assert_eq!(
        refused.to_string(),
        format!(
            "the hub at {} has another key than the one this device pinned",
            served.address()
        )
    );
    // Refused before anything was proved: pair mode is still open, and nothing was paired.
    served.with_hub(|hub| {
        assert!(hub.pairing_left().is_some());
        assert!(hub.members().is_none());
    });
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_paired_again_with_the_same_key_keeps_its_card() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let (pc, machine) = (device_key(1), place.machine("pc"));
    let (first, session) = pair_device(&served, &pc, &machine, "pc", Role::Runner)
        .await
        .unwrap();
    drop(session);
    // Its hub forgotten on the machine (a reinstall), it pairs again with a new code, proposing
    // another name and role: its entry, its role and its machine are those it had.
    let code = code_at(&served, served.address());
    let mut paired = Device::pair(
        &code,
        proposed(&pc, "renamed", Role::Viewer),
        &machine,
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap();
    let (again, members) = paired.listed().await.unwrap();
    assert_eq!(again.list, first.list);
    let entry = members.get(&pc.public()).unwrap();
    assert_eq!((entry.name.as_str(), entry.role), ("pc", Role::Runner));
    assert_eq!(entry.machine, Some(machine.public()));
    // Its machine a member's already, the pairing spent pair mode all the same: its command says
    // the device paired again, and returns.
    served.with_hub(|hub| {
        assert_eq!(hub.pairing_left(), None);
        let Some(flottille_hub::PairEnd::Paired(end)) = hub.pairing_ended(2) else {
            panic!("pair mode is still open");
        };
        assert!(end.again);
    });
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_on_the_hub_is_refused_by_every_device_that_pinned_the_old_list() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let address = served.address();
    let mut two = two(&place, &served).await;
    drop(two.pc_session);
    served.stop().await;

    // On the hub's machine: the list emptied.
    flottille_hub::reset(&place.hub_folder()).unwrap();
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
            "the hub's member list was reset: pair this device again"
        );
        assert_eq!(device.list, kept);
    }

    // Another device paired: the old devices are refused at once, saying it was reset.
    let (_new, _session) = pair_device(
        &served,
        &device_key(9),
        &place.machine("stranger"),
        "stranger",
        Role::Runner,
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
    }
    let _ = two.pc;
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_change_neither_the_hub_nor_a_member_signed_is_refused_everywhere() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut two = two(&place, &served).await;
    let stranger = device_key(7);
    let head = two.pc_device.list.head();
    let renamed = Change::sign(
        head,
        Action::Rename {
            key: two.pc.public(),
            name: "mine".to_owned(),
        },
        &stranger,
    );

    // The hub refuses it, from a member's session.
    let refused = two
        .pc_session
        .ask(flottille_link::Ask::Submit {
            change: renamed.clone(),
        })
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("is signed by a key that is neither a member nor the hub"),
        "{refused}"
    );
    // A member's addition too: a device is paired on the hub.
    let added = Change::sign(
        head,
        Action::Add {
            key: stranger.public(),
            name: "stranger".to_owned(),
            role: Role::Runner,
        },
        &two.pc,
    );
    let refused = two
        .pc_session
        .ask(flottille_link::Ask::Submit {
            change: added.clone(),
        })
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("no member adds one"),
        "{refused}"
    );

    // A hub that sends them anyway is refused by every device.
    for (lie, why) in [
        (
            renamed,
            "change 6 is signed by a key that is neither a member nor the hub",
        ),
        (
            added,
            "change 6 adds a device, signed by pc: a device is paired on the hub",
        ),
    ] {
        let mut changes = two.pc_device.list.changes().to_vec();
        changes.push(lie);
        let lying = lying_hub(
            &place.hub_folder(),
            vec![ToDevice::List(MemberList::from_changes(changes))],
        )
        .await;
        for (device, machine) in [
            (&mut two.pc_device, &two.pc_machine),
            (&mut two.server_device, &two.server_machine),
        ] {
            let refused = told_by(device, lying, machine).await;
            assert!(refused.to_string().contains(why), "{refused}");
        }
    }

    // A change signed by a member that was removed before it.
    let server = device_key(2);
    let mut removed = two.pc_device.list.clone();
    removed.sign(
        Action::Remove {
            key: server.public(),
        },
        &two.pc,
    );
    removed.sign(
        Action::Rename {
            key: two.pc.public(),
            name: "mine".to_owned(),
        },
        &server,
    );
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(removed)]).await;
    let refused = told_by(&mut two.pc_device, lying, &two.pc_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 7 is signed by a key that is neither a member nor the hub"),
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
async fn what_the_hub_signs_is_bounded_and_another_hubs_key_signs_nothing() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let mut two = two(&place, &served).await;
    let hub_key = hub_signer(&place.hub_folder());
    let intruder = device_key(8);

    // A change signed with the hub's key, carried by a member's session: the hub signs on its
    // host only.
    let mut paired = two.pc_device.list.clone();
    paired.sign_as_hub(
        Action::Pair {
            key: intruder.public(),
            name: "intruder".to_owned(),
            role: Role::Runner,
            icon: Icon::Server,
            colour: Colour::Red,
        },
        &hub_key,
    );
    let by_hub = paired.changes().last().unwrap().clone();
    let refused = two
        .pc_session
        .ask(flottille_link::Ask::Submit { change: by_hub })
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("the hub's own key"),
        "{refused}"
    );

    // Another hub's key pairing a device: no device takes it.
    let other = Place::new();
    let mut lie = two.pc_device.list.clone();
    lie.sign_as_hub(
        Action::Pair {
            key: intruder.public(),
            name: "intruder".to_owned(),
            role: Role::Runner,
            icon: Icon::Server,
            colour: Colour::Red,
        },
        &hub_signer(&other.hub_folder()),
    );
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(lie)]).await;
    let refused = told_by(&mut two.server_device, lying, &two.server_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 6 is signed by a key that is neither a member nor the hub"),
        "{refused}"
    );

    // A role changed in a signed entry: the signature no longer holds.
    let mut json = serde_json::to_value(&two.pc_device.list).unwrap();
    assert_eq!(json[3]["action"]["role"], "runner");
    json[3]["action"]["role"] = "viewer".into();
    let changed: MemberList = serde_json::from_value(json).unwrap();
    let lying = lying_hub(&place.hub_folder(), vec![ToDevice::List(changed)]).await;
    let refused = told_by(&mut two.server_device, lying, &two.server_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 4 does not carry its signer's signature"),
        "{refused}"
    );

    // The hub's own key gives a member another machine, or renames it: only the member does.
    let mut lie = two.pc_device.list.clone();
    lie.sign_as_hub(
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
            .contains("change 6 is the hub's and would declare the machine of a member"),
        "{refused}"
    );
    // Nothing of it was kept: the members and roles are those the hub and the members signed.
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
            format!("the hub at {address} has another key than the one this device pinned")
        );
    }
    impostor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_removed_by_a_member_or_on_the_hub_is_refused_its_session() {
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
    // On the hub's host: the other one goes too.
    served.with_hub(|hub| hub.remove("pc").unwrap());
    let refused = two
        .pc_device
        .connect(&two.pc_machine, &hello(), &Tls::System)
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
async fn the_hubs_host_changes_a_role_and_every_device_sees_it() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let socket = place.hub_folder().join(flottille_hub::SOCKET_FILE);
    served.serve_local(&socket).unwrap();
    let mut two = two(&place, &served).await;
    let ask = |line: serde_json::Value| {
        let socket = socket.clone();
        async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
            stream
                .write_all(format!("{line}\n").as_bytes())
                .await
                .unwrap();
            let mut answer = String::new();
            tokio::io::BufReader::new(stream)
                .read_line(&mut answer)
                .await
                .unwrap();
            serde_json::from_str::<serde_json::Value>(&answer).unwrap()
        }
    };
    let changed = ask(serde_json::json!({"role": {"device": "server", "role": "viewer"}})).await;
    assert_eq!(changed["ok"]["role"], "viewer", "{changed}");
    let members = two
        .pc_device
        .take(list_of(&mut two.pc_session, 6).await)
        .unwrap();
    let server = members.get(&device_key(2).public()).unwrap();
    assert_eq!(server.role, Role::Viewer);
    let refused = ask(serde_json::json!({"role": {"device": "server", "role": "chief"}})).await;
    assert_eq!(refused["refused"], "a role is runner, controller or viewer");
    let devices = ask(serde_json::json!({"devices": {}})).await;
    assert_eq!(devices["ok"][1]["role"], "viewer", "{devices}");
    assert_eq!(devices["ok"][1]["added_by"], "the hub", "{devices}");
    let removed = ask(serde_json::json!({"remove": {"device": "server"}})).await;
    assert_eq!(removed["ok"]["name"], "server", "{removed}");
    let members = two
        .pc_device
        .take(list_of(&mut two.pc_session, 7).await)
        .unwrap();
    assert!(members.get(&device_key(2).public()).is_none());
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

// --------------------------------------------------------------------------------------------
// What a stranger, someone on the path, or a hub that lies may try.

/// A hub that holds `key`, welcomes every device, says `frames`, then answers every ask as done.
async fn answering_hub(key: HubKey, frames: Vec<ToDevice>) -> SocketAddr {
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

/// A device that comes to the hub with no list: pinned to the hub's key, with its own key as a
/// root it never had.
fn stranger_at(served: &Served<SystemClock>, stranger: &DeviceKey) -> Device {
    Device {
        url: url(served.address()),
        hub: served.with_hub(|hub| hub.key()),
        root: stranger.public(),
        list: MemberList::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_paired_after_a_reset_does_not_bring_back_a_removed_member() {
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
    assert_eq!(old.changes().len(), 6);
    drop(two.pc_session);
    served.stop().await;

    // The hub reset, and the very device, machine and name paired again.
    flottille_hub::reset(&place.hub_folder()).unwrap();
    let served = place.serve(&address.to_string()).await;
    let (mut again, session) = pair_device(&served, &two.pc, &two.pc_machine, "pc", Role::Runner)
        .await
        .unwrap();
    let fresh = again.list.clone();
    assert_eq!(fresh.changes().len(), 3);
    assert_ne!(fresh.changes()[0], old.changes()[0], "a new list's id");

    // The old list's changes do not follow on the new one: the hub refuses them...
    let refused = session
        .ask(flottille_link::Ask::Submit {
            change: old.changes()[5].clone(),
        })
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("does not follow the change before it"),
        "{refused}"
    );
    // ...and the device paired again refuses a hub that serves them anyway.
    let mut lie = fresh.changes().to_vec();
    lie.extend_from_slice(&old.changes()[3..5]);
    let lying = lying_hub(
        &place.hub_folder(),
        vec![ToDevice::List(MemberList::from_changes(lie))],
    )
    .await;
    let refused = told_by(&mut again, lying, &two.pc_machine).await;
    assert!(
        refused
            .to_string()
            .contains("change 4 does not follow the change before it"),
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
    let (_device, _session) = pair_device(
        &served,
        &device_key(1),
        &place.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap();
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pairing_session_that_declares_no_machine_is_closed() {
    let place = Place::new();
    let served = place.serve_quickly("127.0.0.1:0").await;
    let code = code_at(&served, served.address());
    let pc = device_key(1);
    let mut paired = Device::pair(
        &code,
        proposed(&pc, "pc", Role::Runner),
        &place.machine("pc"),
        &hello(),
        &Tls::System,
    )
    .await
    .unwrap();
    paired.listed().await.unwrap();
    // It keeps asking, within the 2 s a pairing's session is given: still closed.
    let asker = paired.session.asker();
    let asking = tokio::spawn(async move {
        loop {
            let _ = asker
                .ask(flottille_link::Ask::Addresses {
                    addresses: Vec::new(),
                })
                .await;
            tokio::time::sleep(QUICK_PAIR / 4).await;
        }
    });
    let ended = tokio::time::timeout(QUICK_PAIR * 5 / 2, async {
        while paired.session.next().await.is_some() {}
    })
    .await;
    asking.abort();
    assert!(
        ended.is_ok(),
        "the pairing's session is still open after {:?}",
        QUICK_PAIR * 5 / 2
    );
    assert!(
        paired.session.ended().contains("declared none in time"),
        "{}",
        paired.session.ended()
    );
    // The device stays paired: it declares its machine in its next session.
    served.with_hub(|hub| assert!(hub.members().unwrap().get(&pc.public()).is_some()));
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn members_still_connect_while_strangers_hold_the_unauthenticated_places() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let (pc, pc_machine) = (device_key(1), place.machine("pc"));
    let (pc_device, pc_session) = pair_device(&served, &pc, &pc_machine, "pc", Role::Runner)
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
    assert_eq!(list.changes().len(), 3);
    drop(idle);
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_says_nothing_is_let_go_within_a_few_seconds() {
    use tokio::io::AsyncReadExt;
    let place = Place::new();
    let served = place.serve_quickly("127.0.0.1:0").await;
    let mut stream = tokio::net::TcpStream::connect(served.address())
        .await
        .unwrap();
    let mut byte = [0u8; 1];
    let closed = tokio::time::timeout(QUICK_ARRIVAL * 5, stream.read(&mut byte)).await;
    assert!(
        matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
        "a connection that says nothing is still held after {:?}: {closed:?}",
        QUICK_ARRIVAL * 5
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
async fn the_pair_code_never_crosses_the_network() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let (relayed, seen) = relay(served.address(), std::sync::Arc::new(str::to_owned)).await;
    let code = code_at(&served, relayed);
    let text = code.code().unwrap();
    let secret: String = code
        .secret
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let (_device, _session) = pair_with(
        &code,
        &device_key(1),
        &place.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap();
    let seen = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
    assert!(
        seen.contains("\"pair\""),
        "the pairing went through the relay"
    );
    assert!(
        !seen.to_uppercase().contains(&text),
        "the pair code went over the network"
    );
    assert!(
        !seen.contains(&secret),
        "the pair code's secret went over the network"
    );
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pairing_whose_card_is_changed_on_the_way_is_refused() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    // Someone on the path raises the role the device proposed.
    let rewrite = std::sync::Arc::new(move |line: &str| {
        let Ok(mut said) = serde_json::from_str::<serde_json::Value>(line) else {
            return line.to_owned();
        };
        if said["pair"].is_null() {
            return line.to_owned();
        }
        said["pair"]["proposed"]["role"] = "runner".into();
        format!("{said}\n")
    });
    let (relayed, _) = relay(served.address(), rewrite).await;
    let code = code_at(&served, relayed);
    let refused = pair_with(
        &code,
        &device_key(1),
        &place.machine("phone"),
        "phone",
        Role::Viewer,
    )
    .await
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("did not prove it holds the pair code"),
        "{refused}"
    );
    served.with_hub(|hub| {
        assert!(hub.members().is_none());
        // A wrong proof does not close pair mode: the user's device may still pair.
        assert!(hub.pairing_left().is_some());
    });
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_cannot_close_pair_mode() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let code = code_at(&served, served.address());
    // A throwaway key per try, a code of its own with the right hub key and a wrong secret:
    // far more tries than any limit a pair mode could have had.
    for n in 0..12u8 {
        let wrong = PairCode {
            secret: flottille_link::PairSecret([n.wrapping_add(1); 16]),
            ..code.clone()
        };
        let refused = pair_with(
            &wrong,
            &device_key(100 + n),
            &place.machine("stranger"),
            "stranger",
            Role::Runner,
        )
        .await
        .unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("did not prove it holds the pair code"),
            "{refused}"
        );
    }
    served.with_hub(|hub| {
        assert!(hub.members().is_none());
        assert!(hub.pairing_left().is_some());
    });
    // The user's device pairs, with its code, after all of them.
    let (device, _session) = pair_with(
        &code,
        &device_key(1),
        &place.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap();
    assert_eq!(device.list.members().len(), 1);
    served.stop().await;
}

/// What a build of before this protocol says first: protocol 1, the frames of before pair mode.
fn old_hello() -> Hello {
    Hello {
        flottille: "0.3.9".to_owned(),
        protocols: vec![1],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_device_is_told_to_update_by_the_hub() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let code = code_at(&served, served.address());
    let refused = Device::pair(
        &code,
        proposed(&device_key(1), "pc", Role::Runner),
        &place.machine("pc"),
        &old_hello(),
        &Tls::System,
    )
    .await
    .err()
    .expect("an old device is refused");
    assert!(
        refused.to_string().contains("update one of them"),
        "{refused}"
    );
    // Nothing was paired, and pair mode is still open for a device that can.
    served.with_hub(|hub| {
        assert!(hub.members().is_none());
        assert!(hub.pairing_left().is_some());
    });
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_hub_is_told_to_update_by_the_device() {
    let place = Place::new();
    let key = hub_signer(&place.hub_folder());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // The hub of before: its hello says protocol 1. Its refusal is its own to write.
        let _ = arrive(Box::new(stream), &key, &old_hello()).await;
    });
    let code = PairCode {
        hub_url: url(address),
        hub_key: hub_signer(&place.hub_folder()).public(),
        secret: flottille_link::PairSecret([1; 16]),
    };
    let refused = pair_with(
        &code,
        &device_key(1),
        &place.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap_err();
    assert!(
        refused.to_string().contains("update one of them"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pairing_played_again_pairs_nothing() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    let (relayed, seen) = relay(served.address(), std::sync::Arc::new(str::to_owned)).await;
    let code = code_at(&served, relayed);
    let (_device, session) = pair_with(
        &code,
        &device_key(1),
        &place.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap();
    drop(session);
    // Pair mode opened again for the user's next device; someone who kept every byte of the first
    // pairing plays them again: the hub's nonce is a new one, and nothing is paired.
    let _next = code_at(&served, served.address());
    let played: Vec<u8> = seen.lock().unwrap().clone();
    let mut stream = tokio::net::TcpStream::connect(served.address())
        .await
        .unwrap();
    let _ = stream.write_all(&played).await;
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
    served.with_hub(|hub| {
        assert_eq!(hub.members().unwrap().members().len(), 1);
        assert!(hub.pairing_left().is_some(), "the replay spent pair mode");
    });
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hub_that_sends_a_list_without_the_device_is_refused() {
    let place = Place::new();
    // A hub that holds the key the code says, as a hub broken into does: it welcomes the pairing,
    // then sends a list that does not hold the device.
    let other = Place::new();
    let elsewhere = {
        let mut list = MemberList::start_hub(&hub_signer(&place.hub_folder())).unwrap();
        list.sign_as_hub(
            Action::Pair {
                key: device_key(6).public(),
                name: "elsewhere".to_owned(),
                role: Role::Runner,
                icon: Icon::Server,
                colour: Colour::Grey,
            },
            &hub_signer(&place.hub_folder()),
        );
        list
    };
    let withholding = answering_hub(
        hub_signer(&place.hub_folder()),
        vec![ToDevice::List(elsewhere)],
    )
    .await;
    let code = PairCode {
        hub_url: url(withholding),
        hub_key: hub_signer(&place.hub_folder()).public(),
        secret: flottille_link::PairSecret([1; 16]),
    };
    let refused = pair_with(
        &code,
        &device_key(1),
        &other.machine("pc"),
        "pc",
        Role::Runner,
    )
    .await
    .unwrap_err();
    assert!(
        refused.to_string().contains("does not hold this device"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_learns_nothing_of_the_list_from_its_refusal() {
    let place = Place::new();
    let served = place.serve("127.0.0.1:0").await;
    // With no member yet, a stranger gets no session either.
    let refused = stranger_at(&served, &device_key(7))
        .connect(&place.machine("stranger"), &hello(), &Tls::System)
        .await
        .unwrap_err();
    assert!(matches!(refused, ControlError::Refused { .. }), "{refused}");
    let pc = device_key(1);
    let (_device, _session) = pair_device(&served, &pc, &place.machine("pc"), "pc", Role::Runner)
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
