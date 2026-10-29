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
- A daemon whose socket is gone, or whose path now leads to another socket (its runtime folder was
  removed), stops itself within a few seconds, as `daemon stop` does (`daemon.stopping`, reason
  `socket`): nobody could reach it.
- The daemon also holds a lock on `$XDG_STATE_HOME/flottille/store.lock` for its whole life. A
  daemon started while another still holds the store (one whose runtime folder was removed, which
  is stopping its agents) waits for it to exit before it opens the store: it never kills, settles
  or resumes the agents of a daemon still alive, and it takes over, as at any start, what that one
  left.
- The daemon and its agents end with the user's last session, unless `loginctl enable-linger` keeps
  the runtime folder. The socket and the lock carry the sticky bit, so that a cleanup of files
  untouched for 6 hours does not remove them.
- **A client an agent started is refused.** Everything the daemon starts (agents, their
  commands, git, the forges' command lines) descends from it, the daemon is the subreaper of all
  of it (a process orphaned there, even one that detached with `setsid`, is taken in by the
  daemon, not by init), and nothing it starts is a client of its own. So on each connection the
  daemon reads the client's process id (`SO_PEERCRED`) and walks up its parents
  (`/proc/<pid>/stat`): a client that descends from the daemon, or whose ancestry cannot be read
  (gone already, a parent's pid taken since by another process), gets every request answered
  with the error `refused`, "refused: this client was started by an agent; agents act only
  through their tools", for 10 seconds at most, and nothing it sends is done. The daemon's log
  says it once per connection. The user's own clients (the command line, the window, the pilot
  door an outside agent starts) do not descend from the daemon and are not affected.
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
  "version": "0.1.0 (local, dd939a5ec692)",
  "pid": 4242,
  "uptime_s": 192,
  "agents": {"running": 0, "waiting_for_user": 0, "failed": 0}
}
```

`version` is the full version: the release and, for a build from a git checkout, its commit.
`agents` counts the agents running (starting or in a turn), the root agents waiting for the user
(with a request the user holds, `approval.list`, or a question not answered yet), and the agents
that failed (`docs/agents.md`).

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

`{"name"}`. The result is the project, each repository with `branch`: the branch its checkout in
the project folder is on now, `null` when it is on none (a detached checkout, a folder git cannot
read, or one that is not a repository of its own yet). `project.list` leaves it out: it runs no git. `not-found` when there is none,
`invalid-config` when its file cannot be used.

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

#### `project.settings`

`{"name"}`. The project's forges and tracker as its Settings show them (`docs/forges.md`, "The
forge of a repository"): `{"name", "folder", "repositories": [{"path", "remote", "url", "forge",
"proposal", "disagrees"}], "tracker", "agent_actions", "obsolete"}`. `agent_actions` is what its
agents do without asking (`docs/projects.md`, "What agents do on their own"):
`{"procedures", "settings", "references"}`, each `ask` or `allow`. `url` is the remote's address (credentials taken
out, masked; `null` when it has none), `forge` the one declared (`null` when none), `proposal` what
the address proposes (`null` when nothing), `disagrees` why the declared forge is not where the
remote leads (`null` when it is). `obsolete` lists the keys of the file that are no longer read
(the old `[forges]`), each with what replaces it. Nothing is asked of a forge.

#### `project.set_settings`

`{"name", "repositories": [{"path", "forge"}], "tracker"}`. Writes each listed repository's `forge`
(removed when `null`; a repository left out is unchanged) and the project's `[tracker]` (removed
when `null` or left out) in its file, where they stand, what is around them kept, and answers as
`project.settings`. `refused`, with nothing written, when a path is not one of the project's, a
forge or the tracker cannot be used (naming its key), or a forge is not where its repository's
remote leads (`api cannot be declared on GitHub acme/api: its remote origin: it leads to
github.com/other/api, not to GitHub acme/api; nothing was saved`).

#### `project.set_agent_actions`

`{"name", "agent_actions": {"procedures", "settings", "references"}}`, each `ask` or `allow`. Writes
the project's `[agent_actions]` in its file, where it stands, what is around it kept (removed when
every family asks, the default), and answers `{"name", "agent_actions"}`. The user's door alone
sets it: no tool of an agent does (`docs/tools.md`, "The project").

#### `project.ticket`

`{"project", "ticket"}`. Reads a ticket of the project's tracker before any workspace has it, for
the new-workspace form's preview (`docs/tickets.md`): `{"project", "ticket", "tracker",
"tracker_said", "problem"}` and, once read, the fields of `workspace.ticket`. `problem` says why it
is not taken or not read; it is not a refusal.

### The reference methods

A project's references (`docs/projects.md`, "References"). The daemon edits the project's file in
place, the rest of it left as written, and announces `project.changed`. `not-found` for a project
or a reference that does not exist, `refused` for a reference that cannot be used, with its key.

#### `reference.list`

`{"project"}`. The result is `{"project", "references": [{"name", "description", "path"}]}`, by name.

#### `reference.add`

`{"project", "name", "description", "path"}`. `path` is relative to the project folder; a trailing
`/` is dropped. `refused` when the name is already used, when `path` does not exist, leaves the
project folder (a link followed) or goes through a `.git`, or when the name or the description is
not valid. The result is `{"project", "name", "description", "path"}`.

#### `reference.remove`

`{"project", "name"}`. The result is `{"project", "removed": "<name>"}`.

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
      "urls": ["git@git.example.com:acme/api.git"], "commit": "503144c9654d6762b17fd670ddc9f88aa39ecfaf",
      "fetched_now": true, "fetched_at": 1792843730790}}
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
when unknown. Its `urls` are where its remote led at creation, fetch and push, credentials taken
out (`docs/procedures.md`); absent for a workspace created before they were recorded. A step's `kind` is `worktree`, `copy` (`globs`), `link` (`path`), `root_entries`
(`except`: the top-level names of the project's repositories) or `run` (`line`, `time_limit` in
seconds, and `command` when it is a command of the catalogue); its `state` is `pending`, `running`, `done`, `skipped` or
`failed`, and `failure` is there only on a failed step. `repository` is `null` for a step done in
the workspace folder.

Each method answers `not-found` when the project or the workspace does not exist;
`workspace.create`, the only one that reads the project's file, answers `invalid-config` when it
cannot be used.

#### `workspace.create`

`{"project", "name", "repositories"?, "branch"?, "offline"?, "ticket"?}`. Fetches the base of each
repository, writes the workspace, makes the worktrees and answers the workspace; the rest of the
recipe goes on in the background. `repositories` is the project's default set when left out,
`branch` is `<branch prefix>/<name>`, `offline: true` fetches nothing, and `ticket` (`#123`, `123`
or a Jira key) links the workspace to a ticket (`tickets.md`); the workspace's `ticket` says it, in
every answer that shows a workspace, `null` when it has none.

