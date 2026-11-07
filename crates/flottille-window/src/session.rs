//! A session's page: a header on one line (its title and its place, no state: the state is drawn
//! once, by the icons of the tree and the rail), the shown agent's conversation, live, the
//! composer that writes to the root agent (or, reading a sub-agent, a line saying it belongs to
//! its parent; once the session ended, a line saying why, with Resume when it can be), and a rail
//! with, from its top, the session's agents, its workspace, its runs, and what waits for the user
//! pinned at its bottom; the rail becomes a drawer in a narrower window. A run opens its detail in
//! a drawer that slides in from the right.
//!
//! Every row of the conversation is its own view: a message arriving adds a row or draws one row
//! again, never the others. What an agent, a file or a command said is shown as the daemon gave
//! it (masked and escaped): an agent's replies and thoughts drawn as Markdown, whose images are
//! never loaded, the rest as plain text.
//!
//! The root agent has no name on screen: where a text names it, it is "the conversation".

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use flottille_ui::card::Block as CardBlock;
use flottille_ui::color::mix;
use flottille_ui::motion::{HOVER, Motion, PANEL};
use flottille_ui::{
    AgentState, Badge, Button, Card, Drawer, Field, Icon, ListRow, Listing, Menu, Modal, Palette,
    Pill, PulsingLabel, Side, Skeleton, StateIcon, Status, Toast, Type, awaiting, changing, enter,
    fade, fold_frame, follow_theme, hoverable, icon, palette, path_line, presence, presence_of,
    section_label, selectable, space, text_region, tipped_when_cut, toast, turning,
};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::base::input::{
    Escape, Input, InputEvent, InputState, MoveDown, MoveUp, TextareaState,
};
use gpui_kit::base::text::{TextView, TextViewState, TextViewStyle};
use gpui_kit::base::{ElementExt as _, TextSelection, TextSelectionScopeId};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, ElementId, Entity, EventEmitter,
    Focusable as _, FollowMode, ImageCacheError, ImageSource, InteractiveElement, IntoElement,
    ListAlignment, ListOffset, ListState, MouseButton, ParentElement, PathBuilder, Pixels, Render,
    ScrollHandle, SharedString, SharedUri, Stateful, StatefulInteractiveElement, Styled,
    StyledImage as _, Subscription, Task, TestSupportExt as _, Transformation, WeakEntity, Window,
    canvas, div, list, point, px, radians,
};

use crate::actions::{Answer, Command, Picture, RepositoryStatus, RunAct, RunAnswer};
use crate::attachments::{Attachments, STILL_SCALING};
use crate::commands::CommandInfo;
use crate::conversation::{
    Activity, Block, Call, CallState, Change, Conversation, DiffLine, Ending, STARTED_SESSION_KIND,
    Summary, USER, WOKEN_BY_KIND, in_sight, is_hidden,
};
use crate::leaves::LeaveList;
use crate::link::{Connect, Update};
use crate::links_card::{LinksCard, Open};
use crate::mode::ModeSwitch;
use crate::model::Place;
use crate::open::Opener;
use crate::providers::Providers;
use crate::queued::QueuedMessages;
use crate::runs::{self, CommandList, CommandPanel, EndedFold, ended_row, rail_label};
use crate::session_link::{
    self, ForgeFacts, PlaceFacts, PullRequest, RepositoryFacts, Request, SessionLink,
    SessionUpdate, TicketFacts, TreeAgent, Where,
};
use crate::session_options::{Runs, SessionOptions};
use crate::shell::{Layout, Selection, Shell, now_ms};
use crate::subscriptions::SubscriptionList;
use crate::to_end::ToEnd;
use crate::tree::place_name;
use crate::waiting::{Waiting, sentences};
use flottille_ui::drawer::Closed;

/// Width of the session page's right rail.
pub const RAIL: f32 = 290.;

/// How the screen names the root agent, which has no name of its own there.
pub const THE_CONVERSATION: &str = "the conversation";

/// What the confirmation of ending a session says: how many agents end.
pub fn end_sentence(count: usize) -> String {
    match count {
        0 | 1 => format!("This ends {THE_CONVERSATION}. Its workspace stays."),
        n => format!(
            "This ends {n} agents: {THE_CONVERSATION} and its {} sub-agent{}. Workspaces stay.",
            n - 1,
            if n == 2 { "" } else { "s" }
        ),
    }
}

/// The reply field's placeholder and its send button's tooltip: the field writes to the root.
pub const REPLY_PLACEHOLDER: &str = "Reply…";
pub const SEND: &str = "Send";
/// The composer's button that interrupts the turn that runs.
pub const STOP: &str = "Stop the turn";
/// The reply field is one line high when empty, and grows with its lines up to this many.
const REPLY_MAX_ROWS: usize = 10;
/// What a toast says when the daemon refuses the message, or the interruption.
pub const NOT_SENT: &str = "The message was not sent";
/// What a message says under it from the moment it is sent until the daemon records it.
pub const SENDING: &str = "Sending…";
/// What a message the daemon refused says before its reason.
pub const NOT_SENT_LINE: &str = "Not sent";
/// The button of a refused message that sends it again.
pub const RETRY: &str = "Retry";
/// Why a message the daemon took is not sent after all: the agent failed, or ended, first.
pub const AGENT_FAILED: &str = "the agent failed before it read it";
pub const AGENT_ENDED: &str = "the session ended before the agent read it";
pub const NEVER_READ: &str = "the agent ended before it read it";
pub const NOT_INTERRUPTED: &str = "The turn was not interrupted";
/// The state an agent of the tree is drawn with, from what the daemon says of it: only a root can
/// need the user; a child waiting on its parent is waiting, in grey.
pub fn state_of(agent: &TreeAgent) -> AgentState {
    match agent.state.as_str() {
        "failed" => AgentState::Failed,
        "ended" if agent.state_reason.as_deref() == Some("it reported") => AgentState::Completed,
        "ended" => AgentState::Stopped,
        _ if agent.parent.is_none() && agent.needs_user => AgentState::NeedsYou,
        _ if agent.parent.is_some() && agent.waiting_on.as_deref() == Some("parent") => {
            AgentState::Waiting
        }
        // Between turns, for what happens outside: a pull request's checks, a review.
        _ if agent.waiting_on.as_deref() == Some("subscription") => AgentState::Waiting,
        "starting" | "working" => AgentState::Running,
        _ => AgentState::Idle,
    }
}

/// How the page names an agent: its role for the root, its task's title for a child.
pub fn name_of(agent: &TreeAgent) -> String {
    match (&agent.parent, &agent.title) {
        (None, _) => match agent.role.as_str() {
            "" => "root".into(),
            role => role.to_owned(),
        },
        (Some(_), Some(title)) if !title.trim().is_empty() => title.clone(),
        (Some(_), _) => format!("agent {}", agent.id),
    }
}

/// A duration as the page says it: `0.4s`, `38s`, `4m`.
pub fn seconds(ms: u64) -> String {
    match ms {
        0..10_000 => format!("{:.1}s", ms as f64 / 1000.0),
        10_000..120_000 => format!("{}s", ms / 1000),
        _ => format!("{}m", ms / 60_000),
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// When a message was said, in the machine's local time: `31 Oct 2026, 14:05`.
pub fn said_at(ms: u64) -> String {
    said_at_offset(ms, local_offset(ms))
}

/// The time of day of `ms` on the machine's clock: `10:42`.
pub fn clock(ms: u64) -> String {
    let said = said_at(ms);
    said.rsplit(", ").next().unwrap_or(&said).to_owned()
}

/// [`said_at`] at `offset` seconds east of UTC.
pub fn said_at_offset(ms: u64, offset: i64) -> String {
    let seconds = (ms / 1000) as i64 + offset;
    let (days, time) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // The civil date of a count of days since 1970 (Howard Hinnant's `civil_from_days`).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{day} {} {year}, {:02}:{:02}",
        MONTHS[(month - 1) as usize],
        time / 3600,
        time % 3600 / 60
    )
}

/// How far east of UTC the machine's clock was at `ms`, in seconds.
fn local_offset(ms: u64) -> i64 {
    let time = (ms / 1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only into `tm`, which outlives the call; a zeroed `tm` is a
    // valid one.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    match unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        true => 0,
        false => tm.tm_gmtoff,
    }
}

/// What the page needs of the shell to follow a session.
#[derive(Clone)]
pub struct Follow {
    pub connect: Connect,
    pub updates: UnboundedSender<Update>,
    pub opener: Opener,
}

/// What the shell knows of the session when it opens it.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    /// The root agent.
    pub id: String,
    pub title: String,
    pub project: String,
    pub place: Place,
    /// The place's folder, for the editor.
    pub folder: Option<PathBuf>,
    /// The agent that started the session for the user, by the name it goes by; `None` for a
    /// session the user started. The header says it, not the left tree.
    pub started_by: Option<String>,
}

pub struct SessionPage {
    shell: WeakEntity<Shell>,
    opened: Opened,
    follow: Follow,
    layout: Layout,
    /// The session's state, as the shell's tree draws it.
    status: Entity<Status>,
    /// The agent whose conversation is shown: the root, or a sub-agent read.
    shown: String,
    link: Option<SessionLink>,
    tree: Vec<TreeAgent>,
    /// The providers as the daemon lists them, to name the model an agent runs on.
    providers: Providers,
    /// An agent was shown before the tree was read: `set_tree` checks that it is in it.
    unchecked: bool,
    /// One state per agent, read by its rail row and its line in the sub-agents list.
    statuses: HashMap<String, Entity<Status>>,
    conversation: Entity<ConversationView>,
    rail: Entity<SessionRail>,
    drawer: Entity<Drawer>,
    reply: Entity<Reply>,
    /// What the user wrote to the root while it worked, waiting under the conversation.
    queued: Entity<QueuedMessages>,
    /// The detail of one run, which follows its output.
    command_panel: Entity<CommandPanel>,
    /// The drawer the detail of a run slides in with.
    run_drawer: Entity<Drawer>,
    /// The command whose output the link reads, when its panel is open.
    watching: Option<String>,
    requests: HashMap<u64, Request>,
    decisions: HashMap<u64, String>,
    /// How many pages of the shown agent's conversation are read, from its end.
    pages: usize,
    /// The shown agent's conversation was read by the current link.
    read: bool,
    /// How many times a conversation was read and its rows made again.
    histories: usize,
    /// The pictures sent to the shown agent, read back from the daemon by name, and the names
    /// asked for.
    pictures: HashMap<String, Arc<gpui_kit::Image>>,
    asked_pictures: HashSet<String>,
    needs_you: bool,
    /// The confirmation of ending the session is open.
    ending: bool,
    /// What the daemon said when it refused to end the session.
    end_error: Option<SharedString>,
    /// A resume was asked and not answered yet.
    resuming: bool,
    /// What the daemon said when it refused to resume the session.
    resume_error: Option<SharedString>,
    /// A reload of the root's tools was asked and not answered yet.
    reloading: bool,
    renders: usize,
    _status: Subscription,
    _run_drawer: Subscription,
    _reply: Subscription,
}

/// Emitted when the user chooses the conversation of an agent of the session.
pub struct Chose(pub String);

impl EventEmitter<Chose> for SessionPage {}

/// What the page says above the composer of a session that runs an older list of tools than its
/// role has (`docs/tools.md`, "When the tools change").
pub const OLDER_TOOLS: &str = "This session runs an older list of tools than its role has now: \
    reload them for the new one.";

/// The action that restarts the root's process on its conversation with the tools its role has
/// now: the outdated line's button, the root's menu and `/reload-tools`.
pub const RELOAD_TOOLS: &str = "Reload tools";

/// The name of the composer's own command that reloads the tools.
pub const RELOAD_COMMAND: &str = "reload-tools";

/// What `/reload-tools` says in the composer's list.
pub const RELOAD_COMMAND_LINE: &str =
    "Restart the agent on its conversation with the tools its role has now";

/// What the page says above the composer while a reload waits for the turn to end.
pub const RELOADS_AFTER_TURN: &str = "Reloads after this turn";

/// The toast of a reload asked between turns: the tools come with the next message, which
/// starts the next process.
pub const RELOADED: &str = "Tools reload at its next message";

/// The toast of a reload the daemon refused.
pub const NOT_RELOADED: &str = "The tools were not reloaded";

/// The line of an older list of tools, for the session's root: only while it has not ended.
pub fn older_tools(root: Option<&TreeAgent>) -> Option<&'static str> {
    root.filter(|root| root.tools_outdated && !matches!(root.state.as_str(), "ended" | "failed"))
        .map(|_| OLDER_TOOLS)
}

impl SessionPage {
    pub fn new(
        shell: WeakEntity<Shell>,
        opened: Opened,
        status: Entity<Status>,
        reply: Entity<Reply>,
        follow: Follow,
        layout: Layout,
        cx: &mut Context<Self>,
    ) -> Self {
        let page = cx.entity().downgrade();
        let rail = cx.new(|cx| SessionRail::new(page.clone(), opened.id.clone(), &opened, cx));
        let command_panel = cx.new(|_| CommandPanel::new(page.clone()));
        let run_drawer = cx.new(|cx| Drawer::detail("run-drawer", command_panel.clone(), cx));
        // Escape, or a page left, closes the drawer: the output is read no more.
        let _run_drawer = cx.subscribe(&run_drawer, |this, _, _: &Closed, cx| {
            this.close_command(cx);
        });
        let drawer = cx.new(|_| Drawer::new("rail-drawer", Side::Right, px(RAIL), rail.clone()));
        let shown = opened.id.clone();
        let conversation =
            cx.new(|cx| ConversationView::new(page, &shown, THE_CONVERSATION.into(), None, cx));
        let _status = cx.observe(&status, |this, status, cx| {
            let needs_you = status.read(cx).state() == AgentState::NeedsYou;
            if this.needs_you != needs_you {
                this.needs_you = needs_you;
                cx.notify();
            }
        });
        let needs_you = status.read(cx).state() == AgentState::NeedsYou;
        // A message sent, answered or recorded shows in the conversation, or under it.
        let _reply = cx.observe(&reply, |this, _, cx| this.sync_pending(cx));
        let mut page = Self {
            shell: shell.clone(),
            follow,
            layout,
            status,
            shown,
            link: None,
            tree: Vec::new(),
            providers: Providers::default(),
            unchecked: false,
            statuses: HashMap::new(),
            conversation,
            rail,
            drawer,
            reply,
            queued: cx.new(|_| QueuedMessages::new(shell, opened.id.clone())),
            command_panel,
            run_drawer,
            watching: None,
            requests: HashMap::new(),
            decisions: HashMap::new(),
            pages: 1,
            read: false,
            histories: 0,
            pictures: HashMap::new(),
            asked_pictures: HashSet::new(),
            needs_you,
            ending: false,
            end_error: None,
            resuming: false,
            resume_error: None,
            reloading: false,
            renders: 0,
            _status,
            _run_drawer,
            _reply,
            opened,
        };
        page.relink();
        page.sync_pending(cx);
        page
    }

    /// Shows the messages being sent where they are to be: in the conversation, or under it
    /// with the waiting ones when a turn ran as they were sent. One the daemon put in the queue
    /// is the waiting one now, in its place.
    fn sync_pending(&mut self, cx: &mut Context<Self>) {
        let queued: Vec<String> = self
            .queued
            .read(cx)
            .messages()
            .iter()
            .map(|m| m.delivery.clone())
            .collect();
        let waiting: Vec<(u64, String, bool)> = self
            .reply
            .read(cx)
            .pending()
            .iter()
            .filter(|p| p.refused.is_none())
            .filter_map(|p| {
                let delivery = &p.accepted.as_ref()?.delivery;
                queued
                    .contains(delivery)
                    .then(|| (p.key, delivery.clone(), p.queued))
            })
            .collect();
        for (key, delivery, here) in waiting {
            if here {
                self.queued
                    .update(cx, |queued, cx| queued.alias(delivery, key, cx));
            }
            self.reply.update(cx, |reply, cx| reply.resolve(key, cx));
        }
        // What was sent is recorded after the last event the root's conversation shows now.
        if self.shown == self.opened.id {
            let last = self.conversation.read(cx).model.last_seq();
            self.reply.update(cx, |reply, _| reply.stamp(last));
        }
        let all = self.reply.read(cx).pending().to_vec();
        let (conversation, this) = (self.conversation.clone(), &*self);
        conversation.update(cx, |view, cx| view.sync_pending(&all, this, cx));
        let waiting: Vec<Pending> = self
            .reply
            .read(cx)
            .pending()
            .iter()
            .filter(|p| p.queued)
            .cloned()
            .collect();
        self.queued
            .update(cx, |queued, cx| queued.set_pending(waiting, cx));
    }

