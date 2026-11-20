//! The control session every device keeps to its hub (`docs/remote.md`, "The hub",
//! "Architecture"): the member list and its changes, the members' addresses and presence. Both
//! sides are here: the device's, and the handshake the hub answers with, so that the hub and a
//! device cannot disagree on the wire.
//!
//! The hub is not trusted with the members: it stores and forwards what members signed, and every
//! device checks every list it is sent back to the root it pinned at setup (`Device::take`). It is
//! trusted to be the user's own hub only so far as it holds the key the device pinned: a hub that
//! answers with another key is refused before anything else is said.
//!
//! On the wire, a device opens a TCP connection (TLS for an `https://` URL), asks for an HTTP/1.1
//! upgrade to `flottille-hub`, so that a reverse proxy in front of the hub carries the session as
//! it carries a WebSocket, then both sides say one JSON object per line:
//!
//! ```text
//! device → hub   {"key": K, "nonce": Nd, "hello": {…}}
//! hub → device   {"hub": H, "nonce": Nh, "hello": {…}, "proof": sign_H(session, 1, K, Nd, Nh)}
//! device → hub   {"proof": sign_K(session, 2, H, Nh, Nd), "pinned": P | null, "join"?: J}
//! hub → device   {"welcome": {}}  or  {"refused": {"reason": "…", "reset": true | false}}
//! then           device → hub {"ask": {"id": n, …}}; hub → device {"answer": {"id": n, …}},
//!                {"list": […]}, {"addresses": […]}, {"presence": […]}, {"requests": […]}
//! ```
//!
//! A machine that joins through an invitation (`docs/remote.md`, "Pairing") is no member yet: its
//! proof carries `join`, `{"token": T, "key": K}`, the invitation's token and the new device's key.
//! The hub welcomes it while the join request waits, and sends it nothing; once the member that
//! invited added K, it sends the list, which the device checks against the invitation
//! (`Invitation::check_list`), and takes the declaration of its machine. A refused or expired
//! request ends the session. The member that invited is sent the requests that wait for it,
//! `{"requests": [{"token": T, "key": K}]}`, at once and whenever they change.
//!
//! `session` is `flottille-hub-session-v1` and a zero byte: nothing else a key signs starts so.
//! Each side signs the other's fresh nonce and the other's key, so that a proof cannot be played
//! again, nor sent back to the side that made it. `pinned` is the hash of the first change of the
//! list the device checked, if it has one: a hub that refuses the device says `reset` when its own
//! list starts otherwise, and tells a device nothing else of its list.
//!
//! The setup code never crosses the network. The first member's setup carries a proof that the
//! device holds it, bound to this session and to the changes it sets up; the hub answers with its
//! own, and the device pins the hub's key only once it checked it:
//!
//! ```text
//! device → hub   {"ask": {"id": n, "kind": "setup", "proof": M2, "changes": […]}}
//! hub → device   {"answer": {"id": n, "proof": M1}}
//! Ms = HMAC-SHA256(code's 80 bits, "flottille-hub-setup-v1" 0x00, s, H, K, Nd, Nh, head)
//! ```
//!
//! where `head` is the hash of the last change set up: someone on the path, who sees neither the
//! code nor a way to guess its 80 bits, can change neither the changes nor the hub.

use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::members::Signature;
use crate::serial::{from_hex_text, invisible, shown, to_hex};
use crate::{
    Change, ChangeHash, Hello, HubKey, Invitation, InvitationAsk, ListError, MachineKey,
    MemberList, Members, NeedsUpdate, PublicKey, Token, choose,
};

