//! The composer that starts a session, in two layers: an outer shell holding the tabs ("New
//! task", "Running agents"), the field with its own border and focus ring, and the pickers below
//! it (the place, the agent and model, send). The field is its own view: typing redraws it, never
//! the page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use flottille_ui::motion::{Motion, PANEL};
use flottille_ui::{Button, Icon, ListRow, StateIcon, Status, Type, enter, icon, palette};
use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Bounds, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, canvas, div, px,
};

use crate::actions::{Command, Pick, Target};
use crate::providers::{Provider, Providers};
use crate::shell::{Selection, Shell};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    NewTask,
    Running,
}

/// A place the composer can start a session in.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceChoice {
    /// `None` for `main`.
    pub workspace: Option<String>,
    /// Whether a session can start there now: a workspace being prepared, or whose preparation
    /// failed, cannot take one.
    pub ready: bool,
}

/// A session of the "Running agents" list.
#[derive(Clone, Debug, PartialEq)]
pub struct RunningSession {
    pub id: String,
    pub title: String,
    pub place: String,
    pub agents: usize,
    pub waiting: bool,
    /// The state its row in the tree reads.
    pub status: Entity<Status>,
}

/// The list a picker of the composer opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Place,
    Provider,
    Model,
    Effort,
}

/// The provider and the model a project's file gives its agents, each when it gives it.
pub type ProjectDefault<'a> = (Option<&'a str>, Option<&'a str>);

/// The provider the daemon applies the project's model to: the project's own, else the first one
/// enabled, ready or not, as the daemon picks it.
fn project_provider<'a>(providers: &'a Providers, project: ProjectDefault<'a>) -> Option<&'a str> {
    project.0.or_else(|| {
        providers
            .list
            .iter()
            .find(|p| p.enabled)
            .map(|p| p.name.as_str())
    })
}

/// The project's model, when a session started on `provider` without a model takes it.
fn project_model<'a>(
    providers: &Providers,
    project: ProjectDefault<'a>,
    provider: &str,
) -> Option<&'a str> {
    project
        .1
        .filter(|_| project_provider(providers, project) == Some(provider))
}

/// The provider, the model and the effort a new session starts on, when the user has not picked
/// one: the project's provider (its own, else the one its model goes to) with its model, else the
/// last ones used, else the first provider offered; each only when it is offered now.
pub fn first_pick(providers: &Providers, project: ProjectDefault, last: Option<&Pick>) -> Pick {
    let offered = |name: &str| providers.get(name).is_some_and(Provider::offered);
    if project != (None, None)
        && let Some(provider) = project_provider(providers, project)
        && offered(provider)
    {
        return Pick {
            provider: Some(provider.to_owned()),
            ..Pick::default()
        }
        .kept(providers, project);
    }
    if let Some(last) = last
        && last.provider.as_deref().is_some_and(offered)
    {
        return last.clone().kept(providers, project);
    }
    Pick {
        provider: providers.offered().next().map(|p| p.name.clone()),
        ..Pick::default()
    }
    .kept(providers, project)
}

impl Pick {
    /// The pick, less a model or an effort its provider is known not to offer, and with the
    /// project's model when it has none and the daemon would give it the project's. The project's
    /// model stays even when its provider does not offer it: the composer says so
    /// (`Composer::model_refused`).
    pub fn kept(mut self, providers: &Providers, project: ProjectDefault) -> Pick {
        let Some(provider) = self.provider.as_deref().and_then(|p| providers.get(p)) else {
            return Pick::default();
        };
        let projects = project_model(providers, project, &provider.name);
        if let (Some(model), Some(_)) = (&self.model, &provider.models)
            && provider.model(model).is_none()
            && projects != Some(model.as_str())
        {
            self.model = None;
        }
        if self.model.is_none() {
            self.model = projects.map(str::to_owned);
        }
        if let Some(effort) = &self.effort
            && efforts_of(provider, self.model.as_deref())
                .is_some_and(|efforts| !efforts.contains(effort))
        {
            self.effort = None;
        }
        self
    }
}

/// The efforts of the model (the provider's default one when `None`), once a session said them.
pub fn efforts_of<'a>(provider: &'a Provider, model: Option<&str>) -> Option<&'a Vec<String>> {
    let model = match model {
        Some(model) => provider.model(model)?,
        None => provider.default_model()?,
    };
    model.efforts.as_ref()
}

