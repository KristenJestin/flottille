//! The rail's Subscriptions section: what the session's agents wait for outside Flottille (the
//! next event of a pull request, `docs/design.md` §5.6): the pull request, the events, since when
//! and until when, or a wake the agent asked for and when it is due, each with a Cancel that the daemon carries out (`subscription.cancel`). Nothing
//! is drawn while the session waits for nothing; a row unfolds when a subscription opens and folds
//! away when it ends, whatever ended it.

use std::time::Duration;

use flottille_ui::{
    Button, Icon, Item, Listing, Palette, Type, icon, palette, presence, presence_of, space,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Task, TestSupportExt as _, WeakEntity,
    Window, div, px,
};
use serde_json::Value;

use crate::project::repository_label;
use crate::runs::rail_label;
use crate::session::{SessionPage, clock, said_at};
use crate::shell::now_ms;

/// A subscription of the session, as `subscription.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct SubscriptionRow {
    pub id: String,
    /// The agent it wakes.
    pub agent: String,
    /// The pull request in a few words: `#12 of api` (the project's own name for its folder);
    /// `Wake: <why>` for a wake.
    pub pull: String,
    /// A wake the agent asked for (the source `timer`), not a pull request's events.
    pub wake: bool,
    pub events: Vec<String>,
    pub created_at: u64,
    pub expires_at: u64,
}

impl SubscriptionRow {
    pub fn of(value: &Value) -> SubscriptionRow {
        let target = &value["target"];
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let wake = value["source"] == "timer";
        SubscriptionRow {
            id: text(&value["id"]),
            agent: text(&value["agent"]),
            pull: match wake {
                true => format!("Wake: {}", text(&target["why"])),
                false => format!(
                    "#{} of {}",
                    target["number"].as_u64().unwrap_or_default(),
                    repository_label(
                        target["project"].as_str().unwrap_or_default(),
                        target["repository"].as_str().unwrap_or_default()
                    )
                ),
            },
            wake,
            events: value["events"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|event| event.as_str().map(str::to_owned))
                .collect(),
            created_at: value["created_at"].as_u64().unwrap_or(0),
            expires_at: value["expires_at"].as_u64().unwrap_or(0),
        }
    }

    /// `since 14:05 · expires in 30 min`; `at 10:42` for a wake.
    pub fn times(&self, now: u64) -> String {
        if self.wake {
            return format!("at {}", clock(self.expires_at));
        }
        let since = said_at(self.created_at);
        let since = since.rsplit(", ").next().unwrap_or(&since);
        let left = self.expires_at.saturating_sub(now).div_ceil(60_000);
        let left = match left {
            0..=119 => format!("{left} min"),
            _ => format!("{} h", left.div_ceil(60)),
        };
        format!("since {since} · expires in {left}")
    }
}

/// The rail's Subscriptions section.
pub struct SubscriptionList {
    page: WeakEntity<SessionPage>,
    rows: Vec<SubscriptionRow>,
    /// The rows as drawn: one that ends folds away before it is gone.
    list: Listing<SubscriptionRow>,
    error: Option<SharedString>,
    /// Draws the time left again each half minute, while a subscription is open.
    ticker: Option<Task<()>>,
}

impl SubscriptionList {
    pub fn new(page: WeakEntity<SessionPage>) -> Self {
        Self {
            page,
            rows: Vec::new(),
            list: Listing::default(),
            error: None,
            ticker: None,
        }
    }

    pub fn rows(&self) -> &[SubscriptionRow] {
        &self.rows
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// What the daemon listed; nothing is drawn again when it is what was drawn.
    pub fn set(&mut self, rows: Vec<SubscriptionRow>, cx: &mut Context<Self>) {
        if self.rows == rows {
            return;
        }
        if rows.is_empty() {
            self.ticker = None;
        } else if self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(30))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
        self.rows = rows;
        cx.notify();
    }

    /// The daemon refused to cancel one, and why; none clears it.
    pub fn failed(&mut self, why: Option<String>, cx: &mut Context<Self>) {
        self.error = why.map(Into::into);
        cx.notify();
    }

