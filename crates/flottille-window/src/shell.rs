//! The window's frame: the top bar (the mark, search, the bell), the left tree, and the main area
//! showing the open page. It follows the daemon through the [`link`](crate::link) and draws
//! again only the parts a change touches.

use crate::glyphs;
use crate::state::AgentState;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui_design_system::color::mix;
use gpui_design_system::motion::{ENTER, HOVER, Motion, RISE};
use gpui_design_system::{
    Badge, Button, Card, Drawer, Icon, Item, ListRow, Listing, Side, Tie, Toast, ToastStack, Type,
    dismiss_on_navigate, dismiss_top_overlay, enter, fade, forget_overlays, icon, keys_held,
    open_overlay, palette, presence, pressable, space, toast,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, ElementId, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, Modifiers, MouseButton, NavigationDirection,
    ParentElement, PathBuilder, PathStyle, Pixels, Render, SharedString,
    StatefulInteractiveElement, StrokeOptions, Styled, Subscription, Task, TestSupportExt as _,
    WeakEntity, Window, canvas, div, point, px,
};
use lyon_tessellation::{LineCap, LineJoin};

use crate::actions::{Actions, Command, Details, Outcome, Pick, Reading, RunFrom};
use crate::add_project::AddProject;
use crate::arrange::{Arrangement, Change, Listed};
use crate::composer::{Choice, Composer, PlaceChoice};
use crate::end_led::LedEnd;
use crate::extensions::Registry;
use crate::extensions::connection_request::ConnectionRequest;
use crate::extensions::connections::ConnectionsSection;
use crate::extensions::specs::SpecPage;
use crate::group_name::GroupName;
use crate::history::{History, Way};
use crate::kept::{DraftKey, Folds, Written, machine_key};
use crate::link::{self, Connect, Seen, Update};
use crate::machines::Machines;
use crate::machines_page::{MachinesSection, RemovalDialog};
use crate::modals::Modals;
use crate::model::{BellItem, Forged, LinkShown, News, Place, Preparation, Snapshot};
use crate::open::Opener;
use crate::page::{MainArea, Page};
use crate::project::{NewWorkspace, PageTab, PlaceData, ProjectData, SessionLine};
use crate::project_removal::ProjectRemoval;
use crate::project_settings::ProjectSettingsPage;
use crate::providers::Providers;
use crate::rename::RenameSession;
use crate::rules::RulesEditor;
use crate::session::{Chose, Follow, Opened, Reply, SessionPage};
use crate::session_link::PullRequest;
use crate::settings::{Reveal, SettingsPage};
use crate::tree::{Arranged, ArrangedGroup};
use crate::tree::{EndedNode, MachineNode, ProjectNode, SessionRow, Sidebar, node_key};
use crate::vault::VaultSection;
use crate::workspaces_folder::WorkspacesFolder;

pub mod switcher;
use switcher::Switcher;

/// Below this width the session page's rail becomes a drawer from the right.
pub const WIDE: f32 = 1240.;
/// Below this width the left tree becomes a drawer from the left.
pub const NARROW: f32 = 860.;
const SIDEBAR: f32 = 280.;
const SIDEBAR_MEDIUM: f32 = 260.;
/// How often the times ago are looked at; a row is drawn again only when its text changes.
const CLOCK: Duration = Duration::from_secs(30);
/// How often at most an input at the window is told to the daemon: well within the 2 minutes the
/// phone waits for (`crate::machines::Remote::presence`).
const PRESENCE_EVERY: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Three columns.
    Wide,
    /// The rail is a drawer.
    Medium,
    /// The rail and the tree are drawers.
    Narrow,
}

