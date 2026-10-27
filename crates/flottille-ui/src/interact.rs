//! Hover and press, shared by every clickable component: the hover background fades in over
//! [`HOVER`], a button shrinks a little over [`PRESS`] while it is held.
//!
//! The state lives in the element's keyed state; it is touched only by the pointer, so nothing
//! here runs at rest.

use std::sync::Arc;

use gpui_kit::{
    App, Context, Div, ElementId, Entity, InteractiveElement, MouseButton, NavigationDirection,
    Stateful, StatefulInteractiveElement, Window,
};

use crate::motion::{HOVER, Motion, PRESS, PRESSED};

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

pub(crate) struct Feel {
    hover: Motion,
    press: Motion,
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
    });
    let now = cx.background_executor().now();
    let feel = state.read(cx);
    if feel.hover.is_running(now) || feel.press.is_running(now) {
        window.request_animation_frame();
    }
    let sample = Sample {
        hover: feel.hover.value(now),
        scale: feel.press.value(now),
    };
    (state, sample)
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
