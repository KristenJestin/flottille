# Agents

An agent is one agent session, run over ACP (`docs/design.md` §4): a Claude Code adapter process
that Flottille starts, talks to on its stdio, and owns. It works in a place (the project folder
itself, `main`, or one of its workspaces) and runs in bare mode: no built-in tool, no user or
project setting, no skill, no plugin, no MCP server but Flottille's. It follows
`docs/reference/hemera.md` §1 and §2.

The tree of agents, the deliveries that wake them, their roles and their full instructions come
later; so do Flottille's MCP server and its tools. This file says what runs today.

## Commands

| Command | |
|---|---|
| `flottille agent run <project> [--workspace <name>] [--model <model>] <message>` | Starts a Claude agent with a first message and prints its reply as it comes, until the turn ends. Fails when the agent fails or its process dies. `--json` prints the agent instead, once the turn ended. |
| `flottille agent show <agent> [--json]` | The agent and its conversation. |
| `flottille agent install` | Installs the Claude adapter at the version this Flottille runs. |
| `flottille doctor [--json]` | Whether Claude agents can run here. Fails when they cannot. |

These are a minimal way in until the real commands come; the protocol has the rest
(`agent.send`, `agent.cancel`, `docs/protocol.md`).

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
- **A death**: a process that ends without being let go fails the agent and is published
  (`agent.died`); nothing starts it again at this layer.
- **Orphans**: each process's pid and start time are kept in the store while it runs. A daemon
  starting kills the process groups a previous one left behind (a crash), only when the process
  with that pid is still the one it started, then forgets them. An agent that was starting or
  working when its daemon went is idle again (`agent.released`, reason `restart`).
- **Daemon stop**: every agent's process is let go.

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
  `commands_stop` and `report`; one that can have children also `agent_launch`, `agent_send`,
  `agent_status`, `agent_read`, `agent_stop` and `approval_answer`; a root `ask_user`, a child
  `ask_parent`. Roots are the only agents until the tree comes, so today's agents get, in this
  order, the first nine, the five `agent_*`, `report`, `ask_user`, `approval_answer` (17). The
  list is frozen in the agent when it is created (the prompt cache depends on it).
- `settingSources: []`: the adapter's own default loads the user's settings, hooks, plugins,
  skills and instruction files; this overrides it.
- `strictMcpConfig`: only the MCP servers given with the session.
- `systemPrompt`: the agent's instructions, frozen when it is created, split at the cache
  boundary: Claude Code caches the part before it across sessions.
- `env`: no auto memory, none of the user's claude.ai MCP servers, no tool search (Flottille's
  tools are all the agent has, so they load upfront; `docs/reference/mcp.md` §4 rule 7), and both
  MCP time limits at 10 minutes, longer than a call held for the user.
- `CLAUDE_CONFIG_DIR` stays where the user has it: the login lives there.

The MCP servers of the session are Flottille's alone: one HTTP server named `flottille`. Until
Flottille's MCP server exists, the entry points at nothing (`http://127.0.0.1:9/mcp`), unused.

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
stored, numbered, streamed, and read back by `agent.show`. Each carries `replay`: true while a
`session/load` replays the history, which is then neither a sign of activity nor new. The agent's
`active_at` is the time of its last update outside a replay. Texts are cut at 64 KiB and say how
much was cut (`‹+N chars›`); secrets of the daemon's environment are masked in everything kept.

## For tests and development

- `FLOTTILLE_AGENT_COMMAND`: a program the daemon runs in place of Node and the adapter, with no
  argument. The tests run the fake ACP agent this way (`crates/flottille-fake-agent`, whose
  script format is documented in its source).
- `FLOTTILLE_IDLE_RELEASE_MS`: the idle release time, in milliseconds, in place of 5 minutes.

Both are read by the daemon from its environment, which is the environment of the command that
started it.