/// The port a hub listens on unless told otherwise.
pub const HUB_PORT: u16 = 7448;
/// The protocol name of the HTTP upgrade.
const UPGRADE: &str = "flottille-hub";
/// The start of what each side of a session signs.
const SESSION: &[u8] = b"flottille-hub-session-v1\0";
/// The start of what each side of a setup proves with the setup code.
const SETUP: &[u8] = b"flottille-hub-setup-v1\0";
/// The longest line a device takes from its hub: a list of `MOST_CHANGES + MANAGING_HEADROOM`
/// changes fits.
pub const LINE_LIMIT: usize = 4 << 20;
/// The longest line of a handshake, either side's: some hundred bytes each.
pub const HANDSHAKE_LIMIT: usize = 4 << 10;
/// The longest ask the hub takes: a setup, a change, or the most addresses a machine tells.
const ASK_LIMIT: usize = 64 << 10;
/// The longest text of the hub's a device shows.
const SHOWN: usize = 400;
/// The longest HTTP head either side takes.
const HEAD_LIMIT: usize = 8 << 10;
/// How long reaching the hub, or its handshake, may take.
const WAIT: Duration = Duration::from_secs(10);

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
                "the hub at {hub} has another key than when this device joined"
            ),
            ControlError::Reset => ListError::Reset.fmt(f),
            ControlError::Refused { hub, reason } => {
                write!(f, "the hub at {hub} refused: {reason}")
            }
            ControlError::NeedsUpdate(refusal) => refusal.fmt(f),
            ControlError::Lost { hub, why } => {
                write!(f, "the session to the hub at {hub} ended: {why}")
            }
        }
    }
}

impl std::error::Error for ControlError {}

/// The setup code a hub prints (`7KQ2-M4XP-9TRW-H3CD`): 80 random bits in Crockford's base32, by
/// groups of four. It is never sent: each side proves it holds it (`Transcript::setup_proof`).
#[derive(Clone, PartialEq, Eq)]
pub struct SetupCode([u8; 10]);

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl SetupCode {
    /// A new code, from the system's random generator.
    pub fn new() -> Result<SetupCode, String> {
        let mut bytes = [0u8; 10];
        getrandom::fill(&mut bytes).map_err(|error| format!("no random bytes: {error}"))?;
        Ok(SetupCode(bytes))
    }

    /// The code as a person may type it: in any case, `O` as `0`, `I` and `L` as `1`, hyphens and
    /// spaces skipped. `None` when it is not 16 such symbols.
    pub fn parse(text: &str) -> Option<SetupCode> {
        let (mut value, mut symbols) = (0u128, 0);
        for c in text.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            let symbol = match c.to_ascii_uppercase() {
                'O' => 0,
                'I' | 'L' => 1,
                upper => ALPHABET
                    .iter()
                    .position(|&letter| letter as char == upper)? as u128,
            };
            symbols += 1;
            if symbols > 16 {
                return None;
            }
            value = (value << 5) | symbol;
        }
        if symbols != 16 {
            return None;
        }
        value.to_be_bytes()[6..].try_into().ok().map(SetupCode)
    }
}

impl fmt::Display for SetupCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut value = [0u8; 16];
        value[6..].copy_from_slice(&self.0);
        let value = u128::from_be_bytes(value);
        for at in 0..16 {
            if at > 0 && at % 4 == 0 {
                f.write_str("-")?;
            }
            let symbol = ALPHABET[((value >> (75 - 5 * at)) & 31) as usize] as char;
            write!(f, "{symbol}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for SetupCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SetupCode(…)")
    }
}

/// One side's proof that it holds the setup code, for one session and one setup.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SetupProof(pub [u8; 32]);

impl fmt::Debug for SetupProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SetupProof").field(&to_hex(&self.0)).finish()
    }
}

impl Serialize for SetupProof {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for SetupProof {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<SetupProof, D::Error> {
        from_hex_text(deserializer, "a setup proof").map(SetupProof)
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
    /// The proof of `side` (1 the hub's, 2 the device's) that it holds `code`, for the setup
    /// whose last change's hash is `head`.
    pub fn setup_proof(&self, code: &SetupCode, side: u8, head: &ChangeHash) -> SetupProof {
        let mut proof = [0u8; 32];
        proof.copy_from_slice(&self.setup_mac(code, side, head).finalize().into_bytes());
        SetupProof(proof)
    }

