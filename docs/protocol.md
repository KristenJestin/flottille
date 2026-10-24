# The socket protocol

The daemon's API. The command line, the MCP server and the window are its clients; a desktop
shell or a script integrates Flottille through the command line, which speaks this protocol for
it (`docs/design.md` §3, §11).

## Transport

- One daemon per user listens on the Unix socket `$XDG_RUNTIME_DIR/flottille/daemon.sock`. The
  folder is the user's only (mode 700, owned by the user, checked at every start); a folder open
  to other users is refused.
- The daemon holds an exclusive lock on `$XDG_RUNTIME_DIR/flottille/daemon.lock`, which holds its
  process id, for its whole life. A second daemon finds the lock taken and refuses to start.
- Any command but `flottille daemon` starts the daemon in the background when nobody listens on
  the socket, then talks to it. The daemon started this way writes its output to
  `$XDG_STATE_HOME/flottille/daemon.log`.

The protocol is written to travel later over an authenticated network transport (design §12):
every request is self-contained, identifiers are opaque strings, and nothing assumes the client
shares the daemon's file system except paths the user typed.

## Framing

Both ways, one JSON object per line (UTF-8, `\n`-terminated, no newline inside an object). A
connection carries any number of requests; answers may come in any order and are matched by id.

### Request (client to daemon)

```json
{"id": "7", "method": "daemon.status", "params": {}}
```

| Field | |
|---|---|
| `id` | A string chosen by the client, opaque to the daemon, unique among its pending requests on the connection. |
| `method` | The method's name. |
| `params` | An object; may be left out when the method takes none. |

### Answer (daemon to client)

```json
{"id": "7", "result": {}}
{"id": "7", "error": {"code": "unknown-method", "message": "unknown method `nope`"}}
```

Exactly one of `result` and `error`. A line that is not a request is answered with an error
without an `id`.

| Error code | |
|---|---|
| `invalid-request` | The line is not a request object. |
| `invalid-params` | The parameters do not fit the method. |
| `unknown-method` | No such method. |
| `unavailable` | The daemon cannot do it now (for example, it is stopping, or its files cannot be written). |
| `refused` | The request is understood and cannot be done; the message is a sentence the user can act on (for example, a folder with no repository). |
| `not-found` | What the request names does not exist (for example, no project of that name). |
| `invalid-config` | A configuration file cannot be used; the message names the file and the key (`docs/projects.md`). |

### Event (daemon to client)

```json
{"event": {"seq": 12, "at": 1792843730808, "name": "daemon.started", "data": {}}}
```

Sent only on a connection that subscribed with `events.watch`. The event's fields are in
[`events.md`](events.md).

## Methods

### `daemon.status`

No parameters. The result:

```json
{
  "version": "0.1.0 (dd939a5ec692)",
  "pid": 4242,
  "uptime_s": 192,
  "agents": {"running": 0, "waiting_for_user": 0, "failed": 0}
}
```

`version` is the full version: the release and, for a build from a git checkout, its commit.
`agents` counts the agents running, the root agents waiting for the user, and the agents that
failed.

### `daemon.stop`

No parameters. The result is `{"stopping": true}`; the daemon then emits `daemon.stopping`,
removes its socket, lets the subscribers receive what remains, closes every connection and exits.

### `events.watch`

Subscribes the connection to the event stream.

| Parameter | |
|---|---|
| `after` | Optional event number. With it, the stored events numbered after it are sent first, oldest first, then the new ones. Without it, only the events emitted after the subscription. |

The result is `{"after": <n>}`, the number the stream starts after. It is written before the
first event. Then every event follows, in the order of its number, none missing and none twice,
until the connection closes. A client that lost its connection resumes with the `seq` of the last
event it saw as `after`.

### The project methods

A project is a folder registered with its configuration (`docs/projects.md`). A project in a
result is the object of its file with every default filled in:

```json
{
  "name": "app",
  "folder": "/work/app",
  "branch_prefix": "app",
  "workspaces_root": "$XDG_DATA_HOME/flottille/workspaces/app",
  "default_repositories": "all",
  "max_children": 3,
  "run_time_limit": 1800,
  "never": [],
  "repositories": [{"path": ".", "remote": "origin", "base": ["main"], "depends_on": []}]
}
```

`workspaces_root` is an absolute path in a real answer: `$XDG_DATA_HOME` stands for the user's data
folder here. `default_repositories` is `"all"` or a list of repository paths; `variables` and
`commands` are left out when empty. The files are the user's to edit: before it answers any `project.*` request, the
daemon reads them again and announces what changed in them since the last such request as events
(`docs/events.md`).

Parameters that do not fit a method are answered `invalid-params`.

#### `project.list`

No parameters. The result is `{"projects": [...], "errors": [...]}`: the projects by name, and the
files that cannot be used, each `{"name", "file", "message"}` (`file` is `projects/<name>.toml`, the
message starts with the key at fault). A file with an error does not hide the other projects.

#### `project.show`

`{"name"}`. The result is the project. `not-found` when there is none, `invalid-config` when its file
cannot be used.

#### `project.detect`

