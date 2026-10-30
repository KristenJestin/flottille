# AGENTS.md — Flottille

Instructions for any coding agent working in this repository. `CLAUDE.md` is a symbolic link
to this file: there is one contract, not two that drift apart.

## Project overview

Flottille runs coding agents for one person on their own machines: a daemon owns projects,
multi-repository workspaces (git worktrees), agent processes driven over ACP in a locked-down
"bare mode", their tree (an agent's sub-agents are real agents tied to a parent, which Flottille
wakes when something happens to them), and the permission gate every tool call goes through.
A command line (`--json`, `watch`), a local MCP server and a GPUI window are its clients.

**Read [`docs/design.md`](docs/design.md) before any work**, then the reference it points to for
the part you touch (`docs/reference/`). Those files are the specification; this file is how to
work.

Flottille is generic: nothing in it is specific to one machine, one desktop, one forge or one
agent. Paths come from XDG, links open with the system's default handler, colours are
Flottille's own unless the desktop provides a theme to follow (`docs/design.md` §10). The window
is English only.

## Commands

```
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
tools/commit-message.sh --range origin/dev..HEAD
git config core.hooksPath .githooks     # once per clone: the commit-msg and pre-commit hooks
```

## Principles

### 1. Think before coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them; don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If the code and the issue disagree, the issue wins; if the issue is wrong, say so instead of
  quietly deviating.

### 2. Simplicity first

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what the task asks. No abstractions for single-use code.
- No configurability that was not requested.
- No error handling for impossible scenarios. If 200 lines could be 50, rewrite.

### 3. Surgical changes

**Touch only what you must. Clean up only your own mess.**

- Don't "improve" adjacent code, comments or formatting. Don't refactor what isn't broken.
- Match existing style. If you notice unrelated dead code, mention it, don't delete it.
- Every changed line must trace directly to the task.

### 4. The failing test first

**Define success criteria. Loop until verified.**

- Write the test that fails first, see it fail, then make it pass.
- "Fix the bug" → a test that reproduces it, then the fix.
- Every scenario of the issue gets a test named after it. A scenario without a test is a
  defect, not a TODO.

## Code style

- Rust 2024, `cargo fmt`, `cargo clippy -- -D warnings` clean.
- Code, identifiers, comments, docs, issues and commits are in English.
- Building blocks: tokio, `rusqlite` (bundled), `serde`/`serde_json`, `clap`,
  `agent-client-protocol` (ACP), `rmcp` (MCP), gpui-kit on GPUI Fast (window). Git is the user's
  own `git` binary, spawned without a shell, never a git library. Adding another dependency of
  weight is said in the pull request.
- A comment says why, not what.

## Motion in the window (non-negotiable)

**Everything that moves in the window is animated.** The maintainer said it once, and a window
pull request is not asked to be told again. The same durations and easing as everywhere
(`crates/flottille-ui/src/motion.rs`, `docs/design.md` §10), through the helpers of
`flottille-ui`, never a second system:

- hover and press of every interactive element (a button, a row, a link, a chip, a toggle): its
  colours fade over `HOVER`. GPUI's own `.hover(..)` switches at once and is never used (a test
  refuses it);
- a state that changes (selected, chosen, focused, open): the fill, the ink, the ring, the
  chevron move over `PANEL` or `HOVER`;
- an element that appears or leaves, a message, an error, a block that is shown or hidden:
  it unfolds and fades in, folds away and fades out, with the gap around it;
- a row inserted into, removed from or moved in a list: it unfolds, folds away or slides;
- a fold or an unfold, a panel or a menu that opens and closes, a page and each of its
  sections that show up (staggered).

A window pull request, and a window issue, without its animations is incomplete: the review
sends it back. The pull request lists each element it animated; the checklist in
`docs/reference/gpui.md` is ticked in it.

