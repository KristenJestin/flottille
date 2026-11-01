# Reference: GPUI Fast and gpui-kit, what was learned

Lessons from two earlier Rust desktop apps on the same stack (a music player and a video client),
measured on a 16-core Linux machine with a 165 Hz screen. Keep them; they cost a lot to learn.

## The stack

- **gpui-kit** (crates.io, 0.7.x, by Longbridge): components (lists, tables, inputs, icons
  Lucide, dialogs) on top of GPUI, Zed's GPU UI engine. It pins a GPUI snapshot (`gpui-pre`).
- **GPUI Fast** (`gpui-fast`, 0.1.x, by Longbridge): a fork of GPUI in retained mode, which
  redraws only the views that changed. Enable it with gpui-kit's `gpui-fast` feature (no
  `[patch]` needed): `fast = ["gpui-kit/gpui-fast"]` in the default features;
  `--no-default-features` builds plain GPUI. `GPUI_VIEW_RETENTION=0` turns retention off to
  compare or to rule out a stale display.
- Results: at rest about 4 draws per second and under 1% of a core; lists of 7,200 rows and grids
  of 720 covers scroll with no frame over the 6 ms budget; animations at full refresh rate.
- Build cost: a release build from scratch takes 8 to 14 minutes at `nice -n 19` with 6 jobs and
  up to 1 GB of memory for GPUI alone; incremental builds take about 5 s. Binary about 55 MB
  (embed only the icons used: `gpui_kit::assets::icon_assets!(Icons, [Play, …])`).
- CI (Ubuntu): `pkg-config libfontconfig-dev libfreetype-dev libxkbcommon-dev
  libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`; only fontconfig
  and freetype are linked at build time. Cache the build (`Swatinem/rust-cache`).

## Performance rules (measured)

- **One view per moving part.** Anything that changes often (an animation, a ticking clock, a
  blinking caret, a streaming message) lives in its own small view. Measured: a page with an
  animated card and ~120 cards in one view drew in 6.4 to 7.3 ms per frame with ~50 dropped
  frames; with the card and each row in their own views, 0.8 to 1.8 ms. A factor of 5 to 6.
- **Fast-changing state in its own entity**, read by one small view; in a shared model, every
  tick rebuilds everything that reads it.
- **A view created by `window.use_keyed_state` is observed by the view that rendered it**: its
  `cx.notify()` dirties its ancestors. For a ticking mark inside a large list, the host still
  redraws on each tick; keep tick rates low.
- **The cost of an animation is the number of frames it requests**, not what it draws. Prefer a
  timer at a few Hz in a separate view to a continuous `with_animation` loop (340 ms of drawing
  per second against 22).
- Never notify or write a global during prepaint or paint, nor on every frame.
- An animation requested in the same view as a list makes the list lose its scroll layer: split
  them.
- A **text input** (`gpui_kit::component::input`) notifies its state on every caret blink: render
  it in its own view, keep its text in a separate entity updated on `Change`.
- `uniform_list` calls its row closure for row 0 alone to measure, and only for missing rows
  later: side effects in the closure must ignore `0..1` and not count on being called again.
- A list that grows while it is looked at (a streaming conversation): keep the rows in an
  `Arc<[T]>` replaced on change, rows as their own views, and append without rebuilding the
  others.

## Motion in the window: the rule and its checklist

The rule (`AGENTS.md`, `docs/design.md` §10): **everything that moves is animated**, with the
durations and the easing of `crates/flottille-ui/src/motion.rs` (`HOVER` 140 ms, `PRESS` 90 ms,
`ENTER` 240 ms, `PANEL` 280 ms, a stagger of 70 ms for at most eight siblings, a rise of 6 px).
`flottille-ui` has one helper for each thing that moves; a screen uses them and invents none.

