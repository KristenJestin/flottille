//! A project's page: it shows and runs, it configures nothing (that is its Settings). Its header
//! (the name and folder, a read-only chip per repository with the branch it is on, which opens its
//! row in the settings, Settings and New workspace), the composer that starts a session, "Where
//! the work happens" (a card per place, `main` first, then each workspace, ending with a card that
//! makes a new one), then the runs and the tasks. Each card is its own view: a session changing
//! redraws its card, a letter typed in the composer redraws neither.

use std::collections::HashMap;

use flottille_core::workspace::StepState;
use flottille_ui::color::mix;
use flottille_ui::{
    AgentState, Block, Button, Card, CardHead, Chip, Confirm, Field, FlatRow, FooterAction, Icon,
    Item, LINE_GAP, LabelledLine, Listing, Menu, Modal, PageHeader, Section, StateIcon, Status,
    StepRow, StepTone, Toast, Type, caption, enter, hoverable, icon, lines_block, palette,
    presence, presence_of, selectable, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Changes, Command, Details, RepositoryStatus};
use crate::composer::{Choice, Composer, agents_label};
use crate::model::{Place, Preparation, SetupLine, ago};
use crate::project_settings::Reveal;
use crate::repositories::ProjectRepositories;
use crate::session::seconds;
use crate::session_link::{PullRequest, TicketFacts};
use crate::shell::{Selection, Shell, now_ms};
use crate::tasks::ProjectTasks;
use crate::tree::place_name;

/// The element id of a place's card: `place-card-main` or `place-card-<workspace>`.
pub fn card_id(place: &Place) -> ElementId {
    ElementId::Name(format!("place-card-{}", place_name(place)).into())
}

/// A session as a place's card lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionLine {
    pub id: String,
    pub title: String,
    pub agents: usize,
    pub mark: Option<AgentState>,
    pub active_at: u64,
    /// The state its row in the tree reads: one state, held once.
    pub status: Entity<Status>,
}

/// What a place's card shows.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceData {
    pub place: Place,
    /// `from develop`; `main` has none, its title says what it is.
    pub meta: Option<String>,
    /// Its folder, which "Open in editor" opens.
    pub folder: String,
    /// `Ready` for `main`.
    pub preparation: Preparation,
    /// Each step of its preparation; none for `main`.
    pub steps: Vec<SetupLine>,
    pub sessions: Vec<SessionLine>,
}

/// What the page shows of its project, as the shell's read gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectData {
    pub name: String,
    pub folder: String,
    pub repositories: Vec<String>,
    pub places: Vec<PlaceData>,
}

/// A repository's name on the page: the project's own name for the folder itself.
pub fn repository_label(project: &str, path: &str) -> String {
    match path {
        "." => project.to_owned(),
        path => path.to_owned(),
    }
}

/// A folder as the page shows it: under the home folder, from `~`.
fn home_relative(folder: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && folder.starts_with(&format!("{home}/")) => {
            format!("~{}", &folder[home.len()..])
        }
        _ => folder.to_owned(),
    }
}

pub struct ProjectPage {
    shell: WeakEntity<Shell>,
    name: String,
    folder: String,
    repositories: Vec<String>,
    branches: Vec<(String, Option<String>)>,
    cards: Vec<Entity<PlaceCard>>,
    /// The place of each card, kept here: reading a card while drawing the page would draw the
    /// page again each time the card changes.
    places: Vec<Place>,
    composer: Entity<Composer>,
    new_workspace: Entity<NewWorkspace>,
    /// Its tasks, each with Run, and the runs: the user's runs, the run drawer.
    tasks: Entity<ProjectTasks>,
    /// Its repositories as the daemon lists them, read for the new-workspace form's rules; they
    /// are listed and changed in the settings.
    repositories_section: Entity<ProjectRepositories>,
    narrow: bool,
    /// Nothing but `main`, and no session in it: the page says what to start with, instead of a
    /// section of one bare card. Kept here for the same reason as `places`.
    empty: bool,
    /// How many times it was drawn: the tests count them.
    renders: usize,
}

impl ProjectPage {
    pub fn new(
        shell: WeakEntity<Shell>,
        name: String,
        composer: Entity<Composer>,
        new_workspace: Entity<NewWorkspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let tasks = {
            let (shell, project) = (shell.clone(), name.clone());
            cx.new(|cx| ProjectTasks::new(shell, project, cx))
        };
        // Asked once the shell, which is making this page, is no longer being updated.
        let reading = tasks.clone();
        cx.defer(move |cx| reading.update(cx, |tasks, cx| tasks.read(cx)));
        let repositories_section = {
            let (shell, project) = (shell.clone(), name.clone());
            cx.new(|_| ProjectRepositories::new(shell, project))
        };
        let reading = repositories_section.clone();
        cx.defer(move |cx| reading.update(cx, |section, cx| section.read(cx)));
        Self {
            shell,
            name,
            folder: String::new(),
            repositories: Vec::new(),
            branches: Vec::new(),
            cards: Vec::new(),
            places: Vec::new(),
            composer,
            new_workspace,
            tasks,
            repositories_section,
            narrow: false,
            empty: false,
            renders: 0,
        }
    }

