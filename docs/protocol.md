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
| `client` | Optional: what the client is. `window` makes the daemon read the items at the window's rhythm for as long as this connection is open ([`items.md`](items.md), "Rhythm"). |

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
folder here. It is where the project's workspaces are made now (`docs/projects.md`, "Where the
workspaces go"); `workspaces_root_own` is added, as the file says it (`~` and `<project>` not yet
filled in), when the project names a folder of its own. `default_repositories` is `"all"` or a list of repository paths; `variables` and
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
read, or one that is not a repository of its own yet). `project.list` leaves it out: it runs no git.
When the project follows a default that cannot be followed (`workspaces.toml` is invalid, see
`workspaces_folder.show`), the result also holds `workspaces_default_error`, the reason; the data
folder is what `workspaces_root` then is. `not-found` when there is none,
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

#### `project.removal`

`{"name"}`. What removing the project would do, changing nothing (`docs/projects.md`, "Removing a
project"): `{"name", "sessions": [{"id", "title", "parent"}], "runs": [{"run", "name", "line",
"agent"}], "workspaces": [{"name", "branch", "state", "losses", "digest"}]}`. `sessions` are its roots not
ended, `runs` its command runs still running, `workspaces` by name, each `state` `clean`,
`uncommitted` (files not committed, whatever else), `unpushed` (only commits on no remote) or
`unknown` (its state could not be read, `losses` saying why), `losses` the sentences
`workspace.remove` would refuse with, `digest` an opaque string for the state its worktrees were
read in (each one's commit, `git status` and the content of each file that names), empty when it
could not be read. `not-found` when there is no such project.

#### `project.remove`

`{"name", "discard"?: [{"name", "losses", "digest"}]}`. Stops the project's agents (roots and children) and
waits for their processes, stops its command runs, removes its workspaces that would lose
nothing, then removes the project's file (`docs/projects.md`, "Removing a project"). `discard` is
what the user was shown and agreed to lose: the workspaces of `project.removal`, each with its
`losses` and its `digest` as given there (`digest` is required). A workspace that would lose work
is removed too only when it is listed, each of its losses now is among those listed and its digest
now is the one listed, read again right before its forced removal; otherwise it is kept, with
`"changed": true`. A file not committed is one loss whatever it holds: the digest is what tells
that more was written in it since.
The result says what was done:

```json
{"removed": "shop",
 "stopped": {"agents": [{"id", "title", "parent"}], "runs": [{"run", "name", "line", "agent"}]},
 "workspaces": {"removed": [{"name", "branch", "branches"}], "kept": [{"name", "why": ["…"], "changed"?: true}]}}
```

`branches` as `workspace.remove` gives them. The folder and its repositories' checkouts are never
touched. Meanwhile nothing starts in the project: `agent.run`, `agent.start`, `agent.resume`,
`workspace.create`, `task.run` and every edit of the project's file are `refused`, "project
<name> is being removed". `not-found`
when there is no such project; `refused` when it is being removed already, or when a step failed
partway, the project still known and the sentence saying what failed and what was done.

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

#### `project.programs`

`{"name"}`. The command lines the project's forges and tracker need, checked (`docs/forges.md`,
"The programs it needs"): `{"repositories": [state…], "tracker": state | null}`, one state for each
repository that declares a forge (with its `repository`, that repository's path in the project),
and the tracker's when the project declares one. A program is asked once for a host
(and, for `bkt`, a workspace or project) however many repositories use it, all together. A state is:

```json
{"program": "gh", "host": "github.com", "found": true, "path": "/usr/bin/gh", "version": "2.60.0",
 "signed_in": false, "signed_in_to": null, "install": null,
 "sign_in": {"commands": ["gh auth login"]}, "problem": null}