impl Layout {
    pub fn of(width: Pixels) -> Self {
        let width = f32::from(width);
        if width >= WIDE {
            Self::Wide
        } else if width >= NARROW {
            Self::Medium
        } else {
            Self::Narrow
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Project(String),
    Session(String),
    Settings,
    /// A project's own settings.
    ProjectSettings(String),
    /// A spec's page, in its project.
    Spec {
        project: String,
        spec: u64,
    },
    /// The services of the machine.
    Services,
    /// What is new since the version last run.
    WhatsNew,
}

/// Where a session of a project starts, and so which machine's page of it opens (#1266).
#[derive(Clone, Debug, PartialEq)]
enum Start {
    /// On this machine (`None`) or another.
    On(Option<String>),
    /// Several machines can, this one cannot: the start asks which, on the page of `page`.
    Ask {
        page: Option<String>,
        machines: Vec<String>,
    },
    /// This machine lacks the project and none of the row's machines is there to start it: it
    /// starts nowhere else, whatever another machine's project is called (#1439).
    Nowhere { machines: Vec<String> },
}

/// The links an agent shares in a row: the toast that says them is named by the agent and the
/// row's number, so that the next row is a toast of its own.
#[derive(Default)]
struct Sharing {
    row: u64,
    links: Vec<LinkShown>,
}

impl Sharing {
    /// The row ends: the next link starts another, and another toast.
    fn end(&mut self) {
        self.row += 1;
        self.links.clear();
    }
}

/// How a window starts again from the binary that replaced its own, given the page it shows
/// (`Shell::page_name`): false when there is no other file to start from.
pub type Reexec = Arc<dyn Fn(Option<String>) -> bool + Send + Sync>;

/// The page a name of [`Shell::page_name`] is, and the tab of Settings it names.
fn page_named(name: &str) -> Option<(Selection, Option<usize>)> {
    let (kind, rest) = name.split_once(':').unwrap_or((name, ""));
    Some(match kind {
        "session" => (Selection::Session(rest.to_owned()), None),
        "project" => (Selection::Project(rest.to_owned()), None),
        "settings" => (Selection::Settings, rest.parse().ok()),
        "project-settings" => (Selection::ProjectSettings(rest.to_owned()), None),
        "spec" => {
            let (spec, project) = rest.split_once(':')?;
            (
                Selection::Spec {
                    project: project.to_owned(),
                    spec: spec.parse().ok()?,
                },
                None,
            )
        }
        "services" => (Selection::Services, None),
        "whats-new" => (Selection::WhatsNew, None),
        _ => return None,
    })
}

/// A project with no settings file, by name and, when the projects are shown by machine, by the
/// machine's section.
type GoneKey = (String, Option<String>);

pub struct Shell {
    seen: Arc<Seen>,
    /// What "Restart Flottille" runs, off the UI's thread: `flottille daemon stop`, after which
    /// the link starts Flottille again.
    restart: Arc<dyn Fn() + Send + Sync>,
    /// This window's own build, and how it starts again from the binary that replaced it: once
    /// the daemon says another build, the window becomes that one, on the page it shows.
    build: Option<(String, Reexec)>,
    /// Set once the window asked to start again: it asks once.
    reexecuted: bool,
    /// The page to open once the daemon's first read is in: the one shown before a re-exec.
    reopen: Option<(Selection, Option<usize>)>,
    /// The Version section of Settings, General.
    version: Entity<crate::version::VersionSection>,
    snapshot: Snapshot,
    layout: Layout,
    selected: Option<Selection>,
    /// The pages shown in this window, for the mouse's back and forward buttons.
    history: History,
    /// Held by the window's root when nothing inside has the focus (a click anywhere gives it),
    /// so that Alt+Left and Alt+Right reach the root: a key goes to the focused element's path.
    focus: FocusHandle,
    top: Entity<TopBar>,
    sidebar: Entity<Sidebar>,
    main: Entity<MainArea>,
    left: Entity<Drawer>,
    bell: Entity<BellList>,
    /// The results of the user's actions, at the bottom right, above everything.
    toasts: Entity<ToastStack>,
    /// The dialog of an agent's request to write a connection, opened from its card.
    connection_request: Entity<ConnectionRequest>,
    /// The host of the dialogs, drawn over whatever page is shown.
    modals: Entity<Modals>,
    /// The requests for a vault entry whose dismissal was sent: their Dismiss waits for the
    /// daemon's answer.
    dismissing: HashSet<u64>,
    bell_open: bool,
    /// How far the bell's list is in view: it rises in and fades out, and stays drawn until then.
    bell_shown: Motion,
    projects: HashMap<String, Entity<ProjectNode>>,
    /// The projects' rows in a machine's section, by project then machine, while the projects are
    /// shown by machine.
    machine_projects: HashMap<(String, String), Entity<ProjectNode>>,
    /// The rows of this machine's projects with no settings file, by name (and by this machine's
    /// section when the projects are shown by machine): never in `projects` nor `machine_projects`,
    /// which are the projects' of any machine, so that one never takes the other's place.
    gone: HashMap<GoneKey, Entity<ProjectNode>>,
    /// The machines' sections, by name.
    sections: HashMap<String, Entity<MachineNode>>,
    /// The user's choice (`window.get`): the projects are shown by machine.
    by_machine: bool,
    /// Each shared project's machine the user chose its sessions start on, by the project's name:
    /// this machine by its name (`window.get`, #1266).
    starts: HashMap<String, String>,
    /// The rows this machine hides (`project.hidden`, #1526): the user's view of this machine
    /// alone, which hides a project from the sidebar and the pickers and nothing else, in the
    /// order they were hidden.
    hidden: Vec<String>,
    /// They are shown by machine now: the choice, and another machine to group by.
    grouped: bool,
    /// The "Ended" group at the bottom of the tree, made when a session first ends.
    ended: Option<Entity<EndedNode>>,
    /// What the daemon keeps of what the sidebar has folded: read once at opening, which makes
    /// the nodes as they were left, then what was last written. `None` until it is read: nothing
    /// is written before, which would replace it with the defaults.
    kept: Folds,
    kept_read: bool,
    /// What was last asked of the daemon to keep and it did not answer yet (or refused): the same
    /// state is not asked twice.
    kept_asked: Option<Folds>,
    /// How many writes of the sidebar's state in a row the daemon did not take, and the wait that
    /// ends with the next try.
    kept_failures: u32,
    kept_retry: Option<Task<()>>,
    /// The user's order and groups of projects: one key of the daemon's window state, apart from
    /// the sidebar's folds, written only when the user arranges (`arranged`), so that a window
    /// that only folds a project never writes over what another made. This is what the daemon
    /// held when it last said so (as the window opened, then with each change it kept).
    arrangement_kept: Arrangement,
    /// What the user did that the daemon has not kept yet, and whether it was asked: the daemon
    /// applies each to the arrangement it holds, which another window may have changed since this
    /// one read it. One is asked at a time, the oldest, and the next once it is answered.
    arrangement_queue: VecDeque<(Change, bool)>,
    /// How many asks in a row the daemon did not take, and the wait that ends with the next try.
    arrangement_failures: u32,
    arrangement_retry: Option<Task<()>>,
    arrangement_waiting: bool,
    /// The sessions the daemon kept a draft of when the window opened: their rows say it before
    /// their pages are opened.
    listed_drafts: HashSet<String>,
    /// What the composers hold went to the daemon already (the window closes once).
    closed: bool,
    sessions: HashMap<String, Entity<SessionRow>>,
    /// The pull request of each workspace (its machine, `None` for this one, its project and
    /// name) as a project page last read it: a session's row says it on its second line. The
    /// sidebar asks the forge for none of them.
    pulls: HashMap<(Option<String>, String, String), PullRequest>,
    /// The notifications the window has asked the daemon to mark read: it asks once for each,
    /// whatever the answer takes.
    marked: HashSet<u64>,
    /// What is new in each session since it was last opened, the last of it in words: an agent
    /// said something, reported, was woken, or changed a ticket or a pull request. A session's
    /// row says it with one dot, unless a state mark takes its place; they live as long as the
    /// window.
    news: HashMap<String, String>,
    /// The links each agent shared in a row, while the toast that says them is on screen: the
    /// toast counts them, and the agent's next words start a new row.
    sharing: HashMap<String, Sharing>,
    /// Each project's composer and new workspace form, made when its page first opens: a draft,
    /// a send and a name asked belong to their project.
    forms: HashMap<String, Forms>,
    actions: Actions,
    /// The open page's project and its work when its details were last asked for: they are asked
    /// again only when that work changed.
    details_of: Option<(String, Work)>,
    details_asked: usize,
    /// What a session page needs to follow its session.
    follow: Follow,
    /// The open session's page; it follows its session while it is open.
    session_page: Option<Entity<SessionPage>>,
    /// The agent the user chooses in the open session's page, for the history.
    _chose: Option<Subscription>,
    /// Each session's composer, made when its page first opens: a draft and a send belong to
    /// their session.
    replies: HashMap<String, Entity<Reply>>,
    /// Each session's dock of the asks that wait, kept like its composer: what the user chose
    /// and wrote on a card stays when the page is left and opened again.
    docks: HashMap<String, Entity<crate::ask_dock::AskDock>>,
    /// Each composer's changes, which the session's row says as a pencil while it is left.
    _drafts: Vec<Subscription>,
    /// The providers as the daemon last listed them, and the number of that read.
    providers: Providers,
    providers_read: u64,
    /// What the last session started from a composer started on, for this run of the window:
    /// a composer with no project default starts there.
    last_pick: Option<Pick>,
    settings: Entity<SettingsPage>,
    /// The services page, one for the window's life.
    services: Entity<crate::services::ServicesPage>,
    /// What's new, one page for the window's life.
    whats_new: Entity<crate::whats_new::WhatsNewPage>,
    /// The editor of the user's rules, on the settings page.
    rules: Entity<RulesEditor>,
    /// The notice of the Auto judge Flottille picked was dismissed here, before the daemon says
    /// it was seen.
    judge_noticed: bool,
    /// The compactor Flottille picked has not been seen yet (`compaction.show`).
    compaction_picked: bool,
    /// The user saw the notice of the compactor picked.
    compaction_noticed: bool,
    /// The compactor's provider and model, as `compaction.show` last said them.
    compactor_shown: Option<(String, Option<String>)>,
    /// The last texts of the judge's and the compactor's notices, drawn while they fold away.
    notices: [Option<String>; 2],
    /// The number of the latest request about the appearance (see [`Shell::ask_appearance`]).
    appearance_asked: u64,
    /// Whether the texts the pages edit are to be read again when the link next reads the shell:
    /// an edit made elsewhere while the link did not listen is no event of its.
    texts_due: bool,
    /// The open project's settings page; made again for another project.
    project_settings: Option<Entity<ProjectSettingsPage>>,
    /// The form that adds a project, one for the window's life.
    add_project: Entity<AddProject>,
    /// The dialog that renames a session.
    rename: Entity<RenameSession>,
    /// The dialog that makes a group of projects.
    group_form: Entity<GroupName>,
    /// The user's order and groups of projects.
    arrangement: Arrangement,
    /// The quick switcher (Ctrl K): sessions, projects and actions found by what is typed.
    switcher: Entity<Switcher>,
    /// What the window knows of the parts of Flottille it draws in its places (Specs).
    extensions: Entity<Registry>,
    /// The open spec's page; made again for another spec.
    spec_page: Option<Entity<SpecPage>>,
    /// The user's machines, read from this machine whichever one is shown.
    remote: crate::machines::Remote,
    machines: Entity<MachinesSection>,
    /// What every machine's Settings shares.
    shared: Shared,
    /// This machine's daemon, and where every connection's updates go.
    local: Connect,
    sender: UnboundedSender<Update>,
    /// The machine of the page shown, when it is not this one: what the pages' actions, its
    /// Settings and its providers are now ([`Pages`]). Nothing switches with it: every machine's
    /// connections stay open, and the list is this machine's (#1271).
    front: Option<String>,
    /// How this window signs what only the user's device may ask, when it was given its own: every
    /// machine's actions sign with it.
    sign: Option<crate::link::Sign>,
    /// The device key, read once for the window: it signs every request to another machine.
    key: Option<Arc<flottille_link::DeviceKey>>,
    /// What the pages of each machine not in front hold, kept as it was: brought back at once.
    sides: HashMap<Option<String>, Pages>,
    /// What was read of each other machine whose project pages were shown, kept current by its
    /// own link: a project's page there reads that machine's projects and sessions.
    copies: HashMap<String, Copied>,
    /// Each other machine's bell link and what it last read of that machine's notification
    /// centre: the bell holds every machine's (#1280).
    bells: HashMap<String, Bell>,
    /// The device key read for the bells, while it is read.
    key_reading: Option<Task<()>>,
    /// Whether the device key could not be read for the bells, which was said once.
    key_failed: bool,
    /// The sessions that ended on each other machine, as it last answered (#1280).
    ended_there: HashMap<String, Vec<crate::model::Session>>,
    /// Why a machine's ended sessions are not listed, by machine: "Ended" says it once.
    unlisted: std::collections::BTreeMap<String, String>,
    /// The notifications of other machines asked to be marked read, once each.
    marked_there: HashSet<(String, u64)>,
    /// How many connections each other machine's bell links made, and how many reads of its
    /// ended sessions were started: what a test counts of what is asked there.
    asked_there: HashMap<String, (Arc<std::sync::atomic::AtomicUsize>, usize)>,
    /// The pages of the sessions last shown, the one shown and the one left before it on each
    /// machine, still following their session: coming back shows it at once.
    kept_pages: HashMap<String, Entity<SessionPage>>,
    /// Whether the machine of the page shown said anything since it was shown, and the wait past
    /// which its page says it does not answer.
    heard: bool,
    silent: Option<Task<()>>,
    /// The front machine's Settings, Features and connections were read once.
    pages_read: bool,
    /// What a page of another machine reads before anything of it was.
    nothing: Snapshot,
    /// The machine in front when the page was last shown, and the tab of its Settings then.
    selected_on: Option<(Option<String>, usize)>,
    /// The pages in the shell's fields are not those of the page shown: a machine's that is not
    /// in front, taking what it was answered ([`Shell::on_pages`]).
    aside: bool,
    /// The rows of other machines' sessions whose close was asked: gone from the list at once.
    closing: HashSet<String>,
    /// The daemon's last read as it gave it, before the sessions on other machines are added
    /// under their pilots.
    read: Option<Snapshot>,
    /// The machine chosen while its key is read.
    choosing: Option<Task<()>>,
    /// The machine whose answer is being taken (`None`: this one's): what it answered goes to its
    /// own pages, whichever machine's are shown.
    answering: Option<String>,
    /// The sessions started from another machine's project page, until that machine lists them:
    /// their rows, and so their pages, are there at once.
    started_there: Vec<(String, crate::machines::Listed)>,
    /// The link's updates, until a task takes them.
    updates: Option<UnboundedReceiver<Update>>,
    _link: Task<()>,
    _clock: Task<()>,
    /// What this window last told of the user (`client.presence`): whether it was focused, and
    /// when it last told an input, which is told again past `PRESENCE_EVERY` only.
    told_focus: Option<bool>,
    told_input: Option<std::time::Instant>,
}

type Forms = (Entity<Composer>, Entity<NewWorkspace>);

/// What the pages of one machine hold (#1271): the actions they ask it, the way its sessions
/// are followed, its Settings, the parts of Flottille it runs, its providers, and the forms and
/// pages of its projects, whose names another machine may share. The machine in front has its
/// own in the shell's fields; every other one's waits in `Shell::sides`, as it was left.
struct Pages {
    actions: Actions,
    follow: Follow,
    settings: Entity<SettingsPage>,
    rules: Entity<RulesEditor>,
    extensions: Entity<Registry>,
    providers: Providers,
    providers_read: u64,
    forms: HashMap<String, Forms>,
    project_settings: Option<Entity<ProjectSettingsPage>>,
    spec_page: Option<Entity<SpecPage>>,
    details_of: Option<(String, Work)>,
    /// Its Settings, its Features and its connections were read once.
    read: bool,
}

/// Another machine's bell link (#1280): its connection, which the bell's reads and "Ended" use
/// too, and what it last read of that machine's notification centre.
struct Bell {
    connect: link::Connect,
    /// Dropped, its link ends.
    _end: link::End,
    read: Option<Snapshot>,
    /// Whether its ended sessions are being read: one read at a time.
    ending: bool,
}

/// What "Ended" says of a runner that is off.
fn off_line(machine: &crate::machines::Machine) -> String {
    off_words(&machine.name, &machine.state_words())
}

fn off_words(machine: &str, state: &str) -> String {
    format!("{machine} is {state}: its ended sessions are not listed")
}

/// What was read of another machine for its projects' pages, by a link of its own.
struct Copied {
    _seen: Arc<Seen>,
    /// Dropped, its link sees nobody takes its updates any more, and ends.
    _end: link::End,
    read: Option<Snapshot>,
}

/// What a page asked to show, read of its machine: dropped once that machine's pages are no
/// longer in front, since another machine's page of the same name may be shown.
fn of_a_page(outcome: &Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Details(_)
            | Outcome::ProjectSettings { .. }
            | Outcome::Convention { .. }
            | Outcome::CommitChecks { .. }
            | Outcome::MessageChecks { .. }
            | Outcome::Repositories { .. }
            | Outcome::Programs { .. }
            | Outcome::RemovalRead { .. }
            | Outcome::TicketPreview { .. }
            | Outcome::Commands { .. }
            | Outcome::CommandRead { .. }
            | Outcome::Skills { .. }
            | Outcome::SkillRead { .. }
            | Outcome::ClaudeFound { .. }
            | Outcome::Tasks { .. }
            | Outcome::Items { .. }
            | Outcome::Progress { .. }
            | Outcome::Item { .. }
            | Outcome::ProjectRuns { .. }
            | Outcome::Services { .. }
            | Outcome::ServiceOutput { .. }
            | Outcome::Setup { .. }
            | Outcome::Rules {
                project: Some(_),
                ..
            }
            | Outcome::WorkspacesFolder {
                project: Some(_),
                ..
            }
    )
}

/// How long the row of a session started on another machine stands for it, in ms, while that
/// machine does not list it: its lists come through this machine's daemon within seconds.
const STARTED_LISTED_WITHIN: u64 = 60_000;

/// What this machine's link and actions say that fills this machine's pages: taken with them
/// in front, wherever they wait, while another machine's are shown.
fn of_this_machine_s_pages(update: &Update) -> bool {
    matches!(
        update,
        Update::Done(_)
            | Update::Providers(..)
            | Update::TextChanged { .. }
            | Update::AboutChanged
            | Update::SharedChanged
            | Update::DeveloperChanged
            | Update::FindingsChanged
            | Update::ToolResultsChanged
            | Update::CompactionChanged
            | Update::SessionsChanged
            | Update::VaultChanged
            | Update::ProgramSignIn { .. }
            | Update::SignIn { .. }
            | Update::WorkspacesFolderChanged { .. }
            | Update::Extensions
            | Update::ServicesChanged
            | Update::Items { .. }
    )
}

/// What every machine's Settings shares: this machine's own sections and the window's dialogs.
struct Shared {
    modals: Entity<Modals>,
    machines: Entity<MachinesSection>,
    version: Entity<crate::version::VersionSection>,
}

/// A machine's Settings, and its rules' editor: its sections hold what that machine said, and
/// never what another one did (#1271). Their dialogs open through the window's host.
fn settings_of(
    this: &WeakEntity<Shell>,
    machine: Option<&str>,
    shared: &Shared,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> (Entity<SettingsPage>, Entity<RulesEditor>) {
    let modals = &shared.modals;
    // Every section and dialog of these Settings asks their machine, whichever is in front when
    // it does (a dialog left open, a toast's action, a timer).
    let asks = crate::commands::ShellOf::on(this.clone(), machine.map(str::to_owned));
    let rules = cx.new(|_| RulesEditor::new(asks.clone(), None));
    let about = cx.new(|_| crate::about::AboutEditor::new(asks.clone()));
    let judge_guidance = cx.new(|_| RulesEditor::judge(asks.clone()));
    let compaction_guidance = cx.new(|_| RulesEditor::compaction(asks.clone()));
    let workspaces = cx.new(|_| WorkspacesFolder::new(asks.clone(), None));
    let travelled = cx.new(|_| crate::travelled_folder::TravelledFolder::new(asks.clone()));
    let vault = cx.new(|_| VaultSection::new(asks.clone(), modals.clone()));
    let sign_in = cx.new(|_| crate::sign_in::SignInSection::new(asks.clone(), modals.clone()));
    let hidden = cx.new(|_| crate::hidden_projects::HiddenProjects::new(asks.clone()));
    let connections =
        cx.new(|_| ConnectionsSection::new(asks.clone(), vault.clone(), modals.clone()));
    let filing = cx.new(|cx| crate::findings::Filing::new(asks.clone(), window, cx));
    let check = cx.new(|_| crate::check::CheckSection::new(asks.clone()));
    let all_settings = cx.new(|_| crate::all_settings::AllSettings::new(asks.clone()));
    let settings = cx.new(|cx| {
        // Its blocks' marks and the sections of All machines follow what it read.
        cx.observe(&all_settings, |_, _, cx| cx.notify()).detach();
        // A provider's row draws its sign-in from the section: it follows the section's list.
        cx.observe(&sign_in, |_, _, cx| cx.notify()).detach();
        SettingsPage::new(
            asks.clone(),
            rules.clone(),
            about.clone(),
            judge_guidance.clone(),
            compaction_guidance.clone(),
            workspaces.clone(),
            travelled.clone(),
            vault.clone(),
            connections.clone(),
            shared.machines.clone(),
            shared.version.clone(),
            sign_in.clone(),
            hidden,
            filing,
            check,
            all_settings,
        )
        .of_machine(machine.map(str::to_owned))
    });
    modals.update(cx, |modals, cx| {
        modals.add(
            crate::vault::MODAL,
            &vault,
            |vault, window, cx| vault.dialog(window, cx),
            VaultSection::is_open,
            VaultSection::focus_handle,
            cx,
        );
        modals.add(
            crate::sign_in::MODAL,
            &sign_in,
            |section, window, cx| section.dialog(window, cx),
            crate::sign_in::SignInSection::is_open,
            crate::sign_in::SignInSection::focus_handle,
            cx,
        );
        modals.add(
            crate::vault::SHARE_MODAL,
            &vault,
            |vault, window, cx| vault.share_dialog(window, cx),
            VaultSection::is_sharing,
            |_, _| None,
            cx,
        );
        modals.add(
            crate::extensions::connections::FORM_MODAL,
            &connections,
            |section, _, cx| section.form_dialog(cx),
            ConnectionsSection::form_is_open,
            ConnectionsSection::form_focus,
            cx,
        );
        modals.add(
            crate::extensions::connections::REVIEW_MODAL,
            &connections,
            |section, window, cx| section.review_dialog(window, cx),
            ConnectionsSection::review_is_open,
            |_, _| None,
            cx,
        );
        modals.add(
            crate::extensions::connections::SHARE_MODAL,
            &connections,
            |section, window, cx| section.share_dialog(window, cx),
            ConnectionsSection::is_sharing,
            |_, _| None,
            cx,
        );
    });
    (settings, rules)
}

/// What can change a project's details (its branches, its workspaces' lines): the project itself,
/// its workspaces and how far they are prepared, and its sessions' states (an agent changes files
/// while it works).
type Work = (
    crate::model::Project,
    Vec<(String, Preparation)>,
    Vec<(String, Option<AgentState>)>,
);

/// Whether what an agent showed is an address of the web, not a file.
fn is_address(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

/// What a row's dot says: `New since you looked: opened a pull request`.
fn unread_words(said: &str) -> String {
    let mut letters = said.chars();
    let first = letters.next().into_iter().flat_map(char::to_lowercase);
    format!(
        "New since you looked: {}{}",
        first.collect::<String>(),
        letters.as_str()
    )
}

pub fn now_ms() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    #[cfg(feature = "test-support")]
    let now = now + CLOCK_AHEAD_HERE.with(std::cell::Cell::get);
    now
}

#[cfg(feature = "test-support")]
thread_local! {
    static CLOCK_AHEAD_HERE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// For a scenario of the window: the clock the windows opened on this thread read runs `by` ahead
/// of the wall's, so that a time ago moves on without the scenario waiting for it. A thread of its
/// own, so that the scenarios that run beside it keep the wall's.
#[cfg(feature = "test-support")]
pub fn move_clock_on_this_thread(by: Duration) {
    CLOCK_AHEAD_HERE.with(|ahead| ahead.set(ahead.get() + by.as_millis() as u64));
}

impl Shell {
    /// The shell, its updates taken as the link sends them.
    pub fn new(
        connect: Connect,
        device: Option<crate::device::Device>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut shell = Self::new_polled_with(connect, device, window, cx);
        if let Some(mut updates) = shell.updates.take() {
            shell._link = cx.spawn_in(window, async move |this: WeakEntity<Shell>, cx| {
                while let Some(update) = updates.next().await {
                    if this
                        .update_in(cx, |shell, window, cx| shell.apply(update, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
        shell
    }

    /// The shell, its updates taken only by [`Shell::poll`]: on GPUI's test platform, another
    /// thread may not wake the UI's tasks.
    pub fn new_polled(connect: Connect, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_polled_with(connect, None, window, cx)
    }

    /// [`Shell::new_polled`], with the device key of this session's clients, which signs what
    /// the user does to the hub's list and each request to another machine.
    pub fn new_polled_with(
        connect: Connect,
        device: Option<crate::device::Device>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (sender, updates) = unbounded();
        // The machines are this machine's to say, and so is the link: the window's list is this
        // machine's, whichever machine a page shows.
        let remote = crate::machines::Remote::spawn(connect.clone(), device, sender.clone());
        let local = connect.clone();
        let seen = link::spawn_into(local.clone(), sender.clone());
        let follow = Follow {
            connect: local.clone(),
            updates: sender.clone().into(),
            opener: crate::open::system(),
        };
        let actions = Actions::spawn(local.clone(), sender.clone());
        let this = cx.entity().downgrade();
        cx.set_global(crate::tunnels::Tunnels::new(this.clone()));
        cx.set_global(crate::shown_files::ShownFiles::new(
            local.clone(),
            this.clone(),
        ));
        let top = cx.new(|_| TopBar::new(this.clone()));
        let sidebar = cx.new(|_| Sidebar::new(this.clone()));
        let modals = cx.new(|_| Modals::new());
        let version = cx.new(|_| crate::version::VersionSection::new(this.clone()));
        let machines = {
            let (modals, remote) = (modals.clone(), remote.clone());
            cx.new(|cx| {
                let mut section = MachinesSection::new(modals, remote);
                section.fields(window, cx);
                section
            })
        };
        let removal = {
            let machines = machines.clone();
            cx.new(|cx| RemovalDialog::new(machines, cx))
        };
        let shared = Shared {
            modals: modals.clone(),
            machines: machines.clone(),
            version: version.clone(),
        };
        let (settings, rules) = settings_of(&this, None, &shared, window, cx);
        {
            let reading = settings.clone();
            cx.defer(move |cx| {
                let connections = reading.read(cx).connections().clone();
                connections.update(cx, |section, cx| section.read(cx));
            });
        }
        let services = {
            let this = this.clone();
            // The services are this machine's (`docs/services.md`), whichever is in front.
            cx.new(|cx| crate::services::ServicesPage::new(this.into(), cx))
        };
        let whats_new = {
            let this = this.clone();
            cx.new(|_| crate::whats_new::WhatsNewPage::new(this))
        };
        let main = {
            let (this, settings, services) = (this.clone(), settings.clone(), services.clone());
            let whats_new = whats_new.clone();
            cx.new(|cx| MainArea::new(this, settings, services, whats_new, cx))
        };
        let left = cx.new(|_| Drawer::new("tree-drawer", Side::Left, px(SIDEBAR), sidebar.clone()));
        let bell = cx.new(|_| BellList::new(this.clone()));
        let toasts = cx.new(ToastStack::new);
        let connection_request = cx.new(|_| ConnectionRequest::new(this.clone().into()));
        // The dialogs of the window, in the order they are first drawn: each is the kit's modal,
        // and opens through the host over whichever page is shown.
        modals.update(cx, |modals, cx| {
            modals.add_view(
                crate::extensions::connection_request::MODAL,
                &connection_request,
                ConnectionRequest::is_open,
                cx,
            );
            modals.add(
                crate::machines_page::CONNECT_MODAL,
                &machines,
                MachinesSection::connect_dialog,
                MachinesSection::connect_is_open,
                MachinesSection::connect_focus,
                cx,
            );
            modals.add(
                crate::machines_page::RENAME_MODAL,
                &machines,
                MachinesSection::rename_dialog,
                MachinesSection::rename_is_open,
                MachinesSection::rename_focus,
                cx,
            );
            modals.add(
                crate::machines_page::LOOK_MODAL,
                &machines,
                MachinesSection::look_dialog,
                MachinesSection::look_is_open,
                |_, _| None,
                cx,
            );
            modals.add_view(
                crate::machines_page::REMOVAL_DIALOG,
                &removal,
                RemovalDialog::is_open,
                cx,
            );
        });
        let add_project = {
            let this = this.clone();
            cx.new(|cx| AddProject::new(this, window, cx))
        };
        let rename = {
            let this = this.clone();
            cx.new(|cx| RenameSession::new(this.into(), window, cx))
        };
        let group_form = {
            let this = this.clone();
            cx.new(|cx| GroupName::new(this, window, cx))
        };
        let focus = cx.focus_handle();
        modals.update(cx, |modals, _| modals.fallback(focus.clone()));
        let switcher = {
            let (this, focus) = (this.clone(), focus.clone());
            cx.new(|cx| Switcher::new(this, focus, window, cx))
        };
        let extensions = cx.new(|_| Registry::new(this.clone().into()));
        let reading = extensions.clone();
        cx.defer(move |cx| reading.update(cx, |registry, cx| registry.read(cx)));
        // What they list shows in many places: each is drawn again, and the page open is shown
        // as it is now (a spec's page goes where Specs is turned off).
        cx.observe_in(&extensions, window, |this, _, window, cx| {
            this.extensions_moved(window, cx)
        })
        .detach();
        let _clock = cx.spawn(async move |this: WeakEntity<Shell>, cx| {
            loop {
                cx.background_executor().timer(CLOCK).await;
                if this.update(cx, |shell, cx| shell.tick(cx)).is_err() {
                    break;
                }
            }
        });
        cx.observe_window_bounds(window, |this, window, cx| this.relayout(false, window, cx))
            .detach();
        // Whether the user is at this window tells whether the phone is (`client.presence`).
        cx.observe_window_activation(window, |this, window, _| {
            this.tell_presence(window.is_window_active(), false)
        })
        .detach();
        let mut shell = Self {
            seen,
            restart: Arc::new(restart_flottille),
            build: None,
            reexecuted: false,
            reopen: None,
            version,
            snapshot: Snapshot::default(),
            layout: Layout::Wide,
            selected: None,
            history: History::default(),
            focus,
            top,
            sidebar,
            main,
            left,
            bell,
            toasts,
            connection_request,
            modals,
            dismissing: HashSet::new(),
            bell_open: false,
            bell_shown: Motion::settled(0.0),
            projects: HashMap::new(),
            machine_projects: HashMap::new(),
            gone: HashMap::new(),
            sections: HashMap::new(),
            by_machine: false,
            starts: HashMap::new(),
            hidden: Vec::new(),
            grouped: false,
            ended: None,
            kept: Folds::default(),
            kept_read: false,
            kept_asked: None,
            kept_failures: 0,
            kept_retry: None,
            arrangement_kept: Arrangement::default(),
            arrangement_queue: VecDeque::new(),
            arrangement_failures: 0,
            arrangement_retry: None,
            arrangement_waiting: false,
            listed_drafts: HashSet::new(),
            closed: false,
            sessions: HashMap::new(),
            pulls: HashMap::new(),
            marked: HashSet::new(),
            news: HashMap::new(),
            sharing: HashMap::new(),
            forms: HashMap::new(),
            actions,
            details_of: None,
            details_asked: 0,
            follow,
            session_page: None,
            _chose: None,
            replies: HashMap::new(),
            docks: HashMap::new(),
            // Whatever way the program ends (the window closed, the desktop's quit, a signal),
            // what the composers hold is with the daemon first.
            _drafts: vec![cx.on_app_quit(|shell, cx| {
                shell.closing(cx);
                async {}
            })],
            providers: Providers::default(),
            providers_read: 0,
            last_pick: None,
            settings,
            services,
            whats_new,
            rules,
            judge_noticed: false,
            compaction_picked: false,
            compaction_noticed: false,
            compactor_shown: None,
            notices: [None, None],
            appearance_asked: 0,
            texts_due: true,
            project_settings: None,
            add_project,
            rename,
            group_form,
            arrangement: Arrangement::default(),
            switcher,
            extensions,
            spec_page: None,
            remote,
            machines,
            shared,
            local,
            sender,
            front: None,
            sign: None,
            key: None,
            sides: HashMap::new(),
            copies: HashMap::new(),
            bells: HashMap::new(),
            key_reading: None,
            key_failed: false,
            ended_there: HashMap::new(),
            unlisted: std::collections::BTreeMap::new(),
            marked_there: HashSet::new(),
            asked_there: HashMap::new(),
            kept_pages: HashMap::new(),
            heard: true,
            silent: None,
            pages_read: true,
            nothing: Snapshot::default(),
            selected_on: None,
            aside: false,
            closing: HashSet::new(),
            read: None,
            choosing: None,
            answering: None,
            started_there: Vec::new(),
            updates: Some(updates),
            _link: Task::ready(()),
            _clock,
            told_focus: None,
            told_input: None,
        };
        shell.relayout(true, window, cx);
        // The colours the user chose, kept by the daemon for every window.
        shell.ask_appearance(None);
        // Given once the window is built: a root built after would take it back.
        cx.defer_in(window, |this, window, cx| window.focus(&this.focus, cx));
        shell
    }

    /// Tells this machine's daemon whether the user is at this window, and that they gave it an
    /// input: the phone is told only while the user is away (`docs/remote.md`,
    /// "Notifications"). An input is told at most every `PRESENCE_EVERY`, the focus at once.
    fn tell_presence(&mut self, focused: bool, input: bool) {
        let now = std::time::Instant::now();
        let same = self.told_focus == Some(focused);
        if input {
            let recent = self
                .told_input
                .is_some_and(|at| now.saturating_duration_since(at) < PRESENCE_EVERY);
            if same && recent {
                return;
            }
            self.told_input = Some(now);
        } else if same {
            return;
        }
        self.told_focus = Some(focused);
        self.remote.presence(focused, input);
    }

    /// What this machine's daemon said of the user's machines.
    fn machines_news(
        &mut self,
        news: crate::machines::News,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let read = matches!(news, crate::machines::News::Read(_));
        self.machines
            .update(cx, |section, cx| section.news(news, window, cx));
        self.say_held(cx);
        self.forget_gone_machines(window, cx);
        self.follow_bells(window, cx);
        if let Some(lists) = self.machines.read(cx).machines().map(|read| &read.lists) {
            // Listed by its machine, or never in a while: its row is the machine's, or none.
            let now = now_ms();
            self.started_there.retain(|(machine, started)| {
                now.saturating_sub(started.created_at) < STARTED_LISTED_WITHIN
                    && !lists.machines.iter().any(|(name, listed)| {
                        name == machine && listed.iter().any(|s| s.id == started.id)
                    })
            });
        }
        self.share_remote_agents(cx);
        // The sessions on other machines, or their state, may have changed: their rows follow.
        if read && let Some(last) = self.read.clone() {
            let merged = self.with_elsewhere(last, cx);
            if merged != self.snapshot {
                self.show(merged, window, cx);
            }
        }
    }

    /// What the pages read of the launches on other machines, less those the user is closing: a
    /// launch whose machine is off stays in the list until it is back, but leaves every row at once.
    fn share_remote_agents(&self, cx: &mut Context<Self>) {
        let Some(read) = self.machines.read(cx).machines() else {
            return;
        };
        let mut launched = crate::model::remote_agents(read);
        launched.retain(|remote| !self.closing.contains(&remote.key));
        if cx
            .try_global::<crate::model::RemoteAgents>()
            .is_none_or(|now| now.0 != launched)
        {
            cx.set_global(crate::model::RemoteAgents(launched));
            cx.refresh_windows();
        }
    }

    /// Lets go of what the window holds of each machine no longer among the user's (#1271): its
    /// pages and their dialogs, its copy and its link, the kept page of its session.
    fn forget_gone_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A read that failed lists no machine: none of them is gone for it.
        let Some(read) = self
            .machines
            .read(cx)
            .machines()
            .filter(|read| read.off.is_none())
        else {
            return;
        };
        let kept: Vec<String> = self
            .kept_pages
            .values()
            .filter_map(|page| page.read(cx).machine().map(str::to_owned))
            .collect();
        let mut gone: Vec<String> = self
            .sides
            .keys()
            .flatten()
            .chain(self.copies.keys())
            .chain(self.bells.keys())
            .chain(self.ended_there.keys())
            .chain(self.front.iter())
            .chain(kept.iter())
            .filter(|machine| read.machine(machine).is_none())
            .cloned()
            .collect();
        gone.sort();
        gone.dedup();
        for machine in gone {
            // Its page shown: this machine's pages come in front, on no page.
            if self.front.as_deref() == Some(machine.as_str()) {
                self.bring(None, window, cx);
                self.selected = None;
                self.session_page = None;
                self.project_settings = None;
                self.spec_page = None;
                self.main.update(cx, |main, cx| {
                    main.set_spec_page(None);
                    main.show(Page::Empty, None, None, None, cx)
                });
            }
            if let Some(pages) = self.sides.remove(&Some(machine.clone())) {
                let settings = pages.settings.read(cx);
                let owners = [
                    settings.vault().entity_id(),
                    settings.sign_in().entity_id(),
                    settings.connections().entity_id(),
                ];
                self.shared
                    .modals
                    .update(cx, |modals, cx| modals.forget(&owners, cx));
            }
            self.copies.remove(&machine);
            let rang = self.bells.remove(&machine).is_some();
            if self.ended_there.remove(&machine).is_some() || rang {
                self.ring(cx);
            }
            self.unlisted.remove(&machine);
            if self.connection_request.read(cx).machine() == Some(machine.as_str()) {
                self.connection_request
                    .update(cx, |dialog, cx| dialog.forget(cx));
            }
            self.kept_pages
                .retain(|_, page| page.read(cx).machine() != Some(machine.as_str()));
            self.started_there.retain(|(of, _)| *of != machine);
        }
    }

    /// `read`, this machine's, with the sessions this machine's agents started on others under
    /// their pilots and those of every other machine (`docs/remote.md`, "One list of every
    /// machine"): the list is this machine's, whichever machine a page shows.
    fn with_elsewhere(&self, read: Snapshot, cx: &App) -> Snapshot {
        let Some(machines) = self.machines.read(cx).machines() else {
            return read;
        };
        let mut merged = match self.started_there.is_empty() {
            true => read.with_elsewhere(machines).with_lists(machines, None),
            false => {
                let mut machines = machines.clone();
                for (machine, started) in &self.started_there {
                    if let Some((_, listed)) = machines
                        .lists
                        .machines
                        .iter_mut()
                        .find(|(name, _)| name == machine)
                        && !listed.iter().any(|session| session.id == started.id)
                    {
                        listed.push(started.clone());
                    }
                }
                read.with_elsewhere(&machines).with_lists(&machines, None)
            }
        };
        merged
            .elsewhere
            .retain(|session| !self.closing.contains(&session.id));
        // What ended on another machine, as it answered: a row of "Ended" only, never one its
        // list or a launch here already has.
        let mut machines: Vec<&String> = self.ended_there.keys().collect();
        machines.sort();
        for machine in machines {
            for session in &self.ended_there[machine] {
                if merged.elsewhere(&session.id).is_none() {
                    merged.elsewhere.push(session.clone());
                }
            }
        }
        merged
    }

    /// A bell link for each other machine that runs sessions and is online, made once its key is
    /// read: the bell holds what waits there too (#1280). One that goes off has its link ended, its
    /// notifications and its ended sessions taken out, and "Ended" says so once; it is followed
    /// again once it is back.
    fn follow_bells(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(read) = self
            .machines
            .read(cx)
            .machines()
            .filter(|read| read.off.is_none())
            .cloned()
        else {
            return;
        };
        let runners = || {
            read.machines
                .iter()
                .filter(|machine| machine.role.as_deref() == Some("runner"))
        };
        let followed =
            |name: &str| runners().any(|machine| machine.name == name && machine.online());
        let gone: Vec<String> = self
            .bells
            .keys()
            .filter(|name| !followed(name))
            .cloned()
            .collect();
        let mut changed = !gone.is_empty();
        for name in gone {
            self.bells.remove(&name);
            self.ended_there.remove(&name);
            self.marked_there.retain(|(machine, _)| *machine != name);
            if let Some(off) = read.machine(&name).filter(|machine| !machine.online()) {
                self.unlisted.insert(name, off_line(off));
            }
        }
        for machine in runners() {
            // Back: what it ended is read again once its bell reads it; still off, what it says
            // is kept current.
            let said = (!machine.online()).then(|| off_line(machine));
            match said {
                Some(said)
                    if self
                        .unlisted
                        .get(&machine.name)
                        .is_some_and(|was| *was != said) =>
                {
                    self.unlisted.insert(machine.name.clone(), said);
                    changed = true;
                }
                None if self.unlisted.remove(&machine.name).is_some() => changed = true,
                _ => {}
            }
        }
        if changed {
            self.say_unlisted(cx);
            self.show_merged(window, cx);
            self.ring(cx);
        }
        let wanted: Vec<String> = runners()
            .filter(|machine| machine.online())
            .map(|machine| machine.name.clone())
            .filter(|name| !self.bells.contains_key(name))
            .collect();
        if wanted.is_empty() {
            return;
        }
        let Some(key) = self.key.clone() else {
            if self.key_reading.is_none() {
                // Read once, off the UI thread, as a page of another machine reads it; not read,
                // it is said once and read again at the next news of the machines.
                let remote = self.remote.clone();
                self.key_reading = Some(cx.spawn_in(window, async move |this, cx| {
                    let key = cx.background_spawn(async move { remote.key() }).await;
                    this.update_in(cx, |shell, window, cx| {
                        shell.key_reading = None;
                        match key {
                            Ok(key) => {
                                shell.key.get_or_insert(Arc::new(key));
                                shell.follow_bells(window, cx);
                            }
                            Err(why) if !shell.key_failed => {
                                shell.key_failed = true;
                                gpui_design_system::toast(
                                    gpui_design_system::Toast::failure(
                                        "The bell cannot follow your other machines",
                                    )
                                    .detail(why),
                                    cx,
                                );
                            }
                            Err(_) => {}
                        }
                    })
                    .ok();
                }));
            }
            return;
        };
        for name in wanted {
            let aim = crate::machines::Aim::default();
            aim.set(Some((name.clone(), key.clone())));
            let aimed = aim.connect(self.local.clone());
            let dials = self.asked_there.entry(name.clone()).or_default().0.clone();
            let connect: link::Connect = Arc::new(move || {
                dials.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                aimed()
            });
            let (updates, end) = link::Updates::from(self.sender.clone()).of(&name).ending();
            link::spawn_bell_into(connect.clone(), updates);
            self.bells.insert(
                name,
                Bell {
                    connect,
                    _end: end,
                    read: None,
                    ending: false,
                },
            );
        }
    }

    /// Another machine's notification centre, as its bell link read it. Its first read asks it
    /// for its ended sessions too, so that "Ended" holds them before it is first opened.
    fn bell_read(&mut self, machine: String, read: Snapshot, cx: &mut Context<Self>) {
        let Some(bell) = self.bells.get_mut(&machine) else {
            return;
        };
        let first = bell.read.is_none();
        bell.read = Some(read);
        self.ring(cx);
        if first {
            self.ask_ended_of(&machine);
        }
    }

    /// "Ended" was opened: each other machine followed is asked for its ended sessions, unless
    /// it is being asked, and a runner that is off is said so, once (#1280).
    pub fn ended_opened(&mut self, cx: &mut Context<Self>) {
        let mut names: Vec<String> = self.bells.keys().cloned().collect();
        names.sort();
        for name in names {
            self.ask_ended_of(&name);
        }
        let off: Vec<(String, String)> = self
            .machines
            .read(cx)
            .machines()
            .into_iter()
            .flat_map(|read| &read.machines)
            .filter(|machine| machine.role.as_deref() == Some("runner") && !machine.online())
            .map(|machine| (machine.name.clone(), off_line(machine)))
            .collect();
        let mut changed = false;
        for (name, said) in off {
            if self.unlisted.get(&name) != Some(&said) {
                self.unlisted.insert(name, said);
                changed = true;
            }
        }
        if changed {
            self.say_unlisted(cx);
        }
    }

    /// Asks `machine` for its ended sessions, off the UI thread, on its bell link's connection.
    fn ask_ended_of(&mut self, machine: &str) {
        let Some(bell) = self.bells.get_mut(machine).filter(|bell| !bell.ending) else {
            return;
        };
        bell.ending = true;
        self.asked_there.entry(machine.to_owned()).or_default().1 += 1;
        let (connect, updates) = (
            bell.connect.clone(),
            link::Updates::from(self.sender.clone()).of(machine),
        );
        let spawned = std::thread::Builder::new()
            .name("flottille-ended".into())
            .spawn(move || {
                let read = link::read_ended(&connect, link::NOT_ANSWERING);
                let _ = updates.unbounded_send(Update::EndedThere(read));
            });
        if let Err(error) = spawned {
            eprintln!("flottille: the ended sessions of {machine} cannot be read: {error}");
        }
    }

    /// What `machine` answered of its ended sessions: its rows of "Ended", or why there are none.
    fn ended_read(
        &mut self,
        machine: String,
        read: Result<Snapshot, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bell) = self.bells.get_mut(&machine) else {
            return;
        };
        bell.ending = false;
        match read {
            Ok(read) => {
                let look = self
                    .machines
                    .read(cx)
                    .machines()
                    .and_then(|read| read.members.iter().find(|m| m.name == machine))
                    .map(crate::machines::Member::look_words);
                self.ended_there
                    .insert(machine.clone(), read.ended_of(&machine, look));
                self.unlisted.remove(&machine);
            }
            Err(why) => {
                self.ended_there.remove(&machine);
                let off = self
                    .machines
                    .read(cx)
                    .machines()
                    .and_then(|read| read.machine(&machine))
                    .filter(|found| !found.online())
                    .map(|found| found.state_words());
                let said = match off {
                    Some(state) => off_words(&machine, &state),
                    None => format!("{machine}'s ended sessions are not listed: {why}"),
                };
                self.unlisted.insert(machine.clone(), said);
            }
        }
        self.say_unlisted(cx);
        self.show_merged(window, cx);
        self.ring(cx);
    }

    /// Shows what was last read here merged with what other machines said, when that changed.
    fn show_merged(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(last) = self.read.clone() {
            let merged = self.with_elsewhere(last, cx);
            if merged != self.snapshot {
                self.show(merged, window, cx);
            }
        }
    }

    /// The machines whose ended sessions are not listed, in one line of "Ended".
    fn say_unlisted(&mut self, cx: &mut Context<Self>) {
        let said = (!self.unlisted.is_empty()).then(|| {
            self.unlisted
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ")
        });
        if let Some(group) = self.ended.clone()
            && group.read(cx).unlisted() != said.as_deref()
        {
            group.update(cx, |group, cx| group.set_unlisted(said, cx));
        }
    }

    /// The bell's items of every machine: this one's as they are, another's keyed and tied to its
    /// row by that machine's name (`n-3@server`, the session `a1@server`), with that machine.
    fn bell_items(&self) -> Vec<(Option<&str>, BellItem)> {
        let mut items: Vec<(Option<&str>, BellItem)> = self
            .snapshot
            .bell
            .iter()
            .map(|item| (None, item.clone()))
            .collect();
        let mut machines: Vec<&String> = self.bells.keys().collect();
        machines.sort();
        for machine in machines {
            let Some(read) = &self.bells[machine].read else {
                continue;
            };
            for item in &read.bell {
                let mut item = item.clone();
                item.key = format!("{}@{machine}", item.key);
                item.session = item
                    .session
                    .map(|agent| crate::model::elsewhere_id(machine, &agent));
                items.push((Some(machine.as_str()), item));
            }
        }
        items
    }

    /// Whether the bell and "Ended" have read `machine` once.
    pub fn bell_reached(&self, machine: &str) -> bool {
        self.bells
            .get(machine)
            .is_some_and(|bell| bell.read.is_some())
            && (self.ended_there.contains_key(machine) || self.unlisted.contains_key(machine))
    }

    /// How many connections the bell links of `machine` made.
    pub fn bell_dials(&self, machine: &str) -> usize {
        self.asked_there.get(machine).map_or(0, |(dials, _)| {
            dials.load(std::sync::atomic::Ordering::Relaxed)
        })
    }

    /// How many reads of the ended sessions of `machine` were started.
    pub fn ended_reads(&self, machine: &str) -> usize {
        self.asked_there.get(machine).map_or(0, |(_, reads)| *reads)
    }

    /// Asks `machine` to mark notifications read, on its bell link's connection; the link reads
    /// its centre again on the event that follows.
    /// `ids` are those it marks, put in `marked_there` until it says they were not.
    fn read_there(&mut self, machine: &str, reading: Reading, ids: Vec<u64>) {
        let Some(bell) = self.bells.get(machine) else {
            return;
        };
        let updates = link::Updates::from(self.sender.clone()).of(machine);
        link::read_there(bell.connect.clone(), reading.params(), ids.clone(), updates);
        self.marked_there
            .extend(ids.into_iter().map(|id| (machine.to_owned(), id)));
    }

    /// Opens the session `agent` of `machine`: its page is that machine's, read on its own
    /// connection, while the rest of the window stays as it is.
    pub fn open_elsewhere(
        &mut self,
        machine: String,
        agent: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = crate::model::elsewhere_id(&machine, &agent);
        self.open(Selection::Session(id), window, cx);
    }

    /// Opens the session `agent` of this machine.
    pub fn open_here(&mut self, agent: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open(Selection::Session(agent), window, cx);
    }

    /// The user confirmed the end of the session of the row `session`: one of another machine,
    /// of its list or launched there by an agent of this machine, is closed through this machine's
    /// daemon, which keeps the close while that machine is off (`docs/remote.md`, "One list of
    /// every machine"), and leaves the list at once; any other is stopped where it runs.
    pub fn end_confirmed(&mut self, session: String, window: &mut Window, cx: &mut Context<Self>) {
        let listed = self
            .snapshot
            .elsewhere(&session)
            .and_then(|found| found.elsewhere.clone())
            .filter(|there| !there.here);
        let Some(there) = listed else {
            self.perform_here(Command::Stop { session });
            return;
        };
        self.closing.insert(session);
        self.remote.close(there.machine, there.agent);
        self.share_remote_agents(cx);
        if let Some(last) = self.read.clone() {
            let merged = self.with_elsewhere(last, cx);
            self.show(merged, window, cx);
        }
    }

    /// This machine's daemon kept the close of `agent` of `machine`, or said why not: a refused
    /// close brings its row back.
    fn closed(
        &mut self,
        machine: String,
        agent: String,
        refused: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offline = self
            .machines
            .read(cx)
            .machines()
            .and_then(|read| read.machine(&machine))
            .filter(|found| !found.online())
            .map(|found| found.state_words());
        let toast = match (refused, offline) {
            (Some(why), _) => {
                self.closing
                    .remove(&crate::model::elsewhere_id(&machine, &agent));
                self.share_remote_agents(cx);
                if let Some(last) = self.read.clone() {
                    let merged = self.with_elsewhere(last, cx);
                    self.show(merged, window, cx);
                }
                gpui_design_system::Toast::failure("The session was not closed").detail(why)
            }
            (None, Some(state)) => gpui_design_system::Toast::success("Session closed").detail(
                format!("{machine} is {state}: it closes there when it is back"),
            ),
            (None, None) => gpui_design_system::Toast::success("Session closed"),
        };
        gpui_design_system::toast(toast, cx);
    }

    /// The Machines section of Settings.
    pub fn machines(&self) -> &Entity<MachinesSection> {
        &self.machines
    }

    /// Says to every page whether the machine in front takes changes now ([`crate::machines::Held`]),
    /// when that changed.
    fn say_held(&mut self, cx: &mut Context<Self>) {
        let holding =
            crate::machines::holding(self.front.as_deref(), self.machines.read(cx).machines());
        if crate::machines::held(cx) != holding {
            cx.set_global(crate::machines::Held(holding));
            cx.refresh_windows();
        }
    }

    /// The online machine among the homes of the row `row`, while this machine has no such
    /// project (#1439): a project of that name on a machine the row does not mark is another
    /// project, and is never reached from it.
    fn machine_of_project(&self, row: &str, cx: &App) -> Option<String> {
        let machines = self.machines.read(cx).machines()?;
        self.snapshot
            .projects
            .iter()
            .find(|project| project.name == row)?
            .homes
            .iter()
            .find(|home| !home.here && machines.machine(&home.machine).is_some_and(|m| m.online()))
            .map(|home| home.machine.clone())
    }

    /// The names of the machines the row `row` marks, in its order.
    fn homes_of(&self, row: &str) -> Vec<String> {
        self.snapshot
            .projects
            .iter()
            .find(|project| project.name == row)
            .map(|project| {
                project
                    .homes
                    .iter()
                    .map(|home| home.machine.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The name the machine `machine` (`None`: this one) has the project of the projects' row
    /// `row` by: the one the sidebar grouped in that row ([`crate::model::Project::name_on`]),
    /// `None` when the row does not mark that machine. A name no row bears (a project added a
    /// moment ago, not listed yet) is a project of this machine only.
    fn name_on(&self, row: &str, machine: Option<&str>) -> Option<String> {
        match self.snapshot.projects.iter().find(|p| p.name == row) {
            Some(project) => project.name_on(machine).map(str::to_owned),
            None => machine.is_none().then(|| row.to_owned()),
        }
    }

    /// The projects' row of the project the machine `machine` (`None`: this one) has by `name`;
    /// `name` when no row is.
    fn row_of(&self, machine: Option<&str>, name: &str) -> String {
        self.snapshot
            .projects
            .iter()
            .find(|project| project.name_on(machine) == Some(name))
            .map_or_else(|| name.to_owned(), |project| project.name.clone())
    }

    /// The machines the row of the project the machine in front has by `name` marks: those its
    /// switches offer (#1410).
    pub fn project_homes(&self, name: &str) -> Vec<crate::model::Home> {
        let row = self.row_of(self.front.as_deref(), name);
        self.snapshot
            .projects
            .iter()
            .find(|project| project.name == row)
            .map(|project| project.homes.clone())
            .unwrap_or_default()
    }

    /// The machine of the page shown, when it is not this one.
    pub fn machine_shown(&self) -> Option<String> {
        self.front.clone()
    }

    /// Shows Settings of the machine `name` (`None`: this one), every request signed with the
    /// device key. One that is off, or needs an update, is said so and not shown.
    pub fn choose_machine(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The name that machine has the page's project by; `None` when the row does not mark it.
        let there = |project: &str| {
            let row = self.row_of(self.front.as_deref(), project);
            self.name_on(&row, name.as_deref())
        };
        let selection = match &self.selected {
            Some(Selection::Settings) | None => Selection::Settings,
            // A project's page or settings: that machine's of the same project, by its name
            // there. A machine the row does not mark shows its Settings: a project of the same
            // name there is another project (#1439).
            Some(Selection::Project(project)) => there(project)
                .map(Selection::Project)
                .unwrap_or(Selection::Settings),
            Some(Selection::ProjectSettings(project)) => there(project)
                .map(Selection::ProjectSettings)
                .unwrap_or(Selection::Settings),
            // Another page: that machine's page of it.
            Some(selected) => selected.clone(),
        };
        self.reach(name, selection, window, cx);
    }

    /// Shows the page of `project` of the machine `name` (`None`: this one): the composer's
    /// picker chose where the session starts, and that machine's page of the project is where it
    /// starts. One that is off is said so and not shown, as `choose_machine` does.
    pub fn choose_machine_for(
        &mut self,
        name: Option<String>,
        project: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The choice is the project's from now on: its page opens there, and asks no more.
        let this = self
            .machines
            .read(cx)
            .machines()
            .and_then(|read| read.this.clone());
        let row = self.row_of(self.front.as_deref(), project);
        if let Some(chosen) = name.clone().or(this) {
            self.starts.insert(row.clone(), chosen);
            self.sidebar_changed(cx);
        }
        let there = self
            .name_on(&row, name.as_deref())
            .unwrap_or_else(|| project.to_owned());
        self.reach(name, Selection::Project(there), window, cx);
    }

    /// Where a session of `project` starts (#1266): on this machine when it has the project and
    /// can run an agent; else on the one machine online that can; when several can and this one
    /// cannot, on the one the user chose for it, while it still can, else the start asks which.
    fn start_on(&self, project: &str, cx: &App) -> Start {
        let own = self.read.as_ref().unwrap_or(&self.snapshot);
        let ours = |name: &str| own.projects.iter().any(|p| p.name == name);
        let Some(machines) = self.machines.read(cx).machines() else {
            return Start::On(None);
        };
        // Only the machines the row marks, each by the name the row holds of it (#1410).
        let here = self.name_on(project, None).is_some_and(|name| ours(&name));
        let can = |name: &str| {
            machines
                .machine(name)
                .is_some_and(|machine| machine.runs() && machine.online())
                && self.name_on(project, Some(name)).is_some()
        };
        if here && self.this_can_run(machines) {
            return Start::On(None);
        }
        let others: Vec<String> = machines
            .runners()
            .map(|machine| machine.name.clone())
            .filter(|name| can(name))
            .collect();
        match others.as_slice() {
            [] if here => Start::On(None),
            [] => match (self.machine_of_project(project, cx), self.homes_of(project)) {
                (Some(machine), _) => Start::On(Some(machine)),
                // A row that marks no machine is this one's alone.
                (None, homes) if homes.is_empty() => Start::On(None),
                (None, machines) => Start::Nowhere { machines },
            },
            [one] => Start::On(Some(one.clone())),
            // What the user answered when it was asked, while that machine still can.
            several => match self.starts.get(project) {
                Some(chosen) if several.contains(chosen) => Start::On(Some(chosen.clone())),
                Some(chosen) if here && Some(chosen) == machines.this.as_ref() => Start::On(None),
                _ => Start::Ask {
                    page: (!here).then(|| several[0].clone()),
                    machines: several.to_vec(),
                },
            },
        }
    }

    /// Whether this machine runs agents: its role lets it. Its providers are not looked at: the
    /// page that opens would change as they are read, and the composer says itself when none is
    /// ready.
    fn this_can_run(&self, machines: &crate::machines::Machines) -> bool {
        machines
            .this
            .as_deref()
            .and_then(|this| machines.members.iter().find(|member| member.name == this))
            .is_none_or(|member| member.role == "runner")
    }

    /// The machine of the page `selection` names: a session's row says it; a project is this
    /// machine's when it has one of that name; a project's settings and its specs are those of
    /// the page they were opened from, and Settings the machine chosen in it.
    fn machine_of(&self, selection: &Selection, cx: &App) -> Option<String> {
        match selection {
            Selection::Session(id) => self
                .snapshot
                .elsewhere(id)
                .and_then(|session| session.elsewhere.as_ref())
                .filter(|there| !there.here)
                .map(|there| there.machine.clone()),
            // The list has a group for a project another machine only has: this machine's own
            // read says whether it is this one's.
            Selection::Project(project) => match self.start_on(project, cx) {
                Start::On(machine) => machine,
                Start::Ask { page, .. } => page,
                // Nothing opens: `open` says so.
                Start::Nowhere { .. } => None,
            },
            Selection::ProjectSettings(_) | Selection::Spec { .. } | Selection::Settings => {
                self.front.clone()
            }
            Selection::Services | Selection::WhatsNew => None,
        }
    }

    /// Shows `selection`, a page of `machine` (`None`: this one). Nothing switches: the machine's
    /// pages are brought in front, at once once its key was read, and the page reads what it
    /// shows on that machine's own connection. One that is off is said so and not shown.
    fn reach(
        &mut self,
        machine: Option<String>,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = machine.clone();
        self.reach_then(machine, window, cx, move |shell, window, cx| {
            shell.bring(to, window, cx);
            shell.select(selection, window, cx);
        });
    }

    /// Runs `then` once `machine` (`None`: this one) can be asked: at once once its key was
    /// read, after the read otherwise. A choice still reading the key gives way to this one, and
    /// so does any page opened meanwhile ([`Shell::open`]). One that is off is said so, and
    /// `then` is not run.
    fn reach_then(
        &mut self,
        machine: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
    ) {
        self.choosing = None;
        let Some(name) = machine else {
            then(self, window, cx);
            return;
        };
        let read: Option<Machines> = self.machines.read(cx).machines().cloned();
        let found = read.as_ref().and_then(|read| read.machine(&name)).cloned();
        if let Some(found) = found.filter(|found| !found.online()) {
            let detail = found
                .reason
                .clone()
                .unwrap_or_else(|| format!("{} is {}", found.name, found.state_words()));
            gpui_design_system::toast(
                gpui_design_system::Toast::failure(format!("{name} cannot be shown"))
                    .detail(detail),
                cx,
            );
            return;
        }
        if self.key.is_some() {
            then(self, window, cx);
            return;
        }
        // Read once, off the UI thread: a locked keyring holds the read until it is unlocked.
        let remote = self.remote.clone();
        self.choosing = Some(cx.spawn_in(window, async move |this, cx| {
            let key = cx.background_spawn(async move { remote.key() }).await;
            this.update_in(cx, |shell, window, cx| match key {
                Ok(key) => {
                    shell.key = Some(Arc::new(key));
                    then(shell, window, cx);
                }
                Err(why) => gpui_design_system::toast(
                    gpui_design_system::Toast::failure(format!("{name} cannot be shown"))
                        .detail(why),
                    cx,
                ),
            })
            .ok();
        }));
    }

    /// The machine chosen in Add project: the project is added there, while the page behind
    /// stays, its own machine's (#1271). One that is off is said so and not chosen.
    pub fn choose_machine_to_add(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chosen = name.clone();
        self.reach_then(name, window, cx, move |shell, window, cx| {
            shell.ensure_pages(chosen.as_deref(), window, cx);
            shell
                .add_project
                .update(cx, |form, cx| form.set_machine(chosen, cx));
            shell.read_folders(cx);
        });
    }

    /// The pages of `machine`, made the first time, where they wait when it is not in front.
    fn ensure_pages(&mut self, machine: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let machine = machine.map(str::to_owned);
        if machine == self.front || self.sides.contains_key(&machine) {
            return;
        }
        let pages = self.pages_for(machine.as_deref(), window, cx);
        self.sides.insert(machine, pages);
    }

    /// Brings the pages of `machine` (`None`: this one) in front: its actions, its Settings, its
    /// providers and its projects' forms, made the first time (#1271). What the machine left in
    /// front held waits for it, as it was.
    fn bring(&mut self, machine: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if machine == self.front {
            return;
        }
        // Another machine shown, however it was chosen: All machines is chosen again to be shown.
        cx.set_global(crate::settings::SettingsOfAll(false));
        // The tab of the Settings left is the one the next machine's shows.
        if self.selected == Some(Selection::Settings) {
            let tab = self.settings.read(cx).tab();
            self.selected_on = Some((self.front.clone(), tab));
        }
        let pages = match self.sides.remove(&machine) {
            Some(pages) => pages,
            None => self.pages_for(machine.as_deref(), window, cx),
        };
        let left = self.swap_pages(pages);
        let was = std::mem::replace(&mut self.front, machine);
        self.sides.insert(was, left);
        cx.set_global(crate::machines::Shown(self.front.clone()));
        if cx.has_global::<crate::shown_files::ShownFiles>() {
            let connect = self.follow.connect.clone();
            cx.global_mut::<crate::shown_files::ShownFiles>()
                .reach_with(connect);
        }
        self.say_held(cx);
        let settings = self.settings.clone();
        self.main
            .update(cx, |main, cx| main.set_settings(settings, cx));
        self.listen(cx);
    }

    /// The pages of `machine`, made the first time it is shown: its own connections, signed
    /// with the device key, its own Settings. Nothing is read before a page asks.
    fn pages_for(
        &mut self,
        machine: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Pages {
        let this = cx.entity().downgrade();
        let updates = link::Updates::from(self.sender.clone());
        let (actions, follow) = match (machine, self.key.clone()) {
            (Some(name), Some(key)) => {
                let aim = crate::machines::Aim::default();
                aim.set(Some((name.to_owned(), key)));
                let updates = updates.of(name);
                let connect = aim.connect(self.local.clone());
                let mut actions = Actions::spawn_aimed(aim, self.local.clone(), updates.clone());
                if let Some(sign) = self.sign.clone() {
                    actions.sign_with(sign);
                }
                let follow = Follow {
                    connect,
                    updates,
                    opener: self.follow.opener.clone(),
                };
                (actions, follow)
            }
            _ => {
                let follow = Follow {
                    connect: self.local.clone(),
                    updates: updates.clone(),
                    opener: self.follow.opener.clone(),
                };
                let mut actions = Actions::spawn(self.local.clone(), updates);
                if let Some(sign) = self.sign.clone() {
                    actions.sign_with(sign);
                }
                (actions, follow)
            }
        };
        let (settings, rules) = settings_of(&this, machine, &self.shared, window, cx);
        let asks = crate::commands::ShellOf::on(this.clone(), machine.map(str::to_owned));
        let extensions = cx.new(|_| Registry::new(asks));
        cx.observe_in(&extensions, window, |this, _, window, cx| {
            this.extensions_moved(window, cx)
        })
        .detach();
        Pages {
            actions,
            follow,
            settings,
            rules,
            extensions,
            providers: Providers::default(),
            providers_read: 0,
            forms: HashMap::new(),
            project_settings: None,
            spec_page: None,
            details_of: None,
            read: false,
        }
    }

    /// Puts `pages` in front, and gives back what was.
    fn swap_pages(&mut self, mut pages: Pages) -> Pages {
        std::mem::swap(&mut self.actions, &mut pages.actions);
        std::mem::swap(&mut self.follow, &mut pages.follow);
        std::mem::swap(&mut self.settings, &mut pages.settings);
        std::mem::swap(&mut self.rules, &mut pages.rules);
        std::mem::swap(&mut self.extensions, &mut pages.extensions);
        std::mem::swap(&mut self.providers, &mut pages.providers);
        std::mem::swap(&mut self.providers_read, &mut pages.providers_read);
        std::mem::swap(&mut self.forms, &mut pages.forms);
        std::mem::swap(&mut self.project_settings, &mut pages.project_settings);
        std::mem::swap(&mut self.spec_page, &mut pages.spec_page);
        std::mem::swap(&mut self.details_of, &mut pages.details_of);
        std::mem::swap(&mut self.pages_read, &mut pages.read);
        pages
    }

    /// Runs `then` with the pages of `machine` in front, as if they were: what it answered
    /// fills its own Settings, never those of the machine shown. What its pages ask, now or
    /// later, goes to `machine`: each holds its machine ([`crate::commands::ShellOf`]).
    fn on_pages(
        &mut self,
        machine: Option<String>,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Context<Self>),
    ) {
        if machine != self.front && !self.sides.contains_key(&machine) {
            return;
        }
        let answering = std::mem::replace(&mut self.answering, machine.clone());
        if machine == self.front {
            then(self, cx);
        } else if let Some(pages) = self.sides.remove(&machine) {
            let front = self.swap_pages(pages);
            let aside = std::mem::replace(&mut self.aside, true);
            then(self, cx);
            self.aside = aside;
            let pages = self.swap_pages(front);
            self.sides.insert(machine, pages);
        }
        self.answering = answering;
    }

    /// The machine of the pages the shell holds now: the one in front, or the one whose answer
    /// they are taking aside ([`Shell::on_pages`]).
    fn pages_machine(&self) -> Option<String> {
        match self.aside {
            true => self.answering.clone(),
            false => self.front.clone(),
        }
    }

    /// What was read of `machine` for its projects' pages, by a link of its own made the first
    /// time and kept open: its events keep it current.
    fn copy_of(&mut self, machine: &str) {
        if self.copies.contains_key(machine) {
            return;
        }
        let (updates, end) = link::Updates::from(self.sender.clone())
            .of(machine)
            .ending();
        let seen = link::spawn_into(self.follow.connect.clone(), updates);
        self.copies.insert(
            machine.to_owned(),
            Copied {
                _seen: seen,
                _end: end,
                read: None,
            },
        );
    }

    /// A read of `machine` for its projects' pages: one of them shown is drawn with it.
    fn copied(
        &mut self,
        machine: String,
        read: Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(copy) = self.copies.get_mut(&machine) {
            copy.read = Some(read);
        }
        if self.front.as_deref() == Some(machine.as_str()) {
            self.reshow(window, cx);
        }
    }

    /// Whether the pages in the shell's fields are those of the page shown.
    fn pages_shown(&self) -> bool {
        !self.aside
    }

    /// What the pages read: this machine's, or what was read of the machine in front.
    fn pages(&self) -> &Snapshot {
        match &self.front {
            None => &self.snapshot,
            Some(front) => self
                .copies
                .get(front)
                .and_then(|copy| copy.read.as_ref())
                .unwrap_or(&self.nothing),
        }
    }

    /// The machine of the page shown said something: it answers.
    fn heard(&mut self, cx: &mut Context<Self>) {
        self.heard = true;
        self.silent = None;
        self.main
            .update(cx, |main, cx| main.set_machine_silent(None, cx));
    }

    /// A page of another machine was shown: past the limit the window gives a daemon that does
    /// not answer, with nothing heard of that machine, the page says so; the rest of the window
    /// is untouched.
    fn listen(&mut self, cx: &mut Context<Self>) {
        self.heard = false;
        self.silent = None;
        self.main
            .update(cx, |main, cx| main.set_machine_silent(None, cx));
        let Some(machine) = self.front.clone() else {
            self.heard = true;
            return;
        };
        let limit = link::not_answering();
        self.silent = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(limit).await;
            this.update(cx, |shell, cx| {
                if !shell.heard && shell.front.as_deref() == Some(machine.as_str()) {
                    let said = format!("{machine} does not answer");
                    shell
                        .main
                        .update(cx, |main, cx| main.set_machine_silent(Some(said), cx));
                }
            })
            .ok();
        }));
    }

    /// Applies the updates the link sent since the last call.
    pub fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(Ok(update)) = self.updates.as_mut().map(|u| u.try_recv()) {
            self.apply(update, window, cx);
        }
    }

    /// The composer of a project whose page was opened.
    pub fn composer_of(&self, project: &str) -> Option<&Entity<Composer>> {
        self.forms.get(project).map(|(composer, _)| composer)
    }

    pub fn new_workspace_of(&self, project: &str) -> Option<&Entity<NewWorkspace>> {
        self.forms.get(project).map(|(_, form)| form)
    }

    /// The composer and form of the page's project, made the first time.
    fn forms_for(
        &mut self,
        page: &Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Forms> {
        let Page::Project { name } = page else {
            return None;
        };
        let this = cx.entity().downgrade();
        let machine = self.pages_machine();
        let drafted_on = machine.clone();
        let made = !self.forms.contains_key(name);
        let forms = self.forms.entry(name.clone()).or_insert_with(|| {
            let (a, b) = (
                crate::commands::ShellOf::on(this.clone(), machine.clone()),
                crate::commands::ShellOf::on(this, machine),
            );
            (
                cx.new(|cx| Composer::new(a, name.clone(), window, cx)),
                cx.new(|cx| NewWorkspace::new(b, name.clone(), window, cx)),
            )
        });
        if made {
            // What the daemon keeps of its draft comes back as an `Outcome::Draft`.
            self.actions.send(Command::DraftRead {
                key: DraftKey::Project {
                    machine: drafted_on,
                    name: name.clone(),
                },
            });
        }
        let forms = forms.clone();
        // Shown, the composer is on Auto: a mode chosen for one session never reaches the next.
        forms.0.update(cx, |composer, cx| composer.reset_mode(cx));
        if let Some(project) = self.pages().projects.iter().find(|p| &p.name == name) {
            let (repositories, default_set) = (
                project.repositories.clone(),
                project.default_repositories.clone(),
            );
            forms.1.update(cx, |form, cx| {
                form.set_repositories(repositories, default_set, cx)
            });
        }
        Some(forms)
    }

    /// The open project's settings page, made the first time, and kept while the same project's
    /// settings are open; opened, it asks the daemon for them.
    fn project_settings_for(
        &mut self,
        page: &Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ProjectSettingsPage>> {
        let Page::ProjectSettings { name } = page else {
            self.project_settings = None;
            return None;
        };
        if let Some(open) = self.project_settings.clone()
            && open.read(cx).name() == name
        {
            return Some(open);
        }
        let this = cx.entity().downgrade();
        let this = crate::commands::ShellOf::on(this, self.pages_machine());
        let made = cx.new(|cx| ProjectSettingsPage::new(this, name.clone(), window, cx));
        // The shell is being updated here: it asks the daemon itself, not through the page.
        self.perform(Command::ReadProjectSettings(name.clone()));
        self.perform(Command::RulesRead {
            project: Some(name.clone()),
        });
        self.perform(Command::WorkspacesFolderRead {
            project: Some(name.clone()),
        });
        // The Tracker's select offers the entries the project sees.
        self.perform(Command::Vault);
        self.project_settings = Some(made.clone());
        Some(made)
    }

    /// The form that adds a project.
    pub fn add_project_form(&self) -> &Entity<AddProject> {
        &self.add_project
    }

    /// Opens the form that adds a project.
    pub fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        let asks_machine = self
            .machines
            .read(cx)
            .machines()
            .is_some_and(|read| crate::machines::choices(read).len() > 1);
        let machine = self.front.clone();
        self.add_project
            .update(cx, |form, cx| form.open(asks_machine, machine, window, cx));
        self.read_folders(cx);
    }

    /// The folders the machine shown offers to add a project from, while the form is open on
    /// another machine: this one's are picked with the system's dialog.
    fn read_folders(&mut self, cx: &mut Context<Self>) {
        let form = self.add_project.read(cx);
        if form.is_open()
            && let Some(machine) = form.machine().map(str::to_owned)
        {
            self.perform_on(
                Some(&machine),
                Command::ProjectFolders {
                    machine: Some(machine.clone()),
                    path: None,
                },
            );
        }
    }

    /// The dialog that renames a session.
    pub fn rename_form(&self) -> &Entity<RenameSession> {
        &self.rename
    }

    /// "Rename…" in a session's row or header: the dialog, its field holding the title it has.
    pub fn rename_of(&mut self, row: &str, window: &mut Window, cx: &mut Context<Self>) {
        let found = self.snapshot.session(row).or(self.snapshot.elsewhere(row));
        let Some(title) = found.map(|s| s.title.clone()) else {
            return;
        };
        // Renamed on the row's machine, under its id there, whichever machine's page is shown.
        let machine = self.machine_of(&Selection::Session(row.to_owned()), cx);
        self.ensure_pages(machine.as_deref(), window, cx);
        let (agent, asks) = (self.agent_of(row), self.actions_of(machine.as_deref()));
        self.rename
            .update(cx, |form, cx| form.open(&agent, &title, asks, window, cx));
    }

    /// The dialog that makes a group of projects.
    pub fn group_form(&self) -> &Entity<GroupName> {
        &self.group_form
    }

    /// Whether a group has this name already, the group `besides` left out.
    pub fn group_named(&self, name: &str, besides: Option<&str>) -> bool {
        self.arrangement.names(name, besides)
    }

    /// The projects' list as the user arranged it, for the projects the window holds.
    pub fn listing(&self) -> Listed {
        self.arrangement.listed(&self.project_names())
    }

    /// Whether this machine hides the project row `name`.
    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden.iter().any(|hidden| hidden == name)
    }

    /// The rows this machine hides, in the order they were hidden.
    pub fn hidden_projects(&self) -> &[String] {
        &self.hidden
    }

    /// Whether the Projects header's menu offers Group by machine now (there is another machine).
    pub fn offers_grouping(&self, cx: &App) -> bool {
        self.sidebar.read(cx).offers_grouping()
    }

    /// "New group…" in the Projects header's menu: the dialog.
    pub fn new_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.group_form.update(cx, |form, cx| form.open(window, cx));
    }

    /// The names of the projects the window holds, in the order the daemon lists them.
    fn project_names(&self) -> Vec<String> {
        self.snapshot
            .projects
            .iter()
            .filter(|project| !self.is_hidden(&project.name))
            .map(|project| project.name.clone())
            .collect()
    }

    /// A group made, from the dialog: at the end of the groups, unfolded.
    pub fn add_group(&mut self, name: &str, cx: &mut Context<Self>) {
        self.arrange(Change::add_group(name), cx);
    }

    /// "Rename…" in a group's menu: the New group dialog, its field holding the name it has.
    pub fn ask_rename_group(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .arrangement
            .groups
            .iter()
            .find(|group| group.id == id)
            .map(|group| group.name.clone())
        else {
            return;
        };
        self.group_form
            .update(cx, |form, cx| form.open_rename(id, &name, window, cx));
    }

    /// A group given another name, from the dialog.
    pub fn rename_group(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let (id, name) = (id.to_owned(), name.trim().to_owned());
        self.arrange(Change::RenameGroup { id, name }, cx);
    }

    /// A group removed: its projects go back to the list, at the end, none removed, which a
    /// toast says.
    pub fn remove_group(&mut self, id: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(held) = self
            .listing()
            .groups
            .into_iter()
            .find(|group| group.id == id)
            .map(|group| group.projects.len())
        else {
            return;
        };
        self.arrange(Change::RemoveGroup(id.to_owned()), cx);
        let said = gpui_design_system::Toast::success("Group removed");
        gpui_design_system::toast(
            match held {
                0 => said,
                1 => said.detail("Its project is back in the list."),
                held => said.detail(format!("Its {held} projects are back in the list.")),
            },
            cx,
        );
    }

    /// A project row dragged to `index` among the rows of `group` (`None`: outside any group).
    pub fn project_moved(
        &mut self,
        project: &str,
        group: Option<&str>,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A project with no file is in no arrangement: it stays after the user's list.
        if project.starts_with(crate::tree::GONE) {
            return;
        }
        let change = Change::MoveProject {
            present: self.project_names(),
            key: project.to_owned(),
            group: group.map(str::to_owned),
            index,
        };
        self.arrange(change, cx);
    }

    /// A group's heading dragged to `index` among the groups.
    pub fn group_moved(
        &mut self,
        group: &str,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = group.to_owned();
        self.arrange(Change::MoveGroup { id, index }, cx);
    }

    /// A group's heading clicked: its rows fold away or unfold.
    pub fn group_opened(
        &mut self,
        group: &str,
        open: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = group.to_owned();
        self.arrange(Change::OpenGroup { id, open }, cx);
    }

    /// The rows of the projects as the user arranged them, from the nodes the tree holds.
    fn arranged_rows(
        &self,
        present: &[String],
        nodes: &HashMap<String, Entity<ProjectNode>>,
        cx: &App,
    ) -> Arranged {
        let layout = self.arrangement.listed(present);
        let rows = |keys: &[String]| -> Vec<Entity<ProjectNode>> {
            keys.iter()
                .filter_map(|key| nodes.get(key).cloned())
                .collect()
        };
        Arranged {
            groups: layout
                .groups
                .iter()
                .map(|group| {
                    let rows = rows(&group.projects);
                    ArrangedGroup {
                        id: group.id.clone(),
                        name: group.name.clone(),
                        open: group.open,
                        mark: AgentState::strongest(
                            rows.iter().filter_map(|row| row.read(cx).held()),
                        ),
                        rows,
                    }
                })
                .collect(),
            loose: rows(&layout.loose),
        }
    }

    /// The list draws the arrangement.
    fn show_arrangement(&mut self, cx: &mut Context<Self>) {
        let present = self.project_names();
        let arranged = self.arranged_rows(&present, &self.projects, cx);
        if self.sidebar.read(cx).arranged() != &arranged {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_arranged(arranged, cx));
        }
    }

    /// The user changed the arrangement: the list draws it, and the daemon keeps it, under its
    /// own key, by applying the change to the arrangement it holds. This is the only place it is
    /// written.
    fn arrange(&mut self, change: Change, cx: &mut Context<Self>) {
        self.arrangement.apply(&change);
        self.arrangement_queue.push_back((change, false));
        self.show_arrangement(cx);
        self.keep_arrangement();
    }

    /// The oldest change not kept yet is asked of the daemon, unless one is asked already and not
    /// answered, or it could not take one lately: the changes are applied in the order the user
    /// made them, which one asked again after a lost connection would not be if the next were
    /// already on its way.
    fn keep_arrangement(&mut self) {
        if !self.kept_read || self.arrangement_waiting {
            return;
        }
        let Some((change, sent)) = self.arrangement_queue.front_mut() else {
            return;
        };
        if *sent {
            return;
        }
        *sent = true;
        let change = change.clone();
        self.perform_here(Command::ArrangementChange { change });
    }

    /// What the daemon holds, with what the user did since and it has not kept yet.
    fn arrangement_now(&self) -> Arrangement {
        let mut now = self.arrangement_kept.clone();
        for (change, _) in &self.arrangement_queue {
            now.apply(change);
        }
        now
    }

    /// The daemon answered the ask of a change: the arrangement it holds now is the base of what
    /// the list shows, so that what another window arranged shows here too. A change it could not
    /// take is asked again, later.
    fn arrangement_answered(
        &mut self,
        change: Change,
        held: Option<Arrangement>,
        result: Written,
        cx: &mut Context<Self>,
    ) {
        if let Some(held) = held {
            self.arrangement_kept = held;
        }
        let at = self
            .arrangement_queue
            .iter()
            .position(|(asked, sent)| *sent && *asked == change);
        match (result, at) {
            (Written::Retry, Some(at)) => {
                self.arrangement_queue[at].1 = false;
                let wait = (crate::kept::DEBOUNCE * 2u32.pow(self.arrangement_failures.min(6)))
                    .min(Duration::from_secs(30));
                self.arrangement_failures += 1;
                self.arrangement_waiting = true;
                self.arrangement_retry = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |shell, _| {
                        shell.arrangement_waiting = false;
                        shell.keep_arrangement()
                    })
                    .ok();
                }));
            }
            // Kept, or refused for good, or gone: asking again would be answered the same.
            (Written::Retry, None) => {}
            (_, at) => {
                self.arrangement_failures = 0;
                if let Some(at) = at {
                    self.arrangement_queue.remove(at);
                }
                self.keep_arrangement();
            }
        }
        let now = self.arrangement_now();
        if now != self.arrangement {
            self.arrangement = now;
            self.show_arrangement(cx);
        }
    }

    /// The quick switcher.
    pub fn switcher(&self) -> &Entity<Switcher> {
        &self.switcher
    }

    /// What the window knows of the parts of Flottille it draws in its places.
    pub fn extensions(&self) -> &Entity<Registry> {
        &self.extensions
    }

    /// The open spec's page, made the first time and kept while the same spec is open; opened,
    /// it asks the daemon for the spec.
    fn spec_page_for(&mut self, page: &Page, cx: &mut Context<Self>) -> Option<Entity<SpecPage>> {
        let Page::Spec { project, spec } = page else {
            self.spec_page = None;
            return None;
        };
        if let Some(open) = self.spec_page.clone()
            && open.read(cx).spec() == *spec
        {
            return Some(open);
        }
        let this = crate::commands::ShellOf::on(cx.entity().downgrade(), self.pages_machine());
        let made = cx.new(|_| SpecPage::new(this, project.clone(), *spec));
        let reading = made.clone();
        cx.defer(move |cx| reading.update(cx, |page, cx| page.read(cx)));
        self.spec_page = Some(made.clone());
        Some(made)
    }

    /// What the registry lists changed: the pages that draw it are drawn again, and the page open
    /// is shown as it is now.
    fn extensions_moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.update(cx, |_, cx| cx.notify());
        let features = self.settings.read(cx).features(&self.extensions, cx);
        self.switcher
            .update(cx, |switcher, cx| switcher.set_features(features, cx));
        if let Some(page) = self.project_settings.clone() {
            page.update(cx, |_, cx| cx.notify());
        }
        if let Some(page) = self.main.read(cx).project_page().cloned() {
            let items = page.read(cx).items().clone();
            items.update(cx, |items, cx| items.extensions_moved(cx));
        }
        if let Some(page) = self.spec_page.clone() {
            page.update(cx, |_, cx| cx.notify());
        }
        if let Some(selection) = self.selected.clone() {
            let page = self.page_of(&selection, cx);
            if self.main.read(cx).page() != &page {
                let forms = self.forms_for(&page, window, cx);
                let session = self.session_for(&page, window, cx);
                let settings = self.project_settings_for(&page, window, cx);
                let spec = self.spec_page_for(&page, cx);
                self.main.update(cx, |main, cx| {
                    main.set_spec_page(spec);
                    main.show(page, forms, session, settings, cx)
                });
            }
        }
    }

