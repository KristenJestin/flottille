# Projects

A project is a folder registered with Flottille. Flottille keeps its configuration in one TOML
file per project, `$XDG_CONFIG_HOME/flottille/projects/<name>.toml` (`~/.config/flottille/projects/`
by default). The file is the user's to edit; the daemon is the only program that writes it.
**Nothing is ever written in the project's folder or its repositories**, and removing a project
never touches them: it removes only what Flottille made for it ("Removing a project").

The daemon validates a file whenever it reads it and refuses one it cannot use with its file and
its key, for example `projects/app.toml: repositories[2].depends_on: unknown repository "x"`.
Unknown keys are refused too, so a typo is never silently ignored.

## Commands

| Command | |
|---|---|
| `flottille project add <folder> [--name <name>] [--yes]` | Detects the repositories, shows them, asks for a yes on the standard input (`--yes` skips the question; no answer at all is a refusal) and registers the folder. |
| `flottille project list` | The projects. Fails, naming the file and the key, when a file cannot be used. |
| `flottille project show <name>` | One project's configuration, every default filled in. |
| `flottille project remove <name> [--discard [--yes]]` | Stops the project's agents and command runs, removes its workspaces and forgets it, never its folder ("Removing a project"). A workspace holding unsaved work is kept unless `--discard`, which shows what would be lost and asks first. |
| `flottille project check <name>` | Detects the repositories again; fails when they differ from the configuration. |

The project's Settings, Repositories tab, lists the project's repositories (`project.repositories`):
each with where it starts from, where its remote leads or "local only", and its forge; the
repositories the folder holds that the file does not name, each a row with Add; a field that adds
one by its path; and Remove, which asks first and only forgets the repository: its folder,
its branches and the worktrees of its workspaces stay, and the window says so. Adding and removing
write the file where it stands, its comments kept (`project.add_repository`,
`project.remove_repository`), and are refused, nothing written, when the file would no longer be
one the daemon can use. The tab reads the repositories again when it opens and each time the
window takes the focus back while it is shown; it has no button for it (`design.md` §10).

**The branch prefix** starts the branch of a workspace that is given none: `<branch prefix>/<name>`.
It defaults to a slug of the project's name, and a file that does not hold the key keeps that. It
may be empty (`branch_prefix = ""`): the branch is then the workspace's name alone, never
`/<name>`, for a team whose branches follow the tickets (`fix/ABC-12-…`), which an agent gives
with `branch` (`docs/workspaces.md`). Only a space is refused (`branch_prefix: must not hold
spaces`). The project's Settings, Workspaces tab, Branches section, "Prefix", edits it, an empty field
included (`project.set_settings`, `branch_prefix`); the workspaces that exist keep their branch.
A prefix whose default branch (`<prefix>/ABC-1-example`) the branch format refuses is warned about
under the field, as a warning and not a refusal.

