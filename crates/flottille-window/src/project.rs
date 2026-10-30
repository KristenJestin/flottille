//! A project's page: its name and folder, a chip per repository with the branch it is on, the
//! composer that starts a session, and "Where the work happens": a card per place (`main` first,
//! then each workspace), ending with a card that makes a new workspace. Each card is its own view:
//! a session changing redraws its card, a letter typed in the composer redraws neither.

use std::collections::HashMap;

use flottille_ui::color::mix;
use flottille_ui::{
    AgentState, Button, Card, Confirm, Icon, ListRow, Modal, Pill, StateIcon, Status, Switch, Type,
    enter, hoverable, icon, palette, presence, presence_of, selectable, space,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Changes, Command, Details, RepositoryStatus};
use crate::composer::{Choice, Composer, agents_label};
use crate::model::{Place, Preparation, ago};
use crate::repositories::ProjectRepositories;
use crate::session::{pull_hover, pull_row};
use crate::session_link::PullRequest;
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
    /// `from develop · …`; `main` has none, its title says what it is.
    pub meta: Option<String>,
    /// `Ready` for `main`.
    pub preparation: Preparation,
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
    /// Its tasks, each with a Run button, and what the user ran.
    tasks: Entity<ProjectTasks>,
    /// Its repositories, added and removed there.
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
        self.repositories
            .iter()
            .map(|path| {
                let label = repository_label(&self.name, path);
                match self.branches.iter().find(|(p, _)| p == path) {
                    Some((_, Some(branch))) => format!("{label} · {branch}"),
                    _ => label,
                }
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

    /// What the daemon says beyond the shell's read: branches and lines changed.
    pub fn set_details(&mut self, details: &Details, cx: &mut Context<Self>) {
        if details.project != self.name {
            return;
        }
        if self.branches != details.branches {
            self.branches = details.branches.clone();
            cx.notify();
        }
        for card in &self.cards {
            let changes = match &card.read(cx).data.place {
                None => None,
                Some(name) => details.changes.get(name).copied(),
            };
            if card.read(cx).changes != changes {
                card.update(cx, |card, cx| {
                    card.changes = changes;
                    cx.notify();
                });
            }
            let pulls = match &card.read(cx).data.place {
                None => Vec::new(),
                Some(name) => details.pulls.get(name).cloned().unwrap_or_default(),
            };
            if card.read(cx).pulls != pulls {
                card.update(cx, |card, cx| {
                    card.pulls = pulls;
                    cx.notify();
                });
            }
            let statuses = match &card.read(cx).data.place {
                None => Vec::new(),
                Some(name) => details.statuses.get(name).cloned().unwrap_or_default(),
            };
            if card.read(cx).statuses != statuses {
                card.update(cx, |card, cx| {
                    card.statuses = statuses;
                    cx.notify();
                });
            }
        }
    }

    /// The times ago move on: a card is drawn again only when one of its texts changes.
    pub fn tick(page: &Entity<Self>, now: u64, cx: &mut gpui_kit::App) {
        for card in page.read(cx).cards.clone() {
            if card.read(cx).agos(now) != card.read(cx).shown_agos {
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
        let header = div()
            .flex()
            .items_end()
            .gap(space::LG)
            .child(
                div()
                    .flex_none()
                    .text_size(px(24.))
                    .line_height(px(29.))
                    .semibold()
                    .child(SharedString::from(self.name.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pb(space::XS)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .mono(12.)
                    .text_color(p.faint)
                    .child(SharedString::from(self.folder.clone())),
            )
            .child({
                let (shell, name) = (self.shell.clone(), self.name.clone());
                Button::quiet("project-settings", "Settings")
                    .leading(Icon::Settings)
                    .on_click(move |_, window, cx| {
                        let selection = Selection::ProjectSettings(name.clone());
                        shell
                            .update(cx, |shell, cx| shell.open(selection, window, cx))
                            .ok();
                    })
            })
            .child(
                Button::secondary("new-workspace", "New workspace")
                    .leading(Icon::Plus)
                    .on_click(move |_, window, cx| {
                        new_workspace.update(cx, |form, cx| form.open(window, cx))
                    }),
            );
        let chips = div()
            .id("repositories")
            .flex()
            .flex_wrap()
            .gap(space::SM)
            .mt(px(10.))
            .mb(px(22.))
            .children(self.chips().into_iter().enumerate().map(|(i, chip)| {
                let (name, branch) = match chip.split_once(" · ") {
                    Some((name, branch)) => (name.to_owned(), Some(format!("· {branch}"))),
                    None => (chip, None),
                };
                div()
                    .id(ElementId::Name(format!("repository-{i}").into()))
                    .h(px(24.))
                    .px(space::SM)
                    .rounded(px(6.))
                    .bg(p.card)
                    .border_1()
                    .border_color(p.line)
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .mono(11.5)
                    .text_color(p.mute)
                    .child(icon(Icon::Repository).size(px(12.)).text_color(p.faint))
                    .child(SharedString::from(name))
                    .when_some(branch, |d, branch| {
                        d.child(div().text_color(p.faint).child(SharedString::from(branch)))
                    })
                    .test_support()
            }));
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
                            4 + i,
                            card.clone(),
                        )
                    },
                ))
                .child(enter(
                    key("new-workspace"),
                    4 + places,
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
                        .text_color(p.faint)
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
                            Button::primary("empty-new-session", "New session in main")
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
        // The detail of a run slides in over the page, which keeps its scroll beneath it.
        let run_drawer = self.tasks.read(cx).drawer().clone();
        let page = div()
            .id("project-page")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(980.) + pad * 2.)
                    .w_full()
                    .mx_auto()
                    .px(pad)
                    .pt(space::XXL)
                    .pb(space::XXXL)
                    .flex()
                    .flex_col()
                    .child(enter(key("header"), 0, header))
                    .child(enter(key("repositories"), 1, chips))
                    .child(enter(
                        key("composer"),
                        2,
                        div().mb(px(34.)).child(self.composer.clone()),
                    ))
                    .when(self.empty, |d| d.child(enter(key("empty"), 3, nothing)))
                    .when(!self.empty, |d| {
                        d.child(enter(
                            key("places"),
                            3,
                            div()
                                .flex()
                                .items_center()
                                .gap(space::SM)
                                .mb(px(12.))
                                .text_size(px(13.))
                                .semibold()
                                .child("Where the work happens")
                                .child(
                                    div()
                                        .mono(11.)
                                        .font_weight(gpui_kit::FontWeight::NORMAL)
                                        .text_color(p.faint)
                                        .child(places.to_string()),
                                ),
                        ))
                        .child(grid)
                    })
                    // The repositories are configured in the project settings; the page only
                    // reads them (the new workspace form's bases).
                    .child(enter(
                        key("tasks"),
                        6 + places,
                        div().child(self.tasks.clone()),
                    )),
            );
        div().size_full().relative().child(page).child(run_drawer)
    }
}

/// What a card's own action is doing, while the daemon answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Removing,
    Preparing,
}

