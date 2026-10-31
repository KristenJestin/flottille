//! A project's settings: everything that configures the project, once, on one page with a sticky
//! index (`docs/projects.md`). Its repositories (with their forge, `docs/projects.md`, "Forges
//! and tracker"), its tracker, then its Skills, Commands, Tasks and Setup (`docs/skills.md`,
//! `docs/commands.md`), then its rules. The project page only shows and runs.
//!
//! Every item is a row: a click opens its dialog, its "⋯" holds the rest, a removal asks in a
//! confirmation, and what a save did is said in a toast. Flottille never guesses a forge: a
//! repository whose remote is a known forge's address is offered that forge ("acme/api on
//! GitHub? Use"), and nothing is read nor written on a forge until it is declared. The window
//! never writes the project's file: a save asks the daemon (`project.set_settings`), which checks
//! each forge against its remote and keeps the file's comments.

use std::collections::HashMap;

use flottille_core::declared::{ProjectTracker, RepositoryForge};
use flottille_core::project::cycle_sentence;
use flottille_ui::{
    Button, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, PageHeader, PageIndex,
    Section, Select, SelectOption, Switch, Toast, Type, hoverable, icon, palette, presence,
    presence_of, space, toast,
};
use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px, relative,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{CommandsSection, boxed, menu_of, problem_line};
use crate::project::repository_label;
use crate::repositories::{ProjectRepositories, RepositoryInfo};
use crate::rules::RulesEditor;
use crate::sections::Sections;
use crate::setup::SetupSection;
use crate::shell::Shell;
use crate::skills::SkillsSection;
use crate::tasks::TasksSection;

/// A text field in Flottille's colours.
pub fn styled_input(
    placeholder: &str,
    window: &mut Window,
    cx: &mut Context<InputState>,
) -> InputState {
    let p = palette(cx);
    let mut field = InputState::new(window, cx).placeholder(placeholder.to_owned());
    field.set_editor_style(InputEditorStyle {
        foreground: p.text,
        muted_foreground: p.faint,
        selection: p.run.opacity(0.35),
        caret: p.text,
        ..Default::default()
    });
    field
}

/// A field's frame, `id` for the tests and the captures.
pub fn field_frame(
    id: impl Into<ElementId>,
    field: &Entity<InputState>,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(id.into())
        .h(px(32.))
        .px(space::MD)
        .rounded(px(8.))
        .bg(p.field)
        .border_1()
        .border_color(p.line2)
        .flex()
        .items_center()
        .mono(12.5)
        .child(Input::new(field))
        .test_support()
}

/// The sections of the page, in order: each index entry's key and label.
pub const SECTIONS: [(&str, &str); 7] = [
    ("repositories", "Repositories"),
    ("tracker", "Tracker"),
    ("skills", "Skills"),
    ("commands", "Commands"),
    ("tasks", "Tasks"),
    ("setup", "Setup"),
    ("rules", "Rules"),
];

/// Which forge a repository is on, as the form chooses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgeKind {
    None,
    Github,
    BitbucketCloud,
    BitbucketDataCenter,
}

impl ForgeKind {
    pub const ALL: [ForgeKind; 4] = [
        ForgeKind::None,
        ForgeKind::Github,
        ForgeKind::BitbucketCloud,
        ForgeKind::BitbucketDataCenter,
    ];

    pub fn of(forge: &RepositoryForge) -> ForgeKind {
        match forge {
            RepositoryForge::Github { .. } => ForgeKind::Github,
            RepositoryForge::BitbucketCloud { .. } => ForgeKind::BitbucketCloud,
            RepositoryForge::BitbucketDataCenter { .. } => ForgeKind::BitbucketDataCenter,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            ForgeKind::None => "none",
            ForgeKind::Github => "github",
            ForgeKind::BitbucketCloud => "bitbucket-cloud",
            ForgeKind::BitbucketDataCenter => "bitbucket-data-center",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ForgeKind::None => "No forge",
            ForgeKind::Github => "GitHub",
            ForgeKind::BitbucketCloud => "Bitbucket Cloud",
            ForgeKind::BitbucketDataCenter => "Bitbucket Data Center",
        }
    }

    fn line(self) -> &'static str {
        match self {
            ForgeKind::None => "Flottille reads and writes nothing for it",
            ForgeKind::Github => "Pull requests read and opened with gh",
            ForgeKind::BitbucketCloud => "Pull requests of bitbucket.org",
            ForgeKind::BitbucketDataCenter => "Pull requests of a server of your own",
        }
    }

    fn shape(self) -> &'static str {
        match self {
            ForgeKind::None => "",
            ForgeKind::Github => "owner/repo",
            ForgeKind::BitbucketCloud => "workspace/repo",
            ForgeKind::BitbucketDataCenter => "PROJ/repo",
        }
    }
}

/// Which tracker the project's tickets are on, as the form chooses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackerKind {
    None,
    Github,
    Jira,
}

impl TrackerKind {
    pub const ALL: [TrackerKind; 3] = [TrackerKind::Github, TrackerKind::Jira, TrackerKind::None];

    pub fn word(self) -> &'static str {
        match self {
            TrackerKind::None => "none",
            TrackerKind::Github => "github",
            TrackerKind::Jira => "jira",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TrackerKind::None => "None",
            TrackerKind::Github => "GitHub issues",
            TrackerKind::Jira => "Jira",
        }
    }

    fn line(self) -> &'static str {
        match self {
            TrackerKind::None => "No ticket: workspaces are not linked to one",
            TrackerKind::Github => "Issues of a repository of the project, read with gh",
            TrackerKind::Jira => "Issues of a Jira site, read with acli, read only",
        }
    }

    fn of(tracker: Option<&ProjectTracker>) -> TrackerKind {
        match tracker {
            None => TrackerKind::None,
            Some(ProjectTracker::Github { .. }) => TrackerKind::Github,
            Some(ProjectTracker::Jira { .. }) => TrackerKind::Jira,
        }
    }
}

/// What the daemon says of one repository's forge (`project.settings`).
#[derive(Clone, Debug, PartialEq)]
pub struct RepositoryForm {
    pub path: String,
    pub remote: String,
    /// Its base branches, in order of preference.
    pub base: Vec<String>,
    /// The paths of the repositories it depends on.
    pub depends_on: Vec<String>,
    /// Its remote's address, credentials taken out; none without that remote.
    pub url: Option<String>,
    /// What the project's file declares now.
    pub declared: Option<RepositoryForge>,
    /// What the remote's address proposes.
    pub proposal: Option<RepositoryForge>,
    /// Why the declared forge is not the one the remote leads to.
    pub disagrees: Option<String>,
}

impl RepositoryForm {
    /// Whether the row offers the remote's forge, not declared yet.
    pub fn proposed(&self) -> bool {
        self.declared.is_none() && self.proposal.is_some()
    }
}

