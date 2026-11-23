//! The composer that starts a session, in two layers: an outer shell, the field with its own
//! border and focus ring, and the pickers below it (the place, the agent and model, send). The field is its own view: typing redraws it, never the page.

use std::sync::Arc;

use gpui_design_system::color::mix;
use gpui_design_system::motion::HOVER;
use gpui_design_system::{
    Button, Chosen, Dropdown, Icon, MenuItem, ModelEntry, ModelPicker, Toast, TokenEvent,
    TokenField, Type, enter, fade, hoverable, palette, presence_of, space, toast,
};
use gpui_kit::base::input::{InputEvent, RopeExt as _, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Command, Pick, Target};
use crate::attachments::{Attachments, STILL_SCALING};
use crate::kept::{Draft, DraftKey, Keeping, Written};
use crate::mode::ModeSwitch;
use crate::providers::{Provider, Providers};
use crate::settings::Reveal;
use crate::shell::Shell;

/// What a toast says when the daemon starts no session, or the composer waits for a picture.
const NOT_STARTED: &str = "The session was not started";

/// A place the composer can start a session in.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceChoice {
    /// `None` for `main`.
    pub workspace: Option<String>,
    /// Whether a session can start there now: a workspace being prepared, or whose preparation
    /// failed, cannot take one.
    pub ready: bool,
}

/// The list a picker of the composer opens: the place's. The agent's is the kit's `ModelPicker`,
/// which keeps its own menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Place,
    Machine,
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
    providers: &'a Providers,
    project: ProjectDefault<'a>,
    provider: &str,
) -> Option<&'a str> {
    let model = project
        .1
        .filter(|_| project_provider(providers, project) == Some(provider))?;
    // A model the list dropped is the one of its family its session starts on.
    Some(
        providers
            .get(provider)
            .and_then(|p| p.resolved_model(model))
            .map_or(model, |m| m.id.as_str()),
    )
}