    /// One subscription: its pull request, the events waited for, since when and until when, and
    /// its Cancel.
    fn row(&self, row: &SubscriptionRow, now: u64, p: Palette) -> AnyElement {
        let id = |part: &str| ElementId::Name(format!("subscription-{part}-{}", row.id).into());
        let line = |id: ElementId, text: String| {
            div()
                .id(id)
                .text_size(px(11.5))
                .text_color(p.faint)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .aria_label(SharedString::from(text.clone()))
                .child(SharedString::from(text))
                .test_support()
        };
        let cancel = {
            let (page, subscription) = (self.page.clone(), row.id.clone());
            Button::icon(id("cancel"), Icon::Close, "Cancel").on_click(move |_, _, cx| {
                page.update(cx, |page, cx| page.cancel_subscription(&subscription, cx))
                    .ok();
            })
        };
        div()
            .id(id("row"))
            .flex()
            .items_start()
            .gap(space::SM)
            .p(space::SM)
            .child(
                div()
                    .flex_none()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .child(icon(Icon::Clock).size(px(14.)).text_color(p.faint)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(id("pull"))
                            .text_size(px(13.))
                            .medium()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .aria_label(SharedString::from(row.pull.clone()))
                            .child(SharedString::from(row.pull.clone()))
                            .test_support(),
                    )
                    .when(!row.wake, |d| {
                        d.child(line(id("events"), row.events.join(", ")))
                    })
                    .child(line(id("times"), row.times(now))),
            )
            .child(cancel)
            .into_any_element()
    }
}

impl Render for SubscriptionList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries: Vec<(String, SubscriptionRow)> = self
            .rows
            .iter()
            .map(|row| (row.id.clone(), row.clone()))
            .collect();
        let at = cx.background_executor().now();
        self.list.sync(entries, at, cx.reduce_motion());
        let p = palette(cx);
        let now = now_ms();
        let present = !self.list.is_empty();
        let listed: Vec<(String, SubscriptionRow, flottille_ui::Placement)> = self
            .list
            .rows()
            .map(|listed| {
                (
                    listed.key.to_owned(),
                    listed.value.clone(),
                    listed.placement,
                )
            })
            .collect();
        let rows: Vec<AnyElement> = listed
            .into_iter()
            .map(|(key, row, placement)| {
                let content = self.row(&row, now, p);
                Item::new(
                    ElementId::Name(format!("subscription-item-{key}").into()),
                    placement,
                    content,
                )
                .into_any_element()
            })
            .collect();
        let section = div()
            .id("rail-subscriptions")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .pb(space::MD)
            .child(
                div()
                    .id("rail-subscriptions-title")
                    .aria_label("Subscriptions")
                    .child(rail_label("Subscriptions", None, cx))
                    .test_support(),
            )
            .children(rows)
            .child(presence_of(
                "rail-subscriptions-error-block",
                self.error.clone(),
                move |error| {
                    div()
                        .id("rail-subscriptions-error")
                        .px(space::SM)
                        .py(space::XS)
                        .text_size(px(11.5))
                        .text_color(p.fail)
                        .aria_label(error.clone())
                        .child(error)
                        .test_support()
                        .into_any_element()
                },
            ))
            .test_support();
        presence("rail-subscriptions-presence", present, section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(expires_at: u64) -> SubscriptionRow {
        SubscriptionRow::of(&json!({
            "id": "sub-1",
            "target": {"project": "app", "workspace": "login", "repository": ".", "number": 12},
            "events": ["checks", "review"],
            "created_at": 0,
            "expires_at": expires_at,
        }))
    }

    #[test]
    fn a_repository_that_is_the_project_folder_is_named_by_the_project() {
        assert_eq!(row(0).pull, "#12 of app");
    }

    #[test]
    fn a_wake_is_named_by_its_why_and_says_when_it_is_due() {
        let wake = SubscriptionRow::of(&json!({
            "id": "sub-2", "agent": "a1", "source": "timer",
            "target": {"why": "the deploy of api"}, "events": ["wake"],
            "created_at": 0, "expires_at": 20 * 60_000,
        }));
        assert!(wake.wake);
        assert_eq!(wake.pull, "Wake: the deploy of api");
        assert_eq!(wake.times(0), format!("at {}", clock(20 * 60_000)));
        assert!(!row(0).wake);
    }

    #[test]
    fn the_time_left_is_said_in_minutes_then_in_hours() {
        assert!(row(30 * 60_000).times(0).ends_with("expires in 30 min"));
        assert!(row(29 * 60_000 + 1).times(0).ends_with("expires in 30 min"));
        assert!(row(3 * 3_600_000).times(0).ends_with("expires in 3 h"));
        assert!(row(0).times(5).ends_with("expires in 0 min"));
    }
}
