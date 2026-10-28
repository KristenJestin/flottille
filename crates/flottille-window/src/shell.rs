//! The window's frame: the top bar (the mark, search, the bell), the left tree, and the main area
//! showing the open page. It follows the daemon through the [`link`](crate::link) and draws
//! again only the parts a change touches.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flottille_ui::{
    AgentState, Badge, Button, Drawer, Icon, ListRow, Side, StateIcon, Status, Type,
    dismiss_on_navigate, dismiss_top_overlay, forget_overlays, icon, open_overlay, palette,
};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, ElementId, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, Modifiers, MouseButton, NavigationDirection, ParentElement,
    PathBuilder, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Task, WeakEntity, Window, canvas, div, point, px,
};

use crate::actions::{Actions, Command, Outcome, Pick};
use crate::add_project::AddProject;
use crate::composer::{Choice, Composer, PlaceChoice};
use crate::history::{History, Way};
use crate::link::{self, Connect, Seen, Update};
use crate::model::{BellItem, Place, Preparation, Snapshot};
use crate::open::Opener;
use crate::page::{MainArea, Page};
use crate::procedures::RunProcedure;
use crate::project::{NewWorkspace, PlaceData, ProjectData, SessionLine, repository_label};
use crate::project_settings::ProjectSettingsPage;
use crate::providers::Providers;
use crate::session::{Chose, Follow, Opened, Reply, SessionPage};
use crate::settings::SettingsPage;
use crate::tree::{PlaceNode, ProjectNode, SessionRow, Sidebar};

/// Below this width the session page's rail becomes a drawer from the right.
pub const WIDE: f32 = 1240.;
/// Below this width the left tree becomes a drawer from the left.
pub const NARROW: f32 = 860.;
const SIDEBAR: f32 = 280.;
const SIDEBAR_MEDIUM: f32 = 260.;
/// How often the times ago are looked at; a row is drawn again only when its text changes.
const CLOCK: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Three columns.
    Wide,
    /// The rail is a drawer.
    Medium,
    /// The rail and the tree are drawers.
    Narrow,
}