    /// What the root's events say of the messages being sent: one taken back leaves, one that
    /// will never be read (its agent ended) stays, refused with why.
    fn sending_left(&mut self, event: &flottille_core::protocol::Event, cx: &mut Context<Self>) {
        let data = &event.data;
        let reason = data["reason"].as_str().unwrap_or_default();
        let ids = || -> Vec<String> {
            data["deliveries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect()
        };
        let reply = self.reply.clone();
        match event.name.as_str() {
            "agent.failed" => {
                let why = format!("{AGENT_FAILED}: {reason}");
                reply.update(cx, |reply, cx| reply.root_ended(&why, cx));
            }
            "agent.ended" => {
                let why = format!("{AGENT_ENDED}: {reason}");
                reply.update(cx, |reply, cx| reply.root_ended(&why, cx));
            }
            "delivery.withdrawn" => reply.update(cx, |reply, cx| {
                for id in ids() {
                    reply.delivery_left(&id, None, cx);
                }
            }),
            "delivery.superseded" => reply.update(cx, |reply, cx| {
                for id in ids() {
                    reply.delivery_left(&id, Some(NEVER_READ.to_owned()), cx);
                }
            }),
            _ => {}
        }
    }

    pub fn id(&self) -> &str {
        &self.opened.id
    }

    /// The session's state, as the shell's tree draws it.
    pub fn status(&self) -> &Entity<Status> {
        &self.status
    }

    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// The providers as the daemon lists them: the lines that say what the agents run on name
    /// each provider's default model by what they say.
    pub fn set_providers(&mut self, providers: &Providers, cx: &mut Context<Self>) {
        if self.providers == *providers {
            return;
        }
        self.providers = providers.clone();
        let options = self.reply.read(cx).options.clone();
        options.update(cx, |options, cx| options.set_providers(providers, cx));
        if !self.tree.is_empty() {
            self.set_tree(self.tree.clone(), cx);
        }
    }

    pub fn token(&self) -> Option<u64> {
        self.link.as_ref().map(SessionLink::token)
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// How many times a conversation was read and its rows made again.
    pub fn histories(&self) -> usize {
        self.histories
    }

    /// The decision the page knows of a request.
    pub fn decision(&self, number: u64) -> Option<&str> {
        self.decisions
            .get(&number)
            .or_else(|| self.requests.get(&number)?.decision.as_ref())
            .map(String::as_str)
    }

    pub fn tree(&self) -> &[TreeAgent] {
        &self.tree
    }

    pub fn rail(&self) -> &Entity<SessionRail> {
        &self.rail
    }

    /// The rail's Runs section.
    pub fn commands(&self, cx: &gpui_kit::App) -> Entity<CommandList> {
        self.rail.read(cx).commands.clone()
    }

    /// The rail's Subscriptions section.
    pub fn subscriptions(&self, cx: &gpui_kit::App) -> Entity<SubscriptionList> {
        self.rail.read(cx).subscriptions.clone()
    }

    /// The rail's Write access section.
    pub fn leaves(&self, cx: &gpui_kit::App) -> Entity<LeaveList> {
        self.rail.read(cx).leaves.clone()
    }

    /// The rail's "Waiting for you" section.
    pub fn waiting(&self, cx: &gpui_kit::App) -> Entity<Waiting> {
        self.rail.read(cx).waiting.clone()
    }

    /// The panel that follows a command's output.
    pub fn command_panel(&self) -> &Entity<CommandPanel> {
        &self.command_panel
    }

    /// The drawer a run's detail slides in with.
    /// The name of a session a message was relayed from or to, as the sidebar says it: the
    /// daemon names a session that has no title of its own by its agent (`agent <id>`).
    fn peer_named(&self, peer: &str, cx: &App) -> String {
        peer.strip_prefix("agent ")
            .and_then(|id| {
                let shell = self.shell.upgrade()?;
                let row = shell.read(cx).session(id)?;
                Some(row.read(cx).title().to_owned())
            })
            .unwrap_or_else(|| peer.to_owned())
    }

    /// The session a message was relayed from or to, when the window still lists it (an ended one
    /// is listed too): the one its name opens.
    fn peer_session(&self, agent: Option<&str>, cx: &App) -> Option<String> {
        let shell = self.shell.upgrade()?;
        let id = agent?;
        shell.read(cx).session(id).map(|_| id.to_owned())
    }

    pub fn run_drawer(&self) -> &Entity<Drawer> {
        &self.run_drawer
    }

    pub fn drawer(&self) -> &Entity<Drawer> {
        &self.drawer
    }

    pub fn conversation(&self) -> &Entity<ConversationView> {
        &self.conversation
    }

    /// The composer; `None` while a sub-agent is read: nothing writes to it.
    /// The messages that wait for the root to finish its step.
    pub fn queued(&self) -> &Entity<QueuedMessages> {
        &self.queued
    }

    pub fn reply(&self) -> Option<&Entity<Reply>> {
        (self.shown == self.opened.id).then_some(&self.reply)
    }

    /// The state the page draws an agent of its tree with.
    pub fn state(&self, agent: &str, cx: &gpui_kit::App) -> Option<AgentState> {
        self.statuses.get(agent).map(|s| s.read(cx).state())
    }

    /// The agents ending the session would end: the root and the children of its tree that did
    /// not end.
    pub fn end_count(&self) -> usize {
        self.tree
            .iter()
            .filter(|a| !matches!(a.state.as_str(), "ended" | "failed"))
            .count()
    }

    /// The session's root, once the tree is read.
    fn root(&self) -> Option<&TreeAgent> {
        self.tree.iter().find(|a| a.parent.is_none())
    }

    /// Whether the session can be ended: the daemon refuses a root in a final state.
    pub fn endable(&self) -> bool {
        self.root()
            .is_some_and(|a| !matches!(a.state.as_str(), "ended" | "failed"))
    }

    /// What the confirmation says while it is open: how many agents end.
    pub fn end_question(&self) -> Option<String> {
        if !self.ending {
            return None;
        }
        Some(end_sentence(self.end_count()))
    }

    /// Opens the confirmation of ending the session, from the root's menu.
    pub fn ask_end(&mut self, cx: &mut Context<Self>) {
        if self.endable() && !self.ending {
            self.ending = true;
            self.end_error = None;
            cx.notify();
        }
    }

    /// Closes the confirmation: Cancel, Escape or a click outside it.
    pub fn cancel_end(&mut self, cx: &mut Context<Self>) {
        if self.ending {
            self.ending = false;
            cx.notify();
        }
    }

    /// Ends the session: the root agent and its tree stop, deepest first, as the daemon does.
    pub fn confirm_end(&mut self, cx: &mut Context<Self>) {
        self.ending = false;
        let session = self.opened.id.clone();
        self.perform(Command::Stop { session }, cx);
        cx.notify();
    }

    /// What the daemon answered to an end: it says why when it refused.
    pub fn end_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.end_error = refused.map(Into::into);
        cx.notify();
    }

    pub fn end_error(&self) -> Option<&str> {
        self.end_error.as_deref()
    }

    /// What the line in place of the composer says once the session ended, and whether it offers
    /// Resume: `None` while the root runs.
    pub fn ended_line(&self) -> Option<(String, bool)> {
        let root = self.root()?;
        let reason = root
            .state_reason
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('.');
        let why = match (root.state.as_str(), reason) {
            ("ended", "stopped by the user") => "stopped by you".to_owned(),
            ("ended", "") => "stopped".to_owned(),
            ("ended", reason) => reason.to_owned(),
            ("failed", "") => "failed".to_owned(),
            ("failed", reason) => format!("failed: {reason}"),
            _ => return None,
        };
        Some((format!("This session ended: {why}."), root.resumable))
    }

    /// Whether Resume is enabled: offered, not asked already, and the root's last process gone.
    pub fn can_resume(&self) -> bool {
        !self.resuming
            && !self.root().is_some_and(|root| root.stopping)
            && self.ended_line().is_some_and(|(_, resumable)| resumable)
    }

    /// Starts the ended session's root again on its own session, once its last process is gone.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if !self.can_resume() {
            return;
        }
        self.resuming = true;
        self.resume_error = None;
        let session = self.opened.id.clone();
        self.perform(Command::Resume { session }, cx);
        cx.notify();
    }

    /// What the daemon answered to a resume: the composer comes back with the tree read again, or
    /// the line says why it was refused.
    pub fn resumed(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        // Taken: Resume stays disabled until the tree shows the root going again.
        self.resuming = refused.is_none() && self.ended_line().is_some();
        self.resume_error = refused.map(Into::into);
        cx.notify();
    }

    pub fn resume_error(&self) -> Option<&str> {
        self.resume_error.as_deref()
    }

    /// Reloads the root's tools: its process restarts on its conversation, once the turn that
    /// runs ended. Nothing happens for a session that ended, nor while one is asked.
    pub fn reload_tools(&mut self, cx: &mut Context<Self>) {
        if !self.endable() || self.reloading {
            return;
        }
        self.reloading = true;
        let session = self.opened.id.clone();
        self.perform(Command::ReloadTools { session }, cx);
        cx.notify();
    }

    /// What the daemon answered to a reload: asked between turns (a toast), after the turn (the
    /// tree says it above the composer), or refused.
    pub fn tools_reloaded(
        &mut self,
        after_turn: bool,
        refused: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.reloading = false;
        match refused {
            Some(why) => toast(Toast::failure(NOT_RELOADED).detail(why), cx),
            None if after_turn => {}
            None => toast(
                Toast::success(RELOADED)
                    .detail("The session is given the new tools at its next message."),
                cx,
            ),
        }
        cx.notify();
    }

    /// Whether the root's tools reload once its turn ends, as the daemon says, whoever asked.
    pub fn reloads_after_turn(&self) -> bool {
        self.root()
            .is_some_and(|root| root.tools_reload && root.state == "working")
    }

    pub fn set_title(&mut self, title: &str, cx: &mut Context<Self>) {
        if self.opened.title != title {
            self.opened.title = title.to_owned();
            cx.notify();
        }
    }

    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if self.layout != layout {
            self.layout = layout;
            if layout == Layout::Wide {
                self.drawer.update(cx, |drawer, cx| drawer.close(cx));
            }
            cx.notify();
        }
    }

    fn name(&self, id: &str) -> String {
        self.tree
            .iter()
            .find(|a| a.id == id)
            .map(name_of)
            .unwrap_or_else(|| match id == self.opened.id {
                true => "lead".into(),
                false => format!("agent {id}"),
            })
    }

    /// How the screen names an agent: the root is the conversation.
    fn said_name(&self, id: &str) -> String {
        match id == self.opened.id {
            true => THE_CONVERSATION.into(),
            false => self.name(id),
        }
    }

    fn parent_of(&self, id: &str) -> Option<String> {
        self.tree
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.parent.clone())
    }

    /// A new link for the shown agent: the old one stops, and what it sends late is dropped.
    fn relink(&mut self) {
        self.link = None;
        self.read = false;
        self.link = Some(session_link::spawn(
            self.follow.connect.clone(),
            self.follow.updates.clone(),
            self.opened.id.clone(),
            self.shown.clone(),
            Where {
                project: self.opened.project.clone(),
                workspace: self.opened.place.clone(),
            },
            self.pages,
        ));
        if let Some(link) = &self.link {
            link.watch(self.watching.clone());
        }
    }

    /// Opens a run's detail in its drawer, its output followed live.
    pub fn open_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.watching = Some(run.to_owned());
        if let Some(link) = &self.link {
            link.watch(Some(run.to_owned()));
        }
        let rows = self.commands(cx).read(cx).rows().to_vec();
        self.command_panel.update(cx, |panel, cx| {
            panel.open(run, cx);
            panel.set_rows(&rows, cx);
        });
        self.run_drawer.update(cx, |drawer, cx| drawer.open(cx));
    }

    /// Closes the drawer: the output is read no more.
    pub fn close_command(&mut self, cx: &mut Context<Self>) {
        self.watching = None;
        if let Some(link) = &self.link {
            link.watch(None);
        }
        self.command_panel.update(cx, |panel, cx| panel.close(cx));
        self.run_drawer.update(cx, |drawer, cx| drawer.close(cx));
    }

    /// The agent that started a run, from its drawer: its conversation is shown.
    pub fn show_agent(&mut self, agent: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.tree.iter().any(|a| a.id == agent) {
            self.select(agent, cx);
        }
    }

    /// The user stops a command: its whole group, and the agent that started it is told.
    pub fn stop_command(&mut self, run: &str, cx: &mut Context<Self>) {
        let list = self.commands(cx);
        list.update(cx, |list, cx| list.failed(None, cx));
        self.perform(
            Command::Run {
                session: self.opened.id.clone(),
                run: run.to_owned(),
                act: RunAct::Stop,
            },
            cx,
        );
    }

    /// The user runs a command again, in the same place: the daemon stops it if it runs, and
    /// the panel shows the new run once it answers.
    pub fn restart_command(&mut self, run: &str, cx: &mut Context<Self>) {
        let list = self.commands(cx);
        list.update(cx, |list, cx| list.failed(None, cx));
        self.perform(
            Command::Run {
                session: self.opened.id.clone(),
                run: run.to_owned(),
                act: RunAct::Restart,
            },
            cx,
        );
    }

    /// The user cancels a subscription: the daemon ends it, and the list follows its event.
    pub fn cancel_subscription(&mut self, subscription: &str, cx: &mut Context<Self>) {
        let list = self.subscriptions(cx);
        list.update(cx, |list, cx| list.failed(None, cx));
        self.perform(
            Command::CancelSubscription {
                session: self.opened.id.clone(),
                subscription: subscription.to_owned(),
            },
            cx,
        );
    }

    /// The user withdraws a leave: the daemon takes it back, and the list follows its event.
    pub fn withdraw_leave(&mut self, writer: &str, session: &str, cx: &mut Context<Self>) {
        self.perform(
            Command::WithdrawLeave {
                session: self.opened.id.clone(),
                writer: writer.to_owned(),
                leave_session: session.to_owned(),
            },
            cx,
        );
    }

    /// What the daemon answered to a withdrawal: why, in a toast, when it refused.
    pub fn leave_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        if let Some(why) = refused {
            toast(
                Toast::failure("The write access was not withdrawn").detail(why),
                cx,
            );
        }
    }

    /// The user resumes a paused following, as a message of theirs would: the daemon resumes it,
    /// and the list follows its event.
    pub fn resume_following(&mut self, subscription: &str, cx: &mut Context<Self>) {
        let list = self.subscriptions(cx);
        list.update(cx, |list, cx| list.failed(None, cx));
        self.perform(
            Command::ResumeFollowing {
                session: self.opened.id.clone(),
                subscription: subscription.to_owned(),
            },
            cx,
        );
    }

    /// What the daemon answered to a cancellation or a resume: why, when it refused.
    pub fn subscription_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        let list = self.subscriptions(cx);
        list.update(cx, |list, cx| list.failed(refused, cx));
    }

    /// What the daemon answered to a stop, a restart or a dismissal: why, when it refused, and the
    /// new run when it restarted one, which takes the open command's place. A refused restart is
    /// said by its toast alone.
    pub fn command_answered(&mut self, answer: RunAnswer, cx: &mut Context<Self>) {
        let RunAnswer {
            act,
            run,
            refused,
            restarted,
            notice,
        } = answer;
        let mut listed = refused.clone();
        if let (RunAct::Restart, Some(why)) = (act, &refused) {
            toast(
                Toast::failure("The command was not restarted").detail(why.clone()),
                cx,
            );
            listed = None;
            self.command_panel
                .update(cx, |panel, cx| panel.restart_refused(&run, cx));
        }
        if let Some(run) = restarted {
            self.follow_command(&run, cx);
        }
        if let Some(notice) = notice {
            toast(
                Toast::success("The command is now yours").detail(notice),
                cx,
            );
        }
        let list = self.commands(cx);
        list.update(cx, |list, cx| list.failed(listed, cx));
    }

    /// The new run of a restart replaces the open command, its row and its output staying drawn
    /// until its own arrive.
    fn follow_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.watching = Some(run.to_owned());
        if let Some(link) = &self.link {
            link.watch(Some(run.to_owned()));
        }
        let rows = self.commands(cx).read(cx).rows().to_vec();
        self.command_panel.update(cx, |panel, cx| {
            panel.follow(run, cx);
            panel.set_rows(&rows, cx);
        });
        self.run_drawer.update(cx, |drawer, cx| drawer.open(cx));
    }

    /// Reads one more page of the shown agent's conversation, before those shown: the button's
    /// and the scroll's. One page is asked at a time, and one that failed is asked again.
    pub fn show_earlier(&mut self, cx: &mut Context<Self>) {
        let Some(again) = self.conversation.update(cx, |view, cx| view.asking(cx)) else {
            return;
        };
        if !again {
            self.pages += 1;
        }
        self.relink();
        cx.notify();
    }

    /// How many pages of the shown conversation are asked for.
    pub fn pages(&self) -> usize {
        self.pages
    }

    /// The user chose an agent's conversation: the root's, with the composer, or a sub-agent's,
    /// read only. The window's history keeps the choice.
    pub fn select(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.show(agent, cx);
        cx.emit(Chose(agent.to_owned()));
    }

    /// [`SessionPage::select`], for the window's history going back to a place: it does not
    /// record it again.
    pub fn show(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.drawer.update(cx, |drawer, cx| drawer.close(cx));
        // An agent that left the tree has no conversation to show: the session's is. Before the
        // tree is read nothing is known, and `set_tree` looks again.
        let known = self.tree.is_empty() || self.tree.iter().any(|a| a.id == agent);
        let agent = match known {
            true => agent.to_owned(),
            false => self.opened.id.clone(),
        };
        self.unchecked = self.tree.is_empty() && agent != self.opened.id;
        if self.shown == agent {
            return;
        }
        let agent = agent.as_str();
        self.shown = agent.to_owned();
        self.pages = 1;
        let parent = self.parent_of(agent).map(|p| self.said_name(&p));
        let (page, name) = (cx.entity().downgrade(), self.said_name(agent));
        let shown = self.shown.clone();
        self.conversation = cx.new(|cx| ConversationView::new(page, &shown, name, parent, cx));
        self.rail
            .update(cx, |rail, cx| rail.set_selected(agent.to_owned(), cx));
        self.sync_pending(cx);
        self.relink();
        self.refresh_conversation(cx);
        cx.notify();
    }

    /// Opens the place in the user's editor, with the system's default handler.
    pub fn open_in_editor(&self) {
        if let Some(folder) = &self.opened.folder {
            (self.follow.opener)(folder);
        }
    }

    /// Opens a link (a pull request, a ticket) with the system's default handler.
    pub fn open_link(&self, url: &str) {
        (self.follow.opener)(std::path::Path::new(url));
    }

    /// Asks the daemon, off the UI thread.
    fn perform(&self, command: Command, cx: &mut Context<Self>) {
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
    }

    /// What the daemon answered to a ticket linked from the rail: the rail says why when it was
    /// refused.
    pub fn ticket_linked(
        &mut self,
        project: &str,
        name: &str,
        refused: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opened.project == project && self.opened.place.as_deref() == Some(name) {
            self.rail
                .update(cx, |rail, cx| rail.ticket_answered(refused, window, cx));
        }
    }

    /// What the page's link read or saw; what an older link sends is not for it.
    pub fn apply(&mut self, token: u64, update: SessionUpdate, cx: &mut Context<Self>) {
        if self.token() != Some(token) {
            return;
        }
        match update {
            SessionUpdate::Tree(tree) => self.set_tree(tree, cx),
            SessionUpdate::History { events, earlier } => {
                self.histories += 1;
                self.read = true;
                for event in &events {
                    self.note_decision(event);
                }
                let root = self.shown == self.opened.id;
                // The requests another agent of the tree asked are not in this one's events:
                // each is placed at the time it was asked.
                let placed = match root {
                    true => self
                        .requests
                        .values()
                        .map(|r| (r.created_at, r.number))
                        .collect(),
                    false => Vec::new(),
                };
                let launched_before = earlier && self.has_children();
                let follow = self.pages == 1;
                let (conversation, this) = (self.conversation.clone(), &*self);
                conversation.update(cx, |view, cx| {
                    let read = Read {
                        events: &events,
                        earlier,
                        placed,
                        launched_before,
                        follow,
                    };
                    view.rebuild(read, this, cx)
                });
                self.refresh_conversation(cx);
                self.sync_command_cards(cx);
                self.note_activity(cx);
                self.fetch_pictures(cx);
            }
            SessionUpdate::HistoryFailed(why) => {
                // The first read is the skeleton's; only a page asked for is told.
                if self.pages > 1 {
                    self.conversation
                        .update(cx, |view, cx| view.load_failed(why, cx));
                }
            }
            // One row added or drawn again: nothing else is looked at.
            SessionUpdate::Live(event) => {
                self.note_decision(&event);
                if self.shown == self.opened.id {
                    self.sending_left(&event, cx);
                }
                let (conversation, this) = (self.conversation.clone(), &*self);
                conversation.update(cx, |view, cx| {
                    let changes = view.model.apply(&event);
                    view.changed(&changes, this, cx);
                });
                if event.name == "agent.tool_call" {
                    self.sync_command_cards(cx);
                }
                self.note_activity(cx);
                if event.name == "agent.prompted" {
                    self.fetch_pictures(cx);
                }
            }
            SessionUpdate::Requests(requests) => {
                self.requests = requests.into_iter().map(|r| (r.number, r)).collect();
                // The rail's cards do not wait for the conversation to be read.
                self.refresh_conversation(cx);
            }
            SessionUpdate::Decided { number, decision } => {
                self.decisions.insert(number, decision);
                self.refresh_conversation(cx);
            }
            SessionUpdate::Place(facts) => {
                self.rail
                    .update(cx, |rail, cx| rail.set_place(facts.map(|facts| *facts), cx));
            }
            SessionUpdate::PlaceForge(forge) => {
                self.rail.update(cx, |rail, cx| rail.set_forge(forge, cx));
            }
            SessionUpdate::PlaceTicket(ticket) => {
                self.rail.update(cx, |rail, cx| rail.set_ticket(ticket, cx));
            }
            SessionUpdate::Commands(rows) => {
                self.command_panel
                    .update(cx, |panel, cx| panel.set_rows(&rows, cx));
                let list = self.commands(cx);
                list.update(cx, |list, cx| list.set(rows, cx));
                self.sync_command_cards(cx);
            }
            SessionUpdate::Subscriptions(rows) => {
                let list = self.subscriptions(cx);
                list.update(cx, |list, cx| list.set(rows, cx));
            }
            SessionUpdate::Leaves(rows) => {
                let list = self.leaves(cx);
                list.update(cx, |list, cx| list.set(rows, cx));
            }
            SessionUpdate::CommandOutput(output) => {
                self.command_panel
                    .update(cx, |panel, cx| panel.set(*output, cx));
            }
            SessionUpdate::OptionsRefused(reason) => {
                let options = self.reply.read(cx).options.clone();
                options.update(cx, |options, cx| options.refused(reason, cx));
            }
        }
    }

    /// The user answers a request from its card in the rail: Allow or Deny. The daemon's answer
    /// comes back to [`SessionPage::answered`].
    pub fn answer(&mut self, number: u64, answer: Answer, cx: &mut Context<Self>) {
        let command = Command::Answer {
            session: self.opened.id.clone(),
            number,
            answer,
        };
        self.perform(command, cx);
    }

    /// What the daemon answered to an answer given on a card.
    pub fn answered(
        &mut self,
        number: u64,
        decision: Option<String>,
        refused: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(decision) = decision {
            self.decisions.insert(number, decision);
        }
        let waiting = self.waiting(cx);
        waiting.update(cx, |waiting, cx| waiting.answered(number, refused, cx));
        self.refresh_conversation(cx);
    }

    /// The conversation's line about a request was clicked: the rail shows its card, at its top.
    pub fn reveal_waiting(&mut self, number: u64, cx: &mut Context<Self>) {
        if self.layout != Layout::Wide {
            self.drawer.update(cx, |drawer, cx| drawer.open(cx));
        }
        self.rail.update(cx, |rail, cx| rail.reveal(number, cx));
    }

    /// A decision on a request, read from the shown agent's events.
    fn note_decision(&mut self, event: &flottille_core::protocol::Event) {
        let decision = match event.name.as_str() {
            "approval.decided" => event.data["decision"].as_str(),
            "approval.cancelled" => Some("cancelled"),
            _ => None,
        };
        if let (Some(decision), Some(number)) = (decision, event.data["number"].as_u64()) {
            self.decisions.insert(number, decision.to_owned());
        }
    }

    fn has_children(&self) -> bool {
        self.tree
            .iter()
            .any(|a| a.parent.as_deref() == Some(&self.shown))
    }

    /// What the root does, for the line above the composer: read from its conversation, which is
    /// the shown one only while the root is shown.
    fn note_activity(&self, cx: &mut Context<Self>) {
        if self.shown != self.opened.id {
            return;
        }
        let activity = self.conversation.read(cx).model.activity();
        self.reply
            .update(cx, |reply, cx| reply.set_activity(Some(activity), cx));
    }

    /// Asks the daemon for the pictures the shown conversation names and has not read yet.
    fn fetch_pictures(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<String> = self
            .conversation
            .read(cx)
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::Message { images, .. } => Some(images.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        // The pictures of a message that waits are the root's, shown while the root is.
        let queued = self.queued.read(cx);
        let waiting: Vec<String> = match self.shown == queued.session() {
            true => queued
                .messages()
                .iter()
                .flat_map(|m| m.pictures.clone())
                .collect(),
            false => Vec::new(),
        };
        let wanted: Vec<String> = wanted
            .into_iter()
            .chain(waiting)
            .filter(|name| !self.asked_pictures.contains(name))
            .collect();
        for name in wanted {
            self.asked_pictures.insert(name.clone());
            let command = Command::Image {
                session: self.shown.clone(),
                name,
            };
            // The shell is handing this page its updates: it asks once they are over.
            crate::commands::perform_later(&self.shell, command, cx);
        }
    }

    /// A picture read back from the daemon: the rows that show it draw it.
    pub fn picture_read(&mut self, name: String, image: Option<Picture>, cx: &mut Context<Self>) {
        let Some(image) = image else {
            return;
        };
        let Some(format) = gpui_kit::ImageFormat::from_mime_type(&image.mime) else {
            return;
        };
        let picture = Arc::new(gpui_kit::Image::from_bytes(format, image.bytes));
        self.pictures.insert(name.clone(), picture.clone());
        self.queued
            .update(cx, |queued, cx| queued.picture_read(&name, picture, cx));
        let rows = self.conversation.read(cx).rows.clone();
        for row in rows {
            if matches!(&*row.read(cx).block, Block::Message { images, .. } if images.contains(&name))
            {
                row.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// How many pictures of the shown conversation are read.
    pub fn pictures_read(&self) -> usize {
        self.pictures.len()
    }

    fn set_tree(&mut self, tree: Vec<TreeAgent>, cx: &mut Context<Self>) {
        for agent in &tree {
            let state = state_of(agent);
            match self.statuses.get(&agent.id) {
                Some(status) => {
                    if status.read(cx).state() != state {
                        status.update(cx, |status, cx| status.set(state, cx));
                    }
                }
                None => {
                    let status = cx.new(|_| Status::new(state));
                    self.statuses.insert(agent.id.clone(), status);
                }
            }
        }
        if let Some(root) = tree.iter().find(|a| a.parent.is_none()) {
            let runs = Runs {
                provider: root.provider.clone(),
                model: root.model.clone(),
                effort: root.effort.clone(),
                pending: root.pending.clone(),
                working: root.state == "working",
            };
            let options = self.reply.read(cx).options.clone();
            options.update(cx, |options, cx| options.set_runs(runs, cx));
            let mode = self.reply.read(cx).mode.clone();
            let gate_mode = root.gate_mode.clone();
            mode.update(cx, |mode, cx| mode.set_mode(gate_mode, cx));
            let running = root.state == "working";
            let images = root.images;
            let queued = root.queued.clone();
            self.queued.update(cx, |list, cx| list.set(queued, cx));
            self.sync_pending(cx);
            self.fetch_pictures(cx);
            self.reply.update(cx, |reply, cx| {
                reply.set_images(images);
                reply.set_running(running, cx)
            });
        }
        let names: HashMap<String, String> =
            tree.iter().map(|a| (a.id.clone(), name_of(a))).collect();
        let rows: Vec<RailAgent> = tree
            .iter()
            .filter_map(|agent| {
                let status = self.statuses.get(&agent.id)?.clone();
                Some(RailAgent {
                    id: agent.id.clone(),
                    name: name_of(agent),
                    provider: agent.provider.clone(),
                    model: agent.model.clone(),
                    runs_on: runs_on(&self.providers, &agent.provider, agent.model.as_deref()),
                    depth: agent.depth,
                    parent: agent.parent.clone(),
                    ended: matches!(state_of(agent), AgentState::Completed | AgentState::Stopped),
                    context: agent.context,
                    context_tokens: agent.context_tokens,
                    doing: agent.doing.clone(),
                    waits_on_subscription: agent.waiting_on.as_deref() == Some("subscription"),
                    status,
                })
            })
            .collect();
        self.rail.update(cx, |rail, cx| rail.set_agents(rows, cx));
        // The runs and their drawer say who started each as the rail does: the root is the
        // conversation.
        let said: HashMap<String, String> = tree
            .iter()
            .map(|a| match a.parent {
                None => (a.id.clone(), THE_CONVERSATION.to_owned()),
                Some(_) => (a.id.clone(), name_of(a)),
            })
            .collect();
        let list = self.commands(cx);
        list.update(cx, |list, cx| list.set_names(said.clone(), cx));
        let session = Some(self.opened.title.clone());
        self.command_panel
            .update(cx, |panel, cx| panel.set_names(said, session, cx));
        let changed = self.tree != tree;
        let ended = |tree: &[TreeAgent]| {
            tree.iter()
                .find(|a| a.parent.is_none())
                .map(|root| matches!(root.state.as_str(), "ended" | "failed"))
        };
        let (was_ended, now_ended) = (ended(&self.tree), ended(&tree));
        let root = |tree: &[TreeAgent]| older_tools(tree.iter().find(|a| a.parent.is_none()));
        let older_changed = root(&self.tree) != root(&tree);
        let questions = |tree: &[TreeAgent]| {
            tree.iter()
                .find(|a| a.parent.is_none())
                .map(|root| root.questions.clone())
        };
        let asked_changed = questions(&self.tree) != questions(&tree);
        let was_waiting = self.reloads_after_turn();
        self.tree = tree;
        // The line of a reload that waits for the turn comes and goes with the daemon's word.
        if older_changed || was_waiting != self.reloads_after_turn() {
            cx.notify();
        }
        if let Some(now_ended) = now_ended.filter(|now| was_ended != Some(*now)) {
            if was_ended.is_some() {
                // Ended or resumed: what was said before it is over.
                self.resume_error = None;
            }
            if !now_ended {
                self.resuming = false;
            }
            cx.notify();
        }
        if std::mem::take(&mut self.unchecked) && !self.tree.iter().any(|a| a.id == self.shown) {
            let root = self.opened.id.clone();
            self.show(&root, cx);
        }
        if changed {
            let conversation = self.conversation.clone();
            conversation.update(cx, |view, _| view.model.set_names(names));
            self.refresh_conversation(cx);
        } else if asked_changed {
            self.refresh_conversation(cx);
        }
    }

    /// The user answers the question of `seq` from its card: a message to the root, which
    /// names the question when several wait.
    pub fn answer_question(&mut self, seq: u64, answer: String, cx: &mut Context<Self>) {
        let open = self
            .tree
            .iter()
            .find(|a| a.parent.is_none())
            .map(|root| root.questions.clone())
            .unwrap_or_default();
        // A message of the user answers every question open: it quotes each when several wait.
        let message = match open.len() > 1 {
            true => {
                let quoted: Vec<String> = open
                    .iter()
                    .map(|q| format!("> {}", q.text.lines().next().unwrap_or_default()))
                    .collect();
                format!("{}\n\n{answer}", quoted.join("\n>\n"))
            }
            false => answer,
        };
        let mut seqs: Vec<u64> = open.iter().map(|q| q.seq).collect();
        if !seqs.contains(&seq) {
            seqs.push(seq);
        }
        let command = Command::AnswerQuestion {
            session: self.opened.id.clone(),
            seq,
            seqs,
            message,
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
    }

    /// What the daemon answered to the answer of the question of `seq`.
    pub fn question_answered(
        &mut self,
        seq: u64,
        refused: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.conversation.read(cx).rows.clone();
        let answer = rows.iter().find_map(|row| {
            let row = row.read(cx);
            matches!(*row.block, Block::Question { seq: s, .. } if s == seq)
                .then(|| row.answer.clone())
                .flatten()
        });
        if let Some(answer) = answer {
            answer.update(cx, |answer, cx| answer.answered(refused, window, cx));
        }
    }

    /// How many questions of the root wait for the user's answer.
    fn open_questions(&self) -> usize {
        self.tree
            .iter()
            .find(|a| a.parent.is_none())
            .map_or(0, |root| root.questions.len())
    }

    /// Whether the question of `seq` waits for the user's answer, as the daemon says it.
    fn asking(&self, seq: u64) -> bool {
        self.tree
            .iter()
            .find(|a| a.parent.is_none())
            .is_some_and(|root| root.questions.iter().any(|q| q.seq == seq))
    }

    /// Brings the cards of running commands up to the rail's list, which is read each second while
    /// one runs: their time, the end of their output, and how a command that ended meanwhile did.
    fn sync_command_cards(&self, cx: &mut Context<Self>) {
        let rows = self.commands(cx).read(cx).rows().to_vec();
        let conversation = self.conversation.clone();
        conversation.update(cx, |view, cx| {
            let changes = view.model.update_commands(&rows, crate::shell::now_ms());
            view.changed(&changes, self, cx);
        });
    }

    /// Brings the rows that show the tree or the requests up to what the page knows: the
    /// sub-agents list, the requests' cards and their records. Only those rows are looked at.
    fn refresh_conversation(&self, cx: &mut Context<Self>) {
        self.sync_waiting(cx);
        if !self.read {
            return;
        }
        let conversation = self.conversation.clone();
        let has_children = self.has_children();
        let root = self.shown == self.opened.id;
        let mut numbers: Vec<u64> = self.requests.keys().copied().collect();
        numbers.sort();
        conversation.update(cx, |view, cx| {
            if has_children {
                let changes = view.model.add_sub_agents();
                view.changed(&changes, self, cx);
            }
            if root {
                for number in numbers {
                    if !view.model.has_approval(number) {
                        let changes = view.model.add_approval(number);
                        view.changed(&changes, self, cx);
                    }
                }
            }
            for at in view.specials.clone() {
                let row = view.rows[at].clone();
                view.touched += 1;
                if let Block::Question { seq, .. } = *row.read(cx).block {
                    if let Some(answer) = row.read(cx).answer.clone() {
                        let open = self.open_questions();
                        answer.update(cx, |answer, cx| answer.set_open(open, cx));
                    }
                    let asking = self.asking(seq);
                    if row.read(cx).asking != asking {
                        row.update(cx, |row, cx| row.set_asking(asking, cx));
                        view.card_changed(at, cx);
                    }
                    continue;
                }
                let kind = match *row.read(cx).block {
                    Block::Approval { number } => Some(number),
                    _ => None,
                };
                match kind {
                    None => {
                        let lines = self.sub_agents();
                        if row.read(cx).sub_agents != lines {
                            row.update(cx, |row, cx| {
                                row.sub_agents = lines;
                                cx.notify();
                            });
                        }
                    }
                    Some(number) => {
                        let card = self.card(number);
                        if row.read(cx).card != card {
                            row.update(cx, |row, cx| {
                                row.card = card;
                                cx.notify();
                            });
                            view.card_changed(at, cx);
                        }
                    }
                }
            }
        });
    }

    /// Brings the rail's "Waiting for you" up to the requests that wait for the user, in the order
    /// they were asked.
    fn sync_waiting(&self, cx: &mut Context<Self>) {
        let mut numbers: Vec<u64> = self.requests.keys().copied().collect();
        numbers.sort();
        let cards: Vec<ApprovalCard> = numbers
            .into_iter()
            .filter_map(|number| self.card(number))
            .filter(|card| card.decision.is_none())
            .collect();
        let waiting = self.waiting(cx);
        waiting.update(cx, |waiting, cx| waiting.set(cards, cx));
    }

    /// The children of the shown agent, as its sub-agents list shows them.
    fn sub_agents(&self) -> Vec<SubAgent> {
        self.tree
            .iter()
            .filter(|a| a.parent.as_deref() == Some(&self.shown))
            .filter_map(|agent| {
                Some(SubAgent {
                    id: agent.id.clone(),
                    name: name_of(agent),
                    provider: agent.provider.clone(),
                    model: agent.model.clone(),
                    runs_on: runs_on(&self.providers, &agent.provider, agent.model.as_deref()),
                    doing: agent.doing.clone(),
                    status: self.statuses.get(&agent.id)?.clone(),
                    asks: self
                        .requests
                        .values()
                        .any(|r| r.agent == agent.id && self.decision(r.number).is_none()),
                })
            })
            .collect()
    }

    fn card(&self, number: u64) -> Option<ApprovalCard> {
        let request = self.requests.get(&number);
        let decision = self.decision(number).map(str::to_owned);
        if request.is_none() && decision.is_none() {
            return None;
        }
        Some(ApprovalCard {
            number,
            base: request.is_some_and(|r| r.base),
            judgement: request
                .and_then(|r| r.judgement.as_ref())
                .map(|(by, verdict, reason)| match verdict.as_str() {
                    "failed" => format!("The judge ({by}) could not decide: {reason}"),
                    _ => format!("The judge ({by}) asks you: {reason}"),
                }),
            kind: request.map_or_else(|| "call".to_owned(), |r| r.kind.clone()),
            says: request.map(|r| r.says.clone()).unwrap_or_default(),
            asked: request.map(Request::asked),
            call: request.map(Request::call_line),
            why: request.map(|r| {
                let forwards: Vec<String> = r
                    .forwards
                    .iter()
                    .map(|(by, said)| format!("{} forwards it: {said}", self.said_name(by)))
                    .collect();
                sentences(
                    r.reasons
                        .iter()
                        .map(String::as_str)
                        .chain(forwards.iter().map(String::as_str)),
                )
            }),
            source: request.map(|r| {
                let mut source = format!("asked by {}", self.said_name(&r.agent));
                for (by, _) in &r.forwards {
                    source.push_str(&format!(" · forwarded by {}", self.said_name(by)));
                }
                format!("{source} · #{number}")
            }),
            asker: request.map(|r| self.said_name(&r.agent)),
            decision,
        })
    }
}

impl Render for SessionPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let narrow = self.layout == Layout::Narrow;
        let wide = self.layout == Layout::Wide;
        let drawer = self.drawer.clone();
        let place = self.opened.place.clone();
        let header = div()
            .id("session-header")
            .flex_none()
            .h(px(44.))
            .pl(if narrow { space::MD } else { space::XL })
            .pr(space::MD)
            .flex()
            .items_center()
            .gap(space::MD)
            .border_b_1()
            .border_color(p.line)
            .child(tipped_when_cut(
                div()
                    .id("session-title")
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(14.))
                    .semibold()
                    .aria_label(SharedString::from(self.opened.title.clone()))
                    .child(SharedString::from(self.opened.title.clone())),
                SharedString::from(self.opened.title.clone()),
                window,
                cx,
            ))
            .child(
                div()
                    .id("session-place")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .text_color(p.faint)
                    .aria_label(SharedString::from(place_name(&place).to_owned()))
                    .child(
                        icon(match place {
                            None => Icon::Folder,
                            Some(_) => Icon::Branch,
                        })
                        .size(px(12.))
                        .text_color(p.faint),
                    )
                    .child(
                        div()
                            .mono(12.)
                            .child(SharedString::from(place_name(&place).to_owned())),
                    )
                    .test_support(),
            )
            .children(self.opened.started_by.clone().map(|by| {
                let said = SharedString::from(format!("started by {by}"));
                tipped_when_cut(
                    div()
                        .id("session-started-by")
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .aria_label(said.clone())
                        .child(said.clone()),
                    said,
                    window,
                    cx,
                )
            }))
            .child(div().flex_1())
            .children(self.end_error.clone().map(|error| {
                div()
                    .id("session-end-error")
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(error)
                    .test_support()
            }))
            .when(!wide, |d| {
                let mut agents = Button::icon("open-rail", Icon::Agents, "Agents")
                    .on_click(move |_, _, cx| drawer.update(cx, |d, cx| d.open(cx)));
                if self.needs_you {
                    agents = agents.badge(Badge::alert());
                }
                d.child(agents)
            })
            .test_support();
        let root = self.shown == self.opened.id;
        let older = older_tools(self.root().filter(|_| root)).map(SharedString::from);
        let after_turn = (root && self.reloads_after_turn()).then_some(RELOADS_AFTER_TURN.into());
        let reloader = cx.entity().downgrade();
        let reloading = self.reloading;
        let dock = div()
            .flex_none()
            .w_full()
            .max_w(px(780.))
            .mx_auto()
            .px(if narrow { space::MD } else { space::XXL })
            .pb(if narrow { space::MD } else { space::XL })
            // Built on every frame: the line unfolds when the session falls behind its role's
            // tools and folds away once it lists them again or restarts.
            .child(presence_of(
                "session-older-tools-line",
                older,
                move |said| {
                    div()
                        .flex()
                        .items_center()
                        .gap(space::MD)
                        .pb(space::SM)
                        .child(
                            div()
                                .id("session-older-tools")
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.5))
                                .text_color(p.faint)
                                .aria_label(said.clone())
                                .child(said)
                                .test_support(),
                        )
                        .child(
                            Button::secondary("session-reload-tools", RELOAD_TOOLS)
                                .disabled(reloading)
                                .on_click(move |_, _, cx| {
                                    reloader.update(cx, |page, cx| page.reload_tools(cx)).ok();
                                }),
                        )
                        .into_any_element()
                },
            ))
            // While a reload waits for the turn to end; it folds away once it ended.
            .child(presence_of(
                "session-reload-after-turn-line",
                after_turn,
                move |said| {
                    div()
                        .id("session-reload-after-turn")
                        .pb(space::SM)
                        .text_size(px(12.5))
                        .text_color(p.faint)
                        .aria_label(said.clone())
                        .child(said)
                        .test_support()
                        .into_any_element()
                },
            ))
            .map(|d| match (root, self.ended_line()) {
                (true, None) => d.child(self.queued.clone()).child(self.reply.clone()),
                (true, Some(ended)) => d.child(self.ended_bar(ended, cx)),
                (false, _) => d.child(self.read_only(cx)),
            });
        let column = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().child(self.conversation.clone()))
            .child(dock);
        div()
            .id("session-page")
            .size_full()
            .relative()
            .child(div().size_full().flex().child(column).when(wide, |d| {
                d.child(
                    div()
                        .id("session-rail")
                        .flex_none()
                        .w(px(RAIL))
                        .h_full()
                        .bg(p.side)
                        .border_l_1()
                        .border_color(p.line)
                        .child(self.rail.clone())
                        .test_support(),
                )
            }))
            .when(!wide, |d| d.child(self.drawer.clone()))
            .child(self.run_drawer.clone())
            .child(self.end_modal(cx))
    }
}

impl SessionPage {
    /// The confirmation of ending the session: it names how many agents end.
    fn end_modal(&self, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let count = self.end_count();
        let this = cx.entity().downgrade();
        Modal::new("session-end-modal", "End the session?")
            .open(self.ending)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.cancel_end(cx)).ok();
            })
            .child(
                div()
                    .id("session-end-question")
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(
                        self.end_question()
                            .unwrap_or_else(|| format!("{count} to end")),
                    ))
                    .test_support(),
            )
            .action(
                Button::danger(
                    "session-end-confirm",
                    match count {
                        0 | 1 => "End session".to_owned(),
                        n => format!("End {n} agents"),
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.confirm_end(cx))),
            )
    }

    /// In place of the composer once the session ended: why, and Resume when the daemon can
    /// start the root again on its own session.
    fn ended_bar(&self, (said, resumable): (String, bool), cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(
                div()
                    .id("session-ended")
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .border_1()
                    .border_dashed()
                    .border_color(p.line2)
                    .rounded(px(12.))
                    .pl(space::LG)
                    .pr(space::MD)
                    .py(space::MD)
                    .text_size(px(13.))
                    .text_color(p.mute)
                    .aria_label(said.clone())
                    .child(div().flex_1().min_w_0().child(SharedString::from(said)))
                    .when(resumable, |d| {
                        d.child(
                            Button::quiet("session-resume", "Resume")
                                .disabled(!self.can_resume())
                                .on_click(cx.listener(|this, _, _, cx| this.resume(cx))),
                        )
                    })
                    .test_support(),
            )
            .children(self.resume_error.clone().map(|error| {
                div()
                    .id("session-resume-error")
                    .px(space::MD)
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(error)
                    .test_support()
            }))
    }

    /// What the bar in place of the composer says while a sub-agent is read, and its button:
    /// `None` while the root is shown.
    pub fn read_only_text(&self) -> Option<(String, String)> {
        if self.shown == self.opened.id {
            return None;
        }
        Some((
            format!(
                "Reading {}. It belongs to {THE_CONVERSATION}: only {THE_CONVERSATION} takes \
                 your messages.",
                self.name(&self.shown)
            ),
            format!("Back to {THE_CONVERSATION}"),
        ))
    }

    /// In place of the composer while a sub-agent is read: it belongs to the conversation.
    fn read_only(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (said, back) = self.read_only_text().unwrap_or_default();
        let root = self.opened.id.clone();
        div()
            .id("read-only")
            .flex()
            .items_center()
            .gap(space::MD)
            .border_1()
            .border_dashed()
            .border_color(p.line2)
            .rounded(px(12.))
            .pl(space::LG)
            .pr(space::MD)
            .py(space::MD)
            .text_size(px(13.))
            .text_color(p.mute)
            .child(div().flex_1().min_w_0().child(SharedString::from(said)))
            .child(Button::quiet("back-to-root", back).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.select(&root, cx);
                },
            )))
            .test_support()
    }
}

/// What a link read of the shown agent's conversation, to make its rows again.
struct Read<'a> {
    events: &'a [flottille_core::protocol::Event],
    /// Older events were left out.
    earlier: bool,
    /// The requests that reached the user, each `(when it was asked, its number)`.
    placed: Vec<(u64, u64)>,
    /// The shown agent has children, launched before the events read.
    launched_before: bool,
    /// Its end is shown.
    follow: bool,
}

/// The conversation: one row view per block, in a list that lays out and draws only the items in
/// view, and follows its end unless the user scrolled up to read. What the agent said, the
/// messages it was sent and the children it launched are items of their own; everything between
/// two of them is one group, folded to a line by default: the line is one item, and each of its
/// rows is one more while it is unfolded, so that a long group draws only the rows in view.
pub struct ConversationView {
    page: WeakEntity<SessionPage>,
    id: String,
    model: Conversation,
    rows: Vec<Entity<Row>>,
    /// The rows that show what the tree or the requests say (the sub-agents list, the requests),
    /// brought up to date when those change.
    specials: Vec<usize>,
    /// Older events than those read remain.
    earlier: bool,
    /// Whether a page of them is asked for, and why one failed.
    load: Load,
    /// The list's first item still stands while the line it held folds away, once nothing
    /// earlier remains.
    head_leaving: bool,
    /// Takes the head out of the list, and the spinner with it, once the line has folded.
    head_sweep: Option<Task<()>>,
    /// The spinner of the loading line: made when a page is asked for, as it turns by itself.
    spinner: Option<Entity<StateIcon>>,
    /// How many times a row was looked at or changed, for the cost of an update.
    touched: usize,
    /// How many rows of a group were walked to say its line or what stays out of its fold: only
    /// when the group changes, never to draw it.
    walked: usize,
    /// The list's items: "Show earlier" first when older events remain, then the segments, a
    /// long reply as one item per Markdown piece: the list draws only the pieces in view.
    list: ListState,
    /// The items of each row past its first: the pieces of its reply past the first.
    more: Vec<usize>,
    /// The rows in sight and the groups, in order: one per stretch of the list.
    segments: Vec<Segment>,
    /// The segment of each row.
    segment_of: Vec<usize>,
    /// The button back to the end, over the list.
    to_end: Entity<ToEnd>,
    /// Takes the rows that folded away out of the list once their fold has ended.
    sweep: Option<Task<()>>,
    /// The messages the user sent and the daemon has not recorded yet, after the rows: one item
    /// of the list each.
    pending: Vec<SentRow>,
    /// The rows that were a message being sent: they are drawn in its place, never entering.
    in_place: HashMap<usize, u64>,
    /// The user's messages recorded while one sent from here waits for the daemon's answer: not
    /// drawn until it says which is that one, which then becomes it in place.
    held: HashSet<usize>,
    renders: usize,
}

/// Where the page of earlier events stands.
#[derive(Clone, Debug, PartialEq)]
enum Load {
    Idle,
    Asked,
    /// It did not come, with why.
    Failed(String),
}

/// What the user was reading when a page of earlier events arrived: the block at the top of the
/// list, found again by the number of its event, and how far in it the top was.
struct Anchor {
    seq: u64,
    /// The sub-agents' card, which stands first until the event that launched them is read.
    card: bool,
    kind: Reading,
    /// How far into its item the top of the list was.
    offset: Pixels,
}

/// Which item of a block was at the top.
#[derive(Clone, Copy)]
enum Reading {
    /// A group's line.
    Line,
    /// A row, or one of the pieces of a long reply.
    Piece(usize),
}

/// A message being sent, as an item of the list: it enters as any message, and folds away when
/// it is taken back.
struct SentRow {
    key: u64,
    row: Entity<Row>,
    motion: Motion,
    height: Rc<Cell<Pixels>>,
}

/// A stretch of the conversation: a row in sight, or a group of the agent's work.
enum Segment {
    Row(usize),
    Group(Group),
}

/// A group of the agent's work. What its line says and which of its rows stay out of the fold
/// are kept with it, so that a frame never walks the rows it holds.
struct Group {
    rows: Range<usize>,
    summary: Summary,
    /// The rows that stay in sight while it is folded: a call held for the user, a command still
    /// running, a question asked to the user.
    needs: Vec<usize>,
    /// Older events of it are on an earlier page: its line counts only the part read.
    cut: bool,
    open: bool,
    /// Its rows that are items of the list now, in order: unfolding, shown, or folding away
    /// until the sweep takes them out.
    items: Vec<Shown>,
}

