# Reference: GPUI Fast and gpui-kit, what was learned

Lessons from two earlier Rust desktop apps on the same stack (a music player and a video client),
measured on a 16-core Linux machine with a 165 Hz screen. Keep them; they cost a lot to learn.

## The stack

- **gpui-kit** (crates.io, 0.7.x, by Longbridge): components (lists, tables, inputs, icons
  Lucide, dialogs) on top of GPUI, Zed's GPU UI engine. It pins a GPUI snapshot (`gpui-pre`).
- **GPUI Fast** (`gpui-fast`, 0.1.x, by Longbridge): a fork of GPUI in retained mode, which
  redraws only the views that changed. The kit (below) always builds gpui-kit's `gpui-fast`
  feature (no `[patch]` needed): the window has no plain-GPUI build. Keep `gpui-fast` at 0.1.10
  in `Cargo.lock` (0.1.11 breaks `gpui-base` 0.8). `GPUI_VIEW_RETENTION=0` turns retention off
  to compare or to rule out a stale display.
- **The kit** ([`gpui-design-system`](https://github.com/KristenJestin/gpui-design-system)): the
  components, tokens and motion of the window, shared with other GPUI applications. It is a git
  dependency of `flottille-window` (and of `flottille`'s window tests) pinned to one of its tags,
  never a path: Flottille holds no component of its own beside the window's few (`state.rs`,
  `steps.rs`, `tree.rs`). **Adding or changing a component** is a pull request in the kit, with
  its scenarios in the kit's `tests/` and its rules (its `AGENTS.md`); each push of its branch
  publishes an alpha tag (`vX.Y.Z-alpha.<branch>.N`), its merge into the kit's `main` a beta
  (`vX.Y.Z-beta.N`). Flottille then bumps the `tag` of `gpui-design-system` in both
  `Cargo.toml` files to that tag, in the pull request of the issue that needs it: one line each,
  and `Cargo.lock`. An alpha tag may be pinned while both pull requests are open, never on `dev`:
  the kit deletes a branch's alphas once it is merged, so `dev` pins a beta or a stable only. A screen that needs a component the kit lacks waits for it, never draws its
  own.
- Results: at rest about 4 draws per second and under 1% of a core; lists of 7,200 rows and grids
  of 720 covers scroll with no frame over the 6 ms budget; animations at full refresh rate.
- Build cost: a release build from scratch takes 8 to 14 minutes at `nice -n 19` with 6 jobs and
  up to 1 GB of memory for GPUI alone; incremental builds take about 5 s. Binary about 55 MB
  (embed only the icons used: `gpui_kit::assets::icon_assets!(Icons, [Play, …])`).
  The kit knows no app: its `Icons` serves the base set of `Icon`, and an app adds its
  own. Another Lucide icon is `Icon::Lucide("name")`, a glyph of the app's own `Icon::Asset(path)`
  (stroked) or `Icon::Filled(path)` (solid, drawn larger); the app's assets serve their files
  beside `Icons`. Flottille's are in `crates/flottille-window/src/glyphs.rs` (`Assets`; the stop
  glyph, a small filled square Lucide does not draw, is an SVG of
  `crates/flottille-window/assets/icons/`, embedded with `include_bytes!`).
  An app tells the kit its name, its themes and the desktops whose theme it follows at `init`
  (`Identity`); the window's is `flottille_window::identity(env)`: Flottille, its own theme
  (`flottille`) and Omarchy (the kit's default `omarchy` feature). The parts that know agents
  (the state marks, the setup step row, the tree's levels) are the window's own: `state.rs`,
  `steps.rs`, `tree.rs`. A scan in the kit (`tests/no_app_word.rs`) keeps it from naming any of
  them.
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
- **A ticker runs only while its view is drawn.** A page the window keeps but no longer shows
  keeps its views alive, and a view notified while off screen still draws the window. Measured
  (#444): fourteen skeletons' pulses left ticking drew a settled session page 140 times a
  second. A ticker stops at the first tick that finds its view was not drawn since the last one
  (`Skeleton`'s pulse does).
- An animation requested in the same view as a list makes the list lose its scroll layer: split
  them.
- A **text input** (`gpui_kit::component::input`) notifies its state on every caret blink: render
  it in its own view, keep its text in a separate entity updated on `Change`.
  A field's colours (ink, placeholder, selection, caret) are not drawn from the palette by the
  input: they are set on it. `gpui_design_system::follow_theme` sets them from `field_style` and
  again at each frame of a theme change; a field made with an `InputEditorStyle` of its own keeps
  the colours of the theme it was made in (a scan refuses it, in the window's sources and in the
  kit's).
- `uniform_list` calls its row closure for row 0 alone to measure, and only for missing rows
  later: side effects in the closure must ignore `0..1` and not count on being called again.
- A list that grows while it is looked at (a streaming conversation): keep the rows in an
  `Arc<[T]>` replaced on change, rows as their own views, and append without rebuilding the
  others.

## Motion in the window: the rule and its checklist

The rule (`AGENTS.md`, `docs/design.md` §10): **everything that moves is animated**, with the
durations and the easing of the kit's `src/motion.rs` (`HOVER` 140 ms, `PRESS` 90 ms,
`ENTER` 240 ms, `PANEL` 280 ms, a stagger of 70 ms for at most eight siblings, a rise of 6 px).
The kit has one helper for each thing that moves; a screen uses them and invents none.

| What moves | The helper |
|---|---|
| a button, a list row, a tree row, a menu item | the component itself (`Button`, `ListRow`, `TreeRow`, `MenuItem`): hover, press, selection fade |
| any other element that is clicked (a link, a card, a chip, a toggle) | `hoverable(element, paint)`: `paint(element, t)` gets the hover `t`, 0 to 1; it sets a background with `p.hover.opacity(t)`, an ink with `mix(from, to, t)`; `pressable(element, paint)` also gives how far it is pressed, over `PRESS` (a field that opens something) |
| the same element when it can also be the chosen one (a selected row, a taken chip) | `selectable(element, chosen, paint)`: `paint` also gets how far it is the chosen one; a choice among buttons is `Button::choice` |
| a state with no pointer: a focus ring, a highlight, a button enabled or not | `fade(id, on, duration, window, cx)` (`Button` fades its own enabled state) |
| a choice among a few words (Ask \| Auto) | `Segmented`: its highlight slides to the one clicked, the inks fade along it |
| words under the pointer on an element that is no button (a row's mark) | `tipped(element, text, height, window, cx)`: the button's anchored tooltip, `<id>-tooltip` |
| a one-line text cut with an ellipsis (a title, a row's line, a fact, a diff line) | `tipped_when_cut(element, text, window, cx)`: the same tooltip, shown only when the text really is wider than its frame (a probe shapes the whole text in the element's font and compares it with the width less its padding; the answer comes one frame late), `<id>-tooltip`; the element needs an id, `overflow_hidden` and `text_ellipsis`. `ListRow`, `TreeRow`, `CardHead`, `FlatRow`, `LabelledLine`, `StepRow`, `PageHeader` and `path_line` do it for their own texts |
| a chevron | `turning(id, open, window, cx)` |
| a page's blocks, a card, anything that shows once | `enter(id, index, element)` |
| a block that comes and goes while its view stays (an empty-list line, the fields a choice needs, a body that unfolds) | `presence(id, present, element).gap(..)`: build it on every frame, outside a `when`; `.gap` is the gap the parent flex puts between its children, which folds with the block |
| a line that goes away with its text (an error, a refusal) | `presence_of(id, Option<line>, build)`: it still shows the last line while it folds |
| a line whose text changes while its view stays (the folder a setting results in) | `changing(id, text, build)`: it fades back in over `HOVER` when the text differs from the last frame's; build it on every frame |
| a block whose height follows its content (a field that wraps its text and grows by a line) | `growing(id, element)`: it eases to the new height over `PANEL`, also when it shrinks; build it on every frame; `<id>` is the animated frame, so its bounds are the height drawn |
| the rows of a list that comes from elsewhere | `Listing::sync` on each render, each row drawn inside an `Item`: it unfolds when it arrives, folds away when it leaves (it stays drawn until then), slides when it moves |
| the sessions a session leads, under its row | the sidebar's `Level` draws them as `Item`s that unfold from nothing and fold away (`framed` leaves `.rising()` to the rows that arrive at full size); each is tied to the row above by `tree_tie(row, &Tie, first_line, shown, cx)`, a thin line in `p.line` that comes down and turns right into the row (├ with a line going on below, └ for the last one, a line past it for each ancestor with a sibling below), drawn inside the row so it folds and fades with it; `<row>-tie` says `Branch` or `Last branch` |
| a level that folds | `Fold` (a view) or `fold_frame` |
| a part that waits for a slow read, then shows it | `awaiting(id, pending, Skeleton, content)`: the skeleton fades out and the content in at the same place, the height easing from one to the other (to nothing when nothing came); build it on every frame |
| a panel over the page (a menu, a drawer, a modal, the bell's list) | rises a little and fades in, fades out, and stays drawn until it has |

Never in a screen: `.hover(..)` or `.group_hover(..)` (they switch at once; `tests/the_rule.rs`
of the window crate refuses them), a state that changes by swapping one element for another with
nothing between (wrap what appears in `enter`, `presence` or an `Item`), a `Motion` kept by hand
when a helper above does it.

**The guard.** gpui-kit's test support (`test_support()`, `window.find(..)`) says an element's
role, label, bounds and focus, not which listeners it has: no test can walk a view and ask each
interactive element for its hover transition. What a test can hold is held: the instant hover
style is refused in every source of the window (`crates/flottille-window/tests/the_rule.rs`) and
of the kit (its `tests/instant_hovers.rs`), each helper has its own scenarios (the kit's
`tests/everything_moves.rs`: a hover fades and stops, a row arrives, goes and slides, a block
folds, reduced motion jumps, nothing is drawn once it arrived), and the rest is this checklist,
which each window pull request ticks in its description:

- [ ] every element that is clicked fades its hover (a component, `hoverable` or `selectable`);
- [ ] every state that changes (selected, chosen, focused, open, enabled) moves, not jumps;
- [ ] everything that appears, leaves or changes size is an `enter`, a `presence`, a `growing`,
      an `Item` or a `Fold`, and the gap around it goes with it;
- [ ] every list that gains, loses or reorders rows is a `Listing` of `Item`s;
- [ ] every new page, section or panel enters (staggered), and every overlay rises and fades;
- [ ] reduced motion jumps (the helpers do it; a `Motion` kept by hand must pass `reduce`);
- [ ] nothing is redrawn at rest: a scenario ends on `pending_frames == 0`, and a page the pull
      request touches is held by a scenario of `crates/flottille/tests/window_at_rest.rs`, which
      counts the frames drawn over ten seconds of the window's clock once the page settled
      (`window.layout_stats().frames`, none) and the animation frames asked (none);
- [ ] the pull request lists each element it animated, and a capture or a scenario shows one.

## Rules of the window

The window holds together because every screen follows the same rules and builds them from the
same components of the kit, which make the wrong thing hard to write. **[test]** marks a
rule a test holds: the components' scenarios are in the kit's `tests/rules.rs`, the scans
of the window's sources in `crates/flottille-window/tests/window_rules.rs` (each shown a sample of
the wrong code first).

1. **Buttons: four kinds, one size.** Primary (filled accent, at most one per view: Save, Create,
   Send, Allow), Secondary (transparent, bordered: a page's or a section's actions), Quiet (no
   border: Cancel, Deny, lesser actions), Danger (filled fail colour: only the action of a
   confirmation of something destructive). Every text button is 30 px high, 13 px medium; an icon
   button is a 28 px square with a tooltip, anchored to it (below, centred, above when there is no
   room below, inside the window; never GPUI's `.tooltip(..)`, which opens under the pointer).
   **[test]** `Button` takes no size, and the kit's `tests/tooltips.rs` holds the
   tooltip. A page header holds at most two secondary buttons, on the title's line, and a "⋯" menu for the rest
   (**[test]** `PageHeader` refuses a third). A destructive action is never a visible button in a
   page or a row: it is a "Remove…" item of a "⋯" menu, last. A label never repeats a name the
   title already says.
   A "⋯" menu (`Menu`) is as wide as its longest label: an entry is one line, cut only when it
   could not fit the window; every entry has an icon of its own (the menu refuses two entries
   with the same one; the destructive entry's is the bin), the labels start at one x, and an entry
   offers only what applies (a second, fainter line may say what it acts on, never wrapped).
   **[test]** the kit's `tests/menus.rs`.
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
   dialog. A switch's label names what it turns on, with a verb or a name ("Review changes",
   "Claude"), never its state ("On", "Enabled", "Use"): the switch already draws whether it is
   on. A setting that only means something for one choice (a switch on, a radio chosen) unfolds
   right under it, its label read after the choice's ("Compact automatically", then "When the
   context reaches"), never shown greyed elsewhere on the page. A field saved on its own shows
   no Save at rest: a secondary Save unfolds beside it once its value differs from the saved one,
   and folds away after it is saved or set back.
5. **A state is said once per view**: an agent's state by its icon on its own row, "needs you" in
   one place. **[test]** on the session page, "needs you" is said at most once.
6. **Where things live**: the project page shows and runs, the project settings configure; a
   setting editable in two places is a defect; a run opens its detail in a drawer.
7. **Lists, one pattern**: a configured item is a row (icon, name, tags, one line) in a
   `ListCard`; a click opens its edit dialog; its "⋯", shown on hover, holds Edit, the moves and
   "Remove…"; an empty list is one dashed line saying what to add and why; reordering is by drag,
   with Move up and Move down in the "⋯". A separator is drawn only between two blocks that exist.
   **[test]** `Card` draws a block's separator only when the block has content.
8. **Words**: paths relative to the project folder or to `~`, never absolute; a remote in its
   short form, labelled; labelled values when there are more than two, never a run of mono values
   separated by dots; a section has a title, a count when it is a list, and one sentence saying
   what it is for (a section that reads alone, a few switches that say it all, has none). The
   window speaks of what the user sees and does, never of how Flottille is built: no *pilot*,
   *pass*, *verifier*, *slot*, *compactor*, *daemon*, *worktree*, *handoff*, no file of the
   configuration, no path of a login file, no name of an environment variable. A help line is
   said once: one that four fields share goes up a level and is said there, one that repeats the
   option chosen goes. Sentence case everywhere. The same gesture has the same verb (**New**
   step, skill, command, task, entry; **Check again** for any read made anew) and the same thing
   the same name everywhere ("Rules for every project" in Settings and in a project). Durations,
   not seconds ("stops after 10 min").
9. **Spacing**: the scale 4, 8, 12, 16, 24, 32, 48 px (`gpui_design_system::space`). **[test]** the
   constants, and a scan refusing any other gap or padding literal in the window crate. A page's
   column is 880 px at most with 32 px between its sections; a dialog is 520 px (640 wide) with
   24 px of padding.
10. **Motion**, as above: everything that appears, leaves, opens, folds or changes state moves
    with the shared durations; a drawer slides from the right; a select's list fades in; a
    radio's dot scales in. **[test]** each component's scenario.
11. **Waiting**: a section that waits for a read shows its skeleton, never an empty space nor a
    spinner alone. What is known is drawn at once and only the slow parts (a forge, a tracker)
    wait, each by its own skeleton, of the size of the line it stands for, replaced in place
    when its read ends or by its reason when it failed. A skeleton is `Skeleton`, wrapped in
    `awaiting`: it pulses slowly (no shimmer), and the content crossfades into it. **[test]**
    the kit's `tests/skeletons.rs`.
12. **The composer's toolbar**: Send is its only primary; a control that applies only in some
    states (Compact) stays in its place, disabled with its reason as tooltip, and a chip that
    exists only in some states (`Resume at …`) goes at the end of the row, after the fixed
    controls, and animates in, so that nothing moves; a short row (under 740 px) gives the chips
    their icons, never an overflow. **[test]** `window_compaction.rs`, `window_quota.rs` (a 1280
    px window), `window_rules.rs`.
13. **Tabs by subject; a feature is a row, never a tab.** Settings has five tabs (Agents,
    Approvals, Features, Vault, General) and six at most, Usage the last; a project's settings
    have five (Repositories, Workspaces, Tasks, Instructions, Features); the project page four at
    most and room for a fifth. A new part of Flottille adds no tab: a part the user turns on or
    off (Specs, Review, the compaction, Connections, later the handbook) is a row of the
    Features tab, which opens its page in place of the list (`Subpage`); a setting every agent
    shares is a section of a tab that exists. A tab with a single control is a section of
    another.
14. **A feature's page, one pattern.** It starts with the same block, in this order: its switch,
    labelled with what it does ("Review changes", "Write specs before building", "Compact long
    conversations", "Let a model decide in Auto"); its hint, one sentence of what it does and
    what it costs; its **usage line**, which says where and how it acts, the words found
    elsewhere in the window in bold (`/review`, **Write the spec**, **Compact**, **Auto**),
    shown even while it is off, so that the user knows what a switch is for before turning it
    on. Off, the page is that block alone, the rest folding away with the switch. In a
    project, the same page holds the project's switch, with the global switch's label (the
    path says the project), and each value a project may set offers **As in Settings** first,
    its second line the value it gives; a value set in the project carries a dot, "set here",
    and a link under it, **Use Settings' value**. On the global page, one line names the
    projects that do otherwise (`notes-app turns it off`, `acme-shop runs Security too`), each
    a link, and only when there is one.
15. **One way to choose a model**: `ModelChoice`, wherever a model is chosen (the judge, the
    compaction's summary, each of Review's checks, what comes later). One select, its entries
    `<provider> · <model>` grouped by provider, with the effort in a narrow select beside it when
    the model has efforts. A default that is a rule is the first option, said as the rule and, on
    its second line, what it gives today (`Strongest of the session's provider`, `Opus for
    Claude`; in a project, `As in Settings`); a model Flottille picked is the one chosen, marked
    `picked` until its notice is seen. A choice that is not a model (the compaction's `The
    session's own agent`) comes before the models. "No model" is never an entry: a feature is
    turned off by its switch. Under it, one line of cost
    ("Each decision is a turn of this model, on your own quota."). Several choices in a row are
    one card with a line each and the compact select on the right, the default and the cost said
    once under the card, never stacked full-width selects.

The components that hold them:

| Component | What it is for |
|---|---|
| `Button::{primary, secondary, quiet, danger, icon}` | rule 1; `.leading(Icon)`, `.icon_only(bool)` (a short row gives a chip its icon), `.disabled(bool)`, `.badge(Badge)` |
| `PageHeader::new(id, title)` | `.path(p)`, `.crumb(el)`, `.below(el)`, `.action(id, label, Option<Icon>, f)` twice at most, `.more(Entity<Menu>)` |
| `Section::new(id, title)` | `.count(c)`, `.sentence(s)`, `.action(Button)`, then its children |
| `Menu::new(..).entry(..).danger(id, label, f)` | a "⋯" menu; danger entries come last, after a line |
| `Confirm::new(id, question)` | `.sentence(s)`, `.action(label)`, `.refused(Option<why>)`, `.open(bool)`, `.on_confirm(f)`, `.on_cancel(f)`; ids `<id>-action`, `<id>-cancel`, `<id>-scrim` |
| `Modal::new(id, title)` | a form dialog: `.sentence(s)`, `.wide()`, `.action(Button)`, `.on_close(f)`; its body scrolls between the title and the buttons when the window is too small, and `.kept(el)` stays in sight under it (a refusal). `.large()` makes it nearly the whole window (a 32 px margin) for looking at something at length: the title and sentence left in the header, `.action(..)`s and a close button (`<id>-close`) on its right, no Cancel and no footer, and the body takes the height left (its owner scrolls it); `.takes_focus()` so that Escape reaches it. `.search()` makes it a search over the page (the quick switcher): near the top, 640 px, no title drawn (it is its label) and no Cancel, `.above(field)` over the results, `.scroll(&handle)` for the body, `.on_key(f)` for the arrows. While a dialog or a list is open, `keys_held(cx)` says so and the window's shortcuts wait |
| `toast(Toast::success(t).detail(d), cx)` / `Toast::failure` | rule 3; the shell owns the `ToastStack`; `.action(label, f)` adds one secondary button under the lines (`toast-<n>-action`), whose click runs `f` and closes the toast: for something the user may do with what the toast says (Open a link an agent shares), never for a result; `.keyed(key)` names what it says, so that a toast pushed with the key of one on screen replaces its lines instead of adding a second (`toast::shown(key, cx)` says whether one is on screen): the links an agent shares in a row |
| `Field::new(id, label, control)` | `.optional()`, `.hint(h)`, `.hint_mono()`, `.error(Option<e>)` |
| `Switch::new(id, on, label)` | `.hint(h)`, `.on_click(f)`; the whole row toggles |
| `Segmented::new(id, label, choices, chosen)` | at most three choices, never in a dialog; `.unavailable(i)` shows a choice that cannot be taken here, faded over `HOVER` and with no hover, a line beside it saying why |
| `RadioChoices::new(id, label)` | `.choice(key, title, line)`, `.leading(Icon)`, `.chosen(i)`, `.on_choose(f)`; two to four |
| `Select::new(id, options, chosen)` | `SelectOption::new(key, title).line(l)`, `.fit()`, `.disabled(b)`, `.on_choose(f)` |
| `Drawer::detail(id, view, cx)` | a run's detail: 520 px from the right, no scrim, Escape closes; `.show(window, cx)` |
| `Card::new().id(id).block(Block)` | a card whose separators are drawn only between blocks with content |
| `ListCard::new(id).row(ListRow)` | rule 7's list; `.scrolling(&ScrollHandle, max_height)` makes a long one scroll inside its card (the screen keeps the handle: it holds the place and takes it back to the top); `ListRow` has `.mono()`, `.tag(t)`, `.line(l)`, `.more(Entity<Menu>)` (its clicks stay its own), `.below(el)` (labelled values, a reason under the title); `.item(id, placement, row)` draws a row of a `Listing` in its `Item`; a search's result has `.pieces(..)` (what matched, in the accent), `.marked(bool)` (the row the keys move to, its fill moving over `HOVER`) and `.on_enter(f)` (the pointer entered it) |
| `Tabs::new(id, label, chosen).tab(key, words)`, `.counted(key, words, count)`, `.on_choose(f)` | a page of sections shown one at a time (Settings, the project's settings, the project page): as many tabs as sections, a count after the name of a list, the marker slides to the tab chosen over `PANEL`; the page draws the chosen section alone, inside an `enter` that plays again as it is chosen again |
| `Subpage::new(id, label)` | rule 13: a list that opens, in place of itself and inside its tab, the page of one of its rows. `.entry(key, icon, title, tag, line)` draws a row of a `ListCard` (its icon, its name, its state as a tag, said once: `On`, `Off`, `2 on`, and its one line, what it is for, never its state); `.open(Option<key>)`, `.on_open(f)`, `.page(el)`. Above the open page, a path line, `Features / Review`, whose first word is a link back to the list, as Escape is. The page enters sliding from the right while the list leaves to the left, over `PANEL` with a fade; going back does the reverse. The tab keeps the page open while another tab is shown, and a link (a notice, a mode's "unavailable" line, an action of the quick switcher) opens `Settings › Features › Review` directly with the setting in view; ids `<id>-<key>`, `<id>-back` |
| `ModelChoice::new(id, label, choice)` | rule 15: `.default(rule, gives)` for its first option, `.effort(Option<effort>)`, `.compact()` for a line of a card, `.cost(line)`, `.inherited(gives)` in a project (its first option `As in Settings`), `.on_choose(f)`; the options come from the providers enabled and ready, their models and efforts as the daemon knows them |
| `Skeleton::new(id)` | rule 11: `.w(px)` (all the room without), `.h(px)` (12 px without), a block in the faint fill that pulses once every `PULSE`, in its own small view; `awaiting(id, pending, skeleton, content)` crossfades it into the content |
| `EmptyLine::new(id, Icon, text)` | an empty list; `.action(Button)` |
| `fuzzy::find(query, title, context)` | a search's matching (the window's quick switcher): each word typed in the title, else as a whole in the context, ignoring case, accents and ligatures; a score and the marks, `Match::pieces(title)` for a `ListRow` |
| `space::{XS, SM, MD, LG, XL, XXL, XXXL}`, `COLUMN`, `DIALOG`, `DIALOG_WIDE` | rule 9 |

`cargo run --example components`, in the kit, shows them on one page (`--dialog`, `--confirm`,
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
  window's priorities live in `gpui_design_system::layer` (dialog, list, toast); a new floating
  layer takes one there (a pull request in the kit), never a number of its own.
- A path cut by its frame is copied whole by a "Copy the path" icon button beside it (the edit
  card and the file steps), with a `Toast::success`: the click is the button's own
  (`cx.stop_propagation()`), it neither opens the card nor folds the steps. GPUI wraps a long
  unbroken word (a path, a hash) at any character by itself, so a toast keeps it inside; a
  `whitespace_nowrap` text (a tooltip) must be broken by hand.
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
