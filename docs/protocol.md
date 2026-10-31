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
  "max_children": 12,
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

#### `project.repositories`

`{"name"}`. The project's repositories as their checkouts stand, for its page in the window:
`{"name", "folder", "repositories": [{"path", "found", "branch", "remote", "url", "forge", "base",
"branches"}], "new"}`. `found` is false when the folder no longer holds the repository; `branch` is
the branch its checkout is on (`null` when on none); `url` the remote's address, credentials taken
out and masked, `null` when it has no such remote: the window says "local only"; `forge` the one
declared; `base` its base rule; `branches` its local branches. `new` lists the repositories the
folder holds that the file does not name, detected again as `project.detect` detects them.

#### `project.add_repository`

`{"name", "path"}`. Writes a `[[repositories]]` with that `path` (relative to the folder, `.` for
the folder itself) in the project's file, where it stands, what is around it kept, the other keys
left to their defaults (`origin`, base `["main"]`), and answers as `project.repositories`.
`refused`, with nothing written, when the path leads out of the folder, holds no `.git`, or is
already one of the project's.

#### `project.remove_repository`

`{"name", "path"}`. Removes that repository's `[[repositories]]` from the file, its comments
going with it and everything else kept, and answers as `project.repositories`. The project only
forgets it: its folder, its branches and the worktrees of the workspaces that took it are never
touched. `refused`, with nothing written, when it is not one of the project's, is the last one,
or the file would no longer be one the daemon can use (another repository depends on it, a task
or a setup step runs in it…), with the key at fault.

#### `project.settings`

