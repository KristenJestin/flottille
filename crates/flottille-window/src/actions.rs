//! What the window asks of the daemon, off the UI thread: reading a project page's details,
//! creating, preparing and removing a workspace, starting a session. Each answer comes back to the
//! UI as an [`Update::Done`]; nothing here waits on the UI, and the UI never waits on it.
//!
//! The details are read on a thread and a connection of their own. Every other request has its
//! own thread and connection, so a workspace removed (git, up to half an hour) holds neither a
//! session started meanwhile nor the details. Every answer is waited for a bounded time: past it
//! the daemon is taken as silent, the request answers so (a session not started, a workspace
//! refused, the details not changed) and its connection is dropped.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use crate::commands::CommandInfo;
use crate::link::{Conn, Connect, Update};
use crate::rules::Shown;
use crate::runs::{CommandOutput, CommandRow, Watch};
use crate::session_link::{PullRequest, TICKET_FRESH, TicketFacts, pull_requests, ticket_facts};
use crate::skills::SkillInfo;
use crate::tasks::TaskInfo;

/// Where a session is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The project folder.
    Main,
    /// A workspace there is.
    Workspace(String),
}

/// What a session is started on: each left out takes the daemon's default (the project's, else
/// the provider's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// An image and its MIME type, as the daemon takes and gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl Picture {
    fn to_json(&self) -> Value {
        use base64::Engine as _;
        json!({"mime": self.mime,
            "data": base64::engine::general_purpose::STANDARD.encode(&self.bytes)})
    }

    fn of(found: &Value) -> Option<Picture> {
        use base64::Engine as _;
        Some(Picture {
            mime: found["mime"].as_str()?.to_owned(),
            bytes: base64::engine::general_purpose::STANDARD
                .decode(found["data"].as_str()?)
                .ok()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// What the project page shows beyond the shell's read: branches and lines changed.
    Details(String),
    Start {
        project: String,
        target: Target,
        pick: Pick,
        message: String,
    },
    /// Enables or disables a provider in `providers.toml`.
    SetProvider {
        name: String,
        enabled: bool,
    },
    /// Chooses the Auto judge in `providers.toml`, or none.
    SetJudge {
        provider: Option<String>,
        model: Option<String>,
    },
    /// Reads the providers again: `providers.toml` is the user's to edit by hand too.
    ReadProviders,
    /// The developer mode (`developer.show`).
    DeveloperRead,
    /// Switches the developer mode (`developer.set`).
    DeveloperSet {
        log: bool,
    },
    /// The user saw the notice of the Auto judge Flottille picked (`provider.judge_noticed`);
    /// the providers are read again.
    JudgeNoticed,
    /// The Auto judge's guidance (`judge.show`).
    JudgeRead,
    /// Saves the judge's guidance, or with no text resets it to the default (`judge.set`,
    /// `judge.reset`).
    JudgeWrite {
        text: Option<String>,
    },
    CreateWorkspace {
        project: String,
        name: String,
        /// The ticket to link it to: `#123` or a Jira key.
        ticket: Option<String>,
        /// The repositories it takes; the project's default set when none.
        repositories: Option<Vec<String>>,
        /// Its branch; `<prefix>/<name>` when none.
        branch: Option<String>,
        /// The base branch chosen for a repository, by path; its rule for the others.
        bases: std::collections::BTreeMap<String, String>,
    },
    /// The project's repositories as their checkouts stand (`project.repositories`), or one
    /// added or removed (`project.add_repository`, `project.remove_repository`): each answers
    /// the list.
    Repositories {
        project: String,
        method: &'static str,
        params: Value,
        /// The section's request number, given back with the answer.
        serial: u64,
    },
    /// What the project's settings show: its declared forges and tracker, and what each
    /// repository's remote proposes.
    ReadProjectSettings(String),
    /// Declares the project's forges and tracker (`project.set_settings`'s parameters).
    SaveProjectSettings {
        project: String,
        settings: Value,
    },
    /// Detects the project's repositories again, against its file.
    CheckProject(String),
    /// Forgets the project, never its folder.
    RemoveProject(String),
    /// What registering a folder would register.
    DetectProject {
        folder: String,
        name: Option<String>,
    },
    /// Registers a folder with the repositories shown.
    AddProject {
        folder: String,
        name: Option<String>,
        repositories: Vec<String>,
    },
    /// A ticket of the project's tracker, read as the user types it.
    PreviewTicket {
        project: String,
        ticket: String,
    },
    /// Links a workspace to a ticket, or to none (`ticket` empty of a null).
    LinkTicket {
        project: String,
        name: String,
        ticket: Option<String>,
    },
    PrepareWorkspace {
        project: String,
        name: String,
    },
    RemoveWorkspace {
        project: String,
        name: String,
    },
    /// The project's commands.
    Commands {
        project: String,
    },
    /// One command whole, to edit it.
    CommandRead {
        project: String,
        name: String,
    },
    CommandWrite {
        project: String,
        write: CommandWrite,
    },
    /// The project's skills.
    Skills {
        project: String,
    },
    /// The project's tasks.
    Tasks {
        project: String,
    },
    /// The project's pull requests and tickets (`items.list`), and what it declares
    /// (`project.settings`).
    /// `asked` numbers the reads, so that an answer older than one applied is dropped.
    Items {
        project: String,
        asked: u64,
    },
    /// One pull request or ticket in full (`items.show`).
    Item {
        project: String,
        kind: String,
        key: String,
        repository: Option<String>,
        asked: u64,
    },
    /// A change to the tasks: `task.add`, `task.edit` or `task.remove`, with its parameters.
    TaskWrite {
        project: String,
        method: &'static str,
        params: Value,
    },
    /// Runs a task in the project folder, for the user.
    RunTask {
        project: String,
        name: String,
    },
    /// The tasks the user ran in the project, and the output of the one whose panel is open.
    ProjectRuns {
        project: String,
        output: Option<Watch>,
    },
    /// Stops a task the user ran and everything it started, or dismisses one that ended.
    ProjectRun {
        project: String,
        run: String,
        dismiss: bool,
    },
    /// The project's setup.
    Setup {
        project: String,
    },
    /// A change to the setup: `setup.add`, `setup.edit`, `setup.remove` or `setup.move`, with
    /// its parameters.
    SetupWrite {
        project: String,
        method: &'static str,
        params: Value,
    },
    /// What the project's `.claude` folder holds of skills or commands (`skill.detect`,
    /// `command.detect`).
    ClaudeFound {
        project: String,
        kind: ClaudeKind,
    },
    /// Imports from `.claude` the files named, or all that can be (`skill.import`,
    /// `command.import`).
    ClaudeImport {
        project: String,
        kind: ClaudeKind,
        names: Option<Vec<String>>,
    },
    /// One skill whole (a text skill's text), to edit it.
    SkillRead {
        project: String,
        name: String,
    },
    SkillWrite {
        project: String,
        write: SkillWrite,
    },
    /// The rules agents are told to work by: the user's for every project, or a project's own.
    RulesRead {
        project: Option<String>,
    },
    /// Saves the rules, or with no text resets them to the default (`rules.set`, `rules.reset`).
    RulesWrite {
        project: Option<String>,
        text: Option<String>,
    },
    /// A message to a session's root agent: the only agent the window writes to.
    Reply {
        session: String,
        message: String,
        /// Pasted or dropped pictures that go with it, as the clipboard gave them.
        images: Vec<Picture>,
    },
    /// Takes back a message that waits for the session's root (`edit`: to change it and send it
    /// again). The daemon refuses one the root read first.
    Unqueue {
        session: String,
        delivery: String,
        edit: bool,
    },
    /// Stops the root's turn and hands it a message that waits, next, ahead of the others.
    SendNow {
        session: String,
        delivery: String,
    },
    /// Reads back an image the session's agent was sent, by the name its event gave.
    Image {
        session: String,
        name: String,
    },
    /// Ends a session: its root agent and its tree stop.
    Stop {
        session: String,
    },
    /// Starts an ended session's root agent again on its own session.
    Resume {
        session: String,
    },
    /// Interrupts the turn the session's root agent runs; the agent stays.
    Interrupt {
        session: String,
    },
    /// Puts the session in Ask, Auto or Full (`ask`, `auto`, `full`): the user's alone to change.
    SetMode {
        session: String,
        mode: String,
    },
    /// Stops a command of the session and everything it started, or dismisses one that ended.
    Run {
        session: String,
        run: String,
        dismiss: bool,
    },
    /// Ends a subscription of the session's agents, as the agent's own `unsubscribe` does.
    CancelSubscription {
        session: String,
        subscription: String,
    },
    /// The user's answer to a request they hold, given on the page of the session of `session`
    /// (its root agent).
    Answer {
        session: String,
        number: u64,
        answer: Answer,
    },
}

/// What is imported from a project's `.claude` folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeKind {
    Skill,
    Command,
}

impl ClaudeKind {
    fn method(self, verb: &str) -> &'static str {
        match (self, verb) {
            (ClaudeKind::Skill, "detect") => "skill.detect",
            (ClaudeKind::Skill, _) => "skill.import",
            (ClaudeKind::Command, "detect") => "command.detect",
            (ClaudeKind::Command, _) => "command.import",
        }
    }
}

