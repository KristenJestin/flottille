# Tickets

The ticket a workspace is about: its title, state, description and comments, read on the tracker
and shown beside the workspace. The second step of the milestone "Forges and tickets"
(`docs/design.md` §13), after [`forges.md`](forges.md). What a workspace shows is read only. The
agents' `tickets` (below) lists, searches, reads and writes the tracker's tickets, the writes
under the gate, on GitHub as on Jira. Locks (a project or a tracker kept read only) are planned,
as their own feature.

Two trackers are read: the issues of GitHub, through the user's own `gh`, and Jira (Cloud),
through Atlassian's own command line, `acli`.

## Linking a workspace to a ticket

A workspace has at most one ticket, kept with it in the store. **The project's tracker says which
tickets it takes**, and nothing is guessed: `[tracker]` in the project's file, set in the project's
Settings in the window (`docs/projects.md`):

| Tracker | Written | Reads |
|---|---|---|
| `kind = "github"`, `repository = "api"` | `#123` (or `123`) | issue 123 of that repository, on its declared GitHub forge |
| `kind = "jira"`, `site`, `projects = ["ABC"]` | `ABC-123` (any case, kept in upper case), its project one of `projects` | the work item `ABC-123` on that site |

`123` is accepted because a shell takes a word starting with `#` for a comment. A ticket is
accepted only in its tracker's form: `ABC-1 is a Jira key, and this project's tracker is GitHub
issues of api: write an issue number, #123`, `#4 is a GitHub issue, and this project's tracker is
Jira on acme.atlassian.net (ABC)`, `XYZ-1 is not in a Jira project of this project's tracker …`.
A project with no tracker takes no ticket: `no tracker set for this project: choose it in the
project's Settings (the window), or write its [tracker] in the project's file; Flottille never
guesses it`. Anything that is neither form is refused when it is typed (`refused: x is not a
ticket …`), so that what reaches a command line is always a number or a key and never an option.
Linking checks the writing of the reference and its tracker, not that the ticket exists: reading
says that.

A GitHub tracker's repository must be one of the project's whose forge is declared on GitHub
(`forges.md`, "The forge of a repository"); its issues are read with `-R` its declared
`owner/name`, once the forge agrees with the repository's remote.

It is linked:

- **at its creation**: `flottille workspace create <project> <name> --ticket ABC-123`
  (`workspace.create {ticket}`), or the optional ticket field of the window's new-workspace
  form;
- **later**: `flottille workspace ticket <project> <name> <ticket>` (`workspace.link_ticket`),
  or the ticket row of the session page's `WORKSPACE` section;
- and **unlinked** with `flottille workspace ticket <project> <name> --clear`, or the row's
  unlink button.

Each change is an event, `workspace.ticket {project, workspace, ticket}` (`docs/events.md`), the
ticket being none when unlinked.

## The user's own command lines

Both are spawned without a shell, as `git` and `gh` are, with the daemon's environment (the
user's), so that the user's own sign-in works; a call is cut after 30 s.

- **Flottille never reads, keeps nor prints a token.** It never runs `gh auth token` nor
  `--show-token`, never reads `gh`'s or `acli`'s configuration folder. Of `gh auth status --hostname
  github.com` it reads the exit code alone: 0 is signed in. Of `acli jira auth status` it reads
  the exit code and the `Site:` line, nothing else (an e-mail there is never kept).
- **What a tracker says is masked** before it is shown or stored (titles, descriptions, comments,
  errors), as every output is (`flottille_core::secrets`).
- **The reads and the writes Flottille runs are known.** Of `acli`, the reads are `acli jira auth
  status`, `acli jira workitem view <KEY> --json --fields summary,status,description,comment` and
  `acli jira workitem search --jql <JQL> --fields key,summary,status,assignee,labels --json
  --limit 200`; its writes (`workitem create`, `comment create`, `edit`, `transition`, below) run
  only for a `tickets` call the gate let through, and the tests look at every call the fake
  received. Of `gh`, the reads are `gh issue view <n> -R github.com/owner/name --json
  number,title,state,body,comments,url` and `gh issue list` (`docs/tools.md`, "The tickets"); its
  writes (`issue create`, `comment`, `edit`, `close`, `reopen`, and for a status `project list`,
  `field-list` and `item-edit`) run only for a `tickets` call the gate let through. A key is parsed first, so it is a number or `ABC-123`, and a JQL filter is
  quoted, never spliced. A text written is passed as one argument, never through a shell.
