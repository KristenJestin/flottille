# Reference: a real multi-repository project

Flottille must handle this case without any project-specific script. It is a real work project,
described here with invented names. Today it is run by hand with an agent, a set of skills, slash
commands and shell scripts; each of them is listed with what Flottille must provide instead.

## The project

One folder holds six independent git repositories (not a monorepo, not submodules), and is itself
a small repository of its own that ignores them:

| Repository | Role | Base branch |
|---|---|---|
| `app-web` | the front end (Angular) | `develop` |
| `app-api` | the API (NestJS) | `develop` |
| `lib-core` | shared library, front and back | `main` |
| `lib-app` | library specific to the app (translations), uses `lib-core` | `main` |
| `lib-api` | API utilities, uses `lib-core` | `main` |
| `e2e-tests` | the end-to-end harness, observes the app without compiling against it | `main` |

The root folder holds the shared instructions (`CLAUDE.md`), the agent settings and skills
(`.claude/`), and other untracked folders. Each repository has its own `CLAUDE.md` or
`AGENTS.md`.

In development the libraries are linked into their consumers through a local package store
(yalc): a library builds and publishes into the store, a consumer adds it from the store. The
forge is Bitbucket (`bkt` CLI); tickets live in Jira (read through an MCP server, never written
by agents); specifications live in another tool.

## A "feature group", as done today

A script and a skill create one per feature:

1. **Every repository, always.** The set is fixed: all six, with no question. What a ticket says
   it touches is a prediction; a repository missing later costs a worktree, a branch and a
   rebuild while the neighbours' pull requests are already in review. Environment scope is not
   change scope.
2. **Base per repository**: fetch first and stop if the fetch fails (a stale cache once based a
   branch on `main` instead of `develop`), then `develop` if the remote has it, else `main`.
   Branch `feature/<name>` in each, `git worktree add -b feature/<name> <group>/<repo>
   origin/<base>`.
3. **Environment file** at the group's root (direnv `.envrc`): the package store folder isolated
   per group, a `PATH` prefix for a wrapper that forces the store folder on every `yalc` call,
   variables pointing the e2e harness at the group's own `app-web` and `app-api`, a compose
   project name unique per group so that several groups' container stacks never share
   containers, networks or volumes, and an override file that drops host port bindings.
   The variables are computed from the folder of the environment file, never from the current
   directory (an agent `cd`-ing into a sub-folder would otherwise create the store there).
4. **Metadata file** (`.feature.json`): name, repositories, the linked specification, the date.
5. **Editor workspace file** listing the group's folders with a distinct colour.
6. **Registration** in a terminal/command manager, which gives the group its commands
   (`api:dev`, `front:dev`, `generate-data`) with the right working folder, and a terminal
   starting a named agent session.
7. **Setup** (idempotent):
   1. copy `.env`, `.env.local`, `.env.development.local`, `.env.preprod` from each repository
      of the main folder (ignored files; never overwrite);
   2. link every root entry of the main folder that is not a repository into the group, **except
      `.git` and `.gitignore`**: linking those makes git see the group's root as the root
      repository's working copy with every tracked file deleted, and a `git add -A` there empties
      it;
   3. libraries in dependency order (leaves first): add their own local dependencies from the
      store, `npm install`, build, publish into the group's store;
   4. consumers: add their dependencies from the store, `npm install`.
   The dependency graph is read from each `package.json` (names with the organisation's prefix),
   never hard-coded.
8. Some services (authentication relay) are shared and started by the user, not per group.

**What Flottille provides instead**: a project with six repositories, all taken by default; a
base branch rule `[develop, main]` per repository and a fetch that fails loudly; workspace
variables (store folder, compose project name, harness paths, `PATH` prefix) injected into every
command; copy steps with globs; a root-entries link step that excludes `.git` and `.gitignore`;
repository dependencies (declared, or read by a project command) and run steps "for each
repository in dependency order"; the command catalogue; agents started in the workspace. No
script, no environment file, no metadata file.

## Pull requests, merges, deployment, closing (today: slash commands)

- **Open the pull requests**: report per repository (uncommitted, unpushed, commits ahead of the
  base); then, only after the user's explicit go for exactly what was shown, push and open one
  pull request per repository with commits (title `type(TICKET): subject`, fixed reviewers,
  source branch closed on merge).
- **CI against library branches**: a consumer's default CI installs the published library and is
  red until the library is released; a custom pipeline builds against the library branches of the
  same ticket. Launched by hand.
- **Merge the stack**: invoking it authorizes, for that run only, a local rebase, commits, pushes
  including `--force-with-lease`, squash merges and re-runs of failed infrastructure pipelines.
  Check every pull request first (reviewers, comments, CI; blocking unless the user waives it),
  run local checks, then merge by levels: `lib-core`; `lib-api` and `lib-app`; `app-api` and
  `app-web` **together** (their API contract is coupled); `e2e-tests` last after an e2e run in
  CI. Between levels, wait for the libraries' automatic release, bump the consumers, follow the
  pipelines.
- **Deploy to pre-production**: pull requests `develop` → `preprod` on the API and the front,
  merge after an explicit go, follow the deployment, come back to `develop`, refresh the main
  folder, optionally reseed data.
- **Close**: audit each repository (empty branch: delete; branch with commits: confirm its pull
  request is merged; uncommitted or unpushed work: stop), mark the specification done, stop the
  commands, remove the worktrees, delete the local branches, refresh the main folder, remove the
  group's folder last.

**What Flottille provides later** (tranche 3): these as **procedures** of the project, with the
authorization each run grants written in the procedure and shown before it runs, on top of
forge and tracker support (Bitbucket and GitHub, Jira read-only).

## Rules the project's instructions set for agents

- Push and pull request creation only after the user's explicit confirmation of exactly what was
  shown; one confirmation covers one operation.
- Never rewrite published history or merge on their own; never `reset --hard` nor `stash`, even
  inside a procedure.
- Commits off by default; a procedure that says so authorizes them.
- Never write to the tracker or wiki through the shared service account: draft, the user posts.
- Secrets come from a vault reachable only through the company VPN: on a 403 or an unreachable
  vault, stop and tell the user, never work around it.

**In Flottille** these are gate rules (never, ask) and procedure authorizations, not prose an
agent may or may not follow.

## Skills of that project

- Knowledge skills (framework guides, test library, design system, accessibility): become project
  **references**, read by the agent with a tool when relevant.
- Process skills and slash commands (feature group, drive the app in a browser, open pull
  requests, merge, deploy, close): become Flottille features (workspaces, recipes) and
  **procedures**.
