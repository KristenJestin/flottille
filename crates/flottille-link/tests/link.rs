//! The link between two machines (`docs/remote.md`, "Architecture", "The transport", "The
//! version"): two endpoints in one test, each with its own key in a temporary folder, on
//! 127.0.0.1 only. Flottille handles no network channel: no relay, no NAT traversal, no
//! discovery; a machine dials the addresses it is given for a key.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use flottille_link::{
    Endpoint, Hello, Link, LinkError, MachineKey, MachineMessage, Message, PeerState, PublicKey,
    Traffic,
};
use serde_json::json;

/// A machine of the test: its key in a temporary state folder, its endpoint on loopback.
struct Machine {
    _state: tempfile::TempDir,
    endpoint: Endpoint,
}

impl Machine {
    async fn new() -> Machine {
        Machine::at("127.0.0.1").await
    }

    /// A machine listening on loopback at `ip`: another host, as the other side sees it.
    async fn at(ip: &str) -> Machine {
        let state = tempfile::tempdir().unwrap();
        let key = MachineKey::load_or_make(&state.path().join("remote")).unwrap();
        let ip: IpAddr = ip.parse().unwrap();
        let endpoint = Endpoint::bind(&key, SocketAddr::new(ip, 0)).unwrap();
        Machine {
            _state: state,
            endpoint,
        }
    }

    fn key(&self) -> PublicKey {
        self.endpoint.key()
    }

    fn addresses(&self) -> Vec<SocketAddr> {
        self.endpoint.addresses()
    }
}

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn hello(version: &str, protocols: &[u32]) -> Hello {
    Hello {
        flottille: version.to_owned(),
        protocols: protocols.to_vec(),
    }
}

/// The other side accepts the next link that comes, from `trusted` only, and answers each
/// message it receives with what it received.
fn serve(
    machine: &Machine,
    trusted: PublicKey,
    hello: Hello,
) -> tokio::task::JoinHandle<Result<Link, LinkError>> {
    let endpoint = machine.endpoint.clone();
    tokio::spawn(async move {
        let accepting = endpoint.accept().await.expect("the endpoint is open");
        if accepting.peer() != trusted {
            accepting.refuse("refused: this machine's key is not one of the user's");
            return Err(LinkError::Closed("refused".to_owned()));
        }
        let link = accepting.handshake(&hello, "laptop").await?;
        let answering = link.clone();
        tokio::spawn(async move {
            while let Some(received) = answering.next().await {
                let echo = serde_json::to_value(received.message()).unwrap();
                received.answer(Ok(echo));
            }
        });
        Ok(link)
    })
}

const WAIT: Duration = Duration::from_secs(20);

