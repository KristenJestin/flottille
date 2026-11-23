//! The version handshake (`docs/remote.md`, "The version"): what each side says when two devices
//! link or a device connects to the hub, and the protocol they then use. Pure: no I/O, so the link
//! calls it once it has a transport.

use std::fmt;

use flottille_protocol::PROTOCOL;
use serde::{Deserialize, Serialize};

/// What a side says first: its Flottille version and the protocol versions it speaks. A list, so
/// that an app published apart from the daemon can speak two; one number today.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub flottille: String,
    pub protocols: Vec<u32>,
}

impl Hello {
    /// This build's hello. The caller names the version, as the daemon says it in
    /// `daemon.status`: it is the daemon's build that knows it, not this crate.
    pub fn ours(version: &str) -> Self {
        Self {
            flottille: version.to_owned(),
            protocols: vec![PROTOCOL],
        }
    }

    /// The newest protocol this side speaks, `0` for a side that says none.
    pub fn newest(&self) -> u32 {
        self.protocols.iter().copied().max().unwrap_or(0)
    }
}

/// The highest protocol both sides speak. With none in common the link is refused on both sides:
/// each calls this with its own hello and the other's, and gets the same refusal.
///
/// `peer` names the other side as the user knows it (`server`), for the sentence.
pub fn choose(ours: &Hello, theirs: &Hello, peer: &str) -> Result<u32, NeedsUpdate> {
    ours.protocols
        .iter()
        .filter(|protocol| theirs.protocols.contains(protocol))
        .copied()
        .max()
        .ok_or_else(|| NeedsUpdate {
            peer: Some(peer.to_owned()),
            theirs: theirs.clone(),
            ours: ours.clone(),
        })
}

/// [`choose`] for a device and its hub: the same refusal, said as the hub is said (`The hub runs
/// 0.1.0 (protocol 1), this machine 0.4.0 (protocol 2): update the hub on its host.`), which side
/// to update named by the newer protocol. `ours` is the device's side, whoever says it: the hub
/// makes the sentence its device reads from the device's hello and its own.
pub fn choose_hub(ours: &Hello, theirs: &Hello) -> Result<u32, NeedsUpdate> {
    choose(ours, theirs, "the hub").map_err(|mut refusal| {
        refusal.peer = None;
        refusal
    })
}

/// The refusal: no protocol in common. It displays as the sentence of the doc, which says which
/// side runs what, so that the user knows which to update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedsUpdate {
    /// The other side as the user knows it; `None` for the hub, which is told to update on its
    /// host.
    peer: Option<String>,
    theirs: Hello,
    ours: Hello,
}

impl NeedsUpdate {
    /// The peer's state while it is refused: never offline, it answers.
    pub fn state(&self) -> PeerState {
        PeerState::NeedsUpdate
    }
}

impl fmt::Display for NeedsUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(peer) = &self.peer else {
            // The side with the older protocol is the one to update: the hub, on its host, or
            // this machine.
            let update = match self.theirs.newest() < self.ours.newest() {
                true => "the hub on its host",
                false => "this machine",
            };
            return write!(
                f,
                "The hub runs {} (protocol {}), this machine {} (protocol {}): update {update}.",
                // Read before the hub has proven its key: anyone at its address writes it.
                crate::serial::shown(&self.theirs.flottille, 32),
                self.theirs.newest(),
                self.ours.flottille,
                self.ours.newest(),
            );
        };
        write!(
            f,
            "{} runs Flottille {} (protocol {}), this machine {} (protocol {}): update one of \
             them (flottille update)",
            peer,
            crate::serial::shown(&self.theirs.flottille, 32),
            self.theirs.newest(),
            self.ours.flottille,
            self.ours.newest(),
        )
    }
}

impl std::error::Error for NeedsUpdate {}

/// How the window and the app show a machine (`docs/remote.md`, "The version"). The link decides
/// `NeedsUpdate`; `Offline` is the transport's, here so that the three are one type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerState {
    Online,
    Offline,
    NeedsUpdate,
}

