//! The main area: the page of what is open. The project page is [`ProjectPage`]; the session page
//! (#13) plugs in here, and until then shows its header and its rail of agents, which becomes a
//! drawer from the right in a narrower window.

use flottille_ui::{
    AgentState, Badge, Button, Drawer, Icon, ListRow, Pill, Side, StateIcon, Status, Type, icon,
    palette, section_label,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, px,
};

use crate::composer::Composer;
use crate::model::Place;
use crate::project::{NewWorkspace, ProjectPage};
use crate::shell::{Layout, Shell};
use crate::tree::place_name;

/// Width of the session page's right rail.
pub const RAIL: f32 = 290.;

#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    Empty,
    Project {
        name: String,
    },
    Session {
        id: String,
        title: String,
        place: Place,
        /// The model the session's row reads.
        status: Entity<Status>,
    },
}

pub struct MainArea {
    page: Page,
    layout: Layout,
    shell: WeakEntity<Shell>,
    /// The open project's page.
    project: Option<Entity<ProjectPage>>,
    pill: Option<Entity<Pill>>,
    rail: Entity<Rail>,
    drawer: Entity<Drawer>,
    /// The open session's state, followed for the agents button's "!".
    _status: Option<Subscription>,
}

impl MainArea {
    pub fn new(shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        let rail = cx.new(|_| Rail { lead: None });
        let drawer = cx.new(|_| Drawer::new("rail-drawer", Side::Right, px(RAIL), rail.clone()));
        Self {
            page: Page::Empty,
            layout: Layout::Wide,
            shell,
            project: None,
            pill: None,
            rail,
            drawer,
            _status: None,
        }
    }

    pub fn page(&self) -> &Page {
        &self.page
    }

    pub fn drawer(&self) -> &Entity<Drawer> {
        &self.drawer
    }

    pub fn project_page(&self) -> Option<&Entity<ProjectPage>> {
        self.project.as_ref()
    }

    /// Shows `page`; a project's page gets that project's composer and new workspace form.
    pub fn show(
        &mut self,
        page: Page,
        forms: Option<(Entity<Composer>, Entity<NewWorkspace>)>,
        cx: &mut Context<Self>,
    ) {
        // Another project's page is a new one: its content enters, the old one is gone.
        self.project = match (&page, forms) {
            (Page::Project { name }, Some((composer, new_workspace))) => {
                match self.project.take() {
                    Some(open) if open.read(cx).name() == name => Some(open),
                    _ => {
                        let (shell, name) = (self.shell.clone(), name.clone());
                        let narrow = self.layout == Layout::Narrow;
                        Some(cx.new(|cx| {
                            let mut page = ProjectPage::new(shell, name, composer, new_workspace);
                            page.set_narrow(narrow, cx);
                            page
                        }))
                    }
                }
            }
            _ => None,
        };
        self.pill = match &page {
            Page::Session { status, .. } => {
                let status = status.clone();
                Some(cx.new(|cx| Pill::new(status, cx)))
            }
            _ => None,
        };
        self._status = match &page {
            Page::Session { status, .. } => Some(cx.observe(status, |_, _, cx| cx.notify())),
            _ => None,
        };
        let lead = match &page {
            Page::Session { status, .. } => {
                let status = status.clone();
                Some(cx.new(|cx| StateIcon::new(status, cx)))
            }
            _ => None,
        };
        self.rail.update(cx, |rail, cx| {
            rail.lead = lead;
            cx.notify();
        });
        self.drawer.update(cx, |drawer, cx| drawer.close(cx));
        self.page = page;
        cx.notify();
    }

    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if self.layout != layout {
            self.layout = layout;
            if layout == Layout::Wide {
                self.drawer.update(cx, |drawer, cx| drawer.close(cx));
            }
            if let Some(page) = &self.project {
                page.update(cx, |page, cx| page.set_narrow(layout == Layout::Narrow, cx));
            }
            cx.notify();
        }
    }
}

impl Render for MainArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let narrow = self.layout == Layout::Narrow;
        let root = div()
            .id("main")
            .size_full()
            .min_w_0()
            .relative()
            .bg(p.bg)
            .ui_text()
            .text_color(p.text);
        match &self.page {
            Page::Empty => root.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.faint)
                    .child("No project yet: add one with flottille project add."),
            ),
            Page::Project { .. } => root.children(self.project.clone()),
            Page::Session {
                id,
                title,
                place,
                status,
            } => {
                let wide = self.layout == Layout::Wide;
                let drawer = self.drawer.clone();
                let needs_you = status.read(cx).state() == AgentState::NeedsYou;
                let header = div()
                    .flex_none()
                    .h(px(44.))
                    .px(px(if narrow { 10. } else { 24. }))
                    .pr(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(p.line)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(14.))
                            .semibold()
                            .child(SharedString::from(title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_color(p.faint)
                            .child(
                                icon(match place {
                                    None => Icon::Folder,
                                    Some(_) => Icon::Branch,
                                })
                                .size(px(13.))
                                .text_color(p.faint),
                            )
                            .child(
                                div()
                                    .mono(12.)
                                    .child(SharedString::from(place_name(place).to_owned())),
                            ),
                    )
                    .children(self.pill.clone())
                    .child(div().flex_1())
                    .when(!wide, |d| {
                        let mut agents = Button::icon("open-rail", Icon::Agents, "Agents")
                            .on_click(move |_, _, cx| drawer.update(cx, |d, cx| d.open(cx)));
                        if needs_you {
                            agents = agents.badge(Badge::alert());
                        }
                        d.child(agents)
                    });
                let thread = div()
                    .id(SharedString::from(format!("thread-{id}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .max_w(px(780.))
                            .w_full()
                            .mx_auto()
                            .px(px(if narrow { 16. } else { 24. }))
                            .py(px(if narrow { 20. } else { 28. }))
                            .text_color(p.faint)
                            .child("The conversation of this session comes here."),
                    );
                let column = div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(thread);
                root.child(div().size_full().flex().child(column).when(wide, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .w(px(RAIL))
                            .h_full()
                            .bg(p.side)
                            .border_l_1()
                            .border_color(p.line)
                            .child(self.rail.clone()),
                    )
                }))
                .when(!wide, |d| d.child(self.drawer.clone()))
            }
        }
    }
}

/// The session's rail: its agents (#13 fills it with the tree).
pub struct Rail {
    lead: Option<Entity<StateIcon>>,
}

impl Render for Rail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(12.))
            .flex()
            .flex_col()
            .child(section_label("Agents", cx))
            .when_some(self.lead.clone(), |d, lead| {
                d.child(
                    ListRow::new("rail-lead", "lead")
                        .leading(lead)
                        .selected(true),
                )
            })
    }
}
