//! Sessions and children an agent starts on another of the user's machines (`docs/remote.md`,
//! "Starting a session on another machine"): `agent_launch` with `machine`.
//!
//! On the caller's machine, the launch is checked against what that machine offers, sent as the
//! `launch` machine message, then the caller's place travels by itself (`travel.rs`). On the
//! machine it runs on, its gate judges it every time, child or led, before any code is taken:
//! the code of a launch it did not record is refused, and a workspace made of it runs its setup
//! only once the launch is allowed (at once in Full or by the judge, or by the user's answer).
//! The agent gets no network and no tool to move code: the transport is the daemons'.

use std::sync::Arc;
use std::time::Duration;

use flottille_core::agent::{Agent, GateMode};
use flottille_core::approval::{self, Judgement, Kind, Request, RequestState};
pub(crate) use flottille_core::store::Launch;
use flottille_core::store::{LaunchFilter, now_ms};
use flottille_link::{LaunchAsk, MachineMessage, Message, PublicKey, Received, Role};
use serde_json::{Value, json};

use crate::agents::{self, New};
use crate::daemon::Daemon;
use crate::link::Remote;
use crate::mcp::Outcome;
use crate::place::Place;
use crate::projects;
use crate::tree::{LaunchKind, Slot};

/// The most launches one machine may leave here waiting for their code: past them, its next is
/// refused, so that a machine that sends launches and never their code fills no disk.
const MOST_WAITING: usize = 4;

/// How long a launch may wait for its code before it is forgotten, with what came of it.
const WAITING_FOR_CODE: Duration = Duration::from_secs(60 * 60);

/// What an agent asked `agent_launch` to start on another machine.
pub struct Asked {
    pub kind: LaunchKind,
    pub title: String,
    pub brief: String,
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub role: String,
}

type Done = Result<Outcome, Outcome>;

fn refused(text: impl Into<String>) -> Outcome {
    Outcome::Refused(text.into())
}

