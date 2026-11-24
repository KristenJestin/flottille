//! What the session's root runs on, under its field: the one picker of provider, model and effort,
//! the start composer's own (`composer::agent_picker`), offering the session's provider only (a
//! provider change needs a new session of another CLI). A change applies between turns: chosen
//! while a turn runs it is held by the daemon and set before the next prompt: the picker carries a
//! dot, which says when it applies, until the agent took it (`agent.options`, `docs/agents.md`,
//! "The options"). Nothing under the bar, so the composer keeps its height.

use gpui_design_system::motion::HOVER;
use gpui_design_system::{Chosen, ModelPicker, fade, palette, space};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Command, Pick};
use crate::composer::{Pickers, agent_picker};
use crate::providers::{Provider, Providers};
use crate::session_link::Pending;
use crate::shell::Shell;

/// What the agent runs on, as the tree last said it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runs {
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub pending: Pending,
    /// A turn runs: a change waits for its end.
    pub working: bool,
}

/// What the picker's menu says of a change waiting.
pub const AFTER_TURN: &str = "Applies after this turn";
pub const WITH_NEXT: &str = "Applies with your next message";

/// The dot over the picker while a change waits.
const MARK: &str = "session-agent-pending";

pub struct SessionOptions {
    shell: WeakEntity<Shell>,
    /// How it asks its session's machine, given when its session's page is made (#1271).
    asks: Option<crate::actions::Actions>,
    /// The root agent: the only one whose options this changes.
    session: String,
    runs: Runs,
    providers: Providers,
    /// What was chosen and the tree has not said yet: the pickers already show it.
    asked: Pending,
    /// The daemon holds what was asked: the next tree it says has it, waiting, set or replaced
    /// by another change.
    held: bool,
}

impl SessionOptions {
    /// Asks through `asks`, its session's machine's actions, whichever machine is shown.
    pub fn asking(mut self, asks: crate::actions::Actions) -> Self {
        self.asks = Some(asks);
        self
    }

    pub fn new(shell: WeakEntity<Shell>, session: String) -> Self {
        Self {
            shell,
            asks: None,
            session,
            runs: Runs::default(),
            providers: Providers::default(),
            asked: Pending::default(),
            held: false,
        }
    }

    /// The provider and the model as a line, as the agents' rows say them.
    pub fn shown(&self) -> String {
        if self.runs.provider.is_empty() {
            return self.runs.model.clone().unwrap_or_default();
        }
        crate::session::runs_on(
            &self.providers,
            &self.runs.provider,
            self.runs.model.as_deref(),
        )
    }

    pub fn set_providers(&mut self, providers: &Providers, cx: &mut Context<Self>) {
        if self.providers != *providers {
            self.providers = providers.clone();
            cx.notify();
        }
    }

    /// What the tree says the root runs on. A change asked for is no longer only asked once the
    /// tree holds it, waiting or done; a refusal takes it back (`refused`).
    pub fn set_runs(&mut self, runs: Runs, cx: &mut Context<Self>) {
        let caught_up = |asked: &Option<String>, pending: &Option<String>, now: &Option<String>| {
            asked.is_some() && (pending == asked || now == asked)
        };
        if caught_up(&self.asked.model, &runs.pending.model, &runs.model) {
            self.asked.model = None;
        }
        if caught_up(&self.asked.effort, &runs.pending.effort, &runs.effort) {
            self.asked.effort = None;
        }
        if std::mem::take(&mut self.held) {
            self.asked = Pending::default();
        }
        if self.runs != runs {
            self.runs = runs;
            cx.notify();
        }
    }

    /// The pick the pickers show: what the agent runs on, less what is waiting or just asked.
    fn pick(&self) -> Pick {
        let waiting = |asked: &Option<String>, pending: &Option<String>, now: &Option<String>| {
            asked.clone().or(pending.clone()).or(now.clone())
        };
        Pick {
            provider: Some(self.runs.provider.clone()).filter(|p| !p.is_empty()),
            model: waiting(
                &self.asked.model,
                &self.runs.pending.model,
                &self.runs.model,
            ),
            effort: waiting(
                &self.asked.effort,
                &self.runs.pending.effort,
                &self.runs.effort,
            ),
        }
    }

    fn provider(&self) -> Option<&Provider> {
        self.providers.get(&self.runs.provider)
    }

