//! A session's page: a header on one line (title, place, state pill, the editor button), the
//! shown agent's conversation, live, the composer that writes to the root agent (or, reading a
//! sub-agent, a line saying it belongs to its parent), and a rail with the session's tree of
//! agents, its workspace and its facts, which becomes a drawer in a narrower window.
//!
//! Every row of the conversation is its own view: a message arriving adds a row or draws one row
//! again, never the others. What an agent, a file or a command said is shown as the daemon gave
//! it (masked and escaped), as plain text.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use flottille_ui::{
    AgentState, Badge, Button, Card, Drawer, Icon, ListRow, Pill, PulsingLabel, Side, StateIcon,
    Status, Type, enter, icon, palette, section_label,
};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::base::input::{InputEditorStyle, InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Bounds, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, PathBuilder, Pixels, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window, canvas, div, point, px,
};

use crate::actions::{Answer, Command};
use crate::conversation::{Block, Call, CallState, Change, Conversation, DiffLine};
use crate::link::{Connect, Update};
use crate::model::Place;
use crate::open::Opener;
use crate::session_link::{
    self, PlaceFacts, RepositoryFacts, Request, SessionLink, SessionUpdate, TreeAgent, Where,
};
use crate::shell::{Layout, Shell, now_ms};
use crate::tree::place_name;

