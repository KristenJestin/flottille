//! The settings page, in five tabs by subject (`docs/reference/gpui.md`, rule 13): Agents (the
//! providers, the rules for every project, the long tool results, the sessions), Approvals (the
//! Auto judge and what it is told), Features (a list of rows, each opening its page: the compaction
//! here, the others drawn from their declaration, `crate::extensions`), Vault and General (the
//! appearance, the workspaces folder, the developer mode). Every provider Flottille knows is
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
    Button, Chosen, Field, Fold, Icon, ModelChoice, PageHeader, Section, Segmented, Select,
    SelectOption, Skeleton, Subpage, Switch, Tabs, Toast, Type as _, awaiting, enter, hoverable,
    icon, palette, presence, presence_of, space, tipped, toast, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, AppContext as _, Context, ElementId, Entity, InteractiveElement,
    IntoElement, ParentElement, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, Transformation, WeakEntity, Window,
    div, px, radians,
};
use serde_json::{Value, json};

use crate::actions::{Command, SessionsSetting};
use crate::extensions::Registry;
use crate::providers::{Provider, Providers};
use crate::rules::RulesEditor;
use crate::shell::Shell;
use crate::vault::VaultSection;
use crate::workspaces_folder::WorkspacesFolder;

/// The tabs of the page, in order: each one's key and label. A new part of Flottille adds none
/// (rule 13): it is a row of Features.
pub const SECTIONS: [(&str, &str); 5] = [
    ("agents", "Agents"),
    ("approvals", "Approvals"),
    ("features", "Features"),
    ("vault", "Vault"),
    ("general", "General"),
];

/// The blocks of the Agents tab, top to bottom: each one is a section of the page, entering on its
/// own.
const AGENTS_BLOCKS: [&str; 4] = ["providers", "rules", "tool-results", "sessions"];

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
}