    /// The window's own focus: what has it when nothing else does.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Opens or closes the quick switcher: Ctrl K, or a click on the top bar's search. An open
    /// dialog or list (Add a project, a confirmation, a menu) keeps the keys: it is left first.
    pub fn toggle_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.switcher.read(cx).is_open() && keys_held(cx) {
            return;
        }
        self.set_bell(false, cx);
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        self.switcher
            .update(cx, |switcher, cx| switcher.toggle(window, cx));
    }

    /// Opens Settings on the tab of that section of [`crate::settings::SECTIONS`].
    pub fn open_settings_tab(&mut self, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open(Selection::Settings, window, cx);
        self.settings.update(cx, |page, cx| page.choose(tab, cx));
    }

    /// How this window signs what only the user's device may ask, rather than with the process's
    /// signer (`link::sign_with`): a window of its own device, as a test's two machines have.
    pub fn sign_with(&mut self, sign: crate::link::Sign) {
        self.actions.sign_with(sign.clone());
        for pages in self.sides.values_mut() {
            pages.actions.sign_with(sign.clone());
        }
        self.sign = Some(sign);
    }

    /// What opens a folder in the user's editor, for the session pages opened from now on.
    pub fn set_opener(&mut self, opener: Opener) {
        self.follow.opener = opener.clone();
        for pages in self.sides.values_mut() {
            pages.follow.opener = opener.clone();
        }
    }

    /// Opens a file of this machine with the system's default handler: a copy of another
    /// machine's file the window wrote (`crate::shown_files`).
    pub fn open_local(&self, path: &std::path::Path) {
        (self.follow.opener)(path);
    }

    /// Opens a link (a pull request) with the system's default handler.
    pub fn open_link(&self, url: &str) {
        (self.follow.opener)(std::path::Path::new(url));
    }

    /// Opens an address of the machine shown, with the system's default handler: this machine's
    /// own as it is. On another machine, an address of that machine itself (a run's, or a
    /// `localhost` link) opens through a tunnel, then the handler is given the local address; one
    /// of the web opens as it is; a path of that machine opens nowhere here (`docs/remote.md`,
    /// "Tunnels"). `run` is the run whose address it is, when the row knows it.
    pub fn open_address(&mut self, run: Option<&str>, url: &str, cx: &mut Context<Self>) {
        use crate::tunnels::{Ask, Tunnels, failed};
        let Some(machine) = self.front.clone() else {
            self.open_link(url);
            return;
        };
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            failed(&format!("{url} is a file of {machine}"), cx);
            return;
        }
        if !flottille_core::address::is_local(url) {
            self.open_link(url);
            return;
        }
        let port = flottille_core::address::local_port(url);
        // A link has no run: it names a port, and the machine's runs say which one serves it.
        let (run, port) = match (run, port) {
            (Some(run), _) => (Some(run.to_owned()), None),
            (None, Some(port)) => (None, Some(port)),
            (None, None) => {
                failed(
                    &format!(
                        "{url} names no port, so nothing of {machine} can be reached through it"
                    ),
                    cx,
                );
                return;
            }
        };
        let ask = Ask {
            machine,
            run,
            port,
            url: url.to_owned(),
        };
        let held = cx.global::<Tunnels>();
        if let Some(tunnel) = held.reaching(&ask) {
            let local = flottille_core::address::through(url, tunnel.local_port);
            (self.follow.opener)(std::path::Path::new(&local));
            return;
        }
        if held.is_opening(&ask) {
            return;
        }
        cx.global_mut::<Tunnels>().begin(ask.clone());
        self.perform(Command::TunnelOpen(ask));
    }

    /// Opens what an agent showed (`show_link`) after the checks Open makes: of this machine, as
    /// it is; of another, through [`Shell::open_address`].
    pub fn open_shown(&mut self, target: &str, cx: &mut Context<Self>) {
        match self.front.clone() {
            None => crate::open::shown(&self.follow.opener, target, cx),
            Some(_) => self.open_address(None, target, cx),
        }
    }

    /// Closes one of the tunnels the window holds; its connection says when it is closed.
    pub fn close_tunnel(&mut self, tunnel: &str, _cx: &mut Context<Self>) {
        self.perform(Command::TunnelClose {
            tunnel: tunnel.to_owned(),
        });
    }

    /// What a connection that holds a tunnel said: the local address is opened once it is open, a
    /// failure is a toast with its reason, an ended tunnel leaves its row.
    fn tunnel_news(&mut self, news: crate::tunnels::News, cx: &mut Context<Self>) {
        use crate::tunnels::{News, Tunnels, failed};
        match news {
            News::Opened { ask, tunnel } => {
                let local = flottille_core::address::through(&ask.url, tunnel.local_port);
                cx.global_mut::<Tunnels>().opened(&ask, tunnel);
                (self.follow.opener)(std::path::Path::new(&local));
            }
            News::Refused { ask, why } => {
                cx.global_mut::<Tunnels>().refused(&ask);
                failed(&why, cx);
            }
            News::Closed { tunnel, .. } => cx.global_mut::<Tunnels>().closed(&tunnel),
            News::NotClosed { why } => {
                gpui_design_system::toast(
                    gpui_design_system::Toast::failure(format!("Not closed: {why}")),
                    cx,
                );
            }
        }
    }

    /// Whether the session's composer holds a draft: what the field holds once its page was
    /// opened and the daemon answered, what the daemon kept when the window opened until then.
    fn draft_held(&self, session: &str, cx: &App) -> bool {
        let listed = self.listed_drafts.contains(session);
        self.replies.get(session).map_or(listed, |reply| {
            let reply = reply.read(cx);
            reply.holds_draft() || (!reply.draft_was_read() && listed)
        })
    }

    /// Every row says whether its composer holds a draft.
    fn say_drafts(&self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in ids {
            self.say_draft(&id, cx);
        }
    }

    /// Tells a session's row whether its composer holds a draft; the row says it as a pencil
    /// while the session is not the open one.
    fn say_draft(&self, session: &str, cx: &mut Context<Self>) {
        let held = self.draft_held(session, cx);
        if let Some(row) = self.sessions.get(session)
            && row.read(cx).draft() != held
        {
            row.update(cx, |row, cx| row.set_draft(held, cx));
        }
    }

    /// The composer of a session whose page was opened.
    pub fn reply_of(&self, session: &str) -> Option<&Entity<Reply>> {
        self.replies.get(session)
    }

    /// The open session's page, made the first time, and kept while the same session is open.
    fn session_for(
        &mut self,
        page: &Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SessionPage>> {
        let Page::Session {
            id,
            title,
            place,
            status,
        } = page
        else {
            self.session_page = None;
            return None;
        };
        // A session of another machine is known here by its row: its id there is its page's.
        let agent = self.agent_of(id);
        let subject = self.snapshot.session(id).and_then(|s| s.subject.clone());
        if let Some(open) = self.session_page.clone()
            && open.read(cx).id() == agent
        {
            if open.read(cx).subject() != subject.as_ref() {
                open.update(cx, |open, cx| open.set_subject(subject, cx));
            }
            open.update(cx, |open, cx| open.set_title(title, cx));
            return Some(open);
        }
        // Shown before, it followed its session meanwhile: it is shown at once.
        if let Some(kept) = self.kept_pages.get(id).cloned() {
            if kept.read(cx).subject() != subject.as_ref() {
                kept.update(cx, |kept, cx| kept.set_subject(subject, cx));
            }
            kept.update(cx, |kept, cx| kept.set_title(title, cx));
            self.chose_on(&kept, id, cx);
            self.session_page = Some(kept.clone());
            return Some(kept);
        }
        let (project, started_by, led_by, child_of, folder) = match self.snapshot.session(id) {
            Some(session) => {
                let project = session.project.clone();
                let folder = match place {
                    None => self
                        .snapshot
                        .projects
                        .iter()
                        .find(|p| p.name == project)
                        .map(|p| p.folder.clone()),
                    Some(name) => self
                        .snapshot
                        .workspaces
                        .iter()
                        .find(|w| w.project == project && &w.name == name)
                        .map(|w| w.folder.clone()),
                };
                (
                    project,
                    session.started_by.clone(),
                    session.led_by.clone(),
                    session.child_of.clone(),
                    folder,
                )
            }
            // Its row is all this window holds of it: its page reads the rest there.
            None => (
                self.snapshot.elsewhere(id)?.project.clone(),
                None,
                None,
                None,
                None,
            ),
        };
        let opened = Opened {
            id: agent.clone(),
            title: title.clone(),
            project,
            place: place.clone(),
            folder: folder.map(Into::into),
            started_by,
            led_by,
            child_of,
        };
        let this = cx.entity().downgrade();
        // Its machine's actions, taken now: what the page asks goes there for good (#1271).
        let machine = self.machine_of(&Selection::Session(id.clone()), cx);
        let asks = self
            .actions_of(machine.as_deref())
            .unwrap_or_else(|| self.actions.clone());
        let reply = match self.replies.get(id) {
            Some(reply) => reply.clone(),
            None => {
                let (shell, project) = (this.clone(), opened.project.clone());
                let (session, its, on) = (agent.clone(), asks.clone(), machine.clone());
                let extensions = self.extensions.downgrade();
                let reply = cx.new(|cx| {
                    Reply::new(shell, its, on, session, project, window, cx)
                        .with_extensions(extensions)
                });
                self._drafts.push(cx.observe(&reply, |shell, reply, cx| {
                    let session = reply.read(cx).session().to_owned();
                    shell.say_draft(&session, cx);
                }));
                self.replies.insert(id.clone(), reply.clone());
                // What this machine keeps of its draft comes back as an `Outcome::Draft`, said of
                // the machine whose actions asked (the keeper always asks this one).
                asks.send(Command::DraftRead {
                    key: DraftKey::Session {
                        machine: machine.clone(),
                        agent: agent.clone(),
                    },
                });
                reply
            }
        };
        if self.providers.known {
            let mode = reply.read(cx).mode.clone();
            let chosen = self.providers.judge.is_some();
            mode.update(cx, |mode, cx| mode.set_judge_chosen(chosen, cx));
        }
        let (follow, layout, status) = (self.follow.clone(), self.layout, status.clone());
        let dock = self
            .docks
            .entry(id.clone())
            .or_insert_with(|| cx.new(|_| crate::ask_dock::AskDock::new()))
            .clone();
        let asked = (machine.clone(), asks);
        let page = cx.new(|cx| {
            SessionPage::new(this, opened, status, reply, dock, follow, asked, layout, cx)
        });
        let providers = self.providers.clone();
        let blocked = self.snapshot.session(id).and_then(|s| s.blocked.clone());
        page.update(cx, |page, cx| {
            page.set_providers(&providers, cx);
            page.set_subject(subject, cx);
            page.set_blocked(blocked, cx);
        });
        let left = self.session_page.clone();
        self.chose_on(&page, id, cx);
        self.session_page = Some(page.clone());
        // Another machine's, it enters staggered, as a page that shows up.
        if agent != *id {
            page.update(cx, |page, cx| page.arrive(cx));
        }
        // It is kept, following its session, with the one left before it on the same machine,
        // this one too: opened again, a session shows what its page holds at once.
        self.kept_pages.retain(|_, kept| {
            kept.read(cx).machine() != machine.as_deref() || left.as_ref() == Some(kept)
        });
        self.kept_pages.insert(id.clone(), page.clone());
        // The rail's Run a task lists the project's tasks.
        let project = page.read(cx).project().to_owned();
        self.actions.send(Command::Tasks { project });
        Some(page)
    }

    /// The agent the history shows on the session page `page`, of the row `row`.
    fn chose_on(&mut self, page: &Entity<SessionPage>, row: &str, cx: &mut Context<Self>) {
        let row = row.to_owned();
        self._chose = Some(cx.subscribe(page, move |shell, _, Chose(agent), _| {
            shell.history.show_agent(&row, agent);
        }));
    }

    /// The id on its machine of the session of the row `id`: another machine's row names it.
    fn agent_of(&self, id: &str) -> String {
        self.snapshot
            .elsewhere(id)
            .and_then(|session| session.elsewhere.as_ref())
            .filter(|there| !there.here)
            .map_or_else(|| id.to_owned(), |there| there.agent.clone())
    }

    /// The row of the session `agent` an answer names: another machine's answers name its own
    /// id there, and its composer is kept under its row's (#1271).
    fn row_answered(&self, agent: &str) -> String {
        match &self.answering {
            Some(machine) => crate::model::elsewhere_id(machine, agent),
            None => agent.to_owned(),
        }
    }

    /// The actions of the pages of `machine` (`None`: this one), wherever they wait: what a page
    /// of that machine asks goes there, whichever machine's page is shown then (#1271).
    ///
    /// While another machine's answer is taken aside ([`Shell::on_pages`]), `self.actions` are
    /// that machine's and the front's wait outside `sides`: this answers wrongly for the front
    /// then. Pages ask through their [`crate::commands::ShellOf`] from clicks, timers and defers,
    /// outside that update, so nothing is known to ask it there.
    fn actions_of(&self, machine: Option<&str>) -> Option<Actions> {
        if machine == self.front.as_deref() {
            return Some(self.actions.clone());
        }
        self.sides
            .get(&machine.map(str::to_owned))
            .map(|pages| pages.actions.clone())
    }

    /// Asks the daemon of `machine` (`None`: this one), whichever machine's page is shown.
    pub fn perform_on(&self, machine: Option<&str>, command: Command) {
        if let Some(actions) = self.actions_of(machine) {
            actions.send(command);
        }
    }

    /// Asks this machine's daemon, whichever machine's page is shown: what the list, its rows
    /// and the bell ask is this machine's.
    fn perform_here(&self, command: Command) {
        match self.front {
            None => self.actions.send(command),
            Some(_) => match self.sides.get(&None) {
                Some(pages) => pages.actions.send(command),
                None => self.actions.send(command),
            },
        }
    }

    /// Asks the daemon, off the UI thread; the answer comes back as an update.
    pub fn perform(&self, command: Command) {
        self.actions.send(command);
    }

    /// Asks the daemon to keep `choice` as the appearance, or what it keeps when there is none.
    /// Each request is numbered; the window only believes the answer to the latest.
    pub fn ask_appearance(&mut self, choice: Option<gpui_design_system::Appearance>) {
        self.appearance_asked += 1;
        let seq = self.appearance_asked;
        self.actions.send(match choice {
            Some(appearance) => Command::AppearanceSet { appearance, seq },
            None => Command::AppearanceRead { seq },
        });
    }

    /// A place's "New session here": the composer, set to start there.
    pub fn compose_in(
        &mut self,
        project: &str,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(composer) = self.composer_of(project).cloned() {
            composer.update(cx, |composer, cx| composer.choose(choice, window, cx));
        }
    }

    /// "End session" in a session's row: the confirmation, which counts the agents that end.
    pub fn ask_end_of(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(found) = self.snapshot.session(session) else {
            // A session of another machine, of its list or one an agent of this machine launched
            // there (#1520): it alone ends, through its machine.
            if self.snapshot.elsewhere(session).is_some_and(|found| {
                !found.ended && found.elsewhere.as_ref().is_some_and(|t| !t.here)
            }) {
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.ask_end(session.to_owned(), 1, cx));
            }
            return;
        };
        let (id, count) = (found.id.clone(), found.live);
        self.perform_here(Command::ReadEnding {
            session: id.clone(),
        });
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.ask_end(id, count, cx));
    }

    /// "Detach from …" in the row of a session an agent started: the session becomes the
    /// user's alone, and the result is a toast.
    pub fn detach_of(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(from) = self
            .snapshot
            .session(session)
            .and_then(|found| found.started_by.clone())
        else {
            return;
        };
        self.perform_here(Command::Detach {
            session: session.to_owned(),
            from,
        });
        cx.notify();
    }

    /// A row folded or unfolded the led sessions under it: the level that holds it is drawn
    /// again at once, from what the daemon last said, in the arrangement shown (`show`).
    pub fn relist(&mut self, session: &str, cx: &mut Context<Self>) {
        let snapshot = &self.snapshot;
        let Some(found) = snapshot
            .sessions
            .iter()
            .chain(&snapshot.elsewhere)
            .find(|s| s.id == session)
        else {
            eprintln!("flottille: the fold of {session} has no session to draw it from");
            return;
        };
        let project = found.project.clone();
        // Grouped by machine, the level is the project's under the session's machine, and holds
        // that machine's sessions only.
        let machine = self.grouped.then(|| snapshot.machine_of(found).to_owned());
        // This machine's session of a project with no settings file is under that row, not under
        // the group of another machine's project of its name.
        let gone = snapshot.is_gone_here(&project) && snapshot.is_here(found);
        let live: Vec<&crate::model::Session> = snapshot
            .level_sessions(&project, gone)
            .into_iter()
            .filter(|s| {
                machine
                    .as_deref()
                    .is_none_or(|m| snapshot.machine_of(s) == m)
            })
            .collect();
        let node = match (gone, &machine) {
            (true, _) => self.gone.get(&(project.clone(), machine.clone())),
            (false, Some(machine)) => self
                .machine_projects
                .get(&(project.clone(), machine.clone())),
            (false, None) => self.projects.get(&project),
        };
        let Some(node) = node.cloned() else {
            eprintln!("flottille: the fold of {session} has no level of {project} to draw");
            return;
        };
        // A session with no row yet is left out, said, and the others are drawn.
        let (live, rows): (Vec<_>, Vec<_>) = live
            .into_iter()
            .filter_map(|s| match self.sessions.get(&s.id) {
                Some(row) => Some((s, row.clone())),
                None => {
                    eprintln!("flottille: no row for {} of {project} yet", s.id);
                    None
                }
            })
            .unzip();
        let rows = nested(&live, &rows, cx);
        node.update(cx, |node, cx| node.set_rows(rows, cx));
    }

    /// "Resume" in an ended session's row.
    pub fn resume_of(&mut self, session: &str, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.resuming(session, cx));
        self.perform_here(Command::Resume {
            session: session.to_owned(),
        });
    }

    /// The names of the projects of `machine` (`None`: this one), as last read of it: none of
    /// another machine whose projects were never read.
    pub fn projects_on(&self, machine: Option<&str>) -> Vec<String> {
        let read = match machine {
            None => Some(self.read.as_ref().unwrap_or(&self.snapshot)),
            Some(name) => self.copies.get(name).and_then(|copy| copy.read.as_ref()),
        };
        read.map(|read| read.projects.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default()
    }

    /// The pull request the window read of the workspace `workspace` of `project` on `machine`
    /// (`None`: this one).
    pub fn pull_request_of(
        &self,
        machine: Option<&str>,
        project: &str,
        workspace: &str,
    ) -> Option<&PullRequest> {
        self.pulls.get(&(
            machine.map(str::to_owned),
            project.to_owned(),
            workspace.to_owned(),
        ))
    }

    /// What the window shows now, as the daemon last gave it.
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn seen(&self) -> &Arc<Seen> {
        &self.seen
    }

    /// Whether the window holds anything of the pages of `machine` while another is in front:
    /// its pages, a copy of it, the kept page of one of its sessions.
    pub fn holds_pages_of(&self, machine: &str) -> bool {
        self.sides.contains_key(&Some(machine.to_owned()))
            || self.copies.contains_key(machine)
            || self.kept_pages.keys().any(|row| {
                self.snapshot
                    .elsewhere(row)
                    .and_then(|session| session.elsewhere.as_ref())
                    .is_some_and(|there| there.machine == machine)
            })
    }

    /// The visit the history is at: the page shown, unless one is on its way.
    pub fn visit_shown(&self) -> Option<&crate::history::Visit> {
        self.history.current()
    }

    /// How many dialogs the window holds, open or not: each machine's Settings has its own.
    pub fn dialogs_held(&self, cx: &App) -> usize {
        self.shared.modals.read(cx).held()
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn selected(&self) -> Option<&Selection> {
        self.selected.as_ref()
    }

    pub fn top(&self) -> &Entity<TopBar> {
        &self.top
    }

    pub fn main(&self) -> &Entity<MainArea> {
        &self.main
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    /// The vault's section, whichever page is shown: its form is a dialog over that page.
    pub fn vault(&self, cx: &gpui_kit::App) -> Entity<VaultSection> {
        self.settings.read(cx).vault().clone()
    }

    /// The connections' section, whichever page is shown: its form and its review are dialogs
    /// over that page.
    pub fn connections(&self, cx: &gpui_kit::App) -> Entity<ConnectionsSection> {
        self.settings.read(cx).connections().clone()
    }

    /// The dialogs open over the page, the first opened first and the one on top last, by the
    /// id of their modal.
    pub fn modal_stack(&self, cx: &gpui_kit::App) -> Vec<&'static str> {
        self.modals.read(cx).stack(cx)
    }

    /// The window's toasts, at its bottom right.
    pub fn toasts(&self) -> &Entity<ToastStack> {
        &self.toasts
    }

    pub fn left_drawer(&self) -> &Entity<Drawer> {
        &self.left
    }

    pub fn project(&self, name: &str) -> Option<&Entity<ProjectNode>> {
        self.projects.get(name)
    }

    /// The row of this machine's project `name` with no settings file, in the section of `machine`
    /// while the projects are shown by machine.
    pub fn unconfigured_project(
        &self,
        name: &str,
        machine: Option<&str>,
    ) -> Option<&Entity<ProjectNode>> {
        self.gone
            .get(&(name.to_owned(), machine.map(str::to_owned)))
    }

    pub fn session(&self, id: &str) -> Option<&Entity<SessionRow>> {
        self.sessions.get(id)
    }

    /// The row of the project `name` in the section of `machine`, while the projects are shown by
    /// machine.
    pub fn project_on(&self, name: &str, machine: &str) -> Option<&Entity<ProjectNode>> {
        self.machine_projects
            .get(&(name.to_owned(), machine.to_owned()))
    }

    /// The section of `machine`, while the projects are shown by machine.
    pub fn section(&self, machine: &str) -> Option<&Entity<MachineNode>> {
        self.sections.get(machine)
    }

    /// Whether the projects are shown by machine now: the user chose it, and there is another
    /// machine to group by.
    pub fn is_by_machine(&self) -> bool {
        self.grouped
    }

    /// Group by machine, turned on or off from the Projects header's menu: the rows move to their
    /// sections or back, and the daemon keeps the choice.
    pub fn toggle_by_machine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.by_machine = !self.by_machine;
        self.rearrange(window, cx);
        self.sidebar_changed(cx);
    }

    /// "Hide on this machine" in a project's row: its row and its sessions fold away at once, and
    /// the daemon keeps the choice (`project.hide`). Nothing is sent to the project's machine.
    pub fn hide_project(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_hidden(name, true, window, cx);
    }

    /// "Show" in Settings, General, Hidden projects: its row unfolds in the sidebar again.
    pub fn show_project(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_hidden(name, false, window, cx);
    }

    fn set_hidden(&mut self, name: &str, hide: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_hidden(name) == hide {
            return;
        }
        match hide {
            true => self.hidden.push(name.to_owned()),
            false => self.hidden.retain(|hidden| hidden != name),
        }
        self.rearrange(window, cx);
        self.hidden_changed(window, cx);
        // This machine's own daemon keeps it, whichever machine's page is in front.
        self.perform_here(Command::HideProject {
            name: name.to_owned(),
            hide,
        });
    }

    /// What follows the list of hidden projects besides the sidebar: what the switcher finds, and
    /// Settings' list.
    fn hidden_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let read = self.snapshot.clone();
        let hidden = self.hidden.clone();
        self.switcher
            .update(cx, |switcher, cx| switcher.set_snapshot(&read, &hidden, cx));
        let section = self.settings.read(cx).hidden().clone();
        section.update(cx, |section, cx| section.set(hidden, cx));
        self.leave_hidden_page(window, cx);
    }

    /// The page of a project this machine hides is not left in front, with no row in the sidebar:
    /// the next shown project's page opens, or none when none is shown (as a removal does).
    fn leave_hidden_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pages_shown() || self.front.is_some() {
            return;
        }
        let on = match &self.selected {
            Some(
                Selection::Project(name)
                | Selection::ProjectSettings(name)
                | Selection::Spec { project: name, .. },
            ) => name.clone(),
            _ => return,
        };
        if self.is_hidden(&on) {
            let next = self.first_shown();
            self.show_next(next, window, cx);
        }
    }

    /// The first project of this machine's list that it does not hide.
    fn first_shown(&self) -> Option<String> {
        self.snapshot
            .projects
            .iter()
            .find(|p| !self.is_hidden(&p.name))
            .map(|p| p.name.clone())
    }

    /// The page of `next` opens, or an empty one when there is none.
    fn show_next(&mut self, next: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        match next {
            Some(next) => self.select(Selection::Project(next), window, cx),
            None => {
                self.selected = None;
                self.project_settings = None;
                self.spec_page = None;
                self.main.update(cx, |main, cx| {
                    main.set_spec_page(None);
                    main.show(Page::Empty, None, None, None, cx)
                });
            }
        }
    }

    /// The rows again from the snapshot held, as the projects are now to be shown.
    fn rearrange(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.snapshot.clone();
        self.show(snapshot, window, cx);
    }

    /// The window is closing, or the program is ending: what its composers hold is written to the
    /// daemon at once, and the process waits (a moment at most) until it is.
    pub fn closing(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.closed, true) {
            return;
        }
        let replies: Vec<_> = self.replies.values().cloned().collect();
        let composers: Vec<_> = self
            .forms
            .values()
            .map(|(composer, _)| composer.clone())
            .collect();
        let mut writes: Vec<Command> = Vec::new();
        for reply in replies {
            writes.extend(reply.update(cx, |reply, cx| reply.draft_to_write(cx, true)));
        }
        for composer in composers {
            writes.extend(composer.update(cx, |composer, cx| composer.draft_to_write(cx, true)));
        }
        for write in writes {
            self.perform(write);
        }
        self.kept_asked = None;
        self.sidebar_changed(cx);
        self.actions.flush(std::time::Duration::from_secs(2));
    }

    /// What the sidebar has folded, as its nodes show it now.
    fn folds_now(&self, cx: &Context<Self>) -> Folds {
        // What was last asked of the daemon, which the nodes that are not shown now keep.
        let last = self.kept_asked.as_ref().unwrap_or(&self.kept);
        // No rows yet (a window opening reads what the daemon keeps before its rows): the nodes
        // say nothing, and writing their empty maps would forget every fold for every window.
        if self.read.is_none() {
            return Folds {
                by_machine: self.by_machine,
                ..last.clone()
            };
        }
        let by_machine = self.is_by_machine();
        let exists = |name: &str| self.snapshot.projects.iter().any(|p| p.name == name);
        Folds {
            projects: match by_machine {
                true => last
                    .projects
                    .iter()
                    .filter(|(name, _)| exists(name))
                    .map(|(name, open)| (name.clone(), *open))
                    .collect(),
                false => self
                    .projects
                    .iter()
                    .map(|(name, node)| (name.clone(), node.read(cx).is_open()))
                    .chain(
                        self.gone
                            .iter()
                            .filter(|((_, machine), _)| machine.is_none())
                            .map(|((name, _), node)| {
                                (
                                    format!("{}{name}", crate::tree::GONE),
                                    node.read(cx).is_open(),
                                )
                            }),
                    )
                    .collect(),
            },
            by_machine: self.by_machine,
            starts: self.starts.clone(),
            machines: match by_machine {
                true => self
                    .sections
                    .iter()
                    .map(|(name, node)| (name.clone(), node.read(cx).is_open()))
                    .collect(),
                false => last.machines.clone(),
            },
            on_machines: match by_machine {
                true => self
                    .machine_projects
                    .iter()
                    .map(|((name, machine), node)| {
                        (machine_key(machine, name), node.read(cx).is_open())
                    })
                    .chain(self.gone.iter().filter_map(|((name, machine), node)| {
                        let kept = format!("{}{name}", crate::tree::GONE);
                        machine
                            .as_deref()
                            .map(|machine| (machine_key(machine, &kept), node.read(cx).is_open()))
                    }))
                    .collect(),
                false => last.on_machines.clone(),
            },
            leads: self
                .sessions
                .iter()
                .filter(|(_, row)| row.read(cx).leads() > 0)
                .map(|(id, row)| (id.clone(), row.read(cx).is_open()))
                .collect(),
            ended: self
                .ended
                .as_ref()
                .map(|group| group.read(cx).is_open())
                .or(self.kept.ended),
            // Only the feet the user unfolded: a foot is folded until then.
            blocked: self
                .projects
                .values()
                .chain(self.machine_projects.values())
                .chain(self.gone.values())
                .map(|node| node.read(cx))
                .filter(|node| node.blocked_open(cx))
                .map(|node| (node.blocked_key(), true))
                .collect(),
        }
    }

    /// A node of the sidebar was folded or unfolded, or one is gone: the daemon keeps what is
    /// folded now, and forgets the projects and the leads that no longer exist. What it did not
    /// take is asked again (`kept_written`).
    pub fn sidebar_changed(&mut self, cx: &mut Context<Self>) {
        if !self.kept_read {
            return;
        }
        let now = self.folds_now(cx);
        if now == self.kept || self.kept_asked.as_ref() == Some(&now) {
            return;
        }
        self.perform_here(Command::WindowSet {
            key: "sidebar".into(),
            value: now.to_json(),
        });
        self.kept_asked = Some(now);
    }

    /// The daemon answered a write of the sidebar's state: it is what the daemon keeps once it
    /// took it, and it is asked again, later, when it could not.
    fn kept_written(&mut self, value: serde_json::Value, result: Written, cx: &mut Context<Self>) {
        let value = Folds::of(&serde_json::json!({"sidebar": value}));
        match result {
            Written::Kept => {
                self.kept_failures = 0;
                if self.kept_asked.as_ref() == Some(&value) {
                    self.kept_asked = None;
                }
                self.kept = value;
            }
            Written::Retry => {
                if self.kept_asked.as_ref() == Some(&value) {
                    self.kept_asked = None;
                }
                let wait = (crate::kept::DEBOUNCE * 2u32.pow(self.kept_failures.min(6)))
                    .min(Duration::from_secs(30));
                self.kept_failures += 1;
                self.kept_retry = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |shell, cx| shell.sidebar_changed(cx)).ok();
                }));
            }
            // Refused for good, or gone: asking again would be answered the same.
            Written::Gone | Written::Refused => {}
        }
    }

    /// The "Ended" group at the bottom of the tree, once a session ended.
    pub fn ended(&self) -> Option<&Entity<EndedNode>> {
        self.ended.as_ref()
    }

    fn relayout(&mut self, first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let layout = Layout::of(window.viewport_size().width);
        if layout == self.layout && !first {
            return;
        }
        self.layout = layout;
        if layout != Layout::Narrow {
            self.left.update(cx, |drawer, cx| drawer.close(cx));
        }
        self.top
            .update(cx, |top, cx| top.set_menu(layout == Layout::Narrow, cx));
        self.main.update(cx, |main, cx| main.set_layout(layout, cx));
        cx.notify();
    }

    fn apply(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::Machine { machine, update } => self.apply_of(machine, *update, window, cx),
            // This machine's: what fills its pages goes to them, wherever they wait.
            update if self.front.is_some() && of_this_machine_s_pages(&update) => {
                self.on_pages(None, cx, |shell, cx| shell.take(update, window, cx));
            }
            update => self.take(update, window, cx),
        }
    }

    /// An update of another machine's connections: what its pages asked fills its pages only,
    /// and a read of it is kept for its projects' pages. Its link's own words (cut, not
    /// answering, its events) are no news of this machine's daemon.
    fn apply_of(
        &mut self,
        machine: String,
        update: Update,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // What the window keeps (drafts, even of another machine's sessions, and folds), its
        // appearance and its build are asked of this machine's daemon whichever page asks: no
        // word of that machine.
        let here = matches!(
            &update,
            Update::Done(
                Outcome::Draft { .. }
                    | Outcome::DraftWritten { .. }
                    | Outcome::WindowKept { .. }
                    | Outcome::Appearance { .. }
                    | Outcome::Update { .. }
                    | Outcome::Tunnel(_)
            )
        );
        // Its link's own words (cut, waiting) are no answer of that machine.
        let link_s = matches!(&update, Update::Disconnected(..) | Update::NotAnswering(_));
        if !here && !link_s && self.front.as_deref() == Some(machine.as_str()) {
            self.heard(cx);
        }
        match update {
            Update::Snapshot(read, _) => self.copied(machine, read, window, cx),
            Update::Bell(read) => self.bell_read(machine, read, cx),
            Update::EndedThere(read) => self.ended_read(machine, read, window, cx),
            Update::NotReadThere(ids) => {
                for id in ids {
                    self.marked_there.remove(&(machine.clone(), id));
                }
            }
            Update::Session { .. } => self.take(update, window, cx),
            // What fills its pages (its answers, its events) goes to them, wherever they wait.
            update if of_this_machine_s_pages(&update) => {
                self.on_pages(Some(machine), cx, |shell, cx| {
                    shell.take(update, window, cx)
                })
            }
            _ => {}
        }
    }

    fn take(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            // What a page of a machine no longer in front asked: its page is gone.
            Update::Done(outcome) if !self.pages_shown() && of_a_page(&outcome) => {}
            // Another machine's alone, said of it ([`Shell::apply_of`]).
            Update::Machine { .. }
            | Update::Bell(_)
            | Update::EndedThere(_)
            | Update::NotReadThere(_) => {}
            Update::Machines(news) => self.machines_news(*news, window, cx),
            Update::Closed {
                machine,
                agent,
                refused,
            } => self.closed(machine, agent, refused, window, cx),
            Update::Disconnected(..) => {
                self.set_online(false, cx);
                self.waiting_for_the_daemon(false, cx);
                self.texts_due = true;
            }
            Update::NotAnswering(since) => self
                .main
                .update(cx, |main, cx| main.set_not_answering(since, cx)),
            // The link subscribed to the events before it read: an edit made before that is in
            // what the pages read, not in the events, so they read it again.
            Update::Snapshot(snapshot, _) => {
                self.set_online(true, cx);
                self.waiting_for_the_daemon(false, cx);
                self.main.update(cx, |main, cx| main.set_read(cx));
                self.read = Some(snapshot.clone());
                let snapshot = self.with_elsewhere(snapshot, cx);
                self.show(snapshot, window, cx);
                if let Some((page, tab)) = self.reopen.take()
                    && self.page_of(&page, cx) != Page::Empty
                {
                    self.open(page, window, cx);
                    if let Some(tab) = tab {
                        self.settings.update(cx, |page, cx| page.choose(tab, cx));
                    }
                }
                if std::mem::take(&mut self.texts_due) {
                    self.read_texts(cx);
                }
            }
            Update::Kept(folds, arrangement, drafts, hidden) => {
                let by_machine = folds.by_machine;
                let hid = std::mem::replace(&mut self.hidden, hidden) != self.hidden;
                self.starts = folds.starts.clone();
                self.kept = folds;
                self.kept_read = true;
                // What the daemon keeps of how the projects are shown comes after the first rows.
                // It is read once, as the window opens: another window's later changes are seen by
                // the windows opened after them.
                self.arrangement_kept = arrangement;
                let now = self.arrangement_now();
                let moved = std::mem::replace(&mut self.arrangement, now) != self.arrangement;
                if std::mem::replace(&mut self.by_machine, by_machine) != by_machine || hid {
                    self.rearrange(window, cx);
                    self.hidden_changed(window, cx);
                } else if moved {
                    self.show_arrangement(cx);
                }
                self.keep_arrangement();
                self.listed_drafts = drafts;
                self.say_drafts(cx);
            }
            Update::Done(outcome) => self.done(outcome, window, cx),
            Update::Providers(read, providers) => self.set_providers(read, providers, cx),
            Update::TextChanged { judge: true, .. } => self.actions.send(Command::JudgeRead),
            Update::AboutChanged => self.actions.send(Command::AboutRead),
            Update::SharedChanged => self.actions.send(Command::AllSettingsRead),
            Update::DeveloperChanged => self.actions.send(Command::DeveloperRead),
            Update::FindingsChanged => self.actions.send(Command::FindingsRead),
            Update::UpdateShown(shown) => self.update_shown(&shown, cx),
            Update::WhatsNewShown(shown) => self.whats_new_shown(&shown, cx),
            Update::ToolResultsChanged => self.actions.send(Command::ToolResultsRead),
            // A machine's own setting changed: whether it follows all machines is read too.
            Update::CompactionChanged => {
                self.actions.send(Command::CompactionRead);
                self.actions.send(Command::AllSettingsRead);
            }
            Update::SessionsChanged => {
                self.actions.send(Command::SessionsRead);
                self.actions.send(Command::AllSettingsRead);
            }
            Update::VaultChanged => self.actions.send(Command::Vault),
            Update::ProgramSignIn {
                program,
                state,
                why,
            } => {
                let section = self.settings.read(cx).sign_in().clone();
                section.update(cx, |section, cx| section.changed(&program, &state, why, cx));
            }
            Update::SignIn { said, name, why } => {
                let section = self.settings.read(cx).connections().clone();
                section.update(cx, |section, cx| {
                    section.sign_in_said(&said, &name, why, cx)
                });
            }
            Update::ServicesChanged => {
                if self.pages_shown() && self.main.read(cx).services_page().is_some() {
                    self.actions.send(Command::Services);
                }
            }
            // A default that changed changes the folder every project shows, so the open
            // project's section reads again too.
            Update::WorkspacesFolderChanged { project } => {
                self.actions.send(Command::WorkspacesFolderRead {
                    project: project.clone(),
                });
                if project.is_none() {
                    self.actions.send(Command::AllSettingsRead);
                }
                if let Some(open) = self
                    .project_settings
                    .as_ref()
                    .map(|page| page.read(cx).name())
                    && project.as_deref().is_none_or(|name| name != open)
                {
                    self.actions.send(Command::WorkspacesFolderRead {
                        project: Some(open.to_owned()),
                    });
                }
            }
            Update::AppearanceChanged => self.ask_appearance(None),
            Update::Extensions => {
                self.extensions
                    .update(cx, |registry, cx| registry.changed(cx));
                let connections = self.settings.read(cx).connections().clone();
                connections.update(cx, |section, cx| section.read(cx));
                if let Some(page) = self.spec_page.clone() {
                    page.update(cx, |page, cx| page.read(cx));
                }
            }
            Update::Forged(forged) => self.forged(forged, cx),
            Update::News(news) => self.news(news, cx),
            Update::Link(link) => self.linked(link, cx),
            Update::Prompted(agent) => self.spoke(&agent),
            // Another machine's page of a project of that name may be shown.
            Update::Items { .. } if !self.pages_shown() => {}
            Update::Items { project, item } => {
                if let Some(page) = self.main.read(cx).progress_page(cx)
                    && page.read(cx).project() == project
                {
                    page.update(cx, |page, cx| page.changed(cx));
                }
                if let Some(items) = self.project_items_of(&project, cx) {
                    items.update(cx, |items, cx| items.changed(item, cx));
                }
            }
            Update::TextChanged { project, .. } => {
                // A project's editor shows the global rules above its own.
                if project.is_none()
                    && let Some(open) = self
                        .project_settings
                        .as_ref()
                        .map(|page| page.read(cx).name().to_owned())
                {
                    self.actions.send(Command::RulesRead {
                        project: Some(open),
                    });
                }
                self.actions.send(Command::RulesRead { project })
            }
            // The open page's link and those of the other machines' pages kept are listened to:
            // what an older one sends is dropped.
            Update::Session { token, update } => {
                let page = self
                    .session_page
                    .iter()
                    .chain(self.kept_pages.values())
                    .find(|page| page.read(cx).token() == Some(token))
                    .cloned();
                if let Some(page) = page {
                    page.update(cx, |page, cx| page.apply(token, update, cx));
                }
            }
        }
    }

    /// What came of something asked of the daemon.
    fn done(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        match outcome {
            Outcome::Details(details) => {
                // Read by the project page in front (taken aside, they were dropped): its
                // machine's pull requests are not this machine's of the same name (#1271).
                self.read_pulls(self.front.clone(), &details, cx);
                if let Some(page) = self.main.read(cx).project_page().cloned() {
                    page.update(cx, |page, cx| page.set_details(&details, cx));
                }
            }
            // The session opens only if the user is still on the page they started it from.
            Outcome::Started {
                project,
                agent,
                title,
            } => {
                if let Some(composer) = self.composer_of(&project).cloned()
                    && let Some(used) =
                        composer.update(cx, |composer, cx| composer.started(window, cx))
                {
                    self.last_pick = Some(used);
                }
                // Another machine's session is a row of the one list: it is there at once.
                let row = match self.answering.clone() {
                    None => agent.clone(),
                    Some(machine) => {
                        let row = crate::model::elsewhere_id(&machine, &agent);
                        self.started_there.push((
                            machine,
                            crate::machines::Listed {
                                id: agent.clone(),
                                title,
                                project: project.clone(),
                                state: "running".to_owned(),
                                exchanged_at: now_ms(),
                                created_at: now_ms(),
                                pilot: None,
                                pilot_elsewhere: None,
                            },
                        ));
                        row
                    }
                };
                // Only on the page shown: one of a machine not in front is not left meanwhile.
                if self.pages_shown() && self.selected == Some(Selection::Project(project)) {
                    if self.answering.is_some()
                        && let Some(last) = self.read.clone()
                    {
                        let merged = self.with_elsewhere(last, cx);
                        self.show(merged, window, cx);
                    }
                    self.open(Selection::Session(row), window, cx);
                }
            }
            Outcome::NotStarted { project, why } => {
                if let Some(composer) = self.composer_of(&project).cloned() {
                    composer.update(cx, |composer, cx| composer.not_started(&why, cx));
                }
            }
            Outcome::ImageRead {
                session,
                name,
                image,
            } => {
                // The page that asked: of the machine that answered, shown or kept.
                let machine = self.answering.clone();
                let page = self
                    .session_page
                    .iter()
                    .chain(self.kept_pages.values())
                    .find(|page| {
                        let page = page.read(cx);
                        page.machine() == machine.as_deref() && page.shown() == session
                    })
                    .cloned();
                if let Some(page) = page {
                    page.update(cx, |page, cx| page.picture_read(name, image, cx));
                }
            }
            Outcome::VaultAccepted { ask, name, refused } => {
                let toast = match refused {
                    // Refused (the Save's checks): Accept may be pressed again, or Edit.
                    Some(why) => {
                        self.dismissing.remove(&ask);
                        gpui_design_system::Toast::failure("The edit was not accepted").detail(why)
                    }
                    // Taken, the card folds away.
                    None => gpui_design_system::Toast::success(format!("Entry {name} saved")),
                };
                cx.notify();
                gpui_design_system::toast(toast, cx);
            }
            Outcome::VaultDismissed { ask, refused } => {
                if let Some(why) = refused {
                    // Refused: Dismiss may be pressed again. Taken, the card folds away.
                    self.dismissing.remove(&ask);
                    cx.notify();
                    gpui_design_system::toast(
                        gpui_design_system::Toast::failure("The request was not dismissed")
                            .detail(why),
                        cx,
                    );
                }
            }
            Outcome::QuestionAnswered {
                session,
                seq,
                refused,
            } => {
                // Kept by the window, not the page: a page opened again knows it was sent.
                if refused.is_none() {
                    crate::question_card::answers_sent(&session, seq, cx);
                }
                if let Some(page) = self
                    .session_page
                    .clone()
                    .filter(|page| page.read(cx).id() == session)
                {
                    page.update(cx, |page, cx| {
                        page.question_answered(seq, refused, window, cx)
                    });
                }
            }
            Outcome::Replied {
                session,
                key,
                answer,
            } => {
                if let Some(reply) = self.replies.get(&self.row_answered(&session)).cloned() {
                    reply.update(cx, |reply, cx| reply.replied(key, answer, cx));
                }
            }
            Outcome::Unqueued {
                session,
                delivery,
                edit,
                taken,
            } => {
                let queued = self
                    .session_page
                    .clone()
                    .filter(|page| page.read(cx).id() == session);
                let queued = queued.map(|page| page.read(cx).queued().clone());
                // Gone from the queue whether it was taken back, or the root read it first.
                let left = match &taken {
                    Ok(_) => true,
                    Err(why) => why == crate::queued::ALREADY_READ,
                };
                if let Some(queued) = queued {
                    queued.update(cx, |list, cx| list.answered(&delivery, left, cx));
                }
                match taken {
                    Ok((text, pictures)) if edit => {
                        if let Some(reply) = self.replies.get(&self.row_answered(&session)).cloned()
                        {
                            reply
                                .update(cx, |reply, cx| reply.restore(&text, pictures, window, cx));
                        }
                    }
                    Ok(_) => {}
                    Err(why) => gpui_design_system::toast(
                        crate::queued::refused_toast(&why, "The message was not taken back"),
                        cx,
                    ),
                }
            }
            Outcome::SentNow {
                session,
                delivery,
                refused,
            } => {
                let queued = self
                    .session_page
                    .clone()
                    .filter(|page| page.read(cx).id() == session);
                if let Some(queued) = queued.map(|page| page.read(cx).queued().clone()) {
                    let left = refused.as_deref() == Some(crate::queued::ALREADY_READ);
                    queued.update(cx, |list, cx| list.answered(&delivery, left, cx));
                }
                if let Some(why) = refused {
                    gpui_design_system::toast(
                        crate::queued::refused_toast(&why, "The message was not sent now"),
                        cx,
                    );
                }
            }
            Outcome::Stopped { session, refused } => {
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.answered(&session, refused.clone(), cx)
                });
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.end_answered(refused, cx));
                }
            }
            Outcome::EndingRead { session, read } => {
                // A read that failed lists nothing: the sentence says what ends.
                let led = read.map(|read| LedEnd::read(&read)).unwrap_or_default();
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.led_read(&session, led.clone(), cx)
                });
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.led_read(led, cx));
                }
            }
            Outcome::Resumed { session, refused } => {
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.answered(&session, refused.clone(), cx)
                });
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.resumed(refused, cx));
                }
            }
            Outcome::ProjectHidden { name, hide, result } => match result {
                Ok(hidden) => {
                    gpui_design_system::toast(
                        match hide {
                            true => gpui_design_system::Toast::success("Hidden on this machine")
                                .detail(format!("{name}: show it again in Settings, General.")),
                            false => gpui_design_system::Toast::success("Shown again")
                                .detail(format!("{name} is back in the sidebar.")),
                        },
                        cx,
                    );
                    // The daemon's list is the truth (another window may have hidden another).
                    if self.hidden != hidden {
                        self.hidden = hidden;
                        self.rearrange(window, cx);
                        self.hidden_changed(window, cx);
                    }
                }
                Err(why) => {
                    // What the window showed at once is taken back.
                    match hide {
                        true => self.hidden.retain(|hidden| *hidden != name),
                        false if !self.is_hidden(&name) => self.hidden.push(name.clone()),
                        false => {}
                    }
                    self.rearrange(window, cx);
                    self.hidden_changed(window, cx);
                    gpui_design_system::toast(
                        gpui_design_system::Toast::failure(match hide {
                            true => "Not hidden",
                            false => "Not shown again",
                        })
                        .detail(why),
                        cx,
                    );
                }
            },
            Outcome::HiddenRead(Some(hidden)) => {
                if self.hidden != hidden {
                    self.hidden = hidden;
                    self.rearrange(window, cx);
                    self.hidden_changed(window, cx);
                }
            }
            Outcome::HiddenRead(None) => {}
            Outcome::Renamed { refused } => gpui_design_system::toast(
                match refused {
                    None => gpui_design_system::Toast::success("Session renamed"),
                    Some(why) => {
                        gpui_design_system::Toast::failure("Session not renamed").detail(why)
                    }
                },
                cx,
            ),
            Outcome::Detached { from, refused } => gpui_design_system::toast(
                match refused {
                    None => {
                        gpui_design_system::Toast::success("Detached from ").quoted_title(&from)
                    }
                    Some(why) => gpui_design_system::Toast::failure("Not detached from ")
                        .quoted_title(&from)
                        .detail(why),
                },
                cx,
            ),
            Outcome::ToolsReloaded {
                session,
                after_turn,
                refused,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.tools_reloaded(after_turn, refused, cx));
                } else if let Some(why) = refused {
                    gpui_design_system::toast(
                        gpui_design_system::Toast::failure(crate::session::NOT_RELOADED)
                            .detail(why),
                        cx,
                    );
                }
            }
            Outcome::QuotaResumed { session, refused } => {
                if let Some(reply) = self.replies.get(&self.row_answered(&session)) {
                    reply.update(cx, |reply, cx| reply.quota_answered(refused, cx));
                }
            }
            Outcome::ModeSet { session, refused } => {
                if let Some(reply) = self.replies.get(&self.row_answered(&session)) {
                    let mode = reply.read(cx).mode.clone();
                    mode.update(cx, |mode, cx| mode.answered(refused, cx));
                }
            }
            Outcome::OptionsSet { session, refused } => {
                if let Some(reply) = self.replies.get(&self.row_answered(&session)) {
                    let options = reply.read(cx).options().clone();
                    options.update(cx, |options, cx| options.answered(refused, cx));
                }
            }
            Outcome::RunAnswered { session, answer } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.command_answered(answer, cx));
                }
            }
            // Asked once for each, unless the daemon could not: the next look asks again.
            Outcome::NotificationsRead { failed } => {
                if failed {
                    self.marked.clear();
                }
            }
            Outcome::SubscriptionCancelled { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.subscription_answered(refused, cx));
                }
            }
            Outcome::ExceptionWithdrawn { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.exception_answered(refused, cx));
                }
            }
            Outcome::LeaveWithdrawn { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.leave_answered(refused, cx));
                }
            }
            Outcome::Interrupted { session, refused } => {
                if let Some(reply) = self.replies.get(&self.row_answered(&session)).cloned() {
                    reply.update(cx, |reply, cx| reply.interrupted(refused, cx));
                }
            }
            // Only for its own session's page: a page opened since has nothing to fold, and the
            // session's page, opened again, reads the decision from the daemon.
            Outcome::Answered {
                session,
                number,
                decision,
                refused,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.answered(number, decision, refused, cx));
                }
            }
            Outcome::Allowing {
                session,
                number,
                at,
                step,
                why,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.allowing(number, at, step, why, cx));
                }
            }
            Outcome::Providers(read, Some(providers)) => self.set_providers(read, providers, cx),
            Outcome::Providers(_, None) => {}
            Outcome::JudgeSet { refused } => {
                if refused.is_none() {
                    self.actions.send(Command::ReadProviders);
                }
                self.settings
                    .update(cx, |page, cx| page.judge_answered(refused, cx));
            }
            Outcome::NewSessionSet { refused } => {
                if refused.is_none() {
                    self.actions.send(Command::ReadProviders);
                }
                self.settings
                    .update(cx, |page, cx| page.new_session_answered(refused, cx));
            }
            Outcome::NewSessionModeSet { refused } => {
                if refused.is_none() {
                    self.actions.send(Command::ReadProviders);
                }
                self.settings
                    .update(cx, |page, cx| page.new_session_mode_answered(refused, cx));
            }
            Outcome::ProviderSet { name, refused } => {
                // The change shows once the providers are read again: now, rather than only when
                // its event comes, since a change to what the file already said emits none.
                if refused.is_none() {
                    self.actions.send(Command::ReadProviders);
                }
                self.settings
                    .update(cx, |page, cx| page.answered(&name, refused, cx));
            }
            Outcome::ProjectSettings {
                project,
                read,
                saved,
            } => {
                if let Some(page) = self.project_settings.clone()
                    && page.read(cx).name() == project
                {
                    page.update(cx, |page, cx| page.answered(read, saved, window, cx));
                }
            }
            Outcome::Convention {
                project,
                key,
                saved,
            } => {
                let section = self
                    .project_settings
                    .clone()
                    .filter(|page| page.read(cx).name() == project)
                    .map(|page| page.read(cx).convention(key).clone());
                if let Some(section) = section {
                    section.update(cx, |section, cx| section.answered(saved, window, cx));
                }
            }
            Outcome::CommitChecks {
                project,
                read,
                saved,
            } => {
                let section = self
                    .project_settings
                    .clone()
                    .filter(|page| page.read(cx).name() == project)
                    .map(|page| page.read(cx).commit_checks().clone());
                if let Some(section) = section {
                    section.update(cx, |section, cx| section.answered(read, saved, window, cx));
                }
            }
            Outcome::MessageChecks { project, saved } => {
                let section = self
                    .project_settings
                    .clone()
                    .filter(|page| page.read(cx).name() == project)
                    .map(|page| page.read(cx).message_checks().clone());
                if let Some(section) = section {
                    section.update(cx, |section, cx| section.answered(saved, window, cx));
                }
            }
            Outcome::Repositories {
                project,
                read,
                serial,
            } => {
                // The project settings list them when they are open on the project; the project
                // page reads them otherwise.
                let section = self
                    .settings_of_project(&project, cx)
                    .map(|page| page.read(cx).repositories().clone())
                    .or_else(|| {
                        let page = self.main.read(cx).project_page().cloned();
                        page.filter(|page| page.read(cx).name() == project)
                            .map(|page| page.read(cx).repositories().clone())
                    });
                // An answer to an older request than the last is no news, for the form either.
                if section
                    .as_ref()
                    .is_some_and(|section| !section.read(cx).is_latest(serial))
                {
                    return;
                }
                if let Ok(value) = &read
                    && let Some(form) = self.new_workspace_of(&project).cloned()
                {
                    let rules = crate::repositories::rules_of(
                        &crate::repositories::Repositories::of(value),
                    );
                    form.update(cx, |form, cx| form.set_rules(rules, cx));
                }
                if let Some(section) = section {
                    section.update(cx, |section, cx| section.answered(serial, read, window, cx));
                }
            }
            Outcome::Programs {
                project,
                read,
                serial,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let checks = page.read(cx).programs().clone();
                    checks.update(cx, |checks, cx| checks.answered(serial, read, cx));
                }
            }
            // Detect again is the settings' own read of the repositories now.
            Outcome::ProjectChecked { .. } => {}
            Outcome::RemovalRead {
                project,
                serial,
                read,
            } => {
                if let Some(removal) = self.removal_of(&project, cx) {
                    removal.update(cx, |removal, cx| removal.answered(serial, read, cx));
                }
            }
            Outcome::ProjectRemoved { project, result } => {
                // The row of a project with no file asked it: no page is in front of it, and
                // none is left (the row folds away when the daemon no longer lists the project).
                let row_removal = self.removal_of_unconfigured(&project, cx);
                if let Some(removal) = &row_removal {
                    removal.update(cx, |removal, cx| removal.removed(result.clone(), cx));
                } else if let Some(page) = self.settings_of_project(&project, cx) {
                    let removal = page.read(cx).removal().clone();
                    removal.update(cx, |removal, cx| removal.removed(result.clone(), cx));
                } else if let Ok(done) = &result {
                    // The page was left meanwhile: the toast is said all the same.
                    let mut said = Toast::success(format!("The project {project} was removed"));
                    if let Some(detail) = crate::project_removal::done_said(done) {
                        said = said.detail(detail);
                    }
                    toast(said, cx);
                }
                // Another machine's project leaves that machine's copy, never this machine's list;
                // a page not shown is not left.
                if result.is_ok() && row_removal.is_none() {
                    self.forms.remove(&project);
                    let next = match self.answering.clone() {
                        None => {
                            self.snapshot.projects.retain(|p| p.name != project);
                            self.first_shown()
                        }
                        Some(machine) => self
                            .copies
                            .get_mut(&machine)
                            .and_then(|copy| copy.read.as_mut())
                            .and_then(|read| {
                                read.projects.retain(|p| p.name != project);
                                read.projects.first().map(|p| p.name.clone())
                            }),
                    };
                    // The machine in front's own project of that name.
                    if self.pages_shown() {
                        self.show_next(next, window, cx);
                    }
                }
            }
            Outcome::ProjectFolders { machine, read } => {
                self.add_project
                    .update(cx, |form, cx| form.folders_answered(machine, read, cx));
            }
            Outcome::ProjectDetected { seq, read, .. } => {
                self.add_project
                    .update(cx, |form, cx| form.detect_answered(seq, read, cx));
            }
            Outcome::ProjectAdded { read, .. } => {
                let added = self
                    .add_project
                    .update(cx, |form, cx| form.add_answered(read, window, cx));
                // Its settings, on the machine it was added on.
                if let Some(name) = added {
                    let machine = self.answering.clone();
                    cx.defer_in(window, move |shell, window, cx| {
                        shell.reach(machine, Selection::ProjectSettings(name), window, cx)
                    });
                }
            }
            Outcome::TicketPreview {
                project,
                ticket,
                read,
            } => {
                if let Some(form) = self.new_workspace_of(&project).cloned() {
                    form.update(cx, |form, cx| form.previewed(&ticket, read, cx));
                }
            }
            Outcome::TicketLinked {
                project,
                name,
                refused,
            } => {
                if let Some(page) = self.main.read(cx).project_page().cloned()
                    && page.read(cx).name() == project
                    && let Some(block) = page.read(cx).block(&Some(name.clone()), cx)
                {
                    block.update(cx, |block, cx| block.ticket_answered(refused.clone(), cx));
                    self.perform(Command::Details(project.clone()));
                }
                if let Some(page) = self.session_page.clone() {
                    page.update(cx, |page, cx| {
                        page.ticket_linked(&project, &name, refused, window, cx)
                    });
                }
            }
            Outcome::Commands { project, listed } => {
                // The `/` lists of the project's sessions.
                for reply in self.replies.values() {
                    reply.update(cx, |reply, cx| reply.commands_listed(&project, &listed, cx));
                }
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).commands().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::CommandRead {
                project,
                name,
                read,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).commands().clone();
                    section.update(cx, |section, cx| {
                        section.read_whole(&name, read, window, cx)
                    });
                }
            }
            Outcome::CommandWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).commands().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::SignIns(listed) => {
                let section = self.settings.read(cx).sign_in().clone();
                section.update(cx, |section, cx| section.listed(listed, cx));
            }
            Outcome::SignInChecked { program, answer } => {
                let section = self.settings.read(cx).sign_in().clone();
                section.update(cx, |section, cx| section.checked(&program, answer, cx));
            }
            Outcome::SignInStarted { program, answer } => {
                let section = self.settings.read(cx).sign_in().clone();
                section.update(cx, |section, cx| section.started(&program, answer, cx));
            }
            Outcome::SignInAnswered {
                program,
                what,
                refused,
            } => {
                let section = self.settings.read(cx).sign_in().clone();
                section.update(cx, |section, cx| {
                    section.answered(&program, what, refused, cx)
                });
            }
            Outcome::Vault { listed } => {
                // A project's Tracker offers the entries it sees to log time with.
                if let (Some(page), Ok(entries)) = (self.project_settings.clone(), &listed) {
                    page.update(cx, |page, cx| page.vault_listed(entries, cx));
                }
                let section = self.settings.read(cx).vault().clone();
                section.update(cx, |section, cx| section.listed(listed, cx));
            }
            Outcome::ShareMachines { listed } => {
                let section = self.settings.read(cx).vault().clone();
                section.update(cx, |section, cx| section.share_machines(listed, cx));
            }
            Outcome::VaultShared {
                name,
                machine,
                refused,
            } => {
                let section = self.settings.read(cx).vault().clone();
                section.update(cx, |section, cx| section.shared(name, machine, refused, cx));
            }
            Outcome::VaultWritten { refused } => {
                let section = self.settings.read(cx).vault().clone();
                section.update(cx, |section, cx| section.written(refused, window, cx));
                // An entry made for an agent's request to write a connection: its dialog, still
                // open under the entry's, reads the request again.
                // The request is the vault's machine's, whichever machine is in front.
                if let Some((number, machine)) =
                    section.update(cx, |section, _| section.take_request_return())
                {
                    self.open_connection_request_on(number, machine, cx);
                }
            }
            Outcome::Update { method, answer } if method.starts_with("whatsnew.") => {
                // What is new is asked through the update's command: it is the same kind of
                // request, answered by what `whatsnew.show` says.
                if let Ok(shown) = &answer {
                    self.whats_new_shown(shown, cx);
                }
            }
            Outcome::Update { method, answer } => {
                if let Ok(shown) = &answer {
                    self.update_shown(shown, cx);
                }
                self.version
                    .update(cx, |section, cx| section.answered(method, answer, cx));
            }
            Outcome::AllSettings { written, shown } => {
                let section = self.settings.read(cx).all_settings().clone();
                section.update(cx, |section, cx| {
                    section.answered(shown, written, window, cx)
                });
            }
            Outcome::About { written, shown } => {
                let editor = self.settings.read(cx).about().clone();
                editor.update(cx, |editor, cx| editor.answered(shown, written, window, cx));
            }
            Outcome::Developer { written, shown } => {
                self.settings
                    .update(cx, |page, cx| page.developer_answered(shown, written, cx));
            }
            Outcome::Findings(listed) => {
                self.settings
                    .update(cx, |page, cx| page.findings_answered(listed, cx));
            }
            Outcome::FindingsExported(done) => {
                self.settings
                    .update(cx, |page, cx| page.findings_exported(done, cx));
            }
            Outcome::Tunnel(news) => self.tunnel_news(news, cx),
            Outcome::Asked {
                then: crate::extensions::Then::Connections(then),
                answer,
            } => {
                let section = self.settings.read(cx).connections().clone();
                section.update(cx, |section, cx| section.answered(then, answer, window, cx));
            }
            Outcome::Asked {
                then: crate::extensions::Then::ConnectionRequest(then),
                answer,
            } => {
                // An accepted request whose write succeeded and leaves the connection waiting for
                // its review leads into that review at once (#1031): the daemon's answer says so,
                // as the form's Save learns it. A write that failed opens nothing.
                let review = match (&then, &answer) {
                    (
                        crate::extensions::connection_request::Then::Decided {
                            accepted: true, ..
                        },
                        Ok(answer),
                    ) if answer["connection"]["review"] == true && answer["outcome"] == "done" => {
                        answer["connection"]["name"].as_str().map(str::to_owned)
                    }
                    _ => None,
                };
                // The dialog is the request's machine's: another's answer is not its own.
                let dialog = self.connection_request.clone();
                if dialog.read(cx).machine() == self.answering.as_deref() {
                    dialog.update(cx, |dialog, cx| dialog.answered(then, answer, cx));
                    if let Some(name) = review {
                        self.review_connection(&name, cx);
                    }
                }
            }
            Outcome::Asked {
                then: crate::extensions::Then::Findings(crate::findings::Then::Filed { id }),
                answer,
            } => {
                let filing = self.settings.read(cx).filing().clone();
                filing.update(cx, |filing, cx| filing.answered(id, answer, cx));
            }
            Outcome::Asked {
                then: crate::extensions::Then::Check(then),
                answer,
            } => {
                let check = self.settings.read(cx).check().clone();
                check.update(cx, |check, cx| check.answered(then, answer, cx));
            }
            Outcome::Asked {
                then: crate::extensions::Then::Tunnels(then),
                answer,
            } => self
                .services
                .update(cx, |page, cx| page.tunnels_answered(then, answer, cx)),
            Outcome::Asked { then, answer } => {
                let page = match &then {
                    crate::extensions::Then::Specs(specs) => specs.for_page(),
                    _ => None,
                };
                match (page, self.spec_page.clone()) {
                    (Some(spec), Some(open)) if open.read(cx).spec() == spec => {
                        let crate::extensions::Then::Specs(then) = then else {
                            unreachable!("a page's answer is a spec's")
                        };
                        open.update(cx, |page, cx| page.answered(then, answer, cx));
                    }
                    _ => self
                        .extensions
                        .update(cx, |registry, cx| registry.answered(then, answer, cx)),
                }
            }
            // A read that failed is not "no draft": the composer writes nothing, and the read is
            // asked again.
            Outcome::Draft { key, found } => match key {
                DraftKey::Session { agent: id, .. } => {
                    if let Some(reply) = self.replies.get(&self.row_answered(&id)).cloned() {
                        match found {
                            Ok(found) => {
                                reply.update(cx, |reply, cx| reply.draft_read(found, window, cx))
                            }
                            Err(_) => reply.update(cx, |reply, cx| reply.draft_read_failed(cx)),
                        }
                    }
                }
                DraftKey::Project { name, .. } => {
                    let composer = self.forms.get(&name).map(|(composer, _)| composer.clone());
                    if let Some(composer) = composer {
                        match found {
                            Ok(found) => composer
                                .update(cx, |composer, cx| composer.draft_read(found, window, cx)),
                            Err(_) => {
                                composer.update(cx, |composer, cx| composer.draft_read_failed(cx))
                            }
                        }
                    }
                }
            },
            Outcome::DraftWritten { key, result } => match key {
                DraftKey::Session { agent: id, .. } => {
                    if let Some(reply) = self.replies.get(&self.row_answered(&id)).cloned() {
                        reply.update(cx, |reply, cx| reply.draft_written(result, cx));
                    }
                }
                DraftKey::Project { name, .. } => {
                    let composer = self.forms.get(&name).map(|(composer, _)| composer.clone());
                    if let Some(composer) = composer {
                        composer.update(cx, |composer, cx| composer.draft_written(result, cx));
                    }
                }
            },
            Outcome::WindowKept { value, result, .. } => self.kept_written(value, result, cx),
            Outcome::ArrangementKept {
                change,
                arrangement,
                result,
            } => self.arrangement_answered(change, arrangement, result, cx),
            Outcome::ToolResults { written, shown } => {
                self.settings.update(cx, |page, cx| {
                    page.tool_results_answered(shown, written, cx)
                });
            }
            Outcome::Sessions { written, shown } => {
                self.settings
                    .update(cx, |page, cx| page.sessions_answered(shown, written, cx));
            }
            // Only the answer to the latest request counts: an older one would take the window
            // back to a choice the user has since changed.
            // Every machine's Settings ask through one count: a read of another machine's that
            // this one's overtook still lets its Settings know what the window keeps, or their
            // choice would stay held (#1271). This machine's own wait for its latest answer.
            Outcome::Appearance {
                seq,
                written: false,
                shown: Ok(appearance),
            } if seq != self.appearance_asked && self.answering.is_some() => {
                if !self.settings.read(cx).appearance_known() {
                    self.settings.update(cx, |page, cx| {
                        page.appearance_answered(Ok(appearance), false, cx)
                    });
                }
            }
            Outcome::Appearance { seq, .. } if seq != self.appearance_asked => {}
            Outcome::Appearance { written, shown, .. } => {
                match &shown {
                    Ok(appearance) => {
                        gpui_design_system::appearance::appear(appearance.clone(), cx);
                        // What the daemon keeps is the copy the next first frame is drawn in.
                        crate::look::remember(appearance, cx);
                    }
                    // A choice not kept: the window goes back to what the daemon keeps.
                    Err(_) if written => self.ask_appearance(None),
                    Err(_) => {}
                }
                self.settings
                    .update(cx, |page, cx| page.appearance_answered(shown, written, cx));
            }
            Outcome::Compaction {
                written,
                guidance,
                shown,
            } => {
                let compaction = shown
                    .as_ref()
                    .map(crate::settings::Compaction::of)
                    .map_err(Clone::clone);
                if let Ok(shown) = &compaction {
                    self.compaction_picked = shown.picked;
                    self.compactor_shown = shown
                        .compactor
                        .clone()
                        .map(|(provider, model, _)| (provider, model));
                }
                if guidance || !written {
                    let editor = self.settings.read(cx).compaction_guidance().clone();
                    let said = shown
                        .as_ref()
                        .map(|shown| crate::rules::Shown::of(&shown["guidance"]))
                        .map_err(Clone::clone);
                    editor.update(cx, |editor, cx| editor.answered(said, guidance, window, cx));
                }
                if !guidance {
                    self.settings.update(cx, |page, cx| {
                        page.compaction_answered(compaction, written, cx)
                    });
                }
                cx.notify();
            }
            Outcome::Compacting {
                session,
                after_turn,
                refused,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.compacting(after_turn, refused, cx));
                } else if let Some(why) = refused {
                    gpui_design_system::toast(
                        gpui_design_system::Toast::failure(crate::session::NOT_COMPACTED)
                            .detail(why),
                        cx,
                    );
                }
            }
            Outcome::Guidance {
                name,
                part,
                written,
                shown,
            } => {
                let registry = self.extensions.read(cx);
                let editor = match &part {
                    Some(part) => registry.check_guidance(&name, part),
                    None => registry.guidance(&name),
                }
                .cloned();
                if let Some(editor) = editor {
                    editor.update(cx, |editor, cx| editor.answered(shown, written, window, cx));
                }
            }
            Outcome::JudgeGuidance { written, shown } => {
                let editor = self.settings.read(cx).judge_guidance().clone();
                editor.update(cx, |editor, cx| editor.answered(shown, written, window, cx));
            }
            Outcome::Rules {
                project,
                written,
                shown,
            } => {
                let editor = match &project {
                    None => Some(self.rules.clone()),
                    Some(name) => self
                        .project_settings
                        .clone()
                        .filter(|page| page.read(cx).name() == name)
                        .map(|page| page.read(cx).rules().clone()),
                };
                if let Some(editor) = editor {
                    editor.update(cx, |editor, cx| editor.answered(shown, written, window, cx));
                }
            }
            Outcome::RulesSwitched { project, shown } => {
                let editor = self
                    .project_settings
                    .clone()
                    .filter(|page| page.read(cx).name() == project)
                    .map(|page| page.read(cx).rules().clone());
                if let Some(editor) = editor {
                    editor.update(cx, |editor, cx| editor.switched(shown, cx));
                }
            }
            Outcome::WorkspacesFolder {
                project,
                written,
                shown,
            } => {
                let section = match &project {
                    None => Some(self.settings.read(cx).workspaces_folder().clone()),
                    Some(name) => self
                        .project_settings
                        .clone()
                        .filter(|page| page.read(cx).name() == name)
                        .map(|page| page.read(cx).workspaces_folder().clone()),
                };
                if let Some(section) = section {
                    section.update(cx, |section, cx| {
                        section.answered(shown, written, window, cx)
                    });
                }
            }
            Outcome::TravelledFolder { written, shown } => {
                let section = self.settings.read(cx).travelled_folder().clone();
                section.update(cx, |section, cx| {
                    section.answered(shown, written, window, cx)
                });
            }
            Outcome::Skills { project, listed } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).skills().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::SkillRead {
                project,
                name,
                read,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).skills().clone();
                    section.update(cx, |section, cx| {
                        section.read_whole(&name, read, window, cx)
                    });
                }
            }
            Outcome::ClaudeFound {
                project,
                kind,
                found,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    match kind {
                        crate::actions::ClaudeKind::Skill => {
                            let section = page.read(cx).skills().clone();
                            section.update(cx, |section, cx| section.found_listed(found, cx));
                        }
                        crate::actions::ClaudeKind::Command => {
                            let section = page.read(cx).commands().clone();
                            section.update(cx, |section, cx| section.found_listed(found, cx));
                        }
                    }
                }
            }
            Outcome::ClaudeImported {
                project,
                kind,
                read,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    match kind {
                        crate::actions::ClaudeKind::Skill => {
                            let section = page.read(cx).skills().clone();
                            section.update(cx, |section, cx| section.imported(read, cx));
                        }
                        crate::actions::ClaudeKind::Command => {
                            let section = page.read(cx).commands().clone();
                            section.update(cx, |section, cx| section.imported(read, cx));
                        }
                    }
                }
            }
            Outcome::SkillWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).skills().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::Tasks { project, listed } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).tasks().clone();
                    section.update(cx, |section, cx| section.listed(listed.clone(), cx));
                }
                // The rail of a session of the project offers them to run.
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).project() == project
                    && let Ok(tasks) = &listed
                {
                    let names = tasks.iter().map(|task| task.name.clone()).collect();
                    let list = page.read(cx).commands(cx);
                    list.update(cx, |list, cx| list.set_tasks(names, cx));
                }
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| tasks.listed(listed, cx));
                }
            }
            Outcome::Items {
                project,
                asked,
                read,
            } => {
                if let Some(items) = self.project_items_of(&project, cx) {
                    items.update(cx, |items, cx| items.listed(asked, read, cx));
                }
            }
            Outcome::Progress {
                project,
                asked,
                read,
            } => {
                if let Some(page) = self.main.read(cx).progress_page(cx)
                    && page.read(cx).project() == project
                {
                    page.update(cx, |page, cx| page.listed(asked, read, cx));
                }
            }
            Outcome::Item {
                project,
                kind,
                key,
                repository,
                asked,
                shown,
            } => {
                if let Some(items) = self.project_items_of(&project, cx) {
                    items.update(cx, |items, cx| {
                        items.shown(&kind, &key, repository, asked, shown, cx)
                    });
                }
            }
            Outcome::TaskWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).tasks().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::TaskRun {
                project,
                name,
                from,
                refused,
            } => match from {
                RunFrom::Page { .. } => {
                    if let Some(tasks) = self.project_tasks_of(&project, cx) {
                        tasks.update(cx, |tasks, cx| tasks.started(refused, cx));
                    }
                }
                RunFrom::Settings => crate::tasks::ran(&name, None, refused, cx),
                RunFrom::Session { workspace } => {
                    crate::tasks::ran(&name, Some(workspace.as_deref()), refused, cx);
                }
            },
            Outcome::ProjectRuns {
                project,
                listed,
                output,
            } => {
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| {
                        tasks.runs_listed(listed, output.map(|o| *o), cx)
                    });
                }
            }
            Outcome::ProjectRunAnswered { project, answer } => {
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| tasks.run_answered(answer, cx));
                }
            }
            Outcome::Services { listed } => {
                // The open task form offers them as what a task may need, or says why it cannot.
                if let Some(page) = self.project_settings.clone() {
                    let names = listed
                        .as_ref()
                        .map(|services| {
                            services
                                .iter()
                                .map(|s| (s.name.clone(), s.line.clone()))
                                .collect()
                        })
                        .map_err(Clone::clone);
                    let section = page.read(cx).tasks().clone();
                    section.update(cx, |section, cx| section.set_services(names, cx));
                }
                self.services.update(cx, |page, cx| page.listed(listed, cx));
            }
            Outcome::ServiceWritten { refused } => {
                self.services
                    .update(cx, |page, cx| page.written(refused, cx));
            }
            Outcome::ServiceActed {
                name,
                start,
                answer,
                needed,
            } => {
                self.services
                    .update(cx, |page, cx| page.acted(&name, start, answer, needed, cx));
            }
            Outcome::ServiceOutput { output } => {
                self.services
                    .update(cx, |page, cx| page.output(output.map(|o| *o), cx));
            }
            Outcome::Setup { project, listed } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).setup().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::SetupWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).setup().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::WorkspaceDone { project, name } => {
                self.workspace_answered(&project, &name, None, window, cx)
            }
            Outcome::WorkspaceRefused { project, name, why } => {
                self.workspace_answered(&project, &name, Some(&why), window, cx)
            }
        }
    }

    /// The pull requests and tickets of the open project page, when it is `project`'s.
    fn project_items_of(
        &self,
        project: &str,
        cx: &App,
    ) -> Option<Entity<crate::items::ProjectItems>> {
        let page = self.main.read(cx).project_page()?.read(cx);
        (page.name() == project).then(|| page.items().clone())
    }

    /// The tasks of the open project page, when it is `project`'s.
    fn project_tasks_of(
        &self,
        project: &str,
        cx: &App,
    ) -> Option<Entity<crate::tasks::ProjectTasks>> {
        let page = self.main.read(cx).project_page()?.read(cx);
        (page.name() == project).then(|| page.tasks().clone())
    }

    /// The confirmation that asked to remove `project`: its settings page's, or its row's when its
    /// settings file is gone.
    fn removal_of(&self, project: &str, cx: &App) -> Option<Entity<ProjectRemoval>> {
        match self.settings_of_project(project, cx) {
            Some(page) => Some(page.read(cx).removal().clone()),
            None => self.removal_of_unconfigured(project, cx),
        }
    }

    /// The confirmation on the row of `project`, a project with no settings file: this machine's,
    /// so none for an answer of another machine, whose project of that name is its own.
    fn removal_of_unconfigured(&self, project: &str, cx: &App) -> Option<Entity<ProjectRemoval>> {
        if self.answering.is_some() {
            return None;
        }
        self.gone
            .iter()
            .filter(|((name, _), _)| name == project)
            .find_map(|(_, node)| node.read(cx).removal().cloned())
    }

    /// The open settings page, when it is `project`'s.
    fn settings_of_project(&self, project: &str, cx: &App) -> Option<Entity<ProjectSettingsPage>> {
        self.project_settings
            .clone()
            .filter(|page| page.read(cx).name() == project)
    }

    fn workspace_answered(
        &mut self,
        project: &str,
        name: &str,
        refused: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.new_workspace_of(project).cloned()
            && form.read(cx).creating() == Some(name)
        {
            let for_composer = form.update(cx, |form, cx| form.answered(refused, window, cx));
            if for_composer && let Some(composer) = self.composer_of(project).cloned() {
                composer.update(cx, |composer, cx| {
                    composer.choose_when_listed(name.to_owned(), cx)
                });
            }
        } else if let Some(page) = self.main.read(cx).project_page().cloned()
            && page.read(cx).name() == project
            && let Some(block) = page.read(cx).block(&Some(name.to_owned()), cx)
        {
            block.update(cx, |block, cx| block.answered(refused, cx));
        } else if let Some(page) = self.session_page.clone() {
            // A removal asked from the session's rail.
            page.update(cx, |page, cx| {
                page.removal_answered(project, name, refused, cx)
            });
        }
    }

    /// Brings the open project's page up to what the daemon last gave, and asks for its details
    /// again.
    fn refresh_project(&mut self, cx: &mut Context<Self>) {
        self.feed_progress(cx);
        let Some(page) = self.main.read(cx).project_page().cloned() else {
            // A page opened later is a new one: it asks for its details.
            self.details_of = None;
            return;
        };
        let name = page.read(cx).name().to_owned();
        // Its row in the projects' list, whose name may be another machine's (#1410).
        let row = self.row_of(self.front.as_deref(), &name);
        // Another machine's project page reads what was read of it.
        let read = self.pages().clone();
        let Some(project) = read.projects.iter().find(|p| p.name == name) else {
            return;
        };
        let work: Work = (
            project.clone(),
            read.workspaces
                .iter()
                .filter(|w| w.project == name)
                .map(|w| (w.name.clone(), w.preparation.clone()))
                .collect(),
            read.sessions
                .iter()
                .filter(|s| s.project == name)
                .map(|s| (s.id.clone(), s.mark))
                .collect(),
        );
        let sessions_of: Vec<&crate::model::Session> = read
            .sessions
            .iter()
            .filter(|s| s.project == name && !s.ended)
            .collect();
        let line = |session: &crate::model::Session, cx: &App| {
            // The sessions it leads, and the one that leads it: each said once, where it works.
            let leads = sessions_of
                .iter()
                .filter(|led| led.pilot.as_deref() == Some(session.id.as_str()))
                .map(|led| (led.id.clone(), led.place.clone()))
                .collect();
            let pilot = session.pilot.as_ref().map(|pilot| {
                let title = sessions_of
                    .iter()
                    .find(|s| &s.id == pilot)
                    .map(|s| s.title.clone())
                    .or_else(|| session.led_by.clone())
                    .unwrap_or_else(|| pilot.clone());
                (pilot.clone(), title)
            });
            let question = (session.mark == Some(crate::state::AgentState::NeedsYou))
                .then(|| {
                    read.bell
                        .iter()
                        .rev()
                        .find(|item| {
                            item.needs_you
                                && !item.resolved
                                && item.session.as_deref() == Some(session.id.as_str())
                        })
                        .map(|item| item.detail.clone())
                })
                .flatten();
            // Another machine's session is known here by its row of the one list.
            let id = match &self.front {
                Some(front) => crate::model::elsewhere_id(front, &session.id),
                None => session.id.clone(),
            };
            self.sessions.get(&id).map(|row| SessionLine {
                id: id.clone(),
                title: session.title.clone(),
                agents: session.agents,
                mark: session.mark,
                active_at: session.active_at,
                status: row.read(cx).status(),
                leads,
                pilot,
                question,
            })
        };
        let mut places = vec![PlaceData {
            place: None,
            meta: None,
            folder: project.folder.clone(),
            steps: Vec::new(),
            total_ms: None,
            made: Vec::new(),
            preparation: Preparation::Ready,
            sessions: read
                .sessions_in(&name, &None)
                .filter(|s| !s.ended)
                .filter_map(|s| line(s, cx))
                .collect(),
        }];
        let mut choices = vec![PlaceChoice {
            workspace: None,
            ready: true,
        }];
        // Work lists the workspaces from the newest to the oldest, in an order nothing changes.
        let mut workspaces: Vec<&crate::model::Workspace> = read
            .workspaces
            .iter()
            .filter(|w| w.project == name)
            .collect();
        workspaces.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        for workspace in workspaces {
            let place = Some(workspace.name.clone());
            places.push(PlaceData {
                meta: Some(format!("from {}", workspace.bases.join(", "))),
                folder: workspace.folder.clone(),
                steps: workspace.steps.clone(),
                total_ms: workspace.total_ms,
                made: workspace.made.clone(),
                preparation: workspace.preparation.clone(),
                sessions: read
                    .sessions_in(&name, &place)
                    .filter(|s| !s.ended)
                    .filter_map(|s| line(s, cx))
                    .collect(),
                place,
            });
            choices.push(PlaceChoice {
                workspace: Some(workspace.name.clone()),
                ready: workspace.preparation == Preparation::Ready,
            });
        }
        // The composer's place menu keeps its order by name.
        choices[1..].sort_by(|a, b| a.workspace.cmp(&b.workspace));
        let data = ProjectData {
            name: name.clone(),
            folder: project.folder.clone(),
            repositories: project.repositories.clone(),
            places,
            tracked: project.tracked,
            machines: self.machine_sessions(&row, cx),
        };
        page.update(cx, |page, cx| page.set(data, cx));
        let default = (project.provider.clone(), project.model.clone());
        let asking = match self.start_on(&row, cx) {
            Start::Ask { machines, .. } => Some(format!(
                "Choose where it runs: {}.",
                crate::machines::said_or(&machines)
            )),
            Start::On(_) | Start::Nowhere { .. } => None,
        };
        if let Some(composer) = self.composer_of(&name).cloned() {
            let (providers, last) = (&self.providers, self.last_pick.as_ref());
            composer.update(cx, |composer, cx| {
                composer.set_asking(asking, cx);
                composer.set_places(choices, cx);
                composer.set_providers(
                    providers,
                    (default.0.as_deref(), default.1.as_deref()),
                    last,
                    cx,
                );
            });
        }
        if self
            .details_of
            .as_ref()
            .is_none_or(|(asked, then)| *asked != name || *then != work)
        {
            self.details_of = Some((name.clone(), work));
            self.details_asked += 1;
            self.actions.send(Command::Details(name));
        }
    }

    /// The sessions of `project` on the machines whose page of it is not shown, as the one list
    /// has them (`docs/remote.md`, "One list of every machine"): one block a machine, in the order
    /// the project's row marks them, this machine's own while another machine's page is shown.
    fn machine_sessions(&self, project: &str, cx: &App) -> Vec<crate::project::MachineSessions> {
        let list = &self.snapshot;
        let Some(row) = list.projects.iter().find(|p| p.name == project) else {
            return Vec::new();
        };
        let this = list.this_machine.clone().unwrap_or_default();
        let line = |id: &str, title: &str, mark, active_at, cx: &App| {
            self.sessions.get(id).map(|row| SessionLine {
                id: id.to_owned(),
                title: title.to_owned(),
                agents: 1,
                mark,
                active_at,
                status: row.read(cx).status(),
                leads: Vec::new(),
                pilot: None,
                question: None,
            })
        };
        let mut blocks = Vec::new();
        for home in &row.homes {
            let shown = match (&self.front, home.here) {
                (None, here) => here,
                (Some(front), _) => *front == home.machine,
            };
            if shown {
                continue;
            }
            let mut sessions: Vec<SessionLine> = match home.here {
                true => list
                    .sessions
                    .iter()
                    .filter(|s| s.project == project && !s.ended && s.pilot.is_none())
                    .filter_map(|s| line(&s.id, &s.title, s.mark, s.active_at, cx))
                    .collect(),
                false => list
                    .elsewhere
                    .iter()
                    .filter(|s| s.project == project && s.pilot.is_none())
                    .filter(|s| {
                        s.elsewhere
                            .as_ref()
                            .is_some_and(|there| there.listed && there.machine == home.machine)
                    })
                    .filter_map(|s| line(&s.id, &s.title, s.mark, s.active_at, cx))
                    .collect(),
            };
            // A machine with no session of it says nothing.
            if sessions.is_empty() {
                continue;
            }
            sessions.sort_by_key(|a| std::cmp::Reverse(a.active_at));
            blocks.push(crate::project::MachineSessions {
                machine: home.machine.clone(),
                icon: home.icon.clone(),
                colour: home.colour.clone(),
                state: match home.here || home.machine == this {
                    true => "this machine".to_owned(),
                    false => home.state.clone(),
                },
                offline: home.offline,
                sessions,
            });
        }
        blocks
    }

    /// The providers as the daemon lists them, to the settings page and to every composer; an
    /// answer to a read asked before the last one applied is older than it, and dropped.
    fn set_providers(&mut self, read: u64, providers: Providers, cx: &mut Context<Self>) {
        if read < self.providers_read {
            return;
        }
        self.providers_read = read;
        self.settings
            .update(cx, |page, cx| page.set_providers(&providers, cx));
        // A provider ready since may be the compactor Flottille picks: the daemon says it.
        self.actions.send(Command::CompactionRead);
        let chosen = providers.judge.is_some();
        for reply in self.replies.values() {
            let mode = reply.read(cx).mode.clone();
            mode.update(cx, |mode, cx| mode.set_judge_chosen(chosen, cx));
        }
        for (name, (composer, _)) in &self.forms {
            let project = self.pages().projects.iter().find(|p| &p.name == name);
            let default = (
                project.and_then(|p| p.provider.as_deref()),
                project.and_then(|p| p.model.as_deref()),
            );
            let last = self.last_pick.as_ref();
            composer.update(cx, |composer, cx| {
                composer.set_providers(&providers, default, last, cx)
            });
        }
        if let Some(page) = self.session_page.clone() {
            page.update(cx, |page, cx| page.set_providers(&providers, cx));
        }
        // The notice of a judge Flottille picked comes and goes with them.
        let notice = self.judge_notice();
        self.providers = providers;
        if self.judge_notice() != notice {
            cx.notify();
        }
    }

    /// The providers as the daemon last listed them.
    pub fn providers(&self) -> &Providers {
        &self.providers
    }

    /// How many times the open page's details were asked for: the tests count them.
    pub fn details_asked(&self) -> usize {
        self.details_asked
    }

    /// Asks the daemon for what the settings page edits.
    fn read_settings(&mut self) {
        self.actions.send(Command::RulesRead { project: None });
        self.actions.send(Command::AboutRead);
        self.actions.send(Command::AllSettingsRead);
        self.actions.send(Command::JudgeRead);
        self.actions.send(Command::CompactionRead);
        self.actions.send(Command::DeveloperRead);
        self.actions.send(Command::FindingsRead);
        self.actions.send(Command::ToolResultsRead);
        self.actions.send(Command::SessionsRead);
        self.actions
            .send(Command::WorkspacesFolderRead { project: None });
        self.actions.send(Command::TravelledFolderRead);
        self.actions.send(Command::Vault);
        self.actions.send(Command::SignIns);
        // Hidden here, by this window or by `flottille project hide` since it opened.
        self.perform_here(Command::HiddenRead);
        self.ask_appearance(None);
    }

    /// Reads again what the window shows of the daemon's settings, once the link listens to the
    /// events: a page opened later reads for itself.
    fn read_texts(&mut self, cx: &mut Context<Self>) {
        if self.selected == Some(Selection::Settings) {
            self.read_settings();
        } else {
            self.ask_appearance(None);
        }
        if let Some(name) = self
            .project_settings
            .as_ref()
            .map(|page| page.read(cx).name().to_owned())
        {
            self.actions.send(Command::RulesRead {
                project: Some(name.clone()),
            });
            self.actions.send(Command::WorkspacesFolderRead {
                project: Some(name),
            });
        }
    }

    /// This window's build (its `--version`) and how it starts again from its binary, given the
    /// page it shows: what the binary gives; a window given none never starts again.
    pub fn set_build(&mut self, full: String, reexec: Reexec) {
        self.build = Some((full, reexec));
    }

    /// The page to open once the daemon's first read is in, as [`Shell::page_name`] names it.
    pub fn reopen(&mut self, page: &str) {
        self.reopen = page_named(page);
    }

    /// The page shown, named for [`Shell::reopen`]: `session:<id>`, `project:<name>`,
    /// `settings:<tab>`, `project-settings:<name>`, `spec:<n>:<project>`, `services`.
    pub fn page_name(&self, cx: &gpui_kit::App) -> Option<String> {
        Some(match self.selected.as_ref()? {
            Selection::Session(id) => format!("session:{id}"),
            Selection::Project(name) => format!("project:{name}"),
            Selection::Settings => format!("settings:{}", self.settings.read(cx).tab()),
            Selection::ProjectSettings(name) => format!("project-settings:{name}"),
            Selection::Spec { project, spec } => format!("spec:{spec}:{project}"),
            Selection::Services => "services".to_owned(),
            Selection::WhatsNew => "whats-new".to_owned(),
        })
    }

    pub fn version(&self) -> &Entity<crate::version::VersionSection> {
        &self.version
    }

    /// What the daemon says of an update: the Version section and the sidebar's foot show it,
    /// and a daemon of another build than this window's has the window start again from the
    /// binary that replaced it, on the page it shows.
    fn update_shown(&mut self, value: &serde_json::Value, cx: &mut Context<Self>) {
        let shown = crate::version::Shown::of(value);
        let foot = shown.foot_line();
        let build = shown.build_line();
        let full = shown.full.clone();
        self.version
            .update(cx, |section, cx| section.set_shown(shown, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_version_line(foot, cx));
        self.whats_new
            .update(cx, |page, cx| page.set_build(Some(build), cx));
        let Some((own, reexec)) = self.build.clone() else {
            return;
        };
        if full.is_empty() || full == own || self.reexecuted {
            return;
        }
        self.reexecuted = true;
        let page = self.page_name(cx);
        // What the composers hold reaches the daemon before this process becomes another.
        self.closing(cx);
        if !reexec(page) {
            // No newer file at the binary's path: this window goes on as it is.
            self.closed = false;
        }
    }

    /// What the daemon says is new since the version last run: the page, the sidebar's mark and
    /// the Version section follow it. Never opens the page: the user does.
    fn whats_new_shown(&mut self, value: &serde_json::Value, cx: &mut Context<Self>) {
        let notes = crate::whats_new::Notes::of(value);
        let (mark, since) = (notes.mark(), notes.since.clone());
        self.whats_new
            .update(cx, |page, cx| page.set_notes(notes, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_whats_new(mark, cx));
        self.version
            .update(cx, |section, cx| section.set_whats_new(since, cx));
    }

    /// What "Restart Flottille" runs, in place of `flottille daemon stop`.
    pub fn set_restart(&mut self, restart: Arc<dyn Fn() + Send + Sync>) {
        self.restart = restart;
    }

    /// Restarts a Flottille that does not answer, off the UI's thread: what it ran is ended, and
    /// the link, its connection broken, starts it again. Its button says it runs, and cannot be
    /// clicked again, until it ended.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if self.main.read(cx).restarting() {
            return;
        }
        let restart = self.restart.clone();
        // Read on the UI's clock: a thread of its own does not wake the UI's tasks.
        let ended = Arc::new(AtomicBool::new(false));
        let ending = ended.clone();
        let started = std::thread::Builder::new()
            .name("flottille-restart".into())
            .spawn(move || {
                restart();
                ending.store(true, Ordering::SeqCst);
            });
        if let Err(error) = started {
            eprintln!("flottille: Flottille cannot be restarted: {error}");
            return;
        }
        self.main
            .update(cx, |main, cx| main.set_restarting(true, cx));
        cx.spawn(async move |this: WeakEntity<Shell>, cx| {
            while !ended.load(Ordering::SeqCst) {
                cx.background_executor().timer(RESTART_POLL).await;
            }
            let _ = this.update(cx, |shell, cx| {
                shell
                    .main
                    .update(cx, |main, cx| main.set_restarting(false, cx))
            });
        })
        .detach();
    }

    /// Whether the parts that show what the daemon says are waiting for it: they show skeletons
    /// until it answered, and stop when it cannot be reached (the top bar says it reconnects).
    fn waiting_for_the_daemon(&mut self, waiting: bool, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_loading(waiting, cx));
        self.main
            .update(cx, |main, cx| main.set_reachable(waiting, cx));
    }

    fn set_online(&mut self, online: bool, cx: &mut Context<Self>) {
        if self.top.read(cx).online != online {
            self.top.update(cx, |top, cx| top.set_online(online, cx));
        }
    }

    /// What a project page read of its workspaces' pull requests: their sessions' rows say the
    /// first of each on their second line. A workspace the page could not read keeps what it had.
    fn read_pulls(&mut self, machine: Option<String>, details: &Details, cx: &mut Context<Self>) {
        for (name, pulls) in &details.pulls {
            let key = (machine.clone(), details.project.clone(), name.clone());
            match pulls.first() {
                Some(pull) => self.pulls.insert(key, pull.clone()),
                None => self.pulls.remove(&key),
            };
        }
        for session in &self.snapshot.sessions {
            let Some(row) = self.sessions.get(&session.id) else {
                continue;
            };
            let pull = self.pull_of(session, &session.place).cloned();
            if row.read(cx).pull() != pull.as_ref() {
                row.update(cx, |row, cx| row.set_pull(pull.as_ref(), cx));
            }
        }
    }

    /// The pull request of a session: the daemon's, with what it is about, else the one of the
    /// place it works in, when a project page read it. A session of another machine's list says
    /// its own machine's.
    fn pull_of<'a>(
        &'a self,
        session: &'a crate::model::Session,
        place: &Place,
    ) -> Option<&'a PullRequest> {
        if let Some(pull) = session.subject.as_ref().and_then(|s| s.pull.as_ref()) {
            return Some(pull);
        }
        let workspace = place.as_ref()?;
        let machine = session
            .elsewhere
            .as_ref()
            .filter(|there| !there.here)
            .map(|there| there.machine.clone());
        self.pulls
            .get(&(machine, session.project.clone(), workspace.clone()))
    }

    /// The rows of `listed`, each the one the tree had or a new one, brought up to date; their
    /// marks are added to `marks`, and each row to `rows`.
    fn session_rows<'a>(
        &mut self,
        listed: impl Iterator<Item = &'a crate::model::Session>,
        homes: &[crate::model::Home],
        rows: &mut HashMap<String, Entity<SessionRow>>,
        marks: &mut Vec<AgentState>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<SessionRow>> {
        let this = cx.entity().downgrade();
        let mut shown = Vec::new();
        for session in listed {
            let row = self.sessions.remove(&session.id).unwrap_or_else(|| {
                let this = this.clone();
                let id = session.id.clone();
                // A session opened before the tree showed it (one just started).
                let selected = self.selected == Some(Selection::Session(session.id.clone()));
                let open = self.kept.leads.get(&session.id).copied();
                cx.new(|cx| {
                    let mut row = SessionRow::new(id, this, cx);
                    if let Some(open) = open {
                        row.restore_open(open);
                    }
                    row.set_selected(selected, cx);
                    row
                })
            });
            // GPUI Fast takes any update of an entity for a change: touch only what changed.
            if !row
                .read(cx)
                .holds(&session.title, session.mark, session.active_at)
            {
                row.update(cx, |row, cx| {
                    row.set(&session.title, session.mark, session.active_at, now, cx)
                });
            }
            if row.read(cx).said() != session.mark_said {
                row.update(cx, |row, cx| row.set_said(&session.mark_said, cx));
            }
            if row.read(cx).waits() != session.waits {
                row.update(cx, |row, cx| row.set_waits(&session.waits, cx));
            }
            let blocked = session.blocked.is_some();
            if row.read(cx).is_blocked() != blocked {
                row.update(cx, |row, cx| row.set_blocked(blocked, cx));
            }
            if row.read(cx).commands() != session.commands {
                row.update(cx, |row, cx| row.set_commands(session.commands, cx));
            }
            let held = self.draft_held(&session.id, cx);
            if row.read(cx).draft() != held {
                row.update(cx, |row, cx| row.set_draft(held, cx));
            }
            if !row
                .read(cx)
                .holds_subject(session.subject.as_ref(), session.renamed)
            {
                row.update(cx, |row, cx| {
                    row.set_subject(session.subject.as_ref(), session.renamed, cx)
                });
            }
            // Ended, it sits among other projects' sessions: its row says whose it is, and the
            // machine of one of another machine (#1280).
            let scope = session.ended.then(|| {
                match session.elsewhere.as_ref().filter(|there| !there.here) {
                    Some(there) => format!("{} on {}", session.project, there.machine),
                    None => session.project.clone(),
                }
            });
            let pull = self.pull_of(session, &session.place).cloned();
            row.update(cx, |row, cx| {
                row.set_scope(scope.as_deref(), cx);
                row.set_place(&session.place, cx);
                row.set_pull(pull.as_ref(), cx);
                // A session of another machine, of its list or one an agent of this machine
                // started there, is closed from here through this machine's daemon (#1520).
                let there = session.elsewhere.as_ref();
                let closable = there.is_some_and(|there| !there.here);
                row.set_elsewhere(there, cx);
                row.set_tag(session.tag.as_deref(), cx);
                // Its machine's mark, when it is another machine's.
                let machine = there
                    .filter(|there| !there.here)
                    .map(|there| there.machine.as_str())
                    .or(session.tag.as_deref());
                let home =
                    machine.and_then(|machine| homes.iter().find(|home| home.machine == machine));
                row.set_home(home, cx);
                row.set_actions(
                    session.ended || (there.is_some() && !closable),
                    session.resumable && there.is_none(),
                    session.started_by.as_deref().filter(|_| there.is_none()),
                    cx,
                );
                if session.ended {
                    row.set_led_by(None, cx);
                }
            });
            marks.extend(session.mark);
            shown.push(row.clone());
            rows.insert(session.id.clone(), row);
        }
        shown
    }

    /// The rows of the projects whose settings file is gone, in the section of `machine` when the
    /// projects are shown by machine: this machine's alone, each with the sessions it holds, and
    /// never the row of another machine's project of the same name: that one is in `projects`.
    fn unconfigured_nodes(
        &mut self,
        snapshot: &Snapshot,
        machine: Option<&str>,
        sessions: &mut HashMap<String, Entity<SessionRow>>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Vec<(GoneKey, Entity<ProjectNode>)> {
        let this = cx.entity().downgrade();
        let mut nodes = Vec::new();
        for gone in &snapshot.unconfigured {
            let mut marks = Vec::new();
            let live: Vec<&crate::model::Session> = snapshot
                .level_sessions(&gone.name, true)
                .into_iter()
                .filter(|session| {
                    machine.is_none_or(|machine| snapshot.machine_of(session) == machine)
                })
                .collect();
            let rows = self.session_rows(
                live.iter().copied(),
                &snapshot.homes,
                sessions,
                &mut marks,
                now,
                cx,
            );
            let rows = nested(&live, &rows, cx);
            let at_foot: Vec<&crate::model::Session> = snapshot
                .level_blocked(&gone.name, true)
                .into_iter()
                .filter(|session| {
                    machine.is_none_or(|machine| snapshot.machine_of(session) == machine)
                })
                .collect();
            let at_foot = self.session_rows(
                at_foot.iter().copied(),
                &snapshot.homes,
                sessions,
                &mut marks,
                now,
                cx,
            );
            let key = (gone.name.clone(), machine.map(str::to_owned));
            let node = self.gone.remove(&key).unwrap_or_else(|| {
                let this = this.clone();
                let name = gone.name.clone();
                let kept = format!("{}{name}", crate::tree::GONE);
                let open = match machine {
                    None => self.kept.projects.get(&kept).copied(),
                    Some(machine) => self
                        .kept
                        .on_machines
                        .get(&machine_key(machine, &kept))
                        .copied(),
                }
                .unwrap_or(true);
                let unfolded = self
                    .kept
                    .blocked
                    .get(&node_key(&name, machine, true))
                    .copied()
                    .unwrap_or(false);
                let machine = machine.map(str::to_owned);
                cx.new(|cx| ProjectNode::gone(name, machine, this, open, unfolded, cx))
            });
            let mark = AgentState::strongest_held(marks);
            if !node.read(cx).holds(&rows, &at_foot, mark, cx) {
                node.update(cx, |node, cx| node.set(rows, at_foot, mark, cx));
            }
            // Beside the group of another machine's project of its name, the row says whose it is
            // (by machine, the sections do).
            let apart = (machine.is_none()
                && snapshot.projects.iter().any(|p| p.name == gone.name))
            .then_some(snapshot.this_machine.as_deref())
            .flatten();
            if !node.read(cx).says(&gone.name, apart, &[]) {
                node.update(cx, |node, cx| {
                    node.set_look(&gone.name, apart, Vec::new(), cx)
                });
            }
            nodes.push((key, node));
        }
        nodes
    }

    /// Brings every row up to `snapshot`, creating and dropping rows as needed. A row whose data
    /// did not change is not drawn again.
    fn show(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let now = now_ms();
        let this = cx.entity().downgrade();
        // A hidden project keeps its rows and its sessions' (a notification still opens its
        // session): it is only not put in the sidebar, whose list folds its row away.
        let hidden = self.hidden.clone();
        let mut projects = HashMap::new();
        let mut sessions = HashMap::new();
        let mut project_nodes = Vec::new();
        // A workspace that is gone takes its pull request with it, each checked against its own
        // machine's list: another machine's that was never read keeps its own (#1271).
        let copies = &self.copies;
        self.pulls
            .retain(|(machine, project, name), _| match machine {
                None => snapshot.workspace(project, name).is_some(),
                Some(machine) => copies
                    .get(machine)
                    .and_then(|copy| copy.read.as_ref())
                    .is_none_or(|read| read.workspace(project, name).is_some()),
            });
        let grouped = self.by_machine && snapshot.homes.len() > 1;
        let mut section_nodes = Vec::new();
        let mut sections = HashMap::new();
        let mut machine_projects = HashMap::new();
        let mut gone = HashMap::new();
        if grouped {
            // One section for each machine, each with the projects it has and its own sessions.
            for home in &snapshot.homes {
                let mut nodes = Vec::new();
                let mut hid = 0;
                for project in snapshot
                    .projects
                    .iter()
                    .filter(|project| project.homes.iter().any(|h| h.machine == home.machine))
                {
                    let mut marks = Vec::new();
                    let live: Vec<&crate::model::Session> = snapshot
                        .level_sessions(&project.name, false)
                        .into_iter()
                        .filter(|session| snapshot.machine_of(session) == home.machine)
                        .collect();
                    let rows = self.session_rows(
                        live.iter().copied(),
                        &snapshot.homes,
                        &mut sessions,
                        &mut marks,
                        now,
                        cx,
                    );
                    let rows = nested(&live, &rows, cx);
                    let at_foot: Vec<&crate::model::Session> = snapshot
                        .level_blocked(&project.name, false)
                        .into_iter()
                        .filter(|session| snapshot.machine_of(session) == home.machine)
                        .collect();
                    let at_foot = self.session_rows(
                        at_foot.iter().copied(),
                        &snapshot.homes,
                        &mut sessions,
                        &mut marks,
                        now,
                        cx,
                    );
                    let key = (project.name.clone(), home.machine.clone());
                    let node = self.machine_projects.remove(&key).unwrap_or_else(|| {
                        let this = this.clone();
                        let kept = machine_key(&home.machine, &project.name);
                        let open = self
                            .kept
                            .on_machines
                            .get(&kept)
                            .or_else(|| self.kept.projects.get(&project.name))
                            .copied()
                            .unwrap_or(true);
                        let (name, machine) = key.clone();
                        let unfolded = self
                            .kept
                            .blocked
                            .get(&node_key(&name, Some(&machine), false))
                            .copied()
                            .unwrap_or(false);
                        cx.new(|cx| ProjectNode::new(name, Some(machine), this, open, unfolded, cx))
                    });
                    let mark = AgentState::strongest_held(marks);
                    if !node.read(cx).holds(&rows, &at_foot, mark, cx) {
                        node.update(cx, |node, cx| node.set(rows, at_foot, mark, cx));
                    }
                    // Its commands run on the machine this was read from.
                    let commands =
                        match snapshot.this_machine.as_deref() == Some(home.machine.as_str()) {
                            true => project.commands,
                            false => 0,
                        };
                    if node.read(cx).commands() != commands {
                        node.update(cx, |node, cx| node.set_commands(commands, cx));
                    }
                    let homes = std::slice::from_ref(home);
                    if !node
                        .read(cx)
                        .says(project.title(), project.apart.as_deref(), homes)
                    {
                        node.update(cx, |node, cx| {
                            node.set_look(
                                project.title(),
                                project.apart.as_deref(),
                                homes.to_vec(),
                                cx,
                            )
                        });
                    }
                    match hidden.contains(&project.name) {
                        true => hid += 1,
                        false => nodes.push(node.clone()),
                    }
                    machine_projects.insert(key, node);
                }
                if snapshot.homes.first() == Some(home) {
                    for (key, node) in self.unconfigured_nodes(
                        &snapshot,
                        Some(&home.machine),
                        &mut sessions,
                        now,
                        cx,
                    ) {
                        nodes.push(node.clone());
                        gone.insert(key, node);
                    }
                }
                let nodes_empty = nodes.is_empty();
                let section = self.sections.remove(&home.machine).unwrap_or_else(|| {
                    let this = this.clone();
                    let open = self
                        .kept
                        .machines
                        .get(&home.machine)
                        .copied()
                        .unwrap_or(true);
                    let home = home.clone();
                    cx.new(|cx| MachineNode::new(home, this, open, cx))
                });
                if section.read(cx).home() != home || section.read(cx).projects(cx) != nodes {
                    section.update(cx, |section, cx| section.set(home.clone(), nodes, cx));
                }
                // A machine whose projects are all hidden has no section: it folds away with them.
                if !(nodes_empty && hid > 0) {
                    section_nodes.push(section.clone());
                }
                sections.insert(home.machine.clone(), section);
            }
        } else {
            for project in &snapshot.projects {
                let mut marks = Vec::new();
                let live = snapshot.level_sessions(&project.name, false);
                let rows = self.session_rows(
                    live.iter().copied(),
                    &snapshot.homes,
                    &mut sessions,
                    &mut marks,
                    now,
                    cx,
                );
                let rows = nested(&live, &rows, cx);
                let at_foot = snapshot.level_blocked(&project.name, false);
                let at_foot = self.session_rows(
                    at_foot.iter().copied(),
                    &snapshot.homes,
                    &mut sessions,
                    &mut marks,
                    now,
                    cx,
                );
                let node = self.projects.remove(&project.name).unwrap_or_else(|| {
                    let name = project.name.clone();
                    let this = this.clone();
                    let open = self.kept.projects.get(&name).copied().unwrap_or(true);
                    let unfolded = self
                        .kept
                        .blocked
                        .get(&node_key(&name, None, false))
                        .copied()
                        .unwrap_or(false);
                    cx.new(|cx| ProjectNode::new(name, None, this, open, unfolded, cx))
                });
                let mark = AgentState::strongest_held(marks);
                if !node.read(cx).holds(&rows, &at_foot, mark, cx) {
                    node.update(cx, |node, cx| node.set(rows, at_foot, mark, cx));
                }
                if node.read(cx).commands() != project.commands {
                    node.update(cx, |node, cx| node.set_commands(project.commands, cx));
                }
                if !node
                    .read(cx)
                    .says(project.title(), project.apart.as_deref(), &project.homes)
                {
                    node.update(cx, |node, cx| {
                        node.set_look(
                            project.title(),
                            project.apart.as_deref(),
                            project.homes.clone(),
                            cx,
                        )
                    });
                }
                if !hidden.contains(&project.name) {
                    project_nodes.push(node.clone());
                }
                projects.insert(project.name.clone(), node);
            }
            // The projects with no file go after the user's own list, to be removed.
            for (key, node) in self.unconfigured_nodes(&snapshot, None, &mut sessions, now, cx) {
                project_nodes.push(node.clone());
                gone.insert(key, node);
            }
        }
        // The rows of the other way of showing the projects are on their way out: their
        // sessions' rows are the new ones', which they must not show a second time.
        for node in self
            .projects
            .values()
            .chain(self.machine_projects.values())
            .chain(self.gone.values())
            .cloned()
            .collect::<Vec<_>>()
        {
            if !node.read(cx).sessions(cx).is_empty() {
                node.update(cx, |node, cx| node.set_rows(Vec::new(), cx));
            }
        }
        self.machine_projects = machine_projects;
        self.gone = gone;
        self.grouped = grouped;
        for section in self.sections.values().cloned().collect::<Vec<_>>() {
            if !section.read(cx).projects(cx).is_empty() {
                section.update(cx, |section, cx| {
                    section.set(section.home().clone(), Vec::new(), cx)
                });
            }
        }
        self.sections = sections;
        // The ended sessions leave their places for one folded group below every project.
        let mut ended = snapshot.ended_sessions();
        ended.retain(|session| !hidden.contains(&session.project));
        let mut marks = Vec::new();
        let rows = self.session_rows(
            ended.iter().copied(),
            &snapshot.homes,
            &mut sessions,
            &mut marks,
            now,
            cx,
        );
        // A machine whose ended sessions are not listed is said by the group, rows or none.
        let present = !rows.is_empty() || !self.unlisted.is_empty();
        if self.ended.is_none() && present {
            let open = self.kept.ended.unwrap_or(false);
            self.ended = Some(cx.new(|cx| EndedNode::new(this.clone(), open, cx)));
        }
        if let Some(group) = self.ended.clone() {
            if !group.read(cx).holds(&rows, cx) {
                group.update(cx, |group, cx| group.set(rows, cx));
            }
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_ended(group, present, cx));
        }
        self.say_unlisted(cx);
        match grouped {
            true => {
                // The user's list folds away as it was, and its rows come back when it does: the
                // nodes stay, the projects that are gone from them.
                self.projects
                    .retain(|name, _| snapshot.projects.iter().any(|p| &p.name == name));
            }
            false => self.projects = projects,
        }
        self.sessions = sessions;
        let offered = snapshot.homes.len() > 1;
        let sidebar = self.sidebar.read(cx);
        // First, so that the rows below are taken (or kept, while shown by machine).
        if sidebar.is_by_machine() != grouped
            || sidebar.offers_grouping() != offered
            || !sidebar.has_menu()
        {
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.set_arrangement(grouped, offered, cx)
            });
        }
        let names: Vec<String> = snapshot
            .projects
            .iter()
            .map(|p| p.name.clone())
            .filter(|name| !hidden.contains(name))
            .collect();
        let arranged = self.arranged_rows(&names, &self.projects, cx);
        self.sidebar_changed(cx);
        // GPUI Fast takes any update for a change: touch the list only when it did.
        if !grouped && self.sidebar.read(cx).projects() != project_nodes {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_projects(project_nodes, cx));
        }
        if !grouped && self.sidebar.read(cx).arranged() != &arranged {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_arranged(arranged, cx));
        }
        if self.sidebar.read(cx).sections() != section_nodes {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_sections(section_nodes, cx));
        }

        // The forms in front are another machine's while one is: its own read keeps them.
        if self.front.is_none() {
            self.forms
                .retain(|name, _| snapshot.projects.iter().any(|p| &p.name == name));
        }
        let listed = |id: &String| {
            snapshot.sessions.iter().any(|s| &s.id == id)
                || snapshot.elsewhere.iter().any(|s| &s.id == id)
        };
        self.replies.retain(|id, _| listed(id));
        self.docks.retain(|id, _| listed(id));
        self.kept_pages.retain(|id, _| listed(id));
        let edited = snapshot.project_changes != self.snapshot.project_changes;
        self.snapshot = snapshot;
        // A session detached from its pilot: its open page stops saying who led it.
        if let Some(page) = self.session_page.clone()
            && let Some(found) = self.snapshot.session(page.read(cx).id())
            && !page.read(cx).says_by(&found.started_by, &found.led_by)
        {
            let (started_by, led_by) = (found.started_by.clone(), found.led_by.clone());
            page.update(cx, |page, cx| page.set_by(started_by, led_by, cx));
        }
        // A session blocked by someone else, or no longer: its open page's banner says it.
        if let Some(page) = self.session_page.clone()
            && let Some(found) = self.snapshot.session(page.read(cx).id())
            && page.read(cx).blocked() != found.blocked.as_ref()
        {
            let blocked = found.blocked.clone();
            page.update(cx, |page, cx| page.set_blocked(blocked, cx));
        }
        // What the open session is about changed: its page says it.
        if let Some(page) = self.session_page.clone()
            && let Some(found) = self.snapshot.session(page.read(cx).id())
            && page.read(cx).subject() != found.subject.as_ref()
        {
            let subject = found.subject.clone();
            page.update(cx, |page, cx| page.set_subject(subject, cx));
        }
        // The sessions on other machines are found there, not here: they are not in `sessions`.
        let read = self.snapshot.clone();
        let hidden = self.hidden.clone();
        self.switcher
            .update(cx, |switcher, cx| switcher.set_snapshot(&read, &hidden, cx));
        self.ring(cx);

        self.reshow(window, cx);
        // A project's file was edited (by hand, or by the command line): its repositories as the
        // page shows them may no longer be.
        if edited && let Some(page) = self.main.read(cx).project_page().cloned() {
            let section = page.read(cx).repositories().clone();
            section.update(cx, |section, cx| section.read(cx));
        }
        if edited && let Some(page) = self.project_settings.clone() {
            let section = page.read(cx).repositories().clone();
            section.update(cx, |section, cx| section.read(cx));
        }
    }

    /// What was open is shown as it is now; the first project opens when nothing is.
    fn reshow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected.clone() {
            None => {
                if let Some(first) = self.first_shown() {
                    self.select(Selection::Project(first), window, cx);
                }
            }
            Some(selection) => {
                let page = self.page_of(&selection, cx);
                if self.main.read(cx).page() != &page {
                    let forms = self.forms_for(&page, window, cx);
                    let session = self.session_for(&page, window, cx);
                    let settings = self.project_settings_for(&page, window, cx);
                    let spec = self.spec_page_for(&page, cx);
                    self.main.update(cx, |main, cx| {
                        main.set_spec_page(spec);
                        main.show(page, forms, session, settings, cx)
                    });
                }
                self.say_reading(cx);
                self.refresh_project(cx);
            }
        }
    }

    /// While a project's page of another machine waits for that machine's first read, its pane
    /// says it is reading, never another machine's values.
    fn say_reading(&mut self, cx: &mut Context<Self>) {
        let project_page = matches!(
            self.selected,
            Some(Selection::Project(_) | Selection::ProjectSettings(_) | Selection::Spec { .. })
        );
        let reading = project_page
            && self
                .front
                .as_ref()
                .is_some_and(|front| self.copies.get(front).is_none_or(|c| c.read.is_none()));
        self.main
            .update(cx, |main, cx| main.set_reading(reading, cx));
    }

    /// A ticket or a pull request an agent changed: its session's row is marked, unless the
    /// session is the one open, where it is in sight. The bell's item is the daemon's own
    /// notification, which comes with its event.
    fn forged(&mut self, forged: Forged, cx: &mut Context<Self>) {
        let session = self.snapshot.root_of(&forged.agent);
        if self.selected == Some(Selection::Session(session.clone())) {
            return;
        }
        self.news.insert(session, unread_words(&forged.said));
        self.ring(cx);
    }

    /// An agent handed the user a link: a toast offers it with an Open button, unless the session's
    /// conversation is on screen, where its card is. Links shared in a row are one toast, which
    /// counts them while it is on screen. Nothing opens until the click.
    fn linked(&mut self, link: LinkShown, cx: &mut Context<Self>) {
        let session = self.snapshot.root_of(&link.agent);
        // On screen is the conversation of the agent itself: a sub-agent's link, while the
        // root's conversation is shown, is not in sight.
        let on_screen = self.selected == Some(Selection::Session(session.clone()))
            && self
                .session_page
                .as_ref()
                .is_some_and(|page| page.read(cx).shown() == link.agent);
        let sharing = self.sharing.entry(link.agent.clone()).or_default();
        if on_screen {
            sharing.end();
            return;
        }
        let mut key = format!("links-{}-{}", link.agent, sharing.row);
        if !sharing.links.is_empty() && !gpui_design_system::toast::shown(&key, cx) {
            sharing.end();
            key = format!("links-{}-{}", link.agent, sharing.row);
        }
        sharing.links.push(link);
        let shell = cx.entity().downgrade();
        let toast = match sharing.links.as_slice() {
            [only] => {
                let target = only.target.clone();
                let video = crate::shown_files::is_video(&target);
                let sent = video || crate::links_card::is_picture(&target);
                match self.front.clone().filter(|_| !is_address(&target)) {
                    // A picture or a video of another machine: that machine sends a copy.
                    Some(machine) if sent => {
                        let link = crate::conversation::ShownLink {
                            title: only.title.clone(),
                            whence: only.whence.clone(),
                            target: target.clone(),
                            agent: only.agent.clone(),
                            seq: only.seq,
                        };
                        let kind = if video { "video" } else { "picture" };
                        gpui_design_system::Toast::success(format!(
                            "The agent shares a {kind} of {machine}: {}",
                            only.name()
                        ))
                        .action(if video { "Play" } else { "Open" }, move |_, cx| {
                            crate::shown_files::open(&link, cx)
                        })
                    }
                    // A file of another machine is shown where it is, never opened here.
                    Some(machine) => gpui_design_system::Toast::success(format!(
                        "The agent shares a file of {machine}: {target}"
                    )),
                    None => gpui_design_system::Toast::success(format!(
                        "The agent shares a link: {}",
                        only.name()
                    ))
                    .action("Open", move |_, cx| {
                        shell
                            .update(cx, |shell, cx| shell.open_shown(&target, cx))
                            .ok();
                    }),
                }
            }
            shared => gpui_design_system::Toast::success(format!(
                "The agent shares {} links",
                shared.len()
            ))
            .action("Show", move |window, cx| {
                shell
                    .update(cx, |shell, cx| {
                        shell.open(Selection::Session(session.clone()), window, cx)
                    })
                    .ok();
            }),
        };
        gpui_design_system::toast(toast.keyed(key), cx);
    }

    /// What the agent said or what the user told it ends the row of links it was sharing.
    fn spoke(&mut self, agent: &str) {
        if let Some(sharing) = self.sharing.get_mut(agent) {
            sharing.end();
        }
    }

    /// An agent said something, reported or was woken: its session's row says it, unless the
    /// session is the one open, where it is in sight.
    fn news(&mut self, news: News, cx: &mut Context<Self>) {
        self.spoke(&news.agent);
        let session = self.snapshot.root_of(&news.agent);
        if self.selected == Some(Selection::Session(session.clone())) {
            return;
        }
        let said = unread_words(news.said);
        // A message comes in many pieces, and says less than a report or a change: it never
        // replaces what the row said.
        let weak = news.said == News::MESSAGE && self.news.contains_key(&session);
        if !weak && self.news.get(&session) != Some(&said) {
            self.news.insert(session, said);
            self.ring(cx);
        }
    }

    /// Brings the bell and the sessions' marks up to the notification centre the daemon keeps
    /// and what the agents changed: each item says why it is there, a short excerpt, and its
    /// session, which the panel groups them by.
    fn ring(&mut self, cx: &mut Context<Self>) {
        for (id, row) in &self.sessions {
            let said = self.news.get(id).map(String::as_str);
            if row.read(cx).unread() != said {
                row.update(cx, |row, cx| row.set_unread(said, cx));
            }
        }
        // What waits on another machine says which (#1280).
        let snapshot = &self.snapshot;
        let rows: Vec<BellRow> = self
            .bell_items()
            .into_iter()
            .map(|(machine, item)| {
                let found = item.session.as_deref().and_then(|id| match machine {
                    None => snapshot.session(id),
                    Some(_) => snapshot.elsewhere(id),
                });
                let mut title = found
                    .map(|s| match &s.started_by {
                        Some(by) => format!("{} · started by {by}", s.title),
                        None => s.title.clone(),
                    })
                    .unwrap_or_else(|| "Flottille".to_owned());
                if let Some(machine) = machine {
                    title = format!("{title} · on {machine}");
                }
                BellRow {
                    project: found.map(|s| s.project.clone()).unwrap_or_default(),
                    item,
                    title,
                }
            })
            .collect();
        let counts = self
            .bells
            .values()
            .filter_map(|bell| bell.read.as_ref())
            .fold(snapshot.unread, |(needs, new), read| {
                (needs + read.unread.0, new + read.unread.1)
            });
        if self.top.read(cx).counts() != counts {
            self.top.update(cx, |top, cx| top.set_counts(counts, cx));
        }
        if self.bell.read(cx).rows != rows {
            self.bell.update(cx, |bell, cx| bell.set(rows, counts, cx));
        } else if self.bell.read(cx).counts != counts {
            self.bell.update(cx, |bell, cx| bell.set_counts(counts, cx));
        }
        // What arrives in the session that is open stays unread: the window may be open with
        // nobody looking. Opening the session, its row or "Mark all read" reads it.
    }

    /// Asks the daemon to mark a session's unread notifications read, once for each.
    fn read_session(&mut self, session: &str) {
        let there: Vec<(String, u64)> = self
            .bell_items()
            .into_iter()
            .filter_map(|(machine, item)| Some((machine?.to_owned(), item)))
            .filter(|(_, item)| !item.read && item.session.as_deref() == Some(session))
            .map(|(machine, item)| (machine, item.id))
            .filter(|marked| !self.marked_there.contains(marked))
            .collect();
        if let Some((machine, _)) = there.first() {
            let agent = session
                .strip_suffix(&format!("@{machine}"))
                .unwrap_or(session)
                .to_owned();
            let machine = machine.clone();
            let ids = there
                .into_iter()
                .filter(|(of, _)| *of == machine)
                .map(|(_, id)| id)
                .collect();
            self.read_there(&machine, Reading::Session(agent), ids);
            return;
        }
        let unread: Vec<u64> = self
            .snapshot
            .bell
            .iter()
            .filter(|item| !item.read && item.session.as_deref() == Some(session))
            .map(|item| item.id)
            .filter(|id| !self.marked.contains(id))
            .collect();
        if !unread.is_empty() {
            self.marked.extend(unread);
            self.perform_here(Command::ReadNotifications(Reading::Session(
                session.to_owned(),
            )));
        }
    }

    /// "Mark all read" in the bell's panel.
    pub fn read_all(&mut self) {
        let unread: Vec<u64> = self
            .snapshot
            .bell
            .iter()
            .filter(|item| !item.read)
            .map(|item| item.id)
            .collect();
        if !unread.is_empty() {
            self.marked.extend(unread);
            self.perform_here(Command::ReadNotifications(Reading::All));
        }
        let mut there: Vec<(String, u64)> = self
            .bell_items()
            .into_iter()
            .filter_map(|(machine, item)| Some((machine?.to_owned(), item)))
            .filter(|(_, item)| !item.read)
            .map(|(machine, item)| (machine, item.id))
            .collect();
        there.sort();
        let mut asked: Vec<String> = there.iter().map(|(machine, _)| machine.clone()).collect();
        asked.dedup();
        for machine in asked {
            let ids = there
                .iter()
                .filter(|(of, _)| *of == machine)
                .map(|(_, id)| *id)
                .collect();
            self.read_there(&machine, Reading::All, ids);
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        for row in self.sessions.values() {
            if row.read(cx).ago_changes(now) {
                row.update(cx, |row, cx| row.tick(now, cx));
            }
        }
        if let Some(page) = self.main.read(cx).project_page().cloned() {
            crate::project::ProjectPage::tick(&page, now, cx);
        }
        if let Some(page) = self.project_settings.clone() {
            // The project's file is the user's to edit by hand too.
            let project = page.read(cx).name().to_owned();
            self.actions.send(Command::Commands {
                project: project.clone(),
            });
            self.actions.send(Command::Skills {
                project: project.clone(),
            });
            self.actions.send(Command::Tasks {
                project: project.clone(),
            });
            self.actions.send(Command::Setup { project });
        }
    }

    /// The sentence of the notice that says which Auto judge Flottille picked, while the user
    /// has not seen it.
    pub fn judge_notice(&self) -> Option<String> {
        if self.judge_noticed || !self.providers.judge_picked {
            return None;
        }
        let (name, model) = self.providers.judge.as_ref()?;
        let title = self
            .providers
            .get(name)
            .map_or(name.as_str(), |p| p.title.as_str());
        // A model that is not known is left out: the provider chooses it.
        let model = self
            .providers
            .model_said(name, model.as_deref())
            .map_or_else(String::new, |model| format!(" {model}"));
        Some(format!(
            "Flottille chose {title}{model} as your Auto judge: {title} is installed and signed \
             in here. In Auto, it decides the calls the rules do not know, on your quota. Change \
             it, or choose no judge, in Settings."
        ))
    }

    /// The sentence of the notice that says which compactor Flottille picked, while the user has
    /// not seen it. One notice at a time: the judge's first.
    pub fn compaction_notice(&self) -> Option<String> {
        if self.compaction_noticed || !self.compaction_picked || self.judge_notice().is_some() {
            return None;
        }
        let (name, model) = self.compactor_shown.as_ref()?;
        let title = self
            .providers
            .get(name)
            .map_or(name.as_str(), |p| p.title.as_str());
        let model = self
            .providers
            .model_said(name, model.as_deref())
            .map_or_else(String::new, |model| format!(" {model}"));
        Some(format!(
            "Flottille chose {title}{model} as your compaction model: when an agent's context fills up, it \
             summarises the conversation and the agent goes on from the summary, on your quota. \
             Change it, or choose none, in Settings."
        ))
    }

    /// The notice of the compactor was seen: it goes, and the daemon is told.
    pub fn dismiss_compaction_notice(&mut self, cx: &mut Context<Self>) {
        if !self.compaction_noticed {
            self.compaction_noticed = true;
            self.actions.send(Command::CompactionNoticed);
            cx.notify();
        }
    }

    /// The notice was seen: it goes, and the daemon is told, so that it never comes back.
    pub fn dismiss_judge_notice(&mut self, cx: &mut Context<Self>) {
        if !self.judge_noticed {
            self.judge_noticed = true;
            self.actions.send(Command::JudgeNoticed);
            cx.notify();
        }
    }

    /// A notice of what Flottille chose for the user, bottom right: the Auto judge (`judge`), or
    /// the compactor. Its keys start with `judge-notice` or `compaction-notice`.
    /// `present` false, it folds away and fades out with its last text.
    fn notice_card(
        &self,
        judge: bool,
        present: bool,
        text: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = if judge {
            "judge-notice"
        } else {
            "compaction-notice"
        };
        let part = |part: &str| SharedString::from(format!("{key}{part}"));
        let p = palette(cx);
        let buttons = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(div().flex_1())
            .child(
                Button::secondary(part("-dismiss"), "Dismiss").on_click(cx.listener(
                    move |this, _, _, cx| match judge {
                        true => this.dismiss_judge_notice(cx),
                        false => this.dismiss_compaction_notice(cx),
                    },
                )),
            )
            .child(
                Button::primary(part("-open"), "Open Settings").on_click(cx.listener(
                    move |this, _, window, cx| match judge {
                        true => {
                            this.dismiss_judge_notice(cx);
                            this.open_settings_at(Reveal::Judge, window, cx);
                        }
                        false => {
                            this.dismiss_compaction_notice(cx);
                            this.open_settings_at(Reveal::Compaction, window, cx);
                        }
                    },
                )),
            );
        let body = div()
            .px(space::LG)
            .py(space::MD)
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(
                div()
                    .id(part("-text"))
                    .aria_label(SharedString::from(text.clone()))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(text))
                    .test_support(),
            )
            .child(buttons);
        let card = div()
            .id(SharedString::from(key))
            .child(enter(
                part("-enter"),
                0,
                Card::new()
                    .title(if judge {
                        "Auto judge chosen"
                    } else {
                        "Compaction model chosen"
                    })
                    .child(body),
            ))
            .test_support();
        div()
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .w(px(380.))
            .child(presence(part("-presence"), present, card))
    }

    /// Opens a project's settings at a repository's row or at its tasks, from its page.
    pub fn open_project_settings_at(
        &mut self,
        project: &str,
        at: crate::project_settings::Reveal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Selection::ProjectSettings(project.to_owned()), window, cx);
        if let Some(page) = self.settings_of_project(project, cx) {
            page.update(cx, |page, cx| page.reveal(at, cx));
        }
    }

    /// "New entry", the vault's dialog prefilled with `prefill` (a project's Tracker, a project's
    /// Vault tab), `why` the agent's reason when it is one. It opens over the page the user is
    /// on, and closing it leaves them there.
    pub fn add_vault_entry(
        &mut self,
        prefill: &crate::vault::VaultEntry,
        why: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vault = self.vault(cx);
        vault.update(cx, |vault, cx| {
            vault.add_prefilled(prefill, why, window, cx)
        });
    }

    /// A row of a project's Vault tab: the entry in its own dialog, over the tab. An entry gone
    /// since the row was drawn leaves nothing to open.
    pub fn replace_vault_entry_for(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vault = self.vault(cx);
        vault.update(cx, |vault, cx| vault.replace_of_project(name, window, cx));
    }

    /// An agent's request for a vault entry, from its card: a new entry prefilled with it, over
    /// the session.
    pub fn add_asked_vault_entry(
        &mut self,
        entry: crate::vault::VaultEntry,
        why: String,
        host: Option<String>,
        connection: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vault = self.vault(cx);
        vault.update(cx, |vault, cx| {
            vault.add_asked(&entry, why, host, connection, window, cx)
        });
    }

    /// "New entry…" on the dialog of an agent's request to write a connection, which names the
    /// entry `name` that is not made: a new entry named so, its address the `host` the request
    /// sends it to, stacked over the request's dialog, which reads the request again once it is
    /// saved.
    pub fn add_vault_entry_for_request(
        &mut self,
        number: u64,
        machine: Option<String>,
        name: &str,
        host: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The vault of the request's machine, whichever machine is in front (#1271).
        let Some(vault) = self
            .settings_on(&machine)
            .map(|page| page.read(cx).vault().clone())
        else {
            return;
        };
        vault.update(cx, |vault, cx| {
            vault.add_for_request(number, name, host, window, cx)
        });
    }

    /// An agent's proposed edit of a vault entry, from its card's Edit: the entry's own form with
    /// it, over the session.
    pub fn edit_proposed_vault_entry(
        &mut self,
        entry: crate::vault::VaultEntry,
        why: String,
        host: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vault = self.vault(cx);
        vault.update(cx, |vault, cx| {
            vault.edit_proposed(&entry, why, host, window, cx)
        });
    }

    /// Accept of the card of the proposed edit `ask` of the entry `name`: sent once, its buttons
    /// disabled until the daemon answers.
    pub fn accept_vault_ask(&mut self, ask: u64, name: String, cx: &mut Context<Self>) {
        if self.dismissing.insert(ask) {
            self.perform(Command::VaultAccept { ask, name });
            cx.notify();
        }
    }

    /// Whether an answer to the request `ask` (Accept or Dismiss) was sent and not refused.
    pub fn is_answering(&self, ask: u64) -> bool {
        self.dismissing.contains(&ask)
    }

    /// Dismiss of the card of the request `ask` for a vault entry: sent once, Dismiss disabled
    /// until the daemon answers.
    pub fn dismiss_vault_ask(&mut self, ask: u64, cx: &mut Context<Self>) {
        if self.dismissing.insert(ask) {
            self.perform(Command::VaultDismiss { ask });
            cx.notify();
        }
    }

    /// Whether the dismissal of the request `ask` was sent and not refused.
    pub fn is_dismissing(&self, ask: u64) -> bool {
        self.dismissing.contains(&ask)
    }

    /// Opens the dialog of an agent's request to write a connection.
    /// It reads and answers the request on the machine of the pages that open it.
    pub fn open_connection_request(&mut self, number: u64, cx: &mut Context<Self>) {
        let machine = self.pages_machine();
        self.open_connection_request_on(number, machine, cx);
    }

    /// Opens the dialog of the request `number` of `machine` (`None`: this one).
    fn open_connection_request_on(
        &mut self,
        number: u64,
        machine: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.connection_request
            .update(cx, |dialog, cx| dialog.open(number, machine, cx));
    }

    /// The review of the connection `name`, over the page the user is on (the kit's modal, as
    /// the form's Save leads to it).
    pub fn review_connection(&mut self, name: &str, cx: &mut Context<Self>) {
        let section = self.connections(cx);
        section.update(cx, |section, cx| {
            section.open_review_after_request(name, cx)
        });
    }

    /// "Review" on a request's dialog: the review of the connection `name` of the request's
    /// `machine`, whichever machine is in front.
    pub fn review_connection_on(
        &mut self,
        machine: Option<String>,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(section) = self
            .settings_on(&machine)
            .map(|page| page.read(cx).connections().clone())
        else {
            return;
        };
        section.update(cx, |section, cx| {
            section.open_review_after_request(name, cx)
        });
    }

    /// The Settings of `machine` (`None`: this one), in front or aside: none of a machine the
    /// window holds no pages of.
    fn settings_on(&self, machine: &Option<String>) -> Option<Entity<SettingsPage>> {
        match *machine == self.pages_machine() {
            true => Some(self.settings.clone()),
            false => self.sides.get(machine).map(|pages| pages.settings.clone()),
        }
    }

    /// The dialog of an agent's request to write a connection.
    pub fn connection_request(&self) -> &Entity<ConnectionRequest> {
        &self.connection_request
    }

    /// "Edit" on a request's dialog: the server's form filled with the agent's values, over the
    /// page the user is on, whose Save answers the request `number`.
    pub fn answer_connection_request(
        &mut self,
        number: u64,
        machine: Option<String>,
        server: &crate::extensions::connections::Server,
        adding: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The form of the request's machine (`None`: this one), whichever machine is in front:
        // the dialog says which, as it is being updated.
        let Some(section) = self
            .settings_on(&machine)
            .map(|page| page.read(cx).connections().clone())
        else {
            return;
        };
        section.update(cx, |section, cx| {
            section.answer_request(number, server, adding, window, cx)
        });
    }

    /// Opens Settings at a section: its tab is the one chosen.
    pub fn open_settings_at(
        &mut self,
        at: crate::settings::Reveal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Selection::Settings, window, cx);
        self.settings.update(cx, |page, cx| page.reveal(at, cx));
    }

    /// Opens a project's page or a session's: a choice made in a drawer closes it.
    pub fn open(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        // A machine still reading its key gives way: this page is the one asked last.
        self.choosing = None;
        // A project no machine of its row can start is said so, and nothing opens (#1439).
        if let Selection::Project(row) = &selection
            && let Start::Nowhere { machines } = self.start_on(row, cx)
        {
            self.say_nowhere(row, &machines, cx);
            return;
        }
        // A page of another machine is that machine's, its pages brought in front.
        let machine = self.machine_of(&selection, cx);
        // A project's row opens that machine's page of the same project, by its name there.
        let selection = match selection {
            Selection::Project(row) => {
                Selection::Project(self.name_on(&row, machine.as_deref()).unwrap_or(row))
            }
            selection => selection,
        };
        if machine == self.front {
            self.select(selection, window, cx);
        } else {
            self.reach(machine, selection, window, cx);
        }
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        self.set_bell(false, cx);
    }

    /// Says that none of the machines of the row `row` can start its project: each with its
    /// state. Nothing else starts it.
    fn say_nowhere(&self, row: &str, machines: &[String], cx: &mut Context<Self>) {
        let read = self.machines.read(cx).machines();
        let states: Vec<String> = machines
            .iter()
            .map(|name| match read.and_then(|read| read.machine(name)) {
                Some(machine) => format!("{name} is {}", machine.state_words()),
                None => format!("{name} is not known"),
            })
            .collect();
        gpui_design_system::toast(
            gpui_design_system::Toast::failure(format!("{row} cannot be started")).detail(format!(
                "None of its machines can run it: {}.",
                states.join(", ")
            )),
            cx,
        );
    }

    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        self.history.open(selection.clone(), self.front.clone());
        self.show_selection(selection, window, cx);
    }

    /// The mouse's back and forward buttons, Alt+Left and Alt+Right: the nearest page that way
    /// which still exists. A session's page shows the agent that was shown there.
    pub fn go(&mut self, way: Way, window: &mut Window, cx: &mut Context<Self>) {
        self.choosing = None;
        let exists = |visit: &crate::history::Visit| self.visit_exists(visit, cx);
        let Some(at) = self.history.find(way, exists) else {
            return;
        };
        // The page of the machine it was a page of; the history moves there once it is reached.
        let machine = self.history.visit(at).machine.clone();
        self.reach_then(machine, window, cx, move |shell, window, cx| {
            let visit = shell.history.move_to(at);
            shell.bring(visit.machine.clone(), window, cx);
            shell.show_visit(visit, window, cx);
        });
    }

    /// Whether the page of `visit` is still there: one of the machine in front as its pages say
    /// it; a session by its row; another machine's while that machine is the user's.
    fn visit_exists(&self, visit: &crate::history::Visit, cx: &App) -> bool {
        match (&visit.machine, &visit.selection) {
            (machine, selection) if *machine == self.front => {
                self.page_of(selection, cx) != Page::Empty
            }
            (_, Selection::Session(_)) => self.page_of(&visit.selection, cx) != Page::Empty,
            (Some(machine), _) => self
                .machines
                .read(cx)
                .machines()
                .is_some_and(|read| read.machine(machine).is_some()),
            (None, Selection::Project(name) | Selection::ProjectSettings(name)) => {
                self.snapshot.projects.iter().any(|p| &p.name == name)
            }
            (None, _) => true,
        }
    }

    /// Shows the page of `visit`, its machine's pages in front.
    fn show_visit(
        &mut self,
        visit: crate::history::Visit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_selection(visit.selection.clone(), window, cx);
        if let (Selection::Session(id), Some(page)) = (&visit.selection, self.session_page.clone())
        {
            let agent = visit.agent.unwrap_or_else(|| id.clone());
            page.update(cx, |page, cx| page.show(&agent, cx));
        }
        // A project's page shows the tab it was left on: Progress, or Work again.
        if matches!(visit.selection, Selection::Project(_))
            && let Some(page) = self.main.read(cx).project_page().cloned()
        {
            let tab = match (visit.progress, page.read(cx).tab()) {
                (true, _) => Some(PageTab::Progress),
                (false, PageTab::Progress) => Some(PageTab::Work),
                (false, _) => None,
            };
            if let Some(tab) = tab {
                page.update(cx, |page, cx| page.choose(tab, cx));
                self.feed_progress(cx);
            }
        }
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        self.set_bell(false, cx);
        // The page that held the focus is gone.
        window.focus(&self.focus, cx);
    }

    fn show_selection(
        &mut self,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(Selection::Session(old)) = &self.selected
            && let Some(row) = self.sessions.get(old)
        {
            row.update(cx, |row, cx| row.set_selected(false, cx));
        }
        if let Selection::Session(id) = &selection
            && let Some(row) = self.sessions.get(id)
        {
            row.update(cx, |row, cx| row.set_selected(true, cx));
        }
        // Opened, the session's changes are seen: its mark goes, and its notifications are read.
        if let Selection::Session(id) = &selection {
            self.read_session(id);
            if self.news.remove(id).is_some() {
                self.ring(cx);
            }
        }
        // Opened, the page reads the providers again: the file is the user's to edit by hand.
        let tab = self.selected_on.as_ref().map(|(_, tab)| *tab);
        let moved = self.selected_on.as_ref().map(|(front, _)| front) != Some(&self.front);
        if selection == Selection::Settings && (self.selected != Some(Selection::Settings) || moved)
        {
            // A page opened afresh starts on its first tab; a link then chooses its own. Another
            // machine chosen in it stays on the tab shown.
            let tab = match self.selected == Some(Selection::Settings) {
                true => tab.unwrap_or(0),
                false => 0,
            };
            self.settings.update(cx, |page, cx| page.choose(tab, cx));
            self.actions.send(Command::ReadProviders);
            self.read_settings();
            self.read_pages(cx);
        }
        // A project's pages of another machine read what was read of it, by its own link.
        if let (
            Some(front),
            Selection::Project(_) | Selection::ProjectSettings(_) | Selection::Spec { .. },
        ) = (self.front.clone(), &selection)
        {
            self.copy_of(&front);
            self.read_pages(cx);
        }
        // A copy is read only while a project's page of its machine is shown: left, its link
        // ends, and the page coming back reads it again.
        let of_a_project = matches!(
            selection,
            Selection::Project(_) | Selection::ProjectSettings(_) | Selection::Spec { .. }
        );
        let front = self.front.clone();
        self.copies
            .retain(|machine, _| of_a_project && front.as_deref() == Some(machine.as_str()));
        // Left, the services page stops reading the output it shows: nothing is in sight.
        if self.selected == Some(Selection::Services) && selection != Selection::Services {
            self.services.update(cx, |page, cx| page.close_output(cx));
        }
        // Opened, what is new is seen for good: the daemon keeps it, and the mark goes.
        if selection == Selection::WhatsNew && self.selected != Some(Selection::WhatsNew) {
            self.whats_new.update(cx, |page, cx| page.opened(cx));
            self.perform(Command::Update {
                method: "whatsnew.seen",
                params: serde_json::json!({}),
            });
        }
        // Opened, the services are read again: what runs changes outside the window too.
        if selection == Selection::Services && self.selected != Some(Selection::Services) {
            self.services.update(cx, |page, cx| page.opened(cx));
        }
        let page = self.page_of(&selection, cx);
        self.selected = Some(selection);
        let tab = self.settings.read(cx).tab();
        self.selected_on = Some((self.front.clone(), tab));
        let forms = self.forms_for(&page, window, cx);
        let session = self.session_for(&page, window, cx);
        let project_settings = self.project_settings_for(&page, window, cx);
        let spec = self.spec_page_for(&page, cx);
        self.main.update(cx, |main, cx| {
            main.set_spec_page(spec);
            main.show(page, forms, session, project_settings, cx)
        });
        self.say_reading(cx);
        self.refresh_project(cx);
    }

    /// What the front machine's pages read once, the first time one of them is shown: the parts
    /// of Flottille it runs and its connections.
    fn read_pages(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.pages_read, true) {
            return;
        }
        self.extensions.update(cx, |registry, cx| registry.read(cx));
        let connections = self.settings.read(cx).connections().clone();
        connections.update(cx, |section, cx| section.read(cx));
    }

    /// The tab of the open project's page was chosen by the user: Progress is a place of the
    /// history, and its page gets the sessions and workspaces of its project.
    pub fn tab_chosen(&mut self, project: &str, progress: bool, cx: &mut Context<Self>) {
        self.history.show_progress(project, progress);
        self.feed_progress(cx);
    }

    /// The Progress page open gets its project's sessions and workspaces as last read.
    fn feed_progress(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.main.read(cx).progress_page(cx) else {
            return;
        };
        let project = page.read(cx).project().to_owned();
        let sessions = crate::progress::sessions_of(self.pages(), &project);
        let workspaces = crate::progress::workspaces_of(self.pages(), &project);
        page.update(cx, |page, cx| page.set_sessions(sessions, workspaces, cx));
    }

    /// The page of what is selected, as the daemon last gave it.
    fn page_of(&self, selection: &Selection, cx: &App) -> Page {
        match selection {
            Selection::Settings => Page::Settings,
            Selection::Services => Page::Services,
            Selection::WhatsNew => Page::WhatsNew,
            // A project just added is not in the daemon's last read yet: its page, open, stays.
            Selection::ProjectSettings(name) => {
                let open = self
                    .project_settings
                    .as_ref()
                    .is_some_and(|page| page.read(cx).name() == name);
                if open || self.pages().projects.iter().any(|p| &p.name == name) {
                    Page::ProjectSettings { name: name.clone() }
                } else {
                    Page::Empty
                }
            }
            // Only where Specs is on: nothing of it shows elsewhere.
            Selection::Spec { project, spec } => {
                if self
                    .extensions
                    .read(cx)
                    .active(crate::extensions::specs::NAME, project)
                {
                    Page::Spec {
                        project: project.clone(),
                        spec: *spec,
                    }
                } else {
                    Page::Empty
                }
            }
            Selection::Project(name) => {
                match self.pages().projects.iter().find(|p| &p.name == name) {
                    Some(project) => Page::Project {
                        name: project.name.clone(),
                    },
                    None => Page::Empty,
                }
            }
            Selection::Session(id) => {
                let agent = self.agent_of(id);
                let open = self
                    .session_page
                    .as_ref()
                    .map(|page| page.read(cx))
                    .filter(|page| page.id() == agent);
                // Another machine's session: its row of the one list is its page's title.
                if agent != *id
                    && let Some(row) = self.snapshot.elsewhere(id)
                {
                    let status = match (self.sessions.get(id), open) {
                        (Some(row), _) => row.read(cx).status(),
                        (None, Some(page)) => page.status().clone(),
                        (None, None) => return Page::Empty,
                    };
                    return Page::Session {
                        id: id.clone(),
                        title: row.title.clone(),
                        place: None,
                        status,
                    };
                }
                match (self.snapshot.session(id), self.sessions.get(id), open) {
                    (Some(session), Some(row), _) => Page::Session {
                        id: id.clone(),
                        title: session.title.clone(),
                        place: session.place.clone(),
                        status: row.read(cx).status(),
                    },
                    // A session whose workspace was removed has no row in the tree: its page,
                    // open, stays open.
                    (Some(session), None, Some(page)) => Page::Session {
                        id: id.clone(),
                        title: session.title.clone(),
                        place: session.place.clone(),
                        status: page.status().clone(),
                    },
                    _ => Page::Empty,
                }
            }
        }
    }

    /// "New session": the composer that starts one is on the project page.
    pub fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = match &self.selected {
            Some(Selection::Project(name)) => Some(name.clone()),
            Some(Selection::Session(id)) => self.snapshot.session(id).map(|s| s.project.clone()),
            Some(Selection::ProjectSettings(name)) => Some(name.clone()),
            Some(Selection::Spec { project, .. }) => Some(project.clone()),
            Some(Selection::Settings | Selection::Services | Selection::WhatsNew) | None => None,
        }
        // A hidden project's page can be open (a notification opens it), and no session starts
        // there.
        .filter(|project| !self.is_hidden(project))
        .or_else(|| self.first_shown());
        if let Some(project) = project {
            self.open(Selection::Project(project.clone()), window, cx);
            if let Some(composer) = self.composer_of(&project).cloned() {
                composer.update(cx, |composer, cx| composer.focus_end(window, cx));
            }
        }
    }

    pub fn toggle_bell(&mut self, cx: &mut Context<Self>) {
        self.set_bell(!self.bell_open, cx);
    }

    fn set_bell(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.bell_open != open {
            self.bell_open = open;
            let now = cx.background_executor().now();
            let duration = if open { ENTER } else { HOVER };
            self.bell_shown.animate(
                if open { 1.0 } else { 0.0 },
                duration,
                now,
                cx.reduce_motion(),
            );
            if open {
                self.bell.update(cx, |bell, cx| bell.show(true, cx));
            } else {
                // The marks stay while the list fades out.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(HOVER + Duration::from_millis(20))
                        .await;
                    this.update(cx, |this, cx| {
                        if !this.bell_open {
                            this.bell.update(cx, |bell, cx| bell.show(false, cx));
                        }
                    })
                    .ok();
                })
                .detach();
            }
            cx.notify();
        }
    }

    /// What the bell's items say of their sessions, in order: the session's title, and who
    /// started it when an agent did.
    pub fn bell_sessions(&self, cx: &App) -> Vec<String> {
        self.bell
            .read(cx)
            .rows
            .iter()
            .map(|row| row.title.clone())
            .collect()
    }

    /// The list the bell opens.
    pub fn bell_list(&self) -> &Entity<BellList> {
        &self.bell
    }

    pub fn bell_open(&self) -> bool {
        self.bell_open
    }

    /// Opens what a bell item is about: its session, which marks its notifications read; one
    /// that is no session's is marked read by itself.
    pub fn choose(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((machine, item)) = self
            .bell_items()
            .into_iter()
            .find(|(_, item)| item.key == key)
            .map(|(machine, item)| (machine.map(str::to_owned), item))
        else {
            return;
        };
        // Another machine's: its session opens on that machine's page, which reads it there.
        if let Some(machine) = machine {
            match item.session {
                Some(session) if self.snapshot.elsewhere(&session).is_some() => {
                    self.open(Selection::Session(session), window, cx)
                }
                _ if !item.read && !self.marked_there.contains(&(machine.clone(), item.id)) => {
                    self.read_there(&machine, Reading::Ids(vec![item.id]), vec![item.id])
                }
                _ => {}
            }
            return;
        }
        match item.session {
            Some(session) if self.snapshot.session(&session).is_some() => {
                self.open(Selection::Session(session), window, cx)
            }
            _ if !item.read && self.marked.insert(item.id) => {
                self.perform_here(Command::ReadNotifications(Reading::Ids(vec![item.id])))
            }
            _ => {}
        }
    }

    pub fn open_tree(&mut self, cx: &mut Context<Self>) {
        self.left.update(cx, |drawer, cx| drawer.open(cx));
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A notice keeps its last text while it folds away.
        let (judge_notice, compaction_notice) = (self.judge_notice(), self.compaction_notice());
        for (at, text) in [&judge_notice, &compaction_notice].into_iter().enumerate() {
            if let Some(text) = text {
                self.notices[at] = Some(text.clone());
            }
        }
        let notices = self.notices.clone();
        let (judge_notice, compaction_notice) =
            (judge_notice.is_some(), compaction_notice.is_some());
        let p = palette(cx);
        let now = cx.background_executor().now();
        if self.bell_shown.is_running(now) {
            window.request_animation_frame();
        } else {
            self.bell_shown.settle(now);
        }
        let bell_in = self.bell_shown.value(now);
        let bell_drawn = self.bell_open || bell_in > 0.0;
        let side = match self.layout {
            Layout::Wide => Some(SIDEBAR),
            Layout::Medium => Some(SIDEBAR_MEDIUM),
            Layout::Narrow => None,
        };
        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .flex()
            .when_some(side, |d, width| {
                d.child(
                    div()
                        .flex_none()
                        .w(px(width))
                        .h_full()
                        .border_r_1()
                        .border_color(p.line)
                        .child(self.sidebar.clone()),
                )
            })
            .child(div().flex_1().min_w_0().h_full().child(self.main.clone()))
            .when(side.is_none(), |d| d.child(self.left.clone()));
        let shell = cx.entity().downgrade();
        // The overlays drawn from here on say they are open: Alt+Left closes the topmost.
        forget_overlays(cx);
        if self.bell_open {
            let shell = shell.clone();
            open_overlay(
                "bell-scrim".into(),
                // A key reaches this while the shell handles it: the update waits.
                move |_, cx| {
                    let shell = shell.clone();
                    cx.defer(move |cx| {
                        shell.update(cx, |this, cx| this.set_bell(false, cx)).ok();
                    });
                },
                cx,
            );
        }
        div()
            .id("shell")
            .track_focus(&self.focus)
            // Any input, wherever it goes: the user is at this window.
            .capture_any_mouse_down(
                cx.listener(|this, _, window, _| {
                    this.tell_presence(window.is_window_active(), true)
                }),
            )
            .capture_key_down(
                cx.listener(|this, _, window, _| {
                    this.tell_presence(window.is_window_active(), true)
                }),
            )
            // Reading by the wheel is an input too; no scroll in the window stops it on its way.
            .on_scroll_wheel(
                cx.listener(|this, _, window, _| {
                    this.tell_presence(window.is_window_active(), true)
                }),
            )
            .size_full()
            .relative()
            .bg(p.bg)
            .text_color(p.text)
            .ui_text()
            .flex()
            .flex_col()
            // An open list, modal or drawer takes the buttons first (`dismiss_on_navigate`): what
            // reaches here is a press with nothing open under the pointer.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.go(Way::Back, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.go(Way::Forward, window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "k" && event.keystroke.modifiers == Modifiers::control() {
                    cx.stop_propagation();
                    this.toggle_switcher(window, cx);
                    return;
                }
                // A key no field of an open dialog took: Escape closes the one on top, whichever
                // dialog it is and wherever the focus went when the one above it closed.
                if event.keystroke.key == "escape"
                    && event.keystroke.modifiers == Modifiers::default()
                    && dismiss_top_overlay(window, cx)
                {
                    cx.stop_propagation();
                    return;
                }
                let way = match event.keystroke.key.as_str() {
                    "left" => Way::Back,
                    "right" => Way::Forward,
                    _ => return,
                };
                if event.keystroke.modifiers == Modifiers::alt() {
                    cx.stop_propagation();
                    // Like the mouse's buttons: an open overlay closes first and that is all.
                    if dismiss_top_overlay(window, cx) {
                        // Whatever held the focus in it is going: the next key must still arrive.
                        window.focus(&this.focus, cx);
                        // The overlays say again which are open as the shell is drawn: one that
                        // closed must not be dismissed again by the next key.
                        cx.notify();
                    } else {
                        this.go(way, window, cx);
                    }
                }
            }))
            .child(self.top.clone())
            .child(body)
            .when_some(notices[0].clone(), |d, text| {
                d.child(self.notice_card(true, judge_notice, text, cx))
            })
            .when_some(notices[1].clone(), |d, text| {
                d.child(self.notice_card(false, compaction_notice, text, cx))
            })
            .child(self.add_project.clone())
            .child(self.rename.clone())
            .child(self.group_form.clone())
            .child(self.switcher.clone())
            .when(bell_drawn, |d| {
                let scrim = div()
                    .id("bell-scrim")
                    .absolute()
                    .inset_0()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_bell(cx)));
                d.when(self.bell_open, |d| {
                    d.child(dismiss_on_navigate(scrim, move |_, cx| {
                        shell.update(cx, |this, cx| this.set_bell(false, cx)).ok();
                    }))
                })
                .child(
                    div()
                        .absolute()
                        .top(px(40. + RISE * (1.0 - bell_in)))
                        .right(px(12.))
                        .opacity(bell_in)
                        .child(self.bell.clone()),
                )
            })
            .child(self.modals.clone())
            .child(self.toasts.clone())
    }
}

