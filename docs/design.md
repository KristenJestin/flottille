# Flottille: design

Flottille runs coding agents for one person on their own machines. Each agent is a real process
that Flottille owns, works in a place Flottille prepared (a set of git worktrees), and uses only
the tools Flottille gives it. An agent can launch other agents: for the agent they are its
sub-agents, for Flottille they are ordinary agents tied to a parent, which Flottille keeps
informed and wakes when something happens to them.

This document is the reference for what Flottille is and how it is built. It stands alone: the
mechanisms it reuses from an earlier, larger project are written out in full in
[`reference/hemera.md`](reference/hemera.md), the real multi-repository case it must handle is in
[`reference/multi-repo-case.md`](reference/multi-repo-case.md), the way of handing work between
agents that it must build in is in [`reference/orchestration.md`](reference/orchestration.md),
what the MCP protocol and its main client do today, and the rules Flottille's two MCP doors follow
because of it, are in [`reference/mcp.md`](reference/mcp.md), how the daemon waits for and ends
its children is in [`reference/processes.md`](reference/processes.md), and what is known about the
window toolkit is in [`reference/gpui.md`](reference/gpui.md).

## 1. What Flottille is, and what it is not

**It is:**

- A **daemon** that owns projects, workspaces (worktrees), agent processes, their conversations
  and their tree, everything they asked for, and the services of the machine: the programs every
  project shares (a database, an API the apps call), each run once on demand (`projects.md`,
  "Services").
- A **command line** (`flottille …`) that drives all of it, with `--json` outputs and a `watch`
  event stream (one JSON object per line). Other programs (a desktop shell, a status bar, a
  script) integrate Flottille through these, never by reading its database.
- A **local MCP server** with two doors: the tools of the agents Flottille runs, and a *pilot*
  door through which an agent running outside Flottille (for example a coding agent in a
  terminal) drives it: list projects, start root agents and talk to them, read every agent's
  state and conversation.
- A **window** (GPUI) that is one more client of the daemon: projects, the places where their
  work happens, sessions with their tree of agents, a live conversation with the root agent, the
  approvals that reach the user.

**It is not (yet):** a planning tool. There is no specification workflow, no phases, no proof
system, no remote judge. Those may come later; the first milestones must give something to use and
try quickly. When in doubt, the smaller thing wins.

**It is generic.** Nothing is tied to one machine, one desktop, one forge or one agent. Links open
with the system's default handler, colours are Flottille's own unless the desktop provides a
theme to follow (§10), paths come from XDG.

**The user talks to root agents only.** A child belongs to its parent: the parent reads it,
writes to it and is written to by it. The user can read any agent of a tree, never write to a
child, and a child never asks the user anything: what it needs goes to its parent (§5.1, §7).

## 2. Vocabulary

| Word | Meaning |
|---|---|
| Project | A folder. Either a git repository itself, or a folder holding several independent repositories (and possibly being one itself). |
| Repository | One git repository of a project, with its path relative to the project folder, its base branch rule and its remote. |
| Workspace | The place one piece of work happens: one git worktree per repository it takes, on one branch name, in one folder, prepared by the project's setup. |
| Setup | The project's ordered preparation steps for a new workspace: copy, link, run, plus the workspace's variables. |
| Tasks | The project's named command lines (`api:dev`, `test`, `lint`…), runnable by the user and by agents. |
| Skill | A text of the project an agent reads with a tool when its task needs it: a path of the project folder, or a text Flottille keeps. |
| Command | A project's named instructions, sent to a session as `/name`. |
| Agent | One agent session: a provider (Claude Code first), a model, a role, a place, a conversation, a state. Survives process restarts; can be replaced. |
| Root | An agent the user started. The only kind of agent the user writes to and the only one that can need the user. |
| Lead | An agent that has children. Any agent can be one, within the limits. |
| Child | An agent launched by another agent through Flottille's tools. Tied to its parent for life and belongs to it: only its parent writes to it; the user can read it. |
| Place | Where an agent works: the project folder itself (`main`) or one of its workspaces. |
| Session | A root agent and every agent it launched, shown as one conversation; it works in one place. |
| Delivery | A message Flottille hands to an agent (a child's report, a child's question, an approval request or result, a user message to a root), stored before it is sent. |
| Report | What a child returns to its parent when it is done, through the `report` tool. |
| Approval request | A tool call the gate held. A root's request is the user's; a child's request is held by its parent, which denies it or forwards it up (§7). The agent is answered at once; the result comes later as a delivery. |

## 3. Architecture

```
            ┌───────────── window (GPUI) ─────────────┐
            │  projects · places · sessions · agents  │
            │  conversation · reply · approvals       │
            └───────────────────┬─────────────────────┘
                                │ socket protocol (JSON lines)
 shell / bar / scripts ── flottille CLI (--json, watch) ──┐
                                │                          │
 outside agent ── flottille mcp (stdio, pilot door) ───────┤
                                ▼                          │
 ┌──────────────────────────── daemon ──────────────────────────────┐
 │ store (SQLite) · projects · workspaces/git · gate · deliveries    │
 │ sessions tree · sweeps · command runs · event bus                 │
 │ MCP server (HTTP, 127.0.0.1, per-session bearer token) ◄──────┐   │
 │ ACP client ──► Claude bridge (ACP) ──► claude ────────────────┘   │
 └───────────────────────────────────────────────────────────────────┘
```

- **One daemon per user**, started on demand by the CLI or by a user service. It is the only
  process that spawns agents, runs git, or writes the store. Socket in `$XDG_RUNTIME_DIR`,
  state in `$XDG_STATE_HOME/flottille` (or `$XDG_DATA_HOME`), configuration in
  `$XDG_CONFIG_HOME/flottille`. The folder is checked to be owned by the user.
- **The socket protocol is the API.** JSON request/response plus a subscription for events.
  It is written so that the same protocol can later travel over the network (see section 12):
  every request is self-contained, identifiers are opaque strings, nothing depends on sharing a
  file system with the client except paths the user typed.
- **Store**: SQLite (bundled), one file. Agents' conversations (the ACP events), tool calls,
  approvals, deliveries, sessions, projects, workspaces, steps, command runs, and every pull
  request and ticket of the projects (`items.md`).
- **Events**: everything that changes emits an event on the bus; `flottille watch` prints them,
  the window renders them. The event names and fields are a documented contract (`docs/events.md`,
  written with the code). Every event has a sequence number and is stored: a client that drops
  its connection resumes from the last number it saw (`flottille watch --after <n>`), which is
  what a window reopening, a status bar restarting or, later, a phone on a flaky network need.
- **Building blocks**: Rust 2024, tokio, `rusqlite` (bundled), `serde`/`serde_json`, `clap`,
  `agent-client-protocol` (the official ACP crate) for the agent side, `rmcp` (the official MCP
  crate) for the MCP server, GPUI Fast with gpui-kit for the window (see `reference/gpui.md`).
  The user's own `git` binary, spawned without a shell, never a git library.

## 4. Agents

### 4.1 Running them over ACP

Flottille is an ACP client. Each agent session is one process speaking ACP on stdio:

- **Claude Code** (first): Flottille's own bridge, `flottille bridge claude`, written in Rust
  inside Flottille's binary. It speaks ACP to the daemon and drives the user's own `claude` in its
  stream-json mode, at the version of `claude` Flottille was checked against (`flottille doctor`
  warns on another). Nothing else is installed: the user's `claude`, signed in (or given
  `ANTHROPIC_API_KEY`), is the only prerequisite. How the bridge maps ACP to `claude` is
  in `docs/agents.md`.
