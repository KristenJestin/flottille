# The command line

`flottille` is a client of the daemon's socket (`docs/protocol.md`): every command asks the daemon
what it needs, and starts it in the background when nobody listens. Nothing here reads Flottille's
database. A desktop shell, a status bar or a script integrates Flottille through these commands,
and an agent running outside Flottille drives it through [`flottille mcp`](#the-pilot-door).

- **`--json`**: the commands that take it print the daemon's answer as one JSON object (the shape
  is the method's, in `docs/protocol.md`), and nothing else, on the standard output. Without it
  they print a short text for a person.
- **Exit code**: 0 when the command did what it was asked, 1 when it did not. The reason is one
  sentence on the standard error, `flottille: <sentence>`: the sentence of the daemon's refusal
  when it refused. Nothing was done then.
- **Agents are named by their id**, twelve hexadecimal digits, as `agent list` shows them.
- The examples below use invented projects and paths.

## The daemon

### `flottille daemon`

Runs the daemon in the foreground, as a user service would. Any other command starts one in the
background by itself, its output in `$XDG_STATE_HOME/flottille/daemon.log`. A second daemon finds
the lock taken and refuses to start.

### `flottille daemon stop`

Stops the running daemon, and its agents with it.

```
$ flottille daemon stop
The daemon stopped.
```

### `flottille status`

The daemon's version, uptime and counts of agents: running, waiting for you (a request you hold),
failed. `--json` prints `daemon.status`.

```
$ flottille status
Flottille 0.1.0 (f21246dfb458)
Daemon: pid 2638022, up 5s
Agents: 0 running, 1 waiting for you, 0 failed
```

### `flottille watch`

Prints the daemon's events (`docs/events.md`), one JSON object per line, until it is interrupted.
`--after <n>` prints the stored events numbered after `n` first: a client that lost its connection
resumes with the number of the last event it saw.

```
$ flottille watch --after 0
{"seq":1,"at":1792937111111,"name":"daemon.started","data":{"version":"0.1.0 (f21246dfb458)","pid":2654851}}
{"seq":2,"at":1792937111130,"name":"project.added","data":{"name":"shop","folder":"/work/shop"}}
```

### `flottille doctor`

Whether Claude agents can run here: `claude`, its version, and its login (the login file checked
for presence, never read; or `ANTHROPIC_API_KEY` set, never printed). Exit code 1 while something
is missing. A `claude` of another version than the one this Flottille was checked against is a
warning that names both, not a failure. `--json` prints the report: `{"claude": {"present",
"path", "version", "checked"}, "login": {"present", "path", "api_key"}, "warnings", "ready"}`.

```
$ flottille doctor
ok   claude: 2.1.300 (/usr/bin/claude)
ok   Claude login: ~/.claude/.credentials.json
warn claude is 2.1.300, and this Flottille was checked against 2.1.295: its stream-json mode may have changed
Claude agents can run.
```

## Projects

A project is a folder registered with its configuration (`docs/projects.md`).

### `flottille project add <folder>`

Detects the repositories of the folder, shows them and asks you to confirm; `--yes` registers
without asking, `--name <name>` names the project (a slug of the folder's name otherwise).
Nothing is written in the folder.

```
$ flottille project add /work/shop --name shop --yes
Project shop, folder /work/shop
1 repository:
  .
Each takes the remote origin and the base branch main; edit ~/.config/flottille/projects/shop.toml to change that.
Registered project shop. Its configuration is ~/.config/flottille/projects/shop.toml.
```

### `flottille project list`

```
$ flottille project list
NAME  FOLDER      REPOSITORIES
shop  /work/shop  1
```

`--json` prints `project.list`: `{"projects": [...], "errors": [...]}`.

### `flottille project show <name>`

One project's configuration, every default filled in. `--json` prints the project.

```
$ flottille project show shop
shop
  folder: /work/shop
  branch prefix: shop
  workspaces root: ~/.local/share/flottille/workspaces/shop
  workspaces take: all repositories
  children at once: 3
Repositories:
  .: remote origin, base main
```

### `flottille project check <name>`

Detects the repositories again and says how they differ from the configuration, changing nothing.

```
$ flottille project check shop
Project shop: the repositories match the configuration.
```

### `flottille project remove <name>`

Forgets the project: its configuration file, never its folder.

```
$ flottille project remove shop
Removed project shop. Its folder was not touched.
```

## Workspaces

A workspace is one git worktree per repository, on one branch, prepared by the project's recipe
(`docs/workspaces.md`).

### `flottille workspace create <project> <name>`

Fetches the base of each repository, makes the worktrees, then goes on with the recipe in the
background. `--repositories a,b` takes some repositories only, `--branch <branch>` names the
branch (`<branch prefix>/<name>` otherwise), `--offline` fetches nothing.

```
$ flottille workspace create shop login
Workspace login of shop, branch shop/login, in ~/.local/share/flottille/workspaces/shop/login
  .: main, c9eba53c33ef, fetched
Ready.
```

### `flottille workspace list [project]`

```
$ flottille workspace list
PROJECT  NAME   BRANCH      STATE
shop     login  shop/login  ready
```

### `flottille workspace show <project> <name>`

The workspace's repositories and where its preparation stands. `--json` prints the workspace.

```
$ flottille workspace show shop login
login, branch shop/login, ready
  project: shop
  folder: ~/.local/share/flottille/workspaces/shop/login
Repositories:
  .: main, c9eba53c33ef, fetched
Steps:
   1. done     git worktree add .
```

### `flottille workspace prepare <project> <name>`

Resumes a preparation from the step that failed.

```
$ flottille workspace prepare shop login
Nothing to resume: workspace login is ready.
```

### `flottille workspace remove <project> <name>`

Removes a workspace whose work is all committed and pushed; never forced. The message names what
stands in the way otherwise.

```
$ flottille workspace remove shop login
Removed workspace login of shop: its worktrees and its folder. Its branches are kept.
```

## Agents

An agent is one session of a Claude agent run over ACP (`docs/agents.md`). You start **root**
agents and write to them; a root may launch sub-agents of its own, which belong to it (design §5).
You can read any agent of a tree, never write to a child and stop it only with its root: the
refusal names the root.

### `flottille agent start <project> <message>`

Starts a root agent and gives it its first message, then returns at once: the agent works on its
own. `--workspace <name>` runs it in a ready workspace (the project folder otherwise), `--model
<model>` asks for a model (never replaced by another), `--role lead|worker` (`lead` by default).
`--json` prints `agent.start`: `{"agent": {...}, "after": <n>}`, where `after` is the event number
to follow it from (`flottille watch --after <n>`, `flottille agent read <id> --after <n>`).

```
$ flottille agent start shop "Build the login page" --workspace login
Started agent bd7ed8152969 (lead) in shop, workspace login.
Follow it with `flottille agent read bd7ed8152969`, or `flottille watch --after 6`.
```

### `flottille agent run <project> <message>`

`start`, then follows the agent and prints its reply until its first turn ends: for trying an
agent by hand. Same options as `start`, except `--role`. `--json` prints the agent once the turn
ended, as `agent show --json` does. (It is `start` followed by `watch`, nothing more.)

### `flottille agent send <agent> <message>`

Writes to a root agent. The message is played after the turn that runs, if one does, and starts
the agent's process again when it was let go. `--json` prints `{"agent": {...}, "after": <n>}`.
Refused for a child (only its parent writes to it), for an agent that ended or failed, and for an
empty message.

```
$ flottille agent send bd7ed8152969 "Use PostgreSQL."
Sent to agent bd7ed8152969: it plays after the turn that runs, if one does.
Read what follows with `flottille agent read bd7ed8152969 --after 21`.

$ flottille agent send 1eb211e2dbf2 "Do it differently."
flottille: agent 1eb211e2dbf2 is a child of agent bd7ed8152969: only its parent writes to it; the user writes to and stops its root, agent bd7ed8152969
```

### `flottille agent read <agent>`

A page of the agent's conversation: what it was sent, what it said, its tool calls, its reports.
Any agent of a tree. A page is at most 30,000 characters; its end says how to read on, with
`--after <n>`. `--json` prints `agent.read`: `{"agent", "name", "lines", "after", "more"}`, where
`more` says whether the page stopped before the end.

```
$ flottille agent read bd7ed8152969
The conversation of agent bd7ed8152969:
#11 sent: [flottille:user]
Build the login page
#12 tool call: agent_launch
#18 said: I launched a worker for the login page.
#19 turn ended (end_turn)
That is all for now; later, read on with `flottille agent read bd7ed8152969 --after 19`.
```

### `flottille agent show <agent>`

The agent, and its conversation in full as events (`docs/events.md`): the agent's state and
place, then what was said, thought and done, with the turns and the processes. `--json` prints
`agent.show`: `{"agent", "events"}`, the agent with its instructions and tools, and its first
1,000 events.

### `flottille agent stop <agent>`

Stops a root agent and its whole tree, the deepest first. Their workspaces stay. `--json` prints
`{"agent": {...}}`, the root once ended. Refused for a child, and for an agent that is in a final
state.

```
$ flottille agent stop bd7ed8152969
Stopped agent bd7ed8152969 and its tree; it is ended now. Its workspace stays.
```

### `flottille agent list`

The agents, in short, of every project or of `--project <name>`: id, role, place and state. Every
agent, ended ones and children included, oldest first: a plain listing of the daemon's
`agent.list`. (The pilot door's `agents_list` and `agent_status` page through the root agents,
newest first, with `agent.trees`; the command line has no equivalent, it reads `agent list` or
`agent tree <agent>`.) `--json` prints `agent.list`: `{"agents": [...]}`.

```
$ flottille agent list
agent bd7ed8152969, lead in shop, workspace login: idle
  "add the login page" (agent 1eb211e2dbf2), worker in shop, workspace login: working
```

### `flottille agent tree <agent>`

The tree of the agent's root, each agent before its children, with what each does now, whom it
waits for (its parent, or you), and whether a request of the tree waits for you. `--json` prints
`agent.tree`.

```
$ flottille agent tree bd7ed8152969
agent bd7ed8152969, lead in shop, workspace login: idle for 2 s
  "add the login page" (agent 1eb211e2dbf2), worker in shop, workspace login: in a turn
```

## What reaches you

### `flottille inbox`

What your **root** agents sent you, oldest first. An entry is one of:

| Kind | What it is | The event it comes from |
|---|---|---|
| `report` | the root called `report`: it is done, and says what it did | `agent.reported` |
| `question` | the root called `ask_user`: it asks you something and goes on without waiting; answer it with `flottille agent send` | `agent.asked` (`to` is `user`) |
| `failure` | the root cannot go on: its process could not start, its session could not open, an option it was asked is not one it offers, it was replaced too often (twice in 30 minutes), or its new workspace could not be prepared (`docs/events.md` also lists "its parent failed", which only concerns a child) | `agent.failed` |

A child's report, question or failure goes to its parent, never to the inbox. The requests a gate
holds for you are not here: `flottille approvals` lists them.

The inbox is a view of the stored events (`docs/events.md`), not a store of its own: nothing is
marked read. Without `--after`, it lists the last `--limit` entries (50 by default); with
`--after <n>`, the first `--limit` entries after the event number `n`, so that a client reads on
from the last `seq` it saw. `--json` prints `inbox.list`: `{"entries": [{"seq", "at", "kind",
"agent", "project", "workspace", "text"}]}`, `text` being the report, the question or the reason.

```
$ flottille inbox
#33 question of agent 298a0599b45c (shop):
    Which database should the app use?
Answer a question with `flottille agent send <agent> <message>`; `flottille inbox --after <n>` lists what came since.
```

### `flottille approvals`

The requests of your agents that wait for your answer: the call, why the gate held it, and who
forwarded it up the tree. `--json` prints `approval.list`. Only you answer them; the pilot door
cannot.

```
$ flottille approvals
#1 agent ce034b165403: fs_read {"path":".env"}
    .env is a sensitive place (.env)
Answer with `flottille approve <n>` (`--for-tree` for the same call from now on) or `flottille deny <n>`.
```

### `flottille approve <number>`

Allows the request: its call is checked again, performed, and its result handed to the agent.
`--for-tree` allows the same call, exactly, for the whole tree of the agent's root from now on;
never for a sensitive place.

```
$ flottille approve 1
Request #1 is allowed once; Flottille performs it and hands the result to agent ce034b165403.
```

### `flottille deny <number>`

Denies the request: the agent is told nothing was done.

```
$ flottille deny 2
Request #2 is denied; agent ce034b165403 is told nothing was done.
```

## The pilot door

### `flottille bridge claude`

Not for you: the program the daemon starts for each Claude agent, hidden from `--help`. It speaks
ACP on its standard input and output and drives your `claude` in its stream-json mode
(`docs/agents.md`, "The Claude bridge").

### `flottille mcp`

Serves an MCP server on its standard input and output, for an agent running outside Flottille (a
coding agent in a terminal) to drive Flottille: it starts root agents, writes to them, reads every
agent of their trees and follows what happens. It is a client of the daemon's socket, like every
command here, and starts the daemon when nobody listens. It is not meant to be run by hand: the
outside agent's client runs it (see below).

The door serves these tools, always in this order. Agents are named by `agent_id`.

| Tool | |
|---|---|
| `projects_list` | the projects: name, folder, repositories |
| `workspaces_list {project?}` | the workspaces and their state |
| `workspace_create {project, name, repositories?, branch?, offline?}` | makes a workspace; its preparation goes on in the background |
| `agent_start {project, message, workspace?, model?, role?}` | starts a root agent; answers at once with its id and an event number to follow it from |
| `agent_send {agent_id, message}` | writes to a **root** agent; refused for a child, naming its root |
| `agent_status {agent_id?, ended?, before?}` | what an agent does now, its last messages; without `agent_id`, the **live** root agents, newest first, 20 at a time (`ended: true` adds the ended and failed ones; a page that does not hold them all ends with the call that reads on, with `before`) |
| `agent_read {agent_id, after?}` | a page of the conversation of **any** agent of a tree, with how to read on |
| `agent_stop {agent_id}` | stops a **root** agent and its tree; refused for a child, naming its root |
| `agents_list {project?, agent_id?, ended?, before?}` | the agents as trees, with what each does: the live roots with their trees, newest first, 20 at a time and paged like `agent_status`; `agent_id` shows the one tree of that agent's root |
| `approvals_list` | the requests that wait for the user |
| `events_since {after, wait_seconds?}` | the events after a number, oldest first, with the number to ask next |

`events_since` is how an outside agent follows without polling. With `wait_seconds` (at most 90,
under Claude Code's two minutes after which a call moves to the background) and nothing new, the
call waits and returns **as soon as one event arrives**; it answers empty (`No event after <n>
yet…`) at the end of the wait. A result holds at most 100 events and stays under 40,000
characters, every long text of an event cut; it ends with the number to ask next.

What the door **never** does: give the outside agent Flottille's file or command tools, or answer
an approval. A request the gate holds stays the user's, whatever asks: `approvals_list` shows what
waits, and the user answers with `flottille approve` or `flottille deny`.

Both protocol revisions are served, 2025-11-25 (with `initialize`) and 2026-07-28 (with
`server/discover`), and no state lives in the protocol session. The tool list is fixed. Every tool
has a title, a short description and annotations (`readOnlyHint` on the reads, `destructiveHint`
on `agent_stop`). Results are text, under 40,000 characters, and errors under 2,000, a sentence
saying what to do next; what an agent said is escaped, so that it never forges a Flottille marker.
The server's instructions (under 4,096 characters) tell an outside agent what Flottille is and
when to reach for these tools. The tools are not marked `anthropic/alwaysLoad`: the outside agent
finds them through tool search, which is what the instructions are for.

```
$ printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"demo","version":"1"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"projects_list","arguments":{}}}' \
  | flottille mcp
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"flottille","version":"0.1.0 (f21246dfb458)"},"instructions":"Flottille runs coding agents on the user's own machine: …"}}
{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"No project is registered yet: the user registers one with `flottille project add <folder>`.\n"}],"isError":false}}
```

#### Adding the door to a terminal Claude Code

```
claude mcp add --scope user flottille -- flottille mcp
```

The first `flottille` is the name of the server: its tools appear to the agent as
`mcp__flottille__agent_start` and so on. What follows `--` is the command Claude Code runs,
`flottille mcp`; the `--` keeps Claude Code from reading `mcp` or a later flag as its own. The
scope says where the entry lives:

- `local` (the default): you, in this project only;
- `user`: you, in every project (the right one for Flottille, which is not tied to a project);
- `project`: written in the project's `.mcp.json` and shared with whoever clones it. Not for
  Flottille: the entry names a program installed on your machine.

`claude mcp list` shows the entry, `claude mcp remove flottille` takes it out. `flottille` must be
on the `PATH` Claude Code runs with; otherwise give its full path after `--`.

**Tool search.** Claude Code does not load an MCP server's tools into the agent's context at
the start: it defers them, and the agent finds the ones it needs through a search step
(`ENABLE_TOOL_SEARCH`, on by default). So the outside agent first sees only the server's
instructions (the first 4,096 characters of them are read), which say what Flottille is and
when to look for its tools; it then searches, finds `agent_start` or `events_since`, and
calls them. That keeps the context light, and is why the door does not ask for its tools to
be loaded upfront. Set `ENABLE_TOOL_SEARCH=false` in the environment of Claude Code to load
all eleven tools at the start instead: each costs 100 to 500 tokens on every request, and the
agent no longer has to search for them.
