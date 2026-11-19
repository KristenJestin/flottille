//! A tunnel's connections on a link (`docs/remote.md`, "Tunnels"): raw bytes both ways on a
//! stream of a client link, each direction ending alone, and refused on a machine link.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use flottille_link::{Endpoint, Hello, Link, MachineKey, Traffic};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const WAIT: Duration = Duration::from_secs(20);

struct Machine {
    _state: tempfile::TempDir,
    endpoint: Endpoint,
}

impl Machine {
    async fn new() -> Machine {
        let state = tempfile::tempdir().unwrap();
        let key = MachineKey::load_or_make(&state.path().join("remote")).unwrap();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let endpoint = Endpoint::bind(&key, SocketAddr::new(ip, 0)).unwrap();
        Machine {
            _state: state,
            endpoint,
        }
    }
}

/// A link from `laptop` to `server` of `traffic`: both ends.
async fn linked(traffic: Traffic) -> (Machine, Machine, Link, Link) {
    let (laptop, server) = (Machine::new().await, Machine::new().await);
    let ours = Hello::ours("0.4.1");
    let endpoint = server.endpoint.clone();
    let hello = ours.clone();
    let accepting = tokio::spawn(async move {
        let accepting = endpoint.accept().await.unwrap();
        accepting.handshake(&hello, "laptop").await.unwrap()
    });
    let link = tokio::time::timeout(
        WAIT,
        laptop.endpoint.connect(
            server.endpoint.key(),
            &server.endpoint.addresses(),
            traffic,
            &ours,
            "server",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let accepted = accepting.await.unwrap();
    (laptop, server, link, accepted)
}

#[tokio::test]
async fn a_tunnel_connection_carries_bytes_both_ways_and_each_way_ends_alone() {
    let (_laptop, _server, link, accepted) = linked(Traffic::Client).await;
    let serving = tokio::spawn(async move {
        let received = accepted.next().await.unwrap();
        let (id, mut stream) = received.take_tunnel().ok().unwrap();
        assert_eq!(id, "t1");
        // Everything before the client's end, then a reply on the way back.
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.unwrap();
        stream.write_all(&got.to_ascii_uppercase()).await.unwrap();
        stream.shutdown().await.unwrap();
        accepted
    });
    let mut stream = link.tunnel("t1").await.unwrap();
    // More than one frame's limit, which a tunnel is not bound by.
    let body = vec![b'a'; 3 << 20];
    stream.write_all(&body).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back, vec![b'A'; 3 << 20]);
    let _accepted = serving.await.unwrap();
}

#[tokio::test]
async fn a_tunnel_connection_on_a_machine_link_is_closed() {
    let (_laptop, _server, link, accepted) = linked(Traffic::Machine).await;
    // The other side takes no message from it: only what `next` hands out would show it.
    let waiting = tokio::spawn(async move { accepted.next().await.is_some() });
    let mut stream = link.tunnel("t1").await.unwrap();
    let mut back = Vec::new();
    let ended = tokio::time::timeout(WAIT, stream.read_to_end(&mut back)).await;
    assert!(ended.unwrap().is_err() || back.is_empty());
    link.close("done");
    assert!(!waiting.await.unwrap());
}
