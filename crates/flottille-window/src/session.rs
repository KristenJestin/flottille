//! A session's page: a header on one line (title, place, state pill), the
//! shown agent's conversation, live, the composer that writes to the root agent (or, reading a
//! sub-agent, a line saying it belongs to its parent; once the session ended, a line saying why,
//! with Resume when it can be), and a rail with the session's tree of
//! agents, its workspace and its facts, which becomes a drawer in a narrower window.
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
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use flottille_ui::motion::{Motion, PANEL};
use flottille_ui::{
    AgentState, Badge, Button, Card, Drawer, Icon, ListRow, Menu, Modal, Palette, Pill,
    PulsingLabel, Side, StateIcon, Status, Type, enter, fold_frame, icon, palette, section_label,
};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::base::input::{
    Escape, Input, InputEditorStyle, InputEvent, InputState, MoveDown, MoveUp, Paste, TextareaState,
};
use gpui_kit::base::text::{TextView, TextViewState, TextViewStyle};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, ElementId, Entity, EventEmitter,
    Focusable as _, FollowMode, ImageCacheError, ImageSource, InteractiveElement, IntoElement,
    ListAlignment, ListState, ParentElement, PathBuilder, Pixels, Render, SharedString, SharedUri,
    Stateful, StatefulInteractiveElement, Styled, StyledImage as _, Subscription, Task,
    TestSupportExt as _, Transformation, WeakEntity, Window, canvas, div, list, point, px, radians,
};

use crate::actions::{Answer, Command, Picture};
use crate::commands::{self, CommandList, CommandPanel};
use crate::conversation::{Activity, Block, Call, CallState, Change, Conversation, DiffLine, USER};
use crate::link::{Connect, Update};
use crate::mode::ModeSwitch;
use crate::model::Place;
use crate::open::Opener;
use crate::procedures::ProcedureInfo;
use crate::session_link::{
    self, PlaceFacts, PullRequest, RepositoryFacts, Request, SessionLink, SessionUpdate, TreeAgent,
    Where,
};
use crate::shell::{Layout, Shell, now_ms};
use crate::tree::place_name;

/// Width of the session page's right rail.
pub const RAIL: f32 = 290.;

/// How the screen names the root agent, which has no name of its own there.
pub const THE_CONVERSATION: &str = "the conversation";

/// The reply field's placeholder and its send button's tooltip: the field writes to the root.
pub const REPLY_PLACEHOLDER: &str = "Reply…";
pub const SEND: &str = "Send";
/// The composer's button that interrupts the turn that runs.
pub const STOP: &str = "Stop the turn";
/// The reply field is one line high when empty, and grows with its lines up to this many.
const REPLY_MAX_ROWS: usize = 10;
/// What the composer says of a picture it does not attach.
pub const NO_IMAGES: &str = "This agent takes no images, so the picture was not attached.";
const IMAGE_KINDS: &str = "Flottille takes png, jpeg, gif and webp images.";
const TOO_MANY_IMAGES: &str = "A message carries 4 images at most.";
const IMAGE_TOO_BIG: &str = "An image can be 5 MiB at most.";
const MAX_IMAGES: usize = 4;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

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
}

pub struct SessionPage {
    shell: WeakEntity<Shell>,
    opened: Opened,
    follow: Follow,
    layout: Layout,
    /// The session's state, as the shell's tree draws it.
    status: Entity<Status>,
    pill: Entity<Pill>,
    /// What the header's pill shows: the left tree's state, or the root's once it ended.
    pill_status: Entity<Status>,
    /// The agent whose conversation is shown: the root, or a sub-agent read.
    shown: String,
    link: Option<SessionLink>,
    tree: Vec<TreeAgent>,
    /// An agent was shown before the tree was read: `set_tree` checks that it is in it.
    unchecked: bool,
    /// One state per agent, read by its rail row and its line in the sub-agents list.
    statuses: HashMap<String, Entity<Status>>,
    conversation: Entity<ConversationView>,
    rail: Entity<SessionRail>,
    drawer: Entity<Drawer>,
    reply: Entity<Reply>,
    /// The panel that follows one command's output.
    command_panel: Entity<CommandPanel>,
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
    renders: usize,
    _status: Subscription,
}

/// Emitted when the user chooses the conversation of an agent of the session.
pub struct Chose(pub String);

