//! What the window asks of the daemon, off the UI thread: reading a project page's details,
//! creating, preparing and removing a workspace, starting a session. Each answer comes back to the
//! UI as an [`Update::Done`]; nothing here waits on the UI, and the UI never waits on it.
//!
//! The details are read on a thread and a connection of their own. Every other request has its
//! own thread and connection, so a workspace removed (git, up to half an hour) holds neither a
//! session started meanwhile nor the details. Every answer is waited for a bounded time: past it
//! the daemon is taken as silent, the request answers so (a session not started, a workspace
//! refused, the details not changed) and its connection is dropped.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::arrange::{self, Arrangement, Change};
use crate::commands::CommandInfo;
use crate::kept::{Draft, DraftKey, Written};
use crate::link::{Conn, Connect, Sign, Update, Updates};
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

/// A large pasted text and its name, as the daemon takes it: it writes it as a file the message
/// names (#848).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pasted {
    pub name: String,
    pub text: String,
}

impl Pasted {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "text": self.text})
    }
}

/// Where the user ran a task from: the answer goes back there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunFrom {
    /// A place's ▶ on the project page, in that place (`None`: `main`).
    Page { workspace: Option<String> },
    /// Run in a task's "⋯" of the project's settings, in the project folder.
    Settings,
    /// A session's rail, in the session's workspace (`None`: `main`).
    Session { workspace: Option<String> },
}

/// What the user does to a command, as the daemon's `run.*` methods have it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunAct {
    /// Stops it and everything it started.
    Stop,
    /// Stops it if it runs, then runs its line again in the same place.
    Restart,
    /// Takes one that ended off the list.
    Dismiss,
}

impl RunAct {
    fn method(self) -> &'static str {
        match self {
            RunAct::Stop => "run.stop",
            RunAct::Restart => "run.restart",
            RunAct::Dismiss => "run.dismiss",
        }
    }
}

