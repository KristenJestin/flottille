# Workspaces

A workspace is the place one piece of work happens: one git worktree per repository it takes, all
on one branch, in one folder, prepared in the background by the project's setup
(`docs/projects.md`) and removed only when nothing in it would be lost. It follows
`docs/reference/hemera.md` §6, with the additions of `docs/design.md` §6.2 that the
multi-repository case (`docs/reference/multi-repo-case.md`) needs.

## Commands

| Command | |
|---|---|
| `flottille workspace create <project> <name> [--repositories a,b] [--branch <branch>] [--base <repository>=<branch>] [--offline]` | Makes the worktrees and answers; the rest of the setup goes on in the background. Fails when the preparation already failed. |
| `flottille workspace list [<project>]` | The workspaces with their state. |
| `flottille workspace show <project> <name>` | Its repositories, their bases, and each step with its state; a failure with its last lines. |
| `flottille workspace ticket <project> <name> <ticket>` | Links it to a ticket (`#123` or a Jira key), or to none with `--clear` (`docs/tickets.md`). `create` takes `--ticket` too. |
| `flottille workspace prepare <project> <name>` | Resumes a preparation, from the step that failed. |
| `flottille workspace remove <project> <name>` | Removes it when all its work is committed and pushed, then deletes its local branches whose work has landed, never a remote one. Never forced: only the user's answer to an agent's request discards work ("Removal"). |

All of them take `--json` and print what the daemon answered (`docs/protocol.md`).

## Where it is

The folder is `<workspaces root>/<name>` (the root is `$XDG_DATA_HOME/flottille/workspaces/<project>`
unless the project says otherwise), and each repository's worktree is at the repository's path in
it: `api` at `<folder>/api`, `libs/core` at `<folder>/libs/core`. The root repository `.` is the
folder itself, so the workspace looks like the project folder: a command, a path in the
instructions or a relative link written for the main checkout works the same in a workspace. That
is why the root repository's worktree is made first, and the others inside it (the root repository
ignores them, as it does in the main checkout). A workspace that does not take the root
repository has a plain folder there.

The name is 1 to 64 letters, digits, dots, dashes or underscores, starting with a letter or a
digit. The branch is `<branch prefix>/<name>` unless `--branch` gives one; `git check-ref-format
--branch` must accept it as it is. Creation is refused when
the project has a workspace of that name, when the folder exists, or when the branch exists in one
of the repositories.

## The repositories it takes

The project's default set (`default_repositories`, all of them unless the file says otherwise),
or `--repositories`. They come in dependency order, leaves first, with the root repository first
of all.

The window's New workspace form offers the same: a row per repository of the project, taken or
not (the default set taken, at least one stays taken), with the base branch it starts from (its
rule when the field is left empty, which the field says; `--base`), a branch field (`<branch
prefix>/<name>` when empty), the offline switch (`--offline`), and the optional ticket, previewed
as it is typed (`docs/tickets.md`). The agent's `workspace create` takes `repositories` the same
way, each repository on its rule.

## The base of each branch

