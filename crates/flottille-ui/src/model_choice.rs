//! `ModelChoice`: the one way to choose a model (`docs/reference/gpui.md`, rule 15). One select
//! whose entries are `<provider> · <model>`, grouped by provider, with the effort in a narrow
//! select beside it when the model has efforts. A default that is a rule is the first option, said
//! as the rule and, on its second line, what it gives today; a choice that is not a model comes
//! before the models. "No model" is never an entry: a feature is turned off by its switch.
//!
//! The kit knows no provider: the screen gives the models the application knows ([`ModelEntry`]).

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::field::Field;
use crate::interact::fade;
use crate::motion::PANEL;
use crate::select::{Select, SelectOption};
use crate::space;
use crate::theme::palette;

/// The width of the model's select on a line of a card.
const COMPACT: f32 = 240.;
/// The width of the effort's select.
const EFFORT: f32 = 120.;
/// The width of the `picked` mark.
const MARK: f32 = 52.;

/// A model the screen offers: its provider (its key and its name) and the model (`None` is the
/// provider's own default), with the efforts it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelEntry {
    provider: SharedString,
    provider_title: SharedString,
    model: Option<SharedString>,
    name: SharedString,
    efforts: Vec<SharedString>,
}

impl ModelEntry {
    pub fn new(
        provider: impl Into<SharedString>,
        provider_title: impl Into<SharedString>,
        model: Option<SharedString>,
        name: impl Into<SharedString>,
    ) -> Self {
        Self {
            provider: provider.into(),
            provider_title: provider_title.into(),
            model,
            name: name.into(),
            efforts: Vec::new(),
        }
    }

    pub fn efforts(mut self, efforts: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.efforts = efforts.into_iter().map(Into::into).collect();
        self
    }

    fn is(&self, provider: &str, model: Option<&str>) -> bool {
        self.provider == provider && self.model.as_deref() == model
    }
}

/// What is chosen: the first option (the default, or As in Settings), a choice that is not a
/// model, by its key, or a provider's model (`None` its default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    First,
    Choice(SharedString),
    Model(SharedString, Option<SharedString>),
}

type OnChoose = Rc<dyn Fn(Chosen, Option<SharedString>, &mut Window, &mut App) + 'static>;

/// The first option: a rule and what it gives today.
#[derive(Clone)]
struct First {
    title: SharedString,
    gives: SharedString,
}

#[derive(IntoElement)]
pub struct ModelChoice {
    id: ElementId,
    label: SharedString,
    chosen: Chosen,
    effort: Option<SharedString>,
    first: Option<First>,
    choices: Vec<SelectOption>,
    models: Vec<ModelEntry>,
    picked: bool,
    compact: bool,
    hint: Option<SharedString>,
    cost: Option<SharedString>,
    disabled: bool,
    on_choose: Option<OnChoose>,
}