impl EventEmitter<Chose> for SessionPage {}

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
        let drawer = cx.new(|_| Drawer::new("rail-drawer", Side::Right, px(RAIL), rail.clone()));
        let pill = cx.new(|cx| Pill::new(status.clone(), cx).quiet_running());
        let shown = opened.id.clone();
        let conversation =
            cx.new(|_| ConversationView::new(page, &shown, THE_CONVERSATION.into(), None));
        let _status = cx.observe(&status, |this, status, cx| {
            let needs_you = status.read(cx).state() == AgentState::NeedsYou;
            if this.needs_you != needs_you {
                this.needs_you = needs_you;
                cx.notify();
            }
        });
        let needs_you = status.read(cx).state() == AgentState::NeedsYou;
        let mut page = Self {
            shell,
            follow,
            layout,
            pill_status: status.clone(),
            status,
            pill,
            shown,
            link: None,
            tree: Vec::new(),
            unchecked: false,
            statuses: HashMap::new(),
            conversation,
            rail,
            drawer,
            reply,
            command_panel,
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
            renders: 0,
            _status,
            opened,
        };
        page.relink();
        page
    }

    pub fn id(&self) -> &str {
        &self.opened.id
    }

    /// The session's state, as the shell's tree draws it.
    pub fn status(&self) -> &Entity<Status> {
        &self.status
    }

    /// The state the header's pill shows.
    pub fn header_state(&self, cx: &gpui_kit::App) -> AgentState {
        self.pill_status.read(cx).state()
    }

    /// The agent whose conversation is shown.
    pub fn shown(&self) -> &str {
        &self.shown
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

    /// The rail's Commands section.
    pub fn commands(&self, cx: &gpui_kit::App) -> Entity<CommandList> {
        self.rail.read(cx).commands.clone()
    }

    /// The panel that follows a command's output.
    pub fn command_panel(&self) -> &Entity<CommandPanel> {
        &self.command_panel
    }

    pub fn drawer(&self) -> &Entity<Drawer> {
        &self.drawer
    }

    pub fn conversation(&self) -> &Entity<ConversationView> {
        &self.conversation
    }

    /// The composer; `None` while a sub-agent is read: nothing writes to it.
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
        Some(match self.end_count() {
            0 | 1 => format!("This ends {THE_CONVERSATION}. Its workspace stays."),
            n => format!(
                "This ends {n} agents: {THE_CONVERSATION} and its {} sub-agent{}. Workspaces \
                 stay.",
                n - 1,
                if n == 2 { "" } else { "s" }
            ),
        })
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

    /// Opens a command's output in its panel, followed live.
    pub fn open_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.watching = Some(run.to_owned());
        if let Some(link) = &self.link {
            link.watch(Some(run.to_owned()));
        }
        self.command_panel
            .update(cx, |panel, cx| panel.open(run, cx));
    }

    /// Closes the panel: the output is read no more.
    pub fn close_command(&mut self, cx: &mut Context<Self>) {
        self.watching = None;
        if let Some(link) = &self.link {
            link.watch(None);
        }
        self.command_panel.update(cx, |panel, cx| panel.close(cx));
    }

    /// The user stops a command: its whole group, and the agent that started it is told.
    pub fn stop_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.run_command(run, false, cx);
    }

    /// The user dismisses a command that ended.
    pub fn dismiss_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.run_command(run, true, cx);
    }

    fn run_command(&mut self, run: &str, dismiss: bool, cx: &mut Context<Self>) {
        let list = self.commands(cx);
        list.update(cx, |list, cx| list.failed(None, cx));
        self.perform(
            Command::Run {
                session: self.opened.id.clone(),
                run: run.to_owned(),
                dismiss,
            },
            cx,
        );
    }

    /// What the daemon answered to a stop or a dismissal: why, when it refused.
    pub fn command_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        let list = self.commands(cx);
        list.update(cx, |list, cx| list.failed(refused, cx));
    }

    /// Reads one more page of the shown agent's conversation, before those shown.
    pub fn show_earlier(&mut self, cx: &mut Context<Self>) {
        self.pages += 1;
        self.relink();
        cx.notify();
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
        self.conversation = cx.new(|_| ConversationView::new(page, &shown, name, parent));
        self.rail
            .update(cx, |rail, cx| rail.set_selected(agent.to_owned(), cx));
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
                    self.note_usage(event, cx);
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
            // One row added or drawn again: nothing else is looked at.
            SessionUpdate::Live(event) => {
                self.note_usage(&event, cx);
                self.note_decision(&event);
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
                if self.read {
                    self.refresh_conversation(cx);
                }
            }
            SessionUpdate::Decided { number, decision } => {
                self.decisions.insert(number, decision);
                self.refresh_conversation(cx);
            }
            SessionUpdate::Place(facts) => {
                self.rail
                    .update(cx, |rail, cx| rail.set_place(facts.map(|facts| *facts), cx));
            }
            SessionUpdate::Commands(rows) => {
                let list = self.commands(cx);
                list.update(cx, |list, cx| list.set(rows, cx));
                self.sync_command_cards(cx);
            }
            SessionUpdate::CommandOutput(output) => {
                self.command_panel
                    .update(cx, |panel, cx| panel.set(*output, cx));
            }
        }
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
        let row = self.conversation.read(cx).approval_row(number, cx);
        if let Some(row) = row {
            row.update(cx, |row, cx| row.answered(refused, cx));
        }
        self.refresh_conversation(cx);
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
                Block::Images { names } => Some(names.clone()),
                _ => None,
            })
            .flatten()
            .filter(|name| !self.asked_pictures.contains(name))
            .collect();
        for name in wanted {
            self.asked_pictures.insert(name.clone());
            let command = Command::Image {
                session: self.shown.clone(),
                name,
            };
            // The shell is handing this page its updates: it asks once they are over.
            crate::procedures::perform_later(&self.shell, command, cx);
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
        self.pictures.insert(
            name.clone(),
            Arc::new(gpui_kit::Image::from_bytes(format, image.bytes)),
        );
        let rows = self.conversation.read(cx).rows.clone();
        for row in rows {
            if matches!(&*row.read(cx).block, Block::Images { names } if names.contains(&name)) {
                row.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// How many pictures of the shown conversation are read.
    pub fn pictures_read(&self) -> usize {
        self.pictures.len()
    }

    fn note_usage(&self, event: &flottille_core::protocol::Event, cx: &mut Context<Self>) {
        if event.name != "agent.usage" || self.shown != self.opened.id {
            return;
        }
        let (used, size) = (event.data["used"].as_u64(), event.data["size"].as_u64());
        if let (Some(used), Some(size)) = (used, size.filter(|s| *s > 0)) {
            let context = used * 100 / size;
            let facts = self.reply.read(cx).facts.clone();
            facts.update(cx, |facts, cx| facts.set_context(Some(context), cx));
        }
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
            let runs_on = (root.provider.clone(), root.model.clone());
            let facts = self.reply.read(cx).facts.clone();
            facts.update(cx, |facts, cx| facts.set_model(runs_on, cx));
            let mode = self.reply.read(cx).mode.clone();
            let gate_mode = root.gate_mode.clone();
            mode.update(cx, |mode, cx| mode.set_mode(gate_mode, cx));
            let running = root.state == "working";
            let images = root.images;
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
                    depth: agent.depth,
                    parent: agent.parent.clone(),
                    ended: matches!(state_of(agent), AgentState::Completed | AgentState::Stopped),
                    status,
                })
            })
            .collect();
        self.rail.update(cx, |rail, cx| rail.set_agents(rows, cx));
        let list = self.commands(cx);
        list.update(cx, |list, cx| list.set_names(names.clone(), cx));
        let changed = self.tree != tree;
        let ended = |tree: &[TreeAgent]| {
            tree.iter()
                .find(|a| a.parent.is_none())
                .map(|root| matches!(root.state.as_str(), "ended" | "failed"))
        };
        let (was_ended, now_ended) = (ended(&self.tree), ended(&tree));
        self.tree = tree;
        if let Some(now_ended) = now_ended.filter(|now| was_ended != Some(*now)) {
            if was_ended.is_some() {
                // Ended or resumed: what was said before it is over.
                self.resume_error = None;
                self.reply.update(cx, |reply, cx| reply.clear_error(cx));
            }
            if !now_ended {
                self.resuming = false;
            }
            // Ended, the header says how, as the rail does; else what the left tree shows.
            let status = match now_ended {
                true => self
                    .root()
                    .and_then(|root| self.statuses.get(&root.id))
                    .cloned(),
                false => None,
            }
            .unwrap_or_else(|| self.status.clone());
            self.pill_status = status.clone();
            self.pill = cx.new(|cx| Pill::new(status, cx).quiet_running());
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
        }
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
                        }
                    }
                }
            }
        });
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
                    doing: agent.doing.clone(),
                    status: self.statuses.get(&agent.id)?.clone(),
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
            why: request.map(|r| {
                let mut why = r.reasons.join(" ");
                for (by, said) in &r.forwards {
                    why.push_str(&format!(" {} forwards it: {said}", self.said_name(by)));
                }
                why.trim().to_owned()
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .pl(px(if narrow { 10. } else { 24. }))
            .pr(px(12.))
            .flex()
            .items_center()
            .gap(px(10.))
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(14.))
                    .semibold()
                    .child(SharedString::from(self.opened.title.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(p.faint)
                    .child(
                        icon(match place {
                            None => Icon::Folder,
                            Some(_) => Icon::Branch,
                        })
                        .size(px(13.))
                        .text_color(p.faint),
                    )
                    .child(
                        div()
                            .mono(12.)
                            .child(SharedString::from(place_name(&place).to_owned())),
                    ),
            )
            .child(self.pill.clone())
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
        let dock = div()
            .flex_none()
            .w_full()
            .max_w(px(780.))
            .mx_auto()
            .px(px(if narrow { 12. } else { 28. }))
            .pb(px(if narrow { 12. } else { 20. }))
            .map(|d| match (root, self.ended_line()) {
                (true, None) => d.child(self.reply.clone()),
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
                        .child(self.rail.clone()),
                )
            }))
            .when(!wide, |d| d.child(self.drawer.clone()))
            .child(self.end_modal(cx))
            .child(self.command_panel.clone())
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
                Button::primary(
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
            .gap(px(6.))
            .child(
                div()
                    .id("session-ended")
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .border_1()
                    .border_dashed()
                    .border_color(p.line2)
                    .rounded(px(12.))
                    .pl(px(14.))
                    .pr(px(12.))
                    .py(px(10.))
                    .text_size(px(13.))
                    .text_color(p.mute)
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
                    .px(px(10.))
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
            .gap(px(10.))
            .border_1()
            .border_dashed()
            .border_color(p.line2)
            .rounded(px(12.))
            .pl(px(14.))
            .pr(px(12.))
            .py(px(10.))
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

/// The conversation: one row view per block, in a list that lays out and draws only the rows in
/// view, and follows its end unless the user scrolled up to read.
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
    /// How many times a row was looked at or changed, for the cost of an update.
    touched: usize,
    /// The list's items: "Show earlier" first when older events remain, then the rows, a long
    /// reply as one item per Markdown piece: the list draws only the pieces in view.
    list: ListState,
    /// The items of each row past its first: the pieces of its reply past the first.
    more: Vec<usize>,
    renders: usize,
}

impl ConversationView {
    fn new(page: WeakEntity<SessionPage>, id: &str, me: String, parent: Option<String>) -> Self {
        Self {
            page,
            id: id.to_owned(),
            model: Conversation::new(me, parent),
            rows: Vec::new(),
            specials: Vec::new(),
            earlier: false,
            touched: 0,
            list: Self::list(),
            more: Vec::new(),
            renders: 0,
        }
    }

    fn list() -> ListState {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        list
    }

    /// The list's first item of a row.
    fn item(&self, at: usize) -> usize {
        at + usize::from(self.earlier) + self.more[..at.min(self.more.len())].iter().sum::<usize>()
    }

    /// The row of the list's item `ix`, and which of its items it is.
    fn row_of(&self, ix: usize) -> Option<(usize, usize)> {
        let mut ix = ix.checked_sub(usize::from(self.earlier))?;
        for (at, more) in self.more.iter().enumerate() {
            if ix <= *more {
                return Some((at, ix));
            }
            ix -= more + 1;
        }
        None
    }

    /// Gives the row `at` one item per piece of its reply, and measures again its last.
    fn fit(&mut self, at: usize, cx: &gpui_kit::App) {
        let more = self.rows[at].read(cx).more_pieces().len();
        let (first, had) = (self.item(at), self.more[at]);
        if more != had {
            let kept = first + 1 + had.min(more);
            self.list
                .splice(kept..first + 1 + had, more.saturating_sub(had));
            self.more[at] = more;
        }
        self.list.remeasure_items(first + more..first + more + 1);
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

    /// How many times a row was looked at or changed so far.
    pub fn touched(&self) -> usize {
        self.touched
    }

    /// Whether older events than those shown remain.
    pub fn has_earlier(&self) -> bool {
        self.earlier
    }

    /// Makes the rows again from what a link read: the events are folded first, then each
    /// block gets its row.
    fn rebuild(&mut self, read: Read, page: &SessionPage, cx: &mut Context<Self>) {
        self.model = self.model.restarted();
        self.model.place_approvals(read.placed);
        for event in read.events {
            self.model.apply(event);
        }
        if read.launched_before {
            self.model.prepend_sub_agents();
        }
        self.earlier = read.earlier;
        self.rows.clear();
        self.specials.clear();
        self.more.clear();
        for at in 0..self.model.blocks().len() {
            self.add_row(at, page, cx);
            self.more[at] = self.rows[at].read(cx).more_pieces().len();
        }
        let items = self.item(self.rows.len());
        match read.follow {
            true => {
                self.list = Self::list();
                self.list.reset(items);
                self.list.scroll_to_end();
            }
            // Earlier pages read: the rows shown before stay where they were, below them.
            false => self.list.reset(items),
        }
        cx.notify();
    }

    fn add_row(&mut self, at: usize, page: &SessionPage, cx: &mut Context<Self>) {
        let block = self.model.blocks()[at].clone();
        if matches!(block, Block::SubAgents | Block::Approval { .. }) {
            self.specials.push(at);
        }
        let me = self.model.me().to_owned();
        let root = self.id == page.id();
        let row = Row::view(at, block, me, root, page, self.page.clone(), cx);
        self.rows.push(row);
        self.more.push(0);
        self.touched += 1;
    }

    fn approval_row(&self, number: u64, cx: &gpui_kit::App) -> Option<Entity<Row>> {
        self.specials
            .iter()
            .map(|at| &self.rows[*at])
            .find(|row| *row.read(cx).block == Block::Approval { number })
            .cloned()
    }

    /// Adds a row for each block added, and draws again each row whose block changed.
    fn changed(&mut self, changes: &[Change], page: &SessionPage, cx: &mut Context<Self>) {
        let mut added = false;
        for change in changes {
            match change {
                Change::Added => {
                    let (at, item) = (self.rows.len(), self.item(self.rows.len()));
                    self.add_row(at, page, cx);
                    self.list.splice(item..item, 1);
                    self.fit(at, cx);
                    added = true;
                }
                Change::Changed(at) => {
                    let block = self.model.blocks()[*at].clone();
                    if !matches!(block, Block::SubAgents | Block::Approval { .. }) {
                        // A request answered by a card has no card of its own to bring up to date.
                        self.specials.retain(|special| special != at);
                    }
                    if let Some(row) = self.rows.get(*at).cloned() {
                        self.touched += 1;
                        row.update(cx, |row, cx| row.set_block(block, cx));
                        let item = self.item(*at);
                        self.list.remeasure_items(item..item + 1);
                        self.fit(*at, cx);
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
        if added {
            cx.notify();
        }
    }

    /// The list's item `ix`: "Show earlier", a row, or a piece of a row's long reply, in the
    /// conversation's column.
    fn render_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let narrow = window.viewport_size().width < px(crate::shell::NARROW);
        let (at, piece) = self.row_of(ix).unwrap_or((usize::MAX, 0));
        let more = self.more.get(at).copied().unwrap_or(0);
        // Between the pieces of one reply, the gap of its paragraphs.
        let below = match () {
            _ if piece < more => TextViewStyle::default()
                .paragraph_gap()
                .to_pixels(window.rem_size()),
            _ if ix + 1 == self.item(self.rows.len()) => px(20.),
            _ => px(18.),
        };
        let column = div()
            .max_w(px(780.))
            .w_full()
            .mx_auto()
            .px(px(if narrow { 16. } else { 28. }))
            .when(ix == 0, |d| d.pt(px(if narrow { 20. } else { 28. })))
            .pb(below);
        // A list's item is not stretched to its width: the column is centred in a full one.
        let centred = |column| div().w_full().child(column).into_any_element();
        if self.earlier && ix == 0 {
            let page = self.page.clone();
            return centred(column.flex().justify_center().child(
                Button::quiet("show-earlier", "Show earlier").on_click(move |_, _, cx| {
                    page.update(cx, |page, cx| page.show_earlier(cx)).ok();
                }),
            ));
        }
        let Some(row) = self.rows.get(at) else {
            return div().into_any_element();
        };
        // A request answered by a card below it: no row, not even the gap between two.
        if matches!(row.read(cx).block(), Block::Answered { .. }) {
            return div().into_any_element();
        }
        if piece > 0 {
            let piece = row.read(cx).more_pieces().get(piece - 1).cloned();
            return centred(column.children(piece));
        }
        centred(column.child(enter(
            ElementId::Name(format!("enter-row-{}-{at}", self.id).into()),
            0,
            row.clone(),
        )))
    }
}

impl Render for ConversationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let narrow = window.viewport_size().width < px(crate::shell::NARROW);
        let id = SharedString::from(format!("conversation-{}", self.id));
        if self.rows.is_empty() && !self.earlier {
            return div()
                .id(id)
                .size_full()
                .child(
                    div()
                        .max_w(px(780.))
                        .w_full()
                        .mx_auto()
                        .px(px(if narrow { 16. } else { 28. }))
                        .pt(px(if narrow { 20. } else { 28. }))
                        .text_color(p.faint)
                        .child("Nothing said yet."),
                )
                .into_any_element();
        }
        div()
            .id(id)
            .size_full()
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix, window, cx| this.render_item(ix, window, cx)),
                )
                .size_full(),
            )
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
    pub doing: String,
    pub status: Entity<Status>,
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
    pub why: Option<String>,
    pub source: Option<String>,
    pub asker: Option<String>,
    /// `deny`, `once` or `cancelled`, once decided.
    pub decision: Option<String>,
}

impl ApprovalCard {
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
    /// A thought or a report unfolded.
    open: bool,
    /// The reply or the thought as Markdown, in the pieces [`markdown_pieces`] cuts: a piece
    /// streamed parses the last one again, never the whole reply.
    markdown: Vec<Entity<MarkdownPiece>>,
    thinking: Option<(Entity<PulsingLabel>, Entity<ThinkingTimer>)>,
    calls: Vec<(Entity<Status>, Entity<StateIcon>)>,
    sub_agents: Vec<SubAgent>,
    card: Option<ApprovalCard>,
    icons: HashMap<String, Entity<StateIcon>>,
    pills: HashMap<String, Entity<Pill>>,
    busy: bool,
    error: Option<String>,
    renders: usize,
}

impl Row {
    fn view(
        at: usize,
        block: Block,
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
        cx.new(|cx| {
            let mut row = Row {
                at,
                me,
                root,
                page: weak,
                block: Rc::new(Block::Text {
                    text: String::new(),
                }),
                open: false,
                markdown: Vec::new(),
                thinking: None,
                calls: Vec::new(),
                sub_agents,
                card,
                icons: HashMap::new(),
                pills: HashMap::new(),
                busy: false,
                error: None,
                renders: 0,
            };
            row.set_block(block, cx);
            row
        })
    }

    pub fn block(&self) -> &Block {
        &self.block
    }

    /// The line drawn over the row, when it has one: who sent a message (none for the user's:
    /// the bubble says it), whose report it is, who launched the sub-agents, who asked a request.
    pub fn label(&self) -> Option<String> {
        match &*self.block {
            Block::Message { from, .. } if from != USER => Some(from.clone()),
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

    /// The text a reply or a thought shows, its Markdown drawn: as of the last parse that landed.
    pub fn rendered(&self, cx: &gpui_kit::App) -> Option<String> {
        let pieces = self
            .markdown
            .iter()
            .map(|piece| piece.read(cx).state.read(cx));
        let rendered = pieces.map(|state| state.rendered_text().as_str().to_owned());
        matches!(*self.block, Block::Text { .. } | Block::Thought { .. })
            .then(|| rendered.collect())
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
    fn set_markdown(&mut self, text: &str, muted: bool, cx: &mut Context<Self>) {
        let pieces = markdown_pieces(text);
        for (at, piece) in pieces.iter().enumerate() {
            match self.markdown.get(at) {
                Some(markdown) => {
                    let state = markdown.read(cx).state.clone();
                    state.update(cx, |state, cx| state.set_text(piece, cx));
                }
                None => {
                    let state = cx.new(|cx| TextViewState::markdown(piece, cx));
                    self.markdown
                        .push(cx.new(|_| MarkdownPiece { state, muted }));
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

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The state each call's mark shows.
    pub fn marks(&self, cx: &gpui_kit::App) -> Vec<AgentState> {
        self.calls.iter().map(|(s, _)| s.read(cx).state()).collect()
    }

    fn set_block(&mut self, block: Block, cx: &mut Context<Self>) {
        if let Block::Text { text } | Block::Thought { text, .. } = &block {
            let muted = matches!(block, Block::Thought { .. });
            self.set_markdown(text, muted, cx);
        }
        match &block {
            Block::Thought { since, until, .. } => {
                let (since, until) = (*since, *until);
                match &self.thinking {
                    None => {
                        let label = cx.new(|cx| {
                            PulsingLabel::new(
                                if until.is_none() {
                                    "Thinking"
                                } else {
                                    "Thought for"
                                },
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
                            label.set_text("Thought for", cx);
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
            self.set_markdown(&text, muted, cx);
            cx.notify();
        }
    }

    fn answer(&mut self, answer: Answer, cx: &mut Context<Self>) {
        let Some(card) = &self.card else {
            return;
        };
        if self.busy || card.decision.is_some() {
            return;
        }
        self.busy = true;
        self.error = None;
        let number = card.number;
        if let Some(page) = self.page.upgrade() {
            let (shell, session) = {
                let page = page.read(cx);
                (page.shell.clone(), page.opened.id.clone())
            };
            shell
                .update(cx, |shell, _| {
                    shell.perform(Command::Answer {
                        session,
                        number,
                        answer,
                    })
                })
                .ok();
        }
        cx.notify();
    }

    fn answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.busy = false;
        self.error = refused;
        cx.notify();
    }
}

/// A reply or a thought, its Markdown pieces one under the other.
fn markdown_view(pieces: &[Entity<MarkdownPiece>]) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(TextViewStyle::default().paragraph_gap())
        .children(pieces.iter().cloned())
}

/// One piece of a reply's or a thought's Markdown, its own view: a piece streamed draws the last
/// one again, and the others as they were.
pub struct MarkdownPiece {
    state: Entity<TextViewState>,
    /// A thought's, in the muted ink.
    muted: bool,
}

impl Render for MarkdownPiece {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        TextView::new(&self.state)
            .style(markdown_style(&palette(cx), self.muted))
            .image_source(no_image)
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

fn call_state(call: &Call) -> AgentState {
    match call.state {
        CallState::Running => AgentState::Running,
        CallState::Done => AgentState::Completed,
        CallState::Failed => AgentState::Failed,
        CallState::Interrupted => AgentState::Stopped,
    }
}

impl Render for Row {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("row-{}-{part}", self.at).into());
        // The row's own block, not a copy of it.
        let block = self.block.clone();
        match &*block {
            Block::Message { text, .. } => div()
                .id(id("message"))
                .flex()
                .flex_col()
                .items_end()
                .child(
                    div()
                        .max_w(px(540.))
                        .bg(p.card2)
                        .rounded_tl(px(14.))
                        .rounded_tr(px(14.))
                        .rounded_bl(px(14.))
                        .rounded_br(px(4.))
                        .px(px(14.))
                        .py(px(10.))
                        .when_some(self.label(), |d, from| {
                            d.child(
                                div()
                                    .id(id("message-from"))
                                    .flex()
                                    .justify_end()
                                    .mono(11.5)
                                    .text_color(p.faint)
                                    .mb(px(5.))
                                    .child(SharedString::from(from))
                                    .test_support(),
                            )
                        })
                        .child(SharedString::from(text.clone())),
                )
                .test_support()
                .into_any_element(),
            Block::Images { names } => {
                let page = self.page.upgrade();
                let pictures: Vec<Option<Arc<gpui_kit::Image>>> = names
                    .iter()
                    .map(|name| page.as_ref()?.read(cx).pictures.get(name).cloned())
                    .collect();
                div()
                    .id(id("images"))
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(8.))
                    .children(pictures.into_iter().enumerate().map(|(n, picture)| {
                        let thumb = div()
                            .id(id(&format!("image-{n}")))
                            .flex_none()
                            .size(px(96.))
                            .rounded(px(10.))
                            .bg(p.card2)
                            .overflow_hidden();
                        match picture {
                            Some(picture) => thumb.child(
                                gpui_kit::img(picture)
                                    .size_full()
                                    .object_fit(gpui_kit::ObjectFit::Cover),
                            ),
                            None => thumb,
                        }
                        .test_support()
                    }))
                    .test_support()
                    .into_any_element()
            }
            Block::Thought { text, .. } => {
                let (label, timer) = self.thinking.clone().expect("a thought has its timer");
                // A thought the provider sent no text of says how long it took, and opens on
                // nothing.
                let said = !text.trim().is_empty();
                let head = div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(13.))
                    .text_color(p.faint)
                    .child(label)
                    .child(timer);
                let head = match said {
                    true => head
                        .id(id("thought-toggle"))
                        .cursor_pointer()
                        .child(icon(Icon::Chevron).size(px(12.)).text_color(p.faint))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open = !this.open;
                            cx.notify();
                        }))
                        .test_support()
                        .into_any_element(),
                    false => head.into_any_element(),
                };
                div()
                    .id(id("thought"))
                    .flex()
                    .flex_col()
                    .child(head)
                    .when(said && self.open, |d| {
                        d.child(
                            div()
                                .id(id("thought-text"))
                                .mt(px(8.))
                                .ml(px(6.))
                                .pl(px(14.))
                                .border_l_1()
                                .border_color(p.line)
                                .text_size(px(13.))
                                .text_color(p.mute)
                                .child(markdown_view(&self.markdown))
                                .test_support(),
                        )
                    })
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
                div()
                    .id(id("text"))
                    .max_w(px(680.))
                    .when_some(author, |d, (me, letter)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
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
                    .child(markdown_view(&self.markdown[..self.markdown.len().min(1)]))
                    .test_support()
                    .into_any_element()
            }
            Block::Steps { calls } => {
                let running = calls
                    .iter()
                    .filter(|c| c.state == CallState::Running)
                    .count();
                let done = calls.len() - running;
                let meta = match running {
                    0 => format!("{done} of {}", calls.len()),
                    _ => "running".to_owned(),
                };
                let rows =
                    calls
                        .iter()
                        .zip(&self.calls)
                        .enumerate()
                        .map(|(at, (call, (_, mark)))| {
                            div()
                                .id(ElementId::Name(format!("step-{}-{at}", self.at).into()))
                                .flex()
                                .gap(px(10.))
                                .px(px(4.))
                                .py(px(7.))
                                .child(mark.clone())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .mono(12.5)
                                                .child(SharedString::from(call.name.clone()))
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
                                        .child(
                                            div()
                                                .mono(11.5)
                                                .text_color(p.faint)
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(SharedString::from(call.args.clone())),
                                        ),
                                )
                                .child(div().flex_none().mono(11.).text_color(p.faint).child(
                                    SharedString::from(
                                        call.duration_ms().map_or("…".into(), seconds),
                                    ),
                                ))
                                .test_support()
                        });
                let toggle = cx.listener(|this, _, _, cx| {
                    this.open = !this.open;
                    cx.notify();
                });
                let chevron = |turn: f32| {
                    icon(Icon::Chevron)
                        .size(px(12.))
                        .text_color(p.faint)
                        .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * turn)))
                };
                // Folded to one discreet line until a click opens it: a long run of calls
                // would push the conversation down.
                if !self.open {
                    let last = calls.last().expect("a block of steps holds a call");
                    let count = calls.len();
                    let line = SharedString::from(format!(
                        "{count} {} · {}",
                        if count == 1 { "step" } else { "steps" },
                        last.name
                    ));
                    return div()
                        .id(id("steps"))
                        .child(
                            div()
                                .id(id("steps-toggle"))
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .py(px(2.))
                                .cursor_pointer()
                                .mono(12.)
                                .text_color(p.faint)
                                .hover(|d| d.text_color(p.mute))
                                .child(chevron(1.))
                                .aria_label(line.clone())
                                .child(line)
                                .on_click(toggle)
                                .test_support(),
                        )
                        .test_support()
                        .into_any_element();
                }
                div()
                    .id(id("steps"))
                    .child(
                        Card::new()
                            .title(
                                div()
                                    .id(id("steps-toggle"))
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .cursor_pointer()
                                    .child("Agent steps")
                                    .child(chevron(0.))
                                    .on_click(toggle)
                                    .test_support(),
                            )
                            .meta(meta)
                            .child(div().px(px(12.)).py(px(6.)).children(rows)),
                    )
                    .test_support()
                    .into_any_element()
            }
            Block::SubAgents => {
                let by = self.label().unwrap_or_default();
                for agent in &self.sub_agents {
                    if !self.icons.contains_key(&agent.id) {
                        let status = agent.status.clone();
                        let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
                        let pill = cx.new(|cx| Pill::new(status, cx).quiet_running());
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
                    .detail(SharedString::from(runs_on(
                        &agent.provider,
                        agent.model.as_deref(),
                    )))
                    .meta(SharedString::from(agent.doing.clone()))
                    .trailing(self.pills[&agent.id].clone())
                    .on_click(move |_, _, cx| {
                        if let Some(page) = page.upgrade() {
                            page.update(cx, |page, cx| page.select(&target, cx));
                        }
                    })
                });
                div()
                    .id(id("sub-agents"))
                    .child(
                        Card::new()
                            .title("Sub-agents")
                            .meta(by)
                            .child(div().p(px(4.)).children(rows)),
                    )
                    .test_support()
                    .into_any_element()
            }
            // Folded to its one line until a click opens it.
            Block::Report { text, .. } => div()
                .id(id("report"))
                .child(
                    Card::new()
                        .title(
                            div()
                                .id(id("report-toggle"))
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .cursor_pointer()
                                .child(SharedString::from(self.label().unwrap_or_default()))
                                .child(icon(Icon::Chevron).size(px(12.)).text_color(p.faint))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open = !this.open;
                                    cx.notify();
                                }))
                                .test_support(),
                        )
                        .when(self.open, |card| {
                            card.child(
                                div()
                                    .id(id("report-text"))
                                    .px(px(14.))
                                    .pt(px(10.))
                                    .pb(px(12.))
                                    .child(
                                        TextView::markdown(id("report-markdown"), text.clone())
                                            .image_source(no_image),
                                    )
                                    .test_support(),
                            )
                        }),
                )
                .test_support()
                .into_any_element(),
            Block::Notice { kind, text } => div()
                .id(id("notice"))
                .flex()
                .gap(px(8.))
                .mono(12.)
                .text_color(p.faint)
                .child(div().flex_none().child(SharedString::from(kind.clone())))
                .child(div().min_w_0().child(SharedString::from(text.clone())))
                .test_support()
                .into_any_element(),
            Block::Diff { path, lines } => {
                let added = lines
                    .iter()
                    .filter(|l| matches!(l, DiffLine::Added(_)))
                    .count();
                let removed = lines.len() - added;
                div()
                    .id(id("diff"))
                    .child(
                        Card::new()
                            .title(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(icon(Icon::Repository).text_color(p.faint))
                                    .child(
                                        div().mono(12.5).child(SharedString::from(path.clone())),
                                    ),
                            )
                            .meta(format!("+{added} −{removed}"))
                            .child(
                                div()
                                    .py(px(6.))
                                    .mono(12.)
                                    .children(lines.iter().map(|line| {
                                        let (text, bg, ink) = match line {
                                            DiffLine::Removed(text) => {
                                                (format!("- {text}"), p.del, p.fail)
                                            }
                                            DiffLine::Added(text) => {
                                                (format!("+ {text}"), p.add, p.ok)
                                            }
                                        };
                                        div()
                                            .px(px(14.))
                                            .bg(bg)
                                            .text_color(ink)
                                            .whitespace_nowrap()
                                            .child(SharedString::from(text))
                                    })),
                            ),
                    )
                    .test_support()
                    .into_any_element()
            }
            Block::Approval { number } => self.render_card(*number, cx).into_any_element(),
            Block::Answered { .. } => div().into_any_element(),
            Block::Command(result) => {
                let toggle = cx.listener(|this, _, _, cx| {
                    this.open = !this.open;
                    cx.notify();
                });
                commands::card(result, self.at, self.open, toggle, cx)
            }
        }
    }
}

