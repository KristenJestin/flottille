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
use flottille_core::store::{LaunchFilter, LaunchRefused, now_ms};
use flottille_link::{LaunchAsk, MachineMessage, Message, PublicKey, Received, Role};
use serde_json::{Value, json};

use crate::agents::{self, DEPTH_LIMIT, New};
use crate::daemon::Daemon;
use crate::link::Remote;
use crate::mcp::Outcome;
use crate::place::Place;
use crate::projects;
use crate::tree::{LaunchKind, Slot};

/// The most launches one machine may leave here waiting for their code: past them, its next is
/// refused, so that a machine that sends launches and never their code fills no disk.
const MOST_WAITING: usize = 4;

/// The most launches of one machine, of any kind, that may wait here for the user's answer: past
/// them, its next is refused, so that a machine fills no list of requests.
const MOST_HELD: usize = 4;

/// How long a launch may wait for its code before it is forgotten, with what came of it.
const WAITING_FOR_CODE: Duration = Duration::from_secs(60 * 60);

/// The longest title a launch of another machine may have, in characters: it is shown on a card.
const LONGEST_TITLE: usize = 200;

/// The longest branch name a launch of another machine may give, in bytes.
const LONGEST_BRANCH: usize = 255;

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

/// Whether `id` is an agent's id as a machine names one: 1 to 64 letters, digits, dashes or
/// underscores.
fn agent_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The mode of `caller`'s tree as another machine is told it, with a launch or a call: the agent
/// of the tree a switch names (its root otherwise), the mode's word, and the user's signed switch
/// to Full, the only way another machine takes Full.
pub(crate) fn mode_sent(daemon: &Daemon, caller: &Agent) -> (String, &'static str, Option<String>) {
    let mut tree = crate::tree::root_of(daemon, &caller.id);
    let (mode, switch) = match crate::judge::mode(daemon, &caller.id) {
        GateMode::Ask => ("ask", None),
        GateMode::Auto => ("auto", None),
        GateMode::Full => match daemon.read(|store| store.full_switch(&tree)).ok().flatten() {
            Some(switch) => {
                // Kept on the root, it may name any agent of the tree: the tree is named by
                // that agent, which the other machine checks the switch against.
                if let Some(named) = switched(&switch) {
                    tree = named;
                }
                ("full", Some(switch))
            }
            // Only the user's signed switch raises a launch on another machine to Full.
            None => ("auto", None),
        },
    };
    (tree, mode, switch)
}

/// The mode another machine's launch or call is taken in: the one it says, except Full, taken
/// only with the user's switch signed by a device for `tree`, less than 12 hours old here; Auto
/// otherwise. A machine alone cannot raise a mode.
pub(crate) fn mode_taken(
    mode: &str,
    switch: Option<&str>,
    sender: &str,
    tree: &str,
    members: &flottille_link::Members,
) -> GateMode {
    match mode {
        "ask" => GateMode::Ask,
        "full" => match switch {
            Some(switch)
                if flottille_link::full_switch(
                    switch,
                    sender,
                    tree,
                    members,
                    crate::link::wall_now(),
                )
                .is_ok() =>
            {
                GateMode::Full
            }
            _ => GateMode::Auto,
        },
        _ => GateMode::Auto,
    }
}

