# Releases

Flottille is installed from a built package, in seconds, rather than built with `cargo install`.
Every branch that matters publishes its own build, with a version that says where it comes from.

| Branch | Channel | Version (example) | Published as |
|---|---|---|---|
| `main` | stable | `0.3.0` | a GitHub release, tagged `v0.3.0` |
| `dev` | beta | `0.3.0-beta.12` | a pre-release, tagged `v0.3.0-beta.12` |
| `stable/X.Y` | none: its builds install by `--version` | `0.3.2-rc.1`, then `0.3.2` | a pre-release per push, tagged `v0.3.2-rc.1`; its release, marked latest when above the latest (see "Stable lines") |
| `next/X.Y` | none: its builds install by `--version` | `0.5.0-next.7` | a pre-release per push and per merge of dev, tagged `v0.5.0-next.7` (see "The next version") |
| `feat/*`, `fix/*` | alpha | `0.3.0-alpha.feat-46-packages.4` | a pre-release per push, tagged `v0.3.0-alpha.feat-46-packages.4` like a beta, all deleted with the branch |

Nothing is published anywhere but GitHub's releases of the repository. **Releases are immutable**
(since 2026-11-04): a published version is never replaced, its tag never moved nor its files
changed, so each build is its own release, an alpha's too.

**Each line has a pre-release name of its own**: `-beta.N` is `dev`'s alone, `-rc.N` a stable
line's (a release candidate), `-next.N` the next version's. The beta channel (`install.sh
--channel beta`, `flottille update --channel beta`) reads `-beta.` only, as it always did, so a
line's build never takes it over; and two lines never publish the same version. There is no
channel for `-rc.` and `-next.` (none for now): each is installed by its version
(`--version 0.5.0-next.7`).

## Releasing

Branches reach `dev` by squash. A release does not: **`main` moves forward to a commit of `dev`**
(a fast-forward), and is then that very commit, with no squash and no merge commit of its own.
`main` is always an ancestor of `dev`.

- **The release pull request `dev → main` is opened for review**, and is never merged with the
  button. The `Release` workflow (`.github/workflows/release.yml`) closes it: started by hand
  (`gh workflow run release.yml --ref dev`, or "Run workflow" on its page, from `dev`), with the
  commit of `dev` to release as its input, a full SHA or `dev` for its head (the default). Once
  `main` is the pull request's head, GitHub shows it merged. Releasing an older commit of `dev`
  leaves the pull request open.
- **What it checks**: it runs from `dev` only; the branch is `dev` or a stable line
  `stable/X.Y` (never `next/*`), and the commit a full SHA or that branch's name, nothing else;
  the commit is on the branch named; for `dev`, `main` is its ancestor. Then it moves `main` through GitHub's API,
  not forced: GitHub refuses anything but a fast-forward, so nothing can be overwritten. Any of
  these refused, nothing moves and the run fails, saying why. Its job holds the token that
  writes `main` and runs no code of any branch: no checkout, no git, only `gh`.
- **Then the stable build**: a ref moved with the workflow's own token starts no `push` run, so
  the `Release` workflow starts `ci.yml` on `main` itself (`workflow_dispatch`, which a workflow's
  token may start), which builds and publishes as on any push. Run again when `main` is already
  the commit, it moves nothing and starts the build again, which publishes nothing already
  published.
- **`main` is not verified twice.** The commit released is a commit of `dev`, whose own push run
  verified it already: on `main`, the job `verified-on-dev` asks GitHub's API, with a token that
  can only read, for a run of `ci.yml` on that very commit, pushed to `dev`, that succeeded; a
  run succeeds only when its `verify` passed. Found, `verify` is skipped on `main`. Not found (no
  such run, a failed one, an error of the API), `verify` runs on `main` itself. `publish` runs
  only when `verify` passed in the run, or was skipped because that run on `dev` was found: never
  on a commit no `verify` passed on.
