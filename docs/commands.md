# Commands

A command is a project's named instructions, sent to a session as `/name`: what a slash command of
an agent's own client holds (`docs/reference/multi-repo-case.md`, "Pull requests, merges…"). It
holds a name, a one-line description and its instructions, nothing else: no run, no grant, no
child. What the session it is sent to does then (a commit, a push, opening or merging a pull
request) goes through the gate's classes and the session's mode like any other call
(`docs/design.md` §7, `docs/tools.md`, "The gate").

Commands are Flottille's data about a project, kept in the project's file in Flottille's
configuration, never in the project's repositories. This document is the mechanism; the commands
themselves (open the pull requests of a workspace, merge a stack, close a workspace) are a
project's own, which it can start from Flottille's templates (below).

## The file

A command lives in its project's file (`docs/projects.md`), beside the tasks, the setup and the
skills:

```toml
[commands.open-prs]
description = "Open the pull requests of the workspace"         # one line, not empty
instructions = """
Report each repository's state (uncommitted, unpushed, ahead of its base), then push the branch
and open one pull request per repository with commits."""        # what the session is sent, not empty
```

The name is a project name's kind of word (letters, digits, `.`, `-`, `_`). An unknown key is
refused with its key, as every key of the file is.

**Former names.** Until #127 a command was a *procedure*, under `[procedures.<name>]`, and
`[commands.<name>]` held what are now the project's tasks (`docs/projects.md`, "Tasks"). The
former keys are still read for one release; at its start the daemon renames them in the file
(a `[commands.<name>]` with a `line` goes to `[tasks.<name>]`, then `[procedures.<name>]` becomes
`[commands.<name>]`), keeping every comment and everything else the user wrote, and replaces the
file at once. A name found under both a former key and its new one is refused, with both keys
named. A procedure's former `grants` and `children` are read as nothing and removed at the same
start, as `[agent_actions]` is.

## Sending one

`/name` at the start of a message sends the command: the session's agent gets a short header,
then the command's instructions, then what the message holds after the name:

```
The command open-prs of project shop: Open the pull requests of the workspace
It runs in workspace login (branch shop/login), whose repositories are, by dependency level:
- level 1: lib (base main, on origin)
- level 2: api (base main, on origin), web (base main, on origin)
A pull request's title follows the project's format: type(TICKET): subject
---

Report each repository's state…

and the docs too
```

The header says the workspace and its repositories by dependency level (each with its base and
its remote, the levels read from the project's `depends_on`: a repository is one level after the
deepest of those it depends on among the workspace's), what the project says of its pull requests
(below) and the workspace's ticket when it is linked to one; a session in `main` gets the command's
name and description only. A `/word` that names no command is sent as it is written.

It works wherever a message is sent to an agent: the window's composer, `flottille agent send`,
the message of `agent start` and `agent run`, and an agent's own `agent_send` to its child. Nothing
starts that the user did not start: a command is a message, sent to a session that exists.

In the window, `/` at the start of a message in a session's composer lists the project's commands,
filtered as the user types; picking one puts `/name ` in the field, for the user to add what it is
about, and Enter sends it. The commands are edited in the project's Settings, section Commands.

**The environment.** An agent's runs get their environment without forge tokens: a push goes with
the user's own git credentials (an SSH key, a credential helper that needs no forge command line).
`git push` is a call to judge, and a forced one (even with a lease) is base. Every other run has
its pushes walled (`url.<nowhere>.pushInsteadOf`): only a run that is itself a `git push` the user
or the judge let through goes out, so a script cannot push behind the gate (`docs/tools.md`).

**No hook.** Every line an agent runs gets `core.hooksPath=/dev/null`, given through
`GIT_CONFIG_COUNT` in its environment so that the agent's line is unchanged: the hooks are files of
the repository or of its configured hooks folder, which the agent writes. The repository's own
hooks (a commit message check) do not run for an agent's commits.

**Where the remote leads.** A workspace records, at its creation, the URLs its repositories'
remotes lead to (fetch and push, `insteadOf` applied, credentials taken out: the `urls` of each
base, `docs/protocol.md`). An agent never changes them (`git remote` and `git config` writes are
refused, `docs/tools.md`), and a push from a workspace or a `forge_write` is refused, before git or
`gh` runs, when the remote leads anywhere else now. A workspace created before they were recorded
gets no push nor forge write.

## `forge_write`

The tool through which an agent in a workspace writes to its workspace's forge
(`docs/forges.md`). Each call is to judge; merging what the checks or a review block (`waive`) is
base:

`forge_write {action, repository, title?, body?, draft?, waive?}`

- `action`: `pr-create`, `pr-merge-squash` or `pipeline-rerun`;
- `repository`: one of the workspace's repositories, by its path; another is refused, and so is
  one with no forge declared, or whose declared forge is not where its remote leads;
- `pr-create` needs `title`, takes `body` and `draft`: `gh pr create -R github.com/<owner/name> --head <branch>
  --base <base> --title … --body …`, the base the one the workspace's repository started from, and
  a `--reviewer` for each of the project's reviewers (below), on Bitbucket as on GitHub (`draft` is
  refused for Bitbucket, whose `bkt` opens none);