/// What the composer picks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Main,
    Workspace(String),
    /// A new workspace, named in the field beside the picker.
    New,
}

pub struct Composer {
    shell: WeakEntity<Shell>,
    project: String,
    tab: Tab,
    /// 0 on "New task", 1 on "Running agents": where the highlight is.
    slide: Motion,
    /// Where the row of tabs and each tab were drawn, for the highlight to slide between them.
    tabs: Rc<RefCell<[Option<Bounds<Pixels>>; 3]>>,
    field: Entity<TextareaState>,
    name: Entity<InputState>,
    has_text: bool,
    has_name: bool,
    focused: bool,
    places: Vec<PlaceChoice>,
    choice: Choice,
    menu: Option<Menu>,
    /// The providers as the daemon last listed them.
    providers: Providers,
    /// The provider and the model the project's file gives its agents.
    default: (Option<String>, Option<String>),
    /// What the next session starts on, and whether the user picked its provider: a provider
    /// picked is kept while it is offered.
    pick: Pick,
    picked: bool,
    /// What the session being started was asked to start on, until the daemon answers.
    sent: Option<Pick>,
    /// The "Running agents" tab's list and count, views of their own: a session starting or
    /// ending a turn redraws them, not the composer.
    running: Entity<RunningList>,
    count: Entity<RunningCount>,
    /// Where the session being started was asked to start, until the daemon answers.
    sending: Option<Target>,
    error: Option<SharedString>,
    blocked: Option<String>,
    /// How many times it was drawn: the tests count them.
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

fn editor_style(cx: &gpui_kit::App) -> InputEditorStyle {
    let p = palette(cx);
    InputEditorStyle {
        foreground: p.text,
        muted_foreground: p.faint,
        selection: p.run.opacity(0.35),
        caret: p.text,
        ..Default::default()
    }
}

impl Composer {
    /// The composer of one project: its draft and the session it starts belong to it, so that
    /// opening another project's page neither carries the draft there nor loses it.
    pub fn new(
        shell: WeakEntity<Shell>,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .submit_on_enter(true)
                .placeholder("Describe a task, a bug to fix, an idea to try…");
            field.set_editor_style(editor_style(cx));
            field
        });
        let name = cx.new(|cx| {
            let mut name = InputState::new(window, cx).placeholder("workspace name");
            name.set_editor_style(editor_style(cx));
            name
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &field,
                window,
                |this, field, event, window, cx| match event {
                    InputEvent::Change => {
                        let has_text = !field.read(cx).value().trim().is_empty();
                        this.set_flag(|this| &mut this.has_text, has_text, cx);
                    }
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Focus => this.set_flag(|this| &mut this.focused, true, cx),
                    InputEvent::Blur => this.set_flag(|this| &mut this.focused, false, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(&name, window, |this, name, event, window, cx| match event {
                InputEvent::Change => {
                    let has_name = !name.read(cx).value().trim().is_empty();
                    this.set_flag(|this| &mut this.has_name, has_name, cx);
                }
                InputEvent::PressEnter { .. } => this.send(window, cx),
                _ => {}
            }),
        ];
        let running = {
            let shell = shell.clone();
            cx.new(|_| RunningList {
                shell,
                sessions: Vec::new(),
                icons: HashMap::new(),
            })
        };
        Self {
            shell,
            project,
            tab: Tab::NewTask,
            slide: Motion::settled(0.0),
            tabs: Rc::new(RefCell::new([None, None, None])),
            field,
            name,
            has_text: false,
            has_name: false,
            focused: false,
            places: Vec::new(),
            choice: Choice::Main,
            menu: None,
            providers: Providers::default(),
            default: (None, None),
            pick: Pick::default(),
            picked: false,
            sent: None,
            running,
            count: cx.new(|_| RunningCount { agents: 0 }),
            sending: None,
            error: None,
            blocked: None,
            renders: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Redraws only when the flag changes: typing a second letter changes nothing drawn here.
    fn set_flag(
        &mut self,
        flag: impl Fn(&mut Self) -> &mut bool,
        value: bool,
        cx: &mut Context<Self>,
    ) {
        if *flag(self) != value {
            *flag(self) = value;
            cx.notify();
        }
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn choice(&self) -> &Choice {
        &self.choice
    }

    /// What the next session starts on.
    pub fn pick(&self) -> &Pick {
        &self.pick
    }

    /// The providers the provider picker offers, in order: enabled and ready.
    pub fn offered(&self) -> Vec<String> {
        self.providers.offered().map(|p| p.name.clone()).collect()
    }

    /// Why no session can start, as the daemon says it of each provider; empty when one can.
    pub fn not_ready(&self) -> Vec<String> {
        self.providers.why_none()
    }

    /// The providers as the daemon lists them, and what the pickers start on when the user has
    /// not picked: the project's provider and model, else the last ones used (`first_pick`).
    pub fn set_providers(
        &mut self,
        providers: &Providers,
        project: ProjectDefault,
        last: Option<&Pick>,
        cx: &mut Context<Self>,
    ) {
        let still = self
            .pick
            .provider
            .as_deref()
            .and_then(|name| providers.get(name))
            .is_some_and(Provider::offered);
        let pick = if self.picked && still {
            self.pick.clone().kept(providers, project)
        } else {
            self.picked = false;
            first_pick(providers, project, last)
        };
        let default = (project.0.map(str::to_owned), project.1.map(str::to_owned));
        if self.providers != *providers || self.pick != pick || self.default != default {
            self.providers = providers.clone();
            self.default = default;
            self.pick = pick;
            cx.notify();
        }
    }

    fn project_default(&self) -> ProjectDefault<'_> {
        (self.default.0.as_deref(), self.default.1.as_deref())
    }

    /// Picks the provider; its model and effort go back to its defaults (the project's model, when
    /// it is the provider the daemon gives it to).
    pub fn pick_provider(&mut self, name: &str, cx: &mut Context<Self>) {
        self.menu = None;
        if self.providers.get(name).is_some_and(Provider::offered) {
            self.picked = true;
            if self.pick.provider.as_deref() != Some(name) {
                self.pick = Pick {
                    provider: Some(name.to_owned()),
                    ..Pick::default()
                }
                .kept(&self.providers, self.project_default());
            }
        }
        cx.notify();
    }

    /// Picks the model, `None` for the provider's default; an effort it does not offer goes.
    pub fn pick_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        self.menu = None;
        self.picked = true;
        self.pick.model = model;
        self.pick = self
            .pick
            .clone()
            .kept(&self.providers, self.project_default());
        cx.notify();
    }

    /// Picks the effort, `None` for the model's default.
    pub fn pick_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        self.menu = None;
        self.picked = true;
        self.pick.effort = effort;
        cx.notify();
    }

    /// The project's model, when a session on the provider picked takes it.
    fn projects_model(&self) -> Option<&str> {
        let provider = self.pick.provider.as_deref()?;
        project_model(&self.providers, self.project_default(), provider)
    }

    /// What the model picker shows: the project's model marked so.
    pub fn model_shown(&self) -> String {
        let default = self.provider().and_then(Provider::default_model);
        match (&self.pick.model, default) {
            (Some(model), _) if self.projects_model() == Some(model.as_str()) => {
                format!("{model} · the project's")
            }
            (Some(model), _) => model.clone(),
            (None, Some(default)) => default.id.clone(),
            (None, None) => "default model".to_owned(),
        }
    }

    /// Why the model picked cannot start: the project's, which its provider is known not to
    /// offer. Another model picked starts.
    pub fn model_refused(&self) -> Option<String> {
        let provider = self.provider()?;
        let model = self.pick.model.as_deref()?;
        let models = provider.models.as_ref()?;
        (self.projects_model() == Some(model) && provider.model(model).is_none()).then(|| {
            let offered: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
            format!(
                "provider {} offers no model {model}, the project's: it offers {}",
                provider.name,
                offered.join(", ")
            )
        })
    }

    fn provider(&self) -> Option<&Provider> {
        self.providers.get(self.pick.provider.as_deref()?)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_sending(&self) -> bool {
        self.sending.is_some()
    }

    pub fn field(&self) -> &Entity<TextareaState> {
        &self.field
    }

    pub fn name_field(&self) -> &Entity<InputState> {
        &self.name
    }

    /// The sessions the "Running agents" tab lists, in order.
    pub fn running(&self, cx: &gpui_kit::App) -> Vec<RunningSession> {
        self.running.read(cx).sessions.clone()
    }

    /// The sessions of the "Running agents" tab, and the agents it counts: their views are
    /// drawn again when they change, the composer is not.
    pub fn set_running(&mut self, running: Vec<RunningSession>, cx: &mut Context<Self>) {
        let agents = running.iter().map(|s| s.agents).sum();
        if self.count.read(cx).agents != agents {
            self.count.update(cx, |count, cx| {
                count.agents = agents;
                cx.notify();
            });
        }
        if self.running.read(cx).sessions != running {
            self.running.update(cx, |list, cx| list.set(running, cx));
        }
    }

    pub fn set_places(&mut self, places: Vec<PlaceChoice>, cx: &mut Context<Self>) {
        if self.places != places {
            if let Choice::Workspace(name) = &self.choice
                && !places
                    .iter()
                    .any(|p| p.workspace.as_deref() == Some(name.as_str()))
            {
                self.choice = Choice::Main;
            }
            self.places = places;
            self.set_blocked();
            cx.notify();
        }
    }

    /// Picks where the next session starts, and puts the caret in the field.
    pub fn choose(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.tab = Tab::NewTask;
        self.slide_to(0.0, cx);
        if self.choice != choice {
            self.choice = choice;
            self.error = None;
            self.set_blocked();
        }
        let focus = match self.choice {
            Choice::New => self.name.read(cx).focus_handle(cx),
            _ => self.field.read(cx).focus_handle(cx),
        };
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.menu = None;
            self.slide_to(if tab == Tab::NewTask { 0.0 } else { 1.0 }, cx);
            cx.notify();
        }
    }

    fn slide_to(&mut self, target: f32, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.slide.animate(target, PANEL, now, cx.reduce_motion());
    }

    /// Opens one of its pickers' lists.
    pub fn open_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = Some(menu);
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    /// Why the chosen place cannot take a session now: a workspace prepared again. The choice is
    /// kept, not moved to `main`, so that a task meant for the workspace is not sent elsewhere.
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    fn set_blocked(&mut self) {
        self.blocked = match &self.choice {
            Choice::Workspace(name)
                if self
                    .places
                    .iter()
                    .any(|p| p.workspace.as_deref() == Some(name.as_str()) && !p.ready) =>
            {
                Some(format!(
                    "{name} is not ready: a session starts there once it is prepared"
                ))
            }
            _ => None,
        };
    }

    fn can_send(&self) -> bool {
        self.blocked.is_none()
            // Before the daemon listed the providers, it picks one itself, or says why not.
            && (self.provider().is_some_and(Provider::offered) || !self.providers.known)
            && self.model_refused().is_none()
            && self.sending.is_none()
            && self.has_text
            && (self.choice != Choice::New || self.has_name)
    }

    /// Starts a session with what the field holds, where the picker says.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        if !self.can_send() {
            return;
        }
        let message = self.field.read(cx).value().trim().to_owned();
        let target = match &self.choice {
            Choice::Main => Target::Main,
            Choice::Workspace(name) => Target::Workspace(name.clone()),
            Choice::New => Target::New(self.name.read(cx).value().trim().to_owned()),
        };
        let command = Command::Start {
            project: self.project.clone(),
            target: target.clone(),
            pick: self.pick.clone(),
            message,
        };
        self.sending = Some(target);
        self.sent = Some(self.pick.clone());
        self.error = None;
        self.menu = None;
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// The session started: the field is emptied for the next one. What it started on.
    pub fn started(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Pick> {
        let used = self.sent.take();
        let sent = self.sending.take();
        self.error = None;
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.has_text = false;
        // The name was the new workspace's, made now: what the picker says since does not matter.
        if matches!(sent, Some(Target::New(_))) {
            self.name
                .update(cx, |name, cx| name.set_value("", window, cx));
            self.has_name = false;
            if self.choice == Choice::New {
                self.choice = Choice::Main;
            }
        }
        cx.notify();
        used
    }

    /// No session started: the field keeps the message, and says why.
    pub fn not_started(&mut self, why: &str, cx: &mut Context<Self>) {
        self.sending = None;
        self.sent = None;
        self.error = Some(SharedString::from(why.to_owned()));
        cx.notify();
    }

    fn render_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let now = cx.background_executor().now();
        if self.slide.is_running(now) {
            window.request_animation_frame();
        }
        let t = self.slide.value(now);
        let bounds = *self.tabs.borrow();
        // The highlight slides between the two tabs' places, measured as they were drawn.
        let pip = match bounds {
            [Some(row), Some(a), Some(b)] => {
                let x = |at: Bounds<Pixels>| f32::from(at.origin.x - row.origin.x);
                let width = |at: Bounds<Pixels>| f32::from(at.size.width);
                Some(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(px(x(a) + (x(b) - x(a)) * t))
                        .w(px(width(a) + (width(b) - width(a)) * t))
                        .h(px(26.))
                        .rounded(px(7.))
                        .bg(p.sel),
                )
            }
            _ => None,
        };
        let has_pip = pip.is_some();
        let tab =
            |index: usize, tab: Tab, label: SharedString, count: Option<Entity<RunningCount>>| {
                let selected = self.tab == tab;
                let tabs = self.tabs.clone();
                let id = match tab {
                    Tab::NewTask => "tab-task",
                    Tab::Running => "tab-running",
                };
                div()
                    .id(id)
                    .relative()
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(if selected { p.text } else { p.faint })
                    .when(selected && !has_pip, |d| d.bg(p.sel))
                    .child(label)
                    .children(count)
                    .child(
                        canvas(
                            move |b: Bounds<Pixels>, _, _| tabs.borrow_mut()[index + 1] = Some(b),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select_tab(tab, cx)))
                    .test_support()
            };
        div()
            .relative()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .pb(px(6.))
            .child({
                let tabs = self.tabs.clone();
                canvas(
                    move |b: Bounds<Pixels>, _, _| tabs.borrow_mut()[0] = Some(b),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            })
            .children(pip)
            .child(tab(0, Tab::NewTask, "New task".into(), None))
            .child(tab(
                1,
                Tab::Running,
                "Running agents".into(),
                Some(self.count.clone()),
            ))
    }

    fn render_pickers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (glyph, label) = match &self.choice {
            Choice::Main => (Icon::Folder, "main".to_owned()),
            Choice::Workspace(name) => (Icon::Branch, name.clone()),
            Choice::New => (Icon::Branch, "New workspace".to_owned()),
        };
        let can_send = self.can_send();
        div()
            .relative()
            .flex()
            .items_center()
            .gap(px(2.))
            .pt(px(6.))
            .px(px(2.))
            .child(
                div()
                    .id("place-picker")
                    .h(px(28.))
                    .px(px(9.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .hover(|d| d.bg(p.hover).text_color(p.text))
                    .child(icon(glyph).size(px(14.)).text_color(p.faint))
                    .child(SharedString::from(label))
                    .child(icon(Icon::Choose).size(px(10.)).text_color(p.faint))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Place, cx)))
                    .test_support(),
            )
            .when(self.choice == Choice::New, |d| {
                d.child(
                    div()
                        .id("new-workspace-name")
                        .w(px(170.))
                        .h(px(26.))
                        .px(px(8.))
                        .rounded(px(7.))
                        .bg(p.field)
                        .border_1()
                        .border_color(p.field_line)
                        .flex()
                        .items_center()
                        .mono(12.)
                        .child(Input::new(&self.name))
                        .test_support(),
                )
            })
            .child(div().flex_1())
            .children(self.render_agent_pickers(cx))
            .child(
                Button::round("send", Icon::Send, "Start")
                    .disabled(!can_send)
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            )
            .when(self.menu == Some(Menu::Place), |d| {
                d.child(self.render_menu(cx).into_any_element())
            })
            .when_some(self.menu.filter(|m| *m != Menu::Place), |d, menu| {
                d.child(self.render_agent_menu(menu, cx).into_any_element())
            })
    }

