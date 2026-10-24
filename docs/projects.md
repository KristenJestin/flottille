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
never = ["git push", "npm publish"]   # programs with their leading arguments, for the gate to refuse

[[repositories]]                      # at least one
path = "."                            # relative to the folder; "." is the folder itself
remote = "origin"                     # default
base = ["develop", "main"]            # the first that exists on the remote wins; default ["main"]
depends_on = ["lib-core"]             # paths of repositories of the project, leaves first; no cycle

[variables]                           # name -> templated value, for every command run in a workspace
STORE = "{workspace.path}/.store"

[commands."api:dev"]                  # the catalogue: one command line, never run by a shell
line = "npm run start:dev"
repository = "app-api"                # where it runs; the workspace folder when left out
serve = true                          # keeps running
check = false                         # tests, lint
read_only = false                     # changes nothing
ask_before_running = false            # held for approval each time an agent runs it
```

The templates of the variables are read when workspaces exist (`docs/design.md` §6.2): the
configuration only checks that the names of the variables are those of environment variables.

`crates/flottille-core/tests/fixtures/multi-repo-case.toml` describes the project of
`docs/reference/multi-repo-case.md` (a folder that is a repository and holds six others, with
dependencies, base rules, variables and commands) with this file alone.

## Changes

The daemon reads the files again before it answers any `project.*` request and announces what
changed since the last one: `project.added`, `project.changed`, `project.removed`
(`docs/events.md`). A project already there when the daemon starts is not announced.
