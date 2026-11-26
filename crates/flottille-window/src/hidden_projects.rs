//! Settings, General: the projects this machine hides (`docs/design.md` §10, the sidebar; #1526).
//! One list of the one pattern (`ListCard`), each row with a Show that brings the project back:
//! a row unfolds when a project is hidden and folds away when it is shown, and an empty list is
//! one line that says what it is for.

use gpui_design_system::{
    Button, EmptyLine, ListCard, ListRow, Listing, Section, icon, palette, presence,
};
use gpui_kit::{
    Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    Styled, TestSupportExt as _, Window, div, px,
};

use crate::commands::ShellOf;
use crate::glyphs;

/// What the section is for, in one sentence.
pub const SENTENCE: &str = "Left out of this machine's sidebar and lists, on this machine only. \
    Their sessions keep running, and a session that needs you still rings.";

/// The line of an empty list.
pub const EMPTY: &str = "No project is hidden. Hide one from the menu of its row in the sidebar.";

pub struct HiddenProjects {
    shell: ShellOf,
    names: Vec<String>,
    /// The rows as drawn: one that is shown folds away before it is gone.
    rows: Listing<String>,
}

impl HiddenProjects {
    pub fn new(shell: ShellOf) -> Self {
        Self {
            shell,
            names: Vec::new(),
            rows: Listing::default(),
        }
    }

    /// The names of the rows this machine hides, in the order they were hidden.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// What the window says this machine hides; nothing is drawn again when it is what was drawn.
    pub fn set(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        if self.names != names {
            self.names = names;
            cx.notify();
        }
    }

    fn show(&self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.shell
            .update(cx, |shell, cx| shell.show_project(&name, window, cx))
            .ok();
    }
}

impl Render for HiddenProjects {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.rows.sync(
            self.names.iter().map(|name| (name.clone(), name.clone())),
            now,
            reduce,
        );
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("hidden-projects-list");
        for (key, placement, name) in drawn {
            let show = {
                let name = name.clone();
                Button::secondary(
                    ElementId::Name(format!("hidden-project-show-{key}").into()),
                    "Show",
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.show(&name, window, cx)
                }))
            };
            let row = ListRow::new(
                ElementId::Name(format!("hidden-project-row-{key}").into()),
                SharedString::from(name),
            )
            .leading(
                div()
                    .id(ElementId::Name(format!("hidden-project-mark-{key}").into()))
                    .child(icon(glyphs::HIDE).size(px(14.)).text_color(p.faint))
                    .test_support(),
            )
            .trailing(show);
            list = list.item(
                ElementId::Name(format!("hidden-project-item-{key}").into()),
                placement,
                row,
            );
        }
        let section = Section::new("hidden-projects", "Hidden projects")
            .count(self.names.len().to_string())
            .sentence(SENTENCE)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(presence(
                        "hidden-projects-list-block",
                        !self.rows.is_empty(),
                        list,
                    ))
                    .child(presence(
                        "hidden-projects-empty-block",
                        self.rows.is_empty(),
                        EmptyLine::new("hidden-projects-empty", glyphs::HIDE, EMPTY),
                    )),
            );
        div().text_color(p.text).child(section)
    }
}