| What moves | The helper |
|---|---|
| a button, a list row, a tree row, a menu item | the component itself (`Button`, `ListRow`, `TreeRow`, `MenuItem`): hover, press, selection fade |
| any other element that is clicked (a link, a card, a chip, a toggle) | `hoverable(element, paint)`: `paint(element, t)` gets the hover `t`, 0 to 1; it sets a background with `p.hover.opacity(t)`, an ink with `mix(from, to, t)` |
| the same element when it can also be the chosen one (a selected row, a taken chip) | `selectable(element, chosen, paint)`: `paint` also gets how far it is the chosen one; a choice among buttons is `Button::choice` |
| a state with no pointer: a focus ring, a highlight, a button enabled or not | `fade(id, on, duration, window, cx)` (`Button` fades its own enabled state) |
| a choice among a few words (Ask \| Auto) | `Segmented`: its highlight slides to the one clicked, the inks fade along it |
| words under the pointer on an element that is no button (a row's mark) | `tipped(element, text, height, window, cx)`: the button's anchored tooltip, `<id>-tooltip` |
| a chevron | `turning(id, open, window, cx)` |
| a page's blocks, a card, anything that shows once | `enter(id, index, element)` |
| a block that comes and goes while its view stays (an empty-list line, the fields a choice needs, a body that unfolds) | `presence(id, present, element).gap(..)`: build it on every frame, outside a `when`; `.gap` is the gap the parent flex puts between its children, which folds with the block |
| a line that goes away with its text (an error, a refusal) | `presence_of(id, Option<line>, build)`: it still shows the last line while it folds |
| the rows of a list that comes from elsewhere | `Listing::sync` on each render, each row drawn inside an `Item`: it unfolds when it arrives, folds away when it leaves (it stays drawn until then), slides when it moves |
| a level that folds | `Fold` (a view) or `fold_frame` |
| a panel over the page (a menu, a drawer, a modal, the bell's list) | rises a little and fades in, fades out, and stays drawn until it has |

Never in a screen: `.hover(..)` or `.group_hover(..)` (they switch at once; `tests/the_rule.rs`
of the window crate refuses them), a state that changes by swapping one element for another with
nothing between (wrap what appears in `enter`, `presence` or an `Item`), a `Motion` kept by hand
when a helper above does it.

**The guard.** gpui-kit's test support (`test_support()`, `window.find(..)`) says an element's
role, label, bounds and focus, not which listeners it has: no test can walk a view and ask each
interactive element for its hover transition. What a test can hold is held: the instant hover
style is refused in every source of the window (`crates/flottille-window/tests/the_rule.rs`), each
helper has its own scenarios (`crates/flottille-ui/tests/everything_moves.rs`: a hover fades and
stops, a row arrives, goes and slides, a block folds, reduced motion jumps, nothing is drawn once
it arrived), and the rest is this checklist, which each window pull request ticks in its
description:

- [ ] every element that is clicked fades its hover (a component, `hoverable` or `selectable`);
- [ ] every state that changes (selected, chosen, focused, open, enabled) moves, not jumps;
- [ ] everything that appears, leaves or changes size is an `enter`, a `presence`, an `Item` or a
      `Fold`, and the gap around it goes with it;
- [ ] every list that gains, loses or reorders rows is a `Listing` of `Item`s;
- [ ] every new page, section or panel enters (staggered), and every overlay rises and fades;
- [ ] reduced motion jumps (the helpers do it; a `Motion` kept by hand must pass `reduce`);
- [ ] nothing is redrawn at rest: a scenario ends on `pending_frames == 0`;
- [ ] the pull request lists each element it animated, and a capture or a scenario shows one.

## Rules of the window

The window holds together because every screen follows the same rules and builds them from the
same components of `flottille-ui`, which make the wrong thing hard to write. **[test]** marks a
rule a test holds: the components' scenarios are in `crates/flottille-ui/tests/rules.rs`, the scans
of the window's sources in `crates/flottille-window/tests/window_rules.rs` (each shown a sample of
the wrong code first).

1. **Buttons: four kinds, one size.** Primary (filled accent, at most one per view: Save, Create,
   Send, Allow), Secondary (transparent, bordered: a page's or a section's actions), Quiet (no
   border: Cancel, Deny, lesser actions), Danger (filled fail colour: only the action of a
   confirmation of something destructive). Every text button is 30 px high, 13 px medium; an icon
   button is a 28 px square with a tooltip, anchored to it (below, centred, above when there is no
   room below, inside the window; never GPUI's `.tooltip(..)`, which opens under the pointer).
   **[test]** `Button` takes no size, and `crates/flottille-ui/tests/tooltips.rs` holds the
   tooltip. A page header holds at most two secondary buttons, on the title's line, and a "⋯" menu for the rest
   (**[test]** `PageHeader` refuses a third). A destructive action is never a visible button in a
   page or a row: it is a "Remove…" item of a "⋯" menu, last. A label never repeats a name the
   title already says.
2. **One confirmation: a modal dialog.** Its title is the question with the object's name, then
   one sentence of consequence, Cancel and the danger button; Escape and the scrim cancel; when
   the action cannot be done, the dialog says why and its button is disabled. Never a button that
   turns into other buttons in place. **[test]** `Confirm`, and a scan refusing the
   inline-confirmation ids.
3. **An action's result is a toast**: bottom right, 360 px, an icon, one line and an optional
   second; it rises and fades in, folds away with its gap; a success leaves after 4 s (held while
   hovered), a failure stays until dismissed; several stack, the newest on top. Never a line
   appended under a form. **[test]** `Toast`, and a scan refusing such lines outside it. A form's
   own validation stays under its field.
4. **Every control has a visible label.** A text field has its label above it (a placeholder is
   only an example); a switch draws its label beside it. **[test]** `Field` takes a label;
   `Switch` draws its label and its hint. Which control for which choice: a switch for one thing
   on or off; a segmented control only for two or three short words that switch a mode or a view
   at once (Ask | Auto), sized to its words, never in a form; radio choices for two to four kinds
   in a form that each need a line; a select for a list that may grow, more than three options or
   long labels. **[test]** `Segmented` takes at most three choices and a scan refuses it in a
   dialog.
5. **A state is said once per view**: an agent's state by its icon on its own row, "needs you" in
   one place. **[test]** on the session page, "needs you" is said at most once.
6. **Where things live**: the project page shows and runs, the project settings configure; a
   setting editable in two places is a defect; a run opens its detail in a drawer.
7. **Lists, one pattern**: a configured item is a row (icon, name, tags, one line) in a
   `ListCard`; a click opens its edit dialog; its "⋯", shown on hover, holds Edit, the moves and
   "Remove…"; an empty list is one dashed line saying what to add and why; reordering is by drag,
   with Move up and Move down in the "⋯". A separator is drawn only between two blocks that exist.
   **[test]** `Card` draws a block's separator only when the block has content.
8. **Words**: paths relative to the project folder; a remote in its short form, labelled; labelled
   values when there are more than two, never a run of mono values separated by dots; a section
   has a title, a count when it is a list, and one sentence saying what it is for.
9. **Spacing**: the scale 4, 8, 12, 16, 24, 32, 48 px (`flottille_ui::space`). **[test]** the
   constants, and a scan refusing any other gap or padding literal in the window crate. A page's
   column is 880 px at most with 32 px between its sections; a dialog is 520 px (640 wide) with
   24 px of padding.
10. **Motion**, as above: everything that appears, leaves, opens, folds or changes state moves
    with the shared durations; a drawer slides from the right; a select's list fades in; a
    radio's dot scales in. **[test]** each component's scenario.

The components that hold them:

| Component | What it is for |
|---|---|
| `Button::{primary, secondary, quiet, danger, icon}` | rule 1; `.leading(Icon)`, `.disabled(bool)`, `.badge(Badge)` |
| `PageHeader::new(id, title)` | `.path(p)`, `.crumb(el)`, `.below(el)`, `.action(id, label, Option<Icon>, f)` twice at most, `.more(Entity<Menu>)` |
| `Section::new(id, title)` | `.count(c)`, `.sentence(s)`, `.action(Button)`, then its children |
| `Menu::new(..).entry(..).danger(id, label, f)` | a "⋯" menu; danger entries come last, after a line |
| `Confirm::new(id, question)` | `.sentence(s)`, `.action(label)`, `.refused(Option<why>)`, `.open(bool)`, `.on_confirm(f)`, `.on_cancel(f)`; ids `<id>-action`, `<id>-cancel`, `<id>-scrim` |
| `Modal::new(id, title)` | a form dialog: `.sentence(s)`, `.wide()`, `.action(Button)`, `.on_close(f)`; its body scrolls between the title and the buttons when the window is too small, and `.kept(el)` stays in sight under it (a refusal) |
| `toast(Toast::success(t).detail(d), cx)` / `Toast::failure` | rule 3; the shell owns the `ToastStack` |
| `Field::new(id, label, control)` | `.optional()`, `.hint(h)`, `.hint_mono()`, `.error(Option<e>)` |
| `Switch::new(id, on, label)` | `.hint(h)`, `.on_click(f)`; the whole row toggles |
| `Segmented::new(id, label, choices, chosen)` | at most three choices, never in a dialog |
| `RadioChoices::new(id, label)` | `.choice(key, title, line)`, `.leading(Icon)`, `.chosen(i)`, `.on_choose(f)`; two to four |
| `Select::new(id, options, chosen)` | `SelectOption::new(key, title).line(l)`, `.fit()`, `.disabled(b)`, `.on_choose(f)` |
| `Drawer::detail(id, view, cx)` | a run's detail: 520 px from the right, no scrim, Escape closes; `.show(window, cx)` |
| `Card::new().id(id).block(Block)` | a card whose separators are drawn only between blocks with content |
| `ListCard::new(id).row(ListRow)` | rule 7's list; `ListRow` has `.mono()`, `.tag(t)`, `.line(l)`, `.more(Entity<Menu>)` (its clicks stay its own), `.below(el)` (labelled values, a reason under the title); `.item(id, placement, row)` draws a row of a `Listing` in its `Item` |
| `Tabs::new(id, label, chosen).tab(key, words)`, `.counted(key, words, count)`, `.on_choose(f)` | a page of sections shown one at a time (the project's settings): as many tabs as sections, a count after the name of a list, the marker slides to the tab chosen over `PANEL`; the page draws the chosen section alone, inside an `enter` that plays again as it is chosen again |
| `PageIndex::new(id, entries).current(i).on_pick(f)` | a settings page's index of sections, the one in sight lit; the window's `sections::Sections` keeps it at the top once the page scrolls past it |
| `EmptyLine::new(id, Icon, text)` | an empty list; `.action(Button)` |
| `space::{XS, SM, MD, LG, XL, XXL, XXXL}`, `COLUMN`, `DIALOG`, `DIALOG_WIDE` | rule 9 |

`cargo run -p flottille-ui --example components` shows them on one page (`--dialog`, `--confirm`,
`--drawer`, `--menu` open one of them), for a capture in a headless compositor.

## Pitfalls

- `use gpui_kit::prelude::*` brings `ParentElement`, `StatefulInteractiveElement`, `AppContext`,
  `FluentBuilder`; `h_flex`/`v_flex` are in `gpui_kit::base`. `.on_click` needs an `.id(..)`.
  Do not name your root view `App`.
- Icons need `.with_assets(...)`, or icon buttons render as empty squares.
- In a scrolling `v_flex`, children with a fixed height shrink: add `.flex_none()`. Rows of a
  `uniform_list` do not stretch: `.w_full()`. Rounded images: put `.rounded(..)` on the `img`
  itself.
- An `absolute` layer with buttons lets clicks through to what is below: `.occlude()`.
- GPUI Fast draws every `deferred` layer in one list sorted by priority, a layer opened from inside
  another included: a list nested in a dialog with a lower priority is drawn under the dialog. The
  window's priorities live in `flottille_ui::layer` (dialog, list, toast); a new floating layer
  takes one there, never a number of its own.
- A text view sets the I-beam over its own lines only, so a pointer crossing the gap between two
  paragraphs (or two views) flickers between I-beam and arrow. A selectable text sits in a
  `text_region(id)`, one region that keeps the I-beam from its first line to its last.
- Keys: GPUI tries key bindings first, then `on_key_down` from the root down, then gives the
  character to the input. A global shortcut on Space or `/` on the root swallows typing: check
  first whether the input has focus.
- Focus given while building a view is taken back by a root built after it: defer with
  `cx.defer_in(window, …)`.
- A transition is exactly 0 on the first frame of an opening: test the target state too.
- `rgb()` is not `const`: a theme is a set of functions.
- A window without focus is capped at about 30 frames per second
  (`WindowOptions::inactive_frame_interval`).
- `App::on_window_closed` differs between GPUI and GPUI Fast: close the app from
  `Window::on_window_should_close`.

## Testing without a screen (mandatory: never open a window on the user's screens)

- Logic and views: GPUI's own test platform (`TestAppContext`, `add_window_view`,
  `simulate_click`, `executor().advance_clock(..)`, `run_until_parked()`), no window at all. This
  is what CI runs.
- A real window, for screenshots or measurements: a nested headless compositor, never the user's
  session: `weston --backend=headless --renderer=gl --xwayland --shell=kiosk --fake-seat` in its
  own `XDG_RUNTIME_DIR`, the app started with only `DISPLAY=:N` of that Weston (no
  `WAYLAND_DISPLAY`, no session bus: use a private `dbus-daemon` with a config file that has no
  service directories, or the desktop portal attaches to the real session). Plain
  `weston --backend=headless` without Xwayland has no `wl_seat` and GPUI panics. Input by XTEST,
  captures with `weston-screenshooter`. Never kill test processes by pattern (`pkill -f`): keep
  their PIDs.
- Measure on a quiet machine and note the load; a run under load varies threefold.