impl Layout {
    pub fn of(width: Pixels) -> Self {
        let width = f32::from(width);
        if width >= WIDE {
            Self::Wide
        } else if width >= NARROW {
            Self::Medium
        } else {
            Self::Narrow
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Project(String),
    Session(String),
    Settings,
    /// A project's own settings.
    ProjectSettings(String),
}

pub struct Shell {
    seen: Arc<Seen>,
    snapshot: Snapshot,
    layout: Layout,
    selected: Option<Selection>,
    /// The pages shown in this window, for the mouse's back and forward buttons.
    history: History,
    /// Held by the window's root when nothing inside has the focus (a click anywhere gives it),
    /// so that Alt+Left and Alt+Right reach the root: a key goes to the focused element's path.
    focus: FocusHandle,
    top: Entity<TopBar>,
    sidebar: Entity<Sidebar>,
    main: Entity<MainArea>,
    left: Entity<Drawer>,
    bell: Entity<BellList>,
    bell_open: bool,
    projects: HashMap<String, Entity<ProjectNode>>,
    places: HashMap<(String, Place), Entity<PlaceNode>>,
    sessions: HashMap<String, Entity<SessionRow>>,
    /// Each project's composer and new workspace form, made when its page first opens: a draft,
    /// a send and a name asked belong to their project.
    forms: HashMap<String, Forms>,
    actions: Actions,
    /// The open page's project and its work when its details were last asked for: they are asked
    /// again only when that work changed.
    details_of: Option<(String, Work)>,
    details_asked: usize,
    /// What a session page needs to follow its session.
    follow: Follow,
    /// The open session's page; it follows its session while it is open.
    session_page: Option<Entity<SessionPage>>,
    /// The agent the user chooses in the open session's page, for the history.
    _chose: Option<Subscription>,
    /// Each session's composer, made when its page first opens: a draft and a send belong to
    /// their session.
    replies: HashMap<String, Entity<Reply>>,
    /// The providers as the daemon last listed them, and the number of that read.
    providers: Providers,
    providers_read: u64,
    /// What the last session started from a composer started on, for this run of the window:
    /// a composer with no project default starts there.
    last_pick: Option<Pick>,
    settings: Entity<SettingsPage>,
    /// The modal that runs a procedure in a workspace, one for the window: the `/` of every session
    /// opens it.
    run: Entity<RunProcedure>,
    /// The open project's settings page; made again for another project.
    project_settings: Option<Entity<ProjectSettingsPage>>,
    /// The form that adds a project, one for the window's life.
    add_project: Entity<AddProject>,
    /// The link's updates, until a task takes them.
    updates: Option<UnboundedReceiver<Update>>,
    _link: Task<()>,
    _clock: Task<()>,
}

type Forms = (Entity<Composer>, Entity<NewWorkspace>);

/// What can change a project's details (its branches, its workspaces' lines): the project itself,
/// its workspaces and how far they are prepared, and its sessions' states (an agent changes files
/// while it works).
type Work = (
    crate::model::Project,
    Vec<(String, Preparation)>,
    Vec<(String, Option<AgentState>)>,
);

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Shell {
    /// The shell, its updates taken as the link sends them.
    pub fn new(connect: Connect, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut shell = Self::new_polled(connect, window, cx);
        if let Some(mut updates) = shell.updates.take() {
            shell._link = cx.spawn_in(window, async move |this: WeakEntity<Shell>, cx| {
                while let Some(update) = updates.next().await {
                    if this
                        .update_in(cx, |shell, window, cx| shell.apply(update, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
        shell
    }

    /// The shell, its updates taken only by [`Shell::poll`]: on GPUI's test platform, another
    /// thread may not wake the UI's tasks.
    pub fn new_polled(connect: Connect, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (sender, updates) = unbounded();
        let seen = link::spawn_into(connect.clone(), sender.clone());
        let follow = Follow {
            connect: connect.clone(),
            updates: sender.clone(),
            opener: crate::open::system(),
        };
        let actions = Actions::spawn(connect, sender);
        let this = cx.entity().downgrade();
        let top = cx.new(|_| TopBar::new(this.clone()));
        let sidebar = cx.new(|_| Sidebar::new(this.clone()));
        let settings = cx.new(|_| SettingsPage::new(this.clone()));
        let run = cx.new(|_| RunProcedure::new(this.clone()));
        let main = {
            let (this, settings) = (this.clone(), settings.clone());
            cx.new(|cx| MainArea::new(this, settings, cx))
        };
        let left = cx.new(|_| Drawer::new("tree-drawer", Side::Left, px(SIDEBAR), sidebar.clone()));
        let bell = cx.new(|_| BellList::new(this.clone()));
        let add_project = {
            let this = this.clone();
            cx.new(|cx| AddProject::new(this, window, cx))
        };
        let _clock = cx.spawn(async move |this: WeakEntity<Shell>, cx| {
            loop {
                cx.background_executor().timer(CLOCK).await;
                if this.update(cx, |shell, cx| shell.tick(cx)).is_err() {
                    break;
                }
            }
        });
        cx.observe_window_bounds(window, |this, window, cx| this.relayout(false, window, cx))
            .detach();
        let mut shell = Self {
            seen,
            snapshot: Snapshot::default(),
            layout: Layout::Wide,
            selected: None,
            history: History::default(),
            focus: cx.focus_handle(),
            top,
            sidebar,
            main,
            left,
            bell,
            bell_open: false,
            projects: HashMap::new(),
            places: HashMap::new(),
            sessions: HashMap::new(),
            forms: HashMap::new(),
            actions,
            details_of: None,
            details_asked: 0,
            follow,
            session_page: None,
            _chose: None,
            replies: HashMap::new(),
            providers: Providers::default(),
            providers_read: 0,
            last_pick: None,
            settings,
            run,
            project_settings: None,
            add_project,
            updates: Some(updates),
            _link: Task::ready(()),
            _clock,
        };
        shell.relayout(true, window, cx);
        // Given once the window is built: a root built after would take it back.
        cx.defer_in(window, |this, window, cx| window.focus(&this.focus, cx));
        shell
    }

    /// Applies the updates the link sent since the last call.
    pub fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(Ok(update)) = self.updates.as_mut().map(|u| u.try_recv()) {
            self.apply(update, window, cx);
        }
    }

    /// The composer of a project whose page was opened.
    pub fn composer_of(&self, project: &str) -> Option<&Entity<Composer>> {
        self.forms.get(project).map(|(composer, _)| composer)
    }

    pub fn new_workspace_of(&self, project: &str) -> Option<&Entity<NewWorkspace>> {
        self.forms.get(project).map(|(_, form)| form)
    }

    /// The composer and form of the page's project, made the first time.
    fn forms_for(
        &mut self,
        page: &Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Forms> {
        let Page::Project { name } = page else {
            return None;
        };
        let this = cx.entity().downgrade();
        let forms = self.forms.entry(name.clone()).or_insert_with(|| {
            let (a, b) = (this.clone(), this);
            (
                cx.new(|cx| Composer::new(a, name.clone(), window, cx)),
                cx.new(|cx| NewWorkspace::new(b, name.clone(), window, cx)),
            )
        });
        let forms = forms.clone();
        if let Some(project) = self.snapshot.projects.iter().find(|p| &p.name == name) {
            let (repositories, default_set) = (
                project.repositories.clone(),
                project.default_repositories.clone(),
            );
            forms.1.update(cx, |form, cx| {
                form.set_repositories(repositories, default_set, cx)
            });
        }
        Some(forms)
    }

    /// The open project's settings page, made the first time, and kept while the same project's
    /// settings are open; opened, it asks the daemon for them.
    fn project_settings_for(
        &mut self,
        page: &Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ProjectSettingsPage>> {
        let Page::ProjectSettings { name } = page else {
            self.project_settings = None;
            return None;
        };
        if let Some(open) = self.project_settings.clone()
            && open.read(cx).name() == name
        {
            return Some(open);
        }
        let this = cx.entity().downgrade();
        let made = cx.new(|cx| ProjectSettingsPage::new(this, name.clone(), window, cx));
        // The shell is being updated here: it asks the daemon itself, not through the page.
        self.perform(Command::ReadProjectSettings(name.clone()));
        self.project_settings = Some(made.clone());
        Some(made)
    }

    /// The form that adds a project.
    pub fn add_project_form(&self) -> &Entity<AddProject> {
        &self.add_project
    }

    /// Opens the form that adds a project.
    pub fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        self.add_project
            .update(cx, |form, cx| form.open(window, cx));
    }

    /// What opens a folder in the user's editor, for the session pages opened from now on.
    pub fn set_opener(&mut self, opener: Opener) {
        self.follow.opener = opener;
    }

    /// Opens a link (a pull request) with the system's default handler.
    pub fn open_link(&self, url: &str) {
        (self.follow.opener)(std::path::Path::new(url));
    }

    /// The composer of a session whose page was opened.
    pub fn reply_of(&self, session: &str) -> Option<&Entity<Reply>> {
        self.replies.get(session)
    }

    /// The open session's page, made the first time, and kept while the same session is open.
    fn session_for(
        &mut self,
        page: &Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SessionPage>> {
        let Page::Session {
            id,
            title,
            place,
            status,
        } = page
        else {
            self.session_page = None;
            return None;
        };
        if let Some(open) = self.session_page.clone()
            && open.read(cx).id() == id
        {
            open.update(cx, |open, cx| open.set_title(title, cx));
            return Some(open);
        }
        let project = self.snapshot.session(id)?.project.clone();
        let folder = match place {
            None => self
                .snapshot
                .projects
                .iter()
                .find(|p| p.name == project)
                .map(|p| p.folder.clone()),
            Some(name) => self
                .snapshot
                .workspaces
                .iter()
                .find(|w| w.project == project && &w.name == name)
                .map(|w| w.folder.clone()),
        };
        let opened = Opened {
            id: id.clone(),
            title: title.clone(),
            project,
            place: place.clone(),
            folder: folder.map(Into::into),
        };
        let this = cx.entity().downgrade();
        let reply = self
            .replies
            .entry(id.clone())
            .or_insert_with(|| {
                let (this, id) = (this.clone(), id.clone());
                let (project, place) = (opened.project.clone(), opened.place.clone());
                cx.new(|cx| Reply::new(this, id, project, place, window, cx))
            })
            .clone();
        let (follow, layout, status) = (self.follow.clone(), self.layout, status.clone());
        let page = cx.new(|cx| SessionPage::new(this, opened, status, reply, follow, layout, cx));
        self._chose = Some(cx.subscribe(&page, |shell, page, Chose(agent), cx| {
            let session = page.read(cx).id().to_owned();
            shell.history.show_agent(&session, agent);
        }));
        self.session_page = Some(page.clone());
        Some(page)
    }

    /// Asks the daemon, off the UI thread; the answer comes back as an update.
    pub fn perform(&self, command: Command) {
        self.actions.send(command);
    }

    /// "New session in …": the composer, set to start there.
    pub fn compose_in(
        &mut self,
        project: &str,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(composer) = self.composer_of(project).cloned() {
            composer.update(cx, |composer, cx| composer.choose(choice, window, cx));
        }
    }

    /// A procedure picked with `/` in a session of the workspace: the modal shows what it grants.
    /// Nothing starts before Confirm.
    pub fn run_procedure(
        &mut self,
        project: &str,
        workspace: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        self.run
            .update(cx, |run, cx| run.open(project, workspace, name, cx));
    }

    pub fn procedure_run(&self) -> &Entity<RunProcedure> {
        &self.run
    }

    /// What the window shows now, as the daemon last gave it.
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn seen(&self) -> &Arc<Seen> {
        &self.seen
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn selected(&self) -> Option<&Selection> {
        self.selected.as_ref()
    }

    pub fn top(&self) -> &Entity<TopBar> {
        &self.top
    }

    pub fn main(&self) -> &Entity<MainArea> {
        &self.main
    }

    pub fn left_drawer(&self) -> &Entity<Drawer> {
        &self.left
    }

    pub fn project(&self, name: &str) -> Option<&Entity<ProjectNode>> {
        self.projects.get(name)
    }

    pub fn place(&self, project: &str, place: &Place) -> Option<&Entity<PlaceNode>> {
        self.places.get(&(project.to_owned(), place.clone()))
    }

    pub fn session(&self, id: &str) -> Option<&Entity<SessionRow>> {
        self.sessions.get(id)
    }

    fn relayout(&mut self, first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let layout = Layout::of(window.viewport_size().width);
        if layout == self.layout && !first {
            return;
        }
        self.layout = layout;
        if layout != Layout::Narrow {
            self.left.update(cx, |drawer, cx| drawer.close(cx));
        }
        self.top
            .update(cx, |top, cx| top.set_menu(layout == Layout::Narrow, cx));
        self.main.update(cx, |main, cx| main.set_layout(layout, cx));
        cx.notify();
    }

    fn apply(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::Disconnected(_) => self.set_online(false, cx),
            Update::Snapshot(snapshot) => {
                self.set_online(true, cx);
                self.show(snapshot, window, cx);
            }
            Update::Done(outcome) => self.done(outcome, window, cx),
            Update::Providers(read, providers) => self.set_providers(read, providers, cx),
            // Only the open page's link is listened to: what an older one sends is dropped.
            Update::Session { token, update } => {
                if let Some(page) = self.session_page.clone() {
                    page.update(cx, |page, cx| page.apply(token, update, cx));
                }
            }
        }
    }

    /// What came of something asked of the daemon.
    fn done(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        match outcome {
            Outcome::Details(details) => {
                if let Some(page) = self.main.read(cx).project_page().cloned() {
                    page.update(cx, |page, cx| page.set_details(&details, cx));
                }
            }
            // The session opens only if the user is still on the page they started it from.
            Outcome::Started { project, agent } => {
                if let Some(composer) = self.composer_of(&project).cloned()
                    && let Some(used) =
                        composer.update(cx, |composer, cx| composer.started(window, cx))
                {
                    self.last_pick = Some(used);
                }
                if self.selected == Some(Selection::Project(project)) {
                    self.open(Selection::Session(agent), window, cx);
                }
            }
            Outcome::NotStarted { project, why } => {
                if let Some(composer) = self.composer_of(&project).cloned() {
                    composer.update(cx, |composer, cx| composer.not_started(&why, cx));
                }
            }
            Outcome::Replied { session, refused } => {
                if let Some(reply) = self.replies.get(&session).cloned() {
                    reply.update(cx, |reply, cx| reply.replied(refused, window, cx));
                }
            }
            Outcome::Stopped { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.stopped(refused, cx));
                }
            }
            Outcome::Interrupted { session, refused } => {
                if let Some(reply) = self.replies.get(&session).cloned() {
                    reply.update(cx, |reply, cx| reply.interrupted(refused, cx));
                }
            }
            // Only for its own session's page: a page opened since has nothing to fold, and the
            // session's page, opened again, reads the decision from the daemon.
            Outcome::Answered {
                session,
                number,
                decision,
                refused,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.answered(number, decision, refused, cx));
                }
            }
            Outcome::Providers(read, Some(providers)) => self.set_providers(read, providers, cx),
            Outcome::Providers(_, None) => {}
            Outcome::ProviderSet { name, refused } => {
                // The change shows once the providers are read again: now, rather than only when
                // its event comes, since a change to what the file already said emits none.
                if refused.is_none() {
                    self.actions.send(Command::ReadProviders);
                }
                self.settings
                    .update(cx, |page, cx| page.answered(&name, refused, cx));
            }
            Outcome::ProjectSettings {
                project,
                read,
                saved,
            } => {
                if let Some(page) = self.project_settings.clone()
                    && page.read(cx).name() == project
                {
                    page.update(cx, |page, cx| page.answered(read, saved, window, cx));
                }
            }
            Outcome::ProjectChecked { project, read } => {
                if let Some(page) = self.project_settings.clone()
                    && page.read(cx).name() == project
                {
                    page.update(cx, |page, cx| page.check_answered(read, cx));
                }
            }
            Outcome::ProjectRemoved { project, refused } => match refused {
                Some(why) => {
                    if let Some(page) = self.project_settings.clone()
                        && page.read(cx).name() == project
                    {
                        page.update(cx, |page, cx| page.remove_refused(why, cx));
                    }
                }
                None => {
                    self.forms.remove(&project);
                    self.snapshot.projects.retain(|p| p.name != project);
                    let next = self.snapshot.projects.first().map(|p| p.name.clone());
                    match next {
                        Some(next) => self.open(Selection::Project(next), window, cx),
                        None => {
                            self.selected = None;
                            self.project_settings = None;
                            self.main.update(cx, |main, cx| {
                                main.show(Page::Empty, None, None, None, cx)
                            });
                        }
                    }
                }
            },
            Outcome::ProjectDetected { read, .. } => {
                self.add_project
                    .update(cx, |form, cx| form.detect_answered(read, cx));
            }
            Outcome::ProjectAdded { read, .. } => {
                let added = self
                    .add_project
                    .update(cx, |form, cx| form.add_answered(read, window, cx));
                if let Some(name) = added {
                    self.open(Selection::ProjectSettings(name), window, cx);
                }
            }
            Outcome::TicketPreview {
                project,
                ticket,
                read,
            } => {
                if let Some(form) = self.new_workspace_of(&project).cloned() {
                    form.update(cx, |form, cx| form.previewed(&ticket, read, cx));
                }
            }
            Outcome::TicketLinked {
                project,
                name,
                refused,
            } => {
                if let Some(page) = self.session_page.clone() {
                    page.update(cx, |page, cx| {
                        page.ticket_linked(&project, &name, refused, window, cx)
                    });
                }
            }
            Outcome::Procedures { project, listed } => {
                // The `/` lists of the project's sessions.
                for reply in self.replies.values() {
                    reply.update(cx, |reply, cx| {
                        reply.procedures_listed(&project, &listed, cx)
                    });
                }
                if let Some(page) = self.page_of_project(&project, cx) {
                    let section = page.read(cx).procedures().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::ProcedureRead {
                project,
                name,
                read,
            } => {
                if let Some(page) = self.page_of_project(&project, cx) {
                    let section = page.read(cx).procedures().clone();
                    section.update(cx, |section, cx| {
                        section.read_whole(&name, read, window, cx)
                    });
                }
            }
            Outcome::ProcedureWritten { project, refused } => {
                if let Some(page) = self.page_of_project(&project, cx) {
                    let section = page.read(cx).procedures().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::ProcedureShown {
                project,
                name,
                workspace,
                shown,
            } => self.run.update(cx, |run, cx| {
                run.shown_by_daemon(&project, &name, &workspace, shown, cx)
            }),
            // The run opens as a session, wherever the user is.
            Outcome::ProcedureStarted { project, started } => {
                let (agent, refused) = match started {
                    Ok(agent) => (Some(agent), None),
                    Err(why) => (None, Some(why)),
                };
                self.run
                    .update(cx, |run, cx| run.started(&project, refused, cx));
                if let Some(agent) = agent {
                    self.open(Selection::Session(agent), window, cx);
                }
            }
            Outcome::References { project, listed } => {
                if let Some(page) = self.page_of_project(&project, cx) {
                    let section = page.read(cx).references().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::ReferenceWritten { project, refused } => {
                if let Some(page) = self.page_of_project(&project, cx) {
                    let section = page.read(cx).references().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::WorkspaceDone { project, name } => {
                self.workspace_answered(&project, &name, None, window, cx)
            }
            Outcome::WorkspaceRefused { project, name, why } => {
                self.workspace_answered(&project, &name, Some(&why), window, cx)
            }
        }
    }

    /// The open page, when it is `project`'s.
    fn page_of_project(
        &self,
        project: &str,
        cx: &App,
    ) -> Option<Entity<crate::project::ProjectPage>> {
        self.main
            .read(cx)
            .project_page()
            .filter(|page| page.read(cx).name() == project)
            .cloned()
    }

    fn workspace_answered(
        &mut self,
        project: &str,
        name: &str,
        refused: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.new_workspace_of(project).cloned()
            && form.read(cx).creating() == Some(name)
        {
            let for_composer = form.update(cx, |form, cx| form.answered(refused, window, cx));
            if for_composer && let Some(composer) = self.composer_of(project).cloned() {
                composer.update(cx, |composer, cx| {
                    composer.choose_when_listed(name.to_owned(), cx)
                });
            }
        } else if let Some(page) = self.main.read(cx).project_page().cloned()
            && page.read(cx).name() == project
            && let Some(card) = page.read(cx).card(&Some(name.to_owned()), cx)
        {
            card.update(cx, |card, cx| card.answered(refused, cx));
        }
    }

    /// Brings the open project's page up to what the daemon last gave, and asks for its details
    /// again.
    fn refresh_project(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.main.read(cx).project_page().cloned() else {
            // A page opened later is a new one: it asks for its details.
            self.details_of = None;
            return;
        };
        let name = page.read(cx).name().to_owned();
        let Some(project) = self.snapshot.projects.iter().find(|p| p.name == name) else {
            return;
        };
        let work: Work = (
            project.clone(),
            self.snapshot
                .workspaces
                .iter()
                .filter(|w| w.project == name)
                .map(|w| (w.name.clone(), w.preparation.clone()))
                .collect(),
            self.snapshot
                .sessions
                .iter()
                .filter(|s| s.project == name)
                .map(|s| (s.id.clone(), s.mark))
                .collect(),
        );
        let line = |session: &crate::model::Session, cx: &App| {
            self.sessions.get(&session.id).map(|row| SessionLine {
                id: session.id.clone(),
                title: session.title.clone(),
                agents: session.agents,
                mark: session.mark,
                active_at: session.active_at,
                status: row.read(cx).status(),
            })
        };
        let mut places = vec![PlaceData {
            place: None,
            meta: None,
            preparation: Preparation::Ready,
            sessions: self
                .snapshot
                .sessions_in(&name, &None)
                .filter_map(|s| line(s, cx))
                .collect(),
        }];
        let mut choices = vec![PlaceChoice {
            workspace: None,
            ready: true,
        }];
        for workspace in self
            .snapshot
            .workspaces
            .iter()
            .filter(|w| w.project == name)
        {
            let place = Some(workspace.name.clone());
            let repositories: Vec<String> = workspace
                .repositories
                .iter()
                .map(|path| repository_label(&name, path))
                .collect();
            places.push(PlaceData {
                meta: Some(format!(
                    "from {} · {}",
                    workspace.bases.join(", "),
                    repositories.join(", ")
                )),
                preparation: workspace.preparation.clone(),
                sessions: self
                    .snapshot
                    .sessions_in(&name, &place)
                    .filter_map(|s| line(s, cx))
                    .collect(),
                place,
            });
            choices.push(PlaceChoice {
                workspace: Some(workspace.name.clone()),
                ready: workspace.preparation == Preparation::Ready,
            });
        }
        let data = ProjectData {
            name: name.clone(),
            folder: project.folder.clone(),
            repositories: project.repositories.clone(),
            places,
        };
        page.update(cx, |page, cx| page.set(data, cx));
        let default = (project.provider.clone(), project.model.clone());
        if let Some(composer) = self.composer_of(&name).cloned() {
            let (providers, last) = (&self.providers, self.last_pick.as_ref());
            composer.update(cx, |composer, cx| {
                composer.set_places(choices, cx);
                composer.set_providers(
                    providers,
                    (default.0.as_deref(), default.1.as_deref()),
                    last,
                    cx,
                );
            });
        }
        if self
            .details_of
            .as_ref()
            .is_none_or(|(asked, then)| *asked != name || *then != work)
        {
            self.details_of = Some((name.clone(), work));
            self.details_asked += 1;
            self.actions.send(Command::Details(name));
        }
    }

    /// The providers as the daemon lists them, to the settings page and to every composer; an
    /// answer to a read asked before the last one applied is older than it, and dropped.
    fn set_providers(&mut self, read: u64, providers: Providers, cx: &mut Context<Self>) {
        if read < self.providers_read {
            return;
        }
        self.providers_read = read;
        self.settings
            .update(cx, |page, cx| page.set_providers(&providers, cx));
        for (name, (composer, _)) in &self.forms {
            let project = self.snapshot.projects.iter().find(|p| &p.name == name);
            let default = (
                project.and_then(|p| p.provider.as_deref()),
                project.and_then(|p| p.model.as_deref()),
            );
            let last = self.last_pick.as_ref();
            composer.update(cx, |composer, cx| {
                composer.set_providers(&providers, default, last, cx)
            });
        }
        self.providers = providers;
    }

    /// The providers as the daemon last listed them.
    pub fn providers(&self) -> &Providers {
        &self.providers
    }

    /// How many times the open page's details were asked for: the tests count them.
    pub fn details_asked(&self) -> usize {
        self.details_asked
    }

    fn set_online(&mut self, online: bool, cx: &mut Context<Self>) {
        if self.top.read(cx).online != online {
            self.top.update(cx, |top, cx| top.set_online(online, cx));
        }
    }

    /// Brings every row up to `snapshot`, creating and dropping rows as needed. A row whose data
    /// did not change is not drawn again.
    fn show(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let now = now_ms();
        let this = cx.entity().downgrade();
        let mut projects = HashMap::new();
        let mut places = HashMap::new();
        let mut sessions = HashMap::new();
        let mut project_nodes = Vec::new();
        for project in &snapshot.projects {
            let mut place_nodes = Vec::new();
            let mut project_marks = Vec::new();
            let all: Vec<Place> = std::iter::once(None)
                .chain(project.workspaces.iter().cloned().map(Some))
                .collect();
            for place in all {
                let mut rows = Vec::new();
                let mut marks = Vec::new();
                for session in snapshot.sessions_in(&project.name, &place) {
                    let row = self.sessions.remove(&session.id).unwrap_or_else(|| {
                        let this = this.clone();
                        let id = session.id.clone();
                        // A session opened before the tree showed it (one just started).
                        let selected =
                            self.selected == Some(Selection::Session(session.id.clone()));
                        cx.new(|cx| {
                            let mut row = SessionRow::new(id, this, cx);
                            row.set_selected(selected, cx);
                            row
                        })
                    });
                    // GPUI Fast takes any update of an entity for a change: touch only what
                    // changed.
                    if !row
                        .read(cx)
                        .holds(&session.title, session.mark, session.active_at)
                    {
                        row.update(cx, |row, cx| {
                            row.set(&session.title, session.mark, session.active_at, now, cx)
                        });
                    }
                    marks.extend(session.mark);
                    rows.push(row.clone());
                    sessions.insert(session.id.clone(), row);
                }
                let key = (project.name.clone(), place.clone());
                let node = self.places.remove(&key).unwrap_or_else(|| {
                    let (name, place) = key.clone();
                    cx.new(|cx| PlaceNode::new(name, place, cx))
                });
                let mark = AgentState::strongest(marks.iter().copied());
                if !node.read(cx).holds(&rows, mark, cx) {
                    node.update(cx, |node, cx| node.set(rows, mark, cx));
                }
                project_marks.extend(mark);
                place_nodes.push(node.clone());
                places.insert(key, node);
            }
            let node = self.projects.remove(&project.name).unwrap_or_else(|| {
                let name = project.name.clone();
                let this = this.clone();
                cx.new(|cx| ProjectNode::new(name, this, cx))
            });
            let mark = AgentState::strongest(project_marks);
            if !node.read(cx).holds(&place_nodes, mark, cx) {
                node.update(cx, |node, cx| node.set(place_nodes, mark, cx));
            }
            project_nodes.push(node.clone());
            projects.insert(project.name.clone(), node);
        }
        self.projects = projects;
        self.places = places;
        self.sessions = sessions;
        if self.sidebar.read(cx).projects() != project_nodes {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_projects(project_nodes, cx));
        }

        let items: Vec<(BellItem, String)> = snapshot
            .bell
            .iter()
            .map(|item| {
                let title = snapshot
                    .session(&item.session)
                    .map(|s| s.title.clone())
                    .unwrap_or_default();
                (item.clone(), title)
            })
            .collect();
        let count = items.len();
        if self.top.read(cx).count != count {
            self.top.update(cx, |top, cx| top.set_count(count, cx));
        }
        if self.bell.read(cx).items != items {
            self.bell.update(cx, |bell, cx| bell.set(items, cx));
        }
        self.forms
            .retain(|name, _| snapshot.projects.iter().any(|p| &p.name == name));
        self.replies
            .retain(|id, _| snapshot.sessions.iter().any(|s| &s.id == id));
        self.snapshot = snapshot;

        // What was open is shown as it is now; the first project opens when nothing is.
        match self.selected.clone() {
            None => {
                if let Some(first) = self.snapshot.projects.first() {
                    self.select(Selection::Project(first.name.clone()), window, cx);
                }
            }
            Some(selection) => {
                let page = self.page_of(&selection, cx);
                if self.main.read(cx).page() != &page {
                    let forms = self.forms_for(&page, window, cx);
                    let session = self.session_for(&page, window, cx);
                    let settings = self.project_settings_for(&page, window, cx);
                    self.main
                        .update(cx, |main, cx| main.show(page, forms, session, settings, cx));
                }
                self.refresh_project(cx);
            }
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        for row in self.sessions.values() {
            if row.read(cx).ago_changes(now) {
                row.update(cx, |row, cx| row.tick(now, cx));
            }
        }
        if let Some(page) = self.main.read(cx).project_page().cloned() {
            crate::project::ProjectPage::tick(&page, now, cx);
            // The project's file is the user's to edit by hand too.
            let project = page.read(cx).name().to_owned();
            self.actions.send(Command::Procedures {
                project: project.clone(),
            });
            self.actions.send(Command::References { project });
        }
    }

    /// Opens a project's page or a session's: a choice made in a drawer closes it.
    pub fn open(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        self.select(selection, window, cx);
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        self.set_bell(false, cx);
    }

    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        self.history.open(selection.clone());
        self.show_selection(selection, window, cx);
    }

    /// The mouse's back and forward buttons, Alt+Left and Alt+Right: the nearest page that way
    /// which still exists. A session's page shows the agent that was shown there.
    pub fn go(&mut self, way: Way, window: &mut Window, cx: &mut Context<Self>) {
        let exists =
            |visit: &crate::history::Visit| self.page_of(&visit.selection, cx) != Page::Empty;
        let Some(at) = self.history.find(way, exists) else {
            return;
        };
        let visit = self.history.move_to(at);
        self.show_selection(visit.selection.clone(), window, cx);
        if let (Selection::Session(id), Some(page)) = (&visit.selection, self.session_page.clone())
        {
            let agent = visit.agent.unwrap_or_else(|| id.clone());
            page.update(cx, |page, cx| page.show(&agent, cx));
        }
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        self.set_bell(false, cx);
        // The page that held the focus is gone.
        window.focus(&self.focus, cx);
    }

    fn show_selection(
        &mut self,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(Selection::Session(old)) = &self.selected
            && let Some(row) = self.sessions.get(old)
        {
            row.update(cx, |row, cx| row.set_selected(false, cx));
        }
        if let Selection::Session(id) = &selection
            && let Some(row) = self.sessions.get(id)
        {
            row.update(cx, |row, cx| row.set_selected(true, cx));
        }
        // Opened, the page reads the providers again: the file is the user's to edit by hand.
        if selection == Selection::Settings && self.selected != Some(Selection::Settings) {
            self.actions.send(Command::ReadProviders);
        }
        let page = self.page_of(&selection, cx);
        self.selected = Some(selection);
        let forms = self.forms_for(&page, window, cx);
        let session = self.session_for(&page, window, cx);
        let project_settings = self.project_settings_for(&page, window, cx);
        self.main.update(cx, |main, cx| {
            main.show(page, forms, session, project_settings, cx)
        });
        self.refresh_project(cx);
    }

    /// The page of what is selected, as the daemon last gave it.
    fn page_of(&self, selection: &Selection, cx: &App) -> Page {
        match selection {
            Selection::Settings => Page::Settings,
            // A project just added is not in the daemon's last read yet: its page, open, stays.
            Selection::ProjectSettings(name) => {
                let open = self
                    .project_settings
                    .as_ref()
                    .is_some_and(|page| page.read(cx).name() == name);
                if open || self.snapshot.projects.iter().any(|p| &p.name == name) {
                    Page::ProjectSettings { name: name.clone() }
                } else {
                    Page::Empty
                }
            }
            Selection::Project(name) => {
                match self.snapshot.projects.iter().find(|p| &p.name == name) {
                    Some(project) => Page::Project {
                        name: project.name.clone(),
                    },
                    None => Page::Empty,
                }
            }
            Selection::Session(id) => {
                let open = self
                    .session_page
                    .as_ref()
                    .map(|page| page.read(cx))
                    .filter(|page| page.id() == id);
                match (self.snapshot.session(id), self.sessions.get(id), open) {
                    (Some(session), Some(row), _) => Page::Session {
                        id: id.clone(),
                        title: session.title.clone(),
                        place: session.place.clone(),
                        status: row.read(cx).status(),
                    },
                    // A session whose workspace was removed has no row in the tree: its page,
                    // open, stays open.
                    (Some(session), None, Some(page)) => Page::Session {
                        id: id.clone(),
                        title: session.title.clone(),
                        place: session.place.clone(),
                        status: page.status().clone(),
                    },
                    _ => Page::Empty,
                }
            }
        }
    }

    /// "New session": the composer that starts one is on the project page.
    pub fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = match &self.selected {
            Some(Selection::Project(name)) => Some(name.clone()),
            Some(Selection::Session(id)) => self.snapshot.session(id).map(|s| s.project.clone()),
            Some(Selection::ProjectSettings(name)) => Some(name.clone()),
            Some(Selection::Settings) | None => None,
        }
        .or_else(|| self.snapshot.projects.first().map(|p| p.name.clone()));
        if let Some(project) = project {
            self.open(Selection::Project(project), window, cx);
        }
    }

    pub fn toggle_bell(&mut self, cx: &mut Context<Self>) {
        self.set_bell(!self.bell_open, cx);
    }

    fn set_bell(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.bell_open != open {
            self.bell_open = open;
            self.bell.update(cx, |bell, cx| bell.show(open, cx));
            cx.notify();
        }
    }

    pub fn bell_open(&self) -> bool {
        self.bell_open
    }

    /// Opens what a bell item waits in: its session.
    pub fn choose(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let session = self
            .snapshot
            .bell
            .iter()
            .find(|item| item.key == key)
            .map(|item| item.session.clone());
        if let Some(session) = session {
            self.open(Selection::Session(session), window, cx);
        }
    }

    pub fn open_tree(&mut self, cx: &mut Context<Self>) {
        self.left.update(cx, |drawer, cx| drawer.open(cx));
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let side = match self.layout {
            Layout::Wide => Some(SIDEBAR),
            Layout::Medium => Some(SIDEBAR_MEDIUM),
            Layout::Narrow => None,
        };
        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .flex()
            .when_some(side, |d, width| {
                d.child(
                    div()
                        .flex_none()
                        .w(px(width))
                        .h_full()
                        .border_r_1()
                        .border_color(p.line)
                        .child(self.sidebar.clone()),
                )
            })
            .child(div().flex_1().min_w_0().h_full().child(self.main.clone()))
            .when(side.is_none(), |d| d.child(self.left.clone()));
        let shell = cx.entity().downgrade();
        // The overlays drawn from here on say they are open: Alt+Left closes the topmost.
        forget_overlays(cx);
        if self.bell_open {
            let shell = shell.clone();
            open_overlay(
                "bell-scrim".into(),
                // A key reaches this while the shell handles it: the update waits.
                move |_, cx| {
                    let shell = shell.clone();
                    cx.defer(move |cx| {
                        shell.update(cx, |this, cx| this.set_bell(false, cx)).ok();
                    });
                },
                cx,
            );
        }
        div()
            .id("shell")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(p.bg)
            .text_color(p.text)
            .ui_text()
            .flex()
            .flex_col()
            // An open list, modal or drawer takes the buttons first (`dismiss_on_navigate`): what
            // reaches here is a press with nothing open under the pointer.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.go(Way::Back, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.go(Way::Forward, window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let way = match event.keystroke.key.as_str() {
                    "left" => Way::Back,
                    "right" => Way::Forward,
                    _ => return,
                };
                if event.keystroke.modifiers == Modifiers::alt() {
                    cx.stop_propagation();
                    // Like the mouse's buttons: an open overlay closes first and that is all.
                    if dismiss_top_overlay(window, cx) {
                        // Whatever held the focus in it is going: the next key must still arrive.
                        window.focus(&this.focus, cx);
                    } else {
                        this.go(way, window, cx);
                    }
                }
            }))
            .child(self.top.clone())
            .child(body)
            .child(self.run.clone())
            .child(self.add_project.clone())
            .when(self.bell_open, |d| {
                let scrim = div()
                    .id("bell-scrim")
                    .absolute()
                    .inset_0()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_bell(cx)));
                d.child(dismiss_on_navigate(scrim, move |_, cx| {
                    shell.update(cx, |this, cx| this.set_bell(false, cx)).ok();
                }))
                .child(
                    div()
                        .absolute()
                        .top(px(40.))
                        .right(px(12.))
                        .child(self.bell.clone()),
                )
            })
    }
}

/// The top bar: the mark, search, and the bell with the count of what waits for the user.
pub struct TopBar {
    shell: WeakEntity<Shell>,
    count: usize,
    online: bool,
    menu: bool,
}

impl TopBar {
    fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            count: 0,
            online: true,
            menu: false,
        }
    }

    /// How many things wait for the user.
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn online(&self) -> bool {
        self.online
    }

    fn set_count(&mut self, count: usize, cx: &mut Context<Self>) {
        if self.count != count {
            self.count = count;
            cx.notify();
        }
    }

    fn set_online(&mut self, online: bool, cx: &mut Context<Self>) {
        if self.online != online {
            self.online = online;
            cx.notify();
        }
    }

    fn set_menu(&mut self, menu: bool, cx: &mut Context<Self>) {
        if self.menu != menu {
            self.menu = menu;
            cx.notify();
        }
    }
}

impl Render for TopBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let shell = self.shell.clone();
        let menu_shell = self.shell.clone();
        let settings_shell = self.shell.clone();
        let mut bell =
            Button::icon("bell", Icon::Bell, "Waiting for you").on_click(move |_, _, cx| {
                shell.update(cx, |shell, cx| shell.toggle_bell(cx)).ok();
            });
        if self.count > 0 {
            bell = bell.badge(Badge::count(self.count));
        }
        div()
            .flex_none()
            .h(px(40.))
            .w_full()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(p.rail)
            .border_b_1()
            .border_color(p.line)
            .when(self.menu, |d| {
                d.child(Button::icon("open-tree", Icon::Menu, "Projects").on_click(
                    move |_, _, cx| {
                        menu_shell.update(cx, |shell, cx| shell.open_tree(cx)).ok();
                    },
                ))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(6.))
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(5.))
                            .bg(p.acc)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(logo(p.on_acc).size(px(12.))),
                    )
                    .child(div().mono(12.).medium().child("FLOTTILLE")),
            )
            .child(div().flex_1())
            .when(!self.online, |d| {
                d.child(
                    div()
                        .id("offline")
                        .mono(11.)
                        .text_color(p.faint)
                        .child("reconnecting…"),
                )
            })
            .child(
                div()
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_color(p.faint)
                    .text_size(px(12.5))
                    .child(icon(Icon::Search).size(px(13.)).text_color(p.faint))
                    .child("Search")
                    .child(div().mono(11.).child("Ctrl K")),
            )
            .child(
                Button::icon("settings", Icon::Settings, "Settings").on_click(
                    move |_, window, cx| {
                        settings_shell
                            .update(cx, |shell, cx| shell.open(Selection::Settings, window, cx))
                            .ok();
                    },
                ),
            )
            .child(bell)
    }
}