    /// Whether the page shows its empty state in place of "Where the work happens".
    pub fn is_empty(&self) -> bool {
        self.empty
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn tasks(&self) -> &Entity<ProjectTasks> {
        &self.tasks
    }

    pub fn repositories(&self) -> &Entity<ProjectRepositories> {
        &self.repositories_section
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    pub fn cards(&self) -> &[Entity<PlaceCard>] {
        &self.cards
    }

    pub fn card(&self, place: &Place, cx: &gpui_kit::App) -> Option<Entity<PlaceCard>> {
        self.cards
            .iter()
            .find(|card| &card.read(cx).data.place == place)
            .cloned()
    }

    /// The chips of the header: each repository and the branch it is on, once known.
    pub fn chips(&self) -> Vec<String> {
        self.chip_parts()
            .into_iter()
            .map(|(_, label, branch)| match branch {
                Some(branch) => format!("{label} · {branch}"),
                None => label,
            })
            .collect()
    }

    /// Each repository's path, its name and its branch.
    fn chip_parts(&self) -> Vec<(String, String, Option<String>)> {
        self.repositories
            .iter()
            .map(|path| {
                let branch = self
                    .branches
                    .iter()
                    .find(|(p, _)| p == path)
                    .and_then(|(_, branch)| branch.clone());
                (path.clone(), repository_label(&self.name, path), branch)
            })
            .collect()
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// Brings the page up to `data`; a card whose data did not change is not drawn again.
    pub fn set(&mut self, data: ProjectData, cx: &mut Context<Self>) {
        let mut changed = self.folder != data.folder || self.repositories != data.repositories;
        self.folder = data.folder;
        self.repositories = data.repositories;
        let mut old = std::mem::take(&mut self.cards);
        let mut cards = Vec::new();
        self.places = data.places.iter().map(|p| p.place.clone()).collect();
        let empty = !data.places.is_empty()
            && data
                .places
                .iter()
                .all(|p| p.place.is_none() && p.sessions.is_empty());
        if self.empty != empty {
            self.empty = empty;
            changed = true;
            self.new_workspace
                .update(cx, |form, cx| form.set_framed(!empty, cx));
        }
        for place in data.places {
            let at = old
                .iter()
                .position(|card| card.read(cx).data.place == place.place);
            let card = match at {
                Some(at) => old.remove(at),
                None => {
                    changed = true;
                    let shell = self.shell.clone();
                    let project = self.name.clone();
                    let first = place.clone();
                    cx.new(|cx| PlaceCard::new(shell, project, first, cx))
                }
            };
            if card.read(cx).data != place {
                card.update(cx, |card, cx| card.set(place, cx));
            }
            cards.push(card);
        }
        changed |= !old.is_empty();
        self.cards = cards;
        if changed {
            cx.notify();
        }
    }

    /// What the daemon says beyond the shell's read: branches, lines changed, where each
    /// repository stands, pull requests and tickets.
    pub fn set_details(&mut self, details: &Details, cx: &mut Context<Self>) {
        if details.project != self.name {
            return;
        }
        if self.branches != details.branches {
            self.branches = details.branches.clone();
            cx.notify();
        }
        for card in &self.cards {
            let Some(name) = card.read(cx).data.place.clone() else {
                continue;
            };
            let facts = Facts {
                changes: details.changes.get(&name).copied(),
                pulls: details.pulls.get(&name).cloned().unwrap_or_default(),
                statuses: details.statuses.get(&name).cloned().unwrap_or_default(),
                ticket: details.tickets.get(&name).cloned(),
            };
            if card.read(cx).facts != facts {
                card.update(cx, |card, cx| {
                    card.facts = facts;
                    cx.notify();
                });
            }
        }
    }

    /// The times move on: a card is drawn again only when one of its texts changes.
    pub fn tick(page: &Entity<Self>, now: u64, cx: &mut gpui_kit::App) {
        for card in page.read(cx).cards.clone() {
            if card.read(cx).times(now) != card.read(cx).shown_times {
                card.update(cx, |_, cx| cx.notify());
            }
        }
    }
}

impl Render for ProjectPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let pad = if self.narrow { space::LG } else { space::XXL };
        let key = |part: &str| ElementId::Name(format!("enter-{}-{part}", self.name).into());
        let new_workspace = self.new_workspace.clone();
        let chips = self.chip_parts().into_iter().map(|(path, label, branch)| {
            let (shell, project, at) = (self.shell.clone(), self.name.clone(), path.clone());
            let chip = Chip::new(
                ElementId::Name(format!("repository-chip-{path}").into()),
                Icon::Repository,
                SharedString::from(label),
            )
            .on_click(move |_, window, cx| {
                let at = Reveal::Repository(at.clone());
                shell
                    .update(cx, |shell, cx| {
                        shell.open_project_settings_at(&project, at, window, cx)
                    })
                    .ok();
            });
            match branch {
                Some(branch) => chip.faint(SharedString::from(format!("· {branch}"))),
                None => chip,
            }
        });
        let header = {
            let (shell, name) = (self.shell.clone(), self.name.clone());
            let mut header = PageHeader::new("project-header", self.name.clone())
                .path(home_relative(&self.folder))
                .action(
                    "project-settings",
                    "Settings",
                    Some(Icon::Settings),
                    move |window, cx| {
                        let selection = Selection::ProjectSettings(name.clone());
                        shell
                            .update(cx, |shell, cx| shell.open(selection, window, cx))
                            .ok();
                    },
                )
                .action(
                    "new-workspace",
                    "New workspace",
                    Some(Icon::Plus),
                    move |window, cx| new_workspace.update(cx, |form, cx| form.open(window, cx)),
                );
            for chip in chips {
                header = header.below(chip);
            }
            header
        };
        let columns = if self.narrow { 1 } else { 2 };
        let places = self.cards.len();
        let grid =
            div()
                .grid()
                .grid_cols(columns)
                .gap(space::MD)
                .children(self.cards.iter().zip(&self.places).enumerate().map(
                    |(i, (card, place))| {
                        enter(
                            ElementId::Name(
                                format!("enter-{}-{}", self.name, place_name(place)).into(),
                            ),
                            3 + i,
                            card.clone(),
                        )
                    },
                ))
                .child(enter(
                    key("new-workspace"),
                    3 + places,
                    self.new_workspace.clone(),
                ));
        // What a workspace is, and the two ways to start: the form's modal stays in the tree,
        // which only the dashed card of the grid carries otherwise.
        let nothing = {
            let (shell, project) = (self.shell.clone(), self.name.clone());
            let new_workspace = self.new_workspace.clone();
            div()
                .id("project-empty")
                .flex()
                .flex_col()
                .items_center()
                .gap(space::LG)
                .py(space::XXL)
                .child(
                    div()
                        .max_w(px(440.))
                        .text_size(px(13.))
                        .text_center()
                        .text_color(p.mute)
                        .child(
                            "A workspace is a branch of its own in every repository of the \
                             project, so a session can work there without touching main.",
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_center()
                        .gap(space::SM)
                        .child(
                            Button::secondary("empty-new-session", "New session in main")
                                .leading(Icon::Plus)
                                .on_click(move |_, window, cx| {
                                    let project = project.clone();
                                    shell
                                        .update(cx, |shell, cx| {
                                            shell.compose_in(&project, Choice::Main, window, cx)
                                        })
                                        .ok();
                                }),
                        )
                        .child({
                            let form = new_workspace.clone();
                            Button::secondary("empty-new-workspace", "New workspace")
                                .leading(Icon::Plus)
                                .on_click(move |_, window, cx| {
                                    form.update(cx, |form, cx| form.open(window, cx))
                                })
                        }),
                )
                .child(new_workspace)
                .test_support()
        };
        let places_section = Section::new("project-places", "Where the work happens")
            .count(places.to_string())
            .child(grid);
        let column = div()
            .max_w(space::COLUMN + pad * 2.)
            .w_full()
            .mx_auto()
            .px(pad)
            .pt(space::XXL)
            .pb(space::XXXL)
            .flex()
            .flex_col()
            .gap(space::XXL)
            .child(enter(key("header"), 0, header))
            .child(enter(key("composer"), 1, self.composer.clone()))
            .when(self.empty, |d| d.child(enter(key("empty"), 2, nothing)))
            .when(!self.empty, |d| {
                d.child(enter(key("places"), 2, places_section))
            })
            .child(enter(key("tasks"), 4 + places, self.tasks.clone()));
        div()
            .id("project-page-frame")
            .relative()
            .size_full()
            .child(
                div()
                    .id("project-page")
                    .size_full()
                    .overflow_y_scroll()
                    .child(column),
            )
            .child(self.tasks.read(cx).panel().clone())
    }
}

/// What a card's own action is doing, while the daemon answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Removing,
    Preparing,
}

/// What the daemon says of a workspace beyond the shell's read.
#[derive(Clone, Debug, Default, PartialEq)]
struct Facts {
    /// Lines added and removed.
    changes: Option<Changes>,
    pulls: Vec<PullRequest>,
    /// Where each of its repositories stands.
    statuses: Vec<RepositoryStatus>,
    ticket: Option<TicketFacts>,
}

/// The state a step's mark says.
fn step_state(state: StepState) -> AgentState {
    match state {
        StepState::Done | StepState::Skipped => AgentState::Completed,
        StepState::Running => AgentState::Running,
        StepState::Pending => AgentState::Idle,
        StepState::Failed => AgentState::Failed,
    }
}

fn step_word(state: StepState) -> &'static str {
    match state {
        StepState::Done => "done",
        StepState::Skipped => "skipped",
        StepState::Running => "running",
        StepState::Pending => "to do",
        StepState::Failed => "failed",
    }
}