impl Row {
    fn render_card(&mut self, number: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let card_id = ElementId::Name(format!("approval-{number}").into());
        let Some(card) = self.card.clone() else {
            return div().id(card_id).into_any_element();
        };
        if let Some(record) = card.record() {
            return enter(
                ElementId::Name(format!("record-enter-{number}").into()),
                0,
                div()
                    .id(ElementId::Name(format!("approval-record-{number}").into()))
                    .mono(12.)
                    .text_color(p.faint)
                    .child(SharedString::from(record))
                    .test_support(),
            )
            .into_any_element();
        }
        let disabled = self.busy;
        let button = |id: &str, label: &str, answer: Answer, primary: bool| {
            let id = ElementId::Name(format!("{id}-{number}").into());
            let button = match primary {
                true => Button::primary(id, label.to_owned()),
                false => Button::quiet(id, label.to_owned()),
            };
            button
                .disabled(disabled)
                .on_click(cx.listener(move |this, _, _, cx| this.answer(answer, cx)))
        };
        div()
            .id(card_id)
            .child(
                Card::new()
                    .attention()
                    .when(card.base, Card::alarm)
                    .child(
                        div()
                            .px(px(16.))
                            .pt(px(14.))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .semibold()
                                    .child(SharedString::from(format!(
                                        "{}?",
                                        card.asked
                                            .clone()
                                            .unwrap_or_else(|| format!("Request #{number}"))
                                    ))),
                            )
                            .children(card.says.iter().enumerate().map(|(i, said)| {
                                div()
                                    .id(ElementId::Name(
                                        format!("approval-says-{number}-{i}").into(),
                                    ))
                                    .text_size(px(13.))
                                    .child(SharedString::from(said.clone()))
                                    .test_support()
                            }))
                            .child({
                                let class = match card.base {
                                    true => "Base",
                                    false => "To judge",
                                };
                                div()
                                    .id(ElementId::Name(format!("approval-class-{number}").into()))
                                    .aria_label(class)
                                    .mono(11.)
                                    .text_color(if card.base { p.fail } else { p.wait })
                                    .child(class)
                                    .test_support()
                            })
                            .when(card.base, |d| {
                                d.child(
                                    div()
                                        .id(ElementId::Name(
                                            format!("approval-base-{number}").into(),
                                        ))
                                        .aria_label(BASE_SAYS)
                                        .text_size(px(13.))
                                        .semibold()
                                        .text_color(p.fail)
                                        .child(BASE_SAYS)
                                        .test_support(),
                                )
                            })
                            .when_some(card.judgement.clone(), |d, judgement| {
                                d.child(
                                    div()
                                        .id(ElementId::Name(
                                            format!("approval-judge-{number}").into(),
                                        ))
                                        .aria_label(SharedString::from(judgement.clone()))
                                        .text_size(px(13.))
                                        .child(SharedString::from(judgement))
                                        .test_support(),
                                )
                            })
                            .when_some(card.why.clone(), |d, why| {
                                d.child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(p.mute)
                                        .child(SharedString::from(why)),
                                )
                            })
                            .when_some(card.source.clone(), |d, source| {
                                d.child(
                                    div()
                                        .mono(11.)
                                        .text_color(p.faint)
                                        .child(SharedString::from(source)),
                                )
                            })
                            .when_some(self.error.clone(), |d, error| {
                                d.child(
                                    div()
                                        .id(ElementId::Name(
                                            format!("approval-error-{number}").into(),
                                        ))
                                        .text_size(px(12.5))
                                        .text_color(p.fail)
                                        .child(SharedString::from(error))
                                        .test_support(),
                                )
                            }),
                    )
                    .action(button("deny", "Deny", Answer::Deny, false))
                    .action(button("allow", "Allow", Answer::Once, true)),
            )
            .test_support()
            .into_any_element()
    }
}

