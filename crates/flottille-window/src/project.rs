//! A project's page: it shows and runs, it configures nothing (that is its Settings). Its header
//! (the name and folder, a read-only chip per repository with the branch it is on, which opens its
//! row in the settings, Settings and New workspace), the composer that starts a session, then its
//! tabs: **Work**, one column of the project's places (`main` first, then the workspaces from the
//! newest), each with its sessions and the task runs going on there, the ended runs folded at the
//! bottom; **Pull requests**, **Tickets**, and **Specs** while Specs is on. Each place is its own
//! view: a session changing redraws its block, a letter typed in the composer redraws neither.

mod ended;
mod place;

pub use ended::EndedRuns;
pub use place::{PlaceBlock, block_id, session_end_id, session_menu_id, session_row_id};

use std::collections::HashMap;

use gpui_design_system::color::mix;
use gpui_design_system::motion::{Motion, PANEL};
use gpui_design_system::{
    Button, Chip, Field, Icon, Modal, PageHeader, Tabs, Toast, Type, changing, enter, fade,
    follow_theme, icon, palette, presence, selectable, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, point, px,
};

use crate::actions::{Command, Details, RepositoryStatus};
use crate::composer::Composer;
use crate::items::{Kind, ProjectItems};
use crate::model::{Place, Preparation};
use crate::project_settings::Reveal;
use crate::repositories::ProjectRepositories;
use crate::runs::{CommandRow, arrange};
use crate::shell::{Selection, Shell};
use crate::tasks::{ProjectTasks, TaskInfo};

/// A session as its place lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionLine {
    pub id: String,
    pub title: String,
    pub agents: usize,
    pub mark: Option<crate::state::AgentState>,
    pub active_at: u64,
    /// The state its row in the tree reads: one state, held once.
    pub status: Entity<crate::state::Status>,
    /// The sessions it leads, each with its place.
    pub leads: Vec<(String, Place)>,
    /// The session that leads it, and its title.
    pub pilot: Option<(String, String)>,
    /// What it asks the user, while it waits for them: the question or the request.
    pub question: Option<String>,
}

/// What a place's block shows.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceData {
    pub place: Place,
    /// `from develop`; `main` has none, its name says what it is.
    pub meta: Option<String>,
    /// Its folder, which "Open in editor" opens.
    pub folder: String,
    /// `Ready` for `main`.
    pub preparation: Preparation,
    /// Each step of its preparation; none for `main`.
    pub steps: Vec<crate::model::SetupLine>,
    pub sessions: Vec<SessionLine>,
}

/// What the page shows of its project, as the shell's read gives it: the places in the order
/// Work lists them, `main` first, then the workspaces from the newest.
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

/// Whether the project's folder is its only repository: then the repositories are not "one among
/// several", and the screens that name the one a thing happens in have nothing to name.
pub fn only_at_root(repositories: &[String]) -> bool {
    matches!(repositories, [only] if only == ".")
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

/// The tab of the page shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageTab {
    Work,
    /// One of the lists: which one is the items' own.
    Lists,
}

/// What the tabs say of the lists, kept so that the page is drawn again only when it changes.
#[derive(Clone, Debug, Default, PartialEq)]
struct ListsSaid {
    counts: [usize; 2],
    specs: Option<usize>,
    chosen: Option<Kind>,
}

