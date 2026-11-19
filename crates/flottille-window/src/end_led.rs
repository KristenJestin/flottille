//! What the confirmation of ending a session adds when the session leads others
//! (`docs/tools.md`, "The pilot ends"): they end with it, so the one confirmation lists them, by
//! title and project, and says which of them hold work not saved. It is read from the daemon
//! (`agent.ending`) once the dialog is open, and unfolds in it when it comes.

use gpui_design_system::{LabelledLine, caption, enter, palette, presence, space};
use gpui_kit::{App, ElementId, IntoElement, ParentElement, SharedString, Styled, div, px};
use serde_json::Value;

use crate::glyphs;
use crate::project_removal::WorkspaceLoss;

/// A led session that ends with the session being ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedEnd {
    pub id: String,
    pub title: String,
    pub project: String,
    /// What its workspace holds that is not saved, in words (`uncommitted changes`); `None` for
    /// a session with nothing to lose.
    pub unsaved: Option<String>,
}

impl LedEnd {
    /// The sessions `agent.ending` answered.
    pub fn read(answer: &Value) -> Vec<Self> {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        answer["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|session| {
                let lost = WorkspaceLoss {
                    state: text(&session["state"]),
                    ..WorkspaceLoss::default()
                };
                Self {
                    id: text(&session["id"]),
                    title: session["title"]
                        .as_str()
                        .map_or_else(|| text(&session["id"]), str::to_owned),
                    project: text(&session["project"]),
                    unsaved: lost.unsaved().then(|| lost.state_said().to_owned()),
                }
            })
            .collect()
    }

    /// The line in words, for its label.
    fn said(&self) -> String {
        match &self.unsaved {
            Some(unsaved) => format!("{} in {}, {unsaved}", self.title, self.project),
            None => format!("{} in {}", self.title, self.project),
        }
    }
}

/// The button's words: `End session` while nothing is led, else how many sessions end.
pub fn end_label(agents: usize, led: &[LedEnd]) -> String {
    match (led.len(), agents) {
        (0, 0 | 1) => "End session".to_owned(),
        (0, n) => format!("End {n} agents"),
        (n, _) => format!("End {} sessions", n + 1),
    }
}

/// The block under the dialog's sentence: a line for each led session, its fold-in when the read
/// brings some and its fold-out when there are none left. `prefix` names the dialog's elements.
pub fn block(prefix: &str, led: &[LedEnd], cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let id = |part: String| ElementId::Name(format!("{prefix}-led-{part}").into());
    let lines: Vec<_> = led
        .iter()
        .enumerate()
        .map(|(at, session)| {
            let value = div()
                .flex()
                .items_center()
                .gap(space::SM)
                .child(SharedString::from(format!(
                    "{} · {}",
                    session.title, session.project
                )))
                .children(session.unsaved.clone().map(|unsaved| {
                    div()
                        .text_size(px(12.))
                        .text_color(p.fail)
                        .child(SharedString::from(unsaved))
                }));
            enter(
                id(format!("enter-{}", session.id)),
                at,
                LabelledLine::new(
                    id(session.id.clone()),
                    glyphs::AGENTS,
                    "Session",
                    session.said(),
                )
                .child(value),
            )
        })
        .collect();
    presence(
        id("block".to_owned()),
        !led.is_empty(),
        div()
            .flex()
            .flex_col()
            .child(caption(
                id("caption".to_owned()),
                "Ends with it",
                led.len().to_string(),
                cx,
            ))
            .children(lines),
    )
    .space(space::MD)
}
