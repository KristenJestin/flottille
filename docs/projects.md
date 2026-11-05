# Projects

A project is a folder registered with Flottille. Flottille keeps its configuration in one TOML
file per project, `$XDG_CONFIG_HOME/flottille/projects/<name>.toml` (`~/.config/flottille/projects/`
by default). The file is the user's to edit; the daemon is the only program that writes it.
**Nothing is ever written in the project's folder or its repositories**, and removing a project
removes that file only.

The daemon validates a file whenever it reads it and refuses one it cannot use with its file and
its key, for example `projects/app.toml: repositories[2].depends_on: unknown repository "x"`.
Unknown keys are refused too, so a typo is never silently ignored.

## Commands

| Command | |
|---|---|
| `flottille project add <folder> [--name <name>] [--yes]` | Detects the repositories, shows them, asks for a yes on the standard input (`--yes` skips the question; no answer at all is a refusal) and registers the folder. |
| `flottille project list` | The projects. Fails, naming the file and the key, when a file cannot be used. |
| `flottille project show <name>` | One project's configuration, every default filled in. |
| `flottille project remove <name>` | Forgets the project, never its folder. |
| `flottille project check <name>` | Detects the repositories again; fails when they differ from the configuration. |

The project page lists the project's repositories (`project.repositories`): each with the branch
its checkout is on, where its remote leads or "local only", and its forge; the repositories the
folder holds that the file does not name, detected again, each with Add; a field that adds one by
its path; Detect again; and Remove, which asks first and only forgets the repository: its folder,
its branches and the worktrees of its workspaces stay, and the window says so. Adding and removing
write the file where it stands, its comments kept (`project.add_repository`,
`project.remove_repository`), and are refused, nothing written, when the file would no longer be
one the daemon can use.

**The branch prefix** starts the branch of a workspace that is given none: `<branch prefix>/<name>`.
It defaults to a slug of the project's name, and a file that does not hold the key keeps that. It
may be empty (`branch_prefix = ""`): the branch is then the workspace's name alone, never
`/<name>`, for a team whose branches follow the tickets (`fix/ABC-12-…`), which an agent gives
with `branch` (`docs/workspaces.md`). Only a space is refused (`branch_prefix: must not hold
spaces`). The project's Settings, Setup tab, "Branch prefix", edits it, an empty field included
(`project.set_settings`, `branch_prefix`); the workspaces that exist keep their branch.

A click on a repository's row in the project's Settings opens its dialog: its base branches (a list
in order of preference, moved, removed and added there), the other repositories it depends on (one
switch each) and its forge. The remote is git's, shown read-only. Saving asks `project.set_settings`,
which refuses, with its reason and nothing written, a base branch the remote does not have, a
dependency on the repository itself or a cycle; the refusal stays in the dialog and a save is a
toast. The workspaces created after a save take the new bases; the ones already made keep theirs.
An agent does the same with the `project` tool's `set_settings` (`docs/tools.md`): the same daemon code
checks it, so the same refusals come back to it, and the gate judges it like the other settings changes.

The window does the same from the sidebar's **+** beside "Projects" (or the empty window's "Add a
project"): a folder, typed or picked, the repositories detected in it shown before anything is
registered, then the new project's Settings, where it can be checked again or removed.

All of them take `--json` and print what the daemon answered (`docs/protocol.md`).

## Where the workspaces go

A project's workspaces are made in `<root>/<workspace name>` (`docs/workspaces.md`). The root is, in
this order:

1. the project's own `workspaces_root`, when its file has one;
2. the user's default for every project, `workspaces.toml` of Flottille's configuration folder;
3. Flottille's data folder, `$XDG_DATA_HOME/flottille/workspaces/<project>`, when there is no file.

```toml
# $XDG_CONFIG_HOME/flottille/workspaces.toml
place = "beside"                      # "data" (default), "beside" or "own"
path = "~/Worktrees/<project>"        # with place = "own" only, and then needed
```

