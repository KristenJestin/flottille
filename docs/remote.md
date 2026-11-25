# Remote

Flottille on several machines, driven from any of them and from a phone. A machine is a place an
agent can send work to, as it sends work to a workspace: a session on the user's computer starts a
session on their server for the night, that one works alone, and its report comes back to the
computer when the computer is on again. Every client (the window, the command line, the Android
app) reads and answers the sessions of every machine of the user. It is the milestone
`v0.4 - Remote` (`docs/design.md` §12, §13).

**Status: the design of v0.4, decided with the maintainer on 2026-11-12, 2026-11-13, 2026-11-14,
2026-11-15 and 2026-11-20.** Nothing here is built but the keys, and the member list and the pair
code as data and their checks, and the link between two machines with its version check
(`crates/flottille-link`, `crates/flottille/src/link.rs`), and the hub with its rules, served on
the network, and the control session every machine keeps to it (`crates/flottille-hub`,
`crates/flottille-link/src/control.rs`, `crates/flottille/src/hub_session.rs`), and pairing in
pair mode on the hub, with roles changed and devices removed from its host, and members renamed,
restyled and removed from the command line (#1146, which replaced the setup code and the
invitations of #812), and the user's requests to another machine, signed by the client,
carried by its daemon and checked by the machine they are for (`--machine`, #813), a place's
code sent to another machine over the link and its new commits brought back (#815), the roles
bounding what each device may do, with what each runner offers its peers (#862), `agent_launch`
starting a child or a led or handed session on another machine, judged by that machine's gate,
its code sent by itself (#816), the deliveries and calls between a session and the agent that
started it on another machine, kept for a machine that is off, with the commits coming back (#817),
where a question and a held request go while the parent's machine is off (#818), the tunnels
to the ports of a run's server (#829), a vault entry shared with another machine (#832), a
connection shared the same way (#833), the window's machines, in Settings, Machines
(#814), one list of the sessions of every machine in the window, with the close of a session
of a machine that is off kept until it is back (#1150, its points 1 to 3), and the pictures and
videos a session showed, sent to any device (#1165), and the runner's and the hub's side of the
phone's notifications, with the window's presence and **Notify the phone** (#827), and About you for all machines, kept by
the hub sealed end to end ("Settings for all machines", #1330). What is decided is said as such; the defaults taken are in "Defaults taken, to confirm";
what is open
is in "Open questions". It replaces the proposal of 2026-11-03 (a relay
every device connects to, on a private network) and the review that put Tailscale and SSH at the
centre: Flottille stays generic (`AGENTS.md`), and depends on no one network and no account of a
third party. The user runs one small program of Flottille's, the hub, which every device keeps a
connection to; since 2026-11-20 the links between their devices go through it, encrypted end to
end, so that nothing but the hub needs to be reachable ("The transport").

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
| Device | What holds a key the user paired with their hub: a computer (its daemon and its clients), a phone. |
| Member | A device in the user's member list, with its name, its keys, its role, its icon and colour. |
| Member list | The list of the user's members: the hub's entries of the devices it paired, signed with its key, and the members' own changes, each signed by its member. Every device keeps a copy and checks every signature itself. |
| Role | What a member may do, signed in its entry: `runner`, `controller` or `viewer` ("Roles"). |
| Runner | A member that may run agents: a computer, a headless server. |
| Hub | `flottille-hub`, the one program the user runs for all their devices, wherever they like: it holds the member list, pair mode and the presence, and carries the links' packets between members, which it can neither read nor change. It adds the devices, behind access to its host. |
| Link | Flottille's encrypted connection between two devices, end to end, its packets carried by the hub over the connection each device keeps to it. |
| Pair mode | What `flottille-hub pair` opens on the hub's host for one device: a code and its QR, valid a few minutes, once. |
| Pair code | What pair mode prints: the hub's address, its key and a secret the device proves it knows, never sends. |
| Tunnel | A TCP connection carried over the link, from a client's local port to a run's port on a machine. |

"Led session" keeps its name and its meaning (`docs/tools.md`, "Sessions an agent starts"); a led
session on another machine is the heart of case 1, and is called a led session everywhere: in the
window, the command line, the tools and the docs.

## What is decided

1. **Generic, and no network channel of its own.** No dependency on Tailscale, on one network or
   on a service of a third party. Identity and the link are Flottille's own; the network under
   them is the user's. Whether they have a LAN, a VPN, Tailscale, a VPS or a public address is not
   Flottille's concern: the one thing it asks is that every device can reach the user's hub.
2. **A hub, the user's own** (replaced on 2026-11-20; it said "no relay"). One `flottille-hub` per
   user, self-hosted: the members, pair mode, the presence, the signed member list, and
   **the links between the devices, which go through it**, over the connection every device
   already keeps to it. No relay of anyone else's (not hosted, not public), and no public hub by
   default.
3. **End to end, through the hub** (replaced on 2026-11-20; it said "direct only, no NAT
   traversal"). The data between two devices goes over a link, encrypted and authenticated by
   their keys; its packets travel in the two devices' control sessions to the hub, which hands each
   to the device whose key it names and can neither read nor change it. A device needs no address,
   no port and no `listen`: only the hub is reached, and nothing of a device listens. Flottille does
   no NAT traversal, no hole punching, no port mapping and runs no address discovery service. **The
   direct link was removed on 2026-11-21** (the maintainer's decision): no device listens on an
   address, none is dialled at one, and no address is found, declared or told. **No queue on the
   hub** beyond the packets in flight: what waits for a machine that is off waits on the machine
   that sends it.
4. **The hub adds the devices, behind access to its host** (decided 2026-11-20, in place of the
   devices adding each other). A device pairs in pair mode, which only a command on the hub's host
   opens (`flottille-hub pair`); its entry is signed with the hub's key, which every device pinned
   from its pair code, and every device checks each change itself: one neither the hub nor a
   member that may make it signed is refused. **Whoever controls the hub's host can pair a
   device**: the user accepts it, pairing being done once, beforehand. What a compromised hub can
   do is said in "The hub", "What a compromised hub can do".
5. **Roles, signed in the entry**: `runner`, `controller`, `viewer`, chosen on the device when it
   pairs, proposed by what it is, and changed only on the hub's host (`flottille-hub role`), since
   a role gives rights. A runner refuses what a role does not allow.
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
    desktop through UniFFI. The phone is woken by its own connection to the hub, kept in a
    foreground service (no push service: no UnifiedPush, no FCM), never while the user is active at
    another client.
15. **Versions are checked at connection**, and a mismatch is said in a sentence.
16. **One repository** for the daemon, the hub, the mobile core, the Android app and the site, with
    CI split by paths.
17. **A device is paired on the hub only, and removed from any device or the hub's host.** A
    `controller` or a `runner` may remove, rename and restyle any member; a `viewer` itself only. A
    role is changed on the hub's host, never from a device.
18. **Presence goes through the hub.** With the hub unreachable, a runner counts the user as away,
    and the phone is notified.
19. **The CLI's commands are `flottille remote …`** (`on`, `off`, `pair`, `members`, `rename`,
    `icon`, `colour`, `remove`), and the key folder is `remote/`; the hub's host's are
    `flottille-hub pair`, `role`, `remove`, `devices` and `reset`.

## Architecture

```
                         hub (the user's own: members, pair mode, presence, the links' packets)
                        ╱        │         ╲        every device keeps one session to it, and its
                       ╱         │          ╲       links go in it, encrypted end to end
  phone (app + Rust core)        │           machine "server" (daemon, agents)
                                 │
                       machine "pc" (daemon, agents)
                                 ▲ local socket
                       window, CLI of "pc"
```

- **The hub** is reached by every device at the URL its pair code holds (with the hub's key, so
  that a device knows it reached its own hub). Each device keeps a control session to it: it
  receives the member list and its changes, what the hub knows of each device, the presence of the
  user's clients, and which members are online; and its links' packets go in it ("The
  transport"). The hub forwards a link's packets and never terminates a link: it holds none of its
  keys.
- **Each machine's daemon holds one link endpoint**, whose links go through its session to the hub;
  it listens on no address. The local socket is unchanged, with its refusal of a client an agent started (`protocol.md`,
  "Transport").
- **The window and the command line reach another machine through their own machine's daemon**: a
  request names `machine`, is signed with the device key by the client, and the local daemon
  carries it over its link to that machine and carries the answers and events back. The local
  daemon cannot forge a request: it does not hold the device key.
- **The phone holds its own endpoint**, whose key is its device key, and links to each machine
  through the hub, as a machine does. It links only while the app is in the foreground; a push
  wakes it otherwise ("Notifications").
- **Two kinds of traffic on a link**, kept apart by the link's protocol name (ALPN or its
  equivalent): `client`, the user's requests (the methods of `protocol.md`, each signed by a
  device), and `machine`, the closed set of messages one machine sends another for its agents
  ("What a machine may send"). A message of one kind on the other is refused.
- **The protocol is the daemon's** (`protocol.md`), unchanged in its framing: the same requests,
  answers and events, numbered, resumed with `events.watch {after}`. It was written for this.
- **When the hub is down**, devices do not link to each other: the links open end, on both sides,
  and none is made until the hub is back. Each device says so with the hub's own sentence (`the hub
  at hub.example.net is unreachable since 22:14: …`), for the hub and for each machine it cannot
  link to (`server cannot be reached: the hub at … is unreachable since 22:14: …`); a request for
  another machine is refused saying it. What waits for a machine that is off waits on the machine
  that sends it, as it does for a machine that is off.

## The transport

Flottille handles no network channel. What it needs of the transport is stated here, and the
library that gives it, chosen in #811, is hidden in `flottille-link`:

- **a device's identity is its public key**: a link is to a key, and a device that does not present
  the key it was dialed for is refused;
- **encrypted end to end and authenticated by the devices' keys**, on both sides, with nothing
  between the two devices that can read or change it;
- **through the user's hub** (since 2026-11-20, the only path since 2026-11-21): a link is dialled
  by key, and its packets travel in the control sessions both devices keep to their hub, which
  forwards them between members only; no NAT traversal, no hole punching, no port mapping (UPnP,
  NAT-PMP, PCP), no address discovery service, and no server of a third party in the path;
- **streams**: requests, events, a git transfer and tunnels at once on one link, kept apart;
- **it runs** on Linux, macOS later, and Android.

**How it came to this.** iroh was first the candidate, for its NAT traversal and its relay; the
maintainer decided on 2026-11-15 that both are out of scope (decision 1), which also ended the
spike #839. Tailscale and tsnet stay rejected (decision 1). The rest of this document is written
against the needs above, not one library: "an endpoint", "an address", "a stream".

**The transport built** (#811) is QUIC (`quinn`), whose TLS 1.3 runs on the devices' own Ed25519
keys sent as raw public keys (RFC 7250, `rustls`): the dialler checks the other side holds the very
key it dialled, the side dialled reads the key the dialler proved and refuses it unless trusted,
with a sentence (`refused: this machine's key is not one of the user's`). Both libraries were in the
lock already, their default features off; they are dependencies of weight, said in #811's pull
request. iroh, with its relays and discovery off, was not kept: its NAT traversal cannot be turned
off (its QUIC settings enforce a minimum of traversal addresses, and its endpoint runs the hole
punching), and its relay, DNS and network-watching crates come with it whatever is turned off.
Only `crates/flottille-link/src/transport.rs` names the libraries: the daemon, the clients, the hub
and the mobile core see Flottille's types (an endpoint, a link, the two kinds of traffic), so that a
change of transport changes that file.

**Through the hub** (#1151). QUIC runs over a socket of Flottille's own under `quinn` (its
`AsyncUdpSocket`, `RelaySocket` in `transport.rs`): what QUIC sends goes out in the device's control
session as one line, `{"relay": {"to": <key>, "packet": <base64>}}`, one QUIC packet a line (1200
bytes at first, up to 1452 once QUIC's discovery of the path's size grew them, 1500 the most the hub
takes); the hub hands it to the session
of the member whose key it names as `{"relayed": {"from": <key>, "packet": …}}`. QUIC knows
addresses, not keys: each key is given an address of the relay's own, in a unique local IPv6 range
(`fd46:6c74::/32`) no network reaches through it: the device opens no socket of the network for
its links, and none listens. The TLS 1.3 of QUIC, on the devices' raw Ed25519 keys, checks the
other side's key end to end; a
packet the hub changed fails QUIC's authentication and is dropped, and a link whose packets are all
changed ends. Each session to the hub is a transport of its own: when it ends (the hub down, the
session lost), every link through it ends, with it. The hub tells every member which members'
sessions are open, `{"online": [<key>, …]}`, whenever that changes: a device dials only a member
online, and says plainly when the other is not (`server is offline: it is not connected to the
hub`). Members only: a packet from or to a key that is no member's (a stranger, a machine whose
join waits, a member removed) is dropped by the hub, and so is one to a member whose session is
not open; none is kept for later. Who a link is from is the key its TLS proved, never the hub's
word: a hub that says a packet is from another member gets nothing served as that member. A device
lets a packet from a key it did not dial in only from a member the hub says online, so that a hub
that makes up senders spends none of the room its own dials need.

**Behind a reverse proxy.** The control session is HTTP/1.1 upgraded as a WebSocket is, so the
frames pass through a proxy that passes upgrades on (port 443, a container behind it). Each side
says `{"keepalive": {}}` every 25 seconds when it has nothing to say, under the 60 s idle timeout
of the common proxies; a device that hears nothing from its hub for 80 seconds takes the session as
lost, and its links with it. The hub does the same with a device it hears nothing from for 80
seconds, or that reads nothing it sends for as long: its session is closed, and the others are
told it is offline, rather than left trying a connection that died.

**What the hub sees** of the links: which devices talk to which, when, and how much (the size and
timing of each packet); never what they say. It cannot read a request, an answer, an event, a
bundle of code or a tunnel's bytes, nor change one unseen, nor make a link as one of the user's
devices: it holds none of their keys. What it does as a network is what any network can: drop
packets, delay them, or stop carrying them, which ends the links. What it does as the hub is
larger: it pairs the devices, and a hub whose host or key is someone else's can pair a device of
theirs, which declares its own machine and is trusted by every machine; with the links through the
hub, that device reaches every machine through the hub itself, wherever it is, with no other way
in to the user's network ("What a compromised hub can do").

**Its bounds.** One device cannot fill the hub: each device may send others 4 MiB a second through
it (1 MiB at once), every packet it sends counted, those the hub drops too; and at most 128 packets
from one device wait for another (under 200 KiB), each sender having its own share, so that one
that floods a device leaves the others their way to it. A packet past a bound is dropped, as a
network drops one, and QUIC sends it again, slower. The others' links go on: a packet takes neither
the hub's lock nor its member list, which the forwarding is given again each time it changes.

On a link, each message goes on a QUIC stream of its own, one JSON frame one way and its answer
(`{"ok": …}` or `{"refused": "…"}`) the other; the first stream is the version handshake ("The
version"). The protocol names are `flottille-machine` and `flottille-client`; a message of the
other kind is refused by the receiver, the link going on (`a machine's message is not taken on a
client link`).

## Identity and keys

- **Remote is off** until the user turns it on: **Turn on** in Settings, Machines, or
  `flottille remote on`. The daemon writes its configuration itself,
  `$XDG_CONFIG_HOME/flottille/daemon.toml`, keeping every other key and comment of the user's file:

  ```toml
  [remote]
  on = true
  ```

  Off, nothing of it is made or read. On, nothing listens: every link goes through the hub.
  Whether remote is on is set in the window (Settings, Machines) and from the command line
  (`flottille remote on`, `flottille remote off`; `remote.configure`, `protocol.md`), and taken at
  once: the link starts or stops, with no restart. Turning remote off closes the links and keeps
  the pairing with the hub and the keys. A file edited by hand is still read at the daemon's
  start. A `listen` in it, which the direct link read until 2026-11-21, is left where the user
  wrote it and not read: the daemon starts, and says once in its log, at each start of the link,
  that the key is no longer read.
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
  member list's changes included. **A device is known by its key, made once**: the first pairing
  makes it and keeps it at once, and a pairing again (with a new code, after a reinstall, after a
  removal) sends the same key, which finds everything attached to it on the hub: its entry, its
  role, its card. A key a setup or a join of before pair mode left **pending** (an item
  `Flottille: device, pending`, `flottille-key = device-pending`, or `device.key.pending`) is kept
  by the next pairing as the device's key. The keyring holding two items of the kept device key
  (two clients that kept one at the same moment) is refused, never one of them taken: which is the
  member's cannot be told.
- **The phone's device key** is made by the app, kept encrypted under a key of the Android Keystore,
  and never leaves the phone.
- **The hub's key** is made at its first start, in its own state folder. It signs the entries of
  the devices it pairs, their roles and the removals made on its host. Devices pin it from the pair
  code they pair with: a hub that answers with another key is refused before anything is proved
  (`the hub at hub.example.net:443 has another key than the one this device pinned`), and a change
  signed with another key than the pinned one is no hub's.
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

- **The member list**, as the hub and the members signed it: it stores it and sends it, with every
  signed change, to every member. It signs the entries of the devices it pairs, a role changed and
  a removal made on its host; it checks each member's change before storing it, as every device
  does after it.
- **Pair mode**, while its command runs: one secret and its minutes.
- **What it knows of each device** (its card's part that is no signed entry, "A device's card"):
  what the device says of itself, when it was paired and last seen.
- **The presence**: which of the user's clients is active ("Notifications"), as each client tells
  it.
- **The sessions open**, by key, and for each the packets of links waiting for it (128 at most from
  each member's device).

**What it carries**: the links' packets between members ("The transport", "Through the hub"),
encrypted end to end by the devices, which it forwards and never opens; it keeps no message and no
content of what they say, only the packets in flight for a device whose session is open.

**What it sees**: which keys connect to it, when, from which IP addresses; the member list (names,
keys, roles, never a secret); the presence; what each device says
of itself; and of the links, which devices talk to which, when, and how much. Never what they say.

**Its configuration**: a listen address (a device must be able to reach it, like any other), TLS (a certificate given, ACME, or plain behind a reverse
proxy), a state folder. The hub URL is given to every device in its pair code: `--public-url
<url>`, the address the devices reach it at (through a reverse proxy mostly), else the address it
listens on (`pair` refuses to print a code when that is every address or the loopback, which no
other device dials, and says to give one: the systemd example gives it on the hub's command line). Built (#820): `--listen <address>` (`127.0.0.1:7448` by default: loopback, behind a reverse proxy on the
same host), `--tls-cert <pem> --tls-key <pem>` for TLS on a certificate given, plain otherwise,
and `--state <dir>`. Another address than loopback takes TLS, or `--plain` when a reverse proxy on
another host holds the certificate; without either the start is refused, saying so. A plain hub
says in its start line that the traffic is in clear. ACME is not built: `--acme <domain>` is
refused, saying to give a certificate or to put a proxy in front.

**The control session.** A device reaches the hub at its URL (`http://` or `https://`, its port
80 or 443 when the URL says none) over TCP, TLS for `https://` (the certificate checked against
the system's roots: on Android the phone's system roots but those the user removed, and no
certificate the user added, as Android does for this app; no root found is an error that says
so), and asks for an HTTP/1.1 upgrade to `flottille-hub`, so that a reverse proxy carries it as it
carries a WebSocket (it must pass upgrades on). Then each side says one JSON
object per line (`crates/flottille-link/src/control.rs`): the device its key, a fresh nonce and
its hello ("The version"); the hub its key, a nonce of its own, its hello and its proof, a
signature over `flottille-hub-session-v1`, a zero byte, the side, the device's key and both
nonces; the device its own proof, over the hub's key and the nonces, with the hash of the first
change of the list it checked, if it has one, and, when it pairs, its pairing ("Pairing"). A
device that pinned the hub's key, from its pair code or since, refuses another one there, before
it proves anything. The hub then welcomes a member's device, a member's machine, or a device whose
pairing it took, and refuses any other key; the refusal says the list was reset when the device
pinned a list that starts otherwise, and nothing else of the list: a stranger learns neither its
root nor its members. **What a refusal proves**: the hub signs it with its key, over
`flottille-hub-refused-v2`, a zero byte, the device's key, both nonces, what it received of the
device's proof (the hash it pinned, or none; the pairing it asked, or none), `reset` and the
reason, so that it holds for this session, and this request, only. The device's own signature
covers neither the hash nor the pairing: something on the way could add a pairing to a member's
proof, or change the hash, to have the hub refuse it; the device checks the refusal against what it
sent, and such a refusal does not check. A device acts on a refusal (it forgets the hub, or says
its list was reset) only once that signature checks; an unsigned refusal, or one whose signature
does not check (said by something on the way to an `http://` hub, answering a request changed on
the way, or caught in another session), changes nothing, and the device says the hub could not be
reached. A refusal said before the handshake (`{"refused": "…"}`) is the hub's word on the
version only with its hello signed ("The version", "What the hub's hello proves"); otherwise the
device says the hub could not be reached, and shows its words as what answered, not as the
hub's. A hub of before 2026-11-22 signs no
refusal: a device of this version that it refuses says it cannot reach it, and keeps trying, until
the hub is updated; a device of before takes this hub's signed refusal as it took an unsigned one.
A refusal later in a session (a member removed meanwhile) only ends the session. Once welcomed, a device asks (a member's change, what it is, its presence)
and is answered by number; each ask is taken only while the device is still welcome,
so that a session opened before a change that leaves it out asks nothing more. The hub sends the
list whole, what it knows of each device, and, to a runner, the presence, at once and whenever
they change. A machine of before 2026-11-21 still asks to tell its addresses: the hub answers and
keeps nothing of them; a hub of before then still sends the addresses its members told: a device
reads and ignores them, so that neither loses its session while the other is updated. The hub
sends each session an empty list of addresses once, `{"addresses": []}`, which a phone's core of
before waits for before it links to its runners. Text the hub or anything at its address says is shown with its control and format
characters replaced.

**Its limits.** Someone with no key holds little of it: 64 connections at most that have not proved
a member's key (a pairing's session counts while it lasts), 4 KiB a line of the handshake, 5 s for
the TLS handshake and the hub's handshake together, 30 s from its start for a pairing's session
that has not declared its machine, however much it asks, and 64 KiB an ask. When the 64 are taken, 8 more places are
kept for members: a connection in one of them that does not prove a member's key is let go at the
end of its handshake, and one past them is closed as soon as it is accepted. This raises the bar,
it does not remove it: strangers that keep 72 connections open, opened again every 5 s, still keep
members out while they do. When the process runs out of file descriptors it waits
before it accepts again (50 ms, then twice longer each time, 1 s at most), instead of trying again
at once. A list holds 1000 changes at most of any member's, then 100 more, each a controller's or a
runner's change of another member or the hub's (a removal, mostly), so that a list a member filled
with its own changes can still be cleaned: a device refuses a longer one before it checks a change of it, and
the hub refuses the change past it, so that the list always fits the 4 MiB line a device takes. A member makes at most 30 changes of the list within an hour (a viewer declaring
its machine again and again, mostly): past them, the hub refuses, saying when to try again.
A machine's daemon keeps the session with its machine key; what it pinned is
`remote/hub.json` (the hub's URL and key, the list's root and the list as last checked), and it
writes `remote/peers.json` (the machines the link trusts) from the members' machines of the list
that checked.

**Its state folder** is `--state <dir>`, `$XDG_STATE_HOME/flottille-hub` by default, made 700. It
holds the hub's key, `hub.key` (600, made at the first start with the machine key's care),
`hub.json` (600): the member list as the hub and the members signed it, and what the hub knows of
each device, written whole under a temporary name and renamed over the old file; and `hub.sock`,
the socket its host's commands reach the running hub on (600; a connection of another user than the
hub's, root aside, is refused). One process uses the folder at a time (a lock on `hub.lock`): a
second hub, or `flottille-hub reset`, is refused while a hub runs (`another flottille-hub uses
<folder>: stop it first`). A list in it that does not check stops the hub's start, saying to reset
it. Pair mode and the presence are kept in memory: they last minutes, or are told again when a
device connects.

**Its host's commands** talk to the hub that runs, on its socket, beside it (in a container, with
`docker exec`): `flottille-hub pair` ("Pairing"), `flottille-hub role <device> <role>` (the hub
signs the member's new role), `flottille-hub remove <device>` (the hub signs its removal) and
`flottille-hub devices`; a device is named by its name or its key's fingerprint. With no hub
running they say so (`no flottille-hub runs with <folder>`).

**Recovery**: `flottille-hub reset`, on the hub's machine, with the hub stopped, empties the member
list. Every device then refuses the new list (it does not start with the first change they pinned)
and says so (`the hub's member list was reset: pair this device again`). The next device paired
starts another list: its first change, the hub's, names a new random list id, so that no change of
the old list (a member removed before the reset, added back) follows on the new one. Access to the
hub's machine is what recovery needs, and all it gives.

**What a compromised hub can do.** Since pair mode (2026-11-20), **the hub adds the devices**:
whoever controls its host, or its key, can pair a device of their choosing, with any role, change
a role, and remove a member; every device takes it, the change being signed with the key it
pinned. That is the trade-off the maintainer accepted: pairing needs access to the hub's host, and
the hub is the user's own, self-hosted. Every connected client announces a new device at once
(`New device: pixel (control only), added at 18:42 by the hub`), so that an addition behind the
user's back is seen. What the hub cannot do: make a change in a member's name (a name, an icon and
a colour, the declaration of a machine), declare a member's machine (the machine claims it with its
own key), or take back a change: a device checks that a list it is sent holds every change it
checked already. A key other than the pinned one signs nothing a device takes, so a hub whose key
changed is refused, not followed. A hub can still **withhold the tail of the list**: never show a
removal to anyone, so that every machine keeps trusting the removed member's machine. The check
built against it: a device that submits a change keeps the list only once the hub sent it back
holding that change. The rest needs the members to compare the heads of their lists over their
links, and to say so when they differ: not built (a follow-up issue).

## Pairing

**One way in, for every device, the first one and the others alike** (decided 2026-11-20): pair
mode on the hub. Devices add no device; there is no invitation, no setup code and no
verification words.

1. **On the hub's host, `flottille-hub pair`** (in a container, `docker exec <container>
   /flottille-hub --state <folder> pair`; beside a systemd service, as root) opens pair mode on the
   hub that runs, which is not stopped. It prints a **code and its QR** (in half blocks: the console
   and a container's log show it), says how long it is valid (5 minutes, `--minutes` 1 to 15),
   waits until a device paired or the minutes passed, then says which device paired and returns.
   Stopping the command closes pair mode. One pair mode is open at a time; the code works once.
2. **The code holds everything the device needs** (`crates/flottille-link/src/pairing.rs`): the
   hub's address (`--public-url`, else the address it listens on when another device can dial it), the hub's key, which the device
   pins, and a secret of 128 bits. The device types nothing else and knows it reaches the right
   hub: one that answers with another key is refused before anything is proved.
3. **The device proposes its card and confirms it**: its name (the host's name), its icon (a
   laptop with a battery, a desktop with a screen, a server without either), its role (a machine
   with no screen, a server: `runner`, signed in or not, the agents being signed in later; a
   desktop or a laptop with agents signed in: `runner`, without: `controller`; a phone:
   `controller`), and a colour of
   its key's; the user changes any of them before confirming. In the window: one modal, **Connect
   to your hub** (paste the code or scan its QR, then the card, then confirm). On the command line:
   `flottille remote pair [--name] [--role] [--icon] [--colour]`, the code on its standard input.
4. **The code never crosses the network.** The device proves it holds the secret in its session's
   handshake: an HMAC-SHA256 keyed by the secret, over `flottille-hub-pair-v1`, a zero byte, the
   hub's key, the session's key, both nonces of the handshake and the card it proposes. The proof is
   good for that session and that card only: played again, the hub's nonce is another; changed on
   the way (a role raised, a name), it no longer checks.
5. **The hub pairs the device**: once the proof checks, pair mode ends, and the device's entry, the
   card as proposed, is signed with the hub's key (a `pair` change). A device whose key is a
   member already keeps its entry, its role and its card: nothing is added. The hub sends the list;
   the device checks it back to its root and to the hub's key the code holds, finds itself in it,
   and a computer declares its machine (the device signs it, the machine claims it) through the
   same session. The machine pins the hub (`remote/hub.json`) once the declaration came back.
   **Every connected client announces the new device**: `New device: pixel (control only), added at
   18:42 by the hub`, with Remove beside it.

**Removing** (Settings, the row's Remove, `flottille remote remove <name>`, or `flottille-hub
remove <name>` on the hub's host) is a signed change, a member's or the hub's: every device that
hears it removes the member and cuts its links at once; one that was off hears it from the hub when
it connects again, before anything else is exchanged: at start, a machine accepts and dials no link
until it has the hub's list, 10 s at most, and goes on with the list it kept only when the hub
cannot give it one by then. The same holds once its session to the hub is lost (a machine that
wakes from a suspend, or was offline): no new link is accepted or dialled until the next session's
list, 10 s at most from the loss, a hub unreachable meanwhile included; the links up already go on.
A session may look alive for minutes after a suspend, with no heartbeat to say otherwise: every
second, the daemon compares how far the wall clock moved with how far the monotonic clock did,
which does not count a suspend, and more than 5 s apart ends the session as lost. A link that
comes, or a dial due, before that second's round is held the same way: the clocks the round last
saw are compared then too, and so are they where the session tells its clocks, at a round or at
its start, before they are stored. Meanwhile, a link from a key the list kept does not trust is
refused at once, and one from a key it trusts waits, one per key. A removed device comes back only
paired again on the hub, with its own key. A device that wants to leave removes itself the same
way.

**Its name, icon and colour** change from any device (`flottille remote rename`, `icon`, `colour`;
the row's ⋯ in the window), each a change signed by a member. **Its role** changes on the hub's
host only (`flottille-hub role <device> runner|controller|viewer`), since it gives rights.

A device refuses a link from a key that is not in its list, and a list change signed by neither the
hub nor a member that may make it.

### A device's card

Attached to its key: renamed, it keeps the rest; paired again with the same key, it finds it all.

- **Signed in the list**: its name, its icon (`desktop`, `laptop`, `server`, `phone`, `tablet`), its
  colour (`grey`, `red`, `orange`, `yellow`, `green`, `teal`, `blue`, `purple`), its role, who added
  it.
- **What it says itself**, kept by the hub: what it is (`computer`, or `phone` for the app), its system
  (`linux`), Flottille's version (from its hello, at each session). It is what tells a device that
  cannot run agents (`phone`) from one that could: Settings explains and offers the command to make a
  controller or a viewer a runner only to the second (#1286), as a card that has said nothing yet is
  taken for one that could.
- **What the hub knows**: when it was paired, when it was last seen (a session that starts or
  ends).

`remote.members` says all of it (`protocol.md`); a member added before pair mode has no icon nor
colour until one is chosen.

### What exists stays

A hub set up before pair mode, and the machines in it, keep working as they are (`a_hub_updated_over_its_data_keeps_its_members` holds it: a `hub.json` of a hub of protocol 1, with its key, is read by the hub of today and its members are welcome): the list a device
checks may start with the first member's own entry (the root of a list made then) or with the
hub's (one made since), and the members' additions of before still check. Every device pinned the
hub's key already, so the hub's changes after them are taken. From the first change the hub signs,
no member adds a device: an addition a member signs after it is refused, by the hub and by every
device. `hub.json` of the hub keeps its version; a setup code left in it is read and dropped. The
maintainer updates the hub and the machines together: a device of before pair mode refuses a
hub's change as one no member signed.

**Built (#1146), from the command line**: `flottille-hub pair`, `role`, `remove`, `devices`,
`reset`; `flottille remote pair`, `members`, `rename`, `icon`, `colour`, `remove`. The clients sign
with the device key; the daemon carries what they signed (`remote.*`, `protocol.md`) and says what
changed as events (`remote.*`, `events.md`). The phone pairs through the mobile core ("The Android
app"); its screens are #826.

**Built (#1177), in the window**: the page tells the hub's truth. A hub that answers that it no
longer knows this machine (its list was reset, or it dropped this device: `hub.forgotten` in
`remote.machines`) is said at the top of Devices, `The hub no longer knows this machine`, the kept
list greyed as old, and **Pair again** opens the pairing dialog in place of Connect to your hub; a
successful pairing replaces the old membership (`remote.pair {replace: true}`). While the hub
refuses or cannot be reached, a device's Rename, Change look… and Remove… are in its "⋯" disabled
with the reason in one line, and the section says `Changes wait for the hub, which cannot be reached
now: …`; one click is one request, its buttons busy until it is answered, a failure one toast, said
in words (what happened, what to do). Removing this machine says it leaves the hub and its links
close. Each device row, and the machine of each session of the sidebar, shows its icon in its
colour (a member with none gets a desktop and the colour of its key), and **Change look…** opens a
dialog (the five icons as a select, the eight colours as swatches, a live preview) whose save is a
signed change as `flottille remote icon` and `colour` make. The page shows no addresses (every link goes through the hub, decision 3): its Addresses section is gone, and so is the place that asks where this machine listens; the direct link stays in the daemon, unused here (`flottille remote listen` still sets it).

**Built (#814, #1146, #1144), in the window**: Settings, Machines, a tab of its own, says first
whether remote is on (a **Turn on** button while it is off, a **Remote** switch once it is, whose
off asks once), then offers **Connect to your hub** while this machine is in no member list: one button, which opens a dialog where the code is pasted
(a field that hides what is typed), and the card this machine proposes confirmed (its name, This
device is, a select of the five icons, This device can, Run agents / Control only / View only);
a refusal stays in the dialog, a success closes it with a toast. There is no Add a device and no
join request: a device is added on the hub's host. Then the page shows the devices of the
list (each row its name, its machine's state, its role and who added it; Rename and Remove… in its
"⋯", Remove asking once; the row of a controller or a viewer, which runs nothing and so is not
offered in Settings or the composer, says so and shows `flottille-hub role <name> runner`, which
changes it on the hub's host, copyable, #1247; a phone, which never runs agents, has only its role in
words, #1286: the window tells it by what the device said it is, `phone`, and never by its icon or its
name); #1177 took this machine's addresses off the page.
Every new member is a toast, `New device: pixel (control only), added at 18:42 by the hub`, with
Remove. The sidebar lists the sessions of every machine ("One list of every machine"); opening one
of another machine shows its page, read on that machine's own connection, every request signed with
the device key, as the command line signs it, and carried by this machine's daemon; the rest of the
window stays as it is ("Built (#1271)"). Sharing a
vault entry or a connection from the machine shown shares that machine's: the request goes to it
with the device's signature for it, which it checks and sends on with the secret, as when it is
asked on that machine; the sign-in of a copy on another machine comes back to this machine's
`127.0.0.1` whichever machine is shown. A session of a machine that is off is greyed and does not
open; opening a page of a machine that needs an update, or that went off meanwhile, is a failure
toast that says why, and the page shown stays. The window reads the
device key from the same keyring item, or the same file, as the command line.

**Built (#1182), in the window: every page works on any machine.** The window addresses another
machine one way only, the one its sessions use: that machine's own connections, each request signed
with the device key for it and carried by this machine's daemon (#1271: nothing switches, below). Settings has the
machine's picker on its title's line, at the header's right, the title its label (`Settings of`
to a reader; #1244): the kit's select, this machine, then the other runners, each with its role and
state, `runner · online`; with no other runner it narrows away and the title stays in place;
choosing one shows its Settings, and every tab that is a machine's (Agents, Approvals, Features
and Connections, Vault, General's about, workspaces folder and developer mode) shows and changes
that machine's. What is the window's own stays this machine's, asked of its own daemon whichever
machine is shown: the appearance, the version and its update (`update.*`), the folds and drafts the
window keeps; one line, the first of what scrolls under the tabs, says it while another machine is shown (`This window
(appearance, version) stays this machine's, pc, whichever machine is chosen above.`). Settings, Machines is the hub's list, the same from
every machine. **Add project** asks first on which machine (radios: this one first, then the runners,
each with its role and state; Next), then the folder there (Back returns to the machine): on
another machine it is picked in that machine's folders, never typed, from its home, a folder opened
after the other (`project.folders`, `..` going up), each saying whether it is a repository and how
many it holds, or that it is already a project; a repository is picked, then detected and added
there. On this machine it is typed or picked with the system's dialog, as before. A
project of another machine only opens that machine's page of it; its settings and every tab
follow. A page of another machine says its machine once, where the page names what it shows
(Settings: its machine picker; a session: its header), never in a path line above it (#1271).
While the machine of the page shown is off, or this
device is a `viewer` there, one line above the page says why (`server offline since 22:14`, `This
device may only view server`), and the controls that write (Settings' sections, a project's
settings, a project page's composer and its tab, Add project's button) fade their ink and take no
click, while the page still scrolls and its tabs still read; nothing is sent, nothing is kept. A
machine that is off is not opened (a failure toast says it, as for its sessions): what the window
shows of it is what it read while it was on.

**Built (#1271), in the window: it holds every machine at once, and nothing switches.** Each
machine the window shows a page of has its own connections, made at the first need with the device
key (read once for the window), then kept open while the window runs and made again with a backoff
when they break; this machine's own connection is never cut to show another one, and the list,
its folds stay this machine's whatever page is shown, and the bell holds every machine's (#1280). What the window holds of a
machine is kept apart, by machine: its Settings and each of their sections, the parts of Flottille
it runs, its providers, the forms and pages of its projects (another machine may have a project of
the same name). A page of machine A never shows a value of machine B: what B answers fills B's
own copy, wherever it waits. A page reads only what it shows: a session of another machine reads
that session on that machine (its page, the latest messages first), Settings of another machine
reads that machine's settings, and a project's page of another machine reads that machine's
projects and sessions by a link of its own, whose events keep them current while one of that
machine's projects' pages is shown; nothing of another machine is read for a page that does not
show it. Coming back to a machine already read shows its copy at once and refreshes it in place:
the page of the last session of each machine keeps following it, and its Settings are as they were
left, then read again. Its Settings follow a change made there by another client while the window
holds them (#1476): the bell link of that machine, which hears all of its events, says each change
of its settings (`about.changed`, `developer.changed`, `rules.changed` and the other settings
events), and only the section it names is read again; no other connection, no read of the whole
machine. Its providers follow the same way (#1483): a `provider.*` event of that machine has its
bell link read `provider.list` again, on a thread of its own (one read at a time, one more when
events came meanwhile), so that a machine slow to list them never delays that link's notices, and
the page shows the answer.
That link resumes its events after the last one it saw (`events.watch` with `after`) each time it
connects again, so a change made while it was cut is heard when it is back, not lost. The first time, the
page says it is reading that machine (its own skeletons, in the pane). A machine that does not
answer within the window's limit (30 s) is said on its page only, `server does not answer`, a
line above it that folds away once it answers or another page is shown; the rest of the window is
untouched and nothing goes back to another machine; only that machine's answers count, never its
link's own words (cut, made again). Each request is still signed for its machine alone.

Every page asks its own machine, whichever machine's page is shown when it asks: a session's page
and its composer take the actions of their session's machine when they are made, so the kept page
of another machine's session gets its pictures there while it is hidden, and every answer of that
machine (a message sent, a mode or a model changed, an interruption, its draft) reaches its
composer; a Rename, from a row or from the page's header, goes to its session's machine. Each
section, dialog and page of a machine (its Settings and their checks and findings, a project's page,
its settings, its forms and its spec) holds its machine from the moment it is made: whatever it asks
later (a dialog's button, a list read again after an answer, a toast's action, a read after a quiet
while) goes to that machine even once another is in front, or while another is still being reached.
A project page's details and its pull requests are its machine's: one of another machine never
fills the page shown nor a session row of this one. The version and its update are this machine's
own, in every machine's Settings. An agent's request to write a connection is the dialog of the
machine whose page opened it, closed if that machine is removed.
Add project's choice of machine is the form's own: the project is added there, held only while that
machine takes no change, and the page behind stays shown, its machine in front. Back and Forward
return to the page of the machine it was a page of (Settings, a project's settings and a spec are
each machine's own); one of a machine that cannot be shown now is said so, and the history stays
where it was. What a machine answers acts on its own pages only: a project it removed leaves its
copy, never this machine's list, and a session started from its project's page opens, its row in
the list at once with the first line of its brief, until that machine lists it (a minute at most).
A read of the user's machines that fails lets go of none of them.
The link of a project's page of another machine lives while a project's page of that machine is
shown, its events filling that machine's pages; left, it ends at once, idle or not, and the page
coming back reads again. A machine removed from the user's lets go of everything the window held of it. Another
machine's Settings chosen in the picker enter as a page that shows up, staggered.

## Roles

The user chooses what a device may do on the device when it pairs, from what it proposes; the role
is in the signed entry, and changes on the hub's host only (`flottille-hub role`).

| Role | For | May |
|---|---|---|
| `runner` | a computer, a headless server | run agents; and, from its own clients, everything a `controller` may |
| `controller` | a phone, a laptop without agents | drive without running: read, send messages, answer questions and permission requests, start and stop sessions on a runner, switch the mode, open tunnels, remove, rename and restyle devices. The default for a phone, and for a computer with no agent signed in |
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
- **No member adds a member; a `controller` or a `runner` may remove any member, the root of a list
  made before pair mode included**: the root is where the member list starts, not a member above
  the others. The hub adds, and changes roles, on its host.

## The version

When two devices link, or a device connects to the hub, each says its Flottille version and the
protocol versions it speaks (a number, `protocol` in `daemon.status` too, raised by any change of a
method, an answer or an event that an older client would misread). They use the highest one both
speak; with none in common the link is refused on both sides with a sentence that says which to
update: `server runs Flottille 0.4.3 (protocol 9), this machine 0.4.1 (protocol 8): update one of
them (flottille update)`. The hub is said as the hub, with the side to update named by the older
protocol: `The hub runs 0.1.0 (protocol 1), this machine 0.4.0 (protocol 2): update the hub on its
host.` or, the hub being the newer, `…: update this machine.`; the hub sends its own hello with its
refusal so that a device it refuses says the same. The window and the app show the machine as **needs an update**, never as
offline.

**What the hub's hello proves.** On an `http://` hub, anything on the way could add a protocol to
the hub's hello, to have the device say it needs an update, or take one out, to have both speak a
lower one. So the hub signs its hello with its key, in its handshake and in its refusal before it,
over `flottille-hub-hello-v1`, a zero byte, the device's key, both nonces, the device's hello as
the hub received it (which it chose the protocol from) and its own, each its version and its
protocols (`crates/flottille-link/src/control.rs`), so that it holds for this session only and
does not check for a device whose hello was changed on the way; its
refusal before the handshake carries its key and its nonce for it. A device says it needs an
update, or the hub does, and speaks a lower protocol than its newest, only from a hello the key it
pinned signed. A hello whose signature does not check (one of the two hellos was changed on the
way) is the hub not reached. A hello unsigned opens a session only when it offers the device's
newest protocol, nothing being lower then; otherwise the device says the hub could not be reached,
and keeps trying. Something on the way that takes the signature out and changes the device's hello
can still have a hub of this version speak a lower protocol than the device does: the two then
disagree on the protocol, and the device draws no verdict from it. A hub of before 2026-11-22 signs nothing: a device of this
version reaches it while it speaks the device's newest protocol, as before, and otherwise says it
cannot reach it (its words say that an older hub is updated on its host) instead of that it needs
an update; a device of before reads this hub's hello as it did, the signature beside it ignored. At first each side speaks one version; the list lets the Android app, published apart,
speak two.

A new kind of message that a machine of before refuses alone, as the projects of a runner (#1181),
leaves the number: that machine says it so in its log, and nothing else of the link is lost.

Pair mode (#1146) raised it from 1 to 2: its frames and its changes (`hub`, `pair`, `role`,
`look`) are ones a build of before cannot read, so that a device and a hub of different builds say
to update rather than refuse each other's frames without saying why.

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
  message kept would arrive hours later, after the user forgot it. **One written exception: the
  close of a session** ("One list of every machine"). A close cannot surprise anyone hours later:
  it is kept, and sent when that machine links again.
- **Paths are picked, not typed**, from the lists the machine gives, except where the user types one
  on purpose (`flottille --machine server project add ~/code/app`, which is that machine's path).
- **Pictures** travel as bytes (`agent.send {images}`), in chunks when the message would not fit
  a frame ("Pictures sent to another machine"); files other than pictures and pasted texts are left
  for later.
- **A link or a file an agent shows** (`web` `show_link`) belongs to its machine: a `localhost`
  address of another machine opens through a tunnel ("Tunnels"); a picture or a video of another
  machine is sent by that machine ("A file an agent showed"); any other file of another machine is
  shown with its machine and path, and never opened here (a file of another machine is not on this
  one).

### A file an agent showed

A picture or a video an agent showed (`web` `show_link`, a file of its machine) is displayed on
any device (#1150): the client asks the session's machine for it over the link, and the machine
sends it in chunks.

- **Only a file the session showed is served**, named by the session and the number of the
  `agent.link_shown` event that showed it (`agent.file {agent, shown, offset?, length?}`,
  `protocol.md`), never by a path: the method takes none. The event must be that session's, say a
  file, and the file must be a picture or a video by its extension (`.png`, `.jpg`, `.jpeg`,
  `.gif`, `.webp`; `.mp4`, `.m4v`, `.webm`, `.mkv`, `.mov`, `.ogv`). It is opened again at each
  chunk on the checks Open makes (no link followed, a regular file, no execute bit), and the file
  opened must be the very path recorded when it was shown (`/proc/self/fd`): a folder of its path
  made a link since leads elsewhere and is refused. It must also be the very file: the device and
  inode recorded at show time are compared at each chunk (another file put at the path is
  refused), and a file with more than one hard link is refused, since another name of it may be
  outside the place.
- `agent.file` is a `read` method: a `viewer` sees what a session showed as it sees the
  conversation.
- **Bounds**: a chunk is at most 512 KiB (in base64 it fits a link's 1 MiB frame), a file at
  most 2 GiB, and a machine serves 2 MiB a second, all askers together (less than a third of the
  requests a device's signature allows in five minutes). A chunk that would wait more than 3
  seconds for its turn is refused at once (`refused`, "busy: ..."), and so is a call when 8 are in
  flight already; the asker tries again, so that one device asking without end never keeps the
  others waiting. The window asks again, half a second apart, up to 20 times.
- **Each chunk is one signed request**, on a stream of the link of its own; a seek asks for the
  range it needs (`offset`), with nothing before it read.
- **In the window**, the line of a picture of another machine draws a placeholder that fades into
  the picture once it is in (`awaiting`), with **Open**; a video's line has **Play**. Both write a
  copy on this machine (a folder of the window's own in `$XDG_RUNTIME_DIR`, else in a folder of the
  user's in the temporary folder that no one else can enter, readable by the user alone, removed
  when the window quits; leftovers of a window that did not quit are removed when the next one
  starts; files are made new, a link in their place is never written through), hand it to the
  system's default handler as soon as its first chunk is in, and the rest arrives into it while it
  plays. Two Opens of a link share one copy, and a whole copy is given again, never fetched again.
  A session's id that is not 1 to 64 letters, digits, `_` or `-` is never put in a file name: the
  Open is refused. The kit has no video
  element: a video plays in the user's own player, never in the window.

The command line takes `--machine <name>` on every command (`FLOTTILLE_MACHINE` too). The window
shows the sessions of every machine in one list ("One list of every machine", decided by the
maintainer on 2026-11-20); a led session on another machine appears under its pilot with the
machine's name.

### Pictures sent to another machine

A message to another machine is one signed request, one frame of the link (1 MiB): in base64, about
700 KB of pictures in all. A picture the user takes or pastes reaches the session whatever its
size, up to the limits a local message has (#1187), as a file an agent showed comes the other way:

- **The client sends the files first**, each chunk one signed request on its own stream
  (`agent.upload`, `protocol.md`), at most 512 KiB of a file, in order, then the message
  (`agent.send` or `agent.start`) that names their upload (`upload`) instead of carrying them. The
  window and the phone do so when the message, signed, would not fit a frame; a smaller one still
  carries its pictures. Pasted texts sent as files go the same way.
- **The receiving machine keeps them under the bounds of a local message**, judged at each chunk
  before any of it is kept: 20 pictures (png, jpeg, gif, webp) and 20 pasted texts, each 5 MiB at
  most. An upload is its sender's: the device that signed its chunks (this machine's clients
  together), and no other client can add to it or take it. A sender has 2 uploads waiting at most,
  the machine 8; the files are written in a folder of the daemon's data (`uploads/`, mode 0700),
  under names of its own (`picture-3`), never one the sender gave, and the upload's name is 1 to 64
  letters, digits, `_` or `-`.
- **An upload whose message does not come is dropped** 2 minutes after its last chunk, with its
  files; a daemon that starts removes what one that stopped left.
- **The message takes them** as it would have taken them inline: judged, scaled and written in the
  session's folder, then the upload is gone. A picture that arrived in part is refused, saying
  which (`picture 2 arrived in part: …`); every refusal of a chunk says which picture and why, and
  the composer shows it as a local refusal.

### One list of every machine

- **One list.** The sidebar holds the sessions of every member machine together, grouped by
  project: a project that several machines have is one group, whatever its name there (the next
  bullet). Sessions
  are ordered by last exchange, as on one machine. A session of another machine carries that
  machine's name as a tag (`server`); a session of this machine carries none. The machine is
  chosen where a page is a machine's: Settings' header and Add project's `On` (#1182, #1244), and
  in a project's composer (the machine picker) and its settings' header (#1266); the
  machines' states (online, offline since, needs an update) are in those choosers and in Settings,
  Machines.
- **A shared project's page** (#1266) lists, after its places, the sessions of each other machine
  that has it, a block a machine with its icon, its name and its state, each session with the
  machine's icon (`docs/design.md` §10). A session starts on this machine when it has the project
  and can run an agent, else on the one machine online that has it; when several do and this one
  cannot, the composer asks which before it starts anything, and the answer is kept for the
  project by the window. Its settings switch between the machines that
  have it, this one first. The page, the start choice and the settings switch follow the
  sidebar's grouping (#1410): the machines they offer are exactly those the project's row marks,
  each by the name the row holds of it ("the same project" below), and the row of another machine
  alone opens that machine's project, never one of this machine's the row does not hold. A project
  another machine has under another name, with the same remotes, is offered there, and choosing
  that machine shows its page or its settings of it, by its name there; a project of the same name
  with other remotes, or with no remote, is another row, and not offered. A machine the row does
  not mark is never reached by the name (#1439): the window's machine switch, on a project's page
  or settings, shows that machine's own Settings, not its project of the same name; and when this
  machine lacks the project and none of the row's machines is online, opening it says so (`app
  cannot be started`, each machine with its state) and starts nowhere else, a third machine's
  project of the same name included.
- **The same project.** Two projects are the same when the remotes of their repositories are the
  same, each normalised (`github.com/acme/app` for `https://GitHub.com/acme/app.git`,
  `git@github.com:acme/app` and `ssh://git@github.com:22/acme/app/`: no scheme, user, port nor
  `.git`, the host in lower case, the case of the path kept), and a project of several
  repositories matches when all of them match. A repository with no remote, or at a path of the
  machine, says nothing: its project matches none. Each runner tells the machines it links to its
  projects by those remotes in a message of its own, after its sessions (`projects`, "What a
  machine may send"; `remote.sessions` says them), and the copy kept for a machine that is off
  holds them too, so that its projects stay in the sidebar, greyed. A machine whose list came before projects were
  listed says none: its sessions' project names are matched by name, as before. What another
  machine lists matches a project of this machine only by the remotes this machine's own daemon
  says of it. The message is
  one a machine of before does not know, so that it refuses it alone (`not a message a machine may
  send`) and goes on taking the sessions: the protocol number is not raised, two machines of
  different builds keep their lists of sessions, and the one that sent the projects says in its
  log, once for the link, that the other runs an older Flottille (`flottille update`) and sends
  it no more of them; the older machine, which draws no project by machine, loses nothing. The
  remote's address is the only thing of a repository that travels in this list: no credential (a user or a
  password in the address is dropped), no path of the machine. The window draws one row for a
  project the machines share, with a mark for each (`docs/design.md` §10).
- **Live everywhere.** A runner tells each machine it links to its list of sessions, light (the
  `sessions` message, "What a machine may send"): when the link is made, and again whenever it
  changes (a session that starts, ends, is written to, answers or asks). The window reads it from
  its own daemon, which follows every machine linked to it, and the rows move as the lists do. A
  list is kept only from the link now current, and only for a machine that is a runner among the
  members: a machine removed, or added again as another role, loses its list. A list that names an
  id twice or whose pilots make a cycle is refused. There is no rate limit on `sessions` (a
  follow-up).
  Reading, writing, answering, Allow/Deny and Stop work in any session through its machine: opening
  a session of another machine shows its page, read on that machine's own connection, every request
  signed for it and carried by this machine's daemon, its conversation watched live as a request carried
  is ("Requests from another machine's clients").
- **Led from another machine.** A session led by an agent of another machine (`agent_launch`
  `led` with `machine`) shows under its pilot's row, with its machine's mark, on every window, as a
  led session of the same machine does (#1340). The list's `pilot` is a session of the same list
  only (an id of another machine there would make every machine of before refuse the whole list,
  whose fields are closed), so a runner tells its peers these pilots in a message of its own,
  `pilots`, after its sessions and whenever they change: each led session's id, its pilot's
  machine by name and the pilot's id there. A machine of before refuses that message alone, as
  it refuses `projects`, and goes on taking the sessions: there such a session stays a root of its
  machine, as before; a list that comes without `pilots` draws as before too. A receiver keeps it
  beside the sessions and takes it only for what it is: a pilot is said of a session of the
  sender's own list, of a member machine other than the sender, and a window puts the row under
  it only when the machine named lists that pilot. On the pilot's machine the window takes its own
  launches for it and none of these claims: a list never puts a row under a session of the machine
  that reads it. On the session's machine the window reads it from the session itself (its
  `led_by`, the stand-in of its pilot, and its `launched_from`). A row whose pilot is in another
  project row, or is no row, stays among its own project's and says who leads it (`led by agent
  r1 of pc`), as a session led from another project does. A session detached is the user's: under
  no row. A member machine is not trusted to be right: when two machines' claims close a loop of
  any length (`s1` of server under `r1` of lab, `r1` under `s1`), the window drops the claim that
  would close it, and any row it still finds in no walk from a root it draws as a root, so a lying
  `pilots` never hides, moves or duplicates a row. A row whose line gains its machine's mark, once
  its pilot's list comes, fades from one line to the other.
- **A machine that is off.** Its sessions stay in the list, greyed, their machine's mark under a
  hollow ring saying `server · offline since 22:14`, and cannot be opened. Each machine keeps, in its store, a light copy of each other
  machine's list for this: each live root session's title, project, state and last exchange, never
  its conversation, the copy replaced whenever that machine lists again, kept across a restart.
  A machine lists only the sessions of the projects it knows (the window of that machine shows
  those and no other): a session of a project whose file is gone, or whose file cannot be used (a
  typo), is not sent until the file is back, since the machine would answer that it has no such
  project (`docs/projects.md`, "Removing a project"). A folder of project files that cannot be
  read at all says nothing of any project: then every session is sent.
- **Closing such a session is allowed.** It leaves the window's list at once. This machine's
  daemon keeps the close, the user's `agent.stop` signed by their device, stored before it is sent
  like a delivery ("Deliveries across machines"), and sends it once when that machine links again
  (the `close` message); that machine checks the signature, the role and that it is its own, and
  stops the session as the device's request. The other devices see the session closed when that
  machine comes back. It is the written exception to "what the user sends to a machine that is
  off is refused": a close cannot surprise anyone hours later. A close is served once: the receiver
  remembers the nonces of the closes it served, for their 30 days (bounded, 4096, in memory), and
  answers one sent again and does nothing; it also does nothing to a session started or resumed
  after the close was signed, which covers a replay after a restart. A close signed by a device
  the receiver's member list does not hold is answered *for later* (its list may be behind): the
  sender keeps it and sends it again; any other refusal (a viewer, the wrong machine or method,
  older than 30 days) is final, and the sender drops the close and says so in the inbox, naming
  the session (its title if known, else its id). Asking again for the close of a session whose
  close waits replaces the one waiting. A close kept 30 days is dropped, as any message kept.

## What a machine may send

On the `machine` side of a link, a machine sends only these messages, signed with its machine key,
each naming the agent of its own that it acts for, and the receiving machine checks each against
what it recorded (that this agent of that machine started that session, or is that child's parent):

| Message | What it does on the receiving machine |
|---|---|
| `launch` | Starts a session or a child for an agent of the sender (`agent_launch` with `machine`), judged by the receiving machine's gate ("A tree across machines"). |
| `git` | Sends a bundle of a place's commits and its uncommitted work, or of a session's new commits on its way back ("What travels"). |
| `deliver` | A delivery for an agent of the receiver: a report, a question, an answer, a request forwarded, `session-turn-ended`, a child's news, as `design.md` §5.2–5.3 make them. |
| `call` | One of the tree's tools aimed at a session of the receiver: one the caller started (`agent_send`, `agent_status`, `agent_stop`, `session_send`, `session_read`, `approval_answer`, and `review` `{start, agent}` of a led session, checked against the launch, `workspace` `remove` of the workspace made for it, and `session_losses`, the machine's own, which reads what a led session's workspace holds that is not saved for its pilot's End confirmation; a read carries the caller's fields as any other does), or any other, by any tool of the tree that addresses a session (`launch` left empty, `agent` the session, with the caller's `project`, `title`, `tree`, `mode` and signed `switch`): judged there as the same call of one of its own agents ("Sessions of another machine").
| `capabilities` | What a runner offers: its projects, its agents signed in, its free slots ("Roles"). |
| `share` | A vault entry or a connection the user shared, signed by their device ("Sharing"): `{"request", "entry", "secret"}`, the device's signed `vault.share` as the client wrote it, the entry's metadata, its secret; for a connection (`share_connection`), `{"request", "connection", "entries": [{"entry", "secret"}]}`, the signed `connection.share`, the declaration, and each entry the request names with its secret. |
| `addresses` | Sent by a machine of before 2026-11-21, whose direct link was removed then: answered `{}` and ignored, so that it still links while the other machine is updated. No machine sends it since. |
| `sessions` | A runner's live root sessions, light: each one's id, title, project, state, last exchange, and the session leading it there; 500 at most, a title 200 characters at most, no id twice and no cycle of pilots. Kept by the receiver, in place of the one before ("One list of every machine"). |
| `pilots` | The pilots of another machine of a runner's led sessions: for each session of its `sessions` led by an agent of another machine, that machine's name and the agent's id there; 500 at most, each session once, every id 1 to 64 letters, digits, dashes or underscores. Sent after the sessions and again when they change, kept by the receiver beside them ("One list of every machine"). A machine of before refuses it as a kind it does not know, and the sender stops sending it on that link. |
| `projects` | A runner's projects, each one's name and the normalised remotes of its repositories (`github.com/acme/app`), 1024 at most, 64 remotes of 256 characters at most each; sent after the sessions and again when they change, kept by the receiver beside them. A machine of before refuses it as a kind it does not know, and the sender stops sending it on that link ("One list of every machine"). |
| `close` | The user's close of a session of the receiver (`agent.stop`, as their device signed it), kept by the sender while the receiver was off. Taken when its signature is a member's that may stop a session, for the receiver, signed less than 30 days ago; served once (its nonce is remembered 30 days), and not for a session started or resumed after it was signed. |

Nothing else: no user method but the kept close, no `approval.approve`, no `agent.mode`, no `agent.send` as the user, no
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
- **A child there is a child** (#1274). On the machine it runs on, a `child` an agent of another
  machine launched has no parent, and is a child all the same: a child's tools (none of the
  project's actions, no `session_*`, no `agent_*` at the depth limit), and its parent's depth plus
  one, so the depth limit holds across machines. The launch carries the caller's `depth`, left out
  at 0 so that a machine of before reads it; a child asked for by an agent at the limit is refused
  there (`agent b1 of pc is at the depth limit (1): it cannot launch children`). `agent.list`,
  `agent.show` and the tree say whose it is, `launched_from: {machine, agent, kind}`; the window
  draws a bar naming that agent where the composer would be (`This conversation is a child of agent
  b1 of pc, which launched it from there: only its parent writes to it.`), the CLI says `child of
  agent b1 of pc`, and the phone lists it among no sessions. `agent.send` to it is refused, as to
  a child here (`agent a2 is a child of agent b1 of pc: only its parent writes to it`); the user
  still answers its questions and stops it, the ways out while its parent's machine is off ("Where
  a question goes when the parent is off"). The other methods of the user that a child here refuses
  are refused for it too, each in the same words as for a child here and naming its parent's
  machine: `agent.unqueue` and `agent.send_now` (a message its parent queued is its parent's to
  take back or send now), `agent.cancel`, `agent.rename`, `agent.quota_resume`,
  `agent.reload_tools`, `agent.resume` (its session is not kept for the user), and `agent.mode`
  (its mode is its session's, set on the machine of its parent's session: no switch here sets it
  apart from its tree). The inactivity sweep never ends it (it ends with its parent), a review is
  not asked for it, `session_title` is refused to it, and its report and failure are no
  notification or inbox entry of the user: they go to its parent. Its question to the user while
  its parent's machine is off stays the user's. It is subscribed to nothing automatically, and
  gets its brief again after a compaction, as a child does. A `led` or `handed` launch asked for by a
  caller below its root (a `depth` above 0) is refused there, as its own machine refuses it. A
  child stored by a Flottille of before, at depth 0, or at depth 1 with the agents it started
  here left at theirs, is put at its caller's depth plus one when the store opens (1 when its launch
  names none), so it takes a child's tools at its next start, and each agent under it one depth
  below its parent, so that a stop ordered by depth ends them before it; a depth is only raised,
  never lowered. Such a child that had ended then was kept to be resumed, as a root: the next
  sweep forgets its session and its images, as a child's are when it ends (#1368); a child that
  ends now keeps no `reopens`, so that the sweep never forgets it a second time (#1383). An agent
  it starts here is a child of the tree of its parent's machine too: a method the user's door
  refuses for a child names that tree's root, on that machine, as the one the user writes to, and
  `agent.mode` is refused on it as on the remote child.
- **The machine must be a runner, and online**: otherwise the call is refused at once, naming since
  when it is offline, or that it runs no agent. It must have the caller's provider signed in and a
  free slot ("Roles"): otherwise refused, naming the runners that would take it, if any (`refused:
  server has no slot free for project app (2 of 2 run); laptop would take it`). It need not have
  the project: the project travels with the launch (below).
- **Which project** (decided 2026-11-21, built #1188). The receiving machine recognises a project
  it has by its repositories, whatever its name or folder: each repository of the launch at the
  same path in the project, with a root commit in common and, when both have a remote, the same URL
  (`https://github.com/acme/ledgerly.git` and `git@github.com:acme/ledgerly` are the same). A
  project of the launch's name whose repository is not there yet takes it too: it is received
  there. One it has: a new workspace on the caller's branch, only the missing commits sent. None:
  it makes the project itself, under the caller's name of it (or `-2`, `-3`… when that name is
  another project's), in its folder for projects from other machines,
  `$XDG_DATA_HOME/flottille/projects/<project>` (the project's, not Flottille's, to the gate, as
  a workspace in the data folder is: a session the user starts there writes in it); each repository is cloned from the bundle, its
  whole history the first time, never from its remote; then the workspace and the session as
  above. The project is then that machine's like any other, on its side of the list, its file
  marked with that machine's key (`from_machine = "<key>"`): its member's name is not kept, since
  a runner may rename any member and another may take it. The words of the launch say which
  project it runs in, and when it makes one.
- **The project's settings travel with each launch**: its branch rules (`branch_prefix`,
  `branch_format`), `never`, its tasks and services (`tasks`), its `setup`, its `message_checks`,
  its pull request conventions, its `rules` and `rules_alone`, and each repository's `base`,
  `depends_on` and `commit_checks`. A project Flottille made takes them, each replaced by the
  caller's at every launch, **once the launch is allowed** (by the gate at once or by the user's
  answer): a launch held or refused lays nothing. Every other key, its folder, its workspaces'
  folder, its caps, its provider, its forges, its tracker and its `variables` (whose values may be
  secrets: they never travel, and a launch that sends them is refused), stays that machine's own.
  A project the user registered by hand keeps the settings the user gave it there: the caller's
  are not laid over them, and the launch's words say so. A project made for one machine's launches
  takes only that machine's: another machine's launch runs in it and leaves its settings as they
  are, its words saying whose they keep (a project whose file names the machine instead, as the
  first builds of dev wrote it, is no machine's: it keeps its settings; removed, the next launch
  makes it again). The settings
  are laid only on the project file they were classed against, as a held write is done only while
  its words still hold: when the user (or anything else) changed that file meanwhile, the launch
  is refused when it is allowed, saying the project changed, and nothing is laid; the caller
  launches again. Settings that cannot be used there (a task needing a
  service that machine lacks) refuse the launch, saying why.
- **What the settings change is classed as the same writes of an agent there** (`docs/tools.md`,
  "The project"), by the receiving machine's gate, against the project as it is there: each task
  added or changed as `tasks add` or `edit` (its line judged as a run of it: `sudo`, a never-listed
  line or a sensitive path is base; asking turned off, `check` or `read_only` set are base), each
  setup step as `setup add`, the rules and `rules_alone` as `set_rules` (base), each repository's
  base branches and dependencies as `set_settings`; what no agent's tool writes there is the
  user's word, base: an entry removed from `never`, the message checks and the commit checks, the
  branch and pull request conventions (its template is text given to the agents, as the rules
  are), and a repository the project lacks, which is checked as one the user adds there: no link
  out of the project's folder, no repository of another project (`It adds the repository lib to
  the project`). A repository not there yet is checked too: no folder on its way may lead out
  through a link. It is checked again when its code comes, right before its folders are made, and
  those are made one at a time, a link at any of them refused: nothing lands outside the project
  through a link planted after the launch was allowed. A link is refused wherever it leads,
  inside the project too (`sub is a link: nothing is made through it`): following it is what a
  link planted since the check would use. A file laid from the vault is written by walking those
  folders again, each opened from the one before without following a link, so one swapped for a
  link after they were made is refused too. The commits a machine lists (`Has`) are those of a
  repository inside the project folder only. In a project Flottille made, a repository
  whose folder leads out of the project is never the launch's. The
  launch takes the strongest class: one base change makes it the user's to decide, never the
  judge's (in Full it passes, as a base write does); a line the gate refuses refuses the launch.
  Its words name each line and key that changes (`It adds the task deploy: `make deploy``, `It
  turns off asking before the task deploy runs`, `It empties the never list, which was `rm -rf``),
  and the request the reasons of each.
- **A project made for a launch that does not start goes with it**: refused by the user, its agent
  ended while held, or swept for its code that never came, the project made for it is removed,
  its file and its folder, once its workspace is gone, unless another launch of the same machine
  may still start in it. Two launches of a new project at once make it once: the second takes the
  project the first made.
- **The folder for projects from other machines** is a setting of the receiving machine
  (#1214): Settings, General, "Projects from other machines" (`travelled_folder.set`), on that
  machine or from another's window through "Settings of"; `$XDG_DATA_HOME/flottille/projects`
  until the user types another. It is checked as a project's folder is: absolute, reached through
  no link, neither a project's folder nor inside one, outside Flottille's configuration and state;
  and it is no disk's root (`/`, a mount point), not the home itself nor Flottille's data folder
  itself, and holds none of Flottille's configuration, state or data folders.
  Only the projects made afterwards go there; those made before stay where they are.
- **A setup that copies files the receiving machine lacks** (#1214): the caller's machine names,
  with the launch, the files its project's `copy` steps match in its folder, their paths only,
  never what they hold. Each that the project there lacks is made from a vault entry,
  `<project>-<path>` (`app-.env`), which the user shares from the caller's machine through the
  vault's sharing, keyring to keyring: never a file between machines. The entry is bound to that
  file (`file:<project>/<path>`), and only an entry bound to it and shared from the launch's
  machine is ever laid: one of the machine's own, or another machine's, of the same name is not
  (it counts as missing). **A launch that lays a file is held for the user whatever the mode**,
  Full included, the judge not asked, and asks once, on every device, as any held launch does: its
  words name each file and entry there already (`app lays .env from the vault entry app-.env shared
  by pc`); one still missing starts them with `app needs .env on server: share it?`, its reasons
  name each entry, and its card's `needs` gives them (`{"from", "to", "project", "files": [{"path", "entry"}]}`,
  `project` the caller's name of it). On the caller's machine, `vault.from_file` makes the
  entry from the file, read only then, masked before it is kept; `vault.share` sends it. **Allow on
  that card does both** (#1262), whichever machine the window runs on and shows: on `from`,
  reached through this machine's daemon as Settings of another machine is and signed with this
  device's key as a share is, it reads the vault's entries, then for each file in turn makes its
  entry (unless the file's own is there already) and shares it with `to`, then allows the
  launch. **An entry is shared only when it is the file's own**: named as the daemon names it
  (`<project>-<path>`) and bound to that file (`file:<project>/<path>`); an entry of that name that
  holds another secret is left to `vault.from_file`, which refuses it, and a card whose `needs`
  names another entry for a file, or a `to` that is not the machine the launch is held on, is
  refused before anything is asked, nothing shared (the daemon shown is not trusted with what
  leaves `from`). The card names `from`, `to` and the project, so what the user consents to is
  what is shared; each step shows on the card, and the first that fails ends it, saying which file and
  why, the launch still held, Try again resuming at that step. Allowed,
  the receiving machine makes each file from its vault entry in the project's folder, never over
  a file, readable by the user alone, before the setup runs, whose `copy` step then copies it as
  any; one still missing refuses the launch, naming it. Refused, the launch ends and the agent that
  asked it is told what was missing (`child-failed`). A file is laid through no link: each folder
  to it is made one at a time, a link refused, the file never over one.
- **A launch refused on the caller's machine names the window's action**, never a command line
  (#1214): `claude is not signed in on server. Sign in to Claude on server in Settings › Agents`,
  `server is offline since 22:14: …. Settings › Machines says when server is back`, a machine that
  is no runner (Settings › Machines says each machine's role), a runner with no slot free (it
  takes one once one of its children there ends). The receiving machine's own refusals of these
  kinds are said the same way.
- **The making shows on the launch's row** (#1214): the receiving machine answers the launch with
  the project it runs in, whether it made it, and whose settings it keeps (`remote.launches`'
  `there`); the row of a launch whose project was made says `server · preparing ledgerly` until
  that machine's first delivery (its list saying `running` or `idle` does not end it; one saying
  it needs the user does), and the caller's answer says when the project there keeps its
  own settings or another machine's (`It runs in the project app there, which the user registered
  on server: it keeps its own settings, not yours`).
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
  machine. A held launch is answered as a call held here is (`Waiting for the user's approval,
  request #4 on server. …`), then where it will run once allowed. **The user's answer comes to
  the caller as a held call's result does** (#1273), an `approval` delivery from that machine
  worded as the local one, the machine named after the request: allowed, `Request #4 on server
  (agent_launch) was approved by the user, and Flottille has now done it. Its result:` and the
  answer of a session started there, which says what follows; refused, `… was refused by the
  user. Nothing was done.`; allowed but unable to start, `… was approved by the user, but nothing
  was done:` and why; its session ended there while it was held, refused, saying so. The allowed
  answer is sent before the session's setup goes on, so that a setup that fails at once is never
  heard of first. The caller's machine marks its record of the launch `started` or `refused`
  (`ended` when a started one is told refused); an answer that comes before the record of the
  launch's place is never undone by it, and the call then answers what was decided, never that it
  waits (#1317). The user answers a launch once: an answer to a launch answered already is refused,
  but the one stored, sent again by a machine that did not hear it was taken, which changes nothing.
  The caller is never left waiting: a held launch with no session there, or whose answer cannot be
  kept there, is told refused; an allowed one whose setup a restart of that machine cut never
  starts and is told so (`… was approved by the user, but nothing was done: it could not start
  there: Flottille stopped there while it was setting it up.`); a launch with no request number is
  named by its title (`Your launch "Docs" on server …`). The window keys a held session by its
  request and its machine (a name may hold spaces and dots), each machine counting its own, and says
  a session started when the call itself answers that the user allowed it before it returned. A restart
  there never stops a session that ran: an allowed launch whose workspace is ready and whose session
  was briefed only has its answer settled again. One a restart cut after its caller may have heard
  it approved (its answer settled `started`) tells that caller its stop as the end of the session it
  started (`child-failed`, or `session-turn-ended` for a led one), which its machine takes whether the
  approval reached it or not, and records `ended`, or `refused` when the approval had not reached it
  (#1374). A handed launch's caller hears that stop as a led one's does, `session-turn-ended`
  (#1385). The user's answer is kept for the caller's machine beyond the 100 messages (up to 104
  in all), cut to 64,000 characters, so that the end of a session that ran does not find its
  launch still waiting for it (#1385). A refused launch leaves no row under its
  pilot and counts in no cap. A handed session's caller is told too: it waited
  for that answer, and hears of the session after it only its stop, as above.
- **No chain without the user, across machines too** (`tools.md`, "No chain without the user"):
  a `led` or `handed` launch whose caller is itself a session another agent started (here, or on
  a third machine) says so (`launched`), and the receiving machine's gate takes it as base: held
  for its user in every mode but Full, never the judge's (`this agent is a session another agent
  started for the user, …`). A `led` or `handed` session a machine started for an agent of
  another is, to its own gate, a session an agent started: a session it starts comes to the user.
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

### Sessions of another machine

**A session on another machine of the user's is the same as one on this machine, for every tool
that addresses a session** (built #1250): an agent finds it, reads it, writes to it, stops it and
answers it with the same arguments, and gets the same answers and the same refusals; it need not
know, nor say, on which machine a session runs. The machine is where it runs, nothing more.

- **Found the same way.** A session is named by its id or its title (`tools.md`, "Sessions that
  read each other") among the roots of this machine and the live roots each other machine last
  listed (`sessions`, "One list of every machine"), a project of the same name being the same
  project. A title that two sessions share, here or there, is refused with both listed; every
  listing a tool gives holds the sessions of every machine, one of another machine said `on
  server` after its id (`Night lead · 0123456789ab · on server · idle, active 2 min ago`). An id
  two machines list is refused, naming both: ids are each machine's own. A child an agent of
  another machine launched is a child there too, never listed nor called as a session.
  A session the caller started there, named by its title, is called as by its id.
- **Judged by the machine it runs on, by the same code.** The caller's daemon sends the call as it
  is (`call`, signed with its machine key) and judges nothing of it. The receiving machine stands
  the caller in by an agent of its own, `<caller>@<machine's key>` (never an id of its own, never
  listed, shown as `agent <id> of pc`), in the caller's project by its name and in its tree's mode,
  and plays the call through the very path of its own agents' calls: the gate, the judge, the
  user's leave per pair, what is held for the user in every mode (the first message to a session
  the caller did not start, ending one, a first read across projects), the tool itself. A call
  held there is a request of that machine's user, answered at once as held; its answer comes back
  as the `approval` delivery it would be on one machine (`deliver`, its `launch` `request-<n>`,
  taken only by the agent that waits for that very request of that machine). Nothing gives a
  caller of another machine more than an agent of that machine: no other tool than
  `session_read`, `session_send`, `agent_status`, `agent_send` and `agent_stop` (a tool added to
  the tree later is not given until it is written there), and `review` for one call alone, the
  start of the review of a session it leads there ("What travels"), no secret, no file: when its project is
  not on that machine, its place is a folder that does not exist. Its answer, whatever its
  outcome (a refusal and a "cannot answer now" too), is masked again of the caller's machine's
  secrets, then cut there as a local result is (a refusal at the error limit), saying so. That
  machine's cut comes after the masking and splits no secret. The whole answer is masked first,
  a marker or what looks like one included; then, where the other machine cut it (the end of a
  text, `‹+N chars›`; the middle of a long result, its marker exactly as `run` writes it, on a
  line of its own; and in such a result each line end, as a long line is shown as several), the
  start of such a secret before the cut and its end after it (6 bytes or more) are masked too,
  so that no half of one is shown; an answer without
  an outcome, or a call without a project, comes from an older daemon and is refused saying that
  daemon needs updating. A machine stands in 64 callers of one other machine at most
  (one more is refused, saying so), forgets one idle for 30 days, and those of a machine that left
  the members, with their leaves.
- **The mode** is the caller's tree's, Full only with the user's switch signed by a device, as a
  launch's ("Starting a session on another machine"): a machine alone cannot raise it.
- **Only two differences, said in one machine's terms.** A machine that cannot be reached: its
  sessions stay listed with its state (`on server, offline since 22:14`), and a call is refused at
  once saying so (`refused: server is offline since 22:14: the session "Night lead" (agent …)
  runs there, and cannot be reached now; …`). A device that may only view: its agents' calls that
  only read (`session_read`, `agent_status`) are served, every other is refused (`refused: pc may
  only view: a controller or a runner may call session_send on a session of another machine`).
- **The version.** A call of a launch is sent as before; a call of another session carries fields
  a machine of before does not read, and is refused by it, saying so. No protocol change.

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
- **Ignored files do not travel**, nor the project's `variables`: a `.env` is the machine's own,
  taken by its setup's `copy` steps as today. A setup that misses one fails as it does today,
  saying which. Nothing is ever copied as a file between machines; a secret goes only through the
  vault's sharing. The settings that travel are sent as the caller's file writes them: a secret
  written by hand into a task's line would travel with it.
- **An uncommitted file holding a secret that is not ignored** (an `.env` missing from `.gitignore`)
  travels to the other machine with the rest of the uncommitted work, as it would in a commit:
  nothing refuses it nor warns.
- On the receiving machine the workspace is created from those commits, on the same branch name
  (with `-2`, `-3` when that name is taken there), prepared by the project's setup, and the
  uncommitted commit is laid on it as uncommitted changes. A repository the project has there but
  never received, and every repository of a project made for the launch, is cloned from the
  bundle, never from its remote: it has no remote there.

**Coming back.** When the remote session has committed and reports (`child-report`, or a led
session's `session-turn-ended` that reports), its daemon sends a bundle of **only the new commits**,
those past what the caller's machine sent or already has; the caller's machine verifies it and
fetches it into `refs/flottille/machines/<machine>/<launch>/<branch>`, and the delivery names that
ref: two launches from one branch come back to two refs, neither overwriting the other.
Nothing moves the user's branch: the agent or the user reviews and merges it (a `git merge` run,
judged as any), as a local child's work.

**An end brings the rest back** (#1278). A session or child that ends there with commits made
since its last report sends them back first, the same way: a led session's end
(`session-turn-ended` saying it ended, whoever ended it), a child's failure (`child-failed`), and a
child stopped but by its parent's own end (`child-ended`, `Your child … ended: stopped by its
parent, …`), each naming the ref they came into. A child that reported and ends after it sends
nothing more: its report did. What ends with its pilot or its parent sends nothing: nobody is left
to be told.

**The workspace there is removed as one here** (#1278). The answer of the launch names it (`in
workspace login there, …`), and `agent_stop` says it stays, as on one machine. The `workspace`
tool's `remove` takes the same optional `machine` as `agent_launch`: the caller's daemon sends it, through `call` and judged by
nothing here, to that machine, only for the workspace there of a session or child that very agent
started (`refused: you started no session or child on server whose workspace is login: …`
otherwise, nothing sent). That machine checks again that the workspace is the one it made for
that launch of that caller, sends that launch's commits back first, then plays the removal as one
of its own agents' removals (`tools.md`, "The workspace"), its caller stood in for in the
workspace's project there, in its tree's mode: held for its user as a removal of its own agents'
is, its card saying what would be lost, refused while an agent works in it, and done only with nothing lost or
by the user's answer to discard. Commits that went back count as kept, as commits another branch
holds do: what the card names is only what never reached the caller. Nothing of it is done, not
even the commits sent, before the call is found to be one that machine plays: its device's role
(`refused: pc may only view: …` for a viewer), its words, its workspace that launch's. The user's
answer comes back as an `approval` delivery, as any held call of another machine's, and is done for
that launch's workspace alone, whatever another launch's workspace in another project is named. In
Full, a removal that would lose anything waits for the user's word there, as a root's removal of
the workspace it works in does. Only a root removes one, as on one machine (a child is told
`refused: the agent the user talks to manages the project's workspaces; …`); another machine's
caller removes no other workspace, of any name, and `machine` goes with no other action
(`refused: with machine, workspace only removes the workspace there of a session or child you
started: …`).

**Built (#819), in the window.** While the window shows this machine, each led session and each
child an agent of this machine started on another (`remote.launches`, `protocol.md`) has a row
under its pilot's session, or the session whose tree holds its parent: its title (the launch's),
then the machine's name and its state: the one that machine's list says (`server · running`,
`needs you`, `idle`, #1280), else as last known, `running` (no delivery came yet), `needs you`
(held for the user there), `waiting` (its last delivery a turn's end, a question or a request),
`reported`, `failed`, or the machine's own state while it is off (`offline since 22:14`, `needs an
update`), greyed then. Clicking it opens that session's page, read there on that machine's own
connection, its header and its composer entering staggered. A report whose
commits came back names the ref under its step's title, folded or not (`Branch back:
refs/flottille/machines/server/<launch>/feat/export`). While a page of another machine is shown,
its question cards say `· on server` after their time.

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
- **A delivery from another machine is masked as its answers are** (a report, a question, a led
  session's turn, the answer to an approval): on the machine that takes it, before it is stored or
  shown, of that machine's secrets and of those of the secret files of the place of the agent it
  is for, the whole first, then both halves of a secret a cut of the other machine split (its
  `[cut: N more characters; …]` marker, and the end of a body of 64,000 characters, where the
  answer to a request is cut without a marker). So is every other text of another machine an
  agent here is shown: a message of its parent or pilot (`agent_send`, `session_send`), the reason
  of a denial, the brief of a session started for it. A place that cannot be read any more (its
  workspace or project was removed) holds nothing back: the text is masked of the daemon's, the
  vault's and what is left of the place's secrets, and delivered; a delivery for an agent that is
  not here is refused for good. Only a store that cannot be read is refused for now and sent again.

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
  question`), its questions said whole, and the requests of the session and of its children that
  wait for the user there. When that user answers one, the pilot is told, once, as a pilot of the
  same machine is (`session-request-answered`, queued there, ringing nobody: `docs/tools.md`,
  "Following a session", #1470): each is a message of its own while that machine is linked;
  while it is off, one message of the launch waits for it at a time, the latest listing every
  answer not sent yet (the newest that fit, when they do not all fit), replacing the one waiting
  in the same write, so that it is kept even when as many messages wait as may. Only a `led`
  launch takes one.
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
- **The screens, first**: pairing (Connect to your hub: the QR reader, the card), the machines and their state, the
  sessions of a machine, the session page (`design.md` §10): the conversation live, the composer with
  pictures, the ask card, the requests with Allow and Deny, the mode, Stop, the tree read only. A
  phone is a `controller` by default; as a `viewer`, the composer, the answers and the controls are
  not drawn. A project's settings come later.
- **The navigation, decided**: a bottom bar with three destinations. **Needs you**: each request
  and question as a card answered in place (Allow and Deny on the card, the whole call in sight).
  **Sessions**: the window's sidebar, each machine, its projects folded or not, their sessions in one
  list newest first (#1441: a session that reports never moves), led sessions indented under their
  pilot; the state is the row's mark, so rows never jump between groups. **Machines**: their state, their agents, their slots.
  The app opens on Needs you; a notification opens its session.
- **The connection to the hub lives in a foreground service**, as long as the device is paired: the
  same session every device keeps, which carries the links (#1151). Android requires its permanent
  notification ("Flottille is connected", silent, which the user can hide) and the app asks to be
  exempted from battery optimisation; a little battery goes to the keepalive. Links to runners are
  opened over it when the app needs them. A known problem of the transport's library on mobile
  networks is worked around inside `flottille-link`, never in the screens.
- **What a real phone must prove**: the service holds a whole night in deep sleep (Doze, and a
  Samsung's own battery rules), what battery it costs, and answering Allow or Deny from a
  notification with the screen off, the request signed and answered within the few seconds Android
  gives.

**The mobile core, as built (#824).** `crates/flottille-mobile` holds a `Core` over a folder of the
app's private storage that no backup copies (`FlottilleCore(context)`: `noBackupFilesDir`; the
manifest also excludes every domain from the cloud backup and the device transfer): the
endpoint's key is there in clear, and a sealed device key restored elsewhere opens with no
Keystore there.

- **Keys**: the device key, kept by the link's keyring logic in a file whose bytes the app sealed
  (`Sealer`: AES-GCM under a key of the Android Keystore, `KeystoreSealer`), pending until the join
  succeeded, as a computer's is. The endpoint's key, the phone's "machine" in the member list, is a
  key of its own in that folder with the machine key's care: a member's own key may not be its
  machine's. A device key the app cannot open (its Keystore key lost) leaves the phone not joined,
  saying to pair it again; the core still starts.
- **Joining** is the hub's pair mode: `join` takes the code of the QR `flottille-hub pair` printed,
  with the name and role the app proposed and the user confirmed (`controller` or `viewer`; the
  icon `phone`, the colour of its key). The phone pins the hub's key from the code, proves the
  secret, checks the list the hub signed back to the hub's key and its root, declares its endpoint
  and keeps its key. A join again (after a removal, a hub's reset, a key lost) makes a new device
  key and a new endpoint key, which replace the old ones once the hub admitted it. Every list after
  is checked against what it kept; one that does not check is refused and said in the view state,
  the list kept as it was. A runner that leaves the list (or whose machine key changes under its
  name) is unlinked at once and forgotten by the view; the phone's own entry gone, the view claims
  no role and says it was removed.
- **Connected** (`connect`, the Kotlin `Flow` of batches): the control session (what it is and
  its presence told), a
  link to each runner of the list through the hub, dialled by one function of the core (#1167),
  each request signed with the device key, and each runner's
  `events.watch` resumed after the last event handed over of it, kept in the folder by its machine
  key: a new link, or the app's next start, misses nothing and repeats nothing. An event counts as
  seen once its batch reached the app, so that one on its way when the connection ends, or the app
  is killed, is sent again. `request`, `answer` (Allow, Deny), `stop`
  (the turn) and `follow` (a conversation) go over those links.
- **The links go through the hub** (#1167): the core gives the endpoint of the connection the
  relay of its session (`Session::relay`, `Endpoint::through`), as a daemon does, and dials each
  runner by its machine key when the hub says it online, so that a phone on mobile data reaches
  any runner; a runner that comes online is dialled at once. The key of the runner is the one the
  signed member list holds for its machine: a hub that hands the phone another member's packets as
  the runner's is refused by the link's handshake, and the view says why. The view's machine says
  `through_hub` while it is linked: every link goes through the hub since 2026-11-21, and the
  field stays for the app.
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

**The app, as built (#825).** `apps/android/app`, Kotlin and Jetpack Compose, over the `core`
module's `Phone` (its own types: `PhoneView`, `Machine`, `SessionRow`, `Member`…; `RealPhone` is the
core behind it, the screens' tests play a fake) and never the generated bindings.

- **Joining**: **Connect to your hub**: the camera reads the QR of `flottille-hub pair` (CameraX,
  frames decoded by ZXing, no Google service), or the code is pasted (a field that hides what is
  typed); the hub's **card** is shown before anything is sent (`hub_card` of the core: its address
  and its key's fingerprint, read from the code, no network); the user confirms this phone's name
  (the model's) and its role (**Control** or **View only**); the result says the role the hub gave.
  A refused code or a refused join stays on its screen, said.
- **The screens**: a bottom bar, **Sessions** (the window's sidebar, design B, #1441: each machine
  in the list's order, said once, its header carrying its **mark** (#1249: its icon in its colour
  from the member list, a badge of one shape per state: a filled dot online, a hollow ring offline
  with the icon greyed, an amber dot with an arrow for an update, a dashed ring unknown) and its
  state, folded or not, an offline one folded by default; its projects by name with their count,
  each folded or not, a folded one showing the most urgent mark of what it hides; their sessions in
  compact two-line rows newest first, the state the row's mark (a spinner, a pulsing dot for what
  needs the user, the time of the last exchange at rest), so that a row never moves when its
  session reports; a session led by another under its pilot, indented with the window's connector,
  **folded by default**, the pilot's row saying `leads 3 · 1 waiting` and, folded, carrying the
  most urgent mark of its own and theirs; the ended sessions out of the list, in one **Ended**
  group pinned at the bottom, folded by default, each row saying its project and its machine's
  icon, and the group saying once of a machine that is not online that its ended sessions are not
  listed. The folds are kept on the phone, as the window keeps its sidebar's. A session of a
  machine that is offline is greyed and not opened) and **Machines**
  (the hub, reachable or not, and each machine: `Online`, `Offline since 22:14` with why, `Needs an
  update`); **Settings**, from the top bar: this device's name and role, the
  members, **Remove this device** (it asks once). There is no Add a device. The hub's card says
  that the link is in clear when its URL is `http://`.
- **The core's view state** gives each session its `created` (what orders the list) and its `pilot`
  (#1441: the root of the tree of the agent that leads it, `led_by`, on the same machine; a pilot on
  another machine is not drawn yet: its session is a row of its own, without design B's `led from
  <machine> <title>` line, which the view cannot say), read from `agent.list` and
  `agent.created`, which says `led_by` since; `agent.detached` takes it away.
- **The core's view state** gained what the screens say: a machine's `state` (`online`, `offline`,
  `needs an update`) and `since`, the `members` (this device marked, who added each), `hub_card`
  and `leave` (the phone's own removal, signed with its device key and sent to the hub, which ends
  its session without sending the list: once the hub took it, the core ends its links, the view
  claims no role and says the phone was removed, and the app asks to join again). The core also
  deletes the hub's file and the kept device key: started again, the phone is not joined and never
  dials with a revoked key.
- **Motion**, as the window's: the kit's durations and its ease-out cubic are `ui/Tokens.kt`
  (`Motion`); every press fades and sinks (`FButton`), a choice's fill and ring move over `PANEL`, a
  row inserted, removed or moved animates (`animateItem`), a state that changes fades (machine
  state, hub state, a greyed row, the selected tab), a page and its sections come in staggered
  (`Entrance`), a message unfolds and fades in (`Message`), the confirmation and Settings come and go
  with `MODAL` and `PANEL`. In Sessions (#1441) a fold's rows come and go with `animateItem` and the
  chevron turns over `PANEL`, a row's mark cross-fades over `HOVER` (a folded pilot's or project's
  included), a running mark turns and one that needs the user pulses.
- **Tested** against a fake core by Compose UI tests under Robolectric (`app/src/test`), one per
  scenario of the issue; the core's mapping by JVM tests (`core/src/test`); the app starting on the
  real core by an instrumentation test on the emulator (`app/src/androidTest`); the Rust side by
  `crates/flottille-mobile/tests`. A real phone proves what none of those can: the camera reading a
  QR, and how it feels.
- **Not here**: the foreground service and the notifications (#826's followers); closing a session
  of an offline machine (#1157's kept close). Needs you came with #1443 (below).

**The session page, as built (#826).** Tapping a session opens `SessionPage`; its one entry point is
a `SessionKey` (machine and agent), which an intent can carry too (`MainActivity` reads the extras
`org.flottille.app.MACHINE` and `AGENT`), so that a notification opens a session by those two ids.
**Only the app's own intents do**: the main activity is exported (the launcher, the share sheet),
so any app could name a session in it; the extras are honoured only on an intent that came through
`org.flottille.app.OpenSession`, an alias of the activity that is not exported, which a
notification's PendingIntent targets. What is shared is read once, not again when the activity is
recreated.

- **Reading**: the page follows its session through the core (`follow`: the latest page of the
  conversation, `agent.show` with a `before` past every event, and the tree, `agent.tree`) **only
  while it is on screen and the machine is linked**, and drops it when it is left (`unfollow`). A
  conversation is never loaded whole: **Earlier messages** asks the page before the oldest one held
  (`older`). The thoughts and tool calls between two messages are one line (`3 steps`) until a tap
  unfolds them; a message that arrives unfolds and fades in, what the page held when it opened is
  simply there. A picture sent in a message is read back (`agent.image`) and shown above its words,
  a placeholder fading into it.
  **It follows its end** (#1442): it opens on its last message and keeps it in sight as messages
  arrive and unfold, until the user drags it up; then nothing moves it, and **New messages** says
  what arrived below and brings them down. **A delivery is drawn as the window draws it**: the
  core splits a prompt into its deliveries (`flottille_protocol::prompt`, the daemon's own markers)
  and only the user's words (a root's brief included) are the user's bubble; any other delivery
  is one line, what it is and what it says (`woken by` and the window's own sentence for a
  subscription's news, `report` and `Report from <child>`, `message` and who wrote), its body
  folded under it until a tap. What replays the conversation after a compaction is left out.
- **Writing**: the composer sends text and pictures (`send`: `agent.send` with `images`). Pictures
  come from the gallery (the system's photo picker, no storage permission), the camera (it writes one
  shot into a file of the app's cache that a `FileProvider` gives the camera app alone, then the
  file is deleted) and the share sheet (an intent filter for `image/*` only: the pictures wait in
  the composer of the session the user picks, and are never sent without a tap). The window's limits
  hold in the composer and again in the core, before anything is sent: 20 pictures a message, 5
  MiB each, png, jpeg, gif or webp. A picture is sent as it is, never shrunk: a message that does
  not fit one frame of the link goes in chunks ("Pictures sent to another machine"), and only its
  words must fit a frame (the core refuses words that do not, saying so; the frame is measured on
  the signed request itself, escapes included). A camera shot over 5 MiB alone is scaled down to
  2000 px and re-encoded, the only way it can be sent.
  What another app hands over is never trusted: a uri the app has no grant for, one that fails or
  one whose extra is of another type is left out (said once), and a picture is refused before it
  is held when it is over 5 MiB and is never sent cut short. A
  camera shot is read, then deleted, and what a dead process left is cleared at the start.
  Nothing of a picture is logged. A message to a machine
  that is off is refused, said as a toast with since when (`server is offline since 22:14`) and kept
  in the composer: nothing is queued.
- **Answering**: the ask card docks above the composer, one question at a time (Previous, `2 of 4`,
  a dot per question, Next), its options as rows, **Something else…** with its field, a note once a
  choice is made, one **Answer** once every question has a choice or words (`answer_ask`:
  `agent.answer`). A request is a card with the call **as the daemon gave it** (the same one line
  the window shows: a command's line, else the tool and its arguments, computed in the core, never
  summarised by the app), what the judge said, **every** reason it was held and every agent that
  forwarded it, which agent of the tree asked (`asked by the conversation · #4`), **Allow** and
  **Deny**, and, for a base request, the tag `Always asks` and the window's own warning (the first
  message of a session to another says what it leaves, not the generic one). Those sentences are
  the window's, shared in `flottille-protocol` (`words`), never written twice. **A proposal to
  write a connection** (the tool `connections`, kind `action`) has **Deny** only and says to open
  it on a computer, as the window never offers Allow on it: what it would write is read in its
  dialog. An answer the machine refuses because the request is decided already or gone (or the ask
  is answered) is said on the card, which then folds; any other refusal (a link that ended, a time
  out) is a toast and the card stays, to answer again.
- **Controls** (#1442, design B brought up to date): the top bar holds an icon Back, the title
  and where the session runs, the **mode's chip** (its name, in the warning colour in Full; a tap
  opens a panel with the three modes as radio rows and what each lets the agents do; a choice
  applies and closes it, **Full asks for a confirmation on the phone** first, the one confirmation
  dialog, since one tap in a pocket must not give the agents the run of the machine) and the
  **tree's icon**, with an amber dot while an agent under the session waits for the user: it
  opens the tree as a panel from the bottom, read only, the pilot first, then its children and
  grandchildren with what each does, the ended ones folded under one line. A **request is in
  the conversation**, where it was asked, with Allow and Deny; answered from the phone, it folds
  into one line (`Request #4 · <call>`, `Allowed` or `Denied`). The questions stay docked above
  the composer. The composer is one row of icons, Gallery, Camera, the field, Send, each with its
  label for a screen reader; Stop takes Send's place while a turn runs and there is nothing to
  send (`agent.cancel`: the agent stays); a line above says the session works and what waits for
  the user. **A child's page has no composer**, no mode and no
  Stop; an ended session says it has ended in place of the composer.
- **A `viewer` reads only**: the page draws no composer, no pictures buttons, no answer (the
  ask card is read, its options are text), no Allow or Deny, no mode, no Stop, and says `View only`.
  That is the app's half: **the machine refuses a viewer's write too** (`Roles`, `read` and
  `drive`), which `remote_requests` and the catalogue's tests pin for each write this page makes.
- **The core gained**, for the page: `follow` reads a page and the tree, `unfollow`, `older`, `send`
  (checks the pictures' limits), `answer_ask`, `set_mode`, `image`; the conversation of the view
  state carries the title, the mode, `working`, `child`, `ended`, `more`, each entry's number and
  picture names, and the tree; a request carries the call line, its kind and class (`base`), what it
  says, the judge's sentence, every reason and forward, who asked and the warning; an ask carries each question with its options.
- **Motion**, each with the kit's durations: a message or a tree row unfolds and fades in
  (`ENTER`); the list follows the growing end, **New messages** rises and fades in and out, a
  delivery's body unfolds with its chevron turning (`PANEL`), the panels of the tree and of the
  mode rise over their scrim and go down (`PANEL`), their rows come in staggered, the chosen mode's
  radio fills, the mode's chip moves to the alarm colour and cross-fades its name, an answered
  request folds away and its line unfolds, the tree's dot and its ended agents fade and unfold, the
  composer's status line unfolds and its spinner turns, every
  icon button fades and sinks when pressed (`HOVER`, `PRESS`); the steps fold and unfold, with their chevron turning (`PANEL`); the ask card and the
  request cards dock, slide into place and fade away (`animateItem`); a question slides in from
  the side of the move, the dots and the option rows move their fill and ring (`PANEL`), the field
  of "Something else…" and the note unfold and fold; a refusal comes in on its card; the mode's
  fill and ink move (`PANEL`) and Full's turns to the alarm colour; Send and Stop cross-fade; the
  attached pictures unfold in the composer and fold away; a picture fades in once read; the page
  and the banner for a shared picture come in and out as Settings does.

**Needs you, as built (#1443).** The bottom bar has its three destinations, each an icon and a
label: **Needs you**, **Sessions**, **Machines**; the app opens on Needs you (Settings stays in the
top bar).

- **What it lists**: the view's `requests` and `asks`, which are every linked machine's, each on the
  session page's own card (`RequestCard`, `AskCard`: one card pattern), under the session's title
  and the machine's tag, newest first; answered in place, a refusal said on its card as on the page.
  A card leaves when the machine says it is decided or answered, from the phone or elsewhere; while
  it folds its buttons are off. A tap on it outside its buttons opens its session. Empty, one line
  says `Nothing waits for you.`
- **A child's request** (one a sub-agent asked, forwarded up its tree) is under its session, as on
  the session page: the card is headed by the root session's title and the child's
  (`Fix the build · Check the tests`), and a tap opens the root's page. The core names the root
  (`RequestView.session`, from the parents of `agent.list` and `agent.created`) and the child
  (`RequestView.child`, its title).
- **Newest first** reads `at` on a request and an ask: the time the mobile core saw it come, in the
  phone's clock. One that already waited when the runner linked takes the daemon's time of it (a
  request's `created_at`, a question's `at` in `agent.tree`), else the link's; one seen again keeps
  its first time. Request numbers and ask numbers are two counters, and each machine has its own:
  they only break a tie. A question asked before the phone linked shows once the core reads it (see
  "Defaults taken, to confirm").
- **A load is the truth of its machine**: at every load of a runner, the requests of that runner the
  fresh `approval.list` no longer holds leave (decided while the phone was off, no event ever came),
  and the questions the fresh `agent.tree` no longer holds leave with it; another runner's stay.
- **Motion**: a card that comes unfolds and fades in, one that leaves fades and folds away (kept on
  screen until its fold ends), the others slide (`animateItem`); a card sinks a little while
  pressed (`PRESS`); the line of an empty screen fades in once the last card has folded and out as
  one comes (`ENTER`, `HOVER`); the tab change cross-fades (`ENTER`, `HOVER`), and the selected
  tab's fill and ink, the icon's with them, move over `PANEL`.

## Notifications

- **Through the hub, no push service** (decided by the maintainer on 2026-11-20): neither a
  distributor app (UnifiedPush, ntfy) nor Google's FCM. The phone's foreground service keeps its
  session with the hub; when something needs the user, the runner tells the hub, and the hub tells
  the phone over that session. What travels holds only a kind (`request`, `question`, `report`,
  `failure`, `dismissed`) and ids, as every hub message. The app links, reads what it needs
  (`inbox.list`, `approval.list`), and draws its own notification: a request with **Allow** and
  **Deny** (not on a `viewer`), a question that opens its card, a report.
- **Never while the user is at another client.** Each client tells the hub its presence (the
  window: focused, last input; the app: in the foreground), and the hub tells the runners. A thing
  to notify waits 30 seconds; it is sent only if no client of the user had an input in the last 2
  minutes, and dropped if it was seen or answered meanwhile (the request answered, the session
  opened at a client).
- **Dismissal follows**: what is answered or seen elsewhere sends `dismissed` with its ids, and the
  phone removes its notification. A request answered at the desk never stays on the lock screen.
- One setting, Settings, Machines: **Notify the phone**: `when I am away` (the default) or `always`.
- When the hub is unreachable, nothing reaches the phone: it says so in its permanent notification
  ("Not connected to your hub") and reconnects with a backoff.

**Built (#827), the runner's and the hub's side.** The runner reads its notification centre
(`docs/protocol.md`, `notifications.list`): a row of kind `request`, `question`, `report` or
`failure` (the centre makes them for a request held for the user, a question to the user, a root's
report and a root's failure) is decided once, 30 seconds after it was made, on the daemon's clock
(`crates/flottille/src/phone.rs`). Unless it was read or answered by then (`read_at`), it is told
when **Notify the phone** says `always` or when no client of the user had an input in the last 2
minutes: an input at a client of this machine, as the daemon stamped it, or any member's that is no
phone as the hub last told it (the app says an input each time it opens its session, on another
network say: it would silence the next notices). An input said to come more than a minute after
the runner's clock is not counted: a clock ahead would silence the phone until then. Otherwise it
is dropped, and does not ring later. A row told that is then read, answered or gone tells
`dismissed`; a row never sent (no session then) is never dismissed, and what is told is queued in
order, so that a `dismissed` never overtakes its notice. What came before the daemon started is
never told, and with no session to the hub nothing is told and nothing is kept for later.

- **Presence**: the window says `client.presence {focused, input}` to its daemon when its focus
  changes and at an input (a key, a click, the wheel, every 20 seconds at most); the daemon stamps the input
  and tells the hub `{"ask": {"kind": "presence", "active", "last_input"}}` (seconds since 1970), at
  once and again at each new session; the hub tells the runners every member's (`{"presence":
  […]}`), as before.
- **What the runner asks**: `{"ask": {"id": n, "kind": "notify", "notice": N}}`, with
  `N = {"kind": "request" | "question" | "report" | "failure" | "dismissed", "notification": <the
  row's number on the runner>, "session": <the root agent's id>}`; `session` is left out when the
  row has none. An id is 1 to 64 letters, digits, `-` or `_`, and a notice with any other field is
  refused, on both sides: no title, excerpt, path or command can ride in it. The hub takes it from a
  runner's machine only (`only a runner's machine tells the phones`, otherwise) and keeps nothing of
  it.
- **What the phone listens to** (#828): on its session to the hub, `{"notice": {"from": <the
  runner's machine key>, "notice": N}}`, `Pushed::Notice { from, notice }` in the core. The hub
  sends it to every open session of a member that asked for the notices (`{"kind": "about",
  "device": "phone", "system", "notices": true}`), and to no other. The phone finds the runner among the members by its machine key, links to it, reads
  the row (`notifications.list`) or the session's request (`approval.list`) and draws its
  notification; on `dismissed` for the same `from` and `notification` it removes it.
- **The setting**: `[remote] notify = "away" | "always"` in `daemon.toml`, `remote.settings` and
  `remote.configure {notify}` (`docs/protocol.md`), and Settings, Machines, **Phone**: **Notify the
  phone**, **When I am away** or **Always**.

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
  Built (#831): the session page lists the commands of the session that run and report an address
  (`run.started`, `run.ready`, `run.ended` of the page's events), and a link an agent showed
  (`agent.link_shown`, a `url`) is a card of the conversation, each with **Open** and **Open in
  browser**. The phone's core does what a client's daemon does: it signs `tunnel.open {run, port?}`
  for the machine, listens on `127.0.0.1` of the phone (a port the system picks at random, never
  the run's own), opens one stream of the link per connection (64 at once, one past it is closed),
  and holds the tunnel with the request's
  stream, which the machine ends with its run and the phone ends by dropping it (`Core.open_address`,
  `Core.close_tunnel`; the view says the tunnels held and, for each that ended, why). A link names
  no run: the core asks `run.list`, takes the run that reports its port, else the only one that
  runs, and the machine checks the port again. A page of the web opens as it is, in the user's
  browser. The in-app browser is a web view that loads that tunnel's origin only (any other link
  is dropped, unless it is an `http(s)` address its main frame goes to on a tap, which goes to the
  user's browser: a page starts no other app), enters and leaves over the panel's duration, says
  when its tunnel ended, **closes its tunnel with its panel** and forgets the page's cookies and
  storage then. **Open in browser** starts a foreground service (type `specialUse`) that holds the
  core's connection, so that the links stay up with the app in the background, and shows one
  notification, "Tunnel kept open", with **Close**; the service ends when its tunnels have, and an
  end that is not the user's own (the run ended, the link dropped) is a notice of its own that says
  why. A tunnel of **Open in browser** has an idle end: with no connection open for 10 minutes it
  closes, and the notice says so (`no connection for 10 minutes`); the in-app browser's tunnels
  and those of the two uses are never shared (`open_address(…, browser)`). A `viewer`'s page draws
  no tunnel control (the machine would refuse it).
  Built (#1230), what the second review of that left: an **Open** whose panel is left before the
  machine has answered (Back, say) opens no tunnel, and closes the one it had opened if the answer
  came first (`Core.open_address` is undone with its caller; a tunnel held before, which that call
  only found, stays); a form that posts to another site, which the web view does not report to
  `shouldOverrideUrlLoading`, is stopped when its load starts (`onPageStarted`) and the page goes
  back; the web view's HTTP cache goes with the cookies and the storage.
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

## Settings for all machines

A user with four machines fills the same settings four times. The hub keeps **settings for all
machines**: every machine uses them, each machine can **override any one** of them, and go back to
the value of all machines (decided 2026-11-25, #1330). Built for About you first (#1330, part 1);
the guidance files and the rest of "Shared, personal" (#1330's inventory), then the settings a
machine often overrides, follow in their own issues.

- **Three levels**, read in this order: the project's value (where one exists) → **the machine's
  own** → **all machines'** → the default, as `extensions::value()` reads project → global →
  default. About you has no project level: a field written on a machine (`about.toml`) is that
  machine's own; one not written there is all machines' value; neither is nothing.
- **What is shared is the user's change, signed.** The user's device signs it, as any request it
  makes (`about.share`, "Requests from another machine's clients"), for its own machine, whose
  daemon checks it as it checks a request (member, signature, time, nonce, role). That machine then
  **seals** the signed request for every machine of the member list (`crates/flottille-link/src/shared.rs`):
  a random key encrypts it (ChaCha20-Poly1305), and that key is encrypted for each machine's key,
  by X25519 between a key made for this seal alone and the machine's Ed25519 key turned into its
  X25519 form, through HKDF-SHA256; the value's name and time are outside, bound to what is
  encrypted, and the machine signs the whole with its machine key. One sealed copy a value
  (`about.name`, `about.pronouns`, `about.notes`), so that two changes of different values do not
  undo each other.
- **The hub keeps them without reading them**: the latest copy of each name (`hub.json`, `shared`),
  from a member's machine whose role is a runner's or a controller's, its signature checked, 40 KiB
  at most a copy, of a name it knows (`SHARED_NAMES` in `flottille-link`: a setting shared later
  adds its own there), dated no later than its clock plus a request's five minutes (a time far
  ahead would hold the name for ever). A copy of the same time replaces the one kept only when it
  is sealed for a machine more: the same value sealed again for the same machines is not written. It sends them whole to each session that asks for them
  (`{"ask": {"kind": "shared"}}`), then again at each change (`{"shared": [...]}`); a hub of before
  says nothing of them in its welcome, and a machine asks it nothing. What the hub sees of them:
  their names, their times, which machines they are sealed for, and their size.
- **Each machine checks each copy itself**: it opens it with its machine key, then checks the
  user's request inside as its own (a member's device, not a viewer's, the method `about.share`,
  the value and the time the copy says, the limits and the one line `about.set` asks, a name
  trimmed), whichever machine sealed it and whatever the hub says: what authenticates a value is
  the device's signature inside, not the machine's that sealed it, which only lets the hub keep it.
- **A device removed, or made a viewer**: each machine drops the values its device signed at the
  next list, and gives them to the hub no more; About you is then as if none were shared, until the
  user changes it again. The hub's copy stays until a newer one replaces it, refused by every
  machine that opens it.
  Neither the request's five minutes nor its nonce is checked again: a machine off for a week takes
  the latest copy when it is back, and an older copy never replaces a newer one. **The latest of
  two changes wins**, by the time the device signed it (then by its signature, so that every
  machine keeps the same one).
- **Each machine keeps the last copy it checked**, `remote/shared.json` (600), the user's signed
  requests themselves: with the hub out of reach its agents read what it last checked, and a change
  made meanwhile waits on the machine where it was made, given to the hub at the next session.
  `about.share` says where it went (`hub`: `given`, `later` with the hub out of reach, `old-hub`
  with a hub of before them, which keeps none), and the window's toast says it; a value that,
  sealed for every machine, would be larger than the hub keeps (notes of characters escaped twice
  over) is refused on the machine, before anything is kept. A
  change reaches every machine at once, a machine that was off at its connect. A machine paired
  since a value was sealed is given it: a machine that finds the hub's copy sealed for fewer
  machines than the list has seals it again for all of them, the user's request unchanged.
- **Who may change them**: a device allowed to change another machine's settings today, a
  `controller` or a `runner` ("Roles"); a viewer's change is refused by the machine it is asked of,
  by the hub when its machine seals it, and by every machine that opens it.
- **Secrets never go there**: no vault value, no OAuth token, no key, no project `[variables]`.
  `about.share` takes the three fields of About you and nothing else; a setting that names a vault
  entry will name it, the entry staying the machine's.
- **Without a hub** (remote off, or this machine paired with no other runner), Settings is as it
  was: About you is the machine's own, and `about.show` says `all: null`.
- **What a hub that misbehaves can do**: read nothing and change nothing (each copy is sealed and
  signed inside), but withhold a value, or give an older copy than the latest to a machine paired
  since, which has no copy of its own to prefer; the machines that have the latest keep it and give
  it again. A hub downgraded to a version of before these values refuses its own `hub.json` (its
  `shared` is a field it does not know): moving back to an older hub means dropping `shared` from
  its `hub.json` first.

**In the window** (decided 2026-11-25 on the prototype): Settings' picker "Settings of", on the
title's line of the page's header, which stays in place while the sections scroll, lists **All
machines** first, then each machine. All machines shows the sections every machine may share (About
you, for now) on General; a change there is signed with the device key and saved for all machines.
On a machine, About you says once `The values of all machines, unless changed here.`; a field
changed there is that machine's own: its edge takes the colour of what waits (`wait`), and one line
under it says `Changed on server` with **Use all machines' value**, which drops it. A section that
belongs to the machine alone (Providers, Forges, Vault, the folder of travelled projects, Developer
mode, Check) says `This machine only.` above it. Each of these moves: the line and the mark unfold
and fold with their gaps, the edge fades over `PANEL`, the way back fades on hover, and All
machines' page enters staggered as another machine's does.

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
| `site/` | The showcase site, coming, and what is published on the repository's GitHub Pages: Flottille's OAuth client metadata document (`site/oauth/client.json`, `reference/mcp-authorization.md`, §2). |

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
`HOME` and XDG folders and its own in-memory keyring, on `127.0.0.1`, with no discovery; their links
go through the hub, and the link crate's own tests join their endpoints' relay ports in memory as a
hub joins them (`relays_in_memory`). Fake agents, throwaway repositories. A machine
"off" is a daemon stopped and started again on the same folders; a hub "down" the same. The Android
app is tested with its unit and Compose tests, and on an emulator in CI; the Doze proof is done once
on a real phone, by hand, and reported.

## What this does not cover

- **Full mode's limit stays**: code an agent runs on a machine acts with the user's rights there, and
  could read a key a client keeps in an unlocked keyring. A device key read that way can sign what
  its role allows. Roles bound what a device may do; a phone that asks for its unlock before a base
  answer comes later.
- **A tree in Full can sign as this device.** A command an agent runs in Full is held back from
  nothing (`docs/tools.md`, "In Full, a run is the user's own"): it reaches the keyring, and in it
  the `Flottille: device` key that signs the user's requests to every machine, and Flottille's
  state folder. Code it runs can sign what this device's role allows, a switch to Full among
  them: the twelve hours a signed switch to Full counts for, and the check that the user made it,
  do not hold against code run in Full, which can sign a new one at any time.
- **Configuration is synced for About you only** ("Settings for all machines"): the rest of the
  settings follow in their own issues, and a project is registered by hand on each machine.
- **No session moves** between machines; no conversation travels.
- **Files other than pictures** do not travel yet; submodules and Git LFS objects are not sent by a
  launch.
- **One hub per user**; several users, or a hub shared between people, are not planned.
- **iOS** and a desktop build of the app are not planned.

## Defaults taken, to confirm

- **Settings for all machines are sealed to the machines' own keys** (#1330): each machine's
  Ed25519 key is turned into its X25519 form to open what is sealed for it (as libsodium and age
  do), rather than a key of its own for it, which the member list would have to sign and send. The
  use of one key for both is studied (eprint 2021/509) and found sound for this.
- **About you's field written empty on a machine follows all machines again** (#1330): there is no
  way to say "nothing on this machine" while all machines say something; Use all machines' value
  writes the field empty.
- **All machines is offered while the picker is** (#1330): with another runner in the list; with
  this machine alone, Settings is as it was.
- **A section is said "This machine only." when nothing of it will be shared** (#1330's "Never"):
  Providers, Forges, Vault, the folder of travelled projects, Developer mode and Check. Appearance,
  the workspaces folder and Version hold settings that will be shared (the shade, a `~/…` folder,
  the check for updates) beside the machine's own, and say nothing yet.
- **A machine's state this machine cannot vouch for is the dashed ring** (#1249): the badge of a
  machine's mark follows the state the hub's list gives it (`online`, `offline`, `needs an
  update`); any other one is drawn as not known, the dashed ring. A hub that cannot be reached
  does not turn the last states read into unknown ones: they stay as last read, as the words did.
- **The badge's cut-out takes the sidebar's colour** (#1249): a hovered or chosen row keeps it,
  the mark not following the row's fill.

- **The window reads the device key at its start once the user has another runner** (#1280): the
  bell follows every machine, so its links need the key before any page of another machine is
  shown, where it was read only then. A locked keyring holds those links until it is unlocked; the
  rest of the window does not wait. A key that cannot be read is said once (a toast) and read again
  at the next news of the machines.
- **"Ended" lists another machine's latest 50 ended sessions** (#1280): each title is a round trip
  over the link (the session's brief), and the group is asked again each time it opens. A machine
  that is off has its rows taken out rather than shown as last read, so the group never lists what
  it cannot open.
- **A machine whose role is not known yet has no bell link** (#1280): the bell follows runners only,
  and one whose role the list has not said yet joins once it says runner.
- **Another machine can run a shared project's session when it is an online runner that offers the
  project** (#1266): what agents it has signed in is not looked at, so that a machine is not left
  out while its providers are not read; a start it cannot run is refused there, and said. This
  machine "can run" when its role is a runner: its providers are not looked at either, so that the
  page that opens does not change as they are read (its composer says when none is ready).
- **A project with no remote is matched by no other, in the switches as in the sidebar** (#1410):
  the start choice and the settings switch offer the machines of the project's row, so the name is
  the fallback only where the sidebar groups by it (a machine whose list says no remotes). Two
  projects of the same name with no remote are two rows (`notes`, `notes@server`), and each one's
  switches offer its own machine only. Matching them by the name would change the sidebar's rows
  too, in one rule for both.
- **The machine chosen for a shared project is kept with the sidebar's folds** (#1266, `starts`
  under `sidebar`), not under a key of its own as the arrangement is: two windows choosing at the
  same moment may keep one choice, which is harmless (the next choice writes it again).
- **A machine with no session of a shared project has no block on its page** (#1266): the row's
  marks already say which machines have it.

- **An allowed launch whose setup a restart cut is refused, not resumed** (#1317): the daemon that
  starts again finds the request allowed and its answer never stored; it refuses the launch, removes
  its workspace and tells the caller, which launches it again. Resuming it (laying its files again,
  then its settings) could come later; refusing is the simpler and leaves nothing half made.
- **An answer that cannot be kept tells the caller refused** (#1317): when the store refuses the
  user's answer to a held launch, the caller is told nothing was done, while the launch there stays
  held until its session ends; the refusal that end sends is a second answer, which the caller's
  machine refuses.
- **A launch a restart cut after it was told approved is stopped, its caller told it ended** (#1374):
  when the store could not keep the user's answer, the caller may have heard "approved" while the
  daemon there, started again, finds the launch's setup cut and stops it. Its stop goes as the end of
  the session (`child-failed`, `session-turn-ended`), which the caller's machine takes, rather than
  as a second answer, which it refuses. Keeping the launch and resuming its setup instead could come
  later; stopping it is the simpler, and the rule that a launch is answered once stays whole. A
  handed launch's caller is told its end as a led one's (`session-turn-ended`, #1385): it was told
  the session started, and hears nothing else of it. A caller's machine of the release before
  #1374 takes that end too and records the launch `ended`; for one whose approval never reached it
  before the restart (its answer not told), it shows `ended` where this release shows `refused`:
  either way it no longer runs, and nothing is left waiting.
- **The user's answer to a launch is kept beyond the cap, up to a ceiling** (#1385): it is kept
  for the caller's machine even when 100 messages wait for it already, and cut to 64,000
  characters rather than refused, so that the end of a session that ran does not come to a launch
  still waiting for its answer, which the caller's machine would record `refused`. What waits for
  one machine is bounded all the same: the answers go on to 104 (the 100, and the 4 launches the
  user may hold at once for a machine), past which an answer is dropped with its inbox entry like
  any delivery: a machine that stays linked and takes nothing for that long fills no disk.
  Telling the caller's machine apart an end of a session that ran from one that never did would
  need a new field of the link; keeping the answer needs none, and a caller of the release before
  gains from it too. A store that cannot keep the answer at all still
  drops it, said in the inbox.
- **The user's answer to a held call of another machine's agent is kept the same way** (#1405):
  the answer to a request of a caller of another machine (`request-<n>`) is kept beyond the 100
  as a launch's is, to the same 104, and past it, or when the store cannot keep it, is dropped
  with an inbox entry that says which request's answer it was and for which machine (`The answer
  to request #12 for pc was not sent: …`), never its text. It used to be dropped with a line on
  the daemon's stderr only, and the agent that called never heard what the user decided. The
  answer is still cut to 64,000 characters, without the marker a launch's answer has: the simpler
  of the two, which a call's answer never needed.
- **A restart leaves a launch that ran alone** (#1317): of an allowed launch whose answer was never
  stored, only one whose workspace is still preparing or whose session was never briefed is refused;
  one that ran is left running and its answer is stored again, since its caller was told approved.
- **A method the user's door refuses for a child here is refused for a child of another machine
  too, whole** (#1304): the user's two ways out stay (`agent.answer` and `agent.stop`, while its
  parent's machine is off), nothing else of the user reaches it. `agent.mode` is refused rather
  than set on the child alone or forwarded to the parent's machine: forwarding would be a new
  message of the link, and the mode of a session is set where it runs; the simpler refusal can be
  replaced by a forward later. Its report and failure are left out of the user's notifications and
  inbox, its question to the user (while its parent's machine is off) is kept in them.
- **An old remote child's depth is taken from its launch** (#1304): the store's repair puts a
  remote child stored by a release of before at its caller's depth plus one, as the launch it came
  with says (1 when it says none), and every agent under it one below its parent, raising a depth
  only. Its caller's depth read from the launch is the one the child would have been given, rather
  than 1 for every such child.
- **The sentence of another machine's Settings scrolls with the sections** (#1293): `This window
  (appearance, version) stays this machine's…` was a fixed line under the tabs, and what scrolled
  could be drawn over it. It is now the first line of what scrolls, so no text is ever drawn over
  another; the kit has no opaque, fixed header for what scrolls to pass under, and none is drawn
  here. A fixed header can come later, in the kit first.

- **A machine of before refuses a launch whose caller an agent started** (#1275): `launched` is a
  field it does not know, and a launch is read with no unknown field, so the launch is refused
  with the link's own refusal rather than run as any root's: the agent is answered `refused by
  server: server refused: not a message a machine may send`, and nothing starts there until that
  machine is updated. A launch of any other caller leaves the field out and reaches it as before.
  No protocol bump for it.
- **The caller's word on `launched` is taken** (#1275), but for a session the receiving machine
  itself launched on the sender: that one is a session an agent started whatever the launch says.
  Of any other caller, the receiving machine cannot know better, and the machines are the user's
  own.
- **A child on another machine counts as a session an agent started** (#1275): it has no parent
  there yet, and the launch that started it says it was launched, so its `led` and `handed`
  launches would be held for its parent, then the user, never the judge's. Since #1274 it is also
  a real child, whose such launches are refused outright, so the rule is a second line there.
- **A child on another machine is not held for the chain** (#1275): a session an agent started
  starts children here as its mode allows, and on another machine likewise; only `led` and
  `handed` launches are base.
- **A session started here for another machine's agent cannot be detached yet** (#1275): it has no
  `launched_by` here (#1272 records its pilot), so its launches stay the user's until then.
- **A child of another machine's agent stored at depth 0 is put at depth 1 at every open of the
  store** (#1274), not by a numbered migration: the change is done once and does nothing after,
  and every check of a root then reads its depth alone. `agent.trees` still lists it among the
  roots: the window reads there the questions it asks the user while its parent's machine is off.
- **A launch that says no depth is a root's** (#1274): only a root launches on a machine of before
  (its depth limit is 1), so a missing `depth` is read as 0, never refused; a child asked for by a
  caller at the limit is refused by the machine it would run on.
- **What a travelling setup needs is its copied files** (#1214): today a project's settings name no
  vault entry (its services and setup steps hold none), so the entries a launch asks for are those
  made from the files its `copy` steps match on the caller's machine, one entry a file, named
  `<project>-<path>` by the caller's name of the project, every project's (a project's name may
  differ on the other machine). A file is made from its entry only when the project there lacks it,
  in the project's folder, mode `0600`.
- **The ask is the held launch's request** (#1214): `approval.approve` shares nothing by itself;
  the window's Allow makes and shares the entries first (#1262), and an approval while one is
  still missing (the command line's, the phone's until it does the same) refuses the launch,
  saying so.
- **The window orchestrates an Allow that shares files** (#1262): the receiving machine cannot
  read the caller's files, and a share is signed by a device of the user's, so no one method does
  it all. It reads `vault.list` on `from` first and skips the making of the file's own entry
  there (of its name and bound to it; no `replace`): a Try again keeps what an Allow that failed
  further on made, and an entry the user corrected by hand is never overwritten; an entry made a
  minute before keeps the file as it was then. A `vault.list` that fails blames no file: the card
  says it, its rows as they were.
- **Allowed with its files, the card is the result** (#1262): no toast; its marks show a moment
  and it folds away, the request's line recording `Allowed · 2 files shared` where the page still
  shows it (the page of the launched session reads its conversation again once it starts, and that
  line is not in it). A failure stays on the card, never as a toast.
- **After a failure the primary button says Try again** (#1262): the click resumes at the failed
  step instead of doing it all again, and its word says so.
- **A failure brings the card's error and buttons into view** (#1419): the section of "Waiting for
  you" is limited in height, so a card grown by its error has its Try again and Deny below the
  fold. The section scrolls to them once, over `PANEL`, the way a "leads" link moves a page; the
  steps that succeed scroll nothing, and the user's own scroll afterwards is left alone. The error
  and a refused Allow or Deny on any card do the same. The error line unfolds and fades in with
  its gap, and the section scrolls once it has unfolded, so the card is measured at its height;
  the line folds and fades out when it goes (#1430).
- **A card pointed at from the conversation slides into view** (#1430): the section moves to it
  over `PANEL`, as it does to a failed card's buttons, and leaves a card already in sight where it
  is.
- **Deny after a partial share leaves the entries shared** (#1262): each is bound to its one file
  (`file:<project>/<path>`) and laid only by a launch the user allows; the card's Details says so
  once a file is shared, and only then.
  No Remove from the other machine is offered.
- **Allow and Deny are off while it runs** (#1262): stopping in the middle would need a share
  undone; the run takes seconds.
- **A device that may only view signs no share** (#1262): the shares are signed for `from`, so
  its rights on `from` are what count, whichever machine is in front: Allow on such a card is off,
  its reason in its tooltip (`This device may only view pc`); the daemon's refusal stays the
  guard.
- **The steps are not capped on the card** (#1262): they are those of `copied`, which #1214 caps
  already; the section scrolls past its height as for any card.
- **The class of the card stays the daemon's** (#1262): a launch that lays a file is held whatever
  the mode, its class as the gate gives it; the card draws it as any other.
- **The row says `preparing <project>` until the first delivery** (#1214): the receiving machine
  says nothing while the setup runs, so the row cannot tell the setup's end from the first turn's;
  its next delivery says what it does.
- **A secret of several lines is masked line by line too** (#1214), each line and each value after
  its first `=` or `:` (quotes and a final comma left out) and an address's password, of 6
  characters or more, comments left: a file's entry would otherwise be masked only whole, and an
  agent reads one line of a `.env`. A secret transformed is not recognised (`docs/vault.md`).
- **Two files whose entry would have one name are told apart by the entry's binding** (#1214):
  `a/b` and `a-b` both name `app-a-b`; the entry is bound to the file it was made of, so the other
  is refused its entry, naming the first, rather than the name carrying a hash of the path (names
  stay readable on the card). A launch that copies both lays the first and lacks the second,
  saying so; a hash in the name is the change if that case is met.
- **A file is laid only after the user allows it, in every mode** (#1214): even in Full and with
  the entry shared already, a secret written into a project's folder is the user's to allow.
- **The folder for projects from other machines is not moved** (#1214): changing it applies to the
  projects made afterwards; those made before stay, and a launch that did not start removes its
  project from where it was made.
- **A call to a session of a machine that is off is refused at once** (#1250), every tool alike,
  never kept: what the session's machine would answer (held, refused, the leave) cannot be known
  before it is back. Messages to a session the caller started stay kept, as before.
- **The leave given to a caller of another machine lasts until the session written to ends** or
  the user withdraws it (#1250): its machine does not tell when the caller ends, so the end of the
  writer does not take it, as it does on one machine.
- **A session of another machine is in the caller's project when its project has the same name**
  (#1250), as the sidebar groups them ("One list of every machine"); a title is matched among the
  live sessions that machine listed, an untitled one by the first line its list gives it. The
  name only: the identity of a project by its repositories' remotes (#1228's `projects` list) is
  a follow-up.
- **A led session writing to its pilot on another machine** is asked the leave as any other pair
  (#1250): the receiving machine does not stand its caller in as started by that pilot.
- **The user's signed switch to Full counts for a call as for a launch** (#1250): checked against
  the machine that sends it and the tree that machine names. The receiving machine cannot tell
  which tree the caller belongs to; a machine that holds a switch could name its tree for another
  of its agents, for 12 hours at most, as it already could for a launch.
- **A machine of before refuses a call of another session** with the link's own refusal of a
  message it does not read (#1250), not with a sentence of its own: no protocol bump for it.
- **A held call of a caller of another machine is that machine's user's request** (#1250), as a
  request of a child there goes to that machine's user. The stand-in's own id (`<caller>@<key>`),
  seen on a request card, is refused as a target by an agent of that machine (#1263), as a session
  that does not exist (`session_read`, `session_send`, `agent_stop`): it holds no conversation and
  runs nothing, and nothing is ever started for it.
- **A caller of another machine reaches the sessions of the machine that stands it in only**
  (#1263): a session that resolves to a third machine (or to its own) is refused with a sentence
  of its own, and no call goes out on its behalf.
- **A member list that cannot be read keeps the stand-ins** (#1263): while the peers' file was never
  read whole (a broken file at the start), no machine left the members by it: the sweep forgets
  none of them for that, and still forgets those idle for 30 days, with their leaves and the
  answers they await; a list read whole that has nobody in it forgets them all.
- **A stand-in whose project is not on the machine is in a folder that does not exist** (#1250),
  under the user's runtime folder, never made, rather than refused: a read of a session of another
  project is served as on one machine. The callers of one machine are capped at 64, forgotten after
  30 days idle (as long as a message is kept) and when their machine leaves the members.
- **A session resumed by the user keeps its pilot's cap** (#1361): `agent.resume` of an ended root
  session led by a stand-in checks, in the very write that revives it, that its pilot still has room
  in its machine's 64 (as a new session does), and is refused with the cap's own sentence when it
  has none. The user resumes it, so the refusal is theirs to read; the pilot counts nothing while
  all its sessions are ended, and the choice is to refuse rather than to forget another pilot.
- **The answer of a held call is awaited from the moment the call's answer comes back** (#1250):
  an answer the user gave before that (within the same second) is answered "send again later"
  (#1263) while a call of that caller to that machine is still unanswered, and kept by its
  sender. An answer that comes when no such call is out (the call was given up, or its answer
  was lost with the link) is refused for good, as nothing will ever await it, so that it holds
  back nothing its sender keeps for that machine behind it. Answers never given are forgotten
  after 30 days.
- **"No screen" is the icon the machine proposes for itself** (#1247): a server (no battery and
  no connected screen under `/sys/class/drm`) is proposed as a `runner` whether an agent CLI is
  signed in or not. The machine has no other reliable signal of having no screen, so the icon is
  the one already proposed and shown, and a headless machine that has a screen connected is a
  desktop and stays a `controller` until its role is changed on the hub's host.
- **A controller or a viewer is explained on its own row, not once for the page** (#1247): each
  member that runs nothing says, under its row, its role, that it runs nothing and so is not offered
  in Settings or the composer, and the command of the hub's host that changes it, copyable
  (`flottille-hub role <name> runner`). One block per member, because the command holds the name and
  each folds away by itself when the role changes; the window never offers a button that changes a
  role.
- **A page of another machine is that machine's, and nothing else switches** (#1271, replacing
  #1182's "the whole window goes to that machine"): Settings' and Add project's choosers, and
  opening a session or a project there, show that machine's page on its own connections, kept
  open; the sidebar and its folds are this machine's one list, whatever page is shown, and the
  bell and "Ended" hold every machine's (#1280).
  Going to that machine's whole read for one page was slow (seconds behind a link of 150 ms a
  round trip) and showed the previous machine's values meanwhile.
- **`project.folders` lists one folder's folders at a time** (#1182), from the machine's home: the
  dialog opens them one after the other, and a folder that is a repository itself is picked; a
  folder that only holds repositories (not one itself) is opened, not picked. It is `drive`, as
  adding is.
- **The choosers name each machine with its icon, its role and its state** (#1182): the kit draws
  the icon of the machine chosen in its colour before its name on the chooser's button (`server in
  teal` is the colour's word, for the ones who do not see colours), and each row of the list has the
  machine's icon in its colour, its role and state, with a check on the chosen one that moves over
  `PANEL` (the kit's tinted option, `MenuItem::tint` and `checked`; `Select` and `RadioChoices` take
  the same tint). Nothing is drawn beside the kit's chooser. A chooser that stops having a choice
  (the project is on one machine only, the last runner is gone) folds away with its words. The
  choosers offer the runners only: a controller holds no project.
- **The composer's machine picker lists the machines that have the project** (#1182): the runners
  that offered it over their link, or whose light list of sessions names it (kept while a machine is
  off, which offers nothing). One that is off is greyed and says `offline since 22:14`; it is not
  picked. A project on one machine has no picker. Choosing a machine shows that machine's page of
  the project, like every page of another machine, and the place is `main` again. The machine of the
  page that goes off says under the composer to pick another or wait; the composer stays free for
  the picker, and nothing is started.
- **A session started on another machine is a row of its pilot's rail** (#1182): its machine's icon
  and its title in the rail's Agents, and, for a child, a line of the sub-agents list; both open
  its page, that machine's. The header of a session names the machine it runs on when the user
  has several (`[icon] server / main`).

- One list of every machine (#1150): the window follows the other machines through its own
  daemon, which keeps each machine's list as that machine tells it over the link (the `sessions`
  message), not through a watch carried to each machine: the list is the same whether a client is
  open or not, and its light copy is what is shown while the machine is off. The session open is
  watched on its machine, through a watch carried there.
- A machine's list holds 500 sessions at most (those of the latest exchange), a title 200
  characters at most; it is told again on any event of a session but what an agent streams during
  a turn, and every 30 seconds while a link is up when it changed.
- A session's state in a list is `needs you` on the rule its own window follows (`agent.trees`'
  `needs_user`): while its root waits for the user's answer (not its pilot's: a question that
  went to the pilot is the pilot's to answer), a request of its tree is held for the user (a
  launch held there among them; not one its parent or the judge holds), or an agent of its tree
  asked for a vault entry; `running` while a turn runs in its tree, `idle` otherwise.
- Opening a session of another machine shows its page only (#1271): the list stays this
  machine's.
- **The bell and "Ended" hold every machine's** (#1280). The window follows each other runner that
  is online: its notification centre by a light link of its own (made once the device key is read, signed as every
  request to that machine, and reading `notifications.list` at each connection and on each
  `notifications.*` event of that machine, and `provider.list` on each `provider.*` event, so while the bell's list is open too): the bell lists that machine's notifications beside this
  one's, each session's heading saying `on server`, and its badge and counts add them. Choosing one
  opens that machine's page of its session, and opening that page, or **Mark all read**, marks them
  read on that machine (`notifications.read`); the request or question is answered on that page,
  so on its machine. "Ended" asks each other machine for its ended roots (`agent.list`, and each
  one's brief for its title, the latest 50) when the window first reaches it and each time the
  group is opened, one read at a time (a session gone between the list and its brief is left out);
  a row of it opens that machine's page of the session. A machine that goes off has its link ended
  and asked no more until it is back: its notifications leave the bell and its counts, its rows
  leave "Ended", and the group says it once, by one line (`server is offline since 22:14: its ended
  sessions are not listed`). A link the other machine does not answer is dialled again with a
  growing wait (up to five seconds), which starts over only once it was read; a notification not
  marked read there is marked again the next time. What another machine answers only ever names its
  own sessions: each is keyed by that machine (`a1@server`) and under no row. A project of another machine only is a group of the list
  whose page is that machine's.
- A close of a session of another machine goes through this machine's daemon whether that machine
  is on or off: kept, then sent at once when it is linked. A close is kept 30 days, as every
  message kept for another machine.
- **Turning remote on or off is not held** (#1144): the user asked for it, the off one after a
  confirmation.
- **Nothing of the direct link is left** (#1184, the maintainer's decision of 2026-11-21): no
  `listen`, no address found, declared or told, no dial of an address. What a user had before
  still works at the update, with no protocol version changed: a `daemon.toml` with `listen`
  starts, the key left as written and said once in the log as no longer read; a key folder with
  `addresses.json`, or `addresses` and `told` in `peers.json`, has them removed at the link's start
  (the machines trusted kept); a peer of before that tells its addresses on a link is answered and
  ignored; a hub of before that sends them, and a machine of before that asks the hub to keep them,
  keep their sessions (each side reads the field and ignores it). A setting of `listen` sent to
  `remote.configure` is not read.
- **The hub's hello is signed, with two limits taken** (#1232): something on the way that strips
  `signed` *and* rewrites the device's hello can still make a new hub choose a lower protocol than
  the device speaks; harmless today, because neither the hub nor `hub_session.rs` reads
  `protocol()`. And the refusal's signed layout went from `flottille-hub-refused-v1` to `v2`, so a
  signed refusal between this version and v0.4.1-beta.23 or 24 does not check on either side: each
  falls back to "unreachable", with no false verdict. Accepted for pre-releases.

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
  lives, until its report or its failure comes back, or its parent stops it or ends; a launch whose code never came holds a slot of the
  receiving machine's cap until it is forgotten, an hour after it was asked.
- A machine may leave 4 launches waiting for their code on another.
- The returned commits go to `refs/flottille/machines/<machine>/<launch>/<branch>`, never onto
  the user's branch.
- Deliveries between machines are kept 30 days by the sender; the user's requests to an offline
  machine are refused.
- What crosses machines (#817): a child's report, question, failure, idle and stuck notices, the
  notice that its process died and it goes on (`child-replaced`), and a led session's
  `session-turn-ended` (at each end of a turn, and when it ends or fails
  unless it ended with its pilot, its body the local wake's), and since #818 a child's held request (`child-asks`) and what a led
  session asked the user while its pilot's machine was off (`session-meanwhile`), and since #1272
  `led-integrated` (checked on the session's machine, against its forge and its base) and
  `detached`.
- **A session led from another machine is a led session** (#1272). The machine that runs it
  records its pilot as the caller stood in for there (`launched_by` and `led_by`
  `<caller>@<machine's key>`, `led`), kept while it leads a session there or did before it was
  detached; a handed session records the caller the same way, not `led`. Such a stand-in counts
  in the cap of 64 of its machine no more once every session that names it has ended, and one
  that did not count takes a place again, with its next launch, only where the cap has room. Its first
  message says where a question goes as a local one's, and, on a line of the same paragraph (which the window
  does not show) `While pc is not linked to this machine, ask reaches the user's inbox instead.`;
  the launch's answer says what a local one's says, with the place and its return, and a held launch says what follows once the user allows it
  there. Its pilot follows it on its own machine, a subscription of the source `session`
  naming the machine (`forge show`, `subscription.list`, the window's rail): `forge unsubscribe`
  or the user's Cancel tell that machine, kept until it takes it, and its questions go to the user
  there; the following pauses after fifty `session_send` of the pilot with no message of the user
  to the pilot, and resumes when the user writes to the pilot or resumes it. The user detaches it
  there as on one machine: its pilot is told (`detached`; the pilot of a handed session, which
  follows nothing, is told nothing), its following ends and the launch is
  the user's on both machines (no longer a row under its pilot, reached with a leave as any
  session of another machine). A real end of the pilot ends it there (`its pilot ended`), the stop
  kept until that machine takes it; `agent.ending` and the window's End confirmation list it,
  with its machine and its unsaved work as that machine reads it (#1354): asked there with a
  `session_losses` `call` of the launch, all such machines at once, its answer awaited 3 seconds at
  most, masked and cut here as any answer of another machine; `unknown (on server)` when it is
  off, does not answer in time or cannot (a machine of before). A `following`, a
  `pilot-ended`, a `parent-stopped` or a `parent-failed` stop is taken only from the machine of the launch's
  caller, never for a handed launch; a `following` only for a led one, its state `on`, `paused`
  or `ended`. The light list does not name a pilot of another machine (its
  `pilot` is a session of the same list): the message `pilots` says it beside the list, and
  every window draws it under its pilot ("One list of every machine", "Led from another
  machine").
- **A parent's end stops its tree on other machines** (#1277). An agent that ends for good (it is
  stopped, reports, or ends with its own parent or pilot) stops each child it started on another
  machine there (`its parent stopped`), and one that fails stops them too (`its parent failed`),
  as its children here end; each of them, and each session it leads, ends in turn what it
  started, there or on a third machine. The stop is an `agent_stop` `call` of the launch, kept
  until that machine takes it as the pilot's is, and only for a launch this machine made, never
  for anything another machine names. A failed pilot ends no led session, here or there: it can
  be resumed.
- **A held launch goes with its caller, and a stop is never lost** (#1354). A launch held for the
  user there names its agent already, and is stopped as any: that machine withdraws it (its
  request goes, nothing is laid, its workspace is removed), the agent's reason saying the caller's
  end (`withdrawn: agent … of pc, which asked for it, ended`, or `failed`). A launch whose code is
  still travelling when its caller ends, whose agent is not known yet, is stopped as soon as that
  machine names it. The stop is kept under `stop-<launch>` whatever the queue for that machine
  holds: it replaces the calls kept for that launch (a message, a following, an earlier stop),
  which it makes useless, so that the queue grows by one at most per launch it ends.
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
- `session_read` and `agent_status` of a session or child the caller started on another machine
  answer as on one machine: the same pages, `last`, `since`, `until`, `cursor` and cuts, the same
  refusals (`agent_status` of a led session is refused, `session_read` of a child too). That
  machine answers them by the same tools as a read of a session the caller did not start, for the
  caller stood in for, which reads the session it started without a leave and its child as its
  parent; the answer is masked again on the caller's machine and cut there too, as every answer
  of another machine to a tool of the tree is (a send, a stop, an approval's answer, their
  refusals and "cannot answer now"), both halves of a secret a cut of that machine split masked
  too.
- `agent_status` without `agent_id` lists the children of this machine only; a child on another
  machine is read by its id.
- A delivery is stored there under `<launch>-<its id>`: sent again, it is stored once.
- At most 100 messages wait for one machine: one more `agent_send` or `session_send` is refused at
  once (`refused: 100 messages wait for server already`), and one more delivery is not kept, with
  an inbox entry (`dropped`), but the user's answer to a launch or to a held call of another
  machine's agent, kept beyond up to 104 in all. A message of more
  than 64,000 characters is refused on the sender, as the receiver would; that answer is cut
  instead, and so is a review's result to a pilot of another machine, the cut said with the `session_read` that reads the rest there. The kinds a machine takes are the
  session's own and a review's (`review`), no other extension's name.
- A led session's plain end of a turn (it neither asked nor reported) is kept once per session
  for the machine that took none: the next replaces the one still waiting, so that the turns of a
  session that runs while its pilot's machine is off never fill the 100 and push out a report or
  another session's news. What asked, reported or ended is each kept.
- A message the other machine cannot take now (`later`) is sent again 30 seconds later, or sooner
  on the next link or message kept.
- Pair mode (#1146) lasts 5 minutes unless `--minutes` says 1 to 15, works once, and one is open
  at a time: a second `flottille-hub pair` is refused while one is open, rather than closing it. A
  wrong proof pairs nothing, ends its own session and leaves pair mode open, whatever their number
  (the hub's log names the key that sent each): a limit that closed pair mode would let anyone who
  reaches the hub with a throwaway key keep the user from ever pairing, and guessing a 128-bit
  secret is out of reach with or without one.
- The hub signs the devices' entries with its own key, the one every device pins from its code,
  rather than a key of the user's kept apart: the maintainer accepted that the hub's host can pair
  a device ("What a compromised hub can do"), and a second key on the same host would protect
  nothing more. A list made since pair mode starts with the hub's own change (`hub`, naming its
  key, after a new list id), so that every list has a root and an id; one made before keeps its
  first member as root.
- A device paired again with a key that is a member keeps its entry, role and card: what it
  proposed is not taken. A key removed, paired again, is a member again (the hub's `pair` change
  takes it back), so that a device's key is made once.
- The card's colour is one of 8 names, the clients drawing their own shades; a device proposes the
  one of its key's first byte. Its icon is one of 5. Both are signed in the list (`pair`, `look`),
  as a name is, rather than kept by the hub alone, so that the hub cannot restyle a device either.
  What a device says of itself and what the hub knows (its kind, system, version, when it was
  paired and last seen) is the hub's word, unsigned: nothing in it gives a right.
- The proposed icon is the machine's: a battery (`/sys/class/power_supply`, type `Battery`) makes
  it a laptop, a screen connected (`/sys/class/drm`) a desktop, neither a server. The proposed role
  is `runner` for a server (#1247), and for any machine where an agent CLI is signed in,
  `controller` otherwise. `flottille remote pair` says the role it takes before it asks the hub.
- `flottille-hub pair`'s QR is drawn in half blocks, light modules as blocks and dark ones blank,
  as a dark console shows them, with a quiet zone of two modules; the code is printed above it,
  to copy.
- The commands of the hub's host reach the running hub on a Unix socket in its state folder,
  never a network port; a connection from another user than the hub's is refused, root's taken (a
  systemd service runs as a user of its own).
- The pairing's session takes the declaration of the device's machine, 30 s at most; a device
  paired whose session ended before it declares its machine declares it at its next pairing.
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

- The link listens nowhere and opens no socket of the network: its packets go in the session to the
  hub. A link that came in through the hub takes at most 5 seconds to finish its TLS, 256 at once
  (a stalled handshake costs a few kilobytes), 2 from one key (the relay gives each key an IPv6 /64
  and /48 of its own) and 8 from one /48; one that finds no place free is refused at once, and
  dials again. Whether a key is trusted is said after its TLS, so that the other side is told why
  it is refused, by the peers' file as last read (read again only when it changed since). A key
  refused is said in the log once a minute at most, and 16 keys a minute at most: past them, one
  line says the others are counted, and the count is said when the minute ends. The machines a
  daemon trusts are in `remote/peers.json` (`{"machines": [{"name", "key"}]}`), which the control
  session to the hub writes from the member list once the machine is paired. It is read at every
  round (each second): the daemon dials each machine it trusts, the hub says online, and is not
  linked to, waiting longer after each failure (5 seconds at most). A file caught half written,
  or broken by hand, leaves the last one read whole in use (said once in the log).
- Two machines that dial each other at once each hold two links: both keep the one the machine of
  the lower key dialled, and close the other (`linked already`).
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
- The client's link is dialled through the hub by the machine that carries the request. Carrying
  the user's requests over a link the other machine dialled is left for the phone.

- A tunnel (#829) is held by the command that opened it, which stays in the foreground
  (`flottille --machine server tunnel open r1`): Ctrl+C closes it. A tunnel that outlives its
  command would need a name and a way to find it again; the window and the phone hold theirs for as
  long as they are open. Reversible: a `--detach` is a later flag.
- **A link an agent shows names no run (#830).** Its port is looked for among the machine's running
  commands (`run.list`): the one that reports it, else the only one that runs, the machine
  checking the port again. A guess that is wrong is refused there (`port 5173 is none of run r2's`),
  never reached. Reversible: the agent's own run could be named by the link later.
- **A file of another machine is not opened (#830)**, but for a picture or a video (#1150): any
  other file's card says where it is and draws no Open.
- **A video of another machine plays from a copy written here (#1150)**: the player is given the
  file as soon as its first chunk is in, and the rest is written into it in order; a seek in the
  player reaches what has arrived. The machine serves any range (`agent.file {offset}`), so a
  player fed through a local range server could seek ahead; a temporary file was the simpler,
  reversible step, and needs no listener on this machine.
- **The pace of `agent.file` is the machine's, not each device's (#1150)**: 2 MiB a second for
  every asker together, with at most 8 calls in flight and a wait of at most 3 seconds, past
  which the call is refused at once (the daemon does not know the asking device at that point).
  Reversible: one budget per device is a map.
- **The window's tunnels are not listed again after a restart (#830).** The listener is the
  window's connection's: a window that quits closes them, and one that opens again starts with
  none (`tunnel.list` still lists what a command line holds).
- **On the phone (#831), one notification for all the tunnels kept**: the service shows one
  ongoing notification that lists them, and its Close closes every one, rather than one
  notification and one Close each (a foreground service has one notification). Reversible: a
  notification per tunnel is a loop. A tunnel ends with the phone's link to its machine, as the
  daemon's does: a link that drops and comes back is a tunnel to open again, said as a notice.
- **A run's address is listed only for the followed session (#831)**: the page derives the runs from
  the events it holds (the commands of the session's own agent), so a run a child started, or one
  that started before the page's latest events, is not listed; the links an agent showed are in
  the conversation as they are for any message. Reversible: the core can ask `run.list {tree}`.
- **What remains exposed on the phone (#831).** The phone's listener has no token: any app of the
  phone (one with the INTERNET permission, or a page in the user's browser by cross-site requests
  or DNS rebinding) can connect to `127.0.0.1:<port>` and read the run's page with the phone's
  identity, for as long as a tunnel is open. What bounds it: the port is random (an app has to
  scan for it), the in-app browser's tunnel closes with its panel, and an **Open in browser**
  tunnel, which the user's browser needs for as long as it likes, ends after 10 minutes with no
  connection and when the user closes it from the notification. Another app that scans the
  loopback in that time can still reach the page. Serving the in-app web view without a listener
  (`shouldInterceptRequest` over a stream of the link) would remove it for the in-app view, but
  needs a raw-stream API in the core and the web view's own request handling (cookies, redirects,
  websockets) rebuilt over it: not done. Reversible: the listener is `bind_local`.
- **What the idle end counts, and what it leaves (#1230).** The idle end of an **Open in browser**
  tunnel counts connections, not bytes: a connection that only opens and closes starts the 10
  minutes again, and a page that keeps a connection open (a dev server's live-reload websocket)
  keeps the tunnel for as long as the page stays open in the browser. That is the use (the page is
  in the user's browser and is in use), and the notification's Close is the way out; counting bytes
  would end a tunnel under a page that is only listening. Reversible: `accept` is the one place.
  The in-app browser's tunnel has no idle end either: with an **Open in browser** tunnel kept, the
  service keeps the core connected with the app in the background, and an in-app panel left open
  then keeps its tunnel until the user comes back and leaves it. Closing it when the app stops
  would lose the page at every switch of app; the panel is on screen when the user returns.
- **Cache and service workers (#1230).** When a tunnel's panel closes or its tunnel ends, the
  cookies, the web storage and the HTTP cache of the web view are cleared. The platform has no call
  of its own for service workers: `WebStorage.deleteAllData` is the one that may include them, and
  what remains is keyed by the origin `localhost:<port>`, a port the system picked at random for that
  tunnel, which another page is unlikely to be given again.
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
- Any member may remove itself, a viewer too; a viewer may remove, rename or restyle no other
  member, and may not rename or restyle itself.
- A key that is a point of small order is refused wherever it is read (an entry, a pair code's
  hub key): no device makes one. A hub URL is `http://` or `https://` with a host, at
  most 2048 bytes, and a URL or a name holds no invisible character (Unicode's `Cf`: zero-width
  characters, bidirectional overrides), so that two that look the same are the same.
- A member's addition (`add`) of a list made before pair mode never brings back a removed key; the
  hub's pairing does.
- The pair code is Crockford's base32 with a checksum of 4 bytes, about 125 characters with a
  short hub URL, and the QR holds the same text, which a QR holds in its compact alphanumeric mode.
  It holds the hub's whole key, not its fingerprint: a fingerprint of 8 bytes is a match one could
  search for.

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
  key: no database. Pair mode is kept in memory: a hub started again has none, and the command
  that opened it ends.
- A device that pairs (#1146) says its pairing in its session's proof (`pair`: the card proposed
  and the proof), so that a key with no pairing is still refused at the end of its handshake.
  While it has not declared its machine, its session counts among the 64 places of keys that are
  no member's. In one of the 8 places kept for members, it is refused before pair mode is spent.
- The device that paired declares its machine at once, through the session it paired with: the
  daemon never holds the device key, so `flottille remote pair` waits for the pairing and signs
  the declaration then. `remote.pair` is refused on a machine paired already unless asked to
  replace the hub (`--replace`): a machine has one hub.
- A removal is carried out by every daemon that takes the list holding it: the machines it trusts
  are written again, and its link to the removed member's machine is cut at once
  (`remote.disconnected`, `it was removed from the member list`). A link to it that was being made
  when the list came is judged again as it is kept, and closed (`refused: this machine's key is not
  one of the user's`): it never gets `remote.connected`. So is a client's link to it dialled for a
  request then: closed, and the request answered `unavailable`. The removed machine's own session
  to the hub is refused, so it never sees the removal itself; its links are cut by the others.
- `remote.connected` and `remote.disconnected` are the machines' links, not the hub's session
  (its state is `remote.machines`' `hub`, and `remote.hub` says it changed).
- The formats of #820 changed without a version of their own, nothing being released: the
  device's proof has `pair` (#1146, in place of `join`), the ask `about` (in place of `setup`,
  `invite` and `refuse`), and the hub sends `facts` (in place of `requests`). The protocol number
  stays 1: the hub and the machines of a user are updated together.
- `flottille-hub reset` is refused while a hub uses the state folder, rather than told to it: the
  running hub would undo the reset at its next write. It reads nothing of the state, so a list that
  does not check, or a damaged file, does not stop it; the hub's key stays.

- The list holds the devices' keys; the link and the control session run on the machines' keys.
  The tie is a change of the list, `machine` (kind 4 of the signed bytes: the member's key, then
  the machine's), which a member signs for itself only, any role, and the machine claims: its
  `claim`, the machine key's signature over `flottille-machine-claim-v1`, a zero byte, the
  member's key and the hash of the change before, is part of the change's hash. The hub cannot
  make it, a member cannot declare a machine whose key it does not hold, and a removal takes it
  away. A machine key is no member's key, nor another member's machine, nor the hub's; declared
  again, it replaces the one before. The hub signs no machine.
- The first change of a list names, in place of a previous change, the list's id: 32 random bytes
  made with the list. Nothing of the formats is released yet: the signed bytes changed with it, and
  with the claim, without a version of their own.
- The hub listens on `127.0.0.1:7448` unless told otherwise: #820's "every interface" is the
  devices' link, not the hub, and a plain hub on every interface would carry every session in
  clear. Another address takes TLS, or `--plain` for a reverse proxy elsewhere.
- ACME is not built (a dependency of weight): `--acme` is refused with what to do instead. TLS,
  on the hub and to it, is TLS 1.3 only.
- A machine that cannot reach another says why, from where: `server is offline: it is not connected
  to the hub`, `server is unreachable: tried through the hub from pc, no answer in 10 s`; `from` is
  the machine's name in the member list (its key's fingerprint before it has one). A hub that cannot
  be reached is said the same way, with since when: `the hub at hub.example.net is
  unreachable since 22:14: tried 192.0.2.10:443: Connection refused`.
- A machine paired tells the hub what it is at once; an ask the hub does not answer in 10 s is let
  go, and the session goes on meanwhile.
- A refusal of a role names what was asked in the user's words for what they do most (`answer a
  request`, `send a message`, `start a session`, `remove, rename or restyle a device`, …), the method otherwise (`may call project.add`). Each method's class is said beside
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
- The window's Machines are a tab of Settings of their own (Remote on or off, the hub, Devices;
  `docs/reference/gpui.md`, rule 13, revised for it), and nothing of it is typed on the page:
  Connect to your hub and a rename are dialogs. Its switcher offers the
  runners, and the machines whose role is not known yet (linked with no hub's list), never a
  controller or a viewer. While the
  window shows another machine, what it keeps of itself (its folds, the drafts, its appearance)
  stays with this machine. A member's row says its role, who added it and its machine's state:
  `remote.members` says when it was paired and last seen since #1146.
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
  its offer lacks the caller's provider or, for a child, a free slot, naming the
  runners whose offers would take it; `workspace`, `new_workspace`, `repositories`, `branch`,
  `base` and `project` are refused with it, since the place is a copy of the caller's. It then
  sends the `launch` machine message (the launch's name, the caller and the root of its tree, the
  kind, the project, the title, the brief, the provider, model, effort and role, the mode, the
  kept switch to Full, the caller's branch and repositories, each repository's root commits and
  remote URL without credentials, and the project's settings that travel), and the code of the
  caller's place
  under the launch's name, and answers with the agent started there or the request waiting there.
- The receiving machine takes a `launch` only from a runner's machine, for an agent it names, while
  it runs agents itself, recognises the project by its repositories or makes it (`travelled.rs`,
  #1188), and checks the provider signed in
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
  `agent_status`, `session_read`, `agent_stop` and `review` (a led session); anything else is
  refused (`agent … of pc did not start agent …`). A `review` is only `{"action": "start",
  "agent": <the session>}` (#1279): the lead's `review {start, agent}` of a session it leads
  there, sent at once as a read is (refused while that machine cannot be reached), served there
  for the caller stood in for, which is given the review tool for that call alone, by the gate and
  the tool of a pilot of that machine; the review runs there, its findings go to the session, and
  its pilot is told on its own machine (a `deliver` of the kind `review`). Its own change, a pull
  request, a pass or a report is refused: a caller of another machine starts no other review. A call is judged on the caller's machine as one to a session it started
  there (ending a led session as `end_led`), but for the reads (`agent_status`, `session_read`):
  those are served as a call whose `launch` is empty, and judged there, the launch only saying
  which session or child the caller reads as its own. A `call` whose `launch` is empty (and not an
  `approval_answer`) is one to any other session of the receiver: served there by its own gate
  and tools for the caller stood in for ("Sessions of another machine").
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

- A row of a session on another machine says the state that machine's list says of it
  (`running`, `needs you`, `idle`), the launch keeping its row under its pilot (#1280). A session
  its machine does not list (a child, or before the list came) says its state as last known from
  what that machine delivered here: `running` once launched until its first delivery, `waiting`
  after a turn's end, a question or a request, until the next delivery says otherwise (a message
  the pilot sends does not make it `running` again here). An ended launch leaves its pilot's
  rows unless its last delivery was a report: a reported child stays, `reported`, under its pilot
  while the pilot lives. A session handed to the user is not shown under the agent that started it.
  The daemon leaves them out of `remote.launches` itself, in one read, and also a reported launch
  whose caller ended: an old launch is never read again. A launch held for the user there is
  `needs you` from the moment it is held, drawn with the request's mark, until the user's answer
  there comes back: allowed, it shows what its machine lists, or `running` until its first
  delivery; refused, it leaves its pilot's rows. While it is held, a list that says `idle` (a
  machine of before, which did not list a held launch as `needs you`) does not take it back.
- Those rows are rows of the tree only (`Snapshot.elsewhere`): this machine's daemon does not know
  those sessions, so a project's page, search, a session's page and the inbox do not list them,
  and nothing offers to end them from here.
- Those rows are always in the list, which is this machine's whatever page is shown (#1271).
- Links through the hub (#1151): a link's packets travel in the control session as JSON lines, the
  packet in base64, not as binary WebSocket frames: the session is one JSON object a line since
  #820, and a reverse proxy carries it the same. A device dials another through the hub only. The
  hub's bounds: 4 MiB a second for each
  device's packets to the others, 1 MiB at once, each packet charged before anything else is
  looked at; 128 packets from one device waiting for another, a share for each sender, so at most
  128 × the members' devices for one device; a packet past them is dropped, never queued further.
  The keepalive is 25 s each way, a session silent 80 s is lost, on the device and on the hub (on
  the hub too a line the device reads nothing of for 80 s).

- The mobile core (#824): a batch is what arrives within 30 ms of its first event, 256 events at
  most; a runner that cannot be linked is dialled again after 250 ms, doubling up to 30 s, and at
  once when the hub says another list of the members online. Its sessions list holds root sessions only, by project
  name, projects by name (rows never move between groups), sessions newest first by when they were
  made (#1441: by last exchange, a session that reported jumped to the top). The
  questions that wait come from the events seen (`agent.asked`), not from a read when the phone
  links: one asked before the phone's first link shows from the next one. The phone links to the
  runners of the list only, never to another phone or a controller's computer.

- The Android app (#825, #1443): the bottom bar has **Needs you**, **Sessions** and **Machines**,
  and Settings is reached from the top bar; the app opens on Needs you. Since #1441 the Sessions tab
  is design B's: by machine, then project, newest first, the state the row's mark; an ended session
  leaves its project for the folded **Ended** group pinned at the bottom (it said **Ended** in its
  project before). Tapping a session of an online machine opens its page (#826). The app connects
  to the hub only while it is on screen (the foreground service comes later) and dials it again
  after a wait that grows (1 s, up to 30 s) when it cannot be reached. A phone proposes its model's
  name, as a computer proposes its host's. The QR reader is
  ZXing over CameraX, not Google's ML Kit: no proprietary library in a public repository's app.
  A pull request's APK is debug-signed (the release key is never given to the run of a pull
  request, which runs the branch's own scripts); only a push, which runs merged code, signs with
  it. The launcher icon is the brand's (#1242): an adaptive icon (the mark of `assets/brand/mark.svg`
  as a vector drawable over the icon's dark ground `#141414`, the same drawable as the themed icon's
  layer), with PNG mipmaps for the legacy shape made by `apps/android/icons.sh` from
  `assets/brand/icon.svg`; the notifications' small icon is the mark in one colour. The app's
  `minSdk` is 31, so the legacy PNGs and the `-v26` folder are never read: the issue asked for them
  and they are kept, a few kilobytes, until a decision to drop them.

- Needs you (#1443) orders the cards newest first by the time the mobile core saw each come, in the
  phone's clock; one that waited before the link takes the daemon's time of it, in the machine's
  clock, which may differ from the phone's by the clocks' drift. A question asked before the phone
  linked is not in the core's view until a page of its session is opened (the `asks` come from the
  events seen and the followed tree), so Needs you shows it from then on: reading the questions when
  a runner links is the core's to add.

- The session page (#826): a request's call is the window's `call_line` computed again in the
  mobile core (a few lines, the same words), not moved to a shared crate (`flottille-core` carries
  SQLite, which the phone does not need): a follow-up may move both to `flottille-protocol`. A
  question that waits when the page opens is read from the tree (`agent.tree`), so a question asked
  before the phone linked shows on the page; the sessions' list marks it waiting from the events
  only, as before. Pictures shown by an agent (`agent.file`) and the request lines inside the
  conversation, the queued messages (Send now, Edit, Cancel) and Resume of an ended session are not
  on the page yet: this page shows the pictures of the user's messages, the live conversation, the
  cards and the controls. Full asks for a confirmation on the phone (the window has none: a tap in a pocket is not a click). A shared picture asks the
  user to pick the session in the list (a banner, with Discard) rather than a picker of its own. The
  answered card folds 2.5 s after a refusal, long enough to be read.
- **A held launch an older machine lists `idle` still needs you** (#1333): a machine of before
  lists a launch held for its user `idle`, as it lists nothing it holds; while the launch is
  `held` here, that `idle` is not taken over the request's mark. Any other state that machine
  lists wins, so an allowed launch shows what it does as soon as its list says so.
- **The End confirmation waits 3 seconds for another machine** (#1354): the machines of the
  sessions the pilot leads elsewhere are asked at once, and one that has not answered after 3
  seconds is said `unknown (on server)`, rather than holding the confirmation longer.
- **A stop replaces the messages kept for its launch** (#1354): rather than going past the 100
  messages a machine may leave waiting, a stop takes the place of the calls kept for the same
  launch, which it makes useless; a message the agent would have read before its stop is not
  delivered.
- **A held launch's withdrawal is said on the agent there** (#1354): its reason, `withdrawn: agent
  … of pc, which asked for it, ended`; the user's request goes as any request of an agent that
  ended.
- **The review of a led session on another machine is not kept while that machine is off**
  (#1279): the lead's `review {start, agent}` is sent at once and refused while the machine
  cannot be reached, as a read or a stop is, rather than kept as a message is: a review started
  hours later, on a change that moved meanwhile, is not what the lead asked for. The lead calls it
  again once that machine is back. The lead's daemon sends `{"action": "start", "agent": <the
  session>}` alone; that machine refuses any other argument rather than drop it. The refusal
  says to call again once that machine is back, and does not promise a wake-up, as nothing comes
  back from a review that was not started. A machine not updated yet (it answers that `review` is
  no tool another machine calls) is told to the lead as a machine to update.

- **A thing to notify is decided once, at its 30 seconds** (#827): told then, or dropped when a
  client had an input in the last 2 minutes; it does not ring later if the user then goes away. A
  row the user has not read is still in the window's bell and the phone's list. What is decided is
  kept in memory: after a restart of the daemon, what came before is never told.
- **Only an input counts as the user being there** (#827): a window that is focused and untouched
  for 2 minutes is away, as the issue says it ("had an input"); the focus is told to the hub
  (`active`) and not read by the runners.
- **The phones told are the sessions that asked for the notices** (#827), in the `about` the app
  tells the hub; no role or icon decides it. A `viewer`'s phone is told too, and draws no Allow or
  Deny (#828). This app's core asks for them already, and ignores them until #828 draws them.
- **Neither side of before #827 loses its session while the other is updated** (#827): a hub of
  before closes the session of a runner that asks `notify`, and a phone of before ends its session
  at a `notice`. So each says it takes them, with no new protocol number: the hub's welcome says
  `{"welcome": {"notices": true}}`, and a runner asks `notify` only on a session whose welcome said
  it (with a hub of before, nothing reaches the phone until the hub is updated); the app says
  `"notices": true` in its `about`, and the hub sends `notice` only to the sessions that said it (a
  phone of before is told none until it is updated). Both fields are read as `false` when absent,
  and a side of before ignores them.
- **The phones' presence is not counted** (#827): the app says an input each time it opens its
  session, so a phone that changes network would silence the next notices. A runner counts the
  presence of the members whose device says it is no `phone`; the app may tell its real inputs
  later (#828).

- **What an end sends back, and the removal of a workspace there** (#1278): a child stopped by
  its parent is told `child-ended`, with its commits, though it was its parent that stopped it:
  the delivery is the only way commits travel, and a parent that stops a child with work in it
  learns where that work went. A removal asked from another machine sends the commits back on its
  own before it is judged, rather than refusing until an end brought them: the caller asks once.
  A removal that runs while an end's delivery is sending the same commits waits for it (one send
  back at a time for a launch, another launch's going on meanwhile); if the removal goes first,
  that delivery says nothing of commits that all went back.
- **In Full, a removal from another machine is held only when it would lose something** (#1278):
  the caller works in no workspace there, so the rule of one machine (the removal of the workspace
  an agent works in waits for the user's word, even in Full) does not apply as written. The removal
  sends the commits back first; what it would still lose (work never committed, or commits that
  could not go back) makes it wait for the user's word there, and a removal that loses nothing is
  done as Full does any other, unless work came after its card: what it finds then is refused,
  never discarded unseen.
- **Two launches of one caller whose workspaces there have one name** (#1278), in two projects
  there (a project made again for a branch of another root): the caller's daemon sends the removal
  for the first of its launches holding that name, and the machine there removes that launch's
  alone. Naming which, by its session, is a follow-up if the case is met.
- **The session page on the phone** (#1442): the mode's chip is drawn only where the mode may be
  changed (not on a child's page, a viewer's or an ended session's); choosing the mode already
  chosen sends nothing; Full keeps its confirmation, which the extended design left open. The
  tree's icon is always in the top bar (the pilot is a row of its own). A request answered on
  another device leaves no record on the phone: the phone knows only the answers it gave. A
  request sits after the line that was the conversation's last when it came. The composer's field
  says `Write to the agent` until something is written (its label for a screen reader is
  `Message`). A delivery the core does not know by name is one line of its kind and its first
  words, as the window's notice. The machine's mark of #1249 is not drawn yet: the phone has no
  machine colour to draw it with; the subtitle names the machine.
- **The phone's Sessions tab keeps its own folds** (#1441): what a project, a pilot or "Ended" was
  left as is kept in the app's storage, not in the window's state on a machine (`window.get`), so
  the phone and the window fold apart. A session whose pilot runs on another machine, or works in
  another project, is a row of its own in its project rather than under that pilot. "Ended" is one
  group pinned below every machine, its rows newest ended first by their last exchange (the view
  has no `ended_at`).

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
