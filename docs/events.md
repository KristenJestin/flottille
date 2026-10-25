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
| `reason` | `request` (`flottille daemon stop`) or `signal` (`SIGTERM`, `SIGINT`). |

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

### `workspace.removed`

The workspace's worktrees, folder and record are gone. Its branches are kept.

| Field | |
|---|---|
| `project`, `workspace` | |

### The events of an agent

Every event of an agent has `agent`, its id (`docs/agents.md`), and is part of its conversation
(`agent.show`). The events made from what the agent sends also have `replay`: true while a
`session/load` replays its history, so that a client never takes it for new.

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

#### `agent.started`

Its process runs and its session is open, its options set.

| Field | |
|---|---|
| `agent` | |
| `pid` | The adapter's process id. |
| `opened` | `new`, `resumed` or `loaded`. |
| `native_id` | The ACP session id. |
| `agent_version` | The adapter's name and version. |
| `taken` | `{"model", "effort", "mode"}`: what the agent took. |

#### `agent.prompted`

A turn starts with this message.

| Field | |
|---|---|
| `agent` | |
| `text` | The message. |

#### `agent.user_message`, `agent.message`, `agent.thought`

A piece of text of the user (seen in a replay), of the agent's reply, or of its thinking, as it
streams: the pieces of one reply follow each other.

| Field | |
|---|---|
| `agent`, `replay` | |
| `text` | The piece, cut at 64 KiB (`‹+N chars›` says how much was left out). |

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

#### `agent.provider_wait`, `agent.provider_limit`

The provider makes the agent wait, or refuses it for a limit (`message`, when it says one). Each
with `agent` and `replay`.

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
| `gate` | The gate's verdict on the call (`allow` or `deny`), `null` for a call refused before it, because the agent has no such tool. |
| `outcome` | `ok`, `refused` or `failed`. |
| `duration_ms` | How long it took. |

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

Its process ended by itself. The agent failed.

| Field | |
|---|---|
| `agent` | |
| `code` | Its exit code, `null` when a signal ended it. |
| `signal` | The signal, `null` otherwise. |
| `during_turn` | Whether a turn was running. |

#### `agent.failed`

The agent cannot go on: its process could not start, its session could not open, an option it was
asked is not one it offers. Its process is stopped.

| Field | |
|---|---|
| `agent` | |
| `reason` | A sentence, masked. |

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
