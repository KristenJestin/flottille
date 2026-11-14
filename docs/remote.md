# Remote

Flottille on several machines, driven from any of them and from a phone. A machine is a place an
agent can send work to, as it sends work to a workspace: a session on the user's computer starts a
session on their server for the night, that one works alone, and its report comes back to the
computer when the computer is on again. Every client (the window, the command line, the Android
app) reads and answers the sessions of every machine of the user. It is the milestone
`v0.4 - Remote` (`docs/design.md` §12, §13).

**Status: the design of v0.4, decided with the maintainer on 2026-11-12, 2026-11-13 and 2026-11-14.** Nothing
here is built. What is decided is said as such; the defaults taken are in "Defaults taken, to
confirm"; what is open is in "Open questions". It replaces the proposal of 2026-11-03 (a relay
every device connects to, on a private network) and the review that put Tailscale and SSH at the
centre: Flottille stays generic (`AGENTS.md`), and depends on no one network and no account of a
third party. The user runs one small program of Flottille's, the hub; the data between their
devices does not go through it.

## The cases it serves

1. **Work for the night.** A session has worked all afternoon on the computer. The user tells it
   "start an agent on the server, it is alone tonight, ask nothing". The session starts a session
   on the server with a brief it writes, the branch and what is not committed go with it, the
   computer is switched off. The server works; what it would ask goes to the user's phone, since
   its pilot is off. In the morning the computer comes back, its session receives the report and
   the new commits on a branch of their own, and the user reads what was done.
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
| Machine | A computer running a Flottille daemon. |
| Client | Something the user drives Flottille with: the window, the command line, the Android app. A client runs no agent. |
| Device | What holds a key the user added to their members: a computer (its daemon and its clients), a phone. |
| Member | A device in the user's member list, with its name, its keys and its role. |
| Member list | The list of the user's members, each entry signed by the member that added it. Every device keeps a copy and checks every signature itself. |
| Role | What a member may do, signed when it is added: `runner`, `controller` or `viewer` ("Roles"). |
| Runner | A member that may run agents: a computer, a headless server. |
| Hub | `flottille-hub`, the one program the user runs for all their devices, wherever they like: it holds the member list, the invitations and the presence, and relays what cannot go direct. It sees only ciphertext of what it relays, and cannot add a member. |
| Link | Flottille's encrypted connection between two devices, peer to peer: direct when it can be, through the hub's relay otherwise. |
| Invitation | What a member opens to add a device: a token the hub gives, valid 5 minutes, once. |
| Tunnel | A TCP connection carried over the link, from a client's local port to a run's port on a machine. |

"Led session" keeps its name and its meaning (`docs/tools.md`, "Sessions an agent starts"); a led
session on another machine is the heart of case 1, and is called a led session everywhere: in the
window, the command line, the tools and the docs.

## What is decided

1. **Generic.** No dependency on Tailscale, on one network or on a service of a third party.
   Identity and transport are Flottille's own. A tailnet address is one more direct path, found or
   declared by hand like a LAN or public address.
2. **A hub, the user's own.** One `flottille-hub` per user, self-hosted: a control plane (the
   members, the invitations, the presence, the signed member list) and a relay, in one binary. No
   public hub and no public relay by default.
3. **Peer to peer, end to end.** The data between two devices goes over a link encrypted and
   authenticated by their keys, direct when it can be. The hub relays only what cannot go direct,
   and sees only ciphertext. **No queue on the hub**: what waits for a machine that is off waits on
   the machine that sends it.
4. **The hub cannot add a member.** Every addition, removal and role is signed by a member; every
   device keeps the member list and checks each signature itself. A compromised hub can refuse
   service, not let a device in.
5. **Roles, signed at pairing**: `runner`, `controller`, `viewer`, chosen by the user who accepts
   the device. A runner refuses what a role does not allow; the hub cannot change a role.
6. **A tree across machines.** `agent_launch` (`child` or `led`) takes one more optional field,
   `machine`; without it nothing changes. Reports, asks, requests and deliveries follow the tree
   over the link; when the other side is off, the sending machine keeps them and delivers them when
   it is back.
