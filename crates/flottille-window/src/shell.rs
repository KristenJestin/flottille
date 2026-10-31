//! The window's frame: the top bar (the mark, search, the bell), the left tree, and the main area
//! showing the open page. It follows the daemon through the [`link`](crate::link) and draws
//! again only the parts a change touches.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flottille_ui::motion::{ENTER, HOVER, Motion, RISE};
use flottille_ui::{
    AgentState, Badge, Button, Card, Drawer, Icon, Item, ListRow, Listing, Side, StateIcon, Status,
    ToastStack, Type, dismiss_on_navigate, dismiss_top_overlay, enter, forget_overlays, icon,
    open_overlay, palette, presence, space,
};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, ElementId, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, Modifiers, MouseButton, NavigationDirection,
    ParentElement, PathBuilder, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, TestSupportExt as _, WeakEntity, Window, canvas, div, point, px,
};

use crate::actions::{Actions, Command, Outcome, Pick};
use crate::add_project::AddProject;
use crate::composer::{Choice, Composer, PlaceChoice};
use crate::history::{History, Way};
use crate::link::{self, Connect, Seen, Update};
use crate::model::{BellItem, Forged, Place, Preparation, Snapshot};
use crate::open::Opener;
use crate::page::{MainArea, Page};
use crate::project::{NewWorkspace, PlaceData, ProjectData, SessionLine};
use crate::project_settings::ProjectSettingsPage;
use crate::providers::Providers;
use crate::rules::RulesEditor;
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
    /// The results of the user's actions, at the bottom right, above everything.
    toasts: Entity<ToastStack>,
    bell_open: bool,
    /// How far the bell's list is in view: it rises in and fades out, and stays drawn until then.
    bell_shown: Motion,
    projects: HashMap<String, Entity<ProjectNode>>,
    places: HashMap<(String, Place), Entity<PlaceNode>>,
    /// The "Ended" group at the bottom of the tree, made when a session first ends.
    ended: Option<Entity<PlaceNode>>,
    sessions: HashMap<String, Entity<SessionRow>>,
    /// The tickets and pull requests the sessions' agents created or changed, oldest first: each
    /// marks its session's row and rings the bell until the session is opened. They live as long
    /// as the window.
    forged: Vec<Forged>,
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
    /// The editor of the user's rules, on the settings page.
    rules: Entity<RulesEditor>,
    /// The notice of the Auto judge Flottille picked was dismissed here, before the daemon says
    /// it was seen.
    judge_noticed: bool,
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
        let rules = cx.new(|_| RulesEditor::new(this.clone(), None));
        let judge_guidance = cx.new(|_| RulesEditor::judge(this.clone()));
        let settings =
            cx.new(|_| SettingsPage::new(this.clone(), rules.clone(), judge_guidance.clone()));
        let main = {
            let (this, settings) = (this.clone(), settings.clone());
            cx.new(|cx| MainArea::new(this, settings, cx))
        };
        let left = cx.new(|_| Drawer::new("tree-drawer", Side::Left, px(SIDEBAR), sidebar.clone()));
        let bell = cx.new(|_| BellList::new(this.clone()));
        let toasts = cx.new(ToastStack::new);
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
            toasts,
            bell_open: false,
            bell_shown: Motion::settled(0.0),
            projects: HashMap::new(),
            places: HashMap::new(),
            ended: None,
            sessions: HashMap::new(),
            forged: Vec::new(),
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
            rules,
            judge_noticed: false,
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
        self.perform(Command::RulesRead {
            project: Some(name.clone()),
        });
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
        let session = self.snapshot.session(id)?;
        let (project, started_by) = (session.project.clone(), session.started_by.clone());
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
            started_by,
        };
        let this = cx.entity().downgrade();
        let reply = self
            .replies
            .entry(id.clone())
            .or_insert_with(|| {
                let (this, id) = (this.clone(), id.clone());
                let project = opened.project.clone();
                cx.new(|cx| Reply::new(this, id, project, window, cx))
            })
            .clone();
        if self.providers.known {
            let mode = reply.read(cx).mode.clone();
            let chosen = self.providers.judge.is_some();
            mode.update(cx, |mode, cx| mode.set_judge_chosen(chosen, cx));
        }
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

    /// The "+" of a place's row: the composer of its project, set to start there, with the field
    /// focused.
    pub fn start_in(
        &mut self,
        project: &str,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Selection::Project(project.to_owned()), window, cx);
        // The focus given while the page is built is taken back by what is built after it: wait
        // for the frame, then give it.
        let project = project.to_owned();
        cx.defer_in(window, move |shell, window, cx| {
            shell.compose_in(&project, choice, window, cx)
        });
    }

    /// "End session" in a session's row: the confirmation, which counts the agents that end.
    pub fn ask_end_of(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(found) = self.snapshot.session(session) else {
            return;
        };
        let (id, count) = (found.id.clone(), found.live);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.ask_end(id, count, cx));
    }

    /// "Resume" in an ended session's row.
    pub fn resume_of(&mut self, session: &str, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.resuming(session, cx));
        self.perform(Command::Resume {
            session: session.to_owned(),
        });
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

    /// The window's toasts, at its bottom right.
    pub fn toasts(&self) -> &Entity<ToastStack> {
        &self.toasts
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

    /// The "Ended" group at the bottom of the tree, once a session ended.
    pub fn ended(&self) -> Option<&Entity<PlaceNode>> {
        self.ended.as_ref()
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
            Update::TextChanged { judge: true, .. } => self.actions.send(Command::JudgeRead),
            Update::DeveloperChanged => self.actions.send(Command::DeveloperRead),
            Update::Forged(forged) => self.forged(forged, cx),
            Update::TextChanged { project, .. } => {
                self.actions.send(Command::RulesRead { project })
            }
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
            Outcome::ImageRead {
                session,
                name,
                image,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).shown() == session
                {
                    page.update(cx, |page, cx| page.picture_read(name, image, cx));
                }
            }
            Outcome::Replied { session, refused } => {
                if let Some(reply) = self.replies.get(&session).cloned() {
                    reply.update(cx, |reply, cx| reply.replied(refused, window, cx));
                }
            }
            Outcome::Unqueued {
                session,
                delivery,
                edit,
                taken,
            } => {
                let queued = self
                    .session_page
                    .clone()
                    .filter(|page| page.read(cx).id() == session);
                let queued = queued.map(|page| page.read(cx).queued().clone());
                // Gone from the queue whether it was taken back, or the root read it first.
                let left = match &taken {
                    Ok(_) => true,
                    Err(why) => why == crate::queued::ALREADY_READ,
                };
                if let Some(queued) = queued {
                    queued.update(cx, |list, cx| list.answered(&delivery, left, cx));
                }
                match taken {
                    Ok((text, pictures)) if edit => {
                        if let Some(reply) = self.replies.get(&session).cloned() {
                            reply
                                .update(cx, |reply, cx| reply.restore(&text, pictures, window, cx));
                        }
                    }
                    Ok(_) => {}
                    Err(why) => flottille_ui::toast(
                        crate::queued::refused_toast(&why, "The message was not taken back"),
                        cx,
                    ),
                }
            }
            Outcome::SentNow {
                session,
                delivery,
                refused,
            } => {
                let queued = self
                    .session_page
                    .clone()
                    .filter(|page| page.read(cx).id() == session);
                if let Some(queued) = queued.map(|page| page.read(cx).queued().clone()) {
                    let left = refused.as_deref() == Some(crate::queued::ALREADY_READ);
                    queued.update(cx, |list, cx| list.answered(&delivery, left, cx));
                }
                if let Some(why) = refused {
                    flottille_ui::toast(
                        crate::queued::refused_toast(&why, "The message was not sent now"),
                        cx,
                    );
                }
            }
            Outcome::Stopped { session, refused } => {
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.answered(&session, refused.clone(), cx)
                });
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.end_answered(refused, cx));
                }
            }
            Outcome::Resumed { session, refused } => {
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.answered(&session, refused.clone(), cx)
                });
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.resumed(refused, cx));
                }
            }
            Outcome::ModeSet { session, refused } => {
                if let Some(reply) = self.replies.get(&session) {
                    let mode = reply.read(cx).mode.clone();
                    mode.update(cx, |mode, cx| mode.answered(refused, cx));
                }
            }
            Outcome::RunAnswered { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.command_answered(refused, cx));
                }
            }
            Outcome::SubscriptionCancelled { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.subscription_answered(refused, cx));
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
            Outcome::JudgeSet { refused } => {
                if refused.is_none() {
                    self.actions.send(Command::ReadProviders);
                }
                self.settings
                    .update(cx, |page, cx| page.judge_answered(refused, cx));
            }
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
            Outcome::Repositories {
                project,
                read,
                serial,
            } => {
                // The project settings list them when they are open on the project; the project
                // page reads them otherwise.
                let section = self
                    .settings_of_project(&project, cx)
                    .map(|page| page.read(cx).repositories().clone())
                    .or_else(|| {
                        let page = self.main.read(cx).project_page().cloned();
                        page.filter(|page| page.read(cx).name() == project)
                            .map(|page| page.read(cx).repositories().clone())
                    });
                // An answer to an older request than the last is no news, for the form either.
                if section
                    .as_ref()
                    .is_some_and(|section| !section.read(cx).is_latest(serial))
                {
                    return;
                }
                if let Ok(value) = &read
                    && let Some(form) = self.new_workspace_of(&project).cloned()
                {
                    let rules = crate::repositories::rules_of(
                        &crate::repositories::Repositories::of(value),
                    );
                    form.update(cx, |form, cx| form.set_rules(rules, cx));
                }
                if let Some(section) = section {
                    section.update(cx, |section, cx| section.answered(serial, read, window, cx));
                }
            }
            // Detect again is the settings' own read of the repositories now.
            Outcome::ProjectChecked { .. } => {}
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
                if let Some(page) = self.main.read(cx).project_page().cloned()
                    && page.read(cx).name() == project
                    && let Some(card) = page.read(cx).card(&Some(name.clone()), cx)
                {
                    card.update(cx, |card, cx| card.ticket_answered(refused.clone(), cx));
                    self.perform(Command::Details(project.clone()));
                }
                if let Some(page) = self.session_page.clone() {
                    page.update(cx, |page, cx| {
                        page.ticket_linked(&project, &name, refused, window, cx)
                    });
                }
            }
            Outcome::Commands { project, listed } => {
                // The `/` lists of the project's sessions.
                for reply in self.replies.values() {
                    reply.update(cx, |reply, cx| reply.commands_listed(&project, &listed, cx));
                }
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).commands().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::CommandRead {
                project,
                name,
                read,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).commands().clone();
                    section.update(cx, |section, cx| {
                        section.read_whole(&name, read, window, cx)
                    });
                }
            }
            Outcome::CommandWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).commands().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::Developer { written, shown } => {
                self.settings
                    .update(cx, |page, cx| page.developer_answered(shown, written, cx));
            }
            Outcome::JudgeGuidance { written, shown } => {
                let editor = self.settings.read(cx).judge_guidance().clone();
                editor.update(cx, |editor, cx| editor.answered(shown, written, window, cx));
            }
            Outcome::Rules {
                project,
                written,
                shown,
            } => {
                let editor = match &project {
                    None => Some(self.rules.clone()),
                    Some(name) => self
                        .project_settings
                        .clone()
                        .filter(|page| page.read(cx).name() == name)
                        .map(|page| page.read(cx).rules().clone()),
                };
                if let Some(editor) = editor {
                    editor.update(cx, |editor, cx| editor.answered(shown, written, window, cx));
                }
            }
            Outcome::Skills { project, listed } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).skills().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::SkillRead {
                project,
                name,
                read,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).skills().clone();
                    section.update(cx, |section, cx| {
                        section.read_whole(&name, read, window, cx)
                    });
                }
            }
            Outcome::ClaudeFound {
                project,
                kind,
                found,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    match kind {
                        crate::actions::ClaudeKind::Skill => {
                            let section = page.read(cx).skills().clone();
                            section.update(cx, |section, cx| section.found_listed(found, cx));
                        }
                        crate::actions::ClaudeKind::Command => {
                            let section = page.read(cx).commands().clone();
                            section.update(cx, |section, cx| section.found_listed(found, cx));
                        }
                    }
                }
            }
            Outcome::ClaudeImported {
                project,
                kind,
                read,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    match kind {
                        crate::actions::ClaudeKind::Skill => {
                            let section = page.read(cx).skills().clone();
                            section.update(cx, |section, cx| section.imported(read, cx));
                        }
                        crate::actions::ClaudeKind::Command => {
                            let section = page.read(cx).commands().clone();
                            section.update(cx, |section, cx| section.imported(read, cx));
                        }
                    }
                }
            }
            Outcome::SkillWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).skills().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::Tasks { project, listed } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).tasks().clone();
                    section.update(cx, |section, cx| section.listed(listed.clone(), cx));
                }
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| tasks.listed(listed, cx));
                }
            }
            Outcome::TaskWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).tasks().clone();
                    section.update(cx, |section, cx| section.written(refused, cx));
                }
            }
            Outcome::TaskRun {
                project,
                name,
                refused,
            } => {
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    let _ = name;
                    tasks.update(cx, |tasks, cx| tasks.started(refused, cx));
                }
            }
            Outcome::ProjectRuns {
                project,
                listed,
                output,
            } => {
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| {
                        tasks.runs_listed(listed, output.map(|o| *o), cx)
                    });
                }
            }
            Outcome::ProjectRunAnswered { project, refused } => {
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| tasks.run_answered(refused, cx));
                }
            }
            Outcome::Setup { project, listed } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).setup().clone();
                    section.update(cx, |section, cx| section.listed(listed, cx));
                }
            }
            Outcome::SetupWritten { project, refused } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let section = page.read(cx).setup().clone();
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

    /// The tasks of the open project page, when it is `project`'s.
    fn project_tasks_of(
        &self,
        project: &str,
        cx: &App,
    ) -> Option<Entity<crate::tasks::ProjectTasks>> {
        let page = self.main.read(cx).project_page()?.read(cx);
        (page.name() == project).then(|| page.tasks().clone())
    }

    /// The open settings page, when it is `project`'s.
    fn settings_of_project(&self, project: &str, cx: &App) -> Option<Entity<ProjectSettingsPage>> {
        self.project_settings
            .clone()
            .filter(|page| page.read(cx).name() == project)
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
            folder: project.folder.clone(),
            steps: Vec::new(),
            preparation: Preparation::Ready,
            sessions: self
                .snapshot
                .sessions_in(&name, &None)
                .filter(|s| !s.ended)
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
            places.push(PlaceData {
                meta: Some(format!("from {}", workspace.bases.join(", "))),
                folder: workspace.folder.clone(),
                steps: workspace.steps.clone(),
                preparation: workspace.preparation.clone(),
                sessions: self
                    .snapshot
                    .sessions_in(&name, &place)
                    .filter(|s| !s.ended)
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
        let chosen = providers.judge.is_some();
        for reply in self.replies.values() {
            let mode = reply.read(cx).mode.clone();
            mode.update(cx, |mode, cx| mode.set_judge_chosen(chosen, cx));
        }
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
        // The notice of a judge Flottille picked comes and goes with them.
        let notice = self.judge_notice();
        self.providers = providers;
        if self.judge_notice() != notice {
            cx.notify();
        }
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

    /// The rows of `listed`, each the one the tree had or a new one, brought up to date; their
    /// marks are added to `marks`, and each row to `rows`.
    fn session_rows<'a>(
        &mut self,
        listed: impl Iterator<Item = &'a crate::model::Session>,
        rows: &mut HashMap<String, Entity<SessionRow>>,
        marks: &mut Vec<AgentState>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<SessionRow>> {
        let this = cx.entity().downgrade();
        let mut shown = Vec::new();
        for session in listed {
            let row = self.sessions.remove(&session.id).unwrap_or_else(|| {
                let this = this.clone();
                let id = session.id.clone();
                // A session opened before the tree showed it (one just started).
                let selected = self.selected == Some(Selection::Session(session.id.clone()));
                cx.new(|cx| {
                    let mut row = SessionRow::new(id, this, cx);
                    row.set_selected(selected, cx);
                    row
                })
            });
            // GPUI Fast takes any update of an entity for a change: touch only what changed.
            if !row
                .read(cx)
                .holds(&session.title, session.mark, session.active_at)
            {
                row.update(cx, |row, cx| {
                    row.set(&session.title, session.mark, session.active_at, now, cx)
                });
            }
            if row.read(cx).commands() != session.commands {
                row.update(cx, |row, cx| row.set_commands(session.commands, cx));
            }
            // Ended, it sits among other projects' sessions: its row says whose it is.
            let scope = session.ended.then_some(session.project.as_str());
            row.update(cx, |row, cx| {
                row.set_scope(scope, cx);
                row.set_actions(session.ended, session.resumable, cx)
            });
            marks.extend(session.mark);
            shown.push(row.clone());
            rows.insert(session.id.clone(), row);
        }
        shown
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
                let mut marks = Vec::new();
                let live = snapshot
                    .sessions_in(&project.name, &place)
                    .filter(|session| !session.ended);
                let rows = self.session_rows(live, &mut sessions, &mut marks, now, cx);
                let key = (project.name.clone(), place.clone());
                let node = self.places.remove(&key).unwrap_or_else(|| {
                    let (name, place) = key.clone();
                    let this = this.clone();
                    cx.new(|cx| PlaceNode::new(name, place, this, cx))
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
            if node.read(cx).commands() != project.commands {
                node.update(cx, |node, cx| node.set_commands(project.commands, cx));
            }
            project_nodes.push(node.clone());
            projects.insert(project.name.clone(), node);
        }
        // The ended sessions leave their places for one folded group below every project.
        let ended = snapshot.sessions.iter().filter(|session| session.ended);
        let mut marks = Vec::new();
        let rows = self.session_rows(ended, &mut sessions, &mut marks, now, cx);
        let present = !rows.is_empty();
        if self.ended.is_none() && present {
            self.ended = Some(cx.new(|cx| PlaceNode::ended(this.clone(), cx)));
        }
        if let Some(group) = self.ended.clone() {
            if !group.read(cx).holds(&rows, None, cx) {
                group.update(cx, |group, cx| group.set(rows, None, cx));
            }
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_ended(group, present, cx));
        }
        self.projects = projects;
        self.places = places;
        self.sessions = sessions;
        if self.sidebar.read(cx).projects() != project_nodes {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_projects(project_nodes, cx));
        }

        self.forms
            .retain(|name, _| snapshot.projects.iter().any(|p| &p.name == name));
        self.replies
            .retain(|id, _| snapshot.sessions.iter().any(|s| &s.id == id));
        let edited = snapshot.project_changes != self.snapshot.project_changes;
        self.snapshot = snapshot;
        self.ring(cx);

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
        // A project's file was edited (by hand, or by the command line): its repositories as the
        // page shows them may no longer be.
        if edited && let Some(page) = self.main.read(cx).project_page().cloned() {
            let section = page.read(cx).repositories().clone();
            section.update(cx, |section, cx| section.read(cx));
        }
        if edited && let Some(page) = self.project_settings.clone() {
            let section = page.read(cx).repositories().clone();
            section.update(cx, |section, cx| section.read(cx));
        }
    }

    /// A ticket or a pull request an agent changed: its session's row is marked and the bell
    /// rings, unless the session is the one open, where it is in sight.
    fn forged(&mut self, forged: Forged, cx: &mut Context<Self>) {
        let session = self.snapshot.root_of(&forged.agent);
        if self.selected == Some(Selection::Session(session)) {
            return;
        }
        self.forged.push(forged);
        self.ring(cx);
    }

    /// Brings the bell and the sessions' marks up to what waits for the user and what the
    /// agents changed: each item says why it is there, a short excerpt, and its session.
    fn ring(&mut self, cx: &mut Context<Self>) {
        let snapshot = &self.snapshot;
        let mut bell: Vec<BellItem> = snapshot.bell.clone();
        let mut changed: HashMap<String, &str> = HashMap::new();
        for forged in &self.forged {
            let session = snapshot.root_of(&forged.agent);
            changed.insert(session.clone(), &forged.said);
            bell.push(BellItem {
                key: match forged.ticket {
                    true => format!("ticket-{}", forged.seq),
                    false => format!("pull-{}", forged.seq),
                },
                session,
                label: forged.said.clone(),
                detail: forged.excerpt.clone(),
                at: forged.at,
            });
        }
        bell.sort_by_key(|item| item.at);
        for (id, row) in &self.sessions {
            let said = changed.get(id).copied();
            if row.read(cx).changed() != said {
                row.update(cx, |row, cx| row.set_changed(said, cx));
            }
        }
        let items: Vec<(BellItem, String)> = bell
            .into_iter()
            .map(|item| {
                let title = snapshot
                    .session(&item.session)
                    .map(|s| match &s.started_by {
                        Some(by) => format!("{} · started by {by}", s.title),
                        None => s.title.clone(),
                    })
                    .unwrap_or_default();
                (item, title)
            })
            .collect();
        let count = items.len();
        if self.top.read(cx).count != count {
            self.top.update(cx, |top, cx| top.set_count(count, cx));
        }
        if self.bell.read(cx).items != items {
            self.bell.update(cx, |bell, cx| bell.set(items, cx));
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
        }
        if let Some(page) = self.project_settings.clone() {
            // The project's file is the user's to edit by hand too.
            let project = page.read(cx).name().to_owned();
            self.actions.send(Command::Commands {
                project: project.clone(),
            });
            self.actions.send(Command::Skills {
                project: project.clone(),
            });
            self.actions.send(Command::Tasks {
                project: project.clone(),
            });
            self.actions.send(Command::Setup { project });
        }
    }

    /// The sentence of the notice that says which Auto judge Flottille picked, while the user
    /// has not seen it.
    pub fn judge_notice(&self) -> Option<String> {
        if self.judge_noticed || !self.providers.judge_picked {
            return None;
        }
        let (name, model) = self.providers.judge.as_ref()?;
        let title = self
            .providers
            .get(name)
            .map_or(name.as_str(), |p| p.title.as_str());
        let model = model.as_deref().unwrap_or("default model");
        Some(format!(
            "Flottille chose {title} {model} as your Auto judge: {title} is installed and signed \
             in here. In Auto, it decides the calls the rules do not know, on your quota. Change \
             it, or choose no judge, in Settings."
        ))
    }

    /// The notice was seen: it goes, and the daemon is told, so that it never comes back.
    pub fn dismiss_judge_notice(&mut self, cx: &mut Context<Self>) {
        if !self.judge_noticed {
            self.judge_noticed = true;
            self.actions.send(Command::JudgeNoticed);
            cx.notify();
        }
    }

    fn judge_notice_card(&self, text: String, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let buttons = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(div().flex_1())
            .child(
                Button::secondary("judge-notice-dismiss", "Dismiss")
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_judge_notice(cx))),
            )
            .child(
                Button::primary("judge-notice-open", "Open Settings").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.dismiss_judge_notice(cx);
                        this.open(Selection::Settings, window, cx);
                    },
                )),
            );
        let body = div()
            .px(space::LG)
            .py(space::MD)
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(
                div()
                    .id("judge-notice-text")
                    .aria_label(SharedString::from(text.clone()))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(text))
                    .test_support(),
            )
            .child(buttons);
        div()
            .id("judge-notice")
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .w(px(380.))
            .child(enter(
                "judge-notice-enter",
                0,
                Card::new().title("Auto judge chosen").child(body),
            ))
            .test_support()
    }

    /// Opens a project's settings at a repository's row or at its tasks, from its page.
    pub fn open_project_settings_at(
        &mut self,
        project: &str,
        at: crate::project_settings::Reveal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Selection::ProjectSettings(project.to_owned()), window, cx);
        if let Some(page) = self.settings_of_project(project, cx) {
            page.update(cx, |page, cx| page.reveal(at, cx));
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
        // Opened, the session's changes are seen: its mark and its items in the bell go.
        if let Selection::Session(id) = &selection {
            let before = self.forged.len();
            let snapshot = &self.snapshot;
            self.forged
                .retain(|forged| &snapshot.root_of(&forged.agent) != id);
            if self.forged.len() != before {
                self.ring(cx);
            }
        }
        // Opened, the page reads the providers again: the file is the user's to edit by hand.
        if selection == Selection::Settings && self.selected != Some(Selection::Settings) {
            self.actions.send(Command::ReadProviders);
            self.actions.send(Command::RulesRead { project: None });
            self.actions.send(Command::JudgeRead);
            self.actions.send(Command::DeveloperRead);
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
            let now = cx.background_executor().now();
            let duration = if open { ENTER } else { HOVER };
            self.bell_shown.animate(
                if open { 1.0 } else { 0.0 },
                duration,
                now,
                cx.reduce_motion(),
            );
            if open {
                self.bell.update(cx, |bell, cx| bell.show(true, cx));
            } else {
                // The marks stay while the list fades out.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(HOVER + Duration::from_millis(20))
                        .await;
                    this.update(cx, |this, cx| {
                        if !this.bell_open {
                            this.bell.update(cx, |bell, cx| bell.show(false, cx));
                        }
                    })
                    .ok();
                })
                .detach();
            }
            cx.notify();
        }
    }

    /// What the bell's items say of their sessions, in order: the session's title, and who
    /// started it when an agent did.
    pub fn bell_sessions(&self, cx: &App) -> Vec<String> {
        self.bell
            .read(cx)
            .items
            .iter()
            .map(|(_, title)| title.clone())
            .collect()
    }

    /// The list the bell opens.
    pub fn bell_list(&self) -> &Entity<BellList> {
        &self.bell
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let now = cx.background_executor().now();
        if self.bell_shown.is_running(now) {
            window.request_animation_frame();
        } else {
            self.bell_shown.settle(now);
        }
        let bell_in = self.bell_shown.value(now);
        let bell_drawn = self.bell_open || bell_in > 0.0;
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
            .when_some(self.judge_notice(), |d, text| {
                d.child(self.judge_notice_card(text, cx))
            })
            .child(self.add_project.clone())
            .when(bell_drawn, |d| {
                let scrim = div()
                    .id("bell-scrim")
                    .absolute()
                    .inset_0()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_bell(cx)));
                d.when(self.bell_open, |d| {
                    d.child(dismiss_on_navigate(scrim, move |_, cx| {
                        shell.update(cx, |this, cx| this.set_bell(false, cx)).ok();
                    }))
                })
                .child(
                    div()
                        .absolute()
                        .top(px(40. + RISE * (1.0 - bell_in)))
                        .right(px(12.))
                        .opacity(bell_in)
                        .child(self.bell.clone()),
                )
            })
            .child(self.toasts.clone())
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
            .px(space::MD)
            .flex()
            .items_center()
            .gap(space::MD)
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
                    .gap(space::SM)
                    .px(space::SM)
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
                    .px(space::MD)
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(space::SM)
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

