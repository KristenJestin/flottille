//! The control session every device keeps to its hub (`docs/remote.md`, "The hub",
//! "Architecture"): the member list and its changes, the members' presence. Both
//! sides are here: the device's, and the handshake the hub answers with, so that the hub and a
//! device cannot disagree on the wire.
//!
//! The hub adds the devices, behind access to its host (`flottille-hub pair`), and signs their
//! entries with its key; every device checks every list it is sent back to the root and the hub's
//! key it pinned (`Device::take`): a change neither the hub nor a member that may make it signed is
//! refused. A hub that answers with another key than the one pinned is refused before anything
//! else is said.
//!
//! On the wire, a device opens a TCP connection (TLS for an `https://` URL), asks for an HTTP/1.1
//! upgrade to `flottille-hub`, so that a reverse proxy in front of the hub carries the session as
//! it carries a WebSocket, then both sides say one JSON object per line:
//!
//! ```text
//! device → hub   {"key": K, "nonce": Nd, "hello": {…}}
//! hub → device   {"hub": H, "nonce": Nh, "hello": {…, "signed": sign_H(hello, K, Nd, Nh, …)},
//!                 "proof": sign_H(session, 1, K, Nd, Nh)}
//! device → hub   {"proof": sign_K(session, 2, H, Nh, Nd), "pinned": P | null, "pair"?: {…}}
//! hub → device   {"welcome": {}}  or  {"refused": {"reason": "…", "reset": true | false,
//!                                                 "proof": sign_H(refused, K, Nd, Nh, P, pair,
//!                                                                 reset, reason)}}
//! then           device → hub {"ask": {"id": n, …}}; hub → device {"answer": {"id": n, …}},
//!                {"list": […]}, {"presence": […]}, {"facts": […]}
//! and            device → hub {"relay": {"to": K', "packet": P}}; hub → device
//!                {"relayed": {"from": K, "packet": P}}, {"online": [K, …]}
//! and each way   {"keepalive": {}}
//! ```
//!
//! `relay` carries the links between the user's devices (`docs/remote.md`, "The transport"): `P`
//! is one QUIC packet of a link, in base64, encrypted end to end between the two devices; the hub
//! hands it to the session of the member whose key is `K'`, saying it is from `K`, and never
//! opens it. `online` is the members' keys whose session is open, sent to members whenever it
//! changes. A side with nothing to say says `keepalive` every `KEEPALIVE`, so that a reverse
//! proxy on the way does not take the session for idle; a side that hears nothing from the other
//! for `SILENCE` takes the session as lost, and so does the hub when a device reads nothing it
//! sends for as long.
//!
//! `session` is `flottille-hub-session-v1` and a zero byte: nothing else a key signs starts so.
//! Each side signs the other's fresh nonce and the other's key, so that a proof cannot be played
//! again, nor sent back to the side that made it. `pinned` is the hash of the first change of the
//! list the device checked, if it has one: a hub that refuses the device says `reset` when its own
//! list starts otherwise, and tells a device nothing else of its list.
//!
//! The welcome or the refusal is said over a connection nothing protects on an `http://` hub. The
//! refusal is a verdict a device acts on (it forgets the hub, or says its list was reset), so the
//! hub signs it over the handshake: `refused` is `flottille-hub-refused-v2` and a zero byte, then
//! the device's key, both nonces, what the hub received of the device's proof, `reset` as one byte
//! and the reason's bytes. What it received is `pinned` (a zero byte when there is none, else a one
//! and the hash) and `pair` (a zero byte, else a one, the pairing's proof and `Proposed::bytes`):
//! the device's signature covers neither, so something on the way could add a `pair` to a member's
//! proof, or change `pinned`, to have the hub refuse it; the device checks the refusal against what
//! it sent, and such a refusal does not check. A refusal whose proof is missing (a hub of before
//! 2026-11-22) or does not check (said by something on the way, answering a request changed on the
//! way, or caught in another session) changes nothing: the device says the hub could not be
//! reached.
//!
//! The hub's hello decides the protocol and whether a side needs an update, so the hub signs it
//! over the handshake too: `signed` is its signature over `flottille-hub-hello-v1` and a zero
//! byte, the device's key, both nonces, then the device's hello as the hub received it and its
//! own, each the length of its version (4 bytes, big-endian), the version, the number of its
//! protocols and each protocol (4 bytes each): a device whose hello was changed on the way does
//! not check it. A hub that speaks no protocol of the
//! device's says, before the handshake, `{"refused": "…", "hello": {…, "signed": …}, "hub": H,
//! "nonce": Nh}`. A device takes the hub's word on the version, that one side needs an update or
//! that a lower protocol is the one both speak, only from a hello its pinned key signed. One whose
//! signature does not check is the hub not reached; one that is not signed (a hub of before
//! 2026-11-22) opens a session only on the device's newest protocol, and is otherwise the hub not
//! reached.
//!
//! A device that pairs (`docs/remote.md`, "Pairing") is no member yet: its proof carries `pair`,
//! the card it proposes (`Proposed`) and its proof that it holds the pair code's secret, which
//! never crosses the network:
//!
//! ```text
//! pair = {"proposed": {key, name, role, icon, colour}, "proof": M}
//! M    = HMAC-SHA256(secret, "flottille-hub-pair-v1" 0x00, H, K, Nd, Nh, proposed)
//! ```
//!
//! where `K` is the session's key (the machine's, for a computer) and `proposed` the bytes
//! `Proposed::bytes` says. The proof is good for this session and this card only: played again
//! in another session, or with another card, it fails. The hub, its pair mode open, checks it,
//! pairs the device (`Action::Pair`, signed with its key) and sends the list; the session then
//! takes the device's declaration of its machine, as a member's.

use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::members::Signature;
use crate::serial::{from_hex_text, invisible, shown, to_hex};
use crate::{
    Change, ChangeHash, Hello, HubKey, ListError, MachineKey, MemberList, Members, NeedsUpdate,
    PairCode, PairSecret, Proposed, PublicKey, choose_hub,
};

/// The port a hub listens on unless told otherwise.
pub const HUB_PORT: u16 = 7448;
/// The protocol name of the HTTP upgrade.
const UPGRADE: &str = "flottille-hub";
/// The start of what each side of a session signs.
const SESSION: &[u8] = b"flottille-hub-session-v1\0";
/// The start of what a device proves with a pair code's secret.
const PAIR: &[u8] = b"flottille-hub-pair-v1\0";
/// The start of what the hub signs when it refuses a device at the end of the handshake.
const REFUSED: &[u8] = b"flottille-hub-refused-v2\0";
/// The start of what the hub signs of its hello.
const HELLO: &[u8] = b"flottille-hub-hello-v1\0";
/// The longest line a device takes from its hub: a list of `MOST_CHANGES + MANAGING_HEADROOM`
/// changes fits.
pub const LINE_LIMIT: usize = 4 << 20;
/// The longest line of a handshake, either side's: some hundred bytes each.
pub const HANDSHAKE_LIMIT: usize = 4 << 10;
/// The longest ask the hub takes: a change.
const ASK_LIMIT: usize = 64 << 10;
/// The longest text of the hub's a device shows.
const SHOWN: usize = 400;
/// The longest HTTP head either side takes.
const HEAD_LIMIT: usize = 8 << 10;
/// How long reaching the hub, or its handshake, may take.
const WAIT: Duration = Duration::from_secs(10);
/// How often each side of a session says something when it has nothing to say: under the idle
/// timeout of the common reverse proxies, 60 s.
pub const KEEPALIVE: Duration = Duration::from_secs(25);
/// A side that hears nothing from the other for this long takes the session as lost: a device its
/// hub's, the hub a device's, which is said offline then rather than left online on a connection
/// that died.
pub const SILENCE: Duration = Duration::from_secs(80);
/// The packets of links waiting in a device's session, each way: past them a packet is dropped,
/// as a network drops one.
const RELAY_WAITING: usize = 256;
/// The longest packet of a link a session carries: QUIC through the hub starts at 1200 bytes, and
/// its discovery of the path's size goes up to 1452.
pub const RELAY_PACKET: usize = 1500;

/// Anything a session runs over: a TCP stream, or TLS over one.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Stream for T {}

type Reader = BufReader<tokio::io::ReadHalf<Box<dyn Stream>>>;
type Writer = tokio::io::WriteHalf<Box<dyn Stream>>;

/// A hub's URL: `http://` or `https://`, a host, a port (80 or 443 when it says none) and a path,
/// which a reverse proxy may route by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubUrl {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

