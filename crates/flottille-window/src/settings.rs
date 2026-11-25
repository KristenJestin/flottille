//! The settings page, in six tabs by subject (`docs/reference/gpui.md`, rule 13): Agents (the
//! providers, the rules for every project, the long tool results, the sessions), Approvals (the
//! Auto judge and what it is told), Features (a list of rows, each opening its page: the compaction
//! here, the others drawn from their declaration, `crate::extensions`), Vault, Machines (Remote:
//! whether it is on, where this machine is reached, the hub, the devices, the addresses) and
//! General (the appearance, the workspaces folder, the developer mode). Every provider Flottille knows is
//! enabled or not with a switch, as the daemon says it (`provider.list`). The window never writes
//! `providers.toml` itself: a switch asks the daemon (`provider.enable`, `provider.disable`), and
//! the page follows what the daemon lists next. The Auto judge is chosen here too
//! (`provider.set_judge`), and its guidance edited (`judge.set`). The window's appearance is chosen
//! (`appearance.set`). The vault's entries are added, replaced and removed (`vault.*`,
//! `crate::vault`). The developer mode is switched (`developer.set`).

use std::collections::HashMap;

use gpui_design_system::appearance::{self, Appearance, Source};
use gpui_design_system::theme::Shade;
use gpui_design_system::{
    Button, Chosen, EmptyLine, Field, Fold, Icon, ListCard, Listing, Menu, ModelChoice, PageHeader,
    Section, Segmented, Select, SelectOption, Skeleton, Subpage, Switch, Tabs, Toast, Type as _,
    awaiting, enter, hoverable, icon, palette, presence, presence_of, space, tipped, toast,
    turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, AppContext as _, Context, ElementId, Entity, InteractiveElement,
    IntoElement, ParentElement, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, Transformation, WeakEntity, Window,
    div, px, radians,
};
use serde_json::{Value, json};

use crate::about::AboutEditor;
use crate::actions::{Command, SessionsSetting};
use crate::commands::{spaced, stack};
use crate::extensions::Registry;
use crate::extensions::connections::{self, ConnectionsSection};
use crate::providers::{NewSession, Provider, Providers};
use crate::rules::RulesEditor;
use crate::shared_level::{Place, follows_line, set_here};
use crate::vault::VaultSection;
use crate::workspaces_folder::WorkspacesFolder;

/// The tabs of the page, in order: each one's key and label. A part of Flottille gets a tab of its
/// own when it fits (rule 13): a subject the user goes to on its own, with several sections of its
/// own, as Machines; a smaller part is a row of Features or a section of its subject's tab.
pub const SECTIONS: [(&str, &str); 6] = [
    ("agents", "Agents"),
    ("approvals", "Approvals"),
    ("features", "Features"),
    ("vault", "Vault"),
    ("machines", "Machines"),
    ("general", "General"),
];

/// The blocks of the Agents tab, top to bottom: each one is a section of the page, entering on its
/// own.
const AGENTS_BLOCKS: [&str; 6] = [
    "providers",
    "forges",
    "new-sessions",
    "rules",
    "tool-results",
    "sessions",
];

/// The blocks of the General tab, top to bottom.
const GENERAL_BLOCKS: [&str; 7] = [
    "about",
    "appearance",
    "workspaces",
    "travelled",
    "developer",
    "check",
    "version",
];

/// What comes before the blocks among the children of what scrolls: the line of another
/// machine's page (`SettingsPage::own_line`).
const LEADING_CHILDREN: usize = 1;

/// The blocks of a machine that are its own alone, with a hub: each says it once
/// (`docs/remote.md`, "Settings for all machines").
const MACHINE_ONLY: [&str; 6] = [
    "providers",
    "forges",
    "vault",
    "travelled",
    "developer",
    "check",
];

/// What a block of the machine alone says, above it, while the machine has a hub.
pub const MACHINE_ONLY_LINE: &str = "This machine only.";

/// Whether Settings shows All machines (`docs/remote.md`, "Settings for all machines"): the values
/// every machine uses, which this machine's daemon keeps and shares. Chosen in the picker, it
/// stays chosen while the window runs, until a machine is.
#[derive(Clone, Copy, Debug, Default)]
pub struct SettingsOfAll(pub bool);

impl gpui_kit::Global for SettingsOfAll {}

/// Whether Settings shows All machines now.
pub fn of_all(cx: &gpui_kit::App) -> bool {
    cx.try_global::<SettingsOfAll>().is_some_and(|all| all.0)
}

/// The blocks of the Approvals tab: the Auto judge, then what it is told.
const APPROVALS_BLOCKS: [&str; 2] = ["judge", "guidance"];

/// The row of Features that is the compaction's, and its place among the declared ones: after
/// Review, before Connections.
pub const COMPACTION: &str = "compaction";
const COMPACTION_ORDER: u64 = 5;

/// The compaction as the daemon says it (`compaction.show`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compaction {
    /// The compactor's provider, model and effort (`None` is the provider's or the model's
    /// default); none when each CLI compacts on its own.
    pub compactor: Option<(String, Option<String>, Option<String>)>,
    /// Flottille picked it and the user has not seen the notice yet.
    pub picked: bool,
    /// The agents write their own handoff, not the compactor.
    pub by_agent: bool,
    pub automatic: bool,
    /// Percent of the model's context window.
    pub threshold: u64,
    /// A ceiling in tokens; none is no ceiling.
    pub maximum: Option<u64>,
    /// Tokens kept word for word.
    pub kept: u64,
    /// An agent at rest is compacted `margin` minutes before its prompt cache expires.
    pub before_expiry: bool,
    pub margin: u64,
    /// A message to an agent whose cache expired compacts it first.
    pub after_expiry: bool,
    /// Below this context, in tokens, neither of the two compacts.
    pub minimum: u64,
    /// Each provider's cache lifetime in seconds, in the daemon's order; none when unknown.
    pub lifetimes: Vec<(String, Option<u64>)>,
}