    /// The provider, model and effort pickers: the model's once a provider is picked, the
    /// effort's once the model is known to have efforts.
    fn render_agent_pickers(&self, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let p = palette(cx);
        let Some(provider) = self.provider() else {
            return Vec::new();
        };
        let picker = |id: &'static str, menu: Menu, label: String, strong: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(9.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.5))
                .text_color(if strong { p.text } else { p.mute })
                .when(strong, |d| d.medium())
                .when(!strong, |d| d.mono(12.))
                .hover(|d| d.bg(p.hover).text_color(p.text))
                .child(SharedString::from(label))
                .child(icon(Icon::Choose).size(px(10.)).text_color(p.faint))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_menu(menu, cx)))
                .test_support()
                .into_any_element()
        };
        let model = self.model_shown();
        let mut pickers = vec![
            picker(
                "provider-picker",
                Menu::Provider,
                provider.title.clone(),
                true,
            ),
            picker("model-picker", Menu::Model, model, false),
        ];
        if efforts_of(provider, self.pick.model.as_deref()).is_some_and(|e| !e.is_empty()) {
            let effort = self
                .pick
                .effort
                .clone()
                .unwrap_or_else(|| "default effort".into());
            pickers.push(picker("effort-picker", Menu::Effort, effort, false));
        }
        pickers
    }

    fn render_agent_menu(&self, menu: Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let item =
            |id: String, label: String, note: Option<String>, on: bool, pick: Option<Pick>| {
                div()
                    .id(ElementId::Name(id.into()))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .text_color(if pick.is_some() { p.text } else { p.faint })
                    .when(on, |d| d.bg(p.sel))
                    .when(pick.is_some() && !on, |d| d.hover(|d| d.bg(p.hover)))
                    .child(div().flex_1().child(SharedString::from(label)))
                    .when_some(note, |d, note| {
                        d.child(
                            div()
                                .mono(11.)
                                .text_color(p.faint)
                                .child(SharedString::from(note)),
                        )
                    })
                    .when_some(pick, |d, pick| {
                        d.on_click(cx.listener(move |this, _, _, cx| match menu {
                            Menu::Provider => {
                                this.pick_provider(pick.provider.as_deref().unwrap_or_default(), cx)
                            }
                            Menu::Model => this.pick_model(pick.model.clone(), cx),
                            _ => this.pick_effort(pick.effort.clone(), cx),
                        }))
                    })
                    .test_support()
            };
        let pick = |provider: Option<&str>, model: Option<&str>, effort: Option<&str>| Pick {
            provider: provider.map(str::to_owned),
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        };
        let mut items = Vec::new();
        let provider = self.provider();
        let projects = self.projects_model();
        match (menu, provider) {
            (Menu::Provider, _) => {
                for offered in self.providers.offered() {
                    items.push(item(
                        format!("provider-choice-{}", offered.name),
                        offered.title.clone(),
                        Some(offered.name.clone()),
                        self.pick.provider.as_deref() == Some(offered.name.as_str()),
                        Some(pick(Some(&offered.name), None, None)),
                    ));
                }
            }
            (Menu::Model, Some(provider)) => match &provider.models {
                Some(models) => {
                    for model in models {
                        let on = match &self.pick.model {
                            Some(picked) => picked == &model.id,
                            None => model.default,
                        };
                        let note = if projects == Some(model.id.as_str()) {
                            Some("the project's".to_owned())
                        } else {
                            model.default.then(|| "default".to_owned())
                        };
                        items.push(item(
                            format!("model-choice-{}", model.id),
                            model.id.clone(),
                            note,
                            on,
                            Some(pick(None, Some(&model.id), None)),
                        ));
                    }
                }
                // No agent is run just to list them: the first session of the provider says them.
                None => {
                    // Without a model, the daemon gives the project's: no other default to pick.
                    if projects.is_none() {
                        items.push(item(
                            "model-choice-default".into(),
                            "default model".into(),
                            None,
                            self.pick.model.is_none(),
                            Some(pick(None, None, None)),
                        ));
                    }
                    if let Some(model) = &self.pick.model {
                        items.push(item(
                            format!("model-choice-{model}"),
                            model.clone(),
                            (projects == Some(model.as_str())).then(|| "the project's".into()),
                            true,
                            Some(pick(None, Some(model), None)),
                        ));
                    }
                    items.push(item(
                        "models-unknown".into(),
                        format!(
                            "{}'s models are known once one of its sessions opened",
                            provider.title
                        ),
                        None,
                        false,
                        None,
                    ));
                }
            },
            (Menu::Effort, Some(provider)) => {
                items.push(item(
                    "effort-choice-default".into(),
                    "default effort".into(),
                    None,
                    self.pick.effort.is_none(),
                    Some(pick(None, None, None)),
                ));
                for effort in efforts_of(provider, self.pick.model.as_deref())
                    .into_iter()
                    .flatten()
                {
                    items.push(item(
                        format!("effort-choice-{effort}"),
                        effort.clone(),
                        None,
                        self.pick.effort.as_deref() == Some(effort.as_str()),
                        Some(pick(None, None, Some(effort))),
                    ));
                }
            }
            _ => {}
        }
        div()
            .id("agent-menu")
            .occlude()
            .absolute()
            .top(px(40.))
            .right(px(0.))
            .w(px(280.))
            .p(px(6.))
            .rounded(px(10.))
            .bg(p.card)
            .border_1()
            .border_color(p.line2)
            .shadow_lg()
            .flex()
            .flex_col()
            .children(items)
    }

    /// Why no session can start, as the daemon says it, and the way to the settings page.
    fn render_not_ready(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let why = self.providers.why_none();
        if why.is_empty() {
            return None;
        }
        let p = palette(cx);
        let shell = self.shell.clone();
        Some(
            div()
                .id("composer-not-ready")
                .px(px(10.))
                .pt(px(6.))
                .pb(px(2.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .text_size(px(12.5))
                .text_color(p.faint)
                .child(
                    div()
                        .text_color(p.mute)
                        .child("No provider can start a session:"),
                )
                .children(
                    why.into_iter()
                        .map(|line| div().child(SharedString::from(line))),
                )
                .child(
                    div()
                        .id("open-settings")
                        .mt(px(2.))
                        .text_color(p.text)
                        .cursor_pointer()
                        .hover(|d| d.underline())
                        .child("Open the settings")
                        .on_click(move |_, window, cx| {
                            shell
                                .update(cx, |shell, cx| shell.open(Selection::Settings, window, cx))
                                .ok();
                        })
                        .test_support(),
                ),
        )
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let item = |id: String,
                    glyph: Icon,
                    label: String,
                    note: Option<&'static str>,
                    choice: Option<Choice>| {
            div()
                .id(ElementId::Name(id.into()))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.5))
                .text_color(if choice.is_some() { p.text } else { p.faint })
                .when(choice.is_some(), |d| d.hover(|d| d.bg(p.hover)))
                .child(icon(glyph).size(px(14.)).text_color(p.faint))
                .child(div().flex_1().child(SharedString::from(label)))
                .when_some(note, |d, note| {
                    d.child(div().mono(11.).text_color(p.faint).child(note))
                })
                .when_some(choice, |d, choice| {
                    d.on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(choice.clone(), window, cx)
                    }))
                })
                .test_support()
        };
        let mut items = vec![item(
            "place-choice-main".into(),
            Icon::Folder,
            "main".into(),
            Some("project folder"),
            Some(Choice::Main),
        )];
        for place in self.places.iter().filter(|p| p.workspace.is_some()) {
            let name = place.workspace.clone().unwrap_or_default();
            items.push(item(
                format!("place-choice-{name}"),
                Icon::Branch,
                name.clone(),
                (!place.ready).then_some("not ready"),
                place.ready.then(|| Choice::Workspace(name.clone())),
            ));
        }
        items.push(item(
            "place-choice-new".into(),
            Icon::Plus,
            "New workspace".into(),
            None,
            Some(Choice::New),
        ));
        div()
            .id("place-menu")
            .occlude()
            .absolute()
            .top(px(40.))
            .left(px(0.))
            .w(px(260.))
            .p(px(6.))
            .rounded(px(10.))
            .bg(p.card)
            .border_1()
            .border_color(p.line2)
            .shadow_lg()
            .flex()
            .flex_col()
            .children(items)
    }
}