`refused`, with nothing written nor made, when the name is not one, the ticket is not one, a
repository is not the project's, the branch is not one git accepts, the project has a workspace of that name, the folder
exists, the branch exists in a repository, no branch of a base rule is on the remote, or **a fetch
fails** (the message is git's reason). A worktree that cannot be made is not a refusal: the
workspace is answered `failed`, with the step's failure.

#### `workspace.list`

`{"project"?}`. `{"workspaces": [...]}`, every workspace or the project's, by project then name.

#### `workspace.show`

`{"project", "name"}`. The workspace, each repository with `changes`: `{"added", "removed",
"capped"}`, the lines added and removed in its worktree since its base commit, whether its branch
committed them or they are not committed yet, new files git does not ignore counted whole (a
binary file, a link, anything but a regular file, or a file over 8 MiB counts no line). At most
1000 new files and 16 MiB of them are read: past that the count stops and `capped` is `true`, the
real count being higher. It writes nothing, not even git's index. `changes` is `null` when git
cannot read the worktree (not made yet, or gone). `workspace.list` leaves it out: it runs no git.
`ready_at` is when its last preparation ended, the time of its newest `workspace.ready` event
(`docs/events.md`), kept with the workspace; `null` while it is not `ready`. `workspace.list`
leaves it out too.

#### `workspace.forge`

`{"project", "name", "watch"?}`. The forge state of the workspace's repositories
([`forges.md`](forges.md)): the last one read when it is less than a minute old, else read now.
With `watch: true`, the connection watches the workspace: the daemon reads it again about once a
minute (less often while `gh` or `bkt` fails) for as long as the connection is open, and emits
`workspace.forge` when it changed (`docs/events.md`). A connection watches a workspace once, however
often it asks.

```json
{
  "project": "app",
  "workspace": "login",
  "branch": "app/login",
  "read_at": 1792843730808,
  "problem": null,
  "repositories": [
    {"path": "api", "remote": "origin", "forge": "github", "repository": "acme/api", "note": null,
     "pull_request": {"number": 12, "title": "Add the login page", "state": "open", "base": "develop",
       "url": "https://github.com/acme/api/pull/12", "review": "approved",
       "checks": [{"name": "build", "state": "passing"}], "mark": "passing", "comments": 2},
     "error": null},
    {"path": "web", "remote": "origin", "forge": null, "repository": null,
     "note": "no forge set for this repository: choose it in the project's Settings (the window), or write its `forge` in the project's file; Flottille never guesses it", "pull_request": null, "error": null}
  ]
}
```

`problem` says, with what to do, why a forge cannot be read (`gh` or `bkt` missing, not signed in;
several are joined by `; `), else `null`; the repositories it stops have no `pull_request`. For
each repository, `forge` is `github`, `bitbucket` or `null` (then `note` says why), `repository`
is `owner/name` (a Bitbucket Cloud workspace and name, or a Data Center project key and name),
`pull_request` is `null` when its branch has none, and `error` says why its forge could not be
read. A pull request's `state` is `open`, `draft`, `merged` or `closed`; `review` `approved`,
`changes requested`, `review required` or `null`; a check's `state` `passing`, `failing`,
`pending` or `skipped`; `mark` the checks as one (`failing`, `pending`, `passing`), `null` without
checks. A Bitbucket pull request also has `reviewers`, each `{"name", "state"}` with `state`
`approved`, `changes requested` or `pending`, when it has any.

#### `workspace.ticket`

`{"project", "name"}`. The workspace's ticket, read now on its tracker ([`tickets.md`](tickets.md)):

```json
{
  "project": "app",
  "workspace": "login",
  "ticket": "#12",
  "tracker": "github",
  "repository": "acme/api",
  "title": "The login fails",
  "state": "open",
  "url": "https://github.com/acme/api/issues/12",
  "description": "Users cannot sign in.",
  "comments": [{"author": "alice", "at": "2026-10-01T10:00:00Z", "body": "I can reproduce"}],
  "older_comments": 0,
  "read_at": 1792843730808,
  "problem": null
}
```

`ticket` is `null` for a workspace linked to none, and then nothing else but `read_at` and a null
`problem` is there. `tracker` is `github` or `jira`; `repository` is the GitHub repository the issue
is in, `null` for Jira; `state` is `open` or `closed` for GitHub and Jira's own status name for
Jira. When the ticket cannot be read (`gh` or `acli` missing, not signed in, no such ticket, a call
that failed), `problem` says why and with what to do, and the fields of the ticket are absent. All
the tracker's text is masked. Nothing is kept: every call reads again.

#### `workspace.link_ticket`

`{"project", "name", "ticket"}`. Links the workspace to a ticket, or to none when `ticket` is
`null`, and answers as `workspace.ticket` does. `refused` when `ticket` is not `#123`, `123` or a
Jira key (`ABC-123`, any case). Emits `workspace.ticket` (`docs/events.md`).

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

### The procedure methods

A project's procedures and their runs (`docs/procedures.md`). A procedure in a result is `{"name",
"description", "grants", "children", "says"}`, `grants` an object of grant to mode (`{"push":
"allow", "pr-create": "ask"}`), `says` one `{"grant", "mode", "says"}` per grant with the sentence a
client shows; `procedure.show` and the edits add `instructions`. `not-found` for a project or a
procedure that does not exist, `invalid-params` for a grant outside the vocabulary or a mode other
than `allow` and `ask`, `refused` for an empty description or instructions, `invalid-config` when
the project's file cannot be used.

#### `procedure.list`

`{"project"}`: `{"project", "procedures": [...]}`, by name.

#### `procedure.show`

`{"project", "name"}`: the procedure.

#### `procedure.add`

`{"project", "name", "description", "instructions", "grants"?, "children"?}`, or `{"project",
"template", "name"?}` for one of Flottille's templates as it is (`open-prs`, `merge-stack`,
`close`; `name` the template's when left out, and nothing else given). Writes
`[procedures.<name>]` in the project's file, what is around it kept, and answers the procedure.
`refused` when the project has one of that name, `not-found` for a template Flottille has not.

#### `procedure.edit`

`{"project", "name", "description"?, "instructions"?, "grants"?, "children"?}`: what is given
replaces what the procedure had (`grants` whole). Answers the procedure.

#### `procedure.remove`

`{"project", "name"}`: `{"project", "removed": "<name>"}`.

#### `procedure.run`

`{"project", "name", "workspace"}`: what a run would grant, to show the user; it starts nothing.
The procedure with `{"project", "workspace", "started": false}`. `not-found` when the workspace does
not exist.

#### `procedure.start`

`{"project", "name", "workspace", "grants", "children"}`, the grants and `children` the user was
shown by `procedure.run`. `refused` when they are no longer the procedure's (it was edited
meanwhile): nothing starts. Otherwise a root agent starts in the workspace with the procedure's
instructions, as `agent.start` starts one, and its run holds the grants from then on:
`{"agent", "after", "procedure", "grants"}`, `agent` and `after` as `agent.start` answers them.

#### `procedure.of`

`{"agent"}`: the run whose root is that agent, for a client that shows its grants while it goes on:
`{"agent", "run"}`, `run` null when the agent has none, else `{"procedure", "project", "workspace",
"grants", "children", "says", "ended"}` (`grants` and `says` as in a procedure, `ended` once the
run is over).

### The agent methods

An agent is one agent session run over ACP (`docs/agents.md`), named by an opaque id. None of
these methods waits on the agent: what it does comes as events (`docs/events.md`), and `after`,
in the answers that start something, is the number to give `events.watch` to follow it from
there. An agent in a result:

```json
{
  "id": "3f9a1c0b7d2e",
  "provider": "claude",
  "role": "lead",
  "parent": null,
  "depth": 0,
  "epoch": 0,
  "title": null,
  "brief": null,
  "reported_at": null,
  "quiet_turns": 0,
  "waiting_on_answer": false,
  "project": "app",
  "workspace": "login",
  "folder": "/work/app-work/login",
  "state": "idle",
  "state_reason": null,
  "created_at": 1792843730808,
  "updated_at": 1792843741200,
  "active_at": 1792843741190,
  "ended_at": null,
  "chosen": {"model": "sonnet", "effort": "high", "mode": null},
  "taken": {"model": "sonnet", "effort": "high", "mode": null},
  "native_id": "0f6c…",
  "agent_version": "flottille-claude-bridge 0.1.0",
  "reopens": true,
  "images": true,
  "instructions": "…\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\n…",
  "tools": ["mcp__flottille__fs_read", "…"]
}
```

`workspace` is `null` for the project folder itself. `state` is `starting` (its process starts
and its session opens), `working` (a turn runs), `idle` (between turns, its process running or
released), or one of the final states: `ended`, `failed` (`state_reason` says why). A final state
is never left, but for an ended root the user resumes (`agent.resume`).
`provider` is the provider it runs on, for life (`docs/agents.md`, "Providers"). `chosen` is what
was asked, or the project's default model, `null` for the agent's default; `taken` what the agent reported
at the last start of its process; `reopens` whether its agent offered to resume or load a session
when it last opened one, `false` once Flottille forgot an ended root's session; `images` whether
it took images in a prompt when it last started, `null` before it did. `active_at` is its last sign of activity, `null` before the
first. `instructions` and `tools` are frozen when the agent is created. `parent` is the agent that
launched it (`null` for a root), `depth` its depth in the tree, `epoch` the number of its session
(each replacement adds one), `title` and `brief` a child's task, `reported_at` when it reported,
`quiet_turns` its turns ended without a report in a row, `waiting_on_answer` whether a question of
it waits for an answer: a child's from its parent, a root's from the user, until that one writes to
it (`docs/agents.md`, "The tree").

Each method answers `not-found` when the agent does not exist.

#### `agent.start`

`{"project", "workspace"?, "provider"?, "model"?, "effort"?, "mode"?, "role"?, "message"}`. Creates
a root agent in the project folder or the workspace, and gives it its first message: `{"agent",
"after"}`. `role` is `lead` (the default) or `worker`. `provider` is the project's, else the first
one enabled, when left out; `model` the project's when the provider is the project's, else the
provider's default; `model` and `effort` are values of the session's options, as the provider
lists them (`provider.list`). `not-found` when the project or the workspace does not exist;
`refused` when the workspace is not ready, the message is empty, the role is not one (the sentence
lists them), a `mode` is given (Flottille's gate decides every tool call, and a permission mode of
the agent's could only loosen it), the provider does not exist, is not enabled or not ready (the
sentence says why, and names the ready ones), or the provider's models are known and the model or
the effort is not among them (the sentence names those it offers). While they are not known yet,
a model or an effort the agent does not offer is known only once its session is open: the agent
then fails (`agent.failed`), saying what it offers.
`agent.run` is the name this method had before the pilot door: it is still answered, the same.

#### `agent.send`

`{"agent", "message", "images"?}`. Gives the agent a message (a `user` delivery), played after the turn that
runs if one does, starting its process again when it was released: `{"agent", "after",
"delivery"}`. `delivery` is the id of the delivery stored for the message: its `delivery.queued`
event (numbered after `after`), then `delivery.sent` when a prompt takes it (`docs/events.md`), name
it, so that a client follows the turn of its own message even when another client writes to the
agent at the same time.
`refused` when the agent is in a final state, the message is empty, or the agent is a child: only
its parent writes to it, and the sentence names its root.

`images` is a list of `{"mime", "data"}`, `data` the image's bytes in base64: a pasted or dropped
picture, sent as ACP image blocks after the message's text. With images, the message may be
empty. `mime` is `image/png`, `image/jpeg`, `image/gif` or `image/webp`; a message carries 4
images at most, 5 MiB each. The daemon writes them in `$XDG_DATA_HOME/flottille/images/<agent>/`
(never the project's), and the `agent.prompted` event of the turn that sends them names them.
`refused` for an image of another type, bad base64, an empty or too big image, too many, and
when the agent's agent said, when it last started, that it takes no images (`images` of the
agent, below); before it started once, `images` is `null` and the message is accepted: if the
agent turns out to take no images, none is sent and the text of the turn ends with
`[N images not sent: this agent takes no images]`. A file gone when the turn starts leaves
`[image <name> is missing]` in the text. Merged queued messages make one prompt of 4 images and
10 MiB at most (the first message is always whole); the others wait for the next turn. The
images of an agent go with it: when it ends for good, or 7 days after a root that can be resumed.

#### `agent.image`

`{"agent", "name"}`. An image of the agent's, by a name `agent.prompted` gave: `{"mime", "data"}`.
`not-found` when there is none.

#### `agent.cancel`

`{"agent"}`. Cancels the turn that runs: `{"agent", "cancelled"}`, `cancelled` false when no turn
ran. The turn then ends `cancelled`. `refused` for a child: only its parent cancels its turns,
and the sentence names its root.

#### `agent.stop`

`{"agent"}`. Stops a root and its tree, deepest first (`docs/agents.md`, "The tree"):
`{"agent"}`, the root once ended. `refused` when it is in a final state, or a child (the sentence
names its root).

#### `agent.resume`

`{"agent"}`. Starts an ended root again on its own session: `{"agent"}`, the root once idle again
(`agent.resumed`, `docs/events.md`). It keeps its id, its epoch, its conversation and its session;
nothing is sent to it, and the next message (`agent.send`) starts its process, which resumes or
loads the session as after an idle release (`docs/agents.md`, "How an agent runs"). Its children
stay ended, and so does the procedure run it was the root of. Its next prompt starts with a
`resume` delivery saying the user stopped it and naming its requests cancelled then. `refused`
for a child (the sentence names its root), an agent that has not ended, a failed one (its session
went with it), one whose agent can neither resume nor load a session, that never opened one or
whose session was forgotten (7 days after it ended), one whose workspace or project is gone: what
`resumable` says, in short (`agent.tree`); and one whose last process is still stopping
(`stopping`), with a sentence to try again in a moment.

#### `agent.tree`

`{"agent"}`. The whole tree of the agent's root, each agent before its children:
`{"agents": [{"agent", "doing", "waiting_on", "needs_user"}]}`. `agent` is the agent in short
(`id`, `provider`, `role`, `title`, `project`, `workspace`, `parent`, `depth`, `epoch`, `state`,
`state_reason`, `created_at`, `active_at`, `ended_at`, `waiting_on_answer`, `model`: the model it
took at the last start of its process, else the one asked, `null` for the agent's default before it
started; `resumable`: whether `agent.resume` takes it once its last process is gone; `stopping`:
whether the process of an agent that ended is still stopping; `images`: as in the agent's full
form); `doing` a sentence of what it does
(`working`, `idle`, `running \`cargo test\` for 12 min`, `waiting for approval #3`, `failed: …`);
`waiting_on` `parent` for a child whose request or question waits for its parent, `user` for a
root whose own does, else `null`; `needs_user`, for the root, whether a request of its tree or its
own question waits for the user.