pub struct ProjectPage {
    shell: WeakEntity<Shell>,
    name: String,
    folder: String,
    repositories: Vec<String>,
    branches: Vec<(String, Option<String>)>,
    blocks: Vec<Entity<PlaceBlock>>,
    /// The place of each block, kept here: reading a block while drawing the page would draw the
    /// page again each time the block changes.
    places: Vec<Place>,
    /// The blocks as drawn: one that comes unfolds, one that goes folds away.
    place_rows: gpui_design_system::Listing<Place>,
    composer: Entity<Composer>,
    new_workspace: Entity<NewWorkspace>,
    /// Its tasks and runs, and the run drawer.
    tasks: Entity<ProjectTasks>,
    ended: Entity<EndedRuns>,
    /// Its repositories as the daemon lists them, read for the new-workspace form's rules; they
    /// are listed and changed in the settings.
    repositories_section: Entity<ProjectRepositories>,
    /// Its pull requests and tickets.
    items: Entity<ProjectItems>,
    tab: PageTab,
    /// A tab was chosen since the page opened: its content enters at once, not after the
    /// page's own entrance.
    switched: bool,
    lists: ListsSaid,
    narrow: bool,
    /// The page's scroll, which a "leads" link moves to a place, smoothly.
    scroll: ScrollHandle,
    scrolling: Option<Motion>,
    _subscriptions: Vec<Subscription>,
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
        let items = {
            let (shell, project) = (shell.clone(), name.clone());
            cx.new(|cx| ProjectItems::new(shell, project, cx))
        };
        let reading = items.clone();
        cx.defer(move |cx| reading.update(cx, |items, cx| items.read(cx)));
        let ended = {
            let tasks = tasks.downgrade();
            cx.new(|cx| EndedRuns::new(tasks, cx))
        };
        let subscriptions = vec![
            cx.observe(&tasks, |page, _, cx| page.runs_moved(cx)),
            cx.observe(&items, |page, _, cx| page.lists_moved(cx)),
        ];
        Self {
            shell,
            name,
            folder: String::new(),
            repositories: Vec::new(),
            branches: Vec::new(),
            blocks: Vec::new(),
            places: Vec::new(),
            place_rows: gpui_design_system::Listing::default(),
            composer,
            new_workspace,
            tasks,
            ended,
            repositories_section,
            items,
            tab: PageTab::Work,
            switched: false,
            lists: ListsSaid::default(),
            narrow: false,
            scroll: ScrollHandle::new(),
            scrolling: None,
            _subscriptions: subscriptions,
            renders: 0,
        }
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

    pub fn items(&self) -> &Entity<ProjectItems> {
        &self.items
    }

    pub fn ended(&self) -> &Entity<EndedRuns> {
        &self.ended
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    pub fn tab(&self) -> PageTab {
        self.tab
    }

    /// The places' blocks, in the order Work lists them.
    pub fn blocks(&self) -> &[Entity<PlaceBlock>] {
        &self.blocks
    }

    pub fn block(&self, place: &Place, cx: &gpui_kit::App) -> Option<Entity<PlaceBlock>> {
        self.blocks
            .iter()
            .find(|block| &block.read(cx).data().place == place)
            .cloned()
    }

    /// Shows Work, or the lists.
    pub fn choose(&mut self, tab: PageTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            cx.notify();
        }
    }

    /// Each repository and the branch it is on, once known: the header's chips, drawn for a
    /// project of several repositories.
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

    /// Brings the page up to `data`; a block whose data did not change is not drawn again.
    pub fn set(&mut self, data: ProjectData, cx: &mut Context<Self>) {
        let mut changed = self.folder != data.folder || self.repositories != data.repositories;
        self.folder = data.folder;
        self.repositories = data.repositories;
        let mut old = std::mem::take(&mut self.blocks);
        let mut blocks = Vec::new();
        let places: Vec<Place> = data.places.iter().map(|p| p.place.clone()).collect();
        changed |= self.places != places;
        self.places = places;
        for place in data.places {
            let at = old
                .iter()
                .position(|block| block.read(cx).data().place == place.place);
            let block = match at {
                Some(at) => old.remove(at),
                None => {
                    let (shell, page) = (self.shell.clone(), cx.entity().downgrade());
                    let (project, tasks) = (self.name.clone(), self.tasks.downgrade());
                    let first = place.clone();
                    cx.new(|cx| PlaceBlock::new(shell, page, project, tasks, first, cx))
                }
            };
            if block.read(cx).data() != &place {
                block.update(cx, |block, cx| block.set(place, cx));
            }
            blocks.push(block);
        }
        self.blocks = blocks;
        self.runs_moved(cx);
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
        let several = !only_at_root(&self.repositories);
        let mut in_workspace = HashMap::new();
        for block in &self.blocks {
            let Some(name) = block.read(cx).data().place.clone() else {
                continue;
            };
            let facts = place::Facts {
                changes: details.changes.get(&name).copied(),
                pulls: details.pulls.get(&name).cloned().unwrap_or_default(),
                statuses: details.statuses.get(&name).cloned().unwrap_or_default(),
                ticket: details.tickets.get(&name).cloned(),
                pulls_pending: details.pending_pulls.contains(&name),
                ticket_pending: details.pending_tickets.contains(&name),
            };
            for pull in &facts.pulls {
                in_workspace.insert(
                    (pull.repository.clone(), pull.number.to_string()),
                    name.clone(),
                );
            }
            block.update(cx, |block, cx| block.set_facts(facts, several, cx));
        }
        self.items
            .update(cx, |items, cx| items.set_workspaces(in_workspace, cx));
    }

