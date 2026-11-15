//! A request for another machine (`docs/remote.md`, "Requests from another machine's clients"):
//! made and signed by a client with its device key, carried by its daemon, and checked by the
//! receiving daemon before it serves it as one from a local client.
//!
//! It travels as `{"id", "machine", "signed": {"device", "nonce", "at", "method", "params"},
//! "sig"}`: `device` the signing key, `nonce` 16 random bytes, both in hexadecimal; `at` when it was
//! signed, in milliseconds since the Unix epoch; `params` the JSON text of the parameters, as a
//! string. The receiver checks the signature over the very bytes the client signed, then parses
//! them: no JSON is serialised again on either side, so nothing depends on two serialisers agreeing.
//!
//! What the signature covers, its **signed bytes**, is a fixed encoding:
//!
//! ```text
//! "flottille-request-v1" 0x00    the domain: never the start of a member change's signed bytes
//! id        length (4 bytes, big-endian), UTF-8   the answer comes back under it
//! machine   length (4 bytes, big-endian), UTF-8   the target: another machine refuses it
//! device    32 bytes
//! nonce     16 bytes
//! at         8 bytes, big-endian
//! method    length (4 bytes, big-endian), UTF-8
//! params    length (4 bytes, big-endian), the JSON text
//! ```

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::Signer;
use flottille_protocol::{Class, class};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::serial::{from_hex_text, to_hex};
use crate::{DeviceKey, KeyError, Member, Members, PublicKey, Role, Signature};

/// The start of every request's signed bytes.
const DOMAIN: &[u8] = b"flottille-request-v1\0";

/// How far a request's time may be from the receiver's clock, either way.
pub const REQUEST_WINDOW: Duration = Duration::from_secs(5 * 60);

/// How many requests of one device the receiver remembers within the window; past it, the
/// device's next requests are refused until the oldest leave the window, never forgotten early,
/// which would let them be played again.
pub const NONCES_PER_DEVICE: usize = 4096;

/// A request's 16 random bytes: no two requests of a device have the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nonce(pub [u8; 16]);

/// What the device signs, but the target and the id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub device: PublicKey,
    pub nonce: Nonce,
    /// When it was signed, in milliseconds since the Unix epoch.
    pub at: u64,
    pub method: String,
    /// The parameters' JSON text, as the client wrote it.
    pub params: String,
}

/// A request for another machine, as its client signed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    /// The name of the machine it is for.
    pub machine: String,
    pub signed: Signed,
    pub sig: Signature,
}

impl Request {
    /// `method` with `params`, for `machine`, signed with `key` at `at` under a new nonce.
    pub fn sign(
        key: &DeviceKey,
        id: &str,
        machine: &str,
        method: &str,
        params: &Value,
        at: SystemTime,
    ) -> Result<Request, KeyError> {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce)
            .map_err(|error| KeyError(format!("no random bytes for a request's nonce: {error}")))?;
        let mut request = Request {
            id: id.to_owned(),
            machine: machine.to_owned(),
            signed: Signed {
                device: key.public(),
                nonce: Nonce(nonce),
                at: milliseconds(at),
                method: method.to_owned(),
                params: params.to_string(),
            },
            sig: Signature([0; 64]),
        };
        request.sig = Signature(key.0.sign(&request.signed_bytes()).to_bytes());
        Ok(request)
    }

    /// What the signature covers: the encoding of the module's documentation.
    pub fn signed_bytes(&self) -> Vec<u8> {
        // A text longer than 4 GB cannot be signed; no link carries one.
        let text = |bytes: &mut Vec<u8>, text: &str| {
            bytes.extend_from_slice(&(text.len() as u32).to_be_bytes());
            bytes.extend_from_slice(text.as_bytes());
        };
        let mut bytes = DOMAIN.to_vec();
        text(&mut bytes, &self.id);
        text(&mut bytes, &self.machine);
        bytes.extend_from_slice(self.signed.device.as_bytes());
        bytes.extend_from_slice(&self.signed.nonce.0);
        bytes.extend_from_slice(&self.signed.at.to_be_bytes());
        text(&mut bytes, &self.signed.method);
        text(&mut bytes, &self.signed.params);
        bytes
    }

    /// Whether the signature is the device's over the signed bytes; strict, as a member change's.
    fn signed(&self) -> bool {
        let signature = ed25519_dalek::Signature::from_bytes(&self.sig.0);
        self.signed
            .device
            .0
            .verify_strict(&self.signed_bytes(), &signature)
            .is_ok()
    }
}

