# Agents

An agent is one agent session, run over ACP (`docs/design.md` §4): a Claude Code adapter process
that Flottille starts, talks to on its stdio, and owns. It works in a place (the project folder
itself, `main`, or one of its workspaces) and runs in bare mode: no built-in tool, no user or
project setting, no skill, no plugin, no MCP server but Flottille's. It follows
`docs/reference/hemera.md` §1 and §2.

An agent can launch others (its children), which form its tree ("The tree", below), and is
woken only by deliveries. Flottille's MCP server and its tools are in `docs/tools.md`.

## Commands

| Command | |
|---|---|
| `flottille agent run <project> [--workspace <name>] [--model <model>] <message>` | Starts a Claude agent with a first message and prints its reply as it comes, until the turn ends. Fails when the agent fails or its process dies. `--json` prints the agent instead, once the turn ended. |
| `flottille agent show <agent> [--json]` | The agent and its conversation. |
| `flottille agent install` | Installs the Claude adapter at the version this Flottille runs. |
| `flottille doctor [--json]` | Whether Claude agents can run here. Fails when they cannot. |

`agent run` starts a root and follows its first turn; `agent start`, `send`, `read`, `stop`, `list`
and `tree` drive a root and read its tree, and `flottille mcp` does the same for an agent outside
Flottille: all of them are in `docs/cli.md`. The protocol has the rest (`agent.cancel`,
`docs/protocol.md`).

## What the machine needs

- **Node**, on `PATH`: the adapter runs on it.
- **The user's own `claude`**, on `PATH` and signed in by the user. Flottille provides no login of
  its own and never reads, copies or prints the user's credentials: `flottille doctor` checks
  that the login file (`$CLAUDE_CONFIG_DIR/.credentials.json`, else `~/.claude/.credentials.json`)
  exists, without opening it. On macOS the login is in the Keychain and there is no file.
- **The adapter** `@agentclientprotocol/claude-agent-acp`, at the version Flottille pins
  (0.95.0), installed by `flottille agent install`: `npm install --prefix
  $XDG_DATA_HOME/flottille/adapters/claude` of that version, without its optional packages (the
  SDK's own copy of Claude Code: the user's `claude` is the one run) and without install scripts.
  Flottille never uses `npx`.

`flottille doctor` reports each of them: Node and its version, `claude` with its path and version
(`claude --version`, given 5 s), whether the login file is there, the adapter's version beside
the pinned one. `agent.run` is refused with a sentence saying what to do while the adapter is not
installed, Node or `claude` is not on `PATH`.

## How an agent runs

- **The process**: `node <the adapter's program>` (the `bin` of its `package.json`), in the
  agent's folder, in a process group of its own, with the daemon's environment less what it has
  no business with (below), plus `CLAUDE_CODE_EXECUTABLE` set to the user's `claude`. Its stderr
  goes to the daemon's log, masked; its stdout is the protocol.
- **The handshake**: `initialize` with ACP v1, the client named `Flottille`, and no `fs` or
  `terminal` capability (the agent has no built-in file or terminal tool) nor `subagents` (no
  native sub-agent). It declares the `jetbrains.air` extension with `recommendedValue` (a
  "default" option value is named) and `sessionFailure` (provider waits and limits are reported).
- **The session**, opened with the agent's folder as `cwd`, Flottille's MCP server only, and the
  bare options in `_meta`, in this order: `session/resume` when the agent has a session id and
  the adapter resumes; else `session/load` when it loads; else `session/new`. The session id is
  kept: a process started again opens the same session.
- **The options**: the model, the effort (`thought_level`) and the mode the user chose, in this
  order, each set with `session/set_config_option` and checked: an option the agent does not
  have, a value it does not offer, or a value other than the one asked once set fails the agent,
  saying what it offers. A model is never replaced by another. What the agent took is kept.
- **A prompt** is one text block. The turn ends with the agent's stop reason (`end_turn`,
  `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`), or `interrupted` when the process
  died during it. Prompts sent while a turn runs wait their turn.