/// A labelled line of a place's card.
#[derive(Clone)]
enum Line {
    Repository(RepositoryStatus),
    Pull(PullRequest),
    Ticket(TicketFacts),
}

/// How long a step took, or has been running.
fn step_time(step: &SetupLine, now: u64) -> Option<String> {
    match (step.state, step.started_at, step.ended_at) {
        (StepState::Skipped, ..) => Some("skipped".into()),
        (StepState::Running, Some(started), _) => Some(seconds(now.saturating_sub(started))),
        (StepState::Done | StepState::Failed, Some(started), Some(ended)) => {
            Some(seconds(ended.saturating_sub(started)))
        }
        _ => None,
    }
}

/// A place's card: its head (name, where it started from, the lines changed, its "⋯"), its
/// labelled lines (each repository, its pull request, its ticket), its setup while it is
/// prepared, its sessions, and "New session in …". A line is drawn between two blocks that exist.
pub struct PlaceCard {
    shell: WeakEntity<Shell>,
    project: String,
    data: PlaceData,
    facts: Facts,
    /// The removal's confirmation is open.
    confirming: bool,
    busy: Option<Busy>,
    icons: HashMap<String, Entity<StateIcon>>,
    /// Each step's mark, by its position: it turns to the step's new state when it changes.
    marks: HashMap<u32, (Entity<Status>, Entity<StateIcon>)>,
    /// The labelled lines, the setup's steps and the sessions as last drawn: a row that comes
    /// unfolds, one that goes folds away.
    line_rows: Listing<Line>,
    step_rows: Listing<SetupLine>,
    session_rows: Listing<SessionLine>,
    menu: Option<Entity<Menu>>,
    /// "Link a ticket" is open, its field made the first time.
    linking: bool,
    ticket_field: Option<Entity<InputState>>,
    ticket_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
    shown_times: Vec<String>,
    renders: usize,
}

impl PlaceCard {
    fn new(
        shell: WeakEntity<Shell>,
        project: String,
        data: PlaceData,
        cx: &mut Context<Self>,
    ) -> Self {
        let menu = data.place.clone().map(|name| {
            let this = cx.entity().downgrade();
            let (opening, linking, removing) = (this.clone(), this.clone(), this);
            cx.new(|cx| {
                Menu::new(
                    ElementId::Name(format!("workspace-menu-{name}").into()),
                    "Workspace actions",
                    cx,
                )
                .entry(
                    ElementId::Name(format!("open-in-editor-{name}").into()),
                    "Open in editor",
                    Some(Icon::OpenInEditor),
                    move |_, cx| {
                        opening.update(cx, |card, cx| card.open_in_editor(cx)).ok();
                    },
                )
                .entry(
                    ElementId::Name(format!("link-ticket-{name}").into()),
                    "Link a ticket",
                    Some(Icon::Ticket),
                    move |_, cx| {
                        linking
                            .update(cx, |card, cx| {
                                card.linking = true;
                                card.ticket_error = None;
                                cx.notify();
                            })
                            .ok();
                    },
                )
                .danger(
                    ElementId::Name(format!("remove-workspace-{name}").into()),
                    "Remove workspace…",
                    move |_, cx| {
                        removing
                            .update(cx, |card, cx| {
                                card.confirming = true;
                                cx.notify();
                            })
                            .ok();
                    },
                )
            })
        });
        let mut card = Self {
            shell,
            project,
            data: PlaceData {
                sessions: Vec::new(),
                ..data.clone()
            },
            facts: Facts::default(),
            confirming: false,
            busy: None,
            icons: HashMap::new(),
            marks: HashMap::new(),
            line_rows: Listing::default(),
            step_rows: Listing::default(),
            session_rows: Listing::default(),
            menu,
            linking: false,
            ticket_field: None,
            ticket_error: None,
            _subscriptions: Vec::new(),
            shown_times: Vec::new(),
            renders: 0,
        };
        card.set(data, cx);
        card
    }

    pub fn data(&self) -> &PlaceData {
        &self.data
    }

    pub fn changes(&self) -> Option<Changes> {
        self.facts.changes
    }

    pub fn pulls(&self) -> &[PullRequest] {
        &self.facts.pulls
    }

    pub fn statuses(&self) -> &[RepositoryStatus] {
        &self.facts.statuses
    }

    pub fn ticket(&self) -> Option<&TicketFacts> {
        self.facts.ticket.as_ref()
    }

