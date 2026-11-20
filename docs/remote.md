# Remote

Flottille on several machines, driven from any of them and from a phone. A machine is a place an
agent can send work to, as it sends work to a workspace: a session on the user's computer starts a
session on their server for the night, that one works alone, and its report comes back to the
computer when the computer is on again. Every client (the window, the command line, the Android
app) reads and answers the sessions of every machine of the user. It is the milestone
`v0.4 - Remote` (`docs/design.md` §12, §13).

**Status: the design of v0.4, decided with the maintainer on 2026-11-12, 2026-11-13, 2026-11-14 and 2026-11-15.** Nothing
here is built but the keys, and the member list, the invitation code and the four words as data
and their checks, and the link between two machines with its version check (`crates/flottille-link`,
`crates/flottille/src/link.rs`), and the hub with its rules, served on the network, the control
session every machine keeps to it and the first member's setup (`crates/flottille-hub`,
`crates/flottille-link/src/control.rs`, `crates/flottille/src/hub_session.rs`), and pairing
through the hub: invitations, joining, the four words, adding, renaming and removing members
from the command line (#812), and the user's requests to another machine, signed by the client,
carried by its daemon and checked by the machine they are for (`--machine`, #813), a place's
code sent to another machine over the link and its new commits brought back (#815), the roles
bounding what each device may do, with what each runner offers its peers (#862), `agent_launch`
starting a child or a led or handed session on another machine, judged by that machine's gate,
its code sent by itself (#816), the deliveries and calls between a session and the agent that
started it on another machine, kept for a machine that is off, with the commits coming back (#817),
where a question and a held request go while the parent's machine is off (#818), the tunnels
to the ports of a run's server (#829), a vault entry shared with another machine (#832), a
connection shared the same way (#833) and the window's machines, in Settings, General, Machines
(#814). What is decided is said as such; the defaults taken are in "Defaults taken, to confirm";
what is open
is in "Open questions". It replaces the proposal of 2026-11-03 (a relay
every device connects to, on a private network) and the review that put Tailscale and SSH at the
centre: Flottille stays generic (`AGENTS.md`), and depends on no one network and no account of a
third party. The user runs one small program of Flottille's, the hub; the data between their
devices does not go through it. Flottille also handles no network channel: the only thing it asks
is that two devices can reach each other ("The transport").

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
| Hub | `flottille-hub`, the one program the user runs for all their devices, wherever they like: it holds the member list, the invitations, the presence and the members' addresses. A control plane only: it never carries a link's bytes, and it cannot add a member. |
| Link | Flottille's encrypted connection between two devices, peer to peer and always direct: one dials an address the other listens on. |
| Invitation | What a member opens to add a device: a token the hub gives, valid 5 minutes, once. |
| Tunnel | A TCP connection carried over the link, from a client's local port to a run's port on a machine. |

"Led session" keeps its name and its meaning (`docs/tools.md`, "Sessions an agent starts"); a led
session on another machine is the heart of case 1, and is called a led session everywhere: in the
window, the command line, the tools and the docs.

## What is decided

1. **Generic, and no network channel of its own.** No dependency on Tailscale, on one network or
   on a service of a third party. Identity and the link are Flottille's own; the network under
   them is the user's. Whether they have a LAN, a VPN, Tailscale, a VPS or a public address is not
   Flottille's concern: the one thing it asks is that two devices can reach each other. A tailnet
   address is one more address, told by the hub or given by the user like a LAN or public one.
2. **A hub, the user's own.** One `flottille-hub` per user, self-hosted, a control plane only: the
   members, the invitations, the presence, the addresses, the signed member list. **No relay**
   anywhere (not in the hub, not hosted, not public) and no public hub by default.
3. **Peer to peer, direct, end to end.** The data between two devices goes over a link, encrypted
   and authenticated by their keys, from one device to an address the other listens on. **The hub
   never carries a link's bytes**, and Flottille does no NAT traversal, no hole punching, no port
   mapping and runs no address discovery service. Two devices that cannot reach each other say so
   plainly ("Paths"); the fix is the user's network. **No queue on the hub**: what waits for a
   machine that is off waits on the machine that sends it.
4. **The hub cannot add a member.** Every addition, removal and role is signed by a member; every
   device keeps the member list and checks each signature itself. A compromised hub can refuse
   service, not let a device in that no member added; what it can still do by withholding is
   said in "The hub", "What a compromised hub can still do".
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
                         hub (the user's own: members, invitations, presence, addresses)
                        ╱        │         ╲        control only: every device keeps a session to it
                       ╱         │          ╲       it never carries a link's bytes
  phone (app + Rust core)        │           machine "server" (daemon, agents)
           ╲                     │                ╱
            ╲─ link: direct, end to end ─╱
                                 │
                       machine "pc" (daemon, agents)
                                 ▲ local socket
                       window, CLI of "pc"
```

- **The hub** is reached by every device at a URL the user gives once (with the hub's key, so that
  a device knows it reached its own hub). Each device keeps a control session to it: it receives
  the member list and its changes, the invitations to answer, the presence of the user's clients,
  and the addresses of the other members. The hub carries no link's bytes: it neither forwards nor
  terminates a link.
- **Each machine's daemon holds one link endpoint** and listens on it beside its Unix socket, on the
  addresses it is configured with ("Paths"). The local socket is unchanged, with its refusal of a client an agent started (`protocol.md`,
  "Transport").
- **The window and the command line reach another machine through their own machine's daemon**: a
  request names `machine`, is signed with the device key by the client, and the local daemon
  carries it over its link to that machine and carries the answers and events back. The local
  daemon cannot forge a request: it does not hold the device key.
- **The phone holds its own endpoint**, whose key is its device key, and links to each machine. It
  links only while the app is in the foreground; a push wakes it otherwise ("Notifications"). A
  phone on a mobile network has no address a machine can dial: it dials, and what a machine sends
  it travels over the link the phone opened.
- **Two kinds of traffic on a link**, kept apart by the link's protocol name (ALPN or its
  equivalent): `client`, the user's requests (the methods of `protocol.md`, each signed by a
  device), and `machine`, the closed set of messages one machine sends another for its agents
  ("What a machine may send"). A message of one kind on the other is refused.
- **The protocol is the daemon's** (`protocol.md`), unchanged in its framing: the same requests,
  answers and events, numbered, resumed with `events.watch {after}`. It was written for this.
- **When the hub is down**, links already open go on, and devices that know each other's addresses
  link directly; what needs the hub (an invitation, a change of the member list, the
  addresses of a device that moved, presence) waits for it, and the clients say so (`the hub is
  unreachable since 22:14`).

## The transport

Flottille handles no network channel. What it needs of the transport is stated here, and the
library that gives it, chosen in #811, is hidden in `flottille-link`:

- **a device's identity is its public key**: a link is to a key, and a device that does not present
  the key it was dialed for is refused;
- **encrypted end to end and authenticated by the devices' keys**, on both sides, with nothing
  between the two devices that can read or change it;
- **direct to a known address**: a device listens on the address it is configured with (none
  until the user sets one: it then only dials) and a peer dials an address it knows for that key, told by the
  hub or given by the user ("Paths"); nothing else is tried: no relay, no NAT traversal, no hole
  punching, no port mapping (UPnP, NAT-PMP, PCP), no address discovery service, and no server of a
  third party in the path;
- **streams**: requests, events, a git transfer and tunnels at once on one link, kept apart;
- **it runs** on Linux, macOS later, and Android.

**How it came to this.** iroh was first the candidate, for its NAT traversal and its relay; the
maintainer decided on 2026-11-15 that both are out of scope (decision 1), which also ended the
spike #839. Tailscale and tsnet stay rejected (decision 1). The rest of this document is written
against the needs above, not one library: "an endpoint", "an address", "a stream".

**The transport built** (#811) is QUIC (`quinn`), whose TLS 1.3 runs on the devices' own Ed25519
keys sent as raw public keys (RFC 7250, `rustls`): the dialler checks the other side holds the very
key it dialled, the listener reads the key the dialler proved and refuses it unless trusted, with a
sentence (`refused: this machine's key is not one of the user's`). Both libraries were in the
lock already, their default features off; they are dependencies of weight, said in #811's pull
request. iroh, with its relays and discovery off, was not kept: its NAT traversal cannot be turned
off (its QUIC settings enforce a minimum of traversal addresses, and its endpoint runs the hole
punching), and its relay, DNS and network-watching crates come with it whatever is turned off.
Only `crates/flottille-link/src/transport.rs` names the libraries: the daemon, the clients, the hub
and the mobile core see Flottille's types (an endpoint, a link, the two kinds of traffic), so that a
change of transport changes that file.

On a link, each message goes on a QUIC stream of its own, one JSON frame one way and its answer
(`{"ok": …}` or `{"refused": "…"}`) the other; the first stream is the version handshake ("The
version"). The protocol names are `flottille-machine` and `flottille-client`; a message of the
other kind is refused by the receiver, the link going on (`a machine's message is not taken on a
client link`).

**Paths.** A device listens on the address it is configured with, in the daemon's remote
configuration (`[remote] listen`; none until the user sets one, and a device that listens nowhere
still dials the others). It tells the hub those addresses
whenever they change, and the hub tells the other members; the user may also give an address by
hand (`flottille remote address add 100.64.0.7`, Settings, General, Machines): a tailnet address, a DNS name,
a forwarded port, a public address. Members linked to each other tell each other too, so that two
devices that have linked once can link again without the hub. A peer dials the addresses it knows
for a key, whoever told it; the hub's list is a help, not a path.

**Two devices that cannot reach each other say so plainly**, on both sides and in the clients: which
address was tried, from where, and how it failed (`server is unreachable: tried 192.0.2.10:7421
from pc, no answer in 10 s`). Flottille does not work around it: the fix is the user's network (a
firewall rule, a forwarded port, a VPN, another address). A device no one can dial (a phone on a
mobile network) still links to every device it can dial, and what they send it travels over that
link.

## Identity and keys

- **Remote is off** until the user turns it on in the daemon's configuration,
  `$XDG_CONFIG_HOME/flottille/daemon.toml`:

  ```toml
  [remote]
  on = true
  # Where other devices reach this one: nowhere until it is set. An example, not a default.
  # listen = "0.0.0.0:7447"
  ```

  Off, nothing of it is made or read.
- **Keys are Ed25519** (`crates/flottille-link`): a device's identity is its public key, shown as
  its fingerprint, the first 8 bytes of the key's SHA-256 in hexadecimal by groups of four
  (`3f2a-91bc-0d4e-77a1`). A private key is made from the operating system's random generator,
  zeroized when dropped, and its `Debug` prints the fingerprint only.
- **The machine key** is made by the daemon at its first start with remote on:
  `$XDG_STATE_HOME/flottille/remote/machine.key`, mode 600, the folder 700. The daemon alone reads
  it. It signs what a machine may send ("What a machine may send"), never a user's action. It is
  read back at every start; a link, a file of another user, or one others may read or write is
  refused, left as it is, and so is the daemon's start
  (`… machine.key is readable by others (mode 644): a private key is yours only; make it so with chmod 600 …`).
- **The device key of a machine's clients** (the window and the command line of that user session)
  is in the session's keyring (`vault.md`, "Where the secrets are"), an item `Flottille: device`
  with the attributes `application = flottille` and `flottille-key = device` (never
  `flottille-entry`, a vault entry's: one may be named `device`); on
  a machine with no Secret Service (a headless server), `$XDG_STATE_HOME/flottille/remote/device.key`,
  mode 600, read by the clients only. The daemon never reads it. It signs the user's actions, the
  member list's changes included. It is kept once `flottille remote setup` or `join` succeeded with
  it, never before ("What a compromised hub can still do"). Until then it is **pending**, with the
  same care: an item `Flottille: device, pending` (`flottille-key = device-pending`), or
  `device.key.pending` with no Secret Service, written before the hub sees the key, so that a
  command gone before the end (Ctrl+C, a closed terminal, a lost SSH session, a locked keyring)
  loses no key the hub took. The next setup or join looks at a pending key first, once a setup or
  a join the daemon still runs for it ended: it is kept when the list of the hub the machine pinned
  holds it as a member, and removed otherwise, never sent again; also for a setup that replaces
  the hub, whose machine is pinned to the old one meanwhile. When the daemon holds a join
  accepted for it, the join goes on with that join's answer, not with the code typed (the command
  says so), and the key is kept only once the hub took the machine's declaration. A key is kept
  only if no other is: one another client kept first is said, and the pending one stays. A declaration
  that failed because its session ended lets the join go: the next command removes the pending key
  (the member it names, with no machine, is one the inviter removes). The
  keyring holding two items of the kept device key (two clients that kept one at the same moment)
  is refused, never one of them taken: which is the member's cannot be told. Two pending items are
  both removed, never sent again: at most a member with no key is left, which a member removes.
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
docker compose and a systemd unit (`docs/releases.md`, "The hub": what is published, how to run it;
`crates/flottille-hub/compose.example.yml`, `crates/flottille-hub/flottille-hub.service`). One hub
serves one user.

**What it holds:**

- **The member list**, as the members signed it: it stores it and sends it, with every signed
  change, to every member. It checks each change's signature before storing it, as every device
  does after it; it cannot make one.
- **The invitations**: a token per invitation, its expiry, the inviting member, and the join request
  that answers it, until it is accepted, refused or expired.
- **The presence**: which of the user's clients is active ("Notifications"), as each client tells
  it.
- **The addresses** of the members, as they tell it, to help two devices find each other.

**What it never does**: carry a link's bytes. It is a control plane: devices link to each other
directly ("The transport"), so it keeps no message, no queue and no content of what they say, and
there is no relay in it, hosted or public.

**What it sees**: which keys connect to it, when, from which IP addresses; the member list (names,
keys, roles, never a secret); the presence; the addresses members tell it. It does not see which
devices link to each other, nor what they say.

**Its configuration**: a listen address (a device must be able to reach it, like any other), TLS (a certificate given, ACME, or plain behind a reverse
proxy), a state folder. The hub URL is given to every device at join, in the invitation. Built
(#820): `--listen <address>` (`127.0.0.1:7448` by default: loopback, behind a reverse proxy on the
same host), `--tls-cert <pem> --tls-key <pem>` for TLS on a certificate given, plain otherwise,
and `--state <dir>`. Another address than loopback takes TLS, or `--plain` when a reverse proxy on
another host holds the certificate; without either the start is refused, saying so. A plain hub
says in its start line that the traffic is in clear. ACME is not built: `--acme <domain>` is
refused, saying to give a certificate or to put a proxy in front.

**The control session.** A device reaches the hub at its URL (`http://` or `https://`, its port
80 or 443 when the URL says none) over TCP, TLS for `https://` (the certificate checked against
the system's roots), and asks for an HTTP/1.1 upgrade to `flottille-hub`, so that a reverse proxy
carries it as it carries a WebSocket (it must pass upgrades on). Then each side says one JSON
object per line (`crates/flottille-link/src/control.rs`): the device its key, a fresh nonce and
its hello ("The version"); the hub its key, a nonce of its own, its hello and its proof, a
signature over `flottille-hub-session-v1`, a zero byte, the side, the device's key and both
nonces; the device its own proof, over the hub's key and the nonces, with the hash of the first
change of the list it checked, if it has one. A device that pinned the hub's key refuses another
one there, before it proves anything. The hub then welcomes a member's device, a member's machine,
or, while the list is empty, a device that pinned none (it comes to set the hub up), and refuses
any other key; the refusal says the list was reset when the device pinned a list that starts
otherwise, and nothing else of the list: a stranger learns neither its root nor its members. Once
welcomed, a device asks (the setup, a member's change, its addresses, its presence) and is
answered by number; each ask is taken only while the device is still welcome, so that a session
opened before a change that leaves it out asks nothing more. The hub sends the list whole, the
addresses every member's machine told, and, to a runner, the presence, at once and whenever they
change. Text the hub or anything at its address says is shown with its control and format
characters replaced.

**Its limits.** Someone with no key holds little of it: 64 connections at most that have not proved
a member's key (a setup's session counts while it lasts), 4 KiB a line of the handshake, 5 s for
the TLS handshake and the hub's handshake together, 10 s from its start for a session opened to
set the hub up, however much it asks, and 64 KiB an ask. When the 64 are taken, 8 more places are
kept for members: a connection in one of them that does not prove a member's key is let go at the
end of its handshake, and one past them is closed as soon as it is accepted. This raises the bar,
it does not remove it: strangers that keep 72 connections open, opened again every 5 s, still keep
members out while they do. When the process runs out of file descriptors it waits
before it accepts again (50 ms, then twice longer each time, 1 s at most), instead of trying again
at once. A list holds 1000 changes at most of any member's, then 100 more, each a controller's or a
runner's change of another member (a removal, mostly), so that a list a member filled with its own
changes can still be cleaned: a device refuses a longer one before it checks a change of it, and
the hub refuses the change past it, so that the list always fits the 4 MiB line a device takes. A member makes at most 30 changes of the list within an hour (a viewer declaring
its machine again and again, mostly): past them, the hub refuses, saying when to try again.
A machine's daemon keeps the session with its machine key; what it pinned is
`remote/hub.json` (the hub's URL and key, the list's root and the list as last checked), and it
writes `remote/peers.json` (the machines the link trusts) from the members' machines of the list
that checked, with the addresses the hub told.

**Its state folder** is `--state <dir>`, `$XDG_STATE_HOME/flottille-hub` by default, made 700. It
holds the hub's key, `hub.key` (600, made at the first start with the machine key's care), and
`hub.json` (600): the member list as the members signed it, and the setup code while no member
exists, written whole under a temporary name and renamed over the old file. One process uses the
folder at a time (a lock on `hub.lock`): a second hub, or `flottille-hub setup-code --reset`, is
refused while a hub runs (`another flottille-hub uses <folder>: stop it first`); `flottille-hub
setup-code` alone only reads, and serves while it runs. A list in it that does not check stops the hub's start, saying to reset it. The
invitations, their join requests, the presence and the addresses are kept in memory: they last
minutes, or are told again when a device connects.

### The first member

At its first start, with an empty member list, the hub prints a **setup code** in its log, once:
`setup code: 7KQ2-M4XP-9TRW-H3CD, valid until a first member joins`. `flottille-hub setup-code` prints
it again while no member exists. The first device (usually the computer) joins with it:
`flottille remote setup <hub URL> <code>`, or Settings, General, Machines, **Set up a hub** in the window. It
becomes the first member, a `runner`, its entry signed by itself; it is the **root** of the list, and
every device that joins later pins it. The code is 80 random bits in Crockford's base32, read in
any case and with or without its hyphens. The command reads it on its standard input (it asks
for it on a terminal), never on its command line, which other users of the machine and the
shell's history see: `echo 7KQ2-M4XP-9TRW-H3CD | flottille remote setup <hub URL>`. It signs, with
the device key, the root's entry, after a new random list id ("Defaults taken"), and the
declaration of the machine's key (a `machine` change, "Defaults taken"); the daemon claims that
declaration with its machine key and carries both to the hub in a session its machine key opens,
which must be the machine declared.

**The code never crosses the network.** The setup carries the device's proof that it holds the
code: an HMAC-SHA256 keyed by the code's 80 bits, over `flottille-hub-setup-v1`, a zero byte, the
side, the hub's key, the device's, both nonces of the session's handshake, and the hash of the
last change set up. The hub checks it before it takes the code, and answers with its own proof, the
other side's; the device pins the hub's key only once it checked it. Someone on the path, who does
not see the code and cannot guess 80 bits, cannot set the hub up with changes of its own, nor
pretend to be the hub. The root and its machine are stored in one write, or not at all. The device
then keeps the list only once the hub sent it back holding the setup, starting with its own entry:
a hub that withholds it, or sends another list, is refused, saying the hub withheld the setup. A
member's change is the same (`Device::submit`): kept once a list holding it came back.

`remote.setup` is answered to the machine's own clients only, never over a link (its class is
`Local`, `crates/flottille-protocol/src/catalogue.rs`), and refused on a machine set up already
unless it is asked to replace the hub it pinned (`--replace`): what it pinned, and the list it
checked, are let go then.

**Recovery**: `flottille-hub setup-code --reset`, on the hub's machine only, empties the member list
and prints a new setup code. Every device then refuses the new list (it does not start with the
first change they pinned) and says so (`the hub's member list was reset: set this device up
again`). A device set up again after a reset, with the same key, the same name and the same
machine, makes another list: its first change names a new random list id, so that no change of
the old list (a member removed before the reset, added back) follows on the new one. Access to
the hub's machine is what recovery needs, and all it gives.

**What a compromised hub can still do.** It cannot add a member, change a role, declare a machine,
or make a change no member signed: every device checks every change back to the first change it
pinned. A device checks that a list it is sent holds every change it checked already, so a hub
cannot take a change back from it. But a hub can **withhold the tail of the list**: never show a
removal to anyone, so that every machine keeps trusting the removed member's machine; or, to a
device that joins with no list yet (#812), serve the list as it was before a removal, extended
with changes a stolen key signs. The checks built against it are these: a device that sets the hub
up, or submits a change, keeps the list only once the hub sent it back holding its own change. The
rest needs the members to compare the heads of their lists over their links, and to say so when
they differ: not built (a follow-up issue).

A hub can also try to **slip its own key into a pairing**. The member that invited draws the four
words from the token it was given itself, and shows only a join request for a token it was given
and that is still valid: a request the hub pushes for a token it never issued is dropped. That does
not stop the hub from choosing the token it issues; what makes the choice useless is that it does
not know the new device's key yet. A device that is not a member makes a new key for each join and
keeps it only once the join succeeded (`crates/flottille/src/remote_command.rs`), so a hub that
drops a join on purpose, to be asked a new invitation, has seen a key the next join no longer
sends. Once it sees the new key, the token is issued already, and what is left is to search a key:
the words are 44 bits and nothing commits the new device to its key before the hub sees it, so a
hub may make keys until one gives the same four words with the token and the inviter's key, and
put it in the request in place of the new device's. That is about 2^44 keys made and hashed (about
1.8 × 10^13) within the 5 minutes the invitation is valid: out of reach of a machine, not of a
large fleet rented for it. A device that joins with a key it kept already (a member's, one that
joins again) sends a key the hub knows: the hub can search a token for it ahead, with no 5 minutes
to bound it, about 2^44 hashes too, a GPU-hour. A commitment of the new device to its key would
close both: not built.

## Pairing

A device is added by a member, through the hub, the user checking on both screens that they talk to
each other.

1. **An existing member asks the hub for an invitation**: Settings, General, Machines, **Add a device** in the
   window, `flottille remote invite`, or the phone's **Add a device**. It is a user action, signed by
   that member's device key (an ask, `crates/flottille-link/src/invitation.rs`: for this hub, within
   5 minutes of the hub's time, taken once). The hub answers with a token, valid **5 minutes,
   once**. The member shows a QR and the same as a code to copy, holding the hub URL with the hub's
   key, the token, and the inviting member's key fingerprint.
2. **The new device reads it** (the phone scans the QR; a computer is given the code), makes its key
   (a new one for each join, pending until one succeeded, kept then), and connects **to the hub**, not to the inviting member, with the token and its new public key.
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

**A headless machine** joins with `flottille remote join`, given the code on its standard input (or
at its prompt), never on the command line, which other users' `ps` and the shell's history keep;
the code comes from an invitation opened on a human device. It prints the four words, and the user accepts on the device
that invited it, with the role `runner` chosen there.

**Removing** (Settings, General, Machines, the row's Remove, `flottille remote remove <name>`) is the same
kind of signed change, by any member that may add one: every device that hears it removes the
member and cuts its links at once; one that was off hears it from the hub when it connects again,
before anything else is exchanged: at start, a machine accepts and dials no link until it has the
hub's list, 10 s at most, and goes on with the list it kept only when the hub cannot give it one
by then. The same holds once its session to the hub is lost (a machine that wakes from a suspend,
or was offline): no new link is accepted or dialled until the next session's list, 10 s at most
from the loss, a hub unreachable meanwhile included; the links up already go on. A session may
look alive for minutes after a suspend, with no heartbeat to say otherwise: every second, the
daemon compares how far the wall clock moved with how far the monotonic clock did, which does not
count a suspend, and more than 5 s apart ends the session as lost. A link that comes, or a dial
due, before that second's round is held the same way: the clocks the round last saw are compared
then too, and so are they where the session tells its clocks, at a round or at its start, before
they are stored. Meanwhile, a link from a key
the list kept does not trust is refused at once, and one from a key it trusts waits, one per key. A removed member's open invitations close with it, and the joins that wait on them end,
refused. A removed device key cannot join again; the hub refuses removed device keys, not machine
keys, so a removed member's machine may join again with a new device key, through an invitation
and its four words like any other. **Renaming** is the same kind of
signed change. A device that wants to leave removes itself the same way. A role is changed by
removing the device and adding it again.

A device refuses a link from a key that is not in its list, and a list change not signed by a
member.

**Built (#812), from the command line**: `flottille remote invite` prints the code on a line of its
own (the QR holds the same text); `flottille remote join`, given the code on its standard input,
makes the device key, prints the four words and waits
until the member that invited accepts or refuses; `flottille remote requests` lists the requests
that wait for this machine's member with their words; `flottille remote accept <request> --name
<name> --role runner|controller|viewer` prints the words, then signs the addition; `flottille
remote refuse <request>`; `flottille remote members`, `rename <name> <new name>` and `remove
<name>`. A request is named by the new device's fingerprint. The clients sign, with the device key;
the daemon carries what they signed (`remote.*`, `protocol.md`) and says what changed as events
(`remote.*`, `events.md`). The phone's part is not built.

**Built (#814), in the window**: Settings, General, Machines offers **Set up a hub** while this machine is
in no member list (the hub's address, and its setup code in a field that hides what is typed);
then the devices of the list (each row its name, its machine's state, its role and who added
it; Rename and Remove… in its "⋯", Remove asking once), **Add a device** (the invitation's QR
and its code, with the time it expires, folding away then), and this machine's addresses, found
or declared (New address, Remove…). A join request opens its dialog over any page: the four
words, the device's name, **This device can: Run agents / Control only / View only**, Accept
and Refuse. Every new member is a toast, `New device: pixel (control only), added at 18:42 from
pc`, with Remove. The machine switcher at the top of the sidebar shows this machine first, then
each runner with its state; choosing one shows its projects and sessions, every request of the
window signed with the device key, as the command line signs it, and carried by this machine's
daemon. Sharing a vault entry or a connection from the machine shown shares that machine's: the
request goes to it with the device's signature for it, which it checks and sends on with the
secret, as when it is asked on that machine; the sign-in of a copy on another machine comes back
to this machine's `127.0.0.1` whichever machine is shown. Choosing a machine that is off or needs an update is a failure toast that says why, and
the window stays on the machine it shows. The window reads the device key from the same keyring
item, or the same file, as the command line.

A machine that joins is no member yet: it reaches the hub with its machine key, pinning the hub's
key the code holds, and says in its proof the invitation's token and the new device's key. The hub
welcomes it while its request waits, sends it nothing and takes no ask from it, and tells the
member that invited the requests that wait for it, over that member's own sessions. Once the
addition is stored, the hub sends the joining machine the list; the machine checks it
(`Invitation::check_list`), then the new device declares the machine through the same session, and
the machine pins the hub (`remote/hub.json`) once the declaration came back. A refused or expired
request ends the session, saying so. A join the hub recorded spends the token, even when the
connection then drops before the request is accepted: the member asks a new invitation.

## Roles

The user who accepts a device chooses what it may do; the role is in the signed entry, and the hub
cannot change it.

| Role | For | May |
|---|---|---|
| `runner` | a computer, a headless server | run agents; and, from its own clients, everything a `controller` may |
| `controller` | a phone, a laptop without agents | drive without running: read, send messages, answer questions and permission requests, start and stop sessions on a runner, switch the mode, open tunnels, invite and remove devices. The default for a phone |
| `viewer` | a device the user lends, or trusts less | read only: the machines, the sessions, the conversations, the requests, never an answer |

- **A runner refuses a request a role does not allow**, once its signature is checked and before it
  serves it, saying which role allows it (`refused: pixel may only view: a controller or a runner
  may answer a request`). Every method of `protocol.md` is in one of two classes, `read` or `drive`;
  a `viewer` may only `read`.
- **A runner advertises live to its peers**, over the link: its projects, the agent CLIs installed
  and signed in (their login files checked for presence only, never read: `AGENTS.md`), and its
  free slots (the project caps minus what runs). It sends it again when it changes.
- **Starting a session on a machine** (from a client, or by an agent: "A tree across machines")
  offers only the runners that have that project and that agent signed in; a child launched there
  needs a free slot too (a session for the user counts in no children cap). A `controller` or a
  `viewer` runs no agent: a launch aimed at one is refused, naming it.
- **Every method is `read` or `drive`**: a `controller` may call every `drive` method on a runner,
  a `viewer` only `read`.
- **`workspace.show`, `workspace.forge` and `spec.list` are `read`, though they write**: the branch a
  worktree moved to, the forge's state, a spec settled as developed are the daemon catching up on
  facts, not a decision of the user, so a `viewer` sees the workspaces, their pull requests and the
  specs. A `viewer` that asks `workspace.forge {watch: true}` keeps that forge polled while its
  connection stays.
- **A `controller` may add a member of any role, a `runner` too** (a phone accepting a server), **and
  may remove any member, the root included**: the root is where the member list starts, not a member
  above the others.

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
  daemon carries it. The receiving daemon checks the device is a member, the signature, the time
  (within 5 minutes of its clock) and that the nonce was not seen in that window, then that its
  role allows the method ("Roles"), and serves the request as one from a local client, the device
  known: its log and its events name it.
- **A request carried lasts as long as its client's connection.** A watch (`events.watch`, with its
  `forge` when the client asked for one) is carried on a stream the carrying daemon holds only while
  the client's connection is open: when the client leaves, the stream closes and the receiving
  machine ends its subscription and its forge watch, with or without a new event; its log says
  `the watch asked by <device> ended`.
- **What the user sends to a machine that is off is refused**, never kept: a message, an answer to a
  request, a new session. The client says so, with since when (`server is offline since 22:14`). A
  message kept would arrive hours later, after the user forgot it.
- **Paths are picked, not typed**, from the lists the machine gives, except where the user types one
  on purpose (`flottille --machine server project add ~/code/app`, which is that machine's path).
- **Pictures** travel as bytes already (`agent.send {images}`); files other than pictures are left
  for later.
- **A link or a file an agent shows** (`web` `show_link`) belongs to its machine: a `localhost`
  address of another machine opens through a tunnel ("Tunnels"); a file of another machine is shown
  with its machine and path, and never opened here (a file of another machine is not on this one).

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
| `share` | A vault entry or a connection the user shared, signed by their device ("Sharing"): `{"request", "entry", "secret"}`, the device's signed `vault.share` as the client wrote it, the entry's metadata, its secret; for a connection (`share_connection`), `{"request", "connection", "entries": [{"entry", "secret"}]}`, the signed `connection.share`, the declaration, and each entry the request names with its secret. |
| `addresses` | Every address the sender may be reached at now: kept for it, so that the two link again without the hub ("The transport"). |

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
`refs/flottille/machines/<machine>/<launch>/<branch>`, which you review and merge as you would a
local child's work; nothing moves your branch.*

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
  with the launch, and that switch is less than 12 hours old on the receiving machine's clock.
  Otherwise it starts in Auto. A switch out of Full forgets the kept switch.
- **What the sender says is bounded** before a card or the judge reads it: a title of one line, 200
  characters at most; the caller and its tree named by agents' ids (1 to 64 letters, digits,
  dashes or underscores); the branch a name git takes, 255 bytes at most. Otherwise refused.
- **The code taken is the launch's own**: a `has`, an `offer` or a `place` for a recorded launch
  must name its project and its repositories, and the `place` its branch and all its repositories;
  anything else is refused, and nothing of it is taken.
- **A workspace made of another machine's code runs its setup only once its launch is allowed**
  (by the gate at once, or by the user's answer): held otherwise, by default, whatever becomes of
  the launch, a restart of the daemon included. A launch the user refuses ends its agent and
  removes its workspace; a launch forgotten while it waited (below) removes the workspace made of
  it, if any; a launch whose agent could not be made removes its workspace too. A launch held
  whose agent the user stopped is refused the same way: it counts against no cap and keeps no
  place.
- **A held agent can be written to by the user**, directly, on the receiving machine: it is visible
  there before the user answers, and writing to it is the user's own act. Its turn then runs, in
  its workspace, whose setup still waits; the launch's brief is not sent until the user allows the
  launch, and never if they refuse it.
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
- **An uncommitted file holding a secret that is not ignored** (an `.env` missing from `.gitignore`)
  travels to the other machine with the rest of the uncommitted work, as it would in a commit:
  nothing refuses it nor warns.
- On the receiving machine the workspace is created from those commits, on the same branch name
  (with `-2`, `-3` when that name is taken there), prepared by the project's setup, and the
  uncommitted commit is laid on it as uncommitted changes. A repository the project has there but
  never received is cloned from the bundle, never from its remote.

**Coming back.** When the remote session has committed and reports (`child-report`, or a led
session's `session-turn-ended` that reports), its daemon sends a bundle of **only the new commits**,
those past what the caller's machine sent or already has; the caller's machine verifies it and
fetches it into `refs/flottille/machines/<machine>/<launch>/<branch>`, and the delivery names that
ref: two launches from one branch come back to two refs, neither overwriting the other.
Nothing moves the user's branch: the agent or the user reviews and merges it (a `git merge` run,
judged as any), as a local child's work. The pilot of a led session on another machine stops it
and removes its workspace as today, through `call`.

**Built (#819), in the window.** While the window shows this machine, each led session and each
child an agent of this machine started on another (`remote.launches`, `protocol.md`) has a row
under its pilot's session, or the session whose tree holds its parent: its title, then the
machine's name and its state as last known, `server · running` (no delivery came yet),
`waiting` (held for the user there, or its last delivery a turn's end, a question or a request),
`reported`, `failed`, or the machine's own state while it is off (`offline since 22:14`, `needs an
update`). Clicking it switches the window to that machine (the switcher's own switch and its bounded
wait) and opens that session there, its header and its composer entering staggered. A report whose
commits came back names the ref under its step's title, folded or not (`Branch back:
refs/flottille/machines/server/<launch>/feat/export`). While another machine is shown, its inbox
items say `· on server` after their session, and its question cards `· on server` after their time.

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
- A machine that cannot take a message now (it is stopping, or cannot store it) says so in its
  answer (`{"later": why}`): the sender keeps it, in its place, and sends it again on the next
  link, the next message kept, or 30 seconds later. Only a refusal is final: the message is then
  dropped.

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

As built (#818):

- "Linked" is the link to that machine being up now: a question is routed when it is asked, and
  the link ending (the machine stopped, slept or unreachable) moves at once the questions open to
  a pilot of that machine to the user (`agent.asked` again, `moved_from` the pilot, `offline` the
  machine) and each request one of its agents holds, as if that agent forwarded it, with the
  reason `its parent's machine pc is offline`. A child's `ask` answered `Your question was given to
  the user, as pc is offline, …`; a held request the same, `Waiting for the user's approval`.
- A child's request held for its parent on another machine is told there as `child-asks`, naming
  the machine it waits on: the parent answers with `approval_answer {request_id, machine, decision,
  why}`, a `call` that names no launch: that machine finds it from the request, the live launch by
  that parent of the child whose tree asked it, and checks the request is held by it and pending. `deny` refuses it, the child told why; `forward` gives
  it to the user of that machine. A decision other than `deny` or `forward` is refused on both
  machines: no agent, and no machine, allows a request.
- A led session's pilot of another machine answers with `session_send`, as here: the message is
  stored under an id naming the pilot (`session-<pilot>-<launch>-<call>`), which answers the
  questions asked of it, even while it still waits there, queued, the session busy. It is woken at the turn's end (`session-turn-ended`, `it asked you a
  question`), its questions said whole.
- When the pilot's machine links again, each led session it leads here sends it one
  `session-meanwhile` delivery: each question it asked the user while that machine was off, with
  the user's answer or `not answered yet`; kept, ordered and sent once as every delivery
  (`agent.pilot_told` marks what was told).

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
  hours a day); a push wakes it otherwise. A known problem of the transport's library on mobile
  networks is worked around inside `flottille-link`, never in the screens.
- **First thing proven**, before the screens: answering Allow or Deny from a notification while the
  app is killed and the phone in Doze, which needs the link opened, the request signed and answered
  within the few seconds Android gives.

**The mobile core, as built (#824).** `crates/flottille-mobile` holds a `Core` over a folder of the
app's private storage:

- **Keys**: the device key, kept by the link's keyring logic in a file whose bytes the app sealed
  (`Sealer`: AES-GCM under a key of the Android Keystore, `KeystoreSealer`), pending until the join
  succeeded, as a computer's is. The endpoint's key, the phone's "machine" in the member list, is a
  key of its own in that folder with the machine key's care: a member's own key may not be its
  machine's.
- **Joining** is one function of the core (`join`, then the pairing's `accepted`), today the
  invitation of "Pairing" with its four words; the hub's pair mode (#1146) replaces it there alone.
  Once accepted, the phone checks the list back to its root and its inviter, declares its endpoint
  and keeps its key; every list after is checked against what it kept, and one that does not check
  is refused and said in the view state, the list kept as it was.
- **Connected** (`connect`, the Kotlin `Flow` of batches): the control session (presence told), a
  link to each runner of the list, dialled by one function of the core (today the addresses the hub
  tells; through the hub with #1151), each request signed with the device key, and each runner's
  `events.watch` resumed after the last event seen of it, kept in the folder: a new link, or the
  app's next start, misses nothing and repeats nothing. `request`, `answer` (Allow, Deny), `stop`
  (the turn) and `follow` (a conversation) go over those links.
- **The view state** is computed in Rust and handed whole with every batch: the runners and their
  state, one list of sessions for every machine by project (#1150), the followed conversation, the
  requests and the questions that wait for the user, and this device's name and role. A batch is
  the events that arrive within 30 ms of each other, 256 at most.
- **A panic** of the core is written to `panic.txt` of the folder (`keep_panics`), which the app
  reads once at its next start (`last_panic`).
- **Built** by the Gradle module `apps/android/core`: `cargo ndk` (API 31) for `arm64-v8a` and
  `x86_64`, linked with 16 KB pages, the unstripped libraries kept apart in
  `build/rust/symbols/<abi>`, the shipped ones stripped; the Kotlin bindings generated by the
  crate's `uniffi-bindgen` from the library. CI's `android` job runs the Gradle unit tests (the
  bindings on the JVM, the libraries' alignment) and the instrumentation tests on an emulator.

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
- One setting, Settings, General, Machines: **Notify the phone**: `when I am away` (the default) or `always`.
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
  Built (#830): the window asks the same signed `tunnel.open` the command line does, from this
  machine's daemon, on a connection of its own that it holds for as long as the tunnel is wanted:
  Close (`tunnel.close`), the run's end, the link's loss or the window quitting ends it, and the
  window never keeps a listener the user cannot see. The local address the handler is given is
  `http://localhost:<local port>` with the address's own path, query and fragment (a name under
  `.localhost` is kept, for a server that tells hosts apart). A second click on an address a
  tunnel already reaches opens it again and makes no second tunnel. The row says `localhost:5173 →
  server` with **Close**, under the run's lines, on the machine's page as on the project's. A
  failure (the machine is unreachable, the run is gone, the port is refused) is a toast with the
  reason the machine gave, `Not opened: port 22 is none of run r1's`, and opens nothing.
- **A link an agent shows, on another machine.** A `localhost` link (`localhost`, `127.0.0.1`,
  `[::1]`, `0.0.0.0`, a name under `.localhost`) names a port of the machine the agent runs on:
  Open asks that machine for its runs and takes the one that reports an address on that port, else
  the only one running, then opens the tunnel as above; no run to take is a toast (`no run of
  server serves port 5173`). A link of the web opens as it is, as on this machine. A file is the
  agent's machine's: its card says `server: /path/to/file` and draws no Open, and the window reads
  nothing of it, a path of the same name here being none of that file (no thumbnail either).
- **On the phone**, the page opens in an in-app browser by default (the tunnel lives with the app in
  the foreground). **Open in browser** hands it to the user's browser and keeps the tunnel alive
  with a foreground service and its notification, closed from that notification.
- **Built (#829), from the command line**: `flottille --machine server tunnel open <run> [--port
  <p>]` signs `tunnel.open {run, port?}` for the machine and carries on listening: the daemon of
  the client's machine listens on `127.0.0.1` (never another address) on the run's port when it is
  free there, another otherwise, says which (`address`, `local_port`) and holds the tunnel while
  the command runs; Ctrl+C, or the command's end, closes it. `tunnel list [--machine m]` lists the
  tunnels this machine carries (the flag filters, it is no request for another machine) and
  `tunnel close <tunnel>` closes one. The window's rows are above (#830).
- **The machine of the run decides, never the one that asks.** The request is checked as every
  request of another machine's client ("Requests from another machine's clients": member,
  signature, time, nonce, and the role: `tunnel.open` is a `drive` method, a `viewer` is
  refused). Then the run must be one the daemon started and that still runs, and the port one a
  process of the run's session listens on: the sockets of those processes, read from `/proc`
  (Linux; on a system with no `/proc` no port is reachable until another way to read them is
  built), matched to `/proc/net/tcp` and `tcp6`, listening on the loopback or on every address.
  The address the run reports (`localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0` or a name under
  `.localhost`, with a port written) only chooses the port taken when none is asked for: an
  address is a line the run printed, which a script of the workspace may write, so it reaches
  nothing by itself, and a port no process of the run listens on is refused, whatever the run
  reports. A port is also refused when **any other** socket listens on it, on any address and in
  either table: Linux lets `[::1]:22` sit beside `0.0.0.0:22`, so a port the run shares with
  another process may lead to the other one (`refused: port 22 is none of run r1's: ...another
  process listens on 22 too`). Any other port of the machine is refused (`refused: port 22 is
  none of run r1's`). Everything is checked again at each connection, so that a port the run
  gave up and another process took is not reached.
- **The machine connects to the address the run listens on**: `127.0.0.1` for a listener on
  `127.0.0.1`, on `0.0.0.0` or an IPv4-mapped address, `[::1]` for a listener on `::1` or `::`,
  and never another address. A connection is checked, then made: what remains is the instant
  between the two, in which a process that is none of the run's could take the port the run just
  gave up; it needs to bind the very port, on the loopback, in that instant, and the next
  connection is refused. No agent has a tool to open a tunnel: the request is signed by the
  user's device key, which no agent reads, and the user's command line refuses to run under an
  agent.
- **The local listener is open to every process of the carrying machine**, its agents' processes
  included, as the listener of `ssh -L` is: it is on `127.0.0.1` for this reason and no other
  address, but a program of the same machine that connects to it reaches the run's port. A
  tunnel is a door to a port of the run for as long as it is open: close it when it is not in use
  (`tunnel close`, or the command ending). The machine's own user is the one it opens for.
- **A connection is a raw stream.** Each TCP connection is one stream of the `client` link, opened
  by a line (`{"tunnel": "<id>"}`) and raw bytes either way after it; the end of each direction
  ends the other side's reads. A tunnel's connections are not framed like a bundle's chunks
  (base64 in JSON): the transport gives an ordered stream with flow control and a half-close,
  which is all a TCP connection is, and framing would add a copy and a third more bytes to every
  page. A connection for a tunnel the machine did not open for that link's machine is closed and
  said in the log.
- **Bounds and ends.** At most 8 tunnels open for a device on a machine, and 64 connections at
  once on a tunnel (a ninth tunnel is refused; a connection past 64 is closed). The tunnel is held
  by its request's stream: it ends when the run ends, `tunnel close`, the link to the machine is
  lost, the device is removed from the member list, or the client's connection to its daemon ends.
  The events `tunnel.opened` and `tunnel.closed` (`events.md`) are said by both machines.
- Later: names on the desktop (portless) and `*.localhost` names on the phone.

## Sharing

### Vault entries

- **Entry by entry, with consent**: Settings, Vault, an entry's **Share with…** (or `flottille vault
  share <name> --machine <m>`), a signed user request on the machine that holds it. That daemon reads
  the secret from its keyring and sends it, with the entry's metadata and the device's signed
  request, in a `share` message, end to end, to the other runner, which checks that request and
  the metadata itself, and that a runner's or a controller's machine sent it (never a viewer's),
  and writes it into its own keyring. **Share with…** lists the runners online. Never a
  file, never an agent, never the store, never an event, never the hub: as `vault.md` says of every
  secret.
- The copy is marked `shared from pc` on that machine; it is a copy, not kept in sync: sharing again
  from the same machine replaces it, never an entry that machine set up itself. A machine with no
  keyring refuses it, saying so.
- An entry bound to a use (`for`) keeps it; a `worklog:<project>` entry needs that project on the
  other machine.
- **Built (#832)**: `vault.share {name, machine}` (`protocol.md`), `flottille vault share <name>
  --machine <m>`, and Settings, Vault, a row's **Share with…**. How it is checked, and what it
  leaves where, is `vault.md`, "Shared with another machine".

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
- **Built (#833)**: `connection.share {name, machine, entries}` (`protocol.md`), `flottille
  connection share <name> --machine <m> [--entry <e>]…`, and Settings, Connections, a row's
  **Share with…**. It is a signed user request on the machine that holds the connection, as
  `vault.share` is: that daemon refuses a command, an `http://` URL and an `https://` URL of this
  machine (`localhost` or a name under `.localhost`, with or without their final dot, a loopback
  address, `0.0.0.0` or `[::]`, a loopback written as an IPv6 mapping such as
  `[::ffff:127.0.0.1]`), saying `the connection <name> is this machine's own`; it refuses an entry
  the connection does not name, and a sign-in's tokens (an entry named `<x>.oauth`) whoever asks
  (`OAuth tokens are never shared: <m> signs in for itself`). **Known limit**: a name is not
  resolved, so a DNS name that resolves to a loopback address passes as remote (what it resolves
  to when it is shared proves nothing of later); the receiver's checks are the same.
  The `share` message then carries the declaration and each entry the request names, with its
  secret (`{"request", "connection", "entries": [{"entry", "secret"}]}`). The other runner checks the
  request as it checks a vault entry's (member, signature, time, nonce, role, the sender a runner's
  or a controller's machine), then that the connection is the one the request names, that the
  entries are exactly the ones it names, and the declaration and the entries' metadata as its own
  Save would (an entry one project's alone or bound to another use is refused) before anything is
  kept: the entries are kept as shared ones (`shared from pc`), then the connection is declared as
  the user's Save declares it (given at once while Connections and it are on); if it cannot be
  declared, the copies this share added are removed (a copy that replaced one the same machine
  shared before stays). One share is received at a time. A connection of the same name there is
  never replaced (`server has its own connection notes: remove or rename it there first`).
- **The sign-in from a client (#833)**: `flottille --machine server connection login <name>`, or the
  window's **Sign in on server** of a connection it shared. The address that machine answers is
  checked as the window checks it before it is printed or opened (`https://`, or plain `http://` on
  the loopback; no control character). The client signs `connection.login` for
  that machine, whose daemon begins the sign-in as for its own client and notes the member whose
  device asked; then `tunnel.open {sign_in: <name>}`, whose answer is the tunnel; then the browser
  opens the address. That tunnel reaches no run: the machine serves it only while a sign-in of that
  connection waits there, only for the device that began it, and only to its one-shot loopback
  listener, whose port its redirect names; each connection is checked again (the very sign-in
  still waits), and the tunnel ends with the sign-in, its last word saying whether it signed in.
  The client's side listens on that very port of `127.0.0.1` or refuses (the redirect names it).
  What `tunnel.open {run}` may reach is unchanged.

## The repository

One monorepo. The crates and folders this milestone adds:

| Where | What |
|---|---|
| `crates/flottille-protocol` | Typed requests, answers and events; serde only (from the preparation). |
| `crates/flottille-client` | Connect, request, resume `events.watch {after}`; used by the command line, the window and the mobile core (from the preparation). |
| `crates/flottille-link` | Identity, keys, the member list and its checks, joining, the transport, signing and checking requests. |
| `crates/flottille-hub` | The hub's binary (control plane only), its `Dockerfile` and a compose example. |
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
`HOME` and XDG folders and its own in-memory keyring, on `127.0.0.1`, with no discovery; two devices
that cannot reach each other are tested with an address nobody listens on. Fake agents, throwaway repositories. A machine
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
- That signed switch is the `agent.mode` request a device signed, kept whole by the caller's
  machine while the tree stays in Full, checked by the receiving machine for its signature, its
  signer's role, the caller's machine, the tree and Full, and its time: it counts for **12 hours**
  from when it was signed (one night's work), by the receiving machine's clock; past that, the
  launch starts in Auto, and the user switches the tree to Full again. Its nonce is not checked: it
  goes with every launch of those hours.
- The switch is kept on the tree's root and may name any agent of the tree (the user switched it
  from a child): the launch names the tree by that agent, which the receiving machine checks the
  switch against.
- The tree a launch names is the caller's machine's word: a machine holding the user's switch of
  one tree to Full may name that tree for a launch of any of its agents within those 12 hours, and
  that launch starts in Full. Accepted as a limit: the switch is the user's, for that machine, and
  that machine runs the user's agents already.
- A launch held for the user on the receiving machine makes its agent and its workspace there once
  its code came, the agent sent nothing and the setup waiting until the user allows it; the code
  of a launch is taken when the gate accepted it or held it for the user, never when it refused it.
- A machine may leave 4 launches, of any kind, waiting for the user's answer on another: its next
  is refused until the user answered one, however many it sends at once.
- The setup of a workspace made of a launch's code runs only as that launch's, by the workspace's
  id: a launch whose workspace was removed names none, and a launch refused or forgotten removes
  only the workspace made of its code, never one of the user's of the same name.
- A child on another machine counts in its parent's cap on the parent's machine while its parent
  lives, until its report or its failure comes back, or its parent stops it; a launch whose code never came holds a slot of the
  receiving machine's cap until it is forgotten, an hour after it was asked.
- A machine may leave 4 launches waiting for their code on another.
- The returned commits go to `refs/flottille/machines/<machine>/<launch>/<branch>`, never onto
  the user's branch.
- Deliveries between machines are kept 30 days by the sender; the user's requests to an offline
  machine are refused.
- What crosses machines (#817): a child's report, question, failure, idle and stuck notices, the
  notice that its process died and it goes on (`child-replaced`), and a led session's
  `session-turn-ended` (when a turn reported or asked, and when it ends or fails
  unless its pilot ended it), and since #818 a child's held request (`child-asks`) and what a led
  session asked the user while its pilot's machine was off (`session-meanwhile`). Not yet:
  `led-integrated`, whose check reads the forge and the base on the session's machine.
- Where a question goes when the parent is off (#818): a parent of another machine that forwards
  a request of its child gives it to the user of the child's machine, never to its own parent on
  its machine (the request lives where the call was held). A request goes to the user when the
  link to the parent's machine ends, and stays there when that machine links again. A session
  whose machine starts again while the pilot's is off routes on the link it has then: in the
  seconds before the links are made, a question goes to the user. Questions a child asked its
  parent before the parent's machine went off stay its parent's, kept for it with the other
  deliveries; only a pilot's are moved, as the specification says. The session is not told that
  its questions moved.
- `agent_send` and `session_send` to a session on another machine are kept and sent in the
  background even while that machine is on: the caller is answered at once (`Sent to agent … on
  server`), and a refusal there is not told back to it; it is dropped with an inbox entry of the
  sending machine (`dropped`), as a message kept 30 days is.
- `session_read` of a led session on another machine answers what `agent_status` gives: the
  session's line and its last messages, not its paged conversation.
- `agent_status` without `agent_id` lists the children of this machine only; a child on another
  machine is read by its id.
- A delivery is stored there under `<launch>-<its id>`: sent again, it is stored once.
- At most 100 messages wait for one machine: one more `agent_send` or `session_send` is refused at
  once (`refused: 100 messages wait for server already`), and one more delivery is not kept, with
  an inbox entry (`dropped`). A message of more than 64,000 characters is refused on the sender, as
  the receiver would.
- A message the other machine cannot take now (`later`) is sent again 30 seconds later, or sooner
  on the next link or message kept.
- An invitation lasts 5 minutes, works once; a request's signature is valid 5 minutes.
- The verification words are four, from a fixed list of 2048 words.
- The first member, the one that used the setup code, is the root every device pins; a reset of the
  hub makes every device set up again.
- `project.detect` is `drive`: it looks through any folder the client names, and serves only
  `project.add`.
- In the preparation, `flottille-protocol` types the framing and the parameters; answers and events
  stay `serde_json::Value`, and each later ticket types in Rust the ones it reads (the mobile core,
  the link).
- A launch an agent aims at a runner that cannot take it is refused, naming the runners that could;
  the agent's tool description lists no machine.
- A notification waits 30 seconds and is dropped when a client had an input in the last 2 minutes.
- A connection shared (#833) is declared on the other machine as the user's own Save declares it:
  no mark of where it came from (its entries carry `shared from`), and a connection of the same
  name there is refused, never replaced, even one shared earlier from the same machine: the user
  removes it there and shares again.
- **Share with…** of a connection leaves its vault entries behind unless the user turns on **Share
  its vault entries too**: secrets leave the machine only on a gesture of their own.
- The window has no switcher of machines yet: the **Sign in on <machine>** of another machine's
  connection is offered for a connection this window shared (its toast, its row's menu), and the
  row says the copy's state there (`On server: not signed in yet`, `signing in`, `signed in`) for
  as long as the window runs.

- The link listens nowhere until the user sets `[remote] listen` (an IP address and a port,
  `0.0.0.0:7447` in the examples): how a machine is reached is the user's business, and a laptop
  with remote on does not open a port on every network it joins. Listening nowhere, it opens no
  socket until it dials, and links to every machine that listens. Reversible: listening on
  `0.0.0.0:7447` by default again is one line. An address declared without a port takes the link's,
  7447 when it listens nowhere. A link that came in is asked to prove its address (a QUIC retry),
  takes at most 5 seconds to finish its TLS, 256 at once (a stalled handshake costs a few
  kilobytes), 2 from one host (an IPv4 address, an IPv6 /64) and 8 from one IPv6 /48; one that
  finds no place free is refused at once, and dials again, so that a host that stalls its
  handshakes holds no other back. 8 more places are kept for the IP addresses written in
  `peers.json` (`addresses` and `told`; a name is not looked up for it), which a host not there
  cannot take: a machine the daemon trusts that dials from one of them still finds a place when
  strangers hold all the others. The trade-off: machines behind one IPv4 address (a NAT, a
  carrier-grade NAT) are one host and share its 2 places, so a stranger behind the same address
  can hold them; a trusted machine whose address is in `peers.json` still has the kept places, one
  that dials from an address not there (behind a NAT, its told addresses are its own, not the
  NAT's) waits until a place frees, 5 seconds at most. Whether a key is trusted is said after its
  TLS, so that the other side is told why it is refused, by the peers' file as last read (read
  again only when it changed since). A key refused is said in the log once a minute at most, and
  16 keys a minute at most, since a key costs a stranger nothing: past them, one line says the
  others are counted, and the count is said when the minute ends. The
  machines a daemon trusts, and the addresses it knows them at, are in `remote/peers.json` (`{"machines": [{"name", "key", "addresses", "told"}]}`),
  which the control session to the hub writes from the member list once the machine is set up
  (a test writes it for machines with no hub); the addresses declared by hand in
  `remote/addresses.json`. Both are read at every round (each second): the daemon dials each
  machine it trusts and is not linked to, waiting longer after each failure (5 seconds at most).
  A file caught half written, or broken by hand, leaves the last one read whole in use (said once
  in the log), and a told address is not written over it. A file another hand changed while the
  told addresses were being written is read again, once, rather than written over; this is a
  check, not a lock: a change made in the microseconds between the check and the rename is still
  written over, and when the file changes again during the retry the told addresses are dropped
  (said in the log) until the machine tells them again, at its next link or address change. The
  told addresses are written away from the tasks that take links, at most once every 2 seconds
  for one machine, the latest told kept.
- Two machines that dial each other at once each hold two links: both keep the one the machine of
  the lower key dialled, and close the other (`linked already`).
- Two machines linked tell each other their addresses at once, then whenever they change (a
  machine message `{"machine": {"addresses": […]}}`); the one told keeps them in `peers.json`,
  under `told`, in place of those told before, 64 at most, beside the `addresses` it was given
  (by the hub, by hand), which it never changes: the two link again without the hub.
- The version handshake is the first stream of a link: the dialler sends its hello, the other
  answers its own, and each chooses or refuses by itself; the dialler closes a refused link. A
  link that hears nothing for 15 seconds is lost (each side says something every 5).
- A client's request for another machine (#813) is a line of the local socket that holds
  `machine`: the local daemon carries it as it is, never served here, on a `client` link it dials
  to that machine (kept, and dialled again once when it failed), one stream per request. What
  comes back on the stream is the protocol's own lines, `\n`-ended: the answer, then, for
  `events.watch`, the events; the local daemon writes back only the answer under the request's
  own `id`, and events after it. A request the receiver refuses is answered `refused` with its
  sentence. A machine whose link is down is refused at once, `unavailable`, `server is offline
  since 22:14` (the time the link went down, the local time of day), or `server needs an update:
  …`; nothing is kept. A request whose `machine` is this machine's own name is checked and
  served here the same way.
- The receiving daemon checks a request against the member list of its `hub.json` as last
  checked, its own member's name as `machine`; then serves it on a connection of its own, as a
  local client's, which ends with the answer (or, for a watch, when the stream ends). Its log
  says `agent.start asked by pc from pc` (the method, the member whose device signed it, the
  machine that carried it), and every event its serving emits has `device`, the member's name
  (`docs/events.md`). Events emitted later by what it started (an agent's turn) do not.
- A watch of another machine whose link dropped ends the local connection, so that its client
  resumes it: `flottille watch --machine server` connects again, every second while the machine
  is offline (said once), and watches again `after` the last event it printed, a new request
  signed each time. The window's switcher is a later ticket.
- `flottille daemon`, `window`, `update`, `mcp`, `bridge` and `remote` are this machine's own and
  refuse `--machine`; `flottille --machine server daemon stop` asks that daemon to stop.
- The client's link is dialled by the machine that carries the request: a machine that listens
  nowhere can be driven only by a machine it can be dialled by today. Carrying the user's
  requests over a link the other machine dialled is left for the phone.

- A tunnel (#829) is held by the command that opened it, which stays in the foreground
  (`flottille --machine server tunnel open r1`): Ctrl+C closes it. A tunnel that outlives its
  command would need a name and a way to find it again; the window and the phone hold theirs for as
  long as they are open. Reversible: a `--detach` is a later flag.
- **A link an agent shows names no run (#830).** Its port is looked for among the machine's running
  commands (`run.list`): the one that reports it, else the only one that runs, the machine
  checking the port again. A guess that is wrong is refused there (`port 5173 is none of run r2's`),
  never reached. Reversible: the agent's own run could be named by the link later.
- **A file of another machine is not opened (#830).** Fetching it to open it here is the later
  work "Pictures travel as bytes already; files other than pictures are left for later" names:
  until then its card says where it is and draws no Open.
- **The window's tunnels are not listed again after a restart (#830).** The listener is the
  window's connection's: a window that quits closes them, and one that opens again starts with
  none (`tunnel.list` still lists what a command line holds).
- A tunnel carries at most 64 connections at once and a device has at most 8 tunnels open on a
  machine: a small cap for one person's pages and APIs, said in the refusal; raising it is one
  constant.
- A raw stream of a link has 5 seconds to end its opening line (and 512 bytes): a member machine
  that opens one and does not is given up on, and every other stream of the link is taken
  meanwhile.
- `tunnel.list` and `tunnel.close` are of the machine that carries the tunnel (the listener is
  there), class `local`, never over a link; `--machine` filters them. `tunnel.open` is `drive`.
- The local port is the run's own when it is free, so that a page that hard-codes its port still
  works; another one otherwise, said in the answer.

- A key file holds the 32 bytes of the private key, nothing else; it is made under a temporary
  name with `O_EXCL`, synced, then linked into place, so that two clients making the device key at
  once keep the same one.
- A keyring that is there but fails (locked) is said, never replaced by `device.key`: the same
  device would get a second identity.
- A machine key that is refused stops the daemon's start with its sentence, rather than starting
  with remote off.

- The member list is an ordered log of signed changes (add, remove, rename), each naming the
  SHA-256 of the change before it: a change the hub drops, reorders or plays again breaks the
  chain, and a device refuses a list that lacks a change it has already checked. Two members
  changing the list at the same moment: the hub keeps the first, the second signs again.
- What a change signs is a fixed encoding that starts with `flottille-member-change-v1` and a
  zero byte (`crates/flottille-link/src/members.rs`), never a JSON text; nothing else a device key
  signs starts so.
- A name is 1 to 64 characters, with no control character and no space at either end, and no two
  members have names that differ in case only; a removed member's name is free again.
- Any member may remove itself, a viewer too; a viewer may add, remove or rename no other member,
  and may not rename itself.
- A key that is a point of small order is refused wherever it is read (an entry, an invitation's
  hub or inviting key): no device makes one. A hub URL is `http://` or `https://` with a host, at
  most 2048 bytes, and a URL or a name holds no invisible character (Unicode's `Cf`: zero-width
  characters, bidirectional overrides), so that two that look the same are the same.
- A removed key is never added again, so "a role is changed by removing the device and adding it
  again" means that the device joins again with a new key.
- The invitation holds the inviting member's whole key, not its fingerprint: the new device checks
  its entry's signature against that key, and a fingerprint of 8 bytes is a match one could
  search for. The code is Crockford's base32 with a checksum of 4 bytes, about 180 characters with
  a short hub URL, and the QR holds the same text. The token is 16 bytes.
- The four words are BIP-39's English list (MIT licence), drawn from the first 44 bits of a
  SHA-256 of the two keys and the token.

- What a request's signature covers is a fixed encoding that starts with `flottille-request-v1`
  and a zero byte (`crates/flottille-link/src/request.rs`), never a member change's: `id`,
  `machine`, `device`, `nonce`, `at` and `method`, then `params`. `machine` and `id` sit outside
  `signed` in the JSON and are signed all the same: a request signed for one machine is refused by
  another, and its answer cannot be sent under another id. `machine` is the target's name; a name
  freed by a removal and taken within five minutes by a new member could take a request meant for
  the old one: accepted.
- `params` travels as the JSON text the client wrote, in a string; the receiver checks the
  signature over those very bytes, then parses them, so that no side serialises JSON again. `at` is
  in milliseconds since the Unix epoch, `nonce` 16 random bytes in hexadecimal.
- A request is checked in this order: it is a request (else refused before anything), for this
  machine; its device is a member and the signature is the device's; its time; its nonce; its
  device's role, then its nonce is remembered. Until the signature passes, every refusal is one
  sentence that names nothing of the list (`refused: this request is not signed by a member`), so
  that a sender with no member's key learns no name and no role. A method the catalogue lacks is
  refused, whoever asks. What a refusal shows of the sender's text (a machine's name, a method, a
  parse error) is cut to 64 characters, its control and invisible characters replaced.
- A request is refused when its time is more than five minutes from the receiver's clock, either
  way, or before the receiving daemon started: the nonces it remembers are in memory only, one
  memory for all its links, and a restart would otherwise let a request be served again. A sender
  whose clock is behind is refused for at most five minutes after a restart
  (`refused: this request was signed before this machine started: send it again`). The daemon
  keeps them in memory (#813), not in the store: what a restart costs is that refusal.
- The receiver forgets a nonce once it leaves the window of the latest time its clock gave, and
  keeps at most 4096 per device: past that, the device's requests are refused until the oldest
  leave the window, never forgotten early. The time check reads the clock as it is now, so that a
  clock read far ahead once locks no device out; a clock set back by more than five minutes may
  then let a forgotten request in again: accepted.
- A field of a request longer than 4 GB, more than its length prefix says, is neither signed nor
  taken.

- The hub's state is one JSON file, `hub.json`, written whole and renamed into place, beside its
  key: no database. The invitations and their join requests are kept in memory: a hub started
  again has none, and the member asks again.
- The setup code is printed in the log at the start that made it only; a hub started again before
  a first member joined prints none, and `flottille-hub setup-code` prints it, reading the state
  without the folder's lock so that it serves while the hub runs (the state is written whole and
  renamed, never seen half written).
- A member's ask for an invitation signs, after `flottille-hub-invite-v1` and a zero byte, the
  hub's key, its own key, its time and 16 random bytes: an ask is for one hub, taken within 5
  minutes of the hub's time either way, and once. A `viewer`'s ask is refused.
- A join request ends with its invitation, 5 minutes after the invitation was given, however late
  the device joined. Only the member that invited may accept it (its signed entry of that key) or
  refuse it; the refusal is the member's word over its control session, not a signed change, since
  refusing adds nothing.
- The hub takes an addition only as the answer to a join request to its signer: a device joins
  through an invitation, never by a change alone.
- A machine that joins (#812) answers the invitation in its session's proof (`join`: the token
  and the new device's key), so that a key with no token is still refused at the end of its
  handshake. While its request waits, its session counts among the 64 places of keys that are no
  member's, is sent nothing and may ask nothing; it ends with the request (refused, or 5 minutes
  after the invitation was given). In one of the 8 places kept for members, it is refused before
  its token is spent. A key removed from the list, or a member's, is refused at once
  (`… was removed from the member list: a removed device joins again with a new key`).
- The device that joined declares its machine at once, through the session it joined with: the
  daemon never holds the device key, so `flottille remote join` waits for the acceptance and signs
  the declaration then. `remote.join` is refused on a machine set up already: a machine joins one
  hub.
- The member that invited is sent the requests that wait for it (`{"requests": [{"token", "key"}]}`)
  over every session of its device or its machine; the token travels back to the machine that
  asked for it, so that its clients draw the same four words. A request is named by the new
  device's fingerprint (`flottille remote accept 3f2a-91bc-0d4e-77a1`).
- A removal is carried out by every daemon that takes the list holding it: the machines it trusts
  are written again, and its link to the removed member's machine is cut at once
  (`remote.disconnected`, `it was removed from the member list`). The removed machine's own session
  to the hub is refused, so it never sees the removal itself; its links are cut by the others.
- `remote.connected` and `remote.disconnected` are the machines' links, not the hub's session
  (its state is `remote.machines`' `hub`, and `remote.hub` says it changed).
- The formats of #820 changed without a version of their own, nothing being released: the
  device's proof has `join`, the asks `invite` and `refuse`, an answer `token`, and the hub sends
  `requests`.
- `flottille-hub setup-code --reset` is refused while a hub uses the state folder, rather than
  told to it: the running hub would keep the old code valid and undo the reset at its next write.
  It reads nothing of the state, so a list that does not check, or a damaged file, does not stop
  it; the hub's key stays.

- The list holds the devices' keys; the link and the control session run on the machines' keys.
  The tie is a change of the list, `machine` (kind 4 of the signed bytes: the member's key, then
  the machine's), which a member signs for itself only, any role, and the machine claims: its
  `claim`, the machine key's signature over `flottille-machine-claim-v1`, a zero byte, the
  member's key and the hash of the change before, is part of the change's hash. The hub cannot
  make it, a member cannot declare a machine whose key it does not hold, and a removal takes it
  away. A machine key is no member's key, nor another member's machine; declared again, it
  replaces the one before. The first member's setup carries its own, which the daemon claims.
- The first change of a list names, in place of a previous change, the list's id: 32 random bytes
  made at setup. Nothing of the formats is released yet: the signed bytes changed with it, and
  with the claim, without a version of their own.
- The hub listens on `127.0.0.1:7448` unless told otherwise: #820's "every interface" is the
  devices' link, not the hub, and a plain hub on every interface would carry every session in
  clear. Another address takes TLS, or `--plain` for a reverse proxy elsewhere.
- ACME is not built (a dependency of weight): `--acme` is refused with what to do instead. TLS,
  on the hub and to it, is TLS 1.3 only.
- A machine that cannot reach another says every address it tried, from where, and how each
  failed: `server is unreachable: tried 127.0.0.1:7447 from pc, no answer in 10 s`; `from` is the
  machine's name in the member list (its key's fingerprint before it has one). A hub that cannot
  be reached is said the same way, with since when: `the hub at hub.example.net:443 is
  unreachable since 22:14: tried 192.0.2.10:443: Connection refused`.
- A machine set up tells the hub its addresses at once, then whenever they change; the hub keeps
  them in memory, 64 at most per machine, each a host and a port, and sends every member all of
  them. A machine set up again (`remote.setup` while `hub.json` is there) is refused unless asked
  to replace the hub (`--replace`), which replaces what it pinned. An ask the hub does not answer
  in 10 s, or refuses, is asked again at the next round; the session goes on meanwhile.
- A refusal of a role names what was asked in the user's words for what they do most (`answer a
  request`, `send a message`, `start a session`, `invite a device`, `add, remove or rename a
  device`, …), the method otherwise (`may call project.add`). Each method's class is said beside
  it in the table of `protocol.md`, "The classes", which a test holds to the catalogue.
- A launch is a method that may start an agent's process, marked in the catalogue
  (`flottille_protocol::LAUNCHES`): `agent.start` (and its old name `agent.run`), `agent.resume`,
  `agent.quota_resume`, `spec.new` (which starts a writer when given an idea), `spec.from_ticket`,
  `spec.develop` and `review.start`. Aimed at a machine whose member is a `controller` or a
  `viewer`, it is refused by the machine that would carry it and by that machine itself
  (`refused: tablet is a controller: it runs no agent, a runner does`), in one check of
  `flottille-link` (`runs_agents`). A launch by an agent on another machine (`agent_launch
  {machine}`) takes the same check, on both machines. `task.run` is no launch: it runs a
  command of the project, not an agent.
- What a runner offers (`capabilities`, `crates/flottille-link/src/offers.rs`) is looked at only
  while a machine is linked to this one: when a link is made, after an event that may change it (a
  project added, edited or removed, a provider changed, a session that starts or ends), and every
  30 s for what no event says (a login file); `remote.machines` looks at this machine's at once. It
  is sent on each link when it changed, in order, and again, waiting longer each time (1 s doubled
  up to a minute), when the other machine did not take it: its projects, each with its children cap minus the children that run or are being launched (the cap a
  `child` from another machine counts in), and each provider enabled, installed (on `PATH`, or a
  test's override) and signed in where Flottille can see it, as `doctor` says it (its login file
  present by its metadata, never opened, or its sign-in variable set, never read). It names 1024
  projects and 64 agents at most, each name 1 to 64 characters with no control or invisible one.
  It is taken only from a runner's machine, and only from the link kept for it (one replaced says
  nothing any more), and forgotten when its link ends: an offline runner offers nothing. The slots
  bound the children launched from another machine, not a session for the user. No event
  says it changed yet: a client reads `remote.machines` again.
- A watch (`events.watch`) carried for a device ends when the device leaves the member list: each
  list taken, the machine that serves it looks again whether the device is still a member.
- The git work of "What travels" is `crates/flottille/src/git/bundle.rs`. The snapshot is made
  from a copy of the repository's index (a linked worktree's own), under Flottille's name
  (`Flottille <flottille@localhost>`), never signed: it is never kept as a commit. The bundle
  holds one ref, `refs/flottille/outgoing/<launch>`: its parent is the branch as it was when the
  snapshot was made, so the branch travels below it and the branch's name goes beside the bundle.
  When the other side has every commit already, no bundle is written (git makes no empty one)
  and the answer says "nothing new".
- A bundle received is verified, then fetched into `refs/flottille/incoming/<machine>/<launch>`
  (the sending machine's key: two machines' launches of one name are two refs); a repository
  never received is made by `git init` and that fetch, with no remote. The fetch sets
  `protocol.file.allow=always` for itself: a configuration that refuses git's file transport is
  about submodules and remotes, not a file of Flottille's; and `fetch.fsckObjects` and
  `transfer.fsckObjects`: an object git finds malformed fetches nothing.
- The snapshot's ref on the sending side is removed once the travel ends, placed or failed; the
  incoming refs on the receiving side once its workspace is made, or its making failed. A launch
  whose code never came whole (the sender gone mid-way) is forgotten an hour after it was asked,
  with its incoming refs, when its machine next asks a launch; and a machine may leave 4 launches
  at most waiting for their code, its next refused until they are placed or forgotten.
- The snapshot is laid with `read-tree -m -u HEAD <snapshot>`, then `reset`, on a workspace with
  no change of its own (refused otherwise): every change is unstaged, and a file `HEAD` does not
  have is untracked there, even one the sender had staged.
- Every git of a launch runs with `GIT_NO_LAZY_FETCH=1`, and a partial clone
  (`extensions.partialClone`, or a remote with `promisor = true`) is refused on either side: what
  it lacks is on its remote, and a bundle of it would be incomplete.
- Git LFS objects do not travel, their pointers do: the receiving side runs with
  `GIT_LFS_SKIP_SMUDGE=1`, so that its smudge filter downloads nothing and leaves the pointer.
  The sending side's clean filters run as the user's own `git add` would run them, locally.
- A returning bundle holds the one branch of the workspace; a `machine` and a `launch` are each
  one ref component and the `branch` a branch name git accepts, or nothing is fetched.
- Over the link (`crates/flottille/src/travel.rs`), the `git` message is one of four. `has`
  (a project and a repository's path in it): the receiver answers the commits of its branches and
  of `refs/flottille/machines/*`, or none when it does not have the repository yet. `offer` (a
  transfer's name, the launch, the project, the repository, the sender's branch, the bundle's
  length and SHA-256): the receiver reads the bundle with `fetch`, on a stream of its own where
  each frame is a base64 chunk of 192 KiB, checks the length and the SHA-256, then has git verify
  and fetch it. Only the machine a bundle is offered to may read it, and only until its offer is
  answered. `place` (the launch, the project, the branch, the repositories): the workspace made of
  what came, answered with its name, its branch and its folder.
- `has`, `offer` and `place` are taken only for a launch the receiving machine recorded and its
  gate did not refuse (`launch l2 was not accepted by this machine, which takes no code for
  it`); a test build run with `FLOTTILLE_TEST_TRAVEL` set also takes them without one
  (`test.travel`). Even then, they are
  taken only from a runner's or a controller's machine, by the checked member list: a viewer's
  machine, or one the peers' file trusts that is no member's, is refused. `fetch` is answered
  only to the machine a bundle is offered to.
- A bundle is 2 GiB at most, refused past it before a byte of it is read, on either side. A
  machine sends one thing at a time to another (an `offer` or a `place`; a second is refused
  while one is served), and a bundle being read that sends no frame for a minute is given up. The
  sender hashes its bundle a chunk at a time and reads its prerequisites from its header alone.
- The receiver takes a bundle only into a project registered under the same name with that
  repository; a repository whose folder is missing or empty is cloned from it, and a folder that
  holds something else is refused. A repository of a launch is taken again from the same machine
  only while the launch has no workspace there (its sending failed part way and is tried again).
  Workspaces are made one at a time; a branch made meanwhile on the name found free is refused,
  never reused. The workspace is made offline (no fetch of the receiver's own remote), its branch
  the sender's name or the first of `-2`, `-3`… that no repository of the place has, its name the
  branch's with every other character a dash, and its base, in each repository, the commit the
  sender's branch was on (`started from what pc sent`); the snapshot is laid on each worktree as
  soon as it is made. A worktree that fails fails the place, which its sender may send again.
  Made for a launch, it takes the project's setup, which runs at once when the gate accepted the
  launch and waits, from its first step after the worktrees, while it is held for the user, a start
  of the daemon included; made by `test.travel`, it takes its worktrees alone.
- Each side keeps what travelled in its store (`travels`): the machine, the launch, the repository,
  the commits sent, and the workspace made. The sender keeps a launch as sent only once its
  workspace is made there: one that failed part way is sent again under its name. A bundle coming
  back is taken only for a repository of a launch this machine sent to that very machine, into the
  repository it was sent from, under the name this machine gives that machine; the way back sends
  the commits past the sender's tip, its snapshot, and what came back already.
- The window's Machines are a section of Settings, General (Set up a hub, Devices, Add a device,
  Addresses), not a tab of their own: the five tabs of Settings stay five (`docs/reference/gpui.md`,
  rule 13). Its switcher offers the runners, and the machines whose role is not known yet (linked
  with no hub's list), never a controller or a viewer. The device a window sets a hub up from is
  named by the host's name, as `flottille remote setup` names it without `--name`. A join request
  whose dialog the user closed without answering is not opened again by that window until it is
  restarted; it waits until it expires, and `flottille remote requests` still lists it. While the
  window shows another machine, what it keeps of itself (its folds, the drafts, its appearance)
  stays with this machine. A member's row says its role, who added it and its machine's state:
  `remote.members` says neither when it was added nor when it was last seen, which the toast of a
  new member says from its event.
- A test build run with `FLOTTILLE_TEST_STOPPING_FILE` set answers every `deliver` and `call`, while
  the file it names exists, as a daemon that is stopping does (`later`).
- A test build run with `FLOTTILLE_TEST_TRAVEL` set answers `test.travel {machine, project,
  launch, workspace?, corrupt?, hold_ms?}` and `test.travel_back {launch}`, so that two daemons
  can be made to send code to each other without an agent. Otherwise the commits go back before a
  child's report, or a led session's end of turn that reported, is sent (`crates/flottille/src/across.rs`),
  and the delivery names the ref they came into. A bundle coming back is taken only for a launch
  this machine recorded as sent to that very machine (`launch l1 was not sent to server: no commits
  come back for it`).
- `agent_launch {machine}` (`crates/flottille/src/launches.rs`): the caller's machine refuses it
  at once when `machine` is no machine of the user's, is this one, is not a runner, is offline, or
  its offer lacks the project, the caller's provider or, for a child, a free slot, naming the
  runners whose offers would take it; `workspace`, `new_workspace`, `repositories`, `branch`,
  `base` and `project` are refused with it, since the place is a copy of the caller's. It then
  sends the `launch` machine message (the launch's name, the caller and the root of its tree, the
  kind, the project, the title, the brief, the provider, model, effort and role, the mode, the
  kept switch to Full, the caller's branch and repositories), and the code of the caller's place
  under the launch's name, and answers with the agent started there or the request waiting there.
- The receiving machine takes a `launch` only from a runner's machine, for an agent it names, while
  it runs agents itself, and checks again the project and its repositories, the provider signed in
  there, the brief's length (the judge's 8,000 characters) and, for a child, a free slot of its own
  cap. Its gate then judges it: Full (with a switch it verified) accepts it; Auto asks its judge,
  with the words and the brief whole, nothing of the caller's conversation (which is on the other
  machine), and holds what the judge does not allow, or everything when no judge is chosen; Ask
  holds it. A launch held makes its agent and its workspace once the code came, and a request for
  the user of that machine (an action, its words the launch's), its agent sent nothing and its
  setup waiting until the user allows it; refused, the agent ends (`the user refused its launch`).
  Its answer is for no agent of that machine: kept as sent, never delivered.
- The agent started there is a root of that machine's tree (the caller is on another machine),
  in the mode the launch took, its role the caller's choice (`worker` for a child, `lead` for a
  session by default). A child's first message is its brief; a session's says `agent <id> of
  <machine> started this session for the user with this message:` above the brief, never the
  user's words.
- Across machines (`crates/flottille/src/across.rs`), the session's machine sends the agent that
  started it a `deliver` (its id, the launch, that agent, the kind, the body, the urgency, whether
  the session ended with it), and that agent's machine sends a `call` (its id, the launch, the
  calling agent, the session, the tool and its arguments). A `deliver` is taken only for the agent
  of an `out` launch of that name to the sending machine; a `call` only from the `caller` of an
  `in` launch of that name from the sending machine, for the agent it started, never for a
  handed one, and only for `agent_send` (a child), `session_send` (a led session),
  `agent_status`, `session_read` and `agent_stop`; anything else is refused (`agent … of pc did
  not start agent …`). A call is judged on the caller's machine as one to a session it started
  there (ending a led session as `end_led`).
- What goes to another machine is kept in the sender's store (`outbox`) before it is sent: in
  order, one at a time, when the link to that machine is made and whenever a message is kept
  while it is up; forgotten once the other machine answered it. `agent_status`, `session_read`
  and `agent_stop` are sent at once, and refused at once while the machine is off (`refused: server
  is offline since 22:14: agent … cannot be reached now. You will be woken by what comes back
  from it.`). The sweep drops a message kept 30 days, with a `remote.delivery_dropped` event, an
  entry of kind `dropped` in the inbox.
- The user's switch to Full is kept by the caller's machine when `agent.mode {mode: full}` came
  from another machine's client, signed by a device: the request whole, as it was signed. Any
  other switch of that tree's mode forgets it. The receiving machine takes it only signed by a
  member that may switch a mode (a runner or a controller), for the caller's machine, that tree
  and Full; neither its time nor its nonce is checked, since it lasts as long as the tree stays in
  Full. A switch made in the window or the command line of the caller's own machine is signed by
  no device: a launch from such a tree starts in Auto.
- A child on another machine counts in its parent's project cap on the parent's machine from its
  launch for as long as its parent lives, until its report or failure comes back or its parent
  stops it (the `out` launch is then `ended`); in the receiving machine's cap from its launch
  until it ends.

- A row of a session on another machine says its state as last known from what that machine
  delivered here, not a live read: `running` once launched until its first delivery, `waiting`
  after a turn's end, a question or a request, until the next delivery says otherwise (a message
  the pilot sends does not make it `running` again here). An ended launch leaves its pilot's
  rows unless its last delivery was a report: a reported child stays, `reported`, under its pilot
  while the pilot lives. A session handed to the user is not shown under the agent that started it.
  The daemon leaves them out of `remote.launches` itself, in one read, and also a reported launch
  whose caller ended: an old launch is never read again. A launch held for the user there is
  `waiting` until something of it is heard here (its approval there sends nothing back).
- Those rows are rows of the tree only (`Snapshot.elsewhere`): this machine's daemon does not know
  those sessions, so a project's page, search, a session's page and the inbox do not list them,
  and nothing offers to end them from here.
- Those rows are shown only while the window shows this machine: another machine's tree is its own,
  and lists what its agents started elsewhere on its own machine only.

- The mobile core (#824): a batch is what arrives within 30 ms of its first event, 256 events at
  most; a runner that cannot be linked is dialled again after 250 ms, doubling up to 30 s, and at
  once when the hub tells new addresses. Its sessions list holds root sessions only, by project
  name, projects by name (rows never move between groups), sessions by last exchange. The
  questions that wait come from the events seen (`agent.asked`), not from a read when the phone
  links: one asked before the phone's first link shows from the next one. The phone links to the
  runners of the list only, never to another phone or a controller's computer.

## Open questions

1. **The transport's library**: decided in #811, QUIC (`quinn`) with TLS 1.3 on the devices' raw
   keys (`rustls`) ("The transport").
2. **A headless server's keyring**: sharing vault entries needs one; whether Flottille offers a
   keyring of its own there, or the user installs a Secret Service.
3. **A controller's rights**: whether a `controller` may switch a tree to Full, or answer a base
   request, without the phone's unlock (biometrics), or whether those need a runner's client.
4. **A led session on another machine starting its own led sessions** there: today a chain is base
   (`tools.md`, "No chain without the user"); whether a session the user sent away for the night
   gets more.
5. **Syncing the project's configuration** between machines, and registering a project on first
   launch.