/// Why a base card comes back whatever the mode, said on it.
pub const BASE_SAYS: &str = "Flottille always asks you for this, in Ask and in Auto: it may lose \
                             work or reach what must stay private, so no judge decides it.";

/// The time a thought took, or has taken so far: it ticks only while the thought runs.
pub struct ThinkingTimer {
    since: u64,
    until: Option<u64>,
    _ticker: Option<Task<()>>,
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
}

impl Render for ThinkingTimer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let until = self.until.unwrap_or_else(now_ms);
        div().mono(12.).child(SharedString::from(seconds(
            until.saturating_sub(self.since),
        )))
    }
}

/// An agent's row in the rail.
#[derive(Clone, Debug, PartialEq)]
pub struct RailAgent {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    pub depth: u64,
    /// The agent that launched it; `None` for the root.
    pub parent: Option<String>,
    /// A child that completed or was stopped: the rail gathers those under one folded row.
    pub ended: bool,
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

/// The fold of the children behind one "ended" row.
struct EndedFold {
    motion: Motion,
    /// The children's height, measured while they are drawn.
    height: Rc<Cell<Pixels>>,
}

/// The rail: the session's agents, its workspace and its facts.
pub struct SessionRail {
    page: WeakEntity<SessionPage>,
    agents: Vec<RailAgent>,
    marks: HashMap<String, (Entity<StateIcon>, Entity<Pill>)>,
    /// The parents whose "ended" row is open: kept while the window shows the session.
    open_ended: HashSet<String>,
    /// How far each "ended" row's children are unfolded: they stay drawn until a fold ends.
    ended_folds: HashMap<String, EndedFold>,
    selected: String,
    workspace: Option<String>,
    project: String,
    place: Option<PlaceFacts>,
    /// The "⋯" menu of the WORKSPACE section.
    menu: Entity<Menu>,
    /// The "⋯" menu of the root's row: it ends the session.
    agent_menu: Entity<Menu>,
    /// The commands of the session's tree.
    commands: Entity<CommandList>,
    /// Where a ticket is typed to link it, made when the section first shows a workspace.
    ticket_field: Option<Entity<InputState>>,
    /// What the daemon said of the ticket typed.
    ticket_error: Option<SharedString>,
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
        let agent_menu = cx.new(|cx| {
            Menu::new("rail-agent-menu", "Session actions", cx).entry(
                "rail-agent-end",
                "End session…",
                Some(Icon::Stop),
                move |_, cx| {
                    ender.update(cx, |page, cx| page.ask_end(cx)).ok();
                },
            )
        });
        let menu = cx.new(|cx| {
            Menu::new("workspace-menu", "Workspace actions", cx).entry(
                "open-in-editor",
                "Open in editor",
                Some(Icon::OpenInEditor),
                move |_, cx| {
                    editor.update(cx, |page, _| page.open_in_editor()).ok();
                },
            )
        });
        let commands = cx.new(|_| CommandList::new(page.clone()));
        Self {
            page,
            commands,
            agents: Vec::new(),
            marks: HashMap::new(),
            open_ended: HashSet::new(),
            ended_folds: HashMap::new(),
            menu,
            agent_menu,
            selected: root,
            workspace: opened.place.clone(),
            project: opened.project.clone(),
            place: None,
            ticket_field: None,
            ticket_error: None,
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
            .or_insert_with(|| EndedFold {
                motion: Motion::settled(0.0),
                height: Rc::new(Cell::new(px(0.))),
            });
        fold.motion
            .animate(if open { 1.0 } else { 0.0 }, PANEL, now, reduce);
        cx.notify();
    }