/// The top bar: the mark, search, and the bell with the count of what waits for the user.
pub struct TopBar {
    shell: WeakEntity<Shell>,
    /// The unread notifications: those that need the user, and the others.
    counts: (usize, usize),
    online: bool,
    menu: bool,
}

impl TopBar {
    fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            counts: (0, 0),
            online: true,
            menu: false,
        }
    }

    /// How many notifications are unread.
    pub fn count(&self) -> usize {
        self.counts.0 + self.counts.1
    }

    /// The unread ones that need the user, and the others.
    pub fn counts(&self) -> (usize, usize) {
        self.counts
    }

    pub fn online(&self) -> bool {
        self.online
    }

    fn set_counts(&mut self, counts: (usize, usize), cx: &mut Context<Self>) {
        if self.counts != counts {
            self.counts = counts;
            cx.notify();
        }
    }

    fn set_online(&mut self, online: bool, cx: &mut Context<Self>) {
        if self.online != online {
            self.online = online;
            cx.notify();
        }
    }

    fn set_menu(&mut self, menu: bool, cx: &mut Context<Self>) {
        if self.menu != menu {
            self.menu = menu;
            cx.notify();
        }
    }
}

/// How often the window looks whether "Restart Flottille" ended.
const RESTART_POLL: Duration = Duration::from_millis(100);

