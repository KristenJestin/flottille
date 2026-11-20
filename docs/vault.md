# The vault

Agents sometimes need a secret the user holds: a tracker's API token to log time, the login of a
test account, a deploy key. Pasted in a conversation, it would be stored and shown. The vault keeps
it instead, where the operating system keeps secrets, and lets agents **use it without ever seeing
it**.

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
  The folder is the one the daemon made, never one a variable of the connection names;
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

**The SSH agent stays out of every run.** What the daemon runs gets no `SSH_AUTH_SOCK`. The socket
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
workspace's base) is bound by the same Landlock rules, gets no `SSH_AUTH_SOCK`, and is given what a
repository's settings could otherwise turn into a program of theirs: `core.hooksPath=/dev/null`,
`core.fsmonitor=false`, `fetch.recurseSubmodules=false` and `submodule.recurse=false` on its line
(the command's scope, which wins over the repository's settings and what they include: git cannot
be told to skip an `include.path`, so what it brings is overridden instead), and
`GIT_SSH_COMMAND` naming the `ssh` found at its start, which wins over `core.sshCommand` and
`GIT_SSH`: the first `ssh` of the daemon's `PATH` that root owns in a folder root owns, else the
first one (said once in the daemon's log), and in either case the program its links lead to
then, so that a link pointed elsewhere later changes nothing. On the same line,
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
- when that protocol is SSH, the user's SSH agent, lent as to nothing else: `SSH_AUTH_SOCK` names
  the socket found at the start, and from Landlock's ninth ABI a rule on that socket alone grants
  connecting to it, the rest of the runtime folder (the bus) staying denied. An HTTPS remote gets
  no agent and no session bus: its credential helper runs confined (below, "The gaps").

**A push the gate let through runs outside the sandbox.** A run that is itself a `git push` (its own
line, no wrapper) the user, the judge, a rule or Full mode let through, and the dry run the
project's commit checks make of it, run with no Landlock and the daemon's own environment, as if
the user had typed the push: the session bus (so the keyring), the SSH agent, the credential
helper and the forge command line's sign-in are theirs. Only these are added:

- `core.hooksPath=/dev/null`;
- `GIT_SSH_COMMAND`, so that a `core.sshCommand` a run wrote in the repository is not what signs
  in: the user's own when they have one (the daemon's `GIT_SSH_COMMAND`, else the
  `core.sshCommand` of their own git settings as they apply to the repository pushed, an
  `includeIf "gitdir:…"` or `"onbranch:…"` for several accounts of one forge included, read from
  the files of those settings found at the daemon's start and never from the repository's), else
  the `ssh` found at the daemon's start, as for its own fetch (above). A command of the user's
  that names `ssh` alone runs that one; one that names another program runs it only when it is
  in `~/.ssh` or only root may change it and its folders, and one with `-F` (another
  configuration), `-I` (a library), an option that runs a program or anything the shell would
  read as more than words lends the agent to no push (below);
- `GIT_ALLOW_PROTOCOL` set to git's own transports (`ssh:https:http:git:file`), which wins over
  the `protocol.*.allow` settings: a URL a run set through a helper (`ext::<command>`, a
  `git-remote-<name>`) is not run. The push's remote is git's to resolve (the line's options, the
  branch's `pushRemote`, `remote.pushDefault`), and its URL a setting a run can write, so the
  transports allowed are git's own five rather than the one protocol of a remote. A push through
  a remote helper of the user's own is refused too;
- what the repository's settings may name as a program of its transport is not run: an empty
  `GIT_PROXY_COMMAND` and `GIT_ASKPASS`, which win over `core.gitProxy` (a `git://` remote) and
  `core.askPass` (an HTTPS sign-in with no helper), `--receive-pack=git-receive-pack` on the
  push's line, right after `push` (git takes the first `remote.<name>.receivepack` it reads, the
  repository's, and runs it on this machine for a remote reached by its path; the gate refuses
  the option on the agent's own line), and `push.negotiate=false`, so that no fetch runs the
  remote's `uploadpack` first. The user's own `core.gitProxy` and `core.askPass` are not used by
  such a push either;
- no `DBUS_SESSION_BUS_ADDRESS` once the bus the daemon found at its start by a path outside the
  runtime folders is gone or replaced (above): what is at that path now may be a run's. The
  daemon's own use of the keyring (the vault) is unchanged: it still reaches the bus its
  environment names.

The project's and the workspace's variables are not applied (`docs/commands.md`, "The
environment"). The other git reads made for a push, and the probes of a push's task (its `ready`
and `url` commands), run in the sandbox.

**What such a push reads is read-only to every confined command.** The push runs the user's `ssh`,
with their agent, outside the sandbox: whatever a run could write into what `ssh` reads
(`ProxyCommand`, `Match exec`, an `Include`d file) would run there with the user's keys. So in the
private mount namespace of every command Flottille confines (above), `~/.ssh` is read-only whole,
with what is below it, and so are the files of the user's own git settings found at the daemon's
start (`GIT_CONFIG_GLOBAL` when set, else `$XDG_CONFIG_HOME/git/config` and `~/.gitconfig`) and
those they include (`include.path`, `includeIf.*.path`, whatever their condition): those git
reads at every push, and the daemon reads its user's `core.sshCommand` from. Each folder on the
way to them that a command could rename or remove (its own folder is the user's, or one they may
write) is bound onto itself there: a mount point is neither moved nor removed, so no folder on
the way is renamed to put another file at the path. `~/.ssh` is the account's (the home of the
user database, which `ssh` reads `~` as, whatever `HOME` says); the daemon makes it at its start
(`0700`, as `ssh` would) when it is missing, since a folder that is not there could not be kept
read-only. A command started at any time has them read-only from its start, and nothing it runs
can unmount them (above).

