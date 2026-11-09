//! The left tree: "New session", then each project and its sessions in one flat list, the one that
//! moved last on top, each on two lines (its title, then the place it works in and its pull
//! request). Every row is its own view, so one session changing redraws its row and nothing else;
//! a folded project, and the "Ended" group, show the strongest mark of what they hold.

use std::collections::HashSet;
use std::f32::consts::FRAC_PI_2;

use flottille_ui::color::mix;
use flottille_ui::motion::{ENTER, PANEL};
use flottille_ui::rows::{TWO_LINES, TreeLevel};
use flottille_ui::{
    AgentState, Button, Fold, Icon, Item, Listing, Menu, Modal, StateIcon, Status, Tie, TreeRow,
    fade, hoverable, meta, palette, presence, presence_of, section_label, space, tipped,
    tree_children, tree_tie,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    TestSupportExt as _, Transformation, WeakEntity, Window, div, px, radians,
};

use crate::actions::Command;
use crate::model::{Place, ago};
use crate::session::end_sentence;
use crate::session_link::PullRequest;
use crate::shell::{Selection, Shell};

/// The element id of a project's row.
pub fn project_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-{name}").into())
}

/// The element id of the "Ended" group, at the bottom of the tree.
pub fn ended_id() -> ElementId {
    ElementId::Name("ended".into())
}

/// The element id of the block inside the "Ended" group that holds its rows and scrolls.
pub fn ended_scroll_id() -> ElementId {
    ElementId::Name("ended-sessions".into())
}

/// The element id of the "⋯" menu a session's row offers.
pub fn session_menu_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-menu-{id}").into())
}

/// The element id of End session in a session's row menu.
pub fn session_end_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-end-{id}").into())
}

/// The element id of Resume in an ended session's row menu.
pub fn session_resume_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-resume-{id}").into())
}

/// The element id of "Rename…" in a session's row menu.
pub fn session_rename_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-rename-{id}").into())
}

/// The element id of "Detach from …" in the row menu of a session an agent started.
pub fn session_detach_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-detach-{id}").into())
}

/// The element id of the mark a led session's row starts with, whose tooltip names its pilot.
pub fn session_led_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-led-{id}").into())
}

/// The element id of a session's row.
pub fn session_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-{id}").into())
}

pub fn place_name(place: &Place) -> &str {
    place.as_deref().unwrap_or("main")
}

/// The element id of a session row's second line, which holds its place and its pull request.
pub fn session_line_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-line-{id}").into())
}

/// The element id of the place a session works in, on its second line: labelled `main` or the
/// workspace's name.
pub fn session_place_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-place-{id}").into())
}

/// The element id of the icon before the place's name: a folder (`Project folder`) or a branch
/// (`Workspace`).
pub fn session_place_icon_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-place-icon-{id}").into())
}

/// The element id of a session's pull request on its second line, labelled with its number and
/// its checks.
pub fn session_pull_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-pull-{id}").into())
}

/// The element id of the icon that says a command runs in a project.
pub fn project_commands_id(name: &str) -> ElementId {
    ElementId::Name(format!("commands-project-{name}").into())
}

/// The element id of the dot a session's row shows while something new in it waits to be seen.
pub fn session_unread_id(id: &str) -> ElementId {
    ElementId::Name(format!("unread-session-{id}").into())
}

/// The element id of the one mark at the right of a session's row, a state or the unread dot:
/// its label and its tooltip say what it means.
pub fn session_mark_id(id: &str) -> ElementId {
    ElementId::Name(format!("mark-session-{id}").into())
}

/// The element id of the pencil a session's row shows while its composer holds a draft the user
/// has not sent and has left.
pub fn session_draft_id(id: &str) -> ElementId {
    ElementId::Name(format!("draft-session-{id}").into())
}

/// The element id of the icon that says a command runs in a session.
pub fn session_commands_id(id: &str) -> ElementId {
    ElementId::Name(format!("commands-session-{id}").into())
}

/// What ends a row: the small terminal icon while a command of the project or the session runs,
/// so that no server is forgotten, then the row's own mark.
fn trailing(id: ElementId, commands: usize, last: impl IntoElement, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let said = match commands {
        1 => "A command is running".to_owned(),
        n => format!("{n} commands are running"),
    };
    div()
        .flex()
        .items_center()
        .gap(space::SM)
        .when(commands > 0, |d| {
            d.child(
                div()
                    .id(id)
                    .flex_none()
                    .aria_label(SharedString::from(said))
                    .child(
                        flottille_ui::icon(Icon::Terminal)
                            .size(px(12.))
                            .text_color(p.run),
                    )
                    .test_support(),
            )
        })
        .child(last)
}

/// The height of the one mark at the right of a session's row, which its tooltip hangs from.
const MARK: f32 = 16.;

/// A mark shown on a row: a state icon over its own model, shown or not.
struct Mark {
    shown: Option<AgentState>,
    status: Entity<Status>,
    icon: Entity<StateIcon>,
    /// Puts the model at rest once the icon has folded away: it keeps its state while it does.
    settling: Option<Task<()>>,
}

