# Forges

What the forge of each repository of a workspace says of the workspace's branch: its pull request
and the pull request's checks. The first step of the milestone "Forges and tickets"
(`docs/design.md` §13). **Reading**, and the writes an agent makes with `forge`, each a call to
judge at the gate ([`commands.md`](commands.md)).

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
  (`docs/tools.md`); an agent reads the forge only through `forge` below, never by running
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

**`forge` refuses to write for a repository with no forge declared**, naming it (`docs/commands.md`).

## What is read

For each GitHub repository, the pull request of the workspace's branch, the newest when there are
several (`gh pr list -R github.com/owner/name --head <branch> --state all --limit 1`), then the pull request
itself (`gh pr view <n> -R github.com/owner/name --json …`):

- its number, title, state (`open`, `draft`, `merged`, `closed`), base branch and URL;
- its head branch, and whether it is mergeable: `mergeable` says `MERGEABLE` or `CONFLICTING`
  (anything else, `UNKNOWN` while GitHub computes it, is not known yet);
- its review decision (`approved`, `changes requested`, `review required`, or none);
- its checks, each with its name and its state: `passing`, `failing`, `pending` or `skipped`;
  and the checks as one mark: `failing` when one fails, else `pending` when one runs, else
  `passing` (skipped ones count as passing in the mark, but a merge needs one that passed: a pull
  request whose checks were all skipped is refused like one with none); none when there are no
  checks;