/// A place's card: its name, where it started from and what it holds, its preparation, its
/// sessions, and "New session in …".
pub struct PlaceCard {
    shell: WeakEntity<Shell>,
    project: String,
    data: PlaceData,
    /// Lines added and removed, once the daemon said.
    changes: Option<Changes>,
    /// Its repositories' pull requests, once the daemon said.
    pulls: Vec<PullRequest>,
    /// Where each of its repositories stands, once the daemon said.
    statuses: Vec<RepositoryStatus>,
    confirming: bool,
    busy: Option<Busy>,
    error: Option<SharedString>,
    icons: HashMap<String, Entity<StateIcon>>,
    preparing: Entity<StateIcon>,
    failed: Entity<Pill>,
    shown_agos: Vec<String>,
    renders: usize,
}

impl PlaceCard {
    fn new(
        shell: WeakEntity<Shell>,
        project: String,
        data: PlaceData,
        cx: &mut Context<Self>,
    ) -> Self {
        let running = cx.new(|_| Status::new(AgentState::Running));
        let failed = cx.new(|_| Status::new(AgentState::Failed));
        let mut card = Self {
            shell,
            project,
            data: PlaceData {
                sessions: Vec::new(),
                ..data.clone()
            },
            changes: None,
            pulls: Vec::new(),
            statuses: Vec::new(),
            confirming: false,
            busy: None,
            error: None,
            icons: HashMap::new(),
            preparing: cx.new(|cx| StateIcon::new(running, cx)),
            failed: cx.new(|cx| Pill::new(failed, cx)),
            shown_agos: Vec::new(),
            renders: 0,
        };
        card.set(data, cx);
        card
    }