impl HubUrl {
    pub fn parse(text: &str) -> Result<HubUrl, String> {
        let refused = || {
            format!(
                "{text} is not a hub URL: give http:// or https://, a host and its port \
                 (https://hub.example.net, http://192.0.2.10:{HUB_PORT})"
            )
        };
        if text.len() > 2048
            || text
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || invisible(c))
        {
            return Err(refused());
        }
        let (tls, rest) = if let Some(rest) = text.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = text.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(refused());
        };
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        if path.contains(['?', '#']) || authority.contains('@') {
            return Err(refused());
        }
        let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
            let (host, after) = inner.split_once(']').ok_or_else(refused)?;
            host.parse::<std::net::Ipv6Addr>().map_err(|_| refused())?;
            (format!("[{host}]"), after.strip_prefix(':'))
        } else {
            match authority.split_once(':') {
                Some((host, port)) => (host.to_owned(), Some(port)),
                None => (authority.to_owned(), None),
            }
        };
        let port = match port {
            Some(port) => port
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(refused)?,
            None if tls => 443,
            None => 80,
        };
        let name_ok = host.starts_with('[')
            || (!host.is_empty()
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'));
        if !name_ok {
            return Err(refused());
        }
        Ok(HubUrl {
            tls,
            host: host.to_ascii_lowercase(),
            port,
            path: path.to_owned(),
        })
    }

    /// `host:port`, which is reached: the port is always there.
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// The host, with its port only when it is not the scheme's own (80, 443): how a sentence
    /// names the hub.
    pub fn host_shown(&self) -> String {
        let own = if self.tls { 443 } else { 80 };
        if self.port == own {
            self.host.clone()
        } else {
            self.authority()
        }
    }

    /// The URL as a user writes it: `https://hub.example.net`, the port only when it is not the
    /// scheme's own and the path only when there is one. What is stored and sent is
    /// [`Display`](fmt::Display)'s, whole.
    pub fn shown(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        let path = if self.path == "/" { "" } else { &self.path };
        format!("{scheme}://{}{path}", self.host_shown())
    }

    /// `text` as [`HubUrl::shown`] says it, or as it is when it is no hub URL.
    pub fn shown_text(text: &str) -> String {
        HubUrl::parse(text).map_or_else(|_| text.to_owned(), |url| url.shown())
    }

    /// The host as TLS names it: an IPv6 address without its brackets.
    fn tls_name(&self) -> &str {
        self.host.trim_start_matches('[').trim_end_matches(']')
    }
}

impl fmt::Display for HubUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = if self.tls { "https" } else { "http" };
        write!(f, "{scheme}://{}{}", self.authority(), self.path)
    }
}

/// The certificates a device trusts for an `https://` hub: the system's, or those a test gives.
#[derive(Clone, Debug)]
pub enum Tls {
    System,
    /// Certificates in DER: those only.
    Roots(Vec<Vec<u8>>),
}

/// Why a session to the hub could not be made, or ended: each a sentence for the user.
#[derive(Debug)]
pub enum ControlError {
    /// The hub could not be reached: which address was tried, and how it failed.
    Unreachable {
        hub: String,
        why: String,
    },
    /// The hub answered with another key than the one this device pinned.
    AnotherKey {
        hub: String,
    },
    /// The hub's member list was reset: its root is not the one this device pinned.
    Reset,
    /// The hub refused this device, or one of its asks.
    Refused {
        hub: String,
        reason: String,
    },
    NeedsUpdate(NeedsUpdate),
    /// The hub answered that this device is not one of its members (it was removed, or the hub
    /// is another one): what a client offers to pair again for.
    NotMember {
        hub: String,
        reason: String,
    },
    /// The hub took a change and sent back no list that holds it: it is told in words, and the
    /// device's list is as it was.
    Unconfirmed,
    /// The hub said something that is not the protocol, or the session ended.
    Lost {
        hub: String,
        why: String,
    },
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::Unreachable { hub, why } => {
                write!(f, "the hub at {hub} is unreachable: {why}")
            }
            ControlError::AnotherKey { hub } => write!(
                f,
                "the hub at {hub} has another key than the one this device pinned"
            ),
            ControlError::Reset => ListError::Reset.fmt(f),
            ControlError::Refused { hub, reason } | ControlError::NotMember { hub, reason } => {
                write!(f, "the hub at {hub} refused: {reason}")
            }
            ControlError::NeedsUpdate(refusal) => refusal.fmt(f),
            ControlError::Unconfirmed => write!(
                f,
                "The hub did not confirm this change in {} s: it may be busy, or it refused it. \
                 Nothing was changed; try again once the hub answers.",
                WAIT.as_secs()
            ),
            ControlError::Lost { hub, why } => {
                write!(f, "the session to the hub at {hub} ended: {why}")
            }
        }
    }
}

impl std::error::Error for ControlError {}

/// A device's proof that it holds a pair code's secret, for one session and one card.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PairProof(pub [u8; 32]);

impl fmt::Debug for PairProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PairProof").field(&to_hex(&self.0)).finish()
    }
}

impl Serialize for PairProof {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for PairProof {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<PairProof, D::Error> {
        from_hex_text(deserializer, "a pairing's proof").map(PairProof)
    }
}

/// What both sides of a session saw of its handshake: the keys and the nonces.
#[derive(Clone, Copy, Debug)]
pub struct Transcript {
    pub hub: PublicKey,
    pub device: PublicKey,
    device_nonce: Nonce32,
    hub_nonce: Nonce32,
}

impl Transcript {
    /// The device's proof that it holds `secret`, for the card `proposed`.
    pub fn pair_proof(&self, secret: &PairSecret, proposed: &Proposed) -> PairProof {
        let mut proof = [0u8; 32];
        proof.copy_from_slice(&self.pair_mac(secret, proposed).finalize().into_bytes());
        PairProof(proof)
    }

    /// Whether `proof` is the device's for `secret` and `proposed`, compared in constant time.
    pub fn proves_pair(&self, secret: &PairSecret, proposed: &Proposed, proof: &PairProof) -> bool {
        self.pair_mac(secret, proposed)
            .verify_slice(&proof.0)
            .is_ok()
    }

    /// What the hub signs when it refuses the device of this handshake: bound to its keys and
    /// nonces, so that a refusal caught in one session says nothing in another, and to what the
    /// hub received of the device's proof (`pinned`, `pair`), which the device's own signature
    /// does not cover: a refusal answering a request changed on the way does not check for the
    /// device that sent another one.
    fn refusal_bytes(
        &self,
        pinned: Option<ChangeHash>,
        pair: Option<&Pairing>,
        reason: &str,
        reset: bool,
    ) -> Vec<u8> {
        let mut bytes = REFUSED.to_vec();
        bytes.extend_from_slice(self.device.as_bytes());
        bytes.extend_from_slice(&self.device_nonce.0);
        bytes.extend_from_slice(&self.hub_nonce.0);
        match pinned {
            Some(pinned) => {
                bytes.push(1);
                bytes.extend_from_slice(&pinned.0);
            }
            None => bytes.push(0),
        }
        match pair {
            // `Proposed::bytes` says its own length, so the reason after it reads one way only.
            Some(pairing) => {
                bytes.push(1);
                bytes.extend_from_slice(&pairing.proof.0);
                bytes.extend_from_slice(&pairing.proposed.bytes());
            }
            None => bytes.push(0),
        }
        bytes.push(u8::from(reset));
        bytes.extend_from_slice(reason.as_bytes());
        bytes
    }

    /// What the hub signs of its hello: bound to this handshake, so that a hello caught in one
    /// session says nothing in another, and to the device's hello as the hub received it, which
    /// the hub chose the protocol from: a device whose hello was changed on the way does not
    /// check it.
    fn hello_bytes(&self, device: &Hello, hub: &Hello) -> Vec<u8> {
        let mut bytes = HELLO.to_vec();
        bytes.extend_from_slice(self.device.as_bytes());
        bytes.extend_from_slice(&self.device_nonce.0);
        bytes.extend_from_slice(&self.hub_nonce.0);
        // Each said with its lengths, so that the two read one way only.
        let length = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes();
        for hello in [device, hub] {
            let version = hello.flottille.as_bytes();
            bytes.extend_from_slice(&length(version.len()));
            bytes.extend_from_slice(version);
            bytes.extend_from_slice(&length(hello.protocols.len()));
            for protocol in &hello.protocols {
                bytes.extend_from_slice(&protocol.to_be_bytes());
            }
        }
        bytes
    }

    /// The hub's hello `hub`, signed over this handshake and the device's hello `device` with
    /// `key`.
    fn signed_hello(&self, key: &SigningKey, device: &Hello, hub: &Hello) -> SignedHello {
        SignedHello {
            hello: hub.clone(),
            signed: Some(sign(key, &self.hello_bytes(device, hub))),
        }
    }

    /// Whether the hub of this handshake signed `hello`, answering the device's hello `device`.
    fn hub_said(&self, device: &Hello, hello: &SignedHello) -> bool {
        hello.signed.is_some_and(|signed| {
            verify(&self.hub, &self.hello_bytes(device, &hello.hello), &signed)
        })
    }

