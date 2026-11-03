# Reference: handing work between agents, as done by hand today

This is the standard the maintainer's agents follow today to hand work to other agents, with a
separate agent session per task and files as a mailbox. Flottille must make it the default
behaviour of a lead, and replace by features everything below that is done by hand. The
"Flottille" lines say how.

## Why separate sessions and not in-process sub-agents

Measured on real use (October 2026): reading the context back is about 70% of the cost of every
request. An in-process sub-agent lives inside its parent: nobody can open or answer it, its cache
is short-lived, and a long one burns the quota. A session of its own is a main conversation: a
one-hour cache, its own compaction, and the user can open it.

> Flottille: every child is a full agent session the user can open and read. It belongs to its
> parent: only the parent writes to it and answers what it asks.

| Work | Where |
|---|---|
| a read-only search of a few minutes that returns a summary | could stay in-process (Flottille: a child with a short brief is fine too) |
| anything else handed to another agent: code, a pull request, a cleanup, long research | a separate session (a child) |
| one small task the user asks the lead | the lead does it itself: a new session re-reads everything from nothing |

## Cost rules

- A request re-reads the whole context of its session: from the cache at about 10% of the price
  while it is warm, at full price plus the cache write once it is cold. Count a cache as warm for
  20 minutes after the session's last request, to be safe.
- A follow-up goes to the **same** child only when it was active less than 20 minutes ago, its
  conversation is small, and the follow-up is small. Otherwise stop it and launch a new child with
  a brief that names what the previous one left (its report, its branch).
- The **lowest safe model**: the smallest for mechanical work with an exact list (merge a base
  and rerun checks, apply a fix list, rename, summarise files); the middle one by default, for a
  clear spec; the largest for judgment (data shapes, state machines, security, an open design).
- At most **three** children at once, at most two of them running heavy checks; tasks touching
  the same files run one after the other.

> Flottille: `agent_status` gives each child's idle time and context use so the lead can apply
> the follow-up rule; the children cap is enforced; "heavy check" slots come later.

## One child, one task

A task is one precise, reviewable result: a pull request, a cleanup done and checked, a document.
Never two tasks in one child; never a vague one ("look into X and fix what you find").

Today each task gets a run folder (`brief.md` written by the lead, `state.md` kept by the child,
`report.md` written by the child as its very last action, `closed` written by the lead once the
report is acted on), a git worktree beside the repository on a new branch from the right base,
and a background wait that wakes the lead when a report appears or after 55 minutes.

> Flottille: the brief is `agent_launch`'s `brief`; the workspace is created by the launch; the
> report is the `report` tool; the wait is deliveries; "closed" is the child's end. A child keeps
> its own state in its conversation, and Flottille's replacement brief says where it stopped.

## Known failures of the manual way (what Flottille fixes)

- A child that finishes its turn without writing its report looks exactly like one that stopped:
  nobody notices for hours. (Flottille: reminder, then `child-idle` to the parent.)
- A child watching a CI run in the background ends its turn and looks finished. (Flottille: the
  child's running commands are visible in `agent_status`; CI watching belongs to the lead.)
- The lead loses the thread after a compaction or a long pause and no longer waits for its
  children. (Flottille: deliveries wake it; `children-status` when nothing happens.)
- Finished sessions stay listed; worktrees are left behind. (Flottille: a child ends with its
  report; workspace removal is checked and offered.)

## When a report arrives (the lead's duty)

1. Read the report, then the result itself: the whole diff of a pull request, or a sample of the
   records a cleanup wrote. A child reports its mistakes in the same sure words as its
   successes: check the claim that matters most.
2. A task by a small model that turned out to touch something sensitive (data, security, a state
   machine) gets a read-only review by a larger model before it is integrated.
3. Integrate only what the user allowed; merge only when the checks pass on the current base.
4. Stop the child at once, before writing to the user; remove its workspace once nothing in it
   is uncommitted or unpushed.
5. Tell the user in a few lines: what landed, what was checked, what is not verified, what waits
   for them.

## The brief

The child knows only what the brief and the files it names say. Written for a capable reader
with no context:

```
# <task id>: <the result, in one line>

You do <X>, and only <X>.

## Setup (done for you)
- Where you work: <workspace and branch | what you may read>.
- Read first: <the project's agent instructions, the spec (issue), the files that matter>.

## Around you
- Other children running and the files they own: stay out of them.

## What you know that the spec does not say
- The risk of this task; traps hit before; what must be measured or proved; what the user cares
  about here.

## Rules
- <commit identity and format; never merge, deploy or publish>.
- Never paste whole logs into your context: write them to a file, read the failures.
- A choice the spec leaves open: take the simpler, reversible one, write it down with its reason,
  continue. Do not wait for an answer.

## Checks
- <the exact commands>.

## The end
- <the deliverable>. Then call `report`: the result, each point done or skipped and why, what was
  not verified, open questions, follow-up candidates.
```

## The lead's own conduct

- The conversation with the user stays alive and light: every request re-reads all of it.
- Never do a child's work itself, even "quickly".
- Never read whole logs or transcripts: find the failure, read the lines that matter.
- Write what it learns about the user's way of working into the project's notes as it goes.