Motion is the tenth of the **rules of the window** (`docs/reference/gpui.md`, "Rules of the
window"): four kinds of button, one confirmation dialog, results as toasts, a visible label on
every control, a state said once, one list pattern, the spacing scale. A screen builds them from
the components of `flottille-ui` that hold them, never its own.

## Agents and safety (non-negotiable)

- **Bare mode is the product.** An agent Flottille runs has no built-in tool, no user setting,
  no skill, no plugin and no MCP server but Flottille's. A change that lets an agent run a shell,
  start its own sub-agents or reach anything the gate does not see is a defect, whatever it
  enables.
- **No tool call waits for a human.** A call held for approval is answered at once; the result
  comes later as a delivery.
- **Commands run without a shell**, always: shell syntax is refused, never quoted around.
- **Never read, copy, log or print credentials** of the agents or of the user (agent login files
  are only checked for presence). Secrets found in files or outputs are masked before they are
  stored or shown.
- Flottille runs the agent CLIs the user installed and signed in to. It provides no login of its
  own.

## Testing

- **Tests never run a real agent, never use the real configuration, the real data folder, the
  real home or the user's repositories.** They use a fake ACP agent (a small program speaking
  ACP on stdio with scripted behaviour), temporary folders, throwaway git repositories, and a
  temporary `HOME` and XDG folders.
- A real agent run (it costs the user's quota) happens only when the issue asks for it, once, and
  is reported in the pull request.
- **Never open a window on the user's screens.** The window is tested with GPUI's test platform;
  a real window only in a headless nested compositor (`docs/reference/gpui.md`).
- A test that fails only under load is rerun alone before the failure is called real.

## How the work is organised

| Who | What |
|---|---|
| The maintainer | decides what is built, tries each milestone on their machine, and merges what they have not delegated |
| The lead agent | turns the maintainer's decisions into issues, launches the developing agents, reviews their work, keeps the tracker honest, and never hides a failure |
| Developing agents | implement one issue each: the failing test first, then the code, the verification and a pull request; they never merge |
| CI | `verify`, `commit-messages` and `shape` on every pull request |

### The tracker

- **Issues are the tickets, and a ticket stands alone.** It holds the what, the where, the "how
  to verify" and the base branch, with no local path. If it lacks one of them, ask rather than
  guess.
- The plan is cut into **milestones** (GitHub milestones), beside the labels `type:*` and
  `area:*`. A milestone ends on something the maintainer can use, and they accept it on their
  machine.
- **`dev` is the integration branch** and the default branch: a branch starts from
  `origin/dev` and its pull request targets it. `main` holds released versions.

## Git and pull requests (non-negotiable)

- **Never commit, merge, rebase, push or force-push on `main` or `dev` directly.** If you are
  on one of these branches, create a branch first (`feat/<n>-<topic>`, `fix/<n>-<topic>`). The
  `pre-commit` hook refuses it.
- Never rewrite published history. No `--no-verify`.
- One commit = one intent. No `wip` commits. Don't mix formatting and logic in one commit.
- Commits use the Git user configured on the machine.
- The subject is `<type>(<scope>): <subject>`: the **scope is required**, the subject is 72
  characters at most, starts lowercase, has no final period, and the type is one of feat, fix,
  refactor, test, docs, chore, build, ci, perf. `tools/commit-message.sh` is the judge (the
  `commit-msg` hook and CI run it).
- Scopes: `core`, `store`, `git`, `workspace`, `acp`, `agents`, `mcp`, `tools`, `gate`,
  `sessions`, `daemon`, `cli`, `gui`, `repo`, `ci`.
- **Pull requests are merged by squash only.** The title is a plain Angular subject. The
  description is written in the first person as the maintainer and ends with `Closes #<n>`.

## Security and privacy

- **This repository is public.** Nothing private goes into it, an issue, a pull request or a
  commit: no name of a real employer or client project, no private path, host, account or token.
  Examples and fixtures use invented data.
- Secrets are never printed, logged or committed.
- Nothing is published outside the pull request: no gist, no upload, no paste service.
- **A permission denial is never worked around**: do what remains, say what was refused.

## Working on a machine

- The machine is the maintainer's own, in use while you work. Never stop, restart or
  reconfigure their running programs, agents or sessions; run your own instance of Flottille
  with temporary folders.
- No artificial load: no stress runs, no parallel benchmark loops. Heavy builds run at low
  priority (`nice -n 19`), one at a time.
- Worktrees, logs and every other scratch file live outside the clone.
- Before removing a worktree, check it holds no uncommitted change and no unpushed commit
  (`git log HEAD --not --remotes`).

## GitHub gotchas

- Prefer REST (`gh api repos/…`) to GraphQL, which gets rate-limited.
- A body holding backticks or apostrophes is written to a file and passed with `--body-file`.

## When done

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`, plus the
manual checks the issue asks for, and report the real output. If something fails, say so;
don't claim green.
