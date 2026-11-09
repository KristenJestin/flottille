//! A setup step's ink, on GPUI's test platform: it fades when the step's state changes and then
//! stops asking for frames; with reduced motion it jumps.

mod kit_common;

use flottille_ui::motion::PANEL;
use flottille_window::steps::{StepRow, StepTone};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    Context, IntoElement, ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window,
    div, px,
};
use kit_common::{Desktop, advance, init, pending_frames};

struct Stepping {
    tone: StepTone,
}

impl Render for Stepping {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(StepRow::new(
            "step",
            div().size(px(8.)),
            "Run",
            self.tone,
            "step-time",
        ))
    }
}

fn step_tone(view: &gpui_kit::Entity<Stepping>, tone: StepTone, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.tone = tone;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
}

#[gpui_kit::test]
fn a_setup_step_whose_state_changes_fades_its_ink_and_then_stops(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Stepping {
        tone: StepTone::Todo,
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL);
    assert_eq!(pending_frames(cx), 0, "a step first drawn is just there");

    for tone in [StepTone::Now, StepTone::Done, StepTone::Failed] {
        step_tone(&view, tone, cx);
        assert!(pending_frames(cx) > 0, "its ink moves toward {tone:?}");
        advance(cx, PANEL * 2);
        assert_eq!(pending_frames(cx), 0, "and stops once it is {tone:?}");
    }
}

#[gpui_kit::test]
fn with_reduced_motion_a_setup_step_s_ink_jumps(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, _| Stepping {
        tone: StepTone::Todo,
    });
    cx.update(|window, cx| window.render_frame(cx));
    step_tone(&view, StepTone::Now, cx);
    assert_eq!(pending_frames(cx), 0);
}