impl Mark {
    fn new<V: 'static>(cx: &mut Context<V>) -> Self {
        let status = cx.new(|_| Status::new(AgentState::Idle));
        let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
        Self {
            shown: None,
            status,
            icon,
            settling: None,
        }
    }

    /// Whether the row must be drawn again: only when the mark appears or goes away. A change
    /// from one mark to another redraws the icon alone.
    fn set<V: 'static>(&mut self, mark: Option<AgentState>, cx: &mut Context<V>) -> bool {
        let changed = self.shown.is_some() != mark.is_some();
        self.shown = mark;
        match mark {
            Some(state) => {
                self.settling = None;
                // GPUI Fast takes any update of an entity for a change: touch it only when it is
                // one.
                if self.status.read(cx).state() != state {
                    self.status.update(cx, |status, cx| status.set(state, cx));
                }
            }
            // A row at rest holds a still state, so that no ticker runs for it: once the icon,
            // which keeps the state it had, has folded away.
            None if changed => {
                let status = self.status.clone();
                self.settling = Some(cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(PANEL).await;
                    status.update(cx, |status, cx| status.set(AgentState::Idle, cx));
                }));
            }
            None => {}
        }
        changed
    }

    /// The icon, folding in when the mark appears and away when it goes.
    fn icon(&self, id: &str) -> impl IntoElement {
        presence(
            ElementId::Name(format!("mark-{id}").into()),
            self.shown.is_some(),
            div().flex().items_center().child(self.icon.clone()),
        )
    }
}

/// The side panel: the primary button, the `PROJECTS` label, the projects and, pinned at the
/// panel's bottom under a line whatever the tree's length, the "Ended" group, which unfolds upward
/// over the tree.
pub struct Sidebar {
    shell: WeakEntity<Shell>,
    projects: Vec<Entity<ProjectNode>>,
    /// The rows as drawn: a project that went folds away before it is gone.
    list: Listing<Entity<ProjectNode>>,
    /// The "Ended" group, and whether it holds a session: it folds away when it holds none.
    ended: Option<(Entity<EndedNode>, bool)>,
    /// The session whose ending the user is asked to confirm, and how many agents end.
    ending: Option<(String, usize)>,
    /// The count the confirmation said last, which it still shows while it fades out.
    said: usize,
    /// The sessions asked of the daemon from here: an answer that refuses is said here only for
    /// them (the session's page says its own).
    asked: HashSet<String>,
    /// What the daemon refused of what was asked from a row.
    refused: Option<SharedString>,
}

impl Sidebar {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            projects: Vec::new(),
            list: Listing::default(),
            ended: None,
            ending: None,
            said: 0,
            asked: HashSet::new(),
            refused: None,
        }
    }

    pub fn projects(&self) -> &[Entity<ProjectNode>] {
        &self.projects
    }

    pub fn set_projects(&mut self, projects: Vec<Entity<ProjectNode>>, cx: &mut Context<Self>) {
        if self.projects != projects {
            self.projects = projects;
            cx.notify();
        }
    }

    /// The "Ended" group, and whether it holds a session now.
    pub fn set_ended(&mut self, group: Entity<EndedNode>, present: bool, cx: &mut Context<Self>) {
        let same = self
            .ended
            .as_ref()
            .is_some_and(|(held, shown)| *held == group && *shown == present);
        if !same {
            self.ended = Some((group, present));
            cx.notify();
        }
    }

    /// Asks the user to confirm ending `session`, which ends `count` agents.
    pub fn ask_end(&mut self, session: String, count: usize, cx: &mut Context<Self>) {
        self.ending = Some((session, count));
        self.said = count;
        self.refused = None;
        cx.notify();
    }

    /// The session whose ending is asked.
    pub fn ending(&self) -> Option<&str> {
        self.ending.as_ref().map(|(session, _)| session.as_str())
    }

    fn cancel_end(&mut self, cx: &mut Context<Self>) {
        if self.ending.take().is_some() {
            cx.notify();
        }
    }

    fn confirm_end(&mut self, cx: &mut Context<Self>) {
        let Some((session, _)) = self.ending.take() else {
            return;
        };
        self.asked.insert(session.clone());
        self.shell
            .update(cx, |shell, _| shell.perform(Command::Stop { session }))
            .ok();
        cx.notify();
    }

    /// A resume asked from a row.
    pub fn resuming(&mut self, session: &str, cx: &mut Context<Self>) {
        self.asked.insert(session.to_owned());
        self.refused = None;
        cx.notify();
    }

    /// The daemon answered what was asked of `session` from a row: a refusal is said.
    pub fn answered(&mut self, session: &str, refused: Option<String>, cx: &mut Context<Self>) {
        if self.asked.remove(session) && refused.is_some() {
            self.refused = refused.map(SharedString::from);
            cx.notify();
        }
    }

    /// What the daemon refused of what was asked from a row.
    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }
}

