//! The "⋯" menus of the ticket rows' "Write the spec as…" (#687): a menu acts on the ticket its
//! row shows when it is clicked, and the rows no longer drawn keep nothing. On GPUI's test
//! platform with no daemon; no window opens on a screen.

mod kit_common;

use kit_common::{Desktop, init};

use flottille_window::extensions::specs::WriteSpec;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WeakEntity};

fn write_spec(cx: &mut TestAppContext) -> Entity<WriteSpec> {
    init(cx, Desktop::new(false).env());
    cx.new(|_| WriteSpec::new(WeakEntity::new_invalid(), "app".into()))
}

/// Draws the rows `rows`, each `(row, ticket, title)`, as the ticket list does.
fn draw(write: &Entity<WriteSpec>, rows: &[(&str, &str, &str)], cx: &mut TestAppContext) {
    write.update(cx, |write, cx| {
        write.begin_draw();
        for (row, ticket, title) in rows {
            write.menu(row, ticket, title, cx);
        }
        write.end_draw();
    });
}

#[gpui_kit::test]
fn a_row_that_shows_another_ticket_never_acts_on_the_first(cx: &mut TestAppContext) {
    let write = write_spec(cx);
    draw(&write, &[("item-1", "#5", "Users cannot sign in")], cx);
    // The same row, now another ticket: its menu was made for the first.
    draw(&write, &[("item-1", "#6", "Typo on the page")], cx);

    write.update(cx, |write, cx| write.act_as("item-1", cx));
    write.read_with(cx, |write, _| {
        assert_eq!(write.choosing(), Some(("#6", "Typo on the page")));
        assert_eq!(write.kept_rows(), 1, "one menu, kept for the row");
    });
}

#[gpui_kit::test]
fn the_rows_no_longer_drawn_keep_no_menu(cx: &mut TestAppContext) {
    let write = write_spec(cx);
    draw(
        &write,
        &[("item-1", "#5", "Five"), ("item-2", "#6", "Six")],
        cx,
    );
    write.read_with(cx, |write, _| assert_eq!(write.kept_rows(), 2));

    draw(&write, &[("item-1", "#5", "Five")], cx);
    write.read_with(cx, |write, _| assert_eq!(write.kept_rows(), 1));
    // A click on the menu that went does nothing.
    write.update(cx, |write, cx| write.act_as("item-2", cx));
    write.read_with(cx, |write, _| assert_eq!(write.choosing(), None));
}