    /// Whether `proof` is `side`'s for `code` and `head`, compared in constant time.
    pub fn proves(
        &self,
        code: &SetupCode,
        side: u8,
        head: &ChangeHash,
        proof: &SetupProof,
    ) -> bool {
        self.setup_mac(code, side, head)
            .verify_slice(&proof.0)
            .is_ok()
    }

    fn setup_mac(&self, code: &SetupCode, side: u8, head: &ChangeHash) -> Hmac<Sha256> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&code.0).expect("HMAC takes a key of any length");
        mac.update(SETUP);
        mac.update(&[side]);
        mac.update(self.hub.as_bytes());
        mac.update(self.device.as_bytes());
        mac.update(&self.device_nonce.0);
        mac.update(&self.hub_nonce.0);
        mac.update(&head.0);
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
    hello: Hello,
    proof: Signature,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceProof {
    proof: Signature,
    /// The hash of the first change of the list the device checked, when it has one.
    #[serde(default)]
    pinned: Option<ChangeHash>,
    /// The invitation a machine that is no member yet answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    join: Option<Joining>,
}

/// What a machine that joins says in its proof: the invitation's token, and the key of the new
/// device, which the member that invited adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Joining {
    pub token: Token,
    pub key: PublicKey,
}

/// A join request that waits for the member it is sent to: the token of its invitation, and the
/// new device's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requested {
    pub token: Token,
    pub key: PublicKey,
}

/// What a device asks the hub, once in a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// A few asks a session, each sent at once: their size costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
pub enum Ask {
    /// The first member, with the proof that the device holds the setup code the hub printed:
    /// its own entry, then the declaration of the machine that carries it, which must be this
    /// session's key.
    Setup {
        proof: SetupProof,
        changes: Vec<Change>,
    },
    /// A member's change of the list.
    Submit { change: Change },
    /// A member's signed ask for an invitation: answered with its token.
    Invite { ask: InvitationAsk },
    /// The member that invited refuses the join request of this token.
    Refuse { token: Token },
    /// Every address this machine may be reached at now, in place of those told before.
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

/// The addresses a member's machine told the hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    pub key: PublicKey,
    pub addresses: Vec<String>,
}

/// Whether a member's client is active, as it told the hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceOf {
    pub key: PublicKey,
    pub active: bool,
    pub last_input: u64,
}

/// What the hub sends a device once the handshake is done.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToDevice {
    /// The session is a member's, or a setup's while the list is empty.
    Welcome {},
    /// The device is not one of the members; `reset` when the list it pinned is not the hub's
    /// any more, the hub having been set up again.
    Refused {
        reason: String,
        #[serde(default)]
        reset: bool,
    },
    Answer {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refused: Option<String>,
        /// The hub's proof that it holds the setup code, in the answer to a setup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proof: Option<SetupProof>,
        /// The invitation's token, in the answer to an invitation's ask.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<Token>,
    },
    /// The member list, whole, as the members signed it.
    List(MemberList),
    /// The addresses every member's machine told, in place of those sent before.
    Addresses(Vec<Told>),
    /// The presence of the members' clients, for a runner.
    Presence(Vec<PresenceOf>),
    /// The join requests that wait for the member of this device, in place of those sent before.
    Requests(Vec<Requested>),
}

/// What the hub pushes to a device, as the session hands it over.
#[derive(Clone, Debug)]
pub enum Pushed {
    List(MemberList),
    Addresses(Vec<Told>),
    Presence(Vec<PresenceOf>),
    Requests(Vec<Requested>),
}

