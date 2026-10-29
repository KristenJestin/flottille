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
max_children = 3                      # children running at once, 1 to 6
run_time_limit = 1800                 # seconds a run step of the recipe may take; default 30 min
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

[commands."api:dev"]                  # the catalogue: one command line, never run by a shell
line = "npm run start:dev"
repository = "app-api"                # where it runs; the workspace folder when left out
serve = true                          # keeps running
check = false                         # tests, lint
read_only = false                     # changes nothing
ask_before_running = false            # held for approval each time an agent runs it

[[recipe]]                            # the steps that prepare a new workspace, in order
copy = [".env", ".env.*"]             # globs, copied from the main checkout, never over a file
for_each_repository = true            # in each repository of the workspace, leaves first

[[recipe]]
root_entries = true                   # the project folder's other entries, linked

[[recipe]]
run = "npm install"                   # one line, its words templated, never run by a shell
for_each_repository = true

[tracker]                             # where its tickets are (docs/tickets.md); none when left out
kind = "jira"
site = "acme.atlassian.net"
projects = ["ABC", "OPS"]

[procedures.open-prs]                 # a named action run by an agent in a workspace (docs/procedures.md)
description = "Open the pull requests"
instructions = "Push the branch, then open one pull request per repository."

[pull_requests]                       # how its pull requests are opened (docs/procedures.md)
title = "type(TICKET): subject"       # the title's format, told to the agent that writes it
reviewers = ["alice", "bob"]          # asked to review each pull request Flottille opens
```

The repositories are put in dependency order (a repository after those it depends on; among those
that are ready, the first of the file first) by `Project::in_dependency_order()`, which preparing
a workspace uses, and in dependency levels by `Project::dependency_levels()`, which a procedure's
run is told for merging a level at a time.

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

A procedure's keys and what a run of it does are in
[`procedures.md`](procedures.md); `flottille procedure add`, `edit` and `remove` write them in the
file where it stands, keeping what is around them.

## What agents do on their own

The agent the user talks to (a root) does what the window does, through the tools `procedure` and
`project` (`docs/tools.md`, "The project"): it reads the project's procedures and settings at once,
and every change it makes is a call to judge, said in words: a card for the user in Ask, the
judge's in Auto (`docs/design.md` §7). A change of the tracker, which nothing checks against the
project, is base: it always comes to the user. An agent never reaches another project's file.

**Former keys.** `[agent_actions]`, which let a family of these changes run on its own, a
procedure's `grants` and `children`, and a command's `added_by` (a line the user once allowed
always) are no longer read: a file that still has them is accepted, `project show` lists the
first three under "Not read", and the daemon removes them from the file at its start, keeping everything else
the user wrote.

## References

`[references.<name>]` lists documents an agent reads with a tool when its task needs them, in place
of the knowledge skills (framework guides, a test library, a design system) that other tools load
every time. Each holds:

| Key | |
|---|---|
| `description` | One line, not empty: when the document is worth reading. |
| `path` | A file or a folder of files, relative to the project folder (a file of a repository is `<repository>/<file>`). It must stay inside the project folder, once the links are followed, and not go through a `.git`. |

```toml
[references.react]
description = "How we write React components"
path = "docs/react"

[references.style]
description = "The code style"
path = "docs/STYLE.md"
```

An agent sees the names and descriptions in its instructions, after the cache boundary, never
the content (`docs/design.md` §8); it reads one with `reference_read` (`docs/tools.md`). The list
is frozen in an agent's instructions when the agent is created, so a reference added later is
seen by the agents started after it; `reference_read` itself follows the file as it is now. A
reference is read from the project folder, whatever the agent's place: it is the project's, not
the branch's.

`flottille reference list|add|remove` edits the file for you, the rest of it left as written,
comments included (`docs/cli.md`).

## The recipe

`[[recipe]]` lists the steps that prepare a new workspace, done in the order of the file
(`docs/workspaces.md` says how they run). Each step holds exactly one of:

| Key | The step |
|---|---|
| `copy = ["<glob>", …]` | Copies the files that match from the main checkout into the workspace, a folder with what it holds, **never over a file already there**: for ignored files such as `.env`. `*` and `?` match within one part of a path, and only a pattern starting with `.` matches a hidden name. A pattern that matches nothing copies nothing. Nothing in a `.git` is ever copied: a wildcard never matches it, and a pattern naming it is refused. |
| `link = "<path>"` | Links one entry of the main checkout into the workspace, unless something is there already. |
| `root_entries = true` | Links into the workspace folder every top-level entry of the project folder that is neither a repository (nor holds one), nor `.git`, nor `.gitignore`, nor already there. |
| `run = "<line>"` | Runs one command line, without a shell (the same rules as the agents' commands: a line holding shell syntax is refused). Each word is a template. |
| `command = "<name>"` | Runs a command of the catalogue. |

And, for every kind but `root_entries`, where it is done:

| Key | |
|---|---|
| `repository = "<path>"` | In that repository; a step whose repository the workspace does not take is skipped. |
| `for_each_repository = true` | Once in each repository of the workspace, in dependency order (leaves first), `{repository}` being its path. |
| neither | In the workspace folder, `{repository}` being `.`; a `command` runs where the catalogue says. |

A `run` or a `command` step may also say `time_limit = <seconds>` (at least 1): it is stopped after
that long, in place of the project's `run_time_limit` (30 minutes unless the file says otherwise).

Paths are relative to the repository (or to the folder) and stay inside it. The recipe is
snapshotted when a workspace is created, with the variables: editing the file later never
changes the steps nor the variables of a workspace that exists. This is deliberate: a workspace
is prepared, resumed and used by the recipe and the variables it was made with, so a change to
the file cannot alter a preparation under way nor the environment of a piece of work already
started. A workspace that should follow the new file is a new workspace.

`crates/flottille-core/tests/fixtures/multi-repo-case.toml` describes the project of
`docs/reference/multi-repo-case.md` (a folder that is a repository and holds six others, with
dependencies, base rules, variables and commands) with this file alone.

## Changes

The daemon reads the files again before it answers any `project.*` request and announces what
changed since the last one: `project.added`, `project.changed`, `project.removed`
(`docs/events.md`). A project already there when the daemon starts is not announced.