/// `flottille daemon stop`, by the program the window runs in: it ends a Flottille that does not
/// answer.
fn restart_flottille() {
    let stopped = std::env::current_exe().and_then(|program| {
        std::process::Command::new(program)
            .args(["daemon", "stop"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
    });
    if let Err(error) = stopped {
        eprintln!("flottille: Flottille cannot be restarted: {error}");
    }
}

impl Render for TopBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let shell = self.shell.clone();
        let search_shell = self.shell.clone();
        let menu_shell = self.shell.clone();
        let settings_shell = self.shell.clone();
        let services_shell = self.shell.clone();
        let mut bell = Button::icon(
            "bell",
            Icon::Bell,
            counts_said(self.counts, "Notifications"),
        )
        .on_click(move |_, _, cx| {
            shell.update(cx, |shell, cx| shell.toggle_bell(cx)).ok();
        });
        if self.count() > 0 {
            bell = bell.badge(Badge::count(self.count()));
        }
        div()
            .flex_none()
            .h(px(40.))
            .w_full()
            .px(space::MD)
            .flex()
            .items_center()
            .gap(space::MD)
            .bg(p.rail)
            .border_b_1()
            .border_color(p.line)
            .when(self.menu, |d| {
                d.child(Button::icon("open-tree", Icon::Menu, "Projects").on_click(
                    move |_, _, cx| {
                        menu_shell.update(cx, |shell, cx| shell.open_tree(cx)).ok();
                    },
                ))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .px(space::SM)
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(5.))
                            .bg(p.acc)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(logo(p.on_acc).size(px(12.))),
                    )
                    .child(div().mono(12.).medium().child("FLOTTILLE")),
            )
            .child(div().flex_1())
            .when(!self.online, |d| {
                d.child(
                    div()
                        .id("offline")
                        .mono(11.)
                        .text_color(p.faint)
                        .child("reconnecting…"),
                )
            })
            .child(pressable(
                div()
                    .id("top-search")
                    .aria_label("Search")
                    .h(px(26.))
                    .px(space::MD)
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .child(icon(Icon::Search).size(px(13.)))
                    .child("Search")
                    .child(div().mono(11.).child("Ctrl K"))
                    .on_click(move |_, window, cx| {
                        search_shell
                            .update(cx, |shell, cx| shell.toggle_switcher(window, cx))
                            .ok();
                    }),
                move |d, hover, press| {
                    d.bg(mix(p.hover.opacity(hover), p.sel, press))
                        .text_color(mix(p.faint, p.text, hover))
                },
            ))
            .child(
                Button::icon("services", Icon::Service, "Services").on_click(
                    move |_, window, cx| {
                        services_shell
                            .update(cx, |shell, cx| shell.open(Selection::Services, window, cx))
                            .ok();
                    },
                ),
            )
            .child(
                Button::icon("settings", Icon::Settings, "Settings").on_click(
                    move |_, window, cx| {
                        settings_shell
                            .update(cx, |shell, cx| shell.open(Selection::Settings, window, cx))
                            .ok();
                    },
                ),
            )
            .child(bell)
    }
}