    /// Its "⋯" menu, opened: a workspace's card only.
    pub fn open_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &self.menu {
            menu.update(cx, |m, cx| m.open(window, cx));
        }
    }

    /// The removal's confirmation, opened as "Remove workspace…" does.
    pub fn ask_removal(&mut self, cx: &mut Context<Self>) {
        self.confirming = true;
        cx.notify();
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    fn set(&mut self, data: PlaceData, cx: &mut Context<Self>) {
        self.icons
            .retain(|id, _| data.sessions.iter().any(|s| &s.id == id));
        for session in &data.sessions {
            let status = session.status.clone();
            self.icons
                .entry(session.id.clone())
                .or_insert_with(|| cx.new(|cx| StateIcon::new(status, cx)));
        }
        for step in &data.steps {
            let (at, state) = (step.position, step_state(step.state));
            match self.marks.get(&at) {
                Some((status, _)) => {
                    if status.read(cx).state() != state {
                        status.update(cx, |status, cx| status.set(state, cx));
                    }
                }
                None => {
                    let status = cx.new(|_| Status::new(state));
                    let icon = {
                        let status = status.clone();
                        cx.new(|cx| StateIcon::new(status, cx))
                    };
                    self.marks.insert(at, (status, icon));
                }
            }
        }
        // A preparation started again is under way.
        if self.data.preparation != data.preparation && self.busy == Some(Busy::Preparing) {
            self.busy = None;
        }
        self.data = data;
        cx.notify();
    }

    /// What the daemon answered to this card's removal or new preparation: a toast says it.
    pub fn answered(&mut self, refused: Option<&str>, cx: &mut Context<Self>) {
        let name = place_name(&self.data.place).to_owned();
        let said = match (self.busy.take(), refused) {
            (Some(Busy::Removing), None) => Toast::success(format!("Workspace {name} removed"))
                .detail("Its folder is gone; a branch with commits of its own stays."),
            (Some(Busy::Removing), Some(why)) => {
                Toast::failure(format!("Workspace {name} was not removed")).detail(why.to_owned())
            }
            (_, None) => Toast::success(format!("Preparing {name} again"))
                .detail("Its card lists the steps."),
            (_, Some(why)) => {
                Toast::failure(format!("{name} was not prepared again")).detail(why.to_owned())
            }
        };
        toast(said, cx);
        self.confirming = false;
        cx.notify();
    }

    /// What the daemon answered to "Link a ticket".
    pub fn ticket_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        if !self.linking {
            return;
        }
        match refused {
            None => {
                let typed = self
                    .ticket_field
                    .as_ref()
                    .map(|field| field.read(cx).value().trim().to_owned())
                    .unwrap_or_default();
                let name = place_name(&self.data.place).to_owned();
                toast(
                    Toast::success(format!("{name} linked to {typed}"))
                        .detail("Its card shows the ticket."),
                    cx,
                );
                self.linking = false;
                self.ticket_error = None;
            }
            Some(why) => self.ticket_error = Some(why.into()),
        }
        cx.notify();
    }

    fn open_in_editor(&self, cx: &mut Context<Self>) {
        let folder = self.data.folder.clone();
        self.shell
            .update(cx, |shell, _| shell.open_link(&folder))
            .ok();
    }

    fn link_ticket(&mut self, cx: &mut Context<Self>) {
        let (Some(name), Some(field)) = (self.data.place.clone(), self.ticket_field.clone()) else {
            return;
        };
        let typed = field.read(cx).value().trim().to_owned();
        if typed.is_empty() {
            self.ticket_error = Some("Type a ticket: #123 or ABC-123.".into());
            cx.notify();
            return;
        }
        let command = Command::LinkTicket {
            project: self.project.clone(),
            name,
            ticket: Some(typed),
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
    }

    /// The texts that move with the time: the sessions' times ago and the running step's.
    fn times(&self, now: u64) -> Vec<String> {
        self.data
            .sessions
            .iter()
            .filter(|s| s.mark.is_none())
            .map(|s| ago(now, s.active_at))
            .chain(
                self.data
                    .steps
                    .iter()
                    .filter(|step| step.state == StepState::Running)
                    .filter_map(|step| step_time(step, now)),
            )
            .collect()
    }

    fn perform(&mut self, busy: Busy, cx: &mut Context<Self>) {
        let Some(name) = self.data.place.clone() else {
            return;
        };
        let project = self.project.clone();
        let command = match busy {
            Busy::Removing => Command::RemoveWorkspace { project, name },
            Busy::Preparing => Command::PrepareWorkspace { project, name },
        };
        self.busy = Some(busy);
        self.confirming = false;
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// Why removing it now would lose work, when it would: files not committed. Flottille never
    /// forces; the daemon refuses the rest (commits not pushed) and says why.
    fn removal_refused(&self) -> Option<SharedString> {
        let dirty: Vec<(String, u64)> = self
            .facts
            .statuses
            .iter()
            .filter_map(|status| {
                let files = status.uncommitted.filter(|files| *files > 0)?;
                Some((repository_label(&self.project, &status.path), files))
            })
            .collect();
        if dirty.is_empty() {
            return None;
        }
        let files: u64 = dirty.iter().map(|(_, files)| files).sum();
        let places: Vec<String> = dirty.into_iter().map(|(label, _)| label).collect();
        let (count, verb) = match files {
            1 => ("1 file".to_owned(), "is"),
            files => (format!("{files} files"), "are"),
        };
        Some(SharedString::from(format!(
            "{count} {verb} not committed in {}. Flottille never forces: commit or discard \
             them first.",
            places.join(", ")
        )))
    }

    /// The labelled lines that have something to say, each in its frame: a line that comes
    /// unfolds, one that goes folds away.
    fn lines(&mut self, name: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut found = Vec::new();
        for status in &self.facts.statuses {
            let quiet = status.ahead.unwrap_or(0) == 0
                && status.uncommitted.unwrap_or(0) == 0
                && status.note.is_none();
            if !quiet {
                let key = format!("repository-{}", status.path);
                found.push((key, Line::Repository(status.clone())));
            }
        }
        for pull in &self.facts.pulls {
            found.push((
                format!("pull-{}", pull.repository),
                Line::Pull(pull.clone()),
            ));
        }
        if let Some(ticket) = &self.facts.ticket {
            found.push(("ticket".to_owned(), Line::Ticket(ticket.clone())));
        }
        let (now, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        self.line_rows.sync(found, now, reduce);
        let rows: Vec<_> = self
            .line_rows
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        rows.into_iter()
            .map(|(key, line, placement)| {
                Item::new(
                    ElementId::Name(format!("line-item-{name}-{key}").into()),
                    placement,
                    self.line(name, &line, cx),
                )
                .gap(LINE_GAP)
                .into_any_element()
            })
            .collect()
    }

    fn line(&self, name: &str, line: &Line, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        match line {
            Line::Repository(status) => {
                let label = repository_label(&self.project, &status.path);
                let mut parts = Vec::new();
                if let Some(ahead) = status.ahead {
                    parts.push(format!("{ahead} ahead"));
                }
                match status.uncommitted {
                    Some(0) => parts.push("clean".into()),
                    Some(files) => parts.push(format!("{files} uncommitted")),
                    None => {}
                }
                // Why it started from its local branch, when it did.
                parts.extend(status.note.clone());
                let rest = parts.join(" · ");
                LabelledLine::new(
                    ElementId::Name(format!("repository-line-{name}-{label}").into()),
                    Icon::Repository,
                    "Repository",
                    SharedString::from(format!("{label} {rest}")),
                )
                .child(
                    div()
                        .text_color(p.text)
                        .medium()
                        .child(SharedString::from(label.clone())),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(p.mute)
                        .child(SharedString::from(rest)),
                )
                .into_any_element()
            }
            Line::Pull(pull) => {
                let label = repository_label(&self.project, &pull.repository);
                let (shell, url) = (self.shell.clone(), pull.url.clone());
                let (fill, ink) = match pull.state.as_str() {
                    "open" => (p.ok_bg, p.ok),
                    "merged" => (p.run_bg, p.run),
                    "draft" => (p.card2, p.faint),
                    _ => (p.fail_bg, p.fail),
                };
                LabelledLine::new(
                    ElementId::Name(format!("pull-{name}-{label}").into()),
                    Icon::PullRequest,
                    "Pull request",
                    SharedString::from(pull.label()),
                )
                .on_click(move |_, _, cx| {
                    shell.update(cx, |shell, _| shell.open_link(&url)).ok();
                })
                .child(SharedString::from(format!("#{}", pull.number)))
                .child(
                    div()
                        .h(px(16.))
                        .px(space::XS)
                        .rounded(px(5.))
                        .flex()
                        .items_center()
                        .bg(fill)
                        .text_color(ink)
                        .mono(9.5)
                        .medium()
                        .child(SharedString::from(pull.state.to_uppercase())),
                )
                .into_any_element()
            }
            Line::Ticket(ticket) => {
                let title = ticket
                    .title
                    .clone()
                    .or_else(|| ticket.problem.clone())
                    .unwrap_or_default();
                let mut line = LabelledLine::new(
                    ElementId::Name(format!("ticket-{name}").into()),
                    Icon::Ticket,
                    "Ticket",
                    SharedString::from(format!("{} {title}", ticket.reference)),
                )
                .child(SharedString::from(ticket.reference.clone()))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .ui_text()
                        .text_size(px(12.5))
                        .child(SharedString::from(title)),
                );
                if let Some(url) = ticket.url.clone() {
                    let shell = self.shell.clone();
                    line = line.on_click(move |_, _, cx| {
                        shell.update(cx, |shell, _| shell.open_link(&url)).ok();
                    });
                }
                line.into_any_element()
            }
        }
    }

    /// The setup's steps while it is prepared, and what failed: the block stays built once it
    /// is ready, so that it folds away instead of vanishing.
    fn setup(&mut self, name: &str, now: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let steps = self.data.steps.clone();
        self.step_rows.sync(
            steps.into_iter().map(|s| (s.position.to_string(), s)),
            at,
            reduce,
        );
        if self.step_rows.is_empty() {
            return None;
        }
        let p = palette(cx);
        let total = self.data.steps.len();
        let failed = matches!(self.data.preparation, Preparation::Failed { .. });
        let at = self
            .data
            .steps
            .iter()
            .position(|step| !matches!(step.state, StepState::Done | StepState::Skipped))
            .unwrap_or(total.saturating_sub(1));
        let count = match failed {
            true => format!("failed at step {} of {total}", at + 1),
            false => format!("step {} of {total}", at + 1),
        };
        let rows: Vec<_> = self
            .step_rows
            .rows()
            .map(|row| (row.value.clone(), row.placement))
            .collect();
        let rows = rows.into_iter().enumerate().map(|(i, (step, placement))| {
            let tone = match step.state {
                StepState::Done | StepState::Skipped => StepTone::Done,
                StepState::Running => StepTone::Now,
                StepState::Pending => StepTone::Todo,
                StepState::Failed => StepTone::Failed,
            };
            let mark = div()
                .id(ElementId::Name(format!("setup-mark-{name}-{i}").into()))
                .aria_label(step_word(step.state))
                .children(self.marks.get(&step.position).map(|(_, icon)| icon.clone()))
                .test_support();
            let mut row = StepRow::new(
                ElementId::Name(format!("setup-step-{name}-{i}").into()),
                mark,
                SharedString::from(step.verb.clone()),
                tone,
                ElementId::Name(format!("setup-time-{name}-{i}").into()),
            );
            if let Some(object) = &step.object {
                row = row.object(SharedString::from(object.clone()));
            }
            if let Some(time) = step_time(&step, now) {
                row = row.time(SharedString::from(time));
            }
            Item::new(
                ElementId::Name(format!("step-item-{name}-{}", step.position).into()),
                placement,
                row,
            )
        });
        let output = match &self.data.preparation {
            Preparation::Failed { output, .. } => Some(SharedString::from(output.clone())),
            _ => None,
        };
        let failure = {
            let (name, again) = (
                name.to_owned(),
                cx.listener(|this: &mut Self, _, _, cx| this.perform(Busy::Preparing, cx)),
            );
            let busy = self.busy;
            let (field, fail_line, mute) = (p.field, p.fail_line, p.mute);
            presence_of(
                ElementId::Name(format!("failure-{name}").into()),
                output,
                move |output| {
                    div()
                        .px(space::MD)
                        .pt(space::XS)
                        .flex()
                        .flex_col()
                        .gap(space::SM)
                        .child(
                            div()
                                .id(ElementId::Name(format!("failure-output-{name}").into()))
                                .max_h(px(160.))
                                .overflow_y_scroll()
                                .py(space::SM)
                                .px(space::MD)
                                .rounded(px(8.))
                                .bg(field)
                                .border_1()
                                .border_color(fail_line)
                                .mono(11.5)
                                .text_color(mute)
                                .child(output)
                                .test_support(),
                        )
                        .child(
                            div().flex().justify_end().child(
                                Button::secondary(
                                    ElementId::Name(format!("retry-{name}").into()),
                                    if busy == Some(Busy::Preparing) {
                                        "Preparing again…"
                                    } else {
                                        "Prepare again"
                                    },
                                )
                                .disabled(busy.is_some())
                                .on_click(again),
                            ),
                        )
                        .into_any_element()
                },
            )
        };
        Some(
            div()
                .pt(space::XS)
                .pb(space::SM)
                .flex()
                .flex_col()
                .child(caption(
                    ElementId::Name(format!("setup-{name}").into()),
                    "Setup",
                    SharedString::from(count),
                    cx,
                ))
                .children(rows)
                .child(failure)
                .into_any_element(),
        )
    }

    fn link_modal(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) -> Modal {
        if self.linking && self.ticket_field.is_none() {
            let field =
                cx.new(|cx| crate::project_settings::styled_input("#123 or ABC-123", window, cx));
            self._subscriptions
                .push(cx.subscribe_in(&field, window, |this, _, event, _, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.link_ticket(cx)
                    }
                }));
            let focus = field.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            self.ticket_field = Some(field);
        }
        let this = cx.entity().downgrade();
        let mut modal = Modal::new(
            ElementId::Name(format!("link-ticket-modal-{name}").into()),
            format!("Link {name} to a ticket"),
        )
        .sentence(
            "Its card and its sessions show the ticket; agents read it with the tickets tool.",
        )
        .open(self.linking)
        .on_close(move |_, cx| {
            this.update(cx, |this, cx| {
                this.linking = false;
                cx.notify();
            })
            .ok();
        })
        .action(
            Button::primary(
                ElementId::Name(format!("link-ticket-{name}-action").into()),
                "Link",
            )
            .on_click(cx.listener(|this, _, _, cx| this.link_ticket(cx))),
        );
        if let Some(field) = &self.ticket_field {
            modal = modal.child(
                Field::new(
                    ElementId::Name(format!("link-ticket-{name}-field").into()),
                    "Ticket",
                    input_frame(
                        ElementId::Name(format!("link-ticket-{name}-input").into()),
                        field,
                        cx,
                    ),
                )
                .error(self.ticket_error.clone()),
            );
        }
        modal
    }
}