/// Width of the session page's right rail.
pub const RAIL: f32 = 290.;

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
    pill: Entity<Pill>,
    /// The agent whose conversation is shown: the root, or a sub-agent read.
    shown: String,
    link: Option<SessionLink>,
    tree: Vec<TreeAgent>,
    /// One state per agent, read by its rail row and its line in the sub-agents list.
    statuses: HashMap<String, Entity<Status>>,
    conversation: Entity<ConversationView>,
    rail: Entity<SessionRail>,
    drawer: Entity<Drawer>,
    reply: Entity<Reply>,
    requests: HashMap<u64, Request>,
    decisions: HashMap<u64, String>,
    needs_you: bool,
    renders: usize,
    _status: Subscription,
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
        let rail = cx.new(|_| SessionRail::new(page.clone(), opened.id.clone(), &opened));
        let drawer = cx.new(|_| Drawer::new("rail-drawer", Side::Right, px(RAIL), rail.clone()));
        let pill = cx.new(|cx| Pill::new(status.clone(), cx));
        let shown = opened.id.clone();
        let conversation = cx.new(|_| ConversationView::new(page, &shown, "lead".into(), None));
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
            pill,
            shown,
            link: None,
            tree: Vec::new(),
            statuses: HashMap::new(),
            conversation,
            rail,
            drawer,
            reply,
            requests: HashMap::new(),
            decisions: HashMap::new(),
            needs_you,
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

    pub fn tree(&self) -> &[TreeAgent] {
        &self.tree
    }

    pub fn rail(&self) -> &Entity<SessionRail> {
        &self.rail
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

    fn parent_of(&self, id: &str) -> Option<String> {
        self.tree
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.parent.clone())
    }

    /// A new link for the shown agent: the old one stops, and what it sends late is dropped.
    fn relink(&mut self) {
        self.link = None;
        self.link = Some(session_link::spawn(
            self.follow.connect.clone(),
            self.follow.updates.clone(),
            self.opened.id.clone(),
            self.shown.clone(),
            Where {
                project: self.opened.project.clone(),
                workspace: self.opened.place.clone(),
            },
        ));
    }

    /// Shows an agent's conversation: the root's, with the composer, or a sub-agent's, read only.
    pub fn select(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.drawer.update(cx, |drawer, cx| drawer.close(cx));
        if self.shown == agent {
            return;
        }
        self.shown = agent.to_owned();
        let parent = self.parent_of(agent).map(|p| self.name(&p));
        let (page, name) = (cx.entity().downgrade(), self.name(agent));
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

    /// What the page's link read or saw; what an older link sends is not for it.
    pub fn apply(&mut self, token: u64, update: SessionUpdate, cx: &mut Context<Self>) {
        if self.token() != Some(token) {
            return;
        }
        match update {
            SessionUpdate::Tree(tree) => self.set_tree(tree, cx),
            SessionUpdate::History { events, first } => {
                for event in &events {
                    self.note_usage(event, cx);
                }
                let (conversation, this) = (self.conversation.clone(), &*self);
                conversation.update(cx, |view, cx| {
                    if first {
                        view.clear(cx);
                    }
                    for event in &events {
                        let changes = view.model.apply(event);
                        view.changed(&changes, this, cx);
                    }
                });
                self.refresh_conversation(cx);
            }
            SessionUpdate::Live(event) => {
                self.note_usage(&event, cx);
                let (conversation, this) = (self.conversation.clone(), &*self);
                conversation.update(cx, |view, cx| {
                    let changes = view.model.apply(&event);
                    view.changed(&changes, this, cx);
                });
                self.refresh_conversation(cx);
            }
            SessionUpdate::Requests(requests) => {
                self.requests = requests.into_iter().map(|r| (r.number, r)).collect();
                self.refresh_conversation(cx);
            }
            SessionUpdate::Decided { number, decision } => {
                self.decisions.insert(number, decision);
                self.refresh_conversation(cx);
            }
            SessionUpdate::Place(facts) => {
                self.rail.update(cx, |rail, cx| rail.set_place(facts, cx));
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
        let row = self.conversation.read(cx).approval_row(number);
        if let Some(row) = row {
            row.update(cx, |row, cx| row.answered(refused, cx));
        }
        self.refresh_conversation(cx);
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
            let model = root.model.clone();
            let facts = self.reply.read(cx).facts.clone();
            facts.update(cx, |facts, cx| facts.set_model(model, cx));
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
                    model: agent.model.clone(),
                    depth: agent.depth,
                    status,
                })
            })
            .collect();
        self.rail.update(cx, |rail, cx| rail.set_agents(rows, cx));
        let changed = self.tree != tree;
        self.tree = tree;
        if changed {
            let conversation = self.conversation.clone();
            conversation.update(cx, |view, _| view.model.set_names(names));
            self.refresh_conversation(cx);
        }
    }

    /// Brings the rows that show the tree or the requests up to what the page knows: the
    /// sub-agents list, the requests' cards and their records.
    fn refresh_conversation(&self, cx: &mut Context<Self>) {
        let conversation = self.conversation.clone();
        let has_children = self
            .tree
            .iter()
            .any(|a| a.parent.as_deref() == Some(&self.shown));
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
                    let changes = view.model.add_approval(number);
                    view.changed(&changes, self, cx);
                }
            }
            for row in view.rows.clone() {
                let kind = row.read(cx).block.clone();
                match kind {
                    Block::SubAgents => {
                        let lines = self.sub_agents();
                        if row.read(cx).sub_agents != lines {
                            row.update(cx, |row, cx| {
                                row.sub_agents = lines;
                                cx.notify();
                            });
                        }
                    }
                    Block::Approval { number } => {
                        let card = self.card(number);
                        if row.read(cx).card != card {
                            row.update(cx, |row, cx| {
                                row.card = card;
                                cx.notify();
                            });
                        }
                    }
                    _ => {}
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
                    model: agent.model.clone(),
                    doing: agent.doing.clone(),
                    status: self.statuses.get(&agent.id)?.clone(),
                })
            })
            .collect()
    }

    fn card(&self, number: u64) -> Option<ApprovalCard> {
        let request = self.requests.get(&number);
        let decision = self.decisions.get(&number).cloned();
        if request.is_none() && decision.is_none() {
            return None;
        }
        Some(ApprovalCard {
            number,
            asked: request.map(Request::asked),
            why: request.map(|r| {
                let mut why = r.reasons.join(" ");
                for (by, said) in &r.forwards {
                    why.push_str(&format!(" {} forwards it: {said}", self.name(by)));
                }
                why.trim().to_owned()
            }),
            source: request.map(|r| {
                let mut source = format!("asked by {}", self.name(&r.agent));
                for (by, _) in &r.forwards {
                    source.push_str(&format!(" · forwarded by {}", self.name(by)));
                }
                format!("{source} · #{number}")
            }),
            asker: request.map(|r| self.name(&r.agent)),
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
        let editor = cx.entity().downgrade();
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
            .child(
                Button::icon(
                    "open-in-editor",
                    Icon::OpenInEditor,
                    format!("Open {} in your editor", place_name(&place)),
                )
                .on_click(move |_, _, cx| {
                    editor.update(cx, |page, _| page.open_in_editor()).ok();
                }),
            )
            .when(!wide, |d| {
                let mut agents = Button::icon("open-rail", Icon::Agents, "Agents")
                    .on_click(move |_, _, cx| drawer.update(cx, |d, cx| d.open(cx)));
                if self.needs_you {
                    agents = agents.badge(Badge::alert());
                }
                d.child(agents)
            });
        let root = self.shown == self.opened.id;
        let dock = div()
            .flex_none()
            .w_full()
            .max_w(px(780.))
            .mx_auto()
            .px(px(if narrow { 12. } else { 28. }))
            .pb(px(if narrow { 12. } else { 20. }))
            .map(|d| match root {
                true => d.child(self.reply.clone()),
                false => d.child(self.read_only(cx)),
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
    }
}

impl SessionPage {
    /// In place of the composer while a sub-agent is read: it belongs to its parent.
    fn read_only(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let name = self.name(&self.shown);
        let parent = self
            .parent_of(&self.shown)
            .map(|parent| self.name(&parent))
            .unwrap_or_else(|| "its parent".into());
        let root = self.opened.id.clone();
        let back = format!("Back to {}", self.name(&root));
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
            .child(div().flex_1().min_w_0().child(SharedString::from(format!(
                "Reading {name}. It belongs to {parent}: only {parent} takes your messages."
            ))))
            .child(Button::quiet("back-to-root", back).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.select(&root, cx);
                },
            )))
            .test_support()
    }
}