- `data`: `$XDG_DATA_HOME/flottille/workspaces/<project>`. Flottille never writes beside a user's
  folder unasked, so this stays the default.
- `beside`: `<parent of the project's folder>/<project>-work`, the project's name, not its folder's.
- `own`: the folder typed. `<project>` is replaced by the project's name and a leading `~` by the
  home folder, **when the folder is resolved**: the file keeps what was typed. The result must be
  absolute, and a relative one is refused naming its key (`path`, or `workspaces_root` in a
  project's file).

A project's `workspaces_root` takes the same writing as `own`. A file that cannot be followed
(`workspaces.toml` with a word it does not know, or a `path` that would be refused when set, see
below) does not hide the projects: the data folder is followed, the Settings section shows the
error, and so do the project's page (on the line of the folder that results) and `project show`.

A folder for workspaces is refused, naming the reason, when it is:

- inside one of the project's repositories (a workspace is a checkout of it); for a default, the
  project it lands in is named;
- inside Flottille's configuration folder, or its data folder (but the data folder's own
  `workspaces/`, the default, is allowed);
- the home folder itself;
- a default for every project (`own`) without `<project>`: the projects would share one root.

`..` is folded before the folder is compared; a link is not followed. A default edited by hand is
judged on its own (`<project>`, Flottille's folders, the home), not against each project's
repositories: that is checked when it is set from the window.

`project add` writes no `workspaces_root`, so a new project follows the default of the day. At its
start the daemon drops from a project's file a `workspaces_root` that is exactly
`$XDG_DATA_HOME/flottille/workspaces/<project>`: older versions of `project add` wrote it, and pinned
it would hide the default from that project. It resolves the same under the data default, a folder
of any other name is kept, and the existing workspaces keep their folder from their record.

**A change applies to the workspaces made after it.** A workspace's record holds its folder: the
existing ones stay where they are and nothing is moved, whichever way the default or the root
changes.

The window sets both: **Settings**, Workspaces tab, "Workspaces folder" (the default for every
project, `workspaces_folder.set`), and a project's Settings, Setup tab, "Workspaces folder" ("Use
the default" or a folder of its own), which shows the folder that results. `flottille project show` prints that folder as
`workspaces root`.
`workspaces.toml` is edited through the window or by hand; the command line has no command for it.

## Detecting repositories

The folder itself, if it holds a `.git`, and the folders one or two levels down that do. A `.git`
may be a folder or a file (a worktree or a submodule). Hidden folders (`.claude`, `.cache`) and
links are not looked into. A folder with no repository is refused. `project add` writes each
repository with the remote `origin` and the base rule `["main"]`; the user edits the file for the
rest.

## The file

```toml
# The name is the file's name: this is projects/app.toml. A `name` key is allowed if it says the same.
folder = "/work/app"                  # absolute
branch_prefix = "app"                 # default: a slug of the name; "" for none: the branch is the workspace's name
workspaces_root = "/work/app-work"    # absolute (`~` and `<project>` allowed); default: the user's, see "Where the workspaces go"
default_repositories = "all"          # "all", or a list of the repositories a workspace takes
max_children = 12                     # children running at once, 1 to 12
run_time_limit = 1800                 # seconds a run step of the setup may take; default 30 min
never = [["git", "push"], ["rm", "-rf"]]   # programs with their leading arguments, one word per element, for the gate to refuse
provider = "claude"                   # the provider of its agents that name none; default: the first one enabled
model = "sonnet"                      # the model of its agents on that provider that name none; default: the provider's

[[repositories]]                      # at least one
path = "."                            # relative to the folder; "." is the folder itself
remote = "origin"                     # default
base = ["develop", "main"]            # the first that exists on the remote wins; default ["main"]
depends_on = ["lib-core"]             # paths of repositories of the project, leaves first; no cycle
forge = { kind = "github", repository = "acme/app" }   # its forge (docs/forges.md); none when left out
delete_branch_on_merge = false        # "Delete the branch on merge": a merge by `forge` deletes the source branch on the forge; default false

[variables]                           # name -> templated value, for every command run in a workspace
STORE = "{workspace.path}/.store"

[tasks."api:dev"]                     # the tasks: one command line each, never run by a shell
line = "npm run start:dev"
repository = "app-api"                # where it runs; the workspace folder when left out
serve = true                          # keeps running
check = false                         # tests, lint
read_only = false                     # changes nothing
ask_before_running = false            # held for approval each time an agent runs it

[[setup]]                             # the steps that prepare a new workspace, in order
copy = [".env", ".env.*"]             # globs, copied from the main checkout, never over a file
for_each_repository = true            # in each repository of the workspace, leaves first

[[setup]]
root_entries = true                   # the project folder's other entries, linked

[[setup]]
run = "npm install"                   # one line, its words templated, never run by a shell
for_each_repository = true

[tracker]                             # where its tickets are (docs/tickets.md); none when left out
kind = "jira"
site = "acme.atlassian.net"
projects = ["ABC", "OPS"]

[skills.style]                        # a text its agents read when their task needs it (docs/skills.md)
description = "The code style"
path = "docs/STYLE.md"                # a path of the project folder; none: a text Flottille keeps

[commands.open-prs]                   # instructions sent to a session as /open-prs (docs/commands.md)
description = "Open the pull requests"
instructions = "Push the branch, then open one pull request per repository."

message_checks = ["pnpm commitlint --edit {message}"]   # run on an agent's messages (below); none by default

[pull_requests]                       # how its pull requests are opened (docs/commands.md)
title = "type(TICKET): subject"       # the title's format, told to the agent that writes it
reviewers = ["alice", "bob"]          # asked to review each pull request Flottille opens
```

**Delete the branch on merge** is a switch of each repository, in the project's Settings page and in
its file, off by default. On, a `forge` `merge` of that repository's pull request deletes its source
branch on the forge once merged, unless the merge says `delete_branch: false` (`docs/tools.md`, "The
forge"). It never deletes a branch that existed before its workspace was made (the workspace
resumed it) nor one another workspace of the project is on; the answer says it was left, and why.
It is a project setting only: an agent's `project set_settings` does not set it.

The repositories are put in dependency order (a repository after those it depends on; among those
that are ready, the first of the file first) by `Project::in_dependency_order()`, which preparing
a workspace uses, and in dependency levels by `Project::dependency_levels()`, which a command
sent in a workspace is told for merging a level at a time.

`provider` must be one Flottille knows (`claude`, `opencode`, `codex`) and `model` must not be empty; which
providers are enabled is the user's, in `$XDG_CONFIG_HOME/flottille/providers.toml`, beside the
projects (`docs/agents.md`, "Providers"). An agent keeps the provider and model it started with:
editing them changes the agents started later only.

The variables are templates filled for each workspace (`docs/workspaces.md`): the configuration
checks that their names are those of environment variables and that their templates name only
`{workspace}`, `{workspace.path}`, `{project}`, `{branch}` and `{repository}` (`{{` and `}}` are a
literal brace).

A repository's `forge` says where its pull requests and issues are, and nothing else does: a
repository with none is never read nor written on a forge (`docs/forges.md`, "The forge of a
repository"). It is one of:

| `kind` | Keys | |
|---|---|---|
| `github` | `repository = "owner/repo"` | GitHub, through `gh` |
| `bitbucket-cloud` | `repository = "workspace/repo"` | Bitbucket Cloud, through `bkt` |
| `bitbucket-data-center` | `host = "git.example.com"`, `repository = "PROJ/repo"` | a Bitbucket Data Center server, through `bkt` |

`[tracker]` says where the project's tickets are (`docs/tickets.md`): `kind = "github"` with
`repository`, the path of one of the project's repositories whose forge is GitHub (its issues), or
`kind = "jira"` with `site` (the Jira Cloud site, `acme.atlassian.net`) and `projects`, the keys of
its Jira projects (at least one). A project with no `[tracker]` takes no ticket.

The forges and the tracker are set in the project's **Settings** in the window (the project page's
header): each repository's forge is pre-filled from its remote's address, nothing is active until
Save, and Save writes the file where it stands, its comments kept. A forge whose repository is not
the one its remote leads to (another host, another path) is refused with the reason, and nothing
is saved.

The `[forges]` table of earlier versions (Bitbucket Data Center hosts by name) is no longer read:
a file that still has it is accepted, the table is ignored and said (`project show` lists it under
"Not read", the window under the settings), and its repositories have no forge until one is set.
Nothing is converted on its own.

A command's keys and what sending it does are in [`commands.md`](commands.md), a skill's in
[`skills.md`](skills.md); `flottille command` and `flottille skill` (`add`, `edit`, `remove`)
write them in the file where it stands, keeping what is around them, and so do the project's
Settings in the window, sections Skills, Commands, Tasks and Setup.

**Former keys.** Until #127 the tasks were `[commands.<name>]`, the commands
`[procedures.<name>]`, the skills `[references.<name>]` and the setup `[[recipe]]`, a step naming a
task with `command`. They are still read for one release, and the daemon renames them in the file
at its start, in place: every comment and everything else the user wrote stays, the file is
replaced at once, and it is written only when it can be used afterwards (a file that could not be
used for another reason stays as it is). A name under both a former key and its new one is
refused, with both keys named, and the migration moves neither: the daemon says so in its log,
for the user to remove the one they do not want.

## What agents do on their own

The agent the user talks to (a root) does what the window does, through the tools `skills`,
`commands` and `project` (`docs/tools.md`, "The project"): it reads the project's skills, commands
and settings at once,
and every change it makes is a call to judge, said in words: a card for the user in Ask, the
judge's in Auto (`docs/design.md` §7). A change of the tracker, which nothing checks against the
project, is base: it always comes to the user. An agent never reaches another project's file.

**Former keys.** `[agent_actions]`, which let a family of these changes run on its own, a
command's former `grants` and `children`, and a task's `added_by` (a line the user once allowed
always) are no longer read: a file that still has them is accepted, `project show` lists the
first three under "Not read", and the daemon removes them from the file at its start, keeping everything else
the user wrote.

## Rules

An agent's instructions hold two texts about conduct. **What Flottille enforces** is fixed
(`crates/flottille/roles/base.md`): it says what the gate does, so that an agent does not take it
for its own discipline: the three classes, the mode (Ask, Auto or Full, one per tree, the user's alone)
and the judge, what always comes to the user, what is refused, and that a refusal or a held
request is an answer, never to be reached another way. A test checks each sentence against the
gate's answers (`tests/rules.rs`). **How to work** is the user's: commits only when asked, say what you did,
and whatever else they want.

| Level | Where it lives | Edited |
|---|---|---|
| Default | `crates/flottille/roles/rules.md`, in the binary | never; it is what Reset to default returns to |
| All projects | `rules.md` in Flottille's configuration folder (`$XDG_CONFIG_HOME/flottille/`), absent while the default holds | the window's Settings, "Rules"; by hand |
| One project | the `rules` key of its file | the project's Settings, "Rules"; by hand; `project {action: "set_rules"}` of an agent, held |

The project's rules **add to** the global ones: an agent gets the global rules (the user's, else the
default), then the project's under their own heading, "This project's rules", in that order, so a
project's rule can narrow a global one. A project without its own gets the global rules alone. A
project that does not want the global rules at all says `rules_alone = true` in its file (default
false; the project's Settings, "Use only this project's rules"; `set_rules {rules_alone}`): then its
rules replace the global ones, and a project with that switch and no rules gets none. Rules are at
most 8,000 characters, each text on its own; an empty text is no rules. A `rules.md` or a `rules` key
edited by hand past the limit is cut to it: agents get the first 8,000 characters, and the
daemon's log, `rules.show`, `project {action: "rules"}` and the editor say why.

A project written before rules added up may hold a copy of the global text, as this page once
advised, to keep both. When its text starts with the global text, the copy is left out for the
agent, which gets the global text once and the rest of the project's; the project's Settings says
so ("these rules repeat the global ones; you can remove that part"). No file is rewritten.

```toml
rules = """
Answer in French.
Commit only when asked.
"""
```

Reset to default removes the file (global) or the keys `rules` and `rules_alone` (project), and the
level below shows again. The project's Settings shows the global rules above its editor, read-only
and folded, with a link to edit them in Settings.

The text is read when an agent is created and frozen in its instructions (`docs/design.md` §8),
so a change reaches the agents created after it, and no agent created before. The global rules sit
before the cache boundary, where every project shares them, so editing them changes the shared
part once for everyone. A project's own rules would make that part differ from one project to
the next, so they sit after the boundary (a project with `rules_alone` leaves the global ones out
of its agents, and has its own there as well). An agent reads the rules the agents created next will get with
`project {action: "rules"}` and proposes a change with `set_rules {text}`, a base call that only
the user allows, since it rewrites the instructions of every agent created after it
(`docs/tools.md`, "The project").

## Message checks

`message_checks` lists command lines the user writes, each with `{message}` standing for a file
that holds a message, for example `pnpm commitlint --edit {message}`. None by default. They are
set in the file or in the project's Settings, tab Rules, under the rules ("Message checks", one
line each, Save; `project.set_message_checks`, `docs/protocol.md`); an agent never sets them.

They are the user's configuration, never the repository's. Flottille runs no git hook for an
agent (`core.hooksPath=/dev/null`, `docs/tools.md`, "Tasks and runs"): a hook is a file the agent
can write, and would run outside the gate's sight. A repository's own checks on its messages (a
`commit-msg` hook running a linter) are only words of its rules to an agent; declared here, they
run.

- **When**: before an agent's commit (a line of `run`, or a task, at any of its layers:
  `env git commit …`, `sh -c "git commit …"`), each check runs on the commit's message: a
  `git commit`'s, taken from its `-m` (several are paragraphs, as git joins them) or its `-F`
  file; a `git merge`'s that makes a commit (its `-m` or `-F`; none with `--ff-only`,
  `--squash` or `--no-commit`); a `git commit-tree`'s (its `-m` or `-F`); and the same through
  the user's own git alias of one of them (`alias.ci = commit` makes `git ci -m …` a commit;
  an alias of a builtin is never followed, as git follows none). Options are read as git reads
  them, an abbreviated long one too (`--m` is `--message`; one git finds ambiguous is
  refused). Flottille makes no commit of its own for an agent. A commit whose message cannot be
  known before it is made is refused while checks are declared, saying why and what to do
  instead: an editor writes it (no message, `-e`, `--edit`), git takes it from another commit
  (`-C`, `-c`, `--amend --no-edit`), or writes it (`--fixup`, `--squash`, a merge without `-m`,
  `git merge --continue`, a `git revert` without `--no-commit`), it comes from the standard
  input (`-F -`, a `git commit-tree` without `-m`), its `-F` is not a regular file (a FIFO, a
  device), or the alias runs a shell command (`!…`). `forge create` checks the pull request's
  title, `forge edit` the title it gives, and `forge merge` its squash `message`, the same way,
  before anything is written to the forge (`docs/tools.md`, "The forge"), with the variables of
  the workspace written for.
