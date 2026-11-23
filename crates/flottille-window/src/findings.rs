//! The findings in the Developer section of Settings (`docs/developer.md`, "The findings"): what
//! the agents reported about Flottille itself, one row each with its count, its first and last
//! time and whether it is filed. The daemon says them (`developer.reports`); an export writes
//! them to a file for the lead agent.

use gpui_design_system::{
    Button, Field, Icon, ListRow, Menu, Modal, Toast, icon, palette, space, toast,
};
use gpui_kit::base::input::InputState;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::commands::{perform_later, problem_line};
use crate::extensions::Then as Asked;
use crate::project_settings::field_frame;
use crate::session::said_at;
use crate::shell::Shell;

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
    /// The findings `developer.reports` answers, the open ones first, the newest first.
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
        // The open ones first, as `flottille developer reports` lists them, the newest first.
        all.sort_by_key(|finding| (finding.filed.is_some(), std::cmp::Reverse(finding.id)));
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

/// One finding's row: its title, then each fact under a label, and its "⋯" while it is open.
pub fn row(finding: &Finding, menu: Option<Entity<Menu>>, cx: &App) -> ListRow {
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
    let row = ListRow::new(
        ElementId::Name(format!("finding-{id}").into()),
        SharedString::from(finding.title.clone()),
    )
    .leading(icon(Icon::Warning).size(px(14.)).text_color(p.faint))
    .below(facts);
    match menu {
        Some(menu) => row.more(menu),
        None => row,
    }
}

/// What to do with the daemon's answer to a request of the findings.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `developer.report_filed` of the finding.
    Filed { id: u64 },
}

/// The "⋯" of a finding that is not filed: **Mark filed…**, which asks where it went.
pub fn menu(id: u64, filing: WeakEntity<Filing>, cx: &mut App) -> Entity<Menu> {
    cx.new(|cx| {
        Menu::new(
            ElementId::Name(format!("finding-{id}-more").into()),
            "More",
            cx,
        )
        .entry(
            format!("finding-{id}-mark-filed"),
            "Mark filed…",
            Some(Icon::Check),
            move |window, cx| {
                filing
                    .update(cx, |filing, cx| filing.open_for(id, window, cx))
                    .ok();
            },
        )
    })
}

/// The dialog that marks a finding filed: the address of the issue it became.
pub struct Filing {
    shell: WeakEntity<Shell>,
    /// The finding the dialog is open on.
    finding: Option<u64>,
    open: bool,
    address: Entity<InputState>,
    /// Why the address was not taken, under its field.
    error: Option<SharedString>,
    saving: bool,
}

impl Filing {
    pub fn new(shell: WeakEntity<Shell>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| {
            crate::project_settings::styled_input(
                "https://example.com/acme/app/issues/12",
                window,
                cx,
            )
        });
        Self {
            shell,
            finding: None,
            open: false,
            address,
            error: None,
            saving: false,
        }
    }

    /// The field that takes the address.
    pub fn address(&self) -> &Entity<InputState> {
        &self.address
    }

    /// Opens the dialog on finding `id`, its field empty and focused.
    pub fn open_for(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.finding = Some(id);
        self.open = true;
        self.error = None;
        self.saving = false;
        self.address
            .update(cx, |field, cx| field.set_value("", window, cx));
        let focus = self.address.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.finding.filter(|_| !self.saving) else {
            return;
        };
        let address = self.address.read(cx).value().trim().to_owned();
        if address.is_empty() {
            self.error = Some("Say the address of the issue it became.".into());
            return cx.notify();
        }
        self.saving = true;
        self.error = None;
        perform_later(
            &self.shell,
            crate::actions::Command::Ask {
                method: "developer.report_filed".to_owned(),
                params: json!({"id": id, "issue": address}),
                then: Asked::Findings(Then::Filed { id }),
            },
            cx,
        );
        cx.notify();
    }

    /// The daemon's answer: the dialog closes with a toast, or says why under its field.
    pub fn answered(&mut self, id: u64, answer: Result<Value, String>, cx: &mut Context<Self>) {
        if self.finding != Some(id) {
            return;
        }
        self.saving = false;
        match answer {
            Ok(_) => {
                self.open = false;
                toast(Toast::success(format!("Finding #{id} is marked filed")), cx);
            }
            Err(why) => self.error = Some(why.into()),
        }
        cx.notify();
    }
}

impl Render for Filing {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let id = self.finding.unwrap_or_default();
        let field = Field::new(
            "finding-filed-address-field",
            "Address of its issue",
            field_frame("finding-filed-address", &self.address, cx),
        )
        .hint("It leaves the findings the agents search, and the list says where it went.");
        let error = problem_line("finding-filed-error", self.error.clone(), px(0.), cx);
        Modal::new("finding-filed-modal", format!("Mark finding #{id} filed"))
            .sentence("Say where its issue is: it is not offered to agents again.")
            .open(self.open)
            .on_close({
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, |this, cx| this.close(cx)).ok();
                }
            })
            .action(
                Button::primary("finding-filed-save", "Mark filed")
                    .disabled(self.saving)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.save(cx)).ok();
                    }),
            )
            .child(field)
            .kept(error)
    }
}