```

`program` is `gh`, `bkt` or `acli`; `host` is GitHub's, the Bitbucket server's or the Jira site;
`path` is where the program is (`null` when it is not found), never the repository's.
`signed_in` is `null` when the program is not found or could not say (`problem` then says why,
masked); `signed_in_to` is the other site `acli` is signed in to when it is not the tracker's
(`signed_in` is then `false` and `sign_in` the switch command). `install` is there when the program is not found: `{"page", "command"}`, the program's
installation page and a command (`null` where none fits most systems); `sign_in` is there when it
is found and signed out: `{"commands": [...]}`, in order. A program is run for `--version` and for
its sign-in check only, never for anything else, and nothing it prints of the account is kept.

#### `project.set_settings`

`{"name", "repositories": [{"path", "forge", "base", "depends_on"}], "tracker", "branch_prefix"?}`. Writes each listed
repository's `forge` (removed when `null`; a repository left out is unchanged), its `base` (the base
branches in order of preference, one at least) and its `depends_on` (the paths of the repositories it
depends on, `[]` for none) when they are given, and the project's `[tracker]` (removed when `null`
or left out) in its file, where they stand, what is around them kept, and answers as
`project.settings` (which also gives the project's `branch_prefix`). `branch_prefix` is written when
given, `""` included (no prefix: the branch of a workspace is its name), and kept when left out;
a space in it is refused (`invalid-config`). A `base` or a `depends_on` left out keeps the file's: a client that knows only
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

#### `project.set_conventions`

`{"name", "branch_format"?, "pull_request_template"?}`. Writes the project's branch format (the
top-level `branch_format`) and pull request template (`[pull_requests]`'s `template`) in its file,
where they stand (`docs/projects.md`, "Branch format", "Pull request template"): each is kept when
left out and removed when empty (a template of blanks is empty). Answers `{"name",
"branch_format", "pull_request_template"}`, `null` for one that is off; `project.settings` gives
them too. `refused`, nothing written, when the branch format is not a regular expression, with the
reason (`the branch format ^(feat is not a regular expression (unclosed group); nothing was
saved`). It is the user's alone: no tool of an agent writes them.

#### `project.set_message_checks`

`{"name", "checks"}`. Writes the project's message checks (`docs/projects.md`, "Message checks"),
every one in order, in its file where they stand: `message_checks`, removed when `checks` is empty
(a blank line is left out). Answers `{"name", "message_checks"}`. `refused`, nothing written, when a
line cannot be split without a shell or does not name `{message}` (`the check `lint x` cannot be
used: must name {message}, the file that holds the message it checks; nothing was saved`). It is the
user's alone: no tool of an agent writes the checks, which Flottille runs outside the gate.

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

### The compaction methods

`compaction.show`, `compaction.set`, `compaction.set_guidance`, `compaction.reset_guidance` and
`compaction.noticed` read and keep Flottille's own compaction (`docs/design.md` §5.8):
`compaction.toml` of Flottille's configuration folder, and the compactor's guidance in
`compaction.md` beside it, absent while Flottille's default (`crates/flottille/roles/compaction.md`)
holds. The guidance sits inside a fixed text (`crates/flottille/roles/compactor-base.md`).

- `compaction.show` answers `{"compactor", "summary", "automatic", "threshold", "maximum", "kept",
  "guidance"}`: who writes the summary (`compactor`, the default, or `agent`: the agent's own
  handoff, `docs/design.md` §5.8, 7), the compactor `{"provider", "model", "effort", "picked"}` (`model` and `effort`
  only when chosen, `picked` only while the user has not seen the notice of a compactor Flottille
  picked) or `null` for none, whether it compacts by itself (default `true`), the threshold in
  percent of the context window (default 70), the maximum size in tokens (`null`, the default, is
  no ceiling), the tokens kept word for word (default 20000), and the guidance as `judge.show`
  gives the judge's (`{"text", "source", "error", "default"}`). A user who never chose gets a
  compactor picked first, as the Auto judge is (`provider.set_judge`): the smallest model of the
  first provider enabled, installed and signed in among those Flottille lists, written once with
  `picked = true` and said once with `compaction.changed`. `invalid-config` when the file holds
  an unknown key or a value out of its range. It also gives the two triggers around the prompt
  cache's expiry (`docs/design.md` §5.8, 6): `before_expiry` (default `false`), `margin` in
  minutes (default 5), `after_expiry` (default `true`), `minimum` in tokens (default 50000), and
  `lifetimes`, each provider's cache lifetime in seconds, `null` where it is not established.
- `compaction.set` takes any of `compactor` (`{"provider", "model", "effort"}`, or `"none"`),
  `summary` (`"compactor"` or `"agent"`), `automatic` (a boolean), `threshold` (50 to 95), `maximum` (a number of tokens from 20000, or
  `"none"`), `kept` (0 to 50000, and never more than a quarter of the size that compacts the
  agent), `before_expiry` and `after_expiry` (booleans), `margin` (1 to 30 minutes) and `minimum`
  (0 to 1000000 tokens); what is left out stays. It answers as `compaction.show`;
  `invalid-params` for anything else, `not-found` for an unknown provider. `compactor = "none"`
  is a choice: no compactor is ever picked again.
- `compaction.set_guidance {text}` writes the guidance (`refused` above 4,000 characters; an
  empty text resets it); `compaction.reset_guidance` removes it. Both answer as
  `compaction.show`.
- `compaction.noticed` drops `picked`: the user saw the notice.

Each that changes something emits `compaction.changed` with what `compaction.show` answers. The
user's door alone reaches them: no tool of an agent does.

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

### The extension methods

`extension.list` and `extension.set` read and switch the extensions (`docs/extensions.md`):
`[<name>]` of `extensions.toml` in Flottille's configuration folder, with `enabled` and the
extension's settings, and a project's own switch and values under `[extensions.<name>]` of its
file. Both files are refused key by key: an unknown extension, an unknown key, a value of the wrong
kind, naming the key at fault. An extension is off by default; once on, a project-scoped one is on
in every project, off in a project whose switch says `enabled = false`, and off everywhere while it
is off globally, the projects' switches kept.

- `extension.list` answers `{"extensions", "errors"}`: for each extension `{"name", "title",
  "sentence", "scope", "on_by_default", "place", "enabled", "error", "settings", "projects",
  "guidance", "commands"}`, `enabled` being the global switch, `error` why `extensions.toml`
  cannot be used (`null` when it can; the extension is then off), `guidance` whether it has one
  (`<name>.show`, below), `commands` the words of its composer commands (`["review"]`),
  `settings` each `{"key", "label", "hint", "kind", "per_project", "value", "set"}` with its
  global value (its default when not `set`), `kind` being `"language"`, `"model"` (a slot,
  `{"provider", "model"?, "effort"?}`, `null` for the extension's own choice) or `{"subset": [...]}`
  (some of those values, in their order), and
  `projects` each `{"project", "enabled", "switch", "settings", "error"}`: whether it is active
  there, the project's own switch (`null` when its file says none) and values. `errors` names the
  projects' `[extensions.<name>]` of an extension Flottille does not know.
- `extension.set {name, project?, enabled?, settings?}` writes the switch and the settings
  (a setting `null` goes back to its default), globally or in the project's file, the rest of the
  file kept; answers the extension as `extension.list` does. `not-found` for an unknown extension,
  `refused` for a key or a value the declaration does not take, `invalid-config` while the file
  cannot be read.

- `extension.act {name, action, …}`: a gesture of the user (the Action trigger), answered by the
  extension's method of that action with the other parameters, as if called by its name
  (`extension.act {"name": "specs", "action": "answer", "spec": 3, "question": 7, "option": 1}`
  is `spec.answer`). `refused` for an action the extension does not declare, naming those it does:
  for Specs `new`, `from_ticket`, `answer`, `ready`, `draft`, `ticket`, `departures`, `develop`,
  `amendment`.

- `<name>.show`, `<name>.set {text}`, `<name>.reset`: the guidance of an extension that has one
  (Review's: `review.show`…), `$XDG_CONFIG_HOME/flottille/<name>.md`, absent while Flottille's
  default holds. Each answers `{"text", "source", "error", "default"}` as `judge.show` does, and
  `set` and `reset` emit `<name>.changed`; `set` with an empty text resets it, and refuses a text
  over the extension's limit (Review: 4,000 characters).

`extension.set` emits `extension.changed`; turning an extension on or off globally gives each
running session of every project the tools its role has now, before the answer (`docs/tools.md`,
"When the tools change"), and a project's own switch does the same through its file. The user's
door alone reaches these methods: no tool of an agent does.

### The spec methods

Specs' own (`docs/extensions.md`, "Specs"), each refused while Specs is not active in the spec's
project, naming the switch. A spec is answered as `{"spec", "project", "title", "mode", "status",
"language", "tickets", "created_at", "updated_at", "problem", "behaviour", "decisions",
"out_of_scope", "notes", "verification", "questions", "asked", "writer"}`: `mode` is `alone`,
`internal` or `ticket`; `status` `draft`, `ready`, `in_progress`, `review` or `done`; `behaviour` a
list of `{"id", "text"}` (`C1`…), `decisions` of `{"id", "question", "decision", "set_aside"}`
(`D1`…), `out_of_scope` of `{"text", "reason", "ticket"}`, `verification` of `{"id", "text"}` for a
`C`, `questions` of texts; `asked` the questions the writing session asked, each `{"question" (its
number), "text", "options", "recommended", "reason", "state" (`open` or `answered`), "decision"
(the `D` it became, `null` while open)}`; `writer` the agent id of the session that writes it,
`null` for a spec no session was started for; `ticket_write` what became of its last write to its
ticket (mode `ticket`), `{"state": "written" | "changed" | "failed", "problem"}` (what it says:
where it was written, which ticket changed, why it failed), `null` before a Ready wrote or since a
Back to draft; `departures` an internal spec's draft comment of where it departs from its tickets,
and `departures_posted_at` when the user posted it, `null` before; `amendments` those its
development sessions proposed, as `spec.amendments` gives them. A `C` or a `D` an accepted
amendment changed or added holds `amended`, a list of `{"at", "was"}`: when it was decided during
development, and what it read before (the `C`'s text, the `D`'s `{"question", "decision",
"set_aside"}`), `null` for one it added; an item no amendment touched has no `amended`, and one
the user rewrites while the spec is a Draft loses it (written again as it was, it keeps it).

- `spec.new {project, title?, idea?, mode?, tickets?}`: a Draft in the project's `language` (its
  own, else the one of `extensions.toml`, else the user's locale), kept with it. `alone` takes no
  ticket, `internal` and `ticket` one at least; the mode is `alone` without tickets and
  `internal` with unless given; a spec `ticket` with no ticket, in a project with a tracker,
  creates it at its first Ready. A ticket is written as the project's tracker takes it
  (`docs/tickets.md`); one another spec covers is refused, naming that spec. With `idea` ("New
  spec"), the title is the idea's first line unless given, and a `lead` session is started for the
  user in the project's main checkout to write it, its first message Flottille's: the method, then
  the idea (`docs/extensions.md`, "Writing a spec"). It answers `{"spec", "agent"}`, the spec and
  the session's agent id; without `idea`, the spec. A title or an idea is needed.
- `spec.from_ticket {project, ticket, mode?}` ("Write the spec"): reads the ticket on the
  project's tracker, makes a Draft of it (its title the ticket's, the mode `internal` unless
  `ticket` is given) and starts its writing session as `spec.new` with `idea` does; the ticket's
  text follows the method in the first message, framed as data. `refused` for a ticket another spec
  covers, before anything is read; `unavailable` when the ticket cannot be read, nothing made.
  Answers `{"spec", "agent"}`.
- `spec.answer {spec, question, option | text}`: answers a question the writing session asked,
  with an option (from 1) or a text in the user's own words. The answer becomes the next `D`: the
  question, the option chosen (or the text), the options set aside (all of them for a text); the
  question is answered, and the writing session is told with a delivery (`[flottille:specs]`).
  `refused` for a question already answered, and past Draft. Answers the spec.
- `spec.list {project?, status?}` answers `{"specs"}`, each `{"spec", "project", "title", "mode",
  "status", "tickets", "questions", "updated_at", "ticket_write"}` (`questions` counts the open
  ones), of the
  projects where Specs is active.
- `spec.show {spec}` answers the spec.
- `spec.write {spec, section, content}` writes one section whole and answers the spec:
  `problem` and `notes` take a text, `behaviour` a list of `{"id"?, "text"}`, `decisions` of
  `{"id"?, "question", "decision", "set_aside"?}`, `out_of_scope` of `{"text", "reason"?,
  "ticket"?}` (one of the two at least), `verification` of `{"id", "text"}` naming a `C` the spec
  has, `questions` a list of texts, `departures` a text, for an internal spec only. An item without `id` gets the next number; one with an `id`
  names one the spec has now; an identifier removed is never given again, and a `C` removed takes
  its verification with it.
- `spec.link {spec, ticket}` and `spec.unlink {spec, ticket}` cover one more ticket, or one no
  more; the last ticket of a spec `internal` or `ticket` stays.
- `spec.ready {spec}` moves a Draft to Ready: `refused` while a question is open (one of
  `questions`, or one asked and not answered) or `behaviour` is empty. In the mode `ticket`, it
  then reads each ticket again and writes the spec to it (`docs/tickets.md`, "What a spec
  writes"), or creates its ticket when it has none, and answers once that is done: the spec, with
  `ticket_write`. A ticket changed since Flottille last read it is not written (`changed`); a
  write that fails says why (`failed`); the spec stays Ready either way. `spec.draft {spec}` moves
  a spec that is Ready, In progress or Review back to Draft, and tells its writing session with a
  delivery; during a development, the session developing it is told too, and `spec`'s `amend` is
  refused while the spec is a Draft. Its pending amendments stay for the user.
- `spec.ticket {spec, choice: "take" | "overwrite"}`: the user's choice once `ticket_write` is
  `changed`. `take` moves the spec back to Draft, keeps the tickets' texts read now as the last
  read and tells the writing session those texts, framed as data; `overwrite` writes the spec over
  the change. `refused` for a spec of another mode and while nothing changed; `unavailable` when a
  ticket cannot be read to be taken, nothing changed. Answers the spec.
- `spec.departures {spec, post?}`: an internal spec's departures, `{"spec", "departures",
  "offered" (Done, and something to post), "posted_at", "tickets"}`. With `post`, posts them on each
  of its tickets as a comment through the tracker's own write, once: `refused` before Done, when
  empty or already posted, and for a spec of another mode; `unavailable` when the tracker refuses.
- `spec.develop {spec}`, "Launch development": makes a workspace `spec-<n>` (`spec-<n>-<k>` when
  that name is taken, `k` from 2: the first free name, whoever made the others) linked to the spec's first ticket (a workspace has one), then starts a
  `lead` session for the user in it, a root neither led nor handed, whose first message, sent once
  the workspace is prepared, is the spec as it is, labelled, and what Flottille asks of the work
  (its pull request's description holds the Problem, the `C` list with their verification, and a
  link to the spec: its ticket's page in the mode `ticket`, else its number and title). Answers
  `{"spec", "agent", "workspace", "branch"}`, the spec In progress. `refused` unless the spec is
  Ready, and while a development of it runs (one at a time); a workspace that cannot be made
  starts nothing, and a session that cannot start once the workspace is made leaves it, named in
  the error, without blocking a later launch. The spec's status is read again when its session
  ends, fails or is resumed, when a pull request of its branch moves, and when it is read
  (`spec.show`, `spec.list`) after Specs was off. Done is terminal. Nothing calls it on its own at Ready.

- `spec.amendments {spec}` answers `{"spec", "amendments"}`, each `{"amendment" (its number),
  "agent" (the development session that proposed it), "section" (`behaviour` or `decisions`),
  "item" (the `C` or `D` it changes, `null` for a new one), "content" (the item proposed:
  `{"text"}` for a `C`, `{"question", "decision", "set_aside"}` for a `D`), "why", "state"
  (`pending`, `accepted` or `refused`), "became" (the identifier it took once accepted),
  "proposed_at", "answered_at"}`, answered ones kept.
- `spec.amendment {spec, amendment, answer: "accept" | "refuse"}`: the user's answer. Accepted, the
  amendment is written into the spec whatever its status but Draft and Done, the one write the
  lock lets through: a changed item keeps its identifier, its old text kept in `amended`; a new
  one takes the next identifier. Refused, the spec is unchanged and the amendment kept. The
  session that proposed it is told the answer with a delivery (`[flottille:specs]`). `refused` for
  an amendment already answered, an accept of a spec that is Done, of a spec back to Draft
  (`spec 3 is back to Draft: make it Ready again before you accept an amendment, or refuse it`),
  one whose item is no longer in the spec, and one whose item changed since it was proposed, by
  another amendment or a rewrite in Draft, which accepting it would undo (`C1 of spec 3 changed
  since amendment 2 was proposed: it would undo what was written since; refuse it`). Answers the
  spec.

**The statuses read from what Flottille sees.** A spec under development is **In progress** while
its development session runs, **Review** while a pull request of a development's branch is open,
**Done** once those pull requests are merged (a closed one counts for nothing), and back to
**Ready** when its session ended or failed with none open. It is read again when one of its
sessions ends or fails (`agent.ended`, `agent.failed`) and when a round of the items sees a pull
request of its project opened, changed, merged or closed: nothing sets a status and forgets it. A
Draft is the user's and stays one.

**The lock.** Past Draft, every write to a spec (`spec.write`, `spec.link`, `spec.unlink`,
`spec.answer`, and the `spec` tool's `write` and `ask`) is refused, whoever asks, the user's door
included: `spec 3 is Ready: it is locked; send it back to Draft first`. Every write passes one
check, in the same transaction as the write. The one write past Draft is an amendment the user
accepts (`spec.amendment`).

`spec.new` and `spec.from_ticket` emit `spec.created`; a write, a question asked or answered, the
writing session recorded, a write to the ticket, the departures posted, an amendment proposed and
an amendment answered `spec.changed`; `spec.ready`, `spec.draft`, `spec.develop` and the statuses read from what Flottille sees
`spec.status_changed`. The
user's door alone reaches these methods: an agent has the `spec` tool (`docs/tools.md`, "Specs"),
whose actions make no spec Ready nor Draft.

### The review methods

Review (`docs/extensions.md`, "Review"), while it is active in the project. The review's agents
and the findings it delivers are told to the session (`docs/tools.md`, "Review").

- `review.start {agent}`: reviews the session's change, as `/review` in its composer does
  (`agent.send` of the message `/review` alone, which is never sent to the agent: it answers
  `{"agent", "after", "delivery": null, "command"}`, `command` this method's answer), the
  `review` tool and `flottille review`. Answers `{"review", "session", "pilot", "round",
  "passes", "files"}`: the pilot launched as the session's child, the passes it may run (the spec
  pass only for a session that develops a spec), the files of the diff. `refused`, before any agent
  starts, while Review is off in the project, for a child or an ended session, for a session in
  the project folder, while a review of the session runs, when the diff from the base is empty
  (or, in a second round, nothing changed and no finding is open), after two rounds, and at the
  children cap. Also `extension.act {"name": "review", "action": "start", "agent"}`.
- `review.list {session? | spec? | project?}`: `{"reviews"}`, each `{"review", "project",
  "session", "workspace", "pilot", "round", "spec", "state", "passes", "passes_done", "files",
  "changed", "summary", "why", "started_at", "ended_at", "agents", "delivered", "findings"}`:
  `state` `running`, `done` or `failed` (its pilot ended without reporting, `why`), `changed` what
  changed in the workspace during it, `agents` its pilot and passes (`{"agent", "kind", "pass"}`),
  `findings` those it kept, each `{"id", "review", "severity", "confidence", "file", "lines",
  "concerns", "quote", "text", "case", "status", "settled_in"}`. Nothing of a project where Review
  is off: its data stays for when it is on again.
- `review.finding {session, finding, status}`: the user marks a finding `open`, `fixed`,
  `dismissed` or `settle` (to settle with them); emits `review.finding_changed`.
- `review.show`, `review.set {text}`, `review.reset`: its guidance (above).

### The appearance methods

`appearance.show` and `appearance.set` read and keep the window's appearance (`docs/design.md`
§10): `theme` and `shade` in `appearance.toml` of Flottille's configuration folder, so that every
window and a restart show the same colours.

- `appearance.show` answers `{"theme", "shade"}`: `theme` is `omarchy` (the desktop's Omarchy
  theme, followed live) or `flottille` (Flottille's own palette), `shade` is `light` or `dark`
  (Flottille's, also used when Omarchy is chosen and no Omarchy theme can be used). With no file,
  `omarchy` and `dark`. `invalid-config` when the file cannot be read or names another word.
- `appearance.set {theme, shade}` writes the file and answers as `appearance.show`;
  `invalid-params` for another word.

`appearance.set` emits `appearance.changed` with what it answers. The user's door alone reaches
them: no tool of an agent does.

### The tool results methods

`tool_results.show` and `tool_results.set` read and keep the limit above which a tool result is
cut at the source (`docs/tools.md`, "Long results"): `limit` in `tool-results.toml` of
Flottille's configuration folder.

- `tool_results.show` answers `{"limit"}`: a number of characters, or `"off"` when results are
  returned whole. With no file, `8000`. `invalid-config` when the file cannot be read, names
  another key, or holds a `limit` that is neither `"off"` nor a whole number from 4000 to 40000.
- `tool_results.set {limit}` takes a number of characters or `"off"`, writes the file
  (`limit = 16000`, or `limit = "off"`) and answers as `tool_results.show`; `invalid-params` for
  anything else, a number out of the range and a missing `limit` included (a client leaves null
  fields out, so `"off"` is a word).

The daemon reads the file at every tool call: a change applies to the next result of every
running session. `tool_results.set` emits `tool_results.changed` with what it answers. The user's
door alone reaches them: no tool of an agent does.

### The sessions methods

`sessions.show` and `sessions.set` read and keep after how many days without activity the daemon
ends a root session (`docs/design.md` §5.7): `end_inactive_days` in `sessions.toml` of Flottille's
configuration folder; and whether a session waiting for its quota's reset is resumed at it
(`docs/design.md` §5.9): `resume_at_reset`, `true` or `false`, false when it is absent.

- `sessions.show` answers `{"days", "resume"}`: a number of days from 1 to 90, or `"off"` (no
  file, no key, or the word), and a boolean. `invalid-config` when the file cannot be read, names
  another key, or holds a value that is neither `"off"` nor a whole number from 1 to 90, or a
  `resume_at_reset` that is not a boolean.
- `sessions.set {days?, resume?}` takes a number of days or `"off"`, a boolean, or both, keeps
  what it is not given as the file says, writes the file (`end_inactive_days = 7`, or
  `end_inactive_days = "off"`, then `resume_at_reset = true` when it is on) and answers as
  `sessions.show`; `invalid-params` for anything else, a number out of the range and neither field
  given included (a client leaves null fields out, so `"off"` is a word). The window sends 7 when
  the user first turns the ending on.

The daemon reads the file when a check is due, at its start and about once an hour. `sessions.set`
emits `sessions.changed` with what it answers. The user's door alone reaches them: no tool of an
agent does.

### The workspaces folder methods

`workspaces_folder.show` and `workspaces_folder.set` read and keep where workspaces go
(`docs/projects.md`, "Where the workspaces go"): the user's default for every project
(`workspaces.toml` of Flottille's configuration folder), or, with a `project`, that project's own
`workspaces_root`.

- `workspaces_folder.show {}` answers `{"place", "path", "error", "data"}`: `place` is `data`,
  `beside` or `own`; `path` is the folder as typed with `own`, else `null`; `error` says why the
  file cannot be followed (the data folder is), else `null`; `data` is Flottille's data folder.
- `workspaces_folder.show {project}` answers `{"project", "own", "folder", "default_folder",
  "fallback"}`: its own folder as typed (`null` when it follows the default), the folder its
  workspaces are made in now, the one the default would give it, and `fallback`, why the default
  is not followed when the project follows it (else `null`). `not-found` for an unknown project.
- `workspaces_folder.set {place, path?}` keeps the default (`path` with `own` only, and needed
  then). `workspaces_folder.set {project, own}` writes the project's key where it stands, the
  file's comments kept, and removes it when `own` is left out or `null`. Both answer as `show`.
  `invalid-params` for a `place` that is not one of the three and for a folder that is not
  absolute once `~` is expanded, with the key at fault first (`path: must be absolute, not "x"`),
  and for a folder `docs/projects.md` forbids (inside a repository of the project, Flottille's
  configuration or data folder, the home folder itself, or a default without `<project>`), the
  reason first; a project's refusal is `invalid-config` and names `workspaces_root`. Nothing is
  written then.

Only the workspaces made afterwards follow a change; none is moved. `workspaces_folder.set` emits
`workspaces_folder.changed` with `{"project"}` (`null` for the default). The user's door alone
reaches them: no tool of an agent does.

### The vault methods

The user's secrets, kept by the keyring, that agents use and never see (`docs/vault.md`). No
method answers a secret, and an error about a call that carried one never repeats it. The user's
door alone reaches them: an agent's only part is the `vault` tool, which lists, and asks the user
for an entry a feature names and that is missing.

- `vault.list {project?}` answers `{"entries": [{"name", "description", "url", "username",
  "project"}]}`, by name; with `project`, the entries its agents see (its own and every project's).
- `vault.set {name, secret, description?, url?, username?, project?}` keeps the secret in the
  keyring and the rest in the store, in place of the entry of that name if there is one, and
  answers the entry as listed. `invalid-params` for a secret that is not a text; `refused` for a
  name that is not letters, digits, `.`, `_` and `-` (64 at most) or a secret under 6 characters;
  `not-found` for an unknown project; `unavailable` when the keyring cannot be reached, with its
  reason. An empty `description`, `url`, `username` or `project` is left out.
- `vault.remove {name}` deletes the secret from the keyring, then the entry, and answers
  `{"removed": <entry>}`; `not-found` when there is none; `unavailable` when the keyring refuses
  (locked), the entry then left whole.

Both writes emit `vault.changed`, and give the agents of every project the tools their roles have
now (`vault` is listed to an agent whose project sees an entry, or misses one a feature names). A
set answers the open requests for that entry whose project now sees it (`set`).

The requests agents made for a missing entry (`docs/vault.md`, "Asking the user for an entry"):

- `vault.asks` answers `{"asks": [{"ask", "at", "agent", "project", "name", "why", "description",
  "takes_username", "taken", "joined"}]}`, the open requests, oldest first; `ask` is the number of
  its `vault.asked` event, `taken` why its name cannot be set for the project (another project
  uses it), `null` otherwise, `joined` the agents whose ask joined it.
- `vault.ask_set {ask, secret, username?, name?}` sets the entry as `vault.set` does, with the
  description and the project the request holds, under `name` when given, else the name it
  holds; `refused` without a username when `takes_username`, and as `vault.set`; `refused` with
  `` `<name>` is used by the project <other>: choose another name`` (`every project` for a global
  entry) when an entry of that name exists and is not the project's: a request never changes an
  existing entry's project; `not-found` when no such request is open. Each agent that asked is
  handed a `vault` delivery, `the user set the vault entry <name>`, which says the project's
  setting still names the entry asked when the user chose another name.
- `vault.ask_dismiss {ask}` closes it, answers `{"dismissed": <request>}`, and hands each agent
  `the user dismissed the request …`. The entry is not asked for again by this project until the
  project's settings change (`project.changed`) or the entry is set (`vault.changed`).

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
out, `branch` is `<branch prefix>/<name>` when left out (the name alone when the project's prefix is empty), and a branch that exists already is checked
out rather than started (`docs/workspaces.md`), `bases` (`{"api": "main"}`) gives a repository it takes
the branch it starts from in place of its base rule, `offline: true` fetches nothing, and `ticket` (`#123`, `123`
or a Jira key) links the workspace to a ticket (`tickets.md`); the workspace's `ticket` says it, in
every answer that shows a workspace, `null` when it has none. A branch outside the project's
branch format is `refused`, naming the pattern, unless every repository resumes it
(`docs/projects.md`, "Branch format"); so is `workspace.rename_branch` to one.

