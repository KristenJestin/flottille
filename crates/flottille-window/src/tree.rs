//! The left tree: "New session", then project › place › session. Every row is its own view, so
//! one session changing redraws its row and nothing else; a folded project or place shows the
//! strongest mark of what it holds.

use std::collections::HashSet;

use flottille_ui::motion::PANEL;
use flottille_ui::rows::TreeLevel;
use flottille_ui::{
    AgentState, Button, Fold, Icon, Item, Listing, Menu, Modal, StateIcon, Status, TreeRow, fade,
    meta, palette, presence, presence_of, section_label, space, tree_children,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    TestSupportExt as _, WeakEntity, Window, div, px, relative,
};

use crate::actions::Command;
use crate::composer::Choice;
use crate::model::{Place, ago};
use crate::session::end_sentence;
use crate::shell::{Selection, Shell};

/// The element id of a project's row.
pub fn project_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-{name}").into())
}

/// The element id of a place's row: `place-<project>-main` or `place-<project>-<workspace>`.
pub fn place_id(project: &str, place: &Place) -> ElementId {
    ElementId::Name(format!("place-{project}-{}", place_name(place)).into())
}

/// The element id of the "Ended" group, at the bottom of the tree.
pub fn ended_id() -> ElementId {
    ElementId::Name("ended".into())
}

