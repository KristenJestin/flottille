# Forges

What the forge of each repository of a workspace says of the workspace's branch: its pull request
and the pull request's checks. A repository's branch is the one its worktree is on now, not the
one recorded at creation ([`workspaces.md`](workspaces.md), "The branch"): what is read, opened,
merged and checked follows a `git switch`. The first step of the milestone "Forges and tickets"
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

## The programs it needs

Each program is checked where the forge is chosen, as the providers' are (`docs/agents.md`): the
project's Settings and `flottille doctor` say, for the forge of each repository and for the tracker,
whether `gh`, `bkt` or `acli` is **found** (on the daemon's PATH, with the version it prints for
`--version`, given 5 s) and whether it is **signed in**, by the checks Flottille already uses and no
other: `gh auth status --hostname github.com`'s exit code, `acli jira auth status`'s exit code, and
for `bkt` a context for the host (never `bkt auth status`, below). A configuration folder is never
read and nothing a program prints of the account is kept.

When the program is missing, the advice is its **official installation page** and, where one command
fits most systems, that command; when it is signed out, the commands that sign in. Flottille installs
nothing and signs nobody in: the window offers a Copy button for each command, and the page opens
with the system's default handler.

| Program | Installation page | Command | Signs in with |
|---|---|---|---|
| `gh` | <https://cli.github.com/> | none (each system has its own) | `gh auth login` |
| `bkt`, Bitbucket Cloud | <https://github.com/avivsinai/bitbucket-cli> | `brew install bitbucket-cli` (macOS and Linux with Homebrew) | `bkt auth login https://bitbucket.org --kind cloud --web`, then `bkt context create bitbucket-org-<workspace> --host api.bitbucket.org --workspace <workspace>` |
| `bkt`, Data Center | the same | the same | `bkt auth login https://<host> --web-token`, then `bkt context create <host>-<project> --host <host> --project <PROJECT>` |
| `acli` | <https://developer.atlassian.com/cloud/acli/guides/install-acli/> | none | `acli jira auth login`; signed in to another site: `acli jira auth switch --site <site>` |