/// The agent a switch to Full the user signed names: one of the tree it switched.
fn switched(switch: &str) -> Option<String> {
    let request: flottille_link::Request = serde_json::from_str(switch).ok()?;
    let params: Value = serde_json::from_str(&request.signed.params).ok()?;
    params["agent"].as_str().map(str::to_owned)
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
        .map_err(|why| refused(actioned(&why.to_string(), machine, &asked.provider)))?;
    let (link, key) = remote.link_to(machine).map_err(|why| {
        refused(format!(
            "refused: {}",
            actioned(&why, machine, &asked.provider)
        ))
    })?;
    let project =
        projects::load_project(daemon, &caller.project).map_err(|f| refused(f.message))?;
    let child = asked.kind == LaunchKind::Child;
    let offers = remote.offers();
    let takes = |name: &str| {
        offers
            .iter()
            .find(|(offering, _)| offering == name)
            .map(|(_, offers)| {
                // The project goes with the launch: only the agent and the slots bound it.
                let why = offers.agent_refusal(name, Some(&asked.provider));
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
    if let Some(why) = refusal {
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
        let why = actioned(&why, machine, &asked.provider);
        return Err(refused(format!("refused: {why}{others}")));
    }
    // A child counts in its parent's project cap here too, refused above it, never queued.
    let _slot = match child {
        true => Some(Slot::take(daemon, &project.name, project.max_children)?),
        false => None,
    };
    let place = place_of(daemon, caller, &project)?;
    let branch = branch_of(&place[0].1).await.map_err(refused)?;
    let identities = crate::travelled::identities(&project, &place).await;
    let settings = projects::travelling(daemon, &project.name).map_err(|f| refused(f.message))?;
    if serde_json::to_string(&settings)
        .is_ok_and(|text| text.len() > crate::travelled::LONGEST_SETTINGS)
    {
        return Err(refused(format!(
            "refused: the settings of {} are more than {} KiB: they cannot travel",
            project.name,
            crate::travelled::LONGEST_SETTINGS / 1024
        )));
    }
    let (tree, mode, switch) = mode_sent(daemon, caller);
    let launch = flottille_core::agent::new_id().map_err(|error| refused(error.to_string()))?;
    let repositories: Vec<String> = place.iter().map(|(path, _)| path.clone()).collect();
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
        repositories: repositories.clone(),
        identities,
        settings: Some(settings),
        launched: launched(daemon, caller),
        depth: caller.depth,
        copied: crate::travelled::copied(&project, &repositories),
    };
    let judged = link
        .send(&Message::Machine(MachineMessage::Launch(Box::new(
            message.clone(),
        ))))
        .await
        .map_err(|error| {
            refused(format!(
                "refused by {machine}: {}",
                actioned(&error.to_string(), machine, &asked.provider)
            ))
        })?;
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
        workspace_id: None,
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
    while crate::across::playing_slow_call() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
    record.asked["there"] = json!({"project": placed["project"], "made": placed["made"],
        "kept": placed["kept"]});
    record.state = match record.request {
        Some(_) => "held".into(),
        None => "started".into(),
    };
    write(daemon, &record).map_err(refused)?;
    // Its caller may have ended while its code travelled: what it started goes with it.
    let gone = crate::across::recorded(daemon, &record);
    // Its pilot follows it from here, as a session it leads here: held, from now on too, since its
    // first wake may come before anything else is heard of it.
    if let (LaunchKind::Led, Some(session), false) = (asked.kind, &record.agent, gone) {
        crate::subscriptions::lead_elsewhere(daemon, &caller.id, session, &asked.title, machine)
            .map_err(refused)?;
    }
    Ok(Outcome::Done(answer_to_caller(
        kind_word(asked.kind),
        &asked.title,
        machine,
        &launch,
        &placed,
    )))
}

/// `why` a launch on `machine` is refused, with the window's action that would let it through,
/// when there is one: never a command line, which the agent cannot run and the user would have
/// to look up (`docs/remote.md`, "Starting a session on another machine").
fn actioned(why: &str, machine: &str, provider: &str) -> String {
    let action = if why.contains("is not signed in on") {
        let title = match provider {
            "claude" => "Claude",
            "codex" => "Codex",
            "opencode" => "OpenCode",
            other => other,
        };
        format!("Sign in to {title} on {machine} in Settings › Agents")
    } else if why.contains("no agent is signed in on") {
        format!("Sign in to an agent on {machine} in Settings › Agents")
    } else if why.contains("is offline since") {
        format!("Settings › Machines says when {machine} is back")
    } else if why.contains("it runs no agent") {
        format!("Settings › Machines says the role of each machine: {machine}'s is not a runner")
    } else if why.contains("has no slot free") {
        format!("{machine} takes one once one of its children there ends")
    } else {
        return why.to_owned();
    };
    let why = why.trim_end_matches('.');
    format!("{why}. {action}")
}

/// What the caller is told of its launch on `machine`, of `kind` titled `title`: held, and once
/// started, the result of the user's answer when it was held.
fn answer_to_caller(
    kind: &str,
    title: &str,
    machine: &str,
    launch: &str,
    placed: &Value,
) -> String {
    let id = placed["session"].as_str().unwrap_or_default();
    let named = format!("\"{title}\" (agent {id})");
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
    let kept = match placed["kept"].as_str() {
        Some(whose) => {
            let project = placed["project"].as_str().unwrap_or_default();
            let whose = match whose {
                "user" => format!("which the user registered on {machine}: it keeps its own"),
                _ => format!(
                    "which Flottille made on {machine} for another machine's launches: it keeps \
                     that machine's"
                ),
            };
            format!(" It runs in the project {project} there, {whose} settings, not yours.")
        }
        None => String::new(),
    };
    let back = format!("{back}{kept}");
    // Said as a call held here, then where it will run: the user's answer comes as a held call's.
    if let Some(number) = placed["request"].as_u64() {
        let waiting = crate::approvals::waiting(number).replacen(
            &format!("#{number}."),
            &format!("#{number} on {machine}."),
            1,
        );
        let follows = match kind {
            "led" => {
                "it runs as a session led by you: each end of its turns wakes you as a local led \
                 session's does, and session_send reaches it"
            }
            "child" => {
                "it runs as your child: its report, questions and failure come to you as a local \
                 child's do, and agent_send reaches it"
            }
            _ => "it runs as the user's session, handed over",
        };
        return format!(
            "{waiting}\nIt will run as {named} on {machine} {place}, once allowed. {back} Once \
             the user allows it there, {follows}."
        );
    }
    match kind {
        "child" => format!(
            "Launched {named} on {machine}, {place}. {back} Its report, questions and failure \
             come to you as a local child's; agent_send, agent_status and agent_stop reach it \
             while {machine} is on, and a message sent while it is off is kept and delivered \
             when it is back."
        ),
        "led" => format!(
            "Started a session led by you, {named}, on {machine}, {place}. It is a real session \
             of {machine}, not your child: the user sees it there and may talk to it, and the \
             agent_* tools do not reach it. It asks you its questions: a delivery \
             [flottille:session-turn-ended] wakes you at each end of its turns, with its last \
             message, kept while this machine is off. Answer with session_send, relay to the user \
             with ask, or do nothing; session_read reads it, and forge unsubscribe with its \
             session stops the wakes. {back}"
        ),
        _ => format!(
            "Started a session for the user, {named}, on {machine}, {place}, handed over. It is \
             the user's, not your child: the user follows it in the window and the agent_* tools \
             do not reach it. You hear nothing more of it unless it writes to you; you can write \
             to it with session_send. {back}"
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
    // What the sender says goes on a card and to the judge: bounded before either reads it.
    for id in [&asked.caller, &asked.tree] {
        if !agent_id(id) {
            return Err(format!(
                "{:?} is not an agent's id: 1 to 64 letters, digits, dashes or underscores",
                id.chars().take(64).collect::<String>()
            ));
        }
    }
    if asked.title.contains(|c: char| c.is_control()) {
        return Err("a title is one line, of no control character".to_owned());
    }
    let chars = asked.title.chars().count();
    if chars > LONGEST_TITLE {
        return Err(format!(
            "a title is at most {LONGEST_TITLE} characters, this one has {chars}"
        ));
    }
    if asked.branch.len() > LONGEST_BRANCH || !crate::forge::branch_name(&asked.branch) {
        return Err(format!(
            "{:?} is not a branch name: git's rules, at most {LONGEST_BRANCH} bytes",
            asked.branch.chars().take(64).collect::<String>()
        ));
    }
    let kind = match asked.kind.as_str() {
        "child" => LaunchKind::Child,
        "led" => LaunchKind::Led,
        "handed" => LaunchKind::Handed,
        other => return Err(format!("there is no kind of launch {other}")),
    };
    // A child is at its parent's depth plus one, on whichever machine it runs: the limit holds
    // across machines. A caller that sends no depth is a root (`LaunchAsk::depth`).
    if kind == LaunchKind::Child && asked.depth >= DEPTH_LIMIT {
        return Err(format!(
            "agent {} of {sender} is at the depth limit ({DEPTH_LIMIT}): it cannot launch \
             children",
            asked.caller
        ));
    }
    // Only a root starts a session, here as on its own machine.
    if kind != LaunchKind::Child && asked.depth > 0 {
        return Err(format!(
            "agent {} of {sender} is a child: only a root starts a session, led or handed",
            asked.caller
        ));
    }
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
    let launches = daemon
        .read(|store| store.launches("in", Some(&machine), LaunchFilter::All))
        .map_err(|error| error.to_string())?;
    let waiting = launches
        .iter()
        .filter(|launch| launch.agent.is_none() && launch.state != "refused")
        .count();
    if waiting >= MOST_WAITING {
        return Err(too_many_waiting(sender));
    }
    let held = launches
        .iter()
        .filter(|launch| launch.state == "held")
        .count();
    if held >= MOST_HELD {
        return Err(too_many_held(sender));
    }
    // The project it runs in: one this machine has, recognised by its repositories, or one made
    // for it once every check passed.
    let found = crate::travelled::recognise(daemon, &asked, here).await?;
    let project = match &found {
        crate::travelled::Found::Here(project) => (**project).clone(),
        crate::travelled::Found::Made { name, folder } => flottille_core::project::Project::new(
            name,
            folder.clone(),
            asked.repositories.clone(),
            &daemon.paths.data,
        ),
    };
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
    let mode = mode_taken(
        &asked.mode,
        asked.switch.as_deref(),
        sender,
        &asked.tree,
        members,
    );
    // Its project's settings, classed as the same writes of an agent here: one base makes the
    // launch the user's to decide, never the judge's.
    let mut classed = crate::travelled::classed(daemon, &found, &asked, sender, &machine)?;
    // No chain without the user, across machines as here: a session another agent started
    // starts a session only as base (`gate::LAUNCHED_LAUNCH`). A child is not held for it, as a
    // launched session's children are not here. A session this machine itself started on the
    // sender is such a session whatever the launch says; of any other caller, the sender's word
    // is taken (the machines are the user's own).
    let launched = asked.launched || started_there(daemon, &machine, &asked.caller);
    if launched && kind != LaunchKind::Child {
        classed.base = true;
        classed
            .reasons
            .push(crate::gate::LAUNCHED_LAUNCH.to_owned());
    }
    // The files its setup copies that the project here lacks: made from the vault entries the
    // user shares from the caller's machine. A secret written into a file is the user's to allow,
    // whatever the mode, those shared already named, those not yet asked for.
    let needs = crate::travelled::needs(&asked, &project.folder);
    let missing = crate::travelled::missing(daemon, &needs, sender);
    let name_of = |key: &str| {
        members
            .members()
            .iter()
            .find(|member| {
                member
                    .machine
                    .as_ref()
                    .is_some_and(|k| hex(k.as_bytes()) == key)
            })
            .map(|member| member.name.clone())
    };
    let said = |found: &crate::travelled::Found| {
        let mut says = words(kind, &asked, sender);
        says.splice(
            1..1,
            crate::travelled::words(found, &asked, (sender, &machine), name_of, &classed),
        );
        let lays = crate::travelled::lays(&asked.project, sender, &needs, &missing);
        says.splice(0..0, lays);
        if !missing.is_empty() {
            says.insert(
                0,
                crate::travelled::asked_to_share(&asked.project, here, &missing),
            );
        }
        says
    };
    let says = said(&found);
    let judgement = match mode {
        GateMode::Full => None,
        GateMode::Ask => None,
        GateMode::Auto if classed.base || !needs.is_empty() => None,
        GateMode::Auto => match crate::judge::chosen(daemon).await {
            Some(judge) => {
                let place = Place::main(&project, project.folder.clone());
                let args = json!({"kind": asked.kind, "machine": sender, "title": asked.title,
                    "brief": asked.brief});
                let mut reasons = says.clone();
                reasons.extend(classed.reasons.iter().cloned());
                Some(crate::judge::judge_launch(daemon, &judge, &place, &args, &reasons).await)
            }
            None => None,
        },
    };
    let accepted = needs.is_empty()
        && (mode == GateMode::Full
            || judgement
                .as_ref()
                .is_some_and(|judged| judged.verdict == "allow"));
    let mut reasons = classed.reasons.clone();
    let laid: Vec<String> = needs
        .iter()
        .filter(|need| !missing.contains(need))
        .map(|need| format!("{} (the vault entry {})", need.path, need.entry))
        .collect();
    if !laid.is_empty() {
        reasons.push(format!(
            "it writes into this machine's project secrets of the vault that {sender} shared: {}",
            crate::travelled::listed(&laid)
        ));
    }
    if !missing.is_empty() {
        let lacking: Vec<String> = missing
            .iter()
            .map(|need| format!("{} (the vault entry {})", need.path, need.entry))
            .collect();
        reasons.push(format!(
            "its setup copies what this machine lacks, which {sender} shares through the vault: {}",
            crate::travelled::listed(&lacking)
        ));
    }
    let state = if accepted { "accepted" } else { "held" };
    // The project is recognised again first, two launches of a new one at once making it once;
    // it is made only once the launch is recorded. The words kept are said of it as it is now.
    let (found, _making) = crate::travelled::again(daemon, found, &asked, here).await?;
    let says = said(&found);
    let (name, made, folder) = match &found {
        crate::travelled::Found::Here(project) => (project.name.clone(), false, None),
        crate::travelled::Found::Made { name, folder } => (name.clone(), true, Some(folder)),
    };
    let record = Launch {
        way: "in".into(),
        machine,
        launch: asked.launch.clone(),
        caller: asked.caller.clone(),
        kind: asked.kind.clone(),
        project: name.clone(),
        state: state.into(),
        request: None,
        agent: None,
        workspace: None,
        workspace_id: None,
        asked: json!({
            "asked": asked,
            "from": sender,
            "mode": mode.as_str(),
            "says": says,
            "reasons": reasons,
            "base": classed.base,
            "classed": classed.text,
            "judgement": judgement,
            "made": made,
            "folder": folder,
            "needs": needs,
            "missing": missing,
        }),
        at: now_ms(),
    };
    // Counted again as it is recorded: launches asked at once never go past the caps.
    let refused = daemon
        .write(|store| {
            store
                .add_launch(&record, MOST_WAITING, MOST_HELD)
                .map(|refused| (refused, None))
        })
        .map_err(|error| error.to_string())?;
    match refused {
        None => {}
        Some(LaunchRefused::Known) => {
            return Err(format!("launch {} was asked already", record.launch));
        }
        Some(LaunchRefused::Waiting) => return Err(too_many_waiting(sender)),
        Some(LaunchRefused::Held) => return Err(too_many_held(sender)),
    }
    // Its project's settings are laid only once the launch is allowed: now, or at the user's
    // answer.
    let prepared = crate::travelled::made(daemon, &found, &asked, &record.machine).and_then(|()| {
        if !accepted {
            return Ok(());
        }
        let came = serde_json::to_value(&asked).unwrap_or_default();
        crate::travelled::take_settings(
            daemon,
            &name,
            &came,
            &record.machine,
            classed.text.as_deref(),
        )
    });
    if let Err(why) = prepared {
        let _ = daemon.write(|store| {
            store
                .forget_launch("in", &record.machine, &record.launch)
                .map(|()| ((), None))
        });
        crate::travelled::unmake(daemon, &record);
        return Err(why);
    }
    eprintln!(
        "flottille daemon: launch {} of agent {} of {sender} {state} ({})",
        record.launch,
        asked.caller,
        mode.as_str()
    );
    Ok(json!({"state": state}))
}

fn too_many_waiting(sender: &str) -> String {
    format!(
        "{sender} has {MOST_WAITING} launches waiting for their code already: send their code \
         first"
    )
}

fn too_many_held(sender: &str) -> String {
    format!(
        "{sender} has {MOST_HELD} launches waiting for the user already: the user answers them \
         first"
    )
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
async fn sweep(daemon: &Arc<Daemon>, machine: &str) {
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
        // Nothing of it is set up: what was made of its code goes with it, and the project made
        // for it.
        if let Some(workspace) = launch.workspace_id {
            remove_workspace(daemon, workspace, None);
        }
        crate::travelled::unmake(daemon, &launch);
    }
}

/// Removes the workspace `id` made of a launch's code that was refused or forgotten, once `agent`,
/// its agent, has ended: what it holds is a copy of the sender's work, never this machine's. Only
/// while each of its repositories is still one laid of code that came: any other is kept.
fn remove_workspace(daemon: &Arc<Daemon>, id: i64, agent: Option<String>) {
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while let Some(agent) = &agent
            && tokio::time::Instant::now() < deadline
            && !daemon
                .read(|store| store.agent(agent))
                .ok()
                .flatten()
                .is_none_or(|agent| {
                    agent.state.is_final() && !crate::agents::stopping(&daemon, &agent.id)
                })
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let Ok(workspace) = daemon.read(|store| store.workspace_by_id(id)) else {
            return;
        };
        let (project, name) = (workspace.project.clone(), workspace.name.clone());
        let removed = remove_unless_work_came(&daemon, &workspace, async || {});
        if let Err(why) = removed.await {
            eprintln!(
                "flottille daemon: the workspace {name} of project {project}, made of a launch \
                 that did not start, is kept: {why}"
            );
        }
    });
}

/// Removes `workspace`, made of code that came, unless something in it changes while it is read:
/// the function does not tell the code that came from other work, it refuses only what moves
/// between its two reads. What it holds is read first, then what the removal would lose, which it
/// checks again: a file written between the two reads, even one said lost the same, differs from
/// the first, and the workspace is kept. `between` runs after the first read.
async fn remove_unless_work_came(
    daemon: &Arc<Daemon>,
    workspace: &flottille_core::workspace::Workspace,
    between: impl AsyncFnOnce(),
) -> Result<(), String> {
    if workspace.repositories.iter().any(|r| r.laid.is_none()) {
        return Err("it is not made only of code that came".to_owned());
    }
    let digest = crate::workspaces::digest(workspace)
        .await
        .map_err(|failure| failure.message)?;
    between().await;
    let losses = crate::workspaces::losses(daemon, &workspace.project, &workspace.name)
        .await
        .map_err(|failure| failure.message)?;
    crate::workspaces::remove_with_project(daemon.clone(), workspace, Some((&losses, &digest)))
        .await
        .map_err(|failure| failure.message)
        .map(|_| ())
}

/// Refused when what `git` says of the code it sends is not the launch's: its project, one of its
/// repositories, its branch.
pub(crate) fn refuse_other_code(launch: &Launch, git: &flottille_link::Git) -> Result<(), String> {
    use flottille_link::Git;
    let asked: LaunchAsk = serde_json::from_value(launch.asked["asked"].clone())
        .map_err(|error| format!("the launch cannot be read: {error}"))?;
    let name = &launch.launch;
    let (project, repositories, branch): (&str, Vec<&String>, Option<&str>) = match git {
        Git::Has(has) => (&has.project, vec![&has.repository], None),
        Git::Offer(offer) => (&offer.project, vec![&offer.repository], None),
        Git::Place(place) => (
            &place.project,
            place.repositories.iter().collect(),
            Some(&place.branch),
        ),
        Git::Fetch(_) => return Ok(()),
    };
    // The sender names its project as it calls it: the project here may have another name.
    if project != asked.project {
        return Err(format!(
            "launch {name} is for project {}, not {project}: no code of another is taken for it",
            asked.project
        ));
    }
    let whole = matches!(git, Git::Place(_));
    let mut given: Vec<&String> = repositories.clone();
    given.sort();
    let mut taken: Vec<&String> = asked.repositories.iter().collect();
    taken.sort();
    if repositories
        .iter()
        .any(|path| !asked.repositories.contains(path))
        || (whole && given != taken)
    {
        return Err(format!(
            "launch {name} takes the repositories {}, no other",
            asked.repositories.join(", ")
        ));
    }
    if let Some(branch) = branch
        && branch != asked.branch
    {
        return Err(format!(
            "launch {name} is on {}, not {branch}",
            asked.branch
        ));
    }
    Ok(())
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

/// Whether the setup of `workspace` waits: made of code another machine sent, it runs its setup
/// only as the launch that very workspace was made for (by its id, never its name, which a later
/// one may take), once it is allowed (accepted by the gate or by the user). Held by default: a
/// launch held, refused or forgotten, or none, runs nothing.
pub(crate) fn holds(daemon: &Daemon, workspace: &flottille_core::workspace::Workspace) -> bool {
    if workspace.repositories.iter().all(|r| r.laid.is_none()) {
        return false;
    }
    !daemon
        .read(|store| store.launches("in", None, LaunchFilter::All))
        .unwrap_or_default()
        .iter()
        .any(|launch| {
            matches!(launch.state.as_str(), "accepted" | "started")
                && launch.project == workspace.project
                && launch.workspace_id == Some(workspace.id)
        })
}

/// The agent of `launch`, in the workspace `placed` names: started at once when the gate accepted
/// it, or made to wait, with the user's request, when it is held. What `placed` says, with the
/// agent's id and the request's number.
pub(crate) async fn placed(
    daemon: &Arc<Daemon>,
    launch: Launch,
    placed: Value,
) -> Result<Value, String> {
    let name = placed["workspace"].as_str().unwrap_or_default();
    let id = daemon
        .read(|store| store.workspace(&launch.project, name))
        .ok()
        .flatten()
        .map(|workspace| workspace.id);
    let started = with_agent(daemon, launch, placed, id).await;
    // A workspace whose launch has no agent would wait for nothing: it goes.
    if started.is_err()
        && let Some(id) = id
    {
        remove_workspace(daemon, id, None);
    }
    started
}

async fn with_agent(
    daemon: &Arc<Daemon>,
    mut launch: Launch,
    mut placed: Value,
    workspace_id: Option<i64>,
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
    // A led or handed session records the agent that started it as one started here does: the
    // caller, stood in for here.
    let led = launch.kind == "led";
    let pilot = match !child {
        true => Some(crate::across::pilot_stand_in(
            daemon,
            &launch,
            &asked.project,
            mode,
        )?),
        false => None,
    };
    let agent = agents::create(
        daemon,
        &project,
        New {
            id: None,
            workspace: Some(workspace.clone()),
            preparing: true,
            role,
            parent: None,
            // A child here of an agent of there: at that agent's depth plus one.
            remote_parent_depth: child.then_some(asked.depth),
            title: Some(asked.title.trim().to_owned()),
            brief: asked.brief.clone(),
            provider: Some(asked.provider.clone()),
            model: asked.model.clone(),
            effort: asked.effort.clone(),
            gate_mode: Some(mode),
            launched_by: pilot,
            led,
            launched: None,
        },
    )
    .map_err(|failure| failure.message)?;
    launch.agent = Some(agent.id.clone());
    if launch.state == "held" {
        let says: Vec<String> =
            serde_json::from_value(launch.asked["says"].clone()).unwrap_or_default();
        let judgement: Option<Judgement> =
            serde_json::from_value(launch.asked["judgement"].clone()).unwrap_or_default();
        let mut shown = json!({"kind": asked.kind, "machine": launch.asked["from"],
            "title": asked.title, "brief": asked.brief});
        // What the user is asked to share from the caller's machine, by the entries' names:
        // what a device shares before it allows the launch.
        if launch.asked["missing"]
            .as_array()
            .is_some_and(|missing| !missing.is_empty())
        {
            shown["needs"] =
                json!({"from": launch.asked["from"], "files": launch.asked["missing"]});
        }
        let request = Request {
            number: 0,
            agent: agent.id.clone(),
            call_key: Some(format!("launch-{}", launch.launch)),
            tool: "agent_launch".into(),
            arguments: shown.clone(),
            shown,
            reasons: std::iter::once(format!(
                "it starts an agent {} asked for on this machine",
                launch.asked["from"].as_str().unwrap_or_default()
            ))
            .chain(
                serde_json::from_value::<Vec<String>>(launch.asked["reasons"].clone())
                    .unwrap_or_default(),
            )
            .collect(),
            base: launch.asked["base"] == true,
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
        // Never one that lays a file: that one is held for the user.
        launch.state = "started".into();
    }
    // The workspace is recorded as it is in this very write: one removed since `placed` read it
    // is none, never an id a later workspace may take.
    let recorded = daemon
        .write(|store| {
            store
                .place_launch(&launch, workspace_id)
                .map(|recorded| (recorded, None))
        })
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("launch {} was forgotten meanwhile", launch.launch))?;
    launch.workspace_id = recorded;
    if launch.state == "started" {
        go_on(daemon, &launch, &agent.id);
    }
    placed["session"] = agent.id.clone().into();
    // Which project it runs in, whether it was made for it, and whose settings it keeps: what
    // the caller's row and answer say of it.
    placed["project"] = launch.project.clone().into();
    placed["made"] = (launch.asked["made"] == true).into();
    if let Some(kept) = kept_of(&project, &launch) {
        placed["kept"] = kept.into();
    }
    Ok(placed)
}

/// The setup of the workspace of `launch`, allowed now, goes on, and its agent `agent` is sent its
/// first message once it is ready.
fn go_on(daemon: &Arc<Daemon>, launch: &Launch, agent: &str) {
    if let Some(id) = launch.workspace_id
        && let Ok(made) = daemon.read(|store| store.workspace_by_id(id))
        && made.state() == flottille_core::workspace::State::Preparing
    {
        crate::workspaces::go_on_preparing(daemon, made.id);
    }
    if let Ok(Some(agent)) = daemon.read(|store| store.agent(agent)) {
        tokio::spawn(crate::tree::brief_once_ready(daemon.clone(), agent));
    }
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
/// on and its agent is sent its first message once it is ready; refused, its agent ends and its
/// workspace is removed.
pub(crate) fn decided(daemon: &Arc<Daemon>, mut launch: Launch, request: &Request, allowed: bool) {
    let Some(agent) = launch.agent.clone() else {
        return;
    };
    let state = match allowed {
        true => "started",
        false => "refused",
    };
    // Settled first, and only while it is held: a launch whose agent ended meanwhile
    // (`cancelled`) lays nothing. Its workspace is read as it is now, never as `launch` was read.
    let settled = match daemon.write(|store| {
        store
            .settle_launch(&launch.machine, &launch.launch, state)
            .map(|settled| (settled, None))
    }) {
        Ok(Some(workspace_id)) => {
            launch.state = state.into();
            launch.workspace_id = workspace_id;
            true
        }
        Ok(None) => false,
        Err(why) => {
            eprintln!(
                "flottille daemon: launch {}: its answer cannot be kept: {why}",
                launch.launch
            );
            false
        }
    };
    if !settled {
        return;
    }
    // The files it needs are made from the vault first, which is read off this thread.
    let needs = needs_of(&launch);
    if allowed && !needs.is_empty() {
        let (daemon, request) = (daemon.clone(), request.clone());
        tokio::spawn(async move {
            let from = launch.asked["from"].as_str().unwrap_or_default().to_owned();
            let laid = crate::travelled::lay(&daemon, &launch.project, &needs, &from).await;
            finish(&daemon, launch, &request, agent, laid.map_err(Some));
        });
        return;
    }
    let answer = match allowed {
        true => Ok(()),
        false => Err(None),
    };
    finish(daemon, launch, request, agent, answer);
}

/// The files the setup of `launch` copies that its project here lacked when it was judged.
fn needs_of(launch: &Launch) -> Vec<crate::travelled::Need> {
    serde_json::from_value(launch.asked["needs"].clone()).unwrap_or_default()
}

/// The end of the user's answer to the held `launch`, its agent `agent`: `Ok` when it is allowed
/// and what it needed is made, `Err` with why when that failed, or `Err(None)` when the user
/// refused it.
fn finish(
    daemon: &Arc<Daemon>,
    mut launch: Launch,
    request: &Request,
    agent: String,
    answer: Result<(), Option<String>>,
) {
    // Then its project's settings, which its setup follows, laid only on the project the user
    // allowed them for: a launch whose settings cannot be used here, or whose project changed
    // since, is refused, saying why.
    let answer = answer.and_then(|()| {
        crate::travelled::take_settings(
            daemon,
            &launch.project,
            &launch.asked["asked"],
            &launch.machine,
            launch.asked["classed"].as_str(),
        )
        .map_err(Some)
    });
    if let Err(Some(why)) = &answer {
        eprintln!(
            "flottille daemon: launch {} is refused: {why}",
            launch.launch
        );
        launch.state = "refused".into();
        if let Err(error) = daemon.write(|store| {
            store
                .refuse_started_launch(&launch.machine, &launch.launch)
                .map(|()| ((), None))
        }) {
            eprintln!(
                "flottille daemon: launch {}: its refusal cannot be kept: {error}",
                launch.launch
            );
        }
    }
    let outcome = match answer {
        Ok(()) => "done",
        Err(_) => "refused",
    };
    if let Err(error) = daemon.write(|store| Ok(((), store.settle_unsent(request, outcome)?))) {
        eprintln!(
            "flottille daemon: request #{}: its answer cannot be stored: {error}",
            request.number
        );
    }
    if let Err(why) = answer {
        let missing: Vec<crate::travelled::Need> =
            serde_json::from_value(launch.asked["missing"].clone()).unwrap_or_default();
        let reason = match (&why, missing.is_empty()) {
            (Some(why), _) => format!("its launch is refused: {why}"),
            (None, true) => "the user refused its launch".to_owned(),
            (None, false) => {
                let paths: Vec<String> = missing.iter().map(|need| need.path.clone()).collect();
                format!(
                    "the user refused its launch, which needed {} here",
                    crate::travelled::listed(&paths)
                )
            }
        };
        // The agent that launched it is told, and what it lacked, as a held call's result comes:
        // it would wait for nothing.
        let here = this_machine(daemon);
        let asked = asked_of(request.number, &here);
        let body = match (&why, missing.is_empty()) {
            (_, false) => refused_for_missing(&asked, &here, &missing, why.as_deref()),
            (Some(why), true) => format!(
                "{asked} was approved by the user, but nothing was done: it could not start \
                 there: {why}."
            ),
            (None, true) => format!("{asked} was refused by the user. Nothing was done."),
        };
        crate::across::tell_decided(daemon, &launch, &body, false);
        crate::tree::stop_tree(daemon, &agent, &reason);
        // Its setup never runs: the workspace made of its code goes once its agent ended, and
        // the project made for it then.
        if let Some(workspace) = launch.workspace_id {
            remove_workspace(daemon, workspace, Some(agent));
        }
        crate::travelled::unmake(daemon, &launch);
        return;
    }
    // Its workspace was made while it was held, before these settings were laid: its setup is
    // the project's as it is now.
    if let Some(id) = launch.workspace_id
        && let Err(why) = crate::workspaces::resnapshot(daemon, id)
    {
        eprintln!(
            "flottille daemon: launch {}: its setup is not taken again: {why}",
            launch.launch
        );
    }
    // Told before its setup goes on, so that a setup that fails at once is never heard of first.
    let here = this_machine(daemon);
    let asked = asked_of(request.number, &here);
    let title = launch.asked["asked"]["title"].as_str().unwrap_or_default();
    let placed = placed_of(daemon, &launch, &agent);
    let result = answer_to_caller(&launch.kind, title, &here, &launch.launch, &placed);
    crate::across::tell_decided(
        daemon,
        &launch,
        &format!(
            "{asked} was approved by the user, and Flottille has now done it. Its \
             result:\n\n{result}"
        ),
        true,
    );
    go_on(daemon, &launch, &agent);
}

/// What the agent `agent` of the held `launch`, allowed now, was placed in: as `placed` says it
/// to the caller's answer.
fn placed_of(daemon: &Daemon, launch: &Launch, agent: &str) -> Value {
    let branch = launch
        .workspace_id
        .and_then(|id| daemon.read(|store| store.workspace_by_id(id)).ok())
        .map(|workspace| workspace.branch)
        .unwrap_or_default();
    let mut placed = json!({"session": agent, "workspace": launch.workspace, "branch": branch});
    if let Ok(project) = projects::load_project(daemon, &launch.project)
        && let Some(kept) = kept_of(&project, launch)
    {
        placed["project"] = launch.project.clone().into();
        placed["kept"] = kept.into();
    }
    placed
}

/// Whose settings the project of `launch` here keeps instead of the caller's, when it keeps its
/// own: `user` when the user registered it, `machine` when it was made for another machine.
fn kept_of(project: &flottille_core::project::Project, launch: &Launch) -> Option<&'static str> {
    let settings = !launch.asked["asked"]["settings"].is_null();
    if !settings || crate::travelled::takes(project, &launch.machine) {
        return None;
    }
    Some(match project.from_machine {
        None => "user",
        Some(_) => "machine",
    })
}

/// This machine's name among the user's, as the agents of the others know it.
fn this_machine(daemon: &Daemon) -> String {
    crate::link::remote_of(daemon)
        .ok()
        .and_then(|remote| remote.listed().ok().map(|(_, name)| name))
        .unwrap_or_else(|| "the other machine".to_owned())
}

/// The request `number` of a held launch, as its caller is told of the user's answer, as a held
/// call's answer names its request: `Request #4 on server (agent_launch)`.
fn asked_of(number: u64, here: &str) -> String {
    format!("Request #{number} on {here} (agent_launch)")
}

/// What the agent of the other machine that asked `asked` is told when it is refused for lacking
/// `missing` here: refused by the user, or, with `why`, allowed while they were still not shared.
fn refused_for_missing(
    asked: &str,
    here: &str,
    missing: &[crate::travelled::Need],
    why: Option<&str>,
) -> String {
    let lacking: Vec<String> = missing
        .iter()
        .map(|need| format!("{} (the vault entry {})", need.path, need.entry))
        .collect();
    let what = match why {
        None => "was refused by the user. Nothing was done".to_owned(),
        Some(_) => "was approved by the user, but nothing was done: it was allowed before what it \
             needs there was shared"
            .to_owned(),
    };
    format!(
        "{asked} {what}. Its setup copies what {here} lacks: {}, which the user shares from this \
         machine through the vault. Launch it again once they are shared.",
        crate::travelled::listed(&lacking)
    )
}

/// The agent `agent` ended, its requests cancelled: a launch it was held for the user's answer
/// is refused, as if the user had refused it, and its workspace goes, so that it counts against
/// no cap and keeps no place.
pub(crate) fn cancelled(daemon: &Arc<Daemon>, agent: &str) {
    let held = daemon
        .read(|store| store.launches("in", None, LaunchFilter::Agent(agent)))
        .unwrap_or_default()
        .into_iter()
        .find(|launch| launch.state == "held");
    let Some(launch) = held else {
        return;
    };
    let settled = daemon.write(|store| {
        store
            .settle_launch(&launch.machine, &launch.launch, "refused")
            .map(|settled| (settled, None))
    });
    if let Ok(Some(Some(workspace))) = settled {
        remove_workspace(daemon, workspace, Some(agent.to_owned()));
    }
    if matches!(settled, Ok(Some(_))) {
        // Its caller would wait for the user's answer for ever.
        let here = this_machine(daemon);
        let asked = asked_of(launch.request.unwrap_or_default(), &here);
        crate::across::tell_decided(
            daemon,
            &launch,
            &format!(
                "{asked} was refused: its session there ended before the user of {here} \
                 decided. Nothing was done."
            ),
            false,
        );
        crate::travelled::unmake(daemon, &launch);
    }
}

/// For an agent started here for an agent of another machine: that agent, its machine's name,
/// and the launch's kind (`child`, `led` or `handed`).
pub(crate) fn launched_from(daemon: &Daemon, agent: &str) -> Option<(String, String, String)> {
    let launch = daemon
        .read(|store| store.launches("in", None, LaunchFilter::Agent(agent)))
        .ok()?
        .into_iter()
        .next()?;
    let from = launch.asked["from"]
        .as_str()
        .unwrap_or("another machine")
        .to_owned();
    Some((launch.caller, from, launch.kind))
}

/// [`launched_from`] as `agent.list` and `agent.show` say it, `{machine, agent, kind}`: the window,
/// the CLI and the phone draw a child of it as that agent's, never as a session of the user's;
/// `null` for an agent started here.
pub(crate) fn launched_from_shown(daemon: &Daemon, agent: &str) -> Value {
    match launched_from(daemon, agent) {
        Some((caller, machine, kind)) => json!({"machine": machine, "agent": caller, "kind": kind}),
        None => Value::Null,
    }
}

/// Whether `agent` is a session another agent started: here (`launched_by`), or on another
/// machine. Its own launches come to the user (`gate::LAUNCHED_LAUNCH`).
pub(crate) fn launched(daemon: &Daemon, agent: &Agent) -> bool {
    agent.launched_by.is_some() || launched_from(daemon, &agent.id).is_some()
}

/// Whether `agent` of the machine `machine` (its key in hex) is a session an agent of this
/// machine launched there.
fn started_there(daemon: &Daemon, machine: &str, agent: &str) -> bool {
    daemon
        .read(|store| store.launches("out", Some(machine), LaunchFilter::Agent(agent)))
        .unwrap_or_default()
        .iter()
        .any(|launch| launch.state != "refused")
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

#[cfg(test)]
mod tests {
    use super::*;
    use flottille_core::workspace::{Base, Workspace, Worktree};

    /// Runs `git` in `dir`, in a home and a configuration of its own.
    async fn git(dir: &std::path::Path, args: &[&str]) {
        let nowhere = std::ffi::OsStr::new("/dev/null");
        let name = std::ffi::OsStr::new("Test");
        let email = std::ffi::OsStr::new("test@example.org");
        let env = [
            ("GIT_CONFIG_GLOBAL", nowhere),
            ("GIT_CONFIG_NOSYSTEM", std::ffi::OsStr::new("1")),
            ("GIT_AUTHOR_NAME", name),
            ("GIT_COMMITTER_NAME", name),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_COMMITTER_EMAIL", email),
        ];
        crate::git::output_with(dir, args, crate::git::Class::Work, &env)
            .await
            .unwrap();
    }

    /// A workspace made of code that came (a repository laid), in a throwaway repository, with
    /// `notes.txt` written in it and not committed: what the sender's work was.
    async fn made_of_code_that_came(dir: &std::path::Path) -> (Arc<Daemon>, Workspace) {
        let project = dir.join("app");
        let folder = dir.join("workspace");
        std::fs::create_dir_all(&project).unwrap();
        git(&project, &["init", "--quiet", "-b", "main"]).await;
        std::fs::write(project.join("a.txt"), "a\n").unwrap();
        git(&project, &["add", "a.txt"]).await;
        git(&project, &["commit", "--quiet", "-m", "first"]).await;
        let commit = crate::git::output(&project, &["rev-parse", "HEAD"], crate::git::Class::Read)
            .await
            .unwrap()
            .trim()
            .to_owned();
        let at = folder.to_string_lossy().into_owned();
        git(&project, &["worktree", "add", "--quiet", "-b", "fix", &at]).await;
        std::fs::write(folder.join("notes.txt"), "one\n").unwrap();
        let mut workspace = Workspace {
            id: 0,
            project: "app".into(),
            name: "workspace".into(),
            branch: "fix".into(),
            folder,
            project_folder: project,
            created_at: 1,
            variables: Default::default(),
            repositories: vec![Worktree {
                path: ".".into(),
                base: Base {
                    branch: "main".into(),
                    remote: "origin".into(),
                    reference: "refs/heads/main".into(),
                    commit,
                    fetched_now: false,
                    fetched_at: None,
                    note: None,
                    fetch: None,
                },
                resumed: false,
                resumed_branch: None,
                laid: Some("laid".into()),
            }],
            steps: Vec::new(),
            ticket: None,
        };
        let daemon = crate::daemon::tests::a_daemon(dir, false);
        workspace.id = daemon
            .write(|store| {
                let (id, event) =
                    store.create_workspace(&workspace, ("workspace.created", json!({})))?;
                Ok((id, event))
            })
            .unwrap();
        (daemon, workspace)
    }

    #[tokio::test]
    async fn a_workspace_of_code_that_came_goes_when_nothing_was_written_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let (daemon, workspace) = made_of_code_that_came(dir.path()).await;

        let removed = remove_unless_work_came(&daemon, &workspace, async || {}).await;

        assert_eq!(removed, Ok(()));
        assert!(!workspace.folder.exists());
    }

    #[tokio::test]
    async fn work_written_after_a_held_agent_stops_is_not_lost_by_the_removal() {
        let dir = tempfile::tempdir().unwrap();
        let (daemon, workspace) = made_of_code_that_came(dir.path()).await;
        let notes = workspace.folder.join("notes.txt");

        // The file the sender's work holds is written again between the two reads of what the
        // workspace holds: the same sentence says it lost, only its content tells.
        let removed = remove_unless_work_came(&daemon, &workspace, async || {
            std::fs::write(&notes, "two\n").unwrap();
        })
        .await;

        assert!(removed.is_err(), "{removed:?}");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "two\n");
    }
}