`refused`, with nothing written nor made, when the name is not one, the ticket is not one, a
repository is not the project's, a base is chosen for a repository it does not take or is not a
branch name, the branch is not one git accepts, the project has a workspace of that name, the folder
exists, the branch is checked out in a checkout of a repository (naming it), or a repository has none of its base branches, neither
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
real count being higher. It writes nothing, not even git's index, but the workspace's branch when its worktrees moved to
another (`workspace.changed`, `docs/workspaces.md`, "The branch"). `changes` is `null` when git
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
    {"path": "api", "branch": "app/login", "remote": "origin", "forge": "github", "repository": "acme/api", "note": null,
     "pull_request": {"number": 12, "title": "Add the login page", "state": "open", "head": "app/login",
       "base": "develop", "mergeable": true, "url": "https://github.com/acme/api/pull/12", "review": "approved",
       "checks": [{"name": "build", "state": "passing"}], "mark": "passing", "comments": 2},
     "error": null},
    {"path": "web", "branch": "app/login", "remote": "origin", "forge": null, "repository": null,
     "note": "no forge set for this repository: choose it in the project's Settings (the window), or write its `forge` in the project's file; Flottille never guesses it", "pull_request": null, "error": null}
  ]
}
```

`problem` says, with what to do, why a forge cannot be read (`gh` or `bkt` missing, not signed in;
several are joined by `; `), else `null`; the repositories it stops have no `pull_request`. For
each repository, `branch` is the one it was read for, the branch its worktree is on (`docs/workspaces.md`, "The branch"), `forge` is `github`, `bitbucket` or `null` (then `note` says why), `repository`
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
  "attachments": [],
  "read_at": 1792843730808,
  "problem": null
}
```

