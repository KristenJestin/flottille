//! The quick switcher (Ctrl K, or a click on the top bar's search): sessions, projects with their
//! workspaces, and the window's own actions, found by the words typed. What the window already
//! holds from the daemon is searched (the tree, the projects); the content of conversations is
//! not.
//!
//! This is the data and the logic: what matches, in which order, which result is marked and what
//! choosing it does. The look is `flottille_ui::Modal::search` over `ListRow`s.

use std::collections::HashMap;

use flottille_ui::fuzzy::{self, Match};
use flottille_ui::motion::MODAL;
use flottille_ui::{
    AgentState, EmptyLine, Field, Icon, Item, ListRow, Listing, Modal, StateIcon, Status, Type,
    icon, meta, palette, presence, section_label, space,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window, div, px,
};

use super::{Selection, Shell, now_ms};
use crate::model::{Snapshot, ago};
use crate::project_settings::{field_frame, styled_input};
use crate::settings::{Reveal, SECTIONS};
use crate::tree::place_name;

/// The most results of each group while something is typed, and the recent sessions shown while
/// nothing is.
const SESSIONS: usize = 8;
const PROJECTS: usize = 6;
const RECENT: usize = 6;

/// What the window offers to do, besides opening something. One list: a new screen adds its entry
/// in [`actions`] (and its arm in [`Do::run`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    NewSession,
    AddProject,
    Settings,
    /// Settings, on the tab of that section of [`SECTIONS`].
    SettingsTab(usize),
    /// Settings, opened as a link to that place does.
    SettingsAt(Reveal),
    /// That project's own settings.
    ProjectSettings(String),
    /// The services of the machine.
    Services,
    /// The bell's list: what waits for the user.
    Bell,
}

/// An entry of the list of actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub title: String,
    /// More words that find it (`new session` is also `start`, `create`).
    pub words: &'static str,
    pub icon: Icon,
    pub then: Do,
}

/// Every action the switcher offers, in the order it lists them, for the projects there are.
pub fn actions(projects: &[String]) -> Vec<Action> {
    let mut found = vec![
        Action {
            title: "New session".into(),
            words: "start create agent",
            icon: Icon::Plus,
            then: Do::NewSession,
        },
        Action {
            title: "Add a project".into(),
            words: "register folder repository",
            icon: Icon::Folder,
            then: Do::AddProject,
        },
        Action {
            title: "Notifications".into(),
            words: "bell waiting requests questions",
            icon: Icon::Bell,
            then: Do::Bell,
        },
        Action {
            title: "Settings".into(),
            words: "preferences options",
            icon: Icon::Settings,
            then: Do::Settings,
        },
        Action {
            title: "Services".into(),
            words: "machine database cache server start stop",
            icon: Icon::Service,
            then: Do::Services,
        },
    ];
    found.extend(SECTIONS.iter().enumerate().map(|(at, (_, label))| Action {
        title: format!("Settings: {label}"),
        words: "preferences options tab",
        icon: Icon::Settings,
        then: Do::SettingsTab(at),
    }));
    // The judge's guidance is no tab: it is the second block of the Judge tab.
    found.push(Action {
        title: "Settings: Judge guidance".into(),
        words: "preferences options auto judge",
        icon: Icon::Settings,
        then: Do::SettingsAt(Reveal::Guidance),
    });
    found.extend(projects.iter().map(|project| Action {
        title: format!("Settings of {project}"),
        words: "project preferences options",
        icon: Icon::Settings,
        then: Do::ProjectSettings(project.clone()),
    }));
    found
}

impl Do {
    fn run(&self, shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
        match self {
            Do::NewSession => shell.new_session(window, cx),
            Do::AddProject => shell.add_project(window, cx),
            Do::Settings => shell.open(Selection::Settings, window, cx),
            Do::Services => shell.open(Selection::Services, window, cx),
            Do::SettingsTab(tab) => shell.open_settings_tab(*tab, window, cx),
            Do::SettingsAt(at) => shell.open_settings_at(*at, window, cx),
            Do::ProjectSettings(project) => {
                shell.open(Selection::ProjectSettings(project.clone()), window, cx)
            }
            Do::Bell => {
                if !shell.bell_open() {
                    shell.toggle_bell(cx);
                }
            }
        }
    }
}