/// A row of a group that is an item of the list, with its fold.
struct Shown {
    at: usize,
    motion: Motion,
    height: Rc<Cell<Pixels>>,
}

/// What a group's fold is kept by, the same whatever page of the history was read first: the
/// event that made its first block or, while older events of it are on an earlier page, the one
/// that made the block after it (`Cut` when none came yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    First(u64),
    Before(u64),
    Cut,
}

/// The space between two items of the conversation, kept between the rows of a group.
fn row_gap() -> Pixels {
    px(18.)
}

/// The groups the user unfolded, by agent: kept while the window is open, never stored.
#[derive(Default)]
struct Unfolded(HashMap<String, HashSet<Key>>);

impl gpui_kit::Global for Unfolded {}

impl ConversationView {
    fn new(
        page: WeakEntity<SessionPage>,
        id: &str,
        me: String,
        parent: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = Self::list();
        let to_end = cx.new(|_| ToEnd::new(list.clone()));
        let this = Self {
            page,
            id: id.to_owned(),
            model: Conversation::new(me, parent),
            rows: Vec::new(),
            specials: Vec::new(),
            earlier: false,
            load: Load::Idle,
            head_leaving: false,
            head_sweep: None,
            spinner: None,
            touched: 0,
            walked: 0,
            list,
            more: Vec::new(),
            segments: Vec::new(),
            segment_of: Vec::new(),
            to_end,
            sweep: None,
            pending: Vec::new(),
            in_place: HashMap::new(),
            held: HashSet::new(),
            renders: 0,
        };
        this.watch(cx);
        this
    }

    /// The list's first item after the rows: the first message being sent.
    fn pending_item(&self) -> usize {
        self.item_of(self.segments.len())
    }

    /// Follows the messages being sent: the ones drawn in the conversation (not `queued`) enter
    /// at the end, fold away when gone, say a refusal on their own. One whose record is here
    /// already is that record: in its place when the record was held for it.
    fn sync_pending(&mut self, all: &[Pending], page: &SessionPage, cx: &mut Context<Self>) {
        let root = self.id == page.id();
        let mut recorded = Vec::new();
        for pending in all.iter().filter(|_| root) {
            let Some(accepted) = pending
                .accepted
                .as_ref()
                .filter(|_| pending.refused.is_none())
            else {
                continue;
            };
            let at = (0..self.rows.len())
                .rev()
                .find(|&at| self.model.delivery_of(at) == Some(accepted.delivery.as_str()));
            if let Some(at) = at {
                recorded.push((pending.key, at));
            }
        }
        for &(key, at) in &recorded {
            page.reply.update(cx, |reply, cx| reply.resolve(key, cx));
            let Some(n) = self.pending.iter().position(|shown| shown.key == key) else {
                continue;
            };
            let ix = self.pending_item() + n;
            self.list.splice(ix..ix + 1, 0);
            let shown = self.pending.remove(n);
            if self.held.remove(&at) {
                let block = self.model.blocks()[at].clone();
                let when = self.model.at(at);
                shown
                    .row
                    .update(cx, |row, cx| row.recorded(at, block, when, cx));
                self.rows[at] = shown.row;
                self.in_place.insert(at, key);
                let item = self.item(at);
                self.list.remeasure_items(item..item + 1);
                self.fit(at, cx);
            }
        }
        let sent: Vec<&Pending> = all
            .iter()
            .filter(|p| root && !p.queued && !recorded.iter().any(|(key, _)| *key == p.key))
            .collect();
        // Nothing sent from here waits for its answer: the records held are someone else's.
        if !sent.iter().any(|p| p.in_flight()) {
            for at in std::mem::take(&mut self.held) {
                let item = self.item(at);
                self.list.remeasure_items(item..item + 1);
            }
        }
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let mut leaving = false;
        for shown in &mut self.pending {
            match sent.iter().find(|p| p.key == shown.key) {
                Some(pending) => {
                    let refused = pending.refused.clone();
                    shown.row.update(cx, |row, cx| row.set_refused(refused, cx));
                }
                None if shown.motion.target() == 1.0 => {
                    shown.motion.animate(0.0, PANEL, now, reduce);
                    leaving = true;
                }
                None => {}
            }
        }
        for pending in sent {
            if self.pending.iter().any(|shown| shown.key == pending.key) {
                continue;
            }
            let row = Row::sending(pending, root, page, self.page.clone(), cx);
            let ix = self.pending_item() + self.pending.len();
            self.list.splice(ix..ix, 1);
            self.pending.push(SentRow {
                key: pending.key,
                row,
                motion: Motion::settled(1.0),
                height: Rc::default(),
            });
        }
        if leaving {
            self.sweep = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PANEL + FRAME).await;
                this.update(cx, |this, cx| this.swept(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// The message being sent that the block at `at`, just added, records: the one the daemon
    /// took as its delivery.
    fn sent_as(&self, at: usize, page: &SessionPage, cx: &gpui_kit::App) -> Option<u64> {
        let delivery = self.model.delivery_of(at)?;
        if self.id != page.id() {
            return None;
        }
        page.reply
            .read(cx)
            .pending()
            .iter()
            .find(|p| p.is(delivery))
            .map(|p| p.key)
    }

    /// Whether the user's message at `at` waits to be drawn: one sent from here waits for the
    /// daemon's answer, and it may be its record.
    fn holds(&self, at: usize, page: &SessionPage, cx: &gpui_kit::App) -> bool {
        let user = matches!(&self.model.blocks()[at], Block::Message { from, .. } if from == USER);
        user && self.id == page.id()
            && page
                .reply
                .read(cx)
                .pending()
                .iter()
                .any(|p| !p.queued && p.in_flight())
    }

    fn list() -> ListState {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        list
    }

    /// The button back to the end moves this list.
    fn watch(&self, cx: &mut Context<Self>) {
        let (list, me) = (self.list.clone(), cx.entity().downgrade());
        self.to_end
            .update(cx, |to_end, cx| to_end.watch(list, me, cx));
    }

    /// How many items of the list a segment is.
    fn items_of(&self, segment: &Segment) -> usize {
        match segment {
            Segment::Row(at) => 1 + self.more[*at],
            Segment::Group(group) => 1 + group.items.len(),
        }
    }

    /// How many items come before the segments: the line that asks for earlier events, which
    /// stays while it folds away.
    fn head(&self) -> usize {
        usize::from(self.earlier || self.head_leaving)
    }

    /// The list's first item of a segment.
    fn item_of(&self, segment: usize) -> usize {
        self.head()
            + self.segments[..segment.min(self.segments.len())]
                .iter()
                .map(|s| self.items_of(s))
                .sum::<usize>()
    }

    /// The list's first item of a row: its own, or its group's line while it is folded in.
    fn item(&self, at: usize) -> usize {
        let Some(&segment) = self.segment_of.get(at) else {
            return self.item_of(self.segments.len());
        };
        let first = self.item_of(segment);
        match &self.segments[segment] {
            Segment::Group(group) => group
                .items
                .iter()
                .position(|shown| shown.at == at)
                .map_or(first, |n| first + 1 + n),
            Segment::Row(_) => first,
        }
    }

    /// The segment of the list's item `ix`, and which of its items it is.
    fn segment_at(&self, ix: usize) -> Option<(usize, usize)> {
        let mut ix = ix.checked_sub(self.head())?;
        for (at, segment) in self.segments.iter().enumerate() {
            let items = self.items_of(segment);
            if ix < items {
                return Some((at, ix));
            }
            ix -= items;
        }
        None
    }

    /// Gives the row `at` one item per piece of its reply, and measures again its last; a row
    /// of a group is measured again.
    fn fit(&mut self, at: usize, cx: &gpui_kit::App) {
        let first = self.item(at);
        if !matches!(self.segments[self.segment_of[at]], Segment::Row(_)) {
            self.list.remeasure_items(first..first + 1);
            return;
        }
        let more = self.rows[at].read(cx).more_pieces().len();
        let had = self.more[at];
        if more != had {
            let kept = first + 1 + had.min(more);
            self.list
                .splice(kept..first + 1 + had, more.saturating_sub(had));
            self.more[at] = more;
        }
        self.list.remeasure_items(first + more..first + more + 1);
    }

    /// Places the row `at`, just added at the end: a row of its own, or the last group grows
    /// by it. Whether a new segment was made.
    fn place(&mut self, at: usize) -> bool {
        let prose = in_sight(&self.model.blocks()[at]);
        if !prose
            && let Some(Segment::Group(group)) = self.segments.last_mut()
            && group.rows.end == at
        {
            group.rows.end = at + 1;
            self.segment_of.push(self.segments.len() - 1);
            return false;
        }
        // Only rows made before the events read stand before it: its start may be on an
        // earlier page.
        let cut = !prose
            && self.earlier
            && self.segments.iter().all(
                |segment| matches!(segment, Segment::Row(row) if self.model.seq_of(*row) == 0),
            );
        self.segment_of.push(self.segments.len());
        self.segments.push(match prose {
            true => Segment::Row(at),
            false => Segment::Group(Group {
                rows: at..at + 1,
                summary: Summary::default(),
                needs: Vec::new(),
                cut,
                open: false,
                items: Vec::new(),
            }),
        });
        true
    }

    /// Says the line of a group again, from its rows alone. Whether it changed.
    fn summarise(&mut self, segment: usize) -> bool {
        let Some(Segment::Group(group)) = self.segments.get_mut(segment) else {
            return false;
        };
        let rows = group.rows.clone();
        self.walked += rows.len();
        let said = Summary::of(
            &self.model.blocks()[rows.clone()],
            &self.model.times()[rows],
        );
        let changed = group.summary != said;
        group.summary = said;
        changed
    }

    /// Finds again the rows of a group that stay out of its fold. Whether they changed.
    fn find_needs(&mut self, segment: usize, cx: &gpui_kit::App) -> bool {
        let Some(Segment::Group(group)) = self.segments.get(segment) else {
            return false;
        };
        let rows = group.rows.clone();
        self.walked += rows.len();
        let needs: Vec<usize> = rows.filter(|&at| self.needs_the_user(at, cx)).collect();
        let Some(Segment::Group(group)) = self.segments.get_mut(segment) else {
            return false;
        };
        let changed = group.needs != needs;
        group.needs = needs;
        changed
    }

    /// The rows of a group that are to be items of the list: all of them unfolded (but a
    /// request answered by a card below it, which shows nothing), else those that stay out of
    /// the fold.
    fn wanted(&self, group: &Group) -> Vec<usize> {
        match group.open {
            true => group
                .rows
                .clone()
                .filter(|&at| {
                    let block = &self.model.blocks()[at];
                    !matches!(block, Block::Answered { .. }) && !is_hidden(block)
                })
                .collect(),
            false => group.needs.clone(),
        }
    }

    /// Makes the items of a group's rows what its fold wants: a row that comes unfolds, one
    /// that goes folds away first, both over [`PANEL`], and the list follows.
    fn reconcile(&mut self, segment: usize, cx: &mut Context<Self>) {
        let Some(Segment::Group(group)) = self.segments.get(segment) else {
            return;
        };
        let wanted = self.wanted(group);
        let mut ix = self.item_of(segment) + 1;
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let Some(Segment::Group(group)) = self.segments.get_mut(segment) else {
            return;
        };
        let mut old = std::mem::take(&mut group.items).into_iter().peekable();
        let mut wanted = wanted.into_iter().peekable();
        let mut leaving = false;
        enum Step {
            Keep,
            Come(usize),
            Leave,
        }
        loop {
            let step = match (old.peek().map(|shown| shown.at), wanted.peek().copied()) {
                (None, None) => break,
                (Some(o), Some(w)) if o == w => Step::Keep,
                (Some(o), Some(w)) if w < o => Step::Come(w),
                (None, Some(w)) => Step::Come(w),
                (Some(_), _) => Step::Leave,
            };
            match step {
                Step::Keep => {
                    let mut shown = old.next().expect("peeked");
                    wanted.next();
                    if shown.motion.target() != 1.0 {
                        shown.motion.animate(1.0, PANEL, now, reduce);
                    }
                    group.items.push(shown);
                    ix += 1;
                }
                Step::Come(at) => {
                    wanted.next();
                    let mut motion = Motion::settled(0.0);
                    motion.animate(1.0, PANEL, now, reduce);
                    self.list.splice(ix..ix, 1);
                    group.items.push(Shown {
                        at,
                        motion,
                        height: Rc::default(),
                    });
                    ix += 1;
                }
                Step::Leave => {
                    let mut shown = old.next().expect("peeked");
                    if shown.motion.target() != 0.0 {
                        shown.motion.animate(0.0, PANEL, now, reduce);
                    }
                    group.items.push(shown);
                    leaving = true;
                    ix += 1;
                }
            }
        }
        if leaving {
            self.sweep = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PANEL + FRAME).await;
                this.update(cx, |this, cx| this.swept(cx)).ok();
            }));
        }
    }

    /// Takes out of the list the rows of groups whose fold away has ended.
    fn swept(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let mut ix = usize::from(self.earlier);
        let mut again = false;
        for segment in 0..self.segments.len() {
            let items = self.items_of(&self.segments[segment]);
            let Segment::Group(group) = &mut self.segments[segment] else {
                ix += items;
                continue;
            };
            ix += 1;
            let list = &self.list;
            group.items.retain(|shown| {
                let leaving = shown.motion.target() == 0.0;
                let gone = leaving && !shown.motion.is_running(now);
                again |= leaving && !gone;
                match gone {
                    true => list.splice(ix..ix + 1, 0),
                    false => ix += 1,
                }
                !gone
            });
        }
        let first = self.pending_item();
        let mut n = 0;
        let list = &self.list;
        self.pending.retain(|shown| {
            let leaving = shown.motion.target() == 0.0;
            let gone = leaving && !shown.motion.is_running(now);
            again |= leaving && !gone;
            match gone {
                true => list.splice(first + n..first + n + 1, 0),
                false => n += 1,
            }
            !gone
        });
        self.sweep = again.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FRAME).await;
                this.update(cx, |this, cx| this.swept(cx)).ok();
            })
        });
        cx.notify();
    }

    pub fn rows(&self) -> &[Entity<Row>] {
        &self.rows
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn blocks(&self) -> &[Block] {
        self.model.blocks()
    }

    /// When the block at this index came.
    pub fn at(&self, index: usize) -> Option<u64> {
        self.model.at(index)
    }

    /// How many times a row was looked at or changed so far.
    pub fn touched(&self) -> usize {
        self.touched
    }

    /// How many rows of groups were walked so far to say their lines and what stays out of
    /// their folds.
    pub fn walked(&self) -> usize {
        self.walked
    }

    /// How many items the list holds.
    pub fn item_count(&self) -> usize {
        self.list.item_count()
    }

    /// How far the conversation is scrolled from its top.
    pub fn scroll_offset(&self) -> Pixels {
        -self.list.scroll_px_offset_for_scrollbar().y
    }

    /// The button back to the end.
    pub fn to_end(&self) -> &Entity<ToEnd> {
        &self.to_end
    }

    /// Whether older events than those shown remain.
    pub fn has_earlier(&self) -> bool {
        self.earlier
    }

    /// The user scrolled the list: a page of earlier events is asked when what is left above
    /// is about a window's worth. Only a scroll of the user's gets here, never the list
    /// opening on its end nor the jump back to it.
    pub(crate) fn scrolled(&mut self, cx: &mut Context<Self>) {
        if !self.earlier || self.load != Load::Idle || !self.near_top() {
            return;
        }
        let page = self.page.clone();
        // The list is still borrowed while it tells: the page is asked once the scroll is done.
        cx.defer(move |cx| {
            page.update(cx, |page, cx| page.show_earlier(cx)).ok();
        });
    }

    /// Whether the items above the list's top are no more than those in sight. The list measures
    /// only what it draws, so heights above would count what was never seen as nothing.
    fn near_top(&self) -> bool {
        let top = self.list.logical_scroll_top();
        let area = self.list.viewport_bounds();
        let in_sight = (top.item_ix..)
            .take_while(|&ix| {
                self.list
                    .bounds_for_item(ix)
                    .is_some_and(|bounds| bounds.top() < area.bottom())
            })
            .count();
        top.item_ix <= in_sight.max(1)
    }

    /// A page of earlier events is asked: whether it is a page asked again after it failed, or
    /// `None` when there is none to ask or one is on its way already.
    pub(crate) fn asking(&mut self, cx: &mut Context<Self>) -> Option<bool> {
        if !self.earlier {
            return None;
        }
        let again = match self.load {
            Load::Asked => return None,
            Load::Failed(_) => true,
            Load::Idle => false,
        };
        self.load = Load::Asked;
        self.head_sweep = None;
        if self.spinner.is_none() {
            let status = cx.new(|_| Status::new(AgentState::Running));
            self.spinner = Some(cx.new(|cx| StateIcon::new(status, cx)));
        }
        cx.notify();
        Some(again)
    }

    /// The page asked for did not come, with why.
    pub(crate) fn load_failed(&mut self, why: String, cx: &mut Context<Self>) {
        if self.load == Load::Idle || self.load == Load::Failed(why.clone()) {
            return;
        }
        self.load = Load::Failed(why);
        self.sweep_head(cx);
        cx.notify();
    }

    /// Lets the loading line fold away before the head and the spinner go.
    fn sweep_head(&mut self, cx: &mut Context<Self>) {
        self.head_sweep = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANEL + FRAME).await;
            this.update(cx, |this, cx| this.head_swept(cx)).ok();
        }));
    }

    fn head_swept(&mut self, cx: &mut Context<Self>) {
        if self.head_leaving {
            self.head_leaving = false;
            self.list.splice(0..1, 0);
        }
        if self.load != Load::Asked {
            self.spinner = None;
        }
        cx.notify();
    }

    /// What is at the top of the list, to put it back there once the rows are made again.
    fn anchor(&self) -> Option<Anchor> {
        let top = self.list.logical_scroll_top();
        let (segment, piece, offset) = match self.segment_at(top.item_ix) {
            Some((segment, piece)) => (segment, piece, top.offset_in_item),
            // On the head, the first block is what is read, at its top.
            None if top.item_ix < self.head() && !self.segments.is_empty() => (0, 0, px(0.)),
            None => return None,
        };
        let (at, kind) = match &self.segments[segment] {
            Segment::Row(at) => (*at, Reading::Piece(piece)),
            Segment::Group(group) if piece == 0 => (group.rows.start, Reading::Line),
            Segment::Group(group) => (group.items.get(piece - 1)?.at, Reading::Piece(0)),
        };
        Some(Anchor {
            seq: self.model.seq_of(at),
            card: matches!(self.model.blocks()[at], Block::SubAgents),
            kind,
            offset,
        })
    }

    /// Scrolls the list to what the user was reading, wherever it is now: older rows came in
    /// above it.
    fn restore(&self, anchor: &Anchor) {
        let found = match anchor.card {
            true => {
                (0..self.rows.len()).find(|&at| matches!(self.model.blocks()[at], Block::SubAgents))
            }
            false => (0..self.rows.len())
                .rev()
                .find(|&at| self.model.seq_of(at) <= anchor.seq),
        };
        let Some(at) = found else {
            return;
        };
        let segment = self.segment_of[at];
        let item_ix = match (&self.segments[segment], anchor.kind) {
            (Segment::Group(_), Reading::Line) => self.item_of(segment),
            (Segment::Row(_), Reading::Piece(piece)) => self.item(at) + piece.min(self.more[at]),
            _ => self.item(at),
        };
        self.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: anchor.offset,
        });
    }

    /// The groups the conversation folds, each the rows it holds.
    pub fn groups(&self) -> Vec<Range<usize>> {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Group(group) => Some(group.rows.clone()),
                Segment::Row(_) => None,
            })
            .collect()
    }

    /// The id of the line of the group that holds these rows.
    pub fn group_id(&self, rows: &Range<usize>) -> Option<String> {
        let segment = *self.segment_of.get(rows.start)?;
        matches!(self.segments[segment], Segment::Group(_)).then(|| self.line_id(segment))
    }

    /// Whether the conversation follows its end.
    pub fn is_following(&self) -> bool {
        self.list.is_following_tail()
    }

    /// The keys a group's fold is kept by, the first naming it.
    fn keys(&self, segment: usize) -> Vec<Key> {
        let Some(Segment::Group(group)) = self.segments.get(segment) else {
            return Vec::new();
        };
        let next = (group.rows.end < self.model.blocks().len())
            .then(|| Key::Before(self.model.seq_of(group.rows.end)));
        match group.cut {
            true => vec![next.unwrap_or(Key::Cut)],
            false => std::iter::once(Key::First(self.model.seq_of(group.rows.start)))
                .chain(next)
                .collect(),
        }
    }

    /// The id of a group's line, which its other ids derive from: stable while earlier history
    /// loads.
    fn line_id(&self, segment: usize) -> String {
        match self.keys(segment).first() {
            Some(Key::First(seq)) => format!("group-{seq}"),
            Some(Key::Before(seq)) => format!("group-before-{seq}"),
            _ => "group-cut".into(),
        }
    }

    fn is_unfolded(&self, segment: usize, cx: &gpui_kit::App) -> bool {
        let Some(keys) = cx
            .try_global::<Unfolded>()
            .and_then(|unfolded| unfolded.0.get(&self.id))
        else {
            return false;
        };
        self.keys(segment).iter().any(|key| keys.contains(key))
    }

    /// Keeps whether a group is unfolded, by each of its keys.
    fn keep(&self, segment: usize, cx: &mut gpui_kit::App) {
        let Some(Segment::Group(group)) = self.segments.get(segment) else {
            return;
        };
        let (open, keys) = (group.open, self.keys(segment));
        let kept = cx
            .default_global::<Unfolded>()
            .0
            .entry(self.id.clone())
            .or_default();
        for key in keys {
            match open {
                true => kept.insert(key),
                false => kept.remove(&key),
            };
        }
    }

    /// The user folds or unfolds the group of `segment`.
    fn toggle(&mut self, segment: usize, cx: &mut Context<Self>) {
        let Some(Segment::Group(group)) = self.segments.get_mut(segment) else {
            return;
        };
        group.open = !group.open;
        self.keep(segment, cx);
        self.reconcile(segment, cx);
        cx.notify();
    }

    /// Makes the rows again from what a link read: the events are folded first, then each
    /// block gets its row.
    fn rebuild(&mut self, read: Read, page: &SessionPage, cx: &mut Context<Self>) {
        // A page of earlier events keeps the user where they were reading.
        let anchor = (!read.follow).then(|| self.anchor()).flatten();
        let had_head = self.head() == 1;
        self.model = self.model.restarted();
        self.model.place_approvals(read.placed);
        for event in read.events {
            self.model.apply(event);
        }
        if read.launched_before {
            self.model.prepend_sub_agents();
        }
        self.earlier = read.earlier;
        self.load = Load::Idle;
        self.head_leaving = had_head && !self.earlier;
        if had_head {
            self.sweep_head(cx);
        }
        self.rows.clear();
        self.specials.clear();
        self.more.clear();
        self.segments.clear();
        self.segment_of.clear();
        self.sweep = None;
        self.in_place.clear();
        self.held.clear();
        // A message being sent that the history holds is recorded: it leaves. The user's
        // messages recorded since one was sent that waits for its answer wait with it.
        if self.id == page.id() {
            let pending = page.reply.read(cx).pending().to_vec();
            let recorded: Vec<u64> = pending
                .iter()
                .filter(|p| {
                    (0..self.model.blocks().len())
                        .any(|at| self.model.delivery_of(at).is_some_and(|d| p.is(d)))
                })
                .map(|p| p.key)
                .collect();
            for key in &recorded {
                page.reply.update(cx, |reply, cx| reply.resolve(*key, cx));
            }
            self.pending.retain(|shown| !recorded.contains(&shown.key));
            let since = pending
                .iter()
                .filter(|p| !p.queued && p.in_flight())
                .filter_map(|p| p.since)
                .min();
            if let Some(since) = since {
                let known = |at: usize| {
                    self.model
                        .delivery_of(at)
                        .is_some_and(|d| pending.iter().any(|p| p.is(d)))
                };
                self.held = (0..self.model.blocks().len())
                    .filter(|&at| {
                        matches!(&self.model.blocks()[at], Block::Message { from, .. } if from == USER)
                            && self.model.seq_of(at) > since
                            && !known(at)
                    })
                    .collect();
            }
        }
        self.pending.retain(|shown| shown.motion.target() == 1.0);
        for at in 0..self.model.blocks().len() {
            self.add_row(at, page, cx);
            self.more[at] = self.rows[at].read(cx).more_pieces().len();
            self.place(at);
        }
        // Each group is said once, whole, and shows what the user left unfolded, at rest.
        for segment in 0..self.segments.len() {
            if !matches!(self.segments[segment], Segment::Group(_)) {
                continue;
            }
            self.summarise(segment);
            self.find_needs(segment, cx);
            let open = self.is_unfolded(segment, cx);
            if let Segment::Group(group) = &mut self.segments[segment] {
                group.open = open;
            }
            let Segment::Group(group) = &self.segments[segment] else {
                continue;
            };
            let wanted = self.wanted(group);
            if let Segment::Group(group) = &mut self.segments[segment] {
                group.items = wanted
                    .into_iter()
                    .map(|at| Shown {
                        at,
                        motion: Motion::settled(1.0),
                        height: Rc::default(),
                    })
                    .collect();
            }
        }
        let items = self.item_of(self.segments.len()) + self.pending.len();
        match read.follow {
            true => {
                self.list = Self::list();
                self.list.reset(items);
                self.list.scroll_to_end();
                self.watch(cx);
            }
            // Earlier pages read: the rows shown before stay where they were, below them.
            false => {
                self.list.reset(items);
                if let Some(anchor) = anchor {
                    self.restore(&anchor);
                }
            }
        }
        cx.notify();
    }

    fn add_row(&mut self, at: usize, page: &SessionPage, cx: &mut Context<Self>) {
        let block = self.model.blocks()[at].clone();
        if matches!(
            block,
            Block::SubAgents | Block::Approval { .. } | Block::Question { .. }
        ) {
            self.specials.push(at);
        }
        let me = self.model.me().to_owned();
        let root = self.id == page.id();
        let when = self.model.at(at);
        let row = Row::view(at, block, when, me, root, page, self.page.clone(), cx);
        self.rows.push(row);
        self.more.push(0);
        self.touched += 1;
    }

    /// A group changed: its line is said again, and the rows that stay out of its fold follow.
    /// Whether it is to be drawn again.
    fn group_changed(&mut self, segment: usize, cx: &mut Context<Self>) -> bool {
        let said = self.summarise(segment);
        let needs = self.find_needs(segment, cx);
        if needs || said {
            self.reconcile(segment, cx);
        }
        needs || said
    }

    /// Adds a row for each block added, and draws again each row whose block changed; a group
    /// whose line changed is drawn again with it.
    fn changed(&mut self, changes: &[Change], page: &SessionPage, cx: &mut Context<Self>) {
        let mut redraw = false;
        for change in changes {
            match change {
                Change::Added => {
                    let at = self.rows.len();
                    let sent = self.sent_as(at, page, cx);
                    if let Some(key) = sent {
                        page.reply.update(cx, |reply, cx| reply.resolve(key, cx));
                    }
                    let shown = sent.and_then(|key| self.pending.iter().position(|s| s.key == key));
                    match shown {
                        // The message being sent becomes the daemon's, in its place.
                        Some(n) => {
                            let ix = self.pending_item() + n;
                            self.list.splice(ix..ix + 1, 0);
                            let SentRow { key, row, .. } = self.pending.remove(n);
                            let block = self.model.blocks()[at].clone();
                            let when = self.model.at(at);
                            row.update(cx, |row, cx| row.recorded(at, block, when, cx));
                            self.rows.push(row);
                            self.more.push(0);
                            self.touched += 1;
                            self.in_place.insert(at, key);
                        }
                        None => {
                            self.add_row(at, page, cx);
                            if sent.is_none() && self.holds(at, page, cx) {
                                self.held.insert(at);
                            }
                        }
                    }
                    let item = self.item_of(self.segments.len());
                    if self.place(at) {
                        self.list.splice(item..item, 1);
                        // The group before is no longer the last: its question folds in, and
                        // its fold is kept by the block after it too.
                        let segment = self.segments.len() - 1;
                        if segment > 0 && matches!(self.segments[segment - 1], Segment::Group(_)) {
                            self.keep(segment - 1, cx);
                            self.group_changed(segment - 1, cx);
                        }
                    }
                    let segment = self.segment_of[at];
                    if let Segment::Group(group) = &self.segments[segment] {
                        if group.rows.len() == 1 {
                            let open = self.is_unfolded(segment, cx);
                            if let Segment::Group(group) = &mut self.segments[segment] {
                                group.open = open;
                            }
                        }
                        self.summarise(segment);
                        self.find_needs(segment, cx);
                        self.reconcile(segment, cx);
                    } else {
                        self.fit(at, cx);
                    }
                    if matches!(self.model.blocks()[at], Block::Text { .. }) {
                        self.to_end.update(cx, |to_end, cx| to_end.wrote(cx));
                    }
                    redraw = true;
                }
                Change::Changed(at) => {
                    let block = self.model.blocks()[*at].clone();
                    if !matches!(
                        block,
                        Block::SubAgents | Block::Approval { .. } | Block::Question { .. }
                    ) {
                        // A request answered by a card has no card of its own to bring up to date.
                        self.specials.retain(|special| special != at);
                    }
                    if let Some(row) = self.rows.get(*at).cloned() {
                        self.touched += 1;
                        row.update(cx, |row, cx| row.set_block(block, cx));
                        let item = self.item(*at);
                        self.list.remeasure_items(item..item + 1);
                        self.fit(*at, cx);
                        let segment = self.segment_of[*at];
                        if matches!(self.segments[segment], Segment::Group(_)) {
                            redraw |= self.group_changed(segment, cx);
                        }
                    }
                }
                Change::Appended(at, piece) => {
                    if let Some(row) = self.rows.get(*at).cloned() {
                        self.touched += 1;
                        row.update(cx, |row, cx| row.append(piece, cx));
                        self.fit(*at, cx);
                    }
                }
            }
        }
        if redraw {
            cx.notify();
        }
    }

    /// A row's card was brought up to date: a request answered folds into its group.
    fn card_changed(&mut self, at: usize, cx: &mut Context<Self>) {
        if let Some(&segment) = self.segment_of.get(at)
            && matches!(self.segments[segment], Segment::Group(_))
            && self.find_needs(segment, cx)
        {
            self.reconcile(segment, cx);
        }
        let item = self.item(at);
        self.list.remeasure_items(item..item + 1);
        cx.notify();
    }

    /// Whether the row `at` of a group shows under the group's line while it is folded: a call
    /// held for the user's answer, a command still running, a question to the user the daemon
    /// says is open.
    fn needs_the_user(&self, at: usize, cx: &gpui_kit::App) -> bool {
        match &self.model.blocks()[at] {
            Block::Approval { .. } => self.rows[at]
                .read(cx)
                .card()
                .is_some_and(|card| card.decision.is_none()),
            Block::Command(card) => card.ending == Ending::Running,
            Block::Question { .. } => self.rows[at].read(cx).asking,
            _ => false,
        }
    }

    /// A row of the conversation as the list draws it, entering when it first shows.
    fn row_element(&self, at: usize) -> AnyElement {
        if self.in_place.contains_key(&at) {
            return self.rows[at].clone().into_any_element();
        }
        // Named by its event, not its index: a page of earlier events shifts the indices, and
        // what is read must not enter again.
        let seq = self.model.seq_of(at);
        let same = (0..at)
            .rev()
            .take_while(|&before| self.model.seq_of(before) == seq)
            .count();
        enter(
            ElementId::Name(format!("enter-row-{}-{seq}-{same}", self.id).into()),
            0,
            self.rows[at].clone(),
        )
        .into_any_element()
    }

    /// A group's line: how many steps, their kinds, how many failed and how long it took, or
    /// the step running now; a click unfolds its rows.
    fn render_group_line(
        &mut self,
        segment: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(Segment::Group(group)) = self.segments.get(segment) else {
            return div().into_any_element();
        };
        let (open, pieces) = (group.open, group_line(&group.summary, group.cut));
        let failed = (group.summary.failed > 0).then(|| format!("{} failed", group.summary.failed));
        let line_id = self.line_id(segment);
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("{line_id}{part}").into());
        let said = SharedString::from(pieces.join(" · "));
        let turn = turning(id("-chevron"), open, window, cx);
        // The kinds and the running step are what can be long: they are cut with an ellipsis,
        // whole in a tooltip. The count, the failures and the time keep their room.
        let running = group.summary.running.clone();
        let kinds = (!group.summary.kinds.is_empty()).then(|| group.summary.kinds.join(", "));
        let shown: Vec<AnyElement> = pieces
            .into_iter()
            .enumerate()
            .map(|(n, piece)| {
                let lead = if n == 0 { "" } else { " · " };
                let part = if running.as_ref() == Some(&piece) {
                    Some("running")
                } else if n == 1 && kinds.as_ref() == Some(&piece) {
                    Some("kinds")
                } else {
                    None
                };
                match (failed.as_ref() == Some(&piece), part) {
                    (true, _) => div()
                        .flex()
                        .flex_none()
                        .child(lead)
                        .child(
                            div()
                                .id(id("-failed"))
                                .text_color(p.fail)
                                .aria_label(SharedString::from(piece.clone()))
                                .child(piece.clone())
                                .test_support(),
                        )
                        .into_any_element(),
                    (false, Some(part)) => div()
                        .flex()
                        .min_w_0()
                        .child(div().flex_none().child(lead))
                        .child(tipped_when_cut(
                            div()
                                .id(id(&format!("-{part}")))
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(piece.clone())),
                            SharedString::from(piece.clone()),
                            window,
                            cx,
                        ))
                        .into_any_element(),
                    (false, None) => div()
                        .flex_none()
                        .child(format!("{lead}{piece}"))
                        .into_any_element(),
                }
            })
            .collect();
        hoverable(
            div()
                .id(id(""))
                .flex()
                .items_center()
                .gap(space::SM)
                .py(space::XS)
                .cursor_pointer()
                .mono(12.)
                .aria_label(said.clone())
                .aria_expanded(open)
                .child(
                    icon(Icon::Chevron)
                        .size(px(12.))
                        .flex_none()
                        .text_color(p.faint)
                        .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * turn))),
                )
                .child(changing(id("-said"), said, move |_| {
                    div()
                        .flex()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .children(shown)
                        .into_any_element()
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(segment, cx))),
            move |d, t| d.text_color(mix(p.faint, p.mute, t)),
        )
        .into_any_element()
    }

    /// The first item while earlier events remain: "Show earlier", the fallback and the keys'
    /// way, or while a page comes, a quiet line saying so, and why a page did not come. Each
    /// unfolds and folds away.
    fn render_head(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let (loading, failed) = match &self.load {
            Load::Idle => (false, None),
            Load::Asked => (true, None),
            Load::Failed(why) => (false, Some(why.clone())),
        };
        let page = self.page.clone();
        let button = Button::quiet("show-earlier", "Show earlier").on_click(move |_, _, cx| {
            page.update(cx, |page, cx| page.show_earlier(cx)).ok();
        });
        let line = div()
            .id("earlier-loading-line")
            .flex()
            .items_center()
            .gap(space::SM)
            .py(space::XS)
            .text_color(p.faint)
            .aria_label("Loading earlier…")
            .children(self.spinner.clone())
            .child("Loading earlier…")
            .test_support();
        let reason = failed
            .filter(|_| self.earlier)
            .map(|why| SharedString::from(format!("Earlier events did not load: {why}")));
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(presence(
                "show-earlier-fold",
                self.earlier && !loading,
                button,
            ))
            .child(presence_of("show-earlier-failed", reason, move |why| {
                div()
                    .id("show-earlier-reason")
                    .pt(space::XS)
                    .text_color(p.fail)
                    .aria_label(why.clone())
                    .child(why)
                    .test_support()
                    .into_any_element()
            }))
            .child(presence("earlier-loading", self.earlier && loading, line))
            .into_any_element()
    }

    /// The list's item `ix`: "Show earlier", a row, a piece of a row's long reply, a group's
    /// line or one of its rows, in the conversation's column.
    fn render_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let narrow = window.viewport_size().width < px(crate::shell::NARROW);
        let (segment, piece) = self.segment_at(ix).unwrap_or((usize::MAX, 0));
        let last = ix + 1 == self.pending_item() + self.pending.len();
        let column = |below: Pixels| {
            div()
                .max_w(px(780.))
                .w_full()
                .mx_auto()
                .px(if narrow { space::LG } else { space::XXL })
                .when(ix == 0, |d| {
                    d.pt(if narrow { space::XL } else { space::XXL })
                })
                .pb(below)
        };
        let gap = if last { px(20.) } else { row_gap() };
        // A list's item is not stretched to its width: the column is centred in a full one.
        let centred = |column: Div| div().w_full().child(column).into_any_element();
        if self.head() == 1 && ix == 0 {
            let head = self.render_head(cx);
            return centred(column(gap).child(head));
        }
        match self.segments.get(segment) {
            // The steps of links shown are their card's: a group of nothing else has no line.
            Some(Segment::Group(group))
                if piece == 0
                    && group
                        .rows
                        .clone()
                        .all(|at| is_hidden(&self.model.blocks()[at])) =>
            {
                div().into_any_element()
            }
            Some(Segment::Group(_)) if piece == 0 => {
                let line = self.render_group_line(segment, window, cx);
                centred(column(gap).child(line))
            }
            Some(Segment::Group(group)) => {
                let Some(shown) = group.items.get(piece - 1) else {
                    return div().into_any_element();
                };
                let (at, motion, height) = (shown.at, shown.motion, shown.height.clone());
                let now = cx.background_executor().now();
                let running = motion.is_running(now);
                let open = motion.target() == 1.0;
                if !open && !running {
                    return div().into_any_element();
                }
                if running {
                    window.request_animation_frame();
                }
                let t = motion.value(now);
                let id = ElementId::Name(format!("{}-row-{at}", self.line_id(segment)).into());
                let row = fold_frame(id, open, running, t, &height, self.rows[at].clone());
                // The gap under it folds with it.
                centred(column(gap * t).child(row))
            }
            Some(Segment::Row(at)) if self.held.contains(at) => div().into_any_element(),
            Some(Segment::Row(at)) => {
                let (at, more) = (*at, self.more[*at]);
                let row = &self.rows[at];
                if piece == 0 {
                    // Between the pieces of one reply, the gap of its paragraphs.
                    let below = match more {
                        0 => gap,
                        _ => TextViewStyle::default()
                            .paragraph_gap()
                            .to_pixels(window.rem_size()),
                    };
                    return centred(column(below).child(self.row_element(at)));
                }
                let below = match piece < more {
                    true => TextViewStyle::default()
                        .paragraph_gap()
                        .to_pixels(window.rem_size()),
                    false => gap,
                };
                let row = row.read(cx);
                let shown = row.more_pieces().get(piece - 1).cloned();
                // The last piece of a long reply carries its time and Copy.
                if piece == more {
                    let p = palette(cx);
                    let said = row.said_text(div().children(shown), p);
                    return centred(column(below).child(said));
                }
                centred(column(below).children(shown))
            }
            None => {
                let Some(shown) = ix
                    .checked_sub(self.pending_item())
                    .and_then(|n| self.pending.get(n))
                else {
                    return div().into_any_element();
                };
                let now = cx.background_executor().now();
                let running = shown.motion.is_running(now);
                let open = shown.motion.target() == 1.0;
                if !open && !running {
                    return div().into_any_element();
                }
                if running {
                    window.request_animation_frame();
                }
                let t = shown.motion.value(now);
                let key = shown.key;
                let row = enter(
                    ElementId::Name(format!("enter-pending-{}-{key}", self.id).into()),
                    0,
                    shown.row.clone(),
                );
                let row = fold_frame(
                    pending_part(key, "item"),
                    open,
                    running,
                    t,
                    &shown.height,
                    row,
                );
                // The gap under it folds with it.
                centred(column(gap * t).child(row))
            }
        }
    }
}

