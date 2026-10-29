//! Hover and press, shared by every clickable component: the hover background fades in over
//! [`HOVER`], a button shrinks a little over [`PRESS`] while it is held.
//!
//! The state lives in the element's keyed state; it is touched only by the pointer, so nothing
//! here runs at rest.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, Context, Div, Element, ElementId, Entity, Global, InteractiveElement, IntoElement,
    MouseButton, NavigationDirection, RenderOnce, Stateful, StatefulInteractiveElement,
    TestSupportExt, Window,
};

use crate::motion::{HOVER, Motion, PANEL, PRESS, PRESSED};

/// The mouse's back and forward buttons dismiss what `element` belongs to, as a click outside
/// it does: an open list, a modal, a drawer. The window's history does not see them.
pub fn dismiss_on_navigate<E: InteractiveElement>(
    element: E,
    dismiss: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> E {
    [NavigationDirection::Back, NavigationDirection::Forward]
        .into_iter()
        .fold(element, |element, direction| {
            let dismiss = dismiss.clone();
            element.on_mouse_down(MouseButton::Navigate(direction), move |_, window, cx| {
                cx.stop_propagation();
                dismiss(window, cx);
            })
        })
}

/// The overlays drawn open in this frame, in drawing order, each with how it is dismissed. A key
/// has no pointer to say which one it is meant for, and no focus inside the overlay: the window's
/// root asks for the topmost.
#[derive(Default)]
struct Overlays(Vec<(ElementId, Dismiss)>);

type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

impl Global for Overlays {}

/// Called by the window's root before it draws a frame: the overlays that are open say so again.
pub fn forget_overlays(cx: &mut App) {
    cx.default_global::<Overlays>().0.clear();
}

/// An open overlay says, as it is drawn, how it is dismissed. One that is not drawn open (closed,
/// or closing) does not say it.
pub fn open_overlay(
    id: ElementId,
    dismiss: impl Fn(&mut Window, &mut App) + 'static,
    cx: &mut App,
) {
    let open = &mut cx.default_global::<Overlays>().0;
    open.retain(|(known, _)| known != &id);
    open.push((id, Rc::new(dismiss)));
}

/// Dismisses the topmost open overlay, as the mouse's back and forward buttons do. Whether there
/// was one.
pub fn dismiss_top_overlay(window: &mut Window, cx: &mut App) -> bool {
    let Some((_, dismiss)) = cx.default_global::<Overlays>().0.last().cloned() else {
        return false;
    };
    dismiss(window, cx);
    true
}

/// An element of a screen with a hover that fades in over [`HOVER`], as every button's does.
/// `paint` gets the element and how far the hover is (0 to 1) and sets what changes with it: a
/// background (`p.hover.opacity(t)`), an ink (`mix(from, to, t)`). GPUI's own `.hover(..)`
/// switches at once, which is never used in the window. The element needs an id.
pub fn hoverable(
    element: Stateful<Div>,
    paint: impl FnOnce(Stateful<Div>, f32) -> Stateful<Div> + 'static,
) -> Hoverable {
    Hoverable {
        element,
        chosen: None,
        paint: Box::new(move |element, hover, _| paint(element, hover)),
    }
}

/// [`hoverable`] for an element that is also one of a set that can be the chosen one (a selected
/// row): `paint` also gets how far it is the chosen one (0 to 1), which moves over [`PANEL`]
/// when `chosen` changes.
pub fn selectable(
    element: Stateful<Div>,
    chosen: bool,
    paint: impl FnOnce(Stateful<Div>, f32, f32) -> Stateful<Div> + 'static,
) -> Hoverable {
    Hoverable {
        element,
        chosen: Some(chosen),
        paint: Box::new(paint),
    }
}

#[derive(IntoElement)]
pub struct Hoverable {
    element: Stateful<Div>,
    chosen: Option<bool>,
    paint: Box<dyn FnOnce(Stateful<Div>, f32, f32) -> Stateful<Div>>,
}

impl RenderOnce for Hoverable {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = Element::id(&self.element).expect("a hoverable element has an id");
        let (state, sample) = feel(&id, window, cx);
        let chosen = self.chosen.map_or(0.0, |on| choose(&state, on, window, cx));
        wire(
            (self.paint)(self.element, sample.hover, chosen),
            state,
            false,
        )
        .test_support()
    }
}

