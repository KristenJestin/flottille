# Tickets

The ticket a workspace is about: its title, state, description and comments, read on the tracker
and shown beside the workspace. The second step of the milestone "Forges and tickets"
(`docs/design.md` §13), after [`forges.md`](forges.md). **Reading only**: nothing here writes to a
tracker. An agent drafts a comment or a status change; the user posts it.

Two trackers are read: the issues of GitHub, through the user's own `gh`, and Jira (Cloud),
through Atlassian's own command line, `acli`.

## Linking a workspace to a ticket

A workspace has at most one ticket, kept with it in the store. It is written as the tracker
writes it:

| Written | Tracker | Reads |
|---|---|---|
| `#123` (or `123`) | GitHub issues | issue 123 of the first repository of the workspace that is on GitHub |
| `ABC-123` (any case, kept in upper case) | Jira | the work item `ABC-123` |

`123` is accepted because a shell takes a word starting with `#` for a comment. Anything else is
refused when it is typed (`refused: x is not a ticket: write #123 … or a Jira key such as
ABC-123`), so that what reaches a command line is always a number or a key and never an option.
Linking checks the writing of the reference, not that the ticket exists: reading says that.

The repository of `#123` is the first of the workspace's repositories (the root one first, then
in dependency order) whose remote is on GitHub (`forges.md`, "The forge of a repository").
Simplest rule that works, and the answer says which repository it took (`acme/api`). A project
cannot yet name the repository its issues are in; if that is wanted, it is a key of the project's
file, later. A workspace with no repository on GitHub has no `#123` to read, and says so; a Jira
key needs no repository.

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
  `--show-token`, never reads `gh`'s or `acli`'s configuration folder. Of each one's sign-in
  status (`gh auth status --hostname github.com`, `acli jira auth status`) it reads the exit
  code alone: 0 is signed in.
- **What a tracker says is masked** before it is shown or stored (titles, descriptions, comments,
  errors), as every output is (`flottille_core::secrets`).
- **Only reads are reachable.** The whole of what Flottille runs is `gh issue view <n> -R
  owner/name --json number,title,state,body,comments,url`, `acli jira auth status` and `acli
  jira workitem view <KEY> --json --fields summary,status,description,comment`. The key is
  parsed first, so it is a number or `ABC-123`; no write command of either (`issue create`,
  `workitem create`, `comment`, `transition`, …) appears anywhere in the code, and the tests
  look at every call the fakes received.
- **Agents never get that sign-in.** An agent's own command runs have the forge tokens emptied and
  `GH_CONFIG_DIR` pointed at an empty folder (`docs/tools.md`), so `gh` has no login there.
  `acli`'s sign-in lives in the user's home, which no variable of ours moves, so **the gate refuses
  every `acli` line an agent runs, reads included**; an agent reads a ticket with `ticket_read`.

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

Also assumed: `--fields summary,status,description,comment` returns the comments (the
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
`ticket_read` read it now, one call. Nothing is read in the background, nothing is kept in
memory or in the store but the reference: a ticket is as fresh as the call.

## The protocol and the command line

- `workspace.ticket {project, name}` answers `{project, workspace, ticket, tracker, repository,
  title, state, url, description, comments: [{author, at, body}], older_comments, read_at,
  problem}`; with no ticket linked, `ticket` is null and the rest is absent; when it cannot be
  read, `problem` says why (`docs/protocol.md`).
- `workspace.link_ticket {project, name, ticket}` links, or unlinks when `ticket` is null, and
  answers as `workspace.ticket` does.
- `workspace.create {…, ticket?}` links at creation.
- `flottille workspace show <project> <name>` prints the ticket after the steps, and `--json`
  holds what `workspace.ticket` answered under `ticket_read` (the reference itself is
  `ticket`, in `workspace list` too) (`docs/cli.md`).

## The window

The session page's `WORKSPACE` section shows the ticket under the repositories: its reference,
its state and its title, which open the ticket on its tracker with the system's default handler
on a click, or what stops it from being read. Beside it, a field links another ticket (Enter),
and a button unlinks it. The form that makes a workspace has an optional ticket field.

## The agent's tool `ticket_read`

`ticket_read {ticket?}` (`docs/tools.md`): the workspace's ticket, or `ticket`, another ticket of
the same tracker by its key. The tracker is that of the workspace's ticket: GitHub issues for
`#123`, Jira for `ABC-123`; a workspace linked to none has GitHub's (the issues of its own
repositories), so a Jira key is refused until a Jira ticket is linked. A key of the other tracker
is refused, and nothing is asked of any tracker. Read only, through the gate like every call (it
allows it). Only an agent working in a workspace has the tool, whether a ticket is linked or not
(the tool list of a session is fixed when it starts).

## Tests

Tests never call GitHub or Jira nor use anyone's sign-in: a fake `gh` (`crates/flottille-fake-gh`,
which answers `issue view` too) and a fake `acli` (`crates/flottille-fake-acli`), first on the
`PATH` of the test's daemon, answer from a script and record every call; the repositories are
throwaway ones whose `origin` is set to a GitHub URL after their workspace is made
(`crates/flottille/tests/tickets.rs`).