#[tokio::test]
async fn two_endpoints_that_trust_each_other_link_over_loopback() {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let ours = Hello::ours("0.4.1");
    let serving = serve(&server, laptop.key(), ours.clone());

    let link = tokio::time::timeout(
        WAIT,
        laptop.endpoint.connect(
            server.key(),
            &server.addresses(),
            Traffic::Machine,
            &ours,
            "server",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let accepted = serving.await.unwrap().unwrap();

    assert_eq!(link.peer(), server.key());
    assert_eq!(accepted.peer(), laptop.key());
    assert_eq!(link.traffic(), Traffic::Machine);
    assert_eq!(accepted.traffic(), Traffic::Machine);
    assert_eq!(link.protocol(), flottille_protocol::PROTOCOL);
    assert_eq!(link.theirs(), &ours);
    let told = Message::Machine(MachineMessage::Addresses(vec!["192.0.2.7:4000".into()]));
    let answer = link.send(&told).await.unwrap();
    assert_eq!(
        answer,
        json!({"machine": {"addresses": ["192.0.2.7:4000"]}})
    );
}

#[tokio::test]
async fn a_key_that_is_not_trusted_is_refused() {
    let (laptop, server, stranger) = (
        Machine::new().await,
        Machine::new().await,
        Machine::new().await,
    );
    let ours = Hello::ours("0.4.1");
    // The server trusts the laptop only.
    let serving = serve(&server, laptop.key(), ours.clone());

    let refused = tokio::time::timeout(
        WAIT,
        stranger.endpoint.connect(
            server.key(),
            &server.addresses(),
            Traffic::Machine,
            &ours,
            "server",
        ),
    )
    .await
    .unwrap()
    .unwrap_err();

    assert!(serving.await.unwrap().is_err());
    assert_eq!(
        refused.to_string(),
        "server refused the link: refused: this machine's key is not one of the user's"
    );
    assert_eq!(refused.state(), PeerState::Offline);
}

#[tokio::test]
async fn a_machine_answering_with_another_key_is_refused() {
    let (laptop, server, other) = (
        Machine::new().await,
        Machine::new().await,
        Machine::new().await,
    );
    let ours = Hello::ours("0.4.1");
    let _serving = serve(&server, laptop.key(), ours.clone());

    // The server's address, dialled for another key: the transport's own check refuses it.
    let failed = tokio::time::timeout(
        Duration::from_secs(30),
        laptop.endpoint.connect(
            other.key(),
            &server.addresses(),
            Traffic::Machine,
            &ours,
            "other",
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    // Refused by the dialler's own check of the key, not by anything the server said.
    assert!(matches!(failed, LinkError::Unreachable { .. }), "{failed}");
    assert!(
        failed.to_string().contains("invalid peer certificate"),
        "{failed}"
    );
}

/// A host that answers the server's retry, then never finishes its handshake: a real client
/// dialling through a relay at `ip`, which passes the server everything the server sends, and the
/// client's first two datagrams only (the first, and the one that carries the retry's token).
fn staller(ip: &'static str, server: SocketAddr, key: PublicKey) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = Machine::new().await;
        stall(client.endpoint.clone(), ip, server, key).await;
    })
}

/// A staller as `staller`, dialling from `client`.
async fn stall(client: Endpoint, ip: &str, server: SocketAddr, key: PublicKey) {
    let ip: IpAddr = ip.parse().unwrap();
    let relay = tokio::net::UdpSocket::bind(SocketAddr::new(ip, 0))
        .await
        .unwrap();
    let at = relay.local_addr().unwrap();
    let dialling = async {
        let _ = client
            .connect(
                key,
                &[at],
                Traffic::Machine,
                &Hello::ours("0.4.1"),
                "server",
            )
            .await;
        std::future::pending::<()>().await;
    };
    tokio::join!(dialling, relaying(&relay, server));
}

/// What a staller's relay does: passes the server's datagrams to the client, and the client's
/// first two only.
async fn relaying(relay: &tokio::net::UdpSocket, server: SocketAddr) {
    let (mut dialler, mut passed, mut datagram) = (None, 0, vec![0u8; 65536]);
    loop {
        let (size, from) = relay.recv_from(&mut datagram).await.unwrap();
        if from == server {
            if let Some(dialler) = dialler {
                let _ = relay.send_to(&datagram[..size], dialler).await;
            }
        } else {
            dialler = Some(from);
            if passed < 2 {
                passed += 1;
                let _ = relay.send_to(&datagram[..size], server).await;
            }
        }
    }
}

/// How long a held staller's round lasts: less than the server's handshake time, so that the next
/// round asks for the place as it is freed.
const ROUND: Duration = Duration::from_millis(4500);

/// A staller as `stall` that holds its place for as long as it lives, however slow the machine
/// it runs on: when the server lets its handshake go, or refused it for want of a place, it dials
/// again. `placed` counts the stallers that are holding a place just now.
async fn hold(
    client: Endpoint,
    ip: &str,
    server: SocketAddr,
    key: PublicKey,
    placed: Arc<AtomicUsize>,
) {
    let ip: IpAddr = ip.parse().unwrap();
    loop {
        let relay = tokio::net::UdpSocket::bind(SocketAddr::new(ip, 0))
            .await
            .unwrap();
        let at = relay.local_addr().unwrap();
        let round = async {
            let (at, ours) = ([at], Hello::ours("0.4.1"));
            let answered = client.connect(key, &at, Traffic::Machine, &ours, "server");
            tokio::pin!(answered);
            // Refused, or failing at once: no place was taken.
            tokio::select! {
                _ = &mut answered => return false,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
            placed.fetch_add(1, Ordering::SeqCst);
            let _ = tokio::time::timeout(ROUND - Duration::from_secs(1), &mut answered).await;
            placed.fetch_sub(1, Ordering::SeqCst);
            true
        };
        let held = tokio::select! {
            _ = relaying(&relay, server) => unreachable!("the relay never ends"),
            held = round => held,
        };
        if !held {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Dials `server` until it links; how long it took.
async fn link_timed(laptop: &Machine, server: &Machine, ours: &Hello) -> (Link, Duration) {
    let started = Instant::now();
    let link = tokio::time::timeout(
        WAIT,
        laptop.endpoint.connect(
            server.key(),
            &server.addresses(),
            Traffic::Machine,
            ours,
            "server",
        ),
    )
    .await
    .expect("the trusted pair links in time")
    .expect("the trusted pair links");
    (link, started.elapsed())
}

#[tokio::test]
async fn a_stalled_handshake_does_not_stop_a_trusted_pair_from_linking() {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let ours = Hello::ours("0.4.1");
    let at = server.addresses()[0];
    // Hosts that prove their address, then stall: fifteen from one address, three from another,
    // more than the handshakes the server runs at once.
    let mut stallers: Vec<_> = (0..15)
        .map(|_| staller("127.0.0.2", at, server.key()))
        .collect();
    stallers.extend((0..3).map(|_| staller("127.0.0.3", at, server.key())));
    tokio::time::sleep(Duration::from_millis(500)).await;

    let serving = serve(&server, laptop.key(), ours.clone());
    let (link, took) = link_timed(&laptop, &server, &ours).await;
    serving.await.unwrap().unwrap();
    assert!(
        took < Duration::from_secs(3),
        "the trusted pair waited {took:?}"
    );
    assert_eq!(link.peer(), server.key());
    for staller in stallers {
        staller.abort();
    }
}

#[tokio::test]
async fn eight_stalling_hosts_do_not_stop_a_trusted_pair() {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let ours = Hello::ours("0.4.1");
    let at = server.addresses()[0];
    // Eight hosts, each stalling as many handshakes as one host may hold, and more.
    const HOSTS: [&str; 8] = [
        "127.0.1.1",
        "127.0.1.2",
        "127.0.1.3",
        "127.0.1.4",
        "127.0.1.5",
        "127.0.1.6",
        "127.0.1.7",
        "127.0.1.8",
    ];
    let stallers: Vec<_> = HOSTS
        .iter()
        .flat_map(|ip| (0..3).map(|_| staller(ip, at, server.key())))
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let serving = serve(&server, laptop.key(), ours.clone());
    let (link, took) = link_timed(&laptop, &server, &ours).await;
    serving.await.unwrap().unwrap();
    assert!(
        took < Duration::from_secs(3),
        "the trusted pair waited {took:?}"
    );
    assert_eq!(link.peer(), server.key());
    for staller in stallers {
        staller.abort();
    }
}

#[tokio::test]
async fn stalling_hosts_past_every_place_do_not_stop_a_known_machine() {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let ours = Hello::ours("0.4.1");
    let at = server.addresses()[0];
    // The server knows the laptop's address: a place is kept for it.
    server.endpoint.know([laptop.addresses()[0].ip()]);
    // More hosts than the places for all hold, two by host. They hold their places on their own
    // and ask again for those they were refused, so the setup may take as long as a loaded
    // machine needs.
    let (key, client, placed) = (
        server.key(),
        Machine::new().await,
        Arc::new(AtomicUsize::new(0)),
    );
    let mut stallers = Vec::new();
    for host in 0..140 {
        let ip = format!("127.0.{}.{}", 2 + host / 200, 1 + host % 200);
        for _ in 0..2 {
            let (endpoint, ip, placed) = (client.endpoint.clone(), ip.clone(), placed.clone());
            stallers.push(tokio::spawn(async move {
                hold(endpoint, &ip, at, key, placed).await
            }));
        }
        // Ten at a time: of a burst past the 32 connections the transport queues, the rest is
        // refused before it takes a place.
        if host % 5 == 4 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    // The server refuses every key but the laptop's, as long as they come.
    let serving = {
        let (endpoint, laptop, ours) = (server.endpoint.clone(), laptop.key(), ours.clone());
        tokio::spawn(async move {
            loop {
                let accepting = endpoint.accept().await.expect("the endpoint is open");
                if accepting.peer() == laptop {
                    return accepting.handshake(&ours, "laptop").await;
                }
                accepting.refuse("refused: this machine's key is not one of the user's");
            }
        })
    };
    // Until a stranger finds no place: refused at once, before any TLS. It waits for the state,
    // not for a time: as long as it takes a loaded machine, 30 s at most.
    let stranger = Machine::at("127.0.9.1").await;
    let (started, mut last) = (Instant::now(), String::new());
    let full = loop {
        let dialled = tokio::time::timeout(
            Duration::from_millis(500),
            stranger
                .endpoint
                .connect(server.key(), &[at], Traffic::Machine, &ours, "server"),
        )
        .await;
        last = match &dialled {
            Ok(Err(error)) if error.to_string().contains("refused to accept") => break true,
            Ok(Err(error)) => error.to_string(),
            Ok(Ok(_)) => "linked".to_string(),
            Err(_) => "no answer in 500 ms".to_string(),
        };
        if started.elapsed() > Duration::from_secs(30) {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(
        full,
        "the stallers never held every place: {} of 280 hold one after {:?}, the stranger's last dial: {last}",
        placed.load(Ordering::SeqCst),
        started.elapsed()
    );

    let (link, took) = link_timed(&laptop, &server, &ours).await;
    serving.await.unwrap().unwrap();
    assert!(
        took < Duration::from_secs(3),
        "the trusted pair waited {took:?}"
    );
    assert_eq!(link.peer(), server.key());
    for staller in stallers {
        staller.abort();
    }
}

#[tokio::test]
async fn a_flood_of_untrusted_keys_does_not_stop_a_trusted_pair() {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let ours = Hello::ours("0.4.1");
    // The server refuses every key but the laptop's, as long as they come.
    let (sender, mut trusted) = tokio::sync::mpsc::channel(1);
    let refusing = {
        let (endpoint, laptop, ours) = (server.endpoint.clone(), laptop.key(), ours.clone());
        tokio::spawn(async move {
            while let Some(accepting) = endpoint.accept().await {
                if accepting.peer() != laptop {
                    accepting.refuse("refused: this machine's key is not one of the user's");
                    continue;
                }
                let _ = sender
                    .send(accepting.handshake(&ours, "laptop").await)
                    .await;
            }
        })
    };
    // Strangers, each with a key of its own, dialling again as soon as they are refused.
    let mut strangers = Vec::new();
    for _ in 0..20 {
        let stranger = Machine::at("127.0.0.4").await;
        let (key, at, ours) = (server.key(), server.addresses(), ours.clone());
        strangers.push(tokio::spawn(async move {
            loop {
                let _ = stranger
                    .endpoint
                    .connect(key, &at, Traffic::Machine, &ours, "server")
                    .await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    let (link, took) = link_timed(&laptop, &server, &ours).await;
    trusted.recv().await.unwrap().unwrap();
    assert!(
        took < Duration::from_secs(3),
        "the trusted pair waited {took:?}"
    );
    assert_eq!(link.peer(), server.key());
    for stranger in strangers {
        stranger.abort();
    }
    refusing.abort();
}

#[tokio::test]
async fn a_dialler_s_socket_takes_no_link_after_its_first_dial() {
    let state = tempfile::tempdir().unwrap();
    let key = MachineKey::load_or_make(&state.path().join("remote")).unwrap();
    let dialler = Endpoint::dialler(&key).unwrap();
    // Its first dial opens its socket: a trap catches where it dials from.
    let trap = tokio::net::UdpSocket::bind(loopback()).await.unwrap();
    let dialling = {
        let (endpoint, at) = (dialler.clone(), trap.local_addr().unwrap());
        let other = Machine::new().await;
        let key = other.key();
        tokio::spawn(async move {
            let _other = other;
            let _ = endpoint
                .connect(key, &[at], Traffic::Machine, &Hello::ours("0.4.1"), "trap")
                .await;
        })
    };
    let mut datagram = vec![0u8; 4096];
    let (_, socket) = tokio::time::timeout(WAIT, trap.recv_from(&mut datagram))
        .await
        .unwrap()
        .unwrap();
    dialling.abort();

    // Dialled at that socket, with its own key: no link.
    let other = Machine::new().await;
    let tried = tokio::time::timeout(
        WAIT,
        other.endpoint.connect(
            dialler.key(),
            &[socket],
            Traffic::Machine,
            &Hello::ours("0.4.1"),
            "dialler",
        ),
    )
    .await
    .expect("the dial ends");
    assert!(tried.is_err(), "the dialler took a link");
    let accepted = tokio::time::timeout(Duration::from_secs(1), dialler.accept())
        .await
        .expect("accept ends at once");
    assert!(accepted.is_none());
}

#[tokio::test]
async fn no_common_protocol_refuses_the_link_on_both_sides() {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let serving = serve(&server, laptop.key(), hello("0.4.3", &[9]));

    let refused = tokio::time::timeout(
        WAIT,
        laptop.endpoint.connect(
            server.key(),
            &server.addresses(),
            Traffic::Machine,
            &hello("0.4.1", &[8]),
            "server",
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    let other = tokio::time::timeout(WAIT, serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();

    assert_eq!(
        refused.to_string(),
        "server runs Flottille 0.4.3 (protocol 9), this machine 0.4.1 (protocol 8): update one \
         of them (flottille update)"
    );
    assert_eq!(refused.state(), PeerState::NeedsUpdate);
    assert_eq!(
        other.to_string(),
        "laptop runs Flottille 0.4.1 (protocol 8), this machine 0.4.3 (protocol 9): update one \
         of them (flottille update)"
    );
    assert_eq!(other.state(), PeerState::NeedsUpdate);
}

#[tokio::test]
async fn a_machine_message_on_the_client_side_is_refused_and_the_reverse() {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let ours = Hello::ours("0.4.1");
    let machine_message =
        Message::Machine(MachineMessage::Addresses(vec!["192.0.2.7:4000".into()]));
    let client_message = Message::Client(json!({"id": 1, "method": "daemon.status"}));

    // A client link: a client's request goes through, a machine's message is refused.
    let serving = serve(&server, laptop.key(), ours.clone());
    let client = laptop
        .endpoint
        .connect(
            server.key(),
            &server.addresses(),
            Traffic::Client,
            &ours,
            "server",
        )
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
    assert_eq!(client.traffic(), Traffic::Client);
    assert_eq!(
        client.send(&client_message).await.unwrap(),
        json!({"client": {"id": 1, "method": "daemon.status"}})
    );
    let refused = client.send(&machine_message).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        "server refused: a machine's message is not taken on a client link"
    );

    // A machine link: the reverse.
    let serving = serve(&server, laptop.key(), ours.clone());
    let machine = laptop
        .endpoint
        .connect(
            server.key(),
            &server.addresses(),
            Traffic::Machine,
            &ours,
            "server",
        )
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
    assert!(machine.send(&machine_message).await.is_ok());
    let refused = machine.send(&client_message).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        "server refused: a client's request is not taken on a machine link"
    );
    // A message neither kind knows is refused too, and the link goes on.
    let refused = machine
        .send_raw(br#"{"machine": {"shell": "rm -rf /"}}"#)
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .starts_with("server refused: not a message a machine may send"),
        "{refused}"
    );
    assert!(machine.send(&machine_message).await.is_ok());
}

#[tokio::test]
async fn the_endpoint_reaches_only_what_it_is_given() {
    let machine = Machine::new().await;
    // Its addresses are the one it listens on, nothing found elsewhere.
    assert_eq!(
        machine.addresses(),
        vec![machine.endpoint.listening().unwrap()]
    );
    assert!(machine.addresses()[0].ip().is_loopback());

    // A key with no address is not looked up anywhere: refused at once.
    let other = Machine::new().await;
    let started = Instant::now();
    let failed = machine
        .endpoint
        .connect(
            other.key(),
            &[],
            Traffic::Machine,
            &Hello::ours("0.4.1"),
            "other",
        )
        .await
        .unwrap_err();
    assert!(matches!(failed, LinkError::NoAddress(_)), "{failed}");
    assert_eq!(
        failed.to_string(),
        "other has no address this machine knows"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// The files of the crate's sources.
fn sources() -> Vec<(PathBuf, String)> {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            (path, text)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn the_transport_is_named_in_one_module_only() {
    for (path, text) in sources() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name != "transport.rs" {
            for library in ["quinn", "rustls"] {
                assert!(!text.contains(library), "{name} names {library}");
            }
        }
    }
}
