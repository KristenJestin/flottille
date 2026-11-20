//! Deliveries and the tree's calls across machines (`docs/remote.md`, "Deliveries across
//! machines", "Coming back"): a session started on this machine by an agent of another tells that
//! agent what it would tell a parent or a pilot here, and that agent writes to it, reads how it
//! does, and stops it, through its daemon.
//!
//! What goes to another machine is stored before it is sent (`outbox`): sent in order, once (its
//! id is idempotent there), when the link to that machine is up, and again whenever it comes
//! back; kept 30 days, then dropped with an entry in this machine's inbox. A report's commits go
//! back first, and the report names where they came. Each machine checks what comes against what
//! it recorded of the launch: a delivery only for the agent that launched it, a call only from
//! that agent, for the session it started.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::agent::Agent;
use flottille_core::approval;
use flottille_core::store::{Launch, LaunchFilter, Outgoing, now_ms};
use flottille_link::{Call, Deliver, LinkError, MachineMessage, Message, PublicKey, Received};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::deliveries;
use crate::link::Remote;
use crate::mcp::{Grant, Outcome};
use crate::tree::named;

/// How long a message waits for its machine before it is dropped.
pub(crate) const KEPT_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The tools of the tree a session on another machine answers.
const CALLED: [&str; 5] = [
    "agent_send",
    "agent_status",
    "agent_stop",
    "session_send",
    "session_read",
];

/// The kinds of delivery a session on another machine sends the agent that started it.
const KINDS: [&str; 9] = [
    "child-report",
    "child-question",
    "child-failed",
    "child-idle",
    "child-stuck",
    "child-asks",
    "child-ended",
    "session-turn-ended",
    "led-integrated",
];

/// The longest body of a delivery or text of a call another machine sends, in characters.
const LONGEST: usize = 64_000;

/// The machines whose kept messages are being sent now, each with whether more were kept since.
static SENDING: Mutex<BTreeMap<String, bool>> = Mutex::new(BTreeMap::new());

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn key_of(machine: &str) -> Option<PublicKey> {
    serde_json::from_value(json!(machine)).ok()
}

