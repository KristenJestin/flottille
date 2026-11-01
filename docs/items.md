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
| `fetch_cursors` | Per project and source (`kind`, `target`: a repository's path, or `tracker`): `cursor` (when the last complete round started), `round_at` (when the last round started), `failures` (in a row), `problem` (why the last one failed), `first_at` (when its first round started) and `note` (what the last round could not read: a list cut at its cap, `mine` unknown) |

- **`state`** is `open`, `draft`, `merged` or `closed` for a pull request, `open` or `closed` for
  an issue, and for a Jira work item `closed` once its status is in the done category
  (`fields.status.statusCategory.key` is `done`), `open` otherwise; its status itself is in
  `details.status`.
- **`details`** holds what is proper to a kind or a source: of a pull request `head`, `base`,
  `head_commit`, `mergeable`, `review`, the number of its `comments` and `reviews`, and on
  Bitbucket its `reviewers`; of an issue the number of its `comments`; of a work item its
  `status`, `type`, `priority` and status `category`. A pull request or an issue whose details
  were read also has its `description` (what its author wrote, `""` when nothing; a Bitbucket
  pull request has it from its list too) and its `discussion`: its comments, then its reviews,
  each `{kind, author, bot, by_agent, state, body}` (`kind` is `comment` or `review`, `state` a
  review's verdict, `body` the text without Flottille's mark, `by_agent` when an agent of
  Flottille posted it), which the window's detail shows. They are cut as a ticket's are: the
  description at 20,000 characters, each body at 4,000, and only the newest 30 of the discussion
  kept, `discussion_older` counting the ones left out. An item the forge no longer has, or an
  issue transferred to another repository, is `closed` with `details.removed: true`.
- **`mine`**: opened by, or assigned to, the user the forge says it is signed in as: GitHub's
  `gh api user` (`login`), asked once per daemon, for pull requests and issues alike; Bitbucket
  Cloud's `bkt api /user` (`display_name`, assumed from Atlassian's REST documentation). Jira's
  are those a second search of the same window finds with `AND (assignee = currentUser() OR
  reporter = currentUser())`, so that no user is ever read of `acli` (of `acli jira auth status`,
  only the `Site:` line is read). While GitHub or Cloud does not say who the user is (`gh api
  user` failing), an item keeps the `mine` it had, a new one is not `mine`, and the user is asked
  again at the next round; once it is known, the stored items it was not said of are said again. Bitbucket Data Center says no user in what `bkt` gives: its
  pull requests are never `mine`, and its source's `note` says so.
- Every text is masked before it is stored, as every output is (`flottille_core::secrets`).
- A project's items and cursors go once its file is gone (`project.remove`, or by hand), and a
  source's once the project no longer declares it (a repository removed, its forge or the tracker
  unset), at the next sweep. A round that ends once its project is gone has the next sweep
  compare again, so that what it wrote between its last check and the removal goes too.

## Fetched incrementally

