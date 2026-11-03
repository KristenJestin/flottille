# Extensions

**Status: a proposal, nothing built.** Written from a discussion with the maintainer
(November 2026). The maintainer answered its open questions on 2026-11-03; the decisions are in
"Decisions taken", and no question is left open.

Some of Flottille's features add a capability of Flottille's own on top of what it is: the Auto
judge, the compaction, and later specs, a cold read, a reviewer, a handbook. Each brings its
settings, its agents, its pages and its triggers. Built one by one, each would add its own tab, its
own way of picking a model, its own guidance file, its own hooks in the daemon, and the core would
end up naming every one of them. This note makes them **extensions**: parts of Flottille that
declare what they bring, which the core runs, and which the user can turn on or off from their own
settings.

Extensions are **how Flottille is built, not something the user sees as such**. Each feature sits
natively in the app, where it belongs, and its settings are drawn with the same components as the
rest of Settings.

It does not describe a framework to write ahead. The judge is extracted first, the compaction
second, and the frame holds only what these two need; Specs comes third and is the proof that the
frame was right ("The order of the work").

## Words

| Word | Meaning |
|---|---|
| Core | What Flottille is without any extension: the daemon, the store, projects, workspaces, agents and their tree, deliveries, the gate and bare mode, the generic tools, the providers, the window's shell. |
| Extension | A part of Flottille, compiled in, that adds a capability of its own and can be turned off with Flottille still working. |
| Declaration | What an extension says it brings: settings and where they appear, calls, roles, triggers, tools, data, screens. The core reads it and runs it. |
| Place | Where an extension's settings appear in the window: its own tab of Settings (with a title and an order among the tabs), or a section of a screen that exists. |
| Model slot | A setting that names a model: provider, model and effort, picked like the Auto judge. |
| Guidance | An editable text an extension's model follows, set inside a fixed text it cannot move, with Reset to default. |
| Call | One question to a model with no tool at all: input given by Flottille, output checked by the extension. |
| Trigger | What makes an extension act: an event of the core, a time, a gesture of the user. |
| Active | Turned on by the user, globally and, for an extension that touches a project, not turned off in that project. |

## What an extension is, and what it is not

The test: **if it can be turned off and Flottille still works, it is an extension.** The judge is
the edge case: the gate is core, and the judge plugs into it to decide the calls to judge in Auto.
Without it Flottille works, Auto does not: every call to judge comes to the user.

An extension is:

- **Built into Flottille.** Its code is in Flottille's repository, compiled into the one binary,
  reviewed and tested as the rest. Nothing is installed, downloaded or loaded at run time.
- **Declared.** It says what it brings; the core renders its settings, picks its models, runs its
  calls, launches its roles, dispatches its triggers, grants its tools, keeps its data and gives
  its screens a place. The core never names an extension: a test holds it ("The first step").
- **Under the same rules as the rest.** Every tool call of its roles goes through the gate; its
  models run on the user's own quota, through the user's own CLIs, in bare mode; what it does is
  recorded and emits events; its screens follow the rules of the window, motion included
  (`reference/gpui.md`).
- **Native in the window.** It has no "extension" to show: its settings are in the place it
  declares, drawn by the core with the components of the other tabs, and the word never appears in
  the window.
- **Off unless the user wants it** (see "Activation" for the defaults). An extension turned off
  does not exist for the user but for the switch at the top of its settings and one line saying
  what it does: no screen of its own, no tool given to any agent (the tool lists stay what they
  were), no trigger, no quota spent.

It is not:

- **A plugin of a third party.** Bare mode is the product: an agent can do only what Flottille's
  tools let it, and every call is seen by the gate. Code of someone else's in the daemon would run
  with the daemon's rights, see every conversation and every secret the daemon masks, and act
  where the gate does not look: it would end bare mode, whatever it promised. It would also make
  Flottille's interfaces a public contract to keep stable, bring a supply chain to trust, and put
  screens in the window that no rule of the window holds. Flottille is for one person; what they
  want added is added to Flottille.
