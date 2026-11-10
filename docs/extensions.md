# Extensions

**Status: Specs (S1 to S6) and Review built, in 0.2.0; Connections and the rest are proposals.** S1 (#332) built the declaration and the registry,
activation global and per project, `extensions.toml`, `extension.list`/`set`, the data space and
the scan that the core names no extension, and Specs in the store with its statuses, lock, three
modes and tickets, from the command line. S2 (#333) built the tools an extension gives and the
Action trigger, and writing a spec: the `spec` tool, "New spec" and "Write the spec", questions and
answers, from the command line and the protocol. S3 (#334) built Specs in the window: the
renderer of an extension's tab of Settings and of its section of a project's settings (now a
page of Features, "In the window"), the
project's Specs tab, "Write the spec" on a ticket's row and a spec's page. S4 (#459) built the
ticket side. S5 (#336) built "Launch development" and the statuses read from what Flottille sees. S6 (#337)
built the amendments.
Review (#338) built the agents an extension launches and the depth exception, the composer
command, model slots and a subset of values as settings, overridable per project, an extension's
guidance, and the folded line of the tree. Connections (#560) built its daemon part: an app-wide
scope, the tables an extension's table declares, tools named at run time, its events in the
inbox and a block of the judge's prompt; its window is #672. The rest is a proposal. Written from a discussion with the maintainer
(November 2026), and settled with their answers of 2026-11-03 and 2026-11-07 ("Decisions taken"). No
question is left open but those in "Open questions".

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
| Place | Where an extension's settings appear in the window: a row of Settings' Features tab (with a title and an order among the rows) and its page, or a section of a screen that exists (the judge: Approvals). |
| Model slot | A setting that names a model: provider, model and effort, picked like the Auto judge. |
| Guidance | An editable text an extension's model follows, set inside a fixed text it cannot move, with Reset to default. |
| Call | One question to a model with no tool at all: input given by Flottille, output checked by the extension. |
| Trigger | What makes an extension act: an event of the core, a time, a gesture of the user. |
| Active | Turned on by the user, globally and, for an extension that touches a project, not turned off in that project. |
| Connection | An MCP server of the user's, declared once and run beside the daemon, which Flottille gives to every agent as one tool ("Connections"). |

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
  tools let it, and every call is seen by the gate. Code of someone else's **in the daemon** would
  run with the daemon's rights, see every conversation and every secret the daemon masks, and act
  where the gate does not look: it would end bare mode, whatever it promised. It would also make
  Flottille's interfaces a public contract to keep stable, bring a supply chain to trust, and put
  screens in the window that no rule of the window holds. Flottille is for one person; what they
  want added is added to Flottille.

  This is not a refusal of third-party tools. A tool of someone else's that runs **beside** the
  daemon, as a program of its own, is what the **Connections** extension is for ("Connections"):
  Flottille's own code, which the user points at an MCP server they installed. The server runs in
  its own process with the user's rights, trusted as a CLI the user installed is; the daemon never
  loads a line of it. The gate controls **what is asked of it**, never **what it does** with the
  rights it has: that is the limit of Connections, said where the gate's other limits are said
  (`design.md` §7.1, Full mode).
- **A role.** An extension gives **tools**, never a role: the session that writes a spec is a
  `lead` with the `spec` tool, not a "spec session". The agents an extension launches itself (a
  cold read, a reviewer) are workers with its instructions and its tools ("Agents an extension
  launches"). A new tool alone is not an extension either.
- **A way around the core.** An extension reaches agents, the store and the window only through
  what the core gives it below. It never spawns a process of its own (the servers of Connections
  are the user's declaration, spawned by the daemon as a run is, never by the extension's code),
  never writes the store outside its space, never draws outside its places.

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
    /// Where its settings appear: a row of Features, or a section of a screen that exists.
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

- **Its settings, in the place it declares**, rendered from the declaration, and its page in a
  project's Features for its per-project switch and values (Specs).
- **A project tab** (Specs), **a page** (a spec), **an action on a row** ("Write the spec" on a
  ticket) (Specs).
- **An element of the composer** (the **Compact** control, left of Send, drawn only while there is
  something to compact), **an action of a session** (Compact,
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
  said once in a notice that opens the extension's page, marked `picked` until the notice is seen.
  A slot the user set to none is never picked again.
- **Per project** (Review): a slot a project may override, on its page in the project's
  Features.
- **In the window**, every slot is the one model choice of `reference/gpui.md` (rule 15,
  `ModelChoice`): one select of `<provider> · <model>`, the effort beside it, the default first and
  said with what it gives today. A slot is never set to none from it: an extension is turned off by
  its switch.
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
agent at depth 2 launches nothing. The exception is written into `design.md` §5.1 and `agents.md`,
"The tree".

As built (#338): the declaration's `launched` says which of its agents an agent is (`pilot`,
`pass`), and a tool given `To::Launched(kind)` reaches those of that kind. The frame's `launch`
(`tree::launch_for_extension`) launches one as a child of a root or of an agent the same extension
launched, 2 deep at most, in its parent's place and its tree's mode, taking the children cap's slot
first (refused above it, never queued); the extension records the agent as its own before it is
written. Such an agent is a `worker` with a worker's tools less `ask`, never an `agent_*` tool nor
`approval_answer`, plus its extension's; its instructions are its own role text
(`roles/launched.md`: no `ask`, its held calls go to the user, how it ends is its extension's to
say) and the extension's text, without the files of its place, the project's own rules or its
skills. One its parent is not told of is refused the tree's `report`, which would reach no one: it
reports through its extension's tool. Its parent's `agent_*`
tools do not reach it when the parent is not itself the extension's, which is told nothing of it as
of a child (`tree::tell_parent`, `agent_status`, the sweep's `children-status`), and the gate
holds its calls for the user. An extension's tool call is answered by a future
(`Tool::call`), so that a call may run git without holding the daemon.

## Activation

- **Global**, for every extension: on or off, by the switch at the top of its page,
  `extension.set`, or `flottille extension on|off <name>`. The command and the methods are
  the scripts' and the clients' door; the window never says "extension".
- **Per project**, for an extension that touches a project (Specs, Review, Handbook): once turned
  on globally, it is **on in every project**. The user turns it off in a project where they do not
  want it, by its switch in that project's settings, shown only while it is on globally. Off
  globally is off everywhere; the projects' switches are kept for when it comes back.

**Defaults.** The judge is on, as it is today: a user who never chose gets one picked, and "No
judge" is the judge turned off (`judge = "none"`, kept as it is written now). Every other extension
is off until the user turns it on.

**What off removes.** Its page's content but for the switch, its sentence and its usage line, its screens (its
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

Each feature is part of the app, and its settings sit where they belong. An extension the user
turns on or off is **a row of Settings' Features tab, never a tab of its own** (the maintainer's
redesign of 2026-11-10: a tab per feature made too many); a project-scoped one is also a
row of the project's Features tab. The declaration says *where*, with a title, an icon, its line
and an order; the core draws what is in it, with the components every other page is built from.
There is no "Extensions" tab, and the word never shows.

- **Features** is a list (`Subpage`, `reference/gpui.md` rule 13): one row per extension, its
  icon, its title, its state as a tag said once (`On`, `Off`; Connections `2 on`), and one line
  saying what it is for, never its state:

  | Row | Its line |
  |---|---|
  | Specs | A spec of each task, written with you before an agent builds it. |
  | Review | Fresh eyes on a session's change before you merge it. |
  | Compaction | Long conversations summarised so agents keep going. |
  | Connections | Your MCP servers, given to every agent behind the gate. |
  | Handbook (later) | One row more. |

  A click opens its page in place of the list, under the path `Features / Review`, whose first
  word goes back, as Escape does. The declaration gives what the row and the page need beside its
  title and its sentence: its icon, its row's line, its switch's label and its usage line.
- **An extension's page** follows rule 14: at its top, its **switch**, labelled with what it does
  (Specs "Write specs before building", Review "Review changes", the compaction "Compact long
  conversations", Connections "Use connections"), never "On"; its sentence as hint; its **usage
  line**, where and how it acts, shown even while it is off. **Off, the page shows that block,
  nothing else.** On, under it and in that order: its model slots (`ModelChoice`, rule 15; several
  in a row are one card), its settings (each a labelled control chosen by rule 4, a setting that
  depends on another unfolding under it), its guidance (an editor with Reset to default and Save
  once it changed; its title says who reads it: "What reviewers are told", "What a summary
  keeps"), and the one line saying what it costs when it runs a model on the user's own quota.
  The sections fold away and unfold with the switch.
- **A project's page of an extension** (in the project's Features, for each project-scoped
  extension) has the switch with the global one's label (the path says the project:
  `acme-shop / Settings / Features / Review`; its hint, "On in every project unless you turn it
  off here."), the same usage line, and the values a project may set (Specs' language and mode,
  Review's checks and models), each offering **As in Settings** first, its second line the value
  it gives; a value set there carries a dot, "set here", and **Use Settings' value** under it. An
  extension off in Settings is a faded row there, its line "Off in Settings" a link. The global
  page says in one line which projects do otherwise (`notes-app turns it off`), only when one does.
- **A link to a page** (the mode's "Auto unavailable" line, a notice that a model was picked, the
  quick switcher, which has an action per page) opens it, `Settings › Features › Review`, with the
  setting in view.
- **The judge** is not a row of Features: it is found through Auto, and its place is Settings'
  **Approvals** tab, the first section, drawn as a page of rule 14 (its switch "Let a model decide
  in Auto", its usage line, its model, then "What the judge is told"; `design.md` §10). Off writes
  `judge = "none"`, on picks a judge as for a user who never chose; its select has no "No judge"
  entry, the switch says it. This needs nothing of its move into the frame.
- **Long tool results** (#308, core): a section of Settings' Agents tab, since it is about what
  every agent is given, whatever runs it; it moves to the savings of Usage when Usage comes (#547).

Features, in order: Specs, Review, Compaction, then Connections. The order is a number each
declaration gives; the core's tabs and sections have their own places, and none is moved by a
declaration.

As built (S3), before the redesign: the window reads `extension.list` at its start and again on
`extension.changed`, `spec.*` and a project's change. Its module
`crates/flottille-window/src/extensions/` draws an extension's tab (after the Judge's, by its
order) and its project section (a tab of the project's settings after Rules, while it is on
globally) from the declaration; the redesign moves both into the rows and pages above, drawn the
same way. A setting is drawn by its kind (a language is a select, its first option the value it
has when none is set there). Specs' project tab is a tab of the project page beside Pull requests
and Tickets, its action on a row is the ticket row's button, its page opens from a spec's row.
Back to draft is one click from Ready, and asks in a confirmation from In progress or Review,
which only the development (S5) reaches.

Everything in it follows the rules of the window and its motion: a page enters sliding from the
right while the list leaves to the left, and back the other way; its sections enter one after the
other; the switch moves over `HOVER`; the settings fold and unfold with the switch; a Save
unfolds once a value changed; an error appears and leaves animated. Each window issue lists what
it animates and ticks the checklist of `reference/gpui.md`. This note designs no pixels: the
components of `flottille-ui` hold them.

## The extensions

### Specs

A spec is **the internal, technical version of a task to do**: what an agent needs to build it
without guessing. The user writes none of it: an agent reads the ticket and the code, asks the user
the choices only they can make, and writes it; the user answers and says when it is Ready.
Project-scoped.

**Three modes, all in the base.** A spec from a ticket takes the **default mode** (#687), a
setting of Specs, global and overridable per project: Internal by default, or The ticket is the
spec. Another mode for one spec is a manual exception ("Write the spec as…"). A spec with no
ticket is Alone, which is no choice:

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
"New spec" (from an idea, a sentence) and "Write the spec" (from a ticket, at once, in the default
mode; "Write the spec as…" in the ticket row's "⋯" opens the choice of the mode, preselected on
the default; `flottille spec new` without `--mode` takes the default too) start a `lead` session
in the project's main checkout whose first message carries the method: read before asking; ask only
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
lock lets through past Draft, refused while the spec is a Draft and when its item changed since it
was proposed, which it would undo. Whoever reads the spec afterwards (`spec.show`, `read`, the
ticket written at the next Ready) reads it as it is now, each changed or added item marked.

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

**Its page** (Settings, Features, Specs; `design.md` §10): the switch "Write specs before
building" ("What an agent needs to build a task without guessing, written with your answers. It
runs no model on its own."); the usage line "**Write the spec** on a ticket starts it at once, in
the mode below. **New spec** on a project's Specs tab starts one with no ticket. For a single spec
in another mode: **Write the spec as…** in the ticket's ⋯ menu."; **From a ticket**, radios,
**Kept in Flottille** ("The ticket is read, never written. At the end, a draft comment lists where
the spec departs from it; you post it or not.", the default) and **Written to the ticket** ("Each
Ready writes the spec as the ticket's description, after reading it again."), the words
"Internal" and "mode" never shown; **Written in**, a select whose first option is `Your language
(French)`. A project's page offers `As in Settings` first for both.

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
  model and cannot be "off" without Flottille being worse at what it does: it is core, the "Long
  tool results" section of Settings' Agents tab (decisions 5 and 13).
- **Place**: a row of Settings' Features. Its page: the switch "Compact long conversations"
  ("When an agent's context fills up, its conversation is summarised and it goes on from the
  summary. Off, each program compacts on its own."), the usage line ("Automatic past the threshold
  below, or **Compact** in a session's ⋯ menu, or the **Compact** control in its composer, left of Send, once the cache expired."),
  **Summary written by**, one `ModelChoice` whose first option is `The session's own agent` and
  the others the models, the effort beside a model; "Compact automatically" ("At the end of a
  turn, past the threshold. Off, only when you ask.") and **When the context reaches** unfolding
  under it; the rest folded under **More settings**: **Or above** (the maximum size), **Recent
  messages kept as they are**, "Compact an idle agent before its cache expires" and **How long
  before** under it, "Compact when a session comes back after its cache expired", **Not below**
  (the minimum size), and the cache lifetime of each provider as labelled values; the cost line
  "Each summary is a turn of this model, on your own quota."; **What a summary keeps**, its
  guidance. The word "compactor" is never shown.
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
- **Screens**: the composer's Compact control (left of Send, drawn only when there is something to
  compact), the rail's Compact, the conversation's
  line, its page (with the cache lifetime it uses for each provider, or that it knows none).
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
- **Models**: a slot per pass and one for the verifier, on Review's page, overridable per project.
  By default the strongest model of the session's provider; the verifier may be one size smaller.
- **Rounds**: after the fixes, one re-review of what changed since and of the open findings,
  matched by identifier; two rounds at most, then what is left stays open.
- **In the window**: the review's agents are grouped under one folded "Review" line of the tree,
  with its state (`Review · 3/4 checks · 2 findings`).
- **Instructions**: its fixed text, the project's rules from the base branch (the standards pass
  only), an editable guidance shared by every agent of a review, and, for each pass, an optional
  guidance of its own, empty by default ("Each check's guidance").
- **Builds**: agents an extension launches and the depth exception, slots overridable per project,
  the composer command, the folded line of the tree.

As built (#338):

- **Doors.** `review.start {agent}` (and `extension.act`), `/review` alone in a session's composer
  (`agent.send` runs it and answers with no delivery: the word is never sent to the agent; with
  more text it is a message like any other), the `review` tool's `start` for a root, the Core
  event `item.opened` of a pull request whose head is the branch of a workspace a session runs in
  (refused at the cap, the session is told, `[flottille:review]`; refused for another reason,
  nothing is said), and `flottille review <session>`. A session in the project folder has no base
  to read a diff from: refused.
- **The diff** of each repository of the session's workspace starts at the merge base of its base
  (`git diff <from>`, uncommitted and untracked files included); a second round starts at the
  `HEAD` the first one recorded. Each repository's `HEAD` and `git status` are recorded at the
  start and compared at the report.
- **Slots.** `<pass>_model` and `verifier_model`, `null` by default: the session's provider on
  its strongest model (Claude's `opus`; the verifier `sonnet`), once its sessions said they offer
  it, else the provider's default. The pilot runs on the session's provider, as the strongest.
- **Findings** are kept per session and named `F1`, `F2`… across rounds; a `rejected` one is not
  kept. `review.finding` marks one. They show under the review's line of the session's rail and,
  for a spec's development, on the spec's page.
- **Guidance**: `review.md`, 4,000 characters, edited on Review's page.
- **Data**: `ext_review_reviews`, `ext_review_agents`, `ext_review_findings`. The core's scan
  looks for `extensions::review` alone, `review` being the forge's word too (`forge::review`).

**Its page** (Settings, Features, Review; `design.md` §10), in the window's words: none of
*pilot*, *pass* or *verifier* is shown, and the keys (`spec`, `bugs`, `standards`, `security`,
`<pass>_model`, `verifier_model`) stay as they are.

- The switch **Review changes** ("Agents read a session's change with fresh eyes and send it what
  is wrong. Nothing is blocked."), and its usage line, shown even while it is off: "A review starts
  when a session's pull request opens, when you type **/review** in its composer, or when its
  agent asks for one. From a terminal: `flottille review <session>`."
- **What it checks**: one card, a line per pass, each with its switch, its name, one line of what
  it looks for and its model on the right (a compact `ModelChoice`): **The spec** ("Each expected
  behaviour met, nothing out of scope. Only for a session that builds a spec."), **Bugs** ("Wrong
  results, lost data, crashes the change brings."), **Project rules**, the standards pass ("The
  project's AGENTS.md, read from the base branch."), **Security** ("Secrets, injections, access.
  Off by default."). The last line, **Checking the findings**, is the verifier, with no switch
  since it always runs ("Each finding confirmed before it reaches the session."), its first option
  `One size smaller` (`Sonnet for Claude`). The default is said once under the card: "Models: the
  strongest of the session's provider unless you choose one. Each check is a turn on your own
  quota.", then "After fixes, one more review of what changed; two at most."
- Under each check's line, a quiet fold, **Instructions for this check** (below), its folded line
  `none` or the start of its text.
- **What every reviewer is told**, the shared guidance ("Added to Flottille's own instructions,
  which say they change nothing. A check's own instructions come after it."): its editor, Reset to
  default and Save once it changed.
- One line names the projects that do otherwise (`acme-shop runs Security too`), when one does.

The tree's folded line says `Review · 3/4 checks · 2 findings`.

**Each check's guidance** (the maintainer's decision of 2026-11-10, not built). Beside the shared
guidance, each pass may have instructions of its own, **empty by default**:

- A guidance of the frame may have **parts**: the extension names them (Review: its four passes);
  each is the file `$XDG_CONFIG_HOME/flottille/<name>-<part>.md` (`review-bugs.md`), absent while
  it is empty, with no default text, the guidance's limit (4,000 characters) and its reading rules:
  read at each use, a file that cannot be read whole fails that use, saying which file and why
  (a pass that fails so is reported as not run).
- The methods are the guidance's own, given a part: `review.show {part?}`, `review.set {part?,
  text}`, `review.reset {part?}` and the event `review.changed {part?}`; without `part`, the shared
  guidance, as today. Saving an empty text removes the file.
- **What each agent reads, in order**: the pass's agent, Flottille's fixed text, then the shared
  guidance, then its own instructions, each in a block of its own; the pilot and the verifier, the
  fixed text and the shared guidance only. A part can add to what the fixed text asks, never lift
  it, as any guidance.
- **Global only**: a project does not override a check's instructions (its rules already reach
  the Project rules check). Asked for later, it would be a value of the project's page like the
  models.

### Connections

Lets the user give their agents a tool that exists as an MCP server, whichever it is (a memory,
a notes store, a browser, the desktop), without putting it in Flottille. **The daemon is the MCP
client; agents never talk to the server.** Nothing in it is shaped around one server: the examples
below are examples. It is **app-wide**: off by default, with the extension's switch and a switch
of each connection, and no per-project switch ("Activation"). It is built after Specs has built
the frame it uses.

**What a connection is.** The user declares it once, in their own configuration: a name, one
sentence on what it is for, and how to reach the server, either

- a **command**, spawned by the daemon without a shell (a program and its arguments, as a task's
  line is), in its own process group, in a folder of its own (no project's place is its place: one
  server serves every workspace of every project), with the daemon's environment as a run has it
  (the forge tokens emptied, `tools.md`, "Tasks and runs") and the variables the declaration adds; or
- a **local URL** (`http://127.0.0.1:…`, `localhost` or `[::1]`, Streamable HTTP, no redirect
  followed), for a server the user runs themselves; or
- an **`https://` URL** (Streamable HTTP, the system's certificates, no redirect followed), for a
  server elsewhere (decision 21). Its `headers` name vault entries, as `env` does.

A plain `http://` URL to any other address is refused: what crosses a network is encrypted.

```toml
# $XDG_CONFIG_HOME/flottille/extensions.toml
[connections]
enabled = true                       # the extension's switch

[[connections.server]]
name    = "almanac"                  # the tool is conn_almanac
about   = "The user's notes and decisions."
enabled = true                       # this connection's switch
command = ["almanac", "mcp"]
env     = { ALMANAC_TOKEN = { vault = "almanac-token" } }
only    = { }                        # limits on arguments, below
except  = ["client-site"]            # projects whose agents do not get it
```

**On, it is given to every agent of every project**, and off to none, **except the projects its
`except` names** (decision 22): an agent of such a project does not get the tool, and a call of it
is refused as one of a tool it does not have. The project's file holds nothing of it, and no
project has a switch for it: the list is the declaration's, the user's alone. Excepting a project
does not ask for a new review: the server is the same. A connection for one project only is an
open question ("Open questions").

**The user's door only**, as `extension.set` is: the window, `flottille connection …` and the
methods `connection.list`, `.add`, `.edit`, `.remove`, `.review` and `.accept`. No tool of an agent
declares or edits a connection: a command is run with the
user's rights, where no gate looks. The file is in Flottille's own folders, which no tool writes.
A connection's name is lower-case letters, digits and `_`, 24 at most; the tool is `conn_<name>`,
so that no core tool, now or later, can take its name, and the gate's card says plainly that the
tool is a third party's.

**Secrets come from the vault** (`vault.md`). `env` (and, for a URL, `headers`) names an entry
instead of holding a value: the daemon reads it when it starts the server and puts it where it goes
(a variable of the server's environment, a header of every request to the URL).
The entry must be one of every project (`vault.md`: no `project` on it), since the server is given
to all; an entry tied to one project is refused. The agent sees neither the variable nor its
value; the value is masked in everything the daemon stores and shows, a server's output and its
error text included.

**One tool, however many actions.** A connection is one Flottille tool, `conn_<name> {action,
arguments?, for?}`, as `tickets` and `forge` take an `action`; the server's tools are its actions.
Its cost in the agent's context is about one tool line:

- **A short description, pinned**, written by Flottille from a fixed text, the user's sentence and
  the names of the **enabled** actions (1,200 characters at most; beyond that, `and 12 more: help
  lists them`). Never a word the server wrote. Its schema is the same for every connection, with
  `action` a plain string, so it never grows with the server.
- **`action: "help"`** lists the enabled actions, a line each, from the pin; with `for: "<action>"`
  it gives that action's description and full argument schema. Read on demand, answered from the
  pin, never by a call to the server. The description is cut at 4,000 characters, with a line
  saying so; the review shows it whole.
- **`help` with `for: "@instructions"`** gives the server's own `instructions` (below). The name
  cannot be an action's, which is made of letters, digits, `_`, `-` and `.` only.
- **`arguments`** is the action's own object, checked against the pinned schema before anything
  is sent: an unknown action, a disabled one, or arguments that do not fit are refused, naming
  what is wrong, and the server sees nothing.
- **A result** is the server's text blocks, masked, escaped as every result is, framed as a third
  party's data, and cut at the user's limit with the rest read through `run_status` (`tools.md`,
  "Long results"). An image, a sound or a resource has no place in a text result: it is left out
  with one line saying what it was and how big. `isError` is an error.
- **Every call is recorded** as an `agent.tool_served` event of the agent (the action and its
  arguments in the arguments, cut and masked as all are), and a connection that starts, stops,
  fails or is cut off says so in `connection.changed` and `connection.cut_off` (`events.md`).

Calls to one connection run one at a time, in order: a browser or a desktop has one state. A call
has 120 seconds, then it is cancelled and answers `the call failed: …`. The server starts at the
first call, stays while a session of any project runs and for ten minutes after, and its whole
process group is stopped then, with the daemon, and when the connection is turned off. A server that
fails to start answers `the call failed: the connection could not start: <the first line of its
error output, masked>`, and is tried again at the next call, never in the background.

**The server's `instructions`** (the text of its MCP `initialize` result, which many servers use to
say how to use them) are a third party's text like the actions' descriptions, and are kept with
them in the pin. They are **never put in the agent's context by default**: the agent reads them
through `help` with `for: "@instructions"`, framed as a third party's, cut at the same 4,000
characters. The judge does not see them.

**The daemon offers a server nothing but the tools.** It declares no client capability: a server
asking for sampling (a model call outside the gate), elicitation (a question that would wait for a
human) or roots is answered with an error. The server's prompts and resources are not used.

**No classification to do.** A call of a connection is **to judge**: a card in Ask, the judge in
Auto (which sees the call as any, with the action's pinned description in a block of its own,
escaped, as the agent's words are), passed in Full and recorded as allowed by Full mode. The user
may change one action, never more than that:

| Setting | The call is | Use |
|---|---|---|
| **judged** (the default) | to judge | everything the user did not look at |
| **passes** | harmless: passes in every mode | an action that only reads, as the user decided |
| **asked** | base: always a card in Ask and Auto, with a strong warning, never the judge's; Full passes it | an action the judge cannot read |

The server's own annotations (`readOnlyHint`…) are shown in the review and never used: they are
the server's claim about itself. What stays refused in every mode stays refused: an action that
is off, arguments that do not fit, a limit, a cut-off connection, a call of a replaced agent.

**Limits on arguments.** `only = { app = ["gedit", "Files"] }` says that an argument named `app`
may take those values and no other, wherever an action's pinned schema has an argument of that
name: another value, or none where the action takes one, is refused, naming the limit. It is the
one thing the gate can hold for a server it does not know. The review lists, for each limit, the
actions it covers and those it does not, so that a gap is seen before it is used.

**Pinning.** Third-party text put in the agent's context is how a tool is poisoned, and a server
that changes after the user trusted it is a rug pull. So the user's trust is in **what they
read**:

- **At the first connection**, Flottille records the server's announced name and version (MCP
  `serverInfo`), its `instructions` and, for each action, its name, description, argument schema
  and annotations. The
  connection is **awaiting review**: no agent has its tool, no call reaches the server, until the
  user has read that list and those instructions in the window, turned off any action they do not
  want, set any other
  than **judged**, and accepted. That acceptance is the pin.
- **At every start of the server**, and whenever it says its tools changed
  (`notifications/tools/list_changed`), the daemon lists them again and compares with the pin.
  **Any change cuts the connection off**: another version, other instructions (given, removed or
  changed), an action added, removed or changed
  in its description, schema or annotations. An action name that is not made of letters, digits,
  `_`, `-` and `.` (48 at most) is never offered and is said in the review.
- **Cut off**, the tool leaves the lists of every session, as a tool that could only
  refuse is not listed (`tools.md`, "When the tools change", with the `tools` delivery: `- conn_almanac:
  gone`); a call already running ends as it began; the server is stopped once it is done; an
  **inbox item** says that the user is needed, naming what changed.
- **The review** is a **diff** in the window, between the pin and what the server announces now:
  the version, the instructions (the old text and the new, line by line), each action added
  (whole), removed, or changed (the old text and the new, line by
  line), the unchanged ones folded away. Texts are shown as plain text, never as Markdown. The user
  accepts, or leaves the connection off. **Accepting keeps the settings of the actions that did
  not change and puts every new one to judged** (turned on, as the user has just read it).
- **A declaration the user edits** (the command, its arguments, the URL, the variables, the limits)
  is a different server as far as trust goes: the connection is awaiting review again, the diff
  against the last pin.

The pin and what the user decided in a review (which actions are on, which pass or are always
asked) are kept, once per connection, together in the extension's own space of the store
(`ext_connections_…`), and not
in the file: an edit by hand could not mark a new action as passing without anyone having read it.

**Turned off.** The extension's or the connection's own switch: the tool leaves the
agents' lists at once ("Activation"), no call reaches the server, and it is stopped once its running
call ends. A call that was running when it was turned off answers `the call failed: the connection
was turned off while it ran; it may have been performed`.

**In the window.** **Connections** is a row of Settings' Features (decision 23); its page is the
extension's switch and its sentence and, under them, the list pattern (rule 7 of `reference/gpui.md`): one row per connection, its state
(awaiting review, ready, cut
off, off, failing), its switch, and **Review** where a review is due, which opens the diff in a
large dialog; adding or editing one is a form dialog with its command or URL, its sentence, its variables (picked among the vault entries every project sees) and its limits. A call held for
the user is a card as any, naming the connection, the action and its
arguments. Everything animates as the rest of the window does, and the issue lists it.

**What it does not protect.** The server has the user's rights, and what an agent reads from one
connection may go to another place ("Open questions", 3). Flottille sees the name, the
action and the arguments of a call, never what the server then does with the machine, and it does
not read the arguments for paths or addresses. A server that is wrong, compromised or just
powerful can act as the user, and no mode changes that; this is Full mode's limit
(`design.md` §7.1), and the reason the user installs only what they would run as a CLI.

**As built (#560, the daemon).** The frame gained `Scope::Global` (no switch nor setting of a
project: a project's `[extensions.connections]` is refused), `declared` (the extension's own
list of tables, `[[connections.server]]`, checked by it, written through the frame's `rewrite`),
`dynamic` (tools named at run time, given to every agent of a project but those an extension
launched, gated, performed and described to the judge by the extension), and `needs_you` (its
events the inbox lists, kind `connections`). The pin and the settings are in
`ext_connections_connections`. A `help` action of a server's is not offered, as `help` is
Flottille's. The server's environment is a run's (forge tokens emptied, no `XDG_RUNTIME_DIR`),
its folder `$XDG_STATE_HOME/flottille/connections/<name>`. A URL is reached with no proxy,
whatever the daemon's environment says. Accepting stops the server the review started: a change it
announces before the accept is compared with the pin at its next start. Its tools are listed one
listing at a time (notices that come during one make one more), within the start's 30 seconds,
100 pages and 1,000 tools: past them it is cut off, saying why. Every vault secret a connection
reads is masked from then on, even when the keyring was locked at the daemon's start. A server's
own words in a failure (an error result, a JSON-RPC error, its first error line) are framed as a
result is, and a cut-off says its name and version masked and cut at 100 characters.

**Examples.** The first two are invented; the others are public programs, the first cases of
0.3.

- **A personal memory server**, `almanac`, and **a notes server**, `jotter`: `search` and `read` are
  set to **passes**, `write` and `link` stay **judged**. A token is a vault entry, put in the
  server's environment. Given to every project's agents, since they are the user's own.
- **`agent-browser mcp`** (vercel-labs/agent-browser, a browser for agents): its `eval` runs
  arbitrary script in the page, which no gate can judge, so it is **off**; a limit on `url` names
  the addresses its `open` may reach (the exact values listed); the other actions stay **judged**.
- **`agent-desktop mcp`** (vercel-labs/agent-desktop, driving the desktop through AT-SPI on Linux,
  its tools named `desktop_click`, `desktop_snapshot`…): the most powerful kind, since it acts in
  every application the user has open. It is declared without `--headed` (that flag, a global one
  of the command, allows physical input), every action is set to **asked**, and a limit on `app`
  names the applications it may act on.

**Builds**: nothing of the frame that Specs (0.2) does not build (settings, tools given by an
extension, data, screens). It lands in 0.3 (milestone "Connections"). The MCP client is `rmcp`'s, which the tests already use (the daemon's
dependency gains the client features it needs, said in the pull request), the server process is the
daemon's own spawn, and the tests run a small fake MCP server
on stdio with scripted behaviour, never a real one.

### Judge

As it is today (`design.md` §7.1), nothing of its behaviour changed, moved into the frame last
("The last step").

- **Slot**: the judge, `[judge]` of `providers.toml`, `judge = "none"` for off (decision 3).
- **Place**: the first section of Settings' Approvals tab ("In the window").
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
5. **Connections** (0.3): it asks nothing of the frame that Specs did not build (settings, tools
   given by an extension, data, screens), and lands after Specs (0.2) has built it.
6. **The judge moved into the frame**, a pure refactor.
7. **Handbook.**

Remote reads an extension's settings in `extension.list` and its results in events, as the window
does; nothing more is asked of the frame by this note.

**Why the judge comes last**, where the first version of this note put it first: build new value
first, rather than a refactor the user cannot see; prove the frame on the feature that needs most
of it (Specs needs a project scope, tools, data, screens and triggers, which the judge needs none
of); and no risk to the judge, which works, while the frame is still moving. The price is the
judge's code written a second time for the compaction until the last step removes its copy.

## Decisions taken

### 2026-11-03

The maintainer's answers.

1. **Auto with the judge off: shown unavailable** (`Segmented`'s `.unavailable`, faded, with one
   line saying the judge is off and a link to the judge's settings, Approvals). A session already in Auto when
   the judge is turned off goes on asking as Ask until the user picks another mode, its mode kept.
2. **The extensions other than the judge are off by default.** Once the user turns one on, its
   slots are picked. #309 had the compactor picked with no gesture of the user: this asks for one
   click first.
3. **The judge keeps its files:** `[judge]` in `providers.toml` and `judge.md`. The new extensions
   use `extensions.toml`.
4. **No "Extensions" tab, no dialog, no list to open.** Each extension's settings are natively in
   its own tab; its switch is at the top; off, the tab shows the switch and one line. *Replaced on
   2026-11-10 by decision 23: no tab of its own, a row of Features.*
5. **#308's limit on tool results** is a "Tool results" section at the end of the Providers tab.
   *Since 2026-11-10: "Long tool results", a section of the Agents tab (decision 23).*
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

### 2026-11-07

The maintainer's answers on Connections.

13. **Third-party tools run beside the daemon, never in it.** "A plugin of a third party" stays
    refused; **Connections** is Flottille's own extension that lets the user declare an MCP server
    of theirs, trusted as a CLI they installed. The gate controls what is asked of it, not what it
    does.
14. **The daemon is the MCP client; agents never talk to a server.** A connection is **one**
    Flottille tool with an `action`, so that its cost is about one tool line: a short pinned
    description with the enabled actions' names, `help` for an action's schema, results capped,
    masked and recorded like any other.
15. **No manual classification.** Every call of a connection is judged (Auto) or asked (Ask) by
    default. **Passes** is an optional shortcut for an action that only reads; **asked** is for one
    the judge cannot read.
16. **Pinning.** The server's version, its actions and their descriptions are recorded when the user
    reviews them; any change cuts the connection off until the user reviews the diff; the
    settings of unchanged actions are kept and a new action is judged. Descriptions are third-party
    text put in the agent's context, which is why they are pinned.
17. **A server's secrets come from the vault** into its environment; the agent never sees them.
18. **App-wide, off by default.** A connection is declared once, in the user's configuration, and
    has its own switch; on, it is given to every agent of every project. There is no project-level
    declaration and no per-project switch for now ("a connection for one project only" is an open
    question). The declaration is the user's alone, never an agent's.
19. **Any MCP server.** Connections is generic: nothing in it is shaped around one server, and the
    examples are only examples. It lands in 0.3, after Specs (0.2) has built the frame.
20. **A server's `instructions` are pinned with its actions,** a change cuts the connection off, and
    they are given by `help`, never put in the agent's context by default.

### 2026-11-09

21. **A connection may reach an `https://` URL**, not only a command or a loopback URL, over
    Streamable HTTP. Its `headers` name vault entries, as `env` does: read by the daemon, masked,
    never shown to an agent. Only the user's own declaration holds one. A plain `http://` URL stays
    loopback only.
22. **A connection may leave projects out**: `except = ["<project>", …]`, by project name. An agent
    of an excepted project does not get the tool, and its calls are refused as unknown. A
    connection for one project only stays an open question.

### 2026-11-10

The maintainer's validation of the window's redesign (`design.md` §10).

23. **Settings in tabs by subject, an extension a row of Features.** Agents, Approvals, Features,
    Vault, General, and Usage the sixth and last; a project's settings in Repositories,
    Workspaces, Tasks, Instructions, Features. An extension the user turns on or off is a row of
    Features opening its page, never a tab; the judge is the Approvals tab's first section.
24. **One pattern per thing**: one model choice everywhere (`ModelChoice`); a feature's page
    starts with its switch labelled with what it does, its sentence and its usage line; a project
    overrides a value with `As in Settings` first. "Providers" keeps its name.
25. **Review's words**: Review changes, What it checks, The spec, Bugs, Project rules, Security,
    Checking the findings, What every reviewer is told; no pilot, pass or verifier on screen.
26. **A guidance per check of Review**, optional and empty by default, folded under each check
    ("Instructions for this check"), read after the shared guidance by that check's agent only.
27. **Specs' default mode for a spec from a ticket** (#687): Kept in Flottille (Internal) by
    default, or Written to the ticket; "Write the spec" starts at once, "Write the spec as…" is
    the manual exception.

## Open questions

1. **Who reaches a review's agents.** Built as proposed below (#338), to confirm with the
   maintainer. They are children of the reviewed session, which today could
   read, write to and stop its children with the `agent_*` tools, and would hold their requests.
   Proposed, the simpler and reversible choice: the session's `agent_*` tools do not reach them (a
   change does not steer its own review; the session sees the review through its deliveries and
   the folded line), and a call of theirs held by the gate goes to the user, as a root's does,
   rather than to the session or the pilot. To confirm with the Review issue.
2. **A browser of Flottille's own for Firefox.** Connections can carry an existing browser server,
   but the ones that exist drive Chromium. Flottille could have its own: a WebExtension in the
   user's own Firefox, which sends synthetic events and hands the tab back to the user for what
   only they do (a login, a second factor, a payment), or WebDriver BiDi driving a dedicated
   profile. Not decided, and not designed here: it would be an extension, not a connection, since
   Flottille's code would drive the browser.
3. **Data moving between tools.** What an agent reads from one connection (personal data from a
   memory server, given to every project) it may write elsewhere: a public repository, a ticket,
   another connection. The gate judges each call on its own, not where the data in it came from, so
   nothing today follows data from where it was read to where it is written. Not designed here.
4. **A connection for one project only.** App-wide is the first version (decision 18), with
   projects left out by `except` (decision 22). A server
   that should reach one project's agents, for instance a desktop-control server wanted in one
   project, would need a declaration or a switch per project, and a rule for the pin, the vault
   entries and the server's life when it is. Possible later, not decided.