    fn pair_mac(&self, secret: &PairSecret, proposed: &Proposed) -> Hmac<Sha256> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&secret.0).expect("HMAC takes a key of any length");
        mac.update(PAIR);
        mac.update(self.hub.as_bytes());
        mac.update(self.device.as_bytes());
        mac.update(&self.device_nonce.0);
        mac.update(&self.hub_nonce.0);
        mac.update(&proposed.bytes());
        mac
    }
}

/// 32 random bytes one side gives the other to sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nonce32(pub [u8; 32]);

impl Nonce32 {
    fn new() -> Result<Nonce32, String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| format!("no random bytes: {error}"))?;
        Ok(Nonce32(bytes))
    }
}

impl Serialize for Nonce32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Nonce32 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Nonce32, D::Error> {
        from_hex_text(deserializer, "a nonce").map(Nonce32)
    }
}

/// The device's first line.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceHello {
    key: PublicKey,
    nonce: Nonce32,
    hello: Hello,
}

/// The hub's answer to it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HubHello {
    hub: PublicKey,
    nonce: Nonce32,
    hello: SignedHello,
    proof: Signature,
}

/// The hub's hello with its signature over the handshake (`Transcript::hello_bytes`), beside the
/// fields a device of before reads, which ignores it; a hub of before signs none.
#[derive(Clone, Serialize, Deserialize)]
struct SignedHello {
    #[serde(flatten)]
    hello: Hello,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signed: Option<Signature>,
}

/// The hub's refusal before the handshake, when it speaks no protocol of the device's, beside its
/// `refused`: its hello, and the key and nonce it signed it with (a hub of before says neither).
#[derive(Deserialize)]
struct RefusedHello {
    hello: SignedHello,
    hub: PublicKey,
    nonce: Nonce32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceProof {
    proof: Signature,
    /// The hash of the first change of the list the device checked, when it has one.
    #[serde(default)]
    pinned: Option<ChangeHash>,
    /// The pairing of a device that is no member yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pair: Option<Pairing>,
}

/// What a device that pairs says in its proof: the card it proposes, and its proof that it holds
/// the pair code's secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pairing {
    pub proposed: Proposed,
    pub proof: PairProof,
}

/// What a device asks the hub, once in a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// A few asks a session, each sent at once: their size costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
pub enum Ask {
    /// A member's change of the list.
    Submit { change: Change },
    /// What this device says of itself: what it is (`computer`, `phone`) and its system
    /// (`linux`).
    About { device: String, system: String },
    /// The addresses a machine of the previous version tells, for the direct link removed on
    /// 2026-11-21: answered and ignored, so that such a machine keeps its session while it waits
    /// for its update. This version never asks it.
    Addresses { addresses: Vec<String> },
    /// Whether a client of this device is active, and when its last input was.
    Presence { active: bool, last_input: u64 },
}

/// An ask and its number, which its answer names.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asked {
    pub id: u64,
    #[serde(flatten)]
    pub ask: Ask,
}

/// Whether a member's client is active, as it told the hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceOf {
    pub key: PublicKey,
    pub active: bool,
    pub last_input: u64,
}

/// What the hub knows of a member's device besides its entry (`docs/remote.md`, "A device's
/// card"): what the device says of itself, and when it was paired and last seen, in seconds since
/// 1970. Unsigned: nothing in it gives a right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    pub key: PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
}

/// What the hub sends a device once the handshake is done.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToDevice {
    /// The session is a member's, or a pairing's.
    Welcome {},
    /// The device is not one of the members; `reset` when the list it pinned is not the hub's
    /// any more, the hub having been set up again. At the end of the handshake, `proof` is the
    /// hub's signature of it over the handshake (`Transcript::refusal_bytes`); a hub of before
    /// 2026-11-22 sends none.
    Refused {
        reason: String,
        #[serde(default)]
        reset: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proof: Option<Signature>,
    },
    Answer {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refused: Option<String>,
    },
    /// The member list, whole, as the members signed it.
    List(MemberList),
    /// The addresses of the members' machines, for the direct link removed on 2026-11-21: a hub
    /// of before sends those its members told; this version's hub sends an empty list, once a
    /// session, which a phone's core of before waits for before it links. Read and ignored.
    Addresses(serde_json::Value),
    /// The presence of the members' clients, for a runner.
    Presence(Vec<PresenceOf>),
    /// What the hub knows of each member's device, in place of what it sent before.
    Facts(Vec<Facts>),
    /// A packet of a link, as the device of `from` sent it for this one.
    Relayed { from: PublicKey, packet: LinkPacket },
    /// The members' keys whose session to the hub is open now: the devices a link reaches.
    Online(Vec<PublicKey>),
    /// Nothing to say: the session is alive.
    Keepalive {},
}

/// A packet of a link, in base64 in a session's line.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkPacket(pub Vec<u8>);

impl fmt::Debug for LinkPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LinkPacket({} bytes)", self.0.len())
    }
}

impl Serialize for LinkPacket {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64::Engine;
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for LinkPacket {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<LinkPacket, D::Error> {
        use base64::Engine;
        let text = String::deserialize(deserializer)?;
        if text.len() > RELAY_PACKET.div_ceil(3) * 4 {
            return Err(serde::de::Error::custom("a packet is too long"));
        }
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map(LinkPacket)
            .map_err(serde::de::Error::custom)
    }
}

/// What a device sends the hub for another device: a packet of a link, and the key it is for.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayFrame {
    #[serde(serialize_with = "key_bytes", deserialize_with = "key_of_hex")]
    to: [u8; 32],
    packet: LinkPacket,
}

fn key_bytes<S: Serializer>(key: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&to_hex(key))
}

fn key_of_hex<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
    from_hex_text::<_, 32>(deserializer, "a key")
}

/// A device's end of the links its session to the hub carries (`Session::relay`): the packets it
/// sends other devices and those they send it, and the members online. It ends with the session.
pub struct RelayPort {
    out: mpsc::Sender<crate::transport::Packet>,
    incoming: mpsc::Receiver<crate::transport::Packet>,
    online: watch::Receiver<Vec<PublicKey>>,
}

impl fmt::Debug for RelayPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RelayPort")
    }
}

impl RelayPort {
    /// Sends `packet` to the device of `to` through the hub, once there is room for it: whether
    /// the session took it.
    pub async fn send(&self, to: &PublicKey, packet: Vec<u8>) -> bool {
        self.out.send((*to.as_bytes(), packet)).await.is_ok()
    }

    /// Sends `packet` to the device of `to` if there is room for it now: whether it was taken.
    pub fn try_send(&self, to: &PublicKey, packet: Vec<u8>) -> bool {
        self.out.try_send((*to.as_bytes(), packet)).is_ok()
    }

    /// The next packet another device sent this one, and its key; `None` once the session ended.
    pub async fn recv(&mut self) -> Option<(PublicKey, Vec<u8>)> {
        loop {
            let (from, packet) = self.incoming.recv().await?;
            if let Ok(from) = PublicKey::from_bytes(&from) {
                return Some((from, packet));
            }
        }
    }

    /// The members' keys whose session to the hub is open, as the hub last said.
    pub fn online(&self) -> Vec<PublicKey> {
        self.online.borrow().clone()
    }

