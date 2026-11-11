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
  status`, `acli jira workitem view <KEY> --json --fields summary,status,description,comment,issuelinks,attachment,assignee,parent,*navigable`
  (`--fields status` or `--fields comment,attachment` alone for the status of a write and the rounds'
  comments), and `acli jira workitem search --jql <JQL> --fields key,summary,status,assignee,labels --json
  --limit 200`, `workitem link type --json` and `workitem view <KEY> --json --fields issuelinks` (the link types and an item's links; the JQL of a `list` or a `search` is in `docs/tools.md`); its writes (`workitem create`, `comment create`, `comment update`, `comment delete`, `edit`, `transition`, `link create`, `link delete`, below) run
  only for a `tickets` call the gate let through, and the tests look at every call the fake
  received. Of `gh`, the reads are `gh issue view <n> -R github.com/owner/name --json
  number,title,state,body,comments,url,milestone` and `gh issue list` (`docs/tools.md`, "The tickets");
  `gh api repos/<owner>/<name>/milestones` lists a repository's milestones ("Milestones" below); its
  writes (`issue create`, `comment`, `edit`, `close`, `reopen`, for milestones `gh api` with `--method POST` and `PATCH`, for a status `project list`,
  `field-list` and `item-edit`, and for links and comments `gh api` on the issue's `sub_issues`,
  `dependencies/blocked_by` and `issues/comments/<id>`, read first with `gh api` on the issue for its id) run only for a `tickets` call the gate let through. A key is parsed first, so it is a number or `ABC-123`, and a JQL filter is
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
| state | `open` or `closed` | the status's name (`In Progress`), and its category (`statusCategory.key`) |
| link | `url` | `https://<site>/browse/<KEY>`, the tracker's configured site |
| description | `body` | `fields.description` |
| comments | `comments[]`: `author.login`, `createdAt`, `body` | `fields.comment.comments[]`: `author.displayName`, `created`, `body` |

A description or a comment longer than 20,000 and 4,000 characters is cut, saying so; only the
newest 30 comments are kept, the number of older ones is said.

What an agent writes carries nothing of Flottille, and is known by Flottille's record of it
([`forges.md`](forges.md), "What Flottille posts is recorded"): a comment it posted is shown as
`<author> (posted by an agent)`, by the id GitHub gave it or, where the tracker gave none (Jira),
by a hash of its text, one comment for one post. A description is the agent's while it still reads
as the agent left it. A comment or a description that still ends with the old marker (GitHub) or
footer (Jira) is shown the same way, without it.

### Jira's text

Jira Cloud's descriptions and comments are Atlassian Document Format (ADF), a JSON tree. **An
agent writes Markdown, as on every forge**, and Flottille converts it on each Jira write (below);
the tree it reads is turned back into the Markdown it came from (`crates/flottille/src/adf.rs`), so
what an agent reads is what it wrote, and an edit keeps the formatting it did not touch. A
description that is already a string is taken as it is. **Nothing is ever rendered as HTML.**

| Markdown | ADF |
|---|---|
| `#` to `######` | `heading` (level 1 to 6) |
| paragraphs; a line break inside one | `paragraph`; `hardBreak` |
| `**bold**`, `*italic*`, `~~strike~~`, `` `code` `` | the marks `strong`, `em`, `strike`, `code` |
| a fenced block, with its language | `codeBlock` (`attrs.language`) |
| `-` and `1.` lists, nested | `bulletList`, `orderedList` with `listItem` |
| `[text](url)`, a bare address | the `link` mark |
| `> quote`, `---` | `blockquote`, `rule` |
| a table, as wide in every row as its header | `table` with `tableHeader` and `tableCell` |

**Nothing is dropped.** Raw HTML, an image (`![alt](url)`), and a table Jira would not take stay in
the document as the text they were written as. Inside a quote or a list item, where ADF takes only
paragraphs, lists and code blocks, a heading becomes a bold paragraph and a table or a rule stays
text. A code mark goes beside a link and no other mark, as Jira holds it. A text with no Markdown
in it is paragraphs, a line break staying a line break.

