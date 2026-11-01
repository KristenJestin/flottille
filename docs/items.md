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
| `items` | One row per pull request or ticket: `project`, `kind` (`pull`, `ticket`), `source` (`github`, `bitbucket`, `jira`), `repository` (the project's path of it; empty for Jira), `key` (its number, or its Jira key), `title`, `state`, `author`, `assignees`, `labels`, `url`, `updated_at` (when the forge says it last changed, kept in RFC 3339 UTC, `2026-11-01T10:00:00Z`, whatever form the source writes it in, so that the newest sorts first), `seen_at` (when Flottille last read it, in ms), `mine`, and `details` (JSON) |
| `item_checks` | The checks of a pull request, in order: `name`, `state` (`passing`, `failing`, `pending`, `skipped`), `started_at`, `ended_at`, `link`. The CI moves without the pull request's `updated_at`, so it is kept beside it |
| `fetch_cursors` | Per project and source (`kind`, `target`: a repository's path, or `tracker`): `cursor` (when the last complete round started), `round_at` (when the last round started), `failures` (in a row), `problem` (why the last one failed), `first_at` (when its first round ended) and `note` (what the last round could not read: a list cut at its cap, `mine` unknown) |

- **`state`** is `open`, `draft`, `merged` or `closed` for a pull request, `open` or `closed` for
  an issue, and for a Jira work item `closed` once its status is in the done category
  (`fields.status.statusCategory.key` is `done`), `open` otherwise; its status itself is in
  `details.status`.
- **`details`** holds what is proper to a kind or a source: of a pull request `head`, `base`,
  `head_commit`, `mergeable`, `review`, the number of its `comments` and `reviews`, and on
  Bitbucket its `reviewers`; of an issue the number of its `comments`; of a work item its
  `status`, `type`, `priority` and status `category`. An item the forge no longer has, or an
  issue transferred to another repository, is `closed` with `details.removed: true`.
- **`mine`**: opened by, or assigned to, the user the forge says it is signed in as: GitHub's
  `gh api user` (`login`), asked once per daemon, for pull requests and issues alike; Bitbucket
  Cloud's `bkt api /user` (`display_name`, assumed from Atlassian's REST documentation). Jira's
  are those a second search of the same window finds with `AND (assignee = currentUser() OR
  reporter = currentUser())`, so that no user is ever read of `acli` (of `acli jira auth status`,
  only the `Site:` line is read). Bitbucket Data Center says no user in what `bkt` gives: its
  pull requests are never `mine`, and its source's `note` says so.
- Every text is masked before it is stored, as every output is (`flottille_core::secrets`).
- A project's items and cursors go once its file is gone (`project.remove`, or by hand), and a
  source's once the project no longer declares it (a repository removed, its forge or the tracker
  unset), at the next sweep.

## Fetched incrementally

A **round** of a source asks only what changed since its cursor, less five minutes
(`items::OVERLAP_MS`), so that what changed while the last round ran, or a forge whose clock is a
little behind, is not missed; what it reads again unchanged emits nothing. A source's first round
(no cursor) reads everything. The cursor moves only when a round worked.

