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
to unlock it, as it does for any program.

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

## The secret is write-only

It is set from the window (Settings, Vault) or with `flottille vault set <name>`, which reads it
from the standard input or a prompt that does not echo it, **never from an argument** (the shell's
history and the process list would keep it). It can be replaced (the entry is set again, its secret
typed anew) or removed with its entry. It is **never shown again**: no method, no client, no tool
returns it, no event or log line holds it, and an error about a call that carried it never repeats
it. The window's field is masked, and empty again as soon as the secret is sent.

Every value of every entry is added to the masking of everything the daemon stores and shows
(`flottille_core::secrets`): an agent that came across a secret some other way and writes it in a
message, a command's output, a tool's arguments, finds it `[masked]` there. The values are read
from the keyring when the daemon starts, before any agent resumes (the start waits 5 seconds at
most for a keyring that asks to be unlocked, then goes on), and a new secret is masked as soon as
it is set. A secret replaced or removed stays masked until the daemon restarts: another entry may
hold the same value.

## What agents see

The `vault` tool, listed to an agent whose project sees at least one entry (`docs/tools.md`, "The
vault"): `vault {action: "list"}` gives the name, the description, the URL and the username of each
entry the agent's project sees, so that it knows what exists and what it is for. No action returns
a secret. An entry of another project is not listed, and is no entry for a tool of the agent's.

## How a secret is used

Through Flottille's own tools: a tool that needs a secret takes the entry's **name**, and the
daemon puts the secret where it goes itself (an HTTP header, a command's standard input, a form
field), with `vault::secret`, which only a tool's own code calls and whose value never leaves that
use. The first users come with their own issues: `tickets log_time` (#383) and the Browser
extension.

## The tests' keyring

The tests never reach the user's keyring: their daemon's configuration folder holds
`keyring.toml` with `backend = "memory"`, which keeps the secrets in the daemon's memory, lost when
it stops. It is chosen by the daemon's configuration, never by an environment variable an agent
could set for what it runs.
