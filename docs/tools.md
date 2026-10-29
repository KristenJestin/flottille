# Tools

The tools of the agents Flottille runs, and the MCP server that serves them: the agent door of
`docs/design.md` §1. It follows `docs/reference/hemera.md` §3, with the rules of
`docs/reference/mcp.md` §4 where they differ. A session lists the tools of its agent, which
depend on what it can do (`docs/agents.md`): the file and command tools below for every agent,
`skill_read`, then `forge_read` and `forge_write` for every agent whose project's repositories
declare a forge, `ticket_read` for every agent whose project declares a tracker, in `main` as in a
workspace, and `workspace_remove` for every agent (a tool that could only refuse is not listed;
what each may do is the gate's business, not the list's),
`skills`, `commands` and `project` for a root (below, "The project"),
and the tree's tools (`agent_*`, `report`, `ask_user` or `ask_parent`,
`approval_answer`).

## The server

- **Streamable HTTP** on `127.0.0.1`, on a port the system picks when the daemon starts, at
  `http://127.0.0.1:<port>/mcp` (any path is served). Built on `rmcp` 3.x. The body of a request
  is at most 16 MiB.
- **Both protocol revisions**: 2025-11-25, which opens with `initialize`, and 2026-07-28, which
  is stateless and answers `server/discover`. Neither gets a protocol session (no
  `Mcp-Session-Id`): the token is the identity, and state across calls is in Flottille's own
  handles (run ids, cursors, offsets).
- **Before anything else**, against DNS rebinding: the `Host` must be `127.0.0.1:<port>` or
  `localhost:<port>`, and an `Origin`, when there is one, `http://` with one of them; anything
  else gets 403.
- **Tokens**: each start of an agent's process mints a token for it (32 random bytes,
  base64url), handed to the agent in the `Authorization: Bearer <token>` header of the
  session's MCP server. Only its SHA-256 digest is kept, in memory. Minting a token revokes the
  agent's previous one, and the process's end revokes it. The token of a session that was
  replaced (`docs/agents.md`) is kept instead, so that its late calls are told why: every one is
  refused with `refused: this session has been replaced: a new session of this agent goes on with
  its work; stop here`. A request without a known token gets
  401 with `{"error":"unauthorized"}`; the daemon's log says when a revoked token was used and
  whose it was, never the token, at most 20 lines a minute.
- **Per request**: the server that answers holds the grant of the request's token, so that it
  lists and calls the tools of that agent only, in the order frozen in the agent
  (`docs/agents.md`), and works in that agent's place. The list is fixed (`listChanged: false`):
  it is part of the prompt cache.
- **Instructions**: the `initialize` result (and `server/discover`'s) carries
  `crates/flottille/roles/guide.md`, one text for every agent, under 400 words: what Flottille is
  and that the agent runs in bare mode with a tool list fixed for the session; what a project,
  `main` and a workspace are; what the skills, the tasks, the commands and the rules are
  and which tool reads each; that the gate decides every call and a held call's result comes later
  as a delivery; what deliveries are and what wakes the agent. The names it uses for the
  project's things (skills, tasks, commands) are in that file alone, and a test
  checks that every tool it names exists.
- **Every tool listed** has a `title`, a description (what it does, when to use it, its effects,
  when it refuses), `inputSchema`, annotations (`readOnlyHint`, `idempotentHint`,
  `destructiveHint`, `openWorldHint: false`) and `_meta["anthropic/alwaysLoad"]: true`: in bare
  mode these tools are all the agent has, so they load upfront. The bridge also sets `alwaysLoad: true` on the server's entry of the `--mcp-config` it gives `claude` (`docs/reference/mcp.md` §4 rule 7).
- **A call's key**, `_meta["claudecode/toolUseId"]` (Claude Code sends it with every call): a
  call is played once for its key, and a retry with the same key waits for the first one and gets
  its answer. Without a key there is no deduplication, which the log says once per agent. The last
  1024 keys are remembered.
- **A result** is one text block, `isError` for a refusal or a failure. A normal result stays
  under 40,000 characters (`docs/reference/mcp.md` §4 rules 5 and 8: an agent in bare mode cannot
  read a result Claude Code saved to a file), a refusal or a failure under 2,000. A failure reads
  `the call failed: …`. A result and a failure are masked of the daemon's and the place's secrets
  (a file read or a search hit shows `[masked]` for them). A call the gate held is answered with
  the waiting text, not as an error (below). Every result, refusal and failure is escaped as a
  delivery's body is (`docs/agents.md`, "Deliveries"): what a file, an output or another agent
  holds never forges a `[flottille:` marker nor opens or closes a note. The notes pinned for the
  agent (the deliveries that reach it in a turn) are appended after a blank line, whole, in the
  room the text leaves under its limit; the text is never cut for them, and a note that does not
  fit stays pinned for the next result.
- **Every call is recorded** as an `agent.tool_served` event of the agent (`docs/events.md`):
  the tool, the key, the arguments with every text cut at 500 characters and masked, the outcome
  and the duration.

## The place and its paths

The agent's place is its folder: a workspace, or the project folder (`main`).

**Every call is decided once**, by the gate (below), after the tool is known to be the agent's
and before it is performed: `run_status` as much as the file tools. A
path outside the place is asked for; the tools check again that a path they resolve is inside,
and refuse it with `refused: X is outside this agent's place: …`, unless the user allowed the call.
A path is resolved this way:

- relative to the place's top, or to a repository's folder when the call names `repository`
  (one of the project's repositories by its path, else
  `refused: the project has no repository X: it has …`); `~` is the home folder;
- then the real path, on the disk, in the order the kernel takes it: every link is followed (at
  most 40, as Linux does), and each `..` steps up from where the links before it led, so
  `src/cfg/../x` is beside where `src/cfg` leads, never `src/x` when `src/cfg` is a link. A
  missing file is its real folder and its name, and a dangling link leads to its target; a part
  the kernel would not find (missing, a name too long, a name below a file) is taken as written
  with what follows it. A `..` after such a part is refused by the file tools (`refused: X names
  a folder that does not exist and then goes up out of it: …`): the kernel would fail it too. A
  run's word holding one is often no path at all (`sed 's/a/../'`, a message, a URL), and makes
  the run to judge; but a program may make the missing folder first (`install -D`), so a `..`
  after a missing part cancels it and the word is followed on, on the disk, and checked where it
  then leads like any other (`install -D f m/../../.ssh/x` is base). After a part that can never
  be made (a name too long, below a file), nothing can be followed on the disk, but a program
  that takes `..` on the text (`git restore`, `python3 -m zipfile`) still reaches the path as
  written: the word is checked as written (`git restore -- <a long name>/../.env` is base).
  Each part is looked at in the folder opened before
  it, as the kernel walks, so a path whose links expand past the longest path the kernel takes at
  once is still followed. Below `/proc`, links are not followed: `/proc/self` is the reading
  program's own; a `..` below `/proc` goes where such a link leads (`/proc/net/..` is
  `/proc/self`), so it is taken as going through a process's folder. A link that leads out is
  outside, shown where it leads; a path that cannot be followed (more than 40 links, a folder
  that cannot be read) is outside, and base (below);