pub fn agents_label(n: usize) -> String {
    match n {
        1 => "1 agent".into(),
        n => format!("{n} agents"),
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let tabs = self.render_tabs(window, cx).into_any_element();
        let body = match self.tab {
            Tab::NewTask => div()
                .id("composer-field")
                .min_h(px(64.))
                .px(px(14.))
                .py(px(12.))
                .text_size(px(14.))
                .line_height(px(21.))
                .cursor_text()
                .child(self.field.clone())
                .on_click(cx.listener(|this, _, window, cx| {
                    let focus = this.field.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                }))
                .test_support()
                .into_any_element(),
            Tab::Running => self.running.clone().into_any_element(),
        };
        div()
            .id("composer")
            .relative()
            .w_full()
            .p(px(6.))
            .rounded(px(16.))
            .bg(p.shell)
            .border_1()
            .border_color(p.line)
            .shadow_lg()
            .ui_text()
            .text_color(p.text)
            .child(tabs)
            .child(
                div()
                    .rounded(px(11.))
                    .bg(p.field)
                    .border_1()
                    .border_color(if self.focused {
                        p.focus_line
                    } else {
                        p.field_line
                    })
                    .when(self.focused, |d| {
                        d.shadow(vec![gpui_kit::BoxShadow {
                            color: p.focus_ring,
                            offset: gpui_kit::point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(3.),
                            inset: false,
                        }])
                    })
                    .overflow_hidden()
                    .child(body),
            )
            .child(self.render_pickers(cx).into_any_element())
            .children(self.render_not_ready(cx))
            .when_some(self.model_refused(), |d, why| {
                d.child(
                    div()
                        .id("composer-model-refused")
                        .px(px(10.))
                        .pt(px(6.))
                        .pb(px(2.))
                        .text_size(px(12.5))
                        .text_color(p.faint)
                        .child(SharedString::from(why)),
                )
            })
            .when_some(self.blocked.clone(), |d, why| {
                d.child(
                    div()
                        .id("composer-blocked")
                        .px(px(10.))
                        .pt(px(6.))
                        .pb(px(2.))
                        .text_size(px(12.5))
                        .text_color(p.faint)
                        .child(SharedString::from(why)),
                )
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("composer-error")
                        .px(px(10.))
                        .pt(px(6.))
                        .pb(px(2.))
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error),
                )
            })
            .when(self.sending.is_some(), |d| {
                d.child(
                    div()
                        .px(px(10.))
                        .pt(px(6.))
                        .pb(px(2.))
                        .mono(11.5)
                        .text_color(p.faint)
                        .child(match self.choice {
                            Choice::New => "Preparing the workspace, then starting…",
                            _ => "Starting…",
                        }),
                )
            })
    }
}

