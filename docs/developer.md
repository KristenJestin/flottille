# The developer mode

The developer mode is a log of what Flottille decides and why, for the person and the agents
working on Flottille: after the fact, it says what the gate decided on a call and for what
reasons, what the Auto judge was given and what it answered, the request it made and what came of
it. It is off by default; off, nothing is written beyond what Flottille always writes, and nothing
is put together to be dropped.

It is switched in Settings ("Developer mode"), with `flottille developer on` and `off`
(`docs/cli.md`), or by the daemon's `developer.set` (`docs/protocol.md`). The choice is
`log = true` in `developer.toml` of Flottille's configuration folder
(`$XDG_CONFIG_HOME/flottille/developer.toml`), absent while it is off; the daemon reads the file at
start and again on each `developer.show`.

## Where the log is

`$XDG_STATE_HOME/flottille/developer.log` (or `~/.local/state/flottille/developer.log`), readable by
the user only. Past 10 MB it is moved to `developer.log.1`, replacing the one there, and a new log
begins: the two hold the last 10 to 20 MB. Settings and `flottille developer show` say its path.

## What it never holds

Every text of a line is masked before it is written, as everywhere in Flottille: the values of the
daemon's variables whose names say they are secrets, tokens of the kinds Flottille recognises,
private keys, and the secrets of the agent's place (its `.env` files) for a call. The agents' login
files are never read, so nothing of them can reach it. A call's arguments are those the store
records: each text cut at 500 characters. The judge's input is its prompt, which holds the call
already masked and cut as `docs/design.md` §7.1 says.

## The lines

One JSON object per line. Every line has these fields; a reader ignores a field it does not know,
and a field is never renamed.

| Field | |
|---|---|
| `at` | When it was written, in milliseconds since the Unix epoch. |
| `kind` | `call` or `event`. |
| `agent` | The agent the line is about; `null` for an event about no agent. |

### `call`

One of an agent's tool calls, once Flottille answered it: what the gate decided, the judge, the
outcome.

| Field | |
|---|---|
| `tool` | The tool's name, without the `mcp__flottille__` prefix. |
| `call_key` | The call's key (`claudecode/toolUseId`), `null` when the call had none. |
| `arguments` | The arguments, each text cut at 500 characters. |
| `gate` | `allow`, `judged` (to judge, allowed by the judge), `full` (to judge or base, allowed by Full mode), `deny` or `ask`; `null` for a call refused before the gate (the agent has no such tool, or was replaced). |
| `allowed_by` | Who let a held call through without the user: `the judge` or `Full mode`; else `null`. |
| `class` | For a call the gate held: `judge` or `base`; else `null`. |
| `reasons` | Why the gate held it, one sentence each; empty when it did not. |
| `judge` | When the Auto judge was asked: `{"by", "provider", "model", "input", "answer", "verdict", "reason", "duration_ms"}`; else `null`. `input` is the whole prompt it was given (`null` when it could not be asked, for a line or a path too long), `answer` what it said, whole (`null` when it said nothing in time or failed), `verdict` `allow`, `ask` or `failed`, `model` `null` for the provider's default. |
| `outcome` | `ok`, `refused`, `failed` or `held`. |
| `request` | For a held call: the number of the request it made, which the `approval.*` events carry; else `null`. |
| `error` | The text the agent was given for a refused or failed call; else `null`. |
| `result_chars` | The length of the text the agent was given, in characters, before it was cut. |
| `duration_ms` | How long the call took. |

### `event`

An event of the daemon (`docs/events.md`) that reaches a decision or comes of one, as it was
emitted: the requests and their answers (`approval.*`), the deliveries (`delivery.*`, `how` said
by `delivery.sent`), an agent's process and session (`agent.created`, `agent.started`,
`agent.resumed`, `agent.released`, `agent.died`, `agent.failed`, `agent.ended`,
`agent.replaced`), its mode (`agent.gate_mode_changed`), its permissions
(`agent.permission_answered`), its provider's errors (`agent.provider_error`,
`agent.provider_limit`), the commands run (`run.started`, `run.ended`), a setup step that failed
(`workspace.step_failed`), the judge chosen (`provider.judge_changed`), and the daemon's start and
stop.

| Field | |
|---|---|
| `seq` | The event's number. |
| `name` | The event's name. |
| `data` | Its fields, as `docs/events.md` gives them. |

## Reading it

```
$ jq -c 'select(.kind == "call" and .judge != null) | {agent, request, verdict: .judge.verdict}' \
    ~/.local/state/flottille/developer.log
$ jq -r 'select(.request == 12) | .judge.input' ~/.local/state/flottille/developer.log
```
