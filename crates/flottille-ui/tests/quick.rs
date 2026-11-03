//! `QuickPanel` and `QuickRow` on GPUI's test platform: the panel rises and fades in and out and
//! is gone once it has left, Escape and a click outside close it, a row's marked fill moves and
//! the pointer marks the row it enters, and nothing is drawn at rest.

mod common;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{MODAL, PANEL};
use flottille_ui::{QuickPanel, QuickRow};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, ParentElement, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};

struct Switch {
    open: bool,
    marked: usize,
    closed: usize,
    clicked: Vec<usize>,
    focus: FocusHandle,
}

impl Render for Switch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let rows = (0..3).map(|at| {
            let (click, mark) = (this.clone(), this.clone());
            QuickRow::new(format!("row-{at}"), format!("Result {at}"))
                .pieces(vec![("Res".into(), true), (format!("ult {at}"), false)])
                .line("harbor · main")
                .marked(self.marked == at)
                .on_click(move |_, _, cx| {
                    click
                        .update(cx, |this, cx| {
                            this.clicked.push(at);
                            cx.notify();
                        })
                        .ok();
                })
                .on_mark(move |cx| {
                    mark.update(cx, |this, cx| {
                        this.marked = at;
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element()
        });
        let close = this.clone();
        div().size_full().child(
            QuickPanel::new("quick")
                .open(self.open)
                .on_close(move |_, cx| {
                    close
                        .update(cx, |this, cx| {
                            this.open = false;
                            this.closed += 1;
                            cx.notify();
                        })
                        .ok();
                })
                .field(
                    div()
                        .id("quick-field")
                        .track_focus(&self.focus)
                        .child("field"),
                )
                .results(rows),
        )
    }
}

fn switch(cx: &mut TestAppContext) -> (Entity<Switch>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (switch, cx) = cx.add_window_view(|_, cx| Switch {
        open: false,
        marked: 0,
        closed: 0,
        clicked: Vec::new(),
        focus: cx.focus_handle(),
    });
    cx.simulate_resize(gpui_kit::size(px(1000.), px(700.)));
    (switch, cx)
}

fn open(switch: &Entity<Switch>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        switch.update(cx, |s, cx| {
            s.open = true;
            cx.notify();
            window.focus(&s.focus, cx);
        });
        window.render_frame(cx);
    });
}

fn top(cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, _| window.find("quick").bounds().top())
}

#[gpui_kit::test]
fn the_panel_rises_in_sits_near_the_top_and_rests(cx: &mut TestAppContext) {
    let (switch, cx) = switch(cx);
    cx.update(|window, _| assert!(window.try_find("quick").is_none(), "closed: not drawn"));
    open(&switch, cx);
    let first = top(cx);
    advance(cx, MODAL / 2);
    let part_way = top(cx);
    advance(cx, MODAL * 2);
    let rested = top(cx);
    assert!(
        first > part_way && part_way > rested,
        "{first:?} {part_way:?} {rested:?}"
    );
    assert!(rested < px(200.), "near the top, not centred: {rested:?}");
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn at rest");
}

#[gpui_kit::test]
fn escape_and_a_click_outside_close_it_and_it_is_gone_once_it_has_left(cx: &mut TestAppContext) {
    let (switch, cx) = switch(cx);
    open(&switch, cx);
    advance(cx, MODAL * 2);
    cx.update(|window, cx| window.press("escape", cx));
    assert_eq!(switch.read_with(cx, |s, _| s.closed), 1);
    advance(cx, MODAL / 2);
    cx.update(|window, _| assert!(window.try_find("quick").is_some(), "still leaving"));
    advance(cx, MODAL * 2);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("quick").is_none());
    });
    assert_eq!(pending_frames(cx), 0);

    open(&switch, cx);
    advance(cx, MODAL * 2);
    cx.update(|window, cx| window.click("quick-scrim", cx));
    assert_eq!(switch.read_with(cx, |s, _| s.closed), 2);
}

#[gpui_kit::test]
fn the_pointer_marks_the_row_it_enters_and_a_click_chooses_it(cx: &mut TestAppContext) {
    let (switch, cx) = switch(cx);
    open(&switch, cx);
    advance(cx, MODAL * 2);
    cx.update(|window, cx| window.hover("row-2", cx));
    advance(cx, PANEL * 2);
    assert_eq!(switch.read_with(cx, |s, _| s.marked), 2);
    cx.update(|window, cx| window.click("row-1", cx));
    assert_eq!(switch.read_with(cx, |s, _| s.clicked.clone()), [1]);
    advance(cx, PANEL * 2);
    assert_eq!(pending_frames(cx), 0, "the fill rests too");
}

#[gpui_kit::test]
fn a_row_says_its_title_whole_whatever_is_marked_in_it(cx: &mut TestAppContext) {
    let (switch, cx) = switch(cx);
    open(&switch, cx);
    advance(cx, MODAL * 2);
    cx.update(|window, _| {
        assert_eq!(window.find("row-0").label(), Some("Result 0"));
    });
}