    pub fn place(&self) -> Option<&PlaceFacts> {
        self.place.as_ref()
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
                (
                    cx.new(|cx| StateIcon::new(status.clone(), cx)),
                    cx.new(|cx| Pill::new(status, cx).quiet_running()),
                )
            });
        }
        self.agents = agents;
        cx.notify();
    }

    fn set_selected(&mut self, selected: String, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }

    /// The place's facts; `None` when the daemon has none (the workspace was removed).
    fn set_place(&mut self, place: Option<PlaceFacts>, cx: &mut Context<Self>) {
        if self.place != place {
            self.place = place;
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
        // The root alone is ended (the daemon refuses a child), and only while it is not ended.
        let endable = self.agents.iter().any(|a| {
            a.parent.is_none()
                && !matches!(
                    a.status.read(cx).state(),
                    AgentState::Failed | AgentState::Completed | AgentState::Stopped
                )
        });
        let agent_menu = self.agent_menu.clone();
        let agent_row = |agent: &RailAgent| {
            let (mark, pill) = self.marks[&agent.id].clone();
            let page = self.page.clone();
            let target = agent.id.clone();
            let kid = agent.depth > 0;
            let indent = 20. * agent.depth as f32;
            div()
                .id(ElementId::Name(format!("rail-agent-{}", agent.id).into()))
                .relative()
                .flex()
                .items_center()
                .pl(px(indent))
                .when(kid, |d| {
                    d.child(enter(
                        ElementId::Name(format!("elbow-{}", agent.id).into()),
                        0,
                        elbow(p.line2)
                            .absolute()
                            .left(px(indent - 5.))
                            .top(px(-12.))
                            .w(px(10.))
                            .h(px(32.)),
                    ))
                })
                .child(
                    // The name over the provider and model: a rail is too narrow for both on one
                    // line.
                    div()
                        .id(ElementId::Name(format!("rail-row-{}", agent.id).into()))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .when(agent.id == self.selected, |d| d.bg(p.card2))
                        .when(agent.id != self.selected, |d| d.hover(|s| s.bg(p.hover)))
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
                                // The root has no name on screen: what it runs on says it.
                                .when(kid, |d| {
                                    d.child(
                                        div()
                                            .id(ElementId::Name(
                                                format!("rail-name-{}", agent.id).into(),
                                            ))
                                            .medium()
                                            .text_size(px(13.))
                                            .text_ellipsis()
                                            .overflow_hidden()
                                            .child(SharedString::from(agent.name.clone()))
                                            .test_support(),
                                    )
                                })
                                .child(
                                    div()
                                        .id(ElementId::Name(
                                            format!("rail-runs-on-{}", agent.id).into(),
                                        ))
                                        .when(kid, |d| d.mono(11.).text_color(p.faint))
                                        .when(!kid, |d| d.mono(12.).text_color(p.text))
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(SharedString::from(runs_on(
                                            &agent.provider,
                                            agent.model.as_deref(),
                                        )))
                                        .test_support(),
                                ),
                        )
                        .child(div().flex_none().child(pill))
                        .on_click(move |_, _, cx| {
                            if let Some(page) = page.upgrade() {
                                page.update(cx, |page, cx| page.select(&target, cx));
                            }
                        })
                        .test_support(),
                )
                .when(!kid && endable, |d| d.child(agent_menu.clone()))
                .test_support()
        };
        let ended_row = |parent: &str, count: usize, open: bool| {
            let depth = self
                .agents
                .iter()
                .find(|a| a.id == parent)
                .map_or(1, |a| a.depth + 1);
            let indent = 20. * depth as f32;
            let target = parent.to_owned();
            div()
                .relative()
                .pl(px(indent))
                .child(enter(
                    ElementId::Name(format!("elbow-ended-{parent}").into()),
                    0,
                    elbow(p.line2)
                        .absolute()
                        .left(px(indent - 5.))
                        .top(px(-12.))
                        .w(px(10.))
                        .h(px(32.)),
                ))
                .child(
                    div()
                        .id(ElementId::Name(format!("rail-ended-{parent}").into()))
                        .aria_expanded(open)
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .text_color(p.faint)
                        .hover(|s| s.bg(p.hover).text_color(p.text))
                        .child(div().w(px(16.)).flex_none().flex().justify_center().child(
                            icon(Icon::Chevron).size(px(12.)).text_color(p.faint).when(
                                !open,
                                |i| {
                                    i.with_transformation(Transformation::rotate(radians(
                                        -FRAC_PI_2,
                                    )))
                                },
                            ),
                        ))
                        .child(SharedString::from(format!("{count} ended")))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_ended(&target, cx)))
                        .test_support(),
                )
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut listed = self.rows_among(&drawn_open).into_iter();
        while let Some(row) = listed.next() {
            match row {
                RailRow::Agent(id) => {
                    rows.extend(
                        self.agents
                            .iter()
                            .find(|a| a.id == id)
                            .map(|agent| agent_row(agent).into_any_element()),
                    );
                }
                RailRow::Ended {
                    parent,
                    count,
                    open,
                } => {
                    rows.push(
                        ended_row(&parent, count, self.open_ended.contains(&parent))
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
                                Some(agent_row(agent).into_any_element())
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
        let pull_of = |path: &str| {
            self.place
                .iter()
                .flat_map(|place| place.pulls.iter())
                .find(|pull| pull.repository == path)
                .cloned()
        };
        let forge_of = |path: &str| {
            self.place
                .iter()
                .flat_map(|place| place.forges.iter())
                .find(|(repository, _)| repository == path)
                .map(|(_, line)| line.clone())
        };
        let repositories: Vec<(String, String, Option<PullRequest>, Option<String>)> = self
            .place
            .iter()
            .flat_map(|place| place.repositories.iter())
            .map(|(path, facts)| {
                let said = match facts {
                    RepositoryFacts::Changes(Some(c)) if c.added == 0 && c.removed == 0 => {
                        "clean".to_owned()
                    }
                    RepositoryFacts::Changes(Some(c)) => format!(
                        "{}+{} −{}",
                        if c.capped { "≥" } else { "" },
                        c.added,
                        c.removed
                    ),
                    RepositoryFacts::Changes(None) => "unreadable".to_owned(),
                    RepositoryFacts::Branch(Some(branch)) => branch.clone(),
                    RepositoryFacts::Branch(None) => "no branch".to_owned(),
                };
                (
                    crate::project::repository_label(&self.project, path),
                    said,
                    pull_of(path),
                    forge_of(path),
                )
            })
            .collect();
        let forge_problem = self.place.as_ref().and_then(|p| p.forge_problem.clone());
        if self.workspace.is_some() && self.ticket_field.is_none() {
            let field = cx.new(|cx| {
                let mut field =
                    InputState::new(window, cx).placeholder("link a ticket: #123 or ABC-123");
                field.set_editor_style(InputEditorStyle {
                    foreground: p.text,
                    muted_foreground: p.faint,
                    selection: p.run.opacity(0.35),
                    caret: p.text,
                    ..Default::default()
                });
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
        let ticket = self.place.as_ref().and_then(|p| p.ticket.clone());
        let ticket_row = ticket.map(|ticket| {
            let page = self.page.clone();
            let rail = cx.entity().downgrade();
            let said = match (&ticket.state, &ticket.title, &ticket.problem) {
                (_, _, Some(problem)) => problem.clone(),
                (state, title, None) => format!(
                    "{} {}",
                    state.clone().unwrap_or_default(),
                    title.clone().unwrap_or_default()
                )
                .trim()
                .to_owned(),
            };
            let url = ticket.url.clone();
            div()
                .flex()
                .items_start()
                .child(
                    div()
                        .id("rail-ticket")
                        .flex_1()
                        .flex()
                        .flex_col()
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .when(url.is_some(), |d| d.cursor_pointer())
                        .hover(|s| s.bg(p.hover))
                        .aria_label(SharedString::from(format!("{} {said}", ticket.reference)))
                        .child(
                            div()
                                .mono(11.5)
                                .text_color(p.mute)
                                .child(SharedString::from(ticket.reference.clone())),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(if ticket.problem.is_some() {
                                    p.faint
                                } else {
                                    p.text
                                })
                                .child(SharedString::from(said)),
                        )
                        .on_click(move |_, _, cx| {
                            if let Some(url) = &url {
                                page.update(cx, |page, _| page.open_link(url)).ok();
                            }
                        })
                        .test_support(),
                )
                .child(
                    Button::quiet("rail-ticket-unlink", "Unlink").on_click(move |_, _, cx| {
                        rail.update(cx, |rail, cx| rail.link_ticket(None, cx)).ok();
                    }),
                )
        });
        let ticket_field = self
            .ticket_field
            .clone()
            .filter(|_| self.workspace.is_some());
        let ticket_error = self.ticket_error.clone();
        let mut facts: Vec<(&str, String)> = Vec::new();
        match (&self.workspace, &self.place) {
            (Some(_), Some(place)) => {
                facts.extend(place.branch.clone().map(|b| ("Branch", b)));
                if !place.bases.is_empty() {
                    facts.push(("From", place.bases.join(", ")));
                }
                facts.push((
                    "Prepared",
                    place.prepared_ms.map_or("not yet".into(), seconds),
                ));
            }
            (None, _) => facts.push(("Place", "the project folder".into())),
            _ => {}
        }
        facts.push(("Machine", "this PC".into()));
        div()
            .id("rail")
            .size_full()
            .overflow_y_scroll()
            .px(px(12.))
            .py(px(14.))
            .flex()
            .flex_col()
            .child(section_label("Agents", cx))
            .children(rows)
            .child(self.commands.clone())
            .child(div().h(px(1.)).bg(p.line).mx(px(6.)).my(px(16.)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().child(section_label("Workspace", cx)))
                    .child(self.menu.clone()),
            )
            .children(repositories.into_iter().map(|(path, said, pull, forge)| {
                let row = div()
                    .id(ElementId::Name(format!("rail-repository-{path}").into()))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.5))
                    .child(icon(Icon::Repository).size(px(13.)).text_color(p.faint))
                    .child(SharedString::from(path.clone()))
                    .child(
                        div()
                            .ml_auto()
                            .mono(11.)
                            .text_color(p.faint)
                            .child(SharedString::from(said)),
                    )
                    .test_support();
                let pull = pull.map(|pull| {
                    let page = self.page.clone();
                    let url = pull.url.clone();
                    pull_row(
                        ElementId::Name(format!("rail-pull-{path}").into()),
                        &pull,
                        cx,
                    )
                    .pl(px(29.))
                    .on_click(move |_, _, cx| {
                        page.update(cx, |page, _| page.open_link(&url)).ok();
                    })
                    .test_support()
                });
                let forge = forge.map(|line| {
                    div()
                        .id(ElementId::Name(format!("rail-forge-{path}").into()))
                        .pl(px(29.))
                        .pb(px(4.))
                        .mono(11.)
                        .text_color(p.faint)
                        .child(SharedString::from(line))
                        .test_support()
                });
                div().child(row).children(forge).children(pull)
            }))
            .children(forge_problem.map(|problem| {
                div()
                    .id("rail-forge-problem")
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(11.5))
                    .text_color(p.faint)
                    .child(SharedString::from(problem))
                    .test_support()
            }))
            .children(ticket_row)
            .children(
                self.place
                    .as_ref()
                    .and_then(|p| p.run.as_ref())
                    .map(|run| crate::procedures::run_block(run, cx)),
            )
            .children(ticket_field.map(|field| {
                div()
                    .id("rail-ticket-field")
                    .mx(px(8.))
                    .my(px(4.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(8.))
                    .bg(p.field)
                    .border_1()
                    .border_color(p.line2)
                    .flex()
                    .items_center()
                    .mono(11.5)
                    .child(Input::new(&field))
                    .test_support()
            }))
            .children(ticket_error.map(|error| {
                div()
                    .id("rail-ticket-error")
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(11.5))
                    .text_color(p.fail)
                    .child(error)
                    .test_support()
            }))
            .child(div().h(px(1.)).bg(p.line).mx(px(6.)).my(px(16.)))
            .child(
                div()
                    .id("rail-facts")
                    .px(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .children(facts.into_iter().map(|(key, value)| {
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(div().text_color(p.faint).child(key))
                            .child(div().ml_auto().mono(12.).child(SharedString::from(value)))
                    }))
                    .test_support(),
            )
    }
}

/// A pull request's row: its number and state, and its checks' mark, drawn by shape as well as
/// colour. The place card draws the same.
pub(crate) fn pull_row(id: ElementId, pull: &PullRequest, cx: &App) -> Stateful<Div> {
    let p = palette(cx);
    let (glyph, color, said) = match pull.mark.as_deref() {
        Some("passing") => ("✓", p.ok, "checks passing"),
        Some("failing") => ("✗", p.fail, "checks failing"),
        Some(_) => ("●", p.run, "checks pending"),
        None => ("", p.faint, "no checks"),
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(6.))
        .cursor_pointer()
        .mono(11.)
        .text_color(p.mute)
        .hover(|s| s.bg(p.hover).text_color(p.text))
        .aria_label(SharedString::from(format!("{}, {said}", pull.label())))
        .child(SharedString::from(pull.label()))
        .child(div().text_color(color).child(glyph))
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

/// The composer of a session: it writes to the root agent, and only to it. `/` at the start of a
/// message lists the project's procedures, to run one in the session's workspace.
pub struct Reply {
    shell: WeakEntity<Shell>,
    /// The root agent: fixed when the composer is made, the only agent it writes to.
    session: String,
    project: String,
    /// The session's workspace; `None` in `main`, where no procedure runs.
    workspace: Place,
    /// The project's procedures, as the daemon listed them when `/` was last typed.
    procedures: Option<Vec<ProcedureInfo>>,
    /// What follows the `/` while the message is that one word: the `/` list is open.
    slash: Option<String>,
    /// The entry of the `/` list the keyboard is on.
    highlighted: usize,
    /// Escape closed the `/` list: it stays closed until the message is no longer a `/` word.
    dismissed: bool,
    field: Entity<TextareaState>,
    facts: Entity<ReplyFacts>,
    pub mode: Entity<ModeSwitch>,
    has_text: bool,
    focused: bool,
    sending: bool,
    /// The root runs a turn: the composer offers to interrupt it.
    running: bool,
    /// An interruption was asked and not answered yet.
    interrupting: bool,
    /// The field's text as it was sent, while it is: what is typed after it is kept.
    sent: Option<String>,
    /// What the root does while its turn runs, for the line above the field.
    activity: Option<Activity>,
    /// Pictures pasted or dropped, sent with the next message.
    attachments: Vec<Arc<gpui_kit::Image>>,
    /// The ones the message being sent holds: they leave when it is taken, whatever was removed
    /// or added meanwhile.
    sent_images: Vec<Arc<gpui_kit::Image>>,
    /// Whether the root's agent takes images; `None` until it first started.
    images: Option<bool>,
    error: Option<SharedString>,
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl Reply {
    pub fn new(
        shell: WeakEntity<Shell>,
        session: String,
        project: String,
        workspace: Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(1, REPLY_MAX_ROWS)
                .submit_on_enter(true)
                .placeholder(REPLY_PLACEHOLDER);
            let p = palette(cx);
            field.set_editor_style(InputEditorStyle {
                foreground: p.text,
                muted_foreground: p.faint,
                selection: p.run.opacity(0.35),
                caret: p.text,
                ..Default::default()
            });
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
                    this.typed(&text, cx);
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
        Self {
            shell,
            session,
            project,
            workspace,
            procedures: None,
            slash: None,
            highlighted: 0,
            dismissed: false,
            field,
            facts: cx.new(|_| ReplyFacts {
                provider: None,
                model: None,
                context: None,
            }),
            mode,
            has_text: false,
            focused: false,
            sending: false,
            running: false,
            interrupting: false,
            sent: None,
            activity: None,
            attachments: Vec::new(),
            sent_images: Vec::new(),
            images: None,
            error: None,
            renders: 0,
            _subscriptions: subscriptions,
        }
    }

    /// The agent it writes to: the session's root.
    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn field(&self) -> &Entity<TextareaState> {
        &self.field
    }

    pub fn facts(&self) -> &Entity<ReplyFacts> {
        &self.facts
    }

    pub fn is_sending(&self) -> bool {
        self.sending
    }

    /// The pictures that go with the next message.
    pub fn attachments(&self) -> &[Arc<gpui_kit::Image>] {
        &self.attachments
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
            cx.notify();
        }
    }

    fn set_images(&mut self, images: Option<bool>) {
        self.images = images;
    }

    /// Attaches a picture, or says in one sentence why not; the text in the field stays.
    pub fn attach(&mut self, image: gpui_kit::Image, cx: &mut Context<Self>) {
        use gpui_kit::ImageFormat::{Gif, Jpeg, Png, Webp};
        let refused = if self.images == Some(false) {
            Some(NO_IMAGES)
        } else if !matches!(image.format, Png | Jpeg | Gif | Webp) {
            Some(IMAGE_KINDS)
        } else if image.bytes.len() > MAX_IMAGE_BYTES {
            Some(IMAGE_TOO_BIG)
        } else if self.attachments.len() >= MAX_IMAGES {
            Some(TOO_MANY_IMAGES)
        } else {
            None
        };
        match refused {
            Some(why) => self.error = Some(why.into()),
            None => {
                self.error = None;
                self.attachments.push(Arc::new(image));
            }
        }
        cx.notify();
    }

    /// Attaches the picture a file holds (a file dropped on the composer, or picked).
    pub fn attach_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        use gpui_kit::ImageFormat::{Gif, Jpeg, Png, Webp};
        let format = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => Png,
            Some("jpg" | "jpeg") => Jpeg,
            Some("gif") => Gif,
            Some("webp") => Webp,
            _ => {
                self.error = Some(IMAGE_KINDS.into());
                cx.notify();
                return;
            }
        };
        let big = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_IMAGE_BYTES as u64);
        match std::fs::read(path) {
            Ok(_) if big => self.error = Some(IMAGE_TOO_BIG.into()),
            Ok(bytes) => return self.attach(gpui_kit::Image::from_bytes(format, bytes), cx),
            Err(_) => self.error = Some("That file could not be read.".into()),
        }
        cx.notify();
    }

    pub fn detach(&mut self, at: usize, cx: &mut Context<Self>) {
        if at < self.attachments.len() {
            self.attachments.remove(at);
            self.error = None;
            cx.notify();
        }
    }

    /// Paste: a clipboard that holds a picture attaches it, whatever text comes with it (a copy in
    /// a browser gives the picture's address or its HTML as well); else the field takes it.
    fn pasted(&mut self, cx: &mut Context<Self>) -> bool {
        use gpui_kit::ClipboardEntry;
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        let has_image = clipboard
            .entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(_)));
        if clipboard.text().is_some() && !has_image {
            return false;
        }
        let mut taken = false;
        for entry in clipboard.into_entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    self.attach(image, cx);
                    taken = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    for path in paths.paths() {
                        self.attach_file(path, cx);
                        taken = true;
                    }
                }
                ClipboardEntry::String(_) => {}
            }
        }
        taken
    }

    /// The Attach button: pictures picked in the system's file chooser.
    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            this.update(cx, |this, cx| {
                for path in &paths {
                    this.attach_file(path, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The root runs a turn.
    pub fn is_running(&self) -> bool {
        self.running
    }

    fn clear_error(&mut self, cx: &mut Context<Self>) {
        if self.error.take().is_some() {
            cx.notify();
        }
    }

    fn set_running(&mut self, running: bool, cx: &mut Context<Self>) {
        if self.running != running {
            self.running = running;
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
        self.error = None;
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
        self.error = refused.map(Into::into);
        cx.notify();
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// The procedures the `/` list offers now, each with its description: those whose name holds
    /// what follows the `/`. None in `main`, nor while the list is closed.
    pub fn slash_entries(&self) -> Vec<(String, String)> {
        let (Some(typed), Some(procedures), Some(_), false) = (
            &self.slash,
            &self.procedures,
            &self.workspace,
            self.dismissed,
        ) else {
            return Vec::new();
        };
        let typed = typed.to_lowercase();
        procedures
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&typed))
            .map(|p| (p.name.clone(), p.description.clone()))
            .collect()
    }

    /// The daemon has listed the project's procedures since the composer was made.
    pub fn knows_procedures(&self) -> bool {
        self.procedures.is_some()
    }

    fn slash_open(&self) -> bool {
        self.slash.is_some() && !self.dismissed
    }

    /// The message changed: a `/` and one word open the list, asking the daemon for the
    /// procedures again (the project's file is the user's to edit). The field says it when the
    /// user types; a text set from code says it here.
    pub fn typed(&mut self, text: &str, cx: &mut Context<Self>) {
        let slash = text
            .strip_prefix('/')
            .filter(|word| !word.contains(char::is_whitespace))
            .map(str::to_owned);
        if slash.is_none() {
            self.dismissed = false;
        }
        if slash.is_some() && self.slash.is_none() && self.workspace.is_some() {
            let command = Command::Procedures {
                project: self.project.clone(),
            };
            // Later: the shell may be the one setting the text, as it hands a reply its answer.
            crate::procedures::perform_later(&self.shell, command, cx);
        }
        if self.slash != slash {
            self.slash = slash;
            self.highlighted = 0;
            cx.notify();
        }
    }

    /// The project's procedures, as the daemon listed them.
    pub fn procedures_listed(
        &mut self,
        project: &str,
        listed: &Result<Vec<ProcedureInfo>, String>,
        cx: &mut Context<Self>,
    ) {
        if project != self.project {
            return;
        }
        if let Ok(list) = listed
            && self.procedures.as_ref() != Some(list)
        {
            self.procedures = Some(list.clone());
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

    /// Enter: picks the procedure the `/` list is on; with none offered, the message is sent as
    /// text.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.slash_entries();
        match entries.get(self.highlighted).or(entries.first()) {
            Some((name, _)) => {
                let name = name.clone();
                self.pick(&name, window, cx)
            }
            None => self.send(window, cx),
        }
    }

    /// Opens the confirmation of a run of `name` in the session's workspace; the `/` word leaves
    /// the field. Nothing starts before Confirm.
    pub fn pick(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        self.slash = None;
        self.has_text = false;
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        let (project, name) = (self.project.clone(), name.to_owned());
        self.shell
            .update(cx, |shell, cx| {
                shell.run_procedure(&project, &workspace, &name, cx)
            })
            .ok();
        cx.notify();
    }

    /// The `/` list, over the composer: the procedures offered, or in `main` why there are none.
    fn render_slash(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.slash_open() {
            return None;
        }
        let p = palette(cx);
        let note = |id: &'static str, text: &'static str| {
            div()
                .id(id)
                .aria_label(text)
                .px(px(10.))
                .py(px(8.))
                .text_size(px(12.5))
                .text_color(p.faint)
                .child(text)
                .test_support()
                .into_any_element()
        };
        let body: Vec<AnyElement> = if self.workspace.is_none() {
            vec![note(
                "slash-in-main",
                "Procedures run in a workspace: start a session in one to run them.",
            )]
        } else {
            let entries = self.slash_entries();
            if entries.is_empty() {
                // A word that names no procedure is a message like any other: nothing to show.
                if self.procedures.as_ref().is_none_or(|p| !p.is_empty())
                    || self.slash.as_deref() != Some("")
                {
                    return None;
                }
                vec![note(
                    "slash-none",
                    "This project has no procedure yet: add one on its page.",
                )]
            } else {
                let mut rows = vec![section_label("Procedures", cx).into_any_element()];
                for (at, (name, description)) in entries.into_iter().enumerate() {
                    let picked = name.clone();
                    rows.push(
                        ListRow::new(
                            ElementId::Name(format!("slash-{name}").into()),
                            format!("/{name}"),
                        )
                        .line(description)
                        .selected(at == self.highlighted)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.pick(&picked, window, cx)),
                        )
                        .into_any_element(),
                    );
                }
                rows
            }
        };
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(gpui_kit::relative(1.))
                .pb(px(6.))
                .child(
                    div()
                        .id("slash-list")
                        .occlude()
                        .p(px(6.))
                        .rounded(px(12.))
                        .bg(p.card)
                        .border_1()
                        .border_color(p.line2)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .children(body)
                        .test_support(),
                )
                .into_any_element(),
        )
    }

    /// Sends what the field holds to the root agent.
    pub fn send(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let typed = self.field.read(cx).value().to_string();
        let message = typed.trim().to_owned();
        if self.sending || (message.is_empty() && self.attachments.is_empty()) {
            return;
        }
        self.sending = true;
        self.sent = Some(typed);
        self.sent_images = self.attachments.clone();
        self.error = None;
        let images = self
            .attachments
            .iter()
            .map(|image| Picture {
                mime: image.format.mime_type().to_owned(),
                bytes: image.bytes.clone(),
            })
            .collect();
        let command = Command::Reply {
            session: self.session.clone(),
            message,
            images,
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// What the daemon answered: what was sent leaves the field, and what was typed after it
    /// stays; or the field keeps the message and says why.
    pub fn replied(
        &mut self,
        refused: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sending = false;
        let sent = self.sent.take();
        match refused {
            None => {
                self.error = None;
                let sent_images = std::mem::take(&mut self.sent_images);
                self.attachments
                    .retain(|image| !sent_images.iter().any(|sent| Arc::ptr_eq(sent, image)));
                let now = self.field.read(cx).value().to_string();
                // A sent text changed meanwhile is left as it is: nothing typed is lost.
                if let Some(rest) = sent.as_deref().and_then(|sent| now.strip_prefix(sent)) {
                    let rest = rest.trim_start().to_owned();
                    self.has_text = !rest.trim().is_empty();
                    self.field
                        .update(cx, |field, cx| field.set_value(rest.clone(), window, cx));
                    self.typed(&rest, cx);
                }
            }
            Some(why) => self.error = Some(why.into()),
        }
        cx.notify();
    }
}

impl Render for Reply {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let can_send = (self.has_text || !self.attachments.is_empty()) && !self.sending;
        let status = self.status();
        let thumbs: Vec<AnyElement> = self
            .attachments
            .iter()
            .enumerate()
            .map(|(at, image)| {
                div()
                    .id(ElementId::Name(format!("attachment-{at}").into()))
                    .relative()
                    .flex_none()
                    .size(px(56.))
                    .child(
                        gpui_kit::img(image.clone())
                            .size_full()
                            .rounded(px(8.))
                            .object_fit(gpui_kit::ObjectFit::Cover),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("attachment-remove-{at}").into()))
                            .absolute()
                            .top(px(-5.))
                            .right(px(-5.))
                            .size(px(18.))
                            .rounded_full()
                            .bg(p.card2)
                            .border_1()
                            .border_color(p.line2)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .text_color(p.mute)
                            .cursor_pointer()
                            .aria_label("Remove the picture")
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| this.detach(at, cx)))
                            .test_support(),
                    )
                    .test_support()
                    .into_any_element()
            })
            .collect();
        let slash = self.render_slash(cx);
        div()
            .id("reply")
            .relative()
            .w_full()
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
            .children(slash.map(|slash| gpui_kit::deferred(slash).priority(1)))
            // A picture on the clipboard is an attachment; text is left to the field.
            .capture_action(cx.listener(|this, _: &Paste, _, cx| {
                if this.pasted(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                for path in paths.paths() {
                    this.attach_file(path, cx);
                }
            }))
            // Escape interrupts the turn that runs; with none running it is left to others.
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                if this.running && event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.interrupt(cx);
                }
            }))
            .p(px(6.))
            .rounded(px(16.))
            .bg(p.shell)
            .border_1()
            .border_color(p.line)
            .shadow_lg()
            .ui_text()
            .text_color(p.text)
            .when_some(status, |d, line| {
                d.child(
                    div()
                        .id("reply-status")
                        .aria_label(SharedString::from(line.clone()))
                        .px(px(10.))
                        .pb(px(6.))
                        .pt(px(2.))
                        .text_size(px(12.5))
                        .text_color(p.faint)
                        .child(SharedString::from(line))
                        .test_support(),
                )
            })
            .when(!thumbs.is_empty(), |d| {
                d.child(
                    div()
                        .id("reply-attachments")
                        .flex()
                        .gap(px(10.))
                        .px(px(6.))
                        .pt(px(6.))
                        .pb(px(10.))
                        .children(thumbs)
                        .test_support(),
                )
            })
            .child(
                div()
                    .id("reply-field")
                    .rounded(px(11.))
                    .bg(p.field)
                    .border_1()
                    .border_color(if self.focused {
                        p.focus_line
                    } else {
                        p.field_line
                    })
                    .min_h(px(60.))
                    .px(px(14.))
                    .py(px(12.))
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pt(px(6.))
                    .px(px(2.))
                    .child(self.facts.clone())
                    .child(self.mode.clone())
                    .child(
                        Button::quiet("attach", "Attach")
                            .tooltip("Attach pictures")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_files(window, cx)),
                            ),
                    )
                    .child(div().flex_1())
                    .when(self.running, |d| {
                        d.child(
                            Button::icon("reply-stop", Icon::Stop, STOP)
                                .disabled(self.interrupting)
                                .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx))),
                        )
                    })
                    .child(
                        Button::round("reply-send", Icon::Send, SEND)
                            .disabled(!can_send)
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("reply-error")
                        .aria_label(error.clone())
                        .px(px(10.))
                        .pt(px(6.))
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error)
                        .test_support(),
                )
            })
            .test_support()
    }
}

