//! `Subpage`: a list of rows that opens the page of one in place of itself, inside its tab.

mod common;

use common::subpage::Settings;
use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::PANEL;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

fn settings(cx: &mut TestAppContext) -> (Entity<Settings>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Settings::new());
    cx.update(|window, cx| window.render_frame(cx));
    (view, cx)
}

/// The frames after a change, so that what moves has arrived.
fn settle(cx: &mut VisualTestContext) {
    advance(cx, PANEL * 2);
    cx.update(|window, cx| window.render_frame(cx));
}

#[gpui_kit::test]
fn a_click_on_an_entry_opens_its_page_in_place_of_the_list(cx: &mut TestAppContext) {
    let (view, cx) = settings(cx);
    cx.update(|window, _| {
        assert!(
            window.try_find("feat-review").is_some(),
            "the list is there"
        );
        assert!(window.try_find("feat-specs").is_some());
        assert!(window.try_find("review-page").is_none());
    });

    cx.update(|window, cx| window.click("feat-review", cx));
    assert_eq!(
        view.read_with(cx, |v, _| v.told.clone()),
        vec![Some("review".into())]
    );
    settle(cx);
    cx.update(|window, _| {
        assert!(
            window.try_find("review-page").is_some(),
            "its page is shown"
        );
        assert!(
            window.try_find("feat-specs").is_none(),
            "in place of the list, not beside it"
        );
        assert!(window.try_find("feat-review").is_none());
        // The tabs above it stay.
        assert!(window.try_find("tabs-features").is_some());
    });
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn the_path_line_goes_back_to_the_list(cx: &mut TestAppContext) {
    let (view, cx) = settings(cx);
    cx.update(|window, cx| window.click("feat-review", cx));
    settle(cx);
    cx.update(|window, _| {
        assert_eq!(window.find("feat-path").label(), Some("Features / Review"));
    });

    cx.update(|window, cx| window.click("feat-back", cx));
    assert_eq!(view.read_with(cx, |v, _| v.open.clone()), None);
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find("feat-specs").is_some(), "the list is back");
        assert!(window.try_find("review-page").is_none());
        assert!(window.try_find("feat-back").is_none());
    });
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn escape_goes_back_to_the_list(cx: &mut TestAppContext) {
    let (view, cx) = settings(cx);
    cx.update(|window, cx| window.click("feat-specs", cx));
    settle(cx);
    cx.update(|window, cx| window.press("escape", cx));
    assert_eq!(view.read_with(cx, |v, _| v.open.clone()), None);
    settle(cx);
    cx.update(|window, _| assert!(window.try_find("feat-review").is_some()));
}

#[gpui_kit::test]
fn the_open_page_is_kept_across_a_tab_switch(cx: &mut TestAppContext) {
    let (_, cx) = settings(cx);
    cx.update(|window, cx| window.click("feat-connections", cx));
    settle(cx);

    cx.update(|window, cx| window.click("tabs-other", cx));
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find("other-panel").is_some());
        assert!(window.try_find("connections-page").is_none());
    });

    cx.update(|window, cx| window.click("tabs-features", cx));
    settle(cx);
    cx.update(|window, _| {
        assert!(
            window.try_find("connections-page").is_some(),
            "the page is as it was left"
        );
        assert!(window.try_find("feat-review").is_none());
    });
    assert_eq!(pending_frames(cx), 0, "and it is simply there");
}

#[gpui_kit::test]
fn an_entry_says_its_state_once_as_a_tag(cx: &mut TestAppContext) {
    let (_, cx) = settings(cx);
    cx.update(|window, _| {
        assert_eq!(window.find("feat-review-tag-0").label(), Some("On"));
        assert_eq!(window.find("feat-specs-tag-0").label(), Some("Off"));
        assert_eq!(window.find("feat-connections-tag-0").label(), Some("2 on"));
        for key in ["review", "specs", "connections"] {
            assert!(
                window.try_find(format!("feat-{key}-tag-1")).is_none(),
                "{key} has one tag"
            );
        }
        // The line says what it is for, never its state.
        assert_eq!(
            window.find("feat-review-line").label(),
            Some("Reads the changes before they are kept")
        );
    });
}