/// How often a motion the conversation drives itself moves: one frame.
const FRAME: Duration = Duration::from_millis(16);

/// Whether a notice is a delivery the agent was sent, folded to one line, rather than the
/// agent's own end or resumption, or a session it started, said whole.
fn is_delivery(kind: &str) -> bool {
    !matches!(kind, "failed" | "ended" | "resumed" | STARTED_SESSION_KIND)
}

/// The pieces of a group's line: how many steps (of the part read, when its start is on an
/// earlier page), their kinds, how many failed, how long it took or the step running now.
pub fn group_line(summary: &Summary, cut: bool) -> Vec<String> {
    let mut pieces = vec![format!(
        "{} {}{}",
        summary.steps,
        if summary.steps == 1 { "step" } else { "steps" },
        if cut { " loaded" } else { "" }
    )];
    if !summary.kinds.is_empty() {
        pieces.push(summary.kinds.join(", "));
    }
    if summary.failed > 0 {
        pieces.push(format!("{} failed", summary.failed));
    }
    pieces.extend(summary.running.clone());
    pieces.extend(summary.ms.map(seconds));
    pieces
}

impl Render for ConversationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let narrow = window.viewport_size().width < px(crate::shell::NARROW);
        let id = SharedString::from(format!("conversation-{}", self.id));
        if self.rows.is_empty() && !self.earlier && self.pending.is_empty() {
            return div()
                .id(id)
                .size_full()
                .child(
                    div()
                        .max_w(px(780.))
                        .w_full()
                        .mx_auto()
                        .px(if narrow { space::LG } else { space::XXL })
                        .pt(if narrow { space::XL } else { space::XXL })
                        .text_color(p.faint)
                        .child("Nothing said yet."),
                )
                .into_any_element();
        }
        div()
            .id(id)
            .size_full()
            .relative()
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix, window, cx| this.render_item(ix, window, cx)),
                )
                .size_full(),
            )
            .child(self.to_end.clone())
            .test_support()
            .into_any_element()
    }
}

/// A child in the sub-agents list.
#[derive(Clone, Debug, PartialEq)]
pub struct SubAgent {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    /// Its provider and model, as a line.
    pub runs_on: String,
    pub doing: String,
    pub status: Entity<Status>,
    /// A request of its own waits for the user: a child never needs them itself, but what it
    /// asked and its parent forwarded is theirs to answer.
    pub asks: bool,
}

/// What a request's card shows, once the page knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalCard {
    pub number: u64,
    /// In the base: it always comes back to the user, whatever the mode.
    pub base: bool,
    /// What the Auto judge said, in one line, when it was judged.
    pub judgement: Option<String>,
    /// `call`, or `action` (a change of the project the agent asks for).
    pub kind: String,
    /// What it changes, in words, one line each.
    pub says: Vec<String>,
    pub asked: Option<String>,
    /// The call itself, on one line: a command's line, else the tool and its arguments.
    pub call: Option<String>,
    pub why: Option<String>,
    pub source: Option<String>,
    pub asker: Option<String>,
    /// `deny`, `once` or `cancelled`, once decided.
    pub decision: Option<String>,
}

impl ApprovalCard {
    /// Why a base card always comes back: a session's first message to another asks a leave,
    /// which the daemon's reason names.
    pub fn always_says(&self) -> &'static str {
        let leave = self
            .why
            .as_deref()
            .is_some_and(|why| why.contains("has not written to that one before"));
        match leave {
            true => LEAVE_SAYS,
            false => BASE_SAYS,
        }
    }

    /// The one-line record a decided card folds into.
    pub fn record(&self) -> Option<String> {
        let said = match self.decision.as_deref()? {
            "deny" => "Denied",
            "once" => "Allowed",
            "cancelled" => "Cancelled",
            other => other,
        };
        let mut parts = vec![said.to_owned()];
        parts.extend(self.asked.clone());
        parts.extend(self.asker.clone());
        if parts.len() == 1 {
            parts.push(format!("#{}", self.number));
        }
        Some(parts.join(" · "))
    }
}

/// One row of the conversation: a block, and what the row keeps of its own (a fold, the
/// thinking timer, the call marks, the card's answer in flight).
pub struct Row {
    at: usize,
    /// The agent whose conversation it is, as the page names it.
    me: String,
    /// It is the root's: a conversation with one agent needs no name over its replies.
    root: bool,
    page: WeakEntity<SessionPage>,
    /// Shared with nothing: a piece streamed is appended to it in place.
    block: Rc<Block>,
    /// When its block came.
    when: Option<u64>,
    /// A thought, a report or an edit's diff unfolded.
    open: bool,
    /// The reply or the thought as Markdown, in the pieces [`markdown_pieces`] cuts: a piece
    /// streamed parses the last one again, never the whole reply.
    markdown: Vec<Entity<MarkdownPiece>>,
    /// How many bytes of Markdown were handed to the parser to read again so far: a piece that
    /// did not change counts none, one that grew counts its whole length (the parser reads
    /// again its last block, which is no more than that).
    parsed: usize,
    /// Where a selection begun in its text stays: see [`MarkdownPiece`].
    scope: TextSelectionScopeId,
    /// What the agent received for a message that used commands, selectable as the message is: the
    /// text it was made from, and its view.
    received: Option<(String, MarkdownText)>,
    thinking: Option<(Entity<PulsingLabel>, Entity<ThinkingTimer>)>,
    calls: Vec<(Entity<Status>, Entity<StateIcon>)>,
    sub_agents: Vec<SubAgent>,
    card: Option<ApprovalCard>,
    icons: HashMap<String, Entity<StateIcon>>,
    pills: HashMap<String, Entity<Pill>>,
    /// Its question waits for the user's answer, as the daemon says: its card shows.
    asking: bool,
    /// The field that answers its question, made when the card first shows.
    answer: Option<Entity<AnswerBox>>,
    /// It is a message the user sent from the composer: being sent, refused, or recorded since.
    sent: Option<Sent>,
    /// The lines of a card of links, and the pictures among them.
    links: LinksCard,
    renders: usize,
}

/// A message of the user shown from the moment it was sent. Its fades are kept here rather than
/// by `fade` and `presence`: once the daemon records it the row moves to another item of the
/// list and is drawn under other ids, and the state of an element would start over.
struct Sent {
    key: u64,
    /// The pictures as they were sent: they need no reading back.
    images: Vec<Arc<gpui_kit::Image>>,
    /// Why the daemon refused it, the last time it did: said while the reason folds away.
    refused: Option<String>,
    /// 1 once recorded: the ink of a message sent.
    ink: Motion,
    /// 1 while it is being sent: the `Sending…` line.
    sending: Motion,
    sending_height: Rc<Cell<Pixels>>,
    /// 1 while refused: its reason, Retry and Edit.
    failed: Motion,
    failed_height: Rc<Cell<Pixels>>,
}

/// How much of its ink a message keeps while it is being sent.
const QUIET: f32 = 0.6;

impl Row {
    #[allow(clippy::too_many_arguments)]
    fn view(
        at: usize,
        block: Block,
        when: Option<u64>,
        me: String,
        root: bool,
        page: &SessionPage,
        weak: WeakEntity<SessionPage>,
        cx: &mut gpui_kit::App,
    ) -> Entity<Row> {
        let sub_agents = match block {
            Block::SubAgents => page.sub_agents(),
            _ => Vec::new(),
        };
        let card = match block {
            Block::Approval { number } => page.card(number),
            _ => None,
        };
        let asking = match block {
            Block::Question { seq, .. } => page.asking(seq),
            _ => false,
        };
        cx.new(|cx| {
            let mut row = Row {
                at,
                me,
                root,
                page: weak,
                block: Rc::new(Block::Text {
                    text: String::new(),
                }),
                when,
                open: false,
                markdown: Vec::new(),
                parsed: 0,
                scope: TextSelectionScopeId::new(),
                received: None,
                thinking: None,
                calls: Vec::new(),
                sub_agents,
                card,
                icons: HashMap::new(),
                pills: HashMap::new(),
                asking,
                answer: None,
                sent: None,
                links: LinksCard::default(),
                renders: 0,
            };
            row.set_block(block, cx);
            row
        })
    }

    /// The row of a message being sent: the bubble the daemon's record of it will be, quieter.
    fn sending(
        pending: &Pending,
        root: bool,
        page: &SessionPage,
        weak: WeakEntity<SessionPage>,
        cx: &mut gpui_kit::App,
    ) -> Entity<Row> {
        let block = Block::Message {
            from: USER.into(),
            text: pending.text.clone(),
            images: (0..pending.images.len())
                .map(|n| format!("pending-{}-{n}", pending.key))
                .collect(),
            commands: None,
        };
        let row = Row::view(0, block, None, String::new(), root, page, weak, cx);
        row.update(cx, |row, _| {
            row.sent = Some(Sent {
                key: pending.key,
                images: pending.images.clone(),
                refused: pending.refused.clone(),
                ink: Motion::settled(0.0),
                sending: Motion::settled(if pending.refused.is_some() { 0.0 } else { 1.0 }),
                sending_height: Rc::default(),
                failed: Motion::settled(if pending.refused.is_some() { 1.0 } else { 0.0 }),
                failed_height: Rc::default(),
            });
        });
        row
    }

    /// The message being sent was refused (`Some`), or is sent again (`None`).
    fn set_refused(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        let (now, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let Some(sent) = self.sent.as_mut() else {
            return;
        };
        let failed = refused.is_some();
        if sent.failed.target() == f32::from(u8::from(failed)) && sent.refused == refused {
            return;
        }
        if refused.is_some() {
            sent.refused = refused;
        }
        sent.failed
            .animate(if failed { 1.0 } else { 0.0 }, PANEL, now, reduce);
        sent.sending
            .animate(if failed { 0.0 } else { 1.0 }, PANEL, now, reduce);
        cx.notify();
    }

    /// The message being sent is recorded: the row is the daemon's, at `at`, in its place, and its
    /// ink and its line move to the sent state.
    fn recorded(&mut self, at: usize, block: Block, when: Option<u64>, cx: &mut Context<Self>) {
        let (now, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        self.at = at;
        self.when = when;
        if let Some(sent) = self.sent.as_mut() {
            sent.ink.animate(1.0, PANEL, now, reduce);
            sent.sending.animate(0.0, PANEL, now, reduce);
            sent.failed.animate(0.0, PANEL, now, reduce);
        }
        self.set_block(block, cx);
        cx.notify();
    }

    /// The lines under a message the user sent while they show: `Sending…`, or why it was
    /// refused with Retry and Edit; each unfolds and folds away.
    fn sent_lines(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(sent) = &self.sent else {
            return Vec::new();
        };
        let now = cx.background_executor().now();
        let p = palette(cx);
        let key = sent.key;
        let mut lines = Vec::new();
        let mut fold = |motion: &Motion, height: &Rc<Cell<Pixels>>, part: &str, line: Div| {
            let (t, running) = (motion.value(now), motion.is_running(now));
            if running {
                window.request_animation_frame();
            }
            if t > 0.0 || running {
                let frame = fold_frame(
                    pending_part(key, part),
                    motion.target() == 1.0,
                    running,
                    t,
                    height,
                    line,
                );
                lines.push(frame.into_any_element());
            }
        };
        let sending = div().pt(space::XS).flex().justify_end().child(
            div()
                .id(pending_part(key, "sending"))
                .flex()
                .items_center()
                .gap(space::XS)
                .text_size(px(11.5))
                .text_color(p.faint)
                .aria_label(SENDING)
                .child(icon(Icon::Clock).size(px(11.)).text_color(p.faint))
                .child(SENDING)
                .test_support(),
        );
        fold(&sent.sending, &sent.sending_height, "sending-line", sending);
        let why = sent.refused.clone().unwrap_or_default();
        let said = SharedString::from(format!("{NOT_SENT_LINE} · {why}"));
        let page = self.page.clone();
        let on = move |act: fn(&mut Reply, u64, &mut Window, &mut Context<Reply>)| {
            let page = page.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                let Some(reply) = page.upgrade().map(|page| page.read(cx).reply.clone()) else {
                    return;
                };
                reply.update(cx, |reply, cx| act(reply, key, window, cx));
            }
        };
        let failed = div()
            .pt(space::XS)
            .flex()
            .flex_col()
            .items_end()
            .gap(space::XS)
            .child(
                div()
                    .id(pending_part(key, "failed"))
                    .max_w(px(540.))
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .text_size(px(11.5))
                    .text_color(p.fail)
                    .aria_label(said.clone())
                    .child(icon(Icon::Warning).size(px(11.)).text_color(p.fail))
                    .child(said)
                    .test_support(),
            )
            .child(
                div()
                    .flex()
                    .gap(space::XS)
                    .child(
                        Button::quiet(pending_part(key, "retry"), RETRY)
                            .leading(Icon::Restart)
                            .on_click(on(|reply, key, _, cx| reply.retry(key, cx))),
                    )
                    .child(
                        Button::quiet(pending_part(key, "edit"), crate::queued::EDIT)
                            .leading(Icon::Edit)
                            .on_click(on(|reply, key, window, cx| {
                                reply.edit_pending(key, window, cx)
                            })),
                    ),
            );
        fold(&sent.failed, &sent.failed_height, "failed-line", failed);
        lines
    }

    pub fn block(&self) -> &Block {
        &self.block
    }

    /// The field that answers its question, once its card showed.
    pub fn answer(&self) -> Option<&Entity<AnswerBox>> {
        self.answer.as_ref()
    }

    /// Its question is open, or no longer: its card unfolds or folds away.
    fn set_asking(&mut self, asking: bool, cx: &mut Context<Self>) {
        if self.asking != asking {
            self.asking = asking;
            cx.notify();
        }
    }

    /// The line drawn over the row, when it has one: who sent a message (none for the user's:
    /// the bubble says it), whose report it is, who launched the sub-agents, who asked a request.
    pub fn label(&self) -> Option<String> {
        match &*self.block {
            Block::Message { from, .. } if from != USER => Some(from.clone()),
            Block::Relayed { peer, sent, .. } => Some(match sent {
                true => format!("to {peer}"),
                false => format!("from {peer}"),
            }),
            Block::Text { .. } => self.author().map(str::to_owned),
            Block::Report { from, .. } => Some(format!("Report from {from}")),
            Block::SubAgents => Some(format!("launched by {}", self.me)),
            Block::Approval { .. } => self
                .card
                .as_ref()
                .and_then(|card| card.record().or_else(|| card.source.clone())),
            _ => None,
        }
    }

    /// The head of a thought: what its label says, then the time it took when one is shown.
    pub fn thought_head(&self, cx: &gpui_kit::App) -> Option<String> {
        let Block::Thought { since, until, .. } = &*self.block else {
            return None;
        };
        let (label, timer) = self.thinking.as_ref()?;
        let label = label.read(cx).text();
        Some(match until.is_none() || timed(*since, *until) {
            true => format!("{label} {}", timer.read(cx).time()),
            false => label.to_owned(),
        })
    }

    /// How many bytes of Markdown were handed to the parser to read again so far: what a piece
    /// streamed costs the parser, whatever the machine does meanwhile.
    pub fn parsed(&self) -> usize {
        self.parsed
    }

    /// How many times the pieces of its Markdown were drawn so far: a frame draws the pieces in
    /// view, never all of a long reply.
    pub fn pieces_drawn(&self, cx: &gpui_kit::App) -> usize {
        self.markdown.iter().map(|m| m.read(cx).renders).sum()
    }

    /// The text a reply or a thought shows, its Markdown drawn: as of the last parse that landed.
    pub fn rendered(&self, cx: &gpui_kit::App) -> Option<String> {
        let pieces = self
            .markdown
            .iter()
            .map(|piece| piece.read(cx).state.read(cx));
        let rendered = pieces.map(|state| state.rendered_text().as_str().to_owned());
        matches!(
            *self.block,
            Block::Text { .. }
                | Block::Thought { .. }
                | Block::Message { .. }
                | Block::Relayed { .. }
                | Block::Report { .. }
        )
        .then(|| rendered.collect())
    }

    /// Selects all the text the row shows, as `Ctrl+A` over it does.
    pub fn select_all(&self, cx: &mut Context<Self>) {
        for piece in &self.markdown {
            let state = piece.read(cx).state.clone();
            state.update(cx, |state, cx| state.select_all(cx));
        }
    }

    /// What the user selected in the text the row shows.
    pub fn selected_text(&self, cx: &gpui_kit::App) -> String {
        self.markdown
            .iter()
            .map(|piece| piece.read(cx).state.read(cx).selected_text())
            .collect()
    }

    /// The pieces of a reply past its first, which the list draws as items of their own.
    pub fn more_pieces(&self) -> &[Entity<MarkdownPiece>] {
        match *self.block {
            Block::Text { .. } => self.markdown.get(1..).unwrap_or_default(),
            _ => &[],
        }
    }

    /// Shows `text` as Markdown: each piece that did not change keeps its parse, and one that
    /// grew parses what it gained.
    fn set_markdown(&mut self, text: &str, muted: bool, chips: bool, cx: &mut Context<Self>) {
        let pieces = markdown_pieces(text);
        for (at, piece) in pieces.iter().enumerate() {
            match self.markdown.get(at) {
                Some(markdown) => {
                    let (state, grew) = markdown.update(cx, |markdown, _| {
                        markdown.chips = chips;
                        let grew = markdown.bytes != piece.len();
                        markdown.bytes = piece.len();
                        (markdown.state.clone(), grew)
                    });
                    self.parsed += if grew { piece.len() } else { 0 };
                    state.update(cx, |state, cx| state.set_text(piece, cx));
                }
                None => {
                    let state = cx.new(|cx| TextViewState::markdown(piece, cx));
                    let scope = self.scope;
                    self.parsed += piece.len();
                    self.markdown.push(cx.new(|_| MarkdownPiece {
                        state,
                        muted,
                        chips,
                        scope,
                        bytes: piece.len(),
                        renders: 0,
                    }));
                }
            }
        }
        self.markdown.truncate(pieces.len());
    }

    /// The name over an agent's reply: none for the root's.
    pub fn author(&self) -> Option<&str> {
        match *self.block {
            Block::Text { .. } if !self.root => Some(&self.me),
            _ => None,
        }
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn sub_agents(&self) -> &[SubAgent] {
        &self.sub_agents
    }

    pub fn card(&self) -> Option<&ApprovalCard> {
        self.card.as_ref()
    }

    /// The state each call's mark shows.
    pub fn marks(&self, cx: &gpui_kit::App) -> Vec<AgentState> {
        self.calls.iter().map(|(s, _)| s.read(cx).state()).collect()
    }

    fn set_block(&mut self, block: Block, cx: &mut Context<Self>) {
        if let Block::Links { links } = &block {
            self.links.show(links, cx, |row: &mut Row| &mut row.links);
        }
        if let Block::Text { text }
        | Block::Thought { text, .. }
        | Block::Report { text, .. }
        | Block::Question { text, .. } = &block
        {
            let muted = matches!(block, Block::Thought { .. });
            self.set_markdown(text, muted, false, cx);
        }
        // The user's own message is selectable like a reply: the same view, fed what was typed,
        // each command in it a chip.
        match &block {
            Block::Message { text, commands, .. } => {
                let chips = commands
                    .as_ref()
                    .map(|commanded| command_chips(text, &commanded.names))
                    .unwrap_or_default();
                self.set_markdown(
                    &typed_with_chips(text, &chips),
                    false,
                    !chips.is_empty(),
                    cx,
                );
                self.received = commands
                    .as_ref()
                    .map(|commanded| match self.received.take() {
                        Some((seen, view)) if seen == commanded.received => (seen, view),
                        _ => (
                            commanded.received.clone(),
                            MarkdownText::quiet(&as_typed(&commanded.received), cx),
                        ),
                    });
            }
            Block::Relayed { text, .. } => self.set_markdown(&as_typed(text), false, false, cx),
            _ => {}
        }
        match &block {
            Block::Thought { since, until, .. } => {
                let (since, until) = (*since, *until);
                let ended = if timed(since, until) {
                    "Thought for"
                } else {
                    "Thought"
                };
                match &self.thinking {
                    None => {
                        let label = cx.new(|cx| {
                            PulsingLabel::new(
                                if until.is_none() { "Thinking" } else { ended },
                                until.is_none(),
                                cx,
                            )
                        });
                        let timer = cx.new(|cx| ThinkingTimer::new(since, until, cx));
                        self.thinking = Some((label, timer));
                    }
                    Some((label, timer)) if until.is_some() => {
                        label.update(cx, |label, cx| {
                            label.set_active(false, cx);
                            label.set_text(ended, cx);
                        });
                        timer.update(cx, |timer, cx| timer.end(until, cx));
                    }
                    Some(_) => {}
                }
            }
            Block::Steps { calls } => {
                for (at, call) in calls.iter().enumerate() {
                    let state = call_state(call);
                    match self.calls.get(at) {
                        Some((status, _)) => {
                            if status.read(cx).state() != state {
                                status.update(cx, |status, cx| status.set(state, cx));
                            }
                        }
                        None => {
                            let status = cx.new(|_| Status::new(state));
                            let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
                            self.calls.push((status, icon));
                        }
                    }
                }
            }
            _ => {}
        }
        if !matches!(block, Block::Approval { .. }) {
            self.card = None;
        }
        self.block = Rc::new(block);
        cx.notify();
    }

    /// The reply or the thought it shows goes on with `piece`.
    fn append(&mut self, piece: &str, cx: &mut Context<Self>) {
        let block = Rc::make_mut(&mut self.block);
        let muted = matches!(block, Block::Thought { .. });
        if let Block::Text { text } | Block::Thought { text, .. } = block {
            text.push_str(piece);
            let text = text.clone();
            self.set_markdown(&text, muted, false, cx);
            cx.notify();
        }
    }
}

/// The line of a step whose result Flottille cut at the source (`docs/tools.md`, "Long
/// results"): how many characters were left out. Empty, and unlabelled, while none was.
fn cut_said(id: ElementId, cut: Option<u64>, ink: gpui_kit::Hsla) -> impl IntoElement {
    let said = cut.map(|left_out| {
        SharedString::from(format!(
            "cut at the source · {} characters left out",
            crate::settings::grouped(left_out as usize)
        ))
    });
    div()
        .id(id)
        .mono(11.5)
        .text_color(ink)
        .when_some(said.clone(), |d, said| d.aria_label(said))
        .children(said)
        .test_support()
}

/// What the folded line of a block of steps says of who let them through: each of the Auto judge
/// and Full mode, with how many when not all of them.
fn allowed_said(calls: &[Call]) -> String {
    let mut bys: Vec<(&str, usize)> = Vec::new();
    for by in calls.iter().filter_map(|call| call.allowed_by.as_deref()) {
        match bys.iter_mut().find(|(seen, _)| *seen == by) {
            Some((_, n)) => *n += 1,
            None => bys.push((by, 1)),
        }
    }
    bys.iter()
        .map(|(by, n)| match *n == calls.len() {
            true => format!(" · allowed by {by}"),
            false => format!(" · {n} allowed by {by}"),
        })
        .collect()
}

/// What the folded sub-agents card says: how many there are and in which state, as in
/// `23 sub-agents · 21 completed · 2 running`, and how many wait for the user, who are counted
/// there and not under their state.
fn sub_agents_said(agents: &[SubAgent], cx: &App) -> (String, usize) {
    let asks = agents.iter().filter(|agent| agent.asks).count();
    let mut line = format!(
        "{} {}",
        agents.len(),
        if agents.len() == 1 {
            "sub-agent"
        } else {
            "sub-agents"
        }
    );
    for state in [
        AgentState::Completed,
        AgentState::Running,
        AgentState::Waiting,
        AgentState::Idle,
        AgentState::Stopped,
        AgentState::Failed,
    ] {
        let n = agents
            .iter()
            .filter(|agent| !agent.asks && agent.status.read(cx).state() == state)
            .count();
        if n > 0 {
            line.push_str(&format!(" · {n} {}", state.label().to_lowercase()));
        }
    }
    (line, asks)
}

/// What the user typed, as the Markdown that draws it as typed: each mark is escaped, each line
/// keeps its break and its indentation. An address the renderer links by itself is left as typed:
/// an escape inside it would be drawn, backslash and all.
fn as_typed(text: &str) -> String {
    typed_with_chips(text, &[])
}

/// [`as_typed`], each of `chips` (byte ranges of `text`, the commands it used) drawn as inline
/// code, which the message's view styles as the accent's chip: its characters are the command's,
/// nothing in it is escaped.
fn typed_with_chips(text: &str, chips: &[std::ops::Range<usize>]) -> String {
    let line = |line: &str| {
        let base = line.as_ptr() as usize - text.as_ptr() as usize;
        let indent = line.len() - line.trim_start_matches(' ').len();
        let mut out = "\u{a0}".repeat(indent);
        let line = &line[indent..];
        let mut at = 0;
        let mut previous = None;
        while at < line.len() {
            let ahead = &line[at..];
            let here = base + indent + at;
            if let Some(chip) = chips
                .iter()
                .find(|chip| chip.start == here && !text[(*chip).clone()].contains('`'))
            {
                out.push_str(&format!("`{}`", &text[chip.clone()]));
                at += chip.len();
                previous = Some('`');
                continue;
            }
            if let Some(length) = address(previous, ahead) {
                out.push_str(&ahead[..length]);
                at += length;
                previous = ahead[..length].chars().last();
                continue;
            }
            let c = ahead.chars().next().unwrap_or_default();
            if "\\`*_{}[]()#+-.!|<>~&$".contains(c) {
                out.push('\\');
            }
            out.push(c);
            at += c.len_utf8();
            previous = Some(c);
        }
        out
    };
    text.split("\n\n")
        .map(|paragraph| paragraph.lines().map(line).collect::<Vec<_>>().join("\\\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// How many bytes of `ahead` are an address the Markdown renderer links without a mark (GFM's
/// autolink literal): `http://`, `https://` or `www.` after a character that does not continue a
/// word, to the next space or `<`, less the punctuation that ends a sentence and a closing
/// bracket nothing opened.
fn address(previous: Option<char>, ahead: &str) -> Option<usize> {
    let starts = |prefix: &str| {
        ahead
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    };
    let linked = match previous {
        Some(c) if c.is_alphabetic() => false,
        _ => starts("http://") || starts("https://"),
    } || (starts("www.")
        && previous.is_none_or(|c| c.is_whitespace() || "(*_[]~".contains(c)));
    if !linked {
        return None;
    }
    let mut end = ahead
        .find(|c: char| c.is_whitespace() || c == '<')
        .unwrap_or(ahead.len());
    while let Some(last) = ahead[..end].chars().next_back() {
        let opened = ahead[..end].matches('(').count();
        let closed = ahead[..end].matches(')').count();
        match last {
            '?' | '!' | '.' | ',' | ':' | ';' | '*' | '_' | '~' | '\'' | '"' => {}
            ')' if closed > opened => {}
            _ => break,
        }
        end -= last.len_utf8();
    }
    Some(end)
}

/// A text drawn as Markdown on its own, in a selection scope of its own: a pull request's
/// description, a comment. It is drawn as a reply is.
pub struct MarkdownText {
    pieces: Vec<Entity<MarkdownPiece>>,
    scope: TextSelectionScopeId,
}

impl MarkdownText {
    pub fn new(text: &str, cx: &mut App) -> Self {
        Self::drawn(text, false, cx)
    }

    /// In the muted ink, as a thought is.
    pub fn quiet(text: &str, cx: &mut App) -> Self {
        Self::drawn(text, true, cx)
    }

    fn drawn(text: &str, muted: bool, cx: &mut App) -> Self {
        let scope = TextSelectionScopeId::new();
        let pieces = markdown_pieces(text)
            .into_iter()
            .map(|piece| {
                let state = cx.new(|cx| TextViewState::markdown(piece, cx));
                cx.new(|_| MarkdownPiece {
                    state,
                    muted,
                    chips: false,
                    scope,
                    bytes: piece.len(),
                    renders: 0,
                })
            })
            .collect();
        Self { pieces, scope }
    }

    pub fn view(&self, id: impl Into<ElementId>) -> AnyElement {
        markdown_view(id.into(), &self.pieces, self.scope).into_any_element()
    }
}

/// A reply or a thought, its Markdown pieces one under the other, in one text-cursor region: the
/// pointer keeps the I-beam from its first line to its last, gaps between the pieces included. A
/// press in a gap begins the selection in this message too.
fn markdown_view(
    id: ElementId,
    pieces: &[Entity<MarkdownPiece>],
    scope: TextSelectionScopeId,
) -> impl IntoElement {
    text_region(id)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            TextSelection::activate_scope(scope, window, cx)
        })
        .flex()
        .flex_col()
        .gap(TextViewStyle::default().paragraph_gap())
        .children(pieces.iter().cloned())
        .test_support()
}

/// One piece of a reply's or a thought's Markdown, its own view: a piece streamed draws the last
/// one again, and the others as they were.
///
/// A selection stays in the message it begins in, the scope of its row: the kit orders the text
/// views of a selection by the order they painted in the last frame, and a retained view that did
/// not paint keeps the order of an older one, so a selection across two messages saw its ends
/// swap from one frame to the next and its highlight jump between them, which blinked. Within
/// one message there is one view, or a few in their order.
pub struct MarkdownPiece {
    state: Entity<TextViewState>,
    /// A thought's, in the muted ink.
    muted: bool,
    /// Its inline code is the commands of a message the user typed: the accent's chip.
    chips: bool,
    scope: TextSelectionScopeId,
    /// The length of the text it was last given, to tell a piece that grew from one that did not.
    bytes: usize,
    /// How many times it was drawn.
    renders: usize,
}

impl Render for MarkdownPiece {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let scope = self.scope;
        // The scope is set where the view paints: a piece drawn again alone paints without its
        // row around it.
        let p = palette(cx);
        let mut style = markdown_style(&p, self.muted);
        if self.chips {
            style = style.with_inline_code(gpui_kit::HighlightStyle {
                color: Some(p.acc),
                background_color: Some(p.acc.opacity(0.14)),
                font_weight: Some(gpui_kit::FontWeight::MEDIUM),
                ..Default::default()
            });
        }
        div()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                TextSelection::activate_scope(scope, window, cx)
            })
            .child(
                TextView::new(&self.state)
                    .style(style)
                    .image_source(no_image),
            )
            .text_selection_scope(scope)
    }
}