The `bkt` commands are the ones its README gives (Cloud by the browser, Data Center by a personal
access token), with the workspace or the project of the repository that declares the forge, and the
context named after the host and that workspace or project, its letters and digits and the rest a
hyphen (`git-example-com-proj`), so that the commands can be run as they are. A Cloud context names
`api.bitbucket.org`, the host `bkt`'s API needs: Flottille's own check accepts that name and
`bitbucket.org`, and the advice never gives a command the API would refuse. `acli` keeps one active
site: the check reads the `Site:` line of `acli jira auth status`, as the tickets do
(`docs/tickets.md`), and says when it is signed in to another site than the tracker's, with the
switch command. The programs are checked together, each with its own time (the version 5 s, a
sign-in check 30 s): one that hangs costs its own time, not the sum of them. The state is read when the project's Settings open, again after a forge
or the tracker is saved, and whenever the user asks (Check again); never in a loop. See
`project.programs` (`docs/protocol.md`), the window below, and `flottille doctor`
(`docs/cli.md`), which names the programs of the registered projects only, a missing one a warning
and never a failure.

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
- its comments (their number; `forge` `show` gives their text, and the reviews');
- for a merged or closed one, **the comment it was closed with**, when the forge gives one
  ("The closing comment" below).

### The closing comment

`forge` `show` of a merged, declined or closed pull request says `closing comment (<author>): …`
under its other lines, in the short detail as in the full one, and nothing when the forge gives
none (no empty line). It is the comment the pull request was **closed with** (`gh pr close
--comment`, `bkt pr decline --comment`), not any comment its closer wrote earlier: an "LGTM" from
the day before the merge is not one. No forge calls a pull request's last words a field of its
own, so each gives what it really exposes:

- **GitHub**: `gh pr view --json` has no closing comment, and no closer for a closed pull request
  (`mergedBy` and `mergedAt` are the merger's of a merged one alone, and are read with the rest).
  A merged pull request with them costs no call; otherwise the closer is read from the issue
  events (`gh api repos/{owner}/{name}/issues/{n}/events --paginate --jq …`, documented REST: the
  last `merged` or `closed` event's `actor.login` and `created_at`, one line each for those
  events). The closing comment is the closer's last comment **in the minute before** that
  instant; none when they wrote none then. An app is the same whether `gh` names it `app/stale` or
  the events `stale[bot]`. When the events cannot be read, nothing is said of a closing comment,
  the rest of `show` is as usual, and the daemon's log says it once.
- **Bitbucket Cloud**: the pull request's own page (`bkt api`, already read) says `reason`, the
  text it was declined with, and `closed_by`. The reason is the closing comment, said without an
  author when the page names no `closed_by`. Cloud keeps no comment as the one a pull request was
  closed with and gives no time of the closing, so no comment stands in for the reason; a merge
  message lives in the merge commit, not on the pull request: a merged pull request has none.
- **Bitbucket Data Center**: its activities (already read) hold a `MERGED` or `DECLINED` one with
  the `user` who did it and its `createdDate`; the closing comment is that user's newest comment
  in the minute before it (each `COMMENTED` activity has its `createdDate`). Without those times,
  none. Data Center's decline and merge take no message.

**These shapes (`reason`, `closed_by`, the activities' `user` and `createdDate`, the issue
events) are assumed from the forges' REST documentation, not from `gh`'s or `bkt`'s source, and
the maintainer checks them on a real server.**

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
- **And continuously, for the whole project**: every pull request of each repository whose forge
  is declared is read by the daemon on its own, incrementally (only what changed since the last
  round), within a budget (a round a minute while a window is connected, every 5 minutes
  otherwise, longer for a forge that fails), kept in the store and said as `item.*` events
  ([`items.md`](items.md)). The workspace's state above is still read on demand and while it is
  watched. A subscription to a pull request reads nothing of its own: it hears the rounds'
  events ("Subscriptions" below), and `forge list` answers from what they stored.

The state is kept in the daemon's memory, not in the store. When a read finds it different from the
last one (its time aside), the daemon emits `workspace.forge` with the whole state
(`docs/events.md`); a workspace with no forge to read says nothing.

## How fast `forge show` is read

A pull request is several reads of its forge, and on Bitbucket each is a round trip through `bkt`
(10 to 14 s were measured for one Cloud pull request, read one call after the other). Two things
make `forge show` fast, neither of which changes what is read or how a failure is said:

- **What does not depend on another read runs together.** Once the pull request is found, its
  checks, its conflicts, its page and its comments (Data Center: its activities) are read at the
  same time, then the logs of its failed checks, how long its CI takes and its closing comment
  are. The logs of the failed checks are read together as well, and the bounds of "Why a check
  failed" are applied in the checks' order, as they are when the logs come one by one. A read
  that fails fails alone: the best-effort ones (conflicts, CI, closing comment, a log) are still
  left out, and the failure said is still the first of the checks, the page and the comments.
  What is asked of the user's `gh` or `bkt` first (that it is there, and signed in) still is:
  nothing else is asked of a program that is not.
- **What was read is given again for 30 s.** The daemon keeps the last pull request `forge show`
  read, by its forge and its number, and the next `forge show` of it (with or without `number`:
  the one without keeps what it read too) is answered from it without a call, saying so (`(as read
  12 s ago; "fresh": true reads it again)`). `fresh: true` and `detail: full` read it again and keep
  what they read. A read that fails is never kept. Any write through Flottille (`forge create`,
  `comment`, `edit`, `merge`, `checks`, `close`, `reopen`, a pipeline's `run`, `stop` or `step`)
  drops everything kept, and a read that began before the write does not put back what it read.
  Time is the daemon's clock (`FLOTTILLE_TEST_CLOCK`, as everywhere).

The window's session link does not read a pull request by its number: it asks `workspace.forge`,
which reads the workspace's branch, answers from its own state of up to a minute
("When it is read" above), and is made of no logs nor CI times. It does not share this cache; the
tickets it shows are shared ([`tickets.md`](tickets.md), "How fast it is read").

The developer log ([`developer.md`](developer.md)) records, for each `forge` and `tickets` call,
`reads`: how many command lines ran (`ran`), the sum of their durations (`total_ms`), the slowest
(`slowest_ms`) and whether the cache answered (`cached`). A call whose `duration_ms` is near
`slowest_ms` and far under `total_ms` ran its reads together.

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

The project's Settings put, under each repository that declares a forge, the state of the program
it needs (`✓ gh 2.60.0 · signed in to github.com`, or `✗ bkt not found` with the installation
page and its command, or `✗ gh 2.60.0 · not signed in to github.com` with the sign-in commands), and
the Tracker row the same for `acli` or `gh`; "Check programs" (Repositories) and "Check again"
(Tracker) read them anew. Each line unfolds and fades in as it comes and folds away as it goes. When the state itself
changes (not found, signed out, signed in to another site, ready), the old line folds away and
fades out while the new one unfolds and fades in, and the ink moves from red to green with them. A
read that fails is a toast and leaves what was known; the buttons stay enabled to ask again.

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
| `bkt pr merge <id> --context C … --strategy S --close-source=false [--message M]` | the exit code | `pkg/cmd/pr/pr.go:1314-1334` (`--message`, the merge commit's message, `:1332`, given to both servers, `:1372-1376` and Cloud's `MergePullRequest`); `pkg/bbcloud/pullrequests.go:351-388` (Cloud takes `merge_commit`, `squash` and `fast_forward` only, `:352-356`, `:363-365`); `pkg/bbdc/pullrequests.go:108-132` (`mergeStrategyId`, given as it is) |
| `bkt api /repositories/W/R/pipelines/ --context C --param pagelen=50 --param sort=-created_on` (GET; the sort is a `--param`, not a `?` in the path: `bkt api` is not known to merge one into the other) | `values[]`: `build_number`, `state.result.name`, `target.type`, then `target.ref_name` (a branch's pipeline) or `target.source` and `target.destination` (a pull request's: `pipeline_pullrequest_target`), `target.selector.type`, `target.selector.pattern` | Atlassian's REST documentation (Pipelines, list pipelines). Not `bkt pipeline list`: it decodes `target.ref.name` (`pkg/bbcloud/client.go:140-152`) while the API sends `ref_name`, so every branch it prints is empty (seen live with v0.37.0), and a pull request's pipeline has no `ref` at all |
| `bkt pipeline list --context C --workspace W --repo R --limit 50 --json`, for the CI durations only ("How long CI takes") | `pipelines[]`: `state.result.name`, `created_on`, `completed_on` (`pkg/bbcloud/client.go:139-152`; the list has no duration field; its `target.ref.name` is empty, which the durations do not need) | `pkg/cmd/pipeline/pipeline.go:177-210`; `pkg/bbcloud/client.go:100-120`, `:145` (newest first: `sort=-created_on`) |
| `bkt pipeline run --context C --workspace W --repo R --ref <branch> [--selector-type pull-requests --selector-pattern <p>]` | the exit code | `pkg/cmd/pipeline/pipeline.go:81-133` (v0.37.0; the selector flags at `:127-128`), `pkg/bbcloud/client.go:431-482` (`ref_type: branch`, `ref_name`, `selector`) |
| `bkt pipeline view <build> --context C --workspace W --repo R --json` | `steps[]`: `uuid`, `name`, `state.name`, `state.result.name`; for a followed run `pipeline.state` and its times ("Following a run") | `pkg/cmd/pipeline/pipeline.go:416-480` (v0.37.0; `{pipeline, steps}`), `pkg/bbcloud/client.go:559-604` (`PipelineStep`) |
| `bkt pipeline logs <build> --context C --workspace W --repo R --step <uuid>` | the step's log, as text | `pkg/cmd/pipeline/pipeline.go:679-726` (v0.37.0), `pkg/bbcloud/client.go:692-725` (Cloud only) |
| `bkt pipeline run … --ref <branch> [--selector-type custom --selector-pattern <name>]`, for `forge` `run` with no variable ("The CI family") | the exit code, and a run's number `#N` when it prints one | `bkt pipeline run --help` of v0.37.0 (`--selector-type`, `--selector-pattern`, `--var`) |
| `bkt api /repositories/W/R/pipelines/ --context C --method POST --input <json>`, for `forge` `run` with variables | the new pipeline's `build_number` | Atlassian's REST documentation (Pipelines, run a pipeline: `target` a `pipeline_ref_target` with `ref_type`, `ref_name`, `selector.type` and `.pattern`; `variables[]` with `key`, `value`, `secured`); `bkt api --help` of v0.37.0 (`--method`, `--input`) |
| `bkt api /repositories/W/R/pipelines/{uuid}/stopPipeline --context C --method POST`, for `forge` `stop` | the exit code | Atlassian's REST documentation (Pipelines, stop a pipeline); the uuid is `pipeline.uuid` of `bkt pipeline view --json`, **assumed** |

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
- **Which statuses count (Cloud).** A Pipelines build leaves its statuses on the commit, and
  a rerun leaves new ones beside the earlier build's, so the statuses are told apart by the pipeline
  each links to (`…/pipelines/results/<build>`, found among the newest 50 pipelines of the API):
  only the **latest build of each pipeline** counts. **Every pipeline that ran on the head
  counts**, a branch's own push included, as Bitbucket's own merge check counts every status of the
  source commit: a failed branch pipeline fails the pull request's checks. A pipeline is told
  apart by its target (`pipeline_pullrequest_target`, or a branch, a tag, a custom one with its
  `ref_name`), its `selector.type` and `selector.pattern`, and, for a pull request's, its
  `destination`: one branch can be the source of two pull requests, and a passing build for one
  does not hide a failed build for the other. A status no pipeline of the list explains (another
  CI's, an older build) is kept. When the pipelines cannot be read, the statuses are shown as `bkt`
  gives them. **Known limits**: only the newest 50 pipelines are read (no second page), so a status
  of an older build is kept as it is; and a "re-run failed steps" in Bitbucket's page keeps the
  build number, so the statuses of the build are all that are read. Both fail safe.
- **Flottille's own rerun is a branch run.** `bkt` has no way to start a pull request's pipeline:
  `forge` `checks` runs the branch with `--selector-type pull-requests`, which Bitbucket records as
  a branch run (`pipeline_ref_target`) of the pull requests' definition. It has no `BITBUCKET_PR_*`
  variable and no merge with the destination, so it does not test what a real pull request
  pipeline tests. It is kept as the replacement of the pull request's failed run (it replaces the
  run of the one pull request pipeline of its pattern); with two pull requests from the branch it
  is not known which it was for, so it replaces neither and the failed run still counts.
- **Checks that have not started.** A Cloud pull request whose head has no status, changed
  less than 10 minutes ago (its `updated_on`, which a push moves) and in a repository whose newest
  pipelines are not empty, says `checks: not started yet for <short sha>` (`forge` `show`, and
  `not_started` in the JSON read, `checks not started` in the workspace summary) instead of `none
  ran on this pull request`; later, or in a repository with no pipeline, it is `none ran`. A merge
  is refused either way, there being no check, and says `its checks have not started yet for <short
  sha>` in the first case.
- **Draft is not read.** A pull request is `open`, `merged` (`MERGED`) or `closed`
  (`DECLINED`, `SUPERSEDED`); `bkt pr list` has no draft field, so none is shown. `forge` `edit`
  does set it (`bkt pr publish [--undo]`, below), and `create` still refuses `draft: true` for
  Bitbucket (`bkt pr create --draft` exists, not used yet).

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
  (`pkg/cmd/pr/pr.go:1334`) and `gh pr merge` does not. A squash `message` is given whole with
  `--message M` (subject and body in one text, as Bitbucket takes it); left out, the server writes
  its own. Not yet seen on a real server: that Cloud and Data Center keep it as the squashed
  commit's message. When the server refuses the merge, the
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
- `edit` goes further than a title and a description (v0.37.0 help, #360): `--reviewer R` and
  `--remove-reviewer R` of `bkt pr edit`, `bkt pr publish <id>` (alias `ready`) for `draft:
  false` and `bkt pr publish <id> --undo` for `draft: true`, and, for `base`, Cloud's API: `bkt api
  /repositories/<w>/<r>/pullrequests/<id> --method PUT --input
  '{"destination":{"branch":{"name":"<base>"}}}'` once `/repositories/<w>/<r>/refs/branches/<base>`
  answered (a 404 is `refused: <repo> has no branch <base> on the forge; nothing was written`).
  **Assumed, and the maintainer checks them**: that body for the destination (it holds only
  `destination`: check that the pull request's title and **reviewers are still there after the
  move**, Atlassian's "update a pull request" being a `PUT` of the whole pull request, and a `PUT`
  without `title` or `reviewers` having been refused or having dropped them), and that `bkt pr
  publish` and `bkt pr edit --reviewer` behave on his Cloud and Data Center as their help says.
  When a later step fails, the answer names the earlier one that had already changed the pull
  request.
  Data Center is refused for `base` (the `toRef` and the version it needs are not something `bkt`
  helps with), and Bitbucket has no labels on pull requests: `labels` on `create` and
  `labels_add`/`labels_remove` on `edit` are refused naming that, before anything is asked of `bkt`.
- `create` adds the call's `reviewers` after the project's, and `delete_branch_on_merge` is `bkt pr
  create --close-source` (refused before anything is opened for a branch the workspace resumed or
  another workspace is on: the forge's own close of the source branch would delete it). `create` still refuses `draft` on Bitbucket (`bkt pr create --draft`
  exists, and is not used yet). At `merge`, `--close-source=true` replaces `=false` when the branch
  is to be deleted (`docs/tools.md`, "The forge"), since `bkt` sends the flag with every merge and
  its default would override the pull request's own.
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

**Only an open pull request is edited.** `edit` of a merged, declined or closed one is refused
before the forge is asked to write: the state is read first (GitHub: `gh pr view`; Bitbucket: the
pull request's own page through `bkt api`, not the list's newest 50, so an open pull request
older than them is still edited, and so is one whose state cannot be read, the forge then
saying) and the refusal says the pull request is `merged`, `closed` (GitHub), `declined` or
`superseded` (Bitbucket), that nothing was written, and what to do (comment on it, or open a new
pull request). Bitbucket would answer `Can only update an open pull request`. The refusal comes
once the user allowed the call, like `close`'s.

`forge {action, …}` (`docs/tools.md`, "The forge"): `list` the pull requests of the place's
repositories by state and branch, `show` a workspace's branch's or one by its number with its
checks, reviews and comments; `create`, `merge`, `checks`, `comment` and `edit`, each to judge (a
merge that waives the checks base). The same `gh` (or `bkt`, for a Bitbucket repository) and the
same rules: spawned by the daemon, the repository named, its output masked, no token to the agent.
It never reaches a repository outside the agent's place, and is listed to every agent of a
project whose repository declares a forge, in `main` as in a workspace. Until #141 it was
`forge_read` and `forge_write`.

## The CI family

An agent asked to "run the CI for the tests" runs it itself: `forge` lists what can be run, reads
the recent runs, a run's steps and a step's log, runs **any pipeline of any branch** of the place's
repositories with **any variables**, stops a run and acts on one of its steps. There is no list of
allowed pipelines in Flottille: a deploy pipeline is run like the others, and the gate decides as
for every forge write (Full runs it, Ask holds it for the user, Auto asks the judge). `checks`
(re-run the failed) stays as it is. The repository is one of the place's, its declared forge
checked against its remote, as for `comment` and `edit`. The branch is the one named (`branch`,
which git must take for a branch's name), else the one the workspace's repository is on now, else,
in `main`, the one the project's checkout is on.

| Action | GitHub (`gh`) | Bitbucket Cloud (`bkt`) |
|---|---|---|
| `pipelines {repository?, branch?}` (read) | the workflows of `.github/workflows/` that accept `workflow_dispatch`: name, file, inputs (required, type, options, default, description) | the pipelines of `bitbucket-pipelines.yml`: `default`, each of `branches`, `tags` and `pull-requests` by its pattern, each `custom` by its name with the variables it declares (default, allowed values, description) |
| `runs {repository?, branch?}` (read) | `gh run list -R … [--branch <b>] --limit 20 --json databaseId,workflowName,status,conclusion,event,headBranch,createdAt,startedAt,updatedAt,url` | `bkt api /repositories/W/R/pipelines/ --param pagelen=50 --param sort=-created_on`, the branch's (`target.ref_name`, or `target.source` of a pull request's): `build_number`, the selector, `state`, `trigger.name`, `created_on`, `duration_in_seconds` |
| `run_view {repository?, run}` (read) | `gh run view <run> -R … --json databaseId,workflowName,status,conclusion,url,jobs` | `bkt pipeline view <run> --json`: `steps[]` (`name`, `uuid`, `state.name`, `state.result.name`) |
| `run_logs {repository?, run, step}` (read) | `gh run view --job <id> -R … --log`, the job found by its name or id among the run's | `bkt pipeline logs <run> --step <uuid>`, the step found by its name or uuid |
| `run {repository?, pipeline, branch?, variables?, follow?}` (write) | `gh workflow run <file> -R … --ref <b> -f k=v…`; the answer is the run's link when `gh` prints it | without variables `bkt pipeline run --ref <b> [--selector-type <t> --selector-pattern <p>]`, the run's number read by its uuid (`bkt api …/pipelines/{uuid}`) when it prints no `#N`; with variables the REST call through `bkt api` (table above); the answer is the run's number and link |
| `stop {repository?, run}` (write) | `gh run cancel <run> -R …` | `bkt api …/pipelines/{uuid}/stopPipeline --method POST`, the uuid read with `bkt pipeline view` |
| `step {repository?, run, step, do: start \| rerun}` (write) | `rerun`: `gh run rerun <run> -R … --job <id>`; `start` is refused | refused, naming why |

- **What can be run is read from the branch's own files**, through git, not from the forge: the
  file as `<remote>/<branch>` holds it (as last fetched, what the forge runs), else as the local
  branch holds it, which the answer says (`… is not on origin as last fetched, and the forge runs
  what is pushed`). YAML is read with `serde_norway`, merge keys (`<<: *anchor`) applied, so a
  custom pipeline whose variables come from an anchor lists them. A file that cannot be read
  says which of three it is: not in that commit, git could not give it (git's words), or it is
  there and the parser refuses it (its message, line and column); a merge key that cannot be
  applied is said and the rest is listed; a GitHub workflow that cannot be parsed is named under
  `Not read:` and never hides the others. GitHub runs a workflow by `workflow_dispatch`
  only when the file is also on the default branch: that is GitHub's own refusal, passed on.
- **A pipeline is named as `pipelines` lists it**: Bitbucket `default`, `custom: <name>` (a bare
  name is a custom one), `branches: <pattern>`, `tags: <pattern>`, `pull-requests: <pattern>`;
  GitHub the workflow's file (`e2e.yml`), or anything `gh workflow run` takes.
- **Variables** are `[{key, value, secured?}]`: a Bitbucket pipeline's variables, a GitHub
  workflow's inputs. A key is letters, digits, `_` and `-`. On Bitbucket a run with variables goes
  through the REST call, not `bkt pipeline run --var`: `--var` is a list flag that splits a value
  at its commas (`PREVIEW_JOBS=api,web` would be two), and has no secured variable.
- **A secured variable** is sent as Bitbucket's `secured: true`, and its value is in no record of
  the call (`[masked]` in the arguments stored, shown to the user and given to the judge), nor in
  any answer, whatever the forge echoed. GitHub has no secured input: the value is still masked
  the same way in Flottille, and sent as an input.
- **Steps.** Bitbucket Cloud's REST API has no call that starts a manual step nor runs one step
  again: its steps are read only (`GET …/steps/`, `…/log`), and only a whole pipeline is run or
  stopped (checked in Atlassian's OpenAPI description, `swagger.v3.json`, and Atlassian's own
  answers, feature request BCLOUD-21591). `step` there is refused, naming why and the run's page.
  GitHub has no manual step: a job that waits is held by its environment's reviewers, a protection
  the user set for people, which an agent never answers for them; `start` is refused saying so,
  and `rerun` runs one job again (with what it needs, as `gh run rerun --job` does).
- **Bitbucket Data Center** has no pipelines `bkt` can read or run: every action of the family is
  refused naming why, before anything is asked of the forge.
- **Read like the rest**: every output is masked, a log is cleaned of what a terminal adds and cut
  like every long tool result (`docs/tools.md`, "Long results").
- **Assumed, and the maintainer checks it on his Bitbucket**: the run body's shape (`target`,
  `selector`, `variables`) and the answer's `build_number`, from Atlassian's REST documentation;
  that `bkt pipeline view --json` gives the run's `pipeline.uuid`; whether `bkt pipeline run`
  prints the run's number (when it does not, the answer says `forge runs lists it`).

The window says a run an agent started like its other forge writes: `Ran a pipeline` with the
pipeline's name, in the session's list of what was written, and the tool's step with the run's
link in its answer.

### Following a run

An agent that starts a run waits for it: `run` **follows the run it started** unless it is given
`follow: false`. The following is a subscription of the source `run` (`docs/design.md` §5.6), with
no time of its own, and its answer ends with `Followed until it ends: a delivery wakes you then,
and once if it waits on a manual step, so no wake is needed; forge unsubscribe {"run": 43} stops
following it.` A subscription to the pull request of the run's branch does not follow the run:
its `checks` is the end of the pull request's checks, which its answer names as they run, and it
names the runs the agent already follows there (`docs/tools.md`, "The forge").

- **Read by the rounds of the items**, with the pull requests of the run's repository ("Subscriptions"
  below): one request a followed run in each round, `bkt pipeline view <run> --json` (its
  `pipeline` and `steps`) or `gh run view <run> -R … --json status,conclusion,startedAt,updatedAt,jobs`,
  as often as the rounds come, and nothing at all when no run is followed there.
- **Its end wakes the agent**, a `subscription` delivery that names the run (its number, its
  repository, its pipeline, its branch and its link), says its result as the forge does
  (Bitbucket `successful`, `failed`, `error`, `stopped`; GitHub `success`, `failure`, `cancelled`…)
  and its duration (Bitbucket `created_on` to `completed_on`; GitHub `startedAt` to `updatedAt`),
  and for a failure the failed steps (jobs) by name, no log: `run_logs` reads it. The following is
  then over.
- **A wait wakes it once**: a Bitbucket run paused on a manual step (the step named, someone
  starting it on the run's page), a GitHub run held by its environment's reviewers. The following
  goes on until the run ends; the same wait is not said twice.
- **A run that cannot be read** (the forge down, `bkt` without a context, the run gone) is said
  once, `It cannot be read now: <why>`, and the following goes on: nothing more is said of it
  until it is read again.
- **It ends** with the run, when the agent ends, or with `unsubscribe {run, repository?}` (or the
  user's Cancel); `forge show` lists it under `Your subscriptions:` (`- run #43 of api (custom:
  nightly on app/login): until it ends`), the window's rail as `Run #43 of api`, since when.
- **A run whose number the forge did not say** (GitHub printed no link, `bkt` neither a number nor a
  uuid) is not followed, and the answer says so.
- **Assumed**, and the maintainer checks it on his Bitbucket: that `bkt pipeline run` prints the
  new run's uuid (`✓ Triggered pipeline {…} on …`) when it prints no number; that `bkt pipeline
  view --json` gives the run's `state` (`name`, `result.name`) and `created_on` and `completed_on`;
  that a run waiting on a manual step says `state.stage.name` `PAUSED`, or else shows as no step
  running, one done and the next `PENDING` (a run read between two steps then reads as waiting).

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
  Its markers are GitHub's workflow commands (`##[group]`, `##[endgroup]`, `##[error]`, printed
  by the runner itself): the lines kept are those after the `##[endgroup]` that closes the last
  `##[group]Run …` (the command and its `env:`), up to the last `##[error]` line, which is kept.
  A log with no `##[error]` line keeps its end. This is a change for GitHub too: the command
  echoed with its `shell:` and `env:` block, and what follows the last error, are no longer shown.
- **Bitbucket Cloud**: a failed check whose link is a Pipelines build of the repository
  (`https://bitbucket.org/<workspace>/<repo>/pipelines/results/<build>`, the `url` of the commit
  status) names the build number, which is all `bkt` needs. The first failed step is found with
  `bkt pipeline view <build> --json` (`steps[].state.result.name` is `FAILED` or `ERROR`) and its
  log is read with `bkt pipeline logs <build> --step <uuid>`. A status of another CI, or of
  another repository, says so (`no log: it is not a Bitbucket Pipelines build, see …`). Its lines
  have no names nor timestamp to drop.
- **The failing command's output** (Bitbucket Cloud): the log of a step holds its setup, each
  script command, then Bitbucket's teardown, and an agent needs the part of the command that
  failed. The kept lines run from the **last command echoed** (`+ <command>`) to the **teardown**
  (or to the end of the log, or to a command that lists the environment), the command line
  included. In between, a run of 3 or more `NAME=value` lines (capitals, digits and `_` in the
  name) is an environment listing and is left out; a command that is only `env`, `printenv`,
  `export` or `set` is not taken for the failing one. When no command is echoed, or nothing
  is left, the end of the log is given as before, and the line says `lines of its log` instead
  of `lines of the failing command's output`.
  - **Documented** by Atlassian: the test reports are searched for in the "Build Teardown"
    phase, with the lines `Searching for test report files in directories named …`, `Found
    matching test report file …`, `Located test suite containing …`, `Finished scanning for test
    reports.`, `Merged test suites, total number tests is …` and `Uploading test results`
    (support article "Successful test reports not showing in the Pipelines UI"). The first one
    is the teardown marker.
  - **Assumed**, from logs seen and not from documentation (Atlassian describes no log format):
    that each script command is echoed as `+ <command>`; that the failing command is the last
    one echoed (an `after-script` command that is not an environment listing would take its
    place; output that itself starts with `+ ` too); that a teardown line starting
    `Searching for files matching artifact pattern`, `Uploading cache` or `Uploading artifact`
    comes after the commands; that an environment listing is `NAME=value` lines. Each marker is
    a constant of `crates/flottille/src/forge/logs.rs`: a wrong one makes the log fall back to
    its end, or to a wrong part, never to an error.
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

## How long CI takes

An agent that subscribes to its pull request picks a duration blind. So `forge` `show` of a pull
request (the workspace's branch's or one by its number) and the answer of `forge` `subscribe` carry
one line about the repository's recent CI runs, when there are finished ones to measure:

```
Recent CI runs of acme/app took 4 min on average (3 to 6 min, last 5).
```

- **GitHub**: `gh run list -R github.com/owner/name --status completed --limit 20 --json
  conclusion,startedAt,updatedAt`. A run counts when its conclusion is `success` or `failure`
  (cancelled, skipped, timed out and the like measure nothing) and its time is `updatedAt` minus
  `startedAt`, the nearest `gh` gives to a run's end.
- **Bitbucket Cloud**: `bkt pipeline list --limit 50 --json` (the page maximum, "A page is 50
  at most"). `bkt` v0.37.0 prints no duration, only `created_on` and `completed_on`
  (`pkg/cmd/pipeline/pipeline.go:357-386`, `pkg/bbcloud/client.go:139-152`); a pipeline counts when
  its `state.result.name` is `SUCCESSFUL`, `FAILED` or `ERROR` (`STOPPED` and one still running
  do not) and its time runs from creation to completion, so a wait in the queue is part of it,
  which is what an agent waits for.
- **Bitbucket Data Center** has no CI API in common: nothing is said and nothing is asked.
- The **last 5** finished runs of the repository, any branch, newest first; the average, the
  shortest and the longest are rounded to the minute (1 at least). A run of no time is left out.
- **One read**, made only when `forge` `show` or `forge` `subscribe` runs, never by the rounds
  of the items (which a subscription hears) nor the watch of a client. **A read that fails** (`gh` or `bkt` failing, an answer in a
  shape not expected) is not an error: `show` and `subscribe` answer without the line.

**`forge` `subscribe` without `minutes`** is sized by those runs: twice their average, rounded up
to the minute, from 15 minutes (`SHORTEST_MINUTES` in `forge/ci.rs`) to the longest allowed
(`subscriptions::LONGEST_MINUTES`); with no run to measure by it stays
`subscriptions::DEFAULT_MINUTES`, 60. The answer says which duration was taken and why:

```
Duration: 25 min, twice the average of the recent CI runs.
Duration: 15 min, the shortest: twice the average of the recent CI runs is 4 min.
Duration: 60 min, the default: no recent CI run to measure it by.
```

With `minutes`, the agent's choice is kept and the answer carries the line about the runs alone.

## Subscriptions

The pull requests the daemon keeps up to date are a source of events of the subscriptions
(`docs/design.md` §5.6). Each round of the items compares what it read of a pull request with
what the store holds of it and says what changed as `item.*` events ([`items.md`](items.md),
"Events"); a subscription to that pull request is offered them once they are stored, as the
events it names:

- `checks` (`item.checks`): none runs now, and one was running or the head commit changed; the
  delivery is a summary, not the logs: how many passed, failed and were skipped, then for each
  failed job (three at most, the others counted: `and 5 more failed, not listed`) its name, how
  long it took and the line that says why it failed, and where the details are
  ("A wake about failed checks is a summary" below);
- `review` (`item.reviewed`): a review that was not there (GitHub: its state, not `pending`;
  Bitbucket: a reviewer's new verdict), with its author, state and an excerpt (200 characters, on
  one line);
- `comment` (`item.commented`): a comment that was not there, with its author and an excerpt, the
  newest five of a round at most;
- `merged`, `closed` (`item.merged`, `item.closed`): the pull request's state became that; either
  ends every subscription to it.

`forge subscribe` reads the pull request once and stores it before the subscription opens: what
is already so is no event. A round that fails changes nothing. A subscription hears as often as
the rounds come: every minute while a window is connected, every five otherwise. **A bot**, for `from: people`, is a GitHub author whose
login starts with `app/` (as `gh` says an app) or ends with `[bot]`, or that `gh` says `is_bot`;
this is assumed from `gh`'s output, and the maintainer checks it on a real pull request. A
Bitbucket author is always taken for a person: what says a bot there is not read. **What
Flottille posted** is recorded (below), and `from: people` leaves it out, whichever agent posted it:
the user's own comments, written under the same account, wake the subscription.

### A wake about failed checks is a summary

A wake weighs on the agent's context at every turn: with three failed jobs, each log inlined made
about 130 lines. The `checks` delivery says instead:

```
The checks ended: 1 passed, 3 failed.
- job-1 (2 min 5 s): error[E0425]: cannot find value `x1` in this scope
- job-2 (4 min 10 s): test login::form ... FAILED
- job-3 (6 min 15 s): no log: it is not a GitHub Actions job, see https://ci.example.com/builds/5
and 5 more failed, not listed.
The end of each failed job's log is in `forge` `show` of the pull request.
```

- **The failed jobs listed** are the first three (`events::LISTED`); the others are counted, not
  named, and their logs are not read. Whatever the number of jobs, a delivery is about a dozen
  lines.
- **A job's duration** is its check's start to its end (`1 h 2 min`, `2 min 5 s`, `45 s`), the
  start being the check's creation time when the forge gave it no start; none when it gave no
  time.
- **The line that says why** comes from the end of the job's log, as "Why a check failed" finds it
  (the failing command's output): the first line that has the word `error`, `failed` or
  `panicked` (whole words, in any case), else the last line. Passing lines are no candidate
  (`... ok`, `0 failed`, `Compiling thiserror`), nor is the runner's `##[error]Process completed
  with exit code N`, which says nothing of why. The line is cut at 160 characters, and masked as
  every line of a log is. A job whose log cannot be read says `no log:` and why, whole.
- **The details are `forge` `show`**, which gives the end of every failed job's log as before.

## What Flottille posts is recorded

What an agent posts goes out through the user's own `gh`, `bkt` and `acli`, so it appears under
the user's name, in front of their colleagues. **Nothing of Flottille is added to it**: no marker,
no footer, on GitHub, Bitbucket (Cloud and Data Center) and Jira. Every body and comment Flottille
posts for an agent (`forge` `create` body, `comment`, `edit` body, `close` and `reopen` comment;
`tickets` `create`, `comment` and `edit` body) goes out as the agent wrote it. A `forge edit` or
`tickets edit` that changes only the title posts no text and records none.

Flottille knows its own posts by what it records when the forge or the tracker took them. The
store keeps, for each post that succeeded: the project, the forge or tracker (`github`,
`bitbucket`, `jira`), the repository (the repository's path in the project for a pull request, the
tracker's repository for a GitHub issue, none for Jira), the pull request's number or the ticket's
key, whether it is a `comment` or the item's own `body` (its description), the comment's id when
the forge answered one, a hash of the text and the time. A post that failed is not recorded. The
record survives a restart of the daemon, and reading a comment back asks the forge nothing more
than it did.

What the three tools answer to a write decides what identifies the comment:

| Write | Answers | Recorded |
|---|---|---|
| `gh pr comment`, `gh issue comment` | the comment's link, `…#issuecomment-<id>` | the id |
| `gh pr create`, `gh issue create` | the item's link | the item: a `body` |
| `gh pr edit`, `gh issue edit` (with a body) | the item's link | the item: a `body` |
| `gh pr close`, `gh pr reopen` (with a comment) | nothing that names the comment | the text's hash |
| `bkt pr comment`, `bkt pr decline` (with a comment) | `✓ Commented on pull request #N`, nothing that names the comment | the text's hash |
| `bkt pr create`, `bkt pr edit` (with a body) | the pull request's number | the item: a `body` |
| `acli jira workitem comment create` | nothing read (`--json` is not asked) | the id of the newest comment of its words that no record holds, read from the work item right after the post; the text's hash alone when that read fails |
| `acli jira workitem create`, `edit` (with a body) | the work item's key | the item: a `body` |

A comment read back is known as an agent's by its record:

- with the id the forge gave it, when the record has one (GitHub, Jira): the same words under
  another id are a person's;
- else by the hash of its text, and a hash record is **one comment's**: the first comment of those
  words that was not written before the post (the comments are taken oldest first, by the time the
  forge gives: GitHub's `createdAt`, Bitbucket Cloud's `created_on`, Data Center's `createdDate`).
  A person's same words, before it or after it, stay a person's. A comment older than the record
  by more than five minutes (the clocks differ) is not it. Where a read gives no time the first of the
  words is taken: Jira's `created` (`2026-10-01T12:00:00.000+0000`) is not read as a time, so for
  Jira the post claims the first comment of its words.

Only a record that holds a comment's id lets an agent edit or delete that comment (`tickets
comment_edit`, `comment_delete`, `docs/tickets.md`): words alone never do, a person may write the
same. An edit replaces the record of the id with the new words; a delete drops it.

The hash is of the text as a read gives it: masked (the secrets of a text are masked on every
read, the record hashes the masked text too), whitespace one space, only the first 500 characters
counting, and the markdown a tracker rewrites gone: code fences, `*` and `` ` `` marks, heading
marks, list bullets and numbers. What is recorded for a Jira post is the text after the round trip
(Markdown to Atlassian's format and back, `tickets.md`, "Jira's text"), the text a read gives, so
the same words found again are still found, whatever the markup. What the hash does not see: any
rewriting of the words themselves by Jira (a mention, a smart link); such a comment is shown as a
person's.

Bitbucket's closing comment is the same record: Cloud's reason a pull request was declined with
and Data Center's comment of the closer are matched as any comment, the reason by its hash, the
comment by the id it shares with the one among the comments.

The limit of the hash: the user's own comment, written under the same account, that says exactly
what an agent posted, and was written after it and before the agent's own is read, can be taken
for the agent's, the agent's then being the person's. A comment read back in the instant between
the forge taking it and the daemon recording it (milliseconds) is a person's for that read. A
person's comment that ends with the old marker below is taken for an agent's. Both are accepted.

An item's own text is known by the item and by its words: a ticket whose description an agent
created or edited is `description_by_agent` while it still reads as the agent left it; a person
editing it afterwards makes it the person's.

It is how a text read back is told from a person's:

- `from: people` leaves out a comment known as an agent's;
- a delivery, `forge show` and `tickets show` say `(posted by an agent)` after the author of such a
  comment.

**The old mark is still read.** Before the record, Flottille ended its posts with a line of its
own, `<!-- flottille -->` on GitHub and Bitbucket and the visible `— posted by a Flottille agent`
on Jira. A comment or a description that still ends with either is an agent's, and shown without
it, so that the history posted before keeps reading right. Nothing writes it any more.

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

The timing scenarios make the fakes slow: `read_delay_ms` makes every read of the fake `bkt` (and of
the fake `gh`) wait that long, `view_delay_ms` a view of the fake `acli`, and the fake `bkt` records
the millisecond each call started at, so that a test sees which reads ran together without
depending on how loaded the machine is; the time it checks has a wide margin.