For each repository, before anything is written, the base rule (`base = ["develop", "main"]`), or
the one branch chosen for it (`--base`, the form's base field), is read against the remote:

1. `git ls-remote --heads` lists the remote's branches;
2. every branch of the rule that is there is fetched into its tracking ref
   (`+refs/heads/<b>:refs/remotes/<remote>/<b>`), touching no local branch and no checkout of the
   user's;
3. the first one of the rule is the base, at the commit its tracking ref now holds.

**A fetch is never a requirement.** When the repository has no such remote, when listing or
fetching it fails (a network down, an address that leads nowhere), or when none of the rule's
branches is on it, the base is the first branch of the rule the repository has itself
(`refs/heads/<b>`), as it is, and the creation says why in one line, the base's `note`: `no remote
origin: started from the local main`. When the listing or the fetch fails, the same lookup as
`--offline` comes first (the first branch of the rule with a tracking ref, as last fetched), then
the local branches: `origin could not be fetched (<git's reason>): started from origin/develop as
last fetched`, `…: started from the local main`. The CLI prints it on the repository's line, the agent's `workspace create`
says it, and the window shows it under the repository on the workspace's card and in the rail. A
stale cache once based a branch on `main` instead of `develop`: the line is there so that a base
taken without a fetch is never silent. Only a repository with none of its base branches, on the
remote or of its own, is refused.

With `--offline`, nothing is fetched: the first branch of the rule that has a tracking ref is the
base, as it is, and its age is said (from git's reflog of the tracking ref; unknown when git kept
none); with no tracking ref at all, the local branch, as above.

## Preparation

The steps are, in order: one `worktree` step per repository, then the setup as it is at creation,
a step `for_each_repository` expanded into one step per repository. The setup and the project's
variables are **snapshotted** in the store: editing the project's file later never changes the
steps of a workspace that exists.

`workspace create` answers once the worktree steps are done (or one failed); the other steps go on
in the background, one preparation at a time for a workspace. Each step goes `pending`, `running`,
then `done`, `skipped` (its repository is not in the workspace) or `failed`, every change an event
(`docs/events.md`). The first failure stops the preparation and keeps everything done before it;
the step keeps what it was doing and the last 40 lines of its output (at most 8000 characters),
masked for secrets (the values of variables whose name holds `TOKEN`, `SECRET`, `PASS`,
`CREDENTIAL`, `KEY` or `AUTH`, the longest first; private key blocks; the `user:password` of a
URL; JWTs and the tokens of the usual forges, registries and providers). A workspace is
`preparing`, `failed` or `ready`.

| Step | |
|---|---|
| `worktree` | `git worktree prune`, then `git worktree add -b <branch> <folder> <base commit>`, or `git worktree add <folder> <branch>` when an earlier attempt made the branch. Done at once when the worktree is there. |
| `copy` | Copies what the globs match in the main checkout's repository, a folder with what it holds, never over anything already there (`create_new`). A glob matching nothing copies nothing. |
| `link` | A symbolic link to the main checkout's entry, unless something is there already. A missing entry fails the step. |
| `root_entries` | A link to each top-level entry of the project folder that is neither a repository nor holds one, nor `.git` nor `.gitignore` (linking those makes git see the workspace folder as the root repository's checkout with every file deleted), nor holds the workspace, nor is there already. |
| `run` | One line, split without a shell, its words filled, spawned in its repository's worktree (or the workspace folder) with the workspace's environment, in a process group of its own. It ends when its process does: what it left in the background is killed with the group. Past its time limit (`time_limit`, or the project's `run_time_limit`, 30 min by default) the group is killed and the step fails with its last lines and `it was stopped after N s`. A non-zero exit fails the step with its output, or with `it ended with exit N` when it wrote nothing. |

Copies and links are Flottille's own actions, done for the setup the user wrote; they never go
through the agents' gate.

### Resuming

`workspace prepare`:

- puts the `failed` and `running` steps back to `pending`, forgetting their failure;
- puts back a `done` step whose result is gone from the disk: no `.git` for a worktree, something
  left to copy, nothing at a link's place, an entry left to link;
- **never runs a `done` run again**;
- writes `workspace.resumed` with how many steps were redone and retried, then goes on in the
  background.

When the daemon starts, every workspace still `preparing` goes on by itself. A failed one waits
for the user.

### At a glance

`workspace.show` says, for each repository, the branch its worktree is on, its commits ahead of
its base and its files not committed (`docs/protocol.md`). The window shows them with the setup
steps played in it (`setup 2/3`), one line per repository, on the workspace's card of the project
page and in the Workspace section of a session's rail: `app/login · 2 ahead of develop · 3
uncommitted · setup 2/3`, and under it the base's `note` when it started from a local branch.

What the setup put in a worktree is not the user's work: a file a `copy` step wrote, while it is
still byte for byte the main checkout's, counts in neither the lines changed nor the files not
committed, so that a fresh workspace reads `0 uncommitted`. Edited since, it is a change again.
(Removal still refuses a worktree holding such a file: it is not committed.)

## Templates and the environment

The words of a `run` line and the project's variables are templates:

| Name | |
|---|---|
| `{workspace}` | The workspace's name. |
| `{workspace.path}` | Its folder, absolute. |
| `{project}` | The project's name. |
| `{branch}` | The workspace's branch. |
| `{repository}` | The path of the repository the command runs in, `.` for the workspace folder. |

`{{` and `}}` are a literal brace; any other name is refused when the project's file is read.

A command run in a workspace gets the daemon's environment, then the project's variables filled
for it, then the workspace's own (none can be set yet), each overriding the one before. A `PATH`
variable is put in front of the `PATH` before it, never in its place. This replaces an `.envrc`:
for example `PACKAGE_STORE = "{workspace.path}/.store"`, `COMPOSE_PROJECT_NAME = "app-{workspace}"`
and `PATH = "{workspace.path}/.bin"` give each workspace its own package store, its own container
stack and its own wrappers, whatever folder a command runs in.

### A line, without a shell

Flottille never hands a line to a shell. It is split into words on whitespace; a quoted part
(`'…'` or `"…"`) is kept whole and its quotes dropped; a backslash is an ordinary character. The
first word is the program, found on the `PATH`. What a shell would read differently is refused:

- outside single quotes: a new line, `|`, `||`, `&`, `&&`, `;`, a backtick, a redirection (`>`,
  `>>`, `<`, `<<`, `2>&1`…), `$(`, `${`, `$VAR`, `%VAR%`;
- outside any quotes: `*`, `?`, a leading `~`, a `[…]` pattern.

The refusal reads: `Flottille runs a command without a shell, and “T” is shell syntax: put it in a
script of the repository and run the script.` The same rules will hold for the commands agents run.

## Removal

Refused while the workspace is being prepared. Then nothing is touched unless:

- in each worktree made, `git status` shows no changed nor untracked file (`<file> holds work that
  is not committed`); the links Flottille made and the worktrees standing inside the root one are
  not work;
- in each worktree made, `git log HEAD --not --remotes` shows no commit (`<repository> has commits
  on no remote, on its branch <branch>: <hash> <subject>, …`); in a repository with no remote at
  all, no commit is on no other branch (`git log HEAD --not --exclude=<branch> --branches`:
  `<repository> has commits on no other branch, on its branch <branch>: …`), since its work stays
  on its own branches. Commits on no remote are no loss when the repository's forge says the
  branch's pull request is merged and every one of them is in the commit it merged (its head, as
  the forge gives it: GitHub's): a squash-merged branch whose remote branch the forge deleted and
  a fetch pruned. A commit made after, a pull request not merged, a forge that cannot be read or
  none declared still refuse;
- outside the worktrees made, the workspace folder holds nothing but the links Flottille made and
  folders holding nothing else (`<path> is in no worktree`): no `git status` sees a file written in
  the folder of a workspace that does not take the root repository, nor in a repository folder
  whose `.git` is gone, and removing the folder would take it.

Every problem is named in one refusal. Otherwise: the links Flottille made are removed, then each
worktree, the deepest first, with `git worktree remove` (never `--force`), then `git worktree
prune`; the folder is removed (links as links, never followed); Flottille checks the folder is gone
and that `git worktree list` no longer lists it, then forgets the workspace.

Then, in each repository, the workspace's **local** branch is deleted once its work has landed
(`git branch -D`, since `-d` refuses a squash merge), and kept otherwise; the answer says each one
and why (`docs/protocol.md`, `workspace.remove`):

- it holds no commit of its own: every commit of it is in its base (`git merge-base
  --is-ancestor`), and the forge is not asked;
- or the repository's forge says its pull request is merged (a squash merge included, whose
  commits git never finds in the base), and each of its commits is on a remote or in the commit
  the pull request merged: a commit made after the merge and never pushed keeps it.

Anything else keeps it, said: its pull request open, a draft or closed, none, no forge declared,
or a forge that cannot be read (Flottille never deletes on a guess). **The remote branch is never
deleted**, nor anything asked of the forge but the pull request's state. Work that is pushed but
not merged is not lost by the removal: the workspace is removed and its branch kept.

A root agent removes a workspace the same way, with `workspace` `remove`, always held for the
user, its card saying what would be lost (`docs/tools.md`, "The workspace"); never the one it
works in. What would be lost refuses it unless the user's answer said to discard it: then nothing
is checked for loss, each worktree is removed with `git worktree remove --force`, and the folder
with what it held. Its landed branches are then deleted as above, and the others kept, which its
answer says. It is refused, nothing touched, while the
workspace is in use outside the agent's tree (`docs/design.md` §5.1): an agent of another tree not
ended that works in it, or a command run of another tree still running in it, each named; the
tree's own agents and command runs are no obstacle. The user's `flottille workspace remove` does not check
this.

## Git

Always the user's own `git`, spawned without a shell, with `-C <folder>` and `--end-of-options`
before refs and paths, `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never` so that nothing waits
for a password. A call is cut after 30 s (status, rev-parse, log, the fetch) or 30 min (worktree
add, remove, prune), or as soon as it writes more than 32 MB on its output or its errors, killed
with its process group and waited for. Git and the steps never run inside a store transaction,
nor on the daemon's own thread.
