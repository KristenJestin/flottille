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
tell them apart. The user's own form binds an entry to any host.

## Bound to one use

An entry made for one precise feature is that feature's alone. `for` names it:

| `for` | The use |
|---|---|
| `worklog:<project>` | The token the project's tracker logs time with (`docs/tickets.md`, "Logging time"). |
| `connection:<name>` | What the server of a connection is given (`docs/extensions.md`, "Connections"), and the logins its adapter signs in with (agent-browser's `login`, "Adapters"). |

- **Only its use reads it.** A feature reads an entry bound to it or an unbound one it names. An
  entry bound elsewhere is refused by every other feature and every tool, in the daemon where the
  secret is read, with a sentence naming its use (`the vault entry jira-time is bound to the
  worklog of the project app and serves no other use`), never its value. A connection naming
  another use's entry is refused when it is declared, and its server does not start; the
  worklog's `Log time with` does not offer it, and `tickets log_time` refuses it before any card.
- **Made from a feature, bound to it from the start**: the entry set from a request's card (the
  worklog's need), from a project's Settings, Tracker, "New entry in the vault", or from a feature's
  own screen, which says `for`. An entry set from Settings, Vault (or `flottille vault set`)
  without `for` is unbound; set again without `for`, an entry keeps its use, and another use for
  a bound entry is refused, naming the one it has: it is removed first. Settings, Vault shows the
  use of each entry on its row (`For the worklog of the project app alone`).
- **Seen from a project.** A project's Settings has a Vault tab: the entries that project sees (its
  own first, then every project's, marked as such), each the row of Settings, Vault. "New entry"
  there opens Settings, Vault's own dialog with "Seen by agents of" set to the project, and a click
  on a row edits the entry in it (the dialog says when the entry is every project's); closed, the
  window goes back to the tab. The tab follows the vault live.
- **A worklog's entry is its project's.** Bound to `worklog:<project>`, an entry is that project's
  (its `project` is set to it when it names none; an entry of another project is refused), so no
  other project's agent lists it, and the `vault` tool never tells an agent the use of an entry.
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
a page shows a typed secret in (percent-encoded in an address, escaped in HTML or JSON, and, with
the entry's username, in base64 as HTTP Basic authentication sends them). The values are read
from the keyring when the daemon starts, before any agent resumes (the start waits 5 seconds at
most for a keyring that asks to be unlocked, then goes on), and a new secret is masked as soon as
it is set. A secret replaced or removed stays masked until the daemon restarts: another entry may
hold the same value.

## Choosing the entry for time logging

A Jira project's Settings, Repositories tab, Tracker, names the entry (`docs/projects.md`, "Forges
and tracker"): the select `Log time with` lists the entries the project sees by name and username,
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

- **`vault {action: "ask", name, why, url?, username?, for?, project?}`.** `name` is the new
  entry's name; `url` the address the secret is for, which the daemon reads as a connection's
  `login` does (a backslash is a `/`, a domain is said in ASCII) and refuses when it is no `https`
  address, or `http` on this machine; `username` the account; `for` the use it is bound to,
  `worklog` (the agent's project's) or `connection:<name>` (a connection the user declared), or
  none; `project` `this`
  (the agent's project's alone) or `every`, a connection's entry being every project's. A name
  the vault already holds is refused, naming it: the agent uses it (when its project sees it) or
  asks under another name. An entry a feature of Flottille names and the project misses (today
  the tracker's `worklog_token`, `docs/tickets.md`, "Logging time") keeps the feature's prefill:
  its description and its use, the worklog.
- **Offered with the refusal.** The call that needed the entry is refused as before, and the
  refusal says the entry may be asked for, with the call to make: `tickets log_time` with its
  name, a browser connection's `login` with its name and `for: "connection:<name>"`
  (`docs/extensions.md`, "Connections"). The ask answers at once: nothing waits for the user.
- **A way in to the vault's own form.** The user sees one card in the session of the agent that
  asked (and an inbox entry, which names the host and whose entry it is, the project's or every
  project's): the entry's name, the host of its URL **on a line of its own** (where the secret
  will go: the agent chose it; the daemon's reading, never the window's), what it is for, its use,
  its project and the agent's reason, with **New entry** and **Dismiss** (disabled while
  its call runs). New entry opens Settings, Vault's form of a new entry, prefilled with all
  of it, the agent's reason under what it is for, and what it will be bound to ("For …") with the
  host its secret goes to ("Goes to …", under the URL's field, bound or not, while the URL is the
  one asked) and **Remove the binding**: the user changes any field, types the secret and saves,
  with the checks of that form (a connection's entry refused for a URL login cannot read, in the
  words of the ask's refusal, or plain `http` to another machine; the username required when the
  feature that asked reads it, the worklog's), or closes it and nothing is stored. There is no
  second form.
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
answered the same. The first users: `tickets log_time` (`docs/tickets.md`, "Logging time") and
Connections (`docs/extensions.md`, "Connections"): a connection's `env` and `secrets` name entries
of every project, read when its server starts and put in its environment or on its requests,
never shown to an agent; an entry tied to one project, or bound to another use, is refused for
it. A connection an agent proposes (`docs/extensions.md`, "Connections") puts an entry in its requests
only when the entry's `url` has the connection's host: the `url` is where the secret may go, and
an entry without one goes in no request an agent chose; the user's own form binds freely, and the
card of the request says where each entry goes. An agent's connection may name an entry that does
not exist yet, for the user to save (never one that looks like a secret): the request waits, its
Accept refused while the entry is missing, and once saved the entry's host is checked as any
other's. `vault::secret` and the connections' reading take the use asking, and refuse an entry bound to
another.

## The runtime folder

The keyring answers on the session bus, whose socket is in the user's runtime folder
(`/run/user/<uid>/bus`), and any program can speak D-Bus: refusing `secret-tool` or `busctl` by
name keeps nothing out. So what the daemon runs for an agent or a connection reaches no session
bus: a run, a project's service and its readiness probe, a workspace's setup line, a commit's or
a push's message checks and the git reads made for them (the push's dry run, with the line's own
options and the remote's transport), a connection's server and its login:

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
  The folder is the one the daemon made, never one a variable of the connection names.

**The SSH agent is lent to a push alone.** What the daemon runs gets no `SSH_AUTH_SOCK`, but a run
that is itself a `git push` (its own line, no wrapper) the user, the judge or Full mode let
through, and the dry run made for its checks, which reaches the remote: their `SSH_AUTH_SOCK`
is the socket the daemon's own environment named **when the daemon started**, resolved then
(links followed) and kept with its device and inode, never a path a variable of the run or the
agent gives. At each push the name is followed again and the socket looked at again: a link moved
since, or a socket put in its place, lends nothing (the push runs without an agent). A socket that
is the session bus is never lent. From Landlock's ninth ABI the push is granted
`LANDLOCK_ACCESS_FS_RESOLVE_UNIX` on that socket file alone (its device and inode checked again in
the child). Landlock checks connecting from the socket's own path up and counts that right among a
file's, so the rule on the socket grants it and nothing else: the runtime folder stays unlisted,
its other sockets (the bus, gpg-agent) refused. A socket outside the runtime folder
(`/tmp/ssh-…/agent.…`, a password manager's under the home) is lent the same way, and from the
ninth ABI every other command the daemon runs is kept from connecting to it: the folders on the
way to it are ruled without the right to connect, their other entries with every right.

What runs with the agent is pinned. The push runs the `git` the daemon found on its own `PATH` at
its start, whatever the line's `git` and the run's `PATH` lead to, and its `GIT_SSH_COMMAND` is the
`ssh` found then: for each, the first one outside the user's home (`~/bin`, `~/.local/bin`,
`~/.cargo/bin`, the projects, the agents' folders) and Flottille's folders, links followed, so
neither is a program the run's `PATH` or a folder of the home puts first. `GIT_SSH_COMMAND` wins
over `core.sshCommand` and `GIT_SSH`, which the gate refuses on the line anyway, and it shares no
connection (`-o ControlMaster=no -o ControlPath=none`: no master outlives the push for another ssh
to reuse) and runs no `LocalCommand` (`-o PermitLocalCommand=no`). No such `git` or `ssh`, no agent
is lent, and the daemon says why at its start. `GIT_ALLOW_PROTOCOL=ssh` takes away every other
transport (`ext::` and `fd::` run a program, a path runs `git-receive-pack` here), each remote's
`receivepack` is set back to `git-receive-pack`, and `maintenance.auto` is `false` (no
`git maintenance run --auto --detach` goes on after the push with its environment), all through
the environment, which wins over the repository's settings. The push's own line takes no global
option but `-C` into its repository's own folder (no other repository on the way), `--no-pager`,
`-P`, `--no-advice` and `--no-optional-locks`: a setting (`git -c …`, `--config-env`) would win
over the pins and the hooks' wall, `--exec-path` would run git's programs from another folder, and
`--git-dir`, `--work-tree` and the rest bring another repository, whose remote no one checked.
Such a push is refused in every mode, saying why and to push by hand. Such a push signs in over
SSH only: a push to an HTTP(S) URL is refused before it runs (git resolves its URLs in an offline
dry run, which is not lent the agent, since a repository of a path may answer it), saying to give
the remote an SSH push URL (`git remote set-url --push`) or to push by hand. The other git reads
made for a push, and the probes of a push's task (its `ready` and `url` commands), are not lent
the agent.

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
reached), so the private mount namespace of #976 masks the folder instead (below, "The gaps").
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
  cannot be opened, but a process that guesses the bus's path can still connect to it;
- from the ninth ABI on, connecting is denied to every socket of the runtime folder, not only the
  bus, but the SSH agent's lent to a push (above): a gpg-agent socket there cannot be reached, so a
  commit signed through gpg-agent fails;
- below the ninth ABI, connecting is not governed: a run that is not a push gets no
  `SSH_AUTH_SOCK`, but one that guesses the agent's path can still connect to it, as to the bus;
- what `ssh` reads is not pinned: the user's `~/.ssh/config` may run a program
  (`ProxyCommand`, `Match exec`) with the agent's socket, and pinning it would break jump hosts.
  Runs may write under the home, so from the ninth ABI the guarantee is this: code an agent wrote
  reaches the SSH agent, at the next push the gate lets through, only through the files the
  user's `ssh` reads from the home (`~/.ssh/config` and what it includes); a `git` or an `ssh`
  earlier on `PATH` is not run (above). Connection sharing is off for the push itself, but a
  master another program of the user's started (`ControlMaster` in `~/.ssh/config`) is not
  Flottille's to close;
- from the ninth ABI, a socket made after a command started directly in a folder on the way to a
  kept-out agent socket (`/tmp` for `/tmp/ssh-…/agent.…`) cannot be connected to by that command;
- without Landlock (a kernel before 5.19, or one built without it), the process runs as before,
  with only the environment kept from it;
- a bus listening on an abstract socket (`unix:abstract=…`, older setups) is not a path, and is
  kept out only from Landlock's sixth ABI on (its abstract scope, above);
- X11 by its socket's path, until the private mount namespace (#976) gives a run that is not
  given X11 an empty `/tmp/.X11-unix`: at every ABI, a process can connect to
  `/tmp/.X11-unix/X0` itself (a program of its own, or an X11 library given that path as its
  display), and below the sixth ABI `DISPLAY=:0` set by itself is enough. The folder is not denied, since a rule on `/tmp` would
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