/// A text field's frame in a form: 34 px, the field colours.
fn input_frame(
    id: impl Into<ElementId>,
    field: &Entity<InputState>,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(id.into())
        .h(px(34.))
        .px(space::MD)
        .rounded(px(8.))
        .bg(p.field)
        .border_1()
        .border_color(p.field_line)
        .flex()
        .items_center()
        .mono(12.5)
        .child(Input::new(field))
        .test_support()
}

impl Render for PlaceCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let now = now_ms();
        self.shown_times = self.times(now);
        let place = self.data.place.clone();
        let name = place_name(&place).to_owned();
        let ready = matches!(self.data.preparation, Preparation::Ready);
        let id = |part: &str| ElementId::Name(format!("{part}-{name}").into());

        let mut head = CardHead::new(
            id("place-head"),
            if place.is_none() {
                Icon::Folder
            } else {
                Icon::Branch
            },
            SharedString::from(name.clone()),
        )
        .meta(match &self.data.meta {
            Some(meta) => SharedString::from(meta.clone()),
            None => SharedString::from("the project folder"),
        });
        match (&self.data.preparation, self.facts.changes) {
            (Preparation::Preparing { .. }, _) => {
                head = head.trailing(div().mono(12.).text_color(p.faint).child("preparing"));
            }
            (Preparation::Failed { .. }, _) => {
                head = head.trailing(div().mono(12.).text_color(p.fail).child("failed"));
            }
            (
                Preparation::Ready,
                Some(Changes {
                    added,
                    removed,
                    capped,
                }),
            ) => {
                // Capped: the daemon stopped counting new files, there are more.
                let at_least = if capped { "≥" } else { "" };
                head = head.trailing(
                    div()
                        .id(id("changes"))
                        .flex_none()
                        .flex()
                        .gap(space::XS)
                        .mono(12.)
                        .child(div().text_color(p.ok).child(format!("{at_least}+{added}")))
                        .child(div().text_color(p.fail).child(format!("−{removed}"))),
                );
            }
            _ => {}
        }
        head = match &self.menu {
            Some(menu) => head.trailing(menu.clone()),
            // The project folder has no action of its own: its head keeps the same right edge.
            None => head.trailing(div().w(px(8.))),
        };

        // Each block comes and goes with its line; its rows come and go inside it.
        let found = self.lines(&name, cx);
        let mut lines = Block::new().present(id("lines-block"), !self.line_rows.is_empty());
        if !found.is_empty() {
            lines = lines.child(lines_block(found));
        }
        let steps = self.setup(&name, now, cx);
        let mut setup =
            Block::new().present(id("setup-block"), !ready && !self.data.steps.is_empty());
        if let Some(steps) = steps {
            setup = setup.child(steps);
        }
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let listed = self.data.sessions.clone();
        self.session_rows
            .sync(listed.into_iter().map(|s| (s.id.clone(), s)), at, reduce);
        let mut sessions =
            Block::new().present(id("sessions-block"), !self.session_rows.is_empty());
        if self.session_rows.rows().next().is_some() {
            let shown: Vec<_> = self
                .session_rows
                .rows()
                .map(|row| (row.value.clone(), row.placement))
                .collect();
            let rows = shown.into_iter().map(|(session, placement)| {
                let shell = self.shell.clone();
                let open = session.id.clone();
                let meta = match session.agents {
                    0 | 1 => div()
                        .child(SharedString::from(match session.mark {
                            Some(_) => String::new(),
                            None => ago(now, session.active_at),
                        }))
                        .into_any_element(),
                    n => div()
                        .flex()
                        .items_center()
                        .gap(space::XS)
                        .child(icon(Icon::Agents).size(px(12.)))
                        .child(SharedString::from(agents_label(n)))
                        .into_any_element(),
                };
                let row = FlatRow::new(
                    ElementId::Name(format!("card-session-{}", session.id).into()),
                    SharedString::from(session.title.clone()),
                )
                .when_some(self.icons.get(&session.id).cloned(), |row, icon| {
                    row.leading(icon)
                })
                .meta(meta)
                .on_click(move |_, window, cx| {
                    let session = open.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.open(Selection::Session(session), window, cx)
                        })
                        .ok();
                });
                Item::new(
                    ElementId::Name(format!("session-item-{}", session.id).into()),
                    placement,
                    row,
                )
            });
            sessions = sessions.child(
                div()
                    .pb(space::XS)
                    .flex()
                    .flex_col()
                    .child(caption(
                        id("sessions-caption"),
                        "Sessions",
                        SharedString::from(self.data.sessions.len().to_string()),
                        cx,
                    ))
                    .children(rows),
            );
        }
        let (shell, project) = (self.shell.clone(), self.project.clone());
        let choice = match &place {
            None => Choice::Main,
            Some(name) => Choice::Workspace(name.clone()),
        };
        let footer = Block::new().child(
            FooterAction::new(id("new-session-in"), format!("New session in {name}"))
                .enabled(ready)
                .on_click(move |_, window, cx| {
                    let choice = choice.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.compose_in(&project, choice, window, cx)
                        })
                        .ok();
                }),
        );

        let confirm = {
            let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
            Confirm::new(
                id("workspace-removal"),
                format!("Remove the workspace {name}?"),
            )
            .sentence(
                "Its folder goes, and each repository's branch that holds no commit of its own. \
                 Flottille removes it only once all its work is committed and pushed.",
            )
            .action("Remove workspace")
            .refused(self.removal_refused())
            .open(self.confirming)
            .on_confirm(move |_, cx| {
                this.update(cx, |this, cx| this.perform(Busy::Removing, cx))
                    .ok();
            })
            .on_cancel(move |_, cx| {
                again
                    .update(cx, |this, cx| {
                        this.confirming = false;
                        cx.notify();
                    })
                    .ok();
            })
        };
        let link = place.is_some().then(|| self.link_modal(&name, window, cx));
        div()
            .id(card_id(&place))
            .h_full()
            .child(confirm)
            .children(link)
            .child(
                div().h_full().child(
                    Card::new()
                        .id(id("place"))
                        .fill()
                        .block(Block::new().child(head))
                        .block(lines)
                        .block(setup)
                        .block(sessions)
                        .block(footer),
                ),
            )
            .test_support()
    }
}

