# What the tool list costs, and what loading on demand would save

Research for #676. **No tool changes in it**: the maintainer decided on 2026-11-09 that Flottille's
own tools keep their whole description and schema for now, and that loading on demand is for
connections only (`docs/extensions.md`, Connections). This note is the measure that decision
waits on, so that the question can be reopened with numbers instead of a feeling. Measured in
November 2026, on the code of `dev` at 085d955.

## In short

- **The list an agent is served is 4,600 to 11,700 tokens** (o200k), by kind of agent and project:
  a worker 4,564 to 7,461, a root 7,807 to 11,691. About 95% of it is descriptions and schemas;
  the schemas alone are 59%. Claude counts more tokens than o200k for the same text: read the
  Claude figure as **between 1 and about 1.5 times** these (see "How it was counted").
- **Two tools are over a quarter of a root's list** (27%, and 39% of a worker's): `forge` (1,575
  tokens, 84% of it its schema) and `tickets` (1,322). Every other tool is under 750.
- **Slimming has a ceiling and a price.** Slimming every tool that is not a core one to one line
  and a names-only schema (what Connections' tools do) saves 3,000 to 6,200 tokens of the list,
  35 to 59%. In cache-weighted terms of a Claude session that is **about 4 to 8% of the input
  cost of a 30-request session**, and the list is read in every request, so the figure grows with
  the session. It costs one extra call before a first use, and a model that errs meets refusals
  that, in 9 of the 39 wrong calls tried, do not name what it should have sent.
- **Nothing here can say how often a model gets a call wrong without the schema in view.** A fake
  agent plays a script; it cannot be mistaken the way a model is. What can be measured is
  measured (the answers to wrong calls, the cost of the help round trip); the rest needs a real
  run, and a protocol for it is at the end.
- **Recommendation: keep the decision, and do the two things that cost nothing in agent
  quality**: correct the stale counts in `docs/design.md` §4.3 and make the refusals of the
  family tools name their actions (they are a dead end today the moment the schema is out of
  view, and that helps the schema-in-view case too). If the list is ever to be slimmed, start with
  `forge` and `tickets` alone, at the names-only level: 1,794 tokens, no tool the agent uses every
  session.

## How it was counted

- **The lists are the real ones.** `crates/flottille/tests/tool_list_cost.rs` starts a daemon with
  the fake ACP agent, in temporary folders, three throwaway projects (nothing declared; a GitHub
  forge and a Jira tracker declared; the same with Specs and Review on and a workspace), starts
  each kind of agent, and calls `tools/list` on the MCP server the daemon gave its session, over
  HTTP, as the agent's own client would. With `FLOTTILLE_TOOL_LISTS_OUT=<file>` it writes the
  lists as JSON. No real agent runs, no real configuration is read.
- **The tokenizer is `o200k_base`**, OpenAI's, from the `tiktoken-rs` crate, which compiles the
  table in: it counts offline and calls no API. It is Codex's tokenizer. **No Claude tokenizer
  runs offline** (Anthropic's only exact count is its `count_tokens` endpoint, a call to a paid
  API, which this issue excludes), so the Claude figure is an estimate:
  - Anthropic says its newer tokenizer takes about 1.0 to 1.35 times the tokens of the previous
    one for the same text, and independent measures on technical text found 1.2 to 1.47
    (Anthropic's migration notes, as quoted in secondary reports; Simon Willison measured 1.46 on a
    system prompt, [here](https://simonwillison.net/2026/apr/20/claude-token-counts/)).
    How far either is from o200k is not published. **Read Claude's tokens as the o200k figure
    times 1.0 to 1.5, unverified.**
  - What does not depend on it: the shares (this tool against the list, a schema against a
    description) and the proportions of every saving, since the same text is counted on both
    sides. The absolute figures are the soft part.
  - One run of Claude Code's `/context` in a session of each kind gives the real per-tool table
    (reportedly `claude -p "/context"` prints it) and settles the
    factor. It is a real `claude` run, so it is not done here.
- **What is counted of a tool** is what a model is given: its name with the provider's prefix
  (`mcp__flottille__`, the same for Claude and Codex), its description and its input schema, as
  one compact JSON object. `title`, the annotations and `_meta` are the client's and are not
  counted. Claude Code and Codex both turn an MCP tool into a function definition of these three
  fields; the wrapper each puts around them is not measured here, and is a few tokens a tool.
  Codex has no tool of its own listed beside Flottille's (the bridge switches them off), so its
  list is these lists; whether it defers any is not verified.
- **Reproduce**:

  ```
  FLOTTILLE_TOOL_LISTS_OUT=/tmp/dump.json cargo test -p flottille --test tool_list_cost
  cd tools/tool-list-cost
  cargo run --release -- lists /tmp/dump.json                  # the tables of this note
  cargo run --release -- lists /tmp/dump.json forge,tickets    # what slimming only those saves
  cargo run --release -- misuse /tmp/dump.json                 # the answers to wrong calls
  flottille watch --after 0 > events.jsonl                     # then Ctrl-C
  cargo run --release -- usage events.jsonl                    # which tools your sessions call
  ```

  `tools/tool-list-cost` is a crate of its own (its own workspace: the daemon never builds the
  tokenizer). Its tests run on invented tools: `cargo test --manifest-path
  tools/tool-list-cost/Cargo.toml`.

## 1. What each kind of agent is served

Tokens of the whole list, at o200k. A root's tools are `docs/design.md` §4.3's; the table shows
what each kind really gets today.

| Kind | Project | Tools | Characters | Tokens | Descriptions | Schemas |
|---|---|---|---|---|---|---|
| root | nothing declared | 27 | 32,251 | 7,807 | 3,241 | 4,005 |
| root | forge and tracker | 29 | 44,291 | 10,704 | 3,730 | 6,371 |
| led session (a root an agent started) | forge and tracker | 29 | 44,349 | 10,712 | 3,739 | 6,371 |
| root, Specs and Review on | forge and tracker | 31 | 48,356 | 11,691 | 4,193 | 6,857 |
| worker | nothing declared | 15 | 18,710 | 4,564 | 1,926 | 2,330 |
| worker, also with the role `lead` | forge and tracker | 17 | 30,750 | 7,461 | 2,415 | 4,696 |
| worker, Specs and Review on | forge and tracker | 17 | 30,750 | 7,461 | 2,415 | 4,696 |
| review pilot | forge and tracker | 17 | 30,916 | 7,513 | 2,401 | 4,770 |
| review pass | forge and tracker | 16 | 29,297 | 7,102 | 2,228 | 4,552 |

What this says:

- **The list depends on the place in the tree, never on the role's name.** A child launched with
  `role: lead` has exactly a worker's tools; a led session has a root's (a few characters
  apart: `ask`'s description says where the question goes). Both are tests.
- **Specs and Review add 987 tokens to a root and nothing to a worker.** The review pilot is a
  worker plus `review` less `ask`; a pass is a worker less `ask`. No extension's tool is in a
  worker's list.
- **A forge and a tracker add 2,897 tokens to a root and 2,897 to a worker**: they are the same
  tools wherever the agent works (`docs/reference/mcp.md` §4, rule 9), and they are the two
  heaviest.
- **Of a list, 5.5% is name and syntax**: the rest is text. The 4.1 characters a token measured
  here (48,356 characters, 11,691 tokens) is above the 3 the size test counts at
  (`the_biggest_list_stays_under_the_guards_on_tools_and_tokens`): the biggest list is 11.7k
  tokens of its 20k guard in o200k, and under the guard in Claude's if its factor is under 1.7.
- **The counts in `docs/design.md` §4.3 and `docs/reference/mcp.md` §4 are stale**: they say a
  root has 27 tools with a forge and a tracker, a child 21 and a leaf worker 16 (and 25, 19 and
  14 with neither), from before `services`, `wake`, `session_*` and the depth limit of 1 (a
  child is a leaf, `DEPTH_LIMIT`). The real counts are above. Not fixed here: this issue changes
  nothing but this note and the measuring tool.

### Per tool

The biggest list (a root with a forge, a tracker, Specs and Review: 11,691 tokens). "Whole" is
what the list holds today and what a `help` answer would cost to give back in full; the last
three columns are the tool at each lighter level (defined below).

| Tool | Whole | Description | Schema | Share | No schema texts | One line | `action` and `arguments` |
|---|---|---|---|---|---|---|---|
| `forge` | 1,575 | 236 | 1,317 | 13.5% | 797 | 585 | 67 |
| `tickets` | 1,322 | 253 | 1,049 | 11.3% | 662 | 435 | 67 |
| `agent_launch` | 728 | 242 | 459 | 6.2% | 408 | 197 | 79 |
| `services` | 603 | 250 | 330 | 5.2% | 447 | 227 | 74 |
| `spec` | 576 | 290 | 268 | 4.9% | 478 | 205 | 56 |
| `tasks` | 540 | 119 | 401 | 4.6% | 331 | 245 | 74 |
| `workspace` | 465 | 205 | 239 | 4.0% | 314 | 111 | 44 |
| `project` | 460 | 159 | 277 | 3.9% | 322 | 161 | 43 |
| `review` | 411 | 173 | 218 | 3.5% | 411 | 256 | 59 |
| `run_status` | 398 | 240 | 136 | 3.4% | 336 | 129 | 76 |
| `run` | 387 | 194 | 169 | 3.3% | 285 | 123 | 77 |
| `ask` | 355 | 182 | 144 | 3.0% | 330 | 183 | 85 |
| `setup` | 267 | 114 | 131 | 2.3% | 202 | 98 | 53 |
| `agent_status` | 267 | 134 | 112 | 2.3% | 214 | 84 | 46 |
| `session_read` | 261 | 102 | 140 | 2.2% | 176 | 97 | 63 |
| `web` | 252 | 142 | 92 | 2.2% | 229 | 115 | 67 |
| `fs_edit` | 243 | 67 | 157 | 2.1% | 185 | 154 | 76 |
| `fs_read` | 234 | 74 | 140 | 2.0% | 166 | 114 | 63 |
| `skill_read` | 234 | 100 | 115 | 2.0% | 191 | 122 | 71 |
| `session_send` | 233 | 140 | 74 | 2.0% | 192 | 85 | 73 |
| `search` | 232 | 79 | 133 | 2.0% | 144 | 90 | 66 |
| `commands` | 218 | 66 | 132 | 1.9% | 163 | 121 | 65 |
| `skills` | 198 | 62 | 115 | 1.7% | 148 | 94 | 50 |
| `fs_list` | 192 | 67 | 106 | 1.6% | 132 | 106 | 81 |
| `agent_send` | 188 | 70 | 98 | 1.6% | 140 | 76 | 47 |
| `fs_write` | 186 | 65 | 102 | 1.6% | 131 | 84 | 58 |
| `approval_answer` | 153 | 64 | 70 | 1.3% | 138 | 114 | 80 |
| `agent_stop` | 144 | 99 | 26 | 1.2% | 144 | 66 | 61 |
| `session_title` | 130 | 68 | 41 | 1.1% | 113 | 76 | 73 |
| `report` | 125 | 83 | 24 | 1.1% | 125 | 73 | 70 |
| `wake` | 114 | 54 | 42 | 1.0% | 114 | 79 | 58 |

Three things stand out. **The weight is in the schemas of the family tools**: `forge` and `tickets`
carry 19 and 20 actions and every argument of every action, in one flat schema. **The schema
texts are about half of the schemas**: the "No schema texts" column keeps every name, type and
enum and loses only the per-property sentences. **A tool under 250 tokens gains nothing worth a
call**: `fs_read` is 234 whole and 114 at one line; the call that would fetch the rest costs
more than the 120 it saves.

## 2. Which tools are used, and which rarely

**Not measured, and this is the gap in the note.** The daemon records every call as an
`agent.tool_served` event (`docs/events.md`), and the usage of the maintainer's own sessions is
in their store, which this work never reads (the tests and the research use temporary data only).
`tool-list-cost usage` reads the output of `flottille watch --after 0` and prints, per tool, the
calls, the sessions that called it (as roots and as children), the refused calls and the sessions
whose first call of the tool was refused: tool names and counts, no argument. It was run on the
events of the test daemon (its parsing is a test, and a test of the events says what it reads)
but those events are scripted and say nothing of real usage. **One run of it on the maintainer's
store replaces the table below.**

What can be said without it is what each tool is for, from the roles and the design. A judgement,
not a count:

| Used | Tools | Why |
|---|---|---|
| Every session | `fs_read`, `fs_list`, `search`, `fs_edit`, `fs_write`, `run`, `run_status`, `report` (workers), `ask` | They are the work. 2,350 tokens in all: not a candidate. |
| A root's every session | `agent_launch`, `agent_send`, `agent_status`, `agent_stop`, `session_send`, `session_title`, `approval_answer`, `wake` | The orchestration standard names them. 1,950 tokens. |
| Most sessions of a project that declares them | `forge`, `tickets`, `workspace`, `tasks` | A worker closing a ticket reads, opens a pull request, runs a task. Heavy (3,900 tokens with `forge` and `tickets`), and used by the work the projects are for. |
| Some sessions | `skill_read`, `web`, `services`, `session_read` | When a skill is named, a page is needed, a server is to start. |
| Rare, a root only | `skills`, `commands`, `setup`, `project` (the project's own configuration), `spec`, `review` | What the window does for the user; the user asks the agent for it from time to time. 2,100 tokens in all, in every root's list. |

The row that would pay is the last: roughly 2,100 tokens, paid by every root for work most
sessions never ask. The fourth row is the second candidate; `forge` and `tickets` are the heaviest
and the likeliest to be used.

## 3. The other side: what loading on demand costs

### The levels tried

Four forms of a tool, from today's to what Connections does (`tools/tool-list-cost`,
`slimmed`):

| Level | What the list keeps of a tool |
|---|---|
| Whole (today) | the description and the schema |
| No schema texts | the whole description; the schema's names, types, enums and `required`, without the sentence of each property |
| One line | the first sentence of the description (160 characters at most); the same schema |
| `action` and `arguments` | the first sentence; a schema that takes an `action` and an `arguments` object, as Connections' tool does |

Any slimming needs a `help` tool: `help {tool, action?}` answers the whole description and the
arguments. Its own definition is counted in every figure below (155 tokens for the 31 names of the
biggest list).

Every tool of the biggest list slimmed at each level, `help` included:

| Level | Tokens | Saved |
|---|---|---|
| Whole (today) | 11,691 | 0 |
| No schema texts | 8,323 | 3,368 (28.8%) |
| One line | 4,860 | 6,831 (58.4%) |
| `action` and `arguments` | 2,177 | 9,514 (81.4%) |

The last row is a ceiling, not a design: no core tool would be slimmed, and a model that has no
argument names must call `help` before every first use of every tool.

### The extra call

- A first use of a slimmed tool is a call more: the call (about 40 to 60 tokens: not measurable
  offline, an assumption) and the answer, which is the tool's whole text, **up to the "Whole"
  column**: `forge` 1,575, `tickets` 1,322. An answer per action would be smaller (a `forge`
  `merge` is a fraction of its 19 actions) and is a design of its own.
- **The help answer is in the conversation, the list is in the prefix.** Both are cached and read
  at every following request, but the help answer starts at the request where it was asked, and
  the list at the first. A slimmed tool never used saves for the whole session. Used once, at
  "one line" it loses (the list keeps 585 tokens of `forge`, and the help answer then gives the
  1,575 again); at `action` and `arguments` it can still win, because the list kept 67 and the
  answer is paid from the 10th request only: at the price of no argument name in view.
  A **compaction** (`docs/design.md` §5.8) rewrites the conversation and drops the answer, so an
  agent that compacts asks again; the list survives it.
- **In the cache-weighted terms of a Claude session** (an input token costs 1.25 to write and 0.1
  to read, the published multipliers of the 5-minute cache; so a listed token costs 1.25 + 0.1 ×
  (N − 1) over N requests: 4.15 for 30 requests, 11.15 for 100), for 30 requests, help asked at
  the 10th:

  | Tool, level | Never used | Used once | Pays while used in under |
  |---|---|---|---|
  | `forge`, one line | +4,108 | −1,070 | 79% of sessions |
  | `tickets`, one line | +3,681 | −675 | 84% |
  | `tasks`, one line | +1,224 | −591 | 67% |
  | `services`, one line | +1,560 | −459 | 77% |
  | `project`, one line | +1,241 | −314 | 80% |
  | `spec`, one line | +1,540 | −392 | 80% |
  | `review`, one line | +643 | −752 | 46% |
  | `forge`, `action` and `arguments` | +6,258 | +1,079 | always |
  | `commands`, `action` and `arguments` | +635 | −134 | 83% |

  A tool is slimmed at "one line" for as long as it is used in under about 80% of sessions: it
  is a wash for the ones that use it and a gain for the ones that do not. **For the whole of a
  session the sums are small**: a 30-request session of a worker with 45k tokens of average
  context costs about 190k token-equivalents, its tool list 31k of those (16%), and slimming
  every non-core tool to one line saves 12.7k (6.7%).

### How often a model gets a call wrong without the schema

**What a fake agent can measure** is what the server answers to a wrong call, which is what a
model meets. The test calls every tool of a root with no argument, and every tool that has an
`action` with one that does not exist, 39 calls in all, and the report says whether the answer
names what the call should have said (`tool-list-cost misuse`):

| Mistake | Calls | Refused | The answer names what to do | Mean answer, tokens |
|---|---|---|---|---|
| no arguments | 25 | 25 | 19 | 22 |
| an action that does not exist | 14 | 14 | 11 | 28 |

An answer is cheap (22 to 28 tokens) and always an error the model can act on. Whether it
names what to send is counted for what the call lacked (`expect`: the missing fields, or the
actions): **9 of the 39 do not**. `fs_write`, `fs_edit`, `agent_launch`, `agent_send`,
`session_send` and `approval_answer` called with nothing name only the first missing field
(`missing field \`path\``), though the call lacked two or three, and send the model to "its input
schema"; `run_status`, `web` and `review` reject an unknown action without listing the valid ones.
And the count is kind to the seven action families `tasks`, `workspace`, `web`, `skills`,
`commands`, `setup` and `project`: for a call with no `action` they answer `missing field
\`action\``, which names the field and not the actions it takes, and the count calls that named.
Only the hand-written refusals (`forge`, `tickets`, `services`, `spec`) list the actions:
`forge has no action "": its actions are list, show, subscribe, …`. **Today's refusals assume the
schema is in view.** If it were not, each would be a wasted call before `help`.

**What it cannot measure** is how often a model, given a one-line description and no schema,
sends a call that is wrong in a way its refusal does not mend: a field name guessed (`file`
for `path`), a value of the wrong type, an argument put on the wrong action, a required one
forgotten that the server cannot tell is required. A fake agent plays its script and is never
wrong. To know, a real run is needed, which the maintainer decides:

1. Ten tasks that each need one slimmed tool with its harder arguments (`forge create` with a
   base and a body, `tickets edit` with labels to add and remove, `tasks add`, `services start`),
   a fresh session per task, against the fake `gh` and `acli` the tests already use.
2. Each task twice, with the tool whole and with it slimmed at "one line", on one model of each
   provider, five times each (the model is not deterministic): 200 sessions.
3. Count the calls, the refused calls, the `help` calls and the tokens (`agent.tool_served`
   gives the first three; `turn_usage` the last), and read the transcripts of the failures.
4. It costs the maintainer's quota, so it is run once, on one model, and reported.

## 4. Proposal

Numbers for a worker and a root in a project with a forge and a tracker, `help` included.

| What is slimmed | Level | Root, forge and tracker | Worker, forge and tracker | Root, nothing declared |
|---|---|---|---|---|
| nothing | whole | 10,704 | 7,461 | 7,807 |
| `forge`, `tickets` | one line | −1,794 (17%) | −1,794 (24%) | 0 |
| `forge`, `tickets` | `action` and `arguments` | −2,680 (25%) | −2,680 (36%) | 0 |
| `tasks`, `services`, `workspace`, `skills`, `commands`, `setup`, `project`, `skill_read`, `web` | one line | −1,845 (17%) | −1,184 (16%) | −1,845 (24%) |
| the two rows above together | one line | −3,718 (35%) | −3,057 (41%) | −1,845 (24%) |
| the two rows above together | `action` and `arguments` | −5,357 (50%) | −4,433 (59%) | −2,598 (33%) |

(`spec` and `review`, where given, would add 522 tokens at "one line" and 868 at `action` and
`arguments` to a root's figures. A percentage is of the list, not of the session: for the
session, see the 4 to 8% above.)

What I would do, in order of what it risks:

1. **Nothing to the tools**, per the decision of 2026-11-09. The saving of the whole programme is
   4 to 8% of a session's input cost; the loss is the guidance and the examples that issue #264
   restored after they were trimmed to fit, and a failure mode (a wrong call without the schema)
   that this note could not measure.
2. **Without touching a tool, now**: correct the counts in `docs/design.md` §4.3 and
   `docs/reference/mcp.md` §4; and make the refusals of the family tools name their actions
   and, for a missing field, the fields the action takes. They help every agent, and they are the
   precondition of any later slimming. (Two small issues, not done here.)
3. **If the list is slimmed one day**, `forge` and `tickets` alone, at "one line" (names and enums
   stay, per-property sentences go): 1,794 tokens, 17 to 24% of a list that has them, without a
   core tool touched. Not at "action and arguments": that gives up the argument names, and the
   model's first call of every use becomes a `help`. Then the real run above, before anything
   else is slimmed.
4. **Not recommended**: slimming the tools every session uses (2,350 tokens in all, under 400
   each), or the root-only project tools without a measure of how often roots use them
   (`tool-list-cost usage` on the maintainer's store: one command).

### Risks

- **A wrong call the refusal cannot mend**: the one thing this note could not measure. Mitigation:
  the refusals above, and the real run.
- **Guidance lost**: the descriptions hold when to use a tool, its side effects, an example
  (`docs/reference/mcp.md` §3). A first sentence keeps what the tool is, not when not to use it.
  A tool slimmed would need the sentence written for the list, not cut from the description.
- **The gate does not change**: a call goes through the gate whatever the list says, so slimming
  grants nothing. What `help` answers is Flottille's own text, not a third party's, unlike
  Connections' `help` (`#560`: framed and cut as third-party data).
- **The prompt cache**: a list is the prefix of the cache and changes only with the role
  (`docs/reference/mcp.md` §4, rule 2). A `help` answer is in the conversation, after the prefix:
  it does not break the cache, and compaction forgets it (above).
- **Codex and OpenCode**: Codex counts in the same tokenizer and has no deferred loading we rely
  on; OpenCode was not measured (its tools are `flottille_<tool>`, a shorter prefix). The cache multipliers above are Claude's; the other providers' discounts differ,
  so the session percentages are Claude's.
- **A tool with an `action` is already "loaded on demand" in one respect**: its description says
  the actions, and its schema lists them. Slimming moves the argument sentences, not the
  existence of the actions.

## What was not verified

- Claude's token counts (the factor above); Codex's wrapper around a function definition.
- Which tools sessions really use (`tool-list-cost usage`, on the maintainer's store).
- How often a model errs without the schema (a real run, the protocol above).
- A tool list with a vault entry (`vault`, one more tool for the agents of a project that sees an
  entry): not in the measured kinds.
- OpenCode's lists (same tools, other prefix) and a Codex session.