/// How a reply's Markdown is drawn, in Flottille's palette alone: a thought (`muted`) in the muted
/// ink. A table's frame, header and cells are set here too, or the view draws them on its own
/// theme's surface, white.
pub fn markdown_style(p: &Palette, muted: bool) -> TextViewStyle {
    let ink = if muted { p.mute } else { p.text };
    TextViewStyle::default()
        .with_foreground(ink)
        .with_muted_foreground(p.mute)
        .with_link(p.acc)
        .with_selection(p.sel)
        .with_code_background(p.card2)
        .with_code_block(gpui_kit::StyleRefinement::default().mono(12.))
        .with_inline_code(gpui_kit::HighlightStyle {
            color: Some(p.text),
            background_color: Some(p.card2),
            ..Default::default()
        })
        .with_table(gpui_kit::StyleRefinement::default().bg(p.card))
        .with_table_cell(gpui_kit::StyleRefinement::default().text_color(ink))
        .with_border(p.line)
        .with_dark(p.bg.l < 0.5)
}

/// Past this many bytes, a reply is cut into another Markdown piece where a block starts.
const MARKDOWN_PIECE: usize = 4096;

/// A reply cut into pieces that parse alone as they would together: each cut, once the piece
/// before it holds [`MARKDOWN_PIECE`] bytes, is before a line at the left margin that starts a
/// block (after a blank line, or a list item after a list item), never inside a fenced code
/// block. The parser reads again the last block of what it shows on every piece streamed, and
/// keeps the others by copying them all: in one piece, a long reply would cost more on each.
pub fn markdown_pieces(text: &str) -> Vec<&str> {
    let item = |line: &str| {
        let numbered = line.trim_start_matches(|c: char| c.is_ascii_digit());
        ["- ", "* ", "+ "].iter().any(|m| line.starts_with(m))
            || (numbered.len() < line.len() && numbered.starts_with(". "))
    };
    let (mut pieces, mut start, mut at) = (Vec::new(), 0, 0);
    let (mut fenced, mut previous): (bool, Option<&str>) = (false, None);
    for line in text.split_inclusive('\n') {
        let margin = line.starts_with(|c: char| !c.is_whitespace());
        let starts_block = previous.is_some_and(|p| p.trim().is_empty() || (item(p) && item(line)));
        if !fenced && margin && starts_block && at - start >= MARKDOWN_PIECE {
            pieces.push(&text[start..at]);
            start = at;
        }
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
        }
        previous = Some(line);
        at += line.len();
    }
    pieces.push(&text[start..]);
    pieces
}

/// An image of a reply is never loaded: its address, which the agent wrote, would reach what the
/// gate does not see.
pub fn no_image(_: &SharedUri) -> ImageSource {
    ImageSource::Custom(Arc::new(|_, _| {
        Some(Err(ImageCacheError::Asset("images are not loaded".into())))
    }))
}

/// What Copy puts on the clipboard, and the toast says.
pub const COPIED: &str = "Copied";

/// A message with what a hover shows under it: when it was said, and Copy, which puts its text
/// on the clipboard and says it in a toast. Both fade in with the hover, in a line of their own
/// that is always there, so that nothing moves around them. `end`: on the right, under the user's
/// bubble.
fn said(
    id: String,
    content: impl IntoElement,
    end: bool,
    when: Option<u64>,
    text: String,
    p: Palette,
) -> impl IntoElement {
    let part = move |part: &str| ElementId::Name(format!("{id}-{part}").into());
    let (meta, time, copy) = (part("meta"), part("time"), part("copy"));
    hoverable(
        div()
            .id(part("said"))
            .flex()
            .flex_col()
            .when(end, |d| d.items_end())
            .child(content),
        move |d, t| {
            let line = div()
                .id(meta)
                .flex_none()
                .h(px(28.))
                .mt(space::XS)
                .flex()
                .items_center()
                .gap(space::SM)
                .opacity(t)
                // Hidden, it holds nothing to click.
                .when(t > 0.0, |d| {
                    d.children(when.map(|ms| {
                        let said = SharedString::from(said_at(ms));
                        div()
                            .id(time)
                            .mono(11.)
                            .text_color(p.faint)
                            .aria_label(said.clone())
                            .child(said)
                            .test_support()
                    }))
                    .child(
                        Button::icon(copy, Icon::Copy, "Copy").on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                text.clone(),
                            ));
                            toast(Toast::success(COPIED), cx);
                        }),
                    )
                })
                .test_support();
            d.child(line)
        },
    )
}

/// The button that copies a file's whole path, the one a line cut with an ellipsis stands for.
/// Its click is its own: it neither opens the edit nor folds the steps.
fn copy_path(id: ElementId, path: &str) -> Button {
    let path = path.to_owned();
    Button::icon(id, Icon::Copy, "Copy the path").on_click(move |_, _, cx| {
        cx.stop_propagation();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.clone()));
        toast(Toast::success("Path copied").detail(path.clone()), cx);
    })
}

fn call_state(call: &Call) -> AgentState {
    match call.state {
        CallState::Running => AgentState::Running,
        CallState::Done => AgentState::Completed,
        CallState::Failed => AgentState::Failed,
        CallState::Interrupted => AgentState::Stopped,
    }
}

/// Where the commands of a message the user typed stand: each `/name` of `names`.
pub fn command_chips(text: &str, names: &[String]) -> Vec<std::ops::Range<usize>> {
    flottille_core::commands::used(text, |name| names.iter().any(|known| known == name))
        .into_iter()
        .map(|(range, _)| range)
        .collect()
}

/// The line that says the agent received more than was typed.
pub const RECEIVED: &str = "What the agent received";

impl Row {
    /// Under a message that used commands: "What the agent received", which unfolds into it.
    fn render_received(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("row-{}-{part}", self.at).into());
        let turn = turning(id("received-toggle"), self.open, window, cx);
        div()
            .id(id("received-part"))
            .flex()
            .flex_col()
            .px(space::SM)
            .child(hoverable(
                div()
                    .id(id("received-toggle"))
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .aria_label(RECEIVED)
                    .aria_expanded(self.open)
                    .child(RECEIVED)
                    .child(
                        icon(Icon::Chevron).size(px(12.)).with_transformation(
                            Transformation::rotate(radians(-FRAC_PI_2 * turn)),
                        ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open = !this.open;
                        cx.notify();
                    })),
                move |d, t| d.text_color(mix(p.faint, p.mute, t)),
            ))
            .child(presence(
                id("received"),
                self.open,
                div()
                    .id(id("received-text"))
                    .pt(space::SM)
                    .mono(12.)
                    .when_some(self.received.as_ref(), |d, (received, _)| {
                        d.aria_label(SharedString::from(received.clone()))
                    })
                    .children(
                        self.received
                            .as_ref()
                            .map(|(_, view)| view.view(id("received-region"))),
                    )
                    .test_support(),
            ))
            .into_any_element()
    }
}

impl Render for Row {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        // A message being sent is named by its key until the daemon records it.
        let pending = self
            .sent
            .as_ref()
            .filter(|sent| sent.ink.target() == 0.0)
            .map(|sent| sent.key);
        let id = |part: &str| match pending {
            Some(key) => pending_part(key, part),
            None => ElementId::Name(format!("row-{}-{part}", self.at).into()),
        };
        // The row's own block, not a copy of it.
        let block = self.block.clone();
        match &*block {
            Block::Message {
                text,
                images,
                commands,
                ..
            } => {
                let page = self.page.upgrade();
                let pictures: Vec<Option<Arc<gpui_kit::Image>>> = match &self.sent {
                    Some(sent) if !sent.images.is_empty() => {
                        sent.images.iter().cloned().map(Some).collect()
                    }
                    _ => images
                        .iter()
                        .map(|name| page.as_ref()?.read(cx).pictures.get(name).cloned())
                        .collect(),
                };
                let worded = !text.trim().is_empty();
                let now = cx.background_executor().now();
                let ink = self.sent.as_ref().map_or(1.0, |sent| {
                    if sent.ink.is_running(now) {
                        window.request_animation_frame();
                    }
                    sent.ink.value(now)
                });
                let quiet = QUIET + (1.0 - QUIET) * ink;
                // The pictures sit inside the bubble, above its text.
                let thumbs = (!pictures.is_empty()).then(|| {
                    div()
                        .id(id("images"))
                        .flex()
                        .flex_wrap()
                        .gap(space::SM)
                        .when(quiet < 1.0, |d| d.opacity(quiet))
                        .children(pictures.into_iter().enumerate().map(|(n, picture)| {
                            let thumb = div()
                                .id(id(&format!("image-{n}")))
                                .flex_none()
                                .w(px(220.))
                                .h(px(132.))
                                .rounded(px(9.))
                                .border_1()
                                .border_color(p.line2)
                                .bg(p.card)
                                .overflow_hidden();
                            match picture {
                                Some(picture) => thumb.child(
                                    gpui_kit::img(picture)
                                        .size_full()
                                        .rounded(px(9.))
                                        .object_fit(gpui_kit::ObjectFit::Cover),
                                ),
                                None => thumb,
                            }
                            .test_support()
                        }))
                        .test_support()
                });
                let lines = self.sent_lines(window, cx);
                let message = div()
                    .id(id("message"))
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .id(id("bubble"))
                            .max_w(px(540.))
                            .flex()
                            .flex_col()
                            .gap(space::SM)
                            .bg(p.card2)
                            .rounded_tl(px(14.))
                            .rounded_tr(px(14.))
                            .rounded_bl(px(14.))
                            .rounded_br(px(4.))
                            .p(space::SM)
                            .aria_label(SharedString::from(text.clone()))
                            .when_some(self.label(), |d, from| {
                                d.child(
                                    div()
                                        .id(id("message-from"))
                                        .flex()
                                        .justify_end()
                                        .px(space::SM)
                                        .mono(11.5)
                                        .text_color(p.faint)
                                        .child(SharedString::from(from))
                                        .test_support(),
                                )
                            })
                            .children(thumbs)
                            .when(worded, |d| {
                                d.child(
                                    div()
                                        .id(id("message-text"))
                                        .px(space::SM)
                                        .py(space::XS)
                                        .when(quiet < 1.0, |d| d.opacity(quiet))
                                        .child(markdown_view(
                                            id("message-region"),
                                            &self.markdown,
                                            self.scope,
                                        ))
                                        .test_support(),
                                )
                            })
                            .children(commands.as_ref().map(|_| self.render_received(window, cx)))
                            .test_support(),
                    )
                    .children(lines)
                    .test_support();
                let said_id = match pending {
                    Some(key) => format!("pending-{key}"),
                    None => format!("row-{}", self.at),
                };
                said(said_id, message, true, self.when, text.clone(), p).into_any_element()
            }
            // Another session's words, or what this one sent it: the agent's side, marked with
            // who it is from or to, never drawn as the user's.
            Block::Relayed {
                peer,
                agent,
                sent,
                text,
            } => {
                let page = self.page.upgrade();
                let named = SharedString::from(
                    page.as_ref()
                        .map(|page| page.read(cx).peer_named(peer, cx))
                        .unwrap_or_else(|| peer.clone()),
                );
                let session = page
                    .as_ref()
                    .and_then(|page| page.read(cx).peer_session(agent.as_deref(), cx));
                let said_by = SharedString::from(match sent {
                    true => format!("to {named}"),
                    false => format!("from {named}"),
                });
                // A title can be long: it is cut inside the bubble, whole in a tooltip.
                let title = tipped_when_cut(
                    div()
                        .id(id("relayed-name"))
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(named.clone()),
                    named.clone(),
                    window,
                    cx,
                );
                // The name opens the session when it is still there; said plainly when it is
                // gone. A click on it is not a click on the card it sits in.
                let name = match session {
                    Some(session) => {
                        let shell = page.as_ref().map(|page| page.read(cx).shell.clone());
                        hoverable(
                            div()
                                .id(id("relayed-peer"))
                                .min_w_0()
                                .cursor_pointer()
                                .aria_label(named.clone())
                                .child(title)
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    if let Some(shell) = &shell {
                                        let session = session.clone();
                                        shell
                                            .update(cx, |shell, cx| {
                                                shell.open(Selection::Session(session), window, cx)
                                            })
                                            .ok();
                                    }
                                }),
                            move |d, t| d.text_color(mix(p.faint, p.text, t)),
                        )
                        .into_any_element()
                    }
                    None => div().min_w_0().child(title).into_any_element(),
                };
                let from = div()
                    .id(id("relayed-from"))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap(space::SM)
                    .mono(11.5)
                    .text_color(p.faint)
                    .aria_label(said_by)
                    .child(
                        icon(if *sent { Icon::Send } else { Icon::Agents })
                            .size(px(12.))
                            .flex_none()
                            .text_color(p.faint),
                    )
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .gap(space::SM)
                            .child(div().flex_none().child(if *sent { "to" } else { "from" }))
                            .child(name),
                    )
                    .test_support();
                let bubble = div()
                    .id(id("relayed-card"))
                    // One width for the whole fold: the card is as wide as the conversation lets
                    // it be, folded, unfolding and open.
                    .when(!*sent, |d| d.w_full())
                    .max_w(px(540.))
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .border_1()
                    .border_color(p.line2)
                    .rounded_tl(px(14.))
                    .rounded_tr(px(14.))
                    .rounded_br(px(14.))
                    .rounded_bl(px(4.))
                    .px(space::LG)
                    .py(space::MD)
                    .test_support();
                // Another session's message is folded to its first line and who sent it, until a
                // click opens it: a report can be long. What this one sent is its own words.
                let bubble = match sent {
                    true => bubble.child(from).child(markdown_view(
                        id("relayed-region"),
                        &self.markdown,
                        self.scope,
                    )),
                    false => {
                        let turn = turning(id("relayed-toggle"), self.open, window, cx);
                        let first = SharedString::from(
                            text.lines()
                                .map(str::trim)
                                .find(|line| !line.is_empty())
                                .unwrap_or_default()
                                .to_owned(),
                        );
                        bubble
                            .child(hoverable(
                                div()
                                    .id(id("relayed-toggle"))
                                    .flex()
                                    .items_center()
                                    .gap(space::SM)
                                    .cursor_pointer()
                                    .child(from)
                                    .child(
                                        icon(Icon::Chevron)
                                            .size(px(12.))
                                            .flex_none()
                                            .text_color(p.faint)
                                            .with_transformation(Transformation::rotate(radians(
                                                -FRAC_PI_2 * turn,
                                            ))),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open = !this.open;
                                        cx.notify();
                                    })),
                                move |d, t| d.text_color(mix(p.faint, p.mute, t)),
                            ))
                            .child(presence(
                                id("relayed-line"),
                                !self.open,
                                div()
                                    .id(id("relayed-first"))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(13.5))
                                    .text_color(p.mute)
                                    .aria_label(first.clone())
                                    .child(first)
                                    .test_support(),
                            ))
                            .child(presence(
                                id("relayed-body"),
                                self.open,
                                div()
                                    .id(id("relayed-text"))
                                    .child(markdown_view(
                                        id("relayed-region"),
                                        &self.markdown,
                                        self.scope,
                                    ))
                                    .test_support(),
                            ))
                    }
                };
                let relayed = div()
                    .id(id("relayed"))
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(bubble)
                    .test_support();
                said(
                    format!("row-{}", self.at),
                    relayed,
                    false,
                    self.when,
                    text.clone(),
                    p,
                )
                .into_any_element()
            }
            Block::Thought { text, since, until } => {
                let (label, timer) = self.thinking.clone().expect("a thought has its timer");
                let timed = until.is_none() || timed(*since, *until);
                // A thought the provider sent no text of says how long it took, and opens on
                // nothing.
                let said = !text.trim().is_empty();
                let head = div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .text_size(px(13.))
                    .text_color(p.faint)
                    .child(label)
                    .children(timed.then_some(timer));
                let turn = turning(id("thought-toggle"), self.open, window, cx);
                let head = match said {
                    true => hoverable(
                        head.id(id("thought-toggle"))
                            .cursor_pointer()
                            .child(
                                icon(Icon::Chevron)
                                    .size(px(12.))
                                    .text_color(p.faint)
                                    .with_transformation(Transformation::rotate(radians(
                                        -FRAC_PI_2 * turn,
                                    ))),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open = !this.open;
                                cx.notify();
                            })),
                        move |d, t| d.text_color(mix(p.faint, p.mute, t)),
                    )
                    .into_any_element(),
                    false => head.into_any_element(),
                };
                div()
                    .id(id("thought"))
                    .flex()
                    .flex_col()
                    .child(head)
                    .child(presence(
                        id("thought-body"),
                        said && self.open,
                        div()
                            .id(id("thought-text"))
                            .mt(px(8.))
                            .ml(px(6.))
                            .pl(space::LG)
                            .border_l_1()
                            .border_color(p.line)
                            .text_size(px(13.))
                            .text_color(p.mute)
                            .child(markdown_view(
                                id("thought-region"),
                                &self.markdown,
                                self.scope,
                            ))
                            .test_support(),
                    ))
                    .test_support()
                    .into_any_element()
            }
            Block::Text { .. } => {
                let author = self.author().map(|me| {
                    let letter = me.chars().next().unwrap_or('?').to_uppercase().to_string();
                    (
                        SharedString::from(me.to_owned()),
                        SharedString::from(letter),
                    )
                });
                let reply = div()
                    .id(id("text"))
                    .max_w(px(680.))
                    .when_some(author, |d, (me, letter)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::SM)
                                .mb(px(5.))
                                .mono(11.5)
                                .text_color(p.faint)
                                .child(
                                    div()
                                        .size(px(20.))
                                        .rounded(px(6.))
                                        .bg(p.text)
                                        .text_color(p.bg)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .mono(10.)
                                        .semibold()
                                        .child(letter),
                                )
                                .child(div().id(id("author")).child(me).test_support()),
                        )
                    })
                    .child(markdown_view(
                        id("text-region"),
                        &self.markdown[..self.markdown.len().min(1)],
                        self.scope,
                    ))
                    .test_support();
                // A long reply's other pieces are items of the list: the last one carries it.
                match self.markdown.len() > 1 {
                    true => reply.into_any_element(),
                    false => self.said_text(reply, p).into_any_element(),
                }
            }
            // The calls that show a link are the card of links': the steps say the others.
            Block::Steps { .. } if is_hidden(&block) => div().into_any_element(),
            Block::Steps { calls } => {
                let running = calls
                    .iter()
                    .filter(|c| !c.hidden() && c.state == CallState::Running)
                    .count();
                let counted = calls.iter().filter(|c| !c.hidden()).count();
                let done = counted - running;
                let meta = match running {
                    0 => format!("{done} of {counted}"),
                    _ => "running".to_owned(),
                };
                let rows = calls
                    .iter()
                    .zip(&self.calls)
                    .enumerate()
                    .filter(|(_, (call, _))| !call.hidden())
                    .map(|(at, (call, (_, mark)))| {
                        div()
                            .id(ElementId::Name(format!("step-{}-{at}", self.at).into()))
                            .flex()
                            .gap(space::MD)
                            .px(space::XS)
                            .py(space::SM)
                            .child(mark.clone())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(space::SM)
                                            .mono(12.5)
                                            .child(SharedString::from(call.name.clone()))
                                            .children(call.allowed_by.as_ref().map(|by| {
                                                let said =
                                                    SharedString::from(format!("allowed by {by}"));
                                                div()
                                                    .id(ElementId::Name(
                                                        format!("step-{}-{at}-allowed", self.at)
                                                            .into(),
                                                    ))
                                                    .text_size(px(11.5))
                                                    .text_color(p.faint)
                                                    .aria_label(said.clone())
                                                    .child(said)
                                                    .test_support()
                                            }))
                                            .when(call.state == CallState::Interrupted, |d| {
                                                d.child(
                                                    div()
                                                        .mono(10.)
                                                        .medium()
                                                        .text_color(p.faint)
                                                        .child("INTERRUPTED"),
                                                )
                                            }),
                                    )
                                    .child(match &call.path {
                                        Some(path) => div()
                                            .text_color(p.faint)
                                            .child(path_line(
                                                ElementId::Name(
                                                    format!("step-{}-{at}-path", self.at).into(),
                                                ),
                                                path,
                                                11.5,
                                                window,
                                                cx,
                                            ))
                                            .into_any_element(),
                                        None => tipped_when_cut(
                                            div()
                                                .id(ElementId::Name(
                                                    format!("step-{}-{at}-args", self.at).into(),
                                                ))
                                                .mono(11.5)
                                                .text_color(p.faint)
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(SharedString::from(call.args.clone())),
                                            SharedString::from(call.args.clone()),
                                            window,
                                            cx,
                                        ),
                                    })
                                    .child(presence(
                                        ElementId::Name(
                                            format!("step-{}-{at}-cut-line", self.at).into(),
                                        ),
                                        call.cut.is_some(),
                                        cut_said(
                                            ElementId::Name(
                                                format!("step-{}-{at}-cut", self.at).into(),
                                            ),
                                            call.cut,
                                            p.faint,
                                        ),
                                    )),
                            )
                            .children(call.path.as_ref().map(|path| {
                                copy_path(
                                    ElementId::Name(format!("step-{}-{at}-copy", self.at).into()),
                                    path,
                                )
                            }))
                            .child(div().flex_none().mono(11.).text_color(p.faint).child(
                                SharedString::from(call.duration_ms().map_or("…".into(), seconds)),
                            ))
                            .test_support()
                    })
                    .collect::<Vec<_>>();
                let toggle = || {
                    cx.listener(|this, _, _, cx| {
                        this.open = !this.open;
                        cx.notify();
                    })
                };
                let chevron = |turn: f32| {
                    icon(Icon::Chevron)
                        .size(px(12.))
                        .text_color(p.faint)
                        .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * turn)))
                };
                // Folded to one discreet line until a click opens it: a long run of calls
                // would push the conversation down. The line and the card fold into each other.
                let said: Vec<&Call> = calls.iter().filter(|call| !call.hidden()).collect();
                let last = said.last().expect("a block of steps holds a call");
                let count = said.len();
                let line = SharedString::from(format!(
                    "{count} {} · {}{}",
                    if count == 1 { "step" } else { "steps" },
                    last.name,
                    allowed_said(calls)
                ));
                let folded = hoverable(
                    div()
                        .id(id("steps-toggle"))
                        .flex()
                        .items_center()
                        .gap(space::SM)
                        .py(space::XS)
                        .cursor_pointer()
                        .mono(12.)
                        .child(chevron(1.))
                        .aria_label(line.clone())
                        .child(changing(id("steps-said"), line, |line| {
                            div().child(line).into_any_element()
                        }))
                        .on_click(toggle()),
                    move |d, t| d.text_color(mix(p.faint, p.mute, t)),
                );
                let card = Card::new()
                    .title(hoverable(
                        div()
                            .id(id("steps-toggle"))
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .cursor_pointer()
                            .child("Agent steps")
                            .child(chevron(0.))
                            .on_click(toggle()),
                        move |d, t| d.text_color(mix(p.mute, p.text, t)),
                    ))
                    .meta(meta)
                    .child(div().px(space::MD).py(space::SM).children(rows));
                div()
                    .id(id("steps"))
                    .child(presence(id("steps-folded"), !self.open, folded))
                    .child(presence(id("steps-open"), self.open, card))
                    .test_support()
                    .into_any_element()
            }
            Block::SubAgents => {
                let by = self.label().unwrap_or_default();
                for agent in &self.sub_agents {
                    if !self.icons.contains_key(&agent.id) {
                        let status = agent.status.clone();
                        let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
                        let pill =
                            cx.new(|cx| Pill::new(status, cx).quiet_running().quiet_needs_you());
                        self.icons.insert(agent.id.clone(), icon);
                        self.pills.insert(agent.id.clone(), pill);
                    }
                }
                let rows = self.sub_agents.iter().map(|agent| {
                    let target = agent.id.clone();
                    let page = self.page.clone();
                    ListRow::new(
                        ElementId::Name(format!("sub-agent-{}", agent.id).into()),
                        SharedString::from(agent.name.clone()),
                    )
                    .leading(self.icons[&agent.id].clone())
                    .detail(SharedString::from(agent.runs_on.clone()))
                    .meta(SharedString::from(agent.doing.clone()))
                    .trailing(self.pills[&agent.id].clone())
                    .on_click(move |_, _, cx| {
                        if let Some(page) = page.upgrade() {
                            page.update(cx, |page, cx| page.select(&target, cx));
                        }
                    })
                });
                // Folded to one line that counts them until a click opens the list: twenty
                // sub-agents would fill several screens of the conversation.
                let turn = turning(id("sub-agents-toggle"), self.open, window, cx);
                let (counts, asks) = sub_agents_said(&self.sub_agents, cx);
                let line = match asks {
                    0 => counts.clone(),
                    1 => format!("{counts} · 1 needs you"),
                    n => format!("{counts} · {n} need you"),
                };
                let (open, needs_id) = (self.open, id("sub-agents-needs"));
                let shown = SharedString::from(match open {
                    true => "Sub-agents".to_owned(),
                    false => line.clone(),
                });
                let (wait, _) = AgentState::NeedsYou.colors(&p);
                let title =
                    hoverable(
                        div()
                            .id(id("sub-agents-toggle"))
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .cursor_pointer()
                            .aria_label(SharedString::from(line))
                            .aria_expanded(open)
                            .child(changing(id("sub-agents-said"), shown, move |shown| {
                                match open {
                                    true => div().child(shown),
                                    false => div().flex().gap(space::XS).child(counts).when(
                                        asks > 0,
                                        |d| {
                                            d.child(
                                                div()
                                                    .id(needs_id)
                                                    .text_color(wait)
                                                    .child(match asks {
                                                        1 => "· 1 needs you".to_owned(),
                                                        n => format!("· {n} need you"),
                                                    })
                                                    .test_support(),
                                            )
                                        },
                                    ),
                                }
                                .into_any_element()
                            }))
                            .child(
                                icon(Icon::Chevron)
                                    .size(px(12.))
                                    .text_color(p.faint)
                                    .with_transformation(Transformation::rotate(radians(
                                        -FRAC_PI_2 * turn,
                                    ))),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open = !this.open;
                                cx.notify();
                            })),
                        move |d, t| d.text_color(mix(p.mute, p.text, t)),
                    );
                div()
                    .id(id("sub-agents"))
                    .child(
                        Card::new()
                            .id(id("sub-agents-card"))
                            .title(title)
                            .meta(by)
                            .block(
                                CardBlock::new()
                                    .present(id("sub-agents-list"), open)
                                    .child(div().p(space::XS).children(rows)),
                            ),
                    )
                    .test_support()
                    .into_any_element()
            }
            // Folded to its one line until a click opens it.
            Block::Report { .. } => {
                let turn = turning(id("report-toggle"), self.open, window, cx);
                div()
                    .id(id("report"))
                    .child(
                        Card::new()
                            .title(hoverable(
                                div()
                                    .id(id("report-toggle"))
                                    .flex()
                                    .items_center()
                                    .gap(space::SM)
                                    .cursor_pointer()
                                    .child(SharedString::from(self.label().unwrap_or_default()))
                                    .child(
                                        icon(Icon::Chevron)
                                            .size(px(12.))
                                            .text_color(p.faint)
                                            .with_transformation(Transformation::rotate(radians(
                                                -FRAC_PI_2 * turn,
                                            ))),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open = !this.open;
                                        cx.notify();
                                    })),
                                move |d, t| d.text_color(mix(p.mute, p.text, t)),
                            ))
                            .child(presence(
                                id("report-body"),
                                self.open,
                                div()
                                    .id(id("report-text"))
                                    .px(space::LG)
                                    .pt(space::MD)
                                    .pb(space::MD)
                                    .child(markdown_view(
                                        id("report-region"),
                                        &self.markdown,
                                        self.scope,
                                    ))
                                    .test_support(),
                            )),
                    )
                    .test_support()
                    .into_any_element()
            }
            // The links the agent handed the user in a row: one card, a line each with its title,
            // where it leads and Open (a thumbnail for a picture), so that a toast missed is not
            // lost. Nothing opens until the click.
            Block::Links { links } => {
                let page = self.page.clone();
                let open: Open = Rc::new(move |target, cx| {
                    let target = target.to_owned();
                    page.update(cx, |page, cx| {
                        crate::open::shown(&page.follow.opener, &target, cx)
                    })
                    .ok();
                });
                self.links.draw(self.at, links, &open, cx)
            }
            // The agent's own end is shown whole: a failure's reason, what was not done and why.
            Block::Notice { kind, text } if !is_delivery(kind) => div()
                .id(id("notice"))
                .flex()
                .gap(space::SM)
                .mono(12.)
                .text_color(p.faint)
                .aria_label(SharedString::from(text.clone()))
                .child(div().flex_none().child(SharedString::from(kind.clone())))
                .child(div().min_w_0().child(SharedString::from(text.clone())))
                .test_support()
                .into_any_element(),
            // A delivery is one line, its kind and its first words, its whole text folded under
            // it until a click opens it; what woke the agent is its own line, the forge's text
            // folded under it.
            Block::Notice { kind, text } => {
                let (first, rest) = match (kind == WOKEN_BY_KIND, text.split_once('\n')) {
                    (true, Some((first, rest))) => (first.trim().to_owned(), rest.trim()),
                    (true, None) => (text.trim().to_owned(), ""),
                    (false, _) => (
                        text.split_whitespace().collect::<Vec<_>>().join(" "),
                        text.trim(),
                    ),
                };
                let summary = SharedString::from(first);
                let detail = SharedString::from(rest.to_owned());
                let foldable = !rest.is_empty();
                let turn = turning(id("notice-toggle"), self.open, window, cx);
                let head = div()
                    .id(id("notice-toggle"))
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .mono(12.)
                    .when(foldable, |d| d.cursor_pointer())
                    .child(div().flex_none().child(SharedString::from(kind.clone())))
                    .child(
                        div()
                            .id(id("notice-summary"))
                            .min_w_0()
                            .when(foldable, |d| {
                                d.overflow_hidden().whitespace_nowrap().text_ellipsis()
                            })
                            .aria_label(summary.clone())
                            .child(summary)
                            .test_support(),
                    )
                    .when(foldable, |d| {
                        d.child(
                            icon(Icon::Chevron)
                                .size(px(12.))
                                .flex_none()
                                .text_color(p.faint)
                                .with_transformation(Transformation::rotate(radians(
                                    -FRAC_PI_2 * turn,
                                ))),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open = !this.open;
                            cx.notify();
                        }))
                    });
                div()
                    .id(id("notice"))
                    .flex()
                    .flex_col()
                    .text_color(p.faint)
                    .child(hoverable(head, move |d, t| {
                        d.text_color(mix(p.faint, p.mute, t * f32::from(u8::from(foldable))))
                    }))
                    .child(presence(
                        id("notice-body"),
                        foldable && self.open,
                        div()
                            .id(id("notice-text"))
                            .pt(space::XS)
                            .mono(12.)
                            .whitespace_normal()
                            .aria_label(detail.clone())
                            .child(detail)
                            .test_support(),
                    ))
                    .test_support()
                    .into_any_element()
            }
            // Folded to its path and its count until a click opens it.
            Block::Diff { path, lines } => {
                let added = lines
                    .iter()
                    .filter(|l| matches!(l, DiffLine::Added(_)))
                    .count();
                let removed = lines.len() - added;
                let turn = turning(id("diff-toggle"), self.open, window, cx);
                div()
                    .id(id("diff"))
                    .child(
                        Card::new()
                            .id(id("diff-card"))
                            .title(hoverable(
                                div()
                                    .id(id("diff-toggle"))
                                    .flex()
                                    .items_center()
                                    .gap(space::SM)
                                    .flex_1()
                                    .min_w_0()
                                    .cursor_pointer()
                                    .child(icon(Icon::File).flex_none().text_color(p.faint))
                                    .child(path_line(id("diff-path"), path, 12.5, window, cx))
                                    .child(copy_path(id("diff-copy"), path))
                                    .child(
                                        div()
                                            .id(id("diff-chevron"))
                                            .flex_none()
                                            .ml_auto()
                                            .child(
                                                icon(Icon::Chevron)
                                                    .size(px(12.))
                                                    .text_color(p.faint)
                                                    .with_transformation(Transformation::rotate(
                                                        radians(-FRAC_PI_2 * turn),
                                                    )),
                                            )
                                            .test_support(),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open = !this.open;
                                        cx.notify();
                                    })),
                                move |d, t| d.text_color(mix(p.mute, p.text, t)),
                            ))
                            .meta(format!("+{added} −{removed}"))
                            .child(presence(
                                id("diff-body"),
                                self.open,
                                div()
                                    .id(id("diff-lines"))
                                    .py(space::SM)
                                    .mono(12.)
                                    .children(lines.iter().enumerate().map(|(at, line)| {
                                        let (text, bg, ink) = match line {
                                            DiffLine::Removed(text) => {
                                                (format!("- {text}"), p.del, p.fail)
                                            }
                                            DiffLine::Added(text) => {
                                                (format!("+ {text}"), p.add, p.ok)
                                            }
                                        };
                                        let text = SharedString::from(text);
                                        tipped_when_cut(
                                            div()
                                                .id(id(&format!("diff-line-{at}")))
                                                .px(space::LG)
                                                .bg(bg)
                                                .text_color(ink)
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(text.clone()),
                                            text,
                                            window,
                                            cx,
                                        )
                                    }))
                                    .test_support(),
                            )),
                    )
                    .test_support()
                    .into_any_element()
            }
            Block::Approval { number } => self.render_card(*number, window, cx).into_any_element(),
            Block::Question { seq, .. } => self.render_question(*seq, window, cx),
            Block::Answered { .. } => div().into_any_element(),
            // One line; a click opens the output in the panel the rail opens.
            Block::Command(result) => {
                let (page, run) = (self.page.clone(), result.run.clone());
                let open = move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                    page.update(cx, |page, cx| page.open_command(&run, cx)).ok();
                };
                runs::card(result, self.at, open, cx)
            }
        }
    }
}