**Reading** gives, for the same nodes, the Markdown above (a table is `| a | b |` rows under a
`| --- |` line), a mention as its text, a link card as its address, a hard break as a line break.
An image or a file (`mediaSingle`, `mediaGroup`, `media`, `mediaInline`) is a line where it stands:
`[image: <name>, not shown: ask the user to paste it if it matters]`, or `[file: <name>, not
shown]`; an external image (`type: "external"`) gives its address as its name. The name is the
media's `alt`, which is the attachment's file name (the media's `id` is not the attachment's), and
the type is the attachment's `mimeType` when one has that name, else the name's extension. Flottille
cannot show it: `acli` has no command that downloads an attachment, and its address needs the
user's credentials, which Flottille never reads, so that address is never fetched nor shown. The
same text is what a subscription delivers of a new comment. Anything else (status lozenges) is
left out. A character that Markdown would
take for markup (`*`, `` ` ``, `[`, `<`, `~`, `_` at a word's edge, a line starting with `#`, `>`,
`-` or `1.`) is written with a backslash, so that it comes back as the text it was.

**What an agent posts is found again by its words.** The record of a Jira comment is a hash of its
text (`forges.md`, "What Flottille posts is recorded"). What is recorded is the text after the
round trip (Markdown, ADF, Markdown), the text a read gives, so the match does not rely on Jira
keeping anything but the tree. What Jira rewrites on its own (a mention, a smart link) still
breaks the match: that comment is then shown as a person's and `from: people` delivers it. The
maintainer checks one real Jira comment, as for the shapes below.

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
site check above reads it, and refuses when it is missing), `--fields summary,status,description,comment,issuelinks,attachment,assignee,parent,*navigable` returns the comments and the attachments (the
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
`tickets` `show` read it now, or give the read of the last 30 s ("How fast it is read" below); a
workspace keeps only the reference, and what it shows is as fresh as that read.

**And continuously**: every ticket of a declared tracker is read by the daemon on its own,
incrementally, its title, state, assignee, labels and status kept in the store and its changes
said as `item.*` events ([`items.md`](items.md)); its description is not kept, nor its comments,
except those of a Jira work item a subscription follows (below).
`tickets` `list` answers from that store once a round of the tracker worked, saying its age;
`search`, and a `list` with `updated_within`, ask the tracker; `show` reads live and stores what it read; a subscription to a ticket hears those
events.

### Attachments

A work item's attachments (`fields.attachment`, read with the item, so cached with it) are listed
at the end of `tickets show`, one line each: file name, type, size, who added it and when, the first
30 and then `… and N more`. They are named, never shown (the same words as an image in a text): the user can paste one into the
conversation. `workspace.ticket` gives them all as `attachments: [{name, type, bytes, author, at}]`.
Their `content` address is never kept.

## How fast it is read

- **Together.** The check that the program is signed in (to the right site, for Jira) and the read
  of the ticket itself are asked at the same time, not one after the other. When the check says
  no, its problem is the one said and nothing of what the tracker answered is shown, as before;
  the ticket was asked of the tracker meanwhile, which used to be avoided and is not now (a
  Jira work item is still checked to come from the declared site).
- **Given again for 30 s.** The daemon keeps the last read of each ticket, by project and key,
  and gives it to the next `tickets` `show` of it and to `workspace.ticket` (so to the window's
  session link, and to linking a ticket then opening its page), saying how old it is on `show`
  (`(as read 12 s ago; "fresh": true reads it again)`) and by `read_at` in `workspace.ticket`.
  `fresh: true` and `detail: full` read it again, and keep what they read. A read that fails is
  never kept. A write through Flottille (`tickets` `create`, `comment`, `edit`, `close`, `reopen`,
  `status`) drops everything kept. A subscription's own reads never use it.

[`forges.md`](forges.md), "How fast `forge show` is read", has the same for pull requests and the
developer log's `reads`.

## The protocol and the command line

- `workspace.ticket {project, name}` answers `{project, workspace, ticket, tracker, repository,
  title, state, url, description, comments: [{author, at, body}], older_comments, attachments,
  read_at,
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

## The program it needs

Choosing Jira says whether `acli` is found, its version and whether it is signed in (`acli jira
auth status`'s exit code), and GitHub issues the same for `gh`, in the project's Settings and in
`flottille doctor`; a missing `acli` shows its installation page
(<https://developer.atlassian.com/cloud/acli/guides/install-acli/>), a signed-out one `acli jira
auth login` with a button that copies it. See [`forges.md`](forges.md), "The programs it needs".

## The window

The session page's `WORKSPACE` section shows the ticket under the repositories: its reference,
its state and its title, which open the ticket on its tracker with the system's default handler
on a click, or what stops it from being read. Beside it, a field links another ticket (Enter),
and a button unlinks it. The form that makes a workspace has an optional ticket field, and **previews the ticket as it is
typed**: once typing rests (300 ms) it asks `project.ticket` and shows the tracker and the title,
or why the ticket is not taken, before anything is created.

## What a spec writes

With Specs on (`docs/extensions.md`, "Specs"), the user's gestures on a spec write to the tracker,
through the same writes as the agents' `tickets` (below: `gh issue edit`, `create`, `comment`;
`acli jira workitem edit`, `create`, `comment create`), recorded the same way, and nothing of
Flottille in them: no marker, no footer, no "by an agent". They are the user's gestures, so the
gate is not asked; nothing else writes a ticket for a spec.

- **The ticket is the spec.** At each Ready the spec's tickets are read again (`gh issue view`,
  `acli jira workitem view`). Each one whose description is as Flottille last read or wrote it (a
  hash of its whole text, masked, whitespace and the markup a tracker rewrites left out, as for
  the record of a post) gets the spec as its description, in Markdown: its sections in their
  order, `C` and `D` numbered. The ticket is read again after the write, and what the tracker holds
  then becomes the last read (what was sent, when that read fails: a tracker rewrites markup, a
  link kept as its text), and the description is
  recorded as Flottille's by the ticket's key ([`forges.md`](forges.md), "What Flottille posts is
  recorded"). A ticket Flottille never read (a spec made with `spec new --ticket`) is not known to
  be as it last read it (it says "Flottille has not read ABC-8 yet"): the user chooses, as when it
  changed. A spec with no ticket creates one with its title, in the tracker's only Jira project
  (with several, the creation fails and says so); when the ticket is opened and its key cannot be
  read, the spec says to link it with `spec link`. Any failure says why on the spec, which stays
  Ready.
- **Internal.** The ticket is read, never written. The comment of where the spec departs from it
  is posted at Done only by the user (`spec.departures {post}`), on each ticket, and recorded by
  the id of the comment, as an agent's comment is: GitHub gives it, and on Jira it is bound by the
  read of the work item right after the post. A comment whose id is not found is recorded by
  nothing, never by its words: it is not marked as Flottille's, and a person's comment with the
  same words never is. The post is kept per ticket: one that failed on a ticket is made again only on those it did not reach.

The rounds keep no description, so a description a spec wrote emits no event. A comment the user
posts for a spec is recorded as an agent's is: it shows `posted by an agent`, and `from: people`
leaves it out.

## The agent's tool `tickets`

`tickets {action, …}` (`docs/tools.md`, "The tickets"): `list` every ticket of a state in one call
(by label, assignee and `updated_within`), `search` with words, `in_comments` or `jql`, `show` the workspace's ticket or another of the
project's tracker by its key, `types` the work item types of a Jira project, `sprints` the open sprints of its boards, `milestones` the milestones of a GitHub repository; `create` (on Jira also under a parent and in a sprint; on GitHub in a milestone), `milestone_create` and `milestone_edit` (GitHub, "Milestones" below), `comment`, `log_time` (Jira, "Logging time" below) and `time_spent`, `edit` (labels on both; on GitHub assignees, on Jira the assignee), `close` (on
GitHub, completed or not planned), `reopen` and `status` (on GitHub, the Status of the issue on its
Projects board, `docs/tools.md`), each to judge, held in Ask and allowed in Full as the other writes are. A key the
tracker does not take, or a creation in a Jira project that is not one of the tracker's, is
refused, and nothing is asked of any tracker. A GitHub search stays in the tracker's repository:
a query holding `repo:`, `org:`, `user:`, `is:pr` or `type:pr` is refused, and only the issues
whose link is the tracker repository's are kept, the others counted. Every agent of a project that declares a tracker has
the tool, in `main` as in a workspace, whether a ticket is linked or not (a session running
when the tracker is declared is told, `docs/tools.md`, "When the tools change"). Until #141 `show` was `ticket_read`.

`list` answers from what the daemon's rounds keep ([`items.md`](items.md), "What reads this
stream"), with its age (`… : 2 (as of 3 min ago)`); before the first round of the tracker worked,
it asks the tracker. A `search` always asks the tracker, since the rounds keep no description nor
comment: Jira's `text ~` (`comment ~` for `in_comments`), GitHub's `in:title,body,comments`, its
answer saying where it looked; the same search within 30 s is given again. `updated_within` is a
date the rounds do not keep of a Jira work item (`acli` refuses `updated`): a list with it asks
the tracker too. An agent's `jql` is put in parentheses after the tracker's projects, and refused
when it could leave them (`docs/tools.md`, "The tickets").

### Subscribing to a ticket

**`subscribe {ticket?, workspace?, events, from?, minutes?}`**: each of `events` of the ticket
(the one named, or the workspace's) wakes the agent, as a `subscription` delivery
(`docs/design.md` §5.6), until the ticket is closed, with the same shape as `forge` `subscribe`:
the agent subscribes once (`it wakes you at each of these events until the ticket is closed; no
need to subscribe again`). The events are `commented` (a new comment, its author and an excerpt),
`status` (Jira's status moved, or the ticket was reopened), `assigned` (its assignees changed) and
`closed`, which ends any subscription to it, said: a GitHub issue closed, or a Jira work item moved
to a status of the category `done`, whatever the status is called (its item's state is `closed`
then, "What is read"). `from: people` leaves out a bot (a GitHub login `app/<name>` or
`<name>[bot]`, as for a pull request) and a comment known as posted by an agent; `minutes`,
optional and with no cap, is a deadline: past it the agent is woken once and the subscription
ends; without it there is none. A ticket already closed (or a Jira work item already in a `done`
status) is refused, `the ticket ABC-1 is already closed (it is in Done)`: nothing would end the
subscription. `show` ends with the agent's subscriptions to tickets (`Your
subscriptions:` then `- ticket #5 of api: commented, until the ticket is closed`, or `for 52 min
more` with a deadline). The
ticket is read once and stored first: what is so now is no event. The answer says when the rounds
do not see it yet, and so miss what happens before their next one: the tracker had no complete
round (it never worked), or the item is not stored (a Jira work item whose status Jira gives no
category for). **A Jira work item's comments are read by the rounds only while a subscription
to its `commented` follows it** (`acli jira workitem view <KEY> --json --fields comment`, one
call per followed work item that changed: the one the round's search lists in its window, since a
search never prints `updated`, below), since a tracker's every work item would cost one call each;
a comment is then an `item.commented` as on GitHub, and an agent's own, known by its record,
is said `posted by an agent` and left out of `from: people`. Subscribing again to the same
ticket replaces the agent's subscription (its events, `from` and deadline); **`unsubscribe {ticket?}`** cancels those to the ticket
named, every one when it names none. The events are the rounds' (`item.commented`,
`item.changed`, `item.closed`): a subscription hears as often as the rounds come. It is listed,
cancelled and ends as a pull request's (`docs/protocol.md`, "The subscription methods"); the
window's rail names it `Ticket #5 of api` or `Ticket ABC-1`.

### Jira's writes

Through the user's own `acli`, each once the site is checked (above), the texts passed as they
are written (a text holding what looks like a secret is base for the gate, as on GitHub):

| Write | `acli` |
|---|---|
| `create` | `jira workitem create --project <P> --summary <title> [--description-file <file>] [--assignee <who>] [--type <type>] [--label a,b] [--parent <key>] --json`, `P` one of the tracker's projects (the only one when it has one); with no type, the project's default |
| `create` in a sprint | `jira workitem create --from-json <file> --json`, then `jira workitem edit --key <K> --assignee <who> --yes` when it has an assignee ("Parents and sprints", below) |
| `comment` | `jira workitem comment create --key <K> --body-file <file>` |
| `edit` | `jira workitem edit --key <K> [--summary <title>] [--description-file <file>] [--assignee <who> \| --remove-assignee] [--labels a,b] [--remove-labels c] --yes` |
| `close`, `reopen`, `status` | `jira workitem transition --key <K> --status <name> --yes` |

**The text of a description and of a comment is Markdown converted to ADF** (above), and passed
to `acli` through a file (`--description-file`, `--body-file`; `acli` takes "plain text or Atlassian
Document Format" in both), never on the command line: the file is in a folder of the daemon's
runtime folder that only the user can open (`0700`, the file `0600`), and both are removed once
`acli` has answered. The conversion is of the text the gate judged, as it was shown (one that looks like a secret is
base for the gate, and nothing is masked in what is sent); the Markdown parser is the `markdown` crate,
already in the tree for the window. **The summary, the description and the assignee of `create`
and `edit` are built in one place** (`work_item_fields`).

### Links and comments

The family of what ties tickets together, and of an agent's own comments (#365). Every write of it
goes through the gate like the others; `link_types` and what `show` adds are reads.

**A link reads `ticket kind other`**: `{"action": "link", "ticket": "ABC-2", "kind": "blocks",
"other": "ABC-1"}` is `ABC-2 blocks ABC-1`, and `kind: "is blocked by"` with the two swapped is the
same link. The gate's card says it in those words (`it links two tickets on the tracker: ABC-2
blocks ABC-1`), and so does the result, from both ends (`The link was made: ABC-2 blocks ABC-1
(ABC-1 is blocked by ABC-2).`). `ticket` is always named, never the workspace's, so that a card
cannot say a link of a ticket it does not name; `other` is a ticket of the same tracker, not the
ticket itself.

- **On Jira**: `link_types` is `acli jira workitem link type --json`, which names the site's types
  only (`{"issueLinkTypes": [{"name": "Blocks"}, …]}`); each is listed with Jira's own wordings for
  its standard types, Blocks, Cloners, Duplicate and Relates (`blocks (is blocked by) [Blocks]`), and
  by its name alone otherwise (`- Escalates`). `link` finds the kind among the outward wordings (or a
  type's name: `ticket` is on the outward end), then the inward ones (`ticket` is on the inward end),
  in any case, and runs `acli jira workitem link create --out <the item the outward wording points
  at> --in <the other> --type <the type's name> --yes`: `A blocks B` is out B, in A. On a real site
  (#520), `--out A --in B --type Blocks` made Jira's page and its JQL say `A is blocked by B`
  (`issue in linkedIssues(A, "is blocked by")` is B), though acli printed `A Blocks B`: its message
  is not the direction. A kind the site does not have is refused before the user is asked, naming
  the ones it has (`"devours" is not a link type of example.atlassian.net: its kinds are blocks, is
  blocked by, relates to, …`). `unlink` reads the links of `ticket` (below), finds the link to
  `other` of that type with `ticket` at that end and deletes it by its id (`link delete --id <id>
  --yes`, which takes ids and not items); none is refused, saying the item's links. A parent
  (`parent of`, `sub-task of`) is refused on Jira: `acli jira workitem edit` has no flag for a
  parent, only `workitem create --parent` has.
- **On GitHub**: `parent of`, `sub-task of` (also `sub-issue of`), `blocks` and `blocked by`, through
  `gh api repos/<owner>/<name>/issues/<n>/sub_issues` and `…/dependencies/blocked_by` (a
  `--method POST`, a `DELETE` to unlink), with the id of the issue that the endpoint takes, read
  with `gh api repos/<owner>/<name>/issues/<n>`. No other relation exists in its API (a pull
  request closes an issue with `Closes #n` in its description, which is `forge edit`): the other
  kinds are refused, naming that.
- **`show` lists the links** of a Jira work item, after its comments: `Links (2):` and a line each,
  `- is blocked by ABC-2 (In Progress): Add search`. They are read in the same `acli jira workitem view
  <KEY> --json --fields summary,status,description,comment,issuelinks,attachment,assignee,parent,*navigable` as the ticket and kept with it in
  the 30 s cache, so a cached `show` runs nothing and still lists them; a write drops the cache. A
  write that names a link (`unlink`) reads `--fields issuelinks` alone. `link list --key <KEY> --json`
  is not used: it gives only `{"issueLinks": [{"id", "outwardIssueKey", "typeName"}]}`, the key `null`
  when the item is the outward one, so it
  cannot name the other item. Each of `issuelinks` is `{id, type: {name, inward, outward},
  inwardIssue, outwardIssue}`, one of the two null, the field holding the other item, and **it
  reads with the wording the field is named after**, as Jira's page says it (#520): on A,
  `outwardIssue: B` is `A blocks B` (the outward wording) and `inwardIssue: B` is `A is blocked by
  B` (the inward one). Measured on a real site: a link made with `--out A --in B --type Blocks` is
  on A `inwardIssue: B` and on B `outwardIssue: A`, and Jira says B blocks A. Web links are not
  among them and are not listed. GitHub's sub-issues are not listed yet.
- **Comments**: `comment_edit` and `comment_delete` change only a comment Flottille recorded by its
  id as posted by an agent (`posts`, "What Flottille posts is recorded", `forges.md`; on Jira the id
  is read right after the post, never later by its words), found by the id that
  `show` with `detail full` prints after its author (`[comment 10100]`, GitHub's `issuecomment-501`);
  any other comment, a person's or another tool's, is refused whatever its author's name, and a
  comment that cannot be found among the newest ones is said; both before the user is asked, and
  checked again right before the write. On Jira, `acli jira workitem comment
  update --key <K> --id <id> --body-adf <file>` (the text as ADF, as for `comment`; `update`'s
  `--body-file` takes plain text only) and `comment
  delete --key <K> --id <id>`; on GitHub, `gh api repos/<owner>/<name>/issues/comments/<id>
  --method PATCH -f body=<text>` and `--method DELETE`. An edit replaces the record of the id with the
  new words, so that the comment stays the agent's and has one record; a delete drops it.
**Measured on a real Jira Cloud site**: the shapes of `link type --json`, `link list --json` and
`view --fields issuelinks` above, `link delete --id <link id> --yes`, and `comment list --key <K>
--json`, whose comments each carry their `id` (their `body` plain text). The view's comments are
taken to carry their `id` too, as in Jira's REST.

### Types and assignment

Seen in daily use: an agent guessed a work item type in the user's language (`Tâche`), Jira
refused it, and the new work item went to the project's default assignee.

- **`types {project?}`** lists a Jira project's work item types, with `acli jira project view
  --key <P> --json` (not `workitem create --generate-json`, which describes the creation form and
  not the project), after the site check; the project is left out when the tracker has one. The
  list is kept five minutes per site and project, in the daemon's memory. A type that is a
  sub-task is said so (`Sub-task (a sub-task)`). On GitHub the answer is that issues have no
  type; `gh` lists no issue types that Flottille reads. The `create` description tells the agent
  to read them when unsure.
- **A `create` refused for its type**: when `acli` ends with an error and a `type` was given, the
  project's types are read (the same call) and, if the type named is none of them (any case), the
  answer is `"Tâche" is not a work item type of ABC: its types are Task, Bug and Story; nothing
  was written; …` with `acli`'s own line. If the type is one of them, `acli`'s line is the answer,
  as before: Flottille does not read an error's wording to find its cause.
- **`assignee`** on Jira `create` (`--assignee`) and `edit` (`--assignee`; `assignee: null` is
  `--remove-assignee`): `"me"`, an email or an account id, **as `acli` takes them**. `me` is the
  user the agent works for, sent as `acli`'s `@me` (`"@me"` is taken too), which `acli` resolves
  itself (`acli jira workitem create --help`: "Use '@me' to self-assign"), so Flottille reads no
  account and no credential for it. An account id is 24 hexadecimal digits or `<number>:<uuid>`.
  **Anything else is refused before anything is asked**: seen in daily use, an agent gave the
  user's display name, and `acli` refused the whole creation (`invalid format for assignee email:
  mail: missing '@' or angle-addr`); `acli` cannot look a name up. The answer is `assignee is me,
  an email or an account id; a display name cannot be looked up with acli`. A blank assignee is no
  assignee; a `null` one is told from a missing one (a blank is never a removal). On GitHub it is
  `assignees` (`edit`, `--add-assignee`, `me` sent as `gh`'s `@me`); `assignee` there is refused,
  and on `comment`, `close`, `reopen` and `status` on both trackers.
- **An assignee the site does not know**: a `create` `acli` refuses while it names an assignee
  answers `acli refused the creation assigned to "bob@example.org": … User not found for email:
  …; nothing was created`, asking the agent to ask the user before creating it without one: it is
  never tried again without its assignee. `acli jira workitem edit` is a bulk command: with an
  unknown assignee it ends with 0 and prints `✗ Failure: ABC-1 can't be edited: unexpected error,
  …` (measured), so an edit whose output holds a `✗` line is said as not done, with that line and
  its assignee.
- **Nothing is assigned by default**: the user is assigned only when the agent asks for `"@me"`;
  the tool's description says that a tracker may assign a new work item itself.
- **The card** of a ticket write says the assignee, the sprint and the parent in words (`It
  assigns it to you` for `me`, `It assigns it to ana@example.org`, `It removes the assignee`, `It
  puts it in sprint "Sprint 42"`, `It puts it under ABC-10`); each is a write judged as before.
- **Labels on Jira**: `create` `--label a,b`; `edit` `--labels a,b` (which adds to the work item's
  labels, measured) and `--remove-labels c`. The card says them (`It adds the labels bug, p1`,
  `It removes the labels wontfix`). A label holding a comma is refused before anything is asked
  (`acli` and `gh` take labels separated by commas and would split it), and a blank one is left
  out, on GitHub as on Jira, with flags or in a JSON definition.
- **Assumed**, as the other shapes of this page: `project view --json` prints Jira's REST
  project, with `issueTypes[].name` and `.subtask`. Its page gives flags and no output, and it
  could not be run (a call to a site). The maintainer checks it with one `tickets {"action":
  "types"}` on their work machine; a different shape answers `acli did not list the work item
  types of ABC (no issueTypes)`, and the fix is in `work_item_types`.

**`acli` lists no work item's transitions** (its commands,
<https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem/>, have no listing
and no way to call Jira's REST API, and `view` has no transitions field), and Flottille provides
no login of its own to call Jira's REST API with. What `acli jira workitem transition --status`
does is ask Jira for the work item's transitions and take the one that leads to the status named,
so **that is the judge**: a status is never refused because no work item is in it. Each of the
three first reads the work item's own status, `acli jira workitem view <K> --json --fields status`
(its name and `statusCategory.key`: `new`, `indeterminate`, `done`). **One status is attempted,
never several in turn**:

- `status` hands `acli` the name asked, as it was written. A work item already in it (any case)
  answers `ABC-1 is already in To Do; nothing was written.`;
- `close` on a work item already in a `done` category answers `ABC-3 is already closed: it is in
  Done; nothing was written.`. Otherwise the status is chosen among the `done` ones seen on the
  project's work items (`acli jira workitem search --jql 'project = "<P>" ORDER BY updated DESC'
  --fields key,status --json --limit 200`): the only one seen; else, of several, the one named
  `Done` (any case); else, none seen, `Done`. Several and none named `Done` are refused, nothing
  written, saying them and asking for `status` with one of them (a project may have `Won't do`
  beside `Done`, and which one is meant is not for Flottille to try);
- `reopen` on a work item that is not in a `done` category answers `ABC-1 is already open: it is in
  To Do; nothing was written.`. Otherwise the same rule with the `new` category and `To Do`. A
  started (`indeterminate`) status is never taken for it.

**What is said is what the status read back says.** `transition` is a bulk command (it has
`--ignore-errors`), so it may end with 0 on a transition it did not take, and it may be cut or
killed on one it did. After it, the work item's status is read again, once, and compared (any case)
with the one attempted: if it is that one, the work item `was closed`, `reopened` or `moved to` it,
named as Jira spells it and not as the agent wrote it; if it is not, the answer is `ABC-1 was not
moved to Done: it is in To Do`; and if it cannot be read, `the result is unknown`, asking to look at
the work item before asking again. `nothing was written` is said only when `acli` refused (it ended
with an error of its own, with its first line) or when the status read back is the old one. When
`acli` refuses, the statuses seen on the project's work items are said as seen, which does not say
where the work item can move: `acli could not move ABC-1 from To Do to Blocked: …; nothing was
written; move it with status, naming a status; seen on ABC's work items: To Do and Done (that does
not say where ABC-1 can move)`.

This is a known limit: `Done` and `To Do` are Jira's names for its default workflow, attempted only
when no work item shows the project's, and a workflow whose `Done` is not of the `done` category
would be moved to it. The list of the statuses a work item can move to now waits for `acli` to give
it. **Assumed**, from the same pages, which give flags and no output: `--status` is matched by Jira
or `acli` without regard to case; `create --json` prints the created work item with its `key`; a
create with no `--type` takes the project's default type; a refused transition ends with a non-zero
code.

### Parents and sprints

Seen in daily use: an agent writing a Jira work item for the user could not choose its sprint nor
give it a parent, and the user did both by hand. What `acli` 1.3.39 can do decides what Flottille
does (#362):

- **A parent at `create`**: `parent`, a key of the tracker (any case, kept in upper case),
  `--parent <key>`: an epic, or the parent of a sub-task. Measured: a task created with `--parent`
  on an epic's key is under that epic. `acli jira workitem edit` has no parent, so **`edit` with
  `parent` is refused before anything is asked**: `acli's workitem edit has no parent …: changing
  the parent of an existing work item needs Jira's REST API, which Flottille does not call yet
  (#382)`. A `link` of kind `parent of` is refused the same way.
- **`sprints {project?, board?}`**, a read: the project's boards (`acli jira board search
  --project <P> --json`, Jira's agile page `{"values": [{"id", "name", "type"}]}`, as a real site
  printed it), the one named alone when `board` names one (its name or its id; `board` on any
  other action is refused), and for each its
  active and future sprints (`acli jira board list-sprints --id <N> --state active,future
  --json`), one line each: `- Sprint 42 [id 42], active, ends 2026-11-20`. A board without
  sprints is said in `acli`'s words (`✗ The board does not support sprints`, measured on a simple
  board). On GitHub the answer is that issues have no sprint.
- **A sprint at `create`**: `sprint`, an open sprint of the project's boards by its name (any
  case) or its id, **found before the user is asked**, with the site's Sprint field (below), so
  that the card names the sprint as the site does, with its board (`It puts it in sprint "Sprint
  42" (board "ABC board")`, for an agent that gave `42`). One that is not open, a name two boards
  share (asking for its id), and a site whose Sprint field is not known are refused then, nothing
  asked. Once allowed, the sprint is read again (it may have closed meanwhile). `acli` sets a
  sprint only through a custom field, and a custom field only from a JSON definition (`create
  --from-json`, `additionalAttributes`), which takes no other flag (measured: `--assignee`
  alongside is ignored) and no `@me` (measured: `User not found for email: @me`). So a creation
  in a sprint writes the whole work item as JSON, in a private file as a description is
  (`projectKey`, `summary`, `type`, `description` as ADF, `labels`, `parentIssueId`, which takes a
  key as measured, and `additionalAttributes: {"<the Sprint field>": <the sprint's id>}`), and
  sets its assignee right after with `edit --assignee`; if that edit fails, the answer says the
  work item was opened **without its assignee** (`The ticket ABC-11 was opened in ABC, in sprint
  "Sprint 42", without its assignee: …; acli did not assign it to "…": …; it exists with no
  assignee`).
- **A creation `acli` answers without the new key** (on either path) is no success: the answer
  says that what was done is not known, that it may have been opened, which of its sprint,
  parent and assignee may not be set (in a sprint, the assignee was not set: its edit needs the
  key), and to look at the project before creating it again, with `acli`'s own words. The JSON definition has no default type (measured:
  `no value given for required property type`), so **a `sprint` without `type` is refused before
  anything is asked**.
- **The Sprint field** is a custom field whose id is the site's own (`customfield_10020` on many
  sites). `acli` lists no field (`acli jira field` creates, updates, deletes and restores one), so
  it is found where it shows: `acli jira workitem search --jql 'project in (<the tracker's>) AND
  sprint is not EMPTY ORDER BY updated DESC' --fields key,summary --json --limit 1`, then `acli
  jira workitem view <K> --json --fields '*navigable'`, the field whose value is Jira's list of
  sprints (`[{"id", "name", "state", "boardId"}]`). It is kept for the daemon's life, by site.
  When no work item of the tracker's projects has been in a sprint yet, a creation in a sprint is
  refused saying so, nothing written: the user puts the first one in its sprint on the board.
- **Moving an existing work item to a sprint is refused** before anything is asked: `acli's
  workitem edit takes no sprint (it sets no custom field) …`, and `acli jira sprint` creates,
  updates, deletes, views and lists the work items of a sprint. The former `sprint` action, which
  was always refused, is gone.
- **`show`** says a Jira work item's assignee (`Assignee: Anne Example`, `nobody`), its parent
  (`Parent: ABC-10 Sign-in`) and its sprint (`Sprint: Sprint 42 (active)`: the active and future
  ones, or the last closed one), read in the same view as the rest (`--fields
  …,assignee,parent,*navigable`: `*navigable` brings the Sprint field, whatever its id; measured,
  `attachment` is not among the navigable fields and stays asked by name).
- **Not measured**: the maintainer's test site has no board with sprints (its only board is a
  simple one, and JQL answers `field 'sprint' does not exist`), so the shape of `list-sprints
  --json` (taken as the same agile page as `board search`, or a bare list), the Sprint field's
  value, and a `create --from-json` holding it are **assumed**, from Jira's REST API. The
  maintainer checks them on a site with sprints: `tickets sprints`, then a `create` with
  `sprint`.

### Milestones

Seen in use: an agent asked to group tickets into two milestones could not (#758): the tool
created and edited an issue and knew nothing of milestones, so it offered labels `v0.1` and
`v0.2`, or asked the user to make them in the browser. On GitHub a milestone is how a set of
tickets becomes a release with its progress bar. What `gh` can do decides what Flottille does:

- **`gh issue` names a milestone by its title and makes none**, so the milestones themselves are
  `gh api` calls on the tracker's repository (`repos/<owner>/<name>`, from the tracker the project
  declares, never from the agent's arguments), with `--hostname github.com` as every call is made
  against the host the declaration was checked against. `milestones {state?}` reads `…/milestones?
  state=<open|closed|all>&per_page=100 --paginate` (a page after another: the answer is read as
  the stream of lists `gh` prints, whichever form it takes), `milestone_create` is `--method POST
  -f title=… [-f description=…] [-f due_on=<day>T00:00:00Z]`, `milestone_edit` is `--method PATCH`
  of `…/milestones/<number>` with the `-f` fields it changes. Each `-f` is one argument: no text of
  an agent is spliced into anything.
- **A milestone is named by its title, else its number**, and found in the repository's list of
  every state first (a closed one takes a ticket too). The title found is what `gh issue create`,
  `edit` and `list` are given (`--milestone`), which is why a number the agent gave works where
  `gh` takes names only. A title written otherwise than the milestone's is found too, the exact one first.
- **One that does not exist is refused, listing those that do** (`there is no milestone "v9" in
  acme/app (its milestones are v0.1 (#1, open), v0.2 (#2, open)); nothing is created implicitly:
  milestone_create makes one`), **before the user is asked** where it is a write (`prepared`, as a
  Jira sprint is), and again when the write is done, the list being read again then.
- **A title is used once**: `milestone_create`, and a `milestone_edit` that renames, are refused
  naming the milestone that has the title (case ignored, closed or open: GitHub's own rule).
- **`due` is a day** (`YYYY-MM-DD`, a calendar day: `2026-02-30` is refused), sent at midnight UTC.
  `milestone_edit` needs `milestone` and one of `title`, `description`, `due`, `state`; `state` is
  `open` or `closed`. A `due` or a `description` of a ticket's action, a `remove_milestone` off an
  `edit`, a `milestone` off `list`, `search`, `create`, `edit` and `milestone_edit` is refused,
  naming the actions that take it. `milestone` with `remove_milestone` is refused: a ticket is in a
  milestone or in none.
- **The request** says the milestone as the repository names it (`It puts it in the milestone
  "v0.1", due 2026-12-01`, `It changes the milestone v0.1 (#1, open)`, `It closes it`, `It takes
  it out of its milestone`, `Its due date: 2026-12-31`), beside the title and the description as
  the other writes show them; a milestone's description is read by the gate as a body is (a secret
  in it is base).
- **`list` and `search` with `milestone`** ask the tracker (`--milestone`): the rounds keep each
  ticket's milestone (`gh issue list --json …,milestone`, the title in the item's details), so a
  `list` answered from them shows it on each line (`- #5 open: … milestone: v0.1`), but the filter
  is `gh`'s own. A `search`'s kept answer is keyed by it. `show` reads it with the issue
  (`--json …,milestone`) and says `Milestone: v0.1` when there is one.
- **On Jira**: `milestones`, `milestone_create`, `milestone_edit`, `milestone`, `remove_milestone`,
  `due` and `description` are refused, before anything is asked and nothing run, naming a sprint
  (`a work item goes in a sprint: sprints lists them, create takes sprint`). Jira's versions
  (fix versions) are a different field, out of scope here.
- **Not measured against a real repository**: the shape of the API's answers (`number`, `title`,
  `state`, `due_on`, `open_issues`, `closed_issues`, `html_url`, the REST API's documented fields)
  and that `gh issue edit --milestone` takes a title. The maintainer checks them with `tickets
  milestones`, then a `milestone_create` and an `edit` in a throwaway repository.

### What `acli jira workitem search` prints is assumed too

Its page (<https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem-search/>)
gives its flags (`--jql`, `--fields`, `--json`, `--limit`, `--paginate`) and no output. The
reading takes an array of work items in the shape `view` prints, or Jira's REST search object
(`{"issues": [...]}`), and reads `key`, `self`, `fields.summary`, `fields.status.name`,
`fields.assignee.displayName` and `fields.labels`. An item whose `self` names another site (not
the tracker's, nor an Atlassian host, as above) is left out and counted, as is one whose key is of a project the tracker does not declare; one
with no `self` is kept, since the search was of the signed-in site's projects. The maintainer checks it on their work machine
(`tickets list` from an agent of a Jira project); a different shape shows as an empty list.

**Which fields `--fields` allows is not documented**: the page above gives the default
(`issuetype,key,assignee,priority,status,summary`) and the example `key,summary,assignee`, and
nothing on the others; the page of `workitem view` takes any field name (`*all`, `-field`), which
`search` is not known to do. What is known comes from a real site: `acli jira workitem search`
answered `✗ Error: fields 'updated, created' are not allowed`, naming those two alone, so
`key`, `summary`, `status`, `assignee`, `reporter`, `labels`, `issuetype` and `priority`, asked in
the same line, passed. Those eight are what Flottille asks (`ITEM_JIRA_FIELDS` and the two other
searches, which ask fewer), and the fake `acli` refuses any other with that message. `updated`
and `created` are therefore not asked: the items read `fields.updated` and `fields.created` when
the output holds them, and a work item without them is listed without a date (the search's own
order, `ORDER BY updated DESC`, stays). `workitem view` is not run for the dates of a thousand
work items; if the maintainer finds that `acli` prints them under another flag, that is a
follow-up. A search prints only the fields asked (`key` alone prints `null` per work item), and
`workitem view K --fields updated,comment` gives `fields.updated` and `fields.comment.total`. A
round after the first searches `updated >= "-<minutes>m"` since its last, so **a work item it lists
changed**: a followed one is then read for its comments, and a followed one the window does not
list is not read at all. No search is sent for the followed items alone, and none asks `updated`. The maintainer checks the Tickets tab of a Jira project on their machine.

### Logging time

`acli` has no worklog command, so the daemon calls Jira's REST API itself, once the gate let the
call through: `POST https://<site>/rest/api/3/issue/<KEY>/worklog`, with the daemon's own HTTP
client (TLS with the system's certificates, a redirect never followed, since the credentials would
go with it, a call cut after 30 s).

```
tickets {action: "log_time", ticket: "ABC-123", duration: "2h 30m", started: "2026-11-06T09:00:00+01:00", body: "…"}
```

- **`duration`** is Jira's form: whole numbers of weeks, days, hours and minutes, each once and the
  biggest first (`1w 2d`, `2h 30m`, `45m`). It is sent as written and Jira applies the site's own
  working time; only for a start left out is it counted in Jira's default (a day is 8 hours, a
  week 5 days). **`started`** is an RFC 3339 time with its offset, sent in UTC; left out, it is the
  duration before the moment the worklog is logged (the card says so, since the user may take a
  while to allow it). **`body`**, optional, is the worklog's comment, Markdown converted to ADF as
  every Jira text is. **`ticket` is required**: the workspace's ticket is never taken for it, so that the card names the work item the user allows.
- **The token is a vault entry the project names** (`docs/vault.md`): `worklog_token = "<entry>"`
  in `[tracker]` (`kind = "jira"` only), the entry's secret being a Jira API token and its
  username the account's e-mail; the request is `Basic` over `username:secret`. The daemon reads
  the secret with `vault::secret` for this one request. No entry named, an entry the project does
  not see, or one with no username is refused before anyone is asked, with a sentence that says
  how to set one up: the window's way first (the project's Settings, Tracker, `Log time with`), the
  file's second. **The window offers that way**: the refusal's `agent.tool_served` carries
  `setup: "time_logging"` (`docs/events.md`), and the conversation shows a line and a `Set up
  time logging` button that opens the project's Settings at Tracker. The token is in no card, result, log, event nor the store: what Jira
  answers is masked of it, and so is every error.
