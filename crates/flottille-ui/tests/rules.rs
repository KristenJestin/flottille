//! The rules of the window (`docs/reference/gpui.md`, "Rules of the window") held by the
//! components that carry them, on GPUI's test platform: one button size, a confirmation that is
//! a dialog, a toast for an action's result, a label on every control, a separator only between
//! blocks that exist, the spacing scale.

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{ENTER, HOVER, MODAL, PANEL};
use flottille_ui::{
    Block, Button, Card, Confirm, Drawer, EmptyLine, Field, Icon, ListCard, ListRow, Menu,
    PageHeader, RadioChoices, Segmented, Select, SelectOption, Switch, Toast, ToastStack, space,
    toast,
};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Render, Styled, TestAppContext, TestSupportExt as _, VisualTestContext,
    Window, div, point, px, size,
};

type Draw = Box<dyn Fn(&mut Window, &mut Context<Host>) -> AnyElement>;

/// A window drawing whatever a scenario gives it.
struct Host {
    draw: Draw,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().relative().child((self.draw)(window, cx))
    }
}

fn host(
    cx: &mut TestAppContext,
    draw: impl Fn(&mut Window, &mut Context<Host>) -> AnyElement + 'static,
) -> (Entity<Host>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(move |_, _| Host {
        draw: Box::new(draw),
    });
    cx.simulate_resize(size(px(1000.), px(800.)));
    cx.update(|window, cx| window.render_frame(cx));
    (view, cx)
}

fn redraw(view: &Entity<Host>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        view.update(cx, |_, cx| cx.notify());
        window.render_frame(cx);
    });
}

fn bounds(id: &str, cx: &mut VisualTestContext) -> gpui_kit::Bounds<Pixels> {
    let id = id.to_owned();
    cx.update(|window, _| window.find(id).bounds())
}

fn shown(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
}

// ── 1. Buttons: four kinds, one size ───────────────────────────────────────────────────────────

#[gpui_kit::test]
fn every_text_button_is_30_px_high_whatever_its_kind(cx: &mut TestAppContext) {
    let (_, cx) = host(cx, |_, _| {
        div()
            .flex()
            .gap(space::SM)
            .child(Button::primary("primary", "Save"))
            .child(Button::secondary("secondary", "New workspace").leading(Icon::Plus))
            .child(Button::quiet("quiet", "Cancel"))
            .child(Button::danger("danger", "Remove task"))
            .child(Button::icon("icon", Icon::More, "More"))
            .into_any_element()
    });
    for kind in ["primary", "secondary", "quiet", "danger"] {
        assert_eq!(bounds(kind, cx).size.height, px(30.), "{kind}");
    }
    let icon = bounds("icon", cx).size;
    assert_eq!((icon.width, icon.height), (px(28.), px(28.)));
}

#[gpui_kit::test]
fn a_page_header_draws_its_title_its_path_and_its_actions_on_the_title_s_line(
    cx: &mut TestAppContext,
) {
    let kept: Rc<RefCell<Option<Entity<Menu>>>> = Rc::default();
    let (_, cx) = host(cx, move |_, cx| {
        let menu = kept
            .borrow_mut()
            .get_or_insert_with(|| cx.new(|cx| Menu::new("page-more", "More", cx)))
            .clone();
        PageHeader::new("page", "atlas")
            .path("~/work/atlas")
            .action("page-settings", "Settings", Some(Icon::Settings), |_, _| {})
            .action("page-new", "New workspace", Some(Icon::Plus), |_, _| {})
            .more(menu)
            .into_any_element()
    });
    cx.update(|window, _| {
        assert!(window.try_find_by_label("atlas").is_some());
        assert!(window.try_find_by_label("~/work/atlas").is_some());
    });
    let title = bounds("page-title", cx);
    for action in ["page-settings", "page-new", "page-more"] {
        let at = bounds(action, cx);
        assert_eq!(
            at.center().y,
            title.center().y,
            "{action} on the title's line"
        );
    }
    assert_eq!(bounds("page-settings", cx).size.height, px(30.));
}