#### `agent.read`

`{"agent", "after"?}`. A page of the agent's conversation, any agent of a tree: `{"agent", "name",
"lines", "after", "more"}`. `lines` are what the agent was sent, what it said, its tool calls, its
turns' ends and its reports, each led by `#<n>`, the number of its event (a reply streamed in
pieces is one line, led by its first piece's, and masked once joined; two messages are two lines),
at most 30,000 characters of them in all, a line of 2,000 at most: a longer reply goes on, whole,
on the next lines; `name` is the agent as another is told of it (its title and its id);
`after` is the number to read on after, and `more` whether the page stopped before the end of what
is stored. `agent_status` with `action: conversation`, of the agents' tools, gives the same page to a parent.

#### `agent.status`

`{"agent"}`. What one agent is and does, for a client that shows one agent: `{"agent", "name",
"line", "doing", "waiting_on", "needs_user", "messages"}`. `agent` is the agent in short as in
`agent.tree`, `line` a sentence of what it is and does with its last sign of activity and its
context use, `doing`, `waiting_on` and `needs_user` as in `agent.tree`, and `messages` its last
messages, the oldest first, each cut at 600 characters.

#### `agent.list`

`{"project"?}`. Every agent, or the project's, in short as in `agent.tree`: `{"agents"}`.