- **Agents never get that sign-in.** An agent's own command runs have the forge tokens emptied and
  `GH_CONFIG_DIR` pointed at an empty folder (`docs/tools.md`), so `gh` has no login there.
  `acli`'s sign-in lives in the user's home, which no variable of ours moves, so **the gate refuses
  every `acli` line an agent runs, reads included**; an agent reads the tickets with `tickets`.

## The Jira site

**A Jira read is refused when `acli`'s active sign-in is not on the tracker's site**, since `acli`
reads from the site it is signed in to and a key such as `ABC-1` may exist on several: `acli is
signed in to other.atlassian.net, and this project's tracker is Jira on acme.atlassian.net:
nothing was read; switch acli to it (`acli jira auth switch --site acme.atlassian.net`) or correct
the project's tracker`. A status with no `Site:` line is refused the same way. Nothing is asked of
Jira then.

**That check is the guarantee**, and every read and every write goes through it first:
`workspace.ticket`, `project.ticket`, and the agents' `tickets` `show`, `list`, `search` and each
write, which then says `nothing was written`. The work item itself
cannot give it: its `self` link is on the site's own host when `acli` is signed in with an API
token, but on an Atlassian host that names no site when it is signed in with OAuth
(`https://<…>.prod.atl-paas.net/rest/api/3/issue/…`, or
`https://api.atlassian.com/ex/jira/<cloud id>/…`). So a `self` on the tracker's site or on such a
host is taken, and **only a `self` that names another site is refused**, in case the sign-in
changed between the check and the read: `acli answered ABC-123 from other.atlassian.net, and
this project's tracker is Jira on acme.atlassian.net: nothing of it is shown; check the site acli
is signed in to (`acli jira auth status`)`. A work item with no `self` is not shown either.

## What is read

| | GitHub issue | Jira work item |
|---|---|---|
| title | `title` | `fields.summary` |
| state | `open` or `closed` | the status's name (`In Progress`) |
| link | `url` | `https://<site>/browse/<KEY>`, the tracker's configured site |
| description | `body` | `fields.description` |
| comments | `comments[]`: `author.login`, `createdAt`, `body` | `fields.comment.comments[]`: `author.displayName`, `created`, `body` |

A description or a comment longer than 20,000 and 4,000 characters is cut, saying so; only the
newest 30 comments are kept, the number of older ones is said.

What an agent writes is marked ([`forges.md`](forges.md), "What Flottille posts is marked"): a
comment that ends with the marker (GitHub) or the visible footer (Jira) is shown as `<author>
(posted by an agent)`, its text without the mark; a description loses the mark too.

### Jira's text

Jira Cloud's descriptions and comments are Atlassian Document Format (ADF), a JSON tree. They are
turned into plain text simply: the text of paragraphs, headings and code blocks, one block per
line (an empty line between blocks), list items as `- ` or `1. ` lines, table rows as `a | b`, a
mention as its text, a link card as its address, a hard break as a line break. Anything else
(images, attachments, status lozenges) is left out. **Nothing is ever rendered as HTML**; a
description that is already a string is taken as it is.

### What `acli` prints is assumed, and the maintainer checks it

No real `acli` is installed where this was written, none may be run there, and Atlassian's
documentation of `acli jira workitem view`
(<https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem-view/>) gives its
flags (`--json`, `--fields`, default fields `key,issuetype,summary,status,assignee,description`)
and **no example of its output**. The shape read here is therefore an assumption: Jira's REST work
item as `acli … --json` is expected to print it,

```json
{"key": "ABC-123", "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
 "fields": {"summary": "…", "status": {"name": "In Progress"},
            "description": {"type": "doc", "version": 1, "content": ["…"]},
            "comment": {"comments": [{"author": {"displayName": "…"}, "created": "…", "body": {"type": "doc", "…": "…"}}]}}}
```

