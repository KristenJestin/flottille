//! The rail's Write access section, where the session's rights over other sessions are shown:
//! each leave the user gave for one session to write to another it did not start
//! (`session_send`, `docs/tools.md`, "Sessions that talk to each other"), the session's own to
//! write and the others' to write to it, each with a Withdraw that the daemon carries out
//! (`leave.withdraw`). Nothing is drawn while there is none; a row unfolds when a leave is given
//! and folds away when it ends, withdrawn or with either session.

use flottille_ui::{Button, Icon, Item, Listing, Palette, Type, icon, palette, presence, space};
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div,
    px,
};
use serde_json::Value;

use crate::runs::rail_label;
use crate::session::{SessionPage, said_at};

/// A leave that names the session, as `leave.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct LeaveRow {
    pub writer: String,
    pub writer_name: String,
    pub session: String,
    pub session_name: String,
    pub granted_at: u64,
}

/// A session as a row names it: its title, or its agent's id.
fn name(session: &Value) -> String {
    match session["title"].as_str() {
        Some(title) => title.to_owned(),
        None => format!("agent {}", session["id"].as_str().unwrap_or_default()),
    }
}

impl LeaveRow {
    pub fn of(value: &Value) -> LeaveRow {
        let id = |session: &Value| session["id"].as_str().unwrap_or_default().to_owned();
        LeaveRow {
            writer: id(&value["writer"]),
            writer_name: name(&value["writer"]),
            session: id(&value["session"]),
            session_name: name(&value["session"]),
            granted_at: value["granted_at"].as_u64().unwrap_or(0),
        }
    }

    fn key(&self) -> String {
        format!("{}-{}", self.writer, self.session)
    }

    /// What it lets, seen from the session `root`: `May write to Login`, `Login may write here`.
    pub fn said(&self, root: &str) -> String {
        match self.writer == root {
            true => format!("May write to {}", self.session_name),
            false => format!("{} may write here", self.writer_name),
        }
    }

    /// `allowed 3 Nov, 14:05`.
    pub fn since(&self) -> String {
        format!("allowed {}", said_at(self.granted_at))
    }
}

/// The rail's Write access section.
pub struct LeaveList {
    page: WeakEntity<SessionPage>,
    /// The session's root, whose side each row is said from.
    root: String,
    rows: Vec<LeaveRow>,
    /// The rows as drawn: one that ends folds away before it is gone.
    list: Listing<LeaveRow>,
}

impl LeaveList {
    pub fn new(page: WeakEntity<SessionPage>, root: String) -> Self {
        Self {
            page,
            root,
            rows: Vec::new(),
            list: Listing::default(),
        }
    }

    pub fn rows(&self) -> &[LeaveRow] {
        &self.rows
    }

    /// What the daemon listed; nothing is drawn again when it is what was drawn.
    pub fn set(&mut self, rows: Vec<LeaveRow>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }

    /// One leave: what it lets, since when, and its Withdraw.
    fn row(&self, row: &LeaveRow, p: Palette) -> AnyElement {
        let key = row.key();
        let id = |part: &str| ElementId::Name(format!("leave-{part}-{key}").into());
        let withdraw = {
            let (page, writer, session) =
                (self.page.clone(), row.writer.clone(), row.session.clone());
            Button::icon(id("withdraw"), Icon::Close, "Withdraw").on_click(move |_, _, cx| {
                page.update(cx, |page, cx| page.withdraw_leave(&writer, &session, cx))
                    .ok();
            })
        };
        let said = row.said(&self.root);
        let since = row.since();
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
                    .child(icon(Icon::Check).size(px(14.)).text_color(p.faint)),
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
                    .child(
                        div()
                            .id(id("since"))
                            .text_size(px(11.5))
                            .text_color(p.faint)
                            .aria_label(SharedString::from(since.clone()))
                            .child(SharedString::from(since))
                            .test_support(),
                    ),
            )
            .child(withdraw)
            .into_any_element()
    }
}

impl Render for LeaveList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries: Vec<(String, LeaveRow)> = self
            .rows
            .iter()
            .map(|row| (row.key(), row.clone()))
            .collect();
        let at = cx.background_executor().now();
        self.list.sync(entries, at, cx.reduce_motion());
        let p = palette(cx);
        let present = !self.list.is_empty();
        let listed: Vec<(String, LeaveRow, flottille_ui::Placement)> = self
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
                    ElementId::Name(format!("leave-item-{key}").into()),
                    placement,
                    self.row(&row, p),
                )
                .into_any_element()
            })
            .collect();
        let section = div()
            .id("rail-leaves")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .pb(space::MD)
            .child(
                div()
                    .id("rail-leaves-title")
                    .aria_label("Write access")
                    .child(rail_label("Write access", None, cx))
                    .test_support(),
            )
            .children(rows)
            .test_support();
        presence("rail-leaves-presence", present, section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row() -> LeaveRow {
        LeaveRow::of(&json!({
            "writer": {"id": "a1", "title": "Lead", "state": "idle"},
            "session": {"id": "x1", "title": null, "state": "idle"},
            "request": 3, "granted_at": 0,
        }))
    }

    #[test]
    fn a_leave_is_said_from_the_side_of_the_session_shown() {
        assert_eq!(row().said("a1"), "May write to agent x1");
        assert_eq!(row().said("x1"), "Lead may write here");
    }
}