- **A stable line's commit** (`gh workflow run release.yml --ref dev -f branch=stable/0.3 -f
  commit=stable/0.3`, or a full SHA of the line) **is tagged, and `main` does not move**. The
  version is the one of that commit's release candidate, which CI published on its push once its
  `verify` passed (`v0.3.2-rc.1` releases `0.3.2`): a commit without one is refused. The workflow makes
  the annotated tag `v0.3.2` on it, its message naming the line (`stable/0.3`), through GitHub's
  API (a ref made, never one moved: a tag of that name on another commit is refused), then starts
  `ci.yml` on the tag, which builds and publishes it, marked latest when its version is above the
  release marked latest (see "What CI publishes"). Run again when the tag is
  there on that commit, it makes nothing and starts the build again. On its tag, `verified-on-dev`
  asks for the successful push run of that commit on its line.
- **The tag `vX.Y.Z` is put on the released commit**, the same commit on `main` and on `dev`. No
  tag is pushed by hand.
- **The stable version is the one dev already had**: the version of the beta of that commit,
  without its pre-release (`0.3.0-beta.12` releases `0.3.0`). **Its changelog** lists the commits
  of `dev` between the previous stable tag and it.
- A push to `main` that is not a commit of `dev` (none should happen) publishes nothing and
  fails its run, saying why. A run on a commit already released publishes nothing. So does a run
  on a tag `vX.Y.Z` whose commit is not on `stable/X.Y`, or has no release candidate of
  `X.Y.Z` (`-rc.N`).

**The two first releases were made by squash** (0.1.0 and 0.1.1): each squash on `main` was
tagged `vX.Y.Z`, the head of `dev` it squashed was marked `released/vX.Y.Z`, and a merge of the
squash that changed no file was recorded on `dev` (`chore(repo): record the X.Y.Z release in
dev's history`), so that `main` is in `dev`'s history. Those marks and records stay and are still
read (below); no new release makes either.

## The version

Computed, never typed (`tools/version.sh stable|beta|alpha [--branch <name>] [--at <commit>]`,
for HEAD and its branch by default):

- **It counts from the last release**: the highest stable tag `vX.Y.Z`, compared as versions,
  wherever it is, but the releases of the stable lines (below), and from its own commit on, the commit of `dev` it released (a release made by
  squash: from its mark `released/vX.Y.Z`). The commits counted are those of the build's own
  history that the release does not hold, so a feature branch started before a release counts
  only its own commits after it, and the next version is the one `dev` gets.
  Pre-release tags (`v0.3.0-beta.12`) do not count. Without a stable tag, from `0.0.0` and the
  first commit.
- **The next version** follows the Angular commits counted (`tools/commit-message.sh` is the rule
  they follow), merges left out: `feat` bumps the minor, `fix` and `perf` the patch, a `!` after
  the type or a `BREAKING CHANGE:` footer the major. Nothing that bumps is the next patch.
  **Before 1.0, everything bumps one level less**: a breaking change bumps the minor and a `feat`
  the patch, so the 0.1 line takes the everyday features (0.1.2, 0.1.3…) and the minor moves
  only when the maintainer says so (below). From 1.0 on, `feat` bumps the minor again.
- **`Release-As: X.Y.Z` moves the version by hand.** A footer of that form in the message of a
  commit of dev's own line since the last release (a squash's body) sets the next stable version,
  and the betas and alphas that follow it (`0.2.0-beta.N`). It applies when it is above the
  computed one (equal changes nothing; with several, the highest wins), and it is forgotten once
  released, the next release counting from its tag. A value below the computed version, or not
  `X.Y.Z`, is refused: `tools/version.sh` exits 1 naming the commit, and so fails the build until
  the commit is fixed. Example: the Specs pull request is developed on `feat/specs` and reaches
  `dev` as one squash whose body ends with `Release-As: 0.2.0`; the next stable is `0.2.0`
  instead of `0.1.x`, the betas after it `0.2.0-beta.N`, and the release after that `0.2.1`.
- **Above every stable line.** A version of `dev` (and of its branches) is never at or below a
  release a stable line made: when it would be, it is the next minor above the highest of them,
  so a `v0.3.1` made on `stable/0.3` never makes `dev` compute `0.3.x`. A stable line's releases
  are told from `dev`'s by their tag, annotated and naming the line.