/// A remote's address in its short form: `github.com/acme/api` for a forge's, a path relative to
/// the project folder for a local one (`../atlas.git`).
pub fn short_remote(url: &str, folder: &str) -> String {
    let local = url.strip_prefix("file://").unwrap_or(url);
    if local.starts_with('/') {
        return relative_to(local, folder);
    }
    let rest = ["ssh://", "https://", "http://", "git://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme));
    let (rest, scp) = match rest {
        Some(rest) => (rest, false),
        None => (url, true),
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
    let rest = match rest.split_once(if scp { ':' } else { '/' }) {
        Some((host, path)) => {
            let host = host.split(':').next().unwrap_or(host);
            format!("{host}/{}", path.trim_start_matches('/'))
        }
        None => rest.to_owned(),
    };
    rest.trim_end_matches('/')
        .trim_end_matches(".git")
        .to_owned()
}

/// `path` relative to `folder`, both absolute: `../atlas.git`.
fn relative_to(path: &str, folder: &str) -> String {
    let parts = |text: &str| -> Vec<String> {
        text.split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (path, folder) = (parts(path), parts(folder));
    let common = path.iter().zip(&folder).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = vec!["..".to_owned(); folder.len() - common];
    out.extend(path[common..].iter().cloned());
    if out.is_empty() {
        ".".to_owned()
    } else {
        out.join("/")
    }
}

/// A forge in words: `GitHub · acme/api`.
pub fn forge_said(forge: &RepositoryForge) -> String {
    match forge {
        RepositoryForge::BitbucketDataCenter { host, repository } => {
            format!("Bitbucket Data Center · {host} · {repository}")
        }
        other => format!("{} · {}", ForgeKind::of(other).label(), other.repository()),
    }
}

/// A base rule in words: `develop, then main`.
fn base_rule(base: &[String]) -> String {
    match base {
        [one] => one.clone(),
        many => many.join(", then "),
    }
}

fn base_said(info: &RepositoryInfo) -> String {
    match info.base.as_slice() {
        [] => info.branch.clone().unwrap_or_else(|| "none".to_owned()),
        base => base_rule(base),
    }
}

/// What the daemon last answered of the project's own Remove.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Fine(String),
    Refused(String),
}

/// The dialog open over the page.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    Repository(String),
    Tracker,
}

/// What a save of `project.set_settings` is for, said in its toast.
#[derive(Clone, Debug, PartialEq)]
enum Saving {
    Forge { path: String, used: bool },
    Tracker,
}

/// The dialogs' fields, made once a window is at hand.
#[derive(Clone)]
struct Fields {
    host: Entity<InputState>,
    repository: Entity<InputState>,
    site: Entity<InputState>,
    keys: Entity<InputState>,
    /// The base branch to add.
    branch: Entity<InputState>,
}

/// A row of the Repositories section: a repository of the project, or one found in its folder.
#[derive(Clone, Debug, PartialEq)]
enum RepositoryRow {
    Known(RepositoryInfo),
    Found(String),
}

/// Where the settings were opened at, from the project page: a repository's row (its chip) or
/// the tasks ("Edit in Settings").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reveal {
    Repository(String),
    Tasks,
}

impl Reveal {
    /// The index entry of its section.
    fn entry(&self) -> usize {
        let key = match self {
            Reveal::Repository(_) => "repositories",
            Reveal::Tasks => "tasks",
        };
        SECTIONS
            .iter()
            .position(|(k, _)| *k == key)
            .expect("a section of the page")
    }
}

pub struct ProjectSettingsPage {
    shell: WeakEntity<Shell>,
    /// Where it was opened at, when it was.
    revealed: Option<Reveal>,
    /// On its way to the section it was opened at.
    revealing: bool,
    name: String,
    folder: String,
    /// The daemon answered once.
    loaded: bool,
    /// Why the settings cannot be read.
    error: Option<String>,
    forms: Vec<RepositoryForm>,
    tracker: Option<ProjectTracker>,
    obsolete: Vec<String>,
    /// The repositories as their checkouts stand, added, removed and detected again.
    repositories: Entity<ProjectRepositories>,
    repository_rows: Listing<RepositoryRow>,
    menus: HashMap<String, Entity<Menu>>,
    more: Entity<Menu>,
    dialog: Option<Dialog>,
    /// What the dialog chooses.
    forge_kind: ForgeKind,
    tracker_kind: TrackerKind,
    tracker_repository: Option<String>,
    /// The repository dialog's base branches and dependencies, as edited and not yet saved.
    bases: Vec<String>,
    base_rows: Listing<String>,
    base_error: Option<SharedString>,
    depends: Vec<String>,
    fields: Option<Fields>,
    dialog_error: Option<SharedString>,
    saving: Option<Saving>,
    confirming_remove: bool,
    removing: bool,
    removed: Option<String>,
    narrow: bool,
    sections: Sections,
    /// This project's own rules (`docs/projects.md`, "Rules").
    rules: Entity<RulesEditor>,
    skills: Entity<SkillsSection>,
    commands: Entity<CommandsSection>,
    tasks: Entity<TasksSection>,
    setup: Entity<SetupSection>,
    _subscriptions: Vec<Subscription>,
}

impl ProjectSettingsPage {
    pub fn new(
        shell: WeakEntity<Shell>,
        name: String,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rules = {
            let (shell, project) = (shell.clone(), name.clone());
            cx.new(|_| RulesEditor::new(shell, Some(project)))
        };
        let skills = cx.new(|_| SkillsSection::new(shell.clone(), name.clone()));
        let commands = cx.new(|_| CommandsSection::new(shell.clone(), name.clone()));
        let tasks = cx.new(|_| TasksSection::new(shell.clone(), name.clone()));
        let setup = cx.new(|_| SetupSection::new(shell.clone(), name.clone()));
        let repositories = cx.new(|_| ProjectRepositories::new(shell.clone(), name.clone()));
        // Asked once the shell, which is making this page, is no longer being updated.
        let (listing, skilling, tasking, setting, reading) = (
            commands.clone(),
            skills.clone(),
            tasks.clone(),
            setup.clone(),
            repositories.clone(),
        );
        cx.defer(move |cx| {
            listing.update(cx, |section, cx| section.read(cx));
            skilling.update(cx, |section, cx| section.read(cx));
            tasking.update(cx, |section, cx| section.read(cx));
            setting.update(cx, |section, cx| section.read(cx));
            reading.update(cx, |section, cx| section.read(cx));
        });
        let this = cx.entity().downgrade();
        let more = cx.new(|cx| {
            Menu::new("project-more", "More", cx).danger(
                "remove-project",
                "Remove project…",
                move |_, cx| {
                    this.update(cx, |this, cx| this.ask_removal(cx)).ok();
                },
            )
        });
        let observed = cx.observe(&repositories, |_, _, cx| cx.notify());
        Self {
            shell,
            revealed: None,
            revealing: false,
            name,
            folder: String::new(),
            loaded: false,
            error: None,
            forms: Vec::new(),
            tracker: None,
            obsolete: Vec::new(),
            repositories,
            repository_rows: Listing::default(),
            menus: HashMap::new(),
            more,
            dialog: None,
            forge_kind: ForgeKind::None,
            tracker_kind: TrackerKind::None,
            tracker_repository: None,
            bases: Vec::new(),
            base_rows: Listing::default(),
            base_error: None,
            depends: Vec::new(),
            fields: None,
            dialog_error: None,
            saving: None,
            confirming_remove: false,
            removing: false,
            removed: None,
            narrow: false,
            sections: Sections::default(),
            rules,
            skills,
            commands,
            tasks,
            setup,
            _subscriptions: vec![observed],
        }
    }

    pub fn rules(&self) -> &Entity<RulesEditor> {
        &self.rules
    }

    pub fn skills(&self) -> &Entity<SkillsSection> {
        &self.skills
    }

    pub fn commands(&self) -> &Entity<CommandsSection> {
        &self.commands
    }

    pub fn tasks(&self) -> &Entity<TasksSection> {
        &self.tasks
    }

    pub fn setup(&self) -> &Entity<SetupSection> {
        &self.setup
    }