fn kind_word(kind: LaunchKind) -> &'static str {
    match kind {
        LaunchKind::Child => "child",
        LaunchKind::Led => "led",
        LaunchKind::Handed => "handed",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Starts what `caller` asked on the machine named `machine`, in a copy of its place there:
/// refused at once when that machine cannot take it, answered at once with the agent it started
/// there, or the request that waits for the user there.
pub async fn launch(daemon: &Arc<Daemon>, caller: &Agent, machine: &str, asked: Asked) -> Done {
    let remote = crate::link::remote_of(daemon)
        .map_err(|_| refused("refused: this machine is linked to no other: remote is off"))?;
    let (members, this) = {
        let remote = remote.clone();
        tokio::task::spawn_blocking(move || remote.members())
            .await
            .unwrap_or_else(|_| Err("the member list could not be read".to_owned()))
            .map_err(|why| refused(format!("refused: {why}")))?
    };
    if machine == this {
        return Err(refused(format!(
            "refused: {machine} is this machine: leave machine out to start it here"
        )));
    }
    flottille_link::runs_agents(&members, machine, "agent.start")
        .map_err(|why| refused(why.to_string()))?;
    let (link, key) = remote
        .link_to(machine)
        .map_err(|why| refused(format!("refused: {why}")))?;
    let project =
        projects::load_project(daemon, &caller.project).map_err(|f| refused(f.message))?;
    let child = asked.kind == LaunchKind::Child;
    let offers = remote.offers();
    let takes = |name: &str| {
        offers
            .iter()
            .find(|(offering, _)| offering == name)
            .map(|(_, offers)| {
                let why = offers.refusal(name, &project.name, Some(&asked.provider));
                let full = child
                    && offers
                        .projects
                        .iter()
                        .any(|offered| offered.name == project.name && offered.slots == 0);
                match (why, full) {
                    (Some(why), _) => Some(why),
                    (None, true) => Some(format!(
                        "{name} has no slot free for project {}",
                        project.name
                    )),
                    (None, false) => None,
                }
            })
    };
    let refusal = match takes(machine) {
        None => Some(format!(
            "{machine} has not said yet what it offers: try again in a moment"
        )),
        Some(why) => why,
    };
    if let Some(mut why) = refusal {
        if why.ends_with(&format!("is not on {machine}")) {
            why.push_str(&format!(
                ": register it there with flottille --machine {machine} project add <folder>"
            ));
        }
        let others: Vec<&str> = offers
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| *name != machine && takes(name) == Some(None))
            .collect();
        let others = match others.as_slice() {
            [] => String::new(),
            [one] => format!("; {one} would take it"),
            [rest @ .., last] => format!("; {} and {last} would take it", rest.join(", ")),
        };
        return Err(refused(format!("refused: {why}{others}")));
    }
    // A child counts in its parent's project cap here too, refused above it, never queued.
    let _slot = match child {
        true => Some(Slot::take(daemon, &project.name, project.max_children)?),
        false => None,
    };
    let place = place_of(daemon, caller, &project)?;
    let branch = branch_of(&place[0].1).await.map_err(refused)?;
    let tree = crate::tree::root_of(daemon, &caller.id);
    let (mode, switch) = match crate::judge::mode(daemon, &caller.id) {
        GateMode::Ask => ("ask", None),
        GateMode::Auto => ("auto", None),
        GateMode::Full => match daemon.read(|store| store.full_switch(&tree)).ok().flatten() {
            Some(switch) => ("full", Some(switch)),
            // Only the user's signed switch raises a launch on another machine to Full.
            None => ("auto", None),
        },
    };
    let launch = flottille_core::agent::new_id().map_err(|error| refused(error.to_string()))?;
    let message = LaunchAsk {
        launch: launch.clone(),
        caller: caller.id.clone(),
        tree,
        kind: kind_word(asked.kind).to_owned(),
        project: project.name.clone(),
        title: asked.title.clone(),
        brief: asked.brief.clone(),
        provider: asked.provider.clone(),
        model: asked.model.clone(),
        effort: asked.effort.clone(),
        role: Some(asked.role.clone()),
        mode: mode.to_owned(),
        switch,
        branch: branch.clone(),
        repositories: place.iter().map(|(path, _)| path.clone()).collect(),
    };
    let judged = link
        .send(&Message::Machine(MachineMessage::Launch(Box::new(
            message.clone(),
        ))))
        .await
        .map_err(|error| refused(format!("refused by {machine}: {error}")))?;
    let mut record = Launch {
        way: "out".into(),
        machine: hex(key.as_bytes()),
        launch: launch.clone(),
        caller: caller.id.clone(),
        kind: message.kind.clone(),
        project: project.name.clone(),
        state: judged["state"].as_str().unwrap_or("held").to_owned(),
        request: None,
        agent: None,
        workspace: None,
        asked: serde_json::to_value(&message).unwrap_or_default(),
        at: now_ms(),
    };
    // Kept before the code goes, so that a child counts in the cap here from now on.
    write(daemon, &record).map_err(refused)?;
    drop(_slot);
    let sent = crate::travel::send(
        daemon,
        machine,
        &launch,
        &project.name,
        &place,
        crate::travel::Testing::default(),
    )
    .await;
    let placed = match sent {
        Ok(placed) => placed,
        Err(why) => {
            let _ = daemon.write(|store| {
                store
                    .forget_launch("out", &record.machine, &launch)
                    .map(|()| ((), None))
            });
            return Err(refused(format!(
                "refused: the code of your place could not be sent to {machine}: {why}"
            )));
        }
    };
    record.agent = placed["session"].as_str().map(str::to_owned);
    record.request = placed["request"].as_u64();
    record.workspace = placed["workspace"].as_str().map(str::to_owned);
    record.state = match record.request {
        Some(_) => "held".into(),
        None => "started".into(),
    };
    write(daemon, &record).map_err(refused)?;
    Ok(Outcome::Done(answer(&asked, machine, &launch, &placed)))
}

