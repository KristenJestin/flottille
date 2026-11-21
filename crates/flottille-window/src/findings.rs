//! The findings in the Developer section of Settings (`docs/developer.md`, "The findings"): what
//! the agents reported about Flottille itself, one row each with its count, its first and last
//! time and whether it is filed. The daemon says them (`developer.reports`); an export writes
//! them to a file for the lead agent.

use gpui_design_system::{Icon, ListRow, icon, palette, space};
use gpui_kit::{
    App, ElementId, InteractiveElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, TestSupportExt as _, div, px,
};
use serde_json::Value;

use crate::session::said_at;

/// A finding as the window lists it: its occurrences stay in the export.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub id: u64,
    pub title: String,
    pub count: usize,
    pub first_at: u64,
    pub last_at: u64,
    /// The address of the issue it became, once filed.
    pub filed: Option<String>,
}

impl Finding {
    /// The findings `developer.reports` answers, the newest first.
    pub fn all(said: &Value) -> Vec<Finding> {
        let mut all: Vec<Finding> = said["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|finding| {
                Some(Finding {
                    id: finding["id"].as_u64()?,
                    title: finding["title"].as_str()?.to_owned(),
                    count: finding["count"].as_u64()? as usize,
                    first_at: finding["first_at"].as_u64().unwrap_or_default(),
                    last_at: finding["last_at"].as_u64().unwrap_or_default(),
                    filed: finding["filed"].as_str().map(str::to_owned),
                })
            })
            .collect();
        all.sort_by_key(|finding| std::cmp::Reverse(finding.id));
        all
    }
}

/// `1 occurrence`, `2 occurrences`.
pub fn count_said(count: usize) -> String {
    match count {
        1 => "1 occurrence".to_owned(),
        count => format!("{count} occurrences"),
    }
}

/// One finding's row: its title, then each fact under a label.
pub fn row(finding: &Finding, cx: &App) -> ListRow {
    let p = palette(cx);
    let id = finding.id;
    let said = |suffix: &str, text: String| {
        div()
            .id(ElementId::Name(format!("finding-{id}-{suffix}").into()))
            .aria_label(SharedString::from(text.clone()))
            .child(text)
            .test_support()
    };
    let filed = match &finding.filed {
        Some(issue) => format!("Filed: {issue}"),
        None => "Not filed".to_owned(),
    };
    let facts = div()
        .flex()
        .flex_wrap()
        .gap_x(space::MD)
        .text_size(px(12.5))
        .text_color(p.mute)
        .child(said("count", count_said(finding.count)))
        .child(said(
            "first",
            format!("First: {}", said_at(finding.first_at)),
        ))
        .child(said("last", format!("Last: {}", said_at(finding.last_at))))
        .child(said("filed", filed));
    ListRow::new(
        ElementId::Name(format!("finding-{id}").into()),
        SharedString::from(finding.title.clone()),
    )
    .leading(icon(Icon::Warning).size(px(14.)).text_color(p.faint))
    .below(facts)
}