- **A cancel** sends `session/cancel` while a turn runs; the turn then ends `cancelled`.
- **The agent's own permission prompts** (`session/request_permission`) are answered at once:
  a tool of the agent's own list, named by `_meta.claudeCode.toolName`, is allowed once; anything
  else is rejected, a request with no tool name included (its title is never taken for one: the
  model can shape it). In bare mode they should almost never come.
- **Idle release**: a process with no turn running and nothing waiting for 5 minutes is let go
  (stopped: `SIGTERM` to its group, `SIGKILL` 2 s later); the next prompt starts it again and
  resumes or loads its session.
- **A death**: a process that ends without being let go is published (`agent.died`) and its
  session replaced ("Replacement", below).
- **Orphans**: each process's pid and start time are kept in the store while it runs. A daemon
  starting kills the process groups a previous one left behind (a crash), only when the group's
  leader is alive with the start time recorded: never a stranger that took the pid since, and not
  the members of a group whose leader is gone (nothing says whose they are). Then it forgets all
  the rows. An agent that was starting or working when its daemon went is idle again
  (`agent.released`, reason `restart`). Its session is kept. A working agent's prompt was sent
  and is in that session's history: its deliveries are taken as sent, never sent again, and it
  is sent a `resume` saying its turn was cut. A starting agent's were not sent yet: they are
  given back.
- **Daemon stop**: every agent's process is let go. One whose task has not finished within 4 s
  (stuck while its agent opens a session, say) has its process group killed, so that the daemon
  goes with nothing of its own running.

## Bare mode for Claude

`_meta.claudeCode.options` of `session/new`, `session/load` and `session/resume`, which the
adapter passes to the Claude Agent SDK over its own defaults:

```json
{
  "tools": [],
  "allowedTools": ["mcp__flottille__fs_read", "…"],
  "settingSources": [],
  "strictMcpConfig": true,
  "systemPrompt": {"type": "custom", "prompt": ["<shared>", "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__", "<session>"], "snapshot": true},
  "env": {
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
    "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
    "ENABLE_TOOL_SEARCH": "false",
    "MCP_TOOL_TIMEOUT": "600000",
    "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT": "600000"
  }
}
```

- `tools: []`: no built-in tool at all, so no shell, no file tool, no native sub-agent.
- `allowedTools`: the exact names of the agent's Flottille tools (never a wildcard), so that
  Claude never asks for them, each prefixed `mcp__flottille__`. The list depends on what the agent
  can do (`docs/design.md` §4.3, `docs/reference/mcp.md` §4 rule 9). Every agent has `fs_read`,
  `fs_list`, `search`, `fs_write`, `fs_edit`, `commands_list`, `commands_run`, `commands_output`,
  `commands_stop` and `report`; one that can have children (below the depth limit) also
  `agent_launch`, `agent_send`, `agent_status`, `agent_read`, `agent_stop` and
  `approval_answer`; a root `ask_user`, a child `ask_parent`. A root gets, in this order, the
  first nine, the five `agent_*`, `report`, `ask_user`, `approval_answer` (17). The list is
  frozen in the agent when it is created (the prompt cache depends on it).
- `settingSources: []`: the adapter's own default loads the user's settings, hooks, plugins,
  skills and instruction files; this overrides it.
- `strictMcpConfig`: only the MCP servers given with the session.
- `systemPrompt`: the agent's instructions, frozen when it is created, split at the cache
  boundary: Claude Code caches the part before it across sessions.
- `env`: no auto memory, none of the user's claude.ai MCP servers, no tool search (Flottille's
  tools are all the agent has, so they load upfront; `docs/reference/mcp.md` §4 rule 7), and both
  MCP time limits at 10 minutes, longer than a call held for the user.
- `CLAUDE_CONFIG_DIR` stays where the user has it: the login lives there.

The MCP servers of the session are Flottille's alone: one HTTP server named `flottille`, the
daemon's own (`docs/tools.md`), with the token of the agent's process in an `Authorization`
header. Each start of the process mints a new token, which revokes the last; its end revokes it.

