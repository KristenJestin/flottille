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

With every sum comes the share of the input tokens that were read from the cache:
`cache_read ÷ (input + cache_read + cache_write)`, the raw counts, as one reads "the share that was
cache reads". Each sum counts **rows**, one per turn and model.

## One task: a workspace

- A **task** is a workspace, from its creation (`workspace.created`) to its removal
  (`workspace.removed`). A workspace made again under the same name is another task: a turn
  belongs to the last one of that name made before it started.
- Its **cost** is the sum of every turn recorded with that workspace, over its whole tree: the
  root, its children and the sessions it leads. A workspace no agent worked in (no turn) is no
  agent's task: it is left out.
- It **succeeded** when one of its own pull requests was merged, as the forge reads them
  (`forges.md`; the items, `items.md`). A pull request is one task's only: of the tasks with its
  repository whose branch was its head (the branch the workspace was made on, and every one
  `workspace.changed` said), the last made before the pull request was opened (before it was seen
  merged, when the forge did not say when it was opened). A workspace whose name, and so whose
  branch, is reused does not inherit the merge of the one before, nor give it its own.
- It **failed** when it was removed with none merged. Otherwise it is **in progress**.
- Flottille keeps every pull request it sees merged (`merged_pulls`), and a workspace's removal
  says whether one of its repositories had a forge then (`forge`): an outcome once known does not
  change when the items are forgotten, a repository removed or a forge taken away. A pull request
  merged after the workspace was removed still makes it a success.
- A task none of whose repositories has a forge declared has no known outcome: it shows its cost
  (`no_forge`) and is left out of the ratio, whatever the other repositories of its project.
- The turns in `main` (no workspace) are in the totals, and in no task (`main`).

What is not read: a branch switched in only some of a workspace's repositories. Flottille records
a new branch (`workspace.changed`) only when every worktree is on it; a pull request from a branch
one repository alone was switched to is no task's, and the task stays in progress, or fails once
removed. Reading each repository's branch would need it recorded at every switch, for a case that
is rare.

**Cost per merged task** = the cost of the finished tasks (merged and failed) ÷ the number of
merged ones. A failed task's cost counts against the ratio: it is what the merged ones took. Tasks
in progress, tasks without a forge and `main` are left out. With no merged task there is no ratio
(`null`).

### The period

The statistics read a period of days, the daemon's local days, the first and the last included;
a client says how many days end today, and the daemon says which (`protocol.md`).

- The **tokens** (the totals, the bars by day, the splits) are those of the turns that started in
  it.
- The **cost per merged task** is that of the tasks that **finished** in it: merged (seen merged)
  or failed (removed) inside it, each with **all** of its turns, whatever their day. A task that
  started before the period brings its whole cost; one still open at its end is not counted yet.
  The outcomes are read as of the period's last day: a task merged after it is not merged in it.
- `in_progress` and `no_forge` are the tasks with a turn in the period and no outcome at its end,
  with the cost of those turns.

The tasks' sums are therefore no partition of the total: a finished task brings turns from
before the period, and a turn after a merge is in no task's outcome.

## What was on: `tuning`

Each row of `turn_usage` says in `tuning` the optimisations on when its turn started: their
names, sorted and comma-separated, empty when none was. A turn records an optimisation only when
it could apply to its agent: the switch alone is not enough. The rows recorded before the column
read as empty; they are never rewritten.

| Name | On when |
|---|---|
| `unchanged` | `fs_read` says a page is unchanged (`tools.md`, "Files"; `tool_results.set`), for an agent that can be told it: one of the Claude bridge. |

Splitting by `tuning` gives the tokens of the turns that ran with each set, and the cost per
merged task of the tasks that ran with it. Each task is in one group: its `tuning` when every turn
of it had the same, else `mixed`. The other splits (`day`, `model`, `provider`, `role`) say tokens
alone: a task spans them, and counting it whole in each would count it several times; the ratio is
the period's.

The empty group (`""`, "Nothing on") holds every turn that ran with none: those of agents that
cannot have one (Codex, any agent but the Claude bridge's) as well as Claude's with every switch
off. The split does not separate providers, in `usage.stats` nor in the window: comparing an
optimisation with its absence is fair only over days when the same provider ran the tasks.

## Where it shows

- `usage.stats {project?, since, until, by}` (`protocol.md`, "The usage methods").
- `flottille usage [--project P] [--since D] [--by …] [--json]` (`cli.md`).
- The window: Settings, **Usage** (`design.md` §10).