`{"name"}`. The project's forges and tracker as its Settings show them (`docs/forges.md`, "The
forge of a repository"): `{"name", "folder", "repositories": [{"path", "remote", "depends_on",
"url", "forge", "proposal", "disagrees"}], "tracker", "obsolete"}`. `depends_on` lists the paths
of the repositories it depends on (`[]` when none), `url` is the remote's address (credentials taken
out, masked; `null` when it has none), `forge` the one declared (`null` when none), `proposal` what
the address proposes (`null` when nothing), `disagrees` why the declared forge is not where the
remote leads (`null` when it is). `obsolete` lists the keys of the file that are no longer read
(the old `[forges]`, `[agent_actions]`, a command's former `grants` and `children`), each with what
replaces it. Nothing is asked of a forge.

#### `project.set_settings`

`{"name", "repositories": [{"path", "forge", "base", "depends_on"}], "tracker"}`. Writes each listed
repository's `forge` (removed when `null`; a repository left out is unchanged), its `base` (the base
branches in order of preference, one at least) and its `depends_on` (the paths of the repositories it
depends on, `[]` for none) when they are given, and the project's `[tracker]` (removed when `null`
or left out) in its file, where they stand, what is around them kept, and answers as
`project.settings`. A `base` or a `depends_on` left out keeps the file's: a client that knows only
the forge is not affected. A change of base is used by the workspaces created after it; the ones
already made keep theirs. `refused`, with nothing written, when a path is not one of the project's,
a forge or the tracker cannot be used (naming its key), a forge is not where its repository's
remote leads (`api cannot be declared on GitHub acme/api: its remote origin: it leads to
github.com/other/api, not to GitHub acme/api; nothing was saved`), a base branch is not on the
remote (`web: trunk is not a branch of the remote origin; nothing was saved`, read with `git
ls-remote`; from the tracking refs last fetched when the remote cannot be read, from the repository's
own branches when it has no such remote), or a dependency is on the repository itself, on one the
project lacks or closes a cycle (`projects/app.toml: repositories[0].depends_on: cycle web -> api
-> web; nothing was saved`).

#### `project.ticket`

`{"project", "ticket"}`. Reads a ticket of the project's tracker before any workspace has it, for
the new-workspace form's preview (`docs/tickets.md`): `{"project", "ticket", "tracker",
"tracker_said", "problem"}` and, once read, the fields of `workspace.ticket`. `problem` says why it
is not taken or not read; it is not a refusal.

### The rules methods

`rules.show`, `rules.set` and `rules.reset` read, write and reset how agents are told to work
(`docs/projects.md`, "Rules"). Each takes an optional `project`: without it they act on the user's
rules for all projects (`rules.md` in Flottille's configuration folder), with it on the project's
own (the `rules` key of its file, where it stands, what is around it kept).

- `rules.show {project?}` answers `{"project", "text", "source", "cut", "default", "global"}`: the
  text agents created from now on get at that level, its `source` (`default`, `global` or
  `project`), why it was cut to the limit (`null` when it was not), Flottille's default text, and
  the text a project without its own follows.
- `rules.set {project?, text}` writes the text and answers as `rules.show`. `refused` above
  8,000 characters. An empty text is no rules.
- `rules.reset {project?}` removes the file or the key, and answers as `rules.show`.

A change to the user's rules emits `rules.changed {project: null}`; a project's emits
`project.changed` like any edit of its file.

### The judge methods

`judge.show`, `judge.set` and `judge.reset` read, write and reset the Auto judge's guidance
(`docs/design.md` §7.1): how it weighs a call, the part of its instructions the user edits. The
user's is `judge.md` in Flottille's configuration folder, absent while Flottille's default
(`crates/flottille/roles/judge.md`) holds. It is read at each judgement: a change reaches the next
one. The guidance sits between two fixed parts of the judge's instructions
(`crates/flottille/roles/judge-base.md`), and it changes nothing of what reaches the judge (base
calls never do) nor of what its answer may be (`allow` or `ask`).

- `judge.show` answers `{"text", "source", "error", "default"}`: the guidance the next judgement
  follows, its `source` (`default`, or `global` for the user's), why the judge cannot follow the
  user's (`null` when it can), and Flottille's default. `judge.md` that cannot be read, is not a
  regular file, is not UTF-8 text or holds more than 4,000 characters is an `error`, `text` being
  what of it could be read (no more than 4,000 characters can take is ever read): until it is
  fixed, every judgement fails with that reason, and so asks. A `judge.md` of only spaces is the
  default, as an empty `judge.set` is.
- `judge.set {text}` writes the text and answers as `judge.show`. `refused` above 4,000
  characters. An empty text, or only spaces, resets it as `judge.reset` does.
- `judge.reset` removes the file and answers as `judge.show`.

Each emits `judge.changed`. The user's door alone reaches them: no tool of an agent does.

### The developer methods

`developer.show` and `developer.set` read and switch the developer mode (`docs/developer.md`): a
log of what the daemon decides and why, `log = true` in `developer.toml` of Flottille's
configuration folder.

- `developer.show` answers `{"log", "file"}`: whether the log is written, as the file says now
  (read again, the daemon follows it), and the log's path. `invalid-config` when the file cannot be
  read, or its `log` is not `true` or `false`.
- `developer.set {log}` writes the file and answers as `developer.show`.

`developer.set` emits `developer.changed`. The user's door alone reaches them: no tool of an agent
does.

### The skill methods

A project's skills (`docs/skills.md`). The daemon edits the project's file in place, the rest of it
left as written, and announces `project.changed`; a text skill's text is written to Flottille's
data folder, the file replaced at once. `not-found` for a project or a skill that does not exist,
`refused` for a skill that cannot be used, with its key. The `reference.*` methods of earlier
versions are gone.

A skill in a result is `{"name", "description", "kind", "path"}`: `kind` is `path` or `text`, and
`path` is `null` for a text. `skill.show` adds `text` for a text skill.

#### `skill.list`

`{"project"}`. The result is `{"project", "skills": [...]}`, by name.

#### `skill.show`

`{"project", "name"}`: the skill, with its text when Flottille keeps it.

#### `skill.add`

`{"project", "name", "description"}` with one of `"path"` and `"text"`. `path` is relative to the
project folder; a trailing `/` is dropped. `refused` when the name is already used, when both or
neither of `path` and `text` are given, when `path` does not exist, leaves the project folder (a
link followed) or goes through a `.git`, when `text` is empty, or when the name or the description
is not valid. The result is the skill.

#### `skill.edit`

`{"project", "name", "description"?, "path"?, "text"?}`: what is given replaces what the skill had
(at most one of `path` and `text`); a text skill made a path loses its text. Answers the skill.

#### `skill.detect`

`{"project"}`: what the project folder's `.claude/skills` holds (`docs/skills.md`, "Importing a
`.claude` folder"): `{"project", "folder": ".claude/skills", "found": [...]}`, by path. Each file is
`{"name", "path", "state", "description"?, "problem"?}`: `name` is `null` when the file does not
give a usable one, `path` is the `SKILL.md` relative to the project folder, `state` is `new` (it
can be imported), `imported` (the project has a skill of that name pointing at that file), `taken`
(the project has something else under that name) or `unusable`, and `problem` says why for the last
two. Nothing is written.

#### `skill.import`

`{"project", "names"?}`: makes the skills detected `new` into entries of the project's file that
point at their files, `[skills.<name>]` with `path`, the rest of the file left as written and
`project.changed` announced. Without `names`, every file that can be; the answer is `{"project",
"imported": [skill...], "skipped": [{"name", "path", "why"}]}`, `skipped` holding the files that
cannot be (an `imported` one is neither). With `names`, those, and a name that cannot be imported
(`imported`, `taken` or `unusable`) is `refused` with why and nothing is written; a name no file
gives is `not-found`.

#### `skill.remove`

`{"project", "name"}`: `{"project", "removed": "<name>"}`, its text removed with it.

### The workspace methods

A workspace is one worktree per repository, on one branch, prepared by the project's setup
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
      "fetched_now": true, "fetched_at": 1792843730790}},
    {"path": "lib", "base": {"branch": "main", "remote": "origin", "ref": "refs/heads/main",
      "commit": "9d1c0e4f6a2b8c3d5e7f9a1b2c3d4e5f6a7b8c9d", "fetched_now": false, "fetched_at": null,
      "note": "no remote origin: started from the local main"}}
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
creation, not yet filled. A base's `fetched_now` is false when the workspace was created offline or from a local branch;
`note` is there when it started from the repository's own branch rather than the remote's, and
says why in one line (`no remote origin: started from the local main`, `origin could not be
fetched (…): started from origin/develop as last fetched`, `…: started from the local main`);
`fetched_at` (milliseconds since the Unix epoch) is when its tracking ref was last updated, `null`
when unknown. Its `urls` are where its remote led at creation, fetch and push, credentials taken
out (`docs/commands.md`); absent for a workspace created before they were recorded. A step's `kind` is `worktree`, `copy` (`globs`), `link` (`path`), `root_entries`
(`except`: the top-level names of the project's repositories) or `run` (`line`, `time_limit` in
seconds, and `task` when it is one of the project's tasks); its `state` is `pending`, `running`, `done`, `skipped` or
`failed`, and `failure` is there only on a failed step. `repository` is `null` for a step done in
the workspace folder.

Each method answers `not-found` when the project or the workspace does not exist;
`workspace.create`, the only one that reads the project's file, answers `invalid-config` when it
cannot be used.

#### `workspace.create`

`{"project", "name", "repositories"?, "branch"?, "bases"?, "offline"?, "ticket"?}`. Fetches the
base of each repository, writes the workspace, makes the worktrees and answers the workspace; the
rest of the setup goes on in the background. `repositories` is the project's default set when left
out, `branch` is `<branch prefix>/<name>`, `bases` (`{"api": "main"}`) gives a repository it takes
the branch it starts from in place of its base rule, `offline: true` fetches nothing, and `ticket` (`#123`, `123`
or a Jira key) links the workspace to a ticket (`tickets.md`); the workspace's `ticket` says it, in
every answer that shows a workspace, `null` when it has none.

