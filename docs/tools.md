# Tools

The tools of the agents Flottille runs, and the MCP server that serves them: the agent door of
`docs/design.md` §1. It follows `docs/reference/hemera.md` §3, with the rules of
`docs/reference/mcp.md` §4 where they differ. A session lists the tools of its agent, which
depend on what it can do (`docs/agents.md`): the file and command tools below for every agent,
`reference_read`, then `forge_read`, `ticket_read`, `forge_write` and `workspace_remove` for an
agent in a workspace,
`procedure` and `project` for a root (below, "The project"),
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
and before it is performed: `commands_status` as much as the file tools. A
path outside the place is asked for; the tools check again that a path they resolve is inside,
and refuse it with `refused: X is outside this agent's place: …`, unless the user allowed the call.
A path is resolved this way:

- relative to the place's top, or to a repository's folder when the call names `repository`
  (one of the project's repositories by its path, else
  `refused: the project has no repository X: it has …`); `~` is the home folder;
- `.` and `..` are taken on the text first: a path that is not under the place as written is
  outside, and the disk is not looked at;
- then the real path: every link is followed, at most 32; a missing file is its real folder and
  its name, and a dangling link leads to its target. A link that leads out is outside; anything
  that cannot be resolved is outside;
- containment is by segments: `..notes` is a name inside, `../x` a step out.

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
Files are opened without waiting, so that even one swapped for a pipe meanwhile is refused.

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
renamed over it: a link is replaced, never written through. The file's folder is checked to be
inside the place on the disk after it is made and again just before the rename, so that a folder
swapped for a link to the outside meanwhile is refused (`refused: X: its folder is no longer
inside this agent's place …`, nothing written). A check is not a lock: a swap between the last
check and the rename is not seen. The file's folder is checked to be
inside the place on the disk after it is made and again just before the rename, so that a folder
swapped for a link to the outside meanwhile is refused
(`refused: X: its folder is no longer inside this agent's place …`, nothing written). A check is
not a lock: a swap between it and the rename is not seen.

## References

| Tool | Arguments | |
|---|---|---|
| `reference_read` | `name`, `path?`, `range? {offset?, length?}` | One of the project's references (`docs/projects.md`, "References"), read-only. A file reference is read by its name alone, a page at a time exactly as `fs_read` does (32 KiB at most, `range` to go on). A folder reference answers with its files and their sizes (`.git`, links, files of secrets and sensitive places left out, 1000 at most); `path`, relative to the reference, then names the file to read, or a folder of it to list. |

- **The list is the only door**: a name not in the agent's list is refused with `refused: no
  reference named X: the project's references are a, b`; the reference is looked up in the
  project's file as it is now.
- **Nothing outside the reference is served**, as `fs_read` serves nothing outside the place: a
  path that steps out of the reference with `..`, an absolute path, `~`, and a link that leads out
  once followed are refused with `refused: X is outside the reference Y: it serves its own files
  only`; a path through a `.git` or naming a file of secrets (`.env`, `.env.*`) is refused; a
  `path` for a file reference is refused.
- **What `fs_read` holds for the user, a reference never serves**: a path in a sensitive place
  (The gate, 3: `.ssh`, `.netrc`, `.npmrc`, `.git-credentials`…), by the gate's own rule taken
  from the project folder, links followed, is refused with `refused: X is in a sensitive place
  (…), which a reference never serves: read it with fs_read, which asks the user first`.
- **The call goes through the gate** like every other and is allowed: it names a reference, not a
  path of the place, and reads from the project folder whatever the agent's place. The result is
  masked like a file read, and recorded as an `agent.tool_served` event.
- **It grants nothing**: unlike `fs_read`, reading a reference does not let the session write the
  file.

## Commands

