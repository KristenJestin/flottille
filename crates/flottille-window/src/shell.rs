//! The window's frame: the top bar (the mark, search, the bell), the left tree, and the main area
//! showing the open page. It follows the daemon through the [`link`](crate::link) and draws
//! again only the parts a change touches.

use crate::glyphs;
use crate::state::{AgentState, StateIcon, Status};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui_design_system::color::mix;
use gpui_design_system::motion::{ENTER, HOVER, Motion, RISE};
use gpui_design_system::{
    Badge, Button, Card, Drawer, Icon, Item, ListRow, Listing, Side, Tie, Toast, ToastStack, Type,
    dismiss_on_navigate, dismiss_top_overlay, enter, forget_overlays, icon, keys_held,
    open_overlay, palette, presence, pressable, space, toast,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, ElementId, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, Modifiers, MouseButton, NavigationDirection,
    ParentElement, PathBuilder, PathStyle, Pixels, Render, SharedString,
    StatefulInteractiveElement, StrokeOptions, Styled, Subscription, Task, TestSupportExt as _,
    WeakEntity, Window, canvas, div, point, px,
};
use lyon_tessellation::{LineCap, LineJoin};

use crate::actions::{Actions, Command, Details, Outcome, Pick, RunFrom};
use crate::add_project::AddProject;
use crate::composer::{Choice, Composer, PlaceChoice};
use crate::extensions::Registry;
use crate::extensions::specs::SpecPage;
use crate::history::{History, Way};
use crate::link::{self, Connect, Seen, Update};
use crate::model::{BellItem, Forged, LinkShown, News, Place, Preparation, Snapshot};
use crate::open::Opener;
use crate::page::{MainArea, Page};
use crate::project::{NewWorkspace, PlaceData, ProjectData, SessionLine};
use crate::project_settings::ProjectSettingsPage;
use crate::providers::Providers;
use crate::rename::RenameSession;
use crate::rules::RulesEditor;
use crate::session::{Chose, Follow, Opened, Reply, SessionPage};
use crate::session_link::PullRequest;
use crate::settings::{Reveal, SettingsPage};
use crate::tree::{EndedNode, ProjectNode, SessionRow, Sidebar};
use crate::vault::VaultSection;
use crate::workspaces_folder::WorkspacesFolder;

pub mod switcher;
use switcher::Switcher;

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
    /// A spec's page, in its project.
    Spec {
        project: String,
        spec: u64,
    },
    /// The services of the machine.
    Services,
}

/// The links an agent shares in a row: the toast that says them is named by the agent and the
/// row's number, so that the next row is a toast of its own.
#[derive(Default)]
struct Sharing {
    row: u64,
    links: Vec<LinkShown>,
}

