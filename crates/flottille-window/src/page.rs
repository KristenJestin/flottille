//! The main area: the page of what is open, a project's ([`ProjectPage`]) or a session's
//! ([`SessionPage`]).

use flottille_ui::{Status, Type, palette};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, WeakEntity, Window, div,
};

use crate::composer::Composer;
use crate::model::Place;
use crate::project::{NewWorkspace, ProjectPage};
use crate::session::SessionPage;
use crate::settings::SettingsPage;
use crate::shell::{Layout, Shell};

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
    Settings,
}

pub struct MainArea {
    page: Page,
    layout: Layout,
    shell: WeakEntity<Shell>,
    /// The open project's page.
    project: Option<Entity<ProjectPage>>,
    /// The open session's page.
    session: Option<Entity<SessionPage>>,
    /// The settings page, one for the window's life: it follows the providers while closed too.
    settings: Entity<SettingsPage>,
}

impl MainArea {
    pub fn new(
        shell: WeakEntity<Shell>,
        settings: Entity<SettingsPage>,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            page: Page::Empty,
            layout: Layout::Wide,
            shell,
            project: None,
            session: None,
            settings,
        }
    }

    /// The settings page, when it is the one shown.
    pub fn settings_page(&self) -> Option<&Entity<SettingsPage>> {
        (self.page == Page::Settings).then_some(&self.settings)
    }

    pub fn page(&self) -> &Page {
        &self.page
    }

    pub fn project_page(&self) -> Option<&Entity<ProjectPage>> {
        self.project.as_ref()
    }

    pub fn session_page(&self) -> Option<&Entity<SessionPage>> {
        self.session.as_ref()
    }

    /// Shows `page`; a project's page gets that project's composer and new workspace form, a
    /// session's page is the one given.
    pub fn show(
        &mut self,
        page: Page,
        forms: Option<(Entity<Composer>, Entity<NewWorkspace>)>,
        session: Option<Entity<SessionPage>>,
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
                            let mut page =
                                ProjectPage::new(shell, name, composer, new_workspace, cx);
                            page.set_narrow(narrow, cx);
                            page
                        }))
                    }
                }
            }
            _ => None,
        };
        self.session = match &page {
            Page::Session { .. } => session,
            _ => None,
        };
        if let Some(session) = &self.session {
            let layout = self.layout;
            session.update(cx, |page, cx| page.set_layout(layout, cx));
        }
        self.page = page;
        cx.notify();
    }

    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if self.layout != layout {
            self.layout = layout;
            if let Some(page) = &self.project {
                page.update(cx, |page, cx| page.set_narrow(layout == Layout::Narrow, cx));
            }
            if let Some(page) = &self.session {
                page.update(cx, |page, cx| page.set_layout(layout, cx));
            }
            self.settings
                .update(cx, |page, cx| page.set_narrow(layout == Layout::Narrow, cx));
            cx.notify();
        }
    }
}

impl Render for MainArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
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
            Page::Session { .. } => root.children(self.session.clone()),
            Page::Settings => root.child(self.settings.clone()),
        }
    }
}