**The branch format** is a regular expression a branch must match, off by default
(`branch_format = '^(feat|fix|chore)/[A-Z]+-[0-9]+-[a-z0-9-]+$'`; it is not anchored unless it
says `^` and `$`). With one, a workspace refuses to start a branch that does not match it, the
default `<branch prefix>/<name>` as a branch named (`docs/workspaces.md`), a workspace's
`rename_branch` refuses one, and an agent's `git push` that would make a branch outside it on a
remote is refused before anything goes (git's own dry run says which branches it makes, read once
with the message checks'); each refusal names the pattern (`the branch feature/x does
not match the project's branch format ^(feat|fix)/…$: give a branch that does`). A branch a
workspace **resumed** was named before the format and is never refused for its name, nor is a push
to a branch the remote has already. The name resumed is the one recorded when the workspace was made
(each repository's `resumed_branch`), never the branch it is on now: a branch an agent makes later
with `git switch -c` in that workspace is held to the format at its push. A branch an agent makes in
the main checkout before `workspace create` names it counts as resumed, as any branch that exists. An agent's `workspace create` and `agent_launch` refuse a
branch outside the format before the user is asked, unless a repository has it already (its own
or as last fetched): the creation then decides, once it knows which repositories resume it. The
creation refuses a branch named outside the format that no repository has, nor its remote (listed,
not fetched), before it fetches any base. A
pattern that is not a regular expression is refused when the file is read and when it is saved,
with the reason. The project's Settings, Workspaces tab, Branches section, "Allowed names", under the prefix,
edits it (`project.set_conventions`, `docs/protocol.md`); empty turns it off. Agents read it with
`project settings`.

**The pull request template** (`[pull_requests]`'s `template`, Markdown, off by default) is the
description of a pull request an agent opens without one: `forge` `create` without `body` gives the
forge the template, its card saying it whole; with `body`, the body is used as it is. The
description of `forge` says so, and `project settings` shows the template, so that an agent fills
it in `body` rather than replace it. The project's Settings, Instructions tab, "Pull request template",
under the commit and pull request checks, edits it (`project.set_conventions`); empty turns it off.

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

## Removing a project

Removing a project cleans up what Flottille made for it and never loses the user's work silently
(`project.remove`). In this order:

1. every agent of the project is stopped, roots and children, as Stop stops a session, and the
   daemon waits for their processes to be gone (15 seconds at most). An agent whose record has
   ended but whose process still lives counts as running: asking again waits for it again;
2. every command run of the project still running is stopped, the user's tasks as the agents';
3. each workspace is removed as `workspace.remove` removes it (its worktrees, then its local
   branches whose work has landed), when it would lose nothing. One holding files not committed or
   commits on no remote is **kept** on disk with its branch, and the result says which and why;
   with `discard` (the CLI's `--discard`, the window's "Also remove workspaces with unsaved work")
   it is removed too, its work lost, as long as what it would lose is within what the user was
   shown: `discard` carries back each workspace with its losses and its digest (the state its
   files were read in) as `project.removal` gave them. One that changed after the preview (an
   agent wrote in it before it was stopped, a new file or more lines in a file already shown) is
   kept, and the result says it changed. A workspace whose state cannot be read is kept;
4. the project's file is removed and `project.removed` is emitted.

The project is marked from the start: meanwhile no agent, workspace or command run starts in it
and its file is not edited, each refused with "project <name> is being removed". The folder and the checkouts of its
repositories are never touched: only the worktrees and the branches Flottille made.

**A project whose file is gone.** Its file may disappear while Flottille still holds what it made
for the project (a session, a workspace kept because it held work). Such a project is not known,
but it is not forgotten: `project.list` names it under `unconfigured` with what it holds, the
command line's `project list` says so, and `project.remove` (and `project.removal` before it) works
on it as on any project, in the same order and with the same rule, so that a workspace holding work
is kept, and the result says why. There is no file to remove, and its folder is never touched. The
daemon never removes it by itself, at start or when the file goes: a start deleting what the user
may still want is not Flottille's to decide; and the list a machine sends to the others holds only
the sessions of the projects it knows ("One list of every machine", `docs/remote.md`). A project
with nothing left but ended sessions is not listed: they are history.

A failure partway (an agent still running after the wait, a workspace git could not remove)
stops there and leaves the project known; the refusal says what failed and what was done
already, and asking again goes on from there. A workspace kept is not a failure: the project is
forgotten, the workspace stays, and once its work is saved `flottille workspace remove` removes it
as any other.

Before anything, `project.removal` says what the removal would do: the sessions and command runs
still running and each workspace's state (clean, uncommitted changes, commits not pushed). The
window's confirmation (the project's Settings, "⋯", Remove project…) shows it in labelled lines,
what runs first, then the workspaces each with its state and whether it is removed or kept, then
the discard switch when a workspace holds unsaved work; what the removal did comes as a toast.

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

The window sets both: **Settings**, General tab, "Workspaces folder" (the default for every
project, `workspaces_folder.set`), and a project's Settings, Workspaces tab, "Folder" ("As in
Settings" or "A folder for this project"), which shows the folder that results. `flottille project show` prints that folder as
`workspaces root`.
`workspaces.toml` is edited through the window or by hand; the command line has no command for it.

## Detecting repositories

The folder itself, if it holds a `.git`, and the folders one or two levels down that do. A `.git`
may be a folder or a file (a worktree or a submodule). Hidden folders (`.claude`, `.cache`) and
links are not looked into. A folder with no repository is refused. `project add` writes each
repository with the remote `origin` and the base rule `["main"]`; the user edits the file for the
rest.

The window reads them again (`project.repositories`) when the Repositories tab of the project's
Settings opens and each time the window takes the focus back while it is shown, in silence: a
repository cloned meanwhile is a row `web/ found in the folder`, with Add. There is no button for it.

## The file

```toml
# The name is the file's name: this is projects/app.toml. A `name` key is allowed if it says the same.
folder = "/work/app"                  # absolute
branch_prefix = "app"                 # default: a slug of the name; "" for none: the branch is the workspace's name
branch_format = '^(feat|fix)/[A-Z]+-[0-9]+-[a-z0-9-]+$'   # a branch must match it ("Allowed names"); none by default
workspaces_root = "/work/app-work"    # absolute (`~` and `<project>` allowed); default: the user's, see "Where the workspaces go"
default_repositories = "all"          # "all", or a list of the repositories a workspace takes
max_children = 12                     # children running at once, 1 to 12
run_time_limit = 1800                 # seconds a run step of the setup may take; default 30 min
never = [["git", "push"], ["rm", "-rf"]]   # programs with their leading arguments, one word per element, for the gate to refuse
provider = "claude"                   # the provider of its agents that name none; default: the first one enabled
model = "sonnet"                      # the model of its agents on that provider that name none; default: the provider's
windows = true                        # its agents' runs may open windows on your screen (docs/tools.md); the user's alone, default false
from_machine = "3f2a…"                # written by Flottille alone, on a project it made for a launch of another machine: that machine's key; its settings that travel follow that machine's (docs/remote.md, "Which project")

[[repositories]]                      # at least one
path = "."                            # relative to the folder; "." is the folder itself
remote = "origin"                     # default
base = ["develop", "main"]            # the first that exists on the remote wins; default ["main"]
depends_on = ["lib-core"]             # paths of repositories of the project, leaves first; no cycle
forge = { kind = "github", repository = "acme/app" }   # its forge (docs/forges.md); none when left out
delete_branch_on_merge = false        # "Delete the branch after merging": a merge by `forge` deletes the source branch on the forge; default false
commit_checks = { pre_commit = ["lint-staged"], commit_msg = ["npx --no -- commitlint --edit {message_file}"] }   # run before an agent's commit ("Commit checks"); none by default

[variables]                           # name -> templated value, for every command run in a workspace
STORE = "{workspace.path}/.store"

[tasks."api:dev"]                     # the tasks: one command line each, never run by a shell
line = "npm run start:dev"
repository = "app-api"                # where it runs; the workspace folder when left out
serve = true                          # keeps running
check = false                         # tests, lint
read_only = false                     # changes nothing
ask_before_running = false            # held for approval each time an agent runs it
needs = ["mongo"]                     # services of the machine started first ("Services")

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
template = """                        # the description of one opened without body ("Pull request template")
## Why

## How to verify
"""

[extensions.specs]                    # an extension's switch and values here (docs/extensions.md)
enabled = false                       # off in this project; on when left out, once on for all
language = "fr"                       # its own value of a setting a project may set
```

**Delete the branch after merging** is a switch of each repository, in the project's Settings page and in
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
its Jira projects (at least one), and optionally `worklog_token`, the name of the vault entry that
logs time on Jira (`docs/tickets.md`, "Logging time"). A project with no `[tracker]` takes no
ticket. With Jira chosen, the Tracker dialog has the field `Jira API access with`: a select of the vault
entries the project sees (its own and the global ones; name and username, never a secret), `None`
first, saved as `worklog_token`; under it a link, `New entry in the vault`, opens Settings, Vault on
a new entry and goes back to the project's Tracker when that dialog closes.

The forges and the tracker are set in the project's **Settings** in the window (the project page's
header): each repository's forge is pre-filled from its remote's address, nothing is active until
Save, and Save writes the file where it stands, its comments kept. A forge whose repository is not
the one its remote leads to (another host, another path) is refused with the reason, and nothing
is saved.

The `[forges]` table of earlier versions (Bitbucket Data Center hosts by name) is no longer read:
a file that still has it is accepted, the table is ignored and said (`project show` lists it under
"Not read", the window under the settings), and its repositories have no forge until one is set.
Nothing is converted on its own.

`[extensions.<name>]` holds an extension's switch and its own values for the project
(`docs/extensions.md`, "Activation"): `enabled`, and the settings the extension lets a project
set (Specs: `language`, the language its specs are written in). Once an extension is on for all
projects (`extensions.toml`), it is on in every project whose `enabled` is not `false`; off for
all, it is off everywhere and this switch stays. The extension checks the table against its
declaration: an unknown key or a value of the wrong kind makes it unusable in the project, said
by `extension.list` and by its commands, naming the key; `extension.set` and `flottille extension
on|off --project` write it where it stands.

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
set in the file or in the project's Settings, Instructions tab, "Commit and pull request checks"
(one command line per line, Save once changed; `project.set_message_checks`, `docs/protocol.md`);
an agent never sets them.

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
  a command git has, built in or a `git-<command>` it finds, is never an alias, as git follows
  none; a command that is neither, nor an alias, is refused, since git could run another in its
  place, `help.autocorrect`). Options are read as git reads
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
- **At push time**: the checks at commit time are an early answer; what goes out is checked
  again. Before a `git push` the user or the judge let through goes out, each commit it sends to
  a branch or a tag (`refs/heads/*`, `refs/tags/*`) has its message, as the commit holds it,
  given to each check, oldest first; notes (`refs/notes/*`) and other refs are not the project's
  history and are not read. Which refs it sends is git's own reading of the line and of the
  configuration (the same push with `--dry-run --porcelain`, refs up to date, rejected or
  deleted left out); which commits it sends, those they lead to that no ref of the remote has,
  as the remote itself lists them (`git ls-remote`), not the remote-tracking refs, which the
  agent can write. A new branch sends every commit no remote ref has, a force-push the commits
  of its new history, and the first push to an empty remote every commit it sends, those made
  before the checks were declared included. A failing commit refuses the push, naming the commit
  (its short sha and subject) and what the check said: nothing is pushed. A push whose commits
  cannot be known is refused as well: its options cannot be read (a valued option left without
  its value), its dry run fails for any of its URLs (git cannot reach a remote, the push fails
  already), or the remote's URL is rewritten for reads (`url.<base>.insteadOf`, which would list
  another remote than the one the push goes to). So is a push that would send submodule commits
  too (`--recurse-submodules=on-demand` or `only`, `push.recurseSubmodules` set to one of them,
  or `submodule.recurse` true with no other said): push each submodule from its own folder. So
  the commits checked at commit time are not the only ones: those of `git cherry-pick`,
  `git am`, `git rebase`, `git pull`, a stash, a raw commit object moved onto the branch, a
  script calling git, or whose message git changed after the checks (`--cleanup`, `--trailer`,
  `--signoff`) are checked here. In Ask and Auto every other push is walled: `git push` inside another line or a
  script goes nowhere, and `git send-pack`, `git http-push` and git's transport helpers, which
  would push past the wall, are refused in every mode (`docs/tools.md`).