    /// Until the hub says another list of the members online; never, once the session ended.
    pub async fn online_changed(&mut self) {
        if self.online.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// The packets out, the packets in, and the members online, for the transport.
    pub(crate) fn split(
        self,
    ) -> (
        mpsc::Sender<crate::transport::Packet>,
        mpsc::Receiver<crate::transport::Packet>,
        watch::Receiver<Vec<PublicKey>>,
    ) {
        (self.out, self.incoming, self.online)
    }
}

/// The session's end of its relay port.
struct Carried {
    out: mpsc::Receiver<crate::transport::Packet>,
    incoming: mpsc::Sender<crate::transport::Packet>,
    online: watch::Sender<Vec<PublicKey>>,
}

/// Relay ports joined in memory as a hub joins its members' sessions, one for each of `keys`, which
/// it stands for: a packet sent to a key goes to its port, said to be from the port's key, and
/// each port sees every key online. For the tests of a link, which run no hub; the ports end when
/// all of them are dropped.
#[doc(hidden)]
pub fn relays_in_memory(keys: &[PublicKey]) -> Vec<RelayPort> {
    let (ports, carried): (Vec<RelayPort>, Vec<Carried>) =
        keys.iter().map(|_| relay_ends()).unzip();
    let incoming: Vec<_> = carried.iter().map(|ends| ends.incoming.clone()).collect();
    for (from, ends) in keys.iter().zip(carried) {
        ends.online.send_replace(keys.to_vec());
        let (from, keys, incoming) = (*from.as_bytes(), keys.to_vec(), incoming.clone());
        tokio::spawn(async move {
            let Carried {
                mut out, online, ..
            } = ends;
            while let Some((to, packet)) = out.recv().await {
                if let Some(at) = keys.iter().position(|key| key.as_bytes() == &to) {
                    let _ = incoming[at].try_send((from, packet));
                }
            }
            drop(online);
        });
    }
    ports
}

fn relay_ends() -> (RelayPort, Carried) {
    let (out, out_taken) = mpsc::channel(RELAY_WAITING);
    let (incoming_given, incoming) = mpsc::channel(RELAY_WAITING);
    let (online_given, online) = watch::channel(Vec::new());
    (
        RelayPort {
            out,
            incoming,
            online,
        },
        Carried {
            out: out_taken,
            incoming: incoming_given,
            online: online_given,
        },
    )
}

/// What the hub pushes to a device, as the session hands it over.
#[derive(Clone, Debug)]
pub enum Pushed {
    List(MemberList),
    Presence(Vec<PresenceOf>),
    Facts(Vec<Facts>),
}

/// What one side signs: the domain, whose proof it is, the other's key, then the other's nonce
/// and its own.
fn proof_bytes(side: u8, other: &PublicKey, theirs: &Nonce32, own: &Nonce32) -> Vec<u8> {
    let mut bytes = SESSION.to_vec();
    bytes.push(side);
    bytes.extend_from_slice(other.as_bytes());
    bytes.extend_from_slice(&theirs.0);
    bytes.extend_from_slice(&own.0);
    bytes
}

fn sign(key: &SigningKey, bytes: &[u8]) -> Signature {
    Signature(key.sign(bytes).to_bytes())
}

fn verify(key: &PublicKey, bytes: &[u8], signature: &Signature) -> bool {
    key.0
        .verify_strict(bytes, &ed25519_dalek::Signature::from_bytes(&signature.0))
        .is_ok()
}

/// One line, at most `limit` bytes; `None` at the end of the stream.
async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(std::io::ErrorKind::UnexpectedEof.into())
            };
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(at) => {
                line.extend_from_slice(&available[..at]);
                reader.consume(at + 1);
                return Ok(Some(line));
            }
            None => {
                let taken = available.len();
                line.extend_from_slice(available);
                reader.consume(taken);
            }
        }
        if line.len() > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "a line is too long",
            ));
        }
    }
}

async fn write_line(writer: &mut Writer, value: &impl Serialize) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(value).expect("a frame is JSON");
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// The next line as `T`, at most `limit` bytes, or why not.
async fn read_frame<T: for<'de> Deserialize<'de>>(
    reader: &mut Reader,
    limit: usize,
) -> Result<T, String> {
    match read_line(reader, limit).await {
        Ok(Some(line)) => serde_json::from_slice(&line)
            .map_err(|_| "it said what is not Flottille's hub protocol".to_owned()),
        Ok(None) => Err("it closed the connection".to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

// ----------------------------------------------------------------------------------------------
// The device's side.

/// What a device keeps of its hub, pinned when it paired: the hub's URL and key, the root of the
/// member list, and the list as this device last checked it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub url: String,
    pub hub: PublicKey,
    pub root: PublicKey,
    #[serde(default)]
    pub list: MemberList,
}

impl Device {
    /// The members of the list this device last checked.
    pub fn members(&self) -> Result<Members, ListError> {
        self.list.check(&self.root, &self.hub)
    }

    /// A list the hub sent: kept when every change checks back to the root and the hub's key this
    /// device pinned and it holds every change this device checked already; refused otherwise,
    /// the list kept as it was. A hub that forges a member's change, drops, reorders or plays a
    /// change again, or was reset, is refused here, whatever the hub itself checked.
    pub fn take(&mut self, list: MemberList) -> Result<Members, ListError> {
        let members = list.check_update(&self.root, &self.hub, &self.list)?;
        self.list = list;
        Ok(members)
    }

    /// A member's change, asked through `session`: kept once the hub sent back a list that holds
    /// it, which this device checked. A hub that says yes and withholds it is said to.
    pub async fn submit(
        &mut self,
        session: &mut Session,
        change: Change,
    ) -> Result<Members, ControlError> {
        let hash = change.hash();
        session.ask(Ask::Submit { change }).await?;
        self.sent_back(session, &hash)
            .await
            .map_err(|why| match why {
                None => ControlError::Unconfirmed,
                Some(reason) => ControlError::Refused {
                    hub: session.authority.clone(),
                    reason,
                },
            })
    }

    /// Takes the lists `session` sends until one holds the change whose hash is `hash`; why not
    /// when one is refused, `None` when none comes in time.
    async fn sent_back(
        &mut self,
        session: &mut Session,
        hash: &ChangeHash,
    ) -> Result<Members, Option<String>> {
        let waited = tokio::time::timeout(WAIT, async {
            loop {
                let Some(pushed) = session.next().await else {
                    return Err(Some(session.ended()));
                };
                let Pushed::List(list) = pushed else {
                    continue;
                };
                let holds = list.changes().iter().any(|change| change.hash() == *hash);
                let members = self.take(list).map_err(|error| Some(error.to_string()))?;
                if holds {
                    return Ok(members);
                }
            }
        });
        waited.await.unwrap_or(Err(None))
    }

    /// A session to the hub this device pinned, opened by its machine. A hub that answers with
    /// another key, or whose list's root is not the pinned one, is refused.
    pub async fn connect(
        &self,
        machine: &MachineKey,
        hello: &Hello,
        tls: &Tls,
    ) -> Result<Session, ControlError> {
        let url = HubUrl::parse(&self.url).map_err(|why| ControlError::Refused {
            hub: self.url.clone(),
            reason: why,
        })?;
        let pinned = self.list.changes().first().map(Change::hash);
        let session =
            Session::open(&url, Some(&self.hub), &machine.0, hello, tls, pinned, None).await?;
        match session.refused {
            Some((_, true)) => Err(ControlError::Reset),
            Some((reason, false)) => Err(ControlError::NotMember {
                hub: url.host_shown(),
                reason,
            }),
            None => Ok(session),
        }
    }

    /// A device's side of a pairing (`docs/remote.md`, "Pairing"): `key` (the machine's, for a
    /// computer; the device's own, for a phone) reaches the hub of `code`, pinning the key the code
    /// holds, and proves it holds the code's secret for the card `proposed`. The hub pairs the
    /// device and sends the list (`Paired::listed`).
    pub async fn pair(
        code: &PairCode,
        proposed: Proposed,
        machine: &MachineKey,
        hello: &Hello,
        tls: &Tls,
    ) -> Result<Paired, ControlError> {
        let url = HubUrl::parse(&code.hub_url).map_err(|why| ControlError::Refused {
            hub: code.hub_url.clone(),
            reason: why,
        })?;
        proposed.check().map_err(|why| ControlError::Refused {
            hub: url.host_shown(),
            reason: why,
        })?;
        let pairing = Pairing {
            proposed: proposed.clone(),
            proof: PairProof([0; 32]),
        };
        let session = Session::open(
            &url,
            Some(&code.hub_key),
            &machine.0,
            hello,
            tls,
            None,
            Some((pairing, code.secret)),
        )
        .await?;
        if let Some((reason, _)) = &session.refused {
            return Err(ControlError::Refused {
                hub: url.host_shown(),
                reason: reason.clone(),
            });
        }
        Ok(Paired {
            hub: code.hub_key,
            key: proposed.key,
            url: url.to_string(),
            session,
        })
    }
}

/// A pairing the hub took, its session open.
#[derive(Debug)]
pub struct Paired {
    hub: PublicKey,
    key: PublicKey,
    url: String,
    pub session: Session,
}

impl Paired {
    /// The first list the hub sends once it paired the device: it checks back to its own root and
    /// to the hub's key the code holds, which the device pins from then on, and holds the device as
    /// a member. A list that does not is refused, and so is the pairing.
    pub async fn listed(&mut self) -> Result<(Device, Members), ControlError> {
        let authority = self.session.authority.clone();
        let refused = |reason: String| ControlError::Refused {
            hub: authority.clone(),
            reason,
        };
        let waited = tokio::time::timeout(WAIT, async {
            loop {
                let Some(pushed) = self.session.next().await else {
                    return Err(refused(self.session.ended_why()));
                };
                let Pushed::List(list) = pushed else {
                    continue;
                };
                let root = list
                    .root()
                    .ok_or_else(|| refused("it sent an empty member list".to_owned()))?;
                let members = list
                    .check(&root, &self.hub)
                    .map_err(|error| refused(format!("its member list does not check: {error}")))?;
                if members.get(&self.key).is_none() {
                    return Err(refused(
                        "it sent a member list that does not hold this device".to_owned(),
                    ));
                }
                let device = Device {
                    url: self.url.clone(),
                    hub: self.hub,
                    root,
                    list,
                };
                return Ok((device, members));
            }
        });
        waited.await.unwrap_or_else(|_| {
            Err(refused(format!(
                "it sent no member list in {} s",
                WAIT.as_secs()
            )))
        })
    }
}

type Answered = oneshot::Sender<Result<(), String>>;

/// A session to the hub, its handshake done.
pub struct Session {
    hub: PublicKey,
    authority: String,
    protocol: u32,
    asks: mpsc::UnboundedSender<(Ask, Answered)>,
    pushed: mpsc::UnboundedReceiver<Pushed>,
    /// Why the session ended, once it did.
    ended: Arc<Mutex<Option<String>>>,
    /// When the hub refused the device: its reason, and whether the list the device pinned was
    /// reset.
    refused: Option<(String, bool)>,
    /// The device's end of the links the session carries, until it is taken.
    relay: Option<RelayPort>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("hub", &self.hub)
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

/// What asks the hub in a session, apart from it: a task may wait for an answer while the session
/// goes on.
#[derive(Clone)]
pub struct Asker {
    authority: String,
    asks: mpsc::UnboundedSender<(Ask, Answered)>,
    ended: Arc<Mutex<Option<String>>>,
}

impl Asker {
    /// Asks the hub, and waits for its answer.
    pub async fn ask(&self, ask: Ask) -> Result<(), ControlError> {
        let (answer, answered) = oneshot::channel();
        if self.asks.send((ask, answer)).is_err() {
            return Err(lost(&self.authority, &self.ended));
        }
        match answered.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(reason)) => Err(ControlError::Refused {
                hub: self.authority.clone(),
                reason,
            }),
            Err(_) => Err(lost(&self.authority, &self.ended)),
        }
    }
}

fn lost(authority: &str, ended: &Mutex<Option<String>>) -> ControlError {
    ControlError::Lost {
        hub: authority.to_owned(),
        why: ended
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "the hub closed it".to_owned()),
    }
}