## The agent's environment

The agent's process gets the daemon's environment less the variables below (`claude::withheld`),
which are table-tested. The daemon's environment is the user's shell's, so it holds their
secrets, and a Claude Code agent run in bare mode has no use for them. The list is a choice the
maintainer can revise.

- **Forge tokens and their configuration**: `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`,
  `GITHUB_ENTERPRISE_TOKEN`, `GITLAB_TOKEN`, `GL_TOKEN`, `BITBUCKET_*`, `BKT_*`, `GH_CONFIG_DIR`,
  `GLAB_CONFIG_DIR`.
- **Any other variable whose name ends in** `_TOKEN`, `_SECRET`, `_PASSWORD` or `_API_KEY`,
  **except the agent's own sign-in**: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`,
  `CLAUDE_CODE_OAUTH_TOKEN`.
- **The user's Claude Code settings**, which would change the agent Flottille specifies:
  `ANTHROPIC_MODEL`, `ANTHROPIC_SMALL_FAST_MODEL`, `ANTHROPIC_DEFAULT_*_MODEL`, `CLAUDE_CODE_*`
  (Flottille sets `CLAUDE_CODE_EXECUTABLE` itself, and the bare options carry the others it needs;
  the prompt cache lifetime, `CLAUDE_CODE_PROMPT_CACHE_TTL`, is among them), `DISABLE_*`,
  `MAX_THINKING_TOKENS`, `FORCE_PROMPT_CACHING_5M`.

What stays: `ANTHROPIC_BASE_URL`, `CLAUDE_CONFIG_DIR` (the login lives there), proxy and
certificate variables (`HTTPS_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`…), and
the rest of the user's environment (`PATH`, `HOME`, locale). This means that a sign-in through a
cloud provider's own variables (`CLAUDE_CODE_USE_BEDROCK`, for one) does not reach the agent.

## The conversation

Every update the agent sends that Flottille keeps is an event of the agent (`docs/events.md`):
stored, numbered, streamed, and read back by `agent.show`. What a `session/load` replays is
history Flottille already holds, as it happened: it is not stored again, and it is not a sign of
activity (`docs/reference/hemera.md` §8.7). The agent's `active_at` is the time of its last
update outside a replay. Texts are cut at 64 KiB and say how
much was cut (`‹+N chars›`); secrets of the daemon's environment are masked in everything kept.

## The tree

An agent launches children with `agent_launch` (`docs/tools.md`): each is an agent of its own,
with a parent, a depth (a root is at 0), a title and a brief, which is all it knows of its task.
The user writes to and stops roots only: `agent.send` and `agent.stop` on a child are refused,
naming its root. The tree is one project's: a child works in its parent's place, an existing
workspace of the project, or a new one, made and prepared before it starts.

- **Caps**: at most `max_children` children of a project run at once (`docs/projects.md`, 1 to
  6, 3 by default), and a tree is at most 2 deep: an agent at depth 2 has no `agent_*` tool nor
  `approval_answer`. A launch above a cap is refused with a sentence; it is never queued.
- **Roles**: `lead` (by default for a root) and `worker` (by default for a child). An agent's
  instructions are Flottille's base, its role's template, the cache boundary, then what is its
  own: its project, its place, and either "you are the root agent" or its task's title; then the
  project's own instruction files (`CLAUDE.md` else `AGENTS.md`, and `CLAUDE.local.md` else
  `AGENTS.local.md`), read in the agent's place, else in the project's main checkout. The
  templates are in `crates/flottille/roles/`.
- **Stopping** an agent stops its tree, deepest first: each ends (`agent.ended`, or
  `agent.failed` when it is a failure that spreads), what was queued for it is superseded, the
  pending requests it made or held are cancelled (`approval.cancelled`: the user never sees them),
  its process and its runs are stopped. Workspaces stay. Nothing is delivered to an agent that
  ended.
- **A report** (`report`, at most 10,000 characters) of a child goes to its parent at once, and
  the child ends when its turn does, or 2 minutes later if its turn goes on. A root's report is
  kept and the root stays.