| Tool | Arguments | |
|---|---|---|
| `commands_list` | none | The project's catalogue (`docs/projects.md`), one line each: `name: line`, and where it runs, whether it keeps running, is a check, changes nothing, is held for approval. |
| `commands_run` | `command` or `line` (exactly one), `repository?`, `timeout? 1..600`, `why?` | Runs a catalogue command (`refused: the catalogue has no command X: list them with commands_list`) or one line, in the repository's folder (the catalogue's, or the one named) or the place's top. Its description holds a call of each form: `{"command": "test"}`, `{"line": "cargo test -p core", "repository": "api", "timeout": 120}`. |
| `commands_status` | `run`, `action? output \| stop`, `tail? 1..1000`, `start?` | `action output` (the default): the run's state and lines of its output, the last `tail` (40 by default), or `tail` lines from line `start`. `action stop`: stops the run and everything in its process group, `SIGTERM`, then `SIGKILL` 5 s later, and answers once it ended; `tail` and `start` are refused with it. |

- **No shell, ever.** A line is split as `flottille_core::command` does (the recipe's `run` steps
  too): words on whitespace, a quoted part kept whole, a backslash literal. A newline, `|`, `&`,
  `;`, a backtick, a redirection, `$(`, `${`, `$VAR` or `%VAR%` outside single quotes, and `*`,
  `?`, a leading `~` or `[…]` outside any quotes are refused with: `Flottille runs a command
  without a shell, and “T” is shell syntax: put it in a script of the repository and run the
  script.` A line naming a shell as its program is split like any other; the gate judges the
  line a shell runs with `-c` (below).
- **Waiting**: the call waits `timeout` seconds (30 by default) for the end. A catalogue command
  that keeps running (`serve`) answers at once, and one already running for the agent is joined.
  A call stopped by its client stops its run.
- **The answer**: `run <id>: <name> <state> after <s> s` (`is running`, `exited with code N`,
  `was killed by signal N`, `was stopped`, `was interrupted…`), then, while it runs, `it keeps
  going: read it with commands_status {"run": "<id>"}, stop it with commands_status {"run": "<id>", "action": "stop"}`, then `(lines a-b of N…)` with
  how to read the lines before or after (`commands_status {"run", "start"}`), then the lines. A
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
    refused anyway).