#### `agent.trees`

`{"project"?, "ended"?, "before"?}`. A page of **root** agents, newest first, each with its status
(as `agent.status`) and its whole tree (as `agent.tree`), in one answer: `{"trees": [{"status",
"agents"}], "more", "hidden"}`. The live roots only, unless `ended` is true. A page holds at most 20
roots; `more` says whether roots follow, and `before` (the id of the last root of the page before)
reads on after it, even if that root has ended since. `hidden` is how many ended roots a live-only
listing leaves out. `refused` when `before` is not a root agent. The pilot door's `agents_list` and
`agent_status` are built on it.

#### `agent.show`

`{"agent", "after"?, "before"?}`. `{"agent", "events", "more"}`: the agent, and a page of its
events, oldest first: at most 1000, the first ones (those numbered after `after` when it is given),
or with `before` the last ones numbered before it. `more` says whether events remain past the
page: newer ones, or with `before` older ones.

### `provider.list`

`{"project"?}`. Every provider, in order, as `flottille doctor --json` reports it, with what it
offers (`docs/agents.md`, "Providers"), and, for a project, the provider and model its agents get
when they name none:

```json
{
  "providers": [
    {
      "name": "claude",
      "title": "Claude",
      "enabled": true,
      "ready": true,
      "why": null,
      "warnings": [],
      "program": {"name": "claude", "present": true, "path": "/usr/bin/claude",
                  "version": "2.1.295", "checked": "2.1.295"},
      "login": {"present": true, "path": "/home/user/.claude/.credentials.json", "api_key": null},
      "override": null,
      "models": [{"id": "sonnet", "name": "Sonnet", "efforts": ["low", "medium", "high"], "default": true},
                 {"id": "opus", "name": "Opus", "efforts": null}],
      "models_outdated": false
    },
    {"name": "opencode", "title": "OpenCode", "enabled": false, "ready": false,
     "why": "is not enabled (`enabled = true` under [opencode] in providers.toml enables it)", "…": "…",
     "models": null, "models_outdated": false},
    {"name": "codex", "title": "Codex", "enabled": false, "ready": false,
     "why": "is not enabled (`enabled = true` under [codex] in providers.toml enables it)", "…": "…",
     "models": null, "models_outdated": false}
  ],
  "project": {"name": "app", "provider": "claude", "model": "sonnet"}
}
```