- its comments (their number; `forge` `show` gives their text, and the reviews').

**`gh` missing or not signed in** is said once for the workspace, in `problem`, with what to do
(install it, `gh auth login`), and in the daemon's log once when it starts being so. Nothing else
is asked of `gh` then, and the rest of Flottille works as before. The same for `bkt` (below): each
command line's problem is said once, and stops only the repositories it would have read, so that a
workspace with a GitHub repository and a Bitbucket one still shows the one that works.

## When it is read

- **On demand**: `workspace.forge` (and so `flottille workspace show`, the window's project page)
  answers the last state read when it is less than a minute old, else reads it now; `forge` `show`
  always reads now.
- **While a client watches the workspace**: a session page of the window, or `flottille watch
  --forge <project> <workspace>`, asks `workspace.forge` with `watch: true`; from then on, for as
  long as that connection is open, the daemon reads the workspace again about once a minute.
  When a read fails (`gh` missing, not signed in, or a call failing), the next one waits twice as
  long, up to 16 minutes, and back to a minute after a read that works.
- **While an agent subscribes to its pull request** (`forge` `subscribe`, "Subscriptions" below):
  the subscription holds a watch of the workspace, the same reads, until it is over.
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
window, `forge`): a pull request has its number, title, state, base, link, review, checks and
comments, as above, its head branch and whether it is mergeable, and, for Bitbucket, its reviewers (`reviewers`: each name with `approved`,
`changes requested` or `pending`), which GitHub's reviews say otherwise.

**The release read is v0.9.0**, commit `f70c9c50e4496f1fa2ae40ced62061eeb905a00a`. What follows is
what its source says; a newer release may print otherwise, which the maintainer checks on a real
server (nothing here calls Bitbucket). Files are the repository's, lines those of that commit.

| What Flottille runs | What it reads of the answer | Source |
|---|---|---|
| `bkt context list --json` | `contexts[].name`, `.host`, `.active` (no secret in it) | `pkg/cmd/context/context.go:169-262` |
| `bkt pr list --context C --workspace W --repo R --state all --limit 50 --json` (Cloud), `--project P` instead of `--workspace` (Data Center) | `pull_requests[]`: Cloud `id`, `title`, `state`, `source.branch.name`, `source.commit.hash`, `source.repository.full_name`, `destination.branch.name`, `destination.repository.full_name`, `links.html.href`; Data Center `id`, `title`, `state`, `fromRef.displayId`, `fromRef.latestCommit`, `fromRef.repository.slug` and `.project.key`, `toRef` the same, `participants[]` (`user`, `role`, `status`, `approved`), `links.self[0].href` | `pkg/cmd/pr/pr.go:78-91` (flags), `:153` and `:218` (`pull_requests`); `pkg/bbcloud/pullrequests.go:14-60` (shape, `source.commit.hash` at `:41-43`), `:87` (`state=all` is no filter); `pkg/bbdc/client.go:96-122` and `pkg/bbdc/pullrequests.go:17-22` (shape, `Ref.LatestCommit` at `client.go:117-123`), `pkg/bbdc/client.go:265-267` (`state` as given, uppercased) |
| `bkt pr checks <id> --context C … --json` | `statuses[]`: `state`, `name`, `key` | `pkg/cmd/pr/pr.go:1754-1809` (flags), `:1998` (`statuses`), `:2198` (the states); `pkg/types/commit_status.go:5-11` |
| `bkt api <path> --context C [--param k=v]` (GET) | the server's own JSON, unchanged | `pkg/cmd/api/api.go:23-48`, `:138-140` |
| `bkt pr create --context C … --source <branch> --target <base> --title T --description D [--reviewer N]…` | `✓ Created pull request #<id>` | `pkg/cmd/pr/pr.go:597-615`, `:664`, `:696`; `--reviewer` at `:614`, sent as `{"username": N}` on Cloud (`pkg/bbcloud/pullrequests.go:232-237`) and `{"user": {"name": N}}` on Data Center (`pkg/bbdc/pullrequests.go:75-82`) |
| `bkt pr merge <id> --context C … --strategy S --close-source=false` | the exit code | `pkg/cmd/pr/pr.go:1314-1334`; `pkg/bbcloud/pullrequests.go:351-388` (Cloud takes `merge_commit`, `squash` and `fast_forward` only, `:352-356`, `:363-365`); `pkg/bbdc/pullrequests.go:108-132` (`mergeStrategyId`, given as it is) |
| `bkt api /repositories/W/R/pipelines/?sort=-created_on --context C --param pagelen=50` (GET) | `values[]`: `build_number`, `state.result.name`, `target.type`, then `target.ref_name` (a branch's pipeline) or `target.source` (a pull request's: `pipeline_pullrequest_target`), `target.selector.pattern` | Atlassian's REST documentation (Pipelines, list pipelines). Not `bkt pipeline list`: it decodes `target.ref.name` (`pkg/bbcloud/client.go:140-152`) while the API sends `ref_name`, so every branch it prints is empty (seen live with v0.37.0), and a pull request's pipeline has no `ref` at all |
| `bkt pipeline run --context C --workspace W --repo R --ref <branch> [--selector-type pull-requests --selector-pattern <p>]` | the exit code | `pkg/cmd/pipeline/pipeline.go:81-133` (v0.37.0; the selector flags at `:127-128`), `pkg/bbcloud/client.go:431-482` (`ref_type: branch`, `ref_name`, `selector`) |
| `bkt pipeline view <build> --context C --workspace W --repo R --json` | `steps[]`: `uuid`, `state.result.name` | `pkg/cmd/pipeline/pipeline.go:416-480` (v0.37.0; `{pipeline, steps}`), `pkg/bbcloud/client.go:559-604` (`PipelineStep`) |
| `bkt pipeline logs <build> --context C --workspace W --repo R --step <uuid>` | the step's log, as text | `pkg/cmd/pipeline/pipeline.go:679-726` (v0.37.0), `pkg/bbcloud/client.go:692-725` (Cloud only) |

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
- **A page is 50 at most.** Bitbucket Cloud answers `400 Bad Request: Invalid pagelen` to a
  `pagelen` above the maximum of an endpoint, and its pull request list takes 50 (seen with `bkt`
  v0.37.0, which passes `--limit` as `pagelen`, follows `next` only up to that limit, and so can
  read no more than one page of 50). Atlassian's REST documentation gives 100 as the global
  maximum, says "some APIs" differ, and names none for the comments and the diffstat
  (<https://developer.atlassian.com/cloud/bitbucket/rest/intro/#pagination>): every page Flottille
  asks of Bitbucket is 50, and where it needs more it follows the `next` link of Cloud (to
  `api.bitbucket.org` only) or the `nextPageStart` of Data Center, up to the totals below. The
  fake `bkt` of the tests refuses a Cloud `pagelen` above 50 as Bitbucket does.
- **There is no branch filter.** `bkt pr list` filters by state alone, so the 50 newest pull
  requests are listed (`--state all`) and the workspace's branch is picked among them, the
  highest id when there are several. A branch whose pull request is older than the fifty newest
  of its repository is shown as having none, `forge list` says its list may be cut (`Only the
  newest 50 pull requests are read…`), and `forge show` of a number they do not hold says the
  pull request may be older instead of saying it does not exist.
- **The head commit is the source commit.** `source.commit.hash` (Cloud, which may say it cut short:
  Atlassian's own examples show twelve characters, so the shape is assumed) and `fromRef.latestCommit` (Data Center) are read from the list, and
  kept only as a hex hash of at least 7 characters, as GitHub's `headRefOid` is: it is given to
  git as a revision. A removed workspace's squash-merged branch, whose remote branch the forge
  deleted and a fetch pruned, is then covered by its merged pull request (`docs/workspaces.md`).
  Without that field, or with anything else, the removal refuses as it does with no forge.
- **Reviewers and comments are not in what `bkt` models.** Its pull request structs hold no
  comment and, on Cloud, no participant. They are read with `bkt api`, which passes the server's
  answer through, from Atlassian's REST documentation: Cloud
  `/repositories/{workspace}/{repo}/pullrequests/{id}` (`participants[]`: `role`, `approved`,
  `state`, `user.display_name`) and its `/comments?pagelen=50` (`values[]`: `content.raw`,
  `user.display_name`, `deleted`); Data Center
  `/rest/api/1.0/projects/{key}/repos/{repo}/pull-requests/{id}/activities?limit=50`
  (`values[]` whose `action` is `COMMENTED`: `comment.text`, `comment.author.displayName`,
  replies in `comment.comments`). **These shapes are assumed, and the maintainer checks them**:
  nothing in `bkt`'s source prints them. Only the first 100 comments (Data Center: activities) are read, two pages of 50.
- **Mergeable is read the same way, assumed too**: on Cloud, the pull request has a conflict when
  a file of its `/diffstat?pagelen=50` (`values[].status`) is `merge conflict`, and when its
  first 500 files (ten pages of 50) have a `next` still and no conflict, it is not known; on Data
  Center, when its `/rest/api/1.0/projects/{key}/repos/{repo}/pull-requests/{id}/merge` says
  `conflicted`. When that read fails, it is not known, and the rest of the pull request is shown.
- **The review is the reviewers' verdicts as one**: `changes requested` when one reviewer asks
  for it (Cloud `state: changes_requested`, Data Center `status: NEEDS_WORK`), else `review
  required` while one has not decided, else `approved`; none when nobody was asked. It is not
  Bitbucket's merge check.
- **A check is a commit status of the pull request's source commit**, which is where Pipelines
  and the other build tools report: `SUCCESSFUL` is `passing`, `FAILED`, `STOPPED` and the like
  are `failing`, anything else (`INPROGRESS`) is `pending`. Pipelines are on Cloud only
  (`pkg/cmd/pipeline/pipeline.go:20-27`).
- **Draft is not modelled.** A pull request is `open`, `merged` (`MERGED`) or `closed`
  (`DECLINED`, `SUPERSEDED`); `bkt` has no draft flag, so none is shown, and `forge` `create` refuses
  `draft: true` for Bitbucket.

### `forge`'s writes on Bitbucket

The three actions of [`commands.md`](commands.md), under the same classes and the same checks
(the remote leads where it led when the workspace was created; the repository is one of the
workspace's; the pull request is the workspace's own branch's, from the repository itself, into
the base the repository started from):

- `create`: `bkt pr create … --source <branch> --target <base>`, a `--reviewer` for each of
  the project's reviewers; it prints no link, so the pull request is read back by its number for
  the answer. A reviewer is the name Bitbucket knows them by (Cloud: the username; Data Center: the
  user's slug).
- `merge`: the open pull request of the branch, not from a fork (Cloud: source and
  destination repositories are the same; Data Center: the same slug and project key), into the
  base: `bkt pr merge <id> --strategy S --close-source=false`, `S` the method's strategy: `squash`
  is `squash` on both; `merge` is `merge_commit` on Cloud and `no-ff` on Data Center; `rebase` is
  `rebase-ff-only` on Data Center (rebased onto the base, no merge commit, as GitHub's), and is
  refused on Cloud before anything is asked, since `bkt` takes no rebase strategy there.
  `--close-source=false` because `bkt` closes the source branch by default
  (`pkg/cmd/pr/pr.go:1334`) and `gh pr merge` does not. When the server refuses the merge, the
  strategies the repository allows are read (Cloud: `destination.branch.merge_strategies` of the
  pull request's own page; Data Center: the `enabled` ones of `mergeConfig.strategies[].id` in
  `/rest/api/1.0/projects/{key}/repos/{repo}/settings/pull-requests`, both through `bkt api`,
  **assumed shapes**) and, when the method is not among them, the refusal names the methods they
  allow, then the strategies it allows that no method reaches (`ff`, `squash-ff-only`,
  `fast_forward`…) as it names them; otherwise the server's own refusal is the result. Before it, as for GitHub, it is refused
  when no check ran on the pull request (`bkt pr checks` lists none, or answers `"statuses": null`
  for a commit with no status; an answer without `statuses` is not that: the checks could not be
  read, and nothing is merged), while a check of the pull
  request fails or runs, or while the review is `changes requested`
  (Data Center: a reviewer's `NEEDS_WORK` in the list; Cloud: a participant's `changes_requested`
  on the pull request's own page, read with `bkt api`), unless the user waives it.
- `checks`: **Cloud only**; `bkt` has no re-run of one pipeline, only `pipeline run`, which starts
  one again. The newest 50 pipelines of the repository are read from the API (`sort=-created_on`,
  see the table), and the branch's are the ones whose `target.ref_name` is the branch (pushed) or
  whose `target.source` is (a pull request's). It is done when the newest of them (the highest
  `build_number`) ended `FAILED` or `ERROR`; otherwise it is refused. A branch's pipeline is run
  again with `--ref <branch>`; a pull request's with `--ref <branch> --selector-type pull-requests
  --selector-pattern <its target.selector.pattern>` (`**` when it says none). **Assumed, and the
  maintainer checks it**: that Bitbucket runs the pull request's pipeline for a branch trigger
  with that selector. A branch whose pipelines are older than the 50 newest of the repository is
  refused as having none. A Data Center repository is refused: it has no pipelines for `bkt`.
- `comment` and `edit` of a pull request by its number: `bkt pr comment <id> --text <body>` and
  `bkt pr edit <id> [--title T] [--description D]` (`pkg/cmd/pr/pr.go:1654-1669` and `:719-762`,
  which print `✓ Commented on pull request #<id>` and `✓ Updated pull request #<id>`); `bkt pr
  edit` refuses a call with neither, and Flottille refuses it before.
- `close` of a pull request by its number: `bkt pr decline <id> [--comment T] [--delete-source]`
  (v0.37.0, `pkg/cmd/pr/pr.go`, `newDeclineCmd`; alias `close`; it prints `Declined pull request
  #<id>`). `--delete-source` is Data Center's only: on Cloud `bkt` ends with `--delete-source is
  not supported for Bitbucket Cloud`, so Flottille refuses `delete_branch` there before declining.
  Its state is read first (`bkt pr list --state all`): only an open pull request is declined.
  `reopen` is refused without a call: Bitbucket cannot reopen a declined pull request, and `bkt`
  has no command for it.
- `list`: `bkt pr list --state OPEN | MERGED | DECLINED | all` (`closed` is `DECLINED`), the
  newest 50, the branch picked from them; `show` with a number finds it among them.

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

## The agent's tool `forge`

`forge {action, …}` (`docs/tools.md`, "The forge"): `list` the pull requests of the place's
repositories by state and branch, `show` a workspace's branch's or one by its number with its
checks, reviews and comments; `create`, `merge`, `checks`, `comment` and `edit`, each to judge (a
merge that waives the checks base). The same `gh` (or `bkt`, for a Bitbucket repository) and the
same rules: spawned by the daemon, the repository named, its output masked, no token to the agent.
It never reaches a repository outside the agent's place, and is listed to every agent of a
project whose repository declares a forge, in `main` as in a workspace. Until #141 it was
`forge_read` and `forge_write`.

## Why a check failed

A pull request that is "failing" tells an agent nothing it can fix. So `forge` `show`, of a
workspace's branch as of a number, says after the checks, for **each failed check**, the end of its
log, or why there is none:

```
  failed jobs (1):
  - test: the last 5 of 212 lines of its log:
    test login::form ... FAILED
    …
  - ci/outside: no log: it is not a GitHub Actions job, see https://ci.example.com/builds/5
```

- **GitHub**: a failed check run whose link is a GitHub Actions job
  (`…/actions/runs/<run>/job/<job>`, `detailsUrl` of `statusCheckRollup`) is read with `gh run
  view --job <job> --log-failed -R github.com/owner/name`, which prints the failed steps of that
  job. A commit status of another CI, a check with no such link, and a log that `gh` cannot read
  (expired, not ready) say so (`no log: …`); the pull request is still shown.
- **Bitbucket Cloud**: a failed check whose link is a Pipelines build of the repository
  (`https://bitbucket.org/<workspace>/<repo>/pipelines/results/<build>`, the `url` of the commit
  status) names the build number, which is all `bkt` needs. The first failed step is found with
  `bkt pipeline view <build> --json` (`steps[].state.result.name` is `FAILED` or `ERROR`) and its
  log is read with `bkt pipeline logs <build> --step <uuid>`. A status of another CI, or of
  another repository, says so (`no log: it is not a Bitbucket Pipelines build, see …`). Its lines
  have no names nor timestamp to drop.
- **Bitbucket Data Center**: `no log: bkt gives no job logs, read it on Bitbucket`; `bkt` has no
  pipelines there.
- **Cleaned**: the job's and step's names and the timestamp that start every line, the colours and
  the carriage returns of a terminal are dropped; the empty lines at both ends too.
- **Bounded**: the last 40 lines of a job, 4,000 bytes at most (the oldest lines go first, whole;
  one line longer than that keeps its end), the logs of 5 jobs at most and 12,000 bytes in all.
  What is left out is said (`the last 40 of 212 lines`, `no log: at most 5 logs and 12000 bytes
  are given`), never silently dropped.
- **Masked** like every output, before it is kept or shown.
- **Only `forge` `show` asks for it.** The state the window watches (`workspace.forge`, `workspace
  show`) holds no log and calls `gh` no more than before.

## Subscriptions

The forge watch is the first source of events of the subscriptions (`docs/design.md` §5.6). After
each read of a workspace, every subscription to one of its pull requests compares it with what it
saw last, stored with it (the state, whether a check was running, the head commit, a hash of each
comment and review, Bitbucket's reviewers' verdicts):

- `checks`: none runs now, and one was running or the head commit changed; the delivery says how
  many passed, failed and were skipped, and the end of each failed job's log, read then
  ("Why a check failed");
- `review`: a review that was not there (GitHub: its state, not `pending`; Bitbucket: a
  reviewer's new verdict), with its author, state and an excerpt;
- `comment`: a comment that was not there, with its author and an excerpt;
- `merged`, `closed`: the pull request's state became that; either ends every subscription to it.

The first read after subscribing is the one the subscription saw: what is already so is no event.
A read that fails changes nothing. **A bot**, for `from: people`, is a GitHub author whose
login starts with `app/` (as `gh` says an app) or ends with `[bot]`, or that `gh` says `is_bot`;
this is assumed from `gh`'s output, and the maintainer checks it on a real pull request. A
Bitbucket author is always taken for a person: what says a bot there is not read. **The agent's
own** comment is one it posted with `forge` `comment` on that pull request, the same text,
remembered in the daemon's memory (the last 200): after a restart, it counts as a person's.

## Tickets

The issues of GitHub and Jira, linked to a workspace and read the same way (the user's own command
line, spawned without a shell, nothing written, everything masked), are in
[`tickets.md`](tickets.md). Its `#123` reads through the `gh` described here, and the agent's
`tickets` sits beside `forge`.

## Tests

Tests never call GitHub nor Bitbucket nor use anyone's sign-in: a fake `gh`
(`crates/flottille-fake-gh`) and a fake `bkt` (`crates/flottille-fake-bkt`), first on the `PATH` of
the test's daemon, answer from a script and record every call, and the repositories are throwaway
ones whose `origin` is set to a GitHub or Bitbucket URL after their workspace is made. A fake `ssh`,
a script first on the same `PATH`, resolves the SSH aliases a test names and serves git's pushes
from the throwaway bare repositories, so that no test reads the user's SSH configuration. The fake
`bkt` answers in the shapes of the table above, needs `--context` and the repository's flags as
the real one does, and is itself tested (`crates/flottille-fake-bkt/tests`).
