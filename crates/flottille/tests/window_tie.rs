//! The connector of a led row (#740), on GPUI's test platform with rows of their own and no
//! daemon: where its turn meets a row, whatever the row's height. No window opens on a screen.

mod kit_common;

use kit_common::{Desktop, init};

use flottille_window::state::AgentState;
use flottille_window::tree::{SessionRow, session_id, session_mark_id};
use gpui_design_system::{Tie, tie_below_id, tie_turn_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, WeakEntity, Window, div, px,
};
use std::time::Duration;

struct Column(Vec<Entity<SessionRow>>);

impl Render for Column {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(280.))
            .flex()
            .flex_col()
            .children(self.0.iter().cloned())
    }
}

/// Two rows tied to a lead above them, the first with a sibling after it, the second last; the
/// second says `scope`, as a row of another project's does.
fn column<'a>(
    scope: Option<&str>,
    cx: &'a mut TestAppContext,
) -> (Vec<String>, &'a mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let ids = vec!["first".to_owned(), "second".to_owned()];
    let scope = scope.map(str::to_owned);
    let rows = ids.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        let rows = rows
            .iter()
            .enumerate()
            .map(|(at, id)| {
                cx.new(|cx| {
                    let mut row = SessionRow::new(id.clone(), WeakEntity::new_invalid(), cx);
                    row.set(id, Some(AgentState::Running), 0, 0, cx);
                    row.set_tie(
                        Some(Tie {
                            through: Vec::new(),
                            last: at == 1,
                        }),
                        cx,
                    );
                    if at == 1 {
                        row.set_scope(scope.as_deref(), cx);
                    }
                    row
                })
            })
            .collect();
        Column(rows)
    });
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    (ids, cx)
}

fn bounds(
    cx: &mut VisualTestContext,
    id: gpui_kit::ElementId,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update(|window, _| window.find(id).bounds())
}

#[gpui_kit::test]
fn a_scoped_row_s_tie_turns_at_its_first_line_too(cx: &mut TestAppContext) {
    let (ids, cx) = column(Some("atlas"), cx);
    let row = bounds(cx, session_id(&ids[1]));
    let turn = bounds(cx, tie_turn_id(session_id(&ids[1])));
    let mark = bounds(cx, session_mark_id(&ids[1]));
    assert_eq!(row.size.height, px(28.), "a scoped row is one line high");
    assert_eq!(turn.top(), row.top());
    assert!(
        f32::from(turn.bottom() - px(0.5) - mark.center().y).abs() <= 1.,
        "at the middle of the mark: {turn:?} {mark:?} {row:?}"
    );

    // The row above it is two lines high and its line goes on to this row's turn.
    let above = bounds(cx, session_id(&ids[0]));
    let below = bounds(cx, tie_below_id(session_id(&ids[0])));
    assert_eq!(above.size.height, px(40.));
    assert!(f32::from(row.top() - below.bottom()).abs() <= 0.5);
}