/// A change to a project's skills, as the daemon takes it: a skill is a path or a text.
#[derive(Clone, Debug, PartialEq)]
pub enum SkillWrite {
    Add {
        name: String,
        description: String,
        path: Option<String>,
        text: Option<String>,
    },
    /// What is given replaces what the skill had.
    Edit {
        name: String,
        description: String,
        path: Option<String>,
        text: Option<String>,
    },
    Remove {
        name: String,
    },
}

/// A change to a project's commands, as the daemon takes it.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandWrite {
    /// One of Flottille's templates, as it is.
    Template {
        template: String,
    },
    /// A new command.
    Add {
        name: String,
        description: String,
        instructions: String,
    },
    /// What is given replaces what the command had.
    Edit {
        name: String,
        description: String,
        instructions: String,
    },
    Remove {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Deny,
    Once,
}

/// What one project page shows beyond the shell's read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub project: String,
    /// Each repository's path and the branch its checkout in the project folder is on.
    pub branches: Vec<(String, Option<String>)>,
    /// The lines changed in each workspace, all its repositories together.
    pub changes: HashMap<String, Changes>,
    /// The pull requests of each workspace's repositories.
    pub pulls: HashMap<String, Vec<PullRequest>>,
    /// Where each repository of each workspace stands.
    pub statuses: HashMap<String, Vec<RepositoryStatus>>,
    /// The ticket each workspace is linked to, as its tracker said it.
    pub tickets: HashMap<String, TicketFacts>,
}

/// Where one repository of a workspace stands (`workspace.show`): the branch its worktree is on,
/// its commits ahead of its base, its files not committed, the setup steps played in it, and
/// why it started from its local branch, when it did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryStatus {
    pub path: String,
    pub branch: Option<String>,
    pub base: String,
    pub ahead: Option<u64>,
    pub uncommitted: Option<u64>,
    /// The setup's steps done (or skipped) in it, and how many there are.
    pub setup: (usize, usize),
    pub note: Option<String>,
}

impl RepositoryStatus {
    /// Each repository of a workspace as `workspace.show` says it.
    pub fn of_workspace(shown: &Value) -> Vec<RepositoryStatus> {
        let steps = shown["steps"].as_array().map_or(&[][..], Vec::as_slice);
        shown["repositories"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|repository| {
                let path = repository["path"].as_str().unwrap_or_default().to_owned();
                // The worktree step is the workspace's own, not the setup's.
                let setup: Vec<&Value> = steps
                    .iter()
                    .filter(|s| s["repository"] == path.as_str() && s["kind"] != "worktree")
                    .collect();
                let played = setup
                    .iter()
                    .filter(|s| s["state"] == "done" || s["state"] == "skipped")
                    .count();
                RepositoryStatus {
                    branch: repository["branch"].as_str().map(str::to_owned),
                    base: repository["base"]["branch"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    ahead: repository["ahead"].as_u64(),
                    uncommitted: repository["uncommitted"].as_u64(),
                    setup: (played, setup.len()),
                    note: repository["base"]["note"].as_str().map(str::to_owned),
                    path,
                }
            })
            .collect()
    }

    /// In one line: `app/login · 2 ahead of develop · 3 uncommitted · setup 2/2`.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        parts.extend(self.branch.clone());
        if let Some(ahead) = self.ahead {
            parts.push(format!("{ahead} ahead of {}", self.base));
        }
        match self.uncommitted {
            Some(0) => parts.push("clean".into()),
            Some(files) => parts.push(format!("{files} uncommitted")),
            None => {}
        }
        if self.setup.1 > 0 {
            parts.push(format!("setup {}/{}", self.setup.0, self.setup.1));
        }
        parts.join(" · ")
    }
}