- containment is by segments: `..notes` is a name inside, `../x` a step out.

**What the gate guarantees to the file tools.** They open the real path the gate checked, and
nothing else: every open (`fs_read`, `fs_list`, `fs_write`, `fs_edit`, the freshness check before
a write, `search`) goes by descriptors, without following any link (`openat2` with
`RESOLVE_NO_SYMLINKS`, or, where the kernel lacks it, each folder opened in turn with
`O_NOFOLLOW`). The real path holds no link, so a link met on the way means a folder changed since
the check: the call is refused (`refused: X: the path changed since it was checked; nothing was
done`), and nothing of where it now leads is read, shown or made. `search` opens each file below
its top folder's descriptor, never out of it (`RESOLVE_BENEATH`); a write opens its folder (and
makes the missing ones, a folder at a time, each in the one opened before), then makes its
temporary file and renames it in that open folder. A path that goes through a process's folder in
`/proc` is never opened, whoever allowed it, and neither is one through `/proc/self` under another
name (`/proc/mounts`, `/proc/net/…`), which would be the daemon's own (`refused: X goes through …:
Flottille's file tools never open a path through a process's folder in /proc`); one the gate could
not follow for another reason, once the user allowed it, is opened as written, and refused if a
link stands on its way.

A run's program resolves its own paths: the gate's checks on the words of a run are a first
filter, not a guarantee of what the program will open.

Paths are shown relative to the place's top.

## Files

| Tool | Arguments | |
|---|---|---|
| `fs_read` | `path`, `repository?`, `range? {offset?, length? 1..32768}` | A page of a file, lines numbered `%6d\t`, then `(bytes a-b of N; the next page starts at offset M)` or `(…, the end of the file)`, then `{"offset", "end", "size", "next"}`. A page holds at most 32 KiB and ends after its last newline (else on a character); an offset inside a character starts at the next one. A page is cut shorter to keep the result under the limit (a file of empty lines). |
| `fs_list` | `path`, `repository?`, `depth? 1..5` | The entries, sorted, a folder's after it: `path  file  size`, `path/  folder`, `path  link  -> target`. `.git` is left out. It stops at 1000 entries, or shorter to stay under the limit, and says so. |
| `search` | `pattern`, `repository?`, `glob?`, `cursor?` | A case-insensitive substring, not a pattern. A sorted, depth-first walk that never enters `.git` or `node_modules` nor follows a link; the top `.gitignore` applies (simple patterns: no `!`, a pattern with a `/` is anchored, a trailing `/` is a folder). `glob` keeps to the files it matches (on the name alone without a `/`; `**` for any folders). A file with a NUL byte in its first 64 KiB is skipped and named (20 at most). Hits are `path:line: text`, trimmed and cut at 200 characters. It stops at 100 matches, 1 MiB read or the size limit, with a cursor `path:line` that resumes it. The head says the count, the stop and its cursor, the limits, the skipped files. |
| `fs_write` | `path`, `content`, `repository?`, `why?` | The whole file, made with its folders when new. |
| `fs_edit` | `path`, `edits [1..50] {old, new}`, `repository?`, `why?` | Each `old` must appear exactly once (overlapping occurrences count) when its turn comes, else `refused: the anchor of edit N appears K time(s) in X; it must appear exactly once. Nothing was changed`. Edits apply in order, spliced (never as a pattern: `$&` is text), all or none. |

**Only regular files are read.** `fs_read` and `fs_edit` refuse a pipe, a socket or a device
(`refused: X is not a regular file …`), which would hold the call forever; `search` skips them.
A file is looked at first (`O_PATH`), and opened to read only once it is known to be a regular
file, and then must be the same file: a device or a pipe is never opened. A folder that is only
gone through needs only the permission to search it, as for the kernel.

**Optimistic writes, per session.** Reading a file (any page) or writing it keeps its SHA-256 for
the agent, in the store (`session_files`), so it holds across restarts; the text of a file of
256 KiB or less is also kept in memory, for the last 32 files of each agent, to show a diff. Before
`fs_write` or `fs_edit`:

- never read, and the file exists: `refused: X exists and this session has not read it: read it
  with fs_read first`;
- changed since: `refused: X changed since this session read it; read it again before writing.
  What changed:` and the lines that differ (at most 20, each cut, masked), or a sentence saying
  the version read is no longer kept;
- removed since: `refused: X was removed since this session read it; nothing was written`.

A write goes to a temporary file `.<name>.<id>.flottille` beside the file, with the file's mode,
renamed over it: a link is replaced, never written through. Its folder is the one the gate
checked, opened without following a link (above): a folder swapped for a link meanwhile is
refused, and nothing is made or written where it leads.

A path outside the place, which the judge allowed, is resolved again when the tool starts: when it
now leads where the gate would hold the call as base (a folder swapped for a link to Flottille's
own while the judge thought), it is refused (`refused: X: the path changed since it was allowed
(…); nothing was done`). From there on, no link is followed.

## Skills

| Tool | Arguments | |
|---|---|---|
| `skill_read` | `name`, `path?`, `range? {offset?, length?}` | One of the project's skills (`docs/skills.md`), read-only. A file or a text skill is read by its name alone, a page at a time exactly as `fs_read` does (32 KiB at most, `range` to go on). A folder skill answers with its files and their sizes (`.git`, links, files of secrets and sensitive places left out, 1000 at most); `path`, relative to the skill, then names the file to read, or a folder of it to list. |

- **The list is the only door**: a name not in the agent's list is refused with `refused: no
  skill named X: the project's skills are a, b`; the skill is looked up in the project's file as
  it is now.
- **Nothing outside the skill is served**, as `fs_read` serves nothing outside the place: a
  path that steps out of the skill with `..`, an absolute path, `~`, and a link that leads out
  once followed are refused with `refused: X is outside the skill Y: it serves its own files
  only`; a path through a `.git` or naming a file of secrets (`.env`, `.env.*`) is refused; a
  `path` for a file or a text skill is refused.
- **What `fs_read` holds for the user, a skill never serves**: a path in a sensitive place
  (The gate, 3: `.ssh`, `.netrc`, `.npmrc`, `.git-credentials`…), by the gate's own rule taken
  from the project folder, links followed, is refused with `refused: X is in a sensitive place
  (…), which a skill never serves: read it with fs_read, which asks the user first`.