/// What an answer carries besides a yes.
#[derive(Clone, Copy, Debug, Default)]
struct Extra {
    proof: Option<SetupProof>,
    token: Option<Token>,
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

/// What a device keeps of its hub, pinned when it was set up: the hub's URL and key, the root of
/// the member list, and the list as this device last checked it.
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
        self.list.check(&self.root)
    }

    /// A list the hub sent: kept when every change checks back to the root this device pinned
    /// and it holds every change this device checked already; refused otherwise, the list kept
    /// as it was. A hub that forges, drops, reorders or plays a change again, or was set up again,
    /// is refused here, whatever the hub itself checked.
    pub fn take(&mut self, list: MemberList) -> Result<Members, ListError> {
        let members = list.check_update(&self.root, &self.list)?;
        self.list = list;
        Ok(members)
    }

    /// The first member: `changes` (the device's own entry as the root, then its machine's
    /// declaration, signed by the device and claimed by the machine) and the setup `code`, proved
    /// through a session `machine` opens, never sent. The hub's key is pinned from this session
    /// once the hub proved it holds the code too, and the device keeps the list only once the hub
    /// sent it back holding the setup: a hub that withholds it is refused.
    pub async fn setup(
        url: &str,
        code: &str,
        changes: Vec<Change>,
        machine: &MachineKey,
        hello: &Hello,
        tls: &Tls,
    ) -> Result<(Device, Session), ControlError> {
        let parsed = HubUrl::parse(url).map_err(|why| ControlError::Refused {
            hub: url.to_owned(),
            reason: why,
        })?;
        let refused = |reason: &str| ControlError::Refused {
            hub: parsed.host_shown(),
            reason: reason.to_owned(),
        };
        let code = SetupCode::parse(code).ok_or_else(|| {
            refused(
                "this is not a setup code: 16 letters and digits, as the hub printed it \
                 (7KQ2-M4XP-9TRW-H3CD)",
            )
        })?;
        let root = changes
            .first()
            .map(|first| first.signer)
            .ok_or_else(|| refused("there is no entry to set up with"))?;
        let list = MemberList::from_changes(changes.clone());
        list.check(&root)
            .map_err(|error| refused(&error.to_string()))?;
        let mut session = Session::open(&parsed, None, &machine.0, hello, tls, None, None).await?;
        if let Some((reason, _)) = &session.refused {
            return Err(refused(reason));
        }
        let head = list.head();
        let transcript = session.transcript;
        let answer = session
            .asker()
            .answer(Ask::Setup {
                proof: transcript.setup_proof(&code, 2, &head),
                changes,
            })
            .await?;
        if !answer
            .proof
            .is_some_and(|proof| transcript.proves(&code, 1, &head, &proof))
        {
            return Err(refused(
                "it did not prove it knows the setup code: it is not the hub that printed it",
            ));
        }
        let mut device = Device {
            url: parsed.to_string(),
            hub: session.hub(),
            root,
            list,
        };
        let withheld =
            |why: String| refused(&format!("the hub withheld this device's setup: {why}"));
        device
            .sent_back(&mut session, &head, true)
            .await
            .map_err(withheld)?;
        Ok((device, session))
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
        self.sent_back(session, &hash, false)
            .await
            .map_err(|why| ControlError::Refused {
                hub: session.authority.clone(),
                reason: format!("the hub withheld this change: {why}"),
            })
    }

    /// Takes the lists `session` sends until one holds the change whose hash is `hash`; why not,
    /// when one is refused or none comes in time. In a `setup`, an empty list is the one the hub
    /// sent before it.
    async fn sent_back(
        &mut self,
        session: &mut Session,
        hash: &ChangeHash,
        setup: bool,
    ) -> Result<Members, String> {
        let waited = tokio::time::timeout(WAIT, async {
            loop {
                let Some(pushed) = session.next().await else {
                    return Err(session.ended());
                };
                let Pushed::List(list) = pushed else {
                    continue;
                };
                if setup && list.changes().is_empty() {
                    continue;
                }
                let holds = list.changes().iter().any(|change| change.hash() == *hash);
                let members = self.take(list).map_err(|error| error.to_string())?;
                if holds {
                    return Ok(members);
                }
            }
        });
        waited.await.unwrap_or_else(|_| {
            Err(format!(
                "it sent back no list holding it in {} s",
                WAIT.as_secs()
            ))
        })
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
            Some((reason, false)) => Err(ControlError::Refused {
                hub: url.host_shown(),
                reason,
            }),
            None => Ok(session),
        }
    }

    /// The new device's side of a pairing (`docs/remote.md`, "Pairing", 2): `machine` reaches the
    /// hub of `invitation`, pinning the key the invitation holds, and asks to join with the token
    /// and the new device's key, `key`. The hub holds the request until the member that invited
    /// accepts or refuses it (`Joined::accepted`).
    pub async fn join(
        invitation: &Invitation,
        key: &PublicKey,
        machine: &MachineKey,
        hello: &Hello,
        tls: &Tls,
    ) -> Result<Joined, ControlError> {
        let url = HubUrl::parse(&invitation.hub_url).map_err(|why| ControlError::Refused {
            hub: invitation.hub_url.clone(),
            reason: why,
        })?;
        let joining = Joining {
            token: invitation.token,
            key: *key,
        };
        let session = Session::open(
            &url,
            Some(&invitation.hub_key),
            &machine.0,
            hello,
            tls,
            None,
            Some(joining),
        )
        .await?;
        if let Some((reason, _)) = &session.refused {
            return Err(ControlError::Refused {
                hub: url.host_shown(),
                reason: reason.clone(),
            });
        }
        Ok(Joined {
            invitation: invitation.clone(),
            key: *key,
            url: url.to_string(),
            session,
        })
    }
}