impl Session {
    async fn open(
        url: &HubUrl,
        pinned: Option<&PublicKey>,
        key: &SigningKey,
        hello: &Hello,
        tls: &Tls,
        pinned_list: Option<ChangeHash>,
        pair: Option<(Pairing, PairSecret)>,
    ) -> Result<Session, ControlError> {
        let authority = url.authority();
        let stream = reach(url, tls).await?;
        let lost = |why: String| ControlError::Lost {
            hub: authority.clone(),
            why,
        };
        let (read, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);
        let handshake = async {
            upgrade_request(url, &mut reader, &mut writer).await?;
            let own = PublicKey(key.verifying_key());
            let nonce = Nonce32::new().map_err(lost)?;
            write_line(
                &mut writer,
                &DeviceHello {
                    key: own,
                    nonce,
                    hello: hello.clone(),
                },
            )
            .await
            .map_err(|error| lost(error.to_string()))?;
            let answer: serde_json::Value = read_frame(&mut reader, HANDSHAKE_LIMIT)
                .await
                .map_err(lost)?;
            if let Some(reason) = answer.get("refused").and_then(|refused| refused.as_str()) {
                // A hub with no protocol in common says its own hello with its refusal, signed
                // over this handshake: the device says what to update from both, in its own
                // words. Unsigned, or signed by another key, it is no word on the version.
                if let Ok(theirs) = serde_json::from_value::<RefusedHello>(answer.clone())
                    && pinned.is_none_or(|pinned| *pinned == theirs.hub)
                    && (Transcript {
                        hub: theirs.hub,
                        device: own,
                        device_nonce: nonce,
                        hub_nonce: theirs.nonce,
                    })
                    .hub_said(hello, &theirs.hello)
                    && let Err(refusal) = choose_hub(hello, &theirs.hello.hello)
                {
                    return Err(ControlError::NeedsUpdate(refusal));
                }
                // Said before any key is proved, so by anything on the way to an `http://` hub:
                // no verdict, and its words are said as what answered, not as the hub's.
                return Err(ControlError::Unreachable {
                    hub: authority.clone(),
                    why: format!(
                        "what answered refused this device before the handshake, unsigned, \
                         saying \"{}\": the hub could not be reached",
                        shown(reason, SHOWN)
                    ),
                });
            }
            let theirs: HubHello = serde_json::from_value(answer)
                .map_err(|_| lost("it said what is not Flottille's hub protocol".to_owned()))?;
            if pinned.is_some_and(|pinned| *pinned != theirs.hub) {
                return Err(ControlError::AnotherKey {
                    hub: authority.clone(),
                });
            }
            if !verify(
                &theirs.hub,
                &proof_bytes(1, &own, &nonce, &theirs.nonce),
                &theirs.proof,
            ) {
                return Err(lost(
                    "it did not prove it holds the key it showed".to_owned(),
                ));
            }
            let proof = sign(key, &proof_bytes(2, &theirs.hub, &theirs.nonce, &nonce));
            let transcript = Transcript {
                hub: theirs.hub,
                device: own,
                device_nonce: nonce,
                hub_nonce: theirs.nonce,
            };
            // The hub's word that one side needs an update, or that a lower protocol than this
            // device's newest is the one both speak, is taken only when the hub signed it:
            // anything on the way to an `http://` hub can add a protocol to its hello or take one
            // out. A signature that does not check says the hub's hello, or this device's as the
            // hub received it, was changed on the way. A hello the hub did not sign (a hub of
            // before 2026-11-22) is taken for this device's newest protocol only.
            let offered = &theirs.hello.hello.protocols;
            let unreachable = |why: String| ControlError::Unreachable {
                hub: authority.clone(),
                why,
            };
            if theirs.hello.signed.is_some() && !transcript.hub_said(hello, &theirs.hello) {
                return Err(unreachable(
                    "the hub's signature of its hello, or of this machine's as it received it, \
                     does not check, as when something on the way to it changes one: the hub \
                     could not be reached"
                        .to_owned(),
                ));
            }
            if theirs.hello.signed.is_none() && !offered.contains(&hello.newest()) {
                return Err(unreachable(format!(
                    "its hello offers protocols {offered:?} without the hub's signature, as an \
                     older flottille-hub or something on the way to it does, and this machine \
                     speaks {:?}: the hub could not be reached (an older hub is updated on its \
                     host)",
                    hello.protocols
                )));
            }
            let protocol =
                choose_hub(hello, &theirs.hello.hello).map_err(ControlError::NeedsUpdate)?;
            let pair = pair.map(|(mut pairing, secret)| {
                pairing.proof = transcript.pair_proof(&secret, &pairing.proposed);
                pairing
            });
            write_line(
                &mut writer,
                &DeviceProof {
                    proof,
                    pinned: pinned_list,
                    pair: pair.clone(),
                },
            )
            .await
            .map_err(|error| lost(error.to_string()))?;
            let welcome: ToDevice = read_frame(&mut reader, HANDSHAKE_LIMIT)
                .await
                .map_err(lost)?;
            Ok((transcript, protocol, welcome, pair))
        };
        let (transcript, protocol, welcome, pair) =
            tokio::time::timeout(WAIT, handshake).await.map_err(|_| {
                lost(format!(
                    "it did not finish its handshake in {} s",
                    WAIT.as_secs()
                ))
            })??;
        let (asks, asked) = mpsc::unbounded_channel();
        let (pushing, pushed) = mpsc::unbounded_channel();
        let ended = Arc::new(Mutex::new(None));
        let (relay, carried) = relay_ends();
        let refused = match welcome {
            ToDevice::Welcome {} => {
                tokio::spawn(drive(
                    reader,
                    writer,
                    asked,
                    pushing,
                    carried,
                    ended.clone(),
                ));
                None
            }
            ToDevice::Refused {
                reason,
                reset,
                proof,
            } => {
                // Anything on the way to an `http://` hub can say a refusal, or change what the
                // device asked: only the hub's signature over this handshake and over what this
                // device sent makes it the hub's verdict on this device.
                let signed = proof.is_some_and(|proof| {
                    verify(
                        &transcript.hub,
                        &transcript.refusal_bytes(pinned_list, pair.as_ref(), &reason, reset),
                        &proof,
                    )
                });
                if !signed {
                    return Err(ControlError::Unreachable {
                        hub: authority.clone(),
                        why: "what answered refused this device without the hub's signature, as \
                              an older flottille-hub or something on the way to it does: the hub \
                              could not be reached"
                            .to_owned(),
                    });
                }
                Some((shown(&reason, SHOWN), reset))
            }
            _ => {
                return Err(lost(
                    "it said what is not Flottille's hub protocol".to_owned(),
                ));
            }
        };
        Ok(Session {
            hub: transcript.hub,
            authority,
            protocol,
            asks,
            pushed,
            ended,
            refused,
            relay: Some(relay),
        })
    }

    /// This device's end of the links the session carries to the other devices through the hub:
    /// given once, `None` after.
    pub fn relay(&mut self) -> Option<RelayPort> {
        self.relay.take()
    }

    /// The hub's key, as it proved it.
    pub fn hub(&self) -> PublicKey {
        self.hub
    }

    pub fn protocol(&self) -> u32 {
        self.protocol
    }

    /// Asks the hub, and waits for its answer.
    pub async fn ask(&self, ask: Ask) -> Result<(), ControlError> {
        self.asker().ask(ask).await
    }