`why` is `null` when the provider's agents can run, else a clause after its name that says why
not and what to do. `warnings` say what may stop its agents that the daemon cannot tell for
sure: a version other than the one Flottille was checked against, no sign-in it can see.
`login.api_key` is the name of the API key found set, never its value. `override` is the test
program standing for the provider (`docs/agents.md`, "For tests and development"). `models` is
`null` until a session of the provider's agent said them, and `efforts` of a model `null` until a
session had that model; `name` is what the provider shows the model by; `default` is there, `true`,
on the model a new session takes when none is asked, once one said it. `models_outdated` is `true`
when they were said by another version of the program or with other API keys set: they are listed
as they were, but refuse no model and no effort until a session says them again. `project` is there only when one is named; its `provider` is `null` when
none is enabled, its `model` `null` when the project names none. `not-found` when the project
does not exist; `invalid-config` when `providers.toml` cannot be read (the sentence names the key
at fault).

### `provider.enable`, `provider.disable`

`{"name"}`. Enables or disables the provider in `providers.toml` (`docs/agents.md`, "Providers"):
`{"name", "enabled"}`. The daemon sets `enabled` under the provider's table and keeps the rest of
the file as it is, its comments included; it makes the file, or the table, when there is none.
The file is written in place: a temporary file beside it is renamed over it, so that a reader
never sees half of one; a `providers.toml` that is a symbolic link stays one (its target is
written), its mode is kept, and no temporary file stays when the write fails.
When this changes the file it emits `provider.changed` (`docs/events.md`); asking for what the file
already says changes nothing and emits nothing. When the file was written but its event could not
be sent, the change happened all the same: the answer says so in `warning`, a sentence, and the
command line prints it. Disabling a provider stops no agent: those running
on it go on, and each fails at the next start of its process, saying why (`agent.failed`).
`not-found` when there is no provider of that name (the sentence names the providers);
`invalid-config` when `providers.toml` cannot be read (the sentence names the key at fault), and
the file is then left as it is.