`ticket` is `null` for a workspace linked to none, and then nothing else but `read_at` and a null
`problem` is there. `tracker` is `github` or `jira`; `repository` is the GitHub repository the issue
is in, `null` for Jira; `state` is `open` or `closed` for GitHub and Jira's own status name for
Jira. When the ticket cannot be read (`gh` or `acli` missing, not signed in, no such ticket, a call
that failed), `problem` says why and with what to do, and the fields of the ticket are absent. All
the tracker's text is masked. `attachments` are the files of a Jira work item, `[{"name", "type",
"bytes", "author", "at"}]`, named only (Flottille never fetches one), empty for GitHub. Nothing is
kept: every call reads again.

#### `workspace.link_ticket`

`{"project", "name", "ticket"}`. Links the workspace to a ticket, or to none when `ticket` is
`null`, and answers as `workspace.ticket` does. `refused` when `ticket` is not `#123`, `123` or a
Jira key (`ABC-123`, any case). Emits `workspace.ticket` (`docs/events.md`).

#### `workspace.rename_branch`

`{"project", "name", "branch", "repository"?}`. Renames the workspace's branch to `branch` in the
repository named, or in each repository of the workspace, and answers the workspace as
`workspace.show` does without the git reads, with `"renamed": ["<repository>", ...]`
(`docs/workspaces.md`, "Renaming the branch"). All or none: it is `refused`, nothing renamed, when
the workspace is being prepared; when `branch` is not a name `git check-ref-format --branch`
accepts, is the branch a worktree is on already, exists in a repository or is checked out in a
checkout of one; when a worktree is on no branch; when a repository's branch was resumed by the
workspace, has a remote branch (the tracking ref, or what the remote lists) or has a pull request.
Each refusal says which repository and why. Emits `workspace.changed` (`docs/events.md`) when
every worktree made is on the new branch, and the record follows; renaming one repository of
several leaves the record as it is.

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
"Removal"). `refused`, with nothing touched, while it is being prepared or removed, while an agent
not ended or a command run works in it (each named), or when a worktree holds a
changed or untracked file or a commit on no remote, unless the forge says the branch's pull
request merged it: the message names every one, and the branch that holds the commit.