7. **Code travels over Flottille's own link, never through the forge.** A session started on
   another machine always takes a git bundle of each repository of its place, with its uncommitted
   work; the new commits come back the same way, onto a branch of their own. A repository needs no
   remote, nothing is pushed, and nothing is asked.
8. **The transport is the daemons', never the agent's.** The agent has no new tool, sees no
   network and no remote git: bare mode is unchanged. The description of `agent_launch` tells it
   what a session on another machine is.
9. **No handoff, no moving a session.** The parent writes the brief of the session it starts on the
   other machine, as it does on its own; the compaction handoff (`design.md` §5.8, 7) is unrelated.
   The user asks for it in words; there are no presets and no "solo" modes.
10. **The agent knows nothing of phones or clients.** It asks (`ask`); Flottille routes: to its
    parent or pilot when reachable, to the user wherever they are otherwise.
11. **The user's actions are signed by their device's key.** A machine may carry them, never forge
    them. A machine on its own can do only what an agent can: start a session (judged by the gate),
    deliver a report, ask, forward a request.
12. **Vault and Connections are shareable**, entry by entry, with the user's consent, end to end
    into the other machine's keyring. A remote connection's sign-in is done from a client that has a
    browser. Local connections and the services of a machine (`projects.md`, "Services") stay the
    machine's own.
13. **One system of port forwarding**: tunnels over the link, to the ports of runs Flottille started
    only.
14. **The Android app is native**: Kotlin and Jetpack Compose, over a Rust core shared with the
    desktop through UniFFI. Notifications through UnifiedPush, carrying ids only, encrypted, never
    while the user is active at another client.
15. **Versions are checked at connection**, and a mismatch is said in a sentence.
16. **One repository** for the daemon, the hub, the mobile core, the Android app and the site, with
    CI split by paths.
17. **A `controller` may invite and remove devices; a `viewer` may not.** A role is changed by
    removing the device and adding it again: there is no command that changes a role.
18. **Presence goes through the hub.** With the hub unreachable, a runner counts the user as away,
    and the phone is notified.
19. **The CLI's commands are `flottille remote …`** (`setup`, `invite`, `join`, `accept`, `members`,
    `rename`, `remove`, `address`), and the key folder is `remote/`.

## Architecture

```
                         hub (the user's own: members, invitations, presence, relay)
                        ╱        │         ╲        control: every device keeps a session to it
                       ╱         │          ╲       relay: only ciphertext, only when not direct
  phone (app + Rust core)        │           machine "server" (daemon, agents)
           ╲                     │                ╱
            ╲── link, peer to peer, end to end ──╱
                                 │
                       machine "pc" (daemon, agents)
                                 ▲ local socket
                       window, CLI of "pc"
```

- **The hub** is reached by every device at a URL the user gives once (with the hub's key, so that
  a device knows it reached its own hub). Each device keeps a control session to it: it receives
  the member list and its changes, the invitations to answer, the presence of the user's clients,
  and the addresses of the other members. The hub forwards a link's bytes between two devices that
  cannot reach each other directly; it never terminates a link.
- **Each machine's daemon holds one link endpoint** and listens on it beside its Unix socket. The
  local socket is unchanged, with its refusal of a client an agent started (`protocol.md`,
  "Transport").
- **The window and the command line reach another machine through their own machine's daemon**: a
  request names `machine`, is signed with the device key by the client, and the local daemon
  carries it over its link to that machine and carries the answers and events back. The local
  daemon cannot forge a request: it does not hold the device key.
- **The phone holds its own endpoint**, whose key is its device key, and links to each machine. It
  links only while the app is in the foreground; a push wakes it otherwise ("Notifications").
- **Two kinds of traffic on a link**, kept apart by the link's protocol name (ALPN or its
  equivalent): `client`, the user's requests (the methods of `protocol.md`, each signed by a
  device), and `machine`, the closed set of messages one machine sends another for its agents
  ("What a machine may send"). A message of one kind on the other is refused.
