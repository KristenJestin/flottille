//! The rail's Exceptions section: the rules of the gate the user lifted for the session's tree at
//! its agents' ask (`docs/design.md` §7.3), the root's and each sub-agent's, each with what it lets
//! do, the branch or pull request it covers, the agent's reason, who holds it when it is not the root, since when, and a Withdraw that
//! the daemon carries out (`exception.withdraw`). Nothing is drawn while there is none; a row
//! unfolds when an exception is given and folds away when it ends, withdrawn or with its session.

use gpui_design_system::{
    Button, Icon, Item, Listing, Palette, Type, icon, palette, presence, space,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div,
    px,
};
use serde_json::Value;

use crate::runs::rail_label;
use crate::session::{SessionPage, said_at};

/// An exception the session holds, as `exception.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct ExceptionRow {
    /// The session that holds it: the root, or one of its sub-agents.
    pub session: String,
    /// That session's title, when it has one.
    pub title: Option<String>,
    pub rule: String,
    /// The agent's reason, as the user read it on the card.
    pub why: String,
    pub granted_at: u64,
    /// What it covers, as the daemon gives it (`{"repository"?, "branch"?, "pull"?, "head"?}`).
    pub scope: Value,
}

impl ExceptionRow {
    pub fn of(value: &Value) -> ExceptionRow {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        ExceptionRow {
            session: text("session"),
            title: value["title"].as_str().map(str::to_owned),
            rule: text("rule"),
            why: text("why"),
            granted_at: value["granted_at"].as_u64().unwrap_or(0),
            scope: value["scope"].clone(),
        }
    }

    /// What it covers, in words: `for the branch app/w`, `for pull request #12 of app/login at
    /// 1111111`; `forge-state` has no scope, and says so.
    pub fn covers(&self) -> String {
        let scope = &self.scope;
        let branch = scope["branch"].as_str().unwrap_or_default();
        let repository = scope["repository"]
            .as_str()
            .filter(|r| *r != "." && !r.is_empty());
        let of = repository.map_or(String::new(), |repository| format!(" of {repository}"));
        match (scope["pull"].as_u64(), branch) {
            (Some(pull), branch) => {
                let at = scope["head"].as_str().map_or(String::new(), |head| {
                    format!(" at {}", head.chars().take(7).collect::<String>())
                });
                format!("for pull request #{pull} of {branch}{at}")
            }
            (None, "") => "for any issue or pull request: no scope".to_owned(),
            (None, branch) => format!("for the branch {branch}{of}"),
        }
    }

    /// What it lets the session do, as a title: the rule's name when the window does not know it.
    pub fn said(&self) -> String {
        match self.rule.as_str() {
            "force-push" => "Force-push its own branch".into(),
            "merge-blocked" => "Merge past failing checks or a branch protection".into(),
            "forge-state" => "Close, reopen or relabel issues and pull requests".into(),
            rule => rule.to_owned(),
        }
    }

    /// Its key in the list and in the ids of its row: one rule per session.
    pub fn key(&self) -> String {
        format!("{}-{}", self.session, self.rule)
    }

    /// Who holds it, said for a sub-agent's: `held by worker`.
    pub fn holder(&self) -> String {
        match &self.title {
            Some(title) => format!("held by {title}"),
            None => format!("held by agent {}", self.session),
        }
    }

    /// `allowed 3 Nov, 14:05`.
    pub fn since(&self) -> String {
        format!("allowed {}", said_at(self.granted_at))
    }
}

/// The rail's Exceptions section.
pub struct ExceptionList {
    page: WeakEntity<SessionPage>,
    /// The session's root: its tree's exceptions are listed, a sub-agent's with its holder.
    root: String,
    rows: Vec<ExceptionRow>,
    /// The rows as drawn: one that ends folds away before it is gone.
    list: Listing<ExceptionRow>,
}

impl ExceptionList {
    pub fn new(page: WeakEntity<SessionPage>, root: String) -> Self {
        Self {
            page,
            root,
            rows: Vec::new(),
            list: Listing::default(),
        }
    }

    pub fn rows(&self) -> &[ExceptionRow] {
        &self.rows
    }

    /// What the daemon listed; nothing is drawn again when it is what was drawn.
    pub fn set(&mut self, rows: Vec<ExceptionRow>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }

