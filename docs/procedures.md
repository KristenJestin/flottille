# Procedures

A procedure is a named, multi-step action of a project, run by an agent in a workspace: what a
slash command holds today (`docs/reference/multi-repo-case.md`, "Pull requests, merges…"). It
carries the instructions given to the agent and its **grants**: what the agents of a run may do
that agents are refused otherwise (commit, push, open or merge the pull requests…). The grants are
written in the procedure, shown before it runs, confirmed by the user, and applied by the gate to
the agents of that run only. Outside a run the gate decides as it always does (`docs/tools.md`).

This document is the mechanism; the procedures themselves (open the pull requests of a workspace,
merge a stack, close a workspace) are a project's own, which it can start from Flottille's
templates (below).

## The file

A procedure lives in its project's file (`docs/projects.md`), beside the recipe and the commands:

```toml
[procedures.open-prs]
description = "Open the pull requests of the workspace"         # one line, not empty
instructions = """
Report each repository's state (uncommitted, unpushed, ahead of its base), then push the branch
and open one pull request per repository with commits."""        # the agent's first message, not empty
children = false                                                  # the run's children get its grants; default false

[procedures.open-prs.grants]                                      # each grant: "allow" or "ask"
push = "allow"
pr-create = "ask"
```

The name is a project name's kind of word (letters, digits, `.`, `-`, `_`). An unknown key, a grant
outside the vocabulary or a mode other than `allow` and `ask` is refused with its key, as every
key of the file is.

## The grants

A fixed vocabulary Flottille knows, never free text. Each lets through one kind of operation the
gate refuses to agents otherwise:

| Grant | What the agents of a run may do |
|---|---|
| `commit` | `git commit`, never with `--amend` (nor `--fixup=amend:`/`reword:`), `--no-verify` or `-n`, written whole, shortened (`--amen`, `--no-ver`) or in a group (`-an`). |
| `push` | `git push <remote> <refspec>…` of the workspace's own branch: the remote is the one the repository's configuration names (`origin` by default), every refspec is the branch (`app/login`, `refs/heads/app/login`, `HEAD:app/login`); a bare `HEAD` is refused, since the branch it is on may not be the workspace's. |
| `push-force-with-lease` | The same push with `--force-with-lease` (on the branch only, `--force-with-lease=<branch>[:<sha>]`), and `--force-if-includes`. A plain push needs `push`. |
| `rebase` | `git rebase [--onto <base>] <upstream>`, `--continue`, `--abort`, `--skip`, `--quit`, `--keep-base`: the branch checked out, never `-i`, `--exec`, `--autostash` nor a second branch. |
| `pr-create` | `forge_write` `pr-create`: opens the pull request of the workspace's branch into the repository's base. |
| `pr-merge-squash` | `forge_write` `pr-merge-squash`: merges the open pull request of the workspace's branch, by squash. |
| `pipeline-rerun` | `forge_write` `pipeline-rerun`: re-runs the failed jobs of the pipelines of the branch's last commit. |
| `workspace-remove` | `workspace_remove`: removes the run's own workspace, as `flottille workspace remove` does, and deletes its empty branches; refused while an agent or a command run outside the run works in it. |

Each grant is `allow` (let through at once) or `ask` (held for the user every time, as the gate
holds a call today: the agent is answered at once that the request waits, and the result comes
later as a delivery). A request held by an `ask` grant is never allowed for the tree: the user
allows it once, each time.

A grant only ever lets through the line it names, run directly: `git` itself, with nothing but
`--no-pager` before its subcommand (no `-C`, `-c`, `--git-dir`), never wrapped (`env git push`,
`sh -c 'git push'` stay refused). The other rules of the gate still apply before it: the project's
`never` list, the `.git` folders, the recursive deletions.

**Refused even inside a run, whatever it grants**: `git stash`, `git reset --hard`, a push with
`--force`, `-f` or a `+refspec`, a push that deletes (`--delete`, `-d`, `:branch`, `--prune`) or
sends more than the branch (`--all`, `--mirror`, `--tags`, `--follow-tags`), a push to another
remote or of another branch, `git merge` (a pull request is merged on the forge, by squash), and
everything the gate refuses that no grant names (`docs/tools.md`).

## A run