- **The protocol is the daemon's** (`protocol.md`), unchanged in its framing: the same requests,
  answers and events, numbered, resumed with `events.watch {after}`. It was written for this.
- **When the hub is down**, links already open go on, and devices that know each other's addresses
  link directly; what needs the hub (an invitation, a change of the member list, the relay,
  presence) waits for it, and the clients say so (`the hub is unreachable since 22:14`).

## The transport

What Flottille needs of it: a device's identity is its public key; the channel between two keys is
encrypted end to end; it connects directly over every path that exists (same LAN, a public address,
a hole punched through NAT, a tailnet address) and falls back to a relay that sees only ciphertext;
the relay can be self-hosted, inside the hub; it runs on Linux, macOS later, and Android; it
multiplexes streams (requests, events, a git transfer and tunnels at once on one link).

**The candidate is iroh**, to be proved by a spike before anything is built on it (#839). iroh is
Rust and QUIC; an endpoint's id is its Ed25519 public key; it punches holes; its relay
(`iroh-relay`) can be embedded in the hub; version 1.0 has official Kotlin bindings and is used on
Android by Delta Chat and by an app that drives coding agents from a phone. The alternatives weighed:

| | Identity = key | End to end | Direct + NAT | Own relay | Android | Notes |
|---|---|---|---|---|---|---|
| iroh | yes | yes (QUIC TLS on raw keys) | yes | `iroh-relay` | yes, in production | a dependency of weight (QUIC stack, DNS, port mapping); its default discovery uses n0's servers and is turned off here (the hub gives the addresses); a known Android issue on 4G with segmentation offload, worked around by turning it off on the phone |
| rust-libp2p (QUIC, hole punching, circuit relay) | yes | yes (Noise/TLS) | yes | yes | possible, less proven | larger and more general (DHT, pubsub), little gain for a few devices; the second choice if the spike fails |
| QUIC (quinn) + Flottille's own NAT traversal and relay | to build | to build | to build | to build | yes | what iroh already is, without its years of fixes |
| WebSocket over TLS to a relay (the 2026-11-03 proposal) | to build | needs a second layer (Noise) | no | yes | yes | every byte through the relay, no direct path |
| WireGuard in user space | yes | yes | partly | no | conflicts with the system VPN slot | needs a network stack of its own in the process |
| Tailscale / tsnet | Tailscale's | yes | yes | DERP | yes | a dependency on one network and its coordination server: rejected (decision 1) |

The rest of this document is written against these needs, not one library: "an endpoint", "an
address", "a relay", "a stream". The crate that holds it, `flottille-link`, hides the library from
the daemon, the clients, the hub and the mobile core, so that a later change of transport changes
one crate. Whatever is chosen is a dependency of weight, said in its pull request.

**Paths.** A device's addresses are what the transport finds (its interfaces, a public address seen
from outside), plus the addresses the user declares by hand (`flottille remote address add
100.64.0.7`, Settings, Machines): a tailnet address, a DNS name, a forwarded port. Each device tells
the hub its addresses whenever they change, and the hub tells the other members; members linked to
each other tell each other too, so that two devices that have linked once can link again without the
hub. No discovery service of a third party is used.

## Identity and keys

- **The machine key** is made by the daemon at its first start with remote on:
  `$XDG_STATE_HOME/flottille/remote/machine.key`, mode 600, the folder 700. The daemon alone reads
  it. It signs what a machine may send ("What a machine may send"), never a user's action.
- **The device key of a machine's clients** (the window and the command line of that user session)
  is in the session's keyring (`vault.md`, "Where the secrets are"), an item `Flottille: device`; on
  a machine with no Secret Service (a headless server), `$XDG_STATE_HOME/flottille/remote/device.key`,
  mode 600, read by the clients only. The daemon never reads it. It signs the user's actions, the
  member list's changes included.
- **The phone's device key** is made by the app, kept encrypted under a key of the Android Keystore,
  and never leaves the phone.