/// The lines added and removed in a workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: u64,
    pub removed: u64,
    /// The daemon stopped reading new files: there are more lines than said.
    pub capped: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Details(Details),
    /// A session started: its root agent.
    Started {
        project: String,
        agent: String,
    },
    /// No session started, and why, in a sentence.
    NotStarted {
        project: String,
        why: String,
    },
    /// A workspace was created, prepared again or removed as asked.
    WorkspaceDone {
        project: String,
        name: String,
    },
    /// The daemon refused what was asked of a workspace, and why.
    WorkspaceRefused {
        project: String,
        name: String,
        why: String,
    },
    /// The project's settings as the daemon read them, or why not; `saved` when they answer a
    /// save.
    ProjectSettings {
        project: String,
        read: Result<Value, String>,
        saved: bool,
    },
    /// The project's repositories, read, added to or removed from, or why not.
    Repositories {
        project: String,
        read: Result<Value, String>,
        serial: u64,
    },
    /// The project's repositories detected again (`project.check`), or why not.
    ProjectChecked {
        project: String,
        read: Result<Value, String>,
    },
    /// The project was forgotten as asked, or the daemon refused, and why.
    ProjectRemoved {
        project: String,
        refused: Option<String>,
    },
    /// What a folder would register (`project.detect`), or why it cannot be.
    ProjectDetected {
        folder: String,
        read: Result<Value, String>,
    },
    /// A folder was registered (`project.add`'s answer), or why not.
    ProjectAdded {
        folder: String,
        read: Result<Value, String>,
    },
    /// A ticket as the project's tracker says it (`project.ticket`'s answer), or why not.
    TicketPreview {
        project: String,
        ticket: String,
        read: Result<Value, String>,
    },
    /// A workspace was linked to a ticket (or unlinked) as asked, or the daemon refused, and why.
    TicketLinked {
        project: String,
        name: String,
        refused: Option<String>,
    },
    /// The providers, read again, with the read's number (`providers::next_read`); `None` when
    /// the daemon could not be asked.
    Providers(u64, Option<crate::providers::Providers>),
    /// A provider was enabled or disabled as asked, or the daemon refused it with why.
    /// The Auto judge was chosen, or the daemon refused with why.
    JudgeSet {
        refused: Option<String>,
    },
    ProviderSet {
        name: String,
        refused: Option<String>,
    },
    Commands {
        project: String,
        listed: Result<Vec<CommandInfo>, String>,
    },
    CommandRead {
        project: String,
        name: String,
        read: Result<CommandInfo, String>,
    },
    /// A change to the commands was made, or refused with why.
    CommandWritten {
        project: String,
        refused: Option<String>,
    },
    Skills {
        project: String,
        listed: Result<Vec<SkillInfo>, String>,
    },
    SkillRead {
        project: String,
        name: String,
        read: Result<SkillInfo, String>,
    },
    /// The developer mode as the daemon says it, after a read (`written` false) or a switch.
    Developer {
        written: bool,
        shown: Result<crate::settings::Developer, String>,
    },
    /// The judge's guidance as the daemon says it, after a read (`written` false) or a save or
    /// a reset.
    JudgeGuidance {
        written: bool,
        shown: Result<Shown, String>,
    },
    /// The rules as the daemon says them, after a read (`written` false) or a save or a reset.
    Rules {
        project: Option<String>,
        written: bool,
        shown: Result<Shown, String>,
    },
    /// What `.claude` holds of a kind, each file with its state, or why it cannot be read.
    ClaudeFound {
        project: String,
        kind: ClaudeKind,
        found: Result<Vec<crate::claude::Found>, String>,
    },
    /// An import from `.claude`: the names imported and the files left out with why, or why
    /// nothing was.
    ClaudeImported {
        project: String,
        kind: ClaudeKind,
        read: Result<Value, String>,
    },
    /// A change to the skills was made, or refused with why.
    SkillWritten {
        project: String,
        refused: Option<String>,
    },
    Tasks {
        project: String,
        listed: Result<Vec<TaskInfo>, String>,
    },
    Items {
        project: String,
        asked: u64,
        read: Result<crate::items::Read, String>,
    },
    Item {
        project: String,
        kind: String,
        key: String,
        repository: Option<String>,
        asked: u64,
        shown: Result<Value, String>,
    },
    /// A change to the tasks was made, or refused with why.
    TaskWritten {
        project: String,
        refused: Option<String>,
    },
    /// A task was started for the user, or the daemon refused with why.
    TaskRun {
        project: String,
        name: String,
        refused: Option<String>,
    },
    /// The tasks the user ran in the project, and the output asked for.
    ProjectRuns {
        project: String,
        listed: Result<Vec<CommandRow>, String>,
        output: Option<Box<CommandOutput>>,
    },
    /// A task the user ran was stopped or dismissed, or the daemon refused with why.
    ProjectRunAnswered {
        project: String,
        refused: Option<String>,
    },
    Setup {
        project: String,
        listed: Result<Vec<flottille_core::project::SetupStep>, String>,
    },
    /// A change to the setup was made, or refused with why.
    SetupWritten {
        project: String,
        refused: Option<String>,
    },
    /// An image read back from the daemon, `None` when it could not be.
    ImageRead {
        session: String,
        name: String,
        image: Option<Picture>,
    },
    /// A message to the session's root was taken, or refused with why.
    Replied {
        session: String,
        refused: Option<String>,
    },
    /// A waiting message was taken back, with its text and pictures, or the daemon refused with
    /// why ("already read by the agent").
    Unqueued {
        session: String,
        delivery: String,
        edit: bool,
        taken: Result<(String, Vec<Picture>), String>,
    },
    /// A waiting message was handed to the root now, or the daemon refused with why.
    SentNow {
        session: String,
        delivery: String,
        refused: Option<String>,
    },
    /// The session's tree was stopped, or the daemon refused with why.
    Stopped {
        session: String,
        refused: Option<String>,
    },
    /// The session's root was resumed, or the daemon refused with why.
    Resumed {
        session: String,
        refused: Option<String>,
    },
    /// The session's turn was interrupted (or none ran), or the daemon refused with why.
    Interrupted {
        session: String,
        refused: Option<String>,
    },
    /// The session's mode was changed, or the daemon refused with why.
    ModeSet {
        session: String,
        refused: Option<String>,
    },
    /// A command was stopped or dismissed as asked, or the daemon refused with why.
    RunAnswered {
        session: String,
        refused: Option<String>,
    },
    /// A subscription was cancelled as asked, or the daemon refused with why.
    SubscriptionCancelled {
        session: String,
        refused: Option<String>,
    },
    /// A request was decided as asked (`deny`, `once`, `tree`), or the answer refused with why:
    /// for the page of `session` only.
    Answered {
        session: String,
        number: u64,
        decision: Option<String>,
        refused: Option<String>,
    },
}