### The items methods

Every pull request and ticket of a project, as the daemon last read them ([`items.md`](items.md)).
An item is `{project, kind, source, repository, key, title, state, author, assignees, labels, url,
updated_at, seen_at, mine, details, checks: [{name, state, started_at, ended_at, link}]}`; a
source is `{kind, target, source, cursor, round_at, failures, problem, note}`, `note` saying what
its last round could not read (a list cut at its cap, `mine` unknown). `not-found` when there is
no such project.

#### `items.list`

`{"project", "kind"?, "state"?, "repository"?, "mine"?}`: `kind` is `pull` or `ticket`, `state`
one of an item's states (`open`, `draft`, `merged`, `closed`), `repository` a path of the
project's (a Jira work item has none), `mine` true for the user's own. Answers `{project, items,
sources}`, the items most recently changed first, from the store: nothing is read. Each source
has its `mine_unknown`: why its last round could not tell which items are the user's, `null`
when it could.

#### `items.show`

`{"project", "kind", "id", "repository"?}`: the stored item whose key is `id` (`12`, `#12`,
`ABC-1`); `not-found` when none is stored, `refused` when several repositories have one and
`repository` does not say which.

#### `items.refresh`

`{"project"}`: a round of every source of the project now, whatever its rhythm, after the one
running when there is one; answers `{project, sources}` once it ended.

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
`{"name", "line", "repository", "serve", "check", "read_only", "ask_before_running", "ready",
"url", "needs"}`, `repository` `null` for the place's top, `needs` the services it needs (none: `[]`), `ready` `null` or `{"line"}`, `{"url"}` or
`{"command"}`, `url` `null`, an address or `{"command"}` (`docs/projects.md`, "Tasks"). Each write is checked as the file is, and written under
`[tasks.<name>]` where it stands, what is around it kept, a flag that is off left out.
`not-found` for a project or a task that does not exist, `refused` for a task the file would
refuse, `invalid-config` when the project's file cannot be used.

#### `task.list`

`{"project"}`: `{"project", "tasks": [...]}`, by name.

#### `task.show`

`{"project", "name"}`: the task.

#### `task.add`

`{"project", "name", "line", "repository"?, "serve"?, "check"?, "read_only"?,
"ask_before_running"?, "ready"?, "url"?, "needs"?}`: the task. `refused` when the project has one of that
name, or `needs` names a service `services.toml` does not hold.

#### `task.edit`

`{"project", "name", "line"?, "repository"?, "serve"?, "check"?, "read_only"?,
"ask_before_running"?, "ready"?, "url"?, "needs"?}`: what is given replaces what the task had
(`repository: null` puts it back at the top; `ready` or `url` `null` or `""` removes it). Answers
the task.

#### `task.copy`

`{"project", "name", "to"}`: writes the task in the project `to`, as `task.add` writes it (same
name, line and flags), and answers it with `"project"` the target. Its `repository` is kept when
`to` has a repository of that path, else it runs at the top of the place. `refused` when `to` has
a task of that name, when `to` is `project`, or while `to` is being removed; `not-found` for a
project or a task that does not exist.

#### `task.remove`

`{"project", "name"}`: `{"project", "removed": "<name>"}`. `refused` while a setup step runs it.

#### `task.run`

`{"project", "name", "workspace"?}`: `{"project", "name", "run"}`, at once. The user runs the task
in the project folder with the project's variables, or with `workspace` in that workspace with its
own (a session's rail runs it in the session's workspace), as a run of no agent: no gate, no
delivery (`docs/tools.md`, "Tasks and runs"). A task that keeps running (`serve`) and already runs
for the user in the same place is joined: its run is answered. `refused` when the project has no
such workspace. It is followed with the run methods. The services the task
needs are started first and waited for (`docs/projects.md`, "Services"): `refused` naming the
service that failed, or was not ready in time, and why.

### The service methods

The services of the machine (`docs/projects.md`, "Services"), kept in `services.toml`. A service in
a result:

```json
{
  "name": "auth",
  "folder": "/home/me/work/auth-api",
  "line": "npm run start:dev",
  "needs": ["mongo"],
  "ready": {"url": "{url}"},
  "url": "http://localhost:3001",
  "ask_before_running": false,
  "state": "ready",
  "why": null,
  "run": "5d2c0f1a9b3e",
  "address": "http://localhost:3001",
  "needs_stopped": [],
  "needed_by": [{"project": "app", "tasks": ["api"]}]
}
```

`folder` is where it runs, absolute (the home when the file names none); `ready` and `url` are
as the file says them. `state` is `stopped`, `starting`, `ready`, `running` (it says no ready),
`outside` (it answered before Flottille started it) or `failed`, `why` then the line that says why.
`run` is its run, going on or the last one, read with the run methods' `run.output` too; a
service's run has an empty `project` and is not listed by `run.list`. `address` is its run's
address, or the file's. `needs_stopped` lists what it needs that does not run. `needed_by` lists
the projects whose tasks need it, directly or through another service's `needs`, each with those
tasks. `not-found` for a
service the file does not hold, `invalid-config` when the file cannot be used, `refused` for a
write the file would refuse (nothing is written).

#### `service.list`

`{}`: `{"services": [...]}`, by name.

#### `service.show`

`{"name"}`: the service.

#### `service.add`

`{"name", "line", "folder"?, "needs"?, "ready"?, "url"?, "ask_before_running"?}`: the service,
written under `[services.<name>]`, what is around it kept. `refused` when there is one of that
name. Emits `service.changed`.

#### `service.edit`

`{"name", "line"?, "folder"?, "needs"?, "ready"?, "url"?, "ask_before_running"?}`: what is given
replaces what the service had (`folder`, `ready` or `url` `null` or `""` removes it). Answers the
service. Emits `service.changed`.

#### `service.remove`

`{"name"}`: `{"removed": "<name>"}`. `refused` while another service or a project's task needs
it, naming them. Emits `service.changed`.

#### `service.start`

`{"name"}`: `{"service": {...}, "started": [names]}`, once it started: what it needs first, each
waited for until it is ready, then the service itself, not waited for. A service that runs is not
started again (`started` is empty), nor one that answers already (its state is then `outside`).
`refused` naming the service that failed and why. Emits `service.started` for each it starts.

#### `service.stop`

`{"name"}`: `{"service": {...}}`, once it ended: its whole process group, as `run.stop` does.
What it needs and what needs it go on. `refused` when it is not running, or runs outside
Flottille, which does not stop it.

#### `service.output`