- **The hub's key** is made at its first start, in its own state folder. Devices pin it when they
  join: a hub that answers with another key is refused (`the hub at hub.example.net has another key
  than when this device joined`).
- **The gate refuses `remote/`** in every mode, read or write, as a login's place (`design.md` §7),
  where reading Flottille's own folders is otherwise base. What this does not cover is Full mode's
  limit: code an agent runs acts with the user's rights. So a key read by an agent's code gives what
  that key may do, and no more: a machine key gives only what a machine may send (below), never the
  user's methods.
- No private key is ever sent, logged, shown or returned by a method.

## The hub

`flottille-hub`, a Rust binary of its own, run by the user on any host that is always on (a small
server, a home server, a VPS), published as a static binary and a container image, with examples for
docker compose and a systemd unit. One hub serves one user.

**What it holds:**

- **The member list**, as the members signed it: it stores it and sends it, with every signed
  change, to every member. It checks each change's signature before storing it, as every device
  does after it; it cannot make one.
- **The invitations**: a token per invitation, its expiry, the inviting member, and the join request
  that answers it, until it is accepted, refused or expired.
- **The presence**: which of the user's clients is active ("Notifications"), as each client tells
  it.
- **The addresses** of the members, as they tell it, to help two devices find each other.

**What it relays**: the link's encrypted bytes between two members that cannot reach each other
directly (a phone on 4G and a computer behind a NAT where no hole can be punched), and nothing else.
It serves only the keys of the member list: a hub on a public address carries only the user's
traffic. It keeps nothing of what it relays: no message, no queue, no content.

**What it sees**: which keys talk, when, from which IP addresses, and how many bytes; the member
list (names, keys, roles, never a secret); the presence. Never what the devices say to each other.

**Its configuration**: a listen address, TLS (a certificate given, ACME, or plain behind a reverse
proxy), a state folder. The hub URL is given to every device at join, in the invitation.

### The first member

At its first start, with an empty member list, the hub prints a **setup code** in its log, once:
`setup code: 7KQ2-M4XP-9TRW-H3CD, valid until a first member joins`. `flottille-hub setup-code` prints
it again while no member exists. The first device (usually the computer) joins with it:
`flottille remote setup <hub URL> <code>`, or Settings, Machines, **Set up a hub** in the window. It
becomes the first member, a `runner`, its entry signed by itself; it is the **root** of the list, and
every device that joins later pins it.

**Recovery**: `flottille-hub setup-code --reset`, on the hub's machine only, empties the member list
and prints a new setup code. Every device then refuses the new list (its root is not the one they
pinned) and says so (`the hub's member list was reset: set this device up again`). Access to the
hub's machine is what recovery needs, and all it gives.

## Pairing

A device is added by a member, through the hub, the user checking on both screens that they talk to
each other.

1. **An existing member asks the hub for an invitation**: Settings, Machines, **Add a device** in the
   window, `flottille remote invite`, or the phone's **Add a device**. It is a user action, signed by
   that member's device key. The hub answers with a token, valid **5 minutes, once**. The member
   shows a QR and the same as a code to copy, holding the hub URL with the hub's key, the token, and
   the inviting member's key fingerprint.
2. **The new device reads it** (the phone scans the QR; a computer is given the code), makes its key,
   and connects **to the hub**, not to the inviting member, with the token and its new public key.
3. **The hub holds the request** and notifies the inviting member. Both screens show the same **four
   verification words**, drawn from the new device's key, the inviting member's key and the token
   (`maple orbit violet canyon`).
4. **The user accepts on the existing member**, after checking the words, and chooses its name and
   what it may do: **This device can: Run agents / Control only / View only** (`runner`,
   `controller`, `viewer`; "Roles"), in the window's dialog, the phone's, or `flottille remote
   accept <request> --name <name> --role <role>`, which prints the words first. The member signs "add key K, named N, with role R" with its own
   device key and sends it to the hub. **Refuse** ends the request; so does the token's expiry.