impl Row {
    /// The reply with its time and Copy under it, its whole text copied.
    pub(crate) fn said_text(&self, content: impl IntoElement, p: Palette) -> impl IntoElement {
        let text = match &*self.block {
            Block::Text { text } => text.clone(),
            _ => String::new(),
        };
        said(
            format!("row-{}", self.at),
            content,
            false,
            self.when,
            text,
            p,
        )
    }

    /// A request in the conversation is one neutral line where it happened, "Request #1 · Read
    /// .env", which lights its card in the rail; once answered, the record of the outcome. The
    /// card, with Allow and Deny, is in the rail: the page says that the user is needed there,
    /// once.
    fn render_card(
        &mut self,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let Some(card) = self.card.clone() else {
            return div()
                .id(ElementId::Name(format!("approval-line-{number}").into()))
                .into_any_element();
        };
        let record = card.record();
        let said = match &card.asked {
            Some(asked) => format!("Request #{number} · {asked}"),
            None => format!("Request #{number}"),
        };
        let page = self.page.clone();
        let ink = if card.base { p.fail } else { p.wait };
        let line = div()
            .id(ElementId::Name(format!("approval-line-{number}").into()))
            .h(px(34.))
            .flex()
            .items_center()
            .gap(space::SM)
            .px(space::MD)
            .rounded(px(9.))
            .border_1()
            .cursor_pointer()
            .text_size(px(13.))
            .aria_label(SharedString::from(said.clone()))
            .child(div().flex_none().size(px(8.)).rounded_full().bg(ink))
            .child(tipped_when_cut(
                div()
                    .id(ElementId::Name(
                        format!("approval-line-{number}-said").into(),
                    ))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(said.clone())),
                SharedString::from(said),
                window,
                cx,
            ))
            .child(
                div()
                    .flex_none()
                    .mono(11.)
                    .text_color(p.faint)
                    .child("in the rail →"),
            )
            .on_click(move |_, _, cx| {
                page.update(cx, |page, cx| page.reveal_waiting(number, cx))
                    .ok();
            });
        let edge = if card.base { p.fail } else { p.wait_line };
        let waiting = hoverable(line, move |d, t| {
            d.bg(p.hover.opacity(t))
                .border_color(mix(p.line, edge, t))
                .text_color(mix(p.mute, p.text, t))
        });
        let outcome = div()
            .id(ElementId::Name(format!("approval-record-{number}").into()))
            .aria_label(SharedString::from(record.clone().unwrap_or_default()))
            .mono(12.)
            .text_color(p.faint)
            .child(SharedString::from(record.clone().unwrap_or_default()))
            .test_support();
        // The waiting line folds away and the record unfolds in its place.
        div()
            .id(ElementId::Name(format!("approval-{number}-row").into()))
            .child(presence(
                ElementId::Name(format!("approval-waiting-{number}").into()),
                record.is_none(),
                waiting,
            ))
            .child(presence(
                ElementId::Name(format!("approval-decided-{number}").into()),
                record.is_some(),
                outcome,
            ))
            .into_any_element()
    }
}

impl Row {
    /// A question the agent asked the user, in full, with the field that answers it: it unfolds
    /// where it was asked while the daemon says it waits, and folds away once the answer is
    /// taken.
    fn render_question(
        &mut self,
        seq: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = |part: &str| ElementId::Name(format!("row-{}-{part}", self.at).into());
        let answer = match &self.answer {
            Some(answer) => answer.clone(),
            None => {
                let page = self.page.clone();
                let (session, open) = page
                    .upgrade()
                    .map(|page| {
                        let page = page.read(cx);
                        (page.id().to_owned(), page.open_questions())
                    })
                    .unwrap_or_default();
                let answer =
                    cx.new(|cx| AnswerBox::new(self.at, seq, session, open, page, window, cx));
                self.answer = Some(answer.clone());
                answer
            }
        };
        let card = Card::new()
            .title(SharedString::from(format!("{} asks you", self.me)))
            .meta(self.when.map(clock).unwrap_or_default())
            .child(
                div()
                    .id(id("question-text"))
                    .px(space::LG)
                    .pt(space::MD)
                    .child(markdown_view(
                        id("question-region"),
                        &self.markdown,
                        self.scope,
                    ))
                    .test_support(),
            )
            .child(div().px(space::LG).py(space::MD).child(answer));
        div()
            .id(id("question"))
            .child(presence(id("question-open"), self.asking, card))
            .test_support()
            .into_any_element()
    }
}

/// What a question's card says once its answer was sent while the agent works: a message of the
/// user waits for the end of the agent's turn (or "Send now" under the conversation), and the
/// card folds away once the agent took it.
pub const ANSWER_WAITS: &str =
    "Your answer is sent: it waits under the conversation until the agent's turn ends.";

/// What each card says while several questions wait: a message of the user answers them all.
pub const ANSWERS_ALL: &str =
    "Several questions wait: your answer goes to all of them, and quotes each.";

/// The questions whose answer the user sent, by session: the window keeps them, so that a page
/// opened again, or a history read again, does not ask for an answer twice.
#[derive(Default)]
struct SentAnswers(HashSet<(String, u64)>);

impl gpui_kit::Global for SentAnswers {}

/// The answer to the questions `seqs` of `session` was taken by the daemon.
pub fn answers_sent(session: &str, seqs: &[u64], cx: &mut App) {
    let sent = &mut cx.default_global::<SentAnswers>().0;
    sent.extend(seqs.iter().map(|&seq| (session.to_owned(), seq)));
}

/// The field that answers a question, in its own view: its caret blinks without drawing the row.
pub struct AnswerBox {
    at: usize,
    seq: u64,
    session: String,
    /// How many questions of the session wait: its answer goes to all of them.
    open: usize,
    page: WeakEntity<SessionPage>,
    field: Entity<InputState>,
    has_text: bool,
    sending: bool,
    _subscriptions: Vec<Subscription>,
}

impl AnswerBox {
    fn new(
        at: usize,
        seq: u64,
        session: String,
        open: usize,
        page: WeakEntity<SessionPage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = InputState::new(window, cx).placeholder("Your answer");
            follow_theme(&mut field, cx);
            field
        });
        let mut subscriptions = vec![cx.subscribe_in(
            &field,
            window,
            |this: &mut Self, field, event, _, cx| match event {
                InputEvent::Change => {
                    this.has_text = !field.read(cx).value().trim().is_empty();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.send(cx),
                _ => {}
            },
        )];
        subscriptions.push(cx.observe_global::<SentAnswers>(|_, cx| cx.notify()));
        AnswerBox {
            at,
            seq,
            session,
            open,
            page,
            field,
            has_text: false,
            sending: false,
            _subscriptions: subscriptions,
        }
    }

    /// How many questions of the session wait now.
    fn set_open(&mut self, open: usize, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            cx.notify();
        }
    }

    pub fn field(&self) -> &Entity<InputState> {
        &self.field
    }

    /// Whether the answer to its question went: the field gives way to a line saying it waits.
    pub fn is_sent(&self, cx: &App) -> bool {
        cx.try_global::<SentAnswers>()
            .is_some_and(|sent| sent.0.contains(&(self.session.clone(), self.seq)))
    }

    /// Sends what the field holds to the root, as a message.
    pub fn send(&mut self, cx: &mut Context<Self>) {
        let typed = self.field.read(cx).value().trim().to_owned();
        if typed.is_empty() || self.sending {
            return;
        }
        self.sending = true;
        let seq = self.seq;
        self.page
            .update(cx, |page, cx| page.answer_question(seq, typed, cx))
            .ok();
        cx.notify();
    }

    fn answered(&mut self, refused: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.sending = false;
        match refused {
            None => {
                self.has_text = false;
                self.field
                    .update(cx, |field, cx| field.set_value("", window, cx));
            }
            Some(why) => toast(Toast::failure("The answer was not sent").detail(why), cx),
        }
        cx.notify();
    }
}

impl Render for AnswerBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("row-{}-{part}", self.at).into());
        let frame = div()
            .id(id("answer-frame"))
            .h(px(34.))
            .px(space::MD)
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .flex()
            .items_center()
            .text_size(px(13.))
            .child(Input::new(&self.field))
            .test_support();
        let send = Button::primary(id("answer-send"), "Answer")
            .disabled(!self.has_text || self.sending)
            .on_click(cx.listener(|this, _, _, cx| this.send(cx)));
        let field = Field::new(
            id("answer-field"),
            "Your answer",
            div()
                .flex()
                .items_center()
                .gap(space::SM)
                .child(div().flex_1().min_w_0().child(frame))
                .child(send),
        );
        let all = div()
            .id(id("answer-all"))
            .pb(space::SM)
            .text_size(px(12.5))
            .text_color(p.mute)
            .aria_label(ANSWERS_ALL)
            .child(ANSWERS_ALL)
            .test_support();
        let waits = div()
            .id(id("answer-waits"))
            .text_size(px(12.5))
            .text_color(p.faint)
            .aria_label(ANSWER_WAITS)
            .child(ANSWER_WAITS)
            .test_support();
        let sent = self.is_sent(cx);
        div()
            .child(presence(id("answer-several"), self.open > 1 && !sent, all))
            .child(presence(id("answer-open"), !sent, field))
            .child(presence(id("answer-sent"), sent, waits))
    }
}

/// Why a base card comes back whatever the mode, said on it.
pub const BASE_SAYS: &str = "Flottille always asks you for this, in Ask and in Auto: it may lose \
                             work or reach what must stay private, so no judge decides it.";

/// Why the first message of a session to another always comes back, said on its card instead of
/// `BASE_SAYS`: what it asks is a lasting leave, in every mode.
pub const LEAVE_SAYS: &str = "Flottille always asks you this, in every mode: allowing it lets \
                              this session write to that one until either ends or you withdraw \
                              it.";

/// The time a thought took, or has taken so far: it ticks only while the thought runs.
pub struct ThinkingTimer {
    since: u64,
    until: Option<u64>,
    _ticker: Option<Task<()>>,
}

/// A thought shorter than this has no measure worth showing (`0.0s`): it says no time.
const SHORTEST_THOUGHT: u64 = 100;

/// Whether a thought that ran from `since` to `until` (still running when `None`) says its time.
fn timed(since: u64, until: Option<u64>) -> bool {
    until.is_none_or(|until| until.saturating_sub(since) >= SHORTEST_THOUGHT)
}

/// How often a running thought's time is drawn again.
const TICK: Duration = Duration::from_millis(100);

impl ThinkingTimer {
    fn new(since: u64, until: Option<u64>, cx: &mut Context<Self>) -> Self {
        let mut timer = Self {
            since,
            until: None,
            _ticker: None,
        };
        timer.end(until, cx);
        timer
    }

    fn end(&mut self, until: Option<u64>, cx: &mut Context<Self>) {
        self.until = until;
        self._ticker = until.is_none().then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
        });
        cx.notify();
    }

    pub fn is_ticking(&self) -> bool {
        self._ticker.is_some()
    }

    /// The time the thought took, or has taken so far.
    pub fn time(&self) -> String {
        let until = self.until.unwrap_or_else(now_ms);
        seconds(until.saturating_sub(self.since))
    }
}

impl Render for ThinkingTimer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().mono(12.).child(SharedString::from(self.time()))
    }
}

/// An agent's row in the rail.
#[derive(Clone, Debug, PartialEq)]
pub struct RailAgent {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    /// Its provider and model, as a line.
    pub runs_on: String,
    pub depth: u64,
    /// The agent that launched it; `None` for the root.
    pub parent: Option<String>,
    /// A child that completed or was stopped: the rail gathers those under one folded row.
    pub ended: bool,
    /// How much of its context it used, in percent.
    pub context: Option<u64>,
    /// The tokens its context holds, when the window's size is not known yet.
    pub context_tokens: Option<u64>,
    /// What it does, in a few words.
    pub doing: String,
    /// It waits between turns for what happens outside (`doing` says what).
    pub waits_on_subscription: bool,
    pub status: Entity<Status>,
}

/// A row of the rail: an agent, or the folded row that gathers the ended children of `parent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RailRow {
    Agent(String),
    Ended {
        parent: String,
        /// The agents behind the row, the ended children's own ended children included.
        count: usize,
        open: bool,
    },
}

/// What [`rail_rows`] needs of an agent.
struct Node<'a> {
    id: &'a str,
    parent: Option<&'a str>,
    ended: bool,
}

/// The rail's rows, in tree order. A child that ended is left out and counted in the "ended" row
/// that closes its parent's children, unless one of its descendants has not ended: that one
/// would be hidden with it. Those behind an open row follow it, in tree order.
fn rail_rows(nodes: &[Node], open: &HashSet<String>) -> Vec<RailRow> {
    fn children<'a, 'b>(
        nodes: &'b [Node<'a>],
        id: &'b str,
    ) -> impl Iterator<Item = &'b Node<'a>> + use<'a, 'b> {
        nodes.iter().filter(move |n| n.parent == Some(id))
    }
    fn all_ended(nodes: &[Node], id: &str) -> bool {
        children(nodes, id).all(|n| n.ended && all_ended(nodes, n.id))
    }
    fn descendants<'a>(nodes: &[Node<'a>], id: &str, out: &mut Vec<&'a str>) {
        for node in children(nodes, id) {
            out.push(node.id);
            descendants(nodes, node.id, out);
        }
    }
    fn walk(nodes: &[Node], open: &HashSet<String>, parent: &str, rows: &mut Vec<RailRow>) {
        let mut gathered: Vec<&str> = Vec::new();
        for child in children(nodes, parent) {
            if child.ended && all_ended(nodes, child.id) {
                gathered.push(child.id);
                descendants(nodes, child.id, &mut gathered);
            } else {
                rows.push(RailRow::Agent(child.id.to_owned()));
                walk(nodes, open, child.id, rows);
            }
        }
        if gathered.is_empty() {
            return;
        }
        let open = open.contains(parent);
        rows.push(RailRow::Ended {
            parent: parent.to_owned(),
            count: gathered.len(),
            open,
        });
        if open {
            // `gathered` is already in tree order: each agent, then its own descendants.
            rows.extend(gathered.into_iter().map(|id| RailRow::Agent(id.to_owned())));
        }
    }
    let known: HashSet<&str> = nodes.iter().map(|n| n.id).collect();
    let mut rows = Vec::new();
    for top in nodes
        .iter()
        .filter(|n| n.parent.is_none_or(|p| !known.contains(p)))
    {
        rows.push(RailRow::Agent(top.id.to_owned()));
        walk(nodes, open, top.id, &mut rows);
    }
    rows
}