- **The call goes through the gate** like every other and is allowed: it names a skill, not a path
  of the place, and reads from the project folder (or Flottille's data folder, for a text)
  whatever the agent's place. The result is
  masked like a file read, and recorded as an `agent.tool_served` event.
- **It grants nothing**: unlike `fs_read`, reading a skill does not let the session write the
  file.

## Tasks and runs

| Tool | Arguments | |
|---|---|---|
| `tasks` | `action list \| show \| add \| edit \| remove`, `name?`, `line?`, `repository?`, `serve?`, `check?`, `read_only?`, `ask_before_running?` | The project's tasks (`docs/projects.md`, "Tasks"). `list` gives one line each: `name: line`, and where it runs, whether it keeps running, is a check, changes nothing, is judged each time it runs; `show` gives one. Every agent reads them; a root writes them ("The project"): `add` takes `name` and `line` and the flags, `edit` changes what it is given (`repository: null` puts the task back at the place's top), `remove` removes it, refused while a setup step runs it. A child's write is refused, pointing it to `ask_parent`. |
| `run` | `task` or `line` (exactly one), `repository?`, `timeout? 1..600`, `why?` | Runs a task (`refused: the project has no task X: list them with tasks {"action": "list"}`) or one line, in the repository's folder (the task's, or the one named) or the place's top. Its description holds a call of each form: `{"task": "test"}`, `{"line": "cargo test -p core", "repository": "api", "timeout": 120}`. |
| `run_status` | `run`, `action? output \| stop`, `tail? 1..1000`, `start?` | `action output` (the default): the run's state and lines of its output, the last `tail` (40 by default), or `tail` lines from line `start`. `action stop`: stops the run and everything in its process group, `SIGTERM`, then `SIGKILL` 5 s later, and answers once it ended; `tail` and `start` are refused with it. |

Until #131 these were `commands_list`, `commands_run` (its `command` now `task`) and
`commands_status`. For one release a call under an old name is the new tool's (`commands_list`
is `tasks` with `action list`), and the store renamed them in the agents' fixed lists and their
pending requests.

- **No shell, ever.** A line is split as `flottille_core::command` does (the setup's `run` steps
  too): words on whitespace, a quoted part kept whole, a backslash literal. A newline, `|`, `&`,
  `;`, a backtick, a redirection, `$(`, `${`, `$VAR` or `%VAR%` outside single quotes, and `*`,
  `?`, a leading `~` or `[…]` outside any quotes are refused with: `Flottille runs a command
  without a shell, and “T” is shell syntax: put it in a script of the repository and run the
  script.` A line naming a shell as its program is split like any other; the gate judges the
  line a shell runs with `-c` (below).
- **Waiting**: the call waits `timeout` seconds (30 by default) for the end. A task
  that keeps running (`serve`) answers at once, and one already running for the agent is joined.
  A call stopped by its client stops its run.
- **The answer**: `run <id>: <name> <state> after <s> s` (`is running`, `exited with code N`,
  `was killed by signal N`, `was stopped`, `was interrupted…`), then, while it runs, `it keeps
  going: read it with run_status {"run": "<id>"}, stop it with run_status {"run": "<id>", "action": "stop"}; Flottille tells you when it ends`, then `(lines a-b of N…)` with
  how to read the lines before or after (`run_status {"run", "start"}`), then the lines. A
  line longer than 2,000 characters shows as several; the lines shown are cut to stay under the
  limit.
- **Only the agent that started a run** reads or stops it: `refused: X is not a run this agent
  started`.
- **The output kept** is its last 64 KiB (cut back from a line's start once it passes 80 KiB),
  standard output and error together, masked when shown and when stored: the values of the
  daemon's variables named like secrets, every value of the `KEY=VALUE` lines of the place's
  `.env` and `.env.*` files (at its top and each repository's), private keys, URL passwords and
  tokens shaped like the usual providers' (`flottille_core::secrets`).
- **The environment of a run**: the daemon's, then the place's variables filled
  (`docs/workspaces.md`: a workspace's, frozen when it was made; in the project folder the
  project's, with `{workspace}` = `main`, `{workspace.path}` the folder and `{branch}` empty), and
  then, whatever they say:
  - the forge tokens empty: `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`,
    `GITHUB_ENTERPRISE_TOKEN`, `GITLAB_TOKEN`, `GITLAB_ACCESS_TOKEN`, `GL_TOKEN`, `OAUTH_TOKEN`,
    `BKT_TOKEN`;
  - `GH_CONFIG_DIR`, `GLAB_CONFIG_DIR` and `BKT_CONFIG_DIR` pointed at an empty folder
    (`$XDG_STATE_HOME/flottille/empty`), so that the forge command lines find no login;
  - no git hook: `GIT_CONFIG_COUNT` incremented, with `GIT_CONFIG_KEY_<n>=core.hooksPath` and
    `GIT_CONFIG_VALUE_<n>=/dev/null`, since the hooks are files the agent can write, and would
    run outside the gate's sight;
  - no `XDG_RUNTIME_DIR`, which says where the daemon's socket is (a client a run starts is
    refused anyway);
  - no push: `url./dev/null/flottille-refused-push/.pushInsteadOf` set the same way, which
    rewrites every push URL to a path that is no remote, unless the run is itself a `git push`
    (its own line, no wrapper) the user or the judge let through.
- **A push is the gate's**: `git push` is a call to judge, a forced one base, and only the push
  the gate read and let through goes out. A harmless command (a task, a test run)
  cannot push from a script the gate never read. A push from a workspace is also refused, before
  git runs, when a repository's remote no longer leads where it did when the workspace was made
  (`docs/commands.md`, "Where the remote leads").
- **A stop ends the run's process group, not every descendant.** `run_status` with `action stop`, a timeout of
  the call's client and the daemon's stop signal the group the run was spawned in (`SIGTERM`,
  then `SIGKILL` after 5 s), and what the run left in its group is killed when its own process
  ends. A process that left the group (`setsid`, a daemon that forks and detaches) escapes all
  of this and keeps running; Flottille does not stop it. It stays the daemon's descendant,
  though: the daemon is the subreaper of what it starts, so a process orphaned there is taken in
  by the daemon rather than by init, waited for when it ends, and refused if it connects to the
  socket.
- **The agent is told when a command that outlived its call ends.** A call that answered with the
  command still running (a command that `serve`s, or one past the call's `timeout`) is followed by
  a `command-ended` delivery to the agent that started it when the command ends, urgent like a
  child's report and by the same path (`docs/design.md` §5.2: stored, then pumped between turns or
  pinned as a note on the next tool result): the line, how it ended (`exited with code N`, `was
  killed by signal N`, or `was stopped by the user`), how long it ran and the last 20 lines of its
  output, masked. A command whose call saw it end, one the agent stopped with `run_status`,
  and those that stop with their agent or the daemon deliver nothing. No timer is involved. A
  command that ends at the very moment its call answers can give both: the call's answer (the end
  it saw, or the run still going) and the delivery, so the agent may be told the same end twice;
  nothing is lost, and the delivery carries the run's id to recognise it. A command the user stops
  while its call still waits says so in that answer (`was stopped by the user`); the agent's own
  stop says `was stopped`.
- **The user sees them** (`docs/design.md` §10): the session's right rail lists the commands of
  its tree, a project and a session with one running show it in the left sidebar, and the user can
  read a command's output live, stop it (its whole process group, as above) or dismiss it once it
  ended (`docs/protocol.md`, "The run methods"; `flottille runs`, `docs/cli.md`).
- **The user runs a task too**: the Run button of a task on the project page (`task.run`) runs it
  in the project folder, with the project's variables, as a run of no agent: no gate (the user
  asked), no delivery, the same records. It is listed on the project page with the same rows and
  output panel as the session's rail, and in the left sidebar with the project's runs.