impl ModelChoice {
    /// The choice `id`: its select `<id>-model` (each option `<id>-model-<key>`), the effort's
    /// `<id>-effort`, the mark `<id>-picked`, the cost line `<id>-cost`.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        chosen: Chosen,
    ) -> Self {
        Self {
            id: ElementId::Name(id.into()),
            label: label.into(),
            chosen,
            effort: None,
            first: None,
            choices: Vec::new(),
            models: Vec::new(),
            picked: false,
            compact: false,
            hint: None,
            cost: None,
            disabled: false,
            on_choose: None,
        }
    }

    /// The first option, a default that is a rule (`Strongest of the provider`), its
    /// second line what it gives today (`Opus for Claude`).
    pub fn default(
        mut self,
        rule: impl Into<SharedString>,
        gives: impl Into<SharedString>,
    ) -> Self {
        self.first = Some(First {
            title: rule.into(),
            gives: gives.into(),
        });
        self
    }

    /// Where a value is inherited: the first option is `As in Settings`, its second line what it
    /// gives.
    pub fn inherited(mut self, gives: impl Into<SharedString>) -> Self {
        self.first = Some(First {
            title: "As in Settings".into(),
            gives: gives.into(),
        });
        self
    }

    /// A choice that is not a model (a summary written by the conversation itself), before the
    /// models.
    pub fn choice(
        mut self,
        key: impl Into<SharedString>,
        title: impl Into<SharedString>,
        line: impl Into<SharedString>,
    ) -> Self {
        self.choices
            .push(SelectOption::new(key, title).line(line.into()));
        self
    }

    pub fn models(mut self, models: impl IntoIterator<Item = ModelEntry>) -> Self {
        self.models = models.into_iter().collect();
        self
    }

    /// The effort chosen with the model, `None` for the model's own.
    pub fn effort(mut self, effort: Option<SharedString>) -> Self {
        self.effort = effort;
        self
    }

    /// The application picked the model chosen, and the user has not seen its notice yet.
    pub fn picked(mut self, picked: bool) -> Self {
        self.picked = picked;
        self
    }

    /// A line of a card: the label on the left, one line under it, the selects on the right.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// The line under the label of a compact choice, `<id>-hint`.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// What the model costs, one line under it.
    pub fn cost(mut self, cost: impl Into<SharedString>) -> Self {
        self.cost = Some(cost.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with what is chosen and its effort, when it changes.
    pub fn on_choose(
        mut self,
        f: impl Fn(Chosen, Option<SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }
}

fn part(id: &ElementId, name: &str) -> SharedString {
    format!("{id}-{name}").into()
}

/// `content` beside its sibling, unfolding to `width` and fading in while `present`, folding
/// away and fading out when not: nothing at all once it is gone.
fn beside(
    id: SharedString,
    present: bool,
    width: f32,
    content: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = fade(ElementId::Name(id.clone()), present, PANEL, window, cx);
    if t == 0.0 && !present {
        return div().hidden().into_any_element();
    }
    div()
        .id(ElementId::Name(id))
        .flex_none()
        .overflow_hidden()
        .w(px((width + f32::from(space::SM)) * t))
        .opacity(t)
        .child(
            div()
                .pl(space::SM)
                .w(px(width + f32::from(space::SM)))
                .child(content),
        )
        .test_support()
        .into_any_element()
}

impl RenderOnce for ModelChoice {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let id = self.id.clone();
        let mut models = self.models.clone();
        // A model chosen that is not offered (written by hand, or of a provider not ready) is
        // still shown.
        if let Chosen::Model(provider, model) = &self.chosen
            && !models.iter().any(|m| m.is(provider, model.as_deref()))
        {
            let title = models
                .iter()
                .find(|m| &m.provider == provider)
                .map_or_else(|| provider.clone(), |m| m.provider_title.clone());
            let name = model.clone().unwrap_or_else(|| "Its default".into());
            models.push(ModelEntry::new(
                provider.clone(),
                title,
                model.clone(),
                name,
            ));
        }
        let mut options = Vec::new();
        let mut targets = Vec::new();
        if let Some(first) = &self.first {
            options.push(SelectOption::new("first", first.title.clone()).line(first.gives.clone()));
            targets.push(Chosen::First);
        }
        for choice in &self.choices {
            targets.push(Chosen::Choice(choice.key()));
            options.push(choice.clone());
        }
        for (at, model) in models.iter().enumerate() {
            options.push(
                SelectOption::new(
                    format!(
                        "{}-{}",
                        model.provider,
                        model.model.as_deref().unwrap_or("default")
                    )
                    .replace(['/', ' '], "-")
                        + if at < self.models.len() { "" } else { "-own" },
                    format!("{} · {}", model.provider_title, model.name),
                )
                .group(model.provider_title.clone()),
            );
            targets.push(Chosen::Model(model.provider.clone(), model.model.clone()));
        }
        let at = targets.iter().position(|t| *t == self.chosen).unwrap_or(0);
        let efforts = match &self.chosen {
            Chosen::Model(provider, model) => models
                .iter()
                .find(|m| m.is(provider, model.as_deref()))
                .map(|m| m.efforts.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let has_efforts = !efforts.is_empty();
        let on_choose = self.on_choose.clone();
        let model = Select::new(part(&id, "model"), options, at)
            .disabled(self.disabled)
            .on_choose({
                let on_choose = on_choose.clone();
                move |index, window, cx| {
                    if let Some(on_choose) = &on_choose {
                        on_choose(targets[index].clone(), None, window, cx);
                    }
                }
            });
        let model = if self.compact {
            model.width(px(COMPACT))
        } else {
            model
        };
        let mut levels: Vec<Option<SharedString>> = vec![None];
        levels.extend(efforts.iter().cloned().map(Some));
        if has_efforts
            && let Some(own) = &self.effort
            && !efforts.contains(own)
        {
            levels.push(Some(own.clone()));
        }
        let effort_at = levels.iter().position(|l| *l == self.effort).unwrap_or(0);
        let effort = Select::new(
            part(&id, "effort"),
            levels.iter().map(|level| match level {
                None => SelectOption::new("default", "Default"),
                Some(level) => SelectOption::new(level.clone(), level.clone()),
            }),
            effort_at,
        )
        .width(px(EFFORT))
        .disabled(self.disabled)
        .on_choose({
            let chosen = self.chosen.clone();
            move |index, window, cx| {
                if let Some(on_choose) = &on_choose {
                    on_choose(chosen.clone(), levels[index].clone(), window, cx);
                }
            }
        });
        let mark = div()
            .id(ElementId::Name(part(&id, "picked-mark")))
            .aria_label("picked")
            .px(space::SM)
            .py(space::XS)
            .rounded(px(6.))
            .bg(p.card2)
            .text_size(px(11.5))
            .text_color(p.mute)
            .child("picked")
            .test_support();
        let row = div()
            .flex()
            .items_center()
            .min_w_0()
            .when(!self.compact, |d| d.w_full())
            .child(
                div()
                    .min_w_0()
                    .when(!self.compact, |d| d.flex_1())
                    .child(model),
            )
            .child(beside(
                part(&id, "effort-block"),
                has_efforts,
                EFFORT,
                effort.into_any_element(),
                window,
                cx,
            ))
            .child(beside(
                part(&id, "picked"),
                self.picked,
                MARK,
                mark.into_any_element(),
                window,
                cx,
            ));
        let cost = self.cost.clone().map(|cost| {
            div()
                .id(ElementId::Name(part(&id, "cost")))
                .aria_label(cost.clone())
                .pt(space::SM)
                .text_size(px(12.))
                .text_color(p.faint)
                .child(cost)
                .test_support()
        });
        if self.compact {
            let words = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id(ElementId::Name(part(&id, "label")))
                        .aria_label(self.label.clone())
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.text)
                        .child(self.label.clone())
                        .test_support(),
                )
                .children(self.hint.clone().map(|hint| {
                    div()
                        .id(ElementId::Name(part(&id, "hint")))
                        .aria_label(hint.clone())
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .child(hint)
                        .test_support()
                }));
            return div()
                .id(id)
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::MD)
                        .child(words)
                        .child(row),
                )
                .children(cost)
                .test_support()
                .into_any_element();
        }
        div()
            .id(id.clone())
            .flex()
            .flex_col()
            .child(Field::new(part(&id, "field"), self.label.clone(), row))
            .children(cost)
            .test_support()
            .into_any_element()
    }
}