/// What can be found, with what the window knows of it. Built from the daemon's last read.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Session {
        id: String,
        title: String,
        project: String,
        place: String,
        mark: Option<AgentState>,
        active_at: u64,
    },
    Project {
        name: String,
    },
    Workspace {
        project: String,
        name: String,
    },
}

impl Target {
    fn key(&self) -> String {
        match self {
            Target::Session { id, .. } => session_key(id),
            Target::Project { name } => project_key(name),
            Target::Workspace { project, name } => workspace_key(project, name),
        }
    }
}

fn corpus(snapshot: &Snapshot) -> Vec<Target> {
    let mut found: Vec<Target> = snapshot
        .sessions
        .iter()
        .map(|session| Target::Session {
            id: session.id.clone(),
            title: session.title.clone(),
            project: session.project.clone(),
            place: place_name(&session.place).to_owned(),
            mark: session.mark,
            active_at: session.active_at,
        })
        .collect();
    for project in &snapshot.projects {
        found.push(Target::Project {
            name: project.name.clone(),
        });
        found.extend(project.workspaces.iter().map(|name| Target::Workspace {
            project: project.name.clone(),
            name: name.clone(),
        }));
    }
    found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Sessions,
    Projects,
    Actions,
}

impl Group {
    fn heading(self) -> &'static str {
        match self {
            Group::Sessions => "Sessions",
            Group::Projects => "Projects",
            Group::Actions => "Actions",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Group::Sessions => "sessions",
            Group::Projects => "projects",
            Group::Actions => "actions",
        }
    }
}

/// What choosing a result does.
#[derive(Clone, Debug, PartialEq)]
enum Then {
    Open(Selection),
    Do(Do),
}

/// One result, as it is drawn.
#[derive(Clone, Debug, PartialEq)]
struct Found {
    key: String,
    score: u32,
    title: String,
    pieces: Vec<(String, bool)>,
    line: String,
    icon: Icon,
    /// A session's: its state when it has one, how long ago it was active.
    session: Option<(Option<AgentState>, String)>,
    then: Then,
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Heading(Group),
    Result(Found),
}

impl Entry {
    fn key(&self) -> String {
        match self {
            Entry::Heading(group) => format!("heading-{}", group.key()),
            Entry::Result(found) => found.key.clone(),
        }
    }
}

/// The element id of the dialog and of its text field.
pub const SWITCHER: &str = "switcher";
pub const SWITCHER_FIELD: &str = "switcher-field";

/// The element id of a result's row.
pub fn row_id(key: &str) -> ElementId {
    ElementId::Name(format!("switcher-row-{key}").into())
}

/// The key of the session result `session`, of a project, of a workspace, of an action.
pub fn session_key(session: &str) -> String {
    format!("session-{session}")
}

pub fn project_key(project: &str) -> String {
    format!("project-{project}")
}

/// A project's name holds no `/` (`flottille_core::project::valid_name`): the first one ends it.
pub fn workspace_key(project: &str, workspace: &str) -> String {
    format!("workspace-{project}/{workspace}")
}

pub fn action_key(title: &str) -> String {
    format!("action-{}", title.to_lowercase().replace(' ', "-"))
}

/// A session's state mark, held as the tree holds its own: the icon swaps from one state to
/// another, and folds in and away as it comes and goes.
struct Mark {
    status: Entity<Status>,
    icon: Entity<StateIcon>,
}

pub struct Switcher {
    shell: WeakEntity<Shell>,
    /// The window's own focus, which gets it back when nothing had it.
    window_focus: FocusHandle,
    open: bool,
    field: Entity<InputState>,
    query: String,
    corpus: Vec<Target>,
    actions: Vec<Action>,
    entries: Vec<Entry>,
    list: Listing<Entry>,
    /// The result Enter chooses.
    marked: Option<String>,
    marks: HashMap<String, Mark>,
    /// Lets the marks go once it has faded out: a running one ticks while it lives.
    forget: Option<Task<()>>,
    scroll: ScrollHandle,
    /// The result to bring into sight, found among the drawn rows once the next frame has them.
    scroll_to: Option<String>,
    /// What had the focus when it opened, which gets it back.
    before: Option<FocusHandle>,
    _subscription: Subscription,
}