/// A notification with the session it belongs to, as the panel groups them.
#[derive(Clone, Debug, PartialEq)]
pub struct BellRow {
    pub item: BellItem,
    /// The session's title, and who started it when an agent did; `Flottille` for what is no
    /// session's.
    pub title: String,
    pub project: String,
}

/// The unread counts said apart: `2 need you · 5 new`; the fallback when nothing is unread.
fn counts_said((needs, new): (usize, usize), none: &str) -> String {
    match (needs, new) {
        (0, 0) => none.to_owned(),
        (needs, 0) => format!("{needs} need you"),
        (0, new) => format!("{new} new"),
        (needs, new) => format!("{needs} need you · {new} new"),
    }
}

/// What the panel's list draws: a session's heading, or one notification under it.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Group {
        session: Option<String>,
        title: String,
        project: String,
        unread: usize,
    },
    Row(BellRow),
}

/// The panel's order: the sessions with something unread first, the one that heard last on top;
/// under each, its unread notifications first, then the rest, the newest first.
fn grouped(rows: &[BellRow]) -> Vec<(String, Entry)> {
    let mut groups: Vec<(Option<String>, Vec<&BellRow>)> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|(session, _)| *session == row.item.session)
        {
            Some((_, found)) => found.push(row),
            None => groups.push((row.item.session.clone(), vec![row])),
        }
    }
    let newest = |rows: &[&BellRow], unread: bool| {
        rows.iter()
            .filter(|row| !unread || !row.item.read)
            .map(|row| row.item.at)
            .max()
            .unwrap_or(0)
    };
    groups.sort_by_key(|(_, rows)| {
        let unread = rows.iter().any(|row| !row.item.read);
        std::cmp::Reverse((unread, newest(rows, unread)))
    });
    let mut entries = Vec::new();
    for (session, mut rows) in groups {
        rows.sort_by_key(|row| (row.item.read, std::cmp::Reverse((row.item.at, row.item.id))));
        let first = rows[0];
        entries.push((
            format!("bell-group-{}", session.as_deref().unwrap_or("flottille")),
            Entry::Group {
                session,
                title: first.title.clone(),
                project: first.project.clone(),
                unread: rows.iter().filter(|row| !row.item.read).count(),
            },
        ));
        for row in rows {
            entries.push((row.item.key.clone(), Entry::Row(row.clone())));
        }
    }
    entries
}