- **A change of the tracker says its entry**: the card of `project set_settings` reads `Set the tracker to Jira on <site> (ABC), logging time with the vault entry `<name>``, so that the user sees which credentials a worklog would use. The entry is bound to the project's worklog, not to a host: when the site or the entry changes, the card says where the secret goes too (`Log time with it: the secret of the vault entry `<name>` will be sent to <site>`). Such a change, an agent's `set_settings` that sends the token to another site or names another entry for it, is held for the user in every mode, **Full included**, and never reaches the judge: answered at once, the result coming later as a delivery, so that an agent cannot send the user's token to a host it chose. A change that keeps the site and the entry passes in Full as before; the window, the command line and an edit of the project's file are the user's own.
- **GitHub is refused with a sentence** (`GitHub issues have no time tracking`), and so are a
  duration or a start that is wrong, before anyone is asked.
- **It is a ticket write**, judged like `comment`: held in Ask, allowed in Full, and the card says
  the ticket, the duration and the start (`It logs 2h 30m on ABC-1, started
  2026-11-06T09:00:00Z`). The result says the worklog's id.
- `time_spent {}` (a read, never held) says how long the agents of the caller's session actually
  worked: **the sum of their turns** (`turn_usage`, each turn once whatever the models it used),
  the agents that worked at the same time each counted, never the time between the first and the
  last message. The agent proposes it to the user, who confirms or changes it; it never logs it
  alone. A turn whose agent reported no usage is not on record and not counted.