/// The rows of a level inside their frames: one that comes unfolds, one that goes folds away.
///
/// A row rises into place as it comes, at its full size, unless `unfolds` says it unfolds from
/// nothing, as a fold opens (the sessions under a session's row): it is then shown as it folds.
fn framed<V: Render>(
    list: &mut Listing<Entity<V>>,
    rows: &[Entity<V>],
    scope: &str,
    unfolds: impl Fn(&Entity<V>, &App) -> bool,
    cx: &mut App,
) -> Vec<gpui_kit::AnyElement> {
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    list.sync(
        rows.iter()
            .map(|row| (row.entity_id().as_u64().to_string(), row.clone())),
        now,
        reduce,
    );
    list.rows()
        .map(|row| {
            let item = Item::new(
                ElementId::Name(format!("{scope}-{}", row.key).into()),
                row.placement,
                row.value.clone(),
            );
            match unfolds(row.value, cx) {
                true => item,
                false => item.rising(),
            }
            .into_any_element()
        })
        .collect()
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let projects = framed(
            &mut self.list,
            &self.projects,
            "tree-project",
            |_, _| false,
            cx,
        );
        let shell = self.shell.clone();
        let adds = self.shell.clone();
        let ended = self.ended.clone().map(|(group, present)| {
            // At the panel's bottom, under a line; its rows scroll inside their own block, and it
            // folds away when its last session leaves.
            presence(
                "sidebar-ended",
                present,
                div()
                    .id("ended-footer")
                    .flex_none()
                    .border_t_1()
                    .border_color(p.line)
                    .bg(p.side)
                    .px(space::SM)
                    .pt(space::XS)
                    .pb(space::SM)
                    .child(group)
                    .test_support(),
            )
        });
        let count = self.ending.as_ref().map_or(self.said, |(_, count)| *count);
        let this = cx.entity().downgrade();
        let modal = Modal::new("sidebar-end-modal", "End the session?")
            .open(self.ending.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.cancel_end(cx)).ok();
            })
            .child(
                div()
                    .id("sidebar-end-question")
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(end_sentence(count)))
                    .test_support(),
            )
            .action(
                Button::danger(
                    "sidebar-end-confirm",
                    match count {
                        0 | 1 => "End session".to_owned(),
                        n => format!("End {n} agents"),
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.confirm_end(cx))),
            );
        div()
            .id("sidebar")
            .size_full()
            .bg(p.side)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .pt(space::MD)
                    .px(space::MD)
                    .pb(space::XS)
                    .child(
                        Button::primary("new-session", "New session")
                            .leading(Icon::Plus)
                            .full_width()
                            .on_click(move |_, window, cx| {
                                shell
                                    .update(cx, |shell, cx| shell.new_session(window, cx))
                                    .ok();
                            }),
                    ),
            )
            .child(
                div()
                    .id("tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(space::SM)
                    .pt(space::SM)
                    .pb(space::LG)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pr(space::XS)
                            .child(section_label("Projects", cx))
                            .child(
                                Button::icon("add-project", Icon::Plus, "Add a project").on_click(
                                    move |_, window, cx| {
                                        adds.update(cx, |shell, cx| shell.add_project(window, cx))
                                            .ok();
                                    },
                                ),
                            ),
                    )
                    .children(projects),
            )
            .children(ended)
            .child(presence_of(
                "sidebar-refused-line",
                self.refused.clone(),
                move |line| {
                    div()
                        .id("sidebar-refused")
                        .aria_label(line.clone())
                        .px(space::LG)
                        .pb(space::MD)
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(line)
                        .test_support()
                        .into_any_element()
                },
            ))
            .child(modal)
            .test_support()
    }
}

/// A project's row and, folding under it, its sessions in one list.
pub struct ProjectNode {
    name: String,
    open: bool,
    mark: Mark,
    /// The strongest mark of what it holds, which it shows only while it is folded: open, the
    /// work is on its own row.
    held: Option<AgentState>,
    /// How many commands its agents run now.
    commands: usize,
    sessions: Entity<Level>,
    fold: Entity<Fold>,
    shell: WeakEntity<Shell>,
}

impl ProjectNode {
    pub fn new(name: String, shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        let sessions = cx.new(|_| Level::new());
        let fold = cx.new(|_| Fold::new(format!("fold-project-{name}"), sessions.clone(), true));
        Self {
            name,
            open: true,
            mark: Mark::new(cx),
            held: None,
            commands: 0,
            sessions,
            fold,
            shell,
        }
    }

    pub fn mark(&self) -> Option<AgentState> {
        self.mark.shown
    }

    /// How many commands its agents run now.
    pub fn commands(&self) -> usize {
        self.commands
    }

