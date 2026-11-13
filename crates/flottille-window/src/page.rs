//! The main area: the page of what is open, a project's ([`ProjectPage`]) or a session's
//! ([`SessionPage`]).

use crate::state::Status;
use gpui_design_system::{Type, palette, presence_of, space};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, Window, div,
};

use crate::composer::Composer;
use crate::extensions::specs::SpecPage;
use crate::model::Place;
use crate::project::{NewWorkspace, ProjectPage};
use crate::project_settings::ProjectSettingsPage;
use crate::services::ServicesPage;
use crate::session::SessionPage;
use crate::settings::SettingsPage;
use crate::shell::{Layout, Shell};

/// What the main area says while Flottille does not answer.
const TITLE: &str = "Flottille is not answering";

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
    /// A project's own settings: its forges and tracker.
    ProjectSettings {
        name: String,
    },
    /// A spec's page.
    Spec {
        project: String,
        spec: u64,
    },
    /// The services of the machine.
    Services,
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
    /// The open project's settings page.
    project_settings: Option<Entity<ProjectSettingsPage>>,
    /// The open spec's page.
    spec: Option<Entity<SpecPage>>,
    /// The services page, one for the window's life.
    services: Entity<ServicesPage>,
    /// Since when Flottille has not answered the window, in ms after the epoch: said in a banner
    /// above the page while it lasts.
    not_answering: Option<u64>,
    /// Whether "Restart Flottille" runs: it cannot be clicked again meanwhile.
    restarting: bool,
    /// Whether Flottille was read once: until then, an empty page says nothing of the projects.
    read: bool,
}

