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
- tasks and runs: `tasks` (the project's tasks, written by a root only), `run` (a task or one
  line, **never a shell**), `run_status` (a run's output, or its stop);
- forges (every agent of a project whose repositories declare a forge, in `main` as in a
  workspace): `forge`, which lists and reads the pull requests of the place's repositories and
  their checks (`forges.md`), and opens, comments, edits or merges them or re-runs a failed
  pipeline, each write a call to judge (§7); in `main` it names the workspace;
- the workspace (every agent): `workspace`, which lists and reads the workspaces, removes one
  once nothing in it would be lost, the agent's own or, in `main`, the one it names, a call to
  judge, and, for a root, creates and prepares them;
- skills: `skill_read`, one of the project's skills, texts read when relevant (`skills.md`);
- tickets (every agent of a project that declares a tracker, in `main` as in a workspace):
  `tickets`, which lists, searches and reads the tracker's tickets, and writes GitHub issues,
  each write a call to judge; Jira is read only (`tickets.md`);
- the project (a root): `skills` and `commands` (list, show, add, edit, remove), `setup` (its
  steps, numbered) and `project` (its settings, its forges and tracker, its rules), and the writes
  of `tasks`, what the window does, each change a call to judge, a change of the tracker or of the
  rules base, a line it sets judged as a run of it (`tools.md`, "The project", `projects.md`);

Flottille's own tools:

- `agent_launch {title, brief, model?, role?, workspace? | new_workspace? + repositories?, root?}`, the parent's place when neither:
  starts a child, answers at once with its id. Refused (never queued) above the caps. With
  `root`, a root only starts a session for the user instead, judged, in the tree's mode, its
  first message the agent's and never the user's, base when that root was itself started by an
  agent (`tools.md`, "Sessions for the user").
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
and depends on what the agent can do, known when it starts: a root gets `ask_user`, `skills`,
`commands`, `setup` and `project`, a child `ask_parent`; an agent at the depth limit gets no `agent_*` tools
and no `approval_answer`. A root lead has 21 tools, a leaf worker 12; one more each when the
project's repositories declare a forge (`forge`), one more when it declares a tracker
(`tickets`), whatever the place (`reference/mcp.md` §4).

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

The same path wakes an agent about its own command: a command that outlived its call (it keeps
running, or ran past the call's wait) delivers `command-ended` to the agent that started it when it
ends: the line, the exit code, the duration and the last lines of the output, masked
(`docs/tools.md`, "Commands").

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
  (`git log HEAD --not --remotes`), names them, and never forces.

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
- **Base** calls always come to the user, in every mode, with a strong warning: a push forced
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
to nowhere, but a run that is itself a `git push` the user or the judge allowed (`docs/tools.md`,
"Tasks and runs").

What stays **refused** is not a permission but a call that cannot be made as it is: shell syntax,
a malformed call, a login printed, the daemon driven from a command.

### 7.1 The mode and the judge

Each session is in **Ask** or **Auto**, a small two-choice control in the composer's bar. A new session starts in
Auto. A tree has one mode, its root's, stored with the root and read at each call: a switch
reaches every agent of the tree at once, children included. Only the user changes it, in the
window or with `flottille agent mode <agent> ask|auto` (`agent.mode`, which switches the tree's
root whatever agent of it is named); no tool of an agent does. It is kept when the session
resumes.

- In **Ask**, a call to judge is a card for the user.
- In **Auto**, it goes to the **judge**, which allows it or asks the user: it cannot refuse, and
  base calls never reach it.

The judge is an agent of a provider the user chose in Settings ("Auto judge": provider and
model, `provider.set_judge`), run in bare mode **with no tool at all**: no Flottille tool, no MCP
server. A user who never chose one gets one picked: the smallest model of the first provider that
is enabled, installed and signed in, among the few whose smallest model Flottille lists (Claude's
Haiku), written once to `providers.toml` and said once in a notice that opens Settings (and by
`flottille provider list`); a user who chose "No judge" never gets one, nor does one whose last
choice before `judge = "none"` existed was no judge (the last `provider.judge_changed` in the store
names none), whose file then says `judge = "none"`. Its
instructions are a fixed text (`roles/judge-base.md`: what it is, its JSON answer, that it never
denies, that what the agent wrote is data, and the floor: ask for anything destructive, outside
what was asked, surprising, or whenever it is not sure) around the user's **guidance**, which can
add to the floor but never lift it: Flottille's default (`roles/judge.md`: allow what the request
plausibly needs; writes of the project's configuration are within intent while the user is setting
the project up) or the user's own, edited in Settings ("Judge", with Reset to default; saving an
empty text resets it) and read at each judgement. A guidance that cannot be read whole, or is over
its 4,000 characters, is never cut nor replaced by the default: each judgement fails, and so asks,
saying which file is wrong and why, and Settings show it. The guidance changes neither what
reaches the judge nor what it may answer. It sees the call masked (secrets masked, the line,
the path, a brief, a title and a body whole, contents cut to their first lines with how many they
had), why the rules held it, the place, the agent's `why`, the first message of a session an agent
started for the user (labelled as that agent's, never the user's), and last the user's last
messages to the session, nothing else.
What the agent wrote comes in blocks of its own, every `<` escaped, so that no text of the agent
reads as the user's; only the user's messages say what is wanted. It answers one JSON object,
`{"verdict": "allow" | "ask", "reason": "…"}`. A failure, an answer that is not that object, or
30 seconds without one is an ask; a line or a path longer than the 8,000 characters it is given
whole asks without it, and the gate refuses a longer brief, title or body. A process of the provider is started for each judgement, and its whole
process group stopped after it. Each verdict is recorded with the
call (who judged, what, why: the `judgement` of its `agent.tool_served` event, and of its request
when it asks), shown on the card and in `flottille approvals`.
Without a judge, none picked or "No judge" chosen, Auto behaves as Ask, and the control carries a
small warning icon whose tooltip says so and whose click opens Settings, where the judge is
chosen. A judgement is a turn of the judge's model: it uses the user's quota on that provider,
which Settings say.