    pub fn set_commands(&mut self, commands: usize, cx: &mut Context<Self>) {
        if self.commands != commands {
            self.commands = commands;
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Its sessions' rows in the order drawn: the one that moved last first.
    pub fn sessions(&self, cx: &App) -> Vec<Entity<SessionRow>> {
        self.sessions.read(cx).rows.clone()
    }

    /// Whether it holds these sessions and this mark already.
    pub fn holds(
        &self,
        sessions: &[Entity<SessionRow>],
        mark: Option<AgentState>,
        cx: &App,
    ) -> bool {
        self.held == mark && self.sessions.read(cx).rows == sessions
    }

    pub fn set(
        &mut self,
        sessions: Vec<Entity<SessionRow>>,
        mark: Option<AgentState>,
        cx: &mut Context<Self>,
    ) {
        if self.sessions.read(cx).rows != sessions {
            self.sessions
                .update(cx, |level, cx| level.set(sessions, cx));
        }
        self.held = mark;
        self.show_mark(cx);
    }

    /// Its sessions' rows alone, as a fold under a session's row changed them.
    pub fn set_rows(&mut self, sessions: Vec<Entity<SessionRow>>, cx: &mut Context<Self>) {
        if self.sessions.read(cx).rows != sessions {
            self.sessions
                .update(cx, |level, cx| level.set(sessions, cx));
        }
    }

    fn show_mark(&mut self, cx: &mut Context<Self>) {
        let shown = self.held.filter(|_| !self.open);
        if self.mark.set(shown, cx) {
            cx.notify();
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        let open = self.open;
        self.fold.update(cx, |fold, cx| fold.set_open(open, cx));
        self.show_mark(cx);
        cx.notify();
    }
}

impl Render for ProjectNode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = self.shell.clone();
        let name = self.name.clone();
        let row = TreeRow::new(
            project_id(&self.name),
            TreeLevel::Project,
            self.name.clone(),
        )
        .expanded(self.open)
        .trailing(trailing(
            project_commands_id(&self.name),
            self.commands,
            self.mark.icon(&format!("project-{}", self.name)),
            cx,
        ))
        .on_toggle(cx.listener(|this, _, _, cx| this.toggle(cx)))
        .on_click(cx.listener(move |_, _, window, cx| {
            let project = name.clone();
            shell
                .update(cx, |shell, cx| {
                    shell.open(Selection::Project(project), window, cx)
                })
                .ok();
        }));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(row)
            .child(self.fold.clone())
    }
}

/// The rows of one level, on a guide line: the sessions of a project, or of the "Ended" group.
pub struct Level {
    rows: Vec<Entity<SessionRow>>,
    list: Listing<Entity<SessionRow>>,
    /// Takes at most [`ENDED_ROWS`] rows, or [`ENDED_SHARE`] of the window when that is less, and
    /// scrolls inside itself beyond it.
    scrolls: bool,
}

/// How many rows the "Ended" group shows before it scrolls.
const ENDED_ROWS: f32 = 8.;
/// The height of a session's row of one line, which the "Ended" group's rows are.
const ROW: f32 = 28.;
/// The share of the window's height the "Ended" group takes at most, when that is less than
/// [`ENDED_ROWS`] rows.
const ENDED_SHARE: f32 = 0.35;

impl Level {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            list: Listing::default(),
            scrolls: false,
        }
    }

    /// The level of the "Ended" group, which scrolls inside itself.
    fn scrolling() -> Self {
        Self {
            scrolls: true,
            ..Self::new()
        }
    }

    fn set(&mut self, rows: Vec<Entity<SessionRow>>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }
}

impl Render for Level {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = framed(
            &mut self.list,
            &self.rows,
            "tree-row",
            |row, cx| row.read(cx).led_by().is_some(),
            cx,
        );
        if rows.is_empty() {
            return div().into_any_element();
        }
        let rows = tree_children(rows, cx);
        if !self.scrolls {
            return rows.into_any_element();
        }
        let most = (px(ENDED_ROWS * ROW) + px(4.)).min(window.viewport_size().height * ENDED_SHARE);
        div()
            .id(ended_scroll_id())
            .max_h(most)
            .overflow_y_scroll()
            .child(rows)
            .test_support()
            .into_any_element()
    }
}

/// The "Ended" group, at the bottom of the tree: it gathers the ended sessions of every project,
/// folded until the user opens it.
pub struct EndedNode {
    open: bool,
    sessions: Entity<Level>,
    fold: Entity<Fold>,
}

impl EndedNode {
    /// The group, folded.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let sessions = cx.new(|_| Level::scrolling());
        let fold = cx.new(|_| Fold::new("fold-ended", sessions.clone(), false));
        Self {
            open: false,
            sessions,
            fold,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Its sessions' rows, newest first.
    pub fn sessions(&self, cx: &gpui_kit::App) -> Vec<Entity<SessionRow>> {
        self.sessions.read(cx).rows.clone()
    }

    /// Whether it holds these sessions already.
    pub fn holds(&self, sessions: &[Entity<SessionRow>], cx: &App) -> bool {
        self.sessions.read(cx).rows == sessions
    }

    pub fn set(&mut self, sessions: Vec<Entity<SessionRow>>, cx: &mut Context<Self>) {
        if self.sessions.read(cx).rows != sessions {
            self.sessions
                .update(cx, |level, cx| level.set(sessions, cx));
            // The row counts them.
            cx.notify();
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        let open = self.open;
        self.fold.update(cx, |fold, cx| fold.set_open(open, cx));
        cx.notify();
    }
}

impl Render for EndedNode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.sessions.read(cx).rows.len();
        let row = TreeRow::new(ended_id(), TreeLevel::Session, "Ended")
            .unfolds_upward()
            .trailing(meta(count.to_string(), cx))
            .expanded(self.open)
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(row)
            .child(self.fold.clone())
    }
}

/// What ends a session's row: one mark, chosen in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trailing {
    /// The session runs, needs the user or waits.
    State(AgentState),
    /// Something new was written in it since it was last opened.
    Unread,
    /// How long ago it was last active.
    Ago,
}

