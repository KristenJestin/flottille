//! The link between two machines (`docs/remote.md`, "Architecture", "The transport", "The
//! version"): endpoints in one test, each with its own key in a temporary folder, their links
//! through relay ports joined in memory as the hub joins its members' sessions. Flottille handles
//! no network channel: an endpoint opens no socket, and dials a key, never an address.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use flottille_link::{
    Endpoint, Hello, Link, LinkError, MachineKey, MachineMessage, Message, PeerState, PublicKey,
    Traffic,
};
use serde_json::json;

/// A machine of the test: its key in a temporary state folder, and its endpoint.
struct Machine {
    _state: tempfile::TempDir,
    endpoint: Endpoint,
}

impl Machine {
    fn new() -> Machine {
        let state = tempfile::tempdir().unwrap();
        let key = MachineKey::load_or_make(&state.path().join("remote")).unwrap();
        let endpoint = Endpoint::new(&key).unwrap();
        Machine {
            _state: state,
            endpoint,
        }
    }

    fn key(&self) -> PublicKey {
        self.endpoint.key()
    }
}

/// `N` machines, each through a relay port of its own key, the ports joined as a hub joins them.
async fn machines<const N: usize>() -> [Machine; N] {
    let machines: [Machine; N] = std::array::from_fn(|_| Machine::new());
    let keys: Vec<PublicKey> = machines.iter().map(Machine::key).collect();
    for (machine, port) in machines.iter().zip(flottille_link::relays_in_memory(&keys)) {
        machine.endpoint.through(port);
    }
    machines
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
async fn two_endpoints_that_trust_each_other_link_through_the_relay() {
    let [laptop, server] = machines().await;
    let ours = Hello::ours("0.4.1");
    let serving = serve(&server, laptop.key(), ours.clone());

    let link = tokio::time::timeout(
        WAIT,
        laptop
            .endpoint
            .connect(server.key(), Traffic::Machine, &ours, "server"),
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
    let told = Message::Machine(MachineMessage::Sessions(Default::default()));
    let answer = link.send(&told).await.unwrap();
    assert_eq!(answer, serde_json::to_value(&told).unwrap());
}

#[tokio::test]
async fn a_key_that_is_not_trusted_is_refused() {
    let [laptop, server, stranger] = machines().await;
    let ours = Hello::ours("0.4.1");
    // The server trusts the laptop only.
    let serving = serve(&server, laptop.key(), ours.clone());

    let refused = tokio::time::timeout(
        WAIT,
        stranger
            .endpoint
            .connect(server.key(), Traffic::Machine, &ours, "server"),
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
    // A hub that hands what is for `other` to the server: the server's endpoint goes through
    // the port that stands for `other`.
    let (laptop, server, other) = (Machine::new(), Machine::new(), Machine::new());
    let mut ports = flottille_link::relays_in_memory(&[laptop.key(), other.key()]).into_iter();
    laptop.endpoint.through(ports.next().unwrap());
    server.endpoint.through(ports.next().unwrap());
    let ours = Hello::ours("0.4.1");
    let _serving = serve(&server, laptop.key(), ours.clone());

    let failed = tokio::time::timeout(
        Duration::from_secs(30),
        laptop
            .endpoint
            .connect(other.key(), Traffic::Machine, &ours, "other"),
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

/// Dials `server` until it links; how long it took.
async fn link_timed(laptop: &Machine, server: &Machine, ours: &Hello) -> (Link, Duration) {
    let started = Instant::now();
    let link = tokio::time::timeout(
        WAIT,
        laptop
            .endpoint
            .connect(server.key(), Traffic::Machine, ours, "server"),
    )
    .await
    .expect("the trusted pair links in time")
    .expect("the trusted pair links");
    (link, started.elapsed())
}

#[tokio::test]
async fn a_flood_of_untrusted_keys_does_not_stop_a_trusted_pair() {
    let all: [Machine; 22] = machines().await;
    let mut all = all.into_iter();
    let (laptop, server) = (all.next().unwrap(), all.next().unwrap());
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
    for stranger in all {
        let (key, ours) = (server.key(), ours.clone());
        strangers.push(tokio::spawn(async move {
            loop {
                let _ = stranger
                    .endpoint
                    .connect(key, Traffic::Machine, &ours, "server")
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
async fn no_common_protocol_refuses_the_link_on_both_sides() {
    let [laptop, server] = machines().await;
    let serving = serve(&server, laptop.key(), hello("0.4.3", &[9]));

    let refused = tokio::time::timeout(
        WAIT,
        laptop.endpoint.connect(
            server.key(),
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
    let [laptop, server] = machines().await;
    let ours = Hello::ours("0.4.1");
    let machine_message = Message::Machine(MachineMessage::Sessions(Default::default()));
    let client_message = Message::Client(json!({"id": 1, "method": "daemon.status"}));

    // A client link: a client's request goes through, a machine's message is refused.
    let serving = serve(&server, laptop.key(), ours.clone());
    let client = laptop
        .endpoint
        .connect(server.key(), Traffic::Client, &ours, "server")
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
        .connect(server.key(), Traffic::Machine, &ours, "server")
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
async fn the_endpoint_reaches_nothing_but_through_the_hub() {
    // With no session to the hub, nothing: no socket is opened, no address looked up.
    let (machine, other) = (Machine::new(), Machine::new());
    let started = Instant::now();
    let failed = machine
        .endpoint
        .connect(
            other.key(),
            Traffic::Machine,
            &Hello::ours("0.4.1"),
            "other",
        )
        .await
        .unwrap_err();
    assert!(matches!(failed, LinkError::NoHub(_)), "{failed}");
    assert_eq!(
        failed.to_string(),
        "other cannot be reached: this device has no session to the hub"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    let accepted =
        tokio::time::timeout(Duration::from_millis(200), machine.endpoint.accept()).await;
    assert!(
        accepted.is_err(),
        "a link came in with no session to the hub"
    );

    // Through the hub, a key the hub does not say online is not dialled.
    let [linked, _] = machines().await;
    let failed = linked
        .endpoint
        .connect(
            other.key(),
            Traffic::Machine,
            &Hello::ours("0.4.1"),
            "other",
        )
        .await
        .unwrap_err();
    assert_eq!(
        failed.to_string(),
        "other is offline: it is not connected to the hub"
    );
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