impl Compaction {
    pub fn of(value: &Value) -> Compaction {
        let compactor = &value["compactor"];
        let text = |key: &str| compactor[key].as_str().map(str::to_owned);
        Compaction {
            compactor: text("provider").map(|provider| (provider, text("model"), text("effort"))),
            picked: compactor["picked"] == true,
            by_agent: value["summary"] == "agent",
            automatic: value["automatic"] != false,
            threshold: value["threshold"].as_u64().unwrap_or(70),
            maximum: value["maximum"].as_u64(),
            kept: value["kept"].as_u64().unwrap_or(20_000),
            before_expiry: value["before_expiry"] == true,
            margin: value["margin"].as_u64().unwrap_or(5),
            after_expiry: value["after_expiry"] != false,
            minimum: value["minimum"].as_u64().unwrap_or(50_000),
            lifetimes: value["lifetimes"]
                .as_object()
                .map(|lifetimes| {
                    lifetimes
                        .iter()
                        .map(|(provider, seconds)| (provider.clone(), seconds.as_u64()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// The thresholds the compaction's page offers, in percent; the daemon takes 10 to 95.
pub const THRESHOLDS: [u64; 5] = [50, 60, 70, 80, 90];
/// The maximum sizes it offers, in tokens, after No ceiling.
pub const MAXIMUMS: [u64; 4] = [100_000, 150_000, 200_000, 400_000];
/// The margins before a cache's expiry it offers, in minutes; the daemon takes 1 to 30.
pub const MARGINS: [u64; 4] = [2, 5, 10, 15];
/// The minimum sizes of the compaction around the cache's expiry it offers, in tokens.
pub const MINIMUMS: [u64; 4] = [20_000, 50_000, 100_000, 200_000];
/// The sizes kept word for word it offers, in tokens.
pub const KEPTS: [u64; 4] = [5_000, 10_000, 20_000, 40_000];

/// Why Omarchy cannot be chosen here, in the window's words: the kit's when its theme cannot be
/// used, ours when this is not an Omarchy desktop.
fn omarchy_refused(cx: &gpui_kit::App) -> Option<String> {
    let why = appearance::refused(crate::OMARCHY, cx)?;
    Some(match appearance::fallback(crate::OMARCHY, cx) {
        Some(_) => why,
        None => "Not an Omarchy desktop: the window uses Flottille's colours.".to_owned(),
    })
}

/// A provider's name as the window shows it.
fn provider_title(provider: &str) -> &str {
    match provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        other => other,
    }
}

/// The compaction's row of Features.
const COMPACTION_LINE: &str = "Long conversations summarised so agents keep going.";

/// Where and how the compaction acts (`**` around the words found elsewhere in the window).
const COMPACTION_USAGE: &str = "Automatic past the threshold below, or **Compact** in a session's \
     ⋯ menu, or the **Compact** control in its composer, left of Send, once the cache expired.";

/// What the compaction's switch says under its label.
pub const COMPACTION_HINT: &str = "When an agent's context fills up, its conversation is \
     summarised and it goes on from the summary. Off, each program compacts on its own.";

/// What the section of the new sessions says under its title.
pub const NEW_SESSIONS_SENTENCE: &str = "What a session starts on when its project says nothing \
     and you pick nothing: left alone, the provider's own recommendation.";

/// What the mode's row says under its label: which sessions it is for.
pub const NEW_SESSION_MODE_HINT: &str =
    "Sessions agents start, and children, keep their own rules.";

/// What the page says once when Full is chosen for new sessions: what Full lends.
pub const FULL_LENDS: &str = "In Full, a session's commands run as yours, as in your terminal: \
     nothing is held back, the vault included.";

/// The modes of a new session, in the order of the choice: the daemon's word and the word said.
const NEW_SESSION_MODES: [(&str, &str); 3] = [("ask", "Ask"), ("auto", "Auto"), ("full", "Full")];

/// What the page says of the summary's cost, under the model that writes it.
pub const COMPACTOR_COST: &str = "Each summary is a turn of this model, on your own quota.";

/// Where Settings was opened at, from a warning or a notice that leads to a setting: the tab that
/// holds it, with the setting in view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// No judge is chosen (the mode's warning, the judge notice): Approvals.
    Judge,
    /// No provider can start a session (the composer's "Open the settings"): Agents.
    Providers,
    /// The Auto judge's guidance: Approvals, scrolled to what the judge is told.
    Guidance,
    /// The rules for every project (the link above a project's rules): Agents, scrolled to them.
    Rules,
    /// The compaction (the notice of the compactor picked): its page of Features.
    Compaction,
    /// The vault (a Jira project's link to add the entry it logs time with).
    Vault,
    /// The page of a feature, by its name: its row of Features opened.
    Feature(String),
    /// This build's version and its update (the sidebar's foot): General, scrolled to it.
    Version,
    /// The user's machines: their tab.
    Machines,
}

impl Reveal {
    /// The setting an old link names: Machines were a section of General, and a link to it opens
    /// their tab (`general#machines`, `settings:general#machines`).
    pub fn from_link(link: &str) -> Option<Reveal> {
        match link.strip_prefix("settings:").unwrap_or(link) {
            "general#machines" | "machines" => Some(Reveal::Machines),
            _ => None,
        }
    }

    /// The index of its tab in [`SECTIONS`].
    fn tab(&self) -> usize {
        let key = match self {
            Reveal::Providers | Reveal::Rules => "agents",
            Reveal::Judge | Reveal::Guidance => "approvals",
            Reveal::Compaction | Reveal::Feature(_) => "features",
            Reveal::Vault => "vault",
            Reveal::Version => "general",
            Reveal::Machines => "machines",
        };
        SECTIONS
            .iter()
            .position(|(k, _)| *k == key)
            .expect("a tab of the page")
    }
}

/// The developer mode as the daemon says it (`developer.show`).
#[derive(Clone, Debug, PartialEq)]
pub struct Developer {
    /// Whether the daemon logs what it decides.
    pub log: bool,
    /// Where.
    pub file: String,
}

/// The limits the Tool results select offers, in characters; Off comes after them. The daemon
/// takes any number from 4,000 to 40,000 (`docs/protocol.md`): a file edited by hand keeps its
/// own, shown as one more option.
pub const LIMITS: [usize; 4] = [4_000, 8_000, 16_000, 32_000];

/// What the Long tool results section says under its title.
const TOOL_RESULTS_SENTENCE: &str = "A longer result reaches the agent as its start and its end; \
     the agent reads the rest when it needs it.";

/// What the switch that says a file is unchanged does, under its label.
pub const UNCHANGED_HINT: &str = "A file the agent already read and that has not changed \
     since is answered in one line, unless the session was compacted.";

/// The days the Sessions select offers. The daemon takes any number from 1 to 90
/// (`docs/protocol.md`): a file edited by hand keeps its own, shown as one more option.
pub const DAYS: [u64; 7] = [1, 3, 7, 14, 30, 60, 90];

/// The days a first switch on chooses.
const FIRST_DAYS: u64 = 7;

/// What the Sessions switch does, under its label.
pub const SESSIONS_HINT: &str =
    "Ends a session idle for the days below. One with work waiting is never ended.";

/// What the switch that resumes sessions at their quota's reset does, under its label.
pub const RESUME_HINT: &str = "A session stopped by its quota goes on at the reset. Each session \
     can change it in its composer.";

/// What the developer mode's switch logs, under its label.
pub const DEVELOPER_HINT: &str = "Why each call was allowed, held or refused, one JSON line each, \
     secrets masked. For working on Flottille.";

/// What the findings say under their title.
const FINDINGS_HINT: &str = "What agents reported about Flottille itself, one finding per bug. \
     Export them for the agent that files the issues.";

pub struct SettingsPage {
    shell: crate::commands::ShellOf,
    /// The machine whose Settings these are (`None`: this one): another machine's page enters as
    /// a page that shows up when it is chosen (#1271).
    of: Option<String>,
    /// The user's machines: the hub, its devices, this machine's addresses.
    machines: Entity<crate::machines_page::MachinesSection>,
    providers: Providers,
    /// A switch flipped, until the daemon lists the provider so: what it was asked to become.
    pending: HashMap<String, bool>,
    /// The daemon's refusal of the last switch, by provider.
    refused: HashMap<String, String>,
    narrow: bool,
    /// The list of the machine chooser at the top is open.
    machine_menu: bool,
    /// Where the sections that hold shared settings are drawn, as the page last drew them.
    place: Place,
    /// The user's rules for every project, edited on this page.
    rules: Entity<RulesEditor>,
    /// What the user says of themselves, edited on this page.
    about: Entity<AboutEditor>,
    /// The Auto judge's guidance, edited on this page.
    judge_guidance: Entity<RulesEditor>,
    /// The compactor's guidance, edited on this page.
    compaction_guidance: Entity<RulesEditor>,
    /// The compaction, once the daemon said it.
    compaction: Option<Compaction>,
    /// A change asked, until the daemon answers.
    compaction_pending: bool,
    /// Where the workspaces of every project go, chosen on this page.
    workspaces: Entity<WorkspacesFolder>,
    /// Where this machine makes the projects of other machines' launches, chosen on this page.
    travelled: Entity<crate::travelled_folder::TravelledFolder>,
    /// The vault's entries, set and removed on this page.
    vault: Entity<VaultSection>,
    connections: Entity<ConnectionsSection>,
    /// This build's version and its update.
    version: Entity<crate::version::VersionSection>,
    /// What `flottille doctor` checks.
    check: Entity<crate::check::CheckSection>,
    /// A judge chosen, until the daemon lists it.
    judge_pending: bool,
    /// The judge's switch flipped, until the daemon answers: what it was asked to become.
    judge_turning: Option<bool>,
    /// The daemon's refusal of the last judge chosen.
    judge_refused: Option<String>,
    /// What new sessions start on, chosen, until the daemon lists it.
    new_session_pending: bool,
    /// The mode of new sessions chosen, until the daemon lists it: what it was asked to become.
    new_session_mode_pending: Option<String>,
    /// The developer mode, once the daemon said it.
    developer: Option<Developer>,
    /// The developer mode's switch flipped, until the daemon answers: what it was asked to become.
    developer_pending: Option<bool>,
    /// The findings agents filed about Flottille itself, once the daemon said them.
    findings: Option<Vec<crate::findings::Finding>>,
    finding_rows: Listing<crate::findings::Finding>,
    /// The dialog that marks a finding filed, and the "⋯" of each open finding.
    filing: Entity<crate::findings::Filing>,
    finding_menus: HashMap<u64, Entity<Menu>>,
    /// The export asked, until the daemon answers.
    exporting: bool,
    /// The limit above which a tool result is cut, once the daemon said it (`Some(None)` is Off).
    tool_results: Option<Option<usize>>,
    /// What the daemon last said of the limits, with the levels they come from.
    tool_results_levels: Option<ToolResultsShown>,
    /// A limit chosen, until the daemon answers: what it was asked to become.
    tool_results_pending: Option<Option<usize>>,
    /// Whether `fs_read` says a file is unchanged, as the daemon last said it, and what the user
    /// asked it to become while the daemon has not answered.
    unchanged: Option<bool>,
    unchanged_pending: Option<bool>,
    /// After how many days an inactive session is ended, once the daemon said it (`Some(None)` is
    /// off).
    sessions: Option<Option<u64>>,
    /// A choice made, until the daemon answers: what it was asked to become.
    sessions_pending: Option<Option<u64>>,
    /// Whether a session is resumed at its quota's reset, once the daemon said it.
    resume: Option<bool>,
    /// The switch flipped, until the daemon answers: what it was asked to become.
    resume_pending: Option<bool>,
    /// The window's appearance, once the daemon said it.
    appearance: Option<Appearance>,
    /// Where the shade comes from: this machine's own, all machines'.
    shade_levels: Option<crate::actions::AppearanceLevels>,
    /// Why the daemon could not read the appearance it keeps: the tab says so and stays usable,
    /// a choice writes it again.
    appearance_unread: Option<String>,
    /// The programs' sign-ins: the forges' section, and what each provider's row offers.
    sign_in: Entity<crate::sign_in::SignInSection>,
    /// The tab chosen: the index of its section in [`SECTIONS`].
    tab: usize,
    /// The row of Features whose page is open, by its key: kept while another tab is shown.
    feature: Option<String>,
    /// The views of the parts the page draws inside a `Subpage` or a `Fold`, made once.
    parts: HashMap<Part, Entity<PagePart>>,
    /// The compaction's rare settings, folded under "More settings".
    more: Option<Entity<Fold>>,
    /// The scroll of the section shown, to bring a block in view from a link.
    scroll: ScrollHandle,
}

/// A part of the page that is a view of its own, because a `Subpage` or a `Fold` holds a view.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Part {
    /// The page of a row of Features, by its key.
    Feature(String),
    /// What the compaction's "More settings" holds.
    CompactionMore,
}

/// What the daemon says of the limits of tool results (`tool_results.show`): what this machine
/// reads, the keys it holds itself, and the values of all machines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolResultsShown {
    /// The limit in characters (`None` is Off).
    pub limit: Option<usize>,
    pub unchanged: bool,
    /// `limit` and `unchanged`, the keys this machine holds itself.
    pub own: Vec<String>,
    /// The values of all machines, `None` while this machine has no hub.
    pub all: Option<AllToolResults>,
}

/// The values of all machines of the limits of tool results: each `None` while they say none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AllToolResults {
    /// `Some(None)` is Off.
    pub limit: Option<Option<usize>>,
    pub unchanged: Option<bool>,
}

impl ToolResultsShown {
    pub fn of(value: &Value) -> Self {
        Self {
            limit: value["limit"].as_u64().map(|limit| limit as usize),
            unchanged: value["unchanged"] == true,
            own: value["own"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect(),
            all: value["all"].as_object().map(|all| AllToolResults {
                limit: match &all.get("limit") {
                    Some(Value::String(_)) => Some(None),
                    Some(limit) => limit.as_u64().map(|limit| Some(limit as usize)),
                    None => None,
                },
                unchanged: all.get("unchanged").and_then(Value::as_bool),
            }),
        }
    }

    fn holds(&self, key: &str) -> bool {
        self.own.iter().any(|own| own == key)
    }
}

/// A view that has the page draw one of its parts: it observes the page and the registry, and
/// draws again when either changes.
pub struct PagePart {
    page: WeakEntity<SettingsPage>,
    part: Part,
}

impl Render for PagePart {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let part = self.part.clone();
        self.page
            .update(cx, |page, cx| page.draw_part(&part, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// A judge, or a compactor, as the daemon lists it: its provider and its model, the provider's
/// default when none.
pub type Judge = (String, Option<String>);

/// What the judge's switch says under its label.
pub const JUDGE_HINT: &str = "The rules let through what is known to be harmless; the model allows the \
     rest or asks you. It never denies.";

/// Where and how the judge acts (`**` around the words found elsewhere in the window).
pub const JUDGE_USAGE: &str = "Used by sessions in **Auto**, chosen in each session's composer. \
     Base calls always come to you.";

/// What the page says of the judge's cost, under its model.
pub const JUDGE_QUOTA: &str = "Each decision is a turn of this model, on your own quota.";

impl SettingsPage {
    /// These Settings are `machine`'s.
    pub fn of_machine(mut self, machine: Option<String>) -> Self {
        self.of = machine;
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shell: crate::commands::ShellOf,
        rules: Entity<RulesEditor>,
        about: Entity<AboutEditor>,
        judge_guidance: Entity<RulesEditor>,
        compaction_guidance: Entity<RulesEditor>,
        workspaces: Entity<WorkspacesFolder>,
        travelled: Entity<crate::travelled_folder::TravelledFolder>,
        vault: Entity<VaultSection>,
        connections: Entity<ConnectionsSection>,
        machines: Entity<crate::machines_page::MachinesSection>,
        version: Entity<crate::version::VersionSection>,
        sign_in: Entity<crate::sign_in::SignInSection>,
        filing: Entity<crate::findings::Filing>,
        check: Entity<crate::check::CheckSection>,
    ) -> Self {
        Self {
            of: None,
            sign_in,
            check,
            machines,
            version,
            rules,
            about,
            judge_guidance,
            compaction_guidance,
            compaction: None,
            compaction_pending: false,
            workspaces,
            travelled,
            vault,
            connections,
            shell,
            providers: Providers::default(),
            pending: HashMap::new(),
            refused: HashMap::new(),
            narrow: false,
            machine_menu: false,
            place: Place::default(),
            judge_pending: false,
            judge_turning: None,
            judge_refused: None,
            new_session_pending: false,
            new_session_mode_pending: None,
            developer: None,
            developer_pending: None,
            findings: None,
            finding_rows: Listing::default(),
            filing,
            finding_menus: HashMap::new(),
            exporting: false,
            tool_results: None,
            tool_results_levels: None,
            tool_results_pending: None,
            unchanged: None,
            unchanged_pending: None,
            sessions: None,
            sessions_pending: None,
            resume: None,
            resume_pending: None,
            appearance: None,
            appearance_unread: None,
            shade_levels: None,
            tab: 0,
            feature: None,
            parts: HashMap::new(),
            more: None,
            scroll: ScrollHandle::new(),
        }
    }

    /// The tab chosen: the index of its section in [`SECTIONS`].
    pub fn tab(&self) -> usize {
        self.tab
    }

    /// Chooses the tab of that section: it shows alone.
    pub fn choose(&mut self, entry: usize, cx: &mut Context<Self>) {
        let entry = entry.min(SECTIONS.len() - 1);
        if self.tab != entry {
            self.tab = entry;
            cx.notify();
        }
    }

    /// The row of Features whose page is open.
    pub fn open_feature(&self) -> Option<&str> {
        self.feature.as_deref()
    }

    /// Opens the page of a row of Features, or goes back to the list.
    pub fn set_feature(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        if self.feature != key {
            self.feature = key;
            cx.notify();
        }
    }

    /// The registry of the shell, while it is there.
    fn registry(&self, cx: &gpui_kit::App) -> Option<Entity<Registry>> {
        self.shell
            .upgrade()
            .map(|shell| shell.read(cx).extensions().clone())
    }

    /// Opened at a setting: its tab is the one chosen, its page the one open, and it is in view.
    pub fn reveal(&mut self, at: Reveal, cx: &mut Context<Self>) {
        self.choose(at.tab(), cx);
        // The tab may already be shown, scrolled: the link says where the page starts.
        match &at {
            Reveal::Guidance => self.scroll_to(&APPROVALS_BLOCKS, "guidance"),
            Reveal::Rules => self.scroll_to(&AGENTS_BLOCKS, "rules"),
            Reveal::Compaction => {
                self.feature = Some(COMPACTION.to_owned());
                self.scroll.set_offset(Point::default());
            }
            Reveal::Feature(name) => {
                self.feature = Some(name.clone());
                self.scroll.set_offset(Point::default());
            }
            Reveal::Judge | Reveal::Providers => self.scroll.set_offset(Point::default()),
            Reveal::Vault | Reveal::Machines => {}
            Reveal::Version => self.scroll_to(&GENERAL_BLOCKS, "version"),
        }
        cx.notify();
    }

    fn scroll_to(&self, blocks: &[&str], block: &str) {
        let at = blocks
            .iter()
            .position(|key| *key == block)
            .expect("a block of the tab");
        // The sentence of another machine's page is the first child of what scrolls (#1366): the
        // blocks come after it, drawn or not.
        self.scroll.scroll_to_top_of_item(at + LEADING_CHILDREN);
    }

    pub fn rules(&self) -> &Entity<RulesEditor> {
        &self.rules
    }

    pub fn about(&self) -> &Entity<AboutEditor> {
        &self.about
    }

    pub fn judge_guidance(&self) -> &Entity<RulesEditor> {
        &self.judge_guidance
    }

    pub fn compaction_guidance(&self) -> &Entity<RulesEditor> {
        &self.compaction_guidance
    }

    /// The compaction as the daemon last said it.
    pub fn compaction(&self) -> Option<&Compaction> {
        self.compaction.as_ref()
    }

    /// Asks the daemon to change the compaction's settings: the fields of `params` only.
    pub fn set_compaction(&mut self, params: Value, cx: &mut Context<Self>) {
        if self.compaction_pending || self.compaction.is_none() {
            return;
        }
        self.compaction_pending = true;
        self.shell.perform(Command::CompactionSet { params }, cx);
        cx.notify();
    }

    /// The daemon's answer to a read or a change: a change refused stays as it was, and a toast
    /// says why.
    pub fn compaction_answered(
        &mut self,
        shown: Result<Compaction, String>,
        written: bool,
        cx: &mut Context<Self>,
    ) {
        if written {
            self.compaction_pending = false;
        }
        match shown {
            Ok(shown) => {
                if written {
                    toast(
                        Toast::success("Compaction saved")
                            .detail("It applies at the next end of a turn."),
                        cx,
                    );
                }
                self.compaction = Some(shown);
            }
            Err(why) if written => toast(
                Toast::failure("The compaction was not changed").detail(why),
                cx,
            ),
            Err(_) => {}
        }
        cx.notify();
    }

    /// The compaction's page of Features (`docs/extensions.md`, "Compaction"): its switch, its
    /// sentence and its usage line, and while it is on the summary's model, "Compact
    /// automatically" with the threshold under it, "More settings" folded, and what a summary
    /// keeps.
    fn compaction_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let shown = self.compaction.clone().unwrap_or(Compaction {
            compactor: None,
            picked: false,
            by_agent: false,
            automatic: true,
            threshold: 70,
            maximum: None,
            kept: 20_000,
            before_expiry: false,
            margin: 5,
            after_expiry: true,
            minimum: 50_000,
            lifetimes: Vec::new(),
        });
        let off = self.compaction.is_none() || self.compaction_pending;
        let this = cx.entity().downgrade();
        let some = shown.compactor.is_some() || shown.by_agent;
        let switch = Switch::new("compaction-switch", some, "Compact long conversations")
            .hint(COMPACTION_HINT)
            .disabled(off)
            .on_click(cx.listener(move |this, _, _, cx| {
                let on = this
                    .compaction
                    .as_ref()
                    .is_some_and(|c| c.compactor.is_some() || c.by_agent);
                let params = if on {
                    json!({"compactor": "none", "summary": "compactor"})
                } else {
                    json!({"compactor": "pick"})
                };
                this.set_compaction(params, cx)
            }));
        let chosen = match (&shown.compactor, shown.by_agent) {
            (_, true) => Chosen::Choice("agent".into()),
            (Some((provider, model, _)), false) => {
                self.providers.chosen_running(provider, model.as_deref())
            }
            (None, false) => Chosen::First,
        };
        // Marked recommended: the model Flottille picks for the compactor.
        let smallest = Providers::smallest();
        let summary = ModelChoice::new("compaction-summary", "Summary written by", chosen)
            .choice(
                "agent",
                "The session's own agent",
                "The agent summarises its own conversation, in its session.",
            )
            .models(self.providers.model_entries_giving(&smallest))
            .effort(
                shown
                    .compactor
                    .as_ref()
                    .and_then(|(_, _, effort)| effort.clone().map(Into::into)),
            )
            .picked(shown.picked)
            .cost(COMPACTOR_COST)
            .disabled(off)
            .on_choose({
                let this = this.clone();
                let providers = self.providers.clone();
                move |chosen, effort, _, cx| {
                    let params = match chosen {
                        Chosen::Model(provider, model) => json!({
                            "summary": "compactor",
                            "compactor": {
                                "provider": provider,
                                "model": model.map(|m| m.to_string()).or_else(|| {
                                    providers.recommended(&provider, &smallest)
                                }),
                                "effort": effort,
                            },
                        }),
                        _ => json!({"summary": "agent"}),
                    };
                    this.update(cx, |this, cx| this.set_compaction(params, cx))
                        .ok();
                }
            });
        let automatic = Switch::new(
            "compaction-automatic",
            shown.automatic,
            "Compact automatically",
        )
        .hint("At the end of a turn, past the threshold. Off, only when you ask.")
        .disabled(off)
        .on_click(cx.listener(move |this, _, _, cx| {
            let on = this.compaction.as_ref().is_some_and(|c| c.automatic);
            this.set_compaction(json!({"automatic": !on}), cx)
        }));
        let select = Self::compaction_select(off, this);
        let threshold = select(
            "compaction-threshold",
            THRESHOLDS.iter().map(|v| Some(*v)).collect(),
            Some(shown.threshold),
            |v| format!("{} % of the context window", v.unwrap_or(0)),
            "threshold",
        );
        let more = self.more_settings(window, cx);
        let card = div()
            .id("compaction")
            .p(space::LG)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(switch)
            .child(div().pt(space::SM).child(crate::extensions::usage_line(
                "compaction-usage",
                COMPACTION_USAGE,
                cx,
            )))
            .child(presence(
                "compaction-summary-block",
                some,
                div().pt(space::MD).child(summary),
            ))
            .child(presence(
                "compaction-settings-block",
                some,
                div()
                    .pt(space::MD)
                    .flex()
                    .flex_col()
                    .child(automatic)
                    .child(
                        presence(
                            "compaction-threshold-block",
                            shown.automatic,
                            Field::new(
                                "compaction-threshold-field",
                                "When the context reaches",
                                threshold,
                            ),
                        )
                        .space(space::MD),
                    )
                    .child(spaced(space::MD, more)),
            ))
            .test_support();
        div()
            .flex()
            .flex_col()
            .child(enter("compaction-page-enter", 0, card))
            .child(
                presence(
                    "compaction-guidance-block",
                    some,
                    self.compaction_guidance.clone(),
                )
                .space(space::XXL),
            )
            .into_any_element()
    }

    /// A select of numbers, one of which is chosen, for a setting of the compaction: the value in
    /// force is offered too when a file edited by hand holds another.
    #[allow(clippy::type_complexity)]
    fn compaction_select(
        off: bool,
        this: WeakEntity<Self>,
    ) -> impl Fn(
        &'static str,
        Vec<Option<u64>>,
        Option<u64>,
        fn(Option<u64>) -> String,
        &'static str,
    ) -> Select {
        move |id, values, now, label, key| {
            let mut values = values;
            if !values.contains(&now) {
                values.push(now);
                values.sort();
            }
            let at = values.iter().position(|v| *v == now).unwrap_or(0);
            let options = values.iter().map(|v| {
                SelectOption::new(
                    SharedString::from(v.map_or("none".to_owned(), |v| v.to_string())),
                    SharedString::from(label(*v)),
                )
            });
            let picked = values.clone();
            let this = this.clone();
            Select::new(id, options, at)
                .disabled(off)
                .on_choose(move |index, _, cx| {
                    let value = picked[index].map_or(json!("none"), |v| json!(v));
                    this.update(cx, |this, cx| this.set_compaction(json!({key: value}), cx))
                        .ok();
                })
        }
    }

    /// "More settings": a header with a turning chevron, and under it, folded, what is rarely
    /// set.
    fn more_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let page = cx.entity();
        let fold = match self.more.clone() {
            Some(fold) => fold,
            None => {
                let content: AnyView = self.part_view(Part::CompactionMore, cx).into();
                let fold = cx.new(|_| Fold::new("compaction-more-fold", content, false));
                self.more = Some(fold.clone());
                fold
            }
        };
        let open = fold.read(cx).is_open();
        let turned = turning("compaction-more-chevron", open, window, cx);
        let header = hoverable(
            div()
                .id("compaction-more")
                .aria_label("More settings")
                .flex()
                .items_center()
                .gap(space::XS)
                .cursor_pointer()
                .text_size(px(12.5))
                .child(icon(Icon::Chevron).size(px(12.)).with_transformation(
                    Transformation::rotate(radians(-std::f32::consts::FRAC_PI_2 * turned)),
                ))
                .child("More settings")
                .on_click({
                    let fold = fold.clone();
                    move |_, _, cx| {
                        fold.update(cx, |fold, cx| fold.toggle(cx));
                        page.update(cx, |_, cx| cx.notify());
                    }
                }),
            move |d, t| d.text_color(gpui_design_system::color::mix(p.faint, p.text, t)),
        );
        div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(header)
            .child(fold)
            .into_any_element()
    }

    /// What "More settings" holds: the maximum size, the messages kept as they are, the two
    /// settings around the cache's expiry and the cache lifetime of each provider.
    fn compaction_more(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let shown = self.compaction.clone().unwrap_or(Compaction {
            compactor: None,
            picked: false,
            by_agent: false,
            automatic: true,
            threshold: 70,
            maximum: None,
            kept: 20_000,
            before_expiry: false,
            margin: 5,
            after_expiry: true,
            minimum: 50_000,
            lifetimes: Vec::new(),
        });
        let off = self.compaction.is_none() || self.compaction_pending;
        let select = Self::compaction_select(off, cx.entity().downgrade());
        let mut maximums: Vec<Option<u64>> = vec![None];
        maximums.extend(MAXIMUMS.iter().map(|v| Some(*v)));
        let maximum = select(
            "compaction-maximum",
            maximums,
            shown.maximum,
            |v| match v {
                None => "No ceiling".to_owned(),
                Some(v) => format!("{} tokens", grouped(v as usize)),
            },
            "maximum",
        );
        let kept = select(
            "compaction-kept",
            KEPTS.iter().map(|v| Some(*v)).collect(),
            Some(shown.kept),
            |v| format!("{} tokens", grouped(v.unwrap_or(0) as usize)),
            "kept",
        );
        let before_expiry = Switch::new(
            "compaction-before-expiry",
            shown.before_expiry,
            "Compact an idle agent before its cache expires",
        )
        .disabled(off)
        .on_click(cx.listener(move |this, _, _, cx| {
            let on = this.compaction.as_ref().is_some_and(|c| c.before_expiry);
            this.set_compaction(json!({"before_expiry": !on}), cx)
        }));
        let margin = select(
            "compaction-margin",
            MARGINS.iter().map(|v| Some(*v)).collect(),
            Some(shown.margin),
            |v| format!("{} minutes before", v.unwrap_or(0)),
            "margin",
        );
        let after_expiry = Switch::new(
            "compaction-after-expiry",
            shown.after_expiry,
            "Compact when a session comes back after its cache expired",
        )
        .disabled(off)
        .on_click(cx.listener(move |this, _, _, cx| {
            let on = this.compaction.as_ref().is_some_and(|c| c.after_expiry);
            this.set_compaction(json!({"after_expiry": !on}), cx)
        }));
        let minimum = select(
            "compaction-minimum",
            MINIMUMS.iter().map(|v| Some(*v)).collect(),
            Some(shown.minimum),
            |v| format!("{} tokens", grouped(v.unwrap_or(0) as usize)),
            "minimum",
        );
        let lifetimes = shown
            .lifetimes
            .iter()
            .map(|(provider, seconds)| {
                let title = provider_title(provider);
                match seconds {
                    Some(seconds) if seconds % 3_600 == 0 => {
                        format!("{title}: {} hour", seconds / 3_600).replace(" 1 hour", " an hour")
                    }
                    Some(seconds) => format!("{title}: {} minutes", seconds / 60),
                    None => format!("{title}: unknown, neither compacts"),
                }
            })
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .flex()
            .flex_col()
            .pl(space::LG)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::MD)
                    .child(Field::new("compaction-maximum-field", "Or above", maximum))
                    .child(Field::new(
                        "compaction-kept-field",
                        "Recent messages kept as they are",
                        kept,
                    ))
                    .child(before_expiry),
            )
            .child(
                presence(
                    "compaction-margin-block",
                    shown.before_expiry,
                    Field::new("compaction-margin-field", "How long before", margin),
                )
                .space(space::MD),
            )
            .child(
                div()
                    .mt(space::MD)
                    .flex()
                    .flex_col()
                    .gap(space::MD)
                    .child(after_expiry)
                    .child(Field::new("compaction-minimum-field", "Not below", minimum))
                    .child(
                        div()
                            .id("compaction-lifetimes")
                            .text_size(px(12.))
                            .text_color(p.faint)
                            .aria_label(SharedString::from(format!(
                                "Prompt cache lifetimes. {lifetimes}"
                            )))
                            .child(format!("Prompt cache lifetimes. {lifetimes}"))
                            .test_support(),
                    ),
            )
            .into_any_element()
    }

    pub fn workspaces_folder(&self) -> &Entity<WorkspacesFolder> {
        &self.workspaces
    }

    pub fn travelled_folder(&self) -> &Entity<crate::travelled_folder::TravelledFolder> {
        &self.travelled
    }

    pub fn sign_in(&self) -> &Entity<crate::sign_in::SignInSection> {
        &self.sign_in
    }

    pub fn vault(&self) -> &Entity<VaultSection> {
        &self.vault
    }

    pub fn connections(&self) -> &Entity<ConnectionsSection> {
        &self.connections
    }

    pub fn machines(&self) -> &Entity<crate::machines_page::MachinesSection> {
        &self.machines
    }

    pub fn providers(&self) -> &Providers {
        &self.providers
    }

    /// What the page says of a provider, line by line: its state, once, and the daemon's refusal
    /// of its switch.
    pub fn said_of(&self, name: &str) -> Vec<String> {
        let Some(provider) = self.providers.get(name) else {
            return Vec::new();
        };
        let mut lines = vec![provider.said()];
        lines.extend(self.refused.get(name).cloned());
        lines
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// Whether the provider's switch shows it on: what it was asked to become while the daemon
    /// has not listed it so.
    pub fn switch_on(&self, name: &str) -> bool {
        self.pending
            .get(name)
            .copied()
            .unwrap_or_else(|| self.providers.get(name).is_some_and(|p| p.enabled))
    }

    /// Whether a switch of the provider waits for the daemon.
    pub fn is_pending(&self, name: &str) -> bool {
        self.pending.contains_key(name)
    }

    pub fn set_providers(&mut self, providers: &Providers, cx: &mut Context<Self>) {
        // A switch shows what it was asked to become until the daemon lists it so.
        self.pending
            .retain(|name, enabled| providers.get(name).is_some_and(|p| p.enabled != *enabled));
        if self.judge_turning == Some(providers.judge.is_some()) {
            self.judge_turning = None;
        }
        if self.providers != *providers {
            self.providers = providers.clone();
            cx.notify();
        }
    }

    /// The daemon's answer to a switch: on a refusal the switch goes back, and a toast says why.
    pub fn answered(&mut self, name: &str, refused: Option<String>, cx: &mut Context<Self>) {
        match refused {
            Some(why) => {
                self.pending.remove(name);
                let title = self
                    .providers
                    .get(name)
                    .map_or_else(|| name.to_owned(), |p| p.title.clone());
                toast(
                    Toast::failure(format!("{title} was not switched")).detail(why.clone()),
                    cx,
                );
                self.refused.insert(name.to_owned(), why);
            }
            None => {
                self.refused.remove(name);
            }
        }
        cx.notify();
    }

    fn flip(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(provider) = self.providers.get(name) else {
            return;
        };
        if self.pending.contains_key(name) {
            return;
        }
        let enabled = !provider.enabled;
        self.pending.insert(name.to_owned(), enabled);
        self.refused.remove(name);
        let command = Command::SetProvider {
            name: name.to_owned(),
            enabled,
        };
        self.shell.perform(command, cx);
        cx.notify();
    }

    /// The judge as the page shows it: `None`, or a provider's title and the model.
    pub fn judge_shown(&self) -> (String, Option<String>) {
        match &self.providers.judge {
            None => ("No judge".into(), None),
            Some((name, model)) => {
                let title = self
                    .providers
                    .get(name)
                    .map_or_else(|| name.clone(), |p| p.title.clone());
                (
                    title,
                    Some(self.providers.model_line(name, model.as_deref())),
                )
            }
        }
    }

    /// Asks the daemon for this judge, or for none.
    pub fn choose_judge(&mut self, judge: Option<Judge>, cx: &mut Context<Self>) {
        if self.judge_pending || judge == self.providers.judge {
            cx.notify();
            return;
        }
        self.judge_pending = true;
        self.judge_refused = None;
        let (provider, model) = match judge {
            Some((provider, model)) => (Some(provider), model),
            None => (None, None),
        };
        self.shell
            .perform(Command::SetJudge { provider, model }, cx);
        cx.notify();
    }

    /// Whether the judge's switch shows it on: what it was asked to become while the daemon has
    /// not answered.
    pub fn judge_on(&self) -> bool {
        self.judge_turning
            .unwrap_or_else(|| self.providers.judge.is_some())
    }

    /// The judge's switch: off writes no judge (`judge = "none"`), on asks the daemon to pick
    /// one as for a user who never chose.
    fn flip_judge(&mut self, cx: &mut Context<Self>) {
        if self.judge_pending {
            return;
        }
        let on = !self.judge_on();
        self.judge_turning = Some(on);
        if !on {
            self.choose_judge(None, cx);
            return;
        }
        self.judge_pending = true;
        self.judge_refused = None;
        self.shell.perform(Command::PickJudge, cx);
        cx.notify();
    }

    /// The daemon's answer to a judge chosen: a toast says it.
    pub fn judge_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.judge_pending = false;
        if refused.is_some() {
            self.judge_turning = None;
        }
        match &refused {
            Some(why) => toast(
                Toast::failure("The judge was not changed").detail(why.clone()),
                cx,
            ),
            None => toast(
                Toast::success("Auto judge saved").detail("It decides the next judgement."),
                cx,
            ),
        }
        self.judge_refused = refused;
        cx.notify();
    }

    /// Asks the daemon for what new sessions start on: the provider, and its model and effort,
    /// or `None` for what each provider proposes.
    pub fn choose_new_session(&mut self, chosen: Option<NewSession>, cx: &mut Context<Self>) {
        if self.new_session_pending || chosen == self.providers.new_session {
            cx.notify();
            return;
        }
        self.new_session_pending = true;
        let (provider, model, effort) = match chosen {
            Some((provider, model, effort)) => (Some(provider), model, effort),
            None => (None, None, None),
        };
        self.shell.perform(
            Command::SetNewSession {
                provider,
                model,
                effort,
            },
            cx,
        );
        cx.notify();
    }

    /// The mode new sessions start in as the page shows it: the one asked for while the daemon
    /// has not listed it, else what the machine keeps (Auto until it said).
    fn new_session_mode_shown(&self) -> &str {
        self.new_session_mode_pending
            .as_deref()
            .or(self.providers.new_session_mode.as_deref())
            .unwrap_or("auto")
    }

    /// Asks the daemon for the mode a new session starts in.
    pub fn choose_new_session_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.new_session_mode_pending.is_some() || self.new_session_mode_shown() == mode {
            return;
        }
        self.new_session_mode_pending = Some(mode.to_owned());
        let mode = mode.to_owned();
        self.shell.perform(Command::SetNewSessionMode { mode }, cx);
        cx.notify();
    }

    /// The daemon's answer to what new sessions start on: a toast says it.
    pub fn new_session_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.new_session_pending = false;
        match refused {
            Some(why) => toast(
                Toast::failure("New sessions were not changed").detail(why),
                cx,
            ),
            None => toast(
                Toast::success("New sessions saved").detail("They apply to the next session."),
                cx,
            ),
        }
        cx.notify();
    }

