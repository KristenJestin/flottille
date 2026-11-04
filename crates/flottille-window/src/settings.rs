//! The settings page: every provider Flottille knows, enabled or not with a switch, how it signs
//! in, and whether its agents can run here, as the daemon says it (`provider.list`). The window
//! never writes `providers.toml` itself: a switch asks the daemon (`provider.enable`,
//! `provider.disable`), and the page follows what the daemon lists next. The Auto judge is
//! chosen here too (`provider.set_judge`), and its guidance edited (`judge.set`). The window's
//! appearance is chosen (`appearance.set`): the Omarchy theme or Flottille's own, light or dark.
//! Last, the developer mode is switched (`developer.set`), its log's path beside the switch. The
//! page is tabs, one section shown at a time; the Judge tab holds two, the Auto judge and then
//! its guidance.

use std::collections::HashMap;

use flottille_ui::omarchy::{self, Appearance, Source};
use flottille_ui::theme::Shade;
use flottille_ui::{
    Field, Icon, ListCard, ListRow, PageHeader, Section, Segmented, Select, SelectOption, Switch,
    Tabs, Toast, enter, hoverable, icon, palette, presence, presence_of, space, toast, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Point,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    Transformation, WeakEntity, Window, div, px, radians,
};

use crate::actions::Command;
use crate::instructions::InstructionsEditor;
use crate::providers::{Provider, Providers};
use crate::rules::RulesEditor;
use crate::shell::Shell;
use crate::workspaces_folder::WorkspacesFolder;

/// The sections of the page, in order: each index entry's key and label.
pub const SECTIONS: [(&str, &str); 7] = [
    ("judge", "Judge"),
    ("providers", "Providers"),
    ("rules", "Rules"),
    ("instructions", "Instructions"),
    ("workspaces", "Workspaces"),
    ("appearance", "Appearance"),
    ("developer", "Developer mode"),
];

/// The blocks of the Judge tab, top to bottom: each one is a section of the page, entering on its
/// own.
const JUDGE_BLOCKS: [&str; 2] = ["judge", "guidance"];

/// Where Settings was opened at, from a warning or a notice that leads to a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// No judge is chosen (the mode's warning, the judge notice).
    Judge,
    /// No provider can start a session (the composer's "Open the settings").
    Providers,
    /// The Auto judge's guidance: the Judge tab, scrolled to the guidance under the judge.
    Guidance,
}