/// The sessions of the "Running agents" tab.
pub struct RunningList {
    shell: WeakEntity<Shell>,
    sessions: Vec<RunningSession>,
    icons: HashMap<String, Entity<StateIcon>>,
}

impl RunningList {
    fn set(&mut self, sessions: Vec<RunningSession>, cx: &mut Context<Self>) {
        self.icons
            .retain(|id, _| sessions.iter().any(|s| &s.id == id));
        for session in &sessions {
            let status = session.status.clone();
            self.icons
                .entry(session.id.clone())
                .or_insert_with(|| cx.new(|cx| StateIcon::new(status, cx)));
        }
        self.sessions = sessions;
        cx.notify();
    }
}

impl Render for RunningList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let list = div().id("running-list").p(px(6.)).flex().flex_col();
        if self.sessions.is_empty() {
            return list.child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_color(p.faint)
                    .child("Nothing running in this project."),
            );
        }
        list.children(self.sessions.iter().enumerate().map(|(index, session)| {
            let shell = self.shell.clone();
            let id = session.id.clone();
            let (label, color) = if session.waiting {
                ("needs you", p.wait)
            } else {
                ("running", p.faint)
            };
            enter(
                ElementId::Name(format!("enter-running-{}", session.id).into()),
                index,
                ListRow::new(
                    ElementId::Name(format!("running-{}", session.id).into()),
                    SharedString::from(session.title.clone()),
                )
                .when_some(self.icons.get(&session.id).cloned(), |row, icon| {
                    row.leading(icon)
                })
                .detail(SharedString::from(format!("· {}", session.place)))
                .meta(SharedString::from(agents_label(session.agents)))
                .trailing(div().mono(11.5).text_color(color).child(label))
                .on_click(move |_, window, cx| {
                    let session = id.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.open(Selection::Session(session), window, cx)
                        })
                        .ok();
                }),
            )
        }))
    }
}

/// The agents the "Running agents" tab counts.
pub struct RunningCount {
    agents: usize,
}

impl Render for RunningCount {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .mono(11.)
            .text_color(palette(cx).mute)
            .child(self.agents.to_string())
    }
}