/// A join request the hub holds, its session open.
#[derive(Debug)]
pub struct Joined {
    invitation: Invitation,
    key: PublicKey,
    url: String,
    pub session: Session,
}

impl Joined {
    /// Waits until the member that invited accepted the request: the hub's key pinned, and the
    /// first list, which checks back to its own root and holds this device's entry, signed by the
    /// member of the invitation (`Invitation::check_list`). A list that does not is refused, and
    /// so is the pairing; a refused or expired request ends the session, said.
    pub async fn accepted(&mut self) -> Result<(Device, Members), ControlError> {
        let authority = self.session.authority.clone();
        let refused = |reason: String| ControlError::Refused {
            hub: authority.clone(),
            reason,
        };
        // The hub ends the request with its invitation, 5 minutes at most: this is in case it
        // does not.
        let waited = tokio::time::timeout(Duration::from_secs(6 * 60), async {
            loop {
                let Some(pushed) = self.session.next().await else {
                    return Err(refused(self.session.ended_why()));
                };
                let Pushed::List(list) = pushed else {
                    continue;
                };
                let members = self
                    .invitation
                    .check_list(&list, &self.key)
                    .map_err(|error| refused(error.to_string()))?;
                let device = Device {
                    url: self.url.clone(),
                    hub: self.invitation.hub_key,
                    root: members.root(),
                    list,
                };
                return Ok((device, members));
            }
        });
        waited.await.unwrap_or_else(|_| {
            Err(refused(
                "the join request was not answered in 6 minutes".to_owned(),
            ))
        })
    }
}

type Answered = oneshot::Sender<Result<Extra, String>>;