impl Reveal {
    /// The index of its section in [`SECTIONS`].
    fn entry(self) -> usize {
        let key = match self {
            Reveal::Judge | Reveal::Guidance => "judge",
            Reveal::Providers => "providers",
        };
        SECTIONS
            .iter()
            .position(|(k, _)| *k == key)
            .expect("a section of the page")
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

/// What the Tool results section says under its title.
const TOOL_RESULTS_SENTENCE: &str = "A tool result longer than this reaches the agent as its beginning and its end, \
     with the number of characters left out; the agent reads the rest by ranges, and only that \
     session can. It saves the agent's context. For every project and every agent.";

/// What the developer mode's switch logs, under its label.
pub const DEVELOPER_HINT: &str = "Each gate decision with its reasons, the judge's input and \
     answer, requests, deliveries and agent processes, one JSON object per line, every secret \
     masked.";

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
    /// The user's own instructions for every agent, edited on this page.
    instructions: Entity<InstructionsEditor>,
    /// Where the workspaces of every project go, chosen on this page.
    workspaces: Entity<WorkspacesFolder>,
    /// The judge's picker whose list is open.
    judge_menu: Option<JudgeMenu>,
    /// A judge chosen, until the daemon lists it.
    judge_pending: bool,
    /// The daemon's refusal of the last judge chosen.
    judge_refused: Option<String>,
    /// "How Auto works" is unfolded.
    how: bool,
    /// The developer mode, once the daemon said it.
    developer: Option<Developer>,
    /// The developer mode's switch flipped, until the daemon answers: what it was asked to become.
    developer_pending: Option<bool>,
    /// The limit above which a tool result is cut, once the daemon said it (`Some(None)` is Off).
    tool_results: Option<Option<usize>>,
    /// A limit chosen, until the daemon answers: what it was asked to become.
    tool_results_pending: Option<Option<usize>>,
    /// The window's appearance, once the daemon said it.
    appearance: Option<Appearance>,
    /// Why the daemon could not read the appearance it keeps: the tab says so and stays usable,
    /// a choice writes it again.
    appearance_unread: Option<String>,
    /// The tab chosen: the index of its section in [`SECTIONS`].
    tab: usize,
    /// The scroll of the section shown, to bring the guidance in view from a link.
    scroll: ScrollHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JudgeMenu {
    Provider,
    Model,
}

/// A judge as the daemon lists it: its provider and its model, the provider's default when none.
pub type Judge = (String, Option<String>);

/// Auto, in two lines, beside the judge's pickers.
pub const AUTO: [&str; 2] = [
    "In Auto, the rules pass what is known to be harmless, and a small model on your own quota \
     decides the rest: it allows a call, or asks you.",
    "Base calls always come to you, in Ask as in Auto: they never reach the judge.",
];

/// What the judge's picker calls no judge.
pub const NO_JUDGE: &str = "No judge (Auto asks like Ask)";

/// What the page says of the judge's cost, beside its pickers.
pub const JUDGE_QUOTA: &str =
    "Each judgement is a turn of the judge's model: it uses your quota on that provider.";

impl SettingsPage {
    pub fn new(
        shell: WeakEntity<Shell>,
        rules: Entity<RulesEditor>,
        judge_guidance: Entity<RulesEditor>,
        instructions: Entity<InstructionsEditor>,
        workspaces: Entity<WorkspacesFolder>,
    ) -> Self {
        Self {
            rules,
            judge_guidance,
            instructions,
            workspaces,
            shell,
            providers: Providers::default(),
            pending: HashMap::new(),
            refused: HashMap::new(),
            narrow: false,
            judge_menu: None,
            judge_pending: false,
            judge_refused: None,
            how: false,
            developer: None,
            developer_pending: None,
            tool_results: None,
            tool_results_pending: None,
            appearance: None,
            appearance_unread: None,
            tab: 0,
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

    /// Opened at a section: its tab is the one chosen.
    pub fn reveal(&mut self, at: Reveal, cx: &mut Context<Self>) {
        self.choose(at.entry(), cx);
        // The Judge tab is already shown when the link comes from where the page was left
        // scrolled: the link says where the page starts.
        match at {
            Reveal::Guidance => {
                let at = JUDGE_BLOCKS
                    .iter()
                    .position(|key| *key == "guidance")
                    .expect("a block of the Judge tab");
                self.scroll.scroll_to_top_of_item(at);
            }
            Reveal::Judge => self.scroll.set_offset(Point::default()),
            Reveal::Providers => {}
        }
        cx.notify();
    }

    pub fn rules(&self) -> &Entity<RulesEditor> {
        &self.rules
    }

    pub fn judge_guidance(&self) -> &Entity<RulesEditor> {
        &self.judge_guidance
    }

    pub fn instructions(&self) -> &Entity<InstructionsEditor> {
        &self.instructions
    }

    pub fn workspaces_folder(&self) -> &Entity<WorkspacesFolder> {
        &self.workspaces
    }

    pub fn providers(&self) -> &Providers {
        &self.providers
    }

    /// What the page says of a provider, line by line: its state, its sign-in, its program, its
    /// warnings and the daemon's refusal of its switch.
    pub fn said_of(&self, name: &str) -> Vec<String> {
        let Some(provider) = self.providers.get(name) else {
            return Vec::new();
        };
        let mut lines = vec![
            provider.state(),
            provider.sign_in(),
            provider.program_line(),
        ];
        lines.extend(provider.warnings.iter().cloned());
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

    /// What the judge's picker offers: `(id, label, chosen, the judge it chooses)`.
    pub fn judge_entries(&self, menu: JudgeMenu) -> Vec<(String, String, bool, Option<Judge>)> {
        let judge = self.providers.judge.as_ref();
        match menu {
            JudgeMenu::Provider => {
                let mut entries = vec![(
                    "judge-choice-none".to_owned(),
                    NO_JUDGE.to_owned(),
                    judge.is_none(),
                    None,
                )];
                for provider in self.providers.offered() {
                    entries.push((
                        format!("judge-choice-{}", provider.name),
                        provider.title.clone(),
                        judge.is_some_and(|(name, _)| name == &provider.name),
                        Some((provider.name.clone(), None)),
                    ));
                }
                entries
            }
            JudgeMenu::Model => {
                let Some((name, model)) = judge else {
                    return Vec::new();
                };
                let models = self
                    .providers
                    .get(name)
                    .and_then(|p| p.models.clone())
                    .unwrap_or_default();
                // The row that names none: the provider's default model by its name, once known.
                let first = self
                    .providers
                    .model_said(name, None)
                    .unwrap_or_else(|| "Model not known yet".to_owned());
                let default = self
                    .providers
                    .get(name)
                    .and_then(|p| p.default_model())
                    .map(|m| m.id.clone());
                let mut entries = vec![(
                    "judge-model-default".to_owned(),
                    first,
                    model.is_none() || model == &default,
                    Some((name.clone(), None)),
                )];
                // The default model is that first row, not a second one.
                for listed in models
                    .into_iter()
                    .filter(|m| m.id != crate::composer::DEFAULT && Some(&m.id) != default.as_ref())
                {
                    entries.push((
                        format!("judge-model-{}", listed.id),
                        listed.id.clone(),
                        model.as_deref() == Some(listed.id.as_str()),
                        Some((name.clone(), Some(listed.id))),
                    ));
                }
                entries
            }
        }
    }

    /// The judge's picker whose list is open.
    pub fn judge_menu(&self) -> Option<JudgeMenu> {
        self.judge_menu
    }

    pub fn toggle_judge_menu(&mut self, menu: JudgeMenu, cx: &mut Context<Self>) {
        self.judge_menu = if self.judge_menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    /// Asks the daemon for this judge, or for none.
    pub fn choose_judge(&mut self, judge: Option<Judge>, cx: &mut Context<Self>) {
        self.judge_menu = None;
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

    /// The daemon's answer to a judge chosen: a toast says it.
    pub fn judge_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.judge_pending = false;
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
        let file = self
            .developer
            .as_ref()
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
                                .font_family(flottille_ui::typography::MONO)
                                .text_color(p.faint)
                                .child(file),
                        )
                        .test_support()
                        .into_any_element()
                })
                .gap(space::MD),
            )
            .test_support();
        Section::new("developer", "Developer mode")
            .sentence(
                "A log for working on Flottille: why each call was allowed, held or refused. Off, \
                 nothing more is written.",
            )
            .child(card)
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
        Section::new("tool-results", "Tool results")
            .sentence(TOOL_RESULTS_SENTENCE)
            .child(Field::new(
                "tool-results-field",
                "Cut results longer than",
                select,
            ))
    }

    /// Whether the daemon answered about the window's appearance, with it or with why it cannot
    /// read it: until then it cannot be chosen.
    pub fn appearance_known(&self) -> bool {
        self.appearance.is_some() || self.appearance_unread.is_some()
    }

    /// Uses `appearance` at once, and asks the daemon to keep it for every window.
    pub fn choose_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if !self.appearance_known() || self.appearance == Some(appearance) {
            return;
        }
        self.appearance = Some(appearance);
        omarchy::appear(appearance, cx);
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
        let shown = omarchy::shown(cx);
        let kept = self.appearance.unwrap_or(shown);
        let refused = omarchy::omarchy_refused(cx);
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
            "Theme",
            [("omarchy", "Omarchy"), ("flottille", "Flottille")],
            match shown.source {
                Source::Omarchy => 0,
                Source::Flottille => 1,
            },
        )
        .disabled(!known)
        .when(refused.is_some(), |s| s.unavailable(0))
        .on_choose(move |index, _, cx| {
            let source = if index == 0 {
                Source::Omarchy
            } else {
                Source::Flottille
            };
            this.update(cx, |this, cx| {
                this.choose_appearance(Appearance { source, ..kept }, cx)
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
                        source: Source::Flottille,
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
                "Theme",
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
                    shown.source == Source::Flottille,
                    row(
                        "Shade",
                        "Flottille's own colours, light or dark.",
                        shade.into_any_element(),
                    ),
                )
                .gap(space::MD),
            )
            .test_support();
        Section::new("appearance", "Appearance")
            .sentence(
                "Where the window's colours come from: your desktop's Omarchy theme, or \
                 Flottille's own. A choice applies at once, in every window.",
            )
            .child(card)
    }

    /// Whether "How Auto works" is unfolded.
    pub fn is_how_open(&self) -> bool {
        self.how
    }

    pub fn toggle_how(&mut self, cx: &mut Context<Self>) {
        self.how = !self.how;
        cx.notify();
    }

    fn judge_section(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let select = |menu: JudgeMenu, id: &'static str, cx: &mut Context<Self>| {
            let entries = self.judge_entries(menu);
            let chosen = entries.iter().position(|e| e.2).unwrap_or(0);
            let judges: Vec<Option<Judge>> = entries.iter().map(|e| e.3.clone()).collect();
            let options = entries.into_iter().map(|(key, label, _, _)| {
                let key = key.trim_start_matches("judge-").to_owned();
                SelectOption::new(SharedString::from(key), SharedString::from(label))
            });
            let this = cx.entity().downgrade();
            Select::new(id, options, chosen)
                .disabled(self.judge_pending)
                .on_choose(move |index, _, cx| {
                    let judge = judges[index].clone();
                    this.update(cx, |this, cx| this.choose_judge(judge, cx))
                        .ok();
                })
        };
        let provider = select(JudgeMenu::Provider, "judge-provider", cx);
        let has_model = self.providers.judge.is_some();
        let model = select(JudgeMenu::Model, "judge-model", cx);
        let turned = turning("judge-how-chevron", self.how, window, cx);
        let how = hoverable(
            div()
                .id("judge-how")
                .aria_label("How Auto works")
                .flex()
                .items_center()
                .gap(space::XS)
                .cursor_pointer()
                .text_size(px(12.5))
                .child(icon(Icon::Chevron).size(px(12.)).with_transformation(
                    Transformation::rotate(radians(-std::f32::consts::FRAC_PI_2 * turned)),
                ))
                .child("How Auto works")
                .on_click(cx.listener(|this, _, _, cx| this.toggle_how(cx))),
            move |d, t| d.text_color(flottille_ui::color::mix(p.faint, p.text, t)),
        );
        let lines = div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .pl(space::LG)
            .children(AUTO.iter().enumerate().map(|(n, text)| {
                div()
                    .id(ElementId::Name(format!("judge-auto-{n}").into()))
                    .aria_label(*text)
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(*text)
                    .test_support()
            }))
            .child(
                div()
                    .id("judge-quota")
                    .aria_label(JUDGE_QUOTA)
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(JUDGE_QUOTA)
                    .test_support(),
            );
        let card = div()
            .id("judge")
            .p(space::LG)
            .flex()
            .flex_col()
            .gap(space::MD)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(
                div()
                    .flex()
                    .gap(space::MD)
                    .child(div().flex_1().min_w_0().child(Field::new(
                        "judge-provider-field",
                        "Provider",
                        provider,
                    )))
                    .child(div().flex_1().min_w_0().child(presence(
                        "judge-model-block",
                        has_model,
                        Field::new("judge-model-field", "Model", model),
                    ))),
            )
            .child(how)
            .child(presence("judge-how-block", self.how, lines).gap(space::MD))
            .test_support();
        Section::new("judge", "Auto judge")
            .sentence(
                "In Auto, a small model decides the calls the rules do not know to be harmless. \
                 It never denies: it allows, or asks you.",
            )
            .child(card)
    }

    fn provider_row(&self, provider: &Provider, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let name = provider.name.clone();
        let on = self.switch_on(&name);
        let switch = Switch::new(
            ElementId::Name(format!("provider-switch-{name}").into()),
            on,
            "Enabled",
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
        let line = |text: String, color| {
            div()
                .text_size(px(12.5))
                .text_color(color)
                .child(SharedString::from(text))
        };
        let below = div()
            .id(ElementId::Name(
                format!("provider-{}", provider.name).into(),
            ))
            .mt(px(2.))
            .flex()
            .flex_col()
            .gap(space::XS)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .text_size(px(12.5))
                    .text_color(state_color)
                    .child(mark)
                    // The daemon's sentence wraps rather than run past the row.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(provider.state())),
                    ),
            )
            .child(line(provider.sign_in(), p.faint))
            .children(
                provider
                    .warnings
                    .iter()
                    .map(|warning| line(format!("Warning: {warning}"), p.mute)),
            )
            .test_support();
        let detail = match &provider.version {
            Some(version) => format!("{} {version}", provider.program),
            None => provider.program_line(),
        };
        ListRow::new(
            ElementId::Name(format!("provider-row-{}", provider.name).into()),
            SharedString::from(provider.title.clone()),
        )
        .detail(SharedString::from(detail))
        .below(below)
        .trailing(switch)
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pad = if self.narrow { space::LG } else { space::XXL };
        let header = PageHeader::new("settings-header", "Settings");
        let rows: Vec<ListRow> = self
            .providers
            .list
            .clone()
            .iter()
            .map(|provider| self.provider_row(provider, cx))
            .collect();
        let count = rows.len();
        let providers = Section::new("providers", "Providers")
            .count(count.to_string())
            .sentence(
                "The agent programs installed on this machine and signed in. Disabling one stops \
                 no agent that runs; Flottille never shows a key, only whether one is set.",
            )
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
            .child(presence(
                "providers-asking",
                !self.providers.known,
                div()
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child("Asking the daemon…"),
            ))
            .child(presence(
                "provider-list-block",
                count > 0,
                ListCard::new("provider-list").rows(rows),
            ))
            .into_any_element();
        // Every section is built on each frame, as on a project's settings.
        // The Judge tab holds two: the Auto judge, then its guidance.
        let judge_tab: Vec<(&str, AnyElement)> = JUDGE_BLOCKS
            .iter()
            .map(|key| {
                let block = match *key {
                    "judge" if self.providers.known => {
                        self.judge_section(window, cx).into_any_element()
                    }
                    "judge" => div().into_any_element(),
                    _ => self.judge_guidance.clone().into_any_element(),
                };
                (*key, block)
            })
            .collect();
        let mut sections: Vec<Vec<(&str, AnyElement)>> = vec![
            judge_tab,
            vec![
                ("providers", providers),
                (
                    "tool-results",
                    self.tool_results_section(cx).into_any_element(),
                ),
            ],
            vec![("rules", self.rules.clone().into_any_element())],
            vec![("instructions", self.instructions.clone().into_any_element())],
            vec![("workspaces", self.workspaces.clone().into_any_element())],
            vec![("appearance", self.appearance_section(cx).into_any_element())],
            vec![("developer", self.developer_section(cx).into_any_element())],
        ];
        let chosen = self.tab.min(sections.len() - 1);
        let shown = sections.swap_remove(chosen);
        let this = cx.entity().downgrade();
        let tabs = SECTIONS
            .iter()
            .fold(
                Tabs::new("settings-tabs", "Sections", chosen),
                |tabs, (key, label)| match *key {
                    // The count shows once the daemon listed them, so it does not jump.
                    "providers" if self.providers.known => {
                        tabs.counted(*key, *label, count.to_string())
                    }
                    _ => tabs.tab(*key, *label),
                },
            )
            .on_choose(move |entry, _, cx| {
                this.update(cx, |this, cx| this.choose(entry, cx)).ok();
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
            .children(shown.into_iter().enumerate().map(|(n, (part, section))| {
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
    use super::{TOOL_RESULTS_SENTENCE, grouped};

    #[test]
    fn the_sentence_of_the_tool_results_section_has_no_run_of_spaces() {
        assert!(
            !TOOL_RESULTS_SENTENCE.contains("  "),
            "{TOOL_RESULTS_SENTENCE}"
        );
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