- **On a stable line `stable/X.Y`**, it counts from the highest `vX.Y.*` tag of the line's own
  history (the one it started from, or its latest release), and only the patch moves, whatever
  the commits say: `X.Y.(P+1)-rc.N`, then `X.Y.(P+1)`. The patch is also above every `vX.Y.*`
  tag out of its history (a `v0.3.1` `dev` released makes the line `0.3.2`), so a line never
  takes a version `dev` released. `Release-As` is refused there (exit 1, naming the commit).
  **A line with no commit since its tag publishes nothing**: just cut, or right after its
  release, `tools/version.sh` exits 3 on every channel, and CI's push run skips.
- **On `next/X.Y`** it is `X.Y.0-next.N`, `N` counted as for `dev`, and refused (exit 1) when
  `X.Y.0` is not above the version computed as for `dev`: `next` is always above `dev`.
- Several lines at once each count their own (`stable/0.3`, `dev` at `0.4.0`, `next/0.5`).
- **Only the build's own line is read**: the commits counted, the types that bump and the
  changelog follow first parents only, so a release squash that a record of the releases made by
  squash reaches through its second parent is neither counted nor listed, and a merge is one
  commit of that line.
- **The beta and alpha numbers** count those commits (first parents only, so a merge counts
  once): each squash into `dev` makes the next beta, numbered after the previous one, and the
  first beta after a release is `-beta.1`. The record of the 0.1.1 release counts as one beta of
  0.1.2.