`{"name", "tail"?}`: `{"service", "run", "output", "earlier"}`, the last `tail` lines (200 by
default, 1 to 1000) of what its run wrote, masked, `earlier` whether lines before them were left
out; `run` and an empty `output` when it never ran.

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
  "led": false,
  "led_by": null,
  "detached_from": null,
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
released; its `state_reason` says why its last turn failed when that left its session usable,
`docs/agents.md`, "A failed turn"), or one of the final states: `ended`, `failed` (`state_reason`
says why). A final state is never left, but for an ended or failed root the user resumes
(`agent.resume`).
`provider` is the provider it runs on, for life (`docs/agents.md`, "Providers"). `chosen` is what
was asked, or the project's default model, `null` for the agent's default; `taken` what the agent reported
at the last start of its process; `reopens` whether its agent offered to resume or load a session
when it last opened one, `false` once Flottille forgot an ended root's session; `images` whether
it took images in a prompt when it last started, `null` before it did. `active_at` is its last sign of activity, `null` before the
first. `exchanged_at` is its last exchange: the later of the last message the user gave it (the
moment a prompt took it) and the end of its last turn, `null` before either; unlike `active_at`,
what the agent does during a turn never moves it, which is what a list of sessions sorts by. `instructions` are frozen when the agent is created, `tools` are its role's and change with
it (`docs/tools.md`, "When the tools change"); `tools_outdated` says whether its running session
runs an older list than that. `parent` is the agent that
launched it (`null` for a root), `depth` its depth in the tree, `epoch` the number of its session
(each replacement adds one), `title` and `brief` a child's task, or for a root the title its
agent or the user named its session with (`docs/tools.md`, "The session's title"), `titled_by`
who did (`agent` or `user`; `null` for a child and a session not named yet, which keeps the first
line of its first message), `launched_by` the id of the agent
that started a root for the user (`null` for a root the user started, and for a child), whose
`brief` is then that agent's first message, never the user's, `led` whether that agent leads it
(`led_by` its id, `null` for a session handed to the user, `docs/tools.md`, "Sessions an agent
starts"), `detached_from` the agent the user detached it from (`agent.detach`; `launched_by` is
then `null`), `reported_at` when it reported,
`quiet_turns` its turns ended without a report in a row, `waiting_on_answer` whether a question of
it waits for an answer: a child's from its parent, a led session's from its pilot, any other
root's from the user, until that one writes to
it (`docs/agents.md`, "The tree"). `gate_mode` is its session's mode, `ask`, `auto` or `full`
(`agent.mode`).

Each method answers `not-found` when the agent does not exist.

#### `agent.start`