    pub fn data(&self) -> &PlaceData {
        &self.data
    }

    pub fn changes(&self) -> Option<Changes> {
        self.changes
    }

    pub fn pulls(&self) -> &[PullRequest] {
        &self.pulls
    }

    pub fn statuses(&self) -> &[RepositoryStatus] {
        &self.statuses
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
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
        // A preparation started again clears what failed before.
        if self.data.preparation != data.preparation && self.busy == Some(Busy::Preparing) {
            self.busy = None;
        }
        self.data = data;
        cx.notify();
    }

    /// What the daemon answered to this card's action.
    pub fn answered(&mut self, refused: Option<&str>, cx: &mut Context<Self>) {
        self.busy = None;
        self.confirming = false;
        self.error = refused.map(|why| SharedString::from(why.to_owned()));
        cx.notify();
    }

    fn agos(&self, now: u64) -> Vec<String> {
        self.data
            .sessions
            .iter()
            .filter(|s| s.mark.is_none())
            .map(|s| ago(now, s.active_at))
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
        self.error = None;
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }
}

impl Render for PlaceCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let now = now_ms();
        self.shown_agos = self.agos(now);
        let place = self.data.place.clone();
        let name = place_name(&place).to_owned();
        let ready = matches!(self.data.preparation, Preparation::Ready);
        let id = |part: &str| ElementId::Name(format!("{part}-{name}").into());

        let mut title = div()
            .flex_1()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(
                icon(if place.is_none() {
                    Icon::Folder
                } else {
                    Icon::Branch
                })
                .size(px(14.))
                .text_color(p.faint),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .semibold()
                    .child(SharedString::from(name.clone())),
            );
        if place.is_none() {
            title = title.child(
                div()
                    .mono(10.5)
                    .text_color(p.faint)
                    .border_1()
                    .border_color(p.line2)
                    .rounded(px(5.))
                    .px(space::XS)
                    .child("PROJECT FOLDER"),
            );
        }
        title = title.child(div().flex_1());
        if let Some(Changes {
            added,
            removed,
            capped,
        }) = self.changes
        {
            // Capped: the daemon stopped counting new files, there are more.
            let at_least = if capped { "≥" } else { "" };
            title = title.child(
                div()
                    .id(id("changes"))
                    .flex_none()
                    .flex()
                    .gap(space::SM)
                    .mono(11.5)
                    .font_weight(gpui_kit::FontWeight::NORMAL)
                    .child(div().text_color(p.ok).child(format!("{at_least}+{added}")))
                    .child(div().text_color(p.fail).child(format!("−{removed}"))),
            );
        }
        if place.is_some() && self.busy.is_none() {
            title = title.child(Button::quiet(id("remove"), "Remove").on_click(cx.listener(
                |this, _, _, cx| {
                    this.confirming = true;
                    this.error = None;
                    cx.notify();
                },
            )));
        }

        let meta = self.data.meta.clone().map(|meta| {
            div()
                .id(id("meta"))
                .px(space::LG)
                .pl(space::XXL)
                .pb(space::MD)
                .mono(11.5)
                .text_color(p.faint)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(meta))
                .test_support()
        });

        // Where each repository stands; it unfolds once the daemon said.
        let statuses =
            presence(
                id("repositories-status"),
                !self.statuses.is_empty(),
                div()
                    .px(space::LG)
                    .pl(space::XXL)
                    .pb(space::MD)
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .children(self.statuses.iter().map(|status| {
                        let label = repository_label(&self.project, &status.path);
                        div()
                            .id(ElementId::Name(
                                format!("repository-status-{name}-{label}").into(),
                            ))
                            .flex()
                            .flex_col()
                            .mono(11.)
                            .child(
                                div()
                                    .flex()
                                    .gap(space::SM)
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(p.mute)
                                            .child(SharedString::from(label.clone())),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_color(p.faint)
                                            .child(SharedString::from(status.line())),
                                    ),
                            )
                            .children(status.note.clone().map(|note| {
                                div().text_color(p.mute).child(SharedString::from(note))
                            }))
                            .aria_label(SharedString::from(match &status.note {
                                Some(note) => format!("{label} {} · {note}", status.line()),
                                None => format!("{label} {}", status.line()),
                            }))
                            .test_support()
                    })),
            );

        // Each repository's pull request, opened in the browser on a click.
        let pulls = (!self.pulls.is_empty()).then(|| {
            let project = self.project.clone();
            div()
                .px(space::LG)
                .pl(space::XXL)
                .pb(space::MD)
                .flex()
                .flex_wrap()
                .gap(space::XS)
                .children(self.pulls.iter().map(|pull| {
                    let (shell, url) = (self.shell.clone(), pull.url.clone());
                    let label = repository_label(&project, &pull.repository);
                    let row = pull_row(
                        ElementId::Name(format!("pull-{name}-{label}").into()),
                        pull,
                        cx,
                    )
                    .child(div().text_color(p.faint).child(SharedString::from(label)))
                    .on_click(move |_, _, cx| {
                        shell.update(cx, |shell, _| shell.open_link(&url)).ok();
                    });
                    pull_hover(row, cx)
                }))
        });

        let status = match &self.data.preparation {
            Preparation::Preparing { done, total, doing } => Some(
                div()
                    .id(id("preparing"))
                    .px(space::LG)
                    .pb(space::MD)
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .mono(11.5)
                    .text_color(p.mute)
                    .child(self.preparing.clone())
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(format!(
                                "Preparing · step {} of {total} · {doing}",
                                done + 1
                            ))),
                    )
                    .into_any_element(),
            ),
            Preparation::Failed { doing, output } => Some(
                div()
                    .id(id("failure"))
                    .px(space::LG)
                    .pb(space::MD)
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .child(self.failed.clone())
                            .child(
                                div()
                                    .min_w_0()
                                    .mono(11.5)
                                    .text_color(p.mute)
                                    .child(SharedString::from(doing.clone())),
                            ),
                    )
                    .child(
                        div()
                            .id(id("failure-output"))
                            .max_h(px(180.))
                            .overflow_y_scroll()
                            .p(space::MD)
                            .rounded(px(8.))
                            .bg(p.del)
                            .mono(11.5)
                            .text_color(p.text)
                            .child(SharedString::from(output.clone()))
                            .test_support(),
                    )
                    .child(div().flex().justify_end().when(self.busy.is_none(), |d| {
                        d.child(Button::secondary(id("retry"), "Prepare again").on_click(
                            cx.listener(|this, _, _, cx| this.perform(Busy::Preparing, cx)),
                        ))
                    }))
                    .into_any_element(),
            ),
            Preparation::Ready => None,
        };

        let busy = self.busy.map(|busy| {
            div()
                .px(space::LG)
                .pb(space::MD)
                .mono(11.5)
                .text_color(p.faint)
                .child(match busy {
                    Busy::Removing => "Removing…",
                    Busy::Preparing => "Preparing again…",
                })
        });
        let error = self.error.clone().map(|error| {
            div()
                .id(id("card-error"))
                .px(space::LG)
                .pb(space::MD)
                .text_size(px(12.5))
                .text_color(p.fail)
                .child(error)
        });

        let rows = self.data.sessions.iter().map(|session| {
            let shell = self.shell.clone();
            let open = session.id.clone();
            let trailing = match session.mark {
                Some(AgentState::NeedsYou) => {
                    div().mono(11.5).text_color(p.wait).child("needs you")
                }
                Some(_) => div().mono(11.5).text_color(p.faint).child("running"),
                None => div()
                    .mono(11.5)
                    .text_color(p.faint)
                    .child(ago(now, session.active_at)),
            };
            ListRow::new(
                ElementId::Name(format!("card-session-{}", session.id).into()),
                SharedString::from(session.title.clone()),
            )
            .when_some(self.icons.get(&session.id).cloned(), |row, icon| {
                row.leading(icon)
            })
            .meta(SharedString::from(agents_label(session.agents)))
            .trailing(div().min_w(px(64.)).flex().justify_end().child(trailing))
            .on_click(move |_, window, cx| {
                let session = open.clone();
                shell
                    .update(cx, |shell, cx| {
                        shell.open(Selection::Session(session), window, cx)
                    })
                    .ok();
            })
        });
        let (shell, project) = (self.shell.clone(), self.project.clone());
        let choice = match &place {
            None => Choice::Main,
            Some(name) => Choice::Workspace(name.clone()),
        };
        let add = div()
            .id(id("new-session-in"))
            .w_full()
            .flex()
            .items_center()
            .gap(space::SM)
            .px(space::LG)
            .py(space::SM)
            .border_t_1()
            .border_color(p.line)
            .rounded_b(px(12.))
            .text_size(px(12.5))
            .when(ready, |d| {
                d.on_click(move |_, window, cx| {
                    let choice = choice.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.compose_in(&project, choice, window, cx)
                        })
                        .ok();
                })
            })
            .when(!ready, |d| d.opacity(0.5))
            .child(icon(Icon::Plus).size(px(12.)).text_color(p.faint))
            .child(SharedString::from(format!("New session in {name}")));
        let add = hoverable(add, move |d, t| {
            let t = if ready { t } else { 0.0 };
            d.bg(p.hover.opacity(t)).text_color(mix(p.faint, p.text, t))
        });

        let confirm = {
            let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
            Confirm::new(
                id("workspace-removal"),
                format!("Remove the workspace {name}?"),
            )
            .sentence(
                "Its folder goes, and each repository's branch that holds no commit of its \
                     own. Flottille removes it only once all its work is committed and pushed.",
            )
            .action("Remove workspace")
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
        div()
            .id(card_id(&place))
            .h_full()
            .child(confirm)
            .child(
                Card::new()
                    .title(title)
                    .children_opt(meta.map(|meta| div().pt(space::MD).child(meta)))
                    .child(statuses)
                    .children_opt(pulls)
                    .children_opt(status)
                    .children_opt(busy)
                    .children_opt(error)
                    .child(
                        div()
                            .border_t_1()
                            .border_color(p.line)
                            .px(space::SM)
                            .py(space::XS)
                            .flex()
                            .flex_col()
                            .children(rows),
                    )
                    .child(add),
            )
            .test_support()
    }
}

