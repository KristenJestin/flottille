# Remote

Flottille on several machines, driven from any of them and from a phone. A machine is a place an
agent can send work to, as it sends work to a workspace: a session on the user's computer starts a
session on their server for the night, that one works alone, and its report comes back to the
computer when the computer is on again. Every client (the window, the command line, the Android
app) reads and answers the sessions of every machine the user paired. It is the milestone
`v0.4 - Remote` (`docs/design.md` §12, §13).

**Status: the design of v0.4, decided with the maintainer on 2026-11-12.** Nothing here is built.
What is decided is said as such; the defaults taken are in "Defaults taken, to confirm"; what is
open is in "Open questions". It replaces the proposal of 2026-11-03 (a relay every device connects
to, on a private network) and the review that put Tailscale and SSH at the centre: Flottille stays
generic (`AGENTS.md`), and depends on no one network, no always-on server and no account.

## The cases it serves

1. **Work for the night.** A session has worked all afternoon on the computer. The user tells it
   "start an agent on the server, it is alone tonight, ask nothing". The session starts a session
   on the server with a brief it writes, the branch and what is not committed go with it, the
   computer is switched off. The server works; what it would ask goes to the user's phone, since
   its pilot is off. In the morning the computer comes back, its session receives the report and
   the branch, and the user reads what was done.
2. **Following from elsewhere.** A session runs on the computer; the user is away. From the phone
   (or the window of another machine) they read the conversation live, write, send a picture,
   answer questions and requests, switch the mode, stop a turn. The phone rings only when the user
   is not already at a client.
3. **Opening what a run serves.** A dev server an agent started on the server listens on
   `localhost:5173` there; the user opens it from the computer's browser or from the phone, through
   Flottille's own link.

## Words

| Word | Meaning |
|---|---|
| Machine | A computer running a Flottille daemon: it runs agents, holds projects. |
| Client | Something the user drives Flottille with: the window, the command line, the Android app. A client runs no agent. |
| Device | What holds a key the user paired: each machine, and each client device (the clients of one machine's user session share one device; a phone is one). |
| Link | Flottille's encrypted connection between two devices, direct when it can be, through a relay otherwise. |
| Pairing | Adding a device to the user's paired devices, with a code a paired device opened. |
| Relay | A small program of Flottille's, run by the user wherever they like, that passes the link's encrypted bytes between two devices that cannot reach each other directly. It reads and forges nothing. |
| Tunnel | A TCP connection carried over the link, from a client's local port to a run's port on a machine. |

"Led session" keeps its meaning (`docs/tools.md`, "Sessions an agent starts"). A led session on
another machine is the heart of case 1; **whether to rename led sessions "missions" is open**
(below).

## What is decided

1. **Generic.** No dependency on Tailscale, on a server always on, or on one network. Identity and
   transport are Flottille's own. A tailnet address is one more direct path, found or declared by
   hand like a LAN or public address.
2. **End to end.** Between two devices the link is encrypted and authenticated by their keys. A
   relay, a router, a tailnet passes bytes it can neither read nor forge.
3. **A tree across machines.** `agent_launch` (`child` or `led`) takes `machine`. Reports, asks,
   requests and deliveries follow the tree over the link; when the other side is off, the sending
   machine keeps them and delivers them when it is back. **No queue on a relay.**
4. **No handoff, no moving a session.** The parent writes the brief of the session it starts on the
   other machine, as it does on its own; the compaction handoff (`design.md` §5.8, 7) is unrelated.
   The user asks for it in words; there are no presets and no "solo" modes.
5. **The agent knows nothing of phones or clients.** It asks (`ask`); Flottille routes: to its
   parent or pilot when reachable, to the user wherever they are otherwise.
6. **The user's actions are signed by their device's key.** A machine may carry them, never forge
   them. A machine on its own can do only what an agent can: start a session (judged by the gate),
   deliver a report, ask, forward a request. Every paired device of the user has every right of the
   user; per-device permissions come later.
7. **Vault and Connections are shareable**, entry by entry, with the user's consent, end to end
   into the other machine's keyring. A remote connection's sign-in is done from a client that has a
   browser. Local connections and the services of a machine (`projects.md`, "Services") stay the
   machine's own.
8. **One system of port forwarding**: tunnels over the link, to the ports of runs Flottille started
   only.
9. **The Android app is native**: Kotlin and Jetpack Compose, over a Rust core shared with the
   desktop through UniFFI. Notifications through UnifiedPush, carrying ids only, encrypted, never
   while the user is active at another client.
10. **Versions are checked at connection**, and a mismatch is said in a sentence.
11. **One repository** for the daemon, the relay, the mobile core, the Android app and the site,
    with CI split by paths.

## Architecture

```
  phone (app + Rust core) ──┐                     ┌── machine "pc"  (daemon, agents)
                            │  link: end to end   │      ▲ local socket
  window, CLI on "pc" ──────┼─ (direct, or ───────┤      └── window, CLI of "pc"
     (through pc's daemon)  │   through a relay)  │
                            │                     └── machine "server" (daemon, agents)
                       relay (optional, the user's, passes ciphertext)
```

- **Each machine's daemon holds one link endpoint** and listens on it beside its Unix socket. The
  local socket is unchanged, with its refusal of a client an agent started (`protocol.md`,
  "Transport").
- **The window and the command line reach another machine through their own machine's daemon**: a
  request names `machine`, is signed with the device key by the client, and the local daemon
  carries it over its link to that machine and carries the answers and events back. The local
  daemon cannot forge a request: it does not hold the device key.
- **The phone holds its own endpoint**, whose key is its device key, and links to each machine
  directly. It links only while the app is in the foreground; a push wakes it otherwise
  ("Notifications").
- **Two kinds of traffic on a link**, kept apart by the link's protocol name (ALPN or its
  equivalent): `client`, the user's requests (the methods of `protocol.md`, each signed by a
  device), and `machine`, the closed set of messages one machine sends another for its agents
  ("What a machine may send"). A message of one kind on the other is refused.
