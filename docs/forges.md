# Forges

What the forge of each repository of a workspace says of the workspace's branch: its pull request
and the pull request's checks. The first step of the milestone "Forges and tickets"
(`docs/design.md` §13). **Reading**, and the writes an agent in a workspace makes with `forge_write`, each a call to
judge at the gate ([`procedures.md`](procedures.md)).

GitHub and Bitbucket (Cloud and Data Center) are the forges read. In the code a forge is one member
of `forge::Forge`, read through its own command line (`gh`, `bkt`); others will sit beside them.

## The user's own command line

Flottille talks to a forge through the command line the user installed and signed in to: `gh` for
GitHub, `bkt` for Bitbucket (below). It is spawned without a shell, as `git` is, with the daemon's
environment (the user's), so that the user's own sign-in works; `GH_PROMPT_DISABLED=1` keeps `gh`
from waiting for an answer, and a call is cut after 30 s. Every call names the repository (`-R
github.com/owner/name`, from the repository's declared forge; for `bkt`, `--workspace` or
`--project` and `--repo`): nothing depends on the daemon's working folder. The host is named in
`-R` because `gh` otherwise takes it from `GH_HOST` when the user's environment sets it, which
would send a read or a write to another host than the one the declaration was checked against.

- **Flottille never reads, keeps nor prints a token.** It never runs `gh auth token` nor
  `--show-token`, never reads `gh`'s configuration folder. Of `gh auth status --hostname
  github.com` it reads the exit code alone: 0 is signed in.
- **What `gh` and `bkt` print is masked** before it is shown or stored (titles, comments, errors),
  as every output is (`flottille_core::secrets`).
- **Agents never get that environment.** An agent's process and its command runs have the forge
  tokens emptied and `GH_CONFIG_DIR` and `BKT_CONFIG_DIR` pointed at an empty folder
  (`docs/tools.md`); an agent reads the forge only through `forge_read` below, never by running
  `gh` itself, and **never by running `bkt`, reads included** (see "Bitbucket").

## The forge of a repository

**Each repository declares its forge, and nothing is guessed.** The project's file says it, per
repository (`docs/projects.md`), written by the project's Settings in the window or by hand:

```toml
[[repositories]]
path = "api"
forge = { kind = "github", repository = "acme/api" }
# forge = { kind = "bitbucket-cloud", repository = "acme/api" }
# forge = { kind = "bitbucket-data-center", host = "git.example.com", repository = "PROJ/api" }
```

A repository with no `forge` is never read nor written on a forge: no `gh` nor `bkt` is run for
it, and it is said as `no forge set for this repository: choose it in the project's Settings (the
window), or write its `forge` in the project's file; Flottille never guesses it`, never as an
error. A project registered before forges were declared reads nothing until they are set; its old
`[forges]` table is ignored and said (`… (the project's [forges] table is no longer read)`).

The remote's address (the repository's `remote`, `origin` by default, from `git remote get-url`)
serves two things, and only those:

