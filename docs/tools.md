# Tools

The tools of the agents Flottille runs, and the MCP server that serves them: the agent door of
`docs/design.md` §1. It follows `docs/reference/hemera.md` §3, with the rules of
`docs/reference/mcp.md` §4 where they differ. A session lists the tools of its agent, which
depend on what it can do (`docs/agents.md`): the file and command tools below for every agent,
`skill_read`, then `forge` for every agent whose project's repositories declare a forge,
`tickets` for every agent whose project declares a tracker, in `main` as in a workspace, and
`workspace` for every agent (a tool that could only refuse is not listed; what each may do is the
gate's business, not the list's), `skills`, `commands`, `setup` and `project` for a root (below,
"The project"),
`wake` for every agent ("Waking later"), and the tree's tools (`agent_*`, `report`, `ask_user`
or `ask_parent`, `approval_answer`, and `session_send` and `session_read` for a root).

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
  (…), which a skill never serves: read it with fs_read, which asks the user first`. The one
  exception: what is below the project's own `.claude/skills` (where the skills imported from a
  `.claude` folder live, `docs/skills.md`) is looked at from there, `.claude` itself not counting.
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
  too): words on whitespace, a quoted part kept whole, a backslash literal. What a shell expands
  inside double quotes too (a newline, a backtick, `$(`, `${`, `$VAR`, `%VAR%`) is refused outside
  single quotes; `|`, `&`, `;`, a redirection, `*`, `?`, a leading `~` or `[…]` only outside any
  quotes, inside them they are the argument's text (`git commit -m "fix: a -> b"`). A backslash
  before a quote (`\"`, `\'`) is refused anywhere: a shell reads it as an escaped quote, so the
  line a shell runs with `-c` would not split as the gate reads it. Each is refused with:
  `Flottille runs a command without a shell, and “T” is shell syntax: put it in a script of the
  repository and run the script.` A line naming a shell as its program is split like any other; the gate judges the
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
  read a command's output live and stop it (its whole process group, as above); the rail keeps the
  last five that ended, and the protocol and the command line can dismiss one
  (`docs/protocol.md`, "The run methods"; `flottille runs`, `docs/cli.md`).
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
| `forge {action, workspace?, repository?, number?, state?, branch?, detail?, title?, body?, draft?, waive?, method?, events?, from?, minutes?}` | The pull requests of the project's repositories, through the user's own `gh` or `bkt`, run by the daemon; an agent has no forge sign-in of its own. Every agent of a project whose repositories declare a forge has it, in `main` as in a workspace. Its description holds a call of `create`. |

- **The repositories** are those of the agent's place: its workspace's, or the project's in
  `main`. One the place does not take is refused (`refused: X is not a repository of this
  agent's place: it has …; forge acts only on those`), and nothing is asked of the forge.
- **`list {repository?, state?, branch?}`**: the pull requests of each repository, or of the one
  named, a line each (`- #12 open, app/login into main: Add the login page  <link>`): `state` is
  `open` (the default), `closed`, `merged` or `all`, `branch` keeps those of one source branch;
  the newest 100 of each (on Bitbucket, the newest 50, and the answer says when a list may be
  cut). Bitbucket lists by state and the branch is picked from them
  ([`forges.md`](forges.md), "Bitbucket").
- **`show {workspace?, repository?, detail?}`**: the pull request of a workspace's branch in each
  of its repositories, or the one named (`workspace` names the workspace: the agent's own when
  left out, required in `main`, where `refused: this agent works in main, which has no workspace
  of its own: name the workspace forge acts on …` lists the project's; an agent in a workspace
  that names another is refused): number, state, its branch and base, link, whether it is
  mergeable (`mergeable: yes, no conflict with main`, `no, it conflicts with main`, or `not known
  yet` while the forge computes it), review (`none` when there is none), each check with its state
  (`checks: none ran on this pull request` when none did), **for each failed check the end of its
  log, or why there is none** ([`forges.md`](forges.md), "Why a check failed"), one line on how long the
  repository's recent CI runs took (`Recent CI runs of acme/app took 4 min on average (3 to 6 min,
  last 5).`, nothing when there is no finished run to measure; [`forges.md`](forges.md), "How long
  CI takes"), and how many comments and reviews it has (`comments: 2, read them with forge {"action":
  "show", "workspace": "login", "detail": "full"}`); `detail full` gives their texts, cut and
  under the result limit. **`show {number, repository?, detail?}`**: that pull request of a
  repository of the place, the same way; `repository` is needed when the place has several.
  `show` ends with the agent's subscriptions, when it has some (`Your subscriptions:` then `- pull
  request #12 of api, workspace login: checks, review, for 52 min more`).
- **`subscribe {workspace?, repository?, events, from?, minutes?}`**: the next of `events`
  (`checks`, `review`, `comment`, `merged`, `closed`) of the pull request of a workspace's branch
  wakes the agent once, as a `subscription` delivery (`docs/design.md` §5.6). The call answers at
  once; what is so already (checks that already ended) is no event, `show` reads it. `from` is
  `anyone` (the default) or `people`: no bot, nor a comment marked as posted by an agent
  ([`forges.md`](forges.md), "What Flottille posts is marked"). `minutes` is how
  long it waits, 60 by default, 6000 (100 hours) at most. Without `minutes` the wait is sized by the
  repository's CI instead, and the answer says the duration taken and why: twice the average of
  its recent runs, from 15 minutes to the longest, 60 when there is no run to measure by
  ([`forges.md`](forges.md), "How long CI takes"); the answer carries the line `show` gives about
  the runs, with `minutes` too. `checks` delivers how many passed and failed, with
  the end of each failed job's log as `show` gives it; `review` and `comment` their author, their
  state and an excerpt (300 characters). A merged or closed pull request ends any subscription,
  said; so does its time, said. Subscribing again to the same pull request replaces the agent's
  subscription. A pull request merged or closed, a repository with no forge or no pull request,
  and `number` are refused. **`unsubscribe {workspace?, repository?}`** cancels the agent's
  subscriptions to them, every one when it names none.
- **`create`, `merge`, `checks` `{workspace?, repository, …}`**: opens the pull request of a
  workspace's branch (`title`, `body`, `draft`, with the project's reviewers), merges it by
  `method`, `squash` (the default), `merge` or `rebase` (refused when no check ran on it or all were skipped, while its
  checks fail or run or a review asks for changes, unless `waive` gives the
  user's words, which holds the merge for their approval), or re-runs the failed jobs of its last
  commit's pipelines (`docs/commands.md`, "`forge`").
- **`comment {number, repository?, body}`** posts a comment on a pull request of a repository of
  the place; **`edit {number, repository?, title?, body?}`** changes its title or description, at
  least one of them (`gh pr comment`, `gh pr edit`; `bkt pr comment --text`, `bkt pr edit --title
  --description`). A `title` on `comment` is refused before anything is asked (`refused: forge
  comment takes no title: title is for edit and create`).
- **`close {number, repository?, comment?, delete_branch?}`** closes a pull request of a
  repository of the place (GitHub: `gh pr close`; Bitbucket: `bkt pr decline`), posting `comment`
  first; **`reopen {number, repository?, comment?}`** opens a closed one again (`gh pr reopen`).
  The pull request's state is read first: only an open one is closed, only a closed one reopened,
  and the refusal says the state it has (`refused: the pull request #7 of acme/app is merged, not
  open`). Bitbucket cannot reopen a declined pull request: `reopen` there is refused, saying so.
  `delete_branch` (false by default) deletes the remote branch of the pull request, except when a
  workspace of the project is still on it, or the pull request comes from a fork: the branch is
  then left as it is, and the answer says so. Bitbucket Cloud's `bkt` cannot delete a source
  branch when it declines: `delete_branch` there is refused before anything is declined. A
  `title`, a `body` or a `method` on `close` and `reopen` is refused before anything is asked.

`list`, `show`, `subscribe` and `unsubscribe` go through the gate like every call, which allows
them in every mode; `create`, `comment`, `edit`, `merge`, `checks`, `close` and `reopen` are to
judge, and a merge that waives the checks base. A write is done only once the repository's remote is checked to lead where its forge is declared. **What a
write posts is shown whole**: its request says `Its title: …` and `Its body: …` in full (the
arguments recorded are cut at 500 characters), and the judge is given them whole. A title or a
body longer than the 8,000 characters the judge is given whole is refused (`refused: the body is
longer than the 8000 characters the user and the judge are shown whole (…): shorten it`); one
that holds what the masking takes for a secret makes the write base (`its body holds what looks
like a secret: the card shows it masked, and it is sent as it is written`), since the card shows
it masked and the forge gets it as written.

Until #141 the reads were `forge_read` and the writes `forge_write` (`pr-create`,
`pr-merge-squash`, `pipeline-rerun`). For one release a call under an old name is the new
action's, and the store renamed them in the agents' fixed lists and their pending requests.

