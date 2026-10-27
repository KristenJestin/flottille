# Procedures

A procedure is a named, multi-step action of a project, run by an agent in a workspace: what a
slash command holds today (`docs/reference/multi-repo-case.md`, "Pull requests, merges…"). It
carries the instructions given to the agent and its **grants**: what the agents of a run may do
that agents are refused otherwise (commit, push, open or merge the pull requests…). The grants are
written in the procedure, shown before it runs, confirmed by the user, and applied by the gate to
the agents of that run only. Outside a run the gate decides as it always does (`docs/tools.md`).

This document is the mechanism; the procedures themselves (open the pull requests of a workspace,
merge a stack, close a workspace) are a project's own.

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
| `commit` | `git commit`, never with `--amend` (nor `--fixup=amend:`/`reword:`), `--no-verify` or `-n`. |
| `push` | `git push <remote> <refspec>…` of the workspace's own branch: the remote is the one the repository's configuration names (`origin` by default), every refspec is the branch (`app/login`, `refs/heads/app/login`, `HEAD`, `HEAD:app/login`). |
| `push-force-with-lease` | The same push with `--force-with-lease` (on the branch only, `--force-with-lease=<branch>[:<sha>]`), and `--force-if-includes`. A plain push needs `push`. |
| `rebase` | `git rebase [--onto <base>] <upstream>`, `--continue`, `--abort`, `--skip`, `--quit`, `--keep-base`: the branch checked out, never `-i`, `--exec`, `--autostash` nor a second branch. |
| `pr-create` | `forge_write` `pr-create`: opens the pull request of the workspace's branch into the repository's base. |
| `pr-merge-squash` | `forge_write` `pr-merge-squash`: merges the open pull request of the workspace's branch, by squash. |
| `pipeline-rerun` | `forge_write` `pipeline-rerun`: re-runs the failed jobs of the pipelines of the branch's last commit. |

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
workspace, its first message the procedure's: what it runs, what it is granted, then its
instructions. The run is a session like any other: it shows in the tree, the user writes to its
root, its requests reach the user.

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

## `forge_write`

The tool through which an agent writes to its workspace's forge (`docs/forges.md`), refused unless
its run grants the action:

`forge_write {action, repository, title?, body?, draft?}`

- `action`: `pr-create`, `pr-merge-squash` or `pipeline-rerun`, each the grant of its name;
- `repository`: one of the workspace's repositories, by its path; another is refused;
- `pr-create` needs `title`, takes `body` and `draft`: `gh pr create -R <owner/name> --head <branch>
  --base <base> --title … --body …`, the base the one the workspace's repository started from;
- `pr-merge-squash`: the open pull request of the workspace's branch, `gh pr merge <n> -R … --squash`;
  none, or one not open, is refused;
- `pipeline-rerun`: the failed runs of the branch's last commit (`gh run list -R … --branch <branch>`),
  each `gh run rerun <id> -R … --failed`; none failed is refused.

It is run by the daemon with the user's own `gh`, as `forge_read` is: the agent never gets a token.
Every agent in a workspace lists it (its tool list is fixed when it starts); outside a run it
answers `refused: forge_write writes to the forge only inside a procedure that grants it…`.

## The command line and the protocol

| Command | |
|---|---|
| `flottille procedure list <project>` | The procedures, with their grants. |
| `flottille procedure show <project> <name>` | One procedure: its grants, its instructions. |
| `flottille procedure add <project> <name> --description <d> (--instructions <t> \| --instructions-file <f>) [--grant <grant>=<mode>]… [--children]` | Adds it to the project's file. |
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
`run list` and `run rerun` and records every call.