    /// The tasks or the runs changed: each place gets the task runs going on there, and the
    /// fold the last ones that ended. The page itself is not drawn again.
    fn runs_moved(&mut self, cx: &mut Context<Self>) {
        let tasks = self.tasks.read(cx);
        let listed: Vec<TaskInfo> = tasks.tasks().map(<[_]>::to_vec).unwrap_or_default();
        let rows = tasks.commands().read(cx).rows().to_vec();
        let arranged = arrange(&rows);
        let running: Vec<CommandRow> = arranged.running.into_iter().cloned().collect();
        let ended: Vec<CommandRow> = arranged.kept.into_iter().cloned().collect();
        for (block, place) in self.blocks.iter().zip(&self.places) {
            let here: Vec<CommandRow> = running
                .iter()
                .filter(|row| &row.workspace == place)
                .cloned()
                .collect();
            block.update(cx, |block, cx| {
                block.set_tasks(&listed, cx);
                block.set_runs(here, cx);
            });
        }
        self.ended.update(cx, |view, cx| view.set(ended, cx));
    }

    /// What the lists say changed: the tabs are drawn again only then.
    fn lists_moved(&mut self, cx: &mut Context<Self>) {
        let items = self.items.read(cx);
        let said = ListsSaid {
            counts: items.counts(),
            specs: items.specs_count(cx),
            chosen: items.chosen(cx),
        };
        if self.lists != said {
            self.lists = said;
            cx.notify();
        }
    }

    /// Scrolls Work smoothly to the block of `place`, which is highlighted for a moment.
    pub fn scroll_to(&mut self, place: &Place, cx: &mut Context<Self>) {
        let Some(block) = self.block(place, cx) else {
            return;
        };
        if let Some(at) = block.read(cx).bounds() {
            let frame = self.scroll.bounds();
            let offset = self.scroll.offset();
            let max = self.scroll.max_offset();
            let target = (offset.y - (at.origin.y - frame.origin.y) + space::XL)
                .min(px(0.))
                .max(-max.y);
            let now = cx.background_executor().now();
            let mut motion = Motion::settled(f32::from(offset.y));
            motion.animate(f32::from(target), PANEL, now, cx.reduce_motion());
            self.scrolling = Some(motion);
        }
        block.update(cx, |block, cx| block.highlight(cx));
        cx.notify();
    }

    /// The times move on: a block is drawn again only when one of its texts changes.
    pub fn tick(page: &Entity<Self>, now: u64, cx: &mut gpui_kit::App) {
        for block in page.read(cx).blocks.clone() {
            if block.read(cx).times_changed(now) {
                block.update(cx, |_, cx| cx.notify());
            }
        }
    }
}