- **Pre-filling the Settings.** The window proposes, for a repository with no forge, what its
  address says: `github.com` is GitHub `owner/name`, `bitbucket.org` is Bitbucket Cloud
  `workspace/name`, and another host is proposed as Bitbucket Data Center `PROJECT/name` only when
  the address has a Data Center form (`ssh://git@host:7999/PROJECT/name.git`,
  `https://host/scm/PROJECT/name.git`, with a context path before `scm` when the server has one, or
  the `projects/PROJECT/repos/name` page's). Anything else is proposed as no forge. The proposal is
  active only once the user saves.
- **The safety check.** Before anything is read or written, the declared forge must agree with the
  remote: the same host and the same repository path (case aside), for **every** URL of the
  remote, fetch and push (`git remote get-url --all`, and again with `--push`, `insteadOf`
  applied): a `pushurl` or a second `url` that leads elsewhere would send the push there while the
  pull request opens on the declared repository. A repository whose remote leads elsewhere is
  refused with its reason, for example `refused: api is declared on GitHub acme/api, and its
  remote origin: it leads to github.com/other/api, not to GitHub acme/api; nothing was asked of
  the forge: …`; Saving such a forge is refused the same way. A declared forge never silently wins
  over the remote, nor the remote over it. A refusal shows a remote's address without what comes
  before its host (a user, a password, a token) and masked, as every output is, an address
  without a host included.

**An SSH host alias** (`git@gh-work:acme/api.git`, `gh-work` being a `Host` of the user's
`~/.ssh/config`) is resolved the way `ssh` resolves it: when an address reached over SSH has
another host than the declared one, Flottille runs `ssh -G <host>` (spawned without a shell, cut
after 5 s) and reads the `hostname` line of what it prints, nothing else of the SSH
configuration. The declaration is checked against that host; a refusal names both (`… (gh-work
is an SSH alias of gitlab.com)`). When `ssh` is missing or cannot say the host, the alias stays
unresolved and the repository is refused, saying why. A host that is the declared one is never
resolved, and a host that could pass for an option is never given to `ssh`. The window proposes no
forge from an alias: the user chooses it in the Settings, and saving it is checked as above.

A name that could pass for a flag or a path (a leading `-`, anything but letters, digits, `.`, `_`,
`~` and `-`) is no repository, and a host is a name alone (no scheme, port or path).

**A procedure that writes to a forge refuses to start** (`procedure.run`, and a session started
with one) when a repository of its workspace has no forge declared, naming the repositories
(`docs/procedures.md`). `forge_write` refuses the same for the repository it names.

## What is read

For each GitHub repository, the pull request of the workspace's branch, the newest when there are
several (`gh pr list -R github.com/owner/name --head <branch> --state all --limit 1`), then the pull request
itself (`gh pr view <n> -R github.com/owner/name --json …`):

- its number, title, state (`open`, `draft`, `merged`, `closed`), base branch and URL;
- its review decision (`approved`, `changes requested`, `review required`, or none);
- its checks, each with its name and its state: `passing`, `failing`, `pending` or `skipped`;
  and the checks as one mark: `failing` when one fails, else `pending` when one runs, else
  `passing`; none when there are no checks;
- its comments (their number; `forge_read` gives their text, and the reviews').

**`gh` missing or not signed in** is said once for the workspace, in `problem`, with what to do
(install it, `gh auth login`), and in the daemon's log once when it starts being so. Nothing else
is asked of `gh` then, and the rest of Flottille works as before. The same for `bkt` (below): each
command line's problem is said once, and stops only the repositories it would have read, so that a
workspace with a GitHub repository and a Bitbucket one still shows the one that works.

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

## Bitbucket

Read and written through the user's own `bkt`
(<https://github.com/avivsinai/bitbucket-cli>, Go), the same way, and shown the same way (CLI,
window, `forge_read`): a pull request has its number, title, state, base, link, review, checks and
comments, as above, and, for Bitbucket, its reviewers (`reviewers`: each name with `approved`,
`changes requested` or `pending`), which GitHub's reviews say otherwise.

**The release read is v0.9.0**, commit `f70c9c50e4496f1fa2ae40ced62061eeb905a00a`. What follows is
what its source says; a newer release may print otherwise, which the maintainer checks on a real
server (nothing here calls Bitbucket). Files are the repository's, lines those of that commit.

| What Flottille runs | What it reads of the answer | Source |
|---|---|---|
| `bkt context list --json` | `contexts[].name`, `.host`, `.active` (no secret in it) | `pkg/cmd/context/context.go:169-262` |
| `bkt pr list --context C --workspace W --repo R --state all --limit 100 --json` (Cloud), `--project P` instead of `--workspace` (Data Center) | `pull_requests[]`: Cloud `id`, `title`, `state`, `source.branch.name`, `source.repository.full_name`, `destination.branch.name`, `destination.repository.full_name`, `links.html.href`; Data Center `id`, `title`, `state`, `fromRef.displayId`, `fromRef.repository.slug` and `.project.key`, `toRef` the same, `participants[]` (`user`, `role`, `status`, `approved`), `links.self[0].href` | `pkg/cmd/pr/pr.go:78-91` (flags), `:153` and `:218` (`pull_requests`); `pkg/bbcloud/pullrequests.go:14-60` (shape), `:87` (`state=all` is no filter); `pkg/bbdc/client.go:96-122` and `pkg/bbdc/pullrequests.go:17-22` (shape), `pkg/bbdc/client.go:265-267` (`state` as given, uppercased) |
| `bkt pr checks <id> --context C … --json` | `statuses[]`: `state`, `name`, `key` | `pkg/cmd/pr/pr.go:1754-1809` (flags), `:1998` (`statuses`), `:2198` (the states); `pkg/types/commit_status.go:5-11` |
| `bkt api <path> --context C [--param k=v]` (GET) | the server's own JSON, unchanged | `pkg/cmd/api/api.go:23-48`, `:138-140` |
| `bkt pr create --context C … --source <branch> --target <base> --title T --description D [--reviewer N]…` | `✓ Created pull request #<id>` | `pkg/cmd/pr/pr.go:597-615`, `:664`, `:696`; `--reviewer` at `:614`, sent as `{"username": N}` on Cloud (`pkg/bbcloud/pullrequests.go:232-237`) and `{"user": {"name": N}}` on Data Center (`pkg/bbdc/pullrequests.go:75-82`) |
| `bkt pr merge <id> --context C … --strategy squash --close-source=false` | the exit code | `pkg/cmd/pr/pr.go:1314-1334`; `pkg/bbcloud/pullrequests.go:351-388` (`squash` is a Cloud strategy); `pkg/bbdc/pullrequests.go:108-132` (`mergeStrategyId`) |
| `bkt pipeline list --context C --workspace W --repo R --limit 50 --json` | `pipelines[]`: `build_number`, `state.result.name`, `target.ref.name` | `pkg/cmd/pipeline/pipeline.go:177-210`; `pkg/bbcloud/client.go:100-120`, `:145` (newest first: `sort=-created_on`) |
| `bkt pipeline run --context C --workspace W --repo R --ref <branch>` | the exit code | `pkg/cmd/pipeline/pipeline.go:60-91`, `:135-175`; `pkg/bbcloud/client.go:330-357` |

### What is chosen, and why

- **The host comes from a context.** `bkt` has no `-R`: it takes its host from `--context` or else
  from the context the user left active (`pkg/cmdutil/context.go:22-58`), and takes the
  workspace or project and repository from `--workspace`/`--project`/`--repo` first
  (`pkg/cmd/pr/pr.go:110-111`, `:175-176`). Flottille lists the contexts once per read, picks the active one
  when it is on the repository's host, else the first that is, and always names it, so that the
  user's active context never decides which server is asked. A Data Center context's host key
  carries the port (`pkg/cmdutil/url.go:33-42`): it is compared by host name. `bkt` also reads the
  working folder's remote to fill what is missing (`pkg/cmdutil/context.go:191-215`): every call
  here names all of it.
- **Signed in is "a context for the host exists".** `bkt auth status` is not asked: it ends with 0
  whether or not a host is configured, and checks no token (`pkg/cmd/auth/auth.go:314-410`, its
  token source is a label, `:620`). Whether the token itself is there is known at the first call,
  which then fails with `bkt`'s own sentence (`credentials for host … not found; run bkt auth
  login`), shown as that repository's error, masked. With no context for the host the problem
  says what to do: `bkt auth login https://<host>`, then `bkt context create <name> --host
  <host>`. Flottille reads no token, never runs `bkt auth`, and never prints `bkt`'s
  configuration.
- **There is no branch filter.** `bkt pr list` filters by state alone, so the 100 newest pull
  requests are listed (`--state all`) and the workspace's branch is picked among them, the
  highest id when there are several. A branch whose pull request is older than the hundred newest
  of its repository is shown as having none.
- **Reviewers and comments are not in what `bkt` models.** Its pull request structs hold no
  comment and, on Cloud, no participant. They are read with `bkt api`, which passes the server's
  answer through, from Atlassian's REST documentation: Cloud
  `/repositories/{workspace}/{repo}/pullrequests/{id}` (`participants[]`: `role`, `approved`,
  `state`, `user.display_name`) and its `/comments?pagelen=100` (`values[]`: `content.raw`,
  `user.display_name`, `deleted`); Data Center
  `/rest/api/1.0/projects/{key}/repos/{repo}/pull-requests/{id}/activities?limit=100`
  (`values[]` whose `action` is `COMMENTED`: `comment.text`, `comment.author.displayName`,
  replies in `comment.comments`). **These shapes are assumed, and the maintainer checks them**:
  nothing in `bkt`'s source prints them. Only the first 100 comments are read.
- **The review is the reviewers' verdicts as one**: `changes requested` when one reviewer asks
  for it (Cloud `state: changes_requested`, Data Center `status: NEEDS_WORK`), else `review
  required` while one has not decided, else `approved`; none when nobody was asked. It is not
  Bitbucket's merge check.
- **A check is a commit status of the pull request's source commit**, which is where Pipelines
  and the other build tools report: `SUCCESSFUL` is `passing`, `FAILED`, `STOPPED` and the like
  are `failing`, anything else (`INPROGRESS`) is `pending`. Pipelines are on Cloud only
  (`pkg/cmd/pipeline/pipeline.go:20-27`).
- **Draft is not modelled.** A pull request is `open`, `merged` (`MERGED`) or `closed`
  (`DECLINED`, `SUPERSEDED`); `bkt` has no draft flag, so none is shown, and `forge_write` refuses
  `draft: true` for Bitbucket.

### `forge_write` on Bitbucket

The three actions of [`procedures.md`](procedures.md), under the same classes and the same checks
(the remote leads where it led when the workspace was created; the repository is one of the
workspace's; the pull request is the workspace's own branch's, from the repository itself, into
the base the repository started from):

- `pr-create`: `bkt pr create … --source <branch> --target <base>`, a `--reviewer` for each of
  the project's reviewers; it prints no link, so the pull request is read back by its number for
  the answer. A reviewer is the name Bitbucket knows them by (Cloud: the username; Data Center: the
  user's slug).
- `pr-merge-squash`: the open pull request of the branch, not from a fork (Cloud: source and
  destination repositories are the same; Data Center: the same slug and project key), into the
  base: `bkt pr merge <id> --strategy squash --close-source=false`. `--close-source=false` because
  `bkt` closes the source branch by default (`pkg/cmd/pr/pr.go:1334`) and `gh pr merge --squash`
  does not. A Data Center repository whose project does not allow squash answers with the
  server's own refusal, which is the result. Before it, as for GitHub, it is refused while a check
  of the pull request (`bkt pr checks`) fails or runs, or while the review is `changes requested`
  (Data Center: a reviewer's `NEEDS_WORK` in the list; Cloud: a participant's `changes_requested`
  on the pull request's own page, read with `bkt api`), unless the user waives it.
- `pipeline-rerun`: **Cloud only**; `bkt` has no re-run of one pipeline, only `pipeline run --ref
  <branch>`, which runs the branch again. It is done when the newest pipeline of the branch (the
  highest `build_number` whose `target.ref.name` is the branch) ended `FAILED` or `ERROR`;
  otherwise it is refused. A Data Center repository is refused: it has no pipelines for `bkt`.

### Agents never run `bkt`

The gate refuses every `bkt` line an agent runs, reads included, as it does `acli`
(`docs/tools.md`). `bkt`'s token is in the OS keychain, or in an encrypted file under the user's
configuration folder (`internal/secret/store.go:17`, `:295`, `:404`), keyed by the host
(`host/<host>/token`); no variable of ours moves either. `BKT_CONFIG_DIR` moves only the file
that lists hosts and contexts (`internal/config/config.go:245-253`), and a configuration that
names the host is all it takes to use the user's token, `bkt api` with any method included. Unlike
`gh`, whose sign-in an empty configuration folder hides, a `bkt` read would run with the user's own
token. The tokens (`BKT_TOKEN`, `BITBUCKET_*`) and `BKT_CONFIG_DIR` are still emptied from an
agent's environment (`docs/tools.md`), which keeps them out of anything it runs.

## The agent's tool `forge_read`

`forge_read {repository?}` (`docs/tools.md`): the pull requests of the agent's own workspace's
repositories, or of the one named, with their state, base, link, review, checks, comments and
reviews. Read only, through the gate like every call (it allows it). It never reaches a repository
outside the agent's workspace: one the workspace does not take is refused (`refused: X is not a
repository of this agent's workspace: it has …`), and nothing is asked of the forge. Only an
agent working in a workspace has the tool.

## The agent's tool `forge_write`

`forge_write {action, repository, …}` opens the pull request of the workspace's branch (with the
project's reviewers), merges it by squash (never while its checks fail or run or a review asks for
changes, unless the user waives it), or re-runs the failed pipelines of its last commit, with the
same `gh` (or `bkt`, for a Bitbucket repository) and the same rules (spawned by the daemon, the
repository named, its output masked, no token to the agent). Only an agent in a workspace has it, and
each call is to judge (a merge that waives the checks is base): see [`procedures.md`](procedures.md).

## Tickets

The issues of GitHub and Jira, linked to a workspace and read the same way (the user's own command
line, spawned without a shell, nothing written, everything masked), are in
[`tickets.md`](tickets.md). Its `#123` reads through the `gh` described here, and the agent's
`ticket_read` sits beside `forge_read`.

## Tests

Tests never call GitHub nor Bitbucket nor use anyone's sign-in: a fake `gh`
(`crates/flottille-fake-gh`) and a fake `bkt` (`crates/flottille-fake-bkt`), first on the `PATH` of
the test's daemon, answer from a script and record every call, and the repositories are throwaway
ones whose `origin` is set to a GitHub or Bitbucket URL after their workspace is made. A fake `ssh`,
a script first on the same `PATH`, resolves the SSH aliases a test names and serves git's pushes
from the throwaway bare repositories, so that no test reads the user's SSH configuration. The fake
`bkt` answers in the shapes of the table above, needs `--context` and the repository's flags as
the real one does, and is itself tested (`crates/flottille-fake-bkt/tests`).