- `pr-merge-squash`: the open pull request of the workspace's branch from the repository itself
  (not from a fork, which can have a branch of the same name) into its base,
  `gh pr merge <n> -R … --squash`; none is refused. So is one whose checks fail or are still
  running, or whose review asks for changes (`refused: the pull request #n of … is not merged: its
  checks fail (build)…`), on Bitbucket as on GitHub (`docs/forges.md`), unless `waive` gives the user's words for merging it as it is: the gate
  then holds the call for the user, in every mode, and the merge goes without looking
  at the checks once they allow it;
- `pipeline-rerun`: the failed runs of the branch's last commit (`gh run list -R … --branch <branch>`),
  each `gh run rerun <id> -R … --failed`; none failed is refused.

It is run by the daemon with the user's own `gh`, as `forge_read` is: the agent never gets a token.

**On Bitbucket** (`bkt`, Cloud and Data Center) the same three actions run under the same classes and
the same checks (the remote kept, the workspace's own pull request, not a fork's, into the recorded
base), through `bkt pr create`, `bkt pr merge --strategy squash` and, on Cloud only, `bkt pipeline
run --ref <branch>` after the branch's newest pipeline failed: what each does, and what differs
from `gh`, is in [`forges.md`](forges.md#bitbucket).
It is listed to every agent of a project whose repositories declare a forge, in `main` as in a
workspace (its tool list is fixed when it starts). Listing allows nothing: each call goes through
the gate's classes and the session's mode.

The checks of a call's arguments that need the forge (a draft on Bitbucket, the title's format)
are made when it is performed: after the judge or the user allowed it.

## The project's pull requests

What a project's pull requests look like is said once, in its file, for every command that
opens them:

```toml
[pull_requests]
title = "type(TICKET): subject"     # the title's format, in the project's words; told to the agent
reviewers = ["alice", "bob"]        # the forge's names, asked to review each pull request opened
```

The title's format is told to the agent a command is sent to, which writes the titles; the reviewers are added by
Flottille to every `pr-create`, so that no agent forgets one. A reviewer is one name of the forge
(no space, no comma, not starting with `-`). Both are read as the file is when they are used.

## Templates

Flottille ships three commands, written generically, which a project adopts with
`flottille command add <project> --template <template> [<name>]` or from its Settings in the
window: written into its file as they are, under the template's name unless another is given,
they are the project's own from then on, to edit like any other (its instructions are a
multi-line string). They are what
`docs/reference/multi-repo-case.md` runs by hand with slash commands. Their agents use Flottille's
tools only (`forge_read`, `forge_write`, `workspace_remove`, git lines through `commands_run`),
never a forge's command line, so they work on every forge Flottille writes to.

| Template | What its agent does |
|---|---|
| `open-prs` | Reports each repository first (uncommitted work, commits on no remote, commits ahead of its base, its pull request) to the user with `ask_user`, and stops if one holds uncommitted work; then, for each repository with commits of its own and no open pull request, pushes its branch and opens its pull request, its title in the project's format. None for the others. It makes no commit. |
| `merge-stack` | Checks every pull request first (reviews, comments, checks) and sends the user what blocks; then merges by dependency levels, the repositories of one level together, rebasing a branch behind its base and pushing it with a lease, waiting between levels for what the project says follows a merge (its pipelines, a release), re-running a pipeline failed outside the change once. A merge the checks or a review block goes only when the user waives it. |
| `close` | Audits each repository (uncommitted work, commits on no remote, commits of its own, its pull request): uncommitted or unpushed work, or a branch with commits whose pull request is not merged, stops it and is told to the user; otherwise `workspace_remove`, which checks again, removes the workspace and deletes its empty branches. |

What each call of theirs needs is the session's mode: in Ask, each push and pull request is a
card; in Auto, the judge allows what the user asked for and asks the rest. A push with a lease
(`merge-stack`) is base: it always comes to the user.

An agent never waits inside a tool call: when a template's agent must wait for something outside
(a pipeline, a release), it tells the user with `ask_user` what it waits for and ends its turn,
and goes on when it is written to.

The order of the levels is the project's `depends_on`: a repository that must be merged after
another (an end-to-end harness after the applications it observes) says so there.

## The command line, the protocol and the agents

| Command | |
|---|---|
| `flottille command list <project>` | The commands. |
| `flottille command show <project> <name>` | One command: its description, its instructions. |
| `flottille command add <project> <name> --description <d> (--instructions <t> \| --instructions-file <f>)` | Adds it to the project's file. |
| `flottille command add <project> [<name>] --template <template>` | Adds one of Flottille's templates, as it is. |
| `flottille command edit <project> <name> [--description] [--instructions \| --instructions-file]` | Changes what is given. |
| `flottille command remove <project> <name>` | Removes it from the file. |

The daemon writes the project's file where it stands: what the user wrote around a command
(comments, order) stays. The methods are `command.list`, `command.show`, `command.add`,
`command.edit` and `command.remove` (`docs/protocol.md`). A root agent does the same with its
`commands` tool (`docs/tools.md`, "The project"): its reads run at once, its writes are to judge.

## Tests

The gate's classes are tested on their own (`gate::tests`). The scenarios
run the fake agent's sessions through their MCP server, push to throwaway bare repositories and
write to the fake `gh` (`crates/flottille-fake-gh`), which answers `pr create`, `pr merge`,
`run list` and `run rerun`, and to the fake `bkt` (`crates/flottille-fake-bkt`), which answers
`pr create`, `pr merge`, `pipeline list` and `pipeline run`; both record every call. Each template
has its scenario, the fake agent's session doing what its instructions say, on throwaway
repositories with bare remotes.
