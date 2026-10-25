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

use crate::actions::{Command, Target};
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
    menu: bool,
    running: Vec<RunningSession>,
    icons: HashMap<String, Entity<StateIcon>>,
    sending: bool,
    error: Option<SharedString>,
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
    pub fn new(shell: WeakEntity<Shell>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        Self {
            shell,
            project: String::new(),
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
            menu: false,
            running: Vec::new(),
            icons: HashMap::new(),
            sending: false,
            error: None,
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

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_sending(&self) -> bool {
        self.sending
    }

    pub fn field(&self) -> &Entity<TextareaState> {
        &self.field
    }

    pub fn name_field(&self) -> &Entity<InputState> {
        &self.name
    }

    /// The sessions the "Running agents" tab lists, in order.
    pub fn running(&self) -> &[RunningSession] {
        &self.running
    }

    /// The project it starts sessions in; another project starts from `main`, with no error.
    pub fn set_project(&mut self, project: &str, cx: &mut Context<Self>) {
        if self.project != project {
            self.project = project.to_owned();
            self.choice = Choice::Main;
            self.menu = false;
            self.error = None;
            cx.notify();
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
            cx.notify();
        }
    }

    pub fn set_running(&mut self, running: Vec<RunningSession>, cx: &mut Context<Self>) {
        if self.running != running {
            self.icons
                .retain(|id, _| running.iter().any(|s| &s.id == id));
            for session in &running {
                let status = session.status.clone();
                self.icons
                    .entry(session.id.clone())
                    .or_insert_with(|| cx.new(|cx| StateIcon::new(status, cx)));
            }
            self.running = running;
            cx.notify();
        }
    }

    /// Picks where the next session starts, and puts the caret in the field.
    pub fn choose(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = false;
        self.tab = Tab::NewTask;
        self.slide_to(0.0, cx);
        if self.choice != choice {
            self.choice = choice;
            self.error = None;
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
            self.menu = false;
            self.slide_to(if tab == Tab::NewTask { 0.0 } else { 1.0 }, cx);
            cx.notify();
        }
    }

    fn slide_to(&mut self, target: f32, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.slide.animate(target, PANEL, now, cx.reduce_motion());
    }

    fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = !self.menu;
        cx.notify();
    }

    fn can_send(&self) -> bool {
        !self.sending && self.has_text && (self.choice != Choice::New || self.has_name)
    }

    /// Starts a session with what the field holds, where the picker says.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        if !self.can_send() || self.project.is_empty() {
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
            target,
            message,
        };
        self.sending = true;
        self.error = None;
        self.menu = false;
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// The session started: the field is emptied for the next one.
    pub fn started(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sending = false;
        self.error = None;
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.has_text = false;
        if self.choice == Choice::New {
            self.name
                .update(cx, |name, cx| name.set_value("", window, cx));
            self.has_name = false;
            self.choice = Choice::Main;
        }
        cx.notify();
    }

    /// No session started: the field keeps the message, and says why.
    pub fn not_started(&mut self, why: &str, cx: &mut Context<Self>) {
        self.sending = false;
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
        let agents: usize = self.running.iter().map(|s| s.agents).sum();
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
        let tab = |index: usize, tab: Tab, label: SharedString, count: Option<usize>| {
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
                .when_some(count, |d, n| {
                    d.child(div().mono(11.).text_color(p.mute).child(n.to_string()))
                })
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
            .child(tab(1, Tab::Running, "Running agents".into(), Some(agents)))
    }

    fn render_running(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let list = div().id("running-list").p(px(6.)).flex().flex_col();
        if self.running.is_empty() {
            return list.child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_color(p.faint)
                    .child("Nothing running in this project."),
            );
        }
        list.children(self.running.iter().enumerate().map(|(index, session)| {
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
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(cx)))
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
            .child(
                div()
                    .id("agent-picker")
                    .h(px(28.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .medium()
                    .child("Claude")
                    .child(div().text_color(p.faint).child("· default model")),
            )
            .child(
                Button::round("send", Icon::Send, "Start")
                    .disabled(!can_send)
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            )
            .when(self.menu, |d| {
                d.child(self.render_menu(cx).into_any_element())
            })
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
            Tab::Running => self.render_running(cx).into_any_element(),
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
            .when(self.sending, |d| {
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