    /// The repositories as their checkouts stand: the Repositories section's rows.
    pub fn repositories(&self) -> &Entity<ProjectRepositories> {
        &self.repositories
    }

    /// Whether the daemon has answered the read of the settings themselves (the forms), with
    /// them or with a problem.
    fn has_read_settings(&self) -> bool {
        self.loaded || self.error.is_some()
    }

    /// Whether the daemon has answered the lists the page asks for as it opens, with a list or
    /// with a problem: an answer redraws the page, so the tests wait for it.
    pub fn has_read(&self, cx: &App) -> bool {
        let commands = self.commands.read(cx);
        let skills = self.skills.read(cx);
        let tasks = self.tasks.read(cx);
        let setup = self.setup.read(cx);
        let repositories = self.repositories.read(cx);
        self.has_read_settings()
            && (commands.commands().is_some() || commands.problem().is_some())
            && (skills.skills().is_some() || skills.problem().is_some())
            && (tasks.tasks().is_some() || tasks.problem().is_some())
            && (setup.steps().is_some() || setup.problem().is_some())
            && (repositories.repositories().is_some() || repositories.problem().is_some())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// What the daemon says of each repository's forge.
    pub fn forms(&self) -> &[RepositoryForm] {
        &self.forms
    }

    /// The tracker the project's file declares.
    pub fn tracker(&self) -> TrackerKind {
        TrackerKind::of(self.tracker.as_ref())
    }

    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    pub fn dialog_error(&self) -> Option<&str> {
        self.dialog_error.as_deref()
    }

    /// The tracker the dialog chooses.
    pub fn tracker_choice(&self) -> TrackerKind {
        self.tracker_kind
    }

    /// The forge the dialog chooses.
    pub fn forge_choice(&self) -> ForgeKind {
        self.forge_kind
    }

    pub fn is_confirming_removal(&self) -> bool {
        self.confirming_remove
    }

    pub fn removal_refused(&self) -> Option<&str> {
        self.removed.as_deref()
    }

    pub fn site_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.site)
    }

    pub fn keys_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.keys)
    }

    pub fn forge_repository_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.repository)
    }

    pub fn forge_host_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.host)
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// The daemon's answer to a read or a save: the repositories' forges and the tracker as the
    /// file declares them now. What a save did is a toast; its refusal stays in its dialog.
    pub fn answered(
        &mut self,
        read: Result<Value, String>,
        saved: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if saved {
            let saving = self.saving.take();
            match &read {
                Ok(_) => {
                    if self.dialog.is_some() {
                        self.dialog = None;
                        self.dialog_error = None;
                    }
                }
                Err(why) if self.dialog.is_some() => {
                    self.dialog_error = Some(why.clone().into());
                }
                Err(why) => toast(Toast::failure("Not saved").detail(why.clone()), cx),
            }
            if let Ok(settings) = &read {
                self.toast_saved(saving, settings, cx);
            }
        }
        let settings = match read {
            Ok(settings) => settings,
            Err(why) => {
                if !saved {
                    self.error = Some(why);
                }
                cx.notify();
                return;
            }
        };
        self.loaded = true;
        self.error = None;
        self.folder = settings["folder"].as_str().unwrap_or_default().to_owned();
        let folder = self.folder.clone();
        self.skills
            .update(cx, |section, _| section.set_folder(&folder));
        self.obsolete = strings(&settings["obsolete"]);
        let forge = |value: &Value| serde_json::from_value::<RepositoryForge>(value.clone()).ok();
        self.forms = settings["repositories"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|repository| RepositoryForm {
                path: repository["path"].as_str().unwrap_or_default().to_owned(),
                remote: repository["remote"].as_str().unwrap_or("origin").to_owned(),
                base: strings(&repository["base"]),
                depends_on: strings(&repository["depends_on"]),
                url: repository["url"].as_str().map(str::to_owned),
                declared: forge(&repository["forge"]),
                proposal: forge(&repository["proposal"]),
                disagrees: repository["disagrees"].as_str().map(str::to_owned),
            })
            .collect();
        let paths: Vec<String> = self.forms.iter().map(|row| row.path.clone()).collect();
        self.setup.update(cx, |section, cx| {
            section.set_repositories(paths.clone(), cx)
        });
        self.tasks
            .update(cx, |section, cx| section.set_repositories(paths, cx));
        self.tracker = serde_json::from_value::<ProjectTracker>(settings["tracker"].clone()).ok();
        cx.notify();
    }

    fn toast_saved(&self, saving: Option<Saving>, settings: &Value, cx: &mut App) {
        let said = match saving {
            Some(Saving::Forge { path, used }) => {
                let label = repository_label(&self.name, &path);
                let declared = settings["repositories"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|r| r["path"] == path.as_str())
                    .and_then(|r| {
                        serde_json::from_value::<RepositoryForge>(r["forge"].clone()).ok()
                    });
                let bases = settings["repositories"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|r| r["path"] == path.as_str())
                    .map(|r| strings(&r["base"]))
                    .filter(|bases| !bases.is_empty() && !used)
                    .map(|bases| format!(" New workspaces start from {}.", base_rule(&bases)))
                    .unwrap_or_default();
                match declared {
                    Some(forge) if used => {
                        let remote = self
                            .forms
                            .iter()
                            .find(|f| f.path == path)
                            .map_or("origin", |f| f.remote.as_str());
                        Toast::success(format!("Forge of {label} saved")).detail(format!(
                            "{} {}, read from the remote {remote}.",
                            ForgeKind::of(&forge).label(),
                            forge.repository()
                        ))
                    }
                    Some(forge) => {
                        Toast::success(format!("Repository {label} saved")).detail(format!(
                            "Flottille reads its pull requests on {} from now on.{bases}",
                            ForgeKind::of(&forge).label()
                        ))
                    }
                    None => Toast::success(format!("Repository {label} saved")).detail(format!(
                        "No forge: Flottille reads and writes nothing for it.{bases}"
                    )),
                }
            }
            Some(Saving::Tracker) => {
                let tracker =
                    serde_json::from_value::<ProjectTracker>(settings["tracker"].clone()).ok();
                Toast::success("Tracker saved").detail(match tracker {
                    None => "Workspaces are not linked to a ticket.".to_owned(),
                    Some(ProjectTracker::Github { repository }) => format!(
                        "Tickets are the GitHub issues of {}.",
                        repository_label(&self.name, &repository)
                    ),
                    Some(ProjectTracker::Jira { site, .. }) => {
                        format!("Tickets are the Jira issues of {site}.")
                    }
                })
            }
            None => Toast::success("Settings saved"),
        };
        toast(said, cx);
    }

    /// What a save sends: `project.set_settings`'s parameters, every forge and the tracker as
    /// the file declares them, but for `forge` (a repository's) or `tracker` given.
    fn params(
        &self,
        forge: Option<(&str, Option<RepositoryForge>)>,
        lists: Option<(&str, &[String], &[String])>,
        tracker: Option<Option<ProjectTracker>>,
    ) -> Value {
        let repositories: Vec<Value> = self
            .forms
            .iter()
            .map(|form| {
                let declared = match forge {
                    Some((path, ref chosen)) if path == form.path => chosen.clone(),
                    _ => form.declared.clone(),
                };
                match lists {
                    Some((path, base, depends_on)) if path == form.path => {
                        json!({"path": form.path, "forge": declared, "base": base,
                            "depends_on": depends_on})
                    }
                    _ => json!({"path": form.path, "forge": declared}),
                }
            })
            .collect();
        json!({
            "name": self.name,
            "repositories": repositories,
            "tracker": tracker.unwrap_or_else(|| self.tracker.clone()),
        })
    }

    fn send(&mut self, settings: Value, saving: Saving, cx: &mut Context<Self>) {
        if self.saving.is_some() || !self.loaded {
            return;
        }
        self.saving = Some(saving);
        self.dialog_error = None;
        let command = Command::SaveProjectSettings {
            project: self.name.clone(),
            settings,
        };
        crate::commands::perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// "Use" on a repository's row: declares the forge its remote proposes.
    pub fn use_proposal(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(proposal) = self
            .forms
            .iter()
            .find(|form| form.path == path)
            .and_then(|form| form.proposal.clone())
        else {
            return;
        };
        let settings = self.params(Some((path, Some(proposal))), None, None);
        let saving = Saving::Forge {
            path: path.to_owned(),
            used: true,
        };
        self.send(settings, saving, cx);
    }

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.fields.is_none() {
            let line = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| styled_input(placeholder, window, cx))
            };
            let fields = Fields {
                host: line("bitbucket.example.com", window, cx),
                repository: line("acme/api", window, cx),
                site: line("acme.atlassian.net", window, cx),
                keys: line("ABC, DEF", window, cx),
                branch: line("develop", window, cx),
            };
            self._subscriptions.push(cx.subscribe_in(
                &fields.branch,
                window,
                |this, _, event, window, cx| match event {
                    InputEvent::Change => {
                        if this.base_error.take().is_some() | this.dialog_error.take().is_some() {
                            cx.notify();
                        }
                    }
                    InputEvent::PressEnter { .. } => this.add_base(window, cx),
                    _ => {}
                },
            ));
            for field in [&fields.host, &fields.repository, &fields.site, &fields.keys] {
                self._subscriptions
                    .push(cx.subscribe(field, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.dialog_error.take().is_some()
                        {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(fields);
        }
        self.fields.as_ref().expect("made above")
    }

    /// A click on a repository's row: its dialog, on the forge declared, or the one its remote
    /// proposes.
    pub fn open_repository(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.forms.iter().find(|form| form.path == path).cloned() else {
            return;
        };
        let shown = form.declared.clone().or_else(|| form.proposal.clone());
        let (host, repository) = match &shown {
            Some(RepositoryForge::BitbucketDataCenter { host, repository }) => {
                (host.clone(), repository.clone())
            }
            Some(other) => (String::new(), other.repository().to_owned()),
            None => (String::new(), String::new()),
        };
        let fields = self.fields(window, cx);
        let (host_field, repository_field) = (fields.host.clone(), fields.repository.clone());
        host_field.update(cx, |f, cx| f.set_value(host, window, cx));
        repository_field.update(cx, |f, cx| f.set_value(repository, window, cx));
        let branch_field = self.fields(window, cx).branch.clone();
        branch_field.update(cx, |f, cx| f.set_value("", window, cx));
        self.forge_kind = shown.as_ref().map_or(ForgeKind::None, ForgeKind::of);
        self.bases = form.base.clone();
        self.depends = form.depends_on.clone();
        // The rows of another repository's dialog are not this one's to fold away.
        self.base_rows = Listing::default();
        self.base_error = None;
        self.dialog = Some(Dialog::Repository(path.to_owned()));
        self.dialog_error = None;
        cx.notify();
    }

    /// The branch typed, added last to the base branches.
    pub fn add_base(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.fields.as_ref().map(|f| f.branch.clone()) else {
            return;
        };
        let branch = field.read(cx).value().trim().to_owned();
        self.base_error = if branch.is_empty() {
            Some("Type the name of a branch first.".into())
        } else if branch.contains(char::is_whitespace) {
            Some("A branch name has no space.".into())
        } else if self.bases.contains(&branch) {
            Some(format!("{branch} is already in the list.").into())
        } else {
            None
        };
        if self.base_error.is_none() {
            self.bases.push(branch);
            self.dialog_error = None;
            field.update(cx, |f, cx| f.set_value("", window, cx));
        }
        cx.notify();
    }

    /// The base branch at `from`, moved to `to`.
    pub fn move_base(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from < self.bases.len() && to < self.bases.len() && from != to {
            let branch = self.bases.remove(from);
            self.bases.insert(to, branch);
            self.dialog_error = None;
            cx.notify();
        }
    }

    pub fn remove_base(&mut self, at: usize, cx: &mut Context<Self>) {
        if at < self.bases.len() {
            self.bases.remove(at);
            self.dialog_error = None;
            cx.notify();
        }
    }

    /// The base branches the dialog holds, in order.
    pub fn bases(&self) -> &[String] {
        &self.bases
    }

    /// The repositories the dialog's repository depends on.
    pub fn depends(&self) -> &[String] {
        &self.depends
    }

    /// The branch field's refusal, under its field.
    pub fn base_error(&self) -> Option<&str> {
        self.base_error.as_deref()
    }

    /// Whether the dialog's repository depends on `path`, flipped.
    pub fn toggle_dependency(&mut self, path: &str, cx: &mut Context<Self>) {
        match self.depends.iter().position(|d| d == path) {
            Some(at) => {
                self.depends.remove(at);
            }
            None => self.depends.push(path.to_owned()),
        }
        self.dialog_error = None;
        cx.notify();
    }

    pub fn branch_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.branch)
    }

    /// A click on the tracker's row: its dialog, on what the file declares.
    pub fn open_tracker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (site, keys) = match &self.tracker {
            Some(ProjectTracker::Jira { site, projects }) => (site.clone(), projects.join(", ")),
            _ => (String::new(), String::new()),
        };
        let fields = self.fields(window, cx);
        let (site_field, keys_field) = (fields.site.clone(), fields.keys.clone());
        site_field.update(cx, |f, cx| f.set_value(site, window, cx));
        keys_field.update(cx, |f, cx| f.set_value(keys, window, cx));
        self.tracker_kind = TrackerKind::of(self.tracker.as_ref());
        self.tracker_repository = match &self.tracker {
            Some(ProjectTracker::Github { repository }) => Some(repository.clone()),
            _ => None,
        };
        self.dialog = Some(Dialog::Tracker);
        self.dialog_error = None;
        cx.notify();
    }

    pub fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        self.dialog_error = None;
        cx.notify();
    }

    pub fn choose_forge(&mut self, kind: ForgeKind, cx: &mut Context<Self>) {
        self.forge_kind = kind;
        self.dialog_error = None;
        cx.notify();
    }

    pub fn choose_tracker(&mut self, kind: TrackerKind, cx: &mut Context<Self>) {
        self.tracker_kind = kind;
        self.dialog_error = None;
        cx.notify();
    }

    pub fn choose_tracker_repository(&mut self, path: String, cx: &mut Context<Self>) {
        self.tracker_repository = Some(path);
        cx.notify();
    }

    /// The repositories whose forge is GitHub: those whose issues can be the tracker.
    pub fn github_paths(&self) -> Vec<String> {
        self.forms
            .iter()
            .filter(|form| {
                form.declared
                    .as_ref()
                    .is_some_and(|forge| ForgeKind::of(forge) == ForgeKind::Github)
            })
            .map(|form| form.path.clone())
            .collect()
    }

    /// Save in the open dialog.
    pub fn save_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.fields else {
            return;
        };
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        match self.dialog.clone() {
            Some(Dialog::Repository(path)) => {
                let repository = value(&fields.repository);
                let forge = match self.forge_kind {
                    ForgeKind::None => None,
                    ForgeKind::Github => Some(RepositoryForge::Github { repository }),
                    ForgeKind::BitbucketCloud => {
                        Some(RepositoryForge::BitbucketCloud { repository })
                    }
                    ForgeKind::BitbucketDataCenter => Some(RepositoryForge::BitbucketDataCenter {
                        host: value(&fields.host),
                        repository,
                    }),
                };
                // Said here as a sentence: the daemon would refuse the same cycle in the file's
                // words, which are for the command line.
                let graph: Vec<(String, Vec<String>)> = self
                    .forms
                    .iter()
                    .map(|form| match form.path == path {
                        true => (path.clone(), self.depends.clone()),
                        false => (form.path.clone(), form.depends_on.clone()),
                    })
                    .collect();
                if let Some(sentence) = cycle_sentence(&graph, &path) {
                    self.dialog_error = Some(sentence.into());
                    cx.notify();
                    return;
                }
                let lists = (&path[..], &self.bases[..], &self.depends[..]);
                let settings = self.params(Some((&path, forge)), Some(lists), None);
                self.send(settings, Saving::Forge { path, used: false }, cx);
            }
            Some(Dialog::Tracker) => {
                let tracker = match self.tracker_kind {
                    TrackerKind::None => None,
                    TrackerKind::Github => {
                        let paths = self.github_paths();
                        let repository = self
                            .tracker_repository
                            .clone()
                            .filter(|path| paths.contains(path))
                            .or_else(|| paths.first().cloned());
                        match repository {
                            Some(repository) => Some(ProjectTracker::Github { repository }),
                            None => {
                                self.dialog_error = Some(
                                    "No repository of the project is on GitHub: give one its \
                                     forge first."
                                        .into(),
                                );
                                cx.notify();
                                return;
                            }
                        }
                    }
                    TrackerKind::Jira => Some(ProjectTracker::Jira {
                        site: value(&fields.site),
                        projects: value(&fields.keys)
                            .split([',', ' '])
                            .map(str::trim)
                            .filter(|key| !key.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    }),
                };
                let settings = self.params(None, None, Some(tracker));
                self.send(settings, Saving::Tracker, cx);
            }
            None => {}
        }
    }

    /// Asks to remove the project, in a confirmation.
    pub fn ask_removal(&mut self, cx: &mut Context<Self>) {
        self.confirming_remove = true;
        cx.notify();
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        self.removing = true;
        self.confirming_remove = false;
        self.removed = None;
        let command = Command::RemoveProject(self.name.clone());
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// The daemon refused to forget the project, and why; a removal done closes the page.
    pub fn remove_refused(&mut self, why: String, cx: &mut Context<Self>) {
        self.removing = false;
        toast(
            Toast::failure(format!("The project {} was not removed", self.name))
                .detail(why.clone()),
            cx,
        );
        self.removed = Some(why);
        cx.notify();
    }

    fn repository_menu(&mut self, path: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.entity().downgrade();
        let repositories = self.repositories.downgrade();
        let label = repository_label(&self.name, path);
        menu_of(
            &mut self.menus,
            "repository",
            path,
            cx,
            move |path, menu| {
                let (edit, path_edit, path_remove) =
                    (this.clone(), path.to_owned(), path.to_owned());
                menu.entry(
                    format!("edit-repository-{label}"),
                    "Edit",
                    None,
                    move |window, cx| {
                        edit.update(cx, |this, cx| this.open_repository(&path_edit, window, cx))
                            .ok();
                    },
                )
                .danger(
                    format!("remove-repository-{label}"),
                    "Remove from the project…",
                    move |_, cx| {
                        repositories
                            .update(cx, |section, cx| section.remove(&path_remove, cx))
                            .ok();
                    },
                )
            },
        )
    }

    /// The labelled values of a repository's row: base branch, remote, forge.
    fn values(&self, info: &RepositoryInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let path = info.path.clone();
        let form = self.forms.iter().find(|form| form.path == path);
        let id = |part: &str| ElementId::Name(format!("repository-{part}-{path}").into());
        let value = |id: ElementId, label: &'static str, text: String, width: f32| {
            div()
                .w(relative(width))
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(px(11.)).text_color(p.faint).child(label))
                .child(
                    div()
                        .id(id)
                        .aria_label(SharedString::from(text.clone()))
                        .mono(12.)
                        .text_color(p.mute)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(SharedString::from(text))
                        .test_support(),
                )
        };
        let remote = match &info.url {
            Some(url) => format!("{} · {}", info.remote, short_remote(url, &self.folder)),
            None => format!("{}: none, local only", info.remote),
        };
        let forge: gpui_kit::AnyElement = match form {
            Some(form) if form.proposed() => {
                let proposal = form.proposal.clone().expect("proposed");
                let said = format!(
                    "{} on {}?",
                    proposal.repository(),
                    ForgeKind::of(&proposal).label()
                );
                let used = path.clone();
                div()
                    .w(relative(0.41))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(11.)).text_color(p.faint).child("Forge"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .child(
                                div()
                                    .id(id("forge"))
                                    .aria_label(SharedString::from(said.clone()))
                                    .mono(12.)
                                    .text_color(p.run)
                                    .child(SharedString::from(said))
                                    .test_support(),
                            )
                            .child(hoverable(
                                div()
                                    .id(id("use"))
                                    .aria_label("Use")
                                    .px(space::SM)
                                    .rounded(px(5.))
                                    .border_1()
                                    .border_color(p.line2)
                                    .text_size(px(11.))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(p.text)
                                    .cursor_pointer()
                                    .child("Use")
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.use_proposal(&used, cx)
                                    })),
                                move |d, t| d.bg(p.hover.opacity(t)),
                            )),
                    )
                    .into_any_element()
            }
            Some(RepositoryForm {
                declared: Some(forge),
                disagrees: Some(why),
                ..
            }) => div()
                .w(relative(0.41))
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(px(11.)).text_color(p.faint).child("Forge"))
                .child(
                    div()
                        .id(id("forge"))
                        .aria_label(SharedString::from(forge_said(forge)))
                        .mono(12.)
                        .text_color(p.fail)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(SharedString::from(format!("{} · {why}", forge_said(forge))))
                        .test_support(),
                )
                .into_any_element(),
            _ => value(
                id("forge"),
                "Forge",
                // The form holds what was saved last; the repository's reading may be older.
                form.and_then(|form| form.declared.as_ref())
                    .or(info.forge.as_ref())
                    .map_or_else(|| "None".to_owned(), forge_said),
                0.41,
            )
            .into_any_element(),
        };
        div()
            .mt(px(6.))
            .flex()
            .gap(space::LG)
            .child(value(id("base"), "Base branch", base_said(info), 0.22))
            .child(value(id("remote"), "Remote, from git", remote, 0.37))
            .child(forge)
    }

    fn repository_row(&mut self, row: &RepositoryRow, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        match row {
            RepositoryRow::Known(info) => {
                let label = repository_label(&self.name, &info.path);
                let menu = self.repository_menu(&info.path, cx);
                let path = info.path.clone();
                let mut row = ListRow::new(
                    ElementId::Name(format!("repository-{label}").into()),
                    SharedString::from(label),
                )
                .mono()
                .leading(icon(Icon::Repository).size(px(15.)).text_color(p.faint))
                .below(self.values(info, cx))
                .more(menu)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_repository(&path, window, cx)),
                );
                let depends_on = self
                    .forms
                    .iter()
                    .find(|form| form.path == info.path)
                    .map(|form| {
                        let labels: Vec<_> = form
                            .depends_on
                            .iter()
                            .map(|path| repository_label(&self.name, path))
                            .collect();
                        labels.join(", ")
                    })
                    .unwrap_or_default();
                if !depends_on.is_empty() {
                    row = row.detail(SharedString::from(format!("depends on {depends_on}")));
                }
                if !info.found {
                    row = row.tag("not in the folder");
                }
                row
            }
            RepositoryRow::Found(path) => {
                let label = repository_label(&self.name, path);
                let adding = self.repositories.read(cx).is_adding(path);
                let busy = self.repositories.read(cx).is_busy();
                let (added, repositories) = (path.clone(), self.repositories.downgrade());
                ListRow::new(
                    ElementId::Name(format!("found-repository-{label}").into()),
                    SharedString::from(label.clone()),
                )
                .mono()
                .leading(icon(Icon::Repository).size(px(15.)).text_color(p.line2))
                .detail("found in the folder, not in the project")
                .trailing(
                    Button::secondary(
                        ElementId::Name(format!("add-repository-{label}").into()),
                        if adding { "Adding…" } else { "Add" },
                    )
                    .leading(Icon::Plus)
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        repositories
                            .update(cx, |section, cx| section.add(&added, cx))
                            .ok();
                    }),
                )
            }
        }
    }

    fn repositories_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let read = self.repositories.read(cx).repositories().cloned();
        // A row says its forge, which only the settings' read knows: the list waits for it.
        if let Some(read) = read.as_ref().filter(|_| self.has_read_settings()) {
            let now = cx.background_executor().now();
            let rows = read
                .listed
                .iter()
                .map(|info| (info.path.clone(), RepositoryRow::Known(info.clone())))
                .chain(
                    read.new
                        .iter()
                        .map(|path| (format!("found:{path}"), RepositoryRow::Found(path.clone()))),
                );
            self.repository_rows.sync(rows, now, cx.reduce_motion());
        }
        let drawn: Vec<_> = self
            .repository_rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("repository-list");
        for (key, placement, row) in drawn {
            let row = self.repository_row(&row, cx);
            list = list.item(
                ElementId::Name(format!("repository-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = read.as_ref().map_or(0, |read| read.listed.len());
        let detecting = self.repositories.read(cx).is_busy();
        let problem = self
            .repositories
            .read(cx)
            .problem()
            .map(|why| SharedString::from(why.to_owned()));
        let repositories = self.repositories.downgrade();
        Section::new("repositories", "Repositories")
            .count(count.to_string())
            .sentence(
                "The git repositories of the project folder. A workspace takes a worktree of each.",
            )
            .action(
                Button::quiet("detect-repositories", "Detect again")
                    .leading(Icon::Search)
                    .disabled(detecting)
                    .on_click(move |_, _, cx| {
                        repositories
                            .update(cx, |section, cx| section.detect(cx))
                            .ok();
                    }),
            )
            .child(
                presence(
                    "repository-list-block",
                    !self.repository_rows.is_empty(),
                    list,
                )
                .gap(space::MD),
            )
            .child(
                presence(
                    "no-repositories-block",
                    read.as_ref()
                        .is_some_and(|read| read.listed.is_empty() && read.new.is_empty()),
                    EmptyLine::new(
                        "no-repositories",
                        Icon::Repository,
                        "No repository: the folder holds none one or two levels down. Detect \
                         again once it does.",
                    ),
                )
                .gap(space::MD),
            )
            .child(problem_line("repositories-problem", problem, space::MD, cx))
    }

    fn tracker_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let kind = TrackerKind::of(self.tracker.as_ref());
        let detail = match &self.tracker {
            None => "workspaces are not linked to a ticket".to_owned(),
            Some(ProjectTracker::Github { repository }) => format!(
                "of {} · a ticket is #123",
                repository_label(&self.name, repository)
            ),
            Some(ProjectTracker::Jira { site, projects }) => {
                let key = projects.first().map_or("ABC", String::as_str);
                format!("{site} · {} · a ticket is {key}-123", projects.join(", "))
            }
        };
        let row = ListRow::new("tracker-row", kind.label())
            .leading(icon(Icon::Ticket).size(px(15.)).text_color(p.faint))
            .detail(SharedString::from(detail))
            .on_click(cx.listener(|this, _, window, cx| this.open_tracker(window, cx)));
        Section::new("tracker", "Tracker")
            .sentence("Where the project's tickets are. A workspace can be linked to one.")
            .child(ListCard::new("tracker-list").row(row))
    }

    /// The base branches of the repository's dialog: a list of branches in order of preference
    /// (moved, removed, and added from the line under it).
    fn base_field(&mut self, branch: &Entity<InputState>, cx: &mut Context<Self>) -> Field {
        let now = cx.background_executor().now();
        self.base_rows.sync(
            self.bases.iter().map(|base| (base.clone(), base.clone())),
            now,
            cx.reduce_motion(),
        );
        let drawn: Vec<_> = self
            .base_rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let last = self.bases.len().saturating_sub(1);
        let mut list = ListCard::new("base-list");
        for (key, placement, branch_name) in drawn {
            let at = self.bases.iter().position(|base| *base == key);
            let button = |id: &str, glyph: Icon, tooltip: &'static str, enabled: bool| {
                Button::icon(format!("base-{id}-{branch_name}"), glyph, tooltip).disabled(!enabled)
            };
            let (up, down, remove) = (this.clone(), this.clone(), this.clone());
            let buttons = div()
                .flex()
                .items_center()
                .gap(space::XS)
                .child(
                    button(
                        "up",
                        Icon::ChevronUp,
                        "Move up",
                        at.is_some_and(|at| at > 0),
                    )
                    .on_click(move |_, _, cx| {
                        up.update(cx, |this, cx| {
                            if let Some(at) = at {
                                this.move_base(at, at.saturating_sub(1), cx)
                            }
                        })
                        .ok();
                    }),
                )
                .child(
                    button(
                        "down",
                        Icon::Chevron,
                        "Move down",
                        at.is_some_and(|at| at < last),
                    )
                    .on_click(move |_, _, cx| {
                        down.update(cx, |this, cx| {
                            if let Some(at) = at {
                                this.move_base(at, at + 1, cx)
                            }
                        })
                        .ok();
                    }),
                )
                .child(
                    button("remove", Icon::Close, "Remove", at.is_some()).on_click(
                        move |_, _, cx| {
                            remove
                                .update(cx, |this, cx| {
                                    if let Some(at) = at {
                                        this.remove_base(at, cx)
                                    }
                                })
                                .ok();
                        },
                    ),
                );
            let row = ListRow::new(
                ElementId::Name(format!("base-row-{branch_name}").into()),
                SharedString::from(branch_name.clone()),
            )
            .mono()
            .leading(icon(Icon::Branch).size(px(15.)).text_color(p.faint))
            .detail(if at == Some(0) {
                "first choice"
            } else {
                "if none above"
            })
            .trailing(buttons);
            list = list.item(
                ElementId::Name(format!("base-item-{key}").into()),
                placement,
                row,
            );
        }
        let this = cx.entity().downgrade();
        let add = Button::secondary("base-add", "Add")
            .leading(Icon::Plus)
            .on_click(move |_, window, cx| {
                this.update(cx, |this, cx| this.add_base(window, cx)).ok();
            });
        let control = div()
            .flex()
            .flex_col()
            .child(presence("base-list-block", !self.bases.is_empty(), list).gap(space::SM))
            .child(
                presence(
                    "base-empty-block",
                    self.bases.is_empty(),
                    EmptyLine::new(
                        "base-empty",
                        Icon::Branch,
                        "No base branch: add one, a workspace starts from it.",
                    ),
                )
                .gap(space::SM),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(
                        div()
                            .flex_1()
                            .child(boxed("base-branch", Input::new(branch), cx)),
                    )
                    .child(add),
            );
        Field::new("base-field", "Base branches", control)
            .hint(
                "A workspace's branch starts from the first of these that the remote has: \
                 develop, then main.",
            )
            .error(self.base_error.clone())
    }

    /// The other repositories of the project, each a switch: the ones this repository depends on.
    fn depends_field(&self, path: &str, this: WeakEntity<Self>) -> Field {
        let others: Vec<_> = self.forms.iter().filter(|form| form.path != path).collect();
        let mut control = div().flex().flex_col().gap(space::MD);
        for form in &others {
            let label = repository_label(&self.name, &form.path);
            let remote = match &form.url {
                Some(url) => short_remote(url, &self.folder),
                None => "local only".to_owned(),
            };
            let (key, on) = (form.path.clone(), self.depends.contains(&form.path));
            let switch = Switch::new(format!("depends-on-{label}"), on, label).hint(remote);
            let this = this.clone();
            control = control.child(switch.on_click(move |_, _, cx| {
                this.update(cx, |this, cx| this.toggle_dependency(&key, cx))
                    .ok();
            }));
        }
        let hint = match others.is_empty() {
            true => "The project has no other repository.",
            false => {
                "Prepared and merged first. A repository never depends on itself or on one \
                      that depends on it."
            }
        };
        Field::new("depends-field", "Depends on", control).hint(hint)
    }

    fn dialog_modal(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let (title, sentence) = match &self.dialog {
            Some(Dialog::Repository(path)) => {
                let label = repository_label(&self.name, path);
                let form = self.forms.iter().find(|form| &form.path == path);
                let place = if path == "." {
                    "The project folder itself".to_owned()
                } else {
                    format!("{path}/ in the project folder")
                };
                let remote = match form {
                    Some(RepositoryForm {
                        url: Some(url),
                        remote,
                        ..
                    }) => format!(
                        "; its remote {remote} is {}, read from git",
                        short_remote(url, &self.folder)
                    ),
                    Some(form) => format!("; it has no remote {}", form.remote),
                    None => String::new(),
                };
                (format!("Repository {label}"), format!("{place}{remote}."))
            }
            Some(Dialog::Tracker) | None => (
                "Tracker".to_owned(),
                "Where the project's tickets are.".to_owned(),
            ),
        };
        let modal = Modal::new("settings-modal", title)
            .sentence(sentence)
            .open(self.dialog.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close_dialog(cx)).ok();
            });
        let Some(fields) = self.fields.clone() else {
            return modal;
        };
        // Made before the lines that borrow `cx` for as long as they live.
        let lists = match self.dialog.clone() {
            Some(Dialog::Repository(path)) => {
                let bases = self.base_field(&fields.branch, cx);
                Some((bases, self.depends_field(&path, cx.entity().downgrade())))
            }
            _ => None,
        };
        let error = problem_line(
            "settings-modal-error",
            self.dialog_error.clone(),
            px(0.),
            cx,
        );
        let save = Button::primary("save-settings-modal", "Save")
            .disabled(self.saving.is_some())
            .on_click(cx.listener(|this, _, _, cx| this.save_dialog(cx)));
        match self.dialog.clone() {
            None => modal,
            Some(Dialog::Repository(path)) => {
                let form = self.forms.iter().find(|form| form.path == path).cloned();
                let options = ForgeKind::ALL
                    .into_iter()
                    .map(|kind| SelectOption::new(kind.word(), kind.label()).line(kind.line()));
                let chosen = ForgeKind::ALL
                    .iter()
                    .position(|kind| *kind == self.forge_kind)
                    .unwrap_or(0);
                let this = cx.entity().downgrade();
                let select =
                    Select::new("forge-kind", options, chosen).on_choose(move |index, _, cx| {
                        this.update(cx, |this, cx| this.choose_forge(ForgeKind::ALL[index], cx))
                            .ok();
                    });
                let from_remote = form.as_ref().is_some_and(|form| {
                    form.declared.is_none()
                        && form
                            .proposal
                            .as_ref()
                            .is_some_and(|proposal| ForgeKind::of(proposal) == self.forge_kind)
                });
                let hint = if from_remote {
                    format!(
                        "From the remote {}: saved only once you save.",
                        form.as_ref().map_or("origin", |form| form.remote.as_str())
                    )
                } else {
                    format!("As {}.", self.forge_kind.shape())
                };
                let disagrees = form.as_ref().and_then(|form| form.disagrees.clone());
                let (bases, depends) = lists.expect("made for a repository's dialog");
                modal
                    .child(bases)
                    .child(depends)
                    .child(Field::new("forge-kind-field", "Forge", select))
                    .child(
                        presence(
                            "forge-host-block",
                            self.forge_kind == ForgeKind::BitbucketDataCenter,
                            Field::new(
                                "forge-host-field",
                                "Host",
                                boxed("forge-host", Input::new(&fields.host), cx),
                            ),
                        )
                        .gap(space::LG),
                    )
                    .child(
                        presence(
                            "forge-repository-block",
                            self.forge_kind != ForgeKind::None,
                            Field::new(
                                "forge-repository-field",
                                "Repository on the forge",
                                boxed("forge-repository", Input::new(&fields.repository), cx),
                            )
                            .hint(hint),
                        )
                        .gap(space::LG),
                    )
                    .child(
                        presence_of(
                            "forge-disagrees-block",
                            disagrees.map(|why| {
                                SharedString::from(format!(
                                    "The forge declared is refused until it agrees: {why}."
                                ))
                            }),
                            move |said| {
                                div()
                                    .id("forge-disagrees")
                                    .text_size(px(12.5))
                                    .text_color(p.wait)
                                    .child(said)
                                    .test_support()
                                    .into_any_element()
                            },
                        )
                        .gap(space::LG),
                    )
                    .kept(error)
                    .action(save)
            }
            Some(Dialog::Tracker) => {
                let options = TrackerKind::ALL
                    .into_iter()
                    .map(|kind| SelectOption::new(kind.word(), kind.label()).line(kind.line()));
                let chosen = TrackerKind::ALL
                    .iter()
                    .position(|kind| *kind == self.tracker_kind)
                    .unwrap_or(0);
                let this = cx.entity().downgrade();
                let select =
                    Select::new("tracker-kind", options, chosen).on_choose(move |index, _, cx| {
                        this.update(cx, |this, cx| {
                            this.choose_tracker(TrackerKind::ALL[index], cx)
                        })
                        .ok();
                    });
                let paths = self.github_paths();
                let chosen_path = self
                    .tracker_repository
                    .as_ref()
                    .and_then(|path| paths.iter().position(|p| p == path))
                    .unwrap_or(0);
                let forges: HashMap<String, String> = self
                    .forms
                    .iter()
                    .filter_map(|form| {
                        Some((
                            form.path.clone(),
                            form.declared.as_ref()?.repository().to_owned(),
                        ))
                    })
                    .collect();
                let repositories = paths.iter().map(|path| {
                    SelectOption::new(
                        SharedString::from(path.clone()),
                        SharedString::from(format!(
                            "{} · {}",
                            repository_label(&self.name, path),
                            forges.get(path).cloned().unwrap_or_default()
                        )),
                    )
                });
                let this = cx.entity().downgrade();
                let picked = paths.clone();
                let repository = Select::new("tracker-repository", repositories, chosen_path)
                    .on_choose(move |index, _, cx| {
                        let path = picked[index].clone();
                        this.update(cx, |this, cx| this.choose_tracker_repository(path, cx))
                            .ok();
                    });
                let github = if paths.is_empty() {
                    div()
                        .id("tracker-no-github")
                        .text_size(px(12.5))
                        .text_color(p.wait)
                        .child(
                            "No repository of the project is on GitHub: give one its forge in \
                             Repositories first.",
                        )
                        .test_support()
                        .into_any_element()
                } else {
                    Field::new("tracker-repository-field", "Of the repository", repository)
                        .hint("A ticket is an issue number: #123.")
                        .into_any_element()
                };
                modal
                    .child(Field::new("tracker-kind-field", "Tickets", select))
                    .child(
                        presence(
                            "tracker-github-block",
                            self.tracker_kind == TrackerKind::Github,
                            github,
                        )
                        .gap(space::LG),
                    )
                    .child(
                        presence(
                            "tracker-jira-block",
                            self.tracker_kind == TrackerKind::Jira,
                            div()
                                .flex()
                                .flex_col()
                                .gap(space::LG)
                                .child(Field::new(
                                    "tracker-site-field",
                                    "Site",
                                    boxed("tracker-site", Input::new(&fields.site), cx),
                                ))
                                .child(
                                    Field::new(
                                        "tracker-keys-field",
                                        "Project keys",
                                        boxed("tracker-keys", Input::new(&fields.keys), cx),
                                    )
                                    .hint(
                                        "A ticket is a key of one of these projects (ABC-123), \
                                         read with acli when it is signed in to this site.",
                                    ),
                                ),
                        )
                        .gap(space::LG),
                    )
                    .kept(error)
                    .action(save)
            }
        }
    }

    fn confirm(&self, cx: &mut Context<Self>) -> flottille_ui::Confirm {
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        flottille_ui::Confirm::new(
            "project-removal",
            format!("Remove the project {}?", self.name),
        )
        .sentence(
            "Flottille forgets it and its file. The folder and its repositories are not touched.",
        )
        .action("Remove project")
        .open(self.confirming_remove)
        .on_confirm(move |_, cx| {
            this.update(cx, |this, cx| this.remove(cx)).ok();
        })
        .on_cancel(move |_, cx| {
            again
                .update(cx, |this, cx| {
                    this.confirming_remove = false;
                    cx.notify();
                })
                .ok();
        })
    }

    /// Where it was opened at, from the project page.
    pub fn revealed(&self) -> Option<&Reveal> {
        self.revealed.as_ref()
    }

    /// Opened at a repository or at the tasks: their section scrolls up under the index.
    pub fn reveal(&mut self, at: Reveal, cx: &mut Context<Self>) {
        self.revealed = Some(at);
        self.revealing = true;
        cx.notify();
    }

    /// The index entry of the section in sight.
    pub fn current_section(&self) -> usize {
        self.sections.current(SECTIONS.len())
    }

    /// Scrolls the section of that index entry to the top, under the index.
    pub fn go_to(&mut self, entry: usize, cx: &mut Context<Self>) {
        self.sections.go_to(entry, cx);
        cx.notify();
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The folder as the header shows it: under the home folder, from `~`.
fn shown_folder(folder: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => match folder.strip_prefix(&home) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
            _ => folder.to_owned(),
        },
        _ => folder.to_owned(),
    }
}

