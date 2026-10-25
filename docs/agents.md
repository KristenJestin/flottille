# Agents

An agent is one agent session, run over ACP (`docs/design.md` §4): a process of its provider
(Flottille's own Claude bridge in front of the user's `claude`, or the user's `opencode acp`;
"Providers", below) that Flottille starts, talks to on its stdio, and owns. It works in a place (the project folder
itself, `main`, or one of its workspaces) and runs in bare mode: no built-in tool, no user or
project setting, no skill, no plugin, no MCP server but Flottille's. It follows
`docs/reference/hemera.md` §1 and §2.

An agent can launch others (its children), which form its tree ("The tree", below), and is
woken only by deliveries. Flottille's MCP server and its tools are in `docs/tools.md`.

## Commands

| Command | |
|---|---|
| `flottille agent run <project> [--workspace <name>] [--provider <provider>] [--model <model>] [--effort <effort>] <message>` | Starts an agent with a first message and prints its reply as it comes, until the turn ends. Fails when the agent fails or its process dies. `--json` prints the agent instead, once the turn ended. |
| `flottille agent show <agent> [--json]` | The agent (its provider and model among the rest) and its conversation. |
| `flottille doctor [--json]` | Each provider: enabled or not, whether its agents can run here and why not. Fails when no provider is enabled or an enabled one cannot run. |

`agent run` starts a root and follows its first turn; `agent send --follow` and `agent chat` write
to a root and follow the turn of their message; `agent start`, `send`, `read`, `stop`, `list` and
`tree` drive a root and read its tree, and `flottille mcp` does the same for an agent outside
Flottille: all of them are in `docs/cli.md`. The protocol has the rest (`agent.cancel`,
`docs/protocol.md`).

## What the machine needs

For each provider enabled, **the user's own program**, on `PATH`, signed in by the user or given
an API key. Flottille installs nothing and provides no login of its own; an API key in the
environment counts as a sign-in, and is never printed, logged or stored.

For Claude, the user's `claude`, signed in or given `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`
or `ANTHROPIC_AUTH_TOKEN`. Nothing else is installed: the bridge that speaks ACP in front of it is
Flottille's own binary (below). Flottille provides no login of its own and never reads, copies or
prints the user's credentials: `flottille doctor` checks that the login file
(`$CLAUDE_CONFIG_DIR/.credentials.json`, else `~/.claude/.credentials.json`) exists, without
opening it, or that one of those variables is set, without printing it. On macOS the login is in
the Keychain and there is no file.

`flottille doctor` reports `claude` with its path and version (`claude --version`, given 5 s),
beside the version this Flottille was checked against (2.1.295): another version is a warning that
names both, not a failure, since the stream-json mode the bridge speaks may have changed. It
fails while `claude` is not on `PATH`. Neither the login file nor a variable there is a warning,
not a failure: the login may be where Flottille does not look (the Keychain), and an agent that
is not signed in at all fails at its first prompt, saying so ("The Claude bridge", below).