    /// The daemon's answer to the mode new sessions start in: a toast of its own says it.
    pub fn new_session_mode_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        // Taken, the mode stays on the choice while the providers are read again: it never slides
        // back to the old one and forward once more.
        let asked = self.new_session_mode_pending.take();
        if let (None, Some(mode)) = (&refused, asked) {
            self.providers.new_session_mode = Some(mode);
        }
        match refused {
            Some(why) => toast(
                Toast::failure("The mode of new sessions was not changed").detail(why),
                cx,
            ),
            None => toast(
                Toast::success("New session mode saved")
                    .detail("It applies to the next session you start."),
                cx,
            ),
        }
        cx.notify();
    }

    /// What the page says once when what new sessions start on is no longer offered by its
    /// provider: they start on what the provider proposes instead.
    pub fn new_session_fallback(&self) -> Option<String> {
        let (name, model, effort) = self.providers.new_session.as_ref()?;
        let provider = self.providers.get(name)?;
        let offers_model = model
            .as_ref()
            .is_none_or(|model| provider.models.is_none() || provider.model(model).is_some());
        let gone = if !offers_model {
            model.clone()?
        } else {
            let effort = effort.as_ref()?;
            let efforts = crate::composer::efforts_of(provider, model.as_deref())?;
            if efforts.contains(effort) {
                return None;
            }
            effort.clone()
        };
        Some(format!(
            "{} no longer offers {gone}: new sessions start on what it proposes.",
            provider.title
        ))
    }

    /// What new sessions start on as the pickers show it: what the daemon holds, less a model or
    /// an effort its provider no longer offers, which is what a session starts without.
    fn new_session_shown(&self) -> Option<NewSession> {
        let (name, model, effort) = self.providers.new_session.clone()?;
        let provider = self.providers.get(&name)?;
        let model =
            model.filter(|model| provider.models.is_none() || provider.model(model).is_some());
        let effort = effort.filter(|effort| {
            crate::composer::efforts_of(provider, model.as_deref())
                .is_none_or(|efforts| efforts.contains(effort))
        });
        Some((name, model, effort))
    }

    /// The developer mode as the daemon last said it.
    pub fn developer(&self) -> Option<&Developer> {
        self.developer.as_ref()
    }

    /// Whether the developer mode's switch shows it on: what it was asked to become while the
    /// daemon has not answered.
    pub fn developer_on(&self) -> bool {
        self.developer_pending
            .unwrap_or_else(|| self.developer.as_ref().is_some_and(|d| d.log))
    }

    fn flip_developer(&mut self, cx: &mut Context<Self>) {
        if self.developer_pending.is_some() || self.developer.is_none() {
            return;
        }
        let log = !self.developer_on();
        self.developer_pending = Some(log);
        self.shell.perform(Command::DeveloperSet { log }, cx);
        cx.notify();
    }

    /// The daemon's answer to a read or a switch: a switch refused goes back, and a toast says
    /// why.
    pub fn developer_answered(
        &mut self,
        shown: Result<Developer, String>,
        written: bool,
        cx: &mut Context<Self>,
    ) {
        if written {
            self.developer_pending = None;
        }
        match shown {
            Ok(developer) => self.developer = Some(developer),
            Err(why) if written => toast(
                Toast::failure("Developer mode was not switched").detail(why),
                cx,
            ),
            Err(_) => {}
        }
        cx.notify();
    }

    /// The daemon's list of the findings: the rows that came unfold, those that left fold away.
    pub fn findings_answered(
        &mut self,
        listed: Result<Vec<crate::findings::Finding>, String>,
        cx: &mut Context<Self>,
    ) {
        let Ok(listed) = listed else { return };
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.finding_rows.sync(
            listed
                .iter()
                .map(|finding| (finding.id.to_string(), finding.clone())),
            now,
            reduce,
        );
        self.findings = Some(listed);
        cx.notify();
    }

    /// The Check Flottille section.
    pub fn check(&self) -> &Entity<crate::check::CheckSection> {
        &self.check
    }

    /// The dialog that marks a finding filed.
    pub fn filing(&self) -> &Entity<crate::findings::Filing> {
        &self.filing
    }

    /// The findings as they are listed.
    pub fn findings(&self) -> &[crate::findings::Finding] {
        self.findings.as_deref().unwrap_or_default()
    }

    /// Whether an export was asked and the daemon has not answered.
    pub fn exporting(&self) -> bool {
        self.exporting
    }

    fn export_findings(&mut self, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        self.exporting = true;
        self.shell.perform(Command::FindingsExport, cx);
        cx.notify();
    }

    /// Where the findings were written, or why they were not: a toast says it.
    pub fn findings_exported(&mut self, done: Result<String, String>, cx: &mut Context<Self>) {
        self.exporting = false;
        match done {
            Ok(file) => toast(Toast::success("Findings exported").detail(file), cx),
            Err(why) => toast(Toast::failure("Findings were not exported").detail(why), cx),
        }
        cx.notify();
    }

    /// What the findings add under the developer mode's switch: the list, and the export that
    /// writes it for the lead agent. It comes with the switch and folds away with it.
    fn findings_block(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let mut list = ListCard::new("findings-list");
        let drawn: Vec<_> = self
            .finding_rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let open: Vec<u64> = drawn
            .iter()
            .filter(|(_, _, finding)| finding.filed.is_none())
            .map(|(_, _, finding)| finding.id)
            .collect();
        self.finding_menus.retain(|id, _| open.contains(id));
        for id in open {
            if !self.finding_menus.contains_key(&id) {
                let menu = crate::findings::menu(id, self.filing.downgrade(), cx);
                self.finding_menus.insert(id, menu);
            }
        }
        for (key, placement, finding) in drawn {
            let menu = self.finding_menus.get(&finding.id).cloned();
            let row = crate::findings::row(&finding, menu, cx);
            list = list.item(
                ElementId::Name(format!("findings-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.finding_rows.len();
        let empty = self.findings.as_ref().is_some_and(Vec::is_empty);
        div()
            .id("findings")
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::MD)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .child(format!("Findings · {count}")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(p.mute)
                                    .child(FINDINGS_HINT),
                            ),
                    )
                    .child(
                        div().flex_none().child(
                            Button::secondary("findings-export", "Export findings")
                                .leading(Icon::File)
                                .disabled(count == 0 || self.exporting)
                                .on_click(cx.listener(|this, _, _, cx| this.export_findings(cx))),
                        ),
                    ),
            )
            .child(presence("findings-list-block", count > 0, list).space(space::MD))
            .child(self.filing.clone())
            .child(
                presence(
                    "no-findings-block",
                    empty,
                    EmptyLine::new(
                        "no-findings",
                        Icon::Warning,
                        "No finding yet. An agent files one when Flottille itself got \
                         something wrong.",
                    ),
                )
                .space(space::MD),
            )
    }

    fn developer_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let switch = Switch::new(
            "developer-switch",
            self.developer_on(),
            "Log what Flottille decides",
        )
        .hint(DEVELOPER_HINT)
        .disabled(self.developer.is_none() || self.developer_pending.is_some())
        .on_click(cx.listener(|this, _, _, cx| this.flip_developer(cx)));
        // The path is the log's: it comes with the switch, folds away with it.
        let file = self
            .developer
            .as_ref()
            .filter(|_| self.developer_on())
            .map(|d| SharedString::from(d.file.clone()));
        let card = div()
            .id("developer")
            .p(space::LG)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(switch)
            .child(
                presence_of("developer-file-line", file, move |file| {
                    div()
                        .id("developer-file")
                        .aria_label(file.clone())
                        .flex()
                        .gap(space::SM)
                        .text_size(px(12.5))
                        .child(div().flex_none().text_color(p.mute).child("Log"))
                        .child(
                            div()
                                .min_w_0()
                                .font_family(gpui_design_system::typography::MONO)
                                .text_color(p.faint)
                                .child(file),
                        )
                        .test_support()
                        .into_any_element()
                })
                .space(space::MD),
            )
            .child(
                presence(
                    "developer-findings-block",
                    self.developer_on(),
                    self.findings_block(cx),
                )
                .space(space::MD),
            )
            .test_support();
        Section::new("developer", "Developer mode").child(card)
    }

    /// Whether the daemon said the limit of tool results.
    pub fn tool_results_known(&self) -> bool {
        self.tool_results.is_some()
    }

    /// The limit as the daemon last said it (`Some(None)` is Off), whatever was asked since.
    pub fn tool_results_said(&self) -> Option<Option<usize>> {
        self.tool_results
    }

    /// The limit the select shows, in characters (`None` is Off): what it was asked to become
    /// while the daemon has not answered.
    pub fn tool_results_limit(&self) -> Option<usize> {
        // On All machines, what they say, else the default.
        let said = match (self.place.on_all, &self.tool_results_levels) {
            (true, Some(levels)) => Some(
                levels
                    .all
                    .as_ref()
                    .and_then(|all| all.limit)
                    .unwrap_or(Some(8_000)),
            ),
            _ => self.tool_results,
        };
        self.tool_results_pending.or(said).unwrap_or(Some(8_000))
    }

    /// Whether the switch of unchanged files shows on: the daemon's word where the section is
    /// drawn, whatever was asked since.
    fn unchanged_shown(&self) -> bool {
        let said = match (self.place.on_all, &self.tool_results_levels) {
            (true, Some(levels)) => Some(
                levels
                    .all
                    .as_ref()
                    .and_then(|all| all.unchanged)
                    .unwrap_or(false),
            ),
            _ => self.unchanged,
        };
        self.unchanged_pending.or(said).unwrap_or(false)
    }

    /// Whether this machine holds the limit, or the switch of unchanged files, itself.
    fn holds(&self, key: &str) -> bool {
        self.tool_results_levels
            .as_ref()
            .is_some_and(|levels| levels.holds(key))
    }

    /// Drops this machine's own `key`: it follows all machines' value again.
    fn use_all_tool_results(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if self.asked_tool_results() || !self.holds(key) {
            return;
        }
        match key {
            "limit" => self.tool_results_pending = Some(self.tool_results.flatten()),
            _ => self.unchanged_pending = self.unchanged,
        }
        self.shell.perform(
            Command::ToolResultsReset {
                key: key.to_owned(),
            },
            cx,
        );
        cx.notify();
    }

    /// Whether the daemon last said `fs_read` answers "unchanged" for a file the agent already
    /// read (`None` before it said), whatever was asked since.
    pub fn unchanged_said(&self) -> Option<bool> {
        self.unchanged
    }

    fn asked_tool_results(&self) -> bool {
        self.tool_results_pending.is_some() || self.unchanged_pending.is_some()
    }

    fn choose_unchanged(&mut self, cx: &mut Context<Self>) {
        if self.unchanged.is_none() {
            return;
        }
        let unchanged = self.unchanged_shown();
        if self.asked_tool_results() {
            return;
        }
        self.unchanged_pending = Some(!unchanged);
        let command = match self.place.on_all {
            true => Command::Shared {
                name: "tool_results.unchanged".to_owned(),
                value: (!unchanged).to_string(),
                then: Box::new(Command::ToolResultsRead),
            },
            false => Command::UnchangedSet {
                unchanged: !unchanged,
            },
        };
        self.shell.perform(command, cx);
        cx.notify();
    }

    fn choose_tool_results(&mut self, limit: Option<usize>, cx: &mut Context<Self>) {
        if self.asked_tool_results() || self.tool_results.is_none() {
            return;
        }
        self.tool_results_pending = Some(limit);
        let command = match self.place.on_all {
            true => Command::Shared {
                name: "tool_results.limit".to_owned(),
                value: limit.map_or("off".to_owned(), |limit| limit.to_string()),
                then: Box::new(Command::ToolResultsRead),
            },
            false => Command::ToolResultsSet { limit },
        };
        self.shell.perform(command, cx);
        cx.notify();
    }

    /// The daemon's answer to a read or a choice: a choice refused goes back, and a toast says
    /// why.
    pub fn tool_results_answered(
        &mut self,
        shown: Result<ToolResultsShown, String>,
        written: bool,
        cx: &mut Context<Self>,
    ) {
        let asked_unchanged = self.unchanged_pending.is_some();
        if written {
            self.tool_results_pending = None;
            self.unchanged_pending = None;
        }
        match shown {
            Ok(shown) => {
                self.tool_results = Some(shown.limit);
                self.unchanged = Some(shown.unchanged);
                self.tool_results_levels = Some(shown);
            }
            Err(why) if written => toast(
                Toast::failure(if asked_unchanged {
                    "The switch for unchanged files was not kept"
                } else {
                    "The limit of tool results was not kept"
                })
                .detail(why),
                cx,
            ),
            Err(_) => {}
        }
        cx.notify();
    }

    fn tool_results_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.tool_results_limit();
        let mut limits: Vec<Option<usize>> = LIMITS.iter().map(|l| Some(*l)).collect();
        if let Some(own) = shown
            && !LIMITS.contains(&own)
        {
            limits.push(Some(own));
            limits.sort();
        }
        limits.push(None);
        let options = limits.iter().map(|limit| match limit {
            Some(limit) => SelectOption::new(
                SharedString::from(limit.to_string()),
                SharedString::from(format!("{} characters", grouped(*limit))),
            ),
            None => SelectOption::new("off", "Off").line("Every result goes to the agent whole."),
        });
        let chosen = limits.iter().position(|l| *l == shown).unwrap_or(0);
        let this = cx.entity().downgrade();
        let picked = limits.clone();
        let select = Select::new("tool-results-limit", options, chosen)
            .disabled(self.tool_results.is_none() || self.asked_tool_results())
            .on_choose(move |index, _, cx| {
                let limit = picked[index];
                this.update(cx, |this, cx| this.choose_tool_results(limit, cx))
                    .ok();
            });
        let p = palette(cx);
        let unchanged = Switch::new(
            "tool-results-unchanged-switch",
            self.unchanged_shown(),
            "Say when a file is unchanged",
        )
        .hint(UNCHANGED_HINT)
        .disabled(self.unchanged.is_none() || self.asked_tool_results())
        .on_click(cx.listener(|this, _, _, cx| this.choose_unchanged(cx)));
        let unchanged_card = div()
            .id("tool-results-unchanged")
            .p(space::LG)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(unchanged)
            .test_support();
        // On a machine with a hub, a limit or a switch it holds itself wears the mark of a value
        // set apart, and the way back to all machines' value.
        let place = self.place.clone();
        let follows = place.follows()
            && self
                .tool_results_levels
                .as_ref()
                .is_some_and(|levels| levels.all.is_some());
        let marked = |key: &'static str, element: AnyElement, cx: &mut Context<Self>| {
            if !follows {
                return element;
            }
            let this = cx.entity().downgrade();
            set_here(
                &format!("tool-results-{key}"),
                &place,
                element,
                self.holds(key),
                move |_, cx| {
                    this.update(cx, |this, cx| this.use_all_tool_results(key, cx))
                        .ok();
                },
            )
            .into_any_element()
        };
        let limit_field = marked(
            "limit",
            Field::new("tool-results-field", "Cut above", select).into_any_element(),
            cx,
        );
        let unchanged_card = marked("unchanged", unchanged_card.into_any_element(), cx);
        Section::new("tool-results", "Long tool results")
            .sentence(TOOL_RESULTS_SENTENCE)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(follows_line("tool-results", follows, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::MD)
                            .child(limit_field)
                            .child(unchanged_card),
                    ),
            )
    }

    /// Whether the daemon said the setting of inactive sessions.
    pub fn sessions_known(&self) -> bool {
        self.sessions.is_some()
    }

    /// The days as the daemon last said them (`Some(None)` is off), whatever was asked since.
    pub fn sessions_said(&self) -> Option<Option<u64>> {
        self.sessions
    }

    /// The days the page shows (`None` is off): what it was asked to become while the daemon has
    /// not answered.
    pub fn sessions_days(&self) -> Option<u64> {
        self.sessions_pending.or(self.sessions).flatten()
    }

    fn choose_sessions(&mut self, days: Option<u64>, cx: &mut Context<Self>) {
        if self.sessions_pending.is_some() || self.sessions.is_none() {
            return;
        }
        self.sessions_pending = Some(days);
        self.shell.perform(Command::SessionsSet { days }, cx);
        cx.notify();
    }

    /// The daemon's answer to a read or a choice: a choice refused goes back, and a toast says
    /// why.
    pub fn sessions_answered(
        &mut self,
        shown: Result<(Option<u64>, bool), String>,
        written: Option<SessionsSetting>,
        cx: &mut Context<Self>,
    ) {
        match written {
            Some(SessionsSetting::Days) => self.sessions_pending = None,
            Some(SessionsSetting::Resume) => self.resume_pending = None,
            None => {}
        }
        match shown {
            Ok((days, resume)) => {
                self.sessions = Some(days);
                self.resume = Some(resume);
            }
            Err(why) => match written {
                Some(SessionsSetting::Days) => toast(
                    Toast::failure("Ending inactive sessions was not changed").detail(why),
                    cx,
                ),
                Some(SessionsSetting::Resume) => toast(
                    Toast::failure("Resuming at the quota's reset was not changed").detail(why),
                    cx,
                ),
                None => {}
            },
        }
        cx.notify();
    }

    /// Whether a session is resumed at its quota's reset, as the daemon last said it.
    pub fn resume_said(&self) -> Option<bool> {
        self.resume
    }

    fn choose_resume(&mut self, cx: &mut Context<Self>) {
        let Some(resume) = self.resume else {
            return;
        };
        if self.resume_pending.is_some() {
            return;
        }
        self.resume_pending = Some(!resume);
        self.shell
            .perform(Command::ResumeSet { resume: !resume }, cx);
        cx.notify();
    }

    fn sessions_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let shown = self.sessions_days();
        let switch = Switch::new("sessions-switch", shown.is_some(), "End inactive sessions")
            .hint(SESSIONS_HINT)
            .disabled(self.sessions.is_none() || self.sessions_pending.is_some())
            .on_click(cx.listener(|this, _, _, cx| {
                let days = match this.sessions_days() {
                    Some(_) => None,
                    None => Some(FIRST_DAYS),
                };
                this.choose_sessions(days, cx)
            }));
        let mut days: Vec<u64> = DAYS.to_vec();
        if let Some(own) = shown
            && !DAYS.contains(&own)
        {
            days.push(own);
            days.sort();
        }
        let options = days.iter().map(|days| {
            SelectOption::new(
                SharedString::from(days.to_string()),
                SharedString::from(match days {
                    1 => "1 day".to_owned(),
                    days => format!("{days} days"),
                }),
            )
        });
        let chosen = days
            .iter()
            .position(|days| Some(*days) == shown)
            .unwrap_or(0);
        let this = cx.entity().downgrade();
        let picked = days.clone();
        let select = Select::new("sessions-days", options, chosen)
            .disabled(self.sessions.is_none() || self.sessions_pending.is_some())
            .on_choose(move |index, _, cx| {
                let days = picked[index];
                this.update(cx, |this, cx| this.choose_sessions(Some(days), cx))
                    .ok();
            });
        let card = div()
            .id("sessions")
            .p(space::LG)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(switch)
            .child(presence(
                "sessions-days-block",
                shown.is_some(),
                div()
                    .pt(space::MD)
                    .child(Field::new("sessions-days-field", "After", select)),
            ))
            .test_support();
        let resume = Switch::new(
            "sessions-resume-switch",
            self.resume_pending.or(self.resume).unwrap_or(false),
            "Resume when the quota resets",
        )
        .hint(RESUME_HINT)
        .disabled(self.resume.is_none() || self.resume_pending.is_some())
        .on_click(cx.listener(|this, _, _, cx| this.choose_resume(cx)));
        let resume_card = div()
            .id("sessions-resume")
            .p(space::LG)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(resume)
            .test_support();
        Section::new("sessions", "Sessions").child(
            div()
                .flex()
                .flex_col()
                .gap(space::MD)
                .child(card)
                .child(resume_card),
        )
    }

    /// Whether the daemon answered about the window's appearance, with it or with why it cannot
    /// read it: until then it cannot be chosen.
    pub fn appearance_known(&self) -> bool {
        self.appearance.is_some() || self.appearance_unread.is_some()
    }

    /// Uses `appearance` at once, and asks the daemon to keep it for every window.
    pub fn choose_appearance(
        &mut self,
        appearance: Appearance,
        shade: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.appearance_known() || self.appearance.as_ref() == Some(&appearance) {
            return;
        }
        self.appearance = Some(appearance.clone());
        appearance::appear(appearance.clone(), cx);
        crate::look::remember(&appearance, cx);
        self.shell
            .update(cx, |shell, _| {
                shell.ask_appearance_keeping(Some(appearance), shade)
            })
            .ok();
        cx.notify();
    }

    /// Keeps `shade` as the shade of all machines: the window shows what this machine reads next.
    fn share_shade(&mut self, shade: Shade, cx: &mut Context<Self>) {
        let word = match shade {
            Shade::Light => "light",
            Shade::Dark => "dark",
        };
        self.shell
            .update(cx, |shell, _| shell.share_shade(word))
            .ok();
        cx.notify();
    }

    /// Drops this machine's own shade: it follows all machines' again.
    fn use_all_shade(&mut self, cx: &mut Context<Self>) {
        if !self.shade_levels.as_ref().is_some_and(|levels| levels.own) {
            return;
        }
        self.shell.update(cx, |shell, _| shell.use_all_shade()).ok();
        cx.notify();
    }

    /// Where the shade comes from, as the daemon said it.
    pub fn shade_levels_answered(
        &mut self,
        levels: Result<crate::actions::AppearanceLevels, String>,
        cx: &mut Context<Self>,
    ) {
        if let Ok(levels) = levels {
            self.shade_levels = Some(levels);
            cx.notify();
        }
    }

    /// Whether this machine holds its own shade, while it has a hub.
    pub fn shade_is_own(&self) -> bool {
        self.shade_levels.as_ref().is_some_and(|levels| levels.own)
    }

    /// The shade of all machines, when they say one.
    pub fn shade_of_all(&self) -> Option<&str> {
        self.shade_levels
            .as_ref()
            .and_then(|levels| levels.all.as_deref())
    }

    /// The daemon's answer to a read or a choice: a choice not kept is said in a toast (the
    /// window reads the appearance again).
    pub fn appearance_answered(
        &mut self,
        shown: Result<Appearance, String>,
        written: bool,
        cx: &mut Context<Self>,
    ) {
        match shown {
            Ok(appearance) => {
                self.appearance = Some(appearance);
                self.appearance_unread = None;
            }
            Err(why) if written => toast(
                Toast::failure("The appearance was not kept").detail(why),
                cx,
            ),
            // What it keeps cannot be read: the window shows what it uses, a choice writes it.
            Err(why) => self.appearance_unread = Some(why),
        }
        cx.notify();
    }

    fn appearance_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let known = self.appearance_known();
        let shown = appearance::shown(cx);
        let kept = self.appearance.clone().unwrap_or(shown.clone());
        let refused = omarchy_refused(cx);
        let row = |label: &'static str, hint: &'static str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(space::LG)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .child(div().text_size(px(13.)).child(label))
                        .child(div().text_size(px(12.5)).text_color(p.mute).child(hint)),
                )
                .child(control)
        };
        let this = cx.entity().downgrade();
        let theme = Segmented::new(
            "appearance-theme",
            "Colours",
            [("omarchy", "Omarchy"), ("flottille", "Flottille")],
            match shown.source {
                Source::Desktop(_) => 0,
                Source::Theme(_) => 1,
            },
        )
        .disabled(!known)
        .when(refused.is_some(), |s| s.unavailable(0))
        .on_choose(move |index, _, cx| {
            let source = if index == 0 {
                Source::Desktop(crate::OMARCHY.into())
            } else {
                Source::Theme(crate::OWN_THEME.into())
            };
            this.update(cx, |this, cx| {
                this.choose_appearance(
                    Appearance {
                        source,
                        shade: kept.shade,
                    },
                    false,
                    cx,
                )
            })
            .ok();
        });
        let this = cx.entity().downgrade();
        let shade = Segmented::new(
            "appearance-shade",
            "Shade",
            [("light", "Light"), ("dark", "Dark")],
            // On All machines, the shade they say; the dark one while they say none.
            match (self.place.on_all, self.shade_of_all(), shown.shade) {
                (true, Some("light"), _) | (false, _, Shade::Light) => 0,
                _ => 1,
            },
        )
        .disabled(!known)
        .on_choose(move |index, _, cx| {
            let shade = if index == 0 {
                Shade::Light
            } else {
                Shade::Dark
            };
            this.update(cx, |this, cx| match this.place.on_all {
                // On All machines the shade every machine follows, signed with the device key.
                true => this.share_shade(shade, cx),
                false => this.choose_appearance(
                    Appearance {
                        source: Source::Theme(crate::OWN_THEME.into()),
                        shade,
                    },
                    true,
                    cx,
                ),
            })
            .ok();
        });
        // On a machine with a hub, a shade it holds itself wears the mark of a value set apart,
        // and the way back to all machines' shade.
        let on_all = self.place.on_all;
        let follows =
            self.place.follows() && self.shade_levels.as_ref().is_some_and(|levels| levels.hub);
        let shade_row = row(
            "Shade",
            match on_all {
                true => "Used by every machine that did not choose its own.",
                false => "",
            },
            shade.into_any_element(),
        );
        let shade_row = match follows {
            true => {
                let this = cx.entity().downgrade();
                set_here(
                    "appearance-shade",
                    &self.place,
                    shade_row,
                    self.shade_is_own(),
                    move |_, cx| {
                        this.update(cx, |this, cx| this.use_all_shade(cx)).ok();
                    },
                )
                .into_any_element()
            }
            false => shade_row.into_any_element(),
        };
        // On All machines the colours are the machine's own: only the shade is shown.
        let card = div()
            .id("appearance")
            .p(space::LG)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(follows_line("appearance", follows, cx))
            .when(!on_all, |card| {
                card.child(row(
                    "Colours",
                    "Omarchy follows your desktop's theme as it changes.",
                    theme.into_any_element(),
                ))
            })
            .child(
                presence_of(
                    "appearance-omarchy-why-line",
                    refused.map(SharedString::from),
                    move |why| {
                        div()
                            .id("appearance-omarchy-why")
                            .aria_label(why.clone())
                            .text_size(px(12.5))
                            .text_color(p.faint)
                            .child(why)
                            .test_support()
                            .into_any_element()
                    },
                )
                .space(space::MD),
            )
            .child(
                presence_of(
                    "appearance-unread-line",
                    self.appearance_unread.as_ref().map(|why| {
                        SharedString::from(format!("The kept appearance could not be read: {why}"))
                    }),
                    move |line| {
                        div()
                            .id("appearance-unread")
                            .aria_label(line.clone())
                            .text_size(px(12.5))
                            .text_color(p.faint)
                            .child(line)
                            .test_support()
                            .into_any_element()
                    },
                )
                .space(space::MD),
            )
            .child(
                presence(
                    "appearance-shade-block",
                    on_all || matches!(shown.source, Source::Theme(_)),
                    shade_row,
                )
                .space(space::MD),
            )
            .test_support();
        Section::new("appearance", "Appearance").child(card)
    }

    fn judge_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let on = self.judge_on();
        let switch = Switch::new("judge-switch", on, "Let a model decide in Auto")
            .hint(JUDGE_HINT)
            .disabled(self.judge_pending)
            .on_click(cx.listener(|this, _, _, cx| this.flip_judge(cx)));
        let chosen = match &self.providers.judge {
            Some((provider, model)) => self.providers.chosen_running(provider, model.as_deref()),
            None => Chosen::First,
        };
        let this = cx.entity().downgrade();
        // Marked recommended: the model Flottille picks for the judge.
        let smallest = Providers::smallest();
        let providers = self.providers.clone();
        // A judge runs on its model's own effort: the daemon keeps none for it.
        let models = self
            .providers
            .model_entries_giving(&smallest)
            .into_iter()
            .map(|entry| entry.efforts(Vec::<String>::new()));
        let model = ModelChoice::new("judge", "Model", chosen)
            .models(models)
            .picked(self.providers.judge_picked)
            .cost(JUDGE_QUOTA)
            .disabled(self.judge_pending)
            .on_choose(move |chosen, _, _, cx| {
                if let Chosen::Model(provider, model) = chosen {
                    let model = model
                        .map(|m| m.to_string())
                        .or_else(|| providers.recommended(&provider, &smallest));
                    let judge = (provider.to_string(), model);
                    this.update(cx, |this, cx| this.choose_judge(Some(judge), cx))
                        .ok();
                }
            });
        let card = div()
            .id("judge")
            .p(space::LG)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(switch)
            .child(div().pt(space::SM).child(crate::extensions::usage_line(
                "judge-usage",
                JUDGE_USAGE,
                cx,
            )))
            .child(presence(
                "judge-model-block",
                on,
                div().pt(space::MD).child(model),
            ))
            .test_support();
        // Its models are the providers': the card crossfades in from a skeleton once they are known.
        Section::new("judge", "Auto judge").child(awaiting(
            "judge-waiting",
            !self.providers.known,
            Skeleton::new("judge-skeleton").h(px(96.)),
            card,
        ))
    }

    /// "New sessions": the provider, model and effort a session starts on, from the same model
    /// choice as everywhere. Left alone, each is what the provider proposes, selected and marked.
    fn new_sessions_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let shown = self.new_session_shown();
        let chosen = match &shown {
            Some((provider, model, _)) => self.providers.chosen(provider, model.as_deref()),
            None => Chosen::First,
        };
        let this = cx.entity().downgrade();
        let choice = ModelChoice::new("new-sessions-on", "Start on", chosen)
            .follows()
            .models(self.providers.model_entries())
            .effort(
                shown
                    .as_ref()
                    .and_then(|(_, _, effort)| effort.clone().map(Into::into)),
            )
            .disabled(self.new_session_pending)
            .on_choose(move |chosen, effort, _, cx| {
                let chosen = match chosen {
                    Chosen::Model(provider, model) => Some((
                        provider.to_string(),
                        model.map(|m| m.to_string()),
                        effort.map(|e| e.to_string()),
                    )),
                    _ => None,
                };
                this.update(cx, |this, cx| this.choose_new_session(chosen, cx))
                    .ok();
            });
        let notice = presence_of(
            "new-sessions-fallback-line",
            self.new_session_fallback().map(SharedString::from),
            move |said| {
                div()
                    .id("new-sessions-fallback")
                    .aria_label(said.clone())
                    .pt(space::SM)
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(said)
                    .test_support()
                    .into_any_element()
            },
        );
        // The mode: one visible label, the state said once by the choice; the note under it is
        // there only while Full is chosen, and unfolds and folds away with its gap.
        let shown_mode = self.new_session_mode_shown().to_owned();
        let mode = Segmented::new(
            "new-sessions-mode",
            "Mode",
            NEW_SESSION_MODES,
            NEW_SESSION_MODES
                .iter()
                .position(|(key, _)| *key == shown_mode)
                .unwrap_or(1),
        )
        .alarm(2)
        .disabled(!self.providers.known)
        .on_choose({
            let this = cx.entity().downgrade();
            move |index, _, cx| {
                let (mode, _) = NEW_SESSION_MODES[index];
                this.update(cx, |this, cx| this.choose_new_session_mode(mode, cx))
                    .ok();
            }
        });
        let mode_row = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(space::LG)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(div().text_size(px(13.)).child("Mode"))
                    .child(
                        div()
                            .id("new-sessions-mode-hint")
                            .aria_label(NEW_SESSION_MODE_HINT)
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(NEW_SESSION_MODE_HINT)
                            .test_support(),
                    ),
            )
            .child(mode);
        let full_note = presence_of(
            "new-sessions-full-note-line",
            (shown_mode == "full").then(|| SharedString::from(FULL_LENDS)),
            move |said| {
                div()
                    .id("new-sessions-full-note")
                    .aria_label(said.clone())
                    .text_size(px(12.5))
                    .text_color(p.wait)
                    .child(said)
                    .test_support()
                    .into_any_element()
            },
        )
        .space(space::MD);
        let card = div()
            .id("new-sessions")
            .p(space::LG)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(choice)
            .child(notice)
            .child(
                div()
                    .mt(space::MD)
                    .pt(space::MD)
                    .border_t_1()
                    .border_color(p.line)
                    .child(mode_row),
            )
            .child(full_note)
            .test_support();
        // Its models are the providers': the card crossfades in from a skeleton once they are known.
        Section::new("new-sessions", "New sessions")
            .sentence(NEW_SESSIONS_SENTENCE)
            .child(awaiting(
                "new-sessions-waiting",
                !self.providers.known,
                Skeleton::new("new-sessions-skeleton").h(px(96.)),
                card,
            ))
    }

    /// A provider's row: its switch carries its name, its version is a tag (its program's path in
    /// the tooltip) and one line says its state, once.
    fn provider_row(
        &self,
        provider: &Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let name = provider.name.clone();
        let on = self.switch_on(&name);
        let switch = Switch::new(
            ElementId::Name(format!("provider-switch-{name}").into()),
            on,
            SharedString::from(provider.title.clone()),
        )
        .disabled(self.pending.contains_key(&name))
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.flip(&name, cx)
        }));
        // By shape as well as colour: a filled dot when its agents can run, a ring otherwise.
        let state_color = if provider.offered() {
            p.ok
        } else if provider.enabled {
            p.wait
        } else {
            p.faint
        };
        let mark = div()
            .flex_none()
            .size(px(7.))
            .rounded_full()
            .when(provider.offered(), |d| d.bg(state_color))
            .when(!provider.offered(), |d| {
                d.border_1().border_color(state_color)
            });
        let said = SharedString::from(provider.said());
        let state = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .text_size(px(12.5))
            .text_color(state_color)
            .child(mark)
            // The daemon's sentence wraps rather than run past the row.
            .child(
                div()
                    .id(ElementId::Name(
                        format!("provider-state-{}", provider.name).into(),
                    ))
                    .aria_label(said.clone())
                    .flex_1()
                    .min_w_0()
                    .child(said)
                    .test_support(),
            );
        let version = provider.version.clone().map(|version| {
            let tag = div()
                .id(ElementId::Name(
                    format!("provider-version-{}", provider.name).into(),
                ))
                .aria_label(SharedString::from(version.clone()))
                .flex_none()
                .mono(11.)
                .text_color(p.faint)
                .px(space::SM)
                .py(space::XS)
                .rounded(px(5.))
                .border_1()
                .border_color(p.line)
                .whitespace_nowrap()
                .child(SharedString::from(version));
            match &provider.program_path {
                Some(path) => tipped(tag, SharedString::from(path.clone()), 20., window, cx)
                    .into_any_element(),
                None => tag.into_any_element(),
            }
        });
        // Its own sign-in, as the daemon of the machine read it: its state, Sign in or Sign out,
        // and the line to type when it signs in only in a terminal and the row does not say it.
        let says_line = !provider.needs_sign_in();
        let controls = div().child(self.sign_in.update(cx, |section, cx| {
            section.controls(&provider.name, says_line, cx)
        }));
        // What to do about it unfolds under the line: the command that signs it in, in a block to
        // copy, or the page that says how to install it.
        let sign_in = presence_of(
            format!("provider-sign-in-{}-block", provider.name),
            provider
                .needs_sign_in()
                .then(|| SharedString::from(provider.sign_in_command())),
            {
                let id = format!("provider-sign-in-{}", provider.name);
                move |command| crate::programs::command_row(id, command, p)
            },
        )
        .space(space::SM);
        // What may stop its agents that the daemon cannot tell for sure, once, under its line:
        // not where the sign-in command is already said.
        let warning = presence_of(
            format!("provider-warning-{}-block", provider.name),
            (!provider.needs_sign_in() && !provider.warnings.is_empty())
                .then(|| SharedString::from(provider.warnings.join(" · "))),
            {
                let id = format!("provider-warning-{}", provider.name);
                move |warnings| {
                    div()
                        .id(ElementId::Name(id.clone().into()))
                        .aria_label(warnings.clone())
                        .text_size(px(12.5))
                        .text_color(p.faint)
                        .child(warnings)
                        .test_support()
                        .into_any_element()
                }
            },
        )
        .space(space::SM);
        let install = presence_of(
            format!("provider-install-{}-block", provider.name),
            provider
                .needs_install()
                .then(|| provider.install_page().map(SharedString::from))
                .flatten(),
            {
                let id = format!("provider-install-{}", provider.name);
                let shell = self.shell.clone();
                let title = provider.title.clone();
                move |page| {
                    let opened = page.clone();
                    hoverable(
                        div()
                            .id(ElementId::Name(id.into()))
                            .aria_label(page)
                            .text_size(px(12.5))
                            .text_color(p.run)
                            .cursor_pointer()
                            .child(SharedString::from(format!("How to install {title}")))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                shell.update(cx, |shell, _| shell.open_link(&opened)).ok();
                            }),
                        move |link, t| link.bg(p.hover.opacity(t)),
                    )
                    .into_any_element()
                }
            },
        )
        .space(space::SM);
        div()
            .id(ElementId::Name(
                format!("provider-row-{}", provider.name).into(),
            ))
            .px(space::LG)
            .py(space::MD)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .child(div().flex_1().min_w_0().child(switch))
                    .children(version),
            )
            .child(state.mt(space::SM))
            .child(sign_in)
            .child(warning)
            .child(install)
            .child(controls.mt(space::SM))
            .into_any_element()
    }

    /// The Providers section: a card of rows, a skeleton in its place until the daemon says them.
    fn providers_section(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let known = self.providers.known;
        let rows: Vec<AnyElement> = self
            .providers
            .list
            .iter()
            .map(|provider| self.provider_row(provider, window, cx))
            .collect();
        let listed = !rows.is_empty();
        let list = div()
            .id("provider-list")
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .children(rows.into_iter().enumerate().map(|(at, row)| {
                div()
                    .when(at > 0, |d| d.border_t_1().border_color(p.line))
                    .child(row)
            }));
        let check = Button::secondary("providers-check", "Check again").on_click(cx.listener(
            |this, _, _, cx| {
                this.shell.perform(Command::ReadProviders, cx);
            },
        ));
        // What is in use, not how many rows there are: three rows with one switched on count one.
        let count = self.providers.offered().count();
        Section::new("providers", "Providers")
            .when(known, |section| section.count(count.to_string()))
            .sentence("The agent programs Flottille can start. Each signs in with its own login.")
            .action(check)
            .child(
                stack(space::MD)
                    .child(
                        presence_of(
                            "providers-error-line",
                            self.providers.error.clone().map(SharedString::from),
                            move |error| {
                                div()
                                    .id("providers-error")
                                    .text_size(px(12.5))
                                    .text_color(p.fail)
                                    .child(error)
                                    .test_support()
                                    .into_any_element()
                            },
                        )
                        .space(space::MD),
                    )
                    .child(spaced(
                        space::MD,
                        awaiting(
                            "providers-waiting",
                            !known,
                            Skeleton::new("providers-skeleton").h(px(96.)),
                            presence("provider-list-block", listed, list),
                        ),
                    )),
            )
            .into_any_element()
    }

    /// The rows of Features, by key and title: what the quick switcher offers a page of.
    pub fn features(
        &self,
        registry: &Entity<Registry>,
        cx: &gpui_kit::App,
    ) -> Vec<(String, String)> {
        self.feature_rows(Some(registry.clone()), cx)
            .into_iter()
            .map(|row| (row.0, row.2))
            .collect()
    }

    /// The row of a part of Flottille in Features: its key, icon, title, state tag and line.
    fn feature_rows(
        &self,
        registry: Option<Entity<Registry>>,
        cx: &gpui_kit::App,
    ) -> Vec<(String, Icon, String, String, String, u64)> {
        let mut rows = Vec::new();
        if let Some(registry) = registry {
            let registry = registry.read(cx);
            for listed in registry.rows() {
                let on = registry.switch_on(&listed.name, None);
                // Connections says how many of its servers are on.
                let count = match listed.name == connections::NAME {
                    true => self.connections.read(cx).on_count(),
                    false => None,
                };
                let tag = match (on, count) {
                    (false, _) => "Off".to_owned(),
                    (true, Some(count)) => format!("{count} on"),
                    (true, None) => "On".to_owned(),
                };
                rows.push((
                    listed.name.clone(),
                    crate::extensions::icon_named(&listed.icon),
                    listed.title.clone(),
                    tag,
                    listed.line.clone(),
                    listed.order,
                ));
            }
        }
        let on = self
            .compaction
            .as_ref()
            .is_some_and(|c| c.compactor.is_some() || c.by_agent);
        rows.push((
            COMPACTION.to_owned(),
            crate::extensions::icon_named("shrink"),
            "Compaction".to_owned(),
            if on { "On" } else { "Off" }.to_owned(),
            COMPACTION_LINE.to_owned(),
            COMPACTION_ORDER,
        ));
        rows.sort_by_key(|row| row.5);
        rows
    }

    /// The Features tab: a list of rows that opens the page of one in its place.
    fn features_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let registry = self.registry(cx);
        let rows = self.feature_rows(registry.clone(), cx);
        let known = self.compaction.is_some()
            && registry
                .as_ref()
                .is_none_or(|registry| registry.read(cx).is_known());
        let open = self
            .feature
            .clone()
            .filter(|key| rows.iter().any(|row| &row.0 == key));
        let this = cx.entity().downgrade();
        let mut subpage = Subpage::new("features", "Features")
            .open(open.as_deref())
            .on_open(move |key, _, cx| {
                this.update(cx, |this, cx| {
                    this.set_feature(key.map(|key| key.to_string()), cx)
                })
                .ok();
            });
        for (key, icon, title, tag, line, _) in rows {
            subpage = subpage.entry(key, icon, title, tag, line);
        }
        if let Some(key) = open {
            subpage = subpage.page(self.part_view(Part::Feature(key), cx));
        }
        awaiting(
            "features-waiting",
            !known,
            Skeleton::new("features-skeleton").h(px(120.)),
            subpage,
        )
        .into_any_element()
    }

    /// The view of a part, made once; it observes the page and the registry.
    fn part_view(&mut self, part: Part, cx: &mut Context<Self>) -> Entity<PagePart> {
        if let Some(view) = self.parts.get(&part) {
            return view.clone();
        }
        let page = cx.entity();
        let registry = self.registry(cx);
        let view = cx.new({
            let part = part.clone();
            let weak = page.downgrade();
            move |cx| {
                cx.observe(&page, |_, _, cx| cx.notify()).detach();
                if let Some(registry) = &registry {
                    cx.observe(registry, |_, _, cx| cx.notify()).detach();
                }
                PagePart { page: weak, part }
            }
        });
        self.parts.insert(part, view.clone());
        view
    }

    /// Draws one part of the page, for the view that holds it.
    fn draw_part(
        &mut self,
        part: &Part,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match part {
            Part::CompactionMore => self.compaction_more(cx),
            Part::Feature(key) if key == COMPACTION => self.compaction_page(window, cx),
            Part::Feature(key) if key == connections::NAME => {
                let registry = self.registry(cx);
                let page = registry
                    .as_ref()
                    .and_then(|registry| crate::extensions::feature_page(registry, key, cx));
                // Its servers, under its switch while it is on.
                let on = registry.is_some_and(|registry| {
                    let registry = registry.read(cx);
                    registry.switch_on(key, None)
                        && registry.listed(key).is_some_and(|listed| listed.enabled)
                });
                stack(space::XXL)
                    .children(page.map(|page| spaced(space::XXL, page)))
                    .child(
                        presence(
                            "connection-servers-block",
                            on,
                            enter("connection-servers-enter", 1, self.connections.clone()),
                        )
                        .space(space::XXL),
                    )
                    .into_any_element()
            }
            Part::Feature(key) => self
                .registry(cx)
                .and_then(|registry| crate::extensions::feature_page(&registry, key, cx))
                .unwrap_or_else(|| div().into_any_element()),
        }
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pad = if self.narrow { space::LG } else { space::XXL };
        // Only the tab shown is built: what a tab holds enters again as it is shown.
        let tab = self.tab.min(SECTIONS.len() - 1);
        // All machines is shown on this machine's page: it is what this machine's daemon keeps.
        let all = self.of.is_none() && of_all(cx) && self.has_hub(cx);
        // The tabs that hold a setting every machine may share; the others have none to show.
        let tab = match SECTIONS[tab].0 {
            "vault" | "machines" if all => SECTIONS.len() - 1,
            _ => tab,
        };
        let header = PageHeader::new("settings-header", "Settings");
        if SECTIONS[tab].0 == "general" {
            self.check.update(cx, |check, cx| check.ensure(cx));
        }
        let this_machine = self
            .of
            .clone()
            .or_else(|| {
                self.machines
                    .read(cx)
                    .machines()
                    .and_then(|read| read.this.clone())
            })
            .unwrap_or_else(|| "this machine".to_owned());
        let hub = self.has_hub(cx);
        let place = Place {
            on_all: all,
            machine: this_machine.clone(),
            hub,
        };
        self.about.update(cx, |about, cx| {
            about.place(all, this_machine, hub, window, cx)
        });
        for editor in [&self.rules, &self.judge_guidance, &self.compaction_guidance] {
            editor.update(cx, |editor, cx| editor.place(place.clone(), window, cx));
        }
        let registry = self.registry(cx);
        if let Some(registry) = &registry {
            registry.update(cx, |registry, cx| registry.place(place.clone(), window, cx));
        }
        self.place = place.clone();
        let blocks: Vec<(&str, AnyElement)> = match SECTIONS[tab].0 {
            // Only what every machine may share.
            "agents" if all => vec![
                ("rules", self.rules.clone().into_any_element()),
                (
                    "tool-results",
                    self.tool_results_section(cx).into_any_element(),
                ),
            ],
            "approvals" if all => {
                vec![("guidance", self.judge_guidance.clone().into_any_element())]
            }
            "features" if all => registry
                .as_ref()
                .map(|registry| crate::extensions::shared_blocks(registry, cx))
                .unwrap_or_default()
                .into_iter()
                .chain([(
                    "compaction-guidance",
                    self.compaction_guidance.clone().into_any_element(),
                )])
                .collect(),
            _ if all => vec![
                ("about", self.about.clone().into_any_element()),
                ("appearance", self.appearance_section(cx).into_any_element()),
            ],
            "agents" => vec![
                ("providers", self.providers_section(window, cx)),
                ("forges", self.sign_in.clone().into_any_element()),
                (
                    "new-sessions",
                    self.new_sessions_section(cx).into_any_element(),
                ),
                ("rules", self.rules.clone().into_any_element()),
                (
                    "tool-results",
                    self.tool_results_section(cx).into_any_element(),
                ),
                ("sessions", self.sessions_section(cx).into_any_element()),
            ],
            "approvals" => vec![
                ("judge", self.judge_section(cx).into_any_element()),
                ("guidance", self.judge_guidance.clone().into_any_element()),
            ],
            "features" => vec![("features", self.features_tab(cx))],
            "vault" => vec![("vault", self.vault.clone().into_any_element())],
            // Remote (#814, #1144): whether it is on, the hub, the devices.
            "machines" => vec![("machines", self.machines.clone().into_any_element())],
            _ => vec![
                ("about", self.about.clone().into_any_element()),
                ("appearance", self.appearance_section(cx).into_any_element()),
                ("workspaces", self.workspaces.clone().into_any_element()),
                ("travelled", self.travelled.clone().into_any_element()),
                ("developer", self.developer_section(cx).into_any_element()),
                ("check", self.check.clone().into_any_element()),
                ("version", self.version.clone().into_any_element()),
            ],
        };
        let vault = self.vault.read(cx).entries().map(<[_]>::len);
        let this = cx.entity().downgrade();
        // All machines has the tabs that hold a section every machine may share: General.
        let shown_tabs: Vec<(&str, &str)> = SECTIONS
            .iter()
            .filter(|(key, _)| !all || !matches!(*key, "vault" | "machines"))
            .copied()
            .collect();
        let at = shown_tabs
            .iter()
            .position(|(key, _)| *key == SECTIONS[tab].0)
            .unwrap_or(0);
        let tabs = shown_tabs
            .iter()
            .fold(
                Tabs::new("settings-tabs", "Sections", at),
                |tabs, (key, label)| match *key {
                    "vault" if let Some(entries) = vault => {
                        tabs.counted(*key, *label, entries.to_string())
                    }
                    _ => tabs.tab(*key, *label),
                },
            )
            .on_choose(move |at, _, cx| {
                let Some(key) = shown_tabs.get(at).map(|(key, _)| *key) else {
                    return;
                };
                let entry = SECTIONS
                    .iter()
                    .position(|(section, _)| *section == key)
                    .unwrap_or(0);
                this.update(cx, |this, cx| this.choose(entry, cx)).ok();
            });
        // The machine's picker sits on the title's line, at the header's right: the title is its
        // label. The kit's header has no place right after the title; beside it, the picker
        // stays level with the title whether or not the path line is above it.
        let header = div()
            .id("settings-header-line")
            .flex()
            .items_end()
            .child(div().flex_1().min_w_0().child(header))
            .children(self.machine_chooser(window, cx))
            .test_support();
        // Each machine's parts have their own, and All machines its own: chosen, its page enters.
        let key = |part: &str| match &self.of {
            None if all => ElementId::Name(format!("enter-settings-all-{part}").into()),
            None => ElementId::Name(format!("enter-settings-{part}").into()),
            Some(machine) => ElementId::Name(format!("enter-settings-{machine}-{part}").into()),
        };
        let hub = !all && self.has_hub(cx);
        let column = |element: AnyElement| {
            div()
                .w_full()
                .max_w(space::COLUMN)
                .flex_none()
                .child(element)
        };
        // The header and the tabs stay in place; the section under them scrolls.
        let top = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .px(pad)
            .pt(space::XXL)
            .child(column(
                enter(key("header"), 0, div().pb(space::LG).child(header)).into_any_element(),
            ))
            .child(column(enter(key("tabs"), 1, tabs).into_any_element()));
        // Each section enters as it is shown (its state is dropped while it is not drawn, so it
        // plays again), after the tabs.
        let page = div()
            .id("settings-page")
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px(pad)
            .pt(space::XL)
            .pb(space::XXXL)
            // The sentence of another machine's page is the first line of what scrolls: it goes
            // with the sections, so that no text is ever drawn over it or under it (#1293).
            .child(column(self.own_line(cx).into_any_element()))
            .children(blocks.into_iter().enumerate().map(|(n, (part, section))| {
                let section = match MACHINE_ONLY.contains(&part) {
                    true => machine_only(part, hub, section, cx),
                    false => section,
                };
                column(enter(key(part), 2 + n, section).into_any_element())
                    .when(n > 0, |d| d.mt(space::XXL))
            }));
        // Held after its blocks: what it adds comes after them, and a block's place stays its own.
        let page = crate::machines::hold(page, window, cx).test_support();
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.text)
            .child(top)
            .child(page)
    }
}

