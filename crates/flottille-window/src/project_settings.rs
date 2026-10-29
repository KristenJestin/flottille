//! A project's settings page: the forge each repository is on and the tracker of its tickets
//! (`docs/projects.md`, "Forges and tracker"), re-checking or removing the project, then its
//! Skills, Commands, Tasks and Setup (`docs/skills.md`, `docs/commands.md`, `docs/projects.md`),
//! each section saved on its own, then its rules. Flottille
//! never guesses a forge: the form is pre-filled with what each repository's remote proposes, and
//! nothing is read nor written on a forge until the user saves. The window never writes the
//! project's file: Save asks the daemon (`project.set_settings`), which checks each forge against
//! its remote and keeps the file's comments.

use flottille_core::declared::{ProjectTracker, RepositoryForge};
use flottille_ui::{Button, Card, Dropdown, MenuItem, Type, enter, palette};
use gpui_kit::base::input::{Input, InputEditorStyle, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::CommandsSection;
use crate::project::repository_label;
use crate::rules::RulesEditor;
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
        .px(px(10.))
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

/// Which forge a repository is on, as the form chooses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgeKind {
    None,
    Github,
    BitbucketCloud,
    BitbucketDataCenter,
}

impl ForgeKind {
    const ALL: [ForgeKind; 4] = [
        ForgeKind::None,
        ForgeKind::Github,
        ForgeKind::BitbucketCloud,
        ForgeKind::BitbucketDataCenter,
    ];

    fn of(forge: &RepositoryForge) -> ForgeKind {
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

    fn label(self) -> &'static str {
        match self {
            ForgeKind::None => "No forge",
            ForgeKind::Github => "GitHub",
            ForgeKind::BitbucketCloud => "Bitbucket Cloud",
            ForgeKind::BitbucketDataCenter => "Bitbucket Data Center",
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
    const ALL: [TrackerKind; 3] = [TrackerKind::None, TrackerKind::Github, TrackerKind::Jira];

    pub fn word(self) -> &'static str {
        match self {
            TrackerKind::None => "none",
            TrackerKind::Github => "github",
            TrackerKind::Jira => "jira",
        }
    }

    fn label(self) -> &'static str {
        match self {
            TrackerKind::None => "No tracker",
            TrackerKind::Github => "GitHub issues",
            TrackerKind::Jira => "Jira",
        }
    }
}

/// One repository's row of the form.
pub struct RepositoryForm {
    pub path: String,
    pub remote: String,
    /// Its remote's address, credentials taken out; none without that remote.
    pub url: Option<String>,
    /// What the project's file declares now.
    pub declared: Option<RepositoryForge>,
    /// What the remote's address proposes.
    pub proposal: Option<RepositoryForge>,
    /// Why the declared forge is not the one the remote leads to.
    pub disagrees: Option<String>,
    pub kind: ForgeKind,
    host: Entity<InputState>,
    repository: Entity<InputState>,
}

impl RepositoryForm {
    /// The forge the form declares for it, `null` for none.
    fn forge(&self, cx: &App) -> Value {
        let repository = self.repository.read(cx).value().trim().to_owned();
        match self.kind {
            ForgeKind::None => Value::Null,
            ForgeKind::BitbucketDataCenter => json!({
                "kind": self.kind.word(),
                "host": self.host.read(cx).value().trim(),
                "repository": repository,
            }),
            kind => json!({"kind": kind.word(), "repository": repository}),
        }
    }

    /// Whether the form shows the remote's proposal, not yet saved.
    pub fn proposed(&self) -> bool {
        self.declared.is_none() && self.proposal.is_some() && self.kind != ForgeKind::None
    }
}

/// What the daemon last answered of a check, a removal or a save.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Fine(String),
    Refused(String),
}

/// An open picker of the page.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Picker {
    Forge(String),
    Tracker,
    TrackerRepository,
}

