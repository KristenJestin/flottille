//! The Usage tab of Settings (`docs/usage.md`, `docs/design.md` §10): what agents used over the
//! last days, in weighted tokens, and what a task that ends merged costs. The window computes
//! nothing: it shows what `usage.stats` says, read again when the tab is shown and when the period
//! or the project changes.
//!
//! The kit has no bar chart: the bars by day are drawn here from its primitives (`growing` eases
//! each bar to its height, `tipped` says its day, `changing` fades the chart back in when the
//! period or the project changes).

use gpui_design_system::{
    EmptyLine, Field, Icon, ListCard, ListRow, Listing, Section, Segmented, Select, SelectOption,
    Skeleton, Type as _, awaiting, changing, enter, growing, palette, presence, space, tipped,
};
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, Window,
    div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::{perform_later, problem_line, spaced, stack};
use crate::shell::Shell;

/// The periods the tab offers, in days: the last days, today included.
pub const PERIODS: [(&str, &str); 3] = [("7", "7 days"), ("30", "30 days"), ("90", "90 days")];

/// The period the tab opens on.
const FIRST_PERIOD: usize = 1;

/// The height of the tallest bar.
const CHART: f32 = 96.;

/// What the first section says under its title.
pub const COST_SENTENCE: &str = "What the finished tasks cost for each one merged. A task is a \
     workspace; one removed without a merged pull request counts against the merged ones.";

/// What the days' section says under its title: the unit.
pub const WEIGHTED_SENTENCE: &str = "Input + 1.25 × cache write + 0.1 × cache read + 5 × \
     output, so that providers and subscriptions compare.";

/// Said once when a project has no forge to say which tasks were merged.
pub const NO_FORGE: &str = "Tasks of projects without a forge show their cost but are left out: \
     nothing says whether they were merged.";

/// A sum of weighted tokens: of a day, a model, or what was on.
#[derive(Clone, Debug, PartialEq)]
pub struct Spent {
    pub key: String,
    pub weighted: f64,
    /// The part of it that was cache reads, 0 to 1.
    pub share: f64,
    pub per_merged: Option<f64>,
}

/// What `usage.stats` says of a period: its days, its models, what was on, and its tasks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub days: Vec<Spent>,
    pub models: Vec<Spent>,
    pub tunings: Vec<Spent>,
    pub weighted: f64,
    pub share: f64,
    pub merged: u64,
    pub failed: u64,
    pub in_progress: u64,
    pub no_forge: u64,
    pub per_merged: Option<f64>,
}

impl Usage {
    /// The three answers of `usage.stats`, by day, by model and by tuning.
    pub fn of(by_day: &Value, by_model: &Value, by_tuning: &Value) -> Usage {
        let spent = |stats: &Value| -> Vec<Spent> {
            stats["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|group| Spent {
                    key: group["key"].as_str().unwrap_or_default().to_owned(),
                    weighted: group["weighted"].as_f64().unwrap_or(0.0),
                    share: group["cache_read_share"].as_f64().unwrap_or(0.0),
                    per_merged: group["tasks"]["cost_per_merged_task"].as_f64(),
                })
                .collect()
        };
        let total = &by_day["total"];
        let count = |outcome: &str| total["tasks"][outcome]["count"].as_u64().unwrap_or(0);
        Usage {
            days: spent(by_day),
            models: spent(by_model),
            tunings: spent(by_tuning),
            weighted: total["weighted"].as_f64().unwrap_or(0.0),
            share: total["cache_read_share"].as_f64().unwrap_or(0.0),
            merged: count("merged"),
            failed: count("failed"),
            in_progress: count("in_progress"),
            no_forge: count("no_forge"),
            per_merged: total["tasks"]["cost_per_merged_task"].as_f64(),
        }
    }
}

/// A number of weighted tokens, short: `950`, `12.3k`, `4.5M`.
pub fn tokens(weighted: f64) -> String {
    match weighted {
        w if w >= 1_000_000.0 => format!("{:.1}M", w / 1_000_000.0),
        w if w >= 1_000.0 => format!("{:.1}k", w / 1_000.0),
        w => format!("{w:.0}"),
    }
}

fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

/// What was on, as the tab names it.
fn tuning_label(key: &str) -> String {
    match key {
        "" => "Nothing on".to_owned(),
        "unchanged" => "Unchanged files in one line".to_owned(),
        other => other.to_owned(),
    }
}

pub struct UsageSection {
    shell: WeakEntity<Shell>,
    /// The index of the period in [`PERIODS`].
    period: usize,
    /// The project chosen; every project when none.
    project: Option<String>,
    /// What the daemon said of the period and the project chosen.
    usage: Option<Usage>,
    problem: Option<SharedString>,
    models: Listing<Spent>,
    tunings: Listing<Spent>,
}

impl UsageSection {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            period: FIRST_PERIOD,
            project: None,
            usage: None,
            problem: None,
            models: Listing::default(),
            tunings: Listing::default(),
        }
    }

    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// The days of the period chosen.
    pub fn days(&self) -> u64 {
        PERIODS[self.period].0.parse().unwrap_or(30)
    }

    /// Asks the daemon for the period and the project chosen.
    pub fn read(&mut self, cx: &mut Context<Self>) {
        let (project, days) = (self.project.clone(), self.days());
        perform_later(&self.shell, Command::UsageRead { project, days }, cx);
    }

    pub fn choose_period(&mut self, period: usize, cx: &mut Context<Self>) {
        if period < PERIODS.len() && period != self.period {
            self.period = period;
            self.read(cx);
            cx.notify();
        }
    }

    pub fn choose_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        if project != self.project {
            self.project = project;
            self.read(cx);
            cx.notify();
        }
    }

    /// The daemon's answer for a period and a project: one asked before another choice is
    /// dropped.
    pub fn answered(
        &mut self,
        project: Option<String>,
        days: u64,
        read: Result<Usage, String>,
        cx: &mut Context<Self>,
    ) {
        if project != self.project || days != self.days() {
            return;
        }
        match read {
            Ok(usage) => {
                self.usage = Some(usage);
                self.problem = None;
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    fn projects(&self, cx: &Context<Self>) -> Vec<String> {
        self.shell
            .upgrade()
            .map(|shell| {
                shell
                    .read(cx)
                    .snapshot()
                    .projects
                    .iter()
                    .map(|project| project.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let period = Segmented::new("usage-period", "Period", PERIODS, self.period).on_choose(
            move |index, _, cx| {
                this.update(cx, |this, cx| this.choose_period(index, cx))
                    .ok();
            },
        );
        let projects = self.projects(cx);
        let chosen = self
            .project
            .as_ref()
            .and_then(|name| projects.iter().position(|p| p == name))
            .map_or(0, |at| at + 1);
        let options = std::iter::once(SelectOption::new("all", "All projects")).chain(
            projects
                .iter()
                .map(|name| SelectOption::new(name.clone(), name.clone())),
        );
        let this = cx.entity().downgrade();
        let project = Select::new("usage-project", options, chosen)
            .fit()
            .on_choose(move |at, _, cx| {
                let project = at.checked_sub(1).map(|at| projects[at].clone());
                this.update(cx, |this, cx| this.choose_project(project, cx))
                    .ok();
            });
        div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap(space::LG)
            .child(Field::new("usage-period-field", "Period", period))
            .child(Field::new("usage-project-field", "Project", project))
            .into_any_element()
    }

    /// The cost per merged task, and the tasks it is made of.
    fn cost(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let usage = self.usage.clone().unwrap_or_default();
        let figure = match usage.per_merged {
            Some(cost) => format!("{} weighted tokens", tokens(cost)),
            None => "No task merged yet".to_owned(),
        };
        let no_forge = usage.no_forge > 0;
        let fact = move |id: &'static str, label: &'static str, value: u64| {
            div()
                .flex()
                .flex_col()
                .gap(space::XS)
                .child(div().text_size(px(12.5)).text_color(p.mute).child(label))
                .child(
                    div()
                        .id(id)
                        .aria_label(SharedString::from(value.to_string()))
                        .mono(13.)
                        .child(value.to_string())
                        .test_support(),
                )
        };
        let key = format!("{}-{:?}-{figure}", self.days(), self.project);
        let content = changing("usage-cost-change", key, move |_| {
            div()
                .flex()
                .flex_col()
                .gap(space::MD)
                .child(
                    div()
                        .id("usage-cost")
                        .aria_label(SharedString::from(figure.clone()))
                        .text_size(px(22.))
                        .child(figure)
                        .test_support(),
                )
                .child(
                    div()
                        .flex()
                        .gap(space::XL)
                        .child(fact("usage-merged", "Merged", usage.merged))
                        .child(fact("usage-failed", "Failed", usage.failed))
                        .child(fact(
                            "usage-in-progress",
                            "In progress, left out",
                            usage.in_progress,
                        )),
                )
                .into_any_element()
        });
        let waiting = self.usage.is_none() && self.problem.is_none();
        stack(space::MD)
            .child(spaced(space::MD, self.controls(cx)))
            .child(spaced(
                space::MD,
                awaiting(
                    "usage-cost-waiting",
                    waiting,
                    Skeleton::new("usage-cost-skeleton").h(px(72.)),
                    content,
                ),
            ))
            .child(
                presence(
                    "usage-no-forge-block",
                    no_forge,
                    div()
                        .id("usage-no-forge")
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(NO_FORGE)
                        .test_support(),
                )
                .space(space::MD),
            )
            .child(problem_line(
                "usage-problem",
                self.problem.clone(),
                space::MD,
                cx,
            ))
            .into_any_element()
    }

    /// The bars of the days, each eased to its height.
    fn days_chart(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let usage = self.usage.clone().unwrap_or_default();
        let top = usage
            .days
            .iter()
            .map(|day| day.weighted)
            .fold(0.0, f64::max);
        let mut bars = div()
            .id("usage-days-chart")
            .h(px(CHART))
            .flex()
            .items_end()
            .gap(space::XS);
        for day in &usage.days {
            // A day with nothing is a hairline: a bar is never drawn at no height.
            let height = if top > 0.0 {
                (day.weighted / top * f64::from(CHART)).max(2.0) as f32
            } else {
                2.0
            };
            let bar = div()
                .id(ElementId::Name(
                    format!("usage-day-{}-column", day.key).into(),
                ))
                .flex_1()
                .min_w(px(2.))
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .child(growing(
                    ElementId::Name(format!("usage-day-{}-bar", day.key).into()),
                    div()
                        .id(ElementId::Name(format!("usage-day-{}", day.key).into()))
                        .aria_label(SharedString::from(format!(
                            "{}: {}",
                            day.key,
                            tokens(day.weighted)
                        )))
                        .h(px(height))
                        .rounded_t(px(2.))
                        .bg(p.acc)
                        .test_support(),
                ));
            let said = format!(
                "{}: {}, {} cache reads",
                day.key,
                tokens(day.weighted),
                percent(day.share)
            );
            bars = bars.child(tipped(bar, said, CHART, window, cx));
        }
        let total = format!(
            "{} in all, {} of it cache reads",
            tokens(usage.weighted),
            percent(usage.share)
        );
        let key = format!("{}-{:?}", self.days(), self.project);
        let chart = changing("usage-days-change", key, move |_| bars.into_any_element());
        let line = changing("usage-total-change", total, move |total| {
            div()
                .id("usage-total")
                .aria_label(total.clone())
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(total)
                .test_support()
                .into_any_element()
        });
        awaiting(
            "usage-days-waiting",
            self.usage.is_none() && self.problem.is_none(),
            Skeleton::new("usage-days-skeleton").h(px(CHART + 28.)),
            div()
                .flex()
                .flex_col()
                .gap(space::SM)
                .child(chart)
                .child(line),
        )
        .into_any_element()
    }

    /// A list of sums, by model or by what was on.
    fn list(
        id: &'static str,
        rows: &Listing<Spent>,
        label: fn(&str) -> String,
        empty: bool,
    ) -> AnyElement {
        let mut card = ListCard::new(id);
        for row in rows.rows() {
            let spent = row.value;
            let mut line = format!("{} cache reads", percent(spent.share));
            if let Some(cost) = spent.per_merged {
                line.push_str(&format!(", {} per merged task", tokens(cost)));
            }
            card = card.item(
                ElementId::Name(format!("{id}-item-{}", row.key).into()),
                row.placement,
                ListRow::new(
                    ElementId::Name(format!("{id}-{}", row.key).into()),
                    label(&spent.key),
                )
                .line(line)
                .meta(tokens(spent.weighted)),
            );
        }
        stack(space::MD)
            .child(presence(format!("{id}-block"), !rows.is_empty(), card).space(space::MD))
            .child(
                presence(
                    format!("{id}-empty-block"),
                    empty,
                    EmptyLine::new(
                        format!("{id}-empty"),
                        Icon::Clock,
                        "No agent worked in these days: their turns show here once they do.",
                    ),
                )
                .space(space::MD),
            )
            .into_any_element()
    }
}

impl Render for UsageSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        if let Some(usage) = &self.usage {
            let keyed = |spent: &Vec<Spent>| {
                spent
                    .iter()
                    .map(|s| (s.key.clone(), s.clone()))
                    .collect::<Vec<_>>()
            };
            self.models.sync(keyed(&usage.models), now, reduce);
            self.tunings.sync(keyed(&usage.tunings), now, reduce);
        }
        let empty = self.usage.as_ref().is_some_and(|u| u.models.is_empty());
        let model_label: fn(&str) -> String = |key| {
            if key.is_empty() {
                "A model not named".to_owned()
            } else {
                key.to_owned()
            }
        };
        let sections = [
            Section::new("usage-tasks", "Cost per merged task")
                .sentence(COST_SENTENCE)
                .child(self.cost(cx))
                .into_any_element(),
            Section::new("usage-days", "Weighted tokens by day")
                .sentence(WEIGHTED_SENTENCE)
                .child(self.days_chart(window, cx))
                .into_any_element(),
            Section::new("usage-models", "By model")
                .count(self.models.len().to_string())
                .child(Self::list(
                    "usage-models-list",
                    &self.models,
                    model_label,
                    empty,
                ))
                .into_any_element(),
            Section::new("usage-tunings", "By optimisation on")
                .count(self.tunings.len().to_string())
                .sentence("What was on during each turn: the same days, with and without each one.")
                .child(Self::list(
                    "usage-tunings-list",
                    &self.tunings,
                    tuning_label,
                    empty,
                ))
                .into_any_element(),
        ];
        div()
            .flex()
            .flex_col()
            .gap(space::XXL)
            .children(sections.into_iter().enumerate().map(|(index, section)| {
                enter(
                    ElementId::Name(format!("enter-usage-{index}").into()),
                    index,
                    section,
                )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_number_of_weighted_tokens_is_said_short() {
        assert_eq!(tokens(950.0), "950");
        assert_eq!(tokens(12_340.0), "12.3k");
        assert_eq!(tokens(4_500_000.0), "4.5M");
    }

    #[test]
    fn the_answers_give_the_days_the_models_and_the_tasks() {
        let group = |key: &str, weighted: f64| {
            json!({"key": key, "weighted": weighted, "cache_read_share": 0.5,
                "tasks": {"cost_per_merged_task": null}})
        };
        let day = json!({"total": {"weighted": 30.0, "cache_read_share": 0.25,
            "tasks": {"merged": {"count": 2}, "failed": {"count": 1},
                "in_progress": {"count": 3}, "no_forge": {"count": 0},
                "cost_per_merged_task": 15.0}},
            "groups": [group("2026-11-14", 10.0), group("2026-11-15", 20.0)]});
        let model = json!({"groups": [group("model-a", 30.0)]});
        let tuning = json!({"groups": [group("", 30.0)]});

        let usage = Usage::of(&day, &model, &tuning);

        assert_eq!(usage.days.len(), 2);
        assert_eq!(usage.models[0].key, "model-a");
        assert_eq!((usage.merged, usage.failed, usage.in_progress), (2, 1, 3));
        assert_eq!(usage.per_merged, Some(15.0));
    }
}