/// How long an answer is waited for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    /// What answers at once, or after git reads: the details, a session started.
    pub quick: Duration,
    /// What runs git's work (a fetch, a worktree made or removed): its own limit is 30 minutes.
    pub slow: Duration,
}

pub(crate) const BOUNDS: Bounds = Bounds {
    quick: Duration::from_secs(120),
    slow: Duration::from_secs(31 * 60),
};

/// The details are read at most once in this time: a burst of changes asks for them once.
const DETAILS_EVERY: Duration = Duration::from_secs(1);

/// The way to the actions.
#[derive(Clone)]
pub struct Actions {
    details: Sender<String>,
    connect: Connect,
    updates: UnboundedSender<Update>,
    bounds: Bounds,
}

impl Actions {
    /// Starts the details' thread; it ends when every `Actions` is dropped.
    pub fn spawn(connect: Connect, updates: UnboundedSender<Update>) -> Actions {
        Self::spawn_bounded(connect, updates, BOUNDS)
    }

    pub(crate) fn spawn_bounded(
        connect: Connect,
        updates: UnboundedSender<Update>,
        bounds: Bounds,
    ) -> Actions {
        let (details, asked) = channel();
        let (to, updates_to) = (connect.clone(), updates.clone());
        std::thread::Builder::new()
            .name("flottille-details".into())
            .spawn(move || read_details(to, asked, updates_to, bounds))
            .expect("a thread for the project page's details");
        Actions {
            details,
            connect,
            updates,
            bounds,
        }
    }

    pub fn send(&self, command: Command) {
        if let Command::Details(project) = command {
            let _ = self.details.send(project);
            return;
        }
        let (connect, updates, bounds) = (self.connect.clone(), self.updates.clone(), self.bounds);
        std::thread::Builder::new()
            .name("flottille-action".into())
            .spawn(move || {
                let outcome = perform(&connect, command, bounds);
                let _ = updates.unbounded_send(Update::Done(outcome));
            })
            .expect("a thread for a window's action");
    }
}

fn read_details(
    connect: Connect,
    asked: Receiver<String>,
    updates: UnboundedSender<Update>,
    bounds: Bounds,
) {
    let mut conn: Option<Conn> = None;
    let mut tickets = Tickets::new();
    while let Ok(first) = asked.recv() {
        let began = Instant::now();
        // Details asked again before they were read are read once.
        let mut projects = vec![first];
        for project in asked.try_iter() {
            if !projects.contains(&project) {
                projects.push(project);
            }
        }
        for project in projects {
            if let Some(details) =
                details(&connect, &mut conn, &mut tickets, &project, bounds.quick)
                && updates
                    .unbounded_send(Update::Done(Outcome::Details(details)))
                    .is_err()
            {
                return;
            }
        }
        std::thread::sleep(DETAILS_EVERY.saturating_sub(began.elapsed()));
    }
}

/// The daemon's answer on a kept connection, made again when it broke or went silent.
fn ask(
    connect: &Connect,
    conn: &mut Option<Conn>,
    method: &str,
    params: Value,
    bound: Duration,
) -> Result<Result<Value, String>, String> {
    if conn.is_none() {
        *conn = Some(connected(connect)?);
    }
    let answer = conn
        .as_mut()
        .expect("connected")
        .ask_within(method, params, bound);
    if answer.is_err() {
        *conn = None;
    }
    answer
}

fn connected(connect: &Connect) -> Result<Conn, String> {
    let stream = connect().map_err(|e| format!("cannot reach the daemon: {e}"))?;
    Conn::new(stream).map_err(|e| e.to_string())
}

/// One request on a connection of its own.
fn ask_alone(
    connect: &Connect,
    method: &str,
    params: Value,
    bound: Duration,
) -> Result<Result<Value, String>, String> {
    connected(connect)?.ask_within(method, params, bound)
}