/// What the caller is told of its launch on `machine`.
fn answer(asked: &Asked, machine: &str, launch: &str, placed: &Value) -> String {
    let id = placed["session"].as_str().unwrap_or_default();
    let named = format!("\"{}\" (agent {id})", asked.title);
    let there = placed["branch"].as_str().unwrap_or_default();
    let workspace = placed["workspace"].as_str().unwrap_or_default();
    let place = format!(
        "in workspace {workspace} there, a copy of your place: your branch and your uncommitted \
         changes, on {there}"
    );
    let back = format!(
        "Its commits come back to you on refs/flottille/machines/{machine}/{launch}/{there} of \
         each repository, which you review and merge as you would a local child's work; nothing \
         moves your branch."
    );
    if let Some(number) = placed["request"].as_u64() {
        return format!(
            "Not started yet: request #{number} on {machine} is waiting for the user, who allows \
             or refuses it there. {named} will run on {machine} {place}, once allowed. {back} You \
             are told nothing more here: go on with your own work."
        );
    }
    match asked.kind {
        LaunchKind::Child => format!(
            "Launched {named} on {machine}, {place}. {back} It works on its own there; \
             agent_status and agent_send do not reach a child on another machine."
        ),
        LaunchKind::Led => format!(
            "Started a session led by you, {named}, on {machine}, {place}. It is a real session \
             of {machine}, not your child: the user sees it there and may talk to it. {back}"
        ),
        LaunchKind::Handed => format!(
            "Started a session for the user, {named}, on {machine}, {place}, handed over. It is \
             the user's. {back}"
        ),
    }
}

fn write(daemon: &Daemon, launch: &Launch) -> Result<(), String> {
    daemon
        .write(|store| store.record_launch(launch).map(|()| ((), None)))
        .map_err(|error| error.to_string())
}

/// The repositories of the caller's place: their paths in the project and their folders here.
fn place_of(
    daemon: &Daemon,
    caller: &Agent,
    project: &flottille_core::project::Project,
) -> Result<Vec<(String, std::path::PathBuf)>, Outcome> {
    let place: Vec<_> = match &caller.workspace {
        Some(name) => {
            let workspace = daemon
                .read(|store| store.workspace(&project.name, name))
                .ok()
                .flatten()
                .ok_or_else(|| refused(format!("refused: your workspace {name} is gone")))?;
            workspace
                .repositories
                .iter()
                .map(|r| (r.path.clone(), workspace.folder_of(&r.path)))
                .collect()
        }
        None => project
            .repositories
            .iter()
            .map(|r| {
                let folder = match r.path.as_str() {
                    "." => project.folder.clone(),
                    path => project.folder.join(path),
                };
                (r.path.clone(), folder)
            })
            .collect(),
    };
    if place.is_empty() {
        return Err(refused("refused: your place has no repository to send"));
    }
    Ok(place)
}

/// The branch `folder` is on.
async fn branch_of(folder: &std::path::Path) -> Result<String, String> {
    let on = crate::git::output(
        folder,
        &["symbolic-ref", "--quiet", "HEAD"],
        crate::git::Class::Read,
    )
    .await
    .map_err(|_| format!("refused: {} is not on a branch", folder.display()))?;
    on.trim()
        .strip_prefix("refs/heads/")
        .map(str::to_owned)
        .ok_or_else(|| format!("refused: {} is not on a branch", folder.display()))
}

// The machine it runs on.

/// A `launch` the machine of `from` sent: checked, judged by this machine's gate, and recorded,
/// so that its code is taken; refused otherwise, with why.
pub(crate) async fn serve(
    remote: Arc<Remote>,
    from: PublicKey,
    received: Received,
    asked: Box<LaunchAsk>,
) {
    let answer = match remote.daemon() {
        None => Err("the daemon is stopping".to_owned()),
        Some(daemon) => judged(&daemon, &remote, &from, *asked).await,
    };
    if let Err(why) = &answer {
        eprintln!("flottille daemon: a launch was refused: {why}");
    }
    received.answer(answer);
}