`refused`, with nothing written nor made, when the name is not one, the ticket is not one, a
repository is not the project's, a base is chosen for a repository it does not take or is not a
branch name, the branch is not one git accepts, the project has a workspace of that name, the folder
exists, the branch exists in a repository, or a repository has none of its base branches, neither
on its remote nor itself. A fetch is never a requirement: a repository with no remote, whose fetch
fails, or whose remote has none of its base branches starts from the first of them it has itself,
and its base's `note` says so. A worktree that cannot be made is not a refusal: the
workspace is answered `failed`, with the step's failure.

#### `workspace.list`

`{"project"?}`. `{"workspaces": [...]}`, every workspace or the project's, by project then name.

#### `workspace.show`

`{"project", "name"}`. The workspace, each repository with `changes`: `{"added", "removed",
"capped"}`, the lines added and removed in its worktree since its base commit, whether its branch
committed them or they are not committed yet, new files git does not ignore counted whole (a
binary file, a link, anything but a regular file, or a file over 8 MiB counts no line; the files a
setup `copy` step wrote and that are still as it copied them are not changes, and are left out). At most
1000 new files and 16 MiB of them are read: past that the count stops and `capped` is `true`, the
real count being higher. It writes nothing, not even git's index. `changes` is `null` when git
cannot read the worktree (not made yet, or gone). `workspace.list` leaves it out: it runs no git.
Each repository also has `branch`, the branch its worktree is on (`null` when on none), `ahead`,
its commits since its base commit, and `uncommitted`, how many files `git status` shows (the links
Flottille made, the files a setup `copy` step wrote and that are still as it copied them, and the
worktrees inside it left out), each `null` when its worktree is not there.
`ready_at` is when its last preparation ended, the time of its newest `workspace.ready` event
(`docs/events.md`), kept with the workspace; `null` while it is not `ready`. `workspace.list`
leaves it out too.

