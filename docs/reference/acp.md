# Reference: ACP, the adapters and Claude's stream-json mode, as of October 2026

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
| Claude Code | **Flottille's own bridge**, `flottille bridge claude` (Rust, inside Flottille's binary), below. The official adapter `@agentclientprotocol/claude-agent-acp` (TypeScript, on the official Claude Agent SDK) is not used. | `claude` 2.1.295 (the adapter was 0.95.0, on Agent SDK 0.3.309) |
| Codex | `@agentclientprotocol/codex-acp` (TypeScript, drives `codex app-server`; `CODEX_PATH`, `CODEX_CONFIG`) | 2.2.2 (Hemera patched 1.12.0) |
| OpenCode | native `opencode acp` | 1.19.0 |
| Gemini CLI | native `gemini --acp` | 0.65.0 |

Claude Code itself (2.1.295) has no native ACP mode; Codex (0.162.1) neither.

## Claude, through Flottille's own bridge

Claude Code has no ACP mode of its own. Flottille does not run the official adapter (it needs a
JavaScript runtime behind a Rust program); its bridge speaks ACP to the daemon and drives the
user's `claude` in the stream-json mode that the official Agent SDKs use. What the bridge sends
and reads was taken from the TypeScript Agent SDK 0.3.309 (the one the adapter 0.95.0 runs on) and
checked against `claude` 2.1.295; `flottille doctor` warns on another version. The mapping is in
`docs/agents.md` ("The Claude bridge").

- **Arguments**: `--output-format stream-json --verbose --input-format stream-json`, and, for bare
  mode, `--tools=` (no built-in tool), `--setting-sources=` (no user, project or local settings:
  Claude Code's default loads all three, with their hooks, plugins and skills), `--strict-mcp-config`
  with `--mcp-config <file>`, `--allowedTools=<exact names>`, `--permission-prompt-tool=stdio`
  (Claude's permission prompts come to the bridge as control requests),
  `--include-partial-messages` (text and thinking as stream deltas), and `--session-id=<uuid>` or
  `--resume=<id>`. The SDK writes a flag with a value as `--flag=value`.
- **Control requests** on stdin and stdout (`{"type": "control_request", "request_id", "request":
  {"subtype", …}}`, answered by `{"type": "control_response", "response": {"subtype": "success" |
  "error", "request_id", "response" | "error"}}`), both ways. The bridge sends `initialize` (with
  `systemPrompt`, a list of texts, and `systemPromptSnapshot`: this is the only way to keep the
  cache boundary), `interrupt` and `set_model`; `claude` sends `can_use_tool` (`tool_name`,
  `input`, `tool_use_id`), answered `{"behavior": "allow", "updatedInput"}` or `{"behavior":
  "deny", "message"}`.
- **Messages** from `claude`: `system` (`init`, `status`, `compact_boundary` with
  `compact_metadata.post_tokens`, `api_retry`), `stream_event` (Anthropic stream events),
  `assistant` and `user` (whole messages, with `tool_use` and `tool_result` blocks), `result` (the
  end of a turn: `subtype`, `is_error`, `stop_reason`, `usage`, `modelUsage` with each model's
  `contextWindow`, `total_cost_usd`). A prompt is a `user` message written to stdin.
- **The Rust crate `claude-agent-sdk-rs`** (0.6.4, unofficial, MIT) was considered and not taken:
  it cannot send the system prompt with its cache boundary (its `initialize` carries hooks only),
  has no load with history, puts the MCP configuration with the token on the command line, pipes
  `claude`'s stderr without reading it unless given a callback, and brings a dozen dependencies.
  The crate named `claude-agent-sdk` names an Anthropic repository that does not exist; it is not
  official either.

## Codex

`codex features list` shows `multi_agent` stable and **on by default**, plus `unified_exec`
(background terminals) and `shell_tool`. Bare mode turns them off through `CODEX_CONFIG`
(nested objects, never dotted keys) and needs the adapter patch described in `hemera.md` §2.

## Use of a subscription

The Claude Agent SDK documentation says third-party developers may not offer claude.ai login or
its rate limits in their products without approval. Flottille provides no login: it runs the
user's own `claude`, signed in by the user, for that user. Never describe Flottille as a product
that uses its users' subscriptions.