/// The element id of the "+" a place's row offers, which starts a session there.
pub fn place_new_id(project: &str, place: &Place) -> ElementId {
    ElementId::Name(format!("place-new-{project}-{}", place_name(place)).into())
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

/// The element id of the mark a session's row shows while a ticket or a pull request it changed
/// waits to be seen; its label says what changed.
pub fn session_changed_id(id: &str) -> ElementId {
    ElementId::Name(format!("changed-session-{id}").into())
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
    ended: Option<(Entity<PlaceNode>, bool)>,
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
    pub fn set_ended(&mut self, group: Entity<PlaceNode>, present: bool, cx: &mut Context<Self>) {
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
        let ended = self.ended.clone().map(|(group, present)| {
            // At the panel's bottom, under a line; it takes half the panel at most, and folds
            // away when its last session leaves.
            presence(
                "sidebar-ended",
                present,
                div()
                    .id("ended-footer")
                    .flex_none()
                    .max_h(relative(0.5))
                    .overflow_y_scroll()
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

/// A project's row and, folding under it, its places.
pub struct ProjectNode {
    name: String,
    open: bool,
    mark: Mark,
    /// The strongest mark of what it holds, which it shows only while it is folded: open, the
    /// work is on its own row.
    held: Option<AgentState>,
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
            held: None,
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

    /// Whether it holds these places and this mark already.
    pub fn holds(&self, places: &[Entity<PlaceNode>], mark: Option<AgentState>, cx: &App) -> bool {
        self.held == mark && self.places.read(cx).rows == places
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
        self.held = mark;
        self.show_mark(cx);
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
/// under it, its sessions; or, at the bottom of the tree, the "Ended" group that gathers the ended
/// sessions of every project, folded until the user opens it.
pub struct PlaceNode {
    project: String,
    place: Place,
    /// The "Ended" group: no project and no place of its own.
    ended: bool,
    open: bool,
    mark: Mark,
    /// The strongest mark of its sessions, which it shows only while it is folded.
    held: Option<AgentState>,
    sessions: Entity<Level<SessionRow>>,
    fold: Entity<Fold>,
    shell: WeakEntity<Shell>,
}

impl PlaceNode {
    pub fn new(
        project: String,
        place: Place,
        shell: WeakEntity<Shell>,
        cx: &mut Context<Self>,
    ) -> Self {
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
            held: None,
            sessions,
            fold,
            shell,
        }
    }

    /// The "Ended" group, folded.
    pub fn ended(shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        let sessions = cx.new(|_| Level::new());
        let fold = cx.new(|_| Fold::new("fold-ended", sessions.clone(), false));
        Self {
            project: String::new(),
            place: None,
            ended: true,
            open: false,
            mark: Mark::new(cx),
            held: None,
            sessions,
            fold,
            shell,
        }
    }

    /// The mark it shows: that of its sessions, while it is folded.
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
            // The "Ended" row counts them.
            if self.ended {
                cx.notify();
            }
        }
        self.held = mark;
        self.show_mark(cx);
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

impl Render for PlaceNode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = match (self.ended, &self.place) {
            (true, _) => {
                let count = self.sessions.read(cx).rows.len();
                TreeRow::new(ended_id(), TreeLevel::Session, "Ended")
                    .unfolds_upward()
                    .trailing(meta(count.to_string(), cx))
            }
            (false, place) => {
                let (shell, project, choice) = (
                    self.shell.clone(),
                    self.project.clone(),
                    place.clone().map_or(Choice::Main, Choice::Workspace),
                );
                TreeRow::new(
                    place_id(&self.project, place),
                    match place {
                        None => TreeLevel::Main,
                        Some(_) => TreeLevel::Workspace,
                    },
                    SharedString::from(place_name(place).to_owned()),
                )
                .trailing(
                    self.mark
                        .icon(&format!("place-{}-{}", self.project, place_name(place))),
                )
                .actions(
                    Button::icon(
                        place_new_id(&self.project, place),
                        Icon::Plus,
                        "New session here",
                    )
                    .on_click(move |_, window, cx| {
                        let (project, choice) = (project.clone(), choice.clone());
                        shell
                            .update(cx, |shell, cx| shell.start_in(&project, choice, window, cx))
                            .ok();
                    }),
                    false,
                )
            }
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
    /// What its agents did to a ticket or a pull request since it was last opened, the last of it
    /// in words: a small mark on the row says it until the session is opened.
    changed: Option<SharedString>,
    /// The words of the mark, kept while it folds away.
    said_changed: SharedString,
    active_at: u64,
    ago: SharedString,
    selected: bool,
    /// The project it belongs to, said in the "Ended" group where projects are mixed.
    scope: Option<SharedString>,
    /// What its "⋯" menu offers: End session while it lives, Resume when it ended and can.
    actions: Actions,
    menu: Option<(Entity<Menu>, Subscription)>,
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
            changed: None,
            said_changed: SharedString::default(),
            active_at: 0,
            ago: SharedString::default(),
            selected: false,
            scope: None,
            actions: Actions::None,
            menu: None,
            shell,
        }
    }

    /// Says its project in its meta (the "Ended" group), or stops.
    pub fn set_scope(&mut self, scope: Option<&str>, cx: &mut Context<Self>) {
        if self.scope.as_deref() != scope {
            self.scope = scope.map(|scope| SharedString::from(scope.to_owned()));
            cx.notify();
        }
    }

    /// What the row's menu offers: End session for a session that lives, Resume for an ended one
    /// whose agent can resume, nothing else.
    pub fn set_actions(&mut self, ended: bool, resumable: bool, cx: &mut Context<Self>) {
        let actions = match (ended, resumable) {
            (false, _) => Actions::End,
            (true, true) => Actions::Resume,
            (true, false) => Actions::None,
        };
        if self.actions == actions {
            return;
        }
        self.actions = actions;
        self.menu = match actions {
            Actions::None => None,
            offered => {
                let (shell, id) = (self.shell.clone(), self.id.clone());
                let menu = cx.new(|cx| {
                    let menu = Menu::new(session_menu_id(&id), "Session actions", cx);
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

    /// What its agents changed of a ticket or a pull request since it was last opened, as its
    /// mark says it.
    pub fn changed(&self) -> Option<&str> {
        self.changed.as_deref()
    }

    pub fn set_changed(&mut self, changed: Option<&str>, cx: &mut Context<Self>) {
        if self.changed() != changed {
            self.changed = changed.map(|said| SharedString::from(said.to_owned()));
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            })
            .when_some(self.menu.as_ref(), |row, (menu, _)| {
                row.actions(menu.clone(), menu.read(cx).is_open())
            });
        let when = match &self.scope {
            Some(scope) => SharedString::from(format!("{scope} · {}", self.ago)),
            None => self.ago.clone(),
        };
        let id = session_commands_id(&self.id);
        // A ticket or a pull request changed: a dot that widens in and fades, and folds away
        // once the session is opened. It keeps the last words it said while it does.
        if let Some(said) = &self.changed {
            self.said_changed = said.clone();
        }
        let shown = fade(
            ElementId::Name(format!("session-changed-fade-{}", self.id).into()),
            self.changed.is_some(),
            PANEL,
            window,
            cx,
        );
        let p = palette(cx);
        let changed = (shown > 0.0).then(|| {
            div()
                .id(session_changed_id(&self.id))
                .flex_none()
                .w(px(14. * shown))
                .overflow_hidden()
                .opacity(shown)
                .aria_label(self.said_changed.clone())
                .child(div().size(px(6.)).rounded_full().bg(p.acc))
                .test_support()
        });
        // The mark and the time take each other's place: one folds away as the other comes.
        let last = div()
            .flex()
            .items_center()
            .children(changed)
            .child(self.mark.icon(&self.id))
            .child(presence(
                ElementId::Name(format!("meta-{}", self.id).into()),
                self.mark.shown.is_none(),
                meta(when, cx),
            ))
            .into_any_element();
        row.trailing(trailing(id, self.commands, last, cx))
    }
}