    /// What asks the hub apart from this session, for a task that waits for an answer.
    pub fn asker(&self) -> Asker {
        Asker {
            authority: self.authority.clone(),
            asks: self.asks.clone(),
            ended: self.ended.clone(),
        }
    }

    /// The next thing the hub pushes; `None` once the session ended (`ended` says why).
    pub async fn next(&mut self) -> Option<Pushed> {
        self.pushed.recv().await
    }

    /// Whether the session ended already.
    pub fn has_ended(&self) -> bool {
        self.ended.lock().unwrap().is_some()
    }

    /// Why the session ended.
    pub fn ended(&self) -> String {
        lost(&self.authority, &self.ended).to_string()
    }

    /// Why the session ended, as the hub or the connection said it.
    fn ended_why(&self) -> String {
        let why = self.ended.lock().unwrap().clone();
        let why = why.unwrap_or_else(|| "the hub closed it".to_owned());
        why.strip_prefix("it refused: ")
            .map(str::to_owned)
            .unwrap_or(why)
    }
}

/// Reaches the hub: each address its host has, in turn, until one answers; TLS over it for an
/// `https://` URL. A failure names every address tried and how it failed.
async fn reach(url: &HubUrl, tls: &Tls) -> Result<Box<dyn Stream>, ControlError> {
    let authority = url.authority();
    let unreachable = |why: String| ControlError::Unreachable {
        hub: authority.clone(),
        why,
    };
    let host = url.tls_name().to_owned();
    let addresses: Vec<SocketAddr> =
        tokio::time::timeout(WAIT, tokio::net::lookup_host((host.as_str(), url.port)))
            .await
            .map_err(|_| unreachable(format!("{host} was not resolved in {} s", WAIT.as_secs())))?
            .map_err(|error| unreachable(format!("{host} was not resolved: {error}")))?
            .collect();
    let mut tried = Vec::new();
    let mut stream = None;
    for address in addresses {
        match tokio::time::timeout(WAIT, tokio::net::TcpStream::connect(address)).await {
            Ok(Ok(made)) => {
                stream = Some(made);
                break;
            }
            Ok(Err(error)) => tried.push(format!("tried {address}: {error}")),
            Err(_) => tried.push(format!(
                "tried {address}: no answer in {} s",
                WAIT.as_secs()
            )),
        }
    }
    let Some(stream) = stream else {
        return Err(unreachable(if tried.is_empty() {
            format!("{host} has no address")
        } else {
            tried.join("; ")
        }));
    };
    if !url.tls {
        return Ok(Box::new(stream));
    }
    let roots = match tls {
        Tls::System => None,
        Tls::Roots(certificates) => Some(certificates.as_slice()),
    };
    let secured = tokio::time::timeout(WAIT, crate::transport::tls_client(stream, &host, roots))
        .await
        .map_err(|_| unreachable(format!("its TLS did not finish in {} s", WAIT.as_secs())))?
        .map_err(unreachable)?;
    Ok(secured)
}

/// The HTTP upgrade, the device's side.
async fn upgrade_request(
    url: &HubUrl,
    reader: &mut Reader,
    writer: &mut Writer,
) -> Result<(), ControlError> {
    let authority = url.authority();
    let lost = |why: String| ControlError::Lost {
        hub: authority.clone(),
        why,
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: {UPGRADE}\r\n\r\n",
        url.path, authority
    );
    writer
        .write_all(request.as_bytes())
        .await
        .map_err(|error| lost(error.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|error| lost(error.to_string()))?;
    let head = read_head(reader).await.map_err(lost)?;
    let status = head.first().cloned().unwrap_or_default();
    if !status.starts_with("HTTP/1.1 101") {
        return Err(lost(format!(
            "it answered {status:?}, not a flottille-hub's upgrade: is the URL the hub's, and \
             does a proxy in front of it pass upgrades on?"
        )));
    }
    Ok(())
}

/// The lines of an HTTP head, up to the blank line.
async fn read_head(reader: &mut Reader) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut size = 0;
    loop {
        let line = match read_line(reader, HEAD_LIMIT).await {
            Ok(Some(line)) => line,
            Ok(None) => return Err("it closed the connection".to_owned()),
            Err(error) => return Err(error.to_string()),
        };
        size += line.len();
        if size > HEAD_LIMIT {
            return Err("its HTTP head is too long".to_owned());
        }
        let line = String::from_utf8_lossy(&line)
            .trim_end_matches('\r')
            .to_owned();
        if line.is_empty() {
            return Ok(lines);
        }
        lines.push(line);
    }
}

/// Carries a session: the asks out, each answered by its number; what the hub pushes in.
async fn drive(
    mut reader: Reader,
    mut writer: Writer,
    mut asked: mpsc::UnboundedReceiver<(Ask, Answered)>,
    pushing: mpsc::UnboundedSender<Pushed>,
    mut carried: Carried,
    ended: Arc<Mutex<Option<String>>>,
) {
    let (lines_in, mut lines) = mpsc::channel::<Result<ToDevice, String>>(16);
    let reading = tokio::spawn(async move {
        loop {
            let frame = read_frame::<ToDevice>(&mut reader, LINE_LIMIT).await;
            let stop = frame.is_err();
            if lines_in.send(frame).await.is_err() || stop {
                return;
            }
        }
    });
    let mut waiting: Vec<(u64, Answered)> = Vec::new();
    let mut next_id = 1u64;
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = tokio::time::Instant::now();
    let why = loop {
        tokio::select! {
            Some((to, packet)) = carried.out.recv() => {
                let frame = serde_json::json!({"relay": RelayFrame { to, packet: LinkPacket(packet) }});
                if let Err(error) = write_line(&mut writer, &frame).await {
                    break error.to_string();
                }
            }
            _ = keepalive.tick() => {
                if heard.elapsed() > SILENCE {
                    break format!("it said nothing in {} s", SILENCE.as_secs());
                }
                if let Err(error) = write_line(&mut writer, &serde_json::json!({"keepalive": {}})).await {
                    break error.to_string();
                }
            }
            ask = asked.recv() => {
                let Some((ask, answer)) = ask else {
                    break "this device closed it".to_owned();
                };
                let id = next_id;
                next_id += 1;
                if let Err(error) = write_line(&mut writer, &serde_json::json!({"ask": Asked { id, ask }})).await {
                    let _ = answer.send(Err(error.to_string()));
                    break error.to_string();
                }
                // An ask whose asker gave up waiting (a timeout) is answered to no one.
                waiting.retain(|(_, answer)| !answer.is_closed());
                waiting.push((id, answer));
            }
            line = lines.recv() => {
                heard = tokio::time::Instant::now();
                match line {
                    Some(Ok(ToDevice::Relayed { from, packet })) => {
                        // A packet with no room is dropped, as a network drops one.
                        let _ = carried.incoming.try_send((*from.as_bytes(), packet.0));
                    }
                    Some(Ok(ToDevice::Online(keys))) => { carried.online.send_replace(keys); }
                    Some(Ok(ToDevice::Keepalive {})) => {}
                    Some(Ok(ToDevice::Answer { id, refused })) => {
                        if let Some(at) = waiting.iter().position(|(waits, _)| *waits == id) {
                            let (_, answer) = waiting.swap_remove(at);
                            let _ = answer.send(match refused {
                                Some(reason) => Err(shown(&reason, SHOWN)),
                                None => Ok(()),
                            });
                        }
                    }
                    Some(Ok(ToDevice::Facts(facts))) => { let _ = pushing.send(Pushed::Facts(facts)); }
                    Some(Ok(ToDevice::List(list))) => { let _ = pushing.send(Pushed::List(list)); }
                    Some(Ok(ToDevice::Addresses(_))) => {}
                    Some(Ok(ToDevice::Presence(presence))) => { let _ = pushing.send(Pushed::Presence(presence)); }
                    Some(Ok(ToDevice::Refused { reason, .. })) => {
                        break format!("it refused: {}", shown(&reason, SHOWN));
                    }
                    Some(Ok(ToDevice::Welcome {})) => {}
                    Some(Err(why)) => break why,
                    None => break "it closed the connection".to_owned(),
                }
            }
        }
    };
    *ended.lock().unwrap() = Some(why);
    reading.abort();
    let _ = writer.shutdown().await;
}

// ----------------------------------------------------------------------------------------------
// The hub's side.

/// A device that connected to the hub and proved its key: the hub decides whether it is welcome.
pub struct Arriving {
    /// The hub's key, which signs a refusal.
    key: SigningKey,
    transcript: Transcript,
    pinned: Option<ChangeHash>,
    pair: Option<Pairing>,
    version: String,
    protocol: u32,
    reader: Reader,
    writer: Writer,
}

impl Arriving {
    /// The key the device proved it holds.
    pub fn device(&self) -> PublicKey {
        self.transcript.device
    }

    /// The pairing of a device that is no member yet, as it said it: its proof is the hub's to
    /// check (`Transcript::proves_pair`).
    pub fn pairing(&self) -> Option<&Pairing> {
        self.pair.as_ref()
    }

