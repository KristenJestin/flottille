//! The links between the user's devices, carried by the hub (`docs/remote.md`, "The transport",
//! "The hub"): each packet a device sends for another, in its control session, is handed to the
//! session of the device whose key it names, said to be from the sender. A packet is a QUIC
//! packet the two devices encrypted end to end: the hub never opens it, and keeps none of it
//! beyond the queue of the device it goes to.
//!
//! The forwarding holds the sessions open and the members' keys, which the server sets again each
//! time the member list changes (`server.rs`): a packet takes neither the hub's lock nor its list.
//! Each packet is charged to its sender first, whatever becomes of it, so that one the hub drops
//! costs as much as one it forwards; then both sides must be members', the other's session open,
//! and the packets it holds from this sender fewer than `RELAY_QUEUE`. The rate of a sender is
//! `RELAY_RATE`, with `RELAY_BURST` at once. A packet past a bound is dropped, as a network drops
//! one: the two devices' QUIC sends it again, slower, and nothing else waits for it.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use flottille_link::{PublicKey, RELAY_PACKET};
use tokio::sync::mpsc;

/// The bytes a second one device may send the others through the hub, past its burst.
pub const RELAY_RATE: u64 = 4 << 20;
/// The bytes one device may send at once through the hub, before its rate holds it.
pub const RELAY_BURST: u64 = 1 << 20;
/// The packets of one sender waiting for one device on the hub: 128 of at most 1500 bytes, under
/// 200 KiB. Each sender has its own share, so that one that floods a device leaves the others
/// their way to it.
pub const RELAY_QUEUE: usize = 128;

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
    sessions: HashMap<[u8; 32], Place>,
    buckets: HashMap<[u8; 32], Bucket>,
    /// The members' devices and machines: the only keys links go between.
    members: HashSet<[u8; 32]>,
}

/// An open session: where its packets go, and how many wait from each sender. The channel holds
/// at most `RELAY_QUEUE` for each member that sends.
struct Place {
    id: u64,
    key: PublicKey,
    sender: mpsc::UnboundedSender<Packet>,
    waiting: HashMap<[u8; 32], usize>,
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
    inbox: mpsc::UnboundedReceiver<Packet>,
}

impl Opened<'_> {
    /// The next packet for the device, and the key of the device that sent it.
    pub(crate) async fn recv(&mut self) -> Option<Packet> {
        let packet = self.inbox.recv().await?;
        let mut open = self.relay.open.lock().unwrap();
        if let Some(place) = open
            .sessions
            .get_mut(self.key.as_bytes())
            .filter(|place| place.id == self.id)
            && let Some(waiting) = place.waiting.get_mut(packet.0.as_bytes())
        {
            *waiting -= 1;
            if *waiting == 0 {
                place.waiting.remove(packet.0.as_bytes());
            }
        }
        Some(packet)
    }
}

impl Drop for Opened<'_> {
    fn drop(&mut self) {
        let mut open = self.relay.open.lock().unwrap();
        let key = self.key.as_bytes();
        if open
            .sessions
            .get(key)
            .is_some_and(|place| place.id == self.id)
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
        let (sender, inbox) = mpsc::unbounded_channel();
        let place = Place {
            id,
            key,
            sender,
            waiting: HashMap::new(),
        };
        self.open
            .lock()
            .unwrap()
            .sessions
            .insert(*key.as_bytes(), place);
        Opened {
            relay: self,
            key,
            id,
            inbox,
        }
    }

    /// The members' keys, devices and machines, in place of those before: set each time the list
    /// changes.
    pub(crate) fn set_members(&self, members: HashSet<[u8; 32]>) {
        self.open.lock().unwrap().members = members;
    }

    /// The members' keys whose session is open.
    pub(crate) fn online(&self) -> Vec<PublicKey> {
        let open = self.open.lock().unwrap();
        let mut keys: Vec<PublicKey> = open
            .sessions
            .values()
            .map(|place| place.key)
            .filter(|key| open.members.contains(key.as_bytes()))
            .collect();
        keys.sort_by(|one, other| one.as_bytes().cmp(other.as_bytes()));
        keys
    }

    /// Hands `packet` from `from` to the session of `to`, charged to `from` first: if it is under
    /// its rate, the packet no longer than a link sends, both members', the session of `to` open
    /// and holding fewer than `RELAY_QUEUE` of its packets. Drops it otherwise.
    pub(crate) fn forward(&self, from: PublicKey, to: &PublicKey, packet: Vec<u8>) {
        let mut open = self.open.lock().unwrap();
        let open = &mut *open;
        let allowed = open
            .buckets
            .entry(*from.as_bytes())
            .or_insert_with(Bucket::new)
            .take(packet.len());
        if !allowed
            || packet.len() > RELAY_PACKET
            || !open.members.contains(from.as_bytes())
            || !open.members.contains(to.as_bytes())
        {
            return;
        }
        let Some(place) = open.sessions.get_mut(to.as_bytes()) else {
            return;
        };
        let waiting = place.waiting.entry(*from.as_bytes()).or_default();
        if *waiting < RELAY_QUEUE && place.sender.send((from, packet)).is_ok() {
            *waiting += 1;
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
    fn relay_of(members: &[PublicKey]) -> Relay {
        let relay = Relay::default();
        relay.set_members(members.iter().map(|key| *key.as_bytes()).collect());
        relay
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

    #[test]
    fn a_member_removed_is_dropped_while_its_session_is_open() {
        let (_dir, [pc, server, laptop]) = keys();
        let relay = relay_of(&[pc, server, laptop]);
        let mut to_laptop = relay.open(laptop);
        let mut to_server = relay.open(server);

        // The laptop removed, its session not closed yet: nothing goes to it, nor from it.
        relay.set_members([*pc.as_bytes(), *server.as_bytes()].into());
        relay.forward(pc, &laptop, b"to the laptop".to_vec());
        relay.forward(laptop, &server, b"from the laptop".to_vec());
        assert!(to_laptop.inbox.try_recv().is_err());
        assert!(to_server.inbox.try_recv().is_err());
        // Nor is it said online.
        assert_eq!(relay.online(), vec![server]);
    }
}
