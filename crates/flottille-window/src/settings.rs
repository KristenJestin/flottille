//! The settings page: every provider Flottille knows, enabled or not with a switch, how it signs
//! in, and whether its agents can run here, as the daemon says it (`provider.list`). The window
//! never writes `providers.toml` itself: a switch asks the daemon (`provider.enable`,
//! `provider.disable`), and the page follows what the daemon lists next. The Auto judge is
//! chosen here too (`provider.set_judge`), and its guidance edited (`judge.set`).

use std::collections::HashMap;

use flottille_ui::{
    Field, Icon, ListCard, ListRow, PageHeader, PageIndex, Section, Select, SelectOption, Switch,
    Toast, hoverable, icon, palette, presence, presence_of, space, toast, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, Transformation,
    WeakEntity, Window, div, px, radians,
};

use crate::actions::Command;
use crate::providers::{Provider, Providers};
use crate::rules::RulesEditor;
use crate::sections::Sections;
use crate::shell::Shell;

/// The sections of the page, in order: each index entry's key and label.
pub const SECTIONS: [(&str, &str); 4] = [
    ("judge", "Auto judge"),
    ("providers", "Providers"),
    ("guidance", "Judge guidance"),
    ("rules", "Rules"),
];

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
    /// The judge's picker whose list is open.
    judge_menu: Option<JudgeMenu>,
    /// A judge chosen, until the daemon lists it.
    judge_pending: bool,
    /// The daemon's refusal of the last judge chosen.
    judge_refused: Option<String>,
    /// "How Auto works" is unfolded.
    how: bool,
    sections: Sections,
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
    ) -> Self {
        Self {
            rules,
            judge_guidance,
            shell,
            providers: Providers::default(),
            pending: HashMap::new(),
            refused: HashMap::new(),
            narrow: false,
            judge_menu: None,
            judge_pending: false,
            judge_refused: None,
            how: false,
            sections: Sections::default(),
        }
    }

    pub fn rules(&self) -> &Entity<RulesEditor> {
        &self.rules
    }

    pub fn judge_guidance(&self) -> &Entity<RulesEditor> {
        &self.judge_guidance
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
                    Some(model.clone().unwrap_or_else(|| "default model".into())),
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
                let mut entries = vec![(
                    "judge-model-default".to_owned(),
                    "default model".to_owned(),
                    model.is_none(),
                    Some((name.clone(), None)),
                )];
                for listed in models
                    .into_iter()
                    .filter(|m| m.id != crate::composer::DEFAULT)
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
        let judge = self.judge_section(window, cx).into_any_element();
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
        let sections: Vec<(&'static str, AnyElement)> = vec![
            (
                "judge",
                if self.providers.known {
                    judge
                } else {
                    div().into_any_element()
                },
            ),
            ("providers", providers),
            ("guidance", self.judge_guidance.clone().into_any_element()),
            ("rules", self.rules.clone().into_any_element()),
        ];
        let this = cx.entity().downgrade();
        let index = move |id: ElementId, current: usize| {
            let this = this.clone();
            PageIndex::new(id, SECTIONS)
                .current(current)
                .on_pick(move |entry, _, cx| {
                    this.update(cx, |this, cx| {
                        this.sections.go_to(entry, cx);
                        cx.notify();
                    })
                    .ok();
                })
        };
        let page = self.sections.page(
            "settings-page",
            "settings",
            pad,
            header.into_any_element(),
            index,
            sections,
            window,
            cx,
        );
        div().size_full().text_color(p.text).child(page)
    }
}
