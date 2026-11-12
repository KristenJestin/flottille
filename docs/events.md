# Events

Every change the daemon makes emits an event. Events are stored, numbered, and streamed to the
subscribers of `events.watch` (`docs/protocol.md`); `flottille watch [--after <n>]` prints them,
one JSON object per line. This file is the contract: an event is added here with the code that
emits it.

## Shape

```json
{"seq": 12, "at": 1792843730808, "name": "daemon.started", "data": {"pid": 4242, "version": "0.1.0"}}
```

| Field | |
|---|---|
| `seq` | The event's number. Grows with every event, across daemon restarts; a number is never given twice. |
| `at` | When it happened, in milliseconds since the Unix epoch. |
| `name` | `<subject>.<what happened>`. |
| `data` | An object, its fields given per event below. A client ignores fields it does not know. |

## The events

### `daemon.started`

The daemon is listening.

| Field | |
|---|---|
| `version` | The daemon's full version. |
| `pid` | Its process id. |

### `daemon.stopping`

The daemon stops: it accepts no new connection and closes the existing ones once their
subscribers have this event.

| Field | |
|---|---|
| `reason` | `request` (`flottille daemon stop`), `signal` (`SIGTERM`, `SIGINT`), or `socket` (the socket at its path is gone or is another daemon's: its runtime folder was removed; checked every 2 seconds). |

### `project.added`

A project was registered (`project.add`), or its file appeared in the configuration folder.

| Field | |
|---|---|
| `name` | The project's name. |
| `folder` | Its folder. |

### `project.changed`

The project's file was edited: the daemon noticed it at its next request about projects.

| Field | |
|---|---|
| `name` | The project's name. |
| `folder` | Its folder. |

### `project.removed`

The project was removed (`project.remove`), or its file is gone. Never its folder: a removal
emits it last, after the events of what it stopped and removed (`agent.ended`, `run.ended`,
`workspace.removed`).

| Field | |
|---|---|
| `name` | The project's name. |

A project already there when the daemon starts is not announced, and a file that cannot be used is
neither a change nor a removal: the project it was stays as it was until the file is fixed or taken
away.

### `provider.changed`

A provider was enabled or disabled through the daemon (`provider.enable`, `provider.disable`).
An edit of `providers.toml` by hand is not announced: it is read again at the next check.

| Field | |
|---|---|
| `name` | The provider's name. |
| `enabled` | Whether it is enabled now. |

### `provider.models_read`

The daemon read a provider's models with no prompt (`docs/agents.md`, "The models"): at its
start, when the provider was enabled, or when its program (path or modification time), the names
of its API keys set or the presence of its sign-in file changed. The window reads the providers
again.

| Field | |
|---|---|
| `name` | The provider's name. |

### `provider.models_learning`

The daemon started listing a provider's models (`"learning": true`), or the listing ended,
whatever its outcome (`"learning": false`). The window shows the effort picker's skeleton for a
model whose efforts are not known while it lists, and reads the providers again.

| Field | |
|---|---|
| `name` | The provider's name. |
| `learning` | Whether the listing runs. |

### `provider.new_session_changed`

The user chose what a new session starts on, or no choice, through the daemon
(`provider.set_new_session`). An edit of `providers.toml` by hand is not announced.

| Field | |
|---|---|
| `new_session` | `{"provider", "model"?, "effort"?}`, `model` and `effort` left out for the provider's to propose; `null` when none is chosen. |

### `provider.judge_changed`

The user chose the Auto judge, or none, through the daemon (`provider.set_judge`), Flottille picked
one for a user who never chose (`"picked": true`), or the user saw the notice of the one picked
(`provider.judge_noticed`). An edit of `providers.toml` by hand is not announced.

| Field | |
|---|---|
| `judge` | `{"provider", "model", "picked"?}`, `model` `null` for the provider's default; `null` when none is chosen. |

### `judge.changed`

The user edited the Auto judge's guidance, or reset it to the default (`judge.set`,
`judge.reset`). It has no field: `judge.show` says the text.

### `developer.changed`

The user switched the developer mode (`developer.set`, `docs/developer.md`).

| Field | |
|---|---|
| `log` | Whether the log is written now. |

### `appearance.changed`

The user chose the window's appearance (`appearance.set`): every window takes it.

| Field | |
|---|---|
| `theme` | `omarchy` or `flottille`. |
| `shade` | `light` or `dark`. |

### `compaction.changed`

The compaction's settings or guidance changed, or Flottille picked a compactor for a user who
never chose one (`docs/protocol.md`, "The compaction methods"): its data is what
`compaction.show` answers.

### `tool_results.changed`

The user chose the limit above which a tool result is cut (`tool_results.set`,
`docs/tools.md`, "Long results").

| Field | |
|---|---|
| `limit` | The limit in characters, or `"off"` for no cut. |

### `sessions.changed`

The user chose after how many days an inactive session is ended, or switched that off
(`sessions.set`, `docs/design.md` §5.7).

| Field | |
|---|---|
| `days` | The days, 1 to 90, or `"off"`. |

### `workspaces_folder.changed`