**The list is cheap; the details are read one item at a time, and only where they are worth it.**
The list asks no check, review nor comment, which a list of the whole history cannot carry
(GitHub's GraphQL times out on it). An item's details (its checks, reviews and comments) are read
when it is **open** or when it **changed** since it was stored, and never at a first round for a
merged or closed one: a first round of an old repository lists its closed history cheaply and
reads in full its open items alone. **What a round reads is stored every 25 items**
(`items::BATCH`): one that fails halfway keeps what it read, and the next reads again in full only
what was left, its cursor not having moved.

| Source | The list | The details of one |
|---|---|---|
| GitHub pull requests | `gh pr list -R github.com/<owner>/<name> --state all --limit 1000 [--search "updated:>=<cursor>"] --json number,title,state,isDraft,headRefName,headRefOid,baseRefName,url,author,assignees,labels,updatedAt,createdAt` | `gh pr view <n> --json <the fields of forge show>`, read with its parser (`Pull::of`) |
| GitHub issues | `gh issue list -R … --state all --limit 1000 [--search "updated:>=<cursor>"] --json number,title,state,author,assignees,labels,url,updatedAt,createdAt`; only the tracker's repository's issues are kept, as `tickets list` keeps them | `gh issue view <n> --json comments` |
| Bitbucket Cloud | `bkt api /repositories/<w>/<r>/pullrequests --param pagelen=50 --param sort=-updated_on --param state=OPEN --param state=MERGED --param state=DECLINED --param state=SUPERSEDED [--param q=updated_on>="<cursor>"]`, page after page up to 1,000, the filter and the order as `--param`, never in the path (#245) | as `forge show` reads it (its commit statuses, reviewers, comments, conflicts) |
| Bitbucket Data Center | no filter in what `bkt` reads: every **open** pull request, `bkt api /rest/api/1.0/projects/<p>/repos/<r>/pull-requests` page after page (`limit`, `start`) up to 1,000; one stored open that left the list is read by itself (`…/pull-requests/<id>`) to say whether it was merged or declined, a 404 making it removed | as `forge show` reads it, only when its `version` or `updatedDate` moved since it was stored |
| Jira | for each project of the tracker, `acli jira workitem search --jql 'project = "<KEY>" [AND updated >= "-<n>m"] ORDER BY updated DESC' --fields key,summary,status,assignee,reporter,labels,issuetype,priority,updated,created --json --limit 1000`, once `acli` is checked to be on the tracker's site (`tickets.md`, "The Jira site"), then the same search with `currentUser()` and `--fields key` for `mine` | none: the search holds them |

- **The checks of an open pull request whose CI still runs** are read again at every round, even
  when the pull request did not change (`gh pr view <n> --json statusCheckRollup`; on Bitbucket
  `bkt pr checks <id> --json`); those of a merged or closed one never are again.
- **A list cut at its cap** (GitHub's search gives 1,000 results at most; Bitbucket's pages stop at
  1,000) says so in its source's `note`. An item first seen later, once the history moved, is
  stored without `item.opened` when it was made before its source's first round (`first_at`).
- **The cursor is a time** in milliseconds on the daemon's clock. GitHub and Bitbucket take it as
  RFC 3339 in UTC (`2026-11-01T10:00:00Z`). Jira takes it as **minutes before now**
  (`updated >= "-12m"`, rounded up): a date in JQL is read in the time zone of the user's Jira
  profile, which Flottille does not know, while a relative time is the same everywhere.
- **A round whose items cannot be stored** waits as one that failed to read, and says why.
- **A project removed during a round** keeps none of what the round read: nothing is written once
  its file is gone.
- **Assumed, and the maintainer checks it on a real forge**: that `gh pr list --search` and
  `gh issue list --search` take `updated:>=<RFC 3339>` with `--state all` (GitHub's search
  documents the qualifier; `gh` adds no state qualifier for `all`); that `bkt api` passes repeated
  `--param state=…` and a `q` holding quotes as query parameters, encoded, and that Bitbucket's
  filter takes `updated_on>="<RFC 3339>"`; that Cloud's pull request object says
  `author.display_name`, `updated_on` and `created_on`, and `/user` says `display_name`; that Data
  Center's says `version`, `updatedDate` and `createdDate` in milliseconds; that `acli … search
  --limit 1000` reads up to that many, takes `currentUser()`, and its fields hold `reporter`,
  `issuetype`, `priority`, `updated` and `created` in Jira's REST shape; that `gh api user` prints
  the user's `login`; that `gh pr view` and `gh issue view` say "Could not resolve to" for a
  number the repository no longer has.

### What a round costs

| Source | A first round | A round after it |
|---|---|---|
| GitHub pull requests | `gh auth status`, 1 list, 1 `gh pr view` per open pull request, `gh api user` once per daemon | `gh auth status`, 1 list, 1 `gh pr view` per changed one, 1 checks read per open one whose CI still runs |
| GitHub issues | `gh auth status`, 1 list, 1 `gh issue view` per open issue | `gh auth status`, 1 list, 1 `gh issue view` per changed one |
| Bitbucket Cloud | the contexts, 1 page per 50 pull requests, about 4 calls per open one (its checks, the pull request, its comments, its diffstat), `/user` once per daemon | the contexts, 1 page per 50 changed, about 4 calls per changed one, 1 checks read per open one whose CI still runs |
| Bitbucket Data Center | the contexts, 1 page per 50 open pull requests, about 3 calls per open one | the contexts, 1 page per 50 open, about 3 calls per changed one, 1 per stored open one that left the list, 1 checks read per open one whose CI still runs |
| Jira | `acli jira auth status`, 2 searches per project of the tracker | the same |

## Rhythm

- **Every minute while a window is connected** to the daemon (a connection that called
  `events.watch` with `client: "window"`), **every 5 minutes otherwise**; both in the daemon's
  configuration, `$XDG_CONFIG_HOME/flottille/daemon.toml`, read again once its file changed:

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
- `mine` on Bitbucket Data Center; Jira's comments (`item.commented` is not emitted for
  a work item: no comment field is asked of the search).

## Tests

On the fake `gh`, `bkt` and `acli` (`crates/flottille/tests/items.rs`), which keep, for a
`--search` with `updated:>=` or a `q` with `updated_on>=`, only what changed at or after it. The
daemon of every other test makes no round of its own (`FLOTTILLE_TEST_NO_FETCH`, which the tests'
harness sets, and which the daemon's log says once), so that the calls their fakes record are their
own.