/// The provider, the model and the effort a new session starts on, when the user has not picked
/// one: the project's provider (its own, else the one its model goes to) with its model, else the
/// ones the user chose for new sessions (Settings), else the last ones used, else the first
/// provider offered; each only when it is offered now. The setting gives what the project leaves
/// out, for its own provider, and its effort goes with its own model only.
pub fn first_pick(providers: &Providers, project: ProjectDefault, last: Option<&Pick>) -> Pick {
    let offered = |name: &str| providers.get(name).is_some_and(Provider::offered);
    let setting = providers
        .new_session
        .as_ref()
        .filter(|(provider, _, _)| offered(provider));
    if project != (None, None)
        && let Some(provider) = project_provider(providers, project)
        && offered(provider)
    {
        let mut pick = Pick {
            provider: Some(provider.to_owned()),
            ..Pick::default()
        }
        .kept(providers, project);
        if let Some((_, model, effort)) = setting.filter(|(name, _, _)| name == provider) {
            // The project's model first; the setting gives the one the project leaves out, and
            // its effort goes with its own model only.
            if pick.model.is_none() {
                pick.model = model.clone();
            }
            if pick.model == *model {
                pick.effort = effort.clone();
            }
            pick = pick.kept(providers, project);
        }
        return pick;
    }
    if let Some((provider, model, effort)) = setting {
        return Pick {
            provider: Some(provider.clone()),
            model: model.clone(),
            effort: effort.clone(),
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
        // A model picked that the list dropped is the one of its family a session starts on.
        if let Some(model) = &self.model
            && provider.model(model).is_none()
            && let Some(resolved) = provider.resolved_model(model)
        {
            self.model = Some(resolved.id.clone());
        }
        if let (Some(model), Some(_)) = (&self.model, &provider.models)
            && provider.model(model).is_none()
            && projects != Some(model.as_str())
        {
            // The effort chosen went with it.
            self.model = None;
            self.effort = None;
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
}

pub struct Composer {
    shell: WeakEntity<Shell>,
    project: String,
    /// The field: its text and the chips of its pictures.
    tokens: Entity<TokenField>,
    /// Its text, as `tokens` edits it.
    field: Entity<TextareaState>,
    has_text: bool,
    focused: bool,
    /// The pictures that go with the first message.
    attached: Attachments,
    /// What the daemon keeps of the draft, and the wait before it is written.
    keeping: Keeping,
    places: Vec<PlaceChoice>,
    choice: Choice,
    /// The machine the composer starts on is off: nothing is started on it.
    machine_off: bool,
    /// A workspace made from the picker, which becomes the place once the daemon lists it.
    wanted: Option<String>,
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
    /// Where the session being started was asked to start, until the daemon answers.
    sending: Option<Target>,
    error: Option<SharedString>,
    blocked: Option<String>,
    /// The mode the next session starts in: Auto each time the composer is shown and after each
    /// start, so that Full is never on by mistake.
    mode: Entity<ModeSwitch>,
    /// How many times it was drawn: the tests count them.
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

/// The field is as tall as its text, a line at a time, from three lines up to this many, then it
/// scrolls: the project page's composer starts roomy, and no empty room is left under more text.
const MIN_ROWS: usize = 3;
const MAX_ROWS: usize = 8;

impl Composer {
    /// The composer of one project: its draft and the session it starts belong to it, so that
    /// opening another project's page neither carries the draft there nor loses it.
    pub fn new(
        shell: WeakEntity<Shell>,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The text holds the pictures' chips where they were put (#773).
        let tokens = cx.new(|cx| TokenField::new("composer-text", window, cx));
        let field = tokens.read(cx).input().clone();
        field.update(cx, |field, cx| {
            field.set_auto_grow(MIN_ROWS, MAX_ROWS, cx);
            field.set_submit_on_enter(true, cx);
            field.set_placeholder("Describe a task, a bug to fix, an idea to try…", window, cx);
        });
        let mode = cx.new(|_| ModeSwitch::local(shell.clone()));
        let subscriptions = vec![
            cx.subscribe(&tokens, |this: &mut Self, _, event: &TokenEvent, cx| {
                this.attached.chip_changed(event, cx);
                this.draft_changed(cx);
            }),
            cx.subscribe_in(
                &field,
                window,
                |this, field, event, window, cx| match event {
                    InputEvent::Change => {
                        this.draft_changed(cx);
                        let has_text = !field.read(cx).value().trim().is_empty();
                        this.set_flag(|this| &mut this.has_text, has_text, cx);
                        this.attached.follow(cx);
                    }
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Focus => this.set_flag(|this| &mut this.focused, true, cx),
                    InputEvent::Blur => this.set_flag(|this| &mut this.focused, false, cx),
                    _ => {}
                },
            ),
        ];
        Self {
            shell,
            keeping: Keeping::new(DraftKey::Project(project.clone())),
            project,
            attached: Attachments::new(tokens.clone()),
            tokens,
            field,
            has_text: false,
            focused: false,
            places: Vec::new(),
            choice: Choice::Main,
            machine_off: false,
            wanted: None,
            menu: None,
            providers: Providers::default(),
            default: (None, None),
            pick: Pick::default(),
            picked: false,
            sent: None,
            sending: None,
            error: None,
            blocked: None,
            mode,
            renders: 0,
            _subscriptions: subscriptions,
        }
    }

    /// How many times the draft was written to the daemon.
    pub fn drafts_sent(&self) -> usize {
        self.keeping.sent()
    }

    fn draft_now(&self, cx: &App) -> Draft {
        let (text, pictures) = self.attached.draft(cx);
        Draft { text, pictures }
    }

    /// The draft changed: it is written 500 ms after the last change.
    fn draft_changed(&mut self, cx: &mut Context<Self>) {
        self.keeping.changed(cx, Self::keep_draft_now);
    }

    /// Writes the draft to the daemon now, when it does not hold it already. Asked once this
    /// update is over: the shell is the one updating when a session started.
    fn keep_draft_now(&mut self, cx: &mut Context<Self>) {
        if let Some(write) = self.draft_to_write(cx, false) {
            let shell = self.shell.clone();
            cx.defer(move |cx| {
                shell.update(cx, |shell, _| shell.perform(write)).ok();
            });
        }
    }

    /// What the daemon must be asked to keep the draft as it is now, if it does not hold it
    /// (`closing`: the window is closing, `Keeping::write`).
    pub fn draft_to_write(&mut self, cx: &mut Context<Self>, closing: bool) -> Option<Command> {
        let now = self.draft_now(cx);
        self.keeping.write(now, closing)
    }

    /// Whether the daemon answered the read of the draft.
    pub fn draft_was_read(&self) -> bool {
        self.keeping.was_read()
    }

    /// The daemon answered the read of the draft: a composer still empty takes it, a field the
    /// user typed in meanwhile keeps their words, and is written.
    pub fn draft_read(
        &mut self,
        found: Option<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed = !self.draft_now(cx).is_empty();
        self.keeping.answered(found.clone());
        match found {
            Some(Draft { text, pictures }) if !typed => {
                let restored = self.attached.restore_with(&text, pictures, Vec::new(), cx);
                self.tokens
                    .update(cx, |field, cx| field.set_value(restored, window, cx).ok());
                let end = self.field.read(cx).value().len();
                self.field
                    .update(cx, |field, cx| field.set_selected_range(end..end, cx));
                self.has_text = !self.field.read(cx).value().trim().is_empty();
                cx.notify();
            }
            _ if typed => self.draft_changed(cx),
            _ => {}
        }
    }

    /// The daemon did not answer the read of the draft: nothing is written, and it is asked
    /// again.
    pub fn draft_read_failed(&mut self, cx: &mut Context<Self>) {
        self.keeping.read_failed(cx, Self::read_draft_again);
    }

    fn read_draft_again(&mut self, cx: &mut Context<Self>) {
        let key = self.keeping.key().clone();
        self.shell
            .update(cx, |shell, _| shell.perform(Command::DraftRead { key }))
            .ok();
    }

    /// The daemon answered a write of the draft.
    pub fn draft_written(&mut self, result: Written, cx: &mut Context<Self>) {
        if self.keeping.wrote(result) {
            self.draft_changed(cx);
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

    /// The control of the mode the next session starts in.
    pub fn mode(&self) -> &Entity<ModeSwitch> {
        &self.mode
    }

    /// Back on Auto, as each time the composer is shown.
    pub fn reset_mode(&mut self, cx: &mut Context<Self>) {
        self.mode.update(cx, |mode, cx| mode.reset(cx));
    }

    pub fn renders(&self) -> usize {
        self.renders
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
        if providers.known {
            let chosen = providers.judge.is_some();
            self.mode
                .update(cx, |mode, cx| mode.set_judge_chosen(chosen, cx));
        }
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

    fn pickers(&self) -> Pickers<'_> {
        Pickers {
            providers: &self.providers,
            pick: &self.pick,
            project: self.project_default(),
        }
    }

    /// Picks the provider; its model and effort go back to its defaults (the project's model, when
    /// it is the provider the daemon gives it to).
    pub fn pick_provider(&mut self, name: &str, cx: &mut Context<Self>) {
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
        self.picked = true;
        self.pick.effort = effort;
        cx.notify();
    }

    /// Why the model picked cannot start: the project's, which its provider is known not to
    /// offer. Another model picked starts. Models said by another version refuse nothing.
    pub fn model_refused(&self) -> Option<String> {
        let provider = self.provider().filter(|p| !p.models_outdated)?;
        let model = self.pick.model.as_deref()?;
        let models = provider.models.as_ref()?;
        (self.pickers().projects_model() == Some(model) && provider.model(model).is_none()).then(
            || {
                let offered: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
                format!(
                    "provider {} offers no model {model}, the project's: it offers {}",
                    provider.name,
                    offered.join(", ")
                )
            },
        )
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

    /// The field with its chips.
    pub fn tokens(&self) -> &Entity<TokenField> {
        &self.tokens
    }

    /// Whether the chosen provider's agents take images; `None` until one started.
    pub fn takes_images(&self) -> Option<bool> {
        self.provider().and_then(|provider| provider.images)
    }

    /// The pictures that go with the first message.
    pub fn attachments(&self) -> Vec<Arc<gpui_kit::Image>> {
        self.attached.pictures()
    }

    /// The chip of the picture `at` is in the text.
    pub fn has_chip(&self, at: usize, cx: &App) -> bool {
        self.attached.has_chip(at, cx)
    }

    /// The pictures, told first whether the chosen provider's agents take any, so that one that
    /// takes none is said in a toast, as a session's composer says it.
    fn attached_of(&mut self) -> &mut Attachments {
        self.attached.set_takes(self.takes_images());
        &mut self.attached
    }

    /// Attaches a picture, or says in a toast why not; the text in the field stays.
    pub fn attach(&mut self, image: gpui_kit::Image, cx: &mut Context<Self>) {
        self.attached_of().attach(image, cx, Self::attached_of);
    }

    /// Attaches the picture a file holds (a file dropped on the composer, or picked).
    pub fn attach_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.attached_of().attach_file(path, cx, Self::attached_of);
    }

    pub fn detach(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.attached.detach(at, window, cx);
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
            self.take_wanted();
            self.set_blocked();
            cx.notify();
        }
    }

    /// The workspace just made from the picker is the place from now on: at once if the daemon
    /// already lists it, else as soon as it does. Picking another place first drops it.
    pub fn choose_when_listed(&mut self, name: String, cx: &mut Context<Self>) {
        self.wanted = Some(name);
        self.take_wanted();
        self.set_blocked();
        cx.notify();
    }

    fn take_wanted(&mut self) {
        let listed = self.wanted.as_deref().is_some_and(|name| {
            self.places
                .iter()
                .any(|p| p.workspace.as_deref() == Some(name))
        });
        if listed && let Some(name) = self.wanted.take() {
            self.choice = Choice::Workspace(name);
            self.error = None;
        }
    }

    /// Picks where the next session starts, and puts the caret in the field.
    pub fn choose(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.wanted = None;
        if self.choice != choice {
            self.choice = choice;
            self.error = None;
            self.set_blocked();
        }
        let focus = self.field.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// The place is `main` again: another machine's places are not this one's.
    fn back_to_main(&mut self, cx: &mut Context<Self>) {
        self.wanted = None;
        self.choice = Choice::Main;
        self.error = None;
        self.set_blocked();
        cx.notify();
    }

    /// Puts the caret at the end of what the field holds and gives it the focus: where "New
    /// session" lands, a draft kept or not.
    pub fn focus_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| {
            let end = field.text().offset_to_position(field.text().len());
            field.set_cursor_position(end, window, cx);
        });
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
            && !self.machine_off
            // Before the daemon listed the providers, it picks one itself, or says why not.
            && (self.provider().is_some_and(Provider::offered) || !self.providers.known)
            && self.model_refused().is_none()
            && self.sending.is_none()
            // Pictures alone are a first message, as they are a later one.
            && (self.has_text || !self.attached.is_empty())
    }

    /// Starts a session with what the field holds, where the picker says.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        // A picture that is not there yet would be left behind: the sender waits for it.
        if self.attached.is_scaling() {
            return toast(Toast::failure(NOT_STARTED).detail(STILL_SCALING), cx);
        }
        if !self.can_send() {
            return;
        }
        let message = self.attached.message(cx).trim().to_owned();
        let target = match &self.choice {
            Choice::Main => Target::Main,
            Choice::Workspace(name) => Target::Workspace(name.clone()),
        };
        let command = Command::Start {
            project: self.project.clone(),
            target: target.clone(),
            pick: self.asked(),
            message,
            images: self.attached.sending(),
            texts: self.attached.texts(cx),
            mode: self.mode.read(cx).mode().unwrap_or("auto").to_owned(),
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

    /// What a start asks: once the user picked, the model and the effort the pickers show, by
    /// their names, so that the setting the daemon applies to a pick that names none does not
    /// replace what they picked (the recommended ones included).
    fn asked(&self) -> Pick {
        let mut pick = self.pick.clone();
        if !self.picked {
            return pick;
        }
        let pickers = self.pickers();
        if pick.model.is_none() {
            pick.model = self
                .provider()
                .and_then(Provider::default_model)
                .map(|m| m.id.clone());
        }
        if pickers.picked_effort().is_none() {
            pick.effort = pickers.own_effort().cloned();
        }
        pick
    }

    /// The session started: the field is emptied for the next one. What it started on.
    pub fn started(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Pick> {
        let used = self.sent.take();
        self.sending = None;
        self.error = None;
        self.tokens
            .update(cx, |field, cx| field.set_value(Vec::new(), window, cx).ok());
        self.has_text = false;
        self.attached.clear();
        // Started: its draft goes now, not 500 ms later.
        self.keep_draft_now(cx);
        self.reset_mode(cx);
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

    /// A handler of the composer, for a picker or one of its entries.
    fn handler(
        cx: &Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |window, cx| {
            this.update(cx, |this, cx| f(this, window, cx)).ok();
        }
    }

    fn render_pickers(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (glyph, label) = match &self.choice {
            Choice::Main => (Icon::Folder, "main".to_owned()),
            Choice::Workspace(name) => (Icon::Branch, name.clone()),
        };
        let can_send = self.can_send();
        div()
            .relative()
            .flex()
            .items_center()
            .gap(space::XS)
            .pt(space::SM)
            .px(space::XS)
            .child(
                Dropdown::new("place-picker", label)
                    .leading(glyph)
                    .open(self.menu == Some(Menu::Place))
                    .on_toggle(Self::handler(cx, |this, _, cx| {
                        this.toggle_menu(Menu::Place, cx)
                    }))
                    .items(self.place_items(cx)),
            )
            .child(Attachments::button(cx, Self::attached_of))
            .child(div().flex_1())
            .children(self.machine_picker(window, cx))
            .children(self.render_agent_picker(cx))
            .child(self.mode.clone())
            .child(
                Button::round("send", Icon::Send, "Start")
                    .disabled(!can_send)
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            )
    }

    /// The machines that have the project, when there are several: this one first, each with its
    /// state.
    fn machine_choices(&self, cx: &App) -> Vec<crate::machines::Choice> {
        let Some(shell) = self.shell.upgrade() else {
            return Vec::new();
        };
        let shown = crate::machines::shown_machine(cx);
        shell
            .read(cx)
            .machines()
            .read(cx)
            .machines()
            .map(|read| crate::machines::project_choices(read, &self.project, shown.as_deref()))
            .unwrap_or_default()
    }

    /// The picker of the machine the session starts on, beside the agent's: only a project that
    /// is on several machines has one. Choosing another brings it in front, on this project's
    /// page, the place back on `main`.
    fn machine_picker(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        // Once the project is on one machine only, the picker narrows and fades away with the
        // choices it had.
        let (choices, present) =
            crate::machines::kept("machine-picker-kept", self.machine_choices(cx), window, cx);
        if choices.is_empty() {
            return None;
        }
        let shown = crate::machines::shown_machine(cx);
        let chosen = choices
            .iter()
            .position(|choice| choice.name == shown)
            .unwrap_or(0);
        let (this, shell, project) = (
            cx.entity().downgrade(),
            self.shell.clone(),
            self.project.clone(),
        );
        let picker = crate::machines::picker(
            "machine-picker",
            "machine-choice",
            &choices,
            chosen,
            self.open_of(Menu::Machine),
            Self::handler(cx, |this, _, cx| this.toggle_menu(Menu::Machine, cx)),
            move |name, window, cx| {
                this.update(cx, |this, cx| {
                    this.menu = None;
                    this.back_to_main(cx);
                })
                .ok();
                let project = project.clone();
                shell
                    .update(cx, |shell, cx| {
                        shell.choose_machine_for(name, &project, window, cx)
                    })
                    .ok();
            },
        );
        // It enters when the project turns out to be on several machines, and leaves when it is
        // not any more.
        crate::machines::narrowing(
            "machine-picker-fold",
            present,
            enter("machine-picker-enter", 0, picker).into_any_element(),
            window,
            cx,
        )
    }

    /// Whether the list of `menu` is open.
    fn open_of(&self, menu: Menu) -> bool {
        self.menu == Some(menu)
    }

    /// The one picker of the agent, once a provider is picked.
    fn render_agent_picker(&self, cx: &mut Context<Self>) -> Option<ModelPicker> {
        let this = cx.entity().downgrade();
        agent_picker(
            "agent-picker",
            &self.pickers(),
            false,
            move |chosen, effort, _, cx| {
                this.update(cx, |this, cx| this.choose_agent(chosen, effort, cx))
                    .ok();
            },
        )
    }

    /// The user chose in the agent picker: another provider (its model and effort go back to its
    /// defaults), another model, or the effort of the model.
    fn choose_agent(
        &mut self,
        chosen: Chosen,
        effort: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let Chosen::Model(provider, model) = chosen else {
            return;
        };
        if self.pick.provider.as_deref() != Some(provider.as_ref()) {
            return self.pick_provider(&provider, cx);
        }
        // The recommended model is reported as none: what runs is its own, which a model the
        // project gives would replace, so it is named then.
        let own = self
            .provider()
            .and_then(Provider::default_model)
            .map(|m| m.id.clone());
        let model = model.map(|model| model.to_string());
        if model.clone().or(own.clone()) != self.pick.model.clone().or(own.clone()) {
            let named = self.pickers().projects_model().is_some();
            return self.pick_model(model.or(own.filter(|_| named)), cx);
        }
        self.pick_effort(effort.map(|effort| effort.to_string()), cx);
    }

    /// No provider is ready to start a session: one line, and the way to the providers' settings.
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
                .aria_label(SharedString::from(why.join(" ")))
                .px(space::MD)
                .pt(space::SM)
                .pb(space::XS)
                .flex()
                .items_center()
                .gap(space::XS)
                .text_size(px(12.5))
                .text_color(p.mute)
                .child("No provider is ready.")
                .child(hoverable(
                    div()
                        .id("open-settings")
                        .cursor_pointer()
                        .medium()
                        .text_color(p.text)
                        .child("Open Settings")
                        .on_click(move |_, window, cx| {
                            shell
                                .update(cx, |shell, cx| {
                                    shell.open_settings_at(Reveal::Providers, window, cx)
                                })
                                .ok();
                        }),
                    move |d, t| d.text_color(mix(p.text, p.acc, t)),
                )),
        )
    }

    fn place_items(&self, cx: &mut Context<Self>) -> Vec<MenuItem> {
        let item = |id: String,
                    glyph: Icon,
                    label: String,
                    note: Option<&'static str>,
                    choice: Option<Choice>| {
            let entry = MenuItem::new(id, label)
                .leading(glyph)
                .selected(choice.as_ref() == Some(&self.choice));
            let entry = match note {
                Some(note) => entry.note(note),
                None => entry,
            };
            match choice {
                Some(choice) => entry.on_click(Self::handler(cx, move |this, window, cx| {
                    this.choose(choice.clone(), window, cx)
                })),
                None => entry,
            }
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
        // Not a place: it opens the page's new-workspace modal, the one its own button opens.
        items.push(
            MenuItem::new("place-choice-new", "New workspace…")
                .leading(Icon::Plus)
                .on_click(Self::handler(cx, |this, window, cx| {
                    this.menu = None;
                    let form = this
                        .shell
                        .update(cx, |shell, _| {
                            shell.new_workspace_of(&this.project).cloned()
                        })
                        .ok()
                        .flatten();
                    if let Some(form) = form {
                        form.update(cx, |form, cx| form.open_for_composer(window, cx));
                    }
                    cx.notify();
                })),
        );
        items
    }
}

/// What the model and effort pickers show and offer for a pick: the start composer's and the
/// session page's are these, never a second set.
#[derive(Clone, Copy)]
pub struct Pickers<'a> {
    pub providers: &'a Providers,
    pub pick: &'a Pick,
    pub project: ProjectDefault<'a>,
}

impl Pickers<'_> {
    /// The project's model, when a session on the provider picked takes it.
    pub fn projects_model(&self) -> Option<&str> {
        let provider = self.pick.provider.as_deref()?;
        project_model(self.providers, self.project, provider)
    }

    /// What the model picker shows: the model's name, the project's model marked so. Without a
    /// model, the one Flottille applies, by its name; before a session said the models, that they
    /// are not known yet, never a "default" it cannot name.
    pub fn model_shown(&self) -> String {
        let provider = self.provider();
        let chosen = || crate::providers::MODEL_UNKNOWN.to_owned();
        let label = |id: &str| match provider.and_then(|p| p.model(id)) {
            Some(model) => named(provider, model).unwrap_or_else(chosen),
            None if id == DEFAULT => chosen(),
            None => id.to_owned(),
        };
        match &self.pick.model {
            Some(model) if self.projects_model() == Some(model.as_str()) => {
                format!("{} · the project's", label(model))
            }
            Some(model) => label(model),
            None => provider
                .and_then(Provider::default_model)
                .and_then(|default| named(provider, default))
                .unwrap_or_else(chosen),
        }
    }

    /// The effort picked. A `default` that a daemon reported or a store kept is none: the
    /// model's own effort is shown by its name, as the model's label is.
    pub fn picked_effort(&self) -> Option<&String> {
        self.pick.effort.as_ref().filter(|e| e.as_str() != DEFAULT)
    }

    /// The effort the model picked (the provider's default one when none) runs with when none
    /// is asked: the one a session said, else the middle of its levels, the one Flottille
    /// applies; `None` until a session said its efforts.
    pub fn own_effort(&self) -> Option<&String> {
        let provider = self.provider()?;
        let model = match self.pick.model.as_deref() {
            Some(model) => provider.model(model)?,
            None => provider.default_model()?,
        };
        let efforts = efforts_of(provider, self.pick.model.as_deref())?;
        let said = model
            .default_effort
            .as_ref()
            .filter(|own| levels(efforts).any(|e| e == *own));
        said.or_else(|| flottille_core::provider::middle_effort(efforts))
    }

    /// The provider picked's title, "the provider" before the daemon listed it.
    pub fn title(&self) -> String {
        self.provider()
            .map_or_else(|| "the provider".to_owned(), |p| p.title.clone())
    }

    pub fn provider(&self) -> Option<&Provider> {
        self.providers.get(self.pick.provider.as_deref()?)
    }
}

/// The one control that picks the agent, provider, model and effort, for `pickers`' pick: the
/// composer that starts a session and a session's Reply dock build it here, never twice. With
/// `own` the provider of the pick is the only one offered (a session never changes its provider).
/// `None` while no provider is picked or offered, or when it offers no model. A model the
/// provider does not list (the project's) is offered too, so that the picker says what runs.
pub fn agent_picker(
    id: &'static str,
    pickers: &Pickers,
    own: bool,
    on_choose: impl Fn(Chosen, Option<SharedString>, &mut Window, &mut App) + 'static,
) -> Option<ModelPicker> {
    let provider = pickers.provider().filter(|p| p.offered())?;
    let projects = pickers
        .projects_model()
        .map(|model| (provider.name.as_str(), model));
    let mut models = pickers
        .providers
        .agent_entries(own.then_some(provider.name.as_str()), projects);
    let model = pickers
        .pick
        .model
        .as_deref()
        .filter(|model| *model != DEFAULT)
        .map(|model| provider.model(model).map_or(model, |m| m.id.as_str()));
    if let Some(model) = model
        && provider.model(model).is_none()
    {
        models.push(ModelEntry::new(
            provider.name.clone(),
            provider.title.clone(),
            Some(model.to_owned().into()),
            pickers.model_shown(),
        ));
    }
    if models.is_empty() {
        return None;
    }
    Some(
        ModelPicker::new(id, pickers.providers.chosen(&provider.name, model))
            .models(models)
            .effort(pickers.picked_effort().cloned().map(Into::into))
            .on_choose(on_choose),
    )
}

/// The provider's own `default` entry, a model or an effort: the window shows what it stands for,
/// or that the provider chooses.
pub const DEFAULT: &str = "default";

/// A model's name, its provider's `default` named by the model Flottille applies for it.
pub(crate) fn named(
    provider: Option<&Provider>,
    model: &crate::providers::Model,
) -> Option<String> {
    if model.id != DEFAULT {
        return Some(model.label());
    }
    Some(provider?.default_model()?.label())
}

/// A model's effort levels, less the provider's own `default`: the picker's row that asks none
/// stands for it.
pub fn levels(efforts: &[String]) -> impl Iterator<Item = &String> {
    efforts.iter().filter(|e| e.as_str() != DEFAULT)
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
        // The machine in front is off: what it says, and the way out, which is the picker.
        let machine_off = crate::machines::held(cx)
            .filter(|holding| holding.offline)
            .map(|holding| {
                let (name, state) = holding.line.split_once(' ').unwrap_or((&holding.line, ""));
                format!("{name} is {state}: pick another machine, or wait for it.")
            });
        self.machine_off = machine_off.is_some();
        let focus = fade("composer-focus", self.focused, HOVER, window, cx);
        self.attached.place(window, cx);
        let strip = self.attached.strip(window, cx, |this, at, window, cx| {
            this.detach(at, window, cx)
        });
        let body = div()
            .id("composer-field")
            .px(space::LG)
            .py(space::MD)
            .text_size(px(14.))
            .line_height(px(21.))
            .cursor_text()
            .child(self.tokens.clone())
            .on_click(cx.listener(|this, _, window, cx| {
                let focus = this.field.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }))
            .test_support();
        Attachments::taken_by(
            div().id("composer").relative().w_full(),
            cx,
            Self::attached_of,
        )
        .p(space::SM)
        .rounded(px(16.))
        .bg(p.shell)
        .border_1()
        .border_color(p.line)
        .shadow_lg()
        .ui_text()
        .text_color(p.text)
        .test_support()
        .child(strip)
        .child(
            div()
                .rounded(px(11.))
                .bg(p.field)
                .border_1()
                .border_color(mix(p.field_line, p.focus_line, focus))
                .when(focus > 0.0, |d| {
                    d.shadow(vec![gpui_kit::BoxShadow {
                        color: p.focus_ring.opacity(focus),
                        offset: gpui_kit::point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(3.),
                        inset: false,
                    }])
                })
                .overflow_hidden()
                .child(body),
        )
        .child(self.render_pickers(window, cx).into_any_element())
        .children(self.render_not_ready(cx))
        .child(presence_of(
            "composer-machine-off-line",
            machine_off.map(SharedString::from),
            move |why| {
                div()
                    .id("composer-machine-off")
                    .aria_label(why.clone())
                    .px(space::MD)
                    .pt(space::SM)
                    .pb(space::XS)
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child(why)
                    .test_support()
                    .into_any_element()
            },
        ))
        .child(presence_of(
            "composer-model-refused-line",
            self.model_refused().map(SharedString::from),
            move |why| {
                div()
                    .id("composer-model-refused")
                    .px(space::MD)
                    .pt(space::SM)
                    .pb(space::XS)
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child(why)
                    .into_any_element()
            },
        ))
        .when_some(self.blocked.clone(), |d, why| {
            d.child(
                div()
                    .id("composer-blocked")
                    .px(space::MD)
                    .pt(space::SM)
                    .pb(space::XS)
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child(SharedString::from(why)),
            )
        })
        .child(presence_of(
            "composer-error-line",
            self.error.clone(),
            move |error| {
                div()
                    .id("composer-error")
                    .px(space::MD)
                    .pt(space::SM)
                    .pb(space::XS)
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(error)
                    .into_any_element()
            },
        ))
        .when(self.sending.is_some(), |d| {
            d.child(enter(
                "composer-starting",
                0,
                div()
                    .px(space::MD)
                    .pt(space::SM)
                    .pb(space::XS)
                    .mono(11.5)
                    .text_color(p.faint)
                    .child("Starting…"),
            ))
        })
    }
}