/// What an agent runs on, as a line: its provider and its model.
pub fn runs_on(provider: &str, model: Option<&str>) -> String {
    let model = match model {
        None | Some(crate::composer::DEFAULT) => "default model",
        Some(model) => model,
    };
    format!("{provider} · {model}")
}

/// The root's provider and model and the context it used, below the field.
pub struct ReplyFacts {
    provider: Option<String>,
    model: Option<String>,
    context: Option<u64>,
}

impl ReplyFacts {
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// What it shows of what the root runs on.
    pub fn shown(&self) -> String {
        match &self.provider {
            Some(provider) => runs_on(provider, self.model.as_deref()),
            None => self.model.clone().unwrap_or_else(|| "default model".into()),
        }
    }

    pub fn context(&self) -> Option<u64> {
        self.context
    }

    fn set_model(&mut self, (provider, model): (String, Option<String>), cx: &mut Context<Self>) {
        let provider = Some(provider).filter(|p| !p.is_empty());
        if self.model != model || self.provider != provider {
            self.model = model;
            self.provider = provider;
            cx.notify();
        }
    }

    fn set_context(&mut self, context: Option<u64>, cx: &mut Context<Self>) {
        if self.context != context {
            self.context = context;
            cx.notify();
        }
    }
}

impl Render for ReplyFacts {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .id("reply-model")
            .h(px(28.))
            .px(px(9.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.5))
            .text_color(p.mute)
            .child(SharedString::from(self.shown()))
            .when_some(self.context, |d, used| {
                d.child(
                    div()
                        .mono(11.5)
                        .text_color(p.faint)
                        .child(SharedString::from(format!("· context {used}%"))),
                )
            })
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::StyleRefinement;

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
            doing: String::new(),
            waiting_on: None,
            needs_user: false,
            resumable: false,
            stopping: false,
            images: None,
            gate_mode: Some("auto".into()),
        }
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
