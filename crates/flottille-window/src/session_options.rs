//! What the session's root runs on, under its field: the provider (shown, never changed here: a
//! provider change needs a new session of another CLI), and the model and effort pickers, the
//! start composer's own (`composer::Pickers`). A change applies between turns: chosen while a turn
//! runs it is held by the daemon and set before the next prompt: the picker of what changes carries
//! a dot until the agent took it, and its menu says when it applies (`agent.options`,
//! `docs/agents.md`, "The options"). Nothing under the bar, so the composer keeps its height.

use gpui_design_system::motion::{HOVER, PANEL};
use gpui_design_system::{Align, Dropdown, MenuItem, fade, palette, section_label, space};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Command, Pick};
use crate::composer::{Entry, Menu, Pickers, levels};
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

/// The id of the menu's line that says it.
const APPLIES: &str = "session-options-applies";

pub struct SessionOptions {
    shell: WeakEntity<Shell>,
    /// The root agent: the only one whose options this changes.
    session: String,
    runs: Runs,
    providers: Providers,
    menu: Option<Menu>,
    /// What was chosen and the tree has not said yet: the pickers already show it.
    asked: Pending,
    /// The daemon holds what was asked: the next tree it says has it, waiting, set or replaced
    /// by another change.
    held: bool,
}