- **An alpha names its branch**: each byte of the name but ASCII letters, digits and dashes is a
  dash, `feat/46-packages` giving `alpha.feat-46-packages.<n>` (the branch's slug). CI, its
  cleanup, `install.sh` and `flottille update` follow that one rule. Two names that give the same
  one (`feat/a_b` and `feat/a-b`) share their alphas, both counting in the one series.
- **An alpha's number can go down**: it counts the branch's commits, so a force-push with fewer
  of them gives a lower `N`: a new release, but the highest alpha of the branch (the one
  `install.sh` and `flottille update` take) stays the former one. A force-push with as many
  commits gives a version already published, which is not published again. The numbering stays
  as it is: rewriting a branch is the exception.

`Cargo.toml` keeps its own version; CI gives the computed one to the build
(`FLOTTILLE_BUILD_VERSION`, `FLOTTILLE_BUILD_CHANNEL`, `FLOTTILLE_BUILD_BRANCH`, read by
`crates/flottille/build.rs`). `flottille --version` says the version, the channel and the commit:
`flottille 0.3.0-beta.12 (beta, 3f2a9c1d7b44)`. A build CI did not make is `local`:
`flottille 0.1.0 (local, 3f2a9c1d7b44)`.

## What CI publishes

On a push to `dev`, `stable/*`, `next/*`, `feat/**` or `fix/**`, on `main` or a stable line's
release tag when the `Release` workflow starts it, and on `next/X.Y` when `forward.yml` merged
`dev` into it,
`.github/workflows/ci.yml` runs two jobs beside its checks:

- **`build`** runs the branch's code with a token that can only read. On Ubuntu 22.04, old
  enough a C library (glibc 2.35) that the binary runs on current Arch and Debian stable, it
  computes the version, builds with `cargo build --release --locked -p flottille` and makes:
  - **The archive**: `flottille-<version>-x86_64-linux.tar.gz` (`tools/package.sh`), one folder
    `flottille-<version>/` holding `flottille` (the window included), `LICENSE` and
    `LICENSE-fonts.txt` (the SIL Open Font License of the Geist fonts the window carries), and
    `share/`: the desktop entry and the icon (`packaging/linux/`, see "The launcher entry"), and its
    SHA-256 sum beside it, `<archive>.sha256`, as `sha256sum --check` reads it. The same files
    give the same archive: no owner, no time of the build.
  - **The notes**: the version, the channel and the commit, how to install it with the
    `install.sh` of that very commit, the system libraries the window needs (fontconfig, freetype,
    xkbcommon and xkbcommon-x11, wayland-client or libxcb, a Vulkan loader and driver), then the
    changelog (`tools/changelog.sh`): the Angular commits counted, oldest first, grouped under
    breaking changes, features, fixes, performance and other changes, each
    `- **<scope>**: <subject>` with the pull request number its squash kept.
- **`publish`**, once `verify` passed on the pushed commit (on `main`, here or on `dev`, see
  "Releasing"), runs none of the branch's code: it
  takes what `build` made and alone holds the right to write releases and tags. It runs no git
  and never works inside the artifact's folder, whose files the branch's code made (a
  repository's layout left there would hand git its configuration): only `gh`, naming the files
  it uploads. Right before writing, it reads the branch's tip again through GitHub's API: a
  branch moved on to another commit, or deleted, publishes nothing (the later push publishes
  itself). Then:
  - on `main`, the release `vX.Y.Z` on the commit released, marked latest;
  - on `dev`, the pre-release `vX.Y.Z-beta.N`;
  - on `stable/X.Y`, the pre-release `vX.Y.Z-rc.N`, and on `next/X.Y` the pre-release
    `vX.Y.0-next.N`, their notes installing that very version (`--version`); on a stable line's
    release tag, the release `vX.Y.Z`. The stable channel reads the release marked latest
    (`/releases/latest`), so a line's release is marked latest when its version is above the
    one marked (always so when only the latest line is kept: the fix reaches its users), and
    not when below (an older line's would take the channel back). When the API does not say
    which is latest, nothing is published and the run fails: run again;
  - on a feature branch, the pre-release `vX.Y.Z-alpha.<branch>.N`, one per push, marked
    pre-release and never latest.

  **Nothing is replaced**: the release is made with its files in one `gh release create` (an
  immutable release takes none afterwards), and a version already published, of any channel, is
  left as it is (run again, or a branch pushed over to the same number). A release left as a
  draft by a failed upload is deleted and made again. No alpha is pruned while its branch lives:
  they pile up, one per push, until the branch is deleted.

`.github/workflows/alpha-cleanup.yml` deletes **every** alpha of a branch, with their tags, when
the branch is deleted (an immutable release may be deleted), and the rolling `alpha-<branch>`
release of the former design when one is still there. It shares the branch's `publish` concurrency group, so it waits for one still running,
which then finds the branch gone, or publishes what the cleanup then deletes.

Other targets (aarch64, macOS) come later.

## The branches

| Branch | What it is |
|---|---|
| `main` | the latest stable released |
| `stable/X.Y` | the fix line of a released `X.Y` (see "Stable lines") |
| `dev` | the next version, `X.Y.0-beta.N` (its minor set by `Release-As`) |
| `next/X.Y` | the version after `dev`'s, started early (see "The next version") |
| `feat/<n>-<topic>`, `fix/<n>-<topic>` | a ticket, from the branch its pull request targets; its alphas as on any branch |

`dev`, `stable/*` and `next/*` take pull requests only, merged by squash (but the merges forward
into `next/*`, below): the `pre-commit` hook refuses a commit on them.

## Stable lines

- **`stable/X.Y` is made from the tag `vX.Y.0` (or the latest `vX.Y.*`) only when a fix must
  reach a released `X.Y`**, and the first line is made from a release whose commit holds this
  `ci.yml` (the one that publishes a line's pushes and its release tag): a line runs its own
  commit's `ci.yml`, and an older one publishes a line's push as an alpha and builds no tag. Only the latest stable line is kept: it ends (is deleted) when
  `X.(Y+1).0` is released, and its users move up with `flottille update`. The machinery counts
  several lines at once right; the policy is what keeps one.
- **It receives only backports of fixes already merged into `dev`**: nothing reaches a line that
  is not in `dev`. Its versions and its release: "The version" and "Releasing" above. Its
  changelog lists the line's commits since its previous tag.
- **A backport by label.** A pull request into `dev` labelled `backport X.Y`, once merged (or
  labelled after its merge), is brought by `.github/workflows/backport.yml`: the cherry-pick of
  its squash (`-x`: its message ends with where it was picked from) on `backport/<pr>-X.Y`, a
  branch from `stable/X.Y`, and a pull request into the line, titled as the one it brings, merged
  by squash with the same CI. When the pick does not apply, a comment on the pull request merged
  says so, and nothing is opened. Run again after a failure, it finishes what was left: a branch
  made whose pull request was not is given it (and its `ci.yml`), and the work branch
  `backport-work/<pr>-X.Y` of a run cut short is deleted first (a failure in the pick deletes it
  too). It reads the pull request's `merge_commit_sha` as its squash: `dev` stays squash-only, a
  merge commit there would be brought as one. The job holds a token that writes the repository and runs no
  code of any branch: no checkout, no git, the cherry-pick made through GitHub's API (a commit with
  the line's files on the squash's parent, the squash merged into it, the files that gives put on
  the line's tip). A pull request opened with the workflow's token starts no run of its own, so the
  workflow starts `ci.yml` on the branch (`workflow_dispatch`), whose `verify` is the check of the
  pull request's head.
- **A backport by hand**, when the pick does not apply: `git switch -c backport/<pr>-X.Y
  origin/stable/X.Y`, `git cherry-pick -x <squash>`, the conflicts solved, pushed, and a pull
  request into `stable/X.Y`.

## The next version

- **`next/X.Y` is made from `dev` only when the maintainer starts the version after `dev`'s
  early.** It receives the pull requests aimed at that version (squash, the same CI) **and every
  commit of `dev`**, merged forward by `.github/workflows/forward.yml` after each push to `dev`:
  a merge commit `dev → next/X.Y` (`chore(repo): merge dev into next/X.Y`), never a squash, made
  through GitHub's API (no checkout, no code of any branch), then `ci.yml` started on `next/X.Y`.
- **When `dev` does not merge**, the workflow makes the branch `forward/<dev's commit, 12
  characters>-X.Y` at `dev`'s commit and a pull request from it into `next/X.Y`, and says so in its
  run. The conflict is left to resolve there: on that branch, the merge of `dev`'s commit made with
  `next/X.Y` as its first parent (`git switch -C forward/… origin/next/X.Y`, `git merge <dev's
  commit>`, the conflicts solved, `git push origin HEAD:forward/…`, a push forward since the branch
  is at `dev`'s commit), then the pull request merged with a merge commit, never a squash.
  **While a pull request from a `forward/*` branch into `next/X.Y` is open**, a later push to
  `dev` merges nothing and opens nothing (`dev`'s later commits hold the conflict too): its run
  says so and waits for that merge; the push to `dev` after it brings them. Run again after a
  failure, a branch made whose pull request was not is given it.
- **The `Release` workflow never releases a commit of `next/*`.**
- **When `dev`'s version is released**, `next/X.Y` is merged into `dev` once and deleted, and
  `dev` becomes that version:
  1. release `dev` (`vX.(Y-1).0`, say), as always;
  2. open the pull request `next/X.Y → dev`; its squash's body ends with `Release-As: X.Y.0`, so
     `dev`'s betas are `X.Y.0-beta.N` from it on;
  3. once merged, delete `next/X.Y` (and any `forward/*-X.Y` left).

## GitHub settings

The workflows cannot set these; the maintainer does, once, in the repository's settings:

- **`stable/*`**: pull requests required (with `verify` and `commit-messages`), squash only, no
  force-push, no deletion (a line ends by being deleted by the maintainer).
- **`next/*`**: force-push and deletion blocked, **and nothing else**. Pull requests are not
  required there: `forward.yml` merges `dev` into it through the API, and requiring pull
  requests with a bypass for GitHub Actions would let every workflow holding `contents: write`
  write `next/*`. Its pull requests are reviewed as any other, by the maintainer; the forward
  ones are merged with a merge commit, the others by squash.
- **Tags `v*`**: a ruleset that lets only the maintainer and the `Release` workflow (the
  repository's GitHub Actions) create them, and nobody update or delete them.
- **Labels** `backport X.Y`, one per line kept.

## Installing

```
curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/dev/install.sh | sh -s -- --channel beta
curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/dev/install.sh | sh -s -- --channel alpha --branch feat/46-packages
```

`main` has an `install.sh` once the first stable release is out; until then take `dev`'s, or the
one each release's notes name, of its own commit.

`install.sh` needs `curl`, `tar`, `sha256sum` and GNU `sort`, and no root. Its body is one
function called on its last line, so a download cut short runs nothing. It reads the channel's
release from GitHub's API (stable: the latest release; beta: the `-beta.` pre-release published
last, not the highest version (below); alpha: the **highest version** among the branch's alphas,
`*-alpha.<slug>.<n>` of the 100 newest releases, the slug whole so that `feat-a` never takes
`feat-a-b`'s), downloads
its archive and sum, checks the sum (a wrong one installs nothing),
and puts `flottille` in `~/.local/bin`, written beside it then renamed over it, with `flot` a link
to it, and the launcher entry and the icon in the data folder (below). It refuses while Flottille's daemon runs, as `flottille update` does, and says to stop it
first: it tells by the lock the daemon holds beside its socket (with `flock`, or by the socket
alone where `flock` is missing), without starting or speaking to one. A release that is not there and a network or GitHub failure are said apart. When the new
binary does not answer `--version` (a system library missing), it is installed all the same and
that is said. It says when `~/.local/bin` is not on `PATH`.

## The launcher entry

On Linux, Flottille is in the desktop's application launcher with its icon, without root, in the
user's XDG data folder (`$XDG_DATA_HOME`, `~/.local/share` by default):

- `applications/flottille.desktop`: it runs `flottille window` by the installed binary's full
  path (so it works when the launcher's `PATH` lacks `~/.local/bin`; a path with a space is
  quoted), with `Terminal=false`, the category `Development` and `StartupWMClass=flottille`, the
  window's app id, which is how the desktop finds the entry's icon for the window.
- `icons/hicolor/scalable/apps/flottille.svg`, and `48x48`, `128x128` and `256x256`
  `apps/flottille.png`.

The window also sets the 256 px PNG as its own icon, which GPUI uses on X11; Wayland takes the icon
from the entry its app id names.

The package holds them under `share/`, from `packaging/linux/`. **The PNGs are committed**, not
rendered by CI: the archive then needs no renderer, and the same files give the same archive.
The SVG is `assets/brand/icon-full.svg`, the brand's full icon; `tools/icons.sh` copies it and renders
the PNGs again from it (`rsvg-convert`) when the brand's icon changes. The entry
of the package says `Exec=flottille window`; `install.sh` and `flottille update` write it with the
binary's real path (symbolic links resolved, so that the two write the same line whatever the way
the home is reached). **Refusing some characters is Flottille's choice**: the format allows a
`"`, a backtick, `$`, `\` or `%` in an `Exec` line, escaped, but Flottille writes none of them
(nor a newline) rather than escape: a path that holds one gets no entry, and the install or the
update goes on and says so. When the launcher's files cannot be written (a data folder that is
read-only, a full disk), the install and the update still succeed, since the binary is in place,
and say that the entry was not written. After writing an icon, the `icons/hicolor` folder is
dated now, so that a desktop's cache of the icon theme, stale once the folder is newer than it, is
read again.

**The entry and the icons are Flottille's.** `flottille update` rewrites them from the package, so
a local edit of the entry (a changed `Exec`, an added key) is lost at the next update that finds
it different; to start Flottille another way, make your own entry under another name.

`flottille update` writes both again when they differ from the new package's, and points the
entry's `Exec` at the binary it runs from, so a binary that moved is followed; a build that has
none yet gets them. Nothing is rewritten that is already right. A package older than the entry
holds no `share/`: nothing is written, nothing is removed. When there is nothing to update, the
entry's path is still fixed if it is wrong.

Nothing removes Flottille's installed files today (there is no uninstall), so nothing removes the
entry and the icons: to remove Flottille by hand, delete `flottille` and `flot` from
`~/.local/bin`, `applications/flottille.desktop` and the `flottille.*` icons from the data folder.

## Updating

`flottille update [--channel stable|beta|alpha] [--branch <name>] [--version <X.Y.Z[-pre]>]
[--force]` replaces the binary it runs from with the channel's latest build (the most recently
published, see "Installing"; an alpha's, the highest version of its branch): by default the channel last chosen with `--channel` (and, for alpha,
its branch), else the one it was built on (a `local` build takes stable) and the branch it was
built from; `--branch` goes with `--channel alpha` only. It reads the releases as `install.sh` does, with the user's own `curl`
spawned without a shell, checks the archive's SHA-256 sum (a wrong one replaces nothing), unpacks
it with `tar` in a folder beside the binary and renames the new binary over the old one: the file
is the old one or the new one, never half of one.

- **It refuses while the daemon runs**, and says to stop it first with `flottille daemon stop`
  (its agents stop with it). A daemon left running would go on from a file that is gone, and
  could not start an agent's process, which is this binary too. A client that starts a daemon
  later from a binary replaced while it ran (the window left open) starts it from the new file
  at that path, not from the deleted one.
- **The channel is remembered.** `--channel` (with its branch for an alpha) is kept once the
  update succeeds, in `$XDG_STATE_HOME/flottille/update-channel.json`: it is Flottille's own
  memory, not a setting, so it is not in the configuration. A plain `flottille update` stays on it,
  so `--channel alpha --branch feat/specs` then `--channel beta` goes there and back. `install.sh`
  does not write it: after installing another channel by hand, say `--channel` once.
- **`--version <X.Y.Z[-pre]>` installs that release exactly**, of any channel, as `install.sh
  --version` does, and goes without `--channel` and `--branch`. A version that does not exist is
  refused with the nearest published ones, and nothing is downloaded. It does not touch the
  channel remembered.
- **The same version is not installed again**: it says so and replaces nothing. **An older one
  is refused**, whether it is the latest of the channel (a numbering change left it below this
  build) or the version asked for. `--force` installs either, and says that **the store is not
  migrated down**: a store a newer build migrated is refused by an older one ("made by a newer
  Flottille"). No release says the schema of its build, so Flottille cannot tell before installing
  that the store would be refused: `--force` is the one guard.

## For tests

`FLOTTILLE_RELEASES` names another base of the releases' API than
`https://api.github.com/repos/KristenJestin/flottille`, for `install.sh` and `flottille update`
alike: the tests serve a fake one on `127.0.0.1` (`crates/flottille/tests/release.rs`).

## What only a real push proves

The tests run the version and changelog scripts on throwaway repositories, the releases made by
squash and the fast-forward ones after them included, run the `Release` workflow's script and
`ci.yml`'s version step against a throwaway repository (a `gh` standing for GitHub's API), build
the archive from a binary, and run `install.sh` and `flottille update` against a local server in
a throwaway home. What they cannot run is the workflows themselves on GitHub. A push to a feature branch proves the alpha's: `build` on Ubuntu 22.04,
`publish` with the job's token, a second push publishing the next alpha beside the first,
GitHub's API answering as the scripts read it. Deleting the branch proves its cleanup. The first merge into `dev` proves the
beta's, and the first `Release` run the move of `main` with the workflow's token, the pull
request shown merged, `ci.yml` started on `main` by it, the stable tag on the commit released and
the release marked latest. The first stable line proves the rest of it: its first push an rc,
its first `Release` run the annotated tag made through the API and `ci.yml` started on the tag,
`verified-on-dev` finding the line's run, the release marked latest above the latest; the first label
`backport X.Y` the workflow on `pull_request_target`, the cherry-pick through the API, its pull
request and the `ci.yml` it starts taken as that pull request's check; the first `next/X.Y` the
merges forward through the API and the `ci.yml` they start.
