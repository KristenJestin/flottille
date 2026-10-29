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
| a state with no pointer: a focus ring, a highlight | `fade(id, on, duration, window, cx)` |
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

## Pitfalls

- `use gpui_kit::prelude::*` brings `ParentElement`, `StatefulInteractiveElement`, `AppContext`,
  `FluentBuilder`; `h_flex`/`v_flex` are in `gpui_kit::base`. `.on_click` needs an `.id(..)`.
  Do not name your root view `App`.
- Icons need `.with_assets(...)`, or icon buttons render as empty squares.
- In a scrolling `v_flex`, children with a fixed height shrink: add `.flex_none()`. Rows of a
  `uniform_list` do not stretch: `.w_full()`. Rounded images: put `.rounded(..)` on the `img`
  itself.
- An `absolute` layer with buttons lets clicks through to what is below: `.occlude()`.
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