- **The protocol is the daemon's** (`protocol.md`), unchanged in its framing: the same requests,
  answers and events, numbered, resumed with `events.watch {after}`. It was written for this.

## The transport

What Flottille needs of it: a device's identity is its public key; the channel between two keys is
encrypted end to end; it connects directly over every path that exists (same LAN, a public address,
a hole punched through NAT, a tailnet address) and falls back to a relay that sees only ciphertext;
the relay can be self-hosted; it runs on Linux, macOS later, and Android; it multiplexes streams
(requests, events, a git transfer and tunnels at once on one link).

**The choice of transport is open** (it is being researched apart). The current candidate is
**iroh** (Rust, QUIC, an endpoint's id is its Ed25519 public key, hole punching, relays public or
self-hosted through `iroh-relay`, version 1.0 with official Kotlin bindings, used on Android by Delta
Chat and by an app that drives coding agents from a phone). The alternatives weighed so far:

| | Identity = key | End to end | Direct + NAT | Own relay | Android | Notes |
|---|---|---|---|---|---|---|
| iroh | yes | yes (QUIC TLS on raw keys) | yes | `iroh-relay` | yes, in production | a dependency of weight (QUIC stack, DNS, port mapping); its default discovery uses n0's servers and is turned off here; a known Android issue on 4G with segmentation offload, worked around by turning it off on the phone |
| rust-libp2p (QUIC, hole punching, circuit relay) | yes | yes (Noise/TLS) | yes | yes | possible, less proven | larger and more general (DHT, pubsub), little gain for a few paired devices |
| QUIC (quinn) + Flottille's own NAT traversal and relay | to build | to build | to build | to build | yes | what iroh already is, without its years of fixes |
| WebSocket over TLS to a relay (the 2026-11-03 proposal) | to build | needs a second layer (Noise) | no | yes | yes | every byte through the relay, no direct path |
| WireGuard in user space | yes | yes | partly | no | conflicts with the system VPN slot | needs a network stack of its own in the process |
| Tailscale / tsnet | Tailscale's | yes | yes | DERP | yes | a dependency on one network and its coordination server: rejected (decision 1) |

The rest of this document is written against these needs, not one library: "an endpoint", "an
address", "a relay URL", "a stream". The crate that holds it, `flottille-link`, hides the library
from the daemon, the clients and the mobile core, so that a later change of transport changes one
crate. Whatever is chosen is a dependency of weight, said in its pull request.