/// A session to the hub, its handshake done.
pub struct Session {
    hub: PublicKey,
    authority: String,
    protocol: u32,
    transcript: Transcript,
    asks: mpsc::UnboundedSender<(Ask, Answered)>,
    pushed: mpsc::UnboundedReceiver<Pushed>,
    /// Why the session ended, once it did.
    ended: Arc<Mutex<Option<String>>>,
    /// When the hub refused the device: its reason, and whether the list the device pinned was
    /// reset.
    refused: Option<(String, bool)>,
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
        self.answer(ask).await.map(|_| ())
    }

    /// Asks the hub for an invitation, with a member's signed ask: its token.
    pub async fn invite(&self, ask: InvitationAsk) -> Result<Token, ControlError> {
        self.answer(Ask::Invite { ask })
            .await?
            .token
            .ok_or_else(|| ControlError::Lost {
                hub: self.authority.clone(),
                why: "it answered an invitation's ask without a token".to_owned(),
            })
    }

    /// Asks the hub, and waits for its answer, with what it may carry.
    async fn answer(&self, ask: Ask) -> Result<Extra, ControlError> {
        let (answer, answered) = oneshot::channel();
        if self.asks.send((ask, answer)).is_err() {
            return Err(lost(&self.authority, &self.ended));
        }
        match answered.await {
            Ok(Ok(extra)) => Ok(extra),
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
        join: Option<Joining>,
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
                return Err(ControlError::Refused {
                    hub: authority.clone(),
                    reason: shown(reason, SHOWN),
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
            let protocol = choose(hello, &theirs.hello, &format!("the hub at {authority}"))
                .map_err(ControlError::NeedsUpdate)?;
            let proof = sign(key, &proof_bytes(2, &theirs.hub, &theirs.nonce, &nonce));
            write_line(
                &mut writer,
                &DeviceProof {
                    proof,
                    pinned: pinned_list,
                    join,
                },
            )
            .await
            .map_err(|error| lost(error.to_string()))?;
            let welcome: ToDevice = read_frame(&mut reader, HANDSHAKE_LIMIT)
                .await
                .map_err(lost)?;
            let transcript = Transcript {
                hub: theirs.hub,
                device: own,
                device_nonce: nonce,
                hub_nonce: theirs.nonce,
            };
            Ok((transcript, protocol, welcome))
        };
        let (transcript, protocol, welcome) =
            tokio::time::timeout(WAIT, handshake).await.map_err(|_| {
                lost(format!(
                    "it did not finish its handshake in {} s",
                    WAIT.as_secs()
                ))
            })??;
        let (asks, asked) = mpsc::unbounded_channel();
        let (pushing, pushed) = mpsc::unbounded_channel();
        let ended = Arc::new(Mutex::new(None));
        let refused = match welcome {
            ToDevice::Welcome {} => {
                tokio::spawn(drive(reader, writer, asked, pushing, ended.clone()));
                None
            }
            ToDevice::Refused { reason, reset } => Some((shown(&reason, SHOWN), reset)),
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
            transcript,
            asks,
            pushed,
            ended,
            refused,
        })
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
    let why = loop {
        tokio::select! {
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
                match line {
                    Some(Ok(ToDevice::Answer { id, refused, proof, token })) => {
                        if let Some(at) = waiting.iter().position(|(waits, _)| *waits == id) {
                            let (_, answer) = waiting.swap_remove(at);
                            let _ = answer.send(match refused {
                                Some(reason) => Err(shown(&reason, SHOWN)),
                                None => Ok(Extra { proof, token }),
                            });
                        }
                    }
                    Some(Ok(ToDevice::Requests(requests))) => { let _ = pushing.send(Pushed::Requests(requests)); }
                    Some(Ok(ToDevice::List(list))) => { let _ = pushing.send(Pushed::List(list)); }
                    Some(Ok(ToDevice::Addresses(told))) => { let _ = pushing.send(Pushed::Addresses(told)); }
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
    transcript: Transcript,
    pinned: Option<ChangeHash>,
    join: Option<Joining>,
    protocol: u32,
    reader: Reader,
    writer: Writer,
}

impl Arriving {
    /// The key the device proved it holds.
    pub fn device(&self) -> PublicKey {
        self.transcript.device
    }

    /// The invitation a machine that is no member yet answers, as it said it.
    pub fn joining(&self) -> Option<Joining> {
        self.join
    }

    /// The hash of the first change of the list the device checked, as it said it.
    pub fn pinned(&self) -> Option<ChangeHash> {
        self.pinned
    }

    pub fn protocol(&self) -> u32 {
        self.protocol
    }

    /// Refuses the device, saying why, and whether the list it pinned was reset.
    pub async fn refuse(mut self, reason: &str, reset: bool) {
        let _ = write_line(
            &mut self.writer,
            &ToDevice::Refused {
                reason: reason.to_owned(),
                reset,
            },
        )
        .await;
        let _ = self.writer.shutdown().await;
    }

    /// Welcomes the device: the session, whose asks come in on `asks`.
    pub async fn welcome(mut self) -> std::io::Result<HubSession> {
        write_line(&mut self.writer, &ToDevice::Welcome {}).await?;
        let (sender, asks) = mpsc::channel(16);
        let mut reader = self.reader;
        let reading = tokio::spawn(async move {
            loop {
                let frame = read_frame::<serde_json::Value>(&mut reader, ASK_LIMIT).await;
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
            writer: self.writer,
            reading,
        })
    }
}

/// The hub's side of a session.
pub struct HubSession {
    transcript: Transcript,
    pinned: Option<ChangeHash>,
    asks: mpsc::Receiver<Result<Asked, String>>,
    writer: Writer,
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

    /// The keys and nonces of the session's handshake, which a setup's proofs are bound to.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The device's next ask; why the session ended otherwise.
    pub async fn ask(&mut self) -> Result<Asked, String> {
        self.asks
            .recv()
            .await
            .unwrap_or_else(|| Err("it closed the connection".to_owned()))
    }

    pub async fn send(&mut self, frame: &ToDevice) -> std::io::Result<()> {
        write_line(&mut self.writer, frame).await
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
        let protocol = match choose(hello, &theirs.hello, "this device") {
            Ok(protocol) => protocol,
            Err(refusal) => {
                let _ = write_line(
                    &mut writer,
                    &serde_json::json!({"refused": refusal.to_string()}),
                )
                .await;
                return Err(refusal.to_string());
            }
        };
        let proof = sign(&key.0, &proof_bytes(1, &theirs.key, &theirs.nonce, &nonce));
        write_line(
            &mut writer,
            &HubHello {
                hub,
                nonce,
                hello: hello.clone(),
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
        let transcript = Transcript {
            hub,
            device: theirs.key,
            device_nonce: theirs.nonce,
            hub_nonce: nonce,
        };
        Ok((transcript, proved.pinned, proved.join, protocol))
    };
    let (transcript, pinned, join, protocol) = tokio::time::timeout(WAIT, handshake)
        .await
        .map_err(|_| "the handshake did not finish in time".to_owned())??;
    Ok(Arriving {
        transcript,
        pinned,
        join,
        protocol,
        reader,
        writer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_setup_code_reads_as_a_person_types_it_and_proves_one_setup() {
        let code = SetupCode::new().unwrap();
        let text = code.to_string();
        assert_eq!(text.len(), 19, "{text}");
        let typed = text.replace('-', "").to_lowercase().replace('0', "o");
        assert_eq!(SetupCode::parse(&typed), Some(code.clone()));
        assert_eq!(SetupCode::parse("7KQ2-M4XP-9TRW"), None);
        assert_eq!(SetupCode::parse("7KQ2-M4XP-9TRW-H3CDX"), None);
        assert_eq!(SetupCode::parse("7KQ2-M4XP-9TRW-H3CU"), None);

        let key = |n: u8| PublicKey(SigningKey::from_bytes(&[n; 32]).verifying_key());
        let transcript = Transcript {
            hub: key(1),
            device: key(2),
            device_nonce: Nonce32([3; 32]),
            hub_nonce: Nonce32([4; 32]),
        };
        let head = ChangeHash([5; 32]);
        let proof = transcript.setup_proof(&code, 2, &head);
        assert!(transcript.proves(&code, 2, &head, &proof));
        // Not as the other side's, not for other changes, another code, another session.
        assert!(!transcript.proves(&code, 1, &head, &proof));
        assert!(!transcript.proves(&code, 2, &ChangeHash([6; 32]), &proof));
        assert!(!transcript.proves(&SetupCode::new().unwrap(), 2, &head, &proof));
        let other = Transcript {
            hub_nonce: Nonce32([7; 32]),
            ..transcript
        };
        assert!(!other.proves(&code, 2, &head, &proof));
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
}
