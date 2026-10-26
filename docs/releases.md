# Releases

Flottille is installed from a built package, in seconds, rather than built with `cargo install`.
Every branch that matters publishes its own build, with a version that says where it comes from.

| Branch | Channel | Version (example) | Published as |
|---|---|---|---|
| `main` | stable | `0.3.0` | a GitHub release, tagged `v0.3.0` |
| `dev` | beta | `0.3.0-beta.12` | a pre-release, tagged `v0.3.0-beta.12` |
| `feat/*`, `fix/*` | alpha | `0.3.0-alpha.feat-46-packages.4` | one rolling pre-release per branch, tagged `alpha-<branch>`, replaced at each push, deleted with the branch |

Nothing is published anywhere but GitHub's releases of the repository.

## The version

Computed, never typed (`tools/version.sh stable|beta|alpha [--branch <name>]`):

- **It counts from the last stable tag** HEAD holds: a `vX.Y.Z` tag on one of its commits, or on
  the merge of `dev` into `main` whose `dev` side HEAD holds, so that `dev` counts from a release
  as soon as it is tagged. Pre-release tags (`v0.3.0-beta.12`) do not count. Without a stable tag,
  from `0.0.0` and the first commit.
- **The next version** follows the Angular commits since then (`tools/commit-message.sh` is the
  rule they follow), merges left out: `feat` bumps the minor, `fix` and `perf` the patch, a `!`
  after the type or a `BREAKING CHANGE:` footer the major. Before 1.0, a breaking change bumps
  the minor. Other types bump nothing: on `main` there is then nothing to release (the script
  exits 3 and prints nothing, and CI publishes nothing); a beta or an alpha is then the next patch.
- **The beta and alpha numbers** count the commits since the last stable tag (first parents
  only, so a merge counts once): each merge into `dev` makes the next beta, numbered after the
  previous one. An alpha names its branch, every character but letters, digits and dashes made a
  dash: `feat/46-packages` gives `alpha.feat-46-packages.<n>`.

`Cargo.toml` keeps its own version; CI gives the computed one to the build
(`FLOTTILLE_BUILD_VERSION`, `FLOTTILLE_BUILD_CHANNEL`, `FLOTTILLE_BUILD_BRANCH`, read by
`crates/flottille/build.rs`). `flottille --version` says the version, the channel and the commit:
`flottille 0.3.0-beta.12 (beta, 3f2a9c1d7b44)`. A build CI did not make is `local`:
`flottille 0.1.0 (local, 3f2a9c1d7b44)`.

## What CI publishes

The job `publish` of `.github/workflows/ci.yml` runs on a push to `main`, `dev`, `feat/**` or
`fix/**`, once `verify` passed on the pushed commit. It builds with `cargo build --release --locked -p flottille`
on Ubuntu 22.04, old enough a C library (glibc 2.35) that the binary runs on current Arch and
Debian stable, then:

- **The archive**: `flottille-<version>-x86_64-linux.tar.gz` (`tools/package.sh`), one folder
  `flottille-<version>/` holding `flottille` (the window included), `LICENSE` and
  `LICENSE-fonts.txt` (the SIL Open Font License of the Geist fonts the window carries), and its
  SHA-256 sum beside it, `<archive>.sha256`, as `sha256sum --check` reads it. The same files give
  the same archive: no owner, no time of the build.
- **The notes**: the version, the channel and the commit, how to install it, the system libraries
  the window needs (fontconfig, freetype, xkbcommon and xkbcommon-x11, wayland-client or libxcb, a
  Vulkan loader and driver), then the changelog (`tools/changelog.sh`): the Angular commits since
  the last stable tag, oldest first, grouped under breaking changes, features, fixes,
  performance and other changes, each `- **<scope>**: <subject>` with the pull request number its
  squash kept.
- **The release**: on `main`, `vX.Y.Z`, tagged by CI on the pushed commit and marked latest
  (releasing is the maintainer's merge of `dev` into `main`; no tag is pushed by hand); on `dev`,
  the pre-release `vX.Y.Z-beta.N`; on a feature branch, the pre-release `alpha-<branch>`, deleted
  and made again at each push. `.github/workflows/alpha-cleanup.yml` deletes a branch's alpha,
  and its tag, when the branch is deleted.

Other targets (aarch64, macOS) come later.

## Installing

```
curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh -s -- --channel beta
curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh -s -- --channel alpha --branch feat/46-packages
```

`install.sh` needs `curl`, `tar` and `sha256sum`, and no root. It reads the channel's latest
release from GitHub's API (stable: the latest release; beta: the newest `-beta.` pre-release;
alpha: the branch's `alpha-<branch>`), downloads its archive and sum, checks the sum (a wrong one
installs nothing), and puts `flottille` in `~/.local/bin`, written beside it then renamed over
it, with `flot` a link to it. It says when `~/.local/bin` is not on `PATH`.

## Updating

`flottille update [--channel stable|beta|alpha] [--branch <name>]` replaces the binary it runs
from with the channel's latest build: by default the channel it was built on (a `local` build
takes stable), and for alpha the branch it was built from. It reads the releases as `install.sh`
does, with the user's own `curl` spawned without a shell, checks the archive's SHA-256 sum
(a wrong one replaces nothing), unpacks it with `tar` in a folder beside the binary and renames
the new binary over the old one: the file is the old one or the new one, never half of one.

It refuses while an agent starts or works, since their processes are this binary too, and names
the roots to stop (`1 agent is running: … Stop them first (`flottille agent stop <root>`), then
update.`). Without a daemon running, no agent runs. A daemon still running after the update runs
the version before: the command says so, and `flottille daemon stop` lets the next command start
the new one.

## For tests

`FLOTTILLE_RELEASES` names another base of the releases' API than
`https://api.github.com/repos/KristenJestin/flottille`, for `install.sh` and `flottille update`
alike: the tests serve a fake one on `127.0.0.1` (`crates/flottille/tests/release.rs`).

## What only a real push proves

The tests run the version and changelog scripts on throwaway repositories, build the archive from
a binary, and run `install.sh` and `flottille update` against a local server in a throwaway home.
What they cannot run is the workflows themselves: the `publish` job's build on Ubuntu 22.04, `gh
release create` and `delete` with the job's token, the tag CI makes, GitHub's API answering as
the scripts read it, the release marked latest, and the alpha deleted with its branch. The first
push of each channel proves them.