- **What they do not see**: a commit the agent makes and never pushes stays unchecked; a ref
  another run of the agent's, still going, moves between the check and the push goes out
  unchecked; and a push through the user's own git alias of `send-pack` or `http-push`, which
  the gate sees only by the alias's name.
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

## Commit checks

A repository's `commit_checks` are the command lines its `pre-commit` and `commit-msg` hooks would
run for the user, run before an agent's `git commit` in it: `pre_commit`, in the repository's
folder, then `commit_msg`, each with `{message_file}` standing for a file that holds the commit's
message. None by default. Any program the project uses is a check (a commit message linter, a
secret scanner, a test, a script of the repository such as `./scripts/check-commit.sh
{message_file}`, spawned itself: its own `#!` line, no shell of Flottille's). They are set in the
file, in the project's Settings, Instructions tab, "Commit checks" (each repository's two fields,
one command line per line, Save once changed; `project.set_commit_checks`, `docs/protocol.md`), or
with `flottille project set-commit-checks` (`docs/cli.md`); an agent never sets them.

**Never read from the hooks at run time.** Git still runs no hook for an agent
(`core.hooksPath=/dev/null`): a hook is a file the agent can write, run by git through a shell,
outside the gate. The project's Settings and `flottille project commit-checks`
(`project.commit_checks`) read the hooks of each repository in the project's folder only to
**propose** lines, which nothing runs until the user saved them. For each hook, the first of:

