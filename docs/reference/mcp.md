# Reference: MCP as of late 2026, and how Flottille's two doors use it

Flottille serves MCP twice. The **agent door** serves the tools of the agents Flottille runs, over
Streamable HTTP on `127.0.0.1`, with one bearer token per agent. The **pilot door** is
`flottille mcp`, on stdio, for an agent running outside Flottille. This file records what the
protocol and the main client (Claude Code, which Flottille's own Claude bridge drives in its
stream-json mode) do today, and the rules both doors follow because of it. Checked in October 2026.

## 1. The protocol

- **Current revision: 2026-07-28**; the previous one is 2025-11-25. The main changes:
  - **Stateless**:
    - the `initialize` handshake and the `Mcp-Session-Id` header are gone;
    - every request carries its protocol version and the client's capabilities in `_meta`;
    - servers must answer `server/discover`;
    - state across calls goes in server-minted handles passed as ordinary tool arguments.
  - **Notifications**:
    - `subscriptions/listen` replaces the HTTP GET stream and `resources/subscribe`;
    - `ping` and `logging/setLevel` are removed.
  - **Multi round-trip requests**: a server that needs more input returns
    `resultType: "input_required"` instead of sending its own requests (elicitation, sampling,
    roots). Every result now carries `resultType`.
  - **Tasks** have left the core for an extension (`io.modelcontextprotocol/tasks`).
  - **List caching**: `tools/list` should come in a deterministic order, and list results carry
    `ttlMs` and `cacheScope`.
  - **Streamable HTTP**:
    - POST requests carry `Mcp-Method` and `Mcp-Name` headers;
    - there is no stream resumability any more: a broken stream means the client re-issues the
      request.
  - **Deprecated**: Roots, Sampling, Logging, and the old HTTP+SSE transport.
- **Clients speak both revisions.** Claude Code has two runtimes:
  - v1, on TypeScript SDK 1.x, speaks 2025-11-25 with `initialize`;
  - v2, on SDK 2.0, adds 2026-07-28.

  It picks one at startup and still uses v1 in many sessions. **A Flottille server must accept
  both.**
- **`rmcp`** (the official Rust SDK) is at 3.7.0 (22 October 2026). It handles 2026-07-28 peers,
  `server/discover`, and the `initialize` fallback (`LATEST_WITH_INITIALIZE`). It also offers
  cancellation of abandoned calls, a non-blocking stdio transport, and per-request tool schemas.
  Use the current 3.x.