/// What the daemon answered to what the user did to a command.
#[derive(Clone, Debug, PartialEq)]
pub struct RunAnswer {
    pub act: RunAct,
    /// The command it was done to.
    pub run: String,
    /// Why the daemon refused, or why no answer came.
    pub refused: Option<String>,
    /// The new run, when a restart started one.
    pub restarted: Option<String>,
    /// What the daemon says of the restart: the command of an agent that ended is the user's now.
    pub notice: Option<String>,
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
        /// The pictures that go with the first message.
        images: Vec<Picture>,
        /// The large pasted texts that go with it, as files.
        texts: Vec<Pasted>,
        /// The mode the session starts in: `ask`, `auto` or `full`.
        mode: String,
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
    /// Turns the Auto judge on: the daemon picks one as for a user who never chose
    /// (`provider.set_judge {pick}`).
    PickJudge,
    /// Chooses what a new session starts on in `providers.toml`, or none
    /// (`provider.set_new_session`).
    SetNewSession {
        provider: Option<String>,
        model: Option<String>,
        effort: Option<String>,
    },
    /// Reads the providers again: `providers.toml` is the user's to edit by hand too.
    ReadProviders,
    /// What the user says of themselves, which every agent reads (`about.show`).
    AboutRead,
    /// Saves what the user says of themselves (name, how to refer to them, notes), or with no
    /// text forgets it (`about.set`, `about.reset`).
    AboutWrite {
        text: Option<(String, String, String)>,
    },
    /// The developer mode (`developer.show`).
    DeveloperRead,
    /// Switches the developer mode (`developer.set`).
    DeveloperSet {
        log: bool,
    },
    /// The findings agents filed about Flottille itself (`developer.reports`).
    FindingsRead,
    /// Writes them to a file (`developer.reports_export`).
    FindingsExport,
    /// A request of the update's (`update.show`, `.set`, `.check`, `.download`, `.restart`).
    Update {
        method: &'static str,
        params: Value,
    },
    /// A request of a part of Flottille the window draws in its places (`extensions`): what to
    /// do with the answer is said by `then`.
    Ask {
        method: String,
        params: Value,
        then: crate::extensions::Then,
    },
    /// A request only the user's device may ask, signed with its key for the machine `machine`
    /// (this one's own name when `None`): what to do with the answer is said by `then`.
    Signed {
        machine: Option<String>,
        method: String,
        params: Value,
        then: crate::extensions::Then,
    },
    /// The sign-in of the connection `name` of the machine `machine`: begun there, its redirect
    /// carried by a tunnel to this machine's `127.0.0.1` for as long as it waits
    /// (`docs/remote.md`, "Connections"). Answered twice: the address to open, then how it
    /// ended.
    SignInOn {
        name: String,
        machine: String,
    },
    /// A tunnel to a run of another machine, signed by this device and held by a connection of
    /// its own for as long as the tunnel lives (`docs/remote.md`, "Tunnels"). Answered once it is
    /// open or refused, and again when it ends.
    TunnelOpen(crate::tunnels::Ask),
    /// Closes a tunnel this window holds (`tunnel.close`); the connection that holds it says it
    /// ended.
    TunnelClose {
        tunnel: String,
    },
    /// The limit above which a tool result is cut (`tool_results.show`).
    ToolResultsRead,
    /// Chooses it (`tool_results.set`): `None` is Off.
    ToolResultsSet {
        limit: Option<usize>,
    },
    /// Chooses whether `fs_read` says a file is unchanged (`tool_results.set`).
    UnchangedSet {
        unchanged: bool,
    },
    /// After how many days an inactive session is ended (`sessions.show`).
    SessionsRead,
    /// Chooses it (`sessions.set`): `None` is off.
    SessionsSet {
        days: Option<u64>,
    },
    /// Chooses whether a session is resumed at its quota's reset (`sessions.set`).
    ResumeSet {
        resume: bool,
    },
    /// The window's appearance (`appearance.show`); `seq` numbers the request (see
    /// `Shell::ask_appearance`).
    AppearanceRead {
        seq: u64,
    },
    /// Keeps the window's appearance (`appearance.set`).
    AppearanceSet {
        appearance: gpui_design_system::Appearance,
        seq: u64,
    },
    /// What the daemon keeps of a composer's draft (`draft.get`).
    DraftRead {
        key: DraftKey,
    },
    /// Keeps a composer's draft (`draft.set`; an empty one forgets it): the pictures only when
    /// they are not the ones the daemon holds. `after_read`: the window is closing before the
    /// daemon answered the read of the draft, the keeper reads it first and writes only if it
    /// could.
    DraftWrite {
        key: DraftKey,
        text: String,
        pictures: Option<Vec<Picture>>,
        after_read: bool,
    },
    /// Keeps one value of the window's own state (`window.set`).
    WindowSet {
        key: String,
        value: Value,
    },
    /// Keeps a change of the user's arrangement of the projects: the daemon's arrangement is
    /// read, the change applied to it and the whole written (`window.get`, `window.set`), so
    /// that what another window arranged meanwhile stays.
    ArrangementChange {
        change: Change,
    },
    /// The user saw the notice of the Auto judge Flottille picked (`provider.judge_noticed`);
    /// the providers are read again.
    JudgeNoticed,
    /// The Auto judge's guidance (`judge.show`).
    JudgeRead,
    /// The compaction's settings and guidance (`compaction.show`).
    CompactionRead,
    /// Changes the compaction's settings (`compaction.set`): the fields given only.
    CompactionSet {
        params: Value,
    },
    /// Saves the compactor's guidance, or with no text resets it to the default
    /// (`compaction.set_guidance`, `compaction.reset_guidance`).
    CompactionGuidance {
        text: Option<String>,
    },
    /// The user saw the notice of the compactor Flottille picked (`compaction.noticed`).
    CompactionNoticed,
    /// Saves the judge's guidance, or with no text resets it to the default (`judge.set`,
    /// `judge.reset`).
    JudgeWrite {
        text: Option<String>,
    },
    /// The guidance of a part of Flottille the user can turn off (`<name>.show`), or one of its
    /// parts (a check's instructions).
    GuidanceRead {
        name: String,
        part: Option<String>,
    },
    /// Saves it (`<name>.set`), or with none resets it (`<name>.reset`).
    GuidanceWrite {
        name: String,
        part: Option<String>,
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
    /// The command lines the project's forges and tracker need, checked (`project.programs`).
    Programs {
        project: String,
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
    /// Writes one of the project's conventions, `key` of `project.set_conventions`, empty for
    /// none.
    SaveConvention {
        project: String,
        key: &'static str,
        value: String,
    },
    /// Reads each repository's commit checks and what its hooks propose (`project.commit_checks`).
    ReadCommitChecks(String),
    /// Writes one repository's commit checks (`project.set_commit_checks`).
    SaveCommitChecks {
        project: String,
        repository: String,
        pre_commit: Vec<String>,
        commit_msg: Vec<String>,
    },
    /// Writes the project's message checks (`project.set_message_checks`).
    SaveMessageChecks {
        project: String,
        checks: Vec<String>,
    },
    /// Detects the project's repositories again, against its file.
    CheckProject(String),
    /// The led sessions the end of `session` would end (`agent.ending`), for its confirmation.
    ReadEnding {
        session: String,
    },
    /// What removing the project would do (`project.removal`): read `serial` of its dialog.
    ReadRemoval {
        project: String,
        serial: u64,
    },
    /// Removes the project: stops what runs in it, removes its workspaces (with `discard`, the
    /// workspaces as the dialog showed them, those holding unsaved work too as long as it is what
    /// was shown) and forgets it, never its folder.
    RemoveProject {
        project: String,
        discard: Option<Vec<flottille_protocol::ShownWorkspace>>,
    },
    /// What registering a folder would register.
    /// The folders of the machine shown a project may be added from (`project.folders`).
    ProjectFolders {
        /// The machine shown when it was asked: an answer for another one is dropped.
        machine: Option<String>,
        /// The folder whose folders are listed; the home when none.
        path: Option<String>,
    },
    DetectProject {
        folder: String,
        name: Option<String>,
        /// The form's generation: the answer carries it back.
        seq: u64,
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
    /// The programs and whether each is signed in (`signin.list`, #1185).
    SignIns,
    /// A program's own sign-in, started by its daemon (`signin.start`).
    SignInStart(String),
    /// The code the user pasted for that program's sign-in (`signin.code`), sent once.
    SignInCode(String, PastedCode),
    /// A program's sign-in stopped (`signin.cancel`).
    SignInCancel(String),
    /// A program's own sign-out, confirmed (`signin.sign_out`).
    SignOut(String),
    /// The vault's entries, their metadata (`vault.list`).
    Vault,
    /// An entry set or removed (`vault.set`, `vault.remove`).
    VaultWrite(VaultWrite),
    /// The machines an entry may be shared with: the user's others that are online.
    ShareMachines,
    /// The entry `name` shared with the machine `machine` (`vault.share`), signed by this device.
    VaultShare {
        name: String,
        machine: String,
    },
    /// The card of the request `ask` for a vault entry dismissed.
    VaultDismiss {
        ask: u64,
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
    /// The project's milestones and their tickets (`project.progress`); `asked` numbers the
    /// reads.
    Progress {
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
    /// Runs a task for the user, in the place `from` says.
    RunTask {
        project: String,
        name: String,
        from: RunFrom,
    },
    /// The task runs of the project, the user's and its agents', and the output of the one whose
    /// panel is open.
    ProjectRuns {
        project: String,
        output: Option<Watch>,
    },
    /// Stops a task the user ran and everything it started, runs it again, or dismisses one that
    /// ended.
    ProjectRun {
        project: String,
        run: String,
        act: RunAct,
    },
    /// The services of the machine (`service.list`).
    Services,
    /// A change to the services: `service.add`, `service.edit` or `service.remove`, with its
    /// parameters.
    ServiceWrite {
        method: &'static str,
        params: Value,
    },
    /// Starts a service (and what it needs first), or stops it; `force` stops it even though
    /// something running needs it, which the user confirmed.
    ServiceAct {
        name: String,
        start: bool,
        force: bool,
    },
    /// The output of a service's run, as a run's.
    ServiceOutput {
        run: String,
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
    /// Makes a project's agents get its rules alone, or the global ones too (`rules.set`).
    RulesAlone {
        project: String,
        alone: bool,
    },
    /// Where workspaces go: the default for every project, or a project's own
    /// (`workspaces_folder.show`).
    WorkspacesFolderRead {
        project: Option<String>,
    },
    /// Keeps it (`workspaces_folder.set`): the default's `place` and `path`, or a project's own
    /// folder in `path` (none follows the default).
    WorkspacesFolderSet {
        project: Option<String>,
        place: Option<String>,
        path: Option<String>,
    },
    /// Where this machine makes the projects of other machines' launches
    /// (`travelled_folder.show`).
    TravelledFolderRead,
    /// Keeps it (`travelled_folder.set`): the user's folder, or none for the data folder.
    TravelledFolderSet {
        folder: Option<String>,
    },
    /// A message to a session's root agent: the only agent the window writes to.
    Reply {
        session: String,
        /// The message being sent, as the composer keeps it until the daemon answers.
        key: u64,
        message: String,
        /// Pasted or dropped pictures that go with it, as the clipboard gave them.
        images: Vec<Picture>,
        /// The large pasted texts that go with it, as files.
        texts: Vec<Pasted>,
        /// The composer's Compact before sending, while it shows: the agent's cache expired.
        compact: Option<bool>,
    },
    /// The user's answers from the card of the ask of `seq` the session's root asked, one per
    /// question (`agent.answer`): it answers that ask alone.
    AnswerQuestion {
        session: String,
        seq: u64,
        answers: Vec<flottille_core::question::Answer>,
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
    /// Names a session: the user's title, which its agent can no longer change.
    Rename {
        session: String,
        title: String,
    },
    /// Detaches a session from the agent that started it, named `from`.
    Detach {
        session: String,
        from: String,
    },
    /// Reloads the tools of the session's root: its process restarts on its conversation, once
    /// the turn that runs ended.
    ReloadTools {
        session: String,
    },
    /// Compacts the conversation of the session's root (`agent.compact`), now or once the turn
    /// that runs ended.
    Compact {
        session: String,
    },
    /// Interrupts the turn the session's root agent runs; the agent stays.
    Interrupt {
        session: String,
    },
    /// Whether the session, waiting for its quota's reset, is resumed at it (`agent.quota_resume`).
    QuotaResume {
        session: String,
        resume: bool,
    },
    /// Puts the session in Ask, Auto or Full (`ask`, `auto`, `full`): the user's alone to change.
    SetMode {
        session: String,
        mode: String,
    },
    /// Changes the session's model and effort between turns (`agent.options`): the daemon holds
    /// the change until the turn ends. The provider never changes.
    SetOptions {
        session: String,
        model: Option<String>,
        effort: Option<String>,
    },
    /// Stops a command of the session and everything it started, runs it again, or dismisses one
    /// that ended.
    Run {
        session: String,
        run: String,
        act: RunAct,
    },
    /// Ends a subscription of the session's agents, as the agent's own `unsubscribe` does.
    CancelSubscription {
        session: String,
        subscription: String,
    },
    /// Takes back the user's leave for `writer` to write to `leave_session`.
    WithdrawLeave {
        session: String,
        writer: String,
        leave_session: String,
    },
    /// Marks notifications of the centre read (`notifications.read`): the window follows the
    /// daemon's `notifications.changed`, not this command's answer.
    ReadNotifications(Reading),
    /// Resumes a paused following of the session's agents, as a message of the user's does.
    ResumeFollowing {
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

/// A change to the vault, as the daemon takes it.
#[derive(Clone, PartialEq)]
pub enum VaultWrite {
    /// A new entry, or one edited: without a secret, the one the keyring holds is kept.
    Set {
        name: String,
        secret: Option<String>,
        description: String,
        url: String,
        username: String,
        /// Every project's when `None`.
        project: Option<String>,
        /// The one use it is made for (`worklog:<project>`); left out, it keeps the one it has.
        bound: Option<String>,
    },
    Remove {
        name: String,
        /// The user confirmed what stops working without it.
        confirmed: bool,
    },
}

/// What a debug line may say of a write: never its secret.
impl std::fmt::Debug for VaultWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultWrite::Set { name, project, .. } => f
                .debug_struct("Set")
                .field("name", name)
                .field("project", project)
                .finish_non_exhaustive(),
            VaultWrite::Remove { name, .. } => {
                f.debug_struct("Remove").field("name", name).finish()
            }
        }
    }
}

/// A code the user pasted for a program's sign-in: a debug line never says it.
#[derive(Clone, PartialEq)]
pub struct PastedCode(String);

impl From<String> for PastedCode {
    fn from(code: String) -> Self {
        PastedCode(code)
    }
}

impl std::fmt::Debug for PastedCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PastedCode(..)")
    }
}

/// The setting of Sessions a choice wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionsSetting {
    Days,
    Resume,
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

/// Which notifications of the centre are marked read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading {
    Ids(Vec<u64>),
    /// Those of a session, by its root.
    Session(String),
    All,
}

impl Reading {
    fn params(&self) -> Value {
        match self {
            Reading::Ids(ids) => json!({"ids": ids}),
            Reading::Session(session) => json!({"session": session}),
            Reading::All => json!({"all": true}),
        }
    }
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
    /// The workspaces whose forge has not answered yet: their pull requests are still to come.
    pub pending_pulls: HashSet<String>,
    /// The workspaces whose ticket is linked and not read yet.
    pub pending_tickets: HashSet<String>,
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
        /// Its title, as the daemon gave it, or else the first line of its brief.
        title: String,
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
    /// One of the project's conventions as saved, empty for none, or why nothing was.
    Convention {
        project: String,
        key: &'static str,
        saved: Result<String, String>,
    },
    /// Each repository's commit checks and proposals, read or after the save of `saved`'s
    /// repository, or why nothing was read or written.
    CommitChecks {
        project: String,
        read: Result<Value, String>,
        saved: Option<String>,
    },
    /// The project's message checks as saved, or why nothing was.
    MessageChecks {
        project: String,
        saved: Result<Vec<String>, String>,
    },
    /// The project's repositories, read, added to or removed from, or why not.
    Repositories {
        project: String,
        read: Result<Value, String>,
        serial: u64,
    },
    /// The programs of the project's forges and tracker, checked, or why not.
    Programs {
        project: String,
        read: Result<Value, String>,
        serial: u64,
    },
    /// The project's repositories detected again (`project.check`), or why not.
    ProjectChecked {
        project: String,
        read: Result<Value, String>,
    },
    /// The led sessions ending `session` would end, or why they cannot be read.
    EndingRead {
        session: String,
        read: Result<Value, String>,
    },
    /// What removing the project would do, or why it cannot be read.
    RemovalRead {
        project: String,
        serial: u64,
        read: Result<Value, String>,
    },
    /// What the removal of the project did, or why the daemon refused it.
    ProjectRemoved {
        project: String,
        result: Result<Value, String>,
    },
    /// The folders of `machine` a project may be added from, as `project.folders` says them, or
    /// why they cannot be read.
    ProjectFolders {
        machine: Option<String>,
        read: Result<Value, String>,
    },
    /// What a folder would register (`project.detect`), or why it cannot be.
    ProjectDetected {
        folder: String,
        /// The form's generation when it asked: an answer to an older one is dropped.
        seq: u64,
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
    /// What a new session starts on was chosen, or the daemon refused with why.
    NewSessionSet {
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
    /// The programs' sign-in states, as `signin.list` answers them, or why they cannot be read.
    SignIns(Result<Value, String>),
    /// What a program's sign-in said it needs, or why it did not start.
    SignInStarted {
        program: String,
        answer: Result<Value, String>,
    },
    /// A code, a cancel or a sign-out (`what`) was done, or refused with why.
    SignInAnswered {
        program: String,
        what: &'static str,
        refused: Option<String>,
    },
    /// The vault's entries, or why they cannot be read.
    Vault {
        listed: Result<Vec<crate::vault::VaultEntry>, String>,
    },
    /// A change to the vault was made, or refused with why.
    VaultWritten {
        refused: Option<String>,
    },
    /// The user's other machines that are online, by name, or why they cannot be read.
    ShareMachines {
        listed: Result<Vec<String>, String>,
    },
    /// The entry `name` was shared with `machine`, or refused with why.
    VaultShared {
        name: String,
        machine: String,
        refused: Option<String>,
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
    /// What the user says of themselves as the daemon says it, after a read (`written` false) or a
    /// save or a reset.
    About {
        written: bool,
        shown: Result<crate::about::Shown, String>,
    },
    /// The developer mode as the daemon says it, after a read (`written` false) or a switch.
    Developer {
        written: bool,
        shown: Result<crate::settings::Developer, String>,
    },
    /// The findings as the daemon lists them.
    Findings(Result<Vec<crate::findings::Finding>, String>),
    /// Where the findings were exported, or why not.
    FindingsExported(Result<String, String>),
    /// The daemon's answer to a [`Command::Update`]: what `update.show` says, or why not.
    Update {
        method: &'static str,
        answer: Result<Value, String>,
    },
    /// What a connection that holds a tunnel says of it.
    Tunnel(crate::tunnels::News),
    /// The daemon's answer to an [`Command::Ask`], or why there is none.
    Asked {
        then: crate::extensions::Then,
        answer: Result<Value, String>,
    },
    /// The limit of tool results as the daemon says it (`None` is Off) and whether a file is said
    /// unchanged, after a read (`written` false) or a choice.
    ToolResults {
        written: bool,
        shown: Result<(Option<usize>, bool), String>,
    },
    /// After how many days an inactive session is ended as the daemon says it (`None` is off),
    /// and whether a session is resumed at its quota's reset, after a read (`written` none) or a
    /// choice of one of them.
    Sessions {
        written: Option<SessionsSetting>,
        shown: Result<(Option<u64>, bool), String>,
    },
    /// What the daemon keeps of a composer's draft.
    Draft {
        key: DraftKey,
        found: Result<Option<Draft>, String>,
    },
    /// How the write of a composer's draft ended.
    DraftWritten {
        key: DraftKey,
        result: Written,
    },
    /// How the write of a value of the window's state ended, with its key and the value.
    WindowKept {
        key: String,
        value: Value,
        result: Written,
    },
    /// How the keeping of a change of the arrangement ended, with the arrangement the daemon
    /// holds then (none when it could not be read).
    ArrangementKept {
        change: Change,
        arrangement: Option<Arrangement>,
        result: Written,
    },
    /// The window's appearance as the daemon keeps it, after a read (`written` false) or a
    /// choice.
    Appearance {
        /// The number of the request this answers.
        seq: u64,
        written: bool,
        shown: Result<gpui_design_system::Appearance, String>,
    },
    /// The compaction as the daemon says it (`compaction.show`), after a read, a change of its
    /// settings (`written`) or of its guidance (`guidance`).
    Compaction {
        written: bool,
        guidance: bool,
        shown: Result<Value, String>,
    },
    /// The judge's guidance as the daemon says it, after a read (`written` false) or a save or
    /// a reset.
    JudgeGuidance {
        written: bool,
        shown: Result<Shown, String>,
    },
    /// The guidance of the part `name` (or of its part `part`) as the daemon says it, after a
    /// read or a save or a reset.
    Guidance {
        name: String,
        part: Option<String>,
        written: bool,
        shown: Result<Shown, String>,
    },
    /// The rules as the daemon says them, after a read (`written` false) or a save or a reset.
    Rules {
        project: Option<String>,
        written: bool,
        shown: Result<Shown, String>,
    },
    /// A project's switch to its own rules alone, or back: the rules as the daemon says them.
    /// What is typed in the editor stays, and is not saved.
    RulesSwitched {
        project: String,
        shown: Result<Shown, String>,
    },
    /// Where workspaces go as the daemon says it, after a read (`written` false) or a choice.
    WorkspacesFolder {
        project: Option<String>,
        written: bool,
        shown: Result<crate::workspaces_folder::Shown, String>,
    },
    /// Where this machine makes the projects of other machines' launches, after a read
    /// (`written` false) or a choice.
    TravelledFolder {
        written: bool,
        shown: Result<crate::travelled_folder::Shown, String>,
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
    Progress {
        project: String,
        asked: u64,
        read: Result<crate::progress::Progress, String>,
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
        from: RunFrom,
        refused: Option<String>,
    },
    /// The tasks the user ran in the project, and the output asked for.
    ProjectRuns {
        project: String,
        listed: Result<Vec<CommandRow>, String>,
        output: Option<Box<CommandOutput>>,
    },
    /// A task the user ran was stopped, restarted (as the run `restarted`) or dismissed, or the
    /// daemon refused with why.
    ProjectRunAnswered {
        project: String,
        answer: RunAnswer,
    },
    Services {
        listed: Result<Vec<crate::services::ServiceInfo>, String>,
    },
    /// A change to the services was made, or refused with why.
    ServiceWritten {
        refused: Option<String>,
    },
    /// A service was started (with what started) or stopped, or the daemon refused with why.
    ServiceActed {
        name: String,
        start: bool,
        answer: Result<Value, String>,
        /// The stop was refused because something running needs the service (the code
        /// `needed`), which started since the list was read: what needs it, as the daemon's
        /// data says it.
        needed: Option<Vec<String>>,
    },
    /// The output of a service's run.
    ServiceOutput {
        output: Option<Box<CommandOutput>>,
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
        key: u64,
        answer: Result<crate::session::Accepted, String>,
    },
    /// The dismissal of a request for a vault entry was taken, or refused with why.
    VaultDismissed {
        ask: u64,
        refused: Option<String>,
    },
    /// The answers from the card of the ask `seq` were taken, or refused with why.
    QuestionAnswered {
        session: String,
        seq: u64,
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
    /// The session was renamed, or the daemon refused with why.
    Renamed {
        refused: Option<String>,
    },
    /// The session was detached from `from`, or the daemon refused with why.
    Detached {
        from: String,
        refused: Option<String>,
    },
    /// The session's tools reload, now or once its turn ends (`after_turn`), or the daemon
    /// refused with why.
    ToolsReloaded {
        session: String,
        after_turn: bool,
        refused: Option<String>,
    },
    /// The session's compaction starts, now or once its turn ends (`after_turn`), or the daemon
    /// refused with why.
    Compacting {
        session: String,
        after_turn: bool,
        refused: Option<String>,
    },
    /// The session's turn was interrupted (or none ran), or the daemon refused with why.
    Interrupted {
        session: String,
        refused: Option<String>,
    },
    /// The session's choice to resume at its quota's reset was taken, or the daemon refused with
    /// why.
    QuotaResumed {
        session: String,
        refused: Option<String>,
    },
    /// The session's mode was changed, or the daemon refused with why.
    ModeSet {
        session: String,
        refused: Option<String>,
    },
    /// The session's model and effort were taken for its next prompt, or the daemon refused with
    /// why.
    OptionsSet {
        session: String,
        refused: Option<String>,
    },
    /// A command was stopped, restarted (as the run `restarted`) or dismissed as asked, or the
    /// daemon refused with why.
    RunAnswered {
        session: String,
        answer: RunAnswer,
    },
    /// Notifications were marked read, or the daemon could not: the centre's own event says
    /// what changed; a failure lets the window ask again.
    NotificationsRead {
        failed: bool,
    },
    /// A subscription was cancelled, or a following resumed, as asked, or the daemon refused with
    /// why.
    SubscriptionCancelled {
        session: String,
        refused: Option<String>,
    },
    /// A leave was withdrawn as asked, or the daemon refused with why.
    LeaveWithdrawn {
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
    appearance: Sender<Command>,
    keeper: Sender<Keep>,
    connect: Connect,
    /// This machine's daemon, whichever machine the window shows.
    local: Connect,
    /// The machine `connect` goes to, when the window may show another.
    aim: Option<crate::machines::Aim>,
    updates: Updates,
    bounds: Bounds,
    /// How this window signs what only the user's device may ask, when it was given its own
    /// (`Shell::sign_with`); the process's (`link::sign_with`) otherwise.
    sign: Option<Sign>,
}

impl Actions {
    /// Signs with `sign` from now on, rather than with the process's signer.
    pub fn sign_with(&mut self, sign: Sign) {
        self.sign = Some(sign);
    }

    /// Starts the details' thread; it ends when every `Actions` is dropped.
    pub fn spawn(connect: Connect, updates: impl Into<Updates>) -> Actions {
        Self::spawn_bounded(connect, updates.into(), BOUNDS)
    }

    /// The actions on `aim`'s connections, which go to the machine the window shows; what the
    /// window keeps (its folds, the drafts) and its appearance are this machine's, asked on
    /// `local`.
    pub fn spawn_aimed(aim: crate::machines::Aim, local: Connect, updates: Updates) -> Actions {
        let mut actions = Self::spawn_on(aim.connect(local.clone()), local, updates, BOUNDS);
        actions.aim = Some(aim);
        actions
    }

    pub(crate) fn spawn_bounded(
        connect: Connect,
        updates: impl Into<Updates>,
        bounds: Bounds,
    ) -> Actions {
        Self::spawn_on(connect.clone(), connect, updates.into(), bounds)
    }

    fn spawn_on(connect: Connect, local: Connect, updates: Updates, bounds: Bounds) -> Actions {
        let (details, asked) = channel();
        let (to, updates_to) = (connect.clone(), updates.clone());
        std::thread::Builder::new()
            .name("flottille-details".into())
            .spawn(move || read_details(to, asked, updates_to, bounds))
            .expect("a thread for the project page's details");
        // The appearance is asked one request at a time, in the order of the clicks: on threads of
        // their own, two quick choices could reach the daemon in the other order.
        let (appearance, chosen) = channel();
        let (to, updates_to) = (local.clone(), updates.clone());
        std::thread::Builder::new()
            .name("flottille-appearance".into())
            .spawn(move || {
                while let Ok(command) = chosen.recv() {
                    let outcome = perform(&to, None, command, bounds);
                    if updates_to.unbounded_send(Update::Done(outcome)).is_err() {
                        return;
                    }
                }
            })
            .expect("a thread for the window's appearance");
        // What the window keeps is written one request at a time, in the order it was asked: a
        // draft written, then forgotten, must not reach the daemon the other way round.
        let (keeper, kept) = channel();
        let (to, updates_to) = (local.clone(), updates.clone());
        std::thread::Builder::new()
            .name("flottille-keeper".into())
            .spawn(move || keep(to, kept, updates_to, bounds))
            .expect("a thread for what the window keeps");
        Actions {
            details,
            appearance,
            keeper,
            connect,
            local,
            aim: None,
            updates,
            bounds,
            sign: None,
        }
    }

    /// Waits until everything asked of the keeper so far reached the daemon, for `wait` at most:
    /// the window is closing, and what it holds must be written before the process ends.
    pub fn flush(&self, wait: Duration) {
        let (done, reached) = channel();
        if self.keeper.send(Keep::Barrier(done)).is_ok() {
            let _ = reached.recv_timeout(wait);
        }
    }

    pub fn send(&self, command: Command) {
        if matches!(
            command,
            Command::DraftRead { .. }
                | Command::DraftWrite { .. }
                | Command::WindowSet { .. }
                | Command::ArrangementChange { .. }
        ) {
            let _ = self.keeper.send(Keep::Do(command));
            return;
        }
        if let Command::Details(project) = command {
            let _ = self.details.send(project);
            return;
        }
        let sign = self.sign.clone().or_else(crate::link::signer);
        // Its tunnel ends at this machine's `127.0.0.1`, where the browser is: asked of this
        // machine's daemon, signed here for the machine that signs in, whichever one is shown.
        // Its connection stays open as long as the tunnel does: a thread of its own. It goes to
        // this machine's daemon, whichever machine is shown, which carries the request signed
        // for the machine of the run and listens on `127.0.0.1` here.
        if let Command::TunnelOpen(ask) = command {
            let (connect, updates, bounds) =
                (self.local.clone(), self.updates.clone(), self.bounds);
            std::thread::Builder::new()
                .name("flottille-tunnel".into())
                .spawn(move || hold_tunnel(&connect, sign, ask, bounds, &updates))
                .expect("a thread for a tunnel");
            return;
        }
        if let Command::TunnelClose { tunnel } = command {
            let (connect, updates, bounds) =
                (self.local.clone(), self.updates.clone(), self.bounds);
            std::thread::Builder::new()
                .name("flottille-tunnel-close".into())
                .spawn(move || {
                    let refused = match ask_alone(
                        &connect,
                        "tunnel.close",
                        json!({"tunnel": tunnel}),
                        bounds.quick,
                    ) {
                        Ok(Ok(_)) => return,
                        Ok(Err(why)) | Err(why) => why,
                    };
                    let news = crate::tunnels::News::NotClosed { why: refused };
                    let _ = updates.unbounded_send(Update::Done(Outcome::Tunnel(news)));
                })
                .expect("a thread for closing a tunnel");
            return;
        }
        if let Command::SignInOn { name, machine } = command {
            let (connect, updates, bounds) =
                (self.local.clone(), self.updates.clone(), self.bounds);
            std::thread::Builder::new()
                .name("flottille-sign-in".into())
                .spawn(move || sign_in_on(&connect, sign, &name, &machine, bounds, &updates))
                .expect("a thread for a sign-in on another machine");
            return;
        }
        if matches!(
            command,
            Command::AppearanceRead { .. } | Command::AppearanceSet { .. }
        ) {
            let _ = self.appearance.send(command);
            return;
        }
        // A share is the shown machine's entry or connection going to another machine, on the
        // user's consent: the device's signature, which that machine checks and sends on with
        // it. Shown, another machine is asked through the aim, which signs each request for it
        // with the device key; this one is asked on its own connection, signed here.
        let shown = self.aim.as_ref().and_then(crate::machines::Aim::machine);
        let command = match command {
            Command::Signed {
                machine: None,
                method,
                params,
                then,
            } if shown.is_some() => Command::Ask {
                method,
                params,
                then,
            },
            command => command,
        };
        let (updates, bounds) = (self.updates.clone(), self.bounds);
        if let (Some(_), Command::VaultShare { name, machine }) = (&shown, &command) {
            let (connect, name, machine) = (self.connect.clone(), name.clone(), machine.clone());
            std::thread::Builder::new()
                .name("flottille-action".into())
                .spawn(move || {
                    let params = json!({"name": name, "machine": machine});
                    let refused = match ask_alone(&connect, "vault.share", params, bounds.quick) {
                        Ok(Ok(_)) => None,
                        Ok(Err(why)) | Err(why) => Some(why),
                    };
                    let outcome = Outcome::VaultShared {
                        name,
                        machine,
                        refused,
                    };
                    let _ = updates.unbounded_send(Update::Done(outcome));
                })
                .expect("a thread for a window's action");
            return;
        }
        let connect = match &command {
            // This window's build and its update are this machine's, whichever machine is shown:
            // `update.download` and `update.restart` are never carried.
            Command::Update { .. } => self.local.clone(),
            Command::Signed {
                machine: Some(_), ..
            } => self.local.clone(),
            Command::VaultShare { .. } | Command::Signed { .. } | Command::ShareMachines
                if shown.is_none() =>
            {
                self.local.clone()
            }
            _ => self.connect.clone(),
        };
        std::thread::Builder::new()
            .name("flottille-action".into())
            .spawn(move || {
                let outcome = perform(&connect, sign, command, bounds);
                let _ = updates.unbounded_send(Update::Done(outcome));
            })
            .expect("a thread for a window's action");
    }
}

/// What the keeper's thread is asked.
enum Keep {
    Do(Command),
    /// Answers once everything asked before it was.
    Barrier(Sender<()>),
}

/// The keeper's thread: what the window keeps, one request at a time on a connection kept.
fn keep(connect: Connect, asked: Receiver<Keep>, updates: Updates, bounds: Bounds) {
    let mut conn: Option<Conn> = None;
    while let Ok(next) = asked.recv() {
        let command = match next {
            Keep::Barrier(done) => {
                let _ = done.send(());
                continue;
            }
            Keep::Do(command) => command,
        };
        let outcome = match command {
            Command::DraftRead { key } => {
                let params = json!({"kind": key.kind(), "target": key.target()});
                let found = match coded(&connect, &mut conn, "draft.get", params, bounds.quick) {
                    Ok(Ok(answer)) => Ok(draft_of(&answer["draft"])),
                    // Gone: no composer, no draft.
                    Ok(Err(refused)) if refused.code == "not-found" => Ok(None),
                    Ok(Err(refused)) => Err(refused.message),
                    Err(why) => Err(why),
                };
                Outcome::Draft { key, found }
            }
            Command::DraftWrite {
                key,
                text,
                pictures,
                after_read,
            } => {
                let params = json!({"kind": key.kind(), "target": key.target()});
                // The window is closing and did not read the draft: what is typed replaces it
                // only if the daemon can say that it holds one the field did not know.
                let readable = !after_read
                    || coded(
                        &connect,
                        &mut conn,
                        "draft.get",
                        params.clone(),
                        bounds.quick,
                    )
                    .is_ok_and(|read| read.is_ok());
                let result = if readable {
                    let mut params = params;
                    params["text"] = json!(text);
                    if let Some(pictures) = pictures {
                        params["images"] = pictures.iter().map(Picture::to_json).collect();
                    }
                    written(coded(
                        &connect,
                        &mut conn,
                        "draft.set",
                        params,
                        bounds.quick,
                    ))
                } else {
                    Written::Retry
                };
                Outcome::DraftWritten { key, result }
            }
            Command::WindowSet { key, value } => {
                let params = json!({"key": key, "value": value});
                let result = written(coded(
                    &connect,
                    &mut conn,
                    "window.set",
                    params,
                    bounds.quick,
                ));
                Outcome::WindowKept { key, value, result }
            }
            Command::ArrangementChange { change } => {
                let read = coded(&connect, &mut conn, "window.get", json!({}), bounds.quick);
                match read {
                    Ok(Ok(state)) => {
                        let mut arrangement = Arrangement::of(&state["state"][arrange::KEY]);
                        let before = arrangement.clone();
                        arrangement.apply(&change);
                        // What the change does not change is not written.
                        let result = match arrangement == before {
                            true => Written::Kept,
                            false => written(coded(
                                &connect,
                                &mut conn,
                                "window.set",
                                json!({"key": arrange::KEY, "value": arrangement.to_json()}),
                                bounds.quick,
                            )),
                        };
                        Outcome::ArrangementKept {
                            change,
                            arrangement: Some(arrangement),
                            result,
                        }
                    }
                    other => Outcome::ArrangementKept {
                        change,
                        arrangement: None,
                        result: match other {
                            Ok(Err(refused)) => written(Ok(Err(refused))),
                            _ => Written::Retry,
                        },
                    },
                }
            }
            _ => unreachable!("only what the window keeps is sent to the keeper"),
        };
        if updates.unbounded_send(Update::Done(outcome)).is_err() {
            return;
        }
    }
}

/// How a write ended, by what the daemon answered.
fn written(asked: Result<Result<Value, flottille_protocol::ErrorBody>, String>) -> Written {
    match asked {
        Ok(Ok(_)) => Written::Kept,
        Ok(Err(refused)) => match refused.code.as_str() {
            "not-found" => Written::Gone,
            "unavailable" => Written::Retry,
            _ => Written::Refused,
        },
        Err(_) => Written::Retry,
    }
}

/// [`ask`], with the code of the daemon's refusal beside its sentence.
fn coded(
    connect: &Connect,
    conn: &mut Option<Conn>,
    method: &str,
    params: Value,
    bound: Duration,
) -> Result<Result<Value, flottille_protocol::ErrorBody>, String> {
    if conn.is_none() {
        *conn = Some(connected(connect)?);
    }
    let answer = conn
        .as_mut()
        .expect("connected")
        .ask_within_coded(method, params, bound);
    if answer.is_err() {
        *conn = None;
    }
    answer.map_err(|e| e.to_string())
}

/// A draft as the daemon answers it (`null` when it holds none).
fn draft_of(found: &Value) -> Option<Draft> {
    let text = found["text"].as_str()?.to_owned();
    let pictures = found["images"]
        .as_array()
        .map(|images| images.iter().filter_map(Picture::of).collect())
        .unwrap_or_default();
    Some(Draft { text, pictures })
}

fn read_details(connect: Connect, asked: Receiver<String>, updates: Updates, bounds: Bounds) {
    let mut conn: Option<Conn> = None;
    let mut tickets = Tickets::new();
    let mut pulls = Pulls::new();
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
            // What git knows is sent as soon as it is read, the slow parts as each is read.
            let mut partial = |details: &Details| {
                updates
                    .unbounded_send(Update::Done(Outcome::Details(details.clone())))
                    .is_ok()
            };
            if let Some(details) = details(
                &connect,
                &mut conn,
                (&mut tickets, &mut pulls),
                &project,
                bounds.quick,
                &mut partial,
            ) && updates
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
    let stream = connect().map_err(|e| format!("cannot reach Flottille: {e}"))?;
    Conn::new(stream).map_err(|e| e.to_string())
}

/// The parameters of a guidance's method, given its part when it has one.
fn of_part(part: &Option<String>, mut params: Value) -> Value {
    if let Some(part) = part {
        params["part"] = json!(part);
    }
    params
}

/// `vault.share` of the entry `name` with `machine`: a request of this machine's, signed with this
/// device's key, whose signature goes with the secret for that machine to check.
fn share(
    connect: &Connect,
    sign: Option<Sign>,
    name: &str,
    machine: &str,
    bound: Duration,
) -> Result<(), String> {
    let sign = sign.ok_or(
        "this window cannot sign with this device's key: share it with flottille vault share",
    )?;
    let this = ask_alone(connect, "remote.machines", json!({}), bound)??["this"]["name"]
        .as_str()
        .map(str::to_owned)
        .ok_or("this machine is no member of the user's yet")?;
    connected(connect)?
        .signing(sign, this)
        .ask_within(
            "vault.share",
            json!({"name": name, "machine": machine}),
            bound,
        )?
        .map(|_| ())
}

/// `method` on a connection of its own, signed with this device's key for the machine `machine`,
/// this one when `None` (a request that carries the user's consent further: `connection.share`).
fn signed_by_device(
    connect: &Connect,
    sign: Option<Sign>,
    machine: Option<String>,
    method: &str,
    params: Value,
    bound: Duration,
) -> Result<Value, String> {
    let sign =
        sign.ok_or("this window cannot sign with this device's key: use the command line")?;
    let machine = match machine {
        Some(machine) => machine,
        None => ask_alone(connect, "remote.machines", json!({}), bound)??["this"]["name"]
            .as_str()
            .map(str::to_owned)
            .ok_or("this machine is no member of the user's yet")?,
    };
    connected(connect)?
        .signing(sign, machine)
        .ask_within(method, params, bound)?
}

/// How long the window waits for a sign-in on another machine: the daemon's own wait, and a
/// little more.
const SIGN_IN_WAIT: Duration = Duration::from_secs(310);

/// The sign-in of `name` on `machine` ([`Command::SignInOn`]): `connection.login` there, then
/// `tunnel.open {sign_in}` held on a connection of its own, whose end is the sign-in's. Says the
/// address to open, then how it ended.
fn sign_in_on(
    connect: &Connect,
    sign: Option<Sign>,
    name: &str,
    machine: &str,
    bounds: Bounds,
    updates: &Updates,
) {
    let said = |then: crate::extensions::connections::Then, answer: Result<Value, String>| {
        updates
            .unbounded_send(Update::Done(Outcome::Asked {
                then: crate::extensions::Then::Connections(then),
                answer,
            }))
            .is_ok()
    };
    let opened = crate::extensions::connections::Then::SignInOpened {
        name: name.to_owned(),
        machine: machine.to_owned(),
    };
    let ended = crate::extensions::connections::Then::SignedInOn {
        name: name.to_owned(),
        machine: machine.to_owned(),
    };
    let begun = (|| {
        let url = signed_by_device(
            connect,
            sign.clone(),
            Some(machine.to_owned()),
            "connection.login",
            json!({"name": name}),
            bounds.quick.max(Duration::from_secs(40)),
        )?;
        let sign =
            sign.ok_or("this window cannot sign with this device's key: use the command line")?;
        let mut tunnel = connected(connect)?.signing(sign, machine.to_owned());
        tunnel.ask_within("tunnel.open", json!({"sign_in": name}), bounds.quick)??;
        Ok::<_, String>((url, tunnel))
    })();
    let (url, mut tunnel) = match begun {
        Ok(begun) => begun,
        Err(why) => {
            said(opened, Err(why));
            return;
        }
    };
    if !said(opened, Ok(url)) {
        return;
    }
    let deadline = Instant::now() + SIGN_IN_WAIT;
    let answer = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tunnel.next_event(Some(left)) {
            Ok(Some(event)) if event.name == "tunnel.closed" => {
                break match event.data["signed_in"] == true {
                    true => Ok(Value::Null),
                    false => Err(event.data["reason"]
                        .as_str()
                        .unwrap_or("it ended without signing in")
                        .to_owned()),
                };
            }
            Ok(Some(_)) => {}
            Ok(None) => break Err("the browser did not come back in time".to_owned()),
            Err(why) => break Err(why),
        }
    };
    said(ended, answer);
}

/// A tunnel to a run of `ask.machine` ([`Command::TunnelOpen`]): `tunnel.open` signed for it, on
/// a connection that stays open, so that the tunnel lives as long as it does. Said when it is
/// open (or why not), and when it ends: the daemon says why on that connection, which also ends
/// with the window.
fn hold_tunnel(
    connect: &Connect,
    sign: Option<Sign>,
    ask: crate::tunnels::Ask,
    bounds: Bounds,
    updates: &Updates,
) {
    use crate::tunnels::{News, Tunnel, run_for_port};
    let said = |news: News| {
        updates
            .unbounded_send(Update::Done(Outcome::Tunnel(news)))
            .is_ok()
    };
    let begun = (|| {
        let sign =
            sign.ok_or("this window cannot sign with this device's key: use the command line")?;
        let mut tunnel = connected(connect)?.signing(sign, ask.machine.clone());
        // A link names a port, not a run: the machine says which of its runs are running.
        let run = match (&ask.run, ask.port) {
            (Some(run), _) => run.clone(),
            (None, Some(port)) => {
                let listed = tunnel.ask_within("run.list", json!({}), bounds.quick)??;
                run_for_port(&listed, port, &ask.machine)?
            }
            (None, None) => return Err("nothing to reach".to_owned()),
        };
        let answer = tunnel.ask_within(
            "tunnel.open",
            json!({"run": run, "port": ask.port}),
            bounds.quick,
        )??;
        let opened = Tunnel::of(&ask, &run, &answer)?;
        Ok::<_, String>((opened, tunnel))
    })();
    let (opened, mut held) = match begun {
        Ok(begun) => begun,
        Err(why) => {
            said(News::Refused { ask, why });
            return;
        }
    };
    let id = opened.id.clone();
    if !said(News::Opened {
        ask,
        tunnel: opened,
    }) {
        return;
    }
    let reason = loop {
        match held.next_event(Some(TUNNEL_LISTEN)) {
            Ok(Some(event)) if event.name == "tunnel.closed" && event.data["tunnel"] == id => {
                break event.data["reason"]
                    .as_str()
                    .unwrap_or("it ended")
                    .to_owned();
            }
            Ok(Some(_)) => {}
            // Nothing came: the window may have gone, which closes the tunnel with this thread.
            Ok(None) if updates.is_closed() => return,
            Ok(None) => {}
            Err(why) => break why,
        }
    };
    said(News::Closed { tunnel: id, reason });
}

/// How often a connection that holds a tunnel looks whether the window is still there.
const TUNNEL_LISTEN: Duration = Duration::from_secs(1);

/// One request on a connection of its own.
fn ask_alone(
    connect: &Connect,
    method: &str,
    params: Value,
    bound: Duration,
) -> Result<Result<Value, String>, String> {
    connected(connect)?.ask_within(method, params, bound)
}

/// `method` with `words` and the pictures and pasted texts `files`, as [`ask_alone`] asks it;
/// when, signed for another machine, it would not fit one frame of the link, its files go first in
/// chunks (`agent.upload`) and the message names their upload (`docs/remote.md`, "Pictures sent
/// to another machine"). A chunk refused is the message's refusal: it says which picture and why.
/// It is judged from the files' sizes, and a file is encoded only as it goes.
fn ask_with_files(
    connect: &Connect,
    method: &str,
    mut words: Value,
    (images, texts): (&[Picture], &[Pasted]),
    bound: Duration,
) -> Result<Result<Value, String>, String> {
    let files: Vec<flottille_link::Attached> = images
        .iter()
        .map(|picture| flottille_link::Attached::Picture {
            mime: &picture.mime,
            bytes: &picture.bytes,
        })
        .chain(
            texts
                .iter()
                .map(|pasted| flottille_link::Attached::Text(&pasted.text)),
        )
        .collect();
    if files.is_empty() || flottille_link::fits_with(method, &words, &files) {
        words["images"] = images.iter().map(Picture::to_json).collect();
        if !texts.is_empty() {
            words["texts"] = texts.iter().map(Pasted::to_json).collect();
        }
        return ask_alone(connect, method, words, bound);
    }
    // The words still travel in one frame: when they cannot, nothing is sent.
    if !flottille_link::fits(method, &words) {
        return Ok(Err(
            "this message is too big for the link: its words take 1 MiB at most".to_owned(),
        ));
    }
    let upload = flottille_link::upload_name()?;
    let mut conn = connected(connect)?;
    for chunk in flottille_link::upload_chunks(&upload, &files) {
        match conn.ask_within("agent.upload", chunk, bound) {
            Ok(Ok(_)) => {}
            refused => return refused,
        }
    }
    words["upload"] = json!(upload);
    conn.ask_within(method, words, bound)
}

/// What the user does to a command, and what the daemon answered.
fn ask_run(connect: &Connect, act: RunAct, run: &str, bound: Duration) -> RunAnswer {
    run_answer(
        act,
        run,
        ask_alone(connect, act.method(), json!({"run": run}), bound),
    )
}

fn run_answer(
    act: RunAct,
    run: &str,
    answered: Result<Result<Value, String>, String>,
) -> RunAnswer {
    let (refused, restarted, notice) = match answered {
        Ok(Ok(answer)) if act == RunAct::Restart => match answer["run"]["run"].as_str() {
            Some(new) => (
                None,
                Some(new.to_owned()),
                answer["notice"].as_str().map(str::to_owned),
            ),
            // Nothing to follow and nothing that waits: said like a refusal.
            None => (
                Some("Flottille answered without the new run".to_owned()),
                None,
                None,
            ),
        },
        Ok(Ok(_)) => (None, None, None),
        Ok(Err(why)) | Err(why) => (Some(why), None, None),
    };
    RunAnswer {
        act,
        run: run.to_owned(),
        refused,
        restarted,
        notice,
    }
}

/// A step of a program's sign-in that answers nothing but whether it was done.
fn sign_in_step(
    connect: &Connect,
    method: &str,
    what: &'static str,
    program: String,
    time: Duration,
) -> Outcome {
    let refused = match ask_alone(connect, method, json!({"program": program}), time) {
        Ok(Ok(_)) => None,
        Ok(Err(why)) | Err(why) => Some(why),
    };
    Outcome::SignInAnswered {
        program,
        what,
        refused,
    }
}

fn perform(connect: &Connect, sign: Option<Sign>, command: Command, bounds: Bounds) -> Outcome {
    match command {
        Command::Details(_) => unreachable!("the details have their own thread"),
        Command::DraftRead { .. }
        | Command::DraftWrite { .. }
        | Command::WindowSet { .. }
        | Command::ArrangementChange { .. } => {
            unreachable!("what the window keeps has its own thread")
        }
        Command::Start {
            project,
            target,
            pick,
            message,
            images,
            texts,
            mode,
        } => {
            let workspace = match target {
                Target::Main => None,
                Target::Workspace(name) => Some(name),
            };
            match ask_with_files(
                connect,
                "agent.start",
                start_params(&project, workspace.as_deref(), &pick, &message, &mode),
                (&images, &texts),
                bounds.quick,
            ) {
                Ok(Ok(started)) => started_outcome(&project, &message, &started),
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
        Command::SetJudge { .. } | Command::PickJudge => {
            let params = match command {
                Command::SetJudge { provider, model } => {
                    json!({"provider": provider, "model": model})
                }
                _ => json!({"pick": true}),
            };
            let refused = match ask_alone(connect, "provider.set_judge", params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::JudgeSet { refused }
        }
        Command::SetNewSession {
            provider,
            model,
            effort,
        } => {
            let params = json!({"provider": provider, "model": model, "effort": effort});
            let refused = match ask_alone(connect, "provider.set_new_session", params, bounds.quick)
            {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::NewSessionSet { refused }
        }
        Command::JudgeNoticed => {
            // Shown or not, the notice goes once it was seen: the providers say it.
            let _ = ask_alone(connect, "provider.judge_noticed", json!({}), bounds.quick);
            let read = crate::providers::next_read();
            Outcome::Providers(read, read_providers(connect, bounds.quick))
        }
        Command::CompactionRead
        | Command::CompactionSet { .. }
        | Command::CompactionGuidance { .. }
        | Command::CompactionNoticed => {
            let (method, params, written, guidance) = match command {
                Command::CompactionSet { params } => ("compaction.set", params, true, false),
                Command::CompactionGuidance { text: Some(text) } => (
                    "compaction.set_guidance",
                    json!({"text": text}),
                    false,
                    true,
                ),
                Command::CompactionGuidance { text: None } => {
                    ("compaction.reset_guidance", json!({}), false, true)
                }
                Command::CompactionNoticed => ("compaction.noticed", json!({}), false, false),
                _ => ("compaction.show", json!({}), false, false),
            };
            Outcome::Compaction {
                written,
                guidance,
                shown: answer(ask_alone(connect, method, params, bounds.quick)),
            }
        }
        Command::GuidanceRead { name, part } => {
            let shown = ask_alone(
                connect,
                &format!("{name}.show"),
                of_part(&part, json!({})),
                bounds.quick,
            );
            Outcome::Guidance {
                name,
                part,
                shown: shown.and_then(|shown| shown).map(|shown| Shown::of(&shown)),
                written: false,
            }
        }
        Command::GuidanceWrite { name, part, text } => {
            let shown = match &text {
                Some(text) => ask_alone(
                    connect,
                    &format!("{name}.set"),
                    of_part(&part, json!({"text": text})),
                    bounds.quick,
                ),
                None => ask_alone(
                    connect,
                    &format!("{name}.reset"),
                    of_part(&part, json!({})),
                    bounds.quick,
                ),
            };
            Outcome::Guidance {
                name,
                part,
                shown: shown.and_then(|shown| shown).map(|shown| Shown::of(&shown)),
                written: true,
            }
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
        Command::AboutRead => Outcome::About {
            written: false,
            shown: answer(ask_alone(connect, "about.show", json!({}), bounds.quick))
                .map(|shown| crate::about::Shown::of(&shown)),
        },
        Command::AboutWrite { text } => {
            let asked = match &text {
                Some((name, pronouns, notes)) => ask_alone(
                    connect,
                    "about.set",
                    json!({"name": name, "pronouns": pronouns, "notes": notes}),
                    bounds.quick,
                ),
                None => ask_alone(connect, "about.reset", json!({}), bounds.quick),
            };
            Outcome::About {
                written: true,
                shown: answer(asked).map(|shown| crate::about::Shown::of(&shown)),
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
        Command::FindingsRead => Outcome::Findings(
            answer(ask_alone(
                connect,
                "developer.reports",
                json!({}),
                bounds.quick,
            ))
            .map(|said| crate::findings::Finding::all(&said)),
        ),
        Command::FindingsExport => Outcome::FindingsExported(
            answer(ask_alone(
                connect,
                "developer.reports_export",
                json!({}),
                bounds.quick,
            ))
            .map(|said| said["file"].as_str().unwrap_or_default().to_owned()),
        ),
        Command::Update { method, params } => {
            let answer = answer(ask_alone(connect, method, params, bounds.quick));
            Outcome::Update { method, answer }
        }
        Command::Ask {
            method,
            params,
            then,
        } => {
            let answer = answer(ask_alone(connect, &method, params, bounds.quick));
            Outcome::Asked { then, answer }
        }
        Command::Signed {
            machine,
            method,
            params,
            then,
        } => {
            let answer = signed_by_device(connect, sign, machine, &method, params, bounds.quick);
            Outcome::Asked { then, answer }
        }
        Command::SignInOn { .. } => unreachable!("a sign-in on another machine has its thread"),
        Command::TunnelOpen(_) | Command::TunnelClose { .. } => {
            unreachable!("a tunnel has its thread")
        }
        Command::ToolResultsRead
        | Command::ToolResultsSet { .. }
        | Command::UnchangedSet { .. } => {
            let (method, params, written) = match command {
                Command::ToolResultsSet { limit } => (
                    "tool_results.set",
                    json!({"limit": limit.map_or(json!("off"), |l| json!(l))}),
                    true,
                ),
                Command::UnchangedSet { unchanged } => {
                    ("tool_results.set", json!({"unchanged": unchanged}), true)
                }
                _ => ("tool_results.show", json!({}), false),
            };
            let shown = answer(ask_alone(connect, method, params, bounds.quick)).map(|shown| {
                (
                    shown["limit"].as_u64().map(|limit| limit as usize),
                    shown["unchanged"] == true,
                )
            });
            Outcome::ToolResults { written, shown }
        }
        Command::SessionsRead | Command::SessionsSet { .. } | Command::ResumeSet { .. } => {
            let (method, params, written) = match command {
                Command::SessionsSet { days } => (
                    "sessions.set",
                    json!({"days": days.map_or(json!("off"), |d| json!(d))}),
                    Some(SessionsSetting::Days),
                ),
                Command::ResumeSet { resume } => (
                    "sessions.set",
                    json!({"resume": resume}),
                    Some(SessionsSetting::Resume),
                ),
                _ => ("sessions.show", json!({}), None),
            };
            let shown = answer(ask_alone(connect, method, params, bounds.quick))
                .map(|shown| (shown["days"].as_u64(), shown["resume"] == true));
            Outcome::Sessions { written, shown }
        }
        Command::AppearanceRead { seq } | Command::AppearanceSet { seq, .. } => {
            let (method, params, written) = match command {
                Command::AppearanceSet { appearance, .. } => {
                    ("appearance.set", appearance_json(appearance), true)
                }
                _ => ("appearance.show", json!({}), false),
            };
            let shown = answer(ask_alone(connect, method, params, bounds.quick))
                .and_then(|shown| appearance_of(&shown));
            Outcome::Appearance {
                seq,
                written,
                shown,
            }
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
        Command::SaveConvention {
            project,
            key,
            value,
        } => Outcome::Convention {
            saved: answer(ask_alone(
                connect,
                "project.set_conventions",
                json!({"name": project, key: value}),
                bounds.quick,
            ))
            .map(|saved| saved[key].as_str().unwrap_or_default().to_owned()),
            project,
            key,
        },
        Command::ReadCommitChecks(project) => Outcome::CommitChecks {
            read: answer(ask_alone(
                connect,
                "project.commit_checks",
                json!({"name": project}),
                bounds.quick,
            )),
            project,
            saved: None,
        },
        Command::SaveCommitChecks {
            project,
            repository,
            pre_commit,
            commit_msg,
        } => Outcome::CommitChecks {
            read: answer(ask_alone(
                connect,
                "project.set_commit_checks",
                json!({
                    "name": project,
                    "repository": repository,
                    "pre_commit": pre_commit,
                    "commit_msg": commit_msg,
                }),
                bounds.quick,
            )),
            project,
            saved: Some(repository),
        },
        Command::SaveMessageChecks { project, checks } => Outcome::MessageChecks {
            saved: answer(ask_alone(
                connect,
                "project.set_message_checks",
                json!({"name": project, "checks": checks}),
                bounds.quick,
            ))
            .map(|saved| {
                saved["message_checks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            }),
            project,
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
        Command::Programs { project, serial } => Outcome::Programs {
            read: answer(ask_alone(
                connect,
                "project.programs",
                json!({"name": project}),
                bounds.quick,
            )),
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
        Command::ReadEnding { session } => Outcome::EndingRead {
            read: answer(ask_alone(
                connect,
                "agent.ending",
                json!({"agent": session}),
                bounds.quick,
            )),
            session,
        },
        Command::ReadRemoval { project, serial } => Outcome::RemovalRead {
            read: answer(ask_alone(
                connect,
                "project.removal",
                json!({"name": project}),
                bounds.quick,
            )),
            project,
            serial,
        },
        // Its agents stop and its worktrees are removed: git's work.
        Command::RemoveProject { project, discard } => Outcome::ProjectRemoved {
            result: answer(ask_alone(
                connect,
                "project.remove",
                json!(flottille_protocol::ProjectRemove {
                    name: project.clone(),
                    discard,
                }),
                bounds.slow,
            )),
            project,
        },
        Command::ProjectFolders { machine, path } => Outcome::ProjectFolders {
            machine,
            read: answer(ask_alone(
                connect,
                "project.folders",
                json!({"path": path}),
                bounds.quick,
            )),
        },
        Command::DetectProject { folder, name, seq } => Outcome::ProjectDetected {
            seq,
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
        Command::SignIns => Outcome::SignIns(
            ask_alone(connect, "signin.list", json!({}), bounds.quick).and_then(|listed| listed),
        ),
        Command::SignInStart(program) => Outcome::SignInStarted {
            answer: ask_alone(
                connect,
                "signin.start",
                json!({"program": program}),
                bounds.quick,
            )
            .and_then(|started| started),
            program,
        },
        Command::SignInCode(program, PastedCode(code)) => {
            let refused = match ask_alone(
                connect,
                "signin.code",
                json!({"program": program, "code": code}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::SignInAnswered {
                program,
                what: "code",
                refused,
            }
        }
        Command::SignInCancel(program) => {
            sign_in_step(connect, "signin.cancel", "cancel", program, bounds.quick)
        }
        Command::SignOut(program) => sign_in_step(
            connect,
            "signin.sign_out",
            "sign_out",
            program,
            bounds.quick,
        ),
        Command::Vault => {
            let listed = ask_alone(connect, "vault.list", json!({}), bounds.quick);
            Outcome::Vault {
                listed: listed.and_then(|listed| listed).map(|listed| {
                    listed["entries"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(crate::vault::VaultEntry::of)
                        .collect()
                }),
            }
        }
        Command::VaultWrite(write) => {
            let (method, params) = match write {
                VaultWrite::Set {
                    name,
                    secret,
                    description,
                    url,
                    username,
                    project,
                    bound,
                } => (
                    "vault.set",
                    json!({"name": name, "secret": secret, "description": description,
                        "url": url, "username": username, "project": project, "for": bound}),
                ),
                VaultWrite::Remove { name, confirmed } => (
                    "vault.remove",
                    json!({"name": name, "confirmed": confirmed}),
                ),
            };
            let refused = match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::VaultWritten { refused }
        }
        Command::ShareMachines => {
            let listed = ask_alone(connect, "remote.machines", json!({}), bounds.quick)
                .and_then(|listed| listed)
                .map(|listed| {
                    listed["machines"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|machine| machine["state"] == "online")
                        // A share is for a runner: a controller's machine would refuse it.
                        .filter(|machine| machine["role"] == "runner")
                        .filter_map(|machine| machine["name"].as_str().map(str::to_owned))
                        .collect()
                });
            Outcome::ShareMachines { listed }
        }
        Command::VaultShare { name, machine } => {
            let refused = share(connect, sign, &name, &machine, bounds.quick).err();
            Outcome::VaultShared {
                name,
                machine,
                refused,
            }
        }
        Command::VaultDismiss { ask } => {
            let refused = match ask_alone(
                connect,
                "vault.ask_dismiss",
                json!({"ask": ask}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::VaultDismissed { ask, refused }
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
        Command::WorkspacesFolderRead { project } => {
            let shown = ask_alone(
                connect,
                "workspaces_folder.show",
                json!({"project": project}),
                bounds.quick,
            );
            Outcome::WorkspacesFolder {
                shown: shown
                    .and_then(|shown| shown)
                    .map(|shown| crate::workspaces_folder::Shown::of(&shown, project.is_some())),
                written: false,
                project,
            }
        }
        Command::WorkspacesFolderSet {
            project,
            place,
            path,
        } => {
            let params = match &project {
                Some(project) => json!({"project": project, "own": path}),
                None => json!({"place": place, "path": path}),
            };
            let shown = ask_alone(connect, "workspaces_folder.set", params, bounds.quick);
            Outcome::WorkspacesFolder {
                shown: shown
                    .and_then(|shown| shown)
                    .map(|shown| crate::workspaces_folder::Shown::of(&shown, project.is_some())),
                written: true,
                project,
            }
        }
        Command::TravelledFolderRead => {
            let shown = ask_alone(connect, "travelled_folder.show", json!({}), bounds.quick);
            Outcome::TravelledFolder {
                shown: shown
                    .and_then(|shown| shown)
                    .map(|shown| crate::travelled_folder::Shown::of(&shown)),
                written: false,
            }
        }
        Command::TravelledFolderSet { folder } => {
            let shown = ask_alone(
                connect,
                "travelled_folder.set",
                json!({"folder": folder}),
                bounds.quick,
            );
            Outcome::TravelledFolder {
                shown: shown
                    .and_then(|shown| shown)
                    .map(|shown| crate::travelled_folder::Shown::of(&shown)),
                written: true,
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
        Command::RulesAlone { project, alone } => {
            let shown = ask_alone(
                connect,
                "rules.set",
                json!({"project": project, "alone": alone}),
                bounds.quick,
            );
            Outcome::RulesSwitched {
                shown: shown.and_then(|shown| shown).map(|shown| Shown::of(&shown)),
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
        Command::Progress { project, asked } => {
            let params = json!({"project": project});
            let read = ask_alone(connect, "project.progress", params, bounds.quick)
                .and_then(|read| read)
                .map(|read| crate::progress::Progress::of(&read));
            Outcome::Progress {
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
        Command::RunTask {
            project,
            name,
            from,
        } => {
            let mut params = json!({"project": project, "name": name});
            if let RunFrom::Session {
                workspace: Some(workspace),
            }
            | RunFrom::Page {
                workspace: Some(workspace),
            } = &from
            {
                params["workspace"] = workspace.as_str().into();
            }
            let refused = match ask_alone(connect, "task.run", params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::TaskRun {
                project,
                name,
                from,
                refused,
            }
        }
        Command::ProjectRuns { project, output } => {
            let listed = ask_alone(
                connect,
                "run.list",
                json!({"project": project, "ended": true}),
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
        Command::ProjectRun { project, run, act } => Outcome::ProjectRunAnswered {
            project,
            answer: ask_run(connect, act, &run, bounds.quick),
        },
        Command::Services => {
            let listed = ask_alone(connect, "service.list", json!({}), bounds.quick);
            Outcome::Services {
                listed: listed.and_then(|listed| listed).map(|listed| {
                    listed["services"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(crate::services::ServiceInfo::of)
                        .collect()
                }),
            }
        }
        Command::ServiceWrite { method, params } => {
            let refused = match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::ServiceWritten { refused }
        }
        Command::ServiceAct { name, start, force } => {
            // A start waits for what the service needs to be ready.
            let (method, params, bound) = match start {
                true => ("service.start", json!({"name": name}), bounds.slow),
                false => (
                    "service.stop",
                    json!({"name": name, "force": force}),
                    bounds.quick,
                ),
            };
            let asked = connected(connect)
                .and_then(|mut conn| conn.ask_within_coded(method, params, bound));
            let (answer, needed) = match asked {
                Ok(Ok(answer)) => (Ok(answer), None),
                Ok(Err(refused)) => {
                    // A daemon that does not carry the data says nothing to list: its sentence is
                    // what the toast shows.
                    let needed = (refused.code == "needed")
                        .then(|| {
                            let data = refused.data.unwrap_or_default();
                            crate::tasks::names_of(&data["needed_by"])
                        })
                        .filter(|needed| !needed.is_empty());
                    (Err(refused.message), needed)
                }
                Err(why) => (Err(why), None),
            };
            Outcome::ServiceActed {
                name,
                start,
                answer,
                needed,
            }
        }
        Command::ServiceOutput { run } => {
            let output = ask_alone(connect, "run.output", json!({"run": run}), bounds.quick);
            Outcome::ServiceOutput {
                output: match output {
                    Ok(Ok(output)) => Some(Box::new(CommandOutput::of(&output))),
                    Ok(Err(_)) | Err(_) => None,
                },
            }
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
        Command::AnswerQuestion {
            session,
            seq,
            answers,
        } => {
            let refused = match ask_alone(
                connect,
                "agent.answer",
                json!({"agent": session, "question": seq, "answers": answers}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::QuestionAnswered {
                session,
                seq,
                refused,
            }
        }
        Command::Reply {
            session,
            key,
            message,
            images,
            texts,
            compact,
        } => {
            let mut params = json!({"agent": session, "message": message});
            if let Some(compact) = compact {
                params["compact"] = json!(compact);
            }
            let answer = match ask_with_files(
                connect,
                "agent.send",
                params,
                (&images, &texts),
                bounds.quick,
            ) {
                Ok(Ok(taken)) => Ok(crate::session::Accepted {
                    delivery: taken["delivery"].as_str().unwrap_or_default().to_owned(),
                    after: taken["after"].as_u64().unwrap_or_default(),
                }),
                Ok(Err(why)) | Err(why) => Err(why),
            };
            Outcome::Replied {
                session,
                key,
                answer,
            }
        }
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
        Command::Compact { session } => {
            let answer = ask_alone(
                connect,
                "agent.compact",
                json!({"agent": session}),
                bounds.quick,
            );
            let (after_turn, refused) = match answer {
                Ok(Ok(answer)) => (answer["after_turn"] == true, None),
                Ok(Err(why)) | Err(why) => (false, Some(why)),
            };
            Outcome::Compacting {
                session,
                after_turn,
                refused,
            }
        }
        Command::Rename { session, title } => {
            let refused = match ask_alone(
                connect,
                "agent.rename",
                json!({"agent": session, "title": title}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::Renamed { refused }
        }
        Command::Detach { session, from } => {
            let refused = match ask_alone(
                connect,
                "agent.detach",
                json!({"agent": session}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::Detached { from, refused }
        }
        Command::ReloadTools { session } => {
            let answer = ask_alone(
                connect,
                "agent.reload_tools",
                json!({"agent": session}),
                bounds.quick,
            );
            let (after_turn, refused) = match answer {
                Ok(Ok(answer)) => (answer["after_turn"] == true, None),
                Ok(Err(why)) | Err(why) => (false, Some(why)),
            };
            Outcome::ToolsReloaded {
                session,
                after_turn,
                refused,
            }
        }
        Command::QuotaResume { session, resume } => {
            let refused = match ask_alone(
                connect,
                "agent.quota_resume",
                json!({"agent": session, "resume": resume}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::QuotaResumed { session, refused }
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
        Command::SetOptions {
            session,
            model,
            effort,
        } => {
            let refused = match ask_alone(
                connect,
                "agent.options",
                json!({"agent": session, "model": model, "effort": effort}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::OptionsSet { session, refused }
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
        Command::Run { session, run, act } => Outcome::RunAnswered {
            session,
            answer: ask_run(connect, act, &run, bounds.quick),
        },
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
        Command::WithdrawLeave {
            session,
            writer,
            leave_session,
        } => {
            let refused = match ask_alone(
                connect,
                "leave.withdraw",
                json!({"writer": writer, "session": leave_session}),
                bounds.quick,
            ) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::LeaveWithdrawn { session, refused }
        }
        Command::ReadNotifications(reading) => {
            let answer = ask_alone(
                connect,
                "notifications.read",
                reading.params(),
                bounds.quick,
            );
            Outcome::NotificationsRead {
                failed: !matches!(answer, Ok(Ok(_))),
            }
        }
        Command::ResumeFollowing {
            session,
            subscription,
        } => {
            let refused = match ask_alone(
                connect,
                "subscription.resume",
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

/// The appearance in the words of `appearance.show`.
pub(crate) fn appearance_json(appearance: gpui_design_system::Appearance) -> Value {
    use gpui_design_system::Source;
    use gpui_design_system::theme::Shade;
    let theme = match appearance.source {
        Source::Desktop(id) | Source::Theme(id) => id.to_string(),
    };
    let shade = match appearance.shade {
        Shade::Light => "light",
        Shade::Dark => "dark",
    };
    json!({"theme": theme, "shade": shade})
}

/// What `appearance.show` says, or why it says nothing the window knows.
pub fn appearance_of(shown: &Value) -> Result<gpui_design_system::Appearance, String> {
    use gpui_design_system::theme::Shade;
    use gpui_design_system::{Appearance, Source};
    let source = match shown["theme"].as_str() {
        Some(crate::OMARCHY) => Source::Desktop(crate::OMARCHY.into()),
        Some(crate::OWN_THEME) => Source::Theme(crate::OWN_THEME.into()),
        other => return Err(format!("an unknown theme: {other:?}")),
    };
    let shade = match shown["shade"].as_str() {
        Some("light") => Shade::Light,
        Some("dark") => Shade::Dark,
        other => return Err(format!("an unknown shade: {other:?}")),
    };
    Ok(Appearance { source, shade })
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

fn start_params(
    project: &str,
    workspace: Option<&str>,
    pick: &Pick,
    message: &str,
    mode: &str,
) -> Value {
    // What is left out is sent as a null, which the daemon reads as absent: its to choose.
    json!({
        "project": project,
        "workspace": workspace,
        "provider": pick.provider,
        "model": pick.model,
        "effort": pick.effort,
        "gate_mode": mode,
        "message": message,
    })
}

fn started_outcome(project: &str, message: &str, started: &Value) -> Outcome {
    let title = match started["agent"]["title"].as_str() {
        Some(title) if !title.trim().is_empty() => title.to_owned(),
        _ => message
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned(),
    };
    match started["agent"]["id"].as_str() {
        Some(agent) => Outcome::Started {
            project: project.to_owned(),
            agent: agent.to_owned(),
            title,
        },
        None => Outcome::NotStarted {
            project: project.to_owned(),
            why: "Flottille did not say which agent it started".into(),
        },
    }
}

/// Reads the branches of the project's repositories and the lines changed in its workspaces.
/// `None` when the daemon cannot be asked: the page keeps what it showed.
/// The tickets read, by project and workspace, with when: a tracker is asked again only once a
/// read is [`TICKET_FRESH`] no more, or the workspace is linked to another ticket.
type Tickets = HashMap<(String, String), (TicketFacts, Instant)>;

/// The pull requests each workspace's forge last gave: a page read again shows them while it asks
/// again.
type Pulls = HashMap<(String, String), Vec<PullRequest>>;

fn details(
    connect: &Connect,
    conn: &mut Option<Conn>,
    (kept, kept_pulls): (&mut Tickets, &mut Pulls),
    project: &str,
    bound: Duration,
    partial: &mut dyn FnMut(&Details) -> bool,
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
    let mut pending_pulls = HashSet::new();
    let mut pending_tickets = HashSet::new();
    // The workspaces read, and the ticket each is linked to.
    let mut read: Vec<(String, Option<String>)> = Vec::new();
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
        let linked = shown["ticket"].as_str().map(str::to_owned);
        match &linked {
            None => {
                kept.remove(&key);
            }
            Some(linked) => {
                // A ticket read for another reference is not this one's.
                if kept
                    .get(&key)
                    .is_some_and(|(facts, _)| &facts.reference != linked)
                {
                    kept.remove(&key);
                }
                match kept.get(&key) {
                    Some((facts, _)) => {
                        tickets.insert(name.to_owned(), facts.clone());
                    }
                    None => {
                        pending_tickets.insert(name.to_owned());
                    }
                }
            }
        }
        match kept_pulls.get(&key) {
            Some(known) => {
                pulls.insert(name.to_owned(), known.clone());
            }
            None => {
                pending_pulls.insert(name.to_owned());
            }
        }
        read.push((name.to_owned(), linked));
    }
    kept_pulls.retain(|(kept_project, name), _| {
        kept_project != project || read.iter().any(|(read, _)| read == name)
    });
    let mut details = Details {
        project: project.to_owned(),
        branches,
        changes,
        pulls,
        statuses,
        tickets,
        pending_pulls,
        pending_tickets,
    };
    // The workspaces' facts that git knows are shown while the forge and the tracker are asked.
    if !details.pending_pulls.is_empty() || !details.pending_tickets.is_empty() {
        partial(&details);
    }
    for (name, linked) in read {
        let key = (project.to_owned(), name.clone());
        match ask(
            connect,
            conn,
            "workspace.forge",
            json!({"project": project, "name": name}),
            bound,
        ) {
            Ok(Ok(forge)) => {
                let found = pull_requests(&forge).0;
                kept_pulls.insert(key.clone(), found.clone());
                details.pulls.insert(name.clone(), found);
            }
            // A forge that cannot be read is answered all the same: its card does not wait for
            // it again at each round.
            _ => {
                kept_pulls.entry(key.clone()).or_default();
            }
        }
        details.pending_pulls.remove(&name);
        if let Some(linked) = linked {
            let stale = kept
                .get(&key)
                .is_none_or(|(_, at)| at.elapsed() > TICKET_FRESH);
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
                    reference: linked.clone(),
                    state: None,
                    title: None,
                    url: None,
                    problem: Some("the ticket could not be read".into()),
                });
                kept.insert(key.clone(), (facts, Instant::now()));
            }
            if let Some((facts, _)) = kept.get(&key) {
                details.tickets.insert(name.clone(), facts.clone());
            }
        }
        details.pending_tickets.remove(&name);
        // The last workspace's answer is the final snapshot the caller sends.
        if !details.pending_pulls.is_empty() || !details.pending_tickets.is_empty() {
            partial(&details);
        }
    }
    Some(details)
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
                    // A daemon from before the refusal carried data: `needed` and a sentence only.
                    let answer = match method {
                        "service.stop" => json!({"id": request["id"], "error": {
                            "code": "needed", "message": "db is needed by app's api"}}),
                        _ => json!({"id": request["id"], "result": result}),
                    };
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
            images: Vec::new(),
            texts: Vec::new(),
            mode: "auto".into(),
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
                agent: "made-up-agent".into(),
                title: "Fix the timeout".into()
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
        assert_eq!(why, "Flottille did not answer agent.start within 200 ms");
    }

    #[test]
    fn a_stop_refused_as_needed_without_data_is_said_by_the_refusal() {
        let (sender, mut updates) = unbounded();
        let actions = Actions::spawn(daemon(&[]), sender);
        actions.send(Command::ServiceAct {
            name: "db".into(),
            start: false,
            force: false,
        });

        let Some(Outcome::ServiceActed { answer, needed, .. }) =
            next(&mut updates, Duration::from_secs(5))
        else {
            panic!("no answer of the window's own");
        };
        // No empty list to ask a confirmation about: the toast says what the daemon said.
        assert_eq!(needed, None);
        assert_eq!(answer, Err("db is needed by app's api".to_owned()));
    }

    /// A daemon on socket pairs that keeps every request, answers `agent.send` with a delivery,
    /// and refuses the chunks of the picture `refused`.
    fn recording(refused: Option<u64>) -> (Connect, Arc<std::sync::Mutex<Vec<Value>>>) {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let keeping = asked.clone();
        let connect: Connect = Arc::new(move || {
            let (window, daemon) = UnixStream::pair()?;
            let asked = keeping.clone();
            std::thread::spawn(move || {
                let mut writer = daemon.try_clone().unwrap();
                for line in BufReader::new(daemon).lines() {
                    let Ok(line) = line else { return };
                    let request: Value = serde_json::from_str(&line).unwrap();
                    asked.lock().unwrap().push(request.clone());
                    let answer = match request["method"].as_str().unwrap() {
                        "agent.upload"
                            if Some(request["params"]["file"].as_u64().unwrap()) == refused =>
                        {
                            json!({"id": request["id"], "error": {"code": "refused",
                                "message": "picture 2 is empty or over 5 MiB"}})
                        }
                        "agent.send" => json!({"id": request["id"],
                            "result": {"delivery": "d1", "after": 3}}),
                        _ => json!({"id": request["id"], "result": {}}),
                    };
                    writeln!(writer, "{answer}").unwrap();
                }
            });
            Ok(window)
        });
        (connect, asked)
    }

    fn reply_with(images: Vec<Picture>) -> Command {
        Command::Reply {
            session: "s1".into(),
            key: 1,
            message: "look".into(),
            images,
            texts: Vec::new(),
            compact: None,
        }
    }

    fn photo(size: usize) -> Picture {
        Picture {
            mime: "image/jpeg".into(),
            bytes: (0..size).map(|n| (n % 251) as u8).collect(),
        }
    }

    #[test]
    fn a_picture_larger_than_a_frame_goes_in_chunks_before_its_message() {
        let (connect, asked) = recording(None);
        let (sender, mut updates) = unbounded();
        let actions = Actions::spawn(connect, sender);
        let big = photo(3 * 1024 * 1024);
        actions.send(reply_with(vec![big.clone()]));

        let Some(Outcome::Replied { answer, .. }) = next(&mut updates, Duration::from_secs(10))
        else {
            panic!("no answer");
        };
        assert!(answer.is_ok(), "{answer:?}");
        let asked = asked.lock().unwrap();
        let (chunks, sent) = asked.split_at(asked.len() - 1);
        assert_eq!(chunks.len(), 6, "3 MiB in chunks of 512 KiB");
        let mut arrived = Vec::new();
        for chunk in chunks {
            use base64::Engine as _;
            assert_eq!(chunk["method"], "agent.upload");
            assert_eq!(chunk["params"]["upload"], sent[0]["params"]["upload"]);
            arrived.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(chunk["params"]["data"].as_str().unwrap())
                    .unwrap(),
            );
        }
        assert!(arrived == big.bytes);
        assert_eq!(sent[0]["method"], "agent.send");
        assert!(sent[0]["params"].get("images").is_none(), "{}", sent[0]);
    }

    #[test]
    fn a_small_picture_goes_with_its_message() {
        let (connect, asked) = recording(None);
        let (sender, mut updates) = unbounded();
        let actions = Actions::spawn(connect, sender);
        actions.send(reply_with(vec![photo(1000)]));

        assert!(next(&mut updates, Duration::from_secs(5)).is_some());
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0]["params"]["images"][0]["mime"], "image/jpeg");
    }

    #[test]
    fn a_chunk_refused_is_the_message_s_refusal() {
        let (connect, asked) = recording(Some(2));
        let (sender, mut updates) = unbounded();
        let actions = Actions::spawn(connect, sender);
        actions.send(reply_with(vec![photo(600_000), photo(600_000)]));

        let Some(Outcome::Replied { answer, .. }) = next(&mut updates, Duration::from_secs(10))
        else {
            panic!("no answer");
        };
        // Said in the composer as a local refusal is: which picture, and why.
        assert_eq!(
            answer.err().as_deref(),
            Some("picture 2 is empty or over 5 MiB")
        );
        let asked = asked.lock().unwrap();
        assert!(
            asked
                .iter()
                .all(|request| request["method"] != "agent.send"),
            "the message is never sent"
        );
    }

    #[test]
    fn words_that_do_not_fit_a_frame_are_refused_before_any_chunk() {
        let (connect, asked) = recording(None);
        let (sender, mut updates) = unbounded();
        let actions = Actions::spawn(connect, sender);
        let mut command = reply_with(vec![photo(1_200_000)]);
        if let Command::Reply { message, .. } = &mut command {
            *message = "w".repeat(2 * 1024 * 1024);
        }
        actions.send(command);

        let Some(Outcome::Replied { answer, .. }) = next(&mut updates, Duration::from_secs(10))
        else {
            panic!("no answer");
        };
        assert!(
            answer
                .err()
                .unwrap()
                .contains("its words take 1 MiB at most")
        );
        assert!(asked.lock().unwrap().is_empty(), "no chunk was sent");
    }

    #[test]
    fn a_restart_answered_without_a_new_run_is_a_refusal() {
        let answer = run_answer(RunAct::Restart, "r1", Ok(Ok(json!({"run": {}}))));

        assert_eq!(answer.restarted, None);
        assert!(answer.refused.is_some(), "{answer:?}");
    }

    #[test]
    fn a_restart_answer_carries_the_new_run_and_the_notice() {
        let answer = run_answer(
            RunAct::Restart,
            "r1",
            Ok(Ok(
                json!({"run": {"run": "r2"}, "notice": "agent a has ended"}),
            )),
        );

        assert_eq!(answer.restarted.as_deref(), Some("r2"));
        assert_eq!(answer.notice.as_deref(), Some("agent a has ended"));
        assert_eq!(answer.refused, None);
    }
}
