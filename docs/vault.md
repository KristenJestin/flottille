# The vault

Agents sometimes need a secret the user holds: a tracker's API token to log time, the login of a
test account, a deploy key. Pasted in a conversation, it would be stored and shown. The vault keeps
it instead, where the operating system keeps secrets. In Ask and Auto, agents **use it through
Flottille's tools without seeing it**. In Full, a command an agent runs is the user's own and can
read it (see below).

## Where the secrets are

In the keyring of the user's session: the Secret Service on Linux (GNOME Keyring, KWallet,
KeePassXC…), reached through the `oo7` crate (already part of the window, which keeps nothing in
it itself). Each secret is an item labelled `Flottille: <name>`, with the attributes
`application = flottille` and `flottille-entry = <name>`. A keyring that is locked asks the user
to unlock it, as it does for any program. The device key of remote's clients is one more item,
`Flottille: device`, told apart from an entry named `device` by its attribute
`flottille-key = device` instead of `flottille-entry` (`docs/remote.md`, "Identity and keys").

Never a file of Flottille's and never its store: the store keeps only the entries' metadata
(`vault_entries`), and nothing of Flottille's (store, events, logs, files) holds a secret.

## An entry

| Field | |
|---|---|
| `name` | Required, unique: the handle agents use. Letters, digits, `.`, `_` and `-`, starting with a letter or a digit, 64 at most. |
| `secret` | Required, 6 characters at least (a shorter one could not be masked without masking ordinary words). Write-only. |
| `description` | What it is for, as agents read it. Optional. |
| `url` | Where it is used. Optional. |
| `username` | The login that goes with it. Optional. |
| `project` | The only project whose agents see it. Left out, every project's. Optional. |
| `for` | The one use that reads it (below). Left out, any feature that names it. Optional. |

An entry is tied to its project **by name**, not by its folder: a project that is renamed leaves
its entries under the old name, seen by no agent, until they are set again with the new one; a
project removed and added again under the same name sees them again. The window keeps naming an
entry's project when no project of that name exists.

An entry's `url` binds it to a site, and only the host of that site counts: the port and the path
are left out, so `https://app.example.test:8443/api` binds the host `app.example.test`, and so does
`https://app.example.test/`. The scheme is not compared, but it decides whether there is a host at
all: `https`, or `http` on this machine, does; `http` to any other host does not. A subdomain is
another host (`api.app.example.test` is not `app.example.test`). For a connection an agent proposes,
a header takes an entry only on its host (`docs/extensions.md`, "Connections"), so two services on
the same host can both be given the same entry, whatever their ports and paths: the binding does not
tell them apart. The user's own form binds an entry to any host. `web`'s calls of an API are
stricter: for them the binding is the `url`'s **origin**, its scheme, host and port
(`https://app.example.test:8443/api` is `https://app.example.test:8443`, and `https://app.example.test/`
another origin), so that another service on another port of the same host is never given the secret.

## Bound to one use

An entry made for one precise feature is that feature's alone. `for` names it:

| `for` | The use |
|---|---|
| `worklog:<project>` | The token the project's tracker logs time with (`docs/tickets.md`, "Logging time"), corrects and deletes worklogs with ("Correcting a worklog"), and reads worklogs and fields with, edits work items with (their fields, parent and fix version, `docs/tools.md`), posts a reply under a comment, and posts or changes a text that mentions someone, and looks the mentioned accounts up, with (`docs/tickets.md`, "Replies"), and searches the site's people with, for `people` (`docs/tickets.md`, "Mentions"). |
| `connection:<name>` | What the server of a connection is given (`docs/extensions.md`, "Connections"), and the logins its adapter signs in with (agent-browser's `login`, "Adapters"). |

- **Only its use reads it.** A feature reads an entry bound to it or an unbound one it names. An
  entry bound elsewhere is refused by every other feature and every tool, in the daemon where the
  secret is read, with a sentence naming its use (`the vault entry jira-time is bound to the
  worklog of the project app and serves no other use`), never its value. A connection naming
  another use's entry is refused when it is declared, and its server does not start; the
  worklog's `Jira API access with` does not offer it, and `tickets log_time` refuses it before any card.
- **Made from a feature, bound to it from the start**: the entry set from a request's card (the
  worklog's need), from a project's Settings, Tracker, "New entry in the vault", or from a feature's
  own screen, which says `for`. An entry set from Settings, Vault (or `flottille vault set`)
  without `for` is unbound; set again without `for`, an entry keeps its use, and another use for
  a bound entry is refused, naming the one it has: it is removed first. Settings, Vault shows the
  use of each entry on its row (`For the worklog of the project app alone`).
- **Seen from a project.** A project's Settings has a Vault tab: the entries that project sees (its
  own first, then every project's, marked as such), each the row of Settings, Vault. "New entry"
  there opens the vault's own dialog over the tab with "Seen by agents of" set to the project, and
  a click on a row edits the entry in it (the dialog says when the entry is every project's); the
  tab is never left. The tab follows the vault live.
- **A worklog's entry is its project's.** Bound to `worklog:<project>`, an entry is that project's
  (its `project` is set to it when it names none; an entry of another project is refused), so no
  other project's agent lists it, and the `vault` tool never tells an agent the use of an entry.
- **A connection's entry is any project's, unless its server is given it.** A use that reads the
  entry for the calling agent's project at the moment it reads the secret lets it be one
  project's: a browser connection's `login` (`docs/extensions.md`, "Adapters"), so its secret is
  never given to another project's agent (the browser's signed-in session is shared by every
  project's agents of that connection, `docs/extensions.md`). A connection's server is started once by the daemon for every
  project's agents, and is given what its declaration names when it starts (a variable, a header,
  a query parameter, a cookie, basic authentication) and its OAuth tokens (`<name>.oauth`): it
  cannot tell whose agent a secret is for, so such an entry is every project's, and a project on
  it is refused at Save (`the server of the connection notes is given the entry notes-key when it
  starts, and serves every project's agents: …`). `vault.list` says it of each entry
  (`every_project_only`), and the window's form never offers what Save would refuse: for such an
  entry, "Seen by agents of" holds **Every project** alone, with one line under it saying why,
  before anything is typed.
- **Not removed as simply as another.** Removing a bound entry while its feature names it is
  refused, naming the feature and what stops working: its feature's own screen names another
  entry first (the entry then goes as any), or the removal is confirmed
  (`vault.remove {name, confirmed: true}`, `flottille vault remove <name> --confirmed`, the Vault
  page's confirmation, which says what stops working). When the file that would name it cannot be
  read (an `extensions.toml` that does not parse), whether it is named cannot be told: the removal
  is refused the same way, saying why, until the file is mended or the removal confirmed.
- **Unbound entries behave as they always did**, and every entry that existed before uses were
  added is unbound: no worklog token or connection of the user's stops working.

The OAuth tokens a connection signs in for (`flottille connection login`, `docs/extensions.md`,
"Signing in with OAuth") are one entry bound to it the same way, `<name>.oauth`, every project's:
its value holds the tokens, and each of them is masked alone as well as the whole. Signing in
sets it, a refused refresh and removing the connection remove it.

## The secret is write-only

It is set from the window (Settings, Vault) or with `flottille vault set <name>`, which reads it
from the standard input or a prompt that does not echo it, **never from an argument** (the shell's
history and the process list would keep it). It can be replaced (the entry is set again, its secret
typed anew), edited while it is kept (`vault.set` without a `secret`, `flottille vault edit <name>
--url …`, the window's "Edit" with its Secret field left empty: only the metadata is rewritten,
the keyring item is never written, and read only by the daemon, when the username changes, to
mask the new Basic-authentication form; a name the vault does not hold has no secret to keep, so
it is refused), or removed with its entry: the secret is deleted from the keyring first, so that
a keyring that refuses (locked) leaves the entry whole, to be removed again once it is unlocked. It
is **never shown again**: no method, no client, no tool returns it, no event or log line holds it,
and an error about a call that carried it never repeats it. The window's field is masked, and empty
again as soon as the secret is sent.

Every value of every entry is added to the masking of everything the daemon stores and shows
(`flottille_core::secrets`): an agent that came across a secret some other way and writes it in a
message, a command's output, a tool's arguments, finds it `[masked]` there, raw and in the forms
a page shows a typed secret in (percent-encoded in an address, escaped in HTML or JSON, `/` written
`\/` included, and, with the entry's username, in base64 as HTTP Basic authentication sends them,
with and without its padding, and that base64 in the same forms). The values are read
from the keyring when the daemon starts, before any agent resumes (the start waits 5 seconds at
most for a keyring that asks to be unlocked, then goes on), and a new secret is masked as soon as
it is set. A secret replaced or removed stays masked until the daemon restarts: another entry may
hold the same value.

## Choosing the entry for Jira API access

A Jira project's Settings, Repositories tab, Tracker, names the entry (`docs/projects.md`, "Forges
and tracker"): the select `Jira API access with` lists the entries the project sees by name and username,
but those bound to another use, and its link opens the vault's new-entry dialog from there, the
entry it makes bound to the project's worklog.

## What agents see

The `vault` tool, listed to an agent whose project sees at least one entry, misses one a feature
names, or has a connection whose `login` takes one (`docs/tools.md`, "The vault"): `vault {action: "list"}` gives the name, the description, the URL and the username of each
entry the agent's project sees, so that it knows what exists and what it is for. No action returns
a secret. An entry of another project is not listed, and is no entry for a tool of the agent's.

## Asking the user for an entry

An agent often finds out it needs a secret when it reaches for it: a tool refuses because an entry
is missing (the tracker's `worklog_token`, a browser `login`'s entry), or the task needs a sign-in
the vault does not hold. It may ask the user to add **any new entry**: the ask is a helper to add
an entry to the vault, not a request with rules of its own.

- **`vault {action: "ask", name, why, url?, username?, for?, project?, connection?}`.** `name` is
  the new entry's name; `url` the address the secret is for, which the daemon reads as a
  connection's `login` does (a backslash is a `/`, a domain is said in ASCII) and refuses when it
  is no `https` address, or `http` on this machine; `username` the account; `for` the use it is
  bound to, `worklog` (the agent's project's) or `connection:<name>` (a connection the user
  declared), or none; `project` `this` (the agent's project's alone, the default) or `every`, the
  entry a connection's server is given being every project's.
- **An ask for a connection's server is every project's.** `connection` names the connection the
  entry is for: one the user declared, or one the asking agent's own request waiting for the
  user asks to add (another agent's request never decides what this one's ask is for), whose declaration gives that entry to its server (a variable, a header, a query parameter,
  a cookie, basic); one it does not name is refused, a login's entry being asked with `for`
  instead and keeping the project asked. Left out, an ask for an entry such a request of its own
  names, or asked with `for: "connection:<name>"` for an entry that connection's declaration gives
  its server, is for that connection. Such an ask is every
  project's (`project: "this"` is refused), its `url`, left out, is the connection's origin
  (`http://127.0.0.1:7468` for a server on this machine), and its card says `For the connection
  <name> (every project)`. The form it opens has "Seen by agents of" on **Every project** and
  says why under it, and the URL prefilled and editable: the secret typed for that address is
  what binds it, and the request then goes through at Accept (`docs/extensions.md`,
  "Connections"). A name
  the vault already holds is refused, naming it: the agent uses it (when its project sees it) or
  asks under another name. An entry a feature of Flottille names and the project misses (today
  the tracker's `worklog_token`, `docs/tickets.md`, "Logging time") keeps the feature's prefill:
  its description and its use, the worklog.
- **Offered with the refusal.** The call that needed the entry is refused as before, and the
  refusal says the entry may be asked for, with the call to make: `tickets log_time` with its
  name, a browser connection's `login` with its name and `for: "connection:<name>"`
  (`docs/extensions.md`, "Connections"). The ask answers at once: nothing waits for the user.
- **A way in to the vault's own form.** The request waits on the user, so it is a card of the
  session's rail, **Waiting for you**, beside the requests held for the user, and never a card of
  the conversation, which records it in one quiet line with no button (`asked for the vault entry
  <name> · waiting for you`, then `… · saved` or `… · dismissed`). The card is in the session of
  the agent that asked (and an inbox entry, which names the host and whose entry it is, the
  project's or every project's, as the form will save it: a login's is the project's unless asked
  `every`); it counts in the section's number, the session's row in the sidebar shows the same
  "waits for you" mark as for a held request, and the bell rings until it is answered. Its lines:
  the entry's name, the host of its URL **on a line of its own** (where the secret
  will go: the agent chose it; the daemon's reading, never the window's), its project and the
  agent's reason; under **Details**, what it is for, its use and who asked. Its buttons are **New
  entry** and **Dismiss** (disabled while its call runs). The card leaves once the request is
  answered. New entry opens the vault's form of a new entry over the page the user is
  on (the session: it is not left), prefilled with all of it, the agent's reason under what it is for, and what it will be bound to ("For …") with the
  host its secret goes to ("Goes to …", under the URL's field, bound or not, while the URL is the
  one asked) and **Remove the binding**: the user changes any field, types the secret and saves,
  with the checks of that form (a connection's entry refused for a URL login cannot read, in the
  words of the ask's refusal, or plain `http` to another machine; the username required when the
  feature that asked reads it, the worklog's), or closes it and nothing is stored. A save the
  daemon refuses keeps every field as typed, **the secret included**, back in its masked field
  (held in the window's memory alone while the save runs, never stored, logged or shown), and the
  refusal folds away once a field changes. There is no second form.
- **One request a name.** While a request for a name is open, another ask for it joins it when it
  is the same project's or the request is every project's; another project's agent is refused
  (`ask under another name`). Entry names are shared across projects, so an agent of another
  project that asks for a name a request holds can learn that the request is open and how it
  ended (taken or not): that is inherent to shared names. Dismiss closes the request; the name
  may be asked for again.
- **Answered when the entry is saved**, from that form or any other way: each agent that asked
  and whose project sees the entry is handed a delivery, `the user set the vault entry <name>`,
  never the secret; on Dismiss each is handed `the user dismissed …`.

The secret goes from the form's field to the keyring as any secret set from the window: it never
reaches the agent, an event, a log or an error.

## Proposing an edit of an entry

An entry may be almost right: its URL points to a page with no login form, its username is wrong,
it is bound to the wrong project. The agent does not tell the user, in words, to retype it: it
proposes the edit, which the user accepts, edits or dismisses. It is the request of an asked entry,
with the same card, the same place and the same answer; only what the card says and its buttons
differ.

- **`vault {action: "propose", name, why, url?, username?, description?, project?, for?}`.**
  `name` is an entry the agent's project sees (another project's is no entry for it); each field
  given is its new value, an empty text clearing it: `url`, read as an ask reads it (refused when it
  is no `https` address, or `http` on this machine), `username`, `description`, `project` (`this`,
  the agent's project alone, or `every`) and `for` (`worklog`, or `connection:<name>` for a
  connection the user declared) for an entry bound to no use yet: an entry keeps its one use, and
  the refusal does not name it. A proposal that changes nothing is refused, and so is a second one
  for an entry while one waits.
- **Never the secret.** The schema has no field for it and an argument it does not know is
  refused; Accept rewrites the metadata only, as an edit that keeps the secret does: the keyring
  item is never written, replaced or deleted, and read only to mask a new username's Basic form.
- **Always held for the user**, in every mode, Full included: no rule applies it. The call is
  answered at once (`… is held for the user, who accepts, edits or dismisses it`), and the answer
  comes later as a delivery.
- **Its card**, in **Waiting for you** with the asked entries' (and the inbox, the bell, the
  session's "waits for you" mark, the conversation's quiet line `proposed an edit of the vault
  entry <name> · waiting for you`, then `… · accepted`, `… · saved` or `… · dismissed`): **Proposes
  a vault edit**, the entry's name, each field changed on a line of its own as before → after
  (`URL: https://id.example.invalid/realms/dev/ → https://id.example.invalid/realms/dev/account`),
  the host a new URL sends the secret to (**Goes to**, the daemon's reading), and the agent's
  reason. Its buttons: **Dismiss**, **Edit** and **Accept**, all disabled while Accept or Dismiss
  runs. Accept applies it in one action; Edit opens the entry's own form of an edit (`Edit
  <name>`, the secret left empty to keep it) prefilled with the proposal and the agent's reason,
  where the user changes what they want and saves, or closes it and nothing changes; Dismiss
  changes nothing.
- **Checked as a hand edit.** Accept is the user's own Save of the entry as it is then with the
  fields proposed, in the daemon: every check of that Save applies (a login's entry on plain
  `http` to another machine, an address login cannot read, a project the use refuses), its refusal
  is said in a toast and the request stays open, for Edit or Dismiss. An entry removed meanwhile
  is not made anew: it would have no secret.
- **Answered as an asked entry is**: the agent whose project sees the entry is handed
  `the user accepted your proposed edit of the vault entry <name>`, or, saved from its form or any
  other way, `the user saved the vault entry <name> from its form after your proposed edit`; on
  Dismiss, `the user dismissed your proposed edit of the vault entry <name>: nothing of it
  changed`. Like the card of an asked entry, it has no command of its own: the user's
  `flottille vault edit` saves the entry, which answers it.

## How a secret is used

Through Flottille's own tools: a tool that needs a secret takes the entry's **name**, and the
daemon puts the secret where it goes itself (an HTTP header, a command's standard input, a form
field), with `vault::secret`, which only a tool's own code calls and whose value never leaves that
use. It never waits for the user: an item of a locked keyring is answered at once that the keyring
is locked and the user must unlock it, and a keyring that does not answer within 3 seconds is
answered the same. Its users: `tickets log_time` (`docs/tickets.md`, "Logging time"), with `worklog_edit` and `worklog_delete` ("Correcting a
worklog"), `worklogs` and `fields` that read Jira's REST API with the same entry and `edit` with `fields`, `parent` or a milestone that
writes through it (#1067), `web`'s calls of an API (`docs/tools.md`, "The web": `entry` names the entry, the daemon signs in with
HTTP Basic when it has a username, else as a bearer token, and only on the origin of its `url`
(its scheme, host and port), `https` or `http` on this machine; an entry without a `url`, of another project or bound to
another use is refused before anything is sent, and a missing one offers `vault ask`), and
Connections (`docs/extensions.md`, "Connections"): a connection's `env` and `secrets` name entries
of every project, read when its server starts and put in its environment or on its requests,
never shown to an agent; an entry tied to one project, or bound to another use, is refused for
it. A connection an agent proposes (`docs/extensions.md`, "Connections") puts an entry in its requests
only when the entry's `url` has the connection's host: the `url` is where the secret may go, and
an entry without one goes in no request an agent chose; the user's own form binds freely, and the
card of the request says where each entry goes. An agent's connection may name an entry that does
not exist yet, for the user to save (never one that looks like a secret): the request waits, its
Accept refused while the entry is missing, and once saved the entry's host is checked as any
other's. An entry that is one project's, or bound to no address or another host, leaves the
request waiting the same way, never refused, unless it is another project's than the asking
agent's: that one is refused at the call, as no project's entry is offered to another's agent's
connection. The user widens it from the request's dialog, the
scope with **Share with every project**, the address with **Bind it to <host>**, each behind the
window's confirmation, which says that the change stays even if the request is then refused,
and the agent can do neither. `vault::secret` and the connections' reading take the use asking, and refuse an entry bound to
another.

## Shared with another machine

The one way a secret leaves its machine (`docs/remote.md`, "Vault entries"): the user shares an
entry, one at a time, with another of their machines that runs agents, from Settings, Vault, the
row's **Share with…** (a dialog that lists the runners online), or `flottille vault share <name>
--machine <machine>`.

- **Only the user's device asks it.** `vault.share {name, machine}` is a request the client signs
  with the device key (`docs/remote.md`, "Requests from another machine's clients"), for its own
  machine: the daemon, which never holds that key, serves it only so signed, and refuses a client
  that does not sign (`sharing a vault entry is signed by the user's device: …`).
- **The signature goes with the secret.** The holding daemon reads the secret from its keyring
  and sends it, with the entry's metadata and the user's signed request as the client wrote it,
  in a `share` message over the link, encrypted end to end between the two machines. The other
  machine checks that request itself as it checks any user's request: signed by a member's
  device, for the machine that sent it, within five minutes, never seen before, of a role that may
  drive, `vault.share` of that very entry with this machine. The machine that sent it must be a
  runner's or a controller's, as for code (`docs/remote.md`, "What travels"): a viewer's machine
  shares nothing, whoever signed (`refused: phone is a viewer's machine: only a runner's or a
  controller's machine shares a vault entry`).
- **What the signature covers, and what it does not.** The device signs the entry's name and the
  machine it goes to, nothing more: not the secret, nor the metadata, which are the holding
  machine's word. So a machine's message alone shares nothing the user did not: a machine key read
  by an agent's code can carry only an entry the user just shared, by that name, to that machine,
  within the request's five minutes, and once; it cannot share another entry, send it elsewhere,
  or play the share again. The receiving machine therefore takes nothing else on trust: it checks
  the metadata as Save checks it, and never replaces an entry of its own (below).
- **One write of an entry at a time.** A Save, a removal and a share of the same name are served
  one after the other: a share that arrives while the user saves that entry waits, then finds it,
  and is refused; it never overwrites it.
- **Kept as any secret there.** The other machine checks the metadata first as its own Save
  would: the project is one of its own, the use (`for`) is one it knows and fits the entry, a
  `connection:` entry's `url` is no plain `http` to another machine. A `worklog:<project>` entry
  is refused until that project is there (`the vault entry jira is bound to the worklog of the
  project app, which is not on this machine: add the project here first`), as is an entry of a
  project it lacks. Then it masks the secret, writes it into its own keyring under the entry's
  name, and the metadata into its store, marked as shared from the machine it came from
  (`shared_from`, the row's `Shared from pc`); an agent there whose request for that entry was
  open is told, as when the user sets it. A store that refuses the write takes the secret back
  out of the keyring: nothing is left half kept. It is a copy, not kept in sync: sharing again from
  the same machine replaces it, and a secret set there by hand makes it that machine's own. It
  never replaces an entry that machine set up itself, nor a copy from a third machine: that is
  refused (`this machine has its own vault entry tracker: remove or rename it here first`).
- **A machine with no keyring refuses**, saying so (`this machine has no keyring: no Secret Service
  runs in its session …`): the vault keeps no secret in a file, there either.
- **Nowhere else.** Neither side writes the secret to a file, the store, an event, a log or an
  error; the logs say what was shared, with which machine, and by whom, never the value. The hub
  never sees it.

## Made from a file a setup copies

A launch on another machine whose project's setup copies a file that machine lacks (an ignored
`.env`) asks the user to share it (`docs/remote.md`, "Which project"): the file becomes a vault
entry first, on the machine that has it, with `vault.from_file {project, path, name?, replace?}`.
The daemon reads the file only then, inside the project's folder, opened once and never through a
link (what is checked is the file that is read), a text
of 64 KiB at most; its text is the entry's secret, masked whole and line by line before it is
kept, and nothing that is said of it, an answer, an error, a log or an event, holds a word of it.
Each line is masked, a file of one line without a final newline too (any secret bound to a
`file:` is, set by hand or received as well), and each line's value: what follows its first `=` or `:` (`KEY=v`, `key: v`,
`"key": "v",`), its quotes and final comma left out, and the password of an address's
`user:password@`, each of 6 characters or more, comments left. **A secret transformed is not
recognised**: a value an agent decodes, splits, re-encodes (other than the address, HTML and JSON
forms every vault value is masked in) or computes from the file is not masked.

The entry is named `<project>-<path>` (`app-.env`), every project's, described as the file of that
project its setup copies, and **bound to that file** (`"for": "file:<project>/<path>"`), which
serves no tool and no other use; it is then shared as any other, its binding kept. The other
machine makes the file from an entry only when it is bound to that very file and was shared from
the launch's machine: an entry of its own, or another machine's, whatever its name, is never
written into a file. **A machine renamed leaves the copies it shared under its old name matching
nothing**: a launch from the new name finds no entry shared from it, so the file is asked for again
(fail closed); share the entry again from the renamed machine. A name already taken is refused: by another entry (`a/b` and `a-b` both name
`app-a-b`: the second file is refused, naming the first), and by the same file's entry unless the
call says `replace: true`.

## The runtime folder

The keyring answers on the session bus, whose socket is in the user's runtime folder
(`/run/user/<uid>/bus`), and any program can speak D-Bus: refusing `secret-tool` or `busctl` by
name keeps nothing out. So what the daemon runs for an agent or a connection reaches no session
bus: a run, a project's service and its readiness probe, a workspace's setup line, a commit's or
a push's message checks and the git reads made for them, a connection's server and its login. A
push the gate let through, and its checks' dry run, are the exception (below):

- its environment has no `DBUS_SESSION_BUS_ADDRESS`, and no `XDG_RUNTIME_DIR` either, so libdbus,
  sd-bus and GDBus find no bus by default. `DBUS_SYSTEM_BUS_ADDRESS` is kept: the system bus holds
  none of the user's secrets, and its socket is at a fixed path anyway;
- the socket's path is guessable, so on Linux with Landlock (its second ABI or later, Linux 5.19)
  the process is denied the user's runtime folders, `/run/user/<uid>` and the daemon's
  `XDG_RUNTIME_DIR`: before it runs its program, it is bound to rules that grant every right on
  everything else (each entry of `/` and of each folder on the way to a runtime folder, symbolic
  links aside, which lead to where their target is ruled). It gets `no_new_privs` with it, so a
  setuid program it starts gains nothing. A connection's server that shows its browser, and its
  login (and its reload of the page), are also granted their display folder,
  `$XDG_RUNTIME_DIR/flottille/display/<connection>`, which holds a hard link of the Wayland socket
  alone (`docs/extensions.md`, "Adapters"), without the right to link or move a file out of it.
  An agent-browser connection's server, its login and its reload are granted its vault folder,
  `$XDG_RUNTIME_DIR/flottille/vault/<connection>`, the same way (and `agent-browser close` too):
  agent-browser's configuration and its daemon's socket, the login's one-time token and the
  credential plugin's socket, which no run reaches. The folders are the ones the daemon made,
  never one a variable of the connection names;
- Landlock's rules do not govern connecting to a socket by its path before its ninth ABI, so the
  process also starts in a **private mount namespace** of its own, where the kernel lets a user
  make one (an unprivileged user namespace): the user's runtime folders are empty folders there
  (a `tmpfs`), so the bus, the user's systemd, the portal, the sound server and an SSH agent
  that lives there are not found by their path at all. Only what the daemon itself gives the
  process is put back in them, at the same path: the display folder of a run or a connection's
  browser shown on the screen, and the SSH agent's socket lent to the daemon's own fetch (below).
  The daemon's own socket is not among them. X11's socket folder (`/tmp/.X11-unix`) is emptied
  the same way, unless the process is given X11's display. A session bus the daemon's
  `DBUS_SESSION_BUS_ADDRESS` names by a path outside the runtime folders (`unix:path=/tmp/dbus-…`,
  as `dbus-launch` makes one), found at the daemon's start, has its path hidden in every
  confined process, whatever is there now: a socket or a file under `/dev/null`, a folder under
  an empty read-only one, and nothing at all where nothing is there (nothing is reached by that
  path then). The daemon also keeps what tells the socket found from another (its device, inode
  and change time: an inode freed is often given to the next file made); once the socket at the
  path is no longer that one, gone or replaced, no push the gate lets through is given the bus's
  address (below), and `flottille doctor` says so in its warnings. The mounts are made by the process before it runs its
  program, which runs as the user, not as root of the namespace, with `no_new_privs` set first
  (a program with file capabilities gains none in the namespace): it holds no capability to
  unmount or remount them, and a namespace it makes inside gets them locked. Landlock's rules
  apply after it, unchanged. Where the kernel refuses (a kernel built without user namespaces, a
  distribution that forbids them to unprivileged users), the process starts without it, and the
  daemon's log and `flottille doctor` (its `sandbox` part, with Landlock's ABI, as the daemon
  has it) say so. The daemon probes it once, by a child that makes one: a probe that concludes
  nothing (no process could be made, the user's namespaces are all taken) is not kept, and the
  process still starts in its namespace, or fails to start.

**A run in Full is the exception, and nothing is held back from it** (`docs/tools.md`, "In
Full, a run is the user's own"): it has the daemon's environment, no namespace and no Landlock,
as a command in the user's terminal. **In Full, the vault and the device key are reachable by
what the agent runs**: the vault's items over the session bus, in the keyring, with the user's
rights; Flottille's state folder, the connections' vault sockets and the key that signs this
machine's requests to the others (`docs/remote.md`) by their path. The vault's promise holds in
every mode for Flottille itself (no method, tool, event or log returns a secret, and the gate
still refuses `secret-tool` and the other keyring programs named on a line), not for code run in
Full: Flottille says so where Full is chosen and to the agent. A Full that keeps the vault out (a
filtered bus, the vault encrypted at rest) is not built.

**The SSH agent stays out of every run but one in Full.** What the daemon runs gets no `SSH_AUTH_SOCK`. The socket
the daemon's own environment named when it started (resolved then, links followed) is also kept out
by its path: inside the runtime folder it is denied with it; outside it (`/tmp/ssh-…/agent.…`, a
password manager's under the home), from Landlock's ninth ABI, the folders on the way to it are
ruled without the right to connect, their other entries with every right, and the socket itself
gets no rule.

The agent command lines (Claude Code, Codex, OpenCode, the judge's, the one that lists models)
start without `SSH_AUTH_SOCK` either: bare mode gives them no shell, and the variable would still
tell them where the agent is, and lend it to a tool that spawns.

**The daemon's own git is walled like a run's.** Every `git` the daemon runs for itself on a
repository (its status and log reads, its worktrees, the `ls-remote` and `fetch` that find a
workspace's base) is the one found at its start, as its `ssh` is (below): the first of its `PATH`
that only root may change (it and every folder on the way to it are root's, and neither their
group nor others may write them), else the first one, its links followed then, never one a
run put first in the `PATH` since. It is bound by the same Landlock rules, gets no `SSH_AUTH_SOCK`, and is given what a
repository's settings could otherwise turn into a program of theirs: `core.hooksPath=/dev/null`,
`core.fsmonitor=false`, `fetch.recurseSubmodules=false` and `submodule.recurse=false` on its line
(the command's scope, which wins over the repository's settings and what they include: git cannot
be told to skip an `include.path`, so what it brings is overridden instead), and
`GIT_SSH_COMMAND` naming the `ssh` found at its start, which wins over `core.sshCommand` and
`GIT_SSH`: the first `ssh` of the daemon's `PATH` that only root may change (as above), else the
first one (said once in the daemon's log), and in either case the program its links lead to
then, so that a link pointed elsewhere later changes nothing, given `-o ControlMaster=no -o
ControlPath=none -o ForwardAgent=no` first (the first value `ssh` reads wins: no master socket,
which the read-only `~/.ssh` could not hold and which any run could use elsewhere, and the agent
forwarded to no host). On the same line,
`credential.helper=` empties the list of credential helpers the repository's settings built,
and the user's own (the system's and the user's settings, read at the daemon's start, their
names alone) follow it: git hands a sign-in to `store` on every helper it knows, and a
repository's own would get it. Its environment sets `GIT_NO_LAZY_FETCH=1` (a partial clone
asks its promisor remote for no missing object while the daemon reads it), an empty
`GIT_ALLOW_PROTOCOL` (no remote reached at all), and an empty `GIT_PROXY_COMMAND` and
`GIT_ASKPASS`, which win over the `core.gitProxy` and `core.askPass` a repository names. A call
that reaches a remote also gets:

- `GIT_ALLOW_PROTOCOL` set to the one protocol the remote's URL uses, as git resolves it
  (`ls-remote --get-url`, so after `url.*.insteadOf`), and only if it is one of git's own
  transports: `ssh`, `https`, `http`, `git` or `file`. A URL through a helper (`ext::<command>`,
  `<name>::` or `<name>://` for a `git-remote-<name>` of the `PATH`) runs a program it names:
  it is not fetched, and the workspace starts from the base as last fetched, saying why;
- `--upload-pack=git-upload-pack` on its line: git takes the first `remote.<name>.uploadpack` it
  reads, the repository's, so only the line's own option wins over it. The daemon never pushes,
  so no `receivepack` is ever read;
- `maintenance.auto=false` and `gc.auto=0`: no maintenance after the fetch, which could go on in
  the background with the lent agent;
- when that protocol is SSH, the user's SSH agent, lent as to nothing else, to the `git` a push
  runs (below) and to no other: `SSH_AUTH_SOCK` names
  the socket found at the start, and from Landlock's ninth ABI a rule on that socket alone grants
  connecting to it, the rest of the runtime folder (the bus) staying denied. An HTTPS remote gets
  no agent and no session bus: its credential helper runs confined (below, "The gaps").

**A push the gate let through runs outside the sandbox.** A run that is itself a `git push` (its own
line, no wrapper) the user, the judge or a rule let through in Ask or Auto (in Full, a push is a
plain run of the user's own, above), and the dry run the
project's commit checks make of it, run with no Landlock and the daemon's own environment, as if
the user had typed the push: the session bus (so the keyring), the SSH agent, the credential
helper and the forge command line's sign-in are theirs. A push that goes to paths alone (a remote
that is a folder of this machine) needs none of them, and runs in the sandbox instead, with the
run's own environment but the rewrite of its push URLs to nowhere, `file` its one transport:
the git that receives it runs the hooks and settings of a repository a run may have made, as a
run's own push would. Git says which it is: the push's dry run, in the sandbox and with `file`
alone allowed, refuses no transport. A push to a path and to a remote at once goes outside, and
fails on the path. Outside the sandbox, only these are added:

- the `git` of the daemon's `PATH` that only root may change (above), found at the daemon's
  start, its links followed, never one a run put first in the `PATH` since; without one, the
  push is refused before anything of it is read or judged, saying why (the daemon's log says it
  at its start, and `flottille doctor` in its warnings);
- no push at all where Landlock (its second ABI or later) is not enforced: the folder below is
  in the daemon's runtime folder, which only Landlock keeps a confined command from (the private
  mount namespace's empty folder is reached through `/proc/<pid>/root` without it, "The gaps"),
  and a run that could reach it would add a proxy to its settings between the push's checks and
  the push. The push is refused, saying so: push from a terminal of your own (`flottille doctor`
  and the project's settings say it too, of every push and not of SSH's alone);
- of git's global options on the push's line, only `-C` and those that name no program, setting
  or git folder (`--no-pager`, `-P`, `-p`, `--paginate`, `--no-replace-objects`, the pathspec
  ones, `--no-optional-locks`, `--no-advice`, `--no-lazy-fetch`): a line with another is
  refused, saying which. `--exec-path` would take `git-remote-https` from a folder of a run's,
  `-c` and `--config-env` give a setting that wins over the daemon's own (a line's `-c
  http.proxy=…` over everything below), `--git-dir` and `--work-tree` another git folder;
- **none of the repository's settings but which remote it goes to.** A run can write the
  repository's settings, and git has no switch that leaves them unread; a setting given on the
  push's own line does not win over a more specific one of the repository's either
  (`http.<url>.sslVerify` over `http.sslVerify`, `http.<url>.proxy` over `http.proxy`), so
  overriding the dangerous keys one by one cannot hold. The push runs from a git folder the
  daemon makes for it instead, in its own runtime folder (out of every confined command's reach,
  above), and removes once the push ended: `GIT_DIR` names it, so git reads no repository's
  settings but its file, which holds the repository's format alone (`core.repositoryformatversion`
  and its `extensions.*`, each a plain word or refused; refs kept in a `reftable` are refused).
  Its `HEAD` and its refs are the repository's as they are then, written in it (each ref in its
  `packed-refs`, a symbolic one as the ref it leads to), and its `shallow` a copy of the
  repository's, with `GIT_OBJECT_DIRECTORY` its objects: a run that moves a branch between the
  push's checks and the push does not change what the push sends, which is what the checks read.
  The remote-tracking refs the push moved there are moved in the repository once it ended, each
  from the object it had when the folder was made (one a run moved since is left as it is, and
  the daemon's log says so). The repository's `info/grafts` and replacements are not read by
  what the checks read of the push (below). Its settings are then, in git's order: the system's
  (root's); the user's own (`GIT_CONFIG_GLOBAL=/dev/null`, and in their place the files found at
  the daemon's start as they apply to the repository, an `includeIf "gitdir:…"` or
  `"onbranch:…"` included, read with the repository's folder but never its settings, given on
  the command's scope); then, of the repository's own settings and its worktree's (read in the
  sandbox, with what they include), only `remote.<name>.url`, `pushurl` and `fetch`,
  `branch.<name>.remote`, `pushRemote` and `merge`, `remote.pushDefault` and
  `push.autoSetupRemote`; then the daemon's own below, which win over them all. Each of those
  kept is a remote's name, a URL or the refs that track it, never how the remote is reached
  (`http.*`, `url.*.insteadOf` and `pushInsteadOf`, `remote.<name>.proxy`, `remote.<name>.vcs`,
  `core.*`, `include.*` are not given), nor which refs a push that names none sends: a
  repository whose settings hold a `remote.<name>.push`, a `remote.<name>.mirror` that is on or
  a `push.default` is refused, saying which and what to do (unset it, or set it in your own
  settings, and name the refs on the line: `git push origin HEAD:refs/heads/<branch>`). A run
  that wrote `remote.origin.push=:refs/heads/main`, `+refs/heads/*:refs/heads/*` or `mirror`
  would make a plain `git push` delete, force or prune the remote's branches, and
  `push.default=upstream` with a `branch.<name>.merge` of its own send the branch to another;
  the gate refuses all of these on the line, never in the settings it does not read. Where the
  push's own settings (the system's and the user's, as git reads them for it) say
  `push.default=upstream` (or `tracking`), a `branch.<name>.merge` of the repository's that names
  another branch than `refs/heads/<name>` is not given either: a plain `git push` of that branch
  then has no upstream, and git says so; name the branch on the line. Where `push.default`
  cannot be read, the push is refused. A URL a
  run wrote sends the push to that host, with only the sign-in the user's own helper gives that
  host, over git's own transports (below), as the push's line could. The user's own `http.*` and
  `url.*` settings still apply: they are theirs. An `includeIf "hasconfig:remote.*.url:…"` of
  theirs (the settings of one forge) is given as it is, its file named by its full path, and
  git includes it where the push's remotes' URLs match; an `includeIf "gitdir:…"` inside that
  file is read against the daemon's folder, so it does not apply.
  **What the repository's own settings said of how it is reached does not apply** to such a
  push: an `http.sslCAInfo` for a company's certificate, a `credential.username`,
  `credential.useHttpPath`, a `pushInsteadOf` the user wrote in that repository. Set them in
  your own settings instead, for that repository's folder if they are its alone (`[includeIf
  "gitdir:~/work/app/"]` and a file of yours it includes), or for its remote (`[includeIf
  "hasconfig:remote.*.url:https://forge.example/**"]`). An upstream the push sets (`-u`, `push.autoSetupRemote`), which
  git writes in the folder's settings, is written back to the repository's once it ended. A
  push line that names another git folder (`--git-dir`, `--work-tree`) is refused, and one that
  moves (`-C`) is made from the repository it moves to;
- `PATH` made of the folders of the daemon's `PATH` that only root may change (each of them and
  every folder on the way is root's, and no one else may write it): a program the user's
  settings name by a bare name (a credential helper's `!gh auth git-credential`, `ssh` inside a
  wrapper) is root's, never one a run put first in a folder it can write (`~/.local/bin`,
  `~/bin`). It is one folder the daemon makes for each push in the push's folder (below),
  holding a link to each program of those folders that only root may change too, under its name
  and to its name (`python3`, not the `python3.12` it leads to, so that an upgrade is followed):
  a name whose first program there is not (a file of the user's in `/usr/local/bin`, a link to
  one, a link that leads nowhere) has none, and the daemon's log says which. One the system's
  folders do not hold is not found, and the helper fails: name it by its path, in a folder only
  root may change. Where that folder cannot be made, the `PATH` is a path to no folder, and no
  program is found by its bare name;
- `core.hooksPath=/dev/null`;
- `credential.helper=` and then the user's own credential helpers (the system's and the user's
  settings, read at the daemon's start), as for the daemon's own git (above): a helper a run wrote
  in the repository's settings is not asked, nor handed the sign-in to `store`. Those the user
  gives one forge alone, through an `includeIf "hasconfig:remote.*.url:…"`, are read at the start
  too, from the file it includes, and written in a file of the push's folder that is included
  after them only where the daemon finds every URL the push may go to matches the pattern, as
  git matches it (`*` within a folder, `**` across them; a pattern with a class `[…]` or an
  escape matches nothing). Those URLs are the push URLs (as `git remote -v` says them) of
  every remote the push knows, whichever its line or settings choose; a push whose line names a
  URL or `--repo`, or whose `branch.<name>.remote`, `branch.<name>.pushRemote` or
  `remote.pushDefault` names a remote git does not know (a URL), gets none. Git's own
  `hasconfig` would include them where any remote's URL matches, a remote or a `pushurl` a run
  wrote among them, and ask them for a sign-in to wherever the push goes: a helper that gives
  its token to any host would send one forge's sign-in to a run's host, or another account's
  of the same forge;
- `push.gpgSign=false`, and the signing programs (`gpg.program`, `gpg.openpgp.program`,
  `gpg.x509.program`, `gpg.ssh.program`, `gpg.ssh.defaultKeyCommand`) set to none: a push is
  not signed, and a line's `--signed` fails;
- `--no-recurse-submodules` last among the push's options: the push of a submodule runs with the
  submodule's own settings and receiving program, so none is made, whatever the line or the
  settings say;
- `GIT_SSH_COMMAND`, so that a `core.sshCommand` a run wrote in the repository is not what signs
  in: the user's own when they have one (the daemon's `GIT_SSH_COMMAND`, else the
  `core.sshCommand` of their own git settings as they apply to the repository pushed, an
  `includeIf "gitdir:…"` or `"onbranch:…"` for several accounts of one forge included, read from
  the files of those settings found at the daemon's start and never from the repository's), else
  the `ssh` found at the daemon's start, as for its own fetch (above). A command of the user's
  that names `ssh` alone runs that one; one that names another program runs it only when it is
  in `~/.ssh` or only root may change it and its folders, and one with `-F` (another
  configuration, but `-F /dev/null`), `-I` (a library), `-A` (the agent forwarded), `-M` or
  `-S` (a shared connection), an `-o` that runs a program, shares a connection (`ControlMaster`,
  `ControlPath`, `ControlPersist`), forwards the agent or includes a file, or anything the shell
  would read as more than words lends the agent to no push (below). Short options are read as
  `ssh` reads them, grouped too (`-vF<file>`). Whichever runs is given `-o ControlMaster=no -o
  ControlPath=none -o ForwardAgent=no` first, as the daemon's own `ssh` is (above);
- `GIT_ALLOW_PROTOCOL` set to git's own network transports (`ssh:https:http:git`), which wins
  over the `protocol.*.allow` settings: a URL a run set through a helper (`ext::<command>`, a
  `git-remote-<name>`) is not run, and a path is not reached (above). The push's remote is git's
  to resolve (the line's options, the branch's `pushRemote`, `remote.pushDefault`), and its URL a
  setting a run can write, so the transports allowed are git's own four rather than the one
  protocol of a remote. A push through a remote helper of the user's own is refused too;
- what the repository's settings may name as a program of its transport is not run: an empty
  `GIT_PROXY_COMMAND` and `GIT_ASKPASS`, which win over `core.gitProxy` (a `git://` remote) and
  `core.askPass` (an HTTPS sign-in with no helper), `--receive-pack=git-receive-pack` on the
  push's line, right after `push` (git takes the first `remote.<name>.receivepack` it reads, the
  repository's, and runs it on this machine for a remote reached by its path; the gate refuses
  the option on the agent's own line). The user's own `core.gitProxy` and `core.askPass` are not
  used by such a push either;
- no `DBUS_SESSION_BUS_ADDRESS` once the bus the daemon found at its start by a path outside the
  runtime folders is gone or replaced (above): what is at that path now may be a run's. The
  daemon's own use of the keyring (the vault) is unchanged: it still reaches the bus its
  environment names.

The project's and the workspace's variables are not applied (`docs/commands.md`, "The
environment"). What the project's message checks read of such a push runs where the push does,
outside the sandbox, from its folder and with its environment: its dry run, the URL the remote is
read at and what the remote has (`ls-remote`), and the commits it sends (`rev-list`, `cat-file`),
with `GIT_NO_REPLACE_OBJECTS` and `GIT_NO_LAZY_FETCH`. A push whose dry run forces a ref (`+`) or
deletes one (`-`) its line does not ask to (`--force`, `--force-with-lease`, `--delete`,
`--prune`, `--mirror`, a refspec `+…`, `:…` or `+:…`) is refused, saying which: settings chose
it, and the gate judged the line alone. That check reads the dry run of the message checks, so
it runs only for a project that declares message checks or a branch format (`docs/projects.md`):
elsewhere a plain `git push` forces or deletes what the user's own `remote.<name>.push` says
(the repository's is refused, above). So none of the repository's settings
apply to them either (a proxy a run set never sees the sign-in they make, and a `url.*.insteadOf`
of the repository's no longer refuses the push), nor its `info/grafts` or a replacement a run
made, which would hide a commit from the checks while the push sends it. The other git reads
made for a push (whether it would push submodules), and the probes of a push's task (its `ready`
and `url` commands), run in the sandbox.

**What such a push reads is read-only to every confined command.** The push runs the user's `ssh`,
with their agent, outside the sandbox: whatever a run could write into what `ssh` reads
(`ProxyCommand`, `Match exec`, an `Include`d file) would run there with the user's keys. So in the
private mount namespace of every command Flottille confines (above), `~/.ssh` is read-only whole,
with what is below it, and so are the files of the user's own git settings found at the daemon's
start (`GIT_CONFIG_GLOBAL` when set, else `$XDG_CONFIG_HOME/git/config` and `~/.gitconfig`) and
those they include (`include.path`, `includeIf.*.path`, whatever their condition): those git
reads at every push, and the daemon reads its user's `core.sshCommand` from. Where one of those
files is reached through a link (a dotfiles manager's), the folder that holds each link on the way
is read-only too when it is a folder of its own below the home (`~/.config/git`), and so is the
folder that holds the versions: the one above the folder of the file the link leads to
(`~/dots/versions` for `~/dots/versions/a/git-config`), the nearest that holds the versions a
dotfiles manager makes later beside it, and the one above that (`~/dots`) when it can be kept, the
manager's own folder, where it may stage a version before moving it in among the others; never
one higher: no run re-points the link, writes a version made after the start, or makes (and holds
open) the file a manager then moves in as its own. A kept folder is read-only with everything below it, the folders
kept in place below it included. Not kept, a link there being refused when its settings are
relinked (below), nothing kept above the folder of the versions when it is the one above it (that
folder is then the manager's own), and nothing kept at all when it is the folder of the versions
(a relink is then refused saying so): a folder a project of Flottille's or its workspaces are in or that holds them
(their folders as the projects' files and the store say at the daemon's start, whatever their
name, and the default folder of the workspaces), the home itself, the base folders (`$XDG_CONFIG_HOME`, or `~/.config` when it is unset,
`~/.local`, `~/.local/share`, `~/.local/state`, `~/.cache`, resolved the same way), the folders
a user keeps projects in by their usual names (`projects`, `workspaces`, `src`, `code`, `dev`,
`repos`, `git`, `work`, `Documents`, `Downloads`, `Desktop` of the home), and Flottille's own: they hold every other
program's files, which a run would lose. Each folder on the
way to them that a command could rename or remove (its own folder is the user's, or one they may
write) is bound onto itself there: a mount point is neither moved nor removed, so no folder on
the way is renamed to put another file at the path. `~/.ssh` is the account's (the home of the
user database, which `ssh` reads `~` as, whatever `HOME` says); the daemon makes it at its start
(`0700`, as `ssh` would) when it is missing, since a folder that is not there could not be kept
read-only. A command started at any time has them read-only from its start, and nothing it runs
can unmount them (above). Each path is opened once, following no link on its way (one put there
since the start is not followed, and the path is then not protected: the push judges it), and
made read-only through that one descriptor, with every mount below it. Where Landlock is there and
the system refuses one of the calls that does it (a seccomp filter written for an older kernel),
every command Flottille confines is refused its start, saying so (and `flottille doctor` says it),
rather than started with a mount below a read-only one left writable.

What that cannot hold is not protected file by file: the user's SSH agent is then **lent to no
push**, no `SSH_AUTH_SOCK` and no `ssh` among the transports allowed (a push over SSH fails, saying
why on its result), and, when it is what `ssh` itself reads, the daemon's own fetch is not lent
the agent either. It is judged at each push, from the files as they are then, and `flottille
doctor` says it in its warnings with how to fix it (`sandbox.ssh_lent` false in `--json`):

- `~/.ssh` is not a folder of its own (a link), or no longer the one found at the daemon's start;
- a link in `~/.ssh` that leads out of it (`ssh` follows it to a file a run can write: put the file
  itself in `~/.ssh`), a file with a hard link elsewhere, or more than 10,000 entries. A link to a
  file of `~/.ssh`, with no other link on the way, is kept read-only with it; a link to a socket
  (the agent's, where a terminal multiplexer finds it) is one `ssh` never reads, unless it bears a
  name `ssh` reads: `config`, `id_*`, `known_hosts*`, an `Include`d file, or one an
  `IdentityFile`, `CertificateFile`, `UserKnownHostsFile`, `GlobalKnownHostsFile` or
  `RevokedHostKeys` names (any, when one of these names a token or a variable);
- an `Include` of `~/.ssh/config`, or of a file it includes, outside `~/.ssh` (`~` read as the
  account's home, a relative path in `~/.ssh`, no `..`), or that names a variable, a token
  (`%d`) or a class of characters, which Flottille does not read: move the file into `~/.ssh`;
- a program the configuration runs or loads (`ProxyCommand`, `LocalCommand`, `KnownHostsCommand`,
  `Match exec`, `PKCS11Provider`, `SecurityKeyProvider`, `XAuthLocation`; `none` is none), which
  may itself be, or be found through the `PATH` in, a file a run writes. `ProxyJump` is not one.
  A line is split as `ssh` splits it, its quotes undone (`"ProxyCommand"` is the keyword);
- the git settings include a file that was not there at the daemon's start (a missing include is
  nothing, for git and for this judgement, until it is made), or one with a hard link; a file of
  them is no longer the one found at the
  start (git replaces the file it edits: `git config --global` is followed by a restart of the
  daemon);
- another mount of the same files (a bind mount of the home elsewhere, the top of a Btrfs volume
  mounted beside its subvolumes), or a mount below them, which stays writable;
- the kernel gives no user namespace to the commands Flottille confines (above), or no Landlock
  (its second ABI or later): without it, `/proc/<pid>/root` of a process outside the namespace
  leads to the writable `~/.ssh` (below, "The gaps"), and no push the gate let through is run at
  all (above), which is what `flottille doctor` and the project's settings say then;
- the user's own `ssh` command runs a program a run could change, or reads another configuration
  (above).

The daemon's own runtime folder is denied with the rest: a `flottille daemon stop` run for an
agent cannot read the daemon's lock, and refuses itself when the daemon does not answer, as a
client started by an agent does.

X11's display is kept out of the same processes as far as the platform allows, since an X11
client can read the keys typed in other windows and type into them: a run, and a connection's
server, has no `DISPLAY` and no `XAUTHORITY`, and Landlock keeps X11's abstract sockets
(`@/tmp/.X11-unix/X0`) out of reach by its scope `LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET` (from its
sixth ABI, Linux 6.12). Its sockets by their path, under `/tmp/.X11-unix`, are not denied: a rule
on `/tmp` takes from a process what is made there after it started (a file made there could no
longer be moved or linked into its worktree, a socket made there by another process could not be
reached), so the private mount namespace empties the folder instead (above).
Only a run of a project that lets its runs open windows, and a connection's browser that is
shown, in a session without Wayland, are given X11 (`docs/tools.md`, `docs/extensions.md`): their
display folder says so, and their process is not bound to the scope. The scope keeps out every
abstract socket made outside the process's own Landlock domain, not X11's alone: a session bus or
another service of the user's that listens on one (`@/tmp/dbus-…`, a container runtime's shim),
as much as X11's. The abstract sockets the process, or one it starts, makes are its own and stay
reachable.

The gaps, where the platform does not allow more:

- **what a run in Full wrote outlives it**: `~/.ssh`, the user's git settings and every file of
  theirs are its to write, as the user's own, and a later push in Ask or Auto reads them. What
  such a push runs from them is judged at each push, never trusted by where it is: a program
  `ssh`'s configuration runs is refused; the credential helpers are those found at the daemon's
  start; the hooks (`core.hooksPath=/dev/null`), the transports (`GIT_ALLOW_PROTOCOL` wins over a
  `protocol.*.allow`), the `ssh` command and the programs a transport names are the daemon's own;
  and a program of `~/.ssh` the user's `core.sshCommand` names runs only when it was neither made
  nor changed (its change time) since the daemon's start, else the push says so and a restart of
  the daemon trusts it again. What is not judged is data: a key, a `known_hosts`, a setting that
  runs nothing (a push's remote URL is the user's to change, "Where the remote leads");
- **the change-time rule on `core.sshCommand`'s program is best effort**, not a boundary: it
  races with a run in Full at the same time (a change made, then the push, within the clock's
  grain, or after the push has read it); it judges that one program, and not the other files it
  reads or runs (a script the wrapper calls, a library, its own configuration); a restart of the
  daemon trusts whatever is there then, a changed wrapper included; and a clock set back can
  make a later change look older. It is kept because it costs a user nothing but a restart after
  they edit their own wrapper, and refuses what a run in Full plants in the common way;
- **the settings of a lent HTTPS push** are the user's, read from the files found at the daemon's
  start: an included file that was not there then (or is another file at its path now) is left
  out of what the push is given, with a `hasconfig` include of one, so that a file made since
  (by a run in Full, a dotfiles update) cannot set `http.proxy`, `http.sslVerify` or
  `http.sslCAInfo` for it, and the user's settings of such a file apply from the next restart of
  the daemon. A file that was there is trusted as it is, even when written in place since, and so
  is the settings file itself, which the user changes as they like: neither is judged for what
  it says;
- Landlock denies **connecting** to a socket by its path from its ninth ABI only (the right
  `LANDLOCK_ACCESS_FS_RESOLVE_UNIX`). Before it, the runtime folder cannot be listed and its files
  cannot be opened, but a process that guesses the bus's path could still connect to it: the
  private mount namespace is what keeps it out there, where the kernel gives one (below);
- the runtime folder is emptied whole, and from the ninth ABI connecting is denied to every
  socket of it, not only the bus: a gpg-agent socket there cannot be reached, so a commit signed
  through gpg-agent fails;
- below the ninth ABI, connecting is not governed: a run gets no `SSH_AUTH_SOCK`, but one that
  guesses the path of an agent outside the runtime folders (`/tmp/ssh-…/agent.…`, a password
  manager's under the home) can still connect to it: the private mount namespace empties the
  runtime folders alone;
- where the kernel refuses user namespaces, a confined process has no private mount namespace:
  below Landlock's ninth ABI it connects to the bus and the other sockets of the runtime folder
  by their path, and it reaches X11's by theirs. The daemon's log says so at its start, and `flottille doctor` in its warnings
  (`sandbox.private_mounts` false in `--json`). That includes a connection's vault folder: it
  cannot list it nor read the login's token or agent-browser's configuration, and agent-browser's
  daemon's socket is in a folder of a random name it cannot guess, but it can connect to the
  login's socket, whose name it can guess and which refuses a request without the token;
- **a browser of a connection is in the runs' network where it cannot have its own** ("The
  browser's network", below): agent-browser 0.43 starts Chrome with `--remote-debugging-port=0`,
  so Chrome's DevTools listens on the loopback, on a port of its choice, with no token, and
  agent-browser's daemon listens on another. In its own network namespace neither is on the runs'
  loopback. Where pasta is missing or the kernel refuses user namespaces (a distribution that
  restricts them to programs its AppArmor profiles allow, Ubuntu from 23.10, included), the
  browser still runs, in the runs' network: a run that finds the port (`/proc/net/tcp`, a scan)
  drives it with no gate, reads its pages, the cookies and storage of every site signed in to.
  The login refuses to fill a password then, and says to install passt; the daemon's log and
  `flottille doctor` say the gap (`sandbox.browser_network` false in `--json`). Flottille never
  hands the address to an agent (the adapter keeps `get_cdp_url` off). Where the user's runs
  have no private mount namespace (above), a run is in the same user namespace as the daemon and
  can enter the browser's (`setns`): the browser's network keeps it out only where runs have
  theirs;
- **below Landlock's second ABI the browser has no private network, even for ordinary browsing**
  (since the check that a run stays out of the browser's reach is made each time a network is
  used): the server's browser runs in the runs' network, as where pasta is missing, and the login
  refuses;
- **a shown browser reads neither the user's fontconfig and GTK settings nor
  `~/.local/share/fonts`**, since Chrome's `XDG_*` folders are the vault folder's: it may look
  plainer than the user's other windows, and a certificate authority the user added to their own
  certificate database (`~/.pki/nssdb`, `$XDG_DATA_HOME/pki/nssdb`) may not be trusted by it;
- **the programs a connection's browser runs are within a run's reach.** A run in Ask or in Auto
  can write what is the user's under the home: agent-browser itself where the user installed it
  there (npm's global folder under the home, `~/.local`), and the Chrome it downloaded under
  `~/.agent-browser/browsers/`. A run that replaced that Chrome, or agent-browser, has its own
  program run as the browser the login fills. Chrome's certificate database is in the vault folder
  (`data/pki/nssdb`, above); a Chrome that still reads the older `~/.pki/nssdb`, where one is, would
  trust a certificate authority a run wrote there. Flottille does not
  check them: the owner is the user either way, and a hash recorded at first use could be taken
  of a program a run had already replaced, and would refuse every update the user or agent-browser
  makes (agent-browser downloads a new Chrome on its own). Installing agent-browser and its Chrome
  where root alone writes closes the programs' part; the rest needs a sandbox that keeps the
  home's programs read-only, a change of its own;
- a session bus outside the runtime folders is hidden at its path only: where its socket is in
  a folder a confined command can write (`/tmp/dbus-…/bus`, say), the command can move that
  folder (only the masked socket itself is a mount point), and the bus found at the start is then
  reached at its new path, which nothing hides. A socket a confined command made at the bus's
  path is hidden from the others, but the daemon's own keyring use would reach it, since its
  environment still names the path. Any change of the socket's change time (a `chmod`, a new
  link, a bus started there again) counts as replaced: the push loses the bus's address while
  the path stays hidden;
- `/proc/<pid>/root` leads to the mounts of another process: Landlock keeps a confined process
  from every process outside its domain, so it is closed wherever Landlock is used (its second
  ABI or later), and open without it: the user's SSH agent is then lent to no push (above);
- a push the gate let through still goes where the repository's remotes say (`remote.<name>.url`,
  `pushurl`, the branch's `pushRemote`, above): a run that rewrote a URL sends the push to another
  host, over git's own transports and with the sign-in the user's own helper gives that host. The
  gate judges the push's line, not the repository it runs in: an accepted risk, as a push the
  line names by its URL would go there too;
- a program the user's own settings name by its path, in a folder a confined command can write
  (a credential helper `~/.local/bin/helper`), runs outside the sandbox as written there: the
  settings are the user's, and only a bare name is looked for in the folders only root may
  change (above);
- the daemon runs every git of its own (its statuses, worktrees and fetches, the reads of the
  user's settings at its start) from the `git` found at its start: root's when there is one,
  else the first of its `PATH` then. Where root owns none, a `git` a run left in a folder of that
  `PATH` before the daemon started is the one it runs, confined and with nothing of the user's,
  and no push the gate let through is run (above);
- a `branch.<name>.merge` that names `refs/heads/<name>` is given to a push whatever the
  `push.default`: one a run wrote changes nothing of where a plain push goes. One the user set to
  another branch is not, where `push.default` is `upstream`, so their plain `git push` of that
  branch fails, and with `push.autoSetupRemote` goes to the branch of its own name and sets it as
  the upstream;
- a credential helper the user gives one forge alone signs in a push the gate let through only
  where every remote the repository knows is that forge's: a repository with remotes on two
  forges, or a remote a run added, pushes with the helpers set for every remote alone. What else
  the file of such an `includeIf "hasconfig:remote.*.url:…"` sets (a `url.*.insteadOf`, a
  `user.email`) is still included by git itself where any remote matches, a remote a run wrote
  among them: those settings are the user's own;
- what the system's `ssh` configuration (`/etc/ssh/ssh_config` and what it includes) runs is
  root's to say, and not judged;
- `~/.ssh` being read-only to every confined command, the `ssh` of the daemon's own fetch adds no
  host to `known_hosts` (`StrictHostKeyChecking accept-new` warns and goes on), and a run cannot make a key or write the user's git settings
  (`git config --global`); a push the gate let through, outside the sandbox, writes them as the
  user's own;
- a file protected at the daemon's start is protected at the path it had then: a command started
  before the user replaced it (an editor that writes a new file, `git config --global`) still sees
  the new one writable, so the push is lent nothing until the daemon restarts, which ends every
  run (`flottille daemon stop`: the next command starts another, which finds the settings again).
  **A settings file that leads elsewhere** is judged again at each push (`pushed::settle`). The
  start and the push share the judgement of **the links** (which folder may hold one: the same
  functions); the file itself is kept read-only at the start, whatever it is, and judged at the
  push by a rule of its own. A dotfiles update that moves the link to a new version is accepted
  only when **both** hold: each link on the way is in a folder kept read-only since the start
  (above) or that only root may change, **and the new file lies under a folder kept since the
  start** (above: the one that holds the old version's folder) or that only root may change, **with no other
  hard link**. So no command could have written it between the link moving and the push, as a
  command may write any file of the home outside those folders; the push then reads it, every
  command started from then on has it read-only, its identity the one recorded then, and a hard
  link made later is refused at the next push. Refused, saying which rule failed, when a link is
  in a folder a run can write (the home itself: put the link in a folder of its own below it,
  `~/.config/git/config`), when the new file is not under a folder kept since the start (a
  manager that makes its versions elsewhere than beside the one found at the start), when it has
  another hard link, or when no link leads there at all (a file replaced in its place); until the
  daemon restarts. What remains: a settings file that includes a file not found at the start (a
  relative include, which is a file of the new version folder) is refused as before, until a
  restart; the folders kept are those of the settings the daemon found, so a link that did not
  exist at the start is not kept; and a version made directly in one of the folders listed above
  as not kept (`~/projects/<version>`, say) is refused, never trusted. A manager that stages a
  version outside the folders kept (in `~/.cache`, or beside its own folder in the home, where a
  run writes) and then moves it in among its versions is trusted, not refused: the file is then
  under a kept folder with one name, and nothing tells a file moved in from one made there, so a
  run that made it first, and holds it open, writes what the push reads. A manager staging in its
  own folder (`~/dots/staging` beside `~/dots/versions`) is covered: that folder is kept. A
  manager that puts the file deeper in its versions (`versions/<n>/git/config`) has `versions`
  kept, the folder above `versions/<n>`, so a relink to another version is accepted, and a version
  it stages beside `versions` is one moved in from a folder a run writes (above); and a project
  added after the daemon's start in a kept folder, or holding one, finds it read-only to its runs until the daemon restarts. A process a run of an
  older Flottille left behind, outside its session and without its marker, is not ended at the start, and has `~/.ssh` as that version left it;
- the daemon's own fetch or listing of an SSH remote runs the user's `~/.ssh/config` with the
  lent agent, as a push does, under the same judgement; and the remote's URL is read just before
  the call, so a run that rewrites it in between sends that call to another host of the same
  protocol;
- the daemon's own git still runs the filters and the `textconv` a repository's attributes and
  settings name (`filter.<name>.smudge`, `clean`, `process`, `diff.<name>.textconv`) when it
  checks a worktree out or reads a file: git has no switch that keeps them all off. They run
  confined, as a run does, with no agent and no remote;
- a worktree the daemon makes checks out with no `SSH_AUTH_SOCK`: a Git LFS smudge filter that
  signs in over SSH (`git-lfs-authenticate`) cannot, and the worktree then gets LFS pointers or
  an error in place of the files. The agent is not lent to it: a filter is a program the
  repository names;
- the daemon's own fetch of an HTTPS remote runs its credential helper confined: one that reads a
  file signs in, one that asks the keyring over the session bus (`gh auth git-credential` with
  its token in the keyring, `git-credential-libsecret`) cannot, and the workspace then starts
  from the base as last fetched, saying why;
- from the ninth ABI, a socket made after a command started directly in a folder on the way to a
  kept-out agent socket (`/tmp` for `/tmp/ssh-…/agent.…`) cannot be connected to by that command;
- without Landlock (a kernel before 5.19, or one built without it), the process runs as before,
  with only the environment kept from it;
- a bus listening on an abstract socket (`unix:abstract=…`, older setups) is not a path, and is
  kept out only from Landlock's sixth ABI on (its abstract scope, above). A bus by its path
  outside the runtime folders is hidden only when the daemon's own `DBUS_SESSION_BUS_ADDRESS`
  named it at its start: one another program of the session uses, or one started later, is not
  known, and below Landlock's ninth ABI it is reached by its path;
- X11 by its socket's path where the kernel refuses the private mount namespace, which gives a
  process that is not given X11 an empty `/tmp/.X11-unix`: at every ABI, a process can connect to
  `/tmp/.X11-unix/X0` itself (a program of its own, or an X11 library given that path as its
  display), and below the sixth ABI `DISPLAY=:0` set by itself is enough. The folder is not
  denied by Landlock, since a rule on `/tmp` would
hold for what is made there (above). And the missing `XAUTHORITY` does not keep it out: an X11
  client without it reads `~/.Xauthority`, which a run can read, and a display manager's own key
  file (SDDM's `/tmp/xauth_…`) is readable too; a server that lets the user's local connections in
  without a key (`xhost +si:localuser:…`) needs none;
- X11 over TCP (`DISPLAY=localhost:10`, an `ssh -X` forwarding, a server listening on
  `localhost:60…`): the network is not Landlock's here, at any ABI, so a run reaches it like any
  port of the machine;
- a run given X11 is not bound to the abstract scope at all, since an X11 client connects first to
  the abstract name and does not fall back to the path when it is refused: it reaches every
  abstract socket of the session, a session bus that `dbus-launch` put on one included, whose
  name `/proc/net/unix` lists. Leaving it X11's path alone (a link of the socket in its display
  folder, `DISPLAY` naming that path) depends on the client's library and could not be shown to
  hold in tests;
- a run given X11 gets the daemon's `XAUTHORITY` as it is: a key file in the runtime folder (as
  some display managers put it) stays out of its reach, and X11 then refuses it, unless the
  server lets the user's local connections in without a key;
- a process cannot create an entry directly in a folder on the way to a runtime folder (`/`,
  `/run`, `/run/user`): they get no rule of their own, only their entries do; and an entry
  created there after the process started is denied to it. A rule on such a folder would hold
  for the runtime folder under it too, since Landlock grants by hierarchy and denies nothing. The
  user cannot write in these folders anyway; a runtime folder set in one the user writes to (an
  `XDG_RUNTIME_DIR` in `/tmp`) takes that folder's new entries from what the daemon runs.

## The browser's network

agent-browser's daemon and its Chrome, for a connection the agent-browser adapter applies to
(`docs/extensions.md`, "agent-browser"), run in a **network namespace of their own**, so that
their loopback is not the runs': Chrome's DevTools, which has no token and drives the browser
(the pages open, a password filled before it is sent, every cookie), and agent-browser's own
port listen there, where no run connects. passt's `pasta` gives the namespace its way out:

- the browser reaches the internet as the user does, through pasta, unprivileged;
- the host's loopback ports are forwarded into the namespace (`-T auto -U auto`): a developer's
  server on `localhost:<port>`, or one a run started, opens in the browser, and so does the
  host's resolver on its loopback address;
- nothing is forwarded in (`-t none -u none`): no port of the namespace is on the host's
  loopback, DevTools' included.

The namespace is held by a process of its own, `flottille browser-network`, started by the daemon
in a user and a network namespace it makes (the user mapped to themself), and pasta set up on it
by its pid. It outlives a server, as the browser does: the connection's vault folder records it
(`network`, the holder's pid and its namespaces), and the next server, a login and `close` find it
there and start each command of agent-browser's in it, before the rest of its sandbox (its private
mount namespace is made inside). The vault folder's socket keeps working by its path: a socket by
its path is no network's. It ends when the browser is closed (`close`, the daemon's stop, the
connection's removal); pasta ends with it. A browser that answers on the socket folder from
outside the namespace (started by a server that ran without one) is closed before a login or a
server uses the namespace, and a login checks, once its page is loaded, that every browser that
answers runs in it. Which process answers is asked of the kernel, by connecting to each socket of
the socket folder and of the folders in it, never through a link (`SO_PEERCRED`; a namespace puts
a session's socket in a subfolder, and `--namespace` is refused to the login all the same). The
connection does not wait for a daemon to accept, and a socket it cannot connect to, whatever the
error, counts as outside: one that stops accepting, one whose path is too long for a socket address,
one whose mode refuses the connection, and one whose process is gone; so does what cannot be
looked at (a folder or an entry that cannot be read, a link named as a socket, which agent-browser
follows). One exception: a socket nobody listens on (the connection is refused) is a browser that
crashed and left its file, in socket folders that are kept: it is removed before the count, and
holds nothing. The pids are never read from
agent-browser's `<session>.pid` files. The record is the holder's only while the process at its pid
has the start time it records: a process that took the pid since is not the holder.

**Chrome's profile is in the vault folder.** agent-browser makes the browser's profile (its
cookies, Local Storage, History, and Chrome's `Default/Preferences`, which names a proxy) in the
temporary folder, `/tmp/agent-browser-chrome-…`, and Chrome keeps its passwords there in the clear
(`--password-store=basic`): in `/tmp`, which runs reach, a run would read them or plant a
`Preferences` before Chrome reads it. Every command of agent-browser's (the server, `open`, `auth
login`, `close`) is given `TMPDIR`, the folder `tmp` of the connection's vault folder (`0700`,
denied to every run), so the profile and Chrome's own temporary files are there.

Chrome also writes outside its profile, whatever profile it is given: its crash database
(`Crash Reports`, whose dumps hold the page's memory, a password typed included) under its default
configuration folder (`$XDG_CONFIG_HOME/google-chrome`, or the folder of the Chrome agent-browser
runs, else `~/.config/…`), its certificate database (`$XDG_DATA_HOME/pki/nssdb`) and
`mimeapps.list` under `$XDG_DATA_HOME`, and fontconfig's cache under `$XDG_CACHE_HOME`: a real
login wrote all four there before this. The same commands
are given `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `XDG_DATA_HOME`, the folders `config`, `cache`
and `data` of the vault folder, and never `CHROME_CONFIG_HOME` nor `BREAKPAD_DUMP_LOCATION`,
which would move the configuration folder or the dumps elsewhere. These are set after the
connection's own `env` on every start, and a login is refused while the `env` names one of them
(the socket's, `TMPDIR` and the `XDG_*` folders included), so an edit of the connection's `env` an
agent proposes cannot move them back within a run's reach. `HOME` stays the user's:
agent-browser finds the Chrome it downloaded under `~/.agent-browser` ("The gaps" says what a run
can do there).

The vault folder is in the runtime folder, in memory on most systems: Chrome's HTTP disk cache is
capped at 32 MiB (`--disk-cache-size`, in the `args` of the configuration Flottille writes). The
cap is the HTTP cache's alone: what pages keep themselves (IndexedDB, Cache Storage, Local
Storage) is not capped, so a site that stores much takes memory until the browser is closed. A
folder on disk denied to runs would have needed a rule of its own in every run's sandbox; the
vault folder already is one. A Chrome that crashed leaves its profile behind, which agent-browser
never removes: closing the connection's browser (`close`, the daemon's stop, the connection's
removal) empties the folder `tmp` when no browser answers on the socket folder any more (a close
that did not end it, or the other sessions' browsers, keep their profiles), and so does the
daemon's start where no browser was left running. A `tmp` that is a link is never emptied.

**Nothing reads the browser's memory.** A run with the user's identity may attach to Chrome
(`ptrace`) and read the page and the password filled, or open `/proc/<pid>/root` of a process of
the user's outside its namespace, which leads to the vault folder, the browser's socket and its
profile. Landlock from its second ABI (Linux 5.19) keeps a confined process from both: it attaches
to none outside its domain and cannot open their `/proc/<pid>/root`. Yama's `ptrace_scope` keeps
the attach alone, not `/proc/<pid>/root`, which needs only the right to read: it is no substitute.
Where Landlock is missing or below its second ABI, the browser's isolation is unavailable and the
login refuses, saying so and what lets it sign in.

**Full mode.** The namespace keeps the browser from the runs Flottille confines, Ask's and
Auto's. A run that reaches the vault folder or the user's other processes as the user does, as a
run in Full mode may, can enter the namespace (`setns` into its holder) and reach DevTools: the
browser's isolation is no defence against it.

**After Flottille's program is updated in place**, the daemon that still runs cannot start the
namespace's holder, which is its own program by its path, and the login refuses until the daemon
is restarted.

Without pasta on the `PATH`, where the kernel refuses user namespaces, or where Landlock is below
its second ABI, the server still runs, its browser in the runs' network, and the **login refuses
to fill**, saying why and its remedy: "install passt (pasta) to let the agent's browser sign in",
the user namespaces to allow, or the kernel to update. Every other action of the connection works.
`flottille doctor` says whether the browser's isolation is available (`sandbox.browser_network`),
with the gap and its remedy, once, as a warning. passt is packaged by the main distributions, as
`passt` (Arch, Debian, Fedora, Ubuntu); Ubuntu from 23.10 restricts unprivileged user namespaces
through AppArmor, which refuses Flottille's own namespaces, the browser's included, until the user
lifts the restriction or allows Flottille.

## The tests' keyring

The tests never reach the user's keyring: their daemon's configuration folder holds `keyring.toml`
with `backend = "memory"`, which keeps the secrets in the daemon's memory, lost when it stops. The
daemon says so in its log at start. `locked = true` in the same file, read again at each use, plays
a locked keyring. It is chosen by the daemon's configuration, never by an environment variable an
agent could set for what it runs. Only a debug build accepts it (`cargo test`, and as much
`cargo build` and `cargo run`, which build without optimisation), never a packaged build: a
release build (`cargo build --release`, what a release ships) that finds `backend = "memory"` says
in its log that it refuses it and uses the system's keyring, and never reads a secret from that
file, so that no user's file holds secrets in plain text.