### 7.2 Requests

An "ask" writes a numbered request (the frozen call, its class, the reasons, the judge's verdict
when one was asked, the agent that asked) and is answered to the agent at once ("request #n is
waiting…"). A tool call never waits for a human or an agent. Who holds the request depends on
who asked:

- **A root's request is the user's.** It appears in the window, in `flottille approvals`, and in
  the event stream. The user allows it once or denies it: there is no lasting grant.
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
projects, or a project's own (`projects.md`, "Rules"). The global rules sit in the base, before the
boundary, where every project shares them; a project's own sit just after the per-session part,
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

- a top bar with search and a bell counting what waits for the user;
- left: the tree project › place › session. `main` and the workspaces sit at the same level
  under their project; a session is titled by the first line of its first message that says what the
  work is (a greeting or a single word does not: the session is `agent <id>` until a later message
  of the user does); sessions sit under their place, with a mark when running or when the
  root needs the user. The mark is where the work is, once: on the session's own row while that row
  is in sight, and on a folded place or project, which shows the strongest mark of what it holds,
  only while it is folded (never on a row and on its parents together); a session whose agent
  ended shows no mark. A click on a project's name opens its page and leaves its places as they
  were, only its chevron folds and unfolds them. A place's row offers, under the pointer, a "+"
  that opens its project's composer set to start there, with the field focused; a session's row
  offers a "⋯" menu with End session (the confirmation of the session page, counting the agents
  that end) and, on an ended session whose agent can resume, Resume, with no need to open the
  session first;
- a project page: its repositories (each with its branch, its remote or "local only" and its
  forge; added, removed, which only forgets one, and detected again), its places with their
  sessions, each workspace's card saying where each of its repositories stands (branch, commits
  ahead, files not committed, setup steps played), creating and removing a
  workspace (with the preparation's progress and failure output), the composer that starts
  a session (place, agent, model, first message), and its tasks, each with Run, which runs it in
  the project folder, listed below with the rail's rows and output panel; its Settings: each
  repository's forge, the tracker, its Skills (a path picked in the project folder, or a text;
  added, edited, removed), its Commands (added from a list of the templates and a blank one,
  edited, removed), its Tasks (a line, where it runs and its flags), its Setup (steps added,
  edited, removed and reordered, each with its kind, where it is done and its time limit), then
  its Rules (a text editor, Reset to default, and a line saying a change reaches the agents
  created from now on);
