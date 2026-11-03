//! One window (#302): what the open window does when it is asked to come to the front. On
//! GPUI's test platform, where activating a window is observable (the platform's active window),
//! with the real socket of `window_instance`; no window opens on a screen.

mod common;

use std::time::{Duration, Instant};

use common::User;
use flottille_core::paths::Paths;
use flottille_window::instance::{self, Start};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, IntoElement, Render, TestAppContext, Window, div,
};

struct Plain;

impl Render for Plain {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn active(cx: &mut TestAppContext) -> Option<AnyWindowHandle> {
    cx.update(|cx| cx.active_window())
}

#[gpui_kit::test]
fn a_raise_asked_of_the_open_window_makes_it_the_active_one(cx: &mut TestAppContext) {
    let user = User::new();
    let paths = Paths {
        runtime: user.path("runtime/flottille"),
        state: user.path("state/flottille"),
        config: user.path("config/flottille"),
        data: user.path("data/flottille"),
    };
    let open: AnyWindowHandle = cx.add_window(|_, _| Plain).into();
    let other: AnyWindowHandle = cx.add_window(|_, _| Plain).into();
    // Another window is in front of Flottille's.
    cx.update_window(other, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(active(cx), Some(other));
    let Start::First(instance) = instance::acquire(&paths).unwrap() else {
        panic!("another window is open");
    };
    cx.update_window(open, |_, window, cx| {
        flottille_window::raise_on_request(instance, window, cx)
    })
    .unwrap();

    // The second `flottille window`: it only says "raise" on the socket.
    assert!(matches!(instance::acquire(&paths).unwrap(), Start::Raised));

    cx.executor().allow_parking();
    let started = Instant::now();
    while active(cx) != Some(open) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the open window was not brought to the front"
        );
        std::thread::sleep(Duration::from_millis(20));
        cx.run_until_parked();
    }
}