- The tests send the call to a fake HTTP server of this machine (`FLOTTILLE_TEST_JIRA_BASE`, read
  by a debug build only), with the daemon's in-memory keyring.

**The maintainer checks it on their Jira site**: one worklog logged on a real work item, with an
API token of their own: Jira Cloud's answer to a worklog written as above is not something this was
written against.

## Tests

Tests never call GitHub or Jira nor use anyone's sign-in: a fake `gh` (`crates/flottille-fake-gh`,
which answers `issue view`, `issue list`, the issue writes, the `gh project` calls of a status, the
missing `project` scope and the milestones of a repository, `crates/flottille/tests/ticket_milestones.rs`, too) and a fake `acli`
(`crates/flottille-fake-acli`, `workitem view`, `workitem search` and the writes; it reads the
files of `--description-file` and `--body-file` and ends with 1 on a document that is not valid
ADF, so that a test catches one; without a scripted `search`, a search finds the work items its
JQL's `project in`, `text ~`, `comment ~` and `updated >= "-<n>d"` find, as a site does, and the
fake `gh` reads `in:` and `updated:>=` of an issue search), first on the
`PATH` of the test's daemon, answer from a script and record every call; the repositories are
throwaway ones whose `origin` is set to a GitHub URL after their workspace is made, and whose
forge and tracker are declared with `project.set_settings`
(`crates/flottille/tests/tickets.rs`).