- **A tool, a skill, a role or a provider.** These exist and stay what they are. An extension may
  bring roles and tools; a new role or a new tool alone is not an extension.
- **A way around the core.** An extension reaches agents, the store and the window only through
  what the core gives it below. It never spawns a process of its own, never writes the store
  outside its space, never draws outside its slots.

## The declaration

An extension is a Rust value of the daemon (`crates/flottille/src/extensions/<name>/`) and, when
it has screens of its own, a module of the window (`crates/flottille-window/src/extensions/`). The
registry is one list, the only place that names them:

```rust
pub const ALL: &[&Extension] = &[&judge::EXTENSION, &compaction::EXTENSION];

pub struct Extension {
    /// Stable: its settings' keys, its events, its methods, its store tables.
    pub name: &'static str,        // "judge"
    pub title: &'static str,       // "Auto judge"
    /// One sentence: what it does and what it costs. It is the line the settings show alone
    /// while the extension is off.
    pub sentence: &'static str,
    pub scope: Scope,              // Global; Project when Specs comes
    pub on_by_default: bool,
    /// Where its settings appear: a tab of its own, or a section of a screen that exists.
    pub place: Place,
    pub slots: &'static [Slot],
    pub guidance: Option<Guidance>,
    pub settings: &'static [Setting],
    pub calls: &'static [Call],
    pub triggers: &'static [Trigger],
    // Later, when Specs comes: roles, tools, data, screens beyond the settings.
}

pub enum Place {
    /// Its own tab of Settings: the title, and the order among the tabs (the core's tabs
    /// have theirs; the order is a number, so that no tab names another).
    Tab { title: &'static str, order: u16 },
    /// A section of a screen that exists (a tab, a project's settings), at its order.
    Section { screen: Screen, order: u16 },
}
```

What follows is each kind: what the extension gives, and what the core provides. Each is limited
to what the judge and the compaction need; what only Specs and Review need is marked **later**.

### Settings

The extension gives a schema: each setting's key, label, one line of hint, kind and default. The
kinds the compaction needs: a switch, a percentage, a number of tokens (empty for none), a
duration. Nothing else until an extension needs it.

The core provides:

- **Storage.** `$XDG_CONFIG_HOME/flottille/extensions.toml`, one table per extension
  (`[compaction]`), holding `enabled` and its settings, editable by hand and refused key by key as
  `providers.toml` is (an unknown key, a value of the wrong kind, naming the key at fault). A
  project-scoped extension's per-project switch is in the project's own file, under
  `[extensions]` (later). The judge keeps its files as they are (below, and "Decisions taken").
- **Methods.** `extension.list` (every extension: name, title, sentence, scope, on or off, its
  slots and settings with their values, whether its guidance is the user's or the default, and
  why one cannot be used) and `extension.set {name, enabled?, slots?, settings?}`, checked against
  the schema, emitting `extension.changed {name}`. The user's door only: no tool of an agent
  reaches them, as for `judge.set`.
- **One renderer** that draws every schema in the place the extension declares, with the
  components of the other tabs ("In the window"). No extension draws its own settings.

### Model slots

The extension names each slot (`compactor`) and its label. A slot holds a provider, a model and
an effort; the model and the effort may be left to the provider's defaults.

The core provides what the judge does today, made general (`providers.rs`, `pick_judge`):

- **Choosing** among the providers enabled and ready, their models and efforts as the daemon knows
  them (`agents.md`, "The models"), checked as an agent's are: a model or an effort the provider
  is known not to offer is refused, naming those it offers.
- **Picking for a user who never chose:** the smallest model of the first provider that is enabled,
  installed and signed in, among those whose smallest model Flottille lists (`SMALLEST`, Claude's
  Haiku). Written once, said once in a notice that opens the extension's tab, marked `picked`
  until the notice is seen. A slot the user set to none is never picked again.