    /// The keys and nonces of the handshake, which a pairing's proof is bound to.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The Flottille version the device said in its hello.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The hash of the first change of the list the device checked, as it said it.
    pub fn pinned(&self) -> Option<ChangeHash> {
        self.pinned
    }

    pub fn protocol(&self) -> u32 {
        self.protocol
    }

    /// Refuses the device, saying why, and whether the list it pinned was reset, signed over the
    /// handshake.
    pub async fn refuse(mut self, reason: &str, reset: bool) {
        let proof = sign(
            &self.key,
            &self
                .transcript
                .refusal_bytes(self.pinned, self.pair.as_ref(), reason, reset),
        );
        let _ = write_line(
            &mut self.writer,
            &ToDevice::Refused {
                reason: reason.to_owned(),
                reset,
                proof: Some(proof),
            },
        )
        .await;
        let _ = self.writer.shutdown().await;
    }

    /// Welcomes the device: the session, whose asks come in on `asks`, lost after `SILENCE`
    /// without a word from the device.
    pub async fn welcome(self) -> std::io::Result<HubSession> {
        self.welcome_within(SILENCE).await
    }

    /// [`Arriving::welcome`], the session lost after `silence` without a word from the device, or
    /// a line it does not read for as long.
    pub async fn welcome_within(mut self, silence: Duration) -> std::io::Result<HubSession> {
        write_line(&mut self.writer, &ToDevice::Welcome {}).await?;
        let (sender, asks) = mpsc::channel(16);
        let (relaying, relays) = mpsc::channel(RELAY_WAITING);
        let mut reader = self.reader;
        let reading = tokio::spawn(async move {
            loop {
                let read = read_frame::<serde_json::Value>(&mut reader, ASK_LIMIT);
                let Ok(frame) = tokio::time::timeout(silence, read).await else {
                    let why = format!("it said nothing in {} s", silence.as_secs());
                    let _ = sender.send(Err(why)).await;
                    return;
                };
                if let Ok(frame) = &frame {
                    if frame.get("keepalive").is_some() {
                        continue;
                    }
                    if let Some(relay) = frame.get("relay") {
                        if let Ok(RelayFrame { to, packet }) =
                            serde_json::from_value::<RelayFrame>(relay.clone())
                            && let Ok(to) = PublicKey::from_bytes(&to)
                        {
                            // A packet with no room is dropped: the asks are read on.
                            let _ = relaying.try_send((to, packet.0));
                        }
                        continue;
                    }
                }
                let asked = frame.and_then(|frame| {
                    frame
                        .get("ask")
                        .cloned()
                        .and_then(|ask| serde_json::from_value::<Asked>(ask).ok())
                        .ok_or_else(|| "it said what is not an ask".to_owned())
                });
                let stop = asked.is_err();
                if sender.send(asked).await.is_err() || stop {
                    return;
                }
            }
        });
        Ok(HubSession {
            transcript: self.transcript,
            pinned: self.pinned,
            asks,
            relays: Some(Relays(relays)),
            writer: self.writer,
            silence,
            reading,
        })
    }
}

/// The packets of links a device sends the hub for other devices, each with the key it is for.
pub struct Relays(mpsc::Receiver<(PublicKey, Vec<u8>)>);

impl Relays {
    /// The next one; it waits for ever once the device is gone (`HubSession::ask` says why).
    pub async fn next(&mut self) -> (PublicKey, Vec<u8>) {
        match self.0.recv().await {
            Some(relayed) => relayed,
            None => std::future::pending().await,
        }
    }
}

/// The hub's side of a session.
pub struct HubSession {
    transcript: Transcript,
    pinned: Option<ChangeHash>,
    asks: mpsc::Receiver<Result<Asked, String>>,
    /// The packets of links the device sent for others, until they are taken.
    relays: Option<Relays>,
    writer: Writer,
    /// How long a line may wait for the device to read it.
    silence: Duration,
    /// The task that reads the device's asks: it holds the connection's other half, so it ends
    /// with the session, or the device would not see the hub go.
    reading: tokio::task::JoinHandle<()>,
}

impl Drop for HubSession {
    fn drop(&mut self) {
        self.reading.abort();
    }
}

impl HubSession {
    pub fn device(&self) -> PublicKey {
        self.transcript.device
    }

    /// The hash of the first change of the list the device checked, as it said it.
    pub fn pinned(&self) -> Option<ChangeHash> {
        self.pinned
    }

    /// The device's next ask; why the session ended otherwise.
    pub async fn ask(&mut self) -> Result<Asked, String> {
        self.asks
            .recv()
            .await
            .unwrap_or_else(|| Err("it closed the connection".to_owned()))
    }

    /// The packets of links the device sends for other devices: given once, `None` after.
    pub fn relays(&mut self) -> Option<Relays> {
        self.relays.take()
    }

    pub async fn send(&mut self, frame: &ToDevice) -> std::io::Result<()> {
        match tokio::time::timeout(self.silence, write_line(&mut self.writer, frame)).await {
            Ok(written) => written,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("it read nothing in {} s", self.silence.as_secs()),
            )),
        }
    }

    pub async fn close(mut self) {
        let _ = self.writer.shutdown().await;
    }
}