- **OpenCode**: the user's own `opencode acp`, its configuration moved into Flottille's data
  folder and its bare agent given in its environment (`docs/agents.md`, "Bare mode for
  OpenCode").
- **Codex**: Flottille's own bridge, `flottille bridge codex`, in front of the user's
  `codex app-server`, which it starts with Codex's configuration moved into Flottille's data
  folder, every tool of its own switched off and no environment (`docs/agents.md`, "Bare mode for
  Codex"). Gemini CLI speaks ACP natively too.

Each is a provider, an entry of its own in the daemon; which are enabled is the user's
(`providers.toml`), and each agent runs on one for life (`docs/agents.md`, "Providers"). The
daemon reads the models of each enabled provider whose program is there, in the background and
without a prompt (Codex's `model/list` with no thread; Claude's and OpenCode's agent started as
for a session, with no user message): at the daemon's start, when the provider is enabled, and
when its program (path or modification time), the names of its API keys set or the presence of
its sign-in file change. The first session can be given a model (`docs/agents.md`, "The
models").

Handshake, session open order (resume, else load, else new), model/effort/mode options, event
mapping, cancel, idle release of processes and death handling follow `reference/hemera.md` §1.

Flottille runs the agent CLIs the user installed and signed in to; it provides no login of its
own and never reads or copies credentials.

The one credential Flottille holds is what the user puts in its **vault** (`vault.md`): secrets
kept by the operating system's keyring, never by a file of Flottille's nor its store, set by the
user only (the window, or `flottille vault set` reading the standard input, never an argument),
and **never returned** by any method, client or tool, nor written to an event or a log. Agents see
the entries' names and what they are for (`vault list`), never a secret: a tool of Flottille's that
needs one takes the entry's name and the daemon puts the secret where it goes itself. An entry tied
to a project is seen by that project's agents only. Every value of every entry is masked in
whatever the daemon stores and shows.

### 4.2 Bare mode: the agent has only Flottille's tools

Every agent runs in bare mode (`reference/hemera.md` §2): no built-in tools, no user or project
settings, no skills, no plugins, no user MCP servers, only Flottille's MCP server. For Claude,
set by the bridge on `claude` itself: `tools: []`, `settingSources: []`, `strictMcpConfig: true`, `allowedTools` = the exact names of
the session's Flottille tools, a custom system prompt, and the two MCP timeouts raised. For
OpenCode, set in its environment: every tool denied but Flottille's, its own agents disabled,
its prompt the agent's instructions, and no configuration of the user's or the project's. For
Codex, set by the bridge on `codex app-server`: its own folder, a list of switches that turn its
tools off, no environment in any thread, the agent's instructions in place of Codex's, and
Flottille's server alone; Codex has no allowlist, so the bridge also fails a turn in which a tool
of Codex's own starts.

Nor does it reach the web but through Flottille: no fetch or search tool of its provider's, only
`web`, which the daemon performs under the gate, never to this machine nor a private network,
and which judges a URL the agent built (`tools.md`, "The web").

This is what makes the rest hold: with no shell and no native sub-agent tool, an agent **cannot**
start another agent (no `Task`, no `claude --bg`, no background shell) except through
`agent_launch`, and it cannot do anything the gate did not see. Nor can it drive the daemon from
a command it runs: the daemon refuses a client that descends from it (`docs/protocol.md`,
"Transport"). The agent's own permission
prompts answer themselves: Flottille's tools are allowed, anything else is rejected.

### 4.3 The tools an agent gets (first milestones)

Generic tools, as specified in `reference/hemera.md` §3 (arguments, limits, exact refusal texts),
with the result sizes, descriptions and annotations of `reference/mcp.md` §4:

- files: `fs_read`, `fs_list`, `search`, `fs_write`, `fs_edit` (read-before-write fingerprints,
  temp file + rename, all-or-nothing edits);
- tasks and runs: `tasks` (the project's tasks, written by a root only), `run` (a task or one
  line, **never a shell**), `run_status` (a run's output, or its stop; the list of the agent's runs with no run);
- the services of the machine (every agent): `services`, which lists them with their state and
  starts one, judged as a run of its line in its folder; its stop is always held for the user,
  in every mode, and a root's writes are judged as a task's (`tools.md`, "Services");
- the user's vault (every agent of a project that sees an entry): `vault`, which lists the
  entries' names and what they are for, never a secret (`vault.md`);
- forges (every agent of a project whose repositories declare a forge, in `main` as in a
  workspace): `forge`, which lists and reads the pull requests of the place's repositories and
  their checks (`forges.md`), and opens, comments, edits or merges them or re-runs a failed
  pipeline, each write a call to judge (§7); in `main` it names the workspace;
- the workspace (every agent): `workspace`, which lists and reads the workspaces, and, for a root,
  creates and prepares them, and removes one, always held for the user, its card saying what
  would be lost: the one it works in too, once its work is done, which stops its commands there
  and moves the session to `main`;
- skills: `skill_read`, one of the project's skills, texts read when relevant (`skills.md`);
- the web (every agent): `web`, which reads a page (its main content as Markdown, a PDF's text)
  or searches through DuckDuckGo, never this machine nor a private network; a URL the agent was
  shown passes, one it built is judged, since a GET sends out what the URL holds (`tools.md`,
  "The web");
- tickets (every agent of a project that declares a tracker, in `main` as in a workspace):
  `tickets`, which lists, searches, reads and writes the tracker's tickets, GitHub issues or Jira
  work items, each write a call to judge (`tickets.md`);
- the project (a root): `skills` and `commands` (list, show, add, edit, remove), `setup` (its
  steps, numbered) and `project` (its settings, its forges and tracker, its rules), and the writes
  of `tasks`, what the window does, each change a call to judge, a change of the tracker or of the
  rules base, a line it sets judged as a run of it (`tools.md`, "The project", `projects.md`);
- the user's connections (every agent of every project but those a connection excepts, while
  Connections is on and the connection reviewed): `conn_<name> {action, arguments?, for?}`, one
  tool per MCP server the user declared, the daemon being its client; its description written by
  Flottille, `help` answered from what the user accepted, each call judged unless the user marked
  its action (`tools.md`, "Connections", `extensions.md`, "Connections");

Flottille's own tools:

- `agent_launch {kind, title, brief, model?, role?, workspace? | new_workspace? + repositories?}`:
  `kind` is required and never guessed. `child` starts a child in the caller's own place, never
  another (a workspace named is refused, naming `led`), refused (never queued) above the caps.
  `led` and `handed`, for a root only, start a session in a workspace (existing or new) or its own
  place, judged, in the tree's mode, its first message the agent's and never the user's, base when
  that root was itself started by an agent (`tools.md`, "Sessions an agent starts"): a `led`
  session is piloted by the caller, asks it its questions and wakes it when it asks, reports or
  ends; a `handed` one is the user's, and the caller hears nothing more of it. It answers at once
  with the id.
- `session_send {session, text}` (a root): a message to a session the caller started, to the
  agent that started it, or to any other session of its project once the user gave their leave
  for that pair (asked with the first message, in every mode, §7.2), judged as a write is; it
  arrives as a delivery labelled with its sender, never as the user's, and the answer comes back
  the same way.
- `session_read {session?, last?, since?, until?, cursor?}` (a root): a page of the conversation
  of another session of the caller's project (the user's messages to it and its agent's replies,
  with their times), the last messages or those between two dates, with a cursor to the next page,
  and the mode of that session's tree; framed as that session's data, never instructions; judged
  in Ask and Auto; a session of another project is refused. Without `session` (or with the
  caller's own id) it reads this session: its id, its role and its tree's mode, asked of nobody.
- `session_title {title}` (a root): names its session for the user, 3 to 8 words on one line, at
  most 60 characters, in its first turn and again when the work changes, until the user renames
  it (`docs/tools.md`, "The session's title").
- `agent_send {agent_id, message}`: a message to one of the caller's children.
- `agent_status {agent_id?, action?, detail?, after?}`: for one child or all of them: state, what
  it is doing now (thinking, in a tool call, which command runs and for how long, waiting for
  approval #n, waiting on the provider), time since its last sign of activity, context use; the
  last few messages for one child, or for all with `detail: full`. `action: conversation`: a
  page of the child's conversation.
- `agent_stop {agent_id}`: stops the child and its own tree; also ends a session the caller
  started as `led`, judged as any call of its tree, and asks the user to end any other session
  of the project (§7.2).
- `report {text}`: the caller is done; its report goes to its parent at once and the session ends
  (see 5.3). A root agent's report goes to the user.
- `ask {question}` or `ask {questions}`: one question or up to 4, each with 2 to 4 options when
  the answers are known, answered later as a delivery (never blocks), to whom the daemon
  decides for the caller: a child's to its parent, delivered as `child-question` and answered with
  `agent_send`; a led session's to its pilot, which is woken at the turn's end and answers with
  `session_send` (the session shows as waiting on its pilot); any other root's to the user. A
  question to the user is open at once, whatever the agent does next: in the inbox, as "needs you"
  on its session's row, as a card at its place in the conversation named after the session, with
  a block per question (its options as rows, "Something else…" last, whose field of several lines
  unfolds when picked) and one Answer (the call's
  own line says `asked you: <its first line>`), and in what `session_read` gives, until the agent
  takes the answer from that card (`agent.answer`: one message quoting each question with its
  answer, which answers that ask alone, the card then folded to the answers) or a message of the
  user written after it (which answers every question open then), or its session is replaced or
  resumed. A parent or a pilot reads a several-question ask numbered, and answers it in words.
  `ask_user` and `ask_parent`, the names it replaced, are answered for one release, routed as
  `ask` is.
- `approval_answer {request_id, decision: forward | deny, why}`: answer a request held for the
  caller by one of its children (§7). `forward` passes it to the caller's parent, or to the user
  when the caller is a root. There is no way to allow a request.

The tool list of a session is its role's, and changes only when the role's does (prompt cache,
`reference/hemera.md` §7): a project that declares or drops a forge or a tracker, a new version.
A running session is then told (`notifications/tools/list_changed`) and served the new list, or
said to run an older one until a restart (`docs/tools.md`, "When the tools change"). It depends
on what the agent can do: every agent gets `ask`, a root `session_send`, `session_read`,
`session_title`, `skills`, `commands`, `setup` and `project`; a child, whatever its role, gets no `agent_*` tools
and no `approval_answer`. A root lead has 27 tools, a child 14; one more each when the
project's repositories declare a forge (`forge`), one more when it declares a tracker
(`tickets`), whatever the place (`reference/mcp.md` §4).

### 4.4 Roles

Two roles at first, with the tool lists of 4.3:

- **lead**: the role of an agent the user starts. Its instructions carry the orchestration
  standard (`reference/orchestration.md`): when to launch a child, which model, one precise task
  per child, the brief, how to read a report, the caps.
- **worker**: the role of a child. Its instructions say: do the brief and only it, keep going
  without waiting for answers, end with `report`.

A worker launches no children of its own (no depth, 5.1). Roles are data (a template and a few
flags), so more can be added without code changes.

## 5. Parenting and waking (the core of Flottille)

The failure Flottille exists to fix: a lead launches workers, loses track of them, and work sits
still for hours because nobody knows what is happening. In Flottille a parent cannot lose its
children: they are tied to it, Flottille tells it everything that matters, and wakes it when
something happens or when nothing does.

### 5.1 The tree

- Every agent launched through `agent_launch` has a parent (its lineage, stable across
  replacements) and a depth. Root agents are started by the user (window, CLI, pilot door).
- **Caps**: children running at once per project (default 12, configurable 1 to 12), and no
  depth: only an agent the user started (a root) has the `agent_launch` tool, a child never has
  it, whatever its role. Above the cap, `agent_launch` is refused with a sentence the agent can
  act on; it never queues silently.
- **The one exception to the depth: an agent an extension launches** (`extensions.md`, "Agents
  an extension launches"; Review's pilot). Only an agent launched by an extension may have
  children; it launches them only through that extension's tool (the pilot's `review`
  `run_pass`), never `agent_launch`: it has no `agent_*` tool, no `approval_answer`, no `ask`.
  The tree is 2 deep at most there: the session (0), the extension's agent (1), its children
  (2), which launch nothing. Each counts in the project's children cap, refused above it and
  never queued, and is stopped with the tree, deepest first. A worker still has none of the
  `agent_*` tools. The session's `agent_*` tools do not reach the extension's agents, it is told
  nothing of them as of its children (the extension tells it what they found), and a call of
  theirs the gate holds goes to the user, as a root's does.
- **Stopping a parent stops its tree**, deepest first. Closing or archiving an agent closes its
  children. Deleting a workspace that an agent uses is refused while it runs.
- **A child belongs to its parent.** The parent reads it (`agent_status`), writes to
  it (`agent_send`) and is written to by it (its report, its questions, its approval requests).
  The user can open and read **any** agent in the tree, but writes to the root only: a user
  message to a child is refused, from every client, and the user stops a child only by stopping
  its root. A user message to a root is a delivery like any other.
- A child never needs the user. When it waits on its parent (a question, a request held), it shows
  as waiting on its parent; only a root shows as needing the user.

### 5.2 Deliveries

Exactly the mechanism of `reference/hemera.md` §5: stored before sent (idempotent id), targeted
at a lineage, `between-turns` / `urgent` / `redirect`, pumped as one prompt when the agent is
between turns, pinned as a note on the next tool result when it is mid-turn (with the 30 s
fallback that cancels the turn and sends it as a prompt), given back when a turn fails, escaped so
data can never forge a marker. No timers wake an agent: waking is always a delivery.

The same path wakes an agent about its own command: a command that outlived its call (it keeps
running, or ran past the call's wait) delivers `command-ended` to the agent that started it when it
ends: the line, the exit code, the duration and the last lines of the output, masked
(`docs/tools.md`, "Commands").

An agent that pilots a session (`agent_launch` with `kind: led`) is woken, between turns, by
`session-turn-ended` when a turn of that session asked it a question (`ask`) or reported, and when
the session ends or fails; a plain turn's end wakes nobody. The delivery names the session, says
why and holds its last message, masked and cut, after the questions to the pilot that turn asked
and that still wait, whole. It is a following, a subscription that has no end of its own (§5.6,
`docs/tools.md`, "Following a session"). Two sessions that write to each other fifty times
without the user pause it. A pilot that ends leaves its led sessions going on, handed to the
user.

A led session stays open after its report: once its work is integrated, its pilot stops it and
removes its workspace. When a led session reported, is idle and its workspace's branch is
contained in another branch of the repository, its pilot is reminded once, by `led-integrated`
(`"Login" (agent …) reported and its branch is in main: stop it and remove its workspace?`),
checked at the end of a turn of either.

### 5.3 What the parent is told

| Event of a child | Delivery to the parent |
|---|---|
| calls `report` | `child-report`: the report text, the child's workspace and branch, its last commit per repository. Urgent. |
| ends a turn without reporting | Flottille first sends the child one reminder ("end with `report`…"); a second turn without a report tells the parent `child-idle`, with the child's last message. |
| fails (agent error, provider limit, replacement limit) | `child-failed`, with the reason and the last output lines. Urgent. |
| is stuck (no sign of activity for 5 min in a turn, not waiting on the provider, no command of its own running) | `child-stuck`; Flottille replaces it once, and says so. |
| has a call held by the gate (an "ask") | `child-asks`, with the request number, the call and the reasons. Urgent. The parent answers with `approval_answer` (forward or deny, §7). |
| calls `ask` | `child-question`, with the question. The parent answers with `agent_send`. |
| sits idle with nothing queued while another launch waits for a slot | ended, and `child-ended` tells the parent. |

### 5.4 When nothing happens

A lead that is between turns, with children running and no delivery for a while (default 20 min,
configurable), receives a `children-status` delivery: one line per child with what `agent_status`
gives (for example "running `cargo test` for 12 min", "waiting for approval #3", "thinking, last
tool call 40 s ago"). The lead decides whether that is normal (a long command) or not (a child
going in circles), and acts with `agent_send`, `agent_stop` or a new launch. The interval doubles
while nothing changes, up to a ceiling, so a quiet tree costs few wakes.

### 5.5 Replacement and restarts

Replacement on death, stuck, or a full context, the epoch that makes a replaced agent's late
calls fail, the replacement limit (2 in 30 min; then a child fails and its parent gets
`child-failed`, and a root asks the user), and the rebuild at daemon start follow
`reference/hemera.md` §5.

### 5.6 Subscriptions: waking on what happens outside

An agent that waits for something outside Flottille (the end of a pull request's checks, a
review) subscribes to it instead of reading again and again. Four parts, each its own:

- **A source** turns what happens to a target into **events**; a state already there when it
  subscribed is no event. The sources are the daemon's rounds of the items ([`items.md`](items.md),
  "What reads this stream"), which compare each read with what the store holds and say it as
  `item.*` events: a subscription to a pull request filters those of its pull request, as
  `checks` (all ended), `review`, `approved`, `comment`, `merged` and `closed` (`docs/forges.md`,
  "Subscriptions"), one to a ticket those of its ticket, as `commented`, `status`, `assigned` and
  `closed` (`docs/tickets.md`). No subscription reads anything of its own; it hears as often as
  the rounds come. The **timer** of a wake is a source too: its target is the agent's reason, its
  one event the time it asked for (`wake`, `docs/tools.md`, "Waking later").
- **A subscription** is (agent, source, target, the events chosen, whose events count, a
  deadline when the agent gave one), stored, so that it survives a restart of the daemon; nothing that happened meanwhile
  is lost, since the next round compares with what the store kept
  (`crates/flottille/src/subscriptions.rs`). The agent subscribes itself (`forge` `subscribe`,
  `tickets` `subscribe`, `wake`, `docs/tools.md`); nothing subscribes it.
- **Until the target ends.** Each read that holds events the subscription waits for is
  delivered, every such event of that read in one delivery, and the subscription goes on: the
  agent subscribes once, and never renews it nor adds a timed wake "as a safety net". Since it
  reads nothing of its own, keeping it costs nothing. It ends when the target ends (a pull request
  merged or closed, a ticket closed, a Jira work item in a status of the category done, said),
  when the agent ends, when it unsubscribes or the user cancels it, or at the deadline the agent
  gave (`minutes`, optional, no cap: woken once, `Nothing came in N min` when nothing did). A
  subscription stored with a deadline keeps it. Nothing is delivered of what does not change: each
  delivery costs the agent a turn, so the deliveries of one round of the items ring the agent once,
  and those that come while it is in a turn are pumped together, in order, at its end (§5.2).
- **A following** is a subscription whose source is a session the agent leads (`agent_launch`
  with `kind: led`): it has no deadline, and ends when the agent unsubscribes (`forge unsubscribe`
  with `session`), when the session ends or fails, or when the agent ends. It wakes the agent at
  the end of a turn of the session that asked it or reported, never more than once for a turn. A run the agent started (`forge` `run`) is followed the same way
  unless it says `follow: false`: the rounds of the items read it, and it wakes the agent once
  when it waits on a manual step and when it ends, which ends the following (`docs/forges.md`,
  "Following a run").
- **Delivering** is the existing path (§5.2): a `subscription` delivery, masked, its forge texts
  framed as data and not instructions, wakes the agent as a child's report does.

Left for later: relaunching an agent that has ended when an event comes; a rhythm faster than
the rounds' for a subscription that needs it.

The window shows them: the session page's rail lists the session's open subscriptions (the pull
request or the ticket, the events, since when and until when: `until merged or closed`,
`until closed` for a ticket, or the deadline the agent gave; a wake, `Wake: <why>` and when it is due), each with a Cancel that ends it through the daemon
(`subscription.cancel`); the following of a session an agent started, `Following <title>`, since
when, and, paused because the two sessions wrote to each other fifty times without the user, a line
that says so with a Resume (`subscription.resume`), unfolding when it pauses and folding away when
it resumes; an agent between turns that waits on one is drawn waiting, not idle, in the
left tree and in the rail (`Waiting: wake at 10:42` for a wake), and the delivery that wakes the
agent is a folded step saying what woke it (`woken by the timer: <why>`).
Opening and ending are events (`subscription.opened`, `subscription.ended`, `docs/events.md`).

### 5.7 Inactive sessions

Sessions pile up: a root that finished its work stays open until the user ends it. A setting
ends them on its own, **off by default** (nothing ever happens unless the user turns it on): "End
sessions inactive for", 1 to 90 days, `end_inactive_days` of `sessions.toml` in the configuration
folder, set in Settings, Agents, Sessions, or by `sessions.set` (`docs/protocol.md`, "The sessions
methods").

When it is on, the daemon ends a root that is between turns and had **no activity for that many
days** in its whole tree: no event of its conversation (a turn, a tool call, a message of the
user's), no delivery made or sent, no command started or ended, no request asked or answered
(`docs/events.md` has the events; the three the daemon says on its own, `agent.tools_outdated`,
`agent.tools_reload` and `agent.released`, do not count). Nothing new is recorded for it: the latest of those is the
session's last activity. **A session with anything pending or running is never ended, whatever
the dates**: a turn running, a message queued, a command running, a request or a question
waiting for an answer, a subscription or a timer pending anywhere in its tree.

It ends the session as the user's "End session" does (`agent.stop`: the tree under it, the
commands it ran, what was queued; the workspaces stay), with the reason `inactive for N days`:
the conversation says it in the line that ends it, `agent.ended` carries it, and the window shows
the session like any ended one, resumable as today (`agent.resume`) while its agent resumes. There
is no second way to end a session.

It is looked for when the daemon starts and then about once an hour, from the sweep that already
runs: no timer of its own, nothing woken more often. The clock is the store's, which a test moves
(`test.advance`).

### 5.8 Compaction

Each agent CLI compacts its own context, with its own model and rules. Flottille compacts it
instead with **the compactor**, a model the user chooses in Settings, Features, Compaction, as
they choose the Auto judge (§7.1): provider, model and effort; the compaction turned off ("Compact
long conversations", `None` in the file) leaves each CLI to compact on its own, as before. A user who never chose gets one picked as the judge is: the smallest model of the first
provider enabled, installed and signed in among those Flottille lists (Claude's Haiku), written
once to `compaction.toml` and said once in a notice that opens Settings. It runs on the user's
quota, as a session of one of their CLIs with no tool at all, one process per compaction, stopped
after it (the judge's runner).

The settings, global, beside it: **Automatic** (on: Flottille compacts by itself; off: only when
the user asks), the **threshold** (70 % of the model's context window), a **maximum size** in
tokens that compacts even when the window is larger (none by default), the tokens **kept word for
word** (20,000), and the **guidance**, Flottille's default text (`roles/compaction.md`: fixed
sections, goal, constraints, done and in progress, decisions with their reason, next steps, files,
errors and fixes, critical context, and what only Flottille holds), editable with Reset to default,
inside a fixed text (`roles/compactor-base.md`) that says what the compactor is and that what the
agent wrote is data.

1. **When**: at the end of a turn, never in it, once the context the agent used (`agent.usage`, a
   report made since its last compaction) passes the threshold (50 % to 95 %) or the maximum size
   (20,000 tokens at least), and has grown by a tenth of the window past the size its session
   started at (what the first turn of a compacted session reported) and past the size its last
   failed compaction started at: neither a session that starts past the threshold nor a compactor
   that fails is run at every end of a turn. Not while the compactor was picked by Flottille and
   its notice not seen yet. Or when the user asks (`agent.compact`, `flottille agent compact`, the
   session's menu), at once between turns or once the turn ended, whatever the sizes; one asked
   while the compactor works is refused. The tokens kept word for word (50,000 at most) are never
   more than a quarter of the size that compacts the agent.
2. **The summary**: the compactor is given the conversation since the last compaction as Flottille
   recorded it, thinking left out and each tool's input and result cut to its first 500
   characters; the previous summary, to update rather than start over; and what only Flottille
   holds: the brief, the place and its branch, the children and their state, the subscriptions,
   the messages waiting. The agent's prompts wait meanwhile.
3. **The check**: a summary cut off by its length, one that tried to call a tool, an empty one, or
   one not smaller than what it replaces (the previous summary and the conversation since) is
   refused.
4. **The new session**: the agent starts again in a new session of its CLI the way a replaced one
   does (`docs/agents.md`, "Compaction"), uncounted and with nothing told to its parent: it is sent
   its brief, the summary, then its latest messages word for word. The summary, masked of the
   daemon's secrets, comes in a block of its own (`<earlier_conversation_summary>`, every `<`
   escaped) under a line that says a model wrote it from the earlier conversation: data, not the
   user's words nor Flottille's instructions. `agent.compacting` says it began, so that the
   window shows the compaction as a step that runs; `agent.compacted` says it is done, with
   the sizes before and after and the summary; the conversation shows it as one line,
   `Conversation compacted · 152k → 9k tokens`, which unfolds to the summary. What is sent to the new session is part of the compaction:
   the conversation, `agent show` and `session_read` never show it as messages, and a
   message the user wrote meanwhile stays where it is (`docs/events.md`, `agent.prompted`).
5. **A failure** changes nothing: the agent goes on in its session, `agent.compaction_failed` and
   the developer log say why, and the CLI's own compaction stays the safety net. With a compactor
   chosen, each CLI is set to compact as late as it allows (`docs/agents.md`, "Compaction").
6. **Around the cache's expiry**: an agent's prompt cache lives a while after its last request
   (its last turn's end), and the first turn after it expired reads the whole history again at
   full price. Two settings beside the others use the same compactor and compaction: **Compact
   before the cache expires** (off) compacts an agent at rest (no turn running, nothing waiting
   for it) a **margin** (5 minutes, 1 to 30) before its cache expires; **Compact a session resumed
   after its cache expired** (on) compacts first when a message is about to start a turn of an
   agent whose cache expired, and the message goes to the new session. Neither below a **minimum
   size** of context (50,000 tokens), without a compactor, while a picked compactor's notice is
   not seen, for a context that did not grow since the session started or its last compaction
   failed, nor for a provider whose cache lifetime is not established (`docs/agents.md`, "The
   prompt cache's lifetime"); the compaction's page says each provider's. Once an agent's cache
   expired, the composer's toolbar's **Compact** control shows just left of **Send**, ticked from
   the setting (its tooltip says why): unticked, the message goes to the session as it is; ticked, it
   compacts first even with the setting off. While there is nothing to compact the control is not
   drawn, no disabled control and no tooltip saying why not; it unfolds and fades in when the cache
   expires, and folds away and fades out after a compaction or a turn that warms the cache again.
   The composer keeps its height: an option of the composer is a control of its toolbar, never a
   block above it. A
   message from Flottille itself (a child's news, a delivery) follows the setting.
7. **The agent's own handoff**: **Summary written by** (`summary` in `compaction.toml`) is the
   compactor (the default, all of the above, shown in the window as one of the models of the
   select) or **the session's own agent** (the select's first option). Then, at the same moments
   (the threshold at the end of a turn, or asked by hand) and with no compactor needed, Flottille
   sends the agent a message of its own, shown as Flottille's and never as the user's, that asks
   for its handoff in a fixed shape: its task, the decisions and why, the files touched, what is
   done, what is left, what it was about to do. Its prompts wait meanwhile. A turn that goes on
   past the threshold and reaches it plus 5 points (98 % at most) is cut to ask for it, and the
   new session is told to go on from what it was about to do. The new session of the same agent
   (same id, place in the tree, workspace, mode, model and effort) is sent its brief, the handoff
   (`<your_handoff>`) and what Flottille holds of it (`<flottille_state>`: its place and branch,
   its children, its subscriptions, its runs still running, the messages waiting), not the
   latest messages. A handoff that fails (the agent errs, writes nothing, does not end its turn
   within the compactor's time, or its process dies) still starts the new session, from
   Flottille's state and the user's last message, and the line says the handoff is missing. While
   the agents write their own handoff and **Automatic** is on, each CLI's own compaction is
   turned off where it can be (`docs/agents.md`, "Compaction"). The threshold is the one above
   (70 % by default, 50 % to 95 %): #405 asked for its own, at 90 %, and was built on this one
   rather than add a second. The triggers around the cache's expiry stay the compactor's, when
   one is chosen: a handoff would read the whole history again at full price.

### 5.9 A spent quota

When an agent's quota runs out, its provider ends the turn with words of its own, for Claude
"You've hit your weekly limit · resets 1pm (Europe/Paris)", and the agent is idle like one that
finished its work. Flottille tells them apart by what the provider reports, never by what the agent
says: at the end of each turn of a root, the limits its provider reported during it
(`agent.provider_limit`) go through that provider's recogniser (Claude's reads the reset after
`resets`; another provider's wording is one more recogniser). A reported limit makes the session
**wait for its reset**: the instant read from the words, the next occurrence of that time of day
(on that date when they give one) in the zone they name in brackets, the machine's when they name
none, or the one passed up to ten minutes ago when the provider's clock is late; unknown when the
words cannot be read. The wait is stored (`quota_waits`), so a restart keeps it, and it is said by
`agent.quota` and in `agent.list`, `agent.tree` and `agent.show` (`quota`: `reset_at`, `resume`).

The window says it as the session's state, once: waiting, its words `Out of quota, resets at
13:00` (the reset in the user's local time; `Out of quota` when unknown), on the session's row in
the sidebar and in the rail of its page, in place of idle.

A setting resumes the session at its reset, **off by default**: "Resume sessions when the quota
resets", `resume_at_reset` of `sessions.toml`, set in Settings, Agents, Sessions, or by `sessions.set`.
While a session waits for a known reset, its composer's toolbar shows a chip, `Resume at 13:00`,
at the end of its controls, ticked or not from that setting, which the user toggles for this session alone
(`agent.quota_resume`). When it is on, the sweep (no timer of its own) delivers the session one message at the reset plus a
minute, `Your quota has reset: continue where you stopped.` (a `quota-reset` delivery, never the
user's), and the session runs again. The wait for that instant is taken in one write, so the
message goes once; a reset that could not be read resumes nothing; a reset that came while it is
off ends the wait and sends nothing. A reset already past when the daemon starts is looked at
then: it resumes at once when allowed. The wait ends with a message of the user's (which starts a
turn anyway), a turn that ran without the limit, the reset, and the end of the session. Only a
root waits: a child's limit is its parent's to read (§5.3).

## 6. Projects and workspaces

### 6.1 Projects

- `flottille project add <folder>` registers a folder. Repositories are detected: the folder
  itself if it is one, and folders one or two levels down holding a `.git`. The user confirms or
  edits the list.
- Per repository: base branch **rule** (an ordered list, first one that exists on the remote
  wins, default `[main]`; for example `[develop, main]`), remote (default `origin`), and its
  dependencies on other repositories of the project (used to order preparation steps and,
  later, merges).
- Per project: branch prefix, workspaces root (default
  `$XDG_DATA_HOME/flottille/workspaces/<project>`, or beside the project:
  `<parent>/<project>-work/`), which repositories a workspace takes by default (**all**, or a
  chosen set), variables, tasks, setup, skills, commands, "never" list, children cap.
- The project configuration is a TOML file Flottille owns (in its config folder), editable by
  hand and from the window. Nothing is written into the user's repositories.

### 6.2 Workspaces

Creation, the up-to-date base per repository (fetch of one branch into its tracking ref, with
freshness), `git worktree add`, preparation in the background with resume, failure capture and
the git invocation rules follow `reference/hemera.md` §6. Additions over that model, all required
by `reference/multi-repo-case.md`:

- **Repository set**: all of the project's repositories by default; the project can say so
  ("environment scope is not change scope").
- **No fetch imposed**: a repository with no remote, or whose fetch fails, starts from its own
  base branch, and the creation says so in one line (`docs/workspaces.md`).
- **Root entries**: when the project folder is not itself a repository, its other top-level
  entries (instruction files, shared folders) can be linked into the workspace folder by one
  setup step, never `.git` nor `.gitignore`.
- **Ordered per repository**: `run` steps can be declared once "for each repository, in
  dependency order" (leaves first), with `{repository}` in the template.
- **Variables replace shell hooks**: workspace variables (templated, for example
  `PACKAGE_STORE={workspace.path}/.store`, `COMPOSE_PROJECT_NAME=app-{workspace}`, a `PATH`
  prefix) are injected into every command run in the workspace and into the agent's command
  runs. No `.envrc`, no direnv.
- **Copy of ignored files**: `copy` steps take globs (`.env`, `.env.*`), per repository, never
  overwrite.
- **Removal** refuses a worktree with uncommitted files **or commits not on any remote**
  (`git log HEAD --not --remotes`), names them, and never forces. Commits contained in another
  local branch of the repository (`git merge-base --is-ancestor`) are integrated, not lost: the
  removal goes on, and deletes the branch saying where it is (`it is integrated into dev`).

Agents are placed in a workspace or in the main checkout; what each may do there is the gate's
(§7).

## 7. The gate

Every tool call goes through one gate, as in `reference/hemera.md` §4: tool admitted for the
role, arguments decoded, place rules, then the call's **class**. The gate grants no right: it
says what kind of call it is, and the session's **mode** says who answers it.

- **Harmless** calls pass, in every mode: reading in the project, writing in a workspace, the
  commands the rules know to only read, a task in a workspace. A known program is
  the system's only: named bare and found through the run's `PATH` outside the project, its
  workspaces, the place and the user's `~/.local/bin` and `~/bin`; an option that runs a program or reads past ignored files toward a secret takes it
  out of this class.
- **To judge** is everything else: writing in `main`, reading outside the project, a line the
  rules do not know or whose program is the agent's own file, an option of a reading program
  that runs one, a commit, a push, a forge write, a change of the project an agent asks for.
- **Base** calls always come to the user in Ask and Auto, with a strong warning, and pass only in
  Full, the mode the user chose for that: a push forced
  (with a lease or not), rewriting history that is on a remote, discarding work in `main` or
  moving the branch it has checked out, removing git's reflog or unreachable objects, `.git`
  folders, git settings that run programs or lead pushes (`core`, `alias`, `remote`, `url`,
  `push`, `credential`, `include`, and every key whose value git runs, set by `-c` or `git
  config`, or through the environment), a push naming the program that receives it, running as
  another user (`sudo`, `doas`, `pkexec`), sensitive places (`.env` files, credential folders,
  Flottille's own folders, another process's `/proc`), publishing, a merge whose checks are
  waived, a change of the project's tracker or of its rules, and the project's `never` list.
  They are looked for in every line a wrapper, an option or `git rebase --exec` runs, and in
  every end of a line whose program the gate does not know, so that no wrapper turns them into
  calls to judge.

Only a push the gate read and let through goes out: every run's environment rewrites push URLs
to nowhere, but a run that is itself a `git push` the user, the judge or Full mode allowed (`docs/tools.md`,
"Tasks and runs").

What stays **refused** is not a permission but a call that cannot be made as it is: shell syntax,
a malformed call, a login printed, the daemon driven from a command.

### 7.1 The mode and the judge

Each session is in **Ask**, **Auto** or **Full**, a small three-choice control in the composer's
bar, the same in the composer that starts a session. A session the user starts starts in Auto
unless the user chose another mode in that composer (or `flottille agent start --mode`), and the
mode is the root's before its first message, so that even the first turn's calls follow it. That
composer is back on Auto each time it is shown and after each start: a choice never carries over to
the next session, so that Full is never on by mistake. A tree has one mode, its
root's, stored with the root and read at each call: a switch reaches every agent of the tree at
once, children included. Only the user changes it, in the window or with
`flottille agent mode <agent> ask|auto|full` (`agent.mode`, which switches the tree's root
whatever agent of it is named); no tool of an agent does, and a session an agent starts for the
user (`agent_launch` with `kind` `led` or `handed`) takes the mode of the tree that started it, Full included, never
above it: an agent in Full can already do what the session would do, the session caps still apply,
and the rules refused in every mode stay refused in it. Each tree keeps its own mode afterwards: a
switch of the launcher's tree does not reach it. It is kept when the session resumes. An agent
reads its mode, never sets it: `session_read` with no `session` (its own id is not told to it)
answers with its id, its role and its tree's mode, and the text Flottille starts every session
with says so. Reading another session says the mode of that session's tree too.

- In **Ask**, a call to judge is a card for the user.
- In **Auto**, it goes to the **judge**, which allows it or asks the user: it cannot refuse, and
  base calls never reach it.
- In **Full**, nothing is asked: the agent acts as the user. Every call the gate would hold
  passes at once, base calls included (a forced push, a workspace removed with what it would
  lose, inline interpreter code, a script the agent wrote), with no request and no judge, but for
  the first message of a session to another it did not start, whose leave only the user gives
  (§7.2), and a tracker change that would send the worklog's token to another site or name
  another entry for it (`docs/tickets.md`, "Logging time"). Bare mode
  stays: the agent has only Flottille's tools, and every refusal below stays a refusal, those that
  keep the user's logins and Flottille itself included. Each call it let through is recorded as
  allowed by Full mode (`gate` `full` in its `agent.tool_served` event and in the developer log),
  and its step in the session says "allowed by Full mode". The control draws Full in the alarm
  colour, so that it is never on by mistake. A request made before the switch stays the user's to
  answer. **Its limit:** code the agent runs (an interpreter's inline code, a script it wrote) acts
  with the user's rights, and the gate does not see what that code touches; only a sandbox would
  close that. The refusals below protect the user from an agent's direct or accidental use of
  what must never be touched; they are **not a sandbox**: a program the agent runs may still do
  any of it on its own. The same holds for a connection's server (`extensions.md`,
  "Connections"): the gate sees the action called and its arguments, never what the server then
  does with the user's rights, whatever the mode.

A call of a connection is **to judge** by default; the user may mark one action **passes**
(harmless) or **asked** (base: a card in Ask and Auto, never the judge's). The judge reads the
action's description as the user accepted it, in a block of its own, escaped, and never the
server's instructions. An action turned off, arguments that do not fit the accepted schema or a
limit, and a connection not ready are refused in every mode.

What becomes of a call, by what the gate says of it and the tree's mode:

| The gate says | Ask | Auto | Full |
|---|---|---|---|
| Harmless | passes | passes | passes |
| To judge | a card for the user | the judge allows it, else a card | passes, recorded as allowed by Full mode |
| Base (a forced push, a `.env`, a read of Flottille's own folders…) | a card, with a strong warning | a card, with a strong warning | passes, recorded as allowed by Full mode |
| Refused (a login's place, a write of Flottille's own folders or of a startup file, a launcher out of the tree, a git setting that runs later…) | refused | refused | refused |

**Refused**, in every mode, is a call that cannot be made as it is, or that bare mode does not
have: shell syntax (in a line, a task's or a setup step's line, a `git rebase --exec`), a
malformed call or an unknown action, a tool the agent does not have (a child's
`agent_launch` of a session, a child's `workspace`, a tool of its provider), a login printed (`gh
auth token`, `aws configure get`, `kubectl config view --raw`, `gcloud auth print-access-token`,
`git credential fill`, a password manager or a keyring: `pass`, `gopass`, `op`, `bw`, `rbw`,
`secret-tool`, `docker-credential-*`),
the daemon driven from a command (`flottille`), a brief, a title or a body longer than the judge
is given whole, a path that goes up out of a folder that is not there, a text the call does not take, a project write that cannot be done
as asked, the removal of the agent's own workspace or of one another agent works in, and a call
of an agent that was replaced. It is also what would let an agent reach the user's logins, or
drive Flottille later or from outside its tree of processes, through a file tool or any path a
line names, every program a wrapper or a runner runs included:

- **a login's place**, read or written: `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.azure`,
  `~/.config/gcloud`, `~/.kube`, `~/.docker/config.json`, `~/.config/gh`, `~/.netrc`, `~/.npmrc`,
  `~/.pypirc`, `~/.git-credentials`, `~/.local/share/keyrings`, `~/Library/Keychains`,
  `~/.local/share/opencode`, wherever they are along a path, and `/etc/shadow`, `/etc/sudoers`,
  `/etc/sudoers.d`; the agents' own folders of the home (`~/.claude`, `~/.claude.json` and its
  copies beside it, `~/.claude.json.backup`…, `~/.codex`), whole, where the path leads included (a
  link to them, a project that is the home folder), and a file of a revision named `rev:path`
  (`git show HEAD:.claude/.credentials.json`, `git cat-file`, `git diff`) as the path it names;
  and the open files of any process, the agent's own command line and runs included
  (`/proc/<pid>/fd`, `map_files`, `/dev/fd/3`), which may be one. These refusals of paths are
  what keeps a login from an agent. The masking of outputs (a token key's value in a diff of a
  staged login file, `docs/tools.md`) is not a boundary: a line that reshapes the bytes (`xxd`,
  `od`, `base64`) gets them past it;
- **an agent's folder anywhere else** (`.claude`, `.claude.json`, `.codex`, and `.mcp.json`,
  whose servers Claude's command line starts once the user agrees, of a repository, a
  workspace, a copy in `~/Downloads`): it is a project's configuration, not a login, and is read
  as any file, but a `.credentials.json` or `auth.json` below it is a login's place, and **a
  write there** is refused: its settings and hooks run programs the next time the user opens
  that repository with the agent's own command line, outside the gate, so the agent asks the
  user to make the change. A line may write there without naming it (a patch applied, a branch
  checked out or merged, a stash popped, a folder copied, an archive unpacked): what an agent's
  run changed in the agent folders of its place is put back as it was once the run ends, and
  its result says it was refused, naming the files. Only the runs of a line the gate held (to
  judge or base, whoever let it through), the runs of a task and a workspace's setup lines,
  which run what its repositories hold, are guarded so (a setup line's put-back is told in the
  inbox, of no agent, and in its failure); a service, which runs in its own folder until it
  stops, is not. A line the gate let
  through at once only reads, or writes the files it names. Runs that overlap in a place, or in
  places one inside the other (a repository's folder in its project's), share one guard: the
  first notes the agent folders, a run in an outer place takes in what the inner ones noted,
  the last to end puts them back, and its result says so, as does an entry of the user's inbox
  (`run.agent_folders_put_back`), even when no run started. What the put-back removes or overwrites is moved to Flottille's state folder
  (`agent-folders/<run>`, dropped at the daemon's start once older than 30 days), which the
  result names: a user's own edit there during the run is
  undone, never lost. It writes nothing below a folder that is no longer a real one (a link
  made since, to the home or anywhere): that entry is said as not put back; each folder is
  opened by its handle, never through a link, so one made a link while the put-back goes on
  leads it nowhere. The agent folders right in the place and in each of its repositories are
  always looked at whole; the rest of a place of more than 500,000 entries is looked at up to
  that bound, and the result says its agent folders could not all be checked, as does an entry
  of the user's inbox (`run.agent_folders_unchecked`); so does a repository of the place whose
  folder is outside it (reached through a link), whose agent folders are not looked at. A run
  whose place's agent folders could not be noted is refused, with a sentence: nothing it wrote
  there would be put back. Past the bound on the contents kept (64 MiB), the settings files
  (`settings.json`, `settings.local.json`, `config.toml`, `.mcp.json`, `.claude.json`) and the
  scripts below `.claude/hooks/` are kept first, then the smallest; a file written between the
  look at the agent folders and the read of its content keeps none: it is not put back but moved
  to `agent-folders/<run>`, and the result says it changed while the agent folders were noted,
  not that the run wrote it. A run waits for
  the snapshot of its place, and for a put-back going on there, never for another place's. What this does not cover: a process that outlives its run (one that
  left the run's session, `setsid`), a long `serve` run until it ends, and a run going on when
  the daemon stops or crashes, whose place is not put back: a marker of each place guarded
  (`agent-folders/.guarded`) tells the user's inbox at the daemon's next start; the files also
  exist while the run goes on;
- **a write in or above Flottille's own folders** (its configuration, data, state and runtime:
  the store, `providers.toml`, `developer.toml`, the socket), or of Flottille's own program.
  Reading them stays base: a card in Ask and Auto, allowed in Full, since what they hold has its
  secrets masked and the user may want an agent to read a log;
- **any write through a process's folder in `/proc`** that leads elsewhere (`/proc/<pid>/cwd`,
  `root`, `fd`, `exe`, `map_files`, `/dev/fd/3`): the daemon runs in `/`, so through it any path
  of the disk. Reading through `cwd`, `root` or `exe` stays base;
- **a write of a startup file**, or of a folder above one: `~/.bashrc`, `~/.bash_profile`,
  `~/.bash_login`, `~/.profile`, `~/.zshrc`, `~/.zshenv`, `~/.zprofile`, `~/.zlogin`,
  `~/.config/fish`, `~/.gitconfig`, `~/.config/git`, `~/.config/autostart`,
  `~/.config/systemd/user`, `~/.config/environment.d`, the desktop's `~/.config/hypr`,
  `~/.config/sway`, `~/.config/niri` (and the same under `XDG_CONFIG_HOME`),
  `~/.local/share/systemd/user` (and under `XDG_DATA_HOME`), and the user's programs,
  `~/.local/bin` and `~/bin`;
- **a git setting that runs a program later**, as the user: `git config --global` or `--system`
  (the user's and the system's own configuration), and in the repository's a key that runs a
  program or brings in a file (`core.hooksPath`, `core.fsmonitor`, `core.sshCommand`,
  `core.editor`, `core.pager`, any `alias.*`, `include.path`, `includeIf.*`, `credential.*`, a
  diff or merge driver, a filter, `protocol.allow`…). Removing such a key is not refused; where
  pushes go (`remote.*`, `url.*`, `push.*`) stays base. A program named to run for a remote,
  which may be this machine, is refused too: `--upload-pack`, `--receive-pack`, `--exec`,
  `git clone -u`, `git -c core.sshCommand`;
- **a program that opens a link on the desktop** (`xdg-open`, `open`, `start`): a run has no
  graphical session, so nothing would show. Refused where a launcher is, with a sentence that
  points to `web` `show_link`, which hands the user the link and lets the window open it after
  their click (`docs/tools.md`, "Showing a link");
- **a launcher** that starts what it runs outside the daemon's tree, later or on a host:
  `systemd-run`, `tmux`, `screen`, `zellij`, `dtach`, `abduco`, `setsid`, `at`, `batch`,
  `crontab`, `dbus-send`, `gdbus`, `busctl`, `loginctl`, `systemctl` but for its reads (`status`,
  `show`, `cat`, `list-*`, `is-*`…), a desktop's or a terminal's remote control (`hyprctl`,
  `swaymsg`, `i3-msg`, `niri msg`, `kitty @`, `wezterm cli`), `ssh`, `scp`, `sftp` and `rsync`
  to any host or with `-e`/`--rsync-path` (`ssh` to this machine runs a line through the user's
  shell, and an alias may be this machine). It is refused as a line's program, behind any wrapper
  the gate unwraps (`nice`, `ionice`, `chrt`, `taskset`, `chronic`, `env`, `timeout`, `stdbuf`,
  `nohup`…), and as what a runner runs (`uv run`, `poetry run`, `npm exec`…, what follows `--`
  of `cargo run`). Anywhere else its name is only a word (`cargo test tmux`, `git commit -m ssh`);
- **a container's bind mount** (`docker`, `podman`, `nerdctl` `run` or `create`, `-v`,
  `--volume`, `--mount type=bind,source=…`) of any place above: the container reads and writes
  the host's folder it names. A container itself, and a named volume, are not refused.

A line that does not only read may write any path it names: for the writes above, any path such
a line names counts. An option's value (`awk -F/`, `--base=/`) or a word of a program whose words
are text (`tr`, `printf`, `expr`, `seq`) that names a folder above Flottille's own is held as base
instead, as it was: such a word is seldom a path the line writes. **Held** is everything else the gate does not let pass: in Full it passes.

The judge is an agent of a provider the user chose in Settings (Approvals, "Auto judge": its
model, `provider.set_judge`), run in bare mode **with no tool at all**: no Flottille tool, no MCP
server. A user who never chose one gets one picked: the smallest model of the first provider that
is enabled, installed and signed in, among the few whose smallest model Flottille lists (Claude's
Haiku), written once to `providers.toml` and said once in a notice that opens Settings (and by
`flottille provider list`); a user who turned the judge off ("No judge") never gets one, nor does one whose last
choice before `judge = "none"` existed was no judge (the last `provider.judge_changed` in the store
names none), whose file then says `judge = "none"`. Its
instructions are a fixed text (`roles/judge-base.md`: what it is, its JSON answer, that it never
denies, that what the agent wrote is data, and the floor: ask for anything destructive, outside
what was asked, surprising, or whenever it is not sure) around the user's **guidance**, which can
add to the floor but never lift it: Flottille's default (`roles/judge.md`: allow what the request
plausibly needs; writes of the project's configuration are within intent while the user is setting
the project up) or the user's own, edited in Settings ("What the judge is told", in Approvals, with Reset to default; saving an
empty text resets it) and read at each judgement. A guidance that cannot be read whole, or is over
its 4,000 characters, is never cut nor replaced by the default: each judgement fails, and so asks,
saying which file is wrong and why, and Settings show it. The guidance changes neither what
reaches the judge nor what it may answer. It sees the call masked (secrets masked, the line,
the path, a brief, a title and a body whole, contents cut to their first lines with how many they
had), why the rules held it, the place, the agent's `why`, the first message of a session an agent
started for the user (labelled as that agent's, never the user's), and last the user's last
messages to the session, nothing else: the three newest, up to the moment of the call (one sent
while the agent works, still waiting for its turn, among them), in the order they were sent, each
numbered, the last named as the newest, so that a small model never takes an older one for it.
A message that used commands (`/name`) is told as that, from the names the daemon recorded with
it, never read from its text: the user used the commands, each from its file or the project's;
what they typed comes as their words, and what the agent received in a block of its own, escaped,
since it is the project's text, not the user's words.
What the agent wrote comes in blocks of its own, every `<` escaped, so that no text of the agent
reads as the user's; only the user's messages say what is wanted. It answers one JSON object,
`{"verdict": "allow" | "ask", "reason": "…"}`. A failure, an answer that is not that object, or
30 seconds without one is an ask; a line or a path longer than the 8,000 characters it is given
whole asks without it, and the gate refuses a longer brief, title or body. A process of the provider is started for each judgement, and its whole
process group stopped after it. Each verdict is recorded with the
call (who judged, what, why: the `judgement` of its `agent.tool_served` event, and of its request
when it asks), shown on the card and in `flottille approvals`.
Without a judge, none picked or the judge turned off, Auto behaves as Ask, and the control carries a
small warning icon whose tooltip says so and whose click opens Settings, where the judge is
chosen. A judgement is a turn of the judge's model: it uses the user's quota on that provider,
which Settings say.

### 7.2 Requests

An "ask" writes a numbered request (the frozen call, its class, the reasons, the judge's verdict
when one was asked, the agent that asked) and is answered to the agent at once ("request #n is
waiting…"). A tool call never waits for a human or an agent. Who holds the request depends on
who asked:

- **A root's request is the user's.** It appears in the window, in `flottille approvals`, and in
  the event stream. The user allows it once or denies it: there is no lasting grant, but one.
  **The leave of a pair of sessions**: the first `session_send` of a session to another it did
  not start (nor was started by) is held for the user in every mode, Full included, and never
  reaches the judge; allowing it delivers the message and lets that session write to the other
  from then on, one way, each message then judged as a write is, until either session ends or
  the user withdraws it where the session's rights are shown (`docs/tools.md`, "Sessions that
  talk to each other").
  Ending a session that is not the agent's own child (`agent_stop`) is held for the user the same
  way, in every mode, Full included, and every time: nothing is kept (`docs/tools.md`, "Ending
  another session"). The one exception is a session the agent itself started as `led`, which it
  pilots: that one is its own to end, judged as any call in the tree's mode. A session the user
  opened, or a `handed` one, stays the user's.
- **A child's request is held by its parent**, as a `child-asks` delivery. The parent denies it or
  forwards it with one sentence of why (`approval_answer`); forwarding passes it to the parent's
  own parent, one level at a time, and from the root to the user, with every reason given on the
  way. **No agent can allow a request**: the gate's "ask" always ends with the judge or with the
  user, or with a refusal. A request held by an agent is not in `flottille approvals` and the user
  cannot answer it until it is forwarded.

An allowed request is re-checked and performed, and its result comes back to the agent that asked
as an `approval` delivery; a denied one comes back as a refusal with the why.

Agent-run commands get an environment without forge tokens, and without the repository's git
hooks (`reference/hemera.md` §3), and with every push walled but the run that is itself a
`git push` the gate let through: a push goes through the gate like any other call, and only so.

**Commands** are named instructions of the project (`commands.md`), sent to a session as `/name`:
a command grants nothing, each call the session makes goes through its mode like any other.

## 8. Instructions

Built as in `reference/hemera.md` §7: a shared base, the role template, a cache boundary, the
per-session part, then the project layer, frozen per session. The base has two parts. **What
Flottille enforces** is fixed text that states what the gate does, so that an agent does not take
it for its own discipline. **How to work** is the user's: a default text, the user's own for all
projects, with a project's own added after them (`projects.md`, "Rules"). The global rules sit in
the base, before the boundary, where every project shares them; a project's own sit just after the
per-session part,
since they would otherwise make the shared part differ from one project to the next. Alongside, the
MCP server's `instructions` explain Flottille itself to the agent (`tools.md`, "The server"). The project layer holds, per
repository and for the project folder: `CLAUDE.md` or `AGENTS.md` (the agent's own name first),
**and the local files `CLAUDE.local.md` / `AGENTS.local.md` when present** (they hold rules the
user keeps out of the repository).

After the per-session part, before the project layer, comes the list of the project's **skills** (texts an agent reads
with the tool `skill_read`: what knowledge skills hold in other tools): a name and one line each,
never the content (`skills.md`).

A project's **commands** (`commands.md`) are what slash commands hold in other tools, see
`reference/multi-repo-case.md`: `/name` sends a command's instructions, after a header that says
the workspace, its repositories by dependency level and the project's pull requests.

## 9. The built-in orchestration standard

`reference/orchestration.md` is today's manual way of running agents that hand work to each
other. Flottille turns it into the default behaviour: the lead's instructions carry its rules
(one precise task per child, lowest safe model, the brief's content, how to read a report, never
merge without the checks), and Flottille itself provides what was done by hand: the run folder
becomes the child's record, the report is a tool, waiting is deliveries, closing a child is
`agent_stop`, the caps are enforced.

## 10. The window

GPUI Fast with gpui-kit, English only (no translation layer). Screens of the milestone "The window":

- a top bar with a search field and a bell counting what waits for the user (each request, each
  question) and what a session did to a ticket or a pull request (opened, merged, edited or
  commented a pull request; created, edited, commented, closed or reopened a ticket, as
  `agent.tool_served` says a write of `forge` or `tickets` was done), until that session is
  opened; each item says why it is there (`Request #3 waits for you`, `Asks you a question`,
  `Opened a pull request`), a short excerpt (what is asked, the question, the title or the start
  of the text posted) and its session, each on one line cut with an ellipsis. What a session
  changes while its page is open is in sight, and rings nothing; what is changed lives as long
  as the window;
  the search field, "Search  Ctrl K", is a **quick switcher**: Ctrl K anywhere in the window or a
  click on the field opens it over the page (the modal layer, near the top), its own text field
  focused; Escape, a click outside, the mouse's back button or choosing a result closes it, and
  the focus goes back to what had it. It finds, in groups with their headings, **Sessions** (by
  title; each with its project and place and its state as the tree shows it, `NEEDS YOU` or
  running, else how long ago), **Projects** (a project and each of its workspaces, which open
  the project's page) and **Actions** (New session, Add a project, Notifications, Settings
  and each tab of it, each project's settings: one list in code, `actions()` of the switcher,
  where a new screen adds its entry). It
  matches each word typed, ignoring case, accents and ligatures, in the title (its letters in
  order, close together, are enough) or as a whole in the words around it (the project and
  the place a session is in), the best match first and the most recently active among equal
  ones, and draws what matched in the accent; the best match of all groups is the marked one.
  With nothing typed it lists the recent sessions and the actions. While it is open, a read of
  the daemon changes what the results say (a state, how long ago) but not their order: nothing
  moves under the pointer until something is typed. The arrows move the marked result, Enter
  chooses it, and the pointer marks the row it is on and a click chooses it. Ctrl K waits while
  a dialog or a list is open. It searches what the window already holds from the daemon (the
  tree, the projects), never the content of conversations (that needs an index);
- left: the sidebar, each project and, under it, **its sessions in one flat list**, sorted by
  last exchange (the later of the last message the user gave it and the end of its root's last
  turn, its creation before either: the session that last had one on top. Work done during a turn
  never moves a row, so two sessions working at once keep their places; a state is drawn by the
  row's mark, never by reordering). There is no row for `main` nor for a workspace: workspaces live on the project
  page, and one with no session is not in the sidebar. A session is titled by the title its agent
  gives it in its first turn (`session_title`, 3 to 8 words, `docs/tools.md`, "The session's
  title") or the user gives it (`Rename…` in its row's "⋯" menu and its header, a dialog with a
  labelled field; the user's title always wins), and until then by the first line of
  its first message that says what the work is (a greeting or a single word does not: the session
  is `agent <id>` until a later message of the user does); a new title fades in over the old one in
  the row and the header; a session an agent started says it in
  its page's header, "started by <agent>", not in the sidebar. A session an agent leads (`led`)
  is the one exception to the flat list: its row sits right under its pilot's session's row,
  indented one step, tied to it by a connector (the line from the pilot turning into the row,
  ending there with no arrowhead); only a led row is indented, every root's title
  starts at the same x. The pilot's row gains a chevron, at the end of the row beside its time and
  "⋯", that folds its led rows away with it; folded, its state mark is the most urgent of its own
  and its hidden led sessions' (waits for the user, then running, then its own), nothing else is
  added to the row; each led row keeps its own unread dot and draft pencil. Its page's header says "led by <pilot>", and a line above its composer says
  "Your messages go straight to this session; its lead is not asked." (it names no session: a
  sentence of the window that quotes a session's title shows it on one line at most, cut with an
  ellipsis, in emphasis, never wrapped into the sentence). The row's "⋯" menu of a
  session an agent started offers "Detach from its lead" (`agent.detach`, a toast says the
  result; the lead's title is a second, fainter line of the entry, cut to one line, never wrapped; the
  agent's "⋯" menu in the session's rail offers the same action while the session is led): the row slides back among the project's sessions, its indent and mark fade, and the
  header's line and the note fold away. A question a led session asks its pilot shows in its
  conversation as a card, "<session> asks its pilot <pilot>", with no answer box and, under it in
  small text, "Its pilot answers at the end of its turn; you can answer here too." (the user's
  messages reach the session, as the note above its composer says). The last one still waiting
  stays in sight under the steps while the session waits on its pilot; once the pilot no longer
  follows the session, the card folds away and the question comes again as the user's own, "<session>
  asks you", with its answer box. Each session takes
  **two lines**:
  the first is its title and, at the right, **one mark**, chosen in this order: its state
  (running, needs the user, waiting) when it has one, else a small dot when something new was
  written in it since it was last opened (an agent's message, a report, a delivery that woke it, a
  ticket or a pull request an agent changed), else how long ago it was active; and no text badge.
  The second line, faint, is the place it works in (a folder icon and `main` for the project
  folder, a branch icon and the workspace's name) and then, when there is room, its pull request
  `#123` with its checks' mark (a check, a cross or a dot: shape as well as colour); it is cut
  with an ellipsis inside the row and the whole is in a tooltip. The pull request is the one the
  project page last read for that workspace (`workspace.forge`): the sidebar asks the forge for
  none, so a workspace whose project page was never opened in this window shows its name alone,
  and the pull request fades in when it becomes known. The mark says its words under the pointer
  (`Needs you: a permission`, `Waiting: checks of #1`, `Running`, `New since you looked: opened a
  pull request`); the dot is cleared when the session is opened and lives as long as the window.
  The mark is where the work is, once: on the session's own row while that row is in sight, and on
  a folded project, which shows the strongest mark of what it holds, only while it is folded
  (never on a row and on its project together); a session whose agent ended shows no mark. A
  session that becomes the latest active slides up to the top; the one that is open stays open,
  whatever moves around it. A click on a project's name opens its page and leaves its sessions as
  they were, only its chevron folds and unfolds them; a session's row offers a "⋯" menu with End
  session (the confirmation of the session page, counting the agents that end) and, on an ended
  session whose agent can resume, Resume, with no need to open the session first. A session is
  started in a place from the project page (each place's "New session here");
- a project page: a header with its name, its folder (relative to `~`), Settings and New workspace
  (its two buttons), and a chip per repository with the branch its checkout is on (none for a
  project of one repository); under it the composer that starts a session (place, agent, model,
  first message, and pictures taken as a session's composer does, which go with the first
  message; its place's menu ends with New workspace…; with no provider ready it says "No provider
  is ready. **Open Settings**", which opens Agents, Providers); then its tabs, **Work**, **Pull
  requests** and **Tickets**, and **Specs** while Specs is on in the project (four at most, room
  kept for a fifth), the chosen one alone below them (`reference/gpui.md`, rule 13). Creating a
  workspace shows the preparation's progress and failure output, removing one asks first.
  **Work** is one column of the project's places: `main` first, then the workspaces from the
  newest to the oldest, in an order nothing changes (activity orders the sidebar, never this
  page). Each place is a block. Its row: a folder icon and `main`, or a branch icon and the
  workspace's name, whole, never cut for what follows; on its right, quiet chips for what
  describes it, its pull request (`#118` and its checks' mark), its ticket (`SHOP-42`), the lines
  it changed (`+120 −14`), or `no session` for a workspace where none works. On hover, three icon
  buttons fade in: ▶ "Run a task here", which lists the project's tasks (each with its tag,
  `server` or `check`) and runs the one picked in that place (`task.run` with its `workspace`);
  + "New session here", which sets the composer's place; and "⋯", with Open in editor, Link a
  ticket… (Change the ticket… once one is linked), Prepare again and Remove workspace…, which asks
  first and refuses, naming them, while sessions not ended work there. A workspace's second line,
  faint, says in labelled values where it starts from and where each repository stands (`from
  develop · api: 1 commit ahead, 5 files changed · web: clean`; a project of one repository does
  not name it), and the setup steps played. Under it, the sessions that work in this place and
  only they, each listed once, where it works: its title, then one mark only when it says
  something (running, needs you, waiting), else how long since it was active; a session that
  leads others says `leads 2: export-csv, checkout-retry`, each place a link that scrolls to its
  block; a led session says `↳ led by <pilot>`, a link to its pilot's session; a session that
  waits for the user has a second, faint line, the question or the request (the colour stays on
  the mark alone). A click on a session opens it. Then the task runs going on in this place, the
  user's and its agents': one row each, its task, its address as a link when it serves one, how
  long, and a Stop icon button whose tooltip names the task. At the bottom of Work, folded, `3
  ended runs`: the last runs that ended, each with its place (`in main`, `in checkout-retry`) and
  how it ended. A click on a run opens its drawer. A workspace with no session is its row alone,
  with its facts.
  **Pull requests** and **Tickets** (`items.md`) each count what they list, filtered on one row:
  `Open | Closed | All`, **Mine**, a repository for the pull requests, and a search by key or
  title (the quick switcher's matching, what matched in the accent); a line under the row says
  how many are listed, `128 open`, or `3 of 128 open` under a search. No sentence says how they
  are read. The list is about ten rows tall and scrolls inside its tab, the filters staying above
  it, keeps its place when it is read again and goes back to the top when a tab, a filter or the
  search changes; each row its number, title, state, CI mark, author, assignee and when it
  changed, and for a pull request of a workspace, `in checkout-retry`, coming, going and changing
  live with the `item.*` events and going with `items.forgotten`; a project with no forge says
  "No forge set for this project. **Choose one in Settings**", which opens Repositories, one with
  no tracker the same for its tickets, and a source that fails says why; a click opens the item's
  detail in a dialog: its description in Markdown, its branches, its checks with their links, its
  reviews and comments, an agent's marked "posted by an agent", and Open in browser. A ticket's
  row offers "Write the spec", which starts it at once in the default mode, and "Write the spec
  as…" in its "⋯", or "Open the spec" once it has one. **Specs** lists each spec with its status,
  its tickets and its open questions, with "New spec"; a spec's page shows the spec, its
  questions each answered in one click, Ready and Back to draft, and the sessions that worked on
  it (`docs/extensions.md`, "Specs");
  the project's Settings, in five tabs (`reference/gpui.md`, rule 13), opening on the first.
  **Repositories** holds its **Repositories** (a count; "The git repositories of the project. Each
  workspace gets its own copy of each.", or for a project of one, "The project folder is the
  repository."): each a row with **Starts from** (`develop, then main`), **Remote** (`origin ·
  github.com/acme/shop`, or `local only`) and **Forge**, which says in one value the forge and the
  state of the command line it needs (`GitHub · gh ✓`, its version and the host it is signed in to
  in the tooltip; the repository on the forge said only when it differs from the remote's). When the
  program is not ready, that is the line that says what is missing and what to do: the command to
  sign in (`gh is not signed in: gh auth login`, with Copy), or the program's own page and a command
  to copy where one fits most systems (`forges.md`; Flottille installs nothing), and a quiet **Check
  again** on that line only. A repository the folder holds and the project does not name is a row
  of the list, `web/ found in the folder`, with Add; an empty list says "No git repository in this
  folder yet. Flottille looks again when you come back." No button checks the programs or detects the
  repositories: both are read when the tab opens, after a change of forge or tracker, and again each
  time the window takes the focus back while the tab is shown, in silence (only a line that changes
  moves). Under them, its **Tracker** ("Where the project's tickets come from."): **Tickets from**,
  `Nowhere`, `GitHub issues` or `Jira`, its fields, the state of `gh` or `acli` said as a
  repository's forge says it, and Log time with (`projects.md`). **Workspaces** holds its
  **Folder**, with no sentence: **Where**, `As in Settings` (its line the folder it gives, relative
  to `~`) or `A folder for this project`, whose field unfolds under it; its **Branches** ("How new
  workspaces name their branch."): **Prefix** (its line `New workspaces get feat/<name>.`) and
  **Allowed names** (a regular expression; "Empty: any name. Anchor with ^ and $."), a warning
  unfolding under the field it is about; and its **Setup** ("Done in order when a workspace is made.
  Drag to reorder."): its steps, each with its kind, where it is done (`in the project folder`) and
  its time limit (`stops after 10 min`, chosen among durations), added with New step, edited, removed
  and reordered. **Tasks** holds its tasks ("Command lines the project runs: tests, builds, servers.
  Agents run them by name; you run them from a place's ▶ on the project page."): a line, where it
  runs and what it is (`Changes nothing`, `A check (tests, lint)` or `A server`), added with New
  task; a task's "⋯" holds Run, which runs it in the project folder, beside Edit, Copy to… and
  Remove…. **Instructions** holds its **Rules** ("What this project's agents are told, after your
  rules for every project."): **Rules for every project** above, read-only and folded, with Edit in
  Settings; a switch, "Use only this project's rules"; the project's editor, Reset to default, and
  Save once it changed, the toast "Rules saved · Agents started from now on follow them."; its
  **Commands** (a count; added from a list of the templates and a blank one with New command,
  edited, removed); its **Skills** (a count; a path picked in the project folder, or a text; New
  skill, edited, removed); its **Commit and pull request checks** and its **Pull request template**
  (`projects.md`). **Features** holds the same list as Settings', of the features that touch a
  project (Specs, Review, later the handbook): each row opens its page, with the project's switch
  and the values a project may set, each offering As in Settings first (`reference/gpui.md`, rule
  14); a feature off in Settings is a faded row whose line, Off in Settings, is a link. A field saved
  on its own shows its Save only once it changed (rule 4);
- Settings, in tabs by subject (`reference/gpui.md`, rule 13): **Agents**, **Approvals**,
  **Features**, **Vault** and **General**, one shown at a time, as in a project's settings. It
  opens on Agents; a link to a section opens its tab with the section in view; Usage will be the
  sixth tab and the last. No part of Flottille adds a tab of its own: what the user turns on or
  off is a row of Features.
  **Agents** holds four sections. **Providers** (a count; "The agent programs Flottille can
  start. Each signs in with its own login."): one row per provider, its switch labelled with its
  name (`Claude`), its version as a tag (the path of its program in the version's tooltip), and
  one line saying its state once: `Ready · signed in` (or `signed in with an API key`), `Not
  signed in: run claude login` (the command in a block to copy), `Not found on this machine` with
  a link to how to install it, `Off: sessions that run keep running`; a warning is one faint line
  with its icon; Check again, secondary, in the section's header. It names no file of the
  configuration, no path of a login file, no environment variable. **Rules for every project**
  ("What every agent is told, in every project. A project adds its own, or uses only its own."):
  its editor, Reset to default and Save once it changed; saving says in a toast that the agents
  started from now on follow them. **Long tool results** ("A longer result reaches the agent as
  its start and its end; the agent reads the rest when it needs it."): **Cut above**, a narrow
  select, 4,000 to 32,000 characters, 8,000 by default, or Off ("Every result goes to the agent
  whole."; `tools.md`, "Long results"), part of the core and not of an extension, which moves to
  Usage's savings when Usage comes. **Sessions**, with no sentence: a switch, "End inactive
  sessions" ("Ends a session idle for the days below. One with work waiting is never ended."),
  off by default; switched on it chooses 7 days and **After**, a select of the days, unfolds
  under it: 1, 3, 7, 14, 30, 60 or 90, and what a file edited by hand says (`sessions.set`,
  §5.7); under it a switch, "Resume when the quota resets" ("A session stopped by its quota goes
  on at the reset. Each session can change it in its composer."), off by default, §5.9.
  **Approvals** holds the **Auto judge**, drawn as a feature's page (`reference/gpui.md`, rule
  14): its switch, "Let a model decide in Auto" ("The rules let through what is known to be harmless;
  the model allows the rest or asks you. It never denies."), its usage line ("Used by sessions in
  **Auto**, chosen in each session's composer. Base calls always come to you."), then its model
  (`ModelChoice`, rule 15) and the line "Each decision is a turn of this model, on your own
  quota." The switch off is "No judge" (`judge = "none"`, §7.1): no entry of the select says it.
  A judge Flottille picked is said once in a notice, with Open Settings, which opens this tab,
  and Dismiss. Under it, **What the judge is told** ("Added to Flottille's own instructions,
  which always ask before anything destructive."): its editor, Reset to default and Save once it
  changed; a link to the guidance opens this tab with the guidance in view.
  **Features** is the list of the parts of Flottille the user turns on or off (`Subpage`, rule
  13): Specs, Review, Compaction, then Connections and later the handbook, one row each with its
  icon, its state as a tag (`On`, `Off`, `2 on`) and one line saying what it is for; a click opens
  its page in place of the list, under the path `Features / Review` (`docs/extensions.md`, "In
  the window").
  **Vault** holds the entries (`vault.md`).
  **General** holds **Appearance**, with no sentence: **Colours**, Omarchy or Flottille, and for
  Flottille **Shade**, Light or Dark, unfolding under it, each a segmented choice; off an Omarchy
  desktop the Omarchy choice is shown disabled, its tooltip saying why. Then **Workspaces folder**
  ("Where new workspaces are made. A project can choose its own."): **Where**, radios `In
  Flottille's data folder`, `Beside each project` and `A folder you choose`, each line the folder
  it gives relative to `~`, and the **Folder** field unfolding under the last (`docs/projects.md`,
  "Where the workspaces go"). Later **Machines** (§12) comes under it. Last **Developer mode**,
  with no sentence: a switch, "Log what Flottille decides" ("Why each call was allowed, held or
  refused, one JSON line each, secrets masked. For working on Flottille."), off by default
  (`developer.md`), and the path of its log unfolding under it;
- a session page: the root's conversation, live (messages, the user's own as selectable as the agent's, thoughts collapsed (their head says how long the thinking lasted, from the event before it
  to the one after it, and no time when it lasted less than a tenth of a second), tool calls with
  their result (a result cut at the source says so on its step, with the number of characters left out), the children launched, a session started for the user as one line, "started session <title>" (it is no sub-agent), reports and an edit's diff, each folded to one line until a click unfolds it), one compact line for each command a
  tool call ran (an icon, the line, how it ended, how long: a click opens the output panel the
  rail opens, and the output itself is only there; the rail's own row for a command of the root
  agent does not name it, the conversation being the page it is on), one line for each request that reached the
  user, neutral ("Request #1 · Run pnpm test", which lights its card in the rail, then, once
  answered, the outcome), each picture sent inside the bubble of its message, above its words, a
  message another session sent with `session_send` marked with its sender ("from <session>") and
  folded to its first line until a click unfolds it (the card keeps one width while it folds and
  unfolds), and
  the same message in the sender's conversation marked with whom it went to, never as the user's;
  in both, the session's name is a link that opens it (an ended session too; a session that no
  longer exists is named with no link);
  a hover on a message (the user's, the agent's, another session's) shows, under it, its date and
  time and a Copy icon button (a toast says it is copied); each step the Auto judge let through
  says "allowed by the judge" (a ticket or a pull request created included), and the folded line
  of its steps says it too; a message the user sent while the root works stays under the
  conversation, above the composer, drawn as waiting (quieter, with its pictures as small
  thumbnails, and one short line, "Queued · after this step", whose tooltip says "Sent when the
  agent finishes its step"), until the root reads it between its turns and it is a message like
  the others (the queued deliveries of `agent.tree`), with three buttons while it waits: Send now (stops the
  root's turn and hands it this message next), Edit (takes it back into the composer, text and
  pictures, after what is typed) and Cancel (takes it back); the daemon checks that the root has
  not read it in one transaction, and says "already read by the agent" in a toast if it has; and a composer that writes to the root (it carries 20 pictures at most; a refusal, of a picture or of a message, is a toast and never changes the composer's height; what appears in it unfolds and what leaves folds away), with the session's mode
  (an Ask | Auto | Full control, with a warning icon when Auto has no judge, its highlight in the
  alarm colour on Full, each step Full let through saying "allowed by Full mode") in its bar, whose Stop
  button, in Send's place while a turn runs and Send has nothing to send (the two cross-fade), and
  `Escape` interrupt the turn that runs (the agent stays), which grows with its lines up to
  about ten and then scrolls, and takes pictures (pasted from the clipboard, dropped from a file or
  picked with Attach: thumbnails with a remove button, sent with the message as ACP image blocks,
  stored in Flottille's data folder so the conversation shows them again; said in one sentence and
  left out when the session's agent takes none). A click on a picture, in a message or in the
  composer, opens it in the large modal: at its own size, scaled down to fit the window and never
  up, with the arrows (keys and buttons, "2 of 3") through the message's pictures; what the root does while a turn runs (`Thinking…`,
  `Writing…`, `Running <tool>…`) is said in the same bar, where the words cross-fade as they change,
  and goes with the turn. A running agent is
  drawn by its icon alone, in the tree and in the rail: no text badge, nothing in the header. In the
  composer, a word at the cursor that starts with `/`, anywhere in the message, lists the project's
  commands, filtered as the user types: picking one completes that word into `/name `, and sending
  it sends the commands' instructions with the message as it was written (a `/word` that names none
  is sent as text); the message and the queued message show what the user typed, each command as a
  chip, what the agent received in a fold; the header says the session's title and its place,
  and no state; a rail with, from its top, the session's tree of agents (each row with the share
  of its context the agent used, a percentage beside a thin bar, on one line; the session's own
  agent says only what it runs on, the others their name over what they run on and do; the composer's bar does not say
  it; under the composer's field, the provider is named and not changeable, and the model and
  effort pickers are the start composer's own: a change chosen while a turn runs waits for its end,
  and a dot on its picker says it until the agent took it, its menu saying when it applies,
  nothing under the bar), its workspace (labelled lines: each repository with its changes, branch, commits ahead,
  files not committed and setup steps played, its pull request, its ticket; one sentence for the
  repositories with no forge; its actions in a "⋯" menu, the same as its place's on the project page, with
  Open in editor and, for a session in a workspace (one in the project's folder is offered Open in editor only), Link a ticket… (Change the ticket… once one is linked), Unlink the ticket (only while one is linked) and Remove workspace…, which
  asks as the project page does, the dialog refusing it, with the sessions named, while sessions not ended work there; while it runs the section says it is
  being removed, and once removed it folds away, the page staying) and facts, its runs, then, pinned at the very bottom and
  folding away when nothing waits, "Waiting for you", the one place of the page that says the user
  is needed (each request that reached the user is a compact card: the whole call, wrapping over
  as many lines as it takes, its class
  and the judge's reason, or the first reason it was held, on one line, "Details" unfolding the
  rest (why it was held, one sentence for each reason, and for a base card the sentence that says
  why it always comes back), and Deny and Allow; a base card stands apart, in the alarm colour;
  an answered card leaves). The
  root's row menu ("⋯") holds End session, the only way the window ends an agent: it ends the root
  and its tree, after a confirmation that counts the agents that end. An ended session leaves its
  place in the sidebar for the "Ended" group, folded, pinned at the bottom of the sidebar
  whatever the tree's length and unfolding upward over it (its rows say their project). Unfolded
  it takes at most eight rows, or 35% of the window's height when that is less, and its rows
  scroll inside it, the most recently ended first; and its
  page shows,
  in place of the composer, why it ended and Resume when its agent can resume or load its session
  (`agent.resume`), enabled once its process is gone, which brings the composer back; that bar is
  where it says it ended, not its header, and its conversation says it ended, then resumed. A
  failed root whose session is known shows Resume too. A turn that failed for a reason that
  leaves the session usable (an expired login, `docs/agents.md`, "A failed turn") ends nothing:
  the composer stays and the state line above its field says it once (`Claude's login expired:
  sign in again (run claude and /login), then send your message again`), fading in, and out once
  a turn runs again. Selecting a child shows its
  conversation read-only;
- the runs of the agents (`docs/tools.md`, "Tasks and runs"): the rail's **Runs** section lists
  the **task** runs of the agent shown (the root by default, the child once the user opens it from
  the Agents list; a run belongs to the agent that started it), and the tasks the user ran in the
  session's place, whichever agent is shown (it is not called "Commands", which
  are the project's `/name` instructions), running first, each with its line, who started it (the
  agent, and the task), how long, its last output line and Stop, then the last five that ended,
  greyed, folded under `N ended` (unfolded by a click, as the Agents' `N ended`), with how they
  ended. A one-off line (`run {"line": …}`) is not listed, running or ended: its card in the
  conversation is where it is seen, and the section folds away when there is no task run and the
  project has no task. Its title holds **Run a task**, which opens the list of the project's tasks;
  picking one runs it as the user, in the session's workspace (or the project folder), without the
  gate (`task.run`), and its row unfolds into the section. A child
  with task runs still going says so on its row in the Agents list (`2 runs`), and a click opens
  that child with its Runs section; a click on a run opens its drawer (the same on the project
  page, where each place lists the task runs going on in it): its command, its
  task, where it runs, who started it (a link to the agent in its session), since when or how it
  ended, its output, live and following its end, Copy, Restart (the daemon's `run.restart`: it
  stops the command if it runs and starts the same line again in the same place; the drawer, and
  the modal if it is open, keep the old command drawn, Stop and Restart faded, until the new run's
  row and output arrive and replace it in place; a refusal is one toast) and Stop while it runs; who started it is cut to two lines there; its
  **Expand** button opens the same command in a large modal over the window (the rest darkened; a
  click outside, `Escape` or its close button close it and leave the drawer as it was): the
  header with Restart and Stop, the facts on one or two lines, and the output taking all the
  height left, following its end; `Escape` closes the drawer,
  and the conversation's line for that run opens the same drawer. A
  project, and a session, with a command running show a small icon in the left tree, so that no
  server is forgotten. The conversation draws a run as one line, its ANSI colours turned into the
  palette's in the panel (the stored output keeps the program's bytes, masked); a call that came
  back through an approval is the same line with "allowed by you", never the daemon's raw text;
- narrow and portrait windows: the rail, then the left tree, become drawers.
- a history of the pages shown (project pages, sessions with the agent shown in them, settings),
  as a browser keeps one: it lives as long as the window and is not stored. The mouse's back and
  forward buttons and `Alt+Left` / `Alt+Right` move in it; an open list, modal or drawer takes
  the button first and closes; a page that no longer exists is skipped.

Motion: **everything that moves is animated**, with the grammar of `reference/gpui.md` (ease-out
cubic, the durations of the kit's `src/motion.rs`, `gpui-design-system`: hover 140 ms, press 90 ms,
an entrance 240 ms, a fold or a state 280 ms; reduced motion honoured). Hover and press on every
interactive element; a state that changes (selected, chosen, focused, open); an element or a block
that appears or leaves, with the gap around it; a row inserted into, removed from or moved in a
list; a fold; a panel or a menu that opens or closes; a page and its sections, one after the other
(at most eight waits, whatever the length). At rest nothing moves and nothing is redrawn. An issue
about the window without its animations is incomplete.

Colours: the user chooses them in Settings, General, Appearance, and the daemon keeps the choice
(`appearance.toml`, `docs/protocol.md`) for every window and a restart: **Omarchy**, the current
Omarchy theme's colours followed live, or **Flottille**, Flottille's own palette, **Light** or
**Dark**, even on an Omarchy desktop. Until the user chooses, Omarchy when an Omarchy theme is
found, Flottille Dark otherwise; Omarchy chosen and no theme that can be used, Flottille in its
shade. A change applies at once: the colours cross-fade, as when the Omarchy theme changes. The
light palette has the dark one's roles, its surfaces a step toward the ink from a near-white page
(never white cards), every text at WCAG AA on each surface it is drawn on. Only the keys of a theme file that name a colour are read: another string (`mode = "light"`, a name) is ignored, a colour key with a value that is not a colour refuses the theme. A light theme
gets a light palette: the body and the bars are shaded toward the foreground (below the background in lightness), and
the cards toward it too, never to white.
Shapes, type and motion do not change with the theme. A state is drawn the same
everywhere, by shape as well as colour.

Rules from `reference/gpui.md` apply from day one: a view per moving part, the reply field in its
own view, the growing conversation as a list that does not rebuild what did not change.

## 11. Desktop integration

Through the CLI only: `flottille status --json` (counts of agents running, waiting for the user,
failed), `flottille watch` (events), `flottille open <agent>` (focus the window on it). A desktop
shell shows a badge or a notification from these. Nothing of any desktop lives in this
repository, but the generic XDG entry that puts Flottille in an application launcher with its icon
(`docs/releases.md`, "The launcher entry"), and the one window a second `flottille window` brings
to the front.

## 12. Later: remote

Designed for, not built before the milestone "Remote": the daemon's protocol over an authenticated network
transport (for example on a private network such as Tailscale), a light client on a phone, and
sending work to another machine running Flottille (its daemon owns the agents there; the tree
spans machines through deliveries).

## 13. Milestones

The plan is cut into GitHub milestones; each ends on something the user can use.

1. **Usable from the CLI**: projects, multi-repository workspaces with their setup, Claude agents in
   bare mode over ACP, the generic tools, the gate with approvals, the tree with deliveries,
   reports and waking, the pilot door, the CLI and events. Accepted when the user runs a real
   piece of work on a multi-repository project from a terminal: a lead launches two workers in
   their own workspaces, one hits an approval, one gets stuck or fails, the lead is told each
   time without anyone polling, and the user follows every agent and answers what reaches them
   (the root's messages and the requests forwarded up to it) from the CLI.
2. **The window**: the same piece of work followed and answered in the window.
3. **More agents**: Codex and OpenCode in bare mode.
4. **Forges and tickets**: GitHub and Bitbucket (pull requests, checks, pipelines),
   issue trackers (GitHub issues, Jira), linked to workspaces; commands (open the pull
   requests of a workspace, merge a stack in dependency order, close a workspace). Reading a workspace's GitHub pull requests and checks is in
   [`forges.md`](forges.md), and its ticket (a GitHub issue or a Jira work item) in
   [`tickets.md`](tickets.md); every pull request and ticket of a project, kept up to date by the
   daemon, in [`items.md`](items.md).
5. **Remote**: network transport, phone, other machines.

Flottille's own features beyond its core (the Auto judge, the compaction, the agent's own handoff
included, built in today (§7.1, §5.8), later specs, review and a handbook) are proposed to become extensions, declared and turned on by the user: a proposal,
nothing built, in [`extensions.md`](extensions.md).

## 14. Defaults taken, to confirm

- Data in XDG folders; workspaces under the data folder unless the project says otherwise.
- Children cap 12 per project, no depth, quiet interval 20 min.
- Claude reached through Flottille's own bridge; `claude` checked at a pinned version, another one
  only warned about.
- A session the user starts starts in Auto unless the user chose its mode in the composer that
  starts it, which is back on Auto after each start, and a tree has one mode, its root's, read at
  each call; Auto without a judge asks the user as Ask does. Only the user chooses Full, at a
  session's start or later; a session an agent starts takes its launcher's mode, Full included. A user who never chose a judge gets the smallest model
  of the first provider installed and signed in, among those listed (Claude's `haiku`); one that is
  not listed gets nothing picked.
- One judge process per judgement, stopped after it, rather than one kept warm: nothing of a
  judgement is kept for the next, and no process waits idle on the user's quota.
- A judge silent for 30 seconds, or whose answer is not the JSON verdict, asks the user.
- Commits and pushes are calls to judge; a forced push, even with a lease, and a rewrite of
  history on a remote are base.