/// The list the bell opens: what waits for the user and what the agents changed, oldest first.
/// Each item says why it is there, a short excerpt, and its session, each on one line cut with an
/// ellipsis. Its marks exist only while it is open, so that a closed list asks for no frame.
pub struct BellList {
    shell: WeakEntity<Shell>,
    items: Vec<(BellItem, String)>,
    list: Listing<(BellItem, String)>,
    /// The pulsing mark of each item that waits for the user, by its key.
    marks: HashMap<String, Entity<StateIcon>>,
    open: bool,
}

impl BellList {
    fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            items: Vec::new(),
            list: Listing::default(),
            marks: HashMap::new(),
            open: false,
        }
    }

    fn set(&mut self, items: Vec<(BellItem, String)>, cx: &mut Context<Self>) {
        if self.items != items {
            self.items = items;
            if self.open {
                self.show(true, cx);
            }
            cx.notify();
        }
    }

    fn show(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if !open {
            self.marks.clear();
        }
        for (item, _) in self.items.iter().filter(|(item, _)| waits(item)) {
            if open && !self.marks.contains_key(&item.key) {
                let status = cx.new(|_| Status::new(AgentState::NeedsYou));
                let mark = cx.new(|cx| StateIcon::new(status, cx));
                self.marks.insert(item.key.clone(), mark);
            }
        }
        cx.notify();
    }

    /// The words of each item, in order: why it is there, its excerpt, its session.
    pub fn said(&self) -> Vec<(String, String, String)> {
        self.items
            .iter()
            .map(|(item, session)| (item.label.clone(), item.detail.clone(), session.clone()))
            .collect()
    }
}