/// Who the launch's sender is, and this machine's name, by the checked member list.
struct Names {
    from: String,
    here: String,
    members: flottille_link::Members,
}

fn names(remote: &Remote, from: &PublicKey) -> Result<Names, String> {
    let (members, here) = remote.members()?;
    let member = members
        .by_machine(from)
        .ok_or_else(|| "it is no member's machine".to_owned())?;
    if member.role != Role::Runner {
        return Err(format!(
            "{} is a {}: it runs no agent, so no agent of it launches",
            member.name,
            member.role.word()
        ));
    }
    flottille_link::runs_agents(&members, &here, "agent.start").map_err(|why| why.to_string())?;
    Ok(Names {
        from: member.name.clone(),
        here,
        members,
    })
}

async fn judged(
    daemon: &Arc<Daemon>,
    remote: &Arc<Remote>,
    from: &PublicKey,
    asked: LaunchAsk,
) -> Result<Value, String> {
    let names = {
        let (remote, from) = (remote.clone(), *from);
        tokio::task::spawn_blocking(move || names(&remote, &from))
            .await
            .map_err(|_| "the member list could not be read".to_owned())??
    };
    let Names {
        from: sender,
        here,
        members,
    } = &names;
    // Its name goes in ref names: one component git takes, always.
    if asked.launch.is_empty()
        || asked.launch.len() > 64
        || !asked
            .launch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("a launch is named by 1 to 64 letters, digits, dashes or underscores".into());
    }
    if asked.caller.trim().is_empty() || asked.tree.trim().is_empty() {
        return Err(format!(
            "the launch names no agent of {sender}'s: only an agent launches"
        ));
    }
    let kind = match asked.kind.as_str() {
        "child" => LaunchKind::Child,
        "led" => LaunchKind::Led,
        "handed" => LaunchKind::Handed,
        other => return Err(format!("there is no kind of launch {other}")),
    };
    if asked.title.trim().is_empty() || asked.brief.trim().is_empty() {
        return Err("a launch needs a title and a brief".to_owned());
    }
    if let Some(crate::gate::Verdict::Deny(why)) =
        crate::gate::too_long("brief", &json!(asked.brief))
    {
        return Err(why);
    }
    let machine = hex(from.as_bytes());
    sweep(daemon, &machine).await;
    let waiting = daemon
        .read(|store| store.launches("in", Some(&machine), LaunchFilter::All))
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|launch| launch.agent.is_none() && launch.state != "refused")
        .count();
    if waiting >= MOST_WAITING {
        return Err(format!(
            "{sender} has {MOST_WAITING} launches waiting for their code already: send their \
             code first"
        ));
    }
    let project = projects::load_project(daemon, &asked.project).map_err(|_| {
        format!(
            "the project {} is not on {here}: register it there with flottille --machine {here} \
             project add <folder>",
            asked.project
        )
    })?;
    if let Some(missing) = asked
        .repositories
        .iter()
        .find(|path| !project.repositories.iter().any(|r| &r.path == *path))
    {
        return Err(format!(
            "the project {} on {here} has no repository {missing}: add it there with the same \
             path",
            project.name
        ));
    }
    let offered = {
        let daemon = daemon.clone();
        tokio::task::spawn_blocking(move || crate::link::offered(&daemon))
            .await
            .unwrap_or_default()
    };
    if !offered.agents.contains(&asked.provider) {
        return Err(format!("{} is not signed in on {here}", asked.provider));
    }
    crate::providers::choose(
        daemon,
        &project,
        crate::providers::Asked {
            provider: Some(asked.provider.clone()),
            model: asked.model.clone(),
            effort: asked.effort.clone(),
        },
    )
    .map_err(|failure| failure.message)?;
    if let Some(role) = &asked.role
        && crate::roles::role(role).is_none()
    {
        return Err(format!("there is no role {role}"));
    }
    // A child counts in the project's cap here: refused above it, never queued.
    let _slot = match kind {
        LaunchKind::Child => Some(
            Slot::take(daemon, &project.name, project.max_children)
                .map_err(|_| format!("{here} has no slot free for project {}", project.name))?,
        ),
        _ => None,
    };
    let mode = match asked.mode.as_str() {
        "ask" => GateMode::Ask,
        "full" => match &asked.switch {
            Some(switch)
                if flottille_link::full_switch(switch, sender, &asked.tree, members).is_ok() =>
            {
                GateMode::Full
            }
            // A machine alone cannot raise a mode.
            _ => GateMode::Auto,
        },
        _ => GateMode::Auto,
    };
    let says = words(kind, &asked, sender);
    let judgement = match mode {
        GateMode::Full => None,
        GateMode::Ask => None,
        GateMode::Auto => match crate::judge::chosen(daemon).await {
            Some(judge) => {
                let place = Place::main(&project, project.folder.clone());
                let args = json!({"kind": asked.kind, "machine": sender, "title": asked.title,
                    "brief": asked.brief});
                Some(crate::judge::judge_launch(daemon, &judge, &place, &args, &says).await)
            }
            None => None,
        },
    };
    let accepted = mode == GateMode::Full
        || judgement
            .as_ref()
            .is_some_and(|judged| judged.verdict == "allow");
    let state = if accepted { "accepted" } else { "held" };
    let record = Launch {
        way: "in".into(),
        machine,
        launch: asked.launch.clone(),
        caller: asked.caller.clone(),
        kind: asked.kind.clone(),
        project: project.name.clone(),
        state: state.into(),
        request: None,
        agent: None,
        workspace: None,
        asked: json!({
            "asked": asked,
            "from": sender,
            "mode": mode.as_str(),
            "says": says,
            "judgement": judgement,
        }),
        at: now_ms(),
    };
    let known = daemon
        .read(|store| {
            store.launches(
                "in",
                Some(&record.machine),
                LaunchFilter::Named(&record.launch),
            )
        })
        .map_err(|error| error.to_string())?;
    if !known.is_empty() {
        return Err(format!("launch {} was asked already", record.launch));
    }
    write(daemon, &record)?;
    eprintln!(
        "flottille daemon: launch {} of agent {} of {sender} {state} ({})",
        record.launch,
        asked.caller,
        mode.as_str()
    );
    Ok(json!({"state": state}))
}

