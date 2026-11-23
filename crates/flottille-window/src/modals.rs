//! The window's one host of dialogs (#1018, `docs/design.md` §10): a dialog opens over the page it
//! is asked from, whichever page that is. A dialog is a form or a review that belongs to a
//! section (the vault's entry, a connection's server, its review) or a view of its own (an
//! agent's request); the host draws every one of them above the page, in the order they were
//! opened, so that a dialog that opens another has it stacked over it. Each is the kit's `Modal`,
//! which fades and rises when it opens and the other way when it closes, over a scrim of its own;
//! Escape and Cancel close the top one (the window's root asks the kit for the topmost open
//! overlay), and the focus goes back to the one below, where the user was.
//!
//! A dialog keeps the state of its form where it always was (its section's entity: the fields,
//! what the daemon answered), and says whether it is open; the host holds the order and draws.
//! Opening is `Modals::open`, called by whatever fills the form: a page's button and a request's
//! Edit go through the same call, whichever page is shown.

use std::rc::Rc;

use gpui_design_system::Modal;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};

/// What gives a dialog's focus handle, when it has a field to take the focus.
type Focus = Rc<dyn Fn(&App) -> Option<FocusHandle>>;

/// How a dialog is drawn: the view that holds it and the answers about its state.
struct Slot {
    /// The id of its modal: what a test and `Modals::open` name it by.
    id: &'static str,
    view: AnyView,
    open: Rc<dyn Fn(&App) -> bool>,
    /// What takes the focus when it is the top one again, if it has a field to give it to.
    focus: Focus,
    /// Whether it was open when the host was last drawn: one that closed gives the focus back.
    was_open: bool,
}

/// The dialogs, the first opened first and the one on top last.
pub struct Modals {
    slots: Vec<Slot>,
    /// The window's own focus: where the keys go when the dialog on top has no field to take them.
    fallback: Option<FocusHandle>,
    _watching: Vec<Subscription>,
}

/// The modal of a section, drawn by the host: a view of its own that asks the section for it.
struct Part<S: 'static> {
    section: Entity<S>,
    draw: fn(&mut S, &mut Window, &mut Context<S>) -> Modal,
    _watching: Subscription,
}

impl<S: 'static> Render for Part<S> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draw = self.draw;
        self.section
            .update(cx, |section, cx| draw(section, window, cx))
    }
}

impl Modals {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            fallback: None,
            _watching: Vec::new(),
        }
    }

    /// A modal that belongs to the entity `section`: `draw` makes it each frame (open or closing),
    /// `open` says whether it is open, `focus` where the focus goes when it is on top again.
    pub fn add<S: 'static>(
        &mut self,
        id: &'static str,
        section: &Entity<S>,
        draw: fn(&mut S, &mut Window, &mut Context<S>) -> Modal,
        open: fn(&S) -> bool,
        focus: fn(&S, &App) -> Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        let view = {
            let section = section.clone();
            cx.new(|cx| Part {
                _watching: cx.observe(&section, |_, _, cx| cx.notify()),
                section,
                draw,
            })
        };
        let (opened, focused) = (section.clone(), section.clone());
        self.slots.push(Slot {
            id,
            view: view.into(),
            open: Rc::new(move |cx| open(opened.read(cx))),
            focus: Rc::new(move |cx| focus(focused.read(cx), cx)),
            was_open: false,
        });
        self._watching
            .push(cx.observe(section, |_, _, cx| cx.notify()));
    }

    /// A view that draws its own modal (`view`): `open` says whether it is.
    pub fn add_view<V: Render>(
        &mut self,
        id: &'static str,
        view: &Entity<V>,
        open: fn(&V) -> bool,
        cx: &mut Context<Self>,
    ) {
        let opened = view.clone();
        self.slots.push(Slot {
            id,
            view: view.clone().into(),
            open: Rc::new(move |cx| open(opened.read(cx))),
            focus: Rc::new(|_| None),
            was_open: false,
        });
        self._watching
            .push(cx.observe(view, |_, _, cx| cx.notify()));
    }

    /// The focus a closed dialog gives to the one below when that one has no field of its own.
    pub fn fallback(&mut self, focus: FocusHandle) {
        self.fallback = Some(focus);
    }

    /// The dialog `id` is being opened: it goes on top of the ones already open.
    /// Each machine's Settings has its own sections, and so its own dialog of the same id: they
    /// all go on top, and the one whose section opened it is drawn.
    pub fn open(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let (opened, others) = std::mem::take(&mut self.slots)
            .into_iter()
            .partition::<Vec<_>, _>(|slot| slot.id == id);
        self.slots = others;
        if !opened.is_empty() {
            self.slots.extend(opened);
            cx.notify();
        }
    }

    /// How many dialogs are held, open or not.
    pub fn held(&self) -> usize {
        self.slots.len()
    }

    /// The open dialogs by their id, the first opened first, the top one last.
    pub fn stack(&self, cx: &App) -> Vec<&'static str> {
        self.slots
            .iter()
            .filter(|slot| (slot.open)(cx))
            .map(|slot| slot.id)
            .collect()
    }
}

impl Default for Modals {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for Modals {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A dialog that closed gives the focus back to the one that is on top now: the user is
        // where they were, and Escape goes on closing the one in front.
        let mut closed = false;
        for slot in &mut self.slots {
            let open = (slot.open)(cx);
            closed |= slot.was_open && !open;
            slot.was_open = open;
        }
        if closed
            && let Some(focus) = self
                .slots
                .iter()
                .rev()
                .find(|slot| slot.was_open)
                .map(|slot| slot.focus.clone())
        {
            let fallback = self.fallback.clone();
            window.defer(cx, move |window, cx| {
                if let Some(handle) = focus(cx).or(fallback) {
                    window.focus(&handle, cx);
                }
            });
        }
        div()
            .absolute()
            .children(self.slots.iter().map(|slot| slot.view.clone()))
    }
}