- **Using it:** a provider no longer enabled or ready makes each call of the slot fail, and a call
  that fails falls back as its extension says (the judge asks; the compaction changes nothing).

The judge's slot has no effort today; it is read as "the model's default", which is what it does
now, so the field changes nothing until the user sets one.

### Guidance

The extension gives its fixed text (with the place of the guidance in it), its default guidance,
its limit in characters, and the words that name it in a refusal ("your judge's guidance").

The core provides what `judge.rs` does today for `judge.md`:

- the file `$XDG_CONFIG_HOME/flottille/<name>.md`, absent while the default holds: `judge.md` is
  already that, `compaction.md` comes next;
- the read at each call, so that an edit reaches the next one; a file that cannot be read whole,
  is not a regular file, is not UTF-8 or is over the limit is never cut nor replaced by the
  default: each call fails, saying which file and why, and the settings show it;
- the methods `<name>.show`, `<name>.set {text}` and `<name>.reset`, with the answers and the
  event (`<name>.changed`) of `judge.show`, `judge.set`, `judge.reset` and `judge.changed`
  (`protocol.md`, "The judge methods"): the judge's are the first case of the convention, not a
  special case. Saving an empty text resets it.

A guidance can add to what the fixed text asks, never lift it: what reaches the model and what its
answer may be are the extension's code, never its guidance.

### Calls

A call is a question to a model with **no tool at all**: no Flottille tool, no MCP server, an
empty folder, the provider's bare mode. The extension gives the slot it runs on, its time limit,
the input (built from what Flottille holds, masked), and a check of the output. The core provides
the run, taken from `judge.rs` (`ask`, `session`, `as_agent`, `Group`):

- one process of the slot's provider per call, a fresh session, the instructions (the fixed text
  around the guidance), the slot's model and effort set and checked, one prompt, its whole process
  group killed after the answer or when time is up;
- the answer as text, or why there is none (refused to start, no such model, a failed turn,
  silent past the limit);
- nothing kept from one call to the next.

The extension's check turns the text into its result or a failure: the judge's verdict is one
JSON object, `allow` or `ask`; the compactor's summary must not be cut off by length, must be
smaller than what it replaces, and must not try to call a tool. A failure always takes the safe
path, which the extension names: the judge asks the user, the compaction leaves the agent in its
session.

Each call is recorded: who ran it (provider and model), for which agent, how long it took, how it
ended. The judge already records it in the `judgement` of `agent.tool_served` and of its request;
the compaction in `agent.compacted`. Both go to the developer log with their input and their
answer, as the judge's do (`developer.md`).

### Roles (later, when Specs comes)

A role of an extension is a real agent of the tree, with its role's instructions and tools,
reporting by delivery like any other: the spec session, the cold read, the reviewer, the handbook
agent. Roles are already data (`design.md` §4.4); the extension declares its templates and the
tools each gets, and the core launches them as `agent_launch` does, under the caps.

The cold read and the reviewer are **children in the tree**, launched by their trigger, not left to
an agent's goodwill: a fresh context is what they are for. Whether such a launch counts against
the project's children cap, and what happens above it, is settled when Specs comes.

Neither the judge nor the compaction has a role.

### Triggers

The extension names what it reacts to and the handler that acts. The core dispatches; the
handler starts a call, or, later, a role. The kinds needed first:

| Trigger | When the core fires it | Who needs it |
|---|---|---|
| **Decide** | the gate holds a call to judge in a tree in Auto. Synchronous: the gate waits for the answer (`allow`, or no verdict, which asks), within the call's time limit. At most one active extension may decide | the judge |
| **Turn ended** | an agent's turn ended, with its context in use (`agent.usage`) | the compaction (threshold, maximum size) |
| **Agent timer** | a time the extension set for an agent has come; set, moved or cleared by the extension, dropped when the agent ends | the compaction (before the cache expires) |
| **Before a prompt** | the core is about to send a prompt to an agent between turns. Synchronous: the handler may run before the prompt goes, which then goes as it would have | the compaction (a message to a cold session) |
| **Action** | the user asked for it: a button of the window, a CLI command, a protocol method (`extension.act {name, action, agent}`) | the compaction (Compact) |