Sources: [changelog 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/changelog),
[rmcp releases](https://github.com/modelcontextprotocol/rust-sdk/releases),
[Claude Code MCP documentation](https://code.claude.com/docs/en/mcp).

## 2. What Claude Code does with a server's tools

| Behaviour | Value |
|---|---|
| Tool search | On by default: MCP tools are **deferred** and found through a `ToolSearch` step. `ENABLE_TOOL_SEARCH=false` loads them upfront; `ENABLE_TOOL_SEARCH=auto` (or `auto:N`) loads them upfront while their definitions total under 10% of the context window (N percent), and turns search on at that size or more. `_meta["anthropic/alwaysLoad"]: true` on a tool loads that tool upfront; `"alwaysLoad": true` in a server's own entry of the configuration (`.mcp.json`, `--mcp-config`) loads all that server's tools upfront whatever `ENABLE_TOOL_SEARCH` says (it also makes startup wait for the server's tools, 5 s at most) |
| Description cut | 4,096 characters for an upfront tool and for server instructions; 16,384 for a deferred tool. Only the text before the cut is seen |
| Tool name | the full name `mcp__<server>__<tool>` must stay under 128 characters, or the tool is dropped |
| Text result kept in context | up to 50,000 characters, and 25,000 tokens (`MAX_MCP_OUTPUT_TOKENS`, warning at 10,000). Above that, the result is **saved to a file** and replaced by its path. `_meta["anthropic/maxResultSizeChars"]` on a tool raises its threshold, up to 500,000 |
| Error result (`isError`) | over about 11,000 characters, only the first and last 5,000 are kept |
| `readOnlyHint` | read-only calls run in parallel |
| `idempotentHint` / `readOnlyHint` | a call pending when a connection closed may be repeated once |
| Long calls | a call still running after 2 minutes moves to a background task |
| `_meta["anthropic/requiresUserInteraction"]: true` | shows the tool's permission prompt on every call; with `--permission-prompt-tool` (non-interactive mode) the prompt tool's `allow` is turned into a **deny** (`MCP tool requires user interaction; not supported via --permission-prompt-tool`) |

## 3. Tool design: what the guidance agrees on

From Anthropic's [Writing effective tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents)
and the 2026 practice around it:

- **Few tools, built for the job**: not one tool per endpoint. Merge steps that always go
  together, and tools that are near-duplicates, behind an `action` or a detail parameter. Every
  tool listed costs context on every request (100 to 500 tokens each). Around 15 tools is a
  comfortable ceiling for one agent. Anthropic's own line for tool search
  ([Advanced tool use](https://www.anthropic.com/engineering/advanced-tool-use)) is more than 10
  tools or 10k tokens of definitions: below both, loading everything upfront is the right call.
- **Names are a namespace**: prefixes by family (`fs_`, `commands_`, `agent_`), parameters that
  cannot be misread (`agent_id`, not `agent`).
- **Descriptions for a new colleague**:
  - say what the tool does and when to use it rather than another tool;
  - say its side effects and when it refuses;
  - put the important part first;
  - a few hundred characters, rarely more than 1,000;
  - where an argument is easy to get wrong, one or two valid calls, written in the description
    (MCP has no field for the API's `input_examples`);
  - a parameter that asks why a call is made (`why`) asks for **one sentence**, never the
    model's reasoning.
- **High-signal results**: names rather than opaque ids, no low-level fields. A concise default
  with more detail on request. Pagination and limits with defaults, and the way to get the
  next part written in the result.
- **Errors that say what to do next**, with a valid example, never a trace.
- **Measure**: real tasks in an agent loop. Count calls, tokens and errors, and read the
  transcripts.

## 4. Rules for Flottille

### Both doors

1. **Accept 2025-11-25 and 2026-07-28.** Answer `server/discover`. Never depend on a protocol
   session: identity is the bearer token (agent door) or the process (pilot door), and state
   across calls lives in Flottille's own handles (run ids, request numbers, agent ids, event
   sequence numbers).
2. **Fixed tool list, deterministic order**, no `listChanged`. The list is part of the prompt
   cache.
3. **Use no deprecated feature**: no roots, sampling, logging or elicitation. A tool never asks
   for input mid-call; what needs the user is a delivery, later. **MCP tasks** (the
   `io.modelcontextprotocol/tasks` extension, which Claude Code is starting to support) are not
   used: a call that outlasts its wait answers with a run id and is read again, which is what
   `run` and `run_status` do. **`anthropic/requiresUserInteraction` is never set**:
   in a non-interactive session with a permission prompt tool, which is how Flottille drives
   `claude`, it turns every call of the tool into a deny.
4. **Every tool has `title`, a short description following §3, and annotations**:
   - `readOnlyHint` on reads;
   - `idempotentHint` where a repeat is harmless;
   - `destructiveHint` on writes and stops.
5. **Results fit in context.** A tool's normal result stays under 40,000 characters. Pages and
   tails have defaults below that, and the result says how to get the next part. A tool whose
   result can legitimately be larger declares `anthropic/maxResultSizeChars` with its real
   maximum. Error texts stay under 2,000 characters.
6. **Text first.** The result's text is what the model reads. Structured content is added only
   where a client uses it.

### Agent door

7. **Every tool loads upfront.** An agent in bare mode has nothing but Flottille's tools, so it
   must see them without a search step:
   - `_meta["anthropic/alwaysLoad"]: true` on every tool;
   - `"alwaysLoad": true` on the server's entry in the bridge's `--mcp-config`;
   - `ENABLE_TOOL_SEARCH=false` in the agent's environment.

   All three: the entry gives `claude` the server's type, URL, token header and `alwaysLoad`, and
   the variable, which the bridge passes to `claude` from the bare options' `env`, keeps search
   off. Any one keeps the tools upfront; together, a change in `claude`'s defaults (an
   `ENABLE_TOOL_SEARCH=auto` default, say) does not defer them.
8. **A result saved to a file is lost.** An agent in bare mode has no tool to read Claude
   Code's `tool-results` files. Rule 5 is therefore a hard limit here: fs_read pages, search
   results, command tails and agent conversations are cut and paged below the threshold, never
   above it. The reads whose full result can be long (`agent_status` for all the children,
   `forge` and `tickets` `show`) answer **short by default**, with the way to ask for the rest
   written in the short result.
9. **The tool list depends on what the agent can do**, and stays fixed for its life:
   - a child gets no `agent_*` tools, whatever its role;
   - only a root gets `approval_answer`;
   - a root gets `ask_user`, a child gets `ask_parent`.

   - `forge` is listed to every agent whose project's repositories declare a forge, `tickets`
     to every agent whose project declares a tracker, in `main` as in a workspace, and
     `workspace` to every agent: the same tools wherever it works. A tool that could only refuse
     is not listed: without a forge or a tracker there is nothing for them to read or write.
     Listing grants nothing: each call goes through the gate's classes and the session's mode
     (`docs/tools.md`, "The gate"). In `main` they name the workspace they act on (`workspace`).

   - only a root gets `skills`, `commands`, `setup` and `project`, the project's skills,
     commands, setup and settings, writes its tasks and its workspaces, and starts sessions for
     the user (`agent_launch` with `root`) (`docs/tools.md`, "The project", "The workspace",
     "Sessions for the user"): a child does its brief and reads the workspaces.
     Every agent gets `tasks`, `run` and `run_status`, the names `commands_list`,
     `commands_run` and `commands_status` had until #131, and `forge`, `tickets` and
     `workspace` `remove`, the names `forge_read`, `forge_write`, `ticket_read` and
     `workspace_remove` had until #141; the old names are still served under the new ones for
     one release.

   A root lead has 22 tools, a leaf worker 12; one more each when the project's repositories
   declare a forge (`forge`), one more when it declares a tracker (`tickets`).

   **The budget.** Every tool loads upfront (rule 7), so the definitions must stay under
   Anthropic's line for tool search (§3: more than 10 tools or 10k tokens). Flottille's lists
   pass 10 tools, which is why each family is one tool with an `action` rather than one tool per
   verb; the tokens are the line kept. JSON schemas tokenize densely, about 3 characters a token
   at worst and 4 usually: **28,000 characters** of definitions is under 9,400 tokens at worst,
   and a test holds the biggest list, a root's with a forge and a tracker, under it. Measured
   with that test (`the_biggest_list_stays_far_under_the_threshold_of_tool_search`):

   | | Tools | Characters |
   |---|---|---|
   | Before #141 (forge, ticket and removal tools apart, a 27,000 limit) | 25 | 26,889 |
   | After #141 (`forge`, `tickets`, `workspace` complete families) | 23 | 25,236 |

   A family grows by an action and a line of description, not by a tool; a new tool or a long
   description that passes the limit is answered by merging or shortening, and the limit moves
   only with the threshold it stands for.

   **Every family is complete** (#141): what the window lets the user list, search, read,
   create or change, a family lets the agent do under the gate, or this table says why not.

   | Family | List | Search | Read | Create | Change | Left out, and why |
   |---|---|---|---|---|---|---|
   | `forge` | `list` (state, branch) | `list` by branch | `show` (a workspace's branch, or a number) | `create` | `comment`, `edit`, `merge`, `checks` | Closing a pull request: the window has no such action, and closing someone's work is the user's; deleting a branch is `workspace` `remove`'s, for the workspace's own. |
   | `tickets` | `list` (state, label, assignee), every ticket in one call | `search` | `show` | `create` (GitHub) | `comment`, `edit`, `close`, `reopen` (GitHub) | Every write on Jira: Flottille reads Jira and writes nothing to it, by the user's decision; Bitbucket's issues: no tracker of that kind is declared today. |
   | `workspace` | `list` | `list` (few, a line each) | `show` | `create` | `prepare`, `link_ticket` (null unlinks), `remove` | Renaming: a workspace's name is its branch and its folder, fixed when it is made, in the window as here. |
   | `skills` | `list` | `list` (few) | `show`, and `skill_read` for the text | `add` | `edit`, `remove` | Nothing. |
   | `commands` | `list` | `list` (few) | `show` | `add` (own words or a template) | `edit`, `remove` | Nothing. |
   | `tasks` | `list` | `list` (few) | `show` | `add` | `edit`, `remove` | Nothing. |
   | `setup` | `list` (each step whole) | `list` | `list` | `add` | `edit`, `remove`, `move` | A `show` of one step: `list` gives each whole. |
   | `project` | — | — | `settings`, `rules` | — | `set_settings`, `set_rules` | Listing, creating or removing projects: an agent works in its own project only, and another project is the user's to register. |
   | `run`, `run_status` | — | — | `run_status` `output` | `run` | `run_status` `stop` | A list of the agent's runs: each run's id is in the answer that started it and in the delivery that says it ended, and the window lists them for the user. |
   | `agent_*` | `agent_status` (every child) | — | `agent_status` (one, or `conversation`) | `agent_launch` (a child in the caller's place, or a session for the user with `root`, in a workspace) | `agent_send`, `agent_stop`, `session_send` (a session the caller started, or the one that started it) | Changing a child's model or mode: a child keeps what it was launched with, and only the user sets a mode. |

10. **The idempotency key** of a call is read from the call's `_meta` (`claudecode/toolUseId` for
    Claude). `claude` calls Flottille's server itself and sends it with every call; the bridge
    never sees MCP calls. Check that the `claude` version Flottille pins still sends it, and fall
    back to no deduplication, said once per agent in the daemon's log, when a call has none.

11. **The server explains Flottille to the agent.** The `initialize` result (and `server/discover`'s)
    carries the `instructions` of `crates/flottille/roles/guide.md`: under 400 words, well under
    the 4,096 characters Claude Code keeps, one text for every agent. Anthropic's guidance asks
    that a tool's description tell a new colleague what the tool is for; an agent that is told
    nothing of the system around its tools invents it (it claimed that tools "may appear during the
    session"). It says only what holds in the code, and a test checks that every tool it names
    exists.

### Pilot door

12. **The outside agent probably uses tool search**, so the pilot door's server instructions
    (under 4,096 characters) say what Flottille is and when to reach for its tools: start and
    follow agents, read their state and conversations, and catch up with events.
13. **Waiting without polling**: `events_since {after, wait_seconds?}` holds the call up to
    `wait_seconds` (at most 90, under Claude Code's 2-minute backgrounding) and returns as soon
    as an event arrives, with the next sequence number to ask for.
