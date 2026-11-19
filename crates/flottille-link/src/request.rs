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
        let bytes = request.signed_bytes().ok_or_else(|| {
            KeyError("a field of this request is longer than 4 GB: it cannot be signed".to_owned())
        })?;
        request.sig = Signature(key.0.sign(&bytes).to_bytes());
        Ok(request)
    }

    /// What the signature covers: the encoding of the module's documentation. None when a field
    /// is longer than its length prefix says (4 GB): such a request is neither signed nor taken.
    pub fn signed_bytes(&self) -> Option<Vec<u8>> {
        let text = |bytes: &mut Vec<u8>, text: &str| -> Option<()> {
            bytes.extend_from_slice(&prefix(text.len())?);
            bytes.extend_from_slice(text.as_bytes());
            Some(())
        };
        let mut bytes = DOMAIN.to_vec();
        text(&mut bytes, &self.id)?;
        text(&mut bytes, &self.machine)?;
        bytes.extend_from_slice(self.signed.device.as_bytes());
        bytes.extend_from_slice(&self.signed.nonce.0);
        bytes.extend_from_slice(&self.signed.at.to_be_bytes());
        text(&mut bytes, &self.signed.method)?;
        text(&mut bytes, &self.signed.params)?;
        Some(bytes)
    }

    /// Whether the signature is the device's over the signed bytes; strict, as a member change's.
    fn signed(&self) -> bool {
        let Some(bytes) = self.signed_bytes() else {
            return false;
        };
        let signature = ed25519_dalek::Signature::from_bytes(&self.sig.0);
        self.signed
            .device
            .0
            .verify_strict(&bytes, &signature)
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

/// The receiving machine's memory of the nonces it accepted within the window, per device. It is
/// kept in memory only: the daemon keeps one for all its links, never one per connection, which
/// would let a request be played again on another, and starts it again at each start, refusing
/// what was signed before (`new`).
#[derive(Debug)]
pub struct Requests {
    seen: HashMap<[u8; 32], Vec<(u64, Nonce)>>,
    /// When this machine started: a request signed before may have been served by the memory a
    /// restart lost.
    started: u64,
    /// The latest time of the clock this memory was given: a nonce is forgotten once it is out of
    /// the window of that time. Only the forgetting reads it, never the time check, so that a
    /// clock read far ahead once locks no device out; the cost is that a clock set back by more
    /// than the window may let a forgotten request in again.
    latest: u64,
}

impl Requests {
    /// The memory of a machine that started at `started`.
    pub fn new(started: SystemTime) -> Requests {
        let started = milliseconds(started);
        Requests {
            seen: HashMap::new(),
            started,
            latest: started,
        }
    }

    /// Checks `text`, a request as the link carried it, on the machine named `machine` whose
    /// checked member list is `members`, at `now`, in this order: it is a request for this
    /// machine; its device is a member, and the signature is the device's; the time is within the
    /// window of `now`, and not before this machine started; the nonce was not seen in the window;
    /// the device's role allows the method's class, and a launch is for a runner (`runs_agents`).
    /// Until the signature passes, a refusal names nothing of the list. An accepted request's
    /// nonce is remembered.
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
            Err(error) => {
                return refused(format!(
                    "this is not a signed request: {}",
                    shown(&error.to_string())
                ));
            }
        };
        if request.machine != machine {
            return refused(format!(
                "this request is for {}, not for {machine}",
                shown(&request.machine)
            ));
        }
        let signed = &request.signed;
        let Some(member) = members.get(&signed.device).filter(|_| request.signed()) else {
            return refused("this request is not signed by a member".to_owned());
        };
        let window = REQUEST_WINDOW.as_millis() as u64;
        let now = milliseconds(now);
        if signed.at < now.saturating_sub(window) {
            return refused("this request is older than five minutes".to_owned());
        }
        if signed.at > now.saturating_add(window) {
            return refused(
                "this request is dated more than five minutes ahead of this machine's clock"
                    .to_owned(),
            );
        }
        if signed.at < self.started {
            return refused(
                "this request was signed before this machine started: send it again".to_owned(),
            );
        }
        self.latest = self.latest.max(now);
        self.forget(self.latest.saturating_sub(window));
        let seen = self.seen.entry(*signed.device.as_bytes()).or_default();
        if seen.iter().any(|(_, nonce)| *nonce == signed.nonce) {
            return refused("this request was received already".to_owned());
        }
        let Some(class) = class(&signed.method) else {
            return refused(format!(
                "{} is not a method of Flottille's",
                shown(&signed.method)
            ));
        };
        if class == Class::Local {
            return refused(format!(
                "{} is answered only on its own machine, never over a link",
                signed.method
            ));
        }
        if member.role == Role::Viewer && class == Class::Drive {
            return refused(format!(
                "{} may only view: a controller or a runner may {}",
                member.name,
                deed(&signed.method)
            ));
        }
        runs_agents(members, machine, &signed.method)?;
        if seen.len() >= NONCES_PER_DEVICE {
            return refused(format!(
                "{} sent more than {NONCES_PER_DEVICE} requests in five minutes",
                member.name
            ));
        }
        let params = match serde_json::from_str(&signed.params) {
            Ok(params) => params,
            Err(error) => {
                return refused(format!(
                    "its params are not JSON: {}",
                    shown(&error.to_string())
                ));
            }
        };
        seen.push((signed.at, signed.nonce));
        Ok(Accepted {
            device: member.clone(),
            id: request.id,
            method: request.signed.method,
            params,
        })
    }

    /// Forgets the nonces of requests signed before `floor`, which the time check refuses while the
    /// clock goes forward.
    fn forget(&mut self, floor: u64) {
        self.seen.retain(|_, seen| {
            seen.retain(|(at, _)| *at >= floor);
            !seen.is_empty()
        });
    }
}