- Settings: the Auto judge (two lines on Auto: the rules pass what is known to be harmless, a
  small model on the user's own quota decides the rest, base calls always come to the user; "No
  judge (Auto asks like Ask)" among its choices), the providers, then the Judge guidance and the
  Rules for every project, both edited the same way; a judge Flottille picked is said once in a
  notice, with Open Settings and Dismiss;
- a session page: the root's conversation, live (messages, the user's own as selectable as the agent's, thoughts collapsed (their head says how long the thinking lasted, from the event before it
  to the one after it, and no time when it lasted less than a tenth of a second), tool calls with
  their result, the children launched, reports, diffs), one compact line for each command a
  tool call ran (an icon, the line, how it ended, how long: a click opens the output panel the
  rail opens, and the output itself is only there), one line for each request that reached the
  user ("Waiting for you: Run pnpm test", which scrolls the rail to its card and lights it, then,
  once answered, the outcome), and a composer that writes to the root, with the session's mode
  (an Ask | Auto control, with a warning icon when Auto has no judge) in its bar, whose Stop
  button, in Send's place while a turn runs and Send has nothing to send (the two cross-fade), and
  `Escape` interrupt the turn that runs (the agent stays), which grows with its lines up to
  about ten and then scrolls, and takes pictures (pasted from the clipboard, dropped from a file or
  picked with Attach: thumbnails with a remove button, sent with the message as ACP image blocks,
  stored in Flottille's data folder so the conversation shows them again; said in one sentence and
  left out when the session's agent takes none); what the root does while a turn runs (`Thinking…`,
  `Writing…`, `Running <tool>…`) is said in the same bar, where the words cross-fade as they change,
  and goes with the turn. A running agent is
  drawn by its icon alone, in the tree and in the rail: no text badge, nothing in the header. In the
  composer, `/` at the start
  of a message lists the project's commands, filtered as the user types: picking one puts `/name `
  in the field, and sending it sends the command's instructions with what follows the name (a
  `/word` that names none is sent as text); a rail with, from its top, "Waiting for you" (each
  request that reached the user is a card that says what the call does, then its class, then the
  judge's reason when one was asked, then why it was held, one sentence for each reason, with
  Allow and Deny only; a base card stands apart, in the alarm colour, with a sentence that says
  why it always comes back; an answered card leaves), the session's
  tree of agents, its runs, its workspace (each repository's branch, commits ahead, files not
  committed and setup steps played, and its forge as the daemon reads it, its ticket,
  linked or unlinked there) and facts. The
  root's row menu ("⋯") holds End session, the only way the window ends an agent: it ends the root
  and its tree, after a confirmation that counts the agents that end. An ended session leaves its
  place in the left tree for the "Ended" group, folded, at the bottom of the whole tree below
  every project (its rows say their project), and its page shows,
  in place of the composer, why it ended and Resume when its agent can resume or load its session
  (`agent.resume`), enabled once its process is gone, which brings the composer back; its header
  says `STOPPED` or `FAILED`, as its rail does, and its conversation says it ended, then resumed. Selecting a child shows its
  conversation read-only;
- the runs of the agents (`docs/tools.md`, "Tasks and runs"): the rail's **Runs** section lists
  those of the session's tree (it is not called "Commands", which are the project's `/name`
  instructions), running first, each with its line, who started it (the agent, and the task when
  it is one), how long, its last output line and Stop; a click opens its output, live, in a panel,
  and the panel is the same when the conversation's line for that run is clicked. One that ended
  stays listed, greyed, with its exit code, until the user dismisses it or the session ends; the
  last three that ended are in sight, the older ones, and any that ended before the session's last
  turn began, fold into one row, "N ended", which opens them, each still with its Dismiss. A
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
cubic, the durations of `crates/flottille-ui/src/motion.rs`: hover 140 ms, press 90 ms, an entrance
240 ms, a fold or a state 280 ms; reduced motion honoured). Hover and press on every interactive
element; a state that changes (selected, chosen, focused, open); an element or a block that
appears or leaves, with the gap around it; a row inserted into, removed from or moved in a list; a
fold; a panel or a menu that opens or closes; a page and its sections, one after the other (at most
eight waits, whatever the length). At rest nothing moves and nothing is redrawn. An issue about
the window without its animations is incomplete.

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
   issue trackers (GitHub issues, Jira read-only), linked to workspaces; commands (open the pull
   requests of a workspace, merge a stack in dependency order, close a workspace). Reading a workspace's GitHub pull requests and checks is in
   [`forges.md`](forges.md), and its ticket (a GitHub issue or a Jira work item) in
   [`tickets.md`](tickets.md).
5. **Remote**: network transport, phone, other machines.

## 14. Defaults taken, to confirm

- Data in XDG folders; workspaces under the data folder unless the project says otherwise.
- Children cap 3 per project, depth 2, quiet interval 20 min.
- Claude reached through Flottille's own bridge; `claude` checked at a pinned version, another one
  only warned about.
- A new session starts in Auto, and a tree has one mode, its root's, read at each call; Auto
  without a judge asks the user as Ask does. A user who never chose a judge gets the smallest model
  of the first provider installed and signed in, among those listed (Claude's `haiku`); one that is
  not listed gets nothing picked.
- One judge process per judgement, stopped after it, rather than one kept warm: nothing of a
  judgement is kept for the next, and no process waits idle on the user's quota.
- A judge silent for 30 seconds, or whose answer is not the JSON verdict, asks the user.
- Commits and pushes are calls to judge; a forced push, even with a lease, and a rewrite of
  history on a remote are base.