- **Runs are records**: the store's `runs` (with the end of their output once ended), and the
  `run.started` and `run.ended` events. Each run's process group is recorded while it runs: a
  daemon started after a crash kills the groups a previous one left (only while their leader is
  the process it started) and marks their runs interrupted. A daemon that stops stops its runs,
  interrupted. When a run's own process ends, what it left in its group is killed with it. The
  runs that ended go with their session, as its images do: when its root failed or ended and
  cannot be resumed, or 7 days after a root that can (`docs/protocol.md`, `agent.image`).

## The forge

| Tool | |
|---|---|
| `forge_read {workspace?, repository?, detail? short \| full}` | The pull request of a workspace's branch in each of its repositories, or the one named (`workspace` names the workspace: the agent's own when left out, required in `main`, where `refused: this agent works in main, which has no workspace of its own: name the workspace forge_read acts on …` lists the project's): number, state, base, link, review, checks with their states, and how many comments and reviews it has (`comments: 2, read them with forge_read {"detail": "full"}`); `detail full` gives their texts, cut and under the result limit. Read only, through the user's own `gh`, run by the daemon; an agent has no forge sign-in of its own. A repository the workspace does not take is refused (`refused: X is not a repository of this agent's workspace: it has …`). Every agent of a project whose repositories declare a forge has it, in `main` as in a workspace; an agent in a workspace that names another workspace than its own is refused. |