/// The list the bell opens: what waits for the user, oldest first. Its marks exist only while it
/// is open, so that a closed list asks for no frame.
pub struct BellList {
    shell: WeakEntity<Shell>,
    items: Vec<(BellItem, String)>,
    marks: Vec<Entity<StateIcon>>,
}

impl BellList {
    fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            items: Vec::new(),
            marks: Vec::new(),
        }
    }

    fn set(&mut self, items: Vec<(BellItem, String)>, cx: &mut Context<Self>) {
        if self.items != items {
            self.items = items;
            if !self.marks.is_empty() {
                self.show(true, cx);
            }
            cx.notify();
        }
    }

    fn show(&mut self, open: bool, cx: &mut Context<Self>) {
        self.marks = if open {
            self.items
                .iter()
                .map(|_| {
                    let status = cx.new(|_| Status::new(AgentState::NeedsYou));
                    cx.new(|cx| StateIcon::new(status, cx))
                })
                .collect()
        } else {
            Vec::new()
        };
        cx.notify();
    }
}

impl Render for BellList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let list = div()
            .id("bell-list")
            .occlude()
            .w(px(340.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .p(px(6.))
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line2)
            .flex()
            .flex_col();
        let shell = self.shell.clone();
        let list = dismiss_on_navigate(list, move |_, cx| {
            shell.update(cx, |shell, cx| shell.set_bell(false, cx)).ok();
        });
        if self.items.is_empty() {
            return list.child(
                div()
                    .p(px(10.))
                    .text_color(p.faint)
                    .child("Nothing waits for you."),
            );
        }
        list.children(
            self.items
                .iter()
                .zip(&self.marks)
                .map(|((item, title), mark)| {
                    let shell = self.shell.clone();
                    let key = item.key.clone();
                    ListRow::new(
                        ElementId::Name(format!("bell-{}", item.key).into()),
                        SharedString::from(item.label.clone()),
                    )
                    .leading(mark.clone())
                    .meta(SharedString::from(title.clone()))
                    .on_click(move |_, window, cx| {
                        shell
                            .update(cx, |shell, cx| shell.choose(&key, window, cx))
                            .ok();
                    })
                }),
        )
    }
}

/// The Flottille mark: a hull and two sails, drawn in a 16-unit box.
fn logo(color: gpui_kit::Hsla) -> gpui_kit::Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let unit = f32::from(bounds.size.width) / 16.0;
            let at = |x: f32, y: f32| {
                point(
                    bounds.origin.x + px(x * unit),
                    bounds.origin.y + px(y * unit),
                )
            };
            let mut path = PathBuilder::stroke(px(1.7 * unit));
            path.move_to(at(2.0, 11.5));
            path.cubic_bezier_to(at(14.0, 11.5), at(4.5, 13.1), at(11.5, 13.1));
            path.move_to(at(5.5, 10.0));
            path.line_to(at(5.5, 3.0));
            path.line_to(at(9.5, 8.5));
            path.line_to(at(5.5, 8.5));
            path.move_to(at(10.5, 10.0));
            path.line_to(at(10.5, 5.5));
            path.line_to(at(13.0, 9.0));
            path.line_to(at(10.5, 9.0));
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
}