impl fmt::Display for PeerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::NeedsUpdate => "needs an update",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(version: &str, protocols: &[u32]) -> Hello {
        Hello {
            flottille: version.into(),
            protocols: protocols.to_vec(),
        }
    }

    #[test]
    fn the_highest_common_protocol_is_chosen() {
        let ours = hello("0.4.1", &[7, 8]);
        let theirs = hello("0.4.3", &[8, 9, 6]);
        assert_eq!(choose(&ours, &theirs, "server"), Ok(8));
        assert_eq!(choose(&theirs, &ours, "laptop"), Ok(8));
    }

    #[test]
    fn no_common_protocol_refuses_saying_which_to_update() {
        let ours = hello("0.4.1", &[8]);
        let theirs = hello("0.4.3", &[9]);
        let refusal = choose(&ours, &theirs, "server").unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "server runs Flottille 0.4.3 (protocol 9), this machine 0.4.1 (protocol 8): update \
             one of them (flottille update)"
        );
        // The other side refuses too, with the same facts from its own point of view.
        let other = choose(&theirs, &ours, "laptop").unwrap_err();
        assert_eq!(
            other.to_string(),
            "laptop runs Flottille 0.4.1 (protocol 8), this machine 0.4.3 (protocol 9): update \
             one of them (flottille update)"
        );
        // With lists, the sentence says the highest of each side.
        let refusal =
            choose(&hello("0.4.1", &[5, 6]), &hello("0.4.3", &[8, 9]), "server").unwrap_err();
        assert!(refusal.to_string().contains("(protocol 9)"), "{refusal}");
        assert!(refusal.to_string().contains("(protocol 6)"), "{refusal}");
    }

    #[test]
    fn an_old_hub_is_told_to_update_on_its_host() {
        let ours = hello("0.4.0", &[2]);
        let theirs = hello("0.1.0", &[1]);
        let refusal = choose_hub(&ours, &theirs).unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "The hub runs 0.1.0 (protocol 1), this machine 0.4.0 (protocol 2): update the hub on \
             its host."
        );
        assert_eq!(refusal.state(), PeerState::NeedsUpdate);
    }

    #[test]
    fn a_version_said_before_the_hub_is_proven_is_cut_and_shows_no_control() {
        // The hub's hello is read before it has proven its key: whoever answers at its address
        // writes it, so the sentence keeps 32 characters of it and no control character.
        let forged = format!("9.9.9\u{202e}\x1b[2J{}", "x".repeat(4000));
        let refusal = choose_hub(&hello("0.4.0", &[2]), &hello(&forged, &[1])).unwrap_err();
        let said = refusal.to_string();
        assert!(
            !said.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "{said:?}"
        );
        assert!(said.len() < 200, "{} characters", said.len());
        assert!(said.starts_with("The hub runs 9.9.9"), "{said}");
    }

    #[test]
    fn a_newer_hub_is_told_to_update_this_machine() {
        let refusal = choose_hub(&hello("0.4.0", &[2]), &hello("0.5.0", &[3])).unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "The hub runs 0.5.0 (protocol 3), this machine 0.4.0 (protocol 2): update this \
             machine."
        );
        assert_eq!(
            choose_hub(&hello("0.4.0", &[2]), &hello("0.4.1", &[2])),
            Ok(2)
        );
    }

    #[test]
    fn a_peer_without_a_common_protocol_needs_an_update_not_offline() {
        let refusal = choose(&hello("0.4.1", &[8]), &hello("0.4.3", &[9]), "server").unwrap_err();
        assert_eq!(refusal.state(), PeerState::NeedsUpdate);
        assert_ne!(refusal.state(), PeerState::Offline);
        assert_eq!(refusal.state().to_string(), "needs an update");
    }

    #[test]
    fn our_hello_speaks_this_builds_protocol() {
        let ours = Hello::ours("0.4.1");
        assert_eq!(ours.protocols, vec![PROTOCOL]);
        assert_eq!(choose(&ours, &Hello::ours("0.4.1"), "server"), Ok(PROTOCOL));
    }

    #[test]
    fn a_hello_travels_as_json() {
        let sent = hello("0.4.3", &[8, 9]);
        let json = serde_json::to_string(&sent).unwrap();
        assert_eq!(json, r#"{"flottille":"0.4.3","protocols":[8,9]}"#);
        assert_eq!(serde_json::from_str::<Hello>(&json).unwrap(), sent);
    }
}
