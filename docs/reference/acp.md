# Reference: ACP and the adapters, as of October 2026

Checked on 2026-10-23 from the published packages, crates and repositories. Re-check versions
when you start: these move every week.

## The protocol and the Rust crate

- ACP (Agent Client Protocol, github.com/agentclientprotocol/agent-client-protocol): JSON-RPC
  over stdio, one message per line. Stable wire protocol **v1** (release 1.12.0, schema 1.26.0);
  a draft v2 exists (alpha), do not build on it.
- v1 methods. Agent side: `initialize`, `authenticate`, `session/new`, `session/load`,
  `session/list`, `session/resume`, `session/close`, `session/delete`, `session/prompt`,
  `session/cancel`, `session/set_mode`, `session/set_config_option`, `logout`. Client side:
  `session/request_permission`, `session/update`, `fs/read_text_file`, `fs/write_text_file`,
  `terminal/*`, `elicitation/*`.
- Rust: the official crate **`agent-client-protocol`** (3.5.0, repository
  agentclientprotocol/rust-sdk), schema crate `agent-client-protocol-schema`. Client side:
  `AcpAgent::from_str("cmd args")` spawns the agent on stdio; handlers are registered on
  `Client.builder()` (`on_receive_notification` for `SessionNotification`, `on_receive_request`
  for `RequestPermissionRequest`); then `connect_with(agent, |conn| …)` sends `initialize`,
  `NewSessionRequest` (cwd, `mcpServers`, `_meta`) and `PromptRequest`. See
  `examples/yolo_one_shot_client.rs` in that repository.
- Flottille does **not** declare the `fs` or `terminal` client capabilities: agents have no
  built-in file or terminal tools in bare mode, everything goes through Flottille's MCP tools.
- Do not declare `clientCapabilities.subagents`: agents must not have native sub-agents at all.

## Adapters

| Agent | How | Version seen |
|---|---|---|
| Claude Code | `@agentclientprotocol/claude-agent-acp` (TypeScript, on the official Claude Agent SDK; spawns the user's `claude`, override with `CLAUDE_CODE_EXECUTABLE`). The old `@zed-industries/*` names are deprecated. | 0.94.0 (Hemera measured 0.79.0) |
| Codex | `@agentclientprotocol/codex-acp` (TypeScript, drives `codex app-server`; `CODEX_PATH`, `CODEX_CONFIG`) | 2.2.2 (Hemera patched 1.12.0) |
| OpenCode | native `opencode acp` | 1.19.0 |
| Gemini CLI | native `gemini --acp` | 0.65.0 |

Claude Code itself (2.1.295) has no native ACP mode; Codex (0.162.1) neither.

## Locking Claude down through the adapter

The adapter passes `session/new` `_meta.claudeCode.options` almost wholesale as Agent SDK
options (`tools`, `allowedTools`, `disallowedTools`, `settingSources`, `strictMcpConfig`,
`systemPrompt`, `env`, `extraArgs`…). It controls `cwd`, `permissionMode` (via ACP modes),
`canUseTool` (bridged to `session/request_permission`) and the executable itself.

**Trap**: the adapter's own default is `settingSources: ["user", "project", "local"]`, which
loads the user's settings, hooks, plugins, skills and instruction files. Bare mode must pass
`settingSources: []` explicitly (it overrides the default). The full bare option set is in
`hemera.md` §2.

## Codex

`codex features list` shows `multi_agent` stable and **on by default**, plus `unified_exec`
(background terminals) and `shell_tool`. Bare mode turns them off through `CODEX_CONFIG`
(nested objects, never dotted keys) and needs the adapter patch described in `hemera.md` §2.

## Use of a subscription

The Claude Agent SDK documentation says third-party developers may not offer claude.ai login or
its rate limits in their products without approval. Flottille provides no login: it runs the
user's own `claude`, signed in by the user, for that user. Never describe Flottille as a product
that uses its users' subscriptions.