## Waking later

**`wake {minutes, why}`** asks Flottille to wake the agent once, `minutes` from now (1 to 6000, 100 hours, the
subscriptions' longest), by a `subscription` delivery that starts `Woken by the timer: <why>` and
says when it was asked (`You asked for it at 2026-11-01T10:22:00Z, 20 min later.`). It is a
subscription whose source is a timer (`docs/design.md` §5.6), stored, listed and cancelled as the
others are. The answer comes at once (`Set: a delivery wakes you in 20 min with your why. End your
turn now.`): the agent ends its turn, it never sleeps in one. `why` is required, at most 200
characters. An agent has one wake at a time: a new one replaces it, and the answer says which
(`It replaces your wake for: …`). **`wake {cancel: true}`** drops it (`Cancelled your wake: …`),
refused when there is none, or when `minutes` or `why` come with it.

The agent decides when it wants to be woken: wakes in a row are not limited. A wake whose time came while the daemon was down wakes the
agent at start; an agent that ends takes its wake with it. `forge show` and `forge unsubscribe`
leave it alone.

It is the agent's own bookkeeping: the gate allows it in every mode, never held.

## The workspace

| Tool | |
|---|---|
| `workspace {action: list \| show \| create \| prepare \| link_ticket \| remove, name?, repositories?, ticket?}` | The project's workspaces, as the window manages them (`docs/workspaces.md`). `list` gives each one's state, branch, repositories and ticket; `show {name?}` one whole, each step of its preparation with its state and why a failed one failed. `create {name, repositories?, ticket?}` makes one as `workspace.create` does: its worktrees, answered once they are made, then its setup in the background. `prepare {name?}` resumes a failed preparation (`workspace.prepare`); `link_ticket {name?, ticket}` links its ticket, and unlinks it with `ticket: null` (`workspace.link_ticket`). `name` is the agent's own workspace when left out, but for `create`; in `main` it names one. Every agent has it. A child reads the workspaces; its writes, `remove` included, are refused (`refused: the agent the user talks to manages the project's workspaces; … ask your parent (ask_parent)`). |
| `remove {name}` | Removes a workspace of the project, as `flottille workspace remove` does (`docs/workspaces.md`, "Removal"), **always held for the user**, whatever the mode and the judge: a request whose card says what would be lost, each repository's uncommitted files and commits on no remote (`This would be lost, and the workspace is removed only if your answer says to discard it:`, `api has commits on no remote, on its branch shop/login: 1a2b3c4 Add the login`), or `Nothing would be lost`. Allowed, it is refused, nothing touched, while something would be lost, unless the user's answer said to discard it (`flottille approve <n> --discard`), and while another agent than the caller works in the workspace, of its tree or not, or a command run not the tree's own runs there (`refused: workspace … is not removed, nothing was touched: the agent …, outside this agent's tree, works in it…`, `the agent …, of this agent's tree, works in it`), each named; then deletes, in each repository, the workspace's local branch once its work has landed (no commit of its own, or its pull request merged, as the forge says), never the remote one, and keeps the others, its answer saying each and why (`docs/workspaces.md`, "Removal"). An agent never removes the workspace it works in, whose folder its removal would take from under it (`refused: this agent works in the workspace login, and removing it would take the folder it works in from under it: …`), named or left out; a child removes none. Until #141 it was the tool `workspace_remove {workspace?}`, still answered for one release. |

`list` and `show` run at once. `remove` is base (above). `create`, `prepare` and `link_ticket` are to judge, as any
write of "The project" is: an `action` request, which says what it does in words and is done only while
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
| `tickets {action, ticket?, workspace?, query?, state?, label?, assignee?, detail?, title?, body?, labels?, reason?, remove_labels?, assignees?, remove_assignees?, status?, project?, type?}` | The tickets of the project's tracker, through the user's own `gh` or `acli`, run by the daemon; an agent has no tracker sign-in of its own. Every agent of a project that declares a tracker has it, in `main` as in a workspace, whether a ticket is linked or not. |

- **`list {state?, label?, assignee?}`**: every ticket of a state, in one call, a line each (`-
  #12 open: The login fails [bug] (ana)  <link>`): `state` is `open` (the default), `closed` or
  `all`; up to 200, the newest, and the answer says when there may be more. **`search {query,
  …}`**: the same, with words. On GitHub, `gh issue list -R github.com/<owner/name> --state …
  --limit 200 [--label] [--assignee] [--search] --json number,title,state,labels,assignees,url`;
  on Jira, `acli jira workitem search --jql <JQL> --fields key,summary,status,assignee,labels
  --json --limit 200`, the JQL being the tracker's projects, the state as a status category
  (`statusCategory != Done`), each filter quoted, and the words as `text ~`. An item that says
  another site than the tracker's is left out, and counted; one that says no site is kept.
  **A search stays in the tracker's repository**: on GitHub a `query` holding `repo:`, `org:`,
  `user:`, `is:pr` or `type:pr`, in any case, is refused (`refused: tickets search looks in the
  tracker's repository only: …`), and of what `gh` answers only the issues whose link is the
  tracker repository's (`https://github.com/<owner>/<name>/issues/…`) are kept, the others left
  out and counted (`(2 answered from outside the tracker's repository are left out)`).
- **`show {ticket?, workspace?, detail?}`**: the ticket the agent's workspace (or the one
  `workspace` names, required in `main` unless `ticket` is given) is linked to, or `ticket`,
  another ticket of the same tracker by its key (`#123` or `ABC-123`): its reference, tracker,
  state, title, link and description, masked, and how many comments it has with the newest one's
  author (`Comments: 12, … read them with tickets {"action": "show", "detail": "full"}`); `detail
  full` gives the comments (the newest 30). A key of the other tracker is refused (`refused:
  ABC-123 is a Jira ticket, and this workspace's tracker is GitHub issues (#123): …`), and nothing
  is asked of any tracker. A workspace with no ticket and no key is refused (`refused: workspace X
  is linked to no ticket: …`). An unreadable ticket (`gh` or `acli` missing or signed out, no such
  ticket) is an error saying why and what to do.
