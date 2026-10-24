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