5. **The hub checks the signature**, adds the entry to the list, and sends the list to every member.
   **Every device checks the signatures itself**, back to the root it pinned: an entry no member
   signed is refused, whoever sent it. The new device checks that the key of the QR's fingerprint
   signed its own entry. **Every connected client announces it**: `New device: pixel (control only),
   added at 18:42 from pc`, with Remove beside it, so that an addition behind the user's back is
   seen at once.

**A headless machine** joins with `flottille remote join <code>`, the code coming from an
invitation opened on a human device: it prints the four words, and the user accepts on the device
that invited it, with the role `runner` chosen there.

**Removing** (Settings, Machines, the row's Remove, `flottille remote remove <name>`) is the same
kind of signed change, by any member that may add one: every device that hears it removes the
member and cuts its links at once; one that was off hears it from the hub when it connects again,
before anything else is exchanged. A removed key cannot join again. **Renaming** is the same kind of
signed change. A device that wants to leave removes itself the same way. A role is changed by
removing the device and adding it again.

A device refuses a link from a key that is not in its list, and a list change not signed by a
member.

## Roles

The user who accepts a device chooses what it may do; the role is in the signed entry, and the hub
cannot change it.

| Role | For | May |
|---|---|---|
| `runner` | a computer, a headless server | run agents; and, from its own clients, everything a `controller` may |
| `controller` | a phone, a laptop without agents | drive without running: read, send messages, answer questions and permission requests, start and stop sessions on a runner, switch the mode, open tunnels, invite and remove devices. The default for a phone |
| `viewer` | a device the user lends, or trusts less | read only: the machines, the sessions, the conversations, the requests, never an answer |

- **A runner refuses a request a role does not allow**, before anything else, saying which role
  allows it (`refused: pixel may only view: a controller or a runner may answer a request`). Every
  method of `protocol.md` is in one of two classes, `read` or `drive`; a `viewer` may only `read`.
- **A runner advertises live to its peers**, over the link: its projects, the agent CLIs installed
  and signed in (their login files checked for presence only, never read: `AGENTS.md`), and its
  free slots (the project caps minus what runs). It sends it again when it changes.
- **Starting a session on a machine** (from a client, or by an agent: "A tree across machines")
  offers only the runners that have that project, that agent signed in, and a free slot. A
  `controller` or a `viewer` runs no agent: a launch aimed at one is refused, naming it.

## The version

When two devices link, or a device connects to the hub, each says its Flottille version and the
protocol versions it speaks (a number, `protocol` in `daemon.status` too, raised by any change of a
method, an answer or an event that an older client would misread). They use the highest one both
speak; with none in common the link is refused on both sides with a sentence that says which to
update: `server runs Flottille 0.4.3 (protocol 9), this machine 0.4.1 (protocol 8): update one of
them (flottille update)`. The window and the app show the machine as **needs an update**, never as
offline. At first each side speaks one version; the list lets the Android app, published apart,
speak two.

## Requests from another machine's clients

- A request for another machine carries `machine` and a signature: `{"id", "machine", "signed":
  {"device", "nonce", "at", "method", "params"}, "sig"}`. The client crate makes it; the local
  daemon carries it. The receiving daemon checks the device is a member, that its role allows the
  method ("Roles"), the signature, the time (within 5 minutes of its clock) and that the nonce was
  not seen in that window, then serves the request as one from a local client, the device known:
  its log and its events name it.
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
then each runner with its state: online, offline since, needs an update); merging the lists of
several machines in one sidebar is left for later. A led session on another machine appears under
its pilot with the machine's name, and opening it switches to that machine.

## What a machine may send