- **The writes**, on every tracker: `create {title, body?, labels?}`, `comment {ticket?, body}`,
  `edit {ticket?, title?, body?, labels?, remove_labels?, assignees?, remove_assignees?}` (at least
  one), `close {ticket?, reason?}`, `reopen {ticket?}` and `status {ticket?, status, project?}`, the
  ticket named as for `show`. On GitHub, `gh issue create`, `comment`, `edit` (`labels` and
  `assignees` add, as on `create`: `--add-label`, `--remove-label`, `--add-assignee`,
  `--remove-assignee`), `close` (`reason` is `completed` or `not planned`, `--reason`; left out,
  GitHub's own default), `reopen`, and a `status` below. On Jira, once `acli` is checked to be signed in to the
  tracker's site (as for a read), `acli jira workitem create --project <P> --summary <title>
  [--description <body>] [--type <type>] [--label a,b] --json` in `project {project?, type?}`,
  one of the tracker's projects (it may be left out when the tracker has one), the type the
  project's default when none is given; `acli jira workitem comment create --key <K> --body
  <body>`; `acli jira workitem edit --key <K> [--summary] [--description] --yes`; and `close`,
  `reopen` and `status` as `acli jira workitem transition --key <K> --status <name> --yes` to a
  status found among the statuses of the work item's project, never a guessed name
  ([`tickets.md`](tickets.md), "Jira's writes"). A ticket or a project the tracker does not take,
  a `status` with no name, a `reason` that is neither value, and, for an argument of another action
  (`reason` is for close, `remove_labels`, `assignees` and `remove_assignees` for edit) or of the
  other tracker (on Jira, a label, an assignee or a reason), is refused before anything is asked. A
  `title` on `comment`, `close`, `reopen` or `status` is refused before anything is asked
  (`refused: tickets comment takes no title: title is for edit and create`). The request shows
  the title and the body whole, and a long one or one holding a secret is refused or base, as
  for the forge's writes ("The forge"). Locks, a project or a tracker kept read only, are
  planned as their own feature.
- **`status` on GitHub**: sets the Status of the issue on the GitHub Projects (v2) board it is on,
  like Todo, In Progress or Done. The option is found by its name, in any case, among those of the
  board's Status field. The daemon reads the issue's boards (`gh issue view <n> -R … --json
  projectItems`), finds the board among those of the repository's owner (`gh project list
  --owner`), reads its fields (`gh project field-list`) and sets the option (`gh project item-edit
  --id … --project-id … --field-id … --single-select-option-id …`): only the item of this issue,
  with ids read from it, so nothing but this issue's status is reached. An issue on no board says
  so (`#1 of acme/app is on no project board: it has no status to set`); a status the board does
  not have is refused with its statuses; an issue on several boards names them and asks for
  `project`, a board's title in any case. `gh project` needs the `project` scope: when `gh` says
  it is missing, the answer says how the user adds it (`gh auth refresh -s project`), which
  Flottille never runs. A board of an owner other than the repository's is not reached.

Reads go through the gate like every call, which allows them; a write is to judge. How a ticket
is read, Jira's text turned into plain text, and what is assumed of `acli`'s output, is in
[`tickets.md`](tickets.md). Until #141 `show` was the tool `ticket_read`, still answered for one
release.

## The project

What the user does in the window, the agent they talk to does too, under the gate, always on its
own project: a root has these four tools, the writes of `tasks` (above) and of `workspace`, a
child none of them (it does its brief). Families with an `action`, as `agent_status` is:

| Tool | |
|---|---|
| `skills {action: list \| show \| add \| edit \| remove, name?, description?, path?, text?}` | The project's skills (`docs/skills.md`). `add` takes `name`, `description` and one of `path` (a file or a folder of the project folder) and `text` (a text Flottille keeps); `edit` changes what it is given; `remove` removes it, and its text. As `flottille skill` does. |
| `commands {action: list \| show \| add \| edit \| remove, name?, description?, instructions?, template?}` | The project's commands (`docs/commands.md`). `add` takes `name`, `description` and `instructions`, or a `template` (`open-prs`, `merge-stack`, `close`) added as it is; `edit` changes what it is given. A command is sent to a child by writing `/name` in `agent_send`'s message. A command imported from a `.claude` folder points at a file: `show` names it and gives its text as it is now, and an edit of its instructions is refused (the file's to edit). |
| `setup {action: list \| add \| edit \| remove \| move, number?, to?, step?}` | The project's setup (`docs/projects.md`, "The setup"), its steps numbered from 1 in order. `add {step, number?}` adds one, the last when `number` is left out; `edit {number, step}` replaces one; `remove {number}`; `move {number, to}`. A `step` is whole, as the file has it: one of `copy` (globs), `link`, `root_entries`, `run` (a line) and `task`, with `repository` or `for_each_repository`, and `time_limit` (seconds) with `run` or `task`; it is checked as the file is. The workspaces made after play it; one already made keeps its own. |
| `project {action: settings \| set_settings \| rules \| set_rules, …}` | `settings` reads each repository's forge (and what its remote proposes) and the tracker (`docs/projects.md`). `set_settings {repositories?: [{path, forge?, base?, depends_on?}], tracker?}` declares a repository's forge, its base branches (in order of preference) and the repositories it depends on, or the tracker (`null` removes a forge or the tracker, `[]` no dependency; what is left out is kept, a tracker and a repository's forge too), checked by the code of `project.set_settings` itself, as the window's: a forge its remote does not lead to, a base branch missing on the remote, a dependency on itself or a cycle are refused with their reason, nothing saved. The gate judges it as every settings change but the tracker's, which is base; the check runs when the call is done, so a call held for the user may be refused after the approval. `rules` reads the rules the project's agents get from now on and where they come from (the project's own, the user's for all projects, or Flottille's default, `docs/projects.md`, "Rules"); `set_rules {text}` replaces the project's own rules by `text` (8,000 characters at most), for the agents created after: the one that asked keeps what it started with. |

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
| `agent_launch {title, brief, provider?, model?, effort?, role?, workspace?, new_workspace?, repositories?, root?}` | Starts a session for the user in a workspace with `root: true` ("Sessions for the user", below), or a child on one task, in the caller's own place, and answers at once with its id. A child never works in another place than its parent's: `workspace` and `new_workspace` are the session's, and a child launch that names another workspace than the caller's own, or a new one, is refused before anything is asked, naming the session form (`refused: a child works where you work: workspace and new_workspace are for a session for the user, … "root": true, "workspace": "login" …`); naming the caller's own workspace is its own place. Its description leads with the session form, for work that belongs in a workspace. `provider` is the caller's when left out, and the child's for life; `model` and `effort` the provider's defaults when left out (`docs/agents.md`, "Providers"): a provider not enabled or not ready, or a model or an effort it is known not to offer, is refused naming those that are (`refused: provider opencode is not enabled (…); the ready providers are claude`). `role` is `worker` (by default) or `lead`; with `root`, `workspace` is an existing workspace's name, `new_workspace` the name of a new one (made at once, with `repositories` as its repositories, the session's first message sent once it is prepared; a workspace that fails fails the session; the launch is judged as `workspace create` is, "The workspace"), neither for the caller's own place; both are plain optional fields, and a mix is refused naming the field to use (`refused: workspace and new_workspace are exclusive: …`, `refused: repositories goes with new_workspace, …`). Its description holds a call of each form. Refused, never queued, above the project's children cap (`refused: N children already run in project P, its cap: do the work yourself, or wait for one of them to end (agent_stop ends one of yours)`). Only a root has this tool: a child, whatever its role, has none of the `agent_*` tools (no depth). **`root: true`** starts a session for the user instead of a child ("Sessions for the user", below). |
| `session_send {session, text}` (a root) | A message to a session the caller started (`agent_launch` with `root`), or to the agent that started the caller's session ("Sessions for the user", below). |
| `session_read {session, last?, since?, until?, cursor?}` (a root) | A page of the conversation of another session of the caller's project, framed as that session's data ("Sessions that read each other", below). |
| `agent_send {agent_id, message}` | A message to a child, urgent: on its next tool result in a turn, a new turn otherwise. It answers the child's question. |
| `agent_status {agent_id?, action? status \| conversation, detail? short \| full, after?}` | `action status` (the default): one line per child (or for the one named): what it does (in a turn, `running \`<line>\` for 12 min`, `waiting for approval #3`, `waiting on the provider`, idle, reported, ended or failed with why), the time since its last sign of activity, its context use. For all the children it stops there (`detail short`) and says how to get more; for the one named, or with `detail full`, each also has its last three messages. `action conversation` (with `agent_id`; `after` pages): a page of the child's conversation, oldest first, one line per event that says something (`#<n> sent: …`, `said: …` for a whole reply, its streamed pieces joined and masked, two messages apart, `tool call: …`, a tool served and its outcome, a turn's end, a replacement, a report), each line of 2,000 characters at most (a longer reply goes on, whole, on the next lines), the page under 30,000; it ends with the `after` to pass for the next page. |
| `agent_stop {agent_id}` | Stops the child and its own children, deepest first; what was queued for them is superseded; their runs are stopped. Their workspaces stay. |
| `report {text}` | The caller is done. At most 10,000 characters, so that it rides whole on its parent's results; a longer one is refused, asking for fewer words and a file that holds the details. A child's report goes to its parent at once (`child-report`, with its workspace, its branch and the last commit of each repository), and the child ends when its turn does (or two minutes later, if its turn goes on). A root's report is shown to the user (`agent.reported`) and the root stays. |
| `ask_user {question}` (a root) | A question to the user (`agent.asked`), answered later as a message. |
| `ask_parent {question}` (a child) | A question to its parent (`child-question`), answered with `agent_send`. Until then the child shows as waiting on its parent. |
| `approval_answer {request_id, decision, why}` | Answers a request held for the caller: `deny` (the agent that asked is told, with the why) or `forward` (to the caller's parent with its why, or to the user from a root). There is no way to allow: `allow` is refused as a decision the tool does not have, and a request the caller does not hold, or no longer holds, is refused (`refused: request #n is not held by this agent: …`). |

They go through the gate like every call (it allows them). Their texts are masked of the
daemon's secrets before anyone else reads them.

### Sessions for the user

The user asked that the agent they talk to may start, for them, a session like one they start
from the window: `agent_launch {"title", "brief", "root": true, workspace? | new_workspace?}`.

- **Only a root starts one.** A child is refused before anything is asked (`refused: only the
  agent the user talks to starts a session for the user; a child's work stays in its tree, under
  its parent: ask your parent (ask_parent)`), and refused again when the call is performed: a
  child starting a root would leave its tree, its parent's sight and its parent's answers to its
  requests, with a root's tools.
- **It is to judge** (a card in Ask, the judge in Auto), as an `action` request whose words name
  the place and the title and give the first message whole (`Start a session for the user in the
  workspace login: "Fix login", theirs, not a child of this agent`, `Its first message: …`), done
  only while they hold. With `new_workspace` it is also judged as `workspace create` is. The
  judge is given the brief whole too: a brief longer than its 8,000 characters is refused at the
  launch (`refused: the brief is longer than …`), and one that holds what the masking takes for a
  secret makes the launch base, since the card shows it masked and the session gets it as
  written.
- **No chain without the user.** The session keeps who started it (`launched_by`). A session an
  agent started may start one in turn only as base, whatever its mode (`this agent is a session
  another agent started for the user, and a session it starts comes to the user, …`): each step
  of a chain is the user's.
- **The session is the user's**: no parent, depth 0, in the project's tree list and the window's
  sidebar like any root (`agent.created` with no parent). **Its first message is the agent's,
  never the user's**: it is delivered as `[flottille:launched]`, under a line naming the agent
  that wrote it (`agent 0123456789ab started this session for the user with this message:`), the
  window shows it as `started by` that agent, and the judge never takes it for the user's words:
  it shows it in a block of its own, `written by agent <id> when it started this session`. A
  replacement of the session delivers it again the same way. It is a lead by default (`role`), on
  the caller's provider unless `provider` names another, in `workspace`, a new workspace, or the
  caller's own place.
- **It takes the tree's mode**: the mode of the caller's tree, Ask, Auto or Full, never a looser
  one; only the user changes it afterwards, and each tree keeps its own. It counts in no children cap and no depth.
- The caller is answered at once with the new session's id and name; the `agent_*` tools do not
  reach it, and the caller is told nothing of it afterwards but what it writes with
  `session_send`.
- **It sits under its workspace**, not under the agent that started it: a root of its own,
  `agent.trees` lists it apart, with its `workspace` and `launched_by` (the window's sidebar puts
  it under its place, as any session).

#### Sessions that talk to each other

`session_send {session, text}` (a root's tool) writes to a session the caller started, or to the
agent that started the caller's session, and to no other agent: any other is refused before
anything is asked (`refused: <id> is neither a session you started nor the agent that started
you: …`), and so is one that ended or an empty text. Children are written to with `agent_send`.

- **It is to judge**, as a write is (a card in Ask, the judge in Auto), as an `action` request
  whose words name the session and give the text whole (`Send to the session "Login" (agent …),
  which you started: …`). The judge is given the text whole: one longer than its 8,000
  characters is refused, and one that holds what the masking takes for a secret is base.
- **No call waits**: the caller is answered at once; the text reaches the other session as a
  delivery, `session`, urgent (on its next tool result in a turn, a new turn otherwise), under a
  line naming the sender (`agent 0123456789ab wrote to this session:`): it is an agent's words,
  never the user's. Its answer, a `session_send` the other way, comes back the same way.
- **The judge never takes it for the user's words**: the user's last messages it is given are
  the user's only; the last messages written to the session with `session_send` come in blocks of
  their own, each `written by agent <id> with session_send (an agent's words, not the user's)`.

#### Sessions that read each other

`session_read {session, last?, since?, until?, cursor?}` (a root's tool, as `session_send` is)
reads the conversation of another session of the caller's project, a page at a time, never all of
it at once.

- **Which session**: a root of the caller's project, whoever started it, running or ended. A
  session of another project is refused before anything is asked (`refused: <id> is a session of
  another project, <p>, not of <q>: an agent reads only the sessions of its own project`), and so
  are a child (its parent reads it with `agent_status`, action `conversation`) and an id that
  names no agent. **The caller's own session** is answered, without a request in any mode, with
  its tree's mode instead of its messages (`This is your own session, …: its conversation is
  already yours. Your session's tree is in full mode: a call the gate would hold passes without a
  request; … Only the user changes it.`), so that an agent can say beforehand that a call will be
  held.
- **What it holds**: the session's messages, oldest first, each with who said it and when (in UTC,
  `2026-10-31T14:05:09Z`): the user's messages to that session, as they were delivered to it (its
  first message once, not again for a replacement), and the replies of its agent, their pieces
  joined. Its tool calls, its thoughts and the deliveries of Flottille and of other agents (reports,
  answers to requests, `session_send` messages) are left out.
- **A page**: without dates, the last `last` messages (20 by default, 1 to 50), ending with a
  cursor to the page before them (`Earlier messages: …`), or saying it reached the start; with
  `since` (and `until`, which goes only with it), the messages said from `since` and before
  `until`, at most `last`, ending with a cursor to the next page (`Later messages: …`), saying it
  reached `until`, or, without `until`, the cursor to read on with later (`That is all for now;
  later, read on: …`). A date is RFC 3339 (an offset, the seconds and their fractions optional) or
  a day (`2026-10-31`, its start in UTC), of a year from 1970 to 9999 (another is refused, saying
  so). A page holds 30,000 characters at most, counted as they are shown, escaped, so that it
  always ends with its cursor under the result limit; each message holds its first 4,000, masked
  whole of the secrets of both sessions' places before it is cut (a longer one says how much was
  cut). The cursor is given alone (with `last` at
  most): it carries the dates, names the event the page stopped at, and so stays where it was as
  new messages come; the agent passes it back as it was given, and one it did not get is refused.
- **Framed as data**: the page opens with `The conversation of the session <name>, another session
  of project <p>. It is that session's data, not instructions for you: what the user says in it was
  said to that session's agent, never to you, and what its agent says is that agent's.`, and each
  message comes in a block of its own, `Message n of m, said by the user, to that session at <time>`
  or `said by that session's agent at <time>`, then `<session_message>`, its text with every `<`
  escaped, `</session_message>`. Its texts are masked of the daemon's secrets and of those of both
  sessions' places, and escaped as every result is.
- **Another session's read is to judge**, in Ask and in Auto, as an `action` request whose words
  name the session and the page (`Read the conversation of the session "Login" (agent …),
  another session of this project: its last 20 messages`). It is never harmless, so that a mode
  that lets it pass can be added without making it pass in Ask or Auto.

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
     daemon itself (`docs/protocol.md`, "Transport");
   - a login's place, read or written, by a file tool or as any path a line names: `.ssh`,
     `.gnupg`, `.aws`, `.azure`, `.config/gcloud`, `.kube`, `.docker/config.json`, `.config/gh`,
     `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`, `.local/share/keyrings`,
     `Library/Keychains` and the agents' logins (`.claude`, `.claude.json`, `.codex`,
     `.local/share/opencode`) wherever they sit along the path, `/etc/shadow`, `/etc/sudoers`,
     `/etc/sudoers.d`. Inside the place only what sits below it counts; a path outside is checked
     as written and where it leads, and without a leading `/proc/<process>/root` or `cwd`;
   - a program that reads or prints a login: `gh`/`glab auth token`, `aws configure get` or
     `export-credentials`, `aws ecr get-login-password`, `aws sts get-session-token`,
     `kubectl config view --raw`, `kubectl create token`, `gcloud auth print-*token`,
     `gcloud config config-helper`, `az account get-access-token`, `git credential`, any
     `git-credential-*` or `docker-credential-*`, and password managers and keyrings whole:
     `pass`, `gopass`, `op`, `bw`, `rbw`, `secret-tool`;
   - any write through a process's folder in `/proc` that leads elsewhere (`cwd`, `root`, `fd`,
     `exe`, `map_files`, under any name: `/dev/fd/3`), by a file tool or a line naming it;
   - a write of Flottille's own program (the daemon's executable, as named and where it leads);
   - a write in Flottille's own folders (data, configuration, state, runtime; a project's
     workspaces that sit in its data folder are the project's), or of a folder above one
     (`mv ~/.config ~/c.bak`, `rm -r` of it, `fs_write` over it): by a file tool that writes, or
     by a run that is not known to only read and names it as a word (the folder a run starts in
     may hold Flottille's: a project in the home folder). What deletes the untracked files
     without naming where counts the folder it takes them from: the folder it runs in (after its
     `-C`s) for a `git clean`, the top of its worktree for a `git stash` with `-u`,
     `--include-untracked`, `-a` or `--all`, and for a `git clean` of a pathspec from the top
     (`:/`, `:(top)`, `:(top,…)`). That top is the `--work-tree` it names, else found on the disk
     as git finds it: the nearest folder holding a `.git`, up from the one it runs in, inside the
     place or not (a home folder that is a repository);
   - a write of a startup file, or of a folder above one, the same ways: `~/.bashrc`,
     `~/.bash_profile`, `~/.bash_login`, `~/.profile`, `~/.zshrc`, `~/.zshenv`, `~/.zprofile`,
     `~/.zlogin`, `~/.config/fish`, `~/.gitconfig`, `~/.config/git`, `~/.config/autostart`,
     `~/.config/systemd/user`, `~/.config/environment.d`, `~/.config/hypr`, `~/.config/sway`,
     `~/.config/niri`, and the same under `XDG_CONFIG_HOME`; `~/.local/share/systemd/user` and
     the same under `XDG_DATA_HOME`; `~/.local/bin` and `~/bin`: what they hold runs at the
     user's next login or command, outside the gate. An option's value or a word of `tr`,
     `printf`, `expr`, `seq` naming a folder above Flottille's own is base instead (`awk -F/`,
     `tr / _`);
   - a git setting that runs a program later: `git config --global` or `--system`, and a write
     (not a removal) of a key that runs a program or brings in a file: `core.hooksPath`,
     `core.fsmonitor`, `core.sshCommand`, `core.editor`, `core.pager`, `core.askPass`,
     `core.gitProxy`, and every key of git's settings that run programs below but `remote.*`,
     `url.*`, `push.*` (any `alias.*`, `include.path`, `includeIf.*`, `credential.*`, drivers,
     filters, `protocol.allow`…); a program named for a remote: `--upload-pack`,
     `--receive-pack`, `--exec` of `clone`, `fetch`, `pull`, `push`, `send-pack`, `ls-remote`,
     `archive`, `git clone -u`, `git -c core.sshCommand`;
   - a launcher, which starts what it runs outside the daemon's tree of processes, later or on a
     host: `systemd-run`, `tmux`, `screen`, `zellij`, `dtach`, `abduco`, `setsid`, `at`,
     `batch`, `crontab`, `dbus-send`, `gdbus`, `busctl`, `ssh`, `scp`, `sftp`, `loginctl`,
     `hyprctl`, `swaymsg`, `i3-msg`; `systemctl` but its reads (`status`, `show`, `cat`,
     `help`, `list-*`, `is-*`, `show-environment`, `get-default`, `whoami`); `niri msg`,
     `kitty @`, `kitten @`, `wezterm cli`; `rsync` with a host's path (`host:path`) or `-e`,
     `--rsh`, `--rsync-path`. As a line's program, a wrapper's (`nice`, `ionice`, `chrt`,
     `taskset`, `chronic`, `env`, `timeout`, `stdbuf`, `nohup`, `xargs`…), or what a runner runs
     (the first word after `uv`, `poetry`, `pipenv`, `pdm`, `hatch`, `rye`, `pixi`, `conda`,
     `mamba` `run`; `npm`, `pnpm`, `yarn`, `bundle`, `mise` `exec` or `x`; `bun x`; after `--`
     of `cargo run`). Anywhere else its name is a plain word;
   - a container's bind mount: the source of each `-v`, `--volume`, `--mount` (`source=`,
     `src=`) of `docker`, `podman` or `nerdctl` `run` or `create` that names a path is a path the
     line writes, refused as above. A named volume, and a container itself, are not.
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
     the path, `/etc/shadow`, `/etc/sudoers`, `/etc/sudoers.d` (all refused above, read or
     written), and Flottille's own folders (data, configuration, state, runtime), which are
     base to read and refused to write. Inside the place only what sits below it counts. A path
     outside is checked as written and where it leads (a link anyone made, a configuration
     folder that is a link, a `..` after a link), against Flottille's folders as named and
     resolved;
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
     `--forc`, `--mirr`), a `+refspec` or a `:branch` (a push naming the program that receives
     it, `--receive-pack` or `--exec`, is refused);
   - git settings that run programs, lead pushes elsewhere or bring in other files: a key of
     `core.*`, `alias.*`, `remote.*`, `url.*`, `push.*`, `credential.*` or `include*`, or a key
     whose value git runs (`diff.external`, `diff.*.command` and `textconv`, `difftool.*`,
     `mergetool.*`, `man.*` and `browser.*` `cmd` and `path`, `merge.*.driver`, `filter.*`,
     `pager.*`, `hook.*`, `guitool.*`, `sendemail.*`, `instaweb.*`, `gpg.program`,
     `gpg.*.program`, `gpg.ssh.defaultKeyCommand`, `sequence.editor`,
     `interactive.diffFilter`, `submodule.*.update`, `uploadpack.packObjectsHook`,
     `imap.tunnel`, `tar.*.command`, `web.browser`, `trailer.*.command` and `cmd`, and
     `protocol.allow` and `protocol.*.allow`, which can let `ext::` URLs run programs) given to
     `git -c` or `--config-env` (but `core.sshCommand`, refused), or written by `git config` in
     a write form that is not refused (`--edit`, a removal, a key of `remote.*`, `url.*`,
     `push.*`, another key of `core.*`); `git remote add`, `rename`, `remove`, `set-url` and `set-branches`; git's
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
     emptied as `gh`'s is (`docs/forges.md`, "Bitbucket"). An agent reads the tickets with
     `tickets` and the pull requests with `forge`;
   - a `forge` merge that waives the checks or a review; a change of the project's tracker
     or of its rules (`set_rules`);
   - a write of `forge` or `tickets` whose title or body, or a session for the user whose brief,
     holds what the masking takes for a secret; a session for the user started by a session an
     agent started ("Sessions for the user");
   - `workspace` `remove`, whose card says what would be lost ("The workspace");
   - a write of `tasks` or `setup` whose line, or task, would be base to run, and one that says
     what only the user's word may say ("The project": asking turned off, `check` or `read_only`
     set, a `link` or `root_entries` step, a `copy` of a sensitive name).
