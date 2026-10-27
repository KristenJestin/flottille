# Events

Every change the daemon makes emits an event. Events are stored, numbered, and streamed to the
subscribers of `events.watch` (`docs/protocol.md`); `flottille watch [--after <n>]` prints them,
one JSON object per line. This file is the contract: an event is added here with the code that
emits it.

## Shape

```json
{"seq": 12, "at": 1792843730808, "name": "daemon.started", "data": {"pid": 4242, "version": "0.1.0"}}
```

| Field | |
|---|---|
| `seq` | The event's number. Grows with every event, across daemon restarts; a number is never given twice. |
| `at` | When it happened, in milliseconds since the Unix epoch. |
| `name` | `<subject>.<what happened>`. |
| `data` | An object, its fields given per event below. A client ignores fields it does not know. |

## The events

### `daemon.started`

The daemon is listening.

| Field | |
|---|---|
| `version` | The daemon's full version. |
| `pid` | Its process id. |

### `daemon.stopping`

The daemon stops: it accepts no new connection and closes the existing ones once their
subscribers have this event.

| Field | |
|---|---|
| `reason` | `request` (`flottille daemon stop`), `signal` (`SIGTERM`, `SIGINT`), or `socket` (the socket at its path is gone or is another daemon's: its runtime folder was removed; checked every 2 seconds). |

### `project.added`

A project was registered (`project.add`), or its file appeared in the configuration folder.

| Field | |
|---|---|
| `name` | The project's name. |
| `folder` | Its folder. |

### `project.changed`

The project's file was edited: the daemon noticed it at its next request about projects.

| Field | |
|---|---|
| `name` | The project's name. |
| `folder` | Its folder. |

### `project.removed`

The project was removed (`project.remove`), or its file is gone. Only Flottille's configuration is
removed, never the folder.

| Field | |
|---|---|
| `name` | The project's name. |

A project already there when the daemon starts is not announced, and a file that cannot be used is
neither a change nor a removal: the project it was stays as it was until the file is fixed or taken
away.

### `provider.changed`

A provider was enabled or disabled through the daemon (`provider.enable`, `provider.disable`).
An edit of `providers.toml` by hand is not announced: it is read again at the next check.

| Field | |
|---|---|
| `name` | The provider's name. |
| `enabled` | Whether it is enabled now. |

### `workspace.created`

A workspace was written, before anything is made on disk (`docs/workspaces.md`). Every event of a
workspace has `project` and `workspace` (its name).

| Field | |
|---|---|
| `project` | The project's name. |
| `workspace` | The workspace's name. |
| `branch` | Its branch. |
| `folder` | Its folder. |
| `repositories` | Each `{"path", "base"}`, as in a workspace (`docs/protocol.md`). |

### `workspace.step_started`

A step of the preparation started.

| Field | |
|---|---|
| `project`, `workspace` | |
| `position` | The step's place, from 0. |
| `kind` | `worktree`, `copy`, `link`, `root_entries` or `run`. |
| `repository` | Where it is done, `null` for the workspace folder. |
| `doing` | A command-like description of it. |

### `workspace.step_done`, `workspace.step_skipped`, `workspace.step_failed`

The step ended: done, skipped (its repository is not in the workspace), or failed. The same fields
as `workspace.step_started`, and, for a failure, `output`: its last lines, masked. A failure stops
the preparation.

### `workspace.ready`

Every step is done or skipped.

| Field | |
|---|---|
| `project`, `workspace` | |

### `workspace.resumed`

Steps were put back to pending to be done again (`workspace prepare`, or the daemon starting while
a workspace was preparing).

| Field | |
|---|---|
| `project`, `workspace` | |
| `redone` | Done steps whose result is gone from the disk. |
| `retried` | Failed or running steps. |

### `workspace.forge`

What the forges say of the workspace changed: a pull request, its state or its checks, or what
stops them being read ([`forges.md`](forges.md)). Its data is the whole state, as
`workspace.forge` answers it (`docs/protocol.md`), `project` and `workspace` included. A first read
that finds no forge says nothing.

### `workspace.ticket`

The workspace was linked to a ticket, or unlinked ([`tickets.md`](tickets.md)).

| Field | |
|---|---|
| `project`, `workspace` | |
| `ticket` | `#123` or `ABC-123`; `null` when unlinked. |

### `workspace.removed`

The workspace's worktrees, folder and record are gone. Its branches are kept.

| Field | |
|---|---|
| `project`, `workspace` | |

### The events of an agent

Every event of an agent has `agent`, its id (`docs/agents.md`), and is part of its conversation
(`agent.show`). The events made from what the agent sends also have `replay`, always false: what a
`session/load` replays is not stored again (`docs/agents.md`, "The conversation").

#### `agent.created`

An agent was written, before its process starts.

| Field | |
|---|---|
| `agent` | Its id. |
| `project` | Its project. |
| `workspace` | Its workspace, `null` for the project folder. |
| `folder` | Where it works. |
| `provider` | `claude`. |
| `model` | The model asked, `null` for the agent's default. |
| `role` | `lead` or `worker`. |
| `parent` | The agent that launched it, `null` for a root. |
| `depth` | 0 for a root, its parent's plus one for a child. |
| `title` | Its task's title, `null` for a root. |

#### `agent.started`

Its process runs and its session is open, its options set.

| Field | |
|---|---|
| `agent` | |
| `pid` | The agent's process id (the Claude bridge's). |
| `opened` | `new`, `resumed` or `loaded`. |
| `native_id` | The ACP session id. |
| `agent_version` | The agent's name and version, as its `initialize` gives them (`flottille-claude-bridge` and Flottille's version). |
| `taken` | `{"model", "effort", "mode"}`: what the agent took. |