trait CardExt {
    fn children_opt(self, child: Option<impl IntoElement>) -> Self;
}

impl CardExt for Card {
    fn children_opt(self, child: Option<impl IntoElement>) -> Self {
        match child {
            Some(child) => self.child(child),
            None => self,
        }
    }
}

const OFFLINE: &str = "Offline: fetch nothing, branch from what is here";

/// How long typing rests before the ticket typed is read.
const PREVIEW_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

/// The last cell of the grid: a dashed "New workspace" card, which opens the new-workspace modal:
/// its name, its ticket (read as it is typed), the repositories it takes, its branch, whether it
/// fetches, and a button that creates it, as `flottille workspace create` does.
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
    offline: bool,
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
            let mut name = InputState::new(window, cx).placeholder("name, for example login-fix");
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
            let mut ticket =
                InputState::new(window, cx).placeholder("ticket (optional): #123 or ABC-123");
            ticket.set_editor_style(gpui_kit::base::input::InputEditorStyle {
                foreground: p.text,
                muted_foreground: p.faint,
                selection: p.run.opacity(0.35),
                caret: p.text,
                ..Default::default()
            });
            ticket
        });
        let branch = cx.new(|cx| {
            crate::project_settings::styled_input("branch (optional): prefix/name", window, cx)
        });
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
            offline: false,
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

    pub fn offline(&self) -> bool {
        self.offline
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

    pub fn toggle_offline(&mut self, cx: &mut Context<Self>) {
        self.offline = !self.offline;
        cx.notify();
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
            offline: self.offline,
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
                self.offline = false;
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
            .min_h(px(150.))
            .h_full()
            .p(space::XL)
            .rounded(px(12.))
            .border_1()
            .border_dashed()
            .border_color(p.line2)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_size(px(13.))
            .child(icon(Icon::Plus).size(px(18.)).text_color(p.faint))
            .child(
                div()
                    .mt(px(6.))
                    .mb(px(2.))
                    .text_color(p.text)
                    .medium()
                    .child("New workspace"),
            )
            .child("a branch of its own in every repository")
            .on_click(cx.listener(|this, _, window, cx| this.open(window, cx)));
        let creating = self.creating.is_some();
        let this = cx.entity().downgrade();
        let modal = Modal::new("new-workspace-modal", "New workspace")
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            })
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child("A branch of its own in each repository it takes."),
            )
            .child(
                div()
                    .id("new-workspace-modal-name")
                    .h(px(32.))
                    .px(space::MD)
                    .rounded(px(8.))
                    .bg(p.field)
                    .border_1()
                    .border_color(p.focus_line)
                    .flex()
                    .items_center()
                    .mono(12.5)
                    .child(Input::new(&self.name))
                    .test_support(),
            )
            .child(
                div()
                    .id("new-workspace-modal-ticket")
                    .h(px(32.))
                    .px(space::MD)
                    .rounded(px(8.))
                    .bg(p.field)
                    .border_1()
                    .border_color(p.line2)
                    .flex()
                    .items_center()
                    .mono(12.5)
                    .child(Input::new(&self.ticket))
                    .test_support(),
            )
            .children(self.preview_line(cx))
            .child({
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
                            .w(px(130.))
                            .h(px(28.))
                            .px(space::SM)
                            .rounded(px(6.))
                            .border_1()
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .mono(11.5)
                            // By shape as well as colour: a tick when it is taken.
                            .child(if taken { "✓" } else { "·" })
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(repository_label(
                                        &self.project,
                                        &path,
                                    ))),
                            )
                            .when(several, |chip| {
                                chip.on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_repository(&toggled, cx)
                                }))
                            });
                        let chip = selectable(chip, taken, move |d, hover, taken| {
                            d.bg(mix(p.hover.opacity(hover), p.card, taken))
                                .border_color(mix(p.line, p.focus_line, taken))
                                .text_color(mix(p.faint, p.text, taken))
                        });
                        let field = div()
                            .id(ElementId::Name(format!("new-workspace-base-{path}").into()))
                            .h(px(28.))
                            .px(space::MD)
                            .rounded(px(8.))
                            .bg(p.field)
                            .border_1()
                            .border_color(p.line2)
                            .flex()
                            .items_center()
                            .mono(12.)
                            .child(Input::new(&base))
                            .test_support();
                        div().flex().items_start().gap(space::SM).child(chip).child(
                            div().flex_1().min_w_0().child(presence(
                                ElementId::Name(format!("new-workspace-base-{path}-block").into()),
                                taken,
                                field,
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
            })
            .child(
                div()
                    .id("new-workspace-modal-branch")
                    .h(px(32.))
                    .px(space::MD)
                    .rounded(px(8.))
                    .bg(p.field)
                    .border_1()
                    .border_color(p.line2)
                    .flex()
                    .items_center()
                    .mono(12.5)
                    .child(Input::new(&self.branch))
                    .test_support(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(
                        Switch::new("new-workspace-offline", self.offline, OFFLINE)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_offline(cx))),
                    ),
            )
            .child(presence_of(
                "new-workspace-error-block",
                self.error.clone(),
                move |error| {
                    div()
                        .id("new-workspace-error")
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error)
                        .into_any_element()
                },
            ))
            .action(if creating {
                div()
                    .mono(11.5)
                    .text_color(p.faint)
                    .child("Creating…")
                    .into_any_element()
            } else {
                Button::primary("create-workspace", "Create")
                    .disabled(!self.has_name)
                    .on_click(cx.listener(|this, _, _, cx| this.create(cx)))
                    .into_any_element()
            });
        if !self.framed {
            return div()
                .id("new-workspace-anchor")
                .child(modal)
                .into_any_element();
        }
        hoverable(card.child(modal), move |d, t| {
            d.bg(p.hover.opacity(t)).text_color(mix(p.faint, p.text, t))
        })
        .into_any_element()
    }
}