#### `workspace.step_output`

`{"project", "name", "position"}`. One step of the workspace's preparation (`position` as in its
`steps`) and what it wrote, for the window to open from the project page in the run drawer:

```json
{"project": "app", "workspace": "login", "position": 3, "repository": "api", "kind": "run",
 "line": "npm install (in api)", "folder": "/work/app-work/login/api", "state": "running",
 "started_at": 1792843731200, "ended_at": null, "output": "added 212 packages", "earlier": false}
```

`line` is what the step does (the `doing` of a failure), `folder` where it does it, `state` and the
times are the step's. `output` is the last 1000 lines of what the step wrote, standard output and
error together, masked and as the program wrote it, and `earlier` says whether lines before them
were left out. It is kept as a run's output is (the last 64 KiB, cut at a line's start), read while
the step runs and kept in the store once it ended, masked before it is stored. A step that fails
without a program to speak (a `link` whose target is missing) has its reason as its output; a `run`
that wrote nothing has `it ended with exit N`. A step that never ran has none. `not-found` when the
project, the workspace or the step does not exist.

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
     "pull_request": {"number": 12, "title": "Add the login page", "state": "open", "head": "app/login",
       "base": "develop", "mergeable": true, "url": "https://github.com/acme/api/pull/12", "review": "approved",
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
read. A pull request's `state` is `open`, `draft`, `merged` or `closed`; `head` is its branch and `base`
the one it merges into; `mergeable` is `true`, `false` (it conflicts) or `null` while the forge does
not say; `review` `approved`,
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

`{"project", "name"}`. `{"removed": "<name>", "project": "<project>", "branch": "<branch>",
"branches": [{"repository", "deleted", "why"}]}` once its worktrees, its folder and its record are
gone: `branches` says, for each repository that still had the workspace's local branch, whether it
was deleted because its work landed and why (`"it has no commit of its own"`, `"its pull request
#12 is merged"`), or why it is kept (`"its pull request #12 is open, not merged"`, `"the forge
cannot say whether it is merged: …"`); the remote branch is never deleted (`docs/workspaces.md`,
"Removal"). `refused`, with nothing touched, while it is being prepared, or when a worktree holds a
changed or untracked file or a commit on no remote, unless the forge says the branch's pull
request merged it: the message names every one, and the branch that holds the commit.

### The command methods

A project's commands (`docs/commands.md`). A command in a result is `{"name", "description"}`,
and `"path"` when it points at a file (relative to the project folder); `command.show` and the
edits add `instructions`, those of a command that points at a file read from it now, or `"problem"`
(the file and why) when they cannot be read. A command grants nothing: it is a message, sent
as `/name` with `agent.start`, `agent.run` or `agent.send`, and each call the session makes goes
through the gate's classes and the session's mode. `not-found` for a project or a command that
does not exist, `refused` for an empty description or instructions (or instructions given to a
command that points at a file), `invalid-config` when the
project's file cannot be used. The `procedure.*` methods of earlier versions, the runs with them,
are gone.

#### `command.list`

`{"project"}`: `{"project", "commands": [...]}`, by name.

#### `command.show`

`{"project", "name"}`: the command.

#### `command.add`

`{"project", "name", "description", "instructions"}`, or `{"project", "template", "name"?}` for one
of Flottille's templates as it is (`open-prs`, `merge-stack`, `close`; `name` the template's when
left out, and nothing else given). Writes `[commands.<name>]` in the project's file, what is
around it kept, and answers the command. `refused` when the project has one of that name,
`not-found` for a template Flottille has not.

#### `command.edit`

`{"project", "name", "description"?, "instructions"?}`: what is given replaces what the command
had. Answers the command.

#### `command.detect`