For OpenCode, the user's `opencode`, signed in (`opencode auth login`; the login file,
`$XDG_DATA_HOME/opencode/auth.json` else `~/.local/share/opencode/auth.json`, is checked for its
presence only) or given the API key of one of the model providers it takes: `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `GOOGLE_GENERATIVE_AI_API_KEY`, `GEMINI_API_KEY`,
`GROQ_API_KEY`, `MISTRAL_API_KEY`, `DEEPSEEK_API_KEY`, `XAI_API_KEY`, `OPENCODE_API_KEY` (the list
is a choice the maintainer can revise). Neither is a warning, not a failure: OpenCode offers
some free models with no sign-in, and takes a cloud's credentials (`AWS_*` for Bedrock, say) that
Flottille does not count. Its version is checked against 1.19.0, the version whose bare mode was
qualified ("Bare mode for OpenCode", below): another one is refused, its agents with it, since
the bare mode rests on what that version reads and on two variables it keeps for its own tests
(`OPENCODE_TEST_HOME`, `OPENCODE_TEST_MANAGED_CONFIG_DIR`), which another version may drop. The
user may run another version at their own risk with `allow_unqualified_version = true` under
`[opencode]` in `providers.toml`: it is then a warning that names both versions. An `opencode`
that does not say its version is refused the same way.

An agent is refused at its start, with a sentence that says why and names the providers that
are ready, while its provider is not enabled or not ready ("Providers", below).

## Providers

A provider is what an agent runs on: `claude` and `opencode`, in this order. Each is a module of
the daemon (`claude.rs`, `opencode.rs`) that says how its agent starts, its bare environment and
configuration, how it is checked ready, how it names Flottille's tools and how its models and
efforts are read; the tree, the deliveries, the gate and the store know nothing of them.

- **Enabled**: `$XDG_CONFIG_HOME/flottille/providers.toml`, one table per provider, each with
  `enabled`, and OpenCode's with `allow_unqualified_version` (above; `false` by default, and
  refused under `[claude]`, whose version is only ever a warning). Without the file, or without a
  provider's table, Claude is enabled and OpenCode is not. A provider it does not know, another
  key, or a value of the wrong type is refused, naming the key at fault:

  ```toml
  [opencode]
  enabled = true
  ```

- **Ready**: enabled, its program on `PATH`, and, for OpenCode, its version the qualified one or
  allowed (above). The version is asked (`--version`, given 5 s, off the daemon's thread) by
  `doctor`, `provider.list` and each start of an agent's process, and kept while the program's
  file does not change: an agent's start is refused at once when it is known, else the agent
  fails when its process would start. A sign-in Flottille can see (its login file is
  there or one of its API keys is set) is not needed: without one, `doctor` warns, and an agent
  that is not signed in at all fails once its provider refuses it. A test override (below) stands for
  the program.
- **An agent's provider** is the one asked (`--provider`, `agent_start`'s and `agent_launch`'s
  `provider`), else the project's (`provider` in its file, `docs/projects.md`), else the first one
  enabled; a child's is its parent's unless it names another. It is kept in the agent and never
  changes: a replacement runs on the same provider. It is checked again at each start of the
  agent's process, a replacement's included: an agent whose provider is no longer enabled or
  ready fails (`agent.failed`, with the sentence a start would be refused with), and is not
  started again.
- **Its model** is the one asked, else the project's (`model` in its file, for the project's own
  provider only), else the provider's default: the model a new session has before any is set,
  marked `"default": true` among the models once a session said it. Its effort is the one asked,
  else the model's default.
- **The models**: what a session of the provider's agent offers in its config options (the
  `model` option's values, and the `thought_level` option's for the model the session has), kept
  by the daemon per provider (`provider_models` in the store), and learned again from every
  session that opens. They are kept with the program they came from (its path and modification
  time) and the names of the provider's API keys set (never their values): another version of
  the program, or another set of keys in the daemon's environment, forgets them. They stay
  approximate: what the provider offers also depends on what neither says, its sign-in file
  (signing in or out with `opencode auth login`), OpenCode's model catalogue (fetched from
  `models.dev`, `core/src/models-dev.ts`) and its own state, and such a change shows only at the
  next session.
  No agent is ever run just to list them: until a first session said them, they are `null`, an
  agent may ask any model and effort, and one its session does not offer fails the agent
  ("The options", below). Once they are known, a model or an effort they do not hold is refused
  at once, naming those the provider offers. The efforts of a model are `null` until a session
  had that model. An effort asked without a model is checked against the default model's
  efforts, when they are known; until then it is taken, and a session that does not offer it
  fails the agent.
- **Refusals** name what is available: `there is no provider codex: the providers are claude,
  opencode`; `provider opencode is not enabled (…); the ready providers are claude`; `provider
  opencode is not ready: `opencode` is not on PATH (…); no provider is ready: see `flottille
  doctor``; `provider claude offers no model x: it offers a, b`; `provider claude's model a
  offers no effort max: it offers low, high`; `provider claude's default model a offers no
  effort max: it offers low, high`.
- **The method** `provider.list` (`docs/protocol.md`) gives every provider as `doctor` does, with
  its models and, for a project, its default provider and model.

## How an agent runs

- **The process**: the provider's. For Claude, `flottille bridge claude` (the daemon's own
  binary, "The Claude bridge", below); the `claude` it starts is in the same group. For OpenCode,
  `opencode acp`, with its bare environment ("Bare mode for OpenCode", below). It runs in the
  agent's folder, in a process group of its own, with the daemon's environment less what it has
  no business with (below). Its stderr goes to the daemon's log, masked; its stdout is the
  protocol.
- **The handshake**: `initialize` with ACP v1, the client named `Flottille`, and no `fs` or
  `terminal` capability (the agent has no built-in file or terminal tool) nor `subagents` (no
  native sub-agent). It declares the `jetbrains.air` extension with `recommendedValue` (a
  "default" option value is named) and `sessionFailure` (provider waits and limits are reported).
- **The session**, opened with the agent's folder as `cwd`, Flottille's MCP server only, and the
  bare options in `_meta`, in this order: `session/resume` when the agent has a session id and
  the bridge resumes; else `session/load` when it loads; else `session/new`. The session id is
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
  a tool of the agent's own list, named by the provider's own field (Claude:
  `_meta.claudeCode.toolName`), is allowed once; anything else is rejected, a request with no tool
  name included (its title is never taken for one: the model can shape it). OpenCode names the
  tool in the title only, so each of its requests is rejected. In bare mode they should almost
  never come.
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
bridge sets on `claude` ("The Claude bridge", below):

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
- `settingSources: []`: Claude Code's own default loads the user's settings, hooks, plugins,
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

## Bare mode for OpenCode

OpenCode takes its configuration from variables and files, not from the session: its bare mode
is the environment the daemon starts `opencode acp` with (`opencode::bare`, table-tested), and
nothing in `_meta`. It was qualified against OpenCode 1.19.0 by reading its source (tag `v1.19.0`;
the places below are under `packages/`), without running an agent: the run that proves it comes
after.

| Variable | Value | What it shuts |
|---|---|---|
| `XDG_CONFIG_HOME` | `<data>/agents/opencode/config` | The user's `opencode.json`, agents, commands, plugins, `AGENTS.md`: OpenCode's global folder is `$XDG_CONFIG_HOME/opencode` (`opencode/src/config/config.ts`, the global files first; `core/src/instruction-context.ts:58` its `AGENTS.md`). |
| `OPENCODE_TEST_HOME` | `<data>/agents/opencode/home` | The home it reads `~/.opencode`, `~/.claude/CLAUDE.md` and skills from (`core/src/global.ts:19`). |
| `OPENCODE_TEST_MANAGED_CONFIG_DIR` | `<data>/agents/opencode/managed` | The machine's managed settings, `/etc/opencode`, which come after every other file (`opencode/src/config/managed.ts:32`). |
| `OPENCODE_DB` | `<data>/agents/opencode/opencode.db` | Its sessions, kept apart from the user's; with them a signed-in OpenCode account, whose organisation settings would be merged (`core/src/database/database.ts:44`). |
| `OPENCODE_DISABLE_PROJECT_CONFIG` | `1` | The project's `opencode.json`, `.opencode/` and `AGENTS.md` (`opencode/src/config/paths.ts:27`, `core/src/instruction-context.ts:48`). |
| `OPENCODE_DISABLE_CLAUDE_CODE` | `1` | `CLAUDE.md` files and Claude Code skills (`opencode/src/effect/runtime-flags.ts:22-29`, `opencode/src/session/instruction.ts:62-66`). |
| `OPENCODE_DISABLE_EXTERNAL_SKILLS` | `1` | Skills from outside its folder (`opencode/src/effect/runtime-flags.ts:20`). |
| `OPENCODE_PURE` | `1` | External plugins (`opencode/src/plugin/index.ts:181`). |
| `OPENCODE_DISABLE_AUTOUPDATE`, `OPENCODE_DISABLE_LSP_DOWNLOAD` | `1` | Nothing installed or updated behind the user's back (`opencode/src/cli/upgrade.ts:23`). |
| `OPENCODE_SERVER_PASSWORD` | 32 random hexadecimal digits, new for each process | The HTTP server `opencode acp` runs for itself, which has shell and terminal routes and is open to anyone on the machine without a password (`opencode/src/cli/cmd/serve.ts:15`). |
| `OPENCODE_CONFIG_CONTENT` | below | Merged after every file it reads (`core/src/flag/flag.ts:22`, `opencode/src/config/config.ts`). |

```json
{
  "default_agent": "flottille",
  "agent": {
    "flottille": {
      "mode": "primary",
      "prompt": "{file:<runtime>/opencode/<agent>.md}",
      "permission": {
        "*": "deny", "doom_loop": "allow",
        "bash": "deny", "read": "deny", "…": "deny", "external_directory": "deny",
        "flottille_fs_read": "allow", "…": "allow"
      }
    },
    "build": {"disable": true}, "plan": {"disable": true},
    "general": {"disable": true}, "explore": {"disable": true}
  },
  "share": "disabled",
  "autoupdate": false,
  "snapshot": false,
  "experimental": {"mcp_timeout": 600000}
}
```

- **No built-in tool**: OpenCode's tools (shell, read, glob, grep, edit, write, task, webfetch,
  todo, websearch, skill, patch, lsp, plan…; `opencode/src/tool/registry.ts:209-245`) are left
  out of what the model is sent when the last permission rule that matches them denies them for
  every pattern (`opencode/src/permission/index.ts:204`): `"*": "deny"` does it for all of them,
  MCP resource tools included, and each permission OpenCode names for one of its tools is denied
  again by name after it (`bash`, `read`, `glob`, `grep`, `list`, `edit`, `write`, `apply_patch`,
  `task`, `webfetch`, `websearch`, `codesearch`, `todowrite`, `todoread`, `skill`, `question`,
  `plan_enter`, `plan_exit`, `lsp`, `execute`, `external_directory`; any key is accepted, in its
  order: `core/src/v1/config/permission.ts`). The agent's rules come last
  (`opencode/src/agent/agent.ts:138, 277-293`), so nothing of the user's can loosen them. MCP
  tools go through the same rules, named `<server>_<tool>` (`opencode/src/mcp/catalog.ts:119`):
  only the agent's own tools are allowed, each by name (`flottille_fs_read` and so on for an
  OpenCode agent), so that another server named like Flottille's is not.
- **Repeated calls**: OpenCode asks the `doom_loop` permission before a third identical tool
  call in a row (`opencode/src/session/processor.ts:356-379`; `ask` by default,
  `opencode/src/agent/agent.ts:121`), which `"*": "deny"` would turn into a failed call:
  `doom_loop` is allowed, and Flottille's gate judges the call itself.
- **No sub-agent**: the `task` tool is denied, and the agents that ship with OpenCode are
  disabled; `flottille` is the only one.
- **No command**: OpenCode's ACP agent takes a prompt whose text, its text blocks joined and
  trimmed, starts with `/` for one of its commands (`opencode/src/acp/service.ts:519-560,
  826-837`), and a command's template runs `` !`…` `` in a shell, outside the gate, with its
  arguments substituted, and attaches the files `@path` names (`opencode/src/session/prompt.ts:157-190,
  1372-1406`). Every prompt Flottille sends is deliveries, and starts with a delivery's marker
  (`[flottille:<kind>]`, "Deliveries", below): the daemon refuses to send any other, whatever the
  provider (`acp::prompt_text`), so that no prompt is ever read as a command. Inside a prompt
  that is not a command, `@` and `` !`…` `` are text: OpenCode expands them only in a command's
  template and in the `task` tool's prompt, which is denied (`opencode/src/tool/task.ts:201`).
- **Its instructions**: the agent's prompt replaces the provider's own system prompt
  (`opencode/src/session/llm/request.ts:60`); no `AGENTS.md`/`CLAUDE.md` of the user's or the
  project's is read (above). OpenCode replaces `{env:…}` and `{file:…}` in the configuration's text
  (`opencode/src/config/variable.ts:34`): the instructions are never in it, only the path of the
  file they are written to (mode `0600`, under the runtime folder, removed with the agent), and a
  runtime folder whose path holds `{`, `}`, `"`, `\` or a control character is refused.