/// `section`, a block of the machine alone, under one line that says so while the machine has a
/// hub: the line unfolds and folds away with it.
fn machine_only(part: &str, hub: bool, section: AnyElement, cx: &gpui_kit::App) -> AnyElement {
    let p = palette(cx);
    // Its gap to the section is inside it, so that it folds away with it.
    let line = div().pb(space::SM).child(
        div()
            .id(ElementId::Name(format!("settings-only-{part}").into()))
            .aria_label(MACHINE_ONLY_LINE)
            .text_size(px(12.5))
            .text_color(p.mute)
            .child(MACHINE_ONLY_LINE)
            .test_support(),
    );
    div()
        .flex()
        .flex_col()
        .child(presence(
            ElementId::Name(format!("settings-only-{part}-block").into()),
            hub,
            line,
        ))
        .child(section)
        .into_any_element()
}

impl SettingsPage {
    /// Whether this machine has a hub, the member of a list with another machine: the picker
    /// offers All machines, and a machine's own blocks say so.
    fn has_hub(&self, cx: &gpui_kit::App) -> bool {
        self.machines
            .read(cx)
            .machines()
            .is_some_and(|read| crate::machines::choices(read).len() >= 2)
    }

    /// Whose settings these are, on the title's line: this machine, then the other machines
    /// that run agents, each with its state; choosing one shows it (`Shell::choose_machine`), as
    /// opening one of its sessions does. None while this machine is in no member list or knows no
    /// other runner.
    fn machine_chooser(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (choices, present) = crate::machines::kept(
            "settings-machine-kept",
            self.machines
                .read(cx)
                .machines()
                .map(crate::machines::choices)
                .unwrap_or_default(),
            window,
            cx,
        );
        let shown = crate::machines::shown_machine(cx);
        let chosen = choices
            .iter()
            .position(|choice| choice.name == shown)
            .unwrap_or(0);
        let (page, shell) = (cx.entity().downgrade(), self.shell.clone());
        let toggle = page.clone();
        let (all_page, all_shell) = (page.clone(), shell.clone());
        let all = crate::machines::AllMachines {
            chosen: of_all(cx),
            on_choose: Box::new(move |window, cx| {
                all_page
                    .update(cx, |page, cx| {
                        page.machine_menu = false;
                        cx.notify();
                    })
                    .ok();
                // This machine is brought at once (`Shell::reach_then`), which chooses no All
                // machines: chosen after it.
                all_shell
                    .update(cx, |shell, cx| shell.choose_machine(None, window, cx))
                    .ok();
                cx.set_global(SettingsOfAll(true));
            }),
        };
        let picker = (!choices.is_empty()).then(|| {
            crate::machines::picker_of(
                "settings-machine",
                "settings-machine",
                &choices,
                chosen,
                Some(all),
                self.machine_menu,
                move |_, cx| {
                    toggle
                        .update(cx, |page, cx| {
                            page.machine_menu = !page.machine_menu;
                            cx.notify();
                        })
                        .ok();
                },
                move |name, window, cx| {
                    cx.set_global(SettingsOfAll(false));
                    page.update(cx, |page, cx| {
                        page.machine_menu = false;
                        cx.notify();
                    })
                    .ok();
                    shell
                        .update(cx, |shell, cx| shell.choose_machine(name, window, cx))
                        .ok();
                },
            )
        });
        // The title is its visible label; a reader hears whose settings these are. Its gap is
        // inside it, so that it narrows away with it.
        let picker = div()
            .id("settings-machine-field")
            .aria_label("Settings of")
            .flex_none()
            .pl(space::LG)
            .children(picker)
            .test_support();
        // It narrows and fades away on the title's line once there is no other machine to choose,
        // and widens in when one comes: built on every frame, also while there is none.
        crate::machines::narrowing(
            "settings-machine-fold",
            present,
            picker.into_any_element(),
            window,
            cx,
        )
    }