- **A push is the gate's**, not the environment's: `git push` is a call to judge, a forced one
  base, and a push from a workspace is refused, before git runs, when a repository's remote no
  longer leads where it did when the workspace was made (`docs/procedures.md`, "Where the remote
  leads"). A script the agent wrote and runs is not read (below, "Known gaps").
- **A stop ends the run's process group, not every descendant.** `commands_status` with `action stop`, a timeout of
  the call's client and the daemon's stop signal the group the run was spawned in (`SIGTERM`,
  then `SIGKILL` after 5 s), and what the run left in its group is killed when its own process
  ends. A process that left the group (`setsid`, a daemon that forks and detaches) escapes all
  of this and keeps running; Flottille does not stop it. It stays the daemon's descendant,
  though: the daemon is the subreaper of what it starts, so a process orphaned there is taken in
  by the daemon rather than by init, waited for when it ends, and refused if it connects to the
  socket.
- **Runs are records**: the store's `runs` (with the end of their output once ended), and the
  `run.started` and `run.ended` events. Each run's process group is recorded while it runs: a
  daemon started after a crash kills the groups a previous one left (only while their leader is
  the process it started) and marks their runs interrupted. A daemon that stops stops its runs,
  interrupted. When a run's own process ends, what it left in its group is killed with it.

## The forge

| Tool | |
|---|---|
| `forge_read {repository?, detail? short \| full}` | The pull request of the workspace's branch in each of its repositories, or the one named: number, state, base, link, review, checks with their states, and how many comments and reviews it has (`comments: 2, read them with forge_read {"detail": "full"}`); `detail full` gives their texts, cut and under the result limit. Read only, through the user's own `gh`, run by the daemon; an agent has no forge sign-in of its own. A repository the workspace does not take is refused (`refused: X is not a repository of this agent's workspace: it has …`). Only an agent in a workspace has it. |

| `forge_write {action, repository, title?, body?, draft?, waive?}` | Opens the pull request of the workspace's branch (`pr-create`, with the project's reviewers), merges it by squash (`pr-merge-squash`; refused while its checks fail or run or a review asks for changes, unless `waive` gives the user's words, which holds the merge for their approval) or re-runs the failed jobs of its last commit's pipelines (`pipeline-rerun`), in one of the workspace's repositories, through the user's own `gh` run by the daemon (`docs/procedures.md`, "`forge_write`"). Only an agent in a workspace has it. Its description holds a call of `pr-create` and of `pr-merge-squash`. |

`forge_read` goes through the gate like every call, which allows it; a `forge_write` call is to
judge, and one that waives the checks base.

## The workspace

| Tool | |
|---|---|
| `workspace_remove {}` | Removes the agent's own workspace as `flottille workspace remove` does (`docs/workspaces.md`, "Removal"): refused, nothing touched, while a repository holds uncommitted work or commits on no remote, or while another agent than the caller's tree works in the workspace or a command run not the tree's own runs there (`refused: workspace … is not removed, nothing was touched: the agent …, outside this agent's tree, works in it…`), each named; then deletes, in each repository, the workspace's branch when it holds no commit of its own (every commit of it is in its base's tracking ref), and keeps the others. Only an agent in a workspace has it, for its own workspace; each call is to judge. |

 How the forge is read, and what it never touches (a token,
`gh auth token`), is in [`forges.md`](forges.md).

## The tickets

| Tool | |
|---|---|
| `ticket_read {ticket?, detail? short \| full}` | The ticket the agent's workspace is linked to, or `ticket`, another ticket of the same tracker by its key (`#123` or `ABC-123`): its reference, tracker, state, title, link and description, masked, and how many comments it has with the newest one's author (`Comments: 12, … read them with ticket_read {"detail": "full"}`); `detail full` gives the comments (the newest 30). Read only, through the user's own `gh` or `acli`, run by the daemon; an agent has no tracker sign-in of its own, and none of either's write commands is reachable from it. The tracker is that of the workspace's ticket, or GitHub's (its own repositories' issues) when it has none; a key of the other tracker is refused (`refused: ABC-123 is a Jira ticket, and this workspace's tracker is GitHub issues (#123): …`), and nothing is asked of any tracker. A workspace with no ticket and no key is refused (`refused: workspace X is linked to no ticket: …`). An unreadable ticket (`gh` or `acli` missing or signed out, no such ticket) is an error saying why and what to do. Only an agent in a workspace has it, whether a ticket is linked or not. |

It goes through the gate like every call, which allows it. How a ticket is read, Jira's text
turned into plain text, and what is assumed of `acli`'s output, is in [`tickets.md`](tickets.md).

## The project

What the user does in the window, the agent they talk to does too, under the gate, always on its
own project: a root has these two tools, a child none (it does its brief). Two families with an
`action`, as `agent_status` is:

| Tool | |
|---|---|
| `procedure {action: list \| show \| start, name?, workspace?}` | `list` and `show` read the project's procedures (`docs/procedures.md`). `start` runs one in a workspace, the agent's own when `workspace` is left out (one in the project folder must name one), as a new root of its own, in Auto: the same as the window's confirmation. Refused, nothing asked, for a procedure or a workspace the project has not. |
| `project {action: settings \| set_settings \| add_reference \| remove_reference, …}` | `settings` reads each repository's forge (and what its remote proposes), the tracker and the references (`docs/projects.md`). `set_settings {repositories?: [{path, forge}], tracker?}` declares a repository's forge or the tracker (`null` removes it; a tracker left out is kept), checked as `project.set_settings` checks them; `add_reference {name, description, path}` and `remove_reference {name}` edit the references as `flottille reference add` and `remove` do. The file is written where it stands, its comments kept. |

- **Reads run at once.** `procedure` `list` and `show`, `project` `settings`.
- **Every write is to judge** (a card in Ask, the judge in Auto), as any call is (answered at
  once that it waits, the result later as a delivery). Its request is an `action`: it says what
  it changes in words, one sentence each (`Start the procedure ship in workspace login: …`;
  `Set the tracker to Jira on acme.atlassian.net (ABC)`; `Add the reference style
  (docs/STYLE.md): …`), allowed once, and it is done only while those words still hold: a
  procedure whose instructions (`Its agent is told: …`) changed meanwhile is not started
  (`Request #n (procedure) was not executed: what it does changed since the user allowed it; it
  would now: …`). A change of the tracker is base: nothing checks it against the project, as a
  forge is checked against its remote.
- **What an agent never does**: change a session's mode or choose the judge (no tool has them),
  or reach another project (`project` is not an argument). An agent's reference is never the project
  folder itself nor a repository's root (`.`, `api`, or a link to one): what a whole checkout
  holds is not a document to read.
  Such an argument is refused as one that does not fit the tool. A write that cannot be done (an
  unknown procedure, a reference outside the project folder, a repository the project has not) is
  refused before anything is asked.

## The tree's tools

The tools through which agents launch other agents and answer for them (`docs/design.md` §4.3,
§5; `docs/agents.md` "The tree"). `agent_id` is the id `agent_launch` gave; the `agent_*` tools
reach only the caller's own children, and are refused with `refused: <id> is not one of your
children: agent_status lists them` for any other agent.

Two of them take an `action`, as the tools of a family do: `agent_status` (`status` or `conversation`) and `commands_status` above.

| Tool | |
|---|---|
| `agent_launch {title, brief, provider?, model?, effort?, role?, new_workspace?, repositories?}` | Starts a child on one task and answers at once with its id. `provider` is the caller's when left out, and the child's for life; `model` and `effort` the provider's defaults when left out (`docs/agents.md`, "Providers"): a provider not enabled or not ready, or a model or an effort it is known not to offer, is refused naming those that are (`refused: provider opencode is not enabled (…); the ready providers are claude`). `role` is `worker` (by default) or `lead`; `workspace` is an existing workspace's name, `new_workspace` the name of a new one (made at once, with `repositories` as its repositories, the child's brief sent once it is prepared; a workspace that fails fails the child), neither for the caller's own place; both are plain optional fields, and a mix is refused naming the field to use (`refused: workspace and new_workspace are exclusive: …`, `refused: repositories goes with new_workspace, …`). Its description holds a call of each form. Refused, never queued, above the project's children cap (`refused: N children already run in project P, its cap: do the work yourself, or wait for one of them to end (agent_stop ends one of yours)`) and past the depth limit. |
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
of the line a run would start, the catalogue command it names, the paths the call reaches, the
project's never list, Flottille's own folders and whether the workspace's branch is on a remote,
and answers with the call's class, or a refusal. The class does not depend on the mode: the mode
says who answers a call to judge (`docs/design.md` §7.1).

1. **Refused**, with a sentence saying why and what to do instead (under 2,000 characters): what
   cannot be made as it is, in every mode.
   - shell syntax in a line (`commands_run` splits a line without a shell, and refuses what it
     cannot split), and a call whose arguments do not fit the tool;
   - a login printed: `gh auth token`, `glab auth token`, `auth status` with `--show-token` (`-t`);
   - `flottille` itself: it drives the daemon, and agents act only through their tools. A copy
     of it under another name, or any program that speaks to the socket, is refused by the
     daemon itself (`docs/protocol.md`, "Transport").
2. **Base**: a card for the user in every mode, never the judge's, with a strong warning.
   - the project's `never` list: an entry matches a line's words from any position (its first
     word is a program, in any folder), or a catalogue command by its name; and a run when the
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
     configuration, state, runtime). Inside the place only what sits below it counts;
   - a recursive deletion that would take a repository: `rm` with `-r`, `-R` or `--recursive`,
     and `find` with `-delete` (or `-exec rm -r`), when a target, resolved from the folder the
     run starts in, is the place's folder, a repository's, or a folder above one (`rm -rf .`,
     `rm -R <repository>`, `find . -delete`). Deleting inside a repository is not (`rm -r
     target`);
   - a push forced or that deletes: `--force`, `-f`, `--force-with-lease`, `--force-if-includes`,
     `--mirror`, `--delete`, `-d`, `--prune`, a `+refspec` or a `:branch`;
   - rewriting history that is on a remote, or the user's own checkout's: `git rebase`,
     `git commit --amend` (and the fixups that amend or reword), `git reset <commit>`,
     `git update-ref`, when the workspace's branch is already on a remote or the agent works in
     `main`; `git filter-branch` and `filter-repo` anywhere;
   - discarding work in the project folder (`main`): `git checkout` with `--`, `.`, `-f` or
     `--force`; `git restore`, `git clean` and `git reset`, in any form; `git stash` but `list`
     and `show`; `git worktree remove`; `git branch -d`, `-D` or `--delete`;
   - publishing: `npm`, `pnpm`, `yarn`, `bun`, `cargo`, `poetry`, `flit`, `hatch`, `uv` with
     `publish` or `unpublish`, `twine upload`, `gem`, `dotnet` and `nuget` with `push`;
   - `acli` and `bkt`, Atlassian's and Bitbucket's command lines, every line, reads included:
     they run with the user's own sign-in, in a place no variable of ours moves, so it cannot be
     emptied as `gh`'s is (`docs/forges.md`, "Bitbucket"). An agent reads a ticket with
     `ticket_read` and the pull requests with `forge_read`;
   - a `forge_write` merge that waives the checks or a review; a change of the project's tracker.
3. **Harmless**: passes in every mode.
   - reading inside the agent's place (the project folder or the workspace), the tree's tools,
     `commands_list`, `commands_status`, `reference_read`, `forge_read`, `ticket_read`;
   - writing inside a workspace (`fs_write`, `fs_edit`, and `git add`, `mv`, `rm` there);
   - a line the rules know to only read, inside the place: the reads of git (`status`, `log`,
     `diff`, `show`, `branch` and `tag` listings, `config` and `remote` reads…), of `gh` and
     `glab` (`pr view/list/diff/checks/status`, `issue view/list`, `run view/list`, `mr
     view/list/diff`, `ci view/list/status`, `repo view`, `auth status`, `api` with GET, no
     field, no input and not `graphql`), and the common reading programs;
   - a catalogue command in a workspace, not marked `ask_before_running` and whose program is not
     a shell. A line is the catalogue command it is when the catalogue holds the same words where
     it runs.
4. **To judge**: everything else, a card in Ask and the judge in Auto. Among them: writing in the
   project folder (`main`); a path outside the place, or one that cannot be resolved; a catalogue
   command in `main`, marked `ask_before_running`, or whose program is a shell; a line the rules
   do not know to only read; `git commit`, `git push`, `git rebase` of a branch on no remote,
   `git reset --hard` in a workspace, `git -c` or `--config-env`, `git remote` and `git config`
   writes; the forge command lines beyond their reads; `forge_write`, `workspace_remove`, and a
   write of `procedure` or `project` ("The project").

   The rules of 1 and 2 are judged on the line and on every program a wrapper runs, unwrapped in
   turn (16 at most): `env` (its options, `NAME=value` words and `-S` line), `sudo`, `doas`,
   `nice`, `nohup`, `timeout`, `time`, `stdbuf`, `command`, `exec`, `xargs`, `npx` (and its `-c`
   line), `pnpm dlx`/`exec`, `yarn dlx`/`exec`, `bunx`, `uvx`, and a shell (`sh`, `bash`, `zsh`,
   `dash`, `ksh`, `mksh`, `ash`, `fish`) with `-c`, whose line is split as `commands_run` splits
   one and judged in turn: a line that cannot be split is refused. A shell running a script is
   not unwrapped: the script is not read.

   **Known gaps**, which no command gate closes: a script written with `fs_write` and then run
   (`./script.sh`, `sh script.sh`) is not read, so what it does is out of the gate's reach; a git
   alias in the user's own configuration (`alias.<name> = push`) is run by git itself, and the
   gate sees only the alias's name. Running a script the agent wrote is itself a line to judge.

The words of a line that name paths (every word but the program, an option's value after `=`, a
`NAME=value`'s value) are resolved from the folder the run starts in, as a file tool's path is.

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