/// What the user and the judge read of a launch: what it starts, for whom, where, and its first
/// message whole.
fn words(kind: LaunchKind, asked: &LaunchAsk, from: &str) -> Vec<String> {
    let (caller, branch, title) = (&asked.caller, &asked.branch, &asked.title);
    let place = format!("in a new workspace on {branch} with its uncommitted changes");
    let first = match kind {
        LaunchKind::Child => format!(
            "Start a child of agent {caller} of {from} on this machine, {place}, a copy of its \
             place: \"{title}\""
        ),
        LaunchKind::Led => format!(
            "Start a session led by agent {caller} of {from} on this machine, {place}: \"{title}\""
        ),
        LaunchKind::Handed => format!(
            "Start a session for the user, handed over by agent {caller} of {from}, on this \
             machine, {place}: \"{title}\""
        ),
    };
    vec![first, format!("Its first message: {}", asked.brief)]
}

/// Forgets the launches of the machine `machine` that waited for their code too long, with what
/// came of it.
async fn sweep(daemon: &Daemon, machine: &str) {
    let launches = daemon
        .read(|store| store.launches("in", Some(machine), LaunchFilter::All))
        .unwrap_or_default();
    let old = now_ms().saturating_sub(WAITING_FOR_CODE.as_millis() as u64);
    for launch in launches
        .into_iter()
        .filter(|launch| launch.agent.is_none() && launch.at < old)
    {
        let travels = daemon
            .read(|store| store.travels("in", Some(machine), &launch.launch))
            .unwrap_or_default();
        for travel in travels {
            let folder = std::path::Path::new(&travel.folder);
            let reference = crate::git::bundle::incoming_ref(machine, &launch.launch);
            if let Err(error) = crate::git::bundle::remove_ref(folder, &reference).await {
                eprintln!(
                    "flottille daemon: what came for launch {} in {} is kept: {error}",
                    launch.launch,
                    folder.display()
                );
            }
        }
        let _ = daemon.write(|store| {
            store
                .forget_launch("in", machine, &launch.launch)
                .map(|()| ((), None))
        });
    }
}