/// The element id of a session's heading in the panel, and of its title.
pub fn bell_group_id(session: &str) -> ElementId {
    ElementId::Name(format!("bell-group-{session}").into())
}

pub fn bell_group_title_id(session: &str) -> ElementId {
    ElementId::Name(format!("bell-group-{session}-title").into())
}

/// The element id of a notification's line under its title, which is its excerpt.
pub fn bell_line_id(key: &str) -> ElementId {
    ElementId::Name(format!("bell-{key}-line").into())
}

/// The panel the bell opens: the notification centre the daemon keeps, grouped by session, the
/// unread first, each with its time. Each line is one line cut with an ellipsis. It draws nothing
/// while it is closed, so that a closed panel asks for no frame.
pub struct BellList {
    shell: WeakEntity<Shell>,
    rows: Vec<BellRow>,
    counts: (usize, usize),
    list: Listing<Entry>,
    open: bool,
}

impl BellList {
    fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            rows: Vec::new(),
            counts: (0, 0),
            list: Listing::default(),
            open: false,
        }
    }

    fn set(&mut self, rows: Vec<BellRow>, counts: (usize, usize), cx: &mut Context<Self>) {
        self.rows = rows;
        self.counts = counts;
        cx.notify();
    }

    fn set_counts(&mut self, counts: (usize, usize), cx: &mut Context<Self>) {
        self.counts = counts;
        cx.notify();
    }

    fn show(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        cx.notify();
    }

    /// The words of each notification, in the order they were kept: why it is there, its
    /// excerpt, its session.
    pub fn said(&self) -> Vec<(String, String, String)> {
        self.rows
            .iter()
            .map(|row| {
                (
                    label_of(&row.item),
                    row.item.detail.clone(),
                    row.title.clone(),
                )
            })
            .collect()
    }

    /// What needs the user and what is new, every machine's.
    pub fn counts(&self) -> (usize, usize) {
        self.counts
    }

    /// The panel's order: each session's key, then the keys of its notifications.
    pub fn order(&self) -> Vec<String> {
        grouped(&self.rows)
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }
}

