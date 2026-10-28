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
because of it, are in [`reference/mcp.md`](reference/mcp.md), and what is known about the window
toolkit is in [`reference/gpui.md`](reference/gpui.md).

## 1. What Flottille is, and what it is not

**It is:**

- A **daemon** that owns projects, workspaces (worktrees), agent processes, their conversations
  and their tree, and everything they asked for.
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
| Workspace | The place one piece of work happens: one git worktree per repository it takes, on one branch name, in one folder, prepared by the project's recipe. |
| Recipe | The project's ordered preparation steps for a new workspace: copy, link, run, plus the workspace's variables. |
| Command catalogue | The project's named commands (`api:dev`, `test`, `lint`…), runnable by the user and by agents. |
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
  approvals, deliveries, sessions, projects, workspaces, steps, command runs.
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
(`providers.toml`), and each agent runs on one for life (`docs/agents.md`, "Providers").

Handshake, session open order (resume, else load, else new), model/effort/mode options, event
mapping, cancel, idle release of processes and death handling follow `reference/hemera.md` §1.

Flottille runs the agent CLIs the user installed and signed in to; it provides no login of its
own and never reads or copies credentials.

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
- commands: `commands_list`, `commands_run` (catalogue command or one line, **never a shell**),
  `commands_status` (a run's output, or its stop);
- forges (an agent in a workspace): `forge_read`, the pull requests of its workspace's
  repositories and their checks, read only (`forges.md`), and `forge_write`, which opens or merges
  them or re-runs a failed pipeline, listed only to an agent whose procedure run grants one of
  those (`procedures.md`);
- the workspace (an agent in a workspace): `workspace_remove`, which removes the agent's own
  workspace once nothing in it would be lost, listed only to an agent whose procedure run grants
  it;
- references: `reference_read`, one of the project's references, documents read when relevant
  (`docs/projects.md`, "References");
- tickets (an agent in a workspace): `ticket_read`, the ticket its workspace is linked to, or
  another of the same tracker, read only (`tickets.md`);

Flottille's own tools:

- `agent_launch {title, brief, model?, role?, workspace? | new_workspace? + repositories?}`, the parent's place when neither:
  starts a child, answers at once with its id. Refused (never queued) above the caps.
- `agent_send {agent_id, message}`: a message to one of the caller's children.
- `agent_status {agent_id?, action?, detail?, after?}`: for one child or all of them: state, what
  it is doing now (thinking, in a tool call, which command runs and for how long, waiting for
  approval #n, waiting on the provider), time since its last sign of activity, context use; the
  last few messages for one child, or for all with `detail: full`. `action: conversation`: a
  page of the child's conversation.
- `agent_stop {agent_id}`: stops the child and its own tree.
- `report {text}`: the caller is done; its report goes to its parent at once and the session ends
  (see 5.3). A root agent's report goes to the user.
- `ask_user {question}` (a root only): a question to the user, answered later as a delivery
  (never blocks).
- `ask_parent {question}` (a child only, in place of `ask_user`): a question to its parent,
  delivered as `child-question`; the parent answers with `agent_send`.
- `approval_answer {request_id, decision: forward | deny, why}`: answer a request held for the
  caller by one of its children (§7). `forward` passes it to the caller's parent, or to the user
  when the caller is a root. There is no way to allow a request.

The tool list of a session is fixed for its whole life (prompt cache, `reference/hemera.md` §7)
and depends on what the agent can do, known when it starts: a root gets `ask_user`, a child
`ask_parent`; an agent at the depth limit gets no `agent_*` tools and no `approval_answer`. A lead
has about 16 tools, a leaf worker 11, two more each in a workspace (`forge_read`, `ticket_read`),
and `forge_write` or `workspace_remove` for an agent whose procedure run grants what they do
(`reference/mcp.md` §4).

### 4.4 Roles

Two roles at first, with the tool lists of 4.3:

- **lead**: the role of an agent the user starts. Its instructions carry the orchestration
  standard (`reference/orchestration.md`): when to launch a child, which model, one precise task
  per child, the brief, how to read a report, the caps.
- **worker**: the role of a child. Its instructions say: do the brief and only it, keep going
  without waiting for answers, end with `report`.

A worker may launch its own children (depth limit, 5.1). Roles are data (a template and a few
flags), so more can be added without code changes.

## 5. Parenting and waking (the core of Flottille)

The failure Flottille exists to fix: a lead launches workers, loses track of them, and work sits
still for hours because nobody knows what is happening. In Flottille a parent cannot lose its
children: they are tied to it, Flottille tells it everything that matters, and wakes it when
something happens or when nothing does.

### 5.1 The tree

- Every agent launched through `agent_launch` has a parent (its lineage, stable across
  replacements) and a depth. Root agents are started by the user (window, CLI, pilot door).
- **Caps**: children running at once per project (default 3, configurable 1 to 6), and depth
  (default 2: a lead's worker may have workers, theirs may not). Above a cap, `agent_launch` is
  refused with a sentence the agent can act on; it never queues silently.
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

### 5.3 What the parent is told

| Event of a child | Delivery to the parent |
|---|---|
| calls `report` | `child-report`: the report text, the child's workspace and branch, its last commit per repository. Urgent. |
| ends a turn without reporting | Flottille first sends the child one reminder ("end with `report`…"); a second turn without a report tells the parent `child-idle`, with the child's last message. |
| fails (agent error, provider limit, replacement limit) | `child-failed`, with the reason and the last output lines. Urgent. |
| is stuck (no sign of activity for 5 min in a turn, not waiting on the provider, no command of its own running) | `child-stuck`; Flottille replaces it once, and says so. |
| has a call held by the gate (an "ask") | `child-asks`, with the request number, the call and the reasons. Urgent. The parent answers with `approval_answer` (forward or deny, §7). |
| calls `ask_parent` | `child-question`, with the question. The parent answers with `agent_send`. |
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
  chosen set), variables, command catalogue, recipe, "never" list, children cap.
- The project configuration is a TOML file Flottille owns (in its config folder), editable by
  hand and from the window. Nothing is written into the user's repositories.

### 6.2 Workspaces

Creation, the up-to-date base per repository (fetch of one branch into its tracking ref, with
freshness), `git worktree add`, preparation in the background with resume, failure capture and
the git invocation rules follow `reference/hemera.md` §6. Additions over that model, all required
by `reference/multi-repo-case.md`:

- **Repository set**: all of the project's repositories by default; the project can say so
  ("environment scope is not change scope").
- **Root entries**: when the project folder is not itself a repository, its other top-level
  entries (instruction files, shared folders) can be linked into the workspace folder by one
  recipe step, never `.git` nor `.gitignore`.
- **Ordered per repository**: `run` steps can be declared once "for each repository, in
  dependency order" (leaves first), with `{repository}` in the template.
- **Variables replace shell hooks**: workspace variables (templated, for example
  `PACKAGE_STORE={workspace.path}/.store`, `COMPOSE_PROJECT_NAME=app-{workspace}`, a `PATH`
  prefix) are injected into every command run in the workspace and into the agent's command
  runs. No `.envrc`, no direnv.
- **Copy of ignored files**: `copy` steps take globs (`.env`, `.env.*`), per repository, never
  overwrite.
- **Removal** refuses a worktree with uncommitted files **or commits not on any remote**
  (`git log HEAD --not --remotes`), names them, and never forces.

Agents are placed in a workspace (writable) or in the main checkout (read-only by default).

## 7. The gate

Every tool call goes through one gate, as in `reference/hemera.md` §4, minus the remote judge:
grant checks, tool admitted for the role, arguments decoded, place rules, then the verdict:
deny (the project's never list; deleting `.git`; for agents by default: `git push`, history
rewrites, branch deletion, forge writes, publishing), ask (sensitive places such as `.env`
files and credential folders, commands marked "ask before running", anything outside the place),
or allow (reads, catalogue commands, writes in the place).

An "ask" writes a numbered request (the frozen call, the reasons, the agent that asked) and is
answered to the agent at once ("request #n is waiting…"). A tool call never waits for a human or
an agent. Who holds the request depends on who asked:

- **A root's request is the user's.** It appears in the window, in `flottille approvals`, and in
  the event stream. The user answers deny, allow once, or allow for the root's tree (an exact
  identity grant, never for a sensitive place).
- **A child's request is held by its parent**, as a `child-asks` delivery. The parent denies it or
  forwards it with one sentence of why (`approval_answer`); forwarding passes it to the parent's own parent, one level at a time, and from the root to the user, with
  every reason given on the way. **No agent can allow a request**: the gate's "ask" always ends
  with the user or with a refusal. A request held by an agent is not in `flottille approvals` and
  the user cannot answer it until it is forwarded.

An allowed request is re-checked and performed, and its result comes back to the agent that asked
as an `approval` delivery; a denied one comes back as a refusal with the why.

Agent-run commands get an environment without forge tokens and with `git push` neutralised
(`reference/hemera.md` §3).

**Procedures** are the one way an agent commits, pushes or writes to a forge: a run of a
procedure grants these, from a fixed vocabulary, `allow` or `ask`, to the agents of that run only,
in its workspace, until it ends; the user confirms the grants before it starts. Outside a run the
gate decides as above, and some things stay refused even inside one (`git stash`, `git reset
--hard`, a push forced without a lease). See [`procedures.md`](procedures.md).

## 8. Instructions

Built as in `reference/hemera.md` §7: a shared base, the role template, a cache boundary, the
per-session part, then the project layer, frozen per session. The project layer holds, per
repository and for the project folder: `CLAUDE.md` or `AGENTS.md` (the agent's own name first),
**and the local files `CLAUDE.local.md` / `AGENTS.local.md` when present** (they hold rules the
user keeps out of the repository).

After the per-session part, before the project layer, comes the list of the project's **references** (documents an agent reads
with the tool `reference_read`: what knowledge skills hold today): a name and one line each, never
the content (`docs/projects.md`).

Later (milestone "Forges and tickets"), a project gains **procedures** (named multi-step actions
with their own scoped authorizations: what slash commands hold today, [`procedures.md`](procedures.md)), see
`reference/multi-repo-case.md`. A procedure's instructions are its run's first message, after
what it grants.

## 9. The built-in orchestration standard

`reference/orchestration.md` is today's manual way of running agents that hand work to each
other. Flottille turns it into the default behaviour: the lead's instructions carry its rules
(one precise task per child, lowest safe model, the brief's content, how to read a report, never
merge without the checks), and Flottille itself provides what was done by hand: the run folder
becomes the child's record, the report is a tool, waiting is deliveries, closing a child is
`agent_stop`, the caps are enforced.

## 10. The window

GPUI Fast with gpui-kit, English only (no translation layer). Screens of the milestone "The window":

- a top bar with search and a bell counting what waits for the user;
- left: the tree project › place › session. `main` and the workspaces sit at the same level
  under their project; sessions sit under their place, with a mark when running or when the
  root needs the user; a folded level shows the strongest mark it holds;
- a project page: its repositories, its places with their sessions, creating and removing a
  workspace (with the preparation's progress and failure output), and the composer that starts
  a session (place, agent, model, first message); its procedures (add from a template or blank,
  edit with the grants as `allow` / `ask`, remove) and its references (add with a path picked in
  the project folder, remove); "Run procedure…" on a workspace, which shows the grants in words
  (`allow` runs without asking, `ask` is held for the user's answer) and starts nothing before
  Confirm;
- a session page: the root's conversation, live (messages, thoughts collapsed, tool calls with
  their result, the children launched, reports, diffs), the approvals that reach the user, inline,
  with deny / allow for the tree / allow once, and a composer that writes to the root, whose Stop
  button and `Escape` interrupt the turn that runs (the agent stays); a rail with the session's
  tree of agents, its workspace (each repository's forge as the daemon reads it, its ticket,
  linked or unlinked there) and facts (the grants of the procedure it runs, while it runs). The
  header and the root's row menu stop the root and its tree, after a confirmation that counts the agents that stop. Selecting a child shows its
  conversation read-only;
- narrow and portrait windows: the rail, then the left tree, become drawers.
- a history of the pages shown (project pages, sessions with the agent shown in them, settings),
  as a browser keeps one: it lives as long as the window and is not stored. The mouse's back and
  forward buttons and `Alt+Left` / `Alt+Right` move in it; an open list, modal or drawer takes
  the button first and closes; a page that no longer exists is skipped.

Colours: Flottille's own palette, or, on an Omarchy desktop, the current Omarchy theme's colours
followed live. Shapes, type and motion do not change with the theme. A state is drawn the same
everywhere, by shape as well as colour.

Rules from `reference/gpui.md` apply from day one: a view per moving part, the reply field in its
own view, the growing conversation as a list that does not rebuild what did not change.

## 11. Desktop integration

Through the CLI only: `flottille status --json` (counts of agents running, waiting for the user,
failed), `flottille watch` (events), `flottille open <agent>` (focus the window on it). A desktop
shell shows a badge or a notification from these. Nothing of any desktop lives in this
repository.

## 12. Later: remote

Designed for, not built before the milestone "Remote": the daemon's protocol over an authenticated network
transport (for example on a private network such as Tailscale), a light client on a phone, and
sending work to another machine running Flottille (its daemon owns the agents there; the tree
spans machines through deliveries).

## 13. Milestones

The plan is cut into GitHub milestones; each ends on something the user can use.

1. **Usable from the CLI**: projects, multi-repository workspaces with recipes, Claude agents in
   bare mode over ACP, the generic tools, the gate with approvals, the tree with deliveries,
   reports and waking, the pilot door, the CLI and events. Accepted when the user runs a real
   piece of work on a multi-repository project from a terminal: a lead launches two workers in
   their own workspaces, one hits an approval, one gets stuck or fails, the lead is told each
   time without anyone polling, and the user follows every agent and answers what reaches them
   (the root's messages and the requests forwarded up to it) from the CLI.
2. **The window**: the same piece of work followed and answered in the window.
3. **More agents**: Codex and OpenCode in bare mode.
4. **Forges and tickets**: GitHub and Bitbucket (pull requests, checks, pipelines),
   issue trackers (GitHub issues, Jira read-only), linked to workspaces; procedures with scoped
   authorizations (open the pull requests of a workspace, merge a stack in dependency order,
   close a workspace). Reading a workspace's GitHub pull requests and checks is in
   [`forges.md`](forges.md), and its ticket (a GitHub issue or a Jira work item) in
   [`tickets.md`](tickets.md).
5. **Remote**: network transport, phone, other machines.

## 14. Defaults taken, to confirm

- Data in XDG folders; workspaces under the data folder unless the project says otherwise.
- Children cap 3 per project, depth 2, quiet interval 20 min.
- Claude reached through Flottille's own bridge; `claude` checked at a pinned version, another one
  only warned about.
- No commit rights for agents unless a procedure's run grants them; push refused for agents
  outside such a run (`procedures.md`).