/// A session's row: its title, and on the right one mark: its state when it runs, needs the user
/// or waits, else a dot when something new waits to be seen, else how long ago it was last
/// active.
pub struct SessionRow {
    id: String,
    /// How many times it was drawn: the tests count them.
    renders: usize,
    title: SharedString,
    mark: Mark,
    /// How many commands its agents run now.
    commands: usize,
    /// What is new in the session since it was last opened, the last of it in words: a small dot
    /// on the row says it until the session is opened, unless a state takes its place.
    unread: Option<SharedString>,
    /// Its composer holds something unsent: a pencil says it once the session is not the open
    /// one.
    draft: bool,
    /// What its state mark means, in words.
    said: SharedString,
    /// The words of the mark shown, kept while it folds away.
    words: SharedString,
    active_at: u64,
    ago: SharedString,
    selected: bool,
    /// The project it belongs to, said in the "Ended" group where projects are mixed; a row with
    /// none is a live one, which says its place on a second line.
    scope: Option<SharedString>,
    /// Where it works: `main`, or a workspace.
    place: Place,
    /// Its place's pull request, when the window has read it, and the last one it showed, which
    /// stays drawn while it fades out.
    pull: Option<PullRequest>,
    pull_shown: Option<PullRequest>,
    /// What its "⋯" menu offers: End session while it lives, Resume when it ended and can.
    actions: Actions,
    /// The agent that started it, while it is not detached: its menu offers "Detach from …".
    detach: Option<SharedString>,
    menu: Option<(Entity<Menu>, Subscription)>,
    /// The agent that leads it, while its row sits under its pilot's: the row is indented, and a
    /// mark before its title says who leads it.
    led_by: Option<SharedString>,
    /// What the mark said last, kept while it fades out.
    led_said: SharedString,
    /// The state of the session itself, which the mark shows unless it hides a led session that
    /// asks more of the user.
    own: Option<AgentState>,
    /// The most urgent state of the led sessions its folded row hides.
    hidden: Option<AgentState>,
    /// How many led sessions sit under its row, and whether they are unfolded.
    leads: usize,
    open: bool,
    /// Where its row sits among the sessions its pilot leads, which the connector to the row
    /// above draws, and the last one it drew, which stays while it fades out.
    tie: Option<Tie>,
    tie_shown: Tie,
    shell: WeakEntity<Shell>,
}

/// What a session's row offers under the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Actions {
    #[default]
    None,
    End,
    Resume,
}

impl SessionRow {
    pub fn new(id: String, shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        Self {
            id,
            renders: 0,
            title: SharedString::default(),
            mark: Mark::new(cx),
            commands: 0,
            unread: None,
            draft: false,
            said: SharedString::default(),
            words: SharedString::default(),
            active_at: 0,
            ago: SharedString::default(),
            selected: false,
            scope: None,
            place: None,
            pull: None,
            pull_shown: None,
            actions: Actions::None,
            detach: None,
            menu: None,
            led_by: None,
            led_said: SharedString::default(),
            own: None,
            hidden: None,
            leads: 0,
            open: true,
            tie: None,
            tie_shown: Tie::default(),
            shell,
        }
    }

    /// Where its row sits among the sessions its pilot leads, `None` for a row that no session
    /// leads.
    pub fn tie(&self) -> Option<&Tie> {
        self.tie.as_ref()
    }

    pub fn set_tie(&mut self, tie: Option<Tie>, cx: &mut Context<Self>) {
        if self.tie != tie {
            if let Some(tie) = &tie {
                self.tie_shown = tie.clone();
            }
            self.tie = tie;
            cx.notify();
        }
    }

    /// The agent that leads it, while its row sits under its pilot's.
    pub fn led_by(&self) -> Option<&str> {
        self.led_by.as_deref()
    }

    pub fn set_led_by(&mut self, led_by: Option<&str>, cx: &mut Context<Self>) {
        if self.led_by() != led_by {
            self.led_by = led_by.map(|by| SharedString::from(by.to_owned()));
            if let Some(by) = &self.led_by {
                self.led_said = format!("led by {}", flottille_ui::quoted_title(by)).into();
            }
            cx.notify();
        }
    }

    /// How many led sessions sit under its row.
    pub fn leads(&self) -> usize {
        self.leads
    }

    pub fn set_leads(&mut self, leads: usize, cx: &mut Context<Self>) {
        if self.leads != leads {
            self.leads = leads;
            cx.notify();
        }
    }

    /// What its folded row hides.
    pub fn hidden(&self) -> Option<AgentState> {
        self.hidden
    }

    /// The most urgent state among the led sessions its folded row hides: the mark shows it when
    /// it asks more than the session's own. `None` while the row is unfolded.
    pub fn set_hidden(&mut self, hidden: Option<AgentState>, cx: &mut Context<Self>) {
        if self.hidden != hidden {
            self.hidden = hidden;
            if self.mark.set(self.shown_mark(), cx) {
                cx.notify();
            }
        }
    }

    /// What the mark shows: waiting for the user beats running, which beats the session's own.
    fn shown_mark(&self) -> Option<AgentState> {
        AgentState::strongest(self.own.into_iter().chain(self.hidden)).or(self.own)
    }