**Paths.** A machine's addresses are what the transport finds (its interfaces, LAN discovery where
the library has it, a public address seen from outside), plus the addresses the user declares by
hand (`flottille link address add 100.64.0.7`, Settings, Machines): a tailnet address, a DNS name, a
forwarded port. Paired devices tell each other their addresses whenever they change, over the link.
No discovery service of a third party is used: a device found nowhere but through a relay is reached
through the user's relay, if they have one ("The relay").

## Identity and keys

- **The machine key** is made by the daemon at its first start with the link on:
  `$XDG_STATE_HOME/flottille/link/machine.key`, mode 600, the folder 700. The daemon alone reads it.
- **The device key of a machine's clients** (the window and the command line of that user session)
  is in the session's keyring (`vault.md`, "Where the secrets are"), an item `Flottille: device`; on
  a machine with no Secret Service (a headless server), `$XDG_STATE_HOME/flottille/link/device.key`,
  mode 600, read by the clients only. The daemon never reads it.
- **The phone's device key** is made by the app, kept encrypted under a key of the Android Keystore,
  and never leaves the phone.
- **The gate refuses `link/`** in every mode, read or write, as a login's place (`design.md` §7),
  where reading Flottille's own folders is otherwise base. What this does not cover is Full mode's
  limit: code an agent runs acts with the user's rights. So a key read by an agent's code gives what
  that key may do, and no more: a machine key gives only what a machine may send (below), never the
  user's methods.
- No private key is ever sent, logged, shown or returned by a method.

## Pairing

The user's paired devices are a list every device keeps: each one's name (unique), kind (`machine`
or `device`), public key, addresses, when and by which device it was paired, and the signature of
that device. The list starts with the first machine alone.

1. **On a paired device**, the user opens a pairing: Settings, Machines, **Pair a device** in the
   window, `flottille link pair --name <name> [--machine]`, or the phone's **Pair**. It is a user
   action, signed by that device. It shows a QR and the same as a code to copy: the opening device's
   key, its addresses and relay URL, the protocol version, and a random secret of 128 bits. It is
   valid 10 minutes, once.
