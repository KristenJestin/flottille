//! The daemon's control session to the user's hub (`docs/remote.md`, "The hub", "Architecture"),
//! with remote on and the machine set up: it keeps the member list, checked back to the root
//! pinned at setup, and the addresses the hub tells of the members' machines, and writes from them
//! the machines the link trusts (`remote/peers.json`); it tells the hub this machine's addresses.
//!
//! The hub is not trusted with the members: a list that does not check is refused here, whatever
//! the hub said, and the machines trusted stay those of the last list that checked. While the hub
//! is unreachable, the links and the machines known go on; what needs the hub says so, with since
//! when.
//!
//! What the machine pinned is `remote/hub.json` (`flottille_link::Device`): the hub's URL and
//! key, the list's root, and the list as last checked. `remote.setup` writes it, and so does a
//! join (`remote.join`, then `remote.declare`), once the member that invited accepted it and this
//! machine is declared.
//!
//! The user's actions on the list (an invitation's ask, a change) are signed by the clients with
//! the device key, which the daemon never holds; the daemon carries them over its session, and
//! says what changed in the list it checked as events (`docs/events.md`, `remote.*`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_link::{
    Action, Ask, Asker, Change, ControlError, Device, HubUrl, Invitation, InvitationAsk, Joined,
    ListError, MachineKey, MemberList, Members, PublicKey, Pushed, Requested, Role, Session, Tls,
    Token, verification_words,
};
use serde_json::{Value, json};

use crate::link::{Emit, Remote};

/// The machine's hub, as it pinned it.
pub const HUB_FILE: &str = "hub.json";
/// The longest a hub that cannot be reached waits before it is tried again.
const LONGEST_WAIT: Duration = Duration::from_secs(30);
/// How often this machine's addresses are compared with those the hub was told.
const ROUND: Duration = Duration::from_secs(1);
/// How long the hub may take to answer an ask before it is asked again.
const ASK_WAIT: Duration = Duration::from_secs(10);
/// How long an invitation is valid, in seconds (`docs/remote.md`, "Pairing").
const FIVE_MINUTES: u64 = 5 * 60;

/// How the daemon sees its hub.
struct Seen {
    state: &'static str,
    /// Since when, in milliseconds since the Unix epoch.
    since: u64,
    reason: Option<String>,
}

/// An invitation this machine's member was given by the hub: only a join request for its token is
/// shown, and its words are drawn from this token, never from one the hub pushes back, which a
/// hub that lies could have searched so that the words match another key's.
struct Issued {
    token: Token,
    /// Until when it is valid, in seconds since the Unix epoch.
    until: u64,
}

/// A join this machine asked for, its session to the hub open.
struct Joining {
    joined: Joined,
    /// The hub pinned and the list checked, once the member that invited accepted it.
    device: Option<Device>,
}

/// The control session's side of the daemon.
pub struct Control {
    key: MachineKey,
    folder: PathBuf,
    seen: Mutex<Seen>,
    /// Raised when `hub.json` was written by a setup: the session starts again from it.
    again: tokio::sync::Notify,
    /// What asks the hub in the session kept, while there is one.
    asker: tokio::sync::watch::Sender<Option<Asker>>,
    /// The list as last checked, for the changes a client asks to wait for it.
    list: tokio::sync::watch::Sender<MemberList>,
    /// The join requests that wait for this machine's member, as the hub last told them.
    requests: Mutex<Vec<Requested>>,
    /// The invitations given to this machine's member, while they are valid.
    issued: Mutex<Vec<Issued>>,
    joining: tokio::sync::Mutex<Option<Joining>>,
    emit: Emit,
}

impl Control {
    pub fn new(key: MachineKey, folder: PathBuf, emit: Emit) -> Control {
        Control {
            key,
            folder,
            seen: Mutex::new(Seen {
                state: "none",
                since: now(),
                reason: None,
            }),
            again: tokio::sync::Notify::new(),
            asker: tokio::sync::watch::Sender::new(None),
            list: tokio::sync::watch::Sender::new(MemberList::default()),
            requests: Mutex::new(Vec::new()),
            issued: Mutex::new(Vec::new()),
            joining: tokio::sync::Mutex::new(None),
            emit,
        }
    }