/// The launch `launch` the machine `machine` sent, while it waits for its code: its gate did not
/// refuse it, and nothing is made of it yet.
pub(crate) fn open(daemon: &Daemon, machine: &str, launch: &str) -> Option<Launch> {
    daemon
        .read(|store| store.launches("in", Some(machine), LaunchFilter::Named(launch)))
        .ok()?
        .into_iter()
        .find(|launch| {
            launch.agent.is_none() && matches!(launch.state.as_str(), "held" | "accepted")
        })
}

/// Keeps the name of the workspace made of `launch`'s code, before it is made: its setup is held
/// while the launch is (`holds`).
pub(crate) fn set_workspace(
    daemon: &Daemon,
    launch: &Launch,
    workspace: &str,
) -> Result<(), String> {
    let mut launch = launch.clone();
    launch.workspace = Some(workspace.to_owned());
    write(daemon, &launch)
}

/// Whether `workspace` is made of the code of a launch held for the user: its setup waits.
pub(crate) fn holds(daemon: &Daemon, workspace: &flottille_core::workspace::Workspace) -> bool {
    daemon
        .read(|store| store.launches("in", None, LaunchFilter::All))
        .unwrap_or_default()
        .iter()
        .any(|launch| {
            launch.state == "held"
                && launch.project == workspace.project
                && launch.workspace.as_deref() == Some(workspace.name.as_str())
        })
}

/// The agent of `launch`, in the workspace `placed` names: started at once when the gate accepted
/// it, or made to wait, with the user's request, when it is held. What `placed` says, with the
/// agent's id and the request's number.
pub(crate) async fn placed(
    daemon: &Arc<Daemon>,
    mut launch: Launch,
    mut placed: Value,
) -> Result<Value, String> {
    let asked: LaunchAsk = serde_json::from_value(launch.asked["asked"].clone())
        .map_err(|error| format!("the launch cannot be read: {error}"))?;
    let workspace = placed["workspace"].as_str().unwrap_or_default().to_owned();
    let project = projects::load_project(daemon, &launch.project).map_err(|f| f.message)?;
    let child = launch.kind == "child";
    let role = asked
        .role
        .as_deref()
        .unwrap_or(if child { "worker" } else { "lead" });
    let role = crate::roles::role(role).ok_or_else(|| format!("there is no role {role}"))?;
    let mode =
        GateMode::parse(launch.asked["mode"].as_str().unwrap_or_default()).unwrap_or_default();
    let agent = agents::create(
        daemon,
        &project,
        New {
            id: None,
            workspace: Some(workspace.clone()),
            preparing: true,
            role,
            parent: None,
            title: Some(asked.title.trim().to_owned()),
            brief: asked.brief.clone(),
            provider: Some(asked.provider.clone()),
            model: asked.model.clone(),
            effort: asked.effort.clone(),
            gate_mode: Some(mode),
            launched_by: None,
            led: false,
            launched: None,
        },
    )
    .map_err(|failure| failure.message)?;
    launch.agent = Some(agent.id.clone());
    launch.workspace = Some(workspace.clone());
    if launch.state == "held" {
        let says: Vec<String> =
            serde_json::from_value(launch.asked["says"].clone()).unwrap_or_default();
        let judgement: Option<Judgement> =
            serde_json::from_value(launch.asked["judgement"].clone()).unwrap_or_default();
        let shown = json!({"kind": asked.kind, "machine": launch.asked["from"],
            "title": asked.title, "brief": asked.brief});
        let request = Request {
            number: 0,
            agent: agent.id.clone(),
            call_key: Some(format!("launch-{}", launch.launch)),
            tool: "agent_launch".into(),
            arguments: shown.clone(),
            shown,
            reasons: vec![format!(
                "it starts an agent {} asked for on this machine",
                launch.asked["from"].as_str().unwrap_or_default()
            )],
            base: false,
            kind: Kind::Action,
            says,
            judgement,
            held_by: approval::USER.to_owned(),
            forwards: Vec::new(),
            state: RequestState::Pending,
            decision: None,
            created_at: 0,
            decided_at: None,
            place: Some(format!("workspace {workspace}")),
            digest: None,
        };
        let (request, _) = daemon
            .write(|store| {
                let (request, event) = store.ask(&request)?;
                Ok(((request, ()), event))
            })
            .map_err(|error| error.to_string())?;
        launch.request = Some(request.number);
        placed["request"] = request.number.into();
    } else {
        launch.state = "started".into();
        tokio::spawn(crate::tree::brief_once_ready(daemon.clone(), agent.clone()));
    }
    write(daemon, &launch)?;
    placed["session"] = agent.id.clone().into();
    Ok(placed)
}