/// Whether a bell item waits for the user's answer, rather than tells them of a change.
fn waits(item: &BellItem) -> bool {
    item.key.starts_with("request-") || item.key.starts_with("question-")
}

/// The element id of a bell item's line under its title, of its excerpt, of its session.
pub fn bell_line_id(key: &str) -> ElementId {
    ElementId::Name(format!("bell-{key}-line").into())
}

pub fn bell_session_id(key: &str) -> ElementId {
    ElementId::Name(format!("bell-{key}-session").into())
}

impl Render for BellList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let at = cx.background_executor().now();
        self.list.sync(
            self.items
                .iter()
                .map(|(item, session)| (item.key.clone(), (item.clone(), session.clone()))),
            at,
            cx.reduce_motion(),
        );
        let list = div()
            .id("bell-list")
            .occlude()
            .w(px(340.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .p(space::SM)
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
        let rows: Vec<AnyElement> = self
            .list
            .rows()
            .map(|row| {
                let (item, session) = row.value;
                let shell = self.shell.clone();
                let key = item.key.clone();
                let leading = match self.marks.get(&item.key) {
                    Some(mark) => mark.clone().into_any_element(),
                    None => icon(match item.key.starts_with("ticket-") {
                        true => Icon::Ticket,
                        false => Icon::PullRequest,
                    })
                    .size(px(14.))
                    .text_color(p.acc)
                    .into_any_element(),
                };
                let line = |id: ElementId, text: &str, ink| {
                    div()
                        .id(id)
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.))
                        .text_color(ink)
                        .aria_label(SharedString::from(text.to_owned()))
                        .child(SharedString::from(text.to_owned()))
                        .test_support()
                };
                let below = div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when(!item.detail.is_empty(), |d| {
                        d.child(line(bell_line_id(&item.key), &item.detail, p.mute))
                    })
                    .child(line(bell_session_id(&item.key), session, p.faint));
                let content = ListRow::new(
                    ElementId::Name(format!("bell-{}", item.key).into()),
                    SharedString::from(item.label.clone()),
                )
                .leading(leading)
                .below(below)
                .on_click(move |_, window, cx| {
                    shell
                        .update(cx, |shell, cx| shell.choose(&key, window, cx))
                        .ok();
                });
                Item::new(
                    ElementId::Name(format!("bell-item-{}", row.key).into()),
                    row.placement,
                    content,
                )
                .into_any_element()
            })
            .collect();
        list.child(presence(
            "bell-empty",
            self.list.is_empty(),
            div()
                .p(space::MD)
                .text_color(p.faint)
                .child("Nothing waits for you."),
        ))
        .children(rows)
        .test_support()
        .into_any_element()
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