Also assumed: `acli jira auth status` prints the active site on a line `Site: <host>` (the
site check above reads it, and refuses when it is missing), `--fields summary,status,description,comment` returns the comments (the
documentation's own example is `--fields summary,comment`), and `acli jira auth status` ends with 0
when signed in and non-zero when not (its page does not document its exit codes). The reading
tolerates a description that is a string and `fields.comment` being the array itself. **The
maintainer checks Jira on their work machine** (`flottille workspace ticket <project> <name>
ABC-123`, then `workspace show`); a different shape shows as an empty title or description, and
the fix is in `crates/flottille/src/tickets.rs` (`work_item`).

## What a failure says

**`gh` or `acli` missing, or not signed in**, is said in the ticket's `problem`, with what to do
(`gh auth login`; `acli jira auth login`), and `workspace show` prints it under the ticket's
reference. A ticket the tracker does not know, a call that fails or is cut, an answer that is not
JSON: the same, with the tracker's own first line, masked. The rest of Flottille works as before.

## When it is read

**On demand**: `workspace.ticket {project, name}` (and so `flottille workspace show` and the
window's `WORKSPACE` section, when a page opens or the ticket is linked or unlinked) and
`tickets` `show` read it now, one call; a workspace keeps only the reference, and what it shows
is as fresh as the call.

**And continuously**: every ticket of a declared tracker is read by the daemon on its own,
incrementally, its title, state, assignee, labels and status kept in the store and its changes
said as `item.*` events ([`items.md`](items.md)); its description and comments are not kept.
`tickets` `list` and `search` answer from that store once a round of the tracker worked, saying
its age; `show` reads live and stores what it read; a subscription to a ticket hears those
events.

## The protocol and the command line

- `workspace.ticket {project, name}` answers `{project, workspace, ticket, tracker, repository,
  title, state, url, description, comments: [{author, at, body}], older_comments, read_at,
  problem}`; with no ticket linked, `ticket` is null and the rest is absent; when it cannot be
  read, `problem` says why (`docs/protocol.md`).
- `workspace.link_ticket {project, name, ticket}` links, or unlinks when `ticket` is null, and
  answers as `workspace.ticket` does.
- `workspace.create {…, ticket?}` links at creation.
- `project.ticket {project, ticket}` reads a ticket before any workspace has it, for the
  new-workspace form's preview: `{project, ticket, tracker, tracker_said, problem, …}` with the
  fields of `workspace.ticket`; `problem` says why it cannot be taken or read (no tracker, another
  tracker's form, another Jira site, a read that fails).
- `flottille workspace show <project> <name>` prints the ticket after the steps, and `--json`
  holds what `workspace.ticket` answered under `ticket_read` (the reference itself is
  `ticket`, in `workspace list` too) (`docs/cli.md`).

## The window

The session page's `WORKSPACE` section shows the ticket under the repositories: its reference,
its state and its title, which open the ticket on its tracker with the system's default handler
on a click, or what stops it from being read. Beside it, a field links another ticket (Enter),
and a button unlinks it. The form that makes a workspace has an optional ticket field, and **previews the ticket as it is
typed**: once typing rests (300 ms) it asks `project.ticket` and shows the tracker and the title,
or why the ticket is not taken, before anything is created.

## The agent's tool `tickets`

`tickets {action, …}` (`docs/tools.md`, "The tickets"): `list` every ticket of a state in one call
(by label and assignee), `search` with words, `show` the workspace's ticket or another of the
project's tracker by its key; `create`, `comment`, `edit` (on GitHub also labels and assignees), `close` (on
GitHub, completed or not planned), `reopen` and `status` (on GitHub, the Status of the issue on its
Projects board, `docs/tools.md`), each to judge, held in Ask and allowed in Full as the other writes are. A key the
tracker does not take, or a creation in a Jira project that is not one of the tracker's, is
refused, and nothing is asked of any tracker. A GitHub search stays in the tracker's repository:
a query holding `repo:`, `org:`, `user:`, `is:pr` or `type:pr` is refused, and only the issues
whose link is the tracker repository's are kept, the others counted. Every agent of a project that declares a tracker has
the tool, in `main` as in a workspace, whether a ticket is linked or not (the tool list of a
session is fixed when it starts). Until #141 `show` was `ticket_read`.

`list` and `search` answer from what the daemon's rounds keep ([`items.md`](items.md), "What
reads this stream"), with its age (`… : 2 (as of 3 min ago)`); a `search` finds the words in the
titles, all of them, whatever their case, since no description is kept, and its answer says
so. Before the first round
of the tracker worked, they ask the tracker as they did.

### Subscribing to a ticket

**`subscribe {ticket?, workspace?, events, from?, minutes?}`**: the next of `events` of the
ticket (the one named, or the workspace's) wakes the agent once, as a `subscription` delivery
(`docs/design.md` §5.6), with the same shape as `forge` `subscribe`: `commented` (a new comment,
its author and an excerpt), `status` (Jira's status moved, or the ticket was reopened), `assigned`
(its assignees changed) and `closed`, which ends any subscription to it. `from: people` leaves out
a bot (a GitHub login `app/<name>` or `<name>[bot]`, as for a pull request) and a comment marked
as posted by an agent; `minutes` is 60 by default, 6000 at most. The
ticket is read once and stored first: what is so now is no event. Jira's comments are not read
by the rounds: `commented` on a work item is refused, saying so. Subscribing again to the same
ticket replaces the agent's subscription; **`unsubscribe {ticket?}`** cancels those to the ticket
named, every one when it names none. The events are the rounds' (`item.commented`,
`item.changed`, `item.closed`): a subscription hears as often as the rounds come. It is listed,
cancelled and ends as a pull request's (`docs/protocol.md`, "The subscription methods"); the
window's rail names it `Ticket #5 of api` or `Ticket ABC-1`.

### Jira's writes

Through the user's own `acli`, each once the site is checked (above), the texts passed as they
are written (a text holding what looks like a secret is base for the gate, as on GitHub):

| Write | `acli` |
|---|---|
| `create` | `jira workitem create --project <P> --summary <title> [--description <body>] [--type <type>] [--label a,b] --json`, `P` one of the tracker's projects (the only one when it has one); with no type, the project's default |
| `comment` | `jira workitem comment create --key <K> --body <body>` |
| `edit` | `jira workitem edit --key <K> [--summary <title>] [--description <body>] --yes` |
| `close`, `reopen`, `status` | `jira workitem transition --key <K> --status <name> --yes` |

**`acli` lists no work item's transitions**: its commands
(<https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem/>) move a work item
to a status (`transition --status`) but none says which statuses it can move to, and `view` has no
transitions field. So the statuses of the work item's project are read first, as its work items
stand: `acli jira workitem search --jql 'project = "<P>" ORDER BY updated DESC' --fields
key,status --json --limit 200`, each status with its category (`fields.status.statusCategory.key`:
`new`, `indeterminate`, `done`), and a name is only ever one of them, never guessed:

- `status` takes the one its `status` names, in any case; a name that is not one of them is
  refused with them (`refused: Blocked is not a status of ABC's work items, which are To Do, In
  Progress and Done; nothing was written`);
- `close` moves to the project's done status, `reopen` to its to-do one; when the project has
  none in sight, or several, it is refused, saying them, and `status` names the one meant;
- a transition `acli` refuses (the work item has none to that status from where it is) is said
  with `acli`'s own first line and the statuses again.

A status no work item of the project is in yet is not offered: the maintainer checks on their
own Jira whether `acli` gains a way to list transitions. **Also assumed**, from the same pages,
which give flags and no output: `create --json` prints the created work item with its `key`; a
create with no `--type` takes the project's default type; a refused transition ends with a
non-zero code.

### What `acli jira workitem search` prints is assumed too

Its page (<https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem-search/>)
gives its flags (`--jql`, `--fields`, `--json`, `--limit`, `--paginate`) and no output. The
reading takes an array of work items in the shape `view` prints, or Jira's REST search object
(`{"issues": [...]}`), and reads `key`, `self`, `fields.summary`, `fields.status.name`,
`fields.assignee.displayName` and `fields.labels`. An item whose `self` names another site (not
the tracker's, nor an Atlassian host, as above) is left out and counted; one with no `self` is kept, since the search was of the signed-in site's
projects. The maintainer checks it on their work machine
(`tickets list` from an agent of a Jira project); a different shape shows as an empty list.

## Tests

Tests never call GitHub or Jira nor use anyone's sign-in: a fake `gh` (`crates/flottille-fake-gh`,
which answers `issue view`, `issue list`, the issue writes, the `gh project` calls of a status and the
missing `project` scope too) and a fake `acli`
(`crates/flottille-fake-acli`, `workitem view`, `workitem search` and the writes), first on the
`PATH` of the test's daemon, answer from a script and record every call; the repositories are
throwaway ones whose `origin` is set to a GitHub URL after their workspace is made, and whose
forge and tracker are declared with `project.set_settings`
(`crates/flottille/tests/tickets.rs`).