### Deliveries

Everything Flottille hands to an agent is a delivery: the user's messages (`user`), a brief
(`brief`), a parent's message (`parent`), what happens to a child (`child-report`,
`child-question`, `child-asks`, `child-failed`, `child-stuck`, `child-idle`, `child-replaced`,
`children-status`), an approval's answer (`approval`), a reminder (`reminder`), a session's
restart (`resume`). It is stored before it is sent, under an id that makes it idempotent, and it
is the only thing that wakes an agent.

- **As the agent reads it**: `[flottille:<kind>]` on a line, then the body. Bodies are data: a
  `[flottille:` in one is written `\[flottille:`, and `<flottille-note` and `</flottille-note`
  are written `&lt;flottille-note` and `&lt;/flottille-note`, so that no text forges a delivery
  nor opens or closes a note. Every tool's result is escaped the same way (`docs/tools.md`).
  Bodies are masked of the daemon's secrets.
- **Between turns**, the agent's queued deliveries are sent together as one prompt: a `brief`
  first, then a `resume`, then the others in their order.
- **During a turn**, an urgent one (`child-report`, `child-failed`, `child-asks`, `parent`,
  `approval`) is pinned as a note on the agent's next tool result:
  `<flottille-note id="…">`, the delivery, `</flottille-note>`. A note rides only in the room its
  result leaves under its limit, whole, in order; one that does not fit stays pinned for the next
  result. A note not taken within 30 s cancels the turn, and the delivery goes as the next prompt;
  a note whose delivery a prompt took meanwhile is dropped and cancels nothing. The others wait
  for the turn's end.
- **A turn that fails** gives back what it was sent: those deliveries are queued again.
- States: `queued`, `sending` (in a prompt), `sent`, `superseded` (its agent ended first). Each
  move is an event (`delivery.*`, `docs/events.md`).

A child that ends a turn without reporting, with nothing to wait for (no request, no question
held), is reminded once (`reminder`); after a second such turn its parent is told it went idle
(`child-idle`). A delivery other than a reminder starts the count again.

### Replacement

A session that cannot go on is replaced: its process dies, or it is stuck (below). The agent
keeps its id; its epoch counts the sessions. The new session starts from scratch, without the
old one's session id, and is sent its brief (a root's first message is its brief) and a `resume`
saying it starts again, once: the briefs and resumes of the old sessions not sent yet are
superseded. What else the old session was sent and had not finished is given back. The old session's token stays known,
so that its late calls are refused, saying it was replaced. The parent is told (`child-replaced`).
Two replacements within 30 minutes are allowed: the third failure fails the agent and its tree.

A child whose context is compacted is sent its brief again.

### The sweep

Every 5 seconds, the daemon looks at what only time decides:

- a note not taken within 30 s (above);
- a child that reported and is still in its turn 2 minutes later: it ends;
- a turn with no sign of activity for 5 minutes, not waiting on the provider, with no run of the
  agent's own running or ended within 5 minutes: the agent is stuck (`agent.stuck`), its parent
  told (`child-stuck`), and its session replaced;
- a lead between turns whose children run, with no delivery for 20 minutes: it is sent one line
  per child (`children-status`). The interval doubles with each such delivery in a row, up to
  160 minutes.

## For tests and development

- `FLOTTILLE_AGENT_COMMAND`: a program the daemon runs in place of Node and the adapter, with no
  argument. The tests run the fake ACP agent this way (`crates/flottille-fake-agent`, whose
  script format is documented in its source).
- `FLOTTILLE_IDLE_RELEASE_MS`: the idle release time, in milliseconds, in place of 5 minutes.
- `FLOTTILLE_TEST_CLOCK`: set (to anything), the daemon answers `test.advance {"ms"}` on its
  socket: its clock moves that far ahead, and the sweep runs at once. The tests see minutes pass
  this way without waiting for them. Without it, `test.advance` is an unknown method.

All three are read by the daemon from its environment, which is the environment of the command that
started it.