impl Switcher {
    pub fn new(
        shell: WeakEntity<Shell>,
        window_focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field =
            cx.new(|cx| styled_input("a session's title, a project, an action…", window, cx));
        let subscription =
            cx.subscribe_in(
                &field,
                window,
                |this, field, event, window, cx| match event {
                    InputEvent::Change => {
                        this.query = field.read(cx).value().to_string();
                        this.search(cx);
                    }
                    InputEvent::PressEnter { .. } => this.choose_marked(window, cx),
                    _ => {}
                },
            );
        Self {
            shell,
            window_focus,
            open: false,
            field,
            query: String::new(),
            corpus: Vec::new(),
            actions: actions(&[]),
            entries: Vec::new(),
            list: Listing::default(),
            marked: None,
            marks: HashMap::new(),
            forget: None,
            scroll: ScrollHandle::new(),
            scroll_to: None,
            before: None,
            _subscription: subscription,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The text field.
    pub fn field(&self) -> &Entity<InputState> {
        &self.field
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// The results as they are listed: each group's heading, then its results' titles.
    pub fn said(&self) -> Vec<(String, Vec<String>)> {
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for entry in &self.entries {
            match entry {
                Entry::Heading(group) => groups.push((group.heading().to_owned(), Vec::new())),
                Entry::Result(found) => {
                    if let Some((_, titles)) = groups.last_mut() {
                        titles.push(found.title.clone());
                    }
                }
            }
        }
        groups
    }

    /// The results' scroll.
    pub fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// The key of the marked result.
    pub fn marked(&self) -> Option<&str> {
        self.marked.as_deref()
    }

    /// The title of the marked result.
    pub fn marked_title(&self) -> Option<&str> {
        self.results()
            .find(|found| Some(found.key.as_str()) == self.marked.as_deref())
            .map(|found| found.title.as_str())
    }

    fn results(&self) -> impl Iterator<Item = &Found> {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Result(found) => Some(found),
            Entry::Heading(_) => None,
        })
    }

    /// The daemon's last read, when it changed what can be found. While it is open the results
    /// keep their places, under the pointer and the mark: only what they say is read again.
    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let projects: Vec<String> = snapshot.projects.iter().map(|p| p.name.clone()).collect();
        self.actions = actions(&projects);
        let corpus = corpus(snapshot);
        if self.corpus != corpus {
            self.corpus = corpus;
            if self.open {
                self.refresh(cx);
            }
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.open = true;
        self.forget = None;
        self.before = window.focused(cx);
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.query.clear();
        self.search(cx);
        let focus = self.field.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes it and gives the focus back to what had it.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        // What had the focus may have left the window while this was open (a composer whose
        // session is gone): a handle nothing draws takes the keys from the window.
        let back = self
            .before
            .take()
            .filter(|before| self.window_focus.contains(before, window))
            .unwrap_or_else(|| self.window_focus.clone());
        window.focus(&back, cx);
        self.forget = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MODAL).await;
            this.update(cx, |this, _| this.marks.clear()).ok();
        }));
        cx.notify();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
        } else {
            self.open(window, cx);
        }
    }

    /// Marks that result (the pointer is on it).
    pub fn mark(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.marked.as_deref() != Some(key) && self.results().any(|found| found.key == key) {
            self.marked = Some(key.to_owned());
            cx.notify();
        }
    }

    /// The arrows: the mark moves by `by` results, and stops at the first and the last.
    pub fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let keys: Vec<&str> = self.results().map(|found| found.key.as_str()).collect();
        if keys.is_empty() {
            return;
        }
        let at = self
            .marked
            .as_deref()
            .and_then(|key| keys.iter().position(|k| *k == key))
            .map_or(0, |at| {
                (at as isize + by).clamp(0, keys.len() as isize - 1) as usize
            });
        let key = keys[at].to_owned();
        self.scroll_to(&key);
        self.marked = Some(key);
        cx.notify();
    }

    /// Asks for that result to be in sight: the rows drawn are not the results (the ones that
    /// left still fold away among them), so the place is found in `render`.
    fn scroll_to(&mut self, key: &str) {
        self.scroll_to = Some(key.to_owned());
    }

    /// Scrolls to the result among the rows as they are drawn.
    fn bring_into_sight(&self, key: &str) {
        let rows: Vec<_> = self.list.rows().collect();
        let Some(at) = rows.iter().position(|row| row.key == key) else {
            return;
        };
        // The group's heading above the first result of it stays in sight too, past the results
        // of the group that are folding away.
        let above = rows[..at]
            .iter()
            .rposition(|row| !row.placement.is_gone())
            .filter(|above| matches!(rows[*above].value, Entry::Heading(_)));
        self.scroll.scroll_to_item(above.unwrap_or(at));
    }

    /// Enter: chooses the marked result.
    pub fn choose_marked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.marked.clone() {
            self.choose(&key, window, cx);
        }
    }

    /// Closes it and does what that result is: opens it or does it.
    pub fn choose(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(then) = self
            .results()
            .find(|found| found.key == key)
            .map(|found| found.then.clone())
        else {
            return;
        };
        self.close(window, cx);
        // What is chosen has the focus (its page), not what had it before.
        self.shell
            .update(cx, |shell, cx| match then {
                Then::Open(selection) => shell.open(selection, window, cx),
                Then::Do(action) => action.run(shell, window, cx),
            })
            .ok();
    }

    /// Looks for what is typed again, and marks the best match, whatever its group.
    fn search(&mut self, cx: &mut Context<Self>) {
        self.entries = self.find();
        let mut best: Option<&Found> = None;
        for found in self.results() {
            if best.is_none_or(|best| found.score > best.score) {
                best = Some(found);
            }
        }
        self.marked = best.map(|found| found.key.clone());
        if let Some(key) = self.marked.clone() {
            self.scroll_to(&key);
        }
        self.keep_marks(cx);
        cx.notify();
    }

    /// The results as they are, each read again from the daemon's last read: a session's state
    /// and how long ago it was active; one that is gone leaves, and its group's heading with it
    /// when it was the last. Nothing comes in and nothing moves until something is typed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        let known: HashMap<String, &Target> = self
            .corpus
            .iter()
            .map(|target| (target.key(), target))
            .collect();
        let projects: Vec<Do> = self.actions.iter().map(|a| a.then.clone()).collect();
        let entries = std::mem::take(&mut self.entries);
        let mut kept: Vec<Entry> = Vec::new();
        for entry in entries {
            match entry {
                Entry::Heading(group) => kept.push(Entry::Heading(group)),
                Entry::Result(mut found) => {
                    let alive = match (&found.then, known.get(&found.key)) {
                        (Then::Do(action), _) => projects.contains(action),
                        (
                            _,
                            Some(Target::Session {
                                mark, active_at, ..
                            }),
                        ) => {
                            found.session = Some((*mark, ago(now, *active_at)));
                            true
                        }
                        (_, Some(_)) => true,
                        (_, None) => false,
                    };
                    if alive {
                        kept.push(Entry::Result(found));
                    }
                }
            }
        }
        // A heading stays only over a result of its own.
        let mut entries = Vec::with_capacity(kept.len());
        for (at, entry) in kept.iter().enumerate() {
            let alone = matches!(entry, Entry::Heading(_))
                && !matches!(kept.get(at + 1), Some(Entry::Result(_)));
            if !alone {
                entries.push(entry.clone());
            }
        }
        self.entries = entries;
        let still = self
            .marked
            .as_deref()
            .is_some_and(|key| self.results().any(|found| found.key == key));
        if !still {
            let first = self.results().next().map(|found| found.key.clone());
            self.marked = first;
        }
        self.keep_marks(cx);
        cx.notify();
    }

    /// A state mark for each session result that has a state, kept from one read to the next.
    fn keep_marks(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(String, AgentState)> = self
            .results()
            .filter_map(|found| match found.session {
                Some((Some(state), _)) => Some((found.key.clone(), state)),
                _ => None,
            })
            .collect();
        for (key, state) in wanted {
            match self.marks.get(&key) {
                Some(mark) => {
                    // GPUI Fast takes any update of an entity for a change: touch it only when
                    // it is one.
                    if mark.status.read(cx).state() != state {
                        mark.status.update(cx, |status, cx| status.set(state, cx));
                    }
                }
                None => {
                    let status = cx.new(|_| Status::new(state));
                    let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
                    self.marks.insert(key, Mark { status, icon });
                }
            }
        }
    }

    fn find(&self) -> Vec<Entry> {
        let now = now_ms();
        let query = self.query.trim();
        // Scored first, the best kept, and only those built.
        let mut sessions: Vec<(u32, u64, &Target, Match)> = Vec::new();
        let mut projects: Vec<(u32, u64, &Target, Match)> = Vec::new();
        for target in &self.corpus {
            let found = match target {
                Target::Session {
                    title,
                    project,
                    place,
                    active_at,
                    ..
                } => fuzzy::find(query, title, &format!("{project} {place}"))
                    .map(|found| (&mut sessions, *active_at, found)),
                Target::Project { name } if !query.is_empty() => {
                    fuzzy::find(query, name, "").map(|found| (&mut projects, 0, found))
                }
                Target::Workspace { project, name } if !query.is_empty() => {
                    fuzzy::find(query, name, project).map(|found| (&mut projects, 0, found))
                }
                _ => None,
            };
            if let Some((group, active_at, found)) = found {
                group.push((found.score, active_at, target, found));
            }
        }
        // The better match first, then the one that was active last.
        let best = |mut list: Vec<(u32, u64, &Target, Match)>, most: usize| {
            list.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            list.truncate(most);
            list.into_iter()
                .map(|(_, _, target, found)| built(target, &found, now))
                .collect::<Vec<Found>>()
        };
        let sessions = best(sessions, if query.is_empty() { RECENT } else { SESSIONS });
        let projects = best(projects, PROJECTS);
        let mut actions: Vec<Found> = self
            .actions
            .iter()
            .filter_map(|action| {
                let found = fuzzy::find(query, &action.title, action.words)?;
                Some(Found {
                    key: action_key(&action.title),
                    score: found.score,
                    title: action.title.clone(),
                    pieces: found.pieces(&action.title),
                    line: String::new(),
                    icon: action.icon,
                    session: None,
                    then: Then::Do(action.then.clone()),
                })
            })
            .collect();
        // Actions keep the order they are listed in among equal matches.
        actions.sort_by_key(|found| std::cmp::Reverse(found.score));

        let mut entries = Vec::new();
        for (group, found) in [
            (Group::Sessions, sessions),
            (Group::Projects, projects),
            (Group::Actions, actions),
        ] {
            if found.is_empty() {
                continue;
            }
            entries.push(Entry::Heading(group));
            entries.extend(found.into_iter().map(Entry::Result));
        }
        entries
    }
}