On the `machine` side of a link, a machine sends only these messages, signed with its machine key,
each naming the agent of its own that it acts for, and the receiving machine checks each against
what it recorded (that this agent of that machine started that session, or is that child's parent):

| Message | What it does on the receiving machine |
|---|---|
| `launch` | Starts a session or a child for an agent of the sender (`agent_launch` with `machine`), judged by the receiving machine's gate ("A tree across machines"). |
| `git` | Sends a bundle of a place's commits and its uncommitted work, or of a session's new commits on its way back ("What travels"). |
| `deliver` | A delivery for an agent of the receiver: a report, a question, an answer, a request forwarded, `session-turn-ended`, a child's news, as `design.md` §5.2–5.3 make them. |
| `call` | One of the tree's tools aimed at a session it started: `agent_send`, `agent_status`, `agent_stop`, `session_send`, `session_read`, `approval_answer`. Judged as on one machine. |
| `capabilities` | What a runner offers: its projects, its agents signed in, its free slots ("Roles"). |
| `share` | A vault entry or a connection the user shared, signed by their device ("Sharing"). |

Nothing else: no user method, no `approval.approve`, no `agent.mode`, no `agent.send` as the user, no
change of the member list (those go to the hub, signed by a device). A machine that forwards a
user's request does so on the `client` side, with the device's signature.

## A tree across machines

### Starting a session on another machine

`agent_launch {kind: "child" | "led", machine?, title, brief, model?, role?, …}`. **No other tool**:
the agent has the same `agent_launch`, with one more optional field, `machine`, the name of a
runner; left out, this machine, and nothing changes. `handed` takes it too.

**The transport is the daemons', never the agent's.** The agent gets no network, no remote git and
no tool to move code: bare mode is unchanged. Its daemon sends the code and brings the commits back.
**The tool's description says it to the agent**, in these words or close to them: *`machine` starts
the agent on another of the user's machines. It works on a copy of your place there: your branch and
your uncommitted changes are sent with it. Its commits come back to you on a branch of their own,
`refs/flottille/machines/<machine>/<branch>`, which you review and merge as you would a local
child's work; nothing moves your branch.*

- **Who**: as today. `led` and `handed` from a root only; `child` from any agent that launches
  children. A led session on the computer may start a child on the server; a led session on the
  server is a root there, and may start its own children there.
- **The machine must be a runner, and online**: otherwise the call is refused at once, naming since
  when it is offline, or that it runs no agent. It must have the project, the caller's provider
  signed in and a free slot ("Roles"): otherwise refused, naming the runners that would take it, if
  any (`refused: server has no slot free for project app (2 of 2 run); laptop would take it`).
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

**Always, and only over Flottille's link.** A launch on another machine sends the code of its place
every time, by itself: nothing is asked of the agent or the user, and a repository with no remote,
or one the user does not want to push, travels the same. Nothing is pushed to a repository's remote,
nothing goes through a forge; a forge is never needed. Git is the user's own `git`, spawned without
a shell.

For each repository of the caller's place, before the session starts:

- **The uncommitted work** becomes one commit made from a temporary index (`GIT_INDEX_FILE`): the
  changed and untracked files that are not ignored, on top of `HEAD`. The user's branch, index and
  files are not touched; the commit is held by `refs/flottille/outgoing/<launch>`.
- **A git bundle** of the starting branch and that commit goes over the link. When the receiving
  machine already has the repository, it says which commits it has (its branches,
  `refs/flottille/machines/*`), and the bundle holds only the missing ones (a bundle with those as
  prerequisites); otherwise it holds the whole history of the branch. The receiver verifies the
  bundle (`git bundle verify`) and fetches it.
- **Ignored files do not travel**: a `.env` is the machine's own, taken by its setup's `copy` steps
  as today. A setup that misses one fails as it does today, saying which.
- On the receiving machine the workspace is created from those commits, on the same branch name
  (with `-2`, `-3` when that name is taken there), prepared by the project's setup, and the
  uncommitted commit is laid on it as uncommitted changes. A repository the project has there but
  never received is cloned from the bundle, never from its remote.

**Coming back.** When the remote session has committed and reports (`child-report`, or a led
session's `session-turn-ended` that reports), its daemon sends a bundle of **only the new commits**,
those past what the caller's machine sent or already has; the caller's machine verifies it and
fetches it into `refs/flottille/machines/<machine>/<branch>`, and the delivery names that ref.
Nothing moves the user's branch: the agent or the user reviews and merges it (a `git merge` run,
judged as any), as a local child's work. The pilot of a led session on another machine stops it
and removes its workspace as today, through `call`.

### Deliveries across machines

- **Stored before sent**, as every delivery (`design.md` §5.2): a delivery for an agent of another
  machine waits in the sending machine's store until that machine acknowledged it, and is sent again
  when the link comes back, in order, once (its id is idempotent). A report written at 3:00 on the
  server reaches the computer at 8:00 when the computer links again. Kept 30 days, then dropped with
  an entry in the inbox of the sending machine. Never on the hub.
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
- The user answers them from any client whose role allows it: they are in the inbox of the machine
  the session runs on.

## The Android app

- **Native**: Kotlin and Jetpack Compose for every screen. The colours, durations and easing curves
  of the kit (`gpui-design-system`, `src/motion.rs`) are copied as Compose tokens; **everything that
  moves is animated** in the app as in the window, with Compose's own animations.
- **A Rust core, `flottille-mobile`**, exposed through UniFFI: the protocol client (the typed
  protocol of the preparation), the device key, joining through the hub, the member list and its
  checks, the link, the resumption after the last event seen, and a view state computed in Rust.
  The boundary is coarse: `connect`, `request`, `answer`, `stop`, and a stream of event batches
  observed as a Kotlin `Flow`. A Kotlin module wraps the generated bindings; the screens never see
  them.
- **Built** with `cargo ndk` (`arm64-v8a`, and `x86_64` for the emulator) called from a Gradle task,
  libraries aligned on 16 KB, symbols kept apart. No AAR published elsewhere.
- **The screens, first**: joining (the QR reader, the four words), the machines and their state, the
  sessions of a machine, the session page (`design.md` §10): the conversation live, the composer with
  pictures, the ask card, the requests with Allow and Deny, the mode, Stop, the tree read only. A
  phone is a `controller` by default; as a `viewer`, the composer, the answers and the controls are
  not drawn. A project's settings come later.
- **The navigation, decided**: a bottom bar with three destinations. **Needs you**: each request
  and question as a card answered in place (Allow and Deny on the card, the whole call in sight).
  **Sessions**: the window's sidebar, each machine, its projects folded or not, their sessions in one
  list ordered by last exchange, led sessions indented under their pilot; the state is the row's
  mark, so rows never jump between groups. **Machines**: their state, their agents, their slots.
  The app opens on Needs you; a notification opens its session.
- **The link lives only while the app is in the foreground** (Android limits background sync to a few
  hours a day); a push wakes it otherwise. On the phone, segmentation offload is turned off if the
  transport has it (the iroh issue above).
- **First thing proven**, before the screens: answering Allow or Deny from a notification while the
  app is killed and the phone in Doze, which needs the link opened, the request signed and answered
  within the few seconds Android gives.

## Notifications

- **UnifiedPush**, with a distributor the user chooses (ntfy, self-hosted or not). The phone gives
  each runner its push endpoint and keys (`push.register`, a signed request); the runner sends
  **Web Push, encrypted** (RFC 8291, the machine's VAPID key): the distributor sees an opaque blob.
  The blob holds only a kind (`request`, `question`, `report`, `failure`, `dismissed`) and ids. The
  app wakes, links, reads what it needs (`inbox.list`, `approval.list`), and draws its own
  notification: a request with **Allow** and **Deny** (not on a `viewer`), a question that opens its
  card, a report.
- **Never while the user is at another client.** Each client tells the hub its presence (the
  window: focused, last input; the app: in the foreground), and the hub tells the runners. A thing
  to notify waits 30 seconds; it is pushed only if no client of the user had an input in the last 2
  minutes, and dropped if it was seen or answered meanwhile (the request answered, the session
  opened at a client). When the hub is unreachable, the runner counts the user as away and pushes.
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
  tunnel; opening one is a signed user request (`tunnel.open {run, port}`) of a `controller` or a
  runner's client. It closes when the run ends, when the client closes it, or with the link.
- **In the window**, a run's address on another machine (its row, a link an agent showed with
  `show_link`) opens through a tunnel: the window opens it, then the system's default handler opens
  the local address. The rows of open tunnels show in the run's row, with Close.
- **On the phone**, the page opens in an in-app browser by default (the tunnel lives with the app in
  the foreground). **Open in browser** hands it to the user's browser and keeps the tunnel alive
  with a foreground service and its notification, closed from that notification.
- Later: names on the desktop (portless) and `*.localhost` names on the phone.

## Sharing

### Vault entries

- **Entry by entry, with consent**: Settings, Vault, an entry's **Share with…** (or `flottille vault
  share <name> --machine <m>`), a signed user request on the machine that holds it. That daemon reads
  the secret from its keyring and sends it, with the entry's metadata, in a `share` message signed
  by the device, end to end, to the other runner, which writes it into its own keyring. Never a
  file, never an agent, never the store, never an event, never the hub: as `vault.md` says of every
  secret.
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
| `crates/flottille-link` | Identity, keys, the member list and its checks, joining, the transport, signing and checking requests. |
| `crates/flottille-hub` | The hub's binary (control plane and relay), its `Dockerfile` and a compose example. |
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
network but loopback. A hub and two (or three) daemons in one test, each with its own temporary
`HOME` and XDG folders and its own in-memory keyring, on `127.0.0.1`, with no discovery; the relay
is tested by blocking the direct path in the test. Fake agents, throwaway repositories. A machine
"off" is a daemon stopped and started again on the same folders; a hub "down" the same. The Android
app is tested with its unit and Compose tests, and on an emulator in CI; the Doze proof is done once
on a real phone, by hand, and reported.

## What this does not cover

- **Full mode's limit stays**: code an agent runs on a machine acts with the user's rights there, and
  could read a key a client keeps in an unlocked keyring. A device key read that way can sign what
  its role allows. Roles bound what a device may do; a phone that asks for its unlock before a base
  answer comes later.
- **Configuration is not synced**: a project is registered by hand on each machine.
- **No session moves** between machines; no conversation travels.
- **Files other than pictures** do not travel yet; submodules and Git LFS objects are not sent by a
  launch.
- **One hub per user**; several users, or a hub shared between people, are not planned.
- **iOS** and a desktop build of the app are not planned.

## Defaults taken, to confirm

- The window shows one machine at a time, with a switcher.
- The project must be registered by hand on the other machine, under the same name.
- A launch on another machine is judged by the receiving machine's gate, `child` as `led`; a child
  launch on the same machine stays unjudged.
- A session on another machine is in Full only with the user's signed switch to Full of the
  launcher's tree; otherwise Auto.
- The returned commits go to `refs/flottille/machines/<machine>/<branch>`, never onto the user's
  branch.
- Deliveries between machines are kept 30 days by the sender; the user's requests to an offline
  machine are refused.
- An invitation lasts 5 minutes, works once; a request's signature is valid 5 minutes.
- The verification words are four, from a fixed list of 2048 words.
- The first member, the one that used the setup code, is the root every device pins; a reset of the
  hub makes every device set up again.
- Every method is `read` or `drive`; a `controller` may every `drive` method on a runner, a
  `viewer` only `read`.
- A launch an agent aims at a runner that cannot take it is refused, naming the runners that could;
  the agent's tool description lists no machine.
- A notification waits 30 seconds and is dropped when a client had an input in the last 2 minutes.

## Open questions

1. **The transport**: the hub and its relay are decided; iroh is the candidate, to be proved by the
   spike (#839), rust-libp2p the second choice. The design asks only for the needs listed in "The
   transport".
2. **A headless server's keyring**: sharing vault entries needs one; whether Flottille offers a
   keyring of its own there, or the user installs a Secret Service.
3. **A controller's rights**: whether a `controller` may switch a tree to Full, or answer a base
   request, without the phone's unlock (biometrics), or whether those need a runner's client.
4. **A led session on another machine starting its own led sessions** there: today a chain is base
   (`tools.md`, "No chain without the user"); whether a session the user sent away for the night
   gets more.
5. **Syncing the project's configuration** between machines, and registering a project on first
   launch.