    fn file(&self) -> PathBuf {
        self.folder.join(HUB_FILE)
    }

    /// The hub this machine pinned; `None` when it is not set up.
    fn pinned(&self) -> Option<Result<Device, String>> {
        match std::fs::read_to_string(self.file()) {
            Ok(text) => Some(serde_json::from_str(&text).map_err(|error| {
                format!(
                    "{} is not a hub this machine pinned: {error}",
                    self.file().display()
                )
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(Err(format!(
                "{} cannot be read: {error}",
                self.file().display()
            ))),
        }
    }

    fn keep(&self, device: &Device) -> std::io::Result<()> {
        crate::link::write_json(&self.file(), device)
    }

    /// The hub's state, said once in the log when it changes.
    fn set(&self, state: &'static str, reason: Option<String>) {
        let mut seen = self.seen.lock().unwrap();
        if seen.state != state || seen.reason != reason {
            match &reason {
                Some(reason) => eprintln!("flottille daemon: {reason}"),
                None => eprintln!("flottille daemon: the hub is {state}"),
            }
        }
        if seen.state != state {
            seen.since = now();
        }
        seen.state = state;
        seen.reason = reason;
    }

    /// The hub as `remote.machines` says it; `null` when this machine is not set up.
    pub fn seen(&self) -> Value {
        let Some(Ok(device)) = self.pinned() else {
            return Value::Null;
        };
        let seen = self.seen.lock().unwrap();
        json!({
            "url": device.url,
            "key": device.hub,
            "fingerprint": device.hub.fingerprint(),
            "state": seen.state,
            "since": seen.since,
            "reason": seen.reason,
        })
    }

    /// `remote.setup`: this machine, carrying its device's signed changes, becomes the hub's
    /// first member; the declaration of this machine is claimed here, with the machine's key. The
    /// hub's key is pinned from this session. A machine set up already is set up again only when
    /// asked to `replace` the hub it pinned.
    pub async fn setup(
        &self,
        url: &str,
        code: &str,
        mut changes: Vec<Change>,
        replace: bool,
    ) -> Result<Value, String> {
        if !replace && let Some(pinned) = self.pinned() {
            let url =
                pinned.map_or_else(|_| self.file().display().to_string(), |device| device.url);
            return Err(format!(
                "this machine is set up with the hub at {url} already: give --replace to set it \
                 up with another (the machines it trusts through that hub are forgotten)"
            ));
        }
        let own = self.key.public();
        match changes.get_mut(1) {
            Some(change) if matches!(&change.action, Action::Machine { machine, .. } if *machine == own) =>
            {
                *change = change.clone().claimed_by(&self.key);
            }
            _ => {
                return Err(
                    "the setup's second change must declare this machine, whose key is \
                     remote.machines' this.key"
                        .to_owned(),
                );
            }
        }
        let (device, session) = Device::setup(
            url,
            code,
            changes,
            &self.key,
            &crate::link::hello(),
            &Tls::System,
        )
        .await
        .map_err(|error| error.to_string())?;
        drop(session);
        self.keep(&device)
            .map_err(|error| format!("{} cannot be written: {error}", self.file().display()))?;
        eprintln!(
            "flottille daemon: this machine set up the hub at {}, whose key is {}",
            device.url,
            device.hub.fingerprint()
        );
        self.again.notify_one();
        Ok(json!({
            "url": device.url,
            "hub": device.hub.fingerprint(),
            "root": device.root.fingerprint(),
        }))
    }

    /// The hub this machine pinned, and the members of the list it last checked.
    fn checked(&self) -> Result<(Device, Members), String> {
        let device = match self.pinned() {
            None => return Err(NOT_SET_UP.to_owned()),
            Some(pinned) => pinned?,
        };
        let members = device.members().map_err(|error| error.to_string())?;
        Ok((device, members))
    }

    /// What asks the hub, once a session is up: a session made again a moment ago (after a
    /// setup, or a failure) is waited for, a few seconds at most.
    async fn asker(&self) -> Result<Asker, String> {
        let mut watching = self.asker.subscribe();
        let waited = tokio::time::timeout(ASK_WAIT, watching.wait_for(Option::is_some)).await;
        if let Ok(Ok(asker)) = waited {
            return Ok(asker.clone().expect("waited for one"));
        }
        let seen = self.seen.lock().unwrap();
        Err(match &seen.reason {
            Some(reason) => format!("the hub cannot be asked now: {reason}"),
            None => "the hub cannot be asked now: this machine has no session to it".to_owned(),
        })
    }

    /// `remote.members`: the members of the list this machine last checked.
    pub fn members(&self) -> Result<Value, String> {
        let (device, members) = self.checked()?;
        Ok(members_json(&device, &members, &self.key.public()))
    }

    /// `remote.invite`: a member's signed ask for an invitation, carried to the hub; the code to
    /// show, which is also the QR's text.
    pub async fn invite(&self, ask: InvitationAsk) -> Result<Value, String> {
        let (device, members) = self.checked()?;
        let Some(member) = members.get(&ask.member) else {
            return Err(format!(
                "{} is not a member of this machine's list: set this device up, or join first",
                ask.member.fingerprint()
            ));
        };
        let name = member.name.clone();
        let token = tokio::time::timeout(ASK_WAIT, self.asker().await?.invite(ask.clone()))
            .await
            .map_err(|_| format!("the hub did not answer in {} s", ASK_WAIT.as_secs()))?
            .map_err(|error| error.to_string())?;
        let until = ask.at + FIVE_MINUTES;
        {
            let mut issued = self.issued.lock().unwrap();
            issued.retain(|issued| issued.until > now() / 1000);
            issued.push(Issued { token, until });
        }
        let invitation = Invitation {
            hub_url: device.url.clone(),
            hub_key: device.hub,
            token,
            inviter: ask.member,
        };
        let code = invitation.code().map_err(|error| error.to_string())?;
        Ok(json!({
            "code": code,
            "url": device.url,
            "hub": device.hub.fingerprint(),
            "inviter": name,
            "fingerprint": ask.member.fingerprint(),
            "expires": until * 1000,
        }))
    }

    /// `remote.requests`: the join requests that wait for this machine's member, with the four
    /// words to compare with the new device's.
    pub fn requests(&self) -> Result<Value, String> {
        let (_, members) = self.checked()?;
        let Some(this) = members.by_machine(&self.key.public()) else {
            return Ok(json!([]));
        };
        let requests = self.issued_requests(self.requests.lock().unwrap().clone());
        Ok(Value::Array(
            requests
                .iter()
                .map(|request| request_json(request, &this.key))
                .collect(),
        ))
    }

    /// `remote.refuse`: the member refuses the join request of the new device `request`, its key's
    /// fingerprint.
    pub async fn refuse(&self, request: &str) -> Result<Value, String> {
        let token = self
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|waiting| waiting.key.fingerprint() == request)
            .map(|waiting| waiting.token)
            .ok_or_else(|| no_request(request))?;
        tokio::time::timeout(ASK_WAIT, self.asker().await?.ask(Ask::Refuse { token }))
            .await
            .map_err(|_| format!("the hub did not answer in {} s", ASK_WAIT.as_secs()))?
            .map_err(|error| error.to_string())?;
        Ok(json!({"refused": request}))
    }

    /// `remote.change`: a member's signed change of the list (an addition that accepts a join
    /// request, a removal, a renaming), carried to the hub; answered once the hub sent back a
    /// list that holds it and that this machine checked.
    pub async fn change(&self, change: Change) -> Result<Value, String> {
        let hash = change.hash();
        let mut watching = self.list.subscribe();
        tokio::time::timeout(ASK_WAIT, self.asker().await?.ask(Ask::Submit { change }))
            .await
            .map_err(|_| format!("the hub did not answer in {} s", ASK_WAIT.as_secs()))?
            .map_err(|error| error.to_string())?;
        let holds = |list: &MemberList| list.changes().iter().any(|change| change.hash() == hash);
        tokio::time::timeout(ASK_WAIT, watching.wait_for(holds))
            .await
            .map_err(|_| {
                format!(
                    "the hub withheld this change: it sent back no list holding it in {} s",
                    ASK_WAIT.as_secs()
                )
            })?
            .map_err(|_| "the session to the hub ended".to_owned())?;
        self.members()
    }

    /// `remote.join`: this machine, set up with no hub, joins with the invitation `code` and the
    /// new device's key, `key`: it reaches the invitation's hub, which holds the request until
    /// the member that invited accepts or refuses it, and answers once it did, with the list's
    /// head after which the device declares this machine (`remote.declare`).
    pub async fn join(&self, code: &str, key: PublicKey) -> Result<Value, String> {
        if let Some(pinned) = self.pinned() {
            let url =
                pinned.map_or_else(|_| self.file().display().to_string(), |device| device.url);
            return Err(format!(
                "this machine is set up with the hub at {url} already: a machine joins one hub"
            ));
        }
        let invitation = Invitation::from_code(code).map_err(|error| error.to_string())?;
        let mut joining = self.joining.lock().await;
        // A join asked again replaces the one before, whose session ends.
        *joining = None;
        let mut joined = Device::join(
            &invitation,
            &key,
            &self.key,
            &crate::link::hello(),
            &Tls::System,
        )
        .await
        .map_err(|error| error.to_string())?;
        eprintln!(
            "flottille daemon: asked the hub at {} to join, invited by {}",
            invitation.hub_url,
            invitation.inviter.fingerprint()
        );
        let (device, members) = joined.accepted().await.map_err(|error| error.to_string())?;
        let entry = members
            .get(&key)
            .expect("the list checked holds the device's entry");
        let added_by = members.get(&entry.added_by).map_or_else(
            || entry.added_by.fingerprint(),
            |member| member.name.clone(),
        );
        let answer = json!({
            "head": device.list.head(),
            "name": entry.name,
            "role": entry.role,
            "added_by": added_by,
            "hub": device.hub.fingerprint(),
        });
        *joining = Some(Joining {
            joined,
            device: Some(device),
        });
        Ok(answer)
    }

    /// `remote.declare`: the declaration of this machine by the device that joined, signed by it
    /// after the head `remote.join` answered; claimed here with the machine's key and carried to
    /// the hub. Once the hub sent it back, the hub is pinned (`hub.json`) and the session kept.
    pub async fn declare(&self, change: Change) -> Result<Value, String> {
        let mut joining = self.joining.lock().await;
        let Some(Joining {
            joined,
            device: Some(device),
        }) = joining.as_mut()
        else {
            return Err("this machine has no join accepted: run flottille remote join".to_owned());
        };
        let own = self.key.public();
        let declares = matches!(&change.action, Action::Machine { machine, .. } if *machine == own);
        if !declares {
            return Err(
                "the change must declare this machine, whose key is remote.machines' this.key"
                    .to_owned(),
            );
        }
        let members = device
            .submit(&mut joined.session, change.claimed_by(&self.key))
            .await
            .map_err(|error| error.to_string())?;
        self.keep(device)
            .map_err(|error| format!("{} cannot be written: {error}", self.file().display()))?;
        let device = device.clone();
        *joining = None;
        drop(joining);
        let _ = self.list.send(device.list.clone());
        let this = members.by_machine(&own).cloned();
        if let Some(this) = &this {
            (self.emit)(
                "remote.member_added",
                added_json(&this.name, this.role, &this.key, &members, &this.added_by),
            );
        }
        eprintln!(
            "flottille daemon: this machine joined the hub at {}",
            device.url
        );
        self.again.notify_one();
        Ok(json!({
            "url": device.url,
            "hub": device.hub.fingerprint(),
            "root": device.root.fingerprint(),
            "name": this.as_ref().map(|this| this.name.clone()),
        }))
    }

    /// A list the session took: kept for those who wait for it, and what changed since `before`
    /// said as events.
    fn took(&self, before: Option<&Members>, list: &MemberList) {
        let _ = self.list.send(list.clone());
        let Some(before) = before else {
            return;
        };
        let known = before.head();
        let Some(from) = list
            .changes()
            .iter()
            .position(|change| change.previous == known)
        else {
            return;
        };
        // The names as the list moves on, to say who signed each change.
        let mut names: HashMap<[u8; 32], String> = before
            .members()
            .iter()
            .map(|member| (*member.key.as_bytes(), member.name.clone()))
            .collect();
        for change in &list.changes()[from..] {
            let by = names
                .get(change.signer.as_bytes())
                .cloned()
                .unwrap_or_else(|| change.signer.fingerprint());
            match &change.action {
                Action::Add { key, name, role } => {
                    names.insert(*key.as_bytes(), name.clone());
                    (self.emit)(
                        "remote.member_added",
                        json!({
                            "name": name,
                            "fingerprint": key.fingerprint(),
                            "role": role,
                            "added_by": by,
                            "at": now(),
                        }),
                    );
                }
                Action::Remove { key } => {
                    let name = names
                        .remove(key.as_bytes())
                        .unwrap_or_else(|| key.fingerprint());
                    (self.emit)(
                        "remote.member_removed",
                        json!({
                            "name": name,
                            "fingerprint": key.fingerprint(),
                            "removed_by": by,
                            "at": now(),
                        }),
                    );
                }
                Action::Rename { key, name } => {
                    let from = names
                        .insert(*key.as_bytes(), name.clone())
                        .unwrap_or_else(|| key.fingerprint());
                    (self.emit)(
                        "remote.member_renamed",
                        json!({
                            "name": name,
                            "from": from,
                            "fingerprint": key.fingerprint(),
                            "renamed_by": by,
                            "at": now(),
                        }),
                    );
                }
                Action::Machine { .. } => {}
            }
        }
    }

    /// Of the join requests the hub told, those for an invitation this machine's member was given
    /// and that is still valid, with the token it was given.
    fn issued_requests(&self, requests: Vec<Requested>) -> Vec<Requested> {
        let now = now() / 1000;
        let issued = self.issued.lock().unwrap();
        requests
            .into_iter()
            .filter_map(|request| {
                let given = issued
                    .iter()
                    .find(|issued| issued.until > now && issued.token == request.token)?;
                Some(Requested {
                    token: given.token,
                    key: request.key,
                })
            })
            .collect()
    }

    /// The join requests the hub told: kept, and each new one said as an event.
    fn told_requests(&self, requests: Vec<Requested>, members: Option<&Members>) {
        let requests = self.issued_requests(requests);
        let mut kept = self.requests.lock().unwrap();
        let inviter = members.and_then(|members| members.by_machine(&self.key.public()));
        if let Some(inviter) = inviter {
            for request in requests.iter().filter(|request| !kept.contains(request)) {
                (self.emit)("remote.join_requested", request_json(request, &inviter.key));
            }
        }
        *kept = requests;
    }
}

const NOT_SET_UP: &str =
    "this machine is not set up with a hub: flottille remote setup, or flottille remote join";

fn no_request(request: &str) -> String {
    format!(
        "no join request of {request} waits for this machine: flottille remote requests lists them"
    )
}

/// What is said of a member added.
fn added_json(name: &str, role: Role, key: &PublicKey, members: &Members, by: &PublicKey) -> Value {
    let by = members
        .get(by)
        .map_or_else(|| by.fingerprint(), |member| member.name.clone());
    json!({
        "name": name,
        "fingerprint": key.fingerprint(),
        "role": role,
        "added_by": by,
        "at": now(),
    })
}

/// A join request as the methods and events say it: the new device's key, its fingerprint, which
/// names the request, and the four words both screens show.
fn request_json(request: &Requested, inviter: &PublicKey) -> Value {
    json!({
        "request": request.key.fingerprint(),
        "key": request.key,
        "words": verification_words(&request.key, inviter, &request.token),
        "inviter": inviter.fingerprint(),
    })
}

/// The members of a checked list, as `remote.members` says them.
fn members_json(device: &Device, members: &Members, own: &PublicKey) -> Value {
    let name_of = |key: &PublicKey| {
        members
            .get(key)
            .map_or_else(|| key.fingerprint(), |member| member.name.clone())
    };
    json!({
        "hub": device.url,
        "root": name_of(&members.root()),
        "head": members.head(),
        "this": members.by_machine(own).map(|member| member.name.clone()),
        "members": members.members().iter().map(|member| json!({
            "name": member.name,
            "key": member.key,
            "fingerprint": member.key.fingerprint(),
            "role": member.role,
            "added_by": name_of(&member.added_by),
            "machine": member.machine.map(|machine| machine.fingerprint()),
        })).collect::<Vec<_>>(),
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

/// Keeps the session to the hub while the daemon runs: made again after each failure, waiting
/// longer each time, or at once after a setup.
pub async fn run(
    control: Arc<Control>,
    remote: Arc<Remote>,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) {
    let mut wait = ROUND;
    // When the hub was last reached, to say since when it is not.
    let mut unreachable_since: Option<jiff::Zoned> = None;
    // Whether a session to the hub was had and lost since the daemon started.
    let mut lost = false;
    loop {
        let pinned = match control.pinned() {
            None => None,
            Some(Ok(device)) => Some(device),
            Some(Err(why)) => {
                control.set("refused", Some(why));
                None
            }
        };
        if pinned.is_none() {
            remote.heard_hub();
        }
        if let Some(device) = pinned {
            let authority = HubUrl::parse(&device.url)
                .map(|url| url.authority())
                .unwrap_or_else(|_| device.url.clone());
            let hello = crate::link::hello();
            let connected = tokio::select! {
                connected = device.connect(&control.key, &hello, &Tls::System) => connected,
                _ = stopped(&mut stopping) => return,
            };
            let why = match connected {
                Ok(session) => {
                    unreachable_since = None;
                    wait = ROUND;
                    control.set("connected", None);
                    let ended =
                        keep_session(&control, &remote, device, session, &mut stopping).await;
                    // Held until the next session's list, `FIRST_LIST` at most: a machine that
                    // was asleep during a removal does not link by the list it kept meanwhile.
                    remote.lost_hub();
                    lost = true;
                    match ended {
                        Ended::Stopping => return,
                        Ended::Again => continue,
                        Ended::Lost(why) => Some(why),
                    }
                }
                Err(ControlError::Reset) => {
                    control.set("reset", Some(ListError::Reset.to_string()));
                    None
                }
                Err(error @ (ControlError::AnotherKey { .. } | ControlError::Refused { .. })) => {
                    control.set("refused", Some(error.to_string()));
                    None
                }
                Err(ControlError::NeedsUpdate(refusal)) => {
                    control.set("needs an update", Some(refusal.to_string()));
                    None
                }
                Err(ControlError::Unreachable { why, .. } | ControlError::Lost { why, .. }) => {
                    Some(why)
                }
            };
            // No list from the hub at start: the links go on with the one kept. After a session
            // was lost, they wait for the bound.
            if !lost {
                remote.heard_hub();
            }
            if let Some(why) = why {
                let since = unreachable_since.get_or_insert_with(jiff::Zoned::now);
                control.set(
                    "unreachable",
                    Some(format!(
                        "the hub at {authority} is unreachable since {}: {why}",
                        since.strftime("%H:%M")
                    )),
                );
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = control.again.notified() => {}
            _ = stopped(&mut stopping) => return,
        }
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

enum Ended {
    Stopping,
    /// A setup wrote another hub: start from it.
    Again,
    Lost(String),
}

/// One session, until it ends: the lists and addresses the hub sends, checked and kept; this
/// machine's addresses told whenever they change.
async fn keep_session(
    control: &Control,
    remote: &Arc<Remote>,
    mut device: Device,
    mut session: Session,
    stopping: &mut tokio::sync::watch::Receiver<bool>,
) -> Ended {
    let mut told: Option<Vec<String>> = None;
    let mut addresses: HashMap<[u8; 32], Vec<String>> = HashMap::new();
    let mut members: Option<Members> = device.members().ok();
    let mut round = tokio::time::interval(ROUND);
    let asker = session.asker();
    control.asker.send_replace(Some(asker.clone()));
    let _ = control.list.send(device.list.clone());
    // The asker is let go with the session, however it ends.
    struct Forget<'a>(&'a Control);
    impl Drop for Forget<'_> {
        fn drop(&mut self) {
            self.0.asker.send_replace(None);
        }
    }
    let _forget = Forget(control);
    // The ask of this machine's addresses on its way, at most one: dropped with the session.
    let mut telling = tokio::task::JoinSet::new();
    let mut said: Option<String> = None;
    loop {
        tokio::select! {
            pushed = session.next() => {
                let Some(pushed) = pushed else {
                    return Ended::Lost(session.ended());
                };
                match pushed {
                    Pushed::List(list) => match device.take(list) {
                        Ok(checked) => {
                            if let Err(error) = control.keep(&device) {
                                eprintln!(
                                    "flottille daemon: the member list was not kept: {error}"
                                );
                            }
                            control.set("connected", None);
                            control.took(members.as_ref(), &device.list);
                            members = Some(checked);
                        }
                        Err(refused) => {
                            let state = match refused {
                                ListError::Reset => "reset",
                                ListError::Refused(_) => "refused",
                            };
                            control.set(state, Some(refused.to_string()));
                            remote.heard_hub();
                            continue;
                        }
                    },
                    Pushed::Addresses(given) => {
                        addresses = given
                            .into_iter()
                            .map(|told| (*told.key.as_bytes(), told.addresses))
                            .collect();
                    }
                    // Read by the notifications, later (`docs/remote.md`, "Notifications").
                    Pushed::Presence(_) => continue,
                    Pushed::Requests(requests) => {
                        control.told_requests(requests, members.as_ref());
                        continue;
                    }
                }
                if let Some(members) = &members {
                    trust(remote, &control.key.public(), members, &addresses);
                    remote.heard_hub();
                }
            }
            _ = round.tick() => {
                let now = remote.told_addresses();
                if telling.is_empty() && told.as_ref() != Some(&now) {
                    // Answered later: the session goes on meanwhile.
                    let asker = asker.clone();
                    telling.spawn(async move {
                        let asked = asker.ask(Ask::Addresses { addresses: now.clone() });
                        let answer = match tokio::time::timeout(ASK_WAIT, asked).await {
                            Ok(answer) => answer.map_err(|error| error.to_string()),
                            Err(_) => Err(format!("it did not answer in {} s", ASK_WAIT.as_secs())),
                        };
                        (now, answer)
                    });
                }
            }
            Some(Ok((sent, answer))) = telling.join_next() => match answer {
                // Told only once the hub took them: a refusal is asked again at the next round.
                Ok(()) => {
                    told = Some(sent);
                    said = None;
                }
                Err(error) => {
                    if said.as_ref() != Some(&error) {
                        eprintln!("flottille daemon: the hub was not told this machine's addresses: {error}");
                    }
                    said = Some(error);
                }
            },
            _ = control.again.notified() => return Ended::Again,
            _ = stopped(stopping) => return Ended::Stopping,
        }
    }
}

/// The machines the link trusts: every member's machine but this one, named as the member, with
/// the addresses the hub told for it (those kept before, when the hub told none in this session).
fn trust(
    remote: &Remote,
    own: &PublicKey,
    members: &Members,
    addresses: &HashMap<[u8; 32], Vec<String>>,
) {
    if let Some(this) = members.by_machine(own) {
        remote.name_as(&this.name);
    }
    let machines = members
        .members()
        .iter()
        .filter_map(|member| {
            let machine = member.machine?;
            (machine != *own).then(|| {
                (
                    member.name.clone(),
                    machine,
                    addresses.get(machine.as_bytes()).cloned(),
                )
            })
        })
        .collect();
    remote.trust_machines(machines);
}

/// Until `stopping` says so; nothing held of it, so that a `select!` holding it stays `Send`.
async fn stopped(stopping: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}
