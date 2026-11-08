# Workspaces

A workspace is the place one piece of work happens: one git worktree per repository it takes, all
on one branch (the one git says: "The branch"), in one folder, prepared in the background by the project's setup
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
| `flottille workspace remove <project> <name>` | Removes it when all its work is committed and pushed, or integrated into another local branch, then deletes its local branches whose work has landed, never a remote one. Never forced: only the user's answer to an agent's request discards work ("Removal"). |

All of them take `--json` and print what the daemon answered (`docs/protocol.md`).

## Where it is

The folder is `<workspaces root>/<name>` (the root is the project's own, else the user's default,
else `$XDG_DATA_HOME/flottille/workspaces/<project>`: `docs/projects.md`, "Where the workspaces go"; a workspace keeps the
folder it was made in whatever they become), and each repository's worktree is at the repository's path in
it: `api` at `<folder>/api`, `libs/core` at `<folder>/libs/core`. The root repository `.` is the
folder itself, so the workspace looks like the project folder: a command, a path in the
instructions or a relative link written for the main checkout works the same in a workspace. That
is why the root repository's worktree is made first, and the others inside it (the root repository
ignores them, as it does in the main checkout). A workspace that does not take the root
repository has a plain folder there.

The name is 1 to 64 letters, digits, dots, dashes or underscores, starting with a letter or a
digit, and is not `main`: `main` names the project folder where a session for the user is
started (`agent_launch` with `root`, `"workspace": "main"`, `docs/tools.md`, "Sessions for the
user"), so creating a workspace under it is refused from the agent's `workspace create`, the
command line, the window and the protocol alike (`main is reserved: it names the project folder,
where a session works without a workspace; choose another name for the workspace`). A project
that holds a workspace named `main`, made before the name was reserved, keeps it, with no
migration: there `"workspace": "main"` names that workspace, as before, and the card says `the
workspace main`. The branch is `<branch prefix>/<name>` unless `--branch` gives one, and the name
alone when the project's branch prefix is empty (`docs/projects.md`); `git check-ref-format
--branch` must accept it as it is. A repository that names its branches after tickets
(`fix/ABC-12-login`) is followed by naming the branch: the agent's `workspace create` says to give
`branch` when the repository has a naming convention (its `AGENTS.md`, `CONTRIBUTING`, or the
branches it already has), with the ticket's key when the workspace has a ticket; the default is
the fallback. A session started in a new workspace (`agent_launch` with `new_workspace`) names it
the same way, with `branch` (and `base`, as `--base` does): its card says the branch, and that is
the branch made. When the project has a branch format (`docs/projects.md`), a branch the workspace
starts must match it, named or the default, or creation is refused naming the pattern; a branch
every repository resumes is taken as it is. Creation is refused when the project has a workspace of that
name, when the folder exists, or when the branch is checked out in a checkout of one of the
repositories, the main one or another worktree (`the branch feature/x is checked out in <folder>
for web: …`, naming it).

**To resume a branch, name it.** A branch that already exists in a repository is checked out in
its worktree instead of being started: the repository's own (`git worktree add <folder> <branch>`),
or, when only its remote has it (`git ls-remote --heads`, then fetched into its tracking ref; as
last fetched when offline or when the remote cannot be listed), a local branch tracking the
remote's (`git worktree add --track -b <branch> <folder> <remote>/<branch>`). The other
repositories start it from their base. The base of a repository that resumed its branch is only
what the branch is compared against (commits ahead, the pull request's target), never where it
starts. Only a branch named is resumed: under the default name, a branch the repository already has
is a leftover of an earlier workspace, and creation is refused (`the branch app/login already exists
in web: choose another name or branch`). A resumed branch was there before the workspace: removing
the workspace never deletes it.

## The branch

The branch of a repository is the one its worktree is on now (`git symbolic-ref --short HEAD`),
not the one recorded when the workspace was made: an agent or the user may `git switch` in it.
Every place that uses it reads it there: `workspace.show`, the pull requests read and written
(`docs/forges.md`), and the removal's checks. When every worktree made is on one branch other than
the recorded one, the record takes it and `workspace.changed` says so (`docs/events.md`), so the
window and `workspace list` show it; repositories on different branches leave the record as it
is, and each is read and written for its own. To work on another branch in a workspace, switch
with git: the workspace follows. A repository on no branch (a detached HEAD) is read for the
recorded branch, and `forge` `create` refuses it (`refused: <repository> is on no branch (detached
at <hash>): switch to a branch first`).

### Renaming the branch

An agent that made a workspace on a branch which does not follow the repository's convention
(`fix/ABC-12-login`) renames it before the first push: `workspace` `rename_branch {branch,
repository?}` (`docs/tools.md`, "The workspace"; `workspace.rename_branch`, `docs/protocol.md`).
It runs `git branch -m` in the worktree of `repository`, or of each repository of the workspace,
and the record follows as for a `git switch` (`workspace.changed`); the window's rail says the new
name, fading in. It is all or none, refused before anything is renamed when:

- the workspace is being prepared;
- the new name does not match the project's branch format (`docs/projects.md`), naming it;
- the new name is not one `git check-ref-format --branch` accepts, is the branch a worktree is on
  already, exists in a repository, or is checked out in a checkout of one (the main one or
  another worktree);
- a repository's branch was **resumed** by the workspace: it was the user's before it;
- a repository's branch was **pushed**: a remote has it (its tracking ref as last fetched, or what
  the remote lists; a remote that cannot be listed is taken for not having it), or it has a pull
  request (any state): renaming would leave the remote branch or the pull request on the old
  name;
- a worktree is on no branch.

Each refusal names the repository and says which of these it is. Renaming in one repository of
several leaves the record as it is: the repositories are on different branches.

## The repositories it takes

The project's default set (`default_repositories`, all of them unless the file says otherwise),
or `--repositories`. They come in dependency order, leaves first, with the root repository first
of all.

The window's New workspace form offers the same: a row per repository of the project, taken or
not (the default set taken, at least one stays taken), with the base branch it starts from (its
rule when the field is left empty, which the field says; `--base`), a branch field (`<branch
prefix>/<name>`, or the name alone without a prefix, when empty; "To resume a branch, name it."), the offline switch (`--offline`), and the optional ticket, previewed
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
the step keeps what it was doing and the last 40 lines of its output (at most 8000 characters; its
whole output, up to 64 KiB, is read with `workspace.step_output`),
masked for secrets (the values of variables whose name holds `TOKEN`, `SECRET`, `PASS`,
`CREDENTIAL`, `KEY` or `AUTH`, the longest first; private key blocks; the `user:password` of a
URL; JWTs and the tokens of the usual forges, registries and providers). A workspace is
`preparing`, `failed` or `ready`.

| Step | |
|---|---|
| `worktree` | `git worktree prune`, then `git worktree add -b <branch> <folder> <base commit>`, or `git worktree add <folder> <branch>` when the branch exists (resumed, or made by an earlier attempt), or `git worktree add --track -b <branch> <folder> <remote>/<branch>` when only the remote had it. Done at once when the worktree is there. |
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

Refused while the workspace is being prepared or removed, the refusal saying which, and while
something works in it (`docs/design.md` §5.1): an agent not ended or a command run still running,
each named (`the workspace login of project app is not removed, nothing was touched: the agent
1a2b3c4d5e6f works in it; they end first`); it is checked again once the checks below are done,
before anything is touched. From its start to its end the removal holds the workspace: no agent
starts in it (`workspace login of project app is being removed: wait until it is done`), nor a
preparation. Then nothing is touched unless:

- in each worktree made, `git status` shows no changed nor untracked file (`<file> holds work that
  is not committed`); the links Flottille made and the worktrees standing inside the root one are
  not work;
- in each worktree made, `git log HEAD --not --remotes` shows no commit (`<repository> has commits
  on no remote, on its branch <branch>: <hash> <subject>, …`, the branch it is on now, or `on no
  branch (detached at <hash>)`); in a repository with no remote at
  all, no commit is on no other branch (`git log HEAD --not --exclude=<branch> --branches`:
  `<repository> has commits on no other branch, on its branch <branch>: …`), since its work stays
  on its own branches. When the worktree is on another branch than the recorded one and the
  recorded one is still a local branch, it is checked the same way; Commits on no remote are no loss when the repository's forge says the
  branch's pull request is merged and every one of them is in the commit it merged (its head, as
  the forge gives it: GitHub's `headRefOid`, Bitbucket's source commit): a squash-merged branch
  whose remote branch the forge deleted and a fetch pruned. Commits on no remote are no loss
  either when another local branch of the repository holds them: the branch is **integrated**,
  merged into or contained in that branch (`git merge-base --is-ancestor HEAD <branch>`, for
  each local branch but its own, by full ref name, and but a symbolic ref, which may point at its
  own). Any local branch counts, even one never pushed: that branch keeps the commits. Removals
  delete branches one at a time, so that two branches holding the same commits are never both
  deleted, each counted as kept in the other. A commit made after, a pull request not merged, a forge that
  cannot be read or none declared still refuse; uncommitted files refuse, integrated or not;
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
- or it is contained in another local branch (`git merge-base --is-ancestor`), its work
  integrated there: `it is integrated into <branch>`;
- or the repository's forge says its pull request is merged (a squash merge included, whose
  commits git never finds in the base), and each of its commits is on a remote or in the commit
  the pull request merged: a commit made after the merge and never pushed keeps it.

Anything else keeps it, said: its pull request open, a draft or closed, none, no forge declared,
or a forge that cannot be read (Flottille never deletes on a guess). **The remote branch is never
deleted**, nor anything asked of the forge but the pull request's state. Work that is pushed but
not merged is not lost by the removal: the workspace is removed and its branch kept.

A root agent removes a workspace the same way, with `workspace` `remove`, always held for the
user, its card saying what would be lost (`docs/tools.md`, "The workspace"); the one it works in
too, once its work is done ("Its own workspace", below). What would be lost refuses it unless the user's answer said to discard it: then nothing
is checked for loss, each worktree is removed with `git worktree remove --force`, and the folder
with what it held. Its landed branches are then deleted as above, and the others kept, which its
answer says. It is refused, nothing touched, while the
workspace is in use outside the agent's tree (`docs/design.md` §5.1): an agent of another tree not
ended that works in it, or a command run of another tree still running in it, each named; the
tree's own command runs are no obstacle, nor its agents that ended; its other agents not ended
are.

### Its own workspace

A root removes the workspace it works in when its work is done (a child removes none), held for
the user whatever the mode, Full included. Its card says, besides what would be lost, the commands
of its tree running in it that are stopped (`Stop the commands of this session running in it:
pnpm dev:api, pnpm dev:front`) and that the session then goes on in the project's folder. Once
the user allows it, in this order:

1. what the removal of any workspace refuses, nothing touched: another agent not ended works in
   it, of the tree or not, or a command run of another tree runs there; it is being prepared;
2. the tree's command runs in it are stopped, as `run.stop` stops one;
3. the checks and the removal above: refused while something would be lost, unless the answer
   said to discard it; the use of 1 is checked again once these checks are done, before
   anything is touched;
4. in the one store write that forgets the workspace, the agent's place becomes the project's
   folder (`main`), its instructions saying so for its next process, with `workspace.removed`
   and `agent.moved` (`docs/events.md`); its running session's grant works in `main` at once;
   the requests it made that still wait for an answer are cancelled in that write
   (`approval.cancelled`: their cards leave the window), since they were asked in the workspace
   and would otherwise be done in `main`, and each is answered to the agent: `Request #12 (run)
   was not run: you moved from the workspace login to the project's folder (main); ask again if
   it is still needed.`;
5. the answer tells the agent: `The workspace login is removed; you now work in the project's
   folder (main), <folder>`.

From 1 to its end, the session starts nothing in the workspace: its `run`, `agent_launch`,
`fs_write` and `fs_edit` are refused (`refused: the workspace login is being removed: wait for the
removal's answer`), and a request of these the user allows meanwhile is not executed. A request keeps the place its agent asked it in (`main`, or
`workspace <name>`): one that a slow judgement brings back after the move is refused, and one
allowed once its agent works elsewhere is not run (`not run: asked in the workspace login, which
you have left; ask again if it is still needed`). The user's
answer holds to what the card said would be lost, not to the commands it named: one that ended,
or started, between the card and the answer loses nothing, and is no reason to ask again.

A refusal at 1 or 3 leaves the session in its workspace (its commands stopped at 2 stay stopped).
A removal that fails halfway (a worktree git cannot remove) keeps the workspace's record, and the
agent in it, as it keeps it for the user's removal; the agent is moved only by the write that
forgets the workspace, so it is never left with no place. Its process goes on: its folder is gone
from under it, but it reaches nothing but through Flottille's tools, which work in `main`; the
next one starts in the project's folder.

## Git

Always the user's own `git`, spawned without a shell, with `-C <folder>` and `--end-of-options`
before refs and paths, `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never` so that nothing waits
for a password. A call is cut after 30 s (status, rev-parse, log, the fetch) or 30 min (worktree
add, remove, prune), or as soon as it writes more than 32 MB on its output or its errors, killed
with its process group and waited for. Git and the steps never run inside a store transaction,
nor on the daemon's own thread.