/// A count of tokens as the row says it: `812`, `45k`, `1.2M`.
fn tokens(count: u64) -> String {
    match count {
        0..1000 => count.to_string(),
        1000..1_000_000 => format!("{}k", count / 1000),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

/// The rail: the session's agents, its workspace and its facts.
pub struct SessionRail {
    page: WeakEntity<SessionPage>,
    agents: Vec<RailAgent>,
    marks: HashMap<String, Entity<StateIcon>>,
    /// The parents whose "ended" row is open: kept while the window shows the session.
    open_ended: HashSet<String>,
    /// How far each "ended" row's children are unfolded: they stay drawn until a fold ends.
    ended_folds: HashMap<String, EndedFold>,
    /// How far the bar of each agent's gauge is filled, following the use.
    gauges: HashMap<String, Motion>,
    /// How many task runs each agent last had going: its row says it while the count folds away.
    last_runs: HashMap<String, usize>,
    selected: String,
    workspace: Option<String>,
    project: String,
    place: Option<PlaceFacts>,
    /// The daemon answered for the place (with facts, or with none): until then the Workspace
    /// section waits as a skeleton.
    place_known: bool,
    /// The "⋯" menu of the WORKSPACE section.
    menu: Entity<Menu>,
    /// The "⋯" menu of the root's row: it ends the session.
    agent_menu: Entity<Menu>,
    /// The commands of the session's tree.
    commands: Entity<CommandList>,
    /// What the agents of the session's tree wait for outside Flottille.
    subscriptions: Entity<SubscriptionList>,
    /// The leaves that let the session write to another, or another to write to it.
    leaves: Entity<LeaveList>,
    /// The requests that wait for the user, pinned at the rail's bottom.
    waiting: Entity<Waiting>,
    /// The scroll of the sections above what waits.
    scroll: ScrollHandle,
    /// Where a ticket is typed to link it, made when the section first shows a workspace.
    ticket_field: Option<Entity<InputState>>,
    /// The user chose "Link a ticket…": the field shows under the Ticket line.
    linking: bool,
    /// What the daemon said of the ticket typed.
    ticket_error: Option<SharedString>,
    /// The last ticket the place had, drawn while its line folds away after an unlink.
    last_ticket: Option<TicketFacts>,
    _ticket_subscription: Option<Subscription>,
    renders: usize,
}

impl SessionRail {
    fn new(
        page: WeakEntity<SessionPage>,
        root: String,
        opened: &Opened,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = page.clone();
        let ender = page.clone();
        let reloader = page.clone();
        let agent_menu = cx.new(|cx| {
            Menu::new("rail-agent-menu", "Session actions", cx)
                .entry(
                    "rail-agent-reload-tools",
                    RELOAD_TOOLS,
                    Some(Icon::Restart),
                    move |_, cx| {
                        reloader.update(cx, |page, cx| page.reload_tools(cx)).ok();
                    },
                )
                .entry(
                    "rail-agent-end",
                    "End session…",
                    Some(Icon::Stop),
                    move |_, cx| {
                        ender.update(cx, |page, cx| page.ask_end(cx)).ok();
                    },
                )
        });
        let rail = cx.entity().downgrade();
        let unlinks = rail.clone();
        let menu = cx.new(|cx| {
            workspace_menu(
                "workspace-menu",
                WorkspaceActions {
                    open_in_editor: Box::new(move |cx| {
                        editor.update(cx, |page, _| page.open_in_editor()).ok();
                    }),
                    link_ticket: Some(Box::new(move |cx| {
                        rail.update(cx, |rail, cx| rail.start_linking(cx)).ok();
                    })),
                    unlink_ticket: Some(Box::new(move |cx| {
                        unlinks
                            .update(cx, |rail, cx| rail.link_ticket(None, cx))
                            .ok();
                    })),
                    remove: None,
                },
                cx,
            )
        });
        let commands = cx.new(|_| CommandList::new(page.clone()).of_agent(root.clone()));
        let subscriptions = cx.new(|_| SubscriptionList::new(page.clone()));
        let leaves = cx.new(|_| LeaveList::new(page.clone(), root.clone()));
        let waiting = cx.new(|cx| Waiting::new(page.clone(), cx));
        Self {
            page,
            commands,
            subscriptions,
            leaves,
            waiting,
            scroll: ScrollHandle::new(),
            agents: Vec::new(),
            marks: HashMap::new(),
            open_ended: HashSet::new(),
            ended_folds: HashMap::new(),
            gauges: HashMap::new(),
            last_runs: HashMap::new(),
            menu,
            agent_menu,
            selected: root,
            workspace: opened.place.clone(),
            project: opened.project.clone(),
            place: None,
            place_known: false,
            ticket_field: None,
            linking: false,
            ticket_error: None,
            last_ticket: None,
            _ticket_subscription: None,
            renders: 0,
        }
    }

    /// The field a ticket is typed in.
    pub fn ticket_field(&self) -> Option<&Entity<InputState>> {
        self.ticket_field.as_ref()
    }

    pub fn ticket_error(&self) -> Option<&str> {
        self.ticket_error.as_deref()
    }

    /// The user chose "Link a ticket…": the field shows, for the ticket to be typed.
    fn start_linking(&mut self, cx: &mut Context<Self>) {
        self.linking = true;
        cx.notify();
    }

    /// Links the workspace to the ticket typed, or to none (`None`).
    fn link_ticket(&mut self, ticket: Option<String>, cx: &mut Context<Self>) {
        let Some(name) = self.workspace.clone() else {
            return;
        };
        self.ticket_error = None;
        let command = Command::LinkTicket {
            project: self.project.clone(),
            name,
            ticket,
        };
        self.page
            .update(cx, |page, cx| page.perform(command, cx))
            .ok();
        cx.notify();
    }

    /// The daemon linked what was typed, and the field empties; or it says why not.
    fn ticket_answered(
        &mut self,
        refused: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match refused {
            None => {
                self.ticket_error = None;
                self.linking = false;
                if let Some(field) = self.ticket_field.clone() {
                    field.update(cx, |field, cx| field.set_value("", window, cx));
                }
            }
            Some(why) => self.ticket_error = Some(why.into()),
        }
        cx.notify();
    }

    pub fn agents(&self) -> &[RailAgent] {
        &self.agents
    }

    /// What the rail draws of the agents: the ones in their place, and the folded rows.
    pub fn rows(&self) -> Vec<RailRow> {
        self.rows_among(&self.open_ended)
    }

    /// [`SessionRail::rows`] with the "ended" rows of `open` unfolded.
    fn rows_among(&self, open: &HashSet<String>) -> Vec<RailRow> {
        let nodes: Vec<Node> = self
            .agents
            .iter()
            .map(|a| Node {
                id: &a.id,
                parent: a.parent.as_deref(),
                ended: a.ended,
            })
            .collect();
        rail_rows(&nodes, open)
    }

    /// Opens or folds the row that gathers the ended children of `parent`; they come and go over
    /// [`PANEL`].
    pub fn toggle_ended(&mut self, parent: &str, cx: &mut Context<Self>) {
        let open = !self.open_ended.remove(parent);
        if open {
            self.open_ended.insert(parent.to_owned());
        }
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let fold = self
            .ended_folds
            .entry(parent.to_owned())
            .or_insert_with(EndedFold::new);
        fold.animate(open, now, reduce);
        cx.notify();
    }

    /// The Workspace section's "⋯" menu.
    pub fn menu(&self) -> &Entity<Menu> {
        &self.menu
    }

    pub fn place(&self) -> Option<&PlaceFacts> {
        self.place.as_ref()
    }

    /// How far the rail is scrolled, in pixels from its top.
    pub fn scrolled(&self) -> Pixels {
        -self.scroll.offset().y
    }

    /// Lights the card of request `number`, in sight at the rail's bottom.
    pub fn reveal(&mut self, number: u64, cx: &mut Context<Self>) {
        self.waiting
            .update(cx, |waiting, cx| waiting.point_at(number, cx));
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    fn set_agents(&mut self, agents: Vec<RailAgent>, cx: &mut Context<Self>) {
        if self.agents == agents {
            return;
        }
        for agent in &agents {
            self.marks.entry(agent.id.clone()).or_insert_with(|| {
                let status = agent.status.clone();
                cx.new(|cx| StateIcon::new(status, cx))
            });
        }
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        // The gauges of the agents that left the rail go with them.
        self.gauges
            .retain(|id, _| agents.iter().any(|agent| &agent.id == id));
        for agent in &agents {
            let fill = agent.context.unwrap_or(0) as f32;
            let had_bar = self
                .agents
                .iter()
                .any(|before| before.id == agent.id && before.context.is_some());
            match self.gauges.get_mut(&agent.id) {
                // The fill follows a change of the use; a bar that unfolds is already at its
                // share, and an agent first seen shows its gauge at once.
                Some(motion) if had_bar => motion.animate(fill, PANEL, now, reduce),
                Some(motion) => *motion = Motion::settled(fill),
                None => {
                    self.gauges.insert(agent.id.clone(), Motion::settled(fill));
                }
            }
        }
        self.agents = agents;
        cx.notify();
    }

    fn set_selected(&mut self, selected: String, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.commands
                .update(cx, |commands, cx| commands.show_agent(&selected, cx));
            self.selected = selected;
            cx.notify();
        }
    }

    /// The place's facts; `None` when the daemon has none (the workspace was removed).
    fn set_place(&mut self, place: Option<PlaceFacts>, cx: &mut Context<Self>) {
        let first = !std::mem::replace(&mut self.place_known, true);
        if self.place != place || first {
            self.place = place;
            cx.notify();
        }
    }

    /// The forge's part of the place, when it answers; the rest of the place is not touched.
    fn set_forge(&mut self, forge: ForgeFacts, cx: &mut Context<Self>) {
        let Some(place) = &mut self.place else {
            return;
        };
        let ForgeFacts {
            pulls,
            forges,
            problem,
        } = forge;
        if place.forge_pending
            || place.pulls != pulls
            || place.forges != forges
            || place.forge_problem != problem
        {
            place.pulls = pulls;
            place.forges = forges;
            place.forge_problem = problem;
            place.forge_pending = false;
            cx.notify();
        }
    }

    /// The ticket's part of the place, when its tracker answers.
    fn set_ticket(&mut self, ticket: Option<TicketFacts>, cx: &mut Context<Self>) {
        let Some(place) = &mut self.place else {
            return;
        };
        if place.ticket_pending || place.ticket != ticket {
            place.ticket = ticket;
            place.ticket_pending = false;
            cx.notify();
        }
    }
}

impl Render for SessionRail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        // A folding "ended" row still has its children until the fold ends.
        let now = cx.background_executor().now();
        let mut drawn_open = self.open_ended.clone();
        for (parent, fold) in &mut self.ended_folds {
            if fold.motion.is_running(now) {
                window.request_animation_frame();
                drawn_open.insert(parent.clone());
            } else {
                fold.motion.settle(now);
            }
        }
        for motion in self.gauges.values_mut() {
            if motion.is_running(now) {
                window.request_animation_frame();
            } else {
                motion.settle(now);
            }
        }
        // The root alone is ended (the daemon refuses a child), and only while it is not ended.
        let endable = self.agents.iter().any(|a| {
            a.parent.is_none()
                && !matches!(
                    a.status.read(cx).state(),
                    AgentState::Failed | AgentState::Completed | AgentState::Stopped
                )
        });
        let listed = self.rows_among(&drawn_open);
        // The chevrons of the "ended" rows turn as their level folds.
        let turns: HashMap<String, f32> = listed
            .iter()
            .filter_map(|row| match row {
                RailRow::Ended { parent, .. } => Some(parent.clone()),
                _ => None,
            })
            .map(|parent| {
                let open = self.open_ended.contains(&parent);
                let id = ElementId::Name(format!("rail-ended-{parent}").into());
                (parent, turning(id, open, window, cx))
            })
            .collect();
        let agent_menu = self.agent_menu.clone();
        // An agent waiting on its wake says when it is due, as its row in the sidebar does.
        let wakes: HashMap<String, u64> = self
            .subscriptions
            .read(cx)
            .rows()
            .iter()
            .filter(|row| row.wake)
            .map(|row| (row.agent.clone(), row.expires_at))
            .collect();
        // The bar of a gauge unfolds, its gap with it, once the window's size is known.
        let bars: HashMap<String, f32> = self
            .agents
            .iter()
            .map(|agent| {
                let bar = fade(
                    format!("rail-context-bar-{}", agent.id),
                    agent.context.is_some(),
                    PANEL,
                    window,
                    cx,
                );
                (agent.id.clone(), bar)
            })
            .collect();
        // The task runs each agent has going, said on its row in the list while another agent is
        // shown: nothing running in the tree stays hidden.
        let mut going: HashMap<String, usize> = HashMap::new();
        for run in self.commands.read(cx).rows() {
            if run.running() && run.is_task() {
                *going.entry(run.agent.clone()).or_default() += 1;
            }
        }
        self.last_runs.extend(going.clone());
        let counts: HashMap<String, f32> = self
            .agents
            .iter()
            .filter(|agent| agent.depth > 0)
            .map(|agent| {
                let on = agent.id != self.selected && going.contains_key(&agent.id);
                let id = format!("rail-runs-count-fade-{}", agent.id);
                (agent.id.clone(), fade(id, on, PANEL, window, cx))
            })
            .collect();
        let agent_row = |agent: &RailAgent, window: &mut Window, cx: &mut App| {
            let mark = self.marks[&agent.id].clone();
            let page = self.page.clone();
            let target = agent.id.clone();
            let kid = agent.depth > 0;
            let indent = space::LG * agent.depth as f32;
            let runs = agent.runs_on.clone();
            // The root has no name on screen: what it runs on says it, alone on its row.
            let top_id = match kid {
                false => format!("rail-runs-on-{}", agent.id),
                true => format!("rail-name-{}", agent.id),
            };
            let (name, said) = match kid {
                false => (runs, None),
                true => (
                    agent.name.clone(),
                    Some(match agent.doing.as_str() {
                        "" => agent.model.clone().unwrap_or(agent.provider.clone()),
                        doing => format!(
                            "{} · {doing}",
                            agent.model.clone().unwrap_or(agent.provider.clone())
                        ),
                    }),
                ),
            };
            let waiting_line = agent.waits_on_subscription.then(|| {
                SharedString::from(match wakes.get(&agent.id) {
                    Some(at) => format!("waiting: wake at {}", clock(*at)),
                    None => agent.doing.clone(),
                })
            });
            // A child with task runs going says how many; a click opens it with its Runs section.
            let chip_t = counts.get(&agent.id).copied().unwrap_or_default();
            let runs_chip = (kid && chip_t > 0.).then(|| {
                let n = self.last_runs.get(&agent.id).copied().unwrap_or_default();
                let said = SharedString::from(format!("{n} run{}", if n == 1 { "" } else { "s" }));
                let (page, target) = (self.page.clone(), agent.id.clone());
                let chip = div()
                    .id(ElementId::Name(
                        format!("rail-runs-count-{}", agent.id).into(),
                    ))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::XS)
                    .h(px(18.))
                    .px(space::XS)
                    .rounded(px(5.))
                    .max_w(px(80.) * chip_t)
                    .opacity(chip_t)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .mono(10.5)
                    .text_color(p.faint)
                    .cursor_pointer()
                    .aria_label(said.clone())
                    .child(icon(Icon::Play).size(px(9.)))
                    .child(said)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        if let Some(page) = page.upgrade() {
                            page.update(cx, |page, cx| page.select(&target, cx));
                        }
                    });
                hoverable(chip, move |d, t| {
                    d.bg(p.hover.opacity(t)).text_color(mix(p.faint, p.text, t))
                })
            });
            let bar = bars.get(&agent.id).copied().unwrap_or_default();
            let fill = self.gauges.get(&agent.id).map_or(0., |m| m.value(now));
            let gauge_text = match (agent.context, agent.context_tokens) {
                (Some(used), _) => Some((
                    format!("{}%", used.min(100)),
                    format!("{}% of the context used", used.min(100)),
                )),
                (None, Some(used)) => Some((
                    tokens(used),
                    format!("{} tokens in the context", tokens(used)),
                )),
                (None, None) => None,
            };
            let has_gauge = gauge_text.is_some();
            let gauge = match gauge_text {
                Some((text, label)) => {
                    // The share (or the tokens used) and its bar on one line, the share before it.
                    div()
                        .id(ElementId::Name(format!("rail-context-{}", agent.id).into()))
                        .flex()
                        .items_center()
                        .justify_end()
                        .mono(10.5)
                        .text_color(p.faint)
                        .aria_label(SharedString::from(label))
                        .child(
                            div()
                                .id(ElementId::Name(
                                    format!("rail-context-text-{}", agent.id).into(),
                                ))
                                .child(SharedString::from(text))
                                .test_support(),
                        )
                        // Drawn while it unfolds and while it folds away, too.
                        .when(agent.context.is_some() || bar > 0., |d| {
                            d.child(
                                div()
                                    .id(ElementId::Name(
                                        format!("rail-context-bar-{}", agent.id).into(),
                                    ))
                                    .ml(space::XS * bar)
                                    .w(px(32.) * bar)
                                    .h(px(3.))
                                    .flex_none()
                                    .rounded(px(2.))
                                    .bg(p.line2)
                                    .opacity(bar)
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(px(32. * fill.clamp(0., 100.) / 100.) * bar)
                                            .rounded(px(2.))
                                            .bg(p.mute),
                                    )
                                    .test_support(),
                            )
                        })
                        .test_support()
                        .into_any_element()
                }
                None => div().into_any_element(),
            };
            // The whole gauge unfolds and fades in when the first usage arrives.
            let gauge = presence(
                ElementId::Name(format!("rail-context-{}-block", agent.id).into()),
                has_gauge,
                gauge,
            );
            let row = div()
                .id(ElementId::Name(format!("rail-row-{}", agent.id).into()))
                .flex_1()
                .min_w_0()
                .h(px(44.))
                .flex()
                .items_center()
                .gap(space::SM)
                .pl(space::SM)
                .pr(space::XS)
                .rounded(px(8.))
                .cursor_pointer()
                .child(
                    div()
                        .w(px(16.))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(mark),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child({
                            let name = tipped_when_cut(
                                div()
                                    .id(ElementId::Name(top_id.into()))
                                    .medium()
                                    .text_size(px(13.))
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .aria_label(SharedString::from(name.clone()))
                                    .when(runs_chip.is_some(), |d| d.flex_1().min_w_0())
                                    .child(SharedString::from(name.clone())),
                                SharedString::from(name),
                                window,
                                cx,
                            );
                            match runs_chip {
                                Some(chip) => div()
                                    .flex()
                                    .items_center()
                                    .gap(space::XS)
                                    .child(name)
                                    .child(chip)
                                    .into_any_element(),
                                None => name.into_any_element(),
                            }
                        })
                        .children(said.map(|said| {
                            tipped_when_cut(
                                div()
                                    .id(ElementId::Name(
                                        format!("rail-runs-on-{}", agent.id).into(),
                                    ))
                                    .mono(11.)
                                    .text_color(p.faint)
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .aria_label(SharedString::from(said.clone()))
                                    .child(SharedString::from(said.clone())),
                                SharedString::from(said),
                                window,
                                cx,
                            )
                        }))
                        // What the root waits for between turns, folding in and away with it.
                        .when(!kid, |d| {
                            d.child(presence_of(
                                ElementId::Name(format!("rail-waiting-{}-block", agent.id).into()),
                                waiting_line.clone(),
                                {
                                    let (id, p) = (agent.id.clone(), p);
                                    move |line| {
                                        div()
                                            .id(ElementId::Name(
                                                format!("rail-waiting-{id}").into(),
                                            ))
                                            .mono(11.)
                                            .text_color(p.faint)
                                            .text_ellipsis()
                                            .overflow_hidden()
                                            .aria_label(line.clone())
                                            .child(line)
                                            .test_support()
                                            .into_any_element()
                                    }
                                },
                            ))
                        }),
                )
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .child(gauge),
                )
                .map(|d| match !kid && endable {
                    true => d.child(agent_menu.clone()),
                    false => d.child(div().w(px(28.)).flex_none()),
                })
                .on_click(move |_, _, cx| {
                    if let Some(page) = page.upgrade() {
                        page.update(cx, |page, cx| page.select(&target, cx));
                    }
                });
            div()
                .id(ElementId::Name(format!("rail-agent-{}", agent.id).into()))
                .relative()
                .flex()
                .items_center()
                .pl(indent)
                .when(kid, |d| {
                    d.child(enter(
                        ElementId::Name(format!("elbow-{}", agent.id).into()),
                        0,
                        elbow(p.line2)
                            .absolute()
                            .left(indent - px(10.))
                            .top(px(-6.))
                            .w(px(9.))
                            .h(px(28.)),
                    ))
                })
                .child(selectable(
                    row,
                    agent.id == self.selected,
                    move |d, hover, chosen| d.bg(mix(p.hover.opacity(hover), p.card2, chosen)),
                ))
                .test_support()
        };
        let ended_row = |parent: &str, count: usize, open: bool, turn: f32, cx: &Context<Self>| {
            let depth = self
                .agents
                .iter()
                .find(|a| a.id == parent)
                .map_or(1, |a| a.depth + 1);
            let indent = space::LG * depth as f32;
            let target = parent.to_owned();
            let said = SharedString::from(format!("{count} ended"));
            div().pl(indent).child(ended_row(
                ElementId::Name(format!("rail-ended-{parent}").into()),
                said.clone(),
                said,
                open,
                turn,
                p,
                cx.listener(move |this, _, _, cx| this.toggle_ended(&target, cx)),
            ))
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut listed = listed.into_iter();
        while let Some(row) = listed.next() {
            match row {
                RailRow::Agent(id) => {
                    rows.extend(
                        self.agents
                            .iter()
                            .find(|a| a.id == id)
                            .map(|agent| agent_row(agent, window, cx).into_any_element()),
                    );
                }
                RailRow::Ended {
                    parent,
                    count,
                    open,
                } => {
                    rows.push(
                        ended_row(
                            &parent,
                            count,
                            self.open_ended.contains(&parent),
                            turns.get(&parent).copied().unwrap_or(0.0),
                            cx,
                        )
                        .into_any_element(),
                    );
                    if !open {
                        continue;
                    }
                    // The children behind the row follow it, and fold with it.
                    let children: Vec<AnyElement> = listed
                        .by_ref()
                        .take(count)
                        .filter_map(|row| match row {
                            RailRow::Agent(id) => {
                                let agent = self.agents.iter().find(|a| a.id == id)?;
                                Some(agent_row(agent, window, cx).into_any_element())
                            }
                            RailRow::Ended { .. } => None,
                        })
                        .collect();
                    let Some(fold) = self.ended_folds.get(&parent) else {
                        continue;
                    };
                    rows.push(
                        fold_frame(
                            ElementId::Name(format!("rail-ended-children-{parent}").into()),
                            self.open_ended.contains(&parent),
                            fold.motion.is_running(now),
                            fold.motion.value(now),
                            &fold.height,
                            div().flex().flex_col().children(children),
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        let agents = div()
            .id("rail-agents")
            .flex()
            .flex_col()
            .pb(space::MD)
            .child(rail_label("Agents", None, cx))
            .children(rows)
            .test_support();

        // The workspace: one labelled line each, as its card on the project page.
        let pull_of = |path: &str| {
            self.place
                .iter()
                .flat_map(|place| place.pulls.iter())
                .find(|pull| pull.repository == path)
                .cloned()
        };
        let status_of = |path: &str| {
            self.place
                .iter()
                .flat_map(|place| place.statuses.iter())
                .find(|status| status.path == path)
                .cloned()
        };
        let repositories: Vec<(String, String, RepositoryFacts, Option<RepositoryStatus>)> = self
            .place
            .iter()
            .flat_map(|place| place.repositories.iter())
            .map(|(path, facts)| {
                (
                    path.clone(),
                    crate::project::repository_label(&self.project, path),
                    facts.clone(),
                    status_of(path),
                )
            })
            .collect();
        let several = repositories.len() > 1;
        let forge_pending = self.place.as_ref().is_some_and(|place| place.forge_pending);
        let mut lines: Vec<AnyElement> = Vec::new();
        for (path, name, facts, status) in repositories {
            let changes = match facts {
                RepositoryFacts::Changes(Some(c)) if c.added == 0 && c.removed == 0 => {
                    div().text_color(p.faint).child("clean").into_any_element()
                }
                RepositoryFacts::Changes(Some(c)) => div()
                    .flex()
                    .gap(space::XS)
                    .child(div().text_color(p.ok).child(SharedString::from(format!(
                        "{}+{}",
                        if c.capped { "≥" } else { "" },
                        c.added
                    ))))
                    .child(
                        div()
                            .text_color(p.fail)
                            .child(SharedString::from(format!("−{}", c.removed))),
                    )
                    .into_any_element(),
                RepositoryFacts::Changes(None) => div()
                    .text_color(p.faint)
                    .child("unreadable")
                    .into_any_element(),
                RepositoryFacts::Branch(branch) => div()
                    .text_color(p.faint)
                    .child(SharedString::from(
                        branch.unwrap_or_else(|| "no branch".to_owned()),
                    ))
                    .into_any_element(),
            };
            lines.push(
                div()
                    .id(ElementId::Name(
                        format!("rail-line-repository-{name}").into(),
                    ))
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .px(space::XS)
                    .py(space::XS)
                    .aria_label(SharedString::from(format!("Repository {name}")))
                    .child(
                        div()
                            .id(ElementId::Name(format!("rail-repository-{name}").into()))
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .text_size(px(13.))
                            .medium()
                            .child(
                                div()
                                    .id(ElementId::Name(
                                        format!("rail-repository-{name}-icon").into(),
                                    ))
                                    .w(px(FACT_ICON))
                                    .flex_none()
                                    .child(
                                        icon(Icon::Repository)
                                            .size(px(FACT_ICON))
                                            .text_color(p.faint),
                                    )
                                    .test_support(),
                            )
                            .child(
                                div()
                                    .id(ElementId::Name(
                                        format!("rail-repository-{name}-label").into(),
                                    ))
                                    .flex_1()
                                    .min_w_0()
                                    .child(SharedString::from(name.clone()))
                                    .test_support(),
                            )
                            .child(
                                div()
                                    .id(ElementId::Name(
                                        format!("rail-repository-{name}-value").into(),
                                    ))
                                    .flex_none()
                                    .mono(11.5)
                                    .child(changes)
                                    .test_support(),
                            )
                            .test_support(),
                    )
                    // Its branch, commits ahead, files not committed and setup played; it
                    // unfolds once the daemon said, and fades back in when it says another
                    // (a branch renamed).
                    .child(presence(
                        ElementId::Name(format!("rail-status-{name}-block").into()),
                        status.is_some(),
                        {
                            let said = SharedString::from(
                                status.as_ref().map(|s| s.line()).unwrap_or_default(),
                            );
                            let line = tipped_when_cut(
                                div()
                                    .id(ElementId::Name(format!("rail-status-{name}").into()))
                                    .pl(space::XL)
                                    .mono(11.5)
                                    .text_color(p.faint)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .aria_label(SharedString::from(status.as_ref().map_or(
                                        String::new(),
                                        |s| match &s.note {
                                            Some(note) => format!("{} · {note}", s.line()),
                                            None => s.line(),
                                        },
                                    )))
                                    .child(said.clone()),
                                said.clone(),
                                window,
                                cx,
                            );
                            changing(
                                ElementId::Name(format!("rail-status-{name}-change").into()),
                                said,
                                move |_| line,
                            )
                        },
                    ))
                    .test_support()
                    .into_any_element(),
            );
            let pull_line = pull_of(&path).map(|pull| {
                let page = self.page.clone();
                let url = pull.url.clone();
                let row = pull_row(
                    ElementId::Name(format!("rail-pull-{name}").into()),
                    &pull,
                    cx,
                )
                .on_click(move |_, _, cx| {
                    page.update(cx, |page, _| page.open_link(&url)).ok();
                });
                let key = match several {
                    true => format!(
                        "Pull request · {}",
                        crate::project::repository_label(&self.project, &path)
                    ),
                    false => "Pull request".to_owned(),
                };
                fact_line(
                    format!("rail-line-pull-{name}"),
                    Some(Icon::PullRequest),
                    key,
                    pull.label(),
                    pull_hover(row, cx),
                    p,
                )
                .into_any_element()
            });
            // A workspace's pull request waits for its forge: a skeleton stands for the row, and
            // folds away when the repository turns out to have none.
            match self.workspace.is_some() {
                true => lines.push(
                    awaiting(
                        ElementId::Name(format!("rail-pull-wait-{name}").into()),
                        forge_pending,
                        skeleton_line(format!("rail-pull-skeleton-{name}"), 44.),
                        pull_line.unwrap_or_else(|| div().into_any_element()),
                    )
                    .into_any_element(),
                ),
                false => lines.extend(pull_line),
            }
        }
        let forge_problem = self.place.as_ref().and_then(|p| p.forge_problem.clone());
        // The forge is the project's setting, not a line of the workspace: the section only says
        // which repositories have none, once, since their pull requests cannot show.
        let no_forge: Vec<String> = self
            .place
            .iter()
            .flat_map(|place| place.forges.iter())
            .filter(|(_, line)| line.starts_with(flottille_core::declared::NO_FORGE))
            .map(|(path, _)| crate::project::repository_label(&self.project, path))
            .collect();
        let no_forge = (!no_forge.is_empty()).then(|| {
            SharedString::from(format!(
                "No forge set for {}: choose it in the project's Settings",
                no_forge.join(", ")
            ))
        });
        if self.workspace.is_some() && self.ticket_field.is_none() {
            let field = cx.new(|cx| {
                let mut field = InputState::new(window, cx).placeholder("#123 or ABC-123");
                follow_theme(&mut field, cx);
                field
            });
            self._ticket_subscription =
                Some(
                    cx.subscribe_in(&field, window, |this, field, event, _, cx| {
                        if let InputEvent::PressEnter { .. } = event {
                            let typed = field.read(cx).value().trim().to_owned();
                            if !typed.is_empty() {
                                this.link_ticket(Some(typed), cx);
                            }
                        }
                    }),
                );
            self.ticket_field = Some(field);
        }
        let linked = self.place.as_ref().and_then(|p| p.ticket.clone());
        if linked.is_some() {
            self.last_ticket = linked.clone();
        }
        let ticket = linked.or_else(|| self.last_ticket.clone());
        // The ticket is why the workspace exists: its line opens the section's lines, before any
        // pull request.
        let ticket_content = ticket.map(|ticket| {
            let page = self.page.clone();
            let said = match (&ticket.title, &ticket.problem) {
                (_, Some(problem)) => problem.clone(),
                (title, None) => title.clone().unwrap_or_default(),
            };
            let url = ticket.url.clone();
            let row = div()
                .id("rail-ticket")
                .min_w_0()
                .flex()
                .items_center()
                .gap(space::XS)
                .overflow_hidden()
                .whitespace_nowrap()
                .mono(FACT_SIZE)
                .when(url.is_some(), |d| d.cursor_pointer())
                .aria_label(SharedString::from(
                    format!(
                        "{} {} {said}",
                        ticket.reference,
                        ticket.state.clone().unwrap_or_default()
                    )
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                ))
                .child(
                    div()
                        .flex_none()
                        .child(SharedString::from(ticket.reference.clone())),
                )
                .child(tipped_when_cut(
                    div()
                        .id("rail-ticket-title")
                        .min_w_0()
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(SharedString::from(said.clone())),
                    SharedString::from(said),
                    window,
                    cx,
                ))
                .on_click(move |_, _, cx| {
                    if let Some(url) = &url {
                        page.update(cx, |page, _| page.open_link(url)).ok();
                    }
                });
            let row = hoverable(row, move |d, t| d.text_color(mix(p.mute, p.text, t)));
            fact_line(
                "rail-line-ticket".to_owned(),
                Some(Icon::Ticket),
                "Ticket",
                ticket.reference.clone(),
                row,
                p,
            )
            .into_any_element()
        });
        // A linked ticket waits for its tracker: a skeleton holds its line's place until it is read.
        let ticket_pending = self
            .place
            .as_ref()
            .is_some_and(|place| place.ticket_pending);
        // Unlinked, the line folds away with what it last said.
        let ticket_present =
            ticket_pending || self.place.as_ref().is_some_and(|p| p.ticket.is_some());
        let ticket_line = presence(
            "rail-ticket-block",
            ticket_present,
            awaiting(
                "rail-ticket-wait",
                ticket_pending,
                skeleton_line("rail-ticket-skeleton".to_owned(), 150.),
                ticket_content.unwrap_or_else(|| div().into_any_element()),
            ),
        );
        let ticket_field = self
            .ticket_field
            .clone()
            .filter(|_| self.workspace.is_some());
        let ticket_error = self.ticket_error.clone();
        let mut facts: Vec<(&str, String)> = Vec::new();
        match (&self.workspace, &self.place) {
            (Some(_), Some(place)) => {
                if !place.bases.is_empty() {
                    facts.push(("From", place.bases.join(", ")));
                }
            }
            (None, _) => facts.push(("Place", "the project folder".into())),
            _ => {}
        }
        facts.push(("Machine", "this PC".into()));
        let workspace = div()
            .id("rail-workspace")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .pb(space::MD)
            .child(rail_label(
                "Workspace",
                Some(self.menu.clone().into_any_element()),
                cx,
            ))
            // Before the daemon's first answer the section is a skeleton; its lines replace it.
            .child(awaiting(
                "rail-workspace-wait",
                !self.place_known,
                workspace_skeleton(),
                div()
                    .flex()
                    .flex_col()
                    .child(ticket_line)
                    .children(lines)
                    .child(presence(
                        "rail-no-forge-block",
                        no_forge.is_some(),
                        div()
                            .id("rail-no-forge")
                            .px(space::XS)
                            .py(space::XS)
                            .text_size(px(11.5))
                            .text_color(p.faint)
                            .children(no_forge.clone())
                            .aria_label(no_forge.unwrap_or_default())
                            .test_support(),
                    ))
                    .children(forge_problem.map(|problem| {
                        div()
                            .id("rail-forge-problem")
                            .px(space::XS)
                            .py(space::XS)
                            .text_size(px(11.5))
                            .text_color(p.faint)
                            .child(SharedString::from(problem))
                            .test_support()
                    }))
                    .child(presence(
                        "rail-ticket-field-block",
                        self.linking && ticket_field.is_some(),
                        div()
                            .id("rail-ticket-field")
                            .h(px(30.))
                            .mx(space::XS)
                            .my(space::XS)
                            .px(space::SM)
                            .rounded(px(7.))
                            .bg(p.field)
                            .border_1()
                            .border_color(p.line2)
                            .flex()
                            .items_center()
                            .mono(11.5)
                            .children(ticket_field.map(|field| Input::new(&field)))
                            .test_support(),
                    ))
                    .child(presence_of(
                        "rail-ticket-error-block",
                        ticket_error,
                        move |error| {
                            div()
                                .id("rail-ticket-error")
                                .px(space::XS)
                                .py(space::XS)
                                .text_size(px(11.5))
                                .text_color(p.fail)
                                .child(error)
                                .test_support()
                                .into_any_element()
                        },
                    ))
                    .child(
                        div()
                            .id("rail-facts")
                            .pt(space::SM)
                            .flex()
                            .flex_col()
                            .children(facts.into_iter().map(|(key, value)| {
                                let said = SharedString::from(value.clone());
                                let value = tipped_when_cut(
                                    div()
                                        .id(ElementId::Name(
                                            format!("rail-fact-{key}-value").into(),
                                        ))
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .mono(FACT_SIZE)
                                        .child(SharedString::from(value)),
                                    said.clone(),
                                    window,
                                    cx,
                                );
                                fact_line(format!("rail-fact-{key}"), None, key, said, value, p)
                            }))
                            .test_support(),
                    ),
            ))
            .test_support();
        div()
            .id("rail")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("rail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(space::MD)
                    .pt(space::SM)
                    .pb(space::LG)
                    .flex()
                    .flex_col()
                    .child(agents)
                    .child(workspace)
                    .child(self.subscriptions.clone())
                    .child(self.leaves.clone())
                    .child(self.commands.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .px(space::MD)
                    .pb(space::MD)
                    .child(self.waiting.clone()),
            )
            .test_support()
    }
}

/// The size of every value in the Workspace section's facts (From, Place, Machine, Ticket,
/// Pull request).
const FACT_SIZE: f32 = 12.;

/// The width of the icon column of the Workspace section's lines.
const FACT_ICON: f32 = 14.;

/// A fact line that waits for its read: blocks where its icon, its label and its value will be.
fn skeleton_line(id: String, value: f32) -> impl IntoElement {
    let part = |what: &str| ElementId::Name(format!("{id}-{what}").into());
    div()
        .id(ElementId::Name(id.clone().into()))
        .aria_label("Reading…")
        .px(space::XS)
        .min_h(px(24.))
        .flex()
        .items_center()
        .gap(space::SM)
        .child(
            Skeleton::new(part("icon"))
                .w(px(FACT_ICON))
                .h(px(FACT_ICON)),
        )
        .child(Skeleton::new(part("label")).w(px(72.)).h(px(10.)))
        .child(div().flex_1())
        .child(Skeleton::new(part("value")).w(px(value)).h(px(10.)))
        .test_support()
}

/// The Workspace section before the daemon first answered: a line for each of its facts.
fn workspace_skeleton() -> impl IntoElement {
    div()
        .id("rail-workspace-skeleton")
        .flex()
        .flex_col()
        .child(skeleton_line("rail-skeleton-repository".to_owned(), 80.))
        .child(skeleton_line("rail-skeleton-ticket".to_owned(), 150.))
        .child(skeleton_line("rail-skeleton-from".to_owned(), 60.))
        .child(skeleton_line("rail-skeleton-machine".to_owned(), 56.))
        .test_support()
}

/// One fact of the Workspace section: the icon column, the label, then the value at the right
/// edge. Every line of the section sits in the same columns: a fact with no icon keeps the
/// column empty, so that its label starts where the others do. `said` is the value in words.
fn fact_line(
    id: String,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
    said: impl Into<SharedString>,
    value: impl IntoElement,
    p: Palette,
) -> impl IntoElement {
    let label = label.into();
    let slot = div()
        .id(ElementId::Name(format!("{id}-icon").into()))
        .w(px(FACT_ICON))
        .flex_none()
        .children(glyph.map(|glyph| icon(glyph).size(px(FACT_ICON)).text_color(p.faint)))
        .test_support();
    div()
        .id(ElementId::Name(id.clone().into()))
        .aria_label(SharedString::from(format!("{label} {}", said.into())))
        .px(space::XS)
        .min_h(px(24.))
        .flex()
        .items_center()
        .gap(space::SM)
        .text_size(px(12.5))
        .child(slot)
        .child(
            div()
                .id(ElementId::Name(format!("{id}-label").into()))
                .flex_none()
                .text_color(p.faint)
                .child(label)
                .test_support(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_end()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(value),
        )
        .test_support()
}

/// What a workspace's "⋯" offers, wherever it is drawn: the rail's Workspace and the workspace's
/// card on the project page build it with this, so that both say the same.
pub struct WorkspaceActions {
    pub open_in_editor: WorkspaceAction,
    /// Shows where a ticket is typed; `None` where the workspace links none.
    pub link_ticket: Option<WorkspaceAction>,
    pub unlink_ticket: Option<WorkspaceAction>,
    /// Asks before removing it; `None` where it is not removed from.
    pub remove: Option<WorkspaceAction>,
}

pub type WorkspaceAction = Box<dyn Fn(&mut App)>;

/// A workspace's "⋯" menu.
pub fn workspace_menu(id: &str, actions: WorkspaceActions, cx: &mut Context<Menu>) -> Menu {
    let WorkspaceActions {
        open_in_editor,
        link_ticket,
        unlink_ticket,
        remove,
    } = actions;
    let mut menu = Menu::new(
        ElementId::Name(id.to_owned().into()),
        "Workspace actions",
        cx,
    )
    .entry(
        "open-in-editor",
        "Open in editor",
        Some(Icon::OpenInEditor),
        move |_, cx| open_in_editor(cx),
    );
    if let Some(link) = link_ticket {
        menu = menu.entry(
            "workspace-link-ticket",
            "Link a ticket…",
            Some(Icon::Ticket),
            move |_, cx| link(cx),
        );
    }
    if let Some(unlink) = unlink_ticket {
        menu = menu.entry(
            "workspace-unlink-ticket",
            "Unlink the ticket",
            None,
            move |_, cx| unlink(cx),
        );
    }
    if let Some(remove) = remove {
        menu = menu.danger("remove-workspace", "Remove workspace…", move |_, cx| {
            remove(cx)
        });
    }
    menu
}

/// A pull request's row: its number and state, and its checks' mark, drawn by shape as well as
/// colour. The place card draws the same.
pub(crate) fn pull_row(id: ElementId, pull: &PullRequest, cx: &App) -> Stateful<Div> {
    let p = palette(cx);
    let (mark, color, said) = match pull.mark.as_deref() {
        Some("passing") => (Some(Icon::Check), p.ok, "checks passing"),
        Some("failing") => (Some(Icon::Close), p.fail, "checks failing"),
        Some(_) => (Some(Icon::Running), p.run, "checks pending"),
        None => (None, p.faint, "no checks"),
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(space::SM)
        .cursor_pointer()
        .mono(FACT_SIZE)
        .aria_label(SharedString::from(format!("{}, {said}", pull.label())))
        .child(SharedString::from(pull.label()))
        .children(mark.map(|mark| icon(mark).size(px(12.)).text_color(color)))
}

/// A [`pull_row`] once its handlers and children are on: its hover fades in.
pub(crate) fn pull_hover(row: Stateful<Div>, cx: &App) -> flottille_ui::Hoverable {
    let p = palette(cx);
    hoverable(row, move |d, t| d.text_color(mix(p.mute, p.text, t)))
}

/// The drawn link from a parent to its child in the rail: down, then right, with a round corner.
fn elbow(color: gpui_kit::Hsla) -> gpui_kit::Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let (x, y) = (bounds.origin.x, bounds.origin.y);
            let (w, h) = (bounds.size.width, bounds.size.height);
            let mut path = PathBuilder::stroke(px(1.));
            path.move_to(point(x, y));
            path.line_to(point(x, y + h - px(6.)));
            path.curve_to(point(x + px(6.), y + h), point(x, y + h));
            path.line_to(point(x + w, y + h));
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
}

/// What the daemon said when it took a message: its delivery, and the last event before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub delivery: String,
    pub after: u64,
}

/// A message sent from the composer and not recorded yet: shown from the moment it is sent, in
/// the conversation, or under it where a message waits when a turn ran as it was sent.
#[derive(Clone, Debug)]
pub struct Pending {
    pub key: u64,
    pub text: String,
    pub images: Vec<Arc<gpui_kit::Image>>,
    /// Why the daemon refused it: it stays, with Retry and Edit.
    pub refused: Option<String>,
    pub accepted: Option<Accepted>,
    /// A turn ran as it was sent: it is to wait under the conversation.
    pub queued: bool,
    /// The last event of the conversation shown as it was sent: its record comes after.
    pub since: Option<u64>,
}

impl Pending {
    /// Sent, and the daemon has not answered yet.
    pub fn in_flight(&self) -> bool {
        self.accepted.is_none() && self.refused.is_none()
    }

    /// Taken by the daemon as `delivery`, and not refused since.
    fn is(&self, delivery: &str) -> bool {
        self.refused.is_none()
            && self
                .accepted
                .as_ref()
                .is_some_and(|a| a.delivery == delivery)
    }
}

/// The id of a part of a message being sent: `pending-<key>-<part>`.
pub fn pending_part(key: u64, part: &str) -> ElementId {
    ElementId::Name(format!("pending-{key}-{part}").into())
}

/// The composer of a session: it writes to the root agent, and only to it. A word at the cursor
/// that starts with `/` lists the project's commands: picking one completes it into `/name `, and
/// the daemon sends the instructions of each command the message uses with it. The field cannot
/// colour a range, so the commands the message uses show as chips under it.
pub struct Reply {
    shell: WeakEntity<Shell>,
    /// The root agent: fixed when the composer is made, the only agent it writes to.
    session: String,
    project: String,
    /// The project's commands, as the daemon listed them when `/` was last typed.
    commands: Option<Vec<CommandInfo>>,
    /// What follows the `/` of the word at the cursor, while it starts with one: the `/` list is
    /// open.
    slash: Option<String>,
    /// Where that word is in the message, its `/` included: what picking a command replaces.
    slash_at: std::ops::Range<usize>,
    /// The commands the message uses, as chips under the field.
    chips: Listing<String>,
    /// The rows of the `/` list, which unfold as a command matches and fold away as it does not.
    /// Kept as they were while the list closes, so that it folds away with its rows.
    slash_rows: Listing<(String, String)>,
    /// The entry of the `/` list the keyboard is on.
    highlighted: usize,
    /// Escape closed the `/` list: it stays closed until the message is no longer a `/` word.
    dismissed: bool,
    field: Entity<TextareaState>,
    options: Entity<SessionOptions>,
    /// What the root does while its turn runs, in the bar under the field.
    status_line: Entity<StatusLine>,
    pub mode: Entity<ModeSwitch>,
    has_text: bool,
    focused: bool,
    sending: bool,
    /// The root runs a turn: the composer offers to interrupt it.
    running: bool,
    /// An interruption was asked and not answered yet.
    interrupting: bool,
    /// The messages sent and not recorded yet, the oldest first: each shows from the moment it
    /// is sent, and one the daemon refused stays until it is sent again or taken back.
    pending: Vec<Pending>,
    /// The key the next message sent takes.
    next_pending: u64,
    /// Deliveries that left the queue (withdrawn: `None`) or will never be read (`Some`, why)
    /// before the daemon's answer named them: the answer finds them here.
    left: HashMap<String, Option<String>>,
    /// What the root does while its turn runs, for the line above the field.
    activity: Option<Activity>,
    /// The pictures that go with the next message.
    attached: Attachments,
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl Reply {
    pub fn new(
        shell: WeakEntity<Shell>,
        session: String,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(1, REPLY_MAX_ROWS)
                .submit_on_enter(true)
                .placeholder(REPLY_PLACEHOLDER);
            follow_theme(&mut field, cx);
            field
        });
        let subscriptions = vec![cx.subscribe_in(
            &field,
            window,
            |this, field, event, window, cx| match event {
                InputEvent::Change => {
                    let text = field.read(cx).value().to_string();
                    let has_text = !text.trim().is_empty();
                    if this.has_text != has_text {
                        this.has_text = has_text;
                        cx.notify();
                    }
                    let cursor = field.read(cx).cursor();
                    this.typed_at(&text, cursor, cx);
                }
                InputEvent::PressEnter { shift: false, .. } => this.enter(window, cx),
                InputEvent::Focus if !this.focused => {
                    this.focused = true;
                    cx.notify();
                }
                InputEvent::Blur if this.focused => {
                    this.focused = false;
                    cx.notify();
                }
                _ => {}
            },
        )];
        let mode = cx.new(|_| ModeSwitch::new(shell.clone(), session.clone()));
        let options = cx.new(|_| SessionOptions::new(shell.clone(), session.clone()));
        let reply = Self {
            shell,
            session,
            project,
            commands: None,
            slash: None,
            slash_at: 0..0,
            chips: Listing::default(),
            slash_rows: Listing::default(),
            highlighted: 0,
            dismissed: false,
            field,
            options,
            status_line: cx.new(|_| StatusLine::new()),
            mode,
            has_text: false,
            focused: false,
            sending: false,
            running: false,
            interrupting: false,
            pending: Vec::new(),
            next_pending: 0,
            left: HashMap::new(),
            activity: None,
            attached: Attachments::default(),
            renders: 0,
            _subscriptions: subscriptions,
        };
        // A message put back or pasted shows its chips without a `/` typed first.
        reply.list_commands(cx);
        reply
    }

    /// The agent it writes to: the session's root.
    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn field(&self) -> &Entity<TextareaState> {
        &self.field
    }

    /// The provider shown and the model and effort pickers under the field.
    pub fn options(&self) -> &Entity<SessionOptions> {
        &self.options
    }

    pub fn is_sending(&self) -> bool {
        self.sending
    }

    /// The messages sent and not recorded yet, the oldest first.
    pub fn pending(&self) -> &[Pending] {
        &self.pending
    }

    /// The pictures that go with the next message.
    pub fn attachments(&self) -> &[Arc<gpui_kit::Image>] {
        self.attached.pictures()
    }

    /// The composer holds something the user has not sent: text, or a picture.
    pub fn holds_draft(&self) -> bool {
        self.has_text || !self.attachments().is_empty()
    }

    /// A big picture is still being scaled down.
    pub fn is_scaling(&self) -> bool {
        self.attached.is_scaling()
    }

    /// The line above the field while a turn runs: what the root does.
    pub fn status(&self) -> Option<String> {
        self.activity
            .as_ref()
            .filter(|_| self.running)
            .map(Activity::line)
    }

    fn set_activity(&mut self, activity: Option<Activity>, cx: &mut Context<Self>) {
        if self.activity != activity {
            self.activity = activity;
            self.sync_status(cx);
            cx.notify();
        }
    }

    /// Hands the bar's status line what the root does now, or none once the turn ended.
    fn sync_status(&mut self, cx: &mut Context<Self>) {
        let status = self.status().map(SharedString::from);
        self.status_line.update(cx, |line, cx| line.set(status, cx));
    }

    fn set_images(&mut self, images: Option<bool>) {
        self.attached.set_takes(images);
    }

    fn attached_of(&mut self) -> &mut Attachments {
        &mut self.attached
    }

    /// Attaches a picture, or says in a toast why not; the text in the field stays.
    pub fn attach(&mut self, image: gpui_kit::Image, cx: &mut Context<Self>) {
        self.attached.attach(image, cx, Self::attached_of);
    }

    /// Attaches the picture a file holds (a file dropped on the composer, or picked).
    pub fn attach_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.attached.attach_file(path, cx, Self::attached_of);
    }

    pub fn detach(&mut self, at: usize, cx: &mut Context<Self>) {
        self.attached.detach(at, cx);
    }

    /// The root runs a turn.
    pub fn is_running(&self) -> bool {
        self.running
    }

    fn set_running(&mut self, running: bool, cx: &mut Context<Self>) {
        if self.running != running {
            self.running = running;
            self.sync_status(cx);
            cx.notify();
        }
    }

    /// Interrupts the turn that runs: the agent stays alive, and the turn ends cancelled. Nothing
    /// happens while no turn runs.
    pub fn interrupt(&mut self, cx: &mut Context<Self>) {
        if !self.running || self.interrupting {
            return;
        }
        self.interrupting = true;
        let command = Command::Interrupt {
            session: self.session.clone(),
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// What the daemon answered to an interruption.
    pub fn interrupted(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.interrupting = false;
        if let Some(why) = refused {
            toast(Toast::failure(NOT_INTERRUPTED).detail(why), cx);
        }
        cx.notify();
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// The commands the `/` list offers now, each with its description: those whose name holds
    /// what follows the `/`, the project's, then the composer's own (`/reload-tools`), which
    /// takes the place of a project's command of its name. None while the list is closed.
    pub fn slash_entries(&self) -> Vec<(String, String)> {
        let (Some(typed), Some(commands), false) = (&self.slash, &self.commands, self.dismissed)
        else {
            return Vec::new();
        };
        let typed = typed.to_lowercase();
        commands
            .iter()
            .filter(|p| p.name != RELOAD_COMMAND && p.name.to_lowercase().contains(&typed))
            .map(|p| (p.name.clone(), p.description.clone()))
            .chain(
                RELOAD_COMMAND
                    .contains(&typed)
                    .then(|| (RELOAD_COMMAND.to_owned(), RELOAD_COMMAND_LINE.to_owned())),
            )
            .collect()
    }

    /// The daemon has listed the project's commands since the composer was made.
    pub fn knows_commands(&self) -> bool {
        self.commands.is_some()
    }

    fn slash_open(&self) -> bool {
        self.slash.is_some() && !self.dismissed
    }

    /// The message changed, its cursor at its end: a text set from code says it here.
    pub fn typed(&mut self, text: &str, cx: &mut Context<Self>) {
        self.typed_at(text, text.len(), cx);
    }

    /// The message changed, its cursor at the byte `cursor`: a word there that starts with `/`
    /// opens the list, asking the daemon for the commands again (the project's file is the user's
    /// to edit). The field says it when the user types.
    pub fn typed_at(&mut self, text: &str, cursor: usize, cx: &mut Context<Self>) {
        let cursor = cursor.min(text.len());
        let start = text[..cursor].rfind(char::is_whitespace).map_or(0, |at| {
            at + text[at..].chars().next().map_or(1, char::len_utf8)
        });
        let end = text[cursor..]
            .find(char::is_whitespace)
            .map_or(text.len(), |at| cursor + at);
        let slash = text[start..end].strip_prefix('/').map(str::to_owned);
        self.slash_at = start..end;
        if slash.is_none() {
            self.dismissed = false;
        }
        if slash.is_some() && self.slash.is_none() {
            self.list_commands(cx);
        }
        if self.slash != slash {
            self.slash = slash;
            self.highlighted = 0;
            cx.notify();
        }
    }

    /// Asks the daemon for the project's commands: the `/` list offers them, and the chips know
    /// them.
    fn list_commands(&self, cx: &mut Context<Self>) {
        let command = Command::Commands {
            project: self.project.clone(),
        };
        // Later: the shell may be the one setting the text, as it hands a reply its answer.
        crate::commands::perform_later(&self.shell, command, cx);
    }

    /// The project's commands, as the daemon listed them.
    pub fn commands_listed(
        &mut self,
        project: &str,
        listed: &Result<Vec<CommandInfo>, String>,
        cx: &mut Context<Self>,
    ) {
        if project != self.project {
            return;
        }
        if let Ok(list) = listed
            && self.commands.as_ref() != Some(list)
        {
            self.commands = Some(list.clone());
            self.highlighted = 0;
            cx.notify();
        }
    }

    fn highlight(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.slash_entries().len();
        if count > 0 {
            self.highlighted = (self.highlighted as isize + by).rem_euclid(count as isize) as usize;
            cx.notify();
        }
    }

    /// Enter: picks the command the `/` list is on; with none offered, the message is sent as
    /// text.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.slash_entries();
        match entries.get(self.highlighted).or(entries.first()) {
            Some((name, _)) => {
                let name = name.clone();
                self.complete(&name, true, window, cx)
            }
            None => self.send(window, cx),
        }
    }

    /// Completes the `/` word at the cursor into `/name `, the rest of the message as it was, for
    /// the user to add what the command is about before sending.
    pub fn pick(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.complete(name, false, window, cx);
    }

    /// `pick`; `entered` when Enter picked it, whose line break the field put after the word.
    /// `/reload-tools` is done at once, its word taken out of the message.
    fn complete(&mut self, name: &str, entered: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.slash = None;
        self.dismissed = true;
        self.has_text = true;
        let now = self.field.read(cx).value().to_string();
        let at = self.slash_at.start.min(now.len())..self.slash_at.end.min(now.len());
        let (before, mut after) = (&now[..at.start], &now[at.end..]);
        if entered {
            after = after.strip_prefix('\n').unwrap_or(after);
        }
        if name == RELOAD_COMMAND {
            let text = format!("{before}{}", after.trim_start());
            self.has_text = !text.trim().is_empty();
            self.field.update(cx, |field, cx| {
                field.set_value(text.as_str(), window, cx);
                field.set_selected_range(before.len()..before.len(), cx);
            });
            let command = Command::ReloadTools {
                session: self.session.clone(),
            };
            self.shell
                .update(cx, |shell, _| shell.perform(command))
                .ok();
            cx.notify();
            return;
        }
        let space = if after.starts_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        let text = format!("{before}/{name}{space}{after}");
        let cursor = before.len() + name.len() + 2;
        self.field.update(cx, |field, cx| {
            field.set_value(text.as_str(), window, cx);
            field.set_selected_range(cursor..cursor, cx);
        });
        cx.notify();
    }

    /// The commands the message uses now, in the order of their first use: the chips under the
    /// field.
    pub fn commands_used(&self, cx: &App) -> Vec<String> {
        let Some(commands) = &self.commands else {
            return Vec::new();
        };
        let text = self.field.read(cx).value().to_string();
        let mut used: Vec<String> = Vec::new();
        for (_, name) in flottille_core::commands::used(&text, |name| {
            commands.iter().any(|command| command.name == name)
        }) {
            if !used.iter().any(|known| known == name) {
                used.push(name.to_owned());
            }
        }
        used
    }

    /// The chips of the commands the message uses, under the field: each one comes and goes as
    /// the message gains or loses it, the line with the first and the last.
    fn render_chips(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let used = self.commands_used(cx);
        let now = cx.background_executor().now();
        self.chips.sync(
            used.iter().map(|name| (name.clone(), name.clone())),
            now,
            cx.reduce_motion(),
        );
        let p = palette(cx);
        let chips: Vec<AnyElement> = self
            .chips
            .rows()
            .map(|row| {
                let name = row.value.clone();
                flottille_ui::Item::new(
                    ElementId::Name(format!("reply-command-item-{name}").into()),
                    row.placement,
                    div()
                        .id(ElementId::Name(format!("reply-command-{name}").into()))
                        .flex_none()
                        .px(space::SM)
                        .rounded(px(6.))
                        .bg(p.acc.opacity(0.14))
                        .text_color(p.acc)
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_size(px(12.5))
                        .aria_label(SharedString::from(format!("/{name}")))
                        .child(format!("/{name}"))
                        .test_support(),
                )
                .into_any_element()
            })
            .collect();
        presence(
            "reply-commands",
            !used.is_empty(),
            div()
                .id("reply-commands-line")
                .flex()
                .flex_wrap()
                .gap(space::XS)
                .pt(space::SM)
                .px(space::XS)
                .children(chips)
                .test_support(),
        )
        .into_any_element()
    }

    /// The `/` list, over the composer: the commands offered. It unfolds when it opens and folds
    /// away when it closes, and its height follows its rows.
    fn render_slash(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let entries = self.slash_entries();
        // A word that names no command is a message like any other: nothing to show, unless the
        // project has no command at all.
        let none = self.slash_open()
            && self.slash.as_deref() == Some("")
            && self.commands.as_ref().is_some_and(|c| c.is_empty());
        let open = !entries.is_empty() || none;
        let on = entries.get(self.highlighted).or(entries.first()).cloned();
        if open {
            let now = cx.background_executor().now();
            self.slash_rows.sync(
                entries.into_iter().map(|entry| (entry.0.clone(), entry)),
                now,
                cx.reduce_motion(),
            );
        }
        let rows: Vec<AnyElement> = self
            .slash_rows
            .rows()
            .map(|row| {
                let (name, description) = row.value.clone();
                let picked = name.clone();
                let mut line = ListRow::new(
                    ElementId::Name(format!("slash-{name}").into()),
                    format!("/{name}"),
                )
                .line(description)
                .selected(open && on.as_ref().is_some_and(|(on, _)| *on == name));
                if open && !row.placement.is_gone() {
                    line = line.on_click(
                        cx.listener(move |this, _, window, cx| this.pick(&picked, window, cx)),
                    );
                }
                flottille_ui::Item::new(
                    ElementId::Name(format!("slash-item-{name}").into()),
                    row.placement,
                    line,
                )
                .gap(space::XS)
                .into_any_element()
            })
            .collect();
        let listed = !self.slash_rows.is_empty();
        let note = div()
            .id("slash-none")
            .aria_label("This project has no command yet: add one in its settings.")
            .px(space::MD)
            .py(space::SM)
            .text_size(px(12.5))
            .text_color(p.faint)
            .child("This project has no command yet: add one in its settings.")
            .test_support();
        let card = div()
            .id("slash-list")
            .when(open, |card| card.occlude())
            .p(space::SM)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line2)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(space::XS)
            .child(presence("slash-label", listed, section_label("Commands", cx)).gap(space::XS))
            .children(rows)
            .child(presence("slash-note", none, note).gap(space::XS))
            .test_support();
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(gpui_kit::relative(1.))
            .pb(space::SM)
            .child(presence("slash-panel", open, card))
            .into_any_element()
    }

    /// Sends what the field holds to the root agent: it leaves the composer at once and shows as
    /// a message being sent until the daemon records it.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.field.read(cx).value().to_string();
        let message = typed.trim().to_owned();
        if self.sending {
            return;
        }
        // A picture that is not there yet would be left behind: the sender waits for it.
        if self.is_scaling() {
            return toast(Toast::failure(NOT_SENT).detail(STILL_SCALING), cx);
        }
        if message.is_empty() && self.attached.is_empty() {
            return;
        }
        let key = self.next_pending;
        self.next_pending += 1;
        self.pending.push(Pending {
            key,
            text: message,
            images: self.attached.take(cx),
            refused: None,
            accepted: None,
            queued: self.running,
            since: None,
        });
        self.has_text = false;
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.typed("", cx);
        self.ask(key, cx);
    }

    /// Asks the daemon to take the message `key`.
    fn ask(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.iter().find(|p| p.key == key) else {
            return;
        };
        self.sending = true;
        let command = Command::Reply {
            session: self.session.clone(),
            key,
            message: pending.text.clone(),
            images: pending
                .images
                .iter()
                .map(|image| Picture {
                    mime: image.format.mime_type().to_owned(),
                    bytes: image.bytes.clone(),
                })
                .collect(),
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// Sends again a message the daemon refused: it shows as being sent again, in its place.
    pub fn retry(&mut self, key: u64, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let Some(pending) = self.pending.iter_mut().find(|p| p.key == key) else {
            return;
        };
        if pending.refused.take().is_none() {
            return;
        }
        pending.accepted = None;
        pending.since = None;
        pending.queued = self.running;
        self.ask(key, cx);
    }

    /// Puts a message the daemon refused back in the composer: it leaves the conversation.
    pub fn edit_pending(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self
            .pending
            .iter()
            .position(|p| p.key == key && p.refused.is_some())
        else {
            return;
        };
        let pending = self.pending.remove(at);
        let pictures = pending
            .images
            .iter()
            .map(|image| Picture {
                mime: image.format.mime_type().to_owned(),
                bytes: image.bytes.clone(),
            })
            .collect();
        self.restore(&pending.text, pictures, window, cx);
    }

    /// The message `key` is recorded: it is shown as the daemon has it now.
    pub fn resolve(&mut self, key: u64, cx: &mut Context<Self>) -> Option<Pending> {
        let at = self.pending.iter().position(|p| p.key == key)?;
        cx.notify();
        Some(self.pending.remove(at))
    }

    /// A waiting message taken back to be changed: its text goes at the end of what the field
    /// holds, so that nothing typed is lost, and its pictures join the ones already attached.
    pub fn restore(
        &mut self,
        text: &str,
        pictures: Vec<Picture>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed = self.field.read(cx).value().to_string();
        let joined = if typed.trim().is_empty() {
            text.to_owned()
        } else {
            format!("{}\n{text}", typed.trim_end())
        };
        self.has_text = !joined.trim().is_empty();
        self.field
            .update(cx, |field, cx| field.set_value(joined.clone(), window, cx));
        self.typed(&joined, cx);
        // Put back by code, not typed: a message that ends with a command is not asking for the
        // list, and Enter sends it.
        self.dismissed = self.slash.is_some();
        self.list_commands(cx);
        self.attached.restore(pictures, cx, Self::attached_of);
        let focus = self.field.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// What the daemon answered for the message `key`: it waits to be recorded, or it stays
    /// where it is, refused, with why.
    pub fn replied(&mut self, key: u64, answer: Result<Accepted, String>, cx: &mut Context<Self>) {
        self.sending = false;
        // An answer to a message answered already (said refused by the agent's end) is late.
        let Some(at) = self
            .pending
            .iter()
            .position(|p| p.key == key && p.in_flight())
        else {
            return cx.notify();
        };
        match answer {
            Ok(accepted) => {
                let left = self.left.remove(&accepted.delivery);
                self.pending[at].accepted = Some(accepted);
                match left {
                    Some(Some(why)) => self.pending[at].refused = Some(why),
                    Some(None) => {
                        self.pending.remove(at);
                    }
                    None => {}
                }
            }
            Err(why) => self.pending[at].refused = Some(why),
        }
        cx.notify();
    }

    /// A delivery left the queue: taken back (`why` is `None`), or never to be read, and why.
    /// The message it is leaves, or stays refused; one not answered yet finds it on its answer.
    pub fn delivery_left(&mut self, delivery: &str, why: Option<String>, cx: &mut Context<Self>) {
        let Some(at) = self.pending.iter().position(|p| p.is(delivery)) else {
            if self.pending.iter().any(Pending::in_flight) {
                self.left.insert(delivery.to_owned(), why);
            }
            return;
        };
        match why {
            Some(why) => self.pending[at].refused = Some(why),
            None => {
                self.pending.remove(at);
            }
        }
        cx.notify();
    }

    /// The root ended or failed: what it took and has not read yet will never be, and says why.
    pub fn root_ended(&mut self, why: &str, cx: &mut Context<Self>) {
        let mut changed = false;
        for pending in &mut self.pending {
            if pending.accepted.is_some() && pending.refused.is_none() {
                pending.refused = Some(why.to_owned());
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Stamps the messages just sent with the last event of the conversation shown.
    fn stamp(&mut self, last: u64) {
        for pending in &mut self.pending {
            pending.since.get_or_insert(last);
        }
    }
}

impl Render for Reply {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = fade("reply-focus", self.focused, HOVER, window, cx);
        self.renders += 1;
        let p = palette(cx);
        let can_send = (self.has_text || !self.attached.is_empty()) && !self.sending;
        let swap = fade(
            "reply-stop-swap",
            self.running && !can_send,
            HOVER,
            window,
            cx,
        );
        let strip = self
            .attached
            .strip(window, cx, |this, at, cx| this.detach(at, cx));
        let slash = self.render_slash(cx);
        let chips = self.render_chips(cx);
        Attachments::taken_by(div().id("reply").relative().w_full(), cx, Self::attached_of)
            // While the `/` list is open, the arrows move in it and Escape closes it, before the
            // field takes them.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.slash_open() {
                    cx.stop_propagation();
                    this.highlight(-1, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.slash_open() {
                    cx.stop_propagation();
                    this.highlight(1, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.slash_open() {
                    cx.stop_propagation();
                    this.dismissed = true;
                    cx.notify();
                }
            }))
            .child(gpui_kit::deferred(slash).priority(1))
            // Escape interrupts the turn that runs; with none running it is left to others.
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                if this.running && event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.interrupt(cx);
                }
            }))
            .p(space::SM)
            .rounded(px(16.))
            .bg(p.shell)
            .border_1()
            .border_color(p.line)
            .shadow_lg()
            .ui_text()
            .text_color(p.text)
            .child(strip)
            .child(
                div()
                    .id("reply-field")
                    .rounded(px(11.))
                    .bg(p.field)
                    .border_1()
                    .border_color(mix(p.field_line, p.focus_line, focus))
                    .min_h(px(60.))
                    .px(space::LG)
                    .py(space::MD)
                    .text_size(px(14.))
                    .line_height(px(21.))
                    .cursor_text()
                    .child(self.field.clone())
                    .on_click(cx.listener(|this, _, window, cx| {
                        let focus = this.field.read(cx).focus_handle(cx);
                        window.focus(&focus, cx);
                    }))
                    .test_support(),
            )
            .child(chips)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .pt(space::SM)
                    .px(space::XS)
                    .child(self.options.clone())
                    .child(self.mode.clone())
                    .child(Attachments::button(cx, Self::attached_of))
                    // What the root does while its turn runs, where the line above the field was.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_end()
                            .pr(space::MD)
                            .child(self.status_line.clone()),
                    )
                    .child(
                        // Stop takes Send's place while Send has nothing to send: the two cross-fade
                        // in one slot.
                        div()
                            .id("reply-send-slot")
                            .relative()
                            .flex_none()
                            .size(px(30.))
                            .when(swap < 1.0, |d| {
                                d.child(
                                    div().absolute().inset_0().opacity(1.0 - swap).child(
                                        Button::round("reply-send", Icon::Send, SEND)
                                            .disabled(!can_send)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.send(window, cx)
                                            })),
                                    ),
                                )
                            })
                            .when(swap > 0.0, |d| {
                                d.child(
                                    div().absolute().inset_0().opacity(swap).child(
                                        Button::round("reply-stop", Icon::Stop, STOP)
                                            .disabled(self.interrupting)
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.interrupt(cx)),
                                            ),
                                    ),
                                )
                            })
                            .test_support(),
                    ),
            )
            .test_support()
    }
}