What that cannot hold is not protected file by file: the user's SSH agent is then **lent to no
push**, no `SSH_AUTH_SOCK` and no `ssh` among the transports allowed (a push over SSH fails, saying
why on its result), and, when it is what `ssh` itself reads, the daemon's own fetch is not lent
the agent either. It is judged at each push, from the files as they are then, and `flottille
doctor` says it in its warnings with how to fix it (`sandbox.ssh_lent` false in `--json`):

- `~/.ssh` is not a folder of its own (a link), or no longer the one found at the daemon's start;
- a link anywhere in `~/.ssh` (`ssh` follows it to a file a run can write: put the file itself in
  `~/.ssh`), a file with a hard link elsewhere, or more than 10,000 entries;
- an `Include` of `~/.ssh/config`, or of a file it includes, outside `~/.ssh` (`~` read as the
  account's home, a relative path in `~/.ssh`, no `..`), or that names a variable, a token
  (`%d`) or a class of characters, which Flottille does not read: move the file into `~/.ssh`;
- a program the configuration runs or loads (`ProxyCommand`, `LocalCommand`, `KnownHostsCommand`,
  `Match exec`, `PKCS11Provider`, `SecurityKeyProvider`, `XAuthLocation`; `none` is none), which
  may itself be, or be found through the `PATH` in, a file a run writes. `ProxyJump` is not one;
- the git settings include a file that is not there (a run could make it), one that was not there
  at the daemon's start, or one with a hard link; a file of them is no longer the one found at the
  start (git replaces the file it edits: `git config --global` is followed by a restart of the
  daemon);
- another mount of the same files (a bind mount of the home elsewhere, the top of a Btrfs volume
  mounted beside its subvolumes), or a mount below them, which stays writable;
- the kernel gives no user namespace to the commands Flottille confines (above);
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
  (`sandbox.private_mounts` false in `--json`);
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
  ABI or later), and open without it;
- a push the gate let through runs with what the repository's settings say, as the user's own
  push from that folder would: a script the gate never read may have changed them before
  (`credential.helper`, `core.fsmonitor`, `url.*.insteadOf`; not `core.sshCommand` nor a helper
  URL, the transport's programs, above; `gpg.program` with `push.gpgSign` neither), and that
  applies to the push, outside the sandbox, with the user's credentials. The gate judges the
  push's line, not the repository it runs in: an accepted risk, decided so that a push the user
  allowed signs in as theirs does;
- a push the gate let through finds `git`, and the programs its settings name by a bare name
  (`!gh auth git-credential`, a helper, `ssh` inside the user's own command), through the
  daemon's `PATH`: a folder of it that a confined command can write (`~/.local/bin`, `~/bin`)
  can hold one of a run's before the system's, which then runs outside the sandbox. The `ssh`
  Flottille names itself is the one found at the start, whose links were followed then;
- what the system's `ssh` configuration (`/etc/ssh/ssh_config` and what it includes) runs is
  root's to say, and not judged;
- `~/.ssh` being read-only to every confined command, the `ssh` of the daemon's own fetch adds no
  host to `known_hosts` (`StrictHostKeyChecking accept-new` warns and goes on) and makes no
  `ControlMaster` socket there, and a run cannot make a key or write the user's git settings
  (`git config --global`); a push the gate let through, outside the sandbox, writes them as the
  user's own;
- a file protected at the daemon's start is protected at the path it had then: a command started
  before the user replaced it (an editor that writes a new file, `git config --global`) still sees
  the new one writable, so the push is lent nothing until the daemon restarts, which ends every
  run. A process a run of an older Flottille left behind, outside its session and without its
  marker, is not ended at the start, and has `~/.ssh` as that version left it;
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