`{"project", "workspace"?, "provider"?, "model"?, "effort"?, "mode"?, "gate_mode"?, "role"?, "message", "images"?}`.
Creates a root agent in the project folder or the workspace, and gives it its first message:
`{"agent", "after"}`. `gate_mode` is the session's mode (`ask`, `auto` or `full`, `docs/design.md`
§7.1), `auto` when left out: it is the root's before its first message is given, so that the
first turn's calls already follow it. Only the user's door names it: `agent.start` is answered on
the user's socket alone (a client an agent started is refused), and no tool of an agent, nor the
pilot door's `agent_start`, takes a mode. `invalid-params` for another mode, and nothing is
started. `images` is as in `agent.send`, and goes with the first message; with
images, the message may be empty (pictures alone are a first message). They are judged and stored
before the agent exists, so that one refused, or one that cannot be stored, starts none, and a
session replaced is given them again with its first message. A provider whose last agent that
started said it takes no images (`images: false` in `provider.list`) is refused them (`refused`,
"the agent of `<provider>` takes no images"); while none of its agents has said, they go with the
message, and an agent that takes none says so in the text it keeps ("`[n images not sent: this
agent takes no images]`"). `role` is `lead` (the default) or `worker`. `provider` is the project's, else the first
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

`{"agent", "message", "images"?, "compact"?}`. Gives the agent a message (a `user` delivery), played after the turn that
runs if one does, starting its process again when it was released: `{"agent", "after",
"delivery"}`. `delivery` is the id of the delivery stored for the message: its `delivery.queued`
event (numbered after `after`), then `delivery.sent` when a prompt takes it (`docs/events.md`), name
it, so that a client follows the turn of its own message even when another client writes to the
agent at the same time.
`refused` when the agent is in a final state, the message is empty, or the agent is a child: only
its parent writes to it, and the sentence names its root.

`compact` is the composer's **Compact** chip (compact before sending): when the agent's prompt cache expired
(`agent.cache_expired`), `true` compacts it before the turn the message starts and `false` sends it
to the session as it is, whatever the setting `after_expiry`; left out, the setting decides.

`images` is a list of `{"mime", "data"}`, `data` the image's bytes in base64: a pasted or dropped
picture, sent as ACP image blocks after the message's text. With images, the message may be
empty. `mime` is `image/png`, `image/jpeg`, `image/gif` or `image/webp`; a message carries 20
images at most, 5 MiB each (20 is the least of what the providers document: the Claude API takes
100 or more images per request, the OpenAI API 1500, and OpenCode leaves the count to its
provider; the Claude API also asks every image of a request of more than 20 to be 2000 px at most
on its long side). A PNG or a JPEG whose long side is over 2000 px is scaled down to 2000 px
before it is written, keeping its proportions and its format; a GIF and a WebP (they may be
animated) are kept as they came, and so is a file that cannot be read as the picture its `mime`
says. The daemon writes them in `$XDG_DATA_HOME/flottille/images/<agent>/`
(never the project's), and the `agent.prompted` event of the turn that sends them names them.
`refused` for an image of another type, bad base64, an empty or too big image, too many, and
when the agent's agent said, when it last started, that it takes no images (`images` of the
agent, below); before it started once, `images` is `null` and the message is accepted: if the
agent turns out to take no images, none is sent and the text of the turn ends with
`[N images not sent: this agent takes no images]`. A file gone when the turn starts leaves
`[image <name> is missing]` in the text. Merged queued messages make one prompt of 20 images and
10 MiB at most (the first message is always whole); the others wait for the next turn. The
images of an agent go with it: when it ends for good, or 7 days after a root that can be resumed.

#### `agent.answer`

`{"agent", "question", "answers"}`. The user's answers to an ask of the root (`docs/tools.md`,
"ask"): `question` is the number of its `agent.asked` event (`questions` of `agent.tree`), and
`answers` one per question, in order, each `{"choices"?, "text"?}`: the labels of the options
chosen (one at most when the question is not `multiple`) and the user's own words (several lines
allowed), at least one of the two. The daemon sends the root one message (a `user` delivery, as
`agent.send` does, its id `answer-<question>-…`) that quotes each question and gives its answer
under it (the options chosen on one line, the words after); a single question without options is
answered by the words alone, as a message always answered it. It answers that ask alone: the
root's other asks still wait. The answers are kept with the ask (`agent.answered`, `asked` and
`answers`), and the window shows them on the ask's card instead of a message of the user. It
answers as `agent.send`: `{"agent", "after", "delivery"}`. `refused` when the ask does not wait for
the user's answer (answered, replaced, never asked of the user), when an answer to it waits or was
taken already (one withdrawn with `agent.unqueue` does not count, the ask may be answered again), when
the answers do not fit its questions (`1 answers for 3 questions: answer each`, `question 2 has no
answer`, `question 1 has no option Maybe`, `question 1 takes one option`), for a child, or for an
agent in a final state.

#### `agent.unqueue`

`{"agent", "delivery", "images"?}`. Takes back a message the user wrote to a root that still waits
for the root to read it (`queued` of `agent.tree`): `{"agent", "delivery", "text", "images"}`. The
root never reads it, and `delivery.withdrawn` is its event (`docs/events.md`). `text` is the
message as it is stored (masked), as the user typed it for one that used commands; `images` are its pictures, `{"mime", "data"}` as `agent.send`
takes them, when the request says `"images": true`, else empty (the picture files are deleted
either way). The check and the withdrawal are one transaction: `refused` with "already read by
the agent" when the root took the message first (nothing changes), "that message was withdrawn
already", or for a child; `not-found` when no such message waits for the agent. Only the user's
messages (`kind` `user`) can be taken back.

#### `agent.send_now`

`{"agent", "delivery"}`. Hands the root a message that waits, at once: `{"agent", "delivery"}`.
The message becomes a `redirect` (`docs/agents.md`, "Deliveries"): the turn that runs is cancelled
(an ACP cancel, the process lives), and the next prompt is this message alone, ahead of the other
waiting messages, which stay queued in their order. Refused and not found as `agent.unqueue` is.

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

`{"agent"}`. Starts an ended or failed root again on its own session: `{"agent"}`, the root once idle again
(`agent.resumed`, `docs/events.md`). It keeps its id, its epoch, its conversation and its session;
nothing is sent to it, and the next message (`agent.send`) starts its process, which resumes or
loads the session as after an idle release (`docs/agents.md`, "How an agent runs"). Its children
stay ended. Its next prompt starts with a
`resume` delivery saying the user stopped it, or that it failed and why, and naming its requests
cancelled then. `refused` for a child (the sentence names its root), an agent that has not ended,
one whose agent can neither resume nor load a session, that never opened one or
whose session was forgotten (7 days after it ended), one whose workspace or project is gone: what
`resumable` says, in short (`agent.tree`); and one whose last process is still stopping
(`stopping`), with a sentence to try again in a moment.

#### `agent.reload_tools`

`{"agent"}`. Reloads a root's tools (`docs/tools.md`, "When the tools change"): `{"agent",
"after_turn"}`. Its process is let go between turns, as after an idle release
(`agent.released`, reason `reload`), and the next message starts the next one, which resumes or
loads its session with the tools its role has now. A turn that runs is never cut: the process goes
once it ended (`after_turn` true), the prompts waiting going to the next process. The next
process's prompt carries a `tools` delivery: what changed since the last list the agent was given,
by tool, else `Your tools were reloaded: nothing changed.`, whatever its agent does with
`list_changed`. Until that process takes it, the reload waits: `tools_reload` of the agent
(`agent.show`, `agent.tree`) is true, and `agent.tools_reload` says when it is asked and when it
is taken (`docs/events.md`). Its children keep their processes. `refused` for a child (the
sentence names its root) and an agent in a final state.

#### `agent.compact`

`{"agent"}`. Compacts the agent's conversation (`docs/design.md` §5.8): the compactor summarises
it and the agent goes on in a new session from its brief, the summary and its latest messages.
`{"agent", "after_turn"}`: it starts at once between turns, or once the turn that runs ended
(`after_turn` true); asked again before it starts, nothing more happens; asked while the
compactor works, `refused` (a compaction runs). Its prompts wait while the compactor works, and a
turn that started before is never cut: the new session follows its end. A compaction by hand
needs no threshold, no margin and no notice seen. `agent.compacted` says it is done, `agent.compaction_failed` why it failed, in
which case nothing changed. `refused` when there is no compactor (none chosen, or none could be
picked), and for an agent in a final state.

#### `agent.rename`

`{"agent", "title"}`. The user names a session (`docs/tools.md`, "The session's title"): the title
replaces the first line of its first message and any its agent gave, and from then on the agent's
`session_title` is refused; the user may rename it again. The title is trimmed and masked of
secrets; an empty one or one on several lines is refused (`invalid-params`), and so is a child,
whose title is its `agent_launch` title. `agent.titled` is emitted with `by` `user`. Answers
`{"agent", "title", "by": "user"}`.

#### `agent.detach`

`{"agent"}`. Takes a session an agent started (`launched_by`, led or handed) away from that agent,
at once (`docs/tools.md`, "Detach"): the agent's `session` subscription ends and it is told once
(a `detached` delivery, `the user detached the session "<title>" …`); `launched_by` and `led_by`
become `null` and `detached_from` keeps that agent's id; its questions go to the user; the two
need the user's leave to write to each other; the chain rule no longer holds; the session is told
by a `detached` delivery that it is the user's alone. Its first message stays the agent's words.
`agent.detached` is emitted. Answers `{"agent": {...}}`. Refused, saying why, for an agent no
agent started (a root of the user's, a child) and for one already detached. There is no undo.

#### `agent.mode`

`{"agent", "mode"}`, `mode` `ask`, `auto` or `full`. Puts the agent's tree in that mode
(`docs/design.md` §7.1): how the gate answers the calls it holds from then on; in `full` every
call to judge and every base call passes without a request, and what the gate refuses stays
refused. A tree has one mode, its root's: a
child named switches its root, and every agent of the tree reads the root's mode at each call
(an agent's `gate_mode` is its root's). A new session starts in `auto` unless the user's `agent.start`
names its `gate_mode`, and a session an agent starts takes the mode of the tree that started it
(no tool of an agent names one); the mode is kept when it resumes. Answers `{"agent"}`, the root, with its `gate_mode`, and emits
`agent.gate_mode_changed` for the root (`docs/events.md`).
`invalid-params` for another mode, `not-found` for an agent that does not exist. The user's door
alone reaches it: no tool of an agent changes a mode.

#### `agent.quota_resume`

`{"agent", "resume"}`: whether this session, which waits for its quota's reset, is resumed at it
(`docs/design.md` §5.9), whatever the setting says. Answers `{"quota"}` as `agent.show` says it,
and emits `agent.quota`. Refused for a session that does not wait and for a child.

#### `agent.options`

`{"agent", "model"?, "effort"?}`, at least one of the two. Changes the model and the effort of a
running agent between turns (`docs/agents.md`, "The options"); its provider never changes. The
change is checked as an agent's start is, against the models its provider is known to offer, then
held as the agent's `pending` options and set on its session before its next prompt: at once when
it is between turns, once its turn ends otherwise. Answers `{"agent", "after", "dropped"?}`, the
agent with its `pending` options, the event number to follow the change from, and, when a model
change made a pending effort the new model does not offer go, a sentence that says so; the events
are `agent.options_pending`, then `agent.options_taken` or `agent.options_refused`
(`docs/events.md`).
`refused` for an agent that has ended or failed, a model its provider is known not to offer, and an
effort its model is known not to offer, each naming what is offered; `invalid-params` when neither
is named; `not-found` for an agent that does not exist. A child can be changed as well: its parent
does it with `agent_send` (`docs/tools.md`).

#### `agent.tree`

`{"agent"}`. The whole tree of the agent's root, each agent before its children:
`{"agents": [{"agent", "doing", "waiting_on", "needs_user", "questions", "context", "context_tokens", "queued", "extensions"}]}`. `agent` is the agent in short
(`id`, `provider`, `role`, `title`, `project`, `workspace`, `parent`, `launched_by`, `led_by`, `detached_from`, `depth`, `epoch`, `state`,
`state_reason`, `created_at`, `active_at`, `exchanged_at`, `ended_at`, `waiting_on_answer`, `model`: the model it
took at the last start of its process, else the one asked, `null` for the agent's default before it
started; `effort`: the same for its effort; `pending`: `{"model", "effort"}` the user changed to and
the session has not taken yet, `null` when nothing waits; `resumable`: whether `agent.resume` takes it once its last process is gone; `stopping`:
whether the process of an agent that ended is still stopping; `images`: as in the agent's full
form; `tools_outdated`: whether its running session runs an older list of tools than its role
has, which a restart replaces, `docs/tools.md`, "When the tools change"; `tools_reload`: whether
a reload of its tools waits for its next process, `agent.reload_tools`; `compacting`: whether Flottille's compaction of it runs, the compactor at work or the agent writing its handoff, from `agent.compacting` to `agent.compacted` or `agent.compaction_failed`; `quota`: `null`, or while the root waits for its quota's reset `{"reset_at", "resume", "limited_at"}`, `resume` whether it will be resumed then, `docs/design.md` §5.9); `doing` a sentence of what it does
(`working`, `idle`, `compacting the conversation`, `running \`cargo test\` for 12 min`, `waiting for approval #3`, `failed: …`);
`waiting_on` `parent` for a child whose request or question waits for its parent, `pilot` for a
led session whose question waits for its pilot, `user` for a root whose own does, `subscription` for an agent between turns that has an open subscription (`doing` then
says `waiting for #12: checks`, `#12 of api` when the project has several repositories, `waiting for its wake in 20 min`), else `null`; `needs_user`, for the root, whether a request of its tree or its
own question waits for the user; `questions`, for a root, its questions to the user that wait for
the answer, the oldest first, each `{"seq", "at", "question", "questions"}` (the number of its
`agent.asked` event, when it was asked, the ask as one text and each of its questions with their
options, masked, as `agent.asked` has them): each asked while the root waits that no message of
the user written after it, and taken by the root, answered (an answer from an ask's card,
`agent.answer`, answers that ask alone), and that no replacement or resume of its session came
after (a message written before a question does not answer it), empty once it ended; `context`, how much of its context the agent used at its last
report (`agent.usage`), in percent, `null` before one and while the window's size is not known;
`context_tokens`, the tokens its context holds at that report, `null` before one; `queued`, for the root, the messages the
user wrote to it that wait for it to read them between its turns (an answer to an ask, which waits
on the ask's card, is not one of them), the oldest first, each
`{"delivery", "text", "images", "pictures", "at"}` (its text masked as it is stored, how many pictures go with
it and their names, which `agent.image` reads back, when it was written), empty for a child. For a
message that used commands (`docs/commands.md`), `text` is what the user typed, `commands` the
names of the commands, in the order of their first use, and `received` what the agent will receive
for it. `extensions` is what the extensions active in its project say of it, each under its name
(`docs/extensions.md`, "Data"), `{}` when none does: Specs' is `{"spec", "title", "status",
"work"}`, the spec a session it started writes (`work` `writing`) or develops (`developing`).

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
"line", "doing", "waiting_on", "needs_user", "questions", "messages"}`. `agent` is the agent in
short as in `agent.tree`, `line` a sentence of what it is and does with its last sign of activity
and its context use, `doing`, `waiting_on`, `needs_user` and `questions` as in `agent.tree`, and `messages` its last
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
  "ready": "starting",
  "url": "https://login.app.localhost",
  "url_error": null,
  "last_line": "listening on :3000"
}
```

`name` is the task it runs, `null` for a line of the agent's. `agent` and `root` are `null` for a
task the user ran; `project` is always the project's; `workspace` is its agent's, or for a task the
user ran, the one it runs in (`null` in the project folder). `state` is `running`,
`exited`, `stopped` or `interrupted` (`docs/events.md`, "The events of a run"). `ready` is
`null` for a run of a task that says no ready, else `starting`, `ready` or `missed` (it ended before
it was ready); `url` its address, `url_error` why it has none when its task's line for it failed
(`docs/projects.md`, "Tasks"). `last_line` is the
last line of its output with something on it, masked of secrets and as the program wrote it
(colours included), cut at 300 characters. `tail` is the last 20 lines
of its output (a list of texts, masked and as written, each cut at 300 characters), which a result
card in the conversation shows folded, a running command's as well as one that ended.

#### `run.list`

`{"project"?, "tree"?, "ended"?, "user"?}`. `{"commands": [...]}`: the commands running, oldest
first. With `project`, the project's, the user's included; with `tree`, an agent's id, those of the
agents of its root's tree and the tasks the user ran in the root's place (its workspace, or the
project folder); without either, every one. With `user` (and `project`, required), only
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

#### `run.restart`

`{"run"}`. `{"run": {...}, "notice"}`, the new run: stops the command if it runs (as `run.stop` does,
and waits for its end), then starts the same line again in the same place (the same project,
workspace, repository and folder, the same task if it was one, with the environment of a command
started the same way, no shell) and answers with the new run, which keeps who started the first: a
command of an agent stays that agent's, and the agent reads it with `run_status` and is woken when it
ends; a task the user ran stays the user's. The agent is told (`command-ended`, `docs/tools.md`)
that the user restarted it and what the new run's id is, each time a restart happens, also when the
agent was already told that the first run ended; if the command was stopped and the new run could
not start, it is told that too and why. It is the user's act, so the gate is not asked, and a
restart never carries a push approval: the new run starts unlifted, as a command of the user's does,
whatever the first one was let to do. A command that already ended is started again too. A command of
an agent that has ended (or is gone) restarts as a run of the user's, with no agent, and `notice`
says so; otherwise `notice` is `null`. `not-found` when there is no such run; `refused` when its
line cannot be started again (the task is gone, or the line holds a secret its stored form masks),
or when the run is already being restarted (`run R is being restarted`): the command is then left as
it was. A start that fails after the stop (the program cannot be spawned) is refused too, but then
the command is stopped; its agent is told so. Emits `run.ended` for the first, if it ran, then `run.started` (`docs/events.md`).

#### `run.dismiss`

`{"run"}`. `{"run"}`: a command that ended is no longer listed with `ended`, and `run.dismissed` is
emitted. `refused` while it runs; `not-found` when there is no such run.

### The subscription methods

What an agent waits for (`docs/design.md` §5.6), as the window's rail shows it and lets the user end
it. The agent's own way is the `forge` and `tickets` tools (`docs/tools.md`).

#### `subscription.list`

`{"tree"?}`. `{"subscriptions": [...]}`, the open ones, the oldest first. With `tree`, an agent's id,
those of the agents of its root's tree (`not-found` when it names no agent); without, every one. Each
is `{"id", "agent", "source", "named", "target", "events", "from", "created_at", "expires_at", "paused"}`: `named`
says what it watches (`pull request #12 of api, workspace login`, a repository that is the project's
folder named by the project; `ticket #5 of api`, `ticket ABC-1`; `wake: <why>`), `target` is the
source's own (`forge`: `project`, `workspace`, `repository`, `number`; `tickets`: `project`,
`repository`, the tracker's, empty for Jira, and `key`, `5` or `ABC-1`; `timer`: `why`,
`expires_at` being when it wakes; `session`: `session` and `title`, the session followed, with no
end of its own), `from` is `anyone` or `people`, the times are in milliseconds since the Unix
epoch. A subscription with no deadline (one to a pull request or a ticket the agent gave no
`minutes`, a following) has `expires_at` 9223372036854775807 (`i64::MAX`): it lasts until its
target ends. `paused` is true for a following paused because its two sessions wrote to each other
without the user (`docs/tools.md`, "Sessions that talk to each other"), false otherwise.