`flottille procedure run <project> <name> --workspace <workspace>` shows the procedure, its grants
and whether its children get them, and asks for a yes on the standard input (`--yes` for scripts;
no answer is a refusal): nothing starts before. Once confirmed, a root agent starts in the
workspace, its first message the procedure's: what it runs, what it is granted, the workspace's
repositories by dependency level (each with its base and its remote, the levels read from the
project's `depends_on`: a repository is one level after the deepest of those it depends on among
the workspace's), what the project says of its pull requests when the run grants `pr-create`
(below), the workspace's ticket when it is linked to one, then its instructions. The run is a session like any other: it shows in the tree, the user writes to its
root, its requests reach the user.

**A procedure that writes to a forge needs every forge declared.** When it grants `pr-create`,
`pr-merge-squash` or `pipeline-rerun` and a repository of the workspace has no forge in the
project's file (`docs/forges.md`, "The forge of a repository"), it refuses to start, naming them:
`procedure open-prs writes to the forge (pr-create), and no forge is set for api, web of workspace
login: choose it in the project's Settings (the window), or write its `forge` in the project's
file; nothing was started`. Nothing is guessed from a remote's host. A procedure granting only git
actions starts as before.

The grants **belong to the run**: they are frozen when the user confirms them, held by the daemon
for the run's root agent, and no tool nor argument of an agent changes them. Editing the procedure
later changes no run that started. They apply:

- to the run's root agent;
- to its children, at any depth, only when the procedure says `children = true`; a child gets none
  otherwise;
- only in the run's workspace: an agent of the tree working elsewhere gets none.

**The run ends** when its root reports (`report`) or ends (stopped, failed): its grants are given
no more, and the next push is refused as outside a run. A request an `ask` grant held is checked
again when the user allows it: a run that ended meanwhile makes it not executed.

**The push's environment.** An agent's runs have `git push` neutralised by their environment
(`docs/tools.md`). The one run whose line the gate let through as a granted push gets its
environment without that rewrite, and only it: a script the agent wrote and runs keeps the wall.
The forge tokens stay empty either way: a push goes with the user's own git credentials (an SSH
key, a credential helper that needs no forge command line).

**No hook.** A line a grant lets through (a commit, a push, a rebase) runs with
`core.hooksPath=/dev/null`, given through `GIT_CONFIG_COUNT` in its environment so that the
agent's line is unchanged: the hooks are files of the repository or of its configured hooks
folder, which the agent writes, and a `pre-push` hook would run with the push wall down. The
repository's own hooks (a commit message check) do not run for a procedure's commits.

**Where the remote leads.** A workspace records, at its creation, the URLs its repositories'
remotes lead to (fetch and push, `insteadOf` applied, credentials taken out: the `urls` of each
base, `docs/protocol.md`). An agent never changes them (`git remote` and `git config` writes are
refused, `docs/tools.md`), and a granted push or `forge_write` is refused, before git or `gh` runs,
when the remote leads anywhere else now. A workspace created before they were recorded gets no
granted push nor forge write.

## `forge_write`

The tool through which an agent writes to its workspace's forge (`docs/forges.md`), refused unless
its run grants the action:

`forge_write {action, repository, title?, body?, draft?, waive?}`

- `action`: `pr-create`, `pr-merge-squash` or `pipeline-rerun`, each the grant of its name;
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
  then holds the call for the user, whatever the grant's mode, and the merge goes without looking
  at the checks once they allow it;
- `pipeline-rerun`: the failed runs of the branch's last commit (`gh run list -R … --branch <branch>`),
  each `gh run rerun <id> -R … --failed`; none failed is refused.

It is run by the daemon with the user's own `gh`, as `forge_read` is: the agent never gets a token.