impl Render for ProjectSettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Opened at a section: once what is above it is read, it scrolls up to it, and again
        // while what is above still unfolds.
        if self.revealing && self.has_read(cx) {
            let entry = self.revealed.as_ref().map_or(0, Reveal::entry);
            self.revealing = !self.sections.arrive_at(entry, cx);
        }
        if self.revealing {
            window.request_animation_frame();
        }
        let p = palette(cx);
        let pad = if self.narrow { space::LG } else { space::XXL };
        let crumb = {
            let shell = self.shell.clone();
            let back_to = self.name.clone();
            hoverable(
                div()
                    .id("settings-crumb")
                    .aria_label(SharedString::from(format!("{} /", self.name)))
                    .cursor_pointer()
                    .child(SharedString::from(format!("{} /", self.name)))
                    .on_click(move |_, window, cx| {
                        let selection = crate::shell::Selection::Project(back_to.clone());
                        shell
                            .update(cx, |shell, cx| shell.open(selection, window, cx))
                            .ok();
                    }),
                move |d, t| d.text_color(flottille_ui::color::mix(p.faint, p.text, t)),
            )
        };
        let header = div()
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(
                PageHeader::new("project-settings-header", "Settings")
                    .crumb(crumb)
                    .path(SharedString::from(shown_folder(&self.folder)))
                    .more(self.more.clone()),
            )
            .children(self.obsolete.iter().map(|obsolete| {
                div()
                    .text_size(px(12.5))
                    .text_color(p.wait)
                    .child(SharedString::from(format!("Not read: {obsolete}")))
            }))
            .child(presence(
                "asking-the-daemon",
                !self.loaded && self.error.is_none(),
                div()
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child("Asking the daemon…"),
            ))
            .child(presence_of(
                "project-settings-error-block",
                self.error.clone().map(SharedString::from),
                move |error| {
                    div()
                        .id("project-settings-error")
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error)
                        .test_support()
                        .into_any_element()
                },
            ));
        let sections: Vec<(&'static str, gpui_kit::AnyElement)> = vec![
            (
                "repositories",
                self.repositories_section(cx).into_any_element(),
            ),
            ("tracker", self.tracker_section(cx).into_any_element()),
            ("skills", self.skills.clone().into_any_element()),
            ("commands", self.commands.clone().into_any_element()),
            ("tasks", self.tasks.clone().into_any_element()),
            ("setup", self.setup.clone().into_any_element()),
            ("rules", self.rules.clone().into_any_element()),
        ];
        let this = cx.entity().downgrade();
        let index = move |id: ElementId, current: usize| {
            let this = this.clone();
            PageIndex::new(id, SECTIONS)
                .current(current)
                .on_pick(move |entry, _, cx| {
                    this.update(cx, |this, cx| this.go_to(entry, cx)).ok();
                })
        };
        let key = format!("settings-{}", self.name);
        let modal = self.dialog_modal(cx).into_any_element();
        let confirm = self.confirm(cx);
        let page = self.sections.page(
            "project-settings-page",
            &key,
            pad,
            header.into_any_element(),
            index,
            sections,
            window,
            cx,
        );
        div()
            .size_full()
            .text_color(p.text)
            .child(page)
            .child(modal)
            .child(self.repositories.clone())
            .child(confirm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_is_shown_in_its_short_form() {
        let folder = "/home/user/work/harbor";
        assert_eq!(
            short_remote("git@github.com:acme/api.git", folder),
            "github.com/acme/api"
        );
        assert_eq!(
            short_remote("https://github.com/acme/api.git", folder),
            "github.com/acme/api"
        );
        assert_eq!(
            short_remote("ssh://git@bitbucket.example.com:7999/proj/api.git", folder),
            "bitbucket.example.com/proj/api"
        );
        assert_eq!(
            short_remote("/home/user/work/atlas.git", folder),
            "../atlas.git"
        );
        assert_eq!(
            short_remote("file:///home/user/work/harbor/remote.git", folder),
            "remote.git"
        );
    }
}