    /// Whether the led sessions under its row are unfolded.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Folds or unfolds the led sessions under its row: the tree draws its level again.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        cx.notify();
        // The level reads this row: it is drawn again once this update is over.
        let (shell, id) = (self.shell.clone(), self.id.clone());
        cx.defer(move |cx| {
            shell.update(cx, |shell, cx| shell.relist(&id, cx)).ok();
        });
    }

    /// Says its project in its meta (the "Ended" group), or stops.
    pub fn set_scope(&mut self, scope: Option<&str>, cx: &mut Context<Self>) {
        if self.scope.as_deref() != scope {
            self.scope = scope.map(|scope| SharedString::from(scope.to_owned()));
            cx.notify();
        }
    }

    /// The place it works in, `None` for the project folder.
    pub fn place(&self) -> &Place {
        &self.place
    }

    pub fn set_place(&mut self, place: &Place, cx: &mut Context<Self>) {
        if &self.place != place {
            self.place = place.clone();
            cx.notify();
        }
    }

    /// The pull request of its place, which the second line shows when there is room.
    pub fn pull(&self) -> Option<&PullRequest> {
        self.pull.as_ref()
    }

    pub fn set_pull(&mut self, pull: Option<&PullRequest>, cx: &mut Context<Self>) {
        if self.pull.as_ref() != pull {
            self.pull = pull.cloned();
            if self.pull.is_some() {
                self.pull_shown = self.pull.clone();
            }
            cx.notify();
        }
    }

    /// What the row's menu offers: End session for a session that lives, and "Detach from …"
    /// while an agent that started it (`detach`) still has it; Resume for an ended one whose
    /// agent can resume; nothing else.
    pub fn set_actions(
        &mut self,
        ended: bool,
        resumable: bool,
        detach: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let actions = match (ended, resumable) {
            (false, _) => Actions::End,
            (true, true) => Actions::Resume,
            (true, false) => Actions::None,
        };
        let detach = detach.filter(|_| actions == Actions::End);
        if self.actions == actions && self.detach.as_deref() == detach {
            return;
        }
        self.actions = actions;
        self.detach = detach.map(|by| SharedString::from(by.to_owned()));
        let detach = self.detach.clone();
        self.menu = match actions {
            Actions::None => None,
            offered => {
                let (shell, id) = (self.shell.clone(), self.id.clone());
                let menu = cx.new(|cx| {
                    let mut menu = Menu::new(session_menu_id(&id), "Session actions", cx);
                    {
                        let (shell, id) = (shell.clone(), id.clone());
                        menu = menu.entry(
                            session_rename_id(&id),
                            crate::rename::RENAME_LABEL,
                            Some(Icon::Edit),
                            move |window, cx| {
                                let id = id.clone();
                                shell
                                    .update(cx, |shell, cx| shell.rename_of(&id, window, cx))
                                    .ok();
                            },
                        );
                    }
                    if let Some(by) = detach {
                        let (shell, id) = (shell.clone(), id.clone());
                        menu = menu.entry(
                            session_detach_id(&id),
                            format!("Detach from {}", flottille_ui::quoted_title(&by)),
                            None,
                            move |_, cx| {
                                let id = id.clone();
                                shell.update(cx, |shell, cx| shell.detach_of(&id, cx)).ok();
                            },
                        );
                    }
                    match offered {
                        Actions::End => menu.entry(
                            session_end_id(&id),
                            "End session…",
                            Some(Icon::Stop),
                            move |_, cx| {
                                let id = id.clone();
                                shell.update(cx, |shell, cx| shell.ask_end_of(&id, cx)).ok();
                            },
                        ),
                        _ => menu.entry(session_resume_id(&id), "Resume", None, move |_, cx| {
                            let id = id.clone();
                            shell.update(cx, |shell, cx| shell.resume_of(&id, cx)).ok();
                        }),
                    }
                });
                // The row keeps the menu in sight while it is open: it draws again as it changes.
                let watch = cx.observe(&menu, |_, _, cx| cx.notify());
                Some((menu, watch))
            }
        };
        cx.notify();
    }

    pub fn mark(&self) -> Option<AgentState> {
        self.mark.shown
    }

    /// How many commands its agents run now.
    pub fn commands(&self) -> usize {
        self.commands
    }

    pub fn set_commands(&mut self, commands: usize, cx: &mut Context<Self>) {
        if self.commands != commands {
            self.commands = commands;
            cx.notify();
        }
    }

    /// What is new in the session since it was last opened, in words.
    pub fn unread(&self) -> Option<&str> {
        self.unread.as_deref()
    }

    pub fn set_unread(&mut self, unread: Option<&str>, cx: &mut Context<Self>) {
        if self.unread() != unread {
            self.unread = unread.map(|said| SharedString::from(said.to_owned()));
            cx.notify();
        }
    }

    /// Whether its composer holds a draft, in the session being open or not.
    pub fn draft(&self) -> bool {
        self.draft
    }

    pub fn set_draft(&mut self, draft: bool, cx: &mut Context<Self>) {
        if self.draft != draft {
            self.draft = draft;
            cx.notify();
        }
    }

    /// What its state mark means, in words.
    pub fn said(&self) -> &str {
        &self.said
    }

    pub fn set_said(&mut self, said: &str, cx: &mut Context<Self>) {
        if self.said() != said {
            self.said = SharedString::from(said.to_owned());
            cx.notify();
        }
    }

    /// The one mark the row ends with: its state, else the unread dot, else the time ago.
    pub fn trailing(&self) -> Trailing {
        match (self.mark.shown, &self.unread) {
            (Some(state), _) => Trailing::State(state),
            (None, Some(_)) => Trailing::Unread,
            (None, None) => Trailing::Ago,
        }
    }

    /// The session's root agent.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The model the row's mark reads; the session page's pill reads it too.
    pub fn status(&self) -> Entity<Status> {
        self.mark.status.clone()
    }

    /// Whether it shows this already.
    pub fn holds(&self, title: &str, mark: Option<AgentState>, active_at: u64) -> bool {
        self.title.as_ref() == title && self.own == mark && self.active_at == active_at
    }

    pub fn set(
        &mut self,
        title: &str,
        mark: Option<AgentState>,
        active_at: u64,
        now: u64,
        cx: &mut Context<Self>,
    ) {
        self.own = mark;
        let mut changed = self.mark.set(self.shown_mark(), cx);
        if self.title.as_ref() != title {
            self.title = SharedString::from(title.to_owned());
            changed = true;
        }
        // Time passing alone is the clock's to show (`tick`): a read of the daemon that changed
        // nothing of this session draws nothing.
        if self.active_at != active_at || self.ago.is_empty() {
            self.active_at = active_at;
            changed |= self.set_ago(now);
        }
        if changed {
            cx.notify();
        }
    }

    /// Whether the time ago it shows is no longer the right one.
    pub fn ago_changes(&self, now: u64) -> bool {
        self.mark.shown.is_none() && self.ago.as_ref() != ago(now, self.active_at)
    }

    /// The time ago moves on by itself: drawn again only when its text changes.
    pub fn tick(&mut self, now: u64, cx: &mut Context<Self>) {
        if self.mark.shown.is_none() && self.set_ago(now) {
            cx.notify();
        }
    }

    fn set_ago(&mut self, now: u64) -> bool {
        let ago = ago(now, self.active_at);
        let changed = self.ago.as_ref() != ago;
        if changed {
            self.ago = ago.into();
        }
        changed && self.mark.shown.is_none()
    }

    pub fn set_selected(&mut self, selected: bool, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }
}