/// An id another machine gives: one ref-like word, short.
fn an_id(id: &str) -> bool {
    (1..=100).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// The machine the session runs on.

/// The launch that started the agent `agent` here for an agent of another machine.
fn launch_here(daemon: &Daemon, agent: &str) -> Option<Launch> {
    daemon
        .read(|store| store.launches("in", None, LaunchFilter::Agent(agent)))
        .ok()?
        .into_iter()
        .find(|launch| launch.state != "refused")
}

/// Whether `agent` is a child an agent of another machine launched here: it reports to that
/// parent as a child does to one here.
pub(crate) fn remote_child(daemon: &Daemon, agent: &Agent) -> bool {
    agent.parent.is_none() && launch_here(daemon, &agent.id).is_some_and(|l| l.kind == "child")
}

/// Whether `agent` is a session an agent of another machine leads.
pub(crate) fn remote_led(daemon: &Daemon, agent: &Agent) -> bool {
    agent.parent.is_none() && launch_here(daemon, &agent.id).is_some_and(|l| l.kind == "led")
}

/// Tells the agent of another machine that started `agent` here: a delivery of `kind`, kept until
/// that machine takes it. With `back`, the commits of its place go back first, and the delivery
/// names where they came; `ended` says the session ended with it. False when `agent` was started
/// by no agent of another machine.
pub(crate) fn tell(
    daemon: &Arc<Daemon>,
    agent: &str,
    kind: &str,
    body: &str,
    urgency: &str,
    back: bool,
    ended: bool,
) -> bool {
    let Some(launch) = launch_here(daemon, agent) else {
        return false;
    };
    // A session handed to the user is theirs: the agent that started it is told nothing.
    if launch.kind == "handed" {
        return true;
    }
    let id = format!(
        "{kind}-{}",
        flottille_core::agent::new_id().unwrap_or_default()
    );
    let message = MachineMessage::Deliver(Box::new(Deliver {
        id: id.clone(),
        launch: launch.launch.clone(),
        agent: launch.caller.clone(),
        kind: kind.to_owned(),
        body: body.to_owned(),
        urgency: urgency.to_owned(),
        ended,
    }));
    let name = launch.asked["from"]
        .as_str()
        .unwrap_or("another machine")
        .to_owned();
    keep(daemon, &launch, &name, id, message, back);
    true
}

fn keep(
    daemon: &Arc<Daemon>,
    launch: &Launch,
    name: &str,
    id: String,
    message: MachineMessage,
    back: bool,
) {
    let outgoing = Outgoing {
        seq: 0,
        id,
        machine: launch.machine.clone(),
        name: name.to_owned(),
        launch: launch.launch.clone(),
        message: serde_json::to_value(&message).unwrap_or_default(),
        back,
        at: now_ms(),
    };
    if let Err(error) = daemon.write(|store| store.keep_outgoing(&outgoing).map(|()| ((), None))) {
        eprintln!("flottille daemon: a message for {name} cannot be kept: {error}");
        return;
    }
    send_kept(daemon, &launch.machine);
}

/// Sends what is kept for the machine `machine` (its key) while it is linked, in order: at once
/// when a message is kept, and whenever a link to it is made.
pub(crate) fn send_kept(daemon: &Arc<Daemon>, machine: &str) {
    {
        let mut sending = SENDING.lock().unwrap();
        if let Some(again) = sending.get_mut(machine) {
            *again = true;
            return;
        }
        sending.insert(machine.to_owned(), false);
    }
    let (daemon, machine) = (daemon.clone(), machine.to_owned());
    tokio::spawn(async move {
        loop {
            send_each(&daemon, &machine).await;
            let mut sending = SENDING.lock().unwrap();
            match sending.get_mut(&machine) {
                Some(again) if *again => *again = false,
                _ => {
                    sending.remove(&machine);
                    return;
                }
            }
        }
    });
}

async fn send_each(daemon: &Arc<Daemon>, machine: &str) {
    let Ok(remote) = crate::link::remote_of(daemon) else {
        return;
    };
    let Some(key) = key_of(machine) else {
        return;
    };
    loop {
        let kept = daemon
            .read(|store| store.outgoing(Some(machine)))
            .unwrap_or_default();
        if kept.is_empty() {
            return;
        }
        let Some(name) = remote.name_of(&key) else {
            return;
        };
        let Ok((link, _)) = remote.link_to(&name) else {
            return;
        };
        for mut outgoing in kept {
            if outgoing.back {
                let came = crate::travel::send_back(daemon, &outgoing.launch).await;
                if came.is_err() && remote.link_to(&name).is_err() {
                    return;
                }
                outgoing.message = with_commits(outgoing.message, &name, came);
                let written = daemon.write(|store| {
                    store
                        .sent_back_outgoing(&outgoing.id, &outgoing.message)
                        .map(|()| ((), None))
                });
                if written.is_err() {
                    return;
                }
            }
            let message: MachineMessage = match serde_json::from_value(outgoing.message.clone()) {
                Ok(message) => message,
                Err(error) => {
                    eprintln!(
                        "flottille daemon: a message kept for {name} cannot be read, and is \
                         forgotten: {error}"
                    );
                    forget(daemon, &outgoing.id);
                    continue;
                }
            };
            match link.send(&Message::Machine(message)).await {
                Ok(_) => forget(daemon, &outgoing.id),
                // Never taken there, whatever the times it is sent: dropped, and said.
                Err(LinkError::Declined { reason, .. }) => {
                    eprintln!(
                        "flottille daemon: {name} refused a message kept for it, which is \
                         dropped: {reason}"
                    );
                    forget(daemon, &outgoing.id);
                    let _ =
                        daemon.emit("remote.delivery_dropped", dropped(&outgoing, Some(&reason)));
                }
                // The link went: what is left goes when it comes back.
                Err(_) => return,
            }
        }
    }
}

fn forget(daemon: &Daemon, id: &str) {
    let _ = daemon.write(|store| store.forget_outgoing(id).map(|()| ((), None)));
}

/// The delivery `message`, its commits gone back to `name` (`came`): named under its body.
fn with_commits(message: Value, name: &str, came: Result<Value, String>) -> Value {
    let Ok(MachineMessage::Deliver(mut deliver)) =
        serde_json::from_value::<MachineMessage>(message.clone())
    else {
        return message;
    };
    let mut lines = Vec::new();
    match came {
        Ok(came) => {
            for repository in came["repositories"].as_array().into_iter().flatten() {
                let path = repository["path"].as_str().unwrap_or(".");
                match repository["ref"].as_str() {
                    Some(reference) => lines.push(format!(
                        "Its commits came back to you on {reference} of {path}: review and merge \
                         them as a local child's work; nothing moved your branch."
                    )),
                    None => lines.push(format!("It made no new commit in {path}.")),
                }
            }
        }
        Err(why) => lines.push(format!(
            "Its commits could not come back from {name}: {why}."
        )),
    }
    if !lines.is_empty() {
        deliver.body = format!("{}\n{}", deliver.body, lines.join("\n"));
    }
    serde_json::to_value(MachineMessage::Deliver(deliver)).unwrap_or(message)
}

/// What the inbox says of a message dropped: for whom, what, since when, and why.
fn dropped(outgoing: &Outgoing, refused: Option<&str>) -> Value {
    let (kind, agent) = match serde_json::from_value::<MachineMessage>(outgoing.message.clone()) {
        Ok(MachineMessage::Deliver(deliver)) => (deliver.kind, deliver.agent),
        Ok(MachineMessage::Call(call)) => (call.tool, call.agent),
        _ => (String::new(), String::new()),
    };
    json!({"machine": outgoing.name, "launch": outgoing.launch, "kind": kind, "agent": agent,
        "kept_at": outgoing.at, "refused": refused})
}

/// Drops what waited for its machine for 30 days, each with an entry in the inbox.
pub(crate) fn drop_old(daemon: &Daemon) {
    let before = now_ms().saturating_sub(KEPT_FOR.as_millis() as u64);
    let old = daemon
        .write(|store| store.drop_outgoing_before(before).map(|old| (old, None)))
        .unwrap_or_default();
    for outgoing in old {
        eprintln!(
            "flottille daemon: a message kept 30 days for {} is dropped",
            outgoing.name
        );
        let _ = daemon.emit("remote.delivery_dropped", dropped(&outgoing, None));
    }
}

/// The inbox's words of a message dropped (`remote.delivery_dropped`).
pub(crate) fn dropped_text(data: &Value) -> String {
    let machine = data["machine"].as_str().unwrap_or("another machine");
    let what = match data["kind"].as_str().unwrap_or_default() {
        "" => "A message".to_owned(),
        kind => format!("A {kind}"),
    };
    let agent = data["agent"].as_str().unwrap_or_default();
    match data["refused"].as_str() {
        Some(why) => format!(
            "{what} for agent {agent} of {machine} was dropped: {machine} refused it ({why})."
        ),
        None => format!(
            "{what} for agent {agent} of {machine} was dropped: {machine} did not link to this \
             machine for 30 days."
        ),
    }
}

/// A `deliver` the machine of `from` sent: stored for the agent of this machine that launched the
/// session it is of, and that agent woken; refused for any other.
pub(crate) async fn serve_deliver(
    remote: Arc<Remote>,
    from: PublicKey,
    received: Received,
    deliver: Box<Deliver>,
) {
    let answer = match remote.daemon() {
        None => Err("the daemon is stopping".to_owned()),
        Some(daemon) => delivered(&daemon, &remote, &from, *deliver),
    };
    if let Err(why) = &answer {
        eprintln!("flottille daemon: a delivery from another machine was refused: {why}");
    }
    received.answer(answer);
}

fn delivered(
    daemon: &Arc<Daemon>,
    remote: &Remote,
    from: &PublicKey,
    deliver: Deliver,
) -> Result<Value, String> {
    let name = remote
        .name_of(from)
        .ok_or_else(|| "it is no machine of the user's".to_owned())?;
    let machine = hex(from.as_bytes());
    if !an_id(&deliver.id) || !an_id(&deliver.launch) {
        return Err("a delivery and its launch are named by short words".to_owned());
    }
    let launch = daemon
        .read(|store| store.launches("out", Some(&machine), LaunchFilter::Named(&deliver.launch)))
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|launch| launch.caller == deliver.agent && launch.state != "refused")
        .ok_or_else(|| {
            format!(
                "agent {} of this machine did not start launch {} on {name}: a delivery goes only \
                 to the agent that started the session",
                deliver.agent.chars().take(64).collect::<String>(),
                deliver.launch
            )
        })?;
    if !KINDS.contains(&deliver.kind.as_str()) {
        return Err(format!(
            "no delivery of kind {:?} is taken from another machine",
            deliver.kind
        ));
    }
    if ![approval::URGENT, approval::BETWEEN_TURNS].contains(&deliver.urgency.as_str()) {
        return Err(format!("there is no urgency {:?}", deliver.urgency));
    }
    let length = deliver.body.chars().count();
    if length > LONGEST {
        return Err(format!(
            "a delivery is {LONGEST} characters at most, this one {length}"
        ));
    }
    deliveries::deliver(
        daemon,
        &deliver.agent,
        &deliver.kind,
        Some(format!("{}-{}", deliver.launch, deliver.id)),
        &deliver.body,
        &deliver.urgency,
    )
    .ok_or_else(|| "the delivery could not be stored".to_owned())?;
    if deliver.ended {
        daemon
            .write(|store| {
                store
                    .end_launch(&machine, &launch.launch)
                    .map(|()| ((), None))
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(json!({}))
}

/// A `call` the machine of `from` sent: done as the tool would be here, only when the agent that
/// calls started that very session on that machine.
pub(crate) async fn serve_call(
    remote: Arc<Remote>,
    from: PublicKey,
    received: Received,
    call: Box<Call>,
) {
    let answer = match remote.daemon() {
        None => Err("the daemon is stopping".to_owned()),
        Some(daemon) => called(&daemon, &remote, &from, *call),
    };
    if let Err(why) = &answer {
        eprintln!("flottille daemon: a call from another machine was refused: {why}");
    }
    received.answer(answer);
}

fn called(
    daemon: &Arc<Daemon>,
    remote: &Remote,
    from: &PublicKey,
    call: Call,
) -> Result<Value, String> {
    let name = remote
        .name_of(from)
        .ok_or_else(|| "it is no machine of the user's".to_owned())?;
    let machine = hex(from.as_bytes());
    if !an_id(&call.id) || !an_id(&call.launch) {
        return Err("a call and its launch are named by short words".to_owned());
    }
    let caller = call.caller.chars().take(64).collect::<String>();
    let launch = daemon
        .read(|store| store.launches("in", Some(&machine), LaunchFilter::Named(&call.launch)))
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|launch| {
            launch.caller == call.caller
                && launch.agent.as_deref() == Some(call.agent.as_str())
                && launch.state != "refused"
        })
        .ok_or_else(|| {
            format!(
                "agent {caller} of {name} did not start agent {}: only the agent that started a \
                 session, or a child's parent, calls it",
                call.agent.chars().take(64).collect::<String>()
            )
        })?;
    if launch.kind == "handed" {
        return Err(format!(
            "the session was handed to the user: agent {caller} of {name} calls it no more"
        ));
    }
    if !CALLED.contains(&call.tool.as_str()) {
        return Err(format!(
            "{:?} is not a tool another machine calls",
            call.tool
        ));
    }
    let agent = daemon
        .read(|store| store.agent(&call.agent))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("there is no agent {}", call.agent))?;
    let args: Value = serde_json::from_str(&call.arguments)
        .map_err(|_| "the call's arguments are not JSON".to_owned())?;
    let child = launch.kind == "child";
    let text = match call.tool.as_str() {
        "agent_send" | "session_send" => {
            let (field, fits) = match call.tool.as_str() {
                "agent_send" => ("message", child),
                _ => ("text", !child),
            };
            if !fits {
                return Err(format!(
                    "{} is {}: {} is not its tool",
                    named(&agent),
                    if child { "a child" } else { "a session" },
                    call.tool
                ));
            }
            let text = args[field].as_str().unwrap_or_default();
            if text.trim().is_empty() {
                return Err("the message is empty".to_owned());
            }
            if text.chars().count() > LONGEST {
                return Err(format!("a message is {LONGEST} characters at most"));
            }
            if agent.state.is_final() {
                return Err(format!("{} has {}", named(&agent), agent.state.as_str()));
            }
            let id = Some(format!("{}-{}", launch.launch, call.id));
            match child {
                true => {
                    deliveries::deliver(daemon, &agent.id, "parent", id, text, approval::URGENT)
                }
                false => deliveries::deliver(
                    daemon,
                    &agent.id,
                    "session",
                    id,
                    &format!(
                        "agent {caller} of {name}{}\n\n{text}",
                        crate::tree::SESSION_WROTE
                    ),
                    approval::URGENT,
                ),
            }
            .ok_or_else(|| "the message could not be stored".to_owned())?;
            format!("Sent to {} on this machine.", named(&agent))
        }
        "agent_status" | "session_read" => {
            let mut text = format!("- {}", crate::tree::line(daemon, &agent));
            for message in crate::tree::last_messages(daemon, &agent.id) {
                text.push_str(&format!("\n  > {}", crate::tree::cut(&message, 600)));
            }
            text
        }
        "agent_stop" => {
            if agent.state.is_final() {
                format!("{} has {} already.", named(&agent), agent.state.as_str())
            } else {
                let reason = match child {
                    true => format!("stopped by its parent, agent {caller} of {name}"),
                    false => format!("ended by the session that led it, agent {caller} of {name}"),
                };
                crate::tree::stop_tree(daemon, &agent.id, &reason);
                match child {
                    true => format!(
                        "Stopped {} and its own children. Its workspace stays.",
                        named(&agent)
                    ),
                    false => format!(
                        "Ended the session {} with its tree. Its workspace stays.",
                        named(&agent)
                    ),
                }
            }
        }
        _ => unreachable!("the tools are checked above"),
    };
    Ok(json!({"text": text}))
}

// The machine of the agent that started it.

/// The launch of `caller` that started the agent a call of `tool` aims at, on another machine:
/// `None` for an agent of this machine, or any other.
pub(crate) fn aimed(daemon: &Daemon, caller: &str, tool: &str, args: &Value) -> Option<Launch> {
    if !CALLED.contains(&tool) {
        return None;
    }
    let target = args["agent_id"]
        .as_str()
        .or_else(|| args["session"].as_str())?;
    if daemon
        .read(|store| store.agent(target))
        .ok()
        .flatten()
        .is_some()
    {
        return None;
    }
    daemon
        .read(|store| store.launches("out", None, LaunchFilter::Agent(target)))
        .ok()?
        .into_iter()
        .find(|launch| launch.caller == caller && launch.kind != "handed")
}

/// What a call to a session on another machine does, in words, for the gate and the user.
pub(crate) fn says(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    args: &Value,
) -> Option<Vec<String>> {
    let launch = aimed(daemon, &grant.agent, tool, args)?;
    let agent = launch.agent.clone().unwrap_or_default();
    let name = machine_name(daemon, &launch);
    let says = match tool {
        "agent_stop" if launch.kind == "led" => format!(
            "End the session agent {agent} on {name}, which this agent leads: its tree is stopped \
             there; its workspace stays"
        ),
        "agent_stop" => format!("Stop the child agent {agent} on {name}"),
        "session_send" => format!(
            "Write to the session agent {agent} on {name}, which this agent leads: {}",
            args["text"].as_str().unwrap_or_default()
        ),
        _ => return Some(Vec::new()),
    };
    Some(vec![says])
}

fn machine_name(daemon: &Daemon, launch: &Launch) -> String {
    crate::link::remote_of(daemon)
        .ok()
        .zip(key_of(&launch.machine))
        .and_then(|(remote, key)| remote.name_of(&key))
        .unwrap_or_else(|| "another machine".to_owned())
}

/// `tool` of the tree, aimed by `caller` at a session it started on another machine: a message
/// kept until that machine takes it; a read or a stop done there at once, refused while that
/// machine is off. `None` when it aims at no such session.
pub(crate) async fn call(
    daemon: &Arc<Daemon>,
    caller: &Agent,
    tool: &str,
    args: &Value,
) -> Option<Outcome> {
    let launch = aimed(daemon, &caller.id, tool, args)?;
    let agent = launch.agent.clone().unwrap_or_default();
    let name = machine_name(daemon, &launch);
    let refused = |text: String| Some(Outcome::Refused(text));
    let child = launch.kind == "child";
    let message = |id: String| {
        MachineMessage::Call(Box::new(Call {
            id,
            launch: launch.launch.clone(),
            caller: caller.id.clone(),
            agent: agent.clone(),
            tool: tool.to_owned(),
            arguments: args.to_string(),
        }))
    };
    let link = crate::link::remote_of(daemon)
        .map_err(|failure| failure.message)
        .and_then(|remote| remote.link_to(&name).map(|(link, _)| link));
    match tool {
        "agent_send" | "session_send" => {
            let (field, fits) = match tool {
                "agent_send" => ("message", child),
                _ => ("text", !child),
            };
            if !fits {
                return refused(format!(
                    "refused: agent {agent} on {name} is {}: {}",
                    if child {
                        "your child"
                    } else {
                        "a session you lead"
                    },
                    if child {
                        "write to it with agent_send"
                    } else {
                        "write to it with session_send"
                    }
                ));
            }
            if args[field].as_str().unwrap_or_default().trim().is_empty() {
                return refused("refused: the message is empty".to_owned());
            }
            let id = flottille_core::agent::new_id().unwrap_or_default();
            keep(
                daemon,
                &launch,
                &name,
                format!("call-{id}"),
                message(format!("call-{id}")),
                false,
            );
            Some(Outcome::Done(match link {
                Ok(_) => format!(
                    "Sent to agent {agent} on {name}: it reaches it on its next tool result, or as \
                     a new turn."
                ),
                Err(why) => format!(
                    "{why}: your message is kept, and delivered to agent {agent} when {name} links \
                     again. You are woken by what comes back from it."
                ),
            }))
        }
        _ => {
            let link = match link {
                Ok(link) => link,
                Err(why) => {
                    return refused(format!(
                        "refused: {why}: agent {agent} cannot be reached now. You will be woken by \
                         what comes back from it."
                    ));
                }
            };
            let id = format!(
                "call-{}",
                flottille_core::agent::new_id().unwrap_or_default()
            );
            match link.send(&Message::Machine(message(id))).await {
                Ok(answer) => {
                    if tool == "agent_stop" {
                        let _ = daemon.write(|store| {
                            store
                                .end_launch(&launch.machine, &launch.launch)
                                .map(|()| ((), None))
                        });
                    }
                    let text = answer["text"].as_str().unwrap_or_default();
                    Some(Outcome::Done(format!("On {name}: {text}")))
                }
                Err(LinkError::Declined { reason, .. }) => {
                    refused(format!("refused by {name}: {reason}"))
                }
                Err(error) => refused(format!(
                    "refused: {error}: agent {agent} cannot be reached now. You will be woken by \
                     what comes back from it."
                )),
            }
        }
    }
}