Later, when Specs comes: "Ready asked" (the user's gesture on a spec), "a development session
reported" (a `child-report` or a root's report of a session tied to a spec), a merge seen by the
items' rounds.

A trigger never wakes an agent by itself: when an agent must be told, it is a delivery, as
everything else (`design.md` §5.2). A handler runs off the daemon's request path except for
Decide and Before a prompt, which hold what they decide on and nothing else.

### Tools (later, when Specs comes)

An extension declares a tool (`spec`, `handbook`) and the roles that get it, with its actions per
role (the spec session writes a spec, a development session proposes an amendment, the reviewer
reports a finding). The core adds it to those roles' lists while the extension is active, through
the path a forge or a tracker takes today (`tools.md`, "When the tools change"); every call goes
through the gate like any other. The judge and the compaction give no tool to anyone.

### Data

An extension that keeps anything keeps it in **its own space of the store**: tables named
`ext_<name>_…`, created by migrations the extension declares and the store runs in order with its
own. Only the extension reads and writes them; the core and the other extensions never do. They
are kept when the extension is turned off.

The judge keeps nothing of its own: its verdicts are part of the core's records (the call's event
and its request). The compaction keeps each agent's last summary, so that the next one updates it
rather than starting over (`ext_compaction_summary`: the agent, the session it summarised, the
text, when).

