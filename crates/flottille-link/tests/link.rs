//! The link between two machines (`docs/remote.md`, "Architecture", "The transport", "The
//! version"): two endpoints in one test, each with its own key in a temporary folder, on
//! 127.0.0.1 only. Flottille handles no network channel: no relay, no NAT traversal, no
//! discovery; a machine dials the addresses it is given for a key.

use std::net::SocketAddr;
use std::path::PathBuf;
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
        let state = tempfile::tempdir().unwrap();
        let key = MachineKey::load_or_make(&state.path().join("remote")).unwrap();
        let endpoint = Endpoint::bind(&key, loopback()).unwrap();
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
    .unwrap();
    assert!(failed.is_err(), "a link to the wrong key was made");
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
