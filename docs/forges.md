# Forges

What the forge of each repository of a workspace says of the workspace's branch: its pull request
and the pull request's checks. The first step of the milestone "Forges and tickets"
(`docs/design.md` §13). **Reading**, and the writes an agent makes with `forge_write` only inside a
procedure that grants them, through the gate ([`procedures.md`](procedures.md)).

GitHub is the only forge read yet. Bitbucket and others will sit beside it: in the code a forge is
one member of `forge::Forge`, read through its own command line.

## The user's own command line

Flottille talks to a forge through the command line the user installed and signed in to: `gh` for
GitHub. It is spawned without a shell, as `git` is, with the daemon's environment (the user's), so
that the user's own sign-in works; `GH_PROMPT_DISABLED=1` keeps it from waiting for an answer, and
a call is cut after 30 s. Every call names the repository (`-R owner/name`, from the remote's
URL): nothing depends on the daemon's working folder.

- **Flottille never reads, keeps nor prints a token.** It never runs `gh auth token` nor
  `--show-token`, never reads `gh`'s configuration folder. Of `gh auth status --hostname
  github.com` it reads the exit code alone: 0 is signed in.
- **What `gh` prints is masked** before it is shown or stored (titles, comments, errors), as every
  output is (`flottille_core::secrets`).
- **Agents never get that environment.** An agent's process and its command runs have the forge
  tokens emptied and `GH_CONFIG_DIR` pointed at an empty folder (`docs/tools.md`); an agent reads
  the forge only through `forge_read` below, never by running `gh` itself.

## The forge of a repository

For each repository of a workspace, the daemon reads the URL of its remote (the repository's
`remote`, `origin` by default) with `git remote get-url`:

| URL | Forge |
|---|---|
| `git@github.com:owner/name.git`, `ssh://git@github.com/owner/name`, `https://github.com/owner/name.git` | GitHub, `owner/name` |
| any other host | none: `no forge support yet: origin is on <host>` |
| a path on this machine | none: `no forge support yet: origin is not on a forge` |
| no such remote | none: `it has no remote origin` |

A repository with no forge support is said as such, never as an error.

## What is read

For each GitHub repository, the pull request of the workspace's branch, the newest when there are
several (`gh pr list -R owner/name --head <branch> --state all --limit 1`), then the pull request
itself (`gh pr view <n> -R owner/name --json …`):

- its number, title, state (`open`, `draft`, `merged`, `closed`), base branch and URL;
- its review decision (`approved`, `changes requested`, `review required`, or none);
- its checks, each with its name and its state: `passing`, `failing`, `pending` or `skipped`;
  and the checks as one mark: `failing` when one fails, else `pending` when one runs, else
  `passing`; none when there are no checks;
- its comments (their number; `forge_read` gives their text, and the reviews').

**`gh` missing or not signed in** is said once for the workspace, in `problem`, with what to do
(install it, `gh auth login`), and in the daemon's log once when it starts being so. Nothing else
is asked of `gh` then, and the rest of Flottille works as before.

## When it is read

- **On demand**: `workspace.forge` (and so `flottille workspace show`, the window's project page)
  answers the last state read when it is less than a minute old, else reads it now; `forge_read`
  always reads now.
- **While a client watches the workspace**: a session page of the window, or `flottille watch
  --forge <project> <workspace>`, asks `workspace.forge` with `watch: true`; from then on, for as
  long as that connection is open, the daemon reads the workspace again about once a minute.
  When a read fails (`gh` missing, not signed in, or a call failing), the next one waits twice as
  long, up to 16 minutes, and back to a minute after a read that works.
- **Never when nobody looks**: no timer runs for a workspace nobody watches.

The state is kept in the daemon's memory, not in the store. When a read finds it different from the
last one (its time aside), the daemon emits `workspace.forge` with the whole state
(`docs/events.md`); a workspace with no forge to read says nothing.

## The protocol and the command line

- `workspace.forge {project, name, watch?}` answers the workspace's forge state
  (`docs/protocol.md`).
- `flottille workspace show <project> <name>` ends with each repository's pull request and its
  checks, and `--json` holds the state under `forge` (`docs/cli.md`).
- `flottille watch --forge <project> <workspace>` watches the workspace while it prints the events.

## The window

The session page's `WORKSPACE` section shows, under each repository, its pull request: its number
and state, and its checks as one mark drawn by shape as well as colour (`✓` passing, `✗` failing,
`●` pending). The project page's place card shows the same for its workspace. A click opens the
pull request with the system's default handler. The session page watches its workspace while it is
open.

## The agent's tool `forge_read`

`forge_read {repository?}` (`docs/tools.md`): the pull requests of the agent's own workspace's
repositories, or of the one named, with their state, base, link, review, checks, comments and
reviews. Read only, through the gate like every call (it allows it). It never reaches a repository
outside the agent's workspace: one the workspace does not take is refused (`refused: X is not a
repository of this agent's workspace: it has …`), and nothing is asked of the forge. Only an
agent working in a workspace has the tool.

## The agent's tool `forge_write`

`forge_write {action, repository, …}` opens the pull request of the workspace's branch, merges it
by squash, or re-runs the failed pipelines of its last commit, with the same `gh` and the same
rules (spawned by the daemon, `-R owner/name`, its output masked, no token to the agent). It is
refused unless the agent works in a procedure's run that grants the action: see
[`procedures.md`](procedures.md).

## Tests

Tests never call GitHub nor use anyone's sign-in: a fake `gh` (`crates/flottille-fake-gh`), first
on the `PATH` of the test's daemon, answers from a script and records every call, and the
repositories are throwaway ones whose `origin` is set to a GitHub URL after their workspace is
made.