impl SessionOptions {
    pub fn new(shell: WeakEntity<Shell>, session: String) -> Self {
        Self {
            shell,
            session,
            runs: Runs::default(),
            providers: Providers::default(),
            menu: None,
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

    /// What the model picker says.
    pub fn model_shown(&self) -> String {
        let pick = self.pick();
        Pickers {
            providers: &self.providers,
            pick: &pick,
            project: (None, None),
        }
        .model_shown()
    }

    /// What the effort picker says.
    pub fn effort_shown(&self) -> String {
        let pick = self.pick();
        Pickers {
            providers: &self.providers,
            pick: &pick,
            project: (None, None),
        }
        .effort_shown()
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

    /// The menu's rows: when a change of what it picks waits, a line that says when it applies
    /// first.
    pub fn entries(&self, menu: Menu) -> Vec<Entry> {
        let pick = self.pick();
        let rows = Pickers {
            providers: &self.providers,
            pick: &pick,
            project: (None, None),
        }
        .entries(menu);
        let when = self.waiting().filter(|_| self.waits(menu));
        let applies = when.map(|when| Entry {
            id: APPLIES.into(),
            label: when.into(),
            note: None,
            on: false,
            pick: None,
        });
        applies.into_iter().chain(rows).collect()
    }

    /// Whether a change of the model, or of the effort, waits.
    fn waits(&self, menu: Menu) -> bool {
        match menu {
            Menu::Model => self.asked.model.is_some() || self.runs.pending.model.is_some(),
            _ => self.asked.effort.is_some() || self.runs.pending.effort.is_some(),
        }
    }

    /// Whether the model has efforts to choose among, once a session said them.
    fn has_efforts(&self) -> bool {
        let pick = self.pick();
        self.provider().is_some_and(|provider| {
            crate::composer::efforts_of(provider, pick.model.as_deref())
                .is_some_and(|efforts| levels(efforts).next().is_some())
        })
    }

    pub fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    /// The user picked an entry of the model's or the effort's list. The start composer's rows
    /// that ask nothing (the provider's default) stand for the value they name, which the agent
    /// is told: it has a model and an effort already.
    pub fn choose(&mut self, menu: Menu, pick: &Pick, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
        let now = self.pick();
        let provider = self.provider();
        let (model, effort) = match menu {
            Menu::Model => (
                pick.model
                    .clone()
                    .or_else(|| provider?.default_model().map(|m| m.id.clone())),
                None,
            ),
            Menu::Effort => (
                None,
                pick.effort.clone().or_else(|| {
                    let pickers = Pickers {
                        providers: &self.providers,
                        pick: &now,
                        project: (None, None),
                    };
                    pickers.own_effort().cloned()
                }),
            ),
            _ => return,
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
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
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

    fn handler(
        cx: &Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |window, cx| {
            this.update(cx, |this, cx| f(this, window, cx)).ok();
        }
    }

    fn items(&self, menu: Menu, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.entries(menu)
            .into_iter()
            .map(|entry| {
                // When a change applies is a heading of the menu, not a choice: nothing to hover
                // or press.
                if entry.id == APPLIES {
                    return div()
                        .id(APPLIES)
                        .test_support()
                        .child(section_label(&entry.label, cx))
                        .into_any_element();
                }
                let item = MenuItem::new(entry.id, entry.label).selected(entry.on);
                let item = match entry.note {
                    Some(note) => item.note(note),
                    None => item,
                };
                match entry.pick {
                    Some(pick) => item.on_click(Self::handler(cx, move |this, _, cx| {
                        this.choose(menu, &pick, cx)
                    })),
                    None => item,
                }
                .into_any_element()
            })
            .collect()
    }

    /// A picker, with the dot of a change waiting over its corner: it fades in and out over
    /// `HOVER` and takes no room, so nothing around it moves.
    fn picker(
        &self,
        id: &'static str,
        menu: Menu,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let items = self.items(menu, cx);
        let dropdown = Dropdown::new(id, label)
            .mono()
            .align(Align::Start)
            .width(px(280.))
            .open(self.menu == Some(menu))
            .on_toggle(Self::handler(cx, move |this, _, cx| {
                this.toggle_menu(menu, cx)
            }))
            .items(items);
        let mark = match menu {
            Menu::Model => "session-model-pending",
            _ => "session-effort-pending",
        };
        let shown = fade(mark, self.waits(menu), HOVER, window, cx);
        let acc = palette(cx).acc;
        div().relative().child(dropdown).when(shown > 0.0, |d| {
            d.child(
                div()
                    .id(mark)
                    .absolute()
                    .top(px(4.))
                    .right(px(2.))
                    .size(px(6.))
                    .rounded_full()
                    .bg(acc)
                    .opacity(shown)
                    .aria_label(SharedString::from("A change waits"))
                    .test_support(),
            )
        })
    }
}

impl Render for SessionOptions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let provider = self
            .provider()
            .map(|provider| provider.title.clone())
            .unwrap_or_else(|| self.runs.provider.clone());
        let model = self.picker(
            "session-model-picker",
            Menu::Model,
            self.model_shown(),
            window,
            cx,
        );
        // The effort picker comes and goes with the model's efforts: its room opens and it fades
        // in, folds and fades out; at rest it is its natural width.
        let efforts = fade(
            "session-effort-shown",
            self.has_efforts(),
            PANEL,
            window,
            cx,
        );
        let effort = (efforts > 0.0).then(|| {
            let label = self.effort_shown();
            let wide = px(EFFORT_FRAME + EFFORT_CHAR * label.chars().count() as f32);
            let picker = self.picker("session-effort-picker", Menu::Effort, label, window, cx);
            div()
                .flex_none()
                .opacity(efforts)
                .when(efforts < 1.0, |d| d.w(wide * efforts).overflow_hidden())
                .child(picker)
        });
        div()
            .id("reply-model")
            .h(px(28.))
            .flex()
            .items_center()
            .text_size(px(12.5))
            .text_color(p.mute)
            .aria_label(SharedString::from(self.shown()))
            .when(!provider.is_empty(), |d| {
                d.child(
                    div()
                        .id("reply-provider")
                        .px(space::SM)
                        .aria_label(SharedString::from(provider.clone()))
                        .child(SharedString::from(provider))
                        .test_support(),
                )
            })
            .child(model)
            .children(effort)
            .test_support()
    }
}

/// The room a picker takes besides its words (padding, gap and chevron), and what a mono letter
/// takes: how wide the effort picker is while it opens and folds.
const EFFORT_FRAME: f32 = 34.;
const EFFORT_CHAR: f32 = 7.2;