impl Render for SessionRow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let shell = self.shell.clone();
        let id = self.id.clone();
        let mut row = TreeRow::new(session_id(&self.id), TreeLevel::Session, self.title.clone())
            .selected(self.selected)
            .on_click(move |_, window, cx| {
                let session = id.clone();
                shell
                    .update(cx, |shell, cx| {
                        shell.open(Selection::Session(session), window, cx)
                    })
                    .ok();
            })
            .when_some(self.menu.as_ref(), |row, (menu, _)| {
                row.actions(menu.clone(), menu.read(cx).is_open())
            })
            .when(self.leads > 0, |row| {
                row.expanded(self.open)
                    .on_toggle(cx.listener(|this, _, _, cx| this.toggle(cx)))
            });
        let when = match &self.scope {
            Some(scope) => SharedString::from(format!("{scope} · {}", self.ago)),
            None => self.ago.clone(),
        };
        let id = session_commands_id(&self.id);
        let trailing_now = self.trailing();
        // The mark keeps the last words it said while it folds away.
        let words = match trailing_now {
            Trailing::State(_) => Some(self.said.clone()),
            Trailing::Unread => self.unread.clone(),
            Trailing::Ago => None,
        };
        if let Some(words) = words.filter(|words| !words.is_empty()) {
            self.words = words;
        }
        let p = palette(cx);
        let unread = presence(
            ElementId::Name(format!("session-unread-fade-{}", self.id).into()),
            trailing_now == Trailing::Unread,
            div()
                .id(session_unread_id(&self.id))
                .flex()
                .items_center()
                .justify_center()
                .w(px(14.))
                .h(px(MARK))
                .child(div().size(px(6.)).rounded_full().bg(p.acc))
                .test_support(),
        );
        let draft = presence(
            ElementId::Name(format!("session-draft-fade-{}", self.id).into()),
            self.draft && !self.selected,
            self.draft_mark(window, cx).into_any_element(),
        );
        // One mark: the state, the dot or the time take each other's place, one folding away as
        // the other comes. The state and the dot share the tooltip that says what they mean.
        let mark = flottille_ui::tipped(
            div()
                .id(session_mark_id(&self.id))
                .flex()
                .items_center()
                .h(px(MARK))
                .aria_label(self.words.clone())
                .child(self.mark.icon(&self.id))
                .child(unread),
            self.words.clone(),
            MARK,
            window,
            cx,
        );
        let last = div()
            .flex()
            .items_center()
            .child(draft)
            .child(mark)
            .child(presence(
                ElementId::Name(format!("meta-{}", self.id).into()),
                trailing_now == Trailing::Ago,
                meta(when, cx),
            ))
            .into_any_element();
        if self.scope.is_none() {
            row = row.second(self.second_line(window, cx));
        }
        // The connector to the row above, and the indent it runs into, fade with the row.
        let tied = fade(
            ElementId::Name(format!("session-tie-fade-{}", self.id).into()),
            self.tie.is_some(),
            PANEL,
            window,
            cx,
        );
        let first_line = match self.scope {
            None => (TWO_LINES - (FIRST + LINE)) / 2. + FIRST / 2.,
            Some(_) => 14.,
        };
        let arrow = (tied > 0.0).then(|| self.led_arrow(window, cx));
        div()
            .relative()
            .pl(space::XL * (self.tie_shown.depth() as f32 * tied))
            .when_some(arrow, |d, arrow| {
                d.child(tree_tie(
                    session_id(&self.id),
                    &self.tie_shown,
                    first_line,
                    tied,
                    arrow,
                    cx,
                ))
            })
            .child(row.trailing(trailing(id, self.commands, last, cx)))
    }
}