### `inbox.list`

`{"after"?, "limit"?}`. What the root agents sent to the user: `{"entries": [...]}`, oldest first.
An entry is `{"seq", "at", "kind", "agent", "project", "workspace", "text"}`, made from an event of
a root agent (`docs/events.md`): `report` (`agent.reported`, `text` its report), `question`
(`agent.asked` to the user, `text` its question) or `failure` (`agent.failed`, `text` its reason).
A child's own go to its parent and are not here. Without `after`, the last `limit` entries (50 by
default); with it, the first `limit` entries numbered after it. Nothing is marked read: the inbox
is a view of the stored events.

### `test.advance`

`{"ms"}`. Only when the daemon runs with `FLOTTILLE_TEST_CLOCK` set (`docs/agents.md`, "For tests
and development"); otherwise `unknown-method`. Moves the daemon's clock `ms` milliseconds ahead and
runs the sweep at once: `{"now"}`, the clock's time.

### The approval methods

The requests of the gate (`docs/tools.md`, "The gate"). A request in a result:

```json
{
  "number": 3,
  "agent": "3f9a1c0b7d2e",
  "call_key": "toolu_01",
  "tool": "fs_read",
  "call": {"path": ".env"},
  "reasons": [".env is a sensitive place (.env)"],
  "sensitive": true,
  "kind": "call",
  "says": [],
  "held_by": "user",
  "forwards": [],
  "state": "pending",
  "decision": null,
  "created_at": 1792843730808,
  "decided_at": null
}
```

`call` is the call's arguments, every text cut at 500 characters and masked; the call itself is
kept whole, to be performed, and never sent. `held_by` is `user` or the id of the agent that holds
the request; `forwards` are the `{"by", "why"}` of each agent that passed it up. `state` is
`pending`, `denied`, `allowed`, or `cancelled` when the agent that asked or the one that held it
ended first; `decision` is `deny`, `once`, `tree` or `always` once it is decided. `kind` says what
the user may answer: `call` (once, or for the tree unless it is `sensitive`), `catalogue` (a line
outside the catalogue run in the project folder: once, or always) or `action` (a change of the
project an agent asked for, `docs/tools.md`, "The project": once). `says` is what it does, in
words, one sentence each: what the user allows (empty for a `call`).

#### `approval.list`

`{"tree"?}`. `{"requests": [...]}`: the pending requests the user holds, oldest first. A request
held by an agent is not there. With `tree`, an agent's id: every request an agent of its root's
tree asked that reached the user, pending, decided or cancelled, oldest first (`not-found` when
there is no such agent).

#### `approval.approve`

`{"number", "for_tree"?, "always"?}`. Allows the request, once, with `for_tree: true` for the
root's tree (the same call goes through without asking from then on), or with `always: true` for a
`catalogue` request: its line is added to the project's catalogue as allowed always
(`[commands.<name>]` with `added_by = "approval"` in its file, what is around it kept), then run. Answered `{"request"}` once it
is decided; the call is then checked again, performed in the background, and its answer stored for
the agent (`approval.settled`). An `action` is done only while what it `says` still holds (a
procedure edited meanwhile is not started). `not-found` when there is no such request;
`invalid-params` for both `for_tree` and `always`; `refused` when it is held by an agent, already
decided, with `for_tree` when it reaches a sensitive place, is held by a procedure's `ask` grant
(`docs/procedures.md`) or is not a `call`, and with `always` when it is not a `catalogue`
request or reaches a sensitive place.

#### `approval.deny`

`{"number"}`. Denies the request: `{"request"}`, and the agent's answer is stored at once. The same
errors as `approval.approve`.
