# Usage

What the agents Flottille runs used, and what a task that ends merged costs. Every change meant to
make agents spend less (filtered command output, "unchanged" answers, outlines, learned rules) is
judged by this measure, not by the tokens it removes: a study of 2,848 billed Claude Code runs
("Token Reduction Is Not Cost Reduction", arXiv 2607.12161, July 2026) found that removing 38% of
tool output *raised* the cost by 6.8%, since about 80% of the bill is prompt-cache traffic and an
agent that sees less takes more turns.

It is read from what each turn used, one row per turn and model (`turn_usage`,
`reference/acp.md`, "What a turn used"). Nothing is recorded for it alone.

## One unit: weighted tokens

A turn's cost is

    input + 1.25 × cache_write + 0.1 × cache_read + 5 × output

in **weighted tokens**. The weights are the ratios of Anthropic's prices (a cache write, a cache
read and an output token against an input token); OpenAI's cached input is a tenth of its input
too. They are fixed: they compare providers, and stand for a subscription's quota, without a
table of prices to keep up.

- A count the agent did not give is 0 here, and the row is counted as `missing` a count.
- `reasoning` is not added again: it is already counted in `output`.
- `cost_usd` is kept as the agent says it, and not used here.

The share of the weighted tokens that was cache reads (`0.1 × cache_read` of the whole) is given
with every sum: it is where most of a bill goes.

## One task: a workspace

- A **task** is a workspace, from its creation (`workspace.created`) to its removal
  (`workspace.removed`). A workspace made again under the same name is another task: a turn
  belongs to the last one of that name made before it started.
- Its **cost** is the sum of every turn recorded with that workspace, over its whole tree: the
  root, its children and the sessions it leads.
- It **succeeded** when one of its pull requests was merged, as the forge reads them
  (`forges.md`; the items, `items.md`): a merged pull request of one of its repositories whose
  head is a branch the workspace was on (the one it was made on, and every one
  `workspace.changed` said). It **failed** when it was removed with none merged. Otherwise it is
  **in progress**. The outcome is read when the statistics are asked: a pull request merged after
  the workspace was removed makes it a success.
- A project that declares no forge on any repository has no known outcome: its tasks show their
  cost (`no_forge`) and are left out of the ratio.
- The turns in `main` (no workspace) are in the totals, and in no task (`main`).

**Cost per merged task** = the cost of the finished tasks (merged and failed) ÷ the number of
merged ones. A failed task's cost counts against the ratio: it is what the merged ones took. Tasks
in progress, tasks without a forge and `main` are left out. With no merged task there is no ratio
(`null`).

### The period

The statistics read the turns that started in a period of days, the daemon's local days, the
first and the last included. A task across the edge of the period counts the turns of it that are
in it: the sum of the tasks and `main` is the total.

## What was on: `tuning`

Each row of `turn_usage` says in `tuning` the optimisations on during its turn: their names,
sorted and comma-separated, empty when none was. A turn records an optimisation only when it
could apply to its agent: the switch alone is not enough. The rows recorded before the column
read as empty; they are never rewritten.

| Name | On when |
|---|---|
| `unchanged` | `fs_read` says a page is unchanged (`tools.md`, "Files"; `tool_results.set`), for an agent that can be told it: one of the Claude bridge. |

Splitting by `tuning` gives each optimisation's turns, tasks and cost per merged task over the same
period, on the user's real work, without spending quota on replayed tasks. A task whose turns ran
with different optimisations is counted in each, with the turns of it that ran so.

## Where it shows

- `usage.stats {project?, since, until, by}` (`protocol.md`, "The usage methods").
- `flottille usage [--project P] [--since D] [--by …] [--json]` (`cli.md`).
- The window: Settings, **Usage** (`design.md` §10).