- its hook manager's own entry point, so that the whole hook runs as the project wrote it:
  `lefthook run pre-commit` and `lefthook run commit-msg {message_file}` for a `lefthook.yml` (or
  `lefthook.yaml`, `.lefthook.yml`, `.lefthook.yaml`) that has the hook,
  `pre-commit run --hook-stage pre-commit` for a `.pre-commit-config.yaml`, and
  `pre-commit run --hook-stage commit-msg --commit-msg-filename {message_file}` when one of its
  `stages`, `default_stages` or `default_install_hook_types` lists `commit-msg`;
- its hook file, `.husky/<hook>`, `.githooks/<hook>` or `.git/hooks/<hook>` (never a `.sample`):
  one of the repository's own files with a `#!` line that can be run is proposed as itself
  (`./.husky/commit-msg {message_file}`), the others by the commands read in them, a line each,
  comments and husky's loading line left out, `$1` read as `{message_file}`. A command with shell
  syntax (`pnpm test && pnpm lint`) is left out, and said with why.

Every line is masked (the vault's values, those of the repository's files of secrets, the
daemon's), and a line already saved is not proposed again. A file is read only when it is a
regular file, not a link nor a FIFO, of 256 KiB at most.

- **When**: before an agent's `git commit` (a line of `run`, or a task, at any of its layers, and
  the user's git alias of it, read as the message checks read them, above), in the repository the
  commit is in: the folder git works in, as the gate reads it, moved by the wrappers around it
  (`env -C web git commit`, `env`'s options read as `env` reads them: `-vC web`, `--ch=web`, those
  of `-S`) and its own `-C` (`git -C ../web commit`), every link and `..` followed; the `-F` file of
  the message is read there too. The repository is the one git reads in that folder (its
  `git rev-parse --git-common-dir`), whatever folder holds it: a worktree of `web` made inside
  `api`, a `.git` file, a folder outside the workspace commit with `web`'s checks, which run at the
  top of that worktree. While a repository of the project has checks, a commit (or an alias) is
  refused when the gate cannot tell its repository:
  - it points git at another repository than its folder's (`--git-dir`, `--work-tree`, or
    `GIT_DIR`, `GIT_WORK_TREE` set before a program at any layer of the line): `refused: --git-dir
    points the commit at a repository other than its folder's, whose commit checks would not run,
    and the project's repositories have commit checks: commit in the repository's folder, with git
    -C <folder> if it is another one; nothing was committed`;
  - the line sets any other `GIT_*` variable before a program (`GIT_COMMON_DIR`,
    `GIT_INDEX_FILE`), but those of who wrote and made the commit (`GIT_AUTHOR_*`,
    `GIT_COMMITTER_*`): `refused: GIT_INDEX_FILE changes where or how git commits, past the commit
    checks of its repository, …`;
  - a program the gate does not know runs git (`pnpm -C ../web exec git commit`, `uv run
    --directory ../web git commit`), or `pnpm exec` (`yarn exec`, `dlx`) with an option other
    than `--package` (`--dir=../web`, `-c '<line>'`), one that moves to a folder of its own first
    (`find -execdir`, `-okdir`, `fd --base-directory`), or one that fills words in (`find -exec`,
    `-ok`, `fd -x`, `-X`, `xargs`): ``refused: `pnpm` runs git in a folder it chooses itself,
    where Flottille cannot tell which repository's commit checks to run, …``. Under these, a git
    whose command is not on the line (`xargs git`, `fd -x git {}`) is refused too;
  - a repository with checks that git cannot read: `refused: the repositories' commit checks read
    each commit before it is made, and git cannot say which repository … is in (…)`.

  A `git merge`, `git commit-tree`, `cherry-pick`, `rebase`, `am`, `revert`, `stash` or `notes`
  runs none of them, as git runs no `pre-commit` for them. A repository without checks
  commits as before.