/// The conversation: one row view per block, in a column that scrolls.
pub struct ConversationView {
    page: WeakEntity<SessionPage>,
    id: String,
    model: Conversation,
    rows: Vec<Entity<Row>>,
    scroll: ScrollHandle,
    renders: usize,
}

impl ConversationView {
    fn new(page: WeakEntity<SessionPage>, id: &str, me: String, parent: Option<String>) -> Self {
        Self {
            page,
            id: id.to_owned(),
            model: Conversation::new(me, parent),
            rows: Vec::new(),
            scroll: ScrollHandle::new(),
            renders: 0,
        }
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

    fn clear(&mut self, cx: &mut Context<Self>) {
        let me = self.model.me().to_owned();
        let parent = self.model.parent().map(str::to_owned);
        self.model = Conversation::new(me, parent);
        self.rows.clear();
        cx.notify();
    }

    fn approval_row(&self, number: u64) -> Option<Entity<Row>> {
        self.rows
            .iter()
            .zip(self.model.blocks())
            .find(|(_, block)| **block == Block::Approval { number })
            .map(|(row, _)| row.clone())
    }

    /// Adds a row for each block added, and draws again each row whose block changed.
    fn changed(&mut self, changes: &[Change], page: &SessionPage, cx: &mut Context<Self>) {
        let mut added = false;
        for change in changes {
            match *change {
                Change::Added => {
                    let at = self.rows.len();
                    let block = self.model.blocks()[at].clone();
                    let me = self.model.me().to_owned();
                    let row = Row::view(at, block, me, page, self.page.clone(), cx);
                    self.rows.push(row);
                    added = true;
                }
                Change::Changed(at) => {
                    let block = self.model.blocks()[at].clone();
                    if let Some(row) = self.rows.get(at).cloned() {
                        row.update(cx, |row, cx| row.set_block(block, cx));
                    }
                }
            }
        }
        if added {
            // Following the end, unless the user scrolled up to read.
            let (offset, max) = (self.scroll.offset().y, self.scroll.max_offset().y);
            if -offset >= max - px(24.) {
                self.scroll.scroll_to_bottom();
            }
            cx.notify();
        }
    }
}

impl Render for ConversationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let narrow = window.viewport_size().width < px(crate::shell::NARROW);
        let empty = self.rows.is_empty();
        div()
            .id(SharedString::from(format!("conversation-{}", self.id)))
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .max_w(px(780.))
                    .w_full()
                    .mx_auto()
                    .px(px(if narrow { 16. } else { 28. }))
                    .pt(px(if narrow { 20. } else { 28. }))
                    .pb(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .when(empty, |d| {
                        d.child(div().text_color(p.faint).child("Nothing said yet."))
                    })
                    .children(self.rows.iter().enumerate().map(|(at, row)| {
                        enter(
                            ElementId::Name(format!("enter-row-{}-{at}", self.id).into()),
                            0,
                            row.clone(),
                        )
                    })),
            )
    }
}