#[gpui_kit::test]
#[should_panic(expected = "at most two")]
fn a_page_header_refuses_a_third_action(cx: &mut TestAppContext) {
    let (_, _cx) = host(cx, |_, _| {
        PageHeader::new("page", "atlas")
            .action("a", "One", None, |_, _| {})
            .action("b", "Two", None, |_, _| {})
            .action("c", "Three", None, |_, _| {})
            .into_any_element()
    });
}

// ── 2. One confirmation: a modal dialog ────────────────────────────────────────────────────────

#[derive(Default)]
struct Asked {
    open: Cell<bool>,
    confirmed: Cell<usize>,
    cancelled: Cell<usize>,
    refused: RefCell<Option<&'static str>>,
}

fn confirm(cx: &mut TestAppContext) -> (Rc<Asked>, Entity<Host>, &mut VisualTestContext) {
    let asked = Rc::new(Asked::default());
    let seen = asked.clone();
    let (view, cx) = host(cx, move |_, cx| {
        let this = cx.entity().downgrade();
        let (yes, no) = (seen.clone(), seen.clone());
        let (yes_view, no_view) = (this.clone(), this);
        Confirm::new("remove-task", "Remove the task test?")
            .sentence("Agents can no longer run it.")
            .action("Remove task")
            .refused(seen.refused.borrow().map(Into::into))
            .open(seen.open.get())
            .on_confirm(move |_, cx| {
                yes.confirmed.set(yes.confirmed.get() + 1);
                yes.open.set(false);
                yes_view.update(cx, |_, cx| cx.notify()).ok();
            })
            .on_cancel(move |_, cx| {
                no.cancelled.set(no.cancelled.get() + 1);
                no.open.set(false);
                no_view.update(cx, |_, cx| cx.notify()).ok();
            })
            .into_any_element()
    });
    (asked, view, cx)
}

fn ask(asked: &Asked, view: &Entity<Host>, cx: &mut VisualTestContext) {
    asked.open.set(true);
    redraw(view, cx);
    advance(cx, MODAL * 2);
}

#[gpui_kit::test]
fn a_confirmation_is_a_dialog_with_its_question_cancel_and_the_danger_button(
    cx: &mut TestAppContext,
) {
    let (asked, view, cx) = confirm(cx);
    assert!(!shown("remove-task", cx), "nothing while it is closed");
    ask(&asked, &view, cx);
    cx.update(|window, _| {
        assert!(window.try_find_by_label("Remove the task test?").is_some());
        assert!(
            window
                .try_find_by_label("Agents can no longer run it.")
                .is_some()
        );
    });
    assert_eq!(bounds("remove-task", cx).size.width, space::DIALOG);
    let (cancel, action) = (
        bounds("remove-task-cancel", cx),
        bounds("remove-task-action", cx),
    );
    assert!(cancel.origin.x < action.origin.x, "Cancel, then the action");
    assert_eq!(action.size.height, px(30.));
    cx.update(|window, cx| window.click("remove-task-action", cx));
    assert_eq!(asked.confirmed.get(), 1);
}

#[gpui_kit::test]
fn a_confirmation_is_cancelled_by_escape(cx: &mut TestAppContext) {
    let (asked, view, cx) = confirm(cx);
    ask(&asked, &view, cx);
    cx.update(|window, cx| window.press("escape", cx));
    assert_eq!(asked.cancelled.get(), 1);
    assert_eq!(asked.confirmed.get(), 0);
    advance(cx, MODAL * 2);
    assert!(!shown("remove-task", cx), "gone once it folded away");
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_confirmation_is_cancelled_by_its_scrim_and_by_cancel(cx: &mut TestAppContext) {
    let (asked, view, cx) = confirm(cx);
    ask(&asked, &view, cx);
    cx.update(|window, cx| window.click_at("remove-task-scrim", point(px(5.), px(5.)), cx));
    assert_eq!(asked.cancelled.get(), 1);
    ask(&asked, &view, cx);
    cx.update(|window, cx| window.click("remove-task-cancel", cx));
    assert_eq!(asked.cancelled.get(), 2);
    assert_eq!(asked.confirmed.get(), 0);
}

#[gpui_kit::test]
fn a_confirmation_that_cannot_be_done_says_why_and_its_button_does_nothing(
    cx: &mut TestAppContext,
) {
    let (asked, view, cx) = confirm(cx);
    *asked.refused.borrow_mut() = Some("4 files are not committed in atlas.");
    ask(&asked, &view, cx);
    cx.update(|window, _| {
        assert!(
            window
                .try_find_by_label("4 files are not committed in atlas.")
                .is_some()
        );
    });
    cx.update(|window, cx| window.click("remove-task-action", cx));
    assert_eq!(asked.confirmed.get(), 0);
}

// ── 3. An action's result is a toast ───────────────────────────────────────────────────────────

fn toasts(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, cx| {
        let stack = cx.new(ToastStack::new);
        Host {
            draw: Box::new(move |_, _| stack.clone().into_any_element()),
        }
    });
    cx.simulate_resize(size(px(1000.), px(800.)));
    cx.update(|window, cx| window.render_frame(cx));
    (view, cx)
}