pub struct ProjectSettingsPage {
    shell: WeakEntity<Shell>,
    name: String,
    folder: String,
    /// The daemon answered once.
    loaded: bool,
    /// Why the settings cannot be read.
    error: Option<String>,
    repositories: Vec<RepositoryForm>,
    tracker: TrackerKind,
    tracker_repository: Option<String>,
    site: Entity<InputState>,
    keys: Entity<InputState>,
    obsolete: Vec<String>,
    picker: Option<Picker>,
    saving: bool,
    saved: Option<Said>,
    checking: bool,
    checked: Option<Said>,
    confirming_remove: bool,
    removing: bool,
    removed: Option<String>,
    narrow: bool,
    /// This project's own rules (`docs/projects.md`, "Rules").
    rules: Entity<RulesEditor>,
    skills: Entity<SkillsSection>,
    commands: Entity<CommandsSection>,
    tasks: Entity<TasksSection>,
    setup: Entity<SetupSection>,
}

impl ProjectSettingsPage {
    pub fn new(
        shell: WeakEntity<Shell>,
        name: String,
        window: &mut Window,
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
        // Asked once the shell, which is making this page, is no longer being updated.
        let (listing, skilling, tasking, setting) = (
            commands.clone(),
            skills.clone(),
            tasks.clone(),
            setup.clone(),
        );
        cx.defer(move |cx| {
            listing.update(cx, |section, cx| section.read(cx));
            skilling.update(cx, |section, cx| section.read(cx));
            tasking.update(cx, |section, cx| section.read(cx));
            setting.update(cx, |section, cx| section.read(cx));
        });
        Self {
            shell,
            name,
            folder: String::new(),
            loaded: false,
            error: None,
            repositories: Vec::new(),
            tracker: TrackerKind::None,
            tracker_repository: None,
            site: cx.new(|cx| styled_input("site, for example example.atlassian.net", window, cx)),
            keys: cx.new(|cx| styled_input("project keys, for example ABC, DEF", window, cx)),
            obsolete: Vec::new(),
            picker: None,
            saving: false,
            saved: None,
            checking: false,
            checked: None,
            confirming_remove: false,
            removing: false,
            removed: None,
            narrow: false,
            rules,
            skills,
            commands,
            tasks,
            setup,
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

    /// Whether the daemon has answered the lists the page asks for as it opens, with a list or
    /// with a problem: an answer redraws the page, so the tests wait for it.
    pub fn has_read(&self, cx: &App) -> bool {
        let commands = self.commands.read(cx);
        let skills = self.skills.read(cx);
        let tasks = self.tasks.read(cx);
        let setup = self.setup.read(cx);
        (commands.commands().is_some() || commands.problem().is_some())
            && (skills.skills().is_some() || skills.problem().is_some())
            && (tasks.tasks().is_some() || tasks.problem().is_some())
            && (setup.steps().is_some() || setup.problem().is_some())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn repositories(&self) -> &[RepositoryForm] {
        &self.repositories
    }

    pub fn tracker(&self) -> TrackerKind {
        self.tracker
    }

    pub fn saved(&self) -> Option<&Said> {
        self.saved.as_ref()
    }

    pub fn checked(&self) -> Option<&Said> {
        self.checked.as_ref()
    }

    pub fn removal_refused(&self) -> Option<&str> {
        self.removed.as_deref()
    }

    pub fn site_field(&self) -> &Entity<InputState> {
        &self.site
    }

    pub fn keys_field(&self) -> &Entity<InputState> {
        &self.keys
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// The daemon's answer to a read or a save: the form shows the file as it is now, each
    /// repository with no forge declared pre-filled with its remote's proposal.
    pub fn answered(
        &mut self,
        read: Result<Value, String>,
        saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if saved {
            self.saving = false;
            self.saved = Some(match &read {
                Ok(_) => Said::Fine(
                    "Saved: Flottille reads and writes these forges and this tracker from now on."
                        .into(),
                ),
                Err(why) => Said::Refused(why.clone()),
            });
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
        let mut rows = Vec::new();
        for repository in settings["repositories"].as_array().into_iter().flatten() {
            let declared = forge(&repository["forge"]);
            let proposal = forge(&repository["proposal"]);
            let shown = declared.clone().or_else(|| proposal.clone());
            let (host, name) = match &shown {
                Some(RepositoryForge::BitbucketDataCenter { host, repository }) => {
                    (host.clone(), repository.clone())
                }
                Some(other) => (String::new(), other.repository().to_owned()),
                None => (String::new(), String::new()),
            };
            let host_field = cx.new(|cx| {
                let mut field = styled_input("host, for example bitbucket.example.com", window, cx);
                field.set_value(host, window, cx);
                field
            });
            let repository_field = cx.new(|cx| {
                let mut field = styled_input("owner/repo", window, cx);
                field.set_value(name, window, cx);
                field
            });
            rows.push(RepositoryForm {
                path: repository["path"].as_str().unwrap_or_default().to_owned(),
                remote: repository["remote"].as_str().unwrap_or("origin").to_owned(),
                url: repository["url"].as_str().map(str::to_owned),
                kind: shown.as_ref().map_or(ForgeKind::None, ForgeKind::of),
                declared,
                proposal,
                disagrees: repository["disagrees"].as_str().map(str::to_owned),
                host: host_field,
                repository: repository_field,
            });
        }
        let paths = rows.iter().map(|row| row.path.clone()).collect();
        self.setup
            .update(cx, |section, cx| section.set_repositories(paths, cx));
        self.repositories = rows;
        let tracker = serde_json::from_value::<ProjectTracker>(settings["tracker"].clone()).ok();
        let (site, keys) = match &tracker {
            Some(ProjectTracker::Jira { site, projects }) => (site.clone(), projects.join(", ")),
            _ => (String::new(), String::new()),
        };
        self.site.update(cx, |f, cx| f.set_value(site, window, cx));
        self.keys.update(cx, |f, cx| f.set_value(keys, window, cx));
        (self.tracker, self.tracker_repository) = match tracker {
            None => (TrackerKind::None, None),
            Some(ProjectTracker::Github { repository }) => (TrackerKind::Github, Some(repository)),
            Some(ProjectTracker::Jira { .. }) => (TrackerKind::Jira, None),
        };
        cx.notify();
    }

    /// What Save sends: `project.set_settings`'s parameters, as the form says them now.
    pub fn form(&self, cx: &App) -> Value {
        let keys: Vec<String> = self
            .keys
            .read(cx)
            .value()
            .split([',', ' '])
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .collect();
        let tracker = match self.tracker {
            TrackerKind::None => Value::Null,
            TrackerKind::Github => json!({
                "kind": "github",
                "repository": self.tracker_repository.clone().or_else(|| self.github_paths().first().cloned()),
            }),
            TrackerKind::Jira => json!({
                "kind": "jira",
                "site": self.site.read(cx).value().trim(),
                "projects": keys,
            }),
        };
        json!({
            "name": self.name,
            "repositories": self.repositories.iter()
                .map(|r| json!({"path": r.path, "forge": r.forge(cx)}))
                .collect::<Vec<_>>(),
            "tracker": tracker,
        })
    }

    /// The repositories the form puts on GitHub: those whose issues can be the tracker.
    fn github_paths(&self) -> Vec<String> {
        self.repositories
            .iter()
            .filter(|r| r.kind == ForgeKind::Github)
            .map(|r| r.path.clone())
            .collect()
    }

    pub fn choose_forge(&mut self, path: &str, kind: ForgeKind, cx: &mut Context<Self>) {
        if let Some(row) = self.repositories.iter_mut().find(|r| r.path == path) {
            row.kind = kind;
        }
        self.picker = None;
        self.saved = None;
        cx.notify();
    }

    pub fn choose_tracker(&mut self, kind: TrackerKind, cx: &mut Context<Self>) {
        self.tracker = kind;
        self.picker = None;
        self.saved = None;
        cx.notify();
    }

    fn toggle(&mut self, picker: Picker, cx: &mut Context<Self>) {
        self.picker = match self.picker == Some(picker.clone()) {
            true => None,
            false => Some(picker),
        };
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || !self.loaded {
            return;
        }
        self.saving = true;
        self.saved = None;
        let command = Command::SaveProjectSettings {
            project: self.name.clone(),
            settings: self.form(cx),
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.checked = None;
        let command = Command::CheckProject(self.name.clone());
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// What the daemon answered of a check: the repositories match the file, or which differ.
    pub fn check_answered(&mut self, read: Result<Value, String>, cx: &mut Context<Self>) {
        self.checking = false;
        self.checked = Some(match read {
            Err(why) => Said::Refused(why),
            Ok(found) => {
                let (new, missing) = (strings(&found["new"]), strings(&found["missing"]));
                if new.is_empty() && missing.is_empty() {
                    Said::Fine("The repositories in the folder are the project's.".into())
                } else {
                    let mut said = Vec::new();
                    if !new.is_empty() {
                        said.push(format!("New in the folder: {}.", new.join(", ")));
                    }
                    if !missing.is_empty() {
                        said.push(format!("Gone from the folder: {}.", missing.join(", ")));
                    }
                    said.push("Edit the project's file to follow them.".into());
                    Said::Refused(said.join(" "))
                }
            }
        });
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
        self.removed = Some(why);
        cx.notify();
    }

    fn handler(
        cx: &Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |window, cx| {
            this.update(cx, |this, cx| f(this, window, cx)).ok();
        }
    }

    fn repository_card(&self, row: &RepositoryForm, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let path = row.path.clone();
        let id = |part: &str| ElementId::Name(format!("{part}-{path}").into());
        let items: Vec<MenuItem> = ForgeKind::ALL
            .into_iter()
            .map(|kind| {
                let path = path.clone();
                MenuItem::new(
                    ElementId::Name(format!("forge-kind-{path}-{}", kind.word()).into()),
                    kind.label(),
                )
                .selected(row.kind == kind)
                .on_click(Self::handler(cx, move |this, _, cx| {
                    this.choose_forge(&path, kind, cx)
                }))
            })
            .collect();
        let picker = {
            let path = path.clone();
            Dropdown::new(id("forge-kind"), row.kind.label())
                .strong()
                .open(self.picker == Some(Picker::Forge(path.clone())))
                .on_toggle(Self::handler(cx, move |this, _, cx| {
                    this.toggle(Picker::Forge(path.clone()), cx)
                }))
                .items(items)
        };
        let title = div()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(SharedString::from(repository_label(&self.name, &row.path)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(match &row.url {
                        Some(url) => format!("{} · {url}", row.remote),
                        None => format!("no remote {}", row.remote),
                    })),
            )
            .child(picker);
        let mut fields = div().flex().gap(px(8.));
        if row.kind == ForgeKind::BitbucketDataCenter {
            fields = fields.child(div().flex_1().child(field_frame(
                id("forge-host"),
                &row.host,
                cx,
            )));
        }
        if row.kind != ForgeKind::None {
            fields = fields.child(div().flex_1().child(field_frame(
                id("forge-repository"),
                &row.repository,
                cx,
            )));
        }
        let note = |text: String, color| {
            div()
                .text_size(px(12.5))
                .text_color(color)
                .child(SharedString::from(text))
        };
        let said = if let Some(why) = &row.disagrees {
            note(format!("Refused until it agrees: {why}."), p.fail)
        } else if row.proposed() {
            note(
                format!(
                    "Proposed from {}: nothing is read for it until you save.",
                    row.remote
                ),
                p.wait,
            )
        } else if row.kind == ForgeKind::None {
            note(
                "No forge: Flottille reads and writes nothing for this repository.".into(),
                p.faint,
            )
        } else {
            note(
                format!("{} {}", row.kind.label(), row.kind.shape()),
                p.faint,
            )
        };
        Card::new().title(title).child(
            div()
                .id(id("forge"))
                .px(px(14.))
                .py(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .when(row.kind != ForgeKind::None, |d| d.child(fields))
                .child(said)
                .test_support(),
        )
    }

    fn tracker_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let items: Vec<MenuItem> = TrackerKind::ALL
            .into_iter()
            .map(|kind| {
                MenuItem::new(
                    ElementId::Name(format!("tracker-kind-{}", kind.word()).into()),
                    kind.label(),
                )
                .selected(self.tracker == kind)
                .on_click(Self::handler(cx, move |this, _, cx| {
                    this.choose_tracker(kind, cx)
                }))
            })
            .collect();
        let title = div()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(8.))
            .child("Tickets")
            .child(div().flex_1())
            .child(
                Dropdown::new("tracker-kind", self.tracker.label())
                    .strong()
                    .open(self.picker == Some(Picker::Tracker))
                    .on_toggle(Self::handler(cx, |this, _, cx| {
                        this.toggle(Picker::Tracker, cx)
                    }))
                    .items(items),
            );
        let body = div()
            .id("tracker")
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .test_support();
        let line = |text: String, color| {
            div()
                .text_size(px(12.5))
                .text_color(color)
                .child(SharedString::from(text))
        };
        let body = match self.tracker {
            TrackerKind::None => body.child(line(
                "No tracker: no ticket is linked nor read for this project.".into(),
                p.faint,
            )),
            TrackerKind::Github => {
                let paths = self.github_paths();
                let chosen = self
                    .tracker_repository
                    .clone()
                    .filter(|path| paths.contains(path))
                    .or_else(|| paths.first().cloned());
                let items: Vec<MenuItem> = paths
                    .iter()
                    .map(|path| {
                        let picked = path.clone();
                        MenuItem::new(
                            ElementId::Name(format!("tracker-repository-{path}").into()),
                            repository_label(&self.name, path),
                        )
                        .selected(chosen.as_ref() == Some(path))
                        .on_click(Self::handler(cx, move |this, _, cx| {
                            this.tracker_repository = Some(picked.clone());
                            this.picker = None;
                            cx.notify();
                        }))
                    })
                    .collect();
                if paths.is_empty() {
                    body.child(line(
                        "The issues of a repository on GitHub: put one on GitHub above first."
                            .into(),
                        p.wait,
                    ))
                } else {
                    let label = chosen
                        .as_deref()
                        .map(|path| repository_label(&self.name, path))
                        .unwrap_or_default();
                    body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(line("The issues of".into(), p.mute))
                            .child(
                                Dropdown::new("tracker-repository", label)
                                    .mono()
                                    .open(self.picker == Some(Picker::TrackerRepository))
                                    .on_toggle(Self::handler(cx, |this, _, cx| {
                                        this.toggle(Picker::TrackerRepository, cx)
                                    }))
                                    .items(items),
                            ),
                    )
                    .child(line("A ticket is an issue number: #123.".into(), p.faint))
                }
            }
            TrackerKind::Jira => body
                .child(field_frame("tracker-site", &self.site, cx))
                .child(field_frame("tracker-keys", &self.keys, cx))
                .child(line(
                    "A ticket is a key of one of these projects (ABC-123), read through acli \
                     when its sign-in is on this site."
                        .into(),
                    p.faint,
                )),
        };
        Card::new().title(title).child(body)
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

impl Render for ProjectSettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pad = if self.narrow { 16. } else { 28. };
        let key =
            |part: &str| ElementId::Name(format!("enter-settings-{}-{part}", self.name).into());
        let shell = self.shell.clone();
        let back_to = self.name.clone();
        let mut actions = div().flex().items_center().gap(px(8.)).child(
            Button::quiet("project-page", "Back to the project").on_click(move |_, window, cx| {
                let selection = crate::shell::Selection::Project(back_to.clone());
                shell
                    .update(cx, |shell, cx| shell.open(selection, window, cx))
                    .ok();
            }),
        );
        actions = actions.child(
            Button::secondary(
                "check-project",
                if self.checking {
                    "Checking…"
                } else {
                    "Check again"
                },
            )
            .disabled(self.checking)
            .on_click(cx.listener(|this, _, _, cx| this.check(cx))),
        );
        actions = if self.removing {
            actions.child(div().mono(11.5).text_color(p.faint).child("Removing…"))
        } else if self.confirming_remove {
            actions
                .child(
                    Button::quiet("cancel-remove-project", "Cancel").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.confirming_remove = false;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    Button::secondary("confirm-remove-project", format!("Forget {}", self.name))
                        .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                )
        } else {
            actions.child(
                Button::quiet("remove-project", "Remove project").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.confirming_remove = true;
                        cx.notify();
                    },
                )),
            )
        };
        let header = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mb(px(20.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(24.))
                            .line_height(px(29.))
                            .semibold()
                            .child(SharedString::from(format!("{} settings", self.name))),
                    )
                    .child(div().flex_1())
                    .child(actions),
            )
            .child(
                div()
                    .mono(12.)
                    .text_color(p.faint)
                    .child(SharedString::from(self.folder.clone())),
            )
            .when(self.confirming_remove, |d| {
                d.child(div().text_size(px(12.5)).text_color(p.mute).child(
                    "Flottille forgets the project and its file; its folder and repositories \
                     stay as they are.",
                ))
            });
        let said = |id: &'static str, said: &Said| {
            let (text, color) = match said {
                Said::Fine(text) => (text.clone(), p.ok),
                Said::Refused(text) => (text.clone(), p.fail),
            };
            div()
                .id(id)
                .mb(px(12.))
                .text_size(px(12.5))
                .text_color(color)
                .child(SharedString::from(text))
        };
        let label = |text: &'static str| {
            div()
                .mt(px(10.))
                .mb(px(6.))
                .text_size(px(13.))
                .semibold()
                .child(text)
        };
        let note = |text: &'static str| {
            div()
                .mb(px(12.))
                .text_size(px(12.5))
                .text_color(p.faint)
                .child(text)
        };
        let rows: Vec<_> = self
            .repositories
            .iter()
            .enumerate()
            .map(|(i, row)| {
                enter(
                    key(&format!("repository-{}", row.path)),
                    4 + i,
                    div().mb(px(12.)).child(self.repository_card(row, cx)),
                )
            })
            .collect();
        let count = rows.len();
        let footer = div()
            .mt(px(8.))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(div().flex_1())
            .child(if self.saving {
                div()
                    .mono(11.5)
                    .text_color(p.faint)
                    .child("Saving…")
                    .into_any_element()
            } else {
                Button::primary("save-project-settings", "Save")
                    .disabled(!self.loaded)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    .into_any_element()
            });
        let tracker = self.tracker_card(cx).into_any_element();
        div()
            .id("project-settings-page")
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
                    .when_some(self.checked.clone(), |d, checked| {
                        d.child(said("project-checked", &checked))
                    })
                    .when_some(self.removed.clone(), |d, why| {
                        d.child(said("project-remove-error", &Said::Refused(why)))
                    })
                    .when_some(self.error.clone(), |d, why| {
                        d.child(said("project-settings-error", &Said::Refused(why)))
                    })
                    .children(self.obsolete.iter().map(|obsolete| {
                        div()
                            .mb(px(12.))
                            .text_size(px(12.5))
                            .text_color(p.wait)
                            .child(SharedString::from(format!("Not read: {obsolete}")))
                    }))
                    .when(!self.loaded && self.error.is_none(), |d| {
                        d.child(div().text_color(p.faint).child("Asking the daemon…"))
                    })
                    .child(enter(key("forges"), 1, label("Forges")))
                    .child(enter(
                        key("forges-note"),
                        2,
                        note(
                            "Flottille reads pull requests and writes to them only on \
                             the forge you declare here, and only while the repository's remote \
                             leads there.",
                        ),
                    ))
                    .children(rows)
                    .child(enter(key("tracker"), 4 + count, label("Tracker")))
                    .child(enter(
                        key("tracker-card"),
                        5 + count,
                        div().mb(px(12.)).child(tracker),
                    ))
                    .when_some(self.saved.clone(), |d, saved| {
                        d.child(said("project-settings-saved", &saved))
                    })
                    .child(footer)
                    .child(self.skills.clone())
                    .child(self.commands.clone())
                    .child(self.tasks.clone())
                    .child(self.setup.clone())
                    .child(enter(
                        key("rules"),
                        6 + count,
                        div().mt(px(24.)).mb(px(12.)).child(self.rules.clone()),
                    )),
            )
    }
}