/// What the root does while its turn runs (`Thinking…`, `Writing…`, `Running <tool>…`), in the
/// composer's bar. A change of words cross-fades: the old ones fade out as the new fade in.
pub struct StatusLine {
    current: Option<SharedString>,
    leaving: Option<SharedString>,
    /// How far the change is, 0 to 1.
    motion: Motion,
}

impl StatusLine {
    fn new() -> Self {
        Self {
            current: None,
            leaving: None,
            motion: Motion::settled(1.0),
        }
    }

    fn set(&mut self, status: Option<SharedString>, cx: &mut Context<Self>) {
        if self.current == status {
            return;
        }
        self.leaving = self.current.take();
        self.current = status;
        let now = cx.background_executor().now();
        self.motion
            .play(0.0, 1.0, Duration::ZERO, PANEL, now, cx.reduce_motion());
        cx.notify();
    }

    /// The words shown now.
    pub fn shown(&self) -> Option<&str> {
        self.current.as_deref()
    }
}

impl Render for StatusLine {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let now = cx.background_executor().now();
        let t = self.motion.value(now);
        if self.motion.is_running(now) {
            window.request_animation_frame();
        } else {
            self.leaving = None;
        }
        let line = |id: &'static str,
                    text: SharedString,
                    opacity: f32,
                    window: &mut Window,
                    cx: &mut App| {
            tipped_when_cut(
                div()
                    .id(id)
                    .aria_label(text.clone())
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .opacity(opacity)
                    .child(text.clone()),
                text,
                window,
                cx,
            )
        };
        // The new words take the room; the old ones leave over them, from the same edge.
        let current = self
            .current
            .clone()
            .map(|text| line("reply-status", text, t, window, cx));
        div()
            .relative()
            .flex()
            .justify_end()
            .children(current)
            .children(self.leaving.clone().map(|text| {
                let leaving = line("reply-status-leaving", text, 1.0 - t, window, cx);
                match self.current.is_some() {
                    true => div().absolute().right_0().child(leaving).into_any_element(),
                    false => leaving.into_any_element(),
                }
            }))
    }
}

/// What an agent runs on, as a line: its provider and its model.
pub fn runs_on(providers: &Providers, provider: &str, model: Option<&str>) -> String {
    format!("{provider} · {}", providers.model_line(provider, model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::StyleRefinement;

    fn call_allowed_by(by: Option<&str>) -> Call {
        Call {
            id: "c".into(),
            name: "run".into(),
            args: String::new(),
            path: None,
            state: CallState::Done,
            since: 0,
            until: Some(1),
            allowed_by: by.map(str::to_owned),
            cut: None,
            shows: false,
            asked: true,
        }
    }

    #[test]
    fn the_folded_steps_say_who_allowed_them_the_judge_or_full_mode() {
        let line = |by: &[Option<&str>]| {
            allowed_said(&by.iter().map(|by| call_allowed_by(*by)).collect::<Vec<_>>())
        };
        assert_eq!(line(&[None, None]), "");
        assert_eq!(
            line(&[Some("the judge"), Some("the judge")]),
            " · allowed by the judge"
        );
        assert_eq!(
            line(&[Some("Full mode"), Some("Full mode")]),
            " · allowed by Full mode"
        );
        assert_eq!(
            line(&[
                Some("Full mode"),
                None,
                Some("the judge"),
                Some("Full mode")
            ]),
            " · 2 allowed by Full mode · 1 allowed by the judge"
        );
    }

    #[test]
    fn a_markdown_table_takes_its_surface_and_inks_from_the_palette() {
        let dark = Palette::base();
        let mut light = Palette::base();
        (light.bg, light.card, light.card2, light.text, light.mute) = (
            flottille_ui::color::hex(0xfafafa),
            flottille_ui::color::hex(0xf0f0f0),
            flottille_ui::color::hex(0xe6e6e6),
            flottille_ui::color::hex(0x1c1c1c),
            flottille_ui::color::hex(0x555555),
        );
        for (p, muted) in [(dark, false), (dark, true), (light, false), (light, true)] {
            let style = markdown_style(&p, muted);
            let ink = if muted { p.mute } else { p.text };
            let reference = StyleRefinement::default();
            assert_eq!(
                style.table().background,
                reference.clone().bg(p.card).background,
                "the frame is the card colour"
            );
            assert_eq!(
                style.table_cell().text.color,
                reference.text_color(ink).text.color,
                "the cells' ink is the reply's"
            );
            assert_eq!(style.code_background(), p.card2, "the header row");
            assert_eq!(style.border(), p.line);
        }
    }

    #[test]
    fn a_typed_url_is_drawn_as_typed_and_the_marks_around_it_are_escaped() {
        let url = "https://bitbucket.org/krinestt/flottille-sandbox/pull-requests/1";
        assert_eq!(
            as_typed(&format!("see {url} now")),
            format!("see {url} now")
        );
        assert_eq!(
            as_typed(&format!("*see* {url}.")),
            format!("\\*see\\* {url}\\."),
            "the full stop is the sentence's, not the address's"
        );
        assert_eq!(
            as_typed(&format!("(a-b: {url}) and {url}")),
            format!("\\(a\\-b: {url}\\) and {url}")
        );
        assert_eq!(
            as_typed("https://en.wikipedia.org/wiki/Foo_(bar) a_b www.example.com/a-b_c"),
            "https://en.wikipedia.org/wiki/Foo_(bar) a\\_b www.example.com/a-b_c"
        );
        assert_eq!(
            as_typed("xhttps://a.b-c"),
            "xhttps://a\\.b\\-c",
            "a word that ends in a scheme is no address"
        );
    }

    #[test]
    fn a_command_is_inline_code_in_the_text_as_typed_and_the_rest_keeps_its_marks() {
        let text = "check\n\n- one /open-prs\n- two\n\n```\nlet a = 1;\n``` then /ship";
        let chips = command_chips(text, &["open-prs".into(), "ship".into()]);
        assert_eq!(chips.len(), 2);

        assert_eq!(
            typed_with_chips(text, &chips),
            "check\n\n\\- one `/open-prs`\\\n\\- two\n\n\\`\\`\\`\\\nlet a = 1;\\\n\\`\\`\\` then `/ship`",
        );
        assert_eq!(typed_with_chips(text, &[]), as_typed(text));
    }

    fn agent(parent: Option<&str>, state: &str) -> TreeAgent {
        TreeAgent {
            id: "a1".into(),
            parent: parent.map(str::to_owned),
            depth: u64::from(parent.is_some()),
            role: "lead".into(),
            title: Some("build the form".into()),
            state: state.into(),
            state_reason: None,
            provider: "claude".into(),
            model: None,
            effort: None,
            pending: crate::session_link::Pending::default(),
            doing: String::new(),
            waiting_on: None,
            needs_user: false,
            questions: Vec::new(),
            resumable: false,
            stopping: false,
            images: None,
            gate_mode: Some("auto".into()),
            context: None,
            context_tokens: None,
            queued: Vec::new(),
            tools_outdated: false,
            tools_reload: false,
        }
    }

    #[test]
    fn the_older_list_of_tools_is_said_only_of_a_root_that_runs() {
        let mut root = agent(None, "idle");
        assert_eq!(older_tools(Some(&root)), None);
        root.tools_outdated = true;
        assert_eq!(older_tools(Some(&root)), Some(OLDER_TOOLS));
        for ended in ["ended", "failed"] {
            root.state = ended.into();
            assert_eq!(older_tools(Some(&root)), None, "{ended}");
        }
        assert_eq!(older_tools(None), None);
    }

    #[test]
    fn only_a_root_needs_the_user_and_a_child_waiting_on_its_parent_waits() {
        let mut root = agent(None, "working");
        root.needs_user = true;
        assert_eq!(state_of(&root), AgentState::NeedsYou);
        let mut child = agent(Some("r"), "working");
        child.needs_user = true;
        child.waiting_on = Some("parent".into());
        assert_eq!(state_of(&child), AgentState::Waiting);
        child.waiting_on = None;
        assert_eq!(state_of(&child), AgentState::Running);
    }

    #[test]
    fn an_ended_child_that_reported_is_completed_else_stopped() {
        let mut child = agent(Some("r"), "ended");
        assert_eq!(state_of(&child), AgentState::Stopped);
        child.state_reason = Some("it reported".into());
        assert_eq!(state_of(&child), AgentState::Completed);
        assert_eq!(state_of(&agent(Some("r"), "failed")), AgentState::Failed);
        assert_eq!(state_of(&agent(None, "idle")), AgentState::Idle);
    }

    #[test]
    fn a_root_is_named_by_its_role_and_a_child_by_its_task() {
        assert_eq!(name_of(&agent(None, "idle")), "lead");
        assert_eq!(name_of(&agent(Some("r"), "idle")), "build the form");
    }

    fn node<'a>(id: &'a str, parent: Option<&'a str>, ended: bool) -> Node<'a> {
        Node { id, parent, ended }
    }

    fn agent_row(id: &str) -> RailRow {
        RailRow::Agent(id.to_owned())
    }

    fn ended_row(parent: &str, count: usize, open: bool) -> RailRow {
        RailRow::Ended {
            parent: parent.to_owned(),
            count,
            open,
        }
    }

    #[test]
    fn a_running_child_with_ended_children_of_its_own_gathers_them_under_it() {
        let nodes = [
            node("lead", None, false),
            node("mid", Some("lead"), false),
            node("a", Some("mid"), true),
            node("b", Some("mid"), true),
            node("live", Some("mid"), false),
            node("old", Some("lead"), true),
        ];
        assert_eq!(
            rail_rows(&nodes, &HashSet::new()),
            [
                agent_row("lead"),
                agent_row("mid"),
                agent_row("live"),
                ended_row("mid", 2, false),
                ended_row("lead", 1, false),
            ]
        );
        let open: HashSet<String> = ["mid".to_owned()].into();
        assert_eq!(
            rail_rows(&nodes, &open)[3..],
            [
                ended_row("mid", 2, true),
                agent_row("a"),
                agent_row("b"),
                ended_row("lead", 1, false)
            ]
        );
    }

    #[test]
    fn an_ended_child_with_a_child_still_running_keeps_its_place() {
        let nodes = [
            node("lead", None, false),
            node("gone", Some("lead"), true),
            node("deep", Some("gone"), false),
            node("old", Some("lead"), true),
            node("older", Some("old"), true),
        ];
        assert_eq!(
            rail_rows(&nodes, &HashSet::new()),
            [
                agent_row("lead"),
                agent_row("gone"),
                agent_row("deep"),
                ended_row("lead", 2, false),
            ]
        );
    }

    #[test]
    fn an_ended_root_and_a_child_that_did_not_end_keep_their_place() {
        let nodes = [node("lead", None, true), node("c", Some("lead"), false)];
        assert_eq!(
            rail_rows(&nodes, &HashSet::new()),
            [agent_row("lead"), agent_row("c")]
        );
    }

    #[test]
    fn a_decided_card_folds_into_one_line() {
        let card = ApprovalCard {
            number: 4,
            base: false,
            judgement: None,
            kind: "call".into(),
            says: Vec::new(),
            asked: Some("Read .env".into()),
            call: None,
            why: None,
            source: None,
            asker: Some("lead".into()),
            decision: Some("once".into()),
        };
        assert_eq!(card.record().as_deref(), Some("Allowed · Read .env · lead"));
        let pending = ApprovalCard {
            decision: None,
            ..card
        };
        assert_eq!(pending.record(), None);
    }

    #[test]
    fn a_message_s_time_is_its_date_and_its_time_in_the_local_zone() {
        assert_eq!(said_at_offset(1_793_455_500_000, 0), "31 Oct 2026, 14:05");
        assert_eq!(
            said_at_offset(1_793_455_500_000, 3600),
            "31 Oct 2026, 15:05"
        );
        assert_eq!(said_at_offset(1_709_251_140_000, 0), "29 Feb 2024, 23:59");
        assert_eq!(said_at_offset(1_709_251_140_000, 60), "1 Mar 2024, 00:00");
        assert_eq!(said_at_offset(0, -60), "31 Dec 1969, 23:59");
    }

    #[test]
    fn durations_are_said_short() {
        assert_eq!(seconds(2_000), "2.0s");
        assert_eq!(seconds(38_400), "38s");
        assert_eq!(seconds(240_000), "4m");
    }

    #[test]
    fn a_long_reply_is_cut_where_a_block_starts_and_its_pieces_make_it_whole() {
        let paragraph = "word ".repeat(200) + "\n\n";
        let fence = format!("```\n{}\n\nnot a cut\n```\n\n", "code ".repeat(1000));
        let items: String = (0..400).map(|n| format!("- item {n}\n")).collect();
        let text = format!(
            "{}{fence}{}{items}  indented\n",
            paragraph.repeat(5),
            paragraph.repeat(5)
        );

        let pieces = markdown_pieces(&text);

        assert_eq!(pieces.concat(), text);
        assert!(pieces.len() > 2, "{}", pieces.len());
        for piece in &pieces[1..] {
            assert!(
                piece.starts_with("word")
                    || piece.starts_with("```")
                    || piece.starts_with("- item"),
                "{:?}",
                &piece[..20]
            );
        }
        // Inside a code block, after a blank line, nothing is cut.
        assert!(pieces.iter().all(|p| !p.starts_with("not a cut")));
        assert!(pieces.iter().any(|p| p.contains("\n\nnot a cut\n```")));
    }

    #[test]
    fn a_short_reply_is_one_piece() {
        assert_eq!(
            markdown_pieces("**a**\n\n- b\n- c\n"),
            ["**a**\n\n- b\n- c\n"]
        );
        assert_eq!(markdown_pieces(""), [""]);
    }

    #[test]
    fn an_image_in_a_reply_is_never_fetched() {
        let source = no_image(&SharedUri::from("https://example.com/x.png"));
        assert!(matches!(source, ImageSource::Custom(_)));
    }
}
