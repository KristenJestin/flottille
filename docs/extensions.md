# Extensions

**Status: a proposal, S1 to S5 built.** S1 (#332) built the declaration and the registry,
activation global and per project, `extensions.toml`, `extension.list`/`set`, the data space and
the scan that the core names no extension, and Specs in the store with its statuses, lock, three
modes and tickets, from the command line. S2 (#333) built the tools an extension gives and the
Action trigger, and writing a spec: the `spec` tool, "New spec" and "Write the spec", questions and
answers, from the command line and the protocol. S3 (#334) built Specs in the window: the
renderer of an extension's tab of Settings and of its section of a project's settings, the
project's Specs tab, "Write the spec" on a ticket's row and a spec's page. S4 (#459) built the
ticket side. S5 (#336) built "Launch development" and the statuses read from what Flottille sees;
the rest is a proposal. Written from a discussion with the maintainer
(November 2026), and settled with their answers of 2026-11-03 ("Decisions taken"). No question is
left open but the one in "Open questions".

Some of Flottille's features add a capability of Flottille's own on top of what it is: specs, the
compaction, a reviewer and a cold read, the Auto judge, later a handbook. Each brings its
settings, its agents, its pages and its triggers. Built one by one, each would add its own tab, its
own way of picking a model, its own guidance file, its own hooks in the daemon, and the core would
end up naming every one of them. This note makes them **extensions**: parts of Flottille that
declare what they bring, which the core runs, and which the user can turn on or off from their own
settings.

Extensions are **how Flottille is built, not something the user sees as such**. Each feature sits
natively in the app, where it belongs, and its settings are drawn with the same components as the
rest of Settings.

It does not describe a framework to write ahead. **Specs is the first extension and builds the
frame as it needs it**; the compaction adds what it needs, then Review; the judge, which works
today, moves into the frame last ("The order of the work").

## Words

| Word | Meaning |
|---|---|
| Core | What Flottille is without any extension: the daemon, the store, projects, workspaces, agents and their tree, deliveries, the gate and bare mode, the generic tools, the providers, the window's shell. |
| Extension | A part of Flottille, compiled in, that adds a capability of its own and can be turned off with Flottille still working. |
| Declaration | What an extension says it brings: settings and where they appear, model slots, guidance, calls, triggers, tools, the agents it launches, data, screens. The core reads it and runs it. |
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
  calls, launches its agents, dispatches its triggers, grants its tools, keeps its data and gives
  its screens a place. The core never names an extension: a test holds it ("Data").
- **Under the same rules as the rest.** Every tool call of its agents goes through the gate; its
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
- **A role.** An extension gives **tools**, never a role: the session that writes a spec is a
  `lead` with the `spec` tool, not a "spec session". The agents an extension launches itself (a
  cold read, a reviewer) are workers with its instructions and its tools ("Agents an extension
  launches"). A new tool alone is not an extension either.
- **A way around the core.** An extension reaches agents, the store and the window only through
  what the core gives it below. It never spawns a process of its own, never writes the store
  outside its space, never draws outside its places.

## The declaration

An extension is a Rust value of the daemon (`crates/flottille/src/extensions/<name>/`) and, when
it has screens of its own, a module of the window (`crates/flottille-window/src/extensions/`). The
registry is one list, the only place that names them:

```rust
pub const ALL: &[&Extension] = &[&specs::EXTENSION];   // then compaction, review, judge

pub struct Extension {
    /// Stable: its settings' keys, its events, its methods, its store tables.
    pub name: &'static str,        // "specs"
    pub title: &'static str,       // "Specs"
    /// One sentence: what it does and what it costs. It is the line the settings show alone
    /// while the extension is off.
    pub sentence: &'static str,
    pub scope: Scope,              // Global, or Project
    pub on_by_default: bool,
    /// Where its settings appear: a tab of its own, or a section of a screen that exists.
    pub place: Place,
    pub settings: &'static [Setting],
    pub tools: &'static [Tool],
    pub triggers: &'static [Trigger],
    pub migrations: &'static [Migration],
    // Each kind below enters the type with the first extension that needs it.
}
```

Each kind comes with the first extension that needs it, named in its title; nothing is written
for an extension that is not being built.

### Settings (Specs)

The extension gives a schema: each setting's key, label, one line of hint, kind, default, and
whether a project may set its own value. The kinds come with their first need: a choice among
values and a language (Specs), a switch, a percentage, a number of tokens, a duration (the
compaction).

The core provides:

- **Storage.** `$XDG_CONFIG_HOME/flottille/extensions.toml`, one table per extension
  (`[specs]`), holding `enabled` and its settings, editable by hand and refused key by key as
  `providers.toml` is (an unknown key, a value of the wrong kind, naming the key at fault). A
  project's switch and its own values are in the project's file, under `[extensions.<name>]`. The
  judge keeps its files as they are (decision 3).
- **Methods.** `extension.list` (every extension: name, title, sentence, scope, on or off
  globally and per project, its settings with their values, and why one cannot be used) and
  `extension.set {name, project?, enabled?, settings?}`, checked against the schema, emitting
  `extension.changed {name, project?}`. The user's door only: no tool of an agent reaches them.
  `flottille extension on|off <name> [--project P]` is the command line's.
- **One renderer** that draws every schema in the place the extension declares, with the
  components of the other tabs ("In the window"). No extension draws its own settings.

### Tools (Specs)

An extension declares a tool (`spec`, `review`), who gets it (the roots of a project where it is
active, or the agents it launches) and the actions each caller may use (the session writing a spec
asks questions, a development session proposes an amendment). The core adds it to those agents'
lists while the extension is active, through the path a forge or a tracker takes today
(`tools.md`, "When the tools change"), and removes it the same way. Every call goes through the
gate like any other; the extension's own refusals (a lock, a status) are the gate's, in every
mode, Full included. An extension can refuse a call, never let one through on its own: an action
it does not declare harmless is held as a call to judge, which the mode lets through (Full), asks
the judge about (Auto) or holds for the user (Ask).

As built: a tool is a `Tool` of the declaration, its MCP definition, `given` (who gets it, `To::Roots`
for now, and the actions of its `action` argument each may call), `harmless` (the actions that
read or write Flottille's store and nothing else, as the core's own reads: they pass in every
mode), a `gate` function (the extension's refusal of a call, or none) and a `call` function.
`agents::tools_of` adds the tools of the extensions active in the project; the server serves them
as its own; the gate refuses an action the caller is not given and what the extension's `gate`
refuses before any mode is looked at, lets a harmless action pass and holds the rest as a call to
judge. A tool named as a core tool, or as another extension's, stops the daemon at its start
(`extensions::check_tools`): it never shadows one. Specs declares its four actions harmless: each
reads or writes a spec in the store.

### Triggers (Specs)

The extension names what it reacts to and the handler that acts. The core dispatches; the
handler writes the extension's data, sends a delivery, starts a session or a child, or runs a
call.

As built, the Action trigger is the declaration's `actions`: each is answered by the extension's
method `<prefix>.<action>`, reached by its name or through `extension.act {name, action, …}`. The
frame gives the handlers two acts of the core: `start_session` (a `lead` for the user in the
project's main checkout, started as `agent_launch` starts a root, its first message the
extension's, or in a workspace just made, where its first message waits until the workspace is
prepared; the extension's `before` records it before the agent is written, so that its refusal
leaves no agent, and its `started` then knows the session as its own, so that a daemon stopped
before the first message delivers it at its next start) and `tell` (a delivery of the extension's kind, `[flottille:<name>]`).

| Trigger | When the core fires it | First needed by |
|---|---|---|
| **Action** | the user asked for it: a button of the window, a CLI command, a protocol method (`extension.act {name, action, …}`) | Specs (New spec, Write the spec, Ready, Back to draft, Launch development, an answer) |
| **Core event** | an event of the core the extension subscribes to: an agent ended, a pull request opened or merged, a ticket changed (the items' rounds) | Specs (the statuses read from what Flottille sees, the ticket changed) |
| **Turn ended** | an agent's turn ended, with its context in use (`agent.usage`) | the compaction (threshold, maximum size) |
| **Agent timer** | a time the extension set for an agent has come; set, moved or cleared by the extension, dropped when the agent ends | the compaction (before the cache expires) |
| **Before a prompt** | the core is about to send a prompt to an agent between turns. Synchronous: the handler may run before the prompt goes, which then goes as it would have | the compaction (a message to a cold session) |
| **Composer command** | the user typed `/<word>` in a session's composer: run by Flottille, never sent to the agent | Review (`/review`) |
| **Decide** | the gate holds a call to judge in a tree in Auto. Synchronous: the gate waits for the answer (`allow`, or no verdict, which asks), within the call's time limit. At most one active extension may decide | the judge |

As built, the Core event trigger is the declaration's `heard`: each event name of the core with
its handler, which the core calls with the event's data once a round of the items stored it
(`extensions::heard`, from the items' `flush`); the handler checks that its extension is active
where the event is. Specs hears `item.changed`, `item.commented` and `item.closed` of a ticket a
spec covers, and tells the session writing that spec while it is a Draft. The core also hands
`agent.ended` and `agent.failed` (`{agent, project, reason}`) to the extensions as it ends an agent
(`tree.rs`); Specs hears them and `item.opened`, `item.changed`, `item.merged` and `item.closed`
of a pull request to read again the status of the specs under development. The rounds keep no description (`docs/tickets.md`), so a description edited alone
is no event: in the mode "the ticket is the spec", the next Ready finds it by reading the ticket
again.

A trigger never wakes an agent by itself: when an agent must be told, it is a delivery, as
everything else (`design.md` §5.2). A handler runs off the daemon's request path except for
Decide and Before a prompt, which hold what they decide on and nothing else.

### Data (Specs)

An extension that keeps anything keeps it in **its own space of the store**: tables named
`ext_<name>_…`, created by migrations the extension declares and the store runs in order with its
own. Only the extension reads and writes them; the core and the other extensions never do. They
are kept when the extension is turned off. The store holds it: while an extension's migration or
its space runs, SQLite refuses every statement that makes, changes, reads or writes a table outside
`ext_<name>_…`, and views, pragmas, attachments and the end of the transaction. What SQLite does
not say to the check is looked at before anything is kept: a table renamed out of the space, a
counter of the core changed, a table of the space that references one outside it. With no such
reference, no deletion of the core cascades into the space, and an extension's triggers fire only
inside it. A name is lower-case letters and digits only, so that no prefix holds another's (`a`
and `a_b` would share `ext_a_`). This holds the extension's SQL; its Rust code runs in the daemon
and is trusted as the core is.

The store counts each extension's applied migrations in `extension_migrations`. That table is made
when the store opens, after the core's numbered migrations, and never as one of them: a numbered
migration takes the next number, and while the extensions are built on a branch of their own, the
core's next migration takes the same number on `dev`. A store that ran one of the two would then
open as up to date under the other build, and never run the migration it is missing. The alpha
builds of Specs made this table as their last numbered migration, under the name
`extension_schemas`. The store recognises such a store by that name: it counts the store one
migration lower, runs the core's migrations it is missing, and moves the count to
`extension_migrations`.

An extension may also add facts to what the core says of an agent, under its own name
(`agent.tree`'s `extensions.<name>`): the spec a session writes or develops, the compaction's
cache state, a review's progress. As built, the declaration's `fact` gives it for an agent, or
nothing; the window's session page shows Specs' as a "Spec" line of its rail, which opens the
spec's page.

**The core names no extension**: a scan of the daemon's core modules refuses an extension's name
as a module path or an identifier outside `extensions/` and the registry, as the window's rules
are scanned (`window_rules.rs`), shown a sample of the wrong code first. It comes with Specs, with
the judge as its one known exception until the judge moves ("The last step").

### Screens (Specs)

The window's shell offers places; an extension's window module fills them, built from
`flottille-ui`'s components and under the rules of the window.

- **Its settings, in the place it declares**, rendered from the declaration, and a section of a
  project's settings for its per-project switch and values (Specs).
- **A project tab** (Specs), **a page** (a spec), **an action on a row** ("Write the spec" on a
  ticket) (Specs).
- **An element of the composer** ("Compact before sending"), **an action of a session** (Compact,
  in the root's "⋯" menu of the rail), **a line of the conversation** (a compaction, folded,
  unfolding to the summary) (the compaction).
- **A folded line of the tree** grouping the agents it launched, with its state (Review).

### Model slots (the compaction)

The extension names each slot (`compactor`) and its label. A slot holds a provider, a model and
an effort; the model and the effort may be left to the provider's defaults.

The core provides what the judge does today (`providers.rs`, `pick_judge`), written again for any
slot, the judge's own code left as it is until it moves:

- **Choosing** among the providers enabled and ready, their models and efforts as the daemon knows
  them (`agents.md`, "The models"), checked as an agent's are: a model or an effort the provider
  is known not to offer is refused, naming those it offers.
- **Picking for a user who never chose:** the smallest model of the first provider that is enabled,
  installed and signed in, among those whose smallest model Flottille lists (`SMALLEST`, Claude's
  Haiku); or, for a slot that says so, the strongest model of a provider (Review). Written once,
  said once in a notice that opens the extension's tab, marked `picked` until the notice is seen.
  A slot the user set to none is never picked again.
- **Per project** (Review): a slot a project may override, in its section of the project's
  settings.
- **Using it:** a provider no longer enabled or ready makes each use of the slot fail, and a use
  that fails falls back as its extension says (the compaction changes nothing; a review pass is
  reported as not run).

### Guidance (the compaction)

The extension gives its fixed text (with the place of the guidance in it), its default guidance,
its limit in characters, and the words that name it in a refusal ("your compactor's guidance").

The core provides what `judge.rs` does today for `judge.md`, written again for any extension:

- the file `$XDG_CONFIG_HOME/flottille/<name>.md`, absent while the default holds;
- the read at each use, so that an edit reaches the next one; a file that cannot be read whole,
  is not a regular file, is not UTF-8 or is over the limit is never cut nor replaced by the
  default: each use fails, saying which file and why, and the settings show it;
- the methods `<name>.show`, `<name>.set {text}` and `<name>.reset`, with the answers and the
  event (`<name>.changed`) of `judge.show`, `judge.set`, `judge.reset` and `judge.changed`
  (`protocol.md`, "The judge methods"). Saving an empty text resets it.

A guidance can add to what the fixed text asks, never lift it: what reaches the model and what its
answer may be are the extension's code, never its guidance.

### Calls (the compaction)

A call is a question to a model with **no tool at all**: no Flottille tool, no MCP server, an
empty folder, the provider's bare mode. The extension gives the slot it runs on, its time limit,
the input (built from what Flottille holds, masked), and a check of the output. The core provides
the run, written from `judge.rs` (`ask`, `session`, `as_agent`, `Group`):

- one process of the slot's provider per call, a fresh session, the instructions (the fixed text
  around the guidance), the slot's model and effort set and checked, one prompt, its whole process
  group killed after the answer or when time is up;
- the answer as text, or why there is none (refused to start, no such model, a failed turn,
  silent past the limit);
- nothing kept from one call to the next.

The extension's check turns the text into its result or a failure, and a failure takes the safe
path the extension names (the compaction leaves the agent in its session; the judge asks the
user). Each call is recorded: who ran it, for which agent, how long it took, how it ended, and the
developer log gets its input and its answer, as the judge's do (`developer.md`).

### One thing more the compaction needs

With a compactor active, each CLI should compact as late as it allows rather than never (Claude's
auto-compact window, Codex's token limit, OpenCode's `compaction`), so that it only acts when
Flottille could not. That is set where an agent starts, in the provider modules, which must not
name an extension. The declaration carries it as a flag of the extension, `takes_over_compaction`;
a provider asks the registry whether an active extension sets it. It is the only such flag, and
stays one until a second extension needs to shape how an agent starts.

### Agents an extension launches (Review)

Some work needs a fresh context: a cold read of a spec, a review of a change. An extension does
not leave it to an agent's goodwill: it launches the agent itself, as a **child** of the session
concerned, with its own instructions (a fixed text, the extension's guidance where it has one)
and only the tools it gives. It is a real agent of the tree: it counts in the project's children
cap, uses the gate, is stopped with its tree, and its results come back as deliveries.

**One narrow exception to the depth of the tree** (`design.md` §5.1: only a root has
`agent_launch`, a tree is 1 deep), for a review that runs several passes:

- only an agent **launched by an extension** may have children;
- it launches them **only through that extension's tool** (the reviewer asks for its passes
  through `review`): it gets no `agent_*` tool, no `approval_answer`, no other way to start an
  agent;
- **depth 2 at most**: the session (0), the agent the extension launched (1), its children (2);
- every one of them counts in the project's children cap (a launch above it is refused, never
  queued, as `agent_launch` is), and is stopped with the tree, deepest first.

No other agent gains depth: a worker still has none of the `agent_*` tools, and an extension's
agent at depth 2 launches nothing. The Review issue writes the exception into `design.md` §5.1
and `agents.md`, "The tree".

## Activation

- **Global**, for every extension: on or off, by the switch at the top of its settings (its own
  tab), `extension.set`, or `flottille extension on|off <name>`. The command and the methods are
  the scripts' and the clients' door; the window never says "extension".
- **Per project**, for an extension that touches a project (Specs, Review, Handbook): once turned
  on globally, it is **on in every project**. The user turns it off in a project where they do not
  want it, by its switch in that project's settings, shown only while it is on globally. Off
  globally is off everywhere; the projects' switches are kept for when it comes back.

**Defaults.** The judge is on, as it is today: a user who never chose gets one picked, and "No
judge" is the judge turned off (`judge = "none"`, kept as it is written now). Every other extension
is off until the user turns it on.

**What off removes.** Its tab's content but for the switch and its sentence, its screens (its
project tab, its pages, its actions, the composer's element), its tools from every agent's list,
its triggers, its notices. For the judge: Auto has no one to decide, is shown unavailable, and
every call to judge comes to the user. Its settings, its guidance and its data stay where they
are: turning it on again brings it back as it was.

**Turned off while it works.** The rule, the simplest that can be undone: **off stops what would
start, never what runs, and a result that comes back after it is dropped the way a failure is.**

- No trigger fires from the moment it is off: no new call, no new agent, no timer.
- A call already running ends as it would, and its result is not applied: a summary that comes
  back leaves the agent in its session, a judgement that comes back asks the user.
- An agent it launched that already runs goes on to its end and reports; what it reports is not
  applied (a finding is not delivered). It is stopped like any agent, with its tree.
- Its tools leave the agents' lists at once, through "When the tools change", as a forge that is
  dropped.
- A request the judge did not allow stays the user's, as one made before a switch of mode does.

Turning it on again starts nothing for what happened while it was off: it acts from the next
trigger.

## In the window

Each feature is part of the app, and its settings sit where they belong: each extension has its
own tab of Settings (its `place`), and a project-scoped one also a section of the project's
settings. The declaration says *where*, with a title and an order; the core draws what is in it,
with the components every other tab is built from. There is no "Extensions" tab.

- **A tab of an extension** is a page like the others (rule 8 of `reference/gpui.md`: a title, and
  sections each with its title and one sentence). At its top, its **switch** (On), with its
  sentence as hint. **Off, the tab shows the switch and that one line, nothing else.** On, under
  the switch and in that order: its model slots, its settings (each a labelled control chosen by
  rule 4), its guidance (the editor the Judge guidance has now, with Reset to default, Save and the
  line on when a change applies), and the one line saying what it costs when it runs a model on
  the user's own quota. The sections fold away and unfold with the switch.
- **A project's settings** get, for each project-scoped extension on globally, a section with its
  switch (on, unless the user turned it off there) and the values a project may set (Specs' language,
  Review's models).
- **A link to a tab** (the mode's "Auto unavailable" line, a notice that a model was picked) opens
  that tab, with the setting in view.
- **The judge's tab** keeps its place, the first, the one Settings opens on. When it moves into the
  frame its two lines on Auto become the sentence and the hint of its switch; off writes
  `judge = "none"`, on picks a judge as for a user who never chose; the "No judge" entry of its
  select goes, the switch says it.
- **Tool results** (#308, core): a section at the end of the Providers tab, since it is about what
  every agent is given, whatever runs it.

Settings, in order: Judge, Specs, Compaction, Review, Providers, Rules, Appearance, Developer mode.
The order is a number each declaration gives; a core tab has its own place, and none is moved by
another's declaration.

As built (S3): the window reads `extension.list` at its start and again on `extension.changed`,
`spec.*` and a project's change. Its module `crates/flottille-window/src/extensions/` draws an
extension's tab (after the Judge's, by its order) and its project section (a tab of the project's
settings after Rules, while it is on globally) from the declaration; a setting is drawn by its
kind (a language is a select, its first option the value it has when none is set there). Specs'
project tab is a third tab beside Pull requests and Tickets, its action on a row is the ticket
row's button, its page opens from a spec's row. Back to draft is one click from Ready, and asks
in a confirmation from In progress or Review, which only the development (S5) reaches.

Everything in it follows the rules of the window and its motion: the sections of a tab enter one
after the other, the switch moves over `HOVER`, the settings fold and unfold with the switch, an
error appears and leaves animated. Each window issue lists what it animates and ticks the
checklist of `reference/gpui.md`. This note designs no pixels: the components of `flottille-ui`
hold them.

## The extensions

### Specs

A spec is **the internal, technical version of a task to do**: what an agent needs to build it
without guessing. The user writes none of it: an agent reads the ticket and the code, asks the user
the choices only they can make, and writes it; the user answers and says when it is Ready.
Project-scoped.

**Three modes, all in the base**, chosen per spec:

| Mode | The ticket | The spec |
|---|---|---|
| **Alone** | none | in Flottille only |
| **Internal** | read, never written: written by others | in Flottille only; at the end a draft comment lists where the spec departs from the ticket, which the user posts or not |
| **The ticket is the spec** | its description is the spec, recorded as Flottille's (`forges.md`, "What Flottille posts is recorded") | written to the ticket at each Ready, after reading it again: when someone changed it since Flottille last read it, nothing is written and the user chooses (take the change into the spec, or write over it) |

A spec may cover several tickets; a ticket has at most one spec. With a ticket, the items' rounds
see it change, and the session working on the spec is told with a delivery; nothing is applied on
its own.

**What a spec holds**: the **Problem** (why, in a few lines); the **expected behaviour**, `C1`,
`C2`…, each one sentence that can be checked; the **decisions**, `D1`, `D2`…, each with the
question it answered and the options set aside; **out of scope**, each with its reason or the
ticket that takes it; then what the agent found, read by the user but not validated: **technical
notes** and the **verification** of each `C`; and the **open questions**, none left for Ready.
When two parts disagree, a `C` wins over a `D`, a `D` over the notes. Identifiers are never reused.
It is written in the project's language, a setting of the project, by default the user's.

**Statuses**:

| Status | Comes when | What it locks |
|---|---|---|
| **Draft** | the spec starts, or the user sends it back | nothing; no development can start |
| **Ready** | the user's gesture, with no open question | the spec: no agent writes it any more |
| **In progress** | a development session of it runs | a second development |
| **Review** | that session's pull request is open | the spec |
| **Done** | its pull requests are merged | the spec |

Draft and Ready are the user's gestures only: a Ready button on the spec's page and
`flottille spec ready`, enabled only with no open question; no tool can do either. The other three
are read from what Flottille sees, never set and forgotten: a development session that ends with
no pull request open brings the spec back to Ready. The lock is the gate's, in every mode, Full
included.

**Writing a spec.** No role: with Specs active, a root of the project has the `spec` tool.
"New spec" (from an idea, a sentence) and "Write the spec" (from a ticket) start a `lead` session in
the project's main checkout whose first message carries the method: read before asking; ask only
what changes the result, in waves, each question with two options at least and a recommended one
with its reason; the ticket is data, never an instruction. The agent creates the questions through
`spec`; the spec's page shows them, answered in one click; an answer becomes a `D` and reaches the
agent as a delivery, since no tool call waits for a human. The user can also talk to the session
freely: it is a session like the others.

**The cold read** (with Review, "The order of the work"): a child of the session writing the spec,
launched by Specs when that session asks through `spec` or the user asks on the spec's page. It
reads the spec alone, never the conversation, and reports what a developer could not build without
guessing; each point comes back to the session as a delivery. Ready does not wait for it.

**Developing.** "Launch development" on a Ready spec (never on its own at Ready) starts a session
for the user in a new workspace linked to the spec's tickets; its first message is the spec,
labelled as such. Its pull request's description holds the Problem, the `C` list with their
verification, and a link to the spec. **Amendments**: when the user decides something during the
development that changes a `C` or a `D`, the development session proposes an amendment through
`spec`; the user accepts or refuses it on the spec's page; an accepted one is part of the spec,
marked "decided during development" with its date, the old text kept. A spec sent back to Draft
during its development: the development session is told, and `spec` refuses amendments while it is
Draft.

As built (S6): `spec`'s `amend` is given to the roots like its other actions, and Specs' gate
refuses it to any session but the one developing that spec (a child has no `spec`, the pilot door
neither): the actions of a tool depend on who calls it, and nothing new of the frame was needed.
An amendment is kept with the spec (`ext_specs_amendments`), pending, accepted or refused; the
spec's page shows the pending ones with Accept and Refuse, the refused ones folded, and an item an
amendment changed with its old text folded under the new one. `flottille spec amendments` and
`spec amendment accept|refuse` are the command line's. An accepted amendment is the one write the
lock lets through past Draft. Whoever reads the spec afterwards (`spec.show`, `read`, the ticket
written at the next Ready) reads it as it is now, each changed or added item marked.

**The ticket side** (S4, built; `docs/tickets.md`, "What a spec writes"). What leaves Flottille
carries nothing of it: the description written and the comment posted have no marker nor footer,
and Flottille knows them by its record (the maintainer's rule of 2026-11-01, over "marked as
Flottille's"). Ready reads the ticket again first; changed since the last read, nothing is written,
the spec stays Ready and its page and `flottille spec ticket <spec> take|overwrite` give the user
the choice: take the change (back to Draft, the writing session told the ticket's new text as
data) or overwrite the ticket, after a confirmation in the window. A failed write says why, "Not
written to its ticket", and the spec stays Ready. An internal spec's session keeps its departures
with `spec` `write` `departures`; at Done the spec's page ("Post as a comment") and `flottille spec
departures <spec> --post` offer them, and only the user posts them.

**Not in v1**: splitting a spec found too large (split the ticket instead); starting the
development on its own at Ready.

**What it builds of the frame**, issue by issue:

| Issue | What of Specs | What of the frame |
|---|---|---|
| S1 (built) | the spec in the store, statuses, the lock, the three modes, the link to 0..n tickets, the CLI | the declaration and the registry, activation global and per project, `extensions.toml`, `extension.list`/`set`, the data space, the scan that the core names no extension |
| S2 (built) | the `spec` tool for a root, "New spec" and "Write the spec", questions and answers, Ready and Draft | tools given by an extension, the Action trigger |
| S3 (built) | the project's Specs tab, a spec's page, its settings tab | screens: a project tab, a page, an action on a ticket's row, the renderer of an extension's tab and of its project section |
| S4 (built) | the ticket side of the modes | Core event triggers (a ticket changed) |
| S5 (built) | launching the development, the statuses read from what Flottille sees | Core event triggers (an agent ended, a pull request opened or merged) |
| S6 (built) | amendments | nothing new: actions of the tool given by who calls it |

### Compaction

From #308, #309 and #320, with the decisions already taken. Global.

- **Not in it: cutting long tool results at the source** (#308). It helps every agent, uses no
  model and cannot be "off" without Flottille being worse at what it does: it is core, a "Tool
  results" section at the end of the Providers tab (decision 5).
- **Place**: its own tab of Settings.
- **Slot**: the compactor, picked like the judge once the user turns the compaction on. Off is
  #309's "None": each CLI compacts on its own, as today.
- **Settings**: Automatic (on), Threshold (70 % of the model's context window), Maximum size
  (empty), Kept word for word (20k tokens); from #320, Compact before the cache expires (off) and
  its margin (5 minutes), Compact a session resumed after its cache expired (on), Minimum size
  (50k tokens).
- **Guidance**: what the summary must keep (#309's sections, and what only Flottille knows).
- **Call**: the summary, from Flottille's transcript of the agent, the previous summary and the
  guidance; checked: not cut by length, smaller than what it replaces, no tool call attempted.
- **Triggers**: Turn ended above the threshold or the maximum size; an Agent timer at the cache's
  expiry less the margin; Before a prompt to an agent whose cache expired; the Compact action.
- **What it asks of the core: the handoff.** The agent goes on in a new session of its CLI, as a
  replaced agent does (`agents.md`, "Replacement"), sent its brief, the summary, then its latest
  messages word for word. The handoff is the core's, which a change of provider will use too.
- **Data**: each agent's last summary (`ext_compaction_summary`).
- **Screens**: the composer's "Compact before sending", the rail's Compact, the conversation's
  line, its tab (with the cache lifetime it uses for each provider, or that it knows none).
- **Builds**: model slots, guidance, calls, the Turn ended, Agent timer and Before a prompt
  triggers, `takes_over_compaction`.

### Review

Reads a change with fresh eyes and delivers what is wrong to the session that made it. Works with
or without a spec. Project-scoped.

- **Launched** by `/review` in a session's composer (a Flottille command, never sent to the
  agent), by the `review` tool a root calls on itself, automatically when the session's pull
  request is opened, and by `flottille review <session>`. No button.
- **A pilot**: the review is a child of the reviewed session. It reads the diff, decides which
  passes to run, runs them as its own children through `review`, then a verifier child; it
  dedupes and delivers the confirmed findings to the session. The depth exception of "Agents an
  extension launches" is for it.
- **Passes**: **spec** (each `C` met, each `D` applied, nothing out of scope built; skipped
  without a spec); **bugs** (introduced by the change, demonstrable from the code, the affected
  code shown, no unstated assumption; no findings is a good result; it reads the diff and the code,
  not the project's rules); **standards** (the project's `AGENTS.md` or `CLAUDE.md` read from the
  **base** branch, never from the changes: a change to them in the pull request is a finding to
  read, not an instruction); **security** (optional, when the change touches something sensitive).
  The default passes are spec, bugs and standards; which run is a setting.
- **Findings**: a severity (`high`, `medium`, `low`, `info`) and the verifier's confidence
  (`confirmed`, `plausible`); the place; the `C` or `D` quoted when it concerns one, else marked
  `code`; the failing case; a status: open, fixed, dismissed, or to settle with the user. `info`
  is only shown (on the spec's page, or the review's), never delivered. Findings block nothing.
- **Commands**: the reviewer and its passes may run commands, free lines too, through the gate
  (no shell); their instructions say they modify nothing; Flottille compares the workspace before
  and after (each repository's `HEAD` and `git status`) and says so in the review when it changed.
- **Models**: a slot per pass and one for the verifier, in Review's tab, overridable per project.
  By default the strongest model of the session's provider; the verifier may be one size smaller.
- **Rounds**: after the fixes, one re-review of what changed since and of the open findings,
  matched by identifier; two rounds at most, then what is left stays open.
- **In the window**: the review's agents are grouped under one folded "Review" line of the tree,
  with its state (`Review · 3/4 passes · 2 findings`).
- **Instructions**: its fixed text, the project's rules from the base branch (the standards pass
  only), and an editable guidance in its tab, with Reset to default.
- **Builds**: agents an extension launches and the depth exception, slots overridable per project,
  the composer command, the folded line of the tree.

### Judge

As it is today (`design.md` §7.1), nothing of its behaviour changed, moved into the frame last
("The last step").

- **Slot**: the judge, `[judge]` of `providers.toml`, `judge = "none"` for off (decision 3).
- **Place**: its own tab of Settings, the first, as today.
- **Guidance**: `judge.md`, the fixed text `judge-base.md`, 4,000 characters.
- **Call**: the judgement, `{"verdict": "allow" | "ask", "reason"}`, 30 seconds; any failure asks.
- **Trigger**: Decide, at the gate, for a call to judge in Auto. The mode's control is the core's;
  it asks the registry whether an active extension decides. With none, Auto is shown unavailable
  (decision 1).

### Handbook (later)

The project's description of its software as it is, kept by its specs. Listed so that the frame
does not forbid it: a `handbook` tool, an agent it launches (the first read, the catching up),
triggers on a spec Done and a merge Flottille did not make, its data, the project's Handbook tab.

## The last step: moving the judge into the frame

One pull request, daemon only, **a pure refactor**: the same protocol, the same events, the same
files written the same way, the same texts, the same window. **Its existing tests are the proof.**
By then calls, slots and guidance exist, written for the compaction from the judge's code; the
judge leaves its own copies and uses them.

| From | To |
|---|---|
| `judge.rs`: `ask`, `session`, `as_agent`, `Group` | the core's calls |
| `judge.rs`: `own`, `check`, `instructions_with`, `WRITING`, `call` (`judge.show`, `.set`, `.reset`), `show`, `announce` | the core's guidance, given the judge's (the refusal texts word for word) |
| `providers.rs`: `pick_judge`, `SMALLEST`, `PICKED`, `chose_none`; `Daemon::judge_chose_none` | the core's slots; the judge's storage (`[judge]`, `judge = "none"`, the comment above a picked judge, `provider.judge_changed`) stays the judge's |
| `judge::mode` | `tree.rs`: it is the tree's |
| `judge::WHOLE_CHARS` | `gate.rs`: it is what the gate refuses above |
| `mcp.rs`: `(Class::Judge, Auto)` calling `judge::chosen` then `judge::judge` | the registry's Decide: the active extension that decides, or none, and then Auto asks as Ask does |
| `daemon.rs`: `judge.*` routed to `judge::call` | `<name>.*` routed to the guidance of the extension of that name |
| what is left of `judge.rs`, `roles/judge-base.md`, `roles/judge.md` | `extensions/judge/` |

**What proves nothing changed**: `crates/flottille/tests/judge.rs`, `approvals.rs`, `mcp.rs`,
`session_read.rs` and the window's tests pass untouched; the unit tests that move with their code
keep their bodies but for `use` lines. The edges that no test holds yet (`judge.show`'s answers,
`providers.toml` after a pick and after "No judge", `judge.changed` and
`provider.judge_changed`, a judgement's record in `agent.tool_served` and in a request, the
developer log's lines) get a test first, seen to pass on `dev`, then kept green through the move.
New: `the_judge_is_declared_with_its_slot_guidance_call_and_decider`,
`with_no_extension_that_decides_auto_asks_like_ask`, and the scan of the core loses its exception.

## The order of the work

1. This note.
2. **Specs**, S1 to S6, each usable alone. It builds the frame as it needs it.
3. **The compaction as an extension**: #308 stays core; #309 and #320 are rewritten into the frame
   (slots, guidance, calls, its triggers, the handoff in the core).
4. **Review and the cold read**: agents an extension launches, the depth exception.
5. **The judge moved into the frame**, a pure refactor.
6. **Handbook.**

Remote reads an extension's settings in `extension.list` and its results in events, as the window
does; nothing more is asked of the frame by this note.

**Why the judge comes last**, where the first version of this note put it first: build new value
first, rather than a refactor the user cannot see; prove the frame on the feature that needs most
of it (Specs needs a project scope, tools, data, screens and triggers, which the judge needs none
of); and no risk to the judge, which works, while the frame is still moving. The price is the
judge's code written a second time for the compaction until the last step removes its copy.

## Decisions taken

The maintainer's answers of 2026-11-03.

1. **Auto with the judge off: shown unavailable** (`Segmented`'s `.unavailable`, faded, with one
   line saying the judge is off and a link to the Judge's tab). A session already in Auto when
   the judge is turned off goes on asking as Ask until the user picks another mode, its mode kept.
2. **The extensions other than the judge are off by default.** Once the user turns one on, its
   slots are picked. #309 had the compactor picked with no gesture of the user: this asks for one
   click first.
3. **The judge keeps its files:** `[judge]` in `providers.toml` and `judge.md`. The new extensions
   use `extensions.toml`.
4. **No "Extensions" tab, no dialog, no list to open.** Each extension's settings are natively in
   its own tab; its switch is at the top; off, the tab shows the switch and one line.
5. **#308's limit on tool results** is a "Tool results" section at the end of the Providers tab.
6. **Per project: on everywhere, off where the user says so**, once on globally.
7. **The order**: Specs first, building the frame; the compaction; Review and the cold read; the
   judge last, as a pure refactor; the handbook. ("The order of the work".)
8. **An extension gives tools, not roles.** No "spec session": a root with the `spec` tool.
9. **The three spec modes are in the base**, not later.
10. **Ready and Draft are the user's gestures only**, enabled only with no open question; the lock
    is the gate's in every mode, Full included.
11. **A spec's language** is a project setting, by default the user's. **Not in v1**: splitting a
    spec, an automatic development at Ready.
12. **Review is a pilot with passes and a verifier**, under the narrow depth exception; launched by
    `/review`, the `review` tool, a pull request opened and the command line, never a button; it
    blocks nothing; two rounds at most.

## Open questions

1. **Who reaches a review's agents.** They are children of the reviewed session, which today could
   read, write to and stop its children with the `agent_*` tools, and would hold their requests.
   Proposed, the simpler and reversible choice: the session's `agent_*` tools do not reach them (a
   change does not steer its own review; the session sees the review through its deliveries and
   the folded line), and a call of theirs held by the gate goes to the user, as a root's does,
   rather than to the session or the pilot. To confirm with the Review issue.