    /// One line at the top of what scrolls, under the tabs, while another machine is shown,
    /// whichever tab: the appearance and the version are this window's, and stay this machine's.
    /// It scrolls with the sections, never a fixed line that they could pass under.
    fn own_line(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let this = self
            .machines
            .read(cx)
            .machines()
            .and_then(|read| read.this.clone())
            .unwrap_or_else(|| "this machine".to_owned());
        let said = crate::machines::shown_machine(cx).map(|_| {
            SharedString::from(format!(
                "This window (appearance, version) stays this machine's, {this}, whichever \
                 machine is chosen above."
            ))
        });
        // Its gap to the first section is inside it, so that it folds away with it.
        presence_of("settings-own-block", said, move |said| {
            div()
                .pb(space::XL)
                .child(
                    div()
                        .id("settings-own")
                        .aria_label(said.clone())
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(said)
                        .test_support(),
                )
                .into_any_element()
        })
    }
}

/// A count of characters with its thousands set apart: `8,000`.
pub(crate) fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        COMPACTION_HINT, COMPACTION_LINE, COMPACTION_USAGE, COMPACTOR_COST, TOOL_RESULTS_SENTENCE,
        UNCHANGED_HINT, grouped,
    };

    #[test]
    fn the_sentence_of_the_tool_results_section_has_no_run_of_spaces() {
        assert!(!UNCHANGED_HINT.contains("  "), "{UNCHANGED_HINT}");
        assert!(
            !TOOL_RESULTS_SENTENCE.contains("  "),
            "{TOOL_RESULTS_SENTENCE}"
        );
    }

    #[test]
    fn the_compaction_never_says_compactor() {
        for text in [
            COMPACTION_LINE,
            COMPACTION_USAGE,
            COMPACTION_HINT,
            COMPACTOR_COST,
        ] {
            assert!(!text.contains("compactor"), "{text}");
        }
    }

    #[test]
    fn a_count_sets_its_thousands_apart() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(9_400), "9,400");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