- **MCP**: Flottille's server is given with the session, its `Authorization` header honoured
  (`opencode/src/acp/service.ts:1070`). A call carries no call key, so a retried call is played
  again (the daemon logs it once). The SDK's default MCP time limit is raised to 10 minutes.
- **Permission prompts** name the tool in the title only (`opencode/src/acp/permission.ts`): they
  are all rejected.
- **Usage**: OpenCode sends `usage_update` with the context used, its size and the cost
  (`opencode/src/acp/usage.ts:210`); a provider error ends the prompt with an ACP error
  (`opencode/src/acp/service.ts:839-890`), not with the `sessionFailure` extension.
- **Models**: the `model` option's values are `<provider>/<model>`; the effort option lists the
  variants of the current model only (`opencode/src/acp/config-option.ts`).
- **What it keeps**: its configuration folder, home, managed folder and database are in
  `<data>/agents/opencode`. Its data folder (`$XDG_DATA_HOME/opencode`) stays the user's, since the
  login is there.
- **An organisation's sign-in is full control.** A login of type `wellknown` in `auth.json` (an
  organisation's OpenCode server, `opencode auth login <url>`) makes OpenCode fetch that server's
  configuration at every start and merge it before every other one
  (`opencode/src/config/config.ts:370-410`). Flottille's comes after it, merged with `mergeDeep`
  (remeda): a key Flottille sets takes Flottille's value, a key it does not set stays, and the
  lists of `instructions` are joined (`opencode/src/config/config.ts:42-52`). What Flottille wins:
  `default_agent`, its agent's prompt, the disabled agents, and its rules above, as far as they
  go: the remote's own rules for the agent `flottille` keep their place, so a rule of the remote's
  on one of OpenCode's tools followed by a pattern of its own that matches that tool
  (`{"*": …, "bash": …, "b*": "allow"}`) still allows it; a remote rule that comes after `*`
  without naming a tool Flottille lists allows what it matches. What it cannot undo, since their
  names are the remote's: its `mcp` servers (a `local` one is a program the remote names, started
  on the user's machine outside the gate), its `plugin`s (code OpenCode loads), its
  `instructions` (added to the agent's), its `provider` settings (where the prompts go). Such a
  sign-in therefore gives that organisation's server full control of Flottille's OpenCode agents,
  a shell included. Flottille cannot tell: knowing it would mean reading `auth.json`, which it
  never opens, so `flottille doctor` does not warn of it. A user signed in that way should sign
  in to OpenCode another way (an API key) to run its agents under Flottille.

## The Claude bridge

`flottille bridge claude` (hidden from `--help`) is an ACP agent on stdio, written in Rust inside
Flottille's binary, which drives the user's `claude` in its stream-json mode, as the version of
`claude` it was checked against (2.1.295) speaks it. The daemon stays a plain ACP client: it
starts the bridge as it would start any ACP agent, and the bridge translates. It decides nothing:
the bare mode is the daemon's options, which the bridge refuses when they are not bare
(`tools` or `settingSources` not empty, `strictMcpConfig` not set, an option it does not know, an
MCP server that is not HTTP), and Claude's own permission prompts are the daemon's to answer. One
bridge runs one session.

- **`claude` is started** without a shell, in the session's `cwd`, with the bridge's environment,
  the options' `env` and `CLAUDE_CODE_ENTRYPOINT=sdk-rs`, and exactly these arguments:
  `--output-format stream-json --verbose --input-format stream-json --include-partial-messages
  --permission-prompt-tool=stdio --tools= --setting-sources= --strict-mcp-config
  --allowedTools=<the tools, comma-separated> --mcp-config <file>`, then
  `--session-id=<a new UUID>` for `session/new`, or `--resume=<id>` for a resume or a load.
- **The MCP configuration** (`{"mcpServers": {"flottille": {"type": "http", "url", "headers"}}}`)
  is a file only the user can read, `$XDG_RUNTIME_DIR/flottille/bridge/<the bridge's pid>.mcp.json`,
  removed when the bridge ends: the token never shows on a command line, which other users can
  read. A bridge killed before it could remove it leaves it to the daemon, which removes it once
  the bridge's process group is gone, and removes any left at its start.
- **The system prompt**, split at the cache boundary, goes in Claude Code's own `initialize`
  control request (`systemPrompt: [before, boundary, after]`, `systemPromptSnapshot: true`), as
  the official Agent SDK sends it. The session opens once `claude` answered it; a `claude` that
  ends before (a resume of a session it does not know) fails the open.
- **What `claude` says**, mapped to ACP updates: stream deltas of text and thinking become
  `agent_message_chunk` and `agent_thought_chunk` (the whole message that follows does not repeat
  them); a `tool_use` becomes a `tool_call` (`in_progress`, the tool's name as its title and in
  `_meta.claudeCode.toolName`, its input as `rawInput`) whose id is the tool use's, the one
  `claude` gives Flottille's MCP server as `claudecode/toolUseId`; a `tool_result` its
  `tool_call_update` (`completed` or `failed`); a `compact_boundary` a completed `tool_call` with
  `_meta.contextCompaction`; an `api_retry` a `session_info_update` with a `jetbrains.air`
  `sessionFailure` of severity `warning` (a provider wait). A sub-agent's messages
  (`parent_tool_use_id` set) are ignored: there is none in bare mode.
- **A prompt** is its text blocks, parted by a blank line; any other block is refused, as
  `initialize` says (`embeddedContext: false`).
- **A line of `claude`'s output** that is not JSON, or not UTF-8, is skipped.
- **A turn** is one `user` message written to `claude`; its `result` ends it: first a
  `usage_update` (the context in use, as the last message or compaction said, the model's
  context window, and the cost in USD of this `claude` process so far: it counts from zero again
  in each process, after a release or a replacement, and is not the session's), then the prompt's answer with its usage and its stop reason:
  `cancelled` after a cancel, `refusal` or `max_tokens` as `claude` says, `max_turn_requests` for
  a result `error_max_turns`, `error_max_budget_usd` or `error_max_structured_output_retries`,
  `end_turn` otherwise. A result that is an error ends the turn `end_turn` too, after a
  `session_info_update` with a `jetbrains.air` `sessionFailure` of severity `error` and its text:
  category `limit` when the provider refused it for a limit (a `rate_limit_event` rejected, an
  error `rate_limit`, `billing_error` or `account_on_hold`, or Claude Code's own message for a
  spent usage), `service` otherwise. The session lives on, as with Claude Code's own ACP
  adapter. Only a signed-out `claude` (`Please run /login`, an error `authentication_failed`)
  fails the prompt, and so the agent: it cannot go on until the user signs in again.
- **A cancel** (`session/cancel`) is an `interrupt` control request.
- **Claude's permission prompts** (`can_use_tool`) become `session/request_permission` (the tool
  in `_meta.claudeCode.toolName`, the tool use's id, options `allow_once` and `reject_once`); the
  answer allows the call with its input unchanged, or denies it. In bare mode they should almost
  never come: Flottille's tools are pre-allowed.
- **The model** is an option (`model`) whose values are the `models` of `claude`'s answer to
  `initialize` (their `value`, `default` first when it does not list it), set with a `set_model`
  control request; a value it did not list is refused with the list.
- **The effort** is an option (`effort`, category `thought_level`) when the model set has effort
  levels (`supportsEffort`, `supportedEffortLevels` in the same answer): `default` and those
  levels, set with an `apply_flag_settings` control request (`effortLevel`, `null` for
  `default`), the session's own flag layer, as `--effort` would. A model without the level set
  takes `default` back. There is no mode option: `agent.start` refuses a `mode`, since a
  permission mode of Claude Code's could only loosen Flottille's gate.
- **Resume and load**: both start `claude` with `--resume`. A load first replays the session's
  history, the texts of its prompts, replies and thoughts, which the bridge keeps in
  `$XDG_STATE_HOME/flottille/bridge/claude/<session>.jsonl` (it never reads Claude Code's own
  files): one line per message, its secrets masked as the daemon masks what it keeps, only its
  later half kept past 4 MiB. The daemon removes it when the session is replaced or its agent
  ends.
- **A death**: once the session is open, the end of `claude` ends the bridge with its exit code
  (128 and the signal for a signal), which the daemon sees as the agent's death.
- **Its end**: its input closed or a `SIGTERM`, the bridge closes `claude`'s input, kills it if
  it is still there 2 s later, and removes the MCP configuration.

## The agent's environment

The agent's process gets the daemon's environment less the variables its provider withholds
(`claude::withheld`, `opencode::withheld`), which are table-tested. The daemon's environment is
the user's shell's, so it holds their secrets, and an agent run in bare mode has no use for them.
The lists are a choice the maintainer can revise. Each provider lets through its own sign-in
only: a key of one provider never reaches another's agent.

For Claude:

- **Forge tokens and their configuration**: `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`,
  `GITHUB_ENTERPRISE_TOKEN`, `GITLAB_TOKEN`, `GL_TOKEN`, `BITBUCKET_*`, `BKT_*`, `GH_CONFIG_DIR`,
  `GLAB_CONFIG_DIR`.
- **Any other variable whose name ends in** `_TOKEN`, `_SECRET`, `_PASSWORD` or `_API_KEY`,
  **except the agent's own sign-in**: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`,
  `CLAUDE_CODE_OAUTH_TOKEN`.
- **The user's Claude Code settings**, which would change the agent Flottille specifies:
  `ANTHROPIC_MODEL`, `ANTHROPIC_SMALL_FAST_MODEL`, `ANTHROPIC_DEFAULT_*_MODEL`, `CLAUDE_CODE_*`
  (the bare options carry the ones Flottille needs;
  the prompt cache lifetime, `CLAUDE_CODE_PROMPT_CACHE_TTL`, is among them), `DISABLE_*`,
  `MAX_THINKING_TOKENS`, `FORCE_PROMPT_CACHING_5M`.

What stays: `ANTHROPIC_BASE_URL`, `CLAUDE_CONFIG_DIR` (the login lives there), proxy and
certificate variables (`HTTPS_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`…), and
the rest of the user's environment (`PATH`, `HOME`, locale). This means that a sign-in through a
cloud provider's own variables (`CLAUDE_CODE_USE_BEDROCK`, for one) does not reach the agent.

For OpenCode: the same forge tokens and secret-looking names, except the API keys it takes
(above), and every `OPENCODE_*` variable of the user's (`OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`,
`OPENCODE_PERMISSION`…), which would undo its bare mode; Flottille sets the ones it needs.

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

A question waits for its answer from the call (`ask_parent`, or a root's `ask_user`) until the one
asked writes to the agent: a child then waits on its parent, a root on the user, who sees it in
`flottille status` and under the window's bell (`waiting_on_answer`, `docs/protocol.md`).

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

- `FLOTTILLE_AGENT_COMMAND`: a program the daemon runs in place of every provider's, with no
  argument; `FLOTTILLE_AGENT_COMMAND_CLAUDE` and `FLOTTILLE_AGENT_COMMAND_OPENCODE`, in place of
  one provider's (the tests run two fake agents standing for two providers this way). The
  provider's bare environment is still set; the program stands for the provider's program and
  sign-in, so the provider is ready when it is enabled, and its version is never asked. The tests run the fake ACP agent this way (`crates/flottille-fake-agent`, whose
  script format is documented in its source). The bridge itself is tested in front of a fake
  `claude` (`crates/flottille-fake-claude`), first in `PATH`.
- `FLOTTILLE_BRIDGE_NO_RESUME`: set (to anything), the bridge does not offer `session/resume`, so
  that the daemon loads its sessions; the tests reach the load this way.
- `FLOTTILLE_IDLE_RELEASE_MS`: the idle release time, in milliseconds, in place of 5 minutes.
- `FLOTTILLE_TEST_CLOCK`: set (to anything), the daemon answers `test.advance {"ms"}` on its
  socket: its clock moves that far ahead, and the sweep runs at once. The tests see minutes pass
  this way without waiting for them. Without it, `test.advance` is an unknown method.

All of them are read by the daemon (or the bridge it starts) from its environment, which is the environment of the command that
started it.