`{"folder", "name"?}`. What `project.add` would register, written nowhere: `{"name", "folder",
"repositories"}`. `folder` is absolute, as the client's user typed it; the result's `folder` is its
resolved form (links followed). `name` is a slug of the folder's name when left out. The repositories
are the folder itself (`.`) if it holds a `.git` (a folder or a file), then the folders one or two
levels down that do, alphabetically; hidden folders and links are not looked into.

`refused` when the folder is not absolute, does not exist or is not a folder; when no name can be
made from it or the one given is not a name; when it holds no repository; when a project already
has that name or that folder.

#### `project.add`

The same parameters and the same refusals as `project.detect`, and `repositories`: the list the
user confirmed, as `project.detect` gave it. When it is there and the daemon's own detection now
differs, the request is `refused` with a sentence saying the repositories changed since they were
shown and to run it again; nothing is written. A client that shows no list leaves it out.

Registers the folder: writes `projects/<name>.toml` with the repositories detected, each with the
remote `origin` and the base rule `["main"]`, and answers the project. Nothing is written in the
folder or its repositories.

#### `project.remove`

`{"name"}`. Removes the project's file; the result is `{"removed": "<name>"}`. The folder and its
repositories are never touched. `not-found` when there is no such file.

#### `project.check`

`{"name"}`. Detects the folder's repositories again and says how they differ from the configuration,
changing nothing: `{"name", "folder", "new", "missing"}`, `new` the repositories detected and not
configured, `missing` the ones configured and no longer there. `refused` when the folder no longer
exists.

### The workspace methods

A workspace is one worktree per repository, on one branch, prepared by the project's recipe
(`docs/workspaces.md`). It is named by its project and its name. A workspace in a result:

```json
{
  "project": "app",
  "name": "login",
  "branch": "app/login",
  "folder": "/work/app-work/login",
  "project_folder": "/work/app",
  "created_at": 1792843730808,
  "state": "failed",
  "variables": {"STORE": "{workspace.path}/.store"},
  "repositories": [
    {"path": "api", "base": {"branch": "develop", "remote": "origin", "ref": "refs/remotes/origin/develop",
      "commit": "503144c9654d6762b17fd670ddc9f88aa39ecfaf", "fetched_now": true, "fetched_at": 1792843730790}}
  ],
  "steps": [
    {"position": 0, "repository": "api", "kind": "worktree", "state": "done"},
    {"position": 1, "repository": "api", "kind": "copy", "globs": [".env", ".env.*"], "state": "done"},
    {"position": 2, "repository": null, "kind": "root_entries", "except": ["api"], "state": "done"},
    {"position": 3, "repository": "api", "kind": "run", "line": "npm install", "time_limit": 1800, "state": "failed",
      "failure": {"doing": "npm install (in api)", "output": "npm ERR! …"}}
  ]
}
```

`state` is `preparing`, `failed` or `ready`. `variables` are the project's as they were at
creation, not yet filled. A base's `fetched_now` is false when the workspace was created offline;
`fetched_at` (milliseconds since the Unix epoch) is when its tracking ref was last updated, `null`
when unknown. A step's `kind` is `worktree`, `copy` (`globs`), `link` (`path`), `root_entries`
(`except`: the top-level names of the project's repositories) or `run` (`line`, `time_limit` in
seconds, and `command` when it is a command of the catalogue); its `state` is `pending`, `running`, `done`, `skipped` or
`failed`, and `failure` is there only on a failed step. `repository` is `null` for a step done in
the workspace folder.

Each method answers `not-found` when the project or the workspace does not exist;
`workspace.create`, the only one that reads the project's file, answers `invalid-config` when it
cannot be used.

#### `workspace.create`

`{"project", "name", "repositories"?, "branch"?, "offline"?}`. Fetches the base of each
repository, writes the workspace, makes the worktrees and answers the workspace; the rest of the
recipe goes on in the background. `repositories` is the project's default set when left out,
`branch` is `<branch prefix>/<name>`, and `offline: true` fetches nothing.

`refused`, with nothing written nor made, when the name is not one, a repository is not the
project's, the branch is not one git accepts, the project has a workspace of that name, the folder
exists, the branch exists in a repository, no branch of a base rule is on the remote, or **a fetch
fails** (the message is git's reason). A worktree that cannot be made is not a refusal: the
workspace is answered `failed`, with the step's failure.

#### `workspace.list`

`{"project"?}`. `{"workspaces": [...]}`, every workspace or the project's, by project then name.

#### `workspace.show`

`{"project", "name"}`. The workspace.

#### `workspace.prepare`

`{"project", "name"}`. Resumes the preparation (`docs/workspaces.md`) and answers the workspace
with `"resumed": {"redone", "retried"}`: the done steps made again because their result is gone,
and the failed or running steps tried again. A ready workspace whose results are all there is
answered as it is, both counts 0. `refused` while it is being prepared or removed.

#### `workspace.remove`

`{"project", "name"}`. `{"removed": "<name>", "project": "<project>"}` once its worktrees, its
folder and its record are gone. `refused`, with nothing touched, while it is being prepared, or
when a worktree holds a changed or untracked file or a commit on no remote: the message names
every one.