/// The words the pencil of a draft says.
const DRAFT_SAID: &str = "Draft not sent";

impl SessionRow {
    /// The arrowhead that ends the connector of a led row, saying who leads it.
    fn led_arrow(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        tipped(
            div()
                .id(session_led_id(&self.id))
                .flex()
                .items_center()
                .justify_center()
                .size(px(12.))
                .aria_label(self.led_said.clone())
                .child(
                    flottille_ui::icon(Icon::Chevron)
                        .size(px(12.))
                        .text_color(p.faint)
                        .with_transformation(Transformation::rotate(radians(-FRAC_PI_2))),
                ),
            self.led_said.clone(),
            12.,
            window,
            cx,
        )
        .into_any_element()
    }

    /// The pencil of an unsent draft, beside the row's own mark: its ink warms on the pointer.
    fn draft_mark(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pencil = hoverable(
            div()
                .id(ElementId::Name(format!("draft-ink-{}", self.id).into()))
                .flex()
                .items_center()
                .justify_center()
                .w(px(14.))
                .h(px(MARK)),
            move |element, t| {
                element.child(
                    flottille_ui::icon(Icon::Edit)
                        .size(px(12.))
                        .text_color(mix(p.mute, p.text, t)),
                )
            },
        );
        flottille_ui::tipped(
            div()
                .id(session_draft_id(&self.id))
                .flex_none()
                .h(px(MARK))
                .aria_label(DRAFT_SAID)
                .child(pencil),
            DRAFT_SAID,
            MARK,
            window,
            cx,
        )
    }
}

/// The height of a session's second line.
const LINE: f32 = 16.;
/// The height of a row's first line, above its second.
const FIRST: f32 = 18.;

impl SessionRow {
    /// Under the title, faint: the place's icon and name, then, when there is room, its pull
    /// request and its checks' mark. The line is one line high and wraps: what does not fit goes
    /// under it, out of sight, and the name is cut with an ellipsis. The whole is in a tooltip.
    fn second_line(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (name, icon, said) = match &self.place {
            None => ("main".to_owned(), Icon::Folder, "Project folder"),
            Some(name) => (name.clone(), Icon::Branch, "Workspace"),
        };
        // It fades in when the pull request becomes known, and out when it is gone.
        let shown = fade(
            ElementId::Name(format!("session-pull-fade-{}", self.id).into()),
            self.pull.is_some(),
            ENTER,
            window,
            cx,
        );
        let pull = self
            .pull_shown
            .clone()
            .filter(|_| self.pull.is_some() || shown > 0.0);
        let tip = match &pull {
            Some(pull) => format!("{name} · {}", pull_said(pull)),
            None => name.clone(),
        };
        let place = div()
            .id(session_place_id(&self.id))
            .aria_label(SharedString::from(name.clone()))
            .flex()
            .items_center()
            .gap(space::XS)
            .max_w_full()
            .min_w_0()
            .h(px(LINE))
            .child(
                div()
                    .id(session_place_icon_id(&self.id))
                    .aria_label(said)
                    .flex_none()
                    .child(flottille_ui::icon(icon).size(px(11.)).text_color(p.faint))
                    .test_support(),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(name)),
            )
            .test_support();
        let line = div()
            .id(session_line_id(&self.id))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(space::SM)
            .h(px(LINE))
            .overflow_hidden()
            .text_size(px(11.5))
            .text_color(p.faint)
            .child(place)
            .children(pull.map(|pull| {
                let (mark, color) = pull_mark(&pull, &p);
                div()
                    .id(session_pull_id(&self.id))
                    .aria_label(SharedString::from(pull_said(&pull)))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .h(px(LINE))
                    .opacity(shown)
                    .child(SharedString::from(format!("#{}", pull.number)))
                    .children(
                        mark.map(|mark| flottille_ui::icon(mark).size(px(11.)).text_color(color)),
                    )
                    .test_support()
            }));
        tipped(line, SharedString::from(tip), LINE, window, cx)
    }
}

/// A pull request's number and the state of its checks, said: `#12, checks passing`.
fn pull_said(pull: &PullRequest) -> String {
    let checks = match pull.mark.as_deref() {
        Some("passing") => ", checks passing",
        Some("failing") => ", checks failing",
        Some(_) => ", checks pending",
        None => "",
    };
    format!("#{}{checks}", pull.number)
}

/// The mark of a pull request's checks, drawn by shape as well as by colour: a check, a cross, a
/// dot; none without checks.
fn pull_mark(pull: &PullRequest, p: &flottille_ui::Palette) -> (Option<Icon>, gpui_kit::Hsla) {
    match pull.mark.as_deref() {
        Some("passing") => (Some(Icon::Check), p.ok),
        Some("failing") => (Some(Icon::Close), p.fail),
        Some(_) => (Some(Icon::Running), p.run),
        None => (None, p.faint),
    }
}