The user chose where workspaces go (`workspaces_folder.set`, `docs/projects.md`, "Where the
workspaces go"); only the workspaces made afterwards follow it.

| Field | |
|---|---|
| `project` | The project whose own folder changed, `null` for the default of every project. |

### `extension.changed`

The user switched an extension or changed its settings (`extension.set`), globally or in a
project: `extension.list` says what it is now.

| Field | |
|---|---|
| `name` | The extension's (`specs`). |
| `project` | The project whose file changed; `null` for `extensions.toml`. |

### `connection.changed`

A connection was declared, edited, removed, reviewed or accepted, or its server started, stopped
or failed to start (`docs/extensions.md`, "Connections"): `connection.list` says what it is now.

| Field | |
|---|---|
| `name` | The connection's. |
| `state` | `off`, `awaiting review`, `cut off`, `signed out` or `ready`; `null` once it is removed. |
| `running` | Whether its server runs. |
| `why` | Why it failed to start, or what changed when it was cut off; `null` otherwise. |

### `connection.cut_off`

The connection's server is no longer what the user accepted, found as it started or when it said
its tools changed: its tool left every session, and the inbox lists it (kind `connections`).

| Field | |
|---|---|
| `name`, `tool` | The connection's name and its tool (`conn_almanac`). |
| `changed` | What changed, a sentence each (`the server is now almanac 1.1.0, not almanac 1.0.0`). |

### `connection.signed_in`

The sign-in that `connection.login` began ended well: the connection's tokens are in the vault
(`<name>.oauth`), and its server is started with them at the next call or review.

| Field | |
|---|---|
| `name` | The connection's. |

### `connection.sign_in_failed`

The sign-in that `connection.login` began did not end well: the browser did not come back within
five minutes, the authorization server refused, or its answer could not be used.

| Field | |
|---|---|
| `name` | The connection's. |
| `why` | Why, masked. |

### `connection.signed_out`

The connection's authorization server refused to refresh its tokens: they are removed from the
vault, its tool left every session, and the inbox asks the user to sign in again (kind
`connections`).

| Field | |
|---|---|
| `name`, `tool` | The connection's name and its tool. |
| `why` | The authorization server's refusal, masked (`invalid_grant: the grant is not valid`). |

### `connection.tokens_unsaved`

A refresh gave a connection new tokens, and the vault refused them twice: they are kept in the
daemon's memory, and used, until the vault takes them (the next refresh writes them again) or the
daemon stops. The inbox says so (kind `connections`). Said once for as long as they stay unsaved.

| Field | |
|---|---|
| `name` | The connection's. |
| `why` | Why the vault refused, masked. |

### `review.started`, `review.progress`, `review.reported`, `review.failed`

A review of a session started, launched or ended a pass, reported, or ended without its findings
(its pilot ended or failed first). `agent` is its pilot, so that a client following the session's
tree reads it again.

| Field | |
|---|---|
| `review`, `project`, `session`, `agent` | |
| `round`, `passes` | `review.started`: its round (1 or 2) and the passes it may run. |
| `findings`, `shown`, `changed` | `review.reported`: the findings delivered, those kept (`info` and the others), and what changed in the workspace during it, or `null`. |
| `why` | `review.failed`. |

### `review.finding_changed`

The user marked a finding (`review.finding`).

| Field | |
|---|---|
| `session`, `finding`, `status`, `agent` | `agent` is the pilot of the review that found it. |

### `review.changed`

Review's guidance was saved or reset (`review.set`, `review.reset`).

### `spec.created`

A spec was started (`spec.new`, `spec.from_ticket`).

| Field | |
|---|---|
| `spec` | Its number, never given again. |
| `project` | |
| `title` | |
| `mode` | `alone`, `internal` or `ticket`. |
| `tickets` | The tickets it covers, as the tracker writes them. |

### `spec.changed`

A Draft was written: a section, its tickets, its questions, or the session that writes it; or a
Ready spec was written to its ticket, or its departures posted; or its development session proposed
an amendment, or the user answered one.

| Field | |
|---|---|
| `spec`, `project` | |
| `what` | The section written (`problem`, `behaviour`, `decisions`, `out_of_scope`, `notes`, `verification`, `questions`), `tickets`, `asked` (the writing session asked a wave of questions), `answered` (the user answered one: a `D` was added), `writer` (the session started to write it), `ticket` (a write to its ticket, in the mode `ticket`, with `ticket_write`: `written`, `changed` or `failed`), `departures` (the user posted them), `amendments` (an amendment proposed, or refused), or, for an amendment accepted, the section it wrote (`behaviour` or `decisions`). |

### `spec.status_changed`

| Field | |
|---|---|
| `spec`, `project` | |
| `status` | What it is now: `draft`, `ready` (the user's gestures, or a development that ended with no pull request open), `in_progress` (a development launched, `spec.develop`), `review` (its pull request open), `done` (its pull requests merged). |
| `was` | What it was. |
| `agent` | The development session, when `spec.develop` launched it. |

### `vault.changed`

An entry of the vault was set or removed (`vault.set`, `vault.remove`, `docs/vault.md`). Never a
secret.

| Field | |
|---|---|
| `name` | The entry. |
| `project` | The only project whose agents see it, `null` for every project. |
| `for` | The use it is bound to, `null` for none; absent when it was removed. |
| `removed` | `true` when it was removed; absent when it was set. |

### `vault.asked`, `vault.ask_joined`, `vault.ask_answered`

An agent asked the user for a vault entry a feature of Flottille names and that is missing
(`vault {action: "ask"}`, `docs/vault.md`, "Asking the user for an entry"), an event of that
agent. `vault.asked` opens the request, numbered by its `seq`: `agent`, `project`, `name`, `why`
(the agent's reason), and what Flottille prefilled, `description` and `takes_username`, and
`taken` when another project already uses the name (the sentence the request is refused with).
`vault.ask_joined` is another agent's ask while it is open: `agent`, `ask` (the request's
number), `project`, `name`, `why`. `vault.ask_answered`, an event of the agent that opened it,
closes it: `ask`, `project`, `name`, `entry` (the entry set, which is not `name` when the user
chose another), `outcome` (`set` or `dismissed`). Never a secret.

### `workspace.created`

A workspace was written, before anything is made on disk (`docs/workspaces.md`). Every event of a
workspace has `project` and `workspace` (its name).

| Field | |
|---|---|
| `project` | The project's name. |
| `workspace` | The workspace's name. |
| `branch` | Its branch. |
| `folder` | Its folder. |
| `repositories` | Each `{"path", "base"}`, as in a workspace (`docs/protocol.md`). |

### `workspace.step_started`

A step of the preparation started.

| Field | |
|---|---|
| `project`, `workspace` | |
| `position` | The step's place, from 0. |
| `kind` | `worktree`, `copy`, `link`, `root_entries` or `run`. |
| `repository` | Where it is done, `null` for the workspace folder. |
| `doing` | A command-like description of it. |

### `workspace.step_done`, `workspace.step_skipped`, `workspace.step_failed`

The step ended: done, skipped (its repository is not in the workspace), or failed. The same fields
as `workspace.step_started`, and, for a failure, `output`: its last lines, masked. A failure stops
the preparation.

### `workspace.ready`

Every step is done or skipped.

| Field | |
|---|---|
| `project`, `workspace` | |

### `workspace.resumed`

Steps were put back to pending to be done again (`workspace prepare`, or the daemon starting while
a workspace was preparing).

| Field | |
|---|---|
| `project`, `workspace` | |
| `redone` | Done steps whose result is gone from the disk. |
| `retried` | Failed or running steps. |

### `workspace.forge`

What the forges say of the workspace changed: a pull request, its state or its checks, or what
stops them being read ([`forges.md`](forges.md)). Its data is the whole state, as
`workspace.forge` answers it (`docs/protocol.md`), `project` and `workspace` included. A first read
that finds no forge says nothing.

### `workspace.ticket`

The workspace was linked to a ticket, or unlinked ([`tickets.md`](tickets.md)).

| Field | |
|---|---|
| `project`, `workspace` | |
| `ticket` | `#123` or `ABC-123`; `null` when unlinked. |

### `workspace.changed`

Every worktree made of the workspace is on one branch other than the recorded one (a `git switch`
in it, or `workspace.rename_branch`), and the record took it ([`workspaces.md`](workspaces.md),
"The branch").

| Field | |
|---|---|
| `project`, `workspace` | |
| `branch` | The branch it is on now. |
| `was` | The branch recorded before. |

### `workspace.removed`

The workspace's worktrees, folder and record are gone. Its local branches whose work landed are
deleted after it, which the answer to `workspace.remove` says, not an event.

| Field | |
|---|---|
| `project`, `workspace` | |

### The events of an item

What a round of the items found changed on a pull request or a ticket ([`items.md`](items.md)).
Every one has the item's identity and what it is now; a source's first round emits none.

| Field | |
|---|---|
| `project`, `kind` (`pull`, `ticket`), `source` (`github`, `bitbucket`, `jira`), `repository` (empty for Jira), `key` | |
| `title`, `state`, `url` | As read now. |

#### `item.opened`

An item not stored before.

#### `item.changed`

| Field | |
|---|---|
| `fields` | The name of each field that changed: those of `changes`, and `description`, `discussion` (a comment edited or deleted: a new comment or review has its own event) and `checks` (one of them moved while others still run: their end has its own event), whose texts it does not carry. A description or a discussion never read before is no change. |
| `changes` | Each short field that changed, `{"from", "to"}`: `state` (a reopening; a merge and a close have their own events), `title`, `assignees`, `labels`, `status` (Jira's). `{}` when none did. |

#### `item.commented`, `item.reviewed`

More comments, or more reviews or another review decision (Bitbucket: a reviewer's verdict),
than stored.

| Field | |
|---|---|
| `comments` (`item.commented`), `reviews` (`item.reviewed`) | The newest, five at most: `{author, bot, by_agent, state, excerpt}`, `by_agent` when an agent of Flottille posted it (`forges.md`, "What Flottille posts is recorded"). |
| `review`, `reviewers` (`item.reviewed`) | The decision now; Bitbucket's reviewers and their verdicts. |
| `verdicts` (`item.reviewed`, Bitbucket) | The reviewers whose verdict is new, `{name, state}`, none `pending`: what a subscription to `review` is told. |

#### `item.approved`

A pull request's review decision became approved, from any other state (`item.reviewed` comes
with it): GitHub's `reviewDecision` turned `APPROVED`; on Bitbucket every reviewer approved and
none asks for changes (a pull request with no reviewer never). Judged against the decision stored
before the round: one read approved again, or a decision never read before, is no event, nor is a
pull request merged or closed by the same round.

| Field | |
|---|---|
| `review` | `approved`. |
| `by` | Who approved: Bitbucket's reviewers who approved; GitHub's authors whose last review that counts (not a comment) is an approval. |

#### `item.checks`

A pull request's checks ended: none runs now, and one was running, or its head commit moved.

| Field | |
|---|---|
| `checks` | `passing` or `failing`, as one mark. |
| `ended` | The checks that ended, `{name, state}`: those that were running, or all of them after a push. |

#### `item.merged`, `item.closed`

The pull request was merged; the pull request, the issue or the work item (its status in the
done category) was closed.

#### `items.forgotten`

The items of a source went from the store: the project no longer declares it (a repository's
forge or the tracker unset, a repository removed), or the project was removed. One per project
and source, at the sweep that forgets them.

| Field | |
|---|---|
| `project`, `kind` (`pull`, `ticket`), `source` (`github`, `bitbucket`, `jira`), `repository` (empty for Jira) | The source. |
| `items` | How many went. |

### The events of an agent

Every event of an agent has `agent`, its id (`docs/agents.md`), and is part of its conversation
(`agent.show`). The events made from what the agent sends also have `replay`, always false: what a
`session/load` replays is not stored again (`docs/agents.md`, "The conversation").

#### `agent.created`

An agent was written, before its process starts.

| Field | |
|---|---|
| `agent` | Its id. |
| `project` | Its project. |
| `workspace` | Its workspace, `null` for the project folder. |
| `folder` | Where it works. |
| `provider` | `claude`. |
| `model` | The model asked, `null` for the agent's default. |
| `role` | `lead` or `worker`. |
| `parent` | The agent that launched it, `null` for a root. |
| `depth` | 0 for a root, its parent's plus one for a child. |
| `title` | Its task's title, `null` for a root. |

#### `agent.moved`

The agent removed its own workspace and works in the project folder (`main`) now: the same
session, written with `workspace.removed` in one transaction, just after it
([`tools.md`](tools.md), "The workspace").

| Field | |
|---|---|
| `agent`, `project` | |
| `from` | The workspace it worked in. |
| `to` | `null`: the project folder. |
| `folder` | Where it works now. |

#### `agent.started`

Its process runs and its session is open, its options set.

| Field | |
|---|---|
| `agent` | |
| `pid` | The agent's process id (the Claude bridge's). |
| `opened` | `new`, `resumed` or `loaded`. |
| `native_id` | The ACP session id. |
| `agent_version` | The agent's name and version, as its `initialize` gives them (`flottille-claude-bridge` and Flottille's version). |
| `taken` | `{"model", "effort", "mode"}`: what the agent took. |

#### `agent.prompted`

A turn starts with this message.

| Field | |
|---|---|
| `agent` | |
| `text` | The message. |
| `images` | The names of the images that went with it, when some did (`agent.image` gives each back). An image that did not go is said in `text` instead. |
| `deliveries` | The ids of the deliveries the message is made of, in order: one per delivery of `text` (the id `agent.send` answered for a message of the user). An event written before it was kept has none. A delivery that only gives a new session the conversation again after a compaction (its brief, the summary, the latest messages, or the handoff) has an id ending in `-replay` (`brief-<agent>-<epoch>-replay`): it is part of the compaction, not something anyone said, and a reader that shows what was sent leaves its section of `text` out. |
| `typed` | When a delivery of the message used commands (`docs/commands.md`): one entry per delivery of `text`, in order, `{"text", "commands"}` for one that used commands (what was typed, masked, and the names of the commands in the order of their first use), `null` for the others. Left out when none did; an event written before it was kept has none. |

#### `agent.user_message`, `agent.message`, `agent.thought`

A piece of text of the user (seen in a replay), of the agent's reply, or of its thinking, as it
streams: the pieces of one reply follow each other. The pieces of a reply are the daemon's, not
the provider's: it masks the reply whole and keeps what is settled, at most 2,000 characters a
piece (`docs/agents.md` "The conversation").

| Field | |
|---|---|
| `agent`, `replay` | |
| `text` | The piece, cut at 64 KiB (`‹+N chars›` says how much was left out). |
| `message_id` | Of `agent.message`, when the source gives one (ACP's `messageId`; Codex's item id; the API message's id for Claude): the pieces that share it are one message, another id starts another. |

#### `agent.tool_call`

A tool call the agent makes, or a change of one (`update`).

| Field | |
|---|---|
| `agent`, `replay` | |
| `id` | The call's id. |
| `update` | False when the call appears, true for a change of it. |
| `title`, `kind`, `status`, `locations`, `content`, `raw_input`, `raw_output` | As the agent sent them, each only when it did; `status` is `pending`, `in_progress`, `completed` or `failed`. |

#### `agent.compacting`

Flottille's compaction of the agent began (`docs/design.md` §5.8): the compactor was started, or
the agent was asked for its handoff. The window shows a step that runs from it, until
`agent.compacted` or `agent.compaction_failed` ends it. The agent's CLI compacting on its own
says nothing before: only `agent.compacted`.

| Field | |
|---|---|
| `agent` | |
| `source` | `compactor`, or `agent` when it writes its own handoff (`summary = "agent"`). |
| `before` | The tokens of the context in use, as the last `agent.usage` said. |

#### `agent.compacted`

The agent's context was compacted: by its own CLI, or by Flottille (`docs/design.md` §5.8), whose
compactor summarised it before the agent went on in a new session.

| Field | |
|---|---|
| `agent`, `replay` | |
| `by` | `flottille` when Flottille compacted it; absent for the CLI's own compaction. |
| `before` | Flottille's only: the tokens of the context in use before, as the last `agent.usage` said. |
| `after` | Flottille's only: the tokens of what the new session starts with (the brief, the summary and the messages kept), counted as 4 characters a token. |
| `summary` | Flottille's only: the summary the compactor wrote, or the handoff the agent wrote; `null` when the agent's handoff is missing. |
| `source` | Flottille's only: `compactor`, or `agent` when the agent wrote its own handoff (`summary = "agent"`). |
| `missing` | `true` when the agent was asked for its handoff and it failed: the new session started from Flottille's state and the user's last message. Absent otherwise. |

#### `agent.cache_expired`

The prompt cache of the agent, at rest, expired, and its context is past the minimum size of the
compaction around the expiry (`docs/design.md` §5.8, 6): the composer's toolbar's **Compact**
control is enabled until the agent's next turn or compaction. Said once per expiry.

| Field | |
|---|---|
| `agent` | |
| `compact` | The setting `after_expiry` when it was said: whether the chip starts ticked. |

#### `agent.quota`

A root's wait for its quota's reset began, changed or ended (`docs/design.md` §5.9).

| Field | |
|---|---|
| `agent` | |
| `limited` | `true` while it waits; `false` once the wait ended. |
| `reset_at` | While it waits: the reset, in milliseconds since the epoch; `null` when the provider's words could not be read. |
| `resume` | While it waits: the user's choice for this session, `null` to follow the setting. |
| `limited_at` | While it waits: when the provider said it. |
| `why` | Once it ended: `the user wrote`, `a turn ran`, `resumed` or `the quota reset`. |

#### `agent.compaction_failed`

Flottille's compaction of the agent failed, and changed nothing: the agent goes on in its
session, and its CLI's own compaction stays. A failed handoff of an agent that writes its own
(`source` `agent`) is said the same way, and its new session starts all the same, without it
(`agent.compacted` with `missing`).

| Field | |
|---|---|
| `agent` | |
| `used` | The tokens of the context in use when it started, as the last `agent.usage` said: Flottille compacts by itself again only past this size plus a tenth of the window. |
| `reason` | Why: the compactor failed or was silent, its summary was cut off by its length, tried to call a tool, or was not smaller than what it replaces. |

#### `agent.plan`, `agent.usage`, `agent.mode_changed`, `agent.options_changed`

The agent's plan (`entries`), its context use (`used` and `size` in tokens, `cost`), its mode
(`mode`), its options (`options`, as ACP gives them: the agent's own announcement; a change of
model or effort the user asked is `agent.options_taken`). Each with `agent` and `replay`.

`agent.usage` comes during a turn, after an answer of the agent and at most once a second, and at
its end. Its `used` is always a number; `size` is `null` while no one knows the context window yet
(Claude Code gives it with the end of a turn only: before the session's first `result` the update
carries `used` alone; the bridge's `usage_update` says a `size` of 0, which the event turns into `null`). The daemon fills in the last `size` the agent reported when its process
says none, so a process started again after a release keeps the window, as long as the model is
the same: `agent.usage` carries the `model` the agent's process runs, and a window reported for
another model is not carried over.

#### `agent.provider_wait`, `agent.provider_limit`, `agent.provider_error`

The provider makes the agent wait, refuses it for a limit, or fails its turn for another reason
once its retries are spent (`message`, when it says one). Each with `agent` and `replay`. A limit
or an error ends the turn (`agent.turn_ended`): the agent lives on and takes the next message.

#### `agent.permission_answered`

The agent asked its own permission for a tool, answered at once.

| Field | |
|---|---|
| `agent`, `replay` | |
| `tool` | The tool's name. |
| `allowed` | True for one of the agent's Flottille tools, false for anything else. |

#### `agent.tool_served`

Flottille's MCP server answered one of the agent's tool calls (`docs/tools.md`). A sign of
activity.

| Field | |
|---|---|
| `agent` | |
| `tool` | The tool's name, without the `mcp__flottille__` prefix. |
| `call_key` | The call's key (`claudecode/toolUseId`), `null` when the call had none. |
| `arguments` | The arguments, every text cut at 500 characters and masked. |
| `gate` | The gate's verdict on the call: `allow` (harmless, or allowed by the user), `judged` (to judge, allowed by the judge in Auto), `full` (to judge or base, allowed by Full mode), `deny` or `ask`; `null` for a call refused before it, because the agent has no such tool. |
| `judgement` | The judge's verdict when one was asked, `{"by", "verdict", "reason"}` (`docs/protocol.md`, `approval.list`); else `null`. |
| `outcome` | `ok`, `refused`, `failed`, or `held` for a call the gate held (`docs/tools.md`, "The gate"). |
| `input_refused` | Only on a `refused` call whose input broke the tool's own rule before anything ran (arguments that do not fit its schema, a detail it has none of, a session title of two words): `true`. Said by the daemon, never guessed from the text; a refusal of the gate, of the user or of the state of things (a workspace that is gone, a title the user named) carries none. The window counts such a call as retried, not failed, when a call of the same tool passes later in the same turn. |
| `duration_ms` | How long it took. |
| `request` | For a call the user allowed, performed once they did: the request's number; else `null`. |
| `link` | Only for a `forge` `run` that was done and whose answer gave the run's link: that link. |
| `cut` | Only when the result was cut at the source (`docs/tools.md`, "Long results"): `{"result", "chars", "left_out"}`, the id the whole text is read by, its length and how many characters the agent was not shown. |
| `setup` | Only on a `tickets` `log_time` that was refused for want of the project's vault entry (none named, one the project does not see, or one with no username): `time_logging`. The window says it in the conversation, with a button to the project's Settings, Tracker (`docs/tickets.md`, "Logging time"). |

#### `agent.link_shown`

The agent handed the user a link (`web` `show_link`, `docs/tools.md`, "Showing a link"). Nothing
was opened: the window turns it into a toast with an Open button and a line of a card in the
conversation (links shown in a row are one card and one toast, `docs/tools.md`, "Showing a
link"), and the user's click opens it with the system's default handler. The event carries
nothing to tell a row apart: the window makes the rows from the events in order.

| Field | |
|---|---|
| `agent` | |
| `kind` | `url`, or `file` for a file of the agent's place. |
| `target` | What Open opens: the `http(s)` URL as the agent gave it, or the file's absolute real path. |
| `title` | The few words the agent gave, masked; `null` when it gave none. |
| `host` | The URL's host; `null` for a file. |
| `shown` | Only for a file: its path as the place shows it, relative to the place. |

#### `agent.tools_changed`

The agent was given the tools its role has now: its project declared or dropped a forge or a
tracker, or its process starts on a version whose tools differ from those it was made with
(`docs/tools.md`, "When the tools change").

| Field | |
|---|---|
| `agent` | |
| `added`, `removed` | The tools' names, without their prefix, sorted. |

#### `agent.tools_reload`

A reload of the agent's tools waits for its next process (`pending: true`, `agent.reload_tools`),
or no longer (`pending: false`): the next process took it and was given the tools its role has
now, or the agent ended first. Like `agent.tools_outdated`, it is no sign of activity.

| Field | |
|---|---|
| `agent` | |
| `pending` | |

#### `agent.tools_outdated`

The agent's running session runs an older list of tools than its role has (`outdated: true`):
told the list changed, it called a tool without listing them again while it held another list,
or its agent cannot take new tools while it runs and its list is not the one its process started
with. `outdated: false` once it listed them, once its list is back to the one it holds, or once
its process ended.

| Field | |
|---|---|
| `agent` | |
| `outdated` | |

#### `approval.requested`

The gate held one of the agent's calls: a request was written (`docs/tools.md`, "The gate").

| Field | |
|---|---|
| `number` | The request's number, counted across every agent. |
| `agent` | The agent that asked. |
| `call_key` | The call's key, `null` when it had none. |
| `tool` | |
| `call` | The arguments, every text cut at 500 characters and masked. |
| `reasons` | Why it was held, one sentence each, masked. |
| `class` | `judge` or `base` (`docs/tools.md`, "The gate"). |
| `kind` | `call`, or `action` (a change of the project an agent asked for) (`docs/protocol.md`). |
| `says` | What it does, in words, one sentence each; empty for a `call`. |
| `judgement` | The judge's verdict when one was asked in Auto (it asked, or failed); else `null`. |
| `held_by` | `user`, or the id of the agent that holds it. |
| `forwards`, `state`, `decision`, `created_at`, `decided_at` | As in `approval.list` (`docs/protocol.md`). |

#### `approval.forwarded`

The agent that held the request passed it up, with one sentence of why.

| Field | |
|---|---|
| `number` | |
| `agent` | The agent that asked. |
| `by` | The agent that forwarded it. |
| `why` | Its sentence. |
| `held_by` | Who holds it now: `user`, or the agent's parent. |

#### `approval.cancelled`

The request was never answered: the agent that asked, or the one that held it, ended, or the
agent that asked removed its own workspace and works in `main` now (written with
`agent.moved`). It is no longer pending, so the user never sees it.

| Field | |
|---|---|
| `number` | |
| `agent` | The agent that asked. |
| `held_by` | Who held it: `user`, or an agent's id. |
| `why` | Which agent ended, and how; or which agent moved, from which workspace. |

#### `approval.decided`

The user answered the request, or the agent that held it denied it.

| Field | |
|---|---|
| `number` | |
| `agent` | The agent that asked. |
| `decision` | `deny` or `once`. |
| `by` | `user`, or the id of the agent that denied it (`approval_answer`). |

#### `approval.settled`

The request's answer was stored as the agent's `approval` delivery: at once for a denial, once
it was performed for an allowed request.

| Field | |
|---|---|
| `number` | |
| `agent` | |
| `outcome` | `done`, `failed`, `not_executed` (refused, or the situation changed), `refused` (denied by the user), or `interrupted` (a daemon stopped while it performed it). |
| `delivery` | The delivery's id, `approval-<number>`. |

#### `agent.turn_ended`

| Field | |
|---|---|
| `agent` | |
| `stop_reason` | `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`, `interrupted` when its process died during the turn, or `failed` when the agent failed the prompt for a reason that leaves its session usable (`docs/agents.md`, "A failed turn"). |
| `usage` | The turn's token counts when the agent gave them, else `null`. |

#### `agent.released`

Its process was let go; the next message starts it again.

| Field | |
|---|---|
| `agent` | |
| `reason` | `idle` (no turn for the idle release time), `restart` (the daemon that ran it is gone), `reload` (the user reloaded its tools, `agent.reload_tools`), `failed` (its turn failed for a reason that leaves its session usable: its `state_reason` says it until the next prompt, `docs/agents.md`, "A failed turn") or `stopped` (an ended or failed root whose session is kept: it can be resumed now, `agent.resume`). |

#### `agent.died`

Its process ended by itself. Its session is replaced (`agent.replaced`), or the agent fails when
it has been replaced too often (`docs/agents.md`, "Replacement").

| Field | |
|---|---|
| `agent` | |
| `code` | Its exit code, `null` when a signal ended it. |
| `signal` | The signal, `null` otherwise. |
| `during_turn` | Whether a turn was running. |

#### `agent.failed`

The agent cannot go on: its process could not start, its session could not open, an option it was
asked is not one it offers, it was replaced too often, its new workspace could not be prepared, or
its parent failed. Its process is stopped, and so is its tree; what was queued for it is
superseded. A root's failure is for the user to look at: `flottille inbox` lists it.

| Field | |
|---|---|
| `agent` | |
| `reason` | A sentence, masked. |

#### `agent.ended`

The agent was stopped (by the user, its parent, or its parent's end), or it was a child that
reported, or it was a root left alone for the days the user set (`inactive for 7 days`,
`docs/design.md` §5.7). Its process and its runs are stopped; what was queued for it is
superseded.

| Field | |
|---|---|
| `agent` | |
| `reason` | A sentence: `stopped by the user`, `its parent stopped`, `it reported`, `inactive for N days`… |

#### `agent.gate_mode_changed`

The user put the agent's session in Ask, Auto or Full (`agent.mode`, `docs/protocol.md`).

| Field | |
|---|---|
| `agent` | |
| `mode` | `ask`, `auto` or `full`. |

#### `agent.options_pending`

The user (or a parent, for its child) changed the agent's model or effort (`agent.options`,
`docs/protocol.md`); the change waits to be set on its session between turns.

| Field | |
|---|---|
| `agent` | |
| `model`, `effort` | What is waiting, each `null` when it is not changed. |

#### `agent.options_taken`

The agent's session set the change, or what it took instead: from now it runs on `model` and
`effort` (what it reports it has, a provider's `default` named by the model it stands for), and
the agent's stored options follow (`docs/agents.md`, "The options").

| Field | |
|---|---|
| `agent` | |
| `model`, `effort` | What it runs on now. |

#### `agent.options_refused`

The agent's session refused the model or the effort of a change: it goes on with its previous
options, the value refused leaves the change, and the agent is not failed. An error that is not a
refusal is none: the change stays pending.

| Field | |
|---|---|
| `agent` | |
| `reason` | A sentence naming what the agent offers. |

#### `agent.resumed`

The user resumed an ended root (`agent.resume`, `docs/protocol.md`): it is idle again, on its own
session, which its next process resumes or loads. Its children stay ended.

| Field | |
|---|---|
| `agent` | |

#### `agent.reported`

The agent called `report` (`docs/tools.md`). A root's report is for the user: `flottille inbox`
lists it (`docs/cli.md`), and a pilot sees it with `events_since`.

| Field | |
|---|---|
| `agent` | |
| `text` | Its report, masked. |

#### `agent.asked`

The agent asked a question with `ask` (or `ask_user`, `ask_parent`, its old names): a child of its
parent, a led session of its pilot, any other root of the user. A question to the user:
`flottille inbox` lists it, and a pilot sees it with `events_since`. A question to a led
session's pilot is shown in the session, and its pilot is woken at the turn's end. When the pilot
no longer reads the session (it stops following it, it ends, or the user detaches the session),
each question still open to it is written again, to the user, with `moved_from`: the pilot's are
closed by it.

| Field | |
|---|---|
| `agent` | |
| `question` | The ask as one text, masked: a single question without options is its own text; several are numbered, each option listed under its question (what a parent or a pilot reads). |
| `questions` | Each question, masked: `{"question", "options"?: [{"label", "description"?}], "multiple"?}`. An event written before an ask could hold several has none: read it as the single question `question`, without options. |
| `to` | `user`, `parent` or `pilot`. |
| `pilot` | With `to` `pilot`: the pilot's id; `null` otherwise. |
| `moved_from` | Present on a question to the pilot moved to the user: the pilot's id. |

#### `agent.titled`

A session was named (`docs/tools.md`, "The session's title"): by its agent (`session_title`), or by
the user (`agent.rename`), whose title then stays. The window's sidebar, header, quick switcher and
notifications follow it.

| Field | |
|---|---|
| `agent` | The session's root. |
| `title` | Masked. |
| `by` | `agent` or `user`. |

#### `agent.detached`

The user detached a session from the agent that started it (`agent.detach`): `launched_by` is
`null` from then on and `detached_from` keeps that agent. There is no undo.

| Field | |
|---|---|
| `agent` | The session. |
| `from` | The agent it was detached from. |

#### `agent.handed`

A led session's pilot ended: the session goes on, handed to the user (`led_by` is `null` from then
on), and it is told so once by a `handed` delivery.

| Field | |
|---|---|
| `agent` | The session. |
| `from` | Its pilot, which ended. |

#### `agent.answered`

A question of the agent may no longer wait: it took a message of the user (a root) or of its
parent (a child) while a question of its waited, or its session was replaced or resumed while one
did, or the user answered an ask from its card (`agent.answer`). It tells a client to read the
root's open questions again (`agent.tree`, `questions`); it does not say which are open, the store
does: an ask is answered once the root took a message of the user written after it (an answer from
an ask's card answers that ask alone), and none asked before a replacement or a resume waits.

| Field | |
|---|---|
| `agent` | |
| `why` | `replaced` or `resumed`, when that is why; absent otherwise. |
| `asked` | With `agent.answer`: the number of the `agent.asked` event it answers. |
| `answers` | With `agent.answer`: one per question, in order, `{"choices"?, "text"?}`, the words masked. The window's card shows them, folded, once the agent took them. |

#### `agent.stuck`

The sweep found the agent stuck in a turn (`docs/agents.md`, "The sweep"): its session is
replaced.

| Field | |
|---|---|
| `agent` | |
| `quiet_ms` | How long nothing showed. |

#### `agent.replaced`

A new session takes the agent's work on (`docs/agents.md`, "Replacement").

| Field | |
|---|---|
| `agent` | |
| `epoch` | The new session's number, counted from 0 for the first. |
| `reason` | Why, a sentence. |
| `counted` | Whether it counts against the limit of replacements. |

#### `delivery.queued`

A delivery was stored for the agent (`docs/agents.md`, "Deliveries"). One already stored under
the same id is not stored again, and has no event.

| Field | |
|---|---|
| `agent` | |
| `delivery` | Its id. |
| `kind` | `user`, `brief`, `launched`, `parent`, `child-report`… |
| `urgency` | `between-turns`, `urgent` or `redirect`. |

#### `delivery.sent`, `delivery.given_back`, `delivery.superseded`, `delivery.withdrawn`

Deliveries were sent (`how`: `prompt`, or `note` on a tool result), given back to the queue
(their turn failed), superseded (their agent ended first), or withdrawn (the user took back a
message they wrote to the root, before the root read it: `agent.unqueue`).

| Field | |
|---|---|
| `agent` | |
| `deliveries` | Their ids. |
| `how` | For `delivery.sent` only. |

### The events of a run

A command an agent ran with `run` (`docs/tools.md`), or a task the user ran (`task.run`). Every
event of a run has `agent`, the agent that started it (`null` for the user), `project`, and `run`,
its id.

#### `run.started`

| Field | |
|---|---|
| `agent`, `run` | |
| `name` | The task, `null` for a line of the agent's. |
| `line` | The line run, masked of secrets like a call's arguments. |
| `folder` | Where it runs. |
| `ready` | `starting` for a task that says when it is ready, else `null`. |
| `url`, `url_error` | Its address, or why it has none (`docs/projects.md`, "Tasks"). |

#### `run.ready`

A run of a task that says when it is ready is: its probe passed. Once per run, never after it
ended.

| Field | |
|---|---|
| `agent`, `run` | |
| `url` | Its address, `null` without one. |

#### `run.ended`

| Field | |
|---|---|
| `agent`, `run` | |
| `state` | `exited` (by itself), `stopped` (`run_status` with `action: stop`, `run.stop`, or the call that started it stopped), `interrupted` (the daemon stopped, or the one that ran it is gone). |
| `exit_code` | Its exit code, `null` when it did not exit by itself or a signal ended it. |
| `signal` | The signal that ended it, `null` otherwise. |

A restart (`run.restart`) is a `run.ended` with state `stopped` for the first run, if it ran, then a
`run.started` for the new one, of the same `agent`, `project` and `name`.

#### `run.agent_folders_unchecked`

An agent's run ended and the daemon could not check all the agent folders of its place after it
(`docs/design.md` §7.1). `flottille inbox` lists it as an entry of kind `unchecked`
(`docs/protocol.md`, `inbox.list`), for a child's run too.

| Field | |
|---|---|
| `agent`, `project`, `run` | |
| `folder` | Where it ran. |
| `too_large` | The place is too large: the agent folders right in it and in its repositories were checked, one further down past 500,000 entries was not. Absent from an event of before it, which was one. |
| `interrupted` | The daemon stopped or crashed while the place was guarded: found at its next start, nothing there was checked nor put back. `agent` and `project` are the run's, `null` when it is not known. |
| `missing` | The daemon could not note all the agent folders before the run: nothing it changed there is known, nor put back. A run whose own snapshot failed is refused instead; this is said of runs already going on inside a place whose snapshot failed. |
| `unreached` | The folders of the place's repositories that are outside it (reached through a link), whose agent folders were not checked. |

#### `run.agent_folders_put_back`

The put-back of a run's place (`docs/design.md` §7.1) put back, or could not put back, what was
written in its agent folders. The run named is the last of the place's runs to end, which may not
be the one that wrote, nor one that started: the result of that run says it too, when it ran.
`flottille inbox` lists it as an entry of kind `put_back`, for a child's run too.

| Field | |
|---|---|
| `agent`, `project`, `run` | |
| `folder` | Where it ran. |
| `put_back` | The entries put back as they were, relative to the place. |
| `left` | The entries it could not put back. |
| `kept` | Where what it removed or replaced is kept, `null` when nothing was. |

#### `run.dismissed`

The user dismissed a command that ended (`run.dismiss`): it is no longer listed.

| Field | |
|---|---|
| `agent`, `run` | |

### The events of a leave

The user's leave for a session to write to another it did not start (`docs/tools.md`, "Sessions
that talk to each other"). Each has `writer` and `session`, the two agents' ids, `request`, the
number of the request it was given in, and `granted_at`. A leave also goes, with no event of its
own, when either session ends: `agent.ended` (or `agent.failed`) says it.

#### `leave.granted`

The user allowed the first message of `writer` to `session`: the writer's next messages to it
are judged as a write is, never asked again for the pair.

#### `leave.withdrawn`

The user took it back (`leave.withdraw`): the writer's next message to that session asks again.

### The events of a service

The services of the machine (`docs/projects.md`, "Services"). Their runs also emit the events of a
run, with an empty `project`.

#### `service.started`

`{"service", "run", "line", "folder", "ready"}`: the daemon started it, for the user, an agent or
a task that needs it. `ready` is `"starting"` for a service that says when it is ready, else
`null`.

#### `service.ready`

`{"service", "run", "url"}`: it is ready.

#### `service.ended`

`{"service", "run", "state", "why"}`: its run ended; `state` is `stopped` (the user, an agent the
user allowed, or the daemon stopping) or `failed`, `why` the line that says why.

#### `service.changed`

`{"service", "change"}`: `added`, `edited` or `removed` in `services.toml` through Flottille, or
`outside`: it answered before it was started, and Flottille started nothing.

### The events of a subscription

A subscription (`docs/design.md` §5.6) opens and ends; each event has `agent`, the agent it wakes,
and `subscription`, its id. What it delivers is a `delivery.queued` of kind `subscription`.

#### `subscription.opened`

An agent subscribed (`forge` `subscribe`), asked to be woken (`wake`), or started a run it
follows (`forge` `run`). Subscribing again to the
same target replaces the agent's subscription, as a new wake replaces the agent's wake: the old one
ends first, `cancelled`.

| Field | |
|---|---|
| `agent`, `subscription` | |
| `source` | What produces its events: `forge`, `tickets`, `timer` for a wake, `session` for the following of a session the agent started for the user, or `run` for the following of a run it started (`docs/forges.md`, "Following a run"). |
| `named` | What it watches in a few words, as the agent reads it (`pull request #12 of api, workspace login`; `wake: <why>`; `run #43 of api (custom: nightly on app/login)`). |
| `events` | The events it waits for. |

#### `session.following_paused`, `session.following_resumed`

The two sessions of a following (`docs/tools.md`, "Sessions that talk to each other") wrote to each
other fifty times with no message of the user's in either: the following pauses, and wakes nobody
until it resumes, and the `session_send` between the two are refused. A message of the user's to
either session, or the window's Resume (`subscription.resume`), resumes it.

| Field | |
|---|---|
| `agent` | The session that is followed, a root: the user's. |
| `follower` | The agent that started it and follows it. |
| `exchanges` | (`paused` only) How many were written: 50. |

`flottille inbox` lists a pause as an entry of kind `paused` (`docs/protocol.md`, `inbox.list`).

#### `subscription.ended`

A subscription is over, once.

| Field | |
|---|---|
| `agent`, `subscription` | |
| `why` | `delivered` (the end of its target, with what it waited for of that read, or a wake's time, or the end of a followed run, was delivered; an event that leaves the target open is delivered and ends nothing), `expired` (the deadline the agent gave passed, said to the agent), `cancelled` (the agent's `unsubscribe`, the user's `subscription.cancel`, or a new subscription to the same target), `agent-ended` (its agent ended), `session-ended` (the session it followed ended or failed, said to the agent), `unavailable` (until #257, when a subscription held a watch of its own: the daemon could not watch its target again after a restart). Only the first two wake the agent. |