`{"project"}`: what the project folder's `.claude/commands` holds, in the shape of `skill.detect`
(`"folder": ".claude/commands"`, each `.md` file named after the file, its `description` the front
matter's or the first line of its text).

#### `command.import`

`{"project", "names"?}`: as `skill.import`, into `[commands.<name>]` with `path`, answering
`{"project", "imported": [command...], "skipped": [...]}`.

#### `command.remove`

`{"project", "name"}`: `{"project", "removed": "<name>"}`.

### The task methods

A project's tasks (`docs/projects.md`, "Tasks"), as its Settings edit them. A task in a result is
`{"name", "line", "repository", "serve", "check", "read_only", "ask_before_running"}`,
`repository` `null` for the place's top. Each write is checked as the file is, and written under
`[tasks.<name>]` where it stands, what is around it kept, a flag that is off left out.
`not-found` for a project or a task that does not exist, `refused` for a task the file would
refuse, `invalid-config` when the project's file cannot be used.

#### `task.list`

`{"project"}`: `{"project", "tasks": [...]}`, by name.

#### `task.show`

`{"project", "name"}`: the task.

#### `task.add`

`{"project", "name", "line", "repository"?, "serve"?, "check"?, "read_only"?,
"ask_before_running"?}`: the task. `refused` when the project has one of that name.

#### `task.edit`

`{"project", "name", "line"?, "repository"?, "serve"?, "check"?, "read_only"?,
"ask_before_running"?}`: what is given replaces what the task had (`repository: null` puts it back
at the top). Answers the task.

#### `task.remove`

`{"project", "name"}`: `{"project", "removed": "<name>"}`. `refused` while a setup step runs it.

#### `task.run`

`{"project", "name"}`: `{"project", "name", "run"}`, at once. The user runs the task in the
project folder with the project's variables, as a run of no agent: no gate, no delivery
(`docs/tools.md`, "Tasks and runs"). A task that keeps running (`serve`) and already runs for the
user is joined: its run is answered. It is followed with the run methods.

### The setup methods

A project's setup (`docs/projects.md`, "The setup"): the steps that prepare each new workspace,
as `[[setup]]` tables, in order. A step in a result is the step's keys as the file has them, and
`number`, its place from 1. Each write is checked as the file is, and the tables are edited
where they stand: a step keeps the comments above it, and moves with them. Each answers
`{"project", "steps": [...]}`, the whole setup after it. `not-found` for a project or a number it
does not have, `refused` for a step the file would refuse, `invalid-config` when the file's setup
is not `[[setup]]` tables.

#### `setup.list`

`{"project"}`.

#### `setup.add`

`{"project", "step", "at"?}`: `at` the number it takes, the last when left out.

#### `setup.edit`

`{"project", "number", "step"}`: the step replaced, whole.

#### `setup.remove`

`{"project", "number"}`.

#### `setup.move`

`{"project", "number", "to"}`: the step takes number `to`, the others between moving by one.

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
  "launched_by": null,
  "reported_at": null,
  "quiet_turns": 0,
  "waiting_on_answer": false,
  "gate_mode": "auto",
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
(each replacement adds one), `title` and `brief` a child's task, `launched_by` the id of the agent
that started a root for the user (`null` for a root the user started, and for a child), whose
`brief` is then that agent's first message, never the user's, `reported_at` when it reported,
`quiet_turns` its turns ended without a report in a row, `waiting_on_answer` whether a question of
it waits for an answer: a child's from its parent, a root's from the user, until that one writes to
it (`docs/agents.md`, "The tree"). `gate_mode` is its session's mode, `ask` or `auto`
(`agent.mode`).

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
A message `/name rest`, where name is one of the project's commands, is sent as that command
(`docs/commands.md`, "Sending one"), here and in `agent.send`.

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
stay ended. Its next prompt starts with a
`resume` delivery saying the user stopped it and naming its requests cancelled then. `refused`
for a child (the sentence names its root), an agent that has not ended, a failed one (its session
went with it), one whose agent can neither resume nor load a session, that never opened one or
whose session was forgotten (7 days after it ended), one whose workspace or project is gone: what
`resumable` says, in short (`agent.tree`); and one whose last process is still stopping
(`stopping`), with a sentence to try again in a moment.

#### `agent.mode`

`{"agent", "mode"}`, `mode` `ask` or `auto`. Puts the agent's tree in that mode (`docs/design.md`
§7.1): how the gate answers its calls to judge from then on. A tree has one mode, its root's: a
child named switches its root, and every agent of the tree reads the root's mode at each call
(an agent's `gate_mode` is its root's). A new session starts in `auto`, and the mode is kept when
it resumes. Answers `{"agent"}`, the root, with its `gate_mode`, and emits
`agent.gate_mode_changed` for the root (`docs/events.md`).
`invalid-params` for another mode, `not-found` for an agent that does not exist. The user's door
alone reaches it: no tool of an agent changes a mode.