**On Bitbucket** (`bkt`, Cloud and Data Center) the same three actions run under the same grants and
the same checks (the remote kept, the workspace's own pull request, not a fork's, into the recorded
base), through `bkt pr create`, `bkt pr merge --strategy squash` and, on Cloud only, `bkt pipeline
run --ref <branch>` after the branch's newest pipeline failed: what each does, and what differs
from `gh`, is in [`forges.md`](forges.md#bitbucket).
Every agent in a workspace lists it (its tool list is fixed when it starts); outside a run it
answers `refused: forge_write writes to the forge only inside a procedure that grants it…`.

## The project's pull requests

What a project's pull requests look like is said once, in its file, for every procedure that
opens them:

```toml
[pull_requests]
title = "type(TICKET): subject"     # the title's format, in the project's words; told to the agent
reviewers = ["alice", "bob"]        # the forge's names, asked to review each pull request opened
```

The title's format is told to the run's agent, which writes the titles; the reviewers are added by
Flottille to every `pr-create`, so that no agent forgets one. A reviewer is one name of the forge
(no space, no comma, not starting with `-`). Both are read as the file is when they are used.

## Templates

Flottille ships three procedures, written generically, which a project adopts with
`flottille procedure add <project> --template <template> [<name>]`: written into its file as they
are, under the template's name unless another is given, they are the project's own from then on,
to edit like any other (its instructions are a multi-line string). They are what
`docs/reference/multi-repo-case.md` runs by hand with slash commands. Their agents use Flottille's
tools only (`forge_read`, `forge_write`, `workspace_remove`, the git lines the grants let through),
never a forge's command line, so they work on every forge Flottille writes to.

| Template | What its agent does | Grants |
|---|---|---|
| `open-prs` | Reports each repository first (uncommitted work, commits on no remote, commits ahead of its base, its pull request) to the user with `ask_user`, and stops if one holds uncommitted work; then, for each repository with commits of its own and no open pull request, pushes its branch and opens its pull request, its title in the project's format. None for the others. It makes no commit. | `push` (ask), `pr-create` (ask): one confirmation per operation, the user's approval of exactly that push or that pull request. |
| `merge-stack` | Checks every pull request first (reviews, comments, checks) and sends the user what blocks; then merges by dependency levels, the repositories of one level together, rebasing a branch behind its base and pushing it with a lease, waiting between levels for what the project says follows a merge (its pipelines, a release), re-running a pipeline failed outside the change once. A merge the checks or a review block goes only when the user waives it. | `rebase`, `push-force-with-lease`, `pr-merge-squash`, `pipeline-rerun` (all allow: what running it authorizes; Flottille refuses a blocked merge itself). |
| `close` | Audits each repository (uncommitted work, commits on no remote, commits of its own, its pull request): uncommitted or unpushed work, or a branch with commits whose pull request is not merged, stops it and is told to the user; otherwise `workspace_remove`, which checks again, removes the workspace and deletes its empty branches. | `workspace-remove` (allow). |

A run never waits inside a tool call: when a template's agent must wait for something outside (a
pipeline, a release), it tells the user with `ask_user` what it waits for and ends its turn, and
goes on when it is written to. Their children get none of their grants.

The order of the levels is the project's `depends_on`: a repository that must be merged after
another (an end-to-end harness after the applications it observes) says so there.

## The command line and the protocol

| Command | |
|---|---|
| `flottille procedure list <project>` | The procedures, with their grants. |
| `flottille procedure show <project> <name>` | One procedure: its grants, its instructions. |
| `flottille procedure add <project> <name> --description <d> (--instructions <t> \| --instructions-file <f>) [--grant <grant>=<mode>]… [--children]` | Adds it to the project's file. |
| `flottille procedure add <project> [<name>] --template <template>` | Adds one of Flottille's templates, as it is. |
| `flottille procedure edit <project> <name> [--description] [--instructions \| --instructions-file] [--grant …]… [--no-grants] [--children true\|false]` | Changes what is given; the grants given replace all of them. |
| `flottille procedure remove <project> <name>` | Removes it from the file. |
| `flottille procedure run <project> <name> --workspace <w> [--yes]` | Shows the grants, asks, starts the run. |

The daemon writes the project's file where it stands: what the user wrote around a procedure
(comments, order) stays. The methods are `procedure.list`, `procedure.show`, `procedure.add`,
`procedure.edit`, `procedure.remove`, then `procedure.run` (shows, starts nothing) and
`procedure.start` (starts, given the grants that were shown) (`docs/protocol.md`); a run emits
`procedure.started` and `procedure.ended` (`docs/events.md`).

## Tests

The gate's rules are tested on their own (`gate::tests`), with and without a run. The scenarios
run the fake agent's sessions through their MCP server, push to throwaway bare repositories and
write to the fake `gh` (`crates/flottille-fake-gh`), which answers `pr create`, `pr merge`,
`run list` and `run rerun`, and to the fake `bkt` (`crates/flottille-fake-bkt`), which answers
`pr create`, `pr merge`, `pipeline list` and `pipeline run`; both record every call. Each template
has its scenario, the fake agent's session doing what its instructions say, on throwaway
repositories with bare remotes.