/// A child in the sub-agents list.
#[derive(Clone, Debug, PartialEq)]
pub struct SubAgent {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub doing: String,
    pub status: Entity<Status>,
}

/// What a request's card shows, once the page knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalCard {
    pub number: u64,
    pub asked: Option<String>,
    pub why: Option<String>,
    pub source: Option<String>,
    pub asker: Option<String>,
    /// `deny`, `once`, `tree` or `cancelled`, once decided.
    pub decision: Option<String>,
}

impl ApprovalCard {
    /// The one-line record a decided card folds into.
    pub fn record(&self) -> Option<String> {
        let said = match self.decision.as_deref()? {
            "deny" => "Denied",
            "once" => "Allowed once",
            "tree" => "Allowed for this tree",
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
    page: WeakEntity<SessionPage>,
    block: Block,
    /// A thought unfolded.
    open: bool,
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
                page: weak,
                block: Block::Text {
                    text: String::new(),
                },
                open: false,
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

    fn set_block(&mut self, block: Block, cx: &mut Context<Self>) {
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
        self.block = block;
        cx.notify();
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
            let shell = page.read(cx).shell.clone();
            shell
                .update(cx, |shell, _| {
                    shell.perform(Command::Answer { number, answer })
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

fn call_state(call: &Call) -> AgentState {
    match call.state {
        CallState::Running => AgentState::Running,
        CallState::Done => AgentState::Completed,
        CallState::Failed => AgentState::Failed,
    }
}

impl Render for Row {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("row-{}-{part}", self.at).into());
        match self.block.clone() {
            Block::Message { from, text } => div()
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
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .mono(11.5)
                                .text_color(p.faint)
                                .mb(px(5.))
                                .child(SharedString::from(from)),
                        )
                        .child(SharedString::from(text)),
                )
                .test_support()
                .into_any_element(),
            Block::Thought { text, .. } => {
                let (label, timer) = self.thinking.clone().expect("a thought has its timer");
                div()
                    .id(id("thought"))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(id("thought-toggle"))
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .text_size(px(13.))
                            .text_color(p.faint)
                            .cursor_pointer()
                            .child(label)
                            .child(timer)
                            .child(icon(Icon::Chevron).size(px(12.)).text_color(p.faint))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open = !this.open;
                                cx.notify();
                            }))
                            .test_support(),
                    )
                    .when(self.open, |d| {
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
                                .child(SharedString::from(text))
                                .test_support(),
                        )
                    })
                    .test_support()
                    .into_any_element()
            }
            Block::Text { text } => {
                let me = self.me.clone();
                let letter = me.chars().next().unwrap_or('?').to_uppercase().to_string();
                div()
                    .id(id("text"))
                    .max_w(px(680.))
                    .child(
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
                                    .child(SharedString::from(letter)),
                            )
                            .child(SharedString::from(me)),
                    )
                    .child(SharedString::from(text))
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
                                                .when(call.state == CallState::Running, |d| {
                                                    d.child(
                                                        div()
                                                            .mono(10.)
                                                            .medium()
                                                            .text_color(p.run)
                                                            .child("RUNNING"),
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
                div()
                    .id(id("steps"))
                    .child(
                        Card::new()
                            .title("Agent steps")
                            .meta(meta)
                            .child(div().px(px(12.)).py(px(6.)).children(rows)),
                    )
                    .test_support()
                    .into_any_element()
            }
            Block::SubAgents => {
                let by = self.me.clone();
                for agent in &self.sub_agents {
                    if !self.icons.contains_key(&agent.id) {
                        let status = agent.status.clone();
                        let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
                        let pill = cx.new(|cx| Pill::new(status, cx));
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
                    .when_some(agent.model.clone(), |row, model| {
                        row.detail(SharedString::from(model))
                    })
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
                            .meta(format!("launched by {by}"))
                            .child(div().p(px(4.)).children(rows)),
                    )
                    .test_support()
                    .into_any_element()
            }
            Block::Report { from, text } => div()
                .id(id("report"))
                .child(
                    Card::new()
                        .title(SharedString::from(format!("Report from {from}")))
                        .child(
                            div()
                                .px(px(14.))
                                .pt(px(10.))
                                .pb(px(12.))
                                .child(SharedString::from(text)),
                        ),
                )
                .test_support()
                .into_any_element(),
            Block::Notice { kind, text } => div()
                .id(id("notice"))
                .flex()
                .gap(px(8.))
                .mono(12.)
                .text_color(p.faint)
                .child(div().flex_none().child(SharedString::from(kind)))
                .child(div().min_w_0().child(SharedString::from(text)))
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
                                    .child(div().mono(12.5).child(SharedString::from(path))),
                            )
                            .meta(format!("+{added} −{removed}"))
                            .child(div().py(px(6.)).mono(12.).children(lines.into_iter().map(
                                |line| {
                                    let (text, bg, ink) = match line {
                                        DiffLine::Removed(text) => {
                                            (format!("- {text}"), p.del, p.fail)
                                        }
                                        DiffLine::Added(text) => (format!("+ {text}"), p.add, p.ok),
                                    };
                                    div()
                                        .px(px(14.))
                                        .bg(bg)
                                        .text_color(ink)
                                        .whitespace_nowrap()
                                        .child(SharedString::from(text))
                                },
                            ))),
                    )
                    .test_support()
                    .into_any_element()
            }
            Block::Approval { number } => self.render_card(number, cx).into_any_element(),
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
                    .action(button(
                        "allow-tree",
                        "Allow for this tree",
                        Answer::Tree,
                        false,
                    ))
                    .action(button("allow-once", "Allow once", Answer::Once, true)),
            )
            .test_support()
            .into_any_element()
    }
}

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
    pub model: Option<String>,
    pub depth: u64,
    pub status: Entity<Status>,
}

