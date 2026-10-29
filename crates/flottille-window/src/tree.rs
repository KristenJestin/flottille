//! The left tree: "New session", then project › place › session. Every row is its own view, so
//! one session changing redraws its row and nothing else; a folded project or place shows the
//! strongest mark of what it holds.

use flottille_ui::rows::TreeLevel;
use flottille_ui::{
    AgentState, Button, Fold, Icon, Item, Listing, StateIcon, Status, TreeRow, enter, meta,
    palette, section_label, tree_children,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    WeakEntity, Window, div, px,
};

use crate::model::{Place, ago};
use crate::shell::{Selection, Shell};

/// The element id of a project's row.
pub fn project_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-{name}").into())
}

/// The element id of a place's row: `place-<project>-main` or `place-<project>-<workspace>`.
pub fn place_id(project: &str, place: &Place) -> ElementId {
    ElementId::Name(format!("place-{project}-{}", place_name(place)).into())
}

/// The element id of a project's "Ended" group.
pub fn ended_id(project: &str) -> ElementId {
    ElementId::Name(format!("ended-{project}").into())
}

/// The element id of a session's row.
pub fn session_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-{id}").into())
}

pub fn place_name(place: &Place) -> &str {
    place.as_deref().unwrap_or("main")
}

/// The element id of the icon that says a command runs in a project.
pub fn project_commands_id(name: &str) -> ElementId {
    ElementId::Name(format!("commands-project-{name}").into())
}

/// The element id of the icon that says a command runs in a session.
/// The element id of the small text a session started by an agent says it with.
pub fn session_started_by_id(id: &str) -> ElementId {
    ElementId::Name(format!("started-by-session-{id}").into())
}

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
        .gap(px(6.))
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

/// A mark shown on a row: a state icon over its own model, shown or not.
struct Mark {
    shown: Option<AgentState>,
    status: Entity<Status>,
    icon: Entity<StateIcon>,
}

impl Mark {
    fn new<V: 'static>(cx: &mut Context<V>) -> Self {
        let status = cx.new(|_| Status::new(AgentState::Idle));
        let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
        Self {
            shown: None,
            status,
            icon,
        }
    }

    /// Whether the row must be drawn again: only when the mark appears or goes away. A change
    /// from one mark to another redraws the icon alone.
    fn set<V: 'static>(&mut self, mark: Option<AgentState>, cx: &mut Context<V>) -> bool {
        // A row at rest holds a still state, so that no ticker runs for it.
        let state = mark.unwrap_or(AgentState::Idle);
        // GPUI Fast takes any update of an entity for a change: touch it only when it is one.
        if self.status.read(cx).state() != state {
            self.status.update(cx, |status, cx| status.set(state, cx));
        }
        let changed = self.shown.is_some() != mark.is_some();
        self.shown = mark;
        changed
    }
}

/// The side panel: the primary button, the `PROJECTS` label and the projects.
pub struct Sidebar {
    shell: WeakEntity<Shell>,
    projects: Vec<Entity<ProjectNode>>,
    /// The rows as drawn: a project that went folds away before it is gone.
    list: Listing<Entity<ProjectNode>>,
}

impl Sidebar {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            projects: Vec::new(),
            list: Listing::default(),
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
}