impl MainArea {
    pub fn new(
        shell: WeakEntity<Shell>,
        settings: Entity<SettingsPage>,
        services: Entity<ServicesPage>,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            page: Page::Empty,
            layout: Layout::Wide,
            shell,
            project: None,
            session: None,
            settings,
            project_settings: None,
            spec: None,
            services,
            not_answering: None,
            restarting: false,
            read: false,
        }
    }

    pub fn set_not_answering(&mut self, since: Option<u64>, cx: &mut Context<Self>) {
        if self.not_answering != since {
            self.not_answering = since;
            cx.notify();
        }
    }

    pub fn restarting(&self) -> bool {
        self.restarting
    }

    pub fn set_restarting(&mut self, restarting: bool, cx: &mut Context<Self>) {
        if self.restarting != restarting {
            self.restarting = restarting;
            cx.notify();
        }
    }

    /// Flottille was read: it answers, and an empty page now says there is no project.
    pub fn set_read(&mut self, cx: &mut Context<Self>) {
        if !self.read || self.not_answering.is_some() {
            self.read = true;
            self.not_answering = None;
            self.restarting = false;
            cx.notify();
        }
    }

    /// The spec's page to show, when the next page shown is a spec's.
    pub fn set_spec_page(&mut self, spec: Option<Entity<SpecPage>>) {
        self.spec = spec;
    }

    /// The spec's page, when it is the one shown.
    pub fn spec_page(&self) -> Option<&Entity<SpecPage>> {
        match self.page {
            Page::Spec { .. } => self.spec.as_ref(),
            _ => None,
        }
    }

    /// The services page, when it is the one shown.
    pub fn services_page(&self) -> Option<&Entity<ServicesPage>> {
        (self.page == Page::Services).then_some(&self.services)
    }

    /// The settings page, when it is the one shown.
    pub fn settings_page(&self) -> Option<&Entity<SettingsPage>> {
        (self.page == Page::Settings).then_some(&self.settings)
    }

    pub fn page(&self) -> &Page {
        &self.page
    }

    /// The project's settings page, when it is the one shown.
    pub fn project_settings_page(&self) -> Option<&Entity<ProjectSettingsPage>> {
        self.project_settings.as_ref()
    }

    pub fn project_page(&self) -> Option<&Entity<ProjectPage>> {
        self.project.as_ref()
    }

    pub fn session_page(&self) -> Option<&Entity<SessionPage>> {
        self.session.as_ref()
    }

    /// Shows `page`; a project's page gets that project's composer and new workspace form, a
    /// session's page and a project's settings page are the ones given.
    pub fn show(
        &mut self,
        page: Page,
        forms: Option<(Entity<Composer>, Entity<NewWorkspace>)>,
        session: Option<Entity<SessionPage>>,
        project_settings: Option<Entity<ProjectSettingsPage>>,
        cx: &mut Context<Self>,
    ) {
        self.project_settings = match &page {
            Page::ProjectSettings { .. } => project_settings,
            _ => None,
        };
        if let Some(settings) = &self.project_settings {
            let narrow = self.layout == Layout::Narrow;
            settings.update(cx, |page, cx| page.set_narrow(narrow, cx));
        }
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
        if let Some(spec) = &self.spec {
            let narrow = self.layout == Layout::Narrow;
            spec.update(cx, |page, cx| page.set_narrow(narrow, cx));
        }
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
            self.services
                .update(cx, |page, cx| page.set_narrow(layout == Layout::Narrow, cx));
            if let Some(page) = &self.project_settings {
                page.update(cx, |page, cx| page.set_narrow(layout == Layout::Narrow, cx));
            }
            if let Some(page) = &self.spec {
                page.update(cx, |page, cx| page.set_narrow(layout == Layout::Narrow, cx));
            }
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
        let page = div().flex_1().min_h_0().relative();
        let page = match &self.page {
            // Nothing read yet: what there is is not known.
            Page::Empty if !self.read => page,
            Page::Empty => page.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.faint)
                    .flex_col()
                    .gap(space::MD)
                    .child("No project yet.")
                    .child({
                        let shell = self.shell.clone();
                        gpui_design_system::Button::primary("add-first-project", "Add a project")
                            .leading(gpui_design_system::Icon::Plus)
                            .on_click(move |_, window, cx| {
                                shell
                                    .update(cx, |shell, cx| shell.add_project(window, cx))
                                    .ok();
                            })
                    }),
            ),
            Page::Project { .. } => page.children(self.project.clone()),
            Page::Session { .. } => page.children(self.session.clone()),
            Page::Settings => page.child(self.settings.clone()),
            Page::Services => page.child(self.services.clone()),
            Page::ProjectSettings { .. } => page.children(self.project_settings.clone()),
            Page::Spec { .. } => page.children(self.spec.clone()),
        };
        root.flex()
            .flex_col()
            .child(self.not_answering_banner(cx))
            .child(page)
    }
}

impl MainArea {
    /// The banner above the page while Flottille does not answer: it unfolds and fades in, and
    /// folds away and fades out, with its last words, once it answers again.
    fn not_answering_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let said = self.not_answering.map(|since| {
            format!(
                "It has not answered since {}. Restarting it cuts the turns under way.",
                crate::session::clock(since)
            )
            .into()
        });
        let (shell, restarting) = (self.shell.clone(), self.restarting);
        presence_of("not-answering", said, move |said| {
            div()
                .id("not-answering-banner")
                .flex()
                .items_center()
                .gap(space::MD)
                .px(space::LG)
                .py(space::MD)
                .bg(p.wait_bg)
                .border_b_1()
                .border_color(p.wait_line)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("not-answering-title")
                                .aria_label(TITLE)
                                .text_color(p.text)
                                .child(TITLE)
                                .test_support(),
                        )
                        .child(
                            div()
                                .id("not-answering-since")
                                .aria_label(said.clone())
                                .text_color(p.mute)
                                .child(said)
                                .test_support(),
                        ),
                )
                .child(
                    gpui_design_system::Button::primary("restart-flottille", "Restart Flottille")
                        .label(if restarting {
                            "Restarting…"
                        } else {
                            "Restart Flottille"
                        })
                        .disabled(restarting)
                        .on_click(move |_, _, cx| {
                            shell.update(cx, |shell, cx| shell.restart(cx)).ok();
                        }),
                )
                .test_support()
                .into_any_element()
        })
    }
}