#### `agent.tree`

`{"agent"}`. The whole tree of the agent's root, each agent before its children:
`{"agents": [{"agent", "doing", "waiting_on", "needs_user", "context", "queued"}]}`. `agent` is the agent in short
(`id`, `provider`, `role`, `title`, `project`, `workspace`, `parent`, `launched_by`, `depth`, `epoch`, `state`,
`state_reason`, `created_at`, `active_at`, `ended_at`, `waiting_on_answer`, `model`: the model it
took at the last start of its process, else the one asked, `null` for the agent's default before it
started; `resumable`: whether `agent.resume` takes it once its last process is gone; `stopping`:
whether the process of an agent that ended is still stopping; `images`: as in the agent's full
form); `doing` a sentence of what it does
(`working`, `idle`, `running \`cargo test\` for 12 min`, `waiting for approval #3`, `failed: …`);
`waiting_on` `parent` for a child whose request or question waits for its parent, `user` for a
root whose own does, else `null`; `needs_user`, for the root, whether a request of its tree or its
own question waits for the user; `context`, how much of its context the agent used at its last
report (`agent.usage`), in percent, `null` before one; `queued`, for the root, the messages the
user wrote to it that wait for it to read them between its turns, the oldest first, each
`{"delivery", "text", "images", "at"}` (its text masked as it is stored, how many pictures go with
it, when it was written), empty for a child.

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

### The run methods

The commands the agents run with `run` (`docs/tools.md`), and the tasks the user runs with
`task.run`, as the user sees them. A command in a result:

```json
{
  "run": "5d2c0f1a9b3e",
  "agent": "3f9a1c0b7d2e",
  "root": "3f9a1c0b7d2e",
  "project": "app",
  "workspace": "login",
  "name": "server",
  "line": "npm run dev",
  "folder": "/home/me/.local/share/flottille/workspaces/app/login",
  "state": "running",
  "exit_code": null,
  "signal": null,
  "started_at": 1792843730808,
  "ended_at": null,
  "last_line": "listening on :3000"
}
```

`name` is the task it runs, `null` for a line of the agent's. `agent` and `root` are `null` for a
task the user ran; `project` is always the project's. `state` is `running`,
`exited`, `stopped` or `interrupted` (`docs/events.md`, "The events of a run"). `last_line` is the
last line of its output with something on it, masked of secrets and as the program wrote it
(colours included), cut at 300 characters. `tail` is the last 20 lines
of its output (a list of texts, masked and as written, each cut at 300 characters), which a result
card in the conversation shows folded, a running command's as well as one that ended.

#### `run.list`

`{"project"?, "tree"?, "ended"?, "user"?}`. `{"commands": [...]}`: the commands running, oldest
first. With `project`, the project's, the user's included; with `tree`, an agent's id, those of the
agents of its root's tree; without either, every one. With `user` (and `project`, required), only
the tasks the user ran in the project, as its page lists them. With `ended`, the commands that ended and were not dismissed come after them,
the last 30 to have ended at most, and none for a tree whose root ended or failed: they go with
their session, and the daemon forgets them when the session is over for good (`docs/tools.md`)
(`not-found` when `tree` names no agent).

#### `run.output`

`{"run", "tail"?}`. The command without `root` and `workspace`, with `output`, the last
`tail` lines (200 by default, 1 to 1000) of what it wrote, standard output and error together, masked
and as the program wrote it, and `earlier`, whether lines before them were left out. Of a
command running, what it wrote so far. `not-found` when there is no such run.

#### `run.stop`

`{"run"}`. `{"run": {...}}`: stops the command and everything in its process group (`SIGTERM`, then
`SIGKILL` 5 s later, as `run_status` with `action: stop` does) and answers once it ended. The
agent that started it, if one did, is told the user stopped it (`command-ended`, `docs/tools.md`). A command that
already ended is answered as it is. `not-found` when there is no such run.

#### `run.dismiss`