impl Sharing {
    /// The row ends: the next link starts another, and another toast.
    fn end(&mut self) {
        self.row += 1;
        self.links.clear();
    }
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
    /// The "Ended" group at the bottom of the tree, made when a session first ends.
    ended: Option<Entity<EndedNode>>,
    sessions: HashMap<String, Entity<SessionRow>>,
    /// The pull request of each workspace (its project and name) as a project page last read it:
    /// a session's row says it on its second line. The sidebar asks the forge for none of them.
    pulls: HashMap<(String, String), PullRequest>,
    /// The tickets and pull requests the sessions' agents created or changed, oldest first: each
    /// marks its session's row and rings the bell until the session is opened. They live as long
    /// as the window.
    forged: Vec<Forged>,
    /// What is new in each session since it was last opened, the last of it in words: an agent
    /// said something, reported, was woken, or changed a ticket or a pull request. A session's
    /// row says it with one dot, unless a state mark takes its place; they live as long as the
    /// window.
    news: HashMap<String, String>,
    /// The links each agent shared in a row, while the toast that says them is on screen: the
    /// toast counts them, and the agent's next words start a new row.
    sharing: HashMap<String, Sharing>,
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
    /// Each composer's changes, which the session's row says as a pencil while it is left.
    _drafts: Vec<Subscription>,
    /// The providers as the daemon last listed them, and the number of that read.
    providers: Providers,
    providers_read: u64,
    /// What the last session started from a composer started on, for this run of the window:
    /// a composer with no project default starts there.
    last_pick: Option<Pick>,
    settings: Entity<SettingsPage>,
    /// The services page, one for the window's life.
    services: Entity<crate::services::ServicesPage>,
    /// The editor of the user's rules, on the settings page.
    rules: Entity<RulesEditor>,
    /// The notice of the Auto judge Flottille picked was dismissed here, before the daemon says
    /// it was seen.
    judge_noticed: bool,
    /// The compactor Flottille picked has not been seen yet (`compaction.show`).
    compaction_picked: bool,
    /// The user saw the notice of the compactor picked.
    compaction_noticed: bool,
    /// The compactor's provider and model, as `compaction.show` last said them.
    compactor_shown: Option<(String, Option<String>)>,
    /// The last texts of the judge's and the compactor's notices, drawn while they fold away.
    notices: [Option<String>; 2],
    /// The number of the latest request about the appearance (see [`Shell::ask_appearance`]).
    appearance_asked: u64,
    /// Whether the texts the pages edit are to be read again when the link next reads the shell:
    /// an edit made elsewhere while the link did not listen is no event of its.
    texts_due: bool,
    /// The open project's settings page; made again for another project.
    project_settings: Option<Entity<ProjectSettingsPage>>,
    /// The form that adds a project, one for the window's life.
    add_project: Entity<AddProject>,
    /// The dialog that renames a session.
    rename: Entity<RenameSession>,
    /// The quick switcher (Ctrl K): sessions, projects and actions found by what is typed.
    switcher: Entity<Switcher>,
    /// What the window knows of the parts of Flottille it draws in its places (Specs).
    extensions: Entity<Registry>,
    /// The open spec's page; made again for another spec.
    spec_page: Option<Entity<SpecPage>>,
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

/// What a row's dot says: `New since you looked: opened a pull request`.
fn unread_words(said: &str) -> String {
    let mut letters = said.chars();
    let first = letters.next().into_iter().flat_map(char::to_lowercase);
    format!(
        "New since you looked: {}{}",
        first.collect::<String>(),
        letters.as_str()
    )
}

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
        let compaction_guidance = cx.new(|_| RulesEditor::compaction(this.clone()));
        let workspaces = cx.new(|_| WorkspacesFolder::new(this.clone(), None));
        let vault = cx.new(|_| VaultSection::new(this.clone()));
        let settings = cx.new(|_| {
            SettingsPage::new(
                this.clone(),
                rules.clone(),
                judge_guidance.clone(),
                compaction_guidance.clone(),
                workspaces.clone(),
                vault.clone(),
            )
        });
        let services = {
            let this = this.clone();
            cx.new(|cx| crate::services::ServicesPage::new(this, cx))
        };
        let main = {
            let (this, settings, services) = (this.clone(), settings.clone(), services.clone());
            cx.new(|cx| MainArea::new(this, settings, services, cx))
        };
        let left = cx.new(|_| Drawer::new("tree-drawer", Side::Left, px(SIDEBAR), sidebar.clone()));
        let bell = cx.new(|_| BellList::new(this.clone()));
        let toasts = cx.new(ToastStack::new);
        let add_project = {
            let this = this.clone();
            cx.new(|cx| AddProject::new(this, window, cx))
        };
        let rename = {
            let this = this.clone();
            cx.new(|cx| RenameSession::new(this, window, cx))
        };
        let focus = cx.focus_handle();
        let switcher = {
            let (this, focus) = (this.clone(), focus.clone());
            cx.new(|cx| Switcher::new(this, focus, window, cx))
        };
        let extensions = cx.new(|_| Registry::new(this.clone()));
        let reading = extensions.clone();
        cx.defer(move |cx| reading.update(cx, |registry, cx| registry.read(cx)));
        // What they list shows in many places: each is drawn again, and the page open is shown
        // as it is now (a spec's page goes where Specs is turned off).
        cx.observe_in(&extensions, window, |this, _, window, cx| {
            this.extensions_moved(window, cx)
        })
        .detach();
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
            focus,
            top,
            sidebar,
            main,
            left,
            bell,
            toasts,
            bell_open: false,
            bell_shown: Motion::settled(0.0),
            projects: HashMap::new(),
            ended: None,
            sessions: HashMap::new(),
            pulls: HashMap::new(),
            forged: Vec::new(),
            news: HashMap::new(),
            sharing: HashMap::new(),
            forms: HashMap::new(),
            actions,
            details_of: None,
            details_asked: 0,
            follow,
            session_page: None,
            _chose: None,
            replies: HashMap::new(),
            _drafts: Vec::new(),
            providers: Providers::default(),
            providers_read: 0,
            last_pick: None,
            settings,
            services,
            rules,
            judge_noticed: false,
            compaction_picked: false,
            compaction_noticed: false,
            compactor_shown: None,
            notices: [None, None],
            appearance_asked: 0,
            texts_due: true,
            project_settings: None,
            add_project,
            rename,
            switcher,
            extensions,
            spec_page: None,
            updates: Some(updates),
            _link: Task::ready(()),
            _clock,
        };
        shell.relayout(true, window, cx);
        // The colours the user chose, kept by the daemon for every window.
        shell.ask_appearance(None);
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
        // Shown, the composer is on Auto: a mode chosen for one session never reaches the next.
        forms.0.update(cx, |composer, cx| composer.reset_mode(cx));
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
        self.perform(Command::WorkspacesFolderRead {
            project: Some(name.clone()),
        });
        // The Tracker's select offers the entries the project sees.
        self.perform(Command::Vault);
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

    /// The dialog that renames a session.
    pub fn rename_form(&self) -> &Entity<RenameSession> {
        &self.rename
    }

    /// "Rename…" in a session's row or header: the dialog, its field holding the title it has.
    pub fn rename_of(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.snapshot.session(session).map(|s| s.title.clone()) else {
            return;
        };
        self.rename
            .update(cx, |form, cx| form.open(session, &title, window, cx));
    }

    /// The quick switcher.
    pub fn switcher(&self) -> &Entity<Switcher> {
        &self.switcher
    }

    /// What the window knows of the parts of Flottille it draws in its places.
    pub fn extensions(&self) -> &Entity<Registry> {
        &self.extensions
    }

    /// The open spec's page, made the first time and kept while the same spec is open; opened,
    /// it asks the daemon for the spec.
    fn spec_page_for(&mut self, page: &Page, cx: &mut Context<Self>) -> Option<Entity<SpecPage>> {
        let Page::Spec { project, spec } = page else {
            self.spec_page = None;
            return None;
        };
        if let Some(open) = self.spec_page.clone()
            && open.read(cx).spec() == *spec
        {
            return Some(open);
        }
        let this = cx.entity().downgrade();
        let made = cx.new(|_| SpecPage::new(this, project.clone(), *spec));
        let reading = made.clone();
        cx.defer(move |cx| reading.update(cx, |page, cx| page.read(cx)));
        self.spec_page = Some(made.clone());
        Some(made)
    }

    /// What the registry lists changed: the pages that draw it are drawn again, and the page open
    /// is shown as it is now.
    fn extensions_moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.update(cx, |_, cx| cx.notify());
        let features = self.settings.read(cx).features(&self.extensions, cx);
        self.switcher
            .update(cx, |switcher, cx| switcher.set_features(features, cx));
        if let Some(page) = self.project_settings.clone() {
            page.update(cx, |_, cx| cx.notify());
        }
        if let Some(page) = self.main.read(cx).project_page().cloned() {
            let items = page.read(cx).items().clone();
            items.update(cx, |items, cx| items.extensions_moved(cx));
        }
        if let Some(page) = self.spec_page.clone() {
            page.update(cx, |_, cx| cx.notify());
        }
        if let Some(selection) = self.selected.clone() {
            let page = self.page_of(&selection, cx);
            if self.main.read(cx).page() != &page {
                let forms = self.forms_for(&page, window, cx);
                let session = self.session_for(&page, window, cx);
                let settings = self.project_settings_for(&page, window, cx);
                let spec = self.spec_page_for(&page, cx);
                self.main.update(cx, |main, cx| {
                    main.set_spec_page(spec);
                    main.show(page, forms, session, settings, cx)
                });
            }
        }
    }

    /// The window's own focus: what has it when nothing else does.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Opens or closes the quick switcher: Ctrl K, or a click on the top bar's search. An open
    /// dialog or list (Add a project, a confirmation, a menu) keeps the keys: it is left first.
    pub fn toggle_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.switcher.read(cx).is_open() && keys_held(cx) {
            return;
        }
        self.set_bell(false, cx);
        self.left.update(cx, |drawer, cx| drawer.close(cx));
        self.switcher
            .update(cx, |switcher, cx| switcher.toggle(window, cx));
    }

    /// Opens Settings on the tab of that section of [`crate::settings::SECTIONS`].
    pub fn open_settings_tab(&mut self, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open(Selection::Settings, window, cx);
        self.settings.update(cx, |page, cx| page.choose(tab, cx));
    }

    /// What opens a folder in the user's editor, for the session pages opened from now on.
    pub fn set_opener(&mut self, opener: Opener) {
        self.follow.opener = opener;
    }

    /// Opens a link (a pull request) with the system's default handler.
    pub fn open_link(&self, url: &str) {
        (self.follow.opener)(std::path::Path::new(url));
    }

    /// Tells a session's row whether its composer holds a draft; the row says it as a pencil
    /// while the session is not the open one.
    fn say_draft(&self, session: &str, cx: &mut Context<Self>) {
        let held = self
            .replies
            .get(session)
            .is_some_and(|reply| reply.read(cx).holds_draft());
        if let Some(row) = self.sessions.get(session)
            && row.read(cx).draft() != held
        {
            row.update(cx, |row, cx| row.set_draft(held, cx));
        }
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
        let (project, started_by, led_by) = (
            session.project.clone(),
            session.started_by.clone(),
            session.led_by.clone(),
        );
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
            led_by,
        };
        let this = cx.entity().downgrade();
        let reply = match self.replies.get(id) {
            Some(reply) => reply.clone(),
            None => {
                let (shell, project) = (this.clone(), opened.project.clone());
                let session = id.clone();
                let extensions = self.extensions.downgrade();
                let reply = cx.new(|cx| {
                    Reply::new(shell, session, project, window, cx).with_extensions(extensions)
                });
                self._drafts.push(cx.observe(&reply, |shell, reply, cx| {
                    let session = reply.read(cx).session().to_owned();
                    shell.say_draft(&session, cx);
                }));
                self.replies.insert(id.clone(), reply.clone());
                reply
            }
        };
        if self.providers.known {
            let mode = reply.read(cx).mode.clone();
            let chosen = self.providers.judge.is_some();
            mode.update(cx, |mode, cx| mode.set_judge_chosen(chosen, cx));
        }
        let (follow, layout, status) = (self.follow.clone(), self.layout, status.clone());
        let page = cx.new(|cx| SessionPage::new(this, opened, status, reply, follow, layout, cx));
        let providers = self.providers.clone();
        page.update(cx, |page, cx| page.set_providers(&providers, cx));
        self._chose = Some(cx.subscribe(&page, |shell, page, Chose(agent), cx| {
            let session = page.read(cx).id().to_owned();
            shell.history.show_agent(&session, agent);
        }));
        self.session_page = Some(page.clone());
        // The rail's Run a task lists the project's tasks.
        let project = page.read(cx).project().to_owned();
        self.actions.send(Command::Tasks { project });
        Some(page)
    }

    /// Asks the daemon, off the UI thread; the answer comes back as an update.
    pub fn perform(&self, command: Command) {
        self.actions.send(command);
    }

    /// Asks the daemon to keep `choice` as the appearance, or what it keeps when there is none.
    /// Each request is numbered; the window only believes the answer to the latest.
    pub fn ask_appearance(&mut self, choice: Option<gpui_design_system::Appearance>) {
        self.appearance_asked += 1;
        let seq = self.appearance_asked;
        self.actions.send(match choice {
            Some(appearance) => Command::AppearanceSet { appearance, seq },
            None => Command::AppearanceRead { seq },
        });
    }

    /// A place's "New session here": the composer, set to start there.
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

    /// "End session" in a session's row: the confirmation, which counts the agents that end.
    pub fn ask_end_of(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(found) = self.snapshot.session(session) else {
            return;
        };
        let (id, count) = (found.id.clone(), found.live);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.ask_end(id, count, cx));
    }

    /// "Detach from …" in the row of a session an agent started: the session becomes the
    /// user's alone, and the result is a toast.
    pub fn detach_of(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(from) = self
            .snapshot
            .session(session)
            .and_then(|found| found.started_by.clone())
        else {
            return;
        };
        self.perform(Command::Detach {
            session: session.to_owned(),
            from,
        });
        cx.notify();
    }

    /// A row folded or unfolded the led sessions under it: its project's level is drawn again,
    /// from what the daemon last said.
    pub fn relist(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(project) = self.snapshot.session(session).map(|s| s.project.clone()) else {
            return;
        };
        let live = self.snapshot.live_sessions_of(&project);
        let rows: Vec<Entity<SessionRow>> = live
            .iter()
            .filter_map(|s| self.sessions.get(&s.id).cloned())
            .collect();
        if rows.len() != live.len() {
            return;
        }
        let rows = nested(&live, &rows, cx);
        if let Some(node) = self.projects.get(&project).cloned() {
            node.update(cx, |node, cx| node.set_rows(rows, cx));
        }
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

    pub fn session(&self, id: &str) -> Option<&Entity<SessionRow>> {
        self.sessions.get(id)
    }

    /// The "Ended" group at the bottom of the tree, once a session ended.
    pub fn ended(&self) -> Option<&Entity<EndedNode>> {
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
            Update::Disconnected(_) => {
                self.set_online(false, cx);
                self.texts_due = true;
            }
            // The link subscribed to the events before it read: an edit made before that is in
            // what the pages read, not in the events, so they read it again.
            Update::Snapshot(snapshot) => {
                self.set_online(true, cx);
                self.show(snapshot, window, cx);
                if std::mem::take(&mut self.texts_due) {
                    self.read_texts(cx);
                }
            }
            Update::Done(outcome) => self.done(outcome, window, cx),
            Update::Providers(read, providers) => self.set_providers(read, providers, cx),
            Update::TextChanged { judge: true, .. } => self.actions.send(Command::JudgeRead),
            Update::DeveloperChanged => self.actions.send(Command::DeveloperRead),
            Update::ToolResultsChanged => self.actions.send(Command::ToolResultsRead),
            Update::CompactionChanged => self.actions.send(Command::CompactionRead),
            Update::SessionsChanged => self.actions.send(Command::SessionsRead),
            Update::VaultChanged => self.actions.send(Command::Vault),
            Update::ServicesChanged => {
                if self.main.read(cx).services_page().is_some() {
                    self.actions.send(Command::Services);
                }
            }
            // A default that changed changes the folder every project shows, so the open
            // project's section reads again too.
            Update::WorkspacesFolderChanged { project } => {
                self.actions.send(Command::WorkspacesFolderRead {
                    project: project.clone(),
                });
                if let Some(open) = self
                    .project_settings
                    .as_ref()
                    .map(|page| page.read(cx).name())
                    && project.as_deref().is_none_or(|name| name != open)
                {
                    self.actions.send(Command::WorkspacesFolderRead {
                        project: Some(open.to_owned()),
                    });
                }
            }
            Update::AppearanceChanged => self.ask_appearance(None),
            Update::Extensions => {
                self.extensions
                    .update(cx, |registry, cx| registry.changed(cx));
                if let Some(page) = self.spec_page.clone() {
                    page.update(cx, |page, cx| page.read(cx));
                }
            }
            Update::Forged(forged) => self.forged(forged, cx),
            Update::News(news) => self.news(news, cx),
            Update::Link(link) => self.linked(link, cx),
            Update::Prompted(agent) => self.spoke(&agent),
            Update::Items { project, item } => {
                if let Some(items) = self.project_items_of(&project, cx) {
                    items.update(cx, |items, cx| items.changed(item, cx));
                }
            }
            Update::TextChanged { project, .. } => {
                // A project's editor shows the global rules above its own.
                if project.is_none()
                    && let Some(open) = self
                        .project_settings
                        .as_ref()
                        .map(|page| page.read(cx).name().to_owned())
                {
                    self.actions.send(Command::RulesRead {
                        project: Some(open),
                    });
                }
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
                self.read_pulls(&details, cx);
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
            Outcome::VaultAnswered {
                session,
                ask,
                refused,
            } => {
                if let Some(page) = self
                    .session_page
                    .clone()
                    .filter(|page| page.read(cx).id() == session)
                {
                    page.update(cx, |page, cx| page.vault_answered(ask, refused, window, cx));
                }
            }
            Outcome::QuestionAnswered {
                session,
                seq,
                refused,
            } => {
                // Kept by the window, not the page: a page opened again knows it was sent.
                if refused.is_none() {
                    crate::question_card::answers_sent(&session, seq, cx);
                }
                if let Some(page) = self
                    .session_page
                    .clone()
                    .filter(|page| page.read(cx).id() == session)
                {
                    page.update(cx, |page, cx| {
                        page.question_answered(seq, refused, window, cx)
                    });
                }
            }
            Outcome::Replied {
                session,
                key,
                answer,
            } => {
                if let Some(reply) = self.replies.get(&session).cloned() {
                    reply.update(cx, |reply, cx| reply.replied(key, answer, cx));
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
                    Err(why) => gpui_design_system::toast(
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
                    gpui_design_system::toast(
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
            Outcome::Renamed { refused } => gpui_design_system::toast(
                match refused {
                    None => gpui_design_system::Toast::success("Session renamed"),
                    Some(why) => {
                        gpui_design_system::Toast::failure("Session not renamed").detail(why)
                    }
                },
                cx,
            ),
            Outcome::Detached { from, refused } => gpui_design_system::toast(
                match refused {
                    None => {
                        gpui_design_system::Toast::success("Detached from ").quoted_title(&from)
                    }
                    Some(why) => gpui_design_system::Toast::failure("Not detached from ")
                        .quoted_title(&from)
                        .detail(why),
                },
                cx,
            ),
            Outcome::ToolsReloaded {
                session,
                after_turn,
                refused,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.tools_reloaded(after_turn, refused, cx));
                } else if let Some(why) = refused {
                    gpui_design_system::toast(
                        gpui_design_system::Toast::failure(crate::session::NOT_RELOADED)
                            .detail(why),
                        cx,
                    );
                }
            }
            Outcome::QuotaResumed { session, refused } => {
                if let Some(reply) = self.replies.get(&session) {
                    reply.update(cx, |reply, cx| reply.quota_answered(refused, cx));
                }
            }
            Outcome::ModeSet { session, refused } => {
                if let Some(reply) = self.replies.get(&session) {
                    let mode = reply.read(cx).mode.clone();
                    mode.update(cx, |mode, cx| mode.answered(refused, cx));
                }
            }
            Outcome::OptionsSet { session, refused } => {
                if let Some(reply) = self.replies.get(&session) {
                    let options = reply.read(cx).options().clone();
                    options.update(cx, |options, cx| options.answered(refused, cx));
                }
            }
            Outcome::RunAnswered { session, answer } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.command_answered(answer, cx));
                }
            }
            Outcome::SubscriptionCancelled { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.subscription_answered(refused, cx));
                }
            }
            Outcome::LeaveWithdrawn { session, refused } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.leave_answered(refused, cx));
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
            Outcome::NewSessionSet { refused } => {
                if refused.is_none() {
                    self.actions.send(Command::ReadProviders);
                }
                self.settings
                    .update(cx, |page, cx| page.new_session_answered(refused, cx));
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
            Outcome::Convention {
                project,
                key,
                saved,
            } => {
                let section = self
                    .project_settings
                    .clone()
                    .filter(|page| page.read(cx).name() == project)
                    .map(|page| page.read(cx).convention(key).clone());
                if let Some(section) = section {
                    section.update(cx, |section, cx| section.answered(saved, window, cx));
                }
            }
            Outcome::MessageChecks { project, saved } => {
                let section = self
                    .project_settings
                    .clone()
                    .filter(|page| page.read(cx).name() == project)
                    .map(|page| page.read(cx).message_checks().clone());
                if let Some(section) = section {
                    section.update(cx, |section, cx| section.answered(saved, window, cx));
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
            Outcome::Programs {
                project,
                read,
                serial,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let checks = page.read(cx).programs().clone();
                    checks.update(cx, |checks, cx| checks.answered(serial, read, cx));
                }
            }
            // Detect again is the settings' own read of the repositories now.
            Outcome::ProjectChecked { .. } => {}
            Outcome::RemovalRead {
                project,
                serial,
                read,
            } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let removal = page.read(cx).removal().clone();
                    removal.update(cx, |removal, cx| removal.answered(serial, read, cx));
                }
            }
            Outcome::ProjectRemoved { project, result } => {
                if let Some(page) = self.settings_of_project(&project, cx) {
                    let removal = page.read(cx).removal().clone();
                    removal.update(cx, |removal, cx| removal.removed(result.clone(), cx));
                } else if let Ok(done) = &result {
                    // The page was left meanwhile: the toast is said all the same.
                    let mut said = Toast::success(format!("The project {project} was removed"));
                    if let Some(detail) = crate::project_removal::done_said(done) {
                        said = said.detail(detail);
                    }
                    toast(said, cx);
                }
                if result.is_ok() {
                    self.forms.remove(&project);
                    self.snapshot.projects.retain(|p| p.name != project);
                    let next = self.snapshot.projects.first().map(|p| p.name.clone());
                    match next {
                        Some(next) => self.open(Selection::Project(next), window, cx),
                        None => {
                            self.selected = None;
                            self.project_settings = None;
                            self.spec_page = None;
                            self.main.update(cx, |main, cx| {
                                main.set_spec_page(None);
                                main.show(Page::Empty, None, None, None, cx)
                            });
                        }
                    }
                }
            }
            Outcome::ProjectDetected { seq, read, .. } => {
                self.add_project
                    .update(cx, |form, cx| form.detect_answered(seq, read, cx));
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
                    && let Some(block) = page.read(cx).block(&Some(name.clone()), cx)
                {
                    block.update(cx, |block, cx| block.ticket_answered(refused.clone(), cx));
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
            Outcome::Vault { listed } => {
                // A project's Tracker offers the entries it sees to log time with.
                if let (Some(page), Ok(entries)) = (self.project_settings.clone(), &listed) {
                    page.update(cx, |page, cx| page.vault_listed(entries, cx));
                }
                let section = self.settings.read(cx).vault().clone();
                section.update(cx, |section, cx| section.listed(listed, cx));
            }
            Outcome::VaultWritten { refused } => {
                let section = self.settings.read(cx).vault().clone();
                section.update(cx, |section, cx| section.written(refused, cx));
                // An entry added from a project's Tracker: back to it.
                if let Some(project) = section.update(cx, |section, _| section.take_return()) {
                    self.open_project_settings_at(
                        &project,
                        crate::project_settings::Reveal::Tracker,
                        window,
                        cx,
                    );
                }
            }
            Outcome::Developer { written, shown } => {
                self.settings
                    .update(cx, |page, cx| page.developer_answered(shown, written, cx));
            }
            Outcome::Asked { then, answer } => {
                let page = match &then {
                    crate::extensions::Then::Specs(specs) => specs.for_page(),
                    _ => None,
                };
                match (page, self.spec_page.clone()) {
                    (Some(spec), Some(open)) if open.read(cx).spec() == spec => {
                        let crate::extensions::Then::Specs(then) = then else {
                            unreachable!("a page's answer is a spec's")
                        };
                        open.update(cx, |page, cx| page.answered(then, answer, cx));
                    }
                    _ => self
                        .extensions
                        .update(cx, |registry, cx| registry.answered(then, answer, cx)),
                }
            }
            Outcome::ToolResults { written, shown } => {
                self.settings.update(cx, |page, cx| {
                    page.tool_results_answered(shown, written, cx)
                });
            }
            Outcome::Sessions { written, shown } => {
                self.settings
                    .update(cx, |page, cx| page.sessions_answered(shown, written, cx));
            }
            // Only the answer to the latest request counts: an older one would take the window
            // back to a choice the user has since changed.
            Outcome::Appearance { seq, .. } if seq != self.appearance_asked => {}
            Outcome::Appearance { written, shown, .. } => {
                match &shown {
                    Ok(appearance) => {
                        gpui_design_system::appearance::appear(appearance.clone(), cx)
                    }
                    // A choice not kept: the window goes back to what the daemon keeps.
                    Err(_) if written => self.ask_appearance(None),
                    Err(_) => {}
                }
                self.settings
                    .update(cx, |page, cx| page.appearance_answered(shown, written, cx));
            }
            Outcome::Compaction {
                written,
                guidance,
                shown,
            } => {
                let compaction = shown
                    .as_ref()
                    .map(crate::settings::Compaction::of)
                    .map_err(Clone::clone);
                if let Ok(shown) = &compaction {
                    self.compaction_picked = shown.picked;
                    self.compactor_shown = shown
                        .compactor
                        .clone()
                        .map(|(provider, model, _)| (provider, model));
                }
                if guidance || !written {
                    let editor = self.settings.read(cx).compaction_guidance().clone();
                    let said = shown
                        .as_ref()
                        .map(|shown| crate::rules::Shown::of(&shown["guidance"]))
                        .map_err(Clone::clone);
                    editor.update(cx, |editor, cx| editor.answered(said, guidance, window, cx));
                }
                if !guidance {
                    self.settings.update(cx, |page, cx| {
                        page.compaction_answered(compaction, written, cx)
                    });
                }
                cx.notify();
            }
            Outcome::Compacting {
                session,
                after_turn,
                refused,
            } => {
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).id() == session
                {
                    page.update(cx, |page, cx| page.compacting(after_turn, refused, cx));
                } else if let Some(why) = refused {
                    gpui_design_system::toast(
                        gpui_design_system::Toast::failure(crate::session::NOT_COMPACTED)
                            .detail(why),
                        cx,
                    );
                }
            }
            Outcome::Guidance {
                name,
                part,
                written,
                shown,
            } => {
                let registry = self.extensions.read(cx);
                let editor = match &part {
                    Some(part) => registry.check_guidance(&name, part),
                    None => registry.guidance(&name),
                }
                .cloned();
                if let Some(editor) = editor {
                    editor.update(cx, |editor, cx| editor.answered(shown, written, window, cx));
                }
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
            Outcome::RulesSwitched { project, shown } => {
                let editor = self
                    .project_settings
                    .clone()
                    .filter(|page| page.read(cx).name() == project)
                    .map(|page| page.read(cx).rules().clone());
                if let Some(editor) = editor {
                    editor.update(cx, |editor, cx| editor.switched(shown, cx));
                }
            }
            Outcome::WorkspacesFolder {
                project,
                written,
                shown,
            } => {
                let section = match &project {
                    None => Some(self.settings.read(cx).workspaces_folder().clone()),
                    Some(name) => self
                        .project_settings
                        .clone()
                        .filter(|page| page.read(cx).name() == name)
                        .map(|page| page.read(cx).workspaces_folder().clone()),
                };
                if let Some(section) = section {
                    section.update(cx, |section, cx| {
                        section.answered(shown, written, window, cx)
                    });
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
                // The rail of a session of the project offers them to run.
                if let Some(page) = self.session_page.clone()
                    && page.read(cx).project() == project
                    && let Ok(tasks) = &listed
                {
                    let names = tasks.iter().map(|task| task.name.clone()).collect();
                    let list = page.read(cx).commands(cx);
                    list.update(cx, |list, cx| list.set_tasks(names, cx));
                }
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| tasks.listed(listed, cx));
                }
            }
            Outcome::Items {
                project,
                asked,
                read,
            } => {
                if let Some(items) = self.project_items_of(&project, cx) {
                    items.update(cx, |items, cx| items.listed(asked, read, cx));
                }
            }
            Outcome::Item {
                project,
                kind,
                key,
                repository,
                asked,
                shown,
            } => {
                if let Some(items) = self.project_items_of(&project, cx) {
                    items.update(cx, |items, cx| {
                        items.shown(&kind, &key, repository, asked, shown, cx)
                    });
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
                from,
                refused,
            } => match from {
                RunFrom::Page { .. } => {
                    if let Some(tasks) = self.project_tasks_of(&project, cx) {
                        tasks.update(cx, |tasks, cx| tasks.started(refused, cx));
                    }
                }
                RunFrom::Settings => crate::tasks::ran(&name, None, refused, cx),
                RunFrom::Session { workspace } => {
                    crate::tasks::ran(&name, Some(workspace.as_deref()), refused, cx);
                }
            },
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
            Outcome::ProjectRunAnswered { project, answer } => {
                if let Some(tasks) = self.project_tasks_of(&project, cx) {
                    tasks.update(cx, |tasks, cx| tasks.run_answered(answer, cx));
                }
            }
            Outcome::Services { listed } => {
                // The open task form offers them as what a task may need, or says why it cannot.
                if let Some(page) = self.project_settings.clone() {
                    let names = listed
                        .as_ref()
                        .map(|services| services.iter().map(|s| s.name.clone()).collect())
                        .map_err(Clone::clone);
                    let section = page.read(cx).tasks().clone();
                    section.update(cx, |section, cx| section.set_services(names, cx));
                }
                self.services.update(cx, |page, cx| page.listed(listed, cx));
            }
            Outcome::ServiceWritten { refused } => {
                self.services
                    .update(cx, |page, cx| page.written(refused, cx));
            }
            Outcome::ServiceActed {
                name,
                start,
                answer,
            } => {
                self.services
                    .update(cx, |page, cx| page.acted(&name, start, answer, cx));
            }
            Outcome::ServiceOutput { output } => {
                self.services
                    .update(cx, |page, cx| page.output(output.map(|o| *o), cx));
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

    /// The pull requests and tickets of the open project page, when it is `project`'s.
    fn project_items_of(
        &self,
        project: &str,
        cx: &App,
    ) -> Option<Entity<crate::items::ProjectItems>> {
        let page = self.main.read(cx).project_page()?.read(cx);
        (page.name() == project).then(|| page.items().clone())
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
            && let Some(block) = page.read(cx).block(&Some(name.to_owned()), cx)
        {
            block.update(cx, |block, cx| block.answered(refused, cx));
        } else if let Some(page) = self.session_page.clone() {
            // A removal asked from the session's rail.
            page.update(cx, |page, cx| {
                page.removal_answered(project, name, refused, cx)
            });
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
        let sessions_of: Vec<&crate::model::Session> = self
            .snapshot
            .sessions
            .iter()
            .filter(|s| s.project == name && !s.ended)
            .collect();
        let line = |session: &crate::model::Session, cx: &App| {
            // The sessions it leads, and the one that leads it: each said once, where it works.
            let leads = sessions_of
                .iter()
                .filter(|led| led.pilot.as_deref() == Some(session.id.as_str()))
                .map(|led| (led.id.clone(), led.place.clone()))
                .collect();
            let pilot = session.pilot.as_ref().map(|pilot| {
                let title = sessions_of
                    .iter()
                    .find(|s| &s.id == pilot)
                    .map(|s| s.title.clone())
                    .or_else(|| session.led_by.clone())
                    .unwrap_or_else(|| pilot.clone());
                (pilot.clone(), title)
            });
            let question = (session.mark == Some(crate::state::AgentState::NeedsYou))
                .then(|| {
                    self.snapshot
                        .bell
                        .iter()
                        .rev()
                        .find(|item| item.session == session.id)
                        .map(|item| item.detail.clone())
                })
                .flatten();
            self.sessions.get(&session.id).map(|row| SessionLine {
                id: session.id.clone(),
                title: session.title.clone(),
                agents: session.agents,
                mark: session.mark,
                active_at: session.active_at,
                status: row.read(cx).status(),
                leads,
                pilot,
                question,
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
        // Work lists the workspaces from the newest to the oldest, in an order nothing changes.
        let mut workspaces: Vec<&crate::model::Workspace> = self
            .snapshot
            .workspaces
            .iter()
            .filter(|w| w.project == name)
            .collect();
        workspaces.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        for workspace in workspaces {
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
        // The composer's place menu keeps its order by name.
        choices[1..].sort_by(|a, b| a.workspace.cmp(&b.workspace));
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
        // A provider ready since may be the compactor Flottille picks: the daemon says it.
        self.actions.send(Command::CompactionRead);
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
        if let Some(page) = self.session_page.clone() {
            page.update(cx, |page, cx| page.set_providers(&providers, cx));
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

    /// Asks the daemon for what the settings page edits.
    fn read_settings(&mut self) {
        self.actions.send(Command::RulesRead { project: None });
        self.actions.send(Command::JudgeRead);
        self.actions.send(Command::CompactionRead);
        self.actions.send(Command::DeveloperRead);
        self.actions.send(Command::ToolResultsRead);
        self.actions.send(Command::SessionsRead);
        self.actions
            .send(Command::WorkspacesFolderRead { project: None });
        self.actions.send(Command::Vault);
        self.ask_appearance(None);
    }

    /// Reads again what the window shows of the daemon's settings, once the link listens to the
    /// events: a page opened later reads for itself.
    fn read_texts(&mut self, cx: &mut Context<Self>) {
        if self.selected == Some(Selection::Settings) {
            self.read_settings();
        } else {
            self.ask_appearance(None);
        }
        if let Some(name) = self
            .project_settings
            .as_ref()
            .map(|page| page.read(cx).name().to_owned())
        {
            self.actions.send(Command::RulesRead {
                project: Some(name.clone()),
            });
            self.actions.send(Command::WorkspacesFolderRead {
                project: Some(name),
            });
        }
    }

    fn set_online(&mut self, online: bool, cx: &mut Context<Self>) {
        if self.top.read(cx).online != online {
            self.top.update(cx, |top, cx| top.set_online(online, cx));
        }
    }

    /// What a project page read of its workspaces' pull requests: their sessions' rows say the
    /// first of each on their second line. A workspace the page could not read keeps what it had.
    fn read_pulls(&mut self, details: &Details, cx: &mut Context<Self>) {
        for (name, pulls) in &details.pulls {
            let key = (details.project.clone(), name.clone());
            match pulls.first() {
                Some(pull) => self.pulls.insert(key, pull.clone()),
                None => self.pulls.remove(&key),
            };
        }
        for session in &self.snapshot.sessions {
            let Some(row) = self.sessions.get(&session.id) else {
                continue;
            };
            let pull = self.pull_of(&session.project, &session.place);
            if row.read(cx).pull() != pull {
                row.update(cx, |row, cx| row.set_pull(pull, cx));
            }
        }
    }

    /// The pull request of the place a session works in, when the window has read it.
    fn pull_of(&self, project: &str, place: &Place) -> Option<&PullRequest> {
        let workspace = place.as_ref()?;
        self.pulls.get(&(project.to_owned(), workspace.clone()))
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
            if row.read(cx).said() != session.mark_said {
                row.update(cx, |row, cx| row.set_said(&session.mark_said, cx));
            }
            if row.read(cx).waits() != session.waits {
                row.update(cx, |row, cx| row.set_waits(&session.waits, cx));
            }
            if row.read(cx).commands() != session.commands {
                row.update(cx, |row, cx| row.set_commands(session.commands, cx));
            }
            let held = self
                .replies
                .get(&session.id)
                .is_some_and(|reply| reply.read(cx).holds_draft());
            if row.read(cx).draft() != held {
                row.update(cx, |row, cx| row.set_draft(held, cx));
            }
            // Ended, it sits among other projects' sessions: its row says whose it is.
            let scope = session.ended.then_some(session.project.as_str());
            let pull = self.pull_of(&session.project, &session.place).cloned();
            row.update(cx, |row, cx| {
                row.set_scope(scope, cx);
                row.set_place(&session.place, cx);
                row.set_pull(pull.as_ref(), cx);
                row.set_actions(
                    session.ended,
                    session.resumable,
                    session.started_by.as_deref(),
                    cx,
                );
                if session.ended {
                    row.set_led_by(None, cx);
                }
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
        let mut sessions = HashMap::new();
        let mut project_nodes = Vec::new();
        // A workspace that is gone takes its pull request with it.
        self.pulls
            .retain(|(project, name), _| snapshot.workspace(project, name).is_some());
        for project in &snapshot.projects {
            let mut marks = Vec::new();
            let live = snapshot.live_sessions_of(&project.name);
            let rows = self.session_rows(live.iter().copied(), &mut sessions, &mut marks, now, cx);
            let rows = nested(&live, &rows, cx);
            let node = self.projects.remove(&project.name).unwrap_or_else(|| {
                let name = project.name.clone();
                let this = this.clone();
                cx.new(|cx| ProjectNode::new(name, this, cx))
            });
            let mark = AgentState::strongest(marks);
            if !node.read(cx).holds(&rows, mark, cx) {
                node.update(cx, |node, cx| node.set(rows, mark, cx));
            }
            if node.read(cx).commands() != project.commands {
                node.update(cx, |node, cx| node.set_commands(project.commands, cx));
            }
            project_nodes.push(node.clone());
            projects.insert(project.name.clone(), node);
        }
        // The ended sessions leave their places for one folded group below every project.
        let ended = snapshot.ended_sessions();
        let mut marks = Vec::new();
        let rows = self.session_rows(ended.iter().copied(), &mut sessions, &mut marks, now, cx);
        let present = !rows.is_empty();
        if self.ended.is_none() && present {
            self.ended = Some(cx.new(EndedNode::new));
        }
        if let Some(group) = self.ended.clone() {
            if !group.read(cx).holds(&rows, cx) {
                group.update(cx, |group, cx| group.set(rows, cx));
            }
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_ended(group, present, cx));
        }
        self.projects = projects;
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
        // A session detached from its pilot: its open page stops saying who led it.
        if let Some(page) = self.session_page.clone()
            && let Some(found) = self.snapshot.session(page.read(cx).id())
            && !page.read(cx).says_by(&found.started_by, &found.led_by)
        {
            let (started_by, led_by) = (found.started_by.clone(), found.led_by.clone());
            page.update(cx, |page, cx| page.set_by(started_by, led_by, cx));
        }
        self.switcher
            .update(cx, |switcher, cx| switcher.set_snapshot(&self.snapshot, cx));
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
                    let spec = self.spec_page_for(&page, cx);
                    self.main.update(cx, |main, cx| {
                        main.set_spec_page(spec);
                        main.show(page, forms, session, settings, cx)
                    });
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
        if self.selected == Some(Selection::Session(session.clone())) {
            return;
        }
        self.news.insert(session, unread_words(&forged.said));
        self.forged.push(forged);
        self.ring(cx);
    }

    /// An agent handed the user a link: a toast offers it with an Open button, unless the session's
    /// conversation is on screen, where its card is. Links shared in a row are one toast, which
    /// counts them while it is on screen. Nothing opens until the click.
    fn linked(&mut self, link: LinkShown, cx: &mut Context<Self>) {
        let session = self.snapshot.root_of(&link.agent);
        // On screen is the conversation of the agent itself: a sub-agent's link, while the
        // root's conversation is shown, is not in sight.
        let on_screen = self.selected == Some(Selection::Session(session.clone()))
            && self
                .session_page
                .as_ref()
                .is_some_and(|page| page.read(cx).shown() == link.agent);
        let sharing = self.sharing.entry(link.agent.clone()).or_default();
        if on_screen {
            sharing.end();
            return;
        }
        let mut key = format!("links-{}-{}", link.agent, sharing.row);
        if !sharing.links.is_empty() && !gpui_design_system::toast::shown(&key, cx) {
            sharing.end();
            key = format!("links-{}-{}", link.agent, sharing.row);
        }
        sharing.links.push(link);
        let shell = cx.entity().downgrade();
        let toast = match sharing.links.as_slice() {
            [only] => {
                let target = only.target.clone();
                gpui_design_system::Toast::success(format!(
                    "The agent shares a link: {}",
                    only.name()
                ))
                .action("Open", move |_, cx| {
                    shell
                        .update(cx, |shell, cx| {
                            crate::open::shown(&shell.follow.opener, &target, cx)
                        })
                        .ok();
                })
            }
            shared => gpui_design_system::Toast::success(format!(
                "The agent shares {} links",
                shared.len()
            ))
            .action("Show", move |window, cx| {
                shell
                    .update(cx, |shell, cx| {
                        shell.open(Selection::Session(session.clone()), window, cx)
                    })
                    .ok();
            }),
        };
        gpui_design_system::toast(toast.keyed(key), cx);
    }

    /// What the agent said or what the user told it ends the row of links it was sharing.
    fn spoke(&mut self, agent: &str) {
        if let Some(sharing) = self.sharing.get_mut(agent) {
            sharing.end();
        }
    }

    /// An agent said something, reported or was woken: its session's row says it, unless the
    /// session is the one open, where it is in sight.
    fn news(&mut self, news: News, cx: &mut Context<Self>) {
        self.spoke(&news.agent);
        let session = self.snapshot.root_of(&news.agent);
        if self.selected == Some(Selection::Session(session.clone())) {
            return;
        }
        let said = unread_words(news.said);
        // A message comes in many pieces, and says less than a report or a change: it never
        // replaces what the row said.
        let weak = news.said == News::MESSAGE && self.news.contains_key(&session);
        if !weak && self.news.get(&session) != Some(&said) {
            self.news.insert(session, said);
            self.ring(cx);
        }
    }

    /// Brings the bell and the sessions' marks up to what waits for the user and what the
    /// agents changed: each item says why it is there, a short excerpt, and its session.
    fn ring(&mut self, cx: &mut Context<Self>) {
        let snapshot = &self.snapshot;
        let mut bell: Vec<BellItem> = snapshot.bell.clone();
        for forged in &self.forged {
            let session = snapshot.root_of(&forged.agent);
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
            let said = self.news.get(id).map(String::as_str);
            if row.read(cx).unread() != said {
                row.update(cx, |row, cx| row.set_unread(said, cx));
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
        // A model that is not known is left out: the provider chooses it.
        let model = self
            .providers
            .model_said(name, model.as_deref())
            .map_or_else(String::new, |model| format!(" {model}"));
        Some(format!(
            "Flottille chose {title}{model} as your Auto judge: {title} is installed and signed \
             in here. In Auto, it decides the calls the rules do not know, on your quota. Change \
             it, or choose no judge, in Settings."
        ))
    }

    /// The sentence of the notice that says which compactor Flottille picked, while the user has
    /// not seen it. One notice at a time: the judge's first.
    pub fn compaction_notice(&self) -> Option<String> {
        if self.compaction_noticed || !self.compaction_picked || self.judge_notice().is_some() {
            return None;
        }
        let (name, model) = self.compactor_shown.as_ref()?;
        let title = self
            .providers
            .get(name)
            .map_or(name.as_str(), |p| p.title.as_str());
        let model = self
            .providers
            .model_said(name, model.as_deref())
            .map_or_else(String::new, |model| format!(" {model}"));
        Some(format!(
            "Flottille chose {title}{model} as your compaction model: when an agent's context fills up, it \
             summarises the conversation and the agent goes on from the summary, on your quota. \
             Change it, or choose none, in Settings."
        ))
    }

    /// The notice of the compactor was seen: it goes, and the daemon is told.
    pub fn dismiss_compaction_notice(&mut self, cx: &mut Context<Self>) {
        if !self.compaction_noticed {
            self.compaction_noticed = true;
            self.actions.send(Command::CompactionNoticed);
            cx.notify();
        }
    }

    /// The notice was seen: it goes, and the daemon is told, so that it never comes back.
    pub fn dismiss_judge_notice(&mut self, cx: &mut Context<Self>) {
        if !self.judge_noticed {
            self.judge_noticed = true;
            self.actions.send(Command::JudgeNoticed);
            cx.notify();
        }
    }

    /// A notice of what Flottille chose for the user, bottom right: the Auto judge (`judge`), or
    /// the compactor. Its keys start with `judge-notice` or `compaction-notice`.
    /// `present` false, it folds away and fades out with its last text.
    fn notice_card(
        &self,
        judge: bool,
        present: bool,
        text: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = if judge {
            "judge-notice"
        } else {
            "compaction-notice"
        };
        let part = |part: &str| SharedString::from(format!("{key}{part}"));
        let p = palette(cx);
        let buttons = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(div().flex_1())
            .child(
                Button::secondary(part("-dismiss"), "Dismiss").on_click(cx.listener(
                    move |this, _, _, cx| match judge {
                        true => this.dismiss_judge_notice(cx),
                        false => this.dismiss_compaction_notice(cx),
                    },
                )),
            )
            .child(
                Button::primary(part("-open"), "Open Settings").on_click(cx.listener(
                    move |this, _, window, cx| match judge {
                        true => {
                            this.dismiss_judge_notice(cx);
                            this.open_settings_at(Reveal::Judge, window, cx);
                        }
                        false => {
                            this.dismiss_compaction_notice(cx);
                            this.open_settings_at(Reveal::Compaction, window, cx);
                        }
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
                    .id(part("-text"))
                    .aria_label(SharedString::from(text.clone()))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(text))
                    .test_support(),
            )
            .child(buttons);
        let card = div()
            .id(SharedString::from(key))
            .child(enter(
                part("-enter"),
                0,
                Card::new()
                    .title(if judge {
                        "Auto judge chosen"
                    } else {
                        "Compaction model chosen"
                    })
                    .child(body),
            ))
            .test_support();
        div()
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .w(px(380.))
            .child(presence(part("-presence"), present, card))
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

    /// A project's Tracker `New entry in the vault`: Settings, Vault, on a new entry. Closed, it
    /// goes back to the project's Tracker.
    pub fn add_vault_entry_for(
        &mut self,
        project: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_at(crate::settings::Reveal::Vault, window, cx);
        let vault = self.settings.read(cx).vault().clone();
        vault.update(cx, |vault, cx| vault.add_for(project, window, cx));
    }

    /// Opens Settings at a section: its tab is the one chosen.
    pub fn open_settings_at(
        &mut self,
        at: crate::settings::Reveal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Selection::Settings, window, cx);
        self.settings.update(cx, |page, cx| page.reveal(at, cx));
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
            let read = self.news.remove(id).is_some();
            if read || self.forged.len() != before {
                self.ring(cx);
            }
        }
        // Opened, the page reads the providers again: the file is the user's to edit by hand.
        if selection == Selection::Settings && self.selected != Some(Selection::Settings) {
            // A page opened afresh starts on its first tab; a link then chooses its own.
            self.settings.update(cx, |page, cx| page.choose(0, cx));
            self.actions.send(Command::ReadProviders);
            self.read_settings();
        }
        // Left, the services page stops reading the output it shows: nothing is in sight.
        if self.selected == Some(Selection::Services) && selection != Selection::Services {
            self.services.update(cx, |page, cx| page.close_output(cx));
        }
        // Opened, the services are read again: what runs changes outside the window too.
        if selection == Selection::Services && self.selected != Some(Selection::Services) {
            self.services.update(cx, |page, cx| page.opened(cx));
        }
        let page = self.page_of(&selection, cx);
        self.selected = Some(selection);
        let forms = self.forms_for(&page, window, cx);
        let session = self.session_for(&page, window, cx);
        let project_settings = self.project_settings_for(&page, window, cx);
        let spec = self.spec_page_for(&page, cx);
        self.main.update(cx, |main, cx| {
            main.set_spec_page(spec);
            main.show(page, forms, session, project_settings, cx)
        });
        self.refresh_project(cx);
    }

    /// The page of what is selected, as the daemon last gave it.
    fn page_of(&self, selection: &Selection, cx: &App) -> Page {
        match selection {
            Selection::Settings => Page::Settings,
            Selection::Services => Page::Services,
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
            // Only where Specs is on: nothing of it shows elsewhere.
            Selection::Spec { project, spec } => {
                if self
                    .extensions
                    .read(cx)
                    .active(crate::extensions::specs::NAME, project)
                {
                    Page::Spec {
                        project: project.clone(),
                        spec: *spec,
                    }
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
            Some(Selection::Spec { project, .. }) => Some(project.clone()),
            Some(Selection::Settings | Selection::Services) | None => None,
        }
        .or_else(|| self.snapshot.projects.first().map(|p| p.name.clone()));
        if let Some(project) = project {
            self.open(Selection::Project(project.clone()), window, cx);
            if let Some(composer) = self.composer_of(&project).cloned() {
                composer.update(cx, |composer, cx| composer.focus_end(window, cx));
            }
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
        // A notice keeps its last text while it folds away.
        let (judge_notice, compaction_notice) = (self.judge_notice(), self.compaction_notice());
        for (at, text) in [&judge_notice, &compaction_notice].into_iter().enumerate() {
            if let Some(text) = text {
                self.notices[at] = Some(text.clone());
            }
        }
        let notices = self.notices.clone();
        let (judge_notice, compaction_notice) =
            (judge_notice.is_some(), compaction_notice.is_some());
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
                if event.keystroke.key == "k" && event.keystroke.modifiers == Modifiers::control() {
                    cx.stop_propagation();
                    this.toggle_switcher(window, cx);
                    return;
                }
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
            .when_some(notices[0].clone(), |d, text| {
                d.child(self.notice_card(true, judge_notice, text, cx))
            })
            .when_some(notices[1].clone(), |d, text| {
                d.child(self.notice_card(false, compaction_notice, text, cx))
            })
            .child(self.add_project.clone())
            .child(self.rename.clone())
            .child(self.switcher.clone())
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
        let search_shell = self.shell.clone();
        let menu_shell = self.shell.clone();
        let settings_shell = self.shell.clone();
        let services_shell = self.shell.clone();
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
            .child(pressable(
                div()
                    .id("top-search")
                    .aria_label("Search")
                    .h(px(26.))
                    .px(space::MD)
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .child(icon(Icon::Search).size(px(13.)))
                    .child("Search")
                    .child(div().mono(11.).child("Ctrl K"))
                    .on_click(move |_, window, cx| {
                        search_shell
                            .update(cx, |shell, cx| shell.toggle_switcher(window, cx))
                            .ok();
                    }),
                move |d, hover, press| {
                    d.bg(mix(p.hover.opacity(hover), p.sel, press))
                        .text_color(mix(p.faint, p.text, hover))
                },
            ))
            .child(
                Button::icon("services", Icon::Service, "Services").on_click(
                    move |_, window, cx| {
                        services_shell
                            .update(cx, |shell, cx| shell.open(Selection::Services, window, cx))
                            .ok();
                    },
                ),
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
                        true => glyphs::TICKET,
                        false => glyphs::PULL_REQUEST,
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

/// The Flottille mark, drawn as `assets/brand/mark.svg` draws it: its geometry, in that file's
/// 64-unit box. The sails are filled triangles with a round-joined outline; the hull is one
/// curved stroke with round ends.
const MARK_SAILS: [[(f32, f32); 3]; 2] = [
    [(19., 8.), (19., 38.), (37., 38.)],
    [(44., 19.), (44., 38.), (57., 38.)],
];
const MARK_SAIL_OUTLINE: f32 = 4.;
const MARK_HULL: [(f32, f32); 4] = [(7., 46.), (20., 53.5), (44., 53.5), (57., 46.)];
const MARK_HULL_STROKE: f32 = 6.4;

/// The Flottille mark in the box it is given: two filled sails over one curved hull.
fn logo(color: gpui_kit::Hsla) -> gpui_kit::Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let unit = f32::from(bounds.size.width) / 64.0;
            let at = |(x, y): (f32, f32)| {
                point(
                    bounds.origin.x + px(x * unit),
                    bounds.origin.y + px(y * unit),
                )
            };
            let stroke = |width: f32| {
                PathBuilder::fill().with_style(PathStyle::Stroke(
                    StrokeOptions::default()
                        .with_line_width(width * unit)
                        .with_line_join(LineJoin::Round)
                        .with_line_cap(LineCap::Round),
                ))
            };
            for sail in MARK_SAILS {
                let mut fill = PathBuilder::fill();
                let mut outline = stroke(MARK_SAIL_OUTLINE);
                for builder in [&mut fill, &mut outline] {
                    builder.move_to(at(sail[0]));
                    builder.line_to(at(sail[1]));
                    builder.line_to(at(sail[2]));
                    builder.close();
                }
                for builder in [fill, outline] {
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }
                }
            }
            let mut hull = stroke(MARK_HULL_STROKE);
            hull.move_to(at(MARK_HULL[0]));
            hull.cubic_bezier_to(at(MARK_HULL[3]), at(MARK_HULL[1]), at(MARK_HULL[2]));
            if let Ok(path) = hull.build() {
                window.paint_path(path, color);
            }
        },
    )
}

/// The rows of a project's live sessions as the tree draws them: a session led by another of
/// them sits right under its pilot's row, indented, while that row is unfolded; the others keep
/// their order. Each row learns who leads it and how many it leads.
fn nested(
    live: &[&crate::model::Session],
    rows: &[Entity<SessionRow>],
    cx: &mut Context<Shell>,
) -> Vec<Entity<SessionRow>> {
    // The place of each session's pilot among them, when it is one of them.
    let pilots: Vec<Option<usize>> = live
        .iter()
        .map(|session| {
            let pilot = session.pilot.as_deref().filter(|p| *p != session.id)?;
            live.iter().position(|other| other.id == pilot)
        })
        .collect();
    let mut out = Vec::new();
    let mut placed = HashSet::new();
    // Depth first from each session no other leads; `shown` is false under a folded row. The tie
    // of a row is the lines the rows above it go on with and whether it is its pilot's last.
    let mut stack: Vec<(usize, bool, Option<Tie>)> = (0..live.len())
        .rev()
        .filter(|&at| pilots[at].is_none())
        .map(|at| (at, true, None))
        .collect();
    while let Some((at, shown, tie)) = stack.pop() {
        if !placed.insert(at) {
            continue;
        }
        let row = &rows[at];
        let led_by = pilots[at].and(live[at].led_by.as_deref());
        if row.read(cx).led_by() != led_by {
            row.update(cx, |row, cx| row.set_led_by(led_by, cx));
        }
        if row.read(cx).tie() != tie.as_ref() {
            let tie = tie.clone();
            row.update(cx, |row, cx| row.set_tie(tie, cx));
        }
        let led: Vec<usize> = (0..live.len())
            .filter(|&other| pilots[other] == Some(at))
            .collect();
        if row.read(cx).leads() != led.len() {
            row.update(cx, |row, cx| row.set_leads(led.len(), cx));
        }
        // Folded, the row shows what the sessions it hides ask of the user.
        let hidden = match row.read(cx).is_open() {
            true => None,
            false => {
                let mut below = led.clone();
                let mut marks = Vec::new();
                while let Some(at) = below.pop() {
                    marks.extend(live[at].mark);
                    below.extend((0..live.len()).filter(|&other| pilots[other] == Some(at)));
                }
                AgentState::strongest(marks)
            }
        };
        if row.read(cx).hidden() != hidden {
            row.update(cx, |row, cx| row.set_hidden(hidden, cx));
        }
        if shown {
            out.push(row.clone());
        }
        let open = shown && row.read(cx).is_open();
        // A child's line goes on past its own children's rows while it has a sibling below.
        let through = match &tie {
            Some(tie) => [tie.through.clone(), vec![!tie.last]].concat(),
            None => Vec::new(),
        };
        let last = led.len().saturating_sub(1);
        stack.extend(led.into_iter().enumerate().rev().map(|(place, other)| {
            let tie = Tie {
                through: through.clone(),
                last: place == last,
            };
            (other, open, Some(tie))
        }));
    }
    out
}

#[cfg(test)]
mod mark {
    use super::*;

    /// The numbers of the first attribute `name="…"` of each element that has one.
    fn attributes(svg: &str, name: &str) -> Vec<String> {
        let key = format!(" {name}=\"");
        svg.match_indices(&key)
            .map(|(at, _)| {
                let rest = &svg[at + key.len()..];
                rest[..rest.find('"').unwrap()].to_owned()
            })
            .collect()
    }

    fn numbers(path: &str) -> Vec<f32> {
        path.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter(|n| !n.is_empty())
            .map(|n| n.parse().unwrap())
            .collect()
    }

    #[test]
    fn the_corner_mark_is_the_brand_mark() {
        let svg = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/brand/mark.svg"
        ))
        .unwrap();
        let paths = attributes(&svg, "d");
        assert_eq!(paths.len(), 3, "two sails and a hull: {paths:?}");
        let flat = |points: &[(f32, f32)]| -> Vec<f32> {
            points.iter().flat_map(|&(x, y)| [x, y]).collect()
        };
        assert_eq!(numbers(&paths[0]), flat(&MARK_SAILS[0]));
        assert_eq!(numbers(&paths[1]), flat(&MARK_SAILS[1]));
        assert_eq!(numbers(&paths[2]), flat(&MARK_HULL));
        // Sails are filled and outlined; the hull is a stroke alone.
        assert_eq!(attributes(&svg, "fill"), ["#ececec", "#ececec", "none"]);
        let widths = attributes(&svg, "stroke-width");
        let outline = MARK_SAIL_OUTLINE.to_string();
        assert_eq!(widths[..2], [outline.clone(), outline]);
        assert_eq!(widths[2], MARK_HULL_STROKE.to_string());
    }
}
