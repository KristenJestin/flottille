//! The settings page: every provider Flottille knows, enabled or not with a switch, how it signs
//! in, and whether its agents can run here, as the daemon says it (`provider.list`). The window
//! never writes `providers.toml` itself: a switch asks the daemon (`provider.enable`,
//! `provider.disable`), and the page follows what the daemon lists next. The Auto judge is
//! chosen here too (`provider.set_judge`), and its guidance edited (`judge.set`).

use std::collections::HashMap;

use flottille_ui::{Align, Card, Dropdown, MenuItem, Switch, Type, enter, palette};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div,
    px,
};

use crate::actions::Command;
use crate::providers::{Provider, Providers};
use crate::rules::RulesEditor;
use crate::shell::Shell;

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

    /// The daemon's answer to a switch: on a refusal the switch goes back, and the sentence shows.
    pub fn answered(&mut self, name: &str, refused: Option<String>, cx: &mut Context<Self>) {
        match refused {
            Some(why) => {
                self.pending.remove(name);
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

    /// The daemon's answer to a judge chosen: its refusal shows under the pickers.
    pub fn judge_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.judge_pending = false;
        self.judge_refused = refused;
        cx.notify();
    }

    fn judge_handler(
        cx: &Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            this.update(cx, |this, cx| f(this, cx)).ok();
        }
    }

    fn judge_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (provider, model) = self.judge_shown();
        let mut pickers = div().flex().items_center().gap(px(4.));
        let menus = [
            Some((JudgeMenu::Provider, "judge-provider", provider)),
            model.map(|m| (JudgeMenu::Model, "judge-model", m)),
        ];
        for (menu, id, label) in menus.into_iter().flatten() {
            let items: Vec<MenuItem> = self
                .judge_entries(menu)
                .into_iter()
                .map(|(id, label, chosen, judge)| {
                    MenuItem::new(SharedString::from(id), label)
                        .selected(chosen)
                        .on_click(Self::judge_handler(cx, move |this, cx| {
                            this.choose_judge(judge.clone(), cx)
                        }))
                })
                .collect();
            let picker = Dropdown::new(id, label)
                .align(Align::Start)
                .width(px(260.))
                .open(self.judge_menu == Some(menu))
                .on_toggle(Self::judge_handler(cx, move |this, cx| {
                    this.toggle_judge_menu(menu, cx)
                }))
                .items(items);
            pickers = pickers.child(match menu {
                JudgeMenu::Provider => picker.strong(),
                JudgeMenu::Model => picker.mono(),
            });
        }
        let mut body = div()
            .id("judge")
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(pickers)
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
                    .text_color(p.faint)
                    .child(JUDGE_QUOTA)
                    .test_support(),
            )
            .test_support();
        if let Some(why) = &self.judge_refused {
            body = body.child(
                div()
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(SharedString::from(why.clone())),
            );
        }
        Card::new().title(div().child("Auto judge")).child(body)
    }

    fn card(&self, provider: &Provider, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let name = provider.name.clone();
        let on = self.switch_on(&name);
        let switch = Switch::new(
            ElementId::Name(format!("provider-switch-{name}").into()),
            on,
            format!("Enable {}", provider.title),
        )
        .disabled(self.pending.contains_key(&name))
        .on_click(cx.listener(move |this, _, _, cx| this.flip(&name, cx)));
        let title = div()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(SharedString::from(provider.title.clone()))
            .child(
                div()
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(provider.name.clone())),
            )
            .child(div().flex_1())
            .child(div().mono(11.).text_color(p.faint).child(if on {
                "enabled"
            } else {
                "disabled"
            }))
            .child(switch);
        let line = |text: String, color| {
            div()
                .text_size(px(12.5))
                .text_color(color)
                .child(SharedString::from(text))
        };
        // By shape as well as colour: a filled dot when its agents can run, a ring otherwise.
        let state_color = if provider.offered() {
            p.ok
        } else if provider.enabled {
            p.fail
        } else {
            p.faint
        };
        let mark = div()
            .flex_none()
            .mt(px(6.))
            .size(px(8.))
            .rounded_full()
            .when(provider.offered(), |d| d.bg(state_color))
            .when(!provider.offered(), |d| {
                d.border_1().border_color(state_color)
            });
        let mut body = div()
            .id(ElementId::Name(
                format!("provider-{}", provider.name).into(),
            ))
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .text_color(state_color)
                    .child(mark)
                    // The daemon's sentence wraps rather than run past the card.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(provider.state())),
                    ),
            )
            .child(line(provider.sign_in(), p.mute))
            .child(
                div()
                    .mono(11.5)
                    .text_color(p.faint)
                    .child(SharedString::from(provider.program_line())),
            )
            .children(
                provider
                    .warnings
                    .iter()
                    .map(|warning| line(format!("Warning: {warning}"), p.mute)),
            )
            .test_support();
        if let Some(why) = self.refused.get(&provider.name) {
            body = body.child(line(why.clone(), p.fail));
        }
        Card::new().title(title).child(body)
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pad = if self.narrow { 16. } else { 28. };
        let key = |part: &str| ElementId::Name(format!("enter-settings-{part}").into());
        let header = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mb(px(24.))
            .child(
                div()
                    .text_size(px(24.))
                    .line_height(px(29.))
                    .semibold()
                    .child("Settings"),
            )
            .child(
                div()
                    .text_color(p.mute)
                    .child("The providers your agents run on, as this machine has them."),
            );
        let label = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mb(px(6.))
            .text_size(px(13.))
            .semibold()
            .child("Providers")
            .child(
                div()
                    .mono(11.)
                    .font_weight(gpui_kit::FontWeight::NORMAL)
                    .text_color(p.faint)
                    .child(self.providers.list.len().to_string()),
            );
        let note = div()
            .mb(px(14.))
            .text_size(px(12.5))
            .text_color(p.faint)
            .child(
                "Disabling a provider stops no agent: those running on it go on, and fail at \
                 their next start, saying why. Flottille never shows a key: it says whether one \
                 is set.",
            );
        let judge = enter(
            key("judge"),
            3,
            div().mb(px(24.)).child(self.judge_card(cx)),
        );
        let cards: Vec<_> = self
            .providers
            .list
            .clone()
            .iter()
            .enumerate()
            .map(|(i, provider)| {
                enter(
                    key(&provider.name),
                    4 + i,
                    div().mb(px(12.)).child(self.card(provider, cx)),
                )
            })
            .collect();
        let count = cards.len();
        div()
            .id("settings-page")
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
                    .when(self.providers.known, |d| d.child(judge))
                    .child(enter(key("label"), 1, label))
                    .child(enter(key("note"), 2, note))
                    .when_some(self.providers.error.clone(), |d, error| {
                        d.child(
                            div()
                                .id("providers-error")
                                .mb(px(12.))
                                .text_color(p.fail)
                                .child(SharedString::from(error)),
                        )
                    })
                    .when(!self.providers.known, |d| {
                        d.child(div().text_color(p.faint).child("Asking the daemon…"))
                    })
                    .children(cards)
                    // The two long texts last, so that they push no provider out of sight.
                    .child(enter(
                        key("judge-guidance"),
                        3 + count,
                        div().mt(px(24.)).child(self.judge_guidance.clone()),
                    ))
                    .child(enter(
                        key("rules"),
                        4 + count,
                        div().mt(px(24.)).child(self.rules.clone()),
                    )),
            )
    }
}