fn perform(connect: &Connect, command: Command, bounds: Bounds) -> Outcome {
    match command {
        Command::Details(_) => unreachable!("the details have their own thread"),
        Command::Start {
            project,
            target,
            pick,
            message,
        } => {
            let workspace = match target {
                Target::Main => None,
                Target::Workspace(name) => Some(name),
            };
            match ask_alone(
                connect,
                "agent.start",
                start_params(&project, workspace.as_deref(), &pick, &message),
                bounds.quick,
            ) {
                Ok(Ok(started)) => started_outcome(&project, &started),
                Ok(Err(why)) | Err(why) => Outcome::NotStarted { project, why },
            }
        }
        Command::SetProvider { name, enabled } => {
            let method = if enabled {
                "provider.enable"
            } else {
                "provider.disable"
            };
            let refused = match ask_alone(connect, method, json!({"name": name}), bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::ProviderSet { name, refused }
        }
        Command::SetJudge { provider, model } => {
            let refused = match ask_alone(
                connect,
                "provider.set_judge",
                json!({"provider": provider, "model": model}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::JudgeSet { refused }
        }
        Command::JudgeNoticed => {
            // Shown or not, the notice goes once it was seen: the providers say it.
            let _ = ask_alone(connect, "provider.judge_noticed", json!({}), bounds.quick);
            let read = crate::providers::next_read();
            Outcome::Providers(read, read_providers(connect, bounds.quick))
        }
        Command::JudgeRead => {
            let shown = ask_alone(connect, "judge.show", json!({}), bounds.quick);
            Outcome::JudgeGuidance {
                shown: shown.and_then(|shown| shown).map(|shown| Shown::of(&shown)),
                written: false,
            }
        }
        Command::JudgeWrite { text } => {
            let shown = match &text {
                Some(text) => ask_alone(connect, "judge.set", json!({"text": text}), bounds.quick),
                None => ask_alone(connect, "judge.reset", json!({}), bounds.quick),
            };
            Outcome::JudgeGuidance {
                shown: shown.and_then(|shown| shown).map(|shown| Shown::of(&shown)),
                written: true,
            }
        }
        Command::DeveloperRead | Command::DeveloperSet { .. } => {
            let (method, params, written) = match command {
                Command::DeveloperSet { log } => ("developer.set", json!({"log": log}), true),
                _ => ("developer.show", json!({}), false),
            };
            let shown = answer(ask_alone(connect, method, params, bounds.quick)).map(|shown| {
                crate::settings::Developer {
                    log: shown["log"] == true,
                    file: shown["file"].as_str().unwrap_or_default().to_owned(),
                }
            });
            Outcome::Developer { written, shown }
        }
        Command::ReadProviders => {
            let read = crate::providers::next_read();
            Outcome::Providers(read, read_providers(connect, bounds.quick))
        }
        Command::CreateWorkspace {
            project,
            name,
            ticket,
            repositories,
            branch,
            bases,
        } => workspace_outcome(
            ask_alone(
                connect,
                "workspace.create",
                json!({"project": project, "name": name, "ticket": ticket,
                    "repositories": repositories, "branch": branch, "bases": bases}),
                bounds.slow,
            ),
            project,
            name,
        ),
        Command::ReadProjectSettings(project) => Outcome::ProjectSettings {
            read: answer(ask_alone(
                connect,
                "project.settings",
                json!({"name": project}),
                bounds.quick,
            )),
            project,
            saved: false,
        },
        Command::SaveProjectSettings { project, settings } => Outcome::ProjectSettings {
            read: answer(ask_alone(
                connect,
                "project.set_settings",
                settings,
                bounds.quick,
            )),
            project,
            saved: true,
        },
        Command::Repositories {
            project,
            method,
            params,
            serial,
        } => Outcome::Repositories {
            read: answer(ask_alone(connect, method, params, bounds.quick)),
            project,
            serial,
        },
        Command::CheckProject(project) => Outcome::ProjectChecked {
            read: answer(ask_alone(
                connect,
                "project.check",
                json!({"name": project}),
                bounds.quick,
            )),
            project,
        },
        Command::RemoveProject(project) => Outcome::ProjectRemoved {
            refused: answer(ask_alone(
                connect,
                "project.remove",
                json!({"name": project}),
                bounds.quick,
            ))
            .err(),
            project,
        },
        Command::DetectProject { folder, name } => Outcome::ProjectDetected {
            read: answer(ask_alone(
                connect,
                "project.detect",
                json!({"folder": folder, "name": name}),
                bounds.quick,
            )),
            folder,
        },
        Command::AddProject {
            folder,
            name,
            repositories,
        } => Outcome::ProjectAdded {
            read: answer(ask_alone(
                connect,
                "project.add",
                json!({"folder": folder, "name": name, "repositories": repositories}),
                bounds.quick,
            )),
            folder,
        },
        Command::PreviewTicket { project, ticket } => Outcome::TicketPreview {
            read: answer(ask_alone(
                connect,
                "project.ticket",
                json!({"project": project, "ticket": ticket}),
                bounds.quick,
            )),
            project,
            ticket,
        },
        Command::LinkTicket {
            project,
            name,
            ticket,
        } => match ask_alone(
            connect,
            "workspace.link_ticket",
            json!({"project": project, "name": name, "ticket": ticket}),
            bounds.slow,
        ) {
            Ok(Ok(_)) => Outcome::TicketLinked {
                project,
                name,
                refused: None,
            },
            Ok(Err(why)) | Err(why) => Outcome::TicketLinked {
                project,
                name,
                refused: Some(why),
            },
        },
        Command::PrepareWorkspace { project, name } => workspace_outcome(
            ask_alone(
                connect,
                "workspace.prepare",
                json!({"project": project, "name": name}),
                bounds.slow,
            ),
            project,
            name,
        ),
        Command::Commands { project } => {
            let listed = ask_alone(
                connect,
                "command.list",
                json!({"project": project}),
                bounds.quick,
            );
            Outcome::Commands {
                listed: listed.and_then(|listed| listed).map(|listed| {
                    listed["commands"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(CommandInfo::of)
                        .collect()
                }),
                project,
            }
        }
        Command::CommandRead { project, name } => {
            let read = ask_alone(
                connect,
                "command.show",
                json!({"project": project, "name": name}),
                bounds.quick,
            );
            Outcome::CommandRead {
                read: read
                    .and_then(|read| read)
                    .map(|read| CommandInfo::of(&read)),
                project,
                name,
            }
        }
        Command::CommandWrite { project, write } => {
            let (method, params) = command_write(&project, write);
            let refused = match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::CommandWritten { project, refused }
        }
        Command::RulesRead { project } => {
            let shown = ask_alone(
                connect,
                "rules.show",
                json!({"project": project}),
                bounds.quick,
            );
            Outcome::Rules {
                shown: shown.and_then(|shown| shown).map(|shown| Shown::of(&shown)),
                written: false,
                project,
            }
        }
        Command::RulesWrite { project, text } => {
            let shown = match &text {
                Some(text) => ask_alone(
                    connect,
                    "rules.set",
                    json!({"project": project, "text": text}),
                    bounds.quick,
                ),
                None => ask_alone(
                    connect,
                    "rules.reset",
                    json!({"project": project}),
                    bounds.quick,
                ),
            };
            Outcome::Rules {
                shown: shown.and_then(|shown| shown).map(|shown| Shown::of(&shown)),
                written: true,
                project,
            }
        }
        Command::Skills { project } => {
            let listed = ask_alone(
                connect,
                "skill.list",
                json!({"project": project}),
                bounds.quick,
            );
            Outcome::Skills {
                listed: listed.and_then(|listed| listed).map(|listed| {
                    listed["skills"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(SkillInfo::of)
                        .collect()
                }),
                project,
            }
        }
        Command::ClaudeFound { project, kind } => {
            let found = ask_alone(
                connect,
                kind.method("detect"),
                json!({"project": project}),
                bounds.quick,
            );
            Outcome::ClaudeFound {
                found: found.and_then(|found| found).map(|found| {
                    found["found"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(crate::claude::Found::of)
                        .collect()
                }),
                project,
                kind,
            }
        }
        Command::ClaudeImport {
            project,
            kind,
            names,
        } => {
            let read = ask_alone(
                connect,
                kind.method("import"),
                json!({"project": project, "names": names}),
                bounds.quick,
            );
            Outcome::ClaudeImported {
                read: read.and_then(|read| read),
                project,
                kind,
            }
        }
        Command::SkillRead { project, name } => {
            let read = ask_alone(
                connect,
                "skill.show",
                json!({"project": project, "name": name}),
                bounds.quick,
            );
            Outcome::SkillRead {
                read: read.and_then(|read| read).map(|read| SkillInfo::of(&read)),
                project,
                name,
            }
        }
        Command::SkillWrite { project, write } => {
            let (method, params) = skill_write(&project, write);
            let refused = match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::SkillWritten { project, refused }
        }
        Command::Tasks { project } => {
            let listed = ask_alone(
                connect,
                "task.list",
                json!({"project": project}),
                bounds.quick,
            );
            Outcome::Tasks {
                listed: listed.and_then(|listed| listed).map(|listed| {
                    listed["tasks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(TaskInfo::of)
                        .collect()
                }),
                project,
            }
        }
        Command::Items { project, asked } => {
            let params = json!({"project": project});
            let read = ask_alone(connect, "items.list", params, bounds.quick)
                .and_then(|listed| listed)
                .and_then(|listed| {
                    let params = json!({"name": project});
                    ask_alone(connect, "project.settings", params, bounds.quick)
                        .and_then(|settings| settings)
                        .map(|settings| crate::items::Read::of(&listed, &settings))
                });
            Outcome::Items {
                project,
                asked,
                read,
            }
        }
        Command::Item {
            project,
            kind,
            key,
            repository,
            asked,
        } => {
            let params = json!({"project": project, "kind": kind, "id": key,
                "repository": repository});
            let shown =
                ask_alone(connect, "items.show", params, bounds.quick).and_then(|shown| shown);
            Outcome::Item {
                project,
                kind,
                key,
                repository,
                asked,
                shown,
            }
        }
        Command::TaskWrite {
            project,
            method,
            params,
        } => {
            let refused = match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::TaskWritten { project, refused }
        }
        Command::RunTask { project, name } => {
            let refused = match ask_alone(
                connect,
                "task.run",
                json!({"project": project, "name": name}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::TaskRun {
                project,
                name,
                refused,
            }
        }
        Command::ProjectRuns { project, output } => {
            let listed = ask_alone(
                connect,
                "run.list",
                json!({"project": project, "user": true, "ended": true}),
                bounds.quick,
            );
            let output = output.and_then(|watch| {
                let key = watch.key();
                let (method, params) = match &watch {
                    Watch::Run(run) => ("run.output", json!({"run": run})),
                    Watch::Step {
                        workspace,
                        position,
                    } => (
                        "workspace.step_output",
                        json!({"project": project, "name": workspace, "position": position}),
                    ),
                };
                match ask_alone(connect, method, params, bounds.quick) {
                    Ok(Ok(output)) => Some(Box::new(match watch {
                        Watch::Run(_) => CommandOutput::of(&output),
                        Watch::Step { .. } => CommandOutput::of_step(&output, &key),
                    })),
                    Ok(Err(_)) | Err(_) => None,
                }
            });
            Outcome::ProjectRuns {
                listed: listed.and_then(|listed| listed).map(|listed| {
                    listed["commands"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(CommandRow::of)
                        .collect()
                }),
                project,
                output,
            }
        }
        Command::ProjectRun {
            project,
            run,
            dismiss,
        } => {
            let method = if dismiss { "run.dismiss" } else { "run.stop" };
            let refused = match ask_alone(connect, method, json!({"run": run}), bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::ProjectRunAnswered { project, refused }
        }
        Command::Setup { project } => {
            let listed = ask_alone(
                connect,
                "setup.list",
                json!({"project": project}),
                bounds.quick,
            );
            Outcome::Setup {
                listed: listed.and_then(|listed| listed).map(|listed| {
                    listed["steps"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(crate::setup::step_of)
                        .collect()
                }),
                project,
            }
        }
        Command::SetupWrite {
            project,
            method,
            params,
        } => {
            let refused = match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::SetupWritten { project, refused }
        }
        Command::Image { session, name } => {
            let image = match ask_alone(
                connect,
                "agent.image",
                json!({"agent": session, "name": name}),
                bounds.quick,
            ) {
                Ok(Ok(found)) => Picture::of(&found),
                Ok(Err(_)) | Err(_) => None,
            };
            Outcome::ImageRead {
                session,
                name,
                image,
            }
        }
        Command::Reply {
            session,
            message,
            images,
        } => match ask_alone(
            connect,
            "agent.send",
            json!({"agent": session, "message": message,
                "images": images.iter().map(Picture::to_json).collect::<Vec<_>>()}),
            bounds.quick,
        ) {
            Ok(Ok(_)) => Outcome::Replied {
                session,
                refused: None,
            },
            Ok(Err(why)) | Err(why) => Outcome::Replied {
                session,
                refused: Some(why),
            },
        },
        Command::Unqueue {
            session,
            delivery,
            edit,
        } => {
            let taken = match ask_alone(
                connect,
                "agent.unqueue",
                json!({"agent": session, "delivery": delivery, "images": edit}),
                bounds.quick,
            ) {
                Ok(Ok(found)) => Ok((
                    found["text"].as_str().unwrap_or_default().to_owned(),
                    found["images"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Picture::of)
                        .collect(),
                )),
                Ok(Err(why)) | Err(why) => Err(why),
            };
            Outcome::Unqueued {
                session,
                delivery,
                edit,
                taken,
            }
        }
        Command::SendNow { session, delivery } => {
            let refused = match ask_alone(
                connect,
                "agent.send_now",
                json!({"agent": session, "delivery": delivery}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::SentNow {
                session,
                delivery,
                refused,
            }
        }
        Command::Stop { session } => {
            let refused = match ask_alone(
                connect,
                "agent.stop",
                json!({"agent": session}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::Stopped { session, refused }
        }
        Command::Resume { session } => {
            let refused = match ask_alone(
                connect,
                "agent.resume",
                json!({"agent": session}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::Resumed { session, refused }
        }
        Command::SetMode { session, mode } => {
            let refused = match ask_alone(
                connect,
                "agent.mode",
                json!({"agent": session, "mode": mode}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::ModeSet { session, refused }
        }
        Command::Interrupt { session } => {
            let refused = match ask_alone(
                connect,
                "agent.cancel",
                json!({"agent": session}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::Interrupted { session, refused }
        }
        Command::Run {
            session,
            run,
            dismiss,
        } => {
            let method = if dismiss { "run.dismiss" } else { "run.stop" };
            let refused = match ask_alone(connect, method, json!({"run": run}), bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::RunAnswered { session, refused }
        }
        Command::CancelSubscription {
            session,
            subscription,
        } => {
            let refused = match ask_alone(
                connect,
                "subscription.cancel",
                json!({"subscription": subscription}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::SubscriptionCancelled { session, refused }
        }
        Command::Answer {
            session,
            number,
            answer,
        } => {
            let (method, params) = match answer {
                Answer::Deny => ("approval.deny", json!({"number": number})),
                Answer::Once => ("approval.approve", json!({"number": number})),
            };
            match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(answered)) => Outcome::Answered {
                    session,
                    number,
                    decision: answered["request"]["decision"].as_str().map(str::to_owned),
                    refused: None,
                },
                Ok(Err(why)) | Err(why) => Outcome::Answered {
                    session,
                    number,
                    decision: None,
                    refused: Some(why),
                },
            }
        }
        Command::RemoveWorkspace { project, name } => workspace_outcome(
            ask_alone(
                connect,
                "workspace.remove",
                json!({"project": project, "name": name}),
                bounds.slow,
            ),
            project,
            name,
        ),
    }
}

fn command_write(project: &str, write: CommandWrite) -> (&'static str, Value) {
    match write {
        CommandWrite::Template { template } => (
            "command.add",
            json!({"project": project, "template": template}),
        ),
        CommandWrite::Add {
            name,
            description,
            instructions,
        } => (
            "command.add",
            json!({"project": project, "name": name, "description": description,
                "instructions": instructions}),
        ),
        CommandWrite::Edit {
            name,
            description,
            instructions,
        } => (
            "command.edit",
            json!({"project": project, "name": name, "description": description,
                "instructions": instructions}),
        ),
        CommandWrite::Remove { name } => {
            ("command.remove", json!({"project": project, "name": name}))
        }
    }
}

fn skill_write(project: &str, write: SkillWrite) -> (&'static str, Value) {
    let written =
        |name: String, description: String, path: Option<String>, text: Option<String>| {
            let mut params = json!({"project": project, "name": name, "description": description});
            if let Some(path) = path {
                params["path"] = path.into();
            }
            if let Some(text) = text {
                params["text"] = text.into();
            }
            params
        };
    match write {
        SkillWrite::Add {
            name,
            description,
            path,
            text,
        } => ("skill.add", written(name, description, path, text)),
        SkillWrite::Edit {
            name,
            description,
            path,
            text,
        } => ("skill.edit", written(name, description, path, text)),
        SkillWrite::Remove { name } => ("skill.remove", json!({"project": project, "name": name})),
    }
}

/// The providers on a connection of their own, or the daemon's sentence when it cannot list them.
/// `None` when the daemon did not answer: that changes nothing, and the link reads them again once
/// it is back.
pub(crate) fn read_providers(
    connect: &Connect,
    bound: Duration,
) -> Option<crate::providers::Providers> {
    match ask_alone(connect, "provider.list", json!({}), bound) {
        Ok(listed) => Some(crate::providers::Providers::of(
            listed.as_ref().map_err(Clone::clone),
        )),
        Err(_) => None,
    }
}

/// The daemon's answer, or its sentence (or the window's) when there is none.
fn answer(asked: Result<Result<Value, String>, String>) -> Result<Value, String> {
    match asked {
        Ok(answer) => answer,
        Err(why) => Err(why),
    }
}

fn workspace_outcome(
    answer: Result<Result<Value, String>, String>,
    project: String,
    name: String,
) -> Outcome {
    match answer {
        Ok(Ok(_)) => Outcome::WorkspaceDone { project, name },
        Ok(Err(why)) | Err(why) => Outcome::WorkspaceRefused { project, name, why },
    }
}

fn start_params(project: &str, workspace: Option<&str>, pick: &Pick, message: &str) -> Value {
    // What is left out is sent as a null, which the daemon reads as absent: its to choose.
    json!({
        "project": project,
        "workspace": workspace,
        "provider": pick.provider,
        "model": pick.model,
        "effort": pick.effort,
        "message": message,
    })
}

fn started_outcome(project: &str, started: &Value) -> Outcome {
    match started["agent"]["id"].as_str() {
        Some(agent) => Outcome::Started {
            project: project.to_owned(),
            agent: agent.to_owned(),
        },
        None => Outcome::NotStarted {
            project: project.to_owned(),
            why: "the daemon did not say which agent it started".into(),
        },
    }
}

/// Reads the branches of the project's repositories and the lines changed in its workspaces.
/// `None` when the daemon cannot be asked: the page keeps what it showed.
/// The tickets read, by project and workspace, with when: a tracker is asked again only once a
/// read is [`TICKET_FRESH`] no more, or the workspace is linked to another ticket.
type Tickets = HashMap<(String, String), (TicketFacts, Instant)>;

fn details(
    connect: &Connect,
    conn: &mut Option<Conn>,
    kept: &mut Tickets,
    project: &str,
    bound: Duration,
) -> Option<Details> {
    let shown = ask(
        connect,
        conn,
        "project.show",
        json!({"name": project}),
        bound,
    )
    .ok()?
    .ok()?;
    let branches = shown["repositories"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|repository| {
            (
                repository["path"].as_str().unwrap_or_default().to_owned(),
                repository["branch"].as_str().map(str::to_owned),
            )
        })
        .collect();
    let listed = ask(
        connect,
        conn,
        "workspace.list",
        json!({"project": project}),
        bound,
    )
    .ok()?
    .ok()?;
    let mut changes = HashMap::new();
    let mut pulls = HashMap::new();
    let mut statuses = HashMap::new();
    let mut tickets = HashMap::new();
    for workspace in listed["workspaces"].as_array().into_iter().flatten() {
        let name = workspace["name"].as_str().unwrap_or_default();
        let Ok(Ok(shown)) = ask(
            connect,
            conn,
            "workspace.show",
            json!({"project": project, "name": name}),
            bound,
        ) else {
            continue;
        };
        let mut total = Changes::default();
        for repository in shown["repositories"].as_array().into_iter().flatten() {
            let changes = &repository["changes"];
            total.added += changes["added"].as_u64().unwrap_or(0);
            total.removed += changes["removed"].as_u64().unwrap_or(0);
            total.capped |= changes["capped"] == true;
        }
        changes.insert(name.to_owned(), total);
        statuses.insert(name.to_owned(), RepositoryStatus::of_workspace(&shown));
        let key = (project.to_owned(), name.to_owned());
        match shown["ticket"].as_str() {
            None => {
                kept.remove(&key);
            }
            Some(linked) => {
                let stale = kept.get(&key).is_none_or(|(facts, at)| {
                    facts.reference != linked || at.elapsed() > TICKET_FRESH
                });
                if stale {
                    let read = ask(
                        connect,
                        conn,
                        "workspace.ticket",
                        json!({"project": project, "name": name}),
                        bound,
                    );
                    let facts = match read {
                        Ok(Ok(read)) => ticket_facts(&read),
                        _ => None,
                    }
                    .unwrap_or_else(|| TicketFacts {
                        reference: linked.to_owned(),
                        state: None,
                        title: None,
                        url: None,
                        problem: Some("the ticket could not be read".into()),
                    });
                    kept.insert(key.clone(), (facts, Instant::now()));
                }
                if let Some((facts, _)) = kept.get(&key) {
                    tickets.insert(name.to_owned(), facts.clone());
                }
            }
        }
        if let Ok(Ok(forge)) = ask(
            connect,
            conn,
            "workspace.forge",
            json!({"project": project, "name": name}),
            bound,
        ) {
            pulls.insert(name.to_owned(), pull_requests(&forge).0);
        }
    }
    Some(Details {
        project: project.to_owned(),
        branches,
        changes,
        pulls,
        statuses,
        tickets,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    use futures::channel::mpsc::{UnboundedReceiver, unbounded};

    use super::*;

    /// A daemon on socket pairs that answers `agent.start` at once and never answers what is in
    /// `silent`.
    fn daemon(silent: &'static [&'static str]) -> Connect {
        Arc::new(move || {
            let (window, daemon) = UnixStream::pair()?;
            std::thread::spawn(move || {
                let mut writer = daemon.try_clone().unwrap();
                for line in BufReader::new(daemon).lines() {
                    let Ok(line) = line else { return };
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let method = request["method"].as_str().unwrap();
                    if silent.contains(&method) {
                        continue;
                    }
                    let result = match method {
                        "agent.start" => json!({"agent": {"id": "made-up-agent"}}),
                        _ => json!({}),
                    };
                    let answer = json!({"id": request["id"], "result": result});
                    writeln!(writer, "{answer}").unwrap();
                }
            });
            Ok(window)
        })
    }

    fn next(updates: &mut UnboundedReceiver<Update>, within: Duration) -> Option<Outcome> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Ok(Update::Done(outcome)) = updates.try_recv() {
                return Some(outcome);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn start_in_main() -> Command {
        Command::Start {
            project: "atlas".into(),
            target: Target::Main,
            pick: Pick::default(),
            message: "Fix the timeout".into(),
        }
    }

    #[test]
    fn a_session_started_while_a_workspace_is_removed_does_not_wait_for_it() {
        let (sender, mut updates) = unbounded();
        let actions = Actions::spawn(daemon(&["workspace.remove"]), sender);
        actions.send(Command::RemoveWorkspace {
            project: "atlas".into(),
            name: "login".into(),
        });
        actions.send(start_in_main());

        assert_eq!(
            next(&mut updates, Duration::from_secs(5)),
            Some(Outcome::Started {
                project: "atlas".into(),
                agent: "made-up-agent".into()
            })
        );
    }

    #[test]
    fn a_daemon_that_does_not_answer_is_said_after_a_while() {
        let (sender, mut updates) = unbounded();
        let bounds = Bounds {
            quick: Duration::from_millis(200),
            ..BOUNDS
        };
        let actions = Actions::spawn_bounded(daemon(&["agent.start"]), sender, bounds);
        actions.send(start_in_main());

        let Some(Outcome::NotStarted { project, why }) = next(&mut updates, Duration::from_secs(5))
        else {
            panic!("no answer of the window's own");
        };
        assert_eq!(project, "atlas");
        assert_eq!(why, "the daemon did not answer agent.start within 200 ms");
    }
}