#### `subscription.cancel`

`{"subscription"}`. `{"subscription": {...}}`: the subscription ends as the agent's `forge unsubscribe`
(or `tickets unsubscribe`) ends it, without a word to the agent, and `subscription.ended` is emitted with `why` `cancelled`.
`not-found` when it is not open.

#### `subscription.resume`

`{"subscription"}`. `{"subscription": {...}}`: a paused following resumes as a message of the
user's to either session resumes it: its count starts again, its wakes and the `session_send`
between the two come back, and `session.following_resumed` is emitted. The user's alone: no tool
of an agent does it. `not-found` when it is no open following; `invalid-params` when it is not
paused.

### The leave methods

The user's leaves for a session to write to another it did not start (`docs/tools.md`, "Sessions
that talk to each other"), as the window lists them where a session's rights are shown and lets the
user take one back. A leave is given only by allowing the first message of the pair
(`approval.approve`).

#### `leave.list`

`{"session"}`. `{"leaves": [...]}`, the leaves that name that agent as the writer or as the session
written to, the oldest first (`not-found` when it names no agent). Each is `{"writer", "session",
"request", "granted_at"}`: `writer` and `session` are `{"id", "title", "state"}`, `request` the
number of the request the user allowed it in, `granted_at` in milliseconds since the Unix epoch.

#### `leave.withdraw`

`{"writer", "session"}`. `{"leave": {...}}`: the writer's next message to that session asks the
user's leave again, and `leave.withdrawn` is emitted. `not-found` when there is no such leave.

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
      "models_outdated": false,
      "models_learning": false,
      "images": true
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
`images` says whether its agents take images in a prompt, as the last of them that started said
(`null` before one did): the window's composer says "takes no images" for a `false` before a start.
`login.api_key` is the name of the API key found set, never its value. `override` is the test
program standing for the provider (`docs/agents.md`, "For tests and development"). `models` is
`null` until the daemon read them from the provider's agent (as soon as it is enabled and ready, with
no prompt) or a session said them, and `efforts` of a model `null` until a
session had that model (Claude's say every model's: `[]` for one without); `name` is what the provider shows the model by; `default` is there, `true`,
on the model a new session takes when none is asked, once one said it: what the provider's
`default` stands for, else its first model listed, the model Flottille sets (`docs/agents.md`,
"The options"). A provider's own `default`
value that names the model it stands for (`_meta.jetbrains.air.recommendedValue` of the `model`
option) is an alias: it stays listed with `stands_for`, that model's id, and the model it stands
for is the `default` one, with the alias's `efforts`. A `default` whose `recommendedValue` is
itself runs a model no other value does: it is no alias, and is the `default` one, by its name (the
model its description names, never the adapter's "Default (recommended)"; the same when the
description names a model and no `recommendedValue` is given). `default_effort` is the effort a model runs
with when none is asked, once a new session none was set on said it by its name (Claude says it
as `default` only: never there for Claude). `models_outdated` is `true`
when they were said by another version of the program or with other API keys set: they are listed
as they were, but refuse no model and no effort until a session says them again.
`models_learning` is `true` while the daemon lists the provider's models
(`provider.models_learning`). `project` is there only when one is named; its `provider` is `null` when
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
(`agent.asked` to the user, `text` the ask as one text, its questions numbered, and `questions`
each of them with its options, as `agent.asked` has them), `failure` (`agent.failed`, `text` its reason) or
`paused` (`session.following_paused`: two sessions talk to each other without the user, `text` says
which and how to resume; `agent` is the followed session) or `vault` (`vault.asked`, `text` the
entry, its project and the agent's reason) or `unchecked` (`run.agent_folders_unchecked`: a run of
the agent, a child's too, ended in a place too large for the daemon to check all its agent folders
after it, `text` says which and where) or `put_back` (`run.agent_folders_put_back`: the put-back
of a run's place, a child's too, put back or could not put back what was written in its agent
folders, `text` says which and where they are kept). The daemon's own `unchecked` and `put_back`
come with `agent` and `workspace` `null` when no agent of the store ran it. A pause that resumed since is `"resolved": true`, its
`text` ending `It has resumed since.`; a request for a vault entry the user answered is
`"resolved": true` too.
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