/// The hub's side of a new connection: the HTTP upgrade, then the handshake with the hub's key.
/// Anything else is answered as an HTTP server says it is not one.
pub async fn arrive(
    stream: Box<dyn Stream>,
    key: &HubKey,
    hello: &Hello,
) -> Result<Arriving, String> {
    let (read, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    let handshake = async {
        let head = read_head(&mut reader).await?;
        let upgrade = head.first().is_some_and(|line| line.starts_with("GET "))
            && head.iter().skip(1).any(|line| {
                line.split_once(':').is_some_and(|(name, value)| {
                    name.trim().eq_ignore_ascii_case("upgrade")
                        && value.trim().eq_ignore_ascii_case(UPGRADE)
                })
            });
        if !upgrade {
            let body = "this is a flottille-hub: Flottille's devices connect to it\n";
            let answer = format!(
                "HTTP/1.1 426 Upgrade Required\r\nUpgrade: {UPGRADE}\r\nConnection: close\r\n\
                 Content-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = writer.write_all(answer.as_bytes()).await;
            let _ = writer.shutdown().await;
            return Err("not an upgrade to flottille-hub".to_owned());
        }
        writer
            .write_all(
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: \
                     {UPGRADE}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .map_err(|error| error.to_string())?;
        let theirs: DeviceHello = read_frame(&mut reader, HANDSHAKE_LIMIT).await?;
        let nonce = Nonce32::new()?;
        let hub = key.public();
        let transcript = Transcript {
            hub,
            device: theirs.key,
            device_nonce: theirs.nonce,
            hub_nonce: nonce,
        };
        let signed = transcript.signed_hello(&key.0, &theirs.hello, hello);
        let protocol = match choose_hub(&theirs.hello, hello) {
            Ok(protocol) => protocol,
            Err(refusal) => {
                // Said as the device reads it, with the hub's hello, signed, for it to say it
                // itself.
                let _ = write_line(
                    &mut writer,
                    &serde_json::json!({
                        "refused": refusal.to_string(),
                        "hello": signed,
                        "hub": hub,
                        "nonce": nonce,
                    }),
                )
                .await;
                return Err(format!(
                    "a device that speaks protocol {} was refused: {refusal}",
                    theirs.hello.protocols.iter().max().copied().unwrap_or(0)
                ));
            }
        };
        let proof = sign(&key.0, &proof_bytes(1, &theirs.key, &theirs.nonce, &nonce));
        write_line(
            &mut writer,
            &HubHello {
                hub,
                nonce,
                hello: signed,
                proof,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        let proved: DeviceProof = read_frame(&mut reader, HANDSHAKE_LIMIT).await?;
        if !verify(
            &theirs.key,
            &proof_bytes(2, &hub, &nonce, &theirs.nonce),
            &proved.proof,
        ) {
            return Err("the device did not prove it holds the key it showed".to_owned());
        }
        Ok((
            transcript,
            proved.pinned,
            proved.pair,
            theirs.hello.flottille,
            protocol,
        ))
    };
    let (transcript, pinned, pair, version, protocol) = tokio::time::timeout(WAIT, handshake)
        .await
        .map_err(|_| "the handshake did not finish in time".to_owned())??;
    Ok(Arriving {
        key: key.0.clone(),
        transcript,
        pinned,
        pair,
        version,
        protocol,
        reader,
        writer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_holds_a_kind_and_ids_and_nothing_else() {
        let notice: Notice = serde_json::from_value(serde_json::json!({
            "kind": "question", "notification": 12, "session": "3f2a91bc0d4e"
        }))
        .unwrap();
        assert_eq!(notice.kind, NoticeKind::Question);
        // A word of what it is about, a path or a command, is no notice.
        for wrong in [
            serde_json::json!({"kind": "request", "notification": 1, "title": "Read .env"}),
            serde_json::json!({"kind": "request", "notification": 1, "session": "../.env"}),
            serde_json::json!({"kind": "request", "notification": 1, "session": "rm -rf"}),
            serde_json::json!({"kind": "request", "notification": 1, "session": "a".repeat(65)}),
            serde_json::json!({"kind": "request", "notification": 1, "session": ""}),
            serde_json::json!({"kind": "excerpt", "notification": 1}),
        ] {
            assert!(
                serde_json::from_value::<Notice>(wrong.clone()).is_err(),
                "{wrong}"
            );
        }
        assert!("../.env".parse::<NoticeId>().is_err());
    }

    #[test]
    fn the_hubs_address_is_shown_without_its_default_port_or_a_trailing_slash() {
        // What is stored and reached keeps the port and the path whole; what is shown is how a
        // user writes it.
        let shown = |text: &str| HubUrl::parse(text).unwrap().shown();
        assert_eq!(shown("https://hub.example.net"), "https://hub.example.net");
        assert_eq!(
            shown("https://Hub.Example.net:443/"),
            "https://hub.example.net"
        );
        assert_eq!(shown("http://hub.example.net:80"), "http://hub.example.net");
        // A port that is not the scheme's own stays, and so does a path a proxy routes by.
        assert_eq!(
            shown("https://hub.example.net:80"),
            "https://hub.example.net:80"
        );
        assert_eq!(shown("http://192.0.2.10:7448/"), "http://192.0.2.10:7448");
        assert_eq!(
            shown("http://192.0.2.10:7448/flottille"),
            "http://192.0.2.10:7448/flottille"
        );
        assert_eq!(shown("http://[fd7a::7]:7448"), "http://[fd7a::7]:7448");
        assert_eq!(
            HubUrl::parse("https://hub.example.net")
                .unwrap()
                .host_shown(),
            "hub.example.net"
        );
        assert_eq!(HubUrl::shown_text("not a url"), "not a url");
        // Reached as it always was.
        let url = HubUrl::parse("https://hub.example.net").unwrap();
        assert_eq!(url.authority(), "hub.example.net:443");
        assert_eq!(url.to_string(), "https://hub.example.net:443/");
    }

    #[test]
    fn a_hub_url_says_its_port_or_takes_the_scheme_s() {
        let url = HubUrl::parse("https://Hub.Example.net").unwrap();
        assert_eq!(url.to_string(), "https://hub.example.net:443/");
        let url = HubUrl::parse("http://192.0.2.10:7448/flottille").unwrap();
        assert_eq!(url.authority(), "192.0.2.10:7448");

        assert_eq!(url.path, "/flottille");
        let url = HubUrl::parse("http://[fd7a::7]:7448").unwrap();
        assert_eq!(url.tls_name(), "fd7a::7");
        for wrong in [
            "hub.example.net",
            "ftp://hub.example.net",
            "http://",
            "http://user@hub.example.net",
            "http://hub.example.net:0",
            "http://hub.example.net/?a",
            "http://hub\u{200b}.example.net",
        ] {
            assert!(HubUrl::parse(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_pairing_proof_is_for_one_session_one_secret_and_one_card() {
        let key = |n: u8| PublicKey(SigningKey::from_bytes(&[n; 32]).verifying_key());
        let transcript = Transcript {
            hub: key(1),
            device: key(2),
            device_nonce: Nonce32([3; 32]),
            hub_nonce: Nonce32([4; 32]),
        };
        let secret = PairSecret([5; 16]);
        let proposed = Proposed {
            key: key(6),
            name: "pc".to_owned(),
            role: crate::Role::Runner,
            icon: crate::Icon::Desktop,
            colour: crate::Colour::Blue,
        };
        let proof = transcript.pair_proof(&secret, &proposed);
        assert!(transcript.proves_pair(&secret, &proposed, &proof));
        // Not with another secret, not for another card, not in another session.
        assert!(!transcript.proves_pair(&PairSecret([7; 16]), &proposed, &proof));
        let runner_named_otherwise = Proposed {
            name: "server".to_owned(),
            ..proposed.clone()
        };
        assert!(!transcript.proves_pair(&secret, &runner_named_otherwise, &proof));
        let controller = Proposed {
            role: crate::Role::Controller,
            ..proposed.clone()
        };
        assert!(!transcript.proves_pair(&secret, &controller, &proof));
        let other = Transcript {
            hub_nonce: Nonce32([8; 32]),
            ..transcript
        };
        assert!(!other.proves_pair(&secret, &proposed, &proof));
    }

    #[test]
    fn the_hub_s_hello_is_signed_for_one_session_and_one_hello() {
        let hub = SigningKey::from_bytes(&[1; 32]);
        let device = |n: u8| PublicKey(SigningKey::from_bytes(&[n; 32]).verifying_key());
        let transcript = Transcript {
            hub: PublicKey(hub.verifying_key()),
            device: device(2),
            device_nonce: Nonce32([3; 32]),
            hub_nonce: Nonce32([4; 32]),
        };
        let hello = Hello {
            flottille: "0.4.0".to_owned(),
            protocols: vec![1, 2],
        };
        let ours = Hello {
            flottille: "0.4.0".to_owned(),
            protocols: vec![2],
        };
        let signed = transcript.signed_hello(&hub, &ours, &hello);
        assert!(transcript.hub_said(&ours, &signed));
        // Not for a device whose hello the hub received changed: the hub chose from that one.
        let rewritten = Hello {
            protocols: vec![1],
            ..ours.clone()
        };
        assert!(!transcript.hub_said(&rewritten, &signed));
        // Not with a protocol taken out or added, not for another version, not in another
        // session, not unsigned, not as another signature of the hub's.
        let changed = |change: &dyn Fn(&mut Hello)| {
            let mut changed = signed.clone();
            change(&mut changed.hello);
            changed
        };
        assert!(!transcript.hub_said(&ours, &changed(&|hello| hello.protocols = vec![1])));
        assert!(!transcript.hub_said(&ours, &changed(&|hello| hello.protocols.push(999))));
        assert!(!transcript.hub_said(
            &ours,
            &changed(&|hello| hello.flottille = "0.4.01".to_owned())
        ));
        let other = Transcript {
            device_nonce: Nonce32([5; 32]),
            ..transcript
        };
        assert!(!other.hub_said(&ours, &signed));
        let unsigned = SignedHello {
            signed: None,
            ..signed.clone()
        };
        assert!(!transcript.hub_said(&ours, &unsigned));
        let session_proof = sign(
            &hub,
            &proof_bytes(1, &device(2), &Nonce32([3; 32]), &Nonce32([4; 32])),
        );
        let replayed = SignedHello {
            signed: Some(session_proof),
            ..signed.clone()
        };
        assert!(!transcript.hub_said(&ours, &replayed));
        assert!(
            transcript
                .hello_bytes(&ours, &hello)
                .starts_with(b"flottille-hub-hello-v1\0")
        );
        // A device of before reads the hub's hello as it did, the signature beside it ignored.
        let said = serde_json::to_value(&signed).unwrap();
        assert!(said["signed"].is_string());
        assert_eq!(serde_json::from_value::<Hello>(said).unwrap(), hello);
    }

    #[test]
    fn a_proof_is_for_one_side_one_key_and_two_nonces() {
        let (a, b) = (Nonce32([1; 32]), Nonce32([2; 32]));
        let key = SigningKey::from_bytes(&[7; 32]);
        let other = PublicKey(SigningKey::from_bytes(&[8; 32]).verifying_key());
        let signer = PublicKey(key.verifying_key());
        let proof = sign(&key, &proof_bytes(1, &other, &a, &b));
        assert!(verify(&signer, &proof_bytes(1, &other, &a, &b), &proof));
        // Not as the other side's, not for other nonces, not for another key.
        assert!(!verify(&signer, &proof_bytes(2, &other, &a, &b), &proof));
        assert!(!verify(&signer, &proof_bytes(1, &other, &b, &a), &proof));
        assert!(!verify(&signer, &proof_bytes(1, &signer, &a, &b), &proof));
        assert!(proof_bytes(1, &other, &a, &b).starts_with(b"flottille-hub-session-v1\0"));
    }

    #[test]
    fn the_addresses_of_the_previous_version_are_read_and_ignored() {
        // A hub of the previous version tells its members' addresses: read, so that the session
        // goes on, and nothing of it kept.
        let told = format!(
            r#"{{"addresses": [{{"key": "{}", "addresses": ["192.0.2.7:7447"]}}]}}"#,
            "ab".repeat(32)
        );
        assert!(matches!(
            serde_json::from_str::<ToDevice>(&told),
            Ok(ToDevice::Addresses(_))
        ));
        // A machine of the previous version tells its own: still an ask the hub reads.
        let asked: Asked =
            serde_json::from_str(r#"{"id": 3, "kind": "addresses", "addresses": ["192.0.2.7"]}"#)
                .unwrap();
        assert!(matches!(asked.ask, Ask::Addresses { .. }));
    }
}
