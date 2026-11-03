//! The quick switcher (Ctrl K, or a click on the top bar's search): sessions, projects with their
//! workspaces, and the window's own actions, found by the words typed. What the window already
//! holds from the daemon is searched (the tree, the projects); the content of conversations is
//! not.
//!
//! This is the data and the logic: what matches, in which order, which result is marked and what
//! choosing it does. The look is `flottille_ui::QuickPanel` and `QuickRow`.

use flottille_ui::fuzzy;
use flottille_ui::{
    AgentState, EmptyLine, Field, Icon, Item, Listing, QuickPanel, QuickRow, Type, palette,
    presence, section_label,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _, WeakEntity, Window, div,
    px,
};

use super::{Selection, Shell, now_ms};
use crate::model::{Snapshot, ago};
use crate::project_settings::{field_frame, styled_input};
use crate::settings::SECTIONS;
use crate::tree::place_name;

/// The most results of each group while something is typed, and the recent sessions shown while
/// nothing is.
const SESSIONS: usize = 8;
const PROJECTS: usize = 6;
const RECENT: usize = 6;

/// What the window offers to do, besides opening something. One list: a new screen adds its entry
/// here (and its arm in [`Do::run`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    NewSession,
    AddProject,
    Settings,
    /// Settings, on the tab of that section of [`SECTIONS`].
    SettingsTab(usize),
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

/// Every action the switcher offers, in the order it lists them.
pub fn actions() -> Vec<Action> {
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
            title: "Settings".into(),
            words: "preferences options",
            icon: Icon::Settings,
            then: Do::Settings,
        },
    ];
    found.extend(SECTIONS.iter().enumerate().map(|(at, (_, label))| Action {
        title: format!("Settings: {label}"),
        words: "preferences options tab",
        icon: Icon::Settings,
        then: Do::SettingsTab(at),
    }));
    found
}

impl Do {
    fn run(&self, shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
        match self {
            Do::NewSession => shell.new_session(window, cx),
            Do::AddProject => shell.add_project(window, cx),
            Do::Settings => shell.open(Selection::Settings, window, cx),
            Do::SettingsTab(tab) => shell.open_settings_tab(*tab, window, cx),
        }
    }
}

/// What can be found, with what the window knows of it. Built from the daemon's last read.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Session {
        id: String,
        title: String,
        context: String,
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