    /// What the mark says, when a change waits.
    pub fn waiting(&self) -> Option<&'static str> {
        let waits = !self.asked.is_empty() || !self.runs.pending.is_empty();
        waits.then_some(if self.runs.working {
            AFTER_TURN
        } else {
            WITH_NEXT
        })
    }

    /// The user chose in the picker: another model, or the effort of the model. The start
    /// composer's rows that ask nothing (the provider's default) stand for the value they name,
    /// which the agent is told: it has a model and an effort already.
    pub fn choose(&mut self, chosen: Chosen, effort: Option<SharedString>, cx: &mut Context<Self>) {
        let Chosen::Model(_, model) = chosen else {
            return;
        };
        let now = self.pick();
        let provider = self.provider();
        let named = model
            .map(|model| model.to_string())
            .or_else(|| provider?.default_model().map(|m| m.id.clone()));
        let own_model = now
            .model
            .clone()
            .or_else(|| provider?.default_model().map(|m| m.id.clone()));
        let (model, effort) = if named != own_model {
            (named, None)
        } else {
            let own = Pickers {
                providers: &self.providers,
                pick: &now,
                project: (None, None),
            }
            .own_effort()
            .cloned();
            (None, effort.map(|effort| effort.to_string()).or(own))
        };
        // What it runs on already, or is going to, is not a change.
        let (model, effort) = (
            model.filter(|model| now.model.as_ref() != Some(model)),
            effort.filter(|effort| now.effort.as_ref() != Some(effort)),
        );
        if model.is_none() && effort.is_none() {
            return;
        }
        self.held = false;
        if model.is_some() {
            self.asked.model = model.clone();
        }
        if effort.is_some() {
            self.asked.effort = effort.clone();
        }
        let command = Command::SetOptions {
            session: self.session.clone(),
            model,
            effort,
        };
        crate::commands::ask(&self.shell, &self.asks, command, cx);
    }

    /// The daemon's answer: a refusal takes the change back and a toast says why.
    pub fn answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.held = refused.is_none() && !self.asked.is_empty();
        if let Some(why) = refused {
            self.asked = Pending::default();
            gpui_design_system::toast(
                gpui_design_system::Toast::failure("The model or effort was not changed")
                    .detail(why),
                cx,
            );
        }
        cx.notify();
    }

    /// The agent's session refused the change: it goes on with what it ran on, which the
    /// pickers say again, and a toast says why, naming what it offers.
    pub fn refused(&mut self, reason: String, cx: &mut Context<Self>) {
        self.asked = Pending::default();
        gpui_design_system::toast(
            gpui_design_system::Toast::failure("The model or effort was not changed")
                .detail(reason),
            cx,
        );
        cx.notify();
    }

    /// The picker, with the dot of a change waiting over its corner: it fades in and out over
    /// `HOVER` and takes no room, so nothing around it moves.
    fn picker(
        &self,
        picker: ModelPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let waiting = self.waiting();
        let shown = fade(MARK, waiting.is_some(), HOVER, window, cx);
        let acc = palette(cx).acc;
        div().relative().child(picker).when(shown > 0.0, |d| {
            d.child(
                div()
                    .id(MARK)
                    .absolute()
                    .top(px(4.))
                    .right(px(2.))
                    .size(px(6.))
                    .rounded_full()
                    .bg(acc)
                    .opacity(shown)
                    // The dot says when the change applies: there is no room for it in the menu.
                    .aria_label(SharedString::from(waiting.unwrap_or(WITH_NEXT)))
                    .test_support(),
            )
        })
    }
}

impl Render for SessionOptions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let pick = self.pick();
        // Only its own provider is offered: a provider change needs a new session of another CLI.
        let picker = agent_picker(
            "session-agent-picker",
            &Pickers {
                providers: &self.providers,
                pick: &pick,
                project: (None, None),
            },
            true,
            move |chosen, effort, _, cx| {
                this.update(cx, |this, cx| this.choose(chosen, effort, cx))
                    .ok();
            },
        );
        let shown = self.shown();
        div()
            .id("reply-model")
            .h(px(28.))
            .flex()
            .items_center()
            .text_size(px(12.5))
            .text_color(p.mute)
            .aria_label(SharedString::from(shown.clone()))
            // Without a picker (a provider no longer offered), what it runs on is only said.
            .child(match picker {
                Some(picker) => self.picker(picker, window, cx).into_any_element(),
                None => div()
                    .px(space::SM)
                    .child(SharedString::from(shown))
                    .into_any_element(),
            })
            .test_support()
    }
}
