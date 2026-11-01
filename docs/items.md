# Items

Every pull request and every ticket of a project, kept up to date by the daemon on its own, so
that a client lists them at once, the events say what changed, and what waits on a forge or a
tracker filters one stream instead of each reading it again. Part of the milestone "Forges and
tickets" (`docs/design.md` §13), after [`forges.md`](forges.md) and [`tickets.md`](tickets.md),
whose command lines, rules and parsing it reuses: the user's own `gh`, `bkt` and `acli`, spawned
without a shell, no token ever read, everything masked before it is stored.

## What is read

A project is read only for what it **declares**: the pull requests of each repository whose forge
is declared (`forges.md`, "The forge of a repository"), and the tickets of its tracker
(`tickets.md`). A project that declares neither is never read; a repository whose remote leads
elsewhere than its declared forge is refused as `forge` refuses it, and said as that source's
problem.

**All** of them, whoever opened them and whatever their state: every pull request of a
repository, every issue of the tracker's repository, every work item of each Jira project of the
tracker.

## Stored

| Table | What |
|---|---|
| `items` | One row per pull request or ticket: `project`, `kind` (`pull`, `ticket`), `source` (`github`, `bitbucket`, `jira`), `repository` (the project's path of it; empty for Jira), `key` (its number, or its Jira key), `title`, `state`, `author`, `assignees`, `labels`, `url`, `updated_at` (as the forge says it), `seen_at` (when Flottille last read it, in ms), `mine`, and `details` (JSON) |
| `item_checks` | The checks of a pull request, in order: `name`, `state` (`passing`, `failing`, `pending`, `skipped`), `started_at`, `ended_at`, `link`. The CI moves without the pull request's `updated_at`, so it is kept beside it |
| `fetch_cursors` | Per project and source (`kind`, `target`: a repository's path, or `tracker`): `cursor` (when the last complete round started), `round_at` (when the last round started), `failures` (in a row) and `problem` (why the last one failed) |

- **`state`** is `open`, `draft`, `merged` or `closed` for a pull request, `open` or `closed` for
  an issue, and for a Jira work item `closed` once its status is in the done category
  (`fields.status.statusCategory.key` is `done`), `open` otherwise; its status itself is in
  `details.status`.
- **`details`** holds what is proper to a kind or a source: of a pull request `head`, `base`,
  `head_commit`, `mergeable`, `review`, the number of its `comments` and `reviews`, and on
  Bitbucket its `reviewers`; of an issue the number of its `comments`; of a work item its
  `status`, `type`, `priority` and status `category`.
- **`mine`**: opened by, or assigned to, the user the forge says it is signed in as: GitHub's
  `gh api user` (`login`), asked once per daemon; Bitbucket Cloud's `bkt api /user`
  (`display_name`, assumed from Atlassian's REST documentation). Bitbucket Data Center and Jira
  say no user in what Flottille reads of them (of `acli jira auth status`, only the `Site:` line
  is read, never an e-mail), so their items are never `mine` (follow-up).
- Every text is masked before it is stored, as every output is (`flottille_core::secrets`).
- A project's items and cursors go once its file is gone (`project.remove`, or by hand), and a
  source's once the project no longer declares it (a repository removed, its forge or the tracker
  unset), at the next sweep.

## Fetched incrementally

A **round** of a source asks only what changed since its cursor, less five minutes
(`items::OVERLAP_MS`), so that what changed while the last round ran, or a forge whose clock is a
little behind, is not missed; what it reads again unchanged emits nothing. A source's first round
(no cursor) reads everything. The cursor moves only when a round worked.

| Source | Each round |
|---|---|
| GitHub pull requests | `gh pr list -R github.com/<owner>/<name> --state all --limit 1000 [--search "updated:>=<cursor>"] --json <the fields of forge show>,author,assignees,labels,updatedAt`: each pull request with its checks, reviews and comments, read with the fields and the parser of `forge show` (`Pull::of`), in the same call |
| GitHub issues | `gh issue list -R … --state all --limit 1000 [--search "updated:>=<cursor>"] --json number,title,state,author,assignees,labels,url,updatedAt,comments`; only the tracker's repository's issues are kept, as `tickets list` keeps them |
| Bitbucket Cloud | `bkt api /repositories/<w>/<r>/pullrequests --param pagelen=50 --param sort=-updated_on --param state=OPEN --param state=MERGED --param state=DECLINED --param state=SUPERSEDED [--param q=updated_on>="<cursor>"]`, the filter and the order as `--param`, never in the path (#245); then each pull request read as `forge show` reads it (its commit statuses, reviewers, comments, conflicts) |
| Bitbucket Data Center | no filter in what `bkt` reads: `bkt pr list --state all --limit 50 --json`, the newest 50, compared with what is stored; each open one is read as `forge show` reads it, a merged or declined one from the list alone |
| Jira | for each project of the tracker, `acli jira workitem search --jql 'project = "<KEY>" [AND updated >= "-<n>m"] ORDER BY updated DESC' --fields key,summary,status,assignee,reporter,labels,issuetype,priority,updated --json --limit 1000`, once `acli` is checked to be on the tracker's site (`tickets.md`, "The Jira site") |

- **The checks of a pull request whose CI still runs** are read again at every round, even when
  the pull request did not change (`gh pr view <n> --json statusCheckRollup`; on Bitbucket `bkt pr
  checks <id> --json`).
- **The cursor is a time** in milliseconds on the daemon's clock. GitHub and Bitbucket take it as
  RFC 3339 in UTC (`2026-11-01T10:00:00Z`). Jira takes it as **minutes before now**
  (`updated >= "-12m"`, rounded up): a date in JQL is read in the time zone of the user's Jira
  profile, which Flottille does not know, while a relative time is the same everywhere.
- **Assumed, and the maintainer checks it on a real forge**: that `gh pr list --search` and
  `gh issue list --search` take `updated:>=<RFC 3339>` with `--state all` (GitHub's search
  documents the qualifier; `gh` adds no state qualifier for `all`), and give the same fields as
  `gh pr view`; that `bkt api` passes repeated `--param state=…` and a `q` holding quotes as
  query parameters, encoded, and that Bitbucket's filter takes `updated_on>="<RFC 3339>"`; that
  Cloud's pull request object says `author.display_name` and `updated_on`, and `/user` says
  `display_name`; that `acli … search --limit 1000` reads up to that many and its fields hold
  `reporter`, `issuetype`, `priority` and `updated` in Jira's REST shape; that `gh api user` prints
  the user's `login`. GitHub's search gives 1,000 results at most: a first round of a repository
  with more pull requests or issues keeps the newest 1,000 it gives.

## Rhythm

- **Every minute while a window is connected** to the daemon (a connection that called
  `events.watch` with `client: "window"`), **every 5 minutes otherwise**; both in the daemon's
  configuration, `$XDG_CONFIG_HOME/flottille/daemon.toml`, read at each sweep:

  ```toml
  [items]
  window_minutes = 1
  minutes = 5
  ```

- A source is due once its rhythm has passed since its last round started. The sweep (every 5 s)
  starts the rounds due, one project at a time, its sources one after the other.
- **A source that fails** (its command line missing or signed out, rate limited, the network
  down, an answer not in the shape expected) waits twice as long after each failure in a row, up
  to 16 minutes (or its rhythm, when longer), and says why: its `problem` in `items.list`'s
  `sources`, and the daemon's log once when it starts being so. The other sources of the project
  go on.
- **After a sleep** of the machine, the first sweep finds every source due and catches up from
  its cursor: nothing that changed meanwhile is lost.
- `items.refresh` makes a round of every source of the project now, whatever its rhythm.

## Events

Each round compares what it read with what is stored and emits, in the same transaction as it
stores it (`docs/events.md`): `item.opened`, `item.changed` (with what changed: `state`, `title`,
`assignees`, `labels`, `status`), `item.commented`, `item.reviewed`, `item.checks`, `item.merged`,
`item.closed`. **A source's first round stores without emitting.**

## The protocol

- `items.list {project, kind?, state?, repository?, mine?}`: the stored items, the most recently
  changed first, each with its checks and `seen_at`, and the project's `sources`, each with its
  cursor and problem.
- `items.show {project, kind, id, repository?}`: one stored item.
- `items.refresh {project}`: a round now; answers the sources as they stand after it.

## Not yet

- The subscriptions and the agents' tools reading this stream (#257) and the window listing the
  items (#258); the window says `client: "window"` from then on, so until then the rhythm is
  the one without a window.
- `mine` on Bitbucket Data Center and Jira; Jira's comments (`item.commented` is not emitted for
  a work item: no comment field is asked of the search).

## Tests

On the fake `gh`, `bkt` and `acli` (`crates/flottille/tests/items.rs`), which keep, for a
`--search` with `updated:>=` or a `q` with `updated_on>=`, only what changed at or after it. The
daemon of every other test makes no round of its own (`FLOTTILLE_TEST_NO_FETCH`, which the tests'
harness sets), so that the calls their fakes record are their own.