/// The rows of a level inside their frames: one that comes unfolds, one that goes folds away.
fn framed<V: Render>(
    list: &mut Listing<Entity<V>>,
    rows: &[Entity<V>],
    scope: &str,
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
            Item::new(
                ElementId::Name(format!("{scope}-{}", row.key).into()),
                row.placement,
                row.value.clone(),
            )
            .rising()
            .into_any_element()
        })
        .collect()
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let projects = framed(&mut self.list, &self.projects, "tree-project", cx);
        let shell = self.shell.clone();
        let adds = self.shell.clone();
        div()
            .size_full()
            .bg(p.side)
            .flex()
            .flex_col()
            .child(
                div().flex_none().pt(px(12.)).px(px(12.)).pb(px(4.)).child(
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
                    .px(px(8.))
                    .pt(px(8.))
                    .pb(px(14.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pr(px(4.))
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
    }
}

/// A project's row and, folding under it, its places.
pub struct ProjectNode {
    name: String,
    open: bool,
    mark: Mark,
    /// How many commands its agents run now.
    commands: usize,
    places: Entity<Level<PlaceNode>>,
    fold: Entity<Fold>,
    shell: WeakEntity<Shell>,
}

impl ProjectNode {
    pub fn new(name: String, shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        let places = cx.new(|_| Level::new());
        let fold = cx.new(|_| Fold::new(format!("fold-project-{name}"), places.clone(), true));
        Self {
            name,
            open: true,
            mark: Mark::new(cx),
            commands: 0,
            places,
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

    /// Whether it shows these places and this mark already.
    pub fn holds(&self, places: &[Entity<PlaceNode>], mark: Option<AgentState>, cx: &App) -> bool {
        self.mark.shown == mark && self.places.read(cx).rows == places
    }

    pub fn set(
        &mut self,
        places: Vec<Entity<PlaceNode>>,
        mark: Option<AgentState>,
        cx: &mut Context<Self>,
    ) {
        if self.places.read(cx).rows != places {
            self.places.update(cx, |level, cx| level.set(places, cx));
        }
        if self.mark.set(mark, cx) {
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
        .when(self.mark.shown.is_some() || self.commands > 0, |row| {
            row.trailing(trailing(
                project_commands_id(&self.name),
                self.commands,
                div().children(self.mark.shown.map(|_| self.mark.icon.clone())),
                cx,
            ))
        })
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

/// The rows of one level, on a guide line: the places of a project, the sessions of a place.
pub struct Level<V> {
    rows: Vec<Entity<V>>,
    list: Listing<Entity<V>>,
}

impl<V> Level<V> {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            list: Listing::default(),
        }
    }

    fn set(&mut self, rows: Vec<Entity<V>>, cx: &mut Context<Self>)
    where
        V: 'static,
    {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }
}

impl<V: Render> Render for Level<V> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = framed(&mut self.list, &self.rows, "tree-row", cx);
        if rows.is_empty() {
            return div().into_any_element();
        }
        tree_children(rows, cx).into_any_element()
    }
}

/// A place's row (`main` with a folder icon, a workspace with a git-branch icon) and, folding
/// under it, its sessions; or, at the bottom of a project, the "Ended" group that gathers its
/// ended sessions, folded until the user opens it.
pub struct PlaceNode {
    project: String,
    place: Place,
    /// The "Ended" group: no place of its own.
    ended: bool,
    open: bool,
    mark: Mark,
    sessions: Entity<Level<SessionRow>>,
    fold: Entity<Fold>,
}

impl PlaceNode {
    pub fn new(project: String, place: Place, cx: &mut Context<Self>) -> Self {
        let sessions = cx.new(|_| Level::new());
        let fold = cx.new(|_| {
            Fold::new(
                format!("fold-place-{project}-{}", place_name(&place)),
                sessions.clone(),
                true,
            )
        });
        Self {
            project,
            place,
            ended: false,
            open: true,
            mark: Mark::new(cx),
            sessions,
            fold,
        }
    }

    /// The "Ended" group of a project, folded.
    pub fn ended(project: String, cx: &mut Context<Self>) -> Self {
        let sessions = cx.new(|_| Level::new());
        let fold = cx.new(|_| Fold::new(format!("fold-ended-{project}"), sessions.clone(), false));
        Self {
            project,
            place: None,
            ended: true,
            open: false,
            mark: Mark::new(cx),
            sessions,
            fold,
        }
    }

    pub fn mark(&self) -> Option<AgentState> {
        self.mark.shown
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Its sessions' rows, newest first.
    pub fn sessions(&self, cx: &gpui_kit::App) -> Vec<Entity<SessionRow>> {
        self.sessions.read(cx).rows.clone()
    }

    /// Whether it shows these sessions and this mark already.
    pub fn holds(
        &self,
        sessions: &[Entity<SessionRow>],
        mark: Option<AgentState>,
        cx: &App,
    ) -> bool {
        self.mark.shown == mark && self.sessions.read(cx).rows == sessions
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
            // The "Ended" row counts them.
            if self.ended {
                cx.notify();
            }
        }
        if self.mark.set(mark, cx) {
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

impl Render for PlaceNode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = match (self.ended, &self.place) {
            (true, _) => {
                let count = self.sessions.read(cx).rows.len();
                TreeRow::new(ended_id(&self.project), TreeLevel::Session, "Ended")
                    .trailing(meta(count.to_string(), cx))
            }
            (false, place) => TreeRow::new(
                place_id(&self.project, place),
                match place {
                    None => TreeLevel::Main,
                    Some(_) => TreeLevel::Workspace,
                },
                SharedString::from(place_name(place).to_owned()),
            )
            .when_some(self.mark.shown, |row, _| {
                row.trailing(self.mark.icon.clone())
            }),
        }
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

/// A session's row: its title, and on the right its mark when it runs or needs the user, else
/// how long ago it was last active.
pub struct SessionRow {
    id: String,
    /// How many times it was drawn: the tests count them.
    renders: usize,
    title: SharedString,
    mark: Mark,
    /// How many commands its agents run now.
    commands: usize,
    /// The agent that started the session for the user, when one did.
    started_by: Option<SharedString>,
    active_at: u64,
    ago: SharedString,
    selected: bool,
    shell: WeakEntity<Shell>,
}

impl SessionRow {
    pub fn new(id: String, shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        Self {
            id,
            renders: 0,
            title: SharedString::default(),
            mark: Mark::new(cx),
            commands: 0,
            started_by: None,
            active_at: 0,
            ago: SharedString::default(),
            selected: false,
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

    /// The agent that started the session, as the row says it.
    pub fn started_by(&self) -> Option<&str> {
        self.started_by.as_deref()
    }

    pub fn set_started_by(&mut self, by: Option<&str>, cx: &mut Context<Self>) {
        if self.started_by() != by {
            self.started_by = by.map(|by| SharedString::from(by.to_owned()));
            cx.notify();
        }
    }

    pub fn set_commands(&mut self, commands: usize, cx: &mut Context<Self>) {
        if self.commands != commands {
            self.commands = commands;
            cx.notify();
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
        self.title.as_ref() == title && self.mark.shown == mark && self.active_at == active_at
    }

    pub fn set(
        &mut self,
        title: &str,
        mark: Option<AgentState>,
        active_at: u64,
        now: u64,
        cx: &mut Context<Self>,
    ) {
        let mut changed = self.mark.set(mark, cx);
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let shell = self.shell.clone();
        let id = self.id.clone();
        let row = TreeRow::new(session_id(&self.id), TreeLevel::Session, self.title.clone())
            .selected(self.selected)
            .on_click(move |_, window, cx| {
                let session = id.clone();
                shell
                    .update(cx, |shell, cx| {
                        shell.open(Selection::Session(session), window, cx)
                    })
                    .ok();
            });
        let id = session_commands_id(&self.id);
        let last = match self.mark.shown {
            Some(_) => self.mark.icon.clone().into_any_element(),
            None => meta(self.ago.clone(), cx).into_any_element(),
        };
        // A session an agent started says so, small, before its mark or its time ago, and
        // comes in like the rest of the tree's rows.
        let last = match &self.started_by {
            Some(by) => {
                let said = SharedString::from(format!("started by {by}"));
                let text = div()
                    .id(session_started_by_id(&self.id))
                    .flex_none()
                    .max_w(px(120.))
                    .overflow_hidden()
                    .aria_label(said.clone())
                    .child(meta(said, cx))
                    .test_support();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(enter(format!("started-by-enter-{}", self.id), 0, text))
                    .child(last)
                    .into_any_element()
            }
            None => last,
        };
        row.trailing(trailing(id, self.commands, last, cx))
    }
}
