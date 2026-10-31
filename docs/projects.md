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

The window does the same from the sidebar's **+** beside "Projects" (or the empty window's "Add a
project"): a folder, typed or picked, the repositories detected in it shown before anything is
registered, then the new project's Settings, where it can be checked again or removed.

All of them take `--json` and print what the daemon answered (`docs/protocol.md`).

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
branch_prefix = "app"                 # default: a slug of the name
workspaces_root = "/work/app-work"    # absolute; default: $XDG_DATA_HOME/flottille/workspaces/<name>
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

[pull_requests]                       # how its pull requests are opened (docs/commands.md)
title = "type(TICKET): subject"       # the title's format, told to the agent that writes it
reviewers = ["alice", "bob"]          # asked to review each pull request Flottille opens
```

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
for its own discipline: the three classes, the mode (Ask or Auto, one per tree, the user's alone)
and the judge, what always comes to the user, what is refused, and that a refusal or a held
request is an answer, never to be reached another way. A test checks each sentence against the
gate's answers (`tests/rules.rs`). **How to work** is the user's: commits only when asked, say what you did,
and whatever else they want.

| Level | Where it lives | Edited |
|---|---|---|
| Default | `crates/flottille/roles/rules.md`, in the binary | never; it is what Reset to default returns to |
| All projects | `rules.md` in Flottille's configuration folder (`$XDG_CONFIG_HOME/flottille/`), absent while the default holds | the window's Settings, "Rules"; by hand |
| One project | the `rules` key of its file | the project's Settings, "Rules"; by hand; `project {action: "set_rules"}` of an agent, held |

The project's rules **replace** the global ones, they do not extend them: a project that wants
both copies the global text and adds to it. A project without its own follows the global rules,
or the default when there are none. Rules are at most 8,000 characters; an empty text is no rules. A `rules.md` or a `rules` key
edited by hand past the limit is cut to it: agents get the first 8,000 characters, and the
daemon's log, `rules.show`, `project {action: "rules"}` and the editor say why.

```toml
rules = """
Answer in French.
Commit only when asked.
"""
```

Reset to default removes the file (global) or the key (project), and the level below shows again.

The text is read when an agent is created and frozen in its instructions (`docs/design.md` §8),
so a change reaches the agents created after it, and no agent created before. The global rules sit
before the cache boundary, where every project shares them, so editing them changes the shared
part once for everyone. A project's own rules would make that part differ from one project to
the next, so they sit after the boundary. An agent reads the rules the agents created next will get with
`project {action: "rules"}` and proposes a change with `set_rules {text}`, a base call that only
the user allows, since it rewrites the instructions of every agent created after it
(`docs/tools.md`, "The project").

## Tasks

`[tasks.<name>]` lists the project's command lines its agents run by name with `run`
(tests, lint, servers), and the user with the Run button of the project page. Each holds `line` (one line, never run by a shell) and, when they apply,
`repository` (where it runs; the workspace folder when left out), `serve` (it keeps running),
`check` (tests, lint), `read_only` (it changes nothing) and `ask_before_running` (held for
approval each time an agent runs it). A setup step runs one with `task = "<name>"`; a task a step
runs cannot be removed. The Tasks section of the project's Settings adds, edits and removes them
(`task.*`, `docs/protocol.md`), and so does a root agent with `tasks`, each write judged
(`docs/tools.md`).

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