- **`--no-verify`** (`-n`) on an agent's commit in a repository with checks is refused: `refused:
  --no-verify skips the commit checks of the repository api, which are the user's, not the agent's
  to skip: commit without it; nothing was committed`.
- **The message**: a `commit-msg` check gets the commit's `-m` (several are paragraphs, as git
  joins them) or its `-F` file, written in a private folder of Flottille's state, outside the
  repository, and removed once the checks ran. A commit whose message cannot be known before it is
  made (an editor, `-C`, `--fixup`…) is refused while the repository has `commit-msg` checks, as
  for the message checks; its `pre-commit` checks still run without one.
- **How**: each line is split without a shell, as a run's line is, and run by the daemon, not as
  a call of the agent: a check is a line the user saved, so it is not judged again; in the
  repository's folder of the place, with the environment of a run there (no forge token, no push,
  no hook), for 300 s at most (its process group is then stopped), longer than a message check
  since a first `pre-commit run` installs the environments of its hooks. They run in order, the
  `pre-commit` ones first; the first that fails stops the others.
- **What runs unjudged**: a saved check is not judged when it runs, and one that names a file of
  the repository (`./scripts/check-commit.sh`, `lefthook run`, which runs what `lefthook.yml`
  says, `pre-commit run`, what `.pre-commit-config.yaml` says) runs that file as it is at the
  commit, which the agent can change, as a workspace's task or setup step runs the repository's
  files. Saving one is trusting the repository's files as much as its tasks; a check that names
  only programs of the machine (`commitlint --edit {message_file}`) runs nothing the agent wrote.
- **A check that fails** refuses the commit before git runs: nothing is committed, and the agent
  is told which check failed, how it ended and the end of its output (its last 1,200 characters,
  ANSI colours removed, masked as a run's output is): `refused: the pre-commit check `lint-staged`
  of the repository api refused the commit (it exited with code 1); nothing was committed: fix
  what it says and try again. Its output: …`. A check that cannot run (its program is not found,
  it takes too long) refuses too. All pass, the commit runs, still with no hook.
- **What they do not see**: a `pre-commit` check runs before git, so it sees the index as the line
  found it: what `git commit -a` or `git commit <paths>` would stage is not staged yet. The checks
  at push time are the message checks' (above).

## Tasks

`[tasks.<name>]` lists the project's command lines its agents run by name with `run`
(tests, lint, servers), and the user with a place's ▶ on the project page, "Run a task here". Each holds `line` (one line, never run by a shell) and, when they apply,
`repository` (where it runs; the workspace folder when left out), `serve` (it keeps running),
`check` (tests, lint), `read_only` (it changes nothing), `ask_before_running` (held for
approval each time an agent runs it) and `needs` (the services of the machine it needs, below). A setup step runs one with `task = "<name>"`; a task a step
runs cannot be removed. The project's Settings, Tasks tab, adds, edits and removes them
(`task.*`, `docs/protocol.md`), and so does a root agent with `tasks`, each write judged
(`docs/tools.md`). A task's menu there has "Copy to…", which adds the task to another project
(`task.copy`): the same name, line and flags, and where it runs when that project has a
repository of that path, else the place's folder; a name the project already has is refused.

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

## Services

A project leans on programs that belong to the machine, not to one project or one workspace: a
database in a container, a cache, an API the apps call, each in a folder of its own outside every
project. A task runs in the place of the agent that starts it, once per workspace; a **service**
is a task of the machine, defined once, run once for every project and workspace, its state known
to the user and the agents.

The services are in `$XDG_CONFIG_HOME/flottille/services.toml` (`~/.config/flottille/services.toml`),
beside the projects' files: the user's to edit, the daemon the only program that writes it, read
and refused as a project's file is, naming the file and the key
(`services.toml: services.auth.needs: unknown service "postgres"`).

```toml
[services.mongo]
folder = "~/tools/docker/mongo"       # absolute, or ~/…; the home when left out
line = "docker compose up"            # one line, never run by a shell
ready = { command = "docker compose exec -T mongo mongosh --quiet --eval 1" }