/// Why a notification is there: once what it waited for was answered, it says so rather than
/// that it still waits.
fn label_of(item: &BellItem) -> String {
    if !item.resolved {
        return item.label.clone();
    }
    match item.kind.as_str() {
        "request" => match item.label.strip_suffix(" waits for you") {
            Some(request) => format!("{request} answered"),
            None => "Request answered".to_owned(),
        },
        "question" => "Question answered".to_owned(),
        "vault" => "Vault entry answered".to_owned(),
        "paused" => "The session wakes again".to_owned(),
        _ => item.label.clone(),
    }
}

/// How a kind of notification is drawn in front of its title.
fn glyph_of(kind: &str) -> gpui_design_system::Icon {
    match kind {
        "request" | "question" | "vault" => Icon::Bell,
        "ticket" => glyphs::TICKET,
        "pull_request" => glyphs::PULL_REQUEST,
        // A failure is told, never confused with a request that waits.
        "failure" | "unchecked" | "put_back" | "dropped" => Icon::Warning,
        _ => Icon::Text,
    }
}

impl Render for BellList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let at = cx.background_executor().now();
        let entries = grouped(&self.rows);
        self.list.sync(entries, at, cx.reduce_motion());
        let now = now_ms();
        let list = div()
            .id("bell-list")
            .occlude()
            .w(px(360.))
            .max_h(px(480.))
            .overflow_y_scroll()
            .p(space::SM)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line2)
            .flex()
            .flex_col();
        let shell = self.shell.clone();
        let list = dismiss_on_navigate(list, move |_, cx| {
            shell.update(cx, |shell, cx| shell.set_bell(false, cx)).ok();
        });
        let header_shell = self.shell.clone();
        let header = div()
            .flex_none()
            .px(space::SM)
            .pb(space::SM)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("bell-counts")
                    .aria_label(counts_said(self.counts, "Nothing unread"))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(counts_said(self.counts, "Nothing unread"))
                    .test_support(),
            )
            .child(
                Button::quiet("bell-mark-all", "Mark all read")
                    .disabled(self.counts == (0, 0))
                    .on_click(move |_, _, cx| {
                        header_shell.update(cx, |shell, _| shell.read_all()).ok();
                    }),
            );
        let rows: Vec<AnyElement> =
            self.list
                .rows()
                .map(|row| {
                    let content = match row.value {
                        Entry::Group {
                            session,
                            title,
                            project,
                            unread,
                        } => {
                            let key = session.as_deref().unwrap_or("flottille");
                            let faded = fade(
                                ElementId::Name(format!("bell-group-{key}-unread").into()),
                                *unread > 0,
                                HOVER,
                                window,
                                cx,
                            );
                            div()
                                .id(bell_group_id(key))
                                .px(space::SM)
                                .pt(space::MD)
                                .pb(space::XS)
                                .flex()
                                .items_baseline()
                                .gap(space::SM)
                                .min_w_0()
                                .child(
                                    div()
                                        .id(bell_group_title_id(key))
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(12.5))
                                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                                        .text_color(mix(p.mute, p.text, faded))
                                        .aria_label(SharedString::from(title.clone()))
                                        .child(SharedString::from(title.clone()))
                                        .test_support(),
                                )
                                .when(!project.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(11.))
                                            .text_color(p.faint)
                                            .child(SharedString::from(project.clone())),
                                    )
                                })
                                .test_support()
                                .into_any_element()
                        }
                        Entry::Row(BellRow { item, .. }) => {
                            let shell = self.shell.clone();
                            let key = item.key.clone();
                            // Read or unread moves: the ink and the fill fade over `HOVER`.
                            let unread = fade(
                                ElementId::Name(format!("bell-{}-unread", item.key).into()),
                                !item.read,
                                HOVER,
                                window,
                                cx,
                            );
                            // Answered, its glyph gives way to a check: the two cross-fade.
                            let done = fade(
                                ElementId::Name(format!("bell-{}-resolved", item.key).into()),
                                item.resolved,
                                HOVER,
                                window,
                                cx,
                            );
                            let ink = mix(p.faint, p.acc, unread);
                            let leading = div()
                                .relative()
                                .size(px(14.))
                                .child(div().absolute().inset_0().opacity(1. - done).child(
                                    icon(glyph_of(&item.kind)).size(px(14.)).text_color(ink),
                                ))
                                .child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .opacity(done)
                                        .child(icon(Icon::Check).size(px(14.)).text_color(ink)),
                                )
                                .into_any_element();
                            let line = |id: ElementId, text: &str, ink| {
                                div()
                                    .id(id)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .text_color(ink)
                                    .aria_label(SharedString::from(text.to_owned()))
                                    .child(SharedString::from(text.to_owned()))
                                    .test_support()
                            };
                            let below = div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .when(!item.detail.is_empty(), |d| {
                                    d.child(line(bell_line_id(&item.key), &item.detail, p.mute))
                                });
                            let when = crate::model::ago(now, item.at);
                            ListRow::new(
                                ElementId::Name(format!("bell-{}", item.key).into()),
                                SharedString::from(label_of(item)),
                            )
                            .leading(leading)
                            .below(below)
                            .trailing(
                                div()
                                    .id(ElementId::Name(format!("bell-{}-time", item.key).into()))
                                    .text_size(px(11.))
                                    .text_color(p.faint)
                                    .aria_label(SharedString::from(when.clone()))
                                    .child(SharedString::from(when))
                                    .test_support(),
                            )
                            .decorate(move |row| row.opacity(0.6 + 0.4 * unread))
                            .on_click(move |_, window, cx| {
                                shell
                                    .update(cx, |shell, cx| shell.choose(&key, window, cx))
                                    .ok();
                            })
                            .into_any_element()
                        }
                    };
                    Item::new(
                        ElementId::Name(format!("bell-item-{}", row.key).into()),
                        row.placement,
                        content,
                    )
                    .into_any_element()
                })
                .collect();
        list.child(header)
            .child(presence(
                "bell-empty",
                self.list.is_empty(),
                div()
                    .p(space::MD)
                    .text_color(p.faint)
                    .child("No notifications."),
            ))
            .children(rows)
            .test_support()
            .into_any_element()
    }
}

/// The Flottille mark, drawn as `assets/brand/mark.svg` draws it: its geometry, in that file's
/// 64-unit box. The sails are filled triangles with a round-joined outline; the hull is one
/// curved stroke with round ends.
const MARK_SAILS: [[(f32, f32); 3]; 2] = [
    [(19., 8.), (19., 38.), (37., 38.)],
    [(44., 19.), (44., 38.), (57., 38.)],
];
const MARK_SAIL_OUTLINE: f32 = 4.;
const MARK_HULL: [(f32, f32); 4] = [(7., 46.), (20., 53.5), (44., 53.5), (57., 46.)];
const MARK_HULL_STROKE: f32 = 6.4;

/// The Flottille mark in the box it is given: two filled sails over one curved hull.
fn logo(color: gpui_kit::Hsla) -> gpui_kit::Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let unit = f32::from(bounds.size.width) / 64.0;
            let at = |(x, y): (f32, f32)| {
                point(
                    bounds.origin.x + px(x * unit),
                    bounds.origin.y + px(y * unit),
                )
            };
            let stroke = |width: f32| {
                PathBuilder::fill().with_style(PathStyle::Stroke(
                    StrokeOptions::default()
                        .with_line_width(width * unit)
                        .with_line_join(LineJoin::Round)
                        .with_line_cap(LineCap::Round),
                ))
            };
            for sail in MARK_SAILS {
                let mut fill = PathBuilder::fill();
                let mut outline = stroke(MARK_SAIL_OUTLINE);
                for builder in [&mut fill, &mut outline] {
                    builder.move_to(at(sail[0]));
                    builder.line_to(at(sail[1]));
                    builder.line_to(at(sail[2]));
                    builder.close();
                }
                for builder in [fill, outline] {
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }
                }
            }
            let mut hull = stroke(MARK_HULL_STROKE);
            hull.move_to(at(MARK_HULL[0]));
            hull.cubic_bezier_to(at(MARK_HULL[3]), at(MARK_HULL[1]), at(MARK_HULL[2]));
            if let Ok(path) = hull.build() {
                window.paint_path(path, color);
            }
        },
    )
}

/// The rows of a project's live sessions as the tree draws them: a session led by another of
/// them sits right under its pilot's row, indented, while that row is unfolded; the others keep
/// their order. Each row learns who leads it and how many it leads.
fn nested(
    live: &[&crate::model::Session],
    rows: &[Entity<SessionRow>],
    cx: &mut Context<Shell>,
) -> Vec<Entity<SessionRow>> {
    // The place of each session's pilot among them, when it is one of them.
    let mut pilots: Vec<Option<usize>> = live
        .iter()
        .map(|session| {
            let pilot = session.pilot.as_deref().filter(|p| *p != session.id)?;
            live.iter().position(|other| other.id == pilot)
        })
        .collect();
    let mut out = Vec::new();
    let mut placed = HashSet::new();
    // Depth first from each session no other leads; `shown` is false under a folded row. The tie
    // of a row is the lines the rows above it go on with and whether it is its pilot's last.
    let mut stack: Vec<(usize, bool, Option<Tie>)> = (0..live.len())
        .rev()
        .filter(|&at| pilots[at].is_none())
        .map(|at| (at, true, None))
        .collect();
    loop {
        let Some((at, shown, tie)) = stack.pop() else {
            // A row no walk reached sits in a loop of pilots, which the model drops; if one got
            // here, it is a root, so that no row of the project is ever left out.
            let Some(at) = (0..live.len()).find(|at| !placed.contains(at)) else {
                break;
            };
            pilots[at] = None;
            stack.push((at, true, None));
            continue;
        };
        if !placed.insert(at) {
            continue;
        }
        let row = &rows[at];
        let led_by = pilots[at].and(live[at].led_by.as_deref());
        if row.read(cx).led_by() != led_by {
            row.update(cx, |row, cx| row.set_led_by(led_by, cx));
        }
        // A session led from another project sits among its own project's rows, and says who
        // leads it there, with that project (`model`), before what it runs on.
        let runs = match (led_by, pilots[at], live[at].led_by.as_deref()) {
            (Some(_), _, _) => live[at].runs.clone(),
            (None, None, Some(by)) if live[at].led_from_elsewhere => {
                match live[at].runs.is_empty() {
                    true => format!("led by {by}"),
                    false => format!("led by {by} · {}", live[at].runs),
                }
            }
            _ => String::new(),
        };
        if row.read(cx).runs() != runs {
            row.update(cx, |row, cx| row.set_runs(&runs, cx));
        }
        if row.read(cx).tie() != tie.as_ref() {
            let tie = tie.clone();
            row.update(cx, |row, cx| row.set_tie(tie, cx));
        }
        let led: Vec<usize> = (0..live.len())
            .filter(|&other| pilots[other] == Some(at))
            .collect();
        if row.read(cx).leads() != led.len() {
            row.update(cx, |row, cx| row.set_leads(led.len(), cx));
        }
        // Folded, the row shows what the sessions it hides ask of the user.
        let hidden = match row.read(cx).is_open() {
            true => None,
            false => {
                let mut below = led.clone();
                let mut marks = Vec::new();
                while let Some(at) = below.pop() {
                    marks.extend(live[at].mark);
                    below.extend((0..live.len()).filter(|&other| pilots[other] == Some(at)));
                }
                AgentState::strongest(marks)
            }
        };
        if row.read(cx).hidden() != hidden {
            row.update(cx, |row, cx| row.set_hidden(hidden, cx));
        }
        if shown {
            out.push(row.clone());
        }
        let open = shown && row.read(cx).is_open();
        // A child's line goes on past its own children's rows while it has a sibling below.
        let through = match &tie {
            Some(tie) => [tie.through.clone(), vec![!tie.last]].concat(),
            None => Vec::new(),
        };
        let last = led.len().saturating_sub(1);
        stack.extend(led.into_iter().enumerate().rev().map(|(place, other)| {
            let tie = Tie {
                through: through.clone(),
                last: place == last,
            };
            (other, open, Some(tie))
        }));
    }
    out
}

#[cfg(test)]
mod mark {
    use super::*;

    /// The numbers of the first attribute `name="…"` of each element that has one.
    fn attributes(svg: &str, name: &str) -> Vec<String> {
        let key = format!(" {name}=\"");
        svg.match_indices(&key)
            .map(|(at, _)| {
                let rest = &svg[at + key.len()..];
                rest[..rest.find('"').unwrap()].to_owned()
            })
            .collect()
    }

    fn numbers(path: &str) -> Vec<f32> {
        path.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter(|n| !n.is_empty())
            .map(|n| n.parse().unwrap())
            .collect()
    }

    #[test]
    fn the_corner_mark_is_the_brand_mark() {
        let svg = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/brand/mark.svg"
        ))
        .unwrap();
        let paths = attributes(&svg, "d");
        assert_eq!(paths.len(), 3, "two sails and a hull: {paths:?}");
        let flat = |points: &[(f32, f32)]| -> Vec<f32> {
            points.iter().flat_map(|&(x, y)| [x, y]).collect()
        };
        assert_eq!(numbers(&paths[0]), flat(&MARK_SAILS[0]));
        assert_eq!(numbers(&paths[1]), flat(&MARK_SAILS[1]));
        assert_eq!(numbers(&paths[2]), flat(&MARK_HULL));
        // Sails are filled and outlined; the hull is a stroke alone.
        assert_eq!(attributes(&svg, "fill"), ["#ececec", "#ececec", "none"]);
        let widths = attributes(&svg, "stroke-width");
        let outline = MARK_SAIL_OUTLINE.to_string();
        assert_eq!(widths[..2], [outline.clone(), outline]);
        assert_eq!(widths[2], MARK_HULL_STROKE.to_string());
    }
}
