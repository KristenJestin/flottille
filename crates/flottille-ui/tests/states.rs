//! The state vocabulary, on GPUI's test platform: "each state renders its icon and pill", "a
//! state change on a shared model updates every component reading it", "with nothing running or
//! waiting, no frame is requested after the animations end".

mod common;

use std::time::Duration;

use common::{Desktop, advance, count_redraws, init, pending_frames};
use flottille_ui::state::{icon_id, pill_id};
use flottille_ui::{AgentState, Pill, StateIcon, Status};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div,
};

/// A board of components all reading the same models.
struct Board {
    icons: Vec<Entity<StateIcon>>,
    pills: Vec<Entity<Pill>>,
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(self.icons.iter().cloned())
            .children(self.pills.iter().cloned())
    }
}

fn board<'a>(
    statuses: &[Entity<Status>],
    copies: usize,
    cx: &'a mut TestAppContext,
) -> (Entity<Board>, &'a mut VisualTestContext) {
    let statuses = statuses.to_vec();
    cx.add_window_view(move |_, cx| {
        let mut icons = Vec::new();
        let mut pills = Vec::new();
        for status in &statuses {
            for _ in 0..copies {
                icons.push(cx.new(|cx| StateIcon::new(status.clone(), cx)));
                pills.push(cx.new(|cx| Pill::new(status.clone(), cx)));
            }
        }
        Board { icons, pills }
    })
}

#[gpui_kit::test]
fn each_state_renders_its_icon_and_pill(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let statuses: Vec<_> = AgentState::ALL
        .iter()
        .map(|&state| cx.new(|_| Status::new(state)))
        .collect();
    let (_board, cx) = board(&statuses, 1, cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    for state in AgentState::ALL {
        let label = state.label().to_lowercase();
        let icon = cx.update(|window, _| window.find(icon_id(state)));
        assert_eq!(icon.label(), Some(label.as_str()), "{state:?} icon");
        let pill = cx.update(|window, _| window.find(pill_id(state)));
        assert_eq!(pill.label(), Some(label.as_str()), "{state:?} pill");
    }
}

#[gpui_kit::test]
fn a_state_change_on_a_shared_model_updates_every_component(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let status = cx.new(|_| Status::new(AgentState::Running));
    let (_board, cx) = board(std::slice::from_ref(&status), 3, cx);
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        cx.update(|w, _| w.find_all(icon_id(AgentState::Running)).len()),
        3
    );
    assert_eq!(
        cx.update(|w, _| w.find_all(pill_id(AgentState::Running)).len()),
        3
    );

    cx.update(|_, cx| status.update(cx, |s, cx| s.set(AgentState::NeedsYou, cx)));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        cx.update(|w, _| w.find_all(icon_id(AgentState::NeedsYou)).len()),
        3
    );
    assert_eq!(
        cx.update(|w, _| w.find_all(pill_id(AgentState::NeedsYou)).len()),
        3
    );
    assert!(
        cx.update(|w, _| w.try_find(icon_id(AgentState::Running)))
            .is_none()
    );
    assert!(
        cx.update(|w, _| w.try_find(pill_id(AgentState::Running)))
            .is_none()
    );
}

#[gpui_kit::test]
fn with_nothing_running_or_waiting_no_frame_is_requested_after_the_animations_end(
    cx: &mut TestAppContext,
) {
    init(cx, Desktop::new(false).env());
    let status = cx.new(|_| Status::new(AgentState::Running));
    let (board, cx) = board(std::slice::from_ref(&status), 1, cx);
    let (icon, pill) = board.read_with(cx, |b, _| (b.icons[0].clone(), b.pills[0].clone()));
    let icon_redraws = count_redraws(&icon, cx);

    // Running: the spinner turns, at a low rate.
    advance(cx, Duration::from_secs(1));
    let turning = icon_redraws.get();
    assert!(
        (10..=40).contains(&turning),
        "a running spinner redraws at a low rate, not every frame: {turning} in 1 s"
    );

    // Completed: the new icon and pill swap in, then everything stops.
    cx.update(|_, cx| status.update(cx, |s, cx| s.set(AgentState::Completed, cx)));
    advance(cx, Duration::from_millis(400));
    let pill_redraws = count_redraws(&pill, cx);
    let (icons_at_rest, pills_at_rest) = (icon_redraws.get(), pill_redraws.get());
    advance(cx, Duration::from_secs(5));
    assert_eq!(
        icon_redraws.get(),
        icons_at_rest,
        "the icon is redrawn at rest"
    );
    assert_eq!(
        pill_redraws.get(),
        pills_at_rest,
        "the pill is redrawn at rest"
    );
    assert_eq!(pending_frames(cx), 0, "a frame is requested at rest");
}
