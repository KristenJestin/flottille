# Tickets

The ticket a workspace is about: its title, state, description and comments, read on the tracker
and shown beside the workspace. The second step of the milestone "Forges and tickets"
(`docs/design.md` §13), after [`forges.md`](forges.md). What a workspace shows is read only. The
agents' `tickets` (below) lists, searches and reads the tracker's tickets, and writes GitHub
issues under the gate; **Jira is read only**, by the user's decision: an agent drafts a Jira
comment or status change, and the user posts it.

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
- **Jira's reads only are reachable.** Of `acli`, the whole of what Flottille runs is `acli jira
  auth status`, `acli jira workitem view <KEY> --json --fields summary,status,description,comment`
  and `acli jira workitem search --jql <JQL> --fields key,summary,status,assignee,labels --json
  --limit 200`; no write command of it (`workitem create`, `comment`, `transition`, …) appears
  anywhere in the code, and the tests look at every call the fake received. Of `gh`, the reads are
  `gh issue view <n> -R github.com/owner/name --json number,title,state,body,comments,url` and
  `gh issue list` (`docs/tools.md`, "The tickets"); its writes (`issue create`, `comment`,
  `edit`, `close`, `reopen`) run only for a `tickets` call the gate let through. A key is parsed
  first, so it is a number or `ABC-123`, and a JQL filter is quoted, never spliced.
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

**The work item must come from that site too.** The `Site:` line is what `acli` says of its
sign-in; the item it prints says where it came from, in its `self` link. When that link's host is
not the tracker's site, or the item has none, nothing of it is shown: `acli answered ABC-123 from
other.atlassian.net, and this project's tracker is Jira on acme.atlassian.net: nothing of it is
shown; check the site acli is signed in to (`acli jira auth status`)`.

## What is read

| | GitHub issue | Jira work item |
|---|---|---|
| title | `title` | `fields.summary` |
| state | `open` or `closed` | the status's name (`In Progress`) |
| link | `url` | `https://<site>/browse/<KEY>`, the site being the host of `self` |
| description | `body` | `fields.description` |
| comments | `comments[]`: `author.login`, `createdAt`, `body` | `fields.comment.comments[]`: `author.displayName`, `created`, `body` |

A description or a comment longer than 20,000 and 4,000 characters is cut, saying so; only the
newest 30 comments are kept, the number of older ones is said.

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

**On demand only**: `workspace.ticket {project, name}` (and so `flottille workspace show` and the
window's `WORKSPACE` section, when a page opens or the ticket is linked or unlinked) and
`tickets` `show` read it now, one call. Nothing is read in the background, nothing is kept in
memory or in the store but the reference: a ticket is as fresh as the call.

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
project's tracker by its key; on GitHub, `create`, `comment`, `edit`, `close` and `reopen`, each
to judge. A key the tracker does not take is refused, and nothing is asked of any tracker; a write
on Jira is refused before anything is asked. Every agent of a project that declares a tracker has
the tool, in `main` as in a workspace, whether a ticket is linked or not (the tool list of a
session is fixed when it starts). Until #141 `show` was `ticket_read`.

### What `acli jira workitem search` prints is assumed too

Its page (<https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem-search/>)
gives its flags (`--jql`, `--fields`, `--json`, `--limit`, `--paginate`) and no output. The
reading takes an array of work items in the shape `view` prints, or Jira's REST search object
(`{"issues": [...]}`), and reads `key`, `self`, `fields.summary`, `fields.status.name`,
`fields.assignee.displayName` and `fields.labels`. The maintainer checks it on their work machine
(`tickets list` from an agent of a Jira project); a different shape shows as an empty list.

## Tests

Tests never call GitHub or Jira nor use anyone's sign-in: a fake `gh` (`crates/flottille-fake-gh`,
which answers `issue view`, `issue list` and the issue writes too) and a fake `acli`
(`crates/flottille-fake-acli`, `workitem view` and `workitem search`), first on the
`PATH` of the test's daemon, answer from a script and record every call; the repositories are
throwaway ones whose `origin` is set to a GitHub URL after their workspace is made, and whose
forge and tracker are declared with `project.set_settings`
(`crates/flottille/tests/tickets.rs`).