pub(crate) struct Feel {
    hover: Motion,
    press: Motion,
    /// How far a choice button is the chosen one; unset for a button that is not a choice.
    chosen: Option<Motion>,
}

/// The hover (0 to 1) and press scale (1 to [`PRESSED`]) to draw now.
#[derive(Clone, Copy)]
pub(crate) struct Sample {
    pub hover: f32,
    pub scale: f32,
}

pub(crate) fn feel(id: &ElementId, window: &mut Window, cx: &mut App) -> (Entity<Feel>, Sample) {
    let key = ElementId::NamedChild(Arc::new(id.clone()), "feel".into());
    let state = window.use_keyed_state(key, cx, |_, _: &mut Context<Feel>| Feel {
        hover: Motion::settled(0.0),
        press: Motion::settled(1.0),
        chosen: None,
    });
    let now = cx.background_executor().now();
    let feel = state.read(cx);
    if feel.hover.is_running(now)
        || feel.press.is_running(now)
        || feel.chosen.is_some_and(|chosen| chosen.is_running(now))
    {
        window.request_animation_frame();
    }
    let sample = Sample {
        hover: feel.hover.value(now),
        scale: feel.press.value(now),
    };
    (state, sample)
}

/// How far a state is on (0 to 1), moving over `duration` when `on` changes: a focus ring, a
/// highlight. `id` keeps the state; a state first drawn on is simply on.
pub fn fade(
    id: impl Into<ElementId>,
    on: bool,
    duration: std::time::Duration,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let key = ElementId::NamedChild(Arc::new(id.into()), "fade".into());
    let state = window.use_keyed_state(key, cx, move |_, _| {
        Motion::settled(if on { 1.0 } else { 0.0 })
    });
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    let target = if on { 1.0 } else { 0.0 };
    let motion = state.update(cx, |motion, _| {
        if motion.target() != target {
            motion.animate(target, duration, now, reduce);
        }
        *motion
    });
    if motion.is_running(now) {
        window.request_animation_frame();
    }
    motion.value(now)
}

/// Moves a choice button toward `chosen` over [`PANEL`] and says how far it is now (0 to 1). A
/// button first drawn chosen is simply chosen.
pub(crate) fn choose(feel: &Entity<Feel>, chosen: bool, window: &mut Window, cx: &mut App) -> f32 {
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    let target = if chosen { 1.0 } else { 0.0 };
    let motion = feel.update(cx, |feel, _| {
        let motion = feel.chosen.get_or_insert(Motion::settled(target));
        if motion.target() != target {
            motion.animate(target, PANEL, now, reduce);
        }
        *motion
    });
    if motion.is_running(now) {
        window.request_animation_frame();
    }
    motion.value(now)
}

/// Listens to the pointer on `element`, driving `feel`.
pub(crate) fn wire(element: Stateful<Div>, feel: Entity<Feel>, presses: bool) -> Stateful<Div> {
    let on_hover = feel.clone();
    let element = element.on_hover(move |hovered, _, cx| {
        let target = if *hovered { 1.0 } else { 0.0 };
        update(&on_hover, cx, |f, now, reduce| {
            f.hover.animate(target, HOVER, now, reduce)
        });
    });
    if !presses {
        return element;
    }
    let down = feel.clone();
    let up = feel.clone();
    element
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            update(&down, cx, |f, now, reduce| {
                f.press.animate(PRESSED, PRESS, now, reduce)
            });
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            update(&up, cx, |f, now, reduce| {
                f.press.animate(1.0, PRESS, now, reduce)
            });
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            update(&feel, cx, |f, now, reduce| {
                f.press.animate(1.0, PRESS, now, reduce)
            });
        })
}

fn update(feel: &Entity<Feel>, cx: &mut App, f: impl FnOnce(&mut Feel, std::time::Instant, bool)) {
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    feel.update(cx, |feel, cx| {
        f(feel, now, reduce);
        cx.notify();
    });
}