#[gpui_kit::test]
fn a_toast_rises_at_the_bottom_right_and_a_success_leaves_after_its_time(cx: &mut TestAppContext) {
    let (_, cx) = toasts(cx);
    cx.update(|_, cx| {
        toast(
            Toast::success("Task test saved").detail("Agents run it from now on."),
            cx,
        )
    });
    assert!(shown("toast-1", cx));
    let first = bounds("toast-1", cx);
    advance(cx, ENTER * 2);
    let rested = bounds("toast-1", cx);
    assert!(first.origin.y > rested.origin.y, "it rises into place");
    assert_eq!(rested.size.width, px(360.));
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!((viewport.width - rested.right() - px(20.)).abs() < px(1.));
    assert!((viewport.height - rested.bottom() - px(20.)).abs() < px(1.));
    assert_eq!(pending_frames(cx), 0, "nothing is drawn while it waits");

    advance(cx, Duration::from_secs(4));
    advance(cx, PANEL * 2);
    assert!(!shown("toast-1", cx), "it left after four seconds");
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_toast_stays_while_it_is_hovered(cx: &mut TestAppContext) {
    let (_, cx) = toasts(cx);
    cx.update(|_, cx| toast(Toast::success("Task test saved"), cx));
    advance(cx, ENTER * 2);
    cx.update(|window, cx| window.hover("toast-1", cx));
    advance(cx, Duration::from_secs(6));
    assert!(shown("toast-1", cx), "it waits while the pointer is on it");
    cx.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
    advance(cx, Duration::from_secs(4));
    advance(cx, PANEL * 2);
    assert!(
        !shown("toast-1", cx),
        "and leaves once the pointer has left"
    );
}

#[gpui_kit::test]
fn a_failure_stays_until_it_is_dismissed_and_several_stack(cx: &mut TestAppContext) {
    let (_, cx) = toasts(cx);
    cx.update(|_, cx| {
        toast(
            Toast::failure("The workspace was not removed")
                .detail("4 files are not committed in atlas."),
            cx,
        );
        toast(Toast::success("Command open-prs removed"), cx);
    });
    advance(cx, ENTER * 2);
    let (failure, success) = (bounds("toast-1", cx), bounds("toast-2", cx));
    assert!(
        success.bottom() <= failure.top() || failure.bottom() <= success.top(),
        "stacked, not on top of each other"
    );
    advance(cx, Duration::from_secs(10));
    assert!(shown("toast-1", cx), "a failure stays");
    assert!(!shown("toast-2", cx), "a success left");
    cx.update(|window, cx| window.click("toast-1-dismiss", cx));
    advance(cx, PANEL * 2);
    assert!(!shown("toast-1", cx), "dismissed");
    assert_eq!(pending_frames(cx), 0);
}

// ── 4. Every control has a visible label ───────────────────────────────────────────────────────

#[gpui_kit::test]
fn a_field_draws_its_label_above_its_control_and_its_hint_and_error_under_it(
    cx: &mut TestAppContext,
) {
    let error: Rc<RefCell<Option<&'static str>>> = Rc::default();
    let said = error.clone();
    let (view, cx) = host(cx, move |_, _| {
        Field::new(
            "name",
            "Name",
            div().id("name-input").h(px(34.)).w(px(200.)).test_support(),
        )
        .hint("Agents run it as commands_run test.")
        .error(said.borrow().map(Into::into))
        .into_any_element()
    });
    cx.update(|window, _| assert!(window.try_find_by_label("Name").is_some()));
    let (label, control, hint) = (
        bounds("name-label", cx),
        bounds("name-input", cx),
        bounds("name-hint", cx),
    );
    assert!(label.bottom() <= control.top(), "the label is above");
    assert!(control.bottom() <= hint.top(), "the hint is under");
    assert!(!shown("name-error", cx));

    *error.borrow_mut() = Some("A task test exists already.");
    redraw(&view, cx);
    advance(cx, PANEL * 2);
    assert!(shown("name-error", cx), "the error comes under the field");
    assert!(bounds("name-error", cx).top() >= control.bottom());
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_switch_draws_its_label_beside_it_and_its_hint_under_the_label(cx: &mut TestAppContext) {
    let (_, cx) = host(cx, |_, _| {
        Switch::new("keeps", false, "Keeps running")
            .hint("A server: listed under Runs until stopped.")
            .into_any_element()
    });
    cx.update(|window, _| {
        assert_eq!(window.find("keeps-label").label(), Some("Keeps running"));
        assert_eq!(
            window.find("keeps-hint").label(),
            Some("A server: listed under Runs until stopped.")
        );
    });
    let (track, label, hint) = (
        bounds("keeps-track", cx),
        bounds("keeps-label", cx),
        bounds("keeps-hint", cx),
    );
    assert!(
        track.right() < label.left(),
        "the label is beside the track"
    );
    assert!(label.bottom() <= hint.top(), "the hint under the label");
}

#[gpui_kit::test]
fn a_click_on_a_switch_s_label_toggles_it(cx: &mut TestAppContext) {
    let on = Rc::new(Cell::new(false));
    let seen = on.clone();
    let (view, cx) = host(cx, move |_, cx| {
        let (seen, this) = (seen.clone(), cx.entity().downgrade());
        Switch::new("keeps", seen.get(), "Keeps running")
            .on_click(move |_, _, cx| {
                seen.set(!seen.get());
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .into_any_element()
    });
    cx.update(|window, cx| window.click("keeps-label", cx));
    assert!(on.get());
    redraw(&view, cx);
}

#[gpui_kit::test]
#[should_panic(expected = "at most three")]
fn a_segmented_control_refuses_a_fourth_choice(cx: &mut TestAppContext) {
    let (_, _cx) = host(cx, |_, _| {
        Segmented::new(
            "view",
            "View",
            [("a", "One"), ("b", "Two"), ("c", "Three"), ("d", "Four")],
            0,
        )
        .into_any_element()
    });
}

#[gpui_kit::test]
fn a_segmented_control_is_sized_to_its_words(cx: &mut TestAppContext) {
    let (_, cx) = host(cx, |_, _| {
        div()
            .flex()
            .flex_col()
            .child(Segmented::new(
                "short",
                "Mode",
                [("ask", "Ask"), ("auto", "Auto")],
                0,
            ))
            .child(Segmented::new(
                "long",
                "View",
                [("all", "Everything at once"), ("mine", "Only mine")],
                0,
            ))
            .into_any_element()
    });
    let (ask, everything) = (bounds("short-ask", cx), bounds("long-all", cx));
    assert!(
        everything.size.width > ask.size.width,
        "a longer word, a wider segment: {:?} {:?}",
        ask.size.width,
        everything.size.width
    );
    assert!(bounds("short", cx).size.width < px(120.), "never stretched");
}

#[gpui_kit::test]
fn choosing_a_radio_choice_reports_it_and_its_dot_scales_in(cx: &mut TestAppContext) {
    let chosen = Rc::new(Cell::new(0usize));
    let seen = chosen.clone();
    let (view, cx) = host(cx, move |_, cx| {
        let (seen, this) = (seen.clone(), cx.entity().downgrade());
        RadioChoices::new("source", "What it is")
            .choice(
                "file",
                "A file of the project",
                "Read where the agent works.",
            )
            .choice(
                "text",
                "A text Flottille keeps",
                "In Flottille's data folder.",
            )
            .chosen(seen.get())
            .on_choose(move |index, _, cx| {
                seen.set(index);
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .into_any_element()
    });
    cx.update(|window, _| {
        assert!(
            window
                .try_find_by_label("In Flottille's data folder.")
                .is_some()
        );
        assert_eq!(window.find("source-file").selected(), Some(true));
    });
    let full = bounds("source-file-dot", cx).size.width;
    cx.update(|window, cx| window.click("source-text", cx));
    assert_eq!(chosen.get(), 1);
    redraw(&view, cx);
    advance(cx, PANEL / 3);
    let growing = bounds("source-text-dot", cx).size.width;
    assert!(
        px(0.) < growing && growing < full,
        "the dot scales in: {growing:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!(bounds("source-text-dot", cx).size.width, full);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_select_s_option_is_chosen_from_the_list_it_opens(cx: &mut TestAppContext) {
    let chosen = Rc::new(Cell::new(0usize));
    let seen = chosen.clone();
    let (view, cx) = host(cx, move |_, cx| {
        let (seen, this) = (seen.clone(), cx.entity().downgrade());
        Select::new(
            "tracker",
            [
                SelectOption::new("github", "GitHub issues")
                    .line("Issues of a repository, read with gh"),
                SelectOption::new("jira", "Jira").line("A Jira site, read only"),
                SelectOption::new("none", "None"),
            ],
            seen.get(),
        )
        .on_choose(move |index, _, cx| {
            seen.set(index);
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .into_any_element()
    });
    assert!(!shown("tracker-menu", cx));
    assert_eq!(bounds("tracker", cx).size.height, px(34.));
    cx.update(|window, cx| window.click("tracker", cx));
    advance(cx, HOVER * 2);
    assert!(shown("tracker-menu", cx), "the list fades in");
    cx.update(|window, _| {
        assert!(window.try_find_by_label("A Jira site, read only").is_some());
        assert_eq!(window.find("tracker-github").selected(), Some(true));
    });
    cx.update(|window, cx| window.click("tracker-jira", cx));
    assert_eq!(chosen.get(), 1);
    redraw(&view, cx);
    advance(cx, ENTER * 2);
    assert!(!shown("tracker-menu", cx), "the list closed");
    cx.update(|window, _| assert!(window.try_find_by_label("Jira").is_some()));
    assert_eq!(pending_frames(cx), 0);
}

// ── 6. A run opens its detail in a drawer ──────────────────────────────────────────────────────

struct Nothing;

impl Render for Nothing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

#[gpui_kit::test]
fn the_detail_drawer_slides_from_the_right_without_a_scrim_and_escape_closes_it(
    cx: &mut TestAppContext,
) {
    init(cx, Desktop::new(false).env());
    let slot: Rc<RefCell<Option<Entity<Drawer>>>> = Rc::default();
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        let content = cx.new(|_| Nothing);
        let drawer = cx.new(|cx| Drawer::detail("run", content, cx));
        *kept.borrow_mut() = Some(drawer.clone());
        Host {
            draw: Box::new(move |_, _| drawer.clone().into_any_element()),
        }
    });
    let drawer = slot.borrow().clone().unwrap();
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.update(|window, cx| window.render_frame(cx));
    assert!(!shown("run-panel", cx), "nothing while it is closed");
    cx.update(|window, cx| {
        drawer.update(cx, |d, cx| d.show(window, cx));
        window.render_frame(cx);
    });
    let first = bounds("run-panel", cx);
    advance(cx, PANEL * 2);
    let rested = bounds("run-panel", cx);
    assert!(first.left() > rested.left(), "it slides in from the right");
    assert_eq!(rested.right(), px(1200.));
    assert_eq!(rested.size.width, px(520.));
    assert!(!shown("run-scrim", cx), "no scrim over the page");
    assert_eq!(pending_frames(cx), 0);

    cx.update(|window, cx| window.press("escape", cx));
    assert!(!drawer.read_with(cx, |d, _| d.is_open()));
    advance(cx, PANEL * 2);
    assert!(!shown("run-panel", cx));
    assert_eq!(pending_frames(cx), 0);
}

// ── 7. Lists, one pattern ──────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn a_card_draws_a_separator_only_between_blocks_that_exist(cx: &mut TestAppContext) {
    let (_, cx) = host(cx, |_, _| {
        div()
            .flex()
            .flex_col()
            .child(
                Card::new()
                    .id("card")
                    .title("help-pantry-file")
                    .block(Block::new().child(div().h(px(20.))))
                    .block(Block::new())
                    .block(Block::new().child(div().h(px(20.))))
                    .block(Block::new()),
            )
            .into_any_element()
    });
    let separators = cx.update(|window, _| {
        (0..6)
            .filter(|n| window.try_find(format!("card-separator-{n}")).is_some())
            .count()
    });
    assert_eq!(
        separators, 2,
        "under the header, and between the two blocks that have something"
    );
}

#[gpui_kit::test]
fn a_list_row_shows_its_more_button_on_hover(cx: &mut TestAppContext) {
    let kept: Rc<RefCell<Option<Entity<Menu>>>> = Rc::default();
    let (_, cx) = host(cx, move |_, cx| {
        let menu = kept
            .borrow_mut()
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    Menu::new("style-more", "More", cx)
                        .entry("style-edit", "Edit", None, |_, _| {})
                        .danger("style-remove", "Remove…", |_, _| {})
                })
            })
            .clone();
        ListCard::new("skills")
            .row(
                ListRow::new("style", "style")
                    .mono()
                    .tag("docs/STYLE.md")
                    .line("The code style: read before writing code")
                    .more(menu),
            )
            .into_any_element()
    });
    let hidden = cx.update(|window, _| window.find("style-actions").visible());
    assert!(!hidden, "the ⋯ is not drawn at rest");
    cx.update(|window, cx| window.hover("style", cx));
    advance(cx, HOVER / 3);
    let fading = cx.update(|window, _| window.find("style-actions").visible());
    assert!(fading, "it fades in");
    advance(cx, HOVER * 2);
    assert!(cx.update(|window, _| window.find("style-actions").visible()));
    cx.update(|window, _| assert!(window.find("style-more").visible()));
    assert!(bounds("style", cx).size.height >= px(52.));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_menu_s_danger_item_comes_last_and_is_picked(cx: &mut TestAppContext) {
    let removed = Rc::new(Cell::new(false));
    let seen = removed.clone();
    let kept: Rc<RefCell<Option<Entity<Menu>>>> = Rc::default();
    let (_, cx) = host(cx, move |_, cx| {
        let seen = seen.clone();
        let menu = kept
            .borrow_mut()
            .get_or_insert_with(|| {
                cx.new(move |cx| {
                    Menu::new("more", "More", cx)
                        .danger("remove", "Remove…", move |_, _| seen.set(true))
                        .entry("edit", "Edit", None, |_, _| {})
                })
            })
            .clone();
        div().child(menu).into_any_element()
    });
    cx.update(|window, cx| window.click("more", cx));
    advance(cx, ENTER * 2);
    assert!(bounds("edit", cx).top() < bounds("remove", cx).top());
    cx.update(|window, cx| window.click("remove", cx));
    assert!(removed.get());
}

#[gpui_kit::test]
fn an_empty_list_is_one_line_saying_what_to_add(cx: &mut TestAppContext) {
    let (_, cx) = host(cx, |_, _| {
        EmptyLine::new(
            "commands-empty",
            Icon::Slash,
            "No command yet. Start from a template or write your own.",
        )
        .action(Button::secondary("new-command", "New command").leading(Icon::Plus))
        .into_any_element()
    });
    cx.update(|window, _| {
        assert!(
            window
                .try_find_by_label("No command yet. Start from a template or write your own.")
                .is_some()
        );
    });
    let (line, action) = (bounds("commands-empty", cx), bounds("new-command", cx));
    assert!(line.contains(&action.center()));
}

// ── 9. Spacing ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_spacing_scale_is_4_8_12_16_24_32_48() {
    assert_eq!(
        [
            space::XS,
            space::SM,
            space::MD,
            space::LG,
            space::XL,
            space::XXL,
            space::XXXL
        ],
        [4., 8., 12., 16., 24., 32., 48.].map(px)
    );
    assert_eq!(
        (space::COLUMN, space::DIALOG, space::DIALOG_WIDE),
        (px(880.), px(520.), px(640.))
    );
}