/// A request that passed every check, to be served as one from a local client.
#[derive(Clone, Debug)]
pub struct Accepted {
    /// The member that signed it, whom the log and the events name.
    pub device: Member,
    pub id: String,
    pub method: String,
    pub params: Value,
}

/// Why a request is refused, in a sentence that starts with `refused:`.
#[derive(Debug, PartialEq, Eq)]
pub struct RequestError(String);

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused: {}", self.0)
    }
}

impl std::error::Error for RequestError {}

/// The receiving machine's memory of the nonces it accepted within the window, per device.
#[derive(Debug, Default)]
pub struct Requests {
    seen: HashMap<[u8; 32], Vec<(u64, Nonce)>>,
    /// The latest time of the clock this memory was given: a nonce is forgotten only once it is
    /// out of the window of that time, so that a clock set back does not bring a request back.
    latest: u64,
}

impl Requests {
    pub fn new() -> Requests {
        Requests::default()
    }

    /// Checks `text`, a request as the link carried it, on the machine named `machine` whose
    /// checked member list is `members`, at `now`: the device is a member; its role allows the
    /// method's class; the signature; the time within the window of `now`; the nonce not seen in
    /// that window. An accepted request's nonce is remembered.
    pub fn check(
        &mut self,
        text: &str,
        machine: &str,
        members: &Members,
        now: SystemTime,
    ) -> Result<Accepted, RequestError> {
        let refused = |why: String| Err(RequestError(why));
        let request: Request = match serde_json::from_str(text) {
            Ok(request) => request,
            Err(error) => return refused(format!("this is not a signed request: {error}")),
        };
        if request.machine != machine {
            return refused(format!(
                "this request is for {}, not for {machine}",
                request.machine
            ));
        }
        let signed = &request.signed;
        let Some(member) = members.get(&signed.device) else {
            return refused(format!(
                "the device {} is not a member",
                signed.device.fingerprint()
            ));
        };
        let Some(class) = class(&signed.method) else {
            return refused(format!("{} is not a method of Flottille's", signed.method));
        };
        if member.role == Role::Viewer && class == Class::Drive {
            return refused(format!(
                "{} may only view: a controller or a runner may call {}",
                member.name, signed.method
            ));
        }
        if !request.signed() {
            return refused(format!(
                "this request does not carry the signature of {}",
                member.name
            ));
        }
        let window = REQUEST_WINDOW.as_millis() as u64;
        let now = milliseconds(now);
        self.latest = self.latest.max(now);
        if signed.at < self.latest.saturating_sub(window) {
            return refused("this request is older than five minutes".to_owned());
        }
        if signed.at > now.saturating_add(window) {
            return refused(
                "this request is dated more than five minutes ahead of this machine's clock"
                    .to_owned(),
            );
        }
        self.forget(self.latest.saturating_sub(window));
        let seen = self.seen.entry(*signed.device.as_bytes()).or_default();
        if seen.iter().any(|(_, nonce)| *nonce == signed.nonce) {
            return refused("this request was received already".to_owned());
        }
        if seen.len() >= NONCES_PER_DEVICE {
            return refused(format!(
                "{} sent more than {NONCES_PER_DEVICE} requests in five minutes",
                member.name
            ));
        }
        let params = match serde_json::from_str(&signed.params) {
            Ok(params) => params,
            Err(error) => return refused(format!("its params are not JSON: {error}")),
        };
        seen.push((signed.at, signed.nonce));
        Ok(Accepted {
            device: member.clone(),
            id: request.id,
            method: request.signed.method,
            params,
        })
    }

    /// Forgets the nonces of requests signed before `floor`, which the time check refuses anyway.
    fn forget(&mut self, floor: u64) {
        self.seen.retain(|_, seen| {
            seen.retain(|(at, _)| *at >= floor);
            !seen.is_empty()
        });
    }
}

/// `time` in milliseconds since the Unix epoch; zero before it.
fn milliseconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

impl Serialize for Nonce {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Nonce {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Nonce, D::Error> {
        from_hex_text(deserializer, "a nonce").map(Nonce)
    }
}