[services.auth]
folder = "~/work/auth-api"
needs = ["mongo"]                     # started first, and waited for
line = "npm run start:dev"
url = "http://localhost:3001"
ready = { url = "{url}" }
ask_before_running = false            # held for approval each time an agent starts it
```

The keys are a task's (`line`, `ready`, `url`, `ask_before_running`, above), with `folder` and
`needs`, and without `repository`, `serve`, `check` and `read_only`: a service keeps running and
has no workspace, so its templates fill only `{url}`, in `ready`'s `url` and `line`.

- **One instance on the machine.** The daemon owns a service's run, not a workspace: starting one
  that runs (starting or ready) does nothing and says it runs, and two tasks that need it at the
  same moment share one start, each waiting for it to be ready.
- **Running outside Flottille.** Before it starts one with a `ready` it can try (an address that
  answers, a command that passes), the daemon tries it once: answered, the service runs outside
  Flottille (started by hand), nothing is started, and its state says `outside`. Listing or
  showing the services tries again one whose `ready` is an address (a connection, 5 seconds at
  most, its `url` given or none), so that one stopped since is shown stopped; a `ready` or `url`
  command is never run by a read, which no gate judged: such a service is tried at its next
  start. A service with no `ready`, or one that waits for a line of its own output, is started.
- **On demand.** Nothing starts with the daemon. A service runs until the user stops it, or an
  agent the user allowed; when the daemon stops, its services stop with it, their process groups
  as a run's are. Stopping a service does not stop what it needs nor what needs it: the state of
  those that need it says what they need is not running.
- **What runs and needs it is followed.** While a run of a task goes on, each service its `needs`
  reaches (through the needs of a service too) is followed, Flottille's or `outside`: the end of
  one Flottille runs is known at once, one that runs outside Flottille, or no longer runs, is
  tried every 15 s at its `ready` address (a connection, 5 s at most; a `ready` command is never
  run for this, no gate judged it), and nothing is followed once no run needs it. When one stops,
  the run's state says it (`needs auth, stopped`, in `run_status`, `run.list` and the run's row in
  the window), and the agent that started the run is told once, a delivery `service-stopped`:
  `auth, which dev-api needs, stopped` (`was stopped by the user` when the user stopped it). When
  it runs again (started again, or answering again at its address once the user started their
  own: it is `outside` again), the run's state no longer says it, and the agent is told once, a
  delivery `service-back`: `auth, which dev-api needs, runs again`. It says so again if it stops
  and comes back again; the daemon's own stop tells nobody.
- **Stopping what something running needs asks first.** A service that a run of a task needs, or
  another service that runs and needs it, directly or through another, is not stopped in one
  click: the window's Stop opens the confirmation `Stop auth?`, `app's dev-api needs it and is
  running: it goes on without it.`, with Stop and Cancel; `flottille service stop` refuses with the
  same sentence unless `--force`; an agent's `services stop`, held for the user every time,
  says it in its request. Nothing running needs it: it stops at once, as before. The services
  page marks, in a service's "Needed by", the tasks that run now.
- **What a daemon that died left** is ended when the next one starts, before it resumes a
  workspace's setup or starts anything. Each run's first process is recorded with its pid, its
  start time and the machine's boot. A record of another boot is dropped, and nothing is
  signalled. Of this boot: while that first process still runs (same pid, same start time), its
  session and process group are ended whole; once it is gone, only the processes of its session
  that carry the run's marker (`FLOTTILLE_RUN=<run id>` in their environment, which the daemon
  sets on each run, service and probe line it records) and started after it are killed (a
  package manager's server, still holding its port). Otherwise that leftover would answer the
  next start of the service, which would take it for one started outside Flottille, and stop
  answering later, unseen. A program of the user's own is never touched: one in a session whose
  id is a recorded pid carries no marker, and a pid taken by a running program since says that
  nothing of the run is left. A leftover that cleared its environment is not found.
- **Its run** is a run of the daemon's, of no agent and no project (`project` is empty): its
  output is kept, masked and read as a run's is (`docs/tools.md`, "Tasks and runs"), in the
  environment of a run with no place's variables, its folder's `.env` files masked. Its state is
  `stopped`, `starting`, `ready`, `running` (it says no ready), `outside`, or `failed` with the
  line that says why (the last line of its output, or why it could not start).
- **Tasks need them.** A task's `needs` names services; running it (an agent's `run`, the user's
  "Run a task here" and Run again, a setup step) first starts each it needs that does not run, those they need first,
  and waits until each with a `ready` is ready, 120 seconds at most: a service that is not ready by
  then, or fails, refuses the task, naming the service and the line that says why. An unknown name
  is refused when the project's file is read; a cycle or an unknown name in a service's `needs`
  when `services.toml` is. A service removed from the file by hand while a task still names it
  makes that project's file unreadable until the task's `needs` is changed: the daemon refuses
  such a removal itself (`service.remove`), but not an edit of the file.
- **Who wrote it, and what an agent's start is judged on.** The daemon keeps, in its store and
  not in the file, which agent last wrote each service through `services`; a write of the user
  (`flottille service`, the window) makes it the user's again, whatever it changed (even
  `ask_before_running` alone: the user wrote it last, and owns its lines from then), and so is
  every service written by hand. An agent's `services start` is judged as a run of the
  service's line in its folder. Every service an agent's call starts first (the needs of a task
  it runs, of a service it starts, of a task a setup step of a workspace it creates runs) that
  Flottille does not run is held when it is marked `ask_before_running`; one an agent wrote has
  its lines (`line`, `ready`'s and `url`'s `command`) judged in the caller's mode as a run of
  them in its folder, as is the `ready` and `url` of a service an agent wrote and starts itself.
  One seen running outside Flottille is judged too: it is started if it no longer answers. A
  service the user wrote is not judged again: the user decided what it runs. The request of a
  held call says the services it starts first, their lines and folders. The call then starts
  only the services it was judged on: one that Flottille ran when it was judged and that has
  stopped since refuses it (`the service S stopped since the call was judged: call again`).
  What the user starts ("Run a task here", Run again, `service start`, a setup step of a
  workspace the user creates) is never judged.
- **Its folder** is the home when left out: as for any place, the run's output is masked with the
  `.env` files of its folder, those of the home then.

`flottille service` (`docs/cli.md`), the `service.*` methods (`docs/protocol.md`) and the agents'
`services` tool (`docs/tools.md`) read, write, start and stop them.

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