/// How long typing rests before the ticket typed is read.
const PREVIEW_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

/// The last cell of the grid: a dashed "New workspace" card, which opens the new-workspace modal:
/// its name, its ticket (read as it is typed), the repositories it takes and their bases, its
/// branch, and a button that creates it, as `flottille workspace create` does (the command line
/// keeps `--offline`; the window always fetches).
pub struct NewWorkspace {
    shell: WeakEntity<Shell>,
    project: String,
    open: bool,
    name: Entity<InputState>,
    has_name: bool,
    /// The ticket it is linked to, if one is typed: `#123` or a Jira key.
    ticket: Entity<InputState>,
    /// The ticket typed and what its tracker said of it; `None` while it is read.
    preview: Option<(String, Option<Result<serde_json::Value, String>>)>,
    /// Each ticket typed is read once typing rests: only the last one asked is shown.
    previews: u64,
    /// The project's repositories, and the default set a workspace takes (all when none).
    repositories: Vec<String>,
    default_set: Option<Vec<String>>,
    /// The repositories chosen; the project's default set until one is changed.
    chosen: Vec<String>,
    /// Each repository's base rule, once the daemon said: what its base field proposes.
    rules: HashMap<String, Vec<String>>,
    /// The base branch typed for each repository, made when its row is first drawn: the rule
    /// when empty. With the rule its placeholder says.
    bases: HashMap<String, (Entity<InputState>, String)>,
    branch: Entity<InputState>,
    /// Drawn as the dashed card of the grid; not when the page's empty state carries the
    /// actions, where only the modal stays.
    framed: bool,
    /// The name asked for, while the daemon makes it.
    creating: Option<String>,
    /// Opened from the composer's picker: the workspace made becomes the composer's place.
    for_composer: bool,
    /// `for_composer` as it was when the name was asked for.
    creating_for_composer: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl NewWorkspace {
    /// The form of one project: a name asked in it is answered in it.
    pub fn new(
        shell: WeakEntity<Shell>,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let p = palette(cx);
            let mut name = InputState::new(window, cx).placeholder("for example login-fix");
            name.set_editor_style(gpui_kit::base::input::InputEditorStyle {
                foreground: p.text,
                muted_foreground: p.faint,
                selection: p.run.opacity(0.35),
                caret: p.text,
                ..Default::default()
            });
            name
        });
        let ticket = cx.new(|cx| {
            let p = palette(cx);
            let mut ticket = InputState::new(window, cx).placeholder("#123 or ABC-123");
            ticket.set_editor_style(gpui_kit::base::input::InputEditorStyle {
                foreground: p.text,
                muted_foreground: p.faint,
                selection: p.run.opacity(0.35),
                caret: p.text,
                ..Default::default()
            });
            ticket
        });
        let branch = cx
            .new(|cx| crate::project_settings::styled_input("prefix/name by default", window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&ticket, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.create(cx),
                InputEvent::Change => this.ticket_changed(window, cx),
                _ => {}
            }),
            cx.subscribe_in(&branch, window, |this, _, event, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.create(cx)
                }
            }),
            cx.subscribe_in(&name, window, |this, name, event, _, cx| match event {
                InputEvent::Change => {
                    let has = !name.read(cx).value().trim().is_empty();
                    if this.has_name != has {
                        this.has_name = has;
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { .. } => this.create(cx),
                _ => {}
            }),
        ];
        Self {
            shell,
            project,
            open: false,
            name,
            has_name: false,
            ticket,
            preview: None,
            previews: 0,
            repositories: Vec::new(),
            default_set: None,
            chosen: Vec::new(),
            rules: HashMap::new(),
            bases: HashMap::new(),
            branch,
            framed: true,
            creating: None,
            for_composer: false,
            creating_for_composer: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn name_field(&self) -> &Entity<InputState> {
        &self.name
    }

    pub fn ticket_field(&self) -> &Entity<InputState> {
        &self.ticket
    }

    pub fn branch_field(&self) -> &Entity<InputState> {
        &self.branch
    }

    /// The ticket typed and what its tracker said of it, once read.
    pub fn preview(&self) -> Option<&(String, Option<Result<serde_json::Value, String>>)> {
        self.preview.as_ref()
    }

    pub fn chosen(&self) -> &[String] {
        &self.chosen
    }

    /// The field a repository's base branch is typed in, once its row was drawn.
    pub fn base_field(&self, path: &str) -> Option<&Entity<InputState>> {
        self.bases.get(path).map(|(field, _)| field)
    }

    /// What a repository's base field proposes while it is empty, once drawn.
    pub fn base_said(&self, path: &str) -> Option<&str> {
        self.bases.get(path).map(|(_, said)| said.as_str())
    }

    /// Each repository's base rule, as the daemon read the project.
    pub fn set_rules(&mut self, rules: HashMap<String, Vec<String>>, cx: &mut Context<Self>) {
        if self.rules != rules {
            self.rules = rules;
            cx.notify();
        }
    }

    /// What a repository's base field says when it is empty: the project's rule.
    fn rule_said(&self, path: &str) -> String {
        match self.rules.get(path).map(Vec::as_slice) {
            Some([first, rest @ ..]) => {
                let rest: String = rest.iter().map(|b| format!(", else {b}")).collect();
                format!("base: {first}{rest}")
            }
            _ => "base: the project's rule".to_owned(),
        }
    }

    /// The base field of `path`, made the first time, its placeholder kept to the rule.
    fn base_of(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let said = self.rule_said(path);
        let (field, shown) = self.bases.entry(path.to_owned()).or_insert_with(|| {
            let field = cx.new(|cx| crate::project_settings::styled_input("", window, cx));
            (field, String::new())
        });
        let field = field.clone();
        if *shown != said {
            *shown = said.clone();
            field.update(cx, |field, cx| field.set_placeholder(said, window, cx));
        }
        field
    }

    /// The project's repositories and its default set, as the daemon last listed them.
    pub fn set_repositories(
        &mut self,
        repositories: Vec<String>,
        default_set: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if self.repositories == repositories && self.default_set == default_set {
            return;
        }
        self.chosen = default_set.clone().unwrap_or_else(|| repositories.clone());
        self.repositories = repositories;
        self.default_set = default_set;
        cx.notify();
    }

    /// Takes the repository, or leaves it out; at least one is taken.
    pub fn toggle_repository(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(at) = self.chosen.iter().position(|p| p == path) {
            if self.chosen.len() > 1 {
                self.chosen.remove(at);
            }
        } else {
            self.chosen.push(path.to_owned());
            let order = &self.repositories;
            self.chosen
                .sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
        }
        cx.notify();
    }

    /// Whether the dashed card is drawn; the modal is either way.
    pub fn set_framed(&mut self, framed: bool, cx: &mut Context<Self>) {
        if self.framed != framed {
            self.framed = framed;
            cx.notify();
        }
    }

    /// The ticket field changed: read what it says once typing rests.
    fn ticket_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.ticket.read(cx).value().trim().to_owned();
        self.previews += 1;
        if typed.is_empty() {
            self.preview = None;
            cx.notify();
            return;
        }
        self.preview = Some((typed.clone(), None));
        let asked = self.previews;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(PREVIEW_AFTER).await;
            this.update(cx, |this, cx| {
                if this.previews == asked {
                    let command = Command::PreviewTicket {
                        project: this.project.clone(),
                        ticket: typed,
                    };
                    this.shell
                        .update(cx, |shell, _| shell.perform(command))
                        .ok();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// What the tracker said of a ticket typed; kept only for the ticket the field says now.
    pub fn previewed(
        &mut self,
        ticket: &str,
        read: Result<serde_json::Value, String>,
        cx: &mut Context<Self>,
    ) {
        if self.ticket.read(cx).value().trim() == ticket {
            self.preview = Some((ticket.to_owned(), Some(read)));
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn creating(&self) -> Option<&str> {
        self.creating.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Opens its modal, the caret in the name.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.for_composer = false;
        self.show(window, cx);
    }

    /// Opens it from the composer's picker: the workspace it makes becomes the composer's place.
    pub fn open_for_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.for_composer = true;
        self.show(window, cx);
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.error = None;
        let focus = self.name.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes its modal: Escape, Cancel or a click outside. A name being made is still made.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.error = None;
        cx.notify();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        if !self.has_name || self.creating.is_some() {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        self.creating = Some(name.clone());
        self.creating_for_composer = self.for_composer;
        self.error = None;
        let ticket = self.ticket.read(cx).value().trim().to_owned();
        let branch = self.branch.read(cx).value().trim().to_owned();
        let default = self
            .default_set
            .clone()
            .unwrap_or_else(|| self.repositories.clone());
        // The base typed for a repository taken; the others keep their rule.
        let bases = self
            .chosen
            .iter()
            .filter_map(|path| {
                let typed = self.bases.get(path)?.0.read(cx).value().trim().to_owned();
                (!typed.is_empty()).then(|| (path.clone(), typed))
            })
            .collect();
        let command = Command::CreateWorkspace {
            project: self.project.clone(),
            name,
            ticket: (!ticket.is_empty()).then_some(ticket),
            // The daemon's default set unless the user changed it.
            repositories: (self.chosen != default && !self.chosen.is_empty())
                .then(|| self.chosen.clone()),
            branch: (!branch.is_empty()).then_some(branch),
            bases,
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// What the daemon answered: made, and the card closes; or refused, and it says why. `true`
    /// when the workspace made is to become the composer's place.
    pub fn answered(
        &mut self,
        refused: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.creating = None;
        let for_composer = refused.is_none() && self.creating_for_composer;
        match refused {
            None => {
                let name = self.name.read(cx).value().trim().to_owned();
                toast(
                    Toast::success(format!("Workspace {name} is being prepared"))
                        .detail("Its card lists the steps of its setup."),
                    cx,
                );
                self.open = false;
                self.error = None;
                self.name
                    .update(cx, |name, cx| name.set_value("", window, cx));
                self.has_name = false;
                self.ticket
                    .update(cx, |ticket, cx| ticket.set_value("", window, cx));
                self.branch
                    .update(cx, |branch, cx| branch.set_value("", window, cx));
                for (field, _) in self.bases.values() {
                    field.update(cx, |field, cx| field.set_value("", window, cx));
                }
                self.preview = None;
                self.chosen = self
                    .default_set
                    .clone()
                    .unwrap_or_else(|| self.repositories.clone());
            }
            Some(why) => self.error = Some(SharedString::from(why.to_owned())),
        }
        cx.notify();
        for_composer
    }
}

impl NewWorkspace {
    /// What the ticket typed is: its tracker and title, or why it cannot be read.
    fn preview_line(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let p = palette(cx);
        let (typed, read) = self.preview.as_ref()?;
        let (text, color) = match read {
            None => (format!("Reading {typed}…"), p.faint),
            Some(Err(why)) => (why.clone(), p.fail),
            Some(Ok(read)) => match read["problem"].as_str() {
                Some(problem) => (problem.to_owned(), p.fail),
                None => (
                    format!(
                        "{} · {} {}: {}",
                        read["tracker_said"].as_str().unwrap_or_default(),
                        read["ticket"].as_str().unwrap_or_default(),
                        read["state"].as_str().unwrap_or_default(),
                        read["title"].as_str().unwrap_or_default()
                    ),
                    p.mute,
                ),
            },
        };
        Some(
            div()
                .id("new-workspace-ticket-preview")
                .text_size(px(12.5))
                .text_color(color)
                .child(SharedString::from(text))
                .test_support(),
        )
    }
}

impl Render for NewWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let card = div()
            .id("new-workspace-card")
            .aria_label("New workspace")
            .min_h(px(150.))
            .h_full()
            .p(space::XL)
            .rounded(px(12.))
            .border_1()
            .border_dashed()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .ui_text()
            .text_size(px(13.))
            .cursor_pointer()
            .child(icon(Icon::Plus).size(px(16.)).text_color(p.faint))
            .child(
                div()
                    .mt(px(6.))
                    .text_color(p.text)
                    .medium()
                    .child("New workspace"),
            )
            .child("a branch of its own in every repository")
            .on_click(cx.listener(|this, _, window, cx| this.open(window, cx)));
        let creating = self.creating.is_some();
        let this = cx.entity().downgrade();
        let repositories = {
            // A row per repository: taken or not (when there is a choice), and its base.
            let several = self.repositories.len() > 1;
            let rows: Vec<_> = self
                .repositories
                .clone()
                .into_iter()
                .map(|path| {
                    let taken = self.chosen.contains(&path);
                    let base = self.base_of(&path, window, cx);
                    let toggled = path.clone();
                    let chip = div()
                        .id(ElementId::Name(format!("new-workspace-take-{path}").into()))
                        .flex_none()
                        .w(px(140.))
                        .h(px(34.))
                        .px(space::SM)
                        .rounded(px(8.))
                        .border_1()
                        .flex()
                        .items_center()
                        .gap(space::SM)
                        .mono(12.)
                        .when(several, |chip| chip.cursor_pointer())
                        // By shape as well as colour: a tick when it is taken.
                        .child(icon(if taken { Icon::Check } else { Icon::Plus }).size(px(12.)))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(repository_label(&self.project, &path))),
                        )
                        .when(several, |chip| {
                            chip.on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_repository(&toggled, cx)
                            }))
                        });
                    let chip = selectable(chip, taken, move |d, hover, taken| {
                        d.bg(mix(p.hover.opacity(hover), p.card2, taken))
                            .border_color(mix(p.field_line, p.focus_line, taken))
                            .text_color(mix(p.faint, p.text, taken))
                    });
                    div().flex().items_start().gap(space::SM).child(chip).child(
                        div().flex_1().min_w_0().child(presence(
                            ElementId::Name(format!("new-workspace-base-{path}-block").into()),
                            taken,
                            input_frame(
                                ElementId::Name(format!("new-workspace-base-{path}").into()),
                                &base,
                                cx,
                            ),
                        )),
                    )
                })
                .collect();
            div()
                .id("new-workspace-repositories")
                .flex()
                .flex_col()
                .gap(space::SM)
                .children(rows)
        };
        let preview = self.preview_line(cx).map(IntoElement::into_any_element);
        let modal = Modal::new("new-workspace-modal", "New workspace")
            .sentence("A branch of its own in each repository it takes.")
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            })
            .child(
                Field::new(
                    "new-workspace-name",
                    "Name",
                    input_frame("new-workspace-modal-name", &self.name, cx),
                )
                .error(self.error.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .child(
                        Field::new(
                            "new-workspace-ticket",
                            "Ticket",
                            input_frame("new-workspace-modal-ticket", &self.ticket, cx),
                        )
                        .optional(),
                    )
                    .children(preview),
            )
            .when(!self.repositories.is_empty(), |modal| {
                modal.child(
                    Field::new(
                        "new-workspace-takes",
                        "Repositories and their bases",
                        repositories,
                    )
                    .hint("The base is the project's rule when left empty."),
                )
            })
            .child(
                Field::new(
                    "new-workspace-branch",
                    "Branch",
                    input_frame("new-workspace-modal-branch", &self.branch, cx),
                )
                .optional(),
            )
            .action(
                Button::primary(
                    "create-workspace",
                    if creating { "Creating…" } else { "Create" },
                )
                .disabled(!self.has_name || creating)
                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
            );
        if !self.framed {
            return div()
                .id("new-workspace-anchor")
                .child(modal)
                .into_any_element();
        }
        hoverable(card.child(modal), move |d, t| {
            d.bg(p.hover.opacity(t))
                .text_color(mix(p.mute, p.text, t))
                .border_color(mix(p.line2, p.focus_line, t))
        })
        .into_any_element()
    }
}