/// The launch held by the user's request `number`, when it is one.
pub(crate) fn held_by_request(daemon: &Daemon, number: u64) -> Option<Launch> {
    daemon
        .read(|store| store.launches("in", None, LaunchFilter::Request(number)))
        .ok()?
        .into_iter()
        .find(|launch| launch.state == "held")
}

/// The user answered the request of a launch held for them: allowed, its workspace's setup goes
/// on and its agent is sent its first message once it is ready; refused, its agent ends.
pub(crate) fn decided(daemon: &Arc<Daemon>, mut launch: Launch, request: &Request, allowed: bool) {
    let Some(agent) = launch.agent.clone() else {
        return;
    };
    launch.state = match allowed {
        true => "started".into(),
        false => "refused".into(),
    };
    if let Err(why) = write(daemon, &launch) {
        eprintln!(
            "flottille daemon: launch {}: its answer cannot be kept: {why}",
            launch.launch
        );
        return;
    }
    let outcome = match allowed {
        true => "done",
        false => "refused",
    };
    if let Err(error) = daemon.write(|store| Ok(((), store.settle_unsent(request, outcome)?))) {
        eprintln!(
            "flottille daemon: request #{}: its answer cannot be stored: {error}",
            request.number
        );
    }
    if !allowed {
        crate::tree::stop_tree(daemon, &agent, "the user refused its launch");
        return;
    }
    let workspace = launch.workspace.clone().unwrap_or_default();
    if let Ok(Some(made)) = daemon.read(|store| store.workspace(&launch.project, &workspace))
        && made.state() == flottille_core::workspace::State::Preparing
    {
        crate::workspaces::go_on_preparing(daemon, made.id);
    }
    if let Ok(Some(agent)) = daemon.read(|store| store.agent(&agent)) {
        tokio::spawn(crate::tree::brief_once_ready(daemon.clone(), agent));
    }
}

/// For an agent started here for an agent of another machine: that agent, its machine's name,
/// and whether it is a child.
pub(crate) fn launched_from(daemon: &Daemon, agent: &str) -> Option<(String, String, bool)> {
    let launch = daemon
        .read(|store| store.launches("in", None, LaunchFilter::Agent(agent)))
        .ok()?
        .into_iter()
        .next()?;
    let from = launch.asked["from"]
        .as_str()
        .unwrap_or("another machine")
        .to_owned();
    Some((launch.caller, from, launch.kind == "child"))
}

/// Whether `agent` was started here for another machine's agent, and allowed: its first message
/// is due.
pub(crate) fn started_here(daemon: &Daemon, agent: &str) -> bool {
    daemon
        .read(|store| store.launches("in", None, LaunchFilter::Agent(agent)))
        .unwrap_or_default()
        .iter()
        .any(|launch| launch.state == "started")
}