/// The rail: the session's agents, its workspace and its facts.
pub struct SessionRail {
    page: WeakEntity<SessionPage>,
    agents: Vec<RailAgent>,
    marks: HashMap<String, (Entity<StateIcon>, Entity<Pill>)>,
    selected: String,
    workspace: Option<String>,
    place: Option<PlaceFacts>,
    renders: usize,
}

impl SessionRail {
    fn new(page: WeakEntity<SessionPage>, root: String, opened: &Opened) -> Self {
        Self {
            page,
            agents: Vec::new(),
            marks: HashMap::new(),
            selected: root,
            workspace: opened.place.clone(),
            place: None,
            renders: 0,
        }
    }

    pub fn agents(&self) -> &[RailAgent] {
        &self.agents
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
                    cx.new(|cx| Pill::new(status, cx)),
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

    fn set_place(&mut self, place: PlaceFacts, cx: &mut Context<Self>) {
        if self.place.as_ref() != Some(&place) {
            self.place = Some(place);
            cx.notify();
        }
    }
}

impl Render for SessionRail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let agents = self.agents.iter().map(|agent| {
            let (mark, pill) = self.marks[&agent.id].clone();
            let page = self.page.clone();
            let target = agent.id.clone();
            let kid = agent.depth > 0;
            let indent = 20. * agent.depth as f32;
            div()
                .id(ElementId::Name(format!("rail-agent-{}", agent.id).into()))
                .relative()
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
                    ListRow::new(
                        ElementId::Name(format!("rail-row-{}", agent.id).into()),
                        SharedString::from(agent.name.clone()),
                    )
                    .leading(mark)
                    .when_some(agent.model.clone(), |row, model| {
                        row.detail(SharedString::from(model))
                    })
                    .trailing(pill)
                    .selected(agent.id == self.selected)
                    .on_click(move |_, _, cx| {
                        if let Some(page) = page.upgrade() {
                            page.update(cx, |page, cx| page.select(&target, cx));
                        }
                    }),
                )
                .test_support()
        });
        let repositories: Vec<(String, String)> = self
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
                (path.clone(), said)
            })
            .collect();
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
            .children(agents)
            .child(div().h(px(1.)).bg(p.line).mx(px(6.)).my(px(16.)))
            .child(section_label("Workspace", cx))
            .children(repositories.into_iter().map(|(path, said)| {
                div()
                    .id(ElementId::Name(format!("rail-repository-{path}").into()))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.5))
                    .child(icon(Icon::Repository).size(px(13.)).text_color(p.faint))
                    .child(SharedString::from(path))
                    .child(
                        div()
                            .ml_auto()
                            .mono(11.)
                            .text_color(p.faint)
                            .child(SharedString::from(said)),
                    )
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

/// The composer of a session: it writes to the root agent, and only to it.
pub struct Reply {
    shell: WeakEntity<Shell>,
    /// The root agent: fixed when the composer is made, the only agent it writes to.
    session: String,
    field: Entity<TextareaState>,
    facts: Entity<ReplyFacts>,
    has_text: bool,
    focused: bool,
    sending: bool,
    error: Option<SharedString>,
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl Reply {
    pub fn new(
        shell: WeakEntity<Shell>,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .submit_on_enter(true)
                .placeholder("Reply to lead…");
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
                    let has_text = !field.read(cx).value().trim().is_empty();
                    if this.has_text != has_text {
                        this.has_text = has_text;
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
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
        Self {
            shell,
            session,
            field,
            facts: cx.new(|_| ReplyFacts {
                model: None,
                context: None,
            }),
            has_text: false,
            focused: false,
            sending: false,
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

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// Sends what the field holds to the root agent.
    pub fn send(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let message = self.field.read(cx).value().trim().to_owned();
        if self.sending || message.is_empty() {
            return;
        }
        self.sending = true;
        self.error = None;
        let command = Command::Reply {
            session: self.session.clone(),
            message,
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// What the daemon answered: the field is emptied, or keeps the message and says why.
    pub fn replied(
        &mut self,
        refused: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sending = false;
        match refused {
            None => {
                self.error = None;
                self.field
                    .update(cx, |field, cx| field.set_value("", window, cx));
                self.has_text = false;
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
        let can_send = self.has_text && !self.sending;
        div()
            .id("reply")
            .w_full()
            .p(px(6.))
            .rounded(px(16.))
            .bg(p.shell)
            .border_1()
            .border_color(p.line)
            .shadow_lg()
            .ui_text()
            .text_color(p.text)
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
                    .child(
                        Button::quiet("attach", "Attach")
                            .disabled(true)
                            .tooltip("The daemon takes no attachments yet"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::round("reply-send", Icon::Send, "Send to lead")
                            .disabled(!can_send)
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("reply-error")
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

/// The root's model and the context it used, below the field.
pub struct ReplyFacts {
    model: Option<String>,
    context: Option<u64>,
}

impl ReplyFacts {
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn context(&self) -> Option<u64> {
        self.context
    }

    fn set_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if self.model != model {
            self.model = model;
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
            .child(SharedString::from(
                self.model.clone().unwrap_or_else(|| "default model".into()),
            ))
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

    fn agent(parent: Option<&str>, state: &str) -> TreeAgent {
        TreeAgent {
            id: "a1".into(),
            parent: parent.map(str::to_owned),
            depth: u64::from(parent.is_some()),
            role: "lead".into(),
            title: Some("build the form".into()),
            state: state.into(),
            state_reason: None,
            model: None,
            doing: String::new(),
            waiting_on: None,
            needs_user: false,
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

    #[test]
    fn a_decided_card_folds_into_one_line() {
        let card = ApprovalCard {
            number: 4,
            asked: Some("Read .env".into()),
            why: None,
            source: None,
            asker: Some("lead".into()),
            decision: Some("tree".into()),
        };
        assert_eq!(
            card.record().as_deref(),
            Some("Allowed for this tree · Read .env · lead")
        );
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
}