An extension may also add facts to what the core says of an agent, under its own name
(`agent.tree`'s `extensions.<name>`): the compaction says there whether the agent's cache has
expired and whether its context is above the minimum size, which the composer reads.

### Screens

The window's shell offers places; an extension's window module fills them, built from
`flottille-ui`'s components and under the rules of the window. The places needed first:

- **Its settings, in the place it declares** (its own tab, or a section of a screen that exists),
  rendered from the declaration: none needs code of its own there ("In the window").
- **An element of the composer**: the compaction's "Compact before sending".
- **An action of a session**: Compact, in the root's "⋯" menu of the rail.
- **A line of the conversation** for what the extension did: a compaction, folded to one line,
  unfolding to the summary.

Later, when Specs comes: a project tab (Specs, Handbook), a page (a spec), an action on a ticket's
row ("Write the spec"), and a section of the project's settings for its per-project switch.

### One thing more the compaction needs

With a compactor active, each CLI should compact as late as it allows rather than never (Claude's
auto-compact window, Codex's token limit, OpenCode's `compaction`), so that it only acts when
Flottille could not. That is set where an agent starts, in the provider modules, which must not
name an extension. The declaration carries it as a flag of the extension, `takes_over_compaction`;
a provider asks the registry whether an active extension sets it. It is the only such flag, and
stays one until a second extension needs to shape how an agent starts.

## Activation

- **Global**, for every extension: on or off, by the switch at the top of its settings (its own
  tab), `extension.set`, or `flottille extension on|off <name>`. The command and the methods are
  the scripts' and the clients' door; the window never says "extension".
- **Per project**, for an extension that touches a project (Specs, Review, Handbook; later): once
  turned on globally, it is **on in every project**. The user turns it off in a project where they
  do not want it, by its switch in that project's settings, shown only while it is on globally.
  It is not off by default in each project. Off globally is off everywhere; the projects' switches
  are kept for when it comes back.

**Defaults.** The judge is on, as it is today: a user who never chose gets one picked, and "No
judge" is the judge turned off (`judge = "none"`, kept as it is written now). Every other extension
is off until the user turns it on, since each spends the user's quota by itself.

**What off removes.** Its tab's content but for the switch and its sentence, its screens (the
composer's element, its actions, its project tab), its tools from every role's list, its triggers,
its notices. For the judge: Auto has no one to decide, is shown unavailable, and every call to
judge comes to the user. Its settings, its guidance and its data stay where they are: turning it
on again brings it back as it was.

**Turned off while it works.** The rule, the simplest that can be undone: **off stops what would
start, never what runs, and a result that comes back after it is dropped the way a failure is.**

- No trigger fires from the moment it is off: no new call, no new role, no timer.
- A call already running ends as it would, and its result is not applied: a judgement that comes
  back asks the user (a judge that fails asks), a summary that comes back leaves the agent in its
  session (a compaction that fails changes nothing). Both safe paths already exist.
- An agent of one of its roles already running goes on to its end and reports to its parent; it
  is ended like any agent, by its parent or with its session (later).
- Its tools leave the role lists at once, through "When the tools change", as a forge that is
  dropped (later).
- A request the judge did not allow stays the user's, as one made before a switch of mode does.

Turning it on again starts nothing for what happened while it was off: it acts from the next
trigger.

## In the window

Each feature is part of the app, and its settings sit where they belong: the Judge keeps its own
tab of Settings, as today; the Compaction gets its own tab; a later extension declares its own tab
or its section of a screen that exists (its `place`). The declaration says *where*, with a title
and an order; the core draws what is in it, with the components every other tab is built from.

- **A tab of an extension** is a page like the others (rule 8 of `reference/gpui.md`: a title, and
  sections each with its title and one sentence). At its top, its **switch** (On), with its
  sentence as hint. **Off, the tab shows the switch and that one line, nothing else.** On, under
  the switch and in that order: its model slots (a select for the provider and the model, one for
  the effort, a "None" among them only where none is a real choice); its settings, each a labelled
  control chosen by rule 4 (a switch for a thing on or off, a field for a number, its unit in the
  label); its guidance (the editor the Judge guidance has now, with Reset to default, Save and the
  line on when a change applies); and the one line saying what it costs (a model on the user's own
  quota). The sections fold away and unfold with the switch.
- **A link to a tab** (the mode's "Auto unavailable" line, a notice that a model was picked) opens
  that tab, with the setting in view.
- **The judge's tab** keeps its place, the first, the one Settings opens on. Its two lines on Auto
  become the sentence and the hint of its switch; nothing of what they say is lost. Its switch is
  the choice "No judge" made visible: off writes `judge = "none"`, and on picks a judge as for a
  user who never chose, since the file keeps no earlier choice once it says none; the "No judge"
  entry of its select goes, the switch says it.
- **The compaction's tab** holds Automatic, Threshold, Maximum size, Kept word for word, the two
  cache options and their margin, Minimum size, its slot and its guidance, and says the cache
  lifetime it uses for each provider, or that it knows none for one.
- **Tool results** (#308, core): a section at the end of the Providers tab, since it is about what
  every agent is given, whatever runs it.
- **A project's settings** get, for each project-scoped extension that is on globally, a section
  with its switch (on, unless the user turned it off in that project), at its order (later). It is
  a section of the screen that exists.
- **The composer, the rail and the conversation** carry the compaction's element, action and line
  where they belong ("Screens"), with no mention of extensions.

Settings, in order: Judge, Compaction, Providers, Rules, Appearance, Developer mode. The order is
the one the declarations give (the Judge's and the Compaction's come first, as the Judge's does
today); a core tab has its own place, and none is moved by another's declaration.

Everything in it follows the rules of the window and its motion: the sections of a tab enter one
after the other, the switch moves over `HOVER`, the settings fold and unfold with the switch, the
guidance's error appears and leaves animated, the composer's element unfolds and fades in. The
pull request that builds it lists each and ticks the checklist of `reference/gpui.md`. This note
designs no pixels: the components of `flottille-ui` hold them.

## The extensions

What each declares, mapped on the kinds above. Only the first two are designed here; the last
three are listed so that the frame does not forbid them.

### Judge

As it is today (`design.md` §7.1), nothing of its behaviour changed.

- **Slot**: the judge, provider and model, `[judge]` of `providers.toml`, `judge = "none"` for
  off; picked for a user who never chose. Its files stay as they are (decision 3).
- **Place**: its own tab of Settings, the first, as today.
- **Guidance**: `judge.md`, the fixed text `judge-base.md`, the default `judge.md` of Flottille,
  4,000 characters.
- **Call**: the judgement. Input: the call masked, the reasons it was held, the place, the
  agent's `why`, the first message of a session an agent started, the user's last three messages.
  Output: `{"verdict": "allow" | "ask", "reason"}`. 30 seconds. Any failure asks.
- **Trigger**: Decide, at the gate, for a call to judge in Auto. Base never reaches it.
- **Data, roles, tools, screens of its own**: none but its tab. The mode's control (Ask | Auto |
  Full) is the core's; it asks the registry whether an active extension decides. With none, Auto is
  shown unavailable (decision 1).

What moves out of the core is in "The first step".

### Compaction

From #308, #309 and #320, with the decisions already taken.

- **Not in it: cutting long tool results at the source** (#308). It helps every agent, uses no
  model and cannot be "off" without Flottille being worse at what it does: it is core. Its limit
  is a setting of the core, a "Tool results" section at the end of the Providers tab (decision 5).
- **Place**: its own tab of Settings, after the Judge's. Off, the tab shows the switch and one
  line: what the compaction does and what it costs.
- **Slot**: the compactor, picked like the judge (the smallest model of the first provider
  ready) once the user turns the extension on. The extension off is #309's "None": each CLI
  compacts on its own, as today.
- **Settings**: Automatic (on), Threshold (70 % of the model's context window), Maximum size (a
  ceiling in tokens, empty), Kept word for word (20k tokens); from #320, Compact before the cache
  expires (off) and its margin (5 minutes), Compact a session resumed after its cache expired (on),
  Minimum size (50k tokens).
- **Guidance**: what the summary must keep, Flottille's default (#309's sections, and what only
  Flottille knows: the brief, the children, the subscriptions, the pending requests and
  deliveries, the workspace and its branches).
- **Call**: the summary. Input: Flottille's transcript of the agent as text (each tool result cut
  short, thoughts left out), the previous summary, the guidance. Checked: not cut by length,
  smaller than what it replaces, no tool call attempted.
- **Triggers**: Turn ended above the threshold or the maximum size, when Automatic is on; an Agent
  timer at the cache's expiry less the margin, when that option is on and the agent is at rest; Before
  a prompt to an agent whose cache has expired, when that option is on or the composer's toggle
  says so; the Compact action (the rail's menu, `flottille agent compact <agent>`).
- **What it asks of the core: the handoff.** The agent goes on in a new session of its CLI, as a
  replaced agent does (`agents.md`, "Replacement"): its id, its place in the tree, its children,
  its subscriptions and what waits for it kept; sent its brief, the summary, then its latest
  messages word for word. The handoff is the core's: the replacement path given a first message,
  which a change of provider will use too (`agents.md`, "The options").
- **Data**: each agent's last summary.
- **Screens**: the composer's "Compact before sending", the rail's Compact, the conversation's
  line for a compaction (folded, unfolding to the summary), and its own tab, which says the cache
  lifetime it uses for each provider, or that it knows none for one (then neither cache trigger
  runs for it).
- **The flag**: `takes_over_compaction`, so that each CLI compacts as late as it allows.

### Specs (later)

From the maintainer's proposal of specs. Project-scoped.

- **Roles**: the spec session (a root, reads in `main`, writes only its spec), the cold read (a
  child of the spec session, reads the spec alone).
- **Tools**: `spec` (read, write a section, ask a wave, list the open questions; for a development
  session, read and propose an amendment), `handbook` read when Handbook is on.
- **Triggers**: Ready asked (the cold read), a development session that ends without reporting (the
  spec back to Ready), its report (Review), its pull requests merged (Done).
- **Data**: specs, their statuses and lock, decisions, amendments.
- **Settings**: the project's spec mode and template; slots for its roles when they should not run
  on the session's own model. Their place: a tab of their own in Settings for the global ones, a
  section of the project's settings for the per-project switch.
- **Screens**: the project's Specs tab, a spec's page, "Write the spec" on a ticket, "Launch
  development" on a Ready spec. On in every project once turned on globally (decision 6).

### Review (later)

Project-scoped; needs Specs, since it reads a development against its spec.

- **Role**: the reviewer, a child of the development session, which reads the spec and the
  changes, nothing else.
- **Tool**: `spec`'s "report a finding".
- **Trigger**: a development session's report.
- **Data**: findings (severity, where, the `C` or `D` concerned, the case, a status).
- **Screens**: the findings on the spec's page.

### Handbook (later)

Project-scoped.

- **Role**: the handbook agent (the first read, the catching up).
- **Tool**: `handbook` (read for the spec and development sessions, propose for its agent).
- **Triggers**: a spec Done (its spec session proposes changes), a command, a merge Flottille did
  not make.
- **Data**: domains, entries with their history and state, the last commit seen per repository.
- **Screens**: the project's Handbook.

## The first step: extracting the judge

One pull request, daemon only, **no change of behaviour**: the same protocol, the same events, the
same files written the same way, the same texts, the same window. Its only visible effect is that
the core no longer names the judge.

### What moves where

| From | To | Why |
|---|---|---|
| `judge.rs`: `ask`, `session`, `as_agent`, `Group` | `calls.rs` (core): the run of a call, given a slot, instructions, a prompt and a time limit | any extension's model runs this way |
| `judge.rs`: `own`, `check`, `instructions_with`, `WRITING`, `call` (`judge.show`, `.set`, `.reset`), `show`, `announce` | `guidance.rs` (core), given the extension's `Guidance` (file, fixed text, default, limit, the words of its refusals) | the compaction's guidance is the same thing; the refusal texts stay word for word |
| `providers.rs`: `pick_judge`, `SMALLEST`, `PICKED`, `chose_none`, `judge_changed`, `edit_judge` | `slots.rs` (core) for picking; the judge's storage (`[judge]`, `judge = "none"`, the comment above a picked judge, `provider.judge_changed`) stays the judge's, behind its slot | picking is shared; where the judge is written is its own |
| `judge::mode` (the tree's mode) | `tree.rs` | it is the tree's, and five core modules read it |
| `judge::WHOLE_CHARS` | `gate.rs` | it is what the gate refuses above; the judge reads it from there |
| `Daemon::judge_chose_none` | the judge's own state | only the judge's picking reads it |
| `mcp.rs`: `(Class::Judge, Auto)` calling `judge::chosen` then `judge::judge` | the registry's Decide: the active extension that decides, or none, and then Auto asks as Ask does | the gate asks "who decides", never "the judge" |
| `daemon.rs`: `judge.*` routed to `judge::call` | `<name>.*` routed by the registry to the guidance of the extension of that name | the judge's methods become the first case of the convention |
| `judge.rs`: what is left (the prompt, `compose`, the escaping, the user's last messages, `verdict`, `chosen`) | `extensions/judge/` | it is the judge |
| `roles/judge-base.md`, `roles/judge.md` | `extensions/judge/`, included as now | they are the judge's, not a role's; `design.md` and `protocol.md` follow the paths |

New, and nothing else: `extensions/mod.rs` with the `Extension` type holding only what the judge
uses (name, title, sentence, scope, on by default, a slot, a guidance, a call, a Decide trigger),
and the registry `ALL` with the judge alone. `extension.list`, `extension.set`, `extensions.toml`,
the rendering of settings in a tab of their own (the Judge's tab is moved onto it) and the other
triggers come with the compaction, their first need beside the judge.

### What proves nothing changed

- **The tests that exist pass unchanged.** `crates/flottille/tests/judge.rs`, `approvals.rs`,
  `mcp.rs`, `session_read.rs` and the window's tests are not touched. The unit tests of `judge.rs`
  and of `providers.rs` that go with the code they test move with it, their bodies unchanged but
  for `use` lines; the pull request shows that diff.
- **The same bytes at the edges**, each checked by a test written before the move and kept after
  it: `judge.show`'s answer for a default, a user's and a broken guidance; `provider.set_judge` and
  `provider.list`'s `judge`; `providers.toml` after a pick (the comment and the table) and after
  "No judge"; the events `judge.changed` and `provider.judge_changed`; a judgement's record in
  `agent.tool_served` and in a request; the developer log's lines for a judgement. Those that do
  not exist yet are written first, seen to pass on `dev`, then kept green through the move.
- **New tests for the frame:**
  - `the_core_names_no_extension`: a scan of the daemon's core modules refuses `judge` as a module
    path or an identifier outside `extensions/` and the registry, as the window's rules are scanned
    (`window_rules.rs`), shown a sample of the wrong code first.
  - `the_judge_is_declared_with_its_slot_guidance_call_and_decider`.
  - `with_no_extension_that_decides_auto_asks_like_ask`: the registry emptied in the test, a call
    to judge in Auto is a request for the user.
  - `a_call_runs_bare_with_no_tool_and_its_process_group_is_killed`, moved from the judge's.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` clean.

## The order of the work

1. This note.
2. Extract the judge, as above.
3. The compaction as an extension: #308 stays core (tool results cut at the source); #309 and #320
   are rewritten into this frame (the compactor's slot and call, the handoff in the core, the four
   triggers, its data and its screens); with it come `extensions.toml`, `extension.list` and
   `extension.set`, the generic rendering of an extension's settings in the tab it declares (the
   Judge's tab moves onto it, with its switch; the Compaction gets its own tab), Auto shown
   unavailable when no extension decides, the "Tool results" section of the Providers tab for
   #308, and the generic picked-model notice.
4. Specs' base: the spec, its statuses and lock, the `spec` tool, the spec session's role, its
   window, launching the development. Project scope, roles and tools enter the frame here.
5. The cold read and Review.
6. The spec modes of tickets, then Handbook.
7. Remote. An extension's settings travel in `extension.list` and its results in events, which a
   remote client reads as the window does; nothing more is asked of the frame by this note.

## Decisions taken

The maintainer's answers of 2026-11-03 to what the first version of this note left open. **No
question is still open.**

1. **Auto with the judge off: shown unavailable.** After the extraction (which changes nothing),
   the control shows Auto unavailable (`Segmented`'s `.unavailable`, faded, with one line saying
   the judge is off and a link to the Judge's tab). A session already in Auto when the judge is
   turned off goes on asking as Ask until the user picks another mode, its mode kept.
2. **The extensions other than the judge are off by default.** Each spends the quota by itself;
   once the user turns one on, its slot is picked as the judge's is. #309 had the compactor picked
   with no gesture of the user: this asks for one click first.
3. **The judge keeps its files:** `[judge]` in `providers.toml` and `judge.md`. `judge.md` already
   fits the guidance's convention, and moving a file users may have edited by hand buys nothing.
   The new extensions use `extensions.toml`.
4. **No "Extensions" tab, no dialog, no list to open.** Extensions are how Flottille is built, not
   something the user sees as such: each is included natively in the app. Each one's settings sit
   where they belong: the Judge keeps its own tab, the Compaction gets its own, a later extension
   declares its own tab or its section of a screen that exists. Its on/off is at the top of its
   own tab; off, the tab shows the switch and one line saying what it does. ("In the window".)
5. **#308's limit on tool results** is a "Tool results" section at the end of the Providers tab:
   it is about what every agent is given, whatever runs it, and needs no tab of its own.
6. **Per project: on everywhere, off where the user says so.** An extension that touches a project,
   once turned on globally, is on in every project; the user turns it off in a project where they
   do not want it, in that project's settings. It is not off by default in each project (later,
   when Specs comes).
