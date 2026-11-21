//! The links between the user's devices, carried by the hub (`docs/remote.md`, "The transport",
//! "The hub"): each packet a device sends for another, in its control session, is handed to the
//! session of the device whose key it names, said to be from the sender. A packet is a QUIC
//! packet the two devices encrypted end to end: the hub never opens it, and keeps none of it
//! beyond the queue of the device it goes to.
//!
//! The server (`server.rs`) asks the member list whether both sides are members' before it hands a
//! packet here: the forwarding knows only the sessions open, and bounds what each device costs
//! the hub, its packets waiting (`RELAY_QUEUE` for each device) and its rate (`RELAY_RATE`, with
//! `RELAY_BURST` at once). A packet past a bound is dropped, as a network drops one: the two
//! devices' QUIC sends it again, slower, and nothing else waits for it.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use flottille_link::PublicKey;
use tokio::sync::mpsc;

/// The bytes a second one device may send the others through the hub, past its burst.
pub const RELAY_RATE: u64 = 4 << 20;
/// The bytes one device may send at once through the hub, before its rate holds it.
pub const RELAY_BURST: u64 = 1 << 20;
/// The packets waiting for one device on the hub: 256 of at most 1500 bytes, under 400 KiB.
pub const RELAY_QUEUE: usize = 256;

/// A packet for a device, and the key of the device that sent it.
pub(crate) type Packet = (PublicKey, Vec<u8>);

/// The sessions open, each by its device's key, and what each sender may still send.
#[derive(Default)]
pub(crate) struct Relay {
    open: Mutex<Open>,
    made: AtomicU64,
}

#[derive(Default)]
struct Open {
    sessions: HashMap<[u8; 32], (u64, PublicKey, mpsc::Sender<Packet>)>,
    buckets: HashMap<[u8; 32], Bucket>,
}

/// What a device may still send: tokens of a byte, given back at `RELAY_RATE`, `RELAY_BURST` at
/// most.
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new() -> Bucket {
        Bucket {
            tokens: RELAY_BURST as f64,
            at: Instant::now(),
        }
    }

    /// Whether `bytes` may go now, taken if so.
    fn take(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        let given = now.duration_since(self.at).as_secs_f64() * RELAY_RATE as f64;
        self.tokens = (self.tokens + given).min(RELAY_BURST as f64);
        self.at = now;
        if self.tokens < bytes as f64 {
            return false;
        }
        self.tokens -= bytes as f64;
        true
    }
}

/// A session's place among those open: what is forwarded to its device comes in on `inbox`. Let
/// go, the session is no longer open.
pub(crate) struct Opened<'a> {
    relay: &'a Relay,
    key: PublicKey,
    id: u64,
    pub(crate) inbox: mpsc::Receiver<Packet>,
}

impl Drop for Opened<'_> {
    fn drop(&mut self) {
        let mut open = self.relay.open.lock().unwrap();
        let key = self.key.as_bytes();
        if open
            .sessions
            .get(key)
            .is_some_and(|(id, _, _)| *id == self.id)
        {
            // Its bucket stays: a device that connects again starts where it was.
            open.sessions.remove(key);
        }
    }
}

impl Relay {
    /// The session of `key` is open: what is sent to `key` goes to it now, in place of any session
    /// of that key before.
    pub(crate) fn open(&self, key: PublicKey) -> Opened<'_> {
        let id = self.made.fetch_add(1, Ordering::Relaxed);
        let (sender, inbox) = mpsc::channel(RELAY_QUEUE);
        self.open
            .lock()
            .unwrap()
            .sessions
            .insert(*key.as_bytes(), (id, key, sender));
        Opened {
            relay: self,
            key,
            id,
            inbox,
        }
    }

    /// The keys whose session is open.
    pub(crate) fn online(&self) -> Vec<PublicKey> {
        let open = self.open.lock().unwrap();
        let mut keys: Vec<PublicKey> = open.sessions.values().map(|(_, key, _)| *key).collect();
        keys.sort_by(|one, other| one.as_bytes().cmp(other.as_bytes()));
        keys
    }

    /// Hands `packet` from `from` to the session of `to`, if it is open, the sender under its rate
    /// and the queue of `to` not full; drops it otherwise. Both are members': the server checked.
    pub(crate) fn forward(&self, from: PublicKey, to: &PublicKey, packet: Vec<u8>) {
        let mut open = self.open.lock().unwrap();
        let Some((_, _, sender)) = open.sessions.get(to.as_bytes()) else {
            return;
        };
        let sender = sender.clone();
        let allowed = open
            .buckets
            .entry(*from.as_bytes())
            .or_insert_with(Bucket::new)
            .take(packet.len());
        drop(open);
        if allowed {
            let _ = sender.try_send((from, packet));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three members' keys, made in a temporary folder.
    fn keys() -> (tempfile::TempDir, [PublicKey; 3]) {
        let dir = tempfile::tempdir().unwrap();
        let key = |name: &str| {
            flottille_link::MachineKey::load_or_make(&dir.path().join(name))
                .unwrap()
                .public()
        };
        let keys = [key("pc"), key("server"), key("laptop")];
        (dir, keys)
    }

    /// A relay between the members `members`.
    fn relay_of(_members: &[PublicKey]) -> Relay {
        Relay::default()
    }

    #[test]
    fn a_second_sender_still_reaches_a_device_another_floods() {
        let (_dir, [pc, server, laptop]) = keys();
        let relay = relay_of(&[pc, server, laptop]);
        let mut opened = relay.open(server);

        // The pc fills what waits for the server, which reads nothing for now; the laptop's packet
        // still finds its place.
        for _ in 0..RELAY_QUEUE * 2 {
            relay.forward(pc, &server, vec![7; 1000]);
        }
        relay.forward(laptop, &server, b"from the laptop".to_vec());
        let mut got = Vec::new();
        while let Ok(packet) = opened.inbox.try_recv() {
            got.push(packet);
        }
        assert!(got.contains(&(laptop, b"from the laptop".to_vec())));
        assert!(got.len() < RELAY_QUEUE * 2, "{} packets kept", got.len());
    }
}