/// The result of a session, a project or a workspace that matched.
fn built(target: &Target, found: &Match, now: u64) -> Found {
    let (title, line, icon, session, then) = match target {
        Target::Session {
            id,
            title,
            project,
            place,
            mark,
            active_at,
        } => (
            title,
            format!("{project} · {place}"),
            Icon::Agents,
            Some((*mark, ago(now, *active_at))),
            Then::Open(Selection::Session(id.clone())),
        ),
        Target::Project { name } => (
            name,
            "project".to_owned(),
            Icon::Folder,
            None,
            Then::Open(Selection::Project(name.clone())),
        ),
        Target::Workspace { project, name } => (
            name,
            format!("workspace of {project}"),
            Icon::Branch,
            None,
            Then::Open(Selection::Project(project.clone())),
        ),
    };
    Found {
        key: target.key(),
        score: found.score,
        title: title.clone(),
        pieces: found.pieces(title),
        line,
        icon,
        session,
        then,
    }
}

impl Render for Switcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let at = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        // Closed, it keeps the results it showed while it fades out.
        if self.open {
            let items = self
                .entries
                .iter()
                .map(|entry| (entry.key(), entry.clone()));
            self.list.sync(items, at, reduce);
            if let Some(key) = self.scroll_to.take() {
                self.bring_into_sight(&key);
            }
        }
        let this = cx.entity().downgrade();
        let marked = self.marked.clone();
        let rows: Vec<_> = self
            .list
            .rows()
            .map(|row| {
                let id = ElementId::Name(format!("switcher-entry-{}", row.key).into());
                let content = match row.value {
                    Entry::Heading(group) => div()
                        .id(ElementId::Name(
                            format!("switcher-heading-{}", group.key()).into(),
                        ))
                        .aria_label(group.heading())
                        .child(section_label(group.heading(), cx))
                        .test_support()
                        .into_any_element(),
                    Entry::Result(found) => {
                        let key = found.key.clone();
                        let (on_click, on_enter) = (this.clone(), this.clone());
                        let enter_key = key.clone();
                        // As the tree says it: the state when there is one, else how long ago,
                        // one folding away as the other comes.
                        let trailing = found.session.as_ref().map(|(state, ago)| {
                            let mark = self.marks.get(&key).map(|mark| mark.icon.clone());
                            div()
                                .flex()
                                .items_center()
                                .child(presence(
                                    ElementId::Name(format!("switcher-mark-{key}").into()),
                                    state.is_some() && mark.is_some(),
                                    div().flex().items_center().children(mark),
                                ))
                                .child(presence(
                                    ElementId::Name(format!("switcher-ago-{key}").into()),
                                    state.is_none(),
                                    meta(SharedString::from(ago.clone()), cx),
                                ))
                        });
                        ListRow::new(row_id(&key), found.title.clone())
                            .pieces(found.pieces.clone())
                            .leading(icon(found.icon).size(px(14.)).text_color(p.mute))
                            .when_some(
                                (!found.line.is_empty()).then(|| found.line.clone()),
                                |row, line| row.line(line),
                            )
                            .when_some(trailing, |row, trailing| row.trailing(trailing))
                            .marked(marked.as_deref() == Some(found.key.as_str()))
                            .on_click(move |_, window, cx| {
                                on_click
                                    .update(cx, |this, cx| this.choose(&key, window, cx))
                                    .ok();
                            })
                            .on_enter(move |cx| {
                                on_enter
                                    .update(cx, |this, cx| this.mark(&enter_key, cx))
                                    .ok();
                            })
                            .into_any_element()
                    }
                };
                Item::new(id, row.placement, content).into_any_element()
            })
            .collect();
        let nothing = self.open && self.entries.is_empty();
        let close_with = this.clone();
        let key_with = this.clone();
        Modal::new(SWITCHER, "Go to a session, a project or an action")
            .search()
            .open(self.open)
            .on_close(move |window, cx| {
                close_with
                    .update(cx, |this, cx| this.close(window, cx))
                    .ok();
            })
            .on_key(move |event: &KeyDownEvent, _, cx| {
                let by = match event.keystroke.key.as_str() {
                    "down" => 1,
                    "up" => -1,
                    _ => return,
                };
                cx.stop_propagation();
                key_with.update(cx, |this, cx| this.step(by, cx)).ok();
            })
            .scroll(&self.scroll)
            .above(Field::new(
                SWITCHER_FIELD,
                "Go to a session, a project or an action",
                field_frame("switcher-input", &self.field, cx),
            ))
            .children(rows)
            .child(presence(
                "switcher-nothing",
                nothing,
                EmptyLine::new(
                    "switcher-none",
                    Icon::Search,
                    "Nothing matches what is typed.",
                ),
            ))
            .kept(
                div()
                    .id("switcher-keys")
                    .flex_none()
                    .px(space::MD)
                    .py(space::SM)
                    .border_t_1()
                    .border_color(p.line)
                    .mono(11.)
                    .text_color(p.faint)
                    .child("↑↓ move · Enter choose · Esc close"),
            )
    }
}
