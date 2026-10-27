//! A project's page: its name and folder, a chip per repository with the branch it is on, the
//! composer that starts a session, and "Where the work happens": a card per place (`main` first,
//! then each workspace), ending with a card that makes a new workspace. Each card is its own view:
//! a session changing redraws its card, a letter typed in the composer redraws neither.

use std::collections::HashMap;

use flottille_ui::{
    AgentState, Button, Card, Icon, ListRow, Modal, Pill, StateIcon, Status, Type, enter, icon,
    palette,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Changes, Command, Details};
use crate::composer::{Choice, Composer, agents_label};
use crate::model::{Place, Preparation, ago};
use crate::procedures::ProceduresSection;
use crate::references::ReferencesSection;
use crate::session::pull_row;
use crate::session_link::PullRequest;
use crate::shell::{Selection, Shell, now_ms};
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
    procedures: Entity<ProceduresSection>,
    references: Entity<ReferencesSection>,
    narrow: bool,
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
        let procedures = cx.new(|_| ProceduresSection::new(shell.clone(), name.clone()));
        let references = cx.new(|_| ReferencesSection::new(shell.clone(), name.clone()));
        // Asked once the shell, which is making this page, is no longer being updated.
        let (listing, referencing) = (procedures.clone(), references.clone());
        cx.defer(move |cx| {
            listing.update(cx, |section, cx| section.read(cx));
            referencing.update(cx, |section, cx| section.read(cx));
        });
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
            procedures,
            references,
            narrow: false,
            renders: 0,
        }
    }

    pub fn procedures(&self) -> &Entity<ProceduresSection> {
        &self.procedures
    }

    pub fn references(&self) -> &Entity<ReferencesSection> {
        &self.references
    }

    pub fn name(&self) -> &str {
        &self.name
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
        self.references
            .update(cx, |section, _| section.set_folder(&data.folder));
        self.folder = data.folder;
        self.repositories = data.repositories;
        let mut old = std::mem::take(&mut self.cards);
        let mut cards = Vec::new();
        self.places = data.places.iter().map(|p| p.place.clone()).collect();
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
        let pad = if self.narrow { 16. } else { 28. };
        let key = |part: &str| ElementId::Name(format!("enter-{}-{part}", self.name).into());
        let new_workspace = self.new_workspace.clone();
        let header = div()
            .flex()
            .items_end()
            .gap(px(14.))
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
                    .pb(px(3.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .mono(12.)
                    .text_color(p.faint)
                    .child(SharedString::from(self.folder.clone())),
            )
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
            .gap(px(6.))
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
                    .px(px(8.))
                    .rounded(px(6.))
                    .bg(p.card)
                    .border_1()
                    .border_color(p.line)
                    .flex()
                    .items_center()
                    .gap(px(6.))
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
                .gap(px(12.))
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
        div()
            .id("project-page")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(980. + 2. * pad))
                    .w_full()
                    .mx_auto()
                    .px(px(pad))
                    .pt(px(36.))
                    .pb(px(56.))
                    .flex()
                    .flex_col()
                    .child(enter(key("header"), 0, header))
                    .child(enter(key("repositories"), 1, chips))
                    .child(enter(
                        key("composer"),
                        2,
                        div().mb(px(34.)).child(self.composer.clone()),
                    ))
                    .child(enter(
                        key("places"),
                        3,
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
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
                    .child(self.procedures.clone())
                    .child(self.references.clone()),
            )
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
            .gap(px(8.))
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
                    .px(px(5.))
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
                    .gap(px(6.))
                    .mono(11.5)
                    .font_weight(gpui_kit::FontWeight::NORMAL)
                    .child(div().text_color(p.ok).child(format!("{at_least}+{added}")))
                    .child(div().text_color(p.fail).child(format!("−{removed}"))),
            );
        }
        if place.is_some() && ready && self.busy.is_none() && !self.confirming {
            let (shell, project, workspace) =
                (self.shell.clone(), self.project.clone(), name.clone());
            title = title.child(
                Button::quiet(id("run-procedure"), "Run procedure…").on_click(move |_, _, cx| {
                    shell
                        .update(cx, |shell, cx| {
                            shell.run_procedure(&project, &workspace, cx)
                        })
                        .ok();
                }),
            );
        }
        if place.is_some() && self.busy.is_none() {
            title = if self.confirming {
                title
                    .child(
                        Button::quiet(id("cancel-remove"), "Cancel").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.confirming = false;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        Button::secondary(id("confirm-remove"), format!("Remove {name}")).on_click(
                            cx.listener(|this, _, _, cx| this.perform(Busy::Removing, cx)),
                        ),
                    )
            } else {
                title.child(Button::quiet(id("remove"), "Remove").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.confirming = true;
                        this.error = None;
                        cx.notify();
                    },
                )))
            };
        }

        let meta = self.data.meta.clone().map(|meta| {
            div()
                .id(id("meta"))
                .px(px(14.))
                .pl(px(36.))
                .pb(px(10.))
                .mono(11.5)
                .text_color(p.faint)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(meta))
                .test_support()
        });

        // Each repository's pull request, opened in the browser on a click.
        let pulls = (!self.pulls.is_empty()).then(|| {
            let project = self.project.clone();
            div()
                .px(px(14.))
                .pl(px(30.))
                .pb(px(10.))
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(self.pulls.iter().map(|pull| {
                    let (shell, url) = (self.shell.clone(), pull.url.clone());
                    let label = repository_label(&project, &pull.repository);
                    pull_row(
                        ElementId::Name(format!("pull-{name}-{label}").into()),
                        pull,
                        cx,
                    )
                    .child(div().text_color(p.faint).child(SharedString::from(label)))
                    .on_click(move |_, _, cx| {
                        shell.update(cx, |shell, _| shell.open_link(&url)).ok();
                    })
                    .test_support()
                }))
        });

        let status = match &self.data.preparation {
            Preparation::Preparing { done, total, doing } => Some(
                div()
                    .id(id("preparing"))
                    .px(px(14.))
                    .pb(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
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
                    .px(px(14.))
                    .pb(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
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
                            .p(px(10.))
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
                .px(px(14.))
                .pb(px(10.))
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
                .px(px(14.))
                .pb(px(10.))
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
            .gap(px(8.))
            .px(px(14.))
            .py(px(9.))
            .border_t_1()
            .border_color(p.line)
            .rounded_b(px(12.))
            .text_size(px(12.5))
            .text_color(p.faint)
            .when(ready, |d| {
                d.hover(|d| d.bg(p.hover).text_color(p.text))
                    .on_click(move |_, window, cx| {
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
            .child(SharedString::from(format!("New session in {name}")))
            .test_support();

        div()
            .id(card_id(&place))
            .h_full()
            .child(
                Card::new()
                    .title(title)
                    .children_opt(meta.map(|meta| div().pt(px(10.)).child(meta)))
                    .children_opt(pulls)
                    .children_opt(status)
                    .children_opt(busy)
                    .children_opt(error)
                    .child(
                        div()
                            .border_t_1()
                            .border_color(p.line)
                            .px(px(6.))
                            .py(px(4.))
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

/// The last cell of the grid: a dashed "New workspace" card, which opens the new-workspace modal:
/// its name and a button that creates it.
pub struct NewWorkspace {
    shell: WeakEntity<Shell>,
    project: String,
    open: bool,
    name: Entity<InputState>,
    has_name: bool,
    /// The ticket it is linked to, if one is typed: `#123` or a Jira key.
    ticket: Entity<InputState>,
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
        let subscriptions = vec![
            cx.subscribe_in(&ticket, window, |this, _, event, _, cx| {
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
        let command = Command::CreateWorkspace {
            project: self.project.clone(),
            name,
            ticket: (!ticket.is_empty()).then_some(ticket),
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
            }
            Some(why) => self.error = Some(SharedString::from(why.to_owned())),
        }
        cx.notify();
        for_composer
    }
}

impl Render for NewWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let card = div()
            .id("new-workspace-card")
            .min_h(px(150.))
            .h_full()
            .p(px(22.))
            .rounded(px(12.))
            .border_1()
            .border_dashed()
            .border_color(p.line2)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_color(p.faint)
            .text_size(px(13.))
            .hover(|d| d.bg(p.hover).text_color(p.text))
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
            .on_click(cx.listener(|this, _, window, cx| this.open(window, cx)))
            .test_support();
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
                    .child("A branch of its own in every repository of the project."),
            )
            .child(
                div()
                    .id("new-workspace-modal-name")
                    .h(px(32.))
                    .px(px(10.))
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
                    .px(px(10.))
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
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("new-workspace-error")
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error),
                )
            })
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
        card.child(modal)
    }
}