fn corpus(snapshot: &Snapshot) -> Vec<Target> {
    let mut found: Vec<Target> = snapshot
        .sessions
        .iter()
        .map(|session| Target::Session {
            id: session.id.clone(),
            title: session.title.clone(),
            context: format!("{} {}", session.project, place_name(&session.place)),
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
    title: String,
    pieces: Vec<(String, bool)>,
    line: String,
    icon: Icon,
    /// Its state, or how long ago when it has none.
    state: Option<AgentState>,
    ago: String,
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

/// The element id of the panel, of its text field, of a result's row.
pub const SWITCHER: &str = "switcher";
pub const SWITCHER_FIELD: &str = "switcher-field";

pub fn row_id(key: &str) -> ElementId {
    ElementId::Name(format!("switcher-row-{key}").into())
}

/// The element id of the session result `session`, of a project, of a workspace, of an action.
pub fn session_key(session: &str) -> String {
    format!("session-{session}")
}

pub fn project_key(project: &str) -> String {
    format!("project-{project}")
}

pub fn workspace_key(project: &str, workspace: &str) -> String {
    format!("workspace-{project}-{workspace}")
}

pub fn action_key(title: &str) -> String {
    format!("action-{}", title.to_lowercase().replace(' ', "-"))
}

pub struct Switcher {
    shell: WeakEntity<Shell>,
    open: bool,
    field: Entity<InputState>,
    query: String,
    corpus: Vec<Target>,
    actions: Vec<Action>,
    entries: Vec<Entry>,
    list: Listing<Entry>,
    /// The result Enter chooses.
    marked: Option<String>,
    scroll: ScrollHandle,
    /// What had the focus when it opened, which gets it back.
    before: Option<FocusHandle>,
    _subscription: Subscription,
}

impl Switcher {
    pub fn new(shell: WeakEntity<Shell>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field =
            cx.new(|cx| styled_input("a session's title, a project, an action…", window, cx));
        let subscription =
            cx.subscribe_in(
                &field,
                window,
                |this, field, event, window, cx| match event {
                    InputEvent::Change => {
                        this.query = field.read(cx).value().to_string();
                        this.search(true, cx);
                    }
                    InputEvent::PressEnter { .. } => this.choose_marked(window, cx),
                    _ => {}
                },
            );
        Self {
            shell,
            open: false,
            field,
            query: String::new(),
            corpus: Vec::new(),
            actions: actions(),
            entries: Vec::new(),
            list: Listing::default(),
            marked: None,
            scroll: ScrollHandle::new(),
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

    /// The daemon's last read, when it changed what can be found.
    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let corpus = corpus(snapshot);
        if self.corpus != corpus {
            self.corpus = corpus;
            if self.open {
                self.search(false, cx);
            }
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.open = true;
        self.before = window.focused(cx);
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.query.clear();
        self.search(true, cx);
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
        let back = self.before.take();
        let shell = self.shell.clone();
        let back = back.or_else(|| shell.read_with(cx, |shell, _| shell.focus_handle()).ok());
        if let Some(back) = back {
            window.focus(&back, cx);
        }
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

    fn scroll_to(&self, key: &str) {
        let at = self.entries.iter().position(|entry| entry.key() == key);
        if let Some(at) = at {
            // The group's heading above the first result of it stays in sight too.
            let heading = matches!(
                at.checked_sub(1).map(|i| &self.entries[i]),
                Some(Entry::Heading(_))
            );
            self.scroll
                .scroll_to_item(if heading { at - 1 } else { at });
        }
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

    /// Looks for what is typed again. `top` puts the mark on the first result, as typing does; a
    /// read of the daemon keeps it on its result when that is still there.
    fn search(&mut self, top: bool, cx: &mut Context<Self>) {
        self.entries = self.find();
        let still = self
            .marked
            .as_deref()
            .is_some_and(|key| self.results().any(|found| found.key == key));
        if top || !still {
            let first = self.results().next().map(|found| found.key.clone());
            self.marked = first;
            self.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
        }
        cx.notify();
    }

    fn find(&self) -> Vec<Entry> {
        let now = now_ms();
        let query = self.query.trim();
        let mut sessions: Vec<(u32, u64, Found)> = Vec::new();
        let mut projects: Vec<(u32, u64, Found)> = Vec::new();
        for target in &self.corpus {
            match target {
                Target::Session {
                    id,
                    title,
                    context,
                    mark,
                    active_at,
                } => {
                    let Some(found) = fuzzy::find(query, title, context) else {
                        continue;
                    };
                    let place = context.split_once(' ').map_or_else(
                        || context.clone(),
                        |(project, place)| format!("{project} · {place}"),
                    );
                    sessions.push((
                        found.score,
                        *active_at,
                        Found {
                            key: session_key(id),
                            title: title.clone(),
                            pieces: found.pieces(title),
                            line: place,
                            icon: Icon::Agents,
                            state: *mark,
                            ago: ago(now, *active_at),
                            then: Then::Open(Selection::Session(id.clone())),
                        },
                    ));
                }
                Target::Project { name } if !query.is_empty() => {
                    if let Some(found) = fuzzy::find(query, name, "") {
                        projects.push((
                            found.score,
                            0,
                            Found {
                                key: project_key(name),
                                title: name.clone(),
                                pieces: found.pieces(name),
                                line: "project".into(),
                                icon: Icon::Folder,
                                state: None,
                                ago: String::new(),
                                then: Then::Open(Selection::Project(name.clone())),
                            },
                        ));
                    }
                }
                Target::Workspace { project, name } if !query.is_empty() => {
                    if let Some(found) = fuzzy::find(query, name, project) {
                        projects.push((
                            found.score,
                            0,
                            Found {
                                key: workspace_key(project, name),
                                title: name.clone(),
                                pieces: found.pieces(name),
                                line: format!("workspace of {project}"),
                                icon: Icon::Branch,
                                state: None,
                                ago: String::new(),
                                then: Then::Open(Selection::Project(project.clone())),
                            },
                        ));
                    }
                }
                _ => {}
            }
        }
        // The better match first, then the one that was active last.
        let best = |list: &mut Vec<(u32, u64, Found)>, most: usize| -> Vec<Found> {
            list.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            list.drain(..)
                .take(most)
                .map(|(_, _, found)| found)
                .collect()
        };
        let sessions = best(
            &mut sessions,
            if query.is_empty() { RECENT } else { SESSIONS },
        );
        let projects = best(&mut projects, PROJECTS);
        let mut actions: Vec<(u32, u64, Found)> = self
            .actions
            .iter()
            .filter_map(|action| {
                let found = fuzzy::find(query, &action.title, action.words)?;
                Some((
                    found.score,
                    0,
                    Found {
                        key: action_key(&action.title),
                        title: action.title.clone(),
                        pieces: found.pieces(&action.title),
                        line: String::new(),
                        icon: action.icon,
                        state: None,
                        ago: String::new(),
                        then: Then::Do(action.then.clone()),
                    },
                ))
            })
            .collect();
        // Actions keep the order they are listed in among equal matches.
        actions.sort_by_key(|a| std::cmp::Reverse(a.0));
        let actions: Vec<Found> = actions.into_iter().map(|(_, _, found)| found).collect();

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
                        let (on_click, on_mark) = (this.clone(), this.clone());
                        let mark_key = key.clone();
                        let trailing = match found.state {
                            Some(state) => {
                                let (ink, _) = state.colors(&p);
                                div()
                                    .mono(11.)
                                    .text_color(ink)
                                    .child(state.label())
                                    .into_any_element()
                            }
                            None => div()
                                .mono(11.)
                                .text_color(p.faint)
                                .child(SharedString::from(found.ago.clone()))
                                .into_any_element(),
                        };
                        QuickRow::new(row_id(&key), found.title.clone())
                            .pieces(found.pieces.clone())
                            .icon(found.icon)
                            .line(found.line.clone())
                            .trailing(trailing)
                            .marked(marked.as_deref() == Some(found.key.as_str()))
                            .on_click(move |_, window, cx| {
                                on_click
                                    .update(cx, |this, cx| this.choose(&key, window, cx))
                                    .ok();
                            })
                            .on_mark(move |cx| {
                                on_mark.update(cx, |this, cx| this.mark(&mark_key, cx)).ok();
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
        QuickPanel::new(SWITCHER)
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
            .field(Field::new(
                SWITCHER_FIELD,
                "Go to a session, a project or an action",
                field_frame("switcher-input", &self.field, cx),
            ))
            .results(rows)
            .empty(presence(
                "switcher-nothing",
                nothing,
                EmptyLine::new(
                    "switcher-none",
                    Icon::Search,
                    "Nothing matches what is typed.",
                ),
            ))
    }
}