#### `agent.prompted`

A turn starts with this message.

| Field | |
|---|---|
| `agent` | |
| `text` | The message. |

#### `agent.user_message`, `agent.message`, `agent.thought`

A piece of text of the user (seen in a replay), of the agent's reply, or of its thinking, as it
streams: the pieces of one reply follow each other. The pieces of a reply are the daemon's, not
the provider's: it masks the reply whole and keeps what is settled, at most 2,000 characters a
piece (`docs/agents.md` "The conversation").

| Field | |
|---|---|
| `agent`, `replay` | |
| `text` | The piece, cut at 64 KiB (`‹+N chars›` says how much was left out). |
| `message_id` | Of `agent.message`, when the source gives one (ACP's `messageId`; Codex's item id; the API message's id for Claude): the pieces that share it are one message, another id starts another. |

#### `agent.tool_call`

A tool call the agent makes, or a change of one (`update`).

| Field | |
|---|---|
| `agent`, `replay` | |
| `id` | The call's id. |
| `update` | False when the call appears, true for a change of it. |
| `title`, `kind`, `status`, `locations`, `content`, `raw_input`, `raw_output` | As the agent sent them, each only when it did; `status` is `pending`, `in_progress`, `completed` or `failed`. |

#### `agent.compacted`

The agent compacted its context.

| Field | |
|---|---|
| `agent`, `replay` | |

#### `agent.plan`, `agent.usage`, `agent.mode_changed`, `agent.options_changed`

The agent's plan (`entries`), its context use (`used` and `size` in tokens, `cost`), its mode
(`mode`), its options (`options`, as ACP gives them). Each with `agent` and `replay`.

#### `agent.provider_wait`, `agent.provider_limit`, `agent.provider_error`

The provider makes the agent wait, refuses it for a limit, or fails its turn for another reason
once its retries are spent (`message`, when it says one). Each with `agent` and `replay`. A limit
or an error ends the turn (`agent.turn_ended`): the agent lives on and takes the next message.

#### `agent.permission_answered`

The agent asked its own permission for a tool, answered at once.

| Field | |
|---|---|
| `agent`, `replay` | |
| `tool` | The tool's name. |
| `allowed` | True for one of the agent's Flottille tools, false for anything else. |

#### `agent.tool_served`

Flottille's MCP server answered one of the agent's tool calls (`docs/tools.md`). A sign of
activity.

| Field | |
|---|---|
| `agent` | |
| `tool` | The tool's name, without the `mcp__flottille__` prefix. |
| `call_key` | The call's key (`claudecode/toolUseId`), `null` when the call had none. |
| `arguments` | The arguments, every text cut at 500 characters and masked. |
| `gate` | The gate's verdict on the call (`allow`, `deny` or `ask`), `null` for a call refused before it, because the agent has no such tool. |
| `outcome` | `ok`, `refused`, `failed`, or `held` for a call the gate held (`docs/tools.md`, "The gate"). |
| `duration_ms` | How long it took. |
| `request` | For a call the user allowed, performed once they did: the request's number; else `null`. |

#### `approval.requested`

The gate held one of the agent's calls: a request was written (`docs/tools.md`, "The gate").

| Field | |
|---|---|
| `number` | The request's number, counted across every agent. |
| `agent` | The agent that asked. |
| `call_key` | The call's key, `null` when it had none. |
| `tool` | |
| `call` | The arguments, every text cut at 500 characters and masked. |
| `reasons` | Why it was held, one sentence each, masked. |
| `sensitive` | Whether it reaches a sensitive place: it is then never allowed for the tree. |
| `held_by` | `user`, or the id of the agent that holds it. |
| `forwards`, `state`, `decision`, `created_at`, `decided_at` | As in `approval.list` (`docs/protocol.md`). |

#### `approval.forwarded`

The agent that held the request passed it up, with one sentence of why.

| Field | |
|---|---|
| `number` | |
| `agent` | The agent that asked. |
| `by` | The agent that forwarded it. |
| `why` | Its sentence. |
| `held_by` | Who holds it now: `user`, or the agent's parent. |

#### `approval.cancelled`

The request was never answered: the agent that asked, or the one that held it, ended. It is no
longer pending, so the user never sees it.

| Field | |
|---|---|
| `number` | |
| `agent` | The agent that asked. |
| `held_by` | Who held it: `user`, or an agent's id. |
| `why` | Which agent ended, and how. |

#### `approval.decided`

The user answered the request, or the agent that held it denied it.

| Field | |
|---|---|
| `number` | |
| `agent` | The agent that asked. |
| `decision` | `deny`, `once`, or `tree` (allowed for the root's tree). |
| `by` | `user`, or the id of the agent that denied it (`approval_answer`). |

#### `approval.settled`

The request's answer was stored as the agent's `approval` delivery: at once for a denial, once
it was performed for an allowed request.

| Field | |
|---|---|
| `number` | |
| `agent` | |
| `outcome` | `done`, `failed`, `not_executed` (refused, or the situation changed), `refused` (denied by the user), or `interrupted` (a daemon stopped while it performed it). |
| `delivery` | The delivery's id, `approval-<number>`. |

#### `agent.turn_ended`

| Field | |
|---|---|
| `agent` | |
| `stop_reason` | `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`, or `interrupted` when its process died during the turn. |
| `usage` | The turn's token counts when the agent gave them, else `null`. |

#### `agent.released`

Its process was let go; the next message starts it again.

| Field | |
|---|---|
| `agent` | |
| `reason` | `idle` (no turn for the idle release time) or `restart` (the daemon that ran it is gone). |

#### `agent.died`

Its process ended by itself. Its session is replaced (`agent.replaced`), or the agent fails when
it has been replaced too often (`docs/agents.md`, "Replacement").

| Field | |
|---|---|
| `agent` | |
| `code` | Its exit code, `null` when a signal ended it. |
| `signal` | The signal, `null` otherwise. |
| `during_turn` | Whether a turn was running. |

#### `agent.failed`

The agent cannot go on: its process could not start, its session could not open, an option it was
asked is not one it offers, it was replaced too often, its new workspace could not be prepared, or
its parent failed. Its process is stopped, and so is its tree; what was queued for it is
superseded. A root's failure is for the user to look at: `flottille inbox` lists it.

| Field | |
|---|---|
| `agent` | |
| `reason` | A sentence, masked. |

#### `agent.ended`

The agent was stopped (by the user, its parent, or its parent's end), or it was a child that
reported. Its process and its runs are stopped; what was queued for it is superseded.

| Field | |
|---|---|
| `agent` | |
| `reason` | A sentence. |

#### `agent.reported`

The agent called `report` (`docs/tools.md`). A root's report is for the user: `flottille inbox`
lists it (`docs/cli.md`), and a pilot sees it with `events_since`.

| Field | |
|---|---|
| `agent` | |
| `text` | Its report, masked. |

#### `agent.asked`

The agent asked a question: a root of the user (`ask_user`), a child of its parent
(`ask_parent`). A root's question is for the user: `flottille inbox` lists it, and a pilot sees it
with `events_since`.

| Field | |
|---|---|
| `agent` | |
| `question` | Masked. |
| `to` | `user` or `parent`. |

#### `agent.stuck`

The sweep found the agent stuck in a turn (`docs/agents.md`, "The sweep"): its session is
replaced.

| Field | |
|---|---|
| `agent` | |
| `quiet_ms` | How long nothing showed. |

#### `agent.replaced`

A new session takes the agent's work on (`docs/agents.md`, "Replacement").

| Field | |
|---|---|
| `agent` | |
| `epoch` | The new session's number, counted from 0 for the first. |
| `reason` | Why, a sentence. |
| `counted` | Whether it counts against the limit of replacements. |

#### `delivery.queued`

A delivery was stored for the agent (`docs/agents.md`, "Deliveries"). One already stored under
the same id is not stored again, and has no event.

| Field | |
|---|---|
| `agent` | |
| `delivery` | Its id. |
| `kind` | `user`, `brief`, `parent`, `child-report`… |
| `urgency` | `between-turns`, `urgent` or `redirect`. |

#### `delivery.sent`, `delivery.given_back`, `delivery.superseded`

Deliveries were sent (`how`: `prompt`, or `note` on a tool result), given back to the queue
(their turn failed), or superseded (their agent ended first).

| Field | |
|---|---|
| `agent` | |
| `deliveries` | Their ids. |
| `how` | For `delivery.sent` only. |

### The events of a run

A command an agent ran with `commands_run` (`docs/tools.md`). Every event of a run has `agent`,
the agent that started it, and `run`, its id.

#### `run.started`

| Field | |
|---|---|
| `agent`, `run` | |
| `name` | The catalogue command, `null` for a line of the agent's. |
| `line` | The line run, masked of secrets like a call's arguments. |
| `folder` | Where it runs. |

#### `run.ended`

| Field | |
|---|---|
| `agent`, `run` | |
| `state` | `exited` (by itself), `stopped` (`commands_stop`, or the call that started it stopped), `interrupted` (the daemon stopped, or the one that ran it is gone). |
| `exit_code` | Its exit code, `null` when it did not exit by itself or a signal ended it. |
| `signal` | The signal that ended it, `null` otherwise. |