2. **The new device** reads the QR (the phone), or is given the code (`flottille link join <code>`
   on a machine, over the user's own terminal), makes its key, links to the opening device and
   proves it holds the secret.
3. The opening device adds it to the list, signs the entry, and sends the list to every paired
   device it reaches; the others get it when they next link. **Every connected client announces it**:
   `New device: pixel, paired at 18:42 from pc`, with Revoke beside it, so that a pairing behind the
   user's back is seen at once.

**Revoking** (Settings, Machines, the row's Revoke, `flottille link revoke <name>`) is a signed user
action: every device that hears it removes the device and cuts its links at once; one that was off
hears it when it links again, before anything else is exchanged. A revoked device cannot pair again
with its key. **Renaming** is the same kind of signed change. A device that wants to leave removes
itself the same way.

A device refuses a link from a key that is not in its list, and a list change not signed by a
paired device.

## The version

When two devices link, each says its Flottille version and the protocol versions it speaks (a
number, `protocol` in `daemon.status` too, raised by any change of a method, an answer or an event
that an older client would misread). They use the highest one both speak; with none in common the
link is refused on both sides with a sentence that says which to update: `server runs Flottille
0.4.3 (protocol 9), this machine 0.4.1 (protocol 8): update one of them (flottille update)`. The
window and the app show the machine as **needs an update**, never as offline. At first each side
speaks one version; the list lets the Android app, published apart, speak two.

## Requests from another machine's clients

- A request for another machine carries `machine` and a signature: `{"id", "machine", "signed":
  {"device", "nonce", "at", "method", "params"}, "sig"}`. The client crate makes it; the local
  daemon carries it. The receiving daemon checks the device is paired, the signature, the time
  (within 5 minutes of its clock) and that the nonce was not seen in that window, then serves the
  request as one from a local client, the device known: its log and its events name it.
- **What the user sends to a machine that is off is refused**, never kept: a message, an answer to a
  request, a new session. The client says so, with since when (`server is offline since 22:14`). A
  message kept would arrive hours later, after the user forgot it.
- **Paths are picked, not typed**, from the lists the machine gives, except where the user types one
  on purpose (`flottille --machine server project add ~/code/app`, which is that machine's path).
- **Pictures** travel as bytes already (`agent.send {images}`); files other than pictures are left
  for later.
- **A link or a file an agent shows** (`web` `show_link`) belongs to its machine: a `localhost`
  address of another machine opens through a tunnel ("Tunnels"); a file of another machine is shown
  with its machine and path, and opened later.

The command line takes `--machine <name>` on every command (`FLOTTILLE_MACHINE` too). The window
shows **one machine at a time**, chosen in a switcher at the top of the sidebar (this machine first,
then each paired machine with its state: online, offline since, needs an update); merging the lists
of several machines in one sidebar is left for later. A led session on another machine appears under
its pilot with the machine's name, and opening it switches to that machine.

## What a machine may send

On the `machine` side of a link, a machine sends only these messages, each naming the agent of its
own that it acts for, and the receiving machine checks each against what it recorded (that this
agent of that machine started that session, or is that child's parent):

| Message | What it does on the receiving machine |
|---|---|
| `launch` | Starts a session or a child for an agent of the sender (`agent_launch` with `machine`), judged by the receiving machine's gate ("A tree across machines"). |
| `git` | Sends the objects of a place's branch and uncommitted state, or asks for those of a session it started ("What travels"). |
| `deliver` | A delivery for an agent of the receiver: a report, a question, an answer, a request forwarded, `session-turn-ended`, a child's news, as `design.md` §5.2–5.3 make them. |
| `call` | One of the tree's tools aimed at a session it started: `agent_send`, `agent_status`, `agent_stop`, `session_send`, `session_read`, `approval_answer`. Judged as on one machine. |
| `presence` | Which of the user's clients is active ("Notifications"). |
| `members` | The paired list and its signed changes. |
| `share` | A vault entry or a connection the user shared, signed by their device ("Sharing"). |

Nothing else: no user method, no `approval.approve`, no `agent.mode`, no `agent.send` as the user.
A machine that forwards a user's request does so on the `client` side, with the device's signature.

## A tree across machines

### Starting a session on another machine

`agent_launch {kind: "child" | "led", machine, title, brief, model?, role?, …}`. `machine` names a
paired machine; left out, this one. `handed` takes it too.

- **Who**: as today. `led` and `handed` from a root only; `child` from any agent that launches
  children. A led session on the computer may start a child on the server; a led session on the
  server is a root there, and may start its own children there.
- **The machine must be online**: otherwise the call is refused at once, naming since when it is
  offline.
- **The project** must be registered on that machine under the same name, with the same
  repositories (by their paths in the project): otherwise refused, with what to do (`the project app
  is not on server: register it there with flottille --machine server project add <folder>`). The
  first version registers nothing by itself and syncs no configuration.
- **The place**: a new workspace on that machine, on the caller's branch for each repository of the
  caller's place, with the caller's uncommitted changes laid on it as uncommitted changes. A `child`
  on another machine works in that copy of its parent's place: a child still works in its parent's
  place, which here is the same branch on another machine.
- **Judged by the receiving machine's gate**, every time, child or led, as a session launch is
  (`tools.md`, "Sessions an agent starts"): the words name the machine (`Start a session led by
  agent … of pc on this machine, in a new workspace on feat/export with its uncommitted changes:
  "Finish the export"`), the brief whole. The receiving machine judges because its quota, its
  machine and its secrets are used, and because a machine alone must not be able to start anything
  unjudged. The caller is answered at once with the session's id, or the request's number on that
  machine (`request #4 on server is waiting for the user`).
- **The mode** is the caller's tree's, as for any session an agent starts, **except Full**, which a
  machine alone cannot raise: a session on another machine is in Full only when the caller's tree
  was put in Full by the user from a device, whose signed switch the caller's daemon keeps and sends
  with the launch. Otherwise it starts in Auto.
- **The provider** is the caller's if it is enabled and signed in on that machine; otherwise
  refused, naming it. The judge and the compactor are that machine's own settings.
- **Caps**: the child counts in its parent's project cap on the parent's machine and in the
  project's cap on the receiving machine; refused above either, never queued.
- **The brief** is the caller's, as today: nothing of its conversation goes with it.

### What travels

For each repository of the caller's place, before the session starts:

- **The uncommitted state** becomes one commit made from a temporary index (`GIT_INDEX_FILE`): the
  changed and untracked files that are not ignored, on top of `HEAD`. The user's branch, index and
  files are not touched; the commit is held by `refs/flottille/outgoing/<launch>`.
- **The objects** the other machine lacks go as a git bundle over the link: the receiver says which
  commits it has (its branches, `refs/flottille/machines/*`), the sender bundles what is past them,
  the receiver verifies the bundle and fetches it. Git is the user's own `git`, spawned without a
  shell; nothing is pushed to the repository's remote, nothing goes through a forge.
- **Ignored files do not travel**: a `.env` is the machine's own, taken by its setup's `copy` steps
  as today. A setup that misses one fails as it does today, saying which.
- On the receiving machine the workspace is created from those commits, on the same branch name
  (with `-2`, `-3` when that name is taken there), prepared by the project's setup, and the
  uncommitted commit is laid on it as uncommitted changes.

**Coming back.** With a report (`child-report`, or a led session's `session-turn-ended` that
reports), the receiving machine sends the tips of the session's branches; the caller's machine
fetches them, the same way, into `refs/flottille/machines/<machine>/<branch>`, and the delivery
names that ref. Nothing moves the user's branch: the agent or the user merges it (a `git merge` run,
judged as any). The pilot of a led session on another machine stops it and removes its workspace as
today, through `call`.

### Deliveries across machines

- **Stored before sent**, as every delivery (`design.md` §5.2): a delivery for an agent of another
  machine waits in the sending machine's store until that machine acknowledged it, and is sent again
  when the link comes back, in order, once (its id is idempotent). A report written at 3:00 on the
  server reaches the computer at 8:00 when the computer links again. Kept 30 days, then dropped with
  an entry in the inbox of the sending machine.
- What the agents send to each other (`agent_send`, `session_send` to an offline machine) is kept the
  same way; only the user's own requests are refused when the machine is off.
- `agent_status`, `session_read` and `agent_stop` aimed at an offline machine are refused at once,
  saying since when; the agent is told it will be woken by what comes back.

### Where a question goes when the parent is off

- **A led session's `ask`** goes to its pilot while the pilot's machine is linked. When that machine
  is not linked, it goes to the user at once, as when the pilot stops following (`tools.md`, "Where a
  question goes"); questions already open to the pilot go to the user when its machine goes off.
  When it comes back, the pilot gets one delivery saying what was asked and answered meanwhile.
- **A child's `ask` and a child's request held by the gate** (`child-question`, `child-asks`) go to
  its parent while the parent's machine is linked; when it is not, they go to the user (a request as
  if forwarded up, with the reason `its parent's machine pc is offline`). No agent allows a request,
  ever.
- The user answers them from any client: they are in the inbox of the machine the session runs on.

## The relay

A relay is needed only when two devices cannot reach each other directly (a phone on 4G and a
computer behind NAT where no hole can be punched). It passes the link's encrypted bytes and keeps
nothing: no list of messages, no queue, no content.

- **`flottille-relay`**, a Rust binary of its own (built on the chosen transport's relay, `iroh-relay`
  if iroh), published as a static binary and a container image, with examples for docker compose and
  a systemd unit. It runs on any always-on host the user has; it is not needed for the rest to work.
- **Restricted to the user's devices**: it serves only the keys of the paired list it is given (a
  list signed by a device, updated when the list changes), so that a relay on a public address
  carries only the user's traffic.
- **What it sees**: which keys talk, when, from which IP addresses, and how many bytes. Never what
  they say.
- **The relay URL** is set in Settings, Machines (`flottille link relay set <url>`), a signed user
  change sent to every paired device.
- **Whether Flottille falls back to public relays by default** (those of the transport's maintainers)
  when the user has none is **open**. Until decided: no public relay; without the user's relay, two
  devices that cannot reach each other directly say so (`pixel cannot reach pc directly and no relay
  is set`).

## The Android app

- **Native**: Kotlin and Jetpack Compose for every screen. The colours, durations and easing curves
  of the kit (`gpui-design-system`, `src/motion.rs`) are copied as Compose tokens; **everything that
  moves is animated** in the app as in the window, with Compose's own animations.
- **A Rust core, `flottille-mobile`**, exposed through UniFFI: the protocol client (the typed
  protocol of the preparation), the device key and pairing, the link, the resumption after the last
  event seen, and a view state computed in Rust. The boundary is coarse: `connect`, `request`,
  `answer`, `stop`, and a stream of event batches observed as a Kotlin `Flow`. A Kotlin module wraps
  the generated bindings; the screens never see them.
- **Built** with `cargo ndk` (`arm64-v8a`, and `x86_64` for the emulator) called from a Gradle task,
  libraries aligned on 16 KB, symbols kept apart. No AAR published elsewhere.
- **The screens, first**: pairing (QR reader), the machines and their state, the sessions of a
  machine, the session page (`design.md` §10): the conversation live, the composer with pictures,
  the ask card, the requests with Allow and Deny, the mode, Stop, the tree read only. A project's
  settings come later.
- **The link lives only while the app is in the foreground** (Android limits background sync to a few
  hours a day); a push wakes it otherwise. On the phone, segmentation offload is turned off if the
  transport has it (the iroh issue above).
- **First thing proven**, before the screens: answering Allow or Deny from a notification while the
  app is killed and the phone in Doze, which needs the link opened, the request signed and answered
  within the few seconds Android gives.

## Notifications

- **UnifiedPush**, with a distributor the user chooses (ntfy, self-hosted or not). The phone gives
  each machine its push endpoint and keys (`push.register`, a signed request); the machine sends
  **Web Push, encrypted** (RFC 8291, the machine's VAPID key): the distributor sees an opaque blob.
  The blob holds only a kind (`request`, `question`, `report`, `failure`, `dismissed`) and ids. The
  app wakes, links, reads what it needs (`inbox.list`, `approval.list`), and draws its own
  notification: a request with **Allow** and **Deny**, a question that opens its card, a report.
- **Never while the user is at another client.** Each client tells its machine its presence (the
  window: focused, last input; the app: in the foreground), and machines share it over the link
  (`presence`). A thing to notify waits 30 seconds; it is pushed only if no client of the user had an
  input in the last 2 minutes, and dropped if it was seen or answered meanwhile (the request
  answered, the session opened at a client).
- **Dismissal follows**: what is answered or seen elsewhere sends `dismissed` with its ids, and the
  phone removes its notification. A request answered at the desk never stays on the lock screen.
- One setting, Settings, Machines: **Notify the phone**: `when I am away` (the default) or `always`.
- No push service of Google's in the first version; an embedded FCM distributor is a later option if
  the app is ever distributed.

## Tunnels

- **A tunnel carries one run's port** of a machine to a client: the client listens on
  `127.0.0.1:<port>` (the same port when it is free, another otherwise), and each TCP connection to
  it becomes one stream of the link to the machine, whose daemon connects to `127.0.0.1:<the run's
  port>` there.
- **Only the ports of runs Flottille started**: the address the run reports (`run.list`) and the
  ports its processes listen on, read from the system. Any other port is refused; an agent opens no
  tunnel; opening one is a signed user request (`tunnel.open {run, port}`). It closes when the run
  ends, when the client closes it, or with the link.
- **In the window**, a run's address on another machine (its row, a link an agent showed with
  `show_link`) opens through a tunnel: the window opens it, then the system's default handler opens
  the local address. The rows of open tunnels show in the run's row, with Close.
- **On the phone**, the page opens in an in-app browser by default (the tunnel lives with the app in
  the foreground). **Open in browser** hands it to the user's browser (Firefox) and keeps the tunnel
  alive with a foreground service and its notification, closed from that notification.
- Later: names on the desktop (portless) and `*.localhost` names on the phone.

## Sharing

### Vault entries

- **Entry by entry, with consent**: Settings, Vault, an entry's **Share with…** (or `flottille vault
  share <name> --machine <m>`), a signed user request on the machine that holds it. That daemon reads
  the secret from its keyring and sends it, with the entry's metadata, in a `share` message signed
  by the device, end to end, to the other machine, which writes it into its own keyring. Never a
  file, never an agent, never the store, never an event: as `vault.md` says of every secret.
- The copy is marked `shared from pc` on that machine; it is a copy, not kept in sync: sharing again
  replaces it. A machine with no keyring refuses it, saying so.
- An entry bound to a use (`for`) keeps it; a `worklog:<project>` entry needs that project on the
  other machine.

### Connections

- A **remote** connection (an `https://` MCP server, `extensions.md`, "Connections") is shared the
  same way: its declaration, and with consent the vault entries it names. A local connection (a
  command, a `127.0.0.1` URL) is not: it is the machine's own, like its services.
- **Each machine signs in for itself**: OAuth tokens are not shared, since a refresh on one machine
  would make the other's copy fail when the server rotates its refresh tokens. The **Sign in** of a
  connection of another machine is done from a client with a browser: that machine's daemon runs the
  flow as today, and its one-shot loopback redirect is reached through a tunnel the client opens on
  the same port of its own `127.0.0.1`, so that the browser's redirect lands on the machine that
  waits for it.

## The repository

One monorepo. The crates and folders this milestone adds:

| Where | What |
|---|---|
| `crates/flottille-protocol` | Typed requests, answers and events; serde only (from the preparation). |
| `crates/flottille-client` | Connect, request, resume `events.watch {after}`; used by the command line, the window and the mobile core (from the preparation). |
| `crates/flottille-link` | Identity, keys, pairing, the paired list, the transport, signing and checking requests. |
| `crates/flottille-relay` | The relay's binary, its `Dockerfile` and a compose example. |
| `crates/flottille-mobile` | The mobile core, a UniFFI library. |
| `apps/android/` | The Android app (Gradle). |
| `site/` | The showcase site, coming. |

**CI by paths**: the Rust suite does not run for a change of `apps/android/` or `site/` alone, nor the
Android build for a change the core crates do not see; one job that always runs reports the result,
so that the required checks never wait on a job that was skipped.

## Preparation

Planned separately, and shipped as its own patch release so that it can be rolled back: the
`flottille-protocol` and `flottille-client` crates, the window reading the daemon through them, the
repository layout and CI by paths. This milestone builds on it.

## Tests

As everywhere (`AGENTS.md`, "Testing"): no real agent, no real configuration, no real keyring, no
network but loopback. Two (or three) daemons in one test, each with its own temporary `HOME` and XDG
folders and its own in-memory keyring, linked over `127.0.0.1` with no relay and no discovery; a
relay test runs the relay in the same test, on loopback. Fake agents, throwaway repositories. A
machine "off" is a daemon stopped and started again on the same folders. The Android app is tested
with its unit and Compose tests, and on an emulator in CI; the Doze proof is done once on a real
phone, by hand, and reported.

## What this does not cover

- **Full mode's limit stays**: code an agent runs on a machine acts with the user's rights there, and
  could read a key a client keeps in an unlocked keyring. A device key read that way can sign the
  user's requests. Per-device permissions, and a phone that asks for its unlock before a base answer,
  come later.
- **Configuration is not synced**: a project is registered by hand on each machine.
- **No session moves** between machines; no conversation travels.
- **Files other than pictures** do not travel yet; submodules and Git LFS objects are not sent by a
  launch.
- **iOS** and a desktop build of the app are not planned.

## Defaults taken, to confirm

- The window shows one machine at a time, with a switcher.
- The project must be registered by hand on the other machine, under the same name.
- A launch on another machine is judged by the receiving machine's gate, `child` as `led`; a child
  launch on the same machine stays unjudged.
- A session on another machine is in Full only with the user's signed switch to Full of the
  launcher's tree; otherwise Auto.
- The returned work goes to `refs/flottille/machines/<machine>/<branch>`, never onto the user's
  branch.
- Deliveries between machines are kept 30 days by the sender; the user's requests to an offline
  machine are refused.
- A pairing code lasts 10 minutes, works once; a request's signature is valid 5 minutes.
- A notification waits 30 seconds and is dropped when a client had an input in the last 2 minutes.
- No public relay by default.

## Open questions

1. **The transport**: iroh, or another (being researched apart). The design asks only for the needs
   listed in "The transport".
2. **Public relays by default** when the user has none: convenient for a phone off the LAN, but a
   third party sees who talks to whom.
3. **"Mission"** as the new name of led sessions, across the whole of Flottille or only for those on
   another machine.
4. **The pairing code typed by hand**: the code to copy is long (128 bits); a short code would need a
   password-authenticated exchange (SPAKE2). Needed only if copying is too painful.
5. **A headless server's keyring**: sharing vault entries needs one; whether Flottille offers a
   keyring of its own there, or the user installs a Secret Service.
6. **The phone's rights**: whether it may switch to Full and answer a base request without the
   phone's unlock (biometrics), before per-device permissions exist.
7. **A led session on another machine starting its own led sessions** there: today a chain is base
   (`tools.md`, "No chain without the user"); whether a session the user sent away for the night
   gets more.
8. **Syncing the project's configuration** between machines, and registering a project on first
   launch.
