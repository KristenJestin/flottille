# Procedures

A procedure is a named, multi-step action of a project, run by an agent in a workspace: what a
slash command holds today (`docs/reference/multi-repo-case.md`, "Pull requests, merges…"). It
carries the instructions given to the agent, and **grants nothing**: each call its agents make
(a commit, a push, opening or merging a pull request) goes through the gate's classes and the
session's mode like any other (`docs/design.md` §7, `docs/tools.md`, "The gate").

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
```

The name is a project name's kind of word (letters, digits, `.`, `-`, `_`). An unknown key is
refused with its key, as every key of the file is.

**Former grants.** A procedure used to carry `grants` and `children`. They are read as nothing,
and said by `project.show` as obsolete; at its start the daemon removes them from the project's
file, as it removes `[agent_actions]`, keeping everything else the user wrote.

## A run

`flottille procedure run <project> <name> --workspace <workspace>` shows the procedure and asks
for a yes on the standard input (`--yes` for scripts; no answer is a refusal): nothing starts
before. Once confirmed, a root agent starts in the workspace, in Auto as every new session, its
first message the procedure's: what it runs, the workspace's repositories by dependency level
(each with its base and its remote, the levels read from the project's `depends_on`: a
repository is one level after the deepest of those it depends on among the workspace's), what
the project says of its pull requests (below), the workspace's ticket when it is linked to one,
then its instructions. The run is a session like any other: it shows in the tree, the user writes
to its root, its requests reach the user, and the user switches its mode as any session's.

In the window, a procedure is run as a slash command was: `/` at the start of a message in a
session's composer lists the project's procedures, filtered as the user types; picking one shows
the same confirmation (what it runs, the session's workspace) and Confirm starts the run in that
workspace. A session in `main` has no workspace to run one in, and a `/word` that names no
procedure is sent to the agent as text.

**The run ends** when its root reports (`report`) or ends (stopped, failed). A root the user
resumes after it ended (`agent.resume`) does not start its run again: it is an ordinary session.

**The environment.** An agent's runs get their environment without forge tokens: a push goes with
the user's own git credentials (an SSH key, a credential helper that needs no forge command line).
`git push` is not neutralised: it is a call to judge, and a forced one (even with a lease) is base.

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


The checks of a call's arguments that need the forge (a draft on Bitbucket, the title's format)
are made when it is performed: after the judge or the user allowed it.

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

A run never waits inside a tool call: when a template's agent must wait for something outside (a
pipeline, a release), it tells the user with `ask_user` what it waits for and ends its turn, and
goes on when it is written to.

The order of the levels is the project's `depends_on`: a repository that must be merged after
another (an end-to-end harness after the applications it observes) says so there.

## The command line and the protocol

| Command | |
|---|---|
| `flottille procedure list <project>` | The procedures. |
| `flottille procedure show <project> <name>` | One procedure: its description, its instructions. |
| `flottille procedure add <project> <name> --description <d> (--instructions <t> \| --instructions-file <f>)` | Adds it to the project's file. |
| `flottille procedure add <project> [<name>] --template <template>` | Adds one of Flottille's templates, as it is. |
| `flottille procedure edit <project> <name> [--description] [--instructions \| --instructions-file]` | Changes what is given. |
| `flottille procedure remove <project> <name>` | Removes it from the file. |
| `flottille procedure run <project> <name> --workspace <w> [--yes]` | Shows it, asks, starts the run. |

The daemon writes the project's file where it stands: what the user wrote around a procedure
(comments, order) stays. The methods are `procedure.list`, `procedure.show`, `procedure.add`,
`procedure.edit`, `procedure.remove`, then `procedure.run` (shows, starts nothing) and
`procedure.start` (starts) (`docs/protocol.md`); a run emits
`procedure.started` and `procedure.ended` (`docs/events.md`).

## Tests

The gate's classes are tested on their own (`gate::tests`). The scenarios
run the fake agent's sessions through their MCP server, push to throwaway bare repositories and
write to the fake `gh` (`crates/flottille-fake-gh`), which answers `pr create`, `pr merge`,
`run list` and `run rerun`, and to the fake `bkt` (`crates/flottille-fake-bkt`), which answers
`pr create`, `pr merge`, `pipeline list` and `pipeline run`; both record every call. Each template
has its scenario, the fake agent's session doing what its instructions say, on throwaway
repositories with bare remotes.