impl Reveal {
    /// The index of its tab in [`SECTIONS`].
    fn tab(&self) -> usize {
        let key = match self {
            Reveal::Providers | Reveal::Rules => "agents",
            Reveal::Judge | Reveal::Guidance => "approvals",
            Reveal::Compaction | Reveal::Feature(_) => "features",
            Reveal::Vault => "vault",
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

pub struct SettingsPage {
    shell: WeakEntity<Shell>,
    providers: Providers,
    /// A switch flipped, until the daemon lists the provider so: what it was asked to become.
    pending: HashMap<String, bool>,
    /// The daemon's refusal of the last switch, by provider.
    refused: HashMap<String, String>,
    narrow: bool,
    /// The user's rules for every project, edited on this page.
    rules: Entity<RulesEditor>,
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
    /// The vault's entries, set and removed on this page.
    vault: Entity<VaultSection>,
    /// A judge chosen, until the daemon lists it.
    judge_pending: bool,
    /// The judge's switch flipped, until the daemon answers: what it was asked to become.
    judge_turning: Option<bool>,
    /// The daemon's refusal of the last judge chosen.
    judge_refused: Option<String>,
    /// The developer mode, once the daemon said it.
    developer: Option<Developer>,
    /// The developer mode's switch flipped, until the daemon answers: what it was asked to become.
    developer_pending: Option<bool>,
    /// The limit above which a tool result is cut, once the daemon said it (`Some(None)` is Off).
    tool_results: Option<Option<usize>>,
    /// A limit chosen, until the daemon answers: what it was asked to become.
    tool_results_pending: Option<Option<usize>>,
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
    /// Why the daemon could not read the appearance it keeps: the tab says so and stays usable,
    /// a choice writes it again.
    appearance_unread: Option<String>,
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
    pub fn new(
        shell: WeakEntity<Shell>,
        rules: Entity<RulesEditor>,
        judge_guidance: Entity<RulesEditor>,
        compaction_guidance: Entity<RulesEditor>,
        workspaces: Entity<WorkspacesFolder>,
        vault: Entity<VaultSection>,
    ) -> Self {
        Self {
            rules,
            judge_guidance,
            compaction_guidance,
            compaction: None,
            compaction_pending: false,
            workspaces,
            vault,
            shell,
            providers: Providers::default(),
            pending: HashMap::new(),
            refused: HashMap::new(),
            narrow: false,
            judge_pending: false,
            judge_turning: None,
            judge_refused: None,
            developer: None,
            developer_pending: None,
            tool_results: None,
            tool_results_pending: None,
            sessions: None,
            sessions_pending: None,
            resume: None,
            resume_pending: None,
            appearance: None,
            appearance_unread: None,
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
            Reveal::Vault => {}
        }
        cx.notify();
    }

    fn scroll_to(&self, blocks: &[&str], block: &str) {
        let at = blocks
            .iter()
            .position(|key| *key == block)
            .expect("a block of the tab");
        self.scroll.scroll_to_top_of_item(at);
    }

    pub fn rules(&self) -> &Entity<RulesEditor> {
        &self.rules
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
        self.shell
            .update(cx, |shell, _| {
                shell.perform(Command::CompactionSet { params })
            })
            .ok();
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
                self.providers.chosen(provider, model.as_deref())
            }
            (None, false) => Chosen::First,
        };
        let summary = ModelChoice::new("compaction-summary", "Summary written by", chosen)
            .choice(
                "agent",
                "The session's own agent",
                "The agent summarises its own conversation, in its session.",
            )
            .models(self.providers.model_entries())
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
                move |chosen, effort, _, cx| {
                    let params = match chosen {
                        Chosen::Model(provider, model) => json!({
                            "summary": "compactor",
                            "compactor": {"provider": provider, "model": model, "effort": effort},
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
            .child(
                presence(
                    "compaction-summary-block",
                    some,
                    div().pt(space::MD).child(summary),
                )
                .gap(space::MD),
            )
            .child(
                presence(
                    "compaction-settings-block",
                    some,
                    div()
                        .pt(space::MD)
                        .flex()
                        .flex_col()
                        .gap(space::MD)
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
                            .gap(space::MD),
                        )
                        .child(more),
                )
                .gap(space::MD),
            )
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
                .gap(space::XXL),
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
            .gap(space::MD)
            .pl(space::LG)
            .child(Field::new("compaction-maximum-field", "Or above", maximum))
            .child(Field::new(
                "compaction-kept-field",
                "Recent messages kept as they are",
                kept,
            ))
            .child(before_expiry)
            .child(
                presence(
                    "compaction-margin-block",
                    shown.before_expiry,
                    Field::new("compaction-margin-field", "How long before", margin),
                )
                .gap(space::MD),
            )
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
            )
            .into_any_element()
    }

    pub fn workspaces_folder(&self) -> &Entity<WorkspacesFolder> {
        &self.workspaces
    }

    pub fn vault(&self) -> &Entity<VaultSection> {
        &self.vault
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
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
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
            .update(cx, |shell, _| {
                shell.perform(Command::SetJudge { provider, model })
            })
            .ok();
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
        self.shell
            .update(cx, |shell, _| shell.perform(Command::PickJudge))
            .ok();
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
        self.shell
            .update(cx, |shell, _| shell.perform(Command::DeveloperSet { log }))
            .ok();
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

    fn developer_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                .gap(space::MD),
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
        self.tool_results_pending
            .or(self.tool_results)
            .unwrap_or(Some(8_000))
    }

    fn choose_tool_results(&mut self, limit: Option<usize>, cx: &mut Context<Self>) {
        if self.tool_results_pending.is_some() || self.tool_results.is_none() {
            return;
        }
        self.tool_results_pending = Some(limit);
        self.shell
            .update(cx, |shell, _| {
                shell.perform(Command::ToolResultsSet { limit })
            })
            .ok();
        cx.notify();
    }

    /// The daemon's answer to a read or a choice: a choice refused goes back, and a toast says
    /// why.
    pub fn tool_results_answered(
        &mut self,
        shown: Result<Option<usize>, String>,
        written: bool,
        cx: &mut Context<Self>,
    ) {
        if written {
            self.tool_results_pending = None;
        }
        match shown {
            Ok(limit) => self.tool_results = Some(limit),
            Err(why) if written => toast(
                Toast::failure("The limit of tool results was not kept").detail(why),
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
            .disabled(self.tool_results.is_none() || self.tool_results_pending.is_some())
            .on_choose(move |index, _, cx| {
                let limit = picked[index];
                this.update(cx, |this, cx| this.choose_tool_results(limit, cx))
                    .ok();
            });
        Section::new("tool-results", "Long tool results")
            .sentence(TOOL_RESULTS_SENTENCE)
            .child(Field::new("tool-results-field", "Cut above", select))
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
        self.shell
            .update(cx, |shell, _| shell.perform(Command::SessionsSet { days }))
            .ok();
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
            .update(cx, |shell, _| {
                shell.perform(Command::ResumeSet { resume: !resume })
            })
            .ok();
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
            .child(
                presence(
                    "sessions-days-block",
                    shown.is_some(),
                    div()
                        .pt(space::MD)
                        .child(Field::new("sessions-days-field", "After", select)),
                )
                .gap(space::MD),
            )
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
    pub fn choose_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if !self.appearance_known() || self.appearance.as_ref() == Some(&appearance) {
            return;
        }
        self.appearance = Some(appearance.clone());
        appearance::appear(appearance.clone(), cx);
        self.shell
            .update(cx, |shell, _| shell.ask_appearance(Some(appearance)))
            .ok();
        cx.notify();
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
            match shown.shade {
                Shade::Light => 0,
                Shade::Dark => 1,
            },
        )
        .disabled(!known)
        .on_choose(move |index, _, cx| {
            let shade = if index == 0 {
                Shade::Light
            } else {
                Shade::Dark
            };
            this.update(cx, |this, cx| {
                this.choose_appearance(
                    Appearance {
                        source: Source::Theme(crate::OWN_THEME.into()),
                        shade,
                    },
                    cx,
                )
            })
            .ok();
        });
        let card = div()
            .id("appearance")
            .p(space::LG)
            .flex()
            .flex_col()
            .gap(space::MD)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(row(
                "Colours",
                "Omarchy follows your desktop's theme as it changes.",
                theme.into_any_element(),
            ))
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
                .gap(space::MD),
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
                .gap(space::MD),
            )
            .child(
                presence(
                    "appearance-shade-block",
                    matches!(shown.source, Source::Theme(_)),
                    row("Shade", "", shade.into_any_element()),
                )
                .gap(space::MD),
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
            Some((provider, model)) => self.providers.chosen(provider, model.as_deref()),
            None => Chosen::First,
        };
        let this = cx.entity().downgrade();
        // A judge runs on its model's own effort: the daemon keeps none for it.
        let models = self
            .providers
            .model_entries()
            .into_iter()
            .map(|entry| entry.efforts(Vec::<String>::new()));
        let model = ModelChoice::new("judge", "Model", chosen)
            .models(models)
            .picked(self.providers.judge_picked)
            .cost(JUDGE_QUOTA)
            .disabled(self.judge_pending)
            .on_choose(move |chosen, _, _, cx| {
                if let Chosen::Model(provider, model) = chosen {
                    let judge = (provider.to_string(), model.map(|m| m.to_string()));
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
            .child(
                presence("judge-model-block", on, div().pt(space::MD).child(model)).gap(space::MD),
            )
            .test_support();
        // Its models are the providers': the card crossfades in from a skeleton once they are known.
        Section::new("judge", "Auto judge").child(awaiting(
            "judge-waiting",
            !self.providers.known,
            Skeleton::new("judge-skeleton").h(px(96.)),
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
        .gap(space::SM);
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
        .gap(space::SM);
        div()
            .id(ElementId::Name(
                format!("provider-row-{}", provider.name).into(),
            ))
            .px(space::LG)
            .py(space::MD)
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .child(div().flex_1().min_w_0().child(switch))
                    .children(version),
            )
            .child(state)
            .child(sign_in)
            .child(install)
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
                this.shell
                    .update(cx, |shell, _| shell.perform(Command::ReadProviders))
                    .ok();
            },
        ));
        // What is in use, not how many rows there are: three rows with one switched on count one.
        let count = self.providers.offered().count();
        Section::new("providers", "Providers")
            .when(known, |section| section.count(count.to_string()))
            .sentence("The agent programs Flottille can start. Each signs in with its own login.")
            .action(check)
            .child(presence_of(
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
            ))
            .child(awaiting(
                "providers-waiting",
                !known,
                Skeleton::new("providers-skeleton").h(px(96.)),
                presence("provider-list-block", listed, list),
            ))
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
                rows.push((
                    listed.name.clone(),
                    crate::extensions::icon_named(&listed.icon),
                    listed.title.clone(),
                    if on { "On" } else { "Off" }.to_owned(),
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
        let header = PageHeader::new("settings-header", "Settings");
        // Only the tab shown is built: what a tab holds enters again as it is shown.
        let tab = self.tab.min(SECTIONS.len() - 1);
        let blocks: Vec<(&str, AnyElement)> = match SECTIONS[tab].0 {
            "agents" => vec![
                ("providers", self.providers_section(window, cx)),
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
            _ => vec![
                ("appearance", self.appearance_section(cx).into_any_element()),
                ("workspaces", self.workspaces.clone().into_any_element()),
                ("developer", self.developer_section(cx).into_any_element()),
            ],
        };
        let vault = self.vault.read(cx).entries().map(<[_]>::len);
        let this = cx.entity().downgrade();
        let tabs = SECTIONS
            .iter()
            .fold(
                Tabs::new("settings-tabs", "Sections", tab),
                |tabs, (key, label)| match *key {
                    "vault" if let Some(entries) = vault => {
                        tabs.counted(*key, *label, entries.to_string())
                    }
                    _ => tabs.tab(*key, *label),
                },
            )
            .on_choose(move |at, _, cx| {
                this.update(cx, |this, cx| this.choose(at, cx)).ok();
            });
        let key = |part: &str| ElementId::Name(format!("enter-settings-{part}").into());
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
            .children(blocks.into_iter().enumerate().map(|(n, (part, section))| {
                column(enter(key(part), 2 + n, section).into_any_element())
                    .when(n > 0, |d| d.mt(space::XXL))
            }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.text)
            .child(top)
            .child(page)
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
        grouped,
    };

    #[test]
    fn the_sentence_of_the_tool_results_section_has_no_run_of_spaces() {
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