`{"run"}`. `{"run"}`: a command that ended is no longer listed with `ended`, and `run.dismissed` is
emitted. `refused` while it runs; `not-found` when there is no such run.

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
  "project": {"name": "app", "provider": "claude", "model": "sonnet"},
  "judge": {"provider": "claude", "model": "haiku"}
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
none is enabled, its `model` `null` when the project names none. `judge` is the Auto judge the user
chose (`provider.set_judge`) or Flottille picked, its `model` `null` for the provider's default,
`"picked": true` while the user has not seen the notice of a judge Flottille picked, or `null` when
there is none. `not-found` when the project
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

### `provider.set_judge`

`{"provider", "model"?}`. Chooses the Auto judge (`docs/design.md` §7.1): the provider whose agent
judges the calls to judge of sessions in Auto, and its model (the provider's default when left
out or empty; once the provider's models are known, one given without its date suffix is written as the
listed id, `docs/agents.md`, "Providers"); `provider` `null` chooses none, and Auto then asks the user as Ask does. The daemon
writes `[judge]` in `providers.toml` (`provider`, `model`), or `judge = "none"` for none, so that
no judge is ever picked for a user who chose none; the rest of the file is
kept as `provider.enable` keeps it, and answers `{"judge"}`, as `provider.list` gives it. When this
changes the file it emits `provider.judge_changed`. `not-found` when there is no provider of that
name; `invalid-config` when `providers.toml` cannot be read. The provider is checked when it
judges: one not enabled or not ready makes each judgement fail, which asks the user. The user's
door alone reaches it: no tool of an agent chooses the judge.

When `providers.toml` says nothing of the judge (no `[judge]`, no `judge = "none"`), the daemon
picks one, at its start and at the first call to judge after a provider became ready: the
smallest model of the first provider, in Flottille's order, that is enabled, installed (its program
on `PATH`) and signed in where Flottille can see it, among those whose smallest model Flottille
knows (`providers::SMALLEST`: Claude's `haiku`). It writes `[judge]` once, with `picked = true`
and a comment that says so, and emits `provider.judge_changed`. Until `judge = "none"` existed,
choosing no judge removed `[judge]`: when the last `provider.judge_changed` in the store names no
judge, the daemon writes `judge = "none"` and picks nothing. When it names one (the user's, or one
picked), a `[judge]` removed by hand is no choice of none: a judge is picked again.

### `provider.judge_noticed`

`{}`. The user saw the notice of the judge Flottille picked: the daemon removes `picked` from
`[judge]`, which stays the user's judge, and emits `provider.judge_changed`. Answers `{}`; nothing
changes when the judge was not picked.

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
  "class": "base",
  "kind": "call",
  "says": [],
  "judgement": null,
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
ended first; `decision` is `deny` or `once` once it is decided (a request decided `tree` or
`always` before the classes reads as `once`). `class` is `judge` or `base` (`docs/tools.md`, "The
gate"). `kind` is `call`, or `action` (a change of the project an agent asked for, `docs/tools.md`,
"The project"). `says` is what it does, in words, one sentence each: what the user allows (empty
for a `call`). `judgement` is the judge's verdict when one was asked in Auto, else `null`:
`{"by", "verdict", "reason"}`, `by` the provider and model that judged (`claude haiku`),
`verdict` `ask` or `failed` (it could not decide: `reason` says why), `reason` its sentence.

#### `approval.list`

`{"tree"?}`. `{"requests": [...]}`: the pending requests the user holds, oldest first. A request
held by an agent is not there. With `tree`, an agent's id: every request an agent of its root's
tree asked that reached the user, pending, decided or cancelled, oldest first (`not-found` when
there is no such agent).

#### `approval.approve`

`{"number", "discard"?}`. Allows the request, once: nothing it answers allows a later call.
`discard`, `true` for a workspace's removal only (`refused` for any other request), removes it
even with what would be lost (`docs/workspaces.md`, "Removal"). Answered `{"request"}` once it
is decided; the call is then checked again, performed in the background, and its answer stored for
the agent (`approval.settled`). An `action` is done only while what it `says` still holds (a
command edited meanwhile is not written). `not-found` when there is no such request; `refused` when it is held by an agent, or already
decided.

#### `approval.deny`

`{"number"}`. Denies the request: `{"request"}`, and the agent's answer is stored at once. The same
errors as `approval.approve`.