| `forge_write {workspace?, action, repository, title?, body?, draft?, waive?}` | Opens the pull request of a workspace's branch (named as for `forge_read`) (`pr-create`, with the project's reviewers), merges it by squash (`pr-merge-squash`; refused while its checks fail or run or a review asks for changes, unless `waive` gives the user's words, which holds the merge for their approval) or re-runs the failed jobs of its last commit's pipelines (`pipeline-rerun`), in one of the workspace's repositories, through the user's own `gh` run by the daemon (`docs/commands.md`, "`forge_write`"). Every agent of a project whose repositories declare a forge has it, in `main` as in a workspace. Its description holds a call of `pr-create` and of `pr-merge-squash`. |

`forge_read` goes through the gate like every call, which allows it; a `forge_write` call is to
judge, and one that waives the checks base.

## The workspace

| Tool | |
|---|---|
| `workspace {action: list \| show \| create \| prepare \| link_ticket, name?, repositories?, ticket?}` | The project's workspaces, as the window manages them (`docs/workspaces.md`). `list` gives each one's state, branch, repositories and ticket; `show {name?}` one whole, each step of its preparation with its state and why a failed one failed. `create {name, repositories?, ticket?}` makes one as `workspace.create` does: its worktrees, answered once they are made, then its setup in the background. `prepare {name?}` resumes a failed preparation (`workspace.prepare`); `link_ticket {name?, ticket}` links its ticket (`workspace.link_ticket`). `name` is the agent's own workspace when left out, but for `create`; in `main` it names one. Only a root has it: a child is refused (`refused: the agent the user talks to manages the project's workspaces; … ask your parent (ask_parent)`). |
| `workspace_remove {workspace?}` | Removes a workspace, the agent's own when `workspace` is left out and the one named in `main`, where it is required, as `flottille workspace remove` does (`docs/workspaces.md`, "Removal"): refused, nothing touched, while a repository holds uncommitted work or commits on no remote, or while another agent than the caller's tree works in the workspace or a command run not the tree's own runs there (`refused: workspace … is not removed, nothing was touched: the agent …, outside this agent's tree, works in it…`), each named; then deletes, in each repository, the workspace's branch when it holds no commit of its own (every commit of it is in its base's tracking ref), and keeps the others. Every agent has it; an agent in a workspace removes only its own; each call is to judge. |

`list` and `show` run at once. `create`, `prepare` and `link_ticket` are to judge, as any write
of "The project" is: an `action` request, which says what it does in words and is done only while
they hold. Making a workspace ready plays setup steps that run without the gate, so `create`
(the project's setup) and `prepare` (the steps it does again: those not done, and the done ones
whose result is gone) are also judged as setting each of those steps is ("The project", "A line
set is judged as a run of it" and what only the user's word may say), each reason said as `step
2 of the setup, run \`sudo make install\` in the workspace folder: the line it runs, …`; the strongest class wins, so one
base `run` or `task` step makes the call base, and a step refused as a run refuses it. The `copy`
(a secret included), `link` and `root_entries` steps do not raise the class, since the user, or an
agent through the gate, wrote them and playing them is what the user configured: their reasons
are listed for the judge and the card, and the call stays to judge. The request lists the
steps (`Its setup then runs, without the gate:`, `1. copy .env.example in the workspace
folder`), and the judge reads them in a block of their own, as data. `agent_launch` with
`new_workspace` makes its workspace the same way, so it is judged the same, with the same
reasons.

 How the forge is read, and what it never touches (a token,
`gh auth token`), is in [`forges.md`](forges.md).

## The tickets

| Tool | |
|---|---|
| `ticket_read {workspace?, ticket?, detail? short \| full}` | The ticket the agent's workspace (or the one `workspace` names, required in `main` unless `ticket` is given) is linked to, or `ticket`, another ticket of the same tracker by its key (`#123` or `ABC-123`): its reference, tracker, state, title, link and description, masked, and how many comments it has with the newest one's author (`Comments: 12, … read them with ticket_read {"detail": "full"}`); `detail full` gives the comments (the newest 30). Read only, through the user's own `gh` or `acli`, run by the daemon; an agent has no tracker sign-in of its own, and none of either's write commands is reachable from it. The tracker is that of the workspace's ticket, or GitHub's (its own repositories' issues) when it has none; a key of the other tracker is refused (`refused: ABC-123 is a Jira ticket, and this workspace's tracker is GitHub issues (#123): …`), and nothing is asked of any tracker. A workspace with no ticket and no key is refused (`refused: workspace X is linked to no ticket: …`). An unreadable ticket (`gh` or `acli` missing or signed out, no such ticket) is an error saying why and what to do. Every agent of a project that declares a tracker has it, in `main` as in a workspace, whether a ticket is linked or not. |

It goes through the gate like every call, which allows it. How a ticket is read, Jira's text
turned into plain text, and what is assumed of `acli`'s output, is in [`tickets.md`](tickets.md).

## The project

What the user does in the window, the agent they talk to does too, under the gate, always on its
own project: a root has these four tools and the writes of `tasks` (above), a child none of them
(it does its brief). Families with an `action`, as `agent_status` is:

| Tool | |
|---|---|
| `skills {action: list \| show \| add \| edit \| remove, name?, description?, path?, text?}` | The project's skills (`docs/skills.md`). `add` takes `name`, `description` and one of `path` (a file or a folder of the project folder) and `text` (a text Flottille keeps); `edit` changes what it is given; `remove` removes it, and its text. As `flottille skill` does. |
| `commands {action: list \| show \| add \| edit \| remove, name?, description?, instructions?, template?}` | The project's commands (`docs/commands.md`). `add` takes `name`, `description` and `instructions`, or a `template` (`open-prs`, `merge-stack`, `close`) added as it is; `edit` changes what it is given. A command is sent to a child by writing `/name` in `agent_send`'s message. |
| `setup {action: list \| add \| edit \| remove \| move, number?, to?, step?}` | The project's setup (`docs/projects.md`, "The setup"), its steps numbered from 1 in order. `add {step, number?}` adds one, the last when `number` is left out; `edit {number, step}` replaces one; `remove {number}`; `move {number, to}`. A `step` is whole, as the file has it: one of `copy` (globs), `link`, `root_entries`, `run` (a line) and `task`, with `repository` or `for_each_repository`, and `time_limit` (seconds) with `run` or `task`; it is checked as the file is. The workspaces made after play it; one already made keeps its own. |
| `project {action: settings \| set_settings \| rules \| set_rules, …}` | `settings` reads each repository's forge (and what its remote proposes) and the tracker (`docs/projects.md`). `set_settings {repositories?: [{path, forge}], tracker?}` declares a repository's forge or the tracker (`null` removes it; a tracker left out is kept), checked as `project.set_settings` checks them. `rules` reads the rules the project's agents get from now on and where they come from (the project's own, the user's for all projects, or Flottille's default, `docs/projects.md`, "Rules"); `set_rules {text}` replaces the project's own rules by `text` (8,000 characters at most), for the agents created after: the one that asked keeps what it started with. |

The file is written where it stands, its comments kept.

- **Reads run at once.** `list` and `show` of `tasks`, `skills` and `commands`, `setup` `list`,
  `project` `settings` and `rules`.
- **Every write is to judge** (a card in Ask, the judge in Auto) unless it is base below, as any call is (answered at
  once that it waits, the result later as a delivery). Its request is an `action`: it says what
  it changes in words, one sentence each (`Add the skill style (docs/STYLE.md): The code style`;
  `Add the command /ship: …`, `Its instructions: …`; `Set the tracker to Jira on
  acme.atlassian.net (ABC)`; a `set_rules` request shows the new text whole), allowed once, and it
  is done only while those words still hold: a command whose instructions (`They were: …`)
  changed meanwhile is not written (`Request #n (commands) was not executed: what it does changed
  since the user allowed it; it would now: …`). A change of the tracker is base: nothing checks it
  against the project, as a forge is checked against its remote. A `set_rules` is base too: it
  rewrites the instructions of every agent created after it, so no judge decides it.
- **A line set is judged as a run of it.** A task's `line` and a setup step's `run` or `task`
  run later without the gate: a task in a workspace is harmless, a workspace's setup plays on
  its own. So `add` and `edit` that set one are also judged as `run` would judge that run, with
  the same inputs, each reason said as `the line it sets, …`: base when the run would be
  (`sudo`, a forced push, a sensitive path the line names, the never list), refused with shell
  syntax.
  - A task is judged as a run of that task, as the write leaves it (its flags, its name in the
    never list), in the folder it would run in; a `task` step as a run of the task it names, in
    the step's repository or the task's.
  - A setup step is judged in each folder it would run in, those of the agent's own place
    standing for the workspace not made yet: its workspace, or the project's repositories in
    `main`.
  - Whether a rewrite is of history on a remote is looked at for the line set, in its folder.
  - A line that is an existing task's keeps that task's rules under another name: never-listed,
    it is base to set and to run (`run {task}` of a copy, `run {line}` of it); marked
    `ask_before_running`, a copy of it without the flag is base to set.
- **What only the user's word may say is base.** Beside the line, a write is base when it:
  - turns `ask_before_running` off (`tasks edit`);
  - sets `check` or `read_only` (`tasks add` or `edit`): the window and the lists show them as
    what the task is, so an agent does not claim them;
  - adds or edits a `link` or `root_entries` step, which makes what is written in each workspace
    reach the user's checkout;
  - adds or edits a `copy` step with a glob that matches a sensitive name: `.env` and `.env.*`
    but the templates, a key (`*.pem`, `*.key`, `id_rsa`, `id_ed25519`…), a sensitive place
    of "The gate" or a folder holding one (`.ssh`, `.npmrc`, `.netrc`, `.config`…). Copying a
    secret into every workspace is the user's choice, not the judge's.

  An edit's request says what it was (`It was: …`), a step's what it does and where (`Add
  setup step 2: run \`npm ci\` in each repository, leaves first`).
- **What an agent never does**: change a session's mode or choose the judge (no tool has them),
  or reach another project (`project` is not an argument). An agent's skill is never the project
  folder itself nor a repository's root (`.`, `api`, or a link to one): what a whole checkout
  holds is not a text to read.
  Such an argument is refused as one that does not fit the tool. A write that cannot be done (an
  unknown command, a skill outside the project folder, a repository the project has not) is
  refused before anything is asked.

## The tree's tools

The tools through which agents launch other agents and answer for them (`docs/design.md` §4.3,
§5; `docs/agents.md` "The tree"). `agent_id` is the id `agent_launch` gave; the `agent_*` tools
reach only the caller's own children, and are refused with `refused: <id> is not one of your
children: agent_status lists them` for any other agent.

Two of them take an `action`, as the tools of a family do: `agent_status` (`status` or `conversation`) and `run_status` above.

| Tool | |
|---|---|
| `agent_launch {title, brief, provider?, model?, effort?, role?, new_workspace?, repositories?}` | Starts a child on one task and answers at once with its id. `provider` is the caller's when left out, and the child's for life; `model` and `effort` the provider's defaults when left out (`docs/agents.md`, "Providers"): a provider not enabled or not ready, or a model or an effort it is known not to offer, is refused naming those that are (`refused: provider opencode is not enabled (…); the ready providers are claude`). `role` is `worker` (by default) or `lead`; `workspace` is an existing workspace's name, `new_workspace` the name of a new one (made at once, with `repositories` as its repositories, the child's brief sent once it is prepared; a workspace that fails fails the child; the launch is judged as `workspace create` is, "The workspace"), neither for the caller's own place; both are plain optional fields, and a mix is refused naming the field to use (`refused: workspace and new_workspace are exclusive: …`, `refused: repositories goes with new_workspace, …`). Its description holds a call of each form. Refused, never queued, above the project's children cap (`refused: N children already run in project P, its cap: do the work yourself, or wait for one of them to end (agent_stop ends one of yours)`) and past the depth limit. |
| `agent_send {agent_id, message}` | A message to a child, urgent: on its next tool result in a turn, a new turn otherwise. It answers the child's question. |
| `agent_status {agent_id?, action? status \| conversation, detail? short \| full, after?}` | `action status` (the default): one line per child (or for the one named): what it does (in a turn, `running \`<line>\` for 12 min`, `waiting for approval #3`, `waiting on the provider`, idle, reported, ended or failed with why), the time since its last sign of activity, its context use. For all the children it stops there (`detail short`) and says how to get more; for the one named, or with `detail full`, each also has its last three messages. `action conversation` (with `agent_id`; `after` pages): a page of the child's conversation, oldest first, one line per event that says something (`#<n> sent: …`, `said: …` for a whole reply, its streamed pieces joined and masked, two messages apart, `tool call: …`, a tool served and its outcome, a turn's end, a replacement, a report), each line of 2,000 characters at most (a longer reply goes on, whole, on the next lines), the page under 30,000; it ends with the `after` to pass for the next page. |
| `agent_stop {agent_id}` | Stops the child and its own children, deepest first; what was queued for them is superseded; their runs are stopped. Their workspaces stay. |
| `report {text}` | The caller is done. At most 10,000 characters, so that it rides whole on its parent's results; a longer one is refused, asking for fewer words and a file that holds the details. A child's report goes to its parent at once (`child-report`, with its workspace, its branch and the last commit of each repository), and the child ends when its turn does (or two minutes later, if its turn goes on). A root's report is shown to the user (`agent.reported`) and the root stays. |
| `ask_user {question}` (a root) | A question to the user (`agent.asked`), answered later as a message. |
| `ask_parent {question}` (a child) | A question to its parent (`child-question`), answered with `agent_send`. Until then the child shows as waiting on its parent. |
| `approval_answer {request_id, decision, why}` | Answers a request held for the caller: `deny` (the agent that asked is told, with the why) or `forward` (to the caller's parent with its why, or to the user from a root). There is no way to allow: `allow` is refused as a decision the tool does not have, and a request the caller does not hold, or no longer holds, is refused (`refused: request #n is not held by this agent: …`). |

They go through the gate like every call (it allows them). Their texts are masked of the
daemon's secrets before anyone else reads them.

## The gate

Every call goes through `gate::decide` once (`docs/design.md` §7), in the order of
`docs/reference/hemera.md` §4. It is given the agent's place, the tool, the arguments, the words
of the line a run would start, the task it names, the paths the call reaches, the
project's never list, Flottille's own folders and whether the workspace's branch is on a remote,
and answers with the call's class, or a refusal. The class does not depend on the mode: the mode
says who answers a call to judge (`docs/design.md` §7.1).

1. **Refused**, with a sentence saying why and what to do instead (under 2,000 characters): what
   cannot be made as it is, in every mode.
   - shell syntax in a line (`run` splits a line without a shell, and refuses what it
     cannot split), and a call whose arguments do not fit the tool;
   - a login printed: `gh auth token`, `glab auth token`, `auth status` with `--show-token` (`-t`);
   - `flottille` itself: it drives the daemon, and agents act only through their tools. A copy
     of it under another name, or any program that speaks to the socket, is refused by the
     daemon itself (`docs/protocol.md`, "Transport").
2. **Base**: a card for the user in every mode, never the judge's, with a strong warning.
   - the project's `never` list: an entry matches a line's words from any position (its first
     word is a program, in any folder), or a task by its name, and then every task and line of
     the same words as that task's; and a run when the
     project's configuration cannot be read, since what it never allows is not known;
   - a write under a `.git` folder or file (`fs_write`, `fs_edit`, a link that leads there), and a
     command any of whose words (or an option's value after `=`, or a `NAME=value`'s value) names
     a path in one;
   - a sensitive place, read or written: `.env` and `.env.*` anywhere but the templates
     (`.example`, `.sample`, `.template`, `.dist`), `.ssh`, `.gnupg`, `.aws`, `.azure`,
     `.config/gcloud`, `.kube`, `.docker/config.json`, `.config/gh`, `.netrc`, `.npmrc`,
     `.pypirc`, `.git-credentials`, `.local/share/keyrings`, `Library/Keychains` and the agents'
     logins (`.claude`, `.claude.json`, `.codex`, `.local/share/opencode`) wherever they sit along
     the path, `/etc/shadow`, `/etc/sudoers`, `/etc/sudoers.d`, and Flottille's own folders (data,
     configuration, state, runtime). Inside the place only what sits below it counts. A path
     outside is checked as written and where it leads (a link anyone made, a configuration
     folder that is a link, a `..` after a link), against Flottille's folders as named and
     resolved;
   - a write, a move or a removal of a folder above Flottille's own (`mv ~/.config ~/c.bak`, `rm
     -r` of it, `fs_write` over it), which would take its configuration or data away: by a file
     tool that writes, or by a run that is not known to only read and names it as a word (the
     folder a run starts in may hold Flottille's: a project in the home folder). What deletes the
     untracked files without naming where counts the folder it takes them from: the folder it
     runs in (after its `-C`s) for a `git clean`, the top of its worktree for a `git stash` with
     `-u`, `--include-untracked`, `-a` or `--all`, and for a `git clean` of a pathspec from the
     top (`:/`, `:(top)`, `:(top,…)`). That top is the `--work-tree` it names, else found on the
     disk as git finds it: the nearest folder holding a `.git`, up from the one it runs in,
     inside the place or not (a home folder that is a repository);
   - a path in `/proc` through what of any process leads elsewhere on the disk: its `root`,
     `cwd`, `fd`, `exe` or `map_files` (`/proc/self/root/…`, `/proc/<pid>/cwd/…`), under any
     name it is reached by (`/dev/fd/3`, a link to `/proc/self/cwd`; `/dev/stdin` for a file
     tool). The words of a run that name its own streams or nothing (`/dev/stdin`,
     `/dev/stdout`, `/dev/stderr`, `/dev/null`, `/dev/fd/0` to `/dev/fd/2`) are harmless;
   - a path that cannot be followed on the disk (more than 40 links, a folder that cannot be
     read): where it leads is not known;
   - a recursive deletion that would take a repository: `rm` with `-r`, `-R` or `--recursive`,
     and `find` with `-delete` (or `-exec rm -r`), when a target, resolved from the folder the
     run starts in, is the place's folder, a repository's, or a folder above one (`rm -rf .`,
     `rm -R <repository>`, `find . -delete`). Deleting inside a repository is not (`rm -r
     target`);
   - a push forced or that deletes: `--force`, `-f`, `--force-with-lease`, `--force-if-includes`,
     `--mirror`, `--delete`, `-d`, `--prune` (or any start of their names, as git takes them:
     `--forc`, `--mirr`), a `+refspec` or a `:branch`; and a push naming the program that
     receives it (`--receive-pack`, `--exec`), which runs on this machine for a remote that is a
     path;
   - git settings that run programs, lead pushes elsewhere or bring in other files: a key of
     `core.*`, `alias.*`, `remote.*`, `url.*`, `push.*`, `credential.*` or `include*`, or a key
     whose value git runs (`diff.external`, `diff.*.command` and `textconv`, `difftool.*`,
     `mergetool.*`, `man.*` and `browser.*` `cmd` and `path`, `merge.*.driver`, `filter.*`,
     `pager.*`, `hook.*`, `guitool.*`, `sendemail.*`, `instaweb.*`, `gpg.program`,
     `gpg.*.program`, `gpg.ssh.defaultKeyCommand`, `sequence.editor`,
     `interactive.diffFilter`, `submodule.*.update`, `uploadpack.packObjectsHook`,
     `imap.tunnel`, `tar.*.command`, `web.browser`, `trailer.*.command` and `cmd`, and
     `protocol.allow` and `protocol.*.allow`, which can let `ext::` URLs run programs) given to
     `git -c` or `--config-env`, or written by `git config` in any write form (`--edit`
     included); `git remote add`, `rename`, `remove`, `set-url` and `set-branches`; git's
     configuration set or cleared through the environment (`GIT_CONFIG*=…`, `env -i`, `env -u
     GIT_CONFIG…`), which would lift the wall on pushes and hooks; and a program git runs named
     through the environment (`GIT_EXTERNAL_DIFF`, `GIT_PAGER`, `GIT_EDITOR`,
     `GIT_SEQUENCE_EDITOR`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_ASKPASS`, `GIT_PROXY_COMMAND`,
     `GIT_EXEC_PATH`);
   - removing git's safety net, in `main` and in a workspace, which share it: `git reflog
     expire` and `delete`, `git gc --prune=<anything>`, and `git prune` but `-n` and
     `--dry-run`;
   - running as another user: `sudo`, `sudoedit`, `doas`, `pkexec`, `run0` and `su`, in any form.
     An agent rarely needs it, and `sudo` resets the environment, which drops the wall on pushes;
   - reading a process's environment (`/proc/<pid>/environ`, `self` included), or anything under
     `/proc/<pid>/` of a process that is not one of the agent's runs;
   - rewriting history that is on a remote, or the user's own checkout's: `git rebase`,
     `git commit --amend` (and the fixups that amend or reword), `git reset <commit>`,
     `git update-ref`, when the workspace's branch is already on a remote or the agent works in
     `main`; `git filter-branch` and `filter-repo` anywhere;
   - discarding work in the project folder (`main`), or moving the branch it has checked out:
     `git checkout` with `--`, `.`, `-f`, `-B`, `-p`, `--ours`, `--theirs`, or a word that is a
     file or folder there, a pattern or a pathspec magic (not with `-b` or `--orphan`); `git
     switch -f`, `--discard-changes`, `-C`; `git checkout-index -f`; `git read-tree -u`; `git
     restore`, `git clean`, `git reset` and `git update-ref`, in any form; `git stash` but `list`
     and `show`; `git worktree remove`; `git branch -d`, `-D`, `-f`, `-m`, `-M`, `-C`; `git rm`
     but `--cached`; `git apply` and `am` with `--reject`; `--abort` of a merge, a cherry-pick, a
     revert or `am`; `git symbolic-ref` that writes; `git submodule update` or `deinit` forced;
   - publishing: `npm`, `pnpm`, `yarn`, `bun`, `cargo`, `poetry`, `flit`, `hatch`, `uv` with
     `publish` or `unpublish`, `twine upload`, `gem`, `dotnet` and `nuget` with `push`;
   - `acli` and `bkt`, Atlassian's and Bitbucket's command lines, every line, reads included:
     they run with the user's own sign-in, in a place no variable of ours moves, so it cannot be
     emptied as `gh`'s is (`docs/forges.md`, "Bitbucket"). An agent reads a ticket with
     `ticket_read` and the pull requests with `forge_read`;
   - a `forge_write` merge that waives the checks or a review; a change of the project's tracker
     or of its rules (`set_rules`);
   - a write of `tasks` or `setup` whose line, or task, would be base to run, and one that says
     what only the user's word may say ("The project": asking turned off, `check` or `read_only`
     set, a `link` or `root_entries` step, a `copy` of a sensitive name).
3. **Harmless**: passes in every mode.
   - reading inside the agent's place (the project folder or the workspace), the tree's tools,
     `tasks` `list` and `show`, `run_status`, `skill_read`, `forge_read`, `ticket_read`;
   - writing inside a workspace (`fs_write`, `fs_edit`, and `git add`, `mv`, `rm` there);
   - a line the rules know to only read, inside the place: the reads of git (`status`, `log`,
     `diff`, `show`, `branch` and `tag` listings, `config` and `remote` reads…), of `gh` and
     `glab` (`pr view/list/diff/checks/status`, `issue view/list`, `run view/list`, `mr
     view/list/diff`, `ci view/list/status`, `repo view`, `auth status`, `api` with GET, no
     field, no input and not `graphql`), and the common reading programs (`ls`, `cat`, `grep`,
     `rg`, `find`, `fd`, `tree`, `ps`, `pgrep`…; not `less` and `more`, whose commands run
     programs). A program counts as one of them only when it is the system's: named bare, and
     found through the run's `PATH` outside the project folder, its workspaces, the place and
     the user's own `~/.local/bin` and `~/bin`, where an agent a judge allowed may have written
     it. `PATH` is the run's own, filled for the repository it runs in. `./ls`, `/path/to/git`
     or an `ls` the agent put in a folder of `PATH` is the agent's own file, to judge, and the
     judge is told so;
   - an option of those programs that writes a file, inside a workspace only (`git diff`,
     `log`, `show` `--output`, `git format-patch`, `tree -o`, `find -fprint`, `file -C`): in
     `main`, it is to judge;
   - a task in a workspace, not marked `ask_before_running` and whose program is not a shell. A
     line is the task it is when the project has a task of the same words where it runs.
4. **To judge**: everything else, a card in Ask and the judge in Auto. Among them: writing in the
   project folder (`main`); a path outside the place, or one that cannot be resolved; a task in
   `main`, marked `ask_before_running`, or whose program is a shell; a line the rules
   do not know to only read, or whose program is the agent's own file; an option of a reading
   program that runs one (`find -exec`, `-execdir`, `-ok`; `fd -x`, `-X`; `rg --pre`,
   `--hostname-bin`; `git grep -O`; `--ext-diff`, `--textconv`, `cat-file --filters`; `git help`
   with an option; `date -s`); a recursive read that reaches hidden or ignored files (`grep -r`,
   `-R`; `rg -u`, `--hidden`, `--no-ignore`; `git grep --untracked`, `--no-index`; `fd -H`, `-I`,
   `-u`; `find -L`, `-H`) over a folder that holds a sensitive file, or more files than the gate
   looks through (20,000), the judge told which; `git commit`, `git push`, `git rebase` of a
   branch on no remote, `git reset --hard` in a workspace, `git -c` or `--config-env` of other
   keys, git's other global options, `git remote` and `git config` writes of other keys; the
   forge command lines beyond their reads; `forge_write`, `workspace_remove`, and a write of
   `tasks`, `setup`, `skills`, `commands` or `project` ("The project"), with the line it sets;
   `workspace` `create`, `prepare` and `link_ticket`, and `agent_launch` with `new_workspace`,
   with the setup steps they play ("The workspace").

   The rules of 1 and 2 are judged on the line and on every program a wrapper runs, unwrapped in
   turn (16 at most): `env` (its options, `NAME=value` words and `-S` line), `sudo`, `doas`,
   `nice`, `nohup`, `timeout`, `time`, `stdbuf`, `command`, `exec`, `xargs`, `npx` (and its `-c`
   line), `pnpm dlx`/`exec`, `yarn dlx`/`exec`, `bunx`, `uvx`, and a shell (`sh`, `bash`, `zsh`,
   `dash`, `ksh`, `mksh`, `ash`, `fish`) with `-c`, `script -c` and `watch`, whose line is split
   as `run` splits one and judged in turn: a line that cannot be split is refused. A
   shell running a script is not unwrapped: the script is not read. The line an option of a
   reading program runs (`fd -X git push -f`) is read the same way, and so is each line of `git
   rebase -x` and `--exec` (`git rebase -x 'git push -f'` is a forced push); git runs those
   through a shell, so one that cannot be split is refused. git's global options are
   skipped to class its command, and count too: the stronger wins.

   The base rules are also judged on every end of every layer whose program is not a reading
   one, from each word that is not an option (from after the command, for git): a wrapper the
   gate does not know (`setsid`, `ionice`, `taskset`, `flock`, `chrt`, `strace`, `git bisect
   run`…) cannot turn a base line into one to judge.

   **Known gaps**, which no command gate closes: a script written with `fs_write` and then run
   (`./script.sh`, `sh script.sh`) is not read, so what it does is out of the gate's reach, but
   for a push, which the environment walls; a git alias in the user's own configuration
   (`alias.<name> = push`) is run by git itself, and the gate sees only the alias's name.
   Running a script the agent wrote is itself a line to judge.

The words of a line that name paths (every word but the program, an option's value after `=`,
what follows each letter of short options, whose value may be attached as in `-Dt<folder>` or
`-o<file>`, a `NAME=value`'s value) are resolved from the folder the run starts in, as a file tool's path is.

**In Auto**, a call to judge first goes to the judge, when one is chosen (`docs/design.md` §7.1):
an `allow` performs it at once, recorded as an `agent.tool_served` whose `gate` is `judged` and
whose `judgement` says who judged and why; an `ask`, a failure or a silence makes the request
below, the judgement in it. A base call, and every call in Ask, makes the request directly.

**A held call** is written as a request (`flottille approvals`, `docs/protocol.md`) and
answered at once, not as an error; a root is answered:

> Waiting for the user's approval, request #n. This action has not happened; do not assume it
> did. Continue with work that does not depend on it, or end your turn saying you wait. The answer
> will be handed to you when the user gives it.

and a child, whose request goes to its parent:

> Waiting for approval, request #n: your parent decides whether it goes on to the user. This
> action has not happened; do not assume it did. Continue with work that does not depend on it,
> or end your turn saying you wait. The answer will be handed to you.

A request holds its number (counted across every agent), the call frozen whole (what is
performed once it is allowed; it is shown cut and masked), the reasons, masked, its class
(`judge` or `base`), the judge's verdict when one was asked (`judgement`: `by`, `verdict`,
`reason`), the agent that asked, and who holds it: the user for a root's request,
the parent for a child's, which is told at once (`child-asks`, urgent) and denies it or forwards
it up with one sentence of why (`approval_answer`), one level at a time, every why kept in the
request's `forwards`. The same call key (`claudecode/toolUseId`) of the same agent asked
twice gives the same request. A request that cannot be written refuses the call: `refused: the
request for the user could not be made, so nothing was done; say so and go on with what does not
depend on it`.

**The user answers** a request they hold: allow it once, or deny it. Nothing they answer allows
a later call: the next one goes through the gate again. A request held by an agent is not listed
for the user and cannot be answered by them.

| Command | |
|---|---|
| `flottille approvals [--json]` | The requests the user holds, each with its number, the agent, the call, its class, why, and the judge's verdict when one was asked. |
| `flottille approve <n>` | Allows the request once. Refused for a request held by an agent, or one already decided. |
| `flottille deny <n>` | Denies it: the agent is told nothing was done. |

**An allowed request** is checked again (the gate on the frozen call: a refusal now, or a reason
to ask that the request did not have, stops it), performed as the agent's call (recorded as its
`agent.tool_served`, with the request's number), and its answer stored as the agent's `approval`
delivery, urgent and masked, then sent to it (`docs/agents.md`):

- `Request #n (tool) was approved by the user, and Flottille has now done it. Its result:` and
  the result;
- `Request #n (tool) was approved by the user, but nothing was done: …` for a failure;
- `Request #n (tool) was not executed: …` for a refusal, or when the situation changed;
- `Request #n (tool) was refused by the user. Nothing was done.` for a denial;
- `Request #n (tool) was refused by <agent>: <why>. Nothing was done.` for an agent's denial;
- `Request #n (tool) was approved by the user, but Flottille stopped while it was doing it, and
  cannot tell whether it took effect: check before asking again.` when a daemon stopped while it
  performed it (said by the next one, at its start).