/// The methods that start a session: a machine whose member is no runner runs no agent.
const LAUNCHES: &[&str] = &["agent.start", "agent.run"];

/// Refused when `method` starts a session and `machine` is the machine of a member that is no
/// runner (`docs/remote.md`, "Roles"): the one check of it, made by the machine that would
/// carry the request and by the one it is for. A name that is no member's passes: the carrier
/// says it is no machine of the user's.
pub fn runs_agents(members: &Members, machine: &str, method: &str) -> Result<(), RequestError> {
    if !LAUNCHES.contains(&method) {
        return Ok(());
    }
    match members
        .members()
        .iter()
        .find(|member| member.name == machine)
    {
        Some(member) if member.role != Role::Runner => Err(RequestError(format!(
            "{machine} is a {}: it runs no agent, a runner does",
            member.role.word()
        ))),
        _ => Ok(()),
    }
}

/// What `method` does, as a refusal names it: the user's words for what they do most, the
/// method's name otherwise.
fn deed(method: &str) -> String {
    let words = match method {
        "approval.approve" | "approval.deny" => "answer a request",
        "agent.answer" => "answer a question",
        "agent.send" | "agent.send_now" => "send a message",
        "agent.start" | "agent.run" => "start a session",
        "agent.stop" => "stop a session",
        "agent.cancel" => "stop a turn",
        "agent.mode" => "switch the mode",
        "remote.invite" => "invite a device",
        "remote.refuse" => "refuse a device",
        "remote.change" => "add, remove or rename a device",
        _ => return format!("call {method}"),
    };
    words.to_owned()
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

/// The 4-byte big-endian length prefix of a field `length` bytes long.
fn prefix(length: usize) -> Option<[u8; 4]> {
    u32::try_from(length).ok().map(u32::to_be_bytes)
}

/// The longest part of a text a refusal shows.
const SHOWN: usize = 64;

/// `text` as a refusal shows it, the sender's: its first 64 characters (`serial::shown`).
fn shown(text: &str) -> String {
    crate::serial::shown(text, SHOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_longer_than_its_length_prefix_is_refused() {
        assert_eq!(prefix(u32::MAX as usize), Some([0xff; 4]));
        assert_eq!(prefix(u32::MAX as usize + 1), None);
    }
}