    /// One exception: what it lets, why it was asked, since when, and its Withdraw.
    fn row(&self, row: &ExceptionRow, p: Palette) -> AnyElement {
        let key = row.key();
        let id = |part: &str| ElementId::Name(format!("exception-{part}-{key}").into());
        let withdraw = {
            let (page, agent, rule) = (self.page.clone(), row.session.clone(), row.rule.clone());
            Button::icon(id("withdraw"), Icon::Close, "Withdraw").on_click(move |_, _, cx| {
                page.update(cx, |page, cx| page.withdraw_exception(&agent, &rule, cx))
                    .ok();
            })
        };
        let said = row.said();
        let covers = row.covers();
        let why = row.why.clone();
        let since = row.since();
        let line = |part: &str, text: String, size: f32| {
            div()
                .id(id(part))
                .text_size(px(size))
                .text_color(p.faint)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .aria_label(SharedString::from(text.clone()))
                .child(SharedString::from(text))
                .test_support()
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
                    .child(icon(Icon::Warning).size(px(14.)).text_color(p.faint)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(id("said"))
                            .text_size(px(13.))
                            .medium()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .aria_label(SharedString::from(said.clone()))
                            .child(SharedString::from(said))
                            .test_support(),
                    )
                    .child(line("covers", covers, 12.))
                    .child(line("why", why, 12.))
                    .when(row.session != self.root, |held| {
                        held.child(line("holder", row.holder(), 11.5))
                    })
                    .child(line("since", since, 11.5)),
            )
            .child(withdraw)
            .into_any_element()
    }
}

impl Render for ExceptionList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries: Vec<(String, ExceptionRow)> = self
            .rows
            .iter()
            .map(|row| (row.key(), row.clone()))
            .collect();
        let at = cx.background_executor().now();
        self.list.sync(entries, at, cx.reduce_motion());
        let p = palette(cx);
        let present = !self.list.is_empty();
        let listed: Vec<(String, ExceptionRow, gpui_design_system::Placement)> = self
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
                Item::new(
                    ElementId::Name(format!("exception-item-{key}").into()),
                    placement,
                    self.row(&row, p),
                )
                .into_any_element()
            })
            .collect();
        let section = div()
            .id("rail-exceptions")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .pb(space::MD)
            .child(
                div()
                    .id("rail-exceptions-title")
                    .aria_label("Exceptions")
                    .child(rail_label("Exceptions", None, cx))
                    .test_support(),
            )
            .children(rows)
            .test_support();
        presence("rail-exceptions-presence", present, section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_exception_says_what_it_lets_the_session_do() {
        let row = |rule: &str| {
            ExceptionRow::of(
                &json!({"session": "a1", "rule": rule, "said": "x", "why": "w",
                "request": 3, "granted_at": 0}),
            )
        };
        assert_eq!(row("force-push").said(), "Force-push its own branch");
        assert_eq!(
            row("merge-blocked").said(),
            "Merge past failing checks or a branch protection"
        );
        assert_eq!(
            row("forge-state").said(),
            "Close, reopen or relabel issues and pull requests"
        );
        assert_eq!(row("later-rule").said(), "later-rule");
        assert_eq!(row("force-push").why, "w");
        assert_eq!(row("force-push").key(), "a1-force-push");
        assert_eq!(row("force-push").holder(), "held by agent a1");
        let titled = ExceptionRow::of(&json!({"session": "a2", "title": "worker",
            "rule": "force-push", "why": "w", "granted_at": 0}));
        assert_eq!(titled.holder(), "held by worker");
    }

    #[test]
    fn an_exception_says_which_branch_or_pull_request_it_covers() {
        let row = |rule: &str, scope: serde_json::Value| {
            ExceptionRow::of(&json!({"session": "a1", "rule": rule, "why": "w",
                "granted_at": 0, "scope": scope}))
        };
        assert_eq!(
            row("force-push", json!({"repository": ".", "branch": "app/w"})).covers(),
            "for the branch app/w"
        );
        assert_eq!(
            row(
                "force-push",
                json!({"repository": "api", "branch": "app/w"})
            )
            .covers(),
            "for the branch app/w of api"
        );
        assert_eq!(
            row(
                "merge-blocked",
                json!({"repository": ".", "branch": "app/login", "pull": 12,
                    "head": "1111111111111111111111111111111111111111"})
            )
            .covers(),
            "for pull request #12 of app/login at 1111111"
        );
        assert_eq!(
            row("forge-state", json!({})).covers(),
            "for any issue or pull request: no scope"
        );
    }
}