- **What they do not see**: the commits `git cherry-pick`, `git am`, `git rebase` and `git pull`
  make carry messages git takes or writes itself, and are not checked; nor is a commit a
  program the line runs makes itself (a script calling git). The `-F` file is read once for
  the checks, and git reads it again: a run that rewrites it in between commits what was not
  checked. The checks see the message as given: what git adds or removes after them
  (`--trailer`, `-s`/`--signoff`, `--cleanup`, a merge's `--log`) is not in the text they read.
- **How**: each line is split without a shell as a run's line is (`{message}` replaced in its
  words by the file's path), and run by the daemon, not as a call of the agent, in the folder of
  the repository the commit or the pull request is in, with the environment of a run of the place
  (no forge token, no push, no hook). The file is written in Flottille's state folder and removed
  once the checks ran. They run in order; the first that fails stops the others.
- **A check that fails** refuses the commit (nothing is committed), the pull request (nothing is
  written to the forge) or the merge (nothing is merged); the agent is told which check failed,
  how it ended and the end of its output (its last 1,200 characters, ANSI colours removed,
  masked as a run's output is): `refused: the message check `pnpm commitlint --edit {message}`
  refused the commit message (it exited with code 1); nothing was committed: change it and try
  again. Its output: …`.
- **A check that cannot run** never counts as passed: a program that is not found, or a check
  that takes longer than 60 s (its process group is then stopped), refuses with that reason.
  A call the agent gives up stops its checks, their process group with them.

## Tasks

`[tasks.<name>]` lists the project's command lines its agents run by name with `run`
(tests, lint, servers), and the user with the Run button of the project page. Each holds `line` (one line, never run by a shell) and, when they apply,
`repository` (where it runs; the workspace folder when left out), `serve` (it keeps running),
`check` (tests, lint), `read_only` (it changes nothing) and `ask_before_running` (held for
approval each time an agent runs it). A setup step runs one with `task = "<name>"`; a task a step
runs cannot be removed. The Tasks section of the project's Settings adds, edits and removes them
(`task.*`, `docs/protocol.md`), and so does a root agent with `tasks`, each write judged
(`docs/tools.md`).

A task may also say when a run of it is ready and where it answers; nothing is guessed:

| Key | |
|---|---|
| `ready = { line = "<text>" }` | Ready once a line of its output holds the text (plain text, no pattern). |
| `ready = { url = "<url>" }` | Ready once a request to the address is answered, whatever the status, tried every second. |
| `ready = { command = "<line>" }` | Ready once that line exits with 0, run every second in the task's folder with the run's environment, its output dropped. |
| `url = "<url>"` | Where it answers: an `http://` or `https://` address. |
| `url = { command = "<line>" }` | A line that prints the address, run once in the task's folder with the run's environment as the run starts: its first line with something on it, which must be an `http(s)://` address. For a tool that gives each worktree its own address (`portless get api`). |

Each is a template: the names of the variables (`{workspace}`, `{workspace.path}`, `{branch}`,
`{project}`, `{repository}`) and, in `ready`'s `url` and `line`, `{url}`, the run's address. A
line (`ready`'s `command`, `url`'s `command`) never takes `{url}`: the address is known only once
the run starts, after the line was judged, and the line would run what nobody judged. A
line is a task's line: split and refused as one, never run by a shell. A probe takes 5 seconds at
most and never two run at once for one run; probing stops once the run is ready or ended. An
HTTP probe of a host of this machine (`localhost`, a name under `.localhost`, a loopback address)
takes its certificate as it is, since a local development server serves one of its own
authority; any other host's is checked against the system's.

Without `ready`, a run has no ready state: it runs, as before. With it, a run is `starting`, then
`ready` (`run.ready`), or ends before it was ready, which it says. A line for the address that
fails, or prints anything but an `http(s)://` address with a host, leaves the run without one and
says why on it (`url_error`, masked, as the address itself is); a `ready` that asks for `{url}`
then cannot be told, and the run says so and keeps running without a ready state. The lines a
probe or the address start are killed with every process they started once they end, pass their
limit or the run ends. The window draws a starting
run with its spinner, a ready one with a solid dot, and its address as a link.

In a monorepo, one task per server reads best, each with its own `ready` and `url`:

```toml
[tasks."web:dev"]
line = "pnpm --filter web dev"
serve = true
url = { command = "portless get web" }
ready = { url = "{url}" }

[tasks."api:dev"]
line = "pnpm --filter api dev"
serve = true
url = "http://{workspace}.api.localhost:8080"
ready = { line = "listening on" }
```

## Skills

`[skills.<name>]` lists texts an agent reads with `skill_read` when its task needs them: a file or
a folder of the project folder, or a text Flottille keeps in its data folder. What they hold and
how they are read is in [`skills.md`](skills.md). The skills and the commands of a `.claude` folder
in the Anthropic layout are imported as entries that point at its files, with
`flottille skill import` and `flottille command import` (`skills.md` and `commands.md`, "Importing
a `.claude` folder"); nothing is written in the project's folder.

## The setup

`[[setup]]` lists the steps that prepare a new workspace, done in the order of the file
(`docs/workspaces.md` says how they run). Each step holds exactly one of:

| Key | The step |
|---|---|
| `copy = ["<glob>", …]` | Copies the files that match from the main checkout into the workspace, a folder with what it holds, **never over a file already there**: for ignored files such as `.env`. `*` and `?` match within one part of a path, and only a pattern starting with `.` matches a hidden name. A pattern that matches nothing copies nothing. Nothing in a `.git` is ever copied: a wildcard never matches it, and a pattern naming it is refused. |
| `link = "<path>"` | Links one entry of the main checkout into the workspace, unless something is there already. |
| `root_entries = true` | Links into the workspace folder every top-level entry of the project folder that is neither a repository (nor holds one), nor `.git`, nor `.gitignore`, nor already there. |
| `run = "<line>"` | Runs one command line, without a shell (the same rules as the agents' commands: a line holding shell syntax is refused). Each word is a template. |
| `task = "<name>"` | Runs one of the project's tasks. |

And, for every kind but `root_entries`, where it is done:

| Key | |
|---|---|
| `repository = "<path>"` | In that repository; a step whose repository the workspace does not take is skipped. |
| `for_each_repository = true` | Once in each repository of the workspace, in dependency order (leaves first), `{repository}` being its path. |
| neither | In the workspace folder, `{repository}` being `.`; a `task` runs where the task says. |

A `run` or a `task` step may also say `time_limit = <seconds>` (at least 1): it is stopped after
that long, in place of the project's `run_time_limit` (30 minutes unless the file says otherwise).

Paths are relative to the repository (or to the folder) and stay inside it. The setup is
snapshotted when a workspace is created, with the variables: editing the file later never
changes the steps nor the variables of a workspace that exists. This is deliberate: a workspace
is prepared, resumed and used by the setup and the variables it was made with, so a change to
the file cannot alter a preparation under way nor the environment of a piece of work already
started. A workspace that should follow the new file is a new workspace.

The Setup section of the project's Settings adds, edits, removes and reorders the steps: each
with its kind, where it is done and its time limit (`setup.*`, `docs/protocol.md`). A root agent
does the same with `setup`, each write judged, a `run` step's line as a run of it
(`docs/tools.md`).

`crates/flottille-core/tests/fixtures/multi-repo-case.toml` describes the project of
`docs/reference/multi-repo-case.md` (a folder that is a repository and holds six others, with
dependencies, base rules, variables and tasks) with this file alone;
`multi-repo-case-former-keys.toml` is the same file under the former keys, which the migration
is tested on.

## Changes

The daemon reads the files again before it answers any `project.*` request and announces what
changed since the last one: `project.added`, `project.changed`, `project.removed`
(`docs/events.md`). A project already there when the daemon starts is not announced.