impl Render for ProjectPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        // A "leads" link moves the page to a place over `PANEL`.
        if let Some(motion) = self.scrolling {
            let now = cx.background_executor().now();
            let offset = self.scroll.offset();
            self.scroll
                .set_offset(point(offset.x, px(motion.value(now))));
            match motion.is_running(now) {
                true => window.request_animation_frame(),
                false => self.scrolling = None,
            }
        }
        let pad = if self.narrow { space::LG } else { space::XXL };
        let key = |part: &str| ElementId::Name(format!("enter-{}-{part}", self.name).into());
        let new_workspace = self.new_workspace.clone();
        // A project of one repository has no chip: its folder is its repository.
        let several = self.repositories.len() > 1;
        let chips =
            self.chip_parts()
                .into_iter()
                .filter(|_| several)
                .map(|(path, label, branch)| {
                    let (shell, project, at) =
                        (self.shell.clone(), self.name.clone(), path.clone());
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

        // The tabs: Work, the two lists, and Specs while it is on.
        let chosen = match (self.tab, self.lists.chosen) {
            (PageTab::Work, _) => 0,
            (PageTab::Lists, Some(Kind::Pulls)) => 1,
            (PageTab::Lists, Some(Kind::Tickets)) => 2,
            (PageTab::Lists, None) => 3,
        };
        let this = cx.entity().downgrade();
        let items = self.items.clone();
        let tabs = Tabs::new("project-tabs", "Project", chosen)
            .tab("work", "Work")
            .counted("pulls", "Pull requests", self.lists.counts[0].to_string())
            .counted("tickets", "Tickets", self.lists.counts[1].to_string())
            .when_some(self.lists.specs, |tabs, count| {
                tabs.counted("specs", "Specs", count.to_string())
            })
            .on_choose(move |at, _, cx| {
                let tab = if at == 0 {
                    PageTab::Work
                } else {
                    PageTab::Lists
                };
                match at {
                    1 => items.update(cx, |items, cx| items.choose(Kind::Pulls, cx)),
                    2 => items.update(cx, |items, cx| items.choose(Kind::Tickets, cx)),
                    3 => items.update(cx, |items, cx| items.choose_specs(cx)),
                    _ => {}
                }
                this.update(cx, |page, cx| {
                    page.switched = true;
                    page.choose(tab, cx);
                })
                .ok();
            });

        let shown = match self.tab {
            PageTab::Work => {
                let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
                let places: Vec<(String, Place)> = self
                    .places
                    .iter()
                    .map(|place| (crate::tree::place_name(place).to_owned(), place.clone()))
                    .collect();
                self.place_rows.sync(places, at, reduce);
                let blocks: Vec<_> = self
                    .place_rows
                    .rows()
                    .filter_map(|row| {
                        let at = self.places.iter().position(|p| p == row.value)?;
                        Some((row.key.to_owned(), self.blocks[at].clone(), row.placement))
                    })
                    .collect();
                div()
                    .id("project-work")
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .children(blocks.into_iter().map(|(key, block, placement)| {
                        gpui_design_system::Item::new(
                            ElementId::Name(format!("place-item-{key}").into()),
                            placement,
                            block,
                        )
                        .gap(space::SM)
                    }))
                    .child(self.ended.clone())
                    .test_support()
                    .into_any_element()
            }
            PageTab::Lists => self.items.clone().into_any_element(),
        };
        let shown_key = match chosen {
            0 => "work",
            1 => "pulls",
            2 => "tickets",
            _ => "specs",
        };
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::LG)
                    .child(enter(key("tabs"), 2, tabs))
                    // The tab's content enters again each time it is chosen.
                    .child(enter(
                        key(shown_key),
                        if self.switched { 0 } else { 3 },
                        shown,
                    )),
            )
            // The form's dialog stays in the tree whatever the tab.
            .child(self.new_workspace.clone());
        div()
            .id("project-page-frame")
            .relative()
            .size_full()
            .child(
                div()
                    .id("project-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(column),
            )
            // The detail of a run slides in over the page, which keeps its scroll beneath it.
            .child(self.tasks.read(cx).drawer().clone())
    }
}