A **round** of a source asks only what changed since its cursor, less five minutes
(`items::OVERLAP_MS`), so that what changed while the last round ran, or a forge whose clock is a
little behind, is not missed; what it reads again unchanged emits nothing. **The overlap is the
only guard** against a search index late behind its forge and a clock skewed: what is changed
more than five minutes before it shows in the search, or on a clock more than five minutes
behind the daemon's, is missed until it changes again (or, for an open GitHub item, until the
hourly comparison below). A source's first round
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
| GitHub issues | `gh issue list -R … --state all --limit 1000 [--search "updated:>=<cursor>"] --json number,title,state,author,assignees,labels,url,updatedAt,createdAt`; only the tracker's repository's issues are kept, as `tickets list` keeps them | `gh issue view <n> --json comments,body` |
| Bitbucket Cloud | `bkt api /repositories/<w>/<r>/pullrequests --param pagelen=50 --param sort=-updated_on --param state=OPEN --param state=MERGED --param state=DECLINED --param state=SUPERSEDED [--param q=updated_on>="<cursor>"]`, page after page up to 1,000, the filter and the order as `--param`, never in the path (#245) | as `forge show` reads it (its commit statuses, reviewers, comments, conflicts) |
| Bitbucket Data Center | no filter in what `bkt` reads: every **open** pull request, `bkt api /rest/api/1.0/projects/<p>/repos/<r>/pull-requests` page after page (`limit`, `start`) up to 1,000; one stored open that left the list is read by itself (`…/pull-requests/<id>`) to say whether it was merged or declined, a 404 making it removed | as `forge show` reads it, only when its `version` or `updatedDate` moved since it was stored |
| Jira | for each project of the tracker, `acli jira workitem search --jql 'project = "<KEY>" [AND updated >= "-<n>m"] ORDER BY updated DESC' --fields key,summary,status,assignee,reporter,labels,issuetype,priority,updated,created --json --limit 1000`, once `acli` is checked to be on the tracker's site (`tickets.md`, "The Jira site"), then the same search with `currentUser()` and `--fields key` for `mine` | none: the search holds them |

- **The checks of an open pull request whose CI still runs** are read again at every round, even
  when the pull request did not change, whether its list showed it again or not (Data Center
  lists every open one each round) (`gh pr view <n> --json statusCheckRollup`; on Bitbucket
  `bkt pr checks <id> --json`); those of a merged or closed one never are again. A read of them
  that fails is said once in the daemon's log for that pull request, and tried again at the next
  round.
- **Once an hour, a GitHub source compares its stored open items with its open list**
  (`gh pr list` or `gh issue list -R … --state open --limit 1000 --json number`), since a deleted
  or transferred item never shows again in a search of what changed: each stored open one missing
  from it, and not listed by the round, is read alone (`gh pr view` or `gh issue view <n> --json
  <the list's fields>`). Not found, or answering with another repository's url (transferred), it
  is `closed` with `details.removed: true`; merged or closed, it takes its state. An open list cut
  at its cap compares nothing. The hour is kept in the daemon's memory: a daemon started again
  compares at its first round after a source's first. Bitbucket has none to compare: Data Center
  reads every open one each round, and Cloud deletes no pull request. Jira's are not compared yet.
- **Only the pull request itself not found makes it removed**: on Bitbucket, a part of it not
  found (its checks, its comments) while its own page is found keeps what is stored of it, and is
  said in the daemon's log.
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
| GitHub pull requests | `gh auth status`, 1 list, 1 `gh pr view` per open pull request, `gh api user` once per daemon | `gh auth status`, 1 list, 1 `gh pr view` per changed one, 1 checks read per open one whose CI still runs; once an hour, 1 open list and 1 `gh pr view` per stored open one missing from it |
| GitHub issues | `gh auth status`, 1 list, 1 `gh issue view` per open issue | `gh auth status`, 1 list, 1 `gh issue view` per changed one; once an hour, 1 open list and 1 `gh issue view` per stored open one missing from it |
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

## What reads this stream

What waits on a forge or a tracker filters these events and reads this store instead of reading
again on its own (#257):

- **The subscriptions** (`docs/design.md` §5.6): a subscription to a pull request (`forge`
  `subscribe`) or to a ticket (`tickets` `subscribe`) is a filter on the `item.*` events of its
  item, offered once they are stored; it holds no read of its own. A pull request's `item.checks`
  is its `checks`, `item.reviewed` its `review` (each new review, or, on Bitbucket, each new
  verdict, `verdicts`), `item.commented` its `comment`, `item.merged` and `item.closed` its
  `merged` and `closed`. A ticket's `item.commented` is its `commented`, an `item.changed` of its
  `status` or of its `state` (a reopening) its `status`, one of its `assignees` its `assigned`,
  `item.closed` its `closed`. The failed jobs' logs a `checks` delivery holds are read then, after
  the round, which does not wait for them; that delivery ends the subscription, so an event that
  comes while they are read (a comment) is not delivered before it, nor after.
- **Their latency is the rhythm** above: a minute while a window is connected, five otherwise. A
  subscription that needs to hear sooner has no faster rhythm of its own (a follow-up, if one
  needs it).
- **`forge list` and `tickets list` and `search`** answer from the store once a round of the
  source worked, with its age (`as of 40 s ago`, from when that round started) and its problem
  when the last round failed; a source no round read yet is read from the forge or the tracker
  as before. A `search` finds the words in the titles: the store keeps no description, and its
  answer says so (`matching "sign" in their titles (not their descriptions nor comments)`).
- **`forge show` and `tickets show` read live** (one item) and store what they read over what the
  store holds of it, with the events a round would emit of it, so that the next round compares
  with that; so does `forge subscribe` and `tickets subscribe` before the subscription opens, so
  that what is so then is no event. What a live read does not say (its author, labels and
  assignees, when it changed, a Jira state) is kept, as are the links of its checks; a pull
  request found again is no longer `removed`. **One no round stored yet** (made since the last
  round, by `forge create` or `tickets create`, or too old to be listed) is stored as the live
  read found it, once its source had a first round, so that what happens to it before the next
  round is an event of that round; it is `item.opened` as in a round, only open and not made
  before that first round (a live read says no creation time: an open one is opened). Its
  assignees and labels, which a live read does not say, are no change then. A Jira work item no
  round stored is not, its live read saying no state.
- **A live read takes the project's round lock before it reads**, so that no round lands between
  the read and its write and none is written over by an older read: `show` reads without storing
  while a round runs (the round stores a read of its own), and holds the lock for its one read,
  not for the logs and runs it reads after. `subscribe` never waits behind a whole round (a round
  holds the lock for every source, minutes for a big project's first): it reads without the
  lock, then waits for it three seconds at most (`items::LIVE_WAIT`), and stores its read only
  when it got it and nothing wrote since its read started (the lock counts what wrote under it);
  otherwise it stores nothing, and the round running or just run stored what the next ones
  compare with. Its answer says when the rounds do not see the item yet: its source had no
  complete round, or the item is not stored (a Jira work item made since the last round).
- A subscription made before this, stored with what its own watch saw, is carried over as it is:
  its target names the same pull request, the next rounds' events are offered to it, and what its
  watch saw is no longer read.

## The protocol

- `items.list {project, kind?, state?, repository?, mine?}`: the stored items, the most recently
  changed first, each with its checks and `seen_at` but without its `description` and
  `discussion` (`items.show`'s), and the project's `sources`, each with its cursor and problem.
- `items.show {project, kind, id, repository?}`: one stored item, its texts included.
- `items.refresh {project}`: a round now; answers the sources as they stand after it.

## Not yet

- `mine` on Bitbucket Data Center; Jira's description and comments (`item.commented` is not
  emitted for a work item: no comment field is asked of the search); a Jira work item deleted or
  moved to another project stays as it was last read.

## Tests

On the fake `gh`, `bkt` and `acli` (`crates/flottille/tests/items.rs`), which keep, for a
`--search` with `updated:>=` or a `q` with `updated_on>=`, only what changed at or after it. The
fake `gh` assumes what the real one does with an item that is gone: a transferred issue is no
longer listed and `gh issue view` of its old number answers with its new url; a deleted one is
neither listed nor found ("Could not resolve to"). The
daemon of every other test makes no round of its own (`FLOTTILLE_TEST_NO_FETCH`, which the tests'
harness sets, and which the daemon's log says once), so that the calls their fakes record are their
own.
