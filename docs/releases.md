# Releases

Flottille is installed from a built package, in seconds, rather than built with `cargo install`.
Every branch that matters publishes its own build, with a version that says where it comes from.

| Branch | Channel | Version (example) | Published as |
|---|---|---|---|
| `main` | stable | `0.3.0` | a GitHub release, tagged `v0.3.0` |
| `dev` | beta | `0.3.0-beta.12` | a pre-release, tagged `v0.3.0-beta.12` |
| `feat/*`, `fix/*` | alpha | `0.3.0-alpha.feat-46-packages.4` | one rolling pre-release per branch, tagged `alpha-<branch>`, replaced at each push, deleted with the branch |

Nothing is published anywhere but GitHub's releases of the repository.

## Releasing

Every pull request is merged by squash, the release too: releasing is the maintainer's pull
request `dev → main`, merged by squash. Its commit on `main` says nothing of the version, and
holds none of `dev`'s commits, so CI keeps the link itself:

- **The released dev commit is marked.** When `main` publishes `vX.Y.Z`, CI tags the head of
  `dev` that the pull request squashed (its head commit, as GitHub's API gives it) with
  `released/vX.Y.Z`, and tags the squash on `main` `vX.Y.Z`. No tag is pushed by hand.
- **The stable version is the one dev already had**: the version of the beta of that head of
  `dev`, without its pre-release (`0.3.0-beta.12` releases `0.3.0`).
- **Its changelog** lists the commits of `dev` between the previous mark and the new one.
- **The release is recorded in `dev`'s history.** A squash on `main` never reaches `dev`: left
  so, the two share only the repository's first commit, and the next release pull request
  conflicts on every file. Once `vX.Y.Z` is published, CI makes a merge of the squash on the
  head of `dev`, with the message `chore(repo): record the X.Y.Z release in dev's history`, that
  keeps `dev`'s files (what `git merge -s ours` makes) and changes none, and pushes it to `dev`.
  It is the one commit on `dev` no pull request made: CI writes it through GitHub's API with the
  publish job's token, which starts no run, and `tools/branch-guard.sh`, which refuses a commit
  on `dev`, is for people's clones. A `dev` that moved meanwhile gets the merge on its new head,
  still right since `dev`'s files are kept. A run run again finds the squash already in `dev`'s
  history and records nothing.
- **If the record fails** (a push refused three times), the run fails, saying so; the release
  stays published. Record it by hand, the squash being the commit the run was for:

  ```
  git fetch origin
  git switch dev && git merge --ff-only origin/dev
  git merge -s ours -m "chore(repo): record the X.Y.Z release in dev's history" <squash>
  git push origin dev
  ```
- A push to `main` that is not the squash of a pull request from `dev` (none should happen)
  publishes nothing and fails its run, saying why. Should GitHub's API not list the pull request
  of a real release yet, run the run again once it does (`gh run rerun <run id>`, or "Re-run
  jobs" on the run's page): the lookup is made again. A run on a head of `dev` already released
  publishes nothing.

## The version

Computed, never typed (`tools/version.sh stable|beta|alpha [--branch <name>] [--at <commit>]`,
for HEAD by default):

- **It counts from the last release**: the highest stable tag `vX.Y.Z`, compared as versions,
  wherever it is, and from its mark `released/vX.Y.Z` on. The commits counted are those of the
  build's own history that the mark does not hold, so a feature branch started before a release
  counts only its own commits after it. A stable tag without a mark counts from its own commit.
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
  released, the next release counting from its mark. A value below the computed version, or not
  `X.Y.Z`, is refused: `tools/version.sh` exits 1 naming the commit, and so fails the build until
  the commit is fixed. Example: the Specs pull request is developed on `feat/specs` and reaches
  `dev` as one squash whose body ends with `Release-As: 0.2.0`; the next stable is `0.2.0`
  instead of `0.1.x`, the betas after it `0.2.0-beta.N`, and the release after that `0.2.1`.
- **Only the build's own line is read**: the commits counted, the types that bump and the
  changelog follow first parents only, so the release squash a record reaches through its
  second parent is neither counted nor listed, and a merge is one commit of that line.
- **The beta and alpha numbers** count those commits (first parents only, so a merge counts
  once): each squash into `dev` makes the next beta, numbered after the previous one. The record
  of a release counts too, as one: the first beta after a release is `-beta.2`.
- **An alpha names its branch**: each byte of the name but ASCII letters, digits and dashes is a
  dash, `feat/46-packages` giving `alpha.feat-46-packages.<n>` and the tag
  `alpha-feat-46-packages`. CI, its cleanup, `install.sh` and `flottille update` follow that one
  rule. Two names that give the same one (`feat/a_b` and `feat/a-b`) share an alpha: the last
  push wins.
- **An alpha's number can go down**: it counts the branch's commits, so a force-push with fewer
  of them gives a lower `N`, and `flottille update` then calls that newest build older and asks
  for `--force`. The numbering stays as it is: rewriting a branch is the exception.

`Cargo.toml` keeps its own version; CI gives the computed one to the build
(`FLOTTILLE_BUILD_VERSION`, `FLOTTILLE_BUILD_CHANNEL`, `FLOTTILLE_BUILD_BRANCH`, read by
`crates/flottille/build.rs`). `flottille --version` says the version, the channel and the commit:
`flottille 0.3.0-beta.12 (beta, 3f2a9c1d7b44)`. A build CI did not make is `local`:
`flottille 0.1.0 (local, 3f2a9c1d7b44)`.

## What CI publishes

On a push to `main`, `dev`, `feat/**` or `fix/**`, `.github/workflows/ci.yml` runs two jobs
beside its checks:

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
- **`publish`**, once `verify` passed on the pushed commit, runs none of the branch's code: it
  takes what `build` made and alone holds the right to write releases and tags. It runs no git
  and never works inside the artifact's folder, whose files the branch's code made (a
  repository's layout left there would hand git its configuration): only `gh`, naming the files
  it uploads. Right before writing, it reads the branch's tip again through GitHub's API: a
  branch moved on to another commit, or deleted, publishes nothing (the later push publishes
  itself). Then:
  - on `main`, the mark `released/vX.Y.Z`, then the release `vX.Y.Z` on the squash, marked
    latest;
  - on `dev`, the pre-release `vX.Y.Z-beta.N`;
  - on a feature branch, the pre-release `alpha-<branch>`, replaced in place: its tag moves to the
    new commit, the new archive and sum are added (the sum first), the old ones removed, and its
    notes rewritten. The release is never missing. When the new files have the old names (the
    same version, run again or pushed over), a download in that moment can find a sum that is
    not its archive's: it then refuses and installs nothing.

  Run again, a stable or beta release already published is left as it is. A release left as a
  draft by a failed upload, of any channel, is deleted and made again.

`.github/workflows/alpha-cleanup.yml` deletes a branch's alpha, and its tag, when the branch is
deleted. It shares the branch's `publish` concurrency group, so it waits for one still running,
which then finds the branch gone, or publishes what the cleanup then deletes.

Other targets (aarch64, macOS) come later.

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
release from GitHub's API (stable: the latest release; beta: the highest version among the
`-beta.` pre-releases, whatever order they were published in; alpha: the branch's
`alpha-<branch>`, the higher version when it holds two archives while it is replaced), downloads
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
`tools/icons.sh` renders them again from the SVG (`rsvg-convert`) when the SVG changes. The entry
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

`flottille update [--channel stable|beta|alpha] [--branch <name>] [--force]` replaces the binary
it runs from with the channel's latest build: by default the channel it was built on (a `local`
build takes stable), and for alpha the branch it was built from; `--branch` goes with
`--channel alpha` only. It reads the releases as `install.sh` does, with the user's own `curl`
spawned without a shell, checks the archive's SHA-256 sum (a wrong one replaces nothing), unpacks
it with `tar` in a folder beside the binary and renames the new binary over the old one: the file
is the old one or the new one, never half of one.

- **It refuses while the daemon runs**, and says to stop it first with `flottille daemon stop`
  (its agents stop with it). A daemon left running would go on from a file that is gone, and
  could not start an agent's process, which is this binary too. A client that starts a daemon
  later from a binary replaced while it ran (the window left open) starts it from the new file
  at that path, not from the deleted one.
- **The same version is not installed again**: it says so and replaces nothing. **An older one
  is refused.** `--force` installs either.

## For tests

`FLOTTILLE_RELEASES` names another base of the releases' API than
`https://api.github.com/repos/KristenJestin/flottille`, for `install.sh` and `flottille update`
alike: the tests serve a fake one on `127.0.0.1` (`crates/flottille/tests/release.rs`).

## What only a real push proves

The tests run the version and changelog scripts on throwaway repositories, a squash of `dev`
into `main` included, build the archive from a binary, and run `install.sh` and
`flottille update` against a local server in a throwaway home. What they cannot run is the
workflows themselves. A push to a feature branch proves the alpha's: `build` on Ubuntu 22.04,
`publish` with the job's token, the alpha replaced in place, GitHub's API answering as the
scripts read it. Deleting the branch proves its cleanup. The first merge into `dev` proves the
beta's, and the first release pull request into `main` the pull request found from the squash,
the mark, the stable tag and the release marked latest, and the release recorded in `dev`'s
history, through the API, with the job's token.
