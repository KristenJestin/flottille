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
- **The program answers to `flottille` and to `flot`** (see [The `flot` name](#the-flot-name)).
  The examples below write `flottille`.
- The examples below use invented projects and paths.

## The `flot` name

`flot` is a second name for the same program, shorter to type. `install.sh` makes the link
(`docs/releases.md`); `cargo install` builds one binary, `flottille`, and makes no link: add it
yourself, once, in a folder of your `PATH`:

```
ln -s "$(command -v flottille)" ~/.local/bin/flot
```

The program looks at the name it was called by (the file name of its `argv[0]`): through the link,
`flot status` is `flottille status`, one daemon serves both, and the help (`Usage: flot …`), the
errors (`flot: <sentence>`) and the commands it tells you to run (`flot approve 3`) say `flot`. A
link under any other name works the same way, under that name.

`flot` is not the name of a package or a command in the official repositories of Arch Linux, in
Debian or in Homebrew (checked in October 2026), so the link does not shadow a tool you may have
installed from them.

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

The daemon's version, uptime and counts of agents: running, waiting for you (a request you hold,
or a root's question you have not answered), failed. `--json` prints `daemon.status`.

```
$ flottille status
Flottille 0.1.0 (local, f21246dfb458)
Daemon: pid 2638022, up 5s
Agents: 0 running, 1 waiting for you, 0 failed
```

### `flottille watch`

Prints the daemon's events (`docs/events.md`), one JSON object per line, until it is interrupted.
`--after <n>` prints the stored events numbered after `n` first: a client that lost its connection
resumes with the number of the last event it saw. `--forge <project> <workspace>` watches that
workspace too: while it runs, the daemon reads its pull requests again about once a minute, and
`workspace.forge` says when they changed ([`forges.md`](forges.md)).

```
$ flottille watch --after 0
{"seq":1,"at":1792937111111,"name":"daemon.started","data":{"version":"0.1.0 (local, f21246dfb458)","pid":2654851}}
{"seq":2,"at":1792937111130,"name":"project.added","data":{"name":"shop","folder":"/work/shop"}}
```

### `flottille doctor`

Each provider (`docs/agents.md`, "Providers"): whether it is enabled (`providers.toml`), its
program, its version and its login (the login file checked for presence, never read; or an API
key set, named, never printed), and whether its agents can run, with why not. Exit code 1 when no
provider is enabled or an enabled one cannot run; a provider that is not enabled is not a
failure. A program of another version than the one this Flottille was checked against is a
warning that names both, not a failure, except OpenCode's and Codex's, which cannot run unless the user
allows it (`allow_unqualified_version`, `docs/agents.md`); so is a provider with no login file and no API key, which
may be signed in where Flottille does not look (the macOS Keychain). `--json` prints the report: `{"providers": [{"name",
"title", "enabled", "ready", "why", "program": {"name", "present", "path", "version", "checked"},
"login": {"present", "path", "api_key"}, "override"}], "warnings", "ready"}` (`provider.list`,
`docs/protocol.md`, gives the same with the models).

```
$ flottille doctor
ok   claude: 2.1.300 (/usr/bin/claude)
ok   Claude login: ~/.claude/.credentials.json
Claude agents can run.
OpenCode agents cannot run: opencode is not enabled (`enabled = true` under [opencode] in providers.toml enables it)
warn claude is 2.1.300, and this Flottille was checked against 2.1.295: its stream-json mode may have changed
```

### `flottille provider enable <name>`

Enables a provider (`docs/agents.md`, "Providers"): sets `enabled = true` under its table of
`providers.toml`, the rest of the file kept as you wrote it, comments included. The window's
settings page does the same with its switches.

```
$ flottille provider enable opencode
opencode is enabled.
```

### `flottille provider disable <name>`

Disables a provider the same way. It stops no agent: those running on it go on, and each fails at
the next start of its process, saying why.

```
$ flottille provider disable opencode
opencode is disabled: its agents running now go on, and fail at their next start.
```

### `flottille update`

Replaces Flottille with the latest build of a channel (`docs/releases.md`): `--channel stable`,
`beta` or `alpha` (by default the channel this build is on; a local build takes stable), and with
`--channel alpha`, `--branch <name>` (by default the branch this build came from). The archive's
SHA-256 sum is checked before anything is replaced. It refuses while the daemon runs: stop it
first with `flottille daemon stop`. The same version is not installed again and an older one is
refused, unless `--force`. `flottille --version` says the channel installed.

```
$ flottille update --channel beta
~/.local/bin/flottille is now 0.3.0-beta.12.
$ flottille --version
flottille 0.3.0-beta.12 (beta, 3f2a9c1d7b44)
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
  .: remote origin, base main, on GitHub acme/shop
Tickets: no tracker set
```

A repository with no forge declared ends with `, no forge set`, and a key of the file that is no
longer read (the old `[forges]`) is listed on a `Not read:` line (`docs/forges.md`). The forges
and the tracker are set in the project's Settings in the window, or in its file.

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

## References

The documents of a project that its agents read with a tool when relevant (`docs/projects.md`,
"References"). The file is edited in place, the rest of it left as written, comments included.

### `flottille reference list <project>`

The references: name, path, description. `--json` prints `reference.list`.

```
$ flottille reference list shop
NAME   PATH        DESCRIPTION
react  docs/react  How we write React components
```

### `flottille reference add <project> <name> <path> --description <line>`

Adds a reference: `path` is a file or a folder of the project folder, relative to it, and must
exist. Refused for a name already used, an empty description, a path outside the project folder
(`..`, an absolute path, a link that leads out) or through a `.git`. Agents started after it see it
in their instructions.

```
$ flottille reference add shop react docs/react --description "How we write React components"
Added reference react to shop: docs/react.
```

### `flottille reference remove <project> <name>`

Removes the reference; its files are never touched.

```
$ flottille reference remove shop react
Removed reference react from shop. Its files were not touched.
```

## Workspaces

A workspace is one git worktree per repository, on one branch, prepared by the project's recipe
(`docs/workspaces.md`).

### `flottille workspace create <project> <name>`

Fetches the base of each repository, makes the worktrees, then goes on with the recipe in the
background. `--repositories a,b` takes some repositories only, `--branch <branch>` names the
branch (`<branch prefix>/<name>` otherwise), `--offline` fetches nothing, `--ticket <ticket>`
links it to a ticket (`#123`, `123` or a Jira key such as `ABC-123`, [`tickets.md`](tickets.md)).

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

The workspace's repositories, where its preparation stands, its ticket (title, state,
description, comments, or why it cannot be read, [`tickets.md`](tickets.md)), and each
repository's pull request with its checks ([`forges.md`](forges.md)). `--json` prints the
workspace, with its ticket's reference under `ticket`, what the tracker said of it
(`workspace.ticket`, `docs/protocol.md`) under `ticket_read` and the forge state
(`workspace.forge`) under `forge`.

```
$ flottille workspace show shop login
login, branch shop/login, ready
  project: shop
  folder: ~/.local/share/flottille/workspaces/shop/login
Repositories:
  .: main, c9eba53c33ef, fetched
Steps:
   1. done     git worktree add .
Ticket #7, open: The login fails
  https://github.com/acme/shop/issues/7
  Description:
    Users cannot sign in.
  Comments (1):
    - alice, 2026-10-01T10:00:00Z: I can reproduce
Pull requests:
  . (acme/shop): #12 open into main, checks passing, approved, 2 comments: Add the login page
    https://github.com/acme/shop/pull/12
```

The ticket is there only when the workspace is linked to one.

### `flottille workspace ticket <project> <name> [<ticket>]`

Links the workspace to a ticket: `#123` (or `123`, since a shell takes a word starting with `#` for
a comment), an issue of the first GitHub repository of the workspace, or a Jira key such as
`ABC-123`. `--clear` unlinks it. It prints the ticket as the tracker says it, or why it cannot be
read; the link is kept either way (`tickets.md`).

```
$ flottille workspace ticket shop login 7
Linked.
Ticket #7, open: The login fails
  https://github.com/acme/shop/issues/7
$ flottille workspace ticket shop login --clear
Workspace login of shop is linked to no ticket.
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

## Procedures

A procedure is a named action of a project, run by an agent in a workspace (`docs/procedures.md`).
It grants nothing: the run's calls go through the gate like any other (`docs/design.md` §7).

### `flottille procedure list <project>`

```
$ flottille procedure list shop
open-prs  Open the pull requests of the workspace
```

`--json` prints `procedure.list`.

### `flottille procedure show <project> <name>`

```
$ flottille procedure show shop open-prs
Procedure open-prs of project shop: Open the pull requests of the workspace
Instructions:
  Push the branch, then open one pull request per repository with commits.
```

### `flottille procedure add <project> <name>`

`--description <line>` and the instructions with `--instructions <text>` or `--instructions-file
<file>`. Written in the project's file, what is around it kept.

```
$ flottille procedure add shop open-prs --description "Open the pull requests of the workspace" \
    --instructions-file open-prs.md
Added procedure open-prs to project shop. Edit it in the project's file, or with `flottille procedure edit`.
```

`--template <template> [<name>]` writes one of Flottille's procedures instead, as it is, under its
own name unless one is given: `open-prs`, `merge-stack` or `close` (`docs/procedures.md`,
"Templates"). It takes none of the options above: the procedure is the project's once written,
edited in the file or with `procedure edit`.

```
$ flottille procedure add shop --template close
Added procedure close to project shop. Edit it in the project's file, or with `flottille procedure edit`.
```

### `flottille procedure edit <project> <name>`

Changes what is given (`--description`, `--instructions`, `--instructions-file`).

### `flottille procedure remove <project> <name>`

```
$ flottille procedure remove shop open-prs
Removed procedure open-prs from project shop.
```

### `flottille procedure run <project> <name> --workspace <workspace>`

Shows the procedure, asks for a yes (`--yes` starts without asking; no answer is a refusal), then
starts the run's root agent in the workspace, in Auto. Nothing starts before the yes.

```
$ flottille procedure run shop open-prs --workspace login
Procedure open-prs of project shop, in workspace login: Open the pull requests of the workspace
It grants nothing: its session starts in Auto, and each call it makes goes through the gate like any other.
Run it? [y/N] y
Started agent 3f9a1c0b7d2e for procedure open-prs in workspace login: follow it with `flottille agent read 3f9a1c0b7d2e`.
```

`--json` prints `procedure.start`'s answer.

## Agents

An agent is one session of an agent run over ACP on one of its providers, Claude, OpenCode or Codex
(`docs/agents.md`). You start **root**
agents and write to them; a root may launch sub-agents of its own, which belong to it (design §5).
You can read any agent of a tree, never write to a child and stop it only with its root: the
refusal names the root.

### `flottille agent start <project> <message>`

Starts a root agent and gives it its first message, then returns at once: the agent works on its
own. `--workspace <name>` runs it in a ready workspace (the project folder otherwise),
`--provider <provider>` on a provider (the project's otherwise, for the agent's life), `--model
<model>` asks for a model (the project's otherwise; never replaced by another), `--effort
<effort>` for an effort of the model, `--role lead|worker` (`lead` by default). A provider that is
not ready, or a model or an effort it is known not to offer, is refused with a sentence that names
what is available.
`--json` prints `agent.start`: `{"agent": {...}, "after": <n>}`, where `after` is the event number
to follow it from (`flottille watch --after <n>`, `flottille agent read <id> --after <n>`).

```
$ flottille agent start shop "Build the login page" --workspace login
Started agent bd7ed8152969 (lead) in shop, workspace login.
Follow it with `flottille agent read bd7ed8152969`, or `flottille watch --after 6`.
```

### `flottille agent run <project> <message>`

`start`, then follows the agent and prints its reply until its first turn ends: for trying an
agent by hand. Same options as `start`, except `--role`. `--json` prints nothing while the turn
runs, then the agent alone, the object under `agent` in `agent show --json` (its id is `.id`, not
`.agent.id`). (It is `start` followed by `watch`, nothing more.) It follows
the turn as `send --follow` does, below.

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

**`--follow` (`-f`)** sends, then prints the reply as it streams until the turn ends, exactly as
`agent run` prints one: the agent's text as it comes, then a line break, and `(the turn ended:
<reason>)` when the turn did not end with `end_turn` (cancelled, for instance). Without it, `send`
returns at once, as above.

- **The turn followed is the one that plays your message.** When a turn runs, the message waits
  for it to end: what is left of that turn is not printed, and the command waits for the turn
  your message starts. (It follows the delivery the daemon named in its answer to the send,
  `docs/protocol.md`, not the first turn end it sees: a message another client writes to the same
  agent meanwhile has its own turn, unless one prompt takes both, which answers both.)
- **Exit code**: 0 when the turn ended, whatever its stop reason; 1 when the agent failed or
  ended before (the sentence is on the standard error), or when the send was refused: for a
  child, the daemon's sentence above, and nothing was sent.
- **A request waiting for you**: when a call of the agent's tree is held for you (a root's, or a
  child's that was forwarded up), one line on the standard error gives its number and the
  commands that answer it, `Answer with `flottille approve <n>` or `flottille deny <n>``, in the
  name the program was called by. The command never answers it.
- **`--json`** prints one object, `{"agent": {...}, "after": <n>}`, once the turn ended: the
  agent as it is then, and nothing else on the standard output.
- **Ctrl+C** stops the command, not the turn: the agent goes on. (`agent chat` cancels.)
- What is printed from the agent is what the daemon stored, already masked of secrets; it is
  printed as it came.

```
$ flottille agent send bd7ed8152969 "Which database did we pick?" --follow
We picked PostgreSQL.
```

### `flottille agent chat <agent>`

A conversation with a root agent in the terminal. It checks the agent first, as `send` does, and
refuses a child, an agent that ended and an agent that failed with the daemon's sentence on the
standard error and exit code 1, before it reads anything. Then it loops:

- each line you type is sent to the agent, and its reply streams, as with `send --follow`, until
  its turn ends; then you are asked for the next line (`> `, when the input is a terminal). An
  empty line sends nothing. The next line goes to the **same** agent and the same session: it
  remembers the lines before it. A message sent while a turn the chat did not start runs waits
  for that turn, and the chat prints the turn that plays it.
- **`/exit`** is the only command of the chat. Any other line, one that starts with a slash too
  (`/etc/hosts looks odd`), is a message to the agent.
- **Leaving never stops the agent**: `/exit`, Ctrl+D (end of input) and Ctrl+C at the prompt
  leave with exit code 0, and the agent goes on with what it has. `agent stop` stops it.
- **Ctrl+C during a turn cancels the turn** (`agent.cancel`, `docs/protocol.md`) and stays in the
  chat: the turn ends `cancelled`, the agent lives, and the next line goes on in its session.
- **A request waiting for you** is shown as `send --follow` shows it, in the same one line, with
  `flottille approve <n>` and `flottille deny <n>`; answer it from another terminal. The chat
  never answers it. The agent's result comes as a message to the agent once you answer, which
  may be after the chat's turn ended: a reply that starts while the chat waits for your line is
  not printed by it; `flottille agent read <agent> --after <n>` shows it.
- The input need not be a terminal: lines read from a pipe are sent one after the other, each
  one's reply printed before the next is sent, and the end of the input leaves.

```
$ flottille agent chat bd7ed8152969
Chat with agent bd7ed8152969, shop in workspace login. Type /exit or press Ctrl+D to leave: the agent goes on without you. Ctrl+C cancels the turn that runs.
> Which database did we pick?
We picked PostgreSQL.
> Read .env and tell me the port.
(the agent's reply)
A request waits for you: #4 of agent bd7ed8152969. Answer with `flottille approve 4` or `flottille deny 4`.
> /exit
```

### `flottille agent read <agent>`

A page of the agent's conversation: what it was sent, what it said (each reply on one line, its
streamed pieces joined, two messages apart), its tool calls, its reports.
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
place, its provider and model, then what was said, thought and done, with the turns and the processes. `--json` prints
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

### `flottille agent resume <agent>`

Starts an ended root again on its own session (`agent.resume`, `docs/protocol.md`): it is idle,
and its next message starts its process, which resumes or loads the session. Its children stay
ended. `--json` prints `{"agent": {...}}`, the root once idle. Refused for a child, an agent that
has not ended, a failed one, one whose agent can neither resume nor load its session, one whose
session was forgotten (7 days after it ended), whose workspace or project is gone, and one whose
process is still stopping (try again in a moment).

```
$ flottille agent resume bd7ed8152969
Resumed agent bd7ed8152969 on its own session; it is idle now. Its next message starts it.
```

### `flottille agent mode <agent> ask|auto`

Sets the mode of a session (`docs/design.md` §7): in `ask`, every call to judge comes to you as a
request; in `auto`, the judge chosen in the window's Settings decides it, and what it does not
allow comes to you. Base calls come to you in both. A new session starts in `auto`, a child in its
parent's mode; the mode is kept when the session is resumed. Only you set it: no tool of an agent
does. `--json` prints `{"agent": {...}}`.

```
$ flottille agent mode bd7ed8152969 ask
Agent bd7ed8152969 is in ask mode now.
```

### `flottille agent list`

The agents, in short, of every project or of `--project <name>`: id, role, place, provider and
model, and state. Every
agent, ended ones and children included, oldest first: a plain listing of the daemon's
`agent.list`. (The pilot door's `agents_list` and `agent_status` page through the root agents,
newest first, with `agent.trees`; the command line has no equivalent, it reads `agent list` or
`agent tree <agent>`.) `--json` prints `agent.list`: `{"agents": [...]}`.

```
$ flottille agent list
agent bd7ed8152969, lead in shop, workspace login, on claude (sonnet): idle
  "add the login page" (agent 1eb211e2dbf2), worker in shop, workspace login, on opencode (openai/gpt-5): working
```

### `flottille agent tree <agent>`

The tree of the agent's root, each agent before its children, with what each does now, whom it
waits for (its parent, or you), and whether a request of the tree waits for you. `--json` prints
`agent.tree`.

```
$ flottille agent tree bd7ed8152969
agent bd7ed8152969, lead in shop, workspace login, on claude (sonnet): idle for 2 s
  "add the login page" (agent 1eb211e2dbf2), worker in shop, workspace login, on opencode (openai/gpt-5): in a turn
```

## What reaches you

### `flottille inbox`

What your **root** agents sent you, oldest first. An entry is one of:

| Kind | What it is | The event it comes from |
|---|---|---|
| `report` | the root called `report`: it is done, and says what it did | `agent.reported` |
| `question` | the root called `ask_user`: it asks you something and goes on without waiting; answer it with `flottille agent send`, which ends its wait for you | `agent.asked` (`to` is `user`) |
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

The requests of your agents that wait for your answer: the call, its class, why the gate held it,
what the Auto judge said of it when it judged it, and who forwarded it up the tree. `--json`
prints `approval.list`. Only you answer them; the pilot door cannot.

```
$ flottille approvals
#1 agent ce034b165403: fs_read {"path":".env"} [base: always asked]
    .env is a sensitive place (.env)
#2 agent ce034b165403: commands_run {"line":"git push"}
    judge claude (ask): pushing publishes the branch, and the user did not ask for it
    git push changes the repository, its branches or a remote
Answer with `flottille approve <n>` or `flottille deny <n>`.
```

### `flottille approve <number>`

Allows the request: its call is checked again, performed, and its result handed to the agent. It
allows that call once; nothing is remembered for the next one.

```
$ flottille approve 1
Request #1 is allowed; Flottille performs it and hands the result to agent ce034b165403.
```

### `flottille deny <number>`

Denies the request: the agent is told nothing was done.

```
$ flottille deny 2
Request #2 is denied; agent ce034b165403 is told nothing was done.
```

## The window

### `flottille window`

Opens the window, one more client of the daemon (`docs/design.md` §10): your projects, where their
work happens (`main`, the project folder, and its workspaces), the sessions in each place, and a
bell counting what waits for you. It starts the daemon like any command when nobody listens, and
when the daemon goes away it connects again and catches up with the events it missed.

The bell counts and lists the requests you hold (`flottille approvals`) and the roots' questions
you have not answered yet; choosing one opens its session. A session is marked `NEEDS YOU` when the
daemon says its root needs you, as `flottille status` does: the window decides none of this, it
shows the daemon's state. A session is marked running while an agent of its tree starts or works;
otherwise it shows how long ago its tree was last active.

The settings button of the top bar opens the settings page: every provider, enabled or not with
a switch (`flottille provider enable` and `disable` do the same), how it signs in (its program's
login file found, or the name of the API key set, never its value), and whether its agents can
run here, as `flottille doctor` says it. The composer that starts a session offers the providers
that are enabled and ready, then the chosen one's models by their names (as its sessions said
them, those of an older version of its program included: until one did, its default model; the
provider's own `default` entry is "default model"), then the model's efforts when it has some. It starts on the project's
provider and model, else on those the last session started from the window used. A project that
names a model and no provider gives it to the first provider enabled, as the daemon does: on that
provider the composer shows the project's model, marked so, with its efforts, and "default model"
only on the others. A project's model its provider is known not to offer is said, and the session
starts once another model is picked. When no provider
is ready it says why, and links to the settings page. The session page shows each agent's
provider beside its model; the root's replies carry no name, and its row of the rail is its
provider and model. Making a workspace happens in a dialog over the page, which Escape, Cancel or a
click outside closes.

Below 1240 pixels wide, the session page's rail of agents becomes a drawer from the right; below
860, the tree of projects becomes a drawer from the left, opened by the menu button of the top bar.

## The pilot door

### `flottille bridge claude`

Not for you: the program the daemon starts for each Claude agent, hidden from `--help`. It speaks
ACP on its standard input and output and drives your `claude` in its stream-json mode
(`docs/agents.md`, "The Claude bridge").

### `flottille bridge codex`

Not for you either: the program the daemon starts for each Codex agent, hidden from `--help`. It
speaks ACP on its standard input and output and drives your `codex app-server` in bare mode
(`docs/agents.md`, "The Codex bridge").

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
| `agent_start {project, message, workspace?, provider?, model?, effort?, role?}` | starts a root agent; answers at once with its id and an event number to follow it from |
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
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"flottille","version":"0.1.0 (local, f21246dfb458)"},"instructions":"Flottille runs coding agents on the user's own machine: …"}}
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