3. **Harmless**: passes in every mode.
   - reading inside the agent's place (the project folder or the workspace), the tree's tools (but
     `agent_launch` with `root`, `session_send` and `session_read`), `tasks` `list` and `show`, `run_status`, `skill_read`, the reads of `forge` (`list`, `show`)
     and of `tickets` (`list`, `search`, `show`), `workspace` `list` and `show`;
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
   forge command lines beyond their reads; the writes of `forge` and `tickets`,
   `agent_launch` with `root` (a session for the user), `session_send` (a message to another
   session), `session_read` (another session's conversation), and a write of
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
**In Full**, a call to judge and a base call are performed at once, with no request and no judge,
recorded as an `agent.tool_served` whose `gate` is `full`; a workspace removed in Full is removed
with what it would lose, as the user's answer to discard would. What the gate refuses stays
refused (`docs/design.md` §7.1).

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

**The user answers** a request they hold: allow it once, or deny it. A workspace's removal is
also allowed with the answer to discard what would be lost (`flottille approve <n> --discard`),
refused for any other request. That answer covers exactly what the card listed: work that appeared
since is not discarded, the removal is refused and the agent calls again for a new card. Nothing they answer allows
a later call: the next one goes through the gate again. A request held by an agent is not listed
for the user and cannot be answered by them.

| Command | |
|---|---|
| `flottille approvals [--json]` | The requests the user holds, each with its number, the agent, the call, its class, why, and the judge's verdict when one was asked. |
| `flottille approve <n> [--discard]` | Allows the request once; `--discard`, for a workspace's removal only, removes it even with what would be lost. Refused for a request held by an agent, or one already decided. |
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