/// Why a workspace cannot be removed now, when it cannot: the sessions not ended that work in it
/// (`living`, by title), which Flottille never takes the place of, and the files not committed,
/// which it never forces. The daemon refuses the rest (commits not pushed) and says why. The card
/// and the session's rail ask the same.
pub(crate) fn removal_refused(
    project: &str,
    statuses: &[RepositoryStatus],
    living: &[String],
) -> Option<SharedString> {
    let mut why = Vec::new();
    if !living.is_empty() {
        let titles: Vec<String> = living.iter().map(|title| format!("“{title}”")).collect();
        why.push(format!(
            "Sessions work in it: {}. End them first: Flottille never removes a workspace an \
             agent works in.",
            titles.join(", ")
        ));
    }
    let dirty: Vec<(String, u64)> = statuses
        .iter()
        .filter_map(|status| {
            let files = status.uncommitted.filter(|files| *files > 0)?;
            Some((repository_label(project, &status.path), files))
        })
        .collect();
    if !dirty.is_empty() {
        let files: u64 = dirty.iter().map(|(_, files)| files).sum();
        let places: Vec<String> = dirty.into_iter().map(|(label, _)| label).collect();
        let (count, verb) = match files {
            1 => ("1 file".to_owned(), "is"),
            files => (format!("{files} files"), "are"),
        };
        why.push(format!(
            "{count} {verb} not committed in {}. Flottille never forces: commit or discard them \
             first.",
            places.join(", ")
        ));
    }
    (!why.is_empty()).then(|| SharedString::from(why.join(" ")))
}

/// A text field's frame in a form: 34 px, the field colours.
pub(crate) fn input_frame(
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

/// How long typing rests before the ticket typed is read.
const PREVIEW_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

/// The new-workspace modal, which the header's New workspace and the composer's place open:
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
            let mut name = InputState::new(window, cx).placeholder("for example login-fix");
            follow_theme(&mut name, cx);
            name
        });
        let ticket = cx.new(|cx| {
            let mut ticket = InputState::new(window, cx).placeholder("#123 or ABC-123");
            follow_theme(&mut ticket, cx);
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

    /// What the daemon answered: made, and the dialog closes; or refused, and it says why. `true`
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
                        .detail("Its place on the project page lists the steps of its setup."),
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
        let creating = self.creating.is_some();
        let this = cx.entity().downgrade();
        let repositories = {
            // A row per repository: taken or not (when there is a choice), and its base.
            let several = self.repositories.len() > 1;
            // The folder is the only repository: there is nothing to tell it from.
            let alone = only_at_root(&self.repositories);
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
                    // The chip's width and ink follow whether it names anything.
                    let named = fade(
                        ElementId::Name(format!("new-workspace-take-{path}-named").into()),
                        !alone,
                        PANEL,
                        window,
                        cx,
                    );
                    div()
                        .flex()
                        .items_start()
                        .child(
                            div()
                                .flex_none()
                                .overflow_hidden()
                                .w((px(140.) + space::SM) * named)
                                .pr(space::SM)
                                .opacity(named)
                                .when(named > 0., |d| d.child(chip)),
                        )
                        .child(div().flex_1().min_w_0().child(presence(
                            ElementId::Name(format!("new-workspace-base-{path}-block").into()),
                            taken,
                            input_frame(
                                ElementId::Name(format!("new-workspace-base-{path}").into()),
                                &base,
                                cx,
                            ),
                        )))
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
                let label = if only_at_root(&self.repositories) {
                    "Base branch"
                } else {
                    "Repositories and their bases"
                };
                modal.child(changing("new-workspace-takes-block", label, move |label| {
                    Field::new("new-workspace-takes", label, repositories)
                        .hint("The base is the project's rule when left empty.")
                        .into_any_element()
                }))
            })
            .child(
                Field::new(
                    "new-workspace-branch",
                    "Branch",
                    input_frame("new-workspace-modal-branch", &self.branch, cx),
                )
                .optional()
                .hint("To resume a branch, name it."),
            )
            .action(
                Button::primary(
                    "create-workspace",
                    if creating { "Creating…" } else { "Create" },
                )
                .disabled(!self.has_name || creating)
                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
            );
        div().id("new-workspace-anchor").child(modal)
    }
}
